use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

pub const MIN_SHARE_CONTRACT_VERSION: u16 = 2;
pub const SHARE_CONTRACT_VERSION: u16 = 3;

pub fn default_share_parallel_limit() -> i64 {
    -1
}

pub const DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES: u32 = 60;
pub const MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES: u32 = 10;
pub const MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES: u32 = 7 * 24 * 60;

fn default_banked_reset_expiry_lead_minutes() -> u32 {
    DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES
}

fn is_default_banked_reset_expiry_lead_minutes(value: &u32) -> bool {
    *value == DEFAULT_BANKED_RESET_EXPIRY_LEAD_MINUTES
}

/// Distinguishes a missing JSON field (`None`) from an explicit `null`
/// (`Some(None)`). `Option<Option<T>>` with only `default` treats both as
/// absent, so `"description": null` would never clear the stored value.
fn deserialize_optional_nullable_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
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
    #[serde(default)]
    pub client_activated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub delegate_upgrade_to_router_owner: Option<bool>,
    #[serde(default)]
    pub app_commit_id: Option<String>,
    #[serde(default)]
    pub update_available: Option<bool>,
    #[serde(default)]
    pub upgrade_capable: Option<bool>,
    #[serde(default)]
    pub status_reported_at: Option<DateTime<Utc>>,
    /// Self-reported public IPv4 from the server process (startup probe).
    #[serde(default)]
    pub public_ip: Option<String>,
    #[serde(default)]
    pub provision_source: Option<String>,
    #[serde(default)]
    pub log_collection_enabled: bool,
    #[serde(default)]
    pub log_collection_reported_at: Option<DateTime<Utc>>,
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
    pub auth_source_kind: String,
    pub auth_source_id: String,
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
    pub protocol_epoch: String,
    pub router_id: String,
    pub id: String,
    pub installation_id: String,
    pub connection_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterInstallationRequest {
    pub protocol_epoch: String,
    pub public_key: String,
    pub platform: String,
    pub app_version: String,
    pub instance_nonce: String,
    pub timestamp_ms: i64,
    pub signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationResponse {
    pub installation_id: String,
    /// Symmetric secret the server uses to HMAC-sign control-plane RPC calls it
    /// makes back to this installation's local `/_ctl/*` API. Independent of the
    /// client's Ed25519 keypair. Clients must persist it and verify inbound
    /// control calls against it.
    pub control_secret: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterAuthDeviceRequest {
    pub protocol_epoch: String,
    pub public_key: String,
    pub kind: String,
    pub platform: String,
    pub app_version: String,
    pub instance_nonce: String,
    pub timestamp_ms: i64,
    pub signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterAuthDeviceResponse {
    pub auth_device_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationSetupCompletedPayload {
    pub protocol_version: i64,
    pub setup_id: String,
    pub password_hint: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationSetupCompletedRequest {
    pub protocol_epoch: String,
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub setup: InstallationSetupCompletedPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum InstallationSetupCompletedStatus {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "already_recorded")]
    AlreadyRecorded,
    #[serde(rename = "suppressed_disabled")]
    SuppressedDisabled,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationSetupCompletedResponse {
    pub ok: bool,
    pub setup_id: String,
    pub status: InstallationSetupCompletedStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestEmailCodeRequest {
    pub email: String,
    pub auth_device_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientWebRequestEmailCodeRequest {
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifyEmailCodeRequest {
    pub email: String,
    pub code: String,
    pub auth_device_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientWebVerifyEmailCodeRequest {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_token_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserApiTokenStatus {
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserApiTokenResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
    pub token: UserApiTokenStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserApiTokenResetResponse {
    pub api_token: String,
    pub token: UserApiTokenStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareApiAuthUser {
    pub email: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareApiAuthResponse {
    pub authenticated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<ShareApiAuthUser>,
    pub can_manage: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareApiContextResponse {
    pub mode: String,
    pub share_id: String,
    pub subdomain: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareApiShareResponse {
    pub share: ShareView,
    pub auth: ShareApiAuthResponse,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshSessionRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusResponse {
    pub authenticated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<AuthUser>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserShareView {
    pub router_id: String,
    pub share_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    pub role: String,
    pub can_invoke: bool,
    pub can_manage: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub free_access: bool,
    pub subdomain: String,
    pub tunnel_url: String,
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
    pub is_online: bool,
    pub active_requests: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_edit: Option<ShareEditView>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_user_token_periods: Vec<ShareTokenPeriod>,
    #[serde(default)]
    pub config_revision: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSharesResponse {
    pub shares: Vec<UserShareView>,
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
    pub owner_verified: bool,
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
    pub owner_verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelConfig {
    pub owner_email: String,
    pub subdomain: String,
    #[serde(default = "default_client_tunnel_enabled")]
    pub enabled: bool,
}

fn default_client_tunnel_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelQuery {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelClaimRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub tunnel: ClientTunnelConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelUpdateRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub tunnel: ClientTunnelConfig,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<ClientTunnelView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubdomainAvailabilityResponse {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTunnelView {
    pub installation_id: String,
    pub owner_email: String,
    pub subdomain: String,
    pub enabled: bool,
    pub tunnel_url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueLeaseRequest {
    pub protocol_epoch: String,
    pub router_id: String,
    pub installation_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
    pub requested_subdomain: String,
    pub tunnel_type: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareDescriptor>,
    #[serde(skip)]
    pub(crate) signed_share: Option<Box<RawValue>>,
}

impl<'de> Deserialize<'de> for IssueLeaseRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct WireRequest {
            protocol_epoch: String,
            router_id: String,
            installation_id: String,
            route_id: String,
            rotation_id: String,
            generation: u64,
            expected_generation: u64,
            requested_subdomain: String,
            tunnel_type: String,
            timestamp_ms: i64,
            nonce: String,
            signature: String,
            #[serde(default)]
            share: Option<Box<RawValue>>,
        }

        let wire = WireRequest::deserialize(deserializer)?;
        let share = wire
            .share
            .as_ref()
            .map(|raw| serde_json::from_str(raw.get()).map_err(serde::de::Error::custom))
            .transpose()?;
        Ok(Self {
            protocol_epoch: wire.protocol_epoch,
            router_id: wire.router_id,
            installation_id: wire.installation_id,
            route_id: wire.route_id,
            rotation_id: wire.rotation_id,
            generation: wire.generation,
            expected_generation: wire.expected_generation,
            requested_subdomain: wire.requested_subdomain,
            tunnel_type: wire.tunnel_type,
            timestamp_ms: wire.timestamp_ms,
            nonce: wire.nonce,
            signature: wire.signature,
            share,
            signed_share: wire.share,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewLeasePayload {
    pub protocol_epoch: String,
    pub router_id: String,
    pub lease_id: String,
    pub connection_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewLeaseRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub renewal: RenewLeasePayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewLeaseResponse {
    pub protocol_epoch: String,
    pub router_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelActivatePayload {
    pub protocol_epoch: String,
    pub router_id: String,
    pub lease_id: String,
    pub connection_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelActivateRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub activation: TunnelActivatePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatePayload {
    pub protocol_epoch: String,
    pub router_id: String,
    pub lease_id: String,
    pub connection_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStateRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub query: TunnelStatePayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStateResponse {
    pub protocol_epoch: String,
    pub router_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
    pub state: String,
    pub active_generation: Option<u64>,
    pub candidate_generations: Vec<u64>,
    pub draining_generations: Vec<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<ShareClaimPayload>,
    pub share: ShareDescriptor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareClaimPayload {
    pub share_id: String,
    pub subdomain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
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
pub struct SharePruneRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub share_ids: Vec<String>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDescriptorSyncAck {
    pub share_id: String,
    pub descriptor_generation: u64,
    pub descriptor_fingerprint: String,
    pub applied: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDescriptorBatchSyncResponse {
    pub ok: bool,
    pub acks: Vec<ShareDescriptorSyncAck>,
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

/// Minimal, redacted request observation payload used by signed Gateway
/// clients. Downstream user identity, API keys, pricing, and settlement stay
/// outside Router; the registered Gateway is the observation principal.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayRequestObservationBatch {
    pub logs: Vec<GatewayRequestObservation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayRequestObservation {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_subdomain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub request_agent: String,
    pub requested_model: String,
    pub actual_model: String,
    pub actual_model_source: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_tokens: u32,
    #[serde(default)]
    pub cache_creation_tokens: u32,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country_iso3: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRuntimeRefreshPayload {
    pub share_id: String,
    pub subdomain: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRuntimeRefreshRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub refresh: ShareRuntimeRefreshPayload,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_nullable_string"
    )]
    pub description: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_access: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_personal_credits: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_consume_banked_reset: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banked_reset_expiry_lead_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_response_cache_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support: Option<ShareSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_grants: Option<BTreeMap<String, ShareUserGrant>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_usage_edits: Option<BTreeMap<String, ShareUserUsageEdit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_grant: Option<ShareManagedGrantOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareManagedGrantAction {
    Upsert,
    Revoke,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareManagedGrantOperation {
    pub operation_id: String,
    pub entitlement_id: String,
    pub share_sequence: i64,
    pub expected_config_revision: u64,
    pub action: ShareManagedGrantAction,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<ShareUserPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
}

#[cfg(test)]
mod share_settings_patch_tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct LegacyShareSettingsPatch {
        #[serde(default)]
        description: Option<Option<String>>,
    }

    #[test]
    fn absent_user_grants_stays_compatible_with_legacy_server_patch() {
        let patch = ShareSettingsPatch {
            description: Some(Some("updated".to_string())),
            ..ShareSettingsPatch::default()
        };

        let value = serde_json::to_value(&patch).expect("serialize share patch");
        assert!(value.get("userGrants").is_none());

        let legacy: LegacyShareSettingsPatch =
            serde_json::from_value(value).expect("legacy server accepts patch");
        assert_eq!(legacy.description, Some(Some("updated".to_string())));
    }

    #[test]
    fn explicit_null_description_clears_instead_of_being_absent() {
        let patch: ShareSettingsPatch =
            serde_json::from_str(r#"{"description":null}"#).expect("parse null description");
        assert_eq!(patch.description, Some(None));

        let omitted: ShareSettingsPatch =
            serde_json::from_str(r#"{}"#).expect("parse omitted description");
        assert_eq!(omitted.description, None);
    }

    #[test]
    fn market_app_scope_uses_the_v3_allowed_apps_wire_field() {
        let policy = ShareUserPolicy {
            allowed_apps: vec!["codex".to_string()],
            ..ShareUserPolicy::default()
        };

        assert_eq!(
            serde_json::to_value(&policy).expect("serialize App-scoped policy"),
            serde_json::json!({
                "tokenPeriod": "lifetime",
                "allowedApps": ["codex"]
            })
        );
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareTokenPeriod {
    #[default]
    Lifetime,
    Day,
    Week,
    SevenDays,
    CalendarMonth,
    ThirtyDays,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub token_period: ShareTokenPeriod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_period_anchor_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_apps: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserUsageBucket {
    #[serde(default)]
    pub started_at_ms: i64,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub requests_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserUsage {
    #[serde(default)]
    pub lifetime: ShareUserUsageBucket,
    #[serde(default)]
    pub day: ShareUserUsageBucket,
    #[serde(default)]
    pub week: ShareUserUsageBucket,
    #[serde(default)]
    pub calendar_month: ShareUserUsageBucket,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchored: Option<ShareAnchoredUsageBucket>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserUsageRebase {
    pub period: ShareTokenPeriod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_starts_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ends_at_ms: Option<i64>,
    pub target_tokens: u64,
    #[serde(default)]
    pub observed_tokens_at_rebase: u64,
    #[serde(default)]
    pub observed_requests_at_rebase: u64,
    #[serde(default)]
    pub usage_watermark: u64,
    pub applied_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_by: Option<String>,
    #[serde(default)]
    pub source: ShareUsageRebaseSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareUserUsageEditAction {
    Set,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareUserUsageEdit {
    pub action: ShareUserUsageEditAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_grant_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<ShareTokenPeriod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_at_ms: Option<i64>,
    #[serde(default)]
    pub source: ShareUsageRebaseSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserQuotaView {
    #[serde(default)]
    pub period: ShareTokenPeriod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_starts_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ends_at_ms: Option<i64>,
    #[serde(default)]
    pub effective_tokens_used: u64,
    #[serde(default)]
    pub observed_tokens_used: u64,
    #[serde(default)]
    pub manual_offset_tokens: i64,
    #[serde(default)]
    pub observed_requests_count: u64,
    #[serde(default)]
    pub rebase_applies: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareUsageRebaseSource {
    #[default]
    Manual,
    ProviderReset,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAnchoredUsageBucket {
    pub period: ShareTokenPeriod,
    pub anchor_at_ms: i64,
    pub started_at_ms: i64,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub requests_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserGrant {
    pub email: String,
    #[serde(default)]
    pub role: String,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub policy: ShareUserPolicy,
    #[serde(default)]
    pub usage: ShareUserUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_rebase: Option<ShareUserUsageRebase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_quota: Option<ShareUserQuotaView>,
    #[serde(default)]
    pub created_at_ms: u128,
    #[serde(default)]
    pub updated_at_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<u128>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub manager: ShareGrantManager,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entitlement_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareGrantManager {
    Owner,
    #[default]
    Manual,
    RouterShareMarket,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditView {
    pub id: String,
    pub share_id: String,
    pub installation_id: String,
    pub revision: i64,
    pub status: String,
    pub patch: ShareSettingsPatch,
    pub created_by_email: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSettingsUpdateRequest {
    pub patch: ShareSettingsPatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_config_revision: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSettingsUpdateResponse {
    pub ok: bool,
    pub edit: ShareEditView,
    /// True when the edit was applied immediately via the control-plane RPC to
    /// the online client. False means it was queued (client offline / control
    /// channel unavailable) and will apply on the next client sync.
    pub applied_synchronously: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharePendingEditsRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(default)]
    pub share_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharePendingEditsPayload {
    pub share_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharePendingEditsResponse {
    pub edits: Vec<ShareEditView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditAckPayload {
    pub edit_id: String,
    pub revision: i64,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_config_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_share: Option<ShareDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_policy: Option<ShareUserPolicy>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditAckRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub ack: ShareEditAckPayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditAckEnvelope {
    pub ack: ShareEditAckPayload,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditEventSignaturePayload {
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEditAvailableEvent {
    pub kind: String,
    pub installation_id: String,
    pub share_id: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSyncOperation {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareDescriptor>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogEntry {
    /// Downstream clients should prefer the proxied `X-CC-Switch-Request-Id` header as
    /// the request id when present so live dashboard events and synced request logs share
    /// one identity.
    #[serde(default)]
    pub export_sequence: u64,
    pub request_id: String,
    pub share_id: String,
    pub share_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub app_type: String,
    pub model: String,
    pub request_model: String,
    pub request_agent: String,
    pub requested_model: String,
    pub actual_model: String,
    pub actual_model_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier_decision: Option<String>,
    #[serde(default = "default_observed_usage_state")]
    pub usage_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_status: Option<String>,
    #[serde(default)]
    pub usage_revision: u64,
    pub status_code: u16,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_tokens: Option<u32>,
    pub is_streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country_iso3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub is_health_check: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogSyncAck {
    pub request_id: String,
    pub usage_revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogBatchSyncResponse {
    pub ok: bool,
    pub acks: Vec<ShareRequestLogSyncAck>,
}

fn default_observed_usage_state() -> String {
    "observed".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationRequestLogEntry {
    pub request_id: String,
    pub share_id: String,
    pub share_name: String,
    pub installation_id: String,
    pub provider_id: String,
    pub provider_name: String,
    pub app_type: String,
    pub model: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_url: Option<String>,
    #[serde(skip)]
    pub result_storage_key: Option<String>,
    #[serde(skip)]
    pub result_access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareModelHealthCheckEntry {
    pub request_id: String,
    pub share_id: String,
    pub subdomain: String,
    pub app_type: String,
    pub requested_model: String,
    pub actual_model: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub checked_at: i64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelHealthSummary {
    pub app_type: String,
    pub requested_model: String,
    pub actual_model: String,
    pub status: String,
    #[serde(default)]
    pub recent_results: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(
        rename = "checkedAt",
        alias = "lastCheckedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_checked_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareModelHealthSummary {
    #[serde(default)]
    pub claude: Vec<ModelHealthSummary>,
    #[serde(default)]
    pub codex: Vec<ModelHealthSummary>,
    #[serde(default)]
    pub gemini: Vec<ModelHealthSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogFetchResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    #[serde(default)]
    pub logs: Vec<ShareRequestLogEntry>,
    #[serde(default)]
    pub next_sequence: u64,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUsageDailyBucket {
    pub date: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUsageEmailRow {
    pub email: String,
    pub role: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub percent: f64,
    pub daily: Vec<ShareUsageDailyBucket>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUsageByEmailResponse {
    pub share_id: String,
    pub period: String,
    pub bucket_granularity: String,
    pub days: u32,
    pub total_tokens: u64,
    pub rows: Vec<ShareUsageEmailRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageModelRow {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDailyBucket {
    pub date: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageShareRow {
    pub share_id: String,
    pub share_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub models: Vec<UsageModelRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageResponse {
    pub period: String,
    pub bucket_granularity: String,
    pub days: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub models: Vec<UsageModelRow>,
    pub daily: Vec<UsageDailyBucket>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_share: Vec<UsageShareRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCallerRow {
    pub email: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderShareUsage {
    pub share_id: String,
    pub share_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub models: Vec<UsageModelRow>,
    pub callers: Vec<UsageCallerRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstallationUsage {
    pub installation_id: String,
    pub label: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub shares: Vec<ProviderShareUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageResponse {
    pub period: String,
    pub bucket_granularity: String,
    pub days: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub installations: Vec<ProviderInstallationUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalUsageResponse {
    pub period: String,
    pub bucket_granularity: String,
    pub days: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub models: Vec<UsageModelRow>,
    pub daily: Vec<UsageDailyBucket>,
    pub active_shares: usize,
    pub active_clients: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCardSettingsResponse {
    pub user_id: String,
    pub email: String,
    pub public_stats_enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateUsageCardSettingsRequest {
    pub public_stats_enabled: bool,
}

/// One account notification channel. Delivery targets are deliberately
/// omitted; only a safe label such as a Telegram username is returned.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationChannelSettingsResponse {
    pub channel: String,
    pub enabled: bool,
    pub available: bool,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettingsResponse {
    pub email: String,
    /// The one channel every notification for this account is delivered on.
    /// Channels the account cannot currently use never appear here; the
    /// per-channel `channels` entries carry why.
    pub delivery_channel: String,
    pub channels: Vec<NotificationChannelSettingsResponse>,
    /// Internal fence used by the API layer to avoid presenting diagnostics
    /// captured for an older Telegram configuration while a new one is
    /// reconciling. This is deliberately not part of the JSON contract.
    #[serde(skip)]
    pub(crate) telegram_bot_runtime_fingerprint: Option<String>,
    pub telegram_bot_configured: bool,
    pub telegram_bot_status: String,
    pub telegram_bot_transport_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_bot_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_bot_failure_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_bot_failure_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_bot_failure_details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_bot_last_failure_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateNotificationSettingsRequest {
    /// Selecting a channel deselects whatever was selected before — delivery is
    /// never fanned out to more than one place.
    pub channel: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramBindLinkResponse {
    /// `https://t.me/<bot>?start=<token>` — opened in a new tab.
    pub url: String,
    /// Returned once and never persisted in plaintext; the client only needs
    /// it to render a manual fallback (`/start <token>`) when the deep link is
    /// blocked.
    pub token: String,
    pub bot_username: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserLimitStatusRow {
    pub email: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<u64>,
    pub token_period: ShareTokenPeriod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_period_anchor_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    pub tokens_used: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_starts_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserLimitStatusResponse {
    pub share_id: String,
    pub rows: Vec<ShareUserLimitStatusRow>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueLeaseResponse {
    pub protocol_epoch: String,
    pub router_id: String,
    pub lease_id: String,
    pub connection_id: String,
    pub route_id: String,
    pub rotation_id: String,
    pub generation: u64,
    pub expected_generation: u64,
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
    pub database: DatabaseHealthResponse,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseHealthResponse {
    pub mode: String,
    pub available: bool,
    pub last_attempt_at_ms: Option<i64>,
    pub last_success_at_ms: Option<i64>,
    pub last_failure_at_ms: Option<i64>,
    pub consecutive_failures: u64,
    pub last_frames_synced: u64,
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
pub struct PublicNetworkStatsResponse {
    pub active_shares: usize,
    pub active_clients: usize,
}

#[derive(Debug, Clone)]
pub struct GatewayRegistryRecord {
    pub id: String,
    pub owner_email: String,
    pub display_name: String,
    pub public_key: String,
    pub public_base_url: Option<String>,
    pub app_version: Option<String>,
    pub status: String,
    pub scopes: Vec<String>,
}

impl GatewayRegistryRecord {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.status.eq_ignore_ascii_case("active") && self.scopes.iter().any(|value| value == scope)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterGatewayRequest {
    pub owner_email: String,
    pub display_name: String,
    pub public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterGatewayResponse {
    pub gateway_id: String,
    pub owner_email: String,
    pub display_name: String,
    pub status: String,
    pub scopes: Vec<String>,
    pub created_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayShareAppView {
    pub app: String,
    pub supported: bool,
    pub visible: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayShareView {
    pub router_id: String,
    pub share_id: String,
    pub capacity_pool_id: String,
    pub subdomain: String,
    pub installation_id: String,
    pub share_name: String,
    /// Internal grouping key for owner-scoped scheduling feedback. Gateway
    /// inventory must not expose Share or installation owner identities.
    #[serde(skip)]
    pub(crate) scheduling_owner_email: Option<String>,
    pub app_type: String,
    pub share_status: String,
    pub online: bool,
    pub route_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_state_since: Option<String>,
    pub active_requests: usize,
    pub token_limit: i64,
    pub tokens_used: i64,
    pub requests_count: i64,
    pub parallel_limit: i64,
    pub expires_at: String,
    pub online_rate_24h: f64,
    pub observed_minutes_24h: usize,
    pub observation_coverage_24h: f64,
    pub last_seen_at: String,
    /// RFC3339 timestamp from `shares.created_at`. Used by capacity consumers
    /// as a freshness/seniority input for diversification profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_created_at: Option<String>,
    #[serde(default)]
    pub disabled_by_gateway: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_disabled_at: Option<String>,
    #[serde(default)]
    pub support: ShareSupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<ShareUpstreamProvider>,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
    #[serde(default)]
    pub model_health: ShareModelHealthSummary,
    #[serde(default)]
    pub app_availability: CapacityAppAvailability,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(rename = "capacityApps")]
    pub capacity_apps: BTreeMap<String, GatewayShareAppView>,
    #[serde(default)]
    #[serde(rename = "capacityStates")]
    pub capacity_states: Vec<GatewayShareRuntimeStateView>,
    /// Router-computed scheduling signals. Capacity gateways sort using these directly
    /// (no recomputation) and then layer their profile preferences on top.
    #[serde(default)]
    pub signals: ShareSignals,
}

#[cfg(test)]
mod gateway_share_view_tests {
    use super::*;

    #[test]
    fn gateway_share_wire_omits_internal_owner_identity() {
        let view = GatewayShareView {
            router_id: "router.example".into(),
            share_id: "share-1".into(),
            capacity_pool_id: "pool-1".into(),
            subdomain: "share-one".into(),
            installation_id: "installation-1".into(),
            share_name: "share-opaque-1".into(),
            scheduling_owner_email: Some("owner@example.com".into()),
            app_type: "codex".into(),
            share_status: "active".into(),
            online: true,
            route_state: "active".into(),
            route_state_since: None,
            active_requests: 0,
            token_limit: -1,
            tokens_used: 0,
            requests_count: 0,
            parallel_limit: -1,
            expires_at: "2099-12-31T23:59:59Z".into(),
            online_rate_24h: 1.0,
            observed_minutes_24h: 1,
            observation_coverage_24h: 1.0,
            last_seen_at: "2026-08-18T00:00:00Z".into(),
            share_created_at: None,
            disabled_by_gateway: false,
            gateway_disabled_at: None,
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            model_health: ShareModelHealthSummary::default(),
            app_availability: CapacityAppAvailability::default(),
            capacity_apps: BTreeMap::new(),
            capacity_states: Vec::new(),
            signals: ShareSignals::neutral(),
        };

        let json = serde_json::to_value(&view).expect("serialize Gateway Share view");
        assert_eq!(
            view.scheduling_owner_email.as_deref(),
            Some("owner@example.com")
        );
        assert!(json.get("ownerEmail").is_none());
        assert!(json.get("installationOwnerEmail").is_none());
        assert_eq!(
            json.get("shareId").and_then(|value| value.as_str()),
            Some("share-1")
        );
    }
}

/// Router-computed scheduling signals shipped to capacity gateways in every
/// `/v1/gateway/shares` response. All values are normalized so a higher number
/// is preferred. `samples_10m` is included so the consumer can decide whether
/// to trust the short-window stability signal (e.g. for diversification).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSignals {
    /// `0.0..=1.5`: 1.0 = empty quota, 0.0 = exhausted; >1.0 expresses urgency
    /// (a near-reset window with lots of headroom). Neutral = 0.5 when no
    /// quota signal is available.
    pub quota_health: f64,
    /// `0.0..=1.0`: confidence-weighted online rate. Defaults to the 24h rate
    /// when no recent samples exist.
    pub stability: f64,
    /// `0.1..=1.0`: free-capacity ratio against `parallel_limit`. Floored at
    /// 0.1 so saturated shares remain schedulable.
    pub headroom: f64,
    /// Healthy-minute count inside the trailing 10 minutes (0..=10). The
    /// confidence input to `stability`.
    pub samples_10m: u32,
    /// `(0.0..=1.0]`: owner-level penalty applied on top of the base score.
    /// 1.0 = no penalty. Sourced from the in-memory override store (429
    /// feedback). Decays via TTL.
    pub owner_penalty: f64,
}

impl ShareSignals {
    pub fn neutral() -> Self {
        Self {
            quota_health: 0.5,
            stability: 0.0,
            headroom: 1.0,
            samples_10m: 0,
            owner_penalty: 1.0,
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardUxEventRequest {
    pub event_type: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub target_type: Option<String>,
    #[serde(default)]
    pub step_count: Option<u16>,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    #[serde(default)]
    pub keyboard: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardUxEventResponse {
    pub accepted: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSupport {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamQuotaTier {
    #[serde(alias = "name")]
    pub label: String,
    pub utilization: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamQuota {
    pub status: String,
    #[serde(
        default,
        alias = "credentialMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_cost: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queried_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_period_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_scope: Option<String>,
    #[serde(default)]
    pub tiers: Vec<ShareUpstreamQuotaTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamModel {
    pub slot: String,
    pub actual_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareProviderModelPolicyScope {
    Global,
    PerApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareProviderModelPolicySource {
    BundleGlobal,
    AppIndependent,
    ProfileFixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ShareProviderModelPolicy {
    Passthrough,
    Single { upstream_model: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareProviderHealth {
    pub healthy: bool,
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_latency_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamProvider {
    pub kind: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_remaining_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<ShareUpstreamQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ShareUpstreamModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy_scope: Option<ShareProviderModelPolicyScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy_source: Option<ShareProviderModelPolicySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy: Option<ShareProviderModelPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ShareProviderHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
}

impl Default for ShareUpstreamProvider {
    fn default() -> Self {
        Self {
            kind: String::new(),
            app: String::new(),
            provider_name: None,
            provider_type: None,
            account_email: None,
            subscription_level: None,
            subscription_expires_at: None,
            subscription_remaining_ms: None,
            quota_percent: None,
            quota_blocked: None,
            quota: None,
            api_url: None,
            models: Vec::new(),
            model_policy_scope: None,
            model_policy_source: None,
            model_policy: None,
            health: None,
            available: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppProvider {
    pub id: String,
    pub name: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_apps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_current: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub codex_image_generation_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_remaining_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<ShareUpstreamQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ShareUpstreamModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy_scope: Option<ShareProviderModelPolicyScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy_source: Option<ShareProviderModelPolicySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy: Option<ShareProviderModelPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ShareProviderHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
}

impl Default for ShareAppProvider {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            app: String::new(),
            bundle_id: None,
            supported_apps: Vec::new(),
            kind: None,
            provider_type: None,
            is_current: false,
            enabled: false,
            codex_image_generation_enabled: false,
            account_email: None,
            subscription_level: None,
            subscription_expires_at: None,
            subscription_remaining_ms: None,
            quota_percent: None,
            quota_blocked: None,
            quota: None,
            api_url: None,
            models: Vec::new(),
            model_policy_scope: None,
            model_policy_source: None,
            model_policy: None,
            health: None,
            available: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppProviders {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claude: Vec<ShareAppProvider>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codex: Vec<ShareAppProvider>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gemini: Vec<ShareAppProvider>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro: Option<ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antigravity: Option<ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copilot: Option<ShareUpstreamProvider>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppAvailability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<ShareProviderAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<ShareProviderAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ShareProviderAvailability>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareProviderAvailability {
    pub app: String,
    pub provider_id: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRuntimeSnapshotResponse {
    pub share_id: String,
    pub queried_at: i64,
    #[serde(default)]
    pub token_limit: Option<i64>,
    #[serde(default)]
    pub tokens_used: Option<i64>,
    #[serde(default)]
    pub requests_count: Option<i64>,
    #[serde(default)]
    pub share_status: Option<String>,
    pub support: ShareSupport,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
    #[serde(default)]
    pub app_providers: ShareAppProviders,
    #[serde(default)]
    pub model_health: ShareModelHealthSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareDescriptor {
    pub contract_version: u16,
    pub share_id: String,
    pub capacity_pool_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub free_access: bool,
    pub subdomain: String,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Share 的 app/provider bindings。写入时必须包含 1..=3 项，每个
    /// app 最多一项，且 `app_type` / `provider_id` 必须指向其中一项。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, String>,
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
    #[serde(default)]
    pub app_providers: ShareAppProviders,
    #[serde(default)]
    pub app_availability: ShareAppAvailability,
    #[serde(default)]
    pub model_health: ShareModelHealthSummary,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_start: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_personal_credits: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_consume_banked_reset: bool,
    #[serde(
        default = "default_banked_reset_expiry_lead_minutes",
        skip_serializing_if = "is_default_banked_reset_expiry_lead_minutes"
    )]
    pub banked_reset_expiry_lead_minutes: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub previous_response_cache_enabled: bool,
    #[serde(default, skip_serializing_if = "is_zero_revision")]
    pub config_revision: u64,
    #[serde(default, skip_serializing_if = "is_zero_revision")]
    pub descriptor_generation: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub descriptor_fingerprint: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_user_token_periods: Vec<ShareTokenPeriod>,
}

fn is_zero_revision(value: &u64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MapViewportSettings {
    pub visible_start_px: i32,
}

impl Default for MapViewportSettings {
    fn default() -> Self {
        Self {
            visible_start_px: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MapDisplaySettings {
    pub show_flows: bool,
    pub show_heat: bool,
    pub viewport: MapViewportSettings,
    #[serde(default)]
    pub revision: String,
}

impl Default for MapDisplaySettings {
    fn default() -> Self {
        Self {
            show_flows: true,
            show_heat: true,
            viewport: MapViewportSettings::default(),
            revision: "0".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapViewportSettingsUpdate {
    pub visible_start_px: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapDisplaySettingsUpdate {
    pub expected_revision: String,
    pub show_flows: Option<bool>,
    pub show_heat: Option<bool>,
    pub viewport: Option<MapViewportSettingsUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AnnouncementSettings {
    pub enabled: bool,
    pub content_en: String,
    pub content_zh_cn: String,
    pub updated_at: DateTime<Utc>,
}

impl Default for AnnouncementSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            content_en: String::new(),
            content_zh_cn: String::new(),
            updated_at: DateTime::from_timestamp(0, 0).expect("Unix epoch must be valid"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnouncementSettingsUpdate {
    pub expected_revision: String,
    pub enabled: Option<bool>,
    pub content_en: Option<String>,
    pub content_zh_cn: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnouncementResponse {
    pub enabled: bool,
    pub revision: String,
    pub content_en: String,
    pub content_zh_cn: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardResponse {
    pub generated_at: DateTime<Utc>,
    pub stats: DashboardStats,
    pub map: DashboardMap,
    pub map_display: MapDisplaySettings,
    pub clients: Vec<DashboardClientView>,
    /// 所有 share 的平铺数据；前端按 installation 归入对应 client 的横向卡片列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<ShareView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ticker_shares: Vec<DashboardTickerShare>,
    /// Active-client count keyed by ISO 3166-1 alpha-3. Drives the SVG country heatmap
    /// directly (the bundled `world-map.svg` uses alpha-3 as its CSS class names).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub country_counts: std::collections::HashMap<String, usize>,
    /// Per-country client/share board used by the dashboard map hover tooltip.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub country_boards: std::collections::HashMap<String, CountryBoard>,
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

/// Canonical dashboard state shared by the map, entity summaries and drawers.
/// Raw health/capacity fields remain available as supporting evidence; consumers
/// should not independently derive a conflicting top-level state from them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationalSummary {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_reason: Option<OperationalReason>,
    #[serde(default)]
    pub additional_reason_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<String>,
}

impl OperationalSummary {
    pub fn healthy(state: impl Into<String>) -> Self {
        Self {
            state: state.into(),
            primary_reason: None,
            additional_reason_count: 0,
            changed_at: None,
        }
    }
}

/// Whether an enabled Share can currently accept service traffic.
///
/// This is intentionally independent from `OperationalSummary`: advisory
/// conditions can keep a Share service-ready while its operational state is
/// degraded and continues to surface a warning in the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareServiceReadiness {
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_blocker: Option<OperationalReason>,
    #[serde(default)]
    pub additional_blocker_count: usize,
}

impl ShareServiceReadiness {
    pub fn ready() -> Self {
        Self {
            ready: true,
            primary_blocker: None,
            additional_blocker_count: 0,
        }
    }

    pub fn blocked(blockers: Vec<OperationalReason>) -> Self {
        Self {
            ready: false,
            primary_blocker: blockers.first().cloned(),
            additional_blocker_count: blockers.len().saturating_sub(1),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OperationalReason {
    pub code: String,
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<String>,
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
    pub countries: Vec<CountryMapPoint>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountryMapPoint {
    pub country_code: String,
    pub country_code_iso3: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_name: Option<String>,
    pub lat: f64,
    pub lon: f64,
    pub client_count: usize,
    pub share_count: usize,
    pub online_share_count: usize,
    pub inflight_requests: usize,
    pub client_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountryBoard {
    pub country_code: String,
    pub country_code_iso3: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_name: Option<String>,
    pub lat: f64,
    pub lon: f64,
    pub client_count: usize,
    pub share_count: usize,
    pub online_share_count: usize,
    pub inflight_requests: usize,
    pub client_ids: Vec<String>,
    pub clients: Vec<CountryClientBoard>,
    #[serde(default)]
    pub overflow_client_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountryClientBoard {
    pub installation_id: String,
    pub platform: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    pub share_count: usize,
    pub operational_state: String,
    pub shares: Vec<CountryShareBoard>,
    #[serde(default)]
    pub overflow_share_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountryShareBoard {
    pub share_id: String,
    pub share_name: String,
    pub subdomain: String,
    pub app_type: String,
    pub is_online: bool,
    pub active_requests: usize,
    pub operational_state: String,
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
    #[serde(default)]
    pub active_requests: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationView {
    pub id: String,
    pub platform: String,
    pub app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    /// Preferred self-reported public IP, falling back to the router-observed source IP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<InstallationUpgradeView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provision_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationUpgradeView {
    pub delegate_upgrade_to_router_owner: bool,
    pub update_available: bool,
    pub upgrade_capable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportInstallationStatusPayload {
    pub delegate_upgrade_to_router_owner: bool,
    pub auto_upgrade_enabled: bool,
    pub app_commit_id: String,
    pub update_available: bool,
    pub upgrade_capable: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportInstallationStatusRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub payload: ReportInstallationStatusPayload,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportInstallationStatusResponse {
    pub ok: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationHeartbeatPayload {
    pub protocol_version: i64,
    pub boot_id: String,
    pub app_version: String,
    pub commit_id: String,
    /// Optional self-reported public IPv4. Absent/empty keeps the previous value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
    pub log_collection_enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationHeartbeatRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub payload: InstallationHeartbeatPayload,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationHeartbeatResponse {
    pub ok: bool,
    pub server_time: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeInstallationRequest {
    #[serde(default = "default_restart_after_upgrade")]
    pub restart_after: bool,
}

fn default_restart_after_upgrade() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeInstallationResponse {
    pub ok: bool,
    pub task_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationUpgradeLogEntry {
    pub task_id: String,
    pub step: usize,
    pub total_steps: usize,
    pub level: String,
    pub message: String,
    pub progress: Option<u8>,
    pub at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeInstallationStatusResponse {
    pub task_id: String,
    pub status: String,
    #[serde(default)]
    pub restart_pending: bool,
    #[serde(default)]
    pub target_commit_id: Option<String>,
    #[serde(default)]
    pub logs: Vec<InstallationUpgradeLogEntry>,
    pub status_sync: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationUpgradeTaskReportPayload {
    pub task_id: String,
    pub status: String,
    #[serde(default)]
    pub restart_pending: bool,
    #[serde(default)]
    pub logs: Vec<InstallationUpgradeLogEntry>,
    #[serde(default)]
    pub target_commit_id: Option<String>,
    #[serde(default)]
    pub restart_after: bool,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationUpgradeTaskReportRequest {
    pub protocol_epoch: String,
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub payload: InstallationUpgradeTaskReportPayload,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationUpgradeTaskReportResponse {
    pub ok: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardClientView {
    pub installation: InstallationView,
    #[serde(default)]
    pub chat_available: bool,
    #[serde(default)]
    pub log_collection_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_tunnel: Option<DashboardClientTunnelView>,
    /// 该 installation 名下挂的所有 active share id 列表。
    /// 前端 ClientsTable 用它展示 `#shares` 列，并在抽屉里反查顶层 `shares`
    /// 渲染该机器的所有 share 摘要。Share 维度的元数据（owner / status / 健康）
    /// 一律走顶层 `DashboardResponse.shares` 字段，不在 client 上重复。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub share_ids: Vec<String>,
    /// 与 `share_ids.len()` 等价的便利字段，避免前端做长度调用。
    #[serde(default)]
    pub share_count: usize,
    #[serde(default)]
    pub online_minutes_24h: usize,
    #[serde(default)]
    pub online_rate_24h: f64,
    #[serde(default)]
    pub observed_minutes_24h: usize,
    #[serde(default)]
    pub observation_coverage_24h: f64,
    #[serde(default)]
    pub health_checks: Vec<HealthCheckEntry>,
    #[serde(default)]
    pub health_timeline: Vec<HealthTimelineBucket>,
    pub operational_summary: OperationalSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removal_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardClientTunnelView {
    pub owner_email: String,
    pub subdomain: String,
    pub tunnel_url: String,
    pub enabled: bool,
    pub online: bool,
    pub route_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_state_since: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayShareRuntimeStateView {
    pub share_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_id: Option<String>,
    pub scope: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityAppAvailability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<CapacityAppAvailabilityEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<CapacityAppAvailabilityEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<CapacityAppAvailabilityEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityAppAvailabilityEntry {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_results: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    pub router_id: String,
    pub share_id: String,
    pub capacity_pool_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub free_access: bool,
    pub subdomain: String,
    pub can_view_secret: bool,
    pub can_manage: bool,
    pub can_edit_settings: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_edit: Option<ShareEditView>,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Share 的全部 app/provider bindings，供卡片和详情展示。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, String>,
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
    #[serde(default)]
    pub app_providers: ShareAppProviders,
    pub installation_id: String,
    pub is_online: bool,
    pub route_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_state_since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_at: Option<DateTime<Utc>>,
    /// Number of HTTP requests currently in-flight against this share. This is
    /// the same counter the parallel-limit gate increments, so it is directly
    /// comparable to `parallel_limit`.
    pub active_requests: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub active_requests_by_app: BTreeMap<String, usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub active_requests_by_user: BTreeMap<String, BTreeMap<String, usize>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tokens_used_by_app: BTreeMap<String, i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requests_count_by_app: BTreeMap<String, i64>,
    pub online_minutes_24h: usize,
    pub online_rate_24h: f64,
    pub observed_minutes_24h: usize,
    pub observation_coverage_24h: f64,
    pub recent_requests: Vec<ShareRequestLogEntry>,
    pub health_checks: Vec<HealthCheckEntry>,
    #[serde(default)]
    pub health_timeline: Vec<HealthTimelineBucket>,
    #[serde(default)]
    pub recent_model_health_checks: Vec<ShareModelHealthCheckEntry>,
    #[serde(default)]
    pub model_health: ShareModelHealthSummary,
    pub operational_summary: OperationalSummary,
    pub service_readiness: ShareServiceReadiness,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_user_token_periods: Vec<ShareTokenPeriod>,
    #[serde(default)]
    pub config_revision: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_start: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_personal_credits: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_consume_banked_reset: bool,
    #[serde(
        default,
        skip_serializing_if = "is_default_banked_reset_expiry_lead_minutes"
    )]
    pub banked_reset_expiry_lead_minutes: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub previous_response_cache_enabled: bool,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthTimelineBucket {
    pub start_at: String,
    pub end_at: String,
    pub status: String,
    pub score: f64,
    pub online_minutes: usize,
    pub observed_minutes: usize,
    pub request_count: usize,
    pub failure_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatMessageView {
    pub id: String,
    pub seq: i64,
    pub body: String,
    pub author_label: String,
    pub author_kind: String,
    pub message_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_payload: Option<serde_json::Value>,
    pub is_mine: bool,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatMessagePreview {
    pub seq: i64,
    pub body: String,
    pub author_label: String,
    pub author_kind: String,
    pub message_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_payload: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatRoomView {
    pub id: String,
    pub installation_id: String,
    pub client_label: String,
    pub status: String,
    pub can_post: bool,
    pub read_only: bool,
    pub latest_seq: i64,
    pub unread_count: usize,
    pub last_message_at: Option<DateTime<Utc>>,
    pub last_message: Option<ClientChatMessagePreview>,
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatRoomResponse {
    pub room: ClientChatRoomView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatRoomListResponse {
    pub rooms: Vec<ClientChatRoomView>,
    pub total_unread: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientChatRoomLookupRequest {
    pub installation_ids: Vec<String>,
    #[serde(default)]
    pub last_read_seq_by_installation: BTreeMap<String, i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatMessageListResponse {
    pub messages: Vec<ClientChatMessageView>,
    pub latest_seq: i64,
    pub has_more: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PostClientChatMessageRequest {
    pub body: String,
    pub client_message_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientChatReadRequest {
    pub last_read_seq: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatReadResponse {
    pub ok: bool,
    pub last_read_seq: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientChatVisitImportItem {
    pub installation_id: String,
    #[serde(default)]
    pub last_read_seq: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientChatVisitImportRequest {
    pub visits: Vec<ClientChatVisitImportItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatVisitImportResponse {
    pub imported: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatDeliveryView {
    pub id: String,
    pub room_id: String,
    pub installation_id: String,
    pub client_label: String,
    pub recipient_masked: String,
    pub message_count: usize,
    pub status: String,
    pub attempts: u32,
    pub created_at: DateTime<Utc>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub sent_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatDeliveriesResponse {
    pub deliveries: Vec<ClientChatDeliveryView>,
}
