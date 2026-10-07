use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct ServiceListResponse {
    pub services: Option<Vec<Service>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
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

    /// Extracts the location from the resource name:
    /// `projects/{project}/locations/{location}/services/{service}`.
    /// Returns None if the name is malformed or location is empty.
    pub fn location(&self) -> Option<&str> {
        let parts: Vec<&str> = self.name.split('/').collect();
        if let Some(pos) = parts.iter().position(|&p| p == "locations") {
            parts.get(pos + 1).copied().filter(|loc| !loc.is_empty())
        } else if parts.len() >= 4 && parts[0] == "projects" {
            parts.get(3).copied().filter(|loc| !loc.is_empty())
        } else {
            None
        }
    }

    /// Extracts the concrete location, ensuring it is not the wildcard `"-"`.
    pub fn concrete_location(&self) -> Option<&str> {
        self.location().filter(|loc| *loc != "-")
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

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct Condition {
    #[serde(rename = "type")]
    pub type_field: String,
    pub state: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct TrafficStatus {
    pub revision: Option<String>,
    pub percent: Option<i32>,
    pub tag: Option<String>,
    #[serde(rename = "type")]
    pub traffic_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct RevisionListResponse {
    pub revisions: Option<Vec<Revision>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct Revision {
    pub name: String,
    pub conditions: Option<Vec<Condition>>,
    #[serde(rename = "createTime")]
    pub create_time: Option<String>,
}

impl Revision {
    pub fn short_name(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
    }

    #[allow(dead_code)]
    pub fn is_ready(&self) -> bool {
        if let Some(ref conditions) = self.conditions {
            conditions.iter().any(|c| c.state.as_deref() == Some("CONDITION_SUCCEEDED"))
        } else {
            true
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct LogEntriesResponse {
    pub entries: Option<Vec<LogEntry>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct HttpRequestLog {
    #[serde(rename = "requestMethod")]
    pub request_method: Option<String>,
    #[serde(rename = "requestUrl")]
    pub request_url: Option<String>,
    pub status: Option<i32>,
    pub latency: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_service_location_extraction() {
        let svc_std = Service {
            name: "projects/my-project/locations/us-central1/services/my-service".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: None,
            traffic_statuses: None,
        };
        assert_eq!(svc_std.location(), Some("us-central1"));
        assert_eq!(svc_std.concrete_location(), Some("us-central1"));

        let svc_europe = Service {
            name: "projects/my-project/locations/europe-west1/services/api".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: None,
            traffic_statuses: None,
        };
        assert_eq!(svc_europe.location(), Some("europe-west1"));
        assert_eq!(svc_europe.concrete_location(), Some("europe-west1"));

        // Aggregate wildcard "-"
        let svc_wildcard = Service {
            name: "projects/my-project/locations/-/services/wildcard-svc".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: None,
            traffic_statuses: None,
        };
        assert_eq!(svc_wildcard.location(), Some("-"));
        assert_eq!(svc_wildcard.concrete_location(), None);

        // Malformed or short name
        let svc_short = Service {
            name: "my-service".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: None,
            traffic_statuses: None,
        };
        assert_eq!(svc_short.location(), None);
        assert_eq!(svc_short.concrete_location(), None);

        // Empty location part
        let svc_empty_loc = Service {
            name: "projects/my-project/locations//services/my-service".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: None,
            traffic_statuses: None,
        };
        assert_eq!(svc_empty_loc.location(), None);
        assert_eq!(svc_empty_loc.concrete_location(), None);
    }

    #[test]
    fn test_traffic_summary_single_or_empty() {
        let mut svc = Service {
            name: "projects/test/locations/us-central1/services/web".to_string(),
            uri: None,
            latest_ready_revision: Some("web-001".to_string()),
            conditions: None,
            traffic_statuses: None,
        };

        // None traffic_statuses defaults to 100%
        assert_eq!(svc.traffic_summary(), "100%");

        // Empty vec defaults to 100%
        svc.traffic_statuses = Some(vec![]);
        assert_eq!(svc.traffic_summary(), "100%");

        // Single 100%
        svc.traffic_statuses = Some(vec![TrafficStatus {
            revision: Some("web-001".to_string()),
            percent: Some(100),
            tag: None,
            traffic_type: None,
        }]);
        assert_eq!(svc.traffic_summary(), "100%");

        // Single 80%
        svc.traffic_statuses = Some(vec![TrafficStatus {
            revision: Some("web-001".to_string()),
            percent: Some(80),
            tag: None,
            traffic_type: None,
        }]);
        assert_eq!(svc.traffic_summary(), "80%");
    }

    #[test]
    fn test_traffic_summary_split() {
        let svc = Service {
            name: "projects/test/locations/us-central1/services/web".to_string(),
            uri: None,
            latest_ready_revision: Some("web-002".to_string()),
            conditions: None,
            traffic_statuses: Some(vec![
                TrafficStatus {
                    revision: Some("web-001".to_string()),
                    percent: Some(70),
                    tag: None,
                    traffic_type: None,
                },
                TrafficStatus {
                    revision: Some("web-002".to_string()),
                    percent: Some(30),
                    tag: None,
                    traffic_type: None,
                },
            ]),
        };

        assert_eq!(svc.traffic_summary(), "70%/30% (Split)");
    }

    #[test]
    fn test_traffic_details_fallback_and_present() {
        // Fallback when None
        let svc_none = Service {
            name: "projects/p/locations/l/services/s".to_string(),
            uri: None,
            latest_ready_revision: Some("projects/p/locations/l/services/s/revisions/rev-99".to_string()),
            conditions: None,
            traffic_statuses: None,
        };
        let details = svc_none.traffic_details();
        assert_eq!(details, vec![("rev-99".to_string(), 100, "-".to_string())]);

        // Present with multiple statuses
        let svc_present = Service {
            name: "projects/p/locations/l/services/s".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: None,
            traffic_statuses: Some(vec![
                TrafficStatus {
                    revision: Some("projects/.../rev-1".to_string()),
                    percent: Some(60),
                    tag: Some("candidate".to_string()),
                    traffic_type: None,
                },
                TrafficStatus {
                    revision: None,
                    percent: None,
                    tag: None,
                    traffic_type: None,
                },
            ]),
        };
        let details = svc_present.traffic_details();
        assert_eq!(details.len(), 2);
        assert_eq!(details[0], ("rev-1".to_string(), 60, "candidate".to_string()));
        assert_eq!(details[1], ("latest".to_string(), 0, "-".to_string()));
    }

    #[test]
    fn test_service_is_ready() {
        // With CONDITION_SUCCEEDED
        let svc_ready = Service {
            name: "s".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: Some(vec![Condition {
                type_field: "Ready".to_string(),
                state: Some("CONDITION_SUCCEEDED".to_string()),
            }]),
            traffic_statuses: None,
        };
        assert!(svc_ready.is_ready());

        // With CONDITION_FAILED
        let svc_failed = Service {
            name: "s".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: Some(vec![Condition {
                type_field: "Ready".to_string(),
                state: Some("CONDITION_FAILED".to_string()),
            }]),
            traffic_statuses: None,
        };
        assert!(!svc_failed.is_ready());

        // Fallback to latest_ready_revision if conditions is None
        let svc_fallback = Service {
            name: "s".to_string(),
            uri: None,
            latest_ready_revision: Some("rev-1".to_string()),
            conditions: None,
            traffic_statuses: None,
        };
        assert!(svc_fallback.is_ready());

        let svc_not_ready = Service {
            name: "s".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: None,
            traffic_statuses: None,
        };
        assert!(!svc_not_ready.is_ready());
    }

    #[test]
    fn test_log_entry_message_variants() {
        // HTTP Request variant
        let http_entry = LogEntry {
            text_payload: None,
            json_payload: None,
            http_request: Some(HttpRequestLog {
                request_method: Some("POST".to_string()),
                request_url: Some("https://example.com/api/v1/checkout".to_string()),
                status: Some(201),
                latency: Some("0.125s".to_string()),
            }),
            severity: Some("INFO".to_string()),
            timestamp: None,
        };
        assert_eq!(
            http_entry.message().unwrap(),
            "POST 201 api/v1/checkout 125.0ms"
        );

        // HTTP Request with default fallbacks
        let http_default = LogEntry {
            text_payload: None,
            json_payload: None,
            http_request: Some(HttpRequestLog {
                request_method: None,
                request_url: None,
                status: None,
                latency: None,
            }),
            severity: None,
            timestamp: None,
        };
        assert_eq!(http_default.message().unwrap(), "GET 200 / -");

        // Text payload variant
        let text_entry = LogEntry {
            text_payload: Some("  Container started on port 8080 \n".to_string()),
            json_payload: None,
            http_request: None,
            severity: Some("INFO".to_string()),
            timestamp: None,
        };
        assert_eq!(text_entry.message().unwrap(), "Container started on port 8080");

        // JSON payload with message property
        let json_msg_entry = LogEntry {
            text_payload: None,
            json_payload: Some(json!({
                "message": "User authenticated successfully",
                "uid": 12345
            })),
            http_request: None,
            severity: Some("INFO".to_string()),
            timestamp: None,
        };
        assert_eq!(
            json_msg_entry.message().unwrap(),
            "User authenticated successfully"
        );

        // JSON payload without message property
        let json_raw_entry = LogEntry {
            text_payload: None,
            json_payload: Some(json!({"event": "heartbeat", "healthy": true})),
            http_request: None,
            severity: Some("DEBUG".to_string()),
            timestamp: None,
        };
        assert_eq!(
            json_raw_entry.message().unwrap(),
            "{\"event\":\"heartbeat\",\"healthy\":true}"
        );

        // Completely empty entry
        let empty_entry = LogEntry {
            text_payload: None,
            json_payload: None,
            http_request: None,
            severity: None,
            timestamp: None,
        };
        assert_eq!(empty_entry.message(), None);
    }

    #[test]
    fn test_revision_model() {
        let rev = Revision {
            name: "projects/p/locations/l/services/s/revisions/s-00042-abc".to_string(),
            conditions: Some(vec![Condition {
                type_field: "Ready".to_string(),
                state: Some("CONDITION_SUCCEEDED".to_string()),
            }]),
            create_time: Some("2024-01-01T00:00:00Z".to_string()),
        };

        assert_eq!(rev.short_name(), "s-00042-abc");
        assert!(rev.is_ready());
    }
}
