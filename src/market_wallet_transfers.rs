//! Same-Router, non-cash wallet transfers. Quotes freeze exact ledger units;
//! email challenges authorize that quote, not a mutable recipient or amount.
use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use chrono::{Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use uuid::Uuid;

use crate::market_wallet_policy::db_error;
use crate::{
    ServerState,
    db::{Connection, OptionalExtension, TransactionBehavior, params},
    error::AppError,
    market_wallet::{
        Posting, now, post_journal_tx, serialize_units, units_from_minor, wallet_snapshot_tx,
    },
};

fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
}
fn problem(code: &'static str, message: &str) -> AppError {
    AppError::Coded {
        status: StatusCode::CONFLICT,
        code,
        message: message.into(),
        details: serde_json::json!({}),
    }
}
async fn session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::models::AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-wallet/transfer-capacity", get(capacity))
        .route("/v1/market-wallet/transfer-quotes", post(create_quote))
        .route(
            "/v1/market-wallet/transfer-quotes/:id/code",
            post(send_code),
        )
        .route("/v1/market-wallet/transfers", get(list).post(confirm))
        .route("/v1/market-wallet/transfers/:id", get(detail))
}

fn require_user_tx(conn: &Connection, user: &str) -> Result<String, AppError> {
    conn.query_row("SELECT email_normalized FROM users WHERE id = ?1 AND status = 'active' AND email_verified_at IS NOT NULL",
        params![user], |row| row.get(0)).optional().map_err(db_error("read verified transfer user"))?
        .ok_or_else(|| problem("WALLET_TRANSFER_USER_UNVERIFIED", "both users must sign in with a verified email before transferring"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Capacity {
    #[serde(serialize_with = "serialize_units")]
    transferable_units: i64,
    #[serde(serialize_with = "serialize_units")]
    committed_units: i64,
    #[serde(serialize_with = "serialize_units")]
    daily_remaining_units: i64,
    enabled: bool,
    verified: bool,
}

fn capacity_tx(conn: &Connection, user: &str, time: &str) -> Result<Capacity, AppError> {
    let policy = crate::market_wallet_policy::policy_tx(conn)?;
    let available = wallet_snapshot_tx(conn, user)?.available_units;
    let committed =
        crate::market_platform_contracts::prior_due_funding_units_tx(conn, user, None, time)?;
    let day = format!("{}T00:00:00+00:00", &time[..10]);
    let sent: i64 = conn.query_row("SELECT COALESCE(SUM(amount_units), 0) FROM market_wallet_transfers WHERE sender_user_id = ?1 AND created_at >= ?2",
        params![user, day], |row| row.get(0)).map_err(db_error("sum daily wallet transfers"))?;
    let remaining = units_from_minor(policy.transfer_daily_max_minor)?
        .saturating_sub(sent)
        .max(0);
    let mode: bool = conn.query_row("SELECT mode = 'enabled' AND cutover_approved_at IS NOT NULL FROM market_finance_control WHERE id = 1", [], |row| row.get(0))
        .map_err(db_error("read transfer mode"))?;
    Ok(Capacity {
        transferable_units: available
            .saturating_sub(committed)
            .max(0)
            .min(units_from_minor(policy.transfer_max_minor)?)
            .min(remaining),
        committed_units: committed,
        daily_remaining_units: remaining,
        enabled: mode && policy.transfers_enabled && !policy.transfers_paused,
        verified: require_user_tx(conn, user).is_ok(),
    })
}

fn require_transfer_tx(
    conn: &Connection,
    sender: &str,
    recipient: &str,
    amount: i64,
    time: &str,
) -> Result<(), AppError> {
    if sender == recipient || amount <= 0 {
        return Err(AppError::BadRequest("invalid wallet transfer".into()));
    }
    require_user_tx(conn, sender)?;
    require_user_tx(conn, recipient)?;
    crate::market_reconcile::ensure_finance_scopes_safe_tx(
        conn,
        &[
            crate::market_reconcile::FinanceScope::User(sender),
            crate::market_reconcile::FinanceScope::User(recipient),
        ],
    )?;
    let free = capacity_tx(conn, sender, time)?;
    if !free.enabled {
        return Err(problem(
            "WALLET_TRANSFER_PAUSED",
            "wallet transfers are not available",
        ));
    }
    if amount > free.transferable_units {
        return Err(problem(
            "WALLET_TRANSFER_INSUFFICIENT",
            "transfer exceeds available balance or daily limit; review the amount",
        ));
    }
    Ok(())
}

async fn capacity(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut value = capacity_tx(&conn, &user.user_id, &now())?;
    value.enabled &= state
        .config
        .resend_api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty())
        && crate::notifications::notification_sender(&state.config).is_some();
    Ok((no_store(), Json(value)))
}

/// Prune only expired, unused authorization material in bounded batches. The
/// immutable financial record, its quote and consumed challenge stay forever.
pub(crate) fn prune_unused_tx(
    conn: &Connection,
    time: chrono::DateTime<Utc>,
) -> Result<(), AppError> {
    let old = (time - Duration::days(30)).to_rfc3339();
    conn.execute(
        "DELETE FROM market_wallet_transfer_attempts WHERE id IN
        (SELECT id FROM market_wallet_transfer_attempts WHERE created_at < ?1 LIMIT 2000)",
        params![(time - Duration::days(1)).to_rfc3339()],
    )
    .map_err(db_error("prune transfer rate limits"))?;
    conn.execute("DELETE FROM market_wallet_transfer_challenges WHERE id IN
        (SELECT c.id FROM market_wallet_transfer_challenges c WHERE c.expires_at < ?1
         AND NOT EXISTS (SELECT 1 FROM market_wallet_transfers t WHERE t.challenge_id = c.id) LIMIT 2000)",
        params![old]).map_err(db_error("prune unused transfer codes"))?;
    conn.execute("DELETE FROM market_wallet_transfer_quotes WHERE id IN
        (SELECT q.id FROM market_wallet_transfer_quotes q WHERE q.expires_at < ?1
         AND NOT EXISTS (SELECT 1 FROM market_wallet_transfer_challenges c WHERE c.quote_id = q.id)
         AND NOT EXISTS (SELECT 1 FROM market_wallet_transfers t WHERE t.quote_id = q.id) LIMIT 2000)",
        params![old]).map_err(db_error("prune unused transfer quotes"))?;
    conn.execute(
        "DELETE FROM market_topup_quotes WHERE id IN
        (SELECT q.id FROM market_topup_quotes q WHERE q.expires_at < ?1
         AND NOT EXISTS (SELECT 1 FROM market_topup_orders t WHERE t.quote_id = q.id) LIMIT 2000)",
        params![old],
    )
    .map_err(db_error("prune unused top-up quotes"))?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuoteRequest {
    recipient_email: String,
    amount_minor: Option<i64>,
    #[serde(default)]
    all: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Quote {
    id: String,
    #[serde(skip)]
    sender: String,
    #[serde(skip)]
    recipient: String,
    sender_email: String,
    recipient_email: String,
    #[serde(serialize_with = "serialize_units")]
    amount_units: i64,
    expires_at: String,
}
fn read_quote_tx(conn: &Connection, sender: &str, id: &str) -> Result<Quote, AppError> {
    conn.query_row("SELECT id, sender_user_id, recipient_user_id, sender_email, recipient_email, amount_units, expires_at FROM market_wallet_transfer_quotes WHERE id = ?1 AND sender_user_id = ?2",
        params![id, sender], |row| Ok(Quote { id: row.get(0)?, sender: row.get(1)?, recipient: row.get(2)?, sender_email: row.get(3)?, recipient_email: row.get(4)?, amount_units: row.get(5)?, expires_at: row.get(6)? }))
        .optional().map_err(db_error("read wallet transfer quote"))?.ok_or_else(|| AppError::NotFound("wallet transfer quote not found".into()))
}

async fn create_quote(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<QuoteRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let email = input.recipient_email.trim().to_ascii_lowercase();
    if email.len() > 254 || !email.contains('@') {
        return Err(AppError::BadRequest(
            "valid recipient email required".into(),
        ));
    }
    if input.all == input.amount_minor.is_some() {
        return Err(AppError::BadRequest(
            "choose an amount or transfer all".into(),
        ));
    }
    let conn = state.store.conn.lock().await;
    // Persist failed lookups too; otherwise an authenticated user could
    // enumerate registered emails without ever creating a successful quote.
    let ip = crate::client_meta::extract_client_metadata(&headers, peer)
        .ip
        .unwrap_or_else(|| peer.ip().to_string());
    let cutoff = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let attempts: i64 = conn.query_row("SELECT COUNT(*) FROM market_wallet_transfer_attempts WHERE (sender_user_id = ?1 OR created_ip = ?2) AND created_at >= ?3",
        params![user.user_id, ip, cutoff], |row| row.get(0)).map_err(db_error("limit recipient lookups"))?;
    if attempts >= 30 {
        return Err(AppError::TooManyRequests(
            "transfer quote limit reached; try later".into(),
        ));
    }
    conn.execute("INSERT INTO market_wallet_transfer_attempts (id, sender_user_id, created_ip, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![Uuid::new_v4().to_string(), user.user_id, ip, now()]).map_err(db_error("record recipient lookup"))?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin transfer quote"))?;
    let at = Utc::now();
    let time = at.to_rfc3339();
    let recent: i64 = tx.query_row("SELECT COUNT(*) FROM market_wallet_transfer_quotes WHERE sender_user_id = ?1 AND created_at >= ?2",
        params![user.user_id, (at - Duration::hours(1)).to_rfc3339()], |row| row.get(0)).map_err(db_error("count transfer quotes"))?;
    if recent >= 30 {
        return Err(AppError::TooManyRequests(
            "transfer quote limit reached; try later".into(),
        ));
    }
    let sender_email = require_user_tx(&tx, &user.user_id)?;
    let recipient: String = tx.query_row("SELECT id FROM users WHERE email_normalized = ?1 AND status = 'active' AND email_verified_at IS NOT NULL",
        params![email], |row| row.get(0)).optional().map_err(db_error("find wallet recipient"))?
        .ok_or_else(|| problem("WALLET_TRANSFER_RECIPIENT_UNAVAILABLE", "recipient must first sign in to this Router with this email"))?;
    let amount = match input.amount_minor {
        Some(value) => units_from_minor(value)?,
        None => capacity_tx(&tx, &user.user_id, &time)?.transferable_units,
    };
    require_transfer_tx(&tx, &user.user_id, &recipient, amount, &time)?;
    let quote = Quote {
        id: Uuid::new_v4().to_string(),
        sender: user.user_id,
        recipient,
        sender_email,
        recipient_email: email,
        amount_units: amount,
        expires_at: (at + Duration::minutes(15)).to_rfc3339(),
    };
    tx.execute("INSERT INTO market_wallet_transfer_quotes (id, sender_user_id, recipient_user_id, sender_email, recipient_email, amount_units, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![quote.id, quote.sender, quote.recipient, quote.sender_email, quote.recipient_email, quote.amount_units, quote.expires_at, time]).map_err(db_error("save transfer quote"))?;
    tx.commit().map_err(db_error("commit transfer quote"))?;
    Ok((no_store(), Json(quote)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeView {
    id: String,
    expires_at: String,
    resend_after_seconds: i64,
}

async fn send_code(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let api_key = state
        .config
        .resend_api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AppError::ServiceUnavailable("verification email is not configured".into())
        })?;
    let from = crate::notifications::notification_sender(&state.config).ok_or_else(|| {
        AppError::ServiceUnavailable("verification email is not configured".into())
    })?;
    let code = zeroize::Zeroizing::new(format!(
        "{:06}",
        rand::rngs::OsRng.gen_range(0..1_000_000_u32)
    ));
    let challenge_id = Uuid::new_v4().to_string();
    let (version, mac) = state
        .market_payments
        .transfer_mac(&challenge_id, &id, &code)?;
    let ip = crate::client_meta::extract_client_metadata(&headers, peer)
        .ip
        .unwrap_or_else(|| peer.ip().to_string());
    let (quote, expires) = {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin transfer email"))?;
        let quote = read_quote_tx(&tx, &user.user_id, &id)?;
        let completed: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_wallet_transfers WHERE quote_id = ?1)",
                params![id],
                |row| row.get(0),
            )
            .map_err(db_error("check completed transfer"))?;
        if completed {
            return Err(AppError::Conflict(
                "wallet transfer already completed".into(),
            ));
        }
        let at = Utc::now();
        let time = at.to_rfc3339();
        if quote.expires_at <= time {
            return Err(problem(
                "WALLET_TRANSFER_QUOTE_EXPIRED",
                "transfer quote expired; review the transfer again",
            ));
        }
        require_transfer_tx(
            &tx,
            &quote.sender,
            &quote.recipient,
            quote.amount_units,
            &time,
        )?;
        let too_soon: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM market_wallet_transfer_challenges c JOIN market_wallet_transfer_quotes q ON q.id = c.quote_id WHERE q.sender_user_id = ?1 AND c.created_at >= ?2)",
            params![user.user_id, (at - Duration::seconds(60)).to_rfc3339()], |row| row.get(0)).map_err(db_error("check transfer email interval"))?;
        let recent: i64 = tx.query_row("SELECT COUNT(*) FROM market_wallet_transfer_challenges c JOIN market_wallet_transfer_quotes q ON q.id = c.quote_id WHERE (q.sender_user_id = ?1 OR c.created_ip = ?2) AND c.created_at >= ?3",
            params![user.user_id, ip, (at - Duration::hours(1)).to_rfc3339()], |row| row.get(0)).map_err(db_error("check transfer email limit"))?;
        if too_soon || recent >= 10 {
            return Err(AppError::TooManyRequests(
                "wait before requesting another transfer code".into(),
            ));
        }
        // A resend invalidates older codes; the confirmed amount remains frozen.
        tx.execute("UPDATE market_wallet_transfer_challenges SET consumed_at = ?2 WHERE quote_id = ?1 AND consumed_at IS NULL", params![id, time]).map_err(db_error("retire transfer codes"))?;
        let expires = (at + Duration::minutes(10))
            .to_rfc3339()
            .min(quote.expires_at.clone());
        tx.execute("INSERT INTO market_wallet_transfer_challenges (id, quote_id, code_mac, key_version, expires_at, created_ip, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![challenge_id, id, mac, version, expires, ip, time]).map_err(db_error("save transfer email challenge"))?;
        tx.commit()
            .map_err(db_error("commit transfer email challenge"))?;
        (quote, expires)
    };
    let display = exact_usd(quote.amount_units);
    let text = format!(
        "Your wallet transfer code is {}.\nTransfer {display} to {} on this Router.\nThis code expires in 10 minutes. Transfers are final and not cash withdrawals. Never share this code. If you did not request this, do not approve it.",
        &*code, quote.recipient_email
    );
    let escaped = quote
        .recipient_email
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    let html = crate::store::render_email_layout(
        "Confirm wallet transfer",
        "Review the recipient and amount before confirming.",
        &format!(
            "<p>Transfer <strong>{display}</strong> to <strong>{escaped}</strong> on this Router.</p><p style=\"font-size:30px;font-weight:700;letter-spacing:6px\">{}</p><p>Expires in 10 minutes. Transfers are final, not cash withdrawals. Never share this code. If you did not request this transfer, do not approve it.</p>",
            &*code
        ),
    );
    crate::notifications::send_resend_frozen_email(
        &state.proxy_http,
        api_key,
        crate::notifications::FrozenEmailEnvelope {
            from: &from,
            recipient: &quote.sender_email,
            subject: "Confirm your TokenSwitch wallet transfer",
            html: &html,
            text: &text,
            reply_to: state.config.resend_reply_to.as_deref(),
            idempotency_key: &format!("wallet-transfer-code:{challenge_id}"),
        },
    )
    .await
    .map_err(|_| {
        AppError::ServiceUnavailable(
            "verification email could not be delivered; no funds moved".into(),
        )
    })?;
    let conn = state.store.conn.lock().await;
    conn.execute("UPDATE market_wallet_transfer_challenges SET delivered_at = ?2 WHERE id = ?1 AND consumed_at IS NULL",
        params![challenge_id, now()]).map_err(db_error("record transfer email delivery"))?;
    Ok((
        no_store(),
        Json(ChallengeView {
            id: challenge_id,
            expires_at: expires,
            resend_after_seconds: 60,
        }),
    ))
}

/// Display USD with an exact ledger-unit suffix for fractional cents.
fn exact_usd(units: i64) -> String {
    let whole = units / 8_640_000;
    let rest = units % 8_640_000;
    if rest % 86_400 == 0 {
        format!("US${whole}.{:02}", rest / 86_400)
    } else {
        format!(
            "US${:.8} (exactly {units} ledger units)",
            units as f64 / 8_640_000.0
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfirmRequest {
    quote_id: String,
    challenge_id: String,
    code: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Transfer {
    id: String,
    sender_email: String,
    recipient_email: String,
    #[serde(serialize_with = "serialize_units")]
    amount_units: i64,
    journal_id: String,
    created_at: String,
}
const TRANSFER_SELECT: &str = "SELECT id, sender_email, recipient_email, amount_units, journal_id, created_at FROM market_wallet_transfers";
fn transfer_row(row: &crate::db::Row<'_>) -> crate::db::Result<Transfer> {
    Ok(Transfer {
        id: row.get(0)?,
        sender_email: row.get(1)?,
        recipient_email: row.get(2)?,
        amount_units: row.get(3)?,
        journal_id: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn confirm_tx(
    conn: &Connection,
    runtime: &crate::market_payments::MarketPaymentsRuntime,
    sender: &str,
    idem: &str,
    input: &ConfirmRequest,
    time: &str,
) -> Result<Option<Transfer>, AppError> {
    // Success replay precedes expiry, delivery, consumed-code and pause checks.
    let prior = conn.query_row("SELECT id, quote_id, challenge_id, idempotency_key FROM market_wallet_transfers WHERE sender_user_id = ?1 AND (idempotency_key = ?2 OR quote_id = ?3) ORDER BY CASE WHEN idempotency_key = ?2 THEN 0 ELSE 1 END LIMIT 1",
        params![sender, idem, input.quote_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))
        .optional().map_err(db_error("read prior wallet transfer"))?;
    if let Some((id, quote, challenge, _)) = prior {
        if quote != input.quote_id || challenge != input.challenge_id {
            return Err(AppError::Conflict(
                "wallet transfer idempotency conflict".into(),
            ));
        }
        return conn
            .query_row(
                &format!("{TRANSFER_SELECT} WHERE id = ?1"),
                params![id],
                transfer_row,
            )
            .map(Some)
            .map_err(db_error("read prior transfer result"));
    }
    let quote = read_quote_tx(conn, sender, &input.quote_id)?;
    if quote.expires_at.as_str() <= time {
        return Err(problem(
            "WALLET_TRANSFER_QUOTE_EXPIRED",
            "transfer quote expired; review the transfer again",
        ));
    }
    let challenge = conn.query_row("SELECT code_mac, key_version, attempts, expires_at, delivered_at IS NOT NULL, consumed_at IS NOT NULL FROM market_wallet_transfer_challenges WHERE id = ?1 AND quote_id = ?2",
        params![input.challenge_id, input.quote_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?, row.get::<_, bool>(4)?, row.get::<_, bool>(5)?)))
        .optional().map_err(db_error("read transfer code"))?.ok_or_else(|| AppError::BadRequest("invalid transfer verification code".into()))?;
    if challenge.2 >= 5 || challenge.3.as_str() <= time || !challenge.4 || challenge.5 {
        return Err(problem(
            "WALLET_TRANSFER_CODE_EXPIRED",
            "verification code is expired, locked or not delivered; request another code",
        ));
    }
    conn.execute(
        "UPDATE market_wallet_transfer_challenges SET attempts = attempts + 1 WHERE id = ?1",
        params![input.challenge_id],
    )
    .map_err(db_error("record transfer code attempt"))?;
    if input.code.len() != 6
        || !input.code.bytes().all(|byte| byte.is_ascii_digit())
        || !runtime.verify_transfer_mac(
            challenge.1,
            &input.challenge_id,
            &input.quote_id,
            &input.code,
            &challenge.0,
        )?
    {
        return Ok(None);
    }
    require_transfer_tx(conn, sender, &quote.recipient, quote.amount_units, time)?;
    // Identity is stable, and a changed verified email requires a fresh quote.
    if require_user_tx(conn, sender)? != quote.sender_email
        || require_user_tx(conn, &quote.recipient)? != quote.recipient_email
    {
        return Err(problem(
            "WALLET_TRANSFER_QUOTE_EXPIRED",
            "recipient details changed; review the transfer again",
        ));
    }
    crate::market_wallet::ensure_wallet_tx(conn, &quote.recipient, time)?;
    wallet_snapshot_tx(conn, &quote.recipient)?
        .posted_units
        .checked_add(quote.amount_units)
        .ok_or_else(|| AppError::Conflict("recipient wallet balance limit exceeded".into()))?;
    let id = Uuid::new_v4().to_string();
    let fingerprint = format!(
        "{}:{}:{}:{}",
        sender, quote.recipient, quote.amount_units, quote.id
    );
    let journal = post_journal_tx(
        conn,
        "wallet_transfer",
        &id,
        "transfer",
        &fingerprint,
        &[
            Posting {
                kind: "wallet_available",
                subject_id: sender,
                signed_units: quote.amount_units,
            },
            Posting {
                kind: "wallet_available",
                subject_id: &quote.recipient,
                signed_units: -quote.amount_units,
            },
        ],
        time,
    )?;
    let changed = conn.execute("UPDATE market_wallet_accounts SET posted_units = posted_units - ?2, version = version + 1, updated_at = ?3 WHERE user_id = ?1 AND asset_code = 'USD' AND posted_units - held_units >= ?2",
        params![sender, quote.amount_units, time]).map_err(db_error("debit wallet transfer"))?;
    if changed != 1 {
        return Err(problem(
            "WALLET_TRANSFER_INSUFFICIENT",
            "available balance changed; review the transfer again",
        ));
    }
    conn.execute("UPDATE market_wallet_accounts SET posted_units = posted_units + ?2, version = version + 1, updated_at = ?3 WHERE user_id = ?1 AND asset_code = 'USD'",
        params![quote.recipient, quote.amount_units, time]).map_err(db_error("credit wallet transfer"))?;
    conn.execute("INSERT INTO market_wallet_transfers (id, sender_user_id, recipient_user_id, sender_email, recipient_email, amount_units, quote_id, challenge_id, idempotency_key, fingerprint, journal_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![id, sender, quote.recipient, quote.sender_email, quote.recipient_email, quote.amount_units, quote.id, input.challenge_id, idem, fingerprint, journal.id, time]).map_err(db_error("save wallet transfer"))?;
    conn.execute(
        "UPDATE market_wallet_transfer_challenges SET consumed_at = ?2 WHERE id = ?1",
        params![input.challenge_id, time],
    )
    .map_err(db_error("consume transfer code"))?;
    for (recipient, event) in [
        (sender, "wallet_transfer_sent"),
        (quote.recipient.as_str(), "wallet_transfer_received"),
    ] {
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "buyer",
            recipient,
            event,
            "wallet_transfer",
            &id,
            &format!("{event}:{id}"),
            serde_json::json!({ "transferId": id, "amountUnits": quote.amount_units.to_string(), "senderEmail": quote.sender_email, "recipientEmail": quote.recipient_email }),
            time,
        )?;
    }
    Ok(Some(Transfer {
        id,
        sender_email: quote.sender_email,
        recipient_email: quote.recipient_email,
        amount_units: quote.amount_units,
        journal_id: journal.id,
        created_at: time.into(),
    }))
}

async fn confirm(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ConfirmRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
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
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin wallet transfer"))?;
    let result = confirm_tx(
        &tx,
        &state.market_payments,
        &user.user_id,
        idem,
        &input,
        &now(),
    )?;
    tx.commit().map_err(db_error("commit wallet transfer"))?;
    let transfer = result.ok_or_else(|| {
        problem(
            "WALLET_TRANSFER_CODE_INVALID",
            "verification code is invalid",
        )
    })?;
    Ok((no_store(), Json(transfer)))
}

#[derive(Deserialize)]
struct ListQuery {
    offset: Option<i64>,
    limit: Option<i64>,
}
async fn list(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let offset = query.offset.unwrap_or(0).max(0);
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM market_wallet_transfers WHERE sender_user_id = ?1 OR recipient_user_id = ?1", params![user.user_id], |row| row.get(0)).map_err(db_error("count wallet transfers"))?;
    let mut stmt = conn.prepare(&format!("{TRANSFER_SELECT} WHERE sender_user_id = ?1 OR recipient_user_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2 OFFSET ?3")).map_err(db_error("prepare wallet transfers"))?;
    let items = stmt
        .query_map(params![user.user_id, limit, offset], transfer_row)
        .map_err(db_error("query wallet transfers"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read wallet transfers"))?;
    Ok((
        no_store(),
        Json(
            serde_json::json!({ "items": items, "total": total, "offset": offset, "limit": limit }),
        ),
    ))
}
async fn detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let transfer = conn.query_row(&format!("{TRANSFER_SELECT} WHERE id = ?1 AND (sender_user_id = ?2 OR recipient_user_id = ?2)"), params![id, user.user_id], transfer_row)
        .optional().map_err(db_error("read wallet transfer"))?.ok_or_else(|| AppError::NotFound("wallet transfer not found".into()))?;
    Ok((no_store(), Json(transfer)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Connection, crate::market_payments::MarketPaymentsRuntime) {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        conn.execute("UPDATE market_finance_control SET mode = 'enabled', cutover_approved_at = '2026-10-01T00:00:00Z' WHERE id = 1", []).unwrap();
        for (id, email) in [
            ("sender", "sender@example.test"),
            ("recipient", "recipient@example.test"),
        ] {
            conn.execute("INSERT INTO users (id, email_normalized, status, created_at, last_login_at, email_verified_at) VALUES (?1, ?2, 'active', 't', 't', 't')", params![id, email]).unwrap();
        }
        crate::market_wallet::credit_verified_topup_tx(
            &conn,
            "sender",
            "fixture-topup",
            1000,
            "receipt",
            "2026-10-01T00:00:00Z",
        )
        .unwrap();
        (
            conn,
            crate::market_payments::MarketPaymentsRuntime::for_test(),
        )
    }
    fn quoted(
        conn: &Connection,
        runtime: &crate::market_payments::MarketPaymentsRuntime,
        amount: i64,
    ) -> ConfirmRequest {
        quoted_as(conn, runtime, amount, "quote", "challenge")
    }
    fn quoted_as(
        conn: &Connection,
        runtime: &crate::market_payments::MarketPaymentsRuntime,
        amount: i64,
        quote: &str,
        challenge: &str,
    ) -> ConfirmRequest {
        conn.execute("INSERT INTO market_wallet_transfer_quotes (id, sender_user_id, recipient_user_id, sender_email, recipient_email, amount_units, expires_at, created_at) VALUES (?2, 'sender', 'recipient', 'sender@example.test', 'recipient@example.test', ?1, '2026-10-01T00:15:00Z', '2026-10-01T00:00:00Z')", params![amount, quote]).unwrap();
        let (version, mac) = runtime.transfer_mac(challenge, quote, "012345").unwrap();
        conn.execute("INSERT INTO market_wallet_transfer_challenges (id, quote_id, code_mac, key_version, expires_at, delivered_at, created_ip, created_at) VALUES (?3, ?4, ?1, ?2, '2026-10-01T00:10:00Z', '2026-10-01T00:00:01Z', '127.0.0.1', '2026-10-01T00:00:00Z')", params![mac, version, challenge, quote]).unwrap();
        ConfirmRequest {
            quote_id: quote.into(),
            challenge_id: challenge.into(),
            code: "012345".into(),
        }
    }

    #[test]
    fn racing_quotes_recheck_balance_and_daily_capacity_at_commit() {
        for daily_limit in [false, true] {
            let (conn, runtime) = fixture();
            let amount = units_from_minor(if daily_limit { 500 } else { 600 }).unwrap();
            if daily_limit {
                conn.execute("UPDATE market_wallet_policy SET transfer_max_minor = 500, transfer_daily_max_minor = 500", []).unwrap();
            }
            let first = quoted_as(&conn, &runtime, amount, "quote-a", "challenge-a");
            let second = quoted_as(&conn, &runtime, amount, "quote-b", "challenge-b");
            // Both tabs can quote the same original balance. IMMEDIATE commit
            // serializes them and rechecks the current balance and daily usage.
            for (index, input) in [&first, &second].into_iter().enumerate() {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .unwrap();
                let result = confirm_tx(
                    &tx,
                    &runtime,
                    "sender",
                    &format!("idem-key-{index}"),
                    input,
                    "2026-10-01T00:01:00Z",
                );
                if index == 0 {
                    assert!(result.unwrap().is_some());
                    tx.commit().unwrap();
                } else {
                    assert!(result.is_err());
                }
            }
            assert_eq!(
                wallet_snapshot_tx(&conn, "sender").unwrap().posted_units,
                units_from_minor(1000).unwrap() - amount
            );
            assert_eq!(
                wallet_snapshot_tx(&conn, "recipient").unwrap().posted_units,
                amount
            );
            if daily_limit {
                assert_eq!(
                    capacity_tx(&conn, "sender", "2026-10-01T23:59:59+00:00")
                        .unwrap()
                        .daily_remaining_units,
                    0
                );
                assert_eq!(
                    capacity_tx(&conn, "sender", "2026-10-02T00:00:00+00:00")
                        .unwrap()
                        .daily_remaining_units,
                    amount
                );
            }
            crate::market_wallet::reconcile_wallet_tx(&conn, "sender").unwrap();
            crate::market_wallet::reconcile_wallet_tx(&conn, "recipient").unwrap();
        }
    }

    #[test]
    fn transfers_preserve_due_service_commitments_and_existing_holds() {
        let (conn, runtime) = fixture();
        let input = quoted(&conn, &runtime, units_from_minor(701).unwrap());
        let at = "2026-10-01T00:01:00+00:00";
        crate::market_wallet::reserve_wallet_tx(
            &conn,
            "sender",
            "service",
            "reserved",
            "rental",
            units_from_minor(100).unwrap(),
            None,
            at,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_platform_contracts (
            id, product_kind, product_ref, trial_scope_ref, quote_id, request_fingerprint,
            buyer_user_id, provider_user_id, offer_revision, billing_model, cycle_price_minor,
            renewal_policy, status, created_at, updated_at, last_evaluated_at)
            VALUES ('due', 'share', 'due', 'due', 'service-quote', 'fp', 'sender', 'provider',
            1, 'calendar_month', 200, 'manual', 'trialing', ?1, ?1, ?1)",
            params![at],
        )
        .unwrap();
        // A not-yet-consumed free trial is not a due commitment, even if its
        // eventual price is large. Do not reserve all future service costs.
        conn.execute("INSERT INTO market_platform_contracts (
            id, product_kind, product_ref, trial_scope_ref, quote_id, request_fingerprint,
            buyer_user_id, provider_user_id, offer_revision, billing_model, cycle_price_minor,
            renewal_policy, status, trial_seconds_remaining, created_at, updated_at)
            VALUES ('future', 'share', 'future', 'future', 'future-quote', 'fp', 'sender', 'provider',
            1, 'calendar_month', 10000, 'automatic', 'trialing', 3600, ?1, ?1)", params![at]).unwrap();
        let capacity = capacity_tx(&conn, "sender", at).unwrap();
        assert_eq!(capacity.committed_units, units_from_minor(200).unwrap());
        assert_eq!(capacity.transferable_units, units_from_minor(700).unwrap());
        assert!(confirm_tx(&conn, &runtime, "sender", "idem-key-1", &input, at).is_err());
        assert_eq!(
            wallet_snapshot_tx(&conn, "sender").unwrap().posted_units,
            units_from_minor(1000).unwrap()
        );
    }

    #[test]
    fn pruning_unused_authorizations_preserves_financial_evidence() {
        let (conn, runtime) = fixture();
        let used = quoted(&conn, &runtime, 7);
        confirm_tx(
            &conn,
            &runtime,
            "sender",
            "idem-key-1",
            &used,
            "2026-10-01T00:01:00Z",
        )
        .unwrap()
        .unwrap();
        quoted_as(&conn, &runtime, 7, "unused", "unused-code");
        conn.execute("INSERT INTO market_wallet_transfer_attempts VALUES ('old-attempt', 'sender', '127.0.0.1', '2026-10-01T00:00:00Z')", []).unwrap();
        prune_unused_tx(
            &conn,
            chrono::DateTime::parse_from_rfc3339("2026-12-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();
        for table in [
            "market_wallet_transfer_quotes",
            "market_wallet_transfer_challenges",
            "market_wallet_transfers",
            "market_verified_wallet_transfer_evidence",
        ] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1,
                "{table}"
            );
        }
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_wallet_transfer_attempts",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            crate::market_reconcile::audit_finance_tx(&conn, "2026-12-01T00:00:00Z").unwrap(),
            0
        );
    }

    #[test]
    fn orphan_transfer_journal_creates_a_global_financial_hold() {
        let (conn, _) = fixture();
        post_journal_tx(
            &conn,
            "wallet_transfer",
            "orphan",
            "transfer",
            "missing-record",
            &[
                Posting {
                    kind: "wallet_available",
                    subject_id: "sender",
                    signed_units: 7,
                },
                Posting {
                    kind: "wallet_available",
                    subject_id: "recipient",
                    signed_units: -7,
                },
            ],
            "2026-10-01T00:00:00Z",
        )
        .unwrap();
        assert!(
            crate::market_reconcile::audit_finance_tx(&conn, "2026-10-02T00:00:00Z").unwrap() > 0
        );
        assert!(
            crate::market_reconcile::ensure_finance_scopes_safe_tx(
                &conn,
                &[crate::market_reconcile::FinanceScope::User("unrelated")]
            )
            .is_err()
        );
    }
    #[test]
    fn exact_transfer_is_atomic_and_idempotent_after_code_consumed_and_expired() {
        let (conn, runtime) = fixture();
        // Exact sub-cent units are frozen; the commit never re-evaluates "all".
        let amount = 86400000 - 7;
        let input = quoted(&conn, &runtime, amount);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let result = confirm_tx(
            &tx,
            &runtime,
            "sender",
            "idem-key-1",
            &input,
            "2026-10-01T00:01:00Z",
        )
        .unwrap()
        .unwrap();
        tx.commit().unwrap();
        let retry = confirm_tx(
            &conn,
            &runtime,
            "sender",
            "idem-key-1",
            &input,
            "2026-10-02T00:00:00Z",
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.id, retry.id);
        assert_eq!(wallet_snapshot_tx(&conn, "sender").unwrap().posted_units, 7);
        assert_eq!(
            wallet_snapshot_tx(&conn, "recipient").unwrap().posted_units,
            amount
        );
        crate::market_wallet::reconcile_wallet_tx(&conn, "sender").unwrap();
        crate::market_wallet::reconcile_wallet_tx(&conn, "recipient").unwrap();
        let notices: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_notification_outbox",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(notices, 2);
        assert!(
            conn.execute("UPDATE market_wallet_transfers SET amount_units = 1", [])
                .is_err()
        );
        assert_eq!(
            crate::market_reconcile::audit_finance_tx(&conn, "2026-10-02T00:00:00Z").unwrap(),
            0
        );
    }
    #[test]
    fn wrong_code_attempts_are_persisted_and_five_attempts_lock_the_challenge() {
        let (conn, runtime) = fixture();
        let mut input = quoted(&conn, &runtime, 86400);
        input.code = "999999".into();
        for _ in 0..5 {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(
                confirm_tx(
                    &tx,
                    &runtime,
                    "sender",
                    "idem-key-1",
                    &input,
                    "2026-10-01T00:01:00Z"
                )
                .unwrap()
                .is_none()
            );
            tx.commit().unwrap();
        }
        input.code = "012345".into();
        assert!(
            confirm_tx(
                &conn,
                &runtime,
                "sender",
                "idem-key-1",
                &input,
                "2026-10-01T00:01:00Z"
            )
            .is_err()
        );
        assert_eq!(
            wallet_snapshot_tx(&conn, "sender").unwrap().posted_units,
            86400000
        );
    }
    #[test]
    fn holds_identity_pauses_daily_limits_and_recipient_overflow_fail_closed() {
        let (conn, runtime) = fixture();
        let input = quoted(&conn, &runtime, 86400000);
        crate::market_wallet::reserve_wallet_tx(
            &conn,
            "sender",
            "test",
            "hold",
            "service",
            1,
            None,
            "2026-10-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(
            capacity_tx(&conn, "sender", "2026-10-01T00:00:00Z")
                .unwrap()
                .transferable_units,
            86399999
        );
        assert!(
            confirm_tx(
                &conn,
                &runtime,
                "sender",
                "idem-key-1",
                &input,
                "2026-10-01T00:01:00Z"
            )
            .is_err()
        );
        assert!(require_transfer_tx(&conn, "sender", "sender", 1, "2026-10-01T00:01:00Z").is_err());
        conn.execute(
            "UPDATE users SET email_verified_at = NULL WHERE id = 'recipient'",
            [],
        )
        .unwrap();
        assert!(
            require_transfer_tx(&conn, "sender", "recipient", 1, "2026-10-01T00:01:00Z").is_err()
        );
        conn.execute(
            "UPDATE users SET email_verified_at = 't', status = 'blocked' WHERE id = 'recipient'",
            [],
        )
        .unwrap();
        assert!(
            require_transfer_tx(&conn, "sender", "recipient", 1, "2026-10-01T00:01:00Z").is_err()
        );
        conn.execute(
            "UPDATE users SET status = 'active' WHERE id = 'recipient'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE market_wallet_policy SET transfers_paused = 1", [])
            .unwrap();
        assert!(
            require_transfer_tx(&conn, "sender", "recipient", 1, "2026-10-01T00:01:00Z").is_err()
        );
        conn.execute(
            "UPDATE market_wallet_policy SET transfers_paused = 0, transfer_daily_max_minor = 1",
            [],
        )
        .unwrap();
        assert!(
            require_transfer_tx(&conn, "sender", "recipient", 86401, "2026-10-01T00:01:00Z")
                .is_err()
        );
        // HMAC is bound to both quote and challenge, with versioned server keys.
        let (v, mac) = runtime.transfer_mac("c", "q", "123456").unwrap();
        assert!(
            !runtime
                .verify_transfer_mac(v, "c", "other", "123456", &mac)
                .unwrap()
        );
    }
    #[test]
    fn undelivered_codes_and_different_idempotency_payloads_never_move_money() {
        let (conn, runtime) = fixture();
        let input = quoted(&conn, &runtime, 100);
        conn.execute(
            "UPDATE market_wallet_transfer_challenges SET delivered_at = NULL",
            [],
        )
        .unwrap();
        assert!(
            confirm_tx(
                &conn,
                &runtime,
                "sender",
                "idem-key-1",
                &input,
                "2026-10-01T00:01:00Z"
            )
            .is_err()
        );
        conn.execute(
            "UPDATE market_wallet_transfer_challenges SET delivered_at = 't'",
            [],
        )
        .unwrap();
        confirm_tx(
            &conn,
            &runtime,
            "sender",
            "idem-key-1",
            &input,
            "2026-10-01T00:01:00Z",
        )
        .unwrap()
        .unwrap();
        let different = ConfirmRequest {
            quote_id: "different".into(),
            ..input
        };
        assert!(
            confirm_tx(
                &conn,
                &runtime,
                "sender",
                "idem-key-1",
                &different,
                "2026-10-01T00:02:00Z"
            )
            .is_err()
        );
        assert!(read_quote_tx(&conn, "recipient", "quote").is_err());
    }

    #[test]
    fn recipient_overflow_and_outbox_failure_roll_back_every_money_write() {
        let (conn, runtime) = fixture();
        let input = quoted(&conn, &runtime, 100);
        crate::market_wallet::ensure_wallet_tx(&conn, "recipient", "t").unwrap();
        conn.execute(
            "UPDATE market_wallet_accounts SET posted_units = ?1 WHERE user_id = 'recipient'",
            params![i64::MAX],
        )
        .unwrap();
        {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(
                confirm_tx(
                    &tx,
                    &runtime,
                    "sender",
                    "idem-key-1",
                    &input,
                    "2026-10-01T00:01:00Z"
                )
                .is_err()
            );
        }
        assert_eq!(
            wallet_snapshot_tx(&conn, "sender").unwrap().posted_units,
            86400000
        );
        conn.execute(
            "UPDATE market_wallet_accounts SET posted_units = 0 WHERE user_id = 'recipient'",
            [],
        )
        .unwrap();
        conn.execute_batch("CREATE TRIGGER reject_transfer_notice BEFORE INSERT ON market_notification_outbox BEGIN SELECT RAISE(ABORT, 'test outbox unavailable'); END;").unwrap();
        {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(
                confirm_tx(
                    &tx,
                    &runtime,
                    "sender",
                    "idem-key-1",
                    &input,
                    "2026-10-01T00:01:00Z"
                )
                .is_err()
            );
        }
        assert_eq!(
            wallet_snapshot_tx(&conn, "sender").unwrap().posted_units,
            86400000
        );
        assert_eq!(
            wallet_snapshot_tx(&conn, "recipient").unwrap().posted_units,
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM market_wallet_transfers", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM market_wallet_transfer_challenges WHERE consumed_at IS NOT NULL", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        conn.execute_batch("DROP TRIGGER reject_transfer_notice;")
            .unwrap();
        confirm_tx(
            &conn,
            &runtime,
            "sender",
            "idem-key-1",
            &input,
            "2026-10-01T00:01:00Z",
        )
        .unwrap()
        .unwrap();
        conn.execute("UPDATE market_wallet_policy SET transfers_paused = 1", [])
            .unwrap();
        assert!(
            confirm_tx(
                &conn,
                &runtime,
                "sender",
                "idem-key-1",
                &input,
                "2026-10-02T00:01:00Z"
            )
            .unwrap()
            .is_some()
        );
        assert_eq!(
            capacity_tx(&conn, "sender", "2026-10-02T00:01:00Z")
                .unwrap()
                .daily_remaining_units,
            500000 * 86400
        );
    }

    #[test]
    fn broken_transfer_evidence_freezes_both_users_and_cannot_be_dismissed() {
        let (conn, runtime) = fixture();
        let input = quoted(&conn, &runtime, 100);
        let result = confirm_tx(
            &conn,
            &runtime,
            "sender",
            "idem-key-1",
            &input,
            "2026-10-01T00:01:00Z",
        )
        .unwrap()
        .unwrap();
        // Simulate evidence corruption without editing any financial row.
        conn.execute("UPDATE market_wallet_transfer_challenges SET delivered_at = NULL WHERE id = 'challenge'", []).unwrap();
        assert_eq!(
            crate::market_reconcile::audit_finance_tx(&conn, "2026-10-02T00:00:00Z").unwrap(),
            1
        );
        for user in ["sender", "recipient"] {
            assert!(
                crate::market_reconcile::ensure_finance_scopes_safe_tx(
                    &conn,
                    &[crate::market_reconcile::FinanceScope::User(user)]
                )
                .is_err()
            );
        }
        crate::market_reconcile::ensure_finance_scopes_safe_tx(
            &conn,
            &[crate::market_reconcile::FinanceScope::User("unrelated")],
        )
        .unwrap();
        let case: String = conn
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases WHERE object_id = ?1",
                params![result.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            crate::market_reconcile::resolve_case_tx(&conn, &case, "owner", "reviewed", "t")
                .is_err()
        );
    }
}
