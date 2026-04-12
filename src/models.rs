use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub id: String,
    pub public_key: String,
    pub platform: String,
    pub app_version: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelLease {
    pub id: String,
    pub installation_id: String,
    pub connection_id: String,
    pub subdomain: String,
    pub tunnel_type: String,
    pub ssh_username: String,
    pub ssh_password: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub share: Option<ShareDescriptor>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationRequest {
    pub public_key: String,
    pub platform: String,
    pub app_version: String,
    pub instance_nonce: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationResponse {
    pub installation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueLeaseRequest {
    pub installation_id: String,
    pub requested_subdomain: String,
    pub tunnel_type: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareDescriptor>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSyncRequest {
    pub installation_id: String,
    pub share: ShareDescriptor,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDeleteRequest {
    pub installation_id: String,
    pub share_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminLoginRequest {
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareBatchSyncRequest {
    pub installation_id: String,
    pub ops: Vec<ShareSyncOperation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSyncOperation {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueLeaseResponse {
    pub lease_id: String,
    pub connection_id: String,
    pub ssh_username: String,
    pub ssh_password: String,
    pub expires_at: DateTime<Utc>,
    pub tunnel_url: String,
    pub subdomain: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDescriptor {
    pub share_id: String,
    pub share_name: String,
    pub share_token: String,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub token_limit: i64,
    pub tokens_used: i64,
    pub requests_count: i64,
    pub share_status: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardResponse {
    pub generated_at: DateTime<Utc>,
    pub stats: DashboardStats,
    pub installations: Vec<InstallationView>,
    pub shares: Vec<ShareView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardStats {
    pub installations: usize,
    pub shares: usize,
    pub active_leases: usize,
    pub active_shares: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationView {
    pub id: String,
    pub platform: String,
    pub app_version: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub active_lease_count: usize,
    pub leases: Vec<LeaseView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseView {
    pub connection_id: String,
    pub subdomain: String,
    pub tunnel_type: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub is_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareDescriptor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    pub share_id: String,
    pub share_name: String,
    pub share_token: String,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub token_limit: i64,
    pub tokens_used: i64,
    pub requests_count: i64,
    pub share_status: String,
    pub created_at: String,
    pub expires_at: String,
    pub latest_subdomain: String,
    pub installation_id: String,
    pub active_lease_count: usize,
}
