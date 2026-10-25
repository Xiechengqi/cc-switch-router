//! Same-Router, non-cash wallet transfers. Quotes freeze exact ledger units;
//! the existing Router login authorizes the reviewed quote without email OTP.
use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
use chrono::{Duration, Utc};
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
    blockers: Vec<&'static str>,
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
        .saturating_sub(crate::market_c2c::committed_transfer_units_tx(
            conn, user, time,
        )?)
        .max(0);
    let verified = match require_user_tx(conn, user) {
        Ok(_) => true,
        Err(e) if e.code() == Some("WALLET_TRANSFER_USER_UNVERIFIED") => false,
        Err(e) => return Err(e),
    };
    let mut blockers = Vec::new();
    if !policy.transfers_enabled {
        blockers.push("disabled");
    }
    if policy.transfers_paused {
        blockers.push("paused");
    }
    if !verified {
        blockers.push("identity");
    }
    match crate::market_reconcile::ensure_finance_scopes_safe_tx(
        conn,
        &[crate::market_reconcile::FinanceScope::User(user)],
    ) {
        Ok(()) => {}
        Err(e) if crate::market_reconcile::is_finance_hold(&e) => blockers.push("finance_hold"),
        Err(e) => return Err(e),
    }
    Ok(Capacity {
        transferable_units: available
            .saturating_sub(committed)
            .max(0)
            .min(units_from_minor(policy.transfer_max_minor)?)
            .min(remaining),
        committed_units: committed,
        daily_remaining_units: remaining,
        enabled: blockers.is_empty(),
        verified,
        blockers,
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
    let value = capacity_tx(&conn, &user.user_id, &now())?;
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
        "DELETE FROM market_earning_conversion_quotes WHERE id IN
         (SELECT q.id FROM market_earning_conversion_quotes q WHERE q.expires_at < ?1
          AND NOT EXISTS(SELECT 1 FROM market_earning_wallet_conversions c WHERE c.quote_id = q.id) LIMIT 2000)",
        params![old],
    ).map_err(db_error("prune unused conversion quotes"))?;
    conn.execute(
        "DELETE FROM market_wallet_transfer_attempts WHERE id IN
        (SELECT id FROM market_wallet_transfer_attempts WHERE created_at < ?1 LIMIT 2000)",
        params![(time - Duration::days(1)).to_rfc3339()],
    )
    .map_err(db_error("prune transfer rate limits"))?;
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
    let user = crate::market_finance_auth::write_session(&state, &headers).await?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfirmRequest {
    quote_id: String,
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
    authorization: &crate::models::AuthSession,
    sender: &str,
    idem: &str,
    input: &ConfirmRequest,
    time: &str,
) -> Result<Transfer, AppError> {
    if authorization.user_id != sender {
        return Err(AppError::Forbidden(
            "transfer session does not own the sender".into(),
        ));
    }
    // A successful replay precedes quote expiry and funding pause checks.
    let prior = conn.query_row("SELECT id, quote_id FROM market_wallet_transfers WHERE sender_user_id = ?1 AND (idempotency_key = ?2 OR quote_id = ?3) ORDER BY CASE WHEN idempotency_key = ?2 THEN 0 ELSE 1 END LIMIT 1",
        params![sender, idem, input.quote_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .optional().map_err(db_error("read prior wallet transfer"))?;
    if let Some((id, quote)) = prior {
        if quote != input.quote_id {
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
            .map_err(db_error("read prior transfer result"));
    }
    let quote = read_quote_tx(conn, sender, &input.quote_id)?;
    if quote.expires_at.as_str() <= time {
        return Err(problem(
            "WALLET_TRANSFER_QUOTE_EXPIRED",
            "transfer quote expired; review the transfer again",
        ));
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
    let authorization_id = crate::market_finance_auth::record_tx(
        conn,
        authorization,
        sender,
        "wallet_transfer",
        &id,
        &quote.recipient,
        quote.amount_units,
        0,
        &fingerprint,
        "transfer_confirmed",
        time,
    )?;
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
    conn.execute("INSERT INTO market_wallet_transfers (id, sender_user_id, recipient_user_id, sender_email, recipient_email, amount_units, quote_id, authorization_id, idempotency_key, fingerprint, journal_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![id, sender, quote.recipient, quote.sender_email, quote.recipient_email, quote.amount_units, quote.id, authorization_id, idem, fingerprint, journal.id, time]).map_err(db_error("save wallet transfer"))?;
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
    Ok(Transfer {
        id,
        sender_email: quote.sender_email,
        recipient_email: quote.recipient_email,
        amount_units: quote.amount_units,
        journal_id: journal.id,
        created_at: time.into(),
    })
}

async fn confirm(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ConfirmRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = crate::market_finance_auth::write_session(&state, &headers).await?;
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
    let result = confirm_tx(&tx, &user, &user.user_id, idem, &input, &now())?;
    tx.commit().map_err(db_error("commit wallet transfer"))?;
    Ok((no_store(), Json(result)))
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
pub(crate) fn verified_transfer_for_test(
    conn: &Connection,
    sender: &str,
    recipient: &str,
    amount: i64,
    at: &str,
) -> Result<(), AppError> {
    let quote = Uuid::new_v4().to_string();
    let expiry =
        (chrono::DateTime::parse_from_rfc3339(at).unwrap() + Duration::minutes(15)).to_rfc3339();
    conn.execute("INSERT INTO market_wallet_transfer_quotes(id,sender_user_id,recipient_user_id,sender_email,recipient_email,amount_units,expires_at,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![quote,sender,recipient,require_user_tx(conn,sender)?,require_user_tx(conn,recipient)?,amount,expiry,at]).unwrap();
    confirm_tx(
        conn,
        &crate::market_finance_auth::test_session(sender),
        sender,
        &Uuid::new_v4().to_string(),
        &ConfirmRequest { quote_id: quote },
        at,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const AT: &str = "2026-10-01T00:01:00Z";
    fn fixture() -> (Connection, crate::models::AuthSession) {
        let conn = crate::test_support::database().unwrap();
        for (id, email) in [
            ("sender", "sender@example.test"),
            ("recipient", "recipient@example.test"),
        ] {
            conn.execute("INSERT INTO users(id,email_normalized,status,created_at,last_login_at,email_verified_at) VALUES(?1,?2,'active','t','t','t')",params![id,email]).unwrap();
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
        (conn, crate::market_finance_auth::test_session("sender"))
    }
    fn quoted(conn: &Connection, amount: i64, id: &str) -> ConfirmRequest {
        conn.execute("INSERT INTO market_wallet_transfer_quotes(id,sender_user_id,recipient_user_id,sender_email,recipient_email,amount_units,expires_at,created_at) VALUES(?1,'sender','recipient','sender@example.test','recipient@example.test',?2,'2026-10-01T00:15:00Z','2026-10-01T00:00:00Z')",params![id,amount]).unwrap();
        ConfirmRequest {
            quote_id: id.into(),
        }
    }
    fn commit(
        conn: &Connection,
        auth: &crate::models::AuthSession,
        key: &str,
        input: &ConfirmRequest,
        time: &str,
    ) -> Result<Transfer, AppError> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let result = confirm_tx(&tx, auth, &auth.user_id, key, input, time)?;
        tx.commit().unwrap();
        Ok(result)
    }

    #[test]
    fn exact_session_transfer_is_atomic_idempotent_and_does_not_need_mail() {
        let (conn, auth) = fixture();
        let input = quoted(&conn, 86400000 - 7, "quote");
        let first = commit(&conn, &auth, "transfer-1", &input, AT).unwrap();
        conn.execute("UPDATE market_wallet_policy SET transfers_paused=1", [])
            .unwrap();
        let replay = commit(&conn, &auth, "transfer-1", &input, "2026-10-02T00:00:00Z").unwrap();
        assert_eq!(first.id, replay.id);
        assert_eq!(wallet_snapshot_tx(&conn, "sender").unwrap().posted_units, 7);
        assert_eq!(
            wallet_snapshot_tx(&conn, "recipient").unwrap().posted_units,
            86400000 - 7
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_wallet_transfer_challenges",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_financial_authorizations",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM market_notification_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            crate::market_reconcile::audit_finance_tx(&conn, "2026-10-02T00:00:00Z").unwrap(),
            0
        );
        assert!(
            conn.execute("UPDATE market_wallet_transfers SET amount_units=1", [])
                .is_err()
        );
        assert!(
            conn.execute("DELETE FROM market_financial_authorizations", [])
                .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE market_financial_authorizations SET amount_units=1",
                []
            )
            .is_err()
        );
        crate::market_wallet::reconcile_wallet_tx(&conn, "sender").unwrap();
        crate::market_wallet::reconcile_wallet_tx(&conn, "recipient").unwrap();
    }

    #[test]
    fn invalid_sessions_stale_quotes_and_old_code_payloads_never_authorize_money() {
        let (conn, mut auth) = fixture();
        let input = quoted(&conn, 100, "quote");
        assert!(
            confirm_tx(
                &conn,
                &crate::market_finance_auth::test_session("recipient"),
                "sender",
                "key-1",
                &input,
                AT
            )
            .is_err()
        );
        auth.access_expires_at = chrono::DateTime::parse_from_rfc3339(AT)
            .unwrap()
            .with_timezone(&Utc);
        assert!(commit(&conn, &auth, "key-1", &input, AT).is_err());
        assert!(
            commit(
                &conn,
                &crate::market_finance_auth::test_session("sender"),
                "key-1",
                &input,
                "2026-10-01T00:15:00Z"
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<ConfirmRequest>(
                serde_json::json!({"quoteId":"quote","challengeId":"old","code":"123456"})
            )
            .is_err()
        );
        assert_eq!(
            wallet_snapshot_tx(&conn, "sender").unwrap().posted_units,
            86400000
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_financial_authorizations",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn idempotency_payloads_and_other_users_quotes_are_not_interchangeable() {
        let (conn, auth) = fixture();
        let input = quoted(&conn, 100, "quote");
        commit(&conn, &auth, "key-1", &input, AT).unwrap();
        let other = quoted(&conn, 100, "other-quote");
        assert!(commit(&conn, &auth, "key-1", &other, AT).is_err());
        assert!(read_quote_tx(&conn, "recipient", "quote").is_err());
    }

    #[test]
    fn racing_quotes_recheck_balance_and_daily_capacity_at_commit() {
        for daily in [false, true] {
            let (conn, auth) = fixture();
            let amount = units_from_minor(if daily { 500 } else { 600 }).unwrap();
            if daily {
                conn.execute("UPDATE market_wallet_policy SET transfer_max_minor=500,transfer_daily_max_minor=500",[]).unwrap();
            }
            let first = quoted(&conn, amount, "first");
            let second = quoted(&conn, amount, "second");
            commit(&conn, &auth, "key-1", &first, AT).unwrap();
            assert!(commit(&conn, &auth, "key-2", &second, AT).is_err());
            assert_eq!(
                wallet_snapshot_tx(&conn, "sender").unwrap().posted_units,
                86400000 - amount
            );
            assert_eq!(
                wallet_snapshot_tx(&conn, "recipient").unwrap().posted_units,
                amount
            );
            crate::market_wallet::reconcile_wallet_tx(&conn, "sender").unwrap();
            crate::market_wallet::reconcile_wallet_tx(&conn, "recipient").unwrap();
        }
    }

    #[test]
    fn transfers_work_in_all_topup_modes_but_respect_their_own_pause() {
        let (conn, auth) = fixture();
        for (i, mode) in ["disabled", "shadow", "enabled"].iter().enumerate() {
            conn.execute("UPDATE market_finance_control SET mode=?1,cutover_approved_at=NULL,new_orders_paused=1,new_topups_paused=1",params![mode]).unwrap();
            assert!(capacity_tx(&conn, "sender", AT).unwrap().enabled);
            let input = quoted(&conn, 100, &format!("q-{i}"));
            commit(&conn, &auth, &format!("key-{i}"), &input, AT).unwrap();
        }
        conn.execute("UPDATE market_wallet_policy SET transfers_paused=1", [])
            .unwrap();
        assert_eq!(
            capacity_tx(&conn, "sender", AT).unwrap().blockers,
            vec!["paused"]
        );
        assert!(require_transfer_tx(&conn, "sender", "recipient", 1, AT).is_err());
    }

    #[test]
    fn changed_identity_blocked_users_self_transfers_and_limits_still_fail_closed() {
        let (conn, auth) = fixture();
        let input = quoted(&conn, 100, "q");
        assert!(require_transfer_tx(&conn, "sender", "sender", 1, AT).is_err());
        conn.execute(
            "UPDATE users SET email_normalized='changed@example.test' WHERE id='recipient'",
            [],
        )
        .unwrap();
        assert!(commit(&conn, &auth, "key-1", &input, AT).is_err());
        conn.execute("UPDATE users SET email_normalized='recipient@example.test',email_verified_at=NULL WHERE id='recipient'",[]).unwrap();
        assert!(require_transfer_tx(&conn, "sender", "recipient", 1, AT).is_err());
        conn.execute(
            "UPDATE users SET email_verified_at='t',status='blocked' WHERE id='recipient'",
            [],
        )
        .unwrap();
        assert!(require_transfer_tx(&conn, "sender", "recipient", 1, AT).is_err());
        conn.execute("UPDATE users SET status='active'", [])
            .unwrap();
        conn.execute(
            "UPDATE market_wallet_policy SET transfer_daily_max_minor=1",
            [],
        )
        .unwrap();
        assert!(require_transfer_tx(&conn, "sender", "recipient", 86401, AT).is_err());
    }

    #[test]
    fn recipient_overflow_and_outbox_failure_roll_back_authorization_and_money() {
        let (conn, auth) = fixture();
        let input = quoted(&conn, 100, "q");
        crate::market_wallet::ensure_wallet_tx(&conn, "recipient", AT).unwrap();
        conn.execute(
            "UPDATE market_wallet_accounts SET posted_units=?1 WHERE user_id='recipient'",
            params![i64::MAX],
        )
        .unwrap();
        assert!(commit(&conn, &auth, "key-1", &input, AT).is_err());
        conn.execute(
            "UPDATE market_wallet_accounts SET posted_units=0 WHERE user_id='recipient'",
            [],
        )
        .unwrap();
        conn.execute_batch("CREATE TRIGGER reject_transfer_notice BEFORE INSERT ON market_notification_outbox BEGIN SELECT RAISE(ABORT,'test outbox unavailable'); END;").unwrap();
        assert!(commit(&conn, &auth, "key-1", &input, AT).is_err());
        assert_eq!(
            wallet_snapshot_tx(&conn, "sender").unwrap().posted_units,
            86400000
        );
        for table in ["market_financial_authorizations", "market_wallet_transfers"] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        conn.execute_batch("DROP TRIGGER reject_transfer_notice;")
            .unwrap();
        commit(&conn, &auth, "key-1", &input, AT).unwrap();
    }

    #[test]
    fn pruning_unused_quotes_preserves_immutable_session_evidence() {
        let (conn, auth) = fixture();
        let used = quoted(&conn, 7, "used");
        commit(&conn, &auth, "key-1", &used, AT).unwrap();
        quoted(&conn, 7, "unused");
        conn.execute(
            "INSERT INTO market_wallet_transfer_attempts VALUES('old','sender','127.0.0.1',?1)",
            params![AT],
        )
        .unwrap();
        prune_unused_tx(
            &conn,
            chrono::DateTime::parse_from_rfc3339("2026-12-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();
        for table in [
            "market_wallet_transfer_quotes",
            "market_financial_authorizations",
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
    fn corrupt_session_evidence_holds_both_users_and_cannot_be_dismissed() {
        let (conn, auth) = fixture();
        let input = quoted(&conn, 100, "q");
        let result = commit(&conn, &auth, "key-1", &input, AT).unwrap();
        conn.execute_batch("DROP TRIGGER trg_market_financial_authorizations_no_update;")
            .unwrap();
        conn.execute(
            "UPDATE market_financial_authorizations SET amount_units=101",
            [],
        )
        .unwrap();
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
                "SELECT id FROM market_finance_reconciliation_cases WHERE object_id=?1",
                params![result.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            crate::market_reconcile::resolve_case_tx(&conn, &case, "owner", "reviewed", AT)
                .is_err()
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
            "missing",
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
            AT,
        )
        .unwrap();
        assert!(crate::market_reconcile::audit_finance_tx(&conn, AT).unwrap() > 0);
        assert!(
            crate::market_reconcile::ensure_finance_scopes_safe_tx(
                &conn,
                &[crate::market_reconcile::FinanceScope::User("unrelated")]
            )
            .is_err()
        );
    }

    #[test]
    fn automatic_earnings_remain_spendable_and_transferable_without_an_email_code() {
        let (conn, _) = fixture();
        conn.execute("INSERT INTO market_provider_profiles(id,canonical_email,user_id,display_name,claim_state,status,created_at,updated_at) VALUES('profile','recipient@example.test','recipient','Provider','claimed','active',?1,?1)",params![AT]).unwrap();
        crate::market_finance::record_metered_delivery_tx(
            &conn,
            "share",
            "service",
            "earned",
            "sender",
            "recipient",
            "2026-10-01T00:00:00Z",
            AT,
            units_from_minor(700).unwrap(),
            AT,
        )
        .unwrap();
        let settled = "2026-10-09T00:00:00Z";
        crate::market_finance::unlock_due_earnings_isolated(&conn, settled, 200).unwrap();
        crate::market_earning_conversions::credit_due_earnings_isolated(&conn, settled, 200)
            .unwrap();
        assert_eq!(
            capacity_tx(&conn, "recipient", settled)
                .unwrap()
                .transferable_units,
            units_from_minor(700).unwrap()
        );
        verified_transfer_for_test(
            &conn,
            "recipient",
            "sender",
            units_from_minor(700).unwrap(),
            settled,
        )
        .unwrap();
        assert_eq!(
            wallet_snapshot_tx(&conn, "recipient")
                .unwrap()
                .available_units,
            0
        );
        crate::market_wallet::reconcile_wallet_tx(&conn, "recipient").unwrap();
        crate::market_wallet::reconcile_wallet_tx(&conn, "sender").unwrap();
    }
    #[test]
    fn transfers_preserve_due_service_commitments_and_existing_holds() {
        let (conn, runtime) = fixture();
        let input = quoted(&conn, units_from_minor(701).unwrap(), "due-quote");
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
}
