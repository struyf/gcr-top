use anyhow::{Context, Result};
use reqwest::header::AUTHORIZATION;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use crate::models::{
    LogEntriesResponse, LogEntry, Revision, RevisionListResponse, Service, ServiceListResponse,
};

pub fn format_actionable_error(status: reqwest::StatusCode, action: &str, raw_body: &str) -> String {
    let parsed_message = serde_json::from_str::<serde_json::Value>(raw_body)
        .ok()
        .and_then(|val| {
            val.get("error")
                .and_then(|err| err.get("message"))
                .and_then(|msg| msg.as_str())
                .map(|s| s.to_string())
        });

    let details = parsed_message.unwrap_or_else(|| {
        let trimmed = raw_body.trim();
        if trimmed.is_empty() {
            "No additional details provided by API.".to_string()
        } else {
            trimmed.to_string()
        }
    });

    match status {
        reqwest::StatusCode::UNAUTHORIZED => {
            format!(
                "Google Cloud authentication expired (HTTP 401). Please run 'gcloud auth login' to authenticate. Details: {}",
                details
            )
        }
        reqwest::StatusCode::FORBIDDEN => {
            format!(
                "Permission Denied (HTTP 403). Please ensure your active GCP account has the 'Cloud Run Developer' (roles/run.developer) or 'Cloud Run Admin' role. Details: {}",
                details
            )
        }
        reqwest::StatusCode::NOT_FOUND => {
            format!(
                "Resource not found (HTTP 404). The service or revision does not exist or may have been deleted. Details: {}",
                details
            )
        }
        reqwest::StatusCode::CONFLICT => {
            format!(
                "Conflict detected (HTTP 409). The service configuration was modified concurrently. Please refresh [r] and retry. Details: {}",
                details
            )
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            format!(
                "Google Cloud API rate limit exceeded (HTTP 429). Please retry shortly. Details: {}",
                details
            )
        }
        reqwest::StatusCode::BAD_REQUEST => {
            format!(
                "Invalid request (HTTP 400). Please check your configuration parameters. Details: {}",
                details
            )
        }
        s if s.is_server_error() => {
            format!(
                "Google Cloud service temporarily unavailable (HTTP {}). Please check GCP status and retry. Details: {}",
                s, details
            )
        }
        _ => {
            format!("Failed to {} (HTTP {}): {}", action, status, details)
        }
    }
}

struct TokenCache {
    token: String,
    fetched_at: Instant,
}

type TokenProvider = Arc<dyn Fn() -> Result<String> + Send + Sync>;

pub struct GcpClient {
    client: reqwest::Client,
    project_id: String,
    region: String,
    token_cache: Arc<RwLock<Option<TokenCache>>>,
    token_provider: Option<TokenProvider>,
    run_base_url: String,
    logging_base_url: String,
}

impl GcpClient {
    pub fn new(project_id: String, region: String) -> Result<Self> {
        Self::with_base_urls(
            project_id,
            region,
            "https://run.googleapis.com".to_string(),
            "https://logging.googleapis.com".to_string(),
        )
    }

    pub fn with_base_urls(
        project_id: String,
        region: String,
        run_base_url: String,
        logging_base_url: String,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .build()?;

        Ok(Self {
            client,
            project_id,
            region,
            token_cache: Arc::new(RwLock::new(None)),
            token_provider: None,
            run_base_url: run_base_url.trim_end_matches('/').to_string(),
            logging_base_url: logging_base_url.trim_end_matches('/').to_string(),
        })
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    #[allow(dead_code)]
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    #[allow(dead_code)]
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token_cache = Arc::new(RwLock::new(Some(TokenCache {
            token: token.into(),
            fetched_at: Instant::now(),
        })));
        self
    }

    #[allow(dead_code)]
    pub fn with_token_provider<F>(mut self, provider: F) -> Self
    where
        F: Fn() -> Result<String> + Send + Sync + 'static,
    {
        self.token_provider = Some(Arc::new(provider));
        self
    }

    pub async fn invalidate_token(&self) {
        let mut cache = self.token_cache.write().await;
        *cache = None;
    }

    #[allow(dead_code)]
    pub async fn set_cached_token(&self, token: impl Into<String>) {
        let mut cache = self.token_cache.write().await;
        *cache = Some(TokenCache {
            token: token.into(),
            fetched_at: Instant::now(),
        });
    }

    pub async fn get_valid_token(&self) -> Result<String> {
        // Read lock check
        {
            let cache = self.token_cache.read().await;
            if let Some(c) = cache.as_ref().filter(|c| c.fetched_at.elapsed() < Duration::from_secs(3000)) {
                return Ok(c.token.clone());
            }
        }

        // Write lock check for refresh
        let mut cache = self.token_cache.write().await;
        if let Some(c) = cache.as_ref().filter(|c| c.fetched_at.elapsed() < Duration::from_secs(3000)) {
            return Ok(c.token.clone());
        }

        let new_token = if let Some(ref provider) = self.token_provider {
            provider()?
        } else {
            tokio::task::spawn_blocking(Self::fetch_access_token)
                .await
                .context("Failed to join gcloud execution thread")??
        };

        *cache = Some(TokenCache {
            token: new_token.clone(),
            fetched_at: Instant::now(),
        });

        Ok(new_token)
    }

    fn fetch_access_token() -> Result<String> {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "gcloud auth print-access-token"]);
            c
        } else {
            let mut c = Command::new("gcloud");
            c.args(["auth", "print-access-token"]);
            c
        };

        let output = cmd.output().map_err(|e| {
            anyhow::anyhow!(
                "Failed to execute 'gcloud' CLI: {}. Please ensure Google Cloud SDK is installed and 'gcloud' is in your PATH. Please run 'gcloud auth login' to authenticate.",
                e
            )
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                "Google Cloud authentication failed. Please run 'gcloud auth login' to authenticate.".to_string()
            } else if stderr.contains("login") || stderr.contains("credentials") || stderr.contains("active account") {
                format!("Google Cloud authentication required: {}. Please run 'gcloud auth login'.", stderr)
            } else {
                format!("gcloud auth print-access-token failed: {}. Please run 'gcloud auth login'.", stderr)
            };
            anyhow::bail!(msg);
        }

        let token = String::from_utf8(output.stdout)
            .context("Invalid UTF-8 in gcloud token output")?
            .trim()
            .to_string();

        if token.is_empty() {
            anyhow::bail!("Received empty access token from gcloud. Please run 'gcloud auth login'.");
        }

        Ok(token)
    }

    pub async fn list_services(&self) -> Result<Vec<Service>> {
        let url = format!(
            "{}/v2/projects/{}/locations/{}/services",
            self.run_base_url, self.project_id, self.region
        );

        let mut token = self.get_valid_token().await?;
        let mut resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.invalidate_token().await;
            token = self.get_valid_token().await?;
            resp = self
                .client
                .get(&url)
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .send()
                .await?;
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            anyhow::bail!(format_actionable_error(status, "list services", &err_text));
        }

        let parsed: ServiceListResponse = resp.json().await?;
        Ok(parsed.services.unwrap_or_default())
    }

    pub async fn fetch_recent_logs(&self, service_name: &str) -> Result<Vec<LogEntry>> {
        let url = format!("{}/v2/entries:list", self.logging_base_url);

        let filter = format!(
            "resource.type=\"cloud_run_revision\" AND resource.labels.service_name=\"{}\" AND timestamp >= \"{}\"",
            service_name,
            (chrono::Utc::now() - chrono::Duration::minutes(20)).to_rfc3339()
        );

        let body = serde_json::json!({
            "resourceNames": [format!("projects/{}", self.project_id)],
            "filter": filter,
            "orderBy": "timestamp desc",
            "pageSize": 50
        });

        let mut token = self.get_valid_token().await?;
        let mut resp = self
            .client
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.invalidate_token().await;
            token = self.get_valid_token().await?;
            resp = self
                .client
                .post(&url)
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .json(&body)
                .send()
                .await?;
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!(format_actionable_error(status, "fetch logs", &err));
        }

        let parsed: LogEntriesResponse = resp.json().await?;
        let mut entries = parsed.entries.unwrap_or_default();
        entries.reverse();
        Ok(entries)
    }

    #[allow(dead_code)]
    pub async fn get_logs(&self, service_name: &str) -> Result<Vec<LogEntry>> {
        self.fetch_recent_logs(service_name).await
    }

    pub async fn list_revisions(&self, service_name: &str, location: &str) -> Result<Vec<Revision>> {
        let parts: Vec<&str> = service_name.split('/').collect();
        let is_full_resource = parts.len() >= 6
            && parts[0] == "projects"
            && parts[2] == "locations"
            && parts[4] == "services";

        let proj = if is_full_resource {
            parts[1]
        } else {
            &self.project_id
        };

        let loc = if !location.is_empty() && location != "-" {
            location
        } else if is_full_resource && parts[3] != "-" && !parts[3].is_empty() {
            parts[3]
        } else {
            &self.region
        };

        if loc == "-" || loc.is_empty() {
            anyhow::bail!(
                "Cannot list revisions for service '{}': concrete location is required (aggregate region '-' cannot be used). Please specify a valid region.",
                service_name
            );
        }

        let clean_service_name = if is_full_resource {
            parts[5]
        } else {
            service_name.rsplit('/').next().unwrap_or(service_name)
        };

        let url = format!(
            "{}/v2/projects/{}/locations/{}/services/{}/revisions",
            self.run_base_url, proj, loc, clean_service_name
        );

        let mut token = self.get_valid_token().await?;
        let mut resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.invalidate_token().await;
            token = self.get_valid_token().await?;
            resp = self
                .client
                .get(&url)
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .send()
                .await?;
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            anyhow::bail!(format_actionable_error(status, "list revisions", &err_text));
        }

        let parsed: RevisionListResponse = resp.json().await?;
        Ok(parsed.revisions.unwrap_or_default())
    }

    /// Update traffic split allocation with optional revision tags via Cloud Run Admin v2 API
    pub async fn set_traffic_split_with_tags(
        &self,
        service_name: &str,
        splits: Vec<(&str, i32, Option<&str>)>, // (revision_id, percent, tag)
    ) -> Result<()> {
        let parts: Vec<&str> = service_name.split('/').collect();
        let is_full_resource = parts.len() >= 6
            && parts[0] == "projects"
            && parts[2] == "locations"
            && parts[4] == "services";

        let proj = if is_full_resource {
            parts[1]
        } else {
            &self.project_id
        };

        let loc = if self.region == "-" && is_full_resource && parts[3] != "-" && !parts[3].is_empty() {
            parts[3]
        } else {
            &self.region
        };

        let clean_service_name = if is_full_resource {
            parts[5]
        } else {
            service_name.rsplit('/').next().unwrap_or(service_name)
        };

        let url = format!(
            "{}/v2/projects/{}/locations/{}/services/{}?updateMask=traffic",
            self.run_base_url, proj, loc, clean_service_name
        );

        let traffic_array: Vec<serde_json::Value> = splits
            .into_iter()
            .map(|(rev, pct, tag)| {
                let mut item = serde_json::json!({
                    "revision": rev,
                    "percent": pct,
                    "type": "TRAFFIC_TARGET_ALLOCATION_TYPE_REVISION"
                });
                if let Some(t) = tag.filter(|t| !t.trim().is_empty() && *t != "-") {
                    item["tag"] = serde_json::Value::String(t.trim().to_string());
                }
                item
            })
            .collect();

        let body = serde_json::json!({
            "traffic": traffic_array
        });

        let mut token = self.get_valid_token().await?;
        let mut resp = self
            .client
            .patch(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.invalidate_token().await;
            token = self.get_valid_token().await?;
            resp = self
                .client
                .patch(&url)
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .json(&body)
                .send()
                .await?;
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!(format_actionable_error(status, "update traffic split", &err));
        }

        Ok(())
    }

    /// Update traffic split allocation via Cloud Run Admin v2 API
    #[allow(dead_code)]
    pub async fn set_traffic_split(
        &self,
        service_name: &str,
        splits: Vec<(&str, i32)>, // (revision_id, percent)
    ) -> Result<()> {
        let with_tags = splits.into_iter().map(|(rev, pct)| (rev, pct, None)).collect();
        self.set_traffic_split_with_tags(service_name, with_tags).await
    }

    /// Update traffic split allocation via Cloud Run Admin v2 API
    #[allow(dead_code)]
    pub async fn set_traffic(
        &self,
        service_name: &str,
        splits: Vec<(&str, i32)>,
    ) -> Result<()> {
        self.set_traffic_split(service_name, splits).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_list_services_success() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        let response_json = serde_json::json!({
            "services": [
                {
                    "name": "projects/test-project/locations/us-central1/services/my-api",
                    "uri": "https://my-api-xyz.run.app",
                    "latestReadyRevision": "projects/test-project/locations/us-central1/services/my-api/revisions/my-api-00001",
                    "trafficStatuses": [
                        {
                            "revision": "projects/test-project/locations/us-central1/services/my-api/revisions/my-api-00001",
                            "percent": 100
                        }
                    ]
                }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services"))
            .and(header("Authorization", "Bearer test-mock-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_json))
            .mount(&server)
            .await;

        let services = client.list_services().await.expect("Failed to fetch services");
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].short_name(), "my-api");
        assert_eq!(services[0].primary_revision(), "my-api-00001");
        assert_eq!(services[0].traffic_summary(), "100%");
    }

    #[tokio::test]
    async fn test_list_services_http_error() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&server)
            .await;

        let result = client.list_services().await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("500"));
    }

    #[tokio::test]
    async fn test_list_services_unauthorized() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthenticated"))
            .mount(&server)
            .await;

        let result = client.list_services().await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("gcloud auth login"));
    }

    #[tokio::test]
    async fn test_list_services_rate_limited() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services"))
            .respond_with(ResponseTemplate::new(429).set_body_string("Rate limit exceeded"))
            .mount(&server)
            .await;

        let result = client.list_services().await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("429"));
        assert!(err.contains("rate limit exceeded"));
    }

    #[tokio::test]
    async fn test_fetch_recent_logs_success() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        let response_json = serde_json::json!({
            "entries": [
                {
                    "textPayload": "Log entry 2",
                    "severity": "INFO",
                    "timestamp": "2024-01-01T00:00:02Z"
                },
                {
                    "textPayload": "Log entry 1",
                    "severity": "INFO",
                    "timestamp": "2024-01-01T00:00:01Z"
                }
            ]
        });

        Mock::given(method("POST"))
            .and(path("/v2/entries:list"))
            .and(header("Authorization", "Bearer test-mock-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_json))
            .mount(&server)
            .await;

        let logs = client.fetch_recent_logs("my-service").await.expect("Failed to fetch logs");
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].message().unwrap(), "Log entry 1");
        assert_eq!(logs[1].message().unwrap(), "Log entry 2");
    }

    #[tokio::test]
    async fn test_fetch_recent_logs_error() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("POST"))
            .and(path("/v2/entries:list"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Permission Denied"))
            .mount(&server)
            .await;

        let result = client.fetch_recent_logs("my-service").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("403"));
    }

    #[tokio::test]
    async fn test_list_revisions_success() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        let response_json = serde_json::json!({
            "revisions": [
                {
                    "name": "projects/test-project/locations/us-central1/services/web/revisions/web-00002-xyz",
                    "createTime": "2024-01-01T12:00:00Z"
                },
                {
                    "name": "projects/test-project/locations/us-central1/services/web/revisions/web-00001-abc",
                    "createTime": "2024-01-01T10:00:00Z"
                }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web/revisions"))
            .and(header("Authorization", "Bearer test-mock-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_json))
            .mount(&server)
            .await;

        let revisions = client.list_revisions("web", "us-central1").await.expect("Failed to fetch revisions");
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].short_name(), "web-00002-xyz");
        assert_eq!(revisions[1].short_name(), "web-00001-abc");
    }

    #[tokio::test]
    async fn test_list_revisions_with_aggregate_region_uses_concrete_location() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "-".to_string(), // Aggregate region
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        let response_json = serde_json::json!({
            "revisions": [
                {
                    "name": "projects/test-project/locations/europe-west1/services/web/revisions/web-00001",
                    "createTime": "2024-01-01T10:00:00Z"
                }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/europe-west1/services/web/revisions"))
            .and(header("Authorization", "Bearer test-mock-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_json))
            .mount(&server)
            .await;

        let revisions = client
            .list_revisions("web", "europe-west1")
            .await
            .expect("Failed to fetch revisions");
        assert_eq!(revisions.len(), 1);
        assert_eq!(revisions[0].short_name(), "web-00001");
    }

    #[tokio::test]
    async fn test_list_revisions_with_full_resource_name() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "default-proj".to_string(),
            "-".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        let response_json = serde_json::json!({
            "revisions": [
                {
                    "name": "projects/custom-proj/locations/asia-northeast1/services/api/revisions/api-00001",
                    "createTime": "2024-01-01T10:00:00Z"
                }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/v2/projects/custom-proj/locations/asia-northeast1/services/api/revisions"))
            .and(header("Authorization", "Bearer test-mock-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_json))
            .mount(&server)
            .await;

        let revisions = client
            .list_revisions("projects/custom-proj/locations/asia-northeast1/services/api", "")
            .await
            .expect("Failed to fetch revisions");
        assert_eq!(revisions.len(), 1);
        assert_eq!(revisions[0].short_name(), "api-00001");
    }

    #[tokio::test]
    async fn test_list_revisions_missing_concrete_location_fails() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "-".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        let result = client.list_revisions("web", "-").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("concrete location is required"));
    }

    #[tokio::test]
    async fn test_list_revisions_http_error() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web/revisions"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Service Not Found"))
            .mount(&server)
            .await;

        let result = client.list_revisions("web", "us-central1").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("404"));
    }

    #[tokio::test]
    async fn test_set_traffic_split_success() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("PATCH"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web"))
            .and(query_param("updateMask", "traffic"))
            .and(header("Authorization", "Bearer test-mock-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let splits = vec![("web-00002-xyz", 80), ("web-00001-abc", 20)];
        let result = client.set_traffic_split("web", splits).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_set_traffic_split_with_tags_success() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("PATCH"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web"))
            .and(query_param("updateMask", "traffic"))
            .and(header("Authorization", "Bearer test-mock-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let splits = vec![
            ("web-00002-xyz", 80, Some("candidate")),
            ("web-00001-abc", 20, None),
        ];
        let result = client.set_traffic_split_with_tags("web", splits).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_set_traffic_split_error() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("PATCH"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web"))
            .and(query_param("updateMask", "traffic"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Total percentage must be 100"))
            .mount(&server)
            .await;

        let splits = vec![("web-00002-xyz", 50)];
        let result = client.set_traffic_split("web", splits).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("400"));
    }

    #[tokio::test]
    async fn test_set_traffic_split_rate_limited() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("PATCH"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web"))
            .and(query_param("updateMask", "traffic"))
            .respond_with(ResponseTemplate::new(429).set_body_string("Quota limit exceeded"))
            .mount(&server)
            .await;

        let splits = vec![("web-00002-xyz", 100)];
        let result = client.set_traffic_split("web", splits).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("429"));
        assert!(err.contains("rate limit exceeded"));
    }

    #[tokio::test]
    async fn test_set_traffic_split_permission_denied_403() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("PATCH"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web"))
            .and(query_param("updateMask", "traffic"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": {
                    "code": 403,
                    "message": "The caller does not have permission 'run.services.update'",
                    "status": "PERMISSION_DENIED"
                }
            })))
            .mount(&server)
            .await;

        let splits = vec![("web-00002-xyz", 100)];
        let result = client.set_traffic_split("web", splits).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Permission Denied"));
        assert!(err.contains("Cloud Run Developer"));
    }

    #[tokio::test]
    async fn test_set_traffic_split_conflict_409() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("test-mock-token");

        Mock::given(method("PATCH"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web"))
            .and(query_param("updateMask", "traffic"))
            .respond_with(ResponseTemplate::new(409).set_body_string("Resource version mismatch"))
            .mount(&server)
            .await;

        let splits = vec![("web-00002-xyz", 100)];
        let result = client.set_traffic_split("web", splits).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("409"));
        assert!(err.contains("Conflict detected"));
    }

    #[test]
    fn test_format_actionable_error_json_unpacking() {
        let raw_json = serde_json::json!({
            "error": {
                "code": 403,
                "message": "Caller lacks run.services.update permission",
                "status": "PERMISSION_DENIED"
            }
        })
        .to_string();

        let err_msg = format_actionable_error(reqwest::StatusCode::FORBIDDEN, "update traffic", &raw_json);
        assert!(err_msg.contains("Permission Denied"));
        assert!(err_msg.contains("Cloud Run Developer"));
        assert!(err_msg.contains("Caller lacks run.services.update permission"));
    }
    #[tokio::test]
    async fn test_invalidate_token_clears_cache() {
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            "http://localhost".to_string(),
            "http://localhost".to_string(),
        )
        .unwrap()
        .with_token("initial-token");

        assert_eq!(client.get_valid_token().await.unwrap(), "initial-token");
        client.invalidate_token().await;

        let counter = std::sync::atomic::AtomicUsize::new(0);
        let client = client.with_token_provider(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("refreshed-token".to_string())
        });
        assert_eq!(client.get_valid_token().await.unwrap(), "refreshed-token");
    }

    #[tokio::test]
    async fn test_list_services_unauthorized_retries_and_succeeds() {
        let server = MockServer::start().await;
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cc = call_count.clone();

        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("expired-token")
        .with_token_provider(move || {
            cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("fresh-token".to_string())
        });

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services"))
            .and(header("Authorization", "Bearer expired-token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Expired token"))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services"))
            .and(header("Authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "services": [
                    {
                        "name": "projects/test-project/locations/us-central1/services/my-api"
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let services = client.list_services().await.expect("Expected retry to succeed");
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].short_name(), "my-api");
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_list_services_unauthorized_fails_after_single_retry() {
        let server = MockServer::start().await;
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cc = call_count.clone();

        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("expired-token")
        .with_token_provider(move || {
            cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("still-unauthorized-token".to_string())
        });

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services"))
            .and(header("Authorization", "Bearer expired-token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Expired token"))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services"))
            .and(header("Authorization", "Bearer still-unauthorized-token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Still unauthorized"))
            .expect(1)
            .mount(&server)
            .await;

        let result = client.list_services().await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("401"));
        assert!(err.contains("gcloud auth login"));
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_list_revisions_unauthorized_retries_and_succeeds() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("expired-token")
        .with_token_provider(|| Ok("fresh-token".to_string()));

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web/revisions"))
            .and(header("Authorization", "Bearer expired-token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web/revisions"))
            .and(header("Authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "revisions": [
                    {
                        "name": "projects/test-project/locations/us-central1/services/web/revisions/web-001"
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let revs = client.list_revisions("web", "us-central1").await.expect("Expected retry to succeed");
        assert_eq!(revs.len(), 1);
        assert_eq!(revs[0].short_name(), "web-001");
    }

    #[tokio::test]
    async fn test_fetch_recent_logs_unauthorized_retries_and_succeeds() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("expired-token")
        .with_token_provider(|| Ok("fresh-token".to_string()));

        Mock::given(method("POST"))
            .and(path("/v2/entries:list"))
            .and(header("Authorization", "Bearer expired-token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v2/entries:list"))
            .and(header("Authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": [
                    {
                        "textPayload": "Test log output",
                        "severity": "INFO",
                        "timestamp": "2024-01-01T00:00:00Z"
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let logs = client.get_logs("web").await.expect("Expected get_logs retry to succeed");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message().unwrap(), "Test log output");
    }

    #[tokio::test]
    async fn test_set_traffic_unauthorized_retries_and_succeeds() {
        let server = MockServer::start().await;
        let client = GcpClient::with_base_urls(
            "test-project".to_string(),
            "us-central1".to_string(),
            server.uri(),
            server.uri(),
        )
        .unwrap()
        .with_token("expired-token")
        .with_token_provider(|| Ok("fresh-token".to_string()));

        Mock::given(method("PATCH"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web"))
            .and(query_param("updateMask", "traffic"))
            .and(header("Authorization", "Bearer expired-token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("PATCH"))
            .and(path("/v2/projects/test-project/locations/us-central1/services/web"))
            .and(query_param("updateMask", "traffic"))
            .and(header("Authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let splits = vec![("web-001", 100)];
        let res = client.set_traffic("web", splits).await;
        assert!(res.is_ok());
    }
}
