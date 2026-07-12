use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, hash_map::Entry};
use std::path::{Component, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use base64::Engine;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::distributions::{Alphanumeric, DistString};
use resend_rs::Resend;
use resend_rs::types::CreateEmailBaseOptions;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio::time::timeout;
use uuid::Uuid;

const MARKET_APP_AVAILABILITY_FAILURE_TTL_SECS: i64 = 30 * 60;

use crate::ServerGeo;
use crate::config::Config;
use crate::dynamic_settings::BoardSettings;
use crate::error::AppError;
use crate::models::{
    AuthSession, AuthUser, BindInstallationOwnerEmailRequest, BindInstallationOwnerEmailResponse,
    BoardMessageListResponse, BoardMessageView, BoardMetaResponse,
    ChangeInstallationOwnerEmailRequest, ChangeInstallationOwnerEmailResponse, ClientMetadata,
    ClientTunnelClaimRequest, ClientTunnelConfig, ClientTunnelQuery, ClientTunnelResponse,
    ClientTunnelUpdateRequest, ClientTunnelView, DashboardClientTunnelView, DashboardClientView,
    DashboardMap, DashboardMapPoint, DashboardMarketRequestLogView, DashboardMarketView,
    DashboardPayoutProfileView, DashboardPresenceRequest, DashboardResponse, DashboardStats,
    DashboardTickerShare, DashboardUxEventRequest, GatewayRegistryRecord,
    GetInstallationOwnerEmailQuery, GetInstallationOwnerEmailResponse, HealthCheckEntry,
    HealthTimelineBucket, ImageGenerationRequestLogEntry, Installation,
    InstallationPayoutProfileUpdateRequest, InstallationPayoutProfileUpdateResponse,
    InstallationView, IssueLeaseRequest, IssueLeaseResponse, LatLonPoint, MapDisplaySettings,
    MapDisplaySettingsUpdate, MapViewportSettings, MarketAppAvailability,
    MarketAppAvailabilityEntry, MarketDisabledSharesUpdateRequest,
    MarketDisabledSharesUpdateResponse, MarketLinkedShareView, MarketMaintenanceUpdateRequest,
    MarketMaintenanceUpdateResponse, MarketRegistryRecord, MarketRequestLogBatchSyncRequest,
    MarketRequestLogEntry, MarketShareAppView, MarketShareRuntimeStateInput,
    MarketShareRuntimeStateView, MarketShareView, ModelHealthSummary, OperationalReason,
    OperationalSummary, PayoutProfile, PublicMapClientPoint, PublicMapPointsResponse,
    PublicMarketConfig, PublicNetworkStatsResponse, PublicPayoutProfileResponse,
    PublicPayoutProfilesResponse, RefreshSessionRequest, RegisterGatewayRequest,
    RegisterGatewayResponse, RegisterInstallationRequest, RegisterInstallationResponse,
    RegisterMarketRequest, RenewLeaseRequest, RenewLeaseResponse, RequestEmailCodeRequest,
    RequestEmailCodeResponse, SessionStatusResponse, ShareAppAccess, ShareAppAvailability,
    ShareAppProviders, ShareAppRuntimes, ShareAppSettings, ShareBatchSyncRequest,
    ShareClaimPayload, ShareClaimSubdomainRequest, ShareDeleteRequest, ShareDescriptor,
    ShareEditAckRequest, ShareEditView, ShareHeartbeatRequest, ShareMarketGrantRequest,
    ShareMarketGrantResponse, ShareMarketGrantStatusResponse, ShareMarketLinkView,
    ShareMarketListingStatusInput, ShareMarketListingStatusView, ShareModelHealthCheckEntry,
    ShareModelHealthSummary, SharePendingEditsRequest, SharePendingEditsResponse,
    ShareRequestLogBatchSyncRequest, ShareRequestLogEntry, ShareRequestLogFetchResponse,
    ShareRuntimeRefreshPayload, ShareRuntimeRefreshRequest, ShareRuntimeSnapshotResponse,
    ShareSettingsPatch, ShareSettingsUpdateResponse, ShareSignals, ShareSupport, ShareSyncRequest,
    ShareUpstreamProvider, ShareUpstreamQuota, ShareUsageByEmailResponse, ShareUsageDailyBucket,
    ShareUsageEmailRow, ShareView, TunnelLease, UserApiTokenResetResponse, UserApiTokenResponse,
    UserApiTokenStatus, UserShareView, UserSharesResponse, VerifyEmailCodeRequest,
    VerifyEmailCodeResponse,
};
#[cfg(test)]
use crate::models::{RenewLeasePayload, ShareAppProvider, ShareUpstreamModel};
use crate::proxy::ProxyRegistry;
#[cfg(test)]
use crate::proxy::RouteKind;

const SHARE_REQUEST_LOG_RECOVERY_LIMIT: usize = 10;
pub const IMAGE_GENERATION_REQUEST_LOG_RETAIN_PER_SHARE: usize = 10;
const SHARE_MODEL_HEALTH_CHECK_LIMIT: usize = 10;
const SHARE_REQUEST_LOG_RECOVERY_STALE_SECS: i64 = 10 * 60;
const SHARE_REQUEST_LOG_RECOVERY_COOLDOWN_SECS: i64 = 5 * 60;
const ROUTE_REGISTRATION_PENDING_GRACE_SECS: u64 = 30;
const PUBLIC_MAP_CLIENT_ACTIVE_WINDOW_MINUTES: i64 = 5;
const ONLINE_WINDOW_MINUTES: usize = 24 * 60;
const HEALTH_TIMELINE_BUCKETS: usize = 48;
const HEALTH_TIMELINE_BUCKET_SECS: i64 = 30 * 60;
const SIGNED_REQUEST_MAX_SKEW_MS: i64 = 60_000;
const NONCE_RETENTION_SECS: i64 = 10 * 60;
const MARKET_OFFLINE_GRACE_SECS: i64 = 24 * 60 * 60;
const MARKET_ACTIVE_MISSING_GRACE_SECS: i64 = 5 * 60;
const CLEANUP_ACTIVE_SUBDOMAIN_CHUNK_SIZE: usize = 500;
const AUTH_CODE_DIGITS: usize = 6;
const AUTH_PURPOSE_LOGIN: &str = "login";
const MARKET_DEFAULT_SCOPES: &[&str] = &[
    "market:shares:read",
    "market:proxy:use",
    "market:email:notify",
    "market:request_logs:write",
    "market:share_states:write",
    "market:share_states:release",
    "market:share_grants:write",
];
const GATEWAY_DEFAULT_SCOPES: &[&str] = &[
    "gateway:shares:read",
    "gateway:proxy:use",
    "gateway:feedback:write",
    "gateway:request_logs:write",
];
const USER_DEFAULT_API_TOKEN_SCOPES: &[&str] = &["share:read", "share:write", "share:invoke"];
const USER_DEFAULT_API_TOKEN_NAME: &str = "default";
const DASHBOARD_EXPIRY_WARNING_DAYS: i64 = 7;
const DASHBOARD_CAPACITY_WARNING_RATIO: f64 = 0.9;
const DASHBOARD_HIGH_LATENCY_MS: u64 = 2_000;

fn operational_reason(
    code: &str,
    severity: &str,
    started_at: Option<String>,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    current_value: Option<String>,
    threshold: Option<String>,
) -> OperationalReason {
    OperationalReason {
        code: code.to_string(),
        severity: severity.to_string(),
        started_at,
        entity_type: entity_type.map(str::to_string),
        entity_id: entity_id.map(str::to_string),
        current_value,
        threshold,
    }
}

fn operational_summary(state: &str, reasons: Vec<OperationalReason>) -> OperationalSummary {
    let changed_at = reasons.first().and_then(|reason| reason.started_at.clone());
    OperationalSummary {
        state: state.to_string(),
        primary_reason: reasons.first().cloned(),
        additional_reason_count: reasons.len().saturating_sub(1),
        changed_at,
    }
}

fn unix_timestamp_rfc3339(value: i64) -> Option<String> {
    Utc.timestamp_opt(value, 0)
        .single()
        .map(|timestamp| timestamp.to_rfc3339())
}

fn share_model_health_failed(share: &ShareView) -> bool {
    let entries = match share.app_type.trim().to_ascii_lowercase().as_str() {
        "claude" => &share.model_health.claude,
        "codex" => &share.model_health.codex,
        "gemini" => &share.model_health.gemini,
        _ => return false,
    };
    !entries.is_empty()
        && entries.iter().all(|entry| {
            let recent = entry
                .recent_results
                .iter()
                .rev()
                .take(3)
                .collect::<Vec<_>>();
            (!recent.is_empty() && recent.iter().all(|result| result.as_str() == "failed"))
                || matches!(entry.status.as_str(), "failed" | "unavailable" | "blocked")
        })
}

fn share_operational_summary(share: &ShareView, now: DateTime<Utc>) -> OperationalSummary {
    let status = share.share_status.trim().to_ascii_lowercase();
    if status != "active" {
        let (code, severity) = if status == "expired" {
            ("expired", "critical")
        } else {
            ("manually_disabled", "info")
        };
        return operational_summary(
            "disabled",
            vec![operational_reason(
                code,
                severity,
                (status == "expired").then(|| share.expires_at.clone()),
                Some("share"),
                Some(&share.share_id),
                Some(status),
                None,
            )],
        );
    }

    let mut reasons = Vec::new();
    if !share.is_online {
        reasons.push(operational_reason(
            "route_offline",
            "critical",
            None,
            Some("share"),
            Some(&share.share_id),
            None,
            None,
        ));
    }
    if let Some(edit) = &share.active_edit {
        if edit.status == "rejected" {
            reasons.push(operational_reason(
                "edit_failed",
                "critical",
                Some(edit.updated_at.to_rfc3339()),
                Some("share"),
                Some(&share.share_id),
                edit.error_message.clone(),
                None,
            ));
        } else if edit.status == "pending" {
            reasons.push(operational_reason(
                "edit_pending",
                "warning",
                Some(edit.updated_at.to_rfc3339()),
                Some("share"),
                Some(&share.share_id),
                None,
                None,
            ));
        }
    }

    if let Ok(expires_at) = DateTime::parse_from_rfc3339(&share.expires_at) {
        let remaining = expires_at.with_timezone(&Utc) - now;
        if expires_at.year() < 2099 {
            if remaining.num_seconds() <= 0 {
                reasons.push(operational_reason(
                    "expired",
                    "critical",
                    Some(share.expires_at.clone()),
                    Some("share"),
                    Some(&share.share_id),
                    Some("0".to_string()),
                    None,
                ));
            } else if remaining <= Duration::days(DASHBOARD_EXPIRY_WARNING_DAYS) {
                reasons.push(operational_reason(
                    "expires_soon",
                    "warning",
                    Some(
                        (expires_at.with_timezone(&Utc)
                            - Duration::days(DASHBOARD_EXPIRY_WARNING_DAYS))
                        .to_rfc3339(),
                    ),
                    Some("share"),
                    Some(&share.share_id),
                    Some(remaining.num_seconds().to_string()),
                    Some((Duration::days(DASHBOARD_EXPIRY_WARNING_DAYS).num_seconds()).to_string()),
                ));
            }
        }
    }

    if share_model_health_failed(share) {
        let checked_at = share
            .recent_model_health_checks
            .iter()
            .map(|entry| entry.checked_at)
            .max()
            .and_then(unix_timestamp_rfc3339);
        reasons.push(operational_reason(
            "provider_unavailable",
            "critical",
            checked_at,
            Some("provider"),
            share.provider_id.as_deref(),
            None,
            None,
        ));
    }

    let app_key = share.app_type.trim().to_ascii_lowercase();
    let settings = share.app_settings.get(&app_key);
    let parallel_limit = settings
        .map(|value| value.parallel_limit)
        .unwrap_or(share.parallel_limit);
    let token_limit = settings
        .map(|value| value.token_limit)
        .unwrap_or(share.token_limit);
    let active_requests = share
        .active_requests_by_app
        .get(&app_key)
        .copied()
        .unwrap_or(share.active_requests);
    let tokens_used = share
        .tokens_used_by_app
        .get(&app_key)
        .copied()
        .unwrap_or(share.tokens_used);
    if parallel_limit > 0 && active_requests as i64 >= parallel_limit {
        reasons.push(operational_reason(
            "parallel_capacity_full",
            "critical",
            None,
            Some("share"),
            Some(&share.share_id),
            Some(active_requests.to_string()),
            Some(parallel_limit.to_string()),
        ));
    }
    if token_limit > 0
        && tokens_used >= 0
        && tokens_used as f64 / token_limit as f64 >= DASHBOARD_CAPACITY_WARNING_RATIO
    {
        reasons.push(operational_reason(
            "usage_limit_warning",
            "warning",
            None,
            Some("share"),
            Some(&share.share_id),
            Some(tokens_used.to_string()),
            Some(token_limit.to_string()),
        ));
    }
    if let Some(health) = share.health_checks.last().filter(|entry| !entry.is_healthy) {
        reasons.push(operational_reason(
            "health_check_failed",
            "warning",
            unix_timestamp_rfc3339(health.checked_at),
            Some("share"),
            Some(&share.share_id),
            None,
            None,
        ));
    }
    let recent_latency = share
        .recent_requests
        .iter()
        .rev()
        .take(10)
        .filter(|request| !request.is_health_check)
        .map(|request| request.latency_ms)
        .filter(|value| *value > 0)
        .collect::<Vec<_>>();
    if !recent_latency.is_empty() {
        let average = recent_latency.iter().sum::<u64>() / recent_latency.len() as u64;
        if average >= DASHBOARD_HIGH_LATENCY_MS {
            reasons.push(operational_reason(
                "high_latency",
                "warning",
                share
                    .recent_requests
                    .last()
                    .and_then(|request| unix_timestamp_rfc3339(request.created_at)),
                Some("share"),
                Some(&share.share_id),
                Some(average.to_string()),
                Some(DASHBOARD_HIGH_LATENCY_MS.to_string()),
            ));
        }
    }

    if !share.is_online {
        operational_summary("offline", reasons)
    } else if reasons.is_empty() {
        OperationalSummary::healthy("online")
    } else {
        operational_summary("degraded", reasons)
    }
}

fn client_operational_summary(
    client: &DashboardClientView,
    shares_by_id: &HashMap<String, &ShareView>,
    stale_seconds: i64,
    now: DateTime<Utc>,
) -> OperationalSummary {
    let shares = client
        .share_ids
        .iter()
        .filter_map(|share_id| shares_by_id.get(share_id).copied())
        .collect::<Vec<_>>();
    let enabled = shares
        .iter()
        .filter(|share| share.share_status.eq_ignore_ascii_case("active"))
        .copied()
        .collect::<Vec<_>>();
    let online = enabled.iter().filter(|share| share.is_online).count();
    let mut reasons = Vec::new();
    if !enabled.is_empty() && online == 0 {
        reasons.push(operational_reason(
            "route_offline",
            "critical",
            None,
            Some("client"),
            Some(&client.installation.id),
            Some("0".to_string()),
            Some(enabled.len().to_string()),
        ));
    } else if online < enabled.len() {
        reasons.push(operational_reason(
            "partial_share_outage",
            "warning",
            None,
            Some("client"),
            Some(&client.installation.id),
            Some(online.to_string()),
            Some(enabled.len().to_string()),
        ));
    }
    if let Some(degraded_share) = enabled
        .iter()
        .find(|share| share.operational_summary.state == "degraded")
    {
        if let Some(mut reason) = degraded_share.operational_summary.primary_reason.clone() {
            reason.entity_type = Some("share".to_string());
            reason.entity_id = Some(degraded_share.share_id.clone());
            reasons.push(reason);
        }
    }
    if let Some(health) = (!enabled.is_empty() || shares.is_empty())
        .then(|| client.health_checks.last())
        .flatten()
        .filter(|entry| !entry.is_healthy)
    {
        reasons.push(operational_reason(
            "health_check_failed",
            "warning",
            unix_timestamp_rfc3339(health.checked_at),
            Some("client"),
            Some(&client.installation.id),
            None,
            None,
        ));
    }

    let no_enabled_route = enabled.is_empty();
    let tunnel_offline = client
        .client_tunnel
        .as_ref()
        .is_some_and(|tunnel| tunnel.enabled && !tunnel.online);
    let heartbeat_stale = now - client.installation.last_seen_at > Duration::seconds(stale_seconds);
    if no_enabled_route
        && (tunnel_offline
            || (client.client_tunnel.is_none() && shares.is_empty() && heartbeat_stale))
    {
        reasons.insert(
            0,
            operational_reason(
                "route_offline",
                "critical",
                Some(client.installation.last_seen_at.to_rfc3339()),
                Some("client"),
                Some(&client.installation.id),
                None,
                None,
            ),
        );
    }

    if reasons
        .first()
        .is_some_and(|reason| reason.severity == "critical" && reason.code == "route_offline")
    {
        operational_summary("offline", reasons)
    } else if reasons.is_empty() {
        OperationalSummary::healthy("online")
    } else {
        operational_summary("degraded", reasons)
    }
}

fn market_operational_summary(market: &DashboardMarketView) -> OperationalSummary {
    let status = market.status.trim().to_ascii_lowercase();
    if status == "disabled" {
        return operational_summary(
            "disabled",
            vec![operational_reason(
                "manually_disabled",
                "info",
                Some(market.updated_at.clone()),
                Some("market"),
                Some(&market.id),
                None,
                None,
            )],
        );
    }
    if market.maintenance_enabled {
        return operational_summary(
            "maintenance",
            vec![operational_reason(
                "maintenance_enabled",
                "info",
                Some(market.updated_at.clone()),
                Some("market"),
                Some(&market.id),
                market.maintenance_message.clone(),
                None,
            )],
        );
    }
    let mut reasons = Vec::new();
    if !market.online || status == "offline" {
        reasons.push(operational_reason(
            "route_offline",
            "critical",
            market
                .offline_since
                .clone()
                .or_else(|| Some(market.last_seen_at.clone())),
            Some("market"),
            Some(&market.id),
            None,
            None,
        ));
    }
    if market.online_share_count == 0 {
        reasons.push(operational_reason(
            "no_online_shares",
            "critical",
            market.offline_since.clone(),
            Some("market"),
            Some(&market.id),
            Some("0".to_string()),
            Some(market.share_count.max(1).to_string()),
        ));
    }
    if market.parallel_capacity > 0 {
        let percent = market.active_requests as f64 / market.parallel_capacity as f64;
        if percent >= 1.0 {
            reasons.push(operational_reason(
                "parallel_capacity_full",
                "critical",
                None,
                Some("market"),
                Some(&market.id),
                Some(market.active_requests.to_string()),
                Some(market.parallel_capacity.to_string()),
            ));
        } else if percent >= DASHBOARD_CAPACITY_WARNING_RATIO {
            reasons.push(operational_reason(
                "parallel_capacity_warning",
                "warning",
                None,
                Some("market"),
                Some(&market.id),
                Some(market.active_requests.to_string()),
                Some(market.parallel_capacity.to_string()),
            ));
        }
    }
    if let Some(health) = market
        .health_checks
        .last()
        .filter(|entry| !entry.is_healthy)
    {
        reasons.push(operational_reason(
            "health_check_failed",
            "warning",
            unix_timestamp_rfc3339(health.checked_at),
            Some("market"),
            Some(&market.id),
            None,
            None,
        ));
    }
    if !market.online || status == "offline" {
        operational_summary("offline", reasons)
    } else if reasons.is_empty() {
        OperationalSummary::healthy("available")
    } else {
        operational_summary("degraded", reasons)
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BindOwnerEmailSignaturePayload<'a> {
    email: &'a str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification_token: Option<&'a str>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangeOwnerEmailSignaturePayload<'a> {
    old_email: &'a str,
    new_email: &'a str,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerificationRedeemResponse {
    ok: bool,
    email: String,
    purpose: String,
    verified_at: i64,
}

#[derive(Debug, Clone)]
pub struct UserApiTokenPrincipal {
    pub user_id: String,
    pub email: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone)]
struct UserApiTokenRecord {
    raw_token: Option<String>,
    prefix: String,
    scopes: Vec<String>,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

use crate::geo::country_centroid;

#[derive(Clone)]
pub struct AppStore {
    conn: Arc<Mutex<Connection>>,
    share_log_recovery_attempts: Arc<Mutex<HashMap<String, i64>>>,
    ip_hash_salt: Arc<String>,
}

/// P18: 测试接続で必要な share の基本情報。
#[derive(Debug, Clone)]
pub struct ShareForTest {
    pub subdomain: String,
    pub owner_email: String,
    pub shared_with_emails: Vec<String>,
    pub bindings: BTreeMap<String, String>,
    pub app_providers: ShareAppProviders,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSchedulingRecovery {
    pub share_model_health_deleted: usize,
    pub market_model_failures_deleted: usize,
    pub market_runtime_states_deleted: usize,
}

impl ShareSchedulingRecovery {
    pub fn changed(&self) -> bool {
        self.share_model_health_deleted > 0
            || self.market_model_failures_deleted > 0
            || self.market_runtime_states_deleted > 0
    }
}

#[derive(Debug, Clone)]
pub struct NewImageGenerationRequestLog {
    pub request_id: String,
    pub share_id: String,
    pub installation_id: String,
    pub share_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub app_type: String,
    pub model: String,
    pub status: String,
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub prompt_preview: Option<String>,
    pub error_message: Option<String>,
    pub result_mime_type: Option<String>,
    pub result_size_bytes: Option<u64>,
    pub result_storage_key: Option<String>,
    pub result_access_token: Option<String>,
    pub created_by_email: Option<String>,
    pub client_ip: Option<String>,
    pub user_country: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImageGenerationResultAccess {
    pub storage_key: String,
    pub mime_type: Option<String>,
}

pub fn image_results_root(config: &Config) -> PathBuf {
    config
        .db_path
        .parent()
        .map(|parent| parent.join("image-results"))
        .unwrap_or_else(|| PathBuf::from("./image-results"))
}

pub fn image_result_path(config: &Config, storage_key: &str) -> Option<PathBuf> {
    let relative = std::path::Path::new(storage_key);
    if storage_key.trim().is_empty()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        })
    {
        return None;
    }
    Some(image_results_root(config).join(relative))
}

#[derive(Debug, Clone)]
struct GeoLookupResult {
    country_code: Option<String>,
    country: Option<String>,
    region: Option<String>,
    city: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

#[derive(Debug, Clone)]
struct InstallationGeoState {
    last_seen_ip: Option<String>,
    country_code: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    geo_candidate_country_code: Option<String>,
    geo_candidate_latitude: Option<f64>,
    geo_candidate_longitude: Option<f64>,
    geo_candidate_hits: i64,
    geo_candidate_first_seen_at: Option<DateTime<Utc>>,
    geo_last_changed_at: Option<DateTime<Utc>>,
}

const GEO_STABLE_DISTANCE_KM: f64 = 120.0;
const GEO_CANDIDATE_DISTANCE_KM: f64 = 120.0;
const GEO_CANDIDATE_CONFIRM_HITS: i64 = 3;
const GEO_CANDIDATE_MIN_AGE_SECS: i64 = 10 * 60;
const GEO_STABLE_MIN_SWITCH_SECS: i64 = 30 * 60;

#[derive(Debug, Clone)]
pub struct ShareRouteTarget {
    pub share_id: String,
    pub installation_id: String,
    pub share_name: String,
    pub subdomain: String,
    pub app_runtimes: ShareAppRuntimes,
}

#[derive(Debug, Clone)]
pub struct ClientTunnelRouteTarget {
    pub installation_id: String,
    pub subdomain: String,
}

#[derive(Debug, Default)]
pub struct CleanupResult {
    pub deleted_leases: usize,
    pub deleted_shares: usize,
    pub deleted_installations: usize,
    pub removed_routes: usize,
}

impl CleanupResult {
    pub fn has_changes(&self) -> bool {
        self.deleted_leases > 0
            || self.deleted_shares > 0
            || self.deleted_installations > 0
            || self.removed_routes > 0
    }
}

impl AppStore {
    pub fn new(config: &Config) -> Result<Self, AppError> {
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::Internal(format!("create db dir failed: {e}")))?;
        }
        let conn = Connection::open(&config.db_path)
            .map_err(|e| AppError::Internal(format!("open db failed: {e}")))?;
        init_schema(&conn)?;
        let salt = Alphanumeric.sample_string(&mut rand::thread_rng(), 32);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            share_log_recovery_attempts: Arc::new(Mutex::new(HashMap::new())),
            ip_hash_salt: Arc::new(salt),
        })
    }

    pub async fn register_installation(
        &self,
        input: RegisterInstallationRequest,
        metadata: ClientMetadata,
    ) -> Result<RegisterInstallationResponse, AppError> {
        let public_key = input.public_key.trim();
        if public_key.is_empty() {
            return Err(AppError::BadRequest("public_key is required".into()));
        }
        validate_ed25519_public_key(public_key)?;
        let platform = input.platform.trim();
        if platform.is_empty() {
            return Err(AppError::BadRequest("platform is required".into()));
        }
        validate_request_nonce(&input.instance_nonce)?;
        let now = Utc::now();
        let ip = metadata.ip.clone();
        let country_code = metadata.country_code.clone();
        let new_control_secret = Alphanumeric.sample_string(&mut rand::thread_rng(), 44);
        let conn = self.conn.lock().await;
        consume_request_nonce(
            &conn,
            &registration_nonce_subject(public_key),
            "register_installation",
            &input.instance_nonce,
            now,
        )?;
        if let Some(existing_installation_id) =
            find_installation_id_by_public_key(&conn, public_key)?
        {
            let return_control_secret = verify_registration_recovery_signature(
                public_key,
                &input,
                &existing_installation_id,
                now,
            )?;
            conn.execute(
                "UPDATE installations
                 SET public_key = ?2,
                     platform = ?3,
                     app_version = ?4,
                     last_seen_ip = COALESCE(?5, last_seen_ip),
                     country_code = COALESCE(?6, country_code),
                     last_seen_at = ?7,
                     control_secret_b64 = COALESCE(control_secret_b64, ?8)
                 WHERE id = ?1",
                params![
                    existing_installation_id,
                    public_key,
                    platform,
                    input.app_version,
                    ip,
                    country_code,
                    now.to_rfc3339(),
                    new_control_secret,
                ],
            )
            .map_err(|e| AppError::Internal(format!("update installation failed: {e}")))?;
            let control_secret = if return_control_secret {
                conn.query_row(
                    "SELECT control_secret_b64 FROM installations WHERE id = ?1",
                    params![existing_installation_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(|e| AppError::Internal(format!("read control secret failed: {e}")))?
                .flatten()
            } else {
                None
            };
            drop(conn);
            self.refresh_installation_geo(&existing_installation_id, &ip, true)
                .await?;
            return Ok(RegisterInstallationResponse {
                installation_id: existing_installation_id,
                control_secret,
            });
        }

        let installation = Installation {
            id: Uuid::new_v4().to_string(),
            public_key: public_key.to_string(),
            platform: platform.to_string(),
            app_version: input.app_version,
            owner_email: None,
            owner_verified_at: None,
            last_seen_ip: ip.clone(),
            country_code,
            country: None,
            region: None,
            city: None,
            latitude: None,
            longitude: None,
            geo_candidate_country_code: None,
            geo_candidate_country: None,
            geo_candidate_region: None,
            geo_candidate_city: None,
            geo_candidate_latitude: None,
            geo_candidate_longitude: None,
            geo_candidate_hits: 0,
            geo_candidate_first_seen_at: None,
            geo_last_changed_at: None,
            created_at: now,
            last_seen_at: now,
        };
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, owner_email, owner_verified_at, last_seen_ip, country_code, country, region,
                city, latitude, longitude, geo_candidate_country_code, geo_candidate_country,
                geo_candidate_region, geo_candidate_city, geo_candidate_latitude,
                geo_candidate_longitude, geo_candidate_hits, geo_candidate_first_seen_at,
                geo_last_changed_at, created_at, last_seen_at, control_secret_b64
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
            params![
                installation.id,
                installation.public_key,
                installation.platform,
                installation.app_version,
                installation.owner_email,
                installation
                    .owner_verified_at
                    .map(|value| value.to_rfc3339()),
                installation.last_seen_ip,
                installation.country_code,
                installation.country,
                installation.region,
                installation.city,
                installation.latitude,
                installation.longitude,
                installation.geo_candidate_country_code,
                installation.geo_candidate_country,
                installation.geo_candidate_region,
                installation.geo_candidate_city,
                installation.geo_candidate_latitude,
                installation.geo_candidate_longitude,
                installation.geo_candidate_hits,
                installation
                    .geo_candidate_first_seen_at
                    .map(|value| value.to_rfc3339()),
                installation.geo_last_changed_at.map(|value| value.to_rfc3339()),
                installation.created_at.to_rfc3339(),
                installation.last_seen_at.to_rfc3339(),
                new_control_secret,
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert installation failed: {e}")))?;
        drop(conn);
        self.refresh_installation_geo(&installation.id, &ip, true)
            .await?;
        Ok(RegisterInstallationResponse {
            installation_id: installation.id,
            control_secret: Some(new_control_secret),
        })
    }

    pub async fn installation_control_secret(
        &self,
        installation_id: &str,
    ) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().await;
        let secret: Option<String> = conn
            .query_row(
                "SELECT control_secret_b64 FROM installations WHERE id = ?1",
                params![installation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("read control secret failed: {e}")))?
            .flatten();
        Ok(secret)
    }

    pub async fn update_installation_payout_profile(
        &self,
        input: InstallationPayoutProfileUpdateRequest,
    ) -> Result<InstallationPayoutProfileUpdateResponse, AppError> {
        if input.update.schema_version != crate::models::PAYOUT_PROFILE_SCHEMA_VERSION {
            return Err(AppError::BadRequest(
                "unsupported payout profile schema version".into(),
            ));
        }
        if input.update.revision < 1 {
            return Err(AppError::BadRequest(
                "payout profile revision must be positive".into(),
            ));
        }
        if input.update.updated_at_ms <= 0 {
            return Err(AppError::BadRequest(
                "payout profile updatedAtMs must be positive".into(),
            ));
        }
        if input.update.updated_at_ms > Utc::now().timestamp_millis() + 5 * 60 * 1000 {
            return Err(AppError::BadRequest(
                "payout profile updatedAtMs cannot be in the future".into(),
            ));
        }
        let normalized_profile = input
            .update
            .profile
            .clone()
            .map(PayoutProfile::validate_and_normalize)
            .transpose()
            .map_err(AppError::BadRequest)?;
        let profile_json = normalized_profile
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                AppError::Internal(format!("serialize payout profile failed: {error}"))
            })?;

        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "update_installation_payout_profile",
            &input.update,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        let owner_email = installation
            .owner_email
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                AppError::Conflict(
                    "installation owner email must be verified before publishing payout information"
                        .into(),
                )
            })?;

        let existing = conn
            .query_row(
                "SELECT revision, profile_json, source_updated_at_ms
                 FROM installation_payout_profiles WHERE installation_id = ?1",
                params![input.installation_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read payout profile revision failed: {error}"))
            })?;
        if let Some((revision, existing_json, source_updated_at_ms)) = existing {
            if input.update.revision < revision {
                return Err(AppError::Conflict(format!(
                    "stale payout profile revision: current revision is {revision}"
                )));
            }
            if input.update.revision == revision {
                if existing_json == profile_json
                    && source_updated_at_ms == input.update.updated_at_ms
                {
                    conn.execute(
                        "UPDATE installation_payout_profiles SET owner_email = ?2 WHERE installation_id = ?1",
                        params![input.installation_id, owner_email],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("refresh payout profile owner failed: {error}"))
                    })?;
                    return Ok(InstallationPayoutProfileUpdateResponse { ok: true, revision });
                }
                return Err(AppError::Conflict(
                    "payout profile revision already exists with different content".into(),
                ));
            }
        }

        conn.execute(
            "INSERT INTO installation_payout_profiles (
                installation_id, owner_email, revision, profile_json,
                source_updated_at_ms, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(installation_id) DO UPDATE SET
                owner_email = excluded.owner_email,
                revision = excluded.revision,
                profile_json = excluded.profile_json,
                source_updated_at_ms = excluded.source_updated_at_ms,
                updated_at = excluded.updated_at",
            params![
                input.installation_id,
                owner_email,
                input.update.revision,
                profile_json,
                input.update.updated_at_ms,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "upsert installation payout profile failed: {error}"
            ))
        })?;

        Ok(InstallationPayoutProfileUpdateResponse {
            ok: true,
            revision: input.update.revision,
        })
    }

    pub async fn public_installation_payout_profile(
        &self,
        installation_id: &str,
    ) -> Result<PublicPayoutProfileResponse, AppError> {
        let conn = self.conn.lock().await;
        public_payout_profile_for_installation(&conn, installation_id)?
            .ok_or_else(|| AppError::NotFound("installation not found".into()))
    }

    pub async fn public_installation_payout_profiles(
        &self,
        installation_ids: &[String],
    ) -> Result<PublicPayoutProfilesResponse, AppError> {
        let conn = self.conn.lock().await;
        let mut profiles = Vec::with_capacity(installation_ids.len());
        for installation_id in installation_ids {
            if let Some(profile) = public_payout_profile_for_installation(&conn, installation_id)? {
                profiles.push(profile);
            }
        }
        Ok(PublicPayoutProfilesResponse { profiles })
    }

    pub async fn bind_installation_owner_email(
        &self,
        config: &Config,
        input: BindInstallationOwnerEmailRequest,
        access_token: Option<&str>,
    ) -> Result<BindInstallationOwnerEmailResponse, AppError> {
        let email = normalize_email(&input.email)?;
        let now = Utc::now();
        let payload = BindOwnerEmailSignaturePayload {
            email: &email,
            verification_token: input
                .verification_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()),
        };

        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "bind_installation_owner_email",
            &payload,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;

        if let Some(existing_owner_email) = installation.owner_email.as_deref() {
            if existing_owner_email != email {
                return Err(AppError::Conflict(
                    "this installation is locked to a different owner email".into(),
                ));
            }
            return Ok(BindInstallationOwnerEmailResponse {
                ok: true,
                owner_email: email,
                already_bound: true,
            });
        }
        drop(conn);

        let verified_at = if let Some(verification_token) = payload.verification_token {
            let redeemed = redeem_verification_token(config, verification_token).await?;
            if !redeemed.ok || redeemed.purpose != AUTH_PURPOSE_LOGIN || redeemed.email != email {
                return Err(AppError::Unauthorized(
                    "verification token does not match requested owner email".into(),
                ));
            }
            DateTime::<Utc>::from_timestamp(redeemed.verified_at, 0).unwrap_or(now)
        } else {
            let access_token = access_token
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    AppError::Unauthorized(
                        "verification token or authenticated session is required to bind installation owner"
                            .into(),
                    )
                })?;
            let session = self
                .resolve_session_by_access_token(access_token)
                .await?
                .ok_or_else(|| AppError::Unauthorized("session not found".into()))?;
            if session.email != email {
                return Err(AppError::Unauthorized(
                    "authenticated session email does not match requested owner email".into(),
                ));
            }
            session.last_used_at
        };

        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE installations
             SET owner_email = ?2, owner_verified_at = ?3
             WHERE id = ?1
               AND (owner_email IS NULL OR owner_email = '' OR owner_email = ?2)",
            params![input.installation_id, email, verified_at.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("bind installation owner email failed: {e}")))?;

        Ok(BindInstallationOwnerEmailResponse {
            ok: true,
            owner_email: email,
            already_bound: false,
        })
    }

    pub async fn change_installation_owner_email(
        &self,
        input: ChangeInstallationOwnerEmailRequest,
        access_token: Option<&str>,
    ) -> Result<ChangeInstallationOwnerEmailResponse, AppError> {
        let old_email = normalize_email(&input.old_email)?;
        let new_email = normalize_email(&input.new_email)?;
        if old_email == new_email {
            return Err(AppError::BadRequest(
                "new owner email must be different from current owner email".into(),
            ));
        }
        let access_token = access_token
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::Unauthorized("authenticated new owner session is required".into())
            })?;
        let session = self
            .resolve_session_by_access_token(access_token)
            .await?
            .ok_or_else(|| AppError::Unauthorized("session not found".into()))?;
        if session.installation_id != input.installation_id {
            return Err(AppError::Unauthorized(
                "authenticated session installation mismatch".into(),
            ));
        }
        if session.email != new_email {
            return Err(AppError::Unauthorized(
                "authenticated session email does not match new owner email".into(),
            ));
        }

        let now = Utc::now();
        let payload = ChangeOwnerEmailSignaturePayload {
            old_email: &old_email,
            new_email: &new_email,
        };
        let mut conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "change_installation_owner_email",
            &payload,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        if installation.owner_email.as_deref() != Some(old_email.as_str()) {
            return Err(AppError::Conflict(
                "current installation owner email does not match requested old email".into(),
            ));
        }

        let tx = conn
            .transaction()
            .map_err(|e| AppError::Internal(format!("begin owner email change failed: {e}")))?;
        let updated_shares =
            rebind_installation_shares_to_owner(&tx, &input.installation_id, &new_email)?;
        tx.execute(
            "UPDATE installations
                 SET owner_email = ?2, owner_verified_at = ?3
                 WHERE id = ?1
                   AND owner_email = ?4",
            params![
                input.installation_id,
                new_email,
                now.to_rfc3339(),
                old_email
            ],
        )
        .map_err(|e| AppError::Internal(format!("change installation owner email failed: {e}")))?;
        tx.execute(
            "UPDATE installation_payout_profiles
             SET owner_email = ?2
             WHERE installation_id = ?1",
            params![input.installation_id, new_email],
        )
        .map_err(|e| {
            AppError::Internal(format!("change payout profile owner email failed: {e}"))
        })?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit owner email change failed: {e}")))?;

        Ok(ChangeInstallationOwnerEmailResponse {
            ok: true,
            old_email,
            new_email,
            updated_shares,
        })
    }

    pub async fn record_dashboard_presence(
        &self,
        input: DashboardPresenceRequest,
    ) -> Result<usize, AppError> {
        let session_id = input.session_id.trim();
        if session_id.is_empty() {
            return Err(AppError::BadRequest("session_id is required".into()));
        }

        let now = Utc::now().timestamp();
        let cutoff = now - 30;
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO dashboard_presence (session_id, last_seen_at)
             VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET last_seen_at = excluded.last_seen_at",
            params![session_id, now],
        )
        .map_err(|e| AppError::Internal(format!("upsert dashboard presence failed: {e}")))?;
        conn.execute(
            "DELETE FROM dashboard_presence WHERE last_seen_at < ?1",
            params![cutoff],
        )
        .map_err(|e| AppError::Internal(format!("prune dashboard presence failed: {e}")))?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dashboard_presence WHERE last_seen_at >= ?1",
                params![cutoff],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count dashboard presence failed: {e}")))?;
        Ok(count as usize)
    }

    pub async fn record_dashboard_ux_event(
        &self,
        input: DashboardUxEventRequest,
        retention_days: u32,
    ) -> Result<(), AppError> {
        const ALLOWED_EVENTS: &[&str] = &[
            "dashboard_focus_set",
            "dashboard_focus_clear",
            "map_request_selected",
            "client_located_from_map",
            "share_located_from_request",
            "market_located_from_share",
            "drawer_opened",
            "diagnosis_evidence_opened",
            "filter_applied",
            "operation_submitted",
            "operation_verified",
        ];
        let event_type = input.event_type.trim();
        if !ALLOWED_EVENTS.contains(&event_type) {
            return Err(AppError::BadRequest(
                "unsupported dashboard UX event".into(),
            ));
        }
        let source = input.source.as_deref().map(str::trim).filter(|value| {
            matches!(
                *value,
                "map" | "client-board" | "market-table" | "drawer" | "activity"
            )
        });
        let target_type = input
            .target_type
            .as_deref()
            .map(str::trim)
            .filter(|value| matches!(*value, "request" | "client" | "share" | "market"));
        let now = Utc::now();
        let cutoff = now - Duration::days(i64::from(retention_days.clamp(1, 90)));
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO dashboard_ux_events (
                id, event_type, source, target_type, step_count, elapsed_ms, keyboard, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                Uuid::new_v4().to_string(),
                event_type,
                source,
                target_type,
                input.step_count.map(|value| i64::from(value.min(100))),
                input
                    .elapsed_ms
                    .map(|value| value.min(60 * 60 * 1000) as i64),
                i64::from(input.keyboard),
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("record dashboard UX event failed: {e}")))?;
        conn.execute(
            "DELETE FROM dashboard_ux_events
             WHERE created_at < ?1 OR id IN (
               SELECT id FROM dashboard_ux_events ORDER BY created_at DESC LIMIT -1 OFFSET 10000
             )",
            params![cutoff.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("prune dashboard UX events failed: {e}")))?;
        Ok(())
    }

    pub async fn count_sent_emails_last_24h(&self) -> Result<usize, AppError> {
        let cutoff = (Utc::now() - Duration::hours(24)).to_rfc3339();
        let conn = self.conn.lock().await;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM email_send_logs
                 WHERE status = 'sent'
                   AND created_at >= ?1",
                params![cutoff],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count sent emails failed: {e}")))?;
        Ok(count as usize)
    }

    pub async fn request_email_code(
        &self,
        config: &Config,
        resend: Option<&Resend>,
        input: RequestEmailCodeRequest,
        metadata: ClientMetadata,
    ) -> Result<RequestEmailCodeResponse, AppError> {
        let email = normalize_email(&input.email)?;
        let now = Utc::now();
        {
            let conn = self.conn.lock().await;
            let installation = get_installation(&conn, &input.installation_id)?
                .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
            verify_signed_payload(
                &installation.public_key,
                &input.installation_id,
                "auth_request_code",
                &serde_json::json!({ "email": email, "purpose": AUTH_PURPOSE_LOGIN }),
                input.timestamp_ms,
                &input.nonce,
                &input.signature,
            )?;
            consume_request_nonce(
                &conn,
                &input.installation_id,
                "auth_request_code",
                &input.nonce,
                now,
            )?;
            enforce_auth_send_limits(
                &conn,
                config,
                &email,
                &input.installation_id,
                &metadata,
                now,
            )?;
        }

        let code = generate_numeric_code(AUTH_CODE_DIGITS);
        let resend = resend.ok_or_else(|| AppError::Internal("resend is not configured".into()))?;
        let provider_message_id =
            send_login_code_email(resend, config, &email, &code, config.auth_code_ttl_secs).await?;

        let expires_at = now + Duration::seconds(config.auth_code_ttl_secs);
        let resend_available_at = now + Duration::seconds(config.auth_code_cooldown_secs);
        let code_hash = hash_token(&format!("{email}:{code}"));
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE email_login_challenges
             SET consumed_at = ?2
             WHERE email_normalized = ?1
               AND purpose = ?3
               AND consumed_at IS NULL",
            params![email, now.to_rfc3339(), AUTH_PURPOSE_LOGIN],
        )
        .map_err(|e| AppError::Internal(format!("expire old auth challenges failed: {e}")))?;
        conn.execute(
            "INSERT INTO email_login_challenges (
                id, email_normalized, installation_id, purpose, code_hash, expires_at,
                consumed_at, attempt_count, resend_available_at, created_ip, created_user_agent, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 0, ?7, ?8, NULL, ?9)",
            params![
                Uuid::new_v4().to_string(),
                email,
                input.installation_id,
                AUTH_PURPOSE_LOGIN,
                code_hash,
                expires_at.to_rfc3339(),
                resend_available_at.to_rfc3339(),
                metadata.ip,
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert auth challenge failed: {e}")))?;
        conn.execute(
            "INSERT INTO email_send_logs (
                id, email_type, to_email, provider_message_id, status, error_message, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
            params![
                Uuid::new_v4().to_string(),
                "login_code",
                email,
                provider_message_id,
                "sent",
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert email send log failed: {e}")))?;

        Ok(RequestEmailCodeResponse {
            ok: true,
            cooldown_secs: config.auth_code_cooldown_secs,
            masked_destination: mask_email(&email),
        })
    }

    pub async fn verify_email_code(
        &self,
        config: &Config,
        input: VerifyEmailCodeRequest,
    ) -> Result<VerifyEmailCodeResponse, AppError> {
        let email = normalize_email(&input.email)?;
        let code = input.code.trim();
        if code.len() != AUTH_CODE_DIGITS || !code.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(AppError::Unauthorized("invalid verification code".into()));
        }

        let now = Utc::now();
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;

        let challenge = get_latest_active_email_challenge(
            &conn,
            &email,
            &input.installation_id,
            AUTH_PURPOSE_LOGIN,
            now,
        )?
        .ok_or_else(|| AppError::Unauthorized("verification code expired or not found".into()))?;

        if challenge.attempt_count >= config.auth_max_verify_attempts {
            return Err(AppError::TooManyRequests(
                "too many invalid verification attempts".into(),
            ));
        }

        let expected_hash = hash_token(&format!("{email}:{code}"));
        if expected_hash != challenge.code_hash {
            conn.execute(
                "UPDATE email_login_challenges
                 SET attempt_count = attempt_count + 1
                 WHERE id = ?1",
                params![challenge.id],
            )
            .map_err(|e| AppError::Internal(format!("update auth attempts failed: {e}")))?;
            return Err(AppError::Unauthorized("invalid verification code".into()));
        }

        conn.execute(
            "UPDATE email_login_challenges SET consumed_at = ?2 WHERE id = ?1",
            params![challenge.id, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("consume auth challenge failed: {e}")))?;

        let user = upsert_user_by_email(&conn, &email, now)?;
        let access_token = generate_secret(48);
        let refresh_token = generate_secret(64);
        let access_expires_at = now + Duration::seconds(config.auth_session_ttl_secs);
        let refresh_expires_at = now + Duration::seconds(config.auth_refresh_ttl_secs);
        let session = AuthSession {
            session_id: Uuid::new_v4().to_string(),
            user_id: user.id.clone(),
            email: user.email.clone(),
            installation_id: installation.id.clone(),
            access_token_hash: hash_token(&access_token),
            refresh_token_hash: hash_token(&refresh_token),
            access_expires_at,
            refresh_expires_at,
            created_at: now,
            last_used_at: now,
        };
        persist_session(&conn, &session)?;
        let (api_token, api_token_status) = ensure_default_user_api_token(&conn, &user.id, now)?;

        Ok(VerifyEmailCodeResponse {
            user,
            access_token,
            refresh_token,
            expires_at: access_expires_at,
            refresh_expires_at,
            api_token,
            api_token_prefix: Some(api_token_status.prefix),
        })
    }

    pub async fn refresh_session(
        &self,
        config: &Config,
        input: RefreshSessionRequest,
    ) -> Result<VerifyEmailCodeResponse, AppError> {
        let now = Utc::now();
        let refresh_hash = hash_token(input.refresh_token.trim());
        let conn = self.conn.lock().await;
        let current = get_session_by_refresh_hash(&conn, &refresh_hash)?
            .ok_or_else(|| AppError::Unauthorized("refresh session not found".into()))?;
        if current.refresh_expires_at < now {
            return Err(AppError::Unauthorized("refresh session expired".into()));
        }
        if current.installation_id != input.installation_id {
            return Err(AppError::Unauthorized(
                "refresh session installation mismatch".into(),
            ));
        }

        let user = get_user_by_id(&conn, &current.user_id)?
            .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;
        let access_token = generate_secret(48);
        let refresh_token = generate_secret(64);
        let access_expires_at = now + Duration::seconds(config.auth_session_ttl_secs);
        let refresh_expires_at = now + Duration::seconds(config.auth_refresh_ttl_secs);
        conn.execute(
            "UPDATE user_sessions
             SET access_token_hash = ?2,
                 refresh_token_hash = ?3,
                 access_expires_at = ?4,
                 refresh_expires_at = ?5,
                 last_used_at = ?6,
                 revoked_at = NULL
             WHERE id = ?1",
            params![
                current.session_id,
                hash_token(&access_token),
                hash_token(&refresh_token),
                access_expires_at.to_rfc3339(),
                refresh_expires_at.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("refresh session failed: {e}")))?;

        Ok(VerifyEmailCodeResponse {
            user,
            access_token,
            refresh_token,
            expires_at: access_expires_at,
            refresh_expires_at,
            api_token: None,
            api_token_prefix: get_default_user_api_token(&conn, &current.user_id)?
                .map(|token| token.prefix),
        })
    }

    pub async fn session_status(
        &self,
        access_token: Option<&str>,
        installation_id: Option<&str>,
    ) -> Result<SessionStatusResponse, AppError> {
        let owner_email = if let Some(installation_id) = installation_id {
            let conn = self.conn.lock().await;
            get_installation_owner_email(&conn, installation_id)?
        } else {
            None
        };

        let Some(access_token) = access_token.map(str::trim).filter(|v| !v.is_empty()) else {
            return Ok(SessionStatusResponse {
                authenticated: false,
                user: None,
                expires_at: None,
                installation_owner_email: owner_email,
                is_admin: false,
            });
        };

        let session = self
            .resolve_session_by_access_token(access_token)
            .await?
            .ok_or_else(|| AppError::Unauthorized("session not found".into()))?;
        Ok(SessionStatusResponse {
            authenticated: true,
            user: Some(AuthUser {
                id: session.user_id,
                email: session.email,
            }),
            expires_at: Some(session.access_expires_at),
            installation_owner_email: owner_email,
            is_admin: false,
        })
    }

    pub async fn revoke_session_by_access_token(&self, access_token: &str) -> Result<(), AppError> {
        let access_token = access_token.trim();
        if access_token.is_empty() {
            return Ok(());
        }
        let now = Utc::now();
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE user_sessions
             SET revoked_at = ?2,
                 last_used_at = ?2
             WHERE access_token_hash = ?1 AND revoked_at IS NULL",
            params![hash_token(access_token), now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("revoke session failed: {e}")))?;
        Ok(())
    }

    pub async fn get_installation_owner_email_status(
        &self,
        query: GetInstallationOwnerEmailQuery,
    ) -> Result<GetInstallationOwnerEmailResponse, AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &query.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &query.installation_id,
            "get_installation_owner_email",
            &serde_json::json!({}),
            query.timestamp_ms,
            &query.nonce,
            &query.signature,
        )?;
        Ok(GetInstallationOwnerEmailResponse {
            ok: true,
            owner_email: installation.owner_email,
        })
    }

    pub async fn get_client_tunnel(
        &self,
        config: &Config,
        query: ClientTunnelQuery,
    ) -> Result<ClientTunnelResponse, AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &query.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &query.installation_id,
            "client_tunnel_get",
            &serde_json::json!({}),
            query.timestamp_ms,
            &query.nonce,
            &query.signature,
        )?;
        let tunnel = get_client_tunnel_by_installation(&conn, &query.installation_id)?
            .map(|record| record.into_view(config));
        Ok(ClientTunnelResponse { ok: true, tunnel })
    }

    pub async fn claim_client_tunnel(
        &self,
        config: &Config,
        input: ClientTunnelClaimRequest,
        metadata: ClientMetadata,
    ) -> Result<ClientTunnelResponse, AppError> {
        self.upsert_client_tunnel(
            config,
            input.installation_id,
            "client_tunnel_claim",
            input.tunnel,
            input.timestamp_ms,
            input.nonce,
            input.signature,
            metadata,
        )
        .await
    }

    pub async fn update_client_tunnel(
        &self,
        config: &Config,
        input: ClientTunnelUpdateRequest,
        metadata: ClientMetadata,
    ) -> Result<ClientTunnelResponse, AppError> {
        self.upsert_client_tunnel(
            config,
            input.installation_id,
            "client_tunnel_update",
            input.tunnel,
            input.timestamp_ms,
            input.nonce,
            input.signature,
            metadata,
        )
        .await
    }

    async fn upsert_client_tunnel(
        &self,
        config: &Config,
        installation_id: String,
        action: &str,
        tunnel: ClientTunnelConfig,
        timestamp_ms: i64,
        nonce: String,
        signature: String,
        metadata: ClientMetadata,
    ) -> Result<ClientTunnelResponse, AppError> {
        let requested_owner = normalize_email(&tunnel.owner_email)?;
        let subdomain = normalize_subdomain(&tunnel.subdomain)?;
        ensure_subdomain_allowed(&subdomain, config)?;
        let now = Utc::now();

        let conn = self.conn.lock().await;
        ensure_subdomain_not_registered_market(&conn, &subdomain)?;
        ensure_subdomain_not_claimed_by_share(&conn, &subdomain)?;
        let installation = get_installation(&conn, &installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        let signed_payload = ClientTunnelConfig {
            owner_email: requested_owner.clone(),
            subdomain: subdomain.clone(),
            enabled: tunnel.enabled,
        };
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &installation_id,
            action,
            &signed_payload,
            timestamp_ms,
            &nonce,
            &signature,
        )?;
        let owner_email = installation
            .owner_email
            .as_deref()
            .ok_or_else(|| AppError::Conflict("installation owner email is not configured".into()))
            .and_then(normalize_email)?;
        if requested_owner != owner_email {
            return Err(AppError::Conflict(
                "client tunnel owner must match the installation owner".into(),
            ));
        }
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &installation_id, &metadata, now)?;
        conn.execute(
            "INSERT INTO installation_client_tunnels (
                installation_id, owner_email, subdomain, enabled, created_at, updated_at, last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL)
             ON CONFLICT(installation_id) DO UPDATE SET
                owner_email = excluded.owner_email,
                subdomain = excluded.subdomain,
                enabled = excluded.enabled,
                updated_at = excluded.updated_at",
            params![
                installation_id,
                signed_payload.owner_email,
                signed_payload.subdomain,
                if signed_payload.enabled { 1 } else { 0 },
                now.to_rfc3339(),
            ],
        )
        .map_err(map_client_tunnel_constraint_error)?;
        let record = get_client_tunnel_by_installation(&conn, &installation_id)?
            .ok_or_else(|| AppError::Internal("client tunnel upsert did not persist".into()))?;
        drop(conn);
        if should_refresh_geo {
            self.refresh_installation_geo(&installation_id, &metadata.ip, false)
                .await?;
        }
        Ok(ClientTunnelResponse {
            ok: true,
            tunnel: Some(record.into_view(config)),
        })
    }

    pub async fn client_tunnel_owner_email(
        &self,
        subdomain: &str,
    ) -> Result<Option<String>, AppError> {
        self.resolve_client_tunnel_owner_email(subdomain, None)
            .await
    }

    pub async fn resolve_client_tunnel_owner_email(
        &self,
        subdomain: &str,
        installation_id: Option<&str>,
    ) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().await;
        if let Some(email) = conn
            .query_row(
                "SELECT owner_email FROM installation_client_tunnels
                 WHERE subdomain = ?1 AND enabled = 1",
                params![subdomain],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query client tunnel owner failed: {e}")))?
        {
            return Ok(Some(email));
        }

        if let Some(installation_id) = installation_id {
            if let Some(record) = get_client_tunnel_by_installation(&conn, installation_id)? {
                if record.enabled {
                    return Ok(Some(record.owner_email));
                }
            }
            if let Some(installation) = get_installation(&conn, installation_id)? {
                if let Some(email) = installation.owner_email {
                    return Ok(Some(email));
                }
            }
        }

        Ok(None)
    }

    pub async fn resolve_session_by_access_token(
        &self,
        access_token: &str,
    ) -> Result<Option<AuthSession>, AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let Some(session) = get_session_by_access_hash(&conn, &hash_token(access_token))? else {
            return Ok(None);
        };
        if session.access_expires_at < now {
            return Ok(None);
        }
        conn.execute(
            "UPDATE user_sessions SET last_used_at = ?2 WHERE id = ?1",
            params![session.session_id, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("touch session failed: {e}")))?;
        Ok(Some(session))
    }

    pub async fn resolve_user_api_token(
        &self,
        token: &str,
        required_scope: &str,
    ) -> Result<Option<UserApiTokenPrincipal>, AppError> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(None);
        }
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let Some((id, user_id, email, scopes)) =
            get_user_api_token_by_hash(&conn, &hash_token(token))?
        else {
            return Ok(None);
        };
        if !scopes.iter().any(|scope| scope == required_scope) {
            return Err(AppError::Unauthorized(
                "api token scope is not allowed".into(),
            ));
        }
        conn.execute(
            "UPDATE user_api_tokens SET last_used_at = ?2 WHERE id = ?1",
            params![id, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("touch user api token failed: {e}")))?;
        Ok(Some(UserApiTokenPrincipal {
            user_id,
            email,
            scopes,
        }))
    }

    pub async fn get_default_api_token(
        &self,
        current_user_email: &str,
    ) -> Result<UserApiTokenResponse, AppError> {
        let email = normalize_email(current_user_email)?;
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let user = upsert_user_by_email(&conn, &email, now)?;
        let (_raw, record) = ensure_default_user_api_token(&conn, &user.id, now)?;
        Ok(UserApiTokenResponse {
            api_token: record.raw_token.clone(),
            token: user_api_token_status(record),
        })
    }

    pub async fn reset_default_api_token(
        &self,
        current_user_email: &str,
    ) -> Result<UserApiTokenResetResponse, AppError> {
        let email = normalize_email(current_user_email)?;
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let user = upsert_user_by_email(&conn, &email, now)?;
        let (api_token, record) = reset_default_user_api_token(&conn, &user.id, now)?;
        Ok(UserApiTokenResetResponse {
            api_token,
            token: user_api_token_status(record),
        })
    }

    /// P18: テスト接続用 — share の owner_email / subdomain / shared_with_emails を一度に返す。
    pub async fn get_share_for_test(
        &self,
        share_id: &str,
    ) -> Result<Option<ShareForTest>, AppError> {
        let conn = self.conn.lock().await;
        let row = conn
            .query_row(
                "SELECT COALESCE(subdomain, '-'), owner_email, COALESCE(shared_with_emails_json, '[]'), bindings_json, app_providers_json
                 FROM shares WHERE share_id = ?1",
                params![share_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        parse_share_bindings(row.get(3)?)?,
                        parse_app_providers(row.get(4)?)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query share for test failed: {e}")))?;
        let Some((subdomain, owner_email, shared_json, bindings, app_providers)) = row else {
            return Ok(None);
        };
        let shared_with_emails: Vec<String> =
            serde_json::from_str(&shared_json).unwrap_or_default();
        Ok(Some(ShareForTest {
            subdomain,
            owner_email,
            shared_with_emails,
            bindings,
            app_providers,
        }))
    }

    pub async fn recover_share_app_scheduling_after_successful_test(
        &self,
        share_id: &str,
        app_type: &str,
    ) -> Result<ShareSchedulingRecovery, AppError> {
        let app = app_type.trim().to_ascii_lowercase();
        if !matches!(app.as_str(), "claude" | "codex" | "gemini") {
            return Err(AppError::BadRequest(format!(
                "unsupported app for scheduling recovery: {app_type}"
            )));
        }
        let conn = self.conn.lock().await;
        let share_model_health_deleted = conn
            .execute(
                "DELETE FROM share_model_health_state
                 WHERE share_id = ?1 AND lower(app_type) = lower(?2)",
                params![share_id, app],
            )
            .map_err(|e| AppError::Internal(format!("delete share model health failed: {e}")))?;
        let market_model_failures_deleted = conn
            .execute(
                "DELETE FROM market_share_model_failure_state
                 WHERE share_id = ?1 AND lower(app_type) = lower(?2)",
                params![share_id, app],
            )
            .map_err(|e| {
                AppError::Internal(format!("delete market model failure state failed: {e}"))
            })?;
        let market_runtime_states_deleted = conn
            .execute(
                "DELETE FROM market_share_runtime_states
                 WHERE share_id = ?1
                   AND kind IN ('cooldown', 'model_block', 'capability_block')
                   AND (app_type IS NULL OR lower(app_type) = lower(?2))",
                params![share_id, app],
            )
            .map_err(|e| AppError::Internal(format!("delete market runtime states failed: {e}")))?;
        Ok(ShareSchedulingRecovery {
            share_model_health_deleted,
            market_model_failures_deleted,
            market_runtime_states_deleted,
        })
    }

    pub async fn record_image_generation_request_log(
        &self,
        log: NewImageGenerationRequestLog,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO image_generation_request_logs (
                request_id, share_id, installation_id, share_name, provider_id, provider_name,
                app_type, model, status, status_code, latency_ms, created_at, completed_at,
                prompt_preview, error_message, result_mime_type, result_size_bytes,
                result_storage_key, result_access_token, created_by_email, client_ip, user_country
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
             ON CONFLICT(request_id) DO UPDATE SET
                status = excluded.status,
                status_code = excluded.status_code,
                latency_ms = excluded.latency_ms,
                completed_at = excluded.completed_at,
                error_message = excluded.error_message,
                result_mime_type = excluded.result_mime_type,
                result_size_bytes = excluded.result_size_bytes,
                result_storage_key = COALESCE(excluded.result_storage_key, image_generation_request_logs.result_storage_key),
                result_access_token = COALESCE(excluded.result_access_token, image_generation_request_logs.result_access_token)",
            params![
                log.request_id,
                log.share_id,
                log.installation_id,
                log.share_name,
                log.provider_id,
                log.provider_name,
                log.app_type,
                log.model,
                log.status,
                log.status_code.map(i64::from),
                log.latency_ms as i64,
                log.created_at,
                log.completed_at,
                log.prompt_preview,
                log.error_message
                    .as_deref()
                    .map(|value| truncate_error(value, 1000)),
                log.result_mime_type,
                log.result_size_bytes.map(|value| value as i64),
                log.result_storage_key,
                log.result_access_token,
                log.created_by_email,
                log.client_ip,
                log.user_country,
            ],
        )
        .map_err(|e| AppError::Internal(format!("record image request log failed: {e}")))?;
        Ok(())
    }

    pub async fn list_image_generation_request_logs_for_share(
        &self,
        share_id: &str,
        limit: usize,
    ) -> Result<Vec<ImageGenerationRequestLogEntry>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT request_id, share_id, share_name, installation_id, provider_id, provider_name,
                        app_type, model, status, status_code, latency_ms, created_at, completed_at,
                        prompt_preview, error_message, result_mime_type, result_size_bytes,
                        result_storage_key, result_access_token, created_by_email, user_country
                 FROM image_generation_request_logs
                 WHERE share_id = ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
            )
            .map_err(|e| AppError::Internal(format!("prepare image request logs list failed: {e}")))?;
        let rows = stmt
            .query_map(
                params![share_id, limit.min(200) as i64],
                map_image_generation_request_log_row,
            )
            .map_err(|e| {
                AppError::Internal(format!("query image request logs list failed: {e}"))
            })?;
        collect_rows(rows)
    }

    pub async fn get_image_generation_result_for_access(
        &self,
        request_id: &str,
        access_token: &str,
    ) -> Result<Option<ImageGenerationResultAccess>, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT result_storage_key, result_mime_type
             FROM image_generation_request_logs
             WHERE request_id = ?1
               AND result_access_token = ?2
               AND result_storage_key IS NOT NULL
               AND result_storage_key != ''",
            params![request_id, access_token],
            |row| {
                Ok(ImageGenerationResultAccess {
                    storage_key: row.get(0)?,
                    mime_type: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("query image result access failed: {e}")))
    }

    pub async fn prune_image_generation_request_logs_for_share(
        &self,
        share_id: &str,
        keep: usize,
    ) -> Result<Vec<String>, AppError> {
        let conn = self.conn.lock().await;
        let stale = {
            let mut stmt = conn
                .prepare(
                    "SELECT request_id, result_storage_key
                     FROM image_generation_request_logs
                     WHERE share_id = ?1
                     ORDER BY created_at DESC, request_id DESC
                     LIMIT -1 OFFSET ?2",
                )
                .map_err(|e| {
                    AppError::Internal(format!("prepare stale image request logs failed: {e}"))
                })?;
            let rows = stmt
                .query_map(params![share_id, keep as i64], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })
                .map_err(|e| {
                    AppError::Internal(format!("query stale image request logs failed: {e}"))
                })?;
            collect_rows(rows)?
        };
        if stale.is_empty() {
            return Ok(Vec::new());
        }
        let request_ids = stale
            .iter()
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        let placeholders = repeat_vars(request_ids.len());
        conn.execute(
            &format!(
                "DELETE FROM image_generation_request_logs WHERE request_id IN ({placeholders})"
            ),
            params_from_iter(request_ids.iter()),
        )
        .map_err(|e| AppError::Internal(format!("delete stale image request logs failed: {e}")))?;
        Ok(stale
            .into_iter()
            .filter_map(|(_, storage_key)| storage_key)
            .collect())
    }

    pub async fn list_user_shares(
        &self,
        config: &Config,
        current_user_email: &str,
        active_subdomains: &HashSet<String>,
        inflight_by_share: &HashMap<String, usize>,
    ) -> Result<UserSharesResponse, AppError> {
        let email = normalize_email(current_user_email)?;
        let conn = self.conn.lock().await;
        let active_edits = list_active_share_edits(&conn)?;
        let mut shares = Vec::new();
        for (_, share) in list_shares(&conn)? {
            let owner = share
                .owner_email
                .as_deref()
                .is_some_and(|owner| owner.eq_ignore_ascii_case(&email));
            let shared = share
                .shared_with_emails
                .iter()
                .any(|shared| shared.eq_ignore_ascii_case(&email));
            if !owner && !shared {
                continue;
            }
            let role = if owner { "owner" } else { "shared" }.to_string();
            let active_requests = inflight_by_share
                .get(&share.share_id)
                .copied()
                .unwrap_or_default();
            shares.push(UserShareView {
                router_id: "main".to_string(),
                share_id: share.share_id.clone(),
                share_name: share.share_name.clone(),
                owner_email: share.owner_email.clone(),
                shared_with_emails: share.shared_with_emails.clone(),
                role,
                can_invoke: true,
                can_manage: owner,
                description: share.description.clone(),
                for_sale: share.for_sale.clone(),
                sale_market_kind: share.sale_market_kind.clone(),
                market_access_mode: share.market_access_mode.clone(),
                subdomain: share.subdomain.clone(),
                tunnel_url: config.tunnel_url(&share.subdomain),
                app_type: share.app_type.clone(),
                provider_id: share.provider_id.clone(),
                token_limit: share.token_limit,
                parallel_limit: share.parallel_limit,
                tokens_used: share.tokens_used,
                requests_count: share.requests_count,
                share_status: share.share_status.clone(),
                created_at: share.created_at.clone(),
                expires_at: share.expires_at.clone(),
                is_online: active_subdomains.contains(&share.subdomain),
                active_requests,
                active_edit: active_edits.get(&share.share_id).cloned(),
            });
        }
        Ok(UserSharesResponse { shares })
    }

    pub async fn share_view_for_share_url(
        &self,
        share_id: &str,
        active_subdomains: &HashSet<String>,
        inflight_by_share: &HashMap<String, usize>,
        viewer_email: Option<&str>,
    ) -> Result<ShareView, AppError> {
        let conn = self.conn.lock().await;
        let (_, share) = list_shares(&conn)?
            .into_iter()
            .find(|(_, share)| share.share_id == share_id)
            .ok_or_else(|| AppError::NotFound("share not found".into()))?;
        let active_edit = get_active_share_edit(&conn, share_id)?;
        let can_view_share = share_visible_to_email(&share, viewer_email);
        let can_manage = can_manage_share(&share, viewer_email);
        let can_edit_settings = can_manage
            && active_edit
                .as_ref()
                .map(|edit| edit.status != "pending")
                .unwrap_or(true);
        let is_online =
            share.share_status == "active" && active_subdomains.contains(&share.subdomain);
        let active_requests = inflight_by_share.get(&share.share_id).copied().unwrap_or(0);
        let mut view = ShareView {
            router_id: "main".to_string(),
            share_id: share.share_id,
            share_name: share.share_name,
            owner_email: share.owner_email,
            shared_with_emails: if can_view_share {
                share.shared_with_emails
            } else {
                Vec::new()
            },
            access_by_app: if can_view_share {
                share.access_by_app
            } else {
                BTreeMap::new()
            },
            app_settings: if can_view_share {
                share.app_settings
            } else {
                BTreeMap::new()
            },
            market_links: Vec::new(),
            unknown_market_emails: Vec::new(),
            description: share.description,
            for_sale: share.for_sale,
            sale_market_kind: share.sale_market_kind,
            market_access_mode: share.market_access_mode,
            for_sale_official_price_percent_by_app: share.for_sale_official_price_percent_by_app,
            subdomain: share.subdomain,
            can_view_secret: false,
            can_manage,
            can_edit_settings,
            active_edit,
            app_type: share.app_type,
            provider_id: share.provider_id,
            bindings: share.bindings,
            token_limit: share.token_limit,
            parallel_limit: share.parallel_limit,
            tokens_used: share.tokens_used,
            requests_count: share.requests_count,
            share_status: share.share_status,
            created_at: share.created_at,
            expires_at: share.expires_at,
            support: share.support,
            upstream_provider: None,
            app_runtimes: share.app_runtimes,
            app_providers: share.app_providers,
            installation_id: String::new(),
            is_online,
            cleanup_at: None,
            active_requests,
            active_requests_by_app: BTreeMap::new(),
            tokens_used_by_app: BTreeMap::new(),
            requests_count_by_app: BTreeMap::new(),
            online_minutes_24h: 0,
            online_rate_24h: 0.0,
            recent_requests: Vec::new(),
            health_checks: Vec::new(),
            health_timeline: Vec::new(),
            recent_model_health_checks: Vec::new(),
            model_health: ShareModelHealthSummary::default(),
            operational_summary: OperationalSummary::healthy("online"),
        };
        view.operational_summary = share_operational_summary(&view, Utc::now());
        Ok(view)
    }

    pub async fn user_can_invoke_share(
        &self,
        user_email: &str,
        share_id: &str,
        app_type: Option<&str>,
    ) -> Result<bool, AppError> {
        let email = normalize_email(user_email)?;
        let conn = self.conn.lock().await;
        let Some((owner_email, shared_with_emails_json, access_by_app_json, for_sale)): Option<(
            Option<String>,
            String,
            String,
            String,
        )> = conn
            .query_row(
                "SELECT owner_email, shared_with_emails_json, COALESCE(access_by_app_json, '{}'), for_sale FROM shares WHERE share_id = ?1",
                params![share_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query share invoke acl failed: {e}")))?
        else {
            return Ok(false);
        };
        if for_sale == "Free" {
            return Ok(true);
        }
        if owner_email
            .as_deref()
            .is_some_and(|owner| owner.eq_ignore_ascii_case(&email))
        {
            return Ok(true);
        }
        let access_by_app: BTreeMap<String, ShareAppAccess> =
            serde_json::from_str(&access_by_app_json).unwrap_or_default();
        if let Some(app_type) = app_type.map(|value| value.trim().to_ascii_lowercase()) {
            if !access_by_app.is_empty() {
                return Ok(access_by_app.get(&app_type).is_some_and(|access| {
                    access
                        .shared_with_emails
                        .iter()
                        .any(|shared| shared.eq_ignore_ascii_case(&email))
                }));
            }
        }
        let shared_with_emails = parse_string_vec(Some(shared_with_emails_json))
            .map_err(|e| AppError::Internal(format!("parse share invoke acl failed: {e}")))?;
        Ok(shared_with_emails
            .iter()
            .any(|shared| shared.eq_ignore_ascii_case(&email)))
    }

    pub async fn share_usage_by_email(
        &self,
        share_id: &str,
        app_type: &str,
        period: &str,
    ) -> Result<ShareUsageByEmailResponse, AppError> {
        let app = normalize_share_acl_app(app_type)?;
        let window = normalize_usage_period(period)?;
        let period = window.period.clone();
        let bucket_granularity = window.bucket_granularity.clone();
        let days = window.days;
        let conn = self.conn.lock().await;
        let Some((
            owner_email,
            shared_with_emails_json,
            access_by_app_json,
            market_access_mode,
        )): Option<(Option<String>, String, String, String)> = conn
            .query_row(
                "SELECT owner_email, shared_with_emails_json, COALESCE(access_by_app_json, '{}'), market_access_mode
                 FROM shares
                 WHERE share_id = ?1",
                params![share_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query share usage acl failed: {e}")))?
        else {
            return Err(AppError::NotFound("share not found".into()));
        };

        let shared_with_emails = parse_string_vec(Some(shared_with_emails_json))
            .map_err(|e| AppError::Internal(format!("parse share usage acl failed: {e}")))?;
        let access_by_app = parse_share_access_by_app(Some(access_by_app_json)).map_err(|e| {
            AppError::Internal(format!("parse share usage access_by_app failed: {e}"))
        })?;

        let mut roles = BTreeMap::<String, String>::new();
        if let Some(owner) = owner_email.as_deref().and_then(normalize_usage_email) {
            roles.insert(owner, "owner".to_string());
        }
        let (shareto_emails, app_market_access_mode) = if access_by_app.is_empty() {
            (shared_with_emails, market_access_mode.clone())
        } else {
            let access = access_by_app.get(&app);
            (
                access
                    .map(|access| access.shared_with_emails.clone())
                    .unwrap_or_default(),
                access
                    .map(|access| access.market_access_mode.clone())
                    .unwrap_or_else(|| "selected".to_string()),
            )
        };
        let include_actual_usage_emails = app_market_access_mode.eq_ignore_ascii_case("all");
        for email in shareto_emails {
            if let Some(email) = normalize_usage_email(&email) {
                roles.entry(email).or_insert_with(|| "shareto".to_string());
            }
        }

        let start_dt = window.start_at;
        let start_ts = start_dt.timestamp();
        let start_rfc3339 = start_dt.to_rfc3339();
        let bucket_keys = window.bucket_keys;
        let make_usage_row = |email: &str, role: &str| ShareUsageEmailRow {
            email: email.to_string(),
            role: role.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            total_tokens: 0,
            percent: 0.0,
            daily: bucket_keys
                .iter()
                .map(|bucket| ShareUsageDailyBucket {
                    date: bucket.clone(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    total_tokens: 0,
                })
                .collect(),
        };
        let mut rows_by_email = roles
            .iter()
            .map(|(email, role)| (email.clone(), make_usage_row(email, role)))
            .collect::<BTreeMap<_, _>>();
        let bucket_index = bucket_keys
            .iter()
            .enumerate()
            .map(|(idx, bucket)| (bucket.clone(), idx))
            .collect::<BTreeMap<_, _>>();

        let market_bucket_expr = if bucket_granularity == "hour" {
            "strftime('%Y-%m-%dT%H:00:00Z', ml.created_at)"
        } else {
            "date(ml.created_at)"
        };
        let market_input_expr = market_log_input_tokens_expr("ml");
        let market_total_expr = market_log_total_tokens_expr("ml");
        let market_usage_sql = format!(
            "SELECT lower(trim(ml.user_email)),
                    {market_bucket_expr} AS usage_bucket,
                    COALESCE(SUM(CASE WHEN ({market_total_expr}) > 0 OR sl.request_id IS NULL THEN {market_input_expr} ELSE COALESCE(sl.input_tokens, 0) END), 0),
                    COALESCE(SUM(CASE WHEN ({market_total_expr}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.output_tokens, 0) ELSE COALESCE(sl.output_tokens, 0) END), 0),
                    COALESCE(SUM(CASE WHEN ({market_total_expr}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_read_tokens, 0) ELSE COALESCE(sl.cache_read_tokens, 0) END), 0),
                    COALESCE(SUM(CASE WHEN ({market_total_expr}) > 0 OR sl.request_id IS NULL THEN COALESCE(ml.cache_creation_tokens, 0) ELSE COALESCE(sl.cache_creation_tokens, 0) END), 0)
             FROM market_request_logs ml
             LEFT JOIN share_request_logs sl
               ON sl.request_id = ml.request_id
              AND sl.share_id = ml.share_id
              AND sl.is_health_check = 0
              AND lower(CASE WHEN COALESCE(sl.request_agent, '') != '' THEN sl.request_agent ELSE sl.app_type END) = lower(ml.request_agent)
             WHERE ml.share_id = ?1
               AND lower(ml.request_agent) = lower(?2)
               AND ml.created_at >= ?3
               AND ml.user_email IS NOT NULL
               AND trim(ml.user_email) != ''
             GROUP BY lower(trim(ml.user_email)), usage_bucket"
        );
        let mut market_stmt = conn.prepare(&market_usage_sql).map_err(|e| {
            AppError::Internal(format!("prepare market share usage query failed: {e}"))
        })?;
        let market_rows = market_stmt
            .query_map(params![share_id, app, start_rfc3339], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?.max(0) as u64,
                    row.get::<_, i64>(3)?.max(0) as u64,
                    row.get::<_, i64>(4)?.max(0) as u64,
                    row.get::<_, i64>(5)?.max(0) as u64,
                ))
            })
            .map_err(|e| AppError::Internal(format!("query market share usage failed: {e}")))?;
        for row in market_rows {
            let (email, bucket, input, output, cache_read, cache_creation) = row.map_err(|e| {
                AppError::Internal(format!("read market share usage row failed: {e}"))
            })?;
            if include_actual_usage_emails {
                rows_by_email
                    .entry(email.clone())
                    .or_insert_with(|| make_usage_row(&email, "market"));
            }
            let Some(row) = rows_by_email.get_mut(&email) else {
                continue;
            };
            let total = input + output + cache_read + cache_creation;
            row.input_tokens += input;
            row.output_tokens += output;
            row.cache_read_tokens += cache_read;
            row.cache_creation_tokens += cache_creation;
            row.total_tokens += total;
            if let Some(idx) = bucket_index.get(&bucket).copied() {
                if let Some(bucket) = row.daily.get_mut(idx) {
                    bucket.input_tokens += input;
                    bucket.output_tokens += output;
                    bucket.cache_read_tokens += cache_read;
                    bucket.cache_creation_tokens += cache_creation;
                    bucket.total_tokens += total;
                }
            }
        }

        let share_bucket_expr = if bucket_granularity == "hour" {
            "strftime('%Y-%m-%dT%H:00:00Z', created_at, 'unixepoch')"
        } else {
            "date(created_at, 'unixepoch')"
        };
        let share_usage_sql = format!(
            "SELECT lower(trim(user_email)),
                    {share_bucket_expr} AS usage_bucket,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0)
             FROM share_request_logs
             WHERE share_id = ?1
               AND lower(app_type) = lower(?2)
               AND created_at >= ?3
               AND is_health_check = 0
               AND user_email IS NOT NULL
               AND trim(user_email) != ''
               AND NOT EXISTS (
                    SELECT 1
                    FROM market_request_logs ml
                    WHERE ml.request_id = share_request_logs.request_id
                      AND COALESCE(ml.share_id, '') = share_request_logs.share_id
                      AND ml.user_email IS NOT NULL
                      AND trim(ml.user_email) != ''
               )
             GROUP BY lower(trim(user_email)), usage_bucket"
        );
        let mut stmt = conn
            .prepare(&share_usage_sql)
            .map_err(|e| AppError::Internal(format!("prepare share usage query failed: {e}")))?;
        let rows = stmt
            .query_map(params![share_id, app, start_ts], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?.max(0) as u64,
                    row.get::<_, i64>(3)?.max(0) as u64,
                    row.get::<_, i64>(4)?.max(0) as u64,
                    row.get::<_, i64>(5)?.max(0) as u64,
                ))
            })
            .map_err(|e| AppError::Internal(format!("query share usage failed: {e}")))?;
        for row in rows {
            let (email, bucket, input, output, cache_read, cache_creation) =
                row.map_err(|e| AppError::Internal(format!("read share usage row failed: {e}")))?;
            if include_actual_usage_emails {
                rows_by_email
                    .entry(email.clone())
                    .or_insert_with(|| make_usage_row(&email, "market"));
            }
            let Some(row) = rows_by_email.get_mut(&email) else {
                continue;
            };
            let total = input + output + cache_read + cache_creation;
            row.input_tokens += input;
            row.output_tokens += output;
            row.cache_read_tokens += cache_read;
            row.cache_creation_tokens += cache_creation;
            row.total_tokens += total;
            if let Some(idx) = bucket_index.get(&bucket).copied() {
                if let Some(bucket) = row.daily.get_mut(idx) {
                    bucket.input_tokens += input;
                    bucket.output_tokens += output;
                    bucket.cache_read_tokens += cache_read;
                    bucket.cache_creation_tokens += cache_creation;
                    bucket.total_tokens += total;
                }
            }
        }

        let total_tokens = rows_by_email
            .values()
            .map(|row| row.total_tokens)
            .sum::<u64>();
        let mut rows = rows_by_email.into_values().collect::<Vec<_>>();
        for row in &mut rows {
            row.percent = if total_tokens > 0 {
                (row.total_tokens as f64 / total_tokens as f64) * 100.0
            } else {
                0.0
            };
        }
        rows.sort_by(|a, b| {
            b.total_tokens
                .cmp(&a.total_tokens)
                .then_with(|| a.role.cmp(&b.role))
                .then_with(|| a.email.cmp(&b.email))
        });

        Ok(ShareUsageByEmailResponse {
            share_id: share_id.to_string(),
            app,
            period,
            bucket_granularity,
            days,
            total_tokens,
            rows,
        })
    }

    pub async fn issue_lease(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
        input: IssueLeaseRequest,
        metadata: ClientMetadata,
        _current_user_email: Option<&str>,
    ) -> Result<IssueLeaseResponse, AppError> {
        let now = Utc::now();
        let tunnel_type = input.tunnel_type.to_ascii_lowercase();
        let is_client_web_tunnel = tunnel_type == "client-web-http";
        if tunnel_type != "http" && !is_client_web_tunnel {
            return Err(AppError::BadRequest(
                "only http tunnels are supported".into(),
            ));
        }
        if is_client_web_tunnel && input.share.is_some() {
            return Err(AppError::BadRequest(
                "client web tunnels cannot include share metadata".into(),
            ));
        }

        let requested_subdomain = normalize_subdomain(&input.requested_subdomain)?;
        ensure_subdomain_allowed(&requested_subdomain, config)?;
        let installation = {
            let conn = self.conn.lock().await;
            let installation = get_installation(&conn, &input.installation_id)?
                .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
            verify_issue_lease_request(&conn, &installation.public_key, &input, now)?;
            let should_refresh_geo =
                should_refresh_installation_geo(&installation, metadata.ip.as_deref());
            touch_installation_presence(&conn, &input.installation_id, &metadata, now)?;
            (installation, should_refresh_geo)
        };
        if installation.1 {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }
        let installation = installation.0;

        let subdomain = if let Some(share) = input.share.as_ref() {
            let conn = self.conn.lock().await;
            ensure_subdomain_not_registered_market(&conn, &requested_subdomain)?;
            if get_client_tunnel_by_subdomain(&conn, &requested_subdomain)?.is_some() {
                return Err(AppError::Conflict(
                    "subdomain already claimed by client tunnel".into(),
                ));
            }
            let owned_subdomain =
                get_share_owned_subdomain(&conn, &input.installation_id, &share.share_id)?
                    .ok_or_else(|| AppError::Conflict("share subdomain is not claimed".into()))?;
            if owned_subdomain != requested_subdomain {
                return Err(AppError::Conflict(
                    "requested subdomain does not match claimed subdomain".into(),
                ));
            }
            owned_subdomain
        } else if is_client_web_tunnel {
            let conn = self.conn.lock().await;
            ensure_subdomain_not_registered_market(&conn, &requested_subdomain)?;
            ensure_subdomain_not_claimed_by_share(&conn, &requested_subdomain)?;
            let tunnel = get_client_tunnel_by_installation(&conn, &input.installation_id)?
                .ok_or_else(|| AppError::Conflict("client tunnel is not claimed".into()))?;
            if !tunnel.enabled {
                return Err(AppError::Conflict("client tunnel is disabled".into()));
            }
            if tunnel.subdomain != requested_subdomain {
                return Err(AppError::Conflict(
                    "requested subdomain does not match claimed client tunnel".into(),
                ));
            }
            requested_subdomain
        } else {
            let conn = self.conn.lock().await;
            ensure_subdomain_not_registered_market(&conn, &requested_subdomain)?;
            if get_client_tunnel_by_subdomain(&conn, &requested_subdomain)?.is_some() {
                return Err(AppError::Conflict(
                    "subdomain already claimed by client tunnel".into(),
                ));
            }
            requested_subdomain
        };
        let requested_share_id = input.share.as_ref().map(|share| share.share_id.as_str());
        {
            let conn = self.conn.lock().await;
            conn.execute(
                "DELETE FROM leases
                 WHERE subdomain = ?1
                   AND installation_id = ?2
                   AND tunnel_type = ?3",
                params![subdomain, input.installation_id, tunnel_type],
            )
            .map_err(|e| AppError::Internal(format!("delete stale client leases failed: {e}")))?;
            let live_lease_exists: bool = conn
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM leases
                        WHERE subdomain = ?1 AND expires_at > ?2
                    )",
                    params![subdomain, now.to_rfc3339()],
                    |row| row.get(0),
                )
                .map_err(|e| AppError::Internal(format!("check live lease failed: {e}")))?;
            if live_lease_exists {
                return Err(AppError::Conflict("subdomain already leased".into()));
            }
        }
        if let Some(route) = proxy
            .backend_for_host(
                &format!("{subdomain}.{}", config.tunnel_domain),
                &config.tunnel_domain,
            )
            .await
        {
            let is_same_share_route =
                requested_share_id.is_some() && route.share_id() == requested_share_id;
            let is_same_client_web_route = is_client_web_tunnel
                && route.is_client_web()
                && route.installation_id() == Some(input.installation_id.as_str());
            if !is_same_share_route {
                if is_same_client_web_route {
                    proxy.remove_route(&subdomain).await;
                } else {
                    return Err(AppError::Conflict("subdomain already in use".into()));
                }
            }
        }

        let normalized_share = if let Some(mut share) = input.share.clone() {
            {
                let conn = self.conn.lock().await;
                ensure_share_id_writable_by_installation(
                    &conn,
                    &share.share_id,
                    &input.installation_id,
                )?;
            }
            let installation_owner = installation
                .owner_email
                .as_deref()
                .ok_or_else(|| {
                    AppError::Conflict("installation owner email is not configured".into())
                })
                .and_then(normalize_email)?;
            normalize_self_reported_share_owner(&mut share, &installation_owner)?;
            Some(share)
        } else {
            None
        };

        if let Some(share) = normalized_share.clone() {
            self.upsert_share(&input.installation_id, share).await?;
        }
        if is_client_web_tunnel {
            let conn = self.conn.lock().await;
            conn.execute(
                "UPDATE installation_client_tunnels
                 SET last_seen_at = ?2, updated_at = ?2
                 WHERE installation_id = ?1",
                params![input.installation_id, Utc::now().to_rfc3339()],
            )
            .map_err(|e| AppError::Internal(format!("touch client tunnel failed: {e}")))?;
        }

        let issued_at = Utc::now();
        let expires_at = issued_at + Duration::seconds(config.lease_ttl_secs);
        let connection_id = Uuid::new_v4().to_string();
        let ssh_password = Alphanumeric.sample_string(&mut rand::thread_rng(), 24);
        let lease = TunnelLease {
            id: Uuid::new_v4().to_string(),
            installation_id: installation.id.clone(),
            connection_id: connection_id.clone(),
            subdomain: subdomain.clone(),
            tunnel_type,
            ssh_username: connection_id.clone(),
            ssh_password: ssh_password.clone(),
            issued_at,
            expires_at,
            used_at: None,
            share: normalized_share,
        };

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO leases (
                id, installation_id, connection_id, subdomain, tunnel_type,
                ssh_username, ssh_password, issued_at, expires_at, used_at, share_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                lease.id,
                lease.installation_id,
                lease.connection_id,
                lease.subdomain,
                lease.tunnel_type,
                lease.ssh_username,
                lease.ssh_password,
                lease.issued_at.to_rfc3339(),
                lease.expires_at.to_rfc3339(),
                Option::<String>::None,
                lease
                    .share
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| AppError::Internal(format!("serialize share failed: {e}")))?,
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert lease failed: {e}")))?;

        proxy
            .mark_route_pending(
                subdomain.clone(),
                StdDuration::from_secs(ROUTE_REGISTRATION_PENDING_GRACE_SECS),
            )
            .await;

        Ok(IssueLeaseResponse {
            lease_id: lease.id,
            connection_id: lease.connection_id,
            ssh_username: lease.ssh_username,
            ssh_password,
            ssh_addr: config.effective_ssh_public_addr(),
            expires_at,
            tunnel_url: config.tunnel_url(&subdomain),
            subdomain,
            ssh_host_fingerprint: None,
        })
    }

    pub async fn issue_market_lease(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
        market: &MarketRegistryRecord,
    ) -> Result<IssueLeaseResponse, AppError> {
        let now = Utc::now();
        let subdomain = normalize_subdomain(&market.subdomain)?;
        let market_installation_id = format!("market:{}", market.id);
        {
            let conn = self.conn.lock().await;
            let registered = get_market_by_email(&conn, &market.email)?
                .filter(|stored| stored.status.eq_ignore_ascii_case("active"))
                .filter(|stored| stored.subdomain == subdomain)
                .is_some();
            if !registered {
                return Err(AppError::Unauthorized(
                    "market subdomain is not registered".into(),
                ));
            }
            conn.execute(
                "DELETE FROM leases
                 WHERE subdomain = ?1
                   AND installation_id = ?2
                   AND tunnel_type = 'market-http'",
                params![subdomain, market_installation_id],
            )
            .map_err(|e| AppError::Internal(format!("delete stale market leases failed: {e}")))?;
            let live_lease_exists: bool = conn
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM leases
                        WHERE subdomain = ?1 AND expires_at > ?2
                    )",
                    params![subdomain, now.to_rfc3339()],
                    |row| row.get(0),
                )
                .map_err(|e| AppError::Internal(format!("check live market lease failed: {e}")))?;
            if live_lease_exists {
                return Err(AppError::Conflict("market subdomain already leased".into()));
            }
        }
        if proxy
            .backend_for_host(
                &format!("{subdomain}.{}", config.tunnel_domain),
                &config.tunnel_domain,
            )
            .await
            .is_some()
        {
            proxy.remove_route(&subdomain).await;
            tracing::warn!(
                subdomain = %subdomain,
                market_email = %market.email,
                "removed stale market route before issuing replacement lease"
            );
        }

        let issued_at = Utc::now();
        let expires_at = issued_at + Duration::seconds(config.lease_ttl_secs);
        let connection_id = Uuid::new_v4().to_string();
        let ssh_password = Alphanumeric.sample_string(&mut rand::thread_rng(), 24);
        let lease = TunnelLease {
            id: Uuid::new_v4().to_string(),
            installation_id: format!("market:{}", market.id),
            connection_id: connection_id.clone(),
            subdomain: subdomain.clone(),
            tunnel_type: "market-http".to_string(),
            ssh_username: connection_id.clone(),
            ssh_password: ssh_password.clone(),
            issued_at,
            expires_at,
            used_at: None,
            share: None,
        };

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO leases (
                id, installation_id, connection_id, subdomain, tunnel_type,
                ssh_username, ssh_password, issued_at, expires_at, used_at, share_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                lease.id,
                lease.installation_id,
                lease.connection_id,
                lease.subdomain,
                lease.tunnel_type,
                lease.ssh_username,
                lease.ssh_password,
                lease.issued_at.to_rfc3339(),
                lease.expires_at.to_rfc3339(),
                Option::<String>::None,
                Option::<String>::None,
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert market lease failed: {e}")))?;
        conn.execute(
            "UPDATE router_markets
             SET status = 'active', last_seen_at = ?2, updated_at = ?2, offline_since = NULL
             WHERE email = ?1",
            params![market.email, issued_at.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("touch market lease presence failed: {e}")))?;

        Ok(IssueLeaseResponse {
            lease_id: lease.id,
            connection_id: lease.connection_id,
            ssh_username: lease.ssh_username,
            ssh_password,
            ssh_addr: config.effective_ssh_public_addr(),
            expires_at,
            tunnel_url: config.tunnel_url(&subdomain),
            subdomain,
            ssh_host_fingerprint: None,
        })
    }

    pub async fn consume_lease(
        &self,
        username: &str,
        password: &str,
    ) -> Result<TunnelLease, AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let mut lease = get_lease_by_connection_id(&conn, username)?
            .ok_or_else(|| AppError::Unauthorized("lease not found".into()))?;
        if lease.expires_at < now {
            return Err(AppError::Unauthorized("lease expired".into()));
        }
        if lease.used_at.is_some() {
            return Err(AppError::Unauthorized("lease already used".into()));
        }
        if lease.ssh_password != password {
            return Err(AppError::Unauthorized("invalid ssh credentials".into()));
        }
        lease.used_at = Some(now);
        conn.execute(
            "UPDATE leases SET used_at = ?2 WHERE connection_id = ?1",
            params![username, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("update lease use failed: {e}")))?;
        Ok(lease)
    }

    pub async fn renew_lease(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
        input: RenewLeaseRequest,
        metadata: ClientMetadata,
    ) -> Result<RenewLeaseResponse, AppError> {
        let now = Utc::now();
        let (subdomain, tunnel_type) = {
            let conn = self.conn.lock().await;
            let installation = get_installation(&conn, &input.installation_id)?
                .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
            verify_signed_share_request(
                &conn,
                &installation.public_key,
                &input.installation_id,
                "renew_lease",
                &input.renewal,
                input.timestamp_ms,
                &input.nonce,
                &input.signature,
            )?;
            let lease = conn
                .query_row(
                    "SELECT subdomain, tunnel_type, used_at
                     FROM leases
                     WHERE id = ?1 AND installation_id = ?2 AND connection_id = ?3",
                    params![
                        input.renewal.lease_id,
                        input.installation_id,
                        input.renewal.connection_id
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!("query tunnel lease for renewal failed: {error}"))
                })?
                .ok_or_else(|| AppError::Conflict("tunnel lease is no longer renewable".into()))?;
            if lease.2.is_none() {
                return Err(AppError::Conflict(
                    "tunnel lease has not established an SSH session".into(),
                ));
            }
            touch_installation_presence(&conn, &input.installation_id, &metadata, now)?;
            (lease.0, lease.1)
        };

        let route = proxy
            .backend_for_host(
                &format!("{subdomain}.{}", config.tunnel_domain),
                &config.tunnel_domain,
            )
            .await
            .ok_or_else(|| AppError::Conflict("tunnel route is not active".into()))?;
        if route.connection_id() != Some(input.renewal.connection_id.as_str())
            || route.installation_id() != Some(input.installation_id.as_str())
        {
            return Err(AppError::Conflict(
                "tunnel route belongs to a different connection".into(),
            ));
        }

        let expires_at = Utc::now() + Duration::seconds(config.lease_ttl_secs);
        let conn = self.conn.lock().await;
        let updated = conn
            .execute(
                "UPDATE leases
                 SET expires_at = ?4
                 WHERE id = ?1 AND installation_id = ?2 AND connection_id = ?3",
                params![
                    input.renewal.lease_id,
                    input.installation_id,
                    input.renewal.connection_id,
                    expires_at.to_rfc3339()
                ],
            )
            .map_err(|error| AppError::Internal(format!("renew tunnel lease failed: {error}")))?;
        if updated != 1 {
            return Err(AppError::Conflict(
                "tunnel lease changed during renewal".into(),
            ));
        }
        if tunnel_type == "client-web-http" {
            conn.execute(
                "UPDATE installation_client_tunnels
                 SET last_seen_at = ?2, updated_at = ?2
                 WHERE installation_id = ?1",
                params![input.installation_id, Utc::now().to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("touch renewed client tunnel failed: {error}"))
            })?;
        }
        Ok(RenewLeaseResponse { expires_at })
    }

    pub async fn sync_share(
        &self,
        input: ShareSyncRequest,
        metadata: ClientMetadata,
        _current_user_email: &str,
    ) -> Result<(), AppError> {
        let installation_owner = {
            let conn = self.conn.lock().await;
            let installation = get_installation(&conn, &input.installation_id)?;
            let Some(installation) = installation else {
                return Err(AppError::Unauthorized("installation not found".into()));
            };
            let owner = installation
                .owner_email
                .as_deref()
                .ok_or_else(|| {
                    AppError::Conflict("installation owner email is not configured".into())
                })
                .and_then(normalize_email)?;
            verify_signed_share_request(
                &conn,
                &installation.public_key,
                &input.installation_id,
                "share_sync",
                &input.share,
                input.timestamp_ms,
                &input.nonce,
                &input.signature,
            )?;
            let should_refresh_geo =
                should_refresh_installation_geo(&installation, metadata.ip.as_deref());
            touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
            drop(conn);
            if should_refresh_geo {
                self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                    .await?;
            }
            owner
        };
        let mut share = input.share;
        {
            let conn = self.conn.lock().await;
            ensure_share_id_writable_by_installation(
                &conn,
                &share.share_id,
                &input.installation_id,
            )?;
        }
        normalize_self_reported_share_owner(&mut share, &installation_owner)?;
        self.upsert_share(&input.installation_id, share).await
    }

    pub async fn claim_share_subdomain(
        &self,
        config: &Config,
        input: ShareClaimSubdomainRequest,
        metadata: ClientMetadata,
        _current_user_email: &str,
    ) -> Result<(), AppError> {
        let subdomain = normalize_subdomain(&input.share.subdomain)?;
        ensure_subdomain_allowed(&subdomain, config)?;
        let conn = self.conn.lock().await;
        ensure_subdomain_not_registered_market(&conn, &subdomain)?;
        if get_client_tunnel_by_subdomain(&conn, &subdomain)?.is_some() {
            return Err(AppError::Conflict(
                "subdomain already claimed by client tunnel".into(),
            ));
        }
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        let installation_owner = installation
            .owner_email
            .as_deref()
            .ok_or_else(|| AppError::Conflict("installation owner email is not configured".into()))
            .and_then(normalize_email)?;
        verify_share_claim_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            &input.share,
            input.claim.as_ref(),
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        drop(conn);
        if should_refresh_geo {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }

        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Internal(format!("begin share claim tx failed: {e}")))?;
        let mut share = input.share;
        ensure_share_id_writable_by_installation(&tx, &share.share_id, &input.installation_id)?;
        normalize_self_reported_share_owner(&mut share, &installation_owner)?;
        share.subdomain = subdomain;
        release_reclaimable_subdomain_claim(
            &tx,
            &input.installation_id,
            &share.share_id,
            share.owner_email.as_deref(),
            &share.subdomain,
        )?;
        upsert_share_tx(&tx, &input.installation_id, share)?;
        tx.commit().map_err(map_share_constraint_error)?;
        Ok(())
    }

    pub async fn delete_share(
        &self,
        input: ShareDeleteRequest,
        _current_user_email: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        let delete_payload = serde_json::json!({ "shareId": &input.share_id });
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_delete",
            &delete_payload,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        conn.execute(
            "DELETE FROM shares WHERE share_id = ?1 AND installation_id = ?2",
            params![input.share_id, input.installation_id],
        )
        .map_err(|e| AppError::Internal(format!("delete share failed: {e}")))?;
        Ok(())
    }

    pub async fn batch_sync_shares(
        &self,
        input: ShareBatchSyncRequest,
        metadata: ClientMetadata,
        _current_user_email: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        let installation_owner = installation
            .owner_email
            .as_deref()
            .ok_or_else(|| AppError::Conflict("installation owner email is not configured".into()))
            .and_then(normalize_email)?;
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_batch_sync",
            &input.ops,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        drop(conn);
        if should_refresh_geo {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }

        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Internal(format!("begin batch sync tx failed: {e}")))?;
        for op in input.ops {
            match op.kind.as_str() {
                "upsert" => {
                    let mut share = op.share.ok_or_else(|| {
                        AppError::BadRequest("share is required for upsert".into())
                    })?;
                    ensure_share_id_writable_by_installation(
                        &tx,
                        &share.share_id,
                        &input.installation_id,
                    )?;
                    normalize_self_reported_share_owner(&mut share, &installation_owner)?;
                    upsert_share_tx(&tx, &input.installation_id, share)?;
                }
                "delete" => {
                    let share_id = op.share_id.ok_or_else(|| {
                        AppError::BadRequest("shareId is required for delete".into())
                    })?;
                    tx.execute(
                        "DELETE FROM shares WHERE share_id = ?1 AND installation_id = ?2",
                        params![share_id, input.installation_id],
                    )
                    .map_err(|e| {
                        AppError::Internal(format!("delete share in batch failed: {e}"))
                    })?;
                }
                "delete_all" => {
                    delete_all_shares_for_installation_tx(&tx, &input.installation_id)?;
                }
                other => {
                    return Err(AppError::BadRequest(format!(
                        "unsupported share batch op: {other}"
                    )));
                }
            }
        }
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit batch sync failed: {e}")))?;
        Ok(())
    }

    pub async fn create_share_settings_edit(
        &self,
        share_id: &str,
        current_user_email: &str,
        patch: ShareSettingsPatch,
    ) -> Result<ShareSettingsUpdateResponse, AppError> {
        let current_user_email = normalize_email(current_user_email)?;
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let (installation_id, owner_email, shared_with_emails_json, current_sale_market_kind): (
            String,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT installation_id, owner_email, shared_with_emails_json, sale_market_kind FROM shares WHERE share_id = ?1",
                params![share_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query share owner failed: {e}")))?
            .ok_or_else(|| AppError::NotFound("share not found".into()))?;
        let owner_email = normalize_email(&owner_email)?;
        let shared_with_emails = parse_string_vec(Some(shared_with_emails_json))
            .map_err(|e| AppError::Internal(format!("parse share acl failed: {e}")))?;
        let patch = normalize_share_settings_patch(
            patch,
            Some(&owner_email),
            Some(&shared_with_emails),
            Some(&current_sale_market_kind),
        )?;
        if share_settings_patch_is_empty(&patch) {
            return Err(AppError::BadRequest("share settings patch is empty".into()));
        }
        if owner_email != current_user_email {
            return Err(AppError::Forbidden(
                "only share owner can edit share settings".into(),
            ));
        }
        if let Some(active) = get_active_share_edit(&conn, share_id)? {
            return Err(AppError::Conflict(format!(
                "share settings edit {} is {} and must be applied before another edit",
                active.revision, active.status
            )));
        }
        let revision = next_share_edit_revision(&conn, share_id)?;
        let id = Uuid::new_v4().to_string();
        let patch_json = serde_json::to_string(&patch).map_err(|e| {
            AppError::Internal(format!("serialize share settings patch failed: {e}"))
        })?;
        conn.execute(
            "INSERT INTO share_edit_requests (
                id, share_id, installation_id, owner_email, revision, status, patch_json,
                created_by_email, created_at, updated_at, applied_at, error_message
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?8, NULL, NULL)",
            params![
                id,
                share_id,
                installation_id,
                owner_email,
                revision,
                patch_json,
                current_user_email,
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert share settings edit failed: {e}")))?;
        let edit = get_share_edit_by_id(&conn, &id)?
            .ok_or_else(|| AppError::Internal("created share edit is missing".into()))?;
        Ok(ShareSettingsUpdateResponse {
            ok: true,
            edit,
            applied_synchronously: false,
        })
    }

    /// Applies a share-settings edit using the descriptor the client returned
    /// from its `/_ctl/apply_share_settings` call. The client remains
    /// authoritative — the server only writes what the client reported and
    /// only after verifying that report actually satisfies every field the
    /// pending edit requested. A report that drops a requested field (e.g. an
    /// owner transfer that forgot to demote the old owner into `shareto`) is
    /// rejected, the edit is marked `rejected`, and `shares` is left untouched.
    pub async fn apply_share_edit_directly(
        &self,
        edit_id: &str,
        mut returned_share: ShareDescriptor,
    ) -> Result<(), AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let edit = get_share_edit_by_id(&conn, edit_id)?
            .ok_or_else(|| AppError::NotFound("share edit not found".into()))?;
        if edit.status != "pending" {
            return Err(AppError::Conflict(format!(
                "share edit is {} and cannot be applied",
                edit.status
            )));
        }
        if returned_share.share_id != edit.share_id {
            return Err(AppError::BadRequest(
                "control reply share_id does not match edit".into(),
            ));
        }
        if let Err(field) = validate_returned_share_against_patch(&edit.patch, &returned_share) {
            let message = format!("client reply did not satisfy patch field: {field}");
            conn.execute(
                "UPDATE share_edit_requests
                 SET status = 'rejected', updated_at = ?2, error_message = ?3
                 WHERE id = ?1 AND status = 'pending'",
                params![edit_id, now.to_rfc3339(), message],
            )
            .map_err(|e| AppError::Internal(format!("reject share edit failed: {e}")))?;
            return Err(AppError::UnprocessableEntity(message));
        }

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Internal(format!("begin apply edit tx failed: {e}")))?;
        ensure_share_id_writable_by_installation(
            &tx,
            &returned_share.share_id,
            &edit.installation_id,
        )?;
        let installation = get_installation(&tx, &edit.installation_id)?
            .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
        let installation_owner = installation
            .owner_email
            .as_deref()
            .ok_or_else(|| AppError::Conflict("installation owner email is not configured".into()))
            .and_then(normalize_email)?;
        normalize_self_reported_share_owner(&mut returned_share, &installation_owner)?;
        upsert_share_tx(&tx, &edit.installation_id, returned_share)?;
        let changed = tx
            .execute(
                "UPDATE share_edit_requests
                 SET status = 'applied', updated_at = ?2, applied_at = ?2, error_message = NULL
                 WHERE id = ?1 AND status = 'pending'",
                params![edit_id, now.to_rfc3339()],
            )
            .map_err(|e| AppError::Internal(format!("apply share edit failed: {e}")))?;
        if changed == 0 {
            return Err(AppError::Conflict(
                "share edit was no longer pending".into(),
            ));
        }
        tx.commit().map_err(map_share_constraint_error)?;
        Ok(())
    }

    /// Marks a pending edit rejected with an operator-facing reason. Used when
    /// the control RPC itself failed in a non-transport way (e.g. the client
    /// returned a hard error) and the dashboard should see the failure rather
    /// than silently fall back.
    pub async fn mark_share_edit_rejected(
        &self,
        edit_id: &str,
        error_message: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE share_edit_requests
             SET status = 'rejected', updated_at = ?2, error_message = ?3
             WHERE id = ?1 AND status = 'pending'",
            params![edit_id, Utc::now().to_rfc3339(), error_message],
        )
        .map_err(|e| AppError::Internal(format!("reject share edit failed: {e}")))?;
        Ok(())
    }

    pub async fn pending_share_edits(
        &self,
        input: SharePendingEditsRequest,
        metadata: ClientMetadata,
    ) -> Result<SharePendingEditsResponse, AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_pending_edits",
            &input.share_ids,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        let edits = list_pending_share_edits_for_installation(
            &conn,
            &input.installation_id,
            &input.share_ids,
        )?;
        Ok(SharePendingEditsResponse { edits })
    }

    pub async fn ack_share_edit(
        &self,
        input: ShareEditAckRequest,
        metadata: ClientMetadata,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_edit_ack",
            &input.ack,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        let status = match input.ack.status.as_str() {
            "applied" => "applied",
            "rejected" => "rejected",
            _ => {
                return Err(AppError::BadRequest(
                    "share edit ack status must be applied or rejected".into(),
                ));
            }
        };
        let now = Utc::now().to_rfc3339();
        let changed = conn
            .execute(
                "UPDATE share_edit_requests
                 SET status = ?4,
                     updated_at = ?5,
                     applied_at = CASE WHEN ?4 = 'applied' THEN ?5 ELSE applied_at END,
                     error_message = ?6
                 WHERE id = ?1
                   AND installation_id = ?2
                   AND revision = ?3
                   AND status = 'pending'",
                params![
                    input.ack.edit_id,
                    input.installation_id,
                    input.ack.revision,
                    status,
                    now,
                    input.ack.error_message,
                ],
            )
            .map_err(|e| AppError::Internal(format!("ack share edit failed: {e}")))?;
        if changed == 0 {
            return Err(AppError::Conflict(
                "share edit ack did not match an active pending edit".into(),
            ));
        }
        Ok(())
    }

    pub async fn is_share_edit_pending(
        &self,
        edit_id: &str,
        revision: i64,
    ) -> Result<bool, AppError> {
        let conn = self.conn.lock().await;
        let pending = conn
            .query_row(
                "SELECT 1
                 FROM share_edit_requests
                 WHERE id = ?1
                   AND revision = ?2
                   AND status = 'pending'
                 LIMIT 1",
                params![edit_id, revision],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query share edit pending state failed: {e}")))?
            .is_some();
        Ok(pending)
    }

    pub async fn verify_share_edit_event_stream(
        &self,
        installation_id: &str,
        payload: &impl Serialize,
        timestamp_ms: i64,
        nonce: &str,
        signature: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            installation_id,
            "share_edit_events",
            payload,
            timestamp_ms,
            nonce,
            signature,
        )
    }

    pub async fn batch_sync_share_request_logs(
        &self,
        input: ShareRequestLogBatchSyncRequest,
        metadata: ClientMetadata,
        _current_user_email: &str,
        live_request_context_by_id: HashMap<
            String,
            (Option<String>, Option<String>, Option<String>),
        >,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_request_logs_batch_sync",
            &input.logs,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        drop(conn);
        if should_refresh_geo {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }

        let conn = self.conn.lock().await;
        let tx = conn.unchecked_transaction().map_err(|e| {
            AppError::Internal(format!("begin request log batch sync tx failed: {e}"))
        })?;
        for mut log in input.logs {
            if !share_belongs_to_installation(&tx, &log.share_id, &input.installation_id)? {
                return Err(AppError::Unauthorized(
                    "only share installation can sync request logs".into(),
                ));
            }
            if let Some((user_country, user_country_iso3, user_email)) =
                live_request_context_by_id.get(&log.request_id)
            {
                if log.user_country.is_none() {
                    log.user_country = user_country.clone();
                }
                if log.user_country_iso3.is_none() {
                    log.user_country_iso3 = user_country_iso3.clone();
                }
                if log.user_email.is_none() {
                    log.user_email = user_email.clone();
                }
            }
            upsert_share_request_log_tx(&tx, &input.installation_id, log)?;
        }
        tx.commit().map_err(|e| {
            AppError::Internal(format!("commit request log batch sync failed: {e}"))
        })?;
        Ok(())
    }

    pub async fn prepare_share_runtime_refresh(
        &self,
        input: ShareRuntimeRefreshRequest,
        metadata: ClientMetadata,
    ) -> Result<ShareRuntimeRefreshPayload, AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        verify_signed_share_request(
            &conn,
            &installation.public_key,
            &input.installation_id,
            "share_runtime_refresh",
            &input.refresh,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )?;
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;

        let subdomain = conn
            .query_row(
                "SELECT subdomain FROM shares WHERE share_id = ?1 AND installation_id = ?2",
                params![&input.refresh.share_id, &input.installation_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query share runtime target failed: {e}")))?;
        let Some(subdomain) = subdomain else {
            return Err(AppError::BadRequest("share not found".into()));
        };
        let Some(subdomain) = subdomain else {
            return Err(AppError::BadRequest("share subdomain is not set".into()));
        };
        if subdomain != input.refresh.subdomain {
            return Err(AppError::BadRequest("share subdomain mismatch".into()));
        }
        drop(conn);

        if should_refresh_geo {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }

        Ok(input.refresh)
    }

    pub async fn dashboard_snapshot(
        &self,
        config: &Config,
        server_geo: &ServerGeo,
        proxy: &ProxyRegistry,
        viewer_email: Option<&str>,
    ) -> Result<DashboardResponse, AppError> {
        let active_subdomains = proxy
            .active_subdomains()
            .await
            .into_iter()
            .collect::<HashSet<_>>();
        let inflight_by_share = proxy.inflight_by_share().await;
        let inflight_by_share_app = proxy.inflight_by_share_app().await;
        let inflight_by_market_email = proxy.inflight_by_market_email().await;
        let now = Utc::now();
        let (health_timeline_start, _) = health_timeline_window(now);
        let (
            installations,
            shares,
            active_edits_by_share,
            health_by_share,
            health_timeline_by_share,
            online_by_share,
            health_by_installation,
            health_timeline_by_installation,
            online_by_installation,
            recent_logs,
            recent_model_health_checks,
            market_logs,
            market_request_timeline_by_market,
            model_health_by_share,
            share_usage_by_app,
            client_tunnels,
            payout_profiles,
        ) = {
            let conn = self.conn.lock().await;
            (
                list_installations(&conn)?,
                list_shares(&conn)?,
                list_active_share_edits(&conn)?,
                list_health_checks(&conn, 10)?,
                list_share_health_timeline_24h(&conn, now)?,
                list_online_minutes_24h(&conn)?,
                list_installation_health_checks(&conn, 10)?,
                list_installation_health_timeline_24h(&conn, now)?,
                list_installation_online_minutes_24h(&conn)?,
                list_recent_share_request_logs(&conn, SHARE_REQUEST_LOG_RECOVERY_LIMIT)?,
                list_recent_share_model_health_checks(&conn, SHARE_MODEL_HEALTH_CHECK_LIMIT)?,
                list_recent_market_request_logs(&conn, 200)?,
                list_market_request_timeline_stats_24h(&conn, now)?,
                list_model_health_summaries(&conn)?,
                list_share_usage_by_app(&conn)?,
                list_client_tunnels(&conn)?,
                list_dashboard_payout_profiles(&conn)?,
            )
        };
        let client_tunnels_by_installation = client_tunnels
            .into_iter()
            .map(|tunnel| (tunnel.installation_id.clone(), tunnel))
            .collect::<HashMap<_, _>>();
        let market_logs_by_market = market_logs.iter().cloned().fold(
            HashMap::<String, Vec<DashboardMarketRequestLogView>>::new(),
            |mut acc, log| {
                acc.entry(log.market_email.to_ascii_lowercase())
                    .or_default()
                    .push(log);
                acc
            },
        );
        let markets = {
            let conn = self.conn.lock().await;
            list_dashboard_markets(
                &conn,
                viewer_email,
                &active_subdomains,
                &shares,
                &inflight_by_share,
                &online_by_share,
                &health_by_share,
                &health_timeline_by_share,
                &inflight_by_market_email,
                &market_logs_by_market,
                &market_request_timeline_by_market,
                health_timeline_start,
            )?
        };
        let logs_by_share = recent_logs.into_iter().fold(
            HashMap::<String, Vec<ShareRequestLogEntry>>::new(),
            |mut acc, log| {
                acc.entry(log.share_id.clone()).or_default().push(log);
                acc
            },
        );
        let mut logs_by_share = self
            .recover_missing_share_request_logs(config, &active_subdomains, &shares, logs_by_share)
            .await?;
        merge_market_request_logs_into_share_logs(
            &mut logs_by_share,
            &market_logs,
            &shares,
            SHARE_REQUEST_LOG_RECOVERY_LIMIT,
        );
        let model_health_checks_by_share = recent_model_health_checks.into_iter().fold(
            HashMap::<String, Vec<ShareModelHealthCheckEntry>>::new(),
            |mut acc, check| {
                acc.entry(check.share_id.clone()).or_default().push(check);
                acc
            },
        );

        let mut active_share_subdomains_by_installation: HashMap<String, HashSet<String>> =
            HashMap::new();
        for (installation_id, share) in &shares {
            if share.share_status == "active" && active_subdomains.contains(&share.subdomain) {
                active_share_subdomains_by_installation
                    .entry(installation_id.clone())
                    .or_default()
                    .insert(share.subdomain.clone());
            }
        }
        let installations = deduplicate_dashboard_installations(
            installations,
            &active_share_subdomains_by_installation,
        );
        let installation_cleanup_at = installations
            .iter()
            .map(|installation| {
                (
                    installation.id.clone(),
                    installation.last_seen_at + Duration::seconds(config.client_stale_secs),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut installation_views = Vec::new();
        let mut client_map_points = Vec::new();
        let mut country_counts: HashMap<String, usize> = HashMap::new();
        for installation in installations {
            let is_active = active_share_subdomains_by_installation
                .get(&installation.id)
                .map(|subdomains| !subdomains.is_empty())
                .unwrap_or(false);
            if is_active {
                let (lat, lon) = match (installation.latitude, installation.longitude) {
                    (Some(lat), Some(lon)) => (Some(lat), Some(lon)),
                    _ => match installation
                        .country_code
                        .as_deref()
                        .and_then(country_centroid)
                    {
                        Some((lat, lon)) => (Some(lat), Some(lon)),
                        None => (None, None),
                    },
                };
                if let Some(iso3) = installation
                    .country_code
                    .as_deref()
                    .and_then(crate::geo::iso2_to_iso3)
                {
                    *country_counts.entry(iso3.to_string()).or_insert(0) += 1;
                }
                client_map_points.push(DashboardMapPoint {
                    id: installation.id.clone(),
                    label: installation.platform.clone(),
                    point_type: "client".into(),
                    platform: Some(installation.platform.clone()),
                    country_code: installation.country_code.clone(),
                    country: installation.country.clone(),
                    region: installation.region.clone(),
                    city: installation.city.clone(),
                    lat,
                    lon,
                    last_seen_at: Some(installation.last_seen_at),
                    is_active,
                    active_requests: 0,
                });
            }
            installation_views.push(InstallationView {
                id: installation.id,
                platform: installation.platform,
                app_version: installation.app_version,
                owner_email: installation.owner_email,
                region: installation.region,
                country_code: installation.country_code,
                created_at: installation.created_at,
                last_seen_at: installation.last_seen_at,
            });
        }
        installation_views.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));

        let market_by_email = markets
            .iter()
            .cloned()
            .map(|market| (market.email.to_ascii_lowercase(), market))
            .collect::<HashMap<_, _>>();

        let share_views = shares
            .into_iter()
            .map(|(installation_id, share)| {
                let active_requests = inflight_by_share.get(&share.share_id).copied().unwrap_or(0);
                let active_requests_by_app = inflight_by_share_app
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let (tokens_used_by_app, requests_count_by_app) = share_usage_by_app
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let recent_requests = logs_by_share
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let health_checks = health_by_share
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let health_timeline = health_timeline_by_share
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let recent_model_health_checks = model_health_checks_by_share
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let is_online =
                    share.share_status == "active" && active_subdomains.contains(&share.subdomain);
                let online_minutes_24h = online_by_share.get(&share.share_id).copied().unwrap_or(0);
                let online_rate_24h =
                    ((online_minutes_24h as f64 / ONLINE_WINDOW_MINUTES as f64) * 100.0).min(100.0);
                let can_view_share = share_visible_to_email(&share, viewer_email);
                let can_view_secret = can_view_share;
                let can_manage = can_manage_share(&share, viewer_email);
                let active_edit = active_edits_by_share.get(&share.share_id).cloned();
                let can_edit_settings = can_manage
                    && active_edit
                        .as_ref()
                        .map(|edit| edit.status != "pending")
                        .unwrap_or(true);
                let market_links = if share.sale_market_kind == "share" {
                    let market_emails = share_acl_emails(&share);
                    market_emails
                        .iter()
                        .filter_map(|email| market_by_email.get(&email.to_ascii_lowercase()))
                        .filter(|market| market.market_kind == "share")
                        .map(dashboard_market_to_share_link)
                        .collect::<Vec<_>>()
                } else if share.market_access_mode == "all"
                    || share
                        .access_by_app
                        .values()
                        .any(|access| access.market_access_mode == "all")
                {
                    markets
                        .iter()
                        .filter(|market| market.market_kind != "share")
                        .map(dashboard_market_to_share_link)
                        .collect::<Vec<_>>()
                } else {
                    let market_emails = share_acl_emails(&share);
                    market_emails
                        .iter()
                        .filter_map(|email| market_by_email.get(&email.to_ascii_lowercase()))
                        .filter(|market| market.market_kind != "share")
                        .map(dashboard_market_to_share_link)
                        .collect::<Vec<_>>()
                };
                let unknown_market_emails = if can_manage {
                    let market_emails = share_acl_emails(&share);
                    market_emails
                        .iter()
                        .filter(|email| !market_by_email.contains_key(&email.to_ascii_lowercase()))
                        .cloned()
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                let model_health = model_health_by_share
                    .get(&share.share_id)
                    .cloned()
                    .unwrap_or_default();
                let mut view = ShareView {
                    router_id: "main".to_string(),
                    share_id: share.share_id,
                    share_name: share.share_name,
                    owner_email: share.owner_email,
                    shared_with_emails: if can_view_share {
                        share.shared_with_emails
                    } else {
                        Vec::new()
                    },
                    access_by_app: if can_view_share {
                        share.access_by_app
                    } else {
                        BTreeMap::new()
                    },
                    app_settings: if can_view_share {
                        share.app_settings
                    } else {
                        BTreeMap::new()
                    },
                    market_links,
                    unknown_market_emails,
                    description: share.description,
                    for_sale: share.for_sale,
                    sale_market_kind: share.sale_market_kind,
                    market_access_mode: share.market_access_mode,
                    for_sale_official_price_percent_by_app: share
                        .for_sale_official_price_percent_by_app,
                    subdomain: share.subdomain,
                    app_type: share.app_type,
                    can_view_secret,
                    can_manage,
                    can_edit_settings,
                    active_edit,
                    provider_id: share.provider_id,
                    bindings: share.bindings,
                    token_limit: share.token_limit,
                    parallel_limit: share.parallel_limit,
                    tokens_used: share.tokens_used,
                    requests_count: share.requests_count,
                    share_status: share.share_status,
                    created_at: share.created_at,
                    expires_at: share.expires_at,
                    support: share.support,
                    upstream_provider: share.upstream_provider,
                    app_runtimes: share.app_runtimes,
                    app_providers: share.app_providers,
                    installation_id: installation_id.clone(),
                    is_online,
                    cleanup_at: (!is_online)
                        .then(|| installation_cleanup_at.get(&installation_id).copied())
                        .flatten(),
                    active_requests,
                    active_requests_by_app,
                    tokens_used_by_app,
                    requests_count_by_app,
                    online_minutes_24h,
                    online_rate_24h,
                    recent_requests,
                    health_checks,
                    health_timeline,
                    recent_model_health_checks,
                    model_health,
                    operational_summary: OperationalSummary::healthy("online"),
                };
                view.operational_summary = share_operational_summary(&view, Utc::now());
                view
            })
            .collect::<Vec<_>>();
        let active_requests_by_installation =
            share_views
                .iter()
                .fold(HashMap::<String, usize>::new(), |mut acc, share| {
                    *acc.entry(share.installation_id.clone()).or_insert(0) += share.active_requests;
                    acc
                });
        for point in &mut client_map_points {
            point.active_requests = active_requests_by_installation
                .get(&point.id)
                .copied()
                .unwrap_or(0);
        }
        let ticker_shares = share_views
            .iter()
            .map(|share| DashboardTickerShare {
                share_id: share.share_id.clone(),
                share_name: share.share_name.clone(),
                subdomain: share.subdomain.clone(),
                recent_requests: share.recent_requests.clone(),
            })
            .collect::<Vec<_>>();
        // P7 Step 2：clients 表退化成 installation 维度。share 维度信息走顶层 `shares` 数组；
        // 这里只为每个 installation 累积 share_ids（用于 ClientsTable 的 #shares 列和抽屉里
        // 列出该机所有 share），不再 collapse 出"代表 share"，也不再保留 clients[*].share。
        let mut share_ids_by_installation = HashMap::<String, Vec<String>>::new();
        for share in &share_views {
            share_ids_by_installation
                .entry(share.installation_id.clone())
                .or_default()
                .push(share.share_id.clone());
        }
        let mut client_online_minutes_by_installation = HashMap::<String, usize>::new();
        let mut client_health_by_installation = HashMap::<String, Vec<HealthCheckEntry>>::new();
        let mut client_timeline_by_installation =
            HashMap::<String, Vec<HealthTimelineBucket>>::new();
        for share in &share_views {
            client_online_minutes_by_installation
                .entry(share.installation_id.clone())
                .and_modify(|minutes| *minutes = (*minutes).max(share.online_minutes_24h))
                .or_insert(share.online_minutes_24h);
            client_health_by_installation
                .entry(share.installation_id.clone())
                .and_modify(|entries| {
                    *entries = merge_health_checks(entries, &share.health_checks);
                })
                .or_insert_with(|| share.health_checks.clone());
            client_timeline_by_installation
                .entry(share.installation_id.clone())
                .and_modify(|timeline| {
                    *timeline = merge_health_timeline(timeline, &share.health_timeline);
                })
                .or_insert_with(|| share.health_timeline.clone());
        }
        let share_views_by_id = share_views
            .iter()
            .map(|share| (share.share_id.clone(), share))
            .collect::<HashMap<_, _>>();
        let mut client_views = installation_views
            .iter()
            .cloned()
            .map(|installation| {
                let share_ids = share_ids_by_installation
                    .remove(&installation.id)
                    .unwrap_or_default();
                let share_count = share_ids.len();
                let tunnel_record = client_tunnels_by_installation.get(&installation.id);
                let client_tunnel_online = tunnel_record
                    .map(|tunnel| tunnel.enabled && active_subdomains.contains(&tunnel.subdomain))
                    .unwrap_or(false);
                let mut online_minutes_24h = client_online_minutes_by_installation
                    .get(&installation.id)
                    .copied()
                    .unwrap_or(0);
                if share_count == 0 {
                    online_minutes_24h = online_by_installation
                        .get(&installation.id)
                        .copied()
                        .unwrap_or(online_minutes_24h);
                    if online_minutes_24h == 0 && client_tunnel_online {
                        online_minutes_24h = 1;
                    }
                }
                let online_rate_24h =
                    ((online_minutes_24h as f64 / ONLINE_WINDOW_MINUTES as f64) * 100.0).min(100.0);
                let client_tunnel = tunnel_record.map(|tunnel| DashboardClientTunnelView {
                    owner_email: tunnel.owner_email.clone(),
                    subdomain: tunnel.subdomain.clone(),
                    tunnel_url: config.tunnel_url(&tunnel.subdomain),
                    enabled: tunnel.enabled,
                    online: client_tunnel_online,
                });
                let payout_profile = payout_profiles.get(&installation.id).cloned();
                let mut health_checks = client_health_by_installation
                    .get(&installation.id)
                    .cloned()
                    .unwrap_or_default();
                if share_count == 0 {
                    health_checks = health_by_installation
                        .get(&installation.id)
                        .cloned()
                        .unwrap_or(health_checks);
                    if client_tunnel_online {
                        if health_checks.is_empty() {
                            append_recent_online_health_checks(&mut health_checks, 10);
                        } else {
                            append_current_online_health_check(&mut health_checks);
                        }
                    }
                }
                let mut health_timeline = client_timeline_by_installation
                    .get(&installation.id)
                    .cloned()
                    .unwrap_or_default();
                if share_count == 0 {
                    health_timeline = health_timeline_by_installation
                        .get(&installation.id)
                        .cloned()
                        .unwrap_or(health_timeline);
                    if health_timeline.is_empty() && client_tunnel_online {
                        health_timeline = current_online_health_timeline(health_timeline_start);
                    }
                }
                let mut view = DashboardClientView {
                    share_count,
                    share_ids,
                    client_tunnel,
                    payout_profile,
                    online_minutes_24h,
                    online_rate_24h,
                    health_checks,
                    health_timeline,
                    installation,
                    operational_summary: OperationalSummary::healthy("online"),
                };
                view.operational_summary = client_operational_summary(
                    &view,
                    &share_views_by_id,
                    config.client_stale_secs,
                    Utc::now(),
                );
                view
            })
            // 只展示有 share 或已配置 client tunnel 的机器；otherwise dashboard 会被纯
            // lease/heartbeat 但无可用入口的 installation 撑满。
            .filter(|client| {
                !client.share_ids.is_empty()
                    || client.client_tunnel.is_some()
                    || client.payout_profile.is_some()
            })
            .collect::<Vec<_>>();
        client_views.sort_by(|left, right| {
            // P7 Step 2：installation 维度排序。原"can_manage 优先"是 share 字段，已移除；
            // 退化为按"share 数量降序 + 最近上线时间降序"，让活跃机器在上面。
            right.share_count.cmp(&left.share_count).then_with(|| {
                right
                    .installation
                    .last_seen_at
                    .cmp(&left.installation.last_seen_at)
            })
        });
        let clients_count = client_views.len();
        let active_shares_count = share_views
            .iter()
            .filter(|share| share.share_status == "active")
            .count();
        let total_active_requests = share_views.iter().map(|share| share.active_requests).sum();
        let map_display = {
            let conn = self.conn.lock().await;
            read_map_display_settings(&conn)?
        };
        Ok(DashboardResponse {
            generated_at: now,
            stats: DashboardStats {
                clients: clients_count,
                active_shares: active_shares_count,
                total_active_requests,
            },
            map: DashboardMap {
                server: server_geo
                    .lat
                    .zip(server_geo.lon)
                    .map(|(lat, lon)| DashboardMapPoint {
                        id: "server".into(),
                        label: "server".into(),
                        point_type: "server".into(),
                        platform: None,
                        country_code: None,
                        country: None,
                        region: None,
                        city: None,
                        lat: Some(lat),
                        lon: Some(lon),
                        last_seen_at: Some(now),
                        is_active: true,
                        active_requests: 0,
                    }),
                clients: client_map_points,
            },
            clients: client_views,
            // Share 全量列表；前端按 installation 分组为独立横向卡片。
            // clients 只持有 share_ids/share_count，不重复嵌入 share 实体。
            shares: share_views.clone(),
            markets,
            ticker_shares,
            country_counts,
            user_country_counts: HashMap::new(),
            recent_request_events: Vec::new(),
            market_request_logs: market_logs,
            map_display,
        })
    }

    pub async fn map_display_settings(&self) -> Result<MapDisplaySettings, AppError> {
        let conn = self.conn.lock().await;
        read_map_display_settings(&conn)
    }

    pub async fn update_map_display_settings(
        &self,
        update: MapDisplaySettingsUpdate,
    ) -> Result<MapDisplaySettings, AppError> {
        let conn = self.conn.lock().await;
        let mut current = read_map_display_settings(&conn)?;
        if let Some(show_flows) = update.show_flows {
            current.show_flows = show_flows;
        }
        if let Some(show_heat) = update.show_heat {
            current.show_heat = show_heat;
        }
        if let Some(viewport) = update.viewport {
            if let Some(visible_start_px) = viewport.visible_start_px {
                current.viewport.visible_start_px = visible_start_px;
            }
        }
        current = sanitize_map_display_settings(current);
        write_map_display_settings(&conn, &current)?;
        Ok(current)
    }

    async fn recover_missing_share_request_logs(
        &self,
        config: &Config,
        active_subdomains: &HashSet<String>,
        shares: &[(String, ShareDescriptor)],
        mut logs_by_share: HashMap<String, Vec<ShareRequestLogEntry>>,
    ) -> Result<HashMap<String, Vec<ShareRequestLogEntry>>, AppError> {
        let now_ts = Utc::now().timestamp();
        let missing_shares = shares
            .iter()
            .filter(|(_, share)| {
                active_subdomains.contains(&share.subdomain)
                    && share_logs_need_recovery(
                        logs_by_share.get(&share.share_id).map(Vec::as_slice),
                        now_ts,
                    )
            })
            .map(|(installation_id, share)| {
                (
                    installation_id.clone(),
                    share.share_id.clone(),
                    share.subdomain.clone(),
                )
            })
            .collect::<Vec<_>>();
        let missing_shares = {
            let mut attempted = self.share_log_recovery_attempts.lock().await;
            missing_shares
                .into_iter()
                .filter(|(_, share_id, _)| {
                    if attempted
                        .get(share_id)
                        .map(|last_attempt| {
                            now_ts - *last_attempt < SHARE_REQUEST_LOG_RECOVERY_COOLDOWN_SECS
                        })
                        .unwrap_or(false)
                    {
                        return false;
                    }
                    attempted.insert(share_id.clone(), now_ts);
                    true
                })
                .collect::<Vec<_>>()
        };

        if missing_shares.is_empty() {
            return Ok(logs_by_share);
        }

        let client = reqwest::Client::builder()
            .user_agent("cc-switch-router/0.1 share-log-recovery")
            .timeout(StdDuration::from_secs(5))
            .build()
            .map_err(|e| {
                AppError::Internal(format!("build share log recovery client failed: {e}"))
            })?;

        for (installation_id, share_id, subdomain) in missing_shares {
            let response =
                match fetch_share_request_logs_from_route(config, &client, &subdomain).await {
                    Ok(response) => response,
                    Err(err) => {
                        tracing::debug!(
                            share_id = %share_id,
                            subdomain = %subdomain,
                            "share request log recovery skipped: {err}"
                        );
                        continue;
                    }
                };

            if response.logs.is_empty() {
                continue;
            }

            if let Some(response_share_id) = response.share_id.as_deref() {
                if response_share_id != share_id {
                    tracing::debug!(
                        share_id = %share_id,
                        response_share_id = %response_share_id,
                        subdomain = %subdomain,
                        "share request log recovery returned mismatched share id"
                    );
                }
            }

            {
                let mut recovered_logs = response.logs;
                recovered_logs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                recovered_logs.truncate(SHARE_REQUEST_LOG_RECOVERY_LIMIT);
                let conn = self.conn.lock().await;
                let tx = conn.unchecked_transaction().map_err(|e| {
                    AppError::Internal(format!("begin share request log recovery tx failed: {e}"))
                })?;
                for log in &recovered_logs {
                    upsert_share_request_log_tx(&tx, &installation_id, log.clone())?;
                }
                tx.commit().map_err(|e| {
                    AppError::Internal(format!("commit share request log recovery tx failed: {e}"))
                })?;
                logs_by_share.insert(share_id.clone(), recovered_logs);
            }

            tracing::info!(
                share_id = %share_id,
                subdomain = %subdomain,
                recovered = logs_by_share.get(&share_id).map(|logs| logs.len()).unwrap_or(0),
                "recovered share request logs from active route"
            );
        }

        Ok(logs_by_share)
    }

    pub async fn cleanup_expired_data(
        &self,
        config: &Config,
        proxy: &ProxyRegistry,
    ) -> Result<CleanupResult, AppError> {
        let active_subdomains = proxy.active_subdomains().await;
        let cutoff = (Utc::now() - Duration::seconds(config.lease_retention_secs)).to_rfc3339();
        let log_cutoff_ts = DateTime::parse_from_rfc3339(&cutoff)
            .map(|dt| dt.timestamp())
            .unwrap_or_default();
        let stale_cutoff = (Utc::now() - Duration::seconds(config.client_stale_secs)).to_rfc3339();
        let paused_cutoff =
            (Utc::now() - Duration::seconds(config.paused_share_stale_secs)).to_rfc3339();
        let market_missing_cutoff =
            (Utc::now() - Duration::seconds(MARKET_ACTIVE_MISSING_GRACE_SECS)).to_rfc3339();
        let market_release_cutoff =
            (Utc::now() - Duration::seconds(MARKET_OFFLINE_GRACE_SECS)).to_rfc3339();
        let active_subdomains_set = active_subdomains.iter().cloned().collect::<HashSet<_>>();
        let (mut result, stale_subdomains, stale_image_storage_keys) = {
            let conn = self.conn.lock().await;
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| AppError::Internal(format!("begin cleanup tx failed: {e}")))?;

            if !active_subdomains.is_empty() {
                let now = Utc::now().to_rfc3339();
                for chunk in active_subdomains.chunks(CLEANUP_ACTIVE_SUBDOMAIN_CHUNK_SIZE) {
                    let placeholders = repeat_vars(chunk.len());
                    let mut update_installation_params = Vec::with_capacity(chunk.len() + 1);
                    update_installation_params.push(now.clone());
                    update_installation_params.extend(chunk.iter().cloned());
                    tx.execute(
                        &format!(
                            "UPDATE installations
                             SET last_seen_at = ?
                             WHERE id IN (
                                 SELECT DISTINCT installation_id
                                 FROM shares
                                 WHERE subdomain IN ({placeholders})
                             )"
                        ),
                        params_from_iter(update_installation_params),
                    )
                    .map_err(|e| {
                        AppError::Internal(format!("touch active route installations failed: {e}"))
                    })?;

                    let mut update_share_params = Vec::with_capacity(chunk.len() + 1);
                    update_share_params.push(now.clone());
                    update_share_params.extend(chunk.iter().cloned());
                    tx.execute(
                        &format!(
                            "UPDATE shares
                             SET updated_at = ?
                             WHERE subdomain IN ({placeholders})"
                        ),
                        params_from_iter(update_share_params),
                    )
                    .map_err(|e| {
                        AppError::Internal(format!("touch active route shares failed: {e}"))
                    })?;
                }
            }

            let mut stale_subdomains = {
                let mut stmt = tx
                    .prepare(
                        "SELECT DISTINCT subdomain
                         FROM shares
                         WHERE installation_id IN (
                             SELECT id FROM installations WHERE last_seen_at < ?1
                         )
                           AND subdomain IS NOT NULL
                           AND subdomain != ''
                           AND subdomain != '-'",
                    )
                    .map_err(|e| AppError::Internal(format!("prepare stale routes failed: {e}")))?;
                let rows = stmt
                    .query_map(params![stale_cutoff], |row| row.get::<_, String>(0))
                    .map_err(|e| AppError::Internal(format!("query stale routes failed: {e}")))?;
                collect_rows(rows)?
            };

            let deleted_leases = tx
                .execute(
                    "DELETE FROM leases
                     WHERE expires_at < ?1
                       AND (used_at IS NULL OR used_at < ?1)",
                    params![cutoff],
                )
                .map_err(|e| AppError::Internal(format!("delete expired leases failed: {e}")))?
                as usize;

            tx.execute(
                "DELETE FROM share_health_checks
                     WHERE share_id IN (
                         SELECT share_id
                         FROM shares
                         WHERE installation_id IN (
                             SELECT id FROM installations WHERE last_seen_at < ?1
                         )
                     )",
                params![stale_cutoff],
            )
            .map_err(|e| AppError::Internal(format!("delete stale share health failed: {e}")))?;

            let deleted_stale_shares = tx
                .execute(
                    "DELETE FROM shares
                     WHERE installation_id IN (
                         SELECT id FROM installations WHERE last_seen_at < ?1
                     )",
                    params![stale_cutoff],
                )
                .map_err(|e| {
                    AppError::Internal(format!("delete stale client shares failed: {e}"))
                })? as usize;

            let deleted_stale_leases = tx
                .execute(
                    "DELETE FROM leases
                     WHERE installation_id IN (
                         SELECT id FROM installations WHERE last_seen_at < ?1
                     )",
                    params![stale_cutoff],
                )
                .map_err(|e| {
                    AppError::Internal(format!("delete stale client leases failed: {e}"))
                })? as usize;

            let deleted_installations = 0;

            let stale_active_offline_shares = {
                let mut stmt = tx
                    .prepare(
                        "SELECT share_id, subdomain
                         FROM shares
                         WHERE share_status = 'active'
                           AND updated_at < ?1
                           AND subdomain IS NOT NULL
                           AND subdomain != ''
                           AND subdomain != '-'",
                    )
                    .map_err(|e| {
                        AppError::Internal(format!(
                            "prepare stale active offline shares failed: {e}"
                        ))
                    })?;
                let rows = stmt
                    .query_map(params![stale_cutoff], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| {
                        AppError::Internal(format!("query stale active offline shares failed: {e}"))
                    })?;
                collect_rows(rows)?
                    .into_iter()
                    .filter(|(_, subdomain)| !active_subdomains_set.contains(subdomain))
                    .collect::<Vec<_>>()
            };
            let (deleted_stale_active_offline_shares, deleted_stale_active_offline_leases) =
                if stale_active_offline_shares.is_empty() {
                    (0, 0)
                } else {
                    let stale_active_share_ids = stale_active_offline_shares
                        .iter()
                        .map(|(share_id, _)| share_id.clone())
                        .collect::<Vec<_>>();
                    let stale_active_subdomains = stale_active_offline_shares
                        .iter()
                        .map(|(_, subdomain)| subdomain.clone())
                        .collect::<Vec<_>>();

                    let share_placeholders = repeat_vars(stale_active_share_ids.len());
                    tx.execute(
                        &format!(
                            "DELETE FROM share_health_checks
                             WHERE share_id IN ({share_placeholders})"
                        ),
                        params_from_iter(stale_active_share_ids.iter()),
                    )
                    .map_err(|e| {
                        AppError::Internal(format!(
                            "delete stale active offline share health failed: {e}"
                        ))
                    })?;

                    let subdomain_placeholders = repeat_vars(stale_active_subdomains.len());
                    let deleted_leases = tx
                        .execute(
                            &format!(
                                "DELETE FROM leases
                                 WHERE subdomain IN ({subdomain_placeholders})"
                            ),
                            params_from_iter(stale_active_subdomains.iter()),
                        )
                        .map_err(|e| {
                            AppError::Internal(format!(
                                "delete stale active offline share leases failed: {e}"
                            ))
                        })? as usize;

                    let deleted_shares = tx
                        .execute(
                            &format!("DELETE FROM shares WHERE share_id IN ({share_placeholders})"),
                            params_from_iter(stale_active_share_ids.iter()),
                        )
                        .map_err(|e| {
                            AppError::Internal(format!(
                                "delete stale active offline shares failed: {e}"
                            ))
                        })? as usize;

                    stale_subdomains.extend(stale_active_subdomains);
                    (deleted_shares, deleted_leases)
                };

            // 已暂停且 updated_at 长期未刷新的 share 自动 GC：installation 维度的清理
            // 只删整台机器全离线的 share，但用户明确 paused 后只想保留账号、不想留路由的
            // 也应该到期回收。先清掉 health_checks 再删 share 行，避免 orphan。
            tx.execute(
                "DELETE FROM share_health_checks
                     WHERE share_id IN (
                         SELECT share_id FROM shares
                         WHERE share_status = 'paused' AND updated_at < ?1
                     )",
                params![paused_cutoff],
            )
            .map_err(|e| AppError::Internal(format!("delete paused share health failed: {e}")))?;
            let deleted_paused_shares = tx
                .execute(
                    "DELETE FROM shares
                     WHERE share_status = 'paused' AND updated_at < ?1",
                    params![paused_cutoff],
                )
                .map_err(|e| AppError::Internal(format!("delete paused shares failed: {e}")))?
                as usize;

            let deleted_old_shares = tx
                .execute(
                    "DELETE FROM shares
                     WHERE share_status IN ('expired', 'deleted')
                       AND updated_at < ?1",
                    params![cutoff],
                )
                .map_err(|e| AppError::Internal(format!("delete stale shares failed: {e}")))?
                as usize;

            let _deleted_request_logs = tx
                .execute(
                    "DELETE FROM share_request_logs
                     WHERE created_at < ?1",
                    params![log_cutoff_ts],
                )
                .map_err(|e| {
                    AppError::Internal(format!("delete stale request logs failed: {e}"))
                })?;

            let stale_image_storage_keys = {
                let mut stmt = tx
                    .prepare(
                        "SELECT result_storage_key
                         FROM image_generation_request_logs
                         WHERE created_at < ?1
                           AND result_storage_key IS NOT NULL
                           AND result_storage_key != ''",
                    )
                    .map_err(|e| {
                        AppError::Internal(format!(
                            "prepare stale image request result files failed: {e}"
                        ))
                    })?;
                let rows = stmt
                    .query_map(params![log_cutoff_ts], |row| row.get::<_, String>(0))
                    .map_err(|e| {
                        AppError::Internal(format!(
                            "query stale image request result files failed: {e}"
                        ))
                    })?;
                collect_rows(rows)?
            };
            tx.execute(
                "DELETE FROM image_generation_request_logs
                 WHERE created_at < ?1",
                params![log_cutoff_ts],
            )
            .map_err(|e| {
                AppError::Internal(format!("delete stale image request logs failed: {e}"))
            })?;

            tx.execute(
                "DELETE FROM request_nonces
                 WHERE created_at < ?1",
                params![(Utc::now() - Duration::seconds(NONCE_RETENTION_SECS)).to_rfc3339()],
            )
            .map_err(|e| AppError::Internal(format!("delete stale request nonces failed: {e}")))?;
            tx.execute(
                "DELETE FROM email_login_challenges
                 WHERE expires_at < ?1 OR consumed_at IS NOT NULL",
                params![cutoff],
            )
            .map_err(|e| AppError::Internal(format!("delete stale auth challenges failed: {e}")))?;
            tx.execute(
                "DELETE FROM user_sessions
                 WHERE refresh_expires_at < ?1 OR revoked_at IS NOT NULL",
                params![cutoff],
            )
            .map_err(|e| AppError::Internal(format!("delete stale user sessions failed: {e}")))?;

            let mut stmt = tx
                .prepare("SELECT subdomain FROM router_markets WHERE status = 'active'")
                .map_err(|e| {
                    AppError::Internal(format!("prepare active market cleanup failed: {e}"))
                })?;
            let active_market_subdomains = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| {
                    AppError::Internal(format!("query active market cleanup failed: {e}"))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    AppError::Internal(format!("read active market cleanup failed: {e}"))
                })?;
            drop(stmt);
            for subdomain in active_market_subdomains {
                if active_subdomains.contains(&subdomain) {
                    tx.execute(
                        "UPDATE router_markets
                         SET last_seen_at = ?2, updated_at = ?2, offline_since = NULL
                         WHERE subdomain = ?1 AND status = 'active'",
                        params![subdomain, Utc::now().to_rfc3339()],
                    )
                    .map_err(|e| AppError::Internal(format!("touch online market failed: {e}")))?;
                } else {
                    tx.execute(
                        "UPDATE router_markets
                         SET status = 'offline', offline_since = COALESCE(offline_since, ?2), updated_at = ?2
                         WHERE subdomain = ?1 AND status = 'active' AND last_seen_at < ?3",
                        params![subdomain, Utc::now().to_rfc3339(), market_missing_cutoff],
                    )
                    .map_err(|e| AppError::Internal(format!("mark offline market failed: {e}")))?;
                }
            }
            tx.execute(
                "DELETE FROM router_markets
                 WHERE status = 'offline' AND offline_since < ?1",
                params![market_release_cutoff],
            )
            .map_err(|e| {
                AppError::Internal(format!("delete released offline markets failed: {e}"))
            })?;

            tx.commit()
                .map_err(|e| AppError::Internal(format!("commit cleanup tx failed: {e}")))?;

            (
                CleanupResult {
                    deleted_leases: deleted_leases
                        + deleted_stale_leases
                        + deleted_stale_active_offline_leases,
                    deleted_shares: deleted_stale_shares
                        + deleted_stale_active_offline_shares
                        + deleted_paused_shares
                        + deleted_old_shares,
                    deleted_installations,
                    removed_routes: 0,
                },
                stale_subdomains,
                stale_image_storage_keys,
            )
        };

        let mut removed_routes = 0;
        for subdomain in stale_subdomains {
            if active_subdomains_set.contains(&subdomain) {
                continue;
            }
            proxy.remove_route(&subdomain).await;
            removed_routes += 1;
        }
        result.removed_routes = removed_routes;

        for storage_key in stale_image_storage_keys {
            if let Some(path) = image_result_path(config, &storage_key) {
                if let Err(err) = std::fs::remove_file(&path) {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(
                            storage_key = %storage_key,
                            path = %path.display(),
                            error = %err,
                            "delete stale image result file failed"
                        );
                    }
                }
            }
        }

        Ok(result)
    }

    /// Legacy heartbeat endpoint kept for compatibility with older cc-switch
    /// clients. It updates installation presence only and no longer feeds
    /// dashboard health state.
    pub async fn record_share_heartbeat(
        &self,
        input: ShareHeartbeatRequest,
        metadata: ClientMetadata,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let installation = get_installation(&conn, &input.installation_id)?;
        let Some(installation) = installation else {
            return Err(AppError::Unauthorized("installation not found".into()));
        };
        let should_refresh_geo =
            should_refresh_installation_geo(&installation, metadata.ip.as_deref());
        touch_installation_presence(&conn, &input.installation_id, &metadata, Utc::now())?;
        drop(conn);
        if should_refresh_geo {
            self.refresh_installation_geo(&input.installation_id, &metadata.ip, false)
                .await?;
        }
        Ok(())
    }

    pub async fn list_share_route_targets(&self) -> Result<Vec<ShareRouteTarget>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT share_id, installation_id, share_name, subdomain, app_runtimes_json
                 FROM shares
                 WHERE subdomain IS NOT NULL
                   AND subdomain != ''
                   AND subdomain != '-'
                   AND share_status = 'active'
                 ORDER BY share_name ASC",
            )
            .map_err(|e| AppError::Internal(format!("prepare route targets failed: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ShareRouteTarget {
                    share_id: row.get(0)?,
                    installation_id: row.get(1)?,
                    share_name: row.get(2)?,
                    subdomain: row.get(3)?,
                    app_runtimes: parse_app_runtimes(row.get(4)?)?,
                })
            })
            .map_err(|e| AppError::Internal(format!("query route targets failed: {e}")))?;
        collect_rows(rows)
    }

    pub async fn list_client_tunnel_route_targets(
        &self,
    ) -> Result<Vec<ClientTunnelRouteTarget>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT installation_id, subdomain
                 FROM installation_client_tunnels
                 WHERE enabled = 1
                   AND subdomain IS NOT NULL
                   AND subdomain != ''
                 ORDER BY subdomain ASC",
            )
            .map_err(|e| {
                AppError::Internal(format!("prepare client tunnel targets failed: {e}"))
            })?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ClientTunnelRouteTarget {
                    installation_id: row.get(0)?,
                    subdomain: row.get(1)?,
                })
            })
            .map_err(|e| AppError::Internal(format!("query client tunnel targets failed: {e}")))?;
        collect_rows(rows)
    }

    /// True if the given subdomain is currently bound to a registered market
    /// (any status — active, offline, or disabled). Used by HTTP handlers to
    /// skip the "share landing UI" injection on market subdomains, so the
    /// market's own web app reaches the user when they hit `/`.
    pub async fn is_market_subdomain(&self, subdomain: &str) -> bool {
        if subdomain.is_empty() {
            return false;
        }
        let conn = self.conn.lock().await;
        market_subdomain_owner(&conn, subdomain)
            .map(|owner| owner.is_some())
            .unwrap_or(false)
    }

    pub async fn register_market(
        &self,
        email: &str,
        input: RegisterMarketRequest,
    ) -> Result<PublicMarketConfig, AppError> {
        let email = normalize_email(email)?;
        let subdomain = normalize_subdomain(&input.subdomain)?;
        ensure_subdomain_not_reserved_word(&subdomain)?;
        let public_base_url = input.public_base_url.trim();
        if !public_base_url.starts_with("http://") && !public_base_url.starts_with("https://") {
            return Err(AppError::BadRequest(
                "invalid market public base url".into(),
            ));
        }
        let display_name = market_display_name_from_url(public_base_url);
        let market_kind = normalize_market_kind(input.market_kind.as_deref())?;

        let conn = self.conn.lock().await;
        if let Some(existing_owner) = market_subdomain_owner(&conn, &subdomain)? {
            if existing_owner != email {
                return Err(AppError::Conflict(
                    "market subdomain is already registered".into(),
                ));
            }
        }

        let now = Utc::now().to_rfc3339();
        let pricing_json = input
            .pricing_summary
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| AppError::Internal(format!("serialize market pricing failed: {e}")))?;
        let existing_market = get_market_by_email(&conn, &email)?;
        let id = existing_market
            .as_ref()
            .map(|market| market.id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let scopes_json = serde_json::to_string(MARKET_DEFAULT_SCOPES)
            .map_err(|e| AppError::Internal(format!("serialize market scopes failed: {e}")))?;
        conn.execute(
            "INSERT INTO router_markets (
                id, display_name, email, subdomain, public_base_url, market_kind, scopes_json,
                status, listed, created_at, updated_at, last_seen_at, offline_since, pricing_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', 1, ?8, ?8, ?8, NULL, ?9)
             ON CONFLICT(email) DO UPDATE SET
                display_name = excluded.display_name,
                subdomain = excluded.subdomain,
                public_base_url = excluded.public_base_url,
                market_kind = excluded.market_kind,
                scopes_json = excluded.scopes_json,
                pricing_json = excluded.pricing_json,
                status = 'active',
                listed = 1,
                updated_at = excluded.updated_at,
                last_seen_at = excluded.last_seen_at,
                offline_since = NULL",
            params![
                id,
                display_name,
                email,
                subdomain,
                public_base_url,
                market_kind,
                scopes_json.clone(),
                now,
                pricing_json,
            ],
        )
        .map_err(|e| AppError::Internal(format!("register market failed: {e}")))?;

        Ok(PublicMarketConfig {
            id,
            display_name: display_name.to_string(),
            email,
            subdomain,
            public_base_url: public_base_url.to_string(),
            market_kind,
            status: "active".to_string(),
            maintenance_enabled: existing_market
                .as_ref()
                .map(|market| market.maintenance_enabled)
                .unwrap_or(false),
            maintenance_message: existing_market.and_then(|market| market.maintenance_message),
            pricing_summary: input.pricing_summary,
        })
    }

    pub async fn register_gateway(
        &self,
        input: RegisterGatewayRequest,
    ) -> Result<RegisterGatewayResponse, AppError> {
        let owner_email = normalize_email(&input.owner_email)?;
        let display_name = input.display_name.trim();
        if display_name.is_empty() || display_name.len() > 80 {
            return Err(AppError::BadRequest("invalid gateway display name".into()));
        }
        validate_ed25519_public_key(&input.public_key)?;
        let public_base_url = input
            .public_base_url
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        if let Some(url) = public_base_url
            && !url.starts_with("http://")
            && !url.starts_with("https://")
        {
            return Err(AppError::BadRequest(
                "invalid gateway public base url".into(),
            ));
        }
        let app_version = input
            .app_version
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let scopes_json = serde_json::to_string(GATEWAY_DEFAULT_SCOPES)
            .map_err(|e| AppError::Internal(format!("serialize gateway scopes failed: {e}")))?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let existing = get_gateway_by_public_key(&conn, &input.public_key)?;
        let id = existing
            .as_ref()
            .map(|gateway| gateway.id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let created_at = if let Some(gateway) = existing.as_ref() {
            gateway_created_at(&conn, &gateway.id)?.unwrap_or_else(|| now.clone())
        } else {
            now.clone()
        };
        conn.execute(
            "INSERT INTO router_gateways (
                id, owner_email, display_name, public_key, public_base_url,
                app_version, scopes_json, status, created_at, updated_at, last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8, ?9, ?9)
             ON CONFLICT(public_key) DO UPDATE SET
                owner_email = excluded.owner_email,
                display_name = excluded.display_name,
                public_base_url = excluded.public_base_url,
                app_version = excluded.app_version,
                scopes_json = excluded.scopes_json,
                status = 'active',
                updated_at = excluded.updated_at,
                last_seen_at = excluded.last_seen_at",
            params![
                id,
                owner_email,
                display_name,
                input.public_key,
                public_base_url.map(ToOwned::to_owned),
                app_version,
                scopes_json.clone(),
                created_at,
                now,
            ],
        )
        .map_err(|e| AppError::Internal(format!("register gateway failed: {e}")))?;

        Ok(RegisterGatewayResponse {
            gateway_id: id,
            owner_email,
            display_name: display_name.to_string(),
            status: "active".to_string(),
            scopes: GATEWAY_DEFAULT_SCOPES
                .iter()
                .map(|scope| scope.to_string())
                .collect(),
            created_at,
            last_seen_at: now,
        })
    }

    pub async fn authenticate_gateway_signed_request(
        &self,
        gateway_id: &str,
        required_scope: &str,
        action: &str,
        body_sha256_hex: &str,
        timestamp_ms: i64,
        nonce: &str,
        signature: &str,
    ) -> Result<GatewayRegistryRecord, AppError> {
        if gateway_id.trim().is_empty() || nonce.trim().is_empty() || signature.trim().is_empty() {
            return Err(AppError::Unauthorized(
                "missing gateway signature fields".into(),
            ));
        }
        let conn = self.conn.lock().await;
        let gateway = get_gateway_by_id(&conn, gateway_id)?
            .ok_or_else(|| AppError::Unauthorized("gateway is not registered".into()))?;
        if !gateway.has_scope(required_scope) {
            return Err(AppError::Unauthorized(
                "gateway scope is not allowed".into(),
            ));
        }
        verify_signed_gateway_request(
            &conn,
            &gateway.public_key,
            &gateway.id,
            action,
            body_sha256_hex,
            timestamp_ms,
            nonce,
            signature,
        )?;
        conn.execute(
            "UPDATE router_gateways SET last_seen_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), gateway.id],
        )
        .map_err(|e| AppError::Internal(format!("touch gateway failed: {e}")))?;
        Ok(gateway)
    }

    pub async fn list_gateway_shares(
        &self,
        gateway: &GatewayRegistryRecord,
        router_id: &str,
        active_subdomains: &HashSet<String>,
        inflight_by_share: &HashMap<String, usize>,
    ) -> Result<Vec<MarketShareView>, AppError> {
        self.list_market_shares(
            &gateway.owner_email,
            router_id,
            active_subdomains,
            inflight_by_share,
            true,
        )
        .await
    }

    pub async fn batch_sync_gateway_request_logs(
        &self,
        gateway: &GatewayRegistryRecord,
        input: MarketRequestLogBatchSyncRequest,
    ) -> Result<usize, AppError> {
        let market = MarketRegistryRecord {
            id: gateway.id.clone(),
            display_name: gateway.display_name.clone(),
            email: gateway.owner_email.clone(),
            subdomain: format!("gateway:{}", gateway.id),
            public_base_url: gateway.public_base_url.clone().unwrap_or_default(),
            market_kind: "gateway".to_string(),
            scopes: gateway.scopes.clone(),
            status: gateway.status.clone(),
            maintenance_enabled: false,
            maintenance_message: None,
        };
        self.batch_sync_market_request_logs(&market, input).await
    }

    pub async fn list_public_markets(&self) -> Result<Vec<PublicMarketConfig>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, display_name, email, subdomain, public_base_url,
                        COALESCE(market_kind, 'usage'), status,
                        COALESCE(maintenance_enabled, 0), maintenance_message, pricing_json
                 FROM router_markets
                 WHERE status = 'active' AND listed = 1
                 ORDER BY display_name ASC, subdomain ASC",
            )
            .map_err(|e| AppError::Internal(format!("prepare public markets failed: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PublicMarketConfig {
                    id: row.get(0)?,
                    display_name: row.get(1)?,
                    email: row.get(2)?,
                    subdomain: row.get(3)?,
                    public_base_url: row.get(4)?,
                    market_kind: row.get(5)?,
                    status: row.get(6)?,
                    maintenance_enabled: row.get::<_, i64>(7)? != 0,
                    maintenance_message: row.get(8)?,
                    pricing_summary: parse_json_value(row.get(9)?)?,
                })
            })
            .map_err(|e| AppError::Internal(format!("query public markets failed: {e}")))?;
        collect_rows(rows)
    }

    pub async fn authenticate_market_session(
        &self,
        access_token: &str,
        required_scope: &str,
    ) -> Result<MarketRegistryRecord, AppError> {
        let session = self
            .resolve_session_by_access_token(access_token)
            .await?
            .ok_or_else(|| AppError::Unauthorized("invalid market session".into()))?;
        let conn = self.conn.lock().await;
        let market = get_market_by_email(&conn, &session.email)?
            .ok_or_else(|| AppError::Unauthorized("market is not registered".into()))?;
        if !market.has_scope(required_scope) {
            return Err(AppError::Unauthorized("market scope is not allowed".into()));
        }
        Ok(market)
    }

    pub async fn send_market_notification_email(
        &self,
        config: &Config,
        resend: Option<&Resend>,
        market: &MarketRegistryRecord,
        input: crate::models::MarketNotificationEmailRequest,
    ) -> Result<crate::models::MarketNotificationEmailResponse, AppError> {
        let to = normalize_email(&input.to)?;
        let kind = normalize_market_notification_kind(&input.kind)?;
        let locale = normalize_market_notification_locale(input.locale.as_deref());
        let payload = validate_market_notification_payload(&kind, &input.data)?;
        let resend = resend.ok_or_else(|| AppError::Internal("resend is not configured".into()))?;
        // 渲染恒用英文，与 send_login_code_email 保持一致。`locale` 仍写入 DB（见
        // market_notification_emails.locale 列），便于审计客户端原始请求。
        let subject = market_notification_subject(&kind);
        let html = render_market_notification_html(&kind, market, &payload);
        let provider_message_id =
            send_market_template_email(resend, config, &to, &subject, &html).await?;
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO market_notification_emails (id, market_email, kind, to_email, locale, payload_json, provider_message_id, status, error_message, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'sent', NULL, ?8)",
            params![
                id,
                market.email,
                kind,
                to,
                locale,
                serde_json::to_string(&payload).map_err(|e| AppError::Internal(format!("serialize market notification payload failed: {e}")))?,
                provider_message_id,
                now,
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert market notification log failed: {e}")))?;
        Ok(crate::models::MarketNotificationEmailResponse {
            ok: true,
            message_id: id,
            kind,
            to,
        })
    }

    pub async fn list_market_notification_emails(
        &self,
        market_email: &str,
    ) -> Result<Vec<crate::models::MarketNotificationEmailLogView>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, market_email, kind, to_email, locale, status, provider_message_id, error_message, created_at FROM market_notification_emails WHERE market_email = ?1 ORDER BY created_at DESC LIMIT 100",
            )
            .map_err(|e| AppError::Internal(format!("prepare market notification logs failed: {e}")))?;
        let rows = stmt
            .query_map(params![market_email], |row| {
                Ok(crate::models::MarketNotificationEmailLogView {
                    id: row.get(0)?,
                    market_email: row.get(1)?,
                    kind: row.get(2)?,
                    to_email: row.get(3)?,
                    locale: row.get(4)?,
                    status: row.get(5)?,
                    provider_message_id: row.get(6)?,
                    error_message: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })
            .map_err(|e| {
                AppError::Internal(format!("query market notification logs failed: {e}"))
            })?;
        collect_rows(rows)
    }

    pub async fn batch_sync_market_request_logs(
        &self,
        market: &MarketRegistryRecord,
        input: MarketRequestLogBatchSyncRequest,
    ) -> Result<usize, AppError> {
        if input.logs.len() > 500 {
            return Err(AppError::BadRequest("too many market request logs".into()));
        }
        let conn = self.conn.lock().await;
        let tx = conn.unchecked_transaction().map_err(|e| {
            AppError::Internal(format!("begin market request log sync tx failed: {e}"))
        })?;
        let mut count = 0;
        for log in input.logs {
            validate_market_request_log(&log)?;
            record_market_share_model_failure_state_conn(&tx, &market.email, &log)?;
            upsert_market_request_log_tx(&tx, market, log)?;
            count += 1;
        }
        tx.commit().map_err(|e| {
            AppError::Internal(format!("commit market request log sync failed: {e}"))
        })?;
        Ok(count)
    }

    pub async fn list_market_shares(
        &self,
        market_email: &str,
        router_id: &str,
        active_subdomains: &HashSet<String>,
        inflight_by_share: &HashMap<String, usize>,
        allow_all_market_access: bool,
    ) -> Result<Vec<MarketShareView>, AppError> {
        self.list_market_shares_with_signal_app(
            market_email,
            router_id,
            active_subdomains,
            inflight_by_share,
            allow_all_market_access,
            None,
        )
        .await
    }

    async fn list_market_shares_with_signal_app(
        &self,
        market_email: &str,
        router_id: &str,
        active_subdomains: &HashSet<String>,
        inflight_by_share: &HashMap<String, usize>,
        allow_all_market_access: bool,
        signal_app: Option<&str>,
    ) -> Result<Vec<MarketShareView>, AppError> {
        let conn = self.conn.lock().await;
        let online_minutes = list_online_minutes_24h(&conn)?;
        let samples_10m = list_online_minutes_10m(&conn)?;
        let model_health_by_share = list_model_health_summaries(&conn)?;
        let market_email_lower = market_email.to_ascii_lowercase();
        let market_availability_by_share =
            list_market_app_availability(&conn, &market_email_lower)?;
        let mut stmt = conn
            .prepare(
                "SELECT s.share_id, s.installation_id, s.share_name, s.owner_email,
                        i.owner_email, s.shared_with_emails_json, COALESCE(s.access_by_app_json, '{}'),
                        COALESCE(s.app_settings_json, '{}'), s.market_access_mode, s.app_type, s.for_sale, s.sale_market_kind,
                        s.share_status, COALESCE(s.subdomain, ''), s.parallel_limit,
                        i.last_seen_at, s.enabled_claude, s.enabled_codex, s.enabled_gemini,
                        s.upstream_provider_json, s.app_runtimes_json,
                        mds.created_at, s.created_at,
                        s.token_limit, s.tokens_used, s.requests_count, s.expires_at
                 FROM shares s
                 LEFT JOIN installations i ON i.id = s.installation_id
                 LEFT JOIN market_disabled_shares mds
                   ON lower(mds.market_email) = ?1 AND mds.share_id = s.share_id
                 WHERE s.share_status = 'active'
                   AND s.subdomain IS NOT NULL
                   AND s.subdomain != ''
                   AND s.subdomain != '-'
                 ORDER BY s.share_name ASC",
            )
            .map_err(|e| AppError::Internal(format!("prepare market shares failed: {e}")))?;
        let now = Utc::now();
        let rows = stmt
            .query_map(params![market_email_lower], |row| {
                let share_id: String = row.get(0)?;
                let shared_with_emails = parse_string_vec(row.get(5)?)?;
                let access_by_app = parse_share_access_by_app(row.get(6)?)?;
                let app_settings = parse_share_app_settings(row.get(7)?)?;
                let subdomain: String = row.get(13)?;
                let parallel_limit: i64 = row.get(14)?;
                let active_requests = *inflight_by_share.get(&share_id).unwrap_or(&0);
                let online_rate_24h = online_minutes
                    .get(&share_id)
                    .map(|minutes| *minutes as f64 / ONLINE_WINDOW_MINUTES as f64)
                    .unwrap_or(0.0);
                let samples = samples_10m.get(&share_id).copied().unwrap_or(0);
                let upstream_provider: Option<ShareUpstreamProvider> =
                    parse_upstream_provider(row.get(19)?)?;
                let model_health = model_health_by_share
                    .get(&share_id)
                    .cloned()
                    .unwrap_or_default();
                let raw_app_runtimes = parse_app_runtimes(row.get(20)?)?;
                let available_app_runtimes = filter_app_runtimes_by_quota(
                    filter_app_runtimes_by_model_health(
                        raw_app_runtimes.clone(),
                        &model_health,
                        now,
                    ),
                    now,
                );
                let app_runtimes = if allow_all_market_access {
                    available_app_runtimes
                } else {
                    raw_app_runtimes.clone()
                };
                let quota_health = compute_market_share_quota_health(
                    signal_app,
                    &raw_app_runtimes,
                    upstream_provider.as_ref(),
                    now,
                );
                let stability =
                    crate::scheduling_signals::compute_stability(samples, online_rate_24h);
                let headroom =
                    crate::scheduling_signals::compute_headroom(active_requests, parallel_limit);
                let mut app_availability = market_availability_by_share
                    .get(&share_id)
                    .cloned()
                    .unwrap_or_default();
                apply_quota_blocks_to_app_availability(
                    &mut app_availability,
                    &raw_app_runtimes,
                    &model_health,
                    now,
                );
                let market_failure_penalty = market_app_availability_penalty(&app_availability);
                Ok((
                    shared_with_emails,
                    access_by_app,
                    subdomain.clone(),
                    MarketShareView {
                        router_id: router_id.to_string(),
                        share_id: share_id.clone(),
                        subdomain: subdomain.clone(),
                        installation_id: row.get(1)?,
                        share_name: row.get(2)?,
                        owner_email: row.get(3)?,
                        installation_owner_email: row.get(4)?,
                        market_access_mode: row.get(8)?,
                        app_type: row.get(9)?,
                        for_sale: row.get(10)?,
                        sale_market_kind: row.get(11)?,
                        share_status: row.get(12)?,
                        online: false,
                        active_requests,
                        token_limit: row.get(23)?,
                        tokens_used: row.get(24)?,
                        requests_count: row.get(25)?,
                        parallel_limit,
                        expires_at: row.get(26)?,
                        online_rate_24h,
                        last_seen_at: row.get(15)?,
                        share_created_at: row.get(22)?,
                        disabled_by_market: row.get::<_, Option<String>>(21)?.is_some(),
                        market_disabled_at: row.get(21)?,
                        support: ShareSupport {
                            claude: row.get::<_, i64>(16)? != 0,
                            codex: row.get::<_, i64>(17)? != 0,
                            gemini: row.get::<_, i64>(18)? != 0,
                        },
                        upstream_provider,
                        app_runtimes,
                        model_health,
                        app_availability,
                        market_apps: BTreeMap::new(),
                        market_states: Vec::new(),
                        signals: ShareSignals {
                            quota_health,
                            stability,
                            headroom,
                            samples_10m: samples as u32,
                            owner_penalty: market_failure_penalty,
                        },
                    },
                    app_settings,
                ))
            })
            .map_err(|e| AppError::Internal(format!("query market shares failed: {e}")))?;

        let mut shares = Vec::new();
        for row in rows {
            let (shared_with_emails, access_by_app, subdomain, mut share, app_settings) =
                row.map_err(|e| AppError::Internal(format!("read market share row failed: {e}")))?;
            let expected_sale_market_kind = if allow_all_market_access {
                "token"
            } else {
                "share"
            };
            share.market_apps = build_market_share_apps(
                &share.support,
                &app_settings,
                &access_by_app,
                &shared_with_emails,
                &share.market_access_mode,
                &share.for_sale,
                &share.sale_market_kind,
                expected_sale_market_kind,
                allow_all_market_access,
                market_email,
            );
            let candidate_apps = match signal_app {
                Some(app) => vec![app],
                None => vec!["claude", "codex", "gemini"],
            };
            let authorized = candidate_apps.into_iter().any(|app| {
                share
                    .market_apps
                    .get(app)
                    .is_some_and(|entry| entry.supported && entry.visible)
            });
            if !authorized {
                continue;
            }
            share.online = active_subdomains.contains(&subdomain);
            shares.push(share);
        }
        Ok(shares)
    }

    pub async fn list_share_market_delegated_shares(
        &self,
        market_email: &str,
        router_id: &str,
        active_subdomains: &HashSet<String>,
        inflight_by_share: &HashMap<String, usize>,
    ) -> Result<Vec<MarketShareView>, AppError> {
        self.list_market_shares(
            market_email,
            router_id,
            active_subdomains,
            inflight_by_share,
            false,
        )
        .await
    }

    pub async fn create_share_market_grant(
        &self,
        market_email: &str,
        share_id: &str,
        input: ShareMarketGrantRequest,
    ) -> Result<ShareMarketGrantResponse, AppError> {
        let market_email = normalize_email(market_email)?;
        let action = input.action.trim().to_ascii_lowercase();
        if !matches!(action.as_str(), "add" | "revoke") {
            return Err(AppError::BadRequest(
                "grant action must be add or revoke".into(),
            ));
        }
        if input.grant_id.trim().is_empty() {
            return Err(AppError::BadRequest("grantId is required".into()));
        }

        let conn = self.conn.lock().await;
        let (
            installation_id,
            owner_email,
            shared_with_emails_json,
            access_by_app_json,
            app_settings_json,
            market_access_mode,
            for_sale,
            sale_market_kind,
            token_limit,
            parallel_limit,
            expires_at,
        ): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            String,
        ) = conn
            .query_row(
                "SELECT installation_id, owner_email, shared_with_emails_json, COALESCE(access_by_app_json, '{}'), COALESCE(app_settings_json, '{}'), market_access_mode, for_sale, sale_market_kind, token_limit, parallel_limit, expires_at
                 FROM shares WHERE share_id = ?1 AND share_status = 'active'",
                params![share_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("query delegated share failed: {e}")))?
            .ok_or_else(|| AppError::NotFound("share not found".into()))?;
        let owner_email = normalize_email(&owner_email)?;
        let current_shared = parse_string_vec(Some(shared_with_emails_json))
            .map_err(|e| AppError::Internal(format!("parse share acl failed: {e}")))?;
        let access_by_app = parse_share_access_by_app(Some(access_by_app_json))
            .map_err(|e| AppError::Internal(format!("parse share app acl failed: {e}")))?;
        let app_settings = parse_share_app_settings(Some(app_settings_json))
            .map_err(|e| AppError::Internal(format!("parse share app settings failed: {e}")))?;
        let grant_app = match input.app_type.as_deref() {
            Some(app) if !app.trim().is_empty() => Some(normalize_share_acl_app(app)?),
            _ => None,
        };
        let candidate_apps: Vec<&str> = match grant_app.as_deref() {
            Some(app) => vec![app],
            None => vec!["claude", "codex", "gemini"],
        };
        let listed_for_share_market = candidate_apps.iter().any(|app| {
            let setting = app_settings.get(*app);
            let app_for_sale = setting
                .map(|value| value.for_sale.as_str())
                .unwrap_or(&for_sale);
            let app_sale_market_kind = setting
                .map(|value| value.sale_market_kind.as_str())
                .unwrap_or(&sale_market_kind);
            app_for_sale == "Yes" && app_sale_market_kind == "share"
        });
        if !listed_for_share_market {
            return Err(AppError::Forbidden(
                "share is not listed for share-market sale".into(),
            ));
        }
        let delegated = candidate_apps.iter().any(|app| {
            share_app_settings_visible_to_market(
                app,
                &app_settings,
                &access_by_app,
                &current_shared,
                &market_access_mode,
                &for_sale,
                &sale_market_kind,
                "share",
                false,
                &market_email,
            )
        });
        if !delegated {
            return Err(AppError::Forbidden(
                "share is not delegated to this share-market".into(),
            ));
        }
        if let Some(active) = get_active_share_edit(&conn, share_id)? {
            return Err(AppError::Conflict(format!(
                "share settings edit {} is {} and must be applied before another edit",
                active.revision, active.status
            )));
        }

        let buyer_emails = normalize_email_list(&input.buyer_emails, &owner_email);
        if buyer_emails.is_empty() {
            return Err(AppError::BadRequest("buyerEmails is required".into()));
        }
        let patch = if let Some(app) = grant_app.as_deref() {
            let mut next_app_settings = effective_app_settings_from_parts(
                &app_settings,
                &access_by_app,
                &current_shared,
                &market_access_mode,
                &for_sale,
                &sale_market_kind,
                token_limit,
                parallel_limit,
                &expires_at,
            );
            let current_setting = next_app_settings.get(app).cloned().unwrap_or_else(|| {
                effective_app_setting_from_parts(
                    app,
                    &app_settings,
                    &access_by_app,
                    &current_shared,
                    &market_access_mode,
                    &for_sale,
                    &sale_market_kind,
                    token_limit,
                    parallel_limit,
                    &expires_at,
                )
            });
            let mut next_emails = normalize_email_list_with_options(
                &current_setting.shared_with_emails,
                &owner_email,
                true,
            );
            match action.as_str() {
                "add" => {
                    next_emails.extend(buyer_emails);
                    next_emails =
                        normalize_email_list_with_options(&next_emails, &owner_email, true);
                }
                "revoke" => {
                    let revoke = buyer_emails.into_iter().collect::<HashSet<_>>();
                    next_emails.retain(|email| !revoke.contains(email));
                }
                _ => unreachable!(),
            }
            if next_emails
                == normalize_email_list_with_options(
                    &current_setting.shared_with_emails,
                    &owner_email,
                    true,
                )
            {
                return Ok(ShareMarketGrantResponse {
                    ok: true,
                    grant_id: input.grant_id,
                    router_edit_id: format!("noop:{share_id}:{app}"),
                    status: "noop".into(),
                });
            }
            let mut next_setting = current_setting;
            next_setting.shared_with_emails = next_emails;
            next_app_settings.insert(app.to_string(), next_setting);
            normalize_share_settings_patch(
                ShareSettingsPatch {
                    app_settings: Some(next_app_settings),
                    ..Default::default()
                },
                Some(&owner_email),
                Some(&current_shared),
                Some(&sale_market_kind),
            )?
        } else {
            let mut next_shared = normalize_email_list(&current_shared, &owner_email);
            match action.as_str() {
                "add" => {
                    next_shared.extend(buyer_emails);
                    next_shared = normalize_email_list(&next_shared, &owner_email);
                }
                "revoke" => {
                    let revoke = buyer_emails.into_iter().collect::<HashSet<_>>();
                    next_shared.retain(|email| !revoke.contains(email));
                }
                _ => unreachable!(),
            }
            if next_shared == normalize_email_list(&current_shared, &owner_email) {
                return Ok(ShareMarketGrantResponse {
                    ok: true,
                    grant_id: input.grant_id,
                    router_edit_id: format!("noop:{share_id}"),
                    status: "noop".into(),
                });
            }

            normalize_share_settings_patch(
                ShareSettingsPatch {
                    shared_with_emails: Some(next_shared),
                    ..Default::default()
                },
                Some(&owner_email),
                Some(&current_shared),
                Some(&sale_market_kind),
            )?
        };
        let revision = next_share_edit_revision(&conn, share_id)?;
        let edit_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let patch_json = serde_json::to_string(&patch).map_err(|e| {
            AppError::Internal(format!("serialize share-market grant patch failed: {e}"))
        })?;
        conn.execute(
            "INSERT INTO share_edit_requests (
                id, share_id, installation_id, owner_email, revision, status, patch_json,
                created_by_email, created_at, updated_at, applied_at, error_message
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?8, NULL, NULL)",
            params![
                edit_id,
                share_id,
                installation_id,
                owner_email,
                revision,
                patch_json,
                market_email,
                now,
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert share-market grant edit failed: {e}")))?;
        Ok(ShareMarketGrantResponse {
            ok: true,
            grant_id: input.grant_id,
            router_edit_id: edit_id,
            status: "pending".into(),
        })
    }

    pub async fn share_market_grant_status(
        &self,
        market_email: &str,
        share_id: &str,
        router_edit_id: &str,
    ) -> Result<ShareMarketGrantStatusResponse, AppError> {
        let market_email = normalize_email(market_email)?;
        if router_edit_id.starts_with("noop:") {
            let noop_share_id = router_edit_id.trim_start_matches("noop:");
            if noop_share_id != share_id {
                return Err(AppError::NotFound("share grant edit not found".into()));
            }
            return Ok(ShareMarketGrantStatusResponse {
                ok: true,
                router_edit_id: router_edit_id.to_string(),
                status: "applied".into(),
                error_message: None,
                applied_at: Some(Utc::now()),
            });
        }

        let conn = self.conn.lock().await;
        let edit = get_share_edit_by_id(&conn, router_edit_id)?
            .ok_or_else(|| AppError::NotFound("share grant edit not found".into()))?;
        if edit.share_id != share_id || normalize_email(&edit.created_by_email)? != market_email {
            return Err(AppError::NotFound("share grant edit not found".into()));
        }
        Ok(ShareMarketGrantStatusResponse {
            ok: true,
            router_edit_id: router_edit_id.to_string(),
            status: edit.status,
            error_message: edit.error_message,
            applied_at: edit.applied_at,
        })
    }

    /// Batched lookup of `parallel_limit` for a set of share_ids. Backs the
    /// real-time headroom probe so the market doesn't depend on the 30s-stale
    /// snapshot for limits either.
    pub async fn share_parallel_limits(
        &self,
        share_ids: &HashSet<String>,
    ) -> Result<HashMap<String, i64>, AppError> {
        if share_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let conn = self.conn.lock().await;
        // Use a single IN-clause query; rusqlite doesn't support array params,
        // so build a placeholder list. The batch is capped by the caller.
        let placeholders: Vec<&str> = share_ids.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT share_id, parallel_limit FROM shares WHERE share_id IN ({})",
            placeholders.join(",")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AppError::Internal(format!("prepare parallel_limits failed: {e}")))?;
        let params: Vec<&dyn rusqlite::ToSql> = share_ids
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt
            .query_map(params.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| AppError::Internal(format!("query parallel_limits failed: {e}")))?;
        let mut out = HashMap::new();
        for r in rows {
            let (id, limit) =
                r.map_err(|e| AppError::Internal(format!("read parallel_limit row failed: {e}")))?;
            out.insert(id, limit);
        }
        Ok(out)
    }

    /// Look up the share owner email by share_id. Used by the feedback endpoint
    /// to scope 429 penalties to all shares of the same owner.
    pub async fn lookup_share_owner_email(
        &self,
        share_id: &str,
    ) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().await;
        let owner: Option<String> = conn
            .query_row(
                "SELECT s.owner_email FROM shares s WHERE s.share_id = ?1",
                params![share_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|e| AppError::Internal(format!("lookup share owner failed: {e}")))?
            .flatten();
        Ok(owner)
    }

    pub async fn list_manageable_market_shares(
        &self,
        market_email: &str,
        current_user_email: &str,
        is_admin: bool,
        active_subdomains: &HashSet<String>,
        inflight_by_share: &HashMap<String, usize>,
    ) -> Result<Vec<MarketShareView>, AppError> {
        self.require_market_manager(market_email, current_user_email, is_admin)
            .await?;
        let mut shares = self
            .list_market_shares(
                market_email,
                "main",
                active_subdomains,
                inflight_by_share,
                true,
            )
            .await?;
        let conn = self.conn.lock().await;
        let states_by_market = list_market_share_runtime_state_map(&conn)?;
        let states_by_share = states_by_market
            .get(&normalize_email(market_email)?)
            .cloned()
            .unwrap_or_default();
        for share in &mut shares {
            share.market_states = states_by_share
                .get(&share.share_id)
                .cloned()
                .unwrap_or_default();
        }
        Ok(shares)
    }

    pub async fn list_public_market_share_priority(
        &self,
        market_email: &str,
        app: Option<&str>,
        active_subdomains: &HashSet<String>,
        inflight_by_share: &HashMap<String, usize>,
    ) -> Result<Vec<MarketShareView>, AppError> {
        let normalized_market_email = normalize_email(market_email)?;
        let signal_app = app.map(normalize_market_share_priority_app).transpose()?;
        {
            let conn = self.conn.lock().await;
            let market = get_market_by_email(&conn, &normalized_market_email)?
                .ok_or_else(|| AppError::NotFound("market not found".into()))?;
            if market.market_kind == "share" {
                return Err(AppError::BadRequest(
                    "share priority is only available for token markets".into(),
                ));
            }
        }

        let mut shares = self
            .list_market_shares_with_signal_app(
                &normalized_market_email,
                "main",
                active_subdomains,
                inflight_by_share,
                true,
                signal_app.as_deref(),
            )
            .await?;
        let conn = self.conn.lock().await;
        let states_by_market = list_market_share_runtime_state_map(&conn)?;
        let states_by_share = states_by_market
            .get(&normalized_market_email)
            .cloned()
            .unwrap_or_default();
        for share in &mut shares {
            share.market_states = states_by_share
                .get(&share.share_id)
                .cloned()
                .unwrap_or_default();
        }
        if let Some(app) = signal_app.as_deref() {
            sort_market_shares_for_app(&mut shares, app);
        }
        for share in &mut shares {
            share.installation_id.clear();
            share.installation_owner_email = None;
            share.upstream_provider = None;
            share.app_runtimes = Default::default();
            share.model_health = Default::default();
        }
        Ok(shares)
    }

    pub async fn ensure_market_manager(
        &self,
        market_email: &str,
        current_user_email: &str,
        is_admin: bool,
    ) -> Result<MarketRegistryRecord, AppError> {
        self.require_market_manager(market_email, current_user_email, is_admin)
            .await
    }

    pub async fn update_market_disabled_shares(
        &self,
        market_email: &str,
        current_user_email: &str,
        is_admin: bool,
        input: MarketDisabledSharesUpdateRequest,
    ) -> Result<MarketDisabledSharesUpdateResponse, AppError> {
        self.require_market_manager(market_email, current_user_email, is_admin)
            .await?;
        let normalized_market_email = normalize_email(market_email)?;
        let mut disabled_share_ids = input
            .disabled_share_ids
            .into_iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect::<Vec<_>>();
        disabled_share_ids.sort();
        disabled_share_ids.dedup();

        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let visible_share_ids = list_market_visible_share_ids(&conn, &normalized_market_email)?;
        for share_id in &disabled_share_ids {
            if !visible_share_ids.contains(share_id) {
                return Err(AppError::BadRequest(format!(
                    "share is not linked to market: {share_id}"
                )));
            }
        }

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Internal(format!("begin disabled shares update failed: {e}")))?;
        tx.execute(
            "DELETE FROM market_disabled_shares WHERE lower(market_email) = lower(?1)",
            params![normalized_market_email],
        )
        .map_err(|e| AppError::Internal(format!("clear disabled market shares failed: {e}")))?;
        for share_id in &disabled_share_ids {
            tx.execute(
                "INSERT INTO market_disabled_shares (market_email, share_id, disabled_by_email, reason, created_at, updated_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?4)",
                params![normalized_market_email, share_id, current_user_email, now],
            )
            .map_err(|e| AppError::Internal(format!("insert disabled market share failed: {e}")))?;
        }
        tx.commit().map_err(|e| {
            AppError::Internal(format!("commit disabled shares update failed: {e}"))
        })?;

        Ok(MarketDisabledSharesUpdateResponse {
            ok: true,
            disabled_share_ids,
        })
    }

    pub async fn sync_market_share_runtime_states(
        &self,
        market_email: &str,
        replace: bool,
        states: Vec<MarketShareRuntimeStateInput>,
    ) -> Result<usize, AppError> {
        let normalized_market_email = normalize_email(market_email)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn.unchecked_transaction().map_err(|e| {
            AppError::Internal(format!("begin market share states sync failed: {e}"))
        })?;
        tx.execute(
            "DELETE FROM market_share_runtime_states WHERE expires_at IS NOT NULL AND expires_at <= ?1",
            params![now],
        )
        .map_err(|e| AppError::Internal(format!("delete expired market share states failed: {e}")))?;
        if replace {
            tx.execute(
                "DELETE FROM market_share_runtime_states WHERE lower(market_email) = lower(?1)",
                params![normalized_market_email],
            )
            .map_err(|e| AppError::Internal(format!("replace market share states failed: {e}")))?;
        }

        let mut synced = 0usize;
        for state in states {
            let share_id = state.share_id.trim();
            if share_id.is_empty() {
                continue;
            }
            let scope = normalize_runtime_state_token(&state.scope, "scope")?;
            let kind = normalize_runtime_state_token(&state.kind, "kind")?;
            let router_id = normalize_optional_string(state.router_id);
            let app_type = normalize_optional_string(state.app_type);
            let model_id = normalize_optional_string(state.model_id);
            let model_name = normalize_optional_string(state.model_name);
            let reason_kind = normalize_optional_string(state.reason_kind);
            let reason = normalize_optional_string(state.reason).map(|value| {
                if value.chars().count() > 500 {
                    value.chars().take(500).collect()
                } else {
                    value
                }
            });
            let expires_at = normalize_optional_string(state.expires_at);
            let failure_count = state.failure_count.map(|value| value.max(0));

            tx.execute(
                r#"
                DELETE FROM market_share_runtime_states
                 WHERE lower(market_email) = lower(?1)
                   AND share_id = ?2
                   AND scope = ?3
                   AND kind = ?4
                   AND COALESCE(app_type, '') = COALESCE(?5, '')
                   AND COALESCE(model_id, '') = COALESCE(?6, '')
                   AND COALESCE(model_name, '') = COALESCE(?7, '')
                "#,
                params![
                    normalized_market_email,
                    share_id,
                    scope,
                    kind,
                    app_type,
                    model_id,
                    model_name
                ],
            )
            .map_err(|e| AppError::Internal(format!("delete market share state failed: {e}")))?;

            if expires_at.as_ref().is_some_and(|value| value <= &now) {
                synced += 1;
                continue;
            }

            tx.execute(
                r#"
                INSERT INTO market_share_runtime_states
                    (market_email, share_id, router_id, scope, kind, app_type, model_id, model_name,
                     reason_kind, reason, failure_count, expires_at, created_at, updated_at)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)
                "#,
                params![
                    normalized_market_email,
                    share_id,
                    router_id,
                    scope,
                    kind,
                    app_type,
                    model_id,
                    model_name,
                    reason_kind,
                    reason,
                    failure_count,
                    expires_at,
                    now
                ],
            )
            .map_err(|e| AppError::Internal(format!("insert market share state failed: {e}")))?;
            synced += 1;
        }
        tx.commit().map_err(|e| {
            AppError::Internal(format!("commit market share states sync failed: {e}"))
        })?;
        Ok(synced)
    }

    pub async fn sync_share_market_listing_statuses(
        &self,
        market_email: &str,
        replace: bool,
        statuses: Vec<ShareMarketListingStatusInput>,
    ) -> Result<usize, AppError> {
        let normalized_market_email = normalize_email(market_email)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn.unchecked_transaction().map_err(|e| {
            AppError::Internal(format!(
                "begin share market listing status sync failed: {e}"
            ))
        })?;
        if replace {
            tx.execute(
                "DELETE FROM share_market_listing_statuses WHERE lower(market_email) = lower(?1)",
                params![normalized_market_email],
            )
            .map_err(|e| {
                AppError::Internal(format!("replace share market listing statuses failed: {e}"))
            })?;
        }

        let mut synced = 0usize;
        for status in statuses {
            let router_id =
                normalize_optional_string(status.router_id).unwrap_or_else(|| "main".to_string());
            let share_id = status.share_id.trim().to_string();
            if share_id.is_empty() {
                continue;
            }
            let app_type = normalize_app_type_token(&status.app_type)?;
            if !share_market_listing_status_visible_to_market(
                &tx,
                &normalized_market_email,
                &router_id,
                &share_id,
                &app_type,
            )? {
                continue;
            }
            let listing_url = status.listing_url.trim().to_string();
            if !listing_url.starts_with("http://") && !listing_url.starts_with("https://") {
                continue;
            }
            let status_value = normalize_listing_status_token(&status.status)?;
            let sale_mode = normalize_optional_listing_sale_mode(status.sale_mode)?;
            let listing_status = normalize_optional_string(status.listing_status);
            let expires_at = normalize_optional_string(status.expires_at);
            let filled_seats = status.filled_seats.map(|value| value.max(0));
            let required_seats = status.required_seats.map(|value| value.max(0));
            tx.execute(
                r#"
                INSERT INTO share_market_listing_statuses
                    (market_email, router_id, share_id, app_type, listing_url, status, sale_mode,
                     filled_seats, required_seats, listing_status, expires_at, created_at, updated_at)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)
                ON CONFLICT(market_email, router_id, share_id, app_type) DO UPDATE SET
                    listing_url = excluded.listing_url,
                    status = excluded.status,
                    sale_mode = excluded.sale_mode,
                    filled_seats = excluded.filled_seats,
                    required_seats = excluded.required_seats,
                    listing_status = excluded.listing_status,
                    expires_at = excluded.expires_at,
                    updated_at = excluded.updated_at
                "#,
                params![
                    normalized_market_email,
                    router_id,
                    share_id,
                    app_type,
                    listing_url,
                    status_value,
                    sale_mode,
                    filled_seats,
                    required_seats,
                    listing_status,
                    expires_at,
                    now
                ],
            )
            .map_err(|e| {
                AppError::Internal(format!("upsert share market listing status failed: {e}"))
            })?;
            synced += 1;
        }
        tx.commit().map_err(|e| {
            AppError::Internal(format!(
                "commit share market listing status sync failed: {e}"
            ))
        })?;
        Ok(synced)
    }

    pub async fn attach_share_market_listing_statuses(
        &self,
        response: &mut DashboardResponse,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let statuses = list_share_market_listing_status_map(&conn)?;
        drop(conn);
        for share in &mut response.shares {
            for market in share
                .market_links
                .iter_mut()
                .filter(|market| market.market_kind == "share")
            {
                let market_email = market.email.to_ascii_lowercase();
                for app in ["claude", "codex", "gemini"] {
                    let router_id = if share.router_id.trim().is_empty() {
                        "main"
                    } else {
                        share.router_id.as_str()
                    };
                    let key = (
                        market_email.clone(),
                        router_id.to_string(),
                        share.share_id.clone(),
                        app.to_string(),
                    );
                    if let Some(status) = statuses.get(&key) {
                        market
                            .listing_status_by_app
                            .insert(app.to_string(), status.clone());
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn update_market_maintenance(
        &self,
        market_email: &str,
        current_user_email: &str,
        is_admin: bool,
        input: MarketMaintenanceUpdateRequest,
    ) -> Result<MarketMaintenanceUpdateResponse, AppError> {
        self.require_market_manager(market_email, current_user_email, is_admin)
            .await?;
        let normalized_market_email = normalize_email(market_email)?;
        let message = input
            .maintenance_message
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if message
            .as_ref()
            .map(|value| value.chars().count() > 240)
            .unwrap_or(false)
        {
            return Err(AppError::BadRequest(
                "maintenance message is too long".into(),
            ));
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let changed = conn
            .execute(
                "UPDATE router_markets
                 SET maintenance_enabled = ?2,
                     maintenance_message = ?3,
                     updated_at = ?4
                 WHERE lower(email) = lower(?1)",
                params![
                    normalized_market_email,
                    if input.maintenance_enabled { 1 } else { 0 },
                    message,
                    now,
                ],
            )
            .map_err(|e| AppError::Internal(format!("update market maintenance failed: {e}")))?;
        if changed == 0 {
            return Err(AppError::NotFound("market not found".into()));
        }
        Ok(MarketMaintenanceUpdateResponse {
            ok: true,
            maintenance_enabled: input.maintenance_enabled,
            maintenance_message: if input.maintenance_enabled {
                message
            } else {
                None
            },
        })
    }

    async fn require_market_manager(
        &self,
        market_email: &str,
        current_user_email: &str,
        is_admin: bool,
    ) -> Result<MarketRegistryRecord, AppError> {
        let normalized_market_email = normalize_email(market_email)?;
        let conn = self.conn.lock().await;
        let market = get_market_by_email(&conn, &normalized_market_email)?
            .ok_or_else(|| AppError::NotFound("market not found".into()))?;
        if !is_admin && !market.email.eq_ignore_ascii_case(current_user_email) {
            return Err(AppError::Forbidden(
                "only router admin or market owner can manage this market".into(),
            ));
        }
        Ok(market)
    }

    pub async fn public_map_points(
        &self,
        server_geo: &ServerGeo,
    ) -> Result<PublicMapPointsResponse, AppError> {
        let active_cutoff =
            (Utc::now() - Duration::minutes(PUBLIC_MAP_CLIENT_ACTIVE_WINDOW_MINUTES)).to_rfc3339();
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT i.id, i.latitude, i.longitude, i.country_code
                 FROM installations i
                 INNER JOIN shares s ON s.installation_id = i.id
                 WHERE i.last_seen_at >= ?1
                   AND s.share_status = 'active'
                 ORDER BY i.last_seen_at DESC",
            )
            .map_err(|e| AppError::Internal(format!("prepare public map clients failed: {e}")))?;
        let rows = stmt
            .query_map(params![active_cutoff], |row| {
                let lat = row.get::<_, Option<f64>>(1)?;
                let lon = row.get::<_, Option<f64>>(2)?;
                let country_code = row.get::<_, Option<String>>(3)?;
                Ok(lat
                    .zip(lon)
                    .map(|(lat, lon)| LatLonPoint { lat, lon })
                    .or_else(|| {
                        country_code
                            .as_deref()
                            .and_then(country_centroid)
                            .map(|(lat, lon)| LatLonPoint { lat, lon })
                    }))
            })
            .map_err(|e| AppError::Internal(format!("query public map clients failed: {e}")))?;
        let mut grouped_clients = HashMap::<String, PublicMapClientPoint>::new();
        let mut client_count = 0usize;
        for point in collect_rows(rows)?.into_iter().flatten() {
            client_count += 1;
            let key = format!("{:.6},{:.6}", point.lat, point.lon);
            grouped_clients
                .entry(key)
                .and_modify(|existing| existing.count += 1)
                .or_insert(PublicMapClientPoint {
                    lat: point.lat,
                    lon: point.lon,
                    count: 1,
                });
        }
        let mut clients = grouped_clients.into_values().collect::<Vec<_>>();
        clients.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.lat.total_cmp(&b.lat))
                .then_with(|| a.lon.total_cmp(&b.lon))
        });

        Ok(PublicMapPointsResponse {
            server: server_geo
                .lat
                .zip(server_geo.lon)
                .map(|(lat, lon)| LatLonPoint { lat, lon }),
            client_count,
            clients,
        })
    }

    pub async fn public_network_stats(&self) -> Result<PublicNetworkStatsResponse, AppError> {
        let active_cutoff =
            (Utc::now() - Duration::minutes(PUBLIC_MAP_CLIENT_ACTIVE_WINDOW_MINUTES)).to_rfc3339();
        let conn = self.conn.lock().await;
        let active_shares: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE share_status = 'active'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count active shares failed: {e}")))?;
        let active_clients: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT i.id)
                 FROM installations i
                 INNER JOIN shares s ON s.installation_id = i.id
                 WHERE i.last_seen_at >= ?1
                   AND s.share_status = 'active'",
                params![active_cutoff],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count active clients failed: {e}")))?;
        Ok(PublicNetworkStatsResponse {
            active_shares: active_shares.max(0) as usize,
            active_clients: active_clients.max(0) as usize,
        })
    }

    pub async fn record_share_route_health(
        &self,
        share_id: &str,
        is_healthy: bool,
    ) -> Result<(), AppError> {
        let now = Utc::now().timestamp();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO share_health_checks (share_id, checked_at, is_healthy) VALUES (?1, ?2, ?3)",
            params![share_id, now, if is_healthy { 1 } else { 0 }],
        )
        .map_err(|e| AppError::Internal(format!("insert route health failed: {e}")))?;
        conn.execute(
            "DELETE FROM share_health_checks WHERE checked_at < ?1",
            params![now - 86_400],
        )
        .map_err(|e| AppError::Internal(format!("prune route health failed: {e}")))?;
        Ok(())
    }

    pub async fn record_installation_route_health(
        &self,
        installation_id: &str,
        is_healthy: bool,
    ) -> Result<(), AppError> {
        let now = Utc::now().timestamp();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO installation_health_checks (installation_id, checked_at, is_healthy) VALUES (?1, ?2, ?3)",
            params![installation_id, now, if is_healthy { 1 } else { 0 }],
        )
        .map_err(|e| AppError::Internal(format!("insert installation route health failed: {e}")))?;
        conn.execute(
            "DELETE FROM installation_health_checks WHERE checked_at < ?1",
            params![now - 86_400],
        )
        .map_err(|e| AppError::Internal(format!("prune installation route health failed: {e}")))?;
        Ok(())
    }

    pub async fn record_share_model_health_check(
        &self,
        check: ShareModelHealthCheckEntry,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        record_share_model_health_check_conn(&conn, &check)?;
        Ok(())
    }

    pub async fn clear_share_model_health_checks(&self) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM share_model_health_checks", [])
            .map_err(|e| AppError::Internal(format!("clear model health checks failed: {e}")))?;
        conn.execute("DELETE FROM share_model_health_state", [])
            .map_err(|e| AppError::Internal(format!("clear model health state failed: {e}")))?;
        Ok(())
    }

    pub async fn record_share_health_request_log(
        &self,
        installation_id: &str,
        log: ShareRequestLogEntry,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        upsert_share_request_log_tx(&conn, installation_id, log)
    }

    pub async fn record_share_runtime_snapshot(
        &self,
        snapshot: ShareRuntimeSnapshotResponse,
    ) -> Result<(), AppError> {
        let app_runtimes_json = serde_json::to_string(&snapshot.app_runtimes)
            .map_err(|e| AppError::Internal(format!("serialize app runtimes failed: {e}")))?;
        let app_providers_json = serde_json::to_string(&snapshot.app_providers)
            .map_err(|e| AppError::Internal(format!("serialize app providers failed: {e}")))?;
        let refreshed_at = DateTime::<Utc>::from_timestamp(snapshot.queried_at, 0)
            .unwrap_or_else(Utc::now)
            .to_rfc3339();

        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE shares
             SET enabled_claude = ?2,
                 enabled_codex = ?3,
                 enabled_gemini = ?4,
                 app_runtimes_json = ?5,
                 app_providers_json = ?6,
                 runtime_refreshed_at = ?7,
                 token_limit = COALESCE(?8, token_limit),
                 tokens_used = COALESCE(?9, tokens_used),
                 requests_count = COALESCE(?10, requests_count),
                 share_status = COALESCE(?11, share_status),
                 updated_at = ?12
             WHERE share_id = ?1",
            params![
                snapshot.share_id,
                i64::from(snapshot.support.claude as u8),
                i64::from(snapshot.support.codex as u8),
                i64::from(snapshot.support.gemini as u8),
                app_runtimes_json,
                app_providers_json,
                refreshed_at,
                snapshot.token_limit,
                snapshot.tokens_used,
                snapshot.requests_count,
                snapshot.share_status,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("update share runtime snapshot failed: {e}")))?;
        record_runtime_model_health_snapshot_conn(&conn, &snapshot)?;
        Ok(())
    }

    async fn upsert_share(
        &self,
        installation_id: &str,
        mut share: ShareDescriptor,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let existing_subdomain =
            get_share_owned_subdomain(&conn, installation_id, &share.share_id)?
                .ok_or_else(|| AppError::Conflict("share subdomain is not claimed".into()))?;
        share.subdomain = existing_subdomain;
        upsert_share_tx(&conn, installation_id, share)?;
        Ok(())
    }

    async fn refresh_installation_geo(
        &self,
        installation_id: &str,
        ip: &Option<String>,
        force: bool,
    ) -> Result<(), AppError> {
        let Some(ip) = ip.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
            return Ok(());
        };
        let current_state = {
            let conn = self.conn.lock().await;
            let state = get_installation_geo_state(&conn, installation_id)?;
            let Some(state) = state else {
                return Ok(());
            };
            if !force
                && state.last_seen_ip.as_deref() == Some(ip)
                && state.latitude.is_some()
                && state.longitude.is_some()
            {
                return Ok(());
            }
            state
        };
        let Some(geo) = lookup_ip_im_geo(ip).await else {
            return Ok(());
        };
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let no_stable_position =
            current_state.latitude.is_none() || current_state.longitude.is_none();
        if no_stable_position {
            persist_stable_geo(&conn, installation_id, &geo, now)?;
            return Ok(());
        }

        let stable_distance_km = haversine_distance_km(
            current_state.latitude,
            current_state.longitude,
            geo.latitude,
            geo.longitude,
        );
        let crossed_country = current_state.country_code != geo.country_code
            && current_state.country_code.is_some()
            && geo.country_code.is_some();
        let can_stay_stable = !crossed_country
            && stable_distance_km
                .map(|distance| distance <= GEO_STABLE_DISTANCE_KM)
                .unwrap_or(false);

        if can_stay_stable {
            persist_stable_geo(&conn, installation_id, &geo, now)?;
            return Ok(());
        }

        let candidate_matches = current_state
            .geo_candidate_latitude
            .zip(current_state.geo_candidate_longitude)
            .and_then(|(lat, lon)| {
                haversine_distance_km(Some(lat), Some(lon), geo.latitude, geo.longitude)
            })
            .map(|distance| distance <= GEO_CANDIDATE_DISTANCE_KM)
            .unwrap_or(false)
            && current_state.geo_candidate_country_code == geo.country_code;

        let candidate_hits = if candidate_matches {
            current_state.geo_candidate_hits + 1
        } else {
            1
        };
        let candidate_first_seen_at = if candidate_matches {
            current_state.geo_candidate_first_seen_at.unwrap_or(now)
        } else {
            now
        };
        persist_candidate_geo(
            &conn,
            installation_id,
            &geo,
            candidate_hits,
            candidate_first_seen_at,
        )?;

        let candidate_age_secs = (now - candidate_first_seen_at).num_seconds();
        let last_change_age_secs = current_state
            .geo_last_changed_at
            .map(|value| (now - value).num_seconds())
            .unwrap_or(i64::MAX);
        let promote_candidate = candidate_hits >= GEO_CANDIDATE_CONFIRM_HITS
            && candidate_age_secs >= GEO_CANDIDATE_MIN_AGE_SECS
            && last_change_age_secs >= GEO_STABLE_MIN_SWITCH_SECS;
        if promote_candidate {
            persist_stable_geo(&conn, installation_id, &geo, now)?;
        }
        Ok(())
    }

    pub async fn create_board_message(
        &self,
        settings: &BoardSettings,
        author: BoardAuthor,
        body: String,
        client_ip: Option<&str>,
    ) -> Result<BoardMessageView, AppError> {
        let normalized = normalize_board_body(&body, settings.max_len)?;
        let now = Utc::now();
        let id = Uuid::new_v4().to_string();
        let (author_kind, author_user_id, author_email, author_label, guest_id) =
            author.into_storage_fields();
        let ip_hash = client_ip.map(|ip| hash_ip_for_board(ip, &self.ip_hash_salt));
        let viewer_user_id = author_user_id.clone();
        let viewer_guest_id = guest_id.clone();

        let conn = self.conn.lock().await;

        if author_kind != "admin" {
            // Anonymous posts: prefer the salted IP bucket. The
            // `X-Board-Guest-Id` header is attacker-controlled (a client can
            // rotate it on every request to mint a fresh bucket), so it is
            // only used as a fallback when no IP is available — that scope
            // is intentionally narrow.
            let scope = if let Some(email) = author_email.as_deref() {
                format!("user:{email}")
            } else if let Some(hash) = ip_hash.as_deref() {
                format!("ip:{hash}")
            } else if let Some(guest) = guest_id.as_deref() {
                format!("guest:{guest}")
            } else {
                "guest:anon".to_string()
            };
            let limit = if author_kind == "user" {
                settings.user_per_hour
            } else {
                settings.guest_per_hour
            };
            consume_board_rate_limit_tx(&conn, &scope, limit, now)?;
        }

        conn.execute(
            "INSERT INTO board_messages (
                id, author_kind, author_user_id, author_email, author_label,
                guest_id, client_ip_hash, body, status, pinned_at, featured_at,
                deleted_by, deleted_at, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'visible', NULL, NULL, NULL, NULL, ?9, ?9)",
            params![
                id,
                author_kind,
                author_user_id,
                author_email,
                author_label,
                guest_id,
                ip_hash,
                normalized,
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert board message failed: {e}")))?;

        let row = load_board_message_row(&conn, &id)?
            .ok_or_else(|| AppError::Internal("inserted board message not found".into()))?;
        Ok(row.into_view(viewer_user_id.as_deref(), viewer_guest_id.as_deref()))
    }

    pub async fn list_board_messages(
        &self,
        tab: &str,
        limit: usize,
        viewer_user_id: Option<&str>,
        viewer_guest_id: Option<&str>,
        since: Option<DateTime<Utc>>,
    ) -> Result<BoardMessageListResponse, AppError> {
        let limit = limit.clamp(1, 200);
        let tab = normalize_board_tab(tab);
        let tab_visible_clause = match tab.as_str() {
            "pinned" => "status = 'visible' AND pinned_at IS NOT NULL",
            "featured" => {
                "status = 'visible' AND (featured_at IS NOT NULL OR pinned_at IS NOT NULL)"
            }
            _ => "status = 'visible'",
        };

        let conn = self.conn.lock().await;

        let (messages, removed_ids, incremental) = if let Some(since) = since.as_ref() {
            let since_str = since.to_rfc3339();
            let messages_sql = format!(
                "SELECT id, author_kind, author_user_id, author_email, author_label,
                        guest_id, body, pinned_at, featured_at, created_at
                   FROM board_messages
                  WHERE {tab_visible_clause}
                    AND datetime(updated_at) > datetime(?1)
                  ORDER BY (pinned_at IS NOT NULL) DESC, pinned_at DESC,
                           (featured_at IS NOT NULL) DESC, featured_at DESC,
                           datetime(created_at) DESC
                  LIMIT ?2"
            );
            let mut stmt = conn.prepare(&messages_sql).map_err(|e| {
                AppError::Internal(format!("prepare board incremental list failed: {e}"))
            })?;
            let rows = stmt
                .query_map(params![since_str, limit as i64], map_board_row)
                .map_err(|e| AppError::Internal(format!("query board incremental failed: {e}")))?;
            let mut messages = Vec::new();
            for row in rows {
                let row =
                    row.map_err(|e| AppError::Internal(format!("read board row failed: {e}")))?;
                messages.push(row.into_view(viewer_user_id, viewer_guest_id));
            }

            let removed_sql = format!(
                "SELECT id FROM board_messages
                  WHERE datetime(updated_at) > datetime(?1)
                    AND NOT ({tab_visible_clause})
                  LIMIT ?2"
            );
            let mut stmt = conn.prepare(&removed_sql).map_err(|e| {
                AppError::Internal(format!("prepare board removed list failed: {e}"))
            })?;
            let removed: Vec<String> = stmt
                .query_map(params![since_str, limit as i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| AppError::Internal(format!("query board removed failed: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AppError::Internal(format!("read board removed failed: {e}")))?;

            (messages, removed, true)
        } else {
            let sql = format!(
                "SELECT id, author_kind, author_user_id, author_email, author_label,
                        guest_id, body, pinned_at, featured_at, created_at
                   FROM board_messages
                  WHERE {tab_visible_clause}
                  ORDER BY (pinned_at IS NOT NULL) DESC, pinned_at DESC,
                           (featured_at IS NOT NULL) DESC, featured_at DESC,
                           datetime(created_at) DESC
                  LIMIT ?1"
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AppError::Internal(format!("prepare board list failed: {e}")))?;
            let rows = stmt
                .query_map(params![limit as i64], map_board_row)
                .map_err(|e| AppError::Internal(format!("query board messages failed: {e}")))?;
            let mut messages = Vec::new();
            for row in rows {
                let row =
                    row.map_err(|e| AppError::Internal(format!("read board row failed: {e}")))?;
                messages.push(row.into_view(viewer_user_id, viewer_guest_id));
            }
            (messages, Vec::new(), false)
        };

        let total_visible: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM board_messages WHERE status = 'visible'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Capture as_of after the queries — under the connection lock, no concurrent writes
        // could have applied in between, so any future write will have updated_at > as_of.
        let as_of = Utc::now();

        Ok(BoardMessageListResponse {
            messages,
            tab,
            total_visible: total_visible.max(0) as usize,
            as_of,
            removed_ids,
            incremental,
        })
    }

    pub async fn set_board_pinned(
        &self,
        settings: &BoardSettings,
        id: &str,
        value: bool,
    ) -> Result<BoardMessageView, AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let existing = load_board_message_row(&conn, id)?
            .ok_or_else(|| AppError::NotFound("message not found".into()))?;
        if existing.status != "visible" {
            return Err(AppError::Conflict("message is not visible".into()));
        }
        if value {
            // Enforce pin cap: keep the latest N pinned, oldest auto-unpin.
            if settings.pin_limit > 0 {
                let cap = settings.pin_limit as i64;
                let pinned_ids: Vec<String> = {
                    let mut stmt = conn
                        .prepare(
                            "SELECT id FROM board_messages
                              WHERE status = 'visible' AND pinned_at IS NOT NULL
                                AND id <> ?1
                              ORDER BY pinned_at DESC",
                        )
                        .map_err(|e| AppError::Internal(format!("prepare pin scan failed: {e}")))?;
                    let rows = stmt
                        .query_map(params![id], |row| row.get::<_, String>(0))
                        .map_err(|e| AppError::Internal(format!("scan pinned failed: {e}")))?;
                    rows.collect::<Result<Vec<_>, _>>()
                        .map_err(|e| AppError::Internal(format!("read pinned ids failed: {e}")))?
                };
                if pinned_ids.len() as i64 >= cap {
                    let take = pinned_ids.len() as i64 - (cap - 1);
                    for victim in pinned_ids.iter().rev().take(take.max(0) as usize) {
                        conn.execute(
                            "UPDATE board_messages
                                SET pinned_at = NULL, updated_at = ?2
                              WHERE id = ?1",
                            params![victim, now.to_rfc3339()],
                        )
                        .map_err(|e| AppError::Internal(format!("auto-unpin failed: {e}")))?;
                    }
                }
            }
            conn.execute(
                "UPDATE board_messages
                    SET pinned_at = ?2, updated_at = ?2
                  WHERE id = ?1",
                params![id, now.to_rfc3339()],
            )
            .map_err(|e| AppError::Internal(format!("pin message failed: {e}")))?;
        } else {
            conn.execute(
                "UPDATE board_messages
                    SET pinned_at = NULL, updated_at = ?2
                  WHERE id = ?1",
                params![id, now.to_rfc3339()],
            )
            .map_err(|e| AppError::Internal(format!("unpin message failed: {e}")))?;
        }
        let row = load_board_message_row(&conn, id)?
            .ok_or_else(|| AppError::NotFound("message not found".into()))?;
        Ok(row.into_view(None, None))
    }

    pub async fn set_board_featured(
        &self,
        id: &str,
        value: bool,
    ) -> Result<BoardMessageView, AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let existing = load_board_message_row(&conn, id)?
            .ok_or_else(|| AppError::NotFound("message not found".into()))?;
        if existing.status != "visible" {
            return Err(AppError::Conflict("message is not visible".into()));
        }
        let featured_at = if value { Some(now.to_rfc3339()) } else { None };
        conn.execute(
            "UPDATE board_messages
                SET featured_at = ?2, updated_at = ?3
              WHERE id = ?1",
            params![id, featured_at, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("feature message failed: {e}")))?;
        let row = load_board_message_row(&conn, id)?
            .ok_or_else(|| AppError::NotFound("message not found".into()))?;
        Ok(row.into_view(None, None))
    }

    pub async fn delete_board_message(
        &self,
        settings: &BoardSettings,
        id: &str,
        is_admin: bool,
        admin_email: Option<&str>,
        viewer_guest_id: Option<&str>,
    ) -> Result<(), AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let existing = load_board_message_row(&conn, id)?
            .ok_or_else(|| AppError::NotFound("message not found".into()))?;
        if existing.status != "visible" {
            return Ok(());
        }
        let allowed = if is_admin {
            true
        } else if existing.author_kind == "guest" {
            let matches_guest = match (viewer_guest_id, existing.guest_id.as_deref()) {
                (Some(viewer), Some(owner)) => !viewer.is_empty() && viewer == owner,
                _ => false,
            };
            let age = (now - existing.created_at).num_seconds();
            matches_guest && age <= settings.guest_self_delete_secs
        } else {
            false
        };
        if !allowed {
            return Err(AppError::Forbidden("you cannot delete this message".into()));
        }
        conn.execute(
            "UPDATE board_messages
                SET status = 'deleted', deleted_by = ?2, deleted_at = ?3,
                    updated_at = ?3, pinned_at = NULL, featured_at = NULL
              WHERE id = ?1",
            params![id, admin_email, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("delete board message failed: {e}")))?;
        Ok(())
    }

    pub async fn board_meta(
        &self,
        can_post_as_admin: bool,
        max_body_length: usize,
        guest_self_delete_secs: i64,
    ) -> Result<BoardMetaResponse, AppError> {
        let conn = self.conn.lock().await;
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM board_messages WHERE status = 'visible'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let pinned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM board_messages
                  WHERE status = 'visible' AND pinned_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let featured: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM board_messages
                  WHERE status = 'visible'
                    AND (featured_at IS NOT NULL OR pinned_at IS NOT NULL)",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(BoardMetaResponse {
            total: total.max(0) as usize,
            pinned_count: pinned.max(0) as usize,
            featured_count: featured.max(0) as usize,
            can_post_as_admin,
            max_body_length,
            guest_self_delete_secs,
        })
    }

    pub async fn record_admin_audit(
        &self,
        actor_email: Option<&str>,
        action: &str,
        payload: Option<&serde_json::Value>,
        ip: Option<&str>,
    ) -> Result<(), AppError> {
        let payload_json = payload
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string()))
            .or(None);
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO admin_audit_log (id, actor_email, action, payload_json, ip, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                Uuid::new_v4().to_string(),
                actor_email,
                action,
                payload_json,
                ip,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("write audit log failed: {e}")))?;
        Ok(())
    }

    pub async fn list_admin_audit(&self, limit: usize) -> Result<Vec<AdminAuditEntry>, AppError> {
        let limit = limit.clamp(1, 500);
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, actor_email, action, payload_json, ip, created_at
                   FROM admin_audit_log
                  ORDER BY datetime(created_at) DESC
                  LIMIT ?1",
            )
            .map_err(|e| AppError::Internal(format!("prepare audit list failed: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(AdminAuditEntry {
                    id: row.get(0)?,
                    actor_email: row.get(1)?,
                    action: row.get(2)?,
                    payload_json: row.get(3)?,
                    ip: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| AppError::Internal(format!("query audit log failed: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AppError::Internal(format!("read audit row failed: {e}")))?);
        }
        Ok(out)
    }
}

fn share_logs_need_recovery(logs: Option<&[ShareRequestLogEntry]>, now_ts: i64) -> bool {
    let Some(logs) = logs else {
        return true;
    };
    let newest_created_at = logs.iter().map(|log| log.created_at).max().unwrap_or(0);
    newest_created_at <= now_ts - SHARE_REQUEST_LOG_RECOVERY_STALE_SECS
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminAuditEntry {
    pub id: String,
    pub actor_email: Option<String>,
    pub action: String,
    pub payload_json: Option<String>,
    pub ip: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub enum BoardAuthor {
    Admin {
        user_id: String,
        email: String,
    },
    User {
        user_id: String,
        email: String,
    },
    Guest {
        guest_id: String,
        name: Option<String>,
    },
}

impl BoardAuthor {
    fn into_storage_fields(
        self,
    ) -> (
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
    ) {
        match self {
            BoardAuthor::Admin { user_id, email } => (
                "admin".to_string(),
                Some(user_id),
                Some(email),
                "Official".to_string(),
                None,
            ),
            BoardAuthor::User { user_id, email } => {
                let label = mask_email(&email);
                ("user".to_string(), Some(user_id), Some(email), label, None)
            }
            BoardAuthor::Guest { guest_id, name } => {
                let label = name
                    .as_deref()
                    .map(normalize_guest_name)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Guest".to_string());
                ("guest".to_string(), None, None, label, Some(guest_id))
            }
        }
    }
}

#[derive(Debug, Clone)]
struct BoardMessageRow {
    id: String,
    author_kind: String,
    author_user_id: Option<String>,
    #[allow(dead_code)]
    author_email: Option<String>,
    author_label: String,
    guest_id: Option<String>,
    body: String,
    pinned_at: Option<DateTime<Utc>>,
    featured_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    status: String,
}

impl BoardMessageRow {
    fn into_view(
        self,
        viewer_user_id: Option<&str>,
        viewer_guest_id: Option<&str>,
    ) -> BoardMessageView {
        let is_mine = match self.author_kind.as_str() {
            "guest" => match (viewer_guest_id, self.guest_id.as_deref()) {
                (Some(viewer), Some(owner)) => !viewer.is_empty() && viewer == owner,
                _ => false,
            },
            "user" | "admin" => match (viewer_user_id, self.author_user_id.as_deref()) {
                (Some(viewer), Some(owner)) => !viewer.is_empty() && viewer == owner,
                _ => false,
            },
            _ => false,
        };
        BoardMessageView {
            id: self.id,
            body: self.body,
            author_kind: self.author_kind,
            author_label: self.author_label,
            is_mine,
            pinned: self.pinned_at.is_some(),
            featured: self.featured_at.is_some(),
            created_at: self.created_at,
            pinned_at: self.pinned_at,
            featured_at: self.featured_at,
        }
    }
}

fn map_board_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BoardMessageRow> {
    let pinned_at: Option<String> = row.get(7)?;
    let featured_at: Option<String> = row.get(8)?;
    let created_at: String = row.get(9)?;
    Ok(BoardMessageRow {
        id: row.get(0)?,
        author_kind: row.get(1)?,
        author_user_id: row.get(2)?,
        author_email: row.get(3)?,
        author_label: row.get(4)?,
        guest_id: row.get(5)?,
        body: row.get(6)?,
        pinned_at: pinned_at.as_deref().map(parse_dt_sql).transpose()?,
        featured_at: featured_at.as_deref().map(parse_dt_sql).transpose()?,
        created_at: parse_dt_sql(&created_at)?,
        status: "visible".to_string(),
    })
}

fn load_board_message_row(
    conn: &Connection,
    id: &str,
) -> Result<Option<BoardMessageRow>, AppError> {
    conn.query_row(
        "SELECT id, author_kind, author_user_id, author_email, author_label,
                guest_id, body, pinned_at, featured_at, created_at, status
           FROM board_messages
          WHERE id = ?1",
        params![id],
        |row| {
            let pinned_at: Option<String> = row.get(7)?;
            let featured_at: Option<String> = row.get(8)?;
            let created_at: String = row.get(9)?;
            Ok(BoardMessageRow {
                id: row.get(0)?,
                author_kind: row.get(1)?,
                author_user_id: row.get(2)?,
                author_email: row.get(3)?,
                author_label: row.get(4)?,
                guest_id: row.get(5)?,
                body: row.get(6)?,
                pinned_at: pinned_at.as_deref().map(parse_dt_sql).transpose()?,
                featured_at: featured_at.as_deref().map(parse_dt_sql).transpose()?,
                created_at: parse_dt_sql(&created_at)?,
                status: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("load board message failed: {e}")))
}

fn normalize_board_tab(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "pinned" => "pinned".into(),
        "featured" => "featured".into(),
        _ => "all".into(),
    }
}

fn normalize_board_body(body: &str, max_len: usize) -> Result<String, AppError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("message body is empty".into()));
    }
    let max_len = max_len.max(1);
    let char_count = trimmed.chars().count();
    if char_count > max_len {
        return Err(AppError::BadRequest(format!(
            "message body exceeds {max_len} characters"
        )));
    }
    let stripped: String = trimmed
        .chars()
        .filter(|ch| {
            if ch.is_control() {
                matches!(*ch, '\n' | '\r' | '\t')
            } else {
                // Strip common zero-width / bidi formatting characters.
                !matches!(
                    *ch,
                    '\u{200B}'
                        | '\u{200C}'
                        | '\u{200D}'
                        | '\u{2060}'
                        | '\u{FEFF}'
                        | '\u{202A}'
                        | '\u{202B}'
                        | '\u{202C}'
                        | '\u{202D}'
                        | '\u{202E}'
                )
            }
        })
        .collect();
    let cleaned = stripped.trim().to_string();
    if cleaned.is_empty() {
        return Err(AppError::BadRequest("message body is empty".into()));
    }
    let link_count = cleaned.matches("http://").count() + cleaned.matches("https://").count();
    if link_count >= 3 {
        return Err(AppError::BadRequest(
            "too many links in a single message".into(),
        ));
    }
    Ok(cleaned)
}

fn normalize_guest_name(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| !ch.is_control())
        .filter(|ch| {
            !matches!(
                *ch,
                '\u{200B}'
                    | '\u{200C}'
                    | '\u{200D}'
                    | '\u{2060}'
                    | '\u{FEFF}'
                    | '\u{202A}'
                    | '\u{202B}'
                    | '\u{202C}'
                    | '\u{202D}'
                    | '\u{202E}'
            )
        })
        .collect();
    let trimmed = cleaned.trim();
    let truncated: String = trimmed.chars().take(16).collect();
    truncated
}

fn hash_ip_for_board(ip: &str, salt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b"\x00");
    hasher.update(ip.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn consume_board_rate_limit_tx(
    conn: &Connection,
    scope: &str,
    limit: i64,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    if limit <= 0 {
        return Err(AppError::TooManyRequests(
            "message board posting is disabled for this audience".into(),
        ));
    }
    let bucket = now.timestamp() / 3600;
    let cleanup_cutoff = bucket - 24;
    conn.execute(
        "DELETE FROM board_rate_limit WHERE bucket_start < ?1",
        params![cleanup_cutoff],
    )
    .map_err(|e| AppError::Internal(format!("prune board rate limit failed: {e}")))?;

    let current: i64 = conn
        .query_row(
            "SELECT count FROM board_rate_limit WHERE scope = ?1 AND bucket_start = ?2",
            params![scope, bucket],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read board rate limit failed: {e}")))?
        .unwrap_or(0);
    if current >= limit {
        return Err(AppError::TooManyRequests(
            "message board posting rate limit exceeded".into(),
        ));
    }
    conn.execute(
        "INSERT INTO board_rate_limit (scope, bucket_start, count)
         VALUES (?1, ?2, 1)
         ON CONFLICT(scope, bucket_start) DO UPDATE SET count = count + 1",
        params![scope, bucket],
    )
    .map_err(|e| AppError::Internal(format!("bump board rate limit failed: {e}")))?;
    Ok(())
}

async fn fetch_share_request_logs_from_route(
    config: &Config,
    client: &reqwest::Client,
    subdomain: &str,
) -> Result<ShareRequestLogFetchResponse, AppError> {
    let url = format!(
        "{}/_share-router/request-logs",
        config.tunnel_url(subdomain)
    );
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("fetch share request logs failed: {e}")))?;

    if !response.status().is_success() {
        return Err(AppError::Internal(format!(
            "fetch share request logs failed with status {}",
            response.status()
        )));
    }

    response
        .json::<ShareRequestLogFetchResponse>()
        .await
        .map_err(|e| AppError::Internal(format!("decode share request logs failed: {e}")))
}

pub async fn fetch_share_runtime_snapshot_from_route(
    config: &Config,
    client: &reqwest::Client,
    subdomain: &str,
    share_id: &str,
) -> Result<ShareRuntimeSnapshotResponse, AppError> {
    // 多 share 模式：同一 cc-switch 可能挂多个 share。
    // 用 `?shareId=...` 显式定位，避免老路径"取第一个 share"的歧义。
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("shareId", share_id)
        .finish();
    let url = format!(
        "{}/_share-router/share-runtime?{query}",
        config.tunnel_url(subdomain)
    );
    let response = client
        .get(&url)
        .header("X-Share-Router-Probe", "1")
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("fetch share runtime failed: {e}")))?;

    if !response.status().is_success() {
        return Err(AppError::Internal(format!(
            "fetch share runtime failed with status {}",
            response.status()
        )));
    }

    response
        .json::<ShareRuntimeSnapshotResponse>()
        .await
        .map_err(|e| AppError::Internal(format!("decode share runtime failed: {e}")))
}

fn upsert_share_tx(
    conn: &Connection,
    installation_id: &str,
    mut share: ShareDescriptor,
) -> Result<(), AppError> {
    if share.bindings.len() != 1
        || share.provider_id.as_deref().is_none_or(|provider_id| {
            share.bindings.get(&share.app_type).map(String::as_str) != Some(provider_id)
        })
    {
        return Err(AppError::BadRequest(
            "share must contain exactly one binding matching appType/providerId".into(),
        ));
    }
    let installation = get_installation(conn, installation_id)?
        .ok_or_else(|| AppError::Unauthorized("installation not found".into()))?;
    let installation_owner = installation
        .owner_email
        .as_deref()
        .ok_or_else(|| AppError::Conflict("installation owner email is not configured".into()))
        .and_then(normalize_email)?;
    normalize_self_reported_share_owner(&mut share, &installation_owner)?;
    let description = normalize_share_description(share.description.clone())?;
    let for_sale = normalize_share_for_sale(&share.for_sale)?;
    let market_access_mode = normalize_market_access_mode(&share.market_access_mode)?;
    let sale_market_kind = normalize_sale_market_kind(&share.sale_market_kind)?;
    let upstream_provider_json = share
        .upstream_provider
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| AppError::Internal(format!("serialize upstream provider failed: {e}")))?;
    let shared_with_emails_json = serde_json::to_string(&share.shared_with_emails)
        .map_err(|e| AppError::Internal(format!("serialize shared_with_emails failed: {e}")))?;
    let access_by_app_json = serde_json::to_string(&share.access_by_app)
        .map_err(|e| AppError::Internal(format!("serialize access_by_app failed: {e}")))?;
    let app_settings = effective_share_app_settings(&share);
    let app_settings_json = serde_json::to_string(&app_settings)
        .map_err(|e| AppError::Internal(format!("serialize app_settings failed: {e}")))?;
    let bindings_json = if share.bindings.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(&share.bindings)
                .map_err(|e| AppError::Internal(format!("serialize share bindings failed: {e}")))?,
        )
    };
    let app_runtimes_json = serde_json::to_string(&share.app_runtimes)
        .map_err(|e| AppError::Internal(format!("serialize app runtimes failed: {e}")))?;
    let app_providers_json = serde_json::to_string(&share.app_providers)
        .map_err(|e| AppError::Internal(format!("serialize app providers failed: {e}")))?;
    conn.execute(
        "INSERT INTO shares (
            share_id, installation_id, share_name, owner_email, shared_with_emails_json, market_access_mode, access_by_app_json, app_settings_json, description, for_sale, sale_market_kind, subdomain, app_type, provider_id,
            enabled_claude, enabled_codex, enabled_gemini,
            token_limit, parallel_limit, tokens_used, requests_count, share_status, created_at, expires_at, upstream_provider_json, app_runtimes_json, app_providers_json, bindings_json, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29)
        ON CONFLICT(share_id) DO UPDATE SET
            installation_id = excluded.installation_id,
            share_name = excluded.share_name,
            owner_email = excluded.owner_email,
            shared_with_emails_json = excluded.shared_with_emails_json,
            market_access_mode = excluded.market_access_mode,
            access_by_app_json = excluded.access_by_app_json,
            app_settings_json = excluded.app_settings_json,
            description = excluded.description,
            for_sale = excluded.for_sale,
            sale_market_kind = excluded.sale_market_kind,
            subdomain = excluded.subdomain,
            app_type = excluded.app_type,
            provider_id = excluded.provider_id,
            enabled_claude = shares.enabled_claude,
            enabled_codex = shares.enabled_codex,
            enabled_gemini = shares.enabled_gemini,
            token_limit = excluded.token_limit,
            parallel_limit = excluded.parallel_limit,
            tokens_used = excluded.tokens_used,
            requests_count = excluded.requests_count,
            share_status = excluded.share_status,
            created_at = COALESCE(NULLIF(shares.created_at, ''), excluded.created_at),
            expires_at = excluded.expires_at,
            upstream_provider_json = excluded.upstream_provider_json,
            app_runtimes_json = excluded.app_runtimes_json,
            app_providers_json = excluded.app_providers_json,
            bindings_json = excluded.bindings_json,
            runtime_refreshed_at = shares.runtime_refreshed_at,
            updated_at = excluded.updated_at",
        params![
            share.share_id,
            installation_id,
            share.share_name,
            share.owner_email,
            shared_with_emails_json,
            market_access_mode,
            access_by_app_json,
            app_settings_json,
            description,
            for_sale,
            sale_market_kind,
            share.subdomain,
            share.app_type,
            share.provider_id,
            i64::from(share.support.claude as u8),
            i64::from(share.support.codex as u8),
            i64::from(share.support.gemini as u8),
            share.token_limit,
            share.parallel_limit,
            share.tokens_used,
            share.requests_count,
            share.share_status,
            share.created_at,
            share.expires_at,
            upstream_provider_json,
            app_runtimes_json,
            app_providers_json,
            bindings_json,
            Utc::now().to_rfc3339(),
        ],
    )
    .map_err(map_share_constraint_error)?;
    Ok(())
}

fn delete_all_shares_for_installation_tx(
    conn: &Connection,
    installation_id: &str,
) -> Result<usize, AppError> {
    let share_ids = {
        let mut stmt = conn
            .prepare("SELECT share_id FROM shares WHERE installation_id = ?1")
            .map_err(|e| {
                AppError::Internal(format!("prepare installation shares cleanup failed: {e}"))
            })?;
        let rows = stmt
            .query_map(params![installation_id], |row| row.get::<_, String>(0))
            .map_err(|e| {
                AppError::Internal(format!("query installation shares cleanup failed: {e}"))
            })?;
        collect_rows(rows)?
    };

    for share_id in &share_ids {
        conn.execute(
            "DELETE FROM share_request_logs WHERE share_id = ?1",
            params![share_id],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "delete installation share request logs failed: {e}"
            ))
        })?;
        conn.execute(
            "DELETE FROM share_health_checks WHERE share_id = ?1",
            params![share_id],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "delete installation share health checks failed: {e}"
            ))
        })?;
    }

    let deleted = conn
        .execute(
            "DELETE FROM shares WHERE installation_id = ?1",
            params![installation_id],
        )
        .map_err(|e| AppError::Internal(format!("delete installation shares failed: {e}")))?;
    Ok(deleted)
}

fn upsert_share_request_log_tx(
    conn: &Connection,
    installation_id: &str,
    log: ShareRequestLogEntry,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO share_request_logs (
            request_id, installation_id, share_id, share_name, provider_id, provider_name,
            app_type, model, request_model, request_agent, requested_model, actual_model, actual_model_source,
            status_code, latency_ms, first_token_ms,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            is_streaming, session_id, user_country, user_country_iso3, user_email, is_health_check, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27)
        ON CONFLICT(request_id) DO UPDATE SET
            installation_id = excluded.installation_id,
            share_id = excluded.share_id,
            share_name = excluded.share_name,
            provider_id = excluded.provider_id,
            provider_name = excluded.provider_name,
            app_type = excluded.app_type,
            model = excluded.model,
            request_model = excluded.request_model,
            request_agent = excluded.request_agent,
            requested_model = excluded.requested_model,
            actual_model = excluded.actual_model,
            actual_model_source = excluded.actual_model_source,
            status_code = excluded.status_code,
            latency_ms = excluded.latency_ms,
            first_token_ms = excluded.first_token_ms,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_creation_tokens = excluded.cache_creation_tokens,
            is_streaming = excluded.is_streaming,
            session_id = excluded.session_id,
            user_country = COALESCE(excluded.user_country, share_request_logs.user_country),
            user_country_iso3 = COALESCE(excluded.user_country_iso3, share_request_logs.user_country_iso3),
            user_email = COALESCE(excluded.user_email, share_request_logs.user_email),
            is_health_check = excluded.is_health_check,
            created_at = excluded.created_at",
        params![
            log.request_id,
            installation_id,
            log.share_id,
            log.share_name,
            log.provider_id,
            log.provider_name,
            log.app_type,
            log.model,
            log.request_model,
            log.request_agent,
            log.requested_model,
            log.actual_model,
            log.actual_model_source,
            i64::from(log.status_code),
            log.latency_ms as i64,
            log.first_token_ms.map(|v| v as i64),
            i64::from(log.input_tokens),
            i64::from(log.output_tokens),
            i64::from(log.cache_read_tokens),
            i64::from(log.cache_creation_tokens),
            i64::from(log.is_streaming as u8),
            log.session_id,
            log.user_country,
            log.user_country_iso3,
            log.user_email,
            i64::from(log.is_health_check as u8),
            log.created_at,
        ],
    )
    .map_err(|e| AppError::Internal(format!("upsert share request log failed: {e}")))?;
    Ok(())
}

fn validate_market_request_log(log: &MarketRequestLogEntry) -> Result<(), AppError> {
    if !is_valid_request_id(&log.request_id) {
        return Err(AppError::BadRequest("invalid market request id".into()));
    }
    if log.status.trim().is_empty() || log.status.len() > 40 {
        return Err(AppError::BadRequest("invalid market request status".into()));
    }
    Ok(())
}

fn is_valid_request_id(value: &str) -> bool {
    (8..=80).contains(&value.len())
        && value.starts_with("req_")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn upsert_market_request_log_tx(
    conn: &Connection,
    market: &MarketRegistryRecord,
    log: MarketRequestLogEntry,
) -> Result<(), AppError> {
    let synced_at = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO market_request_logs (
            request_id, market_id, market_email, market_subdomain, user_email, api_key_prefix,
            router_id, share_id, share_subdomain, model, request_agent, requested_model, actual_model, actual_model_source,
            status, status_code, error_message, latency_ms,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            usage_amount_usd, created_at, settled_at, user_country, user_country_iso3, synced_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)
        ON CONFLICT(request_id) DO UPDATE SET
            market_id = excluded.market_id,
            market_email = excluded.market_email,
            market_subdomain = excluded.market_subdomain,
            user_email = COALESCE(excluded.user_email, market_request_logs.user_email),
            api_key_prefix = COALESCE(excluded.api_key_prefix, market_request_logs.api_key_prefix),
            router_id = COALESCE(excluded.router_id, market_request_logs.router_id),
            share_id = COALESCE(excluded.share_id, market_request_logs.share_id),
            share_subdomain = COALESCE(excluded.share_subdomain, market_request_logs.share_subdomain),
            model = COALESCE(excluded.model, market_request_logs.model),
            request_agent = excluded.request_agent,
            requested_model = excluded.requested_model,
            actual_model = excluded.actual_model,
            actual_model_source = excluded.actual_model_source,
            status = excluded.status,
            status_code = COALESCE(excluded.status_code, market_request_logs.status_code),
            error_message = COALESCE(excluded.error_message, market_request_logs.error_message),
            latency_ms = COALESCE(excluded.latency_ms, market_request_logs.latency_ms),
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_creation_tokens = excluded.cache_creation_tokens,
            usage_amount_usd = COALESCE(excluded.usage_amount_usd, market_request_logs.usage_amount_usd),
            created_at = excluded.created_at,
            settled_at = COALESCE(excluded.settled_at, market_request_logs.settled_at),
            user_country = COALESCE(excluded.user_country, market_request_logs.user_country),
            user_country_iso3 = COALESCE(excluded.user_country_iso3, market_request_logs.user_country_iso3),
            synced_at = excluded.synced_at",
        params![
            log.request_id,
            market.id,
            market.email,
            market.subdomain,
            log.user_email,
            log.api_key_prefix,
            log.router_id,
            log.share_id,
            log.share_subdomain,
            log.model,
            log.request_agent,
            log.requested_model,
            log.actual_model,
            log.actual_model_source,
            log.status,
            log.status_code.map(i64::from),
            log.error_message,
            log.latency_ms.map(|value| value as i64),
            i64::from(log.input_tokens),
            i64::from(log.output_tokens),
            i64::from(log.cache_read_tokens),
            i64::from(log.cache_creation_tokens),
            log.usage_amount_usd,
            log.created_at,
            log.settled_at,
            log.user_country,
            log.user_country_iso3,
            synced_at,
        ],
    )
    .map_err(|e| AppError::Internal(format!("upsert market request log failed: {e}")))?;
    Ok(())
}

fn sanitize_map_display_settings(mut settings: MapDisplaySettings) -> MapDisplaySettings {
    settings.viewport.visible_start_px = settings.viewport.visible_start_px.clamp(0, 5000);
    settings
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMapDisplaySettings {
    #[serde(default = "default_show_flows")]
    show_flows: bool,
    #[serde(default = "default_show_heat")]
    show_heat: bool,
    viewport: StoredMapViewportSettings,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMapViewportSettings {
    #[serde(default)]
    visible_start_px: i32,
    #[serde(default)]
    visible_end_px: i32,
    #[serde(default)]
    vertical_pan_px: i32,
}

fn default_show_flows() -> bool {
    true
}

fn default_show_heat() -> bool {
    true
}

fn normalize_stored_map_display_settings(stored: StoredMapDisplaySettings) -> MapDisplaySettings {
    let _legacy_visible_end_px = stored.viewport.visible_end_px;
    MapDisplaySettings {
        show_flows: stored.show_flows,
        show_heat: stored.show_heat,
        viewport: MapViewportSettings {
            visible_start_px: (stored.viewport.visible_start_px - stored.viewport.vertical_pan_px)
                .clamp(0, 5000),
        },
    }
}

fn read_map_display_settings(conn: &Connection) -> Result<MapDisplaySettings, AppError> {
    let json: Option<String> = conn
        .query_row(
            "SELECT settings_json FROM router_map_display_settings WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read map display settings failed: {e}")))?;
    match json {
        Some(raw) => {
            let parsed = serde_json::from_str::<StoredMapDisplaySettings>(&raw).map_err(|e| {
                AppError::Internal(format!("parse map display settings failed: {e}"))
            })?;
            Ok(sanitize_map_display_settings(
                normalize_stored_map_display_settings(parsed),
            ))
        }
        None => Ok(MapDisplaySettings::default()),
    }
}

fn write_map_display_settings(
    conn: &Connection,
    settings: &MapDisplaySettings,
) -> Result<(), AppError> {
    let json = serde_json::to_string(settings)
        .map_err(|e| AppError::Internal(format!("serialize map display settings failed: {e}")))?;
    let updated_at = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO router_map_display_settings (id, settings_json, updated_at)
         VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET
            settings_json = excluded.settings_json,
            updated_at = excluded.updated_at",
        params![json, updated_at],
    )
    .map_err(|e| AppError::Internal(format!("write map display settings failed: {e}")))?;
    Ok(())
}

fn init_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS installations (
            id TEXT PRIMARY KEY,
            public_key TEXT NOT NULL,
            platform TEXT NOT NULL,
            app_version TEXT NOT NULL,
            owner_email TEXT,
            owner_verified_at TEXT,
            last_seen_ip TEXT,
            country_code TEXT,
            country TEXT,
            region TEXT,
            city TEXT,
            latitude REAL,
            longitude REAL,
            geo_candidate_country_code TEXT,
            geo_candidate_country TEXT,
            geo_candidate_region TEXT,
            geo_candidate_city TEXT,
            geo_candidate_latitude REAL,
            geo_candidate_longitude REAL,
            geo_candidate_hits INTEGER NOT NULL DEFAULT 0,
            geo_candidate_first_seen_at TEXT,
            geo_last_changed_at TEXT,
            created_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            control_secret_b64 TEXT
        );

        CREATE TABLE IF NOT EXISTS leases (
            id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            connection_id TEXT NOT NULL UNIQUE,
            subdomain TEXT NOT NULL,
            tunnel_type TEXT NOT NULL,
            ssh_username TEXT NOT NULL,
            ssh_password TEXT NOT NULL,
            issued_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            used_at TEXT,
            share_json TEXT
        );

        CREATE TABLE IF NOT EXISTS shares (
            share_id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            owner_email TEXT,
            shared_with_emails_json TEXT NOT NULL DEFAULT '[]',
            market_access_mode TEXT NOT NULL DEFAULT 'selected',
            access_by_app_json TEXT NOT NULL DEFAULT '{}',
            app_settings_json TEXT NOT NULL DEFAULT '{}',
            description TEXT,
            for_sale TEXT NOT NULL DEFAULT 'No',
            sale_market_kind TEXT NOT NULL DEFAULT 'token',
            subdomain TEXT,
            app_type TEXT NOT NULL,
            provider_id TEXT,
            enabled_claude INTEGER NOT NULL DEFAULT 0,
            enabled_codex INTEGER NOT NULL DEFAULT 0,
            enabled_gemini INTEGER NOT NULL DEFAULT 0,
            token_limit INTEGER NOT NULL,
            parallel_limit INTEGER NOT NULL DEFAULT 3,
            tokens_used INTEGER NOT NULL,
            requests_count INTEGER NOT NULL,
            share_status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            upstream_provider_json TEXT,
            app_runtimes_json TEXT,
            app_providers_json TEXT,
            -- Share 的单一 app/provider 绑定快照（JSON: {app_type: provider_id}）。
            bindings_json TEXT,
            runtime_refreshed_at TEXT,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS installation_client_tunnels (
            installation_id TEXT PRIMARY KEY,
            owner_email TEXT NOT NULL,
            subdomain TEXT NOT NULL UNIQUE,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_seen_at TEXT
        );

        CREATE TABLE IF NOT EXISTS installation_payout_profiles (
            installation_id TEXT PRIMARY KEY,
            owner_email TEXT NOT NULL,
            revision INTEGER NOT NULL,
            profile_json TEXT,
            source_updated_at_ms INTEGER NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (installation_id) REFERENCES installations(id)
        );

        CREATE TABLE IF NOT EXISTS share_request_logs (
            request_id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            share_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            request_model TEXT NOT NULL,
            request_agent TEXT NOT NULL DEFAULT '',
            requested_model TEXT NOT NULL DEFAULT '',
            actual_model TEXT NOT NULL DEFAULT '',
            actual_model_source TEXT NOT NULL DEFAULT '',
            status_code INTEGER NOT NULL,
            latency_ms INTEGER NOT NULL,
            first_token_ms INTEGER,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            cache_read_tokens INTEGER NOT NULL,
            cache_creation_tokens INTEGER NOT NULL,
            is_streaming INTEGER NOT NULL,
            session_id TEXT,
            user_country TEXT,
            user_country_iso3 TEXT,
            user_email TEXT,
            is_health_check INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS image_generation_jobs (
            job_id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            status TEXT NOT NULL,
            status_code INTEGER,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            queued_at INTEGER NOT NULL,
            started_at INTEGER,
            completed_at INTEGER,
            expires_at INTEGER,
            prompt_preview TEXT,
            error_message TEXT,
            result_mime_type TEXT,
            result_size_bytes INTEGER,
            result_storage_key TEXT,
            result_token_hash TEXT,
            created_by_email TEXT,
            client_ip TEXT,
            user_country TEXT,
            idempotency_key TEXT
        );

        CREATE TABLE IF NOT EXISTS image_generation_request_logs (
            request_id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            share_name TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            status TEXT NOT NULL,
            status_code INTEGER,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            completed_at INTEGER,
            prompt_preview TEXT,
            error_message TEXT,
            result_mime_type TEXT,
            result_size_bytes INTEGER,
            result_storage_key TEXT,
            result_access_token TEXT,
            created_by_email TEXT,
            client_ip TEXT,
            user_country TEXT
        );

        CREATE TABLE IF NOT EXISTS share_health_checks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            share_id TEXT NOT NULL,
            checked_at INTEGER NOT NULL,
            is_healthy INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS installation_health_checks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            installation_id TEXT NOT NULL,
            checked_at INTEGER NOT NULL,
            is_healthy INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS share_model_health_checks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            request_id TEXT NOT NULL UNIQUE,
            share_id TEXT NOT NULL,
            subdomain TEXT NOT NULL,
            app_type TEXT NOT NULL,
            requested_model TEXT NOT NULL,
            actual_model TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL,
            status_code INTEGER,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            first_token_ms INTEGER,
            error_message TEXT,
            checked_at INTEGER NOT NULL,
            source TEXT NOT NULL DEFAULT 'scheduled'
        );

        CREATE TABLE IF NOT EXISTS share_model_health_state (
            share_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            requested_model TEXT NOT NULL,
            actual_model TEXT NOT NULL DEFAULT '',
            last_status TEXT NOT NULL,
            last_success_at INTEGER,
            last_failed_at INTEGER,
            last_checked_at INTEGER NOT NULL,
            recent_results_json TEXT NOT NULL DEFAULT '[]',
            error_message TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (share_id, app_type, requested_model)
        );

        CREATE TABLE IF NOT EXISTS share_edit_requests (
            id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            revision INTEGER NOT NULL,
            status TEXT NOT NULL,
            patch_json TEXT NOT NULL,
            created_by_email TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            applied_at TEXT,
            error_message TEXT
        );

        CREATE TABLE IF NOT EXISTS market_disabled_shares (
            market_email TEXT NOT NULL,
            share_id TEXT NOT NULL,
            disabled_by_email TEXT NOT NULL,
            reason TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (market_email, share_id)
        );

        CREATE TABLE IF NOT EXISTS market_share_model_failure_state (
            market_email TEXT NOT NULL,
            share_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            requested_model TEXT NOT NULL,
            actual_model TEXT NOT NULL DEFAULT '',
            last_status TEXT NOT NULL,
            last_success_at INTEGER,
            last_failed_at INTEGER,
            last_checked_at INTEGER NOT NULL,
            recent_results_json TEXT NOT NULL DEFAULT '[]',
            error_message TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (market_email, share_id, app_type, requested_model)
        );

        CREATE TABLE IF NOT EXISTS market_share_runtime_states (
            market_email TEXT NOT NULL,
            share_id TEXT NOT NULL,
            router_id TEXT,
            scope TEXT NOT NULL,
            kind TEXT NOT NULL,
            app_type TEXT,
            model_id TEXT,
            model_name TEXT,
            reason_kind TEXT,
            reason TEXT,
            failure_count INTEGER,
            expires_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS share_market_listing_statuses (
            market_email TEXT NOT NULL,
            router_id TEXT NOT NULL,
            share_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            listing_url TEXT NOT NULL,
            status TEXT NOT NULL,
            sale_mode TEXT,
            filled_seats INTEGER,
            required_seats INTEGER,
            listing_status TEXT,
            expires_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (market_email, router_id, share_id, app_type)
        );

        CREATE TABLE IF NOT EXISTS dashboard_presence (
            session_id TEXT PRIMARY KEY,
            last_seen_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS dashboard_ux_events (
            id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            source TEXT,
            target_type TEXT,
            step_count INTEGER,
            elapsed_ms INTEGER,
            keyboard INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS email_send_logs (
            id TEXT PRIMARY KEY,
            email_type TEXT NOT NULL,
            to_email TEXT NOT NULL,
            provider_message_id TEXT,
            status TEXT NOT NULL,
            error_message TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS request_nonces (
            installation_id TEXT NOT NULL,
            action TEXT NOT NULL,
            nonce TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (installation_id, action, nonce)
        );

        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            email_normalized TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL,
            last_login_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS email_login_challenges (
            id TEXT PRIMARY KEY,
            email_normalized TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            purpose TEXT NOT NULL,
            code_hash TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            consumed_at TEXT,
            attempt_count INTEGER NOT NULL DEFAULT 0,
            resend_available_at TEXT NOT NULL,
            created_ip TEXT,
            created_user_agent TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS user_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            access_token_hash TEXT NOT NULL UNIQUE,
            refresh_token_hash TEXT NOT NULL UNIQUE,
            access_expires_at TEXT NOT NULL,
            refresh_expires_at TEXT NOT NULL,
            revoked_at TEXT,
            created_at TEXT NOT NULL,
            last_used_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS user_api_tokens (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            token_prefix TEXT NOT NULL,
            token_plaintext TEXT,
            scopes_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            last_used_at TEXT,
            reset_at TEXT,
            revoked_at TEXT
        );

        CREATE TABLE IF NOT EXISTS router_markets (
            id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            email TEXT NOT NULL UNIQUE,
            subdomain TEXT NOT NULL UNIQUE,
            public_base_url TEXT NOT NULL,
            market_kind TEXT NOT NULL DEFAULT 'usage',
            scopes_json TEXT NOT NULL DEFAULT '[\"market:shares:read\",\"market:proxy:use\",\"market:email:notify\"]',
            pricing_json TEXT,
            maintenance_enabled INTEGER NOT NULL DEFAULT 0,
            maintenance_message TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            listed INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            offline_since TEXT
        );

        CREATE TABLE IF NOT EXISTS router_gateways (
            id TEXT PRIMARY KEY,
            owner_email TEXT NOT NULL,
            display_name TEXT NOT NULL,
            public_key TEXT NOT NULL UNIQUE,
            public_base_url TEXT,
            app_version TEXT,
            scopes_json TEXT NOT NULL DEFAULT '[\"gateway:shares:read\",\"gateway:proxy:use\",\"gateway:feedback:write\",\"gateway:request_logs:write\"]',
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS market_notification_emails (
            id TEXT PRIMARY KEY,
            market_email TEXT NOT NULL,
            kind TEXT NOT NULL,
            to_email TEXT NOT NULL,
            locale TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            provider_message_id TEXT,
            status TEXT NOT NULL,
            error_message TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS market_request_logs (
            request_id TEXT PRIMARY KEY,
            market_id TEXT NOT NULL,
            market_email TEXT NOT NULL,
            market_subdomain TEXT NOT NULL,
            user_email TEXT,
            api_key_prefix TEXT,
            router_id TEXT,
            share_id TEXT,
            share_subdomain TEXT,
            model TEXT,
            request_agent TEXT NOT NULL DEFAULT '',
            requested_model TEXT NOT NULL DEFAULT '',
            actual_model TEXT NOT NULL DEFAULT '',
            actual_model_source TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL,
            status_code INTEGER,
            error_message TEXT,
            latency_ms INTEGER,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            usage_amount_usd TEXT,
            created_at TEXT NOT NULL,
            settled_at TEXT,
            user_country TEXT,
            user_country_iso3 TEXT,
            synced_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_leases_installation_id ON leases(installation_id);
        CREATE INDEX IF NOT EXISTS idx_leases_subdomain ON leases(subdomain);
        CREATE INDEX IF NOT EXISTS idx_installation_client_tunnels_owner ON installation_client_tunnels(owner_email, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_shares_installation_id ON shares(installation_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_shares_subdomain_unique ON shares(subdomain) WHERE subdomain IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_share_request_logs_share_id ON share_request_logs(share_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_image_jobs_share_queued ON image_generation_jobs(share_id, queued_at DESC);
        CREATE INDEX IF NOT EXISTS idx_image_jobs_provider_queued ON image_generation_jobs(provider_id, queued_at DESC);
        CREATE INDEX IF NOT EXISTS idx_image_jobs_installation_queued ON image_generation_jobs(installation_id, queued_at DESC);
        CREATE INDEX IF NOT EXISTS idx_image_jobs_status_queued ON image_generation_jobs(status, queued_at DESC);
        CREATE INDEX IF NOT EXISTS idx_image_jobs_expires ON image_generation_jobs(expires_at);
        CREATE INDEX IF NOT EXISTS idx_image_request_logs_share_created ON image_generation_request_logs(share_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_image_request_logs_provider_created ON image_generation_request_logs(provider_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_image_request_logs_status_created ON image_generation_request_logs(status, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_share_health_checks ON share_health_checks(share_id, checked_at DESC);
        CREATE INDEX IF NOT EXISTS idx_installation_health_checks ON installation_health_checks(installation_id, checked_at DESC);
        CREATE INDEX IF NOT EXISTS idx_share_model_health_checks_share ON share_model_health_checks(share_id, app_type, requested_model, checked_at DESC);
        CREATE INDEX IF NOT EXISTS idx_share_model_health_state_share ON share_model_health_state(share_id, app_type, last_status);
        CREATE INDEX IF NOT EXISTS idx_dashboard_presence_last_seen ON dashboard_presence(last_seen_at DESC);
        CREATE INDEX IF NOT EXISTS idx_dashboard_ux_events_created ON dashboard_ux_events(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_email_send_logs_created_at ON email_send_logs(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_request_nonces_created_at ON request_nonces(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_auth_challenges_email ON email_login_challenges(email_normalized, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_auth_challenges_installation ON email_login_challenges(installation_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_user_sessions_user_id ON user_sessions(user_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_user_sessions_installation ON user_sessions(installation_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_router_markets_status ON router_markets(status, listed, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_router_markets_subdomain ON router_markets(subdomain);
        CREATE INDEX IF NOT EXISTS idx_router_gateways_owner ON router_gateways(owner_email, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_router_gateways_status ON router_gateways(status, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_market_notification_emails_market_created ON market_notification_emails(market_email, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_market_notification_emails_to_created ON market_notification_emails(to_email, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_market_request_logs_market_created ON market_request_logs(market_email, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_market_request_logs_share_created ON market_request_logs(share_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_market_request_logs_created ON market_request_logs(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_market_share_model_failure_state_share ON market_share_model_failure_state(market_email, share_id, app_type, last_status);
        CREATE INDEX IF NOT EXISTS idx_market_share_runtime_states_market_share ON market_share_runtime_states(market_email, share_id);
        CREATE INDEX IF NOT EXISTS idx_market_share_runtime_states_expires ON market_share_runtime_states(expires_at);
        CREATE INDEX IF NOT EXISTS idx_share_market_listing_statuses_market_share ON share_market_listing_statuses(market_email, router_id, share_id);
        CREATE INDEX IF NOT EXISTS idx_share_market_listing_statuses_expires ON share_market_listing_statuses(expires_at);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_market_share_runtime_states_unique
            ON market_share_runtime_states (
                market_email,
                share_id,
                scope,
                kind,
                COALESCE(app_type, ''),
                COALESCE(model_id, ''),
                COALESCE(model_name, '')
            );

        CREATE TABLE IF NOT EXISTS board_messages (
            id TEXT PRIMARY KEY,
            author_kind TEXT NOT NULL,
            author_user_id TEXT,
            author_email TEXT,
            author_label TEXT NOT NULL,
            guest_id TEXT,
            client_ip_hash TEXT,
            body TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'visible',
            pinned_at TEXT,
            featured_at TEXT,
            deleted_by TEXT,
            deleted_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_board_msgs_created ON board_messages(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_board_msgs_pinned ON board_messages(pinned_at DESC) WHERE pinned_at IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_board_msgs_featured ON board_messages(featured_at DESC) WHERE featured_at IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_board_msgs_author_user ON board_messages(author_user_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_board_msgs_guest ON board_messages(guest_id, created_at DESC);

        CREATE TABLE IF NOT EXISTS board_rate_limit (
            scope TEXT NOT NULL,
            bucket_start INTEGER NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (scope, bucket_start)
        );
        CREATE INDEX IF NOT EXISTS idx_board_rate_bucket ON board_rate_limit(bucket_start DESC);

        CREATE TABLE IF NOT EXISTS admin_audit_log (
            id TEXT PRIMARY KEY,
            actor_email TEXT,
            action TEXT NOT NULL,
            payload_json TEXT,
            ip TEXT,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_admin_audit_created ON admin_audit_log(created_at DESC);

        CREATE TABLE IF NOT EXISTS router_map_display_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            settings_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        ",
    )
    .map_err(|e| AppError::Internal(format!("init schema failed: {e}")))?;
    conn.execute(
        "UPDATE router_markets
            SET scopes_json = '[\"market:shares:read\",\"market:proxy:use\",\"market:email:notify\",\"market:request_logs:write\"]'
          WHERE scopes_json = '[]'",
        [],
    )
    .map_err(|e| AppError::Internal(format!("backfill empty market scopes failed: {e}")))?;
    conn.execute(
        "UPDATE router_markets
            SET scopes_json = replace(scopes_json, ']', ',\"market:request_logs:write\"]')
          WHERE instr(scopes_json, 'market:request_logs:write') = 0",
        [],
    )
    .map_err(|e| AppError::Internal(format!("backfill market request log scope failed: {e}")))?;
    conn.execute(
        "UPDATE router_markets
            SET scopes_json = replace(scopes_json, ']', ',\"market:share_states:write\"]')
          WHERE instr(scopes_json, 'market:share_states:write') = 0",
        [],
    )
    .map_err(|e| AppError::Internal(format!("backfill market share state scope failed: {e}")))?;
    conn.execute(
        "UPDATE router_markets
            SET scopes_json = replace(scopes_json, ']', ',\"market:share_states:release\"]')
          WHERE instr(scopes_json, 'market:share_states:release') = 0",
        [],
    )
    .map_err(|e| {
        AppError::Internal(format!(
            "backfill market share state release scope failed: {e}"
        ))
    })?;
    conn.execute(
        "UPDATE router_markets
            SET scopes_json = replace(scopes_json, ']', ',\"market:share_grants:write\"]')
          WHERE instr(scopes_json, 'market:share_grants:write') = 0",
        [],
    )
    .map_err(|e| AppError::Internal(format!("backfill market share grant scope failed: {e}")))?;
    let columns = conn
        .prepare("PRAGMA table_info(installations)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| AppError::Internal(format!("inspect installations schema failed: {e}")))?;
    if !columns.iter().any(|name| name == "last_seen_ip") {
        conn.execute("ALTER TABLE installations ADD COLUMN last_seen_ip TEXT", [])
            .map_err(|e| {
                AppError::Internal(format!("add installations last_seen_ip failed: {e}"))
            })?;
    }
    if !columns.iter().any(|name| name == "owner_email") {
        conn.execute("ALTER TABLE installations ADD COLUMN owner_email TEXT", [])
            .map_err(|e| {
                AppError::Internal(format!("add installations owner_email failed: {e}"))
            })?;
    }
    if !columns.iter().any(|name| name == "owner_verified_at") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN owner_verified_at TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add installations owner_verified_at failed: {e}"))
        })?;
    }
    if !columns.iter().any(|name| name == "country_code") {
        conn.execute("ALTER TABLE installations ADD COLUMN country_code TEXT", [])
            .map_err(|e| {
                AppError::Internal(format!("add installations country_code failed: {e}"))
            })?;
    }
    if !columns.iter().any(|name| name == "country") {
        conn.execute("ALTER TABLE installations ADD COLUMN country TEXT", [])
            .map_err(|e| AppError::Internal(format!("add installations country failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "region") {
        conn.execute("ALTER TABLE installations ADD COLUMN region TEXT", [])
            .map_err(|e| AppError::Internal(format!("add installations region failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "city") {
        conn.execute("ALTER TABLE installations ADD COLUMN city TEXT", [])
            .map_err(|e| AppError::Internal(format!("add installations city failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "latitude") {
        conn.execute("ALTER TABLE installations ADD COLUMN latitude REAL", [])
            .map_err(|e| AppError::Internal(format!("add installations latitude failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "longitude") {
        conn.execute("ALTER TABLE installations ADD COLUMN longitude REAL", [])
            .map_err(|e| AppError::Internal(format!("add installations longitude failed: {e}")))?;
    }
    if !columns
        .iter()
        .any(|name| name == "geo_candidate_country_code")
    {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_country_code TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_country_code failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_country") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_country TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_country failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_region") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_region TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_region failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_city") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_city TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add installations geo_candidate_city failed: {e}"))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_latitude") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_latitude REAL",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_latitude failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_longitude") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_longitude REAL",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_longitude failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_candidate_hits") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_hits INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add installations geo_candidate_hits failed: {e}"))
        })?;
    }
    if !columns
        .iter()
        .any(|name| name == "geo_candidate_first_seen_at")
    {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_candidate_first_seen_at TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!(
                "add installations geo_candidate_first_seen_at failed: {e}"
            ))
        })?;
    }
    if !columns.iter().any(|name| name == "geo_last_changed_at") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN geo_last_changed_at TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add installations geo_last_changed_at failed: {e}"))
        })?;
    }
    if !columns.iter().any(|name| name == "control_secret_b64") {
        conn.execute(
            "ALTER TABLE installations ADD COLUMN control_secret_b64 TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add installations control_secret_b64 failed: {e}"))
        })?;
    }
    let columns = conn
        .prepare("PRAGMA table_info(shares)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| AppError::Internal(format!("inspect shares schema failed: {e}")))?;
    if !columns.iter().any(|name| name == "subdomain") {
        conn.execute("ALTER TABLE shares ADD COLUMN subdomain TEXT", [])
            .map_err(|e| AppError::Internal(format!("add shares subdomain failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "description") {
        conn.execute("ALTER TABLE shares ADD COLUMN description TEXT", [])
            .map_err(|e| AppError::Internal(format!("add shares description failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "owner_email") {
        conn.execute("ALTER TABLE shares ADD COLUMN owner_email TEXT", [])
            .map_err(|e| AppError::Internal(format!("add shares owner_email failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "shared_with_emails_json") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN shared_with_emails_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add shares shared_with_emails_json failed: {e}"))
        })?;
    }
    if !columns.iter().any(|name| name == "market_access_mode") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN market_access_mode TEXT NOT NULL DEFAULT 'selected'",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares market_access_mode failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "access_by_app_json") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN access_by_app_json TEXT NOT NULL DEFAULT '{}'",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares access_by_app_json failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "app_settings_json") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN app_settings_json TEXT NOT NULL DEFAULT '{}'",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares app_settings_json failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "for_sale") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN for_sale TEXT NOT NULL DEFAULT 'No'",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares for_sale failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "sale_market_kind") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN sale_market_kind TEXT NOT NULL DEFAULT 'token'",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares sale_market_kind failed: {e}")))?;
    }
    conn.execute(
        "CREATE TABLE IF NOT EXISTS email_send_logs (
            id TEXT PRIMARY KEY,
            email_type TEXT NOT NULL,
            to_email TEXT NOT NULL,
            provider_message_id TEXT,
            status TEXT NOT NULL,
            error_message TEXT,
            created_at TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create email_send_logs table failed: {e}")))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_email_send_logs_created_at ON email_send_logs(created_at DESC)",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create email_send_logs index failed: {e}")))?;
    if !columns.iter().any(|name| name == "enabled_claude") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN enabled_claude INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares enabled_claude failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "enabled_codex") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN enabled_codex INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares enabled_codex failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "enabled_gemini") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN enabled_gemini INTEGER NOT NULL DEFAULT 0",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares enabled_gemini failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "upstream_provider_json") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN upstream_provider_json TEXT",
            [],
        )
        .map_err(|e| {
            AppError::Internal(format!("add shares upstream_provider_json failed: {e}"))
        })?;
    }
    if !columns.iter().any(|name| name == "app_runtimes_json") {
        conn.execute("ALTER TABLE shares ADD COLUMN app_runtimes_json TEXT", [])
            .map_err(|e| AppError::Internal(format!("add shares app_runtimes_json failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "app_providers_json") {
        conn.execute("ALTER TABLE shares ADD COLUMN app_providers_json TEXT", [])
            .map_err(|e| {
                AppError::Internal(format!("add shares app_providers_json failed: {e}"))
            })?;
    }
    if !columns.iter().any(|name| name == "bindings_json") {
        // Share 的 app/provider 绑定快照。新写入严格要求一个 binding；该列保留
        // nullable 仅用于 SQLite schema 的增量建表过程。
        conn.execute("ALTER TABLE shares ADD COLUMN bindings_json TEXT", [])
            .map_err(|e| AppError::Internal(format!("add shares bindings_json failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "parallel_limit") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN parallel_limit INTEGER NOT NULL DEFAULT 3",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares parallel_limit failed: {e}")))?;
    }
    if !columns.iter().any(|name| name == "runtime_refreshed_at") {
        conn.execute(
            "ALTER TABLE shares ADD COLUMN runtime_refreshed_at TEXT",
            [],
        )
        .map_err(|e| AppError::Internal(format!("add shares runtime_refreshed_at failed: {e}")))?;
    }
    // share_token 已废弃：caller 身份在 router 边界由 user_api_token + email ACL 校验，
    // tunnel→client 用 X-CC-Switch-Share-Id 识别 share。该列对老库可能仍然存在 + 带
    // NOT NULL 约束，新 INSERT 会因此报错，需要 DROP 掉。
    if columns.iter().any(|name| name == "share_token") {
        // 索引必须先删；不然 SQLite 拒绝 DROP COLUMN。
        let _ = conn.execute("DROP INDEX IF EXISTS idx_shares_token", []);
        conn.execute("ALTER TABLE shares DROP COLUMN share_token", [])
            .map_err(|e| AppError::Internal(format!("drop shares.share_token failed: {e}")))?;
    }
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_shares_subdomain_unique ON shares(subdomain) WHERE subdomain IS NOT NULL",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create subdomain unique index failed: {e}")))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_share_edit_requests_share_status ON share_edit_requests(share_id, status, revision DESC)",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create share edit status index failed: {e}")))?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_share_edit_requests_pending_unique ON share_edit_requests(share_id) WHERE status = 'pending'",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create pending share edit unique index failed: {e}")))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_market_disabled_shares_share ON market_disabled_shares(share_id)",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create market disabled share index failed: {e}")))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS market_share_model_failure_state (
            market_email TEXT NOT NULL,
            share_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            requested_model TEXT NOT NULL,
            actual_model TEXT NOT NULL DEFAULT '',
            last_status TEXT NOT NULL,
            last_success_at INTEGER,
            last_failed_at INTEGER,
            last_checked_at INTEGER NOT NULL,
            recent_results_json TEXT NOT NULL DEFAULT '[]',
            error_message TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (market_email, share_id, app_type, requested_model)
        )",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create market failure state table failed: {e}")))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_market_share_model_failure_state_share ON market_share_model_failure_state(market_email, share_id, app_type, last_status)",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create market failure state index failed: {e}")))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS share_market_listing_statuses (
            market_email TEXT NOT NULL,
            router_id TEXT NOT NULL,
            share_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            listing_url TEXT NOT NULL,
            status TEXT NOT NULL,
            sale_mode TEXT,
            filled_seats INTEGER,
            required_seats INTEGER,
            listing_status TEXT,
            expires_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (market_email, router_id, share_id, app_type)
        )",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create listing status table failed: {e}")))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_share_market_listing_statuses_market_share ON share_market_listing_statuses(market_email, router_id, share_id)",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create listing status share index failed: {e}")))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_share_market_listing_statuses_expires ON share_market_listing_statuses(expires_at)",
        [],
    )
    .map_err(|e| AppError::Internal(format!("create listing status expires index failed: {e}")))?;
    add_column_if_missing(
        conn,
        "share_request_logs",
        "request_agent",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        conn,
        "share_request_logs",
        "requested_model",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        conn,
        "share_request_logs",
        "actual_model",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        conn,
        "share_request_logs",
        "actual_model_source",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(conn, "share_request_logs", "user_country", "TEXT")?;
    add_column_if_missing(conn, "share_request_logs", "user_country_iso3", "TEXT")?;
    add_column_if_missing(conn, "share_request_logs", "user_email", "TEXT")?;
    add_column_if_missing(
        conn,
        "share_request_logs",
        "is_health_check",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "market_request_logs",
        "request_agent",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        conn,
        "market_request_logs",
        "requested_model",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        conn,
        "market_request_logs",
        "actual_model",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        conn,
        "market_request_logs",
        "actual_model_source",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(conn, "user_api_tokens", "token_plaintext", "TEXT")?;
    add_column_if_missing(conn, "market_request_logs", "error_message", "TEXT")?;
    add_column_if_missing(conn, "market_request_logs", "user_country", "TEXT")?;
    add_column_if_missing(conn, "market_request_logs", "user_country_iso3", "TEXT")?;
    add_column_if_missing(
        conn,
        "image_generation_request_logs",
        "result_storage_key",
        "TEXT",
    )?;
    add_column_if_missing(
        conn,
        "image_generation_request_logs",
        "result_access_token",
        "TEXT",
    )?;
    add_column_if_missing(
        conn,
        "router_markets",
        "market_kind",
        "TEXT NOT NULL DEFAULT 'usage'",
    )?;
    add_column_if_missing(conn, "router_markets", "pricing_json", "TEXT")?;
    add_column_if_missing(
        conn,
        "router_markets",
        "maintenance_enabled",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(conn, "router_markets", "maintenance_message", "TEXT")?;
    conn.execute(
        "UPDATE installations
         SET owner_email = (
                 SELECT s.owner_email
                 FROM shares s
                 WHERE s.installation_id = installations.id
                   AND s.owner_email IS NOT NULL
                   AND s.owner_email != ''
                 ORDER BY s.created_at DESC
                 LIMIT 1
             ),
             owner_verified_at = COALESCE(owner_verified_at, last_seen_at)
         WHERE (owner_email IS NULL OR owner_email = '')
           AND EXISTS (
                 SELECT 1
                 FROM shares s
                 WHERE s.installation_id = installations.id
                   AND s.owner_email IS NOT NULL
                   AND s.owner_email != ''
             )",
        [],
    )
    .map_err(|e| AppError::Internal(format!("backfill installation owner email failed: {e}")))?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), AppError> {
    let sql = format!("PRAGMA table_info({table})");
    let columns = conn
        .prepare(&sql)
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| AppError::Internal(format!("inspect {table} schema failed: {e}")))?;
    if !columns.iter().any(|name| name == column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )
        .map_err(|e| AppError::Internal(format!("add {table}.{column} failed: {e}")))?;
    }
    Ok(())
}

fn get_installation(
    conn: &Connection,
    installation_id: &str,
) -> Result<Option<Installation>, AppError> {
    conn.query_row(
        "SELECT id, public_key, platform, app_version, owner_email, owner_verified_at, last_seen_ip, country_code, country, region, city, latitude, longitude,
                geo_candidate_country_code, geo_candidate_country, geo_candidate_region, geo_candidate_city,
                geo_candidate_latitude, geo_candidate_longitude, geo_candidate_hits, geo_candidate_first_seen_at,
                geo_last_changed_at, created_at, last_seen_at
         FROM installations WHERE id = ?1",
        params![installation_id],
        |row| {
            Ok(Installation {
                id: row.get(0)?,
                public_key: row.get(1)?,
                platform: row.get(2)?,
                app_version: row.get(3)?,
                owner_email: row.get(4)?,
                owner_verified_at: row
                    .get::<_, Option<String>>(5)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                last_seen_ip: row.get(6)?,
                country_code: row.get(7)?,
                country: row.get(8)?,
                region: row.get(9)?,
                city: row.get(10)?,
                latitude: row.get(11)?,
                longitude: row.get(12)?,
                geo_candidate_country_code: row.get(13)?,
                geo_candidate_country: row.get(14)?,
                geo_candidate_region: row.get(15)?,
                geo_candidate_city: row.get(16)?,
                geo_candidate_latitude: row.get(17)?,
                geo_candidate_longitude: row.get(18)?,
                geo_candidate_hits: row.get(19)?,
                geo_candidate_first_seen_at: row
                    .get::<_, Option<String>>(20)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                geo_last_changed_at: row
                    .get::<_, Option<String>>(21)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                created_at: parse_dt_sql(&row.get::<_, String>(22)?)?,
                last_seen_at: parse_dt_sql(&row.get::<_, String>(23)?)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query installation failed: {e}")))
}

fn find_installation_id_by_public_key(
    conn: &Connection,
    public_key: &str,
) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT id
         FROM installations
         WHERE public_key = ?1
         ORDER BY last_seen_at DESC, created_at DESC
         LIMIT 1",
        params![public_key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query installation by public key failed: {e}")))
}

fn get_lease_by_connection_id(
    conn: &Connection,
    connection_id: &str,
) -> Result<Option<TunnelLease>, AppError> {
    conn.query_row(
        "SELECT id, installation_id, connection_id, subdomain, tunnel_type, ssh_username,
                ssh_password, issued_at, expires_at, used_at, share_json
         FROM leases WHERE connection_id = ?1",
        params![connection_id],
        map_lease_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query lease failed: {e}")))
}

fn list_installations(conn: &Connection) -> Result<Vec<Installation>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, public_key, platform, app_version, owner_email, owner_verified_at, last_seen_ip, country_code, country, region, city, latitude, longitude,
                    geo_candidate_country_code, geo_candidate_country, geo_candidate_region, geo_candidate_city,
                    geo_candidate_latitude, geo_candidate_longitude, geo_candidate_hits, geo_candidate_first_seen_at,
                    geo_last_changed_at, created_at, last_seen_at
             FROM installations ORDER BY last_seen_at DESC",
        )
        .map_err(|e| AppError::Internal(format!("prepare installations failed: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Installation {
                id: row.get(0)?,
                public_key: row.get(1)?,
                platform: row.get(2)?,
                app_version: row.get(3)?,
                owner_email: row.get(4)?,
                owner_verified_at: row
                    .get::<_, Option<String>>(5)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                last_seen_ip: row.get(6)?,
                country_code: row.get(7)?,
                country: row.get(8)?,
                region: row.get(9)?,
                city: row.get(10)?,
                latitude: row.get(11)?,
                longitude: row.get(12)?,
                geo_candidate_country_code: row.get(13)?,
                geo_candidate_country: row.get(14)?,
                geo_candidate_region: row.get(15)?,
                geo_candidate_city: row.get(16)?,
                geo_candidate_latitude: row.get(17)?,
                geo_candidate_longitude: row.get(18)?,
                geo_candidate_hits: row.get(19)?,
                geo_candidate_first_seen_at: row
                    .get::<_, Option<String>>(20)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                geo_last_changed_at: row
                    .get::<_, Option<String>>(21)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                created_at: parse_dt_sql(&row.get::<_, String>(22)?)?,
                last_seen_at: parse_dt_sql(&row.get::<_, String>(23)?)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("query installations failed: {e}")))?;
    collect_rows(rows)
}

fn public_payout_profile_for_installation(
    conn: &Connection,
    installation_id: &str,
) -> Result<Option<PublicPayoutProfileResponse>, AppError> {
    conn.query_row(
        "SELECT i.owner_email, i.created_at, p.revision, p.profile_json,
                p.source_updated_at_ms
         FROM installations i
         LEFT JOIN installation_payout_profiles p ON p.installation_id = i.id
         WHERE i.id = ?1",
        params![installation_id],
        |row| {
            let owner_email = row.get::<_, Option<String>>(0)?;
            let created_at = parse_dt_sql(&row.get::<_, String>(1)?)?;
            let revision = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            let profile_json = row.get::<_, Option<String>>(3)?;
            let source_updated_at_ms = row.get::<_, Option<i64>>(4)?;
            let profile = profile_json
                .as_deref()
                .map(serde_json::from_str::<PayoutProfile>)
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            let updated_at = source_updated_at_ms
                .and_then(DateTime::<Utc>::from_timestamp_millis)
                .unwrap_or(created_at);
            Ok(PublicPayoutProfileResponse {
                schema_version: crate::models::PAYOUT_PROFILE_SCHEMA_VERSION,
                revision,
                configured: profile.is_some(),
                owner_email,
                installation_id: installation_id.to_string(),
                profile,
                updated_at,
            })
        },
    )
    .optional()
    .map_err(|error| AppError::Internal(format!("query public payout profile failed: {error}")))
}

fn list_dashboard_payout_profiles(
    conn: &Connection,
) -> Result<HashMap<String, DashboardPayoutProfileView>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT installation_id, profile_json, source_updated_at_ms
             FROM installation_payout_profiles
             WHERE profile_json IS NOT NULL",
        )
        .map_err(|error| {
            AppError::Internal(format!("prepare dashboard payout profiles failed: {error}"))
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| {
            AppError::Internal(format!("query dashboard payout profiles failed: {error}"))
        })?;
    let mut profiles = HashMap::new();
    for row in rows {
        let (installation_id, profile_json, source_updated_at_ms) = row.map_err(|error| {
            AppError::Internal(format!("read dashboard payout profile failed: {error}"))
        })?;
        let Ok(profile) = serde_json::from_str::<PayoutProfile>(&profile_json) else {
            tracing::error!(
                installation_id = %installation_id,
                "invalid stored payout profile omitted from dashboard"
            );
            continue;
        };
        let Some(updated_at) = DateTime::<Utc>::from_timestamp_millis(source_updated_at_ms) else {
            tracing::error!(
                installation_id = %installation_id,
                "invalid stored payout profile timestamp omitted from dashboard"
            );
            continue;
        };
        profiles.insert(
            installation_id,
            DashboardPayoutProfileView {
                address_type: profile.address_type,
                address: profile.address,
                token: profile.token,
                networks: profile.networks,
                verification_status: profile.verification_status,
                updated_at,
            },
        );
    }
    Ok(profiles)
}

fn get_installation_geo_state(
    conn: &Connection,
    installation_id: &str,
) -> Result<Option<InstallationGeoState>, AppError> {
    conn.query_row(
        "SELECT last_seen_ip, country_code, latitude, longitude,
                geo_candidate_country_code, geo_candidate_country, geo_candidate_region, geo_candidate_city,
                geo_candidate_latitude, geo_candidate_longitude, geo_candidate_hits,
                geo_candidate_first_seen_at, geo_last_changed_at
         FROM installations WHERE id = ?1",
        params![installation_id],
        |row| {
            Ok(InstallationGeoState {
                last_seen_ip: row.get(0)?,
                country_code: row.get(1)?,
                latitude: row.get(2)?,
                longitude: row.get(3)?,
                geo_candidate_country_code: row.get(4)?,
                geo_candidate_latitude: row.get(8)?,
                geo_candidate_longitude: row.get(9)?,
                geo_candidate_hits: row.get(10)?,
                geo_candidate_first_seen_at: row
                    .get::<_, Option<String>>(11)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
                geo_last_changed_at: row
                    .get::<_, Option<String>>(12)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query installation geo state failed: {e}")))
}

fn touch_installation_presence(
    conn: &Connection,
    installation_id: &str,
    metadata: &ClientMetadata,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE installations
         SET last_seen_at = ?2,
             last_seen_ip = COALESCE(?3, last_seen_ip),
             country_code = COALESCE(?4, country_code)
         WHERE id = ?1",
        params![
            installation_id,
            now.to_rfc3339(),
            metadata.ip.as_deref(),
            metadata.country_code.as_deref(),
        ],
    )
    .map_err(|e| AppError::Internal(format!("update installation failed: {e}")))?;
    Ok(())
}

fn repeat_vars(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

fn should_refresh_installation_geo(installation: &Installation, next_ip: Option<&str>) -> bool {
    let Some(next_ip) = next_ip.map(str::trim).filter(|v| !v.is_empty()) else {
        return false;
    };
    installation.last_seen_ip.as_deref() != Some(next_ip)
        || installation.latitude.is_none()
        || installation.longitude.is_none()
}

fn deduplicate_dashboard_installations(
    installations: Vec<Installation>,
    active_share_subdomains_by_installation: &HashMap<String, HashSet<String>>,
) -> Vec<Installation> {
    let mut deduped = Vec::with_capacity(installations.len());
    let mut seen = HashMap::<String, usize>::new();

    for installation in installations {
        let key = installation.public_key.clone();
        match seen.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(deduped.len());
                deduped.push(installation);
            }
            Entry::Occupied(entry) => {
                let existing = &mut deduped[*entry.get()];
                if prefer_dashboard_installation(
                    &installation,
                    existing,
                    active_share_subdomains_by_installation,
                ) {
                    *existing = installation;
                }
            }
        }
    }

    deduped.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));
    deduped
}

fn prefer_dashboard_installation(
    candidate: &Installation,
    existing: &Installation,
    active_share_subdomains_by_installation: &HashMap<String, HashSet<String>>,
) -> bool {
    let candidate_has_share = active_share_subdomains_by_installation
        .get(&candidate.id)
        .map(|subdomains| !subdomains.is_empty())
        .unwrap_or(false);
    let existing_has_share = active_share_subdomains_by_installation
        .get(&existing.id)
        .map(|subdomains| !subdomains.is_empty())
        .unwrap_or(false);
    if candidate_has_share != existing_has_share {
        return candidate_has_share;
    }

    if candidate.last_seen_at != existing.last_seen_at {
        return candidate.last_seen_at > existing.last_seen_at;
    }

    candidate.created_at > existing.created_at
}

fn haversine_distance_km(
    lat1: Option<f64>,
    lon1: Option<f64>,
    lat2: Option<f64>,
    lon2: Option<f64>,
) -> Option<f64> {
    let (lat1, lon1, lat2, lon2) = (lat1?, lon1?, lat2?, lon2?);
    let to_rad = |deg: f64| deg.to_radians();
    let dlat = to_rad(lat2 - lat1);
    let dlon = to_rad(lon2 - lon1);
    let lat1 = to_rad(lat1);
    let lat2 = to_rad(lat2);
    let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    Some(6371.0 * c)
}

fn persist_candidate_geo(
    conn: &Connection,
    installation_id: &str,
    geo: &GeoLookupResult,
    hits: i64,
    first_seen_at: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE installations
         SET geo_candidate_country_code = ?2,
             geo_candidate_country = ?3,
             geo_candidate_region = ?4,
             geo_candidate_city = ?5,
             geo_candidate_latitude = ?6,
             geo_candidate_longitude = ?7,
             geo_candidate_hits = ?8,
             geo_candidate_first_seen_at = ?9
         WHERE id = ?1",
        params![
            installation_id,
            geo.country_code,
            geo.country,
            geo.region,
            geo.city,
            geo.latitude,
            geo.longitude,
            hits,
            first_seen_at.to_rfc3339(),
        ],
    )
    .map_err(|e| AppError::Internal(format!("update installation candidate geo failed: {e}")))?;
    Ok(())
}

fn persist_stable_geo(
    conn: &Connection,
    installation_id: &str,
    geo: &GeoLookupResult,
    changed_at: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE installations
         SET country_code = COALESCE(?2, country_code),
             country = COALESCE(?3, country),
             region = COALESCE(?4, region),
             city = COALESCE(?5, city),
             latitude = COALESCE(?6, latitude),
             longitude = COALESCE(?7, longitude),
             geo_candidate_country_code = NULL,
             geo_candidate_country = NULL,
             geo_candidate_region = NULL,
             geo_candidate_city = NULL,
             geo_candidate_latitude = NULL,
             geo_candidate_longitude = NULL,
             geo_candidate_hits = 0,
             geo_candidate_first_seen_at = NULL,
             geo_last_changed_at = ?8
         WHERE id = ?1",
        params![
            installation_id,
            geo.country_code,
            geo.country,
            geo.region,
            geo.city,
            geo.latitude,
            geo.longitude,
            changed_at.to_rfc3339(),
        ],
    )
    .map_err(|e| AppError::Internal(format!("update installation stable geo failed: {e}")))?;
    Ok(())
}

async fn lookup_ip_im_geo(ip: &str) -> Option<GeoLookupResult> {
    let url = format!("https://ip.im/{ip}");
    let client = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1")
        .timeout(StdDuration::from_secs(3))
        .build()
        .ok()?;
    let response = timeout(StdDuration::from_secs(4), client.get(url).send())
        .await
        .ok()?
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().await.ok()?;
    parse_ip_im_geo(&body)
}

fn parse_ip_im_geo(body: &str) -> Option<GeoLookupResult> {
    let mut result = GeoLookupResult {
        country_code: None,
        country: None,
        region: None,
        city: None,
        latitude: None,
        longitude: None,
    };

    for raw_line in body.lines() {
        let line = raw_line.trim();
        if let Some(value) = line.strip_prefix("Country:") {
            let value = value.trim();
            if value.len() == 2 && value.chars().all(|ch| ch.is_ascii_alphabetic()) {
                result.country_code = Some(value.to_ascii_uppercase());
            } else if !value.is_empty() {
                result.country = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Region:") {
            let value = value.trim();
            if !value.is_empty() {
                result.region = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("City:") {
            let value = value.trim();
            if !value.is_empty() {
                result.city = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Loc:") {
            let value = value.trim();
            if let Some((lat, lon)) = value.split_once(',') {
                result.latitude = lat.trim().parse().ok();
                result.longitude = lon.trim().parse().ok();
            }
        }
    }

    if result.latitude.is_none() || result.longitude.is_none() {
        return None;
    }
    Some(result)
}

fn list_shares(conn: &Connection) -> Result<Vec<(String, ShareDescriptor)>, AppError> {
    let mut stmt = conn
        .prepare(
        "SELECT s.installation_id, s.share_id, s.share_name, s.description, s.for_sale, s.sale_market_kind, s.market_access_mode, COALESCE(s.access_by_app_json, '{}'), COALESCE(s.app_settings_json, '{}'), COALESCE(s.subdomain, '-'), s.app_type, s.provider_id,
                    s.owner_email, s.shared_with_emails_json,
                    s.enabled_claude, s.enabled_codex, s.enabled_gemini,
                    s.token_limit, s.parallel_limit, s.tokens_used, s.requests_count, s.share_status, s.created_at, s.expires_at, s.upstream_provider_json, s.app_runtimes_json, s.app_providers_json,
                    s.bindings_json
             FROM shares s
             ORDER BY s.share_name ASC",
        )
        .map_err(|e| AppError::Internal(format!("prepare shares failed: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                ShareDescriptor {
                    share_id: row.get(1)?,
                    share_name: row.get(2)?,
                    description: row.get(3)?,
                    for_sale: row.get(4)?,
                    sale_market_kind: row.get(5)?,
                    market_access_mode: row.get(6)?,
                    access_by_app: parse_share_access_by_app(row.get(7)?)?,
                    app_settings: parse_share_app_settings(row.get(8)?)?,
                    for_sale_official_price_percent_by_app: Default::default(),
                    subdomain: row.get(9)?,
                    app_type: row.get(10)?,
                    provider_id: row.get(11)?,
                    bindings: parse_share_bindings(row.get(27)?)?,
                    owner_email: row.get(12)?,
                    shared_with_emails: parse_string_vec(row.get(13)?)?,
                    support: ShareSupport {
                        claude: row.get::<_, i64>(14)? != 0,
                        codex: row.get::<_, i64>(15)? != 0,
                        gemini: row.get::<_, i64>(16)? != 0,
                    },
                    token_limit: row.get(17)?,
                    parallel_limit: row.get(18)?,
                    tokens_used: row.get(19)?,
                    requests_count: row.get(20)?,
                    share_status: row.get(21)?,
                    created_at: row.get(22)?,
                    expires_at: row.get(23)?,
                    upstream_provider: parse_upstream_provider(row.get(24)?)?,
                    app_runtimes: parse_app_runtimes(row.get(25)?)?,
                    app_providers: parse_app_providers(row.get(26)?)?,
                    market_grant: None,
                    app_availability: ShareAppAvailability::default(),
                    model_health: ShareModelHealthSummary::default(),
                },
            ))
        })
        .map_err(|e| AppError::Internal(format!("query shares failed: {e}")))?;
    collect_rows(rows)
}

fn list_active_share_edits(conn: &Connection) -> Result<HashMap<String, ShareEditView>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, share_id, installation_id, revision, status, patch_json, created_by_email, created_at, updated_at, applied_at, error_message
             FROM share_edit_requests
             WHERE status IN ('pending', 'rejected')
             ORDER BY revision DESC",
        )
        .map_err(|e| AppError::Internal(format!("prepare active share edits failed: {e}")))?;
    let rows = stmt
        .query_map([], share_edit_from_row)
        .map_err(|e| AppError::Internal(format!("query active share edits failed: {e}")))?;
    let mut result = HashMap::new();
    for edit in collect_rows(rows)? {
        result.entry(edit.share_id.clone()).or_insert(edit);
    }
    Ok(result)
}

fn get_active_share_edit(
    conn: &Connection,
    share_id: &str,
) -> Result<Option<ShareEditView>, AppError> {
    conn.query_row(
        "SELECT id, share_id, installation_id, revision, status, patch_json, created_by_email, created_at, updated_at, applied_at, error_message
         FROM share_edit_requests
         WHERE share_id = ?1 AND status = 'pending'
         ORDER BY revision DESC
         LIMIT 1",
        params![share_id],
        share_edit_from_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query active share edit failed: {e}")))
}

fn get_share_edit_by_id(conn: &Connection, id: &str) -> Result<Option<ShareEditView>, AppError> {
    conn.query_row(
        "SELECT id, share_id, installation_id, revision, status, patch_json, created_by_email, created_at, updated_at, applied_at, error_message
         FROM share_edit_requests
         WHERE id = ?1",
        params![id],
        share_edit_from_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query share edit failed: {e}")))
}

fn list_pending_share_edits_for_installation(
    conn: &Connection,
    installation_id: &str,
    share_ids: &[String],
) -> Result<Vec<ShareEditView>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, share_id, installation_id, revision, status, patch_json, created_by_email, created_at, updated_at, applied_at, error_message
             FROM share_edit_requests
             WHERE installation_id = ?1 AND status = 'pending'
             ORDER BY revision ASC",
        )
        .map_err(|e| AppError::Internal(format!("prepare pending share edits failed: {e}")))?;
    let rows = stmt
        .query_map(params![installation_id], share_edit_from_row)
        .map_err(|e| AppError::Internal(format!("query pending share edits failed: {e}")))?;
    let requested = share_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<HashSet<_>>();
    let mut edits = collect_rows(rows)?;
    if !requested.is_empty() {
        edits.retain(|edit| requested.contains(&edit.share_id));
    }
    Ok(edits)
}

fn next_share_edit_revision(conn: &Connection, share_id: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM share_edit_requests WHERE share_id = ?1",
        params![share_id],
        |row| row.get(0),
    )
    .map_err(|e| AppError::Internal(format!("query next share edit revision failed: {e}")))
}

fn share_edit_from_row(row: &rusqlite::Row<'_>) -> Result<ShareEditView, rusqlite::Error> {
    let patch_json: String = row.get(5)?;
    let patch = serde_json::from_str::<ShareSettingsPatch>(&patch_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let created_at = parse_rfc3339_row(row.get::<_, String>(7)?, 7)?;
    let updated_at = parse_rfc3339_row(row.get::<_, String>(8)?, 8)?;
    let applied_at = match row.get::<_, Option<String>>(9)? {
        Some(value) => Some(parse_rfc3339_row(value, 9)?),
        None => None,
    };
    Ok(ShareEditView {
        id: row.get(0)?,
        share_id: row.get(1)?,
        installation_id: row.get(2)?,
        revision: row.get(3)?,
        status: row.get(4)?,
        patch,
        created_by_email: row.get(6)?,
        created_at,
        updated_at,
        applied_at,
        error_message: row.get(10)?,
    })
}

fn parse_rfc3339_row(value: String, index: usize) -> Result<DateTime<Utc>, rusqlite::Error> {
    DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })
}

fn share_settings_patch_is_empty(patch: &ShareSettingsPatch) -> bool {
    patch.owner_email.is_none()
        && patch.description.is_none()
        && patch.for_sale.is_none()
        && patch.sale_market_kind.is_none()
        && patch.market_access_mode.is_none()
        && patch.shared_with_emails.is_none()
        && patch.access_by_app.is_none()
        && patch.app_settings.is_none()
        && patch.for_sale_official_price_percent_by_app.is_none()
        && patch.token_limit.is_none()
        && patch.parallel_limit.is_none()
        && patch.expires_at.is_none()
        && patch.auto_start.is_none()
}

/// Verifies that the descriptor the client reported back through the control
/// RPC actually reflects every field the pending edit requested. Returns the
/// name of the first field that does not match, or `Ok(())` if the client
/// honored the whole patch. The server uses this to refuse writing a partial
/// application (the desktop bug where an owner transfer dropped the demoted
/// owner from `shareto`) instead of silently persisting it.
///
/// `auto_start` is intentionally not checked: it is not part of
/// `ShareDescriptor`, so the report cannot confirm it.
fn validate_returned_share_against_patch(
    patch: &ShareSettingsPatch,
    share: &ShareDescriptor,
) -> Result<(), &'static str> {
    if let Some(owner) = patch.owner_email.as_deref() {
        let want = normalize_email(owner).map_err(|_| "ownerEmail")?;
        let got = share
            .owner_email
            .as_deref()
            .and_then(|value| normalize_email(value).ok())
            .unwrap_or_default();
        if want != got {
            return Err("ownerEmail");
        }
    }
    if let Some(list) = patch.shared_with_emails.as_ref() {
        let got: std::collections::HashSet<String> = share
            .shared_with_emails
            .iter()
            .filter_map(|value| normalize_email(value).ok())
            .collect();
        for email in list {
            if let Ok(normalized) = normalize_email(email) {
                if !got.contains(&normalized) {
                    return Err("sharedWithEmails");
                }
            }
        }
    }
    if let Some(access_by_app) = patch.access_by_app.as_ref() {
        for (app, access) in access_by_app {
            let got = share.access_by_app.get(app).ok_or("accessByApp")?;
            if got.market_access_mode != access.market_access_mode {
                return Err("accessByApp");
            }
            let got_emails: std::collections::HashSet<String> = got
                .shared_with_emails
                .iter()
                .filter_map(|value| normalize_email(value).ok())
                .collect();
            for email in &access.shared_with_emails {
                if let Ok(normalized) = normalize_email(email) {
                    if !got_emails.contains(&normalized) {
                        return Err("accessByApp");
                    }
                }
            }
        }
    }
    if let Some(app_settings) = patch.app_settings.as_ref() {
        for (app, setting) in app_settings {
            let got = share.app_settings.get(app).ok_or("appSettings")?;
            if got.for_sale != setting.for_sale
                || got.sale_market_kind != setting.sale_market_kind
                || got.market_access_mode != setting.market_access_mode
                || got.token_limit != setting.token_limit
                || got.parallel_limit != setting.parallel_limit
            {
                return Err("appSettings");
            }
            let got_emails: std::collections::HashSet<String> = got
                .shared_with_emails
                .iter()
                .filter_map(|value| normalize_email(value).ok())
                .collect();
            for email in &setting.shared_with_emails {
                if let Ok(normalized) = normalize_email(email) {
                    if !got_emails.contains(&normalized) {
                        return Err("appSettings");
                    }
                }
            }
            if !setting.expires_at.is_empty() {
                let matches = match (
                    DateTime::parse_from_rfc3339(&setting.expires_at),
                    DateTime::parse_from_rfc3339(&got.expires_at),
                ) {
                    (Ok(want), Ok(got)) => want.with_timezone(&Utc) == got.with_timezone(&Utc),
                    _ => setting.expires_at == got.expires_at,
                };
                if !matches {
                    return Err("appSettings");
                }
            }
        }
    }
    if let Some(for_sale) = patch.for_sale.as_deref() {
        if for_sale != share.for_sale {
            return Err("forSale");
        }
    }
    if let Some(sale_market_kind) = patch.sale_market_kind.as_deref() {
        if sale_market_kind != share.sale_market_kind {
            return Err("saleMarketKind");
        }
    }
    if let Some(mode) = patch.market_access_mode.as_deref() {
        if mode != share.market_access_mode {
            return Err("marketAccessMode");
        }
    }
    if let Some(token_limit) = patch.token_limit {
        if token_limit != share.token_limit {
            return Err("tokenLimit");
        }
    }
    if let Some(parallel_limit) = patch.parallel_limit {
        if parallel_limit != share.parallel_limit {
            return Err("parallelLimit");
        }
    }
    if let Some(description) = patch.description.as_ref() {
        // patch.description is Option<Option<String>>: outer Some means "set",
        // inner None means "clear". Both sides are already normalized.
        if description.as_deref() != share.description.as_deref() {
            return Err("description");
        }
    }
    if let Some(expires_at) = patch.expires_at.as_deref() {
        let matches = match (
            DateTime::parse_from_rfc3339(expires_at),
            DateTime::parse_from_rfc3339(&share.expires_at),
        ) {
            (Ok(want), Ok(got)) => want.with_timezone(&Utc) == got.with_timezone(&Utc),
            _ => expires_at == share.expires_at,
        };
        if !matches {
            return Err("expiresAt");
        }
    }
    if let Some(pricing) = patch.for_sale_official_price_percent_by_app.as_ref() {
        for (app, percent) in pricing {
            if share.for_sale_official_price_percent_by_app.get(app) != Some(percent) {
                return Err("forSaleOfficialPricePercentByApp");
            }
        }
    }
    Ok(())
}

fn normalize_share_settings_patch(
    patch: ShareSettingsPatch,
    owner_email: Option<&str>,
    current_shared_with_emails: Option<&[String]>,
    current_sale_market_kind: Option<&str>,
) -> Result<ShareSettingsPatch, AppError> {
    if patch.owner_email.is_some() {
        return Err(AppError::Conflict(
            "share owner is managed by the installation owner".into(),
        ));
    }
    let current_owner_email = owner_email.unwrap_or("");
    let next_owner_email = None;
    let sale_market_kind = match patch.sale_market_kind {
        Some(value) => Some(normalize_sale_market_kind(&value)?),
        None => None,
    };
    let current_sale_market_kind = current_sale_market_kind
        .map(normalize_sale_market_kind)
        .transpose()?;
    let allow_owner_in_acl = sale_market_kind
        .as_deref()
        .or(current_sale_market_kind.as_deref())
        == Some("share");
    let effective_owner_email = next_owner_email.as_deref().unwrap_or(current_owner_email);
    if let Some(next_owner_email) = next_owner_email.as_deref() {
        if next_owner_email == current_owner_email {
            return Err(AppError::BadRequest(
                "new share owner email must be different".into(),
            ));
        }
        let current_shared_normalized = normalize_email_list(
            current_shared_with_emails.unwrap_or(&[]),
            current_owner_email,
        );
        if !current_shared_normalized
            .iter()
            .any(|email| email == next_owner_email)
        {
            return Err(AppError::BadRequest(
                "new share owner must already be a shareto email".into(),
            ));
        }
    }
    let shared_with_emails = match patch.shared_with_emails {
        Some(values) => {
            let mut normalized = normalize_email_list_with_options(
                &values,
                effective_owner_email,
                allow_owner_in_acl,
            );
            if next_owner_email.is_some()
                && !current_owner_email.is_empty()
                && !normalized.iter().any(|email| email == current_owner_email)
            {
                normalized.push(current_owner_email.to_string());
                normalized.sort();
            }
            Some(normalized)
        }
        None if next_owner_email.is_some() => {
            let mut normalized = normalize_email_list(
                current_shared_with_emails.unwrap_or(&[]),
                effective_owner_email,
            );
            if !current_owner_email.is_empty()
                && !normalized.iter().any(|email| email == current_owner_email)
            {
                normalized.push(current_owner_email.to_string());
                normalized.sort();
            }
            Some(normalized)
        }
        None => None,
    };
    let access_by_app = match patch.access_by_app {
        Some(values) => {
            let mut normalized = BTreeMap::new();
            for (app, access) in values {
                let app = normalize_share_acl_app(&app)?;
                let mut emails = normalize_email_list_with_options(
                    &access.shared_with_emails,
                    effective_owner_email,
                    allow_owner_in_acl,
                );
                if next_owner_email.is_some()
                    && !current_owner_email.is_empty()
                    && !emails.iter().any(|email| email == current_owner_email)
                {
                    emails.push(current_owner_email.to_string());
                    emails.sort();
                }
                normalized.insert(
                    app,
                    ShareAppAccess {
                        shared_with_emails: emails,
                        market_access_mode: normalize_market_access_mode(
                            &access.market_access_mode,
                        )?,
                    },
                );
            }
            Some(normalized)
        }
        None => None,
    };
    let app_settings = match patch.app_settings {
        Some(values) => Some(normalize_share_app_settings(values, effective_owner_email)?),
        None => None,
    };
    let pricing = match patch.for_sale_official_price_percent_by_app {
        Some(values) => {
            let mut normalized = BTreeMap::new();
            for (app, percent) in values {
                let app = app.trim().to_ascii_lowercase();
                if !matches!(app.as_str(), "claude" | "codex" | "gemini") {
                    return Err(AppError::BadRequest(
                        "official price percent app must be claude, codex, or gemini".into(),
                    ));
                }
                if !(1..=100).contains(&percent) {
                    return Err(AppError::BadRequest(
                        "official price percent must be between 1 and 100".into(),
                    ));
                }
                normalized.insert(app, percent);
            }
            Some(normalized)
        }
        None => None,
    };
    if let Some(token_limit) = patch.token_limit {
        if token_limit <= 0 && token_limit != -1 {
            return Err(AppError::BadRequest(
                "tokenLimit must be positive or -1".into(),
            ));
        }
    }
    if let Some(parallel_limit) = patch.parallel_limit {
        if parallel_limit != -1 && parallel_limit < 3 {
            return Err(AppError::BadRequest(
                "parallelLimit must be at least 3 or -1".into(),
            ));
        }
    }
    if let Some(expires_at) = patch.expires_at.as_deref() {
        let expires = DateTime::parse_from_rfc3339(expires_at)
            .map_err(|_| AppError::BadRequest("expiresAt must be RFC3339".into()))?
            .with_timezone(&Utc);
        if expires <= Utc::now() {
            return Err(AppError::BadRequest(
                "expiresAt must be in the future".into(),
            ));
        }
    }
    Ok(ShareSettingsPatch {
        owner_email: next_owner_email,
        description: match patch.description {
            Some(value) => Some(normalize_share_description(value)?),
            None => None,
        },
        for_sale: match patch.for_sale {
            Some(value) => Some(normalize_share_for_sale(&value)?),
            None => None,
        },
        sale_market_kind,
        market_access_mode: match patch.market_access_mode {
            Some(value) => Some(normalize_market_access_mode(&value)?),
            None => None,
        },
        shared_with_emails,
        access_by_app,
        app_settings,
        for_sale_official_price_percent_by_app: pricing,
        token_limit: patch.token_limit,
        parallel_limit: patch.parallel_limit,
        expires_at: patch.expires_at,
        auto_start: patch.auto_start,
    })
}

fn normalize_share_description(description: Option<String>) -> Result<Option<String>, AppError> {
    let Some(description) = description else {
        return Ok(None);
    };
    let trimmed = description.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > 200 {
        return Err(AppError::BadRequest(
            "share description must be 200 characters or fewer".into(),
        ));
    }
    Ok(Some(trimmed.to_string()))
}

fn normalize_share_for_sale(value: &str) -> Result<String, AppError> {
    match value.trim() {
        "No" => Ok("No".to_string()),
        "Yes" => Ok("Yes".to_string()),
        "Free" => Ok("Free".to_string()),
        _ => Err(AppError::BadRequest(
            "share for_sale must be Yes, No, or Free".into(),
        )),
    }
}

fn normalize_sale_market_kind(value: &str) -> Result<String, AppError> {
    match value.trim() {
        "token" => Ok("token".to_string()),
        "share" => Ok("share".to_string()),
        _ => Err(AppError::BadRequest(
            "share sale_market_kind must be token or share".into(),
        )),
    }
}

fn normalize_market_access_mode(value: &str) -> Result<String, AppError> {
    match value.trim() {
        "selected" => Ok("selected".to_string()),
        "all" => Ok("all".to_string()),
        _ => Err(AppError::BadRequest(
            "share market_access_mode must be selected or all".into(),
        )),
    }
}

fn normalize_share_acl_app(value: &str) -> Result<String, AppError> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "claude" | "codex" | "gemini" => Ok(value),
        _ => Err(AppError::BadRequest(
            "share accessByApp key must be claude, codex, or gemini".into(),
        )),
    }
}

fn normalize_share_app_settings(
    values: BTreeMap<String, ShareAppSettings>,
    owner_email: &str,
) -> Result<BTreeMap<String, ShareAppSettings>, AppError> {
    let mut normalized = BTreeMap::new();
    for (app, setting) in values {
        let app = normalize_share_acl_app(&app)?;
        let for_sale = normalize_share_for_sale(&setting.for_sale)?;
        let sale_market_kind = normalize_sale_market_kind(&setting.sale_market_kind)?;
        let market_access_mode = normalize_market_access_mode(&setting.market_access_mode)?;
        let allow_owner = sale_market_kind == "share";
        let shared_with_emails = normalize_email_list_with_options(
            &setting.shared_with_emails,
            owner_email,
            allow_owner,
        );
        if for_sale == "Yes" && sale_market_kind == "share" && shared_with_emails.is_empty() {
            return Err(AppError::BadRequest(format!(
                "{app} ShareMarket sale must explicitly delegate to one ShareMarket"
            )));
        }
        if setting.token_limit <= 0 && setting.token_limit != -1 {
            return Err(AppError::BadRequest(format!(
                "{app} tokenLimit must be positive or -1"
            )));
        }
        if setting.parallel_limit != -1 && setting.parallel_limit < 3 {
            return Err(AppError::BadRequest(format!(
                "{app} parallelLimit must be at least 3 or -1"
            )));
        }
        if !setting.expires_at.trim().is_empty() {
            DateTime::parse_from_rfc3339(&setting.expires_at)
                .map_err(|_| AppError::BadRequest(format!("{app} expiresAt must be RFC3339")))?;
        }
        normalized.insert(
            app,
            ShareAppSettings {
                for_sale,
                sale_market_kind,
                market_access_mode,
                shared_with_emails,
                token_limit: setting.token_limit,
                parallel_limit: setting.parallel_limit,
                expires_at: setting.expires_at,
            },
        );
    }
    Ok(normalized)
}

struct UsagePeriodWindow {
    period: String,
    bucket_granularity: String,
    days: u32,
    start_at: DateTime<Utc>,
    bucket_keys: Vec<String>,
}

fn normalize_usage_period(value: &str) -> Result<UsagePeriodWindow, AppError> {
    let now = Utc::now();
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "24h" | "1d" | "1天" => {
            let start_at = now - Duration::hours(24);
            let bucket_keys = usage_hour_keys(start_at, now);
            Ok(UsagePeriodWindow {
                period: "24h".to_string(),
                bucket_granularity: "hour".to_string(),
                days: usage_date_keys(start_at, now).len() as u32,
                start_at,
                bucket_keys,
            })
        }
        "1w" | "7d" => usage_day_window("1w", 7, now),
        "30d" | "30天" => usage_day_window("30d", 30, now),
        _ => Err(AppError::BadRequest(
            "usage period must be 24h, 1w, or 30d".into(),
        )),
    }
}

fn usage_day_window(
    period: &str,
    days: u32,
    now: DateTime<Utc>,
) -> Result<UsagePeriodWindow, AppError> {
    let today = now.date_naive();
    let start_date = today - Duration::days(i64::from(days.saturating_sub(1)));
    let start_at = start_date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| AppError::Internal("invalid usage start date".into()))?
        .and_utc();
    let date_keys = (0..days)
        .map(|offset| {
            (start_date + Duration::days(i64::from(offset)))
                .format("%Y-%m-%d")
                .to_string()
        })
        .collect::<Vec<_>>();
    Ok(UsagePeriodWindow {
        period: period.to_string(),
        bucket_granularity: "day".to_string(),
        days,
        start_at,
        bucket_keys: date_keys,
    })
}

fn usage_hour_keys(start_at: DateTime<Utc>, end_at: DateTime<Utc>) -> Vec<String> {
    let mut bucket = floor_to_utc_hour(start_at);
    let end = floor_to_utc_hour(end_at);
    let mut keys = Vec::new();
    while bucket <= end {
        keys.push(bucket.format("%Y-%m-%dT%H:00:00Z").to_string());
        bucket += Duration::hours(1);
    }
    keys
}

fn floor_to_utc_hour(value: DateTime<Utc>) -> DateTime<Utc> {
    let timestamp = value.timestamp();
    let floored = timestamp - timestamp.rem_euclid(3600);
    DateTime::<Utc>::from_timestamp(floored, 0).unwrap_or(value)
}

fn usage_date_keys(start_at: DateTime<Utc>, end_at: DateTime<Utc>) -> Vec<String> {
    let mut date = start_at.date_naive();
    let end = end_at.date_naive();
    let mut keys = Vec::new();
    while date <= end {
        keys.push(date.format("%Y-%m-%d").to_string());
        date += Duration::days(1);
    }
    keys
}

fn normalize_usage_email(value: &str) -> Option<String> {
    normalize_email(value).ok()
}

fn parse_upstream_provider(
    value: Option<String>,
) -> Result<Option<ShareUpstreamProvider>, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&value).map(Some).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn parse_app_runtimes(value: Option<String>) -> Result<ShareAppRuntimes, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(ShareAppRuntimes::default());
    };
    if value.trim().is_empty() {
        return Ok(ShareAppRuntimes::default());
    }
    serde_json::from_str(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

/// 反序列化 share 的单一 app/provider binding JSON 列。
/// 空字符串 / NULL 只作为数据库读取的防御性兜底；新写入不会产生空 binding。
fn parse_share_bindings(
    value: Option<String>,
) -> Result<BTreeMap<String, String>, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    if value.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn parse_share_access_by_app(
    value: Option<String>,
) -> Result<BTreeMap<String, ShareAppAccess>, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    if value.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn parse_share_app_settings(
    value: Option<String>,
) -> Result<BTreeMap<String, ShareAppSettings>, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    if value.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn share_descriptor_apps(share: &ShareDescriptor) -> Vec<&'static str> {
    const APPS: &[&str] = &["claude", "codex", "gemini"];
    let bound = APPS
        .iter()
        .copied()
        .filter(|app| share.bindings.contains_key(*app))
        .collect::<Vec<_>>();
    if bound.is_empty() {
        APPS.to_vec()
    } else {
        bound
    }
}

fn effective_share_app_settings(share: &ShareDescriptor) -> BTreeMap<String, ShareAppSettings> {
    let mut out = BTreeMap::new();
    for app in share_descriptor_apps(share) {
        let access = share.access_by_app.get(app);
        let mut setting =
            share
                .app_settings
                .get(app)
                .cloned()
                .unwrap_or_else(|| ShareAppSettings {
                    for_sale: share.for_sale.clone(),
                    sale_market_kind: share.sale_market_kind.clone(),
                    market_access_mode: access
                        .map(|entry| entry.market_access_mode.clone())
                        .unwrap_or_else(|| share.market_access_mode.clone()),
                    shared_with_emails: access
                        .map(|entry| entry.shared_with_emails.clone())
                        .unwrap_or_else(|| share.shared_with_emails.clone()),
                    token_limit: share.token_limit,
                    parallel_limit: share.parallel_limit,
                    expires_at: share.expires_at.clone(),
                });
        if let Some(access) = access {
            setting.market_access_mode = access.market_access_mode.clone();
            setting.shared_with_emails = access.shared_with_emails.clone();
        }
        out.insert(app.to_string(), setting);
    }
    out
}

fn effective_app_settings_from_parts(
    app_settings: &BTreeMap<String, ShareAppSettings>,
    access_by_app: &BTreeMap<String, ShareAppAccess>,
    shared_with_emails: &[String],
    market_access_mode: &str,
    for_sale: &str,
    sale_market_kind: &str,
    token_limit: i64,
    parallel_limit: i64,
    expires_at: &str,
) -> BTreeMap<String, ShareAppSettings> {
    ["claude", "codex", "gemini"]
        .into_iter()
        .map(|app| {
            (
                app.to_string(),
                effective_app_setting_from_parts(
                    app,
                    app_settings,
                    access_by_app,
                    shared_with_emails,
                    market_access_mode,
                    for_sale,
                    sale_market_kind,
                    token_limit,
                    parallel_limit,
                    expires_at,
                ),
            )
        })
        .collect()
}

fn effective_app_setting_from_parts(
    app: &str,
    app_settings: &BTreeMap<String, ShareAppSettings>,
    access_by_app: &BTreeMap<String, ShareAppAccess>,
    shared_with_emails: &[String],
    market_access_mode: &str,
    for_sale: &str,
    sale_market_kind: &str,
    token_limit: i64,
    parallel_limit: i64,
    expires_at: &str,
) -> ShareAppSettings {
    let access = access_by_app.get(app);
    let mut setting = app_settings
        .get(app)
        .cloned()
        .unwrap_or_else(|| ShareAppSettings {
            for_sale: for_sale.to_string(),
            sale_market_kind: sale_market_kind.to_string(),
            market_access_mode: access
                .map(|entry| entry.market_access_mode.clone())
                .unwrap_or_else(|| market_access_mode.to_string()),
            shared_with_emails: access
                .map(|entry| entry.shared_with_emails.clone())
                .unwrap_or_else(|| shared_with_emails.to_vec()),
            token_limit,
            parallel_limit,
            expires_at: expires_at.to_string(),
        });
    if let Some(access) = access {
        setting.market_access_mode = access.market_access_mode.clone();
        setting.shared_with_emails = access.shared_with_emails.clone();
    }
    setting
}

fn parse_app_providers(value: Option<String>) -> Result<ShareAppProviders, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(ShareAppProviders::default());
    };
    if value.trim().is_empty() {
        return Ok(ShareAppProviders::default());
    }
    serde_json::from_str(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn parse_json_value(value: Option<String>) -> Result<Option<serde_json::Value>, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&value).map(Some).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn normalize_model_health_status(status: &str) -> &'static str {
    if status.eq_ignore_ascii_case("success") {
        "success"
    } else if status.eq_ignore_ascii_case("skipped") {
        "skipped"
    } else {
        "failed"
    }
}

fn record_runtime_model_health_snapshot_conn(
    conn: &Connection,
    snapshot: &ShareRuntimeSnapshotResponse,
) -> Result<(), AppError> {
    let subdomain = conn
        .query_row(
            "SELECT subdomain FROM shares WHERE share_id = ?1",
            params![snapshot.share_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read share subdomain for health failed: {e}")))?
        .unwrap_or_default();

    // Defense in depth: old cc-switch clients (and any future bug on the
    // sender side) may push model_health entries for app_types that the share
    // isn't actually bound to. The dashboard treats whatever lands in
    // share_model_health_checks as authoritative, so we filter on intake by
    // the canonical binding table to keep the view honest.
    let bound_app_types = load_share_bound_app_types(conn, &snapshot.share_id)?;

    let mut current_models_by_app = HashMap::<String, HashSet<String>>::new();
    for summary in snapshot
        .model_health
        .claude
        .iter()
        .chain(snapshot.model_health.codex.iter())
        .chain(snapshot.model_health.gemini.iter())
    {
        if !bound_app_types.contains(summary.app_type.as_str()) {
            // The share doesn't bind this app right now — drop the entry.
            // We also wipe any leftover history below, so a previously-bound
            // app stops showing up in the dashboard once it's unbound.
            continue;
        }
        let checked_at = summary.last_checked_at.unwrap_or(snapshot.queried_at);
        let requested_model = if summary.requested_model.trim().is_empty() {
            summary.app_type.clone()
        } else {
            summary.requested_model.clone()
        };
        current_models_by_app
            .entry(summary.app_type.clone())
            .or_default()
            .insert(model_health_key(&requested_model));
        let actual_model = if summary.actual_model.trim().is_empty() {
            requested_model.clone()
        } else {
            summary.actual_model.clone()
        };
        let check = ShareModelHealthCheckEntry {
            request_id: format!(
                "cc-switch-health:{}:{}:{}:{}",
                snapshot.share_id, summary.app_type, requested_model, checked_at
            ),
            share_id: snapshot.share_id.clone(),
            subdomain: subdomain.clone(),
            app_type: summary.app_type.clone(),
            requested_model,
            actual_model,
            status: summary.status.clone(),
            status_code: summary.status_code,
            latency_ms: summary.latency_ms,
            first_token_ms: None,
            error_message: summary.error_message.clone(),
            checked_at,
            source: summary
                .source
                .clone()
                .unwrap_or_else(|| "cc-switch-scheduled".to_string()),
        };
        record_share_model_health_check_conn(conn, &check)?;
        if !summary.recent_results.is_empty() {
            let recent_json = serde_json::to_string(&summary.recent_results).map_err(|e| {
                AppError::Internal(format!(
                    "serialize runtime model health results failed: {e}"
                ))
            })?;
            conn.execute(
                "UPDATE share_model_health_state
                 SET recent_results_json = ?4
                 WHERE share_id = ?1
                   AND app_type = ?2
                   AND requested_model = ?3
                   AND last_checked_at <= ?5",
                params![
                    snapshot.share_id,
                    summary.app_type,
                    check.requested_model,
                    recent_json,
                    checked_at,
                ],
            )
            .map_err(|e| AppError::Internal(format!("update model health results failed: {e}")))?;
        }
    }
    for (app_type, current_models) in current_models_by_app {
        if current_models.is_empty() {
            continue;
        }
        let placeholders = repeat_vars(current_models.len());
        let mut values = Vec::<String>::with_capacity(current_models.len() + 3);
        values.push(snapshot.share_id.clone());
        values.push(app_type);
        values.push(snapshot.queried_at.to_string());
        values.extend(current_models.into_iter());
        conn.execute(
            &format!(
                "DELETE FROM share_model_health_state
                 WHERE share_id = ?
                   AND app_type = ?
                   AND last_checked_at <= CAST(? AS INTEGER)
                   AND lower(trim(requested_model)) NOT IN ({placeholders})",
            ),
            params_from_iter(values),
        )
        .map_err(|e| AppError::Internal(format!("delete stale model health state failed: {e}")))?;
    }

    // Clean up any historical rows for app_types this share no longer binds.
    // Without this the dashboard would keep displaying e.g. codex health
    // checks long after the user unbound codex (they'd persist until the share
    // itself is deleted).
    purge_unbound_model_health(conn, &snapshot.share_id, &bound_app_types)?;
    Ok(())
}

/// Read the set of app_types this share currently has a non-empty binding on.
fn load_share_bound_app_types(
    conn: &Connection,
    share_id: &str,
) -> Result<HashSet<String>, AppError> {
    let bindings_json: Option<String> = conn
        .query_row(
            "SELECT bindings_json FROM shares WHERE share_id = ?1",
            params![share_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read share bindings_json failed: {e}")))?
        .flatten();
    let map = parse_share_bindings(bindings_json)
        .map_err(|e| AppError::Internal(format!("decode share bindings failed: {e}")))?;
    Ok(map
        .into_iter()
        .filter(|(_, provider_id)| !provider_id.trim().is_empty())
        .map(|(app, _)| app)
        .collect())
}

/// Delete model_health rows for apps this share no longer binds. Called on
/// every snapshot intake so unbinding an app eventually clears its dashboard
/// history without waiting for the share itself to be deleted.
fn purge_unbound_model_health(
    conn: &Connection,
    share_id: &str,
    bound_app_types: &HashSet<String>,
) -> Result<(), AppError> {
    if bound_app_types.is_empty() {
        // Share currently has zero bindings — wipe both tables for this share.
        conn.execute(
            "DELETE FROM share_model_health_checks WHERE share_id = ?1",
            params![share_id],
        )
        .map_err(|e| AppError::Internal(format!("purge model health checks failed: {e}")))?;
        conn.execute(
            "DELETE FROM share_model_health_state WHERE share_id = ?1",
            params![share_id],
        )
        .map_err(|e| AppError::Internal(format!("purge model health state failed: {e}")))?;
        return Ok(());
    }

    let placeholders = repeat_vars(bound_app_types.len());
    let mut values = Vec::<String>::with_capacity(bound_app_types.len() + 1);
    values.push(share_id.to_string());
    values.extend(bound_app_types.iter().cloned());

    conn.execute(
        &format!(
            "DELETE FROM share_model_health_checks
             WHERE share_id = ?
               AND app_type NOT IN ({placeholders})",
        ),
        params_from_iter(values.clone()),
    )
    .map_err(|e| AppError::Internal(format!("purge unbound model health checks failed: {e}")))?;
    conn.execute(
        &format!(
            "DELETE FROM share_model_health_state
             WHERE share_id = ?
               AND app_type NOT IN ({placeholders})",
        ),
        params_from_iter(values),
    )
    .map_err(|e| AppError::Internal(format!("purge unbound model health state failed: {e}")))?;
    Ok(())
}

fn record_share_model_health_check_conn(
    conn: &Connection,
    check: &ShareModelHealthCheckEntry,
) -> Result<(), AppError> {
    let status = normalize_model_health_status(&check.status);
    conn.execute(
        "INSERT INTO share_model_health_checks (
            request_id, share_id, subdomain, app_type, requested_model, actual_model,
            status, status_code, latency_ms, first_token_ms, error_message, checked_at, source
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(request_id) DO UPDATE SET
            status = excluded.status,
            status_code = excluded.status_code,
            latency_ms = excluded.latency_ms,
            first_token_ms = excluded.first_token_ms,
            error_message = excluded.error_message,
            checked_at = excluded.checked_at,
            source = excluded.source",
        params![
            check.request_id,
            check.share_id,
            check.subdomain,
            check.app_type,
            check.requested_model,
            check.actual_model,
            status,
            check.status_code.map(i64::from),
            check.latency_ms as i64,
            check.first_token_ms.map(|v| v as i64),
            check.error_message,
            check.checked_at,
            check.source,
        ],
    )
    .map_err(|e| AppError::Internal(format!("insert model health check failed: {e}")))?;
    let recent_results = recent_model_health_results(
        conn,
        &check.share_id,
        &check.app_type,
        &check.requested_model,
    )?;
    let recent_json = serde_json::to_string(&recent_results)
        .map_err(|e| AppError::Internal(format!("serialize model health results failed: {e}")))?;
    conn.execute(
        "INSERT INTO share_model_health_state (
            share_id, app_type, requested_model, actual_model, last_status,
            last_success_at, last_failed_at, last_checked_at, recent_results_json,
            error_message, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5,
            CASE WHEN ?5 = 'success' THEN ?6 ELSE NULL END,
            CASE WHEN ?5 = 'failed' THEN ?6 ELSE NULL END,
            ?6, ?7, ?8, ?6)
         ON CONFLICT(share_id, app_type, requested_model) DO UPDATE SET
            actual_model = CASE WHEN excluded.last_checked_at >= share_model_health_state.last_checked_at THEN excluded.actual_model ELSE share_model_health_state.actual_model END,
            last_status = CASE WHEN excluded.last_checked_at >= share_model_health_state.last_checked_at THEN excluded.last_status ELSE share_model_health_state.last_status END,
            last_success_at = CASE
                WHEN excluded.last_status = 'success'
                 AND (share_model_health_state.last_success_at IS NULL OR excluded.last_checked_at > share_model_health_state.last_success_at)
                THEN excluded.last_checked_at
                ELSE share_model_health_state.last_success_at
            END,
            last_failed_at = CASE
                WHEN excluded.last_status = 'failed'
                 AND (share_model_health_state.last_failed_at IS NULL OR excluded.last_checked_at > share_model_health_state.last_failed_at)
                THEN excluded.last_checked_at
                ELSE share_model_health_state.last_failed_at
            END,
            last_checked_at = max(share_model_health_state.last_checked_at, excluded.last_checked_at),
            recent_results_json = CASE WHEN excluded.last_checked_at >= share_model_health_state.last_checked_at THEN excluded.recent_results_json ELSE share_model_health_state.recent_results_json END,
            error_message = CASE WHEN excluded.last_checked_at >= share_model_health_state.last_checked_at THEN excluded.error_message ELSE share_model_health_state.error_message END,
            updated_at = max(share_model_health_state.updated_at, excluded.updated_at)",
        params![
            check.share_id,
            check.app_type,
            check.requested_model,
            check.actual_model,
            status,
            check.checked_at,
            recent_json,
            check.error_message,
        ],
    )
    .map_err(|e| AppError::Internal(format!("upsert model health state failed: {e}")))?;
    conn.execute(
        "DELETE FROM share_model_health_checks WHERE checked_at < ?1",
        params![Utc::now().timestamp() - 7 * 86_400],
    )
    .map_err(|e| AppError::Internal(format!("prune model health checks failed: {e}")))?;
    Ok(())
}

fn recent_model_health_results(
    conn: &Connection,
    share_id: &str,
    app_type: &str,
    requested_model: &str,
) -> Result<Vec<String>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT status
             FROM share_model_health_checks
             WHERE share_id = ?1 AND app_type = ?2 AND requested_model = ?3
             ORDER BY checked_at DESC, id DESC
             LIMIT 3",
        )
        .map_err(|e| {
            AppError::Internal(format!("prepare recent model health results failed: {e}"))
        })?;
    let rows = stmt
        .query_map(params![share_id, app_type, requested_model], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| {
            AppError::Internal(format!("query recent model health results failed: {e}"))
        })?;
    collect_rows(rows)
}

fn parse_recent_results(value: String) -> Result<Vec<String>, rusqlite::Error> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn market_app_availability_status(last_status: &str, recent_results: &[String]) -> String {
    if recent_results.len() >= 3 && recent_results.iter().all(|status| status == "failed") {
        "unavailable".to_string()
    } else if matches!(last_status, "failed" | "degraded")
        || recent_results
            .iter()
            .any(|status| matches!(status.as_str(), "failed" | "degraded"))
    {
        "degraded".to_string()
    } else if last_status == "success" {
        "available".to_string()
    } else {
        "unknown".to_string()
    }
}

fn market_app_availability_rank(status: &str) -> u8 {
    match status {
        "unavailable" => 3,
        "degraded" => 2,
        "available" => 1,
        _ => 0,
    }
}

fn should_replace_market_app_availability(
    current: Option<&MarketAppAvailabilityEntry>,
    candidate: &MarketAppAvailabilityEntry,
) -> bool {
    let Some(current) = current else {
        return true;
    };
    let candidate_rank = market_app_availability_rank(&candidate.status);
    let current_rank = market_app_availability_rank(&current.status);
    let candidate_checked_at = candidate.last_checked_at.unwrap_or_default();
    let current_checked_at = current.last_checked_at.unwrap_or_default();
    candidate_checked_at > current_checked_at
        || (candidate_checked_at == current_checked_at && candidate_rank > current_rank)
}

fn set_market_app_availability_entry(
    availability: &mut MarketAppAvailability,
    app_type: &str,
    entry: MarketAppAvailabilityEntry,
) {
    let slot = match app_type {
        "claude" => &mut availability.claude,
        "codex" => &mut availability.codex,
        "gemini" => &mut availability.gemini,
        _ => return,
    };
    if should_replace_market_app_availability(slot.as_ref(), &entry) {
        *slot = Some(entry);
    }
}

fn normalize_market_share_priority_app(app: &str) -> Result<String, AppError> {
    let normalized = app.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "claude" | "codex" | "gemini") {
        Ok(normalized)
    } else {
        Err(AppError::BadRequest(
            "app must be one of claude, codex, gemini".into(),
        ))
    }
}

fn compute_market_share_quota_health(
    signal_app: Option<&str>,
    runtimes: &ShareAppRuntimes,
    upstream_provider: Option<&ShareUpstreamProvider>,
    now: DateTime<Utc>,
) -> f64 {
    if let Some(app) = signal_app {
        return runtime_provider_for_app(runtimes, app)
            .and_then(|provider| market_provider_quota_health(provider, now))
            .or_else(|| {
                upstream_provider.and_then(|provider| market_provider_quota_health(provider, now))
            })
            .unwrap_or_else(|| crate::scheduling_signals::compute_quota_health(None, now));
    }

    let runtime_healths = runtime_quota_healths(runtimes, now);
    if !runtime_healths.is_empty() {
        return runtime_healths
            .into_iter()
            .fold(f64::INFINITY, f64::min)
            .clamp(0.0, 1.5);
    }

    upstream_provider
        .and_then(|provider| market_provider_quota_health(provider, now))
        .unwrap_or_else(|| crate::scheduling_signals::compute_quota_health(None, now))
}

fn runtime_provider_for_app<'a>(
    runtimes: &'a ShareAppRuntimes,
    app: &str,
) -> Option<&'a ShareUpstreamProvider> {
    match app {
        "claude" => runtimes.claude.as_ref(),
        "codex" => runtimes.codex.as_ref(),
        "gemini" => runtimes.gemini.as_ref(),
        "kiro" => runtimes.kiro.as_ref(),
        "cursor" => runtimes.cursor.as_ref(),
        "antigravity" => runtimes.antigravity.as_ref(),
        "copilot" => runtimes.copilot.as_ref(),
        _ => None,
    }
}

fn runtime_quota_healths(runtimes: &ShareAppRuntimes, now: DateTime<Utc>) -> Vec<f64> {
    [
        runtimes.claude.as_ref(),
        runtimes.codex.as_ref(),
        runtimes.gemini.as_ref(),
        runtimes.kiro.as_ref(),
        runtimes.cursor.as_ref(),
        runtimes.antigravity.as_ref(),
        runtimes.copilot.as_ref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|provider| market_provider_quota_health(provider, now))
    .collect()
}

fn market_provider_quota_health(
    provider: &ShareUpstreamProvider,
    now: DateTime<Utc>,
) -> Option<f64> {
    if provider_has_display_only_quota(provider) {
        Some(crate::scheduling_signals::compute_quota_health(None, now))
    } else {
        provider
            .quota
            .as_ref()
            .map(|quota| market_quota_health(quota, now))
    }
}

fn market_quota_health(quota: &ShareUpstreamQuota, now: DateTime<Utc>) -> f64 {
    if quota_block_is_active(quota, now) || quota_dispatch_limit_reached(quota, now) {
        0.0
    } else {
        crate::scheduling_signals::compute_quota_health(Some(quota), now)
    }
}

fn provider_has_display_only_quota(provider: &ShareUpstreamProvider) -> bool {
    let provider_type = provider
        .provider_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if provider_type == "ollama_cloud" {
        return true;
    }

    let provider_name = provider
        .provider_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    provider.kind == "official_oauth" && provider_name.contains("ollama")
}

fn sort_market_shares_for_app(shares: &mut [MarketShareView], app: &str) {
    shares.sort_by(|left, right| {
        let left_item = market_share_priority_sort_item(left, app);
        let right_item = market_share_priority_sort_item(right, app);
        right_item
            .schedulable
            .cmp(&left_item.schedulable)
            .then_with(|| left_item.degraded.cmp(&right_item.degraded))
            .then_with(|| {
                right_item
                    .score
                    .partial_cmp(&left_item.score)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.active_requests.cmp(&right.active_requests))
            .then_with(|| {
                market_share_sort_name(left)
                    .to_ascii_lowercase()
                    .cmp(&market_share_sort_name(right).to_ascii_lowercase())
            })
    });
}

struct MarketSharePrioritySortItem {
    schedulable: bool,
    degraded: bool,
    score: f64,
}

fn market_share_priority_sort_item(
    share: &MarketShareView,
    app: &str,
) -> MarketSharePrioritySortItem {
    let supported = market_share_supports_app(share, app);
    let availability = market_app_availability_for_app(&share.app_availability, app);
    let parallel_full =
        share.parallel_limit > 0 && share.active_requests as i64 >= share.parallel_limit;
    let blocked_by_market_state = share
        .market_states
        .iter()
        .any(|state| market_state_blocks_app(state, app));
    let schedulable = supported
        && share.online
        && !share.disabled_by_market
        && !parallel_full
        && !blocked_by_market_state
        && availability
            .map(|entry| !entry.status.eq_ignore_ascii_case("unavailable"))
            .unwrap_or(true);
    MarketSharePrioritySortItem {
        schedulable,
        degraded: availability
            .map(|entry| entry.status.eq_ignore_ascii_case("degraded"))
            .unwrap_or(false),
        score: market_share_priority_score(share),
    }
}

fn market_share_supports_app(share: &MarketShareView, app: &str) -> bool {
    match app {
        "claude" => share.support.claude || share.app_runtimes.claude.is_some(),
        "codex" => share.support.codex || share.app_runtimes.codex.is_some(),
        "gemini" => share.support.gemini || share.app_runtimes.gemini.is_some(),
        _ => false,
    }
}

fn market_app_availability_for_app<'a>(
    availability: &'a MarketAppAvailability,
    app: &str,
) -> Option<&'a MarketAppAvailabilityEntry> {
    match app {
        "claude" => availability.claude.as_ref(),
        "codex" => availability.codex.as_ref(),
        "gemini" => availability.gemini.as_ref(),
        _ => None,
    }
}

fn market_state_blocks_app(state: &MarketShareRuntimeStateView, app: &str) -> bool {
    if state.kind == "cooldown" && state.app_type.is_none() {
        return true;
    }
    state
        .app_type
        .as_deref()
        .map(|state_app| state_app.eq_ignore_ascii_case(app))
        .unwrap_or(false)
        && matches!(
            state.kind.as_str(),
            "cooldown" | "model_block" | "capability_block"
        )
}

fn market_share_priority_score(share: &MarketShareView) -> f64 {
    let headroom = if share.parallel_limit <= 0 {
        1.0
    } else {
        (1.0 - share.active_requests as f64 / share.parallel_limit as f64).clamp(0.0, 1.0)
    };
    (0.35 * share.signals.stability + 0.30 * share.signals.quota_health + 0.25 * headroom + 0.10)
        * share.signals.owner_penalty
}

fn market_share_sort_name(share: &MarketShareView) -> &str {
    if !share.subdomain.is_empty() {
        &share.subdomain
    } else if !share.share_name.is_empty() {
        &share.share_name
    } else {
        &share.share_id
    }
}

fn apply_quota_blocks_to_app_availability(
    availability: &mut MarketAppAvailability,
    runtimes: &ShareAppRuntimes,
    model_health: &ShareModelHealthSummary,
    now: DateTime<Utc>,
) {
    for (app_type, provider, health) in [
        (
            "claude",
            runtimes.claude.as_ref(),
            model_health.claude.as_slice(),
        ),
        (
            "codex",
            runtimes.codex.as_ref(),
            model_health.codex.as_slice(),
        ),
        (
            "gemini",
            runtimes.gemini.as_ref(),
            model_health.gemini.as_slice(),
        ),
    ] {
        if let Some(entry) = quota_blocked_app_availability(app_type, provider, health, now) {
            set_market_app_availability_entry(availability, app_type, entry);
        }
    }
}

fn quota_blocked_app_availability(
    app_type: &str,
    provider: Option<&ShareUpstreamProvider>,
    health: &[ModelHealthSummary],
    now: DateTime<Utc>,
) -> Option<MarketAppAvailabilityEntry> {
    if provider
        .map(provider_has_display_only_quota)
        .unwrap_or(false)
    {
        return None;
    }
    if let Some(quota) = provider.and_then(|provider| provider.quota.as_ref()) {
        if quota_block_is_active(quota, now) || quota_dispatch_limit_reached(quota, now) {
            let reason = if quota_block_is_active(quota, now) {
                quota
                    .blocked_reason
                    .clone()
                    .or_else(|| quota.availability.clone())
                    .unwrap_or_else(|| "quota exhausted".to_string())
            } else {
                quota_dispatch_limit_reason(quota)
            };
            return Some(MarketAppAvailabilityEntry {
                status: "unavailable".to_string(),
                reason: Some(reason),
                requested_model: Some(app_type.to_string()),
                actual_model: Some(app_type.to_string()),
                last_checked_at: Some(now.timestamp()),
                recent_results: vec!["quota_blocked".to_string()],
            });
        }
    }

    health
        .iter()
        .filter(|entry| {
            let is_quota_block = entry.status.eq_ignore_ascii_case("quota_blocked")
                || (is_app_level_model_health(entry, app_type)
                    && entry.status.eq_ignore_ascii_case("failed")
                    && model_health_error_is_quota_block(entry.error_message.as_deref()));
            is_quota_block && !quota_block_expired(entry.error_message.as_deref(), now)
        })
        .max_by_key(|entry| entry.last_checked_at.unwrap_or_default())
        .map(|entry| MarketAppAvailabilityEntry {
            status: "unavailable".to_string(),
            reason: entry
                .error_message
                .clone()
                .or_else(|| Some("quota exhausted".to_string())),
            requested_model: Some(entry.requested_model.clone()),
            actual_model: Some(entry.actual_model.clone()),
            last_checked_at: Some(now.timestamp()),
            recent_results: vec!["quota_blocked".to_string()],
        })
}

fn market_app_availability_penalty(availability: &MarketAppAvailability) -> f64 {
    [
        &availability.claude,
        &availability.codex,
        &availability.gemini,
    ]
    .into_iter()
    .filter_map(|entry| entry.as_ref())
    .map(|entry| match entry.status.as_str() {
        "unavailable" => 0.25,
        "degraded" => 0.7,
        _ => 1.0,
    })
    .fold(1.0_f64, f64::min)
}

fn list_market_app_availability(
    conn: &Connection,
    market_email: &str,
) -> Result<HashMap<String, MarketAppAvailability>, AppError> {
    let fresh_after = Utc::now().timestamp() - MARKET_APP_AVAILABILITY_FAILURE_TTL_SECS;
    let mut stmt = conn
        .prepare(
            "SELECT share_id, app_type, requested_model, actual_model, last_status,
                    last_checked_at, recent_results_json, error_message
             FROM market_share_model_failure_state
             WHERE lower(market_email) = lower(?1)
               AND (last_status = 'success' OR last_checked_at >= ?2)",
        )
        .map_err(|e| AppError::Internal(format!("prepare market app availability failed: {e}")))?;
    let rows = stmt
        .query_map(params![market_email, fresh_after], |row| {
            let recent_results = parse_recent_results(row.get(6)?)?;
            let last_status: String = row.get(4)?;
            let status = market_app_availability_status(&last_status, &recent_results);
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?.to_ascii_lowercase(),
                MarketAppAvailabilityEntry {
                    status,
                    reason: row.get(7)?,
                    requested_model: row.get(2)?,
                    actual_model: row.get(3)?,
                    last_checked_at: row.get(5)?,
                    recent_results,
                },
            ))
        })
        .map_err(|e| AppError::Internal(format!("query market app availability failed: {e}")))?;
    let mut map = HashMap::<String, MarketAppAvailability>::new();
    for row in rows {
        let (share_id, app_type, entry) = row
            .map_err(|e| AppError::Internal(format!("read market app availability failed: {e}")))?;
        set_market_app_availability_entry(map.entry(share_id).or_default(), &app_type, entry);
    }
    Ok(map)
}

fn list_all_market_app_availability(
    conn: &Connection,
) -> Result<HashMap<String, HashMap<String, MarketAppAvailability>>, AppError> {
    let fresh_after = Utc::now().timestamp() - MARKET_APP_AVAILABILITY_FAILURE_TTL_SECS;
    let mut stmt = conn
        .prepare(
            "SELECT lower(market_email), share_id, app_type, requested_model, actual_model, last_status,
                    last_checked_at, recent_results_json, error_message
             FROM market_share_model_failure_state
             WHERE last_status = 'success' OR last_checked_at >= ?1",
        )
        .map_err(|e| AppError::Internal(format!("prepare all market app availability failed: {e}")))?;
    let rows = stmt
        .query_map(params![fresh_after], |row| {
            let recent_results = parse_recent_results(row.get(7)?)?;
            let last_status: String = row.get(5)?;
            let status = market_app_availability_status(&last_status, &recent_results);
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?.to_ascii_lowercase(),
                MarketAppAvailabilityEntry {
                    status,
                    reason: row.get(8)?,
                    requested_model: row.get(3)?,
                    actual_model: row.get(4)?,
                    last_checked_at: row.get(6)?,
                    recent_results,
                },
            ))
        })
        .map_err(|e| {
            AppError::Internal(format!("query all market app availability failed: {e}"))
        })?;
    let mut map = HashMap::<String, HashMap<String, MarketAppAvailability>>::new();
    for row in rows {
        let (market_email, share_id, app_type, entry) = row.map_err(|e| {
            AppError::Internal(format!("read all market app availability failed: {e}"))
        })?;
        let share_map = map.entry(market_email).or_default();
        set_market_app_availability_entry(share_map.entry(share_id).or_default(), &app_type, entry);
    }
    Ok(map)
}

fn list_model_health_summaries(
    conn: &Connection,
) -> Result<HashMap<String, ShareModelHealthSummary>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT share_id, app_type, requested_model, actual_model, last_status,
                    last_success_at, last_failed_at, last_checked_at, recent_results_json, error_message
             FROM share_model_health_state
             ORDER BY share_id ASC, app_type ASC, requested_model ASC",
        )
        .map_err(|e| AppError::Internal(format!("prepare model health summaries failed: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ModelHealthSummary {
                    app_type: row.get(1)?,
                    requested_model: row.get(2)?,
                    actual_model: row.get(3)?,
                    status: row.get(4)?,
                    last_success_at: row.get(5)?,
                    last_failed_at: row.get(6)?,
                    last_checked_at: row.get(7)?,
                    recent_results: parse_recent_results(row.get(8)?)?,
                    error_message: row.get(9)?,
                    status_code: None,
                    latency_ms: 0,
                    source: None,
                    provider_id: None,
                    provider_name: None,
                },
            ))
        })
        .map_err(|e| AppError::Internal(format!("query model health summaries failed: {e}")))?;
    let mut out = HashMap::<String, ShareModelHealthSummary>::new();
    for row in rows {
        let (share_id, summary) =
            row.map_err(|e| AppError::Internal(format!("read model health summary failed: {e}")))?;
        let bucket = out.entry(share_id).or_default();
        match summary.app_type.as_str() {
            "claude" => bucket.claude.push(summary),
            "codex" => bucket.codex.push(summary),
            "gemini" => bucket.gemini.push(summary),
            _ => {}
        }
    }
    Ok(out)
}

fn filter_app_runtimes_by_model_health(
    mut runtimes: ShareAppRuntimes,
    health: &ShareModelHealthSummary,
    now: DateTime<Utc>,
) -> ShareAppRuntimes {
    runtimes.claude = filter_provider_by_app_health(runtimes.claude, &health.claude, "claude", now)
        .and_then(|provider| filter_provider_models_by_health(Some(provider), &health.claude));
    runtimes.codex = filter_provider_by_app_health(runtimes.codex, &health.codex, "codex", now)
        .and_then(|provider| filter_provider_models_by_health(Some(provider), &health.codex));
    runtimes.gemini = filter_provider_by_app_health(runtimes.gemini, &health.gemini, "gemini", now)
        .and_then(|provider| filter_provider_models_by_health(Some(provider), &health.gemini));
    // OAuth-only providers have no model-level health entries; pass through as-is.
    runtimes.kiro = filter_provider_models_by_health(runtimes.kiro, &[]);
    runtimes.cursor = filter_provider_models_by_health(runtimes.cursor, &[]);
    runtimes.antigravity = filter_provider_models_by_health(runtimes.antigravity, &[]);
    runtimes.copilot = filter_provider_models_by_health(runtimes.copilot, &[]);
    runtimes
}

fn filter_app_runtimes_by_quota(
    mut runtimes: ShareAppRuntimes,
    now: DateTime<Utc>,
) -> ShareAppRuntimes {
    runtimes.claude = filter_provider_by_quota(runtimes.claude, now);
    runtimes.codex = filter_provider_by_quota(runtimes.codex, now);
    runtimes.gemini = filter_provider_by_quota(runtimes.gemini, now);
    runtimes.kiro = filter_provider_by_quota(runtimes.kiro, now);
    runtimes.cursor = filter_provider_by_quota(runtimes.cursor, now);
    runtimes.antigravity = filter_provider_by_quota(runtimes.antigravity, now);
    runtimes.copilot = filter_provider_by_quota(runtimes.copilot, now);
    runtimes
}

fn filter_provider_by_quota(
    provider: Option<ShareUpstreamProvider>,
    now: DateTime<Utc>,
) -> Option<ShareUpstreamProvider> {
    if provider
        .as_ref()
        .map(provider_has_display_only_quota)
        .unwrap_or(false)
    {
        return provider;
    }
    let provider = provider?;
    if provider
        .quota
        .as_ref()
        .map(|quota| quota_block_is_active(quota, now) || quota_dispatch_limit_reached(quota, now))
        .unwrap_or(false)
    {
        None
    } else {
        Some(provider)
    }
}

fn quota_dispatch_limit_reached(quota: &ShareUpstreamQuota, now: DateTime<Utc>) -> bool {
    let Some(limit) = quota.dispatch_limit_percent else {
        return false;
    };
    if limit <= 0.0 {
        return false;
    }
    let limit = limit.clamp(0.0, 100.0);
    quota.tiers.iter().any(|tier| {
        if tier.utilization < limit {
            return false;
        }
        match tier.resets_at.as_deref() {
            Some(value) => DateTime::parse_from_rfc3339(value)
                .map(|dt| dt.with_timezone(&Utc) > now)
                .unwrap_or(true),
            None => true,
        }
    })
}

fn quota_dispatch_limit_reason(quota: &ShareUpstreamQuota) -> String {
    let limit = quota
        .dispatch_limit_percent
        .unwrap_or_default()
        .clamp(0.0, 100.0);
    if limit > 0.0 {
        format!("quota dispatch limit reached ({limit:.0}%)")
    } else {
        "quota dispatch limit reached".to_string()
    }
}

fn quota_block_is_active(quota: &ShareUpstreamQuota, now: DateTime<Utc>) -> bool {
    let availability = quota.availability.as_deref().unwrap_or("available");
    if !matches!(
        availability,
        "short_window_exhausted" | "long_window_exhausted"
    ) {
        return false;
    }
    let Some(blocked_until) = quota.blocked_until.as_deref() else {
        return true;
    };
    DateTime::parse_from_rfc3339(blocked_until)
        .map(|dt| dt.with_timezone(&Utc) > now)
        .unwrap_or(true)
}

fn filter_provider_by_app_health(
    provider: Option<ShareUpstreamProvider>,
    health: &[ModelHealthSummary],
    app_type: &str,
    now: DateTime<Utc>,
) -> Option<ShareUpstreamProvider> {
    let provider = provider?;
    if app_health_blocks_runtime(health, app_type, now) {
        None
    } else {
        Some(provider)
    }
}

fn app_health_blocks_runtime(
    health: &[ModelHealthSummary],
    app_type: &str,
    now: DateTime<Utc>,
) -> bool {
    health.iter().any(|entry| {
        let is_quota_block = entry.status.eq_ignore_ascii_case("quota_blocked")
            || (is_app_level_model_health(entry, app_type)
                && entry.status.eq_ignore_ascii_case("failed")
                && model_health_error_is_quota_block(entry.error_message.as_deref()));
        is_quota_block && !quota_block_expired(entry.error_message.as_deref(), now)
    })
}

/// Parse the "until <RFC3339>" timestamp from a quota-block error message and
/// return true if the block has already expired.  If no timestamp is found the
/// block is treated as still active (conservative default).
fn quota_block_expired(error_message: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(msg) = error_message else {
        return false;
    };
    let lower = msg.to_ascii_lowercase();
    let Some(pos) = lower.rfind("until ") else {
        return false;
    };
    let raw = msg[pos + 6..].trim();
    // Trim trailing punctuation that might follow the timestamp.
    let raw = raw.trim_end_matches('.');
    match DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt.with_timezone(&Utc) <= now,
        Err(_) => false,
    }
}

fn is_app_level_model_health(entry: &ModelHealthSummary, app_type: &str) -> bool {
    model_health_key(&entry.requested_model) == app_type
        && model_health_key(&entry.actual_model) == app_type
}

fn model_health_error_is_quota_block(error: Option<&str>) -> bool {
    let Some(error) = error else {
        return false;
    };
    let lower = error.to_ascii_lowercase();
    lower.contains("quota exhausted")
        || lower.contains("quota_exhausted")
        || lower.contains("usage limit")
        || lower.contains("usage_limit")
        || lower.contains("weekly limit")
        || lower.contains("monthly limit")
}

#[cfg(test)]
mod quota_runtime_filter_tests {
    use super::*;

    fn provider_with_quota(blocked_until: Option<String>) -> ShareUpstreamProvider {
        ShareUpstreamProvider {
            kind: "official_oauth".to_string(),
            app: "codex".to_string(),
            provider_name: Some("Codex".to_string()),
            provider_type: None,
            for_sale_official_price_percent: None,
            account_email: None,
            api_url: None,
            quota: Some(ShareUpstreamQuota {
                status: "ok".to_string(),
                plan: None,
                queried_at: None,
                subscription_period_end: None,
                availability: Some("short_window_exhausted".to_string()),
                blocked_until,
                blocked_reason: Some("five hour quota exhausted".to_string()),
                blocked_scope: Some("five_hour".to_string()),
                dispatch_limit_percent: None,
                tiers: Vec::new(),
            }),
            models: Vec::new(),
            ..Default::default()
        }
    }

    fn provider_with_quota_tier(
        utilization: f64,
        resets_at: String,
        dispatch_limit_percent: Option<f64>,
    ) -> ShareUpstreamProvider {
        ShareUpstreamProvider {
            kind: "official_oauth".to_string(),
            app: "codex".to_string(),
            provider_name: Some("Codex".to_string()),
            provider_type: None,
            for_sale_official_price_percent: None,
            account_email: None,
            api_url: None,
            quota: Some(ShareUpstreamQuota {
                status: "ok".to_string(),
                plan: Some("ChatGPT Plus".to_string()),
                queried_at: None,
                subscription_period_end: None,
                availability: Some("available".to_string()),
                blocked_until: None,
                blocked_reason: None,
                blocked_scope: None,
                dispatch_limit_percent,
                tiers: vec![crate::models::ShareUpstreamQuotaTier {
                    label: "1w".to_string(),
                    utilization,
                    resets_at: Some(resets_at),
                    used: None,
                    limit: None,
                    unit: None,
                }],
            }),
            models: Vec::new(),
            ..Default::default()
        }
    }

    fn ollama_provider_with_display_only_quota() -> ShareUpstreamProvider {
        ShareUpstreamProvider {
            kind: "official_oauth".to_string(),
            app: "codex".to_string(),
            provider_name: Some("Ollama Cloud".to_string()),
            provider_type: Some("ollama_cloud".to_string()),
            for_sale_official_price_percent: None,
            account_email: Some("xiechengqi01@gmail.com".to_string()),
            api_url: None,
            quota: Some(ShareUpstreamQuota {
                status: "ok".to_string(),
                plan: Some("pro".to_string()),
                queried_at: None,
                subscription_period_end: Some((Utc::now() + Duration::days(28)).to_rfc3339()),
                availability: Some("available".to_string()),
                blocked_until: None,
                blocked_reason: None,
                blocked_scope: None,
                dispatch_limit_percent: Some(1.0),
                tiers: vec![crate::models::ShareUpstreamQuotaTier {
                    label: "xiechengqi01@gmail.com".to_string(),
                    utilization: 100.0,
                    resets_at: Some((Utc::now() + Duration::days(28)).to_rfc3339()),
                    used: None,
                    limit: None,
                    unit: None,
                }],
            }),
            models: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn quota_blocked_runtime_is_filtered_until_reset() {
        let now = Utc::now();
        let runtimes = ShareAppRuntimes {
            codex: Some(provider_with_quota(Some(
                (now + Duration::minutes(30)).to_rfc3339(),
            ))),
            ..Default::default()
        };

        let filtered = filter_app_runtimes_by_quota(runtimes, now);
        assert!(filtered.codex.is_none());
    }

    #[test]
    fn expired_quota_block_runtime_is_kept() {
        let now = Utc::now();
        let runtimes = ShareAppRuntimes {
            codex: Some(provider_with_quota(Some(
                (now - Duration::minutes(1)).to_rfc3339(),
            ))),
            ..Default::default()
        };

        let filtered = filter_app_runtimes_by_quota(runtimes, now);
        assert!(filtered.codex.is_some());
    }

    #[test]
    fn dispatch_limited_runtime_is_filtered_until_reset() {
        let now = Utc::now();
        let runtimes = ShareAppRuntimes {
            codex: Some(provider_with_quota_tier(
                90.0,
                (now + Duration::days(2)).to_rfc3339(),
                Some(90.0),
            )),
            ..Default::default()
        };

        let filtered = filter_app_runtimes_by_quota(runtimes, now);
        assert!(filtered.codex.is_none());
    }

    #[test]
    fn runtime_below_dispatch_limit_is_kept() {
        let now = Utc::now();
        let runtimes = ShareAppRuntimes {
            codex: Some(provider_with_quota_tier(
                89.0,
                (now + Duration::days(2)).to_rfc3339(),
                Some(90.0),
            )),
            ..Default::default()
        };

        let filtered = filter_app_runtimes_by_quota(runtimes, now);
        assert!(filtered.codex.is_some());
    }

    #[test]
    fn expired_dispatch_limit_runtime_is_kept() {
        let now = Utc::now();
        let runtimes = ShareAppRuntimes {
            codex: Some(provider_with_quota_tier(
                95.0,
                (now - Duration::minutes(1)).to_rfc3339(),
                Some(90.0),
            )),
            ..Default::default()
        };

        let filtered = filter_app_runtimes_by_quota(runtimes, now);
        assert!(filtered.codex.is_some());
    }

    #[test]
    fn zero_dispatch_limit_runtime_is_kept() {
        let now = Utc::now();
        let runtimes = ShareAppRuntimes {
            codex: Some(provider_with_quota_tier(
                95.0,
                (now + Duration::days(2)).to_rfc3339(),
                Some(0.0),
            )),
            ..Default::default()
        };

        let filtered = filter_app_runtimes_by_quota(runtimes, now);
        assert!(filtered.codex.is_some());
    }

    #[test]
    fn market_share_quota_health_uses_requested_app_runtime_quota() {
        let now = Utc::now();
        let runtimes = ShareAppRuntimes {
            codex: Some(provider_with_quota_tier(
                95.0,
                (now + Duration::days(2)).to_rfc3339(),
                None,
            )),
            ..Default::default()
        };
        let health = compute_market_share_quota_health(Some("codex"), &runtimes, None, now);

        assert!(
            health < 0.2,
            "high-utilization runtime quota should down-rank codex, got {health}"
        );
    }

    #[test]
    fn ollama_display_only_quota_is_not_used_for_filtering_or_health() {
        let now = Utc::now();
        let runtimes = ShareAppRuntimes {
            codex: Some(ollama_provider_with_display_only_quota()),
            ..Default::default()
        };

        let filtered = filter_app_runtimes_by_quota(runtimes.clone(), now);
        assert!(filtered.codex.is_some());

        let health = compute_market_share_quota_health(Some("codex"), &runtimes, None, now);
        assert_eq!(
            health,
            crate::scheduling_signals::compute_quota_health(None, now)
        );
    }

    #[test]
    fn market_share_quota_health_without_app_uses_worst_runtime_quota() {
        let now = Utc::now();
        let runtimes = ShareAppRuntimes {
            claude: Some(provider_with_quota_tier(
                5.0,
                (now + Duration::days(2)).to_rfc3339(),
                None,
            )),
            codex: Some(provider_with_quota_tier(
                95.0,
                (now + Duration::days(2)).to_rfc3339(),
                None,
            )),
            ..Default::default()
        };
        let health = compute_market_share_quota_health(None, &runtimes, None, now);

        assert!(
            health < 0.2,
            "generic market sync should conservatively use the worst runtime quota, got {health}"
        );
    }
}

fn filter_provider_models_by_health(
    provider: Option<ShareUpstreamProvider>,
    health: &[ModelHealthSummary],
) -> Option<ShareUpstreamProvider> {
    let provider = provider?;
    let current_models = provider_model_keys(&provider);
    let relevant_health = health
        .iter()
        .filter(|entry| {
            current_models.is_empty()
                || current_models.contains(&model_health_key(&entry.requested_model))
                || current_models.contains(&model_health_key(&entry.actual_model))
        })
        .collect::<Vec<_>>();
    if relevant_health.is_empty() {
        return Some(provider);
    }
    let all_relevant_models_failed = relevant_health.iter().all(|entry| {
        let recent_results = entry.recent_results.iter().take(3).collect::<Vec<_>>();
        recent_results.len() >= 3
            && recent_results
                .iter()
                .all(|result| result.as_str() == "failed")
    });
    if all_relevant_models_failed {
        None
    } else {
        Some(provider)
    }
}

fn model_health_key(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn provider_model_keys(provider: &ShareUpstreamProvider) -> HashSet<String> {
    provider
        .models
        .iter()
        .filter_map(|model| {
            let key = model_health_key(&model.actual_model);
            if key.is_empty() { None } else { Some(key) }
        })
        .collect()
}

fn market_request_error_text(log: &MarketRequestLogEntry) -> String {
    [
        Some(log.status.as_str()),
        log.error_message.as_deref(),
        log.model.as_deref(),
        Some(log.requested_model.as_str()),
        Some(log.actual_model.as_str()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase()
}

fn is_request_scoped_market_model_error(log: &MarketRequestLogEntry) -> bool {
    let text = market_request_error_text(log);
    if text.contains("model_max_prompt_tokens_exceeded")
        || text.contains("prompt token count")
        || text.contains("context length")
        || text.contains("context_length_exceeded")
        || text.contains("maximum context")
    {
        return true;
    }
    matches!(log.status_code, Some(400 | 404 | 422))
}

fn is_degraded_market_model_log(log: &MarketRequestLogEntry) -> bool {
    let status = log.status.to_ascii_lowercase();
    if matches!(status.as_str(), "rate_limited" | "degraded") {
        return true;
    }
    log.status_code == Some(429)
}

fn is_failed_market_model_log(log: &MarketRequestLogEntry) -> bool {
    if is_request_scoped_market_model_error(log) || is_degraded_market_model_log(log) {
        return false;
    }
    let status = log.status.to_ascii_lowercase();
    if matches!(
        status.as_str(),
        "failed" | "error" | "rate_limited" | "upstream_error"
    ) {
        return true;
    }
    log.status_code
        .map(|code| code == 429 || code >= 500)
        .unwrap_or(false)
}

fn is_success_market_model_log(log: &MarketRequestLogEntry) -> bool {
    let status = log.status.to_ascii_lowercase();
    if matches!(status.as_str(), "success" | "ok" | "completed") {
        return true;
    }
    log.status_code
        .map(|code| (200..400).contains(&code))
        .unwrap_or(false)
}

fn market_model_status(log: &MarketRequestLogEntry) -> Option<&'static str> {
    if is_request_scoped_market_model_error(log) {
        None
    } else if is_failed_market_model_log(log) {
        Some("failed")
    } else if is_degraded_market_model_log(log) {
        Some("degraded")
    } else if is_success_market_model_log(log) {
        Some("success")
    } else {
        None
    }
}

fn normalize_market_model_key(app_type: &str, model: &str) -> String {
    let model = model.trim();
    if app_type.eq_ignore_ascii_case("claude") {
        match model {
            "claude-sonnet-4-6" => "claude-sonnet-4.6".to_string(),
            "claude-opus-4-7" => "claude-opus-4.7".to_string(),
            _ => model.to_string(),
        }
    } else {
        model.to_string()
    }
}

fn record_market_share_model_failure_state_conn(
    conn: &Connection,
    market_email: &str,
    log: &MarketRequestLogEntry,
) -> Result<(), AppError> {
    let Some(status) = market_model_status(log) else {
        return Ok(());
    };
    let Some(share_id) = log
        .share_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return Ok(());
    };
    let app_type = log.request_agent.trim().to_ascii_lowercase();
    if !matches!(app_type.as_str(), "claude" | "codex" | "gemini") {
        return Ok(());
    }
    if conn
        .query_row(
            "SELECT 1 FROM market_request_logs WHERE request_id = ?1",
            params![log.request_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("check existing market request log failed: {e}")))?
        .is_some()
    {
        return Ok(());
    }

    let requested_model = if log.requested_model.trim().is_empty() {
        log.model
            .as_deref()
            .unwrap_or(app_type.as_str())
            .trim()
            .to_string()
    } else {
        log.requested_model.trim().to_string()
    };
    let requested_model = normalize_market_model_key(&app_type, &requested_model);
    let actual_model = if log.actual_model.trim().is_empty() {
        requested_model.clone()
    } else {
        normalize_market_model_key(&app_type, log.actual_model.trim())
    };
    let market_email = market_email.trim().to_ascii_lowercase();
    let checked_at = DateTime::parse_from_rfc3339(&log.created_at)
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|_| Utc::now().timestamp());
    let existing_recent = conn
        .query_row(
            "SELECT recent_results_json
             FROM market_share_model_failure_state
             WHERE market_email = ?1 AND share_id = ?2 AND app_type = ?3 AND requested_model = ?4",
            params![market_email, share_id, app_type, requested_model],
            |row| parse_recent_results(row.get(0)?),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read market failure state failed: {e}")))?
        .unwrap_or_default();
    let mut recent_results = Vec::with_capacity(3);
    recent_results.push(status.to_string());
    recent_results.extend(existing_recent.into_iter().take(2));
    let recent_json = serde_json::to_string(&recent_results)
        .map_err(|e| AppError::Internal(format!("serialize market failure state failed: {e}")))?;
    let error_message = matches!(status, "failed" | "degraded").then(|| {
        log.error_message
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| log.status.clone())
    });
    conn.execute(
        "INSERT INTO market_share_model_failure_state (
            market_email, share_id, app_type, requested_model, actual_model, last_status,
            last_success_at, last_failed_at, last_checked_at, recent_results_json,
            error_message, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6,
            CASE WHEN ?6 = 'success' THEN ?7 ELSE NULL END,
            CASE WHEN ?6 = 'failed' THEN ?7 ELSE NULL END,
            ?7, ?8, ?9, ?7)
         ON CONFLICT(market_email, share_id, app_type, requested_model) DO UPDATE SET
            actual_model = CASE WHEN excluded.last_checked_at >= market_share_model_failure_state.last_checked_at THEN excluded.actual_model ELSE market_share_model_failure_state.actual_model END,
            last_status = CASE WHEN excluded.last_checked_at >= market_share_model_failure_state.last_checked_at THEN excluded.last_status ELSE market_share_model_failure_state.last_status END,
            last_success_at = CASE
                WHEN excluded.last_status = 'success'
                 AND (market_share_model_failure_state.last_success_at IS NULL OR excluded.last_checked_at > market_share_model_failure_state.last_success_at)
                THEN excluded.last_checked_at
                ELSE market_share_model_failure_state.last_success_at
            END,
            last_failed_at = CASE
                WHEN excluded.last_status = 'failed'
                 AND (market_share_model_failure_state.last_failed_at IS NULL OR excluded.last_checked_at > market_share_model_failure_state.last_failed_at)
                THEN excluded.last_checked_at
                ELSE market_share_model_failure_state.last_failed_at
            END,
            last_checked_at = max(market_share_model_failure_state.last_checked_at, excluded.last_checked_at),
            recent_results_json = CASE WHEN excluded.last_checked_at >= market_share_model_failure_state.last_checked_at THEN excluded.recent_results_json ELSE market_share_model_failure_state.recent_results_json END,
            error_message = CASE WHEN excluded.last_checked_at >= market_share_model_failure_state.last_checked_at THEN excluded.error_message ELSE market_share_model_failure_state.error_message END,
            updated_at = max(market_share_model_failure_state.updated_at, excluded.updated_at)",
        params![
            market_email,
            share_id,
            app_type,
            requested_model,
            actual_model,
            status,
            checked_at,
            recent_json,
            error_message,
        ],
    )
    .map_err(|e| AppError::Internal(format!("upsert market failure state failed: {e}")))?;
    Ok(())
}

fn list_recent_share_request_logs(
    conn: &Connection,
    per_share_limit: usize,
) -> Result<Vec<ShareRequestLogEntry>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT request_id, share_id, share_name, provider_id, provider_name, app_type, model,
                    request_model, request_agent, requested_model, actual_model, actual_model_source,
                    status_code, latency_ms, first_token_ms, input_tokens,
                    output_tokens, cache_read_tokens, cache_creation_tokens, is_streaming,
                    session_id, user_country, user_country_iso3, user_email, is_health_check, created_at
             FROM (
                 SELECT request_id, share_id, share_name, provider_id, provider_name, app_type, model,
                        request_model, request_agent, requested_model, actual_model, actual_model_source,
                        status_code, latency_ms, first_token_ms, input_tokens,
                        output_tokens, cache_read_tokens, cache_creation_tokens, is_streaming,
                        session_id, user_country, user_country_iso3, user_email, is_health_check, created_at,
                        ROW_NUMBER() OVER (PARTITION BY share_id ORDER BY created_at DESC) AS row_num
                 FROM share_request_logs
             )
             WHERE row_num <= ?1
             ORDER BY created_at DESC",
        )
        .map_err(|e| AppError::Internal(format!("prepare recent share request logs failed: {e}")))?;
    let rows = stmt
        .query_map(params![per_share_limit as i64], |row| {
            Ok(ShareRequestLogEntry {
                request_id: row.get(0)?,
                share_id: row.get(1)?,
                share_name: row.get(2)?,
                provider_id: row.get(3)?,
                provider_name: row.get(4)?,
                app_type: row.get(5)?,
                model: row.get(6)?,
                request_model: row.get(7)?,
                request_agent: row.get(8)?,
                requested_model: row.get(9)?,
                actual_model: row.get(10)?,
                actual_model_source: row.get(11)?,
                status_code: row.get::<_, i64>(12)? as u16,
                latency_ms: row.get::<_, i64>(13)? as u64,
                first_token_ms: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
                input_tokens: row.get::<_, i64>(15)? as u32,
                output_tokens: row.get::<_, i64>(16)? as u32,
                cache_read_tokens: row.get::<_, i64>(17)? as u32,
                cache_creation_tokens: row.get::<_, i64>(18)? as u32,
                is_streaming: row.get::<_, i64>(19)? != 0,
                session_id: row.get(20)?,
                user_country: row.get(21)?,
                user_country_iso3: row.get(22)?,
                user_email: row.get(23)?,
                is_health_check: row.get::<_, i64>(24)? != 0,
                created_at: row.get(25)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("query recent share request logs failed: {e}")))?;
    let logs = collect_rows(rows)?;
    Ok(deduplicate_recent_share_request_logs(logs))
}

fn sql_prefix(alias: &str) -> String {
    if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    }
}

fn market_log_input_tokens_expr(alias: &str) -> String {
    let prefix = sql_prefix(alias);
    format!(
        "CASE
            WHEN lower(COALESCE({prefix}request_agent, '')) = 'codex' THEN
                CASE
                    WHEN COALESCE({prefix}input_tokens, 0) > COALESCE({prefix}cache_read_tokens, 0)
                    THEN COALESCE({prefix}input_tokens, 0) - COALESCE({prefix}cache_read_tokens, 0)
                    ELSE 0
                END
            ELSE COALESCE({prefix}input_tokens, 0)
        END"
    )
}

fn market_log_total_tokens_expr(alias: &str) -> String {
    let prefix = sql_prefix(alias);
    let input_expr = market_log_input_tokens_expr(alias);
    format!(
        "({input_expr}
          + COALESCE({prefix}output_tokens, 0)
          + COALESCE({prefix}cache_read_tokens, 0)
          + COALESCE({prefix}cache_creation_tokens, 0))"
    )
}

fn share_log_total_tokens_expr(alias: &str) -> String {
    let prefix = sql_prefix(alias);
    format!(
        "(COALESCE({prefix}input_tokens, 0)
          + COALESCE({prefix}output_tokens, 0)
          + COALESCE({prefix}cache_read_tokens, 0)
          + COALESCE({prefix}cache_creation_tokens, 0))"
    )
}

fn list_share_usage_by_app(
    conn: &Connection,
) -> Result<HashMap<String, (BTreeMap<String, i64>, BTreeMap<String, i64>)>, AppError> {
    let market_total_expr = market_log_total_tokens_expr("ml");
    let share_total_expr = share_log_total_tokens_expr("sl");
    let mut stmt = conn
        .prepare(&format!(
            "WITH usage_rows AS (
                SELECT sl.share_id AS share_id,
                       lower(CASE
                           WHEN COALESCE(sl.request_agent, '') != '' THEN sl.request_agent
                           ELSE sl.app_type
                       END) AS app,
                       sl.request_id AS request_id,
                       'share' AS source,
                       {share_total_expr} AS total_tokens
                  FROM share_request_logs sl
                 WHERE sl.is_health_check = 0
                UNION ALL
                SELECT ml.share_id AS share_id,
                       lower(ml.request_agent) AS app,
                       ml.request_id AS request_id,
                       'market' AS source,
                       {market_total_expr} AS total_tokens
                  FROM market_request_logs ml
                 WHERE ml.share_id IS NOT NULL
                   AND trim(ml.share_id) != ''
             ),
             per_request AS (
                SELECT share_id,
                       app,
                       request_id,
                       MAX(CASE WHEN source = 'market' THEN total_tokens END) AS market_total,
                       MAX(CASE WHEN source = 'share' THEN total_tokens END) AS share_total
                  FROM usage_rows
                 WHERE app IN ('claude', 'codex', 'gemini')
                 GROUP BY share_id, app, request_id
             )
             SELECT share_id,
                    app,
                    COUNT(*) AS requests_count,
                    COALESCE(SUM(
                        CASE
                            WHEN COALESCE(market_total, 0) > 0 THEN market_total
                            ELSE COALESCE(share_total, market_total, 0)
                        END
                    ), 0) AS total_tokens
              FROM per_request
              GROUP BY share_id, app",
        ))
        .map_err(|e| AppError::Internal(format!("prepare share usage by app failed: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query share usage by app failed: {e}")))?;
    let mut result = HashMap::<String, (BTreeMap<String, i64>, BTreeMap<String, i64>)>::new();
    for row in rows {
        let (share_id, app, requests_count, total_tokens) = row
            .map_err(|e| AppError::Internal(format!("read share usage by app row failed: {e}")))?;
        if !matches!(app.as_str(), "claude" | "codex" | "gemini") {
            continue;
        }
        let entry = result.entry(share_id).or_default();
        entry.0.insert(app.clone(), total_tokens);
        entry.1.insert(app, requests_count);
    }
    Ok(result)
}

fn map_image_generation_request_log_row(
    row: &Row<'_>,
) -> Result<ImageGenerationRequestLogEntry, rusqlite::Error> {
    Ok(ImageGenerationRequestLogEntry {
        request_id: row.get(0)?,
        share_id: row.get(1)?,
        share_name: row.get(2)?,
        installation_id: row.get(3)?,
        provider_id: row.get(4)?,
        provider_name: row.get(5)?,
        app_type: row.get(6)?,
        model: row.get(7)?,
        status: row.get(8)?,
        status_code: row.get::<_, Option<i64>>(9)?.map(|value| value as u16),
        latency_ms: row.get::<_, i64>(10)?.max(0) as u64,
        created_at: row.get(11)?,
        completed_at: row.get(12)?,
        prompt_preview: row.get(13)?,
        error_message: row.get(14)?,
        result_mime_type: row.get(15)?,
        result_size_bytes: row
            .get::<_, Option<i64>>(16)?
            .map(|value| value.max(0) as u64),
        result_url: None,
        result_storage_key: row.get(17)?,
        result_access_token: row.get(18)?,
        created_by_email: row.get(19)?,
        user_country: row.get(20)?,
    })
}

fn list_recent_share_model_health_checks(
    conn: &Connection,
    per_share_limit: usize,
) -> Result<Vec<ShareModelHealthCheckEntry>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT request_id, share_id, subdomain, app_type, requested_model, actual_model,
                    status, status_code, latency_ms, first_token_ms, error_message, checked_at, source
             FROM (
                 SELECT request_id, share_id, subdomain, app_type, requested_model, actual_model,
                        status, status_code, latency_ms, first_token_ms, error_message, checked_at, source,
                        ROW_NUMBER() OVER (PARTITION BY share_id ORDER BY checked_at DESC, request_id DESC) AS row_num
                 FROM share_model_health_checks
             )
             WHERE row_num <= ?1
             ORDER BY checked_at DESC, request_id DESC",
        )
        .map_err(|e| AppError::Internal(format!("prepare recent model health checks failed: {e}")))?;
    let rows = stmt
        .query_map(params![per_share_limit as i64], |row| {
            Ok(ShareModelHealthCheckEntry {
                request_id: row.get(0)?,
                share_id: row.get(1)?,
                subdomain: row.get(2)?,
                app_type: row.get(3)?,
                requested_model: row.get(4)?,
                actual_model: row.get(5)?,
                status: row.get(6)?,
                status_code: row.get::<_, Option<i64>>(7)?.map(|v| v as u16),
                latency_ms: row.get::<_, i64>(8)? as u64,
                first_token_ms: row.get::<_, Option<i64>>(9)?.map(|v| v as u64),
                error_message: row.get(10)?,
                checked_at: row.get(11)?,
                source: row.get(12)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("query recent model health checks failed: {e}")))?;
    let mut checks = Vec::new();
    for row in rows {
        checks.push(
            row.map_err(|e| AppError::Internal(format!("read model health check failed: {e}")))?,
        );
    }
    Ok(checks)
}

fn list_recent_market_request_logs(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<DashboardMarketRequestLogView>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT request_id, market_id, market_email, market_subdomain, user_email,
                    api_key_prefix, router_id, share_id, share_subdomain, model,
                    request_agent, requested_model, actual_model, actual_model_source, status,
                    status_code, error_message, latency_ms, input_tokens, output_tokens, cache_read_tokens,
                    cache_creation_tokens, usage_amount_usd, created_at, settled_at,
                    user_country, user_country_iso3
             FROM market_request_logs
             ORDER BY created_at DESC
             LIMIT ?1",
        )
        .map_err(|e| {
            AppError::Internal(format!("prepare recent market request logs failed: {e}"))
        })?;
    let rows = stmt
        .query_map(params![limit as i64], |row| {
            Ok(DashboardMarketRequestLogView {
                request_id: row.get(0)?,
                market_id: row.get(1)?,
                market_email: row.get(2)?,
                market_subdomain: row.get(3)?,
                user_email: row.get(4)?,
                api_key_prefix: row.get(5)?,
                router_id: row.get(6)?,
                share_id: row.get(7)?,
                share_subdomain: row.get(8)?,
                model: row.get(9)?,
                request_agent: row.get(10)?,
                requested_model: row.get(11)?,
                actual_model: row.get(12)?,
                actual_model_source: row.get(13)?,
                status: row.get(14)?,
                status_code: row.get::<_, Option<i64>>(15)?.map(|value| value as u16),
                error_message: row.get(16)?,
                latency_ms: row.get::<_, Option<i64>>(17)?.map(|value| value as u64),
                input_tokens: row.get::<_, i64>(18)? as u32,
                output_tokens: row.get::<_, i64>(19)? as u32,
                cache_read_tokens: row.get::<_, i64>(20)? as u32,
                cache_creation_tokens: row.get::<_, i64>(21)? as u32,
                usage_amount_usd: row.get(22)?,
                created_at: row.get(23)?,
                settled_at: row.get(24)?,
                user_country: row.get(25)?,
                user_country_iso3: row.get(26)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("query recent market request logs failed: {e}")))?;
    collect_rows(rows)
}

fn merge_market_request_logs_into_share_logs(
    logs_by_share: &mut HashMap<String, Vec<ShareRequestLogEntry>>,
    market_logs: &[DashboardMarketRequestLogView],
    shares: &[(String, ShareDescriptor)],
    per_share_limit: usize,
) {
    let share_names = shares
        .iter()
        .map(|(_, share)| (share.share_id.clone(), share.share_name.clone()))
        .collect::<HashMap<_, _>>();

    for market_log in market_logs {
        let Some(share_id) = market_log
            .share_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(created_at) = parse_rfc3339_timestamp(&market_log.created_at) else {
            continue;
        };
        let mut entry =
            market_log_to_share_request_log(market_log, share_id, created_at, &share_names);
        let logs = logs_by_share.entry(share_id.to_string()).or_default();
        if let Some(existing) = logs.iter_mut().find(|candidate| {
            candidate.request_id == entry.request_id
                || share_request_logs_are_semantic_duplicates(candidate, &entry)
        }) {
            if prefer_market_derived_share_log(&entry, existing) {
                merge_share_log_model_route(&mut entry, existing);
                *existing = entry;
            } else {
                merge_share_log_model_route(existing, &entry);
            }
        } else {
            logs.push(entry);
        }
    }

    for logs in logs_by_share.values_mut() {
        logs.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.request_id.cmp(&a.request_id))
        });
        logs.truncate(per_share_limit);
    }
}

fn market_log_to_share_request_log(
    log: &DashboardMarketRequestLogView,
    share_id: &str,
    created_at: i64,
    share_names: &HashMap<String, String>,
) -> ShareRequestLogEntry {
    let app_type = log.request_agent.trim().to_ascii_lowercase();
    let model = log
        .model
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| log.actual_model.clone());
    ShareRequestLogEntry {
        request_id: log.request_id.clone(),
        share_id: share_id.to_string(),
        share_name: share_names
            .get(share_id)
            .cloned()
            .or_else(|| log.share_subdomain.clone())
            .unwrap_or_else(|| share_id.to_string()),
        provider_id: log.market_id.clone(),
        provider_name: if log.market_subdomain.trim().is_empty() {
            log.market_email.clone()
        } else {
            log.market_subdomain.clone()
        },
        app_type: if app_type.is_empty() {
            "codex".to_string()
        } else {
            app_type.clone()
        },
        model: model.clone(),
        request_model: log.requested_model.clone(),
        request_agent: if app_type.is_empty() {
            "codex".to_string()
        } else {
            app_type
        },
        requested_model: log.requested_model.clone(),
        actual_model: log.actual_model.clone(),
        actual_model_source: log.actual_model_source.clone(),
        status_code: log
            .status_code
            .unwrap_or_else(|| status_code_from_market_status(&log.status)),
        latency_ms: log.latency_ms.unwrap_or(0),
        first_token_ms: None,
        input_tokens: log.input_tokens,
        output_tokens: log.output_tokens,
        cache_read_tokens: log.cache_read_tokens,
        cache_creation_tokens: log.cache_creation_tokens,
        is_streaming: log.status == "streaming",
        session_id: None,
        user_country: log.user_country.clone(),
        user_country_iso3: log.user_country_iso3.clone(),
        user_email: log.user_email.clone(),
        created_at,
        is_health_check: false,
    }
}

fn status_code_from_market_status(status: &str) -> u16 {
    match status {
        "settled" | "streaming" => 200,
        "needs_review" => 202,
        _ => 500,
    }
}

fn share_request_logs_are_semantic_duplicates(
    left: &ShareRequestLogEntry,
    right: &ShareRequestLogEntry,
) -> bool {
    if left.share_id != right.share_id
        || left.status_code != right.status_code
        || left.input_tokens != right.input_tokens
        || left.output_tokens != right.output_tokens
        || left.cache_read_tokens != right.cache_read_tokens
        || left.cache_creation_tokens != right.cache_creation_tokens
    {
        return false;
    }

    let left_app = if left.request_agent.trim().is_empty() {
        left.app_type.trim()
    } else {
        left.request_agent.trim()
    };
    let right_app = if right.request_agent.trim().is_empty() {
        right.app_type.trim()
    } else {
        right.request_agent.trim()
    };
    if !left_app.eq_ignore_ascii_case(right_app) {
        return false;
    }

    if !left
        .requested_model
        .trim()
        .eq_ignore_ascii_case(right.requested_model.trim())
        || !left
            .actual_model
            .trim()
            .eq_ignore_ascii_case(right.actual_model.trim())
    {
        return false;
    }

    let latency_window_secs = left
        .latency_ms
        .max(right.latency_ms)
        .div_ceil(1000)
        .saturating_add(10)
        .clamp(10, 300);
    left.created_at.abs_diff(right.created_at) <= latency_window_secs
}

fn prefer_market_derived_share_log(
    candidate: &ShareRequestLogEntry,
    existing: &ShareRequestLogEntry,
) -> bool {
    if share_log_has_model_mapping(candidate) != share_log_has_model_mapping(existing) {
        return share_log_has_model_mapping(candidate);
    }
    let candidate_tokens = share_request_log_total_tokens(candidate);
    let existing_tokens = share_request_log_total_tokens(existing);
    if candidate_tokens != existing_tokens {
        return candidate_tokens > existing_tokens;
    }
    if candidate.user_email.is_some() != existing.user_email.is_some() {
        return candidate.user_email.is_some();
    }
    candidate.created_at >= existing.created_at
}

fn share_log_has_model_mapping(log: &ShareRequestLogEntry) -> bool {
    let requested = log.requested_model.trim();
    let actual = log.actual_model.trim();
    !requested.is_empty() && !actual.is_empty() && requested != actual
}

fn merge_share_log_model_route(target: &mut ShareRequestLogEntry, source: &ShareRequestLogEntry) {
    if share_log_has_model_mapping(target) || !share_log_has_model_mapping(source) {
        return;
    }
    target.request_model = source.requested_model.clone();
    target.requested_model = source.requested_model.clone();
    target.actual_model = source.actual_model.clone();
    target.actual_model_source = source.actual_model_source.clone();
    target.model = source.actual_model.clone();
}

fn share_request_log_total_tokens(log: &ShareRequestLogEntry) -> u32 {
    log.input_tokens
        .saturating_add(log.output_tokens)
        .saturating_add(log.cache_read_tokens)
        .saturating_add(log.cache_creation_tokens)
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RecentShareLogFingerprint {
    share_id: String,
    created_at: i64,
    model: String,
    request_model: String,
    status_code: u16,
    latency_ms: u64,
    first_token_ms: Option<u64>,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    is_streaming: bool,
    session_id: Option<String>,
}

fn deduplicate_recent_share_request_logs(
    logs: Vec<ShareRequestLogEntry>,
) -> Vec<ShareRequestLogEntry> {
    let mut deduped = Vec::with_capacity(logs.len());
    let mut seen = HashMap::<RecentShareLogFingerprint, usize>::new();

    for log in logs {
        let fingerprint = RecentShareLogFingerprint {
            share_id: log.share_id.clone(),
            created_at: log.created_at,
            model: log.model.clone(),
            request_model: log.request_model.clone(),
            status_code: log.status_code,
            latency_ms: log.latency_ms,
            first_token_ms: log.first_token_ms,
            input_tokens: log.input_tokens,
            output_tokens: log.output_tokens,
            cache_read_tokens: log.cache_read_tokens,
            cache_creation_tokens: log.cache_creation_tokens,
            is_streaming: log.is_streaming,
            session_id: log.session_id.clone(),
        };

        match seen.entry(fingerprint) {
            Entry::Vacant(entry) => {
                entry.insert(deduped.len());
                deduped.push(log);
            }
            Entry::Occupied(entry) => {
                let existing = &mut deduped[*entry.get()];
                if prefer_share_request_log(&log, existing) {
                    *existing = log;
                }
            }
        }
    }

    deduped
}

fn prefer_share_request_log(
    candidate: &ShareRequestLogEntry,
    existing: &ShareRequestLogEntry,
) -> bool {
    let candidate_name = candidate.provider_name.trim();
    let existing_name = existing.provider_name.trim();
    let candidate_has_display_name =
        !candidate_name.is_empty() && candidate_name != candidate.provider_id;
    let existing_has_display_name =
        !existing_name.is_empty() && existing_name != existing.provider_id;
    if candidate_has_display_name != existing_has_display_name {
        return candidate_has_display_name;
    }

    let candidate_model_score = usize::from(!candidate.model.trim().is_empty())
        + usize::from(!candidate.request_model.trim().is_empty());
    let existing_model_score = usize::from(!existing.model.trim().is_empty())
        + usize::from(!existing.request_model.trim().is_empty());
    if candidate_model_score != existing_model_score {
        return candidate_model_score > existing_model_score;
    }

    candidate.request_id > existing.request_id
}

fn list_health_checks(
    conn: &Connection,
    minutes: usize,
) -> Result<HashMap<String, Vec<HealthCheckEntry>>, AppError> {
    let current_bucket = Utc::now().timestamp().div_euclid(60);
    let cutoff = (current_bucket - (minutes as i64 - 1)) * 60;
    let mut stmt = conn
        .prepare(
            "SELECT share_id, checked_at, is_healthy
             FROM share_health_checks
             WHERE checked_at >= ?1
             ORDER BY checked_at ASC",
        )
        .map_err(|e| AppError::Internal(format!("prepare health checks failed: {e}")))?;
    let rows = stmt
        .query_map(params![cutoff], |row| {
            Ok((
                row.get::<_, String>(0)?,
                HealthCheckEntry {
                    checked_at: row.get(1)?,
                    is_healthy: row.get::<_, i64>(2)? != 0,
                },
            ))
        })
        .map_err(|e| AppError::Internal(format!("query health checks failed: {e}")))?;
    let mut map: HashMap<String, Vec<HealthCheckEntry>> = HashMap::new();
    for row in rows {
        let (share_id, entry) =
            row.map_err(|e| AppError::Internal(format!("read health check row failed: {e}")))?;
        map.entry(share_id).or_default().push(entry);
    }
    Ok(map)
}

fn list_online_minutes_24h(conn: &Connection) -> Result<HashMap<String, usize>, AppError> {
    let cutoff = Utc::now().timestamp() - 24 * 60 * 60;
    let mut stmt = conn
        .prepare(
            "SELECT share_id, COUNT(DISTINCT checked_at / 60) AS online_minutes
             FROM share_health_checks
             WHERE checked_at >= ?1 AND is_healthy = 1
             GROUP BY share_id",
        )
        .map_err(|e| AppError::Internal(format!("prepare online minutes failed: {e}")))?;
    let rows = stmt
        .query_map(params![cutoff], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })
        .map_err(|e| AppError::Internal(format!("query online minutes failed: {e}")))?;
    let mut map = HashMap::new();
    for row in rows {
        let (share_id, online_minutes) =
            row.map_err(|e| AppError::Internal(format!("read online minute row failed: {e}")))?;
        map.insert(share_id, online_minutes.min(ONLINE_WINDOW_MINUTES));
    }
    Ok(map)
}

fn list_installation_health_checks(
    conn: &Connection,
    minutes: usize,
) -> Result<HashMap<String, Vec<HealthCheckEntry>>, AppError> {
    let current_bucket = Utc::now().timestamp().div_euclid(60);
    let cutoff = (current_bucket - (minutes as i64 - 1)) * 60;
    let mut stmt = conn
        .prepare(
            "SELECT installation_id, checked_at, is_healthy
             FROM installation_health_checks
             WHERE checked_at >= ?1
             ORDER BY checked_at ASC",
        )
        .map_err(|e| {
            AppError::Internal(format!("prepare installation health checks failed: {e}"))
        })?;
    let rows = stmt
        .query_map(params![cutoff], |row| {
            Ok((
                row.get::<_, String>(0)?,
                HealthCheckEntry {
                    checked_at: row.get(1)?,
                    is_healthy: row.get::<_, i64>(2)? != 0,
                },
            ))
        })
        .map_err(|e| AppError::Internal(format!("query installation health checks failed: {e}")))?;
    let mut map: HashMap<String, Vec<HealthCheckEntry>> = HashMap::new();
    for row in rows {
        let (installation_id, entry) = row.map_err(|e| {
            AppError::Internal(format!("read installation health check row failed: {e}"))
        })?;
        map.entry(installation_id).or_default().push(entry);
    }
    Ok(map)
}

fn list_installation_online_minutes_24h(
    conn: &Connection,
) -> Result<HashMap<String, usize>, AppError> {
    let cutoff = Utc::now().timestamp() - 24 * 60 * 60;
    let mut stmt = conn
        .prepare(
            "SELECT installation_id, COUNT(DISTINCT checked_at / 60) AS online_minutes
             FROM installation_health_checks
             WHERE checked_at >= ?1 AND is_healthy = 1
             GROUP BY installation_id",
        )
        .map_err(|e| {
            AppError::Internal(format!("prepare installation online minutes failed: {e}"))
        })?;
    let rows = stmt
        .query_map(params![cutoff], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })
        .map_err(|e| {
            AppError::Internal(format!("query installation online minutes failed: {e}"))
        })?;
    let mut map = HashMap::new();
    for row in rows {
        let (installation_id, online_minutes) = row.map_err(|e| {
            AppError::Internal(format!("read installation online minute row failed: {e}"))
        })?;
        map.insert(installation_id, online_minutes.min(ONLINE_WINDOW_MINUTES));
    }
    Ok(map)
}

#[derive(Debug, Clone, Default)]
struct HealthTimelineBucketStats {
    healthy_minutes: HashSet<i64>,
    observed_minutes: HashSet<i64>,
    request_count: usize,
    failure_count: usize,
}

fn empty_health_timeline_stats() -> Vec<HealthTimelineBucketStats> {
    (0..HEALTH_TIMELINE_BUCKETS)
        .map(|_| HealthTimelineBucketStats::default())
        .collect()
}

fn health_timeline_window(now: DateTime<Utc>) -> (i64, i64) {
    let end = now.timestamp();
    let start = end - HEALTH_TIMELINE_BUCKETS as i64 * HEALTH_TIMELINE_BUCKET_SECS;
    (start, end)
}

fn health_timeline_bucket_index(timestamp: i64, start: i64, end: i64) -> Option<usize> {
    if timestamp < start || timestamp > end {
        return None;
    }
    Some(
        ((timestamp - start) / HEALTH_TIMELINE_BUCKET_SECS)
            .clamp(0, HEALTH_TIMELINE_BUCKETS as i64 - 1) as usize,
    )
}

fn timeline_timestamp_rfc3339(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| timestamp.to_string())
}

fn parse_rfc3339_timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp())
}

fn health_timeline_status(score: f64, stats: &HealthTimelineBucketStats) -> &'static str {
    health_timeline_status_for_values(
        score,
        !stats.observed_minutes.is_empty() || stats.request_count > 0,
    )
}

fn health_timeline_status_for_values(score: f64, has_data: bool) -> &'static str {
    if !has_data {
        return "unknown";
    }
    if score >= 90.0 {
        "healthy"
    } else if score >= 70.0 {
        "degraded"
    } else if score >= 30.0 {
        "unhealthy"
    } else {
        "offline"
    }
}

fn health_timeline_score(stats: &HealthTimelineBucketStats) -> f64 {
    let online_ratio = (stats.healthy_minutes.len() as f64 / 30.0).clamp(0.0, 1.0);
    if stats.request_count == 0 {
        return online_ratio * 100.0;
    }
    let success_count = stats.request_count.saturating_sub(stats.failure_count);
    let request_success_ratio = (success_count as f64 / stats.request_count as f64).clamp(0.0, 1.0);
    if stats.observed_minutes.is_empty() {
        request_success_ratio * 100.0
    } else {
        online_ratio * 70.0 + request_success_ratio * 30.0
    }
}

fn health_timeline_bucket_from_stats(
    stats: &HealthTimelineBucketStats,
    start: i64,
    index: usize,
) -> HealthTimelineBucket {
    let bucket_start = start + index as i64 * HEALTH_TIMELINE_BUCKET_SECS;
    let bucket_end = bucket_start + HEALTH_TIMELINE_BUCKET_SECS;
    let score = health_timeline_score(stats);
    HealthTimelineBucket {
        start_at: timeline_timestamp_rfc3339(bucket_start),
        end_at: timeline_timestamp_rfc3339(bucket_end),
        status: health_timeline_status(score, stats).into(),
        score,
        online_minutes: stats.healthy_minutes.len().min(30),
        observed_minutes: stats.observed_minutes.len().min(30),
        request_count: stats.request_count,
        failure_count: stats.failure_count,
    }
}

fn materialize_health_timeline(
    stats: &[HealthTimelineBucketStats],
    start: i64,
) -> Vec<HealthTimelineBucket> {
    stats
        .iter()
        .enumerate()
        .map(|(index, bucket)| health_timeline_bucket_from_stats(bucket, start, index))
        .collect()
}

fn merge_market_health_timeline(
    linked_shares: &[MarketLinkedShareView],
    health_timeline_by_share: &HashMap<String, Vec<HealthTimelineBucket>>,
    market_request_stats: Option<&Vec<HealthTimelineBucketStats>>,
    start: i64,
) -> Vec<HealthTimelineBucket> {
    (0..HEALTH_TIMELINE_BUCKETS)
        .map(|index| {
            let mut best_score: Option<f64> = None;
            let mut online_minutes = 0;
            let mut observed_minutes = 0;
            for share in linked_shares
                .iter()
                .filter(|share| !share.disabled_by_market)
            {
                let Some(bucket) = health_timeline_by_share
                    .get(&share.share_id)
                    .and_then(|timeline| timeline.get(index))
                else {
                    continue;
                };
                if bucket.status != "unknown" {
                    best_score =
                        Some(best_score.map_or(bucket.score, |score| score.max(bucket.score)));
                    online_minutes = online_minutes.max(bucket.online_minutes);
                    observed_minutes = observed_minutes.max(bucket.observed_minutes);
                }
            }

            let request_stats = market_request_stats
                .and_then(|stats| stats.get(index))
                .cloned()
                .unwrap_or_default();
            let mut score = best_score.unwrap_or_else(|| health_timeline_score(&request_stats));
            if request_stats.request_count > 0 {
                let success_count = request_stats
                    .request_count
                    .saturating_sub(request_stats.failure_count);
                let request_score = (success_count as f64 / request_stats.request_count as f64)
                    .clamp(0.0, 1.0)
                    * 100.0;
                score = if best_score.is_some() {
                    best_score.unwrap_or(0.0) * 0.7 + request_score * 0.3
                } else {
                    request_score
                };
            }
            let has_data = best_score.is_some() || request_stats.request_count > 0;
            let bucket_start = start + index as i64 * HEALTH_TIMELINE_BUCKET_SECS;
            HealthTimelineBucket {
                start_at: timeline_timestamp_rfc3339(bucket_start),
                end_at: timeline_timestamp_rfc3339(bucket_start + HEALTH_TIMELINE_BUCKET_SECS),
                status: health_timeline_status_for_values(score, has_data).into(),
                score,
                online_minutes,
                observed_minutes,
                request_count: request_stats.request_count,
                failure_count: request_stats.failure_count,
            }
        })
        .collect()
}

fn request_status_failed(status: &str, status_code: Option<u16>) -> bool {
    let status = status.trim().to_ascii_lowercase();
    if matches!(
        status.as_str(),
        "failed" | "error" | "failed_released" | "rate_limited" | "upstream_error"
    ) {
        return true;
    }
    status_code.map(|code| code >= 400).unwrap_or(false)
}

fn list_share_health_timeline_24h(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<HashMap<String, Vec<HealthTimelineBucket>>, AppError> {
    let (start, end) = health_timeline_window(now);
    let mut by_share: HashMap<String, Vec<HealthTimelineBucketStats>> = HashMap::new();

    let mut stmt = conn
        .prepare(
            "SELECT share_id, checked_at, is_healthy
             FROM share_health_checks
             WHERE checked_at >= ?1 AND checked_at <= ?2",
        )
        .map_err(|e| AppError::Internal(format!("prepare health timeline failed: {e}")))?;
    let rows = stmt
        .query_map(params![start, end], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)? != 0,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query health timeline failed: {e}")))?;
    for row in rows {
        let (share_id, checked_at, is_healthy) =
            row.map_err(|e| AppError::Internal(format!("read health timeline row failed: {e}")))?;
        let Some(index) = health_timeline_bucket_index(checked_at, start, end) else {
            continue;
        };
        let minute = checked_at.div_euclid(60);
        let bucket = &mut by_share
            .entry(share_id)
            .or_insert_with(empty_health_timeline_stats)[index];
        bucket.observed_minutes.insert(minute);
        if is_healthy {
            bucket.healthy_minutes.insert(minute);
        }
    }

    let mut stmt = conn
        .prepare(
            "SELECT share_id, status_code, created_at
             FROM share_request_logs
             WHERE created_at >= ?1 AND created_at <= ?2 AND is_health_check = 0",
        )
        .map_err(|e| AppError::Internal(format!("prepare share request timeline failed: {e}")))?;
    let rows = stmt
        .query_map(params![start, end], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as u16,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query share request timeline failed: {e}")))?;
    for row in rows {
        let (share_id, status_code, created_at) = row.map_err(|e| {
            AppError::Internal(format!("read share request timeline row failed: {e}"))
        })?;
        let Some(index) = health_timeline_bucket_index(created_at, start, end) else {
            continue;
        };
        let bucket = &mut by_share
            .entry(share_id)
            .or_insert_with(empty_health_timeline_stats)[index];
        bucket.request_count += 1;
        if status_code >= 400 {
            bucket.failure_count += 1;
        }
    }

    Ok(by_share
        .into_iter()
        .map(|(share_id, stats)| (share_id, materialize_health_timeline(&stats, start)))
        .collect())
}

fn list_installation_health_timeline_24h(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<HashMap<String, Vec<HealthTimelineBucket>>, AppError> {
    let (start, end) = health_timeline_window(now);
    let mut by_installation: HashMap<String, Vec<HealthTimelineBucketStats>> = HashMap::new();

    let mut stmt = conn
        .prepare(
            "SELECT installation_id, checked_at, is_healthy
             FROM installation_health_checks
             WHERE checked_at >= ?1 AND checked_at <= ?2",
        )
        .map_err(|e| {
            AppError::Internal(format!("prepare installation health timeline failed: {e}"))
        })?;
    let rows = stmt
        .query_map(params![start, end], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)? != 0,
            ))
        })
        .map_err(|e| {
            AppError::Internal(format!("query installation health timeline failed: {e}"))
        })?;
    for row in rows {
        let (installation_id, checked_at, is_healthy) = row.map_err(|e| {
            AppError::Internal(format!("read installation health timeline row failed: {e}"))
        })?;
        let Some(index) = health_timeline_bucket_index(checked_at, start, end) else {
            continue;
        };
        let minute = checked_at.div_euclid(60);
        let bucket = &mut by_installation
            .entry(installation_id)
            .or_insert_with(empty_health_timeline_stats)[index];
        bucket.observed_minutes.insert(minute);
        if is_healthy {
            bucket.healthy_minutes.insert(minute);
        }
    }

    Ok(by_installation
        .into_iter()
        .map(|(installation_id, stats)| {
            (installation_id, materialize_health_timeline(&stats, start))
        })
        .collect())
}

fn list_market_request_timeline_stats_24h(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<HashMap<String, Vec<HealthTimelineBucketStats>>, AppError> {
    let (start, end) = health_timeline_window(now);
    let mut by_market: HashMap<String, Vec<HealthTimelineBucketStats>> = HashMap::new();
    let mut stmt = conn
        .prepare(
            "SELECT market_email, status, status_code, created_at
             FROM market_request_logs
             WHERE created_at >= ?1",
        )
        .map_err(|e| AppError::Internal(format!("prepare market request timeline failed: {e}")))?;
    let rows = stmt
        .query_map(params![timeline_timestamp_rfc3339(start)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?.map(|value| value as u16),
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query market request timeline failed: {e}")))?;
    for row in rows {
        let (market_email, status, status_code, created_at) = row.map_err(|e| {
            AppError::Internal(format!("read market request timeline row failed: {e}"))
        })?;
        let Some(created_at) = parse_rfc3339_timestamp(&created_at) else {
            continue;
        };
        let Some(index) = health_timeline_bucket_index(created_at, start, end) else {
            continue;
        };
        let bucket = &mut by_market
            .entry(market_email.to_ascii_lowercase())
            .or_insert_with(empty_health_timeline_stats)[index];
        bucket.request_count += 1;
        if request_status_failed(&status, status_code) {
            bucket.failure_count += 1;
        }
    }
    Ok(by_market)
}

/// Healthy-minute counts inside the trailing 10 minutes. Used as the
/// confidence numerator for `stability`.
fn list_online_minutes_10m(conn: &Connection) -> Result<HashMap<String, usize>, AppError> {
    let cutoff = Utc::now().timestamp() - 10 * 60;
    let mut stmt = conn
        .prepare(
            "SELECT share_id, COUNT(DISTINCT checked_at / 60) AS online_minutes
             FROM share_health_checks
             WHERE checked_at >= ?1 AND is_healthy = 1
             GROUP BY share_id",
        )
        .map_err(|e| AppError::Internal(format!("prepare online minutes 10m failed: {e}")))?;
    let rows = stmt
        .query_map(params![cutoff], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })
        .map_err(|e| AppError::Internal(format!("query online minutes 10m failed: {e}")))?;
    let mut map = HashMap::new();
    for row in rows {
        let (share_id, online_minutes) =
            row.map_err(|e| AppError::Internal(format!("read 10m row failed: {e}")))?;
        map.insert(share_id, online_minutes.min(10));
    }
    Ok(map)
}

fn map_lease_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TunnelLease> {
    let share_json: Option<String> = row.get(10)?;
    Ok(TunnelLease {
        id: row.get(0)?,
        installation_id: row.get(1)?,
        connection_id: row.get(2)?,
        subdomain: row.get(3)?,
        tunnel_type: row.get(4)?,
        ssh_username: row.get(5)?,
        ssh_password: row.get(6)?,
        issued_at: parse_dt_sql(&row.get::<_, String>(7)?)?,
        expires_at: parse_dt_sql(&row.get::<_, String>(8)?)?,
        used_at: row
            .get::<_, Option<String>>(9)?
            .map(|value| parse_dt_sql(&value))
            .transpose()?,
        share: share_json
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
    })
}

fn parse_dt_sql(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, AppError> {
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Internal(format!("collect rows failed: {e}")))
}

fn normalize_subdomain(value: &str) -> Result<String, AppError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err(AppError::BadRequest("subdomain is required".into()));
    }
    if value.len() < 3 || value.len() > 63 {
        return Err(AppError::BadRequest("invalid subdomain".into()));
    }
    if value.starts_with('-') || value.ends_with('-') {
        return Err(AppError::BadRequest("invalid subdomain".into()));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(AppError::BadRequest("invalid subdomain".into()));
    }
    Ok(value)
}

fn normalize_market_kind(value: Option<&str>) -> Result<String, AppError> {
    let kind = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("usage")
        .to_ascii_lowercase();
    match kind.as_str() {
        "usage" | "share" => Ok(kind),
        _ => Err(AppError::BadRequest("invalid market kind".into())),
    }
}

fn ensure_subdomain_allowed(value: &str, config: &Config) -> Result<(), AppError> {
    const RESERVED: &[&str] = &["admin", "api", "www", "cdn-cgi"];
    if RESERVED.contains(&value) || config.is_market_subdomain(value) {
        return Err(AppError::Conflict("subdomain is reserved".into()));
    }
    Ok(())
}

fn ensure_subdomain_not_reserved_word(value: &str) -> Result<(), AppError> {
    const RESERVED: &[&str] = &["admin", "api", "www", "cdn-cgi"];
    if RESERVED.contains(&value) {
        return Err(AppError::Conflict("subdomain is reserved".into()));
    }
    Ok(())
}

fn ensure_subdomain_not_registered_market(conn: &Connection, value: &str) -> Result<(), AppError> {
    if market_subdomain_owner(conn, value)?.is_some() {
        return Err(AppError::Conflict("subdomain is reserved".into()));
    }
    Ok(())
}

fn ensure_subdomain_not_claimed_by_share(conn: &Connection, value: &str) -> Result<(), AppError> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM shares WHERE subdomain = ?1)",
            params![value],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("query share subdomain conflict failed: {e}")))?;
    if exists {
        return Err(AppError::Conflict(
            "subdomain already claimed by share".into(),
        ));
    }
    Ok(())
}

fn market_subdomain_owner(conn: &Connection, subdomain: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT email FROM router_markets
         WHERE subdomain = ?1
           AND status IN ('active', 'offline', 'disabled')",
        params![subdomain],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query market subdomain owner failed: {e}")))
}

fn market_display_name_from_url(public_base_url: &str) -> String {
    public_base_url.trim().trim_end_matches('/').to_string()
}

fn get_market_by_email(
    conn: &Connection,
    email: &str,
) -> Result<Option<MarketRegistryRecord>, AppError> {
    conn.query_row(
        "SELECT id, display_name, email, subdomain, public_base_url,
                COALESCE(market_kind, 'usage'), scopes_json, status,
                COALESCE(maintenance_enabled, 0), maintenance_message
         FROM router_markets
         WHERE email = ?1",
        params![email],
        |row| {
            Ok(MarketRegistryRecord {
                id: row.get(0)?,
                display_name: row.get(1)?,
                email: row.get(2)?,
                subdomain: row.get(3)?,
                public_base_url: row.get(4)?,
                market_kind: row.get(5)?,
                scopes: parse_string_vec(row.get(6)?)?,
                status: row.get(7)?,
                maintenance_enabled: row.get::<_, i64>(8)? != 0,
                maintenance_message: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query market by email failed: {e}")))
}

fn get_gateway_by_id(
    conn: &Connection,
    gateway_id: &str,
) -> Result<Option<GatewayRegistryRecord>, AppError> {
    conn.query_row(
        "SELECT id, owner_email, display_name, public_key, public_base_url, app_version,
                status, scopes_json
         FROM router_gateways
         WHERE id = ?1",
        params![gateway_id],
        gateway_record_from_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query gateway by id failed: {e}")))
}

fn get_gateway_by_public_key(
    conn: &Connection,
    public_key: &str,
) -> Result<Option<GatewayRegistryRecord>, AppError> {
    conn.query_row(
        "SELECT id, owner_email, display_name, public_key, public_base_url, app_version,
                status, scopes_json
         FROM router_gateways
         WHERE public_key = ?1",
        params![public_key],
        gateway_record_from_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query gateway by public key failed: {e}")))
}

fn gateway_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GatewayRegistryRecord> {
    Ok(GatewayRegistryRecord {
        id: row.get(0)?,
        owner_email: row.get(1)?,
        display_name: row.get(2)?,
        public_key: row.get(3)?,
        public_base_url: row.get(4)?,
        app_version: row.get(5)?,
        status: row.get(6)?,
        scopes: parse_string_vec(row.get(7)?)?,
    })
}

fn gateway_created_at(conn: &Connection, gateway_id: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT created_at FROM router_gateways WHERE id = ?1",
        params![gateway_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query gateway created_at failed: {e}")))
}

fn list_dashboard_markets(
    conn: &Connection,
    viewer_email: Option<&str>,
    active_subdomains: &HashSet<String>,
    shares: &[(String, ShareDescriptor)],
    inflight_by_share: &HashMap<String, usize>,
    online_by_share: &HashMap<String, usize>,
    health_by_share: &HashMap<String, Vec<HealthCheckEntry>>,
    health_timeline_by_share: &HashMap<String, Vec<HealthTimelineBucket>>,
    inflight_by_market_email: &HashMap<String, usize>,
    market_logs_by_market: &HashMap<String, Vec<DashboardMarketRequestLogView>>,
    market_request_timeline_by_market: &HashMap<String, Vec<HealthTimelineBucketStats>>,
    health_timeline_start: i64,
) -> Result<Vec<DashboardMarketView>, AppError> {
    let market_total_expr = market_log_total_tokens_expr("ml");
    let mut stmt = conn
        .prepare(&format!(
            "SELECT rm.id, rm.display_name, rm.email, rm.subdomain, rm.public_base_url,
                    COALESCE(rm.market_kind, 'usage') AS market_kind, rm.status,
                    rm.created_at, rm.updated_at, rm.last_seen_at, rm.offline_since,
                    rm.pricing_json,
                    COALESCE(rm.maintenance_enabled, 0) AS maintenance_enabled,
                    rm.maintenance_message,
                    COALESCE(SUM(
                        CASE
                            WHEN ml.usage_amount_usd IS NOT NULL THEN {market_total_expr}
                            ELSE 0
                        END
                    ), 0) AS usage_tokens,
                    COALESCE(SUM(CAST(COALESCE(ml.usage_amount_usd, '0') AS REAL)), 0) AS usage_amount_usd
             FROM router_markets rm
             LEFT JOIN market_request_logs ml ON lower(ml.market_email) = lower(rm.email)
             WHERE rm.status IN ('active', 'offline', 'disabled')
             GROUP BY rm.id, rm.display_name, rm.email, rm.subdomain, rm.public_base_url, rm.market_kind, rm.status,
                      rm.created_at, rm.updated_at, rm.last_seen_at, rm.offline_since, rm.pricing_json,
                      rm.maintenance_enabled, rm.maintenance_message
             ORDER BY rm.display_name ASC, rm.subdomain ASC",
        ))
        .map_err(|e| AppError::Internal(format!("prepare dashboard markets failed: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            let subdomain: String = row.get(3)?;
            Ok(DashboardMarketView {
                id: row.get(0)?,
                display_name: row.get(1)?,
                email: row.get(2)?,
                public_base_url: row.get(4)?,
                market_kind: row.get(5)?,
                status: row.get(6)?,
                online: active_subdomains.contains(&subdomain),
                can_manage: false,
                maintenance_enabled: row.get::<_, i64>(12)? != 0,
                maintenance_message: row.get(13)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                last_seen_at: row.get(9)?,
                offline_since: row.get(10)?,
                share_count: 0,
                online_share_count: 0,
                active_requests: 0,
                parallel_capacity: 0,
                online_minutes_24h: 0,
                online_rate_24h: 0.0,
                usage_tokens: row.get::<_, i64>(14)?.max(0) as u64,
                usage_amount_usd: format!("{:.8}", row.get::<_, f64>(15)?.max(0.0)),
                pricing_summary: parse_json_value(row.get(11)?)?,
                health_checks: Vec::new(),
                health_timeline: Vec::new(),
                linked_shares: Vec::new(),
                recent_requests: Vec::new(),
                operational_summary: OperationalSummary::healthy("available"),
                subdomain,
            })
        })
        .map_err(|e| AppError::Internal(format!("query dashboard markets failed: {e}")))?;
    let mut markets = collect_rows(rows)?;
    let disabled_by_market = list_market_disabled_share_map(conn)?;
    let runtime_states_by_market = list_market_share_runtime_state_map(conn)?;
    let app_availability_by_market = list_all_market_app_availability(conn)?;
    for market in &mut markets {
        market.can_manage = viewer_email
            .map(|email| email.eq_ignore_ascii_case(&market.email))
            .unwrap_or(false);
        enrich_dashboard_market(
            market,
            shares,
            &disabled_by_market,
            active_subdomains,
            inflight_by_share,
            online_by_share,
            health_by_share,
            health_timeline_by_share,
            inflight_by_market_email,
            market_logs_by_market,
            market_request_timeline_by_market,
            health_timeline_start,
            &app_availability_by_market,
            &runtime_states_by_market,
        );
        market.operational_summary = market_operational_summary(market);
    }
    Ok(markets)
}

fn enrich_dashboard_market(
    market: &mut DashboardMarketView,
    shares: &[(String, ShareDescriptor)],
    disabled_by_market: &HashMap<String, HashMap<String, String>>,
    active_subdomains: &HashSet<String>,
    inflight_by_share: &HashMap<String, usize>,
    online_by_share: &HashMap<String, usize>,
    health_by_share: &HashMap<String, Vec<HealthCheckEntry>>,
    health_timeline_by_share: &HashMap<String, Vec<HealthTimelineBucket>>,
    inflight_by_market_email: &HashMap<String, usize>,
    market_logs_by_market: &HashMap<String, Vec<DashboardMarketRequestLogView>>,
    market_request_timeline_by_market: &HashMap<String, Vec<HealthTimelineBucketStats>>,
    health_timeline_start: i64,
    app_availability_by_market: &HashMap<String, HashMap<String, MarketAppAvailability>>,
    runtime_states_by_market: &HashMap<String, HashMap<String, Vec<MarketShareRuntimeStateView>>>,
) {
    let market_email = market.email.to_ascii_lowercase();
    let is_share_market = market.market_kind == "share";
    let disabled_for_market = disabled_by_market.get(&market_email);
    let app_availability_for_market = app_availability_by_market.get(&market_email);
    let runtime_states_for_market = runtime_states_by_market.get(&market_email);
    let mut share_market_usage_tokens = 0_u64;
    let mut linked_shares = shares
        .iter()
        .filter_map(|(_, share)| {
            let explicit_market_match = share
                .shared_with_emails
                .iter()
                .any(|email| email.eq_ignore_ascii_case(&market_email));
            let visible_to_market = if is_share_market {
                share.sale_market_kind == "share" && explicit_market_match
            } else {
                share.sale_market_kind != "share"
                    && (share.market_access_mode == "all" || explicit_market_match)
            };
            if share.for_sale != "Yes" || !visible_to_market {
                return None;
            }
            if is_share_market {
                share_market_usage_tokens =
                    share_market_usage_tokens.saturating_add(share.tokens_used.max(0) as u64);
            }
            let active_requests = inflight_by_share.get(&share.share_id).copied().unwrap_or(0);
            let online =
                share.share_status == "active" && active_subdomains.contains(&share.subdomain);
            let online_minutes_24h = online_by_share.get(&share.share_id).copied().unwrap_or(0);
            let online_rate_24h =
                ((online_minutes_24h as f64 / ONLINE_WINDOW_MINUTES as f64) * 100.0).min(100.0);
            let market_disabled_at =
                disabled_for_market.and_then(|disabled| disabled.get(&share.share_id).cloned());
            Some(MarketLinkedShareView {
                share_id: share.share_id.clone(),
                share_name: share.share_name.clone(),
                subdomain: share.subdomain.clone(),
                owner_email: share.owner_email.clone(),
                app_type: share.app_type.clone(),
                online,
                active_requests,
                parallel_limit: share.parallel_limit,
                online_rate_24h,
                disabled_by_market: market_disabled_at.is_some(),
                market_disabled_at,
                support: share.support.clone(),
                app_runtimes: share.app_runtimes.clone(),
                app_availability: app_availability_for_market
                    .and_then(|entries| entries.get(&share.share_id))
                    .cloned()
                    .unwrap_or_default(),
                market_states: runtime_states_for_market
                    .and_then(|entries| entries.get(&share.share_id))
                    .cloned()
                    .unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();

    linked_shares.sort_by(|left, right| {
        right
            .online
            .cmp(&left.online)
            .then_with(|| right.active_requests.cmp(&left.active_requests))
            .then_with(|| {
                left.share_name
                    .to_ascii_lowercase()
                    .cmp(&right.share_name.to_ascii_lowercase())
            })
    });

    market.share_count = linked_shares.len();
    market.online_share_count = linked_shares
        .iter()
        .filter(|share| share.online && !share.disabled_by_market)
        .count();
    let enabled_linked_shares = linked_shares
        .iter()
        .filter(|share| !share.disabled_by_market)
        .collect::<Vec<_>>();
    let enabled_linked_share_count = enabled_linked_shares.len();
    market.active_requests = if is_share_market {
        enabled_linked_shares
            .iter()
            .map(|share| share.active_requests)
            .sum()
    } else {
        // Count only requests that actually traversed the market proxy. Direct
        // share-subdomain traffic stays out of this number even though it shows up
        // in the share-keyed limiter.
        inflight_by_market_email
            .get(&market_email)
            .copied()
            .unwrap_or(0)
    };
    market.parallel_capacity = if enabled_linked_shares
        .iter()
        .any(|share| share.parallel_limit < 0)
    {
        -1
    } else {
        enabled_linked_shares
            .iter()
            .map(|share| share.parallel_limit.max(0))
            .sum()
    };
    if is_share_market {
        market.usage_tokens = share_market_usage_tokens;
        market.usage_amount_usd = "0.00000000".to_string();
    }
    // Online minutes: take the max across linked shares — i.e. the best path
    // the market could route through. Approximates the union of healthy
    // minutes without an additional per-minute SQL pass; stays exact when the
    // market only has one linked share (the common case).
    market.online_minutes_24h = linked_shares
        .iter()
        .filter(|share| !share.disabled_by_market)
        .map(|share| {
            online_by_share
                .get(&share.share_id)
                .copied()
                .unwrap_or(0)
                .min(ONLINE_WINDOW_MINUTES as usize)
        })
        .max()
        .unwrap_or(0);
    if is_share_market && market.online_minutes_24h == 0 {
        if enabled_linked_share_count > 0 {
            market.online_minutes_24h = (market.online_share_count * ONLINE_WINDOW_MINUTES
                + enabled_linked_share_count / 2)
                / enabled_linked_share_count;
        } else if market.online {
            market.online_minutes_24h = ONLINE_WINDOW_MINUTES;
        }
    }
    market.online_rate_24h =
        ((market.online_minutes_24h as f64 / ONLINE_WINDOW_MINUTES as f64) * 100.0).min(100.0);
    market.health_checks = aggregate_market_health_checks(&linked_shares, health_by_share);
    if is_share_market && (market.online || market.online_share_count > 0) {
        if market.health_checks.is_empty() {
            append_recent_online_health_checks(&mut market.health_checks, 10);
        } else {
            append_current_online_health_check(&mut market.health_checks);
        }
    }
    market.health_timeline = merge_market_health_timeline(
        &linked_shares,
        health_timeline_by_share,
        market_request_timeline_by_market.get(&market_email),
        health_timeline_start,
    );
    market.linked_shares = linked_shares;
    market.recent_requests = market_logs_by_market
        .get(&market.email.to_ascii_lowercase())
        .cloned()
        .unwrap_or_default();
}

fn append_current_online_health_check(entries: &mut Vec<HealthCheckEntry>) {
    let current_minute = Utc::now().timestamp().div_euclid(60);
    let checked_at = current_minute * 60;
    if let Some(existing) = entries
        .iter_mut()
        .find(|entry| entry.checked_at.div_euclid(60) == current_minute)
    {
        existing.is_healthy = true;
        return;
    }
    entries.push(HealthCheckEntry {
        checked_at,
        is_healthy: true,
    });
    entries.sort_by_key(|entry| entry.checked_at);
    if entries.len() > 10 {
        let drop_count = entries.len() - 10;
        entries.drain(0..drop_count);
    }
}

fn append_recent_online_health_checks(entries: &mut Vec<HealthCheckEntry>, count: usize) {
    let current_minute = Utc::now().timestamp().div_euclid(60);
    for offset in (0..count).rev() {
        let checked_at = (current_minute - offset as i64) * 60;
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.checked_at.div_euclid(60) == current_minute - offset as i64)
        {
            existing.is_healthy = true;
        } else {
            entries.push(HealthCheckEntry {
                checked_at,
                is_healthy: true,
            });
        }
    }
    entries.sort_by_key(|entry| entry.checked_at);
    if entries.len() > count {
        let drop_count = entries.len() - count;
        entries.drain(0..drop_count);
    }
}

fn aggregate_market_health_checks(
    linked_shares: &[MarketLinkedShareView],
    health_by_share: &HashMap<String, Vec<HealthCheckEntry>>,
) -> Vec<HealthCheckEntry> {
    let mut by_minute = BTreeMap::<i64, bool>::new();
    for share in linked_shares
        .iter()
        .filter(|share| !share.disabled_by_market)
    {
        let Some(entries) = health_by_share.get(&share.share_id) else {
            continue;
        };
        for entry in entries {
            let minute = entry.checked_at.div_euclid(60);
            by_minute
                .entry(minute)
                .and_modify(|healthy| *healthy |= entry.is_healthy)
                .or_insert(entry.is_healthy);
        }
    }
    by_minute
        .into_iter()
        .rev()
        .take(10)
        .map(|(minute, is_healthy)| HealthCheckEntry {
            checked_at: minute * 60,
            is_healthy,
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn merge_health_checks(
    left: &[HealthCheckEntry],
    right: &[HealthCheckEntry],
) -> Vec<HealthCheckEntry> {
    let mut by_minute = BTreeMap::<i64, bool>::new();
    for entry in left.iter().chain(right.iter()) {
        let minute = entry.checked_at.div_euclid(60);
        by_minute
            .entry(minute)
            .and_modify(|healthy| *healthy |= entry.is_healthy)
            .or_insert(entry.is_healthy);
    }
    by_minute
        .into_iter()
        .rev()
        .take(10)
        .map(|(minute, is_healthy)| HealthCheckEntry {
            checked_at: minute * 60,
            is_healthy,
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn merge_health_timeline(
    left: &[HealthTimelineBucket],
    right: &[HealthTimelineBucket],
) -> Vec<HealthTimelineBucket> {
    let mut by_start = BTreeMap::<String, HealthTimelineBucket>::new();
    for bucket in left.iter().chain(right.iter()) {
        by_start
            .entry(bucket.start_at.clone())
            .and_modify(|existing| {
                existing.score = existing.score.max(bucket.score);
                existing.online_minutes = existing.online_minutes.max(bucket.online_minutes);
                existing.observed_minutes = existing.observed_minutes.max(bucket.observed_minutes);
                existing.request_count += bucket.request_count;
                existing.failure_count += bucket.failure_count;
                existing.status = stronger_timeline_status(&existing.status, &bucket.status);
            })
            .or_insert_with(|| bucket.clone());
    }
    by_start
        .into_values()
        .rev()
        .take(48)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn current_online_health_timeline(start: i64) -> Vec<HealthTimelineBucket> {
    let current = Utc::now().timestamp();
    (0..HEALTH_TIMELINE_BUCKETS)
        .map(|index| {
            let bucket_start = start + index as i64 * HEALTH_TIMELINE_BUCKET_SECS;
            let bucket_end = bucket_start + HEALTH_TIMELINE_BUCKET_SECS;
            let online = current >= bucket_start
                && (current < bucket_end || index + 1 == HEALTH_TIMELINE_BUCKETS);
            HealthTimelineBucket {
                start_at: timeline_timestamp_rfc3339(bucket_start),
                end_at: timeline_timestamp_rfc3339(bucket_end),
                status: if online { "healthy" } else { "unknown" }.into(),
                score: if online { 100.0 } else { 0.0 },
                online_minutes: if online { 1 } else { 0 },
                observed_minutes: if online { 1 } else { 0 },
                request_count: 0,
                failure_count: 0,
            }
        })
        .collect()
}

fn stronger_timeline_status(left: &str, right: &str) -> String {
    let score = |status: &str| match status {
        "healthy" => 4,
        "degraded" => 3,
        "unhealthy" => 2,
        "offline" => 1,
        _ => 0,
    };
    if score(right) > score(left) {
        right.to_string()
    } else {
        left.to_string()
    }
}

fn dashboard_market_to_share_link(market: &DashboardMarketView) -> ShareMarketLinkView {
    ShareMarketLinkView {
        id: market.id.clone(),
        display_name: market.display_name.clone(),
        email: market.email.clone(),
        subdomain: market.subdomain.clone(),
        public_base_url: market.public_base_url.clone(),
        market_kind: market.market_kind.clone(),
        status: market.status.clone(),
        online: market.online,
        listing_status_by_app: BTreeMap::new(),
    }
}

fn list_market_disabled_share_map(
    conn: &Connection,
) -> Result<HashMap<String, HashMap<String, String>>, AppError> {
    let mut stmt = conn
        .prepare("SELECT lower(market_email), share_id, created_at FROM market_disabled_shares")
        .map_err(|e| AppError::Internal(format!("prepare disabled market shares failed: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query disabled market shares failed: {e}")))?;
    let mut result: HashMap<String, HashMap<String, String>> = HashMap::new();
    for row in rows {
        let (market_email, share_id, created_at) =
            row.map_err(|e| AppError::Internal(format!("read disabled market share failed: {e}")))?;
        result
            .entry(market_email)
            .or_default()
            .insert(share_id, created_at);
    }
    Ok(result)
}

fn list_market_share_runtime_state_map(
    conn: &Connection,
) -> Result<HashMap<String, HashMap<String, Vec<MarketShareRuntimeStateView>>>, AppError> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "DELETE FROM market_share_runtime_states WHERE expires_at IS NOT NULL AND expires_at <= ?1",
        params![now],
    )
    .map_err(|e| {
        AppError::Internal(format!(
            "delete expired market share runtime states failed: {e}"
        ))
    })?;
    let mut stmt = conn
        .prepare(
            r#"
            SELECT lower(market_email), share_id, router_id, scope, kind, app_type, model_id,
                   model_name, reason_kind, reason, failure_count, expires_at, updated_at
              FROM market_share_runtime_states
             WHERE expires_at IS NULL OR expires_at > ?1
             ORDER BY updated_at DESC
            "#,
        )
        .map_err(|e| {
            AppError::Internal(format!("prepare market share runtime states failed: {e}"))
        })?;
    let rows = stmt
        .query_map(params![now], |row| {
            let share_id: String = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                share_id.clone(),
                MarketShareRuntimeStateView {
                    share_id,
                    router_id: row.get(2)?,
                    scope: row.get(3)?,
                    kind: row.get(4)?,
                    app_type: row.get(5)?,
                    model_id: row.get(6)?,
                    model_name: row.get(7)?,
                    reason_kind: row.get(8)?,
                    reason: row.get(9)?,
                    failure_count: row.get(10)?,
                    expires_at: row.get(11)?,
                    updated_at: row.get(12)?,
                },
            ))
        })
        .map_err(|e| {
            AppError::Internal(format!("query market share runtime states failed: {e}"))
        })?;
    let mut result: HashMap<String, HashMap<String, Vec<MarketShareRuntimeStateView>>> =
        HashMap::new();
    for row in rows {
        let (market_email, share_id, state) = row.map_err(|e| {
            AppError::Internal(format!("read market share runtime state failed: {e}"))
        })?;
        result
            .entry(market_email)
            .or_default()
            .entry(share_id)
            .or_default()
            .push(state);
    }
    Ok(result)
}

fn list_share_market_listing_status_map(
    conn: &Connection,
) -> Result<HashMap<(String, String, String, String), ShareMarketListingStatusView>, AppError> {
    let now = Utc::now().to_rfc3339();
    let mut stmt = conn
        .prepare(
            r#"
            SELECT lower(market_email), router_id, share_id, app_type, listing_url, status,
                   sale_mode, filled_seats, required_seats, listing_status, updated_at, expires_at,
                   CASE WHEN expires_at IS NOT NULL AND expires_at <= ?1 THEN 1 ELSE 0 END AS is_stale
              FROM share_market_listing_statuses
             ORDER BY updated_at DESC
            "#,
        )
        .map_err(|e| {
            AppError::Internal(format!("prepare share market listing statuses failed: {e}"))
        })?;
    let rows = stmt
        .query_map(params![now], |row| {
            let market_email: String = row.get(0)?;
            let router_id: String = row.get(1)?;
            let share_id: String = row.get(2)?;
            let app_type: String = row.get(3)?;
            Ok((
                (market_email, router_id, share_id, app_type),
                ShareMarketListingStatusView {
                    listing_url: row.get(4)?,
                    status: row.get(5)?,
                    sale_mode: row.get(6)?,
                    filled_seats: row.get(7)?,
                    required_seats: row.get(8)?,
                    listing_status: row.get(9)?,
                    updated_at: row.get(10)?,
                    expires_at: row.get(11)?,
                    is_stale: row.get::<_, i64>(12)? != 0,
                },
            ))
        })
        .map_err(|e| {
            AppError::Internal(format!("query share market listing statuses failed: {e}"))
        })?;
    let mut result = HashMap::new();
    for row in rows {
        let (key, value) = row.map_err(|e| {
            AppError::Internal(format!("read share market listing status failed: {e}"))
        })?;
        result.entry(key).or_insert(value);
    }
    Ok(result)
}

fn share_market_listing_status_visible_to_market(
    tx: &rusqlite::Transaction<'_>,
    market_email: &str,
    _router_id: &str,
    share_id: &str,
    app_type: &str,
) -> Result<bool, AppError> {
    let row = tx
        .query_row(
            r#"
            SELECT shared_with_emails_json, market_access_mode,
                   COALESCE(access_by_app_json, '{}'), COALESCE(app_settings_json, '{}'),
                   for_sale, sale_market_kind
              FROM shares
             WHERE share_id = ?1
               AND share_status = 'active'
            "#,
            params![share_id],
            |row| {
                Ok((
                    parse_string_vec(row.get(0)?)?,
                    row.get::<_, String>(1)?,
                    parse_share_access_by_app(row.get(2)?)?,
                    parse_share_app_settings(row.get(3)?)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("query listing status share failed: {e}")))?;
    let Some((
        shared_with_emails,
        market_access_mode,
        access_by_app,
        app_settings,
        for_sale,
        sale_market_kind,
    )) = row
    else {
        return Ok(false);
    };
    Ok(share_app_settings_visible_to_market(
        app_type,
        &app_settings,
        &access_by_app,
        &shared_with_emails,
        &market_access_mode,
        &for_sale,
        &sale_market_kind,
        "share",
        false,
        market_email,
    ))
}

fn list_market_visible_share_ids(
    conn: &Connection,
    market_email: &str,
) -> Result<HashSet<String>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT share_id, shared_with_emails_json, market_access_mode,
                    COALESCE(access_by_app_json, '{}'), COALESCE(app_settings_json, '{}'),
                    for_sale, sale_market_kind
             FROM shares
             WHERE share_status = 'active'
               AND subdomain IS NOT NULL
               AND subdomain != ''
               AND subdomain != '-'",
        )
        .map_err(|e| AppError::Internal(format!("prepare visible market shares failed: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                parse_string_vec(row.get(1)?)?,
                row.get::<_, String>(2)?,
                parse_share_access_by_app(row.get(3)?)?,
                parse_share_app_settings(row.get(4)?)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|e| AppError::Internal(format!("query visible market shares failed: {e}")))?;
    let mut result = HashSet::new();
    for row in rows {
        let (
            share_id,
            shared_with_emails,
            market_access_mode,
            access_by_app,
            app_settings,
            for_sale,
            sale_market_kind,
        ) =
            row.map_err(|e| AppError::Internal(format!("read visible market share failed: {e}")))?;
        let visible = ["claude", "codex", "gemini"].iter().any(|app| {
            share_app_settings_visible_to_market(
                app,
                &app_settings,
                &access_by_app,
                &shared_with_emails,
                &market_access_mode,
                &for_sale,
                &sale_market_kind,
                "token",
                true,
                market_email,
            )
        });
        if visible {
            result.insert(share_id);
        }
    }
    Ok(result)
}

fn get_share_owned_subdomain(
    conn: &Connection,
    installation_id: &str,
    share_id: &str,
) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT subdomain FROM shares WHERE installation_id = ?1 AND share_id = ?2",
        params![installation_id, share_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query owned subdomain failed: {e}")))
}

#[cfg(test)]
fn get_share_owner_email(conn: &Connection, share_id: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT owner_email FROM shares WHERE share_id = ?1",
        params![share_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query share owner email failed: {e}")))
}

fn get_share_owner_binding(
    conn: &Connection,
    share_id: &str,
) -> Result<Option<(String, Option<String>)>, AppError> {
    conn.query_row(
        "SELECT installation_id, owner_email FROM shares WHERE share_id = ?1",
        params![share_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query share owner binding failed: {e}")))
}

fn share_belongs_to_installation(
    conn: &Connection,
    share_id: &str,
    installation_id: &str,
) -> Result<bool, AppError> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM shares WHERE share_id = ?1 AND installation_id = ?2 LIMIT 1",
            params![share_id, installation_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("query share installation ownership failed: {e}")))?
        .is_some();
    Ok(exists)
}

fn ensure_share_id_writable_by_installation(
    conn: &Connection,
    share_id: &str,
    installation_id: &str,
) -> Result<(), AppError> {
    if let Some((existing_installation_id, _)) = get_share_owner_binding(conn, share_id)? {
        if existing_installation_id != installation_id {
            return Err(AppError::Unauthorized(
                "share id belongs to a different installation".into(),
            ));
        }
    }
    Ok(())
}

fn normalize_self_reported_share_owner(
    share: &mut ShareDescriptor,
    installation_owner: &str,
) -> Result<(), AppError> {
    let owner_email = share
        .owner_email
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("share owner email is required".into()))
        .and_then(normalize_email)?;
    if owner_email != installation_owner {
        return Err(AppError::Conflict(
            "share owner must match the installation owner".into(),
        ));
    }
    share.owner_email = Some(owner_email.clone());
    share.share_name = owner_email.clone();
    let allow_owner_in_acl = share.sale_market_kind == "share";
    share.shared_with_emails = normalize_email_list_with_options(
        &share.shared_with_emails,
        &owner_email,
        allow_owner_in_acl,
    );
    let mut access_by_app = BTreeMap::new();
    for (app, access) in std::mem::take(&mut share.access_by_app) {
        let app = normalize_share_acl_app(&app)?;
        access_by_app.insert(
            app,
            ShareAppAccess {
                shared_with_emails: normalize_email_list_with_options(
                    &access.shared_with_emails,
                    &owner_email,
                    allow_owner_in_acl,
                ),
                market_access_mode: normalize_market_access_mode(&access.market_access_mode)?,
            },
        );
    }
    share.access_by_app = access_by_app;
    Ok(())
}

fn rebind_installation_shares_to_owner(
    conn: &Connection,
    installation_id: &str,
    new_owner: &str,
) -> Result<usize, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT share_id, owner_email, shared_with_emails_json,
                    COALESCE(access_by_app_json, '{}'), COALESCE(app_settings_json, '{}')
             FROM shares WHERE installation_id = ?1",
        )
        .map_err(|error| {
            AppError::Internal(format!("prepare share owner rebind failed: {error}"))
        })?;
    let rows = statement
        .query_map(params![installation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|error| {
            AppError::Internal(format!("query shares for owner rebind failed: {error}"))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            AppError::Internal(format!("read share for owner rebind failed: {error}"))
        })?;
    drop(statement);

    let mut updated = 0;
    for (share_id, current_owner, shared_json, access_json, settings_json) in rows {
        if current_owner
            .as_deref()
            .and_then(|email| normalize_email(email).ok())
            .is_some_and(|email| email == new_owner)
        {
            continue;
        }
        let previous_owner = current_owner
            .as_deref()
            .and_then(|email| normalize_email(email).ok())
            .filter(|email| email != new_owner);
        let mut shared = parse_string_vec(Some(shared_json)).map_err(|error| {
            AppError::Internal(format!("parse share ACL for owner rebind failed: {error}"))
        })?;
        if let Some(previous_owner) = previous_owner.as_ref() {
            shared.push(previous_owner.clone());
        }
        shared = normalize_email_list(&shared, new_owner);

        let mut access = parse_share_access_by_app(Some(access_json)).map_err(|error| {
            AppError::Internal(format!("parse app ACL for owner rebind failed: {error}"))
        })?;
        for entry in access.values_mut() {
            if let Some(previous_owner) = previous_owner.as_ref() {
                entry.shared_with_emails.push(previous_owner.clone());
            }
            entry.shared_with_emails = normalize_email_list(&entry.shared_with_emails, new_owner);
        }
        let mut settings = parse_share_app_settings(Some(settings_json)).map_err(|error| {
            AppError::Internal(format!(
                "parse app settings for owner rebind failed: {error}"
            ))
        })?;
        for entry in settings.values_mut() {
            if let Some(previous_owner) = previous_owner.as_ref() {
                entry.shared_with_emails.push(previous_owner.clone());
            }
            entry.shared_with_emails = normalize_email_list(&entry.shared_with_emails, new_owner);
        }

        let shared = serde_json::to_string(&shared).map_err(|error| {
            AppError::Internal(format!("serialize share ACL rebind failed: {error}"))
        })?;
        let access = serde_json::to_string(&access).map_err(|error| {
            AppError::Internal(format!("serialize app ACL rebind failed: {error}"))
        })?;
        let settings = serde_json::to_string(&settings).map_err(|error| {
            AppError::Internal(format!("serialize app settings rebind failed: {error}"))
        })?;
        updated += conn
            .execute(
                "UPDATE shares
                 SET owner_email = ?2, share_name = ?2, shared_with_emails_json = ?3,
                     access_by_app_json = ?4, app_settings_json = ?5, updated_at = ?6
                 WHERE share_id = ?1",
                params![
                    share_id,
                    new_owner,
                    shared,
                    access,
                    settings,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|error| AppError::Internal(format!("rebind share owner failed: {error}")))?;
    }
    Ok(updated)
}

fn find_share_claim_by_subdomain(
    conn: &Connection,
    subdomain: &str,
) -> Result<Option<(String, String, Option<String>)>, AppError> {
    conn.query_row(
        "SELECT share_id, installation_id, owner_email FROM shares WHERE subdomain = ?1",
        params![subdomain],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query subdomain claim failed: {e}")))
}

fn release_reclaimable_subdomain_claim(
    conn: &Connection,
    incoming_installation_id: &str,
    incoming_share_id: &str,
    _incoming_owner_email: Option<&str>,
    subdomain: &str,
) -> Result<(), AppError> {
    let Some((existing_share_id, existing_installation_id, _existing_owner_email)) =
        find_share_claim_by_subdomain(conn, subdomain)?
    else {
        return Ok(());
    };

    if existing_share_id == incoming_share_id {
        return Ok(());
    }

    let same_installation = existing_installation_id == incoming_installation_id;
    if !same_installation {
        return Ok(());
    }

    conn.execute(
        "DELETE FROM share_request_logs WHERE share_id = ?1",
        params![existing_share_id],
    )
    .map_err(|e| AppError::Internal(format!("delete replaced share request logs failed: {e}")))?;
    conn.execute(
        "DELETE FROM share_health_checks WHERE share_id = ?1",
        params![existing_share_id],
    )
    .map_err(|e| AppError::Internal(format!("delete replaced share health checks failed: {e}")))?;
    conn.execute(
        "DELETE FROM shares WHERE share_id = ?1",
        params![existing_share_id],
    )
    .map_err(|e| AppError::Internal(format!("delete replaced share claim failed: {e}")))?;
    Ok(())
}

fn map_share_constraint_error(err: rusqlite::Error) -> AppError {
    let text = err.to_string();
    if text.contains("UNIQUE constraint failed: shares.subdomain")
        || text.contains("idx_shares_subdomain_unique")
    {
        AppError::Conflict("subdomain already claimed".into())
    } else {
        AppError::Internal(format!("upsert share failed: {text}"))
    }
}

#[derive(Debug, Clone)]
struct ClientTunnelRecord {
    installation_id: String,
    owner_email: String,
    subdomain: String,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_seen_at: Option<DateTime<Utc>>,
}

impl ClientTunnelRecord {
    fn into_view(self, config: &Config) -> ClientTunnelView {
        ClientTunnelView {
            installation_id: self.installation_id,
            owner_email: self.owner_email,
            subdomain: self.subdomain.clone(),
            enabled: self.enabled,
            tunnel_url: config.tunnel_url(&self.subdomain),
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_seen_at: self.last_seen_at,
        }
    }
}

fn map_client_tunnel_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClientTunnelRecord> {
    Ok(ClientTunnelRecord {
        installation_id: row.get(0)?,
        owner_email: row.get(1)?,
        subdomain: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        created_at: parse_dt_sql(&row.get::<_, String>(4)?)?,
        updated_at: parse_dt_sql(&row.get::<_, String>(5)?)?,
        last_seen_at: row
            .get::<_, Option<String>>(6)?
            .map(|value| parse_dt_sql(&value))
            .transpose()?,
    })
}

fn get_client_tunnel_by_installation(
    conn: &Connection,
    installation_id: &str,
) -> Result<Option<ClientTunnelRecord>, AppError> {
    conn.query_row(
        "SELECT installation_id, owner_email, subdomain, enabled, created_at, updated_at, last_seen_at
         FROM installation_client_tunnels WHERE installation_id = ?1",
        params![installation_id],
        map_client_tunnel_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query client tunnel failed: {e}")))
}

fn get_client_tunnel_by_subdomain(
    conn: &Connection,
    subdomain: &str,
) -> Result<Option<ClientTunnelRecord>, AppError> {
    conn.query_row(
        "SELECT installation_id, owner_email, subdomain, enabled, created_at, updated_at, last_seen_at
         FROM installation_client_tunnels WHERE subdomain = ?1",
        params![subdomain],
        map_client_tunnel_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query client tunnel by subdomain failed: {e}")))
}

fn list_client_tunnels(conn: &Connection) -> Result<Vec<ClientTunnelRecord>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT installation_id, owner_email, subdomain, enabled, created_at, updated_at, last_seen_at
             FROM installation_client_tunnels",
        )
        .map_err(|e| AppError::Internal(format!("prepare client tunnels failed: {e}")))?;
    let rows = stmt
        .query_map([], map_client_tunnel_row)
        .map_err(|e| AppError::Internal(format!("query client tunnels failed: {e}")))?;
    let mut output = Vec::new();
    for row in rows {
        output.push(
            row.map_err(|e| AppError::Internal(format!("read client tunnel row failed: {e}")))?,
        );
    }
    Ok(output)
}

fn map_client_tunnel_constraint_error(err: rusqlite::Error) -> AppError {
    let text = err.to_string();
    if text.contains("UNIQUE constraint failed: installation_client_tunnels.subdomain") {
        AppError::Conflict("client tunnel subdomain already claimed".into())
    } else {
        AppError::Internal(format!("upsert client tunnel failed: {text}"))
    }
}

fn validate_request_nonce(nonce: &str) -> Result<(), AppError> {
    if nonce.trim() != nonce
        || nonce.len() < 8
        || nonce.len() > 128
        || !nonce
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(AppError::BadRequest(
            "nonce must be 8-128 ASCII letters, digits, hyphen, or underscore".into(),
        ));
    }
    Ok(())
}

fn registration_nonce_subject(public_key: &str) -> String {
    let digest = Sha256::digest(public_key.trim().as_bytes());
    format!(
        "registration:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    )
}

fn verify_registration_recovery_signature(
    public_key: &str,
    input: &RegisterInstallationRequest,
    installation_id: &str,
    now: DateTime<Utc>,
) -> Result<bool, AppError> {
    let Some(timestamp_ms) = input.timestamp_ms else {
        if input.signature.is_some() {
            return Err(AppError::BadRequest(
                "timestamp_ms is required when registration signature is provided".into(),
            ));
        }
        return Ok(false);
    };
    let Some(signature) = input.signature.as_deref() else {
        return Err(AppError::BadRequest(
            "signature is required when registration timestamp_ms is provided".into(),
        ));
    };
    let skew = (now.timestamp_millis() - timestamp_ms).abs();
    if skew > SIGNED_REQUEST_MAX_SKEW_MS {
        return Err(AppError::Unauthorized(
            "stale registration recovery request".into(),
        ));
    }
    let payload = format!(
        "{}\nregister_installation\n{}\n{}\n{}\n{}\n{}",
        installation_id,
        input.public_key.trim(),
        input.platform.trim(),
        input.app_version,
        input.instance_nonce,
        timestamp_ms
    );
    verify_detached_signature(public_key, payload.as_bytes(), signature)?;
    Ok(true)
}

fn verify_issue_lease_request(
    conn: &Connection,
    public_key: &str,
    input: &IssueLeaseRequest,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let skew = (now.timestamp_millis() - input.timestamp_ms).abs();
    if skew > SIGNED_REQUEST_MAX_SKEW_MS {
        return Err(AppError::Unauthorized("stale lease request".into()));
    }
    verify_issue_lease_signature(public_key, input)?;
    consume_request_nonce(
        conn,
        &input.installation_id,
        "issue_lease",
        &input.nonce,
        now,
    )
}

fn verify_issue_lease_signature(
    public_key: &str,
    input: &IssueLeaseRequest,
) -> Result<(), AppError> {
    validate_request_nonce(&input.nonce)?;
    let payload = format!(
        "{}\n{}\n{}\n{}\n{}",
        input.installation_id,
        input.requested_subdomain,
        input.tunnel_type,
        input.timestamp_ms,
        input.nonce
    );
    verify_detached_signature(public_key, payload.as_bytes(), &input.signature)
}

fn verify_detached_signature(
    public_key: &str,
    payload: &[u8],
    signature: &str,
) -> Result<(), AppError> {
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key)
        .map_err(|_| AppError::Unauthorized("invalid stored public key".into()))?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| AppError::Unauthorized("invalid public key length".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&key_array)
        .map_err(|_| AppError::Unauthorized("invalid public key".into()))?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|_| AppError::Unauthorized("invalid signature".into()))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| AppError::Unauthorized("invalid signature length".into()))?;
    let signature = Signature::from_bytes(&sig_array);

    verifying_key
        .verify(payload, &signature)
        .map_err(|_| AppError::Unauthorized("signature verification failed".into()))
}

fn verify_signed_share_request<T: Serialize>(
    conn: &Connection,
    public_key: &str,
    installation_id: &str,
    action: &str,
    payload: &T,
    timestamp_ms: i64,
    nonce: &str,
    signature: &str,
) -> Result<(), AppError> {
    let now = Utc::now();
    let skew = (now.timestamp_millis() - timestamp_ms).abs();
    if skew > SIGNED_REQUEST_MAX_SKEW_MS {
        return Err(AppError::Unauthorized("stale signed request".into()));
    }

    verify_signed_payload(
        public_key,
        installation_id,
        action,
        payload,
        timestamp_ms,
        nonce,
        signature,
    )?;
    consume_request_nonce(conn, installation_id, action, nonce, now)
}

fn verify_signed_gateway_request(
    conn: &Connection,
    public_key: &str,
    gateway_id: &str,
    action: &str,
    body_sha256_hex: &str,
    timestamp_ms: i64,
    nonce: &str,
    signature: &str,
) -> Result<(), AppError> {
    let now = Utc::now();
    let skew = (now.timestamp_millis() - timestamp_ms).abs();
    if skew > SIGNED_REQUEST_MAX_SKEW_MS {
        return Err(AppError::Unauthorized(
            "stale signed gateway request".into(),
        ));
    }
    verify_gateway_signature(
        public_key,
        gateway_id,
        action,
        body_sha256_hex,
        timestamp_ms,
        nonce,
        signature,
    )?;
    consume_request_nonce(conn, gateway_id, action, nonce, now)
}

fn verify_share_claim_request(
    conn: &Connection,
    public_key: &str,
    installation_id: &str,
    share: &ShareDescriptor,
    claim: Option<&ShareClaimPayload>,
    timestamp_ms: i64,
    nonce: &str,
    signature: &str,
) -> Result<(), AppError> {
    let now = Utc::now();
    let skew = (now.timestamp_millis() - timestamp_ms).abs();
    if skew > SIGNED_REQUEST_MAX_SKEW_MS {
        return Err(AppError::Unauthorized("stale signed request".into()));
    }

    let derived_claim = share_claim_payload(share);
    if let Some(claim) = claim {
        if claim.share_id != derived_claim.share_id
            || claim.subdomain != derived_claim.subdomain
            || claim.owner_email != derived_claim.owner_email
        {
            return Err(AppError::BadRequest(
                "share claim does not match share metadata".into(),
            ));
        }
    }

    let claim_payload = claim.unwrap_or(&derived_claim);
    let new_result = verify_signed_payload(
        public_key,
        installation_id,
        "share_claim_subdomain",
        claim_payload,
        timestamp_ms,
        nonce,
        signature,
    );
    if let Err(new_err) = new_result {
        verify_signed_payload(
            public_key,
            installation_id,
            "share_claim_subdomain",
            share,
            timestamp_ms,
            nonce,
            signature,
        )
        .map_err(|_| new_err)?;
    }

    consume_request_nonce(conn, installation_id, "share_claim_subdomain", nonce, now)
}

fn share_claim_payload(share: &ShareDescriptor) -> ShareClaimPayload {
    ShareClaimPayload {
        share_id: share.share_id.clone(),
        subdomain: share.subdomain.clone(),
        owner_email: share.owner_email.clone(),
    }
}

fn consume_request_nonce(
    conn: &Connection,
    installation_id: &str,
    action: &str,
    nonce: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO request_nonces (installation_id, action, nonce, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![installation_id, action, nonce, now.to_rfc3339()],
    )
    .map_err(|err| {
        let text = err.to_string();
        if text.contains("UNIQUE constraint failed")
            || text.contains("request_nonces.installation_id")
        {
            AppError::Unauthorized("nonce already used".into())
        } else {
            AppError::Internal(format!("store request nonce failed: {text}"))
        }
    })?;
    Ok(())
}

fn verify_signed_payload<T: Serialize>(
    public_key: &str,
    installation_id: &str,
    action: &str,
    payload: &T,
    timestamp_ms: i64,
    nonce: &str,
    signature: &str,
) -> Result<(), AppError> {
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key)
        .map_err(|_| AppError::Unauthorized("invalid stored public key".into()))?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| AppError::Unauthorized("invalid public key length".into()))?;
    let verifying_key = VerifyingKey::from_bytes(&key_array)
        .map_err(|_| AppError::Unauthorized("invalid public key".into()))?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|_| AppError::Unauthorized("invalid signature".into()))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| AppError::Unauthorized("invalid signature length".into()))?;
    let signature = Signature::from_bytes(&sig_array);

    let payload_json = serde_json::to_string(payload)
        .map_err(|_| AppError::Unauthorized("invalid signed payload".into()))?;
    let payload = format!(
        "{}\n{}\n{}\n{}\n{}",
        installation_id, action, payload_json, timestamp_ms, nonce
    );
    verifying_key
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| AppError::Unauthorized("signature verification failed".into()))
}

fn verify_gateway_signature(
    public_key: &str,
    gateway_id: &str,
    action: &str,
    body_sha256_hex: &str,
    timestamp_ms: i64,
    nonce: &str,
    signature: &str,
) -> Result<(), AppError> {
    let verifying_key = ed25519_verifying_key(public_key)?;
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|_| AppError::Unauthorized("invalid gateway signature".into()))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| AppError::Unauthorized("invalid gateway signature length".into()))?;
    let signature = Signature::from_bytes(&sig_array);
    let payload = format!(
        "{}\n{}\n{}\n{}\n{}",
        gateway_id, action, body_sha256_hex, timestamp_ms, nonce
    );
    verifying_key
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| AppError::Unauthorized("gateway signature verification failed".into()))
}

fn validate_ed25519_public_key(public_key: &str) -> Result<(), AppError> {
    ed25519_verifying_key(public_key).map(|_| ())
}

fn ed25519_verifying_key(public_key: &str) -> Result<VerifyingKey, AppError> {
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key)
        .map_err(|_| AppError::BadRequest("invalid public key".into()))?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| AppError::BadRequest("invalid public key length".into()))?;
    VerifyingKey::from_bytes(&key_array)
        .map_err(|_| AppError::BadRequest("invalid public key".into()))
}

async fn redeem_verification_token(
    config: &Config,
    verification_token: &str,
) -> Result<VerificationRedeemResponse, AppError> {
    let url = format!(
        "{}/v1/verification/email/redeem",
        config.verification_service_base_url.trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()
        .map_err(|e| AppError::Internal(format!("create verification client failed: {e}")))?;
    let mut request = client.post(&url).json(&serde_json::json!({
        "verificationToken": verification_token,
        "purpose": AUTH_PURPOSE_LOGIN,
    }));
    if let Some(api_key) = config.verification_service_api_key.as_deref() {
        request = request.bearer_auth(api_key);
    }
    let response = request.send().await.map_err(|e| {
        AppError::Internal(format!("redeem verification token request failed: {e}"))
    })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| format!("HTTP {status}"));
        return Err(AppError::Unauthorized(format!(
            "redeem verification token failed: {body}"
        )));
    }
    response
        .json::<VerificationRedeemResponse>()
        .await
        .map_err(|e| AppError::Internal(format!("parse verification redeem response failed: {e}")))
}

#[derive(Debug, Clone)]
struct EmailLoginChallenge {
    id: String,
    code_hash: String,
    attempt_count: i64,
}

fn normalize_email(value: &str) -> Result<String, AppError> {
    let email = value.trim().to_ascii_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(AppError::BadRequest("invalid email".into()));
    };
    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    if email.len() > 254 {
        return Err(AppError::BadRequest("invalid email".into()));
    }
    Ok(email)
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_runtime_state_token(value: &str, field: &str) -> Result<String, AppError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
    {
        return Err(AppError::BadRequest(format!(
            "invalid market share runtime state {field}"
        )));
    }
    Ok(value)
}

fn normalize_app_type_token(value: &str) -> Result<String, AppError> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "claude" | "codex" | "gemini" => Ok(value),
        _ => Err(AppError::BadRequest("invalid app type".into())),
    }
}

fn normalize_listing_status_token(value: &str) -> Result<String, AppError> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "idle" | "carpooling" | "full" | "unavailable" | "unknown" => Ok(value),
        _ => Err(AppError::BadRequest("invalid listing status".into())),
    }
}

fn normalize_optional_listing_sale_mode(value: Option<String>) -> Result<Option<String>, AppError> {
    let Some(value) = normalize_optional_string(value) else {
        return Ok(None);
    };
    let value = value.to_ascii_lowercase();
    match value.as_str() {
        "single" | "carpool" => Ok(Some(value)),
        _ => Err(AppError::BadRequest("invalid listing sale mode".into())),
    }
}

fn normalize_email_list(values: &[String], owner_email: &str) -> Vec<String> {
    normalize_email_list_with_options(values, owner_email, false)
}

fn normalize_email_list_with_options(
    values: &[String],
    owner_email: &str,
    allow_owner: bool,
) -> Vec<String> {
    let mut result = Vec::new();
    for value in values {
        if let Ok(email) = normalize_email(value) {
            if (!allow_owner && email == owner_email) || result.contains(&email) {
                continue;
            }
            result.push(email);
        }
    }
    result
}

fn mask_email(email: &str) -> String {
    let Some((local, domain)) = email.split_once('@') else {
        return "***".into();
    };
    let mut chars = local.chars();
    let first = chars.next().unwrap_or('*');
    let last = local.chars().last().unwrap_or(first);
    format!("{first}***{last}@{domain}")
}

fn hash_token(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

fn truncate_error(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

fn generate_secret(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), len)
}

fn generate_numeric_code(len: usize) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| char::from(b'0' + rng.gen_range(0..10)))
        .collect()
}

fn parse_string_vec(value: Option<String>) -> Result<Vec<String>, rusqlite::Error> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

async fn send_login_code_email(
    resend: &Resend,
    config: &Config,
    email: &str,
    code: &str,
    ttl_secs: i64,
) -> Result<Option<String>, AppError> {
    let from = resend_from_address(config)?;
    let ttl_minutes = (ttl_secs / 60).max(1);
    let body = format!(
        "<p style=\"margin:0 0 18px;color:#475569;font-size:15px;line-height:1.6\">Use this code to finish signing in to <span translate=\"no\">TokenSwitch</span>.</p>\
         {}\
         <p style=\"margin:0;color:#64748b;font-size:14px;line-height:1.6\">This code expires in {} minutes. If you did not request it, you can safely ignore this email.</p>",
        render_verification_code_block(code),
        ttl_minutes
    );
    let html = render_email_layout(
        "Your verification code",
        "Use this code to finish signing in.",
        &body,
    );
    let mut message =
        CreateEmailBaseOptions::new(&from, [email], "Your TokenSwitch verification code")
            .with_html(&html);
    if let Some(reply_to) = config.resend_reply_to.as_deref() {
        message = message.with_reply(reply_to);
    }
    let response = resend
        .emails
        .send(message)
        .await
        .map_err(|e| AppError::Internal(format!("send verification email failed: {e}")))?;
    Ok(Some(response.id.to_string()))
}

fn resend_from_address(config: &Config) -> Result<String, AppError> {
    let from = config
        .resend_from
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Internal("resend from address is not configured".into()))?;
    let from = from
        .chars()
        .filter(|ch| !matches!(ch, '\r' | '\n'))
        .collect::<String>();
    let from = from.trim();

    if from.contains('<') && from.contains('>') {
        return Ok(from.to_string());
    }

    if !from.contains('@') {
        return Err(AppError::Internal(
            "resend from address must be an email address".into(),
        ));
    }

    let name = config
        .resend_from_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("TokenSwitch");
    Ok(format!("{} <{}>", sanitize_email_display_name(name), from))
}

fn sanitize_email_display_name(value: &str) -> String {
    let sanitized = value
        .chars()
        .filter(|ch| !matches!(ch, '\r' | '\n' | '<' | '>' | '"'))
        .collect::<String>()
        .trim()
        .to_string();
    if sanitized.is_empty() {
        "TokenSwitch".to_string()
    } else {
        sanitized
    }
}

fn render_verification_code_block(code: &str) -> String {
    format!(
        "<table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"margin:22px 0;border-collapse:separate\">\
           <tr>\
             <td align=\"center\" translate=\"no\" style=\"padding:18px 20px;border-radius:12px;background:#0f172a;color:#ffffff;font-family:ui-monospace,SFMono-Regular,Menlo,Monaco,Consolas,'Liberation Mono','Courier New',monospace;font-size:32px;line-height:1.15;font-weight:700;letter-spacing:8px;text-align:center;white-space:nowrap\">{}</td>\
           </tr>\
         </table>",
        escape_html(code)
    )
}

fn render_email_layout(title: &str, preheader: &str, body_html: &str) -> String {
    format!(
        "<!doctype html>\
         <html lang=\"en\">\
           <head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><meta name=\"x-apple-disable-message-reformatting\"></head>\
           <body style=\"margin:0;background:#f6f7fb;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Arial,sans-serif;color:#0f172a\">\
             <div style=\"display:none;max-height:0;overflow:hidden;opacity:0;color:transparent\">{preheader}</div>\
             <table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"width:100%;background:#f6f7fb;border-collapse:collapse\">\
               <tr><td align=\"center\">\
                 <table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"width:100%;max-width:560px;margin:28px auto 0;background:#ffffff;border:1px solid #e5e7eb;border-bottom:0;border-radius:18px 18px 0 0;border-collapse:separate;box-shadow:0 16px 40px rgba(15,23,42,0.08)\">\
                   <tr><td style=\"padding:26px 28px 18px;border-bottom:1px solid #eef2f7;background:#ffffff;border-radius:18px 18px 0 0\">\
                     <div translate=\"no\" style=\"font-size:13px;font-weight:700;letter-spacing:.08em;text-transform:uppercase;color:#2563eb\">TokenSwitch</div>\
                     <h1 style=\"margin:10px 0 0;font-size:24px;line-height:1.25;color:#0f172a\">{title}</h1>\
                   </td></tr>\
                 </table>\
                 <table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"width:100%;max-width:560px;margin:0 auto;background:#ffffff;border-left:1px solid #e5e7eb;border-right:1px solid #e5e7eb;border-collapse:collapse\">\
                   <tr><td style=\"padding:28px\">{body_html}</td></tr>\
                 </table>\
                 <table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"width:100%;max-width:560px;margin:0 auto 28px;background:#f8fafc;border:1px solid #e5e7eb;border-top:1px solid #eef2f7;border-radius:0 0 18px 18px;border-collapse:separate\">\
                   <tr><td style=\"padding:18px 28px;color:#64748b;font-size:12px;line-height:1.6;border-radius:0 0 18px 18px\">\
                     <span translate=\"no\">TokenSwitch</span> router notification. If this email was unexpected, no action is required.\
                   </td></tr>\
                 </table>\
               </td></tr>\
             </table>\
           </body>\
         </html>",
        preheader = escape_html(preheader),
        title = escape_html(title),
        body_html = body_html
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn normalize_market_notification_kind(kind: &str) -> Result<String, AppError> {
    let kind = kind.trim().to_ascii_lowercase();
    match kind.as_str() {
        "topup_paid" | "topup_refunded" | "topup_chargeback" | "payout_submitted"
        | "payout_paid" | "payout_failed" | "payout_review" | "payout_cancelled" => Ok(kind),
        _ => Err(AppError::BadRequest("unsupported notification kind".into())),
    }
}

fn normalize_market_notification_locale(locale: Option<&str>) -> &'static str {
    match locale.unwrap_or("zh-CN").trim() {
        "en" | "en-US" => "en",
        _ => "zh-CN",
    }
}

fn validate_market_notification_payload(
    kind: &str,
    data: &serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let obj = data
        .as_object()
        .ok_or_else(|| AppError::BadRequest("notification data must be an object".into()))?;
    let required = match kind {
        "topup_paid" | "topup_refunded" | "topup_chargeback" => [
            "topupId",
            "grossAmountUsd",
            "feeAmountUsd",
            "netAmountUsd",
            "dashboardUrl",
        ]
        .as_slice(),
        "payout_submitted" | "payout_paid" | "payout_failed" | "payout_review"
        | "payout_cancelled" => [
            "payoutId",
            "amountUsd",
            "feeUsd",
            "netPayoutUsd",
            "claimUrl",
        ]
        .as_slice(),
        _ => &[],
    };
    for key in required {
        if !obj.contains_key(*key) {
            return Err(AppError::BadRequest(format!(
                "notification data missing field: {key}"
            )));
        }
    }
    Ok(data.clone())
}

fn market_notification_subject(kind: &str) -> String {
    // 所有 router 发出的邮件均为纯英文，与 send_login_code_email 一致。`locale`
    // 字段仍按客户端请求写入 DB（market_notification_emails.locale），但渲染恒用
    // 英文，避免按用户偏好回退到中文。
    match kind {
        "topup_paid" => "Top-up received · cc-switch Market".into(),
        "topup_refunded" => "Top-up refunded · cc-switch Market".into(),
        "topup_chargeback" => "Top-up chargeback notice · cc-switch Market".into(),
        "payout_submitted" => "Payout submitted · cc-switch Market".into(),
        "payout_paid" => "Payout completed · cc-switch Market".into(),
        "payout_failed" => "Payout failed · cc-switch Market".into(),
        "payout_review" => "Payout under review · cc-switch Market".into(),
        "payout_cancelled" => "Payout cancelled · cc-switch Market".into(),
        _ => "cc-switch Market notification".into(),
    }
}

fn render_market_notification_html(
    kind: &str,
    market: &MarketRegistryRecord,
    payload: &serde_json::Value,
) -> String {
    let get = |key: &str| payload.get(key).and_then(|v| v.as_str()).unwrap_or("-");
    let market_name = escape_html(&market.display_name);
    let topup_details = || {
        vec![
            ("Top-up ID", get("topupId").to_string()),
            ("Gross", format!("${}", get("grossAmountUsd"))),
            ("Fee", format!("${}", get("feeAmountUsd"))),
            ("Net", format!("${}", get("netAmountUsd"))),
        ]
    };
    let payout_details = || {
        vec![
            ("Payout ID", get("payoutId").to_string()),
            ("Gross", format!("${}", get("amountUsd"))),
            ("Fee", format!("${}", get("feeUsd"))),
            ("Net", format!("${}", get("netPayoutUsd"))),
        ]
    };

    match kind {
        "topup_paid" => render_market_notification_card(
            "Top-up received",
            &format!("Your balance has been credited on {market_name}."),
            "Open dashboard",
            get("dashboardUrl"),
            topup_details(),
        ),
        "topup_refunded" => render_market_notification_card(
            "Top-up refunded",
            &format!("Your top-up has been refunded on {market_name}."),
            "Open dashboard",
            get("dashboardUrl"),
            topup_details(),
        ),
        "topup_chargeback" => render_market_notification_card(
            "Top-up chargeback notice",
            &format!("A chargeback or dispute has been recorded on {market_name}."),
            "Open dashboard",
            get("dashboardUrl"),
            topup_details(),
        ),
        "payout_submitted" => render_market_notification_card(
            "Payout submitted",
            &format!("Your payout request has been created on {market_name}."),
            "Open claim page",
            get("claimUrl"),
            payout_details(),
        ),
        "payout_paid" => render_market_notification_card(
            "Payout completed",
            &format!("Your payout has been completed on {market_name}."),
            "Open claim page",
            get("claimUrl"),
            payout_details(),
        ),
        "payout_failed" => render_market_notification_card(
            "Payout failed",
            &format!("Your payout could not be completed on {market_name}."),
            "Open claim page",
            get("claimUrl"),
            payout_details(),
        ),
        "payout_review" => render_market_notification_card(
            "Payout under review",
            &format!(
                "Your payout is being reviewed on {market_name}. Please do not submit another payout."
            ),
            "Open claim page",
            get("claimUrl"),
            payout_details(),
        ),
        "payout_cancelled" => render_market_notification_card(
            "Payout cancelled",
            &format!("Your payout was cancelled on {market_name}."),
            "Open claim page",
            get("claimUrl"),
            payout_details(),
        ),
        _ => render_market_notification_card(
            "Notification",
            "You have a new TokenSwitch Market notification.",
            "Open dashboard",
            "#",
            Vec::new(),
        ),
    }
}

fn render_market_notification_card(
    heading: &str,
    message: &str,
    action_label: &str,
    action_url: &str,
    details: Vec<(&'static str, String)>,
) -> String {
    let mut rows = String::new();
    for (label, value) in details {
        rows.push_str(&format!(
            "<tr>\
               <td style=\"padding:10px 12px;color:#64748b;font-size:13px;border-bottom:1px solid #eef2f7\">{}</td>\
               <td align=\"right\" style=\"padding:10px 12px;color:#0f172a;font-size:13px;font-weight:600;border-bottom:1px solid #eef2f7\">{}</td>\
             </tr>",
            escape_html(label),
            escape_html(&value)
        ));
    }

    let href = normalize_email_href(action_url);
    let action = if href == "#" {
        String::new()
    } else {
        format!(
            "<p style=\"margin:24px 0 0\"><a href=\"{}\" style=\"display:inline-block;background:#2563eb;color:#ffffff;text-decoration:none;border-radius:8px;padding:11px 16px;font-size:14px;font-weight:700\">{}</a></p>",
            href,
            escape_html(action_label)
        )
    };

    let details_table = if rows.is_empty() {
        String::new()
    } else {
        format!(
            "<table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"margin:22px 0 0;border:1px solid #e5e7eb;border-radius:10px;overflow:hidden;background:#ffffff\">{rows}</table>"
        )
    };

    format!(
        "<div>\
           <h2 style=\"margin:0 0 10px;color:#0f172a;font-size:20px;line-height:1.35\">{}</h2>\
           <p style=\"margin:0;color:#475569;font-size:15px;line-height:1.6\">{}</p>\
           {}\
           {}\
         </div>",
        escape_html(heading),
        message,
        details_table,
        action
    )
}

fn normalize_email_href(value: &str) -> String {
    let value = value.trim();
    if value.starts_with("https://") || value.starts_with("http://") {
        escape_html(value)
    } else {
        "#".to_string()
    }
}

async fn send_market_template_email(
    resend: &Resend,
    config: &Config,
    email: &str,
    subject: &str,
    html: &str,
) -> Result<Option<String>, AppError> {
    let from = resend_from_address(config)?;
    let html = render_email_layout(subject, subject, html);
    let mut message = CreateEmailBaseOptions::new(&from, [email], subject).with_html(&html);
    if let Some(reply_to) = config.resend_reply_to.as_deref() {
        message = message.with_reply(reply_to);
    }
    let response =
        resend.emails.send(message).await.map_err(|e| {
            AppError::Internal(format!("send market notification email failed: {e}"))
        })?;
    Ok(Some(response.id.to_string()))
}

fn enforce_auth_send_limits(
    conn: &Connection,
    config: &Config,
    email: &str,
    installation_id: &str,
    metadata: &ClientMetadata,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let hour_cutoff = (now - Duration::hours(1)).to_rfc3339();
    if let Some(next_allowed_at) = latest_challenge_cooldown(conn, email, installation_id)? {
        if next_allowed_at > now {
            return Err(AppError::TooManyRequests(format!(
                "verification email cooldown active, retry in {}s",
                (next_allowed_at - now).num_seconds().max(1)
            )));
        }
    }

    let email_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM email_login_challenges
             WHERE email_normalized = ?1 AND created_at >= ?2",
            params![email, hour_cutoff],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("count auth email requests failed: {e}")))?;
    if email_count >= config.auth_email_hourly_limit {
        return Err(AppError::TooManyRequests(
            "email verification rate limit exceeded".into(),
        ));
    }

    let installation_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM email_login_challenges
             WHERE installation_id = ?1 AND created_at >= ?2",
            params![installation_id, hour_cutoff],
            |row| row.get(0),
        )
        .map_err(|e| AppError::Internal(format!("count installation auth requests failed: {e}")))?;
    if installation_count >= config.auth_installation_hourly_limit {
        return Err(AppError::TooManyRequests(
            "installation verification rate limit exceeded".into(),
        ));
    }

    if let Some(ip) = metadata.ip.as_deref() {
        let ip_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM email_login_challenges
                 WHERE created_ip = ?1 AND created_at >= ?2",
                params![ip, hour_cutoff],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Internal(format!("count ip auth requests failed: {e}")))?;
        if ip_count >= config.auth_ip_hourly_limit {
            return Err(AppError::TooManyRequests(
                "ip verification rate limit exceeded".into(),
            ));
        }
    }

    Ok(())
}

fn latest_challenge_cooldown(
    conn: &Connection,
    email: &str,
    installation_id: &str,
) -> Result<Option<DateTime<Utc>>, AppError> {
    conn.query_row(
        "SELECT resend_available_at
         FROM email_login_challenges
         WHERE email_normalized = ?1
           AND installation_id = ?2
         ORDER BY created_at DESC
         LIMIT 1",
        params![email, installation_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query latest challenge cooldown failed: {e}")))?
    .map(|value| {
        parse_dt_sql(&value).map_err(|e| AppError::Internal(format!("parse cooldown failed: {e}")))
    })
    .transpose()
}

fn get_latest_active_email_challenge(
    conn: &Connection,
    email: &str,
    installation_id: &str,
    purpose: &str,
    now: DateTime<Utc>,
) -> Result<Option<EmailLoginChallenge>, AppError> {
    conn.query_row(
        "SELECT id, code_hash, attempt_count
         FROM email_login_challenges
         WHERE email_normalized = ?1
           AND installation_id = ?2
           AND purpose = ?3
           AND consumed_at IS NULL
           AND expires_at >= ?4
         ORDER BY created_at DESC
         LIMIT 1",
        params![email, installation_id, purpose, now.to_rfc3339()],
        |row| {
            Ok(EmailLoginChallenge {
                id: row.get(0)?,
                code_hash: row.get(1)?,
                attempt_count: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query auth challenge failed: {e}")))
}

fn upsert_user_by_email(
    conn: &Connection,
    email: &str,
    now: DateTime<Utc>,
) -> Result<AuthUser, AppError> {
    if let Some(user) = get_user_by_email(conn, email)? {
        conn.execute(
            "UPDATE users SET last_login_at = ?2 WHERE id = ?1",
            params![user.id, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("update user login failed: {e}")))?;
        return Ok(user);
    }
    let user = AuthUser {
        id: Uuid::new_v4().to_string(),
        email: email.to_string(),
    };
    conn.execute(
        "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
         VALUES (?1, ?2, 'active', ?3, ?4)",
        params![user.id, user.email, now.to_rfc3339(), now.to_rfc3339()],
    )
    .map_err(|e| AppError::Internal(format!("insert user failed: {e}")))?;
    Ok(user)
}

fn get_user_by_email(conn: &Connection, email: &str) -> Result<Option<AuthUser>, AppError> {
    conn.query_row(
        "SELECT id, email_normalized FROM users WHERE email_normalized = ?1",
        params![email],
        |row| {
            Ok(AuthUser {
                id: row.get(0)?,
                email: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query user by email failed: {e}")))
}

fn get_user_by_id(conn: &Connection, user_id: &str) -> Result<Option<AuthUser>, AppError> {
    conn.query_row(
        "SELECT id, email_normalized FROM users WHERE id = ?1",
        params![user_id],
        |row| {
            Ok(AuthUser {
                id: row.get(0)?,
                email: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query user by id failed: {e}")))
}

fn persist_session(conn: &Connection, session: &AuthSession) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO user_sessions (
            id, user_id, installation_id, access_token_hash, refresh_token_hash,
            access_expires_at, refresh_expires_at, revoked_at, created_at, last_used_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9)",
        params![
            session.session_id,
            session.user_id,
            session.installation_id,
            session.access_token_hash,
            session.refresh_token_hash,
            session.access_expires_at.to_rfc3339(),
            session.refresh_expires_at.to_rfc3339(),
            session.created_at.to_rfc3339(),
            session.last_used_at.to_rfc3339(),
        ],
    )
    .map_err(|e| AppError::Internal(format!("persist session failed: {e}")))?;
    Ok(())
}

fn map_auth_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthSession> {
    Ok(AuthSession {
        session_id: row.get(0)?,
        user_id: row.get(1)?,
        installation_id: row.get(2)?,
        access_token_hash: row.get(3)?,
        refresh_token_hash: row.get(4)?,
        access_expires_at: parse_dt_sql(&row.get::<_, String>(5)?)?,
        refresh_expires_at: parse_dt_sql(&row.get::<_, String>(6)?)?,
        created_at: parse_dt_sql(&row.get::<_, String>(7)?)?,
        last_used_at: parse_dt_sql(&row.get::<_, String>(8)?)?,
        email: row.get(9)?,
    })
}

fn get_session_by_access_hash(
    conn: &Connection,
    access_hash: &str,
) -> Result<Option<AuthSession>, AppError> {
    conn.query_row(
        "SELECT s.id, s.user_id, s.installation_id, s.access_token_hash, s.refresh_token_hash,
                s.access_expires_at, s.refresh_expires_at, s.created_at, s.last_used_at, u.email_normalized
         FROM user_sessions s
         INNER JOIN users u ON u.id = s.user_id
         WHERE s.access_token_hash = ?1 AND s.revoked_at IS NULL",
        params![access_hash],
        map_auth_session_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query session by access hash failed: {e}")))
}

fn get_session_by_refresh_hash(
    conn: &Connection,
    refresh_hash: &str,
) -> Result<Option<AuthSession>, AppError> {
    conn.query_row(
        "SELECT s.id, s.user_id, s.installation_id, s.access_token_hash, s.refresh_token_hash,
                s.access_expires_at, s.refresh_expires_at, s.created_at, s.last_used_at, u.email_normalized
         FROM user_sessions s
         INNER JOIN users u ON u.id = s.user_id
         WHERE s.refresh_token_hash = ?1 AND s.revoked_at IS NULL",
        params![refresh_hash],
        map_auth_session_row,
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query session by refresh hash failed: {e}")))
}

fn parse_scopes_json(value: String) -> Result<Vec<String>, rusqlite::Error> {
    serde_json::from_str::<Vec<String>>(&value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn user_api_token_status(record: UserApiTokenRecord) -> UserApiTokenStatus {
    UserApiTokenStatus {
        prefix: record.prefix,
        created_at: record.created_at,
        last_used_at: record.last_used_at,
        scopes: record.scopes,
    }
}

fn get_default_user_api_token(
    conn: &Connection,
    user_id: &str,
) -> Result<Option<UserApiTokenRecord>, AppError> {
    conn.query_row(
        "SELECT token_plaintext, token_prefix, scopes_json, created_at, last_used_at
         FROM user_api_tokens
         WHERE user_id = ?1 AND name = ?2 AND revoked_at IS NULL",
        params![user_id, USER_DEFAULT_API_TOKEN_NAME],
        |row| {
            Ok(UserApiTokenRecord {
                raw_token: row.get(0)?,
                prefix: row.get(1)?,
                scopes: parse_scopes_json(row.get(2)?)?,
                created_at: parse_dt_sql(&row.get::<_, String>(3)?)?,
                last_used_at: row
                    .get::<_, Option<String>>(4)?
                    .map(|value| parse_dt_sql(&value))
                    .transpose()?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query default api token failed: {e}")))
}

fn ensure_default_user_api_token(
    conn: &Connection,
    user_id: &str,
    now: DateTime<Utc>,
) -> Result<(Option<String>, UserApiTokenRecord), AppError> {
    if let Some(record) = get_default_user_api_token(conn, user_id)? {
        return Ok((None, record));
    }
    let (raw, record) = insert_default_user_api_token(conn, user_id, now)?;
    Ok((Some(raw), record))
}

fn reset_default_user_api_token(
    conn: &Connection,
    user_id: &str,
    now: DateTime<Utc>,
) -> Result<(String, UserApiTokenRecord), AppError> {
    conn.execute(
        "UPDATE user_api_tokens
         SET revoked_at = ?3, reset_at = ?3
         WHERE user_id = ?1 AND name = ?2 AND revoked_at IS NULL",
        params![user_id, USER_DEFAULT_API_TOKEN_NAME, now.to_rfc3339()],
    )
    .map_err(|e| AppError::Internal(format!("revoke default api token failed: {e}")))?;
    insert_default_user_api_token(conn, user_id, now)
}

fn insert_default_user_api_token(
    conn: &Connection,
    user_id: &str,
    now: DateTime<Utc>,
) -> Result<(String, UserApiTokenRecord), AppError> {
    let scopes = USER_DEFAULT_API_TOKEN_SCOPES
        .iter()
        .map(|scope| (*scope).to_string())
        .collect::<Vec<_>>();
    let scopes_json = serde_json::to_string(&scopes)
        .map_err(|e| AppError::Internal(format!("serialize api token scopes failed: {e}")))?;
    for _ in 0..5 {
        let raw = format!("ccrt_{}", generate_secret(48));
        let prefix = raw.chars().take(14).collect::<String>();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO user_api_tokens
              (id, user_id, name, token_hash, token_prefix, token_plaintext, scopes_json, created_at, last_used_at, reset_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL)",
            params![
                Uuid::new_v4().to_string(),
                user_id,
                USER_DEFAULT_API_TOKEN_NAME,
                hash_token(&raw),
                prefix.clone(),
                raw.clone(),
                scopes_json,
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| AppError::Internal(format!("insert default api token failed: {e}")))?;
        if inserted > 0 {
            return Ok((
                raw.clone(),
                UserApiTokenRecord {
                    raw_token: Some(raw),
                    prefix,
                    scopes,
                    created_at: now,
                    last_used_at: None,
                },
            ));
        }
    }
    Err(AppError::Internal(
        "failed to generate unique default api token".into(),
    ))
}

fn get_user_api_token_by_hash(
    conn: &Connection,
    token_hash: &str,
) -> Result<Option<(String, String, String, Vec<String>)>, AppError> {
    conn.query_row(
        "SELECT t.id, t.user_id, u.email_normalized, t.scopes_json
         FROM user_api_tokens t
         INNER JOIN users u ON u.id = t.user_id
         WHERE t.token_hash = ?1 AND t.revoked_at IS NULL AND u.status = 'active'",
        params![token_hash],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                parse_scopes_json(row.get(3)?)?,
            ))
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query user api token failed: {e}")))
}

fn get_installation_owner_email(
    conn: &Connection,
    installation_id: &str,
) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT owner_email
         FROM installations
         WHERE id = ?1
           AND owner_email IS NOT NULL
           AND owner_email != ''
         LIMIT 1",
        params![installation_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("query installation owner email failed: {e}")))
}

fn share_visible_to_email(share: &ShareDescriptor, viewer_email: Option<&str>) -> bool {
    let Some(viewer_email) = viewer_email else {
        return false;
    };
    share.owner_email.as_deref() == Some(viewer_email)
        || share
            .shared_with_emails
            .iter()
            .any(|email| email == viewer_email)
        || share.access_by_app.values().any(|access| {
            access
                .shared_with_emails
                .iter()
                .any(|email| email == viewer_email)
        })
}

fn can_manage_share(share: &ShareDescriptor, viewer_email: Option<&str>) -> bool {
    let Some(viewer_email) = viewer_email else {
        return false;
    };
    share.owner_email.as_deref() == Some(viewer_email)
}

fn share_acl_emails(share: &ShareDescriptor) -> Vec<String> {
    let mut emails = share.shared_with_emails.clone();
    for access in share.access_by_app.values() {
        emails.extend(access.shared_with_emails.iter().cloned());
    }
    emails.sort();
    emails.dedup();
    emails
}

fn share_supports_app(support: &ShareSupport, app: &str) -> bool {
    match app {
        "claude" => support.claude,
        "codex" => support.codex,
        "gemini" => support.gemini,
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_market_share_apps(
    support: &ShareSupport,
    app_settings: &BTreeMap<String, ShareAppSettings>,
    access_by_app: &BTreeMap<String, ShareAppAccess>,
    shared_with_emails: &[String],
    market_access_mode: &str,
    legacy_for_sale: &str,
    legacy_sale_market_kind: &str,
    expected_sale_market_kind: &str,
    allow_all_market_access: bool,
    market_email: &str,
) -> BTreeMap<String, MarketShareAppView> {
    ["claude", "codex", "gemini"]
        .into_iter()
        .map(|app| {
            let access = access_by_app.get(app);
            let setting = app_settings.get(app);
            let for_sale = setting
                .map(|value| value.for_sale.clone())
                .unwrap_or_else(|| legacy_for_sale.to_string());
            let sale_market_kind = setting
                .map(|value| value.sale_market_kind.clone())
                .unwrap_or_else(|| legacy_sale_market_kind.to_string());
            let app_market_access_mode = setting
                .map(|value| value.market_access_mode.clone())
                .or_else(|| access.map(|value| value.market_access_mode.clone()))
                .unwrap_or_else(|| market_access_mode.to_string());
            let visible = share_app_settings_visible_to_market(
                app,
                app_settings,
                access_by_app,
                shared_with_emails,
                market_access_mode,
                legacy_for_sale,
                legacy_sale_market_kind,
                expected_sale_market_kind,
                allow_all_market_access,
                market_email,
            );
            (
                app.to_string(),
                MarketShareAppView {
                    app: app.to_string(),
                    supported: share_supports_app(support, app),
                    visible,
                    for_sale,
                    sale_market_kind,
                    market_access_mode: app_market_access_mode,
                },
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn share_app_settings_visible_to_market(
    app: &str,
    app_settings: &BTreeMap<String, ShareAppSettings>,
    access_by_app: &BTreeMap<String, ShareAppAccess>,
    shared_with_emails: &[String],
    market_access_mode: &str,
    legacy_for_sale: &str,
    legacy_sale_market_kind: &str,
    expected_sale_market_kind: &str,
    allow_all_market_access: bool,
    market_email: &str,
) -> bool {
    let access = access_by_app.get(app);
    let setting = app_settings.get(app);
    let for_sale = setting
        .map(|value| value.for_sale.as_str())
        .unwrap_or(legacy_for_sale);
    let sale_market_kind = setting
        .map(|value| value.sale_market_kind.as_str())
        .unwrap_or(legacy_sale_market_kind);
    if for_sale != "Yes" || sale_market_kind != expected_sale_market_kind {
        return false;
    }
    let app_mode = setting
        .map(|value| value.market_access_mode.as_str())
        .or_else(|| access.map(|value| value.market_access_mode.as_str()))
        .unwrap_or(market_access_mode);
    let app_emails = setting
        .map(|value| value.shared_with_emails.as_slice())
        .or_else(|| access.map(|value| value.shared_with_emails.as_slice()))
        .unwrap_or(shared_with_emails);
    if allow_all_market_access && app_mode == "all" {
        return true;
    }
    app_emails
        .iter()
        .any(|email| email.eq_ignore_ascii_case(market_email))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::{ShareEditAckPayload, ShareSyncOperation};
    use crate::proxy::ProxyRegistry;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::path::PathBuf;

    #[test]
    fn resend_from_address_adds_default_display_name_for_plain_email() {
        let mut config = test_config("resend-from-default-name");
        config.resend_from = Some("noreply@jptokenswitch.cc".into());

        assert_eq!(
            resend_from_address(&config).expect("from address"),
            "TokenSwitch <noreply@jptokenswitch.cc>"
        );
    }

    #[test]
    fn resend_from_address_preserves_formatted_sender() {
        let mut config = test_config("resend-from-formatted");
        config.resend_from = Some("TokenSwitch <noreply@jptokenswitch.cc>".into());

        assert_eq!(
            resend_from_address(&config).expect("from address"),
            "TokenSwitch <noreply@jptokenswitch.cc>"
        );
    }

    #[test]
    fn resend_from_address_uses_configured_display_name() {
        let mut config = test_config("resend-from-custom-name");
        config.resend_from = Some("noreply@jptokenswitch.cc".into());
        config.resend_from_name = Some("TokenSwitch Router".into());

        assert_eq!(
            resend_from_address(&config).expect("from address"),
            "TokenSwitch Router <noreply@jptokenswitch.cc>"
        );
    }

    #[test]
    fn resend_from_address_strips_newlines() {
        let mut config = test_config("resend-from-newlines");
        config.resend_from = Some("TokenSwitch\r\n <noreply@jptokenswitch.cc>".into());

        assert_eq!(
            resend_from_address(&config).expect("from address"),
            "TokenSwitch <noreply@jptokenswitch.cc>"
        );
    }

    #[test]
    fn verification_code_email_block_is_translation_resistant() {
        let block = render_verification_code_block("784058");

        assert!(block.contains("role=\"presentation\""));
        assert!(block.contains("translate=\"no\""));
        assert!(block.contains("white-space:nowrap"));
        assert!(block.contains("784058"));
    }

    #[test]
    fn email_layout_sections_carry_their_own_width() {
        let html = render_email_layout(
            "Your verification code",
            "Use this code to finish signing in.",
            "<p>Body</p>",
        );

        assert!(html.contains("x-apple-disable-message-reformatting"));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.matches("max-width:560px").count() >= 3);
        assert!(html.contains("border-radius:18px 18px 0 0"));
        assert!(html.contains("border-radius:0 0 18px 18px"));
        assert!(html.contains("<span translate=\"no\">TokenSwitch</span> router notification"));
    }

    #[test]
    fn share_market_settings_patch_keeps_owner_email_as_market_delegate() {
        let owner = "router@jptokenswitch.cc";
        let patch = ShareSettingsPatch {
            sale_market_kind: Some("share".into()),
            market_access_mode: Some("selected".into()),
            shared_with_emails: Some(vec![owner.into()]),
            access_by_app: Some(BTreeMap::from([(
                "codex".into(),
                ShareAppAccess {
                    shared_with_emails: vec![owner.into()],
                    market_access_mode: "selected".into(),
                },
            )])),
            ..Default::default()
        };

        let normalized =
            normalize_share_settings_patch(patch, Some(owner), Some(&[]), None).expect("normalize");

        assert_eq!(normalized.sale_market_kind.as_deref(), Some("share"));
        assert_eq!(
            normalized.shared_with_emails.as_deref(),
            Some(&[owner.to_string()][..])
        );
        assert_eq!(
            normalized
                .access_by_app
                .as_ref()
                .and_then(|access| access.get("codex"))
                .map(|access| access.shared_with_emails.as_slice()),
            Some(&[owner.to_string()][..])
        );
    }

    #[test]
    fn existing_share_market_settings_patch_keeps_owner_email_as_market_delegate() {
        let owner = "router@jptokenswitch.cc";
        let patch = ShareSettingsPatch {
            shared_with_emails: Some(vec![owner.into()]),
            access_by_app: Some(BTreeMap::from([(
                "codex".into(),
                ShareAppAccess {
                    shared_with_emails: vec![owner.into()],
                    market_access_mode: "selected".into(),
                },
            )])),
            ..Default::default()
        };

        let normalized =
            normalize_share_settings_patch(patch, Some(owner), Some(&[]), Some("share"))
                .expect("normalize");

        assert_eq!(
            normalized.shared_with_emails.as_deref(),
            Some(&[owner.to_string()][..])
        );
        assert_eq!(
            normalized
                .access_by_app
                .as_ref()
                .and_then(|access| access.get("codex"))
                .map(|access| access.shared_with_emails.as_slice()),
            Some(&[owner.to_string()][..])
        );
    }

    fn test_config(name: &str) -> Config {
        let db_path =
            std::env::temp_dir().join(format!("cc-switch-router-{name}-{}.db", Uuid::new_v4()));
        Config {
            api_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8787),
            ssh_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 2222),
            tunnel_domain: "127.0.0.1:8787".into(),
            ssh_public_addr: String::new(),
            use_localhost: true,
            lease_ttl_secs: 60,
            db_path,
            host_key_path: std::env::temp_dir()
                .join(format!("cc-switch-router-{name}-{}.key", Uuid::new_v4())),
            cleanup_interval_secs: 300,
            lease_retention_secs: 7 * 24 * 60 * 60,
            client_stale_secs: 60 * 60,
            paused_share_stale_secs: 60 * 60,
            resend_api_key: None,
            resend_from: None,
            resend_from_name: None,
            resend_reply_to: None,
            auth_code_ttl_secs: 600,
            auth_code_cooldown_secs: 60,
            auth_session_ttl_secs: 7 * 24 * 60 * 60,
            auth_refresh_ttl_secs: 30 * 24 * 60 * 60,
            auth_max_verify_attempts: 8,
            auth_email_hourly_limit: 10,
            auth_ip_hourly_limit: 30,
            auth_installation_hourly_limit: 15,
            ip_blacklist: String::new(),
            free_share_ip_parallel_limit: 1,
            verification_service_base_url: "https://tokenswitch.org".into(),
            verification_service_api_key: None,
            admin_emails: HashSet::new(),
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_topic_id: None,
            telegram_notify_all: false,
            telegram_notify_admin: false,
            board_max_len: 1000,
            board_guest_per_hour: 5,
            board_user_per_hour: 30,
            board_pin_limit: 3,
            board_guest_self_delete_secs: 300,
            ux_telemetry_enabled: false,
            ux_telemetry_retention_days: 7,
            metrics: crate::config::MetricsConfig {
                enabled: true,
                db_path: std::env::temp_dir().join(format!(
                    "cc-switch-router-{name}-{}-metrics.db",
                    Uuid::new_v4()
                )),
                retention_days: 7,
                sample_interval_secs: 5,
            },
        }
    }

    async fn setup_store(name: &str) -> (AppStore, Config) {
        let config = test_config(name);
        let store = AppStore::new(&config).expect("create store");
        (store, config)
    }

    fn test_market() -> MarketRegistryRecord {
        MarketRegistryRecord {
            id: "main-market".into(),
            display_name: "https://market-a.example.com".into(),
            email: "market@example.com".into(),
            subdomain: "market-a".into(),
            public_base_url: "https://market-a.example.com".into(),
            market_kind: "usage".into(),
            scopes: MARKET_DEFAULT_SCOPES
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
            status: "active".into(),
            maintenance_enabled: false,
            maintenance_message: None,
        }
    }

    async fn insert_market(store: &AppStore, market: &MarketRegistryRecord) {
        let conn = store.conn.lock().await;
        let now = Utc::now().to_rfc3339();
        let scopes_json = serde_json::to_string(&market.scopes).expect("scopes json");
        conn.execute(
            "INSERT INTO router_markets (
                id, display_name, email, subdomain, public_base_url, market_kind, scopes_json,
                status, listed, created_at, updated_at, last_seen_at, offline_since
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9, ?9, NULL)
             ON CONFLICT(email) DO UPDATE SET
                display_name = excluded.display_name,
                subdomain = excluded.subdomain,
                public_base_url = excluded.public_base_url,
                market_kind = excluded.market_kind,
                scopes_json = excluded.scopes_json,
                status = excluded.status,
                updated_at = excluded.updated_at,
                last_seen_at = excluded.last_seen_at",
            params![
                market.id,
                market.display_name,
                market.email,
                market.subdomain,
                market.public_base_url,
                market.market_kind,
                scopes_json,
                market.status,
                now,
            ],
        )
        .expect("insert market");
    }

    async fn insert_installation(store: &AppStore, installation_id: &str) {
        let now = Utc::now().to_rfc3339();
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, owner_email, owner_verified_at, created_at, last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                installation_id,
                format!("pk-{installation_id}"),
                "macOS",
                "1.0.0",
                "owner@example.com",
                now,
                now,
                now,
            ],
        )
        .expect("insert installation");
    }

    async fn set_installation_country_code(
        store: &AppStore,
        installation_id: &str,
        country_code: &str,
    ) {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE installations SET country_code = ?2 WHERE id = ?1",
            params![installation_id, country_code],
        )
        .expect("update installation country_code");
    }

    async fn mark_installation_last_seen(
        store: &AppStore,
        installation_id: &str,
        value: DateTime<Utc>,
    ) {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE installations SET last_seen_at = ?2 WHERE id = ?1",
            params![installation_id, value.to_rfc3339()],
        )
        .expect("update installation last_seen_at");
    }

    async fn insert_client_tunnel(
        store: &AppStore,
        installation_id: &str,
        owner_email: &str,
        subdomain: &str,
    ) {
        let now = Utc::now().to_rfc3339();
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO installation_client_tunnels (
                installation_id, owner_email, subdomain, enabled, created_at, updated_at, last_seen_at
             ) VALUES (?1, ?2, ?3, 1, ?4, ?4, ?4)",
            params![installation_id, owner_email, subdomain, now],
        )
        .expect("insert client tunnel");
    }

    async fn insert_share(
        store: &AppStore,
        installation_id: &str,
        share_id: &str,
        subdomain: &str,
        share_status: &str,
    ) {
        // Default to all 3 apps bound so existing tests (which were written
        // before the per-binding model_health filter landed) keep getting
        // their snapshot intakes accepted. Tests that exercise the filter
        // call `set_share_bindings` afterwards to narrow this.
        let default_bindings = serde_json::json!({
            "claude": "provider-test",
            "codex": "provider-test",
            "gemini": "provider-test",
        })
        .to_string();
        let now = Utc::now();
        let expires = now + Duration::hours(1);
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO shares (
                share_id, installation_id, share_name, owner_email, shared_with_emails_json,
                description, for_sale, subdomain, app_type, provider_id,
                enabled_claude, enabled_codex, enabled_gemini, token_limit, parallel_limit,
                tokens_used, requests_count, share_status, created_at, expires_at, bindings_json, updated_at
             ) VALUES (?1, ?2, ?3, ?4, '[]', NULL, 'No', ?5, 'proxy', NULL, 1, 1, 1, 1000, 3, 0, 0, ?6, ?7, ?8, ?9, ?7)",
            params![
                share_id,
                installation_id,
                format!("share-{share_id}"),
                "owner@example.com",
                subdomain,
                share_status,
                now.to_rfc3339(),
                expires.to_rfc3339(),
                default_bindings,
            ],
        )
        .expect("insert share");
    }

    /// Override the bindings_json on an existing test share. Use for tests
    /// that need a specific subset of apps bound (e.g. share with only
    /// claude bound, to verify the model_health filter drops codex entries).
    async fn set_share_bindings(store: &AppStore, share_id: &str, apps: &[&str]) {
        let map: serde_json::Map<String, serde_json::Value> = apps
            .iter()
            .map(|app| {
                (
                    app.to_string(),
                    serde_json::Value::String("provider-test".into()),
                )
            })
            .collect();
        let bindings = serde_json::Value::Object(map).to_string();
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE shares SET bindings_json = ?2 WHERE share_id = ?1",
            params![share_id, bindings],
        )
        .expect("update share bindings_json");
    }

    async fn set_share_shared_with_emails(store: &AppStore, share_id: &str, emails: &[&str]) {
        let conn = store.conn.lock().await;
        let emails = emails
            .iter()
            .map(|email| email.to_string())
            .collect::<Vec<_>>();
        conn.execute(
            "UPDATE shares SET shared_with_emails_json = ?2 WHERE share_id = ?1",
            params![
                share_id,
                serde_json::to_string(&emails).expect("serialize shared emails")
            ],
        )
        .expect("update share shared emails");
    }

    #[tokio::test]
    async fn dashboard_market_linked_share_includes_runtime_states() {
        let (store, config) = setup_store("market-runtime-states").await;
        let market = test_market();
        insert_market(&store, &market).await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale='Yes', market_access_mode='all' WHERE share_id='share-1'",
                [],
            )
            .expect("enable market share");
        }

        let expires_at = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        let synced = store
            .sync_market_share_runtime_states(
                &market.email,
                false,
                vec![MarketShareRuntimeStateInput {
                    share_id: "share-1".into(),
                    router_id: Some("main".into()),
                    scope: "model".into(),
                    kind: "model_block".into(),
                    app_type: Some("codex".into()),
                    model_id: Some("model-1".into()),
                    model_name: Some("gpt-5.5*".into()),
                    reason_kind: Some("model_unsupported".into()),
                    reason: Some("model is not supported".into()),
                    failure_count: None,
                    expires_at: Some(expires_at),
                }],
            )
            .await
            .expect("sync runtime state");
        assert_eq!(synced, 1);

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        proxy
            .set_route(
                "share-sub".into(),
                "127.0.0.1:1234".into(),
                None,
                None,
                None,
                false,
                -1,
                None,
            )
            .await;
        let dashboard = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard");
        let linked_share = dashboard.markets[0]
            .linked_shares
            .iter()
            .find(|share| share.share_id == "share-1")
            .expect("linked share");
        assert_eq!(linked_share.market_states.len(), 1);
        assert_eq!(linked_share.market_states[0].kind, "model_block");
        assert_eq!(
            linked_share.market_states[0].model_name.as_deref(),
            Some("gpt-5.5*")
        );

        store
            .sync_market_share_runtime_states(&market.email, true, Vec::new())
            .await
            .expect("replace clear runtime states");
        let dashboard = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard after clear");
        let linked_share = dashboard.markets[0]
            .linked_shares
            .iter()
            .find(|share| share.share_id == "share-1")
            .expect("linked share after clear");
        assert!(linked_share.market_states.is_empty());

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
        let _ = std::fs::remove_file(PathBuf::from(config.metrics.db_path));
    }

    #[tokio::test]
    async fn default_user_api_token_is_unique_resolvable_and_resettable() {
        let (store, config) = setup_store("user-api-token-lifecycle").await;
        let now = Utc::now();
        let user_id = {
            let conn = store.conn.lock().await;
            let user = upsert_user_by_email(&conn, "owner@example.com", now).expect("upsert user");
            let (first_raw, first_record) =
                ensure_default_user_api_token(&conn, &user.id, now).expect("create api token");
            let first_raw = first_raw.expect("first creation returns raw token");
            assert!(first_raw.starts_with("ccrt_"));
            assert_eq!(first_record.raw_token.as_deref(), Some(first_raw.as_str()));
            assert_eq!(
                first_record.prefix,
                first_raw.chars().take(14).collect::<String>()
            );
            assert_eq!(
                first_record.scopes,
                vec![
                    "share:read".to_string(),
                    "share:write".to_string(),
                    "share:invoke".to_string()
                ]
            );

            let (second_raw, second_record) =
                ensure_default_user_api_token(&conn, &user.id, now).expect("reuse api token");
            assert!(second_raw.is_none());
            assert_eq!(second_record.raw_token.as_deref(), Some(first_raw.as_str()));
            assert_eq!(second_record.prefix, first_record.prefix);

            (user.id, first_raw)
        };
        let old_token = user_id.1;
        let user_id = user_id.0;

        let principal = store
            .resolve_user_api_token(&old_token, "share:invoke")
            .await
            .expect("resolve old token")
            .expect("old token principal");
        assert_eq!(principal.user_id, user_id);
        assert_eq!(principal.email, "owner@example.com");

        let forbidden_scope = store
            .resolve_user_api_token(&old_token, "admin:write")
            .await;
        assert!(matches!(forbidden_scope, Err(AppError::Unauthorized(_))));

        let reset = store
            .reset_default_api_token("Owner@Example.com")
            .await
            .expect("reset default api token");
        assert!(reset.api_token.starts_with("ccrt_"));
        assert_ne!(reset.api_token, old_token);
        assert_eq!(
            reset.token.prefix,
            reset.api_token.chars().take(14).collect::<String>()
        );
        let current = store
            .get_default_api_token("Owner@Example.com")
            .await
            .expect("get default api token");
        assert_eq!(current.api_token.as_deref(), Some(reset.api_token.as_str()));

        assert!(
            store
                .resolve_user_api_token(&old_token, "share:invoke")
                .await
                .expect("old token lookup after reset")
                .is_none()
        );
        assert_eq!(
            store
                .resolve_user_api_token(&reset.api_token, "share:invoke")
                .await
                .expect("new token lookup")
                .expect("new token principal")
                .email,
            "owner@example.com"
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn user_api_token_share_invoke_acl_allows_owner_and_shared_email() {
        let (store, config) = setup_store("user-api-token-share-acl").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-acl", "acl-sub", "active").await;
        set_share_shared_with_emails(&store, "share-acl", &["Shared@Example.com"]).await;

        assert!(
            store
                .user_can_invoke_share("OWNER@example.com", "share-acl", None)
                .await
                .expect("owner acl")
        );
        assert!(
            store
                .user_can_invoke_share("shared@example.com", "share-acl", None)
                .await
                .expect("shared acl")
        );
        assert!(
            !store
                .user_can_invoke_share("other@example.com", "share-acl", None)
                .await
                .expect("other acl")
        );
        assert!(
            !store
                .user_can_invoke_share("owner@example.com", "missing-share", None)
                .await
                .expect("missing share acl")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn user_api_token_share_invoke_acl_is_app_scoped_when_available() {
        let (store, config) = setup_store("user-api-token-share-app-acl").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-app-acl", "app-acl-sub", "active").await;
        set_share_shared_with_emails(&store, "share-app-acl", &["legacy@example.com"]).await;
        {
            let conn = store.conn.lock().await;
            let access_by_app = BTreeMap::from([
                (
                    "claude".to_string(),
                    ShareAppAccess {
                        shared_with_emails: vec!["claude@example.com".to_string()],
                        market_access_mode: "selected".to_string(),
                    },
                ),
                (
                    "codex".to_string(),
                    ShareAppAccess {
                        shared_with_emails: vec!["codex@example.com".to_string()],
                        market_access_mode: "selected".to_string(),
                    },
                ),
            ]);
            conn.execute(
                "UPDATE shares SET access_by_app_json = ?2 WHERE share_id = ?1",
                params![
                    "share-app-acl",
                    serde_json::to_string(&access_by_app).expect("serialize app acl")
                ],
            )
            .expect("set app acl");
        }

        assert!(
            store
                .user_can_invoke_share("claude@example.com", "share-app-acl", Some("claude"))
                .await
                .expect("claude app acl")
        );
        assert!(
            !store
                .user_can_invoke_share("claude@example.com", "share-app-acl", Some("codex"))
                .await
                .expect("claude denied on codex")
        );
        assert!(
            store
                .user_can_invoke_share("legacy@example.com", "share-app-acl", None)
                .await
                .expect("legacy fallback acl")
        );
        assert!(
            !store
                .user_can_invoke_share("legacy@example.com", "share-app-acl", Some("claude"))
                .await
                .expect("legacy denied with app acl")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn user_api_token_share_invoke_acl_allows_any_user_for_free_share() {
        let (store, config) = setup_store("user-api-token-free-share-acl").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-free", "free-sub", "active").await;

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Free' WHERE share_id = 'share-free'",
                [],
            )
            .expect("mark share free");
        }

        assert!(
            store
                .user_can_invoke_share("other@example.com", "share-free", None)
                .await
                .expect("free share acl")
        );

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'No' WHERE share_id = 'share-free'",
                [],
            )
            .expect("mark share private");
        }

        assert!(
            !store
                .user_can_invoke_share("other@example.com", "share-free", None)
                .await
                .expect("private share acl")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    async fn insert_health_check(
        store: &AppStore,
        share_id: &str,
        checked_at: i64,
        is_healthy: bool,
    ) {
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO share_health_checks (share_id, checked_at, is_healthy) VALUES (?1, ?2, ?3)",
            params![share_id, checked_at, if is_healthy { 1 } else { 0 }],
        )
        .expect("insert health check");
    }

    async fn insert_installation_health_check(
        store: &AppStore,
        installation_id: &str,
        checked_at: i64,
        is_healthy: bool,
    ) {
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO installation_health_checks (installation_id, checked_at, is_healthy) VALUES (?1, ?2, ?3)",
            params![
                installation_id,
                checked_at,
                if is_healthy { 1 } else { 0 }
            ],
        )
        .expect("insert installation health check");
    }

    async fn insert_model_health_check(
        store: &AppStore,
        share_id: &str,
        checked_at: i64,
        status: &str,
    ) {
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO share_model_health_checks (
                request_id, share_id, subdomain, app_type, requested_model, actual_model,
                status, status_code, latency_ms, first_token_ms, error_message, checked_at, source
             ) VALUES (?1, ?2, 'share-sub', 'codex', 'codex', 'gpt-5.5', ?3, 200, 100, NULL, NULL, ?4, 'test')",
            params![format!("health-{share_id}-{checked_at}"), share_id, status, checked_at],
        )
        .expect("insert model health check");
    }

    fn test_upstream_provider(model: &str) -> ShareUpstreamProvider {
        ShareUpstreamProvider {
            kind: "test".into(),
            app: "codex".into(),
            provider_name: Some("test".into()),
            provider_type: None,
            for_sale_official_price_percent: None,
            account_email: None,
            api_url: None,
            quota: None,
            models: vec![ShareUpstreamModel {
                slot: "default".into(),
                actual_model: model.into(),
            }],
            ..Default::default()
        }
    }

    fn test_model_summary(model: &str, recent_results: &[&str]) -> ModelHealthSummary {
        ModelHealthSummary {
            app_type: "codex".into(),
            requested_model: model.into(),
            actual_model: model.into(),
            status: recent_results.first().copied().unwrap_or("success").into(),
            recent_results: recent_results
                .iter()
                .map(|value| value.to_string())
                .collect(),
            last_checked_at: Some(Utc::now().timestamp()),
            last_success_at: None,
            last_failed_at: None,
            error_message: None,
            status_code: None,
            latency_ms: 0,
            source: None,
            provider_id: None,
            provider_name: None,
        }
    }

    fn test_health_check(
        share_id: &str,
        model: &str,
        status: &str,
        checked_at: i64,
    ) -> ShareModelHealthCheckEntry {
        ShareModelHealthCheckEntry {
            request_id: format!("health-{share_id}-{model}-{checked_at}"),
            share_id: share_id.into(),
            subdomain: "share-sub".into(),
            app_type: "codex".into(),
            requested_model: model.into(),
            actual_model: model.into(),
            status: status.into(),
            status_code: Some(if status == "success" { 200 } else { 500 }),
            latency_ms: 100,
            first_token_ms: None,
            error_message: None,
            checked_at,
            source: "test".into(),
        }
    }

    fn test_share_descriptor(share_id: &str, subdomain: &str) -> ShareDescriptor {
        ShareDescriptor {
            share_id: share_id.into(),
            share_name: format!("share-{share_id}"),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            market_access_mode: "selected".into(),
            access_by_app: Default::default(),
            app_settings: Default::default(),
            for_sale_official_price_percent_by_app: Default::default(),
            description: None,
            for_sale: "No".into(),
            sale_market_kind: "token".into(),
            subdomain: subdomain.into(),
            app_type: "codex".into(),
            provider_id: Some("provider-test".into()),
            bindings: BTreeMap::from([("codex".into(), "provider-test".into())]),
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "active".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport {
                claude: false,
                codex: true,
                gemini: false,
            },
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            app_providers: ShareAppProviders::default(),
            market_grant: None,
            app_availability: ShareAppAvailability::default(),
            model_health: ShareModelHealthSummary::default(),
        }
    }

    #[test]
    fn self_reported_share_owner_must_match_installation_owner() {
        let mut share = test_share_descriptor("owner-mismatch", "owner-mismatch-sub");
        share.owner_email = Some("other@example.com".into());
        let error = normalize_self_reported_share_owner(&mut share, "owner@example.com")
            .expect_err("mismatched owner must be rejected");
        assert!(
            error
                .to_string()
                .contains("share owner must match the installation owner")
        );
    }

    #[test]
    fn share_market_dashboard_status_aggregates_hosted_share_runtime() {
        let market_email = "share-market@example.com";
        let mut share = test_share_descriptor("share-1", "share-sub");
        share.for_sale = "Yes".into();
        share.sale_market_kind = "share".into();
        share.shared_with_emails = vec![market_email.into()];
        share.parallel_limit = 9;
        share.tokens_used = 987_654;

        let shares = vec![("inst-1".into(), share)];
        let active_subdomains = HashSet::from(["share-sub".to_string()]);
        let inflight_by_share = HashMap::from([("share-1".to_string(), 4_usize)]);
        let inflight_by_market_email = HashMap::from([(market_email.to_string(), 99_usize)]);
        let mut market = DashboardMarketView {
            id: "market-1".into(),
            display_name: "Share Market".into(),
            email: market_email.into(),
            subdomain: "share-market".into(),
            public_base_url: "https://share-market.example.com".into(),
            market_kind: "share".into(),
            status: "active".into(),
            online: true,
            can_manage: false,
            maintenance_enabled: false,
            maintenance_message: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
            last_seen_at: Utc::now().to_rfc3339(),
            offline_since: None,
            share_count: 0,
            online_share_count: 0,
            active_requests: 0,
            parallel_capacity: 0,
            online_minutes_24h: 0,
            online_rate_24h: 0.0,
            usage_tokens: 0,
            usage_amount_usd: "12.34000000".into(),
            pricing_summary: None,
            health_checks: Vec::new(),
            health_timeline: Vec::new(),
            linked_shares: Vec::new(),
            recent_requests: Vec::new(),
            operational_summary: OperationalSummary::healthy("available"),
        };

        enrich_dashboard_market(
            &mut market,
            &shares,
            &HashMap::new(),
            &active_subdomains,
            &inflight_by_share,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &inflight_by_market_email,
            &HashMap::new(),
            &HashMap::new(),
            Utc::now().timestamp(),
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(market.share_count, 1);
        assert_eq!(market.online_share_count, 1);
        assert_eq!(market.active_requests, 4);
        assert_eq!(market.parallel_capacity, 9);
        assert_eq!(market.usage_tokens, 987_654);
        assert_eq!(market.usage_amount_usd, "0.00000000");
        assert_eq!(market.online_minutes_24h, ONLINE_WINDOW_MINUTES);
        assert_eq!(market.online_rate_24h, 100.0);
        assert!(
            market
                .health_checks
                .last()
                .map(|entry| entry.is_healthy)
                .unwrap_or(false)
        );
        assert_eq!(market_operational_summary(&market).state, "available");
        market.active_requests = market.parallel_capacity as usize;
        let full = market_operational_summary(&market);
        assert_eq!(full.state, "degraded");
        assert_eq!(
            full.primary_reason
                .as_ref()
                .map(|reason| reason.code.as_str()),
            Some("parallel_capacity_full")
        );
    }

    #[test]
    fn share_market_dashboard_health_uses_market_online_fallback() {
        let market_email = "share-market@example.com";
        let mut market = DashboardMarketView {
            id: "market-1".into(),
            display_name: "Share Market".into(),
            email: market_email.into(),
            subdomain: "share-market".into(),
            public_base_url: "https://share-market.example.com".into(),
            market_kind: "share".into(),
            status: "active".into(),
            online: true,
            can_manage: false,
            maintenance_enabled: false,
            maintenance_message: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
            last_seen_at: Utc::now().to_rfc3339(),
            offline_since: None,
            share_count: 0,
            online_share_count: 0,
            active_requests: 0,
            parallel_capacity: 0,
            online_minutes_24h: 0,
            online_rate_24h: 0.0,
            usage_tokens: 0,
            usage_amount_usd: "0.00000000".into(),
            pricing_summary: None,
            health_checks: Vec::new(),
            health_timeline: Vec::new(),
            linked_shares: Vec::new(),
            recent_requests: Vec::new(),
            operational_summary: OperationalSummary::healthy("available"),
        };

        enrich_dashboard_market(
            &mut market,
            &[],
            &HashMap::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            Utc::now().timestamp(),
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(market.share_count, 0);
        assert_eq!(market.online_share_count, 0);
        assert_eq!(market.online_minutes_24h, ONLINE_WINDOW_MINUTES);
        assert_eq!(market.online_rate_24h, 100.0);
        assert_eq!(market.health_checks.len(), 10);
        assert!(
            market.health_checks.iter().all(|entry| entry.is_healthy),
            "online share market should show recent healthy dashboard dots even before shares are linked"
        );
    }

    fn test_market_request_log(request_id: &str, share_id: &str) -> MarketRequestLogEntry {
        MarketRequestLogEntry {
            request_id: request_id.into(),
            user_email: Some("user@example.com".into()),
            api_key_prefix: Some("sk-test".into()),
            router_id: Some("router-test".into()),
            share_id: Some(share_id.into()),
            share_subdomain: Some("old-sub".into()),
            model: Some("gpt-5".into()),
            request_agent: "codex".into(),
            requested_model: "gpt-5".into(),
            actual_model: "gpt-5".into(),
            actual_model_source: "official".into(),
            status: "settled".into(),
            status_code: Some(200),
            error_message: None,
            latency_ms: Some(100),
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            usage_amount_usd: Some("0.001".into()),
            created_at: Utc::now().to_rfc3339(),
            settled_at: Some(Utc::now().to_rfc3339()),
            user_country: None,
            user_country_iso3: None,
        }
    }

    fn test_failed_market_request_log(request_id: &str, share_id: &str) -> MarketRequestLogEntry {
        let mut log = test_market_request_log(request_id, share_id);
        log.status = "failed".into();
        log.status_code = Some(500);
        log.usage_amount_usd = None;
        log
    }

    fn test_claude_market_request_log(
        request_id: &str,
        share_id: &str,
        requested_model: &str,
    ) -> MarketRequestLogEntry {
        let mut log = test_market_request_log(request_id, share_id);
        log.model = Some(requested_model.into());
        log.request_agent = "claude".into();
        log.requested_model = requested_model.into();
        log.actual_model = requested_model.into();
        log
    }

    fn test_failed_claude_market_request_log(
        request_id: &str,
        share_id: &str,
        requested_model: &str,
    ) -> MarketRequestLogEntry {
        let mut log = test_claude_market_request_log(request_id, share_id, requested_model);
        log.status = "failed_released".into();
        log.status_code = Some(500);
        log.error_message = Some("upstream failed".into());
        log.usage_amount_usd = None;
        log
    }

    #[tokio::test]
    async fn market_request_failure_is_market_local_and_not_global_health() {
        let (store, config) = setup_store("market-failure-local-only").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', market_access_mode = 'all' WHERE share_id = 'share-1'",
                [],
            )
            .expect("make share market visible");
        }
        let market = test_market();

        store
            .batch_sync_market_request_logs(
                &market,
                MarketRequestLogBatchSyncRequest {
                    logs: vec![test_failed_market_request_log(
                        "req_market_fail_1",
                        "share-1",
                    )],
                },
            )
            .await
            .expect("sync market failure");

        let conn = store.conn.lock().await;
        let global_health_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM share_model_health_state", [], |row| {
                row.get(0)
            })
            .expect("count global health");
        let market_state_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_share_model_failure_state WHERE lower(market_email) = 'market@example.com'",
                [],
                |row| row.get(0),
            )
            .expect("count market state");
        drop(conn);

        assert_eq!(global_health_count, 0);
        assert_eq!(market_state_count, 1);

        let active = HashSet::from(["share-sub".to_string()]);
        let shares = store
            .list_market_shares(&market.email, "main", &active, &HashMap::new(), true)
            .await
            .expect("list market shares");
        assert_eq!(shares.len(), 1);
        assert!((shares[0].signals.owner_penalty - 0.7).abs() < 1e-9);

        let other_shares = store
            .list_market_shares("other@example.com", "main", &active, &HashMap::new(), true)
            .await
            .expect("list other market shares");
        assert_eq!(other_shares.len(), 1);
        assert!((other_shares[0].signals.owner_penalty - 1.0).abs() < 1e-9);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn market_dashboard_marks_only_market_unavailable_app_red() {
        let (store, config) = setup_store("market-dashboard-app-unavailable").await;
        let market = test_market();
        insert_market(&store, &market).await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', market_access_mode = 'all' WHERE share_id = 'share-1'",
                [],
            )
            .expect("make share market visible");
        }

        store
            .batch_sync_market_request_logs(
                &market,
                MarketRequestLogBatchSyncRequest {
                    logs: vec![
                        test_failed_market_request_log("req_market_fail_1", "share-1"),
                        test_failed_market_request_log("req_market_fail_2", "share-1"),
                        test_failed_market_request_log("req_market_fail_3", "share-1"),
                    ],
                },
            )
            .await
            .expect("sync market failures");

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        proxy
            .set_route(
                "share-sub".into(),
                "127.0.0.1:1234".into(),
                None,
                None,
                None,
                false,
                -1,
                None,
            )
            .await;
        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");
        let market_view = snapshot
            .markets
            .iter()
            .find(|item| item.email == market.email)
            .expect("market view");
        let share = market_view
            .linked_shares
            .iter()
            .find(|item| item.share_id == "share-1")
            .expect("linked share");

        assert_eq!(
            share
                .app_availability
                .codex
                .as_ref()
                .map(|entry| entry.status.as_str()),
            Some("unavailable")
        );
        assert!(share.app_availability.claude.is_none());
        assert!(share.app_availability.gemini.is_none());
        assert_eq!(market_view.health_timeline.len(), HEALTH_TIMELINE_BUCKETS);
        let latest = market_view
            .health_timeline
            .last()
            .expect("latest market health bucket");
        assert_eq!(latest.status, "offline");
        assert_eq!(latest.request_count, 3);
        assert_eq!(latest.failure_count, 3);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn market_dashboard_health_dots_or_merge_linked_share_minutes() {
        let (store, config) = setup_store("market-health-dots-or-merge").await;
        let market = test_market();
        insert_market(&store, &market).await;
        insert_installation(&store, "inst-1").await;
        insert_share(
            &store,
            "inst-1",
            "share-failing",
            "share-failing-sub",
            "active",
        )
        .await;
        insert_share(
            &store,
            "inst-1",
            "share-healthy",
            "share-healthy-sub",
            "active",
        )
        .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', market_access_mode = 'all'
                 WHERE share_id IN ('share-failing', 'share-healthy')",
                [],
            )
            .expect("make shares market visible");
        }

        let now_minute = Utc::now().timestamp().div_euclid(60) * 60;
        for offset in (0..10).rev() {
            let checked_at = now_minute - offset * 60;
            insert_health_check(&store, "share-failing", checked_at, false).await;
            insert_health_check(&store, "share-healthy", checked_at + 1, true).await;
        }

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");
        let market_view = snapshot
            .markets
            .iter()
            .find(|item| item.email == market.email)
            .expect("market view");

        assert_eq!(market_view.health_checks.len(), 10);
        assert!(
            market_view
                .health_checks
                .iter()
                .all(|entry| entry.is_healthy),
            "any healthy linked share in a minute should make the market dot healthy"
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn market_prompt_too_long_does_not_mark_claude_unavailable() {
        let (store, config) = setup_store("market-prompt-too-long-ignored").await;
        let market = test_market();
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;

        let mut log = test_failed_claude_market_request_log(
            "req_market_prompt_too_long",
            "share-1",
            "claude-sonnet-4-6",
        );
        log.status_code = Some(400);
        log.error_message = Some("prompt token count of 128078 exceeds the limit of 128000".into());
        store
            .batch_sync_market_request_logs(
                &market,
                MarketRequestLogBatchSyncRequest { logs: vec![log] },
            )
            .await
            .expect("sync prompt-too-long failure");

        let conn = store.conn.lock().await;
        let market_state_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_share_model_failure_state WHERE lower(market_email) = 'market@example.com'",
                [],
                |row| row.get(0),
            )
            .expect("count market state");
        drop(conn);

        assert_eq!(market_state_count, 0);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn market_rate_limit_marks_claude_degraded_not_unavailable() {
        let (store, config) = setup_store("market-rate-limit-degraded").await;
        let market = test_market();
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', market_access_mode = 'all' WHERE share_id = 'share-1'",
                [],
            )
            .expect("make share market visible");
        }

        let logs = (1..=3)
            .map(|idx| {
                let mut log = test_failed_claude_market_request_log(
                    &format!("req_market_rate_limit_{idx}"),
                    "share-1",
                    "claude-sonnet-4-6",
                );
                log.status_code = Some(429);
                log.error_message =
                    Some("Usage credits are required for long context requests.".into());
                log
            })
            .collect();
        store
            .batch_sync_market_request_logs(&market, MarketRequestLogBatchSyncRequest { logs })
            .await
            .expect("sync rate limits");

        let active = HashSet::from(["share-sub".to_string()]);
        let shares = store
            .list_market_shares(&market.email, "main", &active, &HashMap::new(), true)
            .await
            .expect("list market shares");
        assert_eq!(shares.len(), 1);
        assert_eq!(
            shares[0]
                .app_availability
                .claude
                .as_ref()
                .map(|entry| entry.status.as_str()),
            Some("degraded")
        );
        assert!((shares[0].signals.owner_penalty - 0.7).abs() < 1e-9);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn market_claude_alias_success_softens_prior_failures() {
        let (store, config) = setup_store("market-claude-alias-success").await;
        let market = test_market();
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', market_access_mode = 'all' WHERE share_id = 'share-1'",
                [],
            )
            .expect("make share market visible");
        }

        store
            .batch_sync_market_request_logs(
                &market,
                MarketRequestLogBatchSyncRequest {
                    logs: vec![
                        test_failed_claude_market_request_log(
                            "req_market_alias_fail_1",
                            "share-1",
                            "claude-sonnet-4-6",
                        ),
                        test_failed_claude_market_request_log(
                            "req_market_alias_fail_2",
                            "share-1",
                            "claude-sonnet-4-6",
                        ),
                        test_failed_claude_market_request_log(
                            "req_market_alias_fail_3",
                            "share-1",
                            "claude-sonnet-4-6",
                        ),
                        test_claude_market_request_log(
                            "req_market_alias_success",
                            "share-1",
                            "claude-sonnet-4.6",
                        ),
                    ],
                },
            )
            .await
            .expect("sync alias failures and success");

        let conn = store.conn.lock().await;
        let stored_model: String = conn
            .query_row(
                "SELECT requested_model FROM market_share_model_failure_state WHERE lower(market_email) = 'market@example.com'",
                [],
                |row| row.get(0),
            )
            .expect("read stored model");
        drop(conn);
        assert_eq!(stored_model, "claude-sonnet-4.6");

        let active = HashSet::from(["share-sub".to_string()]);
        let shares = store
            .list_market_shares(&market.email, "main", &active, &HashMap::new(), true)
            .await
            .expect("list market shares");
        assert_ne!(
            shares[0]
                .app_availability
                .claude
                .as_ref()
                .map(|entry| entry.status.as_str()),
            Some("unavailable")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn stale_market_failures_do_not_keep_claude_red() {
        let (store, config) = setup_store("market-stale-failures-expire").await;
        let market = test_market();
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', market_access_mode = 'all' WHERE share_id = 'share-1'",
                [],
            )
            .expect("make share market visible");
            let stale_checked_at =
                Utc::now().timestamp() - MARKET_APP_AVAILABILITY_FAILURE_TTL_SECS - 1;
            conn.execute(
                "INSERT INTO market_share_model_failure_state (
                    market_email, share_id, app_type, requested_model, actual_model, last_status,
                    last_failed_at, last_checked_at, recent_results_json, error_message, updated_at
                 ) VALUES (?1, 'share-1', 'claude', 'claude-sonnet-4.6', 'claude-sonnet-4.6',
                    'failed', ?2, ?2, '[\"failed\",\"failed\",\"failed\"]', 'stale failure', ?2)",
                params![market.email.to_ascii_lowercase(), stale_checked_at],
            )
            .expect("insert stale market failure");
        }

        let active = HashSet::from(["share-sub".to_string()]);
        let shares = store
            .list_market_shares(&market.email, "main", &active, &HashMap::new(), true)
            .await
            .expect("list market shares");

        assert!(shares[0].app_availability.claude.is_none());
        assert!((shares[0].signals.owner_penalty - 1.0).abs() < 1e-9);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn market_share_quota_block_overrides_stale_success_availability() {
        let (store, config) = setup_store("market-quota-block-availability").await;
        let market = test_market();
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        let now = Utc::now();
        let mut codex_provider = test_upstream_provider("gpt-5.5");
        codex_provider.quota = Some(ShareUpstreamQuota {
            status: "ok".into(),
            plan: Some("ChatGPT Plus".into()),
            queried_at: Some(now.timestamp_millis()),
            subscription_period_end: None,
            availability: Some("long_window_exhausted".into()),
            blocked_until: Some((now + Duration::days(4)).to_rfc3339()),
            blocked_reason: Some("weekly quota exhausted".into()),
            blocked_scope: Some("weekly".into()),
            dispatch_limit_percent: None,
            tiers: Vec::new(),
        });
        let runtimes_json = serde_json::to_string(&ShareAppRuntimes {
            codex: Some(codex_provider),
            ..ShareAppRuntimes::default()
        })
        .expect("serialize runtimes");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares
                    SET for_sale = 'Yes', market_access_mode = 'all', app_runtimes_json = ?1
                  WHERE share_id = 'share-1'",
                params![runtimes_json],
            )
            .expect("make share visible with blocked codex runtime");
            conn.execute(
                "INSERT INTO market_share_model_failure_state (
                    market_email, share_id, app_type, requested_model, actual_model, last_status,
                    last_success_at, last_checked_at, recent_results_json, error_message, updated_at
                 ) VALUES (?1, 'share-1', 'codex', 'gpt-5.5', 'gpt-5.5',
                    'success', ?2, ?2, '[\"success\",\"success\",\"success\"]', NULL, ?2)",
                params![market.email.to_ascii_lowercase(), now.timestamp()],
            )
            .expect("insert stale success");
        }

        let active = HashSet::from(["share-sub".to_string()]);
        let shares = store
            .list_market_shares(&market.email, "main", &active, &HashMap::new(), true)
            .await
            .expect("list market shares");

        assert_eq!(shares.len(), 1);
        assert!(shares[0].app_runtimes.codex.is_none());
        let codex = shares[0]
            .app_availability
            .codex
            .as_ref()
            .expect("codex availability");
        assert_eq!(codex.status, "unavailable");
        assert_eq!(codex.reason.as_deref(), Some("weekly quota exhausted"));
        assert!((shares[0].signals.owner_penalty - 0.25).abs() < 1e-9);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    fn test_share_request_log_entry(
        request_id: &str,
        share_id: &str,
        created_at: i64,
    ) -> ShareRequestLogEntry {
        ShareRequestLogEntry {
            request_id: request_id.into(),
            share_id: share_id.into(),
            share_name: "Share".into(),
            provider_id: "provider-1".into(),
            provider_name: "Provider One".into(),
            app_type: "codex".into(),
            model: "gpt-5".into(),
            request_model: "gpt-5".into(),
            request_agent: "codex".into(),
            requested_model: "gpt-5".into(),
            actual_model: "gpt-5".into(),
            actual_model_source: "official".into(),
            status_code: 200,
            latency_ms: 100,
            first_token_ms: None,
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            is_streaming: false,
            session_id: None,
            user_country: None,
            user_country_iso3: None,
            user_email: None,
            created_at,
            is_health_check: false,
        }
    }

    fn test_image_request_log(index: usize) -> NewImageGenerationRequestLog {
        NewImageGenerationRequestLog {
            request_id: format!("imgreq-{index}"),
            share_id: "share-img".into(),
            installation_id: "inst-img".into(),
            share_name: "Image Share".into(),
            provider_id: "provider-img".into(),
            provider_name: "Image Provider".into(),
            app_type: "codex".into(),
            model: "gpt-5.5".into(),
            status: "succeeded".into(),
            status_code: Some(200),
            latency_ms: 1000,
            created_at: index as i64,
            completed_at: Some(index as i64 + 1),
            prompt_preview: Some("draw".into()),
            error_message: None,
            result_mime_type: Some("image/png".into()),
            result_size_bytes: Some(8),
            result_storage_key: Some(format!("share-img/imgreq-{index}.png")),
            result_access_token: Some(format!("token-{index}")),
            created_by_email: Some("owner@example.com".into()),
            client_ip: None,
            user_country: Some("US".into()),
        }
    }

    #[tokio::test]
    async fn image_generation_request_log_prune_keeps_recent_ten() {
        let (store, config) = setup_store("image-log-prune").await;
        for index in 0..12 {
            store
                .record_image_generation_request_log(test_image_request_log(index))
                .await
                .expect("record image log");
        }

        let stale = store
            .prune_image_generation_request_logs_for_share(
                "share-img",
                IMAGE_GENERATION_REQUEST_LOG_RETAIN_PER_SHARE,
            )
            .await
            .expect("prune image logs");
        assert_eq!(
            stale,
            vec![
                "share-img/imgreq-1.png".to_string(),
                "share-img/imgreq-0.png".to_string()
            ]
        );

        let logs = store
            .list_image_generation_request_logs_for_share("share-img", 50)
            .await
            .expect("list image logs");
        assert_eq!(logs.len(), 10);
        assert_eq!(logs[0].request_id, "imgreq-11");
        assert_eq!(logs[9].request_id, "imgreq-2");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn share_usage_by_email_is_public_app_scoped_and_limited_to_acl_emails() {
        let (store, config) = setup_store("share-usage-by-email").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-usage", "usage-sub", "active").await;
        {
            let conn = store.conn.lock().await;
            let access_by_app = BTreeMap::from([(
                "claude".to_string(),
                ShareAppAccess {
                    shared_with_emails: vec!["shareto@example.com".to_string()],
                    market_access_mode: "selected".to_string(),
                },
            )]);
            conn.execute(
                "UPDATE shares SET access_by_app_json = ?2 WHERE share_id = ?1",
                params![
                    "share-usage",
                    serde_json::to_string(&access_by_app).expect("serialize access_by_app")
                ],
            )
            .expect("set access_by_app");

            let now = Utc::now().timestamp();
            let mut owner_log = test_share_request_log_entry("req-usage-owner", "share-usage", now);
            owner_log.app_type = "claude".into();
            owner_log.user_email = Some("owner@example.com".into());
            owner_log.input_tokens = 70;
            owner_log.output_tokens = 30;
            upsert_share_request_log_tx(&conn, "inst-1", owner_log).expect("insert owner log");

            let mut shareto_log =
                test_share_request_log_entry("req-usage-shareto", "share-usage", now);
            shareto_log.app_type = "claude".into();
            shareto_log.user_email = Some("shareto@example.com".into());
            shareto_log.input_tokens = 20;
            shareto_log.output_tokens = 30;
            upsert_share_request_log_tx(&conn, "inst-1", shareto_log).expect("insert shareto log");

            let mut unknown_log =
                test_share_request_log_entry("req-usage-unknown", "share-usage", now);
            unknown_log.app_type = "claude".into();
            unknown_log.user_email = Some("unknown@example.com".into());
            unknown_log.input_tokens = 900;
            unknown_log.output_tokens = 99;
            upsert_share_request_log_tx(&conn, "inst-1", unknown_log).expect("insert unknown log");

            let mut codex_log = test_share_request_log_entry("req-usage-codex", "share-usage", now);
            codex_log.app_type = "codex".into();
            codex_log.user_email = Some("owner@example.com".into());
            codex_log.input_tokens = 500;
            codex_log.output_tokens = 500;
            upsert_share_request_log_tx(&conn, "inst-1", codex_log).expect("insert codex log");
        }

        let usage = store
            .share_usage_by_email("share-usage", "claude", "1w")
            .await
            .expect("usage");
        assert_eq!(usage.total_tokens, 150);
        assert_eq!(usage.rows.len(), 2);
        let owner = usage
            .rows
            .iter()
            .find(|row| row.email == "owner@example.com")
            .expect("owner row");
        assert_eq!(owner.role, "owner");
        assert_eq!(owner.total_tokens, 100);
        assert!((owner.percent - 66.666).abs() < 0.1);
        let shareto = usage
            .rows
            .iter()
            .find(|row| row.email == "shareto@example.com")
            .expect("shareto row");
        assert_eq!(shareto.role, "shareto");
        assert_eq!(shareto.total_tokens, 50);
        assert_eq!(shareto.daily.len(), 7);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[test]
    fn merge_market_logs_deduplicates_semantic_duplicate_with_different_request_id() {
        let started_at = Utc::now().timestamp();
        let mut share_log =
            test_share_request_log_entry("share-side-uuid", "share-usage", started_at + 6);
        share_log.requested_model = "gpt-5.5".into();
        share_log.request_model = "gpt-5.5".into();
        share_log.actual_model = "glm-5.2".into();
        share_log.model = "glm-5.2".into();
        share_log.latency_ms = 3338;
        share_log.input_tokens = 156_605;
        share_log.output_tokens = 18;
        share_log.is_streaming = true;

        let market_log = DashboardMarketRequestLogView {
            request_id: "req_market_side".into(),
            market_id: "market-id".into(),
            market_email: "xiechengqi01@gmail.com".into(),
            market_subdomain: "market".into(),
            user_email: Some("user@example.com".into()),
            api_key_prefix: Some("sk-test".into()),
            router_id: Some("main".into()),
            share_id: Some("share-usage".into()),
            share_subdomain: Some("usage-sub".into()),
            model: Some("gpt-5.5".into()),
            request_agent: "codex".into(),
            requested_model: "gpt-5.5".into(),
            actual_model: "glm-5.2".into(),
            actual_model_source: "share_runtime_mapping".into(),
            status: "settled".into(),
            status_code: Some(200),
            error_message: None,
            latency_ms: Some(5157),
            input_tokens: 156_605,
            output_tokens: 18,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            usage_amount_usd: Some("0.001".into()),
            created_at: DateTime::<Utc>::from_timestamp(started_at, 0)
                .expect("timestamp")
                .to_rfc3339(),
            settled_at: None,
            user_country: None,
            user_country_iso3: None,
        };

        let mut logs_by_share = HashMap::from([("share-usage".to_string(), vec![share_log])]);
        let shares: Vec<(String, ShareDescriptor)> = Vec::new();
        merge_market_request_logs_into_share_logs(
            &mut logs_by_share,
            &[market_log],
            &shares,
            SHARE_REQUEST_LOG_RECOVERY_LIMIT,
        );

        let logs = logs_by_share.get("share-usage").expect("share logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].request_id, "req_market_side");
        assert_eq!(logs[0].provider_id, "market-id");
        assert_eq!(logs[0].provider_name, "market");
        assert_eq!(logs[0].requested_model, "gpt-5.5");
        assert_eq!(logs[0].actual_model, "glm-5.2");
        assert_eq!(logs[0].input_tokens, 156_605);
        assert_eq!(logs[0].output_tokens, 18);
    }

    #[tokio::test]
    async fn share_usage_by_email_uses_market_request_logs_for_market_usage() {
        let (store, config) = setup_store("share-usage-by-email-market").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-usage", "usage-sub", "active").await;
        let market = test_market();
        insert_market(&store, &market).await;
        {
            let conn = store.conn.lock().await;
            let access_by_app = BTreeMap::from([(
                "codex".to_string(),
                ShareAppAccess {
                    shared_with_emails: vec![],
                    market_access_mode: "all".to_string(),
                },
            )]);
            conn.execute(
                "UPDATE shares SET access_by_app_json = ?2 WHERE share_id = ?1",
                params![
                    "share-usage",
                    serde_json::to_string(&access_by_app).expect("serialize access_by_app")
                ],
            )
            .expect("set access_by_app");

            let mut market_log = test_market_request_log("req_usage_market", "share-usage");
            market_log.user_email = Some("market@example.com".into());
            market_log.input_tokens = 220;
            market_log.output_tokens = 80;
            market_log.cache_read_tokens = 100;
            upsert_market_request_log_tx(&conn, &market, market_log).expect("insert market log");

            let now = Utc::now().timestamp();
            let mut duplicate_share_log =
                test_share_request_log_entry("req_usage_market", "share-usage", now);
            duplicate_share_log.app_type = "codex".into();
            duplicate_share_log.user_email = Some("market@example.com".into());
            duplicate_share_log.input_tokens = 900;
            upsert_share_request_log_tx(&conn, "inst-1", duplicate_share_log)
                .expect("insert duplicate share log");

            let mut direct_owner_log =
                test_share_request_log_entry("req_usage_direct", "share-usage", now);
            direct_owner_log.app_type = "codex".into();
            direct_owner_log.user_email = Some("owner@example.com".into());
            direct_owner_log.input_tokens = 40;
            direct_owner_log.output_tokens = 10;
            upsert_share_request_log_tx(&conn, "inst-1", direct_owner_log)
                .expect("insert direct owner log");
        }

        let usage = store
            .share_usage_by_email("share-usage", "codex", "1w")
            .await
            .expect("usage");
        assert_eq!(usage.total_tokens, 350);
        let market = usage
            .rows
            .iter()
            .find(|row| row.email == "market@example.com")
            .expect("market row");
        assert_eq!(market.role, "market");
        assert_eq!(market.total_tokens, 300);
        assert_eq!(market.input_tokens, 120);
        assert_eq!(market.output_tokens, 80);
        assert_eq!(market.cache_read_tokens, 100);
        let owner = usage
            .rows
            .iter()
            .find(|row| row.email == "owner@example.com")
            .expect("owner row");
        assert_eq!(owner.total_tokens, 50);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn share_usage_by_app_merges_market_and_share_logs_by_request() {
        let (store, config) = setup_store("share-usage-by-app-merged").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-usage", "usage-sub", "active").await;
        let market = test_market();
        insert_market(&store, &market).await;
        {
            let conn = store.conn.lock().await;
            let now = Utc::now().timestamp();

            let mut codex_market = test_market_request_log("req_usage_codex_market", "share-usage");
            codex_market.input_tokens = 220;
            codex_market.output_tokens = 80;
            codex_market.cache_read_tokens = 100;
            upsert_market_request_log_tx(&conn, &market, codex_market)
                .expect("insert codex market log");

            let mut duplicate_codex_share =
                test_share_request_log_entry("req_usage_codex_market", "share-usage", now);
            duplicate_codex_share.app_type = "codex".into();
            duplicate_codex_share.request_agent = "codex".into();
            duplicate_codex_share.input_tokens = 900;
            duplicate_codex_share.output_tokens = 0;
            upsert_share_request_log_tx(&conn, "inst-1", duplicate_codex_share)
                .expect("insert duplicate codex share log");

            let mut direct_codex =
                test_share_request_log_entry("req_usage_codex_direct", "share-usage", now);
            direct_codex.app_type = "codex".into();
            direct_codex.request_agent = "codex".into();
            direct_codex.input_tokens = 40;
            direct_codex.output_tokens = 10;
            upsert_share_request_log_tx(&conn, "inst-1", direct_codex)
                .expect("insert direct codex share log");

            let mut claude_market = test_market_request_log("req_usage_claude_zero", "share-usage");
            claude_market.request_agent = "claude".into();
            claude_market.requested_model = "claude-opus-4-7".into();
            claude_market.actual_model = "claude-opus-4-7".into();
            claude_market.input_tokens = 0;
            claude_market.output_tokens = 0;
            claude_market.cache_read_tokens = 0;
            claude_market.cache_creation_tokens = 0;
            upsert_market_request_log_tx(&conn, &market, claude_market)
                .expect("insert zero claude market log");

            let mut claude_share =
                test_share_request_log_entry("req_usage_claude_zero", "share-usage", now);
            claude_share.app_type = "claude".into();
            claude_share.request_agent = "claude".into();
            claude_share.input_tokens = 1;
            claude_share.output_tokens = 2;
            claude_share.cache_read_tokens = 100;
            upsert_share_request_log_tx(&conn, "inst-1", claude_share)
                .expect("insert matching claude share log");

            let mut gemini_market =
                test_market_request_log("req_usage_gemini_market", "share-usage");
            gemini_market.request_agent = "gemini".into();
            gemini_market.requested_model = "gemini-2.5-pro".into();
            gemini_market.actual_model = "gemini-2.5-pro".into();
            gemini_market.input_tokens = 4;
            gemini_market.output_tokens = 5;
            gemini_market.cache_read_tokens = 6;
            upsert_market_request_log_tx(&conn, &market, gemini_market)
                .expect("insert gemini market log");

            let usage = list_share_usage_by_app(&conn).expect("list share usage");
            let (tokens_by_app, requests_by_app) =
                usage.get("share-usage").expect("share usage row");
            assert_eq!(tokens_by_app.get("codex"), Some(&350));
            assert_eq!(requests_by_app.get("codex"), Some(&2));
            assert_eq!(tokens_by_app.get("claude"), Some(&103));
            assert_eq!(requests_by_app.get("claude"), Some(&1));
            assert_eq!(tokens_by_app.get("gemini"), Some(&15));
            assert_eq!(requests_by_app.get("gemini"), Some(&1));
        }

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn share_usage_by_email_defaults_to_rolling_24h() {
        let (store, config) = setup_store("share-usage-by-email-24h").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-usage", "usage-sub", "active").await;
        {
            let conn = store.conn.lock().await;
            let now = Utc::now().timestamp();

            let mut recent_log =
                test_share_request_log_entry("req-usage-recent", "share-usage", now - 23 * 3600);
            recent_log.app_type = "codex".into();
            recent_log.user_email = Some("owner@example.com".into());
            recent_log.input_tokens = 20;
            recent_log.output_tokens = 5;
            upsert_share_request_log_tx(&conn, "inst-1", recent_log).expect("insert recent log");

            let mut old_log =
                test_share_request_log_entry("req-usage-old", "share-usage", now - 25 * 3600);
            old_log.app_type = "codex".into();
            old_log.user_email = Some("owner@example.com".into());
            old_log.input_tokens = 900;
            old_log.output_tokens = 100;
            upsert_share_request_log_tx(&conn, "inst-1", old_log).expect("insert old log");
        }

        let usage = store
            .share_usage_by_email("share-usage", "codex", "")
            .await
            .expect("usage");
        assert_eq!(usage.period, "24h");
        assert_eq!(usage.bucket_granularity, "hour");
        assert!((1..=2).contains(&usage.days));
        assert_eq!(usage.total_tokens, 25);
        let owner = usage
            .rows
            .iter()
            .find(|row| row.email == "owner@example.com")
            .expect("owner row");
        assert_eq!(owner.total_tokens, 25);
        assert!(owner.daily.len() >= 24);
        assert!(owner.daily.iter().all(|bucket| bucket.date.contains('T')));
        assert_eq!(
            owner
                .daily
                .iter()
                .map(|bucket| bucket.total_tokens)
                .sum::<u64>(),
            25
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[test]
    fn share_logs_need_recovery_for_empty_or_stale_logs() {
        let now_ts = Utc::now().timestamp();
        assert!(share_logs_need_recovery(None, now_ts));
        assert!(share_logs_need_recovery(Some(&[]), now_ts));

        let stale = vec![test_share_request_log_entry(
            "req-stale",
            "share-1",
            now_ts - SHARE_REQUEST_LOG_RECOVERY_STALE_SECS - 1,
        )];
        assert!(share_logs_need_recovery(Some(&stale), now_ts));

        let fresh = vec![test_share_request_log_entry(
            "req-fresh",
            "share-1",
            now_ts - 60,
        )];
        assert!(!share_logs_need_recovery(Some(&fresh), now_ts));
    }

    #[tokio::test]
    async fn list_market_shares_all_access_allows_future_market_email() {
        let (store, config) = setup_store("market-share-all-access").await;
        insert_installation(&store, "inst-all").await;
        insert_share(&store, "inst-all", "share-all", "all-share-sub", "active").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', market_access_mode = 'all' WHERE share_id = ?1",
                params!["share-all"],
            )
            .expect("enable all market access");
        }

        let shares = store
            .list_market_shares(
                "future-market@example.com",
                "router-test",
                &HashSet::new(),
                &HashMap::new(),
                true,
            )
            .await
            .expect("list market shares");

        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].share_id, "share-all");
        assert_eq!(shares[0].market_access_mode, "all");
        assert_eq!(shares[0].subdomain, "all-share-sub");
        assert_eq!(shares[0].share_status, "active");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn list_market_shares_excludes_share_market_sales() {
        let (store, config) = setup_store("market-share-kind-filter").await;
        insert_installation(&store, "inst-kind").await;
        insert_share(
            &store,
            "inst-kind",
            "share-kind",
            "kind-share-sub",
            "active",
        )
        .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', sale_market_kind = 'share', market_access_mode = 'all' WHERE share_id = ?1",
                params!["share-kind"],
            )
            .expect("enable share market sale");
        }

        let shares = store
            .list_market_shares(
                "usage-market@example.com",
                "router-test",
                &HashSet::new(),
                &HashMap::new(),
                true,
            )
            .await
            .expect("list usage market shares");
        assert!(shares.is_empty());

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_share_market_sale_links_only_explicit_share_market() {
        let (store, config) = setup_store("dashboard-share-market-link").await;
        let usage_market = test_market();
        insert_market(&store, &usage_market).await;
        let mut share_market = test_market();
        share_market.id = "share-market".into();
        share_market.display_name = "Share Market".into();
        share_market.email = "share-market@example.com".into();
        share_market.subdomain = "share-market".into();
        share_market.public_base_url = "https://share-market.example.com".into();
        share_market.market_kind = "share".into();
        insert_market(&store, &share_market).await;
        insert_installation(&store, "inst-share-market-link").await;
        insert_share(
            &store,
            "inst-share-market-link",
            "share-market-link",
            "share-market-link-sub",
            "active",
        )
        .await;
        set_share_shared_with_emails(&store, "share-market-link", &["share-market@example.com"])
            .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares
                    SET for_sale = 'Yes',
                        sale_market_kind = 'share',
                        market_access_mode = 'all',
                        parallel_limit = 7,
                        tokens_used = 123456
                  WHERE share_id = ?1",
                params!["share-market-link"],
            )
            .expect("enable share market sale");
        }

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let dashboard = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard");
        let share = dashboard
            .shares
            .iter()
            .find(|share| share.share_id == "share-market-link")
            .expect("share view");

        assert_eq!(share.router_id, "main");
        assert_eq!(share.sale_market_kind, "share");
        assert_eq!(share.market_links.len(), 1);
        assert_eq!(share.market_links[0].email, "share-market@example.com");
        assert_eq!(share.market_links[0].market_kind, "share");
        assert_eq!(
            share.market_links[0].public_base_url,
            "https://share-market.example.com"
        );
        let dashboard_share_market = dashboard
            .markets
            .iter()
            .find(|market| market.email == "share-market@example.com")
            .expect("share market dashboard view");
        assert_eq!(dashboard_share_market.share_count, 1);
        assert_eq!(dashboard_share_market.parallel_capacity, 7);
        assert_eq!(dashboard_share_market.usage_tokens, 123456);
        assert_eq!(dashboard_share_market.usage_amount_usd, "0.00000000");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_uses_cached_share_market_listing_status() {
        let (store, config) = setup_store("dashboard-share-market-listing-status").await;
        let mut share_market = test_market();
        share_market.id = "share-market-status".into();
        share_market.email = "share-market@example.com".into();
        share_market.subdomain = "share-market".into();
        share_market.public_base_url = "https://share-market.example.com".into();
        share_market.market_kind = "share".into();
        insert_market(&store, &share_market).await;
        insert_installation(&store, "inst-share-market-status").await;
        insert_share(
            &store,
            "inst-share-market-status",
            "share-market-status",
            "share-market-status-sub",
            "active",
        )
        .await;
        set_share_shared_with_emails(&store, "share-market-status", &["share-market@example.com"])
            .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares
                    SET for_sale = 'Yes',
                        sale_market_kind = 'share'
                  WHERE share_id = ?1",
                params!["share-market-status"],
            )
            .expect("enable share market sale");
        }

        let synced = store
            .sync_share_market_listing_statuses(
                "share-market@example.com",
                true,
                vec![ShareMarketListingStatusInput {
                    router_id: Some("main".into()),
                    share_id: "share-market-status".into(),
                    app_type: "codex".into(),
                    listing_url: "https://share-market.example.com/listing/share?router_id=main&share_id=share-market-status&app_type=codex".into(),
                    status: "idle".into(),
                    sale_mode: Some("single".into()),
                    filled_seats: Some(0),
                    required_seats: Some(3),
                    listing_status: Some("active".into()),
                    expires_at: Some((Utc::now() + Duration::minutes(5)).to_rfc3339()),
                }],
            )
            .await
            .expect("sync listing status");
        assert_eq!(synced, 1);

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let mut dashboard = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard");
        store
            .attach_share_market_listing_statuses(&mut dashboard)
            .await
            .expect("attach listing statuses");
        let share = dashboard
            .shares
            .iter()
            .find(|share| share.share_id == "share-market-status")
            .expect("share view");
        let listing_status = share.market_links[0]
            .listing_status_by_app
            .get("codex")
            .expect("codex listing status");
        assert_eq!(listing_status.status, "idle");
        assert_eq!(listing_status.sale_mode.as_deref(), Some("single"));
        assert!(!listing_status.is_stale);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn share_market_grant_requires_explicit_delegation_not_all_access() {
        let (store, config) = setup_store("share-market-grant-explicit-acl").await;
        insert_installation(&store, "inst-share-market").await;
        insert_share(
            &store,
            "inst-share-market",
            "share-market-all-only",
            "share-market-all-sub",
            "active",
        )
        .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', sale_market_kind = 'share', market_access_mode = 'all' WHERE share_id = ?1",
                params!["share-market-all-only"],
            )
            .expect("enable usage market all access");
        }

        let err = store
            .create_share_market_grant(
                "share-market@example.com",
                "share-market-all-only",
                ShareMarketGrantRequest {
                    grant_id: "grant-all-only".into(),
                    action: "add".into(),
                    app_type: None,
                    buyer_emails: vec!["buyer@example.com".into()],
                    order_ids: vec!["order-1".into()],
                    listing_id: None,
                    carpool_group_id: None,
                    seat_count: None,
                },
            )
            .await
            .expect_err("share market must not inherit usage all access");
        assert!(err.to_string().contains("not delegated"));

        set_share_shared_with_emails(
            &store,
            "share-market-all-only",
            &["share-market@example.com"],
        )
        .await;
        let granted = store
            .create_share_market_grant(
                "share-market@example.com",
                "share-market-all-only",
                ShareMarketGrantRequest {
                    grant_id: "grant-explicit".into(),
                    action: "add".into(),
                    app_type: None,
                    buyer_emails: vec!["buyer@example.com".into()],
                    order_ids: vec!["order-1".into()],
                    listing_id: None,
                    carpool_group_id: None,
                    seat_count: None,
                },
            )
            .await
            .expect("explicitly delegated share market can create grant");
        assert_eq!(granted.status, "pending");
        let grant_status = store
            .share_market_grant_status(
                "share-market@example.com",
                "share-market-all-only",
                &granted.router_edit_id,
            )
            .await
            .expect("share market can query its own grant edit");
        assert_eq!(grant_status.status, "pending");

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'applied', applied_at = updated_at WHERE id = ?1",
                params![granted.router_edit_id],
            )
            .expect("mark grant edit applied");
        }
        let applied_status = store
            .share_market_grant_status(
                "share-market@example.com",
                "share-market-all-only",
                &granted.router_edit_id,
            )
            .await
            .expect("share market sees applied grant edit");
        assert_eq!(applied_status.status, "applied");
        assert!(applied_status.applied_at.is_some());

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn share_market_grant_rejects_token_market_sale() {
        let (store, config) = setup_store("share-market-grant-token-kind").await;
        insert_installation(&store, "inst-token-kind").await;
        insert_share(
            &store,
            "inst-token-kind",
            "share-token-kind",
            "token-kind-sub",
            "active",
        )
        .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', sale_market_kind = 'token', shared_with_emails_json = ?2 WHERE share_id = ?1",
                params![
                    "share-token-kind",
                    serde_json::to_string(&vec!["share-market@example.com"]).expect("serialize emails")
                ],
            )
            .expect("enable token market sale with explicit share market email");
        }

        let err = store
            .create_share_market_grant(
                "share-market@example.com",
                "share-token-kind",
                ShareMarketGrantRequest {
                    grant_id: "grant-token-kind".into(),
                    action: "add".into(),
                    app_type: None,
                    buyer_emails: vec!["buyer@example.com".into()],
                    order_ids: vec!["order-1".into()],
                    listing_id: None,
                    carpool_group_id: None,
                    seat_count: None,
                },
            )
            .await
            .expect_err("share market must reject token market sale");
        assert!(err.to_string().contains("share-market sale"));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn list_market_shares_marks_online_by_subdomain() {
        let (store, config) = setup_store("market-share-online-subdomain").await;
        insert_installation(&store, "inst-online").await;
        insert_share(
            &store,
            "inst-online",
            "share-online",
            "online-share-sub",
            "active",
        )
        .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', market_access_mode = 'all' WHERE share_id = ?1",
                params!["share-online"],
            )
            .expect("enable all market access");
        }
        let active_subdomains = HashSet::from(["online-share-sub".to_string()]);

        let shares = store
            .list_market_shares(
                "future-market@example.com",
                "router-test",
                &active_subdomains,
                &HashMap::new(),
                true,
            )
            .await
            .expect("list market shares");

        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].subdomain, "online-share-sub");
        assert!(shares[0].online);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    async fn insert_signed_installation(store: &AppStore, installation_id: &str) -> SigningKey {
        let signing_key = SigningKey::generate(&mut OsRng);
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());
        let now = Utc::now().to_rfc3339();
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, owner_email, owner_verified_at, created_at, last_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                installation_id,
                public_key,
                "macOS",
                "1.0.0",
                "owner@example.com",
                now,
                now,
                now
            ],
        )
        .expect("insert signed installation");
        signing_key
    }

    fn sign_test_payload<T: Serialize>(
        signing_key: &SigningKey,
        installation_id: &str,
        action: &str,
        payload: &T,
        timestamp_ms: i64,
        nonce: &str,
    ) -> String {
        let payload_json = serde_json::to_string(payload).expect("serialize test payload");
        let body = format!("{installation_id}\n{action}\n{payload_json}\n{timestamp_ms}\n{nonce}");
        let signature = signing_key.sign(body.as_bytes());
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
    }

    fn payout_update(
        revision: i64,
        profile: Option<crate::models::PayoutProfile>,
    ) -> crate::models::InstallationPayoutProfileUpdate {
        crate::models::InstallationPayoutProfileUpdate {
            schema_version: crate::models::PAYOUT_PROFILE_SCHEMA_VERSION,
            revision,
            profile,
            updated_at_ms: 1_750_000_000_000 + revision,
        }
    }

    fn test_payout_profile() -> crate::models::PayoutProfile {
        crate::models::PayoutProfile {
            address_type: crate::models::PayoutAddressType::Evm,
            address: "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".into(),
            token: crate::models::PayoutToken::USDC,
            networks: vec![
                crate::models::PayoutNetwork::Base,
                crate::models::PayoutNetwork::Bsc,
            ],
            verification_status: crate::models::PayoutVerificationStatus::SelfDeclared,
        }
    }

    async fn push_test_payout_update(
        store: &AppStore,
        signing_key: &SigningKey,
        installation_id: &str,
        update: crate::models::InstallationPayoutProfileUpdate,
    ) -> Result<crate::models::InstallationPayoutProfileUpdateResponse, AppError> {
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            signing_key,
            installation_id,
            "update_installation_payout_profile",
            &update,
            timestamp_ms,
            &nonce,
        );
        store
            .update_installation_payout_profile(
                crate::models::InstallationPayoutProfileUpdateRequest {
                    installation_id: installation_id.into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    update,
                },
            )
            .await
    }

    #[tokio::test]
    async fn payout_profile_revision_and_tombstone_prevent_stale_restore() {
        let (store, config) = setup_store("payout-profile-revisions").await;
        let signing_key = insert_signed_installation(&store, "inst-payout").await;

        let first = payout_update(1, Some(test_payout_profile()));
        push_test_payout_update(&store, &signing_key, "inst-payout", first.clone())
            .await
            .expect("publish payout profile");
        push_test_payout_update(&store, &signing_key, "inst-payout", first.clone())
            .await
            .expect("same revision and content is idempotent");

        let public = store
            .public_installation_payout_profile("inst-payout")
            .await
            .expect("public payout profile");
        assert!(public.configured);
        assert_eq!(public.revision, 1);
        assert_eq!(
            public.profile.as_ref().unwrap().address,
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
        );

        push_test_payout_update(&store, &signing_key, "inst-payout", payout_update(2, None))
            .await
            .expect("clear payout profile");
        let stale = push_test_payout_update(
            &store,
            &signing_key,
            "inst-payout",
            payout_update(1, Some(test_payout_profile())),
        )
        .await
        .expect_err("stale profile must not restore tombstone");
        assert!(matches!(stale, AppError::Conflict(_)));

        let cleared = store
            .public_installation_payout_profile("inst-payout")
            .await
            .expect("cleared public payout profile");
        assert!(!cleared.configured);
        assert_eq!(cleared.revision, 2);
        assert!(cleared.profile.is_none());

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn payout_profile_rejects_tampered_signature_and_replayed_nonce() {
        let (store, config) = setup_store("payout-profile-signature").await;
        let signing_key = insert_signed_installation(&store, "inst-payout-signature").await;
        let update = payout_update(1, Some(test_payout_profile()));
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-payout-signature",
            "update_installation_payout_profile",
            &update,
            timestamp_ms,
            &nonce,
        );

        let mut tampered = update.clone();
        tampered.profile.as_mut().unwrap().token = crate::models::PayoutToken::USDT;
        let error = store
            .update_installation_payout_profile(
                crate::models::InstallationPayoutProfileUpdateRequest {
                    installation_id: "inst-payout-signature".into(),
                    timestamp_ms,
                    nonce: nonce.clone(),
                    signature: signature.clone(),
                    update: tampered,
                },
            )
            .await
            .expect_err("tampered payout profile must fail signature verification");
        assert!(matches!(error, AppError::Unauthorized(_)));

        store
            .update_installation_payout_profile(
                crate::models::InstallationPayoutProfileUpdateRequest {
                    installation_id: "inst-payout-signature".into(),
                    timestamp_ms,
                    nonce: nonce.clone(),
                    signature: signature.clone(),
                    update: update.clone(),
                },
            )
            .await
            .expect("untampered payout profile");
        let replay = store
            .update_installation_payout_profile(
                crate::models::InstallationPayoutProfileUpdateRequest {
                    installation_id: "inst-payout-signature".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    update,
                },
            )
            .await
            .expect_err("replayed nonce must fail");
        assert!(matches!(replay, AppError::Unauthorized(_)));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_includes_client_with_only_a_payout_profile() {
        let (store, config) = setup_store("payout-profile-dashboard-client").await;
        let signing_key = insert_signed_installation(&store, "inst-payout-only").await;
        push_test_payout_update(
            &store,
            &signing_key,
            "inst-payout-only",
            payout_update(1, Some(test_payout_profile())),
        )
        .await
        .expect("publish payout profile");

        let snapshot = store
            .dashboard_snapshot(
                &config,
                &ServerGeo {
                    lat: None,
                    lon: None,
                },
                &ProxyRegistry::default(),
                None,
            )
            .await
            .expect("dashboard snapshot");
        assert_eq!(snapshot.clients.len(), 1);
        assert_eq!(snapshot.clients[0].installation.id, "inst-payout-only");
        assert!(snapshot.clients[0].payout_profile.is_some());
        assert!(snapshot.clients[0].share_ids.is_empty());
        assert!(snapshot.clients[0].client_tunnel.is_none());

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    fn sign_issue_lease_request(
        signing_key: &SigningKey,
        installation_id: &str,
        requested_subdomain: &str,
        tunnel_type: &str,
        timestamp_ms: i64,
        nonce: &str,
    ) -> String {
        let body = format!(
            "{installation_id}\n{requested_subdomain}\n{tunnel_type}\n{timestamp_ms}\n{nonce}"
        );
        let signature = signing_key.sign(body.as_bytes());
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
    }

    fn public_key_b64(signing_key: &SigningKey) -> String {
        base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key().to_bytes())
    }

    fn sign_registration_recovery_request(
        signing_key: &SigningKey,
        installation_id: &str,
        public_key: &str,
        platform: &str,
        app_version: &str,
        instance_nonce: &str,
        timestamp_ms: i64,
    ) -> String {
        let body = format!(
            "{installation_id}\nregister_installation\n{public_key}\n{platform}\n{app_version}\n{instance_nonce}\n{timestamp_ms}"
        );
        let signature = signing_key.sign(body.as_bytes());
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
    }

    #[tokio::test]
    async fn list_share_route_targets_only_returns_active_shares() {
        let (store, config) = setup_store("route-targets").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-active", "active-sub", "active").await;
        insert_share(&store, "inst-1", "share-paused", "paused-sub", "paused").await;

        let targets = store
            .list_share_route_targets()
            .await
            .expect("list route targets");
        let subdomains = targets
            .into_iter()
            .map(|target| target.subdomain)
            .collect::<Vec<_>>();

        assert_eq!(subdomains, vec!["active-sub".to_string()]);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn share_runtime_refresh_accepts_signed_owner_request() {
        let (store, config) = setup_store("runtime-refresh-signed").await;
        let signing_key = insert_signed_installation(&store, "inst-runtime").await;
        insert_share(
            &store,
            "inst-runtime",
            "share-runtime",
            "runtime-sub",
            "active",
        )
        .await;
        let refresh = ShareRuntimeRefreshPayload {
            share_id: "share-runtime".into(),
            subdomain: "runtime-sub".into(),
        };
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-runtime",
            "share_runtime_refresh",
            &refresh,
            timestamp_ms,
            &nonce,
        );

        let accepted = store
            .prepare_share_runtime_refresh(
                ShareRuntimeRefreshRequest {
                    installation_id: "inst-runtime".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    refresh,
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .expect("runtime refresh should be accepted");

        assert_eq!(accepted.share_id, "share-runtime");
        assert_eq!(accepted.subdomain, "runtime-sub");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn share_runtime_refresh_rejects_subdomain_mismatch() {
        let (store, config) = setup_store("runtime-refresh-mismatch").await;
        let signing_key = insert_signed_installation(&store, "inst-runtime-mismatch").await;
        insert_share(
            &store,
            "inst-runtime-mismatch",
            "share-runtime-mismatch",
            "runtime-sub",
            "active",
        )
        .await;
        let refresh = ShareRuntimeRefreshPayload {
            share_id: "share-runtime-mismatch".into(),
            subdomain: "other-sub".into(),
        };
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-runtime-mismatch",
            "share_runtime_refresh",
            &refresh,
            timestamp_ms,
            &nonce,
        );

        let err = store
            .prepare_share_runtime_refresh(
                ShareRuntimeRefreshRequest {
                    installation_id: "inst-runtime-mismatch".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    refresh,
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .expect_err("subdomain mismatch should be rejected");

        assert!(err.to_string().contains("subdomain mismatch"));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn issue_lease_allows_same_share_to_renew_existing_route() {
        let (store, config) = setup_store("issue-lease-same-share-renew").await;
        let signing_key = insert_signed_installation(&store, "inst-renew").await;
        insert_share(&store, "inst-renew", "share-renew", "aaa", "active").await;
        let proxy = ProxyRegistry::default();
        proxy
            .set_route(
                "aaa".into(),
                "127.0.0.1:65530".into(),
                None,
                Some("share-renew".into()),
                Some("share-share-renew".into()),
                false,
                -1,
                None,
            )
            .await;

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_issue_lease_request(
            &signing_key,
            "inst-renew",
            "aaa",
            "http",
            timestamp_ms,
            &nonce,
        );

        let lease = store
            .issue_lease(
                &config,
                &proxy,
                IssueLeaseRequest {
                    installation_id: "inst-renew".into(),
                    requested_subdomain: "aaa".into(),
                    tunnel_type: "http".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    share: Some(test_share_descriptor("share-renew", "aaa")),
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                None,
            )
            .await
            .expect("same share route should be renewable");

        assert_eq!(lease.subdomain, "aaa");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn renew_lease_extends_active_connection_without_replacing_route() {
        let (store, mut config) = setup_store("renew-active-lease").await;
        config.lease_ttl_secs = 300;
        let signing_key = insert_signed_installation(&store, "inst-active-renew").await;
        insert_share(
            &store,
            "inst-active-renew",
            "share-active-renew",
            "active-renew-sub",
            "active",
        )
        .await;
        let proxy = ProxyRegistry::default();
        let issued_at_ms = Utc::now().timestamp_millis();
        let issue_nonce = Uuid::new_v4().to_string();
        let lease = store
            .issue_lease(
                &config,
                &proxy,
                IssueLeaseRequest {
                    installation_id: "inst-active-renew".into(),
                    requested_subdomain: "active-renew-sub".into(),
                    tunnel_type: "http".into(),
                    timestamp_ms: issued_at_ms,
                    nonce: issue_nonce.clone(),
                    signature: sign_issue_lease_request(
                        &signing_key,
                        "inst-active-renew",
                        "active-renew-sub",
                        "http",
                        issued_at_ms,
                        &issue_nonce,
                    ),
                    share: Some(test_share_descriptor(
                        "share-active-renew",
                        "active-renew-sub",
                    )),
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                None,
            )
            .await
            .expect("issue active lease");
        store
            .consume_lease(&lease.ssh_username, &lease.ssh_password)
            .await
            .expect("consume active lease");
        proxy
            .set_route_with_kind(
                "active-renew-sub".into(),
                "127.0.0.1:65530".into(),
                RouteKind::Share,
                Some("inst-active-renew".into()),
                Some(lease.connection_id.clone()),
                Some("share-active-renew".into()),
                Some("Active Renew".into()),
                false,
                -1,
                None,
            )
            .await;

        let renewal = RenewLeasePayload {
            lease_id: lease.lease_id.clone(),
            connection_id: lease.connection_id.clone(),
        };
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let request = RenewLeaseRequest {
            installation_id: "inst-active-renew".into(),
            timestamp_ms,
            nonce: nonce.clone(),
            signature: sign_test_payload(
                &signing_key,
                "inst-active-renew",
                "renew_lease",
                &renewal,
                timestamp_ms,
                &nonce,
            ),
            renewal,
        };
        let renewed = store
            .renew_lease(
                &config,
                &proxy,
                request.clone(),
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
            )
            .await
            .expect("renew active lease");

        assert!(renewed.expires_at >= lease.expires_at);
        let route = proxy
            .backend_for_host(
                &format!("active-renew-sub.{}", config.tunnel_domain),
                &config.tunnel_domain,
            )
            .await
            .expect("active route remains registered");
        assert_eq!(route.connection_id(), Some(lease.connection_id.as_str()));
        let replay = store
            .renew_lease(
                &config,
                &proxy,
                request,
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .expect_err("renewal nonce replay must fail");
        assert!(replay.to_string().contains("nonce already used"));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn issue_lease_rejects_replayed_nonce_without_replacing_lease() {
        let (store, config) = setup_store("issue-lease-replay-nonce").await;
        let signing_key = insert_signed_installation(&store, "inst-replay").await;
        insert_share(
            &store,
            "inst-replay",
            "share-replay",
            "replay-sub",
            "active",
        )
        .await;
        let proxy = ProxyRegistry::default();

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let request = IssueLeaseRequest {
            installation_id: "inst-replay".into(),
            requested_subdomain: "replay-sub".into(),
            tunnel_type: "http".into(),
            timestamp_ms,
            nonce: nonce.clone(),
            signature: sign_issue_lease_request(
                &signing_key,
                "inst-replay",
                "replay-sub",
                "http",
                timestamp_ms,
                &nonce,
            ),
            share: Some(test_share_descriptor("share-replay", "replay-sub")),
        };

        let first = store
            .issue_lease(
                &config,
                &proxy,
                request.clone(),
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                None,
            )
            .await
            .expect("first lease should succeed");
        let replay = store
            .issue_lease(
                &config,
                &proxy,
                request,
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                None,
            )
            .await
            .expect_err("same nonce must be rejected");
        assert!(replay.to_string().contains("nonce already used"));

        let conn = store.conn.lock().await;
        let lease_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM leases WHERE connection_id = ?1",
                params![first.connection_id],
                |row| row.get(0),
            )
            .expect("count first lease");
        assert_eq!(lease_count, 1);
        drop(conn);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn issue_lease_bad_signature_does_not_delete_existing_lease() {
        let (store, config) = setup_store("issue-lease-bad-signature-preserves").await;
        let signing_key = insert_signed_installation(&store, "inst-bad-sig").await;
        insert_share(
            &store,
            "inst-bad-sig",
            "share-bad-sig",
            "bad-sig-sub",
            "active",
        )
        .await;
        let proxy = ProxyRegistry::default();

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let first = store
            .issue_lease(
                &config,
                &proxy,
                IssueLeaseRequest {
                    installation_id: "inst-bad-sig".into(),
                    requested_subdomain: "bad-sig-sub".into(),
                    tunnel_type: "http".into(),
                    timestamp_ms,
                    nonce: nonce.clone(),
                    signature: sign_issue_lease_request(
                        &signing_key,
                        "inst-bad-sig",
                        "bad-sig-sub",
                        "http",
                        timestamp_ms,
                        &nonce,
                    ),
                    share: Some(test_share_descriptor("share-bad-sig", "bad-sig-sub")),
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                None,
            )
            .await
            .expect("first lease should succeed");

        let bad = store
            .issue_lease(
                &config,
                &proxy,
                IssueLeaseRequest {
                    installation_id: "inst-bad-sig".into(),
                    requested_subdomain: "bad-sig-sub".into(),
                    tunnel_type: "http".into(),
                    timestamp_ms: Utc::now().timestamp_millis(),
                    nonce: Uuid::new_v4().to_string(),
                    signature: "not-a-valid-signature".into(),
                    share: Some(test_share_descriptor("share-bad-sig", "bad-sig-sub")),
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                None,
            )
            .await
            .expect_err("invalid signature must be rejected");
        assert!(bad.to_string().contains("invalid signature"));

        let conn = store.conn.lock().await;
        let lease_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM leases WHERE connection_id = ?1",
                params![first.connection_id],
                |row| row.get(0),
            )
            .expect("count preserved lease");
        assert_eq!(lease_count, 1);
        drop(conn);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn register_installation_does_not_return_existing_control_secret_without_signature() {
        let (store, config) = setup_store("register-existing-no-secret-leak").await;
        let signing_key = insert_signed_installation(&store, "inst-existing-register").await;
        let public_key = public_key_b64(&signing_key);

        let response = store
            .register_installation(
                RegisterInstallationRequest {
                    public_key,
                    platform: "macOS".into(),
                    app_version: "2.0.0".into(),
                    instance_nonce: Uuid::new_v4().to_string(),
                    timestamp_ms: None,
                    signature: None,
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .expect("existing register should refresh metadata");

        assert_eq!(response.installation_id, "inst-existing-register");
        assert!(response.control_secret.is_none());
        let conn = store.conn.lock().await;
        let stored_secret: Option<String> = conn
            .query_row(
                "SELECT control_secret_b64 FROM installations WHERE id = 'inst-existing-register'",
                [],
                |row| row.get(0),
            )
            .expect("read stored control secret");
        assert!(stored_secret.is_some());
        drop(conn);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn register_installation_signed_recovery_returns_existing_control_secret() {
        let (store, config) = setup_store("register-existing-signed-recovery").await;
        let signing_key = insert_signed_installation(&store, "inst-signed-register").await;
        let public_key = public_key_b64(&signing_key);
        let instance_nonce = Uuid::new_v4().to_string();
        let timestamp_ms = Utc::now().timestamp_millis();
        let signature = sign_registration_recovery_request(
            &signing_key,
            "inst-signed-register",
            &public_key,
            "macOS",
            "2.0.0",
            &instance_nonce,
            timestamp_ms,
        );

        let response = store
            .register_installation(
                RegisterInstallationRequest {
                    public_key,
                    platform: "macOS".into(),
                    app_version: "2.0.0".into(),
                    instance_nonce,
                    timestamp_ms: Some(timestamp_ms),
                    signature: Some(signature),
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .expect("signed registration recovery should return control secret");

        assert_eq!(response.installation_id, "inst-signed-register");
        assert!(response.control_secret.is_some());

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn issue_market_lease_uses_registered_market_subdomain() {
        let (store, config) = setup_store("market-lease").await;
        let market = test_market();
        insert_market(&store, &market).await;
        let proxy = ProxyRegistry::default();

        let lease = store
            .issue_market_lease(&config, &proxy, &market)
            .await
            .expect("issue market lease");

        assert_eq!(lease.subdomain, "market-a");
        assert_eq!(lease.tunnel_url, "http://market-a.127.0.0.1:8787");

        let consumed = store
            .consume_lease(&lease.ssh_username, &lease.ssh_password)
            .await
            .expect("consume market lease");
        assert_eq!(consumed.installation_id, "market:main-market");
        assert_eq!(consumed.tunnel_type, "market-http");
        assert!(consumed.share.is_none());

        let replay = store
            .consume_lease(&lease.ssh_username, &lease.ssh_password)
            .await
            .expect_err("market lease cannot be reused");
        assert!(replay.to_string().contains("lease already used"));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    /// `is_market_subdomain` is what HTTP handlers use to decide whether to
    /// skip the router's bundled share-landing UI on the request's host. It
    /// must return true exactly for subdomains that have a row in
    /// `router_markets` — any registration status (active/offline/disabled)
    /// counts, because all of them still proxy traffic to a market backend.
    #[tokio::test]
    async fn is_market_subdomain_matches_registered_markets_only() {
        let (store, config) = setup_store("is-market-subdomain").await;

        assert!(
            !store.is_market_subdomain("market-a").await,
            "before registration, market subdomain must not be considered a market"
        );
        assert!(
            !store.is_market_subdomain("").await,
            "empty subdomain must short-circuit to false"
        );

        store
            .register_market(
                "market@example.com",
                RegisterMarketRequest {
                    subdomain: "market-a".into(),
                    display_name: None,
                    public_base_url: "https://market-a.example.com".into(),
                    market_kind: None,
                    pricing_summary: None,
                },
            )
            .await
            .expect("register market");

        assert!(
            store.is_market_subdomain("market-a").await,
            "registered market subdomain must be recognized"
        );
        assert!(
            !store.is_market_subdomain("market-b").await,
            "unrelated subdomain must stay non-market"
        );
        assert!(
            !store.is_market_subdomain("alpha-share").await,
            "share subdomain must never be flagged as market"
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn register_market_upserts_same_email_and_rejects_subdomain_conflict() {
        let (store, config) = setup_store("market-register").await;

        let registered = store
            .register_market(
                "market@example.com",
                RegisterMarketRequest {
                    subdomain: "market-a".into(),
                    display_name: None,
                    public_base_url: "https://market-a.example.com".into(),
                    market_kind: None,
                    pricing_summary: None,
                },
            )
            .await
            .expect("register market");
        assert_eq!(registered.email, "market@example.com");
        assert_eq!(registered.subdomain, "market-a");

        let updated = store
            .register_market(
                "market@example.com",
                RegisterMarketRequest {
                    subdomain: "market-b".into(),
                    display_name: Some("Renamed Market".into()),
                    public_base_url: "https://market-b.example.com".into(),
                    market_kind: None,
                    pricing_summary: None,
                },
            )
            .await
            .expect("upsert same market email");
        assert_eq!(updated.display_name, "https://market-b.example.com");
        assert_eq!(updated.subdomain, "market-b");

        let err = store
            .register_market(
                "other@example.com",
                RegisterMarketRequest {
                    subdomain: "market-b".into(),
                    display_name: None,
                    public_base_url: "https://market-b.example.com".into(),
                    market_kind: None,
                    pricing_summary: None,
                },
            )
            .await
            .expect_err("subdomain cannot be claimed by a different email");
        assert!(err.to_string().contains("already registered"));

        let public_markets = store.list_public_markets().await.expect("list markets");
        assert_eq!(public_markets.len(), 1);
        assert_eq!(public_markets[0].email, "market@example.com");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn issue_market_lease_replaces_stale_market_lease_and_route() {
        let (store, config) = setup_store("market-lease-duplicate").await;
        let market = test_market();
        insert_market(&store, &market).await;
        let proxy = ProxyRegistry::default();

        let first = store
            .issue_market_lease(&config, &proxy, &market)
            .await
            .expect("first market lease");
        proxy
            .set_route(
                market.subdomain.clone(),
                "127.0.0.1:65530".into(),
                None,
                None,
                None,
                false,
                -1,
                None,
            )
            .await;

        let second = store
            .issue_market_lease(&config, &proxy, &market)
            .await
            .expect("replacement market lease");
        assert_ne!(first.lease_id, second.lease_id);
        let old_lease = store
            .consume_lease(&first.ssh_username, &first.ssh_password)
            .await
            .expect_err("old market lease should be invalidated");
        assert!(!old_lease.to_string().is_empty());
        assert!(
            proxy
                .backend_for_host("market-a.127.0.0.1:8787", &config.tunnel_domain)
                .await
                .is_none()
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_rejects_registered_market_subdomain() {
        let (store, config) = setup_store("market-subdomain-reserved").await;
        let market = test_market();
        insert_market(&store, &market).await;
        let signing_key = insert_signed_installation(&store, "inst-market-reserved").await;

        let share = ShareDescriptor {
            share_id: "share-market-reserved".into(),
            share_name: "Reserved".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            market_access_mode: "selected".into(),
            access_by_app: Default::default(),
            app_settings: Default::default(),
            for_sale_official_price_percent_by_app: Default::default(),
            description: None,
            for_sale: "No".into(),
            sale_market_kind: "token".into(),
            subdomain: "market-a".into(),
            app_type: "codex".into(),
            provider_id: Some("provider-market".into()),
            bindings: BTreeMap::from([("codex".into(), "provider-market".into())]),
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "active".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::days(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            app_providers: ShareAppProviders::default(),
            market_grant: None,
            app_availability: ShareAppAvailability::default(),
            model_health: ShareModelHealthSummary::default(),
        };
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-market-reserved",
            "share_claim_subdomain",
            &share,
            timestamp_ms,
            &nonce,
        );

        let err = store
            .claim_share_subdomain(
                &config,
                ShareClaimSubdomainRequest {
                    installation_id: "inst-market-reserved".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    claim: None,
                    share,
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("market subdomain should be reserved");
        assert!(err.to_string().contains("subdomain is reserved"));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_accepts_valid_signature_and_rejects_replay_and_tamper() {
        let (store, config) = setup_store("signed-share-claim").await;
        let signing_key = insert_signed_installation(&store, "inst-signed").await;

        let share = ShareDescriptor {
            share_id: "share-1".into(),
            share_name: "Signed Share".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            market_access_mode: "selected".into(),
            access_by_app: Default::default(),
            app_settings: Default::default(),
            for_sale_official_price_percent_by_app: Default::default(),
            description: None,
            for_sale: "No".into(),
            sale_market_kind: "token".into(),
            subdomain: "signed-sub".into(),
            app_type: "codex".into(),
            provider_id: Some("provider-signed".into()),
            bindings: BTreeMap::from([("codex".into(), "provider-signed".into())]),
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "paused".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            app_providers: ShareAppProviders::default(),
            market_grant: None,
            app_availability: ShareAppAvailability::default(),
            model_health: ShareModelHealthSummary::default(),
        };

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-signed",
            "share_claim_subdomain",
            &share,
            timestamp_ms,
            &nonce,
        );

        let request = ShareClaimSubdomainRequest {
            installation_id: "inst-signed".into(),
            timestamp_ms,
            nonce: nonce.clone(),
            signature: signature.clone(),
            claim: None,
            share: share.clone(),
        };

        store
            .claim_share_subdomain(
                &config,
                request,
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("valid signed share claim");

        let replay_err = store
            .claim_share_subdomain(
                &config,
                ShareClaimSubdomainRequest {
                    installation_id: "inst-signed".into(),
                    timestamp_ms,
                    nonce: nonce.clone(),
                    signature: signature.clone(),
                    claim: None,
                    share: share.clone(),
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("replay should fail");
        assert!(replay_err.to_string().contains("nonce already used"));

        let tampered_share = ShareDescriptor {
            subdomain: "signed-sub-tampered".into(),
            ..share
        };
        let tampered_err = store
            .claim_share_subdomain(
                &config,
                ShareClaimSubdomainRequest {
                    installation_id: "inst-signed".into(),
                    timestamp_ms: Utc::now().timestamp_millis(),
                    nonce: Uuid::new_v4().to_string(),
                    signature,
                    claim: None,
                    share: tampered_share,
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("tampered payload should fail");
        assert!(
            tampered_err
                .to_string()
                .contains("signature verification failed")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn bind_installation_owner_email_accepts_authenticated_session() {
        let (store, config) = setup_store("bind-owner-session").await;
        let installation_id = "inst-bind-session";
        let signing_key = insert_signed_installation(&store, installation_id).await;
        let email = "owner@example.com";
        let access_token = "access-token-for-owner-binding";
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE installations SET owner_email = NULL, owner_verified_at = NULL WHERE id = ?1",
                params![installation_id],
            )
            .expect("clear owner");
            let user = upsert_user_by_email(&conn, email, now).expect("upsert user");
            persist_session(
                &conn,
                &AuthSession {
                    session_id: Uuid::new_v4().to_string(),
                    user_id: user.id,
                    email: email.into(),
                    installation_id: installation_id.into(),
                    access_token_hash: hash_token(access_token),
                    refresh_token_hash: hash_token("refresh-token-for-owner-binding"),
                    access_expires_at: now + Duration::hours(1),
                    refresh_expires_at: now + Duration::days(1),
                    created_at: now,
                    last_used_at: now,
                },
            )
            .expect("persist session");
        }

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let payload = BindOwnerEmailSignaturePayload {
            email,
            verification_token: None,
        };
        let signature = sign_test_payload(
            &signing_key,
            installation_id,
            "bind_installation_owner_email",
            &payload,
            timestamp_ms,
            &nonce,
        );

        let response = store
            .bind_installation_owner_email(
                &config,
                BindInstallationOwnerEmailRequest {
                    installation_id: installation_id.into(),
                    email: email.into(),
                    verification_token: None,
                    timestamp_ms,
                    nonce,
                    signature,
                },
                Some(access_token),
            )
            .await
            .expect("bind owner with session");

        assert!(response.ok);
        assert_eq!(response.owner_email, email);
        assert!(!response.already_bound);
        let conn = store.conn.lock().await;
        assert_eq!(
            get_installation_owner_email(&conn, installation_id).expect("owner email"),
            Some(email.into())
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn client_tunnel_cannot_change_installation_owner() {
        let (store, config) = setup_store("client-tunnel-owner-gate").await;
        let installation_id = "inst-client-tunnel-owner";
        let signing_key = insert_signed_installation(&store, installation_id).await;
        let tunnel = ClientTunnelConfig {
            owner_email: "other@example.com".into(),
            subdomain: "owner-gate-client".into(),
            enabled: true,
        };
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            installation_id,
            "client_tunnel_claim",
            &tunnel,
            timestamp_ms,
            &nonce,
        );

        let error = store
            .claim_client_tunnel(
                &config,
                ClientTunnelClaimRequest {
                    installation_id: installation_id.into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    tunnel,
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .expect_err("client tunnel owner mismatch must be rejected");
        assert!(
            error
                .to_string()
                .contains("client tunnel owner must match the installation owner")
        );
        let conn = store.conn.lock().await;
        assert_eq!(
            get_installation_owner_email(&conn, installation_id).expect("installation owner"),
            Some("owner@example.com".into())
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn change_installation_owner_email_uses_new_owner_session() {
        let (store, config) = setup_store("change-owner-session").await;
        let installation_id = "inst-change-owner";
        let signing_key = insert_signed_installation(&store, installation_id).await;
        insert_share(
            &store,
            installation_id,
            "share-change-owner",
            "change-owner",
            "paused",
        )
        .await;
        insert_share(
            &store,
            installation_id,
            "share-divergent-owner",
            "divergent-owner",
            "paused",
        )
        .await;
        let old_email = "owner@example.com";
        let new_email = "new-owner@example.com";
        let access_token = "access-token-for-change-owner";
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET owner_email = 'historical@example.com', shared_with_emails_json = '[]'
                 WHERE share_id = 'share-divergent-owner'",
                [],
            )
            .expect("seed divergent share owner");
            let user = upsert_user_by_email(&conn, new_email, now).expect("upsert user");
            persist_session(
                &conn,
                &AuthSession {
                    session_id: Uuid::new_v4().to_string(),
                    user_id: user.id,
                    email: new_email.into(),
                    installation_id: installation_id.into(),
                    access_token_hash: hash_token(access_token),
                    refresh_token_hash: hash_token("refresh-token-for-change-owner"),
                    access_expires_at: now + Duration::hours(1),
                    refresh_expires_at: now + Duration::days(1),
                    created_at: now,
                    last_used_at: now,
                },
            )
            .expect("persist session");
        }

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let payload = ChangeOwnerEmailSignaturePayload {
            old_email,
            new_email,
        };
        let signature = sign_test_payload(
            &signing_key,
            installation_id,
            "change_installation_owner_email",
            &payload,
            timestamp_ms,
            &nonce,
        );

        let response = store
            .change_installation_owner_email(
                ChangeInstallationOwnerEmailRequest {
                    installation_id: installation_id.into(),
                    old_email: old_email.into(),
                    new_email: new_email.into(),
                    timestamp_ms,
                    nonce,
                    signature,
                },
                Some(access_token),
            )
            .await
            .expect("change owner email");

        assert!(response.ok);
        assert_eq!(response.old_email, old_email);
        assert_eq!(response.new_email, new_email);
        assert_eq!(response.updated_shares, 2);
        let conn = store.conn.lock().await;
        assert_eq!(
            get_installation_owner_email(&conn, installation_id).expect("owner email"),
            Some(new_email.into())
        );
        assert_eq!(
            get_share_owner_email(&conn, "share-change-owner").expect("share owner"),
            Some(new_email.into())
        );
        assert_eq!(
            get_share_owner_email(&conn, "share-divergent-owner").expect("divergent share owner"),
            Some(new_email.into())
        );
        let shared_json: String = conn
            .query_row(
                "SELECT shared_with_emails_json FROM shares WHERE share_id = 'share-divergent-owner'",
                [],
                |row| row.get(0),
            )
            .expect("read migrated share ACL");
        let shared: Vec<String> = serde_json::from_str(&shared_json).expect("parse migrated ACL");
        assert_eq!(shared, vec!["historical@example.com".to_string()]);
        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_accepts_minimal_claim_signature() {
        let (store, config) = setup_store("signed-share-minimal-claim").await;
        let signing_key = insert_signed_installation(&store, "inst-minimal").await;

        let share = ShareDescriptor {
            share_id: "share-minimal".into(),
            share_name: "Signed Share".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            market_access_mode: "all".into(),
            access_by_app: Default::default(),
            app_settings: Default::default(),
            for_sale_official_price_percent_by_app: std::collections::BTreeMap::from([
                ("claude".to_string(), 5),
                ("codex".to_string(), 5),
            ]),
            description: Some("metadata outside claim signature".into()),
            for_sale: "Yes".into(),
            sale_market_kind: "token".into(),
            subdomain: "minimal-sub".into(),
            app_type: "codex".into(),
            provider_id: Some("provider-minimal".into()),
            bindings: BTreeMap::from([("codex".into(), "provider-minimal".into())]),
            token_limit: -1,
            parallel_limit: -1,
            tokens_used: 0,
            requests_count: 0,
            share_status: "active".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            app_providers: ShareAppProviders::default(),
            market_grant: None,
            app_availability: ShareAppAvailability::default(),
            model_health: ShareModelHealthSummary::default(),
        };
        let claim = share_claim_payload(&share);
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-minimal",
            "share_claim_subdomain",
            &claim,
            timestamp_ms,
            &nonce,
        );

        store
            .claim_share_subdomain(
                &config,
                ShareClaimSubdomainRequest {
                    installation_id: "inst-minimal".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    claim: Some(claim),
                    share: share.clone(),
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("minimal claim signature should be accepted");

        let conn = store.conn.lock().await;
        let synced: (String, String, String) = conn
            .query_row(
                "SELECT share_name, subdomain, for_sale FROM shares
                 WHERE installation_id = ?1 AND share_id = ?2",
                params!["inst-minimal", "share-minimal"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("query minimal claim share");
        assert_eq!(synced.0, "owner@example.com");
        assert_eq!(synced.1, "minimal-sub");
        assert_eq!(synced.2, "Yes");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_rejects_claim_share_mismatch() {
        let (store, config) = setup_store("signed-share-claim-mismatch").await;
        let signing_key = insert_signed_installation(&store, "inst-mismatch").await;

        let share = ShareDescriptor {
            share_id: "share-mismatch".into(),
            share_name: "Signed Share".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            market_access_mode: "selected".into(),
            access_by_app: Default::default(),
            app_settings: Default::default(),
            for_sale_official_price_percent_by_app: Default::default(),
            description: None,
            for_sale: "No".into(),
            sale_market_kind: "token".into(),
            subdomain: "mismatch-sub".into(),
            app_type: "codex".into(),
            provider_id: Some("provider-mismatch".into()),
            bindings: BTreeMap::from([("codex".into(), "provider-mismatch".into())]),
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "active".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            app_providers: ShareAppProviders::default(),
            market_grant: None,
            app_availability: ShareAppAvailability::default(),
            model_health: ShareModelHealthSummary::default(),
        };
        let claim = ShareClaimPayload {
            subdomain: "other-sub".into(),
            ..share_claim_payload(&share)
        };
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-mismatch",
            "share_claim_subdomain",
            &claim,
            timestamp_ms,
            &nonce,
        );

        let err = store
            .claim_share_subdomain(
                &config,
                ShareClaimSubdomainRequest {
                    installation_id: "inst-mismatch".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    claim: Some(claim),
                    share,
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("claim/share mismatch should fail");
        assert!(err.to_string().contains("claim does not match"));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_rejects_same_owner_reclaim_from_different_installation() {
        let (store, config) = setup_store("signed-share-reject-owner-reclaim").await;
        insert_share(&store, "inst-old", "share-old", "owner-sub", "paused").await;
        let signing_key = insert_signed_installation(&store, "inst-new").await;

        let share = ShareDescriptor {
            share_id: "share-new".into(),
            share_name: "owner@example.com".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            market_access_mode: "selected".into(),
            access_by_app: Default::default(),
            app_settings: Default::default(),
            for_sale_official_price_percent_by_app: Default::default(),
            description: None,
            for_sale: "No".into(),
            sale_market_kind: "token".into(),
            subdomain: "owner-sub".into(),
            app_type: "codex".into(),
            provider_id: Some("provider-owner".into()),
            bindings: BTreeMap::from([("codex".into(), "provider-owner".into())]),
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "paused".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            app_providers: ShareAppProviders::default(),
            market_grant: None,
            app_availability: ShareAppAvailability::default(),
            model_health: ShareModelHealthSummary::default(),
        };

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-new",
            "share_claim_subdomain",
            &share,
            timestamp_ms,
            &nonce,
        );

        let err = store
            .claim_share_subdomain(
                &config,
                ShareClaimSubdomainRequest {
                    installation_id: "inst-new".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    claim: None,
                    share: share.clone(),
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("different installation must not reclaim by self-reported owner email");
        assert!(err.to_string().contains("subdomain already claimed"));

        let conn = store.conn.lock().await;
        let rows: Vec<(String, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT share_id, installation_id, subdomain
                     FROM shares
                     WHERE subdomain = 'owner-sub'",
                )
                .expect("prepare reclaimed subdomain query");
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .expect("query reclaimed subdomain rows")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect reclaimed subdomain rows")
        };
        assert_eq!(
            rows,
            vec![("share-old".into(), "inst-old".into(), "owner-sub".into())]
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_heals_stale_owner_for_same_installation() {
        let (store, config) = setup_store("signed-share-heal-owner").await;
        let signing_key = insert_signed_installation(&store, "inst-heal").await;
        insert_share(&store, "inst-heal", "share-heal", "heal-sub", "paused").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE installations SET owner_email = ?2 WHERE id = ?1",
                params!["inst-heal", "router@example.com"],
            )
            .expect("update installation owner");
            conn.execute(
                "UPDATE shares SET owner_email = ?2 WHERE share_id = ?1",
                params!["share-heal", "free@example.com"],
            )
            .expect("update stale share owner");
        }

        let share = ShareDescriptor {
            share_id: "share-heal".into(),
            share_name: "router@example.com".into(),
            owner_email: Some("router@example.com".into()),
            shared_with_emails: vec![],
            market_access_mode: "selected".into(),
            access_by_app: Default::default(),
            app_settings: Default::default(),
            for_sale_official_price_percent_by_app: Default::default(),
            description: None,
            for_sale: "No".into(),
            sale_market_kind: "token".into(),
            subdomain: "heal-sub".into(),
            app_type: "codex".into(),
            provider_id: Some("provider-heal".into()),
            bindings: BTreeMap::from([("codex".into(), "provider-heal".into())]),
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "active".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            app_providers: ShareAppProviders::default(),
            market_grant: None,
            app_availability: ShareAppAvailability::default(),
            model_health: ShareModelHealthSummary::default(),
        };

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-heal",
            "share_claim_subdomain",
            &share,
            timestamp_ms,
            &nonce,
        );

        store
            .claim_share_subdomain(
                &config,
                ShareClaimSubdomainRequest {
                    installation_id: "inst-heal".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    claim: None,
                    share,
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                "router@example.com",
            )
            .await
            .expect("claim heals stale owner");

        let conn = store.conn.lock().await;
        assert_eq!(
            get_share_owner_email(&conn, "share-heal").expect("share owner"),
            Some("router@example.com".into())
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn claim_share_subdomain_allows_same_installation_to_replace_deleted_share_claim() {
        let (store, config) = setup_store("signed-share-reclaim-installation").await;
        let signing_key = insert_signed_installation(&store, "inst-same").await;
        insert_share(&store, "inst-same", "share-old", "reused-sub", "paused").await;

        let share = ShareDescriptor {
            share_id: "share-new".into(),
            share_name: "owner@example.com".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            market_access_mode: "selected".into(),
            access_by_app: Default::default(),
            app_settings: Default::default(),
            for_sale_official_price_percent_by_app: Default::default(),
            description: None,
            for_sale: "No".into(),
            sale_market_kind: "token".into(),
            subdomain: "reused-sub".into(),
            app_type: "codex".into(),
            provider_id: Some("provider-reused".into()),
            bindings: BTreeMap::from([("codex".into(), "provider-reused".into())]),
            token_limit: 1000,
            parallel_limit: 3,
            tokens_used: 0,
            requests_count: 0,
            share_status: "paused".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(1)).to_rfc3339(),
            support: ShareSupport::default(),
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            app_providers: ShareAppProviders::default(),
            market_grant: None,
            app_availability: ShareAppAvailability::default(),
            model_health: ShareModelHealthSummary::default(),
        };

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-same",
            "share_claim_subdomain",
            &share,
            timestamp_ms,
            &nonce,
        );

        store
            .claim_share_subdomain(
                &config,
                ShareClaimSubdomainRequest {
                    installation_id: "inst-same".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    claim: None,
                    share: share.clone(),
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("claim reclaimed subdomain for same installation");

        let conn = store.conn.lock().await;
        let rows: Vec<(String, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT share_id, installation_id, subdomain
                     FROM shares
                     WHERE subdomain = 'reused-sub'",
                )
                .expect("prepare reclaimed installation subdomain query");
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .expect("query reclaimed installation subdomain rows")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect reclaimed installation subdomain rows")
        };
        assert_eq!(
            rows,
            vec![("share-new".into(), "inst-same".into(), "reused-sub".into())]
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn batch_sync_shares_requires_valid_signature() {
        let (store, config) = setup_store("signed-share-batch").await;
        let signing_key = insert_signed_installation(&store, "inst-batch").await;

        let share = ShareDescriptor {
            share_id: "share-batch-1".into(),
            share_name: "Batch Share".into(),
            owner_email: Some("owner@example.com".into()),
            shared_with_emails: vec![],
            market_access_mode: "all".into(),
            access_by_app: Default::default(),
            app_settings: Default::default(),
            for_sale_official_price_percent_by_app: std::collections::BTreeMap::from([
                ("claude".to_string(), 10),
                ("codex".to_string(), 20),
            ]),
            description: Some("signed batch sync".into()),
            for_sale: "Yes".into(),
            sale_market_kind: "token".into(),
            subdomain: "batch-sub".into(),
            app_type: "codex".into(),
            provider_id: Some("provider-batch".into()),
            bindings: BTreeMap::from([("codex".into(), "provider-batch".into())]),
            token_limit: 2048,
            parallel_limit: 3,
            tokens_used: 12,
            requests_count: 3,
            share_status: "active".into(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: (Utc::now() + Duration::hours(2)).to_rfc3339(),
            support: ShareSupport {
                claude: true,
                codex: true,
                gemini: false,
            },
            upstream_provider: None,
            app_runtimes: ShareAppRuntimes::default(),
            app_providers: ShareAppProviders::default(),
            market_grant: None,
            app_availability: ShareAppAvailability::default(),
            model_health: ShareModelHealthSummary::default(),
        };
        let ops = vec![ShareSyncOperation {
            kind: "upsert".into(),
            share: Some(share.clone()),
            share_id: None,
        }];

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-batch",
            "share_batch_sync",
            &ops,
            timestamp_ms,
            &nonce,
        );

        store
            .batch_sync_shares(
                ShareBatchSyncRequest {
                    installation_id: "inst-batch".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    ops,
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("valid signed batch sync");

        let conn = store.conn.lock().await;
        let synced: (String, String, i64, String, String) = conn
            .query_row(
                "SELECT share_name, subdomain, token_limit, for_sale, market_access_mode FROM shares
                 WHERE installation_id = ?1 AND share_id = ?2",
                params!["inst-batch", "share-batch-1"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .expect("query synced share");
        drop(conn);
        assert_eq!(synced.0, "owner@example.com");
        assert_eq!(synced.1, "batch-sub");
        assert_eq!(synced.2, 2048);
        assert_eq!(synced.3, "Yes");
        assert_eq!(synced.4, "all");

        let tampered_ops = vec![ShareSyncOperation {
            kind: "upsert".into(),
            share: Some(ShareDescriptor {
                share_name: "Batch Share Tampered".into(),
                ..share
            }),
            share_id: None,
        }];
        let tampered_err = store
            .batch_sync_shares(
                ShareBatchSyncRequest {
                    installation_id: "inst-batch".into(),
                    timestamp_ms: Utc::now().timestamp_millis(),
                    nonce: Uuid::new_v4().to_string(),
                    signature: sign_test_payload(
                        &signing_key,
                        "inst-batch",
                        "share_batch_sync",
                        &vec![ShareSyncOperation {
                            kind: "upsert".into(),
                            share: Some(ShareDescriptor {
                                share_name: "Different".into(),
                                ..tampered_ops[0].share.clone().expect("share")
                            }),
                            share_id: None,
                        }],
                        Utc::now().timestamp_millis(),
                        &Uuid::new_v4().to_string(),
                    ),
                    ops: tampered_ops,
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect_err("tampered batch sync should fail");
        assert!(
            tampered_err
                .to_string()
                .contains("signature verification failed")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn upsert_share_requires_one_matching_provider_binding() {
        let (store, config) = setup_store("share-single-binding-validation").await;
        let conn = store.conn.lock().await;

        let mut missing = test_share_descriptor("share-missing", "missing-sub");
        missing.bindings.clear();
        let missing_error = upsert_share_tx(&conn, "inst-binding", missing).unwrap_err();
        assert!(
            missing_error
                .to_string()
                .contains("exactly one binding matching appType/providerId")
        );

        let mut multiple = test_share_descriptor("share-multiple", "multiple-sub");
        multiple
            .bindings
            .insert("claude".into(), "provider-claude".into());
        let multiple_error = upsert_share_tx(&conn, "inst-binding", multiple).unwrap_err();
        assert!(
            multiple_error
                .to_string()
                .contains("exactly one binding matching appType/providerId")
        );

        let mut mismatched = test_share_descriptor("share-mismatch", "mismatch-sub");
        mismatched.provider_id = Some("different-provider".into());
        let mismatch_error = upsert_share_tx(&conn, "inst-binding", mismatched).unwrap_err();
        assert!(
            mismatch_error
                .to_string()
                .contains("exactly one binding matching appType/providerId")
        );

        drop(conn);
        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn upsert_share_preserves_other_installation_shares_and_logs() {
        let (store, config) = setup_store("multi-share-upsert-preserve").await;
        insert_installation(&store, "inst-clean").await;
        insert_share(&store, "inst-clean", "share-old", "old-sub", "active").await;
        insert_health_check(&store, "share-old", Utc::now().timestamp(), true).await;

        {
            let conn = store.conn.lock().await;
            upsert_share_request_log_tx(
                &conn,
                "inst-clean",
                ShareRequestLogEntry {
                    request_id: "req-old-share-log".into(),
                    share_id: "share-old".into(),
                    share_name: "Old Share".into(),
                    provider_id: "provider-1".into(),
                    provider_name: "Provider One".into(),
                    app_type: "codex".into(),
                    model: "gpt-5".into(),
                    request_model: "gpt-5".into(),
                    request_agent: "codex".into(),
                    requested_model: "gpt-5".into(),
                    actual_model: "gpt-5".into(),
                    actual_model_source: "official".into(),
                    status_code: 200,
                    latency_ms: 100,
                    first_token_ms: None,
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    is_streaming: false,
                    session_id: None,
                    user_country: None,
                    user_country_iso3: None,
                    user_email: None,
                    created_at: Utc::now().timestamp(),
                    is_health_check: false,
                },
            )
            .expect("insert old share request log");
            let market = MarketRegistryRecord {
                id: "market-1".into(),
                display_name: "Market".into(),
                email: "market@example.com".into(),
                subdomain: "market".into(),
                public_base_url: "https://market.example.com".into(),
                market_kind: "usage".into(),
                scopes: vec![],
                status: "active".into(),
                maintenance_enabled: false,
                maintenance_message: None,
            };
            upsert_market_request_log_tx(
                &conn,
                &market,
                test_market_request_log("req_old_market_log", "share-old"),
            )
            .expect("insert old market request log");
            upsert_share_tx(
                &conn,
                "inst-clean",
                test_share_descriptor("share-new", "new-sub"),
            )
            .expect("upsert new share");
        }

        let conn = store.conn.lock().await;
        let old_share_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE installation_id='inst-clean' AND share_id='share-old'",
                [],
                |row| row.get(0),
            )
            .expect("count old shares");
        let new_share_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE installation_id='inst-clean' AND share_id='share-new'",
                [],
                |row| row.get(0),
            )
            .expect("count new shares");
        let old_share_logs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_request_logs WHERE share_id='share-old'",
                [],
                |row| row.get(0),
            )
            .expect("count old share logs");
        let old_health: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_health_checks WHERE share_id='share-old'",
                [],
                |row| row.get(0),
            )
            .expect("count old health");
        let old_market_logs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_request_logs WHERE share_id='share-old'",
                [],
                |row| row.get(0),
            )
            .expect("count old market logs");
        assert_eq!(old_share_count, 1);
        assert_eq!(new_share_count, 1);
        assert_eq!(old_share_logs, 1);
        assert_eq!(old_health, 1);
        assert_eq!(old_market_logs, 1);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn batch_sync_shares_accepts_multiple_upserts_for_one_installation() {
        let (store, config) = setup_store("batch-multiple-upsert").await;
        let signing_key = insert_signed_installation(&store, "inst-multi").await;
        let ops = vec![
            ShareSyncOperation {
                kind: "upsert".into(),
                share: Some(test_share_descriptor("share-one", "one-sub")),
                share_id: None,
            },
            ShareSyncOperation {
                kind: "upsert".into(),
                share: Some(test_share_descriptor("share-two", "two-sub")),
                share_id: None,
            },
        ];
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-multi",
            "share_batch_sync",
            &ops,
            timestamp_ms,
            &nonce,
        );

        store
            .batch_sync_shares(
                ShareBatchSyncRequest {
                    installation_id: "inst-multi".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    ops,
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
                "owner@example.com",
            )
            .await
            .expect("multiple independent share upserts should succeed");

        let conn = store.conn.lock().await;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE installation_id='inst-multi'",
                [],
                |row| row.get(0),
            )
            .expect("count reconciled shares");
        assert_eq!(count, 2);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn batch_sync_share_request_logs_requires_valid_signature() {
        let (store, config) = setup_store("signed-log-batch").await;
        let signing_key = insert_signed_installation(&store, "inst-logs").await;
        insert_share(&store, "inst-logs", "share-log-1", "log-sub", "active").await;

        let logs = vec![ShareRequestLogEntry {
            request_id: "req-1".into(),
            share_id: "share-log-1".into(),
            share_name: "Log Share".into(),
            provider_id: "provider-1".into(),
            provider_name: "Provider One".into(),
            app_type: "codex".into(),
            model: "gpt-5".into(),
            request_model: "gpt-5".into(),
            request_agent: "codex".into(),
            requested_model: "gpt-5".into(),
            actual_model: "gpt-5".into(),
            actual_model_source: "official".into(),
            status_code: 200,
            latency_ms: 1234,
            first_token_ms: Some(222),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            is_streaming: true,
            session_id: Some("session-1".into()),
            user_country: None,
            user_country_iso3: None,
            user_email: None,
            created_at: Utc::now().timestamp(),
            is_health_check: false,
        }];

        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-logs",
            "share_request_logs_batch_sync",
            &logs,
            timestamp_ms,
            &nonce,
        );

        let live_request_context_by_id = HashMap::from([(
            "req-1".to_string(),
            (
                Some("JP".into()),
                Some("JPN".into()),
                Some("user@example.com".into()),
            ),
        )]);
        store
            .batch_sync_share_request_logs(
                ShareRequestLogBatchSyncRequest {
                    installation_id: "inst-logs".into(),
                    timestamp_ms,
                    nonce: nonce.clone(),
                    signature: signature.clone(),
                    logs: logs.clone(),
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
                live_request_context_by_id,
            )
            .await
            .expect("valid signed request log batch sync");

        let conn = store.conn.lock().await;
        let (stored_count, user_country, user_country_iso3, user_email): (
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT COUNT(*), MAX(user_country), MAX(user_country_iso3), MAX(user_email) FROM share_request_logs
                 WHERE installation_id = ?1 AND request_id = ?2",
                params!["inst-logs", "req-1"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("count synced request logs");
        drop(conn);
        assert_eq!(stored_count, 1);
        assert_eq!(user_country.as_deref(), Some("JP"));
        assert_eq!(user_country_iso3.as_deref(), Some("JPN"));
        assert_eq!(user_email.as_deref(), Some("user@example.com"));

        let replay_err = store
            .batch_sync_share_request_logs(
                ShareRequestLogBatchSyncRequest {
                    installation_id: "inst-logs".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    logs: logs.clone(),
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
                HashMap::new(),
            )
            .await
            .expect_err("replayed log sync should fail");
        assert!(replay_err.to_string().contains("nonce already used"));

        let tampered_logs = vec![ShareRequestLogEntry {
            status_code: 500,
            ..logs[0].clone()
        }];
        let bad_signature = sign_test_payload(
            &signing_key,
            "inst-logs",
            "share_request_logs_batch_sync",
            &logs,
            Utc::now().timestamp_millis(),
            &Uuid::new_v4().to_string(),
        );
        let tampered_err = store
            .batch_sync_share_request_logs(
                ShareRequestLogBatchSyncRequest {
                    installation_id: "inst-logs".into(),
                    timestamp_ms: Utc::now().timestamp_millis(),
                    nonce: Uuid::new_v4().to_string(),
                    signature: bad_signature,
                    logs: tampered_logs,
                },
                ClientMetadata {
                    ip: Some("127.0.0.1".into()),
                    country_code: None,
                },
                "owner@example.com",
                HashMap::new(),
            )
            .await
            .expect_err("tampered log sync should fail");
        assert!(
            tampered_err
                .to_string()
                .contains("signature verification failed")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_snapshot_does_not_count_paused_share_as_active() {
        let (store, config) = setup_store("dashboard-paused").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-paused", "paused-sub", "paused").await;

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");

        assert_eq!(snapshot.stats.clients, 1);
        assert_eq!(snapshot.stats.active_shares, 0);
        assert_eq!(snapshot.stats.total_active_requests, 0);
        assert_eq!(snapshot.clients.len(), 1);
        assert_eq!(
            snapshot.shares.first().expect("share view").share_status,
            "paused"
        );
        assert_eq!(
            snapshot
                .shares
                .first()
                .expect("share view")
                .operational_summary
                .state,
            "disabled"
        );
        assert_eq!(snapshot.clients[0].operational_summary.state, "online");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_operational_summary_marks_missing_active_route_offline() {
        let (store, config) = setup_store("dashboard-operational-offline").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-offline", "offline-sub", "active").await;

        let snapshot = store
            .dashboard_snapshot(
                &config,
                &ServerGeo {
                    lat: None,
                    lon: None,
                },
                &ProxyRegistry::default(),
                None,
            )
            .await
            .expect("dashboard snapshot");
        let share = snapshot.shares.first().expect("share");
        assert_eq!(share.operational_summary.state, "offline");
        assert_eq!(
            share
                .operational_summary
                .primary_reason
                .as_ref()
                .map(|reason| reason.code.as_str()),
            Some("route_offline")
        );
        assert_eq!(snapshot.clients[0].operational_summary.state, "offline");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn local_ux_telemetry_records_only_minimized_fields() {
        let (store, config) = setup_store("dashboard-ux-telemetry").await;
        store
            .record_dashboard_ux_event(
                DashboardUxEventRequest {
                    event_type: "dashboard_focus_set".into(),
                    source: Some("map".into()),
                    target_type: Some("client".into()),
                    step_count: Some(1),
                    elapsed_ms: Some(25),
                    keyboard: false,
                },
                7,
            )
            .await
            .expect("record UX event");
        let conn = store.conn.lock().await;
        let row: (String, Option<String>, Option<String>, i64) = conn
            .query_row(
                "SELECT event_type, source, target_type, keyboard FROM dashboard_ux_events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("UX event row");
        assert_eq!(
            row,
            (
                "dashboard_focus_set".into(),
                Some("map".into()),
                Some("client".into()),
                0
            )
        );
        drop(conn);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_snapshot_aggregates_inflight_requests_across_installation_shares() {
        let (store, config) = setup_store("dashboard-aggregate-inflight").await;
        insert_installation(&store, "inst-1").await;
        set_installation_country_code(&store, "inst-1", "JP").await;
        insert_share(&store, "inst-1", "share-old", "old-sub", "active").await;
        insert_share(&store, "inst-1", "share-new", "new-sub", "active").await;

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        proxy
            .set_route(
                "old-sub".into(),
                "http://127.0.0.1:1".into(),
                None,
                Some("share-old".into()),
                Some("Old Share".into()),
                false,
                -1,
                None,
            )
            .await;
        proxy
            .set_route(
                "new-sub".into(),
                "http://127.0.0.1:2".into(),
                None,
                Some("share-new".into()),
                Some("New Share".into()),
                false,
                -1,
                None,
            )
            .await;
        proxy.set_share_inflight_for_test("share-old", 2).await;
        proxy.set_share_inflight_for_test("share-new", 1).await;

        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");

        assert_eq!(snapshot.stats.clients, 1);
        assert_eq!(snapshot.stats.total_active_requests, 3);
        assert_eq!(snapshot.map.clients.len(), 1);
        assert_eq!(snapshot.map.clients[0].active_requests, 3);
        // P7 Step 2：取消"prefer 出一个代表 share"的合并；两个 share 都在 snapshot.shares 里。
        assert_eq!(snapshot.shares.len(), 2);
        let mut shares_ids: Vec<_> = snapshot.shares.iter().map(|s| s.share_id.clone()).collect();
        shares_ids.sort();
        assert_eq!(shares_ids, vec!["share-new", "share-old"]);
        assert_eq!(snapshot.clients[0].share_count, 2);
        let mut client_share_ids = snapshot.clients[0].share_ids.clone();
        client_share_ids.sort();
        assert_eq!(client_share_ids, vec!["share-new", "share-old"]);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_snapshot_uses_client_tunnel_health_when_installation_has_no_shares() {
        let (store, config) = setup_store("dashboard-client-tunnel-health").await;
        insert_installation(&store, "inst-client").await;
        insert_client_tunnel(&store, "inst-client", "owner@example.com", "client-sub").await;
        let now = Utc::now();
        for minute_offset in 0..30 {
            insert_installation_health_check(
                &store,
                "inst-client",
                now.timestamp() - minute_offset * 60,
                true,
            )
            .await;
        }

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        proxy
            .set_route(
                "client-sub".into(),
                "http://127.0.0.1:15721".into(),
                None,
                None,
                None,
                false,
                -1,
                None,
            )
            .await;

        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");

        assert_eq!(snapshot.clients.len(), 1);
        let client = &snapshot.clients[0];
        assert_eq!(client.share_count, 0);
        assert!(client.share_ids.is_empty());
        assert!(
            client
                .client_tunnel
                .as_ref()
                .is_some_and(|tunnel| tunnel.online)
        );
        assert_eq!(client.online_minutes_24h, 30);
        assert!(
            (client.online_rate_24h - (30.0 / ONLINE_WINDOW_MINUTES as f64 * 100.0)).abs() < 0.01
        );
        assert_eq!(client.health_checks.len(), 10);
        assert!(client.health_checks.iter().all(|entry| entry.is_healthy));
        assert_eq!(client.health_timeline.len(), HEALTH_TIMELINE_BUCKETS);
        let latest = client.health_timeline.last().expect("latest bucket");
        assert_eq!(latest.status, "healthy");
        assert_eq!(latest.online_minutes, 30);
        assert!(
            client
                .health_timeline
                .iter()
                .filter(|bucket| bucket.status == "healthy")
                .count()
                >= 1
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn public_map_points_returns_total_client_count_alongside_deduplicated_points() {
        let (store, config) = setup_store("public-map-client-count").await;
        for installation_id in ["inst-1", "inst-2", "inst-3"] {
            insert_installation(&store, installation_id).await;
            set_installation_country_code(&store, installation_id, "JP").await;
        }
        insert_share(&store, "inst-1", "share-1", "sub-1", "active").await;
        insert_share(&store, "inst-2", "share-2", "sub-2", "active").await;
        insert_share(&store, "inst-3", "share-3", "sub-3", "active").await;

        let server_geo = ServerGeo {
            lat: Some(35.6895),
            lon: Some(139.692),
        };
        let points = store
            .public_map_points(&server_geo)
            .await
            .expect("public map points");

        assert_eq!(points.client_count, 3);
        assert_eq!(points.clients.len(), 1);
        assert_eq!(points.clients[0].lat, 36.2);
        assert_eq!(points.clients[0].lon, 138.25);
        assert_eq!(points.clients[0].count, 3);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_snapshot_exposes_market_links_without_acl_emails() {
        let (store, config) = setup_store("dashboard-market-links").await;
        let market = test_market();
        insert_market(&store, &market).await;
        insert_installation(&store, "inst-1").await;
        insert_share(
            &store,
            "inst-1",
            "share-market",
            "market-share-sub",
            "active",
        )
        .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET for_sale = 'Yes', shared_with_emails_json = ?2 WHERE share_id = ?1",
                params![
                    "share-market",
                    serde_json::to_string(&vec![market.email.clone()]).expect("emails json")
                ],
            )
            .expect("authorize market");
        }

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");
        let share = snapshot.shares.first().expect("share view");

        assert!(share.shared_with_emails.is_empty());
        assert_eq!(share.market_links.len(), 1);
        assert_eq!(
            share.market_links[0].display_name,
            "https://market-a.example.com"
        );
        assert_eq!(share.market_links[0].email, "market@example.com");
        assert!(share.unknown_market_emails.is_empty());

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn shared_email_can_view_share_read_only_in_dashboard() {
        let (store, config) = setup_store("dashboard-shared-read-only").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-shared", "shared-sub", "active").await;
        set_share_shared_with_emails(&store, "share-shared", &["shared@example.com"]).await;

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, Some("shared@example.com"))
            .await
            .expect("dashboard snapshot for shared viewer");
        let share = snapshot.shares.first().expect("share view");

        assert!(!share.can_manage);
        assert!(!share.can_edit_settings);
        assert_eq!(share.shared_with_emails, vec!["shared@example.com"]);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn shared_email_cannot_create_share_settings_edit() {
        let (store, config) = setup_store("shared-share-settings-edit").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-edit", "edit-sub", "active").await;
        set_share_shared_with_emails(&store, "share-edit", &["shared@example.com"]).await;

        let err = store
            .create_share_settings_edit(
                "share-edit",
                "shared@example.com",
                ShareSettingsPatch {
                    description: Some(Some("updated by shared user".into())),
                    shared_with_emails: Some(vec![
                        "owner@example.com".into(),
                        "shared@example.com".into(),
                        "other@example.com".into(),
                    ]),
                    ..ShareSettingsPatch::default()
                },
            )
            .await
            .expect_err("shared email should not create edit");

        assert!(err.to_string().contains("only share owner"));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn owner_cannot_patch_share_owner_even_when_target_is_shared() {
        let (store, config) = setup_store("share-settings-owner-transfer").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-edit", "edit-sub", "active").await;
        set_share_shared_with_emails(
            &store,
            "share-edit",
            &["shared@example.com", "other@example.com"],
        )
        .await;

        let error = store
            .create_share_settings_edit(
                "share-edit",
                "owner@example.com",
                ShareSettingsPatch {
                    owner_email: Some("shared@example.com".into()),
                    ..ShareSettingsPatch::default()
                },
            )
            .await
            .expect_err("share owner patch must be rejected");
        assert!(
            error
                .to_string()
                .contains("share owner is managed by the installation owner")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn owner_cannot_patch_share_owner_to_unshared_email() {
        let (store, config) = setup_store("share-settings-owner-transfer-unshared").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-edit", "edit-sub", "active").await;
        set_share_shared_with_emails(&store, "share-edit", &["shared@example.com"]).await;

        let err = store
            .create_share_settings_edit(
                "share-edit",
                "owner@example.com",
                ShareSettingsPatch {
                    owner_email: Some("stranger@example.com".into()),
                    ..ShareSettingsPatch::default()
                },
            )
            .await
            .expect_err("share owner patch must be rejected");

        assert!(
            err.to_string()
                .contains("share owner is managed by the installation owner")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn share_edit_pending_state_tracks_ack() {
        let (store, config) = setup_store("share-edit-pending-state").await;
        let signing_key = insert_signed_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-edit", "edit-sub", "active").await;

        let response = store
            .create_share_settings_edit(
                "share-edit",
                "owner@example.com",
                ShareSettingsPatch {
                    description: Some(Some("pending state test".into())),
                    ..ShareSettingsPatch::default()
                },
            )
            .await
            .expect("owner can create edit");

        assert!(
            store
                .is_share_edit_pending(&response.edit.id, response.edit.revision)
                .await
                .expect("query pending state")
        );

        let ack = ShareEditAckPayload {
            edit_id: response.edit.id.clone(),
            revision: response.edit.revision,
            status: "applied".into(),
            error_message: None,
        };
        let timestamp_ms = Utc::now().timestamp_millis();
        let nonce = Uuid::new_v4().to_string();
        let signature = sign_test_payload(
            &signing_key,
            "inst-1",
            "share_edit_ack",
            &ack,
            timestamp_ms,
            &nonce,
        );

        store
            .ack_share_edit(
                ShareEditAckRequest {
                    installation_id: "inst-1".into(),
                    timestamp_ms,
                    nonce,
                    signature,
                    ack,
                },
                ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .expect("ack edit");

        assert!(
            !store
                .is_share_edit_pending(&response.edit.id, response.edit.revision)
                .await
                .expect("query pending state after ack")
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn unshared_email_cannot_create_share_settings_edit() {
        let (store, config) = setup_store("unshared-share-settings-edit").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-edit", "edit-sub", "active").await;
        set_share_shared_with_emails(&store, "share-edit", &["shared@example.com"]).await;

        let err = store
            .create_share_settings_edit(
                "share-edit",
                "stranger@example.com",
                ShareSettingsPatch {
                    description: Some(Some("unauthorized update".into())),
                    ..ShareSettingsPatch::default()
                },
            )
            .await
            .expect_err("unshared email should be rejected");

        assert!(err.to_string().contains("only share owner"));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn cleanup_keeps_stale_clients_with_active_routes() {
        let (store, config) = setup_store("cleanup-stale-client").await;
        insert_installation(&store, "inst-stale").await;
        insert_installation(&store, "inst-fresh").await;
        insert_share(&store, "inst-stale", "share-stale", "stale-sub", "active").await;
        insert_share(&store, "inst-fresh", "share-fresh", "fresh-sub", "active").await;
        mark_installation_last_seen(&store, "inst-stale", Utc::now() - Duration::hours(2)).await;

        let proxy = ProxyRegistry::default();
        proxy
            .set_route(
                "stale-sub".into(),
                "127.0.0.1:1234".into(),
                None,
                None,
                None,
                false,
                -1,
                None,
            )
            .await;
        proxy
            .set_route(
                "fresh-sub".into(),
                "127.0.0.1:5678".into(),
                None,
                None,
                None,
                false,
                -1,
                None,
            )
            .await;

        let result = store
            .cleanup_expired_data(&config, &proxy)
            .await
            .expect("cleanup stale client");

        assert_eq!(result.deleted_installations, 0);
        assert_eq!(result.deleted_shares, 0);
        assert_eq!(result.removed_routes, 0);

        let conn = store.conn.lock().await;
        let stale_installations: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM installations WHERE id = 'inst-stale'",
                [],
                |row| row.get(0),
            )
            .expect("count stale installations");
        let stale_shares: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE share_id = 'share-stale'",
                [],
                |row| row.get(0),
            )
            .expect("count stale shares");
        let fresh_shares: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE share_id = 'share-fresh'",
                [],
                |row| row.get(0),
            )
            .expect("count fresh shares");
        drop(conn);

        assert_eq!(stale_installations, 1);
        assert_eq!(stale_shares, 1);
        assert_eq!(fresh_shares, 1);
        let active_subdomains = proxy.active_subdomains().await;
        assert!(active_subdomains.contains(&"stale-sub".to_string()));
        assert!(active_subdomains.contains(&"fresh-sub".to_string()));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn cleanup_removes_stale_clients_without_active_routes() {
        let (store, config) = setup_store("cleanup-stale-client-no-route").await;
        insert_installation(&store, "inst-stale").await;
        insert_installation(&store, "inst-fresh").await;
        insert_share(&store, "inst-stale", "share-stale", "stale-sub", "active").await;
        insert_share(&store, "inst-fresh", "share-fresh", "fresh-sub", "active").await;
        mark_installation_last_seen(&store, "inst-stale", Utc::now() - Duration::hours(2)).await;

        let proxy = ProxyRegistry::default();
        proxy
            .set_route(
                "fresh-sub".into(),
                "127.0.0.1:5678".into(),
                None,
                None,
                None,
                false,
                -1,
                None,
            )
            .await;

        let result = store
            .cleanup_expired_data(&config, &proxy)
            .await
            .expect("cleanup stale client");

        assert_eq!(result.deleted_installations, 0);
        assert_eq!(result.deleted_shares, 1);
        assert_eq!(result.removed_routes, 1);

        let conn = store.conn.lock().await;
        let stale_shares: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE share_id = 'share-stale'",
                [],
                |row| row.get(0),
            )
            .expect("count stale shares");
        let fresh_shares: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM shares WHERE share_id = 'share-fresh'",
                [],
                |row| row.get(0),
            )
            .expect("count fresh shares");
        drop(conn);

        assert_eq!(stale_shares, 0);
        assert_eq!(fresh_shares, 1);
        let active_subdomains = proxy.active_subdomains().await;
        assert!(!active_subdomains.contains(&"stale-sub".to_string()));
        assert!(active_subdomains.contains(&"fresh-sub".to_string()));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn cleanup_removes_paused_shares_even_when_installation_is_fresh() {
        // 回归：当 installation 上还有活动 share（last_seen_at 被持续 bump）时，
        // 长期 paused 的 share 不应该跟着续命，必须按 paused_share_stale_secs 单独 GC。
        let (store, config) = setup_store("cleanup-paused-share").await;
        insert_installation(&store, "inst-1").await;
        // 一条活的 share 让 installation 永远不 stale；paused 的那条本应独立计时。
        insert_share(&store, "inst-1", "share-active", "active-sub", "active").await;
        insert_share(
            &store,
            "inst-1",
            "share-paused-old",
            "paused-old-sub",
            "paused",
        )
        .await;
        insert_share(
            &store,
            "inst-1",
            "share-paused-fresh",
            "paused-fresh-sub",
            "paused",
        )
        .await;
        insert_share(
            &store,
            "inst-1",
            "share-active-old",
            "active-old-sub",
            "active",
        )
        .await;

        // 把 share-paused-old 的 updated_at 推到 paused 阈值之外（默认 3600s + 余量）。
        // share-paused-fresh 和 active share 留在 now，验证窗口内的 share 不被误删。
        {
            let stale =
                (Utc::now() - Duration::seconds(config.paused_share_stale_secs + 600)).to_rfc3339();
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET updated_at = ?1 WHERE share_id = ?2",
                params![stale, "share-paused-old"],
            )
            .expect("backdate paused-old");
        }

        let proxy = ProxyRegistry::default();
        // active-sub 在 ProxyRegistry 中，installation.last_seen_at 会被 cleanup tick 续命。
        proxy
            .set_route(
                "active-sub".into(),
                "127.0.0.1:5678".into(),
                None,
                None,
                None,
                false,
                -1,
                None,
            )
            .await;

        let result = store
            .cleanup_expired_data(&config, &proxy)
            .await
            .expect("cleanup paused shares");

        // 只 share-paused-old 应该被删；installation/active/active-old/paused-fresh 全部留下。
        assert_eq!(result.deleted_shares, 1);
        let conn = store.conn.lock().await;
        let surviving: Vec<String> = conn
            .prepare("SELECT share_id FROM shares ORDER BY share_id")
            .expect("prepare surviving shares")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query surviving shares")
            .collect::<Result<_, _>>()
            .expect("collect surviving shares");
        drop(conn);

        assert_eq!(
            surviving,
            vec![
                "share-active".to_string(),
                "share-active-old".to_string(),
                "share-paused-fresh".to_string(),
            ],
            "only the long-paused share should be deleted",
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn cleanup_removes_stale_active_offline_share_even_when_installation_is_fresh() {
        // 回归：同一台 installation 下只要还有其它 share 在线，旧逻辑就会刷新
        // installation.last_seen_at，从而让已经掉线很久的 active share 一直留在表里。
        // active share 也必须按自己的 subdomain/updated_at 独立 GC。
        let (store, config) = setup_store("cleanup-active-offline-share").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-online", "online-sub", "active").await;
        insert_share(
            &store,
            "inst-1",
            "share-offline-old",
            "offline-old-sub",
            "active",
        )
        .await;
        insert_share(
            &store,
            "inst-1",
            "share-offline-fresh",
            "offline-fresh-sub",
            "active",
        )
        .await;

        {
            let stale =
                (Utc::now() - Duration::seconds(config.client_stale_secs + 600)).to_rfc3339();
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET updated_at = ?1 WHERE share_id = ?2",
                params![stale, "share-offline-old"],
            )
            .expect("backdate offline-old");
        }

        let proxy = ProxyRegistry::default();
        proxy
            .set_route(
                "online-sub".into(),
                "127.0.0.1:5678".into(),
                None,
                None,
                None,
                false,
                -1,
                None,
            )
            .await;

        let result = store
            .cleanup_expired_data(&config, &proxy)
            .await
            .expect("cleanup stale active offline shares");

        assert_eq!(result.deleted_shares, 1);
        let conn = store.conn.lock().await;
        let surviving: Vec<String> = conn
            .prepare("SELECT share_id FROM shares ORDER BY share_id")
            .expect("prepare surviving shares")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query surviving shares")
            .collect::<Result<_, _>>()
            .expect("collect surviving shares");
        drop(conn);

        assert_eq!(
            surviving,
            vec![
                "share-offline-fresh".to_string(),
                "share-online".to_string(),
            ],
            "only the stale active share without a live route should be deleted",
        );
        let active_subdomains = proxy.active_subdomains().await;
        assert!(active_subdomains.contains(&"online-sub".to_string()));
        assert!(!active_subdomains.contains(&"offline-old-sub".to_string()));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn online_minutes_24h_is_capped_to_one_day() {
        let (store, config) = setup_store("online-minutes-cap").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;

        let now = Utc::now().timestamp();
        for minute_offset in 0..=ONLINE_WINDOW_MINUTES {
            insert_health_check(&store, "share-1", now - (minute_offset as i64 * 60), true).await;
        }

        let conn = store.conn.lock().await;
        let online = list_online_minutes_24h(&conn).expect("list online minutes");
        drop(conn);

        assert_eq!(online.get("share-1"), Some(&ONLINE_WINDOW_MINUTES));

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");
        let share = snapshot.shares.first().expect("share view");
        assert_eq!(share.online_minutes_24h, ONLINE_WINDOW_MINUTES);
        assert_eq!(share.online_rate_24h, 100.0);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn online_minutes_24h_only_counts_successful_probe_minutes() {
        let (store, _config) = setup_store("online-minutes-success-only").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;

        let now = Utc::now().timestamp();
        insert_health_check(&store, "share-1", now, true).await;
        insert_health_check(&store, "share-1", now - 60, false).await;
        insert_health_check(&store, "share-1", now - 120, false).await;
        insert_health_check(&store, "share-1", now - 120 + 10, true).await;

        let conn = store.conn.lock().await;
        let online = list_online_minutes_24h(&conn).expect("list online minutes");
        drop(conn);

        assert_eq!(online.get("share-1"), Some(&2));
    }

    #[tokio::test]
    async fn health_timeline_24h_groups_share_probe_minutes() {
        let (store, config) = setup_store("health-timeline-share-probes").await;
        let now = Utc::now();
        for minute_offset in 0..30 {
            insert_health_check(
                &store,
                "share-1",
                now.timestamp() - minute_offset * 60,
                true,
            )
            .await;
        }
        insert_health_check(&store, "share-1", now.timestamp() - 45 * 60, false).await;

        let conn = store.conn.lock().await;
        let timelines = list_share_health_timeline_24h(&conn, now).expect("list health timeline");
        drop(conn);

        let timeline = timelines.get("share-1").expect("share timeline");
        assert_eq!(timeline.len(), HEALTH_TIMELINE_BUCKETS);
        let latest = timeline.last().expect("latest bucket");
        assert_eq!(latest.status, "healthy");
        assert_eq!(latest.online_minutes, 30);
        assert!(latest.score >= 90.0);
        let previous = timeline
            .get(HEALTH_TIMELINE_BUCKETS - 2)
            .expect("previous bucket");
        assert_eq!(previous.status, "offline");
        assert_eq!(previous.observed_minutes, 1);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn dashboard_snapshot_includes_recent_model_health_checks() {
        let (store, config) = setup_store("recent-model-health-checks").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;

        let now = Utc::now().timestamp();
        for offset in 0..12 {
            let status = if offset % 2 == 0 { "success" } else { "failed" };
            insert_model_health_check(&store, "share-1", now - offset, status).await;
        }

        let server_geo = ServerGeo {
            lat: None,
            lon: None,
        };
        let proxy = ProxyRegistry::default();
        let snapshot = store
            .dashboard_snapshot(&config, &server_geo, &proxy, None)
            .await
            .expect("dashboard snapshot");
        let share = snapshot.shares.first().expect("share view");

        assert_eq!(share.recent_model_health_checks.len(), 10);
        assert_eq!(share.recent_model_health_checks[0].checked_at, now);
        assert_eq!(share.recent_model_health_checks[9].checked_at, now - 9);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[test]
    fn market_runtime_filter_ignores_stale_failed_models() {
        let runtimes = ShareAppRuntimes {
            codex: Some(test_upstream_provider("gpt-5.5")),
            ..ShareAppRuntimes::default()
        };
        let health = ShareModelHealthSummary {
            codex: vec![
                test_model_summary("codex", &["failed", "failed", "failed"]),
                test_model_summary("gpt-5.5", &["success", "success", "success"]),
            ],
            ..ShareModelHealthSummary::default()
        };

        let filtered = filter_app_runtimes_by_model_health(runtimes, &health, Utc::now());

        assert!(filtered.codex.is_some());
    }

    #[test]
    fn market_runtime_filter_removes_current_model_after_three_failures() {
        let runtimes = ShareAppRuntimes {
            codex: Some(test_upstream_provider("gpt-5.5")),
            ..ShareAppRuntimes::default()
        };
        let health = ShareModelHealthSummary {
            codex: vec![test_model_summary(
                "gpt-5.5",
                &["failed", "failed", "failed"],
            )],
            ..ShareModelHealthSummary::default()
        };

        let filtered = filter_app_runtimes_by_model_health(runtimes, &health, Utc::now());

        assert!(filtered.codex.is_none());
    }

    #[test]
    fn market_runtime_filter_removes_app_for_quota_blocked_health() {
        let runtimes = ShareAppRuntimes {
            codex: Some(test_upstream_provider("gpt-5.5")),
            ..ShareAppRuntimes::default()
        };
        let mut quota_health = test_model_summary("codex", &["failed"]);
        quota_health.actual_model = "codex".into();
        let future_reset = (Utc::now() + Duration::hours(1)).to_rfc3339();
        quota_health.error_message =
            Some(format!("long window quota exhausted until {future_reset}"));
        let health = ShareModelHealthSummary {
            codex: vec![quota_health],
            ..ShareModelHealthSummary::default()
        };

        let filtered = filter_app_runtimes_by_model_health(runtimes, &health, Utc::now());

        assert!(filtered.codex.is_none());
    }

    #[test]
    fn market_runtime_filter_keeps_app_when_quota_block_expired() {
        let runtimes = ShareAppRuntimes {
            codex: Some(test_upstream_provider("gpt-5.5")),
            ..ShareAppRuntimes::default()
        };
        let mut quota_health = test_model_summary("codex", &["failed"]);
        quota_health.actual_model = "codex".into();
        // Expired block: 1 hour ago.
        let expired = (Utc::now() - Duration::hours(1)).to_rfc3339();
        quota_health.error_message = Some(format!("five hour quota exhausted until {}", expired));
        let health = ShareModelHealthSummary {
            codex: vec![quota_health],
            ..ShareModelHealthSummary::default()
        };

        let filtered = filter_app_runtimes_by_model_health(runtimes, &health, Utc::now());

        // Expired quota block should NOT remove the runtime.
        assert!(filtered.codex.is_some());
    }

    #[test]
    fn market_runtime_filter_keeps_app_when_quota_block_expired_in_availability() {
        // Test that quota_blocked_app_availability also respects expiry.
        let expired = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let mut quota_health = test_model_summary("codex", &["failed"]);
        quota_health.actual_model = "codex".into();
        quota_health.error_message = Some(format!("five hour quota exhausted until {}", expired));
        let result = quota_blocked_app_availability("codex", None, &[quota_health], Utc::now());
        // Expired block should not produce an availability entry.
        assert!(result.is_none());
    }

    #[test]
    fn market_runtime_filter_blocks_app_when_quota_block_not_expired_in_availability() {
        let future = (Utc::now() + Duration::hours(1)).to_rfc3339();
        let mut quota_health = test_model_summary("codex", &["failed"]);
        quota_health.actual_model = "codex".into();
        quota_health.error_message = Some(format!("five hour quota exhausted until {}", future));
        let result = quota_blocked_app_availability("codex", None, &[quota_health], Utc::now());
        // Active block should produce an availability entry.
        assert!(result.is_some());
    }

    #[test]
    fn market_runtime_filter_keeps_app_when_any_current_model_is_healthy() {
        let mut provider = test_upstream_provider("gpt-5.5");
        provider.models.push(ShareUpstreamModel {
            slot: "backup".into(),
            actual_model: "gpt-5.4".into(),
        });
        let runtimes = ShareAppRuntimes {
            codex: Some(provider),
            ..ShareAppRuntimes::default()
        };
        let health = ShareModelHealthSummary {
            codex: vec![
                test_model_summary("gpt-5.5", &["failed", "failed", "failed"]),
                test_model_summary("gpt-5.4", &["success", "success", "success"]),
            ],
            ..ShareModelHealthSummary::default()
        };

        let filtered = filter_app_runtimes_by_model_health(runtimes, &health, Utc::now());

        assert!(filtered.codex.is_some());
    }

    #[test]
    fn market_runtime_filter_keeps_provider_without_model_keys() {
        let mut provider = test_upstream_provider("");
        provider.models.clear();
        let runtimes = ShareAppRuntimes {
            codex: Some(provider),
            ..ShareAppRuntimes::default()
        };
        let health = ShareModelHealthSummary::default();

        let filtered = filter_app_runtimes_by_model_health(runtimes, &health, Utc::now());

        assert!(filtered.codex.is_some());
    }

    #[tokio::test]
    async fn runtime_snapshot_removes_stale_model_health_state() {
        let (store, config) = setup_store("runtime-removes-stale-model-health").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        let now = Utc::now().timestamp();
        store
            .record_share_model_health_check(test_health_check(
                "share-1",
                "codex",
                "failed",
                now - 60,
            ))
            .await
            .expect("record stale check");

        store
            .record_share_runtime_snapshot(ShareRuntimeSnapshotResponse {
                share_id: "share-1".into(),
                queried_at: now,
                support: ShareSupport {
                    claude: false,
                    codex: true,
                    gemini: false,
                },
                app_runtimes: ShareAppRuntimes {
                    codex: Some(test_upstream_provider("gpt-5.5")),
                    ..ShareAppRuntimes::default()
                },
                app_providers: ShareAppProviders::default(),
                token_limit: None,
                tokens_used: None,
                requests_count: None,
                share_status: None,
                model_health: ShareModelHealthSummary {
                    codex: vec![test_model_summary("gpt-5.5", &["success"])],
                    ..ShareModelHealthSummary::default()
                },
            })
            .await
            .expect("record runtime snapshot");

        let conn = store.conn.lock().await;
        let stale_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_model_health_state WHERE share_id = 'share-1' AND requested_model = 'codex'",
                [],
                |row| row.get(0),
            )
            .expect("count stale state");
        let current_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_model_health_state WHERE share_id = 'share-1' AND requested_model = 'gpt-5.5'",
                [],
                |row| row.get(0),
            )
            .expect("count current state");
        drop(conn);

        assert_eq!(stale_count, 0);
        assert_eq!(current_count, 1);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    /// C-fix: even if a (buggy or pre-fix) cc-switch client pushes
    /// `model_health.codex` for a share that's only bound to claude, the
    /// router intake must drop the codex entries so the dashboard doesn't
    /// surface them.
    #[tokio::test]
    async fn runtime_snapshot_drops_model_health_for_unbound_apps() {
        let (store, config) = setup_store("runtime-drops-unbound-model-health").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        // Narrow the binding to claude only — codex/gemini summaries pushed
        // below must be dropped on intake.
        set_share_bindings(&store, "share-1", &["claude"]).await;

        // The shared `test_model_summary` helper hard-codes app_type="codex";
        // we need per-app variants for this test so the intake sees one entry
        // in each of claude / codex / gemini.
        fn summary_for(app: &str, model: &str) -> ModelHealthSummary {
            ModelHealthSummary {
                app_type: app.into(),
                requested_model: model.into(),
                actual_model: model.into(),
                status: "success".into(),
                recent_results: vec!["success".into()],
                last_checked_at: Some(Utc::now().timestamp()),
                last_success_at: None,
                last_failed_at: None,
                error_message: None,
                status_code: None,
                latency_ms: 0,
                source: None,
                provider_id: None,
                provider_name: None,
            }
        }

        let now = Utc::now().timestamp();
        store
            .record_share_runtime_snapshot(ShareRuntimeSnapshotResponse {
                share_id: "share-1".into(),
                queried_at: now,
                support: ShareSupport {
                    claude: true,
                    codex: false,
                    gemini: false,
                },
                app_runtimes: ShareAppRuntimes::default(),
                app_providers: ShareAppProviders::default(),
                token_limit: None,
                tokens_used: None,
                requests_count: None,
                share_status: None,
                model_health: ShareModelHealthSummary {
                    claude: vec![summary_for("claude", "claude-haiku")],
                    codex: vec![summary_for("codex", "gpt-5.5")],
                    gemini: vec![summary_for("gemini", "gemini-pro")],
                },
            })
            .await
            .expect("record runtime snapshot");

        let conn = store.conn.lock().await;
        let codex_checks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_model_health_checks WHERE share_id = 'share-1' AND app_type = 'codex'",
                [],
                |row| row.get(0),
            )
            .expect("count codex checks");
        let gemini_checks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_model_health_checks WHERE share_id = 'share-1' AND app_type = 'gemini'",
                [],
                |row| row.get(0),
            )
            .expect("count gemini checks");
        let claude_checks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_model_health_checks WHERE share_id = 'share-1' AND app_type = 'claude'",
                [],
                |row| row.get(0),
            )
            .expect("count claude checks");
        drop(conn);

        assert_eq!(
            codex_checks, 0,
            "codex entries must be dropped because share-1 doesn't bind codex"
        );
        assert_eq!(
            gemini_checks, 0,
            "gemini entries must be dropped because share-1 doesn't bind gemini"
        );
        assert_eq!(claude_checks, 1, "claude is bound — its entry must land");

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    /// C-fix: after a user unbinds an app, any historical model_health rows
    /// for that app must be purged so the dashboard stops showing them. We
    /// do this lazily on the next snapshot intake.
    #[tokio::test]
    async fn runtime_snapshot_purges_history_for_unbound_apps() {
        let (store, config) = setup_store("runtime-purges-unbound-history").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        let now = Utc::now().timestamp();

        // Seed a historical codex check (e.g. while the share was still
        // bound to codex). After we unbind codex, this row should go.
        store
            .record_share_model_health_check(test_health_check(
                "share-1",
                "gpt-5.5",
                "success",
                now - 100,
            ))
            .await
            .expect("seed historical codex check");

        // Narrow the binding so codex is gone.
        set_share_bindings(&store, "share-1", &["claude"]).await;

        // Trigger an intake. We don't need codex content — the intake itself
        // does the cleanup pass.
        store
            .record_share_runtime_snapshot(ShareRuntimeSnapshotResponse {
                share_id: "share-1".into(),
                queried_at: now,
                support: ShareSupport {
                    claude: true,
                    codex: false,
                    gemini: false,
                },
                app_runtimes: ShareAppRuntimes::default(),
                app_providers: ShareAppProviders::default(),
                token_limit: None,
                tokens_used: None,
                requests_count: None,
                share_status: None,
                model_health: ShareModelHealthSummary {
                    claude: vec![test_model_summary("claude", &["success"])],
                    ..ShareModelHealthSummary::default()
                },
            })
            .await
            .expect("record runtime snapshot");

        let conn = store.conn.lock().await;
        let codex_remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_model_health_checks WHERE share_id = 'share-1' AND app_type = 'codex'",
                [],
                |row| row.get(0),
            )
            .expect("count codex history");
        drop(conn);

        assert_eq!(
            codex_remaining, 0,
            "historical codex rows must be purged once codex is no longer bound"
        );

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn runtime_snapshot_refreshes_share_usage_counters() {
        let (store, config) = setup_store("runtime-refreshes-share-usage").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        let now = Utc::now().timestamp();

        store
            .record_share_runtime_snapshot(ShareRuntimeSnapshotResponse {
                share_id: "share-1".into(),
                queried_at: now,
                token_limit: Some(-1),
                tokens_used: Some(31_900_000),
                requests_count: Some(137),
                share_status: Some("active".into()),
                support: ShareSupport {
                    claude: true,
                    codex: true,
                    gemini: true,
                },
                app_runtimes: ShareAppRuntimes::default(),
                app_providers: ShareAppProviders {
                    codex: vec![ShareAppProvider {
                        id: "provider-1".into(),
                        name: "OpenAI Official".into(),
                        app: "codex".into(),
                        kind: Some("official_oauth".into()),
                        provider_type: Some("codex_oauth".into()),
                        is_current: true,
                        enabled: true,
                        codex_image_generation_enabled: false,
                        for_sale_official_price_percent: Some(80),
                        account_email: Some("account@example.com".into()),
                        api_url: None,
                        quota: None,
                        models: Vec::new(),
                        ..Default::default()
                    }],
                    ..ShareAppProviders::default()
                },
                model_health: ShareModelHealthSummary::default(),
            })
            .await
            .expect("record runtime snapshot");

        let conn = store.conn.lock().await;
        let (token_limit, tokens_used, requests_count, share_status): (i64, i64, i64, String) =
            conn.query_row(
                "SELECT token_limit, tokens_used, requests_count, share_status FROM shares WHERE share_id = 'share-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("query share usage");
        let app_providers_json: String = conn
            .query_row(
                "SELECT app_providers_json FROM shares WHERE share_id = 'share-1'",
                [],
                |row| row.get(0),
            )
            .expect("query app providers");
        drop(conn);

        assert_eq!(token_limit, -1);
        assert_eq!(tokens_used, 31_900_000);
        assert_eq!(requests_count, 137);
        assert_eq!(share_status, "active");
        assert!(app_providers_json.contains("provider-1"));
        assert!(app_providers_json.contains("account@example.com"));

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn older_runtime_snapshot_does_not_delete_newer_model_health_state() {
        let (store, config) = setup_store("runtime-stale-snapshot-keeps-newer-health").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        let now = Utc::now().timestamp();
        let mut current_summary = test_model_summary("gpt-5.5", &["success"]);
        current_summary.last_checked_at = Some(now);
        store
            .record_share_runtime_snapshot(ShareRuntimeSnapshotResponse {
                share_id: "share-1".into(),
                queried_at: now,
                support: ShareSupport {
                    claude: false,
                    codex: true,
                    gemini: false,
                },
                app_runtimes: ShareAppRuntimes {
                    codex: Some(test_upstream_provider("gpt-5.5")),
                    ..ShareAppRuntimes::default()
                },
                app_providers: ShareAppProviders::default(),
                token_limit: None,
                tokens_used: None,
                requests_count: None,
                share_status: None,
                model_health: ShareModelHealthSummary {
                    codex: vec![current_summary],
                    ..ShareModelHealthSummary::default()
                },
            })
            .await
            .expect("record newer runtime snapshot");

        let mut stale_summary = test_model_summary("codex", &["failed"]);
        stale_summary.last_checked_at = Some(now - 60);
        store
            .record_share_runtime_snapshot(ShareRuntimeSnapshotResponse {
                share_id: "share-1".into(),
                queried_at: now - 60,
                support: ShareSupport {
                    claude: false,
                    codex: true,
                    gemini: false,
                },
                app_runtimes: ShareAppRuntimes {
                    codex: Some(test_upstream_provider("codex")),
                    ..ShareAppRuntimes::default()
                },
                app_providers: ShareAppProviders::default(),
                token_limit: None,
                tokens_used: None,
                requests_count: None,
                share_status: None,
                model_health: ShareModelHealthSummary {
                    codex: vec![stale_summary],
                    ..ShareModelHealthSummary::default()
                },
            })
            .await
            .expect("record older runtime snapshot");

        let conn = store.conn.lock().await;
        let current_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM share_model_health_state WHERE share_id = 'share-1' AND requested_model = 'gpt-5.5'",
                [],
                |row| row.get(0),
            )
            .expect("count current state");
        drop(conn);

        assert_eq!(current_count, 1);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    #[tokio::test]
    async fn older_model_health_check_does_not_roll_back_state() {
        let (store, config) = setup_store("model-health-no-rollback").await;
        insert_installation(&store, "inst-1").await;
        insert_share(&store, "inst-1", "share-1", "share-sub", "active").await;
        let now = Utc::now().timestamp();
        store
            .record_share_model_health_check(test_health_check("share-1", "gpt-5.5", "failed", now))
            .await
            .expect("record newer failed check");
        store
            .record_share_model_health_check(test_health_check(
                "share-1",
                "gpt-5.5",
                "success",
                now - 60,
            ))
            .await
            .expect("record older success check");

        let conn = store.conn.lock().await;
        let (last_status, last_checked_at): (String, i64) = conn
            .query_row(
                "SELECT last_status, last_checked_at FROM share_model_health_state WHERE share_id = 'share-1' AND requested_model = 'gpt-5.5'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read model health state");
        drop(conn);

        assert_eq!(last_status, "failed");
        assert_eq!(last_checked_at, now);

        let _ = std::fs::remove_file(PathBuf::from(config.db_path));
    }

    /// Regression test for the rate-limit bypass Codex flagged: a guest can
    /// rotate `X-Board-Guest-Id` to mint fresh buckets. The fix keys
    /// anonymous posts by salted IP instead, so guest_id rotation must NOT
    /// bypass `board_guest_per_hour`.
    #[tokio::test]
    async fn guest_rate_limit_keys_on_ip_not_guest_id() {
        use crate::dynamic_settings::BoardSettings;
        let (store, _config) = setup_store("guest-rate-limit-by-ip").await;
        let settings = BoardSettings {
            max_len: 1000,
            guest_per_hour: 3,
            user_per_hour: 100,
            pin_limit: 3,
            guest_self_delete_secs: 300,
        };
        let same_ip = Some("203.0.113.5");

        // Rotate guest_id on every call from the same IP. After
        // `guest_per_hour` successful posts, the next one must be rejected.
        for i in 0..settings.guest_per_hour {
            let guest_id = format!("rotated-guest-{i}");
            let author = BoardAuthor::Guest {
                guest_id,
                name: None,
            };
            store
                .create_board_message(&settings, author, format!("msg {i}"), same_ip)
                .await
                .expect("post under limit succeeds");
        }
        let overflow = store
            .create_board_message(
                &settings,
                BoardAuthor::Guest {
                    guest_id: "rotated-guest-final".into(),
                    name: None,
                },
                "overflow".into(),
                same_ip,
            )
            .await;
        assert!(
            matches!(overflow, Err(AppError::TooManyRequests(_))),
            "guest_id rotation must not bypass the per-IP guest rate limit, \
             got: {overflow:?}"
        );

        // A request from a different IP, even with a reused guest_id, gets
        // its own bucket and succeeds.
        let other_ip = Some("198.51.100.7");
        store
            .create_board_message(
                &settings,
                BoardAuthor::Guest {
                    guest_id: "rotated-guest-0".into(),
                    name: None,
                },
                "from-other-ip".into(),
                other_ip,
            )
            .await
            .expect("different IP gets a fresh bucket");
    }
}
