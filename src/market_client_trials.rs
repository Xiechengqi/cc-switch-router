//! Per-buyer/Provider/Host trial epochs. Rent capture, never compensation or
//! refunds, grants the NEXT rental's trial in the same IMMEDIATE transaction.
use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;
use serde::Serialize;

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialState {
    pub epoch: i64,
    pub allowance_seconds: i64,
    pub consumed_seconds: i64,
    pub remaining_seconds: i64,
    pub last_reset_at: Option<String>,
}

pub(crate) fn epoch_tx(
    conn: &Connection,
    buyer: &str,
    provider: &str,
    host: &str,
) -> Result<i64, AppError> {
    Ok(conn
        .query_row(
            "SELECT current_epoch FROM client_market_trial_scopes
         WHERE buyer_user_id=?1 AND provider_user_id=?2 AND host_id=?3",
            params![buyer, provider, host],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read Client trial epoch"))?
        .unwrap_or(0))
}

pub(crate) fn state_tx(
    conn: &Connection,
    buyer: &str,
    provider: &str,
    host: &str,
    allowance_seconds: i64,
) -> Result<TrialState, AppError> {
    if buyer.is_empty()
        || provider.is_empty()
        || host.is_empty()
        || !(0..=30 * 86_400).contains(&allowance_seconds)
    {
        return Err(AppError::BadRequest("invalid Client trial scope".into()));
    }
    let epoch = epoch_tx(conn, buyer, provider, host)?;
    let consumed_seconds: i64 = conn.query_row(
        "SELECT COALESCE(SUM(s.consumed_seconds),0)
         FROM market_platform_contracts c JOIN market_platform_trial_segments s ON s.contract_id=c.id
         WHERE c.product_kind='client_host' AND c.buyer_user_id=?1 AND c.provider_user_id=?2
             AND c.trial_scope_ref=?3 AND c.client_trial_epoch=?4",
        params![buyer, provider, host, epoch], |r| r.get(0),
    ).map_err(db_error("read Client trial consumption"))?;
    let last_reset_at = conn
        .query_row(
            "SELECT created_at FROM client_market_trial_resets WHERE buyer_user_id=?1
         AND provider_user_id=?2 AND host_id=?3 AND epoch=?4",
            params![buyer, provider, host, epoch],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read Client trial restoration"))?;
    Ok(TrialState {
        epoch,
        allowance_seconds,
        consumed_seconds,
        remaining_seconds: allowance_seconds.saturating_sub(consumed_seconds).max(0),
        last_reset_at,
    })
}

pub(crate) fn validate_quote_epoch_tx(
    conn: &Connection,
    buyer: &str,
    provider: &str,
    host: &str,
    expected: i64,
) -> Result<(), AppError> {
    if epoch_tx(conn, buyer, provider, host)? != expected {
        return Err(AppError::Conflict(
            "Client trial allowance changed; request a new quote".into(),
        ));
    }
    Ok(())
}

pub(crate) fn for_contract_tx(
    conn: &Connection,
    contract: &str,
) -> Result<Option<TrialState>, AppError> {
    let identity: Option<(String, String, String)> = conn.query_row(
        "SELECT buyer_user_id,provider_user_id,trial_scope_ref FROM market_platform_contracts
         WHERE id=?1 AND product_kind='client_host' AND EXISTS (
             SELECT 1 FROM client_market_trial_resets r WHERE r.contract_id=market_platform_contracts.id
         )", params![contract],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional().map_err(db_error("read Client trial identity"))?;
    let Some((buyer, provider, host)) = identity else {
        return Ok(None);
    };
    let hours: i64 = conn
        .query_row(
            "SELECT COALESCE(h.trial_hours,p.client_hours) FROM market_trial_policy p
         LEFT JOIN router_ssh_hosts h ON h.id=?1 WHERE p.id=1",
            params![host],
            |r| r.get(0),
        )
        .map_err(db_error("read next Client trial offer"))?;
    state_tx(conn, &buyer, &provider, &host, hours * 3600).map(Some)
}

/// Monthly prepayment is a real rent debit, even if unused consideration is
/// subsequently returned or all termination compensation is waived.
pub(crate) fn paid_month_tx(
    conn: &Connection,
    contract: &str,
    period: &str,
    at: &str,
) -> Result<(), AppError> {
    let evidence: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT c.buyer_user_id,c.provider_user_id,c.trial_scope_ref,p.debit_journal_id
         FROM market_platform_contracts c JOIN market_platform_periods p ON p.contract_id=c.id
         JOIN market_journal_transactions j ON j.id=p.debit_journal_id
             AND j.source_kind='platform_period' AND j.source_id=p.id
         JOIN market_journal_seals seal ON seal.journal_id=j.id
         WHERE c.id=?1 AND c.product_kind='client_host' AND p.id=?2 AND p.amount_units>0
             AND (SELECT COALESCE(SUM(signed_units),0) FROM market_journal_postings
                 WHERE journal_id=j.id AND account_kind IN ('wallet_available','wallet_held')
                     AND subject_id=c.buyer_user_id)=p.amount_units",
            params![contract, period],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(db_error("verify Client rent debit"))?;
    let Some((buyer, provider, host, journal)) = evidence else {
        let client: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_platform_contracts
            WHERE id=?1 AND product_kind='client_host')",
                params![contract],
                |r| r.get(0),
            )
            .map_err(db_error("verify Client rent kind"))?;
        return if client {
            Err(AppError::Conflict(
                "Client trial restoration requires a sealed rent debit".into(),
            ))
        } else {
            Ok(())
        };
    };
    reset_tx(
        conn,
        &buyer,
        &provider,
        &host,
        contract,
        "monthly_period",
        period,
        &journal,
        at,
    )
}

/// A metered hold is NOT payment. The first actual capture in a continuous
/// rental restores its next trial; minute-level captures cannot stack grants.
pub(crate) fn paid_metered_tx(
    conn: &Connection,
    contract: &str,
    batch: &str,
    at: &str,
) -> Result<(), AppError> {
    let evidence: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT c.buyer_user_id,c.provider_user_id,c.trial_scope_ref,b.journal_id
         FROM market_platform_contracts c JOIN market_usage_posting_batches b ON b.contract_id=c.id
         JOIN market_journal_transactions j ON j.id=b.journal_id
             AND j.source_kind='metered_batch' AND j.source_id=b.id
         JOIN market_journal_seals seal ON seal.journal_id=j.id
         WHERE c.id=?1 AND c.product_kind='client_host' AND b.id=?2 AND b.amount_units>0
             AND (SELECT COALESCE(SUM(signed_units),0) FROM market_journal_postings
                 WHERE journal_id=j.id AND account_kind='wallet_held'
                     AND subject_id=c.buyer_user_id)=b.amount_units",
            params![contract, batch],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(db_error("verify captured Client usage"))?;
    let Some((buyer, provider, host, journal)) = evidence else {
        let client: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_platform_contracts
            WHERE id=?1 AND product_kind='client_host')",
                params![contract],
                |r| r.get(0),
            )
            .map_err(db_error("verify captured Client kind"))?;
        return if client {
            Err(AppError::Conflict(
                "Client trial restoration requires captured usage".into(),
            ))
        } else {
            Ok(())
        };
    };
    reset_tx(
        conn,
        &buyer,
        &provider,
        &host,
        contract,
        "metered_contract",
        contract,
        &journal,
        at,
    )
}

#[allow(clippy::too_many_arguments)]
fn reset_tx(
    conn: &Connection,
    buyer: &str,
    provider: &str,
    host: &str,
    contract: &str,
    source_kind: &str,
    source_id: &str,
    journal: &str,
    at: &str,
) -> Result<(), AppError> {
    let existing: Option<String> = conn.query_row(
        "SELECT contract_id FROM client_market_trial_resets WHERE source_kind=?1 AND source_id=?2",
        params![source_kind, source_id], |r| r.get(0),
    ).optional().map_err(db_error("read Client trial reset replay"))?;
    if let Some(saved) = existing {
        if saved != contract {
            return Err(AppError::Conflict(
                "Client trial reset identity mismatch".into(),
            ));
        }
        return Ok(());
    }
    conn.execute(
        "INSERT INTO client_market_trial_scopes (buyer_user_id,provider_user_id,host_id)
        VALUES (?1,?2,?3) ON CONFLICT DO NOTHING",
        params![buyer, provider, host],
    )
    .map_err(db_error("initialize Client trial scope"))?;
    let old = epoch_tx(conn, buyer, provider, host)?;
    let next = old
        .checked_add(1)
        .ok_or_else(|| AppError::Internal("Client trial epoch overflow".into()))?;
    conn.execute("INSERT INTO client_market_trial_resets
        (id,buyer_user_id,provider_user_id,host_id,epoch,contract_id,source_kind,source_id,journal_id,created_at)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![uuid::Uuid::new_v4().to_string(),buyer,provider,host,next,contract,source_kind,source_id,journal,at])
        .map_err(db_error("record Client trial restoration"))?;
    let changed = conn
        .execute(
            "UPDATE client_market_trial_scopes SET current_epoch=?4
        WHERE buyer_user_id=?1 AND provider_user_id=?2 AND host_id=?3 AND current_epoch=?5",
            params![buyer, provider, host, next, old],
        )
        .map_err(db_error("restore next Client trial"))?;
    if changed != 1 {
        return Err(AppError::Conflict("Client trial epoch changed".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
