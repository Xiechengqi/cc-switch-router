use crate::db::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{post, put};
use axum::{Json, Router};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ServerState;
use crate::error::AppError;
use crate::market_billing::{BillingAction, BillingActionKind, MONEY_UNITS_PER_MINOR};
use crate::models::AuthSession;
use crate::store::AppStore;

pub(crate) const PRICING_FREE: &str = "free";
pub(crate) const PRICING_LEGACY_METERED_DAILY: &str = "legacy_metered_daily";
pub(crate) const PRICING_PREPAID_CALENDAR_MONTH: &str = "prepaid_calendar_month";
pub(crate) const BILLING_INTERVAL_CALENDAR_MONTH: &str = "calendar_month";
pub(crate) const RENEWAL_MANUAL: &str = "manual";
pub(crate) const RENEWAL_AUTOMATIC: &str = "automatic";

const STATUS_PENDING_ACTIVATION: &str = "pending_activation";
const STATUS_TRIAL: &str = "trial";
const STATUS_ACTIVE: &str = "active";
const STATUS_RECOVERY: &str = "recovery";
const STATUS_ENDED: &str = "ended";
const STATUS_ACTIVATION_FAILED: &str = "activation_failed";
const SHARE_RECOVERY_HOURS: i64 = 24;
const CLIENT_RECOVERY_HOURS: i64 = 72;
const MAX_RECONCILE_ADVANCES: usize = 120;
const MAX_RECONCILE_ROWS: i64 = 200;

#[derive(Debug, Clone)]
pub(crate) struct PrepareRecurringContractInput<'a> {
    pub product_kind: &'a str,
    pub product_ref: &'a str,
    pub activation_ref: &'a str,
    pub service_ref: &'a str,
    pub service_label: &'a str,
    pub buyer_user_id: &'a str,
    pub buyer_email: &'a str,
    pub supplier_user_id: &'a str,
    pub supplier_email: &'a str,
    pub currency: &'a str,
    pub cycle_price_minor: i64,
    pub offer_revision: i64,
    pub renewal_policy: &'a str,
    pub auto_renew_max_price_minor: Option<i64>,
    pub renewal_priority: i64,
    pub trial_allowance_seconds: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecurringFundingSummaryView {
    pub supplier_user_id: String,
    pub supplier_email: String,
    pub currency: String,
    pub pricing_model: String,
    pub billing_interval: String,
    pub cycle_price_minor: i64,
    pub renewal_policy: String,
    pub prepaid_account_id: Option<String>,
    pub prepaid_balance_minor: i64,
    pub prepaid_held_minor: i64,
    pub prepaid_available_minor: i64,
    pub initial_hold_minor: i64,
    pub renewal_hold_minor: i64,
    pub total_required_hold_minor: i64,
    pub required_topup_minor: i64,
    pub topup_available: bool,
    pub topup_unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecurringContractView {
    pub id: String,
    pub product_kind: String,
    pub product_ref: String,
    pub pricing_model: String,
    pub billing_interval: String,
    pub cycle_price_minor: i64,
    pub currency: String,
    pub supplier_user_id: String,
    pub supplier_email: String,
    pub status: String,
    pub renewal_policy: String,
    pub renewal_status: String,
    pub auto_renew_max_price_minor: Option<i64>,
    pub renewal_priority: i64,
    pub current_period_start: Option<String>,
    pub current_period_end: Option<String>,
    pub next_renewal_at: Option<String>,
    pub next_period_held_minor: i64,
    pub cancel_at_period_end: bool,
    pub recovery_deadline: Option<String>,
    pub trial_ends_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRenewalRequest {
    renewal_policy: String,
    auto_renew_max_price_minor: Option<i64>,
    #[serde(default)]
    renewal_priority: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CancelRecurringRequest {
    #[serde(default = "default_cancel_mode")]
    mode: String,
}

fn default_cancel_mode() -> String {
    "period_end".into()
}

pub(crate) fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/market-billing/recurring-contracts/:id/renewal",
            put(update_renewal),
        )
        .route(
            "/v1/market-billing/recurring-contracts/:id/reserve-next",
            post(reserve_next_period),
        )
        .route(
            "/v1/market-billing/recurring-contracts/:id/cancel",
            post(cancel_recurring),
        )
}

fn map_db(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AppError::Internal("stored recurring billing timestamp is invalid".into()))
}

fn money_units(amount_minor: i64) -> Result<i64, AppError> {
    amount_minor
        .checked_mul(MONEY_UNITS_PER_MINOR)
        .ok_or_else(|| AppError::BadRequest("monthly price is too large".into()))
}

fn floor_minor(amount_units: i64) -> i64 {
    amount_units.max(0) / MONEY_UNITS_PER_MINOR
}

fn ceil_minor(amount_units: i64) -> i64 {
    if amount_units <= 0 {
        0
    } else {
        amount_units / MONEY_UNITS_PER_MINOR + i64::from(amount_units % MONEY_UNITS_PER_MINOR != 0)
    }
}

fn validate_product_kind(product_kind: &str) -> Result<(), AppError> {
    if matches!(product_kind, "share" | "client_host") {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "recurring product kind must be share or client_host".into(),
        ))
    }
}

fn validate_renewal_policy(policy: &str) -> Result<&str, AppError> {
    match policy {
        RENEWAL_MANUAL | RENEWAL_AUTOMATIC => Ok(policy),
        _ => Err(AppError::BadRequest(
            "renewalPolicy must be manual or automatic".into(),
        )),
    }
}

fn prepare_fingerprint(
    input: &PrepareRecurringContractInput<'_>,
    renewal_policy: &str,
    max_price: i64,
) -> Result<String, AppError> {
    serde_json::to_string(&serde_json::json!({
        "productKind": input.product_kind,
        "productRef": input.product_ref,
        "activationRef": input.activation_ref,
        "serviceRef": input.service_ref,
        "serviceLabel": input.service_label,
        "buyerUserId": input.buyer_user_id,
        "buyerEmail": input.buyer_email.to_ascii_lowercase(),
        "supplierUserId": input.supplier_user_id,
        "supplierEmail": input.supplier_email.to_ascii_lowercase(),
        "currency": input.currency,
        "cyclePriceMinor": input.cycle_price_minor,
        "offerRevision": input.offer_revision,
        "renewalPolicy": renewal_policy,
        "autoRenewMaxPriceMinor": max_price,
        "renewalPriority": input.renewal_priority,
        "trialAllowanceSeconds": input.trial_allowance_seconds,
    }))
    .map_err(|error| {
        AppError::Internal(format!(
            "encode recurring contract preparation fingerprint failed: {error}"
        ))
    })
}

fn days_in_month(year: i32, month: u32) -> Result<u32, AppError> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next = NaiveDate::from_ymd_opt(next_year, next_month, 1).ok_or_else(|| {
        AppError::Internal("calendar month is outside the supported range".into())
    })?;
    Ok((next - Duration::days(1)).day())
}

pub(crate) fn next_calendar_month_at(
    current: DateTime<Utc>,
    anchor_day: u32,
) -> Result<DateTime<Utc>, AppError> {
    if !(1..=31).contains(&anchor_day) {
        return Err(AppError::Internal(
            "recurring billing anchor day is invalid".into(),
        ));
    }
    let (year, month) = if current.month() == 12 {
        (current.year() + 1, 1)
    } else {
        (current.year(), current.month() + 1)
    };
    let day = anchor_day.min(days_in_month(year, month)?);
    let date = NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| AppError::Internal("recurring period date is invalid".into()))?;
    Ok(Utc.from_utc_datetime(&date.and_time(current.time())))
}

fn topup_available_tx(conn: &Connection, supplier_user_id: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM binance_payment_accounts
            WHERE supplier_user_id = ?1 AND status = 'verified'
              AND automation_mode = 'enabled' AND uid_confirmed = 1
              AND credentials_ciphertext != ''
         )",
        params![supplier_user_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(map_db("read recurring top-up availability"))
}

pub(crate) fn recurring_funding_summary_tx(
    conn: &Connection,
    buyer_user_id: &str,
    supplier_user_id: &str,
    supplier_email: &str,
    currency: &str,
    cycle_price_minor: i64,
    renewal_policy: &str,
) -> Result<RecurringFundingSummaryView, AppError> {
    let renewal_policy = validate_renewal_policy(renewal_policy)?;
    if currency != crate::market_billing::MARKET_CURRENCY {
        return Err(AppError::BadRequest("currency must be USD".into()));
    }
    if cycle_price_minor <= 0 {
        return Err(AppError::BadRequest(
            "monthly price must be positive".into(),
        ));
    }
    let account = conn
        .query_row(
            "SELECT id, posted_balance_units, held_balance_units
             FROM market_prepaid_accounts
             WHERE buyer_user_id = ?1 AND supplier_user_id = ?2 AND currency = ?3
               AND status != 'closed'",
            params![buyer_user_id, supplier_user_id, currency],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read recurring prepaid account"))?;
    let (account_id, posted_units, held_units) = account
        .map(|(id, posted, held)| (Some(id), posted, held))
        .unwrap_or((None, 0, 0));
    let available_units = posted_units.saturating_sub(held_units).max(0);
    let initial_units = money_units(cycle_price_minor)?;
    let renewal_units = if renewal_policy == RENEWAL_AUTOMATIC {
        initial_units
    } else {
        0
    };
    let required_units = initial_units
        .checked_add(renewal_units)
        .ok_or_else(|| AppError::BadRequest("monthly funding requirement is too large".into()))?;
    let required_topup_units = required_units.saturating_sub(available_units).max(0);
    let topup_available = topup_available_tx(conn, supplier_user_id)?;
    Ok(RecurringFundingSummaryView {
        supplier_user_id: supplier_user_id.into(),
        supplier_email: supplier_email.to_ascii_lowercase(),
        currency: currency.into(),
        pricing_model: PRICING_PREPAID_CALENDAR_MONTH.into(),
        billing_interval: BILLING_INTERVAL_CALENDAR_MONTH.into(),
        cycle_price_minor,
        renewal_policy: renewal_policy.into(),
        prepaid_account_id: account_id,
        prepaid_balance_minor: floor_minor(posted_units),
        prepaid_held_minor: floor_minor(held_units),
        prepaid_available_minor: floor_minor(available_units),
        initial_hold_minor: cycle_price_minor,
        renewal_hold_minor: if renewal_policy == RENEWAL_AUTOMATIC {
            cycle_price_minor
        } else {
            0
        },
        total_required_hold_minor: ceil_minor(required_units),
        required_topup_minor: ceil_minor(required_topup_units),
        topup_available,
        topup_unavailable_reason: (!topup_available).then(|| "supplier_unavailable".into()),
    })
}

fn ensure_period_tx(
    conn: &Connection,
    contract_id: &str,
    sequence: i64,
    amount_minor: i64,
    offer_revision: i64,
    now: &str,
) -> Result<String, AppError> {
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM market_recurring_periods
             WHERE contract_id = ?1 AND sequence = ?2",
            params![contract_id, sequence],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read recurring period"))?
    {
        return Ok(id);
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO market_recurring_periods (
            id, contract_id, sequence, period_start, period_end,
            amount_minor, offer_revision, status, refunded_units,
            idempotency_key, created_at, updated_at
         ) VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?5, 'reserved', 0, ?6, ?7, ?7)",
        params![
            id,
            contract_id,
            sequence,
            amount_minor,
            offer_revision,
            format!("recurring-period:{contract_id}:{sequence}"),
            now,
        ],
    )
    .map_err(map_db("create recurring period"))?;
    Ok(id)
}

fn create_hold_tx(
    conn: &Connection,
    account_id: &str,
    contract_id: &str,
    period_sequence: i64,
    purpose: &str,
    amount_minor: i64,
    offer_revision: i64,
    now: &str,
) -> Result<bool, AppError> {
    let existing = conn
        .query_row(
            "SELECT id, prepaid_account_id, amount_units, status
             FROM market_recurring_holds
             WHERE contract_id = ?1 AND period_sequence = ?2
             ORDER BY created_at DESC LIMIT 1",
            params![contract_id, period_sequence],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read recurring hold"))?;
    let amount_units = money_units(amount_minor)?;
    if let Some((_, existing_account, existing_amount, status)) = existing.as_ref() {
        if matches!(status.as_str(), "active" | "captured") {
            if existing_account != account_id || *existing_amount != amount_units {
                return Err(AppError::Conflict(
                    "the recurring period already has a funding hold with different terms".into(),
                ));
            }
            return Ok(true);
        }
    }
    let changed = conn
        .execute(
            "UPDATE market_prepaid_accounts
             SET held_balance_units = held_balance_units + ?2,
                 version = version + 1, updated_at = ?3
             WHERE id = ?1 AND status != 'closed'
               AND posted_balance_units - held_balance_units >= ?2",
            params![account_id, amount_units, now],
        )
        .map_err(map_db("reserve recurring prepaid funds"))?;
    if changed != 1 {
        return Ok(false);
    }
    ensure_period_tx(
        conn,
        contract_id,
        period_sequence,
        amount_minor,
        offer_revision,
        now,
    )?;
    if let Some((hold_id, _, _, status)) = existing {
        if status != "released" {
            return Err(AppError::Conflict(
                "the recurring period funding hold is not reusable".into(),
            ));
        }
        let changed = conn
            .execute(
                "UPDATE market_recurring_holds
                 SET prepaid_account_id = ?2, purpose = ?3, amount_units = ?4,
                     status = 'active', release_reason = NULL, released_at = NULL,
                     captured_at = NULL, updated_at = ?5
                 WHERE id = ?1 AND status = 'released'",
                params![hold_id, account_id, purpose, amount_units, now],
            )
            .map_err(map_db("reuse recurring hold"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "the recurring period funding hold changed concurrently".into(),
            ));
        }
        conn.execute(
            "UPDATE market_recurring_periods
             SET status = 'reserved', failed_at = NULL, failure_reason = NULL,
                 updated_at = ?3
             WHERE contract_id = ?1 AND sequence = ?2 AND status = 'failed'",
            params![contract_id, period_sequence, now],
        )
        .map_err(map_db("reuse recurring period"))?;
    } else {
        conn.execute(
            "INSERT INTO market_recurring_holds (
                id, prepaid_account_id, contract_id, period_sequence, purpose,
                amount_units, status, idempotency_key, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?8, ?8)",
            params![
                Uuid::new_v4().to_string(),
                account_id,
                contract_id,
                period_sequence,
                purpose,
                amount_units,
                format!("recurring-hold:{contract_id}:{period_sequence}"),
                now,
            ],
        )
        .map_err(map_db("create recurring hold"))?;
    }
    Ok(true)
}

fn release_hold_tx(
    conn: &Connection,
    hold_id: &str,
    reason: &str,
    now: &str,
) -> Result<bool, AppError> {
    let hold = conn
        .query_row(
            "SELECT prepaid_account_id, amount_units, status
             FROM market_recurring_holds WHERE id = ?1",
            params![hold_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read recurring hold for release"))?;
    let Some((account_id, amount_units, status)) = hold else {
        return Ok(false);
    };
    if status != "active" {
        return Ok(false);
    }
    let changed = conn
        .execute(
            "UPDATE market_prepaid_accounts
             SET held_balance_units = held_balance_units - ?2,
                 version = version + 1, updated_at = ?3
             WHERE id = ?1 AND held_balance_units >= ?2",
            params![account_id, amount_units, now],
        )
        .map_err(map_db("release recurring prepaid funds"))?;
    if changed != 1 {
        return Err(AppError::Internal(
            "recurring hold exceeds the account held balance".into(),
        ));
    }
    conn.execute(
        "UPDATE market_recurring_holds
         SET status = 'released', release_reason = ?2, released_at = ?3, updated_at = ?3
         WHERE id = ?1 AND status = 'active'",
        params![hold_id, reason, now],
    )
    .map_err(map_db("finish recurring hold release"))?;
    Ok(true)
}

fn release_holds_from_sequence_tx(
    conn: &Connection,
    contract_id: &str,
    minimum_sequence: i64,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    let holds = conn
        .prepare(
            "SELECT id FROM market_recurring_holds
             WHERE contract_id = ?1 AND period_sequence >= ?2 AND status = 'active'
             ORDER BY period_sequence, id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![contract_id, minimum_sequence], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("load recurring holds for release"))?;
    for hold_id in holds {
        release_hold_tx(conn, &hold_id, reason, now)?;
    }
    conn.execute(
        "UPDATE market_recurring_periods
         SET status = 'failed', failed_at = COALESCE(failed_at, ?4),
             failure_reason = ?3, updated_at = ?4
         WHERE contract_id = ?1 AND sequence >= ?2 AND status = 'reserved'",
        params![contract_id, minimum_sequence, reason, now],
    )
    .map_err(map_db("fail released recurring periods"))?;
    Ok(())
}

fn claim_trial_allowance_tx(
    conn: &Connection,
    contract_id: &str,
    input: &PrepareRecurringContractInput<'_>,
    now: &str,
) -> Result<i64, AppError> {
    let requested = input.trial_allowance_seconds.max(0);
    if requested == 0 {
        return Ok(0);
    }
    conn.execute(
        "INSERT INTO market_trial_ledgers (
            buyer_user_id, supplier_user_id, product_kind, service_ref, currency,
            allowance_seconds, consumed_seconds, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7)
         ON CONFLICT(buyer_user_id, supplier_user_id, product_kind, service_ref, currency)
         DO UPDATE SET updated_at = excluded.updated_at",
        params![
            input.buyer_user_id,
            input.supplier_user_id,
            input.product_kind,
            input.service_ref,
            input.currency,
            requested,
            now,
        ],
    )
    .map_err(map_db("ensure recurring trial ledger"))?;
    let available = conn
        .query_row(
            "SELECT MAX(
                    ledger.allowance_seconds - ledger.consumed_seconds - COALESCE((
                        SELECT SUM(claim.claimed_seconds)
                        FROM market_recurring_trial_claims claim
                        WHERE claim.buyer_user_id = ledger.buyer_user_id
                          AND claim.supplier_user_id = ledger.supplier_user_id
                          AND claim.product_kind = ledger.product_kind
                          AND claim.service_ref = ledger.service_ref
                          AND claim.currency = ledger.currency
                          AND claim.status IN ('reserved', 'active')
                    ), 0),
                    0
                )
             FROM market_trial_ledgers ledger
             WHERE ledger.buyer_user_id = ?1 AND ledger.supplier_user_id = ?2
               AND ledger.product_kind = ?3 AND ledger.service_ref = ?4
               AND ledger.currency = ?5",
            params![
                input.buyer_user_id,
                input.supplier_user_id,
                input.product_kind,
                input.service_ref,
                input.currency,
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("read recurring trial availability"))?;
    if available < requested {
        return Err(AppError::Conflict(
            "trial availability changed while reserving this rental; request a new quote".into(),
        ));
    }
    conn.execute(
        "INSERT INTO market_recurring_trial_claims (
            contract_id, buyer_user_id, supplier_user_id, product_kind,
            service_ref, currency, claimed_seconds, status,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'reserved', ?8, ?8)",
        params![
            contract_id,
            input.buyer_user_id,
            input.supplier_user_id,
            input.product_kind,
            input.service_ref,
            input.currency,
            requested,
            now,
        ],
    )
    .map_err(map_db("claim recurring trial allowance"))?;
    Ok(requested)
}

fn activate_trial_claim_tx(
    conn: &Connection,
    contract_id: &str,
    activated_at: DateTime<Utc>,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE market_recurring_trial_claims
         SET status = 'active', activated_at = COALESCE(activated_at, ?2),
             updated_at = ?3
         WHERE contract_id = ?1 AND status = 'reserved'",
        params![contract_id, activated_at.to_rfc3339(), now],
    )
    .map_err(map_db("activate recurring trial claim"))?;
    Ok(())
}

fn settle_trial_claim_tx(
    conn: &Connection,
    contract_id: &str,
    settled_at: DateTime<Utc>,
    consume_full_claim: bool,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    let claim = conn
        .query_row(
            "SELECT claim.buyer_user_id, claim.supplier_user_id, claim.product_kind,
                    claim.service_ref, claim.currency, claim.claimed_seconds,
                    claim.status, claim.activated_at,
                    contract.trial_seconds_remaining
             FROM market_recurring_trial_claims claim
             JOIN market_recurring_contracts contract ON contract.id = claim.contract_id
             WHERE claim.contract_id = ?1",
            params![contract_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read recurring trial claim for settlement"))?;
    let Some((
        buyer,
        supplier,
        product_kind,
        service_ref,
        currency,
        claimed,
        status,
        activated,
        remaining,
    )) = claim
    else {
        return Ok(());
    };
    if matches!(status.as_str(), "settled" | "released") {
        return Ok(());
    }
    let Some(activated) = activated else {
        conn.execute(
            "UPDATE market_recurring_trial_claims
             SET status = 'released', release_reason = ?2, settled_at = ?3,
                 updated_at = ?4
             WHERE contract_id = ?1 AND status = 'reserved'",
            params![contract_id, reason, settled_at.to_rfc3339(), now],
        )
        .map_err(map_db("release unused recurring trial claim"))?;
        return Ok(());
    };
    let _activated_at = parse_time(&activated)?;
    let consumed = if consume_full_claim {
        claimed
    } else {
        claimed.saturating_sub(remaining.clamp(0, claimed))
    };
    if consumed > 0 {
        let changed = conn
            .execute(
                "UPDATE market_trial_ledgers
                 SET consumed_seconds = MIN(allowance_seconds, consumed_seconds + ?6),
                     updated_at = ?7
                 WHERE buyer_user_id = ?1 AND supplier_user_id = ?2
                   AND product_kind = ?3 AND service_ref = ?4 AND currency = ?5",
                params![
                    buyer,
                    supplier,
                    product_kind,
                    service_ref,
                    currency,
                    consumed,
                    now,
                ],
            )
            .map_err(map_db("consume recurring trial allowance"))?;
        if changed != 1 {
            return Err(AppError::Internal(
                "recurring trial ledger disappeared during settlement".into(),
            ));
        }
    }
    conn.execute(
        "UPDATE market_recurring_trial_claims
         SET status = 'settled', settled_seconds = ?2, release_reason = ?3,
             settled_at = ?4, updated_at = ?5
         WHERE contract_id = ?1 AND status IN ('reserved', 'active')",
        params![contract_id, consumed, reason, settled_at.to_rfc3339(), now,],
    )
    .map_err(map_db("settle recurring trial claim"))?;
    Ok(())
}

pub(crate) fn prepare_contract_tx(
    conn: &Connection,
    input: PrepareRecurringContractInput<'_>,
    now: &str,
) -> Result<String, AppError> {
    validate_product_kind(input.product_kind)?;
    let renewal_policy = validate_renewal_policy(input.renewal_policy)?;
    if input.currency != crate::market_billing::MARKET_CURRENCY {
        return Err(AppError::BadRequest("currency must be USD".into()));
    }
    if input.cycle_price_minor <= 0
        || input.cycle_price_minor > crate::market_billing::MAX_DAILY_RATE_MINOR
    {
        return Err(AppError::BadRequest(
            "monthly price is outside the supported range".into(),
        ));
    }
    if input.offer_revision <= 0 || input.trial_allowance_seconds < 0 {
        return Err(AppError::BadRequest(
            "recurring offer revision or trial allowance is invalid".into(),
        ));
    }
    if input.renewal_priority < 0 {
        return Err(AppError::BadRequest(
            "renewalPriority must not be negative".into(),
        ));
    }
    let max_price = input
        .auto_renew_max_price_minor
        .unwrap_or(input.cycle_price_minor);
    if max_price < input.cycle_price_minor {
        return Err(AppError::BadRequest(
            "autoRenewMaxPriceMinor cannot be below the current monthly price".into(),
        ));
    }
    let fingerprint = prepare_fingerprint(&input, renewal_policy, max_price)?;
    let existing = conn
        .query_row(
            "SELECT id, prepare_fingerprint
             FROM market_recurring_contracts
             WHERE product_kind = ?1 AND activation_ref = ?2
               AND status NOT IN ('ended', 'activation_failed')",
            params![input.product_kind, input.activation_ref],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(map_db("read idempotent recurring contract"))?;
    if let Some((id, existing_fingerprint)) = existing {
        if existing_fingerprint == fingerprint {
            return Ok(id);
        }
        return Err(AppError::Conflict(
            "recurring activation reference was already used with different terms".into(),
        ));
    }
    let account_id = crate::market_billing::ensure_market_prepaid_account_tx(
        conn,
        input.buyer_user_id,
        input.buyer_email,
        input.supplier_user_id,
        input.supplier_email,
        input.currency,
        now,
    )?;
    let summary = recurring_funding_summary_tx(
        conn,
        input.buyer_user_id,
        input.supplier_user_id,
        input.supplier_email,
        input.currency,
        input.cycle_price_minor,
        renewal_policy,
    )?;
    if summary.required_topup_minor > 0 {
        return Err(AppError::coded_conflict(
            crate::market_access::ERROR_MARKET_PREPAID_REQUIRED,
            "a full monthly payment must be prepaid before renting this service",
            serde_json::json!({
                "supplierUserId": input.supplier_user_id,
                "supplierEmail": input.supplier_email,
                "currency": input.currency,
                "pricingModel": PRICING_PREPAID_CALENDAR_MONTH,
                "cyclePriceMinor": input.cycle_price_minor,
                "renewalPolicy": renewal_policy,
                "requiredTopupMinor": summary.required_topup_minor,
                "prepaidAvailableMinor": summary.prepaid_available_minor,
                "totalRequiredHoldMinor": summary.total_required_hold_minor,
            }),
        ));
    }
    let contract_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO market_recurring_contracts (
            id, prepaid_account_id, product_kind, product_ref, activation_ref,
            prepare_fingerprint, service_ref, service_label, buyer_user_id, buyer_email,
            supplier_user_id, supplier_email, currency, pricing_model,
            billing_interval, cycle_price_minor, offer_revision, status,
            renewal_policy, renewal_status, auto_renew_max_price_minor,
            renewal_priority, trial_allowance_seconds, trial_seconds_remaining,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                   'prepaid_calendar_month', 'calendar_month', ?14, ?15,
                   'pending_activation', ?16, 'initial_funded', ?17, ?18, ?19, ?19,
                   ?20, ?20)",
        params![
            contract_id,
            account_id,
            input.product_kind,
            input.product_ref,
            input.activation_ref,
            fingerprint,
            input.service_ref,
            input.service_label,
            input.buyer_user_id,
            input.buyer_email.to_ascii_lowercase(),
            input.supplier_user_id,
            input.supplier_email.to_ascii_lowercase(),
            input.currency,
            input.cycle_price_minor,
            input.offer_revision,
            renewal_policy,
            max_price,
            input.renewal_priority,
            input.trial_allowance_seconds,
            now,
        ],
    )
    .map_err(map_db("create recurring contract"))?;
    let claimed_trial_seconds = claim_trial_allowance_tx(conn, &contract_id, &input, now)?;
    if claimed_trial_seconds != input.trial_allowance_seconds {
        conn.execute(
            "UPDATE market_recurring_contracts
             SET trial_allowance_seconds = ?2, trial_seconds_remaining = ?2,
                 updated_at = ?3 WHERE id = ?1",
            params![contract_id, claimed_trial_seconds, now],
        )
        .map_err(map_db("store recurring trial claim"))?;
    }
    if !create_hold_tx(
        conn,
        &account_id,
        &contract_id,
        1,
        "initial_activation",
        input.cycle_price_minor,
        input.offer_revision,
        now,
    )? {
        return Err(AppError::Conflict(
            "prepaid balance changed while reserving the first monthly payment; retry".into(),
        ));
    }
    if renewal_policy == RENEWAL_AUTOMATIC {
        if !create_hold_tx(
            conn,
            &account_id,
            &contract_id,
            2,
            "automatic_renewal",
            input.cycle_price_minor,
            input.offer_revision,
            now,
        )? {
            return Err(AppError::Conflict(
                "prepaid balance changed while reserving automatic renewal; retry".into(),
            ));
        }
        conn.execute(
            "UPDATE market_recurring_contracts
             SET renewal_status = 'funded', updated_at = ?2 WHERE id = ?1",
            params![contract_id, now],
        )
        .map_err(map_db("mark recurring renewal funded"))?;
    }
    Ok(contract_id)
}

fn set_period_bounds_tx(
    conn: &Connection,
    contract_id: &str,
    sequence: i64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE market_recurring_periods
         SET period_start = ?3, period_end = ?4, updated_at = ?5
         WHERE contract_id = ?1 AND sequence = ?2",
        params![
            contract_id,
            sequence,
            start.to_rfc3339(),
            end.to_rfc3339(),
            now,
        ],
    )
    .map_err(map_db("set recurring period bounds"))?;
    conn.execute(
        "UPDATE market_recurring_periods
         SET period_start = ?3, period_end = ?4, updated_at = ?5
         WHERE contract_id = ?1 AND sequence = ?2
           AND period_start IS NULL AND period_end IS NULL",
        params![
            contract_id,
            sequence + 1,
            end.to_rfc3339(),
            next_calendar_month_at(end, start.day())?.to_rfc3339(),
            now,
        ],
    )
    .map_err(map_db("set next recurring period bounds"))?;
    Ok(())
}

fn capture_hold_tx(
    conn: &Connection,
    contract_id: &str,
    period_sequence: i64,
    now: &str,
) -> Result<bool, AppError> {
    let hold = conn
        .query_row(
            "SELECT hold.id, hold.prepaid_account_id, hold.amount_units, hold.status,
                    period.id, period.amount_minor
             FROM market_recurring_holds hold
             JOIN market_recurring_periods period
               ON period.contract_id = hold.contract_id
              AND period.sequence = hold.period_sequence
             WHERE hold.contract_id = ?1 AND hold.period_sequence = ?2
             ORDER BY hold.created_at DESC LIMIT 1",
            params![contract_id, period_sequence],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read recurring hold for capture"))?;
    let Some((hold_id, account_id, amount_units, status, period_id, amount_minor)) = hold else {
        return Ok(false);
    };
    if status == "captured" {
        return Ok(true);
    }
    if status != "active" || amount_units != money_units(amount_minor)? {
        return Ok(false);
    }
    let changed = conn
        .execute(
            "UPDATE market_prepaid_accounts
             SET posted_balance_units = posted_balance_units - ?2,
                 held_balance_units = held_balance_units - ?2,
                 version = version + 1, updated_at = ?3
             WHERE id = ?1 AND posted_balance_units >= ?2
               AND held_balance_units >= ?2",
            params![account_id, amount_units, now],
        )
        .map_err(map_db("capture recurring prepaid funds"))?;
    if changed != 1 {
        return Err(AppError::Internal(
            "reserved monthly payment is inconsistent with the prepaid account".into(),
        ));
    }
    let balance_after_units = conn
        .query_row(
            "SELECT posted_balance_units FROM market_prepaid_accounts WHERE id = ?1",
            params![account_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("read balance after recurring capture"))?;
    let ledger_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO market_prepaid_ledger_entries (
            id, account_id, entry_kind, direction, amount_units,
            balance_after_units, source_kind, source_id, idempotency_key,
            detail_json, actor_user_id, created_at
         ) VALUES (?1, ?2, 'usage_debit', 'debit', ?3, ?4,
                   'recurring_period', ?5, ?6, ?7, NULL, ?8)",
        params![
            ledger_id,
            account_id,
            amount_units,
            balance_after_units,
            period_id,
            format!("recurring-period-debit:{period_id}"),
            serde_json::json!({
                "contractId": contract_id,
                "periodSequence": period_sequence,
                "pricingModel": PRICING_PREPAID_CALENDAR_MONTH,
                "amountMinor": amount_minor,
            })
            .to_string(),
            now,
        ],
    )
    .map_err(map_db("record recurring period debit"))?;
    conn.execute(
        "UPDATE market_recurring_holds
         SET status = 'captured', captured_at = ?2, updated_at = ?2
         WHERE id = ?1 AND status = 'active'",
        params![hold_id, now],
    )
    .map_err(map_db("finish recurring hold capture"))?;
    conn.execute(
        "UPDATE market_recurring_periods
         SET status = 'paid', debit_ledger_entry_id = ?2, paid_at = ?3, updated_at = ?3
         WHERE id = ?1 AND status = 'reserved'",
        params![period_id, ledger_id, now],
    )
    .map_err(map_db("mark recurring period paid"))?;
    Ok(true)
}

fn initialize_paid_service_tx(
    conn: &Connection,
    contract_id: &str,
    starts_at: DateTime<Utc>,
    now: &str,
) -> Result<bool, AppError> {
    let (status, price, revision) = conn
        .query_row(
            "SELECT status, cycle_price_minor, offer_revision
             FROM market_recurring_contracts WHERE id = ?1",
            params![contract_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .map_err(map_db("read recurring contract for activation"))?;
    if status == STATUS_ACTIVE {
        return Ok(true);
    }
    if status == STATUS_TRIAL {
        settle_trial_claim_tx(conn, contract_id, starts_at, true, "trial_completed", now)?;
    }
    ensure_period_tx(conn, contract_id, 1, price, revision, now)?;
    let anchor_day = starts_at.day();
    let ends_at = next_calendar_month_at(starts_at, anchor_day)?;
    set_period_bounds_tx(conn, contract_id, 1, starts_at, ends_at, now)?;
    if !capture_hold_tx(conn, contract_id, 1, now)? {
        return Ok(false);
    }
    let next_funded = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM market_recurring_holds
                WHERE contract_id = ?1 AND period_sequence = 2 AND status = 'active'
             )",
            params![contract_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("read first recurring renewal hold"))?
        != 0;
    conn.execute(
        "UPDATE market_recurring_contracts
         SET status = 'active', renewal_status = ?2, anchor_day = ?3,
             anchor_at = ?4, current_period_sequence = 1,
             current_period_start = ?4, current_period_end = ?5,
             trial_seconds_remaining = 0,
             trial_ends_at = NULL, trial_deadline_at = NULL,
             desired_control_state = 'active', control_error = NULL,
             activated_at = COALESCE(activated_at, ?4),
             version = version + 1, updated_at = ?6
         WHERE id = ?1 AND status IN ('pending_activation', 'trial', 'recovery')",
        params![
            contract_id,
            if next_funded {
                "funded"
            } else {
                "funding_required"
            },
            i64::from(anchor_day),
            starts_at.to_rfc3339(),
            ends_at.to_rfc3339(),
            now,
        ],
    )
    .map_err(map_db("activate recurring monthly service"))?;
    Ok(true)
}

pub(crate) fn activate_contract_tx(
    conn: &Connection,
    product_kind: &str,
    activation_ref: &str,
    product_ref: &str,
    service_ref: &str,
    service_label: &str,
    activated_at: DateTime<Utc>,
    now: &str,
) -> Result<Option<String>, AppError> {
    let contract = conn
        .query_row(
            "SELECT id, status, product_ref, service_ref, service_label,
                    trial_allowance_seconds
             FROM market_recurring_contracts
             WHERE product_kind = ?1 AND activation_ref = ?2
               AND status NOT IN ('ended', 'activation_failed')",
            params![product_kind, activation_ref],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read recurring activation"))?;
    let Some((
        contract_id,
        status,
        stored_product_ref,
        stored_service_ref,
        stored_service_label,
        trial_seconds,
    )) = contract
    else {
        return Ok(None);
    };
    if status != STATUS_PENDING_ACTIVATION {
        if stored_product_ref == product_ref
            && stored_service_ref == service_ref
            && stored_service_label == service_label
        {
            return Ok(Some(contract_id));
        }
        return Err(AppError::Conflict(
            "recurring activation reference is already bound to another service".into(),
        ));
    }
    conn.execute(
        "UPDATE market_recurring_contracts
         SET product_ref = ?2, service_ref = ?3, service_label = ?4,
             activated_at = COALESCE(activated_at, ?5), updated_at = ?6,
             version = version + 1
         WHERE id = ?1",
        params![
            contract_id,
            product_ref,
            service_ref,
            service_label,
            activated_at.to_rfc3339(),
            now,
        ],
    )
    .map_err(map_db("bind recurring activation"))?;
    if trial_seconds > 0 {
        let trial_ends_at = activated_at + Duration::seconds(trial_seconds);
        activate_trial_claim_tx(conn, &contract_id, activated_at, now)?;
        conn.execute(
            "UPDATE market_recurring_contracts
             SET status = 'trial', trial_seconds_remaining = ?2,
                 trial_last_evaluated_at = ?3, trial_health_state = 'unknown',
                 trial_ends_at = ?4, trial_deadline_at = ?4,
                 desired_control_state = 'active', control_error = NULL,
                 version = version + 1, updated_at = ?5
             WHERE id = ?1 AND status = 'pending_activation'",
            params![
                contract_id,
                trial_seconds,
                activated_at.to_rfc3339(),
                trial_ends_at.to_rfc3339(),
                now
            ],
        )
        .map_err(map_db("start recurring trial"))?;
    } else if !initialize_paid_service_tx(conn, &contract_id, activated_at, now)? {
        return Err(AppError::Conflict(
            "the first monthly payment reservation is no longer available".into(),
        ));
    }
    Ok(Some(contract_id))
}

pub(crate) fn fail_activation_tx(
    conn: &Connection,
    product_kind: &str,
    activation_ref: &str,
    reason: &str,
    now: &str,
) -> Result<bool, AppError> {
    let contract_id = conn
        .query_row(
            "SELECT id FROM market_recurring_contracts
             WHERE product_kind = ?1 AND activation_ref = ?2
               AND status = 'pending_activation'",
            params![product_kind, activation_ref],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read failed recurring activation"))?;
    let Some(contract_id) = contract_id else {
        return Ok(false);
    };
    settle_trial_claim_tx(conn, &contract_id, parse_time(now)?, false, reason, now)?;
    release_holds_from_sequence_tx(conn, &contract_id, 1, reason, now)?;
    conn.execute(
        "UPDATE market_recurring_contracts
         SET status = 'activation_failed', renewal_status = 'ended',
             desired_control_state = 'terminated', applied_control_state = 'terminated',
             end_reason = ?2, ended_at = ?3, version = version + 1, updated_at = ?3
         WHERE id = ?1 AND status = 'pending_activation'",
        params![contract_id, reason, now],
    )
    .map_err(map_db("fail recurring activation"))?;
    Ok(true)
}

fn contract_next_sequence_tx(conn: &Connection, contract_id: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT CASE WHEN current_period_sequence = 0 THEN 2
                     ELSE current_period_sequence + 1 END
         FROM market_recurring_contracts WHERE id = ?1",
        params![contract_id],
        |row| row.get::<_, i64>(0),
    )
    .map_err(map_db("read next recurring period sequence"))
}

fn reserve_next_for_contract_tx(
    conn: &Connection,
    contract_id: &str,
    purpose: &str,
    now: &str,
) -> Result<bool, AppError> {
    let (account_id, price, revision, max_price, status, recovery_deadline) = conn
        .query_row(
            "SELECT prepaid_account_id, cycle_price_minor, offer_revision,
                    auto_renew_max_price_minor, status, recovery_deadline
             FROM market_recurring_contracts WHERE id = ?1",
            params![contract_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .map_err(map_db("read recurring contract for renewal hold"))?;
    if matches!(status.as_str(), STATUS_ENDED | STATUS_ACTIVATION_FAILED)
        || max_price.is_some_and(|limit| price > limit)
        || (status == STATUS_RECOVERY
            && recovery_deadline
                .as_deref()
                .map(parse_time)
                .transpose()?
                .is_none_or(|deadline| deadline <= parse_time(now).unwrap_or(deadline)))
    {
        return Ok(false);
    }
    let sequence = contract_next_sequence_tx(conn, contract_id)?;
    create_hold_tx(
        conn,
        &account_id,
        contract_id,
        sequence,
        purpose,
        price,
        revision,
        now,
    )
}

fn required_topup_minor_for_contract_tx(
    conn: &Connection,
    contract_id: &str,
) -> Result<i64, AppError> {
    let (price, available_units) = conn
        .query_row(
            "SELECT contract.cycle_price_minor,
                    MAX(account.posted_balance_units - account.held_balance_units, 0)
             FROM market_recurring_contracts contract
             JOIN market_prepaid_accounts account
               ON account.id = contract.prepaid_account_id
             WHERE contract.id = ?1",
            params![contract_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(map_db("read recurring reservation shortfall"))?;
    Ok(ceil_minor(
        money_units(price)?.saturating_sub(available_units),
    ))
}

fn resume_funded_recovery_contract_tx(
    conn: &Connection,
    contract_id: &str,
    now: &str,
) -> Result<bool, AppError> {
    let status = conn
        .query_row(
            "SELECT status FROM market_recurring_contracts WHERE id = ?1",
            params![contract_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(map_db("read recurring recovery status"))?;
    if status != STATUS_RECOVERY {
        return Ok(false);
    }
    let next_sequence = contract_next_sequence_tx(conn, contract_id)?;
    let funded = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM market_recurring_holds
                WHERE contract_id = ?1 AND period_sequence = ?2 AND status = 'active'
             )",
            params![contract_id, next_sequence],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("verify recurring recovery funding"))?
        != 0;
    if !funded {
        return Ok(false);
    }
    let now_dt = parse_time(now)?;
    conn_set_status_active_for_retry(conn, contract_id, now)?;
    for _ in 0..MAX_RECONCILE_ADVANCES {
        let advanced = advance_paid_period_tx(conn, contract_id, now_dt)?;
        let (next_status, current_period_end) = conn
            .query_row(
                "SELECT status, current_period_end
                 FROM market_recurring_contracts WHERE id = ?1",
                params![contract_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .map_err(map_db("read advanced recurring recovery"))?;
        if next_status != STATUS_ACTIVE {
            return Ok(false);
        }
        let caught_up = current_period_end
            .as_deref()
            .map(parse_time)
            .transpose()?
            .is_some_and(|period_end| period_end > now_dt);
        if caught_up {
            return Ok(true);
        }
        if !advanced {
            return Err(AppError::Internal(
                "funded recurring recovery could not advance its overdue period".into(),
            ));
        }
    }
    Err(AppError::Internal(
        "recurring recovery exceeded the catch-up limit".into(),
    ))
}

pub(crate) fn reserve_automatic_contracts_for_account_tx(
    conn: &Connection,
    prepaid_account_id: &str,
    now: &str,
) -> Result<usize, AppError> {
    let contract_ids = conn
        .prepare(
            "SELECT contract.id
             FROM market_recurring_contracts contract
             WHERE contract.prepaid_account_id = ?1
               AND contract.status IN ('active', 'recovery')
               AND contract.renewal_policy = 'automatic'
               AND contract.cancel_at_period_end = 0
               AND contract.cycle_price_minor <= COALESCE(
                    contract.auto_renew_max_price_minor,
                    contract.cycle_price_minor
               )
               AND (contract.status != 'recovery' OR contract.recovery_deadline > ?2)
               AND NOT EXISTS (
                    SELECT 1 FROM market_recurring_holds hold
                    WHERE hold.contract_id = contract.id
                      AND hold.period_sequence = contract.current_period_sequence + 1
                      AND hold.status = 'active'
               )
             ORDER BY CASE contract.status WHEN 'recovery' THEN 0 ELSE 1 END,
                      contract.renewal_priority DESC,
                      COALESCE(contract.recovery_deadline, contract.current_period_end),
                      contract.id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![prepaid_account_id, now], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("load automatic recurring funding candidates"))?;
    let mut reserved = 0_usize;
    let mut funded_contract_ids = Vec::new();
    for contract_id in contract_ids {
        if reserve_next_for_contract_tx(conn, &contract_id, "automatic_renewal", now)? {
            conn.execute(
                "UPDATE market_recurring_contracts
                 SET renewal_status = 'funded', version = version + 1, updated_at = ?2
                 WHERE id = ?1 AND status IN ('active', 'recovery')",
                params![contract_id, now],
            )
            .map_err(map_db("mark automatically funded recurring contract"))?;
            funded_contract_ids.push(contract_id);
            reserved += 1;
        }
    }
    // Give every eligible contract one reservation before a recovery contract
    // advances. Advancing an automatic contract immediately tries to fund the
    // following month, which must not starve another contract on this account.
    for contract_id in funded_contract_ids {
        resume_funded_recovery_contract_tx(conn, &contract_id, now)?;
    }
    Ok(reserved)
}

fn enter_recovery_tx(
    conn: &Connection,
    contract_id: &str,
    product_kind: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let recovery_hours = if product_kind == "client_host" {
        CLIENT_RECOVERY_HOURS
    } else {
        SHARE_RECOVERY_HOURS
    };
    conn.execute(
        "UPDATE market_recurring_contracts
         SET status = 'recovery', renewal_status = 'funding_required',
             recovery_deadline = COALESCE(recovery_deadline, ?2),
             desired_control_state = 'suspended', control_error = 'renewal_funding_required',
             version = version + 1, updated_at = ?3
         WHERE id = ?1 AND status = 'active'",
        params![
            contract_id,
            (now + Duration::hours(recovery_hours)).to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(map_db("enter recurring renewal recovery"))?;
    Ok(())
}

fn end_contract_tx(
    conn: &Connection,
    contract_id: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    settle_trial_claim_tx(conn, contract_id, parse_time(now)?, false, reason, now)?;
    let current_sequence = conn
        .query_row(
            "SELECT current_period_sequence FROM market_recurring_contracts WHERE id = ?1",
            params![contract_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("read ending recurring contract"))?;
    release_holds_from_sequence_tx(conn, contract_id, current_sequence + 1, reason, now)?;
    if current_sequence == 0 {
        release_holds_from_sequence_tx(conn, contract_id, 1, reason, now)?;
    }
    conn.execute(
        "UPDATE market_recurring_contracts
         SET status = 'ended', renewal_status = 'ended',
             desired_control_state = 'terminated', control_error = ?2,
             end_reason = ?2, ended_at = COALESCE(ended_at, ?3),
             recovery_deadline = NULL, version = version + 1, updated_at = ?3
         WHERE id = ?1 AND status NOT IN ('ended', 'activation_failed')",
        params![contract_id, reason, now],
    )
    .map_err(map_db("end recurring contract"))?;
    Ok(())
}

pub(crate) fn terminate_contract_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
    reason: &str,
    now: &str,
) -> Result<bool, AppError> {
    let contract_id = conn
        .query_row(
            "SELECT id FROM market_recurring_contracts
             WHERE product_kind = ?1 AND product_ref = ?2
               AND status NOT IN ('ended', 'activation_failed')",
            params![product_kind, product_ref],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read recurring contract for termination"))?;
    let Some(contract_id) = contract_id else {
        return Ok(false);
    };
    end_contract_tx(conn, &contract_id, reason, now)?;
    Ok(true)
}

pub(crate) fn suspend_contract_for_integrity_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
    reason: &str,
    now: &str,
) -> Result<bool, AppError> {
    let changed = conn
        .execute(
            "UPDATE market_recurring_contracts
             SET desired_control_state = 'suspended', control_error = ?3,
                 version = version + 1, updated_at = ?4
             WHERE product_kind = ?1 AND product_ref = ?2
               AND status IN ('trial', 'active')
               AND desired_control_state != 'terminated'",
            params![product_kind, product_ref, reason, now],
        )
        .map_err(map_db(
            "suspend recurring contract for integrity remediation",
        ))?;
    Ok(changed == 1)
}

pub(crate) fn request_contract_resume_after_integrity_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
    now: &str,
) -> Result<bool, AppError> {
    let changed = conn
        .execute(
            "UPDATE market_recurring_contracts
             SET desired_control_state = 'active', control_error = NULL,
                 version = version + 1, updated_at = ?3
             WHERE product_kind = ?1 AND product_ref = ?2
               AND status IN ('trial', 'active')
               AND desired_control_state = 'suspended'
               AND COALESCE(control_error, '') != 'renewal_funding_required'",
            params![product_kind, product_ref, now],
        )
        .map_err(map_db(
            "request recurring contract resume after integrity remediation",
        ))?;
    Ok(changed == 1)
}

pub(crate) fn complete_contract_resume_after_integrity_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM market_recurring_contracts
            WHERE product_kind = ?1 AND product_ref = ?2
              AND status IN ('trial', 'active')
              AND desired_control_state = 'active'
         )",
        params![product_kind, product_ref],
        |row| row.get::<_, bool>(0),
    )
    .map_err(map_db(
        "confirm recurring contract resume after integrity remediation",
    ))
}

pub(crate) fn supplier_terminate_and_refund_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
    reason: &str,
    now: &str,
) -> Result<Option<i64>, AppError> {
    let contract = conn
        .query_row(
            "SELECT id, prepaid_account_id, current_period_sequence,
                    current_period_start, current_period_end, status,
                    supplier_user_id
             FROM market_recurring_contracts
             WHERE product_kind = ?1 AND product_ref = ?2
               AND status NOT IN ('ended', 'activation_failed')",
            params![product_kind, product_ref],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read recurring supplier termination"))?;
    let Some((
        contract_id,
        account_id,
        sequence,
        period_start,
        period_end,
        _status,
        supplier_user_id,
    )) = contract
    else {
        return Ok(None);
    };
    let now_dt = parse_time(now)?;
    let mut refund_units = 0_i64;
    if sequence > 0
        && let (Some(period_start), Some(period_end)) =
            (period_start.as_deref(), period_end.as_deref())
    {
        let starts_at = parse_time(period_start)?;
        let ends_at = parse_time(period_end)?;
        if now_dt < ends_at {
            let amount_units = conn
                .query_row(
                    "SELECT amount_minor FROM market_recurring_periods
                     WHERE contract_id = ?1 AND sequence = ?2 AND status = 'paid'",
                    params![contract_id, sequence],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(map_db("read refundable recurring period"))?
                .map(money_units)
                .transpose()?
                .unwrap_or(0);
            let total_seconds = (ends_at - starts_at).num_seconds().max(1);
            let remaining_seconds = (ends_at - now_dt).num_seconds().clamp(0, total_seconds);
            refund_units = i64::try_from(
                i128::from(amount_units) * i128::from(remaining_seconds)
                    / i128::from(total_seconds),
            )
            .unwrap_or(amount_units)
            .clamp(0, amount_units);
            if refund_units > 0 {
                let period_id = conn
                    .query_row(
                        "SELECT id FROM market_recurring_periods
                         WHERE contract_id = ?1 AND sequence = ?2",
                        params![contract_id, sequence],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(map_db("read recurring period refund source"))?;
                let idempotency_key = format!("recurring-period-refund:{period_id}");
                let already_refunded = conn
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM market_prepaid_ledger_entries
                            WHERE idempotency_key = ?1
                         )",
                        params![idempotency_key],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(map_db("check recurring period refund"))?
                    != 0;
                if !already_refunded {
                    conn.execute(
                        "UPDATE market_prepaid_accounts
                         SET posted_balance_units = posted_balance_units + ?2,
                             version = version + 1, updated_at = ?3
                         WHERE id = ?1 AND status != 'closed'",
                        params![account_id, refund_units, now],
                    )
                    .map_err(map_db("credit recurring period refund"))?;
                    let balance_after = conn
                        .query_row(
                            "SELECT posted_balance_units FROM market_prepaid_accounts WHERE id = ?1",
                            params![account_id],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(map_db("read recurring refund balance"))?;
                    conn.execute(
                        "INSERT INTO market_prepaid_ledger_entries (
                            id, account_id, entry_kind, direction, amount_units,
                            balance_after_units, source_kind, source_id, idempotency_key,
                            detail_json, actor_user_id, created_at
                         ) VALUES (?1, ?2, 'service_credit', 'credit', ?3, ?4,
                                   'recurring_refund', ?5, ?6, ?7, ?8, ?9)",
                        params![
                            Uuid::new_v4().to_string(),
                            account_id,
                            refund_units,
                            balance_after,
                            period_id,
                            idempotency_key,
                            serde_json::json!({
                                "contractId": contract_id,
                                "periodSequence": sequence,
                                "reason": reason,
                                "remainingSeconds": remaining_seconds,
                                "periodSeconds": total_seconds,
                            })
                            .to_string(),
                            supplier_user_id,
                            now,
                        ],
                    )
                    .map_err(map_db("record recurring period refund"))?;
                    conn.execute(
                        "UPDATE market_recurring_periods
                         SET status = CASE
                                WHEN ?3 >= amount_minor * ?4 THEN 'refunded'
                                ELSE 'partially_refunded' END,
                             refunded_units = ?3, updated_at = ?5
                         WHERE contract_id = ?1 AND sequence = ?2",
                        params![
                            contract_id,
                            sequence,
                            refund_units,
                            MONEY_UNITS_PER_MINOR,
                            now,
                        ],
                    )
                    .map_err(map_db("mark recurring period refunded"))?;
                }
            }
        }
    }
    end_contract_tx(conn, &contract_id, reason, now)?;
    Ok(Some(ceil_minor(refund_units)))
}

pub(crate) fn cancel_at_period_end_for_product_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
    buyer_user_id: &str,
    now: &str,
) -> Result<bool, AppError> {
    let contract = conn
        .query_row(
            "SELECT id, buyer_user_id, status, current_period_sequence,
                    current_period_end
             FROM market_recurring_contracts
             WHERE product_kind = ?1 AND product_ref = ?2
               AND status NOT IN ('ended', 'activation_failed')",
            params![product_kind, product_ref],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read recurring period-end cancellation"))?;
    let Some((contract_id, owner, status, current_sequence, current_period_end)) = contract else {
        return Ok(false);
    };
    if owner != buyer_user_id {
        return Err(AppError::Forbidden(
            "recurring contract belongs to another account".into(),
        ));
    }
    let now_dt = parse_time(now)?;
    let period_has_ended = current_period_end
        .as_deref()
        .map(parse_time)
        .transpose()?
        .is_some_and(|period_end| period_end <= now_dt);
    if current_sequence == 0 || status != STATUS_ACTIVE || period_has_ended {
        let reason = if current_sequence == 0 {
            "buyer_cancelled_before_first_period"
        } else if status == STATUS_RECOVERY || period_has_ended {
            "buyer_cancelled_during_renewal_recovery"
        } else {
            "buyer_cancelled_while_suspended"
        };
        end_contract_tx(conn, &contract_id, reason, now)?;
        return Ok(true);
    }
    release_holds_from_sequence_tx(
        conn,
        &contract_id,
        current_sequence + 1,
        "cancel_at_period_end",
        now,
    )?;
    conn.execute(
        "UPDATE market_recurring_contracts
         SET cancel_at_period_end = 1, renewal_status = 'cancel_at_period_end',
             version = version + 1, updated_at = ?2 WHERE id = ?1",
        params![contract_id, now],
    )
    .map_err(map_db("schedule recurring period-end cancellation"))?;
    Ok(true)
}

fn advance_paid_period_tx(
    conn: &Connection,
    contract_id: &str,
    now: DateTime<Utc>,
) -> Result<bool, AppError> {
    let (product_kind, current_sequence, current_end, anchor_day, policy, cancel, price, revision) =
        conn.query_row(
            "SELECT product_kind, current_period_sequence, current_period_end,
                    anchor_day, renewal_policy, cancel_at_period_end,
                    cycle_price_minor, offer_revision
             FROM market_recurring_contracts WHERE id = ?1",
            params![contract_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .map_err(map_db("read due recurring contract"))?;
    let current_end = parse_time(&current_end)?;
    if current_end > now {
        return Ok(false);
    }
    if cancel {
        end_contract_tx(
            conn,
            contract_id,
            "cancelled_at_period_end",
            &now.to_rfc3339(),
        )?;
        return Ok(false);
    }
    let next_sequence = current_sequence + 1;
    ensure_period_tx(
        conn,
        contract_id,
        next_sequence,
        price,
        revision,
        &now.to_rfc3339(),
    )?;
    let next_end = next_calendar_month_at(current_end, u32::try_from(anchor_day).unwrap_or(31))?;
    set_period_bounds_tx(
        conn,
        contract_id,
        next_sequence,
        current_end,
        next_end,
        &now.to_rfc3339(),
    )?;
    if !capture_hold_tx(conn, contract_id, next_sequence, &now.to_rfc3339())? {
        enter_recovery_tx(conn, contract_id, &product_kind, now)?;
        return Ok(false);
    }
    conn.execute(
        "UPDATE market_recurring_contracts
         SET status = 'active', current_period_sequence = ?2,
             current_period_start = ?3, current_period_end = ?4,
             renewal_status = 'funding_required', recovery_deadline = NULL,
             desired_control_state = 'active', control_error = NULL,
             version = version + 1, updated_at = ?5
         WHERE id = ?1",
        params![
            contract_id,
            next_sequence,
            current_end.to_rfc3339(),
            next_end.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(map_db("advance recurring period"))?;
    if policy == RENEWAL_AUTOMATIC
        && reserve_next_for_contract_tx(conn, contract_id, "automatic_renewal", &now.to_rfc3339())?
    {
        conn.execute(
            "UPDATE market_recurring_contracts
             SET renewal_status = 'funded', version = version + 1, updated_at = ?2
             WHERE id = ?1",
            params![contract_id, now.to_rfc3339()],
        )
        .map_err(map_db("fund following recurring period"))?;
    }
    Ok(true)
}

fn advance_trial_tx(
    conn: &Connection,
    contract_id: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let trial = conn
        .query_row(
            "SELECT product_kind, product_ref, service_ref,
                    trial_seconds_remaining, trial_last_evaluated_at, activated_at
             FROM market_recurring_contracts
             WHERE id = ?1 AND status = 'trial'",
            params![contract_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read recurring trial for reconciliation"))?;
    let Some((product_kind, product_ref, service_ref, remaining, last_evaluated, activated_at)) =
        trial
    else {
        return Ok(());
    };
    let last_evaluated = last_evaluated.or(activated_at).ok_or_else(|| {
        AppError::Internal("active recurring trial has no evaluation anchor".into())
    })?;
    let last_evaluated = parse_time(&last_evaluated)?;
    let evaluation_end = now.max(last_evaluated);
    let elapsed_seconds = (evaluation_end - last_evaluated).num_seconds().max(0);
    let (health_state, _health_reason) = crate::market_billing::effective_service_health_tx(
        conn,
        &product_kind,
        &product_ref,
        &service_ref,
        last_evaluated,
        evaluation_end,
    )?;
    let consumed_seconds = if health_state == "healthy" {
        elapsed_seconds.min(remaining.max(0))
    } else {
        0
    };
    let next_remaining = remaining.saturating_sub(consumed_seconds);
    let projected_end = evaluation_end + Duration::seconds(next_remaining);
    let now_text = now.to_rfc3339();
    conn.execute(
        "UPDATE market_recurring_contracts
         SET trial_seconds_remaining = ?2, trial_last_evaluated_at = ?3,
             trial_health_state = ?4, trial_ends_at = ?5, trial_deadline_at = ?5,
             version = version + 1, updated_at = ?6
         WHERE id = ?1 AND status = 'trial'",
        params![
            contract_id,
            next_remaining,
            evaluation_end.to_rfc3339(),
            health_state,
            projected_end.to_rfc3339(),
            now_text,
        ],
    )
    .map_err(map_db("advance recurring healthy-service trial"))?;
    if next_remaining == 0 {
        let paid_period_start = last_evaluated + Duration::seconds(remaining.max(0));
        if !initialize_paid_service_tx(conn, contract_id, paid_period_start, &now_text)? {
            end_contract_tx(conn, contract_id, "initial_payment_unavailable", &now_text)?;
        }
    }
    Ok(())
}

pub(crate) fn reconcile_tx(tx: &Transaction<'_>, now: DateTime<Utc>) -> Result<(), AppError> {
    let now_text = now.to_rfc3339();
    let trials = tx
        .prepare(
            "SELECT id FROM market_recurring_contracts
             WHERE status = 'trial'
             ORDER BY COALESCE(trial_last_evaluated_at, activated_at, created_at), id
             LIMIT ?1",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![MAX_RECONCILE_ROWS], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("load recurring trials due"))?;
    for contract_id in trials {
        advance_trial_tx(tx, &contract_id, now)?;
    }

    let active_ids = tx
        .prepare(
            "SELECT id FROM market_recurring_contracts
             WHERE status = 'active' AND current_period_end <= ?1
             ORDER BY current_period_end, renewal_priority DESC, id LIMIT ?2",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![now_text, MAX_RECONCILE_ROWS], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("load recurring periods due"))?;
    for contract_id in active_ids {
        for _ in 0..MAX_RECONCILE_ADVANCES {
            if !advance_paid_period_tx(tx, &contract_id, now)? {
                break;
            }
            let still_due = tx
                .query_row(
                    "SELECT status = 'active' AND current_period_end <= ?2
                     FROM market_recurring_contracts WHERE id = ?1",
                    params![contract_id, now_text],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(map_db("check recurring catch-up"))?;
            if !still_due {
                break;
            }
        }
    }

    let recovery_ids = tx
        .prepare(
            "SELECT id, recovery_deadline FROM market_recurring_contracts
             WHERE status = 'recovery'
             ORDER BY recovery_deadline, id LIMIT ?1",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![MAX_RECONCILE_ROWS], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("load recurring recovery contracts"))?;
    for (contract_id, deadline) in recovery_ids {
        let next_sequence = contract_next_sequence_tx(tx, &contract_id)?;
        let funded = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM market_recurring_holds
                    WHERE contract_id = ?1 AND period_sequence = ?2 AND status = 'active'
                 )",
                params![contract_id, next_sequence],
                |row| row.get::<_, i64>(0),
            )
            .map_err(map_db("check recovered recurring funding"))?
            != 0;
        if funded {
            resume_funded_recovery_contract_tx(tx, &contract_id, &now_text)?;
        } else if parse_time(&deadline)? <= now {
            end_contract_tx(tx, &contract_id, "renewal_recovery_expired", &now_text)?;
        }
    }
    Ok(())
}

fn conn_set_status_active_for_retry(
    conn: &Connection,
    contract_id: &str,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE market_recurring_contracts SET status = 'active', updated_at = ?2
         WHERE id = ?1 AND status = 'recovery'",
        params![contract_id, now],
    )
    .map_err(map_db("retry funded recurring renewal"))?;
    Ok(())
}

pub(crate) fn pending_control_actions_tx(
    tx: &Transaction<'_>,
) -> Result<Vec<BillingAction>, AppError> {
    tx.prepare(
        "SELECT id, desired_control_state, product_kind, product_ref, service_ref,
                COALESCE(control_error, 'recurring_contract_ended')
         FROM market_recurring_contracts
         WHERE desired_control_state != applied_control_state
           AND desired_control_state IN ('active', 'suspended', 'terminated')
         ORDER BY updated_at, id LIMIT ?1",
    )
    .and_then(|mut statement| {
        statement
            .query_map(params![MAX_RECONCILE_ROWS], |row| {
                let desired: String = row.get(1)?;
                Ok(BillingAction {
                    contract_id: row.get(0)?,
                    kind: match desired.as_str() {
                        "active" => BillingActionKind::Resume,
                        "terminated" => BillingActionKind::Terminate,
                        _ => BillingActionKind::Suspend,
                    },
                    product_kind: row.get(2)?,
                    product_ref: row.get(3)?,
                    service_ref: row.get(4)?,
                    reason: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(map_db("load recurring billing controls"))
}

pub(crate) fn contract_view_for_product_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
) -> Result<Option<RecurringContractView>, AppError> {
    let id = conn
        .query_row(
            "SELECT id FROM market_recurring_contracts
             WHERE product_kind = ?1 AND product_ref = ?2
             ORDER BY created_at DESC LIMIT 1",
            params![product_kind, product_ref],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("find recurring contract view"))?;
    id.map(|id| contract_view_tx(conn, &id)).transpose()
}

fn contract_view_tx(
    conn: &Connection,
    contract_id: &str,
) -> Result<RecurringContractView, AppError> {
    conn.query_row(
        "SELECT contract.id, contract.product_kind, contract.product_ref,
                contract.pricing_model, contract.billing_interval,
                contract.cycle_price_minor, contract.currency,
                contract.supplier_user_id, contract.supplier_email, contract.status,
                contract.renewal_policy, contract.renewal_status,
                contract.auto_renew_max_price_minor, contract.renewal_priority,
                contract.current_period_start, contract.current_period_end,
                contract.cancel_at_period_end, contract.recovery_deadline,
                contract.trial_ends_at,
                COALESCE((
                    SELECT SUM(hold.amount_units)
                    FROM market_recurring_holds hold
                    WHERE hold.contract_id = contract.id AND hold.status = 'active'
                      AND hold.period_sequence > contract.current_period_sequence
                      AND NOT (contract.current_period_sequence = 0 AND hold.period_sequence = 1)
                ), 0)
         FROM market_recurring_contracts contract WHERE contract.id = ?1",
        params![contract_id],
        |row| {
            let current_period_end = row.get::<_, Option<String>>(15)?;
            Ok(RecurringContractView {
                id: row.get(0)?,
                product_kind: row.get(1)?,
                product_ref: row.get(2)?,
                pricing_model: row.get(3)?,
                billing_interval: row.get(4)?,
                cycle_price_minor: row.get(5)?,
                currency: row.get(6)?,
                supplier_user_id: row.get(7)?,
                supplier_email: row.get(8)?,
                status: row.get(9)?,
                renewal_policy: row.get(10)?,
                renewal_status: row.get(11)?,
                auto_renew_max_price_minor: row.get(12)?,
                renewal_priority: row.get(13)?,
                current_period_start: row.get(14)?,
                current_period_end: current_period_end.clone(),
                next_renewal_at: current_period_end,
                cancel_at_period_end: row.get(16)?,
                recovery_deadline: row.get(17)?,
                trial_ends_at: row.get(18)?,
                next_period_held_minor: floor_minor(row.get::<_, i64>(19)?),
            })
        },
    )
    .map_err(map_db("read recurring contract view"))
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated user session required".into()))
}

impl AppStore {
    async fn market_recurring_update_renewal(
        &self,
        session: &AuthSession,
        contract_id: &str,
        policy: &str,
        max_price: Option<i64>,
        priority: i64,
    ) -> Result<RecurringContractView, AppError> {
        let policy = validate_renewal_policy(policy)?;
        if priority < 0 {
            return Err(AppError::BadRequest(
                "renewalPriority must not be negative".into(),
            ));
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin recurring renewal update"))?;
        let (buyer_user_id, price, status, current_sequence) = tx
            .query_row(
                "SELECT buyer_user_id, cycle_price_minor, status, current_period_sequence
                 FROM market_recurring_contracts WHERE id = ?1",
                params![contract_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read recurring renewal owner"))?
            .ok_or_else(|| AppError::NotFound("recurring contract not found".into()))?;
        if buyer_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "recurring contract belongs to another account".into(),
            ));
        }
        if matches!(status.as_str(), STATUS_ENDED | STATUS_ACTIVATION_FAILED) {
            return Err(AppError::Conflict(
                "ended recurring contract cannot change renewal settings".into(),
            ));
        }
        let max_price = max_price.unwrap_or(price);
        if max_price < price {
            return Err(AppError::BadRequest(
                "autoRenewMaxPriceMinor cannot be below the current monthly price".into(),
            ));
        }
        tx.execute(
            "UPDATE market_recurring_contracts
             SET renewal_policy = ?2, auto_renew_max_price_minor = ?3,
                 renewal_priority = ?4, cancel_at_period_end = 0,
                 renewal_status = 'funding_required', version = version + 1,
                 updated_at = ?5 WHERE id = ?1",
            params![contract_id, policy, max_price, priority, now],
        )
        .map_err(map_db("update recurring renewal policy"))?;
        if policy == RENEWAL_AUTOMATIC {
            if !reserve_next_for_contract_tx(&tx, contract_id, "automatic_renewal", &now)? {
                let required_topup = required_topup_minor_for_contract_tx(&tx, contract_id)?;
                if required_topup == 0 {
                    return Err(AppError::Conflict(
                        "this recurring contract is not eligible for an automatic renewal reservation"
                            .into(),
                    ));
                }
                return Err(AppError::coded_conflict(
                    crate::market_access::ERROR_MARKET_PREPAID_REQUIRED,
                    "one full monthly payment must be available to enable automatic renewal",
                    serde_json::json!({
                        "contractId": contract_id,
                        "cyclePriceMinor": price,
                        "requiredTopupMinor": required_topup,
                    }),
                ));
            }
            if !resume_funded_recovery_contract_tx(&tx, contract_id, &now)? {
                tx.execute(
                    "UPDATE market_recurring_contracts
                     SET renewal_status = 'funded', version = version + 1, updated_at = ?2
                     WHERE id = ?1 AND status NOT IN ('recovery', 'ended', 'activation_failed')",
                    params![contract_id, now],
                )
                .map_err(map_db("mark automatic renewal funded"))?;
            }
        } else {
            let holds = tx
                .prepare(
                    "SELECT id FROM market_recurring_holds
                     WHERE contract_id = ?1 AND status = 'active'
                       AND purpose = 'automatic_renewal'
                       AND period_sequence > ?2",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map(params![contract_id, current_sequence.max(1)], |row| {
                            row.get::<_, String>(0)
                        })?
                        .collect::<Result<Vec<_>, _>>()
                })
                .map_err(map_db("load disabled automatic renewal holds"))?;
            for hold_id in holds {
                release_hold_tx(&tx, &hold_id, "automatic_renewal_disabled", &now)?;
            }
        }
        let view = contract_view_tx(&tx, contract_id)?;
        tx.commit()
            .map_err(map_db("commit recurring renewal update"))?;
        Ok(view)
    }

    async fn market_recurring_reserve_next(
        &self,
        session: &AuthSession,
        contract_id: &str,
    ) -> Result<RecurringContractView, AppError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin manual recurring reservation"))?;
        let (buyer_user_id, price) = tx
            .query_row(
                "SELECT buyer_user_id, cycle_price_minor
                 FROM market_recurring_contracts WHERE id = ?1
                   AND status NOT IN ('ended', 'activation_failed')",
                params![contract_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(map_db("read manual recurring reservation owner"))?
            .ok_or_else(|| AppError::NotFound("active recurring contract not found".into()))?;
        if buyer_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "recurring contract belongs to another account".into(),
            ));
        }
        if !reserve_next_for_contract_tx(&tx, contract_id, "manual_renewal", &now)? {
            let required_topup = required_topup_minor_for_contract_tx(&tx, contract_id)?;
            if required_topup == 0 {
                return Err(AppError::Conflict(
                    "this recurring contract is not eligible for a renewal reservation".into(),
                ));
            }
            return Err(AppError::coded_conflict(
                crate::market_access::ERROR_MARKET_PREPAID_REQUIRED,
                "one full monthly payment is required to reserve the next period",
                serde_json::json!({
                    "contractId": contract_id,
                    "cyclePriceMinor": price,
                    "requiredTopupMinor": required_topup,
                }),
            ));
        }
        if !resume_funded_recovery_contract_tx(&tx, contract_id, &now)? {
            tx.execute(
                "UPDATE market_recurring_contracts
                 SET renewal_status = 'funded', cancel_at_period_end = 0,
                     version = version + 1, updated_at = ?2
                 WHERE id = ?1 AND status NOT IN ('recovery', 'ended', 'activation_failed')",
                params![contract_id, now],
            )
            .map_err(map_db("mark manual recurring reservation funded"))?;
        }
        let view = contract_view_tx(&tx, contract_id)?;
        tx.commit()
            .map_err(map_db("commit manual recurring reservation"))?;
        Ok(view)
    }

    async fn market_recurring_cancel(
        &self,
        session: &AuthSession,
        contract_id: &str,
        mode: &str,
    ) -> Result<RecurringContractView, AppError> {
        if !matches!(mode, "period_end" | "immediate") {
            return Err(AppError::BadRequest(
                "mode must be period_end or immediate".into(),
            ));
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin recurring cancellation"))?;
        let (buyer_user_id, status, current_sequence, current_period_end) = tx
            .query_row(
                "SELECT buyer_user_id, status, current_period_sequence, current_period_end
                 FROM market_recurring_contracts WHERE id = ?1",
                params![contract_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read recurring cancellation owner"))?
            .ok_or_else(|| AppError::NotFound("recurring contract not found".into()))?;
        if buyer_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "recurring contract belongs to another account".into(),
            ));
        }
        if !matches!(status.as_str(), STATUS_ENDED | STATUS_ACTIVATION_FAILED) {
            let now_dt = parse_time(&now)?;
            let period_has_ended = current_period_end
                .as_deref()
                .map(parse_time)
                .transpose()?
                .is_some_and(|period_end| period_end <= now_dt);
            if mode == "immediate"
                || current_sequence == 0
                || status != STATUS_ACTIVE
                || period_has_ended
            {
                let reason = if mode == "immediate" {
                    "buyer_cancelled_immediately"
                } else if current_sequence == 0 {
                    "buyer_cancelled_before_first_period"
                } else if status == STATUS_RECOVERY || period_has_ended {
                    "buyer_cancelled_during_renewal_recovery"
                } else {
                    "buyer_cancelled_while_suspended"
                };
                end_contract_tx(&tx, contract_id, reason, &now)?;
            } else {
                release_holds_from_sequence_tx(
                    &tx,
                    contract_id,
                    current_sequence + 1,
                    "cancel_at_period_end",
                    &now,
                )?;
                tx.execute(
                    "UPDATE market_recurring_contracts
                     SET cancel_at_period_end = 1,
                         renewal_status = 'cancel_at_period_end',
                         version = version + 1, updated_at = ?2
                     WHERE id = ?1",
                    params![contract_id, now],
                )
                .map_err(map_db("schedule recurring cancellation"))?;
            }
        }
        let view = contract_view_tx(&tx, contract_id)?;
        tx.commit()
            .map_err(map_db("commit recurring cancellation"))?;
        Ok(view)
    }
}

async fn update_renewal(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(contract_id): Path<String>,
    Json(input): Json<UpdateRenewalRequest>,
) -> Result<Json<RecurringContractView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .market_recurring_update_renewal(
                &session,
                &contract_id,
                &input.renewal_policy,
                input.auto_renew_max_price_minor,
                input.renewal_priority,
            )
            .await?,
    ))
}

async fn reserve_next_period(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(contract_id): Path<String>,
) -> Result<Json<RecurringContractView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .market_recurring_reserve_next(&session, &contract_id)
            .await?,
    ))
}

async fn cancel_recurring(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(contract_id): Path<String>,
    Json(input): Json<CancelRecurringRequest>,
) -> Result<Json<RecurringContractView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .market_recurring_cancel(&session, &contract_id, &input.mode)
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn test_session(label: &str) -> AuthSession {
        AuthSession {
            session_id: format!("{label}-session"),
            user_id: format!("buyer-{label}"),
            email: format!("buyer-{label}@example.com"),
            auth_source_kind: "test".into(),
            auth_source_id: "test".into(),
            access_token_hash: "test".into(),
            refresh_token_hash: "test".into(),
            access_expires_at: Utc::now() + Duration::hours(1),
            refresh_expires_at: Utc::now() + Duration::days(1),
            created_at: Utc::now(),
            last_used_at: Utc::now(),
        }
    }

    async fn create_test_contract(
        store: &AppStore,
        label: &str,
        renewal_policy: &str,
        funded_months: i64,
        starts_at: DateTime<Utc>,
    ) -> (String, String) {
        let price = 1_000_i64;
        let now = starts_at.to_rfc3339();
        let conn = store.conn.lock().await;
        let tx = conn.transaction().expect("begin recurring test setup");
        let account_id = crate::market_billing::ensure_market_prepaid_account_tx(
            &tx,
            &format!("buyer-{label}"),
            &format!("buyer-{label}@example.com"),
            &format!("supplier-{label}"),
            &format!("supplier-{label}@example.com"),
            crate::market_billing::MARKET_CURRENCY,
            &now,
        )
        .expect("create recurring test prepaid account");
        crate::market_billing::credit_market_prepaid_funding_tx(
            &tx,
            &account_id,
            money_units(price * funded_months).expect("test funding units"),
            &format!("initial-funding-{label}"),
            None,
            &now,
        )
        .expect("fund recurring test account");
        let contract_id = prepare_contract_tx(
            &tx,
            PrepareRecurringContractInput {
                product_kind: "share",
                product_ref: &format!("subscription-{label}"),
                activation_ref: &format!("activation-{label}"),
                service_ref: &format!("share-{label}"),
                service_label: &format!("Share {label}"),
                buyer_user_id: &format!("buyer-{label}"),
                buyer_email: &format!("buyer-{label}@example.com"),
                supplier_user_id: &format!("supplier-{label}"),
                supplier_email: &format!("supplier-{label}@example.com"),
                currency: crate::market_billing::MARKET_CURRENCY,
                cycle_price_minor: price,
                offer_revision: 1,
                renewal_policy,
                auto_renew_max_price_minor: Some(price),
                renewal_priority: 0,
                trial_allowance_seconds: 0,
            },
            &now,
        )
        .expect("prepare recurring test contract");
        activate_contract_tx(
            &tx,
            "share",
            &format!("activation-{label}"),
            &format!("subscription-{label}"),
            &format!("share-{label}"),
            &format!("Share {label}"),
            starts_at,
            &now,
        )
        .expect("activate recurring test contract")
        .expect("prepared recurring contract");
        tx.commit().expect("commit recurring test setup");
        (account_id, contract_id)
    }

    async fn reconcile_at(store: &AppStore, now: DateTime<Utc>) {
        let conn = store.conn.lock().await;
        let tx = conn
            .transaction()
            .expect("begin recurring test reconciliation");
        reconcile_tx(&tx, now).expect("reconcile recurring test contract");
        tx.commit().expect("commit recurring test reconciliation");
    }

    #[test]
    fn calendar_month_preserves_original_anchor_after_short_month() {
        let january = at("2025-01-31T12:34:56Z");
        let february = next_calendar_month_at(january, 31).expect("February boundary");
        let march = next_calendar_month_at(february, 31).expect("March boundary");
        assert_eq!(february, at("2025-02-28T12:34:56Z"));
        assert_eq!(march, at("2025-03-31T12:34:56Z"));
    }

    #[test]
    fn calendar_month_handles_leap_year_and_year_rollover() {
        assert_eq!(
            next_calendar_month_at(at("2024-01-31T23:59:59Z"), 31).unwrap(),
            at("2024-02-29T23:59:59Z")
        );
        let next = next_calendar_month_at(at("2025-12-30T01:02:03.123Z"), 30).unwrap();
        assert_eq!(next, at("2026-01-30T01:02:03.123Z"));
        assert_eq!(next.nanosecond(), 123_000_000);
    }

    #[tokio::test]
    async fn activated_contract_replay_rejects_binding_drift() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let starts_at = at("2025-01-01T12:00:00Z");
        let now = starts_at.to_rfc3339();
        let conn = store.conn.lock().await;
        let tx = conn.transaction().expect("begin activation binding test");
        let account_id = crate::market_billing::ensure_market_prepaid_account_tx(
            &tx,
            "buyer-binding",
            "buyer-binding@example.com",
            "supplier-binding",
            "supplier-binding@example.com",
            crate::market_billing::MARKET_CURRENCY,
            &now,
        )
        .expect("create activation binding account");
        crate::market_billing::credit_market_prepaid_funding_tx(
            &tx,
            &account_id,
            money_units(1_000).unwrap(),
            "activation-binding-funding",
            None,
            &now,
        )
        .expect("fund activation binding account");
        let contract_id = prepare_contract_tx(
            &tx,
            PrepareRecurringContractInput {
                product_kind: "client_host",
                product_ref: "pending-job-binding",
                activation_ref: "activation-binding",
                service_ref: "host-binding",
                service_label: "pending.example.com",
                buyer_user_id: "buyer-binding",
                buyer_email: "buyer-binding@example.com",
                supplier_user_id: "supplier-binding",
                supplier_email: "supplier-binding@example.com",
                currency: crate::market_billing::MARKET_CURRENCY,
                cycle_price_minor: 1_000,
                offer_revision: 1,
                renewal_policy: RENEWAL_MANUAL,
                auto_renew_max_price_minor: Some(1_000),
                renewal_priority: 0,
                trial_allowance_seconds: 0,
            },
            &now,
        )
        .expect("prepare activation binding contract");

        let activated = activate_contract_tx(
            &tx,
            "client_host",
            "activation-binding",
            "installation-binding",
            "installation-binding",
            "client.binding.example.com",
            starts_at,
            &now,
        )
        .expect("activate bound contract");
        assert_eq!(activated.as_deref(), Some(contract_id.as_str()));
        let replayed = activate_contract_tx(
            &tx,
            "client_host",
            "activation-binding",
            "installation-binding",
            "installation-binding",
            "client.binding.example.com",
            starts_at + Duration::minutes(1),
            &(starts_at + Duration::minutes(1)).to_rfc3339(),
        )
        .expect("replay identical activation");
        assert_eq!(replayed.as_deref(), Some(contract_id.as_str()));

        for (product_ref, service_ref, service_label) in [
            (
                "other-installation",
                "installation-binding",
                "client.binding.example.com",
            ),
            (
                "installation-binding",
                "other-service",
                "client.binding.example.com",
            ),
            (
                "installation-binding",
                "installation-binding",
                "other-label.example.com",
            ),
        ] {
            let error = activate_contract_tx(
                &tx,
                "client_host",
                "activation-binding",
                product_ref,
                service_ref,
                service_label,
                starts_at + Duration::minutes(2),
                &(starts_at + Duration::minutes(2)).to_rfc3339(),
            )
            .expect_err("an activated binding must be immutable");
            assert!(matches!(error, AppError::Conflict(_)));
        }
        let stored: (String, String, String, String) = tx
            .query_row(
                "SELECT status, product_ref, service_ref, service_label
                 FROM market_recurring_contracts WHERE id = ?1",
                params![contract_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read immutable activation binding");
        assert_eq!(
            stored,
            (
                STATUS_ACTIVE.into(),
                "installation-binding".into(),
                "installation-binding".into(),
                "client.binding.example.com".into(),
            )
        );
        tx.commit().expect("commit activation binding test");
    }

    #[tokio::test]
    async fn prepare_replay_requires_the_complete_original_request() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let starts_at = at("2025-01-01T12:00:00Z");
        let (_account_id, contract_id) =
            create_test_contract(&store, "prepare-replay", RENEWAL_MANUAL, 1, starts_at).await;
        let now = (starts_at + Duration::minutes(1)).to_rfc3339();
        let conn = store.conn.lock().await;
        let replay = |service_label: &str, renewal_priority: i64| {
            prepare_contract_tx(
                &conn,
                PrepareRecurringContractInput {
                    product_kind: "share",
                    product_ref: "subscription-prepare-replay",
                    activation_ref: "activation-prepare-replay",
                    service_ref: "share-prepare-replay",
                    service_label,
                    buyer_user_id: "buyer-prepare-replay",
                    buyer_email: "BUYER-PREPARE-REPLAY@example.com",
                    supplier_user_id: "supplier-prepare-replay",
                    supplier_email: "SUPPLIER-PREPARE-REPLAY@example.com",
                    currency: crate::market_billing::MARKET_CURRENCY,
                    cycle_price_minor: 1_000,
                    offer_revision: 1,
                    renewal_policy: RENEWAL_MANUAL,
                    auto_renew_max_price_minor: Some(1_000),
                    renewal_priority,
                    trial_allowance_seconds: 0,
                },
                &now,
            )
        };

        assert_eq!(
            replay("Share prepare-replay", 0).expect("replay identical preparation"),
            contract_id
        );
        for result in [
            replay("Different service", 0),
            replay("Share prepare-replay", 1),
        ] {
            assert!(matches!(result, Err(AppError::Conflict(_))));
        }
    }

    #[tokio::test]
    async fn topup_funds_the_next_automatic_calendar_month() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let starts_at = at("2025-01-31T12:00:00Z");
        let (account_id, contract_id) =
            create_test_contract(&store, "automatic-topup", RENEWAL_AUTOMATIC, 2, starts_at).await;

        reconcile_at(&store, at("2025-02-28T12:00:00Z")).await;
        {
            let conn = store.conn.lock().await;
            let state: (i64, String, String, i64) = conn
                .query_row(
                    "SELECT current_period_sequence, current_period_end, renewal_status,
                            (SELECT COUNT(*) FROM market_recurring_holds hold
                             WHERE hold.contract_id = contract.id
                               AND hold.period_sequence = 3 AND hold.status = 'active')
                     FROM market_recurring_contracts contract WHERE contract.id = ?1",
                    params![contract_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read unfunded third period");
            assert_eq!(state.0, 2);
            assert_eq!(parse_time(&state.1).unwrap(), at("2025-03-31T12:00:00Z"));
            assert_eq!(state.2, "funding_required");
            assert_eq!(state.3, 0);
        }

        let topup_at = at("2025-03-01T00:00:00Z").to_rfc3339();
        let conn = store.conn.lock().await;
        let tx = conn.transaction().expect("begin automatic renewal topup");
        let (_, actions) = crate::market_billing::credit_market_prepaid_funding_tx(
            &tx,
            &account_id,
            money_units(1_000).unwrap(),
            "automatic-renewal-topup",
            None,
            &topup_at,
        )
        .expect("credit automatic renewal topup");
        assert!(actions.is_empty());
        let state: (i64, String, String, i64, i64) = tx
            .query_row(
                "SELECT contract.current_period_sequence, contract.current_period_end,
                        contract.renewal_status,
                        COALESCE(SUM(CASE WHEN hold.period_sequence = 3
                                           AND hold.status = 'active' THEN 1 ELSE 0 END), 0),
                        account.held_balance_units
                 FROM market_recurring_contracts contract
                 JOIN market_prepaid_accounts account ON account.id = contract.prepaid_account_id
                 LEFT JOIN market_recurring_holds hold ON hold.contract_id = contract.id
                 WHERE contract.id = ?1 GROUP BY contract.id",
                params![contract_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("read funded third period");
        assert_eq!(state.0, 2);
        assert_eq!(parse_time(&state.1).unwrap(), at("2025-03-31T12:00:00Z"));
        assert_eq!(state.2, "funded");
        assert_eq!(state.3, 1);
        assert_eq!(state.4, money_units(1_000).unwrap());
        tx.commit().expect("commit automatic renewal topup");
    }

    #[tokio::test]
    async fn topup_advances_recovery_and_returns_only_a_resume_action() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let starts_at = at("2025-01-31T12:00:00Z");
        let (account_id, contract_id) =
            create_test_contract(&store, "recovery-topup", RENEWAL_AUTOMATIC, 2, starts_at).await;
        reconcile_at(&store, at("2025-02-28T12:00:00Z")).await;
        reconcile_at(&store, at("2025-03-31T12:00:00Z")).await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_recurring_contracts
                 SET applied_control_state = 'suspended' WHERE id = ?1",
                params![contract_id],
            )
            .expect("apply test recovery suspension");
        }

        let topup_at = at("2025-04-01T00:00:00Z").to_rfc3339();
        let conn = store.conn.lock().await;
        let tx = conn.transaction().expect("begin recovery topup");
        let (_, actions) = crate::market_billing::credit_market_prepaid_funding_tx(
            &tx,
            &account_id,
            money_units(1_000).unwrap(),
            "recovery-renewal-topup",
            None,
            &topup_at,
        )
        .expect("credit recovery topup");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].contract_id, contract_id);
        assert_eq!(actions[0].kind, BillingActionKind::Resume);
        assert!(
            actions
                .iter()
                .all(|action| action.kind != BillingActionKind::Suspend)
        );
        let state: (String, i64, String, String, String, i64, i64) = tx
            .query_row(
                "SELECT contract.status, contract.current_period_sequence,
                        contract.current_period_start, contract.current_period_end,
                        contract.renewal_status, account.posted_balance_units,
                        account.held_balance_units
                 FROM market_recurring_contracts contract
                 JOIN market_prepaid_accounts account ON account.id = contract.prepaid_account_id
                 WHERE contract.id = ?1",
                params![contract_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .expect("read resumed recurring contract");
        assert_eq!(state.0, STATUS_ACTIVE);
        assert_eq!(state.1, 3);
        assert_eq!(parse_time(&state.2).unwrap(), at("2025-03-31T12:00:00Z"));
        assert_eq!(parse_time(&state.3).unwrap(), at("2025-04-30T12:00:00Z"));
        assert_eq!(state.4, "funding_required");
        assert_eq!(state.5, 0);
        assert_eq!(state.6, 0);
        tx.commit().expect("commit recovery topup");
    }

    #[tokio::test]
    async fn recovery_does_not_resume_until_every_overdue_month_is_funded() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let starts_at = at("2025-01-01T00:00:00Z");
        let (account_id, contract_id) = create_test_contract(
            &store,
            "multi-month-recovery",
            RENEWAL_AUTOMATIC,
            2,
            starts_at,
        )
        .await;
        reconcile_at(&store, at("2025-04-01T00:00:00Z")).await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_recurring_contracts
                 SET applied_control_state = 'suspended' WHERE id = ?1",
                params![contract_id],
            )
            .expect("apply multi-month recovery suspension");
        }

        let first_topup_at = at("2025-04-01T00:01:00Z").to_rfc3339();
        {
            let conn = store.conn.lock().await;
            let tx = conn.transaction().expect("begin first catch-up topup");
            let (_, actions) = crate::market_billing::credit_market_prepaid_funding_tx(
                &tx,
                &account_id,
                money_units(1_000).unwrap(),
                "first-catch-up-topup",
                None,
                &first_topup_at,
            )
            .expect("credit first catch-up topup");
            assert!(actions.iter().all(|action| {
                action.contract_id != contract_id || action.kind != BillingActionKind::Resume
            }));
            let state: (String, i64, String, String, String) = tx
                .query_row(
                    "SELECT status, current_period_sequence, current_period_end,
                            renewal_status, desired_control_state
                     FROM market_recurring_contracts WHERE id = ?1",
                    params![contract_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .expect("read partially caught-up recovery");
            assert_eq!(state.0, STATUS_RECOVERY);
            assert_eq!(state.1, 3);
            assert_eq!(parse_time(&state.2).unwrap(), at("2025-04-01T00:00:00Z"));
            assert_eq!(state.3, "funding_required");
            assert_eq!(state.4, "suspended");
            tx.commit().expect("commit first catch-up topup");
        }

        let second_topup_at = at("2025-04-01T00:02:00Z").to_rfc3339();
        let conn = store.conn.lock().await;
        let tx = conn.transaction().expect("begin final catch-up topup");
        let (_, actions) = crate::market_billing::credit_market_prepaid_funding_tx(
            &tx,
            &account_id,
            money_units(1_000).unwrap(),
            "second-catch-up-topup",
            None,
            &second_topup_at,
        )
        .expect("credit final catch-up topup");
        assert!(actions.iter().any(|action| {
            action.contract_id == contract_id && action.kind == BillingActionKind::Resume
        }));
        let state: (String, i64, String, String) = tx
            .query_row(
                "SELECT status, current_period_sequence, current_period_end,
                        desired_control_state
                 FROM market_recurring_contracts WHERE id = ?1",
                params![contract_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read fully caught-up recovery");
        assert_eq!(state.0, STATUS_ACTIVE);
        assert_eq!(state.1, 4);
        assert_eq!(parse_time(&state.2).unwrap(), at("2025-05-01T00:00:00Z"));
        assert_eq!(state.3, "active");
        tx.commit().expect("commit final catch-up topup");
    }

    #[tokio::test]
    async fn supplier_termination_refunds_exact_unused_period_once_and_releases_renewal() {
        let starts_at = at("2025-01-01T00:00:00Z");
        for (label, terminated_at, expected_minor, expected_period_status) in [
            (
                "supplier-partial-refund",
                at("2025-01-16T12:00:00Z"),
                500_i64,
                "partially_refunded",
            ),
            ("supplier-full-refund", starts_at, 1_000_i64, "refunded"),
        ] {
            let store = AppStore::new_in_memory_for_tests().expect("test store");
            let (account_id, contract_id) =
                create_test_contract(&store, label, RENEWAL_AUTOMATIC, 2, starts_at).await;
            let now = terminated_at.to_rfc3339();
            let conn = store.conn.lock().await;
            let tx = conn.transaction().expect("begin supplier recurring refund");
            let refunded = supplier_terminate_and_refund_tx(
                &tx,
                "share",
                &format!("subscription-{label}"),
                "supplier_stopped_service",
                &now,
            )
            .expect("terminate and refund recurring contract");
            assert_eq!(refunded, Some(expected_minor));

            let state: (String, String, i64, i64, i64, i64, i64, i64, i64) = tx
                .query_row(
                    "SELECT contract.status, period.status, period.refunded_units,
                            account.posted_balance_units, account.held_balance_units,
                            (SELECT COUNT(*) FROM market_recurring_holds hold
                             WHERE hold.contract_id = contract.id AND hold.status = 'active'),
                            (SELECT COUNT(*) FROM market_recurring_holds hold
                             WHERE hold.contract_id = contract.id
                               AND hold.period_sequence = 2 AND hold.status = 'released'),
                            (SELECT COUNT(*) FROM market_prepaid_ledger_entries entry
                             WHERE entry.account_id = account.id
                               AND entry.source_kind = 'recurring_refund'),
                            (SELECT COALESCE(SUM(entry.amount_units), 0)
                             FROM market_prepaid_ledger_entries entry
                             WHERE entry.account_id = account.id
                               AND entry.source_kind = 'recurring_refund')
                     FROM market_recurring_contracts contract
                     JOIN market_recurring_periods period
                       ON period.contract_id = contract.id AND period.sequence = 1
                     JOIN market_prepaid_accounts account
                       ON account.id = contract.prepaid_account_id
                     WHERE contract.id = ?1",
                    params![contract_id],
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
                .expect("read recurring supplier refund state");
            assert_eq!(state.0, STATUS_ENDED);
            assert_eq!(state.1, expected_period_status);
            assert_eq!(state.2, money_units(expected_minor).unwrap());
            assert_eq!(
                state.3,
                money_units(1_000 + expected_minor).expect("expected posted balance")
            );
            assert_eq!(state.4, 0);
            assert_eq!(state.5, 0);
            assert_eq!(state.6, 1);
            assert_eq!(state.7, 1);
            assert_eq!(state.8, money_units(expected_minor).unwrap());

            assert_eq!(
                supplier_terminate_and_refund_tx(
                    &tx,
                    "share",
                    &format!("subscription-{label}"),
                    "supplier_stopped_service",
                    &now,
                )
                .expect("replay recurring supplier termination"),
                None
            );
            let replay_ledger_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM market_prepaid_ledger_entries
                     WHERE account_id = ?1 AND source_kind = 'recurring_refund'",
                    params![account_id],
                    |row| row.get(0),
                )
                .expect("count replayed recurring refund entries");
            assert_eq!(replay_ledger_count, 1);
            tx.commit().expect("commit supplier recurring refund");
        }
    }

    #[tokio::test]
    async fn buyer_period_end_cancellation_releases_only_the_future_period() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let starts_at = at("2025-01-31T12:00:00Z");
        let (account_id, contract_id) =
            create_test_contract(&store, "period-end", RENEWAL_AUTOMATIC, 2, starts_at).await;
        let cancelled_at = at("2025-02-01T00:00:00Z").to_rfc3339();
        {
            let conn = store.conn.lock().await;
            let tx = conn.transaction().expect("begin period-end cancellation");
            assert!(
                cancel_at_period_end_for_product_tx(
                    &tx,
                    "share",
                    "subscription-period-end",
                    "buyer-period-end",
                    &cancelled_at,
                )
                .expect("schedule period-end cancellation")
            );
            let state: (String, bool, String, String, i64, i64, i64, i64) = tx
                .query_row(
                    "SELECT contract.status, contract.cancel_at_period_end,
                            contract.renewal_status, period.status,
                            account.posted_balance_units, account.held_balance_units,
                            (SELECT COUNT(*) FROM market_recurring_holds hold
                             WHERE hold.contract_id = contract.id
                               AND hold.period_sequence = 2 AND hold.status = 'released'),
                            (SELECT COUNT(*) FROM market_prepaid_ledger_entries entry
                             WHERE entry.account_id = account.id
                               AND entry.source_kind = 'recurring_refund')
                     FROM market_recurring_contracts contract
                     JOIN market_recurring_periods period
                       ON period.contract_id = contract.id AND period.sequence = 1
                     JOIN market_prepaid_accounts account
                       ON account.id = contract.prepaid_account_id
                     WHERE contract.id = ?1",
                    params![contract_id],
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
                        ))
                    },
                )
                .expect("read period-end cancellation state");
            assert_eq!(state.0, STATUS_ACTIVE);
            assert!(state.1);
            assert_eq!(state.2, "cancel_at_period_end");
            assert_eq!(state.3, "paid");
            assert_eq!(state.4, money_units(1_000).unwrap());
            assert_eq!(state.5, 0);
            assert_eq!(state.6, 1);
            assert_eq!(state.7, 0);
            tx.commit().expect("commit period-end cancellation");
        }

        reconcile_at(&store, at("2025-02-28T12:00:00Z")).await;
        let conn = store.conn.lock().await;
        let ended: (String, String, String, i64, i64) = conn
            .query_row(
                "SELECT contract.status, contract.end_reason, period.status,
                        account.posted_balance_units, account.held_balance_units
                 FROM market_recurring_contracts contract
                 JOIN market_recurring_periods period
                   ON period.contract_id = contract.id AND period.sequence = 1
                 JOIN market_prepaid_accounts account ON account.id = ?2
                 WHERE contract.id = ?1",
                params![contract_id, account_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("read completed period-end cancellation");
        assert_eq!(ended.0, STATUS_ENDED);
        assert_eq!(ended.1, "cancelled_at_period_end");
        assert_eq!(ended.2, "paid");
        assert_eq!(ended.3, money_units(1_000).unwrap());
        assert_eq!(ended.4, 0);
    }

    #[tokio::test]
    async fn generic_period_end_cancel_ends_recovery_immediately() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let starts_at = at("2025-01-01T00:00:00Z");
        let (_, contract_id) = create_test_contract(
            &store,
            "generic-recovery-cancel",
            RENEWAL_AUTOMATIC,
            2,
            starts_at,
        )
        .await;
        reconcile_at(&store, at("2025-02-01T00:00:00Z")).await;
        reconcile_at(&store, at("2025-03-01T00:00:00Z")).await;

        let view = store
            .market_recurring_cancel(
                &test_session("generic-recovery-cancel"),
                &contract_id,
                "period_end",
            )
            .await
            .expect("cancel recovery contract");
        assert_eq!(view.status, STATUS_ENDED);
        assert!(!view.cancel_at_period_end);
        let end_reason: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT end_reason FROM market_recurring_contracts WHERE id = ?1",
                params![contract_id],
                |row| row.get(0),
            )
            .expect("read recovery cancellation reason");
        assert_eq!(end_reason, "buyer_cancelled_during_renewal_recovery");
    }

    #[tokio::test]
    async fn shared_topup_funds_each_recovery_before_any_following_month() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let starts_at = at("2025-01-01T00:00:00Z");
        let now = starts_at.to_rfc3339();
        let (account_id, contract_ids) = {
            let conn = store.conn.lock().await;
            let tx = conn.transaction().expect("begin shared recovery setup");
            let account_id = crate::market_billing::ensure_market_prepaid_account_tx(
                &tx,
                "shared-buyer",
                "shared-buyer@example.com",
                "shared-supplier",
                "shared-supplier@example.com",
                crate::market_billing::MARKET_CURRENCY,
                &now,
            )
            .expect("create shared recurring account");
            crate::market_billing::credit_market_prepaid_funding_tx(
                &tx,
                &account_id,
                money_units(4_000).unwrap(),
                "shared-initial-funding",
                None,
                &now,
            )
            .expect("fund shared recurring account");
            let mut contract_ids = Vec::new();
            for (label, priority) in [("high", 10_i64), ("low", 0_i64)] {
                let product_ref = format!("shared-subscription-{label}");
                let activation_ref = format!("shared-activation-{label}");
                let service_ref = format!("shared-service-{label}");
                let service_label = format!("Shared {label}");
                let contract_id = prepare_contract_tx(
                    &tx,
                    PrepareRecurringContractInput {
                        product_kind: "share",
                        product_ref: &product_ref,
                        activation_ref: &activation_ref,
                        service_ref: &service_ref,
                        service_label: &service_label,
                        buyer_user_id: "shared-buyer",
                        buyer_email: "shared-buyer@example.com",
                        supplier_user_id: "shared-supplier",
                        supplier_email: "shared-supplier@example.com",
                        currency: crate::market_billing::MARKET_CURRENCY,
                        cycle_price_minor: 1_000,
                        offer_revision: 1,
                        renewal_policy: RENEWAL_AUTOMATIC,
                        auto_renew_max_price_minor: Some(1_000),
                        renewal_priority: priority,
                        trial_allowance_seconds: 0,
                    },
                    &now,
                )
                .expect("prepare shared recurring contract");
                activate_contract_tx(
                    &tx,
                    "share",
                    &activation_ref,
                    &product_ref,
                    &service_ref,
                    &service_label,
                    starts_at,
                    &now,
                )
                .expect("activate shared recurring contract")
                .expect("prepared shared recurring contract");
                contract_ids.push(contract_id);
            }
            tx.commit().expect("commit shared recovery setup");
            (account_id, contract_ids)
        };

        reconcile_at(&store, at("2025-02-01T00:00:00Z")).await;
        reconcile_at(&store, at("2025-03-01T00:00:00Z")).await;
        {
            let conn = store.conn.lock().await;
            for contract_id in &contract_ids {
                let status: String = conn
                    .query_row(
                        "SELECT status FROM market_recurring_contracts WHERE id = ?1",
                        params![contract_id],
                        |row| row.get(0),
                    )
                    .expect("read shared recovery status");
                assert_eq!(status, STATUS_RECOVERY);
            }
            conn.execute(
                "UPDATE market_recurring_contracts SET applied_control_state = 'suspended'
                 WHERE prepaid_account_id = ?1",
                params![account_id],
            )
            .expect("apply shared recovery suspension");
        }

        let topup_at = at("2025-03-01T00:01:00Z").to_rfc3339();
        let conn = store.conn.lock().await;
        let tx = conn.transaction().expect("begin shared recovery topup");
        let (_, actions) = crate::market_billing::credit_market_prepaid_funding_tx(
            &tx,
            &account_id,
            money_units(2_000).unwrap(),
            "shared-recovery-topup",
            None,
            &topup_at,
        )
        .expect("credit shared recovery topup");
        let mut resumed_ids = actions
            .iter()
            .filter(|action| action.kind == BillingActionKind::Resume)
            .map(|action| action.contract_id.clone())
            .collect::<Vec<_>>();
        resumed_ids.sort();
        let mut expected_ids = contract_ids.clone();
        expected_ids.sort();
        assert_eq!(resumed_ids, expected_ids);
        for contract_id in &contract_ids {
            let state: (String, i64, String, i64) = tx
                .query_row(
                    "SELECT status, current_period_sequence, renewal_status,
                            (SELECT COUNT(*) FROM market_recurring_holds hold
                             WHERE hold.contract_id = contract.id
                               AND hold.period_sequence = 4 AND hold.status = 'active')
                     FROM market_recurring_contracts contract WHERE id = ?1",
                    params![contract_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read fairly resumed recurring contract");
            assert_eq!(state.0, STATUS_ACTIVE);
            assert_eq!(state.1, 3);
            assert_eq!(state.2, "funding_required");
            assert_eq!(state.3, 0);
        }
        let balances: (i64, i64) = tx
            .query_row(
                "SELECT posted_balance_units, held_balance_units
                 FROM market_prepaid_accounts WHERE id = ?1",
                params![account_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read shared recovery balance");
        assert_eq!(balances, (0, 0));
        tx.commit().expect("commit shared recovery topup");
    }

    #[tokio::test]
    async fn renewal_error_reports_only_the_actual_topup_shortfall() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let starts_at = at("2025-01-15T12:00:00Z");
        let (account_id, contract_id) =
            create_test_contract(&store, "shortfall", RENEWAL_MANUAL, 1, starts_at).await;
        let now = at("2025-01-16T00:00:00Z").to_rfc3339();
        {
            let conn = store.conn.lock().await;
            let tx = conn.transaction().expect("begin partial recurring topup");
            crate::market_billing::credit_market_prepaid_funding_tx(
                &tx,
                &account_id,
                money_units(400).unwrap(),
                "partial-renewal-topup",
                None,
                &now,
            )
            .expect("credit partial recurring topup");
            tx.commit().expect("commit partial recurring topup");
        }
        let session = AuthSession {
            session_id: "shortfall-session".into(),
            user_id: "buyer-shortfall".into(),
            email: "buyer-shortfall@example.com".into(),
            auth_source_kind: "test".into(),
            auth_source_id: "test".into(),
            access_token_hash: "test".into(),
            refresh_token_hash: "test".into(),
            access_expires_at: Utc::now() + Duration::hours(1),
            refresh_expires_at: Utc::now() + Duration::days(1),
            created_at: Utc::now(),
            last_used_at: Utc::now(),
        };
        let error = store
            .market_recurring_reserve_next(&session, &contract_id)
            .await
            .expect_err("partial balance must not reserve a full month");
        match error {
            AppError::Coded { code, details, .. } => {
                assert_eq!(code, crate::market_access::ERROR_MARKET_PREPAID_REQUIRED);
                assert_eq!(details["requiredTopupMinor"], 600);
            }
            other => panic!("expected coded prepaid error, got {other:?}"),
        }
    }
}
