mod admin_finance;
mod chains;
mod channel_settings;
mod crypto;
mod dujiaopay;
mod owner_actions;
mod quotes;
mod sweep;
mod topup_bounds;

pub(crate) use self::chains::token_rail;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::ServerState;
use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use crate::error::AppError;
use crate::market_wallet::{credit_verified_topup_tx, now, wallet_snapshot_tx};
use crate::models::AuthSession;

use self::crypto::{PaymentCipher, aad};
use self::dujiaopay::{
    Client as DujiaoPayClient, CreatedOrder, Credentials, GatewayOrder, WebhookEvent,
};
use self::topup_bounds::TopupAmountLimits;

const MAX_TOPUP_MINOR: i64 = 100_000;
const MIN_TOPUP_MINOR: i64 = 100;
const MAX_PENDING_ORDERS: i64 = 3;
const MAX_ABANDONED_ORDERS: i64 = 3;
const MAX_REVIEW_ORDERS: i64 = 10;
const TOPUP_AUTO_CLOSE_MINUTES: i64 = 30;
const MAX_TOPUPS_PER_HOUR: i64 = 12;
const MAX_TOPUPS_PER_IP_HOUR: u32 = 60;
const MAX_PAYMENT_SECRET_BYTES: usize = 4_096;
const WEBHOOK_HINT_COOLDOWN_SECS: i64 = 300;
const WEBHOOK_HINT_RECOVERY_SECS: i64 = 21_600;
const RECONCILE_LEASE_SECS: i64 = 300;
const TERMINAL_RECONCILE_DAYS: i64 = 90;
const WEBHOOK_KEY_GRACE_MINUTES: i64 = 10;
// Code integration is separate from the Owner's live merchant acceptance.
// The latter is recorded only by the guarded cutover API, never by a fixture.
const MARKET_WALLET_CUTOVER_IMPLEMENTED: bool = true;

fn topup_auto_close_deadline(created_at: chrono::DateTime<Utc>) -> String {
    (created_at + chrono::Duration::minutes(TOPUP_AUTO_CLOSE_MINUTES)).to_rfc3339()
}

fn user_topup_allowed(mode: &str, is_shadow_owner: bool, cutover_approved: bool) -> bool {
    (mode == "enabled" && MARKET_WALLET_CUTOVER_IMPLEMENTED && cutover_approved)
        || (mode == "shadow" && is_shadow_owner)
}

#[derive(Clone)]
pub struct MarketPaymentsRuntime {
    cipher: PaymentCipher,
    client: DujiaoPayClient,
    webhook_limiter: Arc<WebhookLimiter>,
    topup_limiter: Arc<WebhookLimiter>,
    reconcile_wakeup: Arc<tokio::sync::Notify>,
    instruction_lookups: Arc<Mutex<HashMap<String, Instant>>>,
}

#[derive(Default)]
struct WebhookLimiter {
    buckets: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl WebhookLimiter {
    fn allow(&self, ip: IpAddr, window: StdDuration, limit: u32) -> bool {
        let Ok(mut buckets) = self.buckets.lock() else {
            return false;
        };
        let now = Instant::now();
        if buckets.len() > 8_192 {
            buckets.retain(|_, (started, _)| now.duration_since(*started) < window);
            if buckets.len() > 8_192 && !buckets.contains_key(&ip) {
                return false;
            }
        }
        let entry = buckets.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        if entry.1 >= limit {
            return false;
        }
        entry.1 += 1;
        true
    }
}

impl MarketPaymentsRuntime {
    pub fn from_data_dir(data_dir: &std::path::Path, secrets_exist: bool) -> anyhow::Result<Self> {
        Ok(Self {
            cipher: PaymentCipher::load(data_dir, secrets_exist)?,
            client: DujiaoPayClient::production()?,
            webhook_limiter: Arc::new(WebhookLimiter::default()),
            topup_limiter: Arc::new(WebhookLimiter::default()),
            reconcile_wakeup: Arc::new(tokio::sync::Notify::new()),
            instruction_lookups: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    #[cfg(test)]
    pub fn for_test() -> Self {
        Self {
            cipher: PaymentCipher::for_test(),
            client: DujiaoPayClient::production().unwrap(),
            webhook_limiter: Arc::new(WebhookLimiter::default()),
            topup_limiter: Arc::new(WebhookLimiter::default()),
            reconcile_wakeup: Arc::new(tokio::sync::Notify::new()),
            instruction_lookups: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The gateway delivers from a small set of egress addresses, often in
    /// bursts after an outage. Signed events are cheap to verify, so the
    /// bucket is generous; it exists only to cap abusive unsigned traffic.
    pub fn allow_webhook_ip(&self, ip: IpAddr) -> bool {
        self.webhook_limiter
            .allow(ip, StdDuration::from_secs(60), 600)
    }

    /// At most one authenticated instruction lookup per order every five
    /// seconds, the polling interval DujiaoPay asks merchants to respect.
    fn allow_instruction_lookup(&self, order_id: &str) -> bool {
        let Ok(mut lookups) = self.instruction_lookups.lock() else {
            return false;
        };
        let now = Instant::now();
        if lookups.len() > 4_096 {
            lookups.retain(|_, at| now.duration_since(*at) < StdDuration::from_secs(5));
        }
        match lookups.get(order_id) {
            Some(at) if now.duration_since(*at) < StdDuration::from_secs(5) => false,
            _ => {
                lookups.insert(order_id.to_string(), now);
                true
            }
        }
    }

    /// Ask the reconciliation loop to run now instead of at its next tick.
    pub fn wake_reconciler(&self) {
        self.reconcile_wakeup.notify_one();
    }

    fn allow_topup_ip(&self, ip: IpAddr) -> bool {
        self.topup_limiter
            .allow(ip, StdDuration::from_secs(3600), MAX_TOPUPS_PER_IP_HOUR)
    }
}

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}

fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
}

fn topup_return_urls(base: &str, order_id: &str) -> (String, String) {
    let wallet_url = format!("{base}/account/wallet/?topup={order_id}");
    (wallet_url.clone(), format!("{wallet_url}&cancelled=1"))
}

fn payment_webhook_url(domain: &str, use_localhost: bool) -> String {
    let scheme = if use_localhost { "http" } else { "https" };
    format!(
        "{scheme}://{}/v1/payment-webhooks/dujiaopay",
        domain.trim_end_matches('/')
    )
}

/// A pause fences only new obligations. Call this after checking for an
/// idempotent replay, so an already committed order can finish activation.
pub(crate) fn require_new_wallet_orders_open_tx(conn: &Connection) -> Result<(), AppError> {
    let paused: bool = conn
        .query_row(
            "SELECT new_orders_paused FROM market_finance_control WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(db_error("read wallet order pause"))?;
    if paused {
        return Err(AppError::ServiceUnavailable(
            "new platform wallet orders are paused by the Router owner".into(),
        ));
    }
    Ok(())
}

/// New purchases use the platform wallet, regardless of official top-up mode.
/// Old paid terms need republication; never reinterpret an immutable quote.
pub(crate) fn require_current_offer_rail(
    funding_rail: &str,
    product: &str,
) -> Result<(), AppError> {
    if !matches!(funding_rail, "legacy_provider" | "platform_wallet") {
        return Err(AppError::Internal(format!(
            "stored {product} offer funding rail is invalid"
        )));
    }
    if funding_rail != "platform_wallet" {
        return Err(AppError::Gone(format!(
            "{product} offer was published under another payment mode; ask the Provider to republish it"
        )));
    }
    Ok(())
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-wallet", get(get_wallet))
        .route(
            "/v1/market-wallet/transactions",
            get(list_wallet_transactions),
        )
        .route("/v1/market-wallet/channels", get(list_enabled_channels))
        .route("/v1/market-wallet/topup-quotes", post(quotes::create_quote))
        .route(
            "/v1/market-wallet/topups",
            get(list_topups).post(create_topup),
        )
        .route("/v1/market-wallet/topups/:id", get(get_topup))
        .route(
            "/v1/market-wallet/topups/:id/recheck",
            post(recheck_buyer_topup),
        )
        .route(
            "/v1/market-wallet/topups/:id/gateway-status",
            get(get_topup_gateway_status),
        )
        .route(
            "/v1/market-wallet/topups/:id/payment-instructions",
            get(get_payment_instructions),
        )
        .route(
            "/v1/market-wallet/topups/:id/cancel",
            post(cancel_buyer_topup),
        )
        .route(
            "/v1/market-wallet/topups/:id/abandon",
            post(abandon_buyer_topup),
        )
        .route("/v1/market-wallet/topups/:id/stop", post(stop_buyer_topup))
        .route(
            "/v1/admin/market-payment-channels",
            get(list_admin_channels).post(create_channel),
        )
        .route(
            "/v1/admin/market-payment-channels/:id",
            put(replace_channel),
        )
        .route(
            "/v1/admin/market-payment-channels/:id/details",
            put(update_channel_details),
        )
        .route(
            "/v1/admin/market-payment-channels/:id/verify",
            post(verify_channel_api),
        )
        .route(
            "/v1/admin/market-payment-channels/:id/api-credentials",
            put(rotate_channel_api),
        )
        .route(
            "/v1/admin/market-payment-channels/:id/webhook-secret",
            put(rotate_channel_webhook),
        )
        .route(
            "/v1/admin/market-payment-channels/:id/archive",
            put(set_channel_archived),
        )
        .route(
            "/v1/admin/market-payment-channels/:id/enabled",
            put(set_channel_enabled),
        )
        .route(
            "/v1/admin/market-payment-channels/:id/settings",
            put(channel_settings::update_channel_settings),
        )
        .route(
            "/v1/admin/market-payment-channels/rewrap",
            post(rewrap_channel_secrets),
        )
        .route(
            "/v1/admin/market-finance/readiness",
            get(get_finance_readiness),
        )
        .route("/v1/admin/market-finance/cases", get(list_finance_cases))
        .route(
            "/v1/admin/market-finance/cases/:id/resolve",
            post(resolve_finance_case),
        )
        .route(
            "/v1/admin/market-finance/earning-holds",
            get(list_earning_holds).post(place_manual_earning_hold),
        )
        .route(
            "/v1/admin/market-finance/earning-holds/release",
            post(release_manual_earning_hold),
        )
        .route(
            "/v1/admin/market-finance/topups/:id/manual-credit",
            post(owner_actions::credit_manual_claim),
        )
        .route(
            "/v1/admin/market-finance/topups/:id/reverse",
            post(owner_actions::reverse_external_refund),
        )
        .route(
            "/v1/admin/market-finance/overview",
            get(admin_finance::finance_overview),
        )
        .route(
            "/v1/admin/market-finance/users",
            get(admin_finance::user_finance_lookup),
        )
        .route("/v1/admin/market-finance/topups", get(list_admin_topups))
        .route(
            "/v1/admin/market-finance/topups/:id/observations",
            get(list_admin_topup_observations),
        )
        .route(
            "/v1/admin/market-finance/topups/:id/recover",
            post(recover_admin_topup),
        )
        .route(
            "/v1/admin/market-finance/topups/:id/cancel",
            post(cancel_admin_topup),
        )
        .route("/v1/admin/market-finance/mode", put(set_finance_mode))
        .route("/v1/admin/market-finance/pause", put(set_finance_pause))
        .route(
            "/v1/payment-webhooks/dujiaopay",
            post(dujiaopay_webhook_hint).layer(DefaultBodyLimit::max(64 * 1024)),
        )
        .route(
            "/v1/payment-webhooks/dujiaopay/:channel_id",
            post(dujiaopay_webhook).layer(DefaultBodyLimit::max(64 * 1024)),
        )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadinessBlocker {
    code: &'static str,
    count: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FinanceReadinessView {
    mode: String,
    version: i64,
    new_orders_paused: bool,
    new_topups_paused: bool,
    code_integrated: bool,
    critical_cases: i64,
    stale_topup_reviews: i64,
    verified_live_topups: i64,
    payment_channel_blockers: Vec<ReadinessBlocker>,
    can_enable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    webhook_url: Option<String>,
}

fn stale_topup_review_count_tx(conn: &Connection) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COUNT(DISTINCT topup.id) FROM market_topup_orders topup
         JOIN market_finance_reconciliation_cases review
           ON review.object_kind = 'topup' AND review.object_id = topup.id
         WHERE topup.status = 'review_required' AND review.status = 'open'
           AND review.created_at <= ?1",
        params![(Utc::now() - chrono::Duration::hours(24)).to_rfc3339()],
        |row| row.get(0),
    )
    .map_err(db_error("count stale top-up reviews"))
}

fn finance_readiness_tx(conn: &Connection) -> Result<FinanceReadinessView, AppError> {
    let (mode, version, new_orders_paused, new_topups_paused): (String, i64, bool, bool) = conn
.query_row("SELECT mode, version, new_orders_paused, new_topups_paused FROM market_finance_control WHERE id=1", [],
|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(db_error("read market finance mode"))?;
    let verified_live_topups: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market_verified_topup_evidence",
            [],
            |row| row.get(0),
        )
        .map_err(db_error("count verified top-ups"))?;
    let critical_cases = crate::market_reconcile::critical_case_count_tx(conn)?;
    let stale_topup_reviews = stale_topup_review_count_tx(conn)?;
    let channel_checks: [(&'static str, &'static str); 6] = [
        (
            "credited_topup_missing_evidence",
            "SELECT COUNT(*) FROM market_topup_orders topup
             WHERE topup.status = 'credited'
               AND NOT EXISTS (SELECT 1 FROM market_completed_topup_evidence evidence
                               WHERE evidence.id = topup.id)",
        ),
        (
            "channel_none_enabled",
            "SELECT CASE WHEN EXISTS (SELECT 1 FROM market_payment_channels c JOIN market_payment_channel_revisions r ON r.channel_id = c.id AND r.revision = c.current_revision WHERE c.enabled = 1 AND c.archived_at IS NULL AND c.api_reverify_required = 0 AND r.verified_at IS NOT NULL) THEN 0 ELSE 1 END",
        ),
        (
            "channel_api_unverified",
            "SELECT COUNT(*) FROM market_payment_channels c LEFT JOIN market_payment_channel_revisions r ON r.channel_id = c.id AND r.revision = c.current_revision WHERE c.enabled = 1 AND c.archived_at IS NULL AND (c.api_reverify_required = 1 OR r.verified_at IS NULL)",
        ),
        (
            "channel_not_signed",
            "SELECT COUNT(*) FROM market_payment_channels c JOIN market_payment_channel_revisions r ON r.channel_id = c.id AND r.revision = c.current_revision WHERE c.enabled = 1 AND c.archived_at IS NULL AND r.webhook_mode != 'signed'",
        ),
        (
            "channel_webhook_unverified",
            "SELECT COUNT(*) FROM market_payment_channels c JOIN market_payment_channel_revisions r ON r.channel_id = c.id AND r.revision = c.current_revision WHERE c.enabled = 1 AND c.archived_at IS NULL AND r.webhook_mode = 'signed' AND (c.webhook_verified_at IS NULL OR c.webhook_verified_revision IS NOT c.current_revision)",
        ),
        (
            "channel_none_live",
            "SELECT CASE WHEN EXISTS (SELECT 1 FROM market_payment_channels c WHERE c.enabled = 1 AND c.rollout_state = 'live' AND c.archived_at IS NULL AND EXISTS (SELECT 1 FROM market_verified_topup_evidence t WHERE t.channel_id = c.id AND t.channel_revision = c.current_revision)) THEN 0 ELSE 1 END",
        ),
    ];
    let mut payment_channel_blockers = Vec::new();
    for (code, sql) in channel_checks {
        let count: i64 = conn
            .query_row(sql, [], |row| row.get(0))
            .map_err(db_error("read payment channel readiness"))?;
        if count > 0 {
            payment_channel_blockers.push(ReadinessBlocker { code, count });
        }
    }
    let can_enable = MARKET_WALLET_CUTOVER_IMPLEMENTED
        && critical_cases == 0
        && verified_live_topups > 0
        && payment_channel_blockers.is_empty();
    Ok(FinanceReadinessView {
        mode,
        version,
        new_orders_paused,
        new_topups_paused,
        code_integrated: MARKET_WALLET_CUTOVER_IMPLEMENTED,
        critical_cases,
        stale_topup_reviews,
        verified_live_topups,
        payment_channel_blockers,
        can_enable,
        webhook_url: None,
    })
}

async fn get_finance_readiness(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut readiness = finance_readiness_tx(&conn)?;
    readiness.webhook_url = Some(payment_webhook_url(
        &state.config.tunnel_domain,
        state.config.use_localhost,
    ));
    Ok((no_store(), Json(readiness)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModeRequest {
    mode: String,
    expected_version: i64,
}

fn set_finance_mode_tx(
    conn: &Connection,
    request: &ModeRequest,
    changed_at: &str,
) -> Result<(), AppError> {
    if !matches!(request.mode.as_str(), "disabled" | "shadow" | "enabled") {
        return Err(AppError::BadRequest("invalid market finance mode".into()));
    }
    let current_mode: String = conn
        .query_row(
            "SELECT mode FROM market_finance_control WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(db_error("read market finance mode"))?;
    if request.mode == "enabled" {
        let readiness = finance_readiness_tx(conn)?;
        // Every reopening checks today's channel revisions and ledger safety.
        // A previous approval must not authorize a rotated/unverified channel.
        if current_mode == "disabled" || !readiness.can_enable {
            return Err(AppError::Conflict(
                "official top-up readiness gates have not passed; test in shadow first".into(),
            ));
        }
    }
    let changed = conn
        .execute(
            "UPDATE market_finance_control SET mode = ?2, version = version + 1,
                updated_at = ?3,
                cutover_approved_at = CASE WHEN ?2 = 'enabled'
                    THEN COALESCE(cutover_approved_at, ?3) ELSE NULL END
             WHERE id = 1 AND version = ?1",
            params![request.expected_version, request.mode, changed_at],
        )
        .map_err(db_error("update finance mode"))?;
    if changed != 1 {
        return Err(AppError::Conflict("finance mode version changed".into()));
    }
    Ok(())
}

async fn set_finance_mode(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<ModeRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin finance mode update"))?;
    let changed_at = now();
    set_finance_mode_tx(&tx, &request, &changed_at)?;
    audit_tx(
        &tx,
        &session.user_id,
        "finance_mode_changed",
        "finance_control",
        "1",
        Some(&request.mode),
        &now(),
    )?;
    tx.commit().map_err(db_error("commit finance mode"))?;
    Ok((
        no_store(),
        Json(serde_json::json!({ "mode": request.mode })),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FinancePauseRequest {
    new_orders_paused: bool,
    #[serde(default)]
    new_topups_paused: Option<bool>,
    expected_version: i64,
}

/// Purchase and official top-up pauses are independent in every top-up mode.
fn set_finance_pause_tx(
    conn: &Connection,
    request: &FinancePauseRequest,
    changed_at: &str,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE market_finance_control
             SET new_orders_paused = ?2,
                 new_topups_paused = COALESCE(?4, new_topups_paused),
                 version = version + 1, updated_at = ?3
             WHERE id = 1 AND version = ?1",
            params![
                request.expected_version,
                request.new_orders_paused,
                changed_at,
                request.new_topups_paused
            ],
        )
        .map_err(db_error("update finance pause"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "finance pause requires the current version".into(),
        ));
    }
    Ok(())
}

/// `orders_paused`, `orders_and_topups_paused`, ... or `resumed`.
fn finance_pause_reason((orders, topups): (bool, bool)) -> String {
    let paused = [(orders, "orders"), (topups, "topups")]
        .into_iter()
        .filter_map(|(paused, name)| paused.then_some(name))
        .collect::<Vec<_>>();
    if paused.is_empty() {
        "resumed".into()
    } else {
        format!("{}_paused", paused.join("_and_"))
    }
}

async fn set_finance_pause(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<FinancePauseRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin finance pause update"))?;
    let changed_at = now();
    set_finance_pause_tx(&tx, &request, &changed_at)?;
    // Describe the resulting state, not one flag of the request: the top-up
    // pause is optional in the request and may be unchanged.
    let flags: (bool, bool) = tx
        .query_row(
            "SELECT new_orders_paused, new_topups_paused
             FROM market_finance_control WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read finance pause"))?;
    audit_tx(
        &tx,
        &session.user_id,
        "finance_pause_changed",
        "finance_control",
        "1",
        Some(&finance_pause_reason(flags)),
        &changed_at,
    )?;
    tx.commit()
        .map_err(db_error("commit finance pause update"))?;
    Ok((
        no_store(),
        Json(serde_json::json!({
            "newOrdersPaused": request.new_orders_paused,
            "newTopupsPaused": request.new_topups_paused,
        })),
    ))
}

async fn get_wallet(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let owner = crate::api::is_finance_owner(&state.config, &session.email);
    let (snapshot, mode, can_topup, has_wallet_activity) = {
        let conn = state.store.conn.lock().await;
        let snapshot = wallet_snapshot_tx(&conn, &session.user_id)?;
        let (mode, cutover_approved, paused): (String, bool, bool) = conn
            .query_row(
                "SELECT mode, cutover_approved_at IS NOT NULL, new_topups_paused
                 FROM market_finance_control WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(db_error("read wallet mode"))?;
        let has_wallet_activity: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_wallet_accounts WHERE user_id = ?1)
                     OR EXISTS(SELECT 1 FROM market_topup_orders WHERE user_id = ?1)",
                params![session.user_id],
                |row| row.get(0),
            )
            .map_err(db_error("read wallet activity"))?;
        let can_topup = user_topup_allowed(&mode, owner, cutover_approved)
            && !paused
            && !buyer_channel_views(&conn, owner)?.is_empty();
        (snapshot, mode, can_topup, has_wallet_activity)
    };
    let committed_units = {
        let conn = state.store.conn.lock().await;
        crate::market_platform_contracts::prior_due_funding_units_tx(
            &conn,
            &session.user_id,
            None,
            &now(),
        )?
    };
    Ok((
        no_store(),
        Json(serde_json::json!({
            "assetCode": snapshot.asset_code,
            "postedUnits": snapshot.posted_units.to_string(),
            "heldUnits": snapshot.held_units.to_string(),
            "availableUnits": snapshot.available_units.to_string(),
            "committedUnits": committed_units.to_string(),
            "version": snapshot.version,
            "mode": mode,
            "canTopup": can_topup,
            "hasWalletActivity": has_wallet_activity,
        })),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageQuery {
    cursor: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WalletTransactionView {
    id: String,
    source_kind: String,
    action: String,
    #[serde(serialize_with = "crate::market_wallet::serialize_units")]
    delta_units: i64,
    #[serde(serialize_with = "crate::market_wallet::serialize_units")]
    held_delta_units: i64,
    created_at: String,
    transfer_id: Option<String>,
    counterparty_email: Option<String>,
    c2c_order_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}

fn wallet_transactions_tx(
    conn: &Connection,
    user_id: &str,
    cursor: Option<&str>,
) -> Result<Page<WalletTransactionView>, AppError> {
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT journal.created_at FROM market_journal_transactions journal
                 WHERE journal.id = ?1 AND EXISTS (
                   SELECT 1 FROM market_journal_postings posting
                   WHERE posting.journal_id = journal.id AND posting.subject_id = ?2
                     AND posting.account_kind IN ('wallet_available', 'wallet_held'))",
                params![id, user_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read wallet transaction cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid wallet transaction cursor".into()))
        })
        .transpose()?;
    let mut statement = conn
        .prepare(
            "SELECT journal.id, journal.source_kind, journal.action,
                    -COALESCE((SELECT SUM(posting.signed_units)
                      FROM market_journal_postings posting
                      WHERE posting.journal_id = journal.id AND posting.subject_id = ?1
                        AND posting.account_kind IN ('wallet_available', 'wallet_held')), 0),
                    -COALESCE((SELECT SUM(posting.signed_units)
                      FROM market_journal_postings posting
                      WHERE posting.journal_id = journal.id AND posting.subject_id = ?1
                        AND posting.account_kind = 'wallet_held'), 0),
                    journal.created_at,
                    (SELECT id FROM market_wallet_transfers WHERE journal_id = journal.id),
                    (SELECT CASE WHEN sender_user_id = ?1 THEN recipient_email ELSE sender_email END
                     FROM market_wallet_transfers WHERE journal_id = journal.id),
                    CASE WHEN journal.source_kind='c2c_order' THEN
                      (SELECT id FROM market_c2c_orders WHERE id=journal.source_id)
                     WHEN journal.source_kind='hold' THEN
                      (SELECT id FROM market_c2c_orders WHERE hold_id=journal.source_id)
                     ELSE NULL END
             FROM market_journal_transactions journal
             WHERE EXISTS (SELECT 1 FROM market_journal_postings posting
               WHERE posting.journal_id = journal.id AND posting.subject_id = ?1
                 AND posting.account_kind IN ('wallet_available', 'wallet_held'))
               AND (?2 IS NULL OR journal.created_at < ?2
                 OR (journal.created_at = ?2 AND journal.id < ?3))
             ORDER BY journal.created_at DESC, journal.id DESC LIMIT 51",
        )
        .map_err(db_error("prepare wallet transactions"))?;
    let mut items = statement
        .query_map(params![user_id, cursor_at, cursor], |row| {
            Ok(WalletTransactionView {
                id: row.get(0)?,
                source_kind: row.get(1)?,
                action: row.get(2)?,
                delta_units: row.get(3)?,
                held_delta_units: row.get(4)?,
                created_at: row.get(5)?,
                transfer_id: row.get(6)?,
                counterparty_email: row.get(7)?,
                c2c_order_id: row.get(8)?,
            })
        })
        .map_err(db_error("query wallet transactions"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read wallet transactions"))?;
    let has_more = items.len() > 50;
    items.truncate(50);
    let next_cursor = has_more.then(|| items.last().unwrap().id.clone());
    Ok(Page { items, next_cursor })
}

#[cfg(test)]
pub(crate) fn wallet_transactions_for_test(
    conn: &Connection,
    user: &str,
) -> Result<serde_json::Value, AppError> {
    serde_json::to_value(wallet_transactions_tx(conn, user, None)?)
        .map_err(|_| AppError::Internal("serialize test wallet history".into()))
}

async fn list_wallet_transactions(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(wallet_transactions_tx(
            &conn,
            &session.user_id,
            query.cursor.as_deref(),
        )?),
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelView {
    id: String,
    display_name: String,
    enabled: bool,
    rollout_state: String,
    promoted_at: Option<String>,
    sort_order: i64,
    current_revision: i64,
    version: i64,
    archived_at: Option<String>,
    verified: bool,
    webhook_mode: String,
    webhook_verified_at: Option<String>,
    webhook_verified: bool,
    unresolved_topups: i64,
    verified_live_topups: i64,
    verified_live_topups_current_revision: i64,
    gateway_kind: &'static str,
    settings: ChannelSettingsView,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ChannelSettingsView {
    order_mode: String,
    allowed_methods: Vec<String>,
    min_topup_minor: Option<i64>,
    max_topup_minor: Option<i64>,
    fee_rate_bps: i64,
    fixed_fee_minor: i64,
    version: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuyerMethodView {
    token_id: String,
    chain: &'static str,
    chain_name: &'static str,
    symbol: &'static str,
    min_confirmations: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuyerChannelView {
    id: String,
    display_name: String,
    gateway_kind: &'static str,
    order_mode: String,
    methods: Vec<BuyerMethodView>,
    #[serde(flatten)]
    amount_limits: TopupAmountLimits,
    channel_revision: i64,
    channel_settings_version: i64,
    canary: bool,
}

/// Wallet-credit bounds: the global principal range intersected with the
/// Owner-configured channel range. Maintenance fees are added separately.
fn effective_topup_bounds(settings: &ChannelSettingsView) -> (i64, i64) {
    let min = settings
        .min_topup_minor
        .unwrap_or(MIN_TOPUP_MINOR)
        .max(MIN_TOPUP_MINOR);
    let max = settings
        .max_topup_minor
        .unwrap_or(MAX_TOPUP_MINOR)
        .min(MAX_TOPUP_MINOR);
    (min, max)
}

impl BuyerChannelView {
    fn new(
        channel: ChannelView,
        policy: &crate::market_wallet_policy::WalletPolicy,
    ) -> Result<Self, AppError> {
        let (min_topup_minor, max_topup_minor) = effective_topup_bounds(&channel.settings);
        Ok(Self {
            id: channel.id,
            display_name: channel.display_name,
            gateway_kind: channel.gateway_kind,
            order_mode: channel.settings.order_mode.clone(),
            methods: channel
                .settings
                .allowed_methods
                .iter()
                .filter_map(|token| token_rail(token))
                .map(|rail| BuyerMethodView {
                    token_id: rail.token_id.into(),
                    chain: rail.chain,
                    chain_name: rail.chain_name,
                    symbol: rail.symbol,
                    min_confirmations: rail.min_confirmations,
                })
                .collect(),
            amount_limits: TopupAmountLimits::new(
                min_topup_minor,
                max_topup_minor,
                policy.maintenance_fee_bps,
                policy.version,
            )?,
            channel_revision: channel.current_revision,
            channel_settings_version: channel.settings.version,
            canary: channel.rollout_state == "canary",
        })
    }
}

const CHANNEL_VIEW_COLUMNS: &str = "c.id, c.display_name, c.enabled, c.sort_order, c.current_revision,
        c.version, c.archived_at, (r.verified_at IS NOT NULL AND c.api_reverify_required = 0), r.webhook_mode,
        c.webhook_verified_at,
        (c.webhook_verified_at IS NOT NULL AND c.webhook_verified_revision IS c.current_revision),
        (SELECT COUNT(*) FROM market_topup_orders t
         WHERE t.channel_id = c.id AND t.status NOT IN ('credited', 'expired', 'cancelled')),
        (SELECT COUNT(*) FROM market_verified_topup_evidence t
         WHERE t.channel_id = c.id),
        (SELECT COUNT(*) FROM market_verified_topup_evidence t
         WHERE t.channel_id = c.id AND t.channel_revision = c.current_revision),
        c.rollout_state, c.promoted_at,
        COALESCE(s.order_mode, 'cashier'), COALESCE(s.allowed_methods_json, '[]'),
        s.min_topup_minor, s.max_topup_minor, COALESCE(s.fee_rate_bps, 0),
        COALESCE(s.fixed_fee_minor, 0), COALESCE(s.version, 0)";

fn channel_view_row(row: &crate::db::Row<'_>) -> crate::db::Result<ChannelView> {
    let methods_json: String = row.get(17)?;
    Ok(ChannelView {
        id: row.get(0)?,
        display_name: row.get(1)?,
        enabled: row.get::<_, i64>(2)? == 1,
        sort_order: row.get(3)?,
        current_revision: row.get(4)?,
        version: row.get(5)?,
        archived_at: row.get(6)?,
        verified: row.get::<_, i64>(7)? == 1,
        webhook_mode: row.get(8)?,
        webhook_verified_at: row.get(9)?,
        webhook_verified: row.get::<_, i64>(10)? == 1,
        unresolved_topups: row.get(11)?,
        verified_live_topups: row.get(12)?,
        verified_live_topups_current_revision: row.get(13)?,
        rollout_state: row.get(14)?,
        promoted_at: row.get(15)?,
        gateway_kind: "dujiaopay",
        settings: ChannelSettingsView {
            order_mode: row.get(16)?,
            allowed_methods: serde_json::from_str(&methods_json).unwrap_or_default(),
            min_topup_minor: row.get(18)?,
            max_topup_minor: row.get(19)?,
            fee_rate_bps: row.get(20)?,
            fixed_fee_minor: row.get(21)?,
            version: row.get(22)?,
        },
    })
}

/// `only_enabled` lists channels that accept new orders from every eligible
/// buyer (live). The Owner additionally sees canary channels through
/// [`buyer_channel_views`].
fn channel_views(conn: &Connection, only_enabled: bool) -> Result<Vec<ChannelView>, AppError> {
    let sql = if only_enabled {
        format!(
            "SELECT {CHANNEL_VIEW_COLUMNS}
             FROM market_payment_channels c
             JOIN market_payment_channel_revisions r
               ON r.channel_id = c.id AND r.revision = c.current_revision
             LEFT JOIN market_payment_channel_settings s ON s.channel_id = c.id
             WHERE c.rollout_state = 'live' AND c.archived_at IS NULL
               AND c.api_reverify_required = 0 AND r.verified_at IS NOT NULL
             ORDER BY c.sort_order, c.id"
        )
    } else {
        format!(
            "SELECT {CHANNEL_VIEW_COLUMNS}
             FROM market_payment_channels c
             LEFT JOIN market_payment_channel_revisions r
               ON r.channel_id = c.id AND r.revision = c.current_revision
             LEFT JOIN market_payment_channel_settings s ON s.channel_id = c.id
             ORDER BY c.archived_at IS NOT NULL, c.sort_order, c.id"
        )
    };
    let mut statement = conn
        .prepare(&sql)
        .map_err(db_error("prepare payment channels"))?;
    statement
        .query_map([], channel_view_row)
        .map_err(db_error("query payment channels"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read payment channels"))
}

fn buyer_channel_views(
    conn: &Connection,
    include_canary: bool,
) -> Result<Vec<BuyerChannelView>, AppError> {
    let policy = crate::market_wallet_policy::policy_tx(conn)?;
    channel_views(conn, false)?
        .into_iter()
        .filter(|channel| {
            channel.archived_at.is_none()
                && channel.verified
                && (channel.rollout_state == "live"
                    || (include_canary && channel.rollout_state == "canary"))
        })
        .map(|channel| BuyerChannelView::new(channel, &policy))
        .collect()
}

async fn list_enabled_channels(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let (mode, cutover_approved): (String, bool) = {
        let conn = state.store.conn.lock().await;
        conn.query_row(
            "SELECT mode, cutover_approved_at IS NOT NULL
             FROM market_finance_control WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read market finance mode"))?
    };
    let owner = crate::api::is_finance_owner(&state.config, &session.email);
    let conn = state.store.conn.lock().await;
    let channels = if user_topup_allowed(&mode, owner, cutover_approved) {
        buyer_channel_views(&conn, owner)?
    } else {
        Vec::new()
    };
    Ok((no_store(), Json(channels)))
}

async fn list_admin_channels(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(channel_views(&conn, false)?)))
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpsertChannelRequest {
    display_name: String,
    sort_order: i64,
    api_key_id: String,
    api_secret: String,
    #[serde(default)]
    webhook_secret: Option<String>,
    #[serde(default)]
    webhook_mode: Option<String>,
    #[serde(default)]
    allowed_methods: Vec<String>,
    #[serde(default)]
    expected_version: Option<i64>,
}

fn validate_channel_input(input: &UpsertChannelRequest) -> Result<(), AppError> {
    let inferred_mode = if input.webhook_secret.is_some() {
        "signed"
    } else {
        "poll_only"
    };
    if input.display_name.trim().is_empty()
        || input.display_name.len() > 80
        || input.api_key_id.trim().is_empty()
        || input.api_key_id.len() > 200
        || input.api_secret.len() < 16
        || input.api_secret.len() > MAX_PAYMENT_SECRET_BYTES
        || input
            .webhook_secret
            .as_ref()
            .is_some_and(|secret| !(16..=MAX_PAYMENT_SECRET_BYTES).contains(&secret.len()))
        || input
            .webhook_mode
            .as_deref()
            .is_some_and(|mode| mode != inferred_mode)
        || (input.webhook_secret.is_none() && input.webhook_mode.as_deref() != Some("poll_only"))
        || input.allowed_methods.len() > 32
        || input.allowed_methods.iter().any(|method| {
            method.len() > 80
                || !method.contains('-')
                || !method
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err(AppError::BadRequest(
            "invalid DujiaoPay channel configuration".into(),
        ));
    }
    Ok(())
}

fn validate_webhook_mode_change(
    current_mode: &str,
    next_mode: &str,
    explicit_next_mode: bool,
) -> Result<(), AppError> {
    let _ = explicit_next_mode;
    match (current_mode, next_mode) {
        // A downgrade drops callback authentication and must go through the
        // dedicated Webhook-secret rotation, which audits it explicitly.
        ("signed", "poll_only") => Err(AppError::Conflict(
            "use the Webhook secret rotation to stop signed callbacks".into(),
        )),
        ("signed" | "poll_only", "signed" | "poll_only") => Ok(()),
        _ => Err(AppError::Internal(
            "invalid stored payment webhook mode".into(),
        )),
    }
}

async fn save_channel(
    state: &ServerState,
    session: &AuthSession,
    channel_id: Option<String>,
    input: UpsertChannelRequest,
) -> Result<ChannelView, AppError> {
    validate_channel_input(&input)?;
    if channel_id.is_some() != input.expected_version.is_some() {
        return Err(AppError::BadRequest(
            "expectedVersion is required only when replacing a channel".into(),
        ));
    }
    let webhook_mode = if input.webhook_secret.is_some() {
        "signed"
    } else {
        "poll_only"
    };
    let credentials = Credentials {
        key_id: input.api_key_id.trim().into(),
        api_secret: Zeroizing::new(input.api_secret.clone()),
        webhook_secret: input.webhook_secret.clone().map(Zeroizing::new),
    };
    // HMAC self-check is outside the database lock. A successful response
    // binds all future orders to the merchant identity observed here.
    let merchant_id = state.market_payments.client.whoami(&credentials).await?;
    let merchant_fingerprint = hex::encode(Sha256::digest(merchant_id.as_bytes()));
    let replacing = channel_id.is_some();
    let id = channel_id.unwrap_or_else(|| format!("djp_{}", Uuid::new_v4().simple()));
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin payment channel update"))?;
    let current_state = tx
        .query_row(
            "SELECT current_revision, version, archived_at FROM market_payment_channels WHERE id = ?1",
            params![id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<String>>(2)?)),
        )
        .optional()
        .map_err(db_error("read payment channel revision"))?;
    if replacing && current_state.is_none() {
        return Err(AppError::NotFound("payment channel not found".into()));
    }
    let current_revision = current_state.as_ref().map(|state| state.0);
    let mut preserve_webhook_verification = false;
    let mut previous_webhook_revision = None;
    let mut previous_webhook_valid_until = None;
    if let Some((current_revision, version, archived_at)) = &current_state {
        if archived_at.is_some() {
            return Err(AppError::Conflict(
                "archived payment channel must be restored first".into(),
            ));
        }
        if input.expected_version != Some(*version) {
            return Err(AppError::Conflict("payment channel version changed".into()));
        }
        let old = load_revision(&tx, &state.market_payments, &id, *current_revision)?;
        if old.merchant_fingerprint != merchant_fingerprint {
            return Err(AppError::Conflict(
                "payment merchant changed; create a new channel".into(),
            ));
        }
        let same_webhook_key = old
            .credentials
            .webhook_secret
            .as_ref()
            .map(|secret| secret.as_str())
            == input.webhook_secret.as_deref();
        preserve_webhook_verification = same_webhook_key;
        if !same_webhook_key
            && old.credentials.webhook_secret.is_some()
            && credentials.webhook_secret.is_some()
        {
            previous_webhook_revision = Some(*current_revision);
            previous_webhook_valid_until = Some(
                (Utc::now() + chrono::Duration::minutes(WEBHOOK_KEY_GRACE_MINUTES)).to_rfc3339(),
            );
        }
        let current_mode: String = tx
            .query_row(
                "SELECT webhook_mode FROM market_payment_channel_revisions
             WHERE channel_id = ?1 AND revision = ?2",
                params![id, current_revision],
                |row| row.get(0),
            )
            .map_err(db_error("read payment webhook mode"))?;
        validate_webhook_mode_change(&current_mode, webhook_mode, input.webhook_mode.is_some())?;
    }
    let revision = current_revision.unwrap_or(0) + 1;
    let (api_ciphertext, api_nonce) = state.market_payments.cipher.seal(
        &input.api_secret,
        &aad(&id, revision, &merchant_fingerprint, "api"),
    )?;
    let (webhook_ciphertext, webhook_nonce) = if let Some(secret) = input.webhook_secret.as_deref()
    {
        let (ciphertext, nonce) = state.market_payments.cipher.seal(
            secret,
            &aad(&id, revision, &merchant_fingerprint, "webhook"),
        )?;
        (ciphertext, nonce)
    } else {
        (String::new(), String::new())
    };
    let time = now();
    if current_revision.is_none() {
        tx.execute(
            "INSERT INTO market_payment_channels (id, gateway_kind, display_name, enabled, sort_order, current_revision, created_at, updated_at)
             VALUES (?1, 'dujiaopay', ?2, 0, ?3, ?4, ?5, ?5)",
            params![id, input.display_name.trim(), input.sort_order, revision, time],
        ).map_err(db_error("insert payment channel"))?;
    } else {
        let changed = tx.execute(
            "UPDATE market_payment_channels SET display_name = ?2, enabled = 0, sort_order = ?3,
                    current_revision = ?4, updated_at = ?5, version = version + 1,
                    api_reverify_required = 0,
                    previous_webhook_revision = ?7, previous_webhook_valid_until = ?8,
                    webhook_verified_revision = CASE WHEN ?9 THEN ?4 ELSE NULL END,
                    webhook_verified_at = CASE WHEN ?9 THEN webhook_verified_at ELSE NULL END
             WHERE id = ?1 AND version = ?6 AND archived_at IS NULL",
            params![
                id,
                input.display_name.trim(),
                input.sort_order,
                revision,
                time,
                input.expected_version,
                previous_webhook_revision,
                previous_webhook_valid_until,
                preserve_webhook_verification,
            ],
        )
        .map_err(db_error("update payment channel"))?;
        if changed != 1 {
            return Err(AppError::Conflict("payment channel version changed".into()));
        }
    }
    // Channel settings are the single authority for allowed methods. The
    // request value only seeds a new channel; a replaced revision copies the
    // current settings, so the per-revision snapshot that legacy orders read
    // can never disagree with them.
    let allowed_methods_json = if current_revision.is_none() {
        let encoded = serde_json::to_string(&input.allowed_methods)
            .map_err(|_| AppError::Internal("encode allowed methods failed".into()))?;
        tx.execute(
            "UPDATE market_payment_channel_settings SET allowed_methods_json = ?2, updated_at = ?3
             WHERE channel_id = ?1",
            params![id, encoded, time],
        )
        .map_err(db_error("initialize payment channel settings"))?;
        encoded
    } else {
        tx.query_row(
            "SELECT allowed_methods_json FROM market_payment_channel_settings WHERE channel_id = ?1",
            params![id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error("read payment channel settings"))?
        .unwrap_or_else(|| "[]".into())
    };
    tx.execute(
        "INSERT INTO market_payment_channel_revisions (
            channel_id, revision, merchant_id, merchant_fingerprint, api_key_id,
            api_secret_ciphertext, api_secret_nonce, webhook_secret_ciphertext,
            webhook_secret_nonce, key_version, allowed_methods_json, verified_at, created_at,
            webhook_mode
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13)",
        params![
            id,
            revision,
            merchant_id,
            merchant_fingerprint,
            credentials.key_id,
            api_ciphertext,
            api_nonce,
            webhook_ciphertext,
            webhook_nonce,
            state.market_payments.cipher.version,
            allowed_methods_json,
            time,
            webhook_mode
        ],
    )
    .map_err(db_error("insert payment channel revision"))?;
    audit_tx(
        &tx,
        &session.user_id,
        "channel_saved",
        "payment_channel",
        &id,
        None,
        &time,
    )?;
    tx.commit().map_err(db_error("commit payment channel"))?;
    channel_views(&conn, false)?
        .into_iter()
        .find(|view| view.id == id)
        .ok_or_else(|| AppError::Internal("saved payment channel missing".into()))
}

async fn create_channel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpsertChannelRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    let view = save_channel(&state, &session, None, input).await?;
    Ok((no_store(), Json(view)))
}

async fn replace_channel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpsertChannelRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    {
        let conn = state.store.conn.lock().await;
        let expected_version = input.expected_version.ok_or_else(|| {
            AppError::BadRequest("expectedVersion is required when replacing a channel".into())
        })?;
        editable_revision_tx(&conn, &id, expected_version)?;
    }
    let view = save_channel(&state, &session, Some(id), input).await?;
    Ok((no_store(), Json(view)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChannelDetailsRequest {
    display_name: String,
    sort_order: i64,
    expected_version: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerifyChannelRequest {
    expected_version: i64,
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RotateApiRequest {
    api_key_id: String,
    api_secret: String,
    expected_version: i64,
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RotateWebhookRequest {
    webhook_mode: String,
    webhook_secret: Option<String>,
    #[serde(default)]
    emergency_revoke_previous: bool,
    expected_version: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveChannelRequest {
    archived: bool,
    expected_version: i64,
}

fn channel_edit_state_tx(
    conn: &Connection,
    id: &str,
) -> Result<(i64, i64, Option<String>), AppError> {
    conn.query_row(
        "SELECT current_revision, version, archived_at FROM market_payment_channels WHERE id = ?1",
        params![id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(db_error("read payment channel edit state"))?
    .ok_or_else(|| AppError::NotFound("payment channel not found".into()))
}

fn editable_revision_tx(conn: &Connection, id: &str, version: i64) -> Result<i64, AppError> {
    let (revision, current_version, archived_at) = channel_edit_state_tx(conn, id)?;
    if current_version != version {
        return Err(AppError::Conflict("payment channel version changed".into()));
    }
    if archived_at.is_some() {
        return Err(AppError::Conflict(
            "archived payment channel must be restored first".into(),
        ));
    }
    Ok(revision)
}

fn channel_view_tx(conn: &Connection, id: &str) -> Result<ChannelView, AppError> {
    channel_views(conn, false)?
        .into_iter()
        .find(|view| view.id == id)
        .ok_or_else(|| AppError::Internal("payment channel view missing".into()))
}

async fn update_channel_details(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ChannelDetailsRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    if input.display_name.trim().is_empty() || input.display_name.len() > 80 {
        return Err(AppError::BadRequest(
            "invalid payment channel display name".into(),
        ));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin payment channel details update"))?;
    editable_revision_tx(&tx, &id, input.expected_version)?;
    let changed = tx.execute(
        "UPDATE market_payment_channels SET display_name = ?2, sort_order = ?3, version = version + 1, updated_at = ?4
         WHERE id = ?1 AND version = ?5 AND archived_at IS NULL",
        params![id, input.display_name.trim(), input.sort_order, now(), input.expected_version],
    ).map_err(db_error("update payment channel details"))?;
    if changed != 1 {
        return Err(AppError::Conflict("payment channel version changed".into()));
    }
    audit_tx(
        &tx,
        &session.user_id,
        "channel_details_updated",
        "payment_channel",
        &id,
        None,
        &now(),
    )?;
    tx.commit()
        .map_err(db_error("commit payment channel details"))?;
    Ok((no_store(), Json(channel_view_tx(&conn, &id)?)))
}

async fn verify_channel_api(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VerifyChannelRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    let current_credentials = {
        let conn = state.store.conn.lock().await;
        let revision = editable_revision_tx(&conn, &id, input.expected_version)?;
        load_revision(&conn, &state.market_payments, &id, revision)?.credentials
    };
    let merchant_id = state
        .market_payments
        .client
        .whoami(&current_credentials)
        .await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin payment merchant reverification"))?;
    let revision = editable_revision_tx(&tx, &id, input.expected_version)?;
    let current = load_revision(&tx, &state.market_payments, &id, revision)?;
    if merchant_id != current.merchant_id {
        return Err(AppError::Conflict(
            "payment merchant identity changed".into(),
        ));
    }
    let time = now();
    mark_channel_api_reverified_tx(&tx, &id, input.expected_version, &time)?;
    audit_tx(
        &tx,
        &session.user_id,
        "channel_api_reverified",
        "payment_channel",
        &id,
        None,
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit payment merchant reverification"))?;
    Ok((no_store(), Json(channel_view_tx(&conn, &id)?)))
}

fn mark_channel_api_reverified_tx(
    conn: &Connection,
    id: &str,
    expected_version: i64,
    time: &str,
) -> Result<(), AppError> {
    editable_revision_tx(conn, id, expected_version)?;
    let changed = conn
        .execute(
            "UPDATE market_payment_channels SET api_reverify_required = 0, version = version + 1,
         updated_at = ?2 WHERE id = ?1 AND version = ?3 AND archived_at IS NULL",
            params![id, time, expected_version],
        )
        .map_err(db_error("reverify payment merchant API"))?;
    if changed != 1 {
        return Err(AppError::Conflict("payment channel version changed".into()));
    }
    Ok(())
}

fn insert_rotated_revision_tx(
    conn: &Connection,
    runtime: &MarketPaymentsRuntime,
    id: &str,
    revision: i64,
    loaded: &LoadedRevision,
    credentials: &Credentials,
    time: &str,
) -> Result<(), AppError> {
    let (api_ciphertext, api_nonce) = runtime.cipher.seal(
        credentials.api_secret.as_str(),
        &aad(id, revision, &loaded.merchant_fingerprint, "api"),
    )?;
    let (webhook_ciphertext, webhook_nonce) =
        if let Some(secret) = credentials.webhook_secret.as_ref() {
            runtime.cipher.seal(
                secret.as_str(),
                &aad(id, revision, &loaded.merchant_fingerprint, "webhook"),
            )?
        } else {
            (String::new(), String::new())
        };
    let methods = serde_json::to_string(&loaded.allowed_methods)
        .map_err(|_| AppError::Internal("encode allowed methods failed".into()))?;
    let webhook_mode = if credentials.webhook_secret.is_some() {
        "signed"
    } else {
        "poll_only"
    };
    conn.execute(
        "INSERT INTO market_payment_channel_revisions (
            channel_id, revision, merchant_id, merchant_fingerprint, api_key_id,
            api_secret_ciphertext, api_secret_nonce, webhook_secret_ciphertext,
            webhook_secret_nonce, key_version, allowed_methods_json, verified_at,
            created_at, webhook_mode
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?13)",
        params![
            id,
            revision,
            loaded.merchant_id,
            loaded.merchant_fingerprint,
            credentials.key_id,
            api_ciphertext,
            api_nonce,
            webhook_ciphertext,
            webhook_nonce,
            runtime.cipher.version,
            methods,
            time,
            webhook_mode
        ],
    )
    .map_err(db_error("insert rotated payment credential revision"))?;
    Ok(())
}

async fn rotate_channel_api(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RotateApiRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    if input.api_key_id.trim().is_empty()
        || input.api_key_id.len() > 200
        || input.api_secret.len() < 16
        || input.api_secret.len() > MAX_PAYMENT_SECRET_BYTES
    {
        return Err(AppError::BadRequest(
            "invalid DujiaoPay API credentials".into(),
        ));
    }
    {
        let conn = state.store.conn.lock().await;
        editable_revision_tx(&conn, &id, input.expected_version)?;
    }
    let credentials = Credentials {
        key_id: input.api_key_id.trim().into(),
        api_secret: Zeroizing::new(input.api_secret.clone()),
        webhook_secret: None,
    };
    let merchant_id = state.market_payments.client.whoami(&credentials).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin payment API rotation"))?;
    let old_revision = editable_revision_tx(&tx, &id, input.expected_version)?;
    let old = load_revision(&tx, &state.market_payments, &id, old_revision)?;
    if merchant_id != old.merchant_id {
        return Err(AppError::Conflict(
            "payment merchant changed; create a new channel".into(),
        ));
    }
    let credentials = Credentials {
        webhook_secret: old.credentials.webhook_secret.clone(),
        ..credentials
    };
    let time = now();
    let revision = old_revision + 1;
    insert_rotated_revision_tx(
        &tx,
        &state.market_payments,
        &id,
        revision,
        &old,
        &credentials,
        &time,
    )?;
    let changed = tx.execute(
        "UPDATE market_payment_channels SET current_revision = ?2, enabled = 0,
            version = version + 1, updated_at = ?3, api_reverify_required = 0,
            webhook_verified_at = CASE WHEN webhook_verified_revision = ?4 THEN webhook_verified_at ELSE NULL END,
            webhook_verified_revision = CASE WHEN webhook_verified_revision = ?4 THEN ?2 ELSE NULL END
         WHERE id = ?1 AND version = ?5 AND archived_at IS NULL",
        params![id, revision, time, old_revision, input.expected_version],
    ).map_err(db_error("rotate payment API credentials"))?;
    if changed != 1 {
        return Err(AppError::Conflict("payment channel version changed".into()));
    }
    audit_tx(
        &tx,
        &session.user_id,
        "channel_api_rotated",
        "payment_channel",
        &id,
        None,
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit payment API rotation"))?;
    Ok((no_store(), Json(channel_view_tx(&conn, &id)?)))
}

async fn rotate_channel_webhook(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RotateWebhookRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    if !matches!(input.webhook_mode.as_str(), "signed" | "poll_only")
        || (input.webhook_mode == "signed"
            && input
                .webhook_secret
                .as_ref()
                .is_none_or(|s| !(16..=MAX_PAYMENT_SECRET_BYTES).contains(&s.len())))
        || (input.webhook_mode == "poll_only" && input.webhook_secret.is_some())
    {
        return Err(AppError::BadRequest(
            "invalid DujiaoPay webhook configuration".into(),
        ));
    }
    let current_credentials = {
        let conn = state.store.conn.lock().await;
        let revision = editable_revision_tx(&conn, &id, input.expected_version)?;
        let current = load_revision(&conn, &state.market_payments, &id, revision)?;
        // No network request while holding the database lock.
        current.credentials
    };
    let verified_merchant_id = state
        .market_payments
        .client
        .whoami(&current_credentials)
        .await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin payment webhook rotation"))?;
    let old_revision = editable_revision_tx(&tx, &id, input.expected_version)?;
    let old = load_revision(&tx, &state.market_payments, &id, old_revision)?;
    if verified_merchant_id != old.merchant_id {
        return Err(AppError::Conflict(
            "payment merchant identity changed".into(),
        ));
    }
    let old_signed = old.credentials.webhook_secret.is_some();
    let credentials = Credentials {
        key_id: old.credentials.key_id.clone(),
        api_secret: old.credentials.api_secret.clone(),
        webhook_secret: input.webhook_secret.clone().map(Zeroizing::new),
    };
    let same_secret = old.credentials.webhook_secret.as_ref().map(|s| s.as_str())
        == credentials.webhook_secret.as_ref().map(|s| s.as_str());
    if same_secret {
        return Err(AppError::BadRequest(
            "webhook credential is unchanged".into(),
        ));
    }
    let time = now();
    let revision = old_revision + 1;
    insert_rotated_revision_tx(
        &tx,
        &state.market_payments,
        &id,
        revision,
        &old,
        &credentials,
        &time,
    )?;
    let graceful = old_signed && input.webhook_mode == "signed" && !input.emergency_revoke_previous;
    let previous_revision = graceful.then_some(old_revision);
    let valid_until = graceful
        .then(|| (Utc::now() + chrono::Duration::minutes(WEBHOOK_KEY_GRACE_MINUTES)).to_rfc3339());
    let changed = tx
        .execute(
        "UPDATE market_payment_channels SET current_revision = ?2, enabled = 0,
            version = version + 1, updated_at = ?3, api_reverify_required = 0, webhook_verified_at = NULL,
            webhook_verified_revision = NULL, previous_webhook_revision = ?4,
            previous_webhook_valid_until = ?5
         WHERE id = ?1 AND version = ?6 AND archived_at IS NULL",
            params![
                id,
                revision,
                time,
                previous_revision,
                valid_until,
                input.expected_version
            ],
        )
        .map_err(db_error("rotate payment webhook secret"))?;
    if changed != 1 {
        return Err(AppError::Conflict("payment channel version changed".into()));
    }
    audit_tx(
        &tx,
        &session.user_id,
        if input.emergency_revoke_previous {
            "channel_webhook_emergency_rotated"
        } else {
            "channel_webhook_rotated"
        },
        "payment_channel",
        &id,
        Some(&input.webhook_mode),
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit payment webhook rotation"))?;
    Ok((no_store(), Json(channel_view_tx(&conn, &id)?)))
}

fn update_channel_archive_tx(
    conn: &Connection,
    id: &str,
    archived: bool,
    expected_version: i64,
    time: &str,
) -> Result<(), AppError> {
    let (_, version, archived_at) = channel_edit_state_tx(conn, id)?;
    if version != expected_version {
        return Err(AppError::Conflict("payment channel version changed".into()));
    }
    if archived == archived_at.is_some() {
        return Err(AppError::Conflict(
            "payment channel archive state unchanged".into(),
        ));
    }
    let changed = conn
        .execute(
            "UPDATE market_payment_channels SET archived_at = ?2, enabled = 0,
            api_reverify_required = 1, webhook_verified_at = NULL,
            webhook_verified_revision = NULL, previous_webhook_revision = NULL,
            previous_webhook_valid_until = NULL,
            version = version + 1, updated_at = ?3 WHERE id = ?1 AND version = ?4",
            params![id, archived.then_some(time), time, expected_version],
        )
        .map_err(db_error("archive payment channel"))?;
    if changed != 1 {
        return Err(AppError::Conflict("payment channel version changed".into()));
    }
    Ok(())
}

async fn set_channel_archived(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ArchiveChannelRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin payment channel archive"))?;
    let time = now();
    update_channel_archive_tx(&tx, &id, input.archived, input.expected_version, &time)?;
    audit_tx(
        &tx,
        &session.user_id,
        if input.archived {
            "channel_archived"
        } else {
            "channel_restored"
        },
        "payment_channel",
        &id,
        None,
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit payment channel archive"))?;
    Ok((no_store(), Json(channel_view_tx(&conn, &id)?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetEnabledRequest {
    enabled: bool,
    expected_version: i64,
}

/// Enabling a channel puts its current revision into canary unless that
/// revision already holds strictly verified live credit. A canary accepts
/// orders only from the Router owner; the first verified credit (with a
/// verified signed Webhook when the wallet is in production) promotes it.
/// Disabling always returns it to `off`. This replaces the earlier rule that
/// demanded live evidence *before* enabling, which no new revision could meet.
fn set_channel_enabled_tx(
    conn: &Connection,
    id: &str,
    enabled: bool,
    expected_version: i64,
    time: &str,
) -> Result<(), AppError> {
    editable_revision_tx(conn, id, expected_version)?;
    let changed = conn
        .execute(
            "UPDATE market_payment_channels
             SET rollout_state = CASE
                     WHEN ?2 = 0 THEN 'off'
                     WHEN rollout_state != 'off' THEN rollout_state
                     WHEN EXISTS (SELECT 1 FROM market_verified_topup_evidence t
                                  WHERE t.channel_id = ?1 AND t.channel_revision = current_revision)
                          AND ((SELECT mode FROM market_finance_control WHERE id = 1) != 'enabled'
                               OR (webhook_verified_at IS NOT NULL
                                   AND webhook_verified_revision = current_revision))
                     THEN 'live' ELSE 'canary' END,
                 updated_at = ?3, version = version + 1
             WHERE id = ?1 AND version = ?4 AND archived_at IS NULL
               AND (?2 = 0 OR api_reverify_required = 0)
               AND (?2 = 0 OR (SELECT verified_at FROM market_payment_channel_revisions
                    WHERE channel_id = ?1 AND revision = current_revision) IS NOT NULL)
               AND (?2 = 0 OR (SELECT mode FROM market_finance_control WHERE id = 1) != 'enabled'
                    OR (SELECT webhook_mode FROM market_payment_channel_revisions
                        WHERE channel_id = ?1 AND revision = current_revision) = 'signed')",
            params![id, i64::from(enabled), time, expected_version],
        )
        .map_err(db_error("toggle payment channel"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "payment channel has not passed verification for this finance mode".into(),
        ));
    }
    Ok(())
}

/// Promote a canary revision after its first strictly verified credit.
/// Returns whether a promotion happened.
fn promote_channel_if_proven_tx(
    conn: &Connection,
    channel_id: &str,
    time: &str,
) -> Result<bool, AppError> {
    let changed = conn
        .execute(
            "UPDATE market_payment_channels
             SET rollout_state = 'live', promoted_at = ?2, updated_at = ?2,
                 version = version + 1
             WHERE id = ?1 AND rollout_state = 'canary' AND archived_at IS NULL
               AND EXISTS (SELECT 1 FROM market_verified_topup_evidence t
                           WHERE t.channel_id = ?1 AND t.channel_revision = current_revision)
               AND ((SELECT mode FROM market_finance_control WHERE id = 1) != 'enabled'
                    OR (webhook_verified_at IS NOT NULL
                        AND webhook_verified_revision = current_revision
                        AND (SELECT webhook_mode FROM market_payment_channel_revisions
                             WHERE channel_id = ?1 AND revision = current_revision) = 'signed'))",
            params![channel_id, time],
        )
        .map_err(db_error("promote canary payment channel"))?;
    if changed == 1 {
        audit_tx(
            conn,
            "system",
            "channel_promoted_live",
            "payment_channel",
            channel_id,
            Some("verified_canary_credit"),
            time,
        )?;
    }
    Ok(changed == 1)
}

async fn set_channel_enabled(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<SetEnabledRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin payment channel toggle"))?;
    set_channel_enabled_tx(&tx, &id, input.enabled, input.expected_version, &now())?;
    audit_tx(
        &tx,
        &session.user_id,
        if input.enabled {
            "channel_enabled"
        } else {
            "channel_disabled"
        },
        "payment_channel",
        &id,
        None,
        &now(),
    )?;
    tx.commit()
        .map_err(db_error("commit payment channel toggle"))?;
    Ok((
        no_store(),
        Json(serde_json::json!({ "enabled": input.enabled })),
    ))
}

/// Re-encrypt every stored credential revision with the newest master key
/// version so an older key can be retired. Idempotent.
fn rewrap_channel_secrets_tx(
    conn: &Connection,
    runtime: &MarketPaymentsRuntime,
) -> Result<usize, AppError> {
    let revisions = {
        let mut statement = conn
            .prepare(
                "SELECT channel_id, revision, merchant_fingerprint, webhook_mode
                 FROM market_payment_channel_revisions WHERE key_version != ?1",
            )
            .map_err(db_error("prepare payment secret rewrap"))?;
        statement
            .query_map(params![runtime.cipher.version], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(db_error("query payment secret rewrap"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read payment secret rewrap"))?
    };
    let mut count = 0;
    for (channel_id, revision, fingerprint, webhook_mode) in revisions {
        let loaded = load_revision(conn, runtime, &channel_id, revision)?;
        let (api_ciphertext, api_nonce) = runtime.cipher.seal(
            loaded.credentials.api_secret.as_str(),
            &aad(&channel_id, revision, &fingerprint, "api"),
        )?;
        let (webhook_ciphertext, webhook_nonce) = match (
            webhook_mode.as_str(),
            loaded.credentials.webhook_secret.as_ref(),
        ) {
            ("signed", Some(secret)) => runtime.cipher.seal(
                secret.as_str(),
                &aad(&channel_id, revision, &fingerprint, "webhook"),
            )?,
            _ => (String::new(), String::new()),
        };
        conn.execute(
            "UPDATE market_payment_channel_revisions
             SET api_secret_ciphertext = ?3, api_secret_nonce = ?4,
                 webhook_secret_ciphertext = ?5, webhook_secret_nonce = ?6, key_version = ?7
             WHERE channel_id = ?1 AND revision = ?2",
            params![
                channel_id,
                revision,
                api_ciphertext,
                api_nonce,
                webhook_ciphertext,
                webhook_nonce,
                runtime.cipher.version
            ],
        )
        .map_err(db_error("rewrap payment secret"))?;
        count += 1;
    }
    Ok(count)
}

async fn rewrap_channel_secrets(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin payment secret rewrap"))?;
    let count = rewrap_channel_secrets_tx(&tx, &state.market_payments)?;
    audit_tx(
        &tx,
        &session.user_id,
        "channel_secrets_rewrapped",
        "finance_control",
        "1",
        None,
        &now(),
    )?;
    tx.commit()
        .map_err(db_error("commit payment secret rewrap"))?;
    Ok((
        no_store(),
        Json(serde_json::json!({
            "rewrapped": count,
            "keyVersion": state.market_payments.cipher.version,
        })),
    ))
}

async fn list_earning_holds(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(crate::market_finance::active_earning_holds_tx(&conn, None)?),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManualHoldRequest {
    contract_id: String,
    reason_code: String,
}

fn valid_reason_code(reason: &str) -> bool {
    (3..=64).contains(&reason.len())
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// The Owner may freeze a contract's earnings, for example while a Provider
/// identity conflict is investigated.
async fn place_manual_earning_hold(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ManualHoldRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    if !valid_reason_code(&input.reason_code) {
        return Err(AppError::BadRequest(
            "a short audit reason code is required".into(),
        ));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin manual earning hold"))?;
    let time = now();
    crate::market_finance::hold_contract_earnings_tx(&tx, &input.contract_id, "manual", &time)?;
    audit_tx(
        &tx,
        &session.user_id,
        "earning_hold_placed",
        "platform_contract",
        &input.contract_id,
        Some(&input.reason_code),
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit manual earning hold"))?;
    Ok((no_store(), Json(serde_json::json!({ "held": true }))))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseHoldRequest {
    reason_kind: String,
    reason_ref: String,
    reason_code: String,
}

/// Holds that belong to a case or dispute are released by closing that case
/// or dispute; only manual and unexplained legacy holds are released here.
async fn release_manual_earning_hold(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ReleaseHoldRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    if !valid_reason_code(&input.reason_code) {
        return Err(AppError::BadRequest(
            "a short audit reason code is required".into(),
        ));
    }
    if !matches!(input.reason_kind.as_str(), "manual" | "legacy_unknown") {
        return Err(AppError::Conflict(
            "close the linked dispute or finance case to release this hold".into(),
        ));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin earning hold release"))?;
    let time = now();
    let released = crate::market_finance::release_earning_holds_tx(
        &tx,
        &input.reason_kind,
        &input.reason_ref,
        &session.user_id,
        &input.reason_code,
        &time,
    )?;
    if input.reason_kind == "legacy_unknown" {
        tx.execute(
            "UPDATE market_finance_reconciliation_cases SET status = 'resolved', resolved_at = ?2
             WHERE code = 'earning_hold_legacy' AND object_kind = 'provider'
               AND object_id = ?1 AND status = 'open'",
            params![input.reason_ref, time],
        )
        .map_err(db_error("close legacy earning hold case"))?;
    }
    audit_tx(
        &tx,
        &session.user_id,
        "earning_hold_released",
        &input.reason_kind,
        &input.reason_ref,
        Some(&input.reason_code),
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit earning hold release"))?;
    Ok((
        no_store(),
        Json(serde_json::json!({ "released": released })),
    ))
}

fn audit_tx(
    conn: &Connection,
    actor: &str,
    action: &str,
    object_kind: &str,
    object_id: &str,
    reason: Option<&str>,
    time: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO market_finance_audit_events (id, actor_user_id, action, object_kind, object_id, reason_code, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![Uuid::new_v4().to_string(), actor, action, object_kind, object_id, reason, time],
    ).map_err(db_error("audit market finance action"))?;
    Ok(())
}

fn open_reconciliation_case_tx(
    conn: &Connection,
    code: &str,
    object_kind: &str,
    object_id: &str,
    time: &str,
) -> Result<(), AppError> {
    open_reconciliation_case_detail_tx(conn, code, object_kind, object_id, None, time)
}

fn open_reconciliation_case_detail_tx(
    conn: &Connection,
    code: &str,
    object_kind: &str,
    object_id: &str,
    detail_code: Option<&str>,
    time: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR IGNORE INTO market_finance_reconciliation_cases
         (id, code, object_kind, object_id, status, detail_code, created_at)
         VALUES (?1, ?2, ?3, ?4, 'open', ?5, ?6)",
        params![
            Uuid::new_v4().to_string(),
            code,
            object_kind,
            object_id,
            detail_code,
            time
        ],
    )
    .map_err(db_error("open market finance reconciliation case"))?;
    if let Some(detail_code) = detail_code {
        conn.execute(
            "UPDATE market_finance_reconciliation_cases SET detail_code = ?4
             WHERE code = ?1 AND object_kind = ?2 AND object_id = ?3
               AND status = 'open'
               AND (detail_code IS NULL
                    OR detail_code = ''
                    OR (detail_code = 'gateway_status_unknown'
                        AND ?4 != 'gateway_status_unknown'))",
            params![code, object_kind, object_id, detail_code],
        )
        .map_err(db_error("classify market finance reconciliation case"))?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FinanceCaseView {
    id: String,
    code: String,
    object_kind: String,
    object_id: String,
    status: String,
    detail_code: Option<String>,
    created_at: String,
    resolved_at: Option<String>,
}

async fn list_finance_cases(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(finance_cases_tx(&conn, query.cursor.as_deref())?),
    ))
}

fn finance_cases_tx(
    conn: &Connection,
    cursor: Option<&str>,
) -> Result<Page<FinanceCaseView>, AppError> {
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT created_at FROM market_finance_reconciliation_cases WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read finance case cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid finance case cursor".into()))
        })
        .transpose()?;
    let mut statement = conn
        .prepare(
            "SELECT id, code, object_kind, object_id, status, detail_code, created_at, resolved_at
             FROM market_finance_reconciliation_cases
             WHERE status = 'open'
               AND (?1 IS NULL OR created_at < ?1 OR (created_at = ?1 AND id < ?2))
             ORDER BY created_at DESC, id DESC LIMIT 51",
        )
        .map_err(db_error("prepare market finance cases"))?;
    let mut items = statement
        .query_map(params![cursor_at, cursor], |row| {
            Ok(FinanceCaseView {
                id: row.get(0)?,
                code: row.get(1)?,
                object_kind: row.get(2)?,
                object_id: row.get(3)?,
                status: row.get(4)?,
                detail_code: row.get(5)?,
                created_at: row.get(6)?,
                resolved_at: row.get(7)?,
            })
        })
        .map_err(db_error("query market finance cases"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read market finance cases"))?;
    let has_more = items.len() > 50;
    items.truncate(50);
    let next_cursor = has_more.then(|| items.last().unwrap().id.clone());
    Ok(Page { items, next_cursor })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveFinanceCaseRequest {
    reason_code: String,
}

async fn resolve_finance_case(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ResolveFinanceCaseRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_admin_session(&state, &headers).await?;
    let owner = crate::api::is_finance_owner(&state.config, &actor.email);
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin finance case resolution"))?;
    ensure_general_case_can_resolve_tx(&tx, &id)?;
    if !owner && crate::market_reconcile::case_is_critical_tx(&tx, &id)? {
        return Err(AppError::Forbidden(
            "only the Router owner may close a critical finance case".into(),
        ));
    }
    crate::market_reconcile::resolve_case_tx(
        &tx,
        &id,
        &actor.user_id,
        &request.reason_code,
        &now(),
    )?;
    tx.commit()
        .map_err(db_error("commit finance case resolution"))?;
    Ok((no_store(), Json(serde_json::json!({ "ok": true }))))
}

fn ensure_general_case_can_resolve_tx(conn: &Connection, id: &str) -> Result<(), AppError> {
    let topup_case: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases
             WHERE id = ?1 AND object_kind = 'topup')",
            params![id],
            |row| row.get(0),
        )
        .map_err(db_error("check finance case kind"))?;
    if topup_case {
        return Err(AppError::Conflict(
            "top-up cases require a verified payment or unpaid recovery decision".into(),
        ));
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminTopupView {
    id: String,
    user_id: String,
    channel_id: String,
    merchant_order_id: String,
    gateway_order_id: Option<String>,
    amount_minor: i64,
    status: String,
    created_at: String,
    updated_at: String,
    buyer_abandoned_at: Option<String>,
    user_email: Option<String>,
    order_mode: String,
    create_last_error_code: Option<String>,
    reconcile_failures: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TopupGatewayObservationView {
    id: String,
    source: String,
    gateway_status: String,
    evidence_flags: Vec<String>,
    evidence_sha256: String,
    first_observed_at: String,
    last_observed_at: String,
    observation_count: i64,
}

async fn list_admin_topup_observations(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_topup_orders WHERE id = ?1)",
            params![order_id],
            |row| row.get(0),
        )
        .map_err(db_error("find top-up observations target"))?;
    if !exists {
        return Err(AppError::NotFound("top-up order not found".into()));
    }
    Ok((
        no_store(),
        Json(admin_topup_observations_tx(
            &conn,
            &order_id,
            query.cursor.as_deref(),
        )?),
    ))
}

fn admin_topup_observations_tx(
    conn: &Connection,
    order_id: &str,
    cursor: Option<&str>,
) -> Result<Page<TopupGatewayObservationView>, AppError> {
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT first_observed_at FROM market_topup_gateway_observations
                 WHERE id = ?1 AND order_id = ?2",
                params![id, order_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read top-up observation cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid top-up observation cursor".into()))
        })
        .transpose()?;
    let mut statement = conn
        .prepare(
            "SELECT id, source, gateway_status, evidence_flags_json, evidence_sha256,
                first_observed_at, last_observed_at, observation_count
         FROM market_topup_gateway_observations WHERE order_id = ?1
           AND (?2 IS NULL OR first_observed_at < ?2
                OR (first_observed_at = ?2 AND id < ?3))
         ORDER BY first_observed_at DESC, id DESC LIMIT 51",
        )
        .map_err(db_error("prepare top-up observations"))?;
    let mut items = statement
        .query_map(params![order_id, cursor_at, cursor], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .map_err(db_error("query top-up observations"))?
        .map(|row| {
            let (
                id,
                source,
                gateway_status,
                flags_json,
                evidence_sha256,
                first_observed_at,
                last_observed_at,
                observation_count,
            ) = row.map_err(db_error("read top-up observation"))?;
            let evidence_flags = serde_json::from_str(&flags_json)
                .map_err(|_| AppError::Internal("invalid redacted top-up observation".into()))?;
            Ok(TopupGatewayObservationView {
                id,
                source,
                gateway_status,
                evidence_flags,
                evidence_sha256,
                first_observed_at,
                last_observed_at,
                observation_count,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let has_more = items.len() > 50;
    items.truncate(50);
    let next_cursor = has_more.then(|| items.last().unwrap().id.clone());
    Ok(Page { items, next_cursor })
}

async fn list_admin_topups(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(admin_topups_tx(&conn, query.cursor.as_deref())?),
    ))
}

fn admin_topups_tx(
    conn: &Connection,
    cursor: Option<&str>,
) -> Result<Page<AdminTopupView>, AppError> {
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT created_at FROM market_topup_orders WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read admin top-up cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid admin top-up cursor".into()))
        })
        .transpose()?;
    let mut statement = conn
        .prepare(
            "SELECT id, user_id, channel_id, merchant_order_id, gateway_order_id,
                    amount_minor, status, created_at, updated_at, buyer_abandoned_at,
                    (SELECT email_normalized FROM users WHERE users.id = market_topup_orders.user_id),
                    order_mode, create_last_error_code, reconcile_failures
             FROM market_topup_orders
             WHERE (status IN ('review_required', 'create_unknown')
                OR (status IN ('created', 'creating', 'pending', 'verifying')
                    AND (updated_at < ?1 OR (buyer_abandoned_at IS NOT NULL AND buyer_abandoned_at < ?1))))
               AND (?2 IS NULL OR created_at > ?2 OR (created_at = ?2 AND id > ?3))
             ORDER BY created_at, id LIMIT 51",
        )
        .map_err(db_error("prepare unresolved top-ups"))?;
    let cutoff = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let mut items = statement
        .query_map(params![cutoff, cursor_at, cursor], |row| {
            Ok(AdminTopupView {
                id: row.get(0)?,
                user_id: row.get(1)?,
                channel_id: row.get(2)?,
                merchant_order_id: row.get(3)?,
                gateway_order_id: row.get(4)?,
                amount_minor: row.get(5)?,
                status: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                buyer_abandoned_at: row.get(9)?,
                user_email: row.get(10)?,
                order_mode: row.get(11)?,
                create_last_error_code: row.get(12)?,
                reconcile_failures: row.get(13)?,
            })
        })
        .map_err(db_error("query unresolved top-ups"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read unresolved top-ups"))?;
    let has_more = items.len() > 50;
    items.truncate(50);
    let next_cursor = has_more.then(|| items.last().unwrap().id.clone());
    Ok(Page { items, next_cursor })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoverTopupRequest {
    expected_status: String,
    expected_updated_at: String,
    gateway_order_id: Option<String>,
    reason_code: String,
    manual_reference: Option<String>,
    #[serde(default)]
    manual_confirmed: bool,
    dry_run: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoverTopupView {
    order_id: String,
    gateway_status: String,
    selection_deadline: Option<String>,
    expires_at: Option<String>,
    can_recover: bool,
    can_cancel: bool,
    recovered: bool,
    requires_manual_reference: bool,
    blockers: Vec<&'static str>,
    review_case_codes: Vec<String>,
}

struct RecoveryAssessment {
    blockers: Vec<&'static str>,
    review_case_codes: Vec<String>,
    open_case_ids: Vec<String>,
    requires_manual_reference: bool,
}

impl RecoveryAssessment {
    fn can_recover(&self) -> bool {
        self.blockers.is_empty()
    }
}

fn can_cancel_gateway_topup(
    local_status: &str,
    gateway_status: &str,
    assessment: &RecoveryAssessment,
    allow_review: bool,
) -> bool {
    (local_status == "pending" || (allow_review && local_status == "review_required"))
        && gateway_status == "pending"
        && assessment.blockers.as_slice() == ["gateway_not_terminal"]
}

fn can_pay_gateway_topup(
    local_status: &str,
    gateway: &GatewayOrder,
    assessment: &RecoveryAssessment,
    abandoned: bool,
    cancel_requested: bool,
    time: chrono::DateTime<Utc>,
) -> bool {
    if local_status != "pending"
        || abandoned
        || cancel_requested
        || !matches!(gateway.status.as_str(), "awaiting_payment" | "pending")
        || assessment.blockers.as_slice() != ["gateway_not_terminal"]
    {
        return false;
    }
    let deadline = if gateway.status == "awaiting_payment" {
        gateway.selection_deadline.as_deref()
    } else {
        gateway.expires_at.as_deref()
    };
    deadline
        .is_none_or(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok_and(|end| end > time))
}

fn topup_payability_reason(
    local_status: &str,
    gateway: &GatewayOrder,
    assessment: &RecoveryAssessment,
    abandoned: bool,
    cancel_requested: bool,
    has_checkout: bool,
    time: chrono::DateTime<Utc>,
) -> &'static str {
    if local_status != "pending" {
        return "order_changed";
    }
    if matches!(
        gateway.status.as_str(),
        "expired" | "canceled" | "cancelled"
    ) {
        return "gateway_closed";
    }
    if matches!(gateway.status.as_str(), "confirming" | "paid") {
        return "payment_processing";
    }
    if !matches!(gateway.status.as_str(), "awaiting_payment" | "pending") {
        return "gateway_unknown";
    }
    if assessment.blockers.as_slice() != ["gateway_not_terminal"] {
        return "payment_evidence";
    }
    if abandoned {
        return "buyer_stopped";
    }
    if cancel_requested {
        return "cancel_pending";
    }
    if !can_pay_gateway_topup(
        local_status,
        gateway,
        assessment,
        abandoned,
        cancel_requested,
        time,
    ) {
        return "deadline_passed";
    }
    if !has_checkout {
        return "checkout_unavailable";
    }
    "payable"
}

fn gateway_cancel_was_requested_tx(conn: &Connection, order_id: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT cancel_requested_at IS NOT NULL FROM market_topup_orders WHERE id = ?1",
        params![order_id],
        |row| row.get(0),
    )
    .optional()
    .map(|requested| requested.unwrap_or(false))
    .map_err(db_error("check earlier gateway cancellation request"))
}

/// Record the one permitted gateway cancellation POST. The marker hides the
/// checkout from every Router surface; the stored URL itself is kept so that
/// "no URL" continues to mean only "create result unknown".
fn mark_gateway_cancel_requested_tx(
    conn: &Connection,
    actor: &str,
    order_id: &str,
    time: &str,
) -> Result<(), AppError> {
    audit_tx(
        conn,
        actor,
        "topup_gateway_cancel_requested",
        "topup",
        order_id,
        None,
        time,
    )?;
    conn.execute(
        "UPDATE market_topup_orders SET cancel_requested_at = COALESCE(cancel_requested_at, ?2)
         WHERE id = ?1",
        params![order_id, time],
    )
    .map_err(db_error("record gateway cancellation request"))?;
    Ok(())
}

fn valid_manual_reference(reference: &str) -> bool {
    (8..=128).contains(&reference.len())
        && reference.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
        })
}

struct RecoverTopupSnapshot {
    channel_id: String,
    revision: i64,
    status: String,
    updated_at: String,
    gateway_order_id: Option<String>,
}

fn recovery_snapshot_tx(
    conn: &Connection,
    order_id: &str,
) -> Result<RecoverTopupSnapshot, AppError> {
    conn.query_row(
        "SELECT channel_id, channel_revision, status, updated_at, gateway_order_id
         FROM market_topup_orders WHERE id = ?1",
        params![order_id],
        |row| {
            Ok(RecoverTopupSnapshot {
                channel_id: row.get(0)?,
                revision: row.get(1)?,
                status: row.get(2)?,
                updated_at: row.get(3)?,
                gateway_order_id: row.get(4)?,
            })
        },
    )
    .optional()
    .map_err(db_error("read top-up recovery snapshot"))?
    .ok_or_else(|| AppError::NotFound("top-up order not found".into()))
}

fn recovery_gateway_id(
    snapshot: &RecoverTopupSnapshot,
    input: &RecoverTopupRequest,
) -> Result<String, AppError> {
    if snapshot.status != input.expected_status || snapshot.updated_at != input.expected_updated_at
    {
        return Err(AppError::Conflict(
            "top-up changed; reload before recovery".into(),
        ));
    }
    if !matches!(
        snapshot.status.as_str(),
        "created" | "creating" | "create_unknown" | "pending" | "verifying" | "review_required"
    ) {
        return Err(AppError::Conflict(
            "top-up is not eligible for manual recovery".into(),
        ));
    }
    match (&snapshot.gateway_order_id, &input.gateway_order_id) {
        (Some(stored), Some(supplied)) if stored != supplied => Err(AppError::Conflict(
            "gateway order ID conflicts with stored order".into(),
        )),
        (Some(stored), _) => Ok(stored.clone()),
        (None, Some(supplied)) => Ok(supplied.clone()),
        (None, None) => Err(AppError::BadRequest(
            "gateway order ID from DujiaoPay is required".into(),
        )),
    }
}

fn recovery_assessment_tx(
    conn: &Connection,
    order_id: &str,
    status: &str,
    gateway: &GatewayOrder,
) -> Result<RecoveryAssessment, AppError> {
    let mut assessment = RecoveryAssessment {
        blockers: gateway_recovery_blockers(gateway),
        review_case_codes: Vec::new(),
        open_case_ids: Vec::new(),
        requires_manual_reference: false,
    };
    let (has_receipt, has_journal): (bool, bool) = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_topup_receipts WHERE order_id = ?1),
                    EXISTS(SELECT 1 FROM market_journal_transactions
                           WHERE source_kind = 'topup' AND source_id = ?1)",
            params![order_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("check top-up accounting evidence"))?;
    if has_receipt {
        assessment.blockers.push("local_receipt_present");
    }
    if has_journal {
        assessment.blockers.push("local_journal_present");
    }

    // A historical terminal response may once have carried payment fields.
    // Only a *current* authenticated, evidence-free terminal response plus a
    // documented merchant-side check may release that hold. The event and case
    // remain immutable evidence of the earlier ambiguity.
    let had_terminal_payment_evidence: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases
             WHERE object_kind = 'topup' AND object_id = ?1
               AND code = 'topup_verification_failed'
               AND detail_code = 'gateway_terminal_payment_evidence' AND status = 'open')",
            params![order_id],
            |row| row.get(0),
        )
        .map_err(db_error("read historical terminal payment evidence"))?;
    let terminal_evidence_cleared =
        had_terminal_payment_evidence && gateway_terminal_without_payment_evidence(gateway);

    let mut events = conn
        .prepare("SELECT event_type, status FROM market_topup_events WHERE order_id = ?1")
        .map_err(db_error("prepare top-up recovery events"))?;
    let event_rows = events
        .query_map(params![order_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error("query top-up recovery events"))?;
    let mut has_progress_review = false;
    for event in event_rows {
        let (event_type, event_status) = event.map_err(db_error("read top-up recovery event"))?;
        let progress = is_non_payment_lifecycle_event(&event_type);
        has_progress_review |= progress && event_status == "review_required";
        if !progress
            && !matches!(
                event_type.as_str(),
                "order.expired" | "order.cancelled" | "order.canceled"
            )
        {
            assessment.blockers.push("payment_or_unknown_event");
        } else if event_status == "pending"
            || (event_status == "review_required"
                && !progress
                && !(terminal_evidence_cleared
                    && matches!(
                        event_type.as_str(),
                        "order.expired" | "order.cancelled" | "order.canceled"
                    )))
        {
            assessment.blockers.push("unprocessed_or_disputed_event");
        }
    }

    let mut cases = conn
        .prepare(
            "SELECT id, code, status, COALESCE(detail_code, '')
             FROM market_finance_reconciliation_cases
             WHERE object_kind = 'topup' AND object_id = ?1 ORDER BY created_at, id",
        )
        .map_err(db_error("prepare top-up recovery cases"))?;
    let case_rows = cases
        .query_map(params![order_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(db_error("query top-up recovery cases"))?;
    let mut has_review_case = false;
    for case in case_rows {
        let (id, code, case_status, detail) =
            case.map_err(db_error("read top-up recovery case"))?;
        assessment.review_case_codes.push(code.clone());
        if case_status == "open" {
            assessment.open_case_ids.push(id);
        }
        match code.as_str() {
            "unknown_gateway_event" if has_progress_review && case_status == "open" => {
                has_review_case = true;
            }
            "unknown_gateway_event" if has_progress_review => {}
            "unknown_gateway_event" => assessment.blockers.push("unclassified_review_case"),
            "topup_verification_failed"
                if detail.is_empty() || detail == "gateway_status_unknown" =>
            {
                if case_status == "open" {
                    has_review_case = true;
                }
                if case_status == "open" || status == "review_required" {
                    assessment.requires_manual_reference = true;
                }
            }
            "topup_verification_failed"
                if detail == "gateway_terminal_payment_evidence"
                    && case_status == "open"
                    && terminal_evidence_cleared =>
            {
                has_review_case = true;
                assessment.requires_manual_reference = true;
            }
            "topup_verification_failed"
                if matches!(
                    detail.as_str(),
                    "gateway_terminal_payment_evidence"
                        | "paid_receipt_invalid"
                        | "paid_amount_short"
                        | "paid_method_not_allowed"
                        | "paid_confirmations_low"
                        | "paid_source_manual"
                ) =>
            {
                assessment.blockers.push("verification_payment_evidence");
            }
            "topup_verification_failed" => assessment
                .blockers
                .push("verification_identity_or_create_conflict"),
            _ => assessment.blockers.push("other_review_case"),
        }
    }
    assessment.review_case_codes.sort();
    assessment.review_case_codes.dedup();
    if status == "review_required" && !has_review_case {
        assessment.blockers.push("review_cause_unclassified");
    } else if status != "review_required" && !assessment.open_case_ids.is_empty() {
        assessment.blockers.push("other_review_case");
    }
    assessment.blockers.sort();
    assessment.blockers.dedup();
    Ok(assessment)
}

fn apply_owner_verified_recovery_tx(
    conn: &Connection,
    order_id: &str,
    input: &RecoverTopupRequest,
    loaded: &LoadedRevision,
    gateway: &GatewayOrder,
    actor: &str,
    time: &str,
) -> Result<RecoveryAssessment, AppError> {
    let current = recovery_snapshot_tx(conn, order_id)?;
    let gateway_id = recovery_gateway_id(&current, input)?;
    let identity_conflict = match validate_gateway_identity_tx(
        conn,
        order_id,
        &current.channel_id,
        current.revision,
        loaded,
        &gateway_id,
        gateway,
    ) {
        Ok(()) => false,
        Err(AppError::Conflict(_)) => true,
        Err(error) => return Err(error),
    };
    let mut assessment = recovery_assessment_tx(conn, order_id, &current.status, gateway)?;
    if identity_conflict {
        assessment.blockers.push("gateway_identity_mismatch");
    }
    record_gateway_observation_tx(conn, order_id, "owner_review", gateway, time)?;
    if input.dry_run {
        return Ok(assessment);
    }
    if !assessment.can_recover() {
        return Err(AppError::Conflict(
            "gateway payment or review evidence prevents recovery".into(),
        ));
    }
    if assessment.requires_manual_reference
        && (!input.manual_confirmed
            || !input
                .manual_reference
                .as_deref()
                .is_some_and(valid_manual_reference))
    {
        return Err(AppError::BadRequest(
            "merchant review confirmation and reference are required for an unclassified historical hold".into(),
        ));
    }
    let status = verified_unpaid_status(&gateway.status)?;
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
             SET gateway_order_id = COALESCE(gateway_order_id, ?2), status = ?3,
                 checkout_url = NULL, updated_at = ?4, next_reconcile_at = ?4
             WHERE id = ?1 AND status = ?5 AND updated_at = ?6
               AND (gateway_order_id IS NULL OR gateway_order_id = ?2)",
            params![
                order_id,
                gateway_id,
                status,
                time,
                current.status,
                current.updated_at
            ],
        )
        .map_err(db_error("close owner-verified unpaid top-up"))?;
    if changed != 1 {
        return Err(AppError::Conflict("top-up changed during recovery".into()));
    }
    for case_id in &assessment.open_case_ids {
        crate::market_reconcile::resolve_case_tx(conn, case_id, actor, &input.reason_code, time)?;
    }
    let evidence = serde_json::json!({
        "orderId": order_id,
        "gatewayOrderId": gateway.order_id,
        "merchantOrderId": gateway.merchant_order_id,
        "gatewayStatus": gateway.status,
        "fiatCurrency": gateway.fiat_currency,
        "fiatAmountMinor": gateway.fiat_amount_minor,
        "paidSource": gateway.paid_source,
        "transactionId": gateway.tx_id,
        "paidAt": gateway.paid_at,
        "confirmations": gateway.confirmations,
        "settledAmount": gateway.settled_amount,
        "reviewCaseCodes": assessment.review_case_codes,
        "paymentEvidenceBlockers": gateway_recovery_blockers(gateway),
        "manualConfirmed": input.manual_confirmed,
    });
    let evidence_sha256 = hex::encode(Sha256::digest(evidence.to_string().as_bytes()));
    let cases_json = serde_json::to_string(&assessment.review_case_codes)
        .map_err(|_| AppError::Internal("encode top-up recovery case codes failed".into()))?;
    conn.execute(
        "INSERT INTO market_topup_recovery_decisions
         (id, order_id, actor_user_id, gateway_order_id, gateway_status,
          gateway_checked_at, evidence_sha256, review_case_codes_json,
          manual_reference, manual_confirmed, reason_code, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            Uuid::new_v4().to_string(),
            order_id,
            actor,
            gateway_id,
            gateway.status,
            time,
            evidence_sha256,
            cases_json,
            input.manual_reference,
            input.manual_confirmed,
            input.reason_code,
            time
        ],
    )
    .map_err(db_error("record top-up recovery evidence"))?;
    audit_tx(
        conn,
        actor,
        "topup_terminal_recovered",
        "topup",
        order_id,
        Some(&input.reason_code),
        time,
    )?;
    finish_buyer_recheck_tx(conn, order_id, Some(status))?;
    Ok(assessment)
}

async fn recover_admin_topup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
    Json(input): Json<RecoverTopupRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_admin_session(&state, &headers).await?;
    if !(3..=64).contains(&input.reason_code.len())
        || !input
            .reason_code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AppError::BadRequest(
            "a short audit reason code is required".into(),
        ));
    }
    if input
        .manual_reference
        .as_deref()
        .is_some_and(|reference| !valid_manual_reference(reference))
    {
        return Err(AppError::BadRequest(
            "a merchant review reference of 8-128 safe characters is required".into(),
        ));
    }
    let (snapshot, loaded) = {
        let conn = state.store.conn.lock().await;
        let snapshot = recovery_snapshot_tx(&conn, &order_id)?;
        let _ = recovery_gateway_id(&snapshot, &input)?;
        let loaded = load_order_revision(
            &conn,
            &state.market_payments,
            &snapshot.channel_id,
            snapshot.revision,
        )?;
        (snapshot, loaded)
    };
    let gateway_id = recovery_gateway_id(&snapshot, &input)?;
    let gateway = get_bound_gateway_order(
        &state,
        &snapshot.channel_id,
        snapshot.revision,
        &loaded,
        &gateway_id,
    )
    .await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin owner top-up recovery"))?;
    let assessment = apply_owner_verified_recovery_tx(
        &tx,
        &order_id,
        &input,
        &loaded,
        &gateway,
        &actor.user_id,
        &now(),
    )?;
    let can_cancel = input.dry_run
        && can_cancel_gateway_topup(&snapshot.status, &gateway.status, &assessment, true)
        && !gateway_cancel_was_requested_tx(&tx, &order_id)?;
    tx.commit()
        .map_err(db_error("commit owner top-up recovery"))?;
    let can_recover = assessment.can_recover();
    Ok((
        no_store(),
        Json(RecoverTopupView {
            order_id,
            gateway_status: gateway.status,
            selection_deadline: gateway.selection_deadline,
            expires_at: gateway.expires_at,
            can_recover,
            can_cancel,
            recovered: !input.dry_run && can_recover,
            requires_manual_reference: assessment.requires_manual_reference,
            blockers: assessment.blockers,
            review_case_codes: assessment.review_case_codes,
        }),
    ))
}

struct LoadedRevision {
    merchant_id: String,
    merchant_fingerprint: String,
    allowed_methods: Vec<String>,
    credentials: Credentials,
}

fn load_revision(
    conn: &Connection,
    runtime: &MarketPaymentsRuntime,
    channel_id: &str,
    revision: i64,
) -> Result<LoadedRevision, AppError> {
    let (
        merchant_id,
        merchant_fingerprint,
        key_id,
        api_ciphertext,
        api_nonce,
        webhook_ciphertext,
        webhook_nonce,
        key_version,
        methods_json,
        webhook_mode,
    ): (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT merchant_id, merchant_fingerprint, api_key_id, api_secret_ciphertext,
                api_secret_nonce, webhook_secret_ciphertext, webhook_secret_nonce,
                key_version, allowed_methods_json, webhook_mode
         FROM market_payment_channel_revisions WHERE channel_id = ?1 AND revision = ?2",
            params![channel_id, revision],
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
                ))
            },
        )
        .map_err(db_error("load payment channel revision"))?;
    let api_secret = runtime.cipher.open(
        &api_ciphertext,
        &api_nonce,
        &aad(channel_id, revision, &merchant_fingerprint, "api"),
        key_version,
    )?;
    let webhook_secret = match webhook_mode.as_str() {
        "signed" => Some(runtime.cipher.open(
            &webhook_ciphertext,
            &webhook_nonce,
            &aad(channel_id, revision, &merchant_fingerprint, "webhook"),
            key_version,
        )?),
        "poll_only" if webhook_ciphertext.is_empty() && webhook_nonce.is_empty() => None,
        _ => {
            return Err(AppError::Internal(
                "invalid stored payment webhook mode".into(),
            ));
        }
    };
    let allowed_methods = serde_json::from_str(&methods_json)
        .map_err(|_| AppError::Internal("stored payment methods invalid".into()))?;
    if hex::encode(Sha256::digest(merchant_id.as_bytes())) != merchant_fingerprint {
        return Err(AppError::Internal(
            "payment merchant identity changed unexpectedly".into(),
        ));
    }
    Ok(LoadedRevision {
        merchant_id,
        merchant_fingerprint,
        allowed_methods,
        credentials: Credentials {
            key_id,
            api_secret,
            webhook_secret,
        },
    })
}

/// Old orders keep their amount, merchant identity, and allowed methods from
/// the bound revision. A merchant's current API key may replace the old key
/// for authenticated recovery after the old key has been revoked.
fn load_order_revision(
    conn: &Connection,
    runtime: &MarketPaymentsRuntime,
    channel_id: &str,
    revision: i64,
) -> Result<LoadedRevision, AppError> {
    let mut bound = load_revision(conn, runtime, channel_id, revision)?;
    let current_revision: i64 = conn
        .query_row(
            "SELECT current_revision FROM market_payment_channels WHERE id = ?1",
            params![channel_id],
            |row| row.get(0),
        )
        .map_err(db_error("read current payment credentials"))?;
    if current_revision != revision {
        let current = load_revision(conn, runtime, channel_id, current_revision)?;
        if current.merchant_fingerprint == bound.merchant_fingerprint {
            bound.credentials.key_id = current.credentials.key_id;
            bound.credentials.api_secret = current.credentials.api_secret;
        }
    }
    Ok(bound)
}

#[derive(Clone, Copy)]
enum GatewayCredentialSource {
    Current,
    Original,
}

async fn get_bound_gateway_order_with_source(
    state: &ServerState,
    channel_id: &str,
    revision: i64,
    loaded: &LoadedRevision,
    gateway_order_id: &str,
) -> Result<(GatewayOrder, GatewayCredentialSource), AppError> {
    let primary = state
        .market_payments
        .client
        .get_order(&loaded.credentials, gateway_order_id)
        .await;
    let primary_error = match primary {
        Ok(order) => return Ok((order, GatewayCredentialSource::Current)),
        Err(error) => error,
    };
    if !dujiaopay::may_be_key_scoped_order(&primary_error) {
        return Err(primary_error);
    }
    let legacy = {
        let conn = state.store.conn.lock().await;
        load_revision(&conn, &state.market_payments, channel_id, revision)?
    };
    if legacy.merchant_fingerprint == loaded.merchant_fingerprint
        && (legacy.credentials.key_id != loaded.credentials.key_id
            || legacy.credentials.api_secret != loaded.credentials.api_secret)
    {
        // Historical lookups can be key-scoped. A cancellation may use this
        // original key only after the caller validates order identity and
        // rechecks local payment evidence; it is never used to create an order.
        if let Ok(order) = state
            .market_payments
            .client
            .get_order(&legacy.credentials, gateway_order_id)
            .await
        {
            return Ok((order, GatewayCredentialSource::Original));
        }
    }
    Err(primary_error)
}

async fn get_bound_gateway_order(
    state: &ServerState,
    channel_id: &str,
    revision: i64,
    loaded: &LoadedRevision,
    gateway_order_id: &str,
) -> Result<GatewayOrder, AppError> {
    get_bound_gateway_order_with_source(state, channel_id, revision, loaded, gateway_order_id)
        .await
        .map(|(order, _)| order)
}

/// The current merchant Webhook secret signs all order revisions. Only the
/// immediately prior secret may be accepted, and only during a short explicit
/// grace period. An emergency rotation clears that grace atomically.
fn verify_bound_webhook_tx(
    conn: &Connection,
    runtime: &MarketPaymentsRuntime,
    channel_id: &str,
    bound_revision: i64,
    headers: &HeaderMap,
    body: &[u8],
    now_unix: i64,
) -> Result<(LoadedRevision, WebhookEvent, Option<i64>), AppError> {
    let (current_revision, previous_revision, valid_until): (i64, Option<i64>, Option<String>) =
        conn.query_row(
            "SELECT current_revision, previous_webhook_revision, previous_webhook_valid_until
         FROM market_payment_channels WHERE id = ?1",
            params![channel_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db_error("read payment webhook key state"))?;
    let mut bound = load_order_revision(conn, runtime, channel_id, bound_revision)?;
    let current = load_revision(conn, runtime, channel_id, current_revision)?;
    if current.merchant_fingerprint != bound.merchant_fingerprint {
        // Historical configurations could replace a channel with a different
        // merchant. The new API refuses that, but their already-bound orders
        // must continue using the original merchant's credentials.
        let event = dujiaopay::verify_webhook(&bound.credentials, headers, body, now_unix)?;
        return Ok((bound, event, None));
    }
    let current_result = dujiaopay::verify_webhook(&current.credentials, headers, body, now_unix);
    if let Ok(event) = current_result {
        return Ok((bound, event, Some(current_revision)));
    }
    let current_error = current_result.unwrap_err();
    if !matches!(current_error, AppError::Unauthorized(_)) {
        return Err(current_error);
    }
    if let (Some(previous_revision), Some(valid_until)) = (previous_revision, valid_until)
        && chrono::DateTime::parse_from_rfc3339(&valid_until)
            .is_ok_and(|until| until.timestamp() >= now_unix)
    {
        let previous = load_revision(conn, runtime, channel_id, previous_revision)?;
        if previous.merchant_fingerprint == bound.merchant_fingerprint {
            let event = dujiaopay::verify_webhook(&previous.credentials, headers, body, now_unix)?;
            // The API call below still uses the current merchant key. A
            // previous Webhook secret is never a payment authority.
            bound.credentials.webhook_secret = None;
            return Ok((bound, event, None));
        }
    }
    Err(current_error)
}

fn mark_channel_webhook_verified_tx(
    conn: &Connection,
    channel_id: &str,
    revision: i64,
    time: &str,
) -> Result<bool, AppError> {
    let changed = conn
        .execute(
            "UPDATE market_payment_channels
             SET webhook_verified_at = ?3, webhook_verified_revision = ?2
             WHERE id = ?1 AND current_revision = ?2 AND archived_at IS NULL",
            params![channel_id, revision, time],
        )
        .map_err(db_error("mark payment webhook signature verified"))?;
    if changed == 1 {
        promote_channel_if_proven_tx(conn, channel_id, time)?;
    }
    Ok(changed == 1)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateTopupRequest {
    channel_id: String,
    amount_minor: i64,
    quote_id: String,
    /// Required for a direct (in-site) channel; forbidden for a hosted one.
    #[serde(default)]
    token_id: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TopupView {
    id: String,
    channel_id: String,
    amount_minor: i64,
    credit_amount_minor: i64,
    platform_fee_minor: i64,
    platform_fee_rate_bps: i64,
    policy_version: i64,
    status: String,
    checkout_url: Option<String>,
    created_at: String,
    auto_close_at: Option<String>,
    buyer_abandoned_at: Option<String>,
    review_reason: Option<String>,
    buyer_recheck_requested_at: Option<String>,
    buyer_recheck_completed_at: Option<String>,
    buyer_recheck_result: Option<String>,
    last_gateway_status: Option<String>,
    last_gateway_checked_at: Option<String>,
    order_mode: String,
    payment_token_id: Option<String>,
    has_payment_instructions: bool,
    local_close_reason: Option<String>,
    paid_at: Option<String>,
}

/// One projection for every buyer top-up surface. A payment handle (hosted
/// checkout URL or direct instructions) is exposed only while the order is
/// pending, not stopped, not cancel-requested and free of review holds.
const TOPUP_VIEW_SQL: &str = "SELECT id, channel_id, amount_minor, status,
        CASE WHEN status = 'pending' AND buyer_abandoned_at IS NULL
            AND cancel_requested_at IS NULL AND NOT EXISTS (
                SELECT 1 FROM market_finance_reconciliation_cases review
                WHERE review.object_kind = 'topup'
                  AND review.object_id = market_topup_orders.id
                  AND review.status = 'open'
            ) THEN checkout_url ELSE NULL END,
        created_at, auto_close_at,
        buyer_abandoned_at,
        CASE WHEN status = 'review_required' AND EXISTS (
            SELECT 1 FROM market_finance_reconciliation_cases review
            WHERE review.object_kind = 'topup' AND review.object_id = market_topup_orders.id
              AND review.code = 'topup_verification_failed'
              AND review.status = 'open'
              AND review.detail_code = 'paid_receipt_invalid'
        ) THEN 'paid_receipt_invalid'
        WHEN status = 'review_required' AND EXISTS (
            SELECT 1 FROM market_finance_reconciliation_cases review
            WHERE review.object_kind = 'topup' AND review.object_id = market_topup_orders.id
              AND review.code = 'topup_verification_failed'
              AND review.status = 'open'
              AND review.detail_code = 'gateway_terminal_payment_evidence'
        ) THEN 'terminal_payment_marker'
        WHEN status = 'review_required' AND EXISTS (
            SELECT 1 FROM market_finance_reconciliation_cases review
            WHERE review.object_kind = 'topup' AND review.object_id = market_topup_orders.id
              AND review.code = 'topup_verification_failed'
              AND review.status = 'open'
              AND review.detail_code IN ('paid_amount_short', 'paid_method_not_allowed',
                                         'paid_confirmations_low', 'paid_source_manual')
        ) THEN 'paid_terms_review' ELSE NULL END,
        buyer_recheck_requested_at, buyer_recheck_completed_at, buyer_recheck_result,
        (SELECT observation.gateway_status FROM market_topup_gateway_observations observation
         WHERE observation.order_id = market_topup_orders.id
         ORDER BY observation.last_observed_at DESC, observation.id DESC LIMIT 1),
        (SELECT observation.last_observed_at FROM market_topup_gateway_observations observation
         WHERE observation.order_id = market_topup_orders.id
         ORDER BY observation.last_observed_at DESC, observation.id DESC LIMIT 1),
        order_mode, gateway_token_id,
        (status = 'pending' AND buyer_abandoned_at IS NULL AND cancel_requested_at IS NULL
         AND order_mode = 'direct' AND pay_address IS NOT NULL AND payable_amount IS NOT NULL
         AND NOT EXISTS (
             SELECT 1 FROM market_finance_reconciliation_cases review
             WHERE review.object_kind = 'topup'
               AND review.object_id = market_topup_orders.id
               AND review.status = 'open')),
        local_close_reason, paid_at, COALESCE(credit_amount_minor, amount_minor),
        platform_fee_minor, platform_fee_rate_bps, wallet_policy_version
     FROM market_topup_orders";

fn topup_view_row(row: &crate::db::Row<'_>) -> crate::db::Result<TopupView> {
    let status: String = row.get(3)?;
    let buyer_abandoned_at: Option<String> = row.get(7)?;
    Ok(TopupView {
        id: row.get(0)?,
        channel_id: row.get(1)?,
        amount_minor: row.get(2)?,
        credit_amount_minor: row.get(19)?,
        platform_fee_minor: row.get(20)?,
        platform_fee_rate_bps: row.get(21)?,
        policy_version: row.get(22)?,
        checkout_url: if status == "pending" && buyer_abandoned_at.is_none() {
            row.get(4)?
        } else {
            None
        },
        status,
        created_at: row.get(5)?,
        auto_close_at: row.get(6)?,
        buyer_abandoned_at,
        review_reason: row.get(8)?,
        buyer_recheck_requested_at: row.get(9)?,
        buyer_recheck_completed_at: row.get(10)?,
        buyer_recheck_result: row.get(11)?,
        last_gateway_status: row.get(12)?,
        last_gateway_checked_at: row.get(13)?,
        order_mode: row.get(14)?,
        payment_token_id: row.get(15)?,
        has_payment_instructions: row.get::<_, i64>(16)? == 1,
        local_close_reason: row.get(17)?,
        paid_at: row.get(18)?,
    })
}

fn read_topup(conn: &Connection, order_id: &str, user_id: &str) -> Result<TopupView, AppError> {
    conn.query_row(
        &format!("{TOPUP_VIEW_SQL} WHERE id = ?1 AND user_id = ?2"),
        params![order_id, user_id],
        topup_view_row,
    )
    .map_err(|error| {
        if matches!(error, crate::db::Error::QueryReturnedNoRows) {
            AppError::NotFound("top-up order not found".into())
        } else {
            AppError::Internal(format!("read top-up order failed: {error}"))
        }
    })
}

/// Bind a create result. A stopped or cancel-requested order still records
/// the gateway identity and terms; its checkout stays hidden by the marker,
/// and it is never reported as "create unknown" once the gateway ID exists.
fn save_created_order_tx(
    conn: &Connection,
    order_id: &str,
    revision: i64,
    created: &CreatedOrder,
    time: &str,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
             SET gateway_order_id = COALESCE(gateway_order_id, ?2),
                 checkout_url = COALESCE(?3, checkout_url),
                 expires_at = COALESCE(?4, expires_at),
                 gateway_expires_at = COALESCE(?4, gateway_expires_at),
                 pay_address = COALESCE(?7, pay_address),
                 payable_amount = COALESCE(?8, payable_amount),
                 gateway_chain = COALESCE(?9, gateway_chain),
                 gateway_token_id = COALESCE(?10, gateway_token_id),
                 create_last_error_code = NULL,
                 status = CASE
                     WHEN status IN ('credited', 'expired', 'cancelled', 'review_required')
                     THEN status
                     WHEN ?3 IS NULL AND checkout_url IS NULL AND ?7 IS NULL AND pay_address IS NULL
                          AND buyer_abandoned_at IS NULL AND cancel_requested_at IS NULL
                     THEN 'create_unknown' ELSE 'pending' END,
                 updated_at = ?5
             WHERE id = ?1 AND channel_revision = ?6
               AND (gateway_order_id IS NULL OR gateway_order_id = ?2)",
            params![
                order_id,
                created.order_id,
                created.checkout_url,
                created.expires_at,
                time,
                revision,
                created.pay_address,
                created.payable_amount,
                created.chain,
                created.token_id
            ],
        )
        .map_err(db_error("save DujiaoPay order"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "DujiaoPay create result conflicts with stored order".into(),
        ));
    }
    Ok(())
}

async fn get_topup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(read_topup(&conn, &id, &session.user_id)?)))
}

async fn recheck_buyer_topup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin buyer top-up recheck"))?;
    let queued = queue_buyer_recheck_tx(&tx, &order_id, &session.user_id)?;
    let order = read_topup(&tx, &order_id, &session.user_id)?;
    tx.commit()
        .map_err(db_error("commit buyer top-up recheck"))?;
    Ok((
        no_store(),
        Json(serde_json::json!({ "queued": queued, "order": order })),
    ))
}

fn queue_buyer_recheck_tx(
    conn: &Connection,
    order_id: &str,
    user_id: &str,
) -> Result<bool, AppError> {
    let order = read_topup(conn, order_id, user_id)?;
    if !matches!(
        order.status.as_str(),
        "created" | "creating" | "create_unknown" | "pending" | "verifying" | "review_required"
    ) {
        return Err(AppError::Conflict("top-up is already final".into()));
    }
    let time = now();
    let queued = queue_webhook_hint_tx(conn, order_id, &time)?;
    if queued {
        conn.execute(
            "UPDATE market_topup_orders
             SET buyer_recheck_requested_at = ?2, buyer_recheck_completed_at = NULL,
                 buyer_recheck_result = 'queued'
             WHERE id = ?1 AND user_id = ?3",
            params![order_id, time, user_id],
        )
        .map_err(db_error("record buyer top-up recheck request"))?;
    }
    Ok(queued)
}

struct CancelTopupSnapshot {
    user_id: String,
    channel_id: String,
    revision: i64,
    status: String,
    gateway_order_id: String,
}

fn cancel_topup_snapshot_tx(
    conn: &Connection,
    order_id: &str,
    buyer_id: Option<&str>,
) -> Result<CancelTopupSnapshot, AppError> {
    let snapshot = conn
        .query_row(
            "SELECT user_id, channel_id, channel_revision, status, gateway_order_id
             FROM market_topup_orders WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2)",
            params![order_id, buyer_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("read top-up cancellation target"))?
        .ok_or_else(|| AppError::NotFound("top-up order not found".into()))?;
    let gateway_order_id = snapshot.4.ok_or_else(|| {
        AppError::Conflict("DujiaoPay order is not bound yet; wait for reconciliation".into())
    })?;
    Ok(CancelTopupSnapshot {
        user_id: snapshot.0,
        channel_id: snapshot.1,
        revision: snapshot.2,
        status: snapshot.3,
        gateway_order_id,
    })
}

fn topup_buyer_abandoned_tx(conn: &Connection, order_id: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT buyer_abandoned_at IS NOT NULL FROM market_topup_orders WHERE id = ?1",
        params![order_id],
        |row| row.get(0),
    )
    .map_err(db_error("read buyer top-up abandonment"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TopupGatewayStatusView {
    gateway_status: String,
    can_pay: bool,
    payability_reason: &'static str,
    can_cancel: bool,
    can_abandon: bool,
    selection_deadline: Option<String>,
    expires_at: Option<String>,
    server_now: String,
}

struct TopupGatewaySnapshot {
    gateway: GatewayOrder,
    payability_reason: &'static str,
    can_cancel: bool,
    can_abandon: bool,
}

/// One fresh authenticated lookup plus the local payability decision, shared
/// by the hosted-checkout status endpoint and the direct-payment instructions.
async fn topup_gateway_snapshot(
    state: &ServerState,
    order_id: &str,
    user_id: &str,
) -> Result<TopupGatewaySnapshot, AppError> {
    let (snapshot, loaded) = {
        let conn = state.store.conn.lock().await;
        let snapshot = cancel_topup_snapshot_tx(&conn, order_id, Some(user_id))?;
        if snapshot.status != "pending" {
            return Err(AppError::Conflict("top-up is not awaiting payment".into()));
        }
        let loaded = load_order_revision(
            &conn,
            &state.market_payments,
            &snapshot.channel_id,
            snapshot.revision,
        )?;
        (snapshot, loaded)
    };
    let gateway = get_bound_gateway_order(
        state,
        &snapshot.channel_id,
        snapshot.revision,
        &loaded,
        &snapshot.gateway_order_id,
    )
    .await?;
    let conn = state.store.conn.lock().await;
    let current = cancel_topup_snapshot_tx(&conn, order_id, Some(user_id))?;
    validate_gateway_identity_tx(
        &conn,
        order_id,
        &current.channel_id,
        current.revision,
        &loaded,
        &current.gateway_order_id,
        &gateway,
    )?;
    record_gateway_terms_tx(&conn, order_id, &gateway)?;
    let assessment = recovery_assessment_tx(&conn, order_id, &current.status, &gateway)?;
    let abandoned = topup_buyer_abandoned_tx(&conn, order_id)?;
    let cancel_requested = gateway_cancel_was_requested_tx(&conn, order_id)?;
    let view = read_topup(&conn, order_id, user_id)?;
    let has_handle = view.checkout_url.is_some() || view.has_payment_instructions;
    Ok(TopupGatewaySnapshot {
        payability_reason: topup_payability_reason(
            &current.status,
            &gateway,
            &assessment,
            abandoned,
            cancel_requested,
            has_handle,
            Utc::now(),
        ),
        can_cancel: can_cancel_gateway_topup(&current.status, &gateway.status, &assessment, false)
            && !cancel_requested,
        can_abandon: current.status == "pending"
            && gateway.status == "awaiting_payment"
            && assessment.blockers.as_slice() == ["gateway_not_terminal"]
            && !abandoned
            && !cancel_requested,
        gateway,
    })
}

async fn get_topup_gateway_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let snapshot = topup_gateway_snapshot(&state, &order_id, &session.user_id).await?;
    Ok((
        no_store(),
        Json(TopupGatewayStatusView {
            gateway_status: snapshot.gateway.status,
            can_pay: snapshot.payability_reason == "payable",
            payability_reason: snapshot.payability_reason,
            can_cancel: snapshot.can_cancel,
            can_abandon: snapshot.can_abandon,
            selection_deadline: snapshot.gateway.selection_deadline,
            expires_at: snapshot.gateway.expires_at,
            server_now: now(),
        }),
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PaymentInstructionsView {
    gateway_status: String,
    can_pay: bool,
    payability_reason: &'static str,
    can_cancel: bool,
    chain: Option<String>,
    chain_name: Option<&'static str>,
    token_id: Option<String>,
    token_symbol: Option<&'static str>,
    min_confirmations: Option<i64>,
    confirmations: Option<i64>,
    /// Present only while the order can be paid.
    pay_address: Option<String>,
    payable_amount: Option<String>,
    fiat_amount_minor: i64,
    expires_at: Option<String>,
    server_now: String,
}

/// In-site payment instructions for a direct (QR) top-up. The address and the
/// exact on-chain amount come from a fresh authenticated lookup and are only
/// returned while the order is payable. Clients must poll at most every five
/// seconds, matching the gateway's lookup guidance.
async fn get_payment_instructions(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    {
        let conn = state.store.conn.lock().await;
        let mode: String = conn
            .query_row(
                "SELECT order_mode FROM market_topup_orders WHERE id = ?1 AND user_id = ?2",
                params![order_id, session.user_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error("read top-up mode"))?
            .ok_or_else(|| AppError::NotFound("top-up order not found".into()))?;
        if mode != "direct" {
            return Err(AppError::Conflict(
                "this top-up uses the hosted checkout".into(),
            ));
        }
    }
    if !state.market_payments.allow_instruction_lookup(&order_id) {
        return Err(AppError::RateLimited {
            message: "payment status was checked moments ago".into(),
            retry_after_secs: 5,
        });
    }
    let snapshot = topup_gateway_snapshot(&state, &order_id, &session.user_id).await?;
    let gateway = snapshot.gateway;
    let rail = gateway.token_id.as_deref().and_then(token_rail);
    let payable = snapshot.payability_reason == "payable";
    Ok((
        no_store(),
        Json(PaymentInstructionsView {
            gateway_status: gateway.status,
            can_pay: payable,
            payability_reason: snapshot.payability_reason,
            can_cancel: snapshot.can_cancel,
            chain: gateway.chain,
            chain_name: rail.map(|rail| rail.chain_name),
            token_id: gateway.token_id,
            token_symbol: rail.map(|rail| rail.symbol),
            min_confirmations: rail.map(|rail| rail.min_confirmations),
            confirmations: gateway.confirmations,
            pay_address: gateway.pay_address.filter(|_| payable),
            payable_amount: gateway.payable_amount.filter(|_| payable),
            fiat_amount_minor: gateway.fiat_amount_minor,
            expires_at: gateway.expires_at,
            server_now: now(),
        }),
    ))
}

async fn abandon_buyer_topup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let (snapshot, loaded) = {
        let conn = state.store.conn.lock().await;
        let snapshot = cancel_topup_snapshot_tx(&conn, &order_id, Some(&session.user_id))?;
        if topup_buyer_abandoned_tx(&conn, &order_id)? {
            return Ok((
                no_store(),
                Json(read_topup(&conn, &order_id, &session.user_id)?),
            ));
        }
        if snapshot.status != "pending" {
            return Err(AppError::Conflict(
                "top-up is not awaiting a payment method".into(),
            ));
        }
        let loaded = load_order_revision(
            &conn,
            &state.market_payments,
            &snapshot.channel_id,
            snapshot.revision,
        )?;
        (snapshot, loaded)
    };
    let gateway = get_bound_gateway_order(
        &state,
        &snapshot.channel_id,
        snapshot.revision,
        &loaded,
        &snapshot.gateway_order_id,
    )
    .await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin buyer top-up abandonment"))?;
    let view = mark_buyer_abandoned_tx(
        &tx,
        &order_id,
        &session.user_id,
        &snapshot,
        &loaded,
        &gateway,
    )?;
    tx.commit()
        .map_err(db_error("commit buyer top-up abandonment"))?;
    Ok((no_store(), Json(view)))
}

async fn stop_buyer_topup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin buyer stop-payment request"))?;
    let view = stop_buyer_topup_tx(&tx, &order_id, &session.user_id)?;
    tx.commit()
        .map_err(db_error("commit buyer stop-payment request"))?;
    Ok((no_store(), Json(view)))
}

fn stop_buyer_topup_tx(
    conn: &Connection,
    order_id: &str,
    buyer_id: &str,
) -> Result<TopupView, AppError> {
    let current = read_topup(conn, order_id, buyer_id)?;
    if current.buyer_abandoned_at.is_some() {
        return Ok(current);
    }
    if !matches!(
        current.status.as_str(),
        "created" | "creating" | "create_unknown" | "pending" | "verifying" | "review_required"
    ) {
        return Err(AppError::Conflict("top-up is already final".into()));
    }
    let time = now();
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
             SET buyer_abandoned_at = ?2,
                 updated_at = ?2, next_reconcile_at = ?2
             WHERE id = ?1 AND user_id = ?3 AND buyer_abandoned_at IS NULL
               AND status IN ('created', 'creating', 'create_unknown', 'pending',
                              'verifying', 'review_required')",
            params![order_id, time, buyer_id],
        )
        .map_err(db_error("record buyer stop-payment request"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "top-up changed; refresh before stopping payment".into(),
        ));
    }
    audit_tx(
        conn,
        buyer_id,
        "topup_buyer_stopped_payment",
        "topup",
        order_id,
        None,
        &time,
    )?;
    read_topup(conn, order_id, buyer_id)
}

fn mark_buyer_abandoned_tx(
    conn: &Connection,
    order_id: &str,
    buyer_id: &str,
    snapshot: &CancelTopupSnapshot,
    loaded: &LoadedRevision,
    gateway: &GatewayOrder,
) -> Result<TopupView, AppError> {
    let current = cancel_topup_snapshot_tx(conn, order_id, Some(buyer_id))?;
    if topup_buyer_abandoned_tx(conn, order_id)? {
        return read_topup(conn, order_id, buyer_id);
    }
    if current.status != "pending"
        || current.channel_id != snapshot.channel_id
        || current.revision != snapshot.revision
        || current.gateway_order_id != snapshot.gateway_order_id
    {
        return Err(AppError::Conflict(
            "top-up changed; refresh its gateway status".into(),
        ));
    }
    validate_gateway_identity_tx(
        conn,
        &order_id,
        &current.channel_id,
        current.revision,
        &loaded,
        &current.gateway_order_id,
        &gateway,
    )?;
    let assessment = recovery_assessment_tx(conn, order_id, &current.status, gateway)?;
    if gateway.status != "awaiting_payment"
        || assessment.blockers.as_slice() != ["gateway_not_terminal"]
    {
        return Err(AppError::Conflict(
            "payment or gateway state changed; refresh before abandoning".into(),
        ));
    }
    let time = now();
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
             SET buyer_abandoned_at = ?2, updated_at = ?2, next_reconcile_at = ?2
             WHERE id = ?1 AND user_id = ?3 AND status = 'pending'
               AND buyer_abandoned_at IS NULL AND gateway_order_id = ?4",
            params![order_id, time, buyer_id, current.gateway_order_id],
        )
        .map_err(db_error("mark buyer top-up abandoned"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "top-up changed; refresh before abandoning".into(),
        ));
    }
    audit_tx(
        conn,
        buyer_id,
        "topup_buyer_abandoned",
        "topup",
        &order_id,
        None,
        &time,
    )?;
    read_topup(conn, order_id, buyer_id)
}

fn finish_gateway_cancel_tx(
    conn: &Connection,
    order_id: &str,
    buyer_id: Option<&str>,
    loaded: &LoadedRevision,
    gateway: &GatewayOrder,
    actor: &str,
    time: &str,
) -> Result<TopupView, AppError> {
    let snapshot = cancel_topup_snapshot_tx(conn, order_id, buyer_id)?;
    validate_gateway_identity_tx(
        conn,
        order_id,
        &snapshot.channel_id,
        snapshot.revision,
        loaded,
        &snapshot.gateway_order_id,
        gateway,
    )?;
    if !matches!(
        gateway.status.as_str(),
        "canceled" | "cancelled" | "expired"
    ) {
        return Err(AppError::Conflict(
            "DujiaoPay cancellation is not confirmed; order remains payable".into(),
        ));
    }
    let assessment = recovery_assessment_tx(conn, order_id, &snapshot.status, gateway)?;
    if !assessment.can_recover() {
        return Err(AppError::Conflict(
            "payment or review evidence prevents closing the top-up".into(),
        ));
    }
    match snapshot.status.as_str() {
        "pending" => {
            let status = verified_unpaid_status(&gateway.status)?;
            apply_verified_unpaid_status_tx(conn, order_id, status, time)?;
            finish_reconciliation_tx(conn, order_id, false)?;
            audit_tx(
                conn,
                actor,
                "topup_gateway_unpaid_terminal_verified",
                "topup",
                order_id,
                Some(status),
                time,
            )?;
        }
        "review_required" if buyer_id.is_none() => {
            if benign_progress_review_case_tx(conn, order_id)?.is_some() {
                // The regular reconciler already releases an old progress-only
                // hold on this same verified unpaid terminal evidence. Resolve
                // it now so owner cancellation has the same visible outcome.
                let status = verified_unpaid_status(&gateway.status)?;
                apply_verified_unpaid_status_tx(conn, order_id, status, time)?;
                finish_reconciliation_tx(conn, order_id, false)?;
                audit_tx(
                    conn,
                    actor,
                    "topup_gateway_unpaid_terminal_verified",
                    "topup",
                    order_id,
                    Some(status),
                    time,
                )?;
            } else {
                // An ambiguous historical hold still needs its separate
                // merchant-reference recovery decision after gateway cancel.
                audit_tx(
                    conn,
                    actor,
                    "topup_gateway_terminal_review_held",
                    "topup",
                    order_id,
                    None,
                    time,
                )?;
            }
        }
        "cancelled" | "expired" => {}
        _ => {
            return Err(AppError::Conflict(
                "top-up changed during gateway cancellation".into(),
            ));
        }
    }
    finish_buyer_recheck_from_order_tx(conn, order_id)?;
    read_topup(conn, order_id, &snapshot.user_id)
}

async fn cancel_topup_for_actor(
    state: &ServerState,
    order_id: &str,
    actor: &str,
    buyer_id: Option<&str>,
) -> Result<TopupView, AppError> {
    let (snapshot, loaded) = {
        let conn = state.store.conn.lock().await;
        let snapshot = cancel_topup_snapshot_tx(&conn, order_id, buyer_id)?;
        if matches!(snapshot.status.as_str(), "cancelled" | "expired") {
            return read_topup(&conn, order_id, &snapshot.user_id);
        }
        if snapshot.status != "pending"
            && !(buyer_id.is_none() && snapshot.status == "review_required")
        {
            return Err(AppError::Conflict(
                "top-up is not eligible for gateway cancellation".into(),
            ));
        }
        let loaded = load_order_revision(
            &conn,
            &state.market_payments,
            &snapshot.channel_id,
            snapshot.revision,
        )?;
        (snapshot, loaded)
    };
    let (gateway, credential_source) = get_bound_gateway_order_with_source(
        state,
        &snapshot.channel_id,
        snapshot.revision,
        &loaded,
        &snapshot.gateway_order_id,
    )
    .await?;
    {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin gateway cancellation preflight"))?;
        if matches!(
            gateway.status.as_str(),
            "canceled" | "cancelled" | "expired"
        ) {
            // A previous cancel may have succeeded even when its response or
            // follow-up lookup was lost. Confirm the terminal state without
            // issuing a second, potentially non-idempotent gateway POST.
            let view = finish_gateway_cancel_tx(
                &tx,
                order_id,
                buyer_id,
                &loaded,
                &gateway,
                actor,
                &now(),
            )?;
            tx.commit()
                .map_err(db_error("commit verified gateway terminal"))?;
            return Ok(view);
        }
        let current = cancel_topup_snapshot_tx(&tx, order_id, buyer_id)?;
        validate_gateway_identity_tx(
            &tx,
            order_id,
            &current.channel_id,
            current.revision,
            &loaded,
            &current.gateway_order_id,
            &gateway,
        )?;
        let assessment = recovery_assessment_tx(&tx, order_id, &current.status, &gateway)?;
        if !can_cancel_gateway_topup(
            &current.status,
            &gateway.status,
            &assessment,
            buyer_id.is_none(),
        ) {
            return Err(AppError::Conflict(
                if gateway.status == "awaiting_payment" {
                    "Choose a payment method at DujiaoPay before cancellation, or wait for expiry"
                } else {
                    "DujiaoPay order is not safely cancelable in its current state"
                }
                .into(),
            ));
        }
        if gateway_cancel_was_requested_tx(&tx, order_id)? {
            // A prior POST may have reached the gateway even if its response
            // was lost. Poll for its terminal state; never repeat it.
            return Err(AppError::Conflict(
                "gateway cancellation was already requested; verify its current status".into(),
            ));
        }
        mark_gateway_cancel_requested_tx(&tx, actor, order_id, &now())?;
        // Persist a near-term authenticated lookup before the external call.
        // If this process dies after the gateway commits the cancellation,
        // reconciliation can still observe its terminal state.
        queue_webhook_hint_tx(&tx, order_id, &now())?;
        tx.commit()
            .map_err(db_error("commit gateway cancellation preflight"))?;
    }
    let original_credentials = if matches!(credential_source, GatewayCredentialSource::Original) {
        let conn = state.store.conn.lock().await;
        let original = load_revision(
            &conn,
            &state.market_payments,
            &snapshot.channel_id,
            snapshot.revision,
        )?;
        if original.merchant_fingerprint != loaded.merchant_fingerprint {
            return Err(AppError::Conflict(
                "DujiaoPay merchant identity changed before cancellation".into(),
            ));
        }
        Some(original.credentials)
    } else {
        None
    };
    let cancel_credentials = original_credentials.as_ref().unwrap_or(&loaded.credentials);
    let cancel_result = state
        .market_payments
        .client
        .cancel_order(cancel_credentials, &snapshot.gateway_order_id)
        .await;
    // A lost POST response or 409 is not proof of failure or success. Do not
    // retry a non-idempotent cancel; authenticate the gateway state instead.
    let gateway = get_bound_gateway_order(
        state,
        &snapshot.channel_id,
        snapshot.revision,
        &loaded,
        &snapshot.gateway_order_id,
    )
    .await;
    let gateway = match gateway {
        Ok(gateway) => gateway,
        Err(error) => {
            let conn = state.store.conn.lock().await;
            queue_webhook_hint_tx(&conn, order_id, &now())?;
            return Err(error);
        }
    };
    if matches!(
        gateway.status.as_str(),
        "canceled" | "cancelled" | "expired"
    ) {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin verified gateway cancellation"))?;
        let view =
            finish_gateway_cancel_tx(&tx, order_id, buyer_id, &loaded, &gateway, actor, &now())?;
        tx.commit()
            .map_err(db_error("commit verified gateway cancellation"))?;
        return Ok(view);
    }
    {
        let conn = state.store.conn.lock().await;
        queue_webhook_hint_tx(&conn, order_id, &now())?;
    }
    if cancel_result.is_err() {
        return Err(AppError::Conflict(
            "DujiaoPay did not confirm cancellation; check the current payment status".into(),
        ));
    }
    Err(AppError::Conflict(
        "DujiaoPay cancellation is not confirmed; order remains payable".into(),
    ))
}

async fn cancel_buyer_topup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = require_session(&state, &headers).await?;
    let view =
        cancel_topup_for_actor(&state, &order_id, &actor.user_id, Some(&actor.user_id)).await?;
    Ok((no_store(), Json(view)))
}

async fn cancel_admin_topup(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_admin_session(&state, &headers).await?;
    let view = cancel_topup_for_actor(&state, &order_id, &actor.user_id, None).await?;
    Ok((no_store(), Json(view)))
}

fn topups_tx(
    conn: &Connection,
    user_id: &str,
    cursor: Option<&str>,
) -> Result<Page<TopupView>, AppError> {
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT created_at FROM market_topup_orders WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read top-up cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid top-up cursor".into()))
        })
        .transpose()?;
    let mut statement = conn
        .prepare(&format!(
            "{TOPUP_VIEW_SQL} WHERE user_id = ?1
               AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id < ?3))
             ORDER BY created_at DESC, id DESC LIMIT 51"
        ))
        .map_err(db_error("prepare top-up history"))?;
    let mut items = statement
        .query_map(params![user_id, cursor_at, cursor], topup_view_row)
        .map_err(db_error("query top-up history"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read top-up history"))?;
    let has_more = items.len() > 50;
    items.truncate(50);
    let next_cursor = has_more.then(|| items.last().unwrap().id.clone());
    Ok(Page { items, next_cursor })
}

async fn list_topups(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(topups_tx(&conn, &session.user_id, query.cursor.as_deref())?),
    ))
}

fn active_matching_topup_tx(
    conn: &Connection,
    user_id: &str,
    request_fingerprint: &str,
) -> Result<Option<(String, String, String, i64)>, AppError> {
    conn.query_row(
        "SELECT id, request_fingerprint, merchant_order_id, channel_revision
         FROM market_topup_orders
         WHERE user_id = ?1 AND request_fingerprint = ?2
           AND buyer_abandoned_at IS NULL
           AND status IN ('created', 'creating', 'create_unknown', 'pending')
           AND cancel_requested_at IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM market_finance_reconciliation_cases review
               WHERE review.object_kind = 'topup'
                 AND review.object_id = market_topup_orders.id
                 AND review.status = 'open'
           )
         ORDER BY created_at DESC, id DESC LIMIT 1",
        params![user_id, request_fingerprint],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .optional()
    .map_err(db_error("read active matching top-up"))
}

fn ensure_pending_topup_capacity_tx(conn: &Connection, user_id: &str) -> Result<(), AppError> {
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market_topup_orders WHERE user_id = ?1
         AND buyer_abandoned_at IS NULL
         AND status IN ('created', 'creating', 'create_unknown', 'pending', 'verifying')",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(db_error("count pending top-ups"))?;
    if pending >= MAX_PENDING_ORDERS {
        return Err(AppError::Coded {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "MARKET_TOPUP_PENDING_LIMIT",
            message: "too many pending top-ups".into(),
            details: serde_json::json!({ "pendingCount": pending, "limit": MAX_PENDING_ORDERS }),
        });
    }
    let abandoned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market_topup_orders WHERE user_id = ?1
             AND buyer_abandoned_at IS NOT NULL
             AND status IN ('created', 'creating', 'create_unknown', 'pending', 'verifying')
             AND created_at >= ?2",
            params![
                user_id,
                (Utc::now() - chrono::Duration::hours(24)).to_rfc3339()
            ],
            |row| row.get(0),
        )
        .map_err(db_error("count abandoned but payable top-ups"))?;
    if abandoned >= MAX_ABANDONED_ORDERS {
        return Err(AppError::Coded {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "MARKET_TOPUP_ABANDONED_LIMIT",
            message: "too many abandoned top-ups remain unresolved".into(),
            details: serde_json::json!({ "abandonedCount": abandoned, "limit": MAX_ABANDONED_ORDERS }),
        });
    }
    let review: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market_topup_orders
             WHERE user_id = ?1 AND status = 'review_required' AND created_at >= ?2",
            params![
                user_id,
                (Utc::now() - chrono::Duration::hours(24)).to_rfc3339()
            ],
            |row| row.get(0),
        )
        .map_err(db_error("count review top-ups"))?;
    if review >= MAX_REVIEW_ORDERS {
        return Err(AppError::Coded {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "MARKET_TOPUP_REVIEW_LIMIT",
            message: "too many top-ups require review".into(),
            details: serde_json::json!({ "reviewCount": review, "limit": MAX_REVIEW_ORDERS }),
        });
    }
    Ok(())
}

struct TopupChannelSettings {
    revision: i64,
    order_mode: String,
    allowed_methods: Vec<String>,
    min_topup_minor: i64,
    max_topup_minor: i64,
    settings_version: i64,
    cost_rate_bps: i64,
    fixed_cost_minor: i64,
}

/// The channel and its settings for a *new* order. A canary accepts only the
/// Router owner; `off`, archived or re-verification-pending channels accept
/// nobody.
fn topup_channel_tx(
    conn: &Connection,
    channel_id: &str,
    owner: bool,
) -> Result<TopupChannelSettings, AppError> {
    let row = conn
        .query_row(
            "SELECT c.current_revision, COALESCE(s.order_mode, 'cashier'),
                    COALESCE(s.allowed_methods_json, '[]'), s.min_topup_minor, s.max_topup_minor,
                    COALESCE(s.version, 0), COALESCE(s.fee_rate_bps, 0), COALESCE(s.fixed_fee_minor, 0)
             FROM market_payment_channels c
             LEFT JOIN market_payment_channel_settings s ON s.channel_id = c.id
             WHERE c.id = ?1 AND c.archived_at IS NULL AND c.api_reverify_required = 0
               AND (c.rollout_state = 'live' OR (c.rollout_state = 'canary' AND ?2 = 1))",
            params![channel_id, owner],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("read enabled payment channel"))?
        .ok_or_else(|| AppError::BadRequest("payment channel unavailable".into()))?;
    let settings = ChannelSettingsView {
        order_mode: row.1.clone(),
        allowed_methods: serde_json::from_str(&row.2)
            .map_err(|_| AppError::Internal("stored payment methods invalid".into()))?,
        min_topup_minor: row.3,
        max_topup_minor: row.4,
        fee_rate_bps: 0,
        fixed_fee_minor: 0,
        version: 0,
    };
    let (min_topup_minor, max_topup_minor) = effective_topup_bounds(&settings);
    Ok(TopupChannelSettings {
        revision: row.0,
        order_mode: settings.order_mode,
        allowed_methods: settings.allowed_methods,
        min_topup_minor,
        max_topup_minor,
        settings_version: row.5,
        cost_rate_bps: row.6,
        fixed_cost_minor: row.7,
    })
}

fn require_new_topups_open_tx(conn: &Connection) -> Result<(), AppError> {
    let paused: bool = conn
        .query_row(
            "SELECT new_topups_paused FROM market_finance_control WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(db_error("read top-up pause"))?;
    if paused {
        return Err(AppError::Coded {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "MARKET_TOPUP_PAUSED",
            message: "new top-ups are paused by the Router owner".into(),
            details: serde_json::json!({}),
        });
    }
    Ok(())
}

fn topup_base_url(state: &ServerState) -> String {
    let scheme = if state.config.use_localhost {
        "http"
    } else {
        "https"
    };
    format!(
        "{scheme}://{}",
        state.config.tunnel_domain.trim_end_matches('/')
    )
}

fn user_reference(user_id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("market-topup-user:{user_id}").as_bytes(),
    ))[..16]
        .into()
}

/// The create body for an order that predates frozen request bodies. It is
/// rebuilt exactly as the original code did (no metadata, revision methods).
fn legacy_create_body(
    state: &ServerState,
    order_id: &str,
    merchant_order_id: &str,
    amount_minor: i64,
    allowed_methods: &[String],
) -> Result<Vec<u8>, AppError> {
    let (success_url, cancel_url) = topup_return_urls(&topup_base_url(state), order_id);
    dujiaopay::create_order_body(&dujiaopay::CreateOrderInput {
        merchant_order_id,
        amount_minor,
        allowed_methods,
        fixed_token_id: None,
        success_url: &success_url,
        cancel_url: &cancel_url,
        metadata: None,
    })
}

/// Persist the outcome of a failed create attempt. Throttling and capacity
/// errors push the next attempt out; an authentication failure takes the
/// channel offline because no order on it can be created or verified.
fn record_create_failure_tx(
    conn: &Connection,
    order_id: &str,
    channel_id: &str,
    error: &AppError,
    time: &str,
) -> Result<(), AppError> {
    let kind = dujiaopay::error_kind(error);
    let code = dujiaopay::gateway_error_code(error)
        .or_else(|| error.code().map(str::to_string))
        .unwrap_or_else(|| "unavailable".into());
    let backoff_secs = match kind {
        Some(dujiaopay::GatewayErrorKind::RateLimited) => 90,
        Some(dujiaopay::GatewayErrorKind::Capacity) => 120,
        _ => 0,
    };
    let next = (Utc::now() + chrono::Duration::seconds(backoff_secs.max(30))).to_rfc3339();
    conn.execute(
        "UPDATE market_topup_orders
         SET create_attempts = create_attempts + 1, create_last_error_code = ?2,
             status = CASE WHEN status IN ('created', 'creating') THEN 'create_unknown' ELSE status END,
             updated_at = ?3,
             next_reconcile_at = CASE WHEN ?5 > 0 THEN ?4 ELSE next_reconcile_at END
         WHERE id = ?1",
        params![order_id, code, time, next, backoff_secs],
    )
    .map_err(db_error("record top-up create failure"))?;
    if kind == Some(dujiaopay::GatewayErrorKind::Auth) {
        disable_channel_for_auth_failure_tx(conn, channel_id, time)?;
    }
    Ok(())
}

fn disable_channel_for_auth_failure_tx(
    conn: &Connection,
    channel_id: &str,
    time: &str,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE market_payment_channels SET rollout_state = 'off', updated_at = ?2,
                    version = version + 1
             WHERE id = ?1 AND rollout_state != 'off'",
            params![channel_id, time],
        )
        .map_err(db_error("disable rejected payment channel"))?;
    if changed == 1 {
        open_reconciliation_case_tx(
            conn,
            "payment_channel_auth_failed",
            "payment_channel",
            channel_id,
            time,
        )?;
        audit_tx(
            conn,
            "system",
            "channel_disabled_auth_failed",
            "payment_channel",
            channel_id,
            Some("gateway_auth_rejected"),
            time,
        )?;
        enqueue_owner_signal_tx(
            conn,
            "payment_channel_auth_failed",
            "payment_channel",
            channel_id,
            serde_json::json!({ "channelId": channel_id }),
            time,
        )?;
    }
    Ok(())
}

fn enqueue_owner_signal_tx(
    conn: &Connection,
    kind: &str,
    entity_kind: &str,
    entity_id: &str,
    details: serde_json::Value,
    time: &str,
) -> Result<(), AppError> {
    crate::market_notifications::owner_signal_tx(
        conn,
        kind,
        entity_kind,
        entity_id,
        details,
        time,
        false,
    )
}

async fn create_topup(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<CreateTopupRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let client_ip = crate::client_meta::extract_client_metadata(&headers, peer)
        .ip
        .and_then(|value| value.parse::<IpAddr>().ok())
        .unwrap_or_else(|| peer.ip());
    let owner = crate::api::is_finance_owner(&state.config, &session.email);
    let idem = headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            value.len() >= 8
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or_else(|| AppError::BadRequest("valid Idempotency-Key required".into()))?;
    if !(MIN_TOPUP_MINOR..=MAX_TOPUP_MINOR).contains(&input.amount_minor) {
        return Err(AppError::BadRequest(
            "top-up amount outside supported range".into(),
        ));
    }
    if input
        .token_id
        .as_deref()
        .is_some_and(|token| token_rail(token).is_none())
    {
        return Err(AppError::BadRequest("unsupported payment method".into()));
    }
    let (order_id, merchant_order_id, revision, channel_id, loaded, existing_view, body) = {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin top-up"))?;
        let quote = quotes::read_quote_tx(&tx, &session.user_id, &input.quote_id)?;
        if quote.channel_id != input.channel_id
            || quote.credit_minor != input.amount_minor
            || quote.token_id != input.token_id
        {
            return Err(quotes::stale_quote());
        }
        // Equivalent quotes may reuse an active order, but a different fee or
        // channel revision must never reuse an old payment's price.
        let request_fingerprint = format!(
            "{}:{}:{}:{}:{}:{}:{}",
            input.channel_id,
            quote.credit_minor,
            quote.fee_minor,
            quote.policy_version,
            quote.channel_revision,
            quote.channel_settings_version,
            input.token_id.as_deref().unwrap_or("hosted")
        );
        let (mode, cutover_approved): (String, bool) = tx
            .query_row(
                "SELECT mode, cutover_approved_at IS NOT NULL
                 FROM market_finance_control WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(db_error("read market finance mode"))?;
        let prior = tx
            .query_row(
                "SELECT id, request_fingerprint, merchant_order_id, channel_revision
             FROM market_topup_orders WHERE user_id = ?1 AND (idempotency_key = ?2 OR quote_id = ?3)
             ORDER BY CASE WHEN idempotency_key = ?2 THEN 0 ELSE 1 END LIMIT 1",
                params![session.user_id, idem, quote.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(db_error("read prior top-up"))?;
        // A browser reload or another tab has a new idempotency key. Reuse
        // the same active payment instead of filling the user's pending quota
        // with duplicate gateway orders for the same channel and amount.
        let prior = if prior.is_some() {
            prior
        } else {
            active_matching_topup_tx(&tx, &session.user_id, &request_fingerprint)?
        };
        let (id, merchant_order_id, revision, already_created, body) =
            if let Some((id, fingerprint, merchant_order_id, revision)) = prior {
                if fingerprint != request_fingerprint {
                    return Err(AppError::Conflict("top-up idempotency conflict".into()));
                }
                let body: Option<String> = tx
                    .query_row(
                        "SELECT create_request_body FROM market_topup_orders WHERE id = ?1",
                        params![id],
                        |row| row.get(0),
                    )
                    .map_err(db_error("read frozen top-up request"))?;
                (
                    id,
                    merchant_order_id,
                    revision,
                    true,
                    body.map(String::into_bytes),
                )
            } else {
                if mode == "enabled" && (!MARKET_WALLET_CUTOVER_IMPLEMENTED || !cutover_approved) {
                    return Err(AppError::ServiceUnavailable(
                        "platform wallet cutover has not been approved".into(),
                    ));
                }
                if !user_topup_allowed(&mode, owner, cutover_approved) {
                    return Err(AppError::ServiceUnavailable(
                        "platform wallet top-up is not enabled".into(),
                    ));
                }
                crate::market_reconcile::ensure_finance_scopes_safe_tx(
                    &tx,
                    &[crate::market_reconcile::FinanceScope::User(
                        &session.user_id,
                    )],
                )?;
                require_new_topups_open_tx(&tx)?;
                let channel = topup_channel_tx(&tx, &input.channel_id, owner)?;
                quotes::validate_current_tx(&tx, &quote, &channel, &now())?;
                quotes::validate_method(&channel, input.token_id.as_deref())?;
                ensure_pending_topup_capacity_tx(&tx, &session.user_id)?;
                let recent: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM market_topup_orders
                     WHERE user_id = ?1 AND created_at >= ?2",
                        params![
                            session.user_id,
                            (Utc::now() - chrono::Duration::hours(1)).to_rfc3339()
                        ],
                        |row| row.get(0),
                    )
                    .map_err(db_error("count recent top-ups"))?;
                if recent >= MAX_TOPUPS_PER_HOUR {
                    return Err(AppError::TooManyRequests(
                        "top-up rate limit reached".into(),
                    ));
                }
                if !state.market_payments.allow_topup_ip(client_ip) {
                    return Err(AppError::TooManyRequests(
                        "top-up IP rate limit reached".into(),
                    ));
                }
                let id = Uuid::new_v4().to_string();
                let merchant_order_id = format!("router-{}", Uuid::new_v4().simple());
                let (success_url, cancel_url) = topup_return_urls(&topup_base_url(&state), &id);
                let body = dujiaopay::create_order_body(&dujiaopay::CreateOrderInput {
                    merchant_order_id: &merchant_order_id,
                    amount_minor: quote.pay_minor,
                    allowed_methods: &channel.allowed_methods,
                    fixed_token_id: input.token_id.as_deref(),
                    success_url: &success_url,
                    cancel_url: &cancel_url,
                    metadata: Some(serde_json::json!({
                        "router_order_id": id,
                        "user_ref": user_reference(&session.user_id),
                    })),
                })?;
                let loaded = load_revision(
                    &tx,
                    &state.market_payments,
                    &input.channel_id,
                    channel.revision,
                )?;
                let created_at = Utc::now();
                let time = created_at.to_rfc3339();
                let fixed_rail = input.token_id.as_deref().and_then(token_rail);
                tx.execute(
                    "INSERT INTO market_topup_orders (
                    id, user_id, channel_id, channel_revision, merchant_fingerprint,
                    merchant_order_id, idempotency_key, request_fingerprint, amount_minor,
                    status, created_at, updated_at, next_reconcile_at, auto_close_at,
                    create_request_body, order_mode, gateway_token_id, gateway_chain,
                    credit_amount_minor, platform_fee_minor, platform_fee_rate_bps,
                    wallet_policy_version, gateway_cost_minor, quote_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'creating', ?10, ?10, ?11, ?12,
                           ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
                    params![
                        id,
                        session.user_id,
                        input.channel_id,
                        channel.revision,
                        loaded.merchant_fingerprint,
                        merchant_order_id,
                        idem,
                        request_fingerprint,
                        quote.pay_minor,
                        time,
                        (created_at + chrono::Duration::seconds(30)).to_rfc3339(),
                        topup_auto_close_deadline(created_at),
                        String::from_utf8(body.clone()).map_err(|_| AppError::Internal(
                            "encode top-up request failed".into()
                        ))?,
                        channel.order_mode,
                        fixed_rail.map(|rail| rail.token_id),
                        fixed_rail.map(|rail| rail.chain),
                        quote.credit_minor,
                        quote.fee_minor,
                        quote.fee_bps,
                        quote.policy_version,
                        quote.gateway_cost_minor,
                        quote.id
                    ],
                )
                .map_err(db_error("insert top-up order"))?;
                (id, merchant_order_id, channel.revision, false, Some(body))
            };
        let loaded = load_revision(&tx, &state.market_payments, &input.channel_id, revision)?;
        let existing_view = already_created
            .then(|| read_topup(&tx, &id, &session.user_id))
            .transpose()?;
        if let Some(view) = &existing_view
            && matches!(
                view.status.as_str(),
                "created" | "creating" | "create_unknown"
            )
        {
            // A replay may only re-submit the frozen body of an order that is
            // still eligible for a fresh gateway attempt.
            let stopped: bool = tx
                .query_row(
                    "SELECT buyer_abandoned_at IS NOT NULL OR cancel_requested_at IS NOT NULL
                     FROM market_topup_orders WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .map_err(db_error("read top-up stop marker"))?;
            if stopped {
                tx.commit().map_err(db_error("commit top-up replay"))?;
                return Ok((no_store(), Json(view.clone())));
            }
        }
        tx.commit().map_err(db_error("commit top-up order"))?;
        (
            id,
            merchant_order_id,
            revision,
            input.channel_id.clone(),
            loaded,
            existing_view,
            body,
        )
    };
    if let Some(view) = existing_view
        && !matches!(
            view.status.as_str(),
            "created" | "creating" | "create_unknown"
        )
    {
        return Ok((no_store(), Json(view)));
    }
    let body = match body {
        Some(body) => body,
        None => legacy_create_body(
            &state,
            &order_id,
            &merchant_order_id,
            {
                let conn = state.store.conn.lock().await;
                conn.query_row(
                    "SELECT amount_minor FROM market_topup_orders WHERE id = ?1",
                    params![order_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(db_error("read legacy top-up amount"))?
            },
            &loaded.allowed_methods,
        )?,
    };
    let created = state
        .market_payments
        .client
        .create_order_with_body(&loaded.credentials, &merchant_order_id, &body)
        .await;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin top-up create result"))?;
    let view = match created {
        Ok(created) => {
            tx.execute(
                "UPDATE market_topup_orders SET create_attempts = create_attempts + 1 WHERE id = ?1",
                params![order_id],
            )
            .map_err(db_error("count top-up create attempt"))?;
            if let Err(error) = save_created_order_tx(&tx, &order_id, revision, &created, &now()) {
                if matches!(error, AppError::Conflict(_)) {
                    hold_topup_verification_failure_tx(
                        &tx,
                        &order_id,
                        "created_order_conflict",
                        &now(),
                    )?;
                    tx.commit()
                        .map_err(db_error("commit conflicting DujiaoPay create result"))?;
                }
                return Err(error);
            }
            read_topup(&tx, &order_id, &session.user_id)?
        }
        Err(error) => {
            record_create_failure_tx(&tx, &order_id, &channel_id, &error, &now())?;
            tx.commit().map_err(db_error("commit uncertain top-up"))?;
            return Err(error);
        }
    };
    tx.commit().map_err(db_error("commit DujiaoPay order"))?;
    if matches!(
        view.status.as_str(),
        "created" | "creating" | "create_unknown" | "pending"
    ) && view.checkout_url.is_none()
        && !view.has_payment_instructions
    {
        return Err(AppError::Coded {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "MARKET_TOPUP_CHECKOUT_UNAVAILABLE",
            message: "DujiaoPay checkout URL unavailable; existing order is being reconciled"
                .into(),
            details: serde_json::json!({ "orderId": order_id }),
        });
    }
    Ok((no_store(), Json(view)))
}

fn receipt_fingerprint(merchant: &str, order: &GatewayOrder) -> Result<String, AppError> {
    let chain = order.chain.as_deref().filter(|value| !value.is_empty());
    let token = order.token_id.as_deref().filter(|value| !value.is_empty());
    let tx_id = order.tx_id.as_deref().filter(|value| !value.is_empty());
    let settled = order
        .settled_amount
        .as_deref()
        .filter(|value| valid_positive_decimal(value));
    if order.status != "paid"
        || order.paid_source.as_deref() != Some("chain")
        || order.confirmations.unwrap_or(0) <= 0
        || chain.is_none()
        || token.is_none()
        || tx_id.is_none()
        || settled.is_none()
    {
        return Err(AppError::Conflict("DujiaoPay payment needs review".into()));
    }
    let source = format!(
        "{merchant}:{}:{}:{}",
        chain.unwrap(),
        token.unwrap(),
        tx_id.unwrap()
    );
    Ok(hex::encode(Sha256::digest(source.as_bytes())))
}

fn valid_positive_decimal(value: &str) -> bool {
    let mut pieces = value.split('.');
    let whole = pieces.next().unwrap_or_default();
    let fractional = pieces.next();
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fractional.is_none_or(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
        && pieces.next().is_none()
        && value.bytes().any(|byte| (b'1'..=b'9').contains(&byte))
}

const PAID_TERMS_CODE: &str = "MARKET_TOPUP_PAID_TERMS";

fn paid_terms_error(detail: &'static str) -> AppError {
    AppError::Coded {
        status: StatusCode::CONFLICT,
        code: PAID_TERMS_CODE,
        message: "DujiaoPay payment terms need review".into(),
        details: serde_json::json!({ "detail": detail }),
    }
}

/// The review detail for a paid-order validation failure, or `None` when the
/// error is not a payment-evidence conflict.
fn paid_review_detail(error: &AppError) -> Option<&'static str> {
    match error {
        AppError::Conflict(_) => Some("paid_receipt_invalid"),
        AppError::Coded { code, details, .. } if *code == PAID_TERMS_CODE => {
            match details.get("detail").and_then(serde_json::Value::as_str) {
                Some("paid_amount_short") => Some("paid_amount_short"),
                Some("paid_method_not_allowed") => Some("paid_method_not_allowed"),
                Some("paid_confirmations_low") => Some("paid_confirmations_low"),
                Some("paid_source_manual") => Some("paid_source_manual"),
                _ => Some("paid_receipt_invalid"),
            }
        }
        _ => None,
    }
}

struct OrderPaymentTerms {
    allowed_methods: Vec<String>,
    fixed_token_id: Option<String>,
    payable_amount: Option<String>,
}

/// The payment methods this order was created with: from its frozen create
/// body, or for a legacy order from its bound credential revision.
fn order_payment_terms_tx(
    conn: &Connection,
    order_id: &str,
    loaded: &LoadedRevision,
) -> Result<OrderPaymentTerms, AppError> {
    let (body, payable_amount): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT create_request_body, payable_amount FROM market_topup_orders WHERE id = ?1",
            params![order_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read top-up payment terms"))?;
    let Some(body) = body else {
        return Ok(OrderPaymentTerms {
            allowed_methods: loaded.allowed_methods.clone(),
            fixed_token_id: None,
            payable_amount,
        });
    };
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| AppError::Internal("stored top-up request invalid".into()))?;
    let allowed_methods = value
        .get("allowed_methods")
        .and_then(serde_json::Value::as_array)
        .map(|methods| {
            methods
                .iter()
                .filter_map(|method| method.get("token_id").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(OrderPaymentTerms {
        allowed_methods,
        fixed_token_id: value
            .get("token_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        payable_amount,
    })
}

/// Payment-term checks beyond the gateway's own `paid` verdict: the source
/// must be an on-chain receipt, the rail must be one this order allowed, the
/// confirmations must reach the rail's documented minimum, and the settled
/// amount may not fall short of the locked payable amount.
fn validate_paid_terms_tx(
    conn: &Connection,
    order_id: &str,
    loaded: &LoadedRevision,
    gateway: &GatewayOrder,
) -> Result<(), AppError> {
    match gateway.paid_source.as_deref() {
        Some("chain") => {}
        Some("merchant_claim" | "platform_manual" | "merchant_manual") => {
            return Err(paid_terms_error("paid_source_manual"));
        }
        _ => return Err(AppError::Conflict("DujiaoPay payment needs review".into())),
    }
    let terms = order_payment_terms_tx(conn, order_id, loaded)?;
    let token = gateway.token_id.as_deref().unwrap_or_default();
    let allowed = match terms.fixed_token_id.as_deref() {
        Some(fixed) => fixed == token,
        None => {
            terms.allowed_methods.is_empty() || terms.allowed_methods.iter().any(|m| m == token)
        }
    };
    // A token outside the rail table has no known chain or confirmation
    // floor, so nothing about its receipt can be checked: hold it for review.
    let Some(rail) = token_rail(token) else {
        return Err(paid_terms_error("paid_method_not_allowed"));
    };
    if !allowed || gateway.chain.as_deref() != Some(rail.chain) {
        return Err(paid_terms_error("paid_method_not_allowed"));
    }
    if gateway.confirmations.unwrap_or(0) < rail.min_confirmations {
        return Err(paid_terms_error("paid_confirmations_low"));
    }
    let payable = gateway
        .payable_amount
        .as_deref()
        .or(terms.payable_amount.as_deref());
    if let (Some(payable), Some(settled)) = (payable, gateway.settled_amount.as_deref()) {
        let fiat = format!(
            "{}.{:02}",
            gateway.fiat_amount_minor / 100,
            gateway.fiat_amount_minor % 100
        );
        // `settled_amount` is compared with the locked on-chain amount. On a
        // USD stablecoin a value exactly equal to the fiat amount is the
        // gateway reporting in fiat terms and is accepted. On any other token
        // the fiat figure is a different unit, so it never excuses a short
        // payment.
        let short = dujiaopay::compare_decimal(settled, payable)
            .is_none_or(|ordering| ordering == std::cmp::Ordering::Less);
        let fiat_terms = rail.usd_stable()
            && dujiaopay::compare_decimal(settled, &fiat) == Some(std::cmp::Ordering::Equal);
        if short && !fiat_terms {
            return Err(paid_terms_error("paid_amount_short"));
        }
    }
    Ok(())
}

struct ValidatedTopup {
    user_id: String,
    amount_minor: i64,
    receipt_fingerprint: String,
}

fn validate_gateway_identity_tx(
    conn: &Connection,
    order_id: &str,
    channel_id: &str,
    revision: i64,
    loaded: &LoadedRevision,
    expected_gateway_id: &str,
    gateway: &GatewayOrder,
) -> Result<(), AppError> {
    let (merchant_order_id, stored_gateway_id, amount_minor, merchant_fingerprint): (
        String,
        Option<String>,
        i64,
        String,
    ) = conn
        .query_row(
            "SELECT merchant_order_id, gateway_order_id, amount_minor, merchant_fingerprint
             FROM market_topup_orders
             WHERE id = ?1 AND channel_id = ?2 AND channel_revision = ?3",
            params![order_id, channel_id, revision],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(db_error("read top-up gateway identity"))?;
    if merchant_order_id != gateway.merchant_order_id
        || expected_gateway_id != gateway.order_id
        || stored_gateway_id
            .as_deref()
            .is_some_and(|id| id != gateway.order_id)
        || gateway.fiat_currency != "USD"
        || gateway.fiat_amount_minor != amount_minor
        || merchant_fingerprint != loaded.merchant_fingerprint
    {
        return Err(AppError::Conflict(
            "DujiaoPay order identity mismatch".into(),
        ));
    }
    let other_order: Option<String> = conn
        .query_row(
            "SELECT id FROM market_topup_orders
         WHERE merchant_fingerprint = ?1 AND gateway_order_id = ?2 AND id != ?3",
            params![merchant_fingerprint, gateway.order_id, order_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("check duplicate gateway order binding"))?;
    if other_order.is_some() {
        return Err(AppError::Conflict(
            "DujiaoPay order is bound to another top-up".into(),
        ));
    }
    Ok(())
}

fn restore_verified_checkout_tx(
    conn: &Connection,
    order_id: &str,
    gateway: &GatewayOrder,
) -> Result<(), AppError> {
    if !matches!(gateway.status.as_str(), "awaiting_payment" | "pending") {
        return Ok(());
    }
    let Some(checkout_url) = gateway.checkout_url.as_deref() else {
        return Ok(());
    };
    conn.execute(
        "UPDATE market_topup_orders
         SET checkout_url = ?2,
             status = CASE WHEN status = 'create_unknown' THEN 'pending' ELSE status END
         WHERE id = ?1 AND status IN ('created', 'creating', 'create_unknown', 'pending')
           AND buyer_abandoned_at IS NULL
           AND cancel_requested_at IS NULL",
        params![order_id, checkout_url],
    )
    .map_err(db_error("restore verified checkout URL"))?;
    Ok(())
}

fn validate_paid_order_tx(
    conn: &Connection,
    order_id: &str,
    channel_id: &str,
    revision: i64,
    loaded: &LoadedRevision,
    event: &WebhookEvent,
    gateway: &GatewayOrder,
) -> Result<Option<ValidatedTopup>, AppError> {
    validate_gateway_identity_tx(
        conn,
        order_id,
        channel_id,
        revision,
        loaded,
        &event.order_id,
        gateway,
    )?;
    let (user_id, merchant_order_id, stored_gateway_id, amount_minor, status, merchant_fingerprint): (String, String, Option<String>, i64, String, String) = conn.query_row(
        "SELECT user_id, merchant_order_id, gateway_order_id, amount_minor, status, merchant_fingerprint
         FROM market_topup_orders WHERE id = ?1 AND channel_id = ?2 AND channel_revision = ?3",
        params![order_id, channel_id, revision],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    ).map_err(db_error("read top-up for verification"))?;
    if merchant_order_id != event.merchant_order_id
        || merchant_order_id != gateway.merchant_order_id
        || event.order_id != gateway.order_id
        || stored_gateway_id
            .as_deref()
            .is_some_and(|id| id != gateway.order_id)
        || gateway.fiat_currency != "USD"
        || gateway.fiat_amount_minor != amount_minor
        || merchant_fingerprint != loaded.merchant_fingerprint
    {
        return Err(AppError::Conflict(
            "DujiaoPay order identity mismatch".into(),
        ));
    }
    if status == "credited" {
        return Ok(None);
    }
    validate_paid_terms_tx(conn, order_id, loaded, gateway)?;
    let fingerprint = receipt_fingerprint(&loaded.merchant_fingerprint, gateway)?;
    let existing_receipt = conn
        .query_row(
            "SELECT order_id FROM market_topup_receipts WHERE receipt_fingerprint = ?1",
            params![fingerprint],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error("check DujiaoPay receipt reuse"))?;
    if let Some(existing) = existing_receipt {
        return Err(if existing == order_id {
            AppError::Internal("top-up receipt exists without credited order".into())
        } else {
            AppError::Conflict("DujiaoPay receipt was already used".into())
        });
    }
    // A journal without a credited order is an invariant failure. Detect it
    // before inserting a receipt, because the webhook caller may persist a
    // review state after a validation conflict in this same transaction.
    let orphaned_journal = conn
        .query_row(
            "SELECT id FROM market_journal_transactions
             WHERE source_kind = 'topup' AND source_id = ?1 AND action = 'credit'",
            params![order_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error("check top-up journal state"))?;
    if orphaned_journal.is_some() {
        return Err(AppError::Internal(
            "top-up journal exists without credited order".into(),
        ));
    }
    Ok(Some(ValidatedTopup {
        user_id,
        amount_minor,
        receipt_fingerprint: fingerprint,
    }))
}

fn apply_paid_order_tx(
    conn: &Connection,
    order_id: &str,
    channel_id: &str,
    revision: i64,
    loaded: &LoadedRevision,
    event: &WebhookEvent,
    gateway: &GatewayOrder,
) -> Result<(), AppError> {
    let Some(validated) =
        validate_paid_order_tx(conn, order_id, channel_id, revision, loaded, event, gateway)?
    else {
        return Ok(());
    };
    resolve_compatible_paid_holds_tx(conn, order_id, &now())?;
    if topup_has_manual_hold_tx(conn, order_id)? {
        return Err(AppError::Conflict(
            "top-up is held for manual reconciliation".into(),
        ));
    }
    let time = now();
    conn.execute(
        "INSERT INTO market_topup_receipts (
            order_id, chain, token_id, tx_id, settled_amount, confirmations,
            receipt_fingerprint, observed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            order_id,
            gateway.chain,
            gateway.token_id,
            gateway.tx_id,
            gateway.settled_amount,
            gateway.confirmations,
            &validated.receipt_fingerprint,
            time
        ],
    )
    .map_err(db_error("record DujiaoPay receipt"))?;
    let journal = credit_verified_topup_tx(
        conn,
        &validated.user_id,
        order_id,
        validated.amount_minor,
        &validated.receipt_fingerprint,
        &time,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal(
            "top-up journal exists without credited order".into(),
        ));
    }
    conn.execute(
        "UPDATE market_topup_orders
         SET gateway_order_id = ?2, status = 'credited', credited_journal_id = ?3,
             paid_at = ?4, updated_at = ?5 WHERE id = ?1 AND status != 'credited'",
        params![
            order_id,
            gateway.order_id,
            journal.id,
            gateway.paid_at,
            time
        ],
    )
    .map_err(db_error("mark top-up credited"))?;
    finish_buyer_recheck_tx(conn, order_id, Some("credited"))?;
    notify_topup_tx(conn, order_id, "topup_credited", &time)?;
    if promote_channel_if_proven_tx(conn, channel_id, &time)? {
        enqueue_owner_signal_tx(
            conn,
            "channel_promoted_live",
            "payment_channel",
            channel_id,
            serde_json::json!({ "channelId": channel_id }),
            &time,
        )?;
    }
    Ok(())
}

fn topup_has_manual_hold_tx(conn: &Connection, order_id: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases
         WHERE object_kind = 'topup' AND object_id = ?1 AND status = 'open')",
        params![order_id],
        |row| row.get(0),
    )
    .map_err(db_error("read top-up review hold"))
}

fn compatible_paid_hold_ids_tx(
    conn: &Connection,
    order_id: &str,
) -> Result<Option<Vec<String>>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT id, code, COALESCE(detail_code, '')
             FROM market_finance_reconciliation_cases
             WHERE object_kind = 'topup' AND object_id = ?1 AND status = 'open'
             ORDER BY id",
        )
        .map_err(db_error("prepare paid top-up holds"))?;
    let cases = statement
        .query_map(params![order_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(db_error("query paid top-up holds"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read paid top-up holds"))?;
    if cases.iter().any(|(_, code, detail)| {
        code != "topup_verification_failed" || detail != "gateway_terminal_payment_evidence"
    }) {
        return Ok(None);
    }
    Ok(Some(cases.into_iter().map(|(id, _, _)| id).collect()))
}

fn resolve_compatible_paid_holds_tx(
    conn: &Connection,
    order_id: &str,
    time: &str,
) -> Result<(), AppError> {
    let Some(case_ids) = compatible_paid_hold_ids_tx(conn, order_id)? else {
        return Ok(());
    };
    for case_id in case_ids {
        crate::market_reconcile::resolve_case_tx(
            conn,
            &case_id,
            "system",
            "verified_gateway_paid",
            time,
        )?;
    }
    Ok(())
}

fn bind_signed_gateway_order_tx(
    conn: &Connection,
    order_id: &str,
    channel_id: &str,
    revision: i64,
    loaded: &LoadedRevision,
    event: &WebhookEvent,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
             SET gateway_order_id = COALESCE(gateway_order_id, ?2)
             WHERE id = ?1 AND channel_id = ?3 AND channel_revision = ?4
               AND merchant_fingerprint = ?5 AND merchant_order_id = ?6
               AND (gateway_order_id IS NULL OR gateway_order_id = ?2)",
            params![
                order_id,
                event.order_id,
                channel_id,
                revision,
                loaded.merchant_fingerprint,
                event.merchant_order_id
            ],
        )
        .map_err(db_error("bind signed gateway order"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "signed gateway order conflicts with stored order".into(),
        ));
    }
    Ok(())
}

fn hold_topup_verification_failure_tx(
    conn: &Connection,
    order_id: &str,
    detail_code: &str,
    time: &str,
) -> Result<(), AppError> {
    open_reconciliation_case_detail_tx(
        conn,
        "topup_verification_failed",
        "topup",
        order_id,
        Some(detail_code),
        time,
    )?;
    conn.execute(
        "UPDATE market_topup_orders SET status = 'review_required', updated_at = ?2
         WHERE id = ?1 AND status != 'credited'",
        params![order_id, time],
    )
    .map_err(db_error("hold top-up verification failure"))?;
    Ok(())
}

fn persist_signed_event_tx(
    conn: &Connection,
    order_id: &str,
    merchant_fingerprint: &str,
    event: &WebhookEvent,
    time: &str,
) -> Result<(), AppError> {
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO market_topup_events (
            id, order_id, merchant_fingerprint, gateway_event_id, event_type,
            raw_body_sha256, status, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
            params![
                Uuid::new_v4().to_string(),
                order_id,
                merchant_fingerprint,
                event.event_id,
                event.event_type,
                event.raw_sha256,
                time
            ],
        )
        .map_err(db_error("persist DujiaoPay webhook"))?;
    if inserted == 0 {
        let (stored_order, stored_type, stored_hash): (String, String, String) = conn
            .query_row(
                "SELECT order_id, event_type, raw_body_sha256 FROM market_topup_events
             WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2",
                params![merchant_fingerprint, event.event_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(db_error("read duplicate DujiaoPay event"))?;
        if stored_order != order_id
            || stored_type != event.event_type
            || stored_hash != event.raw_sha256
        {
            return Err(AppError::Conflict(
                "DujiaoPay event identity mismatch".into(),
            ));
        }
    }
    Ok(())
}

fn benign_progress_review_case_tx(
    conn: &Connection,
    order_id: &str,
) -> Result<Option<(String, bool)>, AppError> {
    conn.query_row(
        "SELECT review.id, review.status = 'open' FROM market_finance_reconciliation_cases review
         JOIN market_topup_orders topup ON topup.id = review.object_id
         WHERE topup.id = ?1 AND topup.status = 'review_required'
           AND review.object_kind = 'topup' AND review.code = 'unknown_gateway_event'
           AND EXISTS (SELECT 1 FROM market_topup_events event
                       WHERE event.order_id = topup.id
                         AND event.event_type IN
                             ('order.created', 'order.method_selected', 'order.confirming')
                         AND event.status = 'review_required')
           AND NOT EXISTS (SELECT 1 FROM market_topup_events event
                           WHERE event.order_id = topup.id
                             AND event.event_type NOT IN
                                 ('order.created', 'order.method_selected', 'order.confirming',
                                  'order.expired', 'order.cancelled', 'order.canceled'))
           AND NOT EXISTS (SELECT 1 FROM market_topup_events event
                           WHERE event.order_id = topup.id
                             AND event.status = 'review_required'
                             AND event.event_type NOT IN
                                 ('order.created', 'order.method_selected', 'order.confirming'))
           AND NOT EXISTS (SELECT 1 FROM market_finance_reconciliation_cases other
                           WHERE other.object_kind = 'topup' AND other.object_id = topup.id
                             AND other.code != 'unknown_gateway_event')
           AND NOT EXISTS (SELECT 1 FROM market_topup_receipts receipt
                           WHERE receipt.order_id = topup.id)
           AND NOT EXISTS (SELECT 1 FROM market_journal_transactions journal
                           WHERE journal.source_kind = 'topup'
                             AND journal.source_id = topup.id)
         ORDER BY review.status = 'open' DESC, review.created_at DESC LIMIT 1",
        params![order_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(db_error("check benign created-event review"))
}

fn chain_only_observation_hash(
    status: &str,
    rule_version: Option<i64>,
    confirmations: Option<i64>,
) -> String {
    let mut evidence = serde_json::json!({
        "status": status,
        "paidSource": "chain",
        "txId": null,
        "paidAt": null,
        "confirmations": confirmations,
        "settledAmount": null,
    });
    if let Some(version) = rule_version {
        evidence["evidenceRuleVersion"] = serde_json::json!(version);
    }
    hex::encode(Sha256::digest(evidence.to_string().as_bytes()))
}

fn historical_chain_only_observations_tx(
    conn: &Connection,
    order_id: &str,
) -> Result<bool, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT source, gateway_status, evidence_flags_json, evidence_sha256
                  FROM market_topup_gateway_observations WHERE order_id = ?1",
        )
        .map_err(db_error("prepare historic top-up observations"))?;
    let rows = statement
        .query_map(params![order_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(db_error("query historic top-up observations"))?;
    let mut saw_legacy_marker = false;
    for row in rows {
        let (source, status, flags_json, digest) =
            row.map_err(db_error("read historic top-up observation"))?;
        // Owner previews can record an unbound gateway response during an
        // identity-mismatch dry run. Only normal bound data-plane observations
        // may support automatic release of an old review hold.
        if !matches!(source.as_str(), "reconcile" | "webhook")
            || !matches!(
                status.as_str(),
                "awaiting_payment" | "pending" | "expired" | "canceled" | "cancelled"
            )
        {
            return Ok(false);
        }
        let Ok(flags): Result<Vec<String>, _> = serde_json::from_str(&flags_json) else {
            return Ok(false);
        };
        let version = if flags == ["gateway_paid_source"] {
            saw_legacy_marker = true;
            None
        } else if flags.is_empty() {
            Some(2)
        } else {
            return Ok(false);
        };
        if ![None, Some(0)].into_iter().any(|confirmations| {
            chain_only_observation_hash(&status, version, confirmations) == digest
        }) {
            return Ok(false);
        }
    }
    Ok(saw_legacy_marker)
}

fn recover_benign_chain_metadata_review_tx(
    conn: &Connection,
    order_id: &str,
    gateway: &GatewayOrder,
    status: &str,
    time: &str,
) -> Result<bool, AppError> {
    if !matches!(status, "expired" | "cancelled") || !unpaid_chain_rail_marker(gateway) {
        return Ok(false);
    }
    let current: String = conn
        .query_row(
            "SELECT status FROM market_topup_orders WHERE id = ?1",
            params![order_id],
            |row| row.get(0),
        )
        .map_err(db_error("read historic top-up review status"))?;
    if current != "review_required" {
        return Ok(false);
    }
    let mut cases = conn
        .prepare(
            "SELECT id, code, COALESCE(detail_code, ''), status
                  FROM market_finance_reconciliation_cases
                  WHERE object_kind = 'topup' AND object_id = ?1",
        )
        .map_err(db_error("prepare historic top-up review case"))?;
    let cases = cases
        .query_map(params![order_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(db_error("query historic top-up review case"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read historic top-up review case"))?;
    let [(case_id, code, detail, case_status)] = cases.as_slice() else {
        return Ok(false);
    };
    if code != "topup_verification_failed"
        || detail != "gateway_terminal_payment_evidence"
        || case_status != "open"
        || !historical_chain_only_observations_tx(conn, order_id)?
    {
        return Ok(false);
    }
    let assessment = recovery_assessment_tx(conn, order_id, &current, gateway)?;
    if !assessment.can_recover()
        || assessment.open_case_ids.len() != 1
        || assessment.open_case_ids[0] != *case_id
    {
        return Ok(false);
    }
    let changed = conn
        .execute(
            "UPDATE market_topup_orders SET status = ?2, checkout_url = NULL,
             updated_at = ?3 WHERE id = ?1 AND status = 'review_required'",
            params![order_id, status, time],
        )
        .map_err(db_error("recover benign chain metadata review"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "top-up changed during benign metadata recovery".into(),
        ));
    }
    crate::market_reconcile::resolve_case_tx(
        conn,
        case_id,
        "system",
        "verified_chain_rail_metadata",
        time,
    )?;
    audit_tx(
        conn,
        "system",
        "topup_chain_metadata_review_recovered",
        "topup",
        order_id,
        Some("verified_chain_rail_metadata"),
        time,
    )?;
    finish_buyer_recheck_from_order_tx(conn, order_id)?;
    Ok(true)
}

fn apply_verified_unpaid_status_tx(
    conn: &Connection,
    order_id: &str,
    status: &str,
    time: &str,
) -> Result<(), AppError> {
    let terminal = matches!(status, "expired" | "cancelled");
    let benign_case = if terminal {
        benign_progress_review_case_tx(conn, order_id)?
    } else {
        None
    };
    conn.execute(
        "UPDATE market_topup_orders
         SET status = CASE WHEN ?2 = 'pending' AND checkout_url IS NULL AND pay_address IS NULL
                                AND buyer_abandoned_at IS NULL AND cancel_requested_at IS NULL
                           THEN 'create_unknown' ELSE ?2 END,
             updated_at = ?3
         WHERE id = ?1 AND (status IN ('created', 'creating', 'create_unknown',
                                      'pending', 'verifying')
                            OR (status IN ('expired', 'cancelled') AND ?4 = 1)
                            OR (status = 'review_required' AND ?5 = 1))",
        params![order_id, status, time, terminal, benign_case.is_some()],
    )
    .map_err(db_error("record verified unpaid top-up"))?;
    if let Some((case_id, true)) = benign_case {
        crate::market_reconcile::resolve_case_tx(
            conn,
            &case_id,
            "system",
            "verified_gateway_terminal",
            time,
        )?;
    }
    finish_buyer_recheck_from_order_tx(conn, order_id)?;
    Ok(())
}

fn verified_unpaid_status(gateway_status: &str) -> Result<&'static str, AppError> {
    match gateway_status {
        "awaiting_payment" | "pending" => Ok("pending"),
        "confirming" => Ok("verifying"),
        "expired" => Ok("expired"),
        "canceled" | "cancelled" => Ok("cancelled"),
        _ => Err(AppError::Conflict(
            "DujiaoPay order status requires review".into(),
        )),
    }
}

fn exact_decimal_zero(value: &str) -> bool {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    !whole.is_empty()
        && whole.bytes().all(|byte| byte == b'0')
        && (fraction.is_empty() && !value.contains('.')
            || !fraction.is_empty() && fraction.bytes().all(|byte| byte == b'0'))
}

fn record_gateway_observation_tx(
    conn: &Connection,
    order_id: &str,
    source: &'static str,
    gateway: &GatewayOrder,
    time: &str,
) -> Result<(), AppError> {
    let flags = gateway_recovery_blockers(gateway)
        .into_iter()
        .filter(|flag| *flag != "gateway_not_terminal")
        .collect::<Vec<_>>();
    let flags_json = serde_json::to_string(&flags)
        .map_err(|_| AppError::Internal("encode redacted gateway observation failed".into()))?;
    let fingerprint = serde_json::json!({
        // Evidence classification changed after a live merchant response
        // showed paid_source=chain on an unpaid awaiting_payment order. Keep
        // the original observation immutable and write a new v2 observation.
        "evidenceRuleVersion": 2,
        "status": gateway.status,
        "paidSource": gateway.paid_source,
        "txId": gateway.tx_id,
        "paidAt": gateway.paid_at,
        "confirmations": gateway.confirmations,
        "settledAmount": gateway.settled_amount,
    });
    let evidence_sha256 = hex::encode(Sha256::digest(fingerprint.to_string().as_bytes()));
    conn.execute(
        "INSERT INTO market_topup_gateway_observations
         (id, order_id, source, gateway_status, evidence_flags_json, evidence_sha256,
          first_observed_at, last_observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT(order_id, source, gateway_status, evidence_sha256)
         DO UPDATE SET last_observed_at = excluded.last_observed_at,
                       observation_count = observation_count + 1",
        params![
            Uuid::new_v4().to_string(),
            order_id,
            source,
            gateway.status,
            flags_json,
            evidence_sha256,
            time
        ],
    )
    .map_err(db_error("record redacted gateway observation"))?;
    Ok(())
}

fn gateway_recovery_blockers(gateway: &GatewayOrder) -> Vec<&'static str> {
    let mut blockers = Vec::new();
    if !matches!(
        gateway.status.as_str(),
        "expired" | "canceled" | "cancelled"
    ) {
        blockers.push("gateway_not_terminal");
    }
    if gateway.paid_source.is_some() && !unpaid_chain_rail_marker(gateway) {
        blockers.push("gateway_paid_source");
    }
    if gateway.tx_id.is_some() {
        blockers.push("gateway_transaction");
    }
    if gateway.paid_at.is_some() {
        blockers.push("gateway_paid_at");
    }
    if gateway.confirmations.unwrap_or(0) != 0 {
        blockers.push("gateway_confirmations");
    }
    if let Some(amount) = gateway.settled_amount.as_deref()
        && !exact_decimal_zero(amount)
    {
        blockers.push("gateway_settled_amount");
    }
    blockers
}

fn unpaid_chain_rail_marker(gateway: &GatewayOrder) -> bool {
    // An authenticated DujiaoPay merchant response contains paid_source=chain
    // even for awaiting_payment and expired orders with no transfer. Treat
    // that exact value as rail metadata only in known unpaid lifecycle states.
    // Every other source and every independent payment clue still blocks an
    // unpaid conclusion; paid orders retain the strict chain receipt gate.
    gateway.paid_source.as_deref() == Some("chain")
        && matches!(
            gateway.status.as_str(),
            "awaiting_payment" | "pending" | "expired" | "canceled" | "cancelled"
        )
        && gateway.tx_id.is_none()
        && gateway.paid_at.is_none()
        && gateway.confirmations.unwrap_or(0) == 0
        && gateway
            .settled_amount
            .as_deref()
            .is_none_or(exact_decimal_zero)
}

fn gateway_terminal_without_payment_evidence(gateway: &GatewayOrder) -> bool {
    gateway_recovery_blockers(gateway).is_empty()
}

fn is_non_payment_lifecycle_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "order.created" | "order.method_selected" | "order.confirming"
    )
}

fn bound_webhook_order_tx(
    conn: &Connection,
    channel_id: &str,
    merchant_order_hint: &str,
) -> Result<Option<(String, i64)>, AppError> {
    conn.query_row(
        "SELECT id, channel_revision FROM market_topup_orders
         WHERE merchant_order_id = ?1 AND channel_id = ?2",
        params![merchant_order_hint, channel_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )
    .optional()
    .map_err(db_error("bind DujiaoPay webhook"))
}

fn queue_webhook_hint_tx(conn: &Connection, order_id: &str, time: &str) -> Result<bool, AppError> {
    let next_hint =
        (Utc::now() + chrono::Duration::seconds(WEBHOOK_HINT_COOLDOWN_SECS)).to_rfc3339();
    let recovery_until =
        (Utc::now() + chrono::Duration::seconds(WEBHOOK_HINT_RECOVERY_SECS)).to_rfc3339();
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
         SET next_reconcile_at = ?2, next_hint_at = ?3,
             hint_reconcile_until = ?4, reconcile_hint_pending = 1
         WHERE id = ?1 AND status != 'credited'
           AND (next_hint_at IS NULL OR next_hint_at <= ?2)",
            params![order_id, time, next_hint, recovery_until],
        )
        .map_err(db_error("queue DujiaoPay webhook hint"))?;
    Ok(changed == 1)
}

/// A verified signed event always schedules an immediate authenticated
/// lookup. Unlike an unsigned hint it is not subject to the cooldown.
fn queue_signed_event_reconcile_tx(
    conn: &Connection,
    order_id: &str,
    time: &str,
) -> Result<(), AppError> {
    let recovery_until =
        (Utc::now() + chrono::Duration::seconds(WEBHOOK_HINT_RECOVERY_SECS)).to_rfc3339();
    conn.execute(
        "UPDATE market_topup_orders
         SET next_reconcile_at = ?2, reconcile_hint_pending = 1,
             hint_reconcile_until = CASE WHEN hint_reconcile_until IS NULL
                 OR hint_reconcile_until < ?3 THEN ?3 ELSE hint_reconcile_until END
         WHERE id = ?1 AND status != 'credited'",
        params![order_id, time, recovery_until],
    )
    .map_err(db_error("queue signed DujiaoPay event"))?;
    Ok(())
}

/// Record verified `order.method_selected` terms. Values only fill blanks; an
/// authenticated lookup remains the authority that may overwrite them.
fn record_selected_terms_tx(
    conn: &Connection,
    order_id: &str,
    terms: &dujiaopay::SelectedTerms,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE market_topup_orders
         SET gateway_chain = COALESCE(gateway_chain, ?2),
             gateway_token_id = COALESCE(gateway_token_id, ?3),
             pay_address = COALESCE(pay_address, ?4),
             payable_amount = COALESCE(payable_amount, ?5),
             gateway_expires_at = COALESCE(gateway_expires_at, ?6)
         WHERE id = ?1",
        params![
            order_id,
            terms.chain,
            terms.token_id,
            terms.pay_address,
            terms.payable_amount,
            terms.expires_at
        ],
    )
    .map_err(db_error("record selected payment terms"))?;
    Ok(())
}

/// Authenticated lookups refresh the order's gateway terms.
fn record_gateway_terms_tx(
    conn: &Connection,
    order_id: &str,
    gateway: &GatewayOrder,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE market_topup_orders
         SET gateway_chain = COALESCE(?2, gateway_chain),
             gateway_token_id = COALESCE(?3, gateway_token_id),
             pay_address = COALESCE(?4, pay_address),
             payable_amount = COALESCE(?5, payable_amount),
             fx_rate = COALESCE(?6, fx_rate),
             gateway_expires_at = COALESCE(?7, gateway_expires_at)
         WHERE id = ?1",
        params![
            order_id,
            gateway
                .chain
                .as_deref()
                .filter(|_| gateway.token_id.is_some()),
            gateway
                .token_id
                .as_deref()
                .filter(|token| token_rail(token).is_some()),
            gateway.pay_address,
            gateway.payable_amount,
            gateway.fx_rate,
            gateway.expires_at
        ],
    )
    .map_err(db_error("record gateway payment terms"))?;
    Ok(())
}

/// Signed events that are not bound to a Router order: `webhook.test` proves
/// the current Webhook secret, `unmatched_claim.claimed` schedules a lookup or
/// opens an Owner case. Nothing here moves money.
async fn handle_unbound_webhook(
    state: &ServerState,
    only_channel: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<bool, AppError> {
    let conn = state.store.conn.lock().await;
    let candidates = {
        let mut statement = conn
            .prepare(
                "SELECT c.id, c.current_revision FROM market_payment_channels c
                 JOIN market_payment_channel_revisions r
                   ON r.channel_id = c.id AND r.revision = c.current_revision
                 WHERE c.archived_at IS NULL AND r.webhook_mode = 'signed'
                   AND (?1 IS NULL OR c.id = ?1)
                 ORDER BY c.rollout_state = 'off', c.sort_order, c.id LIMIT 16",
            )
            .map_err(db_error("prepare signed webhook channels"))?;
        statement
            .query_map(params![only_channel], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(db_error("query signed webhook channels"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read signed webhook channels"))?
    };
    let now_unix = Utc::now().timestamp();
    for (channel_id, revision) in candidates {
        let Ok(loaded) = load_revision(&conn, &state.market_payments, &channel_id, revision) else {
            continue;
        };
        let Ok(event) =
            dujiaopay::verify_unbound_webhook(&loaded.credentials, headers, body, now_unix)
        else {
            continue;
        };
        tracing::info!(
            channel_id = %channel_id,
            event_id = %event.event_id,
            event_type = %event.event_type,
            "verified unbound DujiaoPay event"
        );
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin unbound webhook"))?;
        let time = now();
        match event.event_type.as_str() {
            "webhook.test" => {
                if mark_channel_webhook_verified_tx(&tx, &channel_id, revision, &time)? {
                    audit_tx(
                        &tx,
                        "system",
                        "channel_webhook_test_verified",
                        "payment_channel",
                        &channel_id,
                        None,
                        &time,
                    )?;
                }
            }
            "unmatched_claim.claimed" => {
                let order = match event.order_id.as_deref() {
                    Some(gateway_order_id) => tx
                        .query_row(
                            "SELECT id FROM market_topup_orders
                             WHERE merchant_fingerprint = ?1 AND gateway_order_id = ?2",
                            params![loaded.merchant_fingerprint, gateway_order_id],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(db_error("locate claimed gateway order"))?,
                    None => None,
                };
                match order {
                    Some(order_id) => queue_signed_event_reconcile_tx(&tx, &order_id, &time)?,
                    None => {
                        open_reconciliation_case_tx(
                            &tx,
                            "gateway_unmatched_claim",
                            "payment_channel",
                            &channel_id,
                            &time,
                        )?;
                        enqueue_owner_signal_tx(
                            &tx,
                            "gateway_orphan_paid_order",
                            "payment_channel",
                            &channel_id,
                            serde_json::json!({ "eventType": event.event_type }),
                            &time,
                        )?;
                    }
                }
            }
            _ => {}
        }
        tx.commit().map_err(db_error("commit unbound webhook"))?;
        state.market_payments.wake_reconciler();
        return Ok(true);
    }
    Ok(false)
}

/// The fixed URL is suitable for a merchant dashboard before a channel exists.
/// Its body is never payment evidence: it may only wake a bounded authenticated
/// API lookup of an order already created by this Router.
async fn dujiaopay_webhook_hint(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let Some(merchant_order_hint) = dujiaopay::optional_merchant_order_hint(&body)? else {
        let verified = handle_unbound_webhook(&state, None, &headers, &body).await?;
        return Ok(Json(serde_json::json!({
            "accepted": true,
            "ignored": !verified
        })));
    };
    let bound = {
        let conn = state.store.conn.lock().await;
        conn.query_row(
            "SELECT id, channel_id, channel_revision FROM market_topup_orders
             WHERE merchant_order_id = ?1",
            params![merchant_order_hint],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("bind DujiaoPay callback hint"))?
    };
    let Some((order_id, channel_id, bound_revision)) = bound else {
        // A valid-shaped unknown order is indistinguishable from a duplicate
        // hint. The merchant order ID is a random, unguessable 128-bit value.
        return Ok(Json(serde_json::json!({ "accepted": true })));
    };
    let signed = {
        let conn = state.store.conn.lock().await;
        let current_revision: i64 = conn
            .query_row(
                "SELECT current_revision FROM market_payment_channels WHERE id = ?1",
                params![channel_id],
                |row| row.get(0),
            )
            .map_err(db_error("read callback channel"))?;
        let loaded = load_revision(&conn, &state.market_payments, &channel_id, current_revision)?;
        let bound = load_revision(&conn, &state.market_payments, &channel_id, bound_revision)?;
        if loaded.merchant_fingerprint == bound.merchant_fingerprint {
            loaded.credentials.webhook_secret.is_some()
        } else {
            bound.credentials.webhook_secret.is_some()
        }
    };
    if signed {
        // Old signed revisions retain their full event handling and audit,
        // whether the merchant uses the fixed or legacy URL.
        return dujiaopay_webhook(State(state), Path(channel_id), headers, body)
            .await
            .map(|Json(value)| Json(value));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin DujiaoPay callback hint"))?;
    queue_webhook_hint_tx(&tx, &order_id, &now())?;
    tx.commit()
        .map_err(db_error("commit DujiaoPay callback hint"))?;
    state.market_payments.wake_reconciler();
    Ok(Json(serde_json::json!({ "accepted": true })))
}

fn mark_event_tx(
    conn: &Connection,
    merchant_fingerprint: &str,
    event_id: &str,
    status: &str,
    time: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE market_topup_events SET status = ?3, processed_at = ?4,
                processed_by = CASE WHEN ?3 = 'processed' THEN 'webhook' ELSE processed_by END
         WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2",
        params![merchant_fingerprint, event_id, status, time],
    )
    .map_err(db_error("finish payment event"))?;
    Ok(())
}

/// Signed DujiaoPay events. The handler verifies, persists and binds the
/// event, schedules an immediate authenticated lookup and returns 2xx at
/// once; payment state changes only in reconciliation, from `GET
/// /v1/orders/{id}`. Lookups therefore never hold the gateway's request open.
async fn dujiaopay_webhook(
    State(state): State<ServerState>,
    Path(channel_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    if channel_id.len() > 80
        || !channel_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(AppError::BadRequest("invalid payment channel".into()));
    }
    // The untrusted order hint only locates an already-bound order. In
    // poll-only mode it may schedule a bounded authenticated lookup; signed
    // mode acts on no payment field until the raw body HMAC is verified.
    let Some(merchant_order_hint) = dujiaopay::optional_merchant_order_hint(&body)? else {
        let verified = handle_unbound_webhook(&state, Some(&channel_id), &headers, &body).await?;
        return Ok(Json(serde_json::json!({
            "accepted": true,
            "ignored": !verified
        })));
    };
    let conn = state.store.conn.lock().await;
    let Some(bound) = bound_webhook_order_tx(&conn, &channel_id, &merchant_order_hint)? else {
        // Unknown orders are acknowledged so the gateway does not retry an
        // event this Router can never act on.
        return Ok(Json(
            serde_json::json!({ "accepted": true, "ignored": true }),
        ));
    };
    let current_revision: i64 = conn
        .query_row(
            "SELECT current_revision FROM market_payment_channels WHERE id = ?1",
            params![channel_id],
            |row| row.get(0),
        )
        .map_err(db_error("read callback channel"))?;
    let current = load_revision(&conn, &state.market_payments, &channel_id, current_revision)?;
    let historical = load_revision(&conn, &state.market_payments, &channel_id, bound.1)?;
    let has_signed_key = if current.merchant_fingerprint == historical.merchant_fingerprint {
        current.credentials.webhook_secret.is_some()
    } else {
        historical.credentials.webhook_secret.is_some()
    };
    if !has_signed_key {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin callback lookup hint"))?;
        queue_webhook_hint_tx(&tx, &bound.0, &now())?;
        tx.commit()
            .map_err(db_error("commit callback lookup hint"))?;
        state.market_payments.wake_reconciler();
        return Ok(Json(serde_json::json!({ "accepted": true })));
    }
    let (loaded, event, verified_current_revision) = verify_bound_webhook_tx(
        &conn,
        &state.market_payments,
        &channel_id,
        bound.1,
        &headers,
        &body,
        Utc::now().timestamp(),
    )?;
    if event.merchant_order_id != merchant_order_hint {
        return Err(AppError::Unauthorized(
            "DujiaoPay webhook not recognized".into(),
        ));
    }
    let (order_id, revision) = bound;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin webhook event"))?;
    let time = now();
    persist_signed_event_tx(&tx, &order_id, &loaded.merchant_fingerprint, &event, &time)?;
    if let Some(current_revision) = verified_current_revision {
        mark_channel_webhook_verified_tx(&tx, &channel_id, current_revision, &time)?;
    }
    let mut review_required = false;
    match event.event_type.as_str() {
        "order.created"
        | "order.method_selected"
        | "order.confirming"
        | "order.paid"
        | "order.expired"
        | "order.cancelled"
        | "order.canceled" => {
            // Signed callbacks can arrive before the create API returns. Their
            // order ID is a recovery locator, never evidence of payment.
            match bind_signed_gateway_order_tx(
                &tx,
                &order_id,
                &channel_id,
                revision,
                &loaded,
                &event,
            ) {
                Ok(()) => {
                    if event.event_type == "order.method_selected" {
                        record_selected_terms_tx(
                            &tx,
                            &order_id,
                            &dujiaopay::verified_selected_terms(&body),
                        )?;
                    }
                    if is_non_payment_lifecycle_event(&event.event_type) {
                        mark_event_tx(
                            &tx,
                            &loaded.merchant_fingerprint,
                            &event.event_id,
                            "processed",
                            &time,
                        )?;
                    }
                    queue_signed_event_reconcile_tx(&tx, &order_id, &time)?;
                }
                Err(AppError::Conflict(_)) => {
                    hold_topup_verification_failure_tx(
                        &tx,
                        &order_id,
                        "signed_gateway_order_conflict",
                        &time,
                    )?;
                    mark_event_tx(
                        &tx,
                        &loaded.merchant_fingerprint,
                        &event.event_id,
                        "review_required",
                        &time,
                    )?;
                    enqueue_owner_signal_tx(
                        &tx,
                        "gateway_identity_conflict",
                        "topup",
                        &order_id,
                        serde_json::json!({ "eventType": event.event_type }),
                        &time,
                    )?;
                    review_required = true;
                }
                Err(error) => return Err(error),
            }
        }
        "refund.recorded" => {
            open_reconciliation_case_tx(
                &tx,
                "external_refund_recorded",
                "topup",
                &order_id,
                &time,
            )?;
            tx.execute(
                "UPDATE market_topup_orders SET status = 'review_required', updated_at = ?2
                 WHERE id = ?1 AND status != 'credited'",
                params![order_id, time],
            )
            .map_err(db_error("hold top-up for refund review"))?;
            mark_event_tx(
                &tx,
                &loaded.merchant_fingerprint,
                &event.event_id,
                "review_required",
                &time,
            )?;
            enqueue_owner_signal_tx(
                &tx,
                "gateway_external_refund",
                "topup",
                &order_id,
                serde_json::json!({ "orderId": order_id }),
                &time,
            )?;
            review_required = true;
        }
        _ => {
            // An unrecognized signed event is kept as evidence (it still
            // blocks cancellation and checkout in the recovery assessment) and
            // the Owner is told, but the order is not moved to review on a
            // type this Router cannot interpret.
            mark_event_tx(
                &tx,
                &loaded.merchant_fingerprint,
                &event.event_id,
                "processed",
                &time,
            )?;
            open_reconciliation_case_tx(
                &tx,
                "gateway_event_unrecognized",
                "payment_channel",
                &channel_id,
                &time,
            )?;
            queue_signed_event_reconcile_tx(&tx, &order_id, &time)?;
        }
    }
    tx.commit().map_err(db_error("commit DujiaoPay webhook"))?;
    drop(conn);
    state.market_payments.wake_reconciler();
    Ok(Json(serde_json::json!({
        "accepted": true,
        "reviewRequired": review_required
    })))
}

#[derive(Clone)]
struct PendingTopup {
    id: String,
    user_id: String,
    channel_id: String,
    revision: i64,
    merchant_order_id: String,
    gateway_order_id: Option<String>,
    amount_minor: i64,
    had_hint: bool,
    auto_close_at: Option<String>,
    created_at: String,
    /// Stopped by the buyer or cancel-requested: a gateway order bound for
    /// the first time by this pass must not stay payable.
    stopped: bool,
}

fn auto_close_due(
    deadline: Option<&str>,
    gateway_expires_at: Option<&str>,
    time: chrono::DateTime<Utc>,
) -> bool {
    let local_due = deadline
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|deadline| deadline <= time);
    let gateway_still_payable = gateway_expires_at.is_some_and(|value| {
        chrono::DateTime::parse_from_rfc3339(value).map_or(true, |deadline| deadline > time)
    });
    local_due && !gateway_still_payable
}

fn should_auto_cancel_tx(
    conn: &Connection,
    pending: &PendingTopup,
    gateway: &GatewayOrder,
    time: chrono::DateTime<Utc>,
) -> Result<bool, AppError> {
    if gateway.status != "pending"
        || !auto_close_due(
            pending.auto_close_at.as_deref(),
            gateway.expires_at.as_deref(),
            time,
        )
        || gateway_cancel_was_requested_tx(conn, &pending.id)?
    {
        return Ok(false);
    }
    let status = read_topup(conn, &pending.id, &pending.user_id)?.status;
    let assessment = recovery_assessment_tx(conn, &pending.id, &status, gateway)?;
    Ok(can_cancel_gateway_topup(
        &status,
        &gateway.status,
        &assessment,
        false,
    ))
}

fn pending_topups_tx(conn: &Connection) -> Result<Vec<PendingTopup>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT id, user_id, channel_id, channel_revision, merchant_order_id, gateway_order_id,
                amount_minor, reconcile_hint_pending, auto_close_at, created_at,
                (buyer_abandoned_at IS NOT NULL OR cancel_requested_at IS NOT NULL)
         FROM market_topup_orders
         WHERE COALESCE(next_reconcile_at, updated_at) <= ?1
           AND local_close_reason IS NULL
           AND status IN ('created', 'creating', 'create_unknown', 'pending', 'verifying',
                          'expired', 'cancelled', 'review_required')
           AND (created_at >= ?2 OR status NOT IN ('expired', 'cancelled')
                OR reconcile_hint_pending = 1
                OR hint_reconcile_until >= ?1
                OR (buyer_abandoned_at IS NOT NULL
                    AND status IN ('created', 'creating', 'create_unknown',
                                   'pending', 'verifying', 'review_required')))
         ORDER BY COALESCE(next_reconcile_at, updated_at), id LIMIT 24",
        )
        .map_err(db_error("prepare pending top-ups"))?;
    statement
        .query_map(
            params![
                Utc::now().to_rfc3339(),
                (Utc::now() - chrono::Duration::days(TERMINAL_RECONCILE_DAYS)).to_rfc3339()
            ],
            |row| {
                Ok(PendingTopup {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    channel_id: row.get(2)?,
                    revision: row.get(3)?,
                    merchant_order_id: row.get(4)?,
                    gateway_order_id: row.get(5)?,
                    amount_minor: row.get(6)?,
                    had_hint: row.get::<_, i64>(7)? == 1,
                    auto_close_at: row.get(8)?,
                    created_at: row.get(9)?,
                    stopped: row.get::<_, i64>(10)? == 1,
                })
            },
        )
        .map_err(db_error("query pending top-ups"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read pending top-ups"))
}

fn claim_reconciliation_tx(conn: &Connection, order_id: &str) -> Result<bool, AppError> {
    let time = now();
    let leased_until = (Utc::now() + chrono::Duration::seconds(RECONCILE_LEASE_SECS)).to_rfc3339();
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
         SET next_reconcile_at = ?3, reconcile_hint_pending = 0
         WHERE id = ?1 AND status != 'credited'
           AND COALESCE(next_reconcile_at, updated_at) <= ?2",
            params![order_id, time, leased_until],
        )
        .map_err(db_error("claim top-up reconciliation"))?;
    Ok(changed == 1)
}

fn finish_reconciliation_tx(
    conn: &Connection,
    order_id: &str,
    had_hint: bool,
) -> Result<(), AppError> {
    let (status, created_at, hint_reconcile_until): (String, String, Option<String>) = conn
        .query_row(
            "SELECT status, created_at, hint_reconcile_until
             FROM market_topup_orders WHERE id = ?1",
            params![order_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db_error("read reconciled top-up state"))?;
    if status == "credited" {
        conn.execute(
            "UPDATE market_topup_orders SET next_reconcile_at = NULL,
             reconcile_hint_pending = 0, reconcile_failures = 0 WHERE id = ?1",
            params![order_id],
        )
        .map_err(db_error("finish credited top-up reconciliation"))?;
        return Ok(());
    }
    let age = chrono::DateTime::parse_from_rfc3339(&created_at)
        .map(|created| Utc::now().signed_duration_since(created).num_seconds())
        .unwrap_or(i64::MAX);
    let delay_secs = if matches!(status.as_str(), "expired" | "cancelled") {
        if age < 86_400 {
            300
        } else if age < 604_800 {
            3_600
        } else {
            86_400
        }
    } else if status == "review_required" {
        86_400
    } else if age < 3_600 {
        30
    } else if age < 86_400 {
        300
    } else if age < 604_800 {
        3_600
    } else {
        86_400
    };
    let time = now();
    let recovery_active = hint_reconcile_until
        .as_deref()
        .is_some_and(|until| until >= time.as_str());
    let delay_secs = if had_hint {
        delay_secs.min(300)
    } else if age >= TERMINAL_RECONCILE_DAYS * 86_400 && recovery_active {
        delay_secs.min(1_800)
    } else {
        delay_secs
    };
    let next = (Utc::now() + chrono::Duration::seconds(delay_secs)).to_rfc3339();
    conn.execute(
        "UPDATE market_topup_orders SET next_reconcile_at =
         CASE WHEN reconcile_hint_pending = 1 THEN ?2 ELSE ?3 END
         WHERE id = ?1",
        params![order_id, time, next],
    )
    .map_err(db_error("schedule next top-up reconciliation"))?;
    Ok(())
}

fn finish_buyer_recheck_tx(
    conn: &Connection,
    order_id: &str,
    result: Option<&str>,
) -> Result<(), AppError> {
    let result = match result {
        Some(
            result @ ("pending" | "verifying" | "review_required" | "credited" | "expired"
            | "cancelled"),
        ) => result,
        _ => "retrying",
    };
    conn.execute(
        "UPDATE market_topup_orders
         SET buyer_recheck_completed_at = ?2, buyer_recheck_result = ?3
         WHERE id = ?1 AND buyer_recheck_requested_at IS NOT NULL
           AND buyer_recheck_result IN ('queued', 'retrying')",
        params![order_id, now(), result],
    )
    .map_err(db_error("finish buyer top-up recheck"))?;
    Ok(())
}

fn finish_buyer_recheck_from_order_tx(conn: &Connection, order_id: &str) -> Result<(), AppError> {
    let status: String = conn
        .query_row(
            "SELECT status FROM market_topup_orders WHERE id = ?1",
            params![order_id],
            |row| row.get(0),
        )
        .map_err(db_error("read checked buyer top-up status"))?;
    let result = match status.as_str() {
        "created" | "creating" | "create_unknown" => "pending",
        _ => &status,
    };
    finish_buyer_recheck_tx(conn, order_id, Some(result))
}

/// Outcome of looking for a gateway order by merchant order ID in the
/// authenticated order list.
enum GatewaySearch {
    Found(String),
    ProvenAbsent,
    Inconclusive,
}

const SEARCH_MAX_PAGES: u32 = 10;

/// Scan the gateway's order list from shortly before the local order was
/// created until now. Absence is proven only when the whole window fits in
/// the scanned pages.
async fn find_gateway_order_by_merchant_id(
    state: &ServerState,
    credentials: &Credentials,
    merchant_order_id: &str,
    created_at: &str,
) -> GatewaySearch {
    let Ok(created) = chrono::DateTime::parse_from_rfc3339(created_at) else {
        return GatewaySearch::Inconclusive;
    };
    let after = (created.with_timezone(&Utc) - chrono::Duration::minutes(10))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for page in 0..SEARCH_MAX_PAGES {
        let result = state
            .market_payments
            .client
            .list_orders(
                credentials,
                &dujiaopay::ListOrdersQuery {
                    created_after: Some(after.clone()),
                    limit: dujiaopay::LIST_PAGE_LIMIT,
                    offset: page * dujiaopay::LIST_PAGE_LIMIT,
                    ..Default::default()
                },
            )
            .await;
        let Ok(orders) = result else {
            return GatewaySearch::Inconclusive;
        };
        if let Some(found) = orders
            .orders
            .iter()
            .find(|order| order.merchant_order_id.as_deref() == Some(merchant_order_id))
        {
            return GatewaySearch::Found(found.order_id.clone());
        }
        // A malformed entry might be the order searched for, and a missing
        // total says nothing about further pages: neither proves absence.
        if !orders.complete() {
            return GatewaySearch::Inconclusive;
        }
        let scanned = u64::from(page * dujiaopay::LIST_PAGE_LIMIT) + u64::from(orders.returned);
        if orders.is_last(scanned) {
            return GatewaySearch::ProvenAbsent;
        }
    }
    GatewaySearch::Inconclusive
}

/// Close an order whose identical create request was rejected twice and
/// whose merchant order ID is absent from the gateway's order list. No status
/// is added: it becomes `cancelled` with `local_close_reason = never_created`.
fn close_never_created_tx(conn: &Connection, order_id: &str, time: &str) -> Result<bool, AppError> {
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
             SET status = 'cancelled', local_close_reason = 'never_created',
                 next_reconcile_at = NULL, reconcile_hint_pending = 0, updated_at = ?2
             WHERE id = ?1 AND gateway_order_id IS NULL
               AND status IN ('created', 'creating', 'create_unknown')
               AND NOT EXISTS (SELECT 1 FROM market_topup_events e WHERE e.order_id = ?1)
               AND NOT EXISTS (SELECT 1 FROM market_topup_receipts r WHERE r.order_id = ?1)",
            params![order_id, time],
        )
        .map_err(db_error("close never-created top-up"))?;
    if changed == 1 {
        audit_tx(
            conn,
            "system",
            "topup_closed_never_created",
            "topup",
            order_id,
            Some("gateway_rejected_identical_request"),
            time,
        )?;
        notify_topup_tx(conn, order_id, "topup_closed_unpaid", time)?;
        finish_buyer_recheck_tx(conn, order_id, Some("cancelled"))?;
    }
    Ok(changed == 1)
}

/// Queue a buyer notification for a top-up state change.
fn notify_topup_tx(
    conn: &Connection,
    order_id: &str,
    kind: &str,
    time: &str,
) -> Result<(), AppError> {
    let (user_id, amount_minor, status, fee, gross): (String, i64, String, i64, i64) = conn
        .query_row(
            "SELECT user_id, COALESCE(credit_amount_minor, amount_minor), status, platform_fee_minor,
                    amount_minor FROM market_topup_orders WHERE id = ?1",
            params![order_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .map_err(db_error("read top-up for notification"))?;
    let review_case: Option<String> = if kind == "topup_review_required" {
        conn.query_row(
            "SELECT id FROM market_finance_reconciliation_cases
            WHERE object_kind='topup' AND object_id=?1 AND status='open'
            ORDER BY created_at DESC,id DESC LIMIT 1",
            params![order_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read top-up notification episode"))?
    } else {
        None
    };
    let dedupe = review_case.as_ref().map_or_else(
        || format!("{kind}:{order_id}"),
        |id| format!("{kind}:{order_id}:{id}"),
    );
    crate::market_notifications::enqueue_user_notification_tx(
        conn,
        "buyer",
        &user_id,
        kind,
        "topup",
        order_id,
        &dedupe,
        serde_json::json!({ "amountMinor": amount_minor, "creditMinor": amount_minor,
            "feeMinor": fee, "payMinor": gross, "status": status, "orderId": order_id, "reviewCaseId":review_case }),
        time,
    )
}

/// Create (or idempotently replay) the gateway order for a top-up whose
/// create result is unknown. Returns the bound gateway order ID, or `None`
/// when this pass recorded a failure or closed the order.
async fn recover_gateway_order(
    state: &ServerState,
    pending: &PendingTopup,
    loaded: &LoadedRevision,
    creation_credentials: &Credentials,
) -> Result<Option<String>, AppError> {
    let (status, frozen_body): (String, Option<String>) = {
        let conn = state.store.conn.lock().await;
        conn.query_row(
            "SELECT status, create_request_body FROM market_topup_orders WHERE id = ?1",
            params![pending.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read top-up create request"))?
    };
    if !matches!(status.as_str(), "created" | "creating" | "create_unknown") {
        // Never create a gateway order for a closed or held order that has
        // no gateway identity.
        let conn = state.store.conn.lock().await;
        finish_reconciliation_tx(&conn, &pending.id, pending.had_hint)?;
        return Ok(None);
    }
    let body = match frozen_body {
        Some(body) => body.into_bytes(),
        None => legacy_create_body(
            state,
            &pending.id,
            &pending.merchant_order_id,
            pending.amount_minor,
            &loaded.allowed_methods,
        )?,
    };
    let result = state
        .market_payments
        .client
        .create_order_with_body(creation_credentials, &pending.merchant_order_id, &body)
        .await;
    let bind = |order_id: String| CreatedOrder {
        order_id,
        ..Default::default()
    };
    let created = match result {
        Ok(created) => created,
        Err(error) => {
            let kind = dujiaopay::error_kind(&error);
            {
                let conn = state.store.conn.lock().await;
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(db_error("begin top-up create failure"))?;
                finish_reconciliation_tx(&tx, &pending.id, pending.had_hint)?;
                record_create_failure_tx(&tx, &pending.id, &pending.channel_id, &error, &now())?;
                tx.commit()
                    .map_err(db_error("commit top-up create failure"))?;
            }
            let attempts: i64 = {
                let conn = state.store.conn.lock().await;
                conn.query_row(
                    "SELECT create_attempts FROM market_topup_orders WHERE id = ?1",
                    params![pending.id],
                    |row| row.get(0),
                )
                .map_err(db_error("read top-up create attempts"))?
            };
            match kind {
                // The merchant order exists with another body (for example a
                // legacy order whose return URL changed). Find and bind it.
                Some(dujiaopay::GatewayErrorKind::Idempotency) => {
                    match find_gateway_order_by_merchant_id(
                        state,
                        creation_credentials,
                        &pending.merchant_order_id,
                        &pending.created_at,
                    )
                    .await
                    {
                        GatewaySearch::Found(order_id) => bind(order_id),
                        _ => {
                            let conn = state.store.conn.lock().await;
                            let tx = conn
                                .transaction_with_behavior(TransactionBehavior::Immediate)
                                .map_err(db_error("begin create conflict review"))?;
                            hold_topup_verification_failure_tx(
                                &tx,
                                &pending.id,
                                "created_order_conflict",
                                &now(),
                            )?;
                            tx.commit()
                                .map_err(db_error("commit create conflict review"))?;
                            return Ok(None);
                        }
                    }
                }
                // An identical replay was rejected by validation. After the
                // second such answer, confirm absence in the order list and
                // close the order locally.
                Some(dujiaopay::GatewayErrorKind::Rejected) if attempts >= 2 => {
                    match find_gateway_order_by_merchant_id(
                        state,
                        creation_credentials,
                        &pending.merchant_order_id,
                        &pending.created_at,
                    )
                    .await
                    {
                        GatewaySearch::Found(order_id) => bind(order_id),
                        GatewaySearch::ProvenAbsent => {
                            let conn = state.store.conn.lock().await;
                            let tx = conn
                                .transaction_with_behavior(TransactionBehavior::Immediate)
                                .map_err(db_error("begin never-created close"))?;
                            close_never_created_tx(&tx, &pending.id, &now())?;
                            tx.commit()
                                .map_err(db_error("commit never-created close"))?;
                            return Ok(None);
                        }
                        GatewaySearch::Inconclusive => return Ok(None),
                    }
                }
                Some(
                    dujiaopay::GatewayErrorKind::Rejected
                    | dujiaopay::GatewayErrorKind::RateLimited
                    | dujiaopay::GatewayErrorKind::Capacity,
                ) => return Ok(None),
                _ => return Err(error),
            }
        }
    };
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin top-up recovery"))?;
    tx.execute(
        "UPDATE market_topup_orders SET create_attempts = create_attempts + 1 WHERE id = ?1",
        params![pending.id],
    )
    .map_err(db_error("count top-up create attempt"))?;
    if let Err(error) = save_created_order_tx(&tx, &pending.id, pending.revision, &created, &now())
    {
        if matches!(error, AppError::Conflict(_)) {
            hold_topup_verification_failure_tx(&tx, &pending.id, "created_order_conflict", &now())?;
            finish_reconciliation_tx(&tx, &pending.id, pending.had_hint)?;
            tx.commit()
                .map_err(db_error("commit conflicting top-up recovery"))?;
            return Ok(None);
        }
        return Err(error);
    }
    tx.commit().map_err(db_error("commit top-up recovery"))?;
    Ok(Some(created.order_id))
}

/// Whether a channel still accepts new orders for this order's audience.
fn channel_accepts_orders_tx(conn: &Connection, channel_id: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT rollout_state != 'off' AND archived_at IS NULL
         FROM market_payment_channels WHERE id = ?1",
        params![channel_id],
        |row| row.get(0),
    )
    .optional()
    .map(|open| open.unwrap_or(false))
    .map_err(db_error("read payment channel availability"))
}

fn hold_paid_review_tx(
    conn: &Connection,
    order_id: &str,
    detail: &str,
    time: &str,
) -> Result<(), AppError> {
    hold_topup_verification_failure_tx(conn, order_id, detail, time)?;
    if matches!(detail, "paid_source_manual" | "paid_amount_short") {
        enqueue_owner_signal_tx(
            conn,
            "topup_review_stale",
            "topup",
            order_id,
            serde_json::json!({ "detail": detail }),
            time,
        )?;
    }
    notify_topup_tx(conn, order_id, "topup_review_required", time)
}

async fn reconcile_one_topup(state: &ServerState, pending: PendingTopup) -> Result<(), AppError> {
    let (loaded, creation_credentials) = {
        let conn = state.store.conn.lock().await;
        let loaded = load_order_revision(
            &conn,
            &state.market_payments,
            &pending.channel_id,
            pending.revision,
        )?;
        let original = load_revision(
            &conn,
            &state.market_payments,
            &pending.channel_id,
            pending.revision,
        )?;
        (loaded, original.credentials)
    };
    let (gateway_id, newly_bound) = if let Some(id) = pending.gateway_order_id.clone() {
        (id, false)
    } else {
        match recover_gateway_order(state, &pending, &loaded, &creation_credentials).await? {
            Some(id) => (id, true),
            None => return Ok(()),
        }
    };
    let lookup_started = now();
    let gateway = get_bound_gateway_order(
        state,
        &pending.channel_id,
        pending.revision,
        &loaded,
        &gateway_id,
    )
    .await?;
    let event = WebhookEvent {
        event_id: format!("reconcile-{}", pending.id),
        event_type: "order.paid".into(),
        order_id: gateway_id,
        merchant_order_id: pending.merchant_order_id.clone(),
        raw_sha256: String::new(),
    };
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin top-up reconciliation"))?;
    if let Err(error) = validate_gateway_identity_tx(
        &tx,
        &pending.id,
        &pending.channel_id,
        pending.revision,
        &loaded,
        &event.order_id,
        &gateway,
    ) {
        if matches!(error, AppError::Conflict(_)) {
            hold_topup_verification_failure_tx(
                &tx,
                &pending.id,
                "gateway_identity_mismatch",
                &now(),
            )?;
            enqueue_owner_signal_tx(
                &tx,
                "gateway_identity_conflict",
                "topup",
                &pending.id,
                serde_json::json!({ "orderId": pending.id }),
                &now(),
            )?;
            finish_reconciliation_tx(&tx, &pending.id, pending.had_hint)?;
            tx.commit()
                .map_err(db_error("commit mismatched gateway review"))?;
            return Ok(());
        }
        return Err(error);
    }
    record_gateway_observation_tx(&tx, &pending.id, "reconcile", &gateway, &now())?;
    record_gateway_terms_tx(&tx, &pending.id, &gateway)?;
    tx.execute(
        "UPDATE market_topup_orders SET reconcile_failures = 0 WHERE id = ?1",
        params![pending.id],
    )
    .map_err(db_error("reset top-up reconciliation failures"))?;
    restore_verified_checkout_tx(&tx, &pending.id, &gateway)?;
    let status_before: String = tx
        .query_row(
            "SELECT status FROM market_topup_orders WHERE id = ?1",
            params![pending.id],
            |row| row.get(0),
        )
        .map_err(db_error("read top-up status before reconciliation"))?;
    let manual_hold = topup_has_manual_hold_tx(&tx, &pending.id)?;
    if gateway.status == "paid"
        && (!manual_hold || compatible_paid_hold_ids_tx(&tx, &pending.id)?.is_some())
    {
        match apply_paid_order_tx(
            &tx,
            &pending.id,
            &pending.channel_id,
            pending.revision,
            &loaded,
            &event,
            &gateway,
        ) {
            Ok(()) => {}
            Err(error) => match paid_review_detail(&error) {
                Some(detail) => hold_paid_review_tx(&tx, &pending.id, detail, &now())?,
                None => return Err(error),
            },
        }
    } else if gateway.status == "paid" {
        tx.execute(
            "UPDATE market_topup_orders SET status = 'review_required', updated_at = ?2
             WHERE id = ?1 AND status != 'credited'",
            params![pending.id, now()],
        )
        .map_err(db_error("hold externally disputed top-up"))?;
    } else {
        match verified_unpaid_status(&gateway.status) {
            Ok(status)
                if matches!(status, "expired" | "cancelled")
                    && !gateway_terminal_without_payment_evidence(&gateway) =>
            {
                hold_topup_verification_failure_tx(
                    &tx,
                    &pending.id,
                    "gateway_terminal_payment_evidence",
                    &now(),
                )?;
            }
            Ok(status) => {
                if !recover_benign_chain_metadata_review_tx(
                    &tx,
                    &pending.id,
                    &gateway,
                    status,
                    &now(),
                )? {
                    apply_verified_unpaid_status_tx(&tx, &pending.id, status, &now())?;
                }
            }
            Err(AppError::Conflict(_)) => {
                hold_topup_verification_failure_tx(
                    &tx,
                    &pending.id,
                    "gateway_status_unknown",
                    &now(),
                )?;
            }
            Err(error) => return Err(error),
        }
    }
    // Every signed event received before this authenticated lookup is now
    // accounted for. A pending event no longer blocks recovery or checkout.
    tx.execute(
        "UPDATE market_topup_events
         SET status = 'processed', processed_at = ?2, processed_by = 'reconcile'
         WHERE order_id = ?1 AND status = 'pending' AND created_at <= ?3",
        params![pending.id, now(), lookup_started],
    )
    .map_err(db_error("close events covered by lookup"))?;
    finish_reconciliation_tx(&tx, &pending.id, pending.had_hint)?;
    let current_status: String = tx
        .query_row(
            "SELECT status FROM market_topup_orders WHERE id = ?1",
            params![pending.id],
            |row| row.get(0),
        )
        .map_err(db_error("read reconciled buyer top-up status"))?;
    if current_status != status_before && matches!(current_status.as_str(), "expired" | "cancelled")
    {
        notify_topup_tx(&tx, &pending.id, "topup_closed_unpaid", &now())?;
    }
    finish_buyer_recheck_tx(&tx, &pending.id, Some(&current_status))?;
    // A gateway order first bound now for a stopped order, or on a channel
    // that no longer accepts orders, was never shown to the buyer. Cancel it
    // at once rather than leave a fresh payable order behind.
    let suppress = newly_bound
        && gateway.status == "pending"
        && (pending.stopped || !channel_accepts_orders_tx(&tx, &pending.channel_id)?)
        && !gateway_cancel_was_requested_tx(&tx, &pending.id)?;
    let can_auto_close = suppress || should_auto_cancel_tx(&tx, &pending, &gateway, Utc::now())?;
    tx.commit()
        .map_err(db_error("commit top-up reconciliation"))?;
    drop(conn);
    if can_auto_close {
        // The local deadline only triggers a verified gateway cancellation.
        // It is not itself proof that the checkout has stopped accepting funds.
        cancel_topup_for_actor(state, &pending.id, "system", Some(&pending.user_id)).await?;
    }
    Ok(())
}

/// Count consecutive reconciliation failures and alert the Owner once an
/// order has failed five times in a row.
fn note_reconcile_failure_tx(
    conn: &Connection,
    order_id: &str,
    code: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE market_topup_orders SET reconcile_failures = reconcile_failures + 1 WHERE id = ?1",
        params![order_id],
    )
    .map_err(db_error("count top-up reconciliation failure"))?;
    let failures: i64 = conn
        .query_row(
            "SELECT reconcile_failures FROM market_topup_orders WHERE id = ?1",
            params![order_id],
            |row| row.get(0),
        )
        .map_err(db_error("read top-up reconciliation failures"))?;
    if failures == 5 {
        enqueue_owner_signal_tx(
            conn,
            "topup_reconcile_failing",
            "topup",
            order_id,
            serde_json::json!({ "failures": failures, "errorCode": code }),
            &now(),
        )?;
    }
    Ok(())
}

pub async fn run_service(
    state: ServerState,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    if let Err(error) = verify_channel_secrets_on_start(&state).await {
        tracing::error!(error = %error, "payment channel secret self-check failed");
    }
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut audit_interval = tokio::time::interval(std::time::Duration::from_secs(3_600));
    audit_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Sweeps page through the gateway with deliberate pauses. They run on
    // their own task so that a long sweep never delays top-up reconciliation
    // or a webhook wakeup.
    let sweep_task = tokio::spawn({
        let state = state.clone();
        let mut shutdown = shutdown.clone();
        async move {
            let mut sweep_interval = tokio::time::interval(std::time::Duration::from_secs(600));
            sweep_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = sweep_interval.tick() => sweep::run_due_sweeps(&state).await,
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break; }
                    }
                }
            }
        }
    });
    let c2c_task = tokio::spawn(crate::market_c2c::run_housekeeping(
        state.store.clone(),
        shutdown.clone(),
    ));
    // Income settlement must not wait behind slow or unavailable payment
    // gateways. This task only performs bounded local ledger transactions.
    let earnings_task = tokio::spawn({
        let state = state.clone();
        let mut shutdown = shutdown.clone();
        async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => unlock_due_earnings(&state).await,
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break; }
                    }
                }
            }
        }
    });
    let wakeup = state.market_payments.reconcile_wakeup.clone();
    loop {
        let reconcile_now = tokio::select! {
            _ = audit_interval.tick() => {
                run_finance_audit(&state).await;
                false
            }
            _ = interval.tick() => true,
            _ = wakeup.notified() => true,
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
                false
            }
        };
        if !reconcile_now {
            continue;
        }
        let pending = {
            let conn = state.store.conn.lock().await;
            pending_topups_tx(&conn)
        };
        match pending {
            Ok(orders) => {
                for order in orders {
                    let order_id = order.id.clone();
                    let claimed = {
                        let conn = state.store.conn.lock().await;
                        claim_reconciliation_tx(&conn, &order.id)
                    };
                    if !matches!(claimed, Ok(true)) {
                        if claimed.is_err() {
                            tracing::warn!("market top-up reconciliation claim failed");
                        }
                        continue;
                    }
                    if let Err(error) = reconcile_one_topup(&state, order).await {
                        let code = error.code().unwrap_or("MARKET_TOPUP_RETRY");
                        let result = {
                            let conn = state.store.conn.lock().await;
                            finish_buyer_recheck_tx(&conn, &order_id, None)
                                .and_then(|()| note_reconcile_failure_tx(&conn, &order_id, code))
                        };
                        if result.is_err() {
                            tracing::warn!("buyer top-up recheck status unavailable");
                        }
                        tracing::warn!(error_code = code, "market top-up recovery deferred");
                    }
                }
            }
            Err(_) => tracing::warn!("market top-up queue unavailable"),
        }
    }
    // A sweep page in flight finishes its own transaction; anything left is
    // resumed from its saved offset on the next start.
    sweep_task.abort();
    let _ = sweep_task.await;
    c2c_task.abort();
    let _ = c2c_task.await;
    earnings_task.abort();
    let _ = earnings_task.await;
    Ok(())
}

async fn run_finance_audit(state: &ServerState) {
    let result = {
        let conn = state.store.conn.lock().await;
        conn.transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin market finance audit"))
            .and_then(|tx| {
                crate::market_wallet_transfers::prune_unused_tx(&tx, Utc::now())?;
                let findings = crate::market_reconcile::audit_finance_tx(&tx, &now())?;
                if findings > 0 {
                    enqueue_owner_signal_tx(
                        &tx,
                        "finance_audit_mismatch",
                        "finance_control",
                        "1",
                        serde_json::json!({ "findings": findings }),
                        &now(),
                    )?;
                }
                tx.commit()
                    .map_err(db_error("commit market finance audit"))?;
                Ok(findings)
            })
    };
    match result {
        Ok(findings) if findings > 0 => {
            tracing::error!(findings, "market finance audit found mismatches")
        }
        Err(_) => tracing::error!("market finance audit unavailable"),
        _ => {}
    }
    let stale_reviews = {
        let conn = state.store.conn.lock().await;
        stale_topup_review_count_tx(&conn).and_then(|count| {
            if count > 0 {
                enqueue_owner_signal_tx(
                    &conn,
                    "topup_review_stale",
                    "finance_control",
                    "topups",
                    serde_json::json!({ "count": count }),
                    &now(),
                )?;
            }
            Ok(count)
        })
    };
    match stale_reviews {
        Ok(count) if count > 0 => {
            tracing::warn!(count, "market top-ups need owner review for over 24 hours")
        }
        Err(_) => tracing::warn!("market top-up review age check unavailable"),
        _ => {}
    }
}

/// Mature Provider earnings. Each lot settles in its own savepoint so one bad
/// lot cannot hold back the rest of the batch.
async fn unlock_due_earnings(state: &ServerState) {
    let conn = state.store.conn.lock().await;
    let result = crate::market_finance::unlock_due_earnings_isolated(&conn, &now(), 200);
    if result.is_err() {
        tracing::warn!("market earning maturity deferred");
    }
    if crate::market_earning_conversions::credit_due_earnings_isolated(&conn, &now(), 200).is_err()
    {
        tracing::warn!("market automatic earnings credit deferred");
    }
}

/// Decrypt every current credential revision at startup. A channel whose
/// secrets cannot be read is switched off and the Owner is alerted, instead of
/// failing later inside reconciliation or a buyer request.
async fn verify_channel_secrets_on_start(state: &ServerState) -> Result<(), AppError> {
    let conn = state.store.conn.lock().await;
    let channels = {
        let mut statement = conn
            .prepare(
                "SELECT id, current_revision FROM market_payment_channels
                 WHERE archived_at IS NULL",
            )
            .map_err(db_error("prepare channel secret check"))?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(db_error("query channel secret check"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read channel secret check"))?
    };
    for (channel_id, revision) in channels {
        if load_revision(&conn, &state.market_payments, &channel_id, revision).is_ok() {
            continue;
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin unreadable channel"))?;
        let time = now();
        tx.execute(
            "UPDATE market_payment_channels SET rollout_state = 'off', updated_at = ?2,
                    version = version + 1
             WHERE id = ?1 AND rollout_state != 'off'",
            params![channel_id, time],
        )
        .map_err(db_error("disable unreadable channel"))?;
        enqueue_owner_signal_tx(
            &tx,
            "payment_secret_unreadable",
            "payment_channel",
            &channel_id,
            serde_json::json!({ "revision": revision }),
            &time,
        )?;
        tx.commit().map_err(db_error("commit unreadable channel"))?;
        tracing::error!(
            channel_id,
            revision,
            "payment channel secrets cannot be decrypted"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_wallet::{reconcile_wallet_tx, wallet_snapshot_tx};

    fn memory() -> Connection {
        crate::test_support::database().expect("copy finance schema")
    }

    #[test]
    fn fee_quotes_freeze_gross_principal_cost_and_configuration_versions() {
        let conn = memory();
        fixture_revision(&conn, 1);
        conn.execute(
            "UPDATE market_finance_control SET mode = 'shadow' WHERE id = 1",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE market_payment_channels SET rollout_state = 'canary' WHERE id = 'channel'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE market_payment_channel_settings SET fee_rate_bps = 200, fixed_fee_minor = 3 WHERE channel_id = 'channel'", []).unwrap();
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-22T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let quote = quotes::quote_tx(
            &conn,
            "buyer",
            true,
            quotes::QuoteRequest {
                channel_id: "channel".into(),
                amount_minor: 1000,
                token_id: None,
            },
            at,
        )
        .unwrap();
        assert_eq!(
            (
                quote.credit_minor,
                quote.fee_minor,
                quote.pay_minor,
                quote.gateway_cost_minor
            ),
            (1000, 50, 1050, 24)
        );
        assert!(quotes::read_quote_tx(&conn, "other", &quote.id).is_err());
        assert!(
            conn.execute(
                "UPDATE market_topup_quotes SET fee_minor = 0 WHERE id = ?1",
                params![quote.id]
            )
            .is_err()
        );
        let channel = topup_channel_tx(&conn, "channel", true).unwrap();
        quotes::validate_current_tx(&conn, &quote, &channel, "2026-10-22T12:04:59+00:00").unwrap();
        assert!(
            quotes::validate_current_tx(&conn, &quote, &channel, "2026-10-22T12:05:00+00:00")
                .is_err()
        );
        let mut changed = topup_channel_tx(&conn, "channel", true).unwrap();
        changed.settings_version += 1;
        assert!(
            quotes::validate_current_tx(&conn, &quote, &changed, "2026-10-22T12:01:00+00:00")
                .is_err()
        );
        changed = topup_channel_tx(&conn, "channel", true).unwrap();
        changed.revision += 1;
        assert!(
            quotes::validate_current_tx(&conn, &quote, &changed, "2026-10-22T12:01:00+00:00")
                .is_err()
        );
        conn.execute(
            "UPDATE market_wallet_policy SET version = version + 1, maintenance_fee_bps = 0",
            [],
        )
        .unwrap();
        assert!(
            quotes::validate_current_tx(&conn, &quote, &channel, "2026-10-22T12:01:00+00:00")
                .is_err()
        );
        // A retry still reads the immutable old quote, not today's fee.
        let old = quotes::read_quote_tx(&conn, "buyer", &quote.id).unwrap();
        assert_eq!(
            (old.credit_minor, old.fee_minor, old.pay_minor),
            (1000, 50, 1050)
        );
    }

    #[test]
    fn principal_channel_limits_survive_fee_changes_without_creating_orders() {
        let conn = memory();
        fixture_revision(&conn, 1);
        conn.execute(
            "UPDATE market_finance_control SET mode = 'shadow' WHERE id = 1",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE market_payment_channels SET rollout_state = 'canary' WHERE id = 'channel'",
            [],
        )
        .unwrap();
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let channels = buyer_channel_views(&conn, true).unwrap();
        assert_eq!(channels[0].amount_limits.min_credit_minor, Some(100));
        assert_eq!(channels[0].amount_limits.max_credit_minor, Some(100_000));
        let quote = quotes::quote_tx(
            &conn,
            "buyer",
            true,
            quotes::QuoteRequest {
                channel_id: "channel".into(),
                amount_minor: 100_000,
                token_id: None,
            },
            at,
        )
        .unwrap();
        assert_eq!(
            (quote.credit_minor, quote.fee_minor, quote.pay_minor),
            (100_000, 5_000, 105_000)
        );
        for amount in [100, 95_239, 96_000] {
            let valid = quotes::quote_tx(
                &conn,
                "buyer",
                true,
                quotes::QuoteRequest {
                    channel_id: "channel".into(),
                    amount_minor: amount,
                    token_id: None,
                },
                at,
            )
            .unwrap();
            assert_eq!(valid.credit_minor, amount);
            assert_eq!(valid.pay_minor, amount + valid.fee_minor);
        }
        for amount in [99, 100_001, -1, i64::MAX] {
            let error = quotes::quote_tx(
                &conn,
                "buyer",
                true,
                quotes::QuoteRequest {
                    channel_id: "channel".into(),
                    amount_minor: amount,
                    token_id: None,
                },
                at,
            )
            .err()
            .unwrap();
            let AppError::Coded { code, details, .. } = error else {
                panic!("expected coded amount error")
            };
            assert_eq!(code, "MARKET_TOPUP_AMOUNT_OUT_OF_RANGE");
            assert_eq!(details["maxCreditMinor"], 100_000);
            assert_eq!(details["maxTopupMinor"], 100_000);
            assert_eq!(details["maintenanceFeeBps"], 500);
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM market_topup_quotes", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM market_topup_orders", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();

        // A dynamic policy edit changes the fee, never the principal range.
        // Old quotes stay frozen but cannot create new orders at today's fee.
        conn.execute(
            "UPDATE market_wallet_policy SET maintenance_fee_bps = 725, version = version + 1",
            [],
        )
        .unwrap();
        let channels = buyer_channel_views(&conn, true).unwrap();
        assert_eq!(channels[0].amount_limits.max_credit_minor, Some(100_000));
        assert_eq!(channels[0].amount_limits.maintenance_fee_bps, 725);
        assert_eq!(channels[0].amount_limits.policy_version, 2);
        assert!(
            quotes::validate_current_tx(
                &conn,
                &quote,
                &topup_channel_tx(&conn, "channel", true).unwrap(),
                "2026-10-25T12:01:00+00:00"
            )
            .is_err()
        );
        let old = quotes::read_quote_tx(&conn, "buyer", &quote.id).unwrap();
        assert_eq!(
            (old.credit_minor, old.fee_minor, old.pay_minor),
            (100_000, 5_000, 105_000)
        );
        let current = quotes::quote_tx(
            &conn,
            "buyer",
            true,
            quotes::QuoteRequest {
                channel_id: "channel".into(),
                amount_minor: 100_000,
                token_id: None,
            },
            at,
        )
        .unwrap();
        assert_eq!(
            (
                current.credit_minor,
                current.fee_bps,
                current.fee_minor,
                current.pay_minor
            ),
            (100_000, 725, 7_250, 107_250)
        );

        // Even an exact $1 principal range remains payable with a nonzero fee.
        conn.execute("INSERT INTO market_payment_channel_settings (channel_id, max_topup_minor, updated_at) VALUES ('channel', 100, '2026-10-25')
            ON CONFLICT (channel_id) DO UPDATE SET max_topup_minor = 100, version = market_payment_channel_settings.version + 1", []).unwrap();
        let channels = buyer_channel_views(&conn, true).unwrap();
        assert_eq!(channels[0].amount_limits.min_credit_minor, Some(100));
        assert_eq!(channels[0].amount_limits.max_credit_minor, Some(100));
        let minimum = quotes::quote_tx(
            &conn,
            "buyer",
            true,
            quotes::QuoteRequest {
                channel_id: "channel".into(),
                amount_minor: 100,
                token_id: None,
            },
            at,
        )
        .unwrap();
        assert_eq!(
            (minimum.credit_minor, minimum.fee_minor, minimum.pay_minor),
            (100, 7, 107)
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM market_topup_quotes", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            6
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM market_topup_orders", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn maximum_fee_bearing_paid_topup_credits_only_frozen_principal_and_passes_live_evidence() {
        let conn = memory();
        fixture_revision(&conn, 1);
        conn.execute("INSERT INTO market_topup_orders (id, user_id, channel_id, channel_revision, merchant_fingerprint,
          merchant_order_id, idempotency_key, request_fingerprint, amount_minor, credit_amount_minor,
          platform_fee_minor, platform_fee_rate_bps, status, created_at, updated_at)
          VALUES ('fee-order', 'buyer', 'channel', 1, 'merchant-fingerprint',
          'router-00000000000000000000000000000001', 'fee-idem', 'fee-fp', 105000, 100000, 5000, 500,
          'pending', '2026-10-01', '2026-10-01')", []).unwrap();
        // The Owner may change today's rate before this frozen order is paid.
        conn.execute(
            "UPDATE market_wallet_policy SET maintenance_fee_bps = 725, version = version + 1",
            [],
        )
        .unwrap();
        let mut gateway = paid_order("router-00000000000000000000000000000001", "fee-tx");
        gateway.fiat_amount_minor = 105000;
        gateway.settled_amount = Some("1050.00".into());
        apply_paid_order_tx(
            &conn,
            "fee-order",
            "channel",
            1,
            &loaded(),
            &event(&gateway),
            &gateway,
        )
        .unwrap();
        assert_eq!(
            wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units,
            100_000 * crate::market_billing::MONEY_UNITS_PER_MINOR
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_verified_topup_evidence WHERE id = 'fee-order'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(conn.query_row("SELECT -SUM(signed_units) FROM market_journal_postings WHERE account_kind = 'platform_fee_revenue'", [], |r| r.get::<_, i64>(0)).unwrap(), 5_000 * crate::market_billing::MONEY_UNITS_PER_MINOR);
        assert_eq!(
            crate::market_reconcile::audit_finance_tx(&conn, "2026-10-22T12:00:00Z").unwrap(),
            0
        );
        apply_paid_order_tx(
            &conn,
            "fee-order",
            "channel",
            1,
            &loaded(),
            &event(&gateway),
            &gateway,
        )
        .unwrap();
        assert_eq!(
            wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units,
            100_000 * crate::market_billing::MONEY_UNITS_PER_MINOR
        );
        assert!(
            conn.execute(
                "UPDATE market_topup_orders SET platform_fee_minor = 0 WHERE id = 'fee-order'",
                []
            )
            .is_err()
        );
    }

    #[test]
    fn auto_close_waits_for_both_router_and_known_gateway_deadlines() {
        let time = chrono::DateTime::parse_from_rfc3339("2026-10-20T00:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(!auto_close_due(None, None, time));
        assert!(!auto_close_due(Some("invalid"), None, time));
        assert!(!auto_close_due(Some("2026-10-20T00:30:01Z"), None, time));
        assert!(auto_close_due(Some("2026-10-20T00:30:00Z"), None, time));
        assert!(!auto_close_due(
            Some("2026-10-20T00:30:00Z"),
            Some("2026-10-20T01:00:00Z"),
            time
        ));
        assert!(!auto_close_due(
            Some("2026-10-20T00:30:00Z"),
            Some("invalid"),
            time
        ));
        assert!(auto_close_due(
            Some("2026-10-20T00:30:00Z"),
            Some("2026-10-20T00:29:59Z"),
            time
        ));
    }

    #[test]
    fn old_topups_have_no_auto_deadline_and_new_deadline_is_exactly_thirty_minutes() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "old-order",
            "router-00000000000000000000000000000001",
            "pending",
            1,
        );
        assert!(
            read_topup(&conn, "old-order", "buyer")
                .unwrap()
                .auto_close_at
                .is_none()
        );
        let created_at = chrono::DateTime::parse_from_rfc3339("2026-10-20T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let deadline = chrono::DateTime::parse_from_rfc3339(&topup_auto_close_deadline(created_at))
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            deadline.signed_duration_since(created_at),
            chrono::Duration::minutes(30)
        );
        conn.execute(
            "UPDATE market_topup_orders SET auto_close_at = ?2 WHERE id = ?1",
            params!["old-order", deadline.to_rfc3339()],
        )
        .unwrap();
        assert_eq!(
            read_topup(&conn, "old-order", "buyer")
                .unwrap()
                .auto_close_at
                .as_deref(),
            Some("2026-10-20T00:30:00+00:00")
        );
    }

    #[test]
    fn buyer_abandon_hides_checkout_without_closing_gateway_order() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        conn.execute(
            "UPDATE market_topup_orders
             SET gateway_order_id = 'gateway-1',
                 checkout_url = 'https://www.dujiaopay.com/checkout/ct_one'
             WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        let snapshot = cancel_topup_snapshot_tx(&conn, "order-1", Some("buyer")).unwrap();
        let gateway = unpaid_order(merchant_order_id, "awaiting_payment");
        let view =
            mark_buyer_abandoned_tx(&conn, "order-1", "buyer", &snapshot, &loaded(), &gateway)
                .unwrap();
        assert_eq!(view.status, "pending");
        assert!(view.buyer_abandoned_at.is_some());
        assert!(view.checkout_url.is_none());
        assert!(
            topups_tx(&conn, "buyer", None).unwrap().items[0]
                .checkout_url
                .is_none()
        );
        assert!(
            active_matching_topup_tx(&conn, "buyer", "channel:100")
                .unwrap()
                .is_none()
        );
        ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();
        let stored: (String, String, String) = conn
            .query_row(
                "SELECT status, gateway_order_id, checkout_url FROM market_topup_orders WHERE id = 'order-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored.0, "pending");
        assert_eq!(stored.1, "gateway-1");
        assert!(stored.2.ends_with("/ct_one"));
        let repeated =
            mark_buyer_abandoned_tx(&conn, "order-1", "buyer", &snapshot, &loaded(), &gateway)
                .unwrap();
        assert_eq!(repeated.buyer_abandoned_at, view.buyer_abandoned_at);
        let audits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_finance_audit_events
                 WHERE object_id = 'order-1' AND action = 'topup_buyer_abandoned'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audits, 1);
        assert!(matches!(
            mark_buyer_abandoned_tx(&conn, "order-1", "other", &snapshot, &loaded(), &gateway),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn buyer_abandon_rejects_gateway_progress_or_payment_evidence() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        let snapshot = cancel_topup_snapshot_tx(&conn, "order-1", Some("buyer")).unwrap();
        for status in ["pending", "confirming", "paid", "expired"] {
            let gateway = unpaid_order(merchant_order_id, status);
            assert!(matches!(
                mark_buyer_abandoned_tx(&conn, "order-1", "buyer", &snapshot, &loaded(), &gateway),
                Err(AppError::Conflict(_))
            ));
        }
        let mut gateway = unpaid_order(merchant_order_id, "awaiting_payment");
        gateway.tx_id = Some("possible-transfer".into());
        assert!(matches!(
            mark_buyer_abandoned_tx(&conn, "order-1", "buyer", &snapshot, &loaded(), &gateway),
            Err(AppError::Conflict(_))
        ));
        gateway.tx_id = None;
        gateway.fiat_amount_minor = 200;
        assert!(matches!(
            mark_buyer_abandoned_tx(&conn, "order-1", "buyer", &snapshot, &loaded(), &gateway),
            Err(AppError::Conflict(_))
        ));
        assert!(!topup_buyer_abandoned_tx(&conn, "order-1").unwrap());
    }

    #[test]
    fn abandoned_orders_have_a_separate_bounded_capacity_and_remain_reconciled() {
        let conn = memory();
        fixture_revision(&conn, 1);
        for index in 0..MAX_ABANDONED_ORDERS {
            fixture_topup(
                &conn,
                &format!("abandoned-{index}"),
                &format!("router-abandoned-{index:032}"),
                "pending",
                1,
            );
            conn.execute(
                "UPDATE market_topup_orders
                 SET buyer_abandoned_at = '2025-01-01', created_at = ?2,
                     updated_at = ?2, next_reconcile_at = '2025-01-01'
                 WHERE id = ?1",
                params![format!("abandoned-{index}"), now()],
            )
            .unwrap();
        }
        assert!(matches!(
            ensure_pending_topup_capacity_tx(&conn, "buyer"),
            Err(AppError::Coded {
                code: "MARKET_TOPUP_ABANDONED_LIMIT",
                ..
            })
        ));
        assert_eq!(pending_topups_tx(&conn).unwrap().len(), 3);
        let owner_rows = admin_topups_tx(&conn, None).unwrap().items;
        assert_eq!(owner_rows.len(), 3);
        assert!(
            owner_rows
                .iter()
                .all(|order| order.buyer_abandoned_at.is_some())
        );
        conn.execute(
            "UPDATE market_topup_orders SET created_at = '2025-01-01'
             WHERE id = 'abandoned-0'",
            [],
        )
        .unwrap();
        ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();
        conn.execute(
            "UPDATE market_topup_orders SET status = 'expired' WHERE id = 'abandoned-0'",
            [],
        )
        .unwrap();
        ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();
        for index in 0..MAX_PENDING_ORDERS {
            fixture_topup(
                &conn,
                &format!("active-{index}"),
                &format!("router-active-{index:032}"),
                "pending",
                1,
            );
        }
        assert!(matches!(
            ensure_pending_topup_capacity_tx(&conn, "buyer"),
            Err(AppError::Coded {
                code: "MARKET_TOPUP_PENDING_LIMIT",
                ..
            })
        ));
    }

    #[test]
    fn automatic_cancel_candidate_requires_safe_pending_and_no_earlier_post() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        let time = chrono::DateTime::parse_from_rfc3339("2026-10-20T00:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let pending = PendingTopup {
            id: "order-1".into(),
            user_id: "buyer".into(),
            channel_id: "channel".into(),
            revision: 1,
            merchant_order_id: merchant_order_id.into(),
            gateway_order_id: Some("gateway-1".into()),
            amount_minor: 100,
            had_hint: false,
            auto_close_at: Some("2026-10-20T00:30:00Z".into()),
            created_at: "2026-10-20T00:00:00Z".into(),
            stopped: false,
        };
        let mut gateway = unpaid_order(merchant_order_id, "pending");
        assert!(should_auto_cancel_tx(&conn, &pending, &gateway, time).unwrap());
        gateway.expires_at = Some("2026-10-20T00:31:00Z".into());
        assert!(!should_auto_cancel_tx(&conn, &pending, &gateway, time).unwrap());
        gateway.expires_at = None;
        gateway.tx_id = Some("possible-transfer".into());
        assert!(!should_auto_cancel_tx(&conn, &pending, &gateway, time).unwrap());
        gateway.tx_id = None;
        gateway.status = "awaiting_payment".into();
        assert!(!should_auto_cancel_tx(&conn, &pending, &gateway, time).unwrap());
        gateway.status = "pending".into();
        mark_gateway_cancel_requested_tx(&conn, "system", "order-1", &now()).unwrap();
        assert!(!should_auto_cancel_tx(&conn, &pending, &gateway, time).unwrap());
        assert!(gateway_cancel_was_requested_tx(&conn, "order-1").unwrap());
    }

    #[test]
    fn buyer_history_marks_review_with_payment_evidence_without_exposing_case_details() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            "router-00000000000000000000000000000001",
            "review_required",
            1,
        );
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().review_reason,
            None
        );
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_terminal_payment_evidence"),
            &now(),
        )
        .unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().review_reason,
            Some("terminal_payment_marker".into())
        );
        assert_eq!(
            topups_tx(&conn, "buyer", None).unwrap().items[0].review_reason,
            Some("terminal_payment_marker".into())
        );
        assert!(read_topup(&conn, "order-1", "other").is_err());
        let case_id: String = conn
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases
                 WHERE object_kind = 'topup' AND object_id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::market_reconcile::resolve_case_tx(
            &conn,
            &case_id,
            "owner",
            "test_resolution",
            &now(),
        )
        .unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().review_reason,
            None
        );
        let clean_terminal = unpaid_order("router-00000000000000000000000000000001", "expired");
        let assessment =
            recovery_assessment_tx(&conn, "order-1", "review_required", &clean_terminal).unwrap();
        assert!(!assessment.can_recover());
    }

    #[test]
    fn buyer_recheck_is_owned_cooldown_limited_and_never_reopens_a_final_order() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            "router-00000000000000000000000000000001",
            "review_required",
            1,
        );
        assert!(matches!(
            queue_buyer_recheck_tx(&conn, "order-1", "other"),
            Err(AppError::NotFound(_))
        ));
        assert!(queue_buyer_recheck_tx(&conn, "order-1", "buyer").unwrap());
        let queued = read_topup(&conn, "order-1", "buyer").unwrap();
        assert_eq!(queued.buyer_recheck_result.as_deref(), Some("queued"));
        assert!(queued.buyer_recheck_completed_at.is_none());
        assert!(!queue_buyer_recheck_tx(&conn, "order-1", "buyer").unwrap());
        finish_buyer_recheck_tx(&conn, "order-1", Some("review_required")).unwrap();
        let completed = read_topup(&conn, "order-1", "buyer").unwrap();
        assert_eq!(
            completed.buyer_recheck_result.as_deref(),
            Some("review_required")
        );
        assert!(completed.buyer_recheck_completed_at.is_some());
        conn.execute(
            "UPDATE market_topup_orders SET buyer_recheck_result = 'queued'
             WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        finish_buyer_recheck_tx(&conn, "order-1", None).unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer")
                .unwrap()
                .buyer_recheck_result
                .as_deref(),
            Some("retrying")
        );
        conn.execute(
            "UPDATE market_topup_orders SET status = 'expired' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        assert!(matches!(
            queue_buyer_recheck_tx(&conn, "order-1", "buyer"),
            Err(AppError::Conflict(_))
        ));
    }

    #[test]
    fn buyer_can_stop_payment_without_erasing_reconciliation_or_another_buyers_order() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1',
             checkout_url = 'https://www.dujiaopay.com/checkout/ct_one',
             created_at = ?1 WHERE id = 'order-1'",
            params![now()],
        )
        .unwrap();
        assert!(matches!(
            stop_buyer_topup_tx(&conn, "order-1", "other"),
            Err(AppError::NotFound(_))
        ));
        let stopped = stop_buyer_topup_tx(&conn, "order-1", "buyer").unwrap();
        assert_eq!(stopped.status, "pending");
        assert!(stopped.checkout_url.is_none());
        assert!(stopped.buyer_abandoned_at.is_some());
        assert!(
            active_matching_topup_tx(&conn, "buyer", "channel:100")
                .unwrap()
                .is_none()
        );
        let repeated = stop_buyer_topup_tx(&conn, "order-1", "buyer").unwrap();
        assert_eq!(repeated.buyer_abandoned_at, stopped.buyer_abandoned_at);
        let audits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_finance_audit_events
             WHERE object_id = 'order-1' AND action = 'topup_buyer_stopped_payment'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audits, 1);
        assert!(
            pending_topups_tx(&conn)
                .unwrap()
                .iter()
                .any(|order| order.id == "order-1")
        );
        let mut gateway = unpaid_order(merchant_order_id, "pending");
        gateway.checkout_url = Some("https://www.dujiaopay.com/checkout/ct_old".into());
        restore_verified_checkout_tx(&conn, "order-1", &gateway).unwrap();
        assert!(
            read_topup(&conn, "order-1", "buyer")
                .unwrap()
                .checkout_url
                .is_none()
        );
        let stored_checkout: Option<String> = conn
            .query_row(
                "SELECT checkout_url FROM market_topup_orders WHERE id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // Stopping hides the checkout but keeps the stored link, and a later
        // gateway lookup cannot swap a stopped order's link either.
        assert_eq!(
            stored_checkout.as_deref(),
            Some("https://www.dujiaopay.com/checkout/ct_one")
        );
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
    }

    #[test]
    fn general_case_resolution_rejects_topup_holds() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            "router-00000000000000000000000000000001",
            "review_required",
            1,
        );
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_terminal_payment_evidence"),
            &now(),
        )
        .unwrap();
        let case_id: String = conn
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases WHERE object_id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(matches!(
            ensure_general_case_can_resolve_tx(&conn, &case_id),
            Err(AppError::Conflict(_))
        ));
    }

    #[test]
    fn stale_review_alert_uses_case_age_not_original_order_age() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            "router-00000000000000000000000000000001",
            "review_required",
            1,
        );
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_terminal_payment_evidence"),
            &now(),
        )
        .unwrap();
        assert_eq!(stale_topup_review_count_tx(&conn).unwrap(), 0);
        conn.execute(
            "UPDATE market_finance_reconciliation_cases SET created_at = '2025-01-01'
             WHERE object_id = 'order-1'",
            [],
        )
        .unwrap();
        assert_eq!(stale_topup_review_count_tx(&conn).unwrap(), 1);
    }

    #[test]
    fn strict_late_payment_credits_a_terminal_marker_review_once() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_terminal_payment_evidence"),
            &now(),
        )
        .unwrap();
        assert!(queue_buyer_recheck_tx(&conn, "order-1", "buyer").unwrap());
        stop_buyer_topup_tx(&conn, "order-1", "buyer").unwrap();
        let gateway = paid_order(merchant_order_id, "verified-late-transaction");
        let paid_event = event(&gateway);
        apply_paid_order_tx(
            &conn,
            "order-1",
            "channel",
            1,
            &loaded(),
            &paid_event,
            &gateway,
        )
        .unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "credited"
        );
        assert_eq!(
            read_topup(&conn, "order-1", "buyer")
                .unwrap()
                .buyer_recheck_result
                .as_deref(),
            Some("credited")
        );
        let first_balance = wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units;
        assert!(first_balance > 0);
        apply_paid_order_tx(
            &conn,
            "order-1",
            "channel",
            1,
            &loaded(),
            &paid_event,
            &gateway,
        )
        .unwrap();
        assert_eq!(
            wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units,
            first_balance
        );
        let cases: (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), SUM(status = 'resolved') FROM market_finance_reconciliation_cases
             WHERE object_kind = 'topup' AND object_id = 'order-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cases, (1, 1));
    }

    #[test]
    fn unrelated_review_or_invalid_paid_receipt_never_auto_credits() {
        for extra_detail in ["paid_receipt_invalid", "gateway_identity_mismatch"] {
            let conn = memory();
            fixture_revision(&conn, 1);
            let merchant_order_id = "router-00000000000000000000000000000001";
            fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
            open_reconciliation_case_detail_tx(
                &conn,
                "topup_verification_failed",
                "topup",
                "order-1",
                Some("gateway_terminal_payment_evidence"),
                &now(),
            )
            .unwrap();
            open_reconciliation_case_detail_tx(
                &conn,
                "unknown_gateway_event",
                "topup",
                "order-1",
                Some(extra_detail),
                &now(),
            )
            .unwrap();
            let gateway = paid_order(merchant_order_id, "verified-late-transaction");
            let paid_event = event(&gateway);
            assert!(matches!(
                apply_paid_order_tx(
                    &conn,
                    "order-1",
                    "channel",
                    1,
                    &loaded(),
                    &paid_event,
                    &gateway
                ),
                Err(AppError::Conflict(_))
            ));
            assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
            assert_eq!(
                read_topup(&conn, "order-1", "buyer").unwrap().status,
                "review_required"
            );
        }
    }

    #[test]
    fn review_holds_have_a_separate_bound_from_active_checkout_capacity() {
        let conn = memory();
        fixture_revision(&conn, 1);
        for index in 0..MAX_REVIEW_ORDERS {
            fixture_topup(
                &conn,
                &format!("review-{index}"),
                &format!("router-review-{index:032}"),
                "review_required",
                1,
            );
            conn.execute(
                "UPDATE market_topup_orders SET created_at = ?2 WHERE id = ?1",
                params![format!("review-{index}"), now()],
            )
            .unwrap();
            if index == MAX_REVIEW_ORDERS - 2 {
                ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();
            }
        }
        assert!(matches!(
            ensure_pending_topup_capacity_tx(&conn, "buyer"),
            Err(AppError::Coded {
                code: "MARKET_TOPUP_REVIEW_LIMIT",
                ..
            })
        ));
        conn.execute(
            "UPDATE market_topup_orders SET created_at = '2025-01-01'
             WHERE id = 'review-0'",
            [],
        )
        .unwrap();
        ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();
        conn.execute(
            "UPDATE market_topup_orders SET status = 'expired' WHERE id = 'review-0'",
            [],
        )
        .unwrap();
        for index in 0..MAX_PENDING_ORDERS {
            fixture_topup(
                &conn,
                &format!("pending-{index}"),
                &format!("router-pending-{index:032}"),
                "pending",
                1,
            );
        }
        assert!(matches!(
            ensure_pending_topup_capacity_tx(&conn, "buyer"),
            Err(AppError::Coded {
                code: "MARKET_TOPUP_PENDING_LIMIT",
                ..
            })
        ));
    }

    #[test]
    fn a_new_attempt_never_reuses_a_nonpayable_topup_of_the_same_amount() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            "router-00000000000000000000000000000001",
            "review_required",
            1,
        );
        assert!(
            active_matching_topup_tx(&conn, "buyer", "channel:100")
                .unwrap()
                .is_none()
        );
        for status in ["verifying", "credited", "expired", "cancelled"] {
            conn.execute(
                "UPDATE market_topup_orders SET status = ?2 WHERE id = ?1",
                params!["order-1", status],
            )
            .unwrap();
            assert!(
                active_matching_topup_tx(&conn, "buyer", "channel:100")
                    .unwrap()
                    .is_none()
            );
        }
        conn.execute(
            "UPDATE market_topup_orders SET status = 'pending' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        assert_eq!(
            active_matching_topup_tx(&conn, "buyer", "channel:100")
                .unwrap()
                .unwrap()
                .0,
            "order-1"
        );
        conn.execute(
            "UPDATE market_topup_orders SET checkout_url =
             'https://www.dujiaopay.com/checkout/ct_old' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        mark_gateway_cancel_requested_tx(&conn, "buyer", "order-1", &now()).unwrap();
        assert!(
            active_matching_topup_tx(&conn, "buyer", "channel:100")
                .unwrap()
                .is_none()
        );
        assert!(
            read_topup(&conn, "order-1", "buyer")
                .unwrap()
                .checkout_url
                .is_none()
        );
        assert!(
            topups_tx(&conn, "buyer", None).unwrap().items[0]
                .checkout_url
                .is_none()
        );
        let mut gateway = unpaid_order("router-00000000000000000000000000000001", "pending");
        gateway.checkout_url = Some("https://www.dujiaopay.com/checkout/ct_restored".into());
        restore_verified_checkout_tx(&conn, "order-1", &gateway).unwrap();
        let checkout_url: Option<String> = conn
            .query_row(
                "SELECT checkout_url FROM market_topup_orders WHERE id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            checkout_url.as_deref(),
            Some("https://www.dujiaopay.com/checkout/ct_old")
        );
    }

    #[test]
    fn official_topup_modes_require_owner_testing_and_live_approval() {
        assert!(!user_topup_allowed("disabled", true, false));
        assert!(user_topup_allowed("shadow", true, false));
        assert!(!user_topup_allowed("shadow", false, false));
        assert!(!user_topup_allowed("enabled", false, false));
        assert!(!user_topup_allowed("enabled", true, false));
        assert!(user_topup_allowed("enabled", false, true));
    }

    #[test]
    fn owner_can_close_topups_but_reopening_rechecks_current_channel_readiness() {
        let conn = memory();
        let at = "2026-10-16T00:00:00Z";
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "credited-1",
            &format!("router-{}", "a".repeat(32)),
            "credited",
            1,
        );
        conn.execute("UPDATE market_payment_channels SET webhook_verified_at=?1,webhook_verified_revision=1 WHERE id='channel'", params![at]).unwrap();
        assert!(finance_readiness_tx(&conn).unwrap().can_enable);
        assert!(
            set_finance_mode_tx(
                &conn,
                &ModeRequest {
                    mode: "enabled".into(),
                    expected_version: 1
                },
                at
            )
            .is_err()
        );
        for (version, mode) in [
            (1, "shadow"),
            (2, "enabled"),
            (3, "disabled"),
            (4, "shadow"),
            (5, "enabled"),
            (6, "shadow"),
        ] {
            set_finance_mode_tx(
                &conn,
                &ModeRequest {
                    mode: mode.into(),
                    expected_version: version,
                },
                at,
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE market_payment_channels SET webhook_verified_revision=NULL",
            [],
        )
        .unwrap();
        assert!(!finance_readiness_tx(&conn).unwrap().can_enable);
        assert!(
            set_finance_mode_tx(
                &conn,
                &ModeRequest {
                    mode: "enabled".into(),
                    expected_version: 7
                },
                at
            )
            .is_err()
        );
        assert!(
            set_finance_mode_tx(
                &conn,
                &ModeRequest {
                    mode: "disabled".into(),
                    expected_version: 6
                },
                at
            )
            .is_err()
        );
        require_new_wallet_orders_open_tx(&conn).unwrap();
    }

    #[test]
    fn purchase_and_topup_pauses_are_independent_in_every_topup_mode() {
        let conn = memory();
        let at = "2026-10-16T00:00:00Z";
        for mode in ["disabled", "shadow", "enabled"] {
            conn.execute("UPDATE market_finance_control SET mode=?1,version=1,new_orders_paused=0,new_topups_paused=0", params![mode]).unwrap();
            let pause = FinancePauseRequest {
                new_orders_paused: true,
                new_topups_paused: Some(false),
                expected_version: 1,
            };
            set_finance_pause_tx(&conn, &pause, at).unwrap();
            assert!(require_new_wallet_orders_open_tx(&conn).is_err());
            require_new_topups_open_tx(&conn).unwrap();
            assert!(set_finance_pause_tx(&conn, &pause, at).is_err());
            set_finance_pause_tx(
                &conn,
                &FinancePauseRequest {
                    new_orders_paused: false,
                    new_topups_paused: Some(true),
                    expected_version: 2,
                },
                at,
            )
            .unwrap();
            require_new_wallet_orders_open_tx(&conn).unwrap();
            assert!(require_new_topups_open_tx(&conn).is_err());
        }
    }

    #[test]
    fn credited_status_without_receipt_and_wallet_posting_is_not_cutover_evidence() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "incomplete",
            &format!("router-{}", "c".repeat(32)),
            "pending",
            1,
        );
        conn.execute(
            "UPDATE market_topup_orders SET status = 'credited' WHERE id = 'incomplete'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE market_payment_channels
             SET webhook_verified_at = '2026-10-16T00:00:00Z', webhook_verified_revision = 1
             WHERE id = 'channel'",
            [],
        )
        .unwrap();
        let readiness = finance_readiness_tx(&conn).unwrap();
        assert_eq!(readiness.verified_live_topups, 0);
        assert!(!readiness.can_enable);
        assert!(readiness.payment_channel_blockers.iter().any(|blocker| {
            blocker.code == "credited_topup_missing_evidence" && blocker.count == 1
        }));
        assert!(
            readiness
                .payment_channel_blockers
                .iter()
                .any(|blocker| { blocker.code == "channel_none_live" && blocker.count == 1 })
        );
    }

    #[test]
    fn topup_return_urls_land_on_wallet_for_create_and_recovery() {
        assert_eq!(
            topup_return_urls("https://router.example", "order-1"),
            (
                "https://router.example/account/wallet/?topup=order-1".to_owned(),
                "https://router.example/account/wallet/?topup=order-1&cancelled=1".to_owned(),
            )
        );
        assert_eq!(
            payment_webhook_url("sgptokenswitch.cc", false),
            "https://sgptokenswitch.cc/v1/payment-webhooks/dujiaopay"
        );
    }

    #[test]
    fn finance_mode_request_uses_frontend_camel_case_and_rejects_stale_shape() {
        let request: ModeRequest = serde_json::from_value(serde_json::json!({
            "mode": "shadow", "expectedVersion": 3
        }))
        .unwrap();
        assert_eq!(request.mode, "shadow");
        assert_eq!(request.expected_version, 3);
        assert!(
            serde_json::from_value::<ModeRequest>(serde_json::json!({
                "mode": "shadow", "expected_version": 3
            }))
            .is_err()
        );
    }

    #[test]
    fn merchant_configuration_accepts_poll_only_but_not_a_short_secret() {
        let mut input = UpsertChannelRequest {
            display_name: "Gateway".into(),
            sort_order: 0,
            api_key_id: "key".into(),
            api_secret: "0123456789abcdef".into(),
            webhook_secret: None,
            webhook_mode: None,
            allowed_methods: Vec::new(),
            expected_version: None,
        };
        assert!(validate_channel_input(&input).is_err());
        input.webhook_mode = Some("poll_only".into());
        assert!(validate_channel_input(&input).is_ok());
        input.webhook_secret = Some("short".into());
        assert!(validate_channel_input(&input).is_err());
        input.webhook_secret = Some("0123456789abcdef".into());
        input.webhook_mode = Some("signed".into());
        assert!(validate_channel_input(&input).is_ok());
        input.webhook_secret = Some("x".repeat(MAX_PAYMENT_SECRET_BYTES + 1));
        assert!(validate_channel_input(&input).is_err());
        input.webhook_secret = Some("0123456789abcdef".into());
        assert!(validate_webhook_mode_change("signed", "poll_only", false).is_err());
        assert!(validate_webhook_mode_change("signed", "poll_only", true).is_err());
        assert!(validate_webhook_mode_change("signed", "signed", false).is_ok());
        assert!(validate_webhook_mode_change("poll_only", "poll_only", false).is_ok());
        input.webhook_secret = None;
        input.webhook_mode = Some("signed".into());
        assert!(validate_channel_input(&input).is_err());
    }

    #[test]
    fn buyer_channel_projection_hides_webhook_and_finance_state() {
        let channel = ChannelView {
            id: "channel".into(),
            display_name: "Gateway".into(),
            enabled: true,
            sort_order: 0,
            current_revision: 7,
            version: 1,
            archived_at: None,
            verified: true,
            webhook_mode: "poll_only".into(),
            webhook_verified_at: None,
            webhook_verified: false,
            unresolved_topups: 0,
            verified_live_topups: 42,
            verified_live_topups_current_revision: 0,
            gateway_kind: "dujiaopay",
            rollout_state: "live".into(),
            promoted_at: None,
            settings: ChannelSettingsView {
                order_mode: "direct".into(),
                allowed_methods: vec!["tron-usdt".into(), "unknown-token".into()],
                min_topup_minor: Some(50),
                max_topup_minor: Some(5_000),
                fee_rate_bps: 150,
                fixed_fee_minor: 10,
                version: 3,
            },
        };
        let policy = crate::market_wallet_policy::policy_tx(&memory()).unwrap();
        let value = serde_json::to_value(BuyerChannelView::new(channel, &policy).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "id": "channel", "displayName": "Gateway", "gatewayKind": "dujiaopay",
                "orderMode": "direct",
                "methods": [{
                    "tokenId": "tron-usdt", "chain": "tron", "chainName": "Tron",
                    "symbol": "USDT", "minConfirmations": 19
                }],
                "minTopupMinor": 100, "maxTopupMinor": 5000,
                "minCreditMinor": 100, "maxCreditMinor": 5000,
                "maintenanceFeeBps": 500, "policyVersion": 1,
                "channelRevision": 7, "channelSettingsVersion": 3, "canary": false
            })
        );
    }

    #[test]
    fn separate_ip_buckets_bound_new_orders_without_affecting_other_sources() {
        let limiter = WebhookLimiter::default();
        let first: IpAddr = "192.0.2.1".parse().unwrap();
        let second: IpAddr = "192.0.2.2".parse().unwrap();
        assert!(limiter.allow(first, StdDuration::from_secs(3600), 2));
        assert!(limiter.allow(first, StdDuration::from_secs(3600), 2));
        assert!(!limiter.allow(first, StdDuration::from_secs(3600), 2));
        assert!(limiter.allow(second, StdDuration::from_secs(3600), 2));
        assert!(limiter.allow(first, StdDuration::ZERO, 2));
    }

    fn fixture_revision(conn: &Connection, revision: i64) {
        if revision == 1 {
            conn.execute(
                "INSERT INTO market_payment_channels
                 (id, gateway_kind, display_name, enabled, current_revision, created_at, updated_at)
                 VALUES ('channel', 'dujiaopay', 'Gateway', 1, 1, '2026-10-01', '2026-10-01')",
                [],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO market_payment_channel_revisions
             (channel_id, revision, merchant_id, merchant_fingerprint, api_key_id,
              api_secret_ciphertext, api_secret_nonce, webhook_secret_ciphertext,
              webhook_secret_nonce, key_version, verified_at, created_at)
             VALUES ('channel', ?1, 'merchant', 'merchant-fingerprint', 'key',
                     'encrypted', 'nonce', 'encrypted', 'nonce', 1, '2026-10-01', '2026-10-01')",
            params![revision],
        )
        .unwrap();
    }

    #[test]
    fn migrated_legacy_revisions_remain_signed_and_poll_only_is_explicit() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let old_mode: String = conn
            .query_row(
                "SELECT webhook_mode FROM market_payment_channel_revisions
             WHERE channel_id = 'channel' AND revision = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_mode, "signed");
        conn.execute(
            "INSERT INTO market_payment_channel_revisions
             (channel_id, revision, merchant_id, merchant_fingerprint, api_key_id,
              api_secret_ciphertext, api_secret_nonce, webhook_secret_ciphertext,
              webhook_secret_nonce, key_version, webhook_mode, verified_at, created_at)
             VALUES ('channel', 2, 'merchant', 'merchant-fingerprint', 'key',
                     'encrypted', 'nonce', '', '', 1, 'poll_only', '2026-10-01', '2026-10-01')",
            [],
        )
        .unwrap();
        let new_mode: String = conn
            .query_row(
                "SELECT webhook_mode FROM market_payment_channel_revisions
             WHERE channel_id = 'channel' AND revision = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_mode, "poll_only");
    }

    #[test]
    fn old_signed_and_new_poll_only_revisions_load_independently() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_revision(&conn, 2);
        let runtime = MarketPaymentsRuntime::for_test();
        let fingerprint = hex::encode(Sha256::digest(b"merchant"));
        for revision in [1, 2] {
            let (api_ciphertext, api_nonce) = runtime
                .cipher
                .seal(
                    &format!("api-secret-{revision}"),
                    &aad("channel", revision, &fingerprint, "api"),
                )
                .unwrap();
            let (webhook_ciphertext, webhook_nonce, mode) = if revision == 1 {
                let (ciphertext, nonce) = runtime
                    .cipher
                    .seal(
                        "legacy-webhook-secret",
                        &aad("channel", revision, &fingerprint, "webhook"),
                    )
                    .unwrap();
                (ciphertext, nonce, "signed")
            } else {
                (String::new(), String::new(), "poll_only")
            };
            conn.execute(
                "UPDATE market_payment_channel_revisions
                 SET merchant_fingerprint = ?3, api_secret_ciphertext = ?4,
                     api_secret_nonce = ?5, webhook_secret_ciphertext = ?6,
                     webhook_secret_nonce = ?7, key_version = ?8, webhook_mode = ?9
                 WHERE channel_id = ?1 AND revision = ?2",
                params![
                    "channel",
                    revision,
                    fingerprint,
                    api_ciphertext,
                    api_nonce,
                    webhook_ciphertext,
                    webhook_nonce,
                    runtime.cipher.version,
                    mode
                ],
            )
            .unwrap();
        }
        let old = load_revision(&conn, &runtime, "channel", 1).unwrap();
        let new = load_revision(&conn, &runtime, "channel", 2).unwrap();
        assert_eq!(old.credentials.api_secret.as_str(), "api-secret-1");
        assert!(old.credentials.webhook_secret.is_some());
        assert_eq!(new.credentials.api_secret.as_str(), "api-secret-2");
        assert!(new.credentials.webhook_secret.is_none());
    }

    fn seed_live_revision(
        conn: &Connection,
        runtime: &MarketPaymentsRuntime,
        revision: i64,
        api_key: &str,
        api_secret: &str,
        webhook_secret: Option<&str>,
    ) {
        let fingerprint = hex::encode(Sha256::digest(b"merchant"));
        if revision == 1 {
            conn.execute(
                "INSERT INTO market_payment_channels (id, gateway_kind, display_name, enabled,
                 current_revision, created_at, updated_at)
                 VALUES ('channel', 'dujiaopay', 'Gateway', 1, 1, '2026-10-01', '2026-10-01')",
                [],
            )
            .unwrap();
        }
        let (api_ciphertext, api_nonce) = runtime
            .cipher
            .seal(api_secret, &aad("channel", revision, &fingerprint, "api"))
            .unwrap();
        let (webhook_ciphertext, webhook_nonce) = if let Some(secret) = webhook_secret {
            runtime
                .cipher
                .seal(secret, &aad("channel", revision, &fingerprint, "webhook"))
                .unwrap()
        } else {
            (String::new(), String::new())
        };
        conn.execute(
            "INSERT INTO market_payment_channel_revisions (
                channel_id, revision, merchant_id, merchant_fingerprint, api_key_id,
                api_secret_ciphertext, api_secret_nonce, webhook_secret_ciphertext,
                webhook_secret_nonce, key_version, allowed_methods_json, verified_at,
                created_at, webhook_mode
             ) VALUES ('channel', ?1, 'merchant', ?2, ?3, ?4, ?5, ?6, ?7, 1, '[]',
                       '2026-10-01', '2026-10-01', ?8)",
            params![
                revision,
                fingerprint,
                api_key,
                api_ciphertext,
                api_nonce,
                webhook_ciphertext,
                webhook_nonce,
                if webhook_secret.is_some() {
                    "signed"
                } else {
                    "poll_only"
                }
            ],
        )
        .unwrap();
        conn.execute(
            "UPDATE market_payment_channels SET current_revision = ?1 WHERE id = 'channel'",
            params![revision],
        )
        .unwrap();
    }

    fn signed_webhook_headers(secret: &str, body: &[u8], timestamp: i64) -> HeaderMap {
        use hmac::{Hmac, Mac};
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        let mut headers = HeaderMap::new();
        headers.insert(
            "DJP-Webhook-Timestamp",
            timestamp.to_string().parse().unwrap(),
        );
        headers.insert(
            "DJP-Webhook-Signature",
            hex::encode(mac.finalize().into_bytes()).parse().unwrap(),
        );
        headers
    }

    #[test]
    fn channel_archive_and_versions_leave_historical_orders_visible() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            &format!("router-{}", "a".repeat(32)),
            "pending",
            1,
        );
        conn.execute(
            "UPDATE market_payment_channels SET previous_webhook_revision = 1,
             previous_webhook_valid_until = '2026-10-03' WHERE id = 'channel'",
            [],
        )
        .unwrap();
        assert_eq!(editable_revision_tx(&conn, "channel", 1).unwrap(), 1);
        update_channel_archive_tx(&conn, "channel", true, 1, "2026-10-02").unwrap();
        assert!(matches!(
            editable_revision_tx(&conn, "channel", 1),
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            editable_revision_tx(&conn, "channel", 2),
            Err(AppError::Conflict(_))
        ));
        let view = channel_view_tx(&conn, "channel").unwrap();
        assert!(view.archived_at.is_some());
        assert!(!view.verified);
        assert_eq!(view.unresolved_topups, 1);
        let previous_key: Option<i64> = conn
            .query_row(
                "SELECT previous_webhook_revision FROM market_payment_channels WHERE id = 'channel'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(previous_key.is_none());
        assert!(!mark_channel_webhook_verified_tx(&conn, "channel", 1, "2026-10-02").unwrap());
        assert!(!channel_view_tx(&conn, "channel").unwrap().webhook_verified);
        assert!(channel_views(&conn, true).unwrap().is_empty());
        update_channel_archive_tx(&conn, "channel", false, 2, "2026-10-03").unwrap();
        let restored = channel_view_tx(&conn, "channel").unwrap();
        assert!(restored.archived_at.is_none());
        assert!(!restored.verified);
        assert!(!restored.enabled);
        conn.execute(
            "UPDATE market_payment_channels SET enabled = 1 WHERE id = 'channel'",
            [],
        )
        .unwrap();
        assert!(channel_views(&conn, true).unwrap().is_empty());
        mark_channel_api_reverified_tx(&conn, "channel", 3, "2026-10-04").unwrap();
        assert!(matches!(
            mark_channel_api_reverified_tx(&conn, "channel", 3, "2026-10-05"),
            Err(AppError::Conflict(_))
        ));
        assert!(channel_view_tx(&conn, "channel").unwrap().verified);
        assert_eq!(channel_views(&conn, true).unwrap().len(), 1);
        assert_eq!(
            bound_webhook_order_tx(&conn, "channel", &format!("router-{}", "a".repeat(32)))
                .unwrap()
                .unwrap()
                .0,
            "order-1"
        );
    }

    #[test]
    fn current_merchant_keys_recover_old_orders_and_emergency_revokes_old_webhook_key() {
        let conn = memory();
        let runtime = MarketPaymentsRuntime::for_test();
        seed_live_revision(
            &conn,
            &runtime,
            1,
            "old-key",
            "old-api-secret-16",
            Some("old-webhook-secret"),
        );
        seed_live_revision(
            &conn,
            &runtime,
            2,
            "new-key",
            "new-api-secret-16",
            Some("new-webhook-secret"),
        );
        let expiry = (Utc::now() + chrono::Duration::minutes(10)).to_rfc3339();
        conn.execute(
            "UPDATE market_payment_channels SET previous_webhook_revision = 1,
            previous_webhook_valid_until = ?1 WHERE id = 'channel'",
            params![expiry],
        )
        .unwrap();
        let old_order = load_order_revision(&conn, &runtime, "channel", 1).unwrap();
        assert_eq!(old_order.credentials.key_id, "new-key");
        assert_eq!(
            old_order.credentials.api_secret.as_str(),
            "new-api-secret-16"
        );
        let body = format!(
            r#"{{"event_type":"order.paid","data":{{"order_id":"gateway-1","merchant_order_id":"router-{}"}}}}"#,
            "a".repeat(32)
        );
        let timestamp = Utc::now().timestamp();
        let old_headers = signed_webhook_headers("old-webhook-secret", body.as_bytes(), timestamp);
        let new_headers = signed_webhook_headers("new-webhook-secret", body.as_bytes(), timestamp);
        assert_eq!(
            verify_bound_webhook_tx(
                &conn,
                &runtime,
                "channel",
                1,
                &old_headers,
                body.as_bytes(),
                timestamp
            )
            .unwrap()
            .2,
            None
        );
        assert_eq!(
            verify_bound_webhook_tx(
                &conn,
                &runtime,
                "channel",
                1,
                &new_headers,
                body.as_bytes(),
                timestamp
            )
            .unwrap()
            .2,
            Some(2)
        );
        conn.execute(
            "UPDATE market_payment_channels SET previous_webhook_revision = NULL,
            previous_webhook_valid_until = NULL, archived_at = '2026-10-02', enabled = 0
            WHERE id = 'channel'",
            [],
        )
        .unwrap();
        assert!(
            verify_bound_webhook_tx(
                &conn,
                &runtime,
                "channel",
                1,
                &old_headers,
                body.as_bytes(),
                timestamp
            )
            .is_err()
        );
        assert!(
            verify_bound_webhook_tx(
                &conn,
                &runtime,
                "channel",
                1,
                &new_headers,
                body.as_bytes(),
                timestamp
            )
            .is_ok()
        );
    }

    #[test]
    fn channel_readiness_does_not_confuse_api_verification_with_callback_or_live_credit() {
        let conn = memory();
        let runtime = MarketPaymentsRuntime::for_test();
        seed_live_revision(
            &conn,
            &runtime,
            1,
            "key",
            "api-secret-16chars",
            Some("webhook-secret-16chars"),
        );
        let view = channel_view_tx(&conn, "channel").unwrap();
        assert!(view.verified);
        assert!(!view.webhook_verified);
        assert_eq!(view.verified_live_topups_current_revision, 0);
        let readiness = finance_readiness_tx(&conn).unwrap();
        assert!(
            readiness
                .payment_channel_blockers
                .iter()
                .any(|blocker| blocker.code == "channel_webhook_unverified")
        );
        assert!(
            readiness
                .payment_channel_blockers
                .iter()
                .any(|blocker| blocker.code == "channel_none_live")
        );
        assert!(!readiness.can_enable);

        conn.execute(
            "UPDATE market_payment_channels SET webhook_verified_at = '2026-10-02',
             webhook_verified_revision = NULL WHERE id = 'channel'",
            [],
        )
        .unwrap();
        assert!(!channel_view_tx(&conn, "channel").unwrap().webhook_verified);
        assert!(
            finance_readiness_tx(&conn)
                .unwrap()
                .payment_channel_blockers
                .iter()
                .any(|blocker| blocker.code == "channel_webhook_unverified")
        );
        conn.execute(
            "UPDATE market_payment_channel_revisions SET verified_at = NULL
             WHERE channel_id = 'channel' AND revision = 1",
            [],
        )
        .unwrap();
        let blockers = finance_readiness_tx(&conn)
            .unwrap()
            .payment_channel_blockers;
        assert!(
            blockers
                .iter()
                .any(|blocker| blocker.code == "channel_api_unverified")
        );
        assert!(
            blockers
                .iter()
                .any(|blocker| blocker.code == "channel_none_enabled")
        );
    }

    #[test]
    fn production_channel_reenable_goes_through_canary_and_needs_a_signed_revision() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let rollout = |conn: &Connection| -> String {
            conn.query_row(
                "SELECT rollout_state FROM market_payment_channels WHERE id = 'channel'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        conn.execute(
            "UPDATE market_payment_channels SET enabled = 0 WHERE id = 'channel'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE market_finance_control SET mode = 'shadow' WHERE id = 1",
            [],
        )
        .unwrap();
        set_channel_enabled_tx(&conn, "channel", true, 1, "2026-10-02").unwrap();
        assert_eq!(rollout(&conn), "canary");
        set_channel_enabled_tx(&conn, "channel", false, 2, "2026-10-03").unwrap();
        assert_eq!(rollout(&conn), "off");
        conn.execute(
            "UPDATE market_finance_control SET mode = 'enabled' WHERE id = 1",
            [],
        )
        .unwrap();
        // A signed revision without evidence re-enters through canary.
        set_channel_enabled_tx(&conn, "channel", true, 3, "2026-10-04").unwrap();
        assert_eq!(rollout(&conn), "canary");
        set_channel_enabled_tx(&conn, "channel", false, 4, "2026-10-04").unwrap();
        // With verified live evidence and a verified Webhook it is live at once.
        fixture_topup(
            &conn,
            "credited-1",
            &format!("router-{}", "a".repeat(32)),
            "credited",
            1,
        );
        conn.execute(
            "UPDATE market_payment_channels SET webhook_verified_at = '2026-10-04',
             webhook_verified_revision = 1 WHERE id = 'channel'",
            [],
        )
        .unwrap();
        set_channel_enabled_tx(&conn, "channel", true, 5, "2026-10-04").unwrap();
        assert_eq!(rollout(&conn), "live");
        set_channel_enabled_tx(&conn, "channel", false, 6, "2026-10-05").unwrap();

        // A poll-only revision cannot accept production orders at all.
        fixture_revision(&conn, 2);
        conn.execute(
            "UPDATE market_payment_channel_revisions SET webhook_mode = 'poll_only'
             WHERE channel_id = 'channel' AND revision = 2",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE market_payment_channels SET current_revision = 2,
             webhook_verified_revision = 2 WHERE id = 'channel'",
            [],
        )
        .unwrap();
        assert!(set_channel_enabled_tx(&conn, "channel", true, 7, "2026-10-06").is_err());
    }

    fn fixture_topup(
        conn: &Connection,
        id: &str,
        merchant_order_id: &str,
        status: &str,
        revision: i64,
    ) {
        conn.execute(
            "INSERT INTO market_topup_orders
             (id, user_id, channel_id, channel_revision, merchant_fingerprint,
              merchant_order_id, idempotency_key, request_fingerprint, amount_minor,
              status, created_at, updated_at)
             VALUES (?1, 'buyer', 'channel', ?2, 'merchant-fingerprint',
                     ?3, ?1, 'channel:100', 100, ?4, '2026-10-01', '2026-10-01')",
            params![id, revision, merchant_order_id, status],
        )
        .unwrap();
        if status == "credited" {
            let receipt_fingerprint = format!("fixture-{id}");
            conn.execute(
                "INSERT INTO market_topup_receipts
                 (order_id, chain, token_id, tx_id, settled_amount, confirmations,
                  receipt_fingerprint, observed_at)
                 VALUES (?1, 'tron', 'tron-usdt', ?1, '1.00', 1, ?2, '2026-10-01')",
                params![id, receipt_fingerprint],
            )
            .unwrap();
            let journal = credit_verified_topup_tx(
                conn,
                "buyer",
                id,
                100,
                &receipt_fingerprint,
                "2026-10-01",
            )
            .unwrap();
            conn.execute(
                "UPDATE market_topup_orders
                 SET gateway_order_id = ?2, credited_journal_id = ?3, paid_at = '2026-10-01'
                 WHERE id = ?1",
                params![id, format!("gateway-{id}"), journal.id],
            )
            .unwrap();
        }
    }

    fn loaded() -> LoadedRevision {
        LoadedRevision {
            merchant_id: "merchant".into(),
            merchant_fingerprint: "merchant-fingerprint".into(),
            allowed_methods: Vec::new(),
            credentials: Credentials {
                key_id: "key".into(),
                api_secret: Zeroizing::new("api-secret".into()),
                webhook_secret: Some(Zeroizing::new("webhook-secret".into())),
            },
        }
    }

    fn paid_order(merchant_order_id: &str, tx_id: &str) -> GatewayOrder {
        GatewayOrder {
            order_id: "gateway-1".into(),
            merchant_order_id: merchant_order_id.into(),
            status: "paid".into(),
            fiat_currency: "USD".into(),
            fiat_amount_minor: 100,
            paid_source: Some("chain".into()),
            chain: Some("tron".into()),
            token_id: Some("tron-usdt".into()),
            tx_id: Some(tx_id.into()),
            settled_amount: Some("1.00".into()),
            confirmations: Some(19),
            paid_at: Some("2026-10-01T00:00:00+00:00".into()),
            checkout_url: None,
            expires_at: None,
            selection_deadline: None,
            pay_address: None,
            payable_amount: None,
            fx_rate: None,
        }
    }

    fn unpaid_order(merchant_order_id: &str, status: &str) -> GatewayOrder {
        let mut order = paid_order(merchant_order_id, "unused");
        order.status = status.into();
        order.paid_source = None;
        order.tx_id = None;
        order.settled_amount = None;
        order.confirmations = None;
        order.paid_at = None;
        order
    }

    fn event(order: &GatewayOrder) -> WebhookEvent {
        WebhookEvent {
            event_id: "event-1".into(),
            event_type: "order.paid".into(),
            order_id: order.order_id.clone(),
            merchant_order_id: order.merchant_order_id.clone(),
            raw_sha256: "hash".into(),
        }
    }

    #[test]
    fn unsafe_checkout_keeps_the_gateway_id_without_exposing_a_payment_link() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "creating", 1);
        apply_verified_unpaid_status_tx(&conn, "order-1", "pending", &now()).unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "create_unknown"
        );
        let created = CreatedOrder {
            order_id: "gateway-1".into(),
            checkout_url: None,
            expires_at: None,
            ..Default::default()
        };
        save_created_order_tx(&conn, "order-1", 1, &created, &now()).unwrap();
        let stored: (String, String) = conn
            .query_row(
                "SELECT gateway_order_id, status FROM market_topup_orders WHERE id = 'order-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored, ("gateway-1".into(), "create_unknown".into()));
        assert!(
            read_topup(&conn, "order-1", "buyer")
                .unwrap()
                .checkout_url
                .is_none()
        );
        apply_verified_unpaid_status_tx(&conn, "order-1", "pending", &now()).unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "create_unknown"
        );
        let mut gateway = paid_order(merchant_order_id, "chain-tx");
        gateway.status = "pending".into();
        gateway.checkout_url = Some("https://www.dujiaopay.com/checkout/ct_1".into());
        validate_gateway_identity_tx(
            &conn,
            "order-1",
            "channel",
            1,
            &loaded(),
            "gateway-1",
            &gateway,
        )
        .unwrap();
        restore_verified_checkout_tx(&conn, "order-1", &gateway).unwrap();
        let view = read_topup(&conn, "order-1", "buyer").unwrap();
        assert_eq!(view.status, "pending");
        assert_eq!(
            view.checkout_url.as_deref(),
            Some("https://www.dujiaopay.com/checkout/ct_1")
        );
        gateway.status = "expired".into();
        apply_verified_unpaid_status_tx(&conn, "order-1", "expired", &now()).unwrap();
        assert!(
            read_topup(&conn, "order-1", "buyer")
                .unwrap()
                .checkout_url
                .is_none()
        );
    }

    #[test]
    fn signed_progress_events_bind_identity_but_never_prove_payment() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "creating", 1);
        let mut created = event(&paid_order(merchant_order_id, "chain-tx"));
        created.event_type = "order.created".into();
        assert!(is_non_payment_lifecycle_event(&created.event_type));
        assert!(is_non_payment_lifecycle_event("order.method_selected"));
        assert!(is_non_payment_lifecycle_event("order.confirming"));
        assert!(!is_non_payment_lifecycle_event("order.paid"));
        assert!(!is_non_payment_lifecycle_event("refund.recorded"));
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &created,
            &now(),
        )
        .unwrap();
        bind_signed_gateway_order_tx(&conn, "order-1", "channel", 1, &loaded(), &created).unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "creating"
        );
        assert!(!topup_has_manual_hold_tx(&conn, "order-1").unwrap());
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &created,
            &now(),
        )
        .unwrap();
        let mut different_body = created.clone();
        different_body.raw_sha256 = "other-hash".into();
        assert!(matches!(
            persist_signed_event_tx(
                &conn,
                "order-1",
                &loaded().merchant_fingerprint,
                &different_body,
                &now()
            ),
            Err(AppError::Conflict(_))
        ));
        fixture_topup(
            &conn,
            "order-2",
            "router-00000000000000000000000000000002",
            "creating",
            1,
        );
        assert!(matches!(
            persist_signed_event_tx(
                &conn,
                "order-2",
                &loaded().merchant_fingerprint,
                &created,
                &now()
            ),
            Err(AppError::Conflict(_))
        ));
        let mut different_order = created.clone();
        different_order.order_id = "gateway-2".into();
        assert!(matches!(
            bind_signed_gateway_order_tx(
                &conn,
                "order-1",
                "channel",
                1,
                &loaded(),
                &different_order
            ),
            Err(AppError::Conflict(_))
        ));
    }

    #[test]
    fn only_a_verified_unpaid_terminal_closes_the_old_created_event_hold() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        for index in 2..=3 {
            fixture_topup(
                &conn,
                &format!("order-{index}"),
                &format!("router-{index:032}"),
                "pending",
                1,
            );
        }
        let mut created = event(&paid_order(merchant_order_id, "chain-tx"));
        created.event_type = "order.created".into();
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &created,
            &now(),
        )
        .unwrap();
        conn.execute(
            "UPDATE market_topup_events SET status = 'review_required' WHERE order_id = 'order-1'",
            [],
        )
        .unwrap();
        let mut method_selected = created.clone();
        method_selected.event_id = "event-method".into();
        method_selected.event_type = "order.method_selected".into();
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &method_selected,
            &now(),
        )
        .unwrap();
        conn.execute(
            "UPDATE market_topup_events SET status = 'review_required' WHERE gateway_event_id = 'event-method'",
            [],
        )
        .unwrap();
        open_reconciliation_case_tx(&conn, "unknown_gateway_event", "topup", "order-1", &now())
            .unwrap();
        // A held review is visible and auditable, but it does not consume one
        // of the three active checkout slots forever.
        ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();
        assert!(topup_has_manual_hold_tx(&conn, "order-1").unwrap());
        apply_verified_unpaid_status_tx(&conn, "order-1", "pending", &now()).unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "review_required"
        );
        let mut gateway = unpaid_order(merchant_order_id, "expired");
        gateway.order_id = created.order_id.clone();
        validate_gateway_identity_tx(
            &conn,
            "order-1",
            "channel",
            1,
            &loaded(),
            &created.order_id,
            &gateway,
        )
        .unwrap();
        apply_verified_unpaid_status_tx(
            &conn,
            "order-1",
            verified_unpaid_status(&gateway.status).unwrap(),
            &now(),
        )
        .unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "expired"
        );
        assert!(!topup_has_manual_hold_tx(&conn, "order-1").unwrap());
        ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();
        let resolved: (String, String) = conn.query_row(
            "SELECT status, resolved_at FROM market_finance_reconciliation_cases WHERE object_id = 'order-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(resolved.0, "resolved");
        assert!(!resolved.1.is_empty());
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
        assert_eq!(
            active_matching_topup_tx(&conn, "buyer", "channel:100")
                .unwrap()
                .unwrap()
                .0,
            "order-3"
        );
    }

    #[test]
    fn paid_or_disputed_evidence_never_auto_releases_an_old_review() {
        for extra in ["paid_event", "refund_event", "other_case", "receipt"] {
            let conn = memory();
            fixture_revision(&conn, 1);
            let merchant_order_id = "router-00000000000000000000000000000001";
            fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
            let mut created = event(&paid_order(merchant_order_id, "chain-tx"));
            created.event_type = "order.created".into();
            persist_signed_event_tx(
                &conn,
                "order-1",
                &loaded().merchant_fingerprint,
                &created,
                &now(),
            )
            .unwrap();
            conn.execute("UPDATE market_topup_events SET status = 'review_required' WHERE order_id = 'order-1'", []).unwrap();
            open_reconciliation_case_tx(&conn, "unknown_gateway_event", "topup", "order-1", &now())
                .unwrap();
            match extra {
                "paid_event" | "refund_event" => {
                    let mut second = created.clone();
                    second.event_id = "event-2".into();
                    second.event_type = if extra == "paid_event" {
                        "order.paid"
                    } else {
                        "refund.recorded"
                    }
                    .into();
                    persist_signed_event_tx(
                        &conn,
                        "order-1",
                        &loaded().merchant_fingerprint,
                        &second,
                        &now(),
                    )
                    .unwrap();
                    conn.execute("UPDATE market_topup_events SET status = 'review_required' WHERE gateway_event_id = 'event-2'", []).unwrap();
                }
                "other_case" => open_reconciliation_case_tx(
                    &conn,
                    "topup_verification_failed",
                    "topup",
                    "order-1",
                    &now(),
                )
                .unwrap(),
                "receipt" => {
                    conn.execute("INSERT INTO market_topup_receipts (order_id, chain, token_id, tx_id, settled_amount, confirmations, receipt_fingerprint, observed_at) VALUES ('order-1', 'tron', 'tron-usdt', 'tx', '1.00', 1, 'receipt', '2026-10-01')", []).unwrap();
                }
                _ => unreachable!(),
            }
            apply_verified_unpaid_status_tx(&conn, "order-1", "expired", &now()).unwrap();
            assert_eq!(
                read_topup(&conn, "order-1", "buyer").unwrap().status,
                "review_required",
                "{extra}"
            );
            assert!(
                topup_has_manual_hold_tx(&conn, "order-1").unwrap(),
                "{extra}"
            );
        }
    }

    #[test]
    fn previously_acknowledged_progress_case_still_needs_verified_terminal_to_release() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        let mut created = event(&paid_order(merchant_order_id, "chain-tx"));
        created.event_type = "order.created".into();
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &created,
            &now(),
        )
        .unwrap();
        conn.execute(
            "UPDATE market_topup_events SET status = 'review_required' WHERE order_id = 'order-1'",
            [],
        )
        .unwrap();
        open_reconciliation_case_tx(&conn, "unknown_gateway_event", "topup", "order-1", &now())
            .unwrap();
        let case_id: String = conn
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases WHERE object_id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::market_reconcile::resolve_case_tx(
            &conn,
            &case_id,
            "owner",
            "manual_acknowledgement",
            &now(),
        )
        .unwrap();
        apply_verified_unpaid_status_tx(&conn, "order-1", "pending", &now()).unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "review_required"
        );
        apply_verified_unpaid_status_tx(&conn, "order-1", "expired", &now()).unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "expired"
        );
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
    }

    #[test]
    fn mismatched_gateway_identity_and_unknown_status_fail_closed() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        let gateway = paid_order(merchant_order_id, "chain-tx");
        assert!(
            validate_gateway_identity_tx(
                &conn,
                "order-1",
                "channel",
                1,
                &loaded(),
                "other-id",
                &gateway
            )
            .is_err()
        );
        let mut wrong_amount = gateway.clone();
        wrong_amount.fiat_amount_minor = 101;
        assert!(
            validate_gateway_identity_tx(
                &conn,
                "order-1",
                "channel",
                1,
                &loaded(),
                "gateway-1",
                &wrong_amount
            )
            .is_err()
        );
        let mut wrong_merchant = gateway.clone();
        wrong_merchant.merchant_order_id = "other-merchant-order".into();
        assert!(
            validate_gateway_identity_tx(
                &conn,
                "order-1",
                "channel",
                1,
                &loaded(),
                "gateway-1",
                &wrong_merchant
            )
            .is_err()
        );
        fixture_topup(
            &conn,
            "order-2",
            "router-00000000000000000000000000000002",
            "pending",
            1,
        );
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1' WHERE id = 'order-2'",
            [],
        )
        .unwrap();
        assert!(matches!(
            validate_gateway_identity_tx(
                &conn,
                "order-1",
                "channel",
                1,
                &loaded(),
                "gateway-1",
                &gateway
            ),
            Err(AppError::Conflict(_))
        ));
        assert_eq!(verified_unpaid_status("confirming").unwrap(), "verifying");
        assert!(verified_unpaid_status("refunded").is_err());
        hold_topup_verification_failure_tx(&conn, "order-1", "gateway_identity_mismatch", &now())
            .unwrap();
        assert!(topup_has_manual_hold_tx(&conn, "order-1").unwrap());
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
    }

    #[test]
    fn owner_recovery_requires_versioned_preview_and_audits_only_verified_terminal_change() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "create_unknown", 1);
        let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
        let mut input = RecoverTopupRequest {
            expected_status: snapshot.status,
            expected_updated_at: snapshot.updated_at,
            gateway_order_id: Some("gateway-1".into()),
            reason_code: "gateway_terminal_verified".into(),
            manual_reference: None,
            manual_confirmed: false,
            dry_run: true,
        };
        let gateway = unpaid_order(merchant_order_id, "expired");
        assert!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now()
            )
            .unwrap()
            .can_recover()
        );
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "create_unknown"
        );
        let dry_run_audits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_finance_audit_events WHERE object_id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dry_run_audits, 0);
        input.dry_run = false;
        apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &gateway,
            "owner",
            &now(),
        )
        .unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "expired"
        );
        let audit: (String, String, String) = conn.query_row("SELECT actor_user_id, action, reason_code FROM market_finance_audit_events WHERE object_id = 'order-1'", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert_eq!(
            audit,
            (
                "owner".into(),
                "topup_terminal_recovered".into(),
                "gateway_terminal_verified".into()
            )
        );
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now()
            ),
            Err(AppError::Conflict(_))
        ));
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
    }

    #[test]
    fn historical_double_review_needs_merchant_reference_and_keeps_late_payment_reconciliation() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        let mut created = event(&unpaid_order(merchant_order_id, "pending"));
        created.event_type = "order.created".into();
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &created,
            &now(),
        )
        .unwrap();
        conn.execute(
            "UPDATE market_topup_events SET status = 'review_required' WHERE order_id = 'order-1'",
            [],
        )
        .unwrap();
        let mut expired_event = created.clone();
        expired_event.event_id = "expired-event".into();
        expired_event.event_type = "order.expired".into();
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &expired_event,
            &now(),
        )
        .unwrap();
        conn.execute("UPDATE market_topup_events SET status = 'processed' WHERE gateway_event_id = 'expired-event'", []).unwrap();
        open_reconciliation_case_tx(&conn, "unknown_gateway_event", "topup", "order-1", &now())
            .unwrap();
        open_reconciliation_case_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            &now(),
        )
        .unwrap();
        let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
        let mut input = RecoverTopupRequest {
            expected_status: snapshot.status,
            expected_updated_at: snapshot.updated_at,
            gateway_order_id: Some("gateway-1".into()),
            reason_code: "historical_merchant_review".into(),
            manual_reference: None,
            manual_confirmed: false,
            dry_run: true,
        };
        let mut gateway = unpaid_order(merchant_order_id, "expired");
        gateway.settled_amount = Some("0.00000000".into());
        let preview = apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &gateway,
            "owner",
            &now(),
        )
        .unwrap();
        assert!(preview.can_recover());
        assert!(preview.requires_manual_reference);
        assert_eq!(preview.review_case_codes.len(), 2);
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "review_required"
        );

        input.dry_run = false;
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now()
            ),
            Err(AppError::BadRequest(_))
        ));
        input.manual_reference = Some("merchant-case-1234".into());
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now()
            ),
            Err(AppError::BadRequest(_))
        ));
        input.manual_confirmed = true;
        let released = apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &gateway,
            "owner",
            &now(),
        )
        .unwrap();
        assert!(released.can_recover());
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "expired"
        );
        let open_cases: i64 = conn.query_row("SELECT COUNT(*) FROM market_finance_reconciliation_cases WHERE object_id = 'order-1' AND status = 'open'", [], |row| row.get(0)).unwrap();
        assert_eq!(open_cases, 0);
        let recorded: (String, String, String, bool) = conn.query_row(
            "SELECT actor_user_id, manual_reference, gateway_status, manual_confirmed FROM market_topup_recovery_decisions WHERE order_id = 'order-1'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(
            recorded,
            (
                "owner".into(),
                "merchant-case-1234".into(),
                "expired".into(),
                true
            )
        );
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
        ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();

        // A terminal unpaid order is retained, so a later authenticated paid
        // order still credits exactly once through the ordinary receipt path.
        let paid = paid_order(merchant_order_id, "late-chain-tx");
        apply_paid_order_tx(
            &conn,
            "order-1",
            "channel",
            1,
            &loaded(),
            &event(&paid),
            &paid,
        )
        .unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "credited"
        );
        assert!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units > 0);
        apply_paid_order_tx(
            &conn,
            "order-1",
            "channel",
            1,
            &loaded(),
            &event(&paid),
            &paid,
        )
        .unwrap();
        let receipts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_topup_receipts WHERE order_id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receipts, 1);
    }

    #[test]
    fn resolved_historical_case_cannot_explain_a_new_manual_hold() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        let mut created = event(&unpaid_order(merchant_order_id, "pending"));
        created.event_type = "order.created".into();
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &created,
            &now(),
        )
        .unwrap();
        conn.execute(
            "UPDATE market_topup_events SET status = 'review_required' WHERE order_id = 'order-1'",
            [],
        )
        .unwrap();
        open_reconciliation_case_tx(&conn, "unknown_gateway_event", "topup", "order-1", &now())
            .unwrap();
        let case_id: String = conn
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases WHERE object_id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::market_reconcile::resolve_case_tx(
            &conn,
            &case_id,
            "owner",
            "older_case_checked",
            &now(),
        )
        .unwrap();
        let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
        let input = RecoverTopupRequest {
            expected_status: snapshot.status,
            expected_updated_at: snapshot.updated_at,
            gateway_order_id: Some("gateway-1".into()),
            reason_code: "gateway_terminal_verified".into(),
            manual_reference: None,
            manual_confirmed: false,
            dry_run: true,
        };
        let gateway = unpaid_order(merchant_order_id, "expired");
        let preview = apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &gateway,
            "owner",
            &now(),
        )
        .unwrap();
        assert!(!preview.can_recover());
        assert!(preview.blockers.contains(&"review_cause_unclassified"));
    }

    #[test]
    fn resolving_an_unclassified_failure_does_not_erase_manual_review_requirement() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        let mut created = event(&unpaid_order(merchant_order_id, "pending"));
        created.event_type = "order.created".into();
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &created,
            &now(),
        )
        .unwrap();
        conn.execute(
            "UPDATE market_topup_events SET status = 'review_required' WHERE order_id = 'order-1'",
            [],
        )
        .unwrap();
        open_reconciliation_case_tx(&conn, "unknown_gateway_event", "topup", "order-1", &now())
            .unwrap();
        open_reconciliation_case_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            &now(),
        )
        .unwrap();
        let failure_case_id: String = conn
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases
                 WHERE object_id = 'order-1' AND code = 'topup_verification_failed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::market_reconcile::resolve_case_tx(
            &conn,
            &failure_case_id,
            "owner",
            "manual_acknowledgement",
            &now(),
        )
        .unwrap();
        let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
        let input = RecoverTopupRequest {
            expected_status: snapshot.status,
            expected_updated_at: snapshot.updated_at,
            gateway_order_id: Some("gateway-1".into()),
            reason_code: "gateway_terminal_verified".into(),
            manual_reference: None,
            manual_confirmed: false,
            dry_run: true,
        };
        let gateway = unpaid_order(merchant_order_id, "expired");
        let preview = apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &gateway,
            "owner",
            &now(),
        )
        .unwrap();
        assert!(preview.can_recover());
        assert!(preview.requires_manual_reference);
    }

    #[test]
    fn gateway_terminal_zero_decimal_is_precision_independent_but_other_evidence_blocks() {
        let mut gateway = unpaid_order("merchant-order", "expired");
        for zero in ["0", "00.0", "0.00000000"] {
            gateway.settled_amount = Some(zero.into());
            assert!(
                gateway_terminal_without_payment_evidence(&gateway),
                "{zero}"
            );
        }
        for unsafe_amount in ["0.00000001", "1.00", "0.", "not-a-number"] {
            gateway.settled_amount = Some(unsafe_amount.into());
            assert!(
                !gateway_terminal_without_payment_evidence(&gateway),
                "{unsafe_amount}"
            );
        }
        gateway.settled_amount = None;
        gateway.confirmations = Some(1);
        assert!(!gateway_terminal_without_payment_evidence(&gateway));
    }

    #[test]
    fn gateway_observations_preserve_redacted_payment_markers_without_transaction_ids() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        let mut gateway = unpaid_order(merchant_order_id, "expired");
        gateway.tx_id = Some("sensitive-chain-transaction".into());
        record_gateway_observation_tx(&conn, "order-1", "reconcile", &gateway, &now()).unwrap();
        record_gateway_observation_tx(&conn, "order-1", "reconcile", &gateway, &now()).unwrap();
        let (flags, count, hash): (String, i64, String) = conn
            .query_row(
                "SELECT evidence_flags_json, observation_count, evidence_sha256
             FROM market_topup_gateway_observations WHERE order_id = 'order-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(flags, "[\"gateway_transaction\"]");
        assert_eq!(count, 2);
        assert_eq!(hash.len(), 64);
        assert!(!flags.contains("sensitive-chain-transaction"));
        assert!(!hash.contains("sensitive-chain-transaction"));
    }

    #[test]
    fn gateway_observations_page_without_hiding_older_evidence_or_crossing_orders() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        fixture_topup(
            &conn,
            "order-2",
            "router-00000000000000000000000000000002",
            "review_required",
            1,
        );
        for index in 0..52 {
            let mut gateway = unpaid_order(merchant_order_id, "expired");
            gateway.tx_id = Some(format!("transaction-{index}"));
            record_gateway_observation_tx(&conn, "order-1", "reconcile", &gateway, &now()).unwrap();
        }
        let first = admin_topup_observations_tx(&conn, "order-1", None).unwrap();
        assert_eq!(first.items.len(), 50);
        let cursor = first.next_cursor.expect("older observations available");
        let mut repeated = unpaid_order(merchant_order_id, "expired");
        repeated.tx_id = Some("transaction-0".into());
        let later = (Utc::now() + chrono::Duration::days(1)).to_rfc3339();
        record_gateway_observation_tx(&conn, "order-1", "reconcile", &repeated, &later).unwrap();
        let second = admin_topup_observations_tx(&conn, "order-1", Some(&cursor)).unwrap();
        assert_eq!(second.items.len(), 2);
        assert!(second.next_cursor.is_none());
        let ids = first
            .items
            .into_iter()
            .chain(second.items)
            .map(|item| item.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 52);
        assert!(matches!(
            admin_topup_observations_tx(&conn, "order-2", Some(&cursor)),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn unresolved_topups_remain_in_reconciliation_after_the_terminal_history_window() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "old-review",
            "router-00000000000000000000000000000001",
            "review_required",
            1,
        );
        conn.execute(
            "UPDATE market_topup_orders SET created_at = '2020-01-01',
             updated_at = '2020-01-01', next_reconcile_at = '2020-01-01'
             WHERE id = 'old-review'",
            [],
        )
        .unwrap();
        assert!(
            pending_topups_tx(&conn)
                .unwrap()
                .iter()
                .any(|order| order.id == "old-review")
        );
    }

    #[test]
    fn stale_pending_order_can_close_only_after_terminal_gateway_check() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
        assert!(queue_buyer_recheck_tx(&conn, "order-1", "buyer").unwrap());
        let mut input = RecoverTopupRequest {
            expected_status: snapshot.status,
            expected_updated_at: snapshot.updated_at,
            gateway_order_id: Some("gateway-1".into()),
            reason_code: "gateway_terminal_verified".into(),
            manual_reference: None,
            manual_confirmed: false,
            dry_run: true,
        };
        let pending = unpaid_order(merchant_order_id, "pending");
        let preview = apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &pending,
            "owner",
            &now(),
        )
        .unwrap();
        assert!(!preview.can_recover());
        assert!(preview.blockers.contains(&"gateway_not_terminal"));
        input.dry_run = false;
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &pending,
                "owner",
                &now()
            ),
            Err(AppError::Conflict(_))
        ));
        let expired = unpaid_order(merchant_order_id, "expired");
        assert!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &expired,
                "owner",
                &now()
            )
            .unwrap()
            .can_recover()
        );
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "expired"
        );
        assert_eq!(
            read_topup(&conn, "order-1", "buyer")
                .unwrap()
                .buyer_recheck_result
                .as_deref(),
            Some("expired")
        );
    }

    #[test]
    fn known_payment_and_identity_failures_are_not_legacy_manual_release_candidates() {
        for detail_code in [
            "paid_receipt_invalid",
            "gateway_identity_mismatch",
            "created_order_conflict",
        ] {
            let conn = memory();
            fixture_revision(&conn, 1);
            let merchant_order_id = "router-00000000000000000000000000000001";
            fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
            open_reconciliation_case_detail_tx(
                &conn,
                "topup_verification_failed",
                "topup",
                "order-1",
                Some(detail_code),
                &now(),
            )
            .unwrap();
            let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
            let input = RecoverTopupRequest {
                expected_status: snapshot.status,
                expected_updated_at: snapshot.updated_at,
                gateway_order_id: Some("gateway-1".into()),
                reason_code: "gateway_terminal_verified".into(),
                manual_reference: Some("merchant-case-1234".into()),
                manual_confirmed: true,
                dry_run: true,
            };
            let gateway = unpaid_order(merchant_order_id, "expired");
            let preview = apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now(),
            )
            .unwrap();
            assert!(!preview.can_recover(), "{detail_code}");
            assert!(!preview.requires_manual_reference, "{detail_code}");
            assert_eq!(
                read_topup(&conn, "order-1", "buyer").unwrap().status,
                "review_required"
            );
        }
    }

    #[test]
    fn historical_terminal_payment_hint_needs_fresh_clean_gateway_and_owner_attestation() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        let mut expired_event = event(&unpaid_order(merchant_order_id, "expired"));
        expired_event.event_type = "order.expired".into();
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &expired_event,
            &now(),
        )
        .unwrap();
        conn.execute(
            "UPDATE market_topup_events SET status = 'review_required' WHERE order_id = 'order-1'",
            [],
        )
        .unwrap();
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_terminal_payment_evidence"),
            &now(),
        )
        .unwrap();
        let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
        let mut input = RecoverTopupRequest {
            expected_status: snapshot.status,
            expected_updated_at: snapshot.updated_at,
            gateway_order_id: Some("gateway-1".into()),
            reason_code: "merchant_verified_unpaid".into(),
            manual_reference: None,
            manual_confirmed: false,
            dry_run: true,
        };
        let mut suspicious = unpaid_order(merchant_order_id, "expired");
        suspicious.tx_id = Some("possible-late-transfer".into());
        let blocked = apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &suspicious,
            "owner",
            &now(),
        )
        .unwrap();
        assert!(!blocked.can_recover());
        assert!(blocked.blockers.contains(&"gateway_transaction"));
        let clean = unpaid_order(merchant_order_id, "expired");
        let preview = apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &clean,
            "owner",
            &now(),
        )
        .unwrap();
        assert!(preview.can_recover());
        assert!(preview.requires_manual_reference);
        input.dry_run = false;
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &clean,
                "owner",
                &now(),
            ),
            Err(AppError::BadRequest(_))
        ));
        input.manual_reference = Some("merchant-check-20261020".into());
        input.manual_confirmed = true;
        apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &clean,
            "owner",
            &now(),
        )
        .unwrap();
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "expired"
        );
        assert!(!topup_has_manual_hold_tx(&conn, "order-1").unwrap());
        let decision: (String, bool) = conn
            .query_row(
                "SELECT manual_reference, manual_confirmed
                 FROM market_topup_recovery_decisions WHERE order_id = 'order-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(decision, ("merchant-check-20261020".into(), true));
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
    }

    #[test]
    fn owner_recovery_preview_rejects_gateway_identity_and_stale_order_version() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
        let mut input = RecoverTopupRequest {
            expected_status: snapshot.status,
            expected_updated_at: snapshot.updated_at,
            gateway_order_id: Some("gateway-1".into()),
            reason_code: "gateway_terminal_verified".into(),
            manual_reference: None,
            manual_confirmed: false,
            dry_run: true,
        };
        let mut gateway = unpaid_order(merchant_order_id, "expired");
        gateway.fiat_amount_minor = 101;
        let preview = apply_owner_verified_recovery_tx(
            &conn,
            "order-1",
            &input,
            &loaded(),
            &gateway,
            "owner",
            &now(),
        )
        .unwrap();
        assert!(!preview.can_recover());
        assert!(preview.blockers.contains(&"gateway_identity_mismatch"));

        gateway.fiat_amount_minor = 100;
        assert!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now(),
            )
            .unwrap()
            .can_recover()
        );
        conn.execute(
            "UPDATE market_topup_orders SET updated_at = '2026-10-02' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        input.dry_run = false;
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now(),
            ),
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "pending"
        );
    }

    #[test]
    fn owner_recovery_never_ignores_local_payment_or_refund_evidence() {
        for evidence in ["paid_event", "refund_event", "receipt", "journal"] {
            let conn = memory();
            fixture_revision(&conn, 1);
            let merchant_order_id = "router-00000000000000000000000000000001";
            fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
            match evidence {
                "paid_event" | "refund_event" => {
                    let mut signed = event(&paid_order(merchant_order_id, "chain-tx"));
                    if evidence == "refund_event" {
                        signed.event_type = "refund.recorded".into();
                    }
                    persist_signed_event_tx(
                        &conn,
                        "order-1",
                        &loaded().merchant_fingerprint,
                        &signed,
                        &now(),
                    )
                    .unwrap();
                    conn.execute(
                        "UPDATE market_topup_events SET status = 'processed' WHERE order_id = 'order-1'",
                        [],
                    )
                    .unwrap();
                }
                "receipt" => {
                    conn.execute(
                        "INSERT INTO market_topup_receipts
                         (order_id, chain, token_id, tx_id, settled_amount, confirmations,
                          receipt_fingerprint, observed_at)
                         VALUES ('order-1', 'tron', 'tron-usdt', 'chain-tx', '1.00', 1,
                                 'receipt', '2026-10-01')",
                        [],
                    )
                    .unwrap();
                }
                "journal" => {
                    credit_verified_topup_tx(&conn, "buyer", "order-1", 100, "receipt", &now())
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
            let input = RecoverTopupRequest {
                expected_status: snapshot.status,
                expected_updated_at: snapshot.updated_at,
                gateway_order_id: Some("gateway-1".into()),
                reason_code: "gateway_terminal_verified".into(),
                manual_reference: None,
                manual_confirmed: false,
                dry_run: true,
            };
            let gateway = unpaid_order(merchant_order_id, "expired");
            let preview = apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now(),
            )
            .unwrap();
            assert!(!preview.can_recover(), "{evidence}");
            assert!(preview.blockers.iter().any(|blocker| matches!(
                *blocker,
                "payment_or_unknown_event" | "local_receipt_present" | "local_journal_present"
            )));
        }
    }

    #[test]
    fn admin_topup_pages_reach_every_order_after_the_first_fifty() {
        let conn = memory();
        fixture_revision(&conn, 1);
        for index in 0..55 {
            fixture_topup(
                &conn,
                &format!("review-{index:03}"),
                &format!("router-{index:032}"),
                "review_required",
                1,
            );
        }
        fixture_topup(
            &conn,
            "fresh-pending",
            "router-ffffffffffffffffffffffffffffffff",
            "pending",
            1,
        );
        conn.execute(
            "UPDATE market_topup_orders SET updated_at = ?1 WHERE id = 'fresh-pending'",
            params![now()],
        )
        .unwrap();
        fixture_topup(
            &conn,
            "stale-pending",
            "router-eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "pending",
            1,
        );
        let first = admin_topups_tx(&conn, None).unwrap();
        assert_eq!(first.items.len(), 50);
        assert!(first.next_cursor.is_some());
        conn.execute(
            "UPDATE market_topup_orders SET status = 'expired' WHERE id = ?1",
            params![first.next_cursor.as_deref()],
        )
        .unwrap();
        let second = admin_topups_tx(&conn, first.next_cursor.as_deref()).unwrap();
        assert_eq!(second.items.len(), 6);
        assert!(second.next_cursor.is_none());
        assert_eq!(first.items[0].id, "review-000");
        assert!(second.items.iter().any(|item| item.id == "stale-pending"));
        assert!(
            !first
                .items
                .iter()
                .chain(&second.items)
                .any(|item| item.id == "fresh-pending")
        );
        assert!(admin_topups_tx(&conn, Some("unknown-cursor")).is_err());
    }

    #[test]
    fn finance_case_pages_do_not_hide_cases_after_the_first_fifty() {
        let conn = memory();
        for index in 0..56 {
            open_reconciliation_case_tx(
                &conn,
                "test_case",
                "test_object",
                &format!("object-{index:03}"),
                "2026-10-01",
            )
            .unwrap();
        }
        let first = finance_cases_tx(&conn, None).unwrap();
        assert_eq!(first.items.len(), 50);
        crate::market_reconcile::resolve_case_tx(
            &conn,
            first.next_cursor.as_deref().unwrap(),
            "owner",
            "case_checked",
            &now(),
        )
        .unwrap();
        let second = finance_cases_tx(&conn, first.next_cursor.as_deref()).unwrap();
        assert_eq!(second.items.len(), 6);
        assert!(second.next_cursor.is_none());
        let ids = first
            .items
            .iter()
            .chain(&second.items)
            .map(|item| item.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 56);
        assert!(finance_cases_tx(&conn, Some("unknown-cursor")).is_err());
    }

    #[test]
    fn later_payment_evidence_promotes_a_weaker_verification_case() {
        let conn = memory();
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_status_unknown"),
            &now(),
        )
        .unwrap();
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_terminal_payment_evidence"),
            &now(),
        )
        .unwrap();
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_status_unknown"),
            &now(),
        )
        .unwrap();
        let detail: String = conn.query_row(
            "SELECT detail_code FROM market_finance_reconciliation_cases WHERE object_id = 'order-1'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(detail, "gateway_terminal_payment_evidence");
        open_reconciliation_case_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-2",
            &now(),
        )
        .unwrap();
        conn.execute(
            "UPDATE market_finance_reconciliation_cases SET detail_code = ''
             WHERE object_id = 'order-2'",
            [],
        )
        .unwrap();
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-2",
            Some("gateway_terminal_payment_evidence"),
            &now(),
        )
        .unwrap();
        let promoted: String = conn
            .query_row(
                "SELECT detail_code FROM market_finance_reconciliation_cases
                 WHERE object_id = 'order-2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(promoted, "gateway_terminal_payment_evidence");
    }

    #[test]
    fn owner_recovery_rejects_paid_mismatched_and_disputed_orders() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "create_unknown", 1);
        let snapshot = recovery_snapshot_tx(&conn, "order-1").unwrap();
        let input = RecoverTopupRequest {
            expected_status: snapshot.status,
            expected_updated_at: snapshot.updated_at,
            gateway_order_id: Some("gateway-1".into()),
            reason_code: "merchant_terminal_checked".into(),
            manual_reference: None,
            manual_confirmed: false,
            dry_run: false,
        };
        let mut gateway = paid_order(merchant_order_id, "chain-tx");
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now()
            ),
            Err(AppError::Conflict(_))
        ));
        gateway.status = "expired".into();
        assert!(!gateway_terminal_without_payment_evidence(&gateway));
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now()
            ),
            Err(AppError::Conflict(_))
        ));
        gateway = unpaid_order(merchant_order_id, "expired");
        gateway.fiat_amount_minor = 101;
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now()
            ),
            Err(AppError::Conflict(_))
        ));
        gateway.fiat_amount_minor = 100;
        open_reconciliation_case_tx(
            &conn,
            "external_refund_recorded",
            "topup",
            "order-1",
            &now(),
        )
        .unwrap();
        assert!(matches!(
            apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &input,
                &loaded(),
                &gateway,
                "owner",
                &now()
            ),
            Err(AppError::Conflict(_))
        ));
        let mut wrong_id = RecoverTopupRequest {
            gateway_order_id: None,
            ..input
        };
        assert!(
            recovery_gateway_id(&recovery_snapshot_tx(&conn, "order-1").unwrap(), &wrong_id)
                .is_err()
        );
        wrong_id.gateway_order_id = Some("gateway-2".into());
        wrong_id.dry_run = true;
        gateway.order_id = "gateway-2".into();
        assert!(
            !apply_owner_verified_recovery_tx(
                &conn,
                "order-1",
                &wrong_id,
                &loaded(),
                &gateway,
                "owner",
                &now()
            )
            .unwrap()
            .can_recover()
        );
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "create_unknown"
        );
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
    }

    #[test]
    fn only_confirmed_chain_receipts_can_auto_credit() {
        let mut order = GatewayOrder {
            order_id: "do_1".into(),
            merchant_order_id: "mo_1".into(),
            status: "paid".into(),
            fiat_currency: "USD".into(),
            fiat_amount_minor: 2000,
            paid_source: Some("chain".into()),
            chain: Some("tron".into()),
            token_id: Some("tron-usdt".into()),
            tx_id: Some("tx".into()),
            settled_amount: Some("20.00".into()),
            confirmations: Some(1),
            paid_at: None,
            checkout_url: None,
            expires_at: None,
            selection_deadline: None,
            pay_address: None,
            payable_amount: None,
            fx_rate: None,
        };
        assert!(receipt_fingerprint("merchant", &order).is_ok());
        order.paid_source = Some("merchant_claim".into());
        assert!(receipt_fingerprint("merchant", &order).is_err());
        order.paid_source = Some("chain".into());
        order.confirmations = Some(0);
        assert!(receipt_fingerprint("merchant", &order).is_err());
    }

    #[test]
    fn callback_before_create_result_and_late_cancelled_payment_credit_once() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        fixture_revision(&tx, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&tx, "order-1", merchant_order_id, "creating", 1);
        let gateway = paid_order(merchant_order_id, "chain-tx-1");
        let callback = event(&gateway);
        apply_paid_order_tx(&tx, "order-1", "channel", 1, &loaded(), &callback, &gateway).unwrap();
        let created = CreatedOrder {
            order_id: gateway.order_id.clone(),
            checkout_url: Some("https://www.dujiaopay.com/checkout/ct_one".into()),
            expires_at: None,
            ..Default::default()
        };
        save_created_order_tx(&tx, "order-1", 1, &created, &now()).unwrap();
        assert_eq!(
            read_topup(&tx, "order-1", "buyer").unwrap().status,
            "credited"
        );
        apply_paid_order_tx(&tx, "order-1", "channel", 1, &loaded(), &callback, &gateway).unwrap();
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            8_640_000
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();

        fixture_topup(
            &tx,
            "order-2",
            "router-00000000000000000000000000000002",
            "cancelled",
            1,
        );
        tx.execute(
            "UPDATE market_topup_orders SET buyer_abandoned_at = '2026-10-01' WHERE id = 'order-2'",
            [],
        )
        .unwrap();
        let mut late = paid_order("router-00000000000000000000000000000002", "chain-tx-2");
        late.order_id = "gateway-2".into();
        apply_paid_order_tx(
            &tx,
            "order-2",
            "channel",
            1,
            &loaded(),
            &event(&late),
            &late,
        )
        .unwrap();
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            17_280_000
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn mismatched_or_reused_receipt_never_creates_wallet_money() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        fixture_revision(&tx, 1);
        fixture_topup(
            &tx,
            "order-1",
            "router-00000000000000000000000000000001",
            "pending",
            1,
        );
        fixture_topup(
            &tx,
            "order-2",
            "router-00000000000000000000000000000002",
            "pending",
            1,
        );
        let first = paid_order("router-00000000000000000000000000000001", "same-chain-tx");
        let mut wrong_source = first.clone();
        wrong_source.paid_source = Some("merchant_claim".into());
        assert!(
            apply_paid_order_tx(
                &tx,
                "order-1",
                "channel",
                1,
                &loaded(),
                &event(&first),
                &wrong_source
            )
            .is_err()
        );
        let mut wrong_amount = first.clone();
        wrong_amount.fiat_amount_minor = 101;
        assert!(
            apply_paid_order_tx(
                &tx,
                "order-1",
                "channel",
                1,
                &loaded(),
                &event(&first),
                &wrong_amount
            )
            .is_err()
        );
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units, 0);
        apply_paid_order_tx(
            &tx,
            "order-1",
            "channel",
            1,
            &loaded(),
            &event(&first),
            &first,
        )
        .unwrap();
        let second = paid_order("router-00000000000000000000000000000002", "same-chain-tx");
        assert!(
            apply_paid_order_tx(
                &tx,
                "order-2",
                "channel",
                1,
                &loaded(),
                &event(&second),
                &second
            )
            .is_err()
        );
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            8_640_000
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
    }

    #[test]
    fn external_refund_review_blocks_even_a_matching_paid_receipt() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        open_reconciliation_case_tx(
            &conn,
            "external_refund_recorded",
            "topup",
            "order-1",
            &now(),
        )
        .unwrap();
        let paid = paid_order(merchant_order_id, "refund-reviewed-chain-tx");
        assert!(topup_has_manual_hold_tx(&conn, "order-1").unwrap());
        assert!(matches!(
            apply_paid_order_tx(
                &conn,
                "order-1",
                "channel",
                1,
                &loaded(),
                &event(&paid),
                &paid
            ),
            Err(AppError::Conflict(_))
        ));
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
    }

    #[test]
    fn webhook_binding_preserves_early_revisions_after_many_key_rotations() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        for revision in 1..=40 {
            fixture_revision(&tx, revision);
        }
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&tx, "order-1", merchant_order_id, "expired", 1);
        assert_eq!(
            bound_webhook_order_tx(&tx, "channel", merchant_order_id).unwrap(),
            Some(("order-1".into(), 1))
        );
        assert!(
            bound_webhook_order_tx(&tx, "other", merchant_order_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unsigned_hints_only_schedule_bounded_lookups_even_after_seven_days() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "cancelled", 1);
        let two_weeks_ago = (Utc::now() - chrono::Duration::days(14)).to_rfc3339();
        conn.execute(
            "UPDATE market_topup_orders SET created_at = ?2, updated_at = ?2,
             gateway_order_id = 'gateway-1', next_reconcile_at = ?2 WHERE id = ?1",
            params!["order-1", two_weeks_ago],
        )
        .unwrap();
        assert_eq!(pending_topups_tx(&conn).unwrap().len(), 1);
        assert!(claim_reconciliation_tx(&conn, "order-1").unwrap());
        assert!(pending_topups_tx(&conn).unwrap().is_empty());
        assert!(queue_webhook_hint_tx(&conn, "order-1", &now()).unwrap());
        assert!(!queue_webhook_hint_tx(&conn, "order-1", &now()).unwrap());
        let due = pending_topups_tx(&conn).unwrap();
        assert_eq!(due.len(), 1);
        assert!(due[0].had_hint);
        assert!(claim_reconciliation_tx(&conn, "order-1").unwrap());
        finish_reconciliation_tx(&conn, "order-1", due[0].had_hint).unwrap();
        let follow_up: String = conn
            .query_row(
                "SELECT next_reconcile_at FROM market_topup_orders WHERE id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let wait = chrono::DateTime::parse_from_rfc3339(&follow_up)
            .unwrap()
            .signed_duration_since(Utc::now())
            .num_seconds();
        assert!((0..=300).contains(&wait));
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);

        let long_ago = (Utc::now() - chrono::Duration::days(100)).to_rfc3339();
        conn.execute(
            "UPDATE market_topup_orders SET created_at = ?2, next_reconcile_at = ?2,
             next_hint_at = NULL, hint_reconcile_until = ?2,
             reconcile_hint_pending = 0 WHERE id = ?1",
            params!["order-1", long_ago],
        )
        .unwrap();
        assert!(pending_topups_tx(&conn).unwrap().is_empty());
        assert!(queue_webhook_hint_tx(&conn, "order-1", &now()).unwrap());
        assert_eq!(pending_topups_tx(&conn).unwrap().len(), 1);
        assert!(claim_reconciliation_tx(&conn, "order-1").unwrap());
        conn.execute(
            "UPDATE market_topup_orders SET next_reconcile_at = ?2 WHERE id = ?1",
            params!["order-1", long_ago],
        )
        .unwrap();
        assert_eq!(pending_topups_tx(&conn).unwrap().len(), 1);
        conn.execute(
            "UPDATE market_topup_orders SET hint_reconcile_until = ?2 WHERE id = ?1",
            params!["order-1", long_ago],
        )
        .unwrap();
        assert!(pending_topups_tx(&conn).unwrap().is_empty());
    }

    #[test]
    fn failed_gateway_lookup_keeps_a_persisted_retry_lease() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            "router-00000000000000000000000000000001",
            "review_required",
            1,
        );
        assert_eq!(pending_topups_tx(&conn).unwrap().len(), 1);
        assert!(claim_reconciliation_tx(&conn, "order-1").unwrap());
        // A network error after claim cannot consume the work permanently or
        // cause a 30-second hot loop. The lease survives process restarts.
        assert!(pending_topups_tx(&conn).unwrap().is_empty());
        let next: String = conn
            .query_row(
                "SELECT next_reconcile_at FROM market_topup_orders WHERE id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(next > now());
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
    }

    #[test]
    fn legacy_supply_is_not_an_official_topup_readiness_gate() {
        let conn = memory();
        let before = serde_json::to_value(finance_readiness_tx(&conn).unwrap()).unwrap();
        conn.execute("INSERT INTO router_ssh_hosts(id,ip,port,host_owner_email,status,created_at,updated_at,daily_rate_minor,currency) VALUES('old-host','192.0.2.10',22,'provider@example.com','idle','2026-10-01','2026-10-01',100,'USD')", []).unwrap();
        conn.execute(
            "UPDATE router_ssh_hosts SET funding_rail='legacy_provider' WHERE id='old-host'",
            [],
        )
        .unwrap();
        assert_eq!(
            before,
            serde_json::to_value(finance_readiness_tx(&conn).unwrap()).unwrap()
        );
    }

    #[test]
    fn paid_supply_only_changes_rail_on_explicit_price_republication() {
        let conn = memory();
        conn.execute(
            "INSERT INTO share_market_listings
             (id, share_id, installation_id, owner_user_id, owner_email, status, created_at, updated_at)
             VALUES ('listing', 'share', 'installation', 'provider', 'provider@example.com',
                     'active', '2026-10-01', '2026-10-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO share_market_seats
             (id, listing_id, position, status, token_period_json, daily_rate_minor,
              currency, created_at, updated_at)
             VALUES ('seat', 'listing', 1, 'available', '\"day\"', 100,
                     'USD', '2026-10-01', '2026-10-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO router_ssh_hosts
             (id, ip, port, host_owner_email, status, created_at, updated_at,
              daily_rate_minor, currency)
             VALUES ('host', '192.0.2.10', 22, 'provider@example.com', 'idle',
                     '2026-10-01', '2026-10-01', 100, 'USD')",
            [],
        )
        .unwrap();
        let rails = || -> (String, String) {
            (
                conn.query_row(
                    "SELECT funding_rail FROM share_market_seats WHERE id = 'seat'",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
                conn.query_row(
                    "SELECT funding_rail FROM router_ssh_hosts WHERE id = 'host'",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
            )
        };
        assert_eq!(
            rails(),
            ("platform_wallet".into(), "platform_wallet".into())
        );
        // Simulate immutable publications made before migration 82.
        conn.execute(
            "UPDATE share_market_seats SET funding_rail='legacy_provider' WHERE id='seat'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE router_ssh_hosts SET funding_rail='legacy_provider' WHERE id='host'",
            [],
        )
        .unwrap();
        assert_eq!(
            rails(),
            ("legacy_provider".into(), "legacy_provider".into())
        );
        conn.execute(
            "UPDATE market_finance_control SET mode = 'enabled' WHERE id = 1",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE share_market_seats SET status = 'reserved' WHERE id = 'seat'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE router_ssh_hosts SET status = 'locked' WHERE id = 'host'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE share_market_seats SET status = 'available' WHERE id = 'seat'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE router_ssh_hosts SET status = 'idle' WHERE id = 'host'",
            [],
        )
        .unwrap();
        assert_eq!(
            rails(),
            ("legacy_provider".into(), "legacy_provider".into())
        );
        conn.execute(
            "UPDATE share_market_seats SET daily_rate_minor = 200, offer_revision = offer_revision + 1
             WHERE id = 'seat'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE router_ssh_hosts SET daily_rate_minor = 200, offer_revision = offer_revision + 1
             WHERE id = 'host'",
            [],
        )
        .unwrap();
        assert_eq!(
            rails(),
            ("platform_wallet".into(), "platform_wallet".into())
        );
    }

    #[test]
    fn paid_offer_cannot_cross_payment_rails_without_republication() {
        for product in ["Share", "Client Host"] {
            assert!(require_current_offer_rail("platform_wallet", product).is_ok());
            assert!(matches!(
                require_current_offer_rail("legacy_provider", product),
                Err(AppError::Gone(_))
            ));
            assert!(matches!(
                require_current_offer_rail("unknown", product),
                Err(AppError::Internal(_))
            ));
        }
    }

    #[test]
    fn wallet_history_is_owner_scoped_and_paginates_without_duplicates() {
        let conn = memory();
        let time = "2026-10-01T00:00:00+00:00";
        for index in 0..51 {
            credit_verified_topup_tx(
                &conn,
                "buyer",
                &format!("order-{index}"),
                100,
                &format!("receipt-{index}"),
                time,
            )
            .unwrap();
        }
        credit_verified_topup_tx(&conn, "other", "other-order", 100, "other-receipt", time)
            .unwrap();
        let first = wallet_transactions_tx(&conn, "buyer", None).unwrap();
        assert_eq!(first.items.len(), 50);
        assert!(first.items.iter().all(|item| item.delta_units == 8_640_000));
        let second = wallet_transactions_tx(&conn, "buyer", first.next_cursor.as_deref()).unwrap();
        assert_eq!(second.items.len(), 1);
        assert!(second.next_cursor.is_none());
        assert!(!first.items.iter().any(|item| item.id == second.items[0].id));
        assert!(wallet_transactions_tx(&conn, "buyer", Some("other-order")).is_err());
        let other = wallet_transactions_tx(&conn, "other", None).unwrap();
        assert_eq!(other.items.len(), 1);
    }

    #[test]
    fn topup_history_does_not_expose_other_buyers_orders() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "buyer-order",
            "router-00000000000000000000000000000001",
            "pending",
            1,
        );
        conn.execute(
            "INSERT INTO market_topup_orders
             (id, user_id, channel_id, channel_revision, merchant_fingerprint,
              merchant_order_id, idempotency_key, request_fingerprint, amount_minor,
              status, created_at, updated_at)
             VALUES ('other-order', 'other', 'channel', 1, 'merchant-fingerprint',
                     'router-00000000000000000000000000000002', 'other-key',
                     'channel:100', 100, 'pending', '2026-10-02', '2026-10-02')",
            [],
        )
        .unwrap();
        assert_eq!(topups_tx(&conn, "buyer", None).unwrap().items.len(), 1);
        assert!(topups_tx(&conn, "buyer", Some("other-order")).is_err());
    }

    #[test]
    fn gateway_cancel_is_only_eligible_for_verified_unpaid_pending() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        assert!(cancel_topup_snapshot_tx(&conn, "order-1", Some("other")).is_err());
        let pending = unpaid_order(merchant_order_id, "pending");
        let assessment = recovery_assessment_tx(&conn, "order-1", "pending", &pending).unwrap();
        assert!(can_cancel_gateway_topup(
            "pending",
            "pending",
            &assessment,
            false
        ));
        assert!(!can_cancel_gateway_topup(
            "pending",
            "awaiting_payment",
            &assessment,
            false
        ));
        assert!(!can_cancel_gateway_topup(
            "pending",
            "confirming",
            &assessment,
            false
        ));
        let mut with_payment = pending;
        with_payment.tx_id = Some("observed-transfer".into());
        let assessment =
            recovery_assessment_tx(&conn, "order-1", "pending", &with_payment).unwrap();
        assert!(!can_cancel_gateway_topup(
            "pending",
            "pending",
            &assessment,
            false
        ));
        let paid_event = event(&paid_order(merchant_order_id, "chain-tx"));
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &paid_event,
            &now(),
        )
        .unwrap();
        let assessment = recovery_assessment_tx(
            &conn,
            "order-1",
            "pending",
            &unpaid_order(merchant_order_id, "pending"),
        )
        .unwrap();
        assert!(!can_cancel_gateway_topup(
            "pending",
            "pending",
            &assessment,
            false
        ));
    }

    #[test]
    fn checkout_capability_requires_clean_payable_gateway_evidence_and_a_live_deadline() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        let mut gateway = unpaid_order(merchant_order_id, "pending");
        let assessment = recovery_assessment_tx(&conn, "order-1", "pending", &gateway).unwrap();
        let time = Utc::now();
        assert!(can_pay_gateway_topup(
            "pending",
            &gateway,
            &assessment,
            false,
            false,
            time
        ));
        assert!(!can_pay_gateway_topup(
            "pending",
            &gateway,
            &assessment,
            false,
            true,
            time
        ));
        assert!(!can_pay_gateway_topup(
            "pending",
            &gateway,
            &assessment,
            true,
            false,
            time
        ));
        assert!(!can_pay_gateway_topup(
            "review_required",
            &gateway,
            &assessment,
            false,
            false,
            time
        ));
        gateway.expires_at = Some((time - chrono::Duration::seconds(1)).to_rfc3339());
        assert!(!can_pay_gateway_topup(
            "pending",
            &gateway,
            &assessment,
            false,
            false,
            time
        ));
        gateway.expires_at = None;
        gateway.tx_id = Some("observed-transfer".into());
        let disputed = recovery_assessment_tx(&conn, "order-1", "pending", &gateway).unwrap();
        assert!(!can_pay_gateway_topup(
            "pending", &gateway, &disputed, false, false, time
        ));
    }

    #[test]
    fn live_unpaid_chain_source_is_rail_metadata_but_other_payment_clues_still_block() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        let mut gateway = unpaid_order(merchant_order_id, "awaiting_payment");
        gateway.paid_source = Some("chain".into());
        gateway.confirmations = Some(0);
        let assessment = recovery_assessment_tx(&conn, "order-1", "pending", &gateway).unwrap();
        assert_eq!(assessment.blockers, ["gateway_not_terminal"]);
        assert_eq!(
            topup_payability_reason(
                "pending",
                &gateway,
                &assessment,
                false,
                false,
                true,
                Utc::now()
            ),
            "payable"
        );
        assert_eq!(
            topup_payability_reason(
                "pending",
                &gateway,
                &assessment,
                false,
                false,
                false,
                Utc::now()
            ),
            "checkout_unavailable"
        );
        gateway.status = "expired".into();
        assert!(gateway_terminal_without_payment_evidence(&gateway));
        gateway.status = "awaiting_payment".into();
        gateway.paid_source = Some("merchant_claim".into());
        assert!(gateway_recovery_blockers(&gateway).contains(&"gateway_paid_source"));
        gateway.paid_source = Some("chain".into());
        gateway.tx_id = Some("possible-transfer".into());
        let blockers = gateway_recovery_blockers(&gateway);
        assert!(blockers.contains(&"gateway_paid_source"));
        assert!(blockers.contains(&"gateway_transaction"));
        gateway.tx_id = None;
        gateway.status = "confirming".into();
        assert!(gateway_recovery_blockers(&gateway).contains(&"gateway_paid_source"));
        let processing = recovery_assessment_tx(&conn, "order-1", "pending", &gateway).unwrap();
        assert_eq!(
            topup_payability_reason(
                "pending",
                &gateway,
                &processing,
                false,
                true,
                true,
                Utc::now()
            ),
            "payment_processing"
        );
        gateway.status = "expired".into();
        let closed = recovery_assessment_tx(&conn, "order-1", "pending", &gateway).unwrap();
        assert_eq!(
            topup_payability_reason("pending", &gateway, &closed, false, true, true, Utc::now()),
            "gateway_closed"
        );
    }

    #[test]
    fn only_digest_proven_historical_chain_metadata_review_auto_recovers() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_terminal_payment_evidence"),
            &now(),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_topup_gateway_observations
             (id, order_id, source, gateway_status, evidence_flags_json, evidence_sha256,
              first_observed_at, last_observed_at)
             VALUES ('old-observation', 'order-1', 'reconcile', 'awaiting_payment',
                     '[\"gateway_paid_source\"]', ?1, ?2, ?2)",
            params![
                chain_only_observation_hash("awaiting_payment", None, Some(0)),
                now()
            ],
        )
        .unwrap();
        let mut gateway = unpaid_order(merchant_order_id, "expired");
        gateway.paid_source = Some("chain".into());
        gateway.confirmations = Some(0);
        record_gateway_observation_tx(&conn, "order-1", "reconcile", &gateway, &now()).unwrap();
        assert!(
            recover_benign_chain_metadata_review_tx(&conn, "order-1", &gateway, "expired", &now(),)
                .unwrap()
        );
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "expired"
        );
        let open_cases: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases
             WHERE object_id = 'order-1' AND status = 'open'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(open_cases, 0);
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
        assert!(!recover_benign_chain_metadata_review_tx(
            &conn, "order-1", &gateway, "expired", &now(),
        ).unwrap());
    }

    #[test]
    fn historical_review_with_unexplained_or_unbound_evidence_keeps_its_hold() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_terminal_payment_evidence"),
            &now(),
        )
        .unwrap();
        let unexplained_digest = hex::encode(Sha256::digest(
            serde_json::json!({
                "status": "expired", "paidSource": "merchant_claim", "txId": null,
                "paidAt": null, "confirmations": 0, "settledAmount": null,
            })
            .to_string()
            .as_bytes(),
        ));
        conn.execute(
            "INSERT INTO market_topup_gateway_observations
             (id, order_id, source, gateway_status, evidence_flags_json, evidence_sha256,
              first_observed_at, last_observed_at)
             VALUES ('old-observation', 'order-1', 'reconcile', 'expired',
                     '[\"gateway_paid_source\"]', ?1, ?2, ?2)",
            params![unexplained_digest, now()],
        )
        .unwrap();
        let mut gateway = unpaid_order(merchant_order_id, "expired");
        gateway.paid_source = Some("chain".into());
        gateway.confirmations = Some(0);
        assert!(!recover_benign_chain_metadata_review_tx(
            &conn, "order-1", &gateway, "expired", &now(),
        ).unwrap());
        gateway.tx_id = Some("possible-transfer".into());
        assert!(!recover_benign_chain_metadata_review_tx(
            &conn, "order-1", &gateway, "expired", &now(),
        ).unwrap());
        gateway.tx_id = None;
        conn.execute(
            "UPDATE market_topup_gateway_observations
             SET source = 'owner_review', evidence_sha256 = ?1 WHERE id = 'old-observation'",
            params![chain_only_observation_hash("expired", None, Some(0))],
        )
        .unwrap();
        assert!(!recover_benign_chain_metadata_review_tx(
            &conn, "order-1", &gateway, "expired", &now(),
        ).unwrap());
        conn.execute(
            "UPDATE market_topup_gateway_observations
             SET source = 'reconcile', evidence_flags_json = 'not-json'
             WHERE id = 'old-observation'",
            [],
        )
        .unwrap();
        assert!(!recover_benign_chain_metadata_review_tx(
            &conn, "order-1", &gateway, "expired", &now(),
        ).unwrap());
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "review_required"
        );
    }

    #[test]
    fn verified_gateway_cancel_closes_once_without_creating_wallet_money() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        let gateway = unpaid_order(merchant_order_id, "canceled");
        let first = finish_gateway_cancel_tx(
            &conn,
            "order-1",
            Some("buyer"),
            &loaded(),
            &gateway,
            "buyer",
            &now(),
        )
        .unwrap();
        assert_eq!(first.status, "cancelled");
        assert!(first.checkout_url.is_none());
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
        assert!(
            active_matching_topup_tx(&conn, "buyer", "channel:100")
                .unwrap()
                .is_none()
        );
        finish_gateway_cancel_tx(
            &conn,
            "order-1",
            Some("buyer"),
            &loaded(),
            &gateway,
            "buyer",
            &now(),
        )
        .unwrap();
        let audits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_finance_audit_events
                 WHERE object_id = 'order-1' AND action = 'topup_gateway_unpaid_terminal_verified'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audits, 1);

        fixture_topup(
            &conn,
            "order-2",
            "router-00000000000000000000000000000002",
            "pending",
            1,
        );
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-2' WHERE id = 'order-2'",
            [],
        )
        .unwrap();
        let mut expired = unpaid_order("router-00000000000000000000000000000002", "expired");
        expired.order_id = "gateway-2".into();
        let second = finish_gateway_cancel_tx(
            &conn,
            "order-2",
            Some("buyer"),
            &loaded(),
            &expired,
            "buyer",
            &now(),
        )
        .unwrap();
        assert_eq!(second.status, "expired");
        assert_eq!(wallet_snapshot_tx(&conn, "buyer").unwrap().posted_units, 0);
    }

    #[test]
    fn gateway_cancel_does_not_close_a_paid_or_review_held_topup() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        assert!(
            finish_gateway_cancel_tx(
                &conn,
                "order-1",
                Some("buyer"),
                &loaded(),
                &paid_order(merchant_order_id, "chain-tx"),
                "buyer",
                &now(),
            )
            .is_err()
        );
        conn.execute(
            "INSERT INTO market_topup_receipts
             (order_id, chain, token_id, tx_id, settled_amount, confirmations,
              receipt_fingerprint, observed_at)
             VALUES ('order-1', 'tron', 'tron-usdt', 'chain-tx', '1.00', 1,
                     'receipt-1', '2026-10-01')",
            [],
        )
        .unwrap();
        assert!(
            finish_gateway_cancel_tx(
                &conn,
                "order-1",
                Some("buyer"),
                &loaded(),
                &unpaid_order(merchant_order_id, "canceled"),
                "buyer",
                &now(),
            )
            .is_err()
        );
        assert_eq!(
            read_topup(&conn, "order-1", "buyer").unwrap().status,
            "pending"
        );
    }

    #[test]
    fn owner_gateway_cancel_releases_only_a_benign_historical_review_case() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        let mut created = event(&paid_order(merchant_order_id, "unused"));
        created.event_type = "order.created".into();
        persist_signed_event_tx(
            &conn,
            "order-1",
            &loaded().merchant_fingerprint,
            &created,
            &now(),
        )
        .unwrap();
        conn.execute(
            "UPDATE market_topup_events SET status = 'review_required' WHERE order_id = 'order-1'",
            [],
        )
        .unwrap();
        open_reconciliation_case_tx(&conn, "unknown_gateway_event", "topup", "order-1", &now())
            .unwrap();
        let pending = unpaid_order(merchant_order_id, "pending");
        let assessment =
            recovery_assessment_tx(&conn, "order-1", "review_required", &pending).unwrap();
        assert!(can_cancel_gateway_topup(
            "review_required",
            "pending",
            &assessment,
            true
        ));
        assert!(!can_cancel_gateway_topup(
            "review_required",
            "pending",
            &assessment,
            false
        ));
        let view = finish_gateway_cancel_tx(
            &conn,
            "order-1",
            None,
            &loaded(),
            &unpaid_order(merchant_order_id, "canceled"),
            "owner",
            &now(),
        )
        .unwrap();
        assert_eq!(view.status, "cancelled");
        assert!(!topup_has_manual_hold_tx(&conn, "order-1").unwrap());
        let audit_action: String = conn
            .query_row(
                "SELECT action FROM market_finance_audit_events WHERE object_id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audit_action, "topup_gateway_unpaid_terminal_verified");
    }

    #[test]
    fn owner_gateway_cancel_keeps_an_ambiguous_historical_review_case() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "review_required", 1);
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        open_reconciliation_case_detail_tx(
            &conn,
            "topup_verification_failed",
            "topup",
            "order-1",
            Some("gateway_status_unknown"),
            &now(),
        )
        .unwrap();
        let pending = unpaid_order(merchant_order_id, "pending");
        let assessment =
            recovery_assessment_tx(&conn, "order-1", "review_required", &pending).unwrap();
        assert!(assessment.requires_manual_reference);
        assert!(can_cancel_gateway_topup(
            "review_required",
            "pending",
            &assessment,
            true
        ));
        let view = finish_gateway_cancel_tx(
            &conn,
            "order-1",
            None,
            &loaded(),
            &unpaid_order(merchant_order_id, "canceled"),
            "owner",
            &now(),
        )
        .unwrap();
        assert_eq!(view.status, "review_required");
        assert!(topup_has_manual_hold_tx(&conn, "order-1").unwrap());
        let audit_action: String = conn
            .query_row(
                "SELECT action FROM market_finance_audit_events WHERE object_id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audit_action, "topup_gateway_terminal_review_held");
    }

    fn stored_status_and_url(conn: &Connection, id: &str) -> (String, Option<String>) {
        conn.query_row(
            "SELECT status, checkout_url FROM market_topup_orders WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn stopping_payment_hides_but_keeps_the_checkout_and_the_order_stays_pending() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            "router-00000000000000000000000000000001",
            "pending",
            1,
        );
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1',
             checkout_url = 'https://www.dujiaopay.com/checkout/ct_one' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        stop_buyer_topup_tx(&conn, "order-1", "buyer").unwrap();
        // A later authenticated pending lookup used to turn this into
        // create_unknown, which disabled cancellation and auto-close.
        apply_verified_unpaid_status_tx(&conn, "order-1", "pending", &now()).unwrap();
        let (status, url) = stored_status_and_url(&conn, "order-1");
        assert_eq!(status, "pending");
        assert!(url.is_some());
        assert!(
            read_topup(&conn, "order-1", "buyer")
                .unwrap()
                .checkout_url
                .is_none()
        );
    }

    #[test]
    fn cancel_request_marker_hides_checkout_without_erasing_it() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            "router-00000000000000000000000000000001",
            "pending",
            1,
        );
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-1',
             checkout_url = 'https://www.dujiaopay.com/checkout/ct_one' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        mark_gateway_cancel_requested_tx(&conn, "system", "order-1", &now()).unwrap();
        assert!(gateway_cancel_was_requested_tx(&conn, "order-1").unwrap());
        assert!(
            read_topup(&conn, "order-1", "buyer")
                .unwrap()
                .checkout_url
                .is_none()
        );
        apply_verified_unpaid_status_tx(&conn, "order-1", "pending", &now()).unwrap();
        assert_eq!(stored_status_and_url(&conn, "order-1").0, "pending");
        // A genuinely unknown create result still reports create_unknown.
        fixture_topup(
            &conn,
            "order-2",
            "router-00000000000000000000000000000002",
            "creating",
            1,
        );
        apply_verified_unpaid_status_tx(&conn, "order-2", "pending", &now()).unwrap();
        assert_eq!(stored_status_and_url(&conn, "order-2").0, "create_unknown");
    }

    #[test]
    fn a_never_created_order_closes_only_without_gateway_identity_or_evidence() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "lost",
            "router-00000000000000000000000000000001",
            "create_unknown",
            1,
        );
        fixture_topup(
            &conn,
            "bound",
            "router-00000000000000000000000000000002",
            "create_unknown",
            1,
        );
        conn.execute(
            "UPDATE market_topup_orders SET gateway_order_id = 'gateway-2' WHERE id = 'bound'",
            [],
        )
        .unwrap();
        assert!(close_never_created_tx(&conn, "lost", &now()).unwrap());
        assert!(!close_never_created_tx(&conn, "bound", &now()).unwrap());
        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, local_close_reason FROM market_topup_orders WHERE id = 'lost'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (status.as_str(), reason.as_deref()),
            ("cancelled", Some("never_created"))
        );
        conn.execute(
            "UPDATE market_topup_orders SET next_reconcile_at = NULL, updated_at = '2000-01-01'",
            [],
        )
        .unwrap();
        assert!(
            pending_topups_tx(&conn)
                .unwrap()
                .iter()
                .all(|order| order.id != "lost")
        );
        // A closed order no longer counts against the buyer's pending quota.
        ensure_pending_topup_capacity_tx(&conn, "buyer").unwrap();
    }

    fn gateway_error(code: &'static str) -> AppError {
        AppError::Coded {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code,
            message: "test".into(),
            details: serde_json::json!({ "gatewayCode": "test.code" }),
        }
    }

    #[test]
    fn create_failures_back_off_and_an_auth_failure_takes_the_channel_offline() {
        let conn = memory();
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "order-1",
            "router-00000000000000000000000000000001",
            "creating",
            1,
        );
        record_create_failure_tx(
            &conn,
            "order-1",
            "channel",
            &gateway_error(dujiaopay::ERR_RATE_LIMITED),
            &now(),
        )
        .unwrap();
        let (status, attempts, code, next): (String, i64, Option<String>, String) = conn
            .query_row(
                "SELECT status, create_attempts, create_last_error_code, next_reconcile_at
                 FROM market_topup_orders WHERE id = 'order-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!((status.as_str(), attempts), ("create_unknown", 1));
        assert_eq!(code.as_deref(), Some("test.code"));
        assert!(next > now());
        record_create_failure_tx(
            &conn,
            "order-1",
            "channel",
            &gateway_error(dujiaopay::ERR_AUTH_FAILED),
            &now(),
        )
        .unwrap();
        let (rollout, enabled): (String, i64) = conn
            .query_row(
                "SELECT rollout_state, enabled FROM market_payment_channels WHERE id = 'channel'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((rollout.as_str(), enabled), ("off", 0));
        let cases: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases
                 WHERE code = 'payment_channel_auth_failed' AND status = 'open'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cases, 1);
    }

    #[test]
    fn a_rotated_production_channel_reenters_through_canary_and_promotes_itself() {
        let conn = memory();
        fixture_revision(&conn, 1);
        conn.execute(
            "UPDATE market_finance_control SET mode = 'enabled', cutover_approved_at = '2026-10-01'
             WHERE id = 1",
            [],
        )
        .unwrap();
        // A new revision: no live evidence yet, channel switched off.
        fixture_revision(&conn, 2);
        conn.execute(
            "UPDATE market_payment_channels SET current_revision = 2, enabled = 0 WHERE id = 'channel'",
            [],
        )
        .unwrap();
        let version: i64 = conn
            .query_row("SELECT version FROM market_payment_channels", [], |row| {
                row.get(0)
            })
            .unwrap();
        set_channel_enabled_tx(&conn, "channel", true, version, "2026-10-05").unwrap();
        let rollout: String = conn
            .query_row(
                "SELECT rollout_state FROM market_payment_channels",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rollout, "canary");
        assert!(topup_channel_tx(&conn, "channel", false).is_err());
        assert_eq!(
            topup_channel_tx(&conn, "channel", true).unwrap().revision,
            2
        );
        assert!(channel_views(&conn, true).unwrap().is_empty());
        assert_eq!(buyer_channel_views(&conn, true).unwrap().len(), 1);
        // A credit alone is not enough in production without a verified
        // signed Webhook for this revision.
        fixture_topup(
            &conn,
            "credited-2",
            &format!("router-{}", "e".repeat(32)),
            "credited",
            2,
        );
        assert!(!promote_channel_if_proven_tx(&conn, "channel", "2026-10-05").unwrap());
        assert!(mark_channel_webhook_verified_tx(&conn, "channel", 2, "2026-10-05").unwrap());
        let (rollout, promoted): (String, Option<String>) = conn
            .query_row(
                "SELECT rollout_state, promoted_at FROM market_payment_channels",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(rollout, "live");
        assert!(promoted.is_some());
        assert_eq!(channel_views(&conn, true).unwrap().len(), 1);
    }

    #[test]
    fn finance_pause_reason_names_every_paused_flow() {
        assert_eq!(finance_pause_reason((false, false)), "resumed");
        assert_eq!(finance_pause_reason((true, false)), "orders_paused");
        assert_eq!(
            finance_pause_reason((true, true)),
            "orders_and_topups_paused"
        );
        assert_eq!(finance_pause_reason((false, true)), "topups_paused");
    }

    #[test]
    fn paid_terms_check_rail_confirmations_amount_and_source() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        conn.execute(
            "UPDATE market_topup_orders SET create_request_body = ?2, payable_amount = '1.0003'
             WHERE id = ?1",
            params![
                "order-1",
                r#"{"token_id":"tron-usdt","allowed_methods":[{"chain":"tron","token_id":"tron-usdt"}]}"#
            ],
        )
        .unwrap();
        let detail = |gateway: &GatewayOrder| {
            validate_paid_terms_tx(&conn, "order-1", &loaded(), gateway)
                .err()
                .and_then(|error| paid_review_detail(&error))
        };
        let mut gateway = paid_order(merchant_order_id, "tx-1");
        gateway.settled_amount = Some("1.0003".into());
        assert_eq!(detail(&gateway), None);
        gateway.settled_amount = Some("1.00".into());
        assert_eq!(
            detail(&gateway),
            None,
            "a fiat-denominated settlement is accepted"
        );
        gateway.settled_amount = Some("0.9".into());
        assert_eq!(detail(&gateway), Some("paid_amount_short"));
        gateway.settled_amount = Some("1.0003".into());
        gateway.confirmations = Some(5);
        assert_eq!(detail(&gateway), Some("paid_confirmations_low"));
        gateway.confirmations = Some(19);
        gateway.token_id = Some("bsc-usdt".into());
        gateway.chain = Some("bsc".into());
        assert_eq!(detail(&gateway), Some("paid_method_not_allowed"));
        gateway.token_id = Some("tron-usdt".into());
        gateway.chain = Some("ethereum".into());
        assert_eq!(detail(&gateway), Some("paid_method_not_allowed"));
        gateway.chain = Some("tron".into());
        gateway.paid_source = Some("merchant_claim".into());
        assert_eq!(detail(&gateway), Some("paid_source_manual"));

        // A token with no rail entry cannot have its chain or confirmations
        // checked, even when the order allows any method.
        conn.execute(
            "UPDATE market_topup_orders SET create_request_body = '{}' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        let mut unlisted = paid_order(merchant_order_id, "tx-2");
        unlisted.token_id = Some("tron-usdt-trc20".into());
        unlisted.confirmations = Some(1);
        assert_eq!(detail(&unlisted), Some("paid_method_not_allowed"));

        // On a non-stablecoin rail the fiat figure is a different unit and
        // never excuses a short payment.
        conn.execute(
            "UPDATE market_topup_orders SET payable_amount = '4.0' WHERE id = 'order-1'",
            [],
        )
        .unwrap();
        let mut trx = paid_order(merchant_order_id, "tx-3");
        trx.token_id = Some("tron-trx".into());
        trx.fiat_amount_minor = 100;
        trx.settled_amount = Some("1.00".into());
        assert_eq!(detail(&trx), Some("paid_amount_short"));
        trx.settled_amount = Some("4".into());
        assert_eq!(detail(&trx), None);
    }

    #[test]
    fn legacy_revision_methods_still_govern_orders_without_a_frozen_body() {
        let conn = memory();
        fixture_revision(&conn, 1);
        let merchant_order_id = "router-00000000000000000000000000000001";
        fixture_topup(&conn, "order-1", merchant_order_id, "pending", 1);
        let mut revision = loaded();
        revision.allowed_methods = vec!["tron-usdt".into()];
        let mut gateway = paid_order(merchant_order_id, "tx-1");
        assert!(validate_paid_terms_tx(&conn, "order-1", &revision, &gateway).is_ok());
        gateway.token_id = Some("solana-usdc".into());
        gateway.chain = Some("solana".into());
        gateway.confirmations = Some(40);
        assert!(validate_paid_terms_tx(&conn, "order-1", &revision, &gateway).is_err());
    }

    #[test]
    fn rewrap_reseals_every_revision_with_the_newest_key() {
        let conn = memory();
        let old = MarketPaymentsRuntime {
            cipher: PaymentCipher::for_test_ring(&[(1, 7)]),
            ..MarketPaymentsRuntime::for_test()
        };
        seed_live_revision(
            &conn,
            &old,
            1,
            "key",
            "api-secret-0000000",
            Some("webhook-secret-000"),
        );
        let ring = MarketPaymentsRuntime {
            cipher: PaymentCipher::for_test_ring(&[(1, 7), (2, 9)]),
            ..MarketPaymentsRuntime::for_test()
        };
        assert_eq!(rewrap_channel_secrets_tx(&conn, &ring).unwrap(), 1);
        assert_eq!(rewrap_channel_secrets_tx(&conn, &ring).unwrap(), 0);
        let only_new = MarketPaymentsRuntime {
            cipher: PaymentCipher::for_test_ring(&[(2, 9)]),
            ..MarketPaymentsRuntime::for_test()
        };
        let loaded = load_revision(&conn, &only_new, "channel", 1).unwrap();
        assert_eq!(loaded.credentials.api_secret.as_str(), "api-secret-0000000");
        assert!(loaded.credentials.webhook_secret.is_some());
    }
}
