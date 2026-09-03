mod client;
mod crypto;
mod store;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration as StdDuration;
use std::time::Instant;

use anyhow::{Context, bail};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use chrono::Utc;
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::ServerState;
use crate::error::AppError;
use crate::models::AuthSession;

use self::client::{BinanceApiError, BinanceClient, BinancePayTransaction};
use self::crypto::{BinanceCredentials, CredentialCipher, credential_aad};
pub(crate) use self::store::cancel_invoice_intents_tx;
pub use self::store::{
    BinancePaymentAccountView, BinancePaymentIntentView, BinanceSettlementAdminView,
};
use self::store::{
    StoredPaymentAccount, decode_account_credentials, validate_automation_mode,
    validate_credentials, validate_uid,
};

const MAX_POLL_ACCOUNTS_PER_CYCLE: usize = 8;
const MAX_TRANSACTION_PAGES: usize = 10;
const PERMISSION_REVERIFY_HOURS: i64 = 24;
const VERIFICATION_ATTEMPT_COOLDOWN_SECS: u64 = 30;
const MAX_VERIFICATION_ATTEMPT_SCOPES: usize = 10_000;
pub(crate) const MIN_POLL_INTERVAL_SECS: i64 = 2;
pub(crate) const MAX_POLL_INTERVAL_SECS: i64 = 60;
pub(crate) const MAX_MASTER_KEY_VERSION: i64 = 1_000_000;
const OFFICIAL_BINANCE_API_HOSTS: &[&str] = &[
    "api.binance.com",
    "api-gcp.binance.com",
    "api1.binance.com",
    "api2.binance.com",
    "api3.binance.com",
    "api4.binance.com",
];

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

#[derive(Clone)]
pub struct BinanceSettlementRuntime {
    mode: GlobalMode,
    cipher: Option<CredentialCipher>,
    client: BinanceClient,
    payment_home_region: Arc<str>,
    poll_interval_secs: i64,
    worker_id: Arc<str>,
    verification_attempts: Arc<tokio::sync::Mutex<HashMap<String, Instant>>>,
}

impl BinanceSettlementRuntime {
    pub fn from_env(default_region: &str) -> anyhow::Result<Self> {
        let mode = GlobalMode::parses(
            std::env::var("CC_SWITCH_ROUTER_BINANCE_AUTO_SETTLEMENT_MODE").ok(),
        )?;
        let key_text = std::env::var("CC_SWITCH_ROUTER_BINANCE_MASTER_KEY")
            .ok()
            .map(Zeroizing::new);
        let key = key_text
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| parse_master_key(value.as_str()))
            .transpose()?;
        if mode != GlobalMode::Disabled && key.is_none() {
            bail!(
                "CC_SWITCH_ROUTER_BINANCE_MASTER_KEY is required when Binance auto-settlement is not disabled"
            );
        }
        let key_version = std::env::var("CC_SWITCH_ROUTER_BINANCE_MASTER_KEY_VERSION")
            .ok()
            .map(|value| value.parse::<i64>())
            .transpose()
            .context("invalid CC_SWITCH_ROUTER_BINANCE_MASTER_KEY_VERSION")?
            .unwrap_or(1);
        if !(1..=MAX_MASTER_KEY_VERSION).contains(&key_version) {
            bail!(
                "CC_SWITCH_ROUTER_BINANCE_MASTER_KEY_VERSION must be between 1 and {MAX_MASTER_KEY_VERSION}"
            );
        }
        let base_url = std::env::var("CC_SWITCH_ROUTER_BINANCE_API_BASE")
            .unwrap_or_else(|_| "https://api.binance.com".into());
        let base_url = Url::parse(base_url.trim()).context("invalid Binance API base URL")?;
        validate_api_base(&base_url)?;
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
            .unwrap_or(4);
        if !(MIN_POLL_INTERVAL_SECS..=MAX_POLL_INTERVAL_SECS).contains(&poll_interval_secs) {
            bail!(
                "CC_SWITCH_ROUTER_BINANCE_POLL_INTERVAL_SECS must be between {MIN_POLL_INTERVAL_SECS} and {MAX_POLL_INTERVAL_SECS}"
            );
        }
        Ok(Self {
            mode,
            cipher: key.map(|key| CredentialCipher::from_zeroizing(key, key_version)),
            client: BinanceClient::new(base_url)?,
            payment_home_region: Arc::from(region),
            poll_interval_secs,
            worker_id: Arc::from(Uuid::new_v4().to_string()),
            verification_attempts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
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

    fn require_payment_enabled(&self) -> Result<&CredentialCipher, AppError> {
        if self.mode != GlobalMode::Enabled {
            return Err(AppError::ServiceUnavailable(
                "Binance auto-settlement is not enabled on this Router".into(),
            ));
        }
        self.cipher()
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
    account: Option<BinancePaymentAccountView>,
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindBinanceAccountRequest {
    binance_uid: String,
    api_key: String,
    api_secret: String,
    #[serde(default = "default_account_mode")]
    automation_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveReconciliationRequest {
    resolution: String,
    invoice_id: Option<String>,
    note: Option<String>,
}

fn default_account_mode() -> String {
    "enabled".into()
}

fn effective_binding_mode(global_mode: GlobalMode, requested_mode: &'static str) -> &'static str {
    if global_mode == GlobalMode::Enabled {
        requested_mode
    } else {
        "shadow"
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
            "/v1/account/binance-auto-settlement/disable",
            post(disable_account),
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
            "/v1/admin/market-billing/binance-reconciliation",
            get(get_admin_reconciliation),
        )
        .route(
            "/v1/admin/market-billing/binance-reconciliation/:case_id/resolve",
            post(resolve_admin_reconciliation),
        )
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
    Ok(BinanceAccountStatusResponse {
        global_mode: state.binance_settlement.mode().as_str(),
        credential_storage_configured: state.binance_settlement.cipher.is_some(),
        payment_home_region: state.binance_settlement.payment_home_region().to_string(),
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

async fn bind_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<BindBinanceAccountRequest>,
) -> Result<Json<BinanceAccountStatusResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let cipher = state.binance_settlement.require_binance_network_enabled()?;
    let binance_uid = validate_uid(&input.binance_uid)?;
    validate_credentials(&input.api_key, &input.api_secret)?;
    let requested_automation_mode = validate_automation_mode(&input.automation_mode)?;
    let automation_mode =
        effective_binding_mode(state.binance_settlement.mode(), requested_automation_mode);
    let (account_id, revision) = state
        .store
        .binance_prepare_account_binding(
            &session.user_id,
            state.binance_settlement.payment_home_region(),
            &binance_uid,
        )
        .await?;
    state
        .binance_settlement
        .consume_verification_attempt(&session.user_id)
        .await?;
    let credentials = BinanceCredentials {
        api_key: input.api_key.trim().to_string(),
        api_secret: input.api_secret.trim().to_string(),
    };
    let verification = state
        .binance_settlement
        .client
        .verify_credentials(&credentials, &binance_uid)
        .await
        .map_err(map_verification_error)?;
    require_initial_uid_confirmation(&verification)?;
    let aad = credential_aad(&account_id, &session.user_id, revision);
    let (ciphertext, nonce) = cipher.seal_json(&credentials, aad.as_bytes())?;
    state
        .store
        .binance_save_verified_account(
            &session.user_id,
            &account_id,
            state.binance_settlement.payment_home_region(),
            &binance_uid,
            &credentials.api_key,
            &ciphertext,
            &nonce,
            cipher.version(),
            revision,
            automation_mode,
            &verification,
        )
        .await?;
    Ok(Json(account_status(&state, &session).await?))
}

async fn verify_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BinanceAccountStatusResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let cipher = state.binance_settlement.require_binance_network_enabled()?;
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
    let cipher = state.binance_settlement.require_payment_enabled()?;
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
    let cipher = state.binance_settlement.require_payment_enabled()?;
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

pub async fn run_service(state: ServerState) -> anyhow::Result<()> {
    if state.binance_settlement.mode() == GlobalMode::Disabled {
        state
            .store
            .binance_cancel_live_intents_for_global_disable()
            .await?;
        return Ok(());
    }
    let mut interval = tokio::time::interval(StdDuration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = state.store.binance_expire_due_intents().await {
            tracing::warn!(error = %error, "expire Binance payment intents failed");
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
        let results = stream::iter(claimed)
            .map(|account| {
                let state = state.clone();
                async move { poll_one_account(&state, account).await }
            })
            .buffer_unordered(MAX_POLL_ACCOUNTS_PER_CYCLE)
            .collect::<Vec<_>>()
            .await;
        for result in results {
            match result {
                Ok(actions) => crate::market_billing::dispatch_actions(&state, actions).await,
                Err(error) => tracing::warn!(error = %error, "Binance settlement poll failed"),
            }
        }
    }
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
                    "BINANCE_PERMISSION_VERIFICATION_STALE",
                    None,
                )
                .await?;
            return Err(error);
        }
    }
    let now_ms = Utc::now().timestamp_millis();
    let stored_cursor_ms = account
        .poll_cursor_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis());
    let active_intent_started_ms = account
        .active_intent_started_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis());
    let cursor_ms = select_poll_cursor_ms(stored_cursor_ms, active_intent_started_ms, now_ms);
    let start_ms = cursor_ms
        .saturating_sub(10 * 60 * 1_000)
        .max(now_ms.saturating_sub(90 * 24 * 60 * 60 * 1_000));
    let transactions = match fetch_transaction_window(
        &state.binance_settlement.client,
        &envelope.credentials,
        start_ms,
        now_ms,
    )
    .await
    {
        Ok(transactions) => transactions,
        Err(error) => {
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
                    &error.code,
                    error.retry_after_secs,
                )
                .await?;
            return Ok(Vec::new());
        }
    };
    let result = state
        .store
        .binance_process_poll_success(
            &account,
            &state.binance_settlement.worker_id,
            &transactions,
            cipher,
            state.binance_settlement.mode() == GlobalMode::Enabled,
            state.binance_settlement.poll_interval_secs,
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
                    "BINANCE_POLL_PROCESSING_FAILED",
                    None,
                )
                .await?;
            Err(error)
        }
    }
}

async fn fetch_transaction_window(
    client: &BinanceClient,
    credentials: &BinanceCredentials,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<BinancePayTransaction>, BinanceApiError> {
    let mut all = Vec::new();
    let mut seen = HashSet::new();
    let mut page_end = end_ms;
    for _ in 0..MAX_TRANSACTION_PAGES {
        let page = client
            .pay_transactions(credentials, start_ms, page_end, 100)
            .await?;
        let mut minimum_time = None::<i64>;
        for transaction in &page {
            minimum_time = Some(
                minimum_time
                    .map(|current| current.min(transaction.transaction_time))
                    .unwrap_or(transaction.transaction_time),
            );
            if !transaction.transaction_id.trim().is_empty()
                && seen.insert(transaction.transaction_id.clone())
            {
                all.push(transaction.clone());
            }
        }
        if page.len() < 100 {
            all.sort_by_key(|transaction| transaction.transaction_time);
            return Ok(all);
        }
        let Some(minimum_time) = minimum_time else {
            break;
        };
        page_end = minimum_time.saturating_sub(1);
        if page_end <= start_ms {
            all.sort_by_key(|transaction| transaction.transaction_time);
            return Ok(all);
        }
    }
    Err(BinanceApiError {
        code: "BINANCE_PAGINATION_LIMIT_REACHED".into(),
        retry_after_secs: Some(60),
    })
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
        "READ_PERMISSION_REQUIRED"
        | "DANGEROUS_PERMISSION_ENABLED"
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

fn require_initial_uid_confirmation(
    verification: &self::client::VerificationResult,
) -> Result<(), AppError> {
    if !verification.uid_confirmed {
        return Err(AppError::UnprocessableEntity(
            "Binance credential verification failed: RECEIVER_UID_UNCONFIRMED; receive a small Binance Pay transfer, then bind again"
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

pub(crate) fn validate_master_key(value: &str) -> anyhow::Result<()> {
    let _ = parse_master_key(value)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_key_accepts_hex_and_rejects_short_values() {
        assert_eq!(*parse_master_key(&"ab".repeat(32)).unwrap(), [0xab; 32]);
        assert!(
            validate_master_key(&base64::engine::general_purpose::STANDARD.encode([7; 32])).is_ok()
        );
        assert!(parse_master_key("short").is_err());
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
    fn api_base_is_fail_closed() {
        assert_eq!(
            effective_binding_mode(GlobalMode::Shadow, "enabled"),
            "shadow"
        );
        assert_eq!(
            effective_binding_mode(GlobalMode::Enabled, "enabled"),
            "enabled"
        );
        assert_eq!(
            effective_binding_mode(GlobalMode::Enabled, "shadow"),
            "shadow"
        );
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
            })
            .is_err()
        );
        assert!(
            require_initial_uid_confirmation(&self::client::VerificationResult {
                reading_enabled: true,
                dangerous_permissions_disabled: true,
                uid_confirmed: true,
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
