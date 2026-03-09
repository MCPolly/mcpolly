use serde::{Deserialize, Serialize};

// ─── Agent (DB row) ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub metadata_json: Option<String>,
    pub current_state: String,
    pub last_message: Option<String>,
    pub last_error_message: Option<String>,
    pub registered_at: String,
    pub last_update_at: Option<String>,
}

/// View model used by templates — field names match the template variables.
#[derive(Debug, Clone)]
pub struct AgentView {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub last_seen: String,
    pub last_error: Option<String>,
    pub registered_at: String,
    pub total_updates: i64,
    pub total_errors: i64,
}

impl AgentRow {
    pub fn to_view(&self, total_updates: i64, total_errors: i64) -> AgentView {
        let desc = if self.description.is_empty() {
            None
        } else {
            Some(self.description.clone())
        };
        AgentView {
            id: self.id.clone(),
            name: self.name.clone(),
            description: desc,
            status: self.current_state.clone(),
            last_seen: self
                .last_update_at
                .as_deref()
                .map(relative_time)
                .unwrap_or_else(|| "never".to_string()),
            last_error: self.last_error_message.clone(),
            registered_at: self.registered_at.clone(),
            total_updates,
            total_errors,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RegisterAgentRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct RegisterAgentResponse {
    pub id: String,
    pub name: String,
    pub created: bool,
}

// ─── Status Update ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusUpdate {
    pub id: String,
    pub agent_id: String,
    pub state: String,
    pub message: String,
    pub metadata_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct PostStatusRequest {
    pub agent_id: String,
    pub state: String,
    pub message: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

// ─── Error ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentError {
    pub id: String,
    pub agent_id: String,
    pub message: String,
    pub severity: String,
    pub stack_trace: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct PostErrorRequest {
    pub agent_id: String,
    pub message: String,
    #[serde(default = "default_severity")]
    pub severity: String,
    pub stack_trace: Option<String>,
}

fn default_severity() -> String {
    "error".to_string()
}

// ─── Alert Rule (DB row) ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub condition: String, // "agent_error" or "agent_offline"
    pub agent_id: Option<String>,
    pub webhook_url: String,
    pub enabled: bool,
    pub silence_minutes: i64,
    pub created_at: String,
}

/// View model used by alert templates.
#[derive(Debug, Clone)]
pub struct AlertView {
    pub id: String,
    pub condition: String,
    pub agent_name: Option<String>,
    pub webhook_url: String,
    pub active: bool,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateAlertRuleRequest {
    pub name: String,
    pub condition: String,
    pub agent_id: Option<String>,
    pub webhook_url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_silence_minutes")]
    pub silence_minutes: Option<i64>,
}

fn default_true() -> bool {
    true
}

fn default_silence_minutes() -> Option<i64> {
    Some(5)
}

// ─── Alert History ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertHistoryEntry {
    pub id: String,
    pub alert_rule_id: String,
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub condition: String,
    pub message: String,
    pub delivery_status: String,
    pub created_at: String,
}

// ─── API Key (DB row) ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRow {
    pub id: String,
    pub label: String,
    pub key_prefix: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

/// View model for settings template — field names match template variables.
#[derive(Debug, Clone)]
pub struct ApiKeyView {
    pub id: String,
    pub label: String,
    pub key_suffix: String,
    pub created_at: String,
}

impl ApiKeyRow {
    pub fn to_view(&self) -> ApiKeyView {
        ApiKeyView {
            id: self.id.clone(),
            label: self.label.clone(),
            key_suffix: self.key_prefix.clone(),
            created_at: self.created_at.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct CreateApiKeyResponse {
    pub id: String,
    pub label: String,
    pub key: String,
    pub key_prefix: String,
}

// ─── Activity entry for templates ───

#[derive(Debug, Clone)]
pub struct ActivityEntry {
    pub timestamp: String,
    pub entry_type: String, // "status", "error", "warn"
    pub state: Option<String>,
    pub message: String,
}

// ─── Valid states ───

pub const VALID_STATES: &[&str] = &[
    "starting", "running", "warning", "error", "completed", "offline", "paused", "errored", "stopping", "stopped",
];

pub fn is_valid_state(state: &str) -> bool {
    VALID_STATES.contains(&state)
}

pub fn is_error_state(state: &str) -> bool {
    matches!(state, "error" | "errored")
}

pub fn is_stoppable_state(state: &str) -> bool {
    matches!(state, "starting" | "running" | "warning" | "paused")
}

// ─── Helpers ───

pub fn relative_time(timestamp: &str) -> String {
    use chrono::{NaiveDateTime, Utc};
    let parsed = NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S");
    match parsed {
        Ok(dt) => {
            let now = Utc::now().naive_utc();
            let duration = now.signed_duration_since(dt);

            if duration.num_seconds() < 0 {
                "just now".to_string()
            } else if duration.num_seconds() < 60 {
                format!("{} sec ago", duration.num_seconds())
            } else if duration.num_minutes() < 60 {
                format!("{} min ago", duration.num_minutes())
            } else if duration.num_hours() < 24 {
                format!("{} hours ago", duration.num_hours())
            } else {
                format!("{} days ago", duration.num_days())
            }
        }
        Err(_) => timestamp.to_string(),
    }
}
