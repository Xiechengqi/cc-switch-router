use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct AlertCondition {
    pub fingerprint: String,
    pub scope: String,
    pub kind: String,
    pub entity_kind: String,
    pub entity_id: Option<String>,
    pub severity: String,
    pub title: String,
    pub message: String,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct OperatorAlertSignal {
    pub source_event_id: String,
    pub fingerprint: String,
    pub transition: String,
    pub kind: String,
    pub entity_kind: String,
    pub entity_id: Option<String>,
    pub severity: String,
    pub title: String,
    pub message: String,
    pub details: serde_json::Value,
    pub occurred_at: i64,
    pub attempts: u32,
}

#[derive(Debug, Clone)]
pub struct AlertDeliveryPolicy {
    pub enabled: bool,
    pub dashboard_url: String,
    pub channels: Vec<AlertChannelPolicy>,
}

#[derive(Debug, Clone)]
pub struct AlertChannelPolicy {
    pub channel: String,
    pub min_severity: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertIncident {
    pub id: String,
    pub fingerprint: String,
    pub scope: String,
    pub kind: String,
    pub entity_kind: String,
    pub entity_id: Option<String>,
    pub severity: String,
    pub status: String,
    pub title: String,
    pub message: String,
    pub details: serde_json::Value,
    pub occurrence_count: u64,
    pub started_at: i64,
    pub last_seen_at: i64,
    pub last_transition_at: i64,
    pub resolved_at: Option<i64>,
    pub acknowledged_at: Option<i64>,
    pub acknowledged_by: Option<String>,
    pub acknowledgement_note: Option<String>,
    pub silenced_at: Option<i64>,
    pub silenced_until: Option<i64>,
    pub silenced_by: Option<String>,
    pub silence_note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertTransition {
    pub id: String,
    pub incident_id: String,
    pub source_event_id: Option<String>,
    pub transition: String,
    pub severity: String,
    pub message: String,
    pub details: serde_json::Value,
    pub actor_email: Option<String>,
    pub note: Option<String>,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertDelivery {
    pub id: String,
    pub incident_id: String,
    pub transition_id: String,
    pub channel: String,
    pub status: String,
    pub attempts: u32,
    pub provider_message_id: Option<String>,
    pub next_attempt_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub sent_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct AlertDeliveryClaim {
    pub id: String,
    pub incident_id: String,
    pub transition_id: String,
    pub channel: String,
    pub payload_text: String,
    pub attempts: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertChannelState {
    pub channel: String,
    pub enabled: bool,
    pub configured: bool,
    pub status: String,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub failure_code: Option<String>,
    pub failure_hint: Option<String>,
    pub failure_details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertingOverview {
    pub active_count: u64,
    pub critical_count: u64,
    pub resolved_count: u64,
    pub failed_delivery_count: u64,
    pub incidents: Vec<AlertIncident>,
    pub deliveries: Vec<AlertDelivery>,
    pub channels: Vec<AlertChannelState>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AlertOverviewCounts {
    pub active: u64,
    pub critical: u64,
    pub resolved: u64,
    pub failed_deliveries: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertAcknowledgeRequest {
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertSilenceRequest {
    pub duration_secs: i64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertChannelTestResponse {
    pub ok: bool,
    pub channel: String,
    pub provider_message_id: Option<String>,
    pub tested_at: i64,
}

pub fn severity_rank(value: &str) -> u8 {
    match value {
        "critical" => 3,
        "warning" => 2,
        "info" => 1,
        _ => 0,
    }
}
