mod crypto;
mod dujiaopay;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, header};
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
const MAX_TOPUPS_PER_HOUR: i64 = 12;
const MAX_TOPUPS_PER_IP_HOUR: u32 = 60;
// Keep this false until Share and Client purchases, reconcile, renewal,
// failure release, refunds, and UI have all moved to the same wallet rail.
const MARKET_WALLET_CUTOVER_IMPLEMENTED: bool = false;

fn user_topup_allowed(mode: &str, is_shadow_owner: bool) -> bool {
    (mode == "enabled" && MARKET_WALLET_CUTOVER_IMPLEMENTED)
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

/// The Owner API is the only supported way to enter enabled mode; its
/// readiness gate remains closed until every purchase and control path is
/// integrated. Existing contracts must still dispatch by their own rail.
pub(crate) fn wallet_purchase_mode_tx(conn: &Connection) -> Result<bool, AppError> {
    let enabled = conn
        .query_row(
            "SELECT mode = 'enabled' FROM market_finance_control WHERE id = 1",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(db_error("read market finance purchase mode"))?;
    // The Owner endpoint enforces readiness, but an imported database or a
    // manual SQL update can still contain `enabled`. Never let that turn a
    // partially integrated wallet quote into a purchase on the legacy rail.
    if enabled && !MARKET_WALLET_CUTOVER_IMPLEMENTED {
        return Err(AppError::ServiceUnavailable(
            "platform wallet purchase integration is not complete".into(),
        ));
    }
    Ok(enabled)
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
            "/v1/admin/market-payment-channels",
            get(list_admin_channels).post(create_channel),
        )
        .route(
            "/v1/admin/market-payment-channels/:id",
            put(replace_channel),
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
        .route("/v1/admin/market-finance/mode", put(set_finance_mode))
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
    code_integrated: bool,
    legacy_blockers: Vec<ReadinessBlocker>,
    critical_cases: i64,
    verified_live_topups: i64,
    can_enable: bool,
}

fn finance_readiness_tx(conn: &Connection) -> Result<FinanceReadinessView, AppError> {
    let (mode, version): (String, i64) = conn
        .query_row(
            "SELECT mode, version FROM market_finance_control WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read market finance mode"))?;
    let checks: [(&'static str, &'static str); 15] = [
        (
            "legacy_share_paid_offers",
            "SELECT COUNT(*) FROM share_market_seats seat
             JOIN share_market_listings listing ON listing.id = seat.listing_id
             WHERE listing.status = 'active' AND listing.deleted_at IS NULL
               AND seat.status = 'available'
               AND (seat.daily_rate_minor IS NOT NULL OR seat.cycle_price_minor IS NOT NULL)",
        ),
        (
            "legacy_client_paid_offers",
            "SELECT COUNT(*) FROM router_ssh_hosts
             WHERE status = 'idle'
               AND (daily_rate_minor IS NOT NULL OR cycle_price_minor IS NOT NULL)",
        ),
        (
            "legacy_active_share_quotes",
            "SELECT COUNT(*) FROM share_market_rent_quotes
             WHERE status = 'active' AND pricing_model != 'free'",
        ),
        (
            "legacy_active_client_quotes",
            "SELECT COUNT(*) FROM client_market_allocation_quotes quote
             JOIN client_market_allocation_quote_items item ON item.quote_id = quote.id
             WHERE quote.status = 'active'
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
            "SELECT COUNT(*) FROM market_topup_orders WHERE status = 'credited'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error("count verified top-ups"))?;
    let critical_cases = crate::market_reconcile::critical_case_count_tx(conn)?;
    let can_enable = MARKET_WALLET_CUTOVER_IMPLEMENTED
        && legacy_blockers.is_empty()
        && critical_cases == 0
        && verified_live_topups > 0;
    Ok(FinanceReadinessView {
        mode,
        version,
        code_integrated: MARKET_WALLET_CUTOVER_IMPLEMENTED,
        legacy_blockers,
        critical_cases,
        verified_live_topups,
        can_enable,
    })
}

async fn get_finance_readiness(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(finance_readiness_tx(&conn)?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeRequest {
    mode: String,
    expected_version: i64,
}

async fn set_finance_mode(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<ModeRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    if !matches!(request.mode.as_str(), "disabled" | "shadow" | "enabled") {
        return Err(AppError::BadRequest("invalid market finance mode".into()));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin finance mode update"))?;
    let readiness = finance_readiness_tx(&tx)?;
    if request.mode == "enabled" && !readiness.can_enable {
        return Err(AppError::Conflict(
            "market wallet cutover gates have not passed".into(),
        ));
    }
    let changed = tx
        .execute(
            "UPDATE market_finance_control SET mode = ?2, version = version + 1, updated_at = ?3
         WHERE id = 1 AND version = ?1",
            params![request.expected_version, request.mode, now()],
        )
        .map_err(db_error("update finance mode"))?;
    if changed != 1 {
        return Err(AppError::Conflict("finance mode version changed".into()));
    }
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

async fn get_wallet(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let (snapshot, mode, has_wallet_activity) = {
        let conn = state.store.conn.lock().await;
        let snapshot = wallet_snapshot_tx(&conn, &session.user_id)?;
        let mode: String = conn
            .query_row(
                "SELECT mode FROM market_finance_control WHERE id = 1",
                [],
                |row| row.get(0),
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
        (snapshot, mode, has_wallet_activity)
    };
    let can_topup = user_topup_allowed(&mode, state.dynamic.read().await.is_admin(&session.email));
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
    verified: bool,
    gateway_kind: &'static str,
}

fn channel_views(conn: &Connection, only_enabled: bool) -> Result<Vec<ChannelView>, AppError> {
    let sql = if only_enabled {
        "SELECT c.id, c.display_name, c.enabled, c.sort_order, c.current_revision,
                r.verified_at IS NOT NULL
         FROM market_payment_channels c
         JOIN market_payment_channel_revisions r
           ON r.channel_id = c.id AND r.revision = c.current_revision
         WHERE c.enabled = 1 AND r.verified_at IS NOT NULL
         ORDER BY c.sort_order, c.id"
    } else {
        "SELECT c.id, c.display_name, c.enabled, c.sort_order, c.current_revision,
                r.verified_at IS NOT NULL
         FROM market_payment_channels c
         LEFT JOIN market_payment_channel_revisions r
           ON r.channel_id = c.id AND r.revision = c.current_revision
         ORDER BY c.sort_order, c.id"
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
                verified: row.get::<_, i64>(5)? == 1,
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
    let mode: String = {
        let conn = state.store.conn.lock().await;
        conn.query_row(
            "SELECT mode FROM market_finance_control WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(db_error("read market finance mode"))?
    };
    let shadow_admin = mode == "shadow" && state.dynamic.read().await.is_admin(&session.email);
    let conn = state.store.conn.lock().await;
    let channels = if user_topup_allowed(&mode, shadow_admin) {
        channel_views(&conn, true)?
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
    webhook_secret: String,
    #[serde(default)]
    allowed_methods: Vec<String>,
}

fn validate_channel_input(input: &UpsertChannelRequest) -> Result<(), AppError> {
    if input.display_name.trim().is_empty()
        || input.display_name.len() > 80
        || input.api_key_id.trim().is_empty()
        || input.api_key_id.len() > 200
        || input.api_secret.len() < 16
        || input.webhook_secret.len() < 16
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

async fn save_channel(
    state: &ServerState,
    session: &AuthSession,
    channel_id: Option<String>,
    input: UpsertChannelRequest,
) -> Result<ChannelView, AppError> {
    validate_channel_input(&input)?;
    let credentials = Credentials {
        key_id: input.api_key_id.trim().into(),
        api_secret: Zeroizing::new(input.api_secret.clone()),
        webhook_secret: Zeroizing::new(input.webhook_secret.clone()),
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
    let current_revision = tx
        .query_row(
            "SELECT current_revision FROM market_payment_channels WHERE id = ?1",
            params![id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(db_error("read payment channel revision"))?;
    if replacing && current_revision.is_none() {
        return Err(AppError::NotFound("payment channel not found".into()));
    }
    let revision = current_revision.unwrap_or(0) + 1;
    let (api_ciphertext, api_nonce) = state.market_payments.cipher.seal(
        &input.api_secret,
        &aad(&id, revision, &merchant_fingerprint, "api"),
    )?;
    let (webhook_ciphertext, webhook_nonce) = state.market_payments.cipher.seal(
        &input.webhook_secret,
        &aad(&id, revision, &merchant_fingerprint, "webhook"),
    )?;
    let time = now();
    if current_revision.is_none() {
        tx.execute(
            "INSERT INTO market_payment_channels (id, gateway_kind, display_name, enabled, sort_order, current_revision, created_at, updated_at)
             VALUES (?1, 'dujiaopay', ?2, 0, ?3, ?4, ?5, ?5)",
            params![id, input.display_name.trim(), input.sort_order, revision, time],
        ).map_err(db_error("insert payment channel"))?;
    } else {
        tx.execute(
            "UPDATE market_payment_channels SET display_name = ?2, enabled = 0, sort_order = ?3,
                    current_revision = ?4, updated_at = ?5 WHERE id = ?1",
            params![
                id,
                input.display_name.trim(),
                input.sort_order,
                revision,
                time
            ],
        )
        .map_err(db_error("update payment channel"))?;
    }
    let allowed_methods_json = serde_json::to_string(&input.allowed_methods)
        .map_err(|_| AppError::Internal("encode allowed methods failed".into()))?;
    tx.execute(
        "INSERT INTO market_payment_channel_revisions (
            channel_id, revision, merchant_id, merchant_fingerprint, api_key_id,
            api_secret_ciphertext, api_secret_nonce, webhook_secret_ciphertext,
            webhook_secret_nonce, key_version, allowed_methods_json, verified_at, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
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
            time
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
    Ok(ChannelView {
        id,
        display_name: input.display_name.trim().into(),
        enabled: false,
        sort_order: input.sort_order,
        current_revision: revision,
        verified: true,
        gateway_kind: "dujiaopay",
    })
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
        let exists = conn
            .query_row(
                "SELECT 1 FROM market_payment_channels WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(db_error("check payment channel"))?
            .is_some();
        if !exists {
            return Err(AppError::NotFound("payment channel not found".into()));
        }
    }
    let view = save_channel(&state, &session, Some(id), input).await?;
    Ok((no_store(), Json(view)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetEnabledRequest {
    enabled: bool,
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
    let changed = tx
        .execute(
            "UPDATE market_payment_channels SET enabled = ?2, updated_at = ?3
         WHERE id = ?1 AND (SELECT verified_at FROM market_payment_channel_revisions
                            WHERE channel_id = ?1 AND revision = current_revision) IS NOT NULL",
            params![id, i64::from(input.enabled), now()],
        )
        .map_err(db_error("toggle payment channel"))?;
    if changed != 1 {
        return Err(AppError::NotFound(
            "verified payment channel not found".into(),
        ));
    }
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
    conn.execute(
        "INSERT OR IGNORE INTO market_finance_reconciliation_cases
         (id, code, object_kind, object_id, status, created_at)
         VALUES (?1, ?2, ?3, ?4, 'open', ?5)",
        params![
            Uuid::new_v4().to_string(),
            code,
            object_kind,
            object_id,
            time
        ],
    )
    .map_err(db_error("open market finance reconciliation case"))?;
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
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut statement = conn
        .prepare(
            "SELECT id, code, object_kind, object_id, status, detail_code, created_at, resolved_at
             FROM market_finance_reconciliation_cases
             WHERE status = 'open' ORDER BY created_at, id LIMIT 200",
        )
        .map_err(db_error("prepare market finance cases"))?;
    let cases = statement
        .query_map([], |row| {
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
    Ok((no_store(), Json(cases)))
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
}

async fn list_admin_topups(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut statement = conn
        .prepare(
            "SELECT id, user_id, channel_id, merchant_order_id, gateway_order_id,
                    amount_minor, status, created_at, updated_at
             FROM market_topup_orders
             WHERE status IN ('review_required', 'create_unknown')
                OR (status IN ('creating', 'pending', 'verifying') AND updated_at < ?1)
             ORDER BY updated_at, id LIMIT 200",
        )
        .map_err(db_error("prepare unresolved top-ups"))?;
    let cutoff = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let items = statement
        .query_map(params![cutoff], |row| {
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
            })
        })
        .map_err(db_error("query unresolved top-ups"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read unresolved top-ups"))?;
    Ok((no_store(), Json(items)))
}

struct LoadedRevision {
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
    ) = conn
        .query_row(
            "SELECT merchant_id, merchant_fingerprint, api_key_id, api_secret_ciphertext,
                api_secret_nonce, webhook_secret_ciphertext, webhook_secret_nonce,
                key_version, allowed_methods_json
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
    let webhook_secret = runtime.cipher.open(
        &webhook_ciphertext,
        &webhook_nonce,
        &aad(channel_id, revision, &merchant_fingerprint, "webhook"),
        key_version,
    )?;
    let allowed_methods = serde_json::from_str(&methods_json)
        .map_err(|_| AppError::Internal("stored payment methods invalid".into()))?;
    if hex::encode(Sha256::digest(merchant_id.as_bytes())) != merchant_fingerprint {
        return Err(AppError::Internal(
            "payment merchant identity changed unexpectedly".into(),
        ));
    }
    Ok(LoadedRevision {
        merchant_fingerprint,
        allowed_methods,
        credentials: Credentials {
            key_id,
            api_secret,
            webhook_secret,
        },
    })
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
}

fn read_topup(conn: &Connection, order_id: &str, user_id: &str) -> Result<TopupView, AppError> {
    conn.query_row(
        "SELECT id, channel_id, amount_minor, status, checkout_url, created_at
         FROM market_topup_orders WHERE id = ?1 AND user_id = ?2",
        params![order_id, user_id],
        |row| {
            Ok(TopupView {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                amount_minor: row.get(2)?,
                status: row.get(3)?,
                checkout_url: row.get(4)?,
                created_at: row.get(5)?,
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
             SET gateway_order_id = COALESCE(gateway_order_id, ?2), checkout_url = ?3,
                 expires_at = ?4, status = CASE
                     WHEN status IN ('credited', 'expired', 'cancelled', 'review_required')
                     THEN status ELSE 'pending' END,
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
            "SELECT id, channel_id, amount_minor, status, checkout_url, created_at
             FROM market_topup_orders WHERE user_id = ?1
               AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id < ?3))
             ORDER BY created_at DESC, id DESC LIMIT 51",
        )
        .map_err(db_error("prepare top-up history"))?;
    let mut items = statement
        .query_map(params![user_id, cursor_at, cursor], |row| {
            Ok(TopupView {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                amount_minor: row.get(2)?,
                status: row.get(3)?,
                checkout_url: row.get(4)?,
                created_at: row.get(5)?,
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
        let mode: String = tx
            .query_row(
                "SELECT mode FROM market_finance_control WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(db_error("read market finance mode"))?;
        if mode == "enabled" && !MARKET_WALLET_CUTOVER_IMPLEMENTED {
            return Err(AppError::ServiceUnavailable(
                "platform wallet purchase integration is not complete".into(),
            ));
        }
        if !user_topup_allowed(&mode, shadow_admin) {
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
                "SELECT current_revision FROM market_payment_channels WHERE id = ?1 AND enabled = 1",
                params![input.channel_id], |row| row.get::<_, i64>(0),
            ).optional().map_err(db_error("read enabled payment channel"))?
                .ok_or_else(|| AppError::BadRequest("payment channel unavailable".into()))?;
            let pending: i64 = tx.query_row(
                "SELECT COUNT(*) FROM market_topup_orders WHERE user_id = ?1
                 AND status IN ('created', 'creating', 'create_unknown', 'pending', 'verifying', 'review_required')",
                params![session.user_id], |row| row.get(0),
            ).map_err(db_error("count pending top-ups"))?;
            if pending >= MAX_PENDING_ORDERS {
                return Err(AppError::TooManyRequests("too many pending top-ups".into()));
            }
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
            let time = now();
            tx.execute(
                "INSERT INTO market_topup_orders (
                    id, user_id, channel_id, channel_revision, merchant_fingerprint,
                    merchant_order_id, idempotency_key, request_fingerprint, amount_minor,
                    status, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'creating', ?10, ?10)",
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
                    time
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
    let success_url = format!("{base}/account/billing?topup={order_id}");
    let cancel_url = format!("{base}/account/billing?topup={order_id}&cancelled=1");
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
            save_created_order_tx(&tx, &order_id, revision, &created, &now())?;
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

fn validate_paid_order_tx(
    conn: &Connection,
    order_id: &str,
    channel_id: &str,
    revision: i64,
    loaded: &LoadedRevision,
    event: &WebhookEvent,
    gateway: &GatewayOrder,
) -> Result<Option<ValidatedTopup>, AppError> {
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

async fn dujiaopay_webhook(
    State(state): State<ServerState>,
    Path(channel_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, AppError> {
    if channel_id.len() > 80
        || !channel_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(AppError::BadRequest("invalid payment channel".into()));
    }
    // The untrusted body is used only to choose the already-bound order key.
    // No field is acted on until the HMAC over the original bytes succeeds.
    // This also avoids silently dropping late payments when a merchant has
    // accumulated more than an arbitrary number of key revisions.
    let merchant_order_hint = dujiaopay::merchant_order_hint(&body)?;
    let (revision, loaded, event, order_id) = {
        let conn = state.store.conn.lock().await;
        let bound = bound_webhook_order_tx(&conn, &channel_id, &merchant_order_hint)?;
        let loaded = load_revision(&conn, &state.market_payments, &channel_id, bound.1)?;
        let event = dujiaopay::verify_webhook(
            &loaded.credentials,
            &headers,
            &body,
            Utc::now().timestamp(),
        )?;
        if event.merchant_order_id != merchant_order_hint {
            return Err(AppError::Unauthorized(
                "DujiaoPay webhook not recognized".into(),
            ));
        }
        (bound.1, loaded, event, bound.0)
    };
    {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin webhook event"))?;
        tx.execute(
            "INSERT OR IGNORE INTO market_topup_events (
                id, order_id, merchant_fingerprint, gateway_event_id, event_type,
                raw_body_sha256, status, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
            params![
                Uuid::new_v4().to_string(),
                order_id,
                loaded.merchant_fingerprint,
                event.event_id,
                event.event_type,
                event.raw_sha256,
                now()
            ],
        )
        .map_err(db_error("persist DujiaoPay webhook"))?;
        tx.commit().map_err(db_error("commit DujiaoPay webhook"))?;
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
    let gateway = state
        .market_payments
        .client
        .get_order(&loaded.credentials, &event.order_id)
        .await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin verified top-up"))?;
    if gateway.status != "paid" {
        let (merchant_order_id, stored_gateway_id, amount_minor, merchant_fingerprint): (
            String,
            Option<String>,
            i64,
            String,
        ) = tx
            .query_row(
                "SELECT merchant_order_id, gateway_order_id, amount_minor, merchant_fingerprint
             FROM market_topup_orders WHERE id = ?1 AND channel_id = ?2 AND channel_revision = ?3",
                params![order_id, channel_id, revision],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(db_error("read non-paid DujiaoPay order"))?;
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
        let status = match gateway.status.as_str() {
            "expired" => "expired",
            "canceled" | "cancelled" => "cancelled",
            _ => "pending",
        };
        tx.execute(
            "UPDATE market_topup_orders SET status = ?2, updated_at = ?3
             WHERE id = ?1 AND status NOT IN ('credited', 'review_required')",
            params![order_id, status, now()],
        )
        .map_err(db_error("record DujiaoPay order status"))?;
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
            open_reconciliation_case_tx(
                &tx,
                "topup_verification_failed",
                "topup",
                &order_id,
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
    channel_id: String,
    revision: i64,
    merchant_order_id: String,
    gateway_order_id: Option<String>,
    amount_minor: i64,
}

fn pending_topups_tx(conn: &Connection) -> Result<Vec<PendingTopup>, AppError> {
    let mut statement = conn.prepare(
        "SELECT id, channel_id, channel_revision, merchant_order_id, gateway_order_id, amount_minor
         FROM market_topup_orders
         WHERE updated_at <= ?2 AND (
             status IN ('created', 'creating', 'create_unknown', 'pending', 'verifying')
             OR (status IN ('expired', 'cancelled') AND created_at >= ?1)
         )
         ORDER BY updated_at, id LIMIT 24",
    ).map_err(db_error("prepare pending top-ups"))?;
    statement
        .query_map(
            params![
                (Utc::now() - chrono::Duration::days(7)).to_rfc3339(),
                (Utc::now() - chrono::Duration::seconds(30)).to_rfc3339()
            ],
            |row| {
                Ok(PendingTopup {
                    id: row.get(0)?,
                    channel_id: row.get(1)?,
                    revision: row.get(2)?,
                    merchant_order_id: row.get(3)?,
                    gateway_order_id: row.get(4)?,
                    amount_minor: row.get(5)?,
                })
            },
        )
        .map_err(db_error("query pending top-ups"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read pending top-ups"))
}

async fn reconcile_one_topup(state: &ServerState, pending: PendingTopup) -> Result<(), AppError> {
    let loaded = {
        let conn = state.store.conn.lock().await;
        load_revision(
            &conn,
            &state.market_payments,
            &pending.channel_id,
            pending.revision,
        )?
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
        let created = state
            .market_payments
            .client
            .create_order(
                &loaded.credentials,
                &pending.merchant_order_id,
                pending.amount_minor,
                &loaded.allowed_methods,
                &format!("{base}/account/billing?topup={}", pending.id),
                &format!("{base}/account/billing?topup={}&cancelled=1", pending.id),
            )
            .await?;
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin top-up recovery"))?;
        save_created_order_tx(&tx, &pending.id, pending.revision, &created, &now())?;
        tx.commit().map_err(db_error("commit top-up recovery"))?;
        created.order_id
    };
    let gateway = state
        .market_payments
        .client
        .get_order(&loaded.credentials, &gateway_id)
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
    if gateway.status == "paid" {
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
                let time = now();
                open_reconciliation_case_tx(
                    &tx,
                    "topup_verification_failed",
                    "topup",
                    &pending.id,
                    &time,
                )?;
                tx.execute(
                    "UPDATE market_topup_orders SET status = 'review_required', updated_at = ?2
                     WHERE id = ?1 AND status != 'credited'",
                    params![pending.id, time],
                )
                .map_err(db_error("hold reconciled top-up for review"))?;
            }
            Err(error) => return Err(error),
        }
    } else if matches!(
        gateway.status.as_str(),
        "expired" | "canceled" | "cancelled"
    ) {
        let status = if gateway.status == "expired" {
            "expired"
        } else {
            "cancelled"
        };
        tx.execute(
            "UPDATE market_topup_orders SET status = ?2, updated_at = ?3
             WHERE id = ?1 AND status NOT IN ('credited', 'review_required')",
            params![pending.id, status, now()],
        )
        .map_err(db_error("reconcile unpaid top-up"))?;
    } else {
        tx.execute(
            "UPDATE market_topup_orders SET updated_at = ?2
             WHERE id = ?1 AND status IN ('created', 'creating', 'create_unknown', 'pending', 'verifying')",
            params![pending.id, now()],
        ).map_err(db_error("touch pending top-up"))?;
    }
    tx.commit()
        .map_err(db_error("commit top-up reconciliation"))?;
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
                if let Err(_) = result { tracing::warn!("market earning maturity deferred"); }
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
    fn manually_enabled_mode_cannot_bypass_unfinished_purchase_cutover() {
        let conn = memory();
        assert!(!wallet_purchase_mode_tx(&conn).unwrap());
        assert!(!user_topup_allowed("disabled", true));
        assert!(user_topup_allowed("shadow", true));
        assert!(!user_topup_allowed("shadow", false));
        assert!(!user_topup_allowed("enabled", false));
        assert!(!user_topup_allowed("enabled", true));
        conn.execute(
            "UPDATE market_finance_control SET mode = 'enabled' WHERE id = 1",
            [],
        )
        .unwrap();
        assert!(matches!(
            wallet_purchase_mode_tx(&conn),
            Err(AppError::ServiceUnavailable(_))
        ));
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
    }

    fn loaded() -> LoadedRevision {
        LoadedRevision {
            merchant_fingerprint: "merchant-fingerprint".into(),
            allowed_methods: Vec::new(),
            credentials: Credentials {
                key_id: "key".into(),
                api_secret: Zeroizing::new("api-secret".into()),
                webhook_secret: Zeroizing::new("webhook-secret".into()),
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
        }
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
            checkout_url: "https://www.dujiaopay.com/c/one".into(),
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
}
