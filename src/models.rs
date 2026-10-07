use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct ServiceListResponse {
    pub services: Option<Vec<Service>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Service {
    pub name: String,
    pub uri: Option<String>,
    #[serde(rename = "latestReadyRevision")]
    pub latest_ready_revision: Option<String>,
    pub conditions: Option<Vec<Condition>>,
    #[serde(rename = "trafficStatuses")]
    pub traffic_statuses: Option<Vec<TrafficStatus>>,
}

impl Service {
    pub fn short_name(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
    }

    pub fn is_ready(&self) -> bool {
        if let Some(ref conditions) = self.conditions {
            conditions.iter().any(|c| c.state.as_deref() == Some("CONDITION_SUCCEEDED"))
        } else {
            self.latest_ready_revision.is_some()
        }
    }

    pub fn primary_revision(&self) -> &str {
        self.latest_ready_revision
            .as_ref()
            .and_then(|r| r.rsplit('/').next())
            .unwrap_or("-")
    }

    /// Formats traffic for compact table display: e.g. "100%" or "80/20 (Split)"
    pub fn traffic_summary(&self) -> String {
        match &self.traffic_statuses {
            Some(statuses) if !statuses.is_empty() => {
                let valid_splits: Vec<String> = statuses
                    .iter()
                    .filter_map(|t| t.percent.map(|p| format!("{}%", p)))
                    .collect();

                if valid_splits.len() > 1 {
                    format!("{} (Split)", valid_splits.join("/"))
                } else if let Some(first) = valid_splits.first() {
                    first.clone()
                } else {
                    "100%".to_string()
                }
            }
            _ => "100%".to_string(),
        }
    }

    /// Returns clean (Revision, Percent, Tag) tuples for detail rendering
    pub fn traffic_details(&self) -> Vec<(String, i32, String)> {
        match &self.traffic_statuses {
            Some(statuses) => statuses
                .iter()
                .map(|t| {
                    let rev = t
                        .revision
                        .as_deref()
                        .and_then(|r| r.rsplit('/').next())
                        .unwrap_or("latest")
                        .to_string();
                    let pct = t.percent.unwrap_or(0);
                    let tag = t.tag.as_deref().unwrap_or("-").to_string();
                    (rev, pct, tag)
                })
                .collect(),
            None => vec![(self.primary_revision().to_string(), 100, "-".to_string())],
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Condition {
    #[serde(rename = "type")]
    pub type_field: String,
    pub state: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TrafficStatus {
    pub revision: Option<String>,
    pub percent: Option<i32>,
    pub tag: Option<String>,
    #[serde(rename = "type")]
    pub traffic_type: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LogEntriesResponse {
    pub entries: Option<Vec<LogEntry>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpRequestLog {
    #[serde(rename = "requestMethod")]
    pub request_method: Option<String>,
    #[serde(rename = "requestUrl")]
    pub request_url: Option<String>,
    pub status: Option<i32>,
    pub latency: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LogEntry {
    #[serde(rename = "textPayload")]
    pub text_payload: Option<String>,
    #[serde(rename = "jsonPayload")]
    pub json_payload: Option<serde_json::Value>,
    #[serde(rename = "httpRequest")]
    pub http_request: Option<HttpRequestLog>,
    pub severity: Option<String>,
    pub timestamp: Option<String>,
}

impl LogEntry {
    pub fn message(&self) -> Option<String> {
        if let Some(ref http) = self.http_request {
            let method = http.request_method.as_deref().unwrap_or("GET");
            let status = http.status.unwrap_or(200);
            let url = http.request_url.as_deref().unwrap_or("/");
            let path = url.split('/').skip(3).collect::<Vec<&str>>().join("/");
            let display_path = if path.is_empty() { "/" } else { &path };
            
            // Format latency naar ms indien mogelijk
            let latency_display = if let Some(ref lat) = http.latency {
                lat.trim_end_matches('s')
                    .parse::<f64>()
                    .map(|s| format!("{:.1}ms", s * 1000.0))
                    .unwrap_or_else(|_| lat.clone())
            } else {
                "-".to_string()
            };

            Some(format!("{} {} {} {}", method, status, display_path, latency_display))
        } else if let Some(ref text) = self.text_payload {
            Some(text.trim().to_string())
        } else if let Some(ref json) = self.json_payload {
            if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
                Some(msg.trim().to_string())
            } else {
                Some(json.to_string())
            }
        } else {
            None
        }
    }
}
