//! Delivery-backed Provider earnings and their seven-day settlement fence.

use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;
use crate::market_wallet::{Posting, capture_hold_tx, post_journal_tx, serialize_units};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use uuid::Uuid;

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn parse_utc(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| AppError::Internal("invalid market finance timestamp".into()))
}

fn unlock_at(debited_at: &str, fulfilled_at: &str) -> Result<String, AppError> {
    let debited_at = parse_utc(debited_at)?;
    let fulfilled_at = parse_utc(fulfilled_at)?;
    Ok(debited_at
        .max(fulfilled_at)
        .checked_add_signed(Duration::days(7))
        .ok_or_else(|| AppError::Internal("market earning maturity overflow".into()))?
        .to_rfc3339())
}

fn validate_slice(kind: &str, start: &str, end: &str, units: i64) -> Result<(), AppError> {
    if !matches!(kind, "share" | "client_host")
        || units <= 0
        || parse_utc(start)? >= parse_utc(end)?
    {
        return Err(AppError::BadRequest("invalid market delivery slice".into()));
    }
    Ok(())
}

fn existing_slice_tx(
    conn: &Connection,
    source_kind: &str,
    source_id: &str,
    buyer: &str,
    provider: &str,
    amount_units: i64,
) -> Result<Option<String>, AppError> {
    let existing = conn
        .query_row(
            "SELECT slice.id, lot.buyer_user_id, lot.provider_user_id, lot.amount_units
         FROM market_delivery_slices slice JOIN market_provider_earning_lots lot
           ON lot.delivery_slice_id = slice.id
         WHERE slice.source_kind = ?1 AND slice.source_id = ?2",
            params![source_kind, source_id],
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
        .map_err(db_error("read market delivery slice"))?;
    if let Some((id, saved_buyer, saved_provider, saved_units)) = existing {
        if saved_buyer != buyer || saved_provider != provider || saved_units != amount_units {
            return Err(AppError::Conflict(
                "market delivery idempotency conflict".into(),
            ));
        }
        return Ok(Some(id));
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn insert_slice_and_lot_tx(
    conn: &Connection,
    contract_kind: &str,
    contract_id: &str,
    source_kind: &str,
    source_id: &str,
    period_id: Option<&str>,
    buyer: &str,
    provider: &str,
    started_at: &str,
    ended_at: &str,
    amount_units: i64,
    debit_journal_id: &str,
    earning_journal_id: &str,
    buyer_debited_at: &str,
    now: &str,
) -> Result<String, AppError> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO market_delivery_slices (
            id, contract_kind, contract_id, source_kind, source_id, period_id,
            started_at, ended_at, amount_units, debit_journal_id,
            earning_journal_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            contract_kind,
            contract_id,
            source_kind,
            source_id,
            period_id,
            started_at,
            ended_at,
            amount_units,
            debit_journal_id,
            earning_journal_id,
            now
        ],
    )
    .map_err(db_error("insert delivery slice"))?;
    let lot_id = Uuid::new_v4().to_string();
    let allocation = crate::market_commission::allocate_delivery_tx(
        conn,
        &lot_id,
        contract_id,
        period_id,
        source_kind,
        amount_units,
        provider,
        now,
    )?;
    let disputed_by_open_case: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_service_disputes
             WHERE contract_id = ?1 AND status = 'open')",
            params![contract_id],
            |row| row.get(0),
        )
        .map_err(db_error("check ongoing service dispute"))?;
    conn.execute(
        "INSERT INTO market_provider_earning_lots (
            id, delivery_slice_id, provider_user_id, buyer_user_id, amount_units,
            buyer_debited_at, fulfilled_at, unlock_at, disputed, created_at, commission_units
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            lot_id,
            id,
            provider,
            buyer,
            amount_units,
            buyer_debited_at,
            ended_at,
            unlock_at(buyer_debited_at, ended_at)?,
            disputed_by_open_case,
            now,
            allocation.fee_units
        ],
    )
    .map_err(db_error("insert Provider earning lot"))?;
    crate::market_commission::finish_delivery_tx(conn, &lot_id, allocation, now)?;
    if disputed_by_open_case != 0 {
        conn.execute(
            "INSERT INTO market_service_dispute_lots (dispute_id, earning_lot_id)
             SELECT id, ?2 FROM market_service_disputes
             WHERE contract_id = ?1 AND status = 'open'",
            params![contract_id, lot_id],
        )
        .map_err(db_error("record earning frozen by ongoing dispute"))?;
        // Delivery may continue while a dispute is open. New lots need the
        // same durable hold cause as the original lots; a bare disputed flag
        // would otherwise survive resolution with nothing to release.
        conn.execute(
            "INSERT OR IGNORE INTO market_provider_earning_holds
                (id, lot_id, reason_kind, reason_ref, created_at)
             SELECT lower(hex(randomblob(16))), ?2, 'dispute', id, ?3
             FROM market_service_disputes WHERE contract_id = ?1 AND status = 'open'",
            params![contract_id, lot_id, now],
        )
        .map_err(db_error("hold new earning for ongoing dispute"))?;
    }
    Ok(id)
}

#[cfg(test)]
/// Fixture helper for historical direct-debit evidence. The debit and
/// earning lot share one journal, so there is no Provider credit without cash.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_metered_delivery_tx(
    conn: &Connection,
    contract_kind: &str,
    contract_id: &str,
    accrual_id: &str,
    buyer: &str,
    provider: &str,
    started_at: &str,
    ended_at: &str,
    amount_units: i64,
    now: &str,
) -> Result<String, AppError> {
    validate_slice(contract_kind, started_at, ended_at, amount_units)?;
    if let Some(id) = existing_slice_tx(
        conn,
        "metered_accrual",
        accrual_id,
        buyer,
        provider,
        amount_units,
    )? {
        return Ok(id);
    }
    let journal = crate::market_wallet::debit_available_tx(
        conn,
        buyer,
        amount_units,
        "provider_pending",
        provider,
        "metered_accrual",
        accrual_id,
        "debit_and_earn",
        now,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal(
            "metered journal missing delivery evidence".into(),
        ));
    }
    insert_slice_and_lot_tx(
        conn,
        contract_kind,
        contract_id,
        "metered_accrual",
        accrual_id,
        None,
        buyer,
        provider,
        started_at,
        ended_at,
        amount_units,
        &journal.id,
        &journal.id,
        now,
        now,
    )
}

/// Platform-wallet metered earnings must consume a previously reserved buyer
/// budget. A direct available-balance debit cannot bypass older service holds.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_captured_metered_delivery_tx(
    conn: &Connection,
    contract_kind: &str,
    contract_id: &str,
    batch_id: &str,
    hold_id: &str,
    buyer: &str,
    provider: &str,
    started_at: &str,
    ended_at: &str,
    amount_units: i64,
    now: &str,
) -> Result<(String, String), AppError> {
    validate_slice(contract_kind, started_at, ended_at, amount_units)?;
    if existing_slice_tx(
        conn,
        "metered_batch",
        batch_id,
        buyer,
        provider,
        amount_units,
    )?
    .is_some()
    {
        return Err(AppError::Conflict("metered batch already posted".into()));
    }
    let journal = capture_hold_tx(
        conn,
        hold_id,
        amount_units,
        "provider_pending",
        provider,
        "metered_batch",
        batch_id,
        now,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal(
            "metered batch journal exists without delivery evidence".into(),
        ));
    }
    let slice = insert_slice_and_lot_tx(
        conn,
        contract_kind,
        contract_id,
        "metered_batch",
        batch_id,
        None,
        buyer,
        provider,
        started_at,
        ended_at,
        amount_units,
        &journal.id,
        &journal.id,
        now,
        now,
    )?;
    Ok((slice, journal.id))
}

fn liability_balance_tx(conn: &Connection, kind: &str, subject: &str) -> Result<i64, AppError> {
    let signed: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(signed_units), 0) FROM market_journal_postings
         WHERE account_kind = ?1 AND subject_id = ?2",
            params![kind, subject],
            |row| row.get(0),
        )
        .map_err(db_error("read market liability balance"))?;
    signed
        .checked_neg()
        .ok_or_else(|| AppError::Internal("market liability overflow".into()))
}

/// Moves already-debited monthly consideration to Provider pending liability
/// only after a uniquely identified interval of available service.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_monthly_delivery_tx(
    conn: &Connection,
    contract_kind: &str,
    contract_id: &str,
    period_id: &str,
    delivery_source_id: &str,
    buyer: &str,
    provider: &str,
    started_at: &str,
    ended_at: &str,
    amount_units: i64,
    debit_journal_id: &str,
    buyer_debited_at: &str,
    now: &str,
) -> Result<String, AppError> {
    validate_slice(contract_kind, started_at, ended_at, amount_units)?;
    if let Some(id) = existing_slice_tx(
        conn,
        "monthly_delivery",
        delivery_source_id,
        buyer,
        provider,
        amount_units,
    )? {
        return Ok(id);
    }
    if liability_balance_tx(conn, "unfulfilled", period_id)? < amount_units {
        return Err(AppError::Conflict(
            "monthly unfulfilled funds are insufficient".into(),
        ));
    }
    let fingerprint =
        format!("{contract_id}:{period_id}:{provider}:{amount_units}:{started_at}:{ended_at}");
    let journal = post_journal_tx(
        conn,
        "monthly_delivery",
        delivery_source_id,
        "earn",
        &fingerprint,
        &[
            Posting {
                kind: "unfulfilled",
                subject_id: period_id,
                signed_units: amount_units,
            },
            Posting {
                kind: "provider_pending",
                subject_id: provider,
                signed_units: -amount_units,
            },
        ],
        now,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal(
            "monthly earning journal missing delivery evidence".into(),
        ));
    }
    insert_slice_and_lot_tx(
        conn,
        contract_kind,
        contract_id,
        "monthly_delivery",
        delivery_source_id,
        Some(period_id),
        buyer,
        provider,
        started_at,
        ended_at,
        amount_units,
        debit_journal_id,
        &journal.id,
        buyer_debited_at,
        now,
    )
}

/// Recompute the `disputed` projection of the given lots from their active
/// holds. Every hold change goes through this projection.
fn recompute_lot_holds_tx(
    conn: &Connection,
    lot_filter_sql: &str,
    arg: &str,
) -> Result<(), AppError> {
    conn.execute(
        &format!(
            "UPDATE market_provider_earning_lots
             SET disputed = CASE WHEN EXISTS (
                     SELECT 1 FROM market_provider_earning_holds hold
                     WHERE hold.lot_id = market_provider_earning_lots.id
                       AND hold.released_at IS NULL) THEN 1 ELSE 0 END
             WHERE id IN ({lot_filter_sql})"
        ),
        params![arg],
    )
    .map_err(db_error("recompute Provider earning holds"))?;
    Ok(())
}

fn notify_providers_for_lots_tx(
    conn: &Connection,
    lot_filter_sql: &str,
    arg: &str,
    reason_kind: &str,
    reason_ref: &str,
    time: &str,
) -> Result<(), AppError> {
    let providers = {
        let mut statement = conn
            .prepare(&format!(
                "SELECT provider_user_id, SUM(net_amount_units - reversed_units - paid_units - converted_units)
                 FROM market_provider_earning_lots WHERE id IN ({lot_filter_sql})
                 GROUP BY provider_user_id"
            ))
            .map_err(db_error("prepare held earning recipients"))?;
        statement
            .query_map(params![arg], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(db_error("query held earning recipients"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read held earning recipients"))?
    };
    for (provider, units) in providers {
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "provider",
            &provider,
            "earnings_held",
            "earning_hold",
            reason_ref,
            &format!("earnings_held:{reason_kind}:{reason_ref}:{provider}"),
            serde_json::json!({
                "reason": reason_kind,
                "amountMinor": units / crate::market_common::MONEY_UNITS_PER_MINOR,
            }),
            time,
        )?;
    }
    Ok(())
}

const CONTRACT_LOTS_SQL: &str = "SELECT lot.id FROM market_provider_earning_lots lot
     JOIN market_delivery_slices slice ON slice.id = lot.delivery_slice_id
     WHERE slice.contract_id = ?1";

/// Hold every earning lot of a contract for a recorded cause.
pub(crate) fn hold_contract_earnings_tx(
    conn: &Connection,
    contract_id: &str,
    reason_kind: &str,
    time: &str,
) -> Result<(), AppError> {
    let inserted = conn
        .execute(
            &format!(
                "INSERT OR IGNORE INTO market_provider_earning_holds
                     (id, lot_id, reason_kind, reason_ref, created_at)
                 SELECT lower(hex(randomblob(16))), held.id, ?2, ?1, ?3
                 FROM ({CONTRACT_LOTS_SQL}) held"
            ),
            params![contract_id, reason_kind, time],
        )
        .map_err(db_error("hold contract earnings"))?;
    recompute_lot_holds_tx(conn, CONTRACT_LOTS_SQL, contract_id)?;
    if inserted > 0 {
        notify_providers_for_lots_tx(
            conn,
            CONTRACT_LOTS_SQL,
            contract_id,
            reason_kind,
            contract_id,
            time,
        )?;
    }
    Ok(())
}

/// Hold the lots registered for a service dispute.
pub(crate) fn hold_dispute_earnings_tx(
    conn: &Connection,
    dispute_id: &str,
    time: &str,
) -> Result<(), AppError> {
    let lots = "SELECT earning_lot_id FROM market_service_dispute_lots WHERE dispute_id = ?1";
    conn.execute(
        &format!(
            "INSERT OR IGNORE INTO market_provider_earning_holds
                 (id, lot_id, reason_kind, reason_ref, created_at)
             SELECT lower(hex(randomblob(16))), held.earning_lot_id, 'dispute', ?1, ?2
             FROM ({lots}) held"
        ),
        params![dispute_id, time],
    )
    .map_err(db_error("hold disputed earnings"))?;
    recompute_lot_holds_tx(conn, lots, dispute_id)?;
    notify_providers_for_lots_tx(conn, lots, dispute_id, "dispute", dispute_id, time)
}

/// Release every active hold with this cause. A lot becomes withdrawable only
/// when no other hold remains. Returns the number of released holds.
pub(crate) fn release_earning_holds_tx(
    conn: &Connection,
    reason_kind: &str,
    reason_ref: &str,
    actor: &str,
    release_reason: &str,
    time: &str,
) -> Result<usize, AppError> {
    let lots = "SELECT lot_id FROM market_provider_earning_holds
                WHERE reason_kind = ?1 AND reason_ref = ?2 AND released_by = ?3
                  AND released_at = ?4";
    let released = conn
        .execute(
            "UPDATE market_provider_earning_holds
             SET released_at = ?4, released_by = ?3, release_reason = ?5
             WHERE reason_kind = ?1 AND reason_ref = ?2 AND released_at IS NULL",
            params![reason_kind, reason_ref, actor, time, release_reason],
        )
        .map_err(db_error("release Provider earning holds"))?;
    if released == 0 {
        return Ok(0);
    }
    conn.execute(
        &format!(
            "UPDATE market_provider_earning_lots
             SET disputed = CASE WHEN EXISTS (
                     SELECT 1 FROM market_provider_earning_holds hold
                     WHERE hold.lot_id = market_provider_earning_lots.id
                       AND hold.released_at IS NULL) THEN 1 ELSE 0 END
             WHERE id IN ({lots})"
        ),
        params![reason_kind, reason_ref, actor, time],
    )
    .map_err(db_error("recompute released Provider earnings"))?;
    let providers = {
        let mut statement = conn
            .prepare(&format!(
                "SELECT DISTINCT provider_user_id FROM market_provider_earning_lots
                 WHERE id IN ({lots}) AND disputed = 0"
            ))
            .map_err(db_error("prepare released earning recipients"))?;
        statement
            .query_map(params![reason_kind, reason_ref, actor, time], |row| {
                row.get::<_, String>(0)
            })
            .map_err(db_error("query released earning recipients"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read released earning recipients"))?
    };
    for provider in providers {
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "provider",
            &provider,
            "earnings_released",
            "earning_hold",
            reason_ref,
            &format!("earnings_released:{reason_kind}:{reason_ref}:{provider}:{time}"),
            serde_json::json!({ "reason": reason_kind }),
            time,
        )?;
    }
    Ok(released)
}

/// Hold one lot whose unlock failed, open its case and alert the Owner.
/// Resolving the case (or releasing the manual hold) returns it to maturity.
fn hold_failed_unlock_tx(conn: &Connection, lot_id: &str, time: &str) -> Result<(), AppError> {
    let lot = "SELECT ?1";
    conn.execute(
        "INSERT OR IGNORE INTO market_provider_earning_holds
             (id, lot_id, reason_kind, reason_ref, created_at)
         VALUES (lower(hex(randomblob(16))), ?1, 'manual', ?1, ?2)",
        params![lot_id, time],
    )
    .map_err(db_error("hold failed earning unlock"))?;
    recompute_lot_holds_tx(conn, lot, lot_id)?;
    conn.execute(
        "INSERT OR IGNORE INTO market_finance_reconciliation_cases
            (id, code, object_kind, object_id, status, created_at)
         VALUES (?1, 'earning_unlock_failed', 'earning_lot', ?2, 'open', ?3)",
        params![Uuid::new_v4().to_string(), lot_id, time],
    )
    .map_err(db_error("record failed earning unlock"))?;
    crate::market_notifications::owner_signal_tx(
        conn,
        "earning_unlock_failed",
        "earning_lot",
        lot_id,
        serde_json::json!({ "lotId": lot_id }),
        time,
        false,
    )
}

/// The hold cause a finance case code places on contract earnings.
pub(crate) fn hold_kind_for_case(code: &str) -> Option<&'static str> {
    match code {
        "wallet_cleanup_uncertain" => Some("cleanup_uncertain"),
        "wallet_service_missing" => Some("service_missing"),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EarningHoldView {
    pub reason_kind: String,
    pub reason_ref: String,
    pub provider_user_id: String,
    pub lots: i64,
    #[serde(serialize_with = "serialize_units")]
    pub held_units: i64,
    pub created_at: String,
}

/// Active holds grouped by cause, for the Owner console and the Provider.
pub(crate) fn active_earning_holds_tx(
    conn: &Connection,
    provider_user_id: Option<&str>,
) -> Result<Vec<EarningHoldView>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT hold.reason_kind, hold.reason_ref, lot.provider_user_id, COUNT(*),
                    SUM(lot.net_amount_units - lot.reversed_units - lot.paid_units - lot.converted_units), MIN(hold.created_at)
             FROM market_provider_earning_holds hold
             JOIN market_provider_earning_lots lot ON lot.id = hold.lot_id
             WHERE hold.released_at IS NULL AND (?1 IS NULL OR lot.provider_user_id = ?1)
             GROUP BY hold.reason_kind, hold.reason_ref, lot.provider_user_id
             ORDER BY MIN(hold.created_at) LIMIT 500",
        )
        .map_err(db_error("prepare earning holds"))?;
    statement
        .query_map(params![provider_user_id], |row| {
            Ok(EarningHoldView {
                reason_kind: row.get(0)?,
                reason_ref: row.get(1)?,
                provider_user_id: row.get(2)?,
                lots: row.get(3)?,
                held_units: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(db_error("query earning holds"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read earning holds"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderFinanceSummary {
    #[serde(serialize_with = "serialize_units")]
    pub converted_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub gross_earned_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub commission_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub net_earned_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub pending_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub available_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub withdrawable_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub withdrawal_held_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub paid_units: i64,
}

pub(crate) fn provider_summary_tx(
    conn: &Connection,
    provider_user_id: &str,
) -> Result<ProviderFinanceSummary, AppError> {
    let mut summary = conn.query_row(
        "SELECT COALESCE(SUM(net_amount_units - reversed_units - unlocked_units), 0),
                COALESCE(SUM(unlocked_units - held_units - paid_units - converted_units), 0),
                COALESCE(SUM(CASE WHEN disputed = 0
                                  THEN unlocked_units - held_units - paid_units - converted_units ELSE 0 END), 0),
                COALESCE(SUM(held_units), 0), COALESCE(SUM(paid_units), 0),
                COALESCE(SUM(amount_units - reversed_units - commission_reversed_units), 0),
                COALESCE(SUM(commission_units - commission_reversed_units), 0),
                COALESCE(SUM(net_amount_units - reversed_units), 0),
                COALESCE(SUM(converted_units), 0)
         FROM market_provider_earning_lots WHERE provider_user_id = ?1",
        params![provider_user_id],
        |row| {
            Ok(ProviderFinanceSummary {
                converted_units: row.get(8)?,
                gross_earned_units: row.get(5)?,
                commission_units: row.get(6)?,
                net_earned_units: row.get(7)?,
                pending_units: row.get(0)?,
                available_units: row.get(1)?,
                withdrawable_units: row.get(2)?,
                withdrawal_held_units: row.get(3)?,
                paid_units: row.get(4)?,
            })
        },
    )
    .map_err(db_error("read Provider finance summary"))?;
    let (gross, pending, credited): (i64, i64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(amount_units-reversed_units),0),
        COALESCE(SUM(amount_units-reversed_units-credited_units),0),COALESCE(SUM(credited_units),0)
        FROM client_termination_compensations WHERE provider_user_id=?1",
            params![provider_user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(db_error("read Client compensation summary"))?;
    summary.gross_earned_units += gross;
    summary.net_earned_units += gross;
    summary.pending_units += pending;
    summary.converted_units += credited;
    Ok(summary)
}

/// One bounded pass; running it repeatedly is harmless. The lot is locked by
/// the caller's IMMEDIATE transaction before the journal and projection move.
/// Production uses [`unlock_due_earnings_isolated`]; tests drive this directly.
#[cfg(test)]
pub(crate) fn unlock_due_earnings_tx(
    conn: &Connection,
    at: &str,
    limit: i64,
) -> Result<usize, AppError> {
    let at_utc = parse_utc(at)?;
    let mut statement = conn
        .prepare(
            "SELECT id, provider_user_id, net_amount_units - reversed_units - unlocked_units,
                unlock_at FROM market_provider_earning_lots
         WHERE disputed = 0 AND net_amount_units - reversed_units - unlocked_units
             + commission_units - commission_reversed_units - commission_unlocked_units > 0
         ORDER BY unlock_at, id LIMIT ?1",
        )
        .map_err(db_error("prepare due Provider earnings"))?;
    let rows = statement
        .query_map(params![limit.clamp(1, 200)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(db_error("query due Provider earnings"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read due Provider earnings"))?;
    let mut count = 0;
    for (id, provider, units, maturity) in rows {
        if parse_utc(&maturity)? > at_utc {
            continue;
        }
        unlock_lot_tx(conn, &id, &provider, units, &maturity, at)?;
        count += 1;
    }
    Ok(count)
}

fn unlock_lot_tx(
    conn: &Connection,
    id: &str,
    provider: &str,
    units: i64,
    maturity: &str,
    at: &str,
) -> Result<(), AppError> {
    if !crate::market_commission::lot_evidence_valid_tx(conn, id)? {
        return Err(AppError::Internal(
            "commission evidence requires Owner review".into(),
        ));
    }
    let fee: i64 = conn
        .query_row(
            "SELECT commission_units - commission_reversed_units - commission_unlocked_units
        FROM market_provider_earning_lots WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(db_error("read pending commission maturity"))?;
    let fingerprint = format!("{provider}:{units}:{maturity}:commission={fee}");
    let mut postings = Vec::new();
    if units > 0 {
        postings.push(Posting {
            kind: "provider_pending",
            subject_id: provider,
            signed_units: units,
        });
        postings.push(Posting {
            kind: "provider_available",
            subject_id: provider,
            signed_units: -units,
        });
    }
    if fee > 0 {
        postings.push(Posting {
            kind: "platform_commission_pending",
            subject_id: id,
            signed_units: fee,
        });
        postings.push(Posting {
            kind: "platform_commission_revenue",
            subject_id: id,
            signed_units: -fee,
        });
    }
    let journal = post_journal_tx(
        conn,
        "earning_lot",
        id,
        "unlock",
        &fingerprint,
        &postings,
        at,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal(
            "earning lot unlock projection drift".into(),
        ));
    }
    let changed = conn
        .execute(
            "UPDATE market_provider_earning_lots SET unlocked_units = unlocked_units + ?2,
                commission_unlocked_units = commission_unlocked_units + ?3
         WHERE id = ?1 AND disputed = 0
           AND net_amount_units - reversed_units - unlocked_units = ?2
           AND commission_units - commission_reversed_units - commission_unlocked_units = ?3",
            params![id, units, fee],
        )
        .map_err(db_error("unlock Provider earning"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "Provider earning changed during unlock".into(),
        ));
    }
    Ok(())
}

/// The background variant of [`unlock_due_earnings_tx`]: one transaction,
/// one savepoint per lot. A failing lot is rolled back and logged without
/// blocking the other due lots in the batch.
pub(crate) fn unlock_due_earnings_isolated(
    conn: &Connection,
    at: &str,
    limit: i64,
) -> Result<usize, AppError> {
    let at_utc = parse_utc(at)?;
    let tx = conn
        .transaction_with_behavior(crate::db::TransactionBehavior::Immediate)
        .map_err(db_error("begin earning maturity"))?;
    let rows = {
        let mut statement = tx
            .prepare(
                "SELECT id, provider_user_id, net_amount_units - reversed_units - unlocked_units,
                    unlock_at FROM market_provider_earning_lots
             WHERE disputed = 0 AND net_amount_units - reversed_units - unlocked_units
                 + commission_units - commission_reversed_units - commission_unlocked_units > 0
             ORDER BY unlock_at, id LIMIT ?1",
            )
            .map_err(db_error("prepare due Provider earnings"))?;
        statement
            .query_map(params![limit.clamp(1, 1_000)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(db_error("query due Provider earnings"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read due Provider earnings"))?
    };
    let mut count = 0;
    for (id, provider, units, maturity) in rows {
        if parse_utc(&maturity)? > at_utc {
            continue;
        }
        tx.execute_batch("SAVEPOINT earning_lot_unlock")
            .map_err(db_error("open earning unlock savepoint"))?;
        match unlock_lot_tx(&tx, &id, &provider, units, &maturity, at) {
            Ok(()) => {
                tx.execute_batch("RELEASE SAVEPOINT earning_lot_unlock")
                    .map_err(db_error("release earning unlock savepoint"))?;
                count += 1;
            }
            Err(_) => {
                tx.execute_batch(
                    "ROLLBACK TO SAVEPOINT earning_lot_unlock;
                     RELEASE SAVEPOINT earning_lot_unlock",
                )
                .map_err(db_error("roll back earning unlock savepoint"))?;
                // A lot that cannot unlock would stay at the head of the
                // maturity order and starve every later lot. Hold it with a
                // visible cause for the Owner instead of retrying it forever.
                hold_failed_unlock_tx(&tx, &id, at)?;
                tracing::warn!(lot_id = %id, "Provider earning unlock failed; lot held for review");
            }
        }
    }
    tx.commit().map_err(db_error("commit earning maturity"))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TransactionBehavior;
    use crate::market_wallet::{credit_verified_topup_tx, reconcile_wallet_tx, wallet_snapshot_tx};

    #[test]
    fn metered_earning_unlocks_only_seven_days_after_debit_and_delivery() {
        let conn = crate::test_support::database().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let debited_at = "2026-10-01T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "order", 100, "receipt", debited_at).unwrap();
        record_metered_delivery_tx(
            &tx,
            "share",
            "contract",
            "accrual",
            "buyer",
            "provider",
            "2026-10-01T00:00:00+00:00",
            "2026-10-01T00:00:01+00:00",
            86_400,
            debited_at,
        )
        .unwrap();
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            86_400
        );
        assert_eq!(
            unlock_due_earnings_tx(&tx, "2026-10-08T00:00:00+00:00", 10).unwrap(),
            0
        );
        assert_eq!(
            unlock_due_earnings_tx(&tx, "2026-10-08T00:00:01+00:00", 10).unwrap(),
            1
        );
        assert_eq!(
            unlock_due_earnings_tx(&tx, "2026-10-09T00:00:00+00:00", 10).unwrap(),
            0
        );
        assert_eq!(
            provider_summary_tx(&tx, "provider")
                .unwrap()
                .available_units,
            86_400
        );
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            8_553_600
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }
}
