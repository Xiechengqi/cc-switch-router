//! Delivery-backed Provider earnings and their seven-day settlement fence.

use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;
use crate::market_wallet::{Posting, capture_hold_tx, debit_available_tx, post_journal_tx};
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
    conn.execute(
        "INSERT INTO market_provider_earning_lots (
            id, delivery_slice_id, provider_user_id, buyer_user_id, amount_units,
            buyer_debited_at, fulfilled_at, unlock_at, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            Uuid::new_v4().to_string(),
            id,
            provider,
            buyer,
            amount_units,
            buyer_debited_at,
            ended_at,
            unlock_at(buyer_debited_at, ended_at)?,
            now
        ],
    )
    .map_err(db_error("insert Provider earning lot"))?;
    Ok(id)
}

/// Use only for persisted, billable metered service intervals. The debit and
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
    let journal = debit_available_tx(
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderFinanceSummary {
    pub pending_units: i64,
    pub available_units: i64,
    pub withdrawable_units: i64,
    pub withdrawal_held_units: i64,
    pub paid_units: i64,
}

pub(crate) fn provider_summary_tx(
    conn: &Connection,
    provider_user_id: &str,
) -> Result<ProviderFinanceSummary, AppError> {
    conn.query_row(
        "SELECT COALESCE(SUM(amount_units - reversed_units - unlocked_units), 0),
                COALESCE(SUM(unlocked_units - held_units - paid_units), 0),
                COALESCE(SUM(CASE WHEN disputed = 0
                                  THEN unlocked_units - held_units - paid_units ELSE 0 END), 0),
                COALESCE(SUM(held_units), 0), COALESCE(SUM(paid_units), 0)
         FROM market_provider_earning_lots WHERE provider_user_id = ?1",
        params![provider_user_id],
        |row| {
            Ok(ProviderFinanceSummary {
                pending_units: row.get(0)?,
                available_units: row.get(1)?,
                withdrawable_units: row.get(2)?,
                withdrawal_held_units: row.get(3)?,
                paid_units: row.get(4)?,
            })
        },
    )
    .map_err(db_error("read Provider finance summary"))
}

/// One bounded pass; running it repeatedly is harmless. The lot is locked by
/// the caller's IMMEDIATE transaction before the journal and projection move.
pub(crate) fn unlock_due_earnings_tx(
    conn: &Connection,
    at: &str,
    limit: i64,
) -> Result<usize, AppError> {
    let at_utc = parse_utc(at)?;
    let mut statement = conn
        .prepare(
            "SELECT id, provider_user_id, amount_units - reversed_units - unlocked_units,
                unlock_at FROM market_provider_earning_lots
         WHERE disputed = 0 AND amount_units - reversed_units - unlocked_units > 0
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
        let fingerprint = format!("{provider}:{units}:{maturity}");
        let journal = post_journal_tx(
            conn,
            "earning_lot",
            &id,
            "unlock",
            &fingerprint,
            &[
                Posting {
                    kind: "provider_pending",
                    subject_id: &provider,
                    signed_units: units,
                },
                Posting {
                    kind: "provider_available",
                    subject_id: &provider,
                    signed_units: -units,
                },
            ],
            at,
        )?;
        if !journal.inserted {
            return Err(AppError::Internal(
                "earning lot unlock projection drift".into(),
            ));
        }
        let changed = conn
            .execute(
                "UPDATE market_provider_earning_lots SET unlocked_units = unlocked_units + ?2
             WHERE id = ?1 AND disputed = 0
               AND amount_units - reversed_units - unlocked_units = ?2",
                params![id, units],
            )
            .map_err(db_error("unlock Provider earning"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "Provider earning changed during unlock".into(),
            ));
        }
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TransactionBehavior;
    use crate::market_wallet::{credit_verified_topup_tx, reconcile_wallet_tx, wallet_snapshot_tx};

    #[test]
    fn metered_earning_unlocks_only_seven_days_after_debit_and_delivery() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
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
