//! Wire contract shared by the Router and Server (signed Share edit ACK).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminationFence {
    pub termination_id: String,
    pub cutoff_at_ms: i64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    pub policy: crate::models::ShareUserPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageWindow {
    pub starts_at_ms: i64,
    pub ends_at_ms: i64,
    /// Exact quantity: Tokens or USD-equivalent picos, never wallet money.
    pub used: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminationReceipt {
    pub termination_id: String,
    pub entitlement_id: String,
    pub share_id: String,
    pub email: String,
    pub operation_id: String,
    pub cutoff_at_ms: i64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    pub revoked_at_ms: i64,
    pub archive_since_ms: i64,
    /// Paid usage before this billing interval, for a lifetime quota's
    /// remaining opening allowance. Not a wallet balance.
    pub quota_used_before_period: String,
    pub windows: Vec<UsageWindow>,
}
