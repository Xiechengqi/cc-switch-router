use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

fn default_share_for_sale() -> String {
    "No".to_string()
}

fn default_share_parallel_limit() -> i64 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub id: String,
    pub public_key: String,
    pub platform: String,
    pub app_version: String,
    pub owner_email: Option<String>,
    pub owner_verified_at: Option<DateTime<Utc>>,
    pub last_seen_ip: Option<String>,
    pub country_code: Option<String>,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub geo_candidate_country_code: Option<String>,
    pub geo_candidate_country: Option<String>,
    pub geo_candidate_region: Option<String>,
    pub geo_candidate_city: Option<String>,
    pub geo_candidate_latitude: Option<f64>,
    pub geo_candidate_longitude: Option<f64>,
    pub geo_candidate_hits: i64,
    pub geo_candidate_first_seen_at: Option<DateTime<Utc>>,
    pub geo_last_changed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ClientMetadata {
    pub ip: Option<String>,
    pub country_code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuthSession {
    pub session_id: String,
    pub user_id: String,
    pub email: String,
    pub installation_id: String,
    pub access_token_hash: String,
    pub refresh_token_hash: String,
    pub access_expires_at: DateTime<Utc>,
    pub refresh_expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUser {
    pub id: String,
    pub email: String,
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
pub struct RequestEmailCodeRequest {
    pub email: String,
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestEmailCodeResponse {
    pub ok: bool,
    pub cooldown_secs: i64,
    pub masked_destination: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyEmailCodeRequest {
    pub email: String,
    pub code: String,
    pub installation_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyEmailCodeResponse {
    pub user: AuthUser,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub refresh_expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshSessionRequest {
    pub refresh_token: String,
    pub installation_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusResponse {
    pub authenticated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<AuthUser>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_owner_email: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindInstallationOwnerEmailRequest {
    pub installation_id: String,
    pub email: String,
    pub verification_token: Option<String>,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindInstallationOwnerEmailResponse {
    pub ok: bool,
    pub owner_email: String,
    pub already_bound: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeInstallationOwnerEmailRequest {
    pub installation_id: String,
    pub old_email: String,
    pub new_email: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeInstallationOwnerEmailResponse {
    pub ok: bool,
    pub old_email: String,
    pub new_email: String,
    pub updated_shares: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetInstallationOwnerEmailQuery {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetInstallationOwnerEmailResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
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
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub share: ShareDescriptor,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareClaimSubdomainRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub share: ShareDescriptor,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDeleteRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub share_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareBatchSyncRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub ops: Vec<ShareSyncOperation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogBatchSyncRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub logs: Vec<ShareRequestLogEntry>,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogEntry {
    /// Downstream clients should prefer the proxied `X-CC-Switch-Request-Id` header as
    /// the request id when present so live dashboard events and synced request logs share
    /// one identity.
    pub request_id: String,
    pub share_id: String,
    pub share_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub app_type: String,
    pub model: String,
    pub request_model: String,
    pub status_code: u16,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    pub is_streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogFetchResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    #[serde(default)]
    pub logs: Vec<ShareRequestLogEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueLeaseResponse {
    pub lease_id: String,
    pub connection_id: String,
    pub ssh_username: String,
    pub ssh_password: String,
    pub ssh_addr: String,
    pub expires_at: DateTime<Utc>,
    pub tunnel_url: String,
    pub subdomain: String,
    /// SSH host key 指纹（`SHA256:<base64-nopad>` 格式），由客户端用于校验远端身份，
    /// 防止中间人攻击。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_host_fingerprint: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicMapPointsResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<LatLonPoint>,
    pub client_count: usize,
    pub clients: Vec<PublicMapClientPoint>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketsResponse {
    pub markets: Vec<PublicMarketConfig>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicMarketConfig {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub subdomain: String,
    pub public_base_url: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct MarketRegistryRecord {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub subdomain: String,
    pub public_base_url: String,
    pub scopes: Vec<String>,
    pub status: String,
}

impl MarketRegistryRecord {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.status.eq_ignore_ascii_case("active") && self.scopes.iter().any(|value| value == scope)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterMarketRequest {
    pub subdomain: String,
    pub display_name: String,
    pub public_base_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketNotificationEmailRequest {
    pub kind: String,
    pub to: String,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketNotificationEmailResponse {
    pub ok: bool,
    pub message_id: String,
    pub kind: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketNotificationEmailLogView {
    pub id: String,
    pub market_email: String,
    pub kind: String,
    pub to_email: String,
    pub locale: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShareView {
    pub router_id: String,
    pub share_id: String,
    pub installation_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_owner_email: Option<String>,
    pub app_type: String,
    pub for_sale: String,
    pub share_status: String,
    pub online: bool,
    pub active_requests: usize,
    pub parallel_limit: i64,
    pub online_rate_24h: f64,
    pub last_seen_at: String,
    #[serde(default)]
    pub support: ShareSupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<ShareUpstreamProvider>,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicMapClientPoint {
    pub lat: f64,
    pub lon: f64,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatLonPoint {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardPresenceRequest {
    pub session_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardPresenceResponse {
    pub online_count: usize,
    pub email_sent_24h: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResendUsageResponse {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_usage_percent: Option<f64>,
    pub daily_usage_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_header: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSupport {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamQuotaTier {
    pub label: String,
    pub utilization: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamQuota {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queried_at: Option<i64>,
    #[serde(default)]
    pub tiers: Vec<ShareUpstreamQuotaTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamProvider {
    pub kind: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<ShareUpstreamQuota>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppRuntimes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ShareUpstreamProvider>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRuntimeSnapshotResponse {
    pub share_id: String,
    pub queried_at: i64,
    pub support: ShareSupport,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDescriptor {
    pub share_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_share_for_sale")]
    pub for_sale: String,
    pub subdomain: String,
    pub share_token: String,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub token_limit: i64,
    #[serde(default = "default_share_parallel_limit")]
    pub parallel_limit: i64,
    pub tokens_used: i64,
    pub requests_count: i64,
    pub share_status: String,
    pub created_at: String,
    pub expires_at: String,
    #[serde(default)]
    pub support: ShareSupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<ShareUpstreamProvider>,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardResponse {
    pub generated_at: DateTime<Utc>,
    pub stats: DashboardStats,
    pub map: DashboardMap,
    pub clients: Vec<DashboardClientView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markets: Vec<DashboardMarketView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ticker_shares: Vec<DashboardTickerShare>,
    /// Active-client count keyed by ISO 3166-1 alpha-3. Drives the SVG country heatmap
    /// directly (the bundled `world-map.svg` uses alpha-3 as its CSS class names).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub country_counts: std::collections::HashMap<String, usize>,
    /// User-origin request counts over the last 5 minutes, keyed by ISO 3166-1 alpha-3.
    /// Drives the dashboard "demand" pins. Sourced from `cf-ipcountry` on trusted
    /// Cloudflare peers; spoofed values are dropped at the proxy.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub user_country_counts: std::collections::HashMap<String, usize>,
    /// Last N proxy request starts in chronological order. The frontend dedupes by
    /// `request_id` and animates a one-shot burst arc per new event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_request_events: Vec<crate::recent_traffic::RecentRequestEvent>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardStats {
    pub clients: usize,
    pub active_shares: usize,
    /// Total number of HTTP requests currently in-flight across every share.
    pub total_active_requests: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardTickerShare {
    pub share_id: String,
    pub share_name: String,
    pub subdomain: String,
    #[serde(default)]
    pub recent_requests: Vec<ShareRequestLogEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardMap {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<DashboardMapPoint>,
    pub clients: Vec<DashboardMapPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardMapPoint {
    pub id: String,
    pub label: String,
    pub point_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<DateTime<Utc>>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationView {
    pub id: String,
    pub platform: String,
    pub app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardClientView {
    pub installation: InstallationView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardMarketView {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub subdomain: String,
    pub public_base_url: String,
    pub status: String,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    pub share_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub for_sale: String,
    pub subdomain: String,
    pub share_token: String,
    pub can_view_secret: bool,
    pub can_manage: bool,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub token_limit: i64,
    pub parallel_limit: i64,
    pub tokens_used: i64,
    pub requests_count: i64,
    pub share_status: String,
    pub created_at: String,
    pub expires_at: String,
    pub support: ShareSupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<ShareUpstreamProvider>,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
    pub installation_id: String,
    pub is_online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_at: Option<DateTime<Utc>>,
    /// Number of HTTP requests currently in-flight against this share. This is
    /// the same counter the parallel-limit gate increments, so it is directly
    /// comparable to `parallel_limit`.
    pub active_requests: usize,
    pub online_minutes_24h: usize,
    pub online_rate_24h: f64,
    pub recent_requests: Vec<ShareRequestLogEntry>,
    pub health_checks: Vec<HealthCheckEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareHeartbeatRequest {
    pub installation_id: String,
    pub share_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheckEntry {
    pub checked_at: i64,
    pub is_healthy: bool,
}
