mod client;
mod crypto;
mod store;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use std::time::Instant;

use anyhow::{Context, bail};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use chrono::{TimeZone, Utc};
use futures_util::{StreamExt, stream};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::ServerState;
use crate::error::AppError;
use crate::models::AuthSession;

use self::client::{BinanceApiError, BinanceClient, BinancePayTransaction};
use self::crypto::{BinanceCredentials, CredentialCipher, credential_aad};
pub use self::store::{
    BinanceFundingIntentView, BinancePaymentAccountView, BinancePaymentIntentView,
    BinanceReceiptHistoryEntryView, BinanceSettlementAdminView,
};
use self::store::{
    StoredPaymentAccount, decode_account_credentials, mask_api_key, validate_credentials,
    validate_uid,
};
pub(crate) use self::store::{cancel_invoice_intents_tx, format_amount};

const MAX_POLL_ACCOUNTS_PER_CYCLE: usize = 8;
const MAX_TRANSACTION_QUERIES_PER_POLL: usize = 32;
// GET /sapi/v1/pay/transactions costs 3000 UID weight against a documented
// 180000/minute UID budget. Spacing query starts also leaves headroom for
// credential verification and other operators using the same Binance UID.
const MIN_TRANSACTION_QUERY_SPACING: StdDuration = StdDuration::from_millis(1_250);
const TRANSACTION_PAGE_LIMIT: usize = 100;
const POLL_OVERLAP_MS: i64 = 10 * 60 * 1_000;
const MAX_POLL_SCAN_WINDOW_MS: i64 = 60 * 60 * 1_000;
const PERMISSION_REVERIFY_HOURS: i64 = 24;
const VERIFICATION_ATTEMPT_COOLDOWN_SECS: u64 = 30;
const BINDING_CONFIRMATION_TTL_SECS: i64 = 10 * 60;
const BINDING_CONFIRMATION_VERSION: u8 = 1;
const MAX_VERIFICATION_ATTEMPT_SCOPES: usize = 10_000;
const DEFAULT_MASTER_KEY_VERSION: i64 = 1;
const DEFAULT_POLL_INTERVAL_SECS: i64 = 4;
const MASTER_KEY_FILE: &str = "binance-master-key";
const AVAILABILITY_OK_TTL: StdDuration = StdDuration::from_secs(10 * 60);
const AVAILABILITY_REGION_RESTRICTED_TTL: StdDuration = StdDuration::from_secs(30 * 60);
const AVAILABILITY_TEMPORARY_FAILURE_TTL: StdDuration = StdDuration::from_secs(30);
const MIN_POLL_INTERVAL_SECS: i64 = 2;
const MAX_POLL_INTERVAL_SECS: i64 = 60;
const MAX_MASTER_KEY_VERSION: i64 = 1_000_000;
const OFFICIAL_BINANCE_API_HOSTS: &[&str] = &[
    "api.binance.com",
    "api-gcp.binance.com",
    "api1.binance.com",
    "api2.binance.com",
    "api3.binance.com",
    "api4.binance.com",
];

#[derive(Debug, Clone)]
struct PollScanCheckpoint {
    completed_cursor_at: Option<String>,
    scan_cursor_at: Option<String>,
    scan_target_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalMode {
    Disabled,
    Shadow,
    Enabled,
}

impl GlobalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Shadow => "shadow",
            Self::Enabled => "enabled",
        }
    }

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "disabled" => Ok(Self::Disabled),
            "shadow" => Ok(Self::Shadow),
            "enabled" => Ok(Self::Enabled),
            value => bail!(
                "invalid CC_SWITCH_ROUTER_BINANCE_AUTO_SETTLEMENT_MODE: {value}; expected disabled, shadow, or enabled"
            ),
        }
    }

    fn parses(value: Option<String>) -> anyhow::Result<Self> {
        Self::parse(value.as_deref().unwrap_or("disabled"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinanceServiceAvailability {
    Unchecked,
    Available,
    RegionRestricted,
    TemporarilyUnavailable,
}

impl BinanceServiceAvailability {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unchecked => "unchecked",
            Self::Available => "available",
            Self::RegionRestricted => "region_restricted",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
        }
    }

    fn cache_ttl(self) -> StdDuration {
        match self {
            Self::Unchecked => StdDuration::ZERO,
            Self::Available => AVAILABILITY_OK_TTL,
            Self::RegionRestricted => AVAILABILITY_REGION_RESTRICTED_TTL,
            Self::TemporarilyUnavailable => AVAILABILITY_TEMPORARY_FAILURE_TTL,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BinanceServiceAvailabilitySnapshot {
    pub(crate) status: BinanceServiceAvailability,
    pub(crate) checked_at: Option<String>,
    pub(crate) error_code: Option<String>,
}

#[derive(Debug)]
struct BinanceServiceAvailabilityCache {
    status: BinanceServiceAvailability,
    checked_at: Option<String>,
    checked_instant: Option<Instant>,
    error_code: Option<String>,
}

impl Default for BinanceServiceAvailabilityCache {
    fn default() -> Self {
        Self {
            status: BinanceServiceAvailability::Unchecked,
            checked_at: None,
            checked_instant: None,
            error_code: None,
        }
    }
}

impl BinanceServiceAvailabilityCache {
    fn is_fresh(&self, now: Instant) -> bool {
        self.checked_instant.is_some_and(|checked_at| {
            now.saturating_duration_since(checked_at) < self.status.cache_ttl()
        })
    }

    fn snapshot(&self) -> BinanceServiceAvailabilitySnapshot {
        BinanceServiceAvailabilitySnapshot {
            status: self.status,
            checked_at: self.checked_at.clone(),
            error_code: self.error_code.clone(),
        }
    }
}

#[derive(Clone)]
pub struct BinanceSettlementRuntime {
    mode: GlobalMode,
    cipher: Option<CredentialCipher>,
    client: BinanceClient,
    payment_home_region: Arc<str>,
    poll_interval_secs: i64,
    worker_id: Arc<str>,
    verification_attempts: Arc<tokio::sync::Mutex<HashMap<String, Instant>>>,
    service_availability: Arc<tokio::sync::Mutex<BinanceServiceAvailabilityCache>>,
}

impl BinanceSettlementRuntime {
    pub fn from_env(default_region: &str, data_dir: &FsPath) -> anyhow::Result<Self> {
        let mode = GlobalMode::parses(
            std::env::var("CC_SWITCH_ROUTER_BINANCE_AUTO_SETTLEMENT_MODE").ok(),
        )?;
        let key_override = std::env::var("CC_SWITCH_ROUTER_BINANCE_MASTER_KEY").ok();
        let key = load_or_create_master_key(data_dir, key_override.as_deref())?;
        let key_version = std::env::var("CC_SWITCH_ROUTER_BINANCE_MASTER_KEY_VERSION")
            .ok()
            .map(|value| value.parse::<i64>())
            .transpose()
            .context("invalid CC_SWITCH_ROUTER_BINANCE_MASTER_KEY_VERSION")?
            .unwrap_or(DEFAULT_MASTER_KEY_VERSION);
        if !(1..=MAX_MASTER_KEY_VERSION).contains(&key_version) {
            bail!(
                "CC_SWITCH_ROUTER_BINANCE_MASTER_KEY_VERSION must be between 1 and {MAX_MASTER_KEY_VERSION}"
            );
        }
        let base_url = std::env::var("CC_SWITCH_ROUTER_BINANCE_API_BASE")
            .unwrap_or_else(|_| "https://api.binance.com".into());
        let base_url = Url::parse(base_url.trim()).context("invalid Binance API base URL")?;
        validate_api_base(&base_url)?;
        let socks_proxy = std::env::var("CC_SWITCH_ROUTER_BINANCE_SOCKS_PROXY_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                validate_binance_socks_proxy_url(&value)?;
                Url::parse(value.trim()).context("invalid Binance SOCKS proxy URL")
            })
            .transpose()?;
        let region = std::env::var("CC_SWITCH_ROUTER_BINANCE_PAYMENT_HOME_REGION")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| normalize_payment_home_region(&value))
            .transpose()?
            .unwrap_or_else(|| {
                let value = default_region.trim();
                if value.is_empty() {
                    "local".into()
                } else {
                    value.into()
                }
            });
        let region = normalize_payment_home_region(&region)?;
        let poll_interval_secs = std::env::var("CC_SWITCH_ROUTER_BINANCE_POLL_INTERVAL_SECS")
            .ok()
            .map(|value| value.parse::<i64>())
            .transpose()
            .context("invalid CC_SWITCH_ROUTER_BINANCE_POLL_INTERVAL_SECS")?
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECS);
        if !(MIN_POLL_INTERVAL_SECS..=MAX_POLL_INTERVAL_SECS).contains(&poll_interval_secs) {
            bail!(
                "CC_SWITCH_ROUTER_BINANCE_POLL_INTERVAL_SECS must be between {MIN_POLL_INTERVAL_SECS} and {MAX_POLL_INTERVAL_SECS}"
            );
        }
        Ok(Self {
            mode,
            cipher: Some(CredentialCipher::from_zeroizing(key, key_version)),
            client: BinanceClient::with_proxy(base_url, socks_proxy.as_ref())?,
            payment_home_region: Arc::from(region),
            poll_interval_secs,
            worker_id: Arc::from(Uuid::new_v4().to_string()),
            verification_attempts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            service_availability: Arc::new(tokio::sync::Mutex::new(
                BinanceServiceAvailabilityCache::default(),
            )),
        })
    }

    #[cfg(test)]
    pub fn disabled_for_tests() -> Self {
        Self {
            mode: GlobalMode::Disabled,
            cipher: Some(CredentialCipher::new([9; 32], 1)),
            client: BinanceClient::new(Url::parse("https://api.binance.com").unwrap()).unwrap(),
            payment_home_region: Arc::from("test"),
            poll_interval_secs: 4,
            worker_id: Arc::from("test-worker"),
            verification_attempts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            service_availability: Arc::new(tokio::sync::Mutex::new(
                BinanceServiceAvailabilityCache::default(),
            )),
        }
    }

    pub fn mode(&self) -> GlobalMode {
        self.mode
    }

    pub fn payment_home_region(&self) -> &str {
        &self.payment_home_region
    }

    fn cipher(&self) -> Result<&CredentialCipher, AppError> {
        self.cipher.as_ref().ok_or_else(|| {
            AppError::ServiceUnavailable("Binance credential encryption is not configured".into())
        })
    }

    fn require_binance_network_enabled(&self) -> Result<&CredentialCipher, AppError> {
        if self.mode == GlobalMode::Disabled {
            return Err(AppError::ServiceUnavailable(
                "Binance integration is disabled on this Router".into(),
            ));
        }
        self.cipher()
    }

    async fn require_binance_available(&self) -> Result<&CredentialCipher, AppError> {
        self.require_binance_network_enabled()?;
        let availability = self.service_availability(false).await;
        match availability.status {
            BinanceServiceAvailability::Available => self.cipher(),
            BinanceServiceAvailability::RegionRestricted => Err(binance_region_restricted_error()),
            BinanceServiceAvailability::Unchecked
            | BinanceServiceAvailability::TemporarilyUnavailable => Err(
                binance_temporarily_unavailable_error(availability.error_code.as_deref()),
            ),
        }
    }

    fn require_payment_enabled(&self) -> Result<&CredentialCipher, AppError> {
        if self.mode != GlobalMode::Enabled {
            return Err(AppError::ServiceUnavailable(
                "Binance auto-settlement is not enabled on this Router".into(),
            ));
        }
        self.cipher()
    }

    async fn require_payment_available(&self) -> Result<&CredentialCipher, AppError> {
        self.require_payment_enabled()?;
        let availability = self.service_availability(false).await;
        match availability.status {
            BinanceServiceAvailability::Available => self.cipher(),
            BinanceServiceAvailability::RegionRestricted => Err(binance_region_restricted_error()),
            BinanceServiceAvailability::Unchecked
            | BinanceServiceAvailability::TemporarilyUnavailable => Err(
                binance_temporarily_unavailable_error(availability.error_code.as_deref()),
            ),
        }
    }

    pub(crate) async fn supplier_funding_unavailable_reason(
        &self,
        store: &crate::store::AppStore,
        supplier_user_id: &str,
    ) -> Result<Option<&'static str>, AppError> {
        match self.mode {
            GlobalMode::Disabled => return Ok(Some("router_disabled")),
            GlobalMode::Shadow => return Ok(Some("router_shadow")),
            GlobalMode::Enabled => {}
        }
        let availability = self.service_availability(false).await;
        match availability.status {
            BinanceServiceAvailability::Available => {}
            BinanceServiceAvailability::RegionRestricted => {
                return Ok(Some("region_restricted"));
            }
            BinanceServiceAvailability::Unchecked
            | BinanceServiceAvailability::TemporarilyUnavailable => {
                return Ok(Some("temporarily_unavailable"));
            }
        }
        let Some(cipher) = self.cipher.as_ref() else {
            return Ok(Some("credential_storage_unavailable"));
        };
        let available = store
            .binance_supplier_funding_available_for_cipher(
                supplier_user_id,
                self.payment_home_region(),
                cipher,
            )
            .await?;
        Ok((!available).then_some("supplier_unavailable"))
    }

    pub(crate) async fn service_availability(
        &self,
        force: bool,
    ) -> BinanceServiceAvailabilitySnapshot {
        let mut cache = self.service_availability.lock().await;
        if self.mode == GlobalMode::Disabled {
            return cache.snapshot();
        }
        let now = Instant::now();
        if !force && cache.is_fresh(now) {
            return cache.snapshot();
        }
        let previous = cache.status;
        let (status, error_code) = match self.client.probe_service_availability().await {
            Ok(()) => (BinanceServiceAvailability::Available, None),
            Err(error) if error.code == "BINANCE_REGION_RESTRICTED" => (
                BinanceServiceAvailability::RegionRestricted,
                Some(error.code),
            ),
            Err(error) => (
                BinanceServiceAvailability::TemporarilyUnavailable,
                Some(error.code),
            ),
        };
        cache.status = status;
        cache.checked_at = Some(Utc::now().to_rfc3339());
        cache.checked_instant = Some(now);
        cache.error_code = error_code.clone();
        let snapshot = cache.snapshot();
        drop(cache);
        if status != previous {
            match status {
                BinanceServiceAvailability::Available
                    if previous == BinanceServiceAvailability::Unchecked =>
                {
                    tracing::info!("Binance service availability confirmed")
                }
                BinanceServiceAvailability::Available => {
                    tracing::info!("Binance service availability recovered")
                }
                BinanceServiceAvailability::RegionRestricted => tracing::warn!(
                    error_code = error_code.as_deref().unwrap_or("BINANCE_REGION_RESTRICTED"),
                    "Binance is unavailable from the Router network region"
                ),
                BinanceServiceAvailability::TemporarilyUnavailable => tracing::warn!(
                    error_code = error_code.as_deref().unwrap_or("BINANCE_UNAVAILABLE"),
                    "Binance service availability probe failed"
                ),
                BinanceServiceAvailability::Unchecked => {}
            }
        }
        snapshot
    }

    async fn observe_api_error(&self, error: &BinanceApiError) {
        let status = if error.code == "BINANCE_REGION_RESTRICTED" {
            Some(BinanceServiceAvailability::RegionRestricted)
        } else if matches!(
            error.code.as_str(),
            "BINANCE_TIMEOUT"
                | "BINANCE_NETWORK_ERROR"
                | "BINANCE_UPSTREAM_ERROR"
                | "BINANCE_RESPONSE_READ_FAILED"
                | "BINANCE_RESPONSE_TOO_LARGE"
        ) {
            Some(BinanceServiceAvailability::TemporarilyUnavailable)
        } else {
            None
        };
        let Some(status) = status else {
            return;
        };
        let mut cache = self.service_availability.lock().await;
        let changed = cache.status != status;
        cache.status = status;
        cache.checked_at = Some(Utc::now().to_rfc3339());
        cache.checked_instant = Some(Instant::now());
        cache.error_code = Some(error.code.clone());
        drop(cache);
        if changed && status == BinanceServiceAvailability::RegionRestricted {
            tracing::warn!(
                error_code = %error.code,
                "Binance signed API reported a Router network region restriction"
            );
        }
    }

    async fn consume_verification_attempt(&self, supplier_user_id: &str) -> Result<(), AppError> {
        let now = Instant::now();
        let cooldown = StdDuration::from_secs(VERIFICATION_ATTEMPT_COOLDOWN_SECS);
        let mut attempts = self.verification_attempts.lock().await;
        if let Some(previous) = attempts.get(supplier_user_id) {
            let elapsed = now.saturating_duration_since(*previous);
            if elapsed < cooldown {
                return Err(AppError::RateLimited {
                    message: "wait before verifying Binance credentials again".into(),
                    retry_after_secs: cooldown.saturating_sub(elapsed).as_secs().max(1),
                });
            }
        }
        attempts.retain(|_, attempted_at| now.saturating_duration_since(*attempted_at) < cooldown);
        if attempts.len() >= MAX_VERIFICATION_ATTEMPT_SCOPES
            && !attempts.contains_key(supplier_user_id)
            && let Some(oldest) = attempts
                .iter()
                .min_by_key(|(_, attempted_at)| *attempted_at)
                .map(|(scope, _)| scope.clone())
        {
            attempts.remove(&oldest);
        }
        attempts.insert(supplier_user_id.to_string(), now);
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceAccountStatusResponse {
    global_mode: &'static str,
    credential_storage_configured: bool,
    payment_home_region: String,
    service_availability: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_availability_checked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_availability_error_code: Option<String>,
    account: Option<BinancePaymentAccountView>,
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiscoverBinanceAccountRequest {
    api_key: String,
    api_secret: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfirmBinanceAccountRequest {
    confirmation_token: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindingConfirmationPayload {
    version: u8,
    user_id: String,
    payment_home_region: String,
    account_id: String,
    credential_revision: i64,
    binance_uid: String,
    credentials: BinanceCredentials,
    verification: self::client::VerificationResult,
    issued_at_ms: i64,
    expires_at_ms: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveredBinanceAccountView {
    binance_uid: String,
    masked_api_key: String,
    reading_enabled: bool,
    dangerous_permissions_disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    uid_confirmation_source: Option<self::client::UidConfirmationSource>,
    evidence_count: usize,
    detected_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_binance_uid: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoverBinanceAccountResponse {
    account: DiscoveredBinanceAccountView,
    confirmation_token: String,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveReconciliationRequest {
    resolution: String,
    invoice_id: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateFundingIntentRequest {
    supplier_user_id: String,
    amount_minor: i64,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptHistoryQuery {
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptHistoryCursor {
    v: u8,
    confirmed_at: String,
    receipt_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptHistoryResponse {
    items: Vec<BinanceReceiptHistoryEntryView>,
    next_cursor: Option<String>,
}

fn binding_mode(global_mode: GlobalMode) -> &'static str {
    match global_mode {
        GlobalMode::Enabled => "enabled",
        GlobalMode::Disabled | GlobalMode::Shadow => "shadow",
    }
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/account/binance-auto-settlement",
            get(get_account_status)
                .put(bind_account)
                .delete(delete_account),
        )
        .route(
            "/v1/account/binance-auto-settlement/verify",
            post(verify_account),
        )
        .route(
            "/v1/account/binance-auto-settlement/enable",
            post(enable_account),
        )
        .route(
            "/v1/account/binance-auto-settlement/discover",
            post(discover_account),
        )
        .route(
            "/v1/account/binance-auto-settlement/disable",
            post(disable_account),
        )
        .route(
            "/v1/account/binance-auto-settlement/receipts",
            get(get_receipt_history),
        )
        .route(
            "/v1/market-billing/invoices/:invoice_id/binance-intent",
            get(get_payment_intent)
                .post(create_payment_intent)
                .delete(cancel_payment_intent),
        )
        .route(
            "/v1/market-billing/invoices/:invoice_id/binance-intent/refresh",
            post(refresh_payment_intent),
        )
        .route(
            "/v1/market-billing/funding-intents",
            post(create_funding_intent),
        )
        .route(
            "/v1/market-billing/funding-intents/:intent_id",
            get(get_funding_intent).delete(cancel_funding_intent),
        )
        .route(
            "/v1/market-billing/funding-intents/:intent_id/refresh",
            post(refresh_funding_intent),
        )
        .route(
            "/v1/admin/market-billing/binance-reconciliation",
            get(get_admin_reconciliation),
        )
        .route(
            "/v1/admin/market-billing/binance-reconciliation/:case_id/resolve",
            post(resolve_admin_reconciliation),
        )
}

fn decode_receipt_cursor(value: &str) -> Result<ReceiptHistoryCursor, AppError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AppError::BadRequest("invalid receipt history cursor".into()))?;
    let cursor: ReceiptHistoryCursor = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid receipt history cursor".into()))?;
    if cursor.v != 1
        || cursor.receipt_id.is_empty()
        || cursor.receipt_id.len() > 128
        || chrono::DateTime::parse_from_rfc3339(&cursor.confirmed_at).is_err()
    {
        return Err(AppError::BadRequest(
            "invalid receipt history cursor".into(),
        ));
    }
    Ok(cursor)
}

fn encode_receipt_cursor(item: &BinanceReceiptHistoryEntryView) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(&ReceiptHistoryCursor {
        v: 1,
        confirmed_at: item.confirmed_at.clone(),
        receipt_id: item.receipt_id.clone(),
    })
    .map_err(|error| AppError::Internal(format!("encode receipt history cursor: {error}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

async fn get_receipt_history(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ReceiptHistoryQuery>,
) -> Result<Json<ReceiptHistoryResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    let cursor = query
        .cursor
        .as_deref()
        .map(decode_receipt_cursor)
        .transpose()?;
    let before = cursor
        .as_ref()
        .map(|cursor| (cursor.confirmed_at.as_str(), cursor.receipt_id.as_str()));
    let batch = state
        .store
        .binance_receipt_history(&session, before, limit)
        .await?;
    let next_cursor = if batch.has_more {
        batch.items.last().map(encode_receipt_cursor).transpose()?
    } else {
        None
    };
    Ok(Json(ReceiptHistoryResponse {
        items: batch.items,
        next_cursor,
    }))
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated user session required".into()))
}

async fn account_status(
    state: &ServerState,
    session: &AuthSession,
) -> Result<BinanceAccountStatusResponse, AppError> {
    let availability = state.binance_settlement.service_availability(false).await;
    Ok(BinanceAccountStatusResponse {
        global_mode: state.binance_settlement.mode().as_str(),
        credential_storage_configured: state.binance_settlement.cipher.is_some(),
        payment_home_region: state.binance_settlement.payment_home_region().to_string(),
        service_availability: availability.status.as_str(),
        service_availability_checked_at: availability.checked_at,
        service_availability_error_code: availability.error_code,
        account: state
            .store
            .binance_payment_account_view(
                &session.user_id,
                state.binance_settlement.payment_home_region(),
            )
            .await?,
    })
}

async fn get_account_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BinanceAccountStatusResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(account_status(&state, &session).await?))
}

fn binding_confirmation_aad(user_id: &str, region: &str) -> String {
    format!("binance-binding-confirmation:v1:{user_id}:{region}")
}

fn seal_binding_confirmation(
    cipher: &CredentialCipher,
    payload: &BindingConfirmationPayload,
) -> Result<String, AppError> {
    let aad = binding_confirmation_aad(&payload.user_id, &payload.payment_home_region);
    let (ciphertext, nonce) = cipher.seal_json(payload, aad.as_bytes())?;
    Ok(format!("v1.{nonce}.{ciphertext}"))
}

fn open_binding_confirmation(
    cipher: &CredentialCipher,
    token: &str,
    session: &AuthSession,
    region: &str,
) -> Result<BindingConfirmationPayload, AppError> {
    if token.len() > 16 * 1024 {
        return Err(AppError::BadRequest(
            "Binance binding confirmation is invalid".into(),
        ));
    }
    let mut parts = token.split('.');
    let (Some("v1"), Some(nonce), Some(ciphertext), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(AppError::BadRequest(
            "Binance binding confirmation is invalid".into(),
        ));
    };
    let aad = binding_confirmation_aad(&session.user_id, region);
    let payload = cipher
        .open_json::<BindingConfirmationPayload>(ciphertext, nonce, aad.as_bytes())
        .map_err(|_| AppError::BadRequest("Binance binding confirmation is invalid".into()))?;
    let now_ms = Utc::now().timestamp_millis();
    if payload.version != BINDING_CONFIRMATION_VERSION
        || payload.user_id != session.user_id
        || payload.payment_home_region != region
        || payload.credential_revision <= 0
        || Uuid::parse_str(&payload.account_id).is_err()
        || payload.issued_at_ms > now_ms.saturating_add(30_000)
        || payload.expires_at_ms <= now_ms
        || payload.expires_at_ms.saturating_sub(payload.issued_at_ms)
            != BINDING_CONFIRMATION_TTL_SECS * 1_000
    {
        return Err(AppError::BadRequest(
            "Binance binding confirmation is invalid or expired".into(),
        ));
    }
    validate_uid(&payload.binance_uid)?;
    validate_credentials(
        &payload.credentials.api_key,
        &payload.credentials.api_secret,
    )?;
    require_initial_uid_confirmation(&payload.verification)?;
    Ok(payload)
}

async fn discover_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<DiscoverBinanceAccountRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let cipher = state.binance_settlement.require_binance_available().await?;
    validate_credentials(&input.api_key, &input.api_secret)?;
    state
        .binance_settlement
        .consume_verification_attempt(&session.user_id)
        .await?;
    let credentials = BinanceCredentials {
        api_key: input.api_key.trim().to_string(),
        api_secret: input.api_secret.trim().to_string(),
    };
    let discovery = match state
        .binance_settlement
        .client
        .discover_account(&credentials)
        .await
    {
        Ok(discovery) => discovery,
        Err(error) => {
            state.binance_settlement.observe_api_error(&error).await;
            return Err(map_verification_error(error));
        }
    };
    let binance_uid = validate_uid(&discovery.binance_uid)?;
    let previous_binance_uid = state
        .store
        .binance_payment_account_view(
            &session.user_id,
            state.binance_settlement.payment_home_region(),
        )
        .await?
        .filter(|account| !account.masked_api_key.is_empty())
        .map(|account| account.binance_uid)
        .filter(|uid| uid != &binance_uid);
    let (account_id, revision) = state
        .store
        .binance_prepare_account_binding(
            &session.user_id,
            state.binance_settlement.payment_home_region(),
            &binance_uid,
        )
        .await?;
    let now = Utc::now();
    let expires_at = now + chrono::Duration::seconds(BINDING_CONFIRMATION_TTL_SECS);
    let payload = BindingConfirmationPayload {
        version: BINDING_CONFIRMATION_VERSION,
        user_id: session.user_id.clone(),
        payment_home_region: state.binance_settlement.payment_home_region().to_string(),
        account_id,
        credential_revision: revision,
        binance_uid: binance_uid.clone(),
        credentials,
        verification: discovery.verification.clone(),
        issued_at_ms: now.timestamp_millis(),
        expires_at_ms: expires_at.timestamp_millis(),
    };
    let confirmation_token = seal_binding_confirmation(cipher, &payload)?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(DiscoverBinanceAccountResponse {
            account: DiscoveredBinanceAccountView {
                binance_uid,
                masked_api_key: mask_api_key(&payload.credentials.api_key),
                reading_enabled: discovery.verification.reading_enabled,
                dangerous_permissions_disabled: discovery
                    .verification
                    .dangerous_permissions_disabled,
                uid_confirmation_source: discovery.verification.uid_confirmation_source,
                evidence_count: discovery.evidence_count,
                detected_at: now.to_rfc3339(),
                previous_binance_uid,
            },
            confirmation_token,
            expires_at: expires_at.to_rfc3339(),
        }),
    ))
}

async fn bind_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ConfirmBinanceAccountRequest>,
) -> Result<Json<BinanceAccountStatusResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let cipher = state.binance_settlement.require_binance_available().await?;
    let payload = open_binding_confirmation(
        cipher,
        input.confirmation_token.trim(),
        &session,
        state.binance_settlement.payment_home_region(),
    )?;
    state
        .store
        .binance_assert_account_binding_pending(
            &session.user_id,
            state.binance_settlement.payment_home_region(),
            &payload.binance_uid,
            &payload.account_id,
            payload.credential_revision,
        )
        .await?;
    state
        .binance_settlement
        .consume_verification_attempt(&format!("binding-confirm:{}", session.user_id))
        .await?;
    if let Err(error) = state
        .binance_settlement
        .client
        .verify_permissions(&payload.credentials)
        .await
    {
        state.binance_settlement.observe_api_error(&error).await;
        return Err(map_verification_error(error));
    }
    let aad = credential_aad(
        &payload.account_id,
        &session.user_id,
        payload.credential_revision,
    );
    let (ciphertext, nonce) = cipher.seal_json(&payload.credentials, aad.as_bytes())?;
    state
        .store
        .binance_save_verified_account(
            &session.user_id,
            &session.email,
            &payload.account_id,
            state.binance_settlement.payment_home_region(),
            &payload.binance_uid,
            &payload.credentials.api_key,
            &ciphertext,
            &nonce,
            cipher.version(),
            payload.credential_revision,
            binding_mode(state.binance_settlement.mode()),
            &payload.verification,
        )
        .await?;
    Ok(Json(account_status(&state, &session).await?))
}

async fn verify_stored_account(
    state: &ServerState,
    session: &AuthSession,
    enable_automation: bool,
) -> Result<(), AppError> {
    let cipher = if enable_automation {
        state.binance_settlement.require_payment_available().await?
    } else {
        state.binance_settlement.require_binance_available().await?
    };
    let stored = state
        .store
        .binance_load_payment_account(
            &session.user_id,
            state.binance_settlement.payment_home_region(),
        )
        .await?
        .ok_or_else(|| AppError::NotFound("Binance payment account not found".into()))?;
    let account_id = stored.id.clone();
    let credential_revision = stored.credential_revision;
    let envelope = match decode_account_credentials(stored, cipher) {
        Ok(envelope) => envelope,
        Err(error) => {
            state
                .store
                .binance_mark_account_verification_failed(
                    &session.user_id,
                    state.binance_settlement.payment_home_region(),
                    &account_id,
                    credential_revision,
                    "CREDENTIAL_DECRYPT_FAILED",
                )
                .await?;
            return Err(error);
        }
    };
    state
        .binance_settlement
        .consume_verification_attempt(&session.user_id)
        .await?;
    let verification = match state
        .binance_settlement
        .client
        .verify_credentials(&envelope.credentials, &envelope.account.binance_uid)
        .await
    {
        Ok(verification) => verification,
        Err(error) => {
            state.binance_settlement.observe_api_error(&error).await;
            state
                .store
                .binance_mark_account_verification_failed(
                    &session.user_id,
                    state.binance_settlement.payment_home_region(),
                    &envelope.account.id,
                    envelope.account.credential_revision,
                    &error.code,
                )
                .await?;
            return Err(map_verification_error(error));
        }
    };
    if enable_automation {
        state
            .store
            .binance_enable_payment_account(
                &session.user_id,
                state.binance_settlement.payment_home_region(),
                &envelope.account.id,
                envelope.account.credential_revision,
                &verification,
            )
            .await?;
    } else {
        state
            .store
            .binance_mark_account_verified(
                &session.user_id,
                state.binance_settlement.payment_home_region(),
                &envelope.account.id,
                envelope.account.credential_revision,
                None,
                &verification,
            )
            .await?;
    }
    Ok(())
}

async fn verify_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BinanceAccountStatusResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    verify_stored_account(&state, &session, false).await?;
    Ok(Json(account_status(&state, &session).await?))
}

async fn enable_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BinanceAccountStatusResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    verify_stored_account(&state, &session, true).await?;
    Ok(Json(account_status(&state, &session).await?))
}

async fn disable_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BinanceAccountStatusResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .binance_disable_payment_account(
            &session.user_id,
            state.binance_settlement.payment_home_region(),
            false,
        )
        .await?;
    Ok(Json(account_status(&state, &session).await?))
}

async fn delete_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BinanceAccountStatusResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .binance_disable_payment_account(
            &session.user_id,
            state.binance_settlement.payment_home_region(),
            true,
        )
        .await?;
    Ok(Json(account_status(&state, &session).await?))
}

async fn create_payment_intent(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(invoice_id): Path<String>,
) -> Result<Json<BinancePaymentIntentView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let cipher = state.binance_settlement.require_payment_available().await?;
    Ok(Json(
        state
            .store
            .binance_create_or_refresh_intent_for_cipher(
                &session,
                &invoice_id,
                state.binance_settlement.payment_home_region(),
                cipher,
                false,
            )
            .await?,
    ))
}

async fn refresh_payment_intent(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(invoice_id): Path<String>,
) -> Result<Json<BinancePaymentIntentView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let cipher = state.binance_settlement.require_payment_available().await?;
    Ok(Json(
        state
            .store
            .binance_create_or_refresh_intent_for_cipher(
                &session,
                &invoice_id,
                state.binance_settlement.payment_home_region(),
                cipher,
                true,
            )
            .await?,
    ))
}

async fn get_payment_intent(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(invoice_id): Path<String>,
) -> Result<Json<Option<BinancePaymentIntentView>>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .binance_intent_for_invoice(&session, &invoice_id)
            .await?,
    ))
}

async fn cancel_payment_intent(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(invoice_id): Path<String>,
) -> Result<Json<BinancePaymentIntentView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .binance_cancel_intent(&session, &invoice_id)
            .await?,
    ))
}

async fn create_funding_intent(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateFundingIntentRequest>,
) -> Result<Json<BinanceFundingIntentView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let cipher = state.binance_settlement.require_payment_available().await?;
    Ok(Json(
        state
            .store
            .binance_create_funding_intent_for_cipher(
                &session,
                input.supplier_user_id.trim(),
                input.amount_minor,
                &input.idempotency_key,
                state.binance_settlement.payment_home_region(),
                cipher,
            )
            .await?,
    ))
}

async fn get_funding_intent(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(intent_id): Path<String>,
) -> Result<Json<BinanceFundingIntentView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let intent = state
        .store
        .binance_funding_intent(&session, &intent_id)
        .await?;
    if intent.status == "pending" {
        let cipher = state.binance_settlement.require_payment_available().await?;
        if !state
            .store
            .binance_supplier_funding_available_for_cipher(
                &intent.supplier_user_id,
                state.binance_settlement.payment_home_region(),
                cipher,
            )
            .await?
        {
            return Err(AppError::ServiceUnavailable(
                "the supplier Binance funding account is not currently available".into(),
            ));
        }
    }
    Ok(Json(intent))
}

async fn cancel_funding_intent(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(intent_id): Path<String>,
) -> Result<Json<BinanceFundingIntentView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .binance_cancel_funding_intent(&session, &intent_id)
            .await?,
    ))
}

async fn refresh_funding_intent(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(intent_id): Path<String>,
) -> Result<Json<BinanceFundingIntentView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let cipher = state.binance_settlement.require_payment_available().await?;
    Ok(Json(
        state
            .store
            .binance_refresh_funding_intent_for_cipher(
                &session,
                &intent_id,
                state.binance_settlement.payment_home_region(),
                cipher,
            )
            .await?,
    ))
}

async fn get_admin_reconciliation(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BinanceSettlementAdminView>, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    Ok(Json(state.store.binance_admin_reconciliation().await?))
}

async fn resolve_admin_reconciliation(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(case_id): Path<String>,
    Json(input): Json<ResolveReconciliationRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    let resolution = input.resolution.trim().to_ascii_lowercase();
    let invoice_id = input
        .invoice_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if invoice_id.is_some_and(|value| value.len() > 200) {
        return Err(AppError::BadRequest("invoiceId is too long".into()));
    }
    let note = input
        .note
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if note.is_some_and(|value| value.chars().count() > 2_000) {
        return Err(AppError::BadRequest("note is too long".into()));
    }
    let actions = state
        .store
        .binance_resolve_reconciliation_case(&session, &case_id, &resolution, invoice_id, note)
        .await?;
    crate::market_billing::dispatch_actions(&state, actions).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn run_service(
    state: ServerState,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    if state.binance_settlement.mode() == GlobalMode::Disabled {
        state
            .store
            .binance_cancel_live_intents_for_global_disable()
            .await?;
        return Ok(());
    }
    if *shutdown.borrow() {
        state
            .store
            .binance_release_poll_leases(&state.binance_settlement.worker_id)
            .await?;
        return Ok(());
    }
    let mut interval = tokio::time::interval(StdDuration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_health_reconciliation = Instant::now()
        .checked_sub(StdDuration::from_secs(60))
        .unwrap_or_else(Instant::now);
    'service: loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break 'service;
                }
            }
            _ = interval.tick() => {
                tokio::select! {
                    biased;
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break 'service;
                        }
                    }
                    _ = run_poll_cycle(&state, &mut last_health_reconciliation) => {}
                }
            }
        }
    }
    state
        .store
        .binance_release_poll_leases(&state.binance_settlement.worker_id)
        .await?;
    Ok(())
}

async fn run_poll_cycle(state: &ServerState, last_health_reconciliation: &mut Instant) {
    if let Err(error) = state.store.binance_expire_due_intents().await {
        tracing::warn!(error = %error, "expire Binance payment intents failed");
    }
    if last_health_reconciliation.elapsed() >= StdDuration::from_secs(60) {
        if let Err(error) = state.store.binance_reconcile_poll_health_alerts().await {
            tracing::warn!(error = %error, "reconcile Binance poll health alerts failed");
        }
        *last_health_reconciliation = Instant::now();
    }
    if state
        .binance_settlement
        .service_availability(false)
        .await
        .status
        != BinanceServiceAvailability::Available
    {
        return;
    }
    let mut claimed = Vec::new();
    for _ in 0..MAX_POLL_ACCOUNTS_PER_CYCLE {
        match state
            .store
            .binance_claim_poll_account(
                state.binance_settlement.payment_home_region(),
                &state.binance_settlement.worker_id,
            )
            .await
        {
            Ok(Some(account)) => claimed.push(account),
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(error = %error, "claim Binance poll account failed");
                break;
            }
        }
    }
    stream::iter(claimed)
        .for_each_concurrent(MAX_POLL_ACCOUNTS_PER_CYCLE, |account| async move {
            match poll_one_account(state, account).await {
                Ok(actions) => crate::market_billing::dispatch_actions(state, actions).await,
                Err(error) => tracing::warn!(error = %error, "Binance settlement poll failed"),
            }
        })
        .await;
}

async fn poll_one_account(
    state: &ServerState,
    account: StoredPaymentAccount,
) -> Result<Vec<crate::market_billing::BillingAction>, AppError> {
    let cipher = state.binance_settlement.cipher()?;
    let envelope = match decode_account_credentials(account.clone(), cipher) {
        Ok(envelope) => envelope,
        Err(error) => {
            state
                .store
                .binance_record_poll_failure(
                    &account.id,
                    &state.binance_settlement.worker_id,
                    account.credential_revision,
                    "CREDENTIAL_DECRYPT_FAILED",
                    None,
                )
                .await?;
            return Err(error);
        }
    };
    let permissions_stale =
        !permission_verification_is_fresh(account.permissions_verified_at.as_deref(), Utc::now());
    if permissions_stale {
        let verification = match state
            .binance_settlement
            .client
            .verify_credentials(&envelope.credentials, &account.binance_uid)
            .await
        {
            Ok(verification) => verification,
            Err(error) => {
                state.binance_settlement.observe_api_error(&error).await;
                tracing::warn!(
                    account_id = %account.id,
                    error_code = %error.code,
                    "periodic Binance permission verification failed"
                );
                state
                    .store
                    .binance_record_poll_failure(
                        &account.id,
                        &state.binance_settlement.worker_id,
                        account.credential_revision,
                        &error.code,
                        error.retry_after_secs,
                    )
                    .await?;
                return Ok(Vec::new());
            }
        };
        if let Err(error) = state
            .store
            .binance_mark_account_verified(
                &account.supplier_user_id,
                state.binance_settlement.payment_home_region(),
                &account.id,
                account.credential_revision,
                Some(state.binance_settlement.worker_id.as_ref()),
                &verification,
            )
            .await
        {
            state
                .store
                .binance_record_poll_failure(
                    &account.id,
                    &state.binance_settlement.worker_id,
                    account.credential_revision,
                    "BINANCE_PERMISSION_VERIFICATION_STALE",
                    None,
                )
                .await?;
            return Err(error);
        }
    }
    let now_ms = Utc::now().timestamp_millis();
    let (start_ms, target_ms) = select_poll_scan_window(&account, now_ms);
    let transactions = match fetch_transaction_window(
        state,
        &account,
        &state.binance_settlement.client,
        &envelope.credentials,
        start_ms,
        target_ms,
    )
    .await
    {
        Ok(transactions) => transactions,
        Err(error) => {
            state.binance_settlement.observe_api_error(&error).await;
            tracing::warn!(
                account_id = %account.id,
                error_code = %error.code,
                "Binance Pay transaction query failed"
            );
            state
                .store
                .binance_record_poll_failure(
                    &account.id,
                    &state.binance_settlement.worker_id,
                    account.credential_revision,
                    &error.code,
                    error.retry_after_secs,
                )
                .await?;
            return Ok(Vec::new());
        }
    };
    let checkpoint = if transactions.complete {
        PollScanCheckpoint {
            completed_cursor_at: Some(timestamp_text(target_ms)?),
            scan_cursor_at: None,
            scan_target_at: None,
        }
    } else {
        PollScanCheckpoint {
            completed_cursor_at: None,
            scan_cursor_at: Some(timestamp_text(
                transactions.covered_through_ms.saturating_add(1),
            )?),
            scan_target_at: Some(timestamp_text(target_ms)?),
        }
    };
    let result = state
        .store
        .binance_process_poll_batch(
            &account,
            &state.binance_settlement.worker_id,
            &transactions.transactions,
            cipher,
            state.binance_settlement.mode() == GlobalMode::Enabled,
            state.binance_settlement.poll_interval_secs,
            &checkpoint,
        )
        .await;
    match result {
        Ok(actions) => Ok(actions),
        Err(error) => {
            state
                .store
                .binance_record_poll_failure(
                    &account.id,
                    &state.binance_settlement.worker_id,
                    account.credential_revision,
                    "BINANCE_POLL_PROCESSING_FAILED",
                    None,
                )
                .await?;
            Err(error)
        }
    }
}

async fn fetch_transaction_window(
    state: &ServerState,
    account: &StoredPaymentAccount,
    client: &BinanceClient,
    credentials: &BinanceCredentials,
    start_ms: i64,
    end_ms: i64,
) -> Result<FetchedTransactionWindow, BinanceApiError> {
    let mut collector = AdaptiveWindowCollector::new(start_ms, end_ms)?;
    let mut previous_query_started_at: Option<Instant> = None;
    for _ in 0..MAX_TRANSACTION_QUERIES_PER_POLL {
        let Some(window) = collector.next_window() else {
            break;
        };
        if let Some(previous_query_started_at) = previous_query_started_at {
            let remaining =
                MIN_TRANSACTION_QUERY_SPACING.saturating_sub(previous_query_started_at.elapsed());
            tokio::time::sleep(remaining).await;
        }
        if let Err(error) = state
            .store
            .binance_renew_poll_lease(
                &account.id,
                &state.binance_settlement.worker_id,
                account.credential_revision,
            )
            .await
        {
            let error_code = if matches!(error, AppError::Conflict(_)) {
                "BINANCE_POLL_LEASE_LOST"
            } else {
                "BINANCE_POLL_LEASE_RENEW_FAILED"
            };
            tracing::warn!(
                account_id = %account.id,
                %error,
                error_code,
                "renew Binance poll lease before transaction query failed"
            );
            return Err(binance_api_error(error_code, Some(4)));
        }
        previous_query_started_at = Some(Instant::now());
        let page = client
            .pay_transactions(
                credentials,
                window.start_ms,
                window.end_ms,
                TRANSACTION_PAGE_LIMIT,
            )
            .await?;
        collector.accept_page(window, page)?;
    }
    collector.finish()
}

#[derive(Debug, Clone, Copy)]
struct TimeWindow {
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug)]
struct FetchedTransactionWindow {
    transactions: Vec<BinancePayTransaction>,
    covered_through_ms: i64,
    complete: bool,
}

struct AdaptiveWindowCollector {
    start_ms: i64,
    end_ms: i64,
    pending: Vec<TimeWindow>,
    transactions: Vec<BinancePayTransaction>,
    seen_transaction_ids: HashSet<String>,
    covered_through_ms: Option<i64>,
}

impl AdaptiveWindowCollector {
    fn new(start_ms: i64, end_ms: i64) -> Result<Self, BinanceApiError> {
        if start_ms > end_ms {
            return Err(binance_api_error("BINANCE_POLL_WINDOW_INVALID", None));
        }
        Ok(Self {
            start_ms,
            end_ms,
            pending: vec![TimeWindow { start_ms, end_ms }],
            transactions: Vec::new(),
            seen_transaction_ids: HashSet::new(),
            covered_through_ms: None,
        })
    }

    fn next_window(&mut self) -> Option<TimeWindow> {
        self.pending.pop()
    }

    fn accept_page(
        &mut self,
        window: TimeWindow,
        page: Vec<BinancePayTransaction>,
    ) -> Result<(), BinanceApiError> {
        if page.iter().any(|transaction| {
            transaction.transaction_id.trim().is_empty()
                || transaction.transaction_time < window.start_ms
                || transaction.transaction_time > window.end_ms
        }) {
            return Err(binance_api_error("BINANCE_RESPONSE_INVALID", None));
        }
        if page.len() >= TRANSACTION_PAGE_LIMIT {
            if window.start_ms == window.end_ms {
                return Err(binance_api_error(
                    "BINANCE_TIMESTAMP_SATURATED",
                    Some(60 * 60),
                ));
            }
            let midpoint = i64::try_from(
                i128::from(window.start_ms)
                    + (i128::from(window.end_ms) - i128::from(window.start_ms)) / 2,
            )
            .map_err(|_| binance_api_error("BINANCE_POLL_WINDOW_INVALID", None))?;
            self.pending.push(TimeWindow {
                start_ms: midpoint.saturating_add(1),
                end_ms: window.end_ms,
            });
            self.pending.push(TimeWindow {
                start_ms: window.start_ms,
                end_ms: midpoint,
            });
            return Ok(());
        }
        let expected_start = self
            .covered_through_ms
            .map(|value| value.saturating_add(1))
            .unwrap_or(self.start_ms);
        if window.start_ms != expected_start {
            return Err(binance_api_error("BINANCE_POLL_WINDOW_INVALID", None));
        }
        for transaction in page {
            if self
                .seen_transaction_ids
                .insert(transaction.transaction_id.clone())
            {
                self.transactions.push(transaction);
            }
        }
        self.covered_through_ms = Some(window.end_ms);
        Ok(())
    }

    fn finish(mut self) -> Result<FetchedTransactionWindow, BinanceApiError> {
        let complete = self.pending.is_empty();
        let covered_through_ms = self
            .covered_through_ms
            .ok_or_else(|| binance_api_error("BINANCE_PAGINATION_BUDGET_REACHED", Some(15)))?;
        if complete && covered_through_ms != self.end_ms {
            return Err(binance_api_error("BINANCE_POLL_WINDOW_INVALID", None));
        }
        self.transactions.sort_by(|left, right| {
            left.transaction_time
                .cmp(&right.transaction_time)
                .then_with(|| left.transaction_id.cmp(&right.transaction_id))
        });
        Ok(FetchedTransactionWindow {
            transactions: self.transactions,
            covered_through_ms,
            complete,
        })
    }
}

fn binance_api_error(code: &str, retry_after_secs: Option<u64>) -> BinanceApiError {
    BinanceApiError {
        code: code.into(),
        retry_after_secs,
    }
}

fn timestamp_ms(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis())
}

fn timestamp_text(value: i64) -> Result<String, AppError> {
    Utc.timestamp_millis_opt(value)
        .single()
        .map(|value| value.to_rfc3339())
        .ok_or_else(|| AppError::Internal("Binance poll checkpoint is invalid".into()))
}

fn select_poll_scan_window(account: &StoredPaymentAccount, now_ms: i64) -> (i64, i64) {
    if let (Some(scan_cursor_ms), Some(scan_target_ms)) = (
        timestamp_ms(account.poll_scan_cursor_at.as_deref()),
        timestamp_ms(account.poll_scan_target_at.as_deref()),
    ) && scan_cursor_ms <= scan_target_ms
        && scan_target_ms <= now_ms
    {
        return (scan_cursor_ms, scan_target_ms);
    }
    let cursor_ms = select_poll_cursor_ms(
        timestamp_ms(account.poll_cursor_at.as_deref()),
        timestamp_ms(account.active_intent_started_at.as_deref()),
        now_ms,
    );
    let start_ms = cursor_ms
        .saturating_sub(POLL_OVERLAP_MS)
        .max(now_ms.saturating_sub(90 * 24 * 60 * 60 * 1_000));
    let target_ms = start_ms
        .saturating_add(MAX_POLL_SCAN_WINDOW_MS)
        .min(now_ms)
        .max(start_ms);
    (start_ms, target_ms)
}

fn select_poll_cursor_ms(
    stored_cursor_ms: Option<i64>,
    active_intent_started_ms: Option<i64>,
    now_ms: i64,
) -> i64 {
    stored_cursor_ms
        .max(active_intent_started_ms)
        .unwrap_or_else(|| now_ms.saturating_sub(30 * 60 * 1_000))
        .min(now_ms)
}

fn map_verification_error(error: BinanceApiError) -> AppError {
    match error.code.as_str() {
        "BINANCE_REGION_RESTRICTED" => binance_region_restricted_error(),
        "READ_PERMISSION_REQUIRED"
        | "DANGEROUS_PERMISSION_ENABLED"
        | "ACCOUNT_UID_UNCONFIRMED"
        | "ACCOUNT_UID_AMBIGUOUS"
        | "ACCOUNT_UID_MISMATCH"
        | "RECEIVER_UID_MISMATCH"
        | "BINANCE_CREDENTIALS_REJECTED" => AppError::UnprocessableEntity(format!(
            "Binance credential verification failed: {}",
            error.code
        )),
        "BINANCE_RATE_LIMITED" | "BINANCE_IP_BANNED" => {
            let default_retry = if error.code == "BINANCE_IP_BANNED" {
                60 * 60
            } else {
                60
            };
            AppError::RateLimited {
                message: "Binance credential verification is temporarily rate limited".into(),
                retry_after_secs: error
                    .retry_after_secs
                    .unwrap_or(default_retry)
                    .clamp(1, 3 * 24 * 60 * 60),
            }
        }
        _ => AppError::ServiceUnavailable(
            "Binance credential verification is temporarily unavailable".into(),
        ),
    }
}

fn binance_region_restricted_error() -> AppError {
    AppError::Coded {
        status: StatusCode::UNAVAILABLE_FOR_LEGAL_REASONS,
        code: "BINANCE_REGION_RESTRICTED",
        message: "Binance is unavailable from this Router's network region".into(),
        details: serde_json::json!({
            "serviceAvailability": BinanceServiceAvailability::RegionRestricted.as_str(),
        }),
    }
}

fn binance_temporarily_unavailable_error(reason: Option<&str>) -> AppError {
    AppError::Coded {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "BINANCE_TEMPORARILY_UNAVAILABLE",
        message: "Binance is temporarily unreachable from this Router".into(),
        details: serde_json::json!({
            "serviceAvailability": BinanceServiceAvailability::TemporarilyUnavailable.as_str(),
            "reason": reason.unwrap_or("BINANCE_UNAVAILABLE"),
        }),
    }
}

fn require_initial_uid_confirmation(
    verification: &self::client::VerificationResult,
) -> Result<(), AppError> {
    if !verification.uid_confirmed {
        return Err(AppError::UnprocessableEntity(
            "Binance credential verification failed: ACCOUNT_UID_UNCONFIRMED; receive a small Binance Pay transfer, then bind again"
                .into(),
        ));
    }
    Ok(())
}

fn permission_verification_is_fresh(value: Option<&str>, now: chrono::DateTime<Utc>) -> bool {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|verified_at| {
            let verified_at = verified_at.with_timezone(&Utc);
            verified_at <= now + chrono::Duration::minutes(5)
                && verified_at > now - chrono::Duration::hours(PERMISSION_REVERIFY_HOURS)
        })
}

fn parse_master_key(value: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let trimmed = value.trim();
    let decoded = Zeroizing::new(
        if trimmed.len() == 64 && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            hex::decode(trimmed).context("invalid hexadecimal Binance master key")?
        } else {
            base64::engine::general_purpose::STANDARD
                .decode(trimmed)
                .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(trimmed))
                .context("Binance master key must be 32 bytes encoded as hex or base64")?
        },
    );
    if decoded.len() != 32 {
        bail!("Binance master key must decode to exactly 32 bytes");
    }
    let mut key = Zeroizing::new([0_u8; 32]);
    key.copy_from_slice(&decoded);
    Ok(key)
}

fn load_or_create_master_key(
    data_dir: &FsPath,
    configured: Option<&str>,
) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    if let Some(value) = configured.filter(|value| !value.trim().is_empty()) {
        return parse_master_key(&value)
            .context("invalid CC_SWITCH_ROUTER_BINANCE_MASTER_KEY override");
    }

    let path = data_dir.join(MASTER_KEY_FILE);
    if !path.exists() {
        let mut generated = Zeroizing::new([0_u8; 32]);
        rand::rngs::OsRng.fill_bytes(generated.as_mut());
        let encoded = Zeroizing::new(format!("{}\n", hex::encode(generated.as_ref())));
        crate::secure_file::atomic_create_file_mode(&path, encoded.as_bytes(), 0o600)
            .with_context(|| format!("create Binance master key: {}", path.display()))?;
    }
    crate::secure_file::enforce_file_mode(&path, 0o600)
        .with_context(|| format!("protect Binance master key: {}", path.display()))?;
    let encoded = Zeroizing::new(
        fs::read_to_string(&path)
            .with_context(|| format!("read Binance master key: {}", path.display()))?,
    );
    parse_master_key(&encoded)
        .with_context(|| format!("invalid Binance master key file: {}", path.display()))
}

pub(crate) fn normalize_payment_home_region(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("Binance payment-home Region must not be empty");
    }
    if value.len() > 128 {
        bail!("Binance payment-home Region must be at most 128 characters");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        bail!(
            "Binance payment-home Region may only contain ASCII letters, digits, dot, underscore, hyphen, and colon"
        );
    }
    Ok(value.to_ascii_lowercase())
}

pub(crate) fn validate_api_base(url: &Url) -> anyhow::Result<()> {
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Binance API base must not contain credentials, a query, or a fragment");
    }
    if url.path() != "/" {
        bail!("Binance API base must not contain a path");
    }
    let loopback = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if matches!(url.scheme(), "http" | "https") && loopback {
        return Ok(());
    }
    if url.scheme() != "https" {
        bail!("Binance API base must use HTTPS (HTTP is allowed only for loopback tests)");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Binance API base must contain a host"))?;
    if !OFFICIAL_BINANCE_API_HOSTS
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
    {
        bail!("Binance API base must use an approved official Binance API host");
    }
    if url.port_or_known_default() != Some(443) {
        bail!("Binance API base must use the standard HTTPS port");
    }
    Ok(())
}

pub(crate) fn validate_binance_socks_proxy_url(value: &str) -> anyhow::Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    if value.len() > 2_048 {
        bail!("Binance SOCKS proxy URL must be at most 2048 characters");
    }
    let url = Url::parse(value).context("Binance SOCKS proxy must be a valid URL")?;
    if url.scheme() != "socks5h" {
        bail!("Binance SOCKS proxy must use socks5h:// so DNS is resolved by the proxy");
    }
    if url.host_str().is_none() || !url.port().is_some_and(|port| port > 0) {
        bail!("Binance SOCKS proxy must include a host and port");
    }
    if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
        bail!("Binance SOCKS proxy must not contain a path, query, or fragment");
    }
    if url.username().is_empty() && url.password().is_some() {
        bail!("Binance SOCKS proxy cannot contain a password without a username");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session(user_id: &str) -> AuthSession {
        let now = Utc::now();
        AuthSession {
            session_id: format!("session-{user_id}"),
            user_id: user_id.into(),
            email: format!("{user_id}@example.com"),
            auth_source_kind: "auth_device".into(),
            auth_source_id: format!("browser-{user_id}"),
            access_token_hash: format!("access-{user_id}"),
            refresh_token_hash: format!("refresh-{user_id}"),
            access_expires_at: now + chrono::Duration::hours(1),
            refresh_expires_at: now + chrono::Duration::days(30),
            created_at: now,
            last_used_at: now,
        }
    }

    fn binding_confirmation_payload(
        session: &AuthSession,
        region: &str,
        issued_at_ms: i64,
    ) -> BindingConfirmationPayload {
        BindingConfirmationPayload {
            version: BINDING_CONFIRMATION_VERSION,
            user_id: session.user_id.clone(),
            payment_home_region: region.into(),
            account_id: Uuid::new_v4().to_string(),
            credential_revision: 1,
            binance_uid: "123456789".into(),
            credentials: BinanceCredentials {
                api_key: "confirmation-api-key-0123456789".into(),
                api_secret: "confirmation-api-secret-0123456789".into(),
            },
            verification: self::client::VerificationResult {
                reading_enabled: true,
                dangerous_permissions_disabled: true,
                uid_confirmed: true,
                uid_confirmation_source: Some(self::client::UidConfirmationSource::ReceiverHistory),
            },
            issued_at_ms,
            expires_at_ms: issued_at_ms + BINDING_CONFIRMATION_TTL_SECS * 1_000,
        }
    }

    fn synthetic_transaction(index: usize, transaction_time: i64) -> BinancePayTransaction {
        BinancePayTransaction {
            order_id: format!("order-{index}"),
            note: String::new(),
            order_type: "C2C".into(),
            transaction_id: format!("transaction-{index}"),
            transaction_time,
            amount: "1.0001".into(),
            currency: "USDT".into(),
            counterparty_id: serde_json::json!(index + 10_000),
            payer_info: super::client::PartyInfo::default(),
            receiver_info: super::client::PartyInfo::default(),
        }
    }

    fn collect_synthetic_window(
        transactions: &[BinancePayTransaction],
        start_ms: i64,
        end_ms: i64,
        query_budget: usize,
    ) -> Result<FetchedTransactionWindow, BinanceApiError> {
        let mut collector = AdaptiveWindowCollector::new(start_ms, end_ms)?;
        for _ in 0..query_budget {
            let Some(window) = collector.next_window() else {
                break;
            };
            let page = transactions
                .iter()
                .filter(|transaction| {
                    (window.start_ms..=window.end_ms).contains(&transaction.transaction_time)
                })
                .take(TRANSACTION_PAGE_LIMIT)
                .cloned()
                .collect();
            collector.accept_page(window, page)?;
        }
        collector.finish()
    }

    #[test]
    fn master_key_accepts_hex_and_rejects_short_values() {
        assert_eq!(*parse_master_key(&"ab".repeat(32)).unwrap(), [0xab; 32]);
        assert!(
            parse_master_key(&base64::engine::general_purpose::STANDARD.encode([7; 32])).is_ok()
        );
        assert!(parse_master_key("short").is_err());
    }

    #[test]
    fn binding_confirmation_round_trips_without_exposing_credentials() {
        let cipher = CredentialCipher::new([17; 32], 1);
        let session = test_session("binding-owner");
        let payload = binding_confirmation_payload(&session, "test", Utc::now().timestamp_millis());

        let token = seal_binding_confirmation(&cipher, &payload).expect("seal confirmation");
        assert!(!token.contains(&payload.credentials.api_key));
        assert!(!token.contains(&payload.credentials.api_secret));

        let opened = open_binding_confirmation(&cipher, &token, &session, "test")
            .expect("open confirmation");
        assert_eq!(opened.user_id, session.user_id);
        assert_eq!(opened.payment_home_region, "test");
        assert_eq!(opened.binance_uid, "123456789");
        assert_eq!(opened.credentials.api_key, payload.credentials.api_key);
        assert_eq!(
            opened.credentials.api_secret,
            payload.credentials.api_secret
        );
    }

    #[test]
    fn binding_confirmation_rejects_tampering_and_scope_changes() {
        let cipher = CredentialCipher::new([18; 32], 1);
        let session = test_session("binding-owner");
        let payload = binding_confirmation_payload(&session, "test", Utc::now().timestamp_millis());
        let token = seal_binding_confirmation(&cipher, &payload).expect("seal confirmation");

        let mut tampered = token.clone();
        let replacement = if tampered.ends_with('A') { 'B' } else { 'A' };
        tampered.pop();
        tampered.push(replacement);
        assert!(matches!(
            open_binding_confirmation(&cipher, &tampered, &session, "test"),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            open_binding_confirmation(&cipher, &token, &test_session("other-owner"), "test"),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            open_binding_confirmation(&cipher, &token, &session, "other-region"),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn binding_confirmation_rejects_expired_evidence() {
        let cipher = CredentialCipher::new([19; 32], 1);
        let session = test_session("binding-owner");
        let issued_at_ms = (Utc::now()
            - chrono::Duration::seconds(BINDING_CONFIRMATION_TTL_SECS + 1))
        .timestamp_millis();
        let payload = binding_confirmation_payload(&session, "test", issued_at_ms);
        let token = seal_binding_confirmation(&cipher, &payload).expect("seal confirmation");

        assert!(matches!(
            open_binding_confirmation(&cipher, &token, &session, "test"),
            Err(AppError::BadRequest(message)) if message.contains("expired")
        ));
    }

    #[test]
    fn master_key_is_generated_once_with_private_permissions() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-binance-key-{}",
            uuid::Uuid::new_v4()
        ));
        let first = load_or_create_master_key(&root, None).expect("generate managed key");
        let second = load_or_create_master_key(&root, None).expect("reload managed key");
        assert_eq!(*first, *second);
        let path = root.join(MASTER_KEY_FILE);
        assert_eq!(fs::read_to_string(&path).unwrap().trim().len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn configured_master_key_override_does_not_create_a_file() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-binance-key-override-{}",
            uuid::Uuid::new_v4()
        ));
        let key = load_or_create_master_key(&root, Some(&"cd".repeat(32)))
            .expect("load configured key override");
        assert_eq!(*key, [0xcd; 32]);
        assert!(!root.join(MASTER_KEY_FILE).exists());
    }

    #[test]
    fn payment_home_region_is_stable_and_rejects_unsafe_values() {
        assert_eq!(
            normalize_payment_home_region(" Region-A:443 ").unwrap(),
            "region-a:443"
        );
        assert!(normalize_payment_home_region("").is_err());
        assert!(normalize_payment_home_region("region a").is_err());
        assert!(normalize_payment_home_region(&"a".repeat(129)).is_err());
    }

    #[test]
    fn a_new_live_intent_fences_an_old_poll_cursor() {
        assert_eq!(select_poll_cursor_ms(Some(100), Some(200), 300), 200);
        assert_eq!(select_poll_cursor_ms(Some(250), Some(200), 300), 250);
        assert_eq!(select_poll_cursor_ms(Some(400), Some(200), 300), 300);
        assert_eq!(select_poll_cursor_ms(None, None, 2_000_000), 200_000);
    }

    #[test]
    fn adaptive_windows_cover_full_page_boundaries_without_skipping() {
        for count in [100_usize, 101, 1_000, 1_001] {
            let transactions = (0..count)
                .map(|index| synthetic_transaction(index, index as i64))
                .collect::<Vec<_>>();
            let result =
                collect_synthetic_window(&transactions, 0, i64::try_from(count).unwrap(), 128)
                    .expect("adaptive window should enumerate every transaction");
            assert!(result.complete, "count={count}");
            assert_eq!(result.covered_through_ms, count as i64);
            assert_eq!(result.transactions.len(), count, "count={count}");
            assert_eq!(
                result
                    .transactions
                    .iter()
                    .map(|transaction| transaction.transaction_id.as_str())
                    .collect::<HashSet<_>>()
                    .len(),
                count,
                "count={count}"
            );
        }
    }

    #[test]
    fn a_busy_window_resumes_from_the_last_complete_time_partition() {
        let transactions = (0..1_001)
            .map(|index| synthetic_transaction(index, index as i64))
            .collect::<Vec<_>>();
        let mut next_start = 0_i64;
        let mut observed = HashSet::new();
        for _ in 0..32 {
            let result = collect_synthetic_window(&transactions, next_start, 1_001, 6)
                .expect("each bounded pass should complete a contiguous prefix");
            for transaction in result.transactions {
                assert!(observed.insert(transaction.transaction_id));
            }
            if result.complete {
                next_start = 1_002;
                break;
            }
            let resumed = result.covered_through_ms.saturating_add(1);
            assert!(resumed > next_start);
            next_start = resumed;
        }
        assert_eq!(next_start, 1_002);
        assert_eq!(observed.len(), transactions.len());
    }

    #[test]
    fn duplicate_rows_are_deduplicated_but_a_saturated_millisecond_fails_closed() {
        let mut duplicate = synthetic_transaction(1, 10);
        duplicate.transaction_time = 11;
        let result = collect_synthetic_window(&[synthetic_transaction(1, 10), duplicate], 0, 20, 2)
            .expect("a non-saturated duplicate page is complete");
        assert_eq!(result.transactions.len(), 1);

        let saturated = (0..TRANSACTION_PAGE_LIMIT)
            .map(|index| synthetic_transaction(index, 10))
            .collect::<Vec<_>>();
        let error = collect_synthetic_window(&saturated, 10, 10, 1)
            .expect_err("a full one-millisecond window cannot prove completeness");
        assert_eq!(error.code, "BINANCE_TIMESTAMP_SATURATED");
        assert_eq!(error.retry_after_secs, Some(60 * 60));
    }

    #[test]
    fn api_base_is_fail_closed() {
        assert_eq!(binding_mode(GlobalMode::Disabled), "shadow");
        assert_eq!(binding_mode(GlobalMode::Shadow), "shadow");
        assert_eq!(binding_mode(GlobalMode::Enabled), "enabled");
        assert!(validate_api_base(&Url::parse("https://api.binance.com").unwrap()).is_ok());
        assert!(validate_api_base(&Url::parse("https://api1.binance.com").unwrap()).is_ok());
        assert!(validate_api_base(&Url::parse("http://127.0.0.1:9000").unwrap()).is_ok());
        assert!(validate_api_base(&Url::parse("http://[::1]:9000").unwrap()).is_ok());
        assert!(validate_api_base(&Url::parse("http://example.com").unwrap()).is_err());
        assert!(validate_api_base(&Url::parse("https://example.com").unwrap()).is_err());
        assert!(
            validate_api_base(&Url::parse("https://api.binance.com/redirect").unwrap()).is_err()
        );
        assert!(
            validate_api_base(&Url::parse("https://user:secret@api.binance.com").unwrap()).is_err()
        );
        assert!(
            validate_api_base(&Url::parse("https://api.binance.com?redirect=1").unwrap()).is_err()
        );
        assert!(validate_binance_socks_proxy_url("socks5h://127.0.0.1:1080").is_ok());
        assert!(
            validate_binance_socks_proxy_url("socks5h://user:secret@proxy.example:1080").is_ok()
        );
        for invalid in [
            "socks5://proxy.example:1080",
            "http://proxy.example:1080",
            "socks5h://proxy.example",
            "socks5h://proxy.example:0",
            "socks5h://proxy.example:1080/path",
            "socks5h://:secret@proxy.example:1080",
        ] {
            assert!(
                validate_binance_socks_proxy_url(invalid).is_err(),
                "proxy URL should be rejected: {invalid}"
            );
        }
        assert!(
            BinanceSettlementRuntime::disabled_for_tests()
                .require_binance_network_enabled()
                .is_err()
        );
        assert!(
            require_initial_uid_confirmation(&self::client::VerificationResult {
                reading_enabled: true,
                dangerous_permissions_disabled: true,
                uid_confirmed: false,
                uid_confirmation_source: None,
            })
            .is_err()
        );
        assert!(
            require_initial_uid_confirmation(&self::client::VerificationResult {
                reading_enabled: true,
                dangerous_permissions_disabled: true,
                uid_confirmed: true,
                uid_confirmation_source: Some(
                    self::client::UidConfirmationSource::ReceiverHistory,
                ),
            })
            .is_ok()
        );
        let now = Utc::now();
        assert!(permission_verification_is_fresh(
            Some(&(now - chrono::Duration::hours(1)).to_rfc3339()),
            now,
        ));
        assert!(!permission_verification_is_fresh(
            Some(&(now + chrono::Duration::minutes(6)).to_rfc3339()),
            now,
        ));
        assert!(!permission_verification_is_fresh(
            Some(&(now - chrono::Duration::hours(24)).to_rfc3339()),
            now,
        ));
    }

    #[tokio::test]
    async fn credential_verification_attempts_are_rate_limited_per_supplier() {
        let runtime = BinanceSettlementRuntime::disabled_for_tests();
        runtime
            .consume_verification_attempt("supplier-a")
            .await
            .expect("first verification attempt");
        assert!(matches!(
            runtime.consume_verification_attempt("supplier-a").await,
            Err(AppError::RateLimited { .. })
        ));
        runtime
            .consume_verification_attempt("supplier-b")
            .await
            .expect("another supplier has an independent limit");
    }
}
