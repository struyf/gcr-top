use anyhow::{Context, Result};
use reqwest::header::AUTHORIZATION;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use crate::models::{
    LogEntriesResponse, LogEntry, Revision, RevisionListResponse, Service, ServiceListResponse,
};

struct TokenCache {
    token: String,
    fetched_at: Instant,
}

pub struct GcpClient {
    client: reqwest::Client,
    project_id: String,
    region: String,
    token_cache: Arc<RwLock<Option<TokenCache>>>,
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
        let client = reqwest::Client::builder().build()?;

        Ok(Self {
            client,
            project_id,
            region,
            token_cache: Arc::new(RwLock::new(None)),
            run_base_url: run_base_url.trim_end_matches('/').to_string(),
            logging_base_url: logging_base_url.trim_end_matches('/').to_string(),
        })
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

        let new_token = tokio::task::spawn_blocking(Self::fetch_access_token)
            .await
            .context("Failed to join gcloud execution thread")??;

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
                "Failed to execute 'gcloud' CLI: {}. Please ensure Google Cloud SDK is installed and 'gcloud' is in your PATH.",
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
        let token = self.get_valid_token().await?;
        let url = format!(
            "{}/v2/projects/{}/locations/{}/services",
            self.run_base_url, self.project_id, self.region
        );

        let resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                anyhow::bail!(
                    "Google Cloud authentication expired (HTTP 401). Please run 'gcloud auth login'. Details: {}",
                    err_text
                );
            }
            anyhow::bail!("API Error ({}): {}", status, err_text);
        }

        let parsed: ServiceListResponse = resp.json().await?;
        Ok(parsed.services.unwrap_or_default())
    }

    pub async fn fetch_recent_logs(&self, service_name: &str) -> Result<Vec<LogEntry>> {
        let token = self.get_valid_token().await?;
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

        let resp = self
            .client
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                anyhow::bail!(
                    "Google Cloud authentication expired (HTTP 401). Please run 'gcloud auth login'. Details: {}",
                    err
                );
            }
            anyhow::bail!("Logging API Error ({}): {}", status, err);
        }

        let parsed: LogEntriesResponse = resp.json().await?;
        let mut entries = parsed.entries.unwrap_or_default();
        entries.reverse();
        Ok(entries)
    }

    pub async fn list_revisions(&self, service_name: &str) -> Result<Vec<Revision>> {
        let token = self.get_valid_token().await?;
        let url = format!(
            "{}/v2/projects/{}/locations/{}/services/{}/revisions",
            self.run_base_url, self.project_id, self.region, service_name
        );

        let resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                anyhow::bail!(
                    "Google Cloud authentication expired (HTTP 401). Please run 'gcloud auth login'. Details: {}",
                    err_text
                );
            }
            anyhow::bail!("Revisions API Error ({}): {}", status, err_text);
        }

        let parsed: RevisionListResponse = resp.json().await?;
        Ok(parsed.revisions.unwrap_or_default())
    }

    /// Update traffic split allocation via Cloud Run Admin v2 API
    pub async fn set_traffic_split(
        &self,
        service_name: &str,
        splits: Vec<(&str, i32)>, // (revision_id, percent)
    ) -> Result<()> {
        let token = self.get_valid_token().await?;
        let url = format!(
            "{}/v2/projects/{}/locations/{}/services/{}?updateMask=traffic",
            self.run_base_url, self.project_id, self.region, service_name
        );

        let traffic_array: Vec<serde_json::Value> = splits
            .into_iter()
            .map(|(rev, pct)| {
                serde_json::json!({
                    "revision": rev,
                    "percent": pct,
                    "type": "TRAFFIC_TARGET_ALLOCATION_TYPE_REVISION"
                })
            })
            .collect();

        let body = serde_json::json!({
            "traffic": traffic_array
        });

        let resp = self
            .client
            .patch(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                anyhow::bail!(
                    "Google Cloud authentication expired (HTTP 401). Please run 'gcloud auth login'. Details: {}",
                    err
                );
            }
            anyhow::bail!("Failed to update traffic ({}): {}", status, err);
        }

        Ok(())
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

        let revisions = client.list_revisions("web").await.expect("Failed to fetch revisions");
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].short_name(), "web-00002-xyz");
        assert_eq!(revisions[1].short_name(), "web-00001-abc");
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

        let result = client.list_revisions("web").await;
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
}
