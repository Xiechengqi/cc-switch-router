//! Automatic, exact-unit credit of matured net earnings to the same user's
//! wallet. Historical conversion evidence is retained, never rewritten.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    routing::{get, post},
};
#[cfg(test)]
use chrono::Utc;
use chrono::{DateTime, Duration};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[cfg(test)]
use crate::market_wallet::units_from_minor;
use crate::{
    ServerState,
    db::{Connection, OptionalExtension, TransactionBehavior, params},
    error::AppError,
    market_wallet::{Posting, post_journal_tx, serialize_units},
    market_wallet_policy::db_error,
};

const TERMS_VERSION: i64 = 1;
#[cfg(test)]
const MAX_LOTS: i64 = 10_000;
const SELECT: &str =
    "SELECT id, quote_id, amount_units, terms_version, journal_id, created_at, automatic
 FROM market_earning_wallet_conversions";
const ELIGIBLE: &str = "l.provider_user_id = ?1 AND l.disputed = 0
 AND l.unlocked_units - l.held_units - l.paid_units - l.converted_units > 0
 AND julianday(l.unlock_at) <= julianday(?2)
 AND NOT EXISTS (SELECT 1 FROM market_provider_earning_holds h
   WHERE h.lot_id = l.id AND h.released_at IS NULL)";

fn problem(code: &'static str, message: &str) -> AppError {
    AppError::Coded {
        status: StatusCode::CONFLICT,
        code,
        message: message.into(),
        details: serde_json::json!({}),
    }
}
fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
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
        .route(
            "/v1/market-provider-finance/wallet-conversion-quotes",
            post(retired_manual_conversion),
        )
        .route(
            "/v1/market-provider-finance/wallet-conversions",
            get(list).post(retired_manual_conversion),
        )
        .route(
            "/v1/market-provider-finance/wallet-conversions/:id",
            get(detail),
        )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Capacity {
    #[serde(serialize_with = "serialize_units")]
    pub convertible_units: i64,
    pub can_convert: bool,
    pub conversion_blockers: Vec<&'static str>,
}
pub(crate) fn capacity_tx(conn: &Connection, user: &str, at: &str) -> Result<Capacity, AppError> {
    let mut blockers = Vec::new();
    let verified: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1 AND status = 'active' AND email_verified_at IS NOT NULL)",
        params![user], |r| r.get(0),
    ).map_err(db_error("read conversion user"))?;
    if !verified || !crate::market_provider_finance::can_manage_earnings_tx(conn, user)? {
        blockers.push("identity");
    }
    // The obsolete manual-conversion pause never prevents automatic credit.
    match crate::market_reconcile::ensure_finance_scopes_safe_tx(
        conn,
        &[
            crate::market_reconcile::FinanceScope::User(user),
            crate::market_reconcile::FinanceScope::Provider(user),
        ],
    ) {
        Ok(()) => {}
        Err(e) if crate::market_reconcile::is_finance_hold(&e) => blockers.push("finance_hold"),
        Err(e) => return Err(e),
    }
    let amount: i64 = conn.query_row(
        &format!("SELECT COALESCE(SUM(l.unlocked_units - l.held_units - l.paid_units - l.converted_units), 0)
          FROM market_provider_earning_lots l WHERE {ELIGIBLE}"),
        params![user, at], |r| r.get(0),
    ).map_err(db_error("sum convertible earnings"))?;
    Ok(Capacity {
        convertible_units: amount,
        can_convert: blockers.is_empty(),
        conversion_blockers: blockers,
    })
}
fn require_open_tx(conn: &Connection, user: &str, at: &str) -> Result<Capacity, AppError> {
    let capacity = capacity_tx(conn, user, at)?;
    if !capacity.can_convert {
        return Err(problem(
            "EARNING_CONVERSION_UNAVAILABLE",
            capacity
                .conversion_blockers
                .first()
                .copied()
                .unwrap_or("unavailable"),
        ));
    }
    Ok(capacity)
}

#[cfg(test)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Quote {
    id: String,
    #[serde(serialize_with = "serialize_units")]
    amount_units: i64,
    terms_version: i64,
    expires_at: String,
}
#[cfg(test)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuoteRequest {
    amount_minor: Option<i64>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    batch: bool,
}

#[cfg(test)]
fn batch_units_tx(conn: &Connection, user: &str, at: &str, limit: i64) -> Result<i64, AppError> {
    conn.query_row(
        &format!(
            "SELECT COALESCE(SUM(available),0) FROM (
          SELECT l.unlocked_units-l.held_units-l.paid_units-l.converted_units AS available
          FROM market_provider_earning_lots l WHERE {ELIGIBLE}
          ORDER BY l.unlock_at,l.id LIMIT ?3)"
        ),
        params![user, at, limit],
        |r| r.get(0),
    )
    .map_err(db_error("sum next conversion batch"))
}
#[cfg(test)]
fn create_quote_tx(
    conn: &Connection,
    user: &str,
    input: &QuoteRequest,
    at: DateTime<Utc>,
) -> Result<Quote, AppError> {
    if u8::from(input.all) + u8::from(input.batch) + u8::from(input.amount_minor.is_some()) != 1 {
        return Err(AppError::BadRequest(
            "choose an amount, all earnings, or the next batch".into(),
        ));
    }
    let time = at.to_rfc3339();
    let capacity = require_open_tx(conn, user, &time)?;
    let amount = if input.batch {
        batch_units_tx(conn, user, &time, MAX_LOTS)?
    } else {
        match input.amount_minor {
            Some(amount) => units_from_minor(amount)?,
            None => capacity.convertible_units,
        }
    };
    if amount <= 0 || amount > capacity.convertible_units {
        return Err(problem(
            "EARNING_CONVERSION_INSUFFICIENT",
            "matured earnings changed; review the amount",
        ));
    }
    let recent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM market_earning_conversion_quotes WHERE user_id = ?1 AND created_at >= ?2",
        params![user, (at - Duration::hours(1)).to_rfc3339()], |r| r.get(0),
    ).map_err(db_error("limit conversion quotes"))?;
    if recent >= 30 {
        return Err(AppError::TooManyRequests(
            "too many conversion previews; try later".into(),
        ));
    }
    // The quote never locks lots. Confirm allocates the exact frozen amount
    // from the then-eligible pool, and never silently converts a smaller sum.
    selected_lots_tx(conn, user, amount, &time)?;
    let value = Quote {
        id: Uuid::new_v4().to_string(),
        amount_units: amount,
        terms_version: TERMS_VERSION,
        expires_at: (at + Duration::minutes(5)).to_rfc3339(),
    };
    conn.execute(
        "INSERT INTO market_earning_conversion_quotes (id,user_id,amount_units,terms_version,expires_at,created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![value.id,user,amount,TERMS_VERSION,value.expires_at,time],
    ).map_err(db_error("create conversion quote"))?;
    Ok(value)
}
struct Lot {
    id: String,
    amount: i64,
    converted_before: i64,
    contract: String,
}
#[cfg(test)]
fn selected_lots_tx(
    conn: &Connection,
    user: &str,
    amount: i64,
    time: &str,
) -> Result<Vec<Lot>, AppError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT l.id, l.unlocked_units - l.held_units - l.paid_units - l.converted_units,
          l.converted_units, d.contract_id FROM market_provider_earning_lots l
          JOIN market_delivery_slices d ON d.id = l.delivery_slice_id WHERE {ELIGIBLE}
          ORDER BY l.unlock_at, l.id LIMIT ?3"
        ))
        .map_err(db_error("prepare convertible lots"))?;
    let rows = stmt
        .query_map(params![user, time, MAX_LOTS], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(db_error("query convertible lots"))?;
    let mut left = amount;
    let mut lots = Vec::new();
    for row in rows {
        let (id, available, converted_before, contract) =
            row.map_err(db_error("read convertible lot"))?;
        let taken = left.min(available);
        lots.push(Lot {
            id,
            amount: taken,
            converted_before,
            contract,
        });
        left -= taken;
        if left == 0 {
            return Ok(lots);
        }
    }
    Err(problem(
        "EARNING_CONVERSION_BATCH_LIMIT",
        "earnings changed or more than 10000 lots are required; reduce the amount and convert in batches",
    ))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Conversion {
    automatic: bool,
    id: String,
    quote_id: String,
    #[serde(serialize_with = "serialize_units")]
    amount_units: i64,
    terms_version: i64,
    journal_id: String,
    created_at: String,
}
fn row(r: &crate::db::Row<'_>) -> crate::db::Result<Conversion> {
    Ok(Conversion {
        automatic: r.get(6)?,
        id: r.get(0)?,
        quote_id: r.get(1)?,
        amount_units: r.get(2)?,
        terms_version: r.get(3)?,
        journal_id: r.get(4)?,
        created_at: r.get(5)?,
    })
}
#[cfg(test)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfirmRequest {
    quote_id: String,
    terms_version: i64,
    acknowledge_non_withdrawable: bool,
}

fn verify_balances_tx(conn: &Connection, user: &str) -> Result<(), AppError> {
    crate::market_wallet::reconcile_wallet_tx(conn, user)?;
    let summary = crate::market_finance::provider_summary_tx(conn, user)?;
    let (pending, available, held):(i64,i64,i64) = conn.query_row(
        "SELECT COALESCE(SUM(CASE WHEN account_kind = 'provider_pending' THEN signed_units ELSE 0 END),0),
          COALESCE(SUM(CASE WHEN account_kind = 'provider_available' THEN signed_units ELSE 0 END),0),
          COALESCE(SUM(CASE WHEN account_kind = 'provider_held' THEN signed_units ELSE 0 END),0)
         FROM market_journal_postings WHERE subject_id = ?1 AND account_kind IN ('provider_pending','provider_available','provider_held')",
        params![user], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).map_err(db_error("check Provider conversion balances"))?;
    if pending != -summary.pending_units
        || available != -summary.available_units
        || held != -summary.withdrawal_held_units
    {
        return Err(problem(
            "MARKET_FINANCE_HOLD",
            "Provider ledger needs review",
        ));
    }
    Ok(())
}
pub(crate) fn lot_evidence_valid_tx(conn: &Connection, lot: &str) -> Result<bool, AppError> {
    EvidenceBatch::default().lot_valid_tx(conn, lot)
}

/// A transaction-local cache only: a conversion spanning thousands of lots
/// must not rescan the same immutable parent evidence once for every lot.
/// Drop this before changing conversion evidence or crossing transactions.
#[derive(Default)]
pub(crate) struct EvidenceBatch {
    conversions: HashMap<String, bool>,
}
impl EvidenceBatch {
    pub(crate) fn lot_valid_tx(&mut self, conn: &Connection, lot: &str) -> Result<bool, AppError> {
        let matches: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_provider_earning_lots l WHERE l.id = ?1
             AND l.converted_units = COALESCE((SELECT SUM(a.amount_units)
               FROM market_earning_conversion_allocations a WHERE a.earning_lot_id = l.id),0))",
                params![lot],
                |r| r.get(0),
            )
            .map_err(db_error("verify earning conversion allocation totals"))?;
        if !matches {
            return Ok(false);
        }
        let mut stmt = conn.prepare("SELECT conversion_id FROM market_earning_conversion_allocations WHERE earning_lot_id = ?1")
            .map_err(db_error("prepare earning conversion evidence"))?;
        let ids = stmt
            .query_map(params![lot], |r| r.get::<_, String>(0))
            .map_err(db_error("query earning conversion evidence"))?;
        for id in ids {
            let id = id.map_err(db_error("read earning conversion evidence"))?;
            let valid = match self.conversions.get(&id) {
                Some(valid) => *valid,
                None => {
                    let valid = conn.query_row("SELECT EXISTS(SELECT 1 FROM market_verified_earning_conversion_evidence WHERE id = ?1)",
                        params![id], |r| r.get(0)).map_err(db_error("verify earning conversion evidence"))?;
                    self.conversions.insert(id, valid);
                    valid
                }
            };
            if !valid {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
#[cfg(test)]
fn confirm_tx(
    conn: &Connection,
    user: &str,
    idem: &str,
    input: &ConfirmRequest,
    at: &str,
) -> Result<Conversion, AppError> {
    // Historical success is returned before expiry and pause checks, but never
    // before session ownership and request-fingerprint validation.
    let prior: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT id,quote_id,terms_version FROM (
             SELECT id,quote_id,terms_version,0 AS priority FROM market_earning_wallet_conversions
               WHERE user_id = ?1 AND idempotency_key = ?2
             UNION ALL SELECT id,quote_id,terms_version,1 AS priority FROM market_earning_wallet_conversions
               WHERE quote_id = ?3 AND user_id = ?1
             ) ORDER BY priority LIMIT 1",
            params![user, idem, input.quote_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(db_error("read prior conversion"))?;
    if let Some((id, quote, terms)) = prior {
        if quote != input.quote_id
            || terms != input.terms_version
            || !input.acknowledge_non_withdrawable
        {
            return Err(problem(
                "EARNING_CONVERSION_IDEMPOTENCY_CONFLICT",
                "conversion request changed",
            ));
        }
        return read_tx(conn, user, &id);
    }
    if input.terms_version != TERMS_VERSION || !input.acknowledge_non_withdrawable {
        return Err(problem(
            "EARNING_CONVERSION_TERMS_REQUIRED",
            "confirm the non-withdrawable balance terms",
        ));
    }
    let (amount,terms,expires):(i64,i64,String) = conn.query_row(
        "SELECT amount_units,terms_version,expires_at FROM market_earning_conversion_quotes WHERE id = ?1 AND user_id = ?2",
        params![input.quote_id,user], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional().map_err(db_error("read own conversion quote"))?
        .ok_or_else(|| AppError::Coded {
            status: StatusCode::NOT_FOUND,
            code: "EARNING_CONVERSION_QUOTE_NOT_FOUND",
            message: "conversion quote not found; review the amount again".into(),
            details: serde_json::json!({}),
        })?;
    let at_utc = DateTime::parse_from_rfc3339(at)
        .map_err(|_| AppError::Internal("invalid conversion timestamp".into()))?;
    if terms != input.terms_version
        || DateTime::parse_from_rfc3339(&expires)
            .map_err(|_| AppError::Internal("invalid conversion deadline".into()))?
            <= at_utc
    {
        return Err(problem(
            "EARNING_CONVERSION_QUOTE_EXPIRED",
            "preview expired; review the amount again",
        ));
    }
    let cap = require_open_tx(conn, user, at)?;
    if cap.convertible_units < amount {
        return Err(problem(
            "EARNING_CONVERSION_INSUFFICIENT",
            "matured earnings changed; review the amount",
        ));
    }
    let lots = selected_lots_tx(conn, user, amount, at)?;
    credit_lots_tx(conn, user, idem, &input.quote_id, amount, lots, at, false)
}

#[allow(clippy::too_many_arguments)]
fn credit_lots_tx(
    conn: &Connection,
    user: &str,
    idem: &str,
    quote_id: &str,
    amount: i64,
    lots: Vec<Lot>,
    at: &str,
    automatic: bool,
) -> Result<Conversion, AppError> {
    verify_balances_tx(conn, user)?;
    let mut evidence = EvidenceBatch::default();
    for lot in &lots {
        crate::market_reconcile::ensure_finance_scopes_safe_tx(
            conn,
            &[crate::market_reconcile::FinanceScope::Contract(
                &lot.contract,
            )],
        )?;
        if !crate::market_commission::lot_evidence_valid_tx(conn, &lot.id)?
            || !evidence.lot_valid_tx(conn, &lot.id)?
        {
            return Err(problem(
                "MARKET_FINANCE_HOLD",
                "source earning evidence needs review",
            ));
        }
    }
    crate::market_wallet::ensure_wallet_tx(conn, user, at)?;
    crate::market_wallet::wallet_snapshot_tx(conn, user)?
        .posted_units
        .checked_add(amount)
        .ok_or_else(|| AppError::Conflict("wallet balance limit exceeded".into()))?;
    let id = Uuid::new_v4().to_string();
    let journal = post_journal_tx(
        conn,
        "earning_conversion",
        &id,
        "convert",
        &format!("{user}:{quote_id}:{amount}:terms={TERMS_VERSION}"),
        &[
            Posting {
                kind: "provider_available",
                subject_id: user,
                signed_units: amount,
            },
            Posting {
                kind: "wallet_available",
                subject_id: user,
                signed_units: -amount,
            },
        ],
        at,
    )?;
    conn.execute(
        "INSERT INTO market_earning_wallet_conversions (id,user_id,quote_id,idempotency_key,amount_units,terms_version,journal_id,created_at,automatic)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id,user,quote_id,idem,amount,TERMS_VERSION,journal.id,at,automatic],
    ).map_err(db_error("record earning conversion"))?;
    for lot in lots {
        let changed = conn.execute(
            "UPDATE market_provider_earning_lots SET converted_units = converted_units + ?2
             WHERE id = ?1 AND provider_user_id = ?3 AND disputed = 0
              AND converted_units = ?4 AND unlocked_units - held_units - paid_units - converted_units >= ?2
              AND NOT EXISTS(SELECT 1 FROM market_provider_earning_holds h WHERE h.lot_id = ?1 AND h.released_at IS NULL)",
            params![lot.id,lot.amount,user,lot.converted_before],
        ).map_err(db_error("dispose converted earning"))?;
        if changed != 1 {
            return Err(problem(
                "EARNING_CONVERSION_INSUFFICIENT",
                "earning allocation changed",
            ));
        }
        conn.execute(
            "INSERT INTO market_earning_conversion_allocations (conversion_id,earning_lot_id,amount_units,converted_before_units)
             VALUES (?1,?2,?3,?4)",
            params![id,lot.id,lot.amount,lot.converted_before],
        ).map_err(db_error("allocate earning conversion"))?;
    }
    if conn.execute(
        "UPDATE market_wallet_accounts SET posted_units = posted_units + ?2, version = version + 1, updated_at = ?3
         WHERE user_id = ?1 AND asset_code = 'USD' AND posted_units <= ?4",
        params![user,amount,at,i64::MAX-amount],
    ).map_err(db_error("credit converted earnings"))? != 1 {
        return Err(AppError::Conflict("wallet balance changed".into()));
    }
    let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM market_verified_earning_conversion_evidence WHERE id = ?1)",
        params![id], |r| r.get(0)).map_err(db_error("verify completed conversion"))?;
    if !valid {
        return Err(AppError::Internal(
            "earning conversion evidence mismatch".into(),
        ));
    }
    verify_balances_tx(conn, user)?;
    conn.execute("INSERT INTO market_finance_audit_events (id,actor_user_id,action,object_kind,object_id,reason_code,created_at)
        VALUES (?1,?2,'convert','earning_conversion',?3,?4,?5)",
        params![Uuid::new_v4().to_string(),user,id,if automatic { "automatic_settlement" } else { "non_withdrawable_terms_v1" },at]).map_err(db_error("audit earning conversion"))?;
    // Automatic micro-batches are visible in wallet history. Do not send a
    // mandatory email per service slice every thirty seconds.
    if !automatic {
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "provider",
            user,
            "earnings_converted",
            "earning_conversion",
            &id,
            &format!("earnings_converted:{id}"),
            serde_json::json!({"conversionId":id,"amountUnits":amount.to_string(),"amountMinor":amount / crate::market_billing::MONEY_UNITS_PER_MINOR}),
            at,
        )?;
    }
    read_tx(conn, user, &id)
}
fn read_tx(conn: &Connection, user: &str, id: &str) -> Result<Conversion, AppError> {
    conn.query_row(
        &format!("{SELECT} WHERE id = ?1 AND user_id = ?2"),
        params![id, user],
        row,
    )
    .optional()
    .map_err(db_error("read own conversion"))?
    .ok_or_else(|| AppError::NotFound("conversion not found".into()))
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageQuery {
    cursor: Option<String>,
    quote_id: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    items: Vec<Conversion>,
    next_cursor: Option<String>,
}
fn page_tx(conn: &Connection, user: &str, q: &PageQuery) -> Result<Page, AppError> {
    let cursor_at = q
        .cursor
        .as_ref()
        .map(|id| read_tx(conn, user, id).map(|v| v.created_at))
        .transpose()?;
    let mut stmt = conn
        .prepare(&format!(
            "{SELECT} WHERE user_id = ?1
        AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id < ?3))
        AND (?4 IS NULL OR quote_id = ?4) ORDER BY created_at DESC,id DESC LIMIT 51"
        ))
        .map_err(db_error("prepare conversion history"))?;
    let mut items = stmt
        .query_map(params![user, cursor_at, q.cursor, q.quote_id], row)
        .map_err(db_error("query conversion history"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read conversion history"))?;
    let more = items.len() > 50;
    items.truncate(50);
    let next_cursor = more.then(|| items.last().unwrap().id.clone());
    Ok(Page { items, next_cursor })
}
async fn retired_manual_conversion(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    session(&state, &headers).await?;
    Err(AppError::Coded {
        status: StatusCode::GONE,
        code: "MANUAL_EARNING_CONVERSIONS_RETIRED",
        message: "matured earnings are automatically credited to platform balance; no confirmation is needed".into(),
        details: serde_json::json!({ "walletUrl": "/account/wallet/" }),
    })
}

/// One eligible lot, exact to the ledger unit. The old `quote` table is now
/// just a frozen evidence envelope created and consumed in this transaction;
/// there is no browser quote, user consent, fee, rate limit or external call.
fn automatic_credit_lot_tx(conn: &Connection, lot_id: &str, at: &str) -> Result<i64, AppError> {
    let lot = conn.query_row(&format!("SELECT l.provider_user_id,
        l.unlocked_units-l.held_units-l.paid_units-l.converted_units,l.converted_units,d.contract_id
        FROM market_provider_earning_lots l JOIN market_delivery_slices d ON d.id=l.delivery_slice_id
        WHERE l.id=?3 AND {ELIGIBLE}"),
        // Resolve the owner from this lot, not from caller-controlled input.
        params![conn.query_row("SELECT provider_user_id FROM market_provider_earning_lots WHERE id=?1",params![lot_id],|r|r.get::<_,String>(0))
            .map_err(db_error("read automatic credit owner"))?,at,lot_id],
        |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?)))
        .optional().map_err(db_error("read automatic credit lot"))?;
    let Some((user, amount, converted_before, contract)) = lot else {
        return Ok(0);
    };
    require_open_tx(conn, &user, at)?;
    let quote_id = Uuid::new_v4().to_string();
    let expires = (DateTime::parse_from_rfc3339(at)
        .map_err(|_| AppError::Internal("invalid settlement time".into()))?
        + Duration::minutes(5))
    .to_rfc3339();
    conn.execute("INSERT INTO market_earning_conversion_quotes (id,user_id,amount_units,terms_version,expires_at,created_at)
        VALUES (?1,?2,?3,1,?4,?5)",params![quote_id,user,amount,expires,at]).map_err(db_error("freeze automatic credit evidence"))?;
    let lot = Lot {
        id: lot_id.into(),
        amount,
        converted_before,
        contract,
    };
    credit_lots_tx(
        conn,
        &user,
        &format!("automatic:{lot_id}:{converted_before}"),
        &quote_id,
        amount,
        vec![lot],
        at,
        true,
    )?;
    Ok(amount)
}

/// Persisted fair, bounded scan with one savepoint per lot. Frozen or invalid
/// earnings never stop other users (or other contracts of the same user).
pub(crate) fn credit_due_earnings_isolated(
    conn: &Connection,
    at: &str,
    limit: i64,
) -> Result<usize, AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin automatic earnings credit"))?;
    let cursor: String = tx
        .query_row(
            "SELECT earning_settlement_cursor FROM market_finance_control WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(db_error("read automatic credit cursor"))?;
    let ids = {
        let mut stmt=tx.prepare("SELECT l.id FROM market_provider_earning_lots l WHERE l.id>?1 AND l.disputed=0
            AND l.unlocked_units-l.held_units-l.paid_units-l.converted_units>0 AND julianday(l.unlock_at)<=julianday(?2)
            AND NOT EXISTS(SELECT 1 FROM market_provider_earning_holds h WHERE h.lot_id=l.id AND h.released_at IS NULL)
            ORDER BY l.id LIMIT ?3").map_err(db_error("prepare automatic credit candidates"))?;
        stmt.query_map(params![cursor, at, limit.clamp(1, 1000)], |r| {
            r.get::<_, String>(0)
        })
        .map_err(db_error("query automatic credit candidates"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read automatic credit candidates"))?
    };
    let mut count = 0;
    for id in &ids {
        tx.execute_batch("SAVEPOINT automatic_earning_credit")
            .map_err(db_error("open automatic credit savepoint"))?;
        match automatic_credit_lot_tx(&tx, id, at) {
            Ok(amount) => {
                tx.execute_batch("RELEASE SAVEPOINT automatic_earning_credit")
                    .map_err(db_error("release automatic credit savepoint"))?;
                count += usize::from(amount > 0);
            }
            Err(error) => {
                tx.execute_batch("ROLLBACK TO SAVEPOINT automatic_earning_credit; RELEASE SAVEPOINT automatic_earning_credit")
                    .map_err(db_error("roll back automatic credit savepoint"))?;
                tracing::warn!(lot_id=%id,code=error.code().unwrap_or("settlement_deferred"),"automatic earnings credit deferred; funds unchanged");
            }
        }
    }
    tx.execute(
        "UPDATE market_finance_control SET earning_settlement_cursor=?1 WHERE id=1",
        params![ids.last().map(String::as_str).unwrap_or("")],
    )
    .map_err(db_error("advance automatic credit cursor"))?;
    tx.commit()
        .map_err(db_error("commit automatic earnings credit"))?;
    Ok(count)
}
async fn list(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(page_tx(&conn, &user.user_id, &q)?)))
}
async fn detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let user = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(read_tx(&conn, &user.user_id, &id)?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_platform_contracts::{self as contracts, BillingModel, PrepareContract};
    const START: &str = "2026-01-10T00:00:00+00:00";
    const END: &str = "2026-02-10T00:00:00+00:00";
    const AT: &str = "2026-02-18T00:00:00+00:00";
    #[test]
    fn automatic_credit_is_exactly_once_net_of_commission_and_requires_no_action() {
        for product in ["share", "client_host"] {
            let (conn, _) = fixture(1000, product);
            assert_eq!(credit_due_earnings_isolated(&conn, AT, 200).unwrap(), 1);
            assert_eq!(credit_due_earnings_isolated(&conn, AT, 200).unwrap(), 0);
            let summary = crate::market_finance::provider_summary_tx(&conn, "provider").unwrap();
            assert_eq!(summary.converted_units, units_from_minor(900).unwrap());
            assert_eq!(summary.available_units, 0);
            assert_eq!(
                crate::market_wallet::wallet_snapshot_tx(&conn, "provider")
                    .unwrap()
                    .available_units,
                summary.converted_units
            );
            assert_eq!(
                number(
                    &conn,
                    "SELECT COUNT(*) FROM market_earning_wallet_conversions WHERE automatic=1"
                ),
                1
            );
            assert_eq!(
                number(
                    &conn,
                    "SELECT COUNT(*) FROM market_verified_earning_conversion_evidence"
                ),
                1
            );
            assert_eq!(
                number(&conn, "SELECT COUNT(*) FROM market_payout_requests"),
                0
            );
            assert_eq!(
                number(
                    &conn,
                    "SELECT COUNT(*) FROM market_notification_outbox WHERE event_kind='earnings_converted'"
                ),
                0
            );
            verify_balances_tx(&conn, "provider").unwrap();
            assert_eq!(
                crate::market_reconcile::audit_finance_tx(&conn, AT).unwrap(),
                0
            );
        }
    }
    #[test]
    fn automatic_credit_obeys_exact_maturity_identity_and_dispute_holds() {
        let (conn, contract) = fixture(1000, "share");
        assert_eq!(
            credit_due_earnings_isolated(&conn, "2026-02-16T23:59:59Z", 200).unwrap(),
            0
        );
        crate::market_finance::hold_contract_earnings_tx(&conn, &contract, "manual", AT).unwrap();
        assert_eq!(credit_due_earnings_isolated(&conn, AT, 200).unwrap(), 0);
        crate::market_finance::release_earning_holds_tx(
            &conn,
            "manual",
            &contract,
            "owner",
            "review_complete",
            AT,
        )
        .unwrap();
        conn.execute("UPDATE users SET status='disabled' WHERE id='provider'", [])
            .unwrap();
        assert_eq!(credit_due_earnings_isolated(&conn, AT, 200).unwrap(), 0);
        conn.execute("UPDATE users SET status='active' WHERE id='provider'", [])
            .unwrap();
        conn.execute(
            "UPDATE market_wallet_policy SET earnings_conversions_paused=1",
            [],
        )
        .unwrap();
        // Old manual switches and purchase/top-up pauses cannot stop already
        // earned income from settling into the wallet.
        conn.execute("UPDATE market_finance_control SET new_orders_paused=1,new_topups_paused=1,new_withdrawals_paused=1",[]).unwrap();
        let mut credited = 0;
        for _ in 0..3 {
            credited += credit_due_earnings_isolated(&conn, "2026-02-17T00:00:00Z", 200).unwrap();
        }
        assert_eq!(credited, 1);
        verify_balances_tx(&conn, "provider").unwrap();
    }
    #[test]
    fn automatic_credit_keeps_fractional_units_and_has_no_manual_preview_rate_limit() {
        let (conn, _) = fixture(0, "share");
        for i in 0..35 {
            crate::market_finance::record_metered_delivery_tx(
                &conn,
                "share",
                "legacy",
                &format!("tiny-{i}"),
                "buyer",
                "provider",
                START,
                "2026-01-10T00:00:01Z",
                1,
                END,
            )
            .unwrap();
        }
        crate::market_finance::unlock_due_earnings_tx(&conn, AT, 200).unwrap();
        assert_eq!(credit_due_earnings_isolated(&conn, AT, 200).unwrap(), 36);
        assert_eq!(
            crate::market_wallet::wallet_snapshot_tx(&conn, "provider")
                .unwrap()
                .available_units,
            units_from_minor(1000).unwrap() + 35
        );
        verify_balances_tx(&conn, "provider").unwrap();
    }
    #[test]
    fn automatic_credit_rollback_leaves_no_partial_evidence_or_money_and_retries() {
        let (conn, _) = fixture(1000, "share");
        conn.execute_batch("CREATE TRIGGER fail_auto_credit BEFORE UPDATE OF posted_units ON market_wallet_accounts WHEN NEW.user_id='provider' BEGIN SELECT RAISE(ABORT,'injected wallet failure'); END;").unwrap();
        assert_eq!(credit_due_earnings_isolated(&conn, AT, 200).unwrap(), 0);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_earning_wallet_conversions"
            ),
            0
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_earning_conversion_quotes"
            ),
            0
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_journal_transactions WHERE source_kind='earning_conversion'"
            ),
            0
        );
        conn.execute_batch("DROP TRIGGER fail_auto_credit").unwrap();
        let mut credited = 0;
        for _ in 0..3 {
            credited += credit_due_earnings_isolated(&conn, AT, 200).unwrap();
        }
        assert_eq!(credited, 1);
        verify_balances_tx(&conn, "provider").unwrap();
    }
    #[test]
    fn automatic_credit_round_robin_does_not_starve_unrelated_contracts() {
        let (conn, contract) = fixture(0, "share");
        crate::market_finance::record_metered_delivery_tx(
            &conn,
            "share",
            "other-contract",
            "tiny",
            "buyer",
            "provider",
            START,
            "2026-01-10T00:00:01Z",
            1,
            END,
        )
        .unwrap();
        crate::market_finance::unlock_due_earnings_tx(&conn, AT, 200).unwrap();
        let period: String = conn
            .query_row(
                "SELECT id FROM market_platform_periods WHERE contract_id=?1",
                params![contract],
                |r| r.get(0),
            )
            .unwrap();
        crate::market_reconcile::case_tx(
            &conn,
            "period_unfulfilled_mismatch",
            "period",
            &period,
            AT,
        )
        .unwrap();
        let mut credited = 0;
        for _ in 0..5 {
            credited += credit_due_earnings_isolated(&conn, AT, 1).unwrap();
        }
        assert_eq!(credited, 1);
        assert_eq!(
            crate::market_wallet::wallet_snapshot_tx(&conn, "provider")
                .unwrap()
                .available_units,
            1
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_earning_wallet_conversions"
            ),
            1
        );
        verify_balances_tx(&conn, "provider").unwrap();
    }
    fn fixture(rate: i64, kind: &str) -> (Connection, String) {
        fixture_on(crate::test_support::database().unwrap(), rate, kind)
    }
    fn fixture_on(conn: Connection, rate: i64, kind: &str) -> (Connection, String) {
        crate::schema::apply(&conn).unwrap();
        conn.execute(
            "UPDATE market_finance_control SET mode='enabled',cutover_approved_at=?1",
            params![START],
        )
        .unwrap();
        for (id, email) in [
            ("provider", "provider@example.test"),
            ("buyer", "buyer@example.test"),
            ("other", "other@example.test"),
        ] {
            conn.execute("INSERT INTO users (id,email_normalized,status,email_verified_at,created_at,last_login_at)
                VALUES (?1,?2,'active',?3,?3,?3)",params![id,email,START]).unwrap();
        }
        conn.execute("INSERT INTO market_provider_profiles (id,canonical_email,user_id,display_name,claim_state,status,created_at,updated_at)
            VALUES ('profile','provider@example.test','provider','Provider','claimed','active',?1,?1)",params![START]).unwrap();
        conn.execute("INSERT INTO market_commission_policies VALUES (2,?1,'2026-01-01T00:00:00+00:00','owner',?2)",params![rate,START]).unwrap();
        conn.execute("UPDATE market_commission_policy_control SET version=2", [])
            .unwrap();
        crate::market_wallet::credit_verified_topup_tx(
            &conn,
            "buyer",
            "test-topup",
            10000,
            "receipt",
            START,
        )
        .unwrap();
        crate::market_commission::freeze_quote_tx(&conn, "test-quote", "provider", START).unwrap();
        let contract = contracts::prepare_contract_tx(
            &conn,
            &PrepareContract {
                product_kind: kind,
                product_ref: "product",
                trial_scope_ref: "service",
                quote_id: "test-quote",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::CalendarMonth,
                price_minor: 1000,
                auto_renew: false,
                trial_seconds: 0,
            },
            START,
        )
        .unwrap();
        contracts::activate_contract_tx(&conn, &contract, START, START).unwrap();
        contracts::settle_monthly_segment_tx(&conn, &contract, "delivery", START, END, true, END)
            .unwrap();
        crate::market_finance::unlock_due_earnings_tx(&conn, AT, 200).unwrap();
        (conn, contract)
    }
    fn preview(conn: &Connection, minor: Option<i64>) -> Quote {
        create_quote_tx(
            conn,
            "provider",
            &QuoteRequest {
                amount_minor: minor,
                all: minor.is_none(),
                batch: false,
            },
            DateTime::parse_from_rfc3339(AT)
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap()
    }
    fn input(q: &Quote) -> ConfirmRequest {
        ConfirmRequest {
            quote_id: q.id.clone(),
            terms_version: 1,
            acknowledge_non_withdrawable: true,
        }
    }
    fn convert(conn: &Connection, q: &Quote, key: &str) -> Result<Conversion, AppError> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let result = confirm_tx(&tx, "provider", key, &input(q), AT)?;
        tx.commit().unwrap();
        Ok(result)
    }
    fn number(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn explicit_next_batch_is_bounded_and_keeps_subcent_earnings() {
        for rate in [0, 10000] {
            let (conn, _) = fixture(rate, "share");
            crate::market_finance::record_metered_delivery_tx(
                &conn,
                "share",
                "legacy",
                "tiny-batch",
                "buyer",
                "provider",
                START,
                "2026-01-10T00:00:01+00:00",
                1,
                START,
            )
            .unwrap();
            crate::market_finance::unlock_due_earnings_tx(&conn, AT, 200).unwrap();
            assert_eq!(batch_units_tx(&conn, "provider", AT, 1).unwrap(), 1);
            let at = DateTime::parse_from_rfc3339(AT)
                .unwrap()
                .with_timezone(&Utc);
            for value in [
                serde_json::json!({"all":true,"batch":true}),
                serde_json::json!({"amountMinor":1,"batch":true}),
                serde_json::json!({}),
            ] {
                let input: QuoteRequest = serde_json::from_value(value).unwrap();
                assert!(create_quote_tx(&conn, "provider", &input, at).is_err());
            }
            let input: QuoteRequest =
                serde_json::from_value(serde_json::json!({"batch":true})).unwrap();
            let q = create_quote_tx(&conn, "provider", &input, at).unwrap();
            assert_eq!(
                q.amount_units,
                if rate == 10000 {
                    1
                } else {
                    units_from_minor(1000).unwrap() + 1
                }
            );
            convert(&conn, &q, "explicit-batch").unwrap();
            assert_eq!(
                capacity_tx(&conn, "provider", AT)
                    .unwrap()
                    .convertible_units,
                0
            );
        }
    }

    #[test]
    fn multi_lot_evidence_is_checked_once_per_parent_and_never_cached_across_writes() {
        let (conn, _) = fixture(0, "share");
        crate::market_finance::record_metered_delivery_tx(
            &conn,
            "share",
            "legacy",
            "tiny",
            "buyer",
            "provider",
            START,
            "2026-01-10T00:00:01+00:00",
            1,
            END,
        )
        .unwrap();
        crate::market_finance::unlock_due_earnings_tx(&conn, AT, 200).unwrap();
        let q = preview(&conn, None);
        convert(&conn, &q, "multi-lot").unwrap();
        let lots = conn
            .prepare("SELECT id FROM market_provider_earning_lots ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<crate::db::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(lots.len(), 2);
        let mut batch = EvidenceBatch::default();
        for lot in &lots {
            assert!(batch.lot_valid_tx(&conn, lot).unwrap());
        }
        assert_eq!(batch.conversions.len(), 1);
        assert!(!batch.lot_valid_tx(&conn, "missing").unwrap());
        drop(batch);
        conn.execute_batch("DROP TRIGGER trg_market_earning_conversion_quote_update")
            .unwrap();
        conn.execute(
            "UPDATE market_earning_conversion_quotes SET amount_units=amount_units+1 WHERE id=?1",
            params![q.id],
        )
        .unwrap();
        for lot in &lots {
            assert!(!lot_evidence_valid_tx(&conn, lot).unwrap());
        }
    }

    #[test]
    fn unused_pruned_preview_is_a_definitive_rejection_but_used_quotes_are_preserved() {
        let (conn, _) = fixture(0, "share");
        let unused = preview(&conn, Some(1));
        let used = preview(&conn, Some(1));
        let done = convert(&conn, &used, "kept").unwrap();
        let later = DateTime::parse_from_rfc3339(AT).unwrap() + Duration::days(40);
        crate::market_wallet_transfers::prune_unused_tx(&conn, later.with_timezone(&Utc)).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(matches!(
            confirm_tx(
                &tx,
                "provider",
                "unused",
                &input(&unused),
                &later.to_rfc3339()
            ),
            Err(AppError::Coded {
                status: StatusCode::NOT_FOUND,
                code: "EARNING_CONVERSION_QUOTE_NOT_FOUND",
                ..
            })
        ));
        assert_eq!(
            confirm_tx(&tx, "provider", "kept", &input(&used), &later.to_rfc3339())
                .unwrap()
                .id,
            done.id
        );
    }

    #[test]
    fn same_user_conversion_preserves_net_fee_and_total_liabilities_for_both_markets() {
        for kind in ["share", "client_host"] {
            let (conn, _) = fixture(1000, kind);
            let before = crate::market_finance::provider_summary_tx(&conn, "provider").unwrap();
            let q = preview(&conn, Some(300));
            let value = convert(&conn, &q, "once").unwrap();
            let after = crate::market_finance::provider_summary_tx(&conn, "provider").unwrap();
            assert_eq!(after.net_earned_units, before.net_earned_units);
            assert_eq!(after.commission_units, before.commission_units);
            assert_eq!(after.paid_units, 0);
            assert_eq!(after.converted_units, units_from_minor(300).unwrap());
            assert_eq!(after.withdrawable_units, units_from_minor(600).unwrap());
            assert_eq!(
                crate::market_wallet::wallet_snapshot_tx(&conn, "provider")
                    .unwrap()
                    .available_units,
                value.amount_units
            );
            assert_eq!(
                number(
                    &conn,
                    "SELECT COUNT(*) FROM market_verified_earning_conversion_evidence"
                ),
                1
            );
            verify_balances_tx(&conn, "provider").unwrap();
        }
    }
    #[test]
    fn all_preserves_subcent_units_and_zero_net_cannot_convert() {
        for rate in [0, 10000] {
            let (conn, _) = fixture(rate, "share");
            if rate == 10000 {
                assert!(
                    create_quote_tx(
                        &conn,
                        "provider",
                        &QuoteRequest {
                            amount_minor: None,
                            all: true,
                            batch: false,
                        },
                        DateTime::parse_from_rfc3339(AT)
                            .unwrap()
                            .with_timezone(&Utc)
                    )
                    .is_err()
                );
                continue;
            }
            crate::market_finance::record_metered_delivery_tx(
                &conn,
                "share",
                "legacy",
                "tiny",
                "buyer",
                "provider",
                START,
                "2026-01-10T00:00:01+00:00",
                1,
                END,
            )
            .unwrap();
            crate::market_finance::unlock_due_earnings_tx(&conn, AT, 200).unwrap();
            let q = preview(&conn, None);
            assert_eq!(q.amount_units, units_from_minor(1000).unwrap() + 1);
            convert(&conn, &q, "all").unwrap();
            assert_eq!(
                capacity_tx(&conn, "provider", AT)
                    .unwrap()
                    .convertible_units,
                0
            );
            assert_eq!(
                crate::market_wallet::wallet_snapshot_tx(&conn, "provider")
                    .unwrap()
                    .available_units,
                q.amount_units
            );
        }
    }
    #[test]
    fn idempotency_and_quote_uniqueness_survive_expiry_pause_and_different_retry_keys() {
        let (conn, _) = fixture(1000, "share");
        let q = preview(&conn, Some(200));
        let first = convert(&conn, &q, "one").unwrap();
        conn.execute(
            "UPDATE market_wallet_policy SET earnings_conversions_paused=1",
            [],
        )
        .unwrap();
        let retry = confirm_tx(
            &conn,
            "provider",
            "different-key",
            &input(&q),
            "2027-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(first.id, retry.id);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_earning_wallet_conversions"
            ),
            1
        );
        let mut changed = input(&q);
        changed.acknowledge_non_withdrawable = false;
        assert!(confirm_tx(&conn, "provider", "one", &changed, AT).is_err());
    }
    #[test]
    fn stale_quotes_never_partially_convert_and_all_is_a_frozen_amount() {
        let (conn, _) = fixture(1000, "share");
        let all = preview(&conn, None);
        let part = preview(&conn, Some(100));
        convert(&conn, &part, "part").unwrap();
        assert!(convert(&conn, &all, "stale").is_err());
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_earning_wallet_conversions"
            ),
            1
        );
        let fresh = preview(&conn, None);
        let value = convert(&conn, &fresh, "rest").unwrap();
        assert_eq!(value.amount_units, units_from_minor(800).unwrap());
    }
    #[test]
    fn identity_mode_pause_and_freeze_checks_are_independent_of_payout_address_and_gateway() {
        let (conn, contract) = fixture(1000, "share");
        assert!(capacity_tx(&conn, "provider", AT).unwrap().can_convert);
        for mode in ["disabled", "shadow", "enabled"] {
            conn.execute("UPDATE market_finance_control SET mode=?1,cutover_approved_at=NULL,new_orders_paused=1,new_topups_paused=1",params![mode]).unwrap();
            assert!(capacity_tx(&conn, "provider", AT).unwrap().can_convert);
        }
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM market_payout_destinations"),
            0
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM market_payment_channels"),
            0
        );
        conn.execute("UPDATE market_provider_profiles SET status='retired'", [])
            .unwrap();
        assert!(capacity_tx(&conn, "provider", AT).unwrap().can_convert);
        conn.execute(
            "UPDATE users SET email_verified_at=NULL WHERE id='provider'",
            [],
        )
        .unwrap();
        assert!(!capacity_tx(&conn, "provider", AT).unwrap().can_convert);
        conn.execute(
            "UPDATE users SET email_verified_at=?1 WHERE id='provider'",
            params![START],
        )
        .unwrap();
        conn.execute(
            "UPDATE market_finance_control SET new_topups_paused=1,new_withdrawals_paused=1",
            [],
        )
        .unwrap();
        assert!(capacity_tx(&conn, "provider", AT).unwrap().can_convert);
        crate::market_finance::hold_contract_earnings_tx(&conn, &contract, "manual", AT).unwrap();
        assert_eq!(
            capacity_tx(&conn, "provider", AT)
                .unwrap()
                .convertible_units,
            0
        );
        assert!(
            capacity_tx(&conn, "provider", "2026-02-11T00:00:00Z")
                .unwrap()
                .convertible_units
                == 0
        );
    }
    #[test]
    fn automatic_matured_credit_runs_in_shadow_and_disabled_without_a_gateway() {
        for mode in ["shadow", "disabled"] {
            let (conn, _) = fixture(1000, "share");
            conn.execute("UPDATE market_finance_control SET mode=?1,cutover_approved_at=NULL,new_orders_paused=1,new_topups_paused=1",params![mode]).unwrap();
            assert!(credit_due_earnings_isolated(&conn, AT, 100).unwrap() > 0);
            assert!(
                crate::market_wallet::wallet_snapshot_tx(&conn, "provider")
                    .unwrap()
                    .available_units
                    > 0
            );
            assert_eq!(credit_due_earnings_isolated(&conn, AT, 100).unwrap(), 0);
            verify_balances_tx(&conn, "provider").unwrap();
        }
    }
    #[test]
    fn consent_ownership_expiry_and_oversized_or_ambiguous_amounts_are_rejected() {
        let (conn, _) = fixture(1000, "share");
        let q = preview(&conn, Some(100));
        let mut req = input(&q);
        req.acknowledge_non_withdrawable = false;
        assert!(confirm_tx(&conn, "provider", "k", &req, AT).is_err());
        assert!(confirm_tx(&conn, "other", "k", &input(&q), AT).is_err());
        assert!(confirm_tx(&conn, "provider", "k", &input(&q), "2026-02-18T00:05:00Z").is_err());
        for minor in [0, -1, 901, i64::MAX] {
            assert!(
                create_quote_tx(
                    &conn,
                    "provider",
                    &QuoteRequest {
                        amount_minor: Some(minor),
                        all: false,
                        batch: false,
                    },
                    DateTime::parse_from_rfc3339(AT)
                        .unwrap()
                        .with_timezone(&Utc)
                )
                .is_err()
            );
        }
        assert!(serde_json::from_str::<ConfirmRequest>(r#"{"quoteId":"q","termsVersion":1,"acknowledgeNonWithdrawable":true,"userId":"other"}"#).is_err());
        assert!(
            serde_json::from_str::<QuoteRequest>(
                r#"{"amountMinor":100,"recipientUserId":"other"}"#
            )
            .is_err()
        );
    }
    #[test]
    fn notification_failure_rolls_back_every_financial_write() {
        let (conn, _) = fixture(1000, "share");
        let q = preview(&conn, Some(100));
        conn.execute_batch("CREATE TRIGGER reject_conversion_notice BEFORE INSERT ON market_notification_outbox
            WHEN NEW.event_kind='earnings_converted' BEGIN SELECT RAISE(ABORT,'test delivery storage failure'); END;").unwrap();
        assert!(convert(&conn, &q, "retry").is_err());
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_earning_wallet_conversions"
            ),
            0
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_journal_transactions WHERE source_kind='earning_conversion'"
            ),
            0
        );
        assert_eq!(
            crate::market_finance::provider_summary_tx(&conn, "provider")
                .unwrap()
                .converted_units,
            0
        );
        assert_eq!(
            crate::market_wallet::wallet_snapshot_tx(&conn, "provider")
                .unwrap()
                .posted_units,
            0
        );
        conn.execute_batch("DROP TRIGGER reject_conversion_notice;")
            .unwrap();
        convert(&conn, &q, "retry").unwrap();
    }
    #[test]
    fn converted_earnings_cannot_be_reserved_for_withdrawal_or_reversed_again() {
        let (conn, _) = fixture(1000, "share");
        let q = preview(&conn, None);
        convert(&conn, &q, "all").unwrap();
        assert!(
            conn.execute(
                "UPDATE market_provider_earning_lots SET held_units=86400",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE market_provider_earning_lots SET paid_units=86400",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE market_provider_earning_lots SET converted_units=0",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE market_earning_wallet_conversions SET amount_units=1",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute("DELETE FROM market_earning_conversion_allocations", [])
                .is_err()
        );
    }
    #[test]
    fn service_adjustments_keep_conversion_disposed_and_refund_only_the_original_remaining_split() {
        let (conn, contract) = fixture(1000, "share");
        let q = preview(&conn, Some(450));
        convert(&conn, &q, "half").unwrap();
        let dispute = crate::market_service_disputes::open_dispute_tx(
            &conn,
            &contract,
            "buyer",
            "service_quality",
            AT,
        )
        .unwrap();
        assert_eq!(
            capacity_tx(&conn, "provider", AT)
                .unwrap()
                .convertible_units,
            0
        );
        assert!(
            crate::market_service_disputes::resolve_dispute_tx(
                &conn,
                &dispute.id,
                "owner",
                1,
                "reviewed",
                units_from_minor(1000).unwrap(),
                AT
            )
            .is_err()
        );
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        crate::market_service_disputes::resolve_dispute_tx(
            &tx,
            &dispute.id,
            "owner",
            1,
            "reviewed",
            units_from_minor(500).unwrap(),
            AT,
        )
        .unwrap();
        tx.commit().unwrap();
        let summary = crate::market_finance::provider_summary_tx(&conn, "provider").unwrap();
        assert_eq!(summary.converted_units, units_from_minor(450).unwrap());
        assert_eq!(summary.withdrawable_units, 0);
        verify_balances_tx(&conn, "provider").unwrap();
    }
    #[test]
    fn corrupted_projection_is_held_and_cannot_be_dismissed_as_resolved() {
        let (conn, _) = fixture(1000, "share");
        let q = preview(&conn, Some(100));
        convert(&conn, &q, "one").unwrap();
        conn.execute(
            "UPDATE market_provider_earning_lots SET converted_units=converted_units+1",
            [],
        )
        .unwrap();
        crate::market_reconcile::audit_finance_tx(&conn, AT).unwrap();
        assert!(!capacity_tx(&conn, "provider", AT).unwrap().can_convert);
        let case:String=conn.query_row("SELECT id FROM market_finance_reconciliation_cases WHERE code='earning_conversion_allocation_mismatch'",[],|r|r.get(0)).unwrap();
        assert!(
            crate::market_reconcile::resolve_case_tx(&conn, &case, "owner", "checked", AT).is_err()
        );
    }
    #[test]
    fn unbound_conversion_journal_blocks_all_finance_scopes() {
        let (conn, _) = fixture(1000, "share");
        post_journal_tx(
            &conn,
            "earning_conversion",
            "orphan",
            "convert",
            "orphan",
            &[
                Posting {
                    kind: "provider_available",
                    subject_id: "provider",
                    signed_units: 1,
                },
                Posting {
                    kind: "wallet_available",
                    subject_id: "provider",
                    signed_units: -1,
                },
            ],
            AT,
        )
        .unwrap();
        crate::market_reconcile::audit_finance_tx(&conn, AT).unwrap();
        assert!(
            crate::market_reconcile::ensure_finance_scopes_safe_tx(
                &conn,
                &[crate::market_reconcile::FinanceScope::User("other")]
            )
            .is_err()
        );
    }
    #[test]
    fn conversion_history_and_cursor_are_owner_scoped() {
        let (conn, _) = fixture(0, "share");
        for n in 0..6 {
            let q = preview(&conn, Some(1));
            convert(&conn, &q, &format!("key{n}")).unwrap();
        }
        let page = page_tx(&conn, "provider", &PageQuery::default()).unwrap();
        assert_eq!(page.items.len(), 6);
        assert_eq!(
            page_tx(&conn, "other", &PageQuery::default())
                .unwrap()
                .items
                .len(),
            0
        );
        assert!(read_tx(&conn, "other", &page.items[0].id).is_err());
        assert!(
            page_tx(
                &conn,
                "other",
                &PageQuery {
                    cursor: Some(page.items[0].id.clone()),
                    quote_id: None
                }
            )
            .is_err()
        );
        let matched = page_tx(
            &conn,
            "provider",
            &PageQuery {
                cursor: None,
                quote_id: Some(page.items[0].quote_id.clone()),
            },
        )
        .unwrap();
        assert_eq!(matched.items.len(), 1);
    }
    #[test]
    fn converted_credit_can_pay_for_both_markets_and_transfer_by_verified_email() {
        for kind in ["share", "client_host"] {
            let (conn, _) = fixture(1000, kind);
            let q = preview(&conn, None);
            convert(&conn, &q, "all").unwrap();
            crate::market_commission::freeze_quote_tx(&conn, "provider-purchase", "other", AT)
                .unwrap();
            let contract = contracts::prepare_contract_tx(
                &conn,
                &PrepareContract {
                    product_kind: kind,
                    product_ref: "another-product",
                    trial_scope_ref: "provider-buying",
                    quote_id: "provider-purchase",
                    buyer_user_id: "provider",
                    provider_user_id: "other",
                    offer_revision: 1,
                    billing_model: BillingModel::CalendarMonth,
                    price_minor: 100,
                    auto_renew: false,
                    trial_seconds: 0,
                },
                AT,
            )
            .unwrap();
            contracts::activate_contract_tx(&conn, &contract, AT, AT).unwrap();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            crate::market_wallet_transfers::verified_transfer_for_test(
                &tx,
                "provider",
                "other",
                units_from_minor(200).unwrap(),
                AT,
            )
            .unwrap();
            tx.commit().unwrap();
            assert_eq!(
                crate::market_wallet::wallet_snapshot_tx(&conn, "provider")
                    .unwrap()
                    .posted_units,
                units_from_minor(600).unwrap()
            );
            assert_eq!(
                crate::market_wallet::wallet_snapshot_tx(&conn, "other")
                    .unwrap()
                    .posted_units,
                units_from_minor(200).unwrap()
            );
            crate::market_wallet::reconcile_wallet_tx(&conn, "other").unwrap();
            verify_balances_tx(&conn, "provider").unwrap();
            assert_eq!(
                number(
                    &conn,
                    "SELECT COUNT(*) FROM market_verified_wallet_transfer_evidence"
                ),
                1
            );
            assert_eq!(
                number(
                    &conn,
                    "SELECT SUM(commission_units) FROM market_provider_earning_lots"
                ),
                units_from_minor(100).unwrap()
            );
        }
    }
    #[test]
    fn independent_connections_racing_two_disposals_cannot_double_spend() {
        let directory =
            std::env::temp_dir().join(format!("router-conversion-race-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("finance.sqlite");
        let (conn, _) = fixture_on(
            crate::test_support::database_at(&path).unwrap(),
            1000,
            "share",
        );
        let quotes = [preview(&conn, Some(600)), preview(&conn, Some(600))];
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let tasks = (0..2)
            .map(|i| {
                let db = Connection::open(&path).unwrap();
                let ready = barrier.clone();
                let quote = quotes[i].clone();
                std::thread::spawn(move || {
                    ready.wait();
                    let Ok(tx) = db.transaction_with_behavior(TransactionBehavior::Immediate)
                    else {
                        return false;
                    };
                    let result = confirm_tx(
                        &tx,
                        "provider",
                        &format!("racing-conversion-{i}"),
                        &input(&quote),
                        AT,
                    )
                    .map(|_| ());
                    if result.is_ok() {
                        tx.commit().unwrap();
                        true
                    } else {
                        false
                    }
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tasks
                .into_iter()
                .map(|task| task.join().unwrap())
                .filter(|ok| *ok)
                .count(),
            1
        );
        let summary = crate::market_finance::provider_summary_tx(&conn, "provider").unwrap();
        assert!(
            summary.converted_units + summary.withdrawal_held_units <= summary.net_earned_units
        );
        verify_balances_tx(&conn, "provider").unwrap();
        drop(conn);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn mature_boundary_is_exact_and_new_earnings_do_not_expand_an_all_quote() {
        let (conn, _) = fixture(0, "share");
        assert_eq!(
            capacity_tx(&conn, "provider", "2026-02-16T23:59:59+00:00")
                .unwrap()
                .convertible_units,
            0
        );
        assert_eq!(
            capacity_tx(&conn, "provider", "2026-02-17T00:00:00+00:00")
                .unwrap()
                .convertible_units,
            units_from_minor(1000).unwrap()
        );
        let q = preview(&conn, None);
        crate::market_finance::record_metered_delivery_tx(
            &conn,
            "share",
            "another",
            "later",
            "buyer",
            "provider",
            START,
            "2026-01-10T00:00:01+00:00",
            1,
            END,
        )
        .unwrap();
        crate::market_finance::unlock_due_earnings_tx(&conn, AT, 200).unwrap();
        let result = convert(&conn, &q, "frozen-all").unwrap();
        assert_eq!(result.amount_units, units_from_minor(1000).unwrap());
        assert_eq!(
            capacity_tx(&conn, "provider", AT)
                .unwrap()
                .convertible_units,
            1
        );
    }
    #[test]
    fn previews_are_bounded_but_history_pages_beyond_fifty_and_audit_rotates_beyond_two_hundred() {
        let (conn, _) = fixture(0, "share");
        for i in 0..201 {
            let at = (DateTime::parse_from_rfc3339(AT).unwrap() + Duration::hours(i)).to_rfc3339();
            let q = create_quote_tx(
                &conn,
                "provider",
                &QuoteRequest {
                    amount_minor: Some(1),
                    all: false,
                    batch: false,
                },
                DateTime::parse_from_rfc3339(&at)
                    .unwrap()
                    .with_timezone(&Utc),
            )
            .unwrap();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            confirm_tx(&tx, "provider", &format!("page-{i}"), &input(&q), &at).unwrap();
            tx.commit().unwrap();
        }
        let mut seen = std::collections::HashSet::new();
        let mut cursor = None;
        loop {
            let page = page_tx(
                &conn,
                "provider",
                &PageQuery {
                    cursor,
                    quote_id: None,
                },
            )
            .unwrap();
            for item in page.items {
                assert!(seen.insert(item.id));
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen.len(), 201);
        let tail: String = conn
            .query_row(
                "SELECT id FROM market_earning_wallet_conversions ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute_batch("DROP TRIGGER trg_market_earning_conversion_quote_update;")
            .unwrap();
        conn.execute("UPDATE market_earning_conversion_quotes SET amount_units=amount_units+1 WHERE id=(SELECT quote_id FROM market_earning_wallet_conversions WHERE id=?1)",params![tail]).unwrap();
        crate::market_reconcile::audit_finance_tx(&conn, AT).unwrap();
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases WHERE code='earning_conversion_missing_evidence'"
            ),
            0
        );
        crate::market_reconcile::audit_finance_tx(&conn, AT).unwrap();
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases WHERE code='earning_conversion_missing_evidence'"
            ),
            1
        );
        assert!(
            crate::market_reconcile::ensure_finance_scopes_safe_tx(
                &conn,
                &[crate::market_reconcile::FinanceScope::User("provider")]
            )
            .is_err()
        );
        assert!(
            crate::market_reconcile::ensure_finance_scopes_safe_tx(
                &conn,
                &[crate::market_reconcile::FinanceScope::User("other")]
            )
            .is_ok()
        );
    }
    #[test]
    fn quote_rate_limit_and_wallet_overflow_never_commit_financial_changes() {
        let (conn, _) = fixture(0, "share");
        for _ in 0..30 {
            preview(&conn, Some(1));
        }
        assert!(
            create_quote_tx(
                &conn,
                "provider",
                &QuoteRequest {
                    amount_minor: Some(1),
                    all: false,
                    batch: false,
                },
                DateTime::parse_from_rfc3339(AT)
                    .unwrap()
                    .with_timezone(&Utc)
            )
            .is_err()
        );
        let later = DateTime::parse_from_rfc3339(AT).unwrap() + Duration::hours(2);
        let q = create_quote_tx(
            &conn,
            "provider",
            &QuoteRequest {
                amount_minor: Some(1),
                all: false,
                batch: false,
            },
            later.with_timezone(&Utc),
        )
        .unwrap();
        crate::market_wallet::ensure_wallet_tx(&conn, "provider", AT).unwrap();
        post_journal_tx(
            &conn,
            "test",
            "limit",
            "credit",
            "limit",
            &[
                Posting {
                    kind: "owner_clearing",
                    subject_id: "owner",
                    signed_units: i64::MAX,
                },
                Posting {
                    kind: "wallet_available",
                    subject_id: "provider",
                    signed_units: -i64::MAX,
                },
            ],
            AT,
        )
        .unwrap();
        conn.execute(
            "UPDATE market_wallet_accounts SET posted_units=?1 WHERE user_id='provider'",
            params![i64::MAX],
        )
        .unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(confirm_tx(&tx, "provider", "overflow", &input(&q), &later.to_rfc3339()).is_err());
        drop(tx);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM market_earning_wallet_conversions"
            ),
            0
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COALESCE(SUM(converted_units),0) FROM market_provider_earning_lots"
            ),
            0
        );
    }
}
