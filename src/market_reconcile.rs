//! Independent journal/projection checks. A case is never treated as a
//! balancing entry: it stops new financial commitments until an Owner has
//! investigated the underlying immutable evidence.

use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;
use crate::market_finance::provider_summary_tx;
use crate::market_wallet::reconcile_wallet_tx;
use uuid::Uuid;

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

const CRITICAL_CODES: &[&str] = &[
    "wallet_projection_mismatch",
    "provider_liability_mismatch",
    "period_unfulfilled_mismatch",
    "journal_unsealed",
    "credited_topup_missing_evidence",
    "paid_payout_missing_evidence",
];

fn case_tx(
    conn: &Connection,
    code: &str,
    object_kind: &str,
    object_id: &str,
    now: &str,
) -> Result<usize, AppError> {
    conn.execute(
        "INSERT OR IGNORE INTO market_finance_reconciliation_cases
            (id, code, object_kind, object_id, status, created_at)
         VALUES (?1, ?2, ?3, ?4, 'open', ?5)",
        params![
            Uuid::new_v4().to_string(),
            code,
            object_kind,
            object_id,
            now
        ],
    )
    .map_err(db_error("open finance reconciliation case"))
}

pub(crate) fn critical_case_count_tx(conn: &Connection) -> Result<i64, AppError> {
    let placeholders = CRITICAL_CODES
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT COUNT(*) FROM market_finance_reconciliation_cases
         WHERE status = 'open' AND code IN ({placeholders})"
    );
    let params = crate::db::params_from_iter(CRITICAL_CODES.iter().copied());
    conn.query_row(&sql, params, |row| row.get(0))
        .map_err(db_error("count critical finance cases"))
}

pub(crate) fn ensure_new_finance_writes_safe_tx(conn: &Connection) -> Result<(), AppError> {
    if critical_case_count_tx(conn)? > 0 {
        return Err(AppError::ServiceUnavailable(
            "market finance reconciliation is required".into(),
        ));
    }
    Ok(())
}

fn critical_case_still_failed_tx(
    conn: &Connection,
    code: &str,
    object_kind: &str,
    object_id: &str,
) -> Result<bool, AppError> {
    match (code, object_kind) {
        ("wallet_projection_mismatch", "wallet") => {
            Ok(reconcile_wallet_tx(conn, object_id).is_err())
        }
        ("provider_liability_mismatch", "provider") => {
            let summary = provider_summary_tx(conn, object_id)?;
            let (pending, available, held): (i64, i64, i64) = conn
                .query_row(
                    "SELECT
                        COALESCE(SUM(CASE WHEN account_kind = 'provider_pending' THEN signed_units ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN account_kind = 'provider_available' THEN signed_units ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN account_kind = 'provider_held' THEN signed_units ELSE 0 END), 0)
                     FROM market_journal_postings WHERE subject_id = ?1
                       AND account_kind IN ('provider_pending', 'provider_available', 'provider_held')",
                    params![object_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(db_error("recheck Provider liability"))?;
            Ok(pending != -summary.pending_units
                || available != -summary.available_units
                || held != -summary.withdrawal_held_units)
        }
        ("period_unfulfilled_mismatch", "period") => {
            let projection = conn
                .query_row(
                    "SELECT period.amount_units - period.fulfilled_units - period.refunded_units,
                        COALESCE((SELECT SUM(signed_units) FROM market_journal_postings
                                  WHERE account_kind = 'unfulfilled' AND subject_id = period.id), 0)
                     FROM market_platform_periods period WHERE period.id = ?1",
                    params![object_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(db_error("recheck unfulfilled period"))?;
            Ok(projection.is_none_or(|(remaining, ledger)| ledger != -remaining))
        }
        ("journal_unsealed", "journal") => {
            let state = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM market_journal_seals WHERE journal_id = ?1)
                     FROM market_journal_transactions WHERE id = ?1",
                    params![object_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(db_error("recheck journal seal"))?;
            Ok(state != Some(1))
        }
        ("credited_topup_missing_evidence", "topup") => {
            let state = conn
                .query_row(
                    "SELECT topup.status, receipt.order_id IS NOT NULL,
                            journal.source_kind = 'topup' AND journal.source_id = topup.id
                     FROM market_topup_orders topup
                     LEFT JOIN market_topup_receipts receipt ON receipt.order_id = topup.id
                     LEFT JOIN market_journal_transactions journal
                       ON journal.id = topup.credited_journal_id
                     WHERE topup.id = ?1",
                    params![object_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, bool>(1)?,
                            row.get::<_, Option<bool>>(2)?.unwrap_or(false),
                        ))
                    },
                )
                .optional()
                .map_err(db_error("recheck credited top-up"))?;
            Ok(state.is_none_or(|(status, receipt, journal)| {
                status != "credited" || !receipt || !journal
            }))
        }
        ("paid_payout_missing_evidence", "provider_withdrawal" | "buyer_refund") => {
            let (table, network_join, network_column, journal_kind, held_kind, held_subject) =
                if object_kind == "provider_withdrawal" {
                    (
                        "market_payout_requests",
                        "JOIN market_payout_destinations destination ON destination.id = payout.destination_id",
                        "destination.network",
                        "payout",
                        "provider_held",
                        "payout.provider_user_id",
                    )
                } else {
                    (
                        "market_buyer_refund_requests",
                        "",
                        "payout.network",
                        "buyer_refund",
                        "buyer_refund_held",
                        "payout.id",
                    )
                };
            let sql = format!(
                "SELECT payout.status, payout.external_reference,
                        payout.amount_units / ?2, {network_column},
                        receipt.external_reference, receipt.amount_minor, receipt.network,
                        paid_journal.id, paid_seal.journal_id, payout.amount_units,
                        (SELECT COALESCE(SUM(posting.signed_units), 0)
                           FROM market_journal_postings posting
                          WHERE posting.journal_id = paid_journal.id
                            AND posting.account_kind = '{held_kind}'
                            AND posting.subject_id = {held_subject}),
                        (SELECT COALESCE(SUM(posting.signed_units), 0)
                           FROM market_journal_postings posting
                          WHERE posting.journal_id = paid_journal.id
                            AND posting.account_kind = 'owner_clearing'
                            AND posting.subject_id = 'owner')
                 FROM {table} payout {network_join}
                 LEFT JOIN market_external_payout_receipts receipt
                   ON receipt.object_kind = ?3 AND receipt.object_id = payout.id
                 LEFT JOIN market_journal_transactions paid_journal
                   ON paid_journal.source_kind = '{journal_kind}'
                  AND paid_journal.source_id = payout.id AND paid_journal.action = 'paid'
                 LEFT JOIN market_journal_seals paid_seal
                   ON paid_seal.journal_id = paid_journal.id
                 WHERE payout.id = ?1"
            );
            let state = conn
                .query_row(
                    &sql,
                    params![
                        object_id,
                        crate::market_billing::MONEY_UNITS_PER_MINOR,
                        object_kind
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, i64>(9)?,
                            row.get::<_, i64>(10)?,
                            row.get::<_, i64>(11)?,
                        ))
                    },
                )
                .optional()
                .map_err(db_error("recheck external payout receipt"))?;
            Ok(state.is_none_or(
                |(
                    status,
                    reference,
                    amount,
                    network,
                    receipt_ref,
                    receipt_amount,
                    receipt_network,
                    journal,
                    seal,
                    amount_units,
                    held_posting,
                    clearing_posting,
                )| {
                    status != "paid"
                        || reference != receipt_ref
                        || receipt_amount != Some(amount)
                        || receipt_network.as_deref() != Some(network.as_str())
                        || journal.is_none()
                        || seal.is_none()
                        || held_posting != amount_units
                        || clearing_posting != -amount_units
                },
            ))
        }
        _ => Err(AppError::Conflict(
            "reconciliation case identity is not recognized".into(),
        )),
    }
}

/// Resolution is an audited acknowledgement, never an accounting entry. A
/// critical case can close only after its exact invariant is independently
/// rechecked against immutable evidence and current projections.
pub(crate) fn resolve_case_tx(
    conn: &Connection,
    case_id: &str,
    actor: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    if actor.is_empty()
        || !(3..=64).contains(&reason.len())
        || !reason
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AppError::BadRequest(
            "a short audit reason code is required".into(),
        ));
    }
    let (code, kind, object_id, status): (String, String, String, String) = conn
        .query_row(
            "SELECT code, object_kind, object_id, status
             FROM market_finance_reconciliation_cases WHERE id = ?1",
            params![case_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(db_error("read market finance case for resolution"))?;
    if status != "open" {
        return Err(AppError::Conflict(
            "market finance case is already resolved".into(),
        ));
    }
    if CRITICAL_CODES.contains(&code.as_str())
        && critical_case_still_failed_tx(conn, &code, &kind, &object_id)?
    {
        return Err(AppError::Conflict(
            "market finance mismatch is still present".into(),
        ));
    }
    let changed = conn
        .execute(
            "UPDATE market_finance_reconciliation_cases
             SET status = 'resolved', resolved_at = ?2 WHERE id = ?1 AND status = 'open'",
            params![case_id, now],
        )
        .map_err(db_error("resolve market finance case"))?;
    if changed != 1 {
        return Err(AppError::Conflict("market finance case changed".into()));
    }
    conn.execute(
        "INSERT INTO market_finance_audit_events
            (id, actor_user_id, action, object_kind, object_id, reason_code, created_at)
         VALUES (?1, ?2, 'reconciliation_case_resolved', 'finance_case', ?3, ?4, ?5)",
        params![Uuid::new_v4().to_string(), actor, case_id, reason, now],
    )
    .map_err(db_error("audit market finance case resolution"))?;
    Ok(())
}

pub(crate) fn audit_finance_tx(conn: &Connection, now: &str) -> Result<usize, AppError> {
    let mut opened = 0;
    let mut journal = conn
        .prepare(
            "SELECT journal.id FROM market_journal_transactions journal
         LEFT JOIN market_journal_seals seal ON seal.journal_id = journal.id
         WHERE seal.journal_id IS NULL ORDER BY journal.id LIMIT 100",
        )
        .map_err(db_error("prepare unsealed market journals"))?;
    let unsealed = journal
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error("query unsealed market journals"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read unsealed market journals"))?;
    for id in unsealed {
        opened += case_tx(conn, "journal_unsealed", "journal", &id, now)?;
    }

    let mut statement = conn
        .prepare(
            "SELECT user_id FROM market_wallet_accounts
         UNION SELECT subject_id FROM market_journal_postings
           WHERE account_kind IN ('wallet_available', 'wallet_held')
         UNION SELECT user_id FROM market_wallet_holds",
        )
        .map_err(db_error("prepare wallet projection audit"))?;
    let users = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error("query wallet projection audit"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read wallet projection audit"))?;
    for user in users {
        if reconcile_wallet_tx(conn, &user).is_err() {
            opened += case_tx(conn, "wallet_projection_mismatch", "wallet", &user, now)?;
        }
    }

    let mut statement = conn
        .prepare(
            "SELECT provider_user_id FROM market_provider_earning_lots
         UNION SELECT subject_id FROM market_journal_postings
           WHERE account_kind IN ('provider_pending', 'provider_available', 'provider_held')",
        )
        .map_err(db_error("prepare Provider liability audit"))?;
    let providers = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error("query Provider liability audit"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read Provider liability audit"))?;
    for provider in providers {
        let projection = provider_summary_tx(conn, &provider);
        let ledger: Result<(i64, i64, i64), AppError> = conn.query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN account_kind = 'provider_pending' THEN signed_units ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN account_kind = 'provider_available' THEN signed_units ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN account_kind = 'provider_held' THEN signed_units ELSE 0 END), 0)
             FROM market_journal_postings WHERE subject_id = ?1
               AND account_kind IN ('provider_pending', 'provider_available', 'provider_held')",
            params![provider], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).map_err(db_error("read Provider liability journal"));
        let matches = match (projection, ledger) {
            (Ok(projection), Ok((pending, available, held))) => {
                pending == -projection.pending_units
                    && available == -projection.available_units
                    && held == -projection.withdrawal_held_units
            }
            _ => false,
        };
        if !matches {
            opened += case_tx(
                conn,
                "provider_liability_mismatch",
                "provider",
                &provider,
                now,
            )?;
        }
    }

    let mut statement = conn
        .prepare(
            "SELECT period.id FROM market_platform_periods period
         LEFT JOIN market_journal_postings posting
           ON posting.account_kind = 'unfulfilled' AND posting.subject_id = period.id
         GROUP BY period.id
         HAVING COALESCE(SUM(posting.signed_units), 0)
             != -(period.amount_units - period.fulfilled_units - period.refunded_units)
         LIMIT 100",
        )
        .map_err(db_error("prepare unfulfilled audit"))?;
    let mismatched_periods = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error("query unfulfilled audit"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read unfulfilled audit"))?;
    for id in mismatched_periods {
        opened += case_tx(conn, "period_unfulfilled_mismatch", "period", &id, now)?;
    }

    let mut statement = conn
        .prepare(
            "SELECT topup.id FROM market_topup_orders topup
         LEFT JOIN market_topup_receipts receipt ON receipt.order_id = topup.id
         LEFT JOIN market_journal_transactions journal ON journal.id = topup.credited_journal_id
         WHERE topup.status = 'credited'
           AND (receipt.order_id IS NULL OR journal.id IS NULL
                OR journal.source_kind != 'topup' OR journal.source_id != topup.id)
         LIMIT 100",
        )
        .map_err(db_error("prepare credited top-up audit"))?;
    let mismatched_topups = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error("query credited top-up audit"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read credited top-up audit"))?;
    for id in mismatched_topups {
        opened += case_tx(conn, "credited_topup_missing_evidence", "topup", &id, now)?;
    }

    for (table, kind, network_join, expected_network, journal_kind, held_kind, held_subject) in [
        (
            "market_payout_requests",
            "provider_withdrawal",
            "JOIN market_payout_destinations destination ON destination.id = payout.destination_id",
            "destination.network",
            "payout",
            "provider_held",
            "payout.provider_user_id",
        ),
        (
            "market_buyer_refund_requests",
            "buyer_refund",
            "",
            "payout.network",
            "buyer_refund",
            "buyer_refund_held",
            "payout.id",
        ),
    ] {
        let sql = format!(
            "SELECT payout.id FROM {table} payout
             {network_join}
             LEFT JOIN market_external_payout_receipts receipt
               ON receipt.object_kind = ?1 AND receipt.object_id = payout.id
             LEFT JOIN market_journal_transactions paid_journal
               ON paid_journal.source_kind = '{journal_kind}'
              AND paid_journal.source_id = payout.id AND paid_journal.action = 'paid'
             LEFT JOIN market_journal_seals paid_seal
               ON paid_seal.journal_id = paid_journal.id
             WHERE payout.status = 'paid'
               AND (receipt.id IS NULL OR receipt.external_reference IS NOT payout.external_reference
                    OR receipt.amount_minor != payout.amount_units / ?2
                    OR receipt.network != {expected_network}
                    OR paid_journal.id IS NULL OR paid_seal.journal_id IS NULL
                    OR (SELECT COALESCE(SUM(posting.signed_units), 0)
                          FROM market_journal_postings posting
                         WHERE posting.journal_id = paid_journal.id
                           AND posting.account_kind = '{held_kind}'
                           AND posting.subject_id = {held_subject}) != payout.amount_units
                    OR (SELECT COALESCE(SUM(posting.signed_units), 0)
                          FROM market_journal_postings posting
                         WHERE posting.journal_id = paid_journal.id
                           AND posting.account_kind = 'owner_clearing'
                           AND posting.subject_id = 'owner') != -payout.amount_units)
             LIMIT 100"
        );
        let mut statement = conn
            .prepare(&sql)
            .map_err(db_error("prepare paid transfer audit"))?;
        let mismatched = statement
            .query_map(
                params![kind, crate::market_billing::MONEY_UNITS_PER_MINOR],
                |row| row.get::<_, String>(0),
            )
            .map_err(db_error("query paid transfer audit"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read paid transfer audit"))?;
        for id in mismatched {
            opened += case_tx(conn, "paid_payout_missing_evidence", kind, &id, now)?;
        }
    }
    Ok(opened)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TransactionBehavior;
    use crate::market_wallet::{
        Posting, capture_hold_tx, credit_verified_topup_tx, post_journal_tx, reserve_wallet_tx,
    };

    #[test]
    fn internal_audit_finds_projection_drift_and_blocks_new_money() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let at = "2026-01-01T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 100, "receipt", at).unwrap();
        assert_eq!(audit_finance_tx(&tx, at).unwrap(), 0);
        tx.execute(
            "UPDATE market_wallet_accounts SET posted_units = posted_units - 1 WHERE user_id = 'buyer'",
            [],
        ).unwrap();
        assert_eq!(audit_finance_tx(&tx, at).unwrap(), 1);
        assert_eq!(critical_case_count_tx(&tx).unwrap(), 1);
        assert!(ensure_new_finance_writes_safe_tx(&tx).is_err());
        let case_id: String = tx
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases
                 WHERE code = 'wallet_projection_mismatch' AND object_id = 'buyer'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(resolve_case_tx(&tx, &case_id, "owner", "projection_checked", at).is_err());
        tx.execute(
            "UPDATE market_wallet_accounts SET posted_units = posted_units + 1 WHERE user_id = 'buyer'",
            [],
        )
        .unwrap();
        resolve_case_tx(&tx, &case_id, "owner", "projection_checked", at).unwrap();
        assert_eq!(critical_case_count_tx(&tx).unwrap(), 0);
        ensure_new_finance_writes_safe_tx(&tx).unwrap();
        assert!(resolve_case_tx(&tx, &case_id, "owner", "projection_checked", at).is_err());
        tx.commit().unwrap();
    }

    #[test]
    fn payout_receipt_case_requires_matching_reference_amount_and_network() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let at = "2026-01-01T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 100, "receipt", at).unwrap();
        let hold_id = reserve_wallet_tx(
            &tx,
            "buyer",
            "buyer_refund",
            "refund",
            "requested",
            8_640_000,
            None,
            at,
        )
        .unwrap();
        tx.execute(
            "INSERT INTO market_buyer_refund_requests
                (id, user_id, hold_id, idempotency_key, request_fingerprint,
                 amount_units, asset_code, network, address, status, external_reference,
                 created_at, updated_at)
             VALUES ('refund', 'buyer', ?1, 'idem', 'fp', 8640000,
                     'USDT', 'TRON', 'address', 'paid', 'transfer', ?2, ?2)",
            params![hold_id, at],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO market_external_payout_receipts
                (id, network, external_reference, object_kind, object_id,
                 amount_minor, recorded_at)
             VALUES ('receipt', 'ETHEREUM', 'transfer', 'buyer_refund', 'refund', 100, ?1)",
            params![at],
        )
        .unwrap();
        assert_eq!(audit_finance_tx(&tx, at).unwrap(), 1);
        let case_id: String = tx
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases WHERE object_id = 'refund'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(resolve_case_tx(&tx, &case_id, "owner", "receipt_checked", at).is_err());
        tx.execute(
            "UPDATE market_external_payout_receipts SET network = 'TRON' WHERE id = 'receipt'",
            [],
        )
        .unwrap();
        assert!(resolve_case_tx(&tx, &case_id, "owner", "receipt_checked", at).is_err());
        capture_hold_tx(
            &tx,
            &hold_id,
            8_640_000,
            "buyer_refund_held",
            "refund",
            "buyer_refund",
            "refund",
            at,
        )
        .unwrap();
        post_journal_tx(
            &tx,
            "buyer_refund",
            "refund",
            "paid",
            "buyer:8640000:transfer",
            &[
                Posting {
                    kind: "buyer_refund_held",
                    subject_id: "refund",
                    signed_units: 8_640_000,
                },
                Posting {
                    kind: "owner_clearing",
                    subject_id: "owner",
                    signed_units: -8_640_000,
                },
            ],
            at,
        )
        .unwrap();
        resolve_case_tx(&tx, &case_id, "owner", "receipt_checked", at).unwrap();
    }
}
