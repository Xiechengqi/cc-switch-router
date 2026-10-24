//! C2C sells existing wallet credit. External payment is a party's assertion,
//! never gateway evidence. Only the reserved credit can be delivered.
use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;
use crate::market_wallet::{
    Posting, post_journal_tx, serialize_units, units_from_minor, wallet_snapshot_tx,
};
use crate::market_wallet_policy::db_error;

mod http;
pub use http::router;
#[cfg(test)]
mod tests;

const ACTIVE: &str = "('awaiting_payment','paid','disputed')";
const TERMS_VERSION: i64 = 1;

fn problem(code: &'static str, message: &str) -> AppError {
    AppError::Coded {
        status: StatusCode::CONFLICT,
        code,
        message: message.into(),
        details: serde_json::json!({}),
    }
}
fn at(time: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(time)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| AppError::Internal("invalid C2C time".into()))
}
fn text(value: &str, max: usize) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > max
        || value.chars().any(|c| c.is_control() && c != '\n')
    {
        return Err(AppError::BadRequest("invalid C2C text field".into()));
    }
    Ok(value.into())
}
fn verified_tx(conn: &Connection, user: &str) -> Result<String, AppError> {
    conn.query_row("SELECT email_normalized FROM users WHERE id=?1 AND status='active' AND email_verified_at IS NOT NULL",params![user],|r|r.get(0))
        .optional().map_err(db_error("read C2C identity"))?
        .ok_or_else(||problem("C2C_IDENTITY","an active account with a verified email is required"))
}
fn safe_tx(conn: &Connection, seller: &str, buyer: &str) -> Result<(), AppError> {
    use crate::market_reconcile::{FinanceScope, ensure_finance_scopes_safe_tx};
    ensure_finance_scopes_safe_tx(
        conn,
        &[FinanceScope::User(seller), FinanceScope::User(buyer)],
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Policy {
    version: i64,
    enabled: bool,
    admission_minor: i64,
    min_minor: i64,
    max_minor: i64,
    payment_minutes: i64,
}
fn policy_tx(conn: &Connection) -> Result<Policy, AppError> {
    conn.query_row("SELECT version,enabled,admission_minor,min_minor,max_minor,payment_minutes FROM market_c2c_policy WHERE id=1",[],|r|Ok(Policy {
        version:r.get(0)?,enabled:r.get(1)?,admission_minor:r.get(2)?,min_minor:r.get(3)?,max_minor:r.get(4)?,payment_minutes:r.get(5)?
    })).map_err(db_error("read C2C policy"))
}
fn open_tx(conn: &Connection) -> Result<Policy, AppError> {
    if let Some(code) = opening_blockers_tx(conn)?.first() {
        return Err(problem(code, "new C2C orders are not available"));
    }
    policy_tx(conn)
}
/// C2C transfers existing credit: official top-ups and market purchase pauses
/// cannot disable it. Ordinary transfers and C2C share the outflow switch.
fn opening_blockers_tx(conn: &Connection) -> Result<Vec<&'static str>, AppError> {
    let p = policy_tx(conn)?;
    let w = crate::market_wallet_policy::policy_tx(conn)?;
    let mut blockers = Vec::new();
    if !p.enabled {
        blockers.push("C2C_DISABLED");
    }
    if !w.transfers_enabled {
        blockers.push("C2C_TRANSFERS_DISABLED");
    }
    if w.transfers_paused {
        blockers.push("C2C_TRANSFERS_PAUSED");
    }
    Ok(blockers)
}
fn participation_blockers_tx(conn: &Connection, user: &str) -> Result<Vec<&'static str>, AppError> {
    let mut blockers = opening_blockers_tx(conn)?;
    let identity: Option<(bool, bool)> = conn
        .query_row(
            "SELECT status = 'active', email_verified_at IS NOT NULL FROM users WHERE id=?1",
            params![user],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error("read C2C account eligibility"))?;
    match identity {
        Some((false, _)) => blockers.push("C2C_ACCOUNT_INACTIVE"),
        Some((true, false)) | None => blockers.push("C2C_IDENTITY"),
        Some((true, true)) => {}
    }
    match safe_tx(conn, user, user) {
        Ok(()) => {}
        Err(e) if crate::market_reconcile::is_finance_hold(&e) => blockers.push("C2C_FINANCE_HOLD"),
        Err(e) => return Err(e),
    }
    Ok(blockers)
}
fn seller_blockers_tx(
    conn: &Connection,
    user: &str,
    time: &str,
) -> Result<Vec<&'static str>, AppError> {
    let mut blockers = participation_blockers_tx(conn, user)?;
    let prior = seller_tx(conn, user)?;
    if prior.as_ref().is_some_and(|s| s.suspended) {
        blockers.push("C2C_SUSPENDED");
    }
    if prior.as_ref().is_none_or(|s| !s.enabled)
        && admission_balance_tx(conn, user, time)?
            < units_from_minor(policy_tx(conn)?.admission_minor)?
    {
        blockers.push("C2C_ADMISSION");
    }
    Ok(blockers)
}
fn save_policy_tx(
    conn: &Connection,
    actor: &str,
    input: &Policy,
    time: &str,
) -> Result<Policy, AppError> {
    if !(100..=10_000_000).contains(&input.admission_minor)
        || input.min_minor < 1
        || input.max_minor < input.min_minor
        || input.max_minor > 10_000_000
        || !(5..=60).contains(&input.payment_minutes)
    {
        return Err(AppError::BadRequest("invalid C2C policy limits".into()));
    }
    let before = policy_tx(conn)?;
    let changed=conn.execute("UPDATE market_c2c_policy SET version=version+1,enabled=?2,admission_minor=?3,min_minor=?4,max_minor=?5,payment_minutes=?6,updated_at=?7 WHERE id=1 AND version=?1",
        params![input.version,input.enabled,input.admission_minor,input.min_minor,input.max_minor,input.payment_minutes,time]).map_err(db_error("save C2C policy"))?;
    if changed != 1 {
        return Err(problem("C2C_STALE", "C2C policy version changed; reload"));
    }
    audit_tx(
        conn,
        actor,
        "policy",
        "c2c_policy",
        "1",
        &serde_json::to_string(&before).unwrap_or_default(),
        time,
    )?;
    policy_tx(conn)
}
fn audit_tx(
    conn: &Connection,
    actor: &str,
    action: &str,
    kind: &str,
    id: &str,
    reason: &str,
    time: &str,
) -> Result<(), AppError> {
    conn.execute("INSERT INTO market_finance_audit_events(id,actor_user_id,action,object_kind,object_id,reason_code,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![Uuid::new_v4().to_string(),actor,action,kind,id,reason,time]).map_err(db_error("audit C2C action"))?;
    Ok(())
}

/// Includes unresolved C2C reservations across midnight, not just completed
/// sales. Both ordinary transfers and C2C consume the same per-day budget.
pub(crate) fn committed_transfer_units_tx(
    conn: &Connection,
    user: &str,
    time: &str,
) -> Result<i64, AppError> {
    let day = at(time)?.date_naive().to_string();
    conn.query_row(
        "SELECT COALESCE(SUM(amount_units),0) FROM market_c2c_orders
        WHERE seller_id=?1 AND (status IN ('awaiting_payment','paid','disputed')
         OR (status='released' AND date(released_at)>=?2))",
        params![user, day],
        |r| r.get(0),
    )
    .map_err(db_error("sum C2C transfer commitments"))
}
fn stock_tx(conn: &Connection, user: &str, time: &str) -> Result<i64, AppError> {
    let w = crate::market_wallet_policy::policy_tx(conn)?;
    let day = at(time)?.date_naive().to_string();
    let sent:i64=conn.query_row("SELECT COALESCE(SUM(amount_units),0) FROM market_wallet_transfers WHERE sender_user_id=?1 AND date(created_at)>=?2",params![user,day],|r|r.get(0)).map_err(db_error("sum ordinary transfers"))?;
    let committed =
        crate::market_platform_contracts::prior_due_funding_units_tx(conn, user, None, time)?;
    let daily = units_from_minor(w.transfer_daily_max_minor)?
        .saturating_sub(sent)
        .saturating_sub(committed_transfer_units_tx(conn, user, time)?)
        .max(0);
    Ok(wallet_snapshot_tx(conn, user)?
        .available_units
        .saturating_sub(committed)
        .max(0)
        .min(daily)
        .min(units_from_minor(w.transfer_max_minor)?))
}
fn admission_balance_tx(conn: &Connection, user: &str, time: &str) -> Result<i64, AppError> {
    let committed =
        crate::market_platform_contracts::prior_due_funding_units_tx(conn, user, None, time)?;
    Ok(wallet_snapshot_tx(conn, user)?
        .available_units
        .saturating_sub(committed)
        .max(0))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Seller {
    user_id: String,
    version: i64,
    enabled: bool,
    suspended: bool,
    display_name: String,
    description: String,
    currency: String,
    price_bps: i64,
    min_minor: i64,
    max_minor: i64,
    heartbeat_at: String,
}
fn seller_tx(conn: &Connection, id: &str) -> Result<Option<Seller>, AppError> {
    conn.query_row("SELECT user_id,version,enabled,suspended,display_name,description,currency,price_bps,min_minor,max_minor,heartbeat_at FROM market_c2c_sellers WHERE user_id=?1",params![id],|r|Ok(Seller {
        user_id:r.get(0)?,version:r.get(1)?,enabled:r.get(2)?,suspended:r.get(3)?,display_name:r.get(4)?,description:r.get(5)?,currency:r.get(6)?,price_bps:r.get(7)?,min_minor:r.get(8)?,max_minor:r.get(9)?,heartbeat_at:r.get(10)?
    })).optional().map_err(db_error("read C2C seller"))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SellerInput {
    expected_version: i64,
    enabled: bool,
    description: String,
    currency: String,
    price_bps: i64,
    min_minor: i64,
    max_minor: i64,
    acknowledge_external_payment: bool,
}
fn save_seller_tx(
    conn: &Connection,
    user: &str,
    input: &SellerInput,
    time: &str,
) -> Result<Seller, AppError> {
    if !input.enabled {
        verified_tx(conn, user)?;
    }
    let prior = seller_tx(conn, user)?;
    if prior.as_ref().map_or(0, |s| s.version) != input.expected_version {
        return Err(problem("C2C_STALE", "seller settings changed; reload"));
    }
    if !input.acknowledge_external_payment
        || !matches!(input.currency.as_str(), "USD" | "CNY")
        || !(5000..=20000).contains(&input.price_bps)
        || input.min_minor < 1
        || input.max_minor < input.min_minor
        || input.max_minor > 10_000_000
    {
        return Err(AppError::BadRequest("invalid C2C seller terms".into()));
    }
    if input
        .description
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(AppError::BadRequest("invalid C2C description".into()));
    }
    let description = text(
        &input
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        300,
    )?;
    let name = prior
        .as_ref()
        .map(|s| s.display_name.clone())
        .unwrap_or_else(|| format!("C2C-{}", &Uuid::new_v4().simple().to_string()[..12]));
    if input.enabled {
        if let Some(code) = seller_blockers_tx(conn, user, time)?.first() {
            return Err(problem(
                code,
                "C2C seller eligibility requirements have not been met",
            ));
        }
    }
    conn.execute("INSERT INTO market_c2c_sellers(user_id,version,enabled,display_name,description,currency,price_bps,min_minor,max_minor,heartbeat_at,created_at,updated_at)
        VALUES(?1,1,?2,?3,?4,?5,?6,?7,?8,?9,?9,?9)
        ON CONFLICT(user_id) DO UPDATE SET version=version+1,enabled=excluded.enabled,description=excluded.description,
         currency=excluded.currency,price_bps=excluded.price_bps,
         min_minor=excluded.min_minor,max_minor=excluded.max_minor,heartbeat_at=excluded.heartbeat_at,updated_at=excluded.updated_at",
        params![user,input.enabled,name,description,input.currency,input.price_bps,input.min_minor,input.max_minor,time]).map_err(db_error("save C2C seller"))?;
    audit_tx(
        conn,
        user,
        "configure",
        "c2c_seller",
        user,
        "seller_description_v2",
        time,
    )?;
    crate::market_notifications::enqueue_user_notification_tx(
        conn,
        "buyer",
        user,
        "c2c_seller_changed",
        "c2c_seller",
        user,
        &format!("c2c-seller:{user}:{}", input.expected_version + 1),
        serde_json::json!({}),
        time,
    )?;
    seller_tx(conn, user)?.ok_or_else(|| AppError::Internal("C2C seller disappeared".into()))
}
// Stopping new orders must work even when admission, finance, email, identity
// verification or publication validation would prevent starting new orders.
fn pause_seller_tx(
    conn: &Connection,
    user: &str,
    expected_version: i64,
    time: &str,
) -> Result<Seller, AppError> {
    let prior =
        seller_tx(conn, user)?.ok_or_else(|| AppError::NotFound("C2C seller not found".into()))?;
    if prior.version != expected_version {
        return Err(problem("C2C_STALE", "seller settings changed; reload"));
    }
    if prior.enabled {
        conn.execute("UPDATE market_c2c_sellers SET enabled=0,version=version+1,updated_at=?2 WHERE user_id=?1 AND version=?3",
            params![user,time,expected_version]).map_err(db_error("pause C2C seller"))?;
        audit_tx(
            conn,
            user,
            "pause",
            "c2c_seller",
            user,
            "seller_paused",
            time,
        )?;
    }
    seller_tx(conn, user)?.ok_or_else(|| AppError::Internal("C2C seller disappeared".into()))
}
fn require_seller_tx(conn: &Connection, user: &str, time: &str) -> Result<Seller, AppError> {
    verified_tx(conn, user)?;
    let s = seller_tx(conn, user)?
        .ok_or_else(|| problem("C2C_SELLER_UNAVAILABLE", "seller is not available"))?;
    if !s.enabled
        || s.suspended
        || s.description.is_empty()
        || at(&s.heartbeat_at)? + Duration::minutes(5) < at(time)?
    {
        return Err(problem(
            "C2C_SELLER_UNAVAILABLE",
            "seller is offline or not accepting orders",
        ));
    }
    safe_tx(conn, user, user)?;
    Ok(s)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Offer {
    seller_id: String,
    version: i64,
    display_name: String,
    description: String,
    currency: String,
    price_bps: i64,
    min_minor: i64,
    max_minor: i64,
    available_minor: i64,
    completed_orders: i64,
}
fn offers_tx(
    conn: &Connection,
    user: &str,
    after: &str,
    time: &str,
) -> Result<(Vec<Offer>, Option<String>), AppError> {
    if !opening_blockers_tx(conn)?.is_empty() {
        return Ok((vec![], None));
    }
    let p = policy_tx(conn)?;
    let experienced:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_c2c_orders WHERE buyer_id=?1 AND status='released')",params![user],|r|r.get(0)).map_err(db_error("read C2C discovery purchase limit"))?;
    let buyer_max = if experienced {
        p.max_minor
    } else {
        p.max_minor.min(10000)
    };
    let mut stmt=conn.prepare("SELECT user_id FROM market_c2c_sellers WHERE enabled=1 AND suspended=0 AND user_id!=?1 AND user_id>?3 AND julianday(heartbeat_at)>=julianday(?2)-5.0/1440 ORDER BY user_id LIMIT 101").map_err(db_error("list C2C sellers"))?;
    let mut ids = stmt
        .query_map(params![user, time, after], |r| r.get::<_, String>(0))
        .map_err(db_error("query C2C sellers"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read C2C sellers"))?;
    let more = ids.len() > 100;
    ids.truncate(100);
    let next = if more { ids.last().cloned() } else { None };
    let mut offers = Vec::new();
    for id in ids {
        let s = match require_seller_tx(conn, &id, time) {
            Ok(s) => s,
            Err(AppError::Coded { .. }) => continue,
            Err(e) => return Err(e),
        };
        let available = stock_tx(conn, &id, time)? / 86400;
        let min = s.min_minor.max(p.min_minor);
        let max = s.max_minor.min(buyer_max).min(available);
        let active:i64=conn.query_row(&format!("SELECT COUNT(*) FROM market_c2c_orders WHERE seller_id=?1 AND status IN {ACTIVE}"),params![id],|r|r.get(0)).map_err(db_error("count C2C seller orders"))?;
        if max < min || active >= 10 {
            continue;
        }
        let completed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_c2c_orders WHERE seller_id=?1 AND status='released'",
                params![id],
                |r| r.get(0),
            )
            .map_err(db_error("read C2C completed orders"))?;
        offers.push(Offer {
            seller_id: id,
            version: s.version,
            display_name: s.display_name,
            description: s.description,
            currency: s.currency,
            price_bps: s.price_bps,
            min_minor: min,
            max_minor: max,
            available_minor: available,
            completed_orders: completed,
        });
    }
    // Self-reported offline completions can be gamed. Never turn their volume
    // into a trust rank; show the quoted rate first within each cursor page.
    offers.sort_by(|a, b| {
        a.price_bps
            .cmp(&b.price_bps)
            .then(a.seller_id.cmp(&b.seller_id))
    });
    Ok((offers, next))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Quote {
    id: String,
    seller_id: String,
    seller_name: String,
    seller_version: i64,
    credit_minor: i64,
    pay_minor: i64,
    currency: String,
    fx_micros: i64,
    price_bps: i64,
    description: String,
    expires_at: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuoteInput {
    seller_id: String,
    amount_minor: i64,
}
fn quote_tx(
    conn: &Connection,
    buyer: &str,
    input: &QuoteInput,
    time: &str,
) -> Result<Quote, AppError> {
    verified_tx(conn, buyer)?;
    let p = open_tx(conn)?;
    if buyer == input.seller_id {
        return Err(AppError::BadRequest("cannot buy from yourself".into()));
    }
    let s = require_seller_tx(conn, &input.seller_id, time)?;
    safe_tx(conn, &s.user_id, buyer)?;
    let recent:i64=conn.query_row("SELECT COUNT(*) FROM market_c2c_quotes WHERE buyer_id=?1 AND julianday(created_at)>=julianday(?2)-1.0/24",params![buyer,time],|r|r.get(0)).map_err(db_error("limit C2C quotes"))?;
    if recent >= 30 {
        return Err(AppError::TooManyRequests(
            "too many C2C previews; try later".into(),
        ));
    }
    let max = s
        .max_minor
        .min(p.max_minor)
        .min(stock_tx(conn, &s.user_id, time)? / 86400);
    if input.amount_minor < s.min_minor.max(p.min_minor) || input.amount_minor > max {
        return Err(problem(
            "C2C_STOCK",
            "amount is outside the seller's available range",
        ));
    }
    let rate = if s.currency == "CNY" {
        let fx = crate::market_fx::load_snapshot(conn, 0)?;
        if !fx.is_fresh_at(at(time)?.timestamp()) {
            return Err(problem(
                "C2C_FX_UNAVAILABLE",
                "a verified current CNY rate is required",
            ));
        }
        fx.rate_micros
    } else {
        1_000_000
    };
    let pay = (i128::from(input.amount_minor) * i128::from(s.price_bps) * i128::from(rate)
        + 5_000_000_000_i128)
        / 10_000_000_000_i128;
    let pay = i64::try_from(pay).map_err(|_| AppError::BadRequest("C2C amount overflow".into()))?;
    if pay <= 0 {
        return Err(AppError::BadRequest(
            "C2C payable amount is too small".into(),
        ));
    }
    let q = Quote {
        id: Uuid::new_v4().to_string(),
        seller_id: s.user_id.clone(),
        seller_name: s.display_name.clone(),
        seller_version: s.version,
        credit_minor: input.amount_minor,
        pay_minor: pay,
        currency: s.currency.clone(),
        fx_micros: rate,
        price_bps: s.price_bps,
        description: s.description.clone(),
        expires_at: (at(time)? + Duration::minutes(5)).to_rfc3339(),
    };
    conn.execute("INSERT INTO market_c2c_quotes(id,buyer_id,seller_id,seller_version,policy_version,credit_minor,pay_minor,currency,fx_micros,price_bps,seller_name,description,contact,payment_method,payment_details,payment_minutes,expires_at,created_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'','','',?13,?14,?15)",
        params![q.id,buyer,q.seller_id,s.version,p.version,q.credit_minor,q.pay_minor,q.currency,rate,s.price_bps,s.display_name,s.description,p.payment_minutes,q.expires_at,time]).map_err(db_error("create C2C preview"))?;
    Ok(q)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Order {
    id: String,
    quote_id: String,
    buyer_id: String,
    seller_id: String,
    #[serde(serialize_with = "serialize_units")]
    amount_units: i64,
    #[serde(skip_serializing)]
    hold_id: String,
    status: String,
    version: i64,
    payment_deadline: String,
    paid_at: Option<String>,
    released_at: Option<String>,
    late_payment_reported: bool,
    late_payment_resolved: bool,
    created_at: String,
    updated_at: String,
    seller_name: String,
    pay_minor: i64,
    currency: String,
    fx_micros: i64,
    price_bps: i64,
    description: Option<String>,
    legacy_payment: Option<LegacyPayment>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPayment {
    contact: String,
    payment_method: String,
    payment_details: String,
}
fn order_tx(conn: &Connection, id: &str) -> Result<Order, AppError> {
    conn.query_row("SELECT o.id,o.quote_id,o.buyer_id,o.seller_id,o.amount_units,o.hold_id,o.status,o.version,o.payment_deadline,o.paid_at,o.released_at,o.late_payment_reported,o.late_payment_resolved,o.created_at,o.updated_at,
       q.seller_name,q.pay_minor,q.currency,q.fx_micros,q.price_bps,q.contact,q.payment_method,q.payment_details,q.description
       FROM market_c2c_orders o JOIN market_c2c_quotes q ON q.id=o.quote_id WHERE o.id=?1",params![id],|r| {
        let description: String = r.get(23)?;
        let legacy_payment = if description.is_empty() { Some(LegacyPayment { contact:r.get(20)?,payment_method:r.get(21)?,payment_details:r.get(22)? }) } else { None };
        Ok(Order {
        id:r.get(0)?,quote_id:r.get(1)?,buyer_id:r.get(2)?,seller_id:r.get(3)?,amount_units:r.get(4)?,hold_id:r.get(5)?,status:r.get(6)?,version:r.get(7)?,payment_deadline:r.get(8)?,paid_at:r.get(9)?,released_at:r.get(10)?,late_payment_reported:r.get(11)?,late_payment_resolved:r.get(12)?,created_at:r.get(13)?,updated_at:r.get(14)?,seller_name:r.get(15)?,pay_minor:r.get(16)?,currency:r.get(17)?,fx_micros:r.get(18)?,price_bps:r.get(19)?,description:(!description.is_empty()).then_some(description),legacy_payment
    })}).optional().map_err(db_error("read C2C order"))?.ok_or_else(||AppError::NotFound("C2C order not found".into()))
}
fn owned_tx(conn: &Connection, id: &str, user: &str) -> Result<Order, AppError> {
    let o = order_tx(conn, id)?;
    if o.buyer_id != user && o.seller_id != user {
        return Err(AppError::NotFound("C2C order not found".into()));
    }
    Ok(o)
}
fn event_tx(
    conn: &Connection,
    o: &Order,
    actor: &str,
    action: &str,
    note: &str,
    time: &str,
) -> Result<(), AppError> {
    conn.execute("INSERT INTO market_c2c_events(id,order_id,actor_id,action,note,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![Uuid::new_v4().to_string(),o.id,actor,action,note,time]).map_err(db_error("record C2C event"))?;
    for user in [&o.buyer_id, &o.seller_id] {
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "buyer",
            user,
            "c2c_order_updated",
            "c2c_order",
            &o.id,
            &format!("c2c:{}:{}:{action}:{user}", o.id, o.version),
            serde_json::json!({"orderId":o.id,"amountUnits":o.amount_units.to_string(),"status":o.status}),
            time,
        )?;
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateInput {
    quote_id: String,
    terms_version: i64,
    acknowledge_external_payment: bool,
}
fn create_tx(
    conn: &Connection,
    buyer: &str,
    input: &CreateInput,
    key: &str,
    ip: &str,
    time: &str,
) -> Result<Order, AppError> {
    if input.terms_version != TERMS_VERSION || !input.acknowledge_external_payment {
        return Err(problem("C2C_TERMS", "accept the external payment terms"));
    }
    if key.len() < 8
        || key.len() > 128
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(AppError::BadRequest("invalid idempotency key".into()));
    }
    let prior:Option<(String,String)>=conn.query_row("SELECT id,quote_id FROM market_c2c_orders WHERE buyer_id=?1 AND (idempotency_key=?2 OR quote_id=?3) ORDER BY CASE WHEN idempotency_key=?2 THEN 0 ELSE 1 END LIMIT 1",params![buyer,key,input.quote_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db_error("recover C2C order"))?;
    if let Some((id, q)) = prior {
        if q != input.quote_id {
            return Err(problem("C2C_IDEMPOTENCY", "the request changed"));
        }
        return order_tx(conn, &id);
    }
    verified_tx(conn, buyer)?;
    let p = open_tx(conn)?;
    let (seller,seller_version,policy_version,minor,minutes,expires):(String,i64,i64,i64,i64,String)=conn.query_row("SELECT seller_id,seller_version,policy_version,credit_minor,payment_minutes,expires_at FROM market_c2c_quotes WHERE id=?1 AND buyer_id=?2",params![input.quote_id,buyer],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(db_error("read own C2C preview"))?.ok_or_else(||problem("C2C_STALE","preview expired or is unavailable; review again"))?;
    let s = require_seller_tx(conn, &seller, time)?;
    if seller == buyer
        || seller_version != s.version
        || policy_version != p.version
        || at(&expires)? <= at(time)?
    {
        return Err(problem(
            "C2C_STALE",
            "preview expired or terms changed; review again",
        ));
    }
    safe_tx(conn, &seller, buyer)?;
    let amount = units_from_minor(minor)?;
    if amount > stock_tx(conn, &seller, time)? {
        return Err(problem("C2C_STOCK", "seller balance changed; review again"));
    }
    let (buyer_active, seller_active, ip_active): (i64, i64, i64) = conn
        .query_row(
            &format!(
                "SELECT
        (SELECT COUNT(*) FROM market_c2c_orders WHERE buyer_id=?1 AND status IN {ACTIVE}),
        (SELECT COUNT(*) FROM market_c2c_orders WHERE seller_id=?2 AND status IN {ACTIVE}),
        (SELECT COUNT(*) FROM market_c2c_orders WHERE created_ip=?3 AND status IN {ACTIVE})"
            ),
            params![buyer, seller, ip],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(db_error("bound C2C active orders"))?;
    if buyer_active >= 2 || seller_active >= 10 || ip_active >= 6 {
        return Err(problem("C2C_ORDER_LIMIT", "too many unfinished C2C orders"));
    }
    let experienced:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_c2c_orders WHERE buyer_id=?1 AND status='released')",params![buyer],|r|r.get(0)).map_err(db_error("read C2C buyer limits"))?;
    if !experienced && minor > 10000 {
        return Err(problem(
            "C2C_FIRST_ORDER_LIMIT",
            "the first C2C purchase is limited to US$100",
        ));
    }
    let recent:i64=conn.query_row("SELECT COUNT(*) FROM market_c2c_attempts WHERE (user_id=?1 OR ip=?2) AND julianday(created_at)>=julianday(?3)-1.0/24",params![buyer,ip,time],|r|r.get(0)).map_err(db_error("rate limit C2C orders"))?;
    if recent >= 10 {
        return Err(AppError::TooManyRequests(
            "too many C2C purchase attempts; try later".into(),
        ));
    }
    conn.execute(
        "INSERT INTO market_c2c_attempts(id,user_id,ip,created_at) VALUES(?1,?2,?3,?4)",
        params![Uuid::new_v4().to_string(), buyer, ip, time],
    )
    .map_err(db_error("record C2C order attempt"))?;
    crate::market_wallet::reconcile_wallet_tx(conn, &seller)?;
    crate::market_wallet::reconcile_wallet_tx(conn, buyer)?;
    let id = Uuid::new_v4().to_string();
    let deadline = (at(time)? + Duration::minutes(minutes)).to_rfc3339();
    let hold = crate::market_wallet::reserve_wallet_tx(
        conn,
        &seller,
        "c2c_order",
        &id,
        "c2c",
        amount,
        None,
        time,
    )?;
    conn.execute("INSERT INTO market_c2c_orders(id,quote_id,buyer_id,seller_id,idempotency_key,amount_units,hold_id,status,terms_version,payment_deadline,created_ip,created_at,updated_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,'awaiting_payment',1,?8,?9,?10,?10)",params![id,input.quote_id,buyer,seller,key,amount,hold,deadline,ip,time]).map_err(db_error("create C2C order"))?;
    let o = order_tx(conn, &id)?;
    event_tx(conn, &o, buyer, "created", "terms_v1", time)?;
    Ok(o)
}

fn change_tx(conn: &Connection, o: &Order, status: &str, time: &str) -> Result<Order, AppError> {
    let changed=conn.execute("UPDATE market_c2c_orders SET status=?3,version=version+1,updated_at=?4 WHERE id=?1 AND version=?2",params![o.id,o.version,status,time]).map_err(db_error("transition C2C order"))?;
    if changed != 1 {
        return Err(problem("C2C_STALE", "order version changed; reload"));
    }
    order_tx(conn, &o.id)
}
fn cancel_tx(
    conn: &Connection,
    o: &Order,
    actor: &str,
    expired: bool,
    time: &str,
) -> Result<Order, AppError> {
    if matches!(o.status.as_str(), "cancelled" | "expired") {
        return Ok(o.clone());
    }
    if o.status != "awaiting_payment" || (!expired && actor != o.buyer_id) {
        return Err(problem(
            "C2C_PAYMENT_HOLD",
            "a paid or disputed order cannot be cancelled without a verified resolution",
        ));
    }
    if expired && at(time)? < at(&o.payment_deadline)? + Duration::minutes(2) {
        return Err(problem("C2C_STALE", "payment deadline has not elapsed"));
    }
    if !evidence_valid_tx(conn, &o.id)? {
        return Err(problem(
            "MARKET_FINANCE_HOLD",
            "C2C reservation evidence needs review",
        ));
    }
    crate::market_wallet::reconcile_wallet_tx(conn, &o.seller_id)?;
    crate::market_wallet::release_hold_tx(conn, &o.hold_id, time)?;
    crate::market_wallet::reconcile_wallet_tx(conn, &o.seller_id)?;
    let o = change_tx(conn, o, if expired { "expired" } else { "cancelled" }, time)?;
    event_tx(
        conn,
        &o,
        actor,
        if expired { "expired" } else { "cancelled" },
        "no_payment_declared",
        time,
    )?;
    Ok(o)
}
fn mark_paid_tx(
    conn: &Connection,
    o: &Order,
    buyer: &str,
    version: i64,
    time: &str,
) -> Result<Order, AppError> {
    if o.buyer_id != buyer {
        return Err(AppError::Forbidden(
            "only the buyer can declare payment".into(),
        ));
    }
    if matches!(o.status.as_str(), "paid" | "disputed" | "released") {
        return Ok(o.clone());
    }
    if o.version != version
        || o.status != "awaiting_payment"
        || at(time)? >= at(&o.payment_deadline)? + Duration::minutes(2)
    {
        return Err(problem(
            "C2C_PAYMENT_WINDOW",
            "payment window closed; report any late payment instead",
        ));
    }
    if !evidence_valid_tx(conn, &o.id)? {
        // Recording the buyer's assertion moves no money. Preserve it even
        // during reconciliation so a real payment cannot silently expire.
        crate::market_reconcile::case_tx(conn, "c2c_evidence_mismatch", "c2c_order", &o.id, time)?;
    }
    conn.execute(
        "UPDATE market_c2c_orders SET paid_at=?2 WHERE id=?1",
        params![o.id, time],
    )
    .map_err(db_error("record C2C payment assertion"))?;
    let o = change_tx(conn, o, "paid", time)?;
    event_tx(conn, &o, buyer, "paid", "buyer_declared_not_verified", time)?;
    Ok(o)
}
fn release_tx(conn: &Connection, o: &Order, actor: &str, time: &str) -> Result<Order, AppError> {
    if o.status == "released" {
        return Ok(o.clone());
    }
    if !matches!(o.status.as_str(), "paid" | "disputed") {
        return Err(problem(
            "C2C_PAYMENT_HOLD",
            "payment must be declared before release",
        ));
    }
    safe_tx(conn, &o.seller_id, &o.buyer_id)?;
    if !evidence_valid_tx(conn, &o.id)? {
        return Err(problem(
            "MARKET_FINANCE_HOLD",
            "C2C reservation evidence needs review",
        ));
    }
    crate::market_wallet::reconcile_wallet_tx(conn, &o.seller_id)?;
    crate::market_wallet::reconcile_wallet_tx(conn, &o.buyer_id)?;
    crate::market_wallet::ensure_wallet_tx(conn, &o.buyer_id, time)?;
    wallet_snapshot_tx(conn, &o.buyer_id)?
        .posted_units
        .checked_add(o.amount_units)
        .ok_or_else(|| AppError::Conflict("recipient balance limit exceeded".into()))?;
    let j = post_journal_tx(
        conn,
        "c2c_order",
        &o.id,
        "release",
        &format!(
            "{}:{}:{}:{}",
            o.seller_id, o.buyer_id, o.amount_units, o.quote_id
        ),
        &[
            Posting {
                kind: "wallet_held",
                subject_id: &o.seller_id,
                signed_units: o.amount_units,
            },
            Posting {
                kind: "wallet_available",
                subject_id: &o.buyer_id,
                signed_units: -o.amount_units,
            },
        ],
        time,
    )?;
    if !j.inserted {
        return Err(AppError::Internal(
            "C2C delivery journal has no completed order".into(),
        ));
    }
    let held=conn.execute("UPDATE market_wallet_holds SET remaining_units=0,status='captured',updated_at=?2 WHERE id=?1 AND status='active' AND remaining_units=?3",params![o.hold_id,time,o.amount_units]).map_err(db_error("capture C2C hold"))?;
    let seller=conn.execute("UPDATE market_wallet_accounts SET posted_units=posted_units-?2,held_units=held_units-?2,version=version+1,updated_at=?3 WHERE user_id=?1 AND held_units>=?2",params![o.seller_id,o.amount_units,time]).map_err(db_error("debit C2C seller"))?;
    let buyer=conn.execute("UPDATE market_wallet_accounts SET posted_units=posted_units+?2,version=version+1,updated_at=?3 WHERE user_id=?1 AND posted_units<=?4",params![o.buyer_id,o.amount_units,time,i64::MAX-o.amount_units]).map_err(db_error("credit C2C buyer"))?;
    let changed=conn.execute("UPDATE market_c2c_orders SET status='released',version=version+1,release_journal_id=?3,released_at=?4,updated_at=?4 WHERE id=?1 AND version=?2",params![o.id,o.version,j.id,time]).map_err(db_error("complete C2C order"))?;
    if held != 1 || seller != 1 || buyer != 1 || changed != 1 {
        return Err(AppError::Conflict(
            "C2C delivery changed concurrently".into(),
        ));
    }
    if !evidence_valid_tx(conn, &o.id)? {
        return Err(AppError::Internal("C2C release evidence mismatch".into()));
    }
    crate::market_wallet::reconcile_wallet_tx(conn, &o.seller_id)?;
    crate::market_wallet::reconcile_wallet_tx(conn, &o.buyer_id)?;
    let o = order_tx(conn, &o.id)?;
    event_tx(
        conn,
        &o,
        actor,
        "released",
        "receipt_confirmed_by_actor",
        time,
    )?;
    audit_tx(
        conn,
        actor,
        "release",
        "c2c_order",
        &o.id,
        "existing_wallet_credit_only",
        time,
    )?;
    crate::market_notifications::owner_signal_tx(
        conn,
        "c2c_dispute",
        "c2c_order",
        &o.id,
        serde_json::json!({}),
        time,
        true,
    )?;
    Ok(o)
}
fn dispute_tx(
    conn: &Connection,
    o: &Order,
    actor: &str,
    note: &str,
    time: &str,
) -> Result<Order, AppError> {
    let note = text(note, 1500)?;
    if o.status == "disputed" {
        return Ok(o.clone());
    }
    if o.status != "paid" {
        return Err(problem(
            "C2C_PAYMENT_HOLD",
            "only a payment-declared order can enter escrow dispute",
        ));
    }
    let o = change_tx(conn, o, "disputed", time)?;
    event_tx(conn, &o, actor, "disputed", &note, time)?;
    crate::market_notifications::owner_signal_tx(
        conn,
        "c2c_dispute",
        "c2c_order",
        &o.id,
        serde_json::json!({"orderId":o.id}),
        time,
        false,
    )?;
    Ok(o)
}
fn late_payment_tx(
    conn: &Connection,
    o: &Order,
    buyer: &str,
    note: &str,
    time: &str,
) -> Result<Order, AppError> {
    if o.buyer_id != buyer || !matches!(o.status.as_str(), "cancelled" | "expired") {
        return Err(problem(
            "C2C_PAYMENT_HOLD",
            "late payment reporting is only for closed purchases",
        ));
    }
    if o.late_payment_reported {
        return Ok(o.clone());
    }
    let note = text(note, 1500)?;
    conn.execute("UPDATE market_c2c_orders SET late_payment_reported=1,version=version+1,updated_at=?2 WHERE id=?1",params![o.id,time]).map_err(db_error("record late C2C payment"))?;
    let o = order_tx(conn, &o.id)?;
    event_tx(conn, &o, buyer, "late_payment_reported", &note, time)?;
    crate::market_notifications::owner_signal_tx(
        conn,
        "c2c_dispute",
        "c2c_order",
        &o.id,
        serde_json::json!({"orderId":o.id,"late":true}),
        time,
        false,
    )?;
    Ok(o)
}
fn resolve_tx(
    conn: &Connection,
    o: &Order,
    actor: &str,
    version: i64,
    action: &str,
    evidence: &str,
    attested: bool,
    time: &str,
) -> Result<Order, AppError> {
    if actor == o.buyer_id || actor == o.seller_id {
        return Err(AppError::Forbidden(
            "a party cannot adjudicate its own C2C order".into(),
        ));
    }
    if version != o.version || !attested {
        return Err(problem(
            "C2C_STALE",
            "verify the evidence and reload the current order",
        ));
    }
    let evidence = text(evidence, 1500)?;
    let result = match (action, o.status.as_str()) {
        ("release", "disputed") => release_tx(conn, o, actor, time)?,
        ("return_to_seller", "disputed") => {
            safe_tx(conn, &o.seller_id, &o.buyer_id)?;
            if !evidence_valid_tx(conn, &o.id)? {
                return Err(problem("MARKET_FINANCE_HOLD", "C2C evidence needs review"));
            }
            crate::market_wallet::reconcile_wallet_tx(conn, &o.seller_id)?;
            crate::market_wallet::release_hold_tx(conn, &o.hold_id, time)?;
            crate::market_wallet::reconcile_wallet_tx(conn, &o.seller_id)?;
            change_tx(conn, o, "cancelled", time)?
        }
        ("record_external_resolution", "expired" | "cancelled")
            if o.late_payment_reported && !o.late_payment_resolved =>
        {
            conn.execute("UPDATE market_c2c_orders SET late_payment_resolved=1,version=version+1,updated_at=?2 WHERE id=?1",params![o.id,time]).map_err(db_error("resolve late C2C payment"))?;
            order_tx(conn, &o.id)?
        }
        _ => return Err(AppError::Conflict("invalid C2C resolution".into())),
    };
    audit_tx(conn, actor, action, "c2c_order", &o.id, &evidence, time)?;
    event_tx(conn, &result, actor, action, &evidence, time)?;
    crate::market_notifications::owner_signal_tx(
        conn,
        "c2c_dispute",
        "c2c_order",
        &o.id,
        serde_json::json!({}),
        time,
        true,
    )?;
    Ok(result)
}

pub(crate) fn evidence_valid_tx(conn: &Connection, id: &str) -> Result<bool, AppError> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM market_c2c_orders o
       JOIN market_c2c_quotes q ON q.id=o.quote_id AND q.seller_id=o.seller_id AND q.buyer_id=o.buyer_id
       JOIN market_wallet_holds h ON h.id=o.hold_id AND h.user_id=o.seller_id
        AND h.source_kind='c2c_order' AND h.source_id=o.id AND h.purpose='c2c' AND h.amount_units=o.amount_units
       WHERE o.id=?1 AND o.amount_units=q.credit_minor*86400 AND o.terms_version=1
        AND (o.status NOT IN ('paid','disputed','released') OR o.paid_at IS NOT NULL)
        AND (o.status!='awaiting_payment' OR o.paid_at IS NULL)
        AND EXISTS(SELECT 1 FROM market_verified_c2c_reservation_evidence r WHERE r.id=o.id) AND (
        (o.status IN ('awaiting_payment','paid','disputed') AND h.status='active' AND h.remaining_units=o.amount_units AND o.release_journal_id IS NULL)
        OR (o.status IN ('cancelled','expired') AND h.status='released' AND h.remaining_units=0 AND o.release_journal_id IS NULL AND EXISTS(SELECT 1 FROM market_verified_c2c_return_evidence e WHERE e.id=o.id))
        OR (o.status='released' AND EXISTS(SELECT 1 FROM market_verified_c2c_release_evidence e WHERE e.id=o.id))))",params![id],|r|r.get(0)).map_err(db_error("verify C2C evidence"))
}
pub(crate) fn orphan_journal_tx(conn: &Connection, id: &str) -> Result<bool, AppError> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM market_journal_transactions j WHERE j.id=?1 AND j.source_kind='c2c_order' AND
       NOT EXISTS(SELECT 1 FROM market_c2c_orders o WHERE o.id=j.source_id AND
        ((j.action='release' AND o.release_journal_id=j.id) OR j.action='reserve:c2c')))",params![id],|r|r.get(0)).map_err(db_error("verify C2C journal ownership"))
}
pub(crate) fn audit_tx_cases(conn: &Connection, time: &str) -> Result<usize, AppError> {
    let cursor: String = conn
        .query_row(
            "SELECT audit_cursor FROM market_c2c_policy WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(db_error("read C2C audit cursor"))?;
    let read = |after: &str| -> Result<Vec<String>, AppError> {
        let mut s = conn
            .prepare("SELECT id FROM market_c2c_orders WHERE id>?1 ORDER BY id LIMIT 200")
            .map_err(db_error("prepare C2C audit"))?;
        s.query_map(params![after], |r| r.get(0))
            .map_err(db_error("query C2C audit"))?
            .collect::<crate::db::Result<Vec<_>>>()
            .map_err(db_error("read C2C audit"))
    };
    let mut ids = read(&cursor)?;
    if ids.is_empty() && !cursor.is_empty() {
        ids = read("")?;
    }
    let mut findings = 0;
    for id in &ids {
        if !evidence_valid_tx(conn, id)? {
            findings += crate::market_reconcile::case_tx(
                conn,
                "c2c_evidence_mismatch",
                "c2c_order",
                id,
                time,
            )?;
        }
    }
    conn.execute(
        "UPDATE market_c2c_policy SET audit_cursor=?1 WHERE id=1",
        params![ids.last().map(String::as_str).unwrap_or("")],
    )
    .map_err(db_error("advance C2C audit"))?;
    let mut s=conn.prepare("SELECT j.id FROM market_journal_transactions j WHERE j.source_kind='c2c_order'
       AND NOT EXISTS(SELECT 1 FROM market_c2c_orders o WHERE o.id=j.source_id AND ((j.action='release' AND o.release_journal_id=j.id) OR j.action='reserve:c2c'))
       AND NOT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases c WHERE c.object_id=j.id AND c.code='c2c_orphan_journal' AND c.status='open') LIMIT 100").map_err(db_error("prepare C2C orphan audit"))?;
    let orphan = s
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(db_error("query C2C orphan audit"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read C2C orphan audit"))?;
    for id in orphan {
        findings +=
            crate::market_reconcile::case_tx(conn, "c2c_orphan_journal", "journal", &id, time)?;
    }
    Ok(findings)
}
pub(crate) fn tick_tx(conn: &Connection, time: &str) -> Result<(), AppError> {
    let cutoff = (at(time)? - Duration::minutes(2)).to_rfc3339();
    let mut stmt=conn.prepare("SELECT id FROM market_c2c_orders o WHERE status='awaiting_payment' AND julianday(payment_deadline)<=julianday(?1)
        AND NOT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases c WHERE c.status='open' AND ((c.object_id=o.id AND c.code='c2c_evidence_mismatch') OR (c.object_id=o.seller_id AND c.code='wallet_projection_mismatch')))
        ORDER BY payment_deadline LIMIT 100").map_err(db_error("prepare C2C expiration"))?;
    let ids = stmt
        .query_map(params![cutoff], |r| r.get::<_, String>(0))
        .map_err(db_error("query C2C expiration"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read C2C expiration"))?;
    for id in ids {
        let o = order_tx(conn, &id)?;
        if !evidence_valid_tx(conn, &id)? {
            crate::market_reconcile::case_tx(
                conn,
                "c2c_evidence_mismatch",
                "c2c_order",
                &id,
                time,
            )?;
        } else if crate::market_wallet::reconcile_wallet_tx(conn, &o.seller_id).is_err() {
            crate::market_reconcile::case_tx(
                conn,
                "wallet_projection_mismatch",
                "wallet",
                &o.seller_id,
                time,
            )?;
        } else {
            cancel_tx(conn, &o, "system", true, time)?;
        }
    }
    let mut stmt=conn.prepare("SELECT id FROM market_c2c_orders WHERE status='paid' AND reminded_at IS NULL AND julianday(paid_at)<=julianday(?1)-30.0/1440 LIMIT 100").map_err(db_error("prepare C2C reminders"))?;
    let ids = stmt
        .query_map(params![time], |r| r.get::<_, String>(0))
        .map_err(db_error("query C2C reminders"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read C2C reminders"))?;
    for id in ids {
        let o = order_tx(conn, &id)?;
        event_tx(
            conn,
            &o,
            "system",
            "release_reminder",
            "no_automatic_release",
            time,
        )?;
        conn.execute(
            "UPDATE market_c2c_orders SET reminded_at=?2 WHERE id=?1",
            params![id, time],
        )
        .map_err(db_error("record C2C reminder"))?;
    }
    conn.execute("DELETE FROM market_c2c_attempts WHERE id IN(SELECT id FROM market_c2c_attempts WHERE julianday(created_at)<julianday(?1)-2 LIMIT 1000)",params![time]).map_err(db_error("prune C2C rate records"))?;
    conn.execute("DELETE FROM market_c2c_challenges WHERE id IN(SELECT id FROM market_c2c_challenges WHERE julianday(expires_at)<julianday(?1)-30 AND consumed_at IS NULL LIMIT 1000)",params![time]).map_err(db_error("prune unused C2C release codes"))?;
    conn.execute("DELETE FROM market_c2c_quotes WHERE id IN(SELECT q.id FROM market_c2c_quotes q WHERE julianday(expires_at)<julianday(?1)-30 AND NOT EXISTS(SELECT 1 FROM market_c2c_orders o WHERE o.quote_id=q.id) LIMIT 1000)",params![time]).map_err(db_error("prune unused C2C quotes"))?;
    Ok(())
}

/// Independent from gateway I/O: a slow or unavailable top-up provider must
/// not delay expiry of an unrelated offline credit reservation.
pub(crate) async fn run_housekeeping(
    store: crate::store::AppStore,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(30));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = timer.tick() => {
                let result = {
                    let conn = store.conn.lock().await;
                    conn.transaction_with_behavior(crate::db::TransactionBehavior::Immediate)
                        .map_err(db_error("begin C2C housekeeping"))
                        .and_then(|tx| {
                            tick_tx(&tx, &crate::market_wallet::now())?;
                            tx.commit().map_err(db_error("commit C2C housekeeping"))
                        })
                };
                if result.is_err() {
                    tracing::warn!("C2C housekeeping deferred; reservations remain protected");
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
        }
    }
}
