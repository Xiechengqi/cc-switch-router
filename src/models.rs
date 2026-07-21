use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha3::{Digest, Keccak256};
use std::collections::BTreeMap;

fn default_share_for_sale() -> String {
    "No".to_string()
}

fn default_market_access_mode() -> String {
    "selected".to_string()
}

fn default_sale_market_kind() -> String {
    "token".to_string()
}

pub fn default_share_parallel_limit() -> i64 {
    -1
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
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationRequest {
    pub protocol_epoch: String,
    pub public_key: String,
    pub platform: String,
    pub app_version: String,
    pub instance_nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_version: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterInstallationResponse {
    pub installation_id: String,
    /// Symmetric secret the server uses to HMAC-sign control-plane RPC calls it
    /// makes back to this installation's local `/_ctl/*` API. Independent of the
    /// client's Ed25519 keypair. Clients must persist it and verify inbound
    /// control calls against it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_secret: Option<String>,
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
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    pub role: String,
    pub can_invoke: bool,
    pub can_manage: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub for_sale: String,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    pub market_access_mode: String,
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

pub const PAYOUT_PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayoutAddressType {
    #[serde(rename = "evm")]
    Evm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayoutToken {
    USDC,
    USDT,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PayoutNetwork {
    #[serde(rename = "eip155:56")]
    Bsc,
    #[serde(rename = "eip155:8453")]
    Base,
    #[serde(rename = "eip155:42161")]
    ArbitrumOne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayoutVerificationStatus {
    #[serde(rename = "self_declared")]
    SelfDeclared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PayoutProfile {
    pub address_type: PayoutAddressType,
    pub address: String,
    pub token: PayoutToken,
    pub networks: Vec<PayoutNetwork>,
    pub verification_status: PayoutVerificationStatus,
}

impl PayoutProfile {
    pub fn validate_and_normalize(mut self) -> Result<Self, String> {
        self.address = normalize_evm_address(&self.address)?;
        self.networks.sort_unstable();
        self.networks.dedup();
        if self.networks.is_empty() {
            return Err("at least one payout network is required".into());
        }
        Ok(self)
    }
}

pub fn normalize_evm_address(value: &str) -> Result<String, String> {
    if value.trim() != value || !value.starts_with("0x") || value.len() != 42 {
        return Err("EVM address must be 0x followed by 40 hexadecimal characters".into());
    }
    let body = &value[2..];
    if !body.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("EVM address must contain only hexadecimal characters".into());
    }
    let normalized = checksum_evm_address(&body.to_ascii_lowercase());
    let has_lower = body.bytes().any(|byte| byte.is_ascii_lowercase());
    let has_upper = body.bytes().any(|byte| byte.is_ascii_uppercase());
    if has_lower && has_upper && normalized != value {
        return Err("mixed-case EVM address must use a valid EIP-55 checksum".into());
    }
    Ok(normalized)
}

fn checksum_evm_address(lowercase_body: &str) -> String {
    let digest = Keccak256::digest(lowercase_body.as_bytes());
    let mut output = String::with_capacity(42);
    output.push_str("0x");
    for (index, byte) in lowercase_body.bytes().enumerate() {
        if byte.is_ascii_alphabetic() {
            let hash_nibble = if index % 2 == 0 {
                digest[index / 2] >> 4
            } else {
                digest[index / 2] & 0x0f
            };
            if hash_nibble >= 8 {
                output.push((byte as char).to_ascii_uppercase());
                continue;
            }
        }
        output.push(byte as char);
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationPayoutProfileUpdate {
    pub schema_version: u32,
    pub revision: i64,
    pub profile: Option<PayoutProfile>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationPayoutProfileUpdateRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub update: InstallationPayoutProfileUpdate,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationPayoutProfileUpdateResponse {
    pub ok: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicPayoutProfileResponse {
    pub schema_version: u32,
    pub revision: i64,
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    pub installation_id: String,
    pub profile: Option<PayoutProfile>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicPayoutProfilesResponse {
    pub profiles: Vec<PublicPayoutProfileResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicPayoutProfilesQuery {
    pub installation_ids: String,
}

#[cfg(test)]
mod payout_profile_tests {
    use super::*;

    #[test]
    fn evm_address_normalization_enforces_eip55_for_mixed_case() {
        assert_eq!(
            normalize_evm_address("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").unwrap(),
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
        );
        assert!(normalize_evm_address("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAee").is_err());
        assert!(normalize_evm_address("5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").is_err());
    }

    #[test]
    fn payout_profile_sorts_and_deduplicates_networks() {
        let profile = PayoutProfile {
            address_type: PayoutAddressType::Evm,
            address: "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".into(),
            token: PayoutToken::USDC,
            networks: vec![
                PayoutNetwork::ArbitrumOne,
                PayoutNetwork::Bsc,
                PayoutNetwork::Bsc,
            ],
            verification_status: PayoutVerificationStatus::SelfDeclared,
        }
        .validate_and_normalize()
        .unwrap();
        assert_eq!(
            profile.networks,
            vec![PayoutNetwork::Bsc, PayoutNetwork::ArbitrumOne]
        );
    }

    #[test]
    fn payout_update_canonical_json_matches_client_signing_contract() {
        let update = InstallationPayoutProfileUpdate {
            schema_version: 1,
            revision: 3,
            profile: Some(PayoutProfile {
                address_type: PayoutAddressType::Evm,
                address: "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".into(),
                token: PayoutToken::USDT,
                networks: vec![PayoutNetwork::Bsc, PayoutNetwork::Base],
                verification_status: PayoutVerificationStatus::SelfDeclared,
            }),
            updated_at_ms: 1_753_000_000_000,
        };
        assert_eq!(
            serde_json::to_string(&update).unwrap(),
            r#"{"schemaVersion":1,"revision":3,"profile":{"addressType":"evm","address":"0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed","token":"USDT","networks":["eip155:56","eip155:8453"],"verificationStatus":"self_declared"},"updatedAtMs":1753000000000}"#
        );
    }
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogBatchSyncRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    pub logs: Vec<ShareRequestLogEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketRequestLogBatchSyncRequest {
    pub logs: Vec<MarketRequestLogEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketRequestLogEntry {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_prefix: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_amount_usd: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country_iso3: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardMarketRequestLogView {
    pub request_id: String,
    pub market_id: String,
    pub market_email: String,
    pub market_subdomain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_prefix: Option<String>,
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
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_amount_usd: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_sale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sale_market_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_access_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_with_emails: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_by_app: Option<BTreeMap<String, ShareAppAccess>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_settings: Option<BTreeMap<String, ShareAppSettings>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_sale_official_price_percent_by_app: Option<BTreeMap<String, u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_grants: Option<BTreeMap<String, ShareUserGrant>>,
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareTokenPeriod {
    #[default]
    Lifetime,
    Day,
    Week,
    CalendarMonth,
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
    pub expires_at: Option<i64>,
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
    #[serde(default)]
    pub created_at_ms: u128,
    #[serde(default)]
    pub updated_at_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<u128>,
    #[serde(default)]
    pub revision: u64,
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
    pub app: String,
    pub period: String,
    pub bucket_granularity: String,
    pub days: u32,
    pub total_tokens: u64,
    pub rows: Vec<ShareUsageEmailRow>,
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
    pub market_kind: String,
    pub status: String,
    #[serde(default)]
    pub maintenance_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_summary: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct MarketRegistryRecord {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub subdomain: String,
    pub public_base_url: String,
    pub market_kind: String,
    pub scopes: Vec<String>,
    pub status: String,
    pub maintenance_enabled: bool,
    pub maintenance_message: Option<String>,
}

impl MarketRegistryRecord {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.status.eq_ignore_ascii_case("active") && self.scopes.iter().any(|value| value == scope)
    }
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
pub struct RegisterMarketRequest {
    pub subdomain: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub public_base_url: String,
    #[serde(default)]
    pub market_kind: Option<String>,
    #[serde(default)]
    pub pricing_summary: Option<serde_json::Value>,
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
pub struct MarketShareAppView {
    pub app: String,
    pub supported: bool,
    pub visible: bool,
    pub for_sale: String,
    pub sale_market_kind: String,
    pub market_access_mode: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShareView {
    pub router_id: String,
    pub share_id: String,
    pub subdomain: String,
    pub installation_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_owner_email: Option<String>,
    pub app_type: String,
    pub for_sale: String,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
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
    /// RFC3339 timestamp from `shares.created_at`. Used by markets as a
    /// freshness/seniority input for diversification profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_created_at: Option<String>,
    #[serde(default)]
    pub disabled_by_market: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_disabled_at: Option<String>,
    #[serde(default)]
    pub support: ShareSupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<ShareUpstreamProvider>,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
    #[serde(default)]
    pub model_health: ShareModelHealthSummary,
    #[serde(default)]
    pub app_availability: MarketAppAvailability,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub market_apps: BTreeMap<String, MarketShareAppView>,
    #[serde(default)]
    pub market_states: Vec<MarketShareRuntimeStateView>,
    /// Router-computed scheduling signals. Markets sort using these directly
    /// (no recomputation) and then layer their profile preferences on top.
    #[serde(default)]
    pub signals: ShareSignals,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketGrantRequest {
    pub grant_id: String,
    pub action: String,
    #[serde(default)]
    pub app_type: Option<String>,
    #[serde(default)]
    pub buyer_emails: Vec<String>,
    #[serde(default)]
    pub order_ids: Vec<String>,
    #[serde(default)]
    pub listing_id: Option<String>,
    #[serde(default)]
    pub carpool_group_id: Option<String>,
    #[serde(default)]
    pub seat_count: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketGrantResponse {
    pub ok: bool,
    pub grant_id: String,
    pub router_edit_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketGrantStatus {
    pub status: String,
    #[serde(default)]
    pub grant_id: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub updated_at_ms: Option<u128>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketGrantStatusResponse {
    pub ok: bool,
    pub router_edit_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at: Option<DateTime<Utc>>,
}

/// Router-computed scheduling signals shipped to markets in every
/// `/v1/market/shares` response. All values are normalized so a higher number
/// is preferred. `samples_10m` is included so the market can decide whether
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketDisabledSharesUpdateRequest {
    #[serde(default)]
    pub disabled_share_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketDisabledSharesUpdateResponse {
    pub ok: bool,
    pub disabled_share_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketMaintenanceUpdateRequest {
    pub maintenance_enabled: bool,
    #[serde(default)]
    pub maintenance_message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketMaintenanceUpdateResponse {
    pub ok: bool,
    pub maintenance_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_message: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_limit_percent: Option<f64>,
    #[serde(default)]
    pub tiers: Vec<ShareUpstreamQuotaTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamModel {
    pub slot: String,
    pub actual_model: String,
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
    pub for_sale_official_price_percent: Option<u16>,
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
            for_sale_official_price_percent: None,
            account_email: None,
            subscription_level: None,
            subscription_expires_at: None,
            subscription_remaining_ms: None,
            quota_percent: None,
            quota_blocked: None,
            quota: None,
            api_url: None,
            models: Vec::new(),
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
    pub for_sale_official_price_percent: Option<u16>,
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
            kind: None,
            provider_type: None,
            is_current: false,
            enabled: false,
            codex_image_generation_enabled: false,
            for_sale_official_price_percent: None,
            account_email: None,
            subscription_level: None,
            subscription_expires_at: None,
            subscription_remaining_ms: None,
            quota_percent: None,
            quota_blocked: None,
            quota: None,
            api_url: None,
            models: Vec::new(),
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
#[serde(rename_all = "camelCase")]
pub struct ShareDescriptor {
    pub share_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub access_by_app: BTreeMap<String, ShareAppAccess>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub app_settings: BTreeMap<String, ShareAppSettings>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub for_sale_official_price_percent_by_app: BTreeMap<String, u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_grant: Option<ShareMarketGrantStatus>,
    #[serde(default = "default_share_for_sale")]
    pub for_sale: String,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    pub subdomain: String,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Share 的唯一 app/provider binding。写入时必须恰好一项，且与
    /// `app_type` / `provider_id` 一致。
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
    #[serde(default, skip_serializing_if = "is_zero_revision")]
    pub config_revision: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
}

fn is_zero_revision(value: &u64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppAccess {
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppSettings {
    #[serde(default = "default_share_for_sale")]
    pub for_sale: String,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default)]
    pub token_limit: i64,
    #[serde(default = "default_share_parallel_limit")]
    pub parallel_limit: i64,
    #[serde(default)]
    pub expires_at: String,
}

impl Default for ShareAppSettings {
    fn default() -> Self {
        Self {
            for_sale: default_share_for_sale(),
            sale_market_kind: default_sale_market_kind(),
            market_access_mode: default_market_access_mode(),
            shared_with_emails: Vec::new(),
            token_limit: -1,
            parallel_limit: default_share_parallel_limit(),
            expires_at: String::new(),
        }
    }
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
}

impl Default for MapDisplaySettings {
    fn default() -> Self {
        Self {
            show_flows: true,
            show_heat: true,
            viewport: MapViewportSettings::default(),
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
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnouncementSettingsUpdate {
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
    pub markets: Vec<DashboardMarketView>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub market_request_logs: Vec<DashboardMarketRequestLogView>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<InstallationUpgradeView>,
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardClientView {
    pub installation: InstallationView,
    #[serde(default)]
    pub chat_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_tunnel: Option<DashboardClientTunnelView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payout_profile: Option<DashboardPayoutProfileView>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardPayoutProfileView {
    pub address_type: PayoutAddressType,
    pub address: String,
    pub token: PayoutToken,
    pub networks: Vec<PayoutNetwork>,
    pub verification_status: PayoutVerificationStatus,
    pub updated_at: DateTime<Utc>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardMarketView {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub subdomain: String,
    pub public_base_url: String,
    pub market_kind: String,
    pub status: String,
    pub online: bool,
    pub route_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_state_since: Option<String>,
    #[serde(default)]
    pub can_manage: bool,
    #[serde(default)]
    pub maintenance_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_seen_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_since: Option<String>,
    pub share_count: usize,
    pub online_share_count: usize,
    pub active_requests: usize,
    pub parallel_capacity: i64,
    /// Rolled-up "any linked share was healthy this minute" probe count over
    /// the last 24h, capped at 1440. Drives the ONLINE % and tooltip.
    pub online_minutes_24h: usize,
    pub online_rate_24h: f64,
    pub observed_minutes_24h: usize,
    pub observation_coverage_24h: f64,
    pub usage_tokens: u64,
    pub usage_amount_usd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_summary: Option<serde_json::Value>,
    /// Recent health probe trail aggregated from linked shares for the
    /// dashboard's STATUS dots. Each minute is healthy when any enabled linked
    /// share was healthy in that minute.
    #[serde(default)]
    pub health_checks: Vec<HealthCheckEntry>,
    #[serde(default)]
    pub health_timeline: Vec<HealthTimelineBucket>,
    #[serde(default)]
    pub linked_shares: Vec<MarketLinkedShareView>,
    #[serde(default)]
    pub recent_requests: Vec<DashboardMarketRequestLogView>,
    pub operational_summary: OperationalSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketLinkedShareView {
    pub share_id: String,
    pub share_name: String,
    pub subdomain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    pub app_type: String,
    pub online: bool,
    pub route_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_state_since: Option<String>,
    pub active_requests: usize,
    pub parallel_limit: i64,
    pub online_rate_24h: f64,
    pub observed_minutes_24h: usize,
    pub observation_coverage_24h: f64,
    #[serde(default)]
    pub disabled_by_market: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_disabled_at: Option<String>,
    pub support: ShareSupport,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
    #[serde(default)]
    pub app_availability: MarketAppAvailability,
    #[serde(default)]
    pub market_states: Vec<MarketShareRuntimeStateView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShareRuntimeStateView {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShareRuntimeStateSyncRequest {
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub states: Vec<MarketShareRuntimeStateInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShareRuntimeStateInput {
    pub share_id: String,
    #[serde(default)]
    pub router_id: Option<String>,
    pub scope: String,
    pub kind: String,
    #[serde(default)]
    pub app_type: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub reason_kind: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub failure_count: Option<i64>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShareRuntimeStateSyncResponse {
    pub ok: bool,
    pub synced: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketListingStatusSyncRequest {
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub statuses: Vec<ShareMarketListingStatusInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketListingStatusInput {
    #[serde(default)]
    pub router_id: Option<String>,
    pub share_id: String,
    pub app_type: String,
    pub listing_url: String,
    pub status: String,
    #[serde(default)]
    pub sale_mode: Option<String>,
    #[serde(default)]
    pub filled_seats: Option<i64>,
    #[serde(default)]
    pub required_seats: Option<i64>,
    #[serde(default)]
    pub listing_status: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketListingStatusSyncResponse {
    pub ok: bool,
    pub synced: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShareRuntimeStateReleaseRequest {
    pub router_id: String,
    pub share_id: String,
    pub kind: String,
    #[serde(default)]
    pub app_type: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShareRuntimeStateReleaseResponse {
    pub ok: bool,
    pub released: usize,
    pub synced: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketAppAvailability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<MarketAppAvailabilityEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<MarketAppAvailabilityEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<MarketAppAvailabilityEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketAppAvailabilityEntry {
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
pub struct ShareMarketListingStatusView {
    pub listing_url: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sale_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filled_seats: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_seats: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub is_stale: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketLinkView {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub subdomain: String,
    pub public_base_url: String,
    pub market_kind: String,
    pub status: String,
    pub online: bool,
    pub route_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_state_since: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub listing_status_by_app: BTreeMap<String, ShareMarketListingStatusView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    pub router_id: String,
    pub share_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub access_by_app: BTreeMap<String, ShareAppAccess>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub app_settings: BTreeMap<String, ShareAppSettings>,
    #[serde(default)]
    pub market_links: Vec<ShareMarketLinkView>,
    #[serde(default)]
    pub unknown_market_emails: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub for_sale: String,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub for_sale_official_price_percent_by_app: BTreeMap<String, u16>,
    pub subdomain: String,
    pub can_view_secret: bool,
    pub can_manage: bool,
    pub can_edit_settings: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_edit: Option<ShareEditView>,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Share 的唯一 app/provider binding，供卡片和详情展示。
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
    #[serde(default)]
    pub config_revision: u64,
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
pub struct BoardMessageView {
    pub id: String,
    pub body: String,
    pub author_kind: String,
    pub author_label: String,
    pub is_mine: bool,
    pub pinned: bool,
    pub featured: bool,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub featured_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardMessageListResponse {
    pub messages: Vec<BoardMessageView>,
    pub tab: String,
    pub total_visible: usize,
    /// Server-snapshot time clients echo back as `?since=` to receive only changes.
    pub as_of: DateTime<Utc>,
    /// IDs that became invisible to this tab since `since` (deleted, unpinned, unfeatured).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_ids: Vec<String>,
    /// True when the response is a delta against `since` rather than a full snapshot.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub incremental: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct PostBoardMessageRequest {
    pub body: String,
    #[serde(default)]
    pub guest_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct BoardMessageToggleRequest {
    pub value: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardMetaResponse {
    pub total: usize,
    pub pinned_count: usize,
    pub featured_count: usize,
    pub can_post_as_admin: bool,
    pub max_body_length: usize,
    pub guest_self_delete_secs: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatMessageView {
    pub id: String,
    pub seq: i64,
    pub body: String,
    pub author_label: String,
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
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientChatRoomView {
    pub id: String,
    pub installation_id: String,
    pub client_label: String,
    pub status: String,
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
