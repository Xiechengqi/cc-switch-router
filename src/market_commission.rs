//! Independently versioned service commissions. Terms belong to a quote and
//! service budget/period, never to the time a settlement worker happens to run.
use crate::market_wallet::{Posting, post_journal_tx};
use crate::market_wallet_policy::db_error;
use crate::{
    ServerState,
    db::{Connection, OptionalExtension, TransactionBehavior, params},
    error::AppError,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, header},
    routing::get,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Terms {
    pub policy_version: i64,
    pub rate_bps: i64,
    pub effective_at: String,
}

pub(crate) fn terms_at_tx(conn: &Connection, at: &str) -> Result<Terms, AppError> {
    conn.query_row("SELECT version, rate_bps, effective_at FROM market_commission_policies p
        WHERE julianday(effective_at) <= julianday(?1)
          AND NOT EXISTS (SELECT 1 FROM market_commission_policy_cancellations c WHERE c.policy_version = p.version)
        ORDER BY julianday(effective_at) DESC, version DESC LIMIT 1", params![at],
        |row| Ok(Terms { policy_version: row.get(0)?, rate_bps: row.get(1)?, effective_at: row.get(2)? }))
        .map_err(db_error("read service commission policy"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyView {
    version: i64,
    current: Terms,
    scheduled: Option<Terms>,
    increase_notice_days: i64,
    can_edit: bool,
}

fn view_tx(conn: &Connection, at: &str, can_edit: bool) -> Result<PolicyView, AppError> {
    let version = conn
        .query_row(
            "SELECT version FROM market_commission_policy_control WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .map_err(db_error("read commission control"))?;
    let scheduled = conn.query_row("SELECT version, rate_bps, effective_at FROM market_commission_policies p
        WHERE julianday(effective_at) > julianday(?1)
          AND NOT EXISTS (SELECT 1 FROM market_commission_policy_cancellations c WHERE c.policy_version = p.version)
        ORDER BY julianday(effective_at), version DESC LIMIT 1", params![at],
        |r| Ok(Terms { policy_version: r.get(0)?, rate_bps: r.get(1)?, effective_at: r.get(2)? }))
        .optional().map_err(db_error("read scheduled commission"))?;
    Ok(PolicyView {
        version,
        current: terms_at_tx(conn, at)?,
        scheduled,
        increase_notice_days: 7,
        can_edit,
    })
}

fn set_policy_tx(
    conn: &Connection,
    expected: i64,
    rate: i64,
    actor: &str,
    at: &str,
) -> Result<PolicyView, AppError> {
    if !(0..=10_000).contains(&rate) {
        return Err(AppError::BadRequest(
            "commission rate must be between 0 and 100 percent".into(),
        ));
    }
    let before = view_tx(conn, at, true)?;
    if before.version != expected {
        return Err(AppError::Conflict(
            "commission policy changed; reload before saving".into(),
        ));
    }
    let time = DateTime::parse_from_rfc3339(at)
        .map_err(|_| AppError::Internal("invalid commission policy time".into()))?
        .with_timezone(&Utc);
    let effective = if rate > before.current.rate_bps {
        time + Duration::days(7)
    } else {
        time
    };
    let version = expected
        .checked_add(1)
        .ok_or_else(|| AppError::Internal("commission version overflow".into()))?;
    if conn.execute("UPDATE market_commission_policy_control SET version = ?2 WHERE id = 1 AND version = ?1", params![expected, version])
        .map_err(db_error("update commission policy control"))? != 1 {
        return Err(AppError::Conflict("commission policy changed; reload before saving".into()));
    }
    conn.execute(
        "INSERT INTO market_commission_policies VALUES (?1, ?2, ?3, ?4, ?5)",
        params![version, rate, effective.to_rfc3339(), actor, at],
    )
    .map_err(db_error("record commission policy"))?;
    if let Some(pending) = before.scheduled.as_ref() {
        conn.execute(
            "INSERT INTO market_commission_policy_cancellations VALUES (?1, ?2, ?3)",
            params![pending.policy_version, version, at],
        )
        .map_err(db_error("supersede scheduled commission"))?;
    }
    let after = view_tx(conn, at, true)?;
    crate::market_notifications::owner_signal_tx(
        conn,
        "commission_policy_changed",
        "commission_policy",
        &version.to_string(),
        serde_json::json!({"actorUserId": actor, "before": before, "after": after}),
        at,
        false,
    )?;
    // Durable, deduplicated notices are part of the policy transaction.
    let providers = {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT provider_user_id FROM market_platform_contracts
                UNION SELECT DISTINCT provider_user_id FROM market_provider_earning_lots
                UNION SELECT user_id FROM market_provider_profiles WHERE user_id IS NOT NULL",
            )
            .map_err(db_error("prepare commission notice recipients"))?;
        stmt.query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error("query commission notice recipients"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read commission notice recipients"))?
    };
    for provider in providers {
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "provider",
            &provider,
            "commission_policy_changed",
            "commission_policy",
            &version.to_string(),
            &format!("commission_policy:{version}:{provider}"),
            serde_json::json!({"rateBps": rate, "effectiveAt": effective.to_rfc3339(), "reason": "new_service_periods_only"}),
            at,
        )?;
    }
    Ok(after)
}

pub(crate) fn freeze_quote_tx(
    conn: &Connection,
    quote: &str,
    provider: &str,
    at: &str,
) -> Result<(), AppError> {
    let terms = terms_at_tx(conn, at)?;
    conn.execute(
        "INSERT OR IGNORE INTO market_commission_quote_terms VALUES (?1, ?2, ?3, ?4, ?5)",
        params![quote, provider, terms.policy_version, terms.rate_bps, at],
    )
    .map_err(db_error("freeze quote commission"))?;
    Ok(())
}

pub(crate) fn freeze_contract_tx(
    conn: &Connection,
    contract: &str,
    quote: &str,
    provider: &str,
    at: &str,
) -> Result<(), AppError> {
    // Absence means a pre-feature obligation, not permission to backcharge it.
    let terms: Option<(i64, i64)> = conn
        .query_row(
            "SELECT policy_version, rate_bps FROM market_commission_quote_terms
        WHERE quote_id = ?1 AND provider_user_id = ?2",
            params![quote, provider],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error("read quoted commission"))?;
    let (version, rate) = terms.unwrap_or((0, 0));
    conn.execute(
        "INSERT INTO market_commission_contract_terms VALUES (?1, ?2, ?3, ?4, ?5)",
        params![contract, quote, version, rate, at],
    )
    .map_err(db_error("freeze contract commission"))?;
    Ok(())
}

pub(crate) fn freeze_period_tx(
    conn: &Connection,
    contract: &str,
    kind: &str,
    source: &str,
    first: bool,
    anchor: &str,
    at: &str,
) -> Result<(), AppError> {
    let (version, rate) = if first {
        conn.query_row("SELECT policy_version, rate_bps FROM market_commission_contract_terms WHERE contract_id = ?1", params![contract],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).optional().map_err(db_error("read initial commission terms"))?.unwrap_or((0, 0))
    } else {
        let legacy_backlog: bool = conn
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM market_commission_contract_terms
                 WHERE contract_id = ?1 AND policy_version > 0)
                AND julianday(?2) < julianday(created_at)
             FROM market_commission_policies WHERE version = 1",
                params![contract, anchor],
                |r| r.get(0),
            )
            .map_err(db_error("check pre-commission service backlog"))?;
        if legacy_backlog {
            // The default policy must not retroactively charge service which
            // predates installation merely because its worker was delayed.
            (0, 0)
        } else {
            let terms = terms_at_tx(conn, anchor)?;
            (terms.policy_version, terms.rate_bps)
        }
    };
    conn.execute(
        "INSERT INTO market_commission_periods VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            uuid::Uuid::new_v4().to_string(),
            contract,
            kind,
            source,
            version,
            rate,
            anchor,
            at
        ],
    )
    .map_err(db_error("freeze service period commission"))?;
    Ok(())
}

/// Exact cumulative round-half-up. Refunds do not reduce this gross base.
pub(crate) fn cumulative_fee(gross: i64, bps: i64) -> Result<i64, AppError> {
    if gross < 0 || !(0..=10_000).contains(&bps) {
        return Err(AppError::Internal("invalid cumulative commission".into()));
    }
    Ok(((i128::from(gross) * i128::from(bps) + 5_000) / 10_000) as i64)
}

pub(crate) struct DeliveryAllocation {
    pub fee_units: i64,
    evidence: Option<SplitEvidence>,
}
struct SplitEvidence {
    period_id: String,
    gross_before: i64,
    gross_after: i64,
    fee_before: i64,
    fee_after: i64,
    journal_id: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn allocate_delivery_tx(
    conn: &Connection,
    lot: &str,
    contract: &str,
    period: Option<&str>,
    source_kind: &str,
    gross: i64,
    provider: &str,
    at: &str,
) -> Result<DeliveryAllocation, AppError> {
    let source = if let Some(period) = period {
        Some(("monthly_period", period.to_owned()))
    } else if source_kind == "metered_batch" {
        let hold: String = conn
            .query_row(
                "SELECT budget_hold_id FROM market_platform_contracts WHERE id = ?1",
                params![contract],
                |r| r.get(0),
            )
            .map_err(db_error("read service commission budget"))?;
        Some(("metered_budget", hold))
    } else {
        None
    };
    let Some((kind, source)) = source else {
        return Ok(DeliveryAllocation {
            fee_units: 0,
            evidence: None,
        });
    };
    let terms: Option<(String, i64)> = conn.query_row("SELECT id, rate_bps FROM market_commission_periods WHERE source_kind = ?1 AND source_id = ?2 AND contract_id = ?3",
        params![kind, source, contract], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(db_error("read commission period"))?;
    let Some((period, rate)) = terms else {
        let protected: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM market_commission_contract_terms WHERE contract_id = ?1 AND policy_version > 0)", params![contract], |r| r.get(0))
            .map_err(db_error("check commission period evidence"))?;
        if protected {
            return Err(AppError::Internal(
                "service commission period missing".into(),
            ));
        }
        return Ok(DeliveryAllocation {
            fee_units: 0,
            evidence: None,
        });
    };
    let (before, fee_before): (i64, i64) = conn
        .query_row(
            "SELECT COALESCE(MAX(gross_after_units), 0), COALESCE(MAX(fee_after_units), 0)
        FROM market_commission_splits WHERE period_id = ?1",
            params![period],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(db_error("read cumulative commission"))?;
    let after = before
        .checked_add(gross)
        .ok_or_else(|| AppError::Internal("commission gross overflow".into()))?;
    if cumulative_fee(before, rate)? != fee_before {
        return Err(AppError::Internal(
            "cumulative commission evidence drift".into(),
        ));
    }
    let fee_after = cumulative_fee(after, rate)?;
    let fee = fee_after - fee_before;
    let journal = if fee > 0 {
        let journal = post_journal_tx(
            conn,
            "earning_lot",
            lot,
            "commission",
            &format!("{period}:{before}:{after}:{fee}"),
            &[
                Posting {
                    kind: "provider_pending",
                    subject_id: provider,
                    signed_units: fee,
                },
                Posting {
                    kind: "platform_commission_pending",
                    subject_id: lot,
                    signed_units: -fee,
                },
            ],
            at,
        )?;
        if !journal.inserted {
            return Err(AppError::Internal(
                "commission allocation replay drift".into(),
            ));
        }
        Some(journal.id)
    } else {
        None
    };
    Ok(DeliveryAllocation {
        fee_units: fee,
        evidence: Some(SplitEvidence {
            period_id: period,
            gross_before: before,
            gross_after: after,
            fee_before,
            fee_after,
            journal_id: journal,
        }),
    })
}

pub(crate) fn finish_delivery_tx(
    conn: &Connection,
    lot: &str,
    allocation: DeliveryAllocation,
    at: &str,
) -> Result<(), AppError> {
    if let Some(evidence) = allocation.evidence {
        conn.execute(
            "INSERT INTO market_commission_splits VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                lot,
                evidence.period_id,
                evidence.gross_before,
                evidence.gross_after,
                evidence.fee_before,
                evidence.fee_after,
                evidence.journal_id,
                at
            ],
        )
        .map_err(db_error("record commission split evidence"))?;
    }
    Ok(())
}

/// Independent of the net-balance projection: verify immutable gross/split
/// evidence, cumulative rounding, refund evidence and both commission books.
pub(crate) fn lot_evidence_valid_tx(conn: &Connection, lot: &str) -> Result<bool, AppError> {
    let basic: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM market_provider_earning_lots l
        JOIN market_delivery_slices d ON d.id = l.delivery_slice_id
        LEFT JOIN market_commission_splits s ON s.lot_id = l.id
        LEFT JOIN market_commission_periods p ON p.id = s.period_id
        WHERE l.id = ?1 AND l.amount_units = d.amount_units AND (
          (s.lot_id IS NULL AND l.commission_units = 0 AND l.commission_reversed_units = 0 AND l.commission_unlocked_units = 0
            AND NOT EXISTS(SELECT 1 FROM market_commission_contract_terms c WHERE c.contract_id = d.contract_id AND c.policy_version > 0))
          OR (s.lot_id IS NOT NULL AND p.contract_id = d.contract_id
            AND (p.source_kind != 'monthly_period' OR p.source_id = d.period_id)
            AND s.gross_after_units - s.gross_before_units = l.amount_units
            AND s.fee_after_units - s.fee_before_units = l.commission_units
            AND (s.gross_before_units = 0 AND s.fee_before_units = 0 OR EXISTS
                (SELECT 1 FROM market_commission_splits prev WHERE prev.period_id = s.period_id
                 AND prev.gross_after_units = s.gross_before_units AND prev.fee_after_units = s.fee_before_units))
            AND (p.policy_version = 0 AND p.rate_bps = 0 OR EXISTS
                (SELECT 1 FROM market_commission_policies policy WHERE policy.version = p.policy_version AND policy.rate_bps = p.rate_bps))
            AND l.reversed_units = COALESCE((SELECT SUM(net_units) FROM market_commission_refunds WHERE lot_id = l.id), 0)
            AND l.commission_reversed_units = COALESCE((SELECT SUM(fee_units) FROM market_commission_refunds WHERE lot_id = l.id), 0)
            AND EXISTS(SELECT 1 FROM market_journal_seals seal WHERE seal.journal_id = d.earning_journal_id)
            AND (SELECT COALESCE(SUM(signed_units), 0) FROM market_journal_postings
                 WHERE journal_id = d.earning_journal_id AND account_kind = 'provider_pending' AND subject_id = l.provider_user_id) = -l.amount_units
            AND (l.commission_units = 0 AND s.allocation_journal_id IS NULL OR EXISTS (
                SELECT 1 FROM market_journal_transactions j JOIN market_journal_seals seal ON seal.journal_id = j.id
                WHERE j.id = s.allocation_journal_id AND j.source_kind = 'earning_lot' AND j.source_id = l.id
                 AND j.action = 'commission' AND seal.posting_count = 2
                 AND (SELECT COALESCE(SUM(signed_units), 0) FROM market_journal_postings WHERE journal_id = j.id
                     AND account_kind = 'provider_pending' AND subject_id = l.provider_user_id) = l.commission_units
                 AND (SELECT COALESCE(SUM(signed_units), 0) FROM market_journal_postings WHERE journal_id = j.id
                     AND account_kind = 'platform_commission_pending' AND subject_id = l.id) = -l.commission_units)))))",
        params![lot], |r| r.get(0)).map_err(db_error("audit commission split evidence"))?;
    if !basic {
        return Ok(false);
    }
    let (gross, fee, reversed_net, reversed_fee): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT amount_units, commission_units, reversed_units, commission_reversed_units
         FROM market_provider_earning_lots WHERE id = ?1",
            params![lot],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(db_error("audit cumulative refund allocation"))?;
    let refunded_gross = i128::from(reversed_net) + i128::from(reversed_fee);
    if (refunded_gross * i128::from(fee) + i128::from(gross / 2)) / i128::from(gross)
        != i128::from(reversed_fee)
    {
        return Ok(false);
    }
    let split = conn.query_row("SELECT s.gross_before_units, s.gross_after_units, s.fee_before_units, s.fee_after_units, p.rate_bps
        FROM market_commission_splits s JOIN market_commission_periods p ON p.id = s.period_id WHERE s.lot_id = ?1", params![lot],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?)))
        .optional().map_err(db_error("audit cumulative commission"))?;
    if let Some((before, after, fee_before, fee_after, rate)) = split
        && (cumulative_fee(before, rate)? != fee_before
            || cumulative_fee(after, rate)? != fee_after)
    {
        return Ok(false);
    }
    let refunds_valid: bool = conn.query_row("SELECT NOT EXISTS (SELECT 1 FROM market_commission_refunds f
        JOIN market_provider_earning_lots l ON l.id = f.lot_id
        WHERE f.lot_id = ?1 AND (
          f.net_units != COALESCE((SELECT SUM(p.signed_units) FROM market_journal_transactions j
            JOIN market_journal_postings p ON p.journal_id = j.id
            WHERE j.source_kind = 'service_dispute' AND j.source_id = f.dispute_id
              AND j.action IN ('refund_pending:' || l.id, 'refund_available:' || l.id)
              AND p.account_kind IN ('provider_pending', 'provider_available') AND p.subject_id = l.provider_user_id), 0)
          OR f.fee_units != COALESCE((SELECT SUM(p.signed_units) FROM market_journal_transactions j
            JOIN market_journal_postings p ON p.journal_id = j.id
            WHERE j.source_kind = 'service_dispute' AND j.source_id = f.dispute_id
              AND j.action IN ('refund_commission_pending:' || l.id, 'refund_commission_revenue:' || l.id)
              AND p.account_kind IN ('platform_commission_pending', 'platform_commission_revenue') AND p.subject_id = l.id), 0)
          OR f.gross_units != -COALESCE((SELECT SUM(p.signed_units) FROM market_journal_transactions j
            JOIN market_journal_postings p ON p.journal_id = j.id
            WHERE j.source_kind = 'service_dispute' AND j.source_id = f.dispute_id
              AND j.action IN ('refund_pending:' || l.id, 'refund_available:' || l.id, 'refund_commission_pending:' || l.id, 'refund_commission_revenue:' || l.id)
              AND p.account_kind = 'wallet_available' AND p.subject_id = l.buyer_user_id), 0)))",
        params![lot], |r| r.get(0)).map_err(db_error("audit original refund split postings"))?;
    if !refunds_valid {
        return Ok(false);
    }
    conn.query_row("SELECT
        COALESCE((SELECT SUM(signed_units) FROM market_journal_postings WHERE account_kind = 'platform_commission_pending' AND subject_id = l.id), 0)
            = -(l.commission_units - l.commission_reversed_units - l.commission_unlocked_units)
        AND COALESCE((SELECT SUM(signed_units) FROM market_journal_postings WHERE account_kind = 'platform_commission_revenue' AND subject_id = l.id), 0)
            = -l.commission_unlocked_units
        FROM market_provider_earning_lots l WHERE l.id = ?1", params![lot], |r| r.get(0))
        .map_err(db_error("audit commission liability"))
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-wallet/commission-policy", get(get_policy))
        .route(
            "/v1/admin/market-finance/commission-policy",
            get(get_admin_policy).put(set_policy),
        )
}
async fn get_policy(
    State(state): State<ServerState>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let conn = state.store.conn.lock().await;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(view_tx(&conn, &crate::market_wallet::now(), false)?),
    ))
}
async fn get_admin_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(view_tx(
            &conn,
            &crate::market_wallet::now(),
            crate::api::is_finance_owner(&state.config, &actor.email),
        )?),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetPolicyRequest {
    expected_version: i64,
    rate_bps: i64,
}
async fn set_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<SetPolicyRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let actor = crate::api::require_owner_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin commission policy"))?;
    let view = set_policy_tx(
        &tx,
        input.expected_version,
        input.rate_bps,
        &actor.user_id,
        &crate::market_wallet::now(),
    )?;
    tx.commit().map_err(db_error("commit commission policy"))?;
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(view)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_finance::{provider_summary_tx, unlock_due_earnings_tx};
    use crate::market_platform_contracts::{
        BillingModel, PrepareContract, activate_contract_tx, observe_metered_interval_tx,
        prepare_contract_tx, settle_metered_batch_tx, settle_monthly_segment_tx,
    };
    const START: &str = "2026-01-10T00:00:00+00:00";
    fn fixture(kind: &str, model: BillingModel, rate: i64) -> (Connection, String) {
        fixture_with_trial(kind, model, rate, 0)
    }
    fn fixture_with_trial(
        kind: &str,
        model: BillingModel,
        rate: i64,
        trial_seconds: i64,
    ) -> (Connection, String) {
        let conn = crate::test_support::database().unwrap();
        set_policy_tx(&conn, 1, rate, "owner", "2026-01-01T00:00:00+00:00").unwrap();
        crate::market_wallet::credit_verified_topup_tx(
            &conn, "buyer", "topup", 100_000, "receipt", START,
        )
        .unwrap();
        freeze_quote_tx(&conn, "quote", "provider", START).unwrap();
        let id = prepare_contract_tx(
            &conn,
            &PrepareContract {
                product_kind: kind,
                product_ref: "product",
                trial_scope_ref: "service",
                quote_id: "quote",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: model,
                price_minor: 1000,
                auto_renew: model == BillingModel::CalendarMonth,
                trial_seconds,
            },
            START,
        )
        .unwrap();
        activate_contract_tx(&conn, &id, START, START).unwrap();
        (conn, id)
    }
    fn ledger(conn: &Connection, kind: &str) -> i64 {
        conn.query_row("SELECT -COALESCE(SUM(signed_units), 0) FROM market_journal_postings WHERE account_kind = ?1", params![kind], |r| r.get(0)).unwrap()
    }
    fn deliver_month(conn: &Connection, id: &str) {
        settle_monthly_segment_tx(
            conn,
            id,
            "delivery",
            START,
            "2026-02-10T00:00:00+00:00",
            true,
            "2026-02-10T00:00:00+00:00",
        )
        .unwrap();
    }
    #[test]
    fn commission_precision_is_partition_invariant() {
        for rate in [0, 1, 333, 1000, 1234, 9999, 10000] {
            let mut fee = 0;
            for gross in 1..10001 {
                fee +=
                    cumulative_fee(gross, rate).unwrap() - cumulative_fee(gross - 1, rate).unwrap();
            }
            assert_eq!(fee, cumulative_fee(10000, rate).unwrap());
        }
        assert_eq!(cumulative_fee(i64::MAX, 10000).unwrap(), i64::MAX);
        assert!(cumulative_fee(1, 10001).is_err());
    }
    #[test]
    fn monthly_gross_net_fee_and_maturity_cover_both_products_and_extremes() {
        for kind in ["share", "client_host"] {
            for rate in [0, 1000, 1234, 10000] {
                let (conn, id) = fixture(kind, BillingModel::CalendarMonth, rate);
                assert_eq!(ledger(&conn, "platform_commission_pending"), 0);
                deliver_month(&conn, &id);
                let gross = 1000 * 86400;
                let fee = cumulative_fee(gross, rate).unwrap();
                let summary = provider_summary_tx(&conn, "provider").unwrap();
                assert_eq!(
                    (
                        summary.gross_earned_units,
                        summary.commission_units,
                        summary.net_earned_units
                    ),
                    (gross, fee, gross - fee)
                );
                assert_eq!(summary.pending_units, gross - fee);
                assert_eq!(ledger(&conn, "platform_commission_pending"), fee);
                assert_eq!(
                    unlock_due_earnings_tx(&conn, "2026-02-16T23:59:59+00:00", 20).unwrap(),
                    0
                );
                assert_eq!(
                    unlock_due_earnings_tx(&conn, "2026-02-17T00:00:00+00:00", 20).unwrap(),
                    1
                );
                assert_eq!(ledger(&conn, "platform_commission_revenue"), fee);
                assert_eq!(
                    provider_summary_tx(&conn, "provider")
                        .unwrap()
                        .withdrawable_units,
                    gross - fee
                );
                assert_eq!(
                    crate::market_reconcile::audit_finance_tx(&conn, "2026-02-18T00:00:00+00:00")
                        .unwrap(),
                    0
                );
                assert_eq!(
                    unlock_due_earnings_tx(&conn, "2026-02-18T00:00:00+00:00", 20).unwrap(),
                    0
                );
            }
        }
    }
    #[test]
    fn metered_uses_frozen_budget_terms_not_worker_time() {
        for kind in ["share", "client_host"] {
            let (conn, id) = fixture(kind, BillingModel::Metered, 1000);
            set_policy_tx(&conn, 2, 0, "owner", "2026-01-11T00:00:00+00:00").unwrap();
            observe_metered_interval_tx(
                &conn,
                &id,
                if kind == "share" {
                    "share_control"
                } else {
                    "client_host_health"
                },
                "observed",
                START,
                "2026-01-10T00:01:00+00:00",
                "2026-01-20T00:00:00+00:00",
            )
            .unwrap();
            let batch = settle_metered_batch_tx(&conn, &id, "2026-01-20T00:00:00+00:00").unwrap();
            assert_eq!(batch.captured_units, 60_000);
            assert_eq!(ledger(&conn, "platform_commission_pending"), 6000);
            assert_eq!(
                provider_summary_tx(&conn, "provider")
                    .unwrap()
                    .pending_units,
                54000
            );
            assert_eq!(
                settle_metered_batch_tx(&conn, &id, "2026-01-20T00:00:00+00:00")
                    .unwrap()
                    .captured_units,
                0
            );
            assert_eq!(
                crate::market_reconcile::audit_finance_tx(&conn, START).unwrap(),
                0
            );
        }
    }
    #[test]
    fn increases_have_notice_cas_and_independent_topup_versions() {
        let conn = crate::test_support::database().unwrap();
        let before = crate::market_wallet_policy::policy_tx(&conn)
            .unwrap()
            .version;
        let next = set_policy_tx(&conn, 1, 2000, "owner", START).unwrap();
        assert_eq!(next.current.rate_bps, 1000);
        assert_eq!(next.scheduled.unwrap().rate_bps, 2000);
        assert_eq!(
            terms_at_tx(&conn, "2026-01-16T23:59:59Z").unwrap().rate_bps,
            1000
        );
        assert_eq!(
            terms_at_tx(&conn, "2026-01-17T00:00:00Z").unwrap().rate_bps,
            2000
        );
        assert!(set_policy_tx(&conn, 1, 500, "owner", START).is_err());
        set_policy_tx(&conn, 2, 500, "owner", "2026-01-11T00:00:00Z").unwrap();
        assert_eq!(
            terms_at_tx(&conn, "2026-01-18T00:00:00Z").unwrap().rate_bps,
            500
        );
        assert_eq!(
            crate::market_wallet_policy::policy_tx(&conn)
                .unwrap()
                .version,
            before
        );
        assert!(
            conn.execute("UPDATE market_commission_policies SET rate_bps = 0", [])
                .is_err()
        );
    }
    #[test]
    fn service_refund_reverses_original_net_and_fee_before_and_after_maturity() {
        for mature in [false, true] {
            for rate in [0, 1000, 10000] {
                let (conn, id) = fixture("share", BillingModel::CalendarMonth, rate);
                deliver_month(&conn, &id);
                let time = "2026-02-20T00:00:00+00:00";
                if mature {
                    unlock_due_earnings_tx(&conn, time, 20).unwrap();
                }
                let dispute = crate::market_service_disputes::open_dispute_tx(
                    &conn,
                    &id,
                    "buyer",
                    "service_quality",
                    time,
                )
                .unwrap();
                assert_eq!(dispute.refundable_units, 1000 * 86400);
                assert_eq!(unlock_due_earnings_tx(&conn, time, 20).unwrap(), 0);
                crate::market_service_disputes::resolve_dispute_tx(
                    &conn,
                    &dispute.id,
                    "owner",
                    1,
                    "partial_refund",
                    400 * 86400,
                    time,
                )
                .unwrap();
                assert!(
                    crate::market_service_disputes::resolve_dispute_tx(
                        &conn,
                        &dispute.id,
                        "owner",
                        1,
                        "partial_refund",
                        400 * 86400,
                        time
                    )
                    .is_err()
                );
                assert_eq!(
                    provider_summary_tx(&conn, "provider")
                        .unwrap()
                        .net_earned_units,
                    600 * 86400 - cumulative_fee(600 * 86400, rate).unwrap()
                );
                assert_eq!(
                    ledger(&conn, "platform_commission_pending")
                        + ledger(&conn, "platform_commission_revenue"),
                    cumulative_fee(600 * 86400, rate).unwrap()
                );
                assert_eq!(
                    crate::market_reconcile::audit_finance_tx(&conn, time).unwrap(),
                    0
                );
                let rest = crate::market_service_disputes::open_dispute_tx(
                    &conn,
                    &id,
                    "buyer",
                    "service_quality",
                    "2026-02-21T00:00:00Z",
                )
                .unwrap();
                assert_eq!(rest.refundable_units, 600 * 86400);
                crate::market_service_disputes::resolve_dispute_tx(
                    &conn,
                    &rest.id,
                    "owner",
                    1,
                    "remaining_refund",
                    600 * 86400,
                    "2026-02-21T00:00:00Z",
                )
                .unwrap();
                assert_eq!(
                    provider_summary_tx(&conn, "provider")
                        .unwrap()
                        .net_earned_units,
                    0
                );
                assert_eq!(
                    ledger(&conn, "platform_commission_pending")
                        + ledger(&conn, "platform_commission_revenue"),
                    0
                );
                assert_eq!(
                    crate::market_reconcile::audit_finance_tx(&conn, time).unwrap(),
                    0
                );
                assert!(matches!(
                    crate::market_service_disputes::open_dispute_tx(
                        &conn,
                        &id,
                        "buyer",
                        "service_quality",
                        "2026-02-22T00:00:00Z"
                    ),
                    Err(AppError::Conflict(_))
                ));
            }
        }
    }
    #[test]
    fn invalid_commission_evidence_blocks_unlock_and_opens_critical_case() {
        let (conn, id) = fixture("share", BillingModel::CalendarMonth, 1000);
        deliver_month(&conn, &id);
        conn.execute_batch("DROP TRIGGER trg_market_commission_split_update")
            .unwrap();
        conn.execute(
            "UPDATE market_commission_splits SET fee_before_units = 1",
            [],
        )
        .unwrap();
        assert!(unlock_due_earnings_tx(&conn, "2026-02-20T00:00:00Z", 20).is_err());
        assert!(crate::market_reconcile::audit_finance_tx(&conn, START).unwrap() > 0);
        assert!(crate::market_reconcile::ensure_contract_finance_safe_tx(&conn, &id).is_err());
    }

    #[test]
    fn renewal_gets_new_policy_but_committed_month_does_not() {
        let (conn, id) = fixture("share", BillingModel::CalendarMonth, 1000);
        set_policy_tx(&conn, 2, 0, "owner", "2026-01-11T00:00:00Z").unwrap();
        deliver_month(&conn, &id);
        assert_eq!(ledger(&conn, "platform_commission_pending"), 100 * 86400);
        crate::market_platform_contracts::renew_monthly_contract_tx(
            &conn,
            &id,
            "2026-02-10T00:00:00+00:00",
        )
        .unwrap();
        settle_monthly_segment_tx(
            &conn,
            &id,
            "renewed_delivery",
            "2026-02-10T00:00:00+00:00",
            "2026-03-10T00:00:00+00:00",
            true,
            "2026-03-10T00:00:00+00:00",
        )
        .unwrap();
        assert_eq!(
            provider_summary_tx(&conn, "provider")
                .unwrap()
                .net_earned_units,
            1900 * 86400
        );
        assert_eq!(ledger(&conn, "platform_commission_pending"), 100 * 86400);
        assert_eq!(
            crate::market_reconcile::audit_finance_tx(&conn, START).unwrap(),
            0
        );
    }

    #[test]
    fn unavailable_month_is_refunded_in_full_without_commission() {
        let (conn, id) = fixture("client_host", BillingModel::CalendarMonth, 1000);
        settle_monthly_segment_tx(
            &conn,
            &id,
            "unavailable",
            START,
            "2026-02-10T00:00:00+00:00",
            false,
            "2026-02-10T00:00:00+00:00",
        )
        .unwrap();
        assert_eq!(
            provider_summary_tx(&conn, "provider")
                .unwrap()
                .net_earned_units,
            0
        );
        assert_eq!(ledger(&conn, "platform_commission_pending"), 0);
        assert_eq!(
            crate::market_wallet::wallet_snapshot_tx(&conn, "buyer")
                .unwrap()
                .posted_units,
            100_000 * 86400
        );
    }

    #[test]
    fn tiny_deliveries_preserve_cumulative_rounding_and_zero_net_evidence() {
        let (conn, id) = fixture("share", BillingModel::CalendarMonth, 5000);
        let (period, journal): (String, String) = conn
            .query_row(
                "SELECT id, debit_journal_id FROM market_platform_periods WHERE contract_id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        for i in 1..=19 {
            let slice = crate::market_finance::record_monthly_delivery_tx(
                &conn,
                "share",
                &id,
                &period,
                &format!("tiny-{i}"),
                "buyer",
                "provider",
                START,
                "2026-01-10T00:00:01+00:00",
                1,
                &journal,
                START,
                START,
            )
            .unwrap();
            assert_eq!(
                crate::market_finance::record_monthly_delivery_tx(
                    &conn,
                    "share",
                    &id,
                    &period,
                    &format!("tiny-{i}"),
                    "buyer",
                    "provider",
                    START,
                    "2026-01-10T00:00:01+00:00",
                    1,
                    &journal,
                    START,
                    START
                )
                .unwrap(),
                slice
            );
        }
        assert_eq!(ledger(&conn, "platform_commission_pending"), 10);
        assert_eq!(
            provider_summary_tx(&conn, "provider")
                .unwrap()
                .pending_units,
            9
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_provider_earning_lots WHERE net_amount_units = 0",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            10
        );
        assert_eq!(
            unlock_due_earnings_tx(&conn, "2026-01-18T00:00:00Z", 100).unwrap(),
            19
        );
        assert_eq!(ledger(&conn, "platform_commission_revenue"), 10);
    }

    #[test]
    fn allocation_failure_rolls_back_gross_journal_and_split_together() {
        let (conn, id) = fixture("share", BillingModel::CalendarMonth, 1000);
        conn.execute_batch("CREATE TRIGGER test_commission_failure BEFORE INSERT ON market_commission_splits BEGIN SELECT RAISE(ABORT, 'injected failure'); END").unwrap();
        {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(
                settle_monthly_segment_tx(
                    &tx,
                    &id,
                    "failure",
                    START,
                    "2026-02-10T00:00:00+00:00",
                    true,
                    "2026-02-10T00:00:00+00:00"
                )
                .is_err()
            );
        }
        assert_eq!(
            provider_summary_tx(&conn, "provider")
                .unwrap()
                .pending_units,
            0
        );
        assert_eq!(ledger(&conn, "platform_commission_pending"), 0);
        conn.execute_batch("DROP TRIGGER test_commission_failure")
            .unwrap();
        deliver_month(&conn, &id);
        assert_eq!(ledger(&conn, "platform_commission_pending"), 100 * 86400);
    }

    #[test]
    fn policy_request_rejects_unknown_fields_and_invalid_rates() {
        assert!(
            serde_json::from_value::<SetPolicyRequest>(
                serde_json::json!({"expectedVersion":1,"rateBps":1000})
            )
            .is_ok()
        );
        assert!(
            serde_json::from_value::<SetPolicyRequest>(
                serde_json::json!({"expectedVersion":1,"rate_bps":1000})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<SetPolicyRequest>(
                serde_json::json!({"expectedVersion":1,"rateBps":1000,"canEdit":true})
            )
            .is_err()
        );
        let conn = crate::test_support::database().unwrap();
        for rate in [-1, 10001, i64::MAX] {
            assert!(set_policy_tx(&conn, 1, rate, "owner", START).is_err());
        }
        assert_eq!(view_tx(&conn, START, false).unwrap().version, 1);
    }

    #[test]
    fn policy_and_provider_notice_fail_atomically() {
        let (conn, _) = fixture("share", BillingModel::CalendarMonth, 1000);
        conn.execute_batch(
            "CREATE TRIGGER test_notice_failure BEFORE INSERT ON market_notification_outbox
            WHEN NEW.event_kind = 'commission_policy_changed'
            BEGIN SELECT RAISE(ABORT, 'injected notice failure'); END;",
        )
        .unwrap();
        {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            assert!(set_policy_tx(&tx, 2, 1500, "owner", START).is_err());
        }
        assert_eq!(view_tx(&conn, START, true).unwrap().version, 2);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM market_commission_policies", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        conn.execute_batch("DROP TRIGGER test_notice_failure;")
            .unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        set_policy_tx(&tx, 2, 1500, "owner", START).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_notification_outbox
            WHERE event_kind = 'commission_policy_changed' AND recipient_user_id = 'provider'
              AND mandatory = 1 AND json_extract(payload_json, '$.rateBps') = 1500",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn trial_is_free_and_first_paid_period_honors_quoted_commission() {
        for model in [BillingModel::Metered, BillingModel::CalendarMonth] {
            let (conn, id) = fixture_with_trial("client_host", model, 1000, 60);
            assert_eq!(ledger(&conn, "platform_commission_pending"), 0);
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM market_provider_earning_lots",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            set_policy_tx(&conn, 2, 0, "owner", "2026-01-10T00:00:30Z").unwrap();
            let end = "2026-01-10T00:01:00+00:00";
            let result = crate::market_platform_contracts::advance_trial_interval_tx(
                &conn,
                &id,
                "client_host_health",
                "trial",
                START,
                end,
                true,
                end,
            )
            .unwrap();
            assert!(result.paid_started);
            assert_eq!(ledger(&conn, "platform_commission_pending"), 0);
            assert_eq!(
                conn.query_row(
                    "SELECT rate_bps FROM market_commission_periods WHERE contract_id = ?1",
                    params![id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1000
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM market_provider_earning_lots",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
    }

    #[test]
    fn commission_audit_rotates_past_first_page_and_cannot_dismiss_corruption() {
        let (conn, id) = fixture("share", BillingModel::CalendarMonth, 1000);
        let (period, journal): (String, String) = conn
            .query_row(
                "SELECT id, debit_journal_id FROM market_platform_periods WHERE contract_id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        for i in 0..201 {
            crate::market_finance::record_monthly_delivery_tx(
                &conn,
                "share",
                &id,
                &period,
                &format!("audit-{i}"),
                "buyer",
                "provider",
                START,
                "2026-01-10T00:00:01+00:00",
                1,
                &journal,
                START,
                START,
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE market_platform_periods SET fulfilled_units = 201 WHERE id = ?1",
            params![period],
        )
        .unwrap();
        let tail: String = conn
            .query_row(
                "SELECT id FROM market_provider_earning_lots ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute_batch("DROP TRIGGER trg_market_commission_split_update;")
            .unwrap();
        conn.execute(
            "UPDATE market_commission_splits SET allocation_journal_id = ?2 WHERE lot_id = ?1",
            params![tail, journal],
        )
        .unwrap();
        crate::market_reconcile::audit_finance_tx(&conn, START).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases
            WHERE code = 'commission_evidence_mismatch'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        crate::market_reconcile::audit_finance_tx(&conn, START).unwrap();
        let case: String = conn
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases
            WHERE code = 'commission_evidence_mismatch' AND object_id = ?1",
                params![tail],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            crate::market_reconcile::resolve_case_tx(&conn, &case, "owner", "reviewed", START)
                .is_err()
        );
        crate::market_reconcile::audit_finance_tx(&conn, START).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT last_lot_id != ?1 FROM market_commission_audit_cursor WHERE id = 1",
                params![tail],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn replenished_metered_budget_uses_service_time_not_delayed_worker_time() {
        let (conn, id) = fixture("share", BillingModel::Metered, 1000);
        let hold: String = conn
            .query_row(
                "SELECT budget_hold_id FROM market_platform_contracts
            WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        crate::market_wallet::release_hold_tx(&conn, &hold, START).unwrap();
        set_policy_tx(&conn, 2, 0, "owner", "2026-01-11T00:00:00Z").unwrap();
        observe_metered_interval_tx(
            &conn,
            &id,
            "share_control",
            "budget-renewal",
            START,
            "2026-01-10T00:01:00+00:00",
            "2026-01-20T00:00:00Z",
        )
        .unwrap();
        let batch = settle_metered_batch_tx(&conn, &id, "2026-01-20T00:00:00Z").unwrap();
        assert_eq!(batch.captured_units, 60_000);
        assert_eq!(ledger(&conn, "platform_commission_pending"), 6000);
        assert_eq!(
            conn.query_row(
                "SELECT p.rate_bps FROM market_commission_periods p
            JOIN market_platform_contracts c ON c.budget_hold_id = p.source_id
            WHERE c.id = ?1 AND c.budget_sequence = 2",
                params![id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1000
        );
        assert_eq!(
            crate::market_reconcile::audit_finance_tx(&conn, START).unwrap(),
            0
        );
    }

    #[test]
    fn balanced_but_disproportionate_refund_evidence_is_rejected() {
        let (conn, id) = fixture("share", BillingModel::CalendarMonth, 1000);
        deliver_month(&conn, &id);
        let at = "2026-02-11T00:00:00Z";
        let dispute = crate::market_service_disputes::open_dispute_tx(
            &conn,
            &id,
            "buyer",
            "service_quality",
            at,
        )
        .unwrap();
        crate::market_service_disputes::resolve_dispute_tx(
            &conn,
            &dispute.id,
            "owner",
            1,
            "partial_refund",
            400 * 86400,
            at,
        )
        .unwrap();
        let lot: String = conn
            .query_row(
                "SELECT lot_id FROM market_commission_refunds
            WHERE dispute_id = ?1",
                params![dispute.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(lot_evidence_valid_tx(&conn, &lot).unwrap());
        conn.execute_batch(
            "DROP TRIGGER trg_market_commission_refund_update;
            DROP TRIGGER trg_market_journal_postings_no_update;",
        )
        .unwrap();
        conn.execute(
            "UPDATE market_commission_refunds SET net_units = net_units - 86400,
            fee_units = fee_units + 86400 WHERE lot_id = ?1",
            params![lot],
        )
        .unwrap();
        conn.execute(
            "UPDATE market_provider_earning_lots SET reversed_units = reversed_units - 86400,
            commission_reversed_units = commission_reversed_units + 86400 WHERE id = ?1",
            params![lot],
        )
        .unwrap();
        conn.execute("UPDATE market_journal_postings SET signed_units = signed_units + CASE
            WHEN account_kind IN ('provider_pending', 'wallet_available') AND journal_id IN
                (SELECT id FROM market_journal_transactions WHERE source_kind = 'service_dispute'
                 AND source_id = ?1 AND action LIKE 'refund_pending:%')
              THEN CASE WHEN account_kind = 'wallet_available' THEN 86400 ELSE -86400 END
            ELSE CASE WHEN account_kind = 'wallet_available' THEN -86400 ELSE 86400 END END
            WHERE journal_id IN (SELECT id FROM market_journal_transactions
                WHERE source_kind = 'service_dispute' AND source_id = ?1
                  AND (action LIKE 'refund_pending:%' OR action LIKE 'refund_commission_pending:%'))",
            params![dispute.id]).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COALESCE(SUM(signed_units), 0) FROM market_journal_postings",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert!(!lot_evidence_valid_tx(&conn, &lot).unwrap());
        assert!(crate::market_reconcile::audit_finance_tx(&conn, at).unwrap() > 0);
    }

    #[test]
    fn pre_feature_months_and_delayed_renewals_are_not_backcharged() {
        let conn = crate::test_support::database().unwrap();
        let installed: String = conn
            .query_row(
                "SELECT created_at FROM market_commission_policies WHERE version = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let start = (DateTime::parse_from_rfc3339(&installed).unwrap() - Duration::days(90))
            .format("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string();
        crate::market_wallet::credit_verified_topup_tx(
            &conn,
            "buyer",
            "legacy-topup",
            100_000,
            "receipt",
            &start,
        )
        .unwrap();
        // An obligation with no commission quote predates this feature.
        let id = prepare_contract_tx(
            &conn,
            &PrepareContract {
                product_kind: "share",
                product_ref: "legacy",
                trial_scope_ref: "legacy",
                quote_id: "pre-feature",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::CalendarMonth,
                price_minor: 1000,
                auto_renew: true,
                trial_seconds: 0,
            },
            &start,
        )
        .unwrap();
        activate_contract_tx(&conn, &id, &start, &start).unwrap();
        let end: String = conn
            .query_row(
                "SELECT period_end FROM market_platform_periods WHERE contract_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        settle_monthly_segment_tx(&conn, &id, "old-service", &start, &end, true, &installed)
            .unwrap();
        crate::market_platform_contracts::renew_monthly_contract_tx(&conn, &id, &end).unwrap();
        assert_eq!(ledger(&conn, "platform_commission_pending"), 0);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_commission_periods
            WHERE contract_id = ?1 AND policy_version = 0 AND rate_bps = 0",
                params![id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
    }

    #[test]
    fn orphan_commission_journal_freezes_all_scopes_until_repaired() {
        let conn = crate::test_support::database().unwrap();
        let journal = post_journal_tx(
            &conn,
            "earning_lot",
            "missing",
            "commission",
            "orphan",
            &[
                Posting {
                    kind: "provider_pending",
                    subject_id: "provider",
                    signed_units: 10,
                },
                Posting {
                    kind: "platform_commission_pending",
                    subject_id: "missing",
                    signed_units: -10,
                },
            ],
            START,
        )
        .unwrap();
        let untyped = post_journal_tx(
            &conn,
            "unexpected",
            "missing",
            "misc",
            "orphan-unknown",
            &[
                Posting {
                    kind: "provider_pending",
                    subject_id: "provider",
                    signed_units: 10,
                },
                Posting {
                    kind: "platform_commission_pending",
                    subject_id: "missing",
                    signed_units: -10,
                },
            ],
            START,
        )
        .unwrap();
        assert!(crate::market_reconcile::audit_finance_tx(&conn, START).unwrap() > 0);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases
            WHERE code = 'commission_orphan_journal' AND object_id = ?1",
                params![untyped.id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let case: String = conn
            .query_row(
                "SELECT id FROM market_finance_reconciliation_cases
            WHERE code = 'commission_orphan_journal' AND object_id = ?1",
                params![journal.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            crate::market_reconcile::resolve_case_tx(&conn, &case, "owner", "reviewed", START)
                .is_err()
        );
        assert!(
            crate::market_reconcile::ensure_finance_scopes_safe_tx(
                &conn,
                &[crate::market_reconcile::FinanceScope::User("unrelated")]
            )
            .is_err()
        );
    }
}
