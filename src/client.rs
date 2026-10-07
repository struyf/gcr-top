use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{Duration, Instant};
use crate::models::{LogEntriesResponse, LogEntry, Service, ServiceListResponse};

struct TokenCache {
    token: String,
    fetched_at: Instant,
}

pub struct GcpClient {
    client: reqwest::Client,
    project_id: String,
    region: String,
    token_cache: Arc<RwLock<Option<TokenCache>>>,
}

impl GcpClient {
    pub fn new(project_id: String, region: String) -> Result<Self> {
        let client = reqwest::Client::builder().build()?;

        Ok(Self {
            client,
            project_id,
            region,
            token_cache: Arc::new(RwLock::new(None)),
        })
    }

    async fn get_valid_token(&self) -> Result<String> {
        // Lees lock
        {
            let cache = self.token_cache.read().await;
            if let Some(ref c) = *cache {
                // Tokens van Google zijn 3600s geldig; vernieuw na 50 minuten (3000s)
                if c.fetched_at.elapsed() < Duration::from_secs(3000) {
                    return Ok(c.token.clone());
                }
            }
        }

        // Schrijf lock voor refresh
        let mut cache = self.token_cache.write().await;
        // Dubbelcheck na verkrijgen lock
        if let Some(ref c) = *cache {
            if c.fetched_at.elapsed() < Duration::from_secs(3000) {
                return Ok(c.token.clone());
            }
        }

        let new_token = tokio::task::spawn_blocking(Self::fetch_access_token)
            .await??;

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

        let output = cmd.output().context("Failed to execute gcloud command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("gcloud auth print-access-token failed: {}", stderr.trim());
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    pub async fn list_services(&self) -> Result<Vec<Service>> {
        let token = self.get_valid_token().await?;
        let url = format!(
            "https://run.googleapis.com/v2/projects/{}/locations/{}/services",
            self.project_id, self.region
        );

        let resp = self.client.get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .send()
            .await?;

        if !resp.status().is_success() {
            let err_text = resp.text().await?;
            anyhow::bail!("API Error: {}", err_text);
        }

        let parsed: ServiceListResponse = resp.json().await?;
        Ok(parsed.services.unwrap_or_default())
    }

    pub async fn fetch_recent_logs(&self, service_name: &str) -> Result<Vec<LogEntry>> {
        let token = self.get_valid_token().await?;
        let url = "https://logging.googleapis.com/v2/entries:list";
        
        // Alleen logs van de afgelopen 20 minuten ophalen:
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

        let resp = self.client.post(url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Logging API Error: {}", err);
        }

        let parsed: LogEntriesResponse = resp.json().await?;
        let mut entries = parsed.entries.unwrap_or_default();
        entries.reverse();
        Ok(entries)
    }

    /// Nieuw: Voeg direct een traffic update call toe via Cloud Run Admin v2 API
    pub async fn set_traffic_split(
        &self,
        service_name: &str,
        splits: Vec<(&str, i32)>, // (revision_id, percent)
    ) -> Result<()> {
        let token = self.get_valid_token().await?;
        let url = format!(
            "https://run.googleapis.com/v2/projects/{}/locations/{}/services/{}?updateMask=traffic",
            self.project_id, self.region, service_name
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

        let resp = self.client.patch(&url)
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Failed to update traffic: {}", err);
        }

        Ok(())
    }
}
