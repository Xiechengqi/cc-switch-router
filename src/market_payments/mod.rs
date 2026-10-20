mod crypto;
mod dujiaopay;

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
    pub fn from_data_dir(data_dir: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self {
            cipher: PaymentCipher::load(data_dir)?,
            client: DujiaoPayClient::production()?,
            webhook_limiter: Arc::new(WebhookLimiter::default()),
            topup_limiter: Arc::new(WebhookLimiter::default()),
        })
    }

    #[cfg(test)]
    pub fn for_test() -> Self {
        Self {
            cipher: PaymentCipher::for_test(),
            client: DujiaoPayClient::production().unwrap(),
            webhook_limiter: Arc::new(WebhookLimiter::default()),
            topup_limiter: Arc::new(WebhookLimiter::default()),
        }
    }

    pub fn allow_webhook_ip(&self, ip: IpAddr) -> bool {
        self.webhook_limiter
            .allow(ip, StdDuration::from_secs(60), 60)
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

/// A raw mode edit cannot authorize wallet purchases. Existing contracts
/// still dispatch by their own funding rail.
pub(crate) fn wallet_purchase_mode_tx(conn: &Connection) -> Result<bool, AppError> {
    let (enabled, approved): (bool, bool) = conn
        .query_row(
            "SELECT mode = 'enabled', cutover_approved_at IS NOT NULL
             FROM market_finance_control WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read market finance purchase mode"))?;
    if enabled && (!MARKET_WALLET_CUTOVER_IMPLEMENTED || !approved) {
        return Err(AppError::ServiceUnavailable(
            "platform wallet cutover has not been approved".into(),
        ));
    }
    if approved && !enabled {
        return Err(AppError::ServiceUnavailable(
            "platform wallet cutover cannot return to the legacy payment rail".into(),
        ));
    }
    Ok(enabled)
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

pub(crate) fn require_new_wallet_withdrawals_open_tx(conn: &Connection) -> Result<(), AppError> {
    let paused: bool = conn
        .query_row(
            "SELECT new_withdrawals_paused FROM market_finance_control WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(db_error("read wallet withdrawal pause"))?;
    if paused {
        return Err(AppError::ServiceUnavailable(
            "new Provider withdrawals are paused by the Router owner".into(),
        ));
    }
    Ok(())
}

/// An offer's publication rail is immutable until the Provider explicitly
/// edits its paid terms. A mode switch must not reinterpret an old offer.
pub(crate) fn require_current_offer_rail(
    funding_rail: &str,
    wallet_mode: bool,
    product: &str,
) -> Result<(), AppError> {
    if !matches!(funding_rail, "legacy_provider" | "platform_wallet") {
        return Err(AppError::Internal(format!(
            "stored {product} offer funding rail is invalid"
        )));
    }
    if (funding_rail == "platform_wallet") != wallet_mode {
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
            "/v1/market-wallet/topups/:id/cancel",
            post(cancel_buyer_topup),
        )
        .route(
            "/v1/market-wallet/topups/:id/abandon",
            post(abandon_buyer_topup),
        )
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
            "/v1/admin/market-finance/readiness",
            get(get_finance_readiness),
        )
        .route("/v1/admin/market-finance/cases", get(list_finance_cases))
        .route(
            "/v1/admin/market-finance/cases/:id/resolve",
            post(resolve_finance_case),
        )
        .route("/v1/admin/market-finance/topups", get(list_admin_topups))
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
    new_withdrawals_paused: bool,
    code_integrated: bool,
    legacy_blockers: Vec<ReadinessBlocker>,
    critical_cases: i64,
    verified_live_topups: i64,
    payment_channel_blockers: Vec<ReadinessBlocker>,
    can_enable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    webhook_url: Option<String>,
}

fn finance_readiness_tx(conn: &Connection) -> Result<FinanceReadinessView, AppError> {
    let (mode, version, new_orders_paused, new_withdrawals_paused): (String, i64, bool, bool) =
        conn.query_row(
            "SELECT mode, version, new_orders_paused, new_withdrawals_paused
             FROM market_finance_control WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(db_error("read market finance mode"))?;
    let checks: [(&'static str, &'static str); 15] = [
        (
            "legacy_share_paid_offers",
            "SELECT COUNT(*) FROM share_market_seats seat
             JOIN share_market_listings listing ON listing.id = seat.listing_id
             WHERE listing.status = 'active' AND listing.deleted_at IS NULL
               AND seat.status = 'available'
               AND (seat.daily_rate_minor IS NOT NULL OR seat.cycle_price_minor IS NOT NULL)
               AND seat.funding_rail = 'legacy_provider'",
        ),
        (
            "legacy_client_paid_offers",
            "SELECT COUNT(*) FROM router_ssh_hosts
             WHERE status = 'idle'
               AND (daily_rate_minor IS NOT NULL OR cycle_price_minor IS NOT NULL)
               AND funding_rail = 'legacy_provider'",
        ),
        (
            "legacy_active_share_quotes",
            "SELECT COUNT(*) FROM share_market_rent_quotes
             WHERE status = 'active' AND pricing_model != 'free'
               AND CASE WHEN json_valid(snapshot_json)
                        THEN COALESCE(json_extract(snapshot_json, '$.fundingRail'),
                                      'legacy_provider')
                        ELSE 'legacy_provider' END = 'legacy_provider'",
        ),
        (
            "legacy_active_client_quotes",
            "SELECT COUNT(*) FROM client_market_allocation_quotes quote
             JOIN client_market_allocation_quote_items item ON item.quote_id = quote.id
             WHERE quote.status = 'active'
               AND quote.funding_rail = 'legacy_provider'
               AND (item.daily_rate_minor IS NOT NULL OR item.cycle_price_minor IS NOT NULL)",
        ),
        (
            "legacy_metered_contracts",
            "SELECT COUNT(*) FROM market_service_contracts WHERE status != 'terminated'",
        ),
        (
            "legacy_monthly_contracts",
            "SELECT COUNT(*) FROM market_recurring_contracts WHERE status NOT IN ('ended', 'activation_failed')",
        ),
        (
            "legacy_prepaid_funds",
            "SELECT COUNT(*) FROM market_prepaid_accounts WHERE posted_balance_units > 0 OR held_balance_units > 0",
        ),
        (
            "legacy_credit_debt",
            "SELECT COUNT(*) FROM market_credit_accounts WHERE balance_units > 0",
        ),
        (
            "legacy_open_invoices",
            "SELECT COUNT(*) FROM market_invoices WHERE status NOT IN ('paid', 'void')",
        ),
        (
            "legacy_refund_obligations",
            "SELECT COUNT(*) FROM market_refund_obligations WHERE status != 'recorded'",
        ),
        (
            "legacy_prepaid_refunds",
            "SELECT COUNT(*) FROM market_prepaid_refund_requests WHERE status IN ('requested', 'approved')",
        ),
        (
            "legacy_funding_intents",
            "SELECT COUNT(*) FROM market_funding_intents WHERE status IN ('pending', 'review_required')",
        ),
        (
            "legacy_billing_disputes",
            "SELECT COUNT(*) FROM market_billing_disputes WHERE status NOT IN ('resolved', 'rejected')",
        ),
        (
            "legacy_payment_cases",
            "SELECT COUNT(*) FROM market_payment_reconciliation_cases WHERE status = 'open'",
        ),
        (
            "legacy_active_reservations",
            "SELECT COUNT(*) FROM market_funding_reservations WHERE status = 'active'",
        ),
    ];
    let mut legacy_blockers = Vec::new();
    for (code, sql) in checks {
        let count: i64 = conn
            .query_row(sql, [], |row| row.get(0))
            .map_err(db_error("read legacy cutover blocker"))?;
        if count > 0 {
            legacy_blockers.push(ReadinessBlocker { code, count });
        }
    }
    let verified_live_topups: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market_verified_topup_evidence",
            [],
            |row| row.get(0),
        )
        .map_err(db_error("count verified top-ups"))?;
    let critical_cases = crate::market_reconcile::critical_case_count_tx(conn)?;
    let channel_checks: [(&'static str, &'static str); 6] = [
        (
            "credited_topup_missing_evidence",
            "SELECT COUNT(*) FROM market_topup_orders topup
             WHERE topup.status = 'credited'
               AND NOT EXISTS (SELECT 1 FROM market_verified_topup_evidence evidence
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
            "channel_no_current_live_topup",
            "SELECT COUNT(*) FROM market_payment_channels c WHERE c.enabled = 1 AND c.archived_at IS NULL AND NOT EXISTS (SELECT 1 FROM market_verified_topup_evidence t WHERE t.channel_id = c.id AND t.channel_revision = c.current_revision)",
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
        && legacy_blockers.is_empty()
        && critical_cases == 0
        && verified_live_topups > 0
        && payment_channel_blockers.is_empty();
    Ok(FinanceReadinessView {
        mode,
        version,
        new_orders_paused,
        new_withdrawals_paused,
        code_integrated: MARKET_WALLET_CUTOVER_IMPLEMENTED,
        legacy_blockers,
        critical_cases,
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
    let (current_mode, was_approved): (String, bool) = conn
        .query_row(
            "SELECT mode, cutover_approved_at IS NOT NULL FROM market_finance_control WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read market finance mode"))?;
    if was_approved && (request.mode != "enabled" || current_mode != "enabled") {
        return Err(AppError::Conflict(
            "platform wallet cutover is permanent; disable payment channels to pause top-ups"
                .into(),
        ));
    }
    if request.mode == "enabled" {
        let readiness = finance_readiness_tx(conn)?;
        if !was_approved && (readiness.mode != "shadow" || !readiness.can_enable) {
            return Err(AppError::Conflict(
                "market wallet cutover gates have not passed".into(),
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
    let session = crate::api::require_admin_session(&state, &headers).await?;
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
    new_withdrawals_paused: bool,
    expected_version: i64,
}

fn set_finance_pause_tx(
    conn: &Connection,
    request: &FinancePauseRequest,
    changed_at: &str,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE market_finance_control
             SET new_orders_paused = ?2, new_withdrawals_paused = ?3,
                 version = version + 1, updated_at = ?4
             WHERE id = 1 AND version = ?1 AND mode = 'enabled'
               AND cutover_approved_at IS NOT NULL",
            params![
                request.expected_version,
                request.new_orders_paused,
                request.new_withdrawals_paused,
                changed_at
            ],
        )
        .map_err(db_error("update finance pause"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "finance pause requires an enabled wallet and current version".into(),
        ));
    }
    Ok(())
}

async fn set_finance_pause(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<FinancePauseRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin finance pause update"))?;
    let changed_at = now();
    set_finance_pause_tx(&tx, &request, &changed_at)?;
    audit_tx(
        &tx,
        &session.user_id,
        "finance_pause_changed",
        "finance_control",
        "1",
        Some(
            if request.new_orders_paused && request.new_withdrawals_paused {
                "orders_and_withdrawals_paused"
            } else if request.new_orders_paused {
                "orders_paused"
            } else if request.new_withdrawals_paused {
                "withdrawals_paused"
            } else {
                "resumed"
            },
        ),
        &changed_at,
    )?;
    tx.commit()
        .map_err(db_error("commit finance pause update"))?;
    Ok((
        no_store(),
        Json(serde_json::json!({
            "newOrdersPaused": request.new_orders_paused,
            "newWithdrawalsPaused": request.new_withdrawals_paused,
        })),
    ))
}

async fn get_wallet(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let (snapshot, mode, cutover_approved, has_wallet_activity) = {
        let conn = state.store.conn.lock().await;
        let snapshot = wallet_snapshot_tx(&conn, &session.user_id)?;
        let (mode, cutover_approved): (String, bool) = conn
            .query_row(
                "SELECT mode, cutover_approved_at IS NOT NULL
                 FROM market_finance_control WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
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
        (snapshot, mode, cutover_approved, has_wallet_activity)
    };
    let can_topup = user_topup_allowed(
        &mode,
        state.dynamic.read().await.is_admin(&session.email),
        cutover_approved,
    );
    Ok((
        no_store(),
        Json(serde_json::json!({
            "assetCode": snapshot.asset_code,
            "postedUnits": snapshot.posted_units.to_string(),
            "heldUnits": snapshot.held_units.to_string(),
            "availableUnits": snapshot.available_units.to_string(),
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
                    journal.created_at
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuyerChannelView {
    id: String,
    display_name: String,
    gateway_kind: &'static str,
}

impl From<ChannelView> for BuyerChannelView {
    fn from(channel: ChannelView) -> Self {
        Self {
            id: channel.id,
            display_name: channel.display_name,
            gateway_kind: channel.gateway_kind,
        }
    }
}

fn channel_views(conn: &Connection, only_enabled: bool) -> Result<Vec<ChannelView>, AppError> {
    let sql = if only_enabled {
        "SELECT c.id, c.display_name, c.enabled, c.sort_order, c.current_revision,
                c.version, c.archived_at, (r.verified_at IS NOT NULL AND c.api_reverify_required = 0), r.webhook_mode,
                c.webhook_verified_at,
                (c.webhook_verified_at IS NOT NULL AND c.webhook_verified_revision IS c.current_revision),
                (SELECT COUNT(*) FROM market_topup_orders t
                 WHERE t.channel_id = c.id AND t.status NOT IN ('credited', 'expired', 'cancelled')),
                (SELECT COUNT(*) FROM market_verified_topup_evidence t
                 WHERE t.channel_id = c.id),
                (SELECT COUNT(*) FROM market_verified_topup_evidence t
                 WHERE t.channel_id = c.id AND t.channel_revision = c.current_revision)
         FROM market_payment_channels c
         JOIN market_payment_channel_revisions r
           ON r.channel_id = c.id AND r.revision = c.current_revision
         WHERE c.enabled = 1 AND c.archived_at IS NULL AND c.api_reverify_required = 0 AND r.verified_at IS NOT NULL
         ORDER BY c.sort_order, c.id"
    } else {
        "SELECT c.id, c.display_name, c.enabled, c.sort_order, c.current_revision,
                c.version, c.archived_at, (r.verified_at IS NOT NULL AND c.api_reverify_required = 0), r.webhook_mode,
                c.webhook_verified_at,
                (c.webhook_verified_at IS NOT NULL AND c.webhook_verified_revision IS c.current_revision),
                (SELECT COUNT(*) FROM market_topup_orders t
                 WHERE t.channel_id = c.id AND t.status NOT IN ('credited', 'expired', 'cancelled')),
                (SELECT COUNT(*) FROM market_verified_topup_evidence t
                 WHERE t.channel_id = c.id),
                (SELECT COUNT(*) FROM market_verified_topup_evidence t
                 WHERE t.channel_id = c.id AND t.channel_revision = c.current_revision)
         FROM market_payment_channels c
         LEFT JOIN market_payment_channel_revisions r
           ON r.channel_id = c.id AND r.revision = c.current_revision
         ORDER BY c.archived_at IS NOT NULL, c.sort_order, c.id"
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(db_error("prepare payment channels"))?;
    statement
        .query_map([], |row| {
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
                gateway_kind: "dujiaopay",
            })
        })
        .map_err(db_error("query payment channels"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read payment channels"))
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
    let shadow_admin = mode == "shadow" && state.dynamic.read().await.is_admin(&session.email);
    let conn = state.store.conn.lock().await;
    let channels = if user_topup_allowed(&mode, shadow_admin, cutover_approved) {
        channel_views(&conn, true)?
            .into_iter()
            .map(BuyerChannelView::from)
            .collect()
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
    match (current_mode, next_mode, explicit_next_mode) {
        ("signed", "poll_only", false) => Err(AppError::Conflict(
            "signed webhook channel cannot be replaced without a signature secret".into(),
        )),
        ("signed" | "poll_only", "signed" | "poll_only", _) => Ok(()),
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
    let allowed_methods_json = serde_json::to_string(&input.allowed_methods)
        .map_err(|_| AppError::Internal("encode allowed methods failed".into()))?;
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
    let session = crate::api::require_admin_session(&state, &headers).await?;
    let view = save_channel(&state, &session, None, input).await?;
    Ok((no_store(), Json(view)))
}

async fn replace_channel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpsertChannelRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
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
    let session = crate::api::require_admin_session(&state, &headers).await?;
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
    let session = crate::api::require_admin_session(&state, &headers).await?;
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
    let session = crate::api::require_admin_session(&state, &headers).await?;
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
            "UPDATE market_payment_channels SET enabled = ?2, updated_at = ?3, version = version + 1
         WHERE id = ?1 AND version = ?4 AND archived_at IS NULL
           AND (?2 = 0 OR api_reverify_required = 0)
           AND (?2 = 0 OR (SELECT verified_at FROM market_payment_channel_revisions
                WHERE channel_id = ?1 AND revision = current_revision) IS NOT NULL)
           AND (?2 = 0 OR (SELECT mode FROM market_finance_control WHERE id = 1) != 'enabled'
                OR (webhook_verified_at IS NOT NULL
                    AND webhook_verified_revision = current_revision
                    AND (SELECT webhook_mode FROM market_payment_channel_revisions
                         WHERE channel_id = ?1 AND revision = current_revision) = 'signed'
                    AND EXISTS (SELECT 1 FROM market_verified_topup_evidence t
                                WHERE t.channel_id = ?1 AND t.channel_revision = current_revision)))",
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

async fn set_channel_enabled(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<SetEnabledRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
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
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin finance case resolution"))?;
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
                    amount_minor, status, created_at, updated_at, buyer_abandoned_at
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

fn gateway_cancel_was_requested_tx(conn: &Connection, order_id: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM market_finance_audit_events
         WHERE object_kind = 'topup' AND object_id = ?1
           AND action = 'topup_gateway_cancel_requested')",
        params![order_id],
        |row| row.get(0),
    )
    .map_err(db_error("check earlier gateway cancellation request"))
}

fn automatic_cancel_recently_requested_tx(
    conn: &Connection,
    order_id: &str,
) -> Result<bool, AppError> {
    let requested_at: Option<String> = conn
        .query_row(
            "SELECT created_at FROM market_finance_audit_events
             WHERE object_kind = 'topup' AND object_id = ?1
               AND action = 'topup_gateway_cancel_requested' AND actor_user_id = 'system'
             ORDER BY created_at DESC LIMIT 1",
            params![order_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read automatic gateway cancellation request"))?;
    Ok(requested_at.is_some_and(|value| {
        chrono::DateTime::parse_from_rfc3339(&value)
            .is_ok_and(|time| time > Utc::now() - chrono::Duration::seconds(60))
    }))
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
                    "gateway_terminal_payment_evidence" | "paid_receipt_invalid"
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
    tx.commit()
        .map_err(db_error("commit owner top-up recovery"))?;
    let can_recover = assessment.can_recover();
    let can_cancel = input.dry_run
        && can_cancel_gateway_topup(&snapshot.status, &gateway.status, &assessment, true);
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
    Ok(changed == 1)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateTopupRequest {
    channel_id: String,
    amount_minor: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TopupView {
    id: String,
    channel_id: String,
    amount_minor: i64,
    status: String,
    checkout_url: Option<String>,
    created_at: String,
    auto_close_at: Option<String>,
    buyer_abandoned_at: Option<String>,
    review_reason: Option<String>,
}

fn read_topup(conn: &Connection, order_id: &str, user_id: &str) -> Result<TopupView, AppError> {
    conn.query_row(
        "SELECT id, channel_id, amount_minor, status, checkout_url, created_at, auto_close_at,
                buyer_abandoned_at,
                CASE WHEN status = 'review_required' AND EXISTS (
                    SELECT 1 FROM market_finance_reconciliation_cases review
                    WHERE review.object_kind = 'topup' AND review.object_id = market_topup_orders.id
                      AND review.code = 'topup_verification_failed'
                      AND review.status = 'open'
                      AND review.detail_code IN ('gateway_terminal_payment_evidence', 'paid_receipt_invalid')
                ) THEN 'possible_payment' ELSE NULL END
         FROM market_topup_orders WHERE id = ?1 AND user_id = ?2",
        params![order_id, user_id],
        |row| {
            let status: String = row.get(3)?;
            let buyer_abandoned_at: Option<String> = row.get(7)?;
            Ok(TopupView {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                amount_minor: row.get(2)?,
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
            })
        },
    )
    .map_err(|error| {
        if matches!(error, crate::db::Error::QueryReturnedNoRows) {
            AppError::NotFound("top-up order not found".into())
        } else {
            AppError::Internal(format!("read top-up order failed: {error}"))
        }
    })
}

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
                 expires_at = COALESCE(?4, expires_at), status = CASE
                     WHEN status IN ('credited', 'expired', 'cancelled', 'review_required')
                     THEN status WHEN ?3 IS NULL AND checkout_url IS NULL
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
                revision
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
    let queued = queue_buyer_recheck_tx(&conn, &order_id, &session.user_id)?;
    Ok((no_store(), Json(serde_json::json!({ "queued": queued }))))
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
    queue_webhook_hint_tx(conn, order_id, &now())
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
    can_cancel: bool,
    can_abandon: bool,
    selection_deadline: Option<String>,
    expires_at: Option<String>,
    server_now: String,
}

async fn get_topup_gateway_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let (snapshot, loaded) = {
        let conn = state.store.conn.lock().await;
        let snapshot = cancel_topup_snapshot_tx(&conn, &order_id, Some(&session.user_id))?;
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
        &state,
        &snapshot.channel_id,
        snapshot.revision,
        &loaded,
        &snapshot.gateway_order_id,
    )
    .await?;
    let (can_cancel, can_abandon) = {
        let conn = state.store.conn.lock().await;
        let current = cancel_topup_snapshot_tx(&conn, &order_id, Some(&session.user_id))?;
        validate_gateway_identity_tx(
            &conn,
            &order_id,
            &current.channel_id,
            current.revision,
            &loaded,
            &current.gateway_order_id,
            &gateway,
        )?;
        let assessment = recovery_assessment_tx(&conn, &order_id, &current.status, &gateway)?;
        (
            can_cancel_gateway_topup(&current.status, &gateway.status, &assessment, false)
                && !automatic_cancel_recently_requested_tx(&conn, &order_id)?,
            current.status == "pending"
                && gateway.status == "awaiting_payment"
                && assessment.blockers.as_slice() == ["gateway_not_terminal"]
                && !topup_buyer_abandoned_tx(&conn, &order_id)?,
        )
    };
    Ok((
        no_store(),
        Json(TopupGatewayStatusView {
            gateway_status: gateway.status,
            can_cancel,
            can_abandon,
            selection_deadline: gateway.selection_deadline,
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
        if actor == "system" && gateway_cancel_was_requested_tx(&tx, order_id)? {
            // A prior POST may have reached the gateway even if its response
            // was lost. Poll for its terminal state; never repeat it silently.
            return Err(AppError::Conflict(
                "automatic gateway cancellation was already requested".into(),
            ));
        }
        if automatic_cancel_recently_requested_tx(&tx, order_id)? {
            return Err(AppError::Conflict(
                "automatic gateway cancellation is being verified".into(),
            ));
        }
        audit_tx(
            &tx,
            actor,
            "topup_gateway_cancel_requested",
            "topup",
            order_id,
            None,
            &now(),
        )?;
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
        .prepare(
            "SELECT id, channel_id, amount_minor, status, checkout_url, created_at, auto_close_at,
                    buyer_abandoned_at,
                    CASE WHEN status = 'review_required' AND EXISTS (
                        SELECT 1 FROM market_finance_reconciliation_cases review
                        WHERE review.object_kind = 'topup' AND review.object_id = market_topup_orders.id
                          AND review.code = 'topup_verification_failed'
                          AND review.status = 'open'
                          AND review.detail_code IN ('gateway_terminal_payment_evidence', 'paid_receipt_invalid')
                    ) THEN 'possible_payment' ELSE NULL END
             FROM market_topup_orders WHERE user_id = ?1
               AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id < ?3))
             ORDER BY created_at DESC, id DESC LIMIT 51",
        )
        .map_err(db_error("prepare top-up history"))?;
    let mut items = statement
        .query_map(params![user_id, cursor_at, cursor], |row| {
            let status: String = row.get(3)?;
            let buyer_abandoned_at: Option<String> = row.get(7)?;
            Ok(TopupView {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                amount_minor: row.get(2)?,
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
            })
        })
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
           AND status IN ('created', 'creating', 'create_unknown',
                          'pending', 'verifying', 'review_required')
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
             AND status IN ('created', 'creating', 'create_unknown', 'pending', 'verifying')",
            params![user_id],
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
             WHERE user_id = ?1 AND status = 'review_required'",
            params![user_id],
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
    let shadow_admin = state.dynamic.read().await.is_admin(&session.email);
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
    let request_fingerprint = format!("{}:{}", input.channel_id, input.amount_minor);
    let (order_id, merchant_order_id, revision, loaded, existing_view) = {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin top-up"))?;
        let (mode, cutover_approved): (String, bool) = tx
            .query_row(
                "SELECT mode, cutover_approved_at IS NOT NULL
                 FROM market_finance_control WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(db_error("read market finance mode"))?;
        if mode == "enabled" && (!MARKET_WALLET_CUTOVER_IMPLEMENTED || !cutover_approved) {
            return Err(AppError::ServiceUnavailable(
                "platform wallet cutover has not been approved".into(),
            ));
        }
        if !user_topup_allowed(&mode, shadow_admin, cutover_approved) {
            return Err(AppError::ServiceUnavailable(
                "platform wallet top-up is not enabled".into(),
            ));
        }
        let prior = tx
            .query_row(
                "SELECT id, request_fingerprint, merchant_order_id, channel_revision
             FROM market_topup_orders WHERE user_id = ?1 AND idempotency_key = ?2",
                params![session.user_id, idem],
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
        let (id, merchant_order_id, revision, already_created) = if let Some((
            id,
            fingerprint,
            merchant_order_id,
            revision,
        )) = prior
        {
            if fingerprint != request_fingerprint {
                return Err(AppError::Conflict("top-up idempotency conflict".into()));
            }
            (id, merchant_order_id, revision, true)
        } else {
            crate::market_reconcile::ensure_new_finance_writes_safe_tx(&tx)?;
            let revision = tx.query_row(
                "SELECT current_revision FROM market_payment_channels WHERE id = ?1 AND enabled = 1 AND archived_at IS NULL AND api_reverify_required = 0",
                params![input.channel_id], |row| row.get::<_, i64>(0),
            ).optional().map_err(db_error("read enabled payment channel"))?
                .ok_or_else(|| AppError::BadRequest("payment channel unavailable".into()))?;
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
            (
                Uuid::new_v4().to_string(),
                format!("router-{}", Uuid::new_v4().simple()),
                revision,
                false,
            )
        };
        let loaded = load_revision(&tx, &state.market_payments, &input.channel_id, revision)?;
        let existing_view = already_created
            .then(|| read_topup(&tx, &id, &session.user_id))
            .transpose()?;
        if !already_created {
            let created_at = Utc::now();
            let time = created_at.to_rfc3339();
            tx.execute(
                "INSERT INTO market_topup_orders (
                    id, user_id, channel_id, channel_revision, merchant_fingerprint,
                    merchant_order_id, idempotency_key, request_fingerprint, amount_minor,
                    status, created_at, updated_at, next_reconcile_at, auto_close_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'creating', ?10, ?10, ?11, ?12)",
                params![
                    id,
                    session.user_id,
                    input.channel_id,
                    revision,
                    loaded.merchant_fingerprint,
                    merchant_order_id,
                    idem,
                    request_fingerprint,
                    input.amount_minor,
                    time,
                    (created_at + chrono::Duration::seconds(30)).to_rfc3339(),
                    topup_auto_close_deadline(created_at)
                ],
            )
            .map_err(db_error("insert top-up order"))?;
        }
        tx.commit().map_err(db_error("commit top-up order"))?;
        (id, merchant_order_id, revision, loaded, existing_view)
    };
    if let Some(view) = existing_view
        && !matches!(
            view.status.as_str(),
            "created" | "creating" | "create_unknown"
        )
    {
        return Ok((no_store(), Json(view)));
    }
    let scheme = if state.config.use_localhost {
        "http"
    } else {
        "https"
    };
    let base = format!(
        "{scheme}://{}",
        state.config.tunnel_domain.trim_end_matches('/')
    );
    let (success_url, cancel_url) = topup_return_urls(&base, &order_id);
    let created = state
        .market_payments
        .client
        .create_order(
            &loaded.credentials,
            &merchant_order_id,
            input.amount_minor,
            &loaded.allowed_methods,
            &success_url,
            &cancel_url,
        )
        .await;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin top-up create result"))?;
    let view = match created {
        Ok(created) => {
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
            tx.execute(
                "UPDATE market_topup_orders SET status = 'create_unknown', updated_at = ?2
                 WHERE id = ?1 AND status IN ('created', 'creating', 'create_unknown')",
                params![order_id, now()],
            )
            .map_err(db_error("mark uncertain top-up"))?;
            tx.commit().map_err(db_error("commit uncertain top-up"))?;
            return Err(error);
        }
    };
    tx.commit().map_err(db_error("commit DujiaoPay order"))?;
    if matches!(
        view.status.as_str(),
        "created" | "creating" | "create_unknown" | "pending"
    ) && view.checkout_url.is_none()
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
         WHERE id = ?1 AND status IN ('created', 'creating', 'create_unknown',
                                      'pending', 'verifying')",
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
         SET status = CASE WHEN ?2 = 'pending' AND checkout_url IS NULL
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

fn gateway_recovery_blockers(gateway: &GatewayOrder) -> Vec<&'static str> {
    let mut blockers = Vec::new();
    if !matches!(
        gateway.status.as_str(),
        "expired" | "canceled" | "cancelled"
    ) {
        blockers.push("gateway_not_terminal");
    }
    if gateway.paid_source.is_some() {
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
) -> Result<(String, i64), AppError> {
    conn.query_row(
        "SELECT id, channel_revision FROM market_topup_orders
         WHERE merchant_order_id = ?1 AND channel_id = ?2",
        params![merchant_order_hint, channel_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )
    .optional()
    .map_err(db_error("bind DujiaoPay webhook"))?
    .ok_or_else(|| AppError::Unauthorized("DujiaoPay webhook not recognized".into()))
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

/// The fixed URL is suitable for a merchant dashboard before a channel exists.
/// Its body is never payment evidence: it may only wake a bounded authenticated
/// API lookup of an order already created by this Router.
async fn dujiaopay_webhook_hint(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let Some(merchant_order_hint) = dujiaopay::optional_merchant_order_hint(&body)? else {
        return Ok(Json(
            serde_json::json!({ "accepted": true, "ignored": true }),
        ));
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
        return dujiaopay_webhook(State(state), Path(channel_id), headers, body).await;
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin DujiaoPay callback hint"))?;
    queue_webhook_hint_tx(&tx, &order_id, &now())?;
    tx.commit()
        .map_err(db_error("commit DujiaoPay callback hint"))?;
    Ok(Json(serde_json::json!({ "accepted": true })))
}

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
    let merchant_order_hint = dujiaopay::merchant_order_hint(&body)?;
    let (revision, loaded, event, order_id, verified_current_revision) = {
        let conn = state.store.conn.lock().await;
        let bound = bound_webhook_order_tx(&conn, &channel_id, &merchant_order_hint)?;
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
        (bound.1, loaded, event, bound.0, verified_current_revision)
    };
    {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin webhook event"))?;
        persist_signed_event_tx(&tx, &order_id, &loaded.merchant_fingerprint, &event, &now())?;
        if let Some(current_revision) = verified_current_revision {
            mark_channel_webhook_verified_tx(&tx, &channel_id, current_revision, &now())?;
        }
        // Signed events also need a persisted recovery window: the immediate
        // authenticated lookup may fail, including for an order older than the
        // regular polling horizon.
        queue_webhook_hint_tx(&tx, &order_id, &now())?;
        tx.commit().map_err(db_error("commit DujiaoPay webhook"))?;
    }
    if is_non_payment_lifecycle_event(&event.event_type) {
        // Signed lifecycle callbacks can arrive before the create API returns.
        // Their order ID is a recovery locator, never evidence of payment.
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin created-event binding"))?;
        let time = now();
        let review_required = match bind_signed_gateway_order_tx(
            &tx,
            &order_id,
            &channel_id,
            revision,
            &loaded,
            &event,
        ) {
            Ok(()) => false,
            Err(AppError::Conflict(_)) => {
                hold_topup_verification_failure_tx(
                    &tx,
                    &order_id,
                    "signed_gateway_order_conflict",
                    &time,
                )?;
                true
            }
            Err(error) => return Err(error),
        };
        tx.execute(
            "UPDATE market_topup_events SET status = ?3, processed_at = ?4
             WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2",
            params![
                loaded.merchant_fingerprint,
                event.event_id,
                if review_required {
                    "review_required"
                } else {
                    "processed"
                },
                time
            ],
        )
        .map_err(db_error("finish created payment event"))?;
        tx.commit()
            .map_err(db_error("commit created-event binding"))?;
        return Ok(Json(serde_json::json!({
            "accepted": true,
            "reviewRequired": review_required
        })));
    }
    if !matches!(
        event.event_type.as_str(),
        "order.paid" | "order.expired" | "order.cancelled" | "order.canceled"
    ) {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin unsupported payment event review"))?;
        let time = now();
        let code = if event.event_type == "refund.recorded" {
            "external_refund_recorded"
        } else {
            "unknown_gateway_event"
        };
        open_reconciliation_case_tx(&tx, code, "topup", &order_id, &time)?;
        tx.execute(
            "UPDATE market_topup_orders SET status = 'review_required', updated_at = ?2
             WHERE id = ?1 AND status != 'credited'",
            params![order_id, time],
        )
        .map_err(db_error("hold top-up for event review"))?;
        tx.execute(
            "UPDATE market_topup_events SET status = 'review_required', processed_at = ?3
             WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2",
            params![loaded.merchant_fingerprint, event.event_id, time],
        )
        .map_err(db_error("mark payment event for review"))?;
        tx.commit()
            .map_err(db_error("commit payment event review"))?;
        return Ok(Json(
            serde_json::json!({ "accepted": true, "reviewRequired": true }),
        ));
    }
    let gateway =
        get_bound_gateway_order(&state, &channel_id, revision, &loaded, &event.order_id).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin verified top-up"))?;
    if gateway.status == "paid" && topup_has_manual_hold_tx(&tx, &order_id)? {
        let time = now();
        tx.execute(
            "UPDATE market_topup_orders SET status = 'review_required', updated_at = ?2
             WHERE id = ?1 AND status != 'credited'",
            params![order_id, time],
        )
        .map_err(db_error("hold disputed webhook top-up"))?;
        tx.execute(
            "UPDATE market_topup_events SET status = 'review_required', processed_at = ?3
             WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2",
            params![loaded.merchant_fingerprint, event.event_id, time],
        )
        .map_err(db_error("mark disputed webhook for review"))?;
        tx.commit()
            .map_err(db_error("commit disputed webhook review"))?;
        return Ok(Json(
            serde_json::json!({ "accepted": true, "reviewRequired": true }),
        ));
    }
    if gateway.status != "paid" {
        if let Err(error) = validate_gateway_identity_tx(
            &tx,
            &order_id,
            &channel_id,
            revision,
            &loaded,
            &event.order_id,
            &gateway,
        ) {
            if matches!(error, AppError::Conflict(_)) {
                let time = now();
                hold_topup_verification_failure_tx(
                    &tx,
                    &order_id,
                    "gateway_identity_mismatch",
                    &time,
                )?;
                tx.execute("UPDATE market_topup_events SET status = 'review_required', processed_at = ?3 WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2", params![loaded.merchant_fingerprint, event.event_id, time])
                    .map_err(db_error("hold mismatched gateway identity event"))?;
                tx.commit()
                    .map_err(db_error("commit gateway identity review"))?;
                return Ok(Json(
                    serde_json::json!({ "accepted": true, "reviewRequired": true }),
                ));
            }
            return Err(error);
        }
        let status = match verified_unpaid_status(&gateway.status) {
            Ok(status) => status,
            Err(AppError::Conflict(_)) => {
                let time = now();
                hold_topup_verification_failure_tx(
                    &tx,
                    &order_id,
                    "gateway_status_unknown",
                    &time,
                )?;
                tx.execute("UPDATE market_topup_events SET status = 'review_required', processed_at = ?3 WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2", params![loaded.merchant_fingerprint, event.event_id, time])
                    .map_err(db_error("hold unknown gateway status event"))?;
                tx.commit()
                    .map_err(db_error("commit unknown gateway status review"))?;
                return Ok(Json(
                    serde_json::json!({ "accepted": true, "reviewRequired": true }),
                ));
            }
            Err(error) => return Err(error),
        };
        if matches!(status, "expired" | "cancelled")
            && !gateway_terminal_without_payment_evidence(&gateway)
        {
            let time = now();
            hold_topup_verification_failure_tx(
                &tx,
                &order_id,
                "gateway_terminal_payment_evidence",
                &time,
            )?;
            tx.execute("UPDATE market_topup_events SET status = 'review_required', processed_at = ?3 WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2", params![loaded.merchant_fingerprint, event.event_id, time])
                .map_err(db_error("hold terminal gateway payment evidence"))?;
            tx.commit()
                .map_err(db_error("commit terminal gateway evidence review"))?;
            return Ok(Json(
                serde_json::json!({ "accepted": true, "reviewRequired": true }),
            ));
        }
        bind_signed_gateway_order_tx(&tx, &order_id, &channel_id, revision, &loaded, &event)?;
        restore_verified_checkout_tx(&tx, &order_id, &gateway)?;
        apply_verified_unpaid_status_tx(&tx, &order_id, status, &now())?;
        tx.execute("UPDATE market_topup_events SET status = 'processed', processed_at = ?3 WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2", params![loaded.merchant_fingerprint, event.event_id, now()])
            .map_err(db_error("mark webhook processed"))?;
        tx.commit()
            .map_err(db_error("commit DujiaoPay order status"))?;
        return Ok(Json(serde_json::json!({ "accepted": true })));
    }
    if let Err(error) = apply_paid_order_tx(
        &tx,
        &order_id,
        &channel_id,
        revision,
        &loaded,
        &event,
        &gateway,
    ) {
        if matches!(error, AppError::Conflict(_)) {
            let time = now();
            open_reconciliation_case_detail_tx(
                &tx,
                "topup_verification_failed",
                "topup",
                &order_id,
                Some("paid_receipt_invalid"),
                &time,
            )?;
            tx.execute("UPDATE market_topup_orders SET status = 'review_required', updated_at = ?2 WHERE id = ?1 AND status != 'credited'", params![order_id, time])
                .map_err(db_error("mark top-up for review"))?;
            tx.execute("UPDATE market_topup_events SET status = 'review_required', processed_at = ?3 WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2", params![loaded.merchant_fingerprint, event.event_id, time])
                .map_err(db_error("mark webhook for review"))?;
            tx.commit().map_err(db_error("commit top-up review"))?;
            return Ok(Json(
                serde_json::json!({ "accepted": true, "reviewRequired": true }),
            ));
        }
        return Err(error);
    }
    tx.execute("UPDATE market_topup_events SET status = 'processed', processed_at = ?3 WHERE merchant_fingerprint = ?1 AND gateway_event_id = ?2", params![loaded.merchant_fingerprint, event.event_id, now()])
        .map_err(db_error("mark webhook processed"))?;
    tx.commit().map_err(db_error("commit verified top-up"))?;
    Ok(Json(serde_json::json!({ "accepted": true })))
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
                amount_minor, reconcile_hint_pending, auto_close_at
         FROM market_topup_orders
         WHERE COALESCE(next_reconcile_at, updated_at) <= ?1
           AND status IN ('created', 'creating', 'create_unknown', 'pending', 'verifying',
                          'expired', 'cancelled', 'review_required')
           AND (created_at >= ?2 OR reconcile_hint_pending = 1
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
             reconcile_hint_pending = 0 WHERE id = ?1",
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
    let gateway_id = if let Some(id) = pending.gateway_order_id.clone() {
        id
    } else {
        let scheme = if state.config.use_localhost {
            "http"
        } else {
            "https"
        };
        let base = format!(
            "{scheme}://{}",
            state.config.tunnel_domain.trim_end_matches('/')
        );
        let (success_url, cancel_url) = topup_return_urls(&base, &pending.id);
        let created = state
            .market_payments
            .client
            .create_order(
                &creation_credentials,
                &pending.merchant_order_id,
                pending.amount_minor,
                &loaded.allowed_methods,
                &success_url,
                &cancel_url,
            )
            .await?;
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin top-up recovery"))?;
        if let Err(error) =
            save_created_order_tx(&tx, &pending.id, pending.revision, &created, &now())
        {
            if matches!(error, AppError::Conflict(_)) {
                hold_topup_verification_failure_tx(
                    &tx,
                    &pending.id,
                    "created_order_conflict",
                    &now(),
                )?;
                finish_reconciliation_tx(&tx, &pending.id, pending.had_hint)?;
                tx.commit()
                    .map_err(db_error("commit conflicting top-up recovery"))?;
                return Ok(());
            }
            return Err(error);
        }
        tx.commit().map_err(db_error("commit top-up recovery"))?;
        created.order_id
    };
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
            finish_reconciliation_tx(&tx, &pending.id, pending.had_hint)?;
            tx.commit()
                .map_err(db_error("commit mismatched gateway review"))?;
            return Ok(());
        }
        return Err(error);
    }
    restore_verified_checkout_tx(&tx, &pending.id, &gateway)?;
    let manual_hold = topup_has_manual_hold_tx(&tx, &pending.id)?;
    if gateway.status == "paid" && !manual_hold {
        let validation = validate_paid_order_tx(
            &tx,
            &pending.id,
            &pending.channel_id,
            pending.revision,
            &loaded,
            &event,
            &gateway,
        );
        match validation {
            Ok(_) => apply_paid_order_tx(
                &tx,
                &pending.id,
                &pending.channel_id,
                pending.revision,
                &loaded,
                &event,
                &gateway,
            )?,
            Err(AppError::Conflict(_)) => {
                hold_topup_verification_failure_tx(
                    &tx,
                    &pending.id,
                    "paid_receipt_invalid",
                    &now(),
                )?;
            }
            Err(error) => return Err(error),
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
            Ok(status) => apply_verified_unpaid_status_tx(&tx, &pending.id, status, &now())?,
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
    finish_reconciliation_tx(&tx, &pending.id, pending.had_hint)?;
    let can_auto_close = should_auto_cancel_tx(&tx, &pending, &gateway, Utc::now())?;
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

pub async fn run_service(
    state: ServerState,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut audit_interval = tokio::time::interval(std::time::Duration::from_secs(3_600));
    audit_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = audit_interval.tick() => {
                let result = {
                    let conn = state.store.conn.lock().await;
                    conn.transaction_with_behavior(TransactionBehavior::Immediate)
                        .map_err(db_error("begin market finance audit"))
                        .and_then(|tx| {
                            let findings = crate::market_reconcile::audit_finance_tx(&tx, &now())?;
                            tx.commit().map_err(db_error("commit market finance audit"))?;
                            Ok(findings)
                        })
                };
                match result {
                    Ok(findings) if findings > 0 => tracing::error!(findings, "market finance audit found mismatches"),
                    Err(_) => tracing::error!("market finance audit unavailable"),
                    _ => {}
                }
            }
            _ = interval.tick() => {
                let pending = {
                    let conn = state.store.conn.lock().await;
                    pending_topups_tx(&conn)
                };
                match pending {
                    Ok(orders) => for order in orders {
                        let claimed = {
                            let conn = state.store.conn.lock().await;
                            claim_reconciliation_tx(&conn, &order.id)
                        };
                        if !matches!(claimed, Ok(true)) {
                            if claimed.is_err() { tracing::warn!("market top-up reconciliation claim failed"); }
                            continue;
                        }
                        if let Err(error) = reconcile_one_topup(&state, order).await {
                            tracing::warn!(error_code = error.code().unwrap_or("MARKET_TOPUP_RETRY"),
                                "market top-up recovery deferred");
                        }
                    },
                    Err(_) => tracing::warn!("market top-up queue unavailable"),
                }
                let result = {
                    let conn = state.store.conn.lock().await;
                    conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_error("begin earning maturity"))
                        .and_then(|tx| {
                            let count = crate::market_finance::unlock_due_earnings_tx(&tx, &now(), 100)?;
                            tx.commit().map_err(db_error("commit earning maturity"))?;
                            Ok(count)
                        })
                };
                if result.is_err() { tracing::warn!("market earning maturity deferred"); }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_wallet::{reconcile_wallet_tx, wallet_snapshot_tx};

    fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("open finance DB");
        crate::schema::apply(&conn).expect("apply finance schema");
        conn
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
                 SET buyer_abandoned_at = '2025-01-01', created_at = '2025-01-01',
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
        audit_tx(
            &conn,
            "system",
            "topup_gateway_cancel_requested",
            "topup",
            "order-1",
            None,
            &now(),
        )
        .unwrap();
        assert!(!should_auto_cancel_tx(&conn, &pending, &gateway, time).unwrap());
        assert!(gateway_cancel_was_requested_tx(&conn, "order-1").unwrap());
        assert!(automatic_cancel_recently_requested_tx(&conn, "order-1").unwrap());
        conn.execute(
            "UPDATE market_finance_audit_events SET created_at = '2026-10-01T00:00:00+00:00'
             WHERE object_id = 'order-1' AND action = 'topup_gateway_cancel_requested'",
            [],
        )
        .unwrap();
        assert!(!automatic_cancel_recently_requested_tx(&conn, "order-1").unwrap());
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
            Some("possible_payment".into())
        );
        assert_eq!(
            topups_tx(&conn, "buyer", None).unwrap().items[0].review_reason,
            Some("possible_payment".into())
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
        assert!(!queue_buyer_recheck_tx(&conn, "order-1", "buyer").unwrap());
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
    fn manually_enabled_mode_cannot_bypass_live_cutover_approval() {
        let conn = memory();
        assert!(!wallet_purchase_mode_tx(&conn).unwrap());
        assert!(!user_topup_allowed("disabled", true, false));
        assert!(user_topup_allowed("shadow", true, false));
        assert!(!user_topup_allowed("shadow", false, false));
        assert!(!user_topup_allowed("enabled", false, false));
        assert!(!user_topup_allowed("enabled", true, false));
        conn.execute(
            "UPDATE market_finance_control SET mode = 'enabled' WHERE id = 1",
            [],
        )
        .unwrap();
        assert!(matches!(
            wallet_purchase_mode_tx(&conn),
            Err(AppError::ServiceUnavailable(_))
        ));
        conn.execute(
            "UPDATE market_finance_control SET cutover_approved_at = '2026-10-16T00:00:00Z'
             WHERE id = 1",
            [],
        )
        .unwrap();
        assert!(wallet_purchase_mode_tx(&conn).unwrap());
        assert!(user_topup_allowed("enabled", false, true));
    }

    #[test]
    fn owner_cutover_requires_live_gates_and_never_reopens_legacy_money() {
        let conn = memory();
        let enable = ModeRequest {
            mode: "enabled".into(),
            expected_version: 2,
        };
        assert!(matches!(
            set_finance_mode_tx(&conn, &enable, "2026-10-16T00:00:00Z"),
            Err(AppError::Conflict(_))
        ));
        fixture_revision(&conn, 1);
        fixture_topup(
            &conn,
            "credited-1",
            &format!("router-{}", "a".repeat(32)),
            "credited",
            1,
        );
        conn.execute(
            "UPDATE market_payment_channels
             SET webhook_verified_at = '2026-10-16T00:00:00Z', webhook_verified_revision = 1
             WHERE id = 'channel'",
            [],
        )
        .unwrap();
        assert!(finance_readiness_tx(&conn).unwrap().can_enable);
        assert!(set_finance_mode_tx(&conn, &enable, "2026-10-16T00:00:00Z").is_err());
        set_finance_mode_tx(
            &conn,
            &ModeRequest {
                mode: "shadow".into(),
                expected_version: 1,
            },
            "2026-10-16T00:00:00Z",
        )
        .unwrap();
        set_finance_mode_tx(&conn, &enable, "2026-10-16T00:00:00Z").unwrap();
        assert!(wallet_purchase_mode_tx(&conn).unwrap());

        assert!(matches!(
            set_finance_mode_tx(
                &conn,
                &ModeRequest {
                    mode: "disabled".into(),
                    expected_version: 3,
                },
                "2026-10-16T01:00:00Z",
            ),
            Err(AppError::Conflict(_))
        ));
        let approved: Option<String> = conn
            .query_row(
                "SELECT cutover_approved_at FROM market_finance_control WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(approved.as_deref(), Some("2026-10-16T00:00:00Z"));
        assert!(wallet_purchase_mode_tx(&conn).unwrap());
        conn.execute(
            "UPDATE market_finance_control SET mode = 'disabled' WHERE id = 1",
            [],
        )
        .unwrap();
        assert!(matches!(
            wallet_purchase_mode_tx(&conn),
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[test]
    fn owner_can_pause_new_wallet_obligations_without_changing_the_funding_rail() {
        let conn = memory();
        let at = "2026-10-16T00:00:00Z";
        conn.execute(
            "UPDATE market_finance_control
             SET mode = 'enabled', cutover_approved_at = ?1 WHERE id = 1",
            params![at],
        )
        .unwrap();
        let request = FinancePauseRequest {
            new_orders_paused: true,
            new_withdrawals_paused: true,
            expected_version: 1,
        };
        set_finance_pause_tx(&conn, &request, at).unwrap();
        assert!(wallet_purchase_mode_tx(&conn).unwrap());
        assert!(finance_readiness_tx(&conn).unwrap().new_orders_paused);
        assert!(finance_readiness_tx(&conn).unwrap().new_withdrawals_paused);
        assert!(matches!(
            require_new_wallet_orders_open_tx(&conn),
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(matches!(
            require_new_wallet_withdrawals_open_tx(&conn),
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(matches!(
            set_finance_pause_tx(&conn, &request, at),
            Err(AppError::Conflict(_))
        ));
        set_finance_pause_tx(
            &conn,
            &FinancePauseRequest {
                new_orders_paused: false,
                new_withdrawals_paused: false,
                expected_version: 2,
            },
            at,
        )
        .unwrap();
        require_new_wallet_orders_open_tx(&conn).unwrap();
        require_new_wallet_withdrawals_open_tx(&conn).unwrap();
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
        assert!(readiness.payment_channel_blockers.iter().any(|blocker| {
            blocker.code == "channel_no_current_live_topup" && blocker.count == 1
        }));
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
        assert!(validate_webhook_mode_change("signed", "poll_only", true).is_ok());
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
        };
        let value = serde_json::to_value(BuyerChannelView::from(channel)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "id": "channel", "displayName": "Gateway", "gatewayKind": "dujiaopay"
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
                .any(|blocker| blocker.code == "channel_no_current_live_topup")
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
    fn production_channel_reenable_requires_current_signed_live_evidence() {
        let conn = memory();
        fixture_revision(&conn, 1);
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
        set_channel_enabled_tx(&conn, "channel", false, 2, "2026-10-03").unwrap();
        conn.execute(
            "UPDATE market_finance_control SET mode = 'enabled' WHERE id = 1",
            [],
        )
        .unwrap();
        assert!(set_channel_enabled_tx(&conn, "channel", true, 3, "2026-10-04").is_err());
        conn.execute(
            "UPDATE market_payment_channels SET webhook_verified_at = '2026-10-04',
             webhook_verified_revision = 1 WHERE id = 'channel'",
            [],
        )
        .unwrap();
        assert!(set_channel_enabled_tx(&conn, "channel", true, 3, "2026-10-04").is_err());
        fixture_topup(
            &conn,
            "credited-1",
            &format!("router-{}", "a".repeat(32)),
            "credited",
            1,
        );
        set_channel_enabled_tx(&conn, "channel", true, 3, "2026-10-04").unwrap();
        set_channel_enabled_tx(&conn, "channel", false, 4, "2026-10-05").unwrap();

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
        fixture_topup(
            &conn,
            "credited-2",
            &format!("router-{}", "b".repeat(32)),
            "credited",
            2,
        );
        assert!(set_channel_enabled_tx(&conn, "channel", true, 5, "2026-10-06").is_err());
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
            confirmations: Some(1),
            paid_at: Some("2026-10-01T00:00:00+00:00".into()),
            checkout_url: None,
            expires_at: None,
            selection_deadline: None,
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
    fn stale_pending_order_can_close_only_after_terminal_gateway_check() {
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
            ("order-1".into(), 1)
        );
        assert!(bound_webhook_order_tx(&tx, "other", merchant_order_id).is_err());
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
    fn paid_legacy_supply_blocks_finance_cutover_before_a_contract_exists() {
        let conn = memory();
        conn.execute(
            "INSERT INTO router_ssh_hosts
             (id, ip, port, host_owner_email, status, created_at, updated_at,
              daily_rate_minor, currency)
             VALUES ('host-1', '192.0.2.10', 22, 'provider@example.com', 'idle',
                     '2026-10-01', '2026-10-01', 100, 'USD')",
            [],
        )
        .unwrap();
        let readiness = finance_readiness_tx(&conn).unwrap();
        assert!(!readiness.can_enable);
        assert!(
            readiness
                .legacy_blockers
                .iter()
                .any(|blocker| blocker.code == "legacy_client_paid_offers" && blocker.count == 1)
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
        assert_eq!(
            finance_readiness_tx(&conn)
                .unwrap()
                .legacy_blockers
                .iter()
                .filter(|blocker| blocker.code.ends_with("paid_offers"))
                .map(|blocker| blocker.count)
                .sum::<i64>(),
            2
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
        assert!(
            !finance_readiness_tx(&conn)
                .unwrap()
                .legacy_blockers
                .iter()
                .any(|blocker| blocker.code.ends_with("paid_offers"))
        );
    }

    #[test]
    fn paid_offer_cannot_cross_payment_rails_without_republication() {
        for product in ["Share", "Client Host"] {
            assert!(require_current_offer_rail("legacy_provider", false, product).is_ok());
            assert!(require_current_offer_rail("platform_wallet", true, product).is_ok());
            assert!(matches!(
                require_current_offer_rail("legacy_provider", true, product),
                Err(AppError::Gone(_))
            ));
            assert!(matches!(
                require_current_offer_rail("platform_wallet", false, product),
                Err(AppError::Gone(_))
            ));
            assert!(matches!(
                require_current_offer_rail("unknown", false, product),
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
}
