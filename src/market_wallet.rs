//! USD obligation subledger for the platform wallet. All writers must run inside
//! the caller's IMMEDIATE market transaction; a journal and its projections are
//! never committed separately.

use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;
use crate::market_billing::MONEY_UNITS_PER_MINOR;
use chrono::Utc;
use serde::{Serialize, Serializer};
use uuid::Uuid;

const ASSET: &str = "USD";

pub(crate) fn serialize_units<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn parse_usd_minor(raw: &str) -> Result<i64, AppError> {
    let (whole, fractional) = raw.split_once('.').unwrap_or((raw, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fractional.len() > 2
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
        || raw.matches('.').count() > 1
    {
        return Err(AppError::BadRequest("invalid USD amount".into()));
    }
    let dollars = whole
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest("USD amount is too large".into()))?;
    let cents = match fractional.len() {
        0 => 0,
        1 => (fractional.as_bytes()[0] - b'0') as i64 * 10,
        _ => fractional
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest("invalid USD amount".into()))?,
    };
    dollars
        .checked_mul(100)
        .and_then(|value| value.checked_add(cents))
        .ok_or_else(|| AppError::BadRequest("USD amount is too large".into()))
}

pub(crate) fn units_from_minor(amount_minor: i64) -> Result<i64, AppError> {
    if amount_minor <= 0 {
        return Err(AppError::BadRequest("amount must be positive".into()));
    }
    amount_minor
        .checked_mul(MONEY_UNITS_PER_MINOR)
        .ok_or_else(|| AppError::BadRequest("amount is too large".into()))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Posting<'a> {
    pub kind: &'a str,
    pub subject_id: &'a str,
    pub signed_units: i64,
}

#[derive(Debug)]
pub(crate) struct JournalResult {
    pub id: String,
    pub inserted: bool,
}

fn existing_journal_tx(
    conn: &Connection,
    source_kind: &str,
    source_id: &str,
    action: &str,
    fingerprint: &str,
) -> Result<Option<JournalResult>, AppError> {
    let existing = conn
        .query_row(
            "SELECT id, fingerprint FROM market_journal_transactions
             WHERE source_kind = ?1 AND source_id = ?2 AND action = ?3",
            params![source_kind, source_id, action],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_error("read market journal"))?;
    if let Some((id, saved_fingerprint)) = existing {
        if saved_fingerprint != fingerprint {
            return Err(AppError::Conflict(
                "market journal idempotency conflict".into(),
            ));
        }
        return Ok(Some(JournalResult {
            id,
            inserted: false,
        }));
    }
    Ok(None)
}

/// A source/action pair is permanent. Replays with a different fingerprint
/// fail closed; callers update their cached balances only when inserted=true.
pub(crate) fn post_journal_tx(
    conn: &Connection,
    source_kind: &str,
    source_id: &str,
    action: &str,
    fingerprint: &str,
    postings: &[Posting<'_>],
    now: &str,
) -> Result<JournalResult, AppError> {
    if source_kind.is_empty() || source_id.is_empty() || action.is_empty() || fingerprint.is_empty()
    {
        return Err(AppError::Internal(
            "market journal source is incomplete".into(),
        ));
    }
    if let Some(existing) = existing_journal_tx(conn, source_kind, source_id, action, fingerprint)?
    {
        return Ok(existing);
    }
    if postings.len() < 2
        || postings.iter().any(|posting| {
            posting.kind.is_empty() || posting.subject_id.is_empty() || posting.signed_units == 0
        })
        || postings
            .iter()
            .map(|posting| posting.signed_units as i128)
            .sum::<i128>()
            != 0
    {
        return Err(AppError::Internal("unbalanced market journal".into()));
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO market_journal_transactions (
            id, source_kind, source_id, action, asset_code, fingerprint, created_at
         ) VALUES (?1, ?2, ?3, ?4, 'USD', ?5, ?6)",
        params![id, source_kind, source_id, action, fingerprint, now],
    )
    .map_err(db_error("insert market journal"))?;
    for posting in postings {
        conn.execute(
            "INSERT INTO market_journal_postings (
                id, journal_id, account_kind, subject_id, asset_code, signed_units, created_at
             ) VALUES (?1, ?2, ?3, ?4, 'USD', ?5, ?6)",
            params![
                Uuid::new_v4().to_string(),
                id,
                posting.kind,
                posting.subject_id,
                posting.signed_units,
                now,
            ],
        )
        .map_err(db_error("insert market posting"))?;
    }
    conn.execute(
        "INSERT INTO market_journal_seals (journal_id, posting_count, sealed_at)
         VALUES (?1, ?2, ?3)",
        params![id, postings.len() as i64, now],
    )
    .map_err(db_error("seal market journal"))?;
    Ok(JournalResult { id, inserted: true })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WalletSnapshot {
    pub asset_code: &'static str,
    #[serde(serialize_with = "serialize_units")]
    pub posted_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub held_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub available_units: i64,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletQuoteFunding {
    pub asset_code: &'static str,
    #[serde(serialize_with = "serialize_units")]
    pub available_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub held_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub committed_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub required_units: i64,
    #[serde(serialize_with = "serialize_units")]
    pub shortfall_units: i64,
}

pub(crate) fn wallet_quote_funding_tx(
    conn: &Connection,
    user_id: &str,
    price_minor: i64,
    trial_seconds_remaining: i64,
    now: &str,
) -> Result<WalletQuoteFunding, AppError> {
    if price_minor < 0 || trial_seconds_remaining < 0 {
        return Err(AppError::BadRequest("invalid wallet quote amount".into()));
    }
    let snapshot = wallet_snapshot_tx(conn, user_id)?;
    let required_units = if trial_seconds_remaining > 0 || price_minor == 0 {
        0
    } else {
        units_from_minor(price_minor)?
    };
    let committed_units = if required_units > 0 {
        crate::market_platform_contracts::prior_due_funding_units_tx(conn, user_id, None, now)?
    } else {
        0
    };
    let total_required = required_units
        .checked_add(committed_units)
        .ok_or_else(|| AppError::Internal("wallet quote commitments overflow".into()))?;
    Ok(WalletQuoteFunding {
        asset_code: ASSET,
        available_units: snapshot.available_units,
        held_units: snapshot.held_units,
        committed_units,
        required_units,
        shortfall_units: total_required.saturating_sub(snapshot.available_units),
    })
}

pub(crate) fn wallet_snapshot_tx(
    conn: &Connection,
    user_id: &str,
) -> Result<WalletSnapshot, AppError> {
    let row = conn
        .query_row(
            "SELECT posted_units, held_units, version FROM market_wallet_accounts
             WHERE user_id = ?1 AND asset_code = 'USD'",
            params![user_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("read market wallet"))?;
    let (posted_units, held_units, version) = row.unwrap_or((0, 0, 0));
    Ok(WalletSnapshot {
        asset_code: ASSET,
        posted_units,
        held_units,
        available_units: posted_units - held_units,
        version,
    })
}

pub(crate) fn ensure_wallet_tx(
    conn: &Connection,
    user_id: &str,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR IGNORE INTO market_wallet_accounts (
            user_id, asset_code, posted_units, held_units, version, created_at, updated_at
         ) VALUES (?1, 'USD', 0, 0, 1, ?2, ?2)",
        params![user_id, now],
    )
    .map_err(db_error("ensure market wallet"))?;
    Ok(())
}

pub(crate) fn credit_verified_topup_tx(
    conn: &Connection,
    user_id: &str,
    order_id: &str,
    amount_minor: i64,
    receipt_fingerprint: &str,
    now: &str,
) -> Result<JournalResult, AppError> {
    let (credit_minor, fee_minor) = topup_credit_terms_tx(conn, order_id, amount_minor)?;
    let units = units_from_minor(credit_minor)?;
    let gross_units = units_from_minor(amount_minor)?;
    ensure_wallet_tx(conn, user_id, now)?;
    let fingerprint = format!(
        "{user_id}:{amount_minor}:{receipt_fingerprint}{}",
        if fee_minor > 0 {
            format!(":credit={credit_minor}:fee={fee_minor}")
        } else {
            String::new()
        }
    );
    let mut postings = vec![
        Posting {
            kind: "owner_clearing",
            subject_id: "owner",
            signed_units: gross_units,
        },
        Posting {
            kind: "wallet_available",
            subject_id: user_id,
            signed_units: -units,
        },
    ];
    if fee_minor > 0 {
        postings.push(Posting {
            kind: "platform_fee_revenue",
            subject_id: "owner",
            signed_units: -units_from_minor(fee_minor)?,
        });
    }
    let result = post_journal_tx(
        conn,
        "topup",
        order_id,
        "credit",
        &fingerprint,
        &postings,
        now,
    )?;
    if result.inserted {
        if conn
            .execute(
                "UPDATE market_wallet_accounts
                 SET posted_units = posted_units + ?2, version = version + 1, updated_at = ?3
                 WHERE user_id = ?1 AND asset_code = 'USD' AND posted_units <= ?4",
                params![user_id, units, now, i64::MAX - units],
            )
            .map_err(db_error("credit market wallet"))?
            != 1
        {
            return Err(AppError::Internal("market wallet credit overflow".into()));
        }
    }
    Ok(result)
}

/// Credit a top-up the Owner settled from merchant evidence. It posts like a
/// verified credit but under its own action, so it never counts as on-chain
/// proof for channel promotion or cutover readiness.
pub(crate) fn credit_manual_topup_tx(
    conn: &Connection,
    user_id: &str,
    order_id: &str,
    amount_minor: i64,
    receipt_fingerprint: &str,
    now: &str,
) -> Result<JournalResult, AppError> {
    let (credit_minor, fee_minor) = topup_credit_terms_tx(conn, order_id, amount_minor)?;
    let units = units_from_minor(credit_minor)?;
    let mut postings = vec![
        Posting {
            kind: "owner_clearing",
            subject_id: "owner",
            signed_units: units_from_minor(amount_minor)?,
        },
        Posting {
            kind: "wallet_available",
            subject_id: user_id,
            signed_units: -units,
        },
    ];
    if fee_minor > 0 {
        postings.push(Posting {
            kind: "platform_fee_revenue",
            subject_id: "owner",
            signed_units: -units_from_minor(fee_minor)?,
        });
    }
    ensure_wallet_tx(conn, user_id, now)?;
    let result = post_journal_tx(
        conn,
        "topup",
        order_id,
        "credit_manual",
        &format!(
            "manual:{user_id}:{amount_minor}:{receipt_fingerprint}:credit={credit_minor}:fee={fee_minor}"
        ),
        &postings,
        now,
    )?;
    if result.inserted
        && conn
            .execute(
                "UPDATE market_wallet_accounts
                 SET posted_units = posted_units + ?2, version = version + 1, updated_at = ?3
                 WHERE user_id = ?1 AND asset_code = 'USD' AND posted_units <= ?4",
                params![user_id, units, now, i64::MAX - units],
            )
            .map_err(db_error("credit market wallet"))?
            != 1
    {
        return Err(AppError::Internal("market wallet credit overflow".into()));
    }
    Ok(result)
}

pub(crate) fn topup_credit_terms_tx(
    conn: &Connection,
    order_id: &str,
    gross_minor: i64,
) -> Result<(i64, i64), AppError> {
    let terms = conn
        .query_row(
            "SELECT amount_minor, COALESCE(credit_amount_minor, amount_minor), platform_fee_minor
         FROM market_topup_orders WHERE id = ?1",
            params![order_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("read frozen top-up amounts"))?;
    let Some((gross, credit, fee)) = terms else {
        // Low-level ledger fixtures do not create gateway orders. Production
        // must never mint a credit without its frozen financial terms.
        #[cfg(test)]
        return Ok((gross_minor, 0));
        #[cfg(not(test))]
        return Err(AppError::NotFound(
            "top-up financial terms not found".into(),
        ));
    };
    if gross != gross_minor || credit <= 0 || fee < 0 || credit.checked_add(fee) != Some(gross) {
        return Err(AppError::Conflict(
            "top-up financial terms do not match".into(),
        ));
    }
    Ok((credit, fee))
}

/// A partial external refund reverses its proportional fee, not wallet money
/// the buyer never received. The current fee configuration is irrelevant.
pub(crate) fn topup_reversal_terms_tx(
    conn: &Connection,
    order_id: &str,
    refund_minor: i64,
) -> Result<(i64, i64), AppError> {
    let terms = conn
        .query_row(
            "SELECT amount_minor, platform_fee_minor FROM market_topup_orders WHERE id = ?1",
            params![order_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(db_error("read top-up reversal amounts"))?;
    let Some((gross, fee)) = terms else {
        #[cfg(test)]
        return Ok((refund_minor, 0));
        #[cfg(not(test))]
        return Err(AppError::NotFound(
            "top-up financial terms not found".into(),
        ));
    };
    let prior: i64 = conn.query_row("SELECT COALESCE(SUM(amount_minor), 0) FROM market_topup_owner_actions WHERE order_id = ?1 AND action = 'reverse_external_refund'",
        params![order_id], |row| row.get(0)).map_err(db_error("sum prior top-up reversals"))?;
    let cumulative = prior
        .checked_add(refund_minor)
        .ok_or_else(|| AppError::BadRequest("reversal overflow".into()))?;
    if refund_minor <= 0 || cumulative > gross {
        return Err(AppError::BadRequest(
            "reversal exceeds the original unreversed payment".into(),
        ));
    }
    // Cumulative rounding makes a final full reversal exactly undo the fee.
    let fee_at = |amount: i64| {
        (i128::from(amount) * i128::from(fee) + i128::from(gross) / 2) / i128::from(gross)
    };
    let reversed_fee = i64::try_from(fee_at(cumulative) - fee_at(prior))
        .map_err(|_| AppError::Internal("refund fee overflow".into()))?;
    Ok((refund_minor - reversed_fee, reversed_fee))
}

/// Reverse part or all of a credited top-up after an external refund. The
/// buyer's available balance covers what it can; the Owner absorbs the rest.
/// Returns the journal and the shortfall in minor units.
pub(crate) fn reverse_topup_credit_tx(
    conn: &Connection,
    user_id: &str,
    order_id: &str,
    amount_minor: i64,
    evidence_reference: &str,
    now: &str,
) -> Result<(JournalResult, i64), AppError> {
    let (principal_minor, reversed_fee_minor) =
        topup_reversal_terms_tx(conn, order_id, amount_minor)?;
    let total = principal_minor
        .checked_mul(MONEY_UNITS_PER_MINOR)
        .ok_or_else(|| AppError::BadRequest("reversal overflow".into()))?;
    let available = wallet_snapshot_tx(conn, user_id)?.available_units.max(0);
    // Debit whole cents only, so the shortfall stays an exact cent amount.
    let per_minor = crate::market_billing::MONEY_UNITS_PER_MINOR;
    let from_wallet = total.min(available / per_minor * per_minor);
    let shortfall = total - from_wallet;
    let mut postings = vec![Posting {
        kind: "owner_clearing",
        subject_id: "owner",
        signed_units: -units_from_minor(amount_minor)?,
    }];
    if reversed_fee_minor > 0 {
        postings.push(Posting {
            kind: "platform_fee_revenue",
            subject_id: "owner",
            signed_units: units_from_minor(reversed_fee_minor)?,
        });
    }
    if from_wallet > 0 {
        postings.push(Posting {
            kind: "wallet_available",
            subject_id: user_id,
            signed_units: from_wallet,
        });
    }
    if shortfall > 0 {
        postings.push(Posting {
            kind: "owner_loss",
            subject_id: "owner",
            signed_units: shortfall,
        });
    }
    let result = post_journal_tx(
        conn,
        "topup",
        order_id,
        &format!("reverse:{evidence_reference}"),
        &format!("reverse:{user_id}:{amount_minor}:{from_wallet}:{evidence_reference}"),
        &postings,
        now,
    )?;
    if result.inserted
        && from_wallet > 0
        && conn
            .execute(
                "UPDATE market_wallet_accounts
                 SET posted_units = posted_units - ?2, version = version + 1, updated_at = ?3
                 WHERE user_id = ?1 AND asset_code = 'USD' AND posted_units - held_units >= ?2",
                params![user_id, from_wallet, now],
            )
            .map_err(db_error("reverse market wallet credit"))?
            != 1
    {
        return Err(AppError::Conflict(
            "wallet balance changed during reversal".into(),
        ));
    }
    Ok((result, shortfall / per_minor))
}

pub(crate) fn reserve_wallet_tx(
    conn: &Connection,
    user_id: &str,
    source_kind: &str,
    source_id: &str,
    purpose: &str,
    amount_units: i64,
    expires_at: Option<&str>,
    now: &str,
) -> Result<String, AppError> {
    if amount_units <= 0 {
        return Err(AppError::BadRequest("hold amount must be positive".into()));
    }
    let existing = conn
        .query_row(
            "SELECT id, user_id, amount_units, status FROM market_wallet_holds
             WHERE source_kind = ?1 AND source_id = ?2 AND purpose = ?3",
            params![source_kind, source_id, purpose],
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
        .map_err(db_error("read market hold"))?;
    if let Some((id, saved_user_id, saved_amount, status)) = existing {
        if saved_user_id != user_id || saved_amount != amount_units {
            return Err(AppError::Conflict(
                "market hold idempotency conflict".into(),
            ));
        }
        if status != "active" {
            return Err(AppError::Conflict(
                "market hold has already completed".into(),
            ));
        }
        return Ok(id);
    }
    ensure_wallet_tx(conn, user_id, now)?;
    let available = wallet_snapshot_tx(conn, user_id)?.available_units;
    if available < amount_units {
        return Err(AppError::Conflict(
            "insufficient market wallet balance".into(),
        ));
    }
    let id = Uuid::new_v4().to_string();
    let fingerprint = format!("{user_id}:{amount_units}:{purpose}");
    let result = post_journal_tx(
        conn,
        source_kind,
        source_id,
        &format!("reserve:{purpose}"),
        &fingerprint,
        &[
            Posting {
                kind: "wallet_available",
                subject_id: user_id,
                signed_units: amount_units,
            },
            Posting {
                kind: "wallet_held",
                subject_id: user_id,
                signed_units: -amount_units,
            },
        ],
        now,
    )?;
    if !result.inserted {
        return Err(AppError::Internal(
            "market hold journal is missing its hold".into(),
        ));
    }
    conn.execute(
        "INSERT INTO market_wallet_holds (
            id, user_id, asset_code, source_kind, source_id, purpose,
            amount_units, remaining_units, status, created_at, updated_at, expires_at
         ) VALUES (?1, ?2, 'USD', ?3, ?4, ?5, ?6, ?6, 'active', ?7, ?7, ?8)",
        params![
            id,
            user_id,
            source_kind,
            source_id,
            purpose,
            amount_units,
            now,
            expires_at
        ],
    )
    .map_err(db_error("insert market hold"))?;
    if conn
        .execute(
            "UPDATE market_wallet_accounts
             SET held_units = held_units + ?2, version = version + 1, updated_at = ?3
             WHERE user_id = ?1 AND asset_code = 'USD'
               AND posted_units - held_units >= ?2",
            params![user_id, amount_units, now],
        )
        .map_err(db_error("reserve market wallet"))?
        != 1
    {
        return Err(AppError::Conflict(
            "insufficient market wallet balance".into(),
        ));
    }
    Ok(id)
}

/// Captures all or part of a hold into an unfulfilled or provider-pending
/// liability. The delivery/earning writer supplies a stable action identity.
pub(crate) fn capture_hold_tx(
    conn: &Connection,
    hold_id: &str,
    amount_units: i64,
    target_kind: &str,
    target_id: &str,
    action_kind: &str,
    action_id: &str,
    now: &str,
) -> Result<JournalResult, AppError> {
    if amount_units <= 0
        || !matches!(
            target_kind,
            "unfulfilled" | "provider_pending" | "buyer_refund_held"
        )
    {
        return Err(AppError::BadRequest("invalid market hold capture".into()));
    }
    let (user_id, remaining_units, status): (String, i64, String) = conn
        .query_row(
            "SELECT user_id, remaining_units, status FROM market_wallet_holds WHERE id = ?1",
            params![hold_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db_error("read market hold for capture"))?;
    let fingerprint = format!("{hold_id}:{amount_units}:{target_kind}:{target_id}");
    if let Some(existing) =
        existing_journal_tx(conn, action_kind, action_id, "capture", &fingerprint)?
    {
        return Ok(existing);
    }
    if status != "active" || remaining_units < amount_units {
        return Err(AppError::Conflict(
            "market hold is not available for capture".into(),
        ));
    }
    let result = post_journal_tx(
        conn,
        action_kind,
        action_id,
        "capture",
        &fingerprint,
        &[
            Posting {
                kind: "wallet_held",
                subject_id: &user_id,
                signed_units: amount_units,
            },
            Posting {
                kind: target_kind,
                subject_id: target_id,
                signed_units: -amount_units,
            },
        ],
        now,
    )?;
    debug_assert!(result.inserted);
    if conn
        .execute(
            "UPDATE market_wallet_holds
             SET remaining_units = remaining_units - ?2,
                 status = CASE WHEN remaining_units = ?2 THEN 'captured' ELSE 'active' END,
                 updated_at = ?3
             WHERE id = ?1 AND status = 'active' AND remaining_units >= ?2",
            params![hold_id, amount_units, now],
        )
        .map_err(db_error("capture market hold"))?
        != 1
    {
        return Err(AppError::Conflict("market hold capture raced".into()));
    }
    if conn
        .execute(
            "UPDATE market_wallet_accounts
             SET posted_units = posted_units - ?2, held_units = held_units - ?2,
                 version = version + 1, updated_at = ?3
             WHERE user_id = ?1 AND asset_code = 'USD' AND held_units >= ?2",
            params![user_id, amount_units, now],
        )
        .map_err(db_error("debit captured market wallet"))?
        != 1
    {
        return Err(AppError::Internal(
            "market wallet hold projection drift".into(),
        ));
    }
    Ok(result)
}

/// Direct debit is for earned metered slices and due renewals. It cannot use
/// reserved money; active service budgets must capture their own hold instead.
pub(crate) fn debit_available_tx(
    conn: &Connection,
    user_id: &str,
    amount_units: i64,
    target_kind: &str,
    target_id: &str,
    source_kind: &str,
    source_id: &str,
    action: &str,
    now: &str,
) -> Result<JournalResult, AppError> {
    if amount_units <= 0 || !matches!(target_kind, "unfulfilled" | "provider_pending") {
        return Err(AppError::BadRequest("invalid market wallet debit".into()));
    }
    let fingerprint = format!("{user_id}:{amount_units}:{target_kind}:{target_id}");
    if let Some(existing) = existing_journal_tx(conn, source_kind, source_id, action, &fingerprint)?
    {
        return Ok(existing);
    }
    if wallet_snapshot_tx(conn, user_id)?.available_units < amount_units {
        return Err(AppError::Conflict(
            "insufficient market wallet balance".into(),
        ));
    }
    let result = post_journal_tx(
        conn,
        source_kind,
        source_id,
        action,
        &fingerprint,
        &[
            Posting {
                kind: "wallet_available",
                subject_id: user_id,
                signed_units: amount_units,
            },
            Posting {
                kind: target_kind,
                subject_id: target_id,
                signed_units: -amount_units,
            },
        ],
        now,
    )?;
    debug_assert!(result.inserted);
    if conn
        .execute(
            "UPDATE market_wallet_accounts
             SET posted_units = posted_units - ?2, version = version + 1, updated_at = ?3
             WHERE user_id = ?1 AND asset_code = 'USD' AND posted_units - held_units >= ?2",
            params![user_id, amount_units, now],
        )
        .map_err(db_error("debit market wallet"))?
        != 1
    {
        return Err(AppError::Conflict(
            "insufficient market wallet balance".into(),
        ));
    }
    Ok(result)
}

pub(crate) fn credit_wallet_from_liability_tx(
    conn: &Connection,
    user_id: &str,
    amount_units: i64,
    liability_kind: &str,
    liability_id: &str,
    source_kind: &str,
    source_id: &str,
    action: &str,
    now: &str,
) -> Result<JournalResult, AppError> {
    if amount_units <= 0
        || !matches!(
            liability_kind,
            "unfulfilled"
                | "provider_pending"
                | "provider_available"
                | "provider_held"
                | "buyer_refund_held"
                | "platform_commission_pending"
                | "platform_commission_revenue"
        )
    {
        return Err(AppError::BadRequest("invalid wallet refund source".into()));
    }
    let fingerprint = format!("{user_id}:{amount_units}:{liability_kind}:{liability_id}");
    if let Some(existing) = existing_journal_tx(conn, source_kind, source_id, action, &fingerprint)?
    {
        return Ok(existing);
    }
    let signed_balance: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(signed_units), 0) FROM market_journal_postings
             WHERE account_kind = ?1 AND subject_id = ?2",
            params![liability_kind, liability_id],
            |row| row.get(0),
        )
        .map_err(db_error("read refundable market liability"))?;
    if signed_balance > -amount_units {
        return Err(AppError::Conflict(
            "refundable market liability is insufficient".into(),
        ));
    }
    ensure_wallet_tx(conn, user_id, now)?;
    let result = post_journal_tx(
        conn,
        source_kind,
        source_id,
        action,
        &fingerprint,
        &[
            Posting {
                kind: liability_kind,
                subject_id: liability_id,
                signed_units: amount_units,
            },
            Posting {
                kind: "wallet_available",
                subject_id: user_id,
                signed_units: -amount_units,
            },
        ],
        now,
    )?;
    debug_assert!(result.inserted);
    if conn
        .execute(
            "UPDATE market_wallet_accounts
             SET posted_units = posted_units + ?2, version = version + 1, updated_at = ?3
             WHERE user_id = ?1 AND asset_code = 'USD' AND posted_units <= ?4",
            params![user_id, amount_units, now, i64::MAX - amount_units],
        )
        .map_err(db_error("credit refunded market wallet"))?
        != 1
    {
        return Err(AppError::Internal("market wallet refund overflow".into()));
    }
    Ok(result)
}

pub(crate) fn release_hold_tx(
    conn: &Connection,
    hold_id: &str,
    now: &str,
) -> Result<Option<JournalResult>, AppError> {
    let (user_id, remaining_units, status): (String, i64, String) = conn
        .query_row(
            "SELECT user_id, remaining_units, status FROM market_wallet_holds WHERE id = ?1",
            params![hold_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db_error("read market hold for release"))?;
    if status != "active" {
        return Ok(None);
    }
    let fingerprint = format!("{hold_id}:{remaining_units}:release");
    let result = post_journal_tx(
        conn,
        "hold",
        hold_id,
        "release",
        &fingerprint,
        &[
            Posting {
                kind: "wallet_held",
                subject_id: &user_id,
                signed_units: remaining_units,
            },
            Posting {
                kind: "wallet_available",
                subject_id: &user_id,
                signed_units: -remaining_units,
            },
        ],
        now,
    )?;
    if !result.inserted {
        return Err(AppError::Internal(
            "market hold release journal projection drift".into(),
        ));
    }
    conn.execute(
        "UPDATE market_wallet_holds
         SET remaining_units = 0, status = 'released', updated_at = ?2
         WHERE id = ?1 AND status = 'active'",
        params![hold_id, now],
    )
    .map_err(db_error("release market hold"))?;
    if conn
        .execute(
            "UPDATE market_wallet_accounts
             SET held_units = held_units - ?2, version = version + 1, updated_at = ?3
             WHERE user_id = ?1 AND asset_code = 'USD' AND held_units >= ?2",
            params![user_id, remaining_units, now],
        )
        .map_err(db_error("unfreeze market wallet"))?
        != 1
    {
        return Err(AppError::Internal(
            "market wallet hold projection drift".into(),
        ));
    }
    Ok(Some(result))
}

pub(crate) fn reconcile_wallet_tx(conn: &Connection, user_id: &str) -> Result<(), AppError> {
    let snapshot = wallet_snapshot_tx(conn, user_id)?;
    let (available_signed, held_signed): (i64, i64) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN account_kind = 'wallet_available' THEN signed_units ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN account_kind = 'wallet_held' THEN signed_units ELSE 0 END), 0)
             FROM market_journal_postings
             WHERE subject_id = ?1 AND account_kind IN ('wallet_available', 'wallet_held')",
            params![user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("reconcile market wallet journal"))?;
    let hold_sum: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(remaining_units), 0) FROM market_wallet_holds
             WHERE user_id = ?1 AND status = 'active'",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(db_error("reconcile market wallet holds"))?;
    if snapshot.available_units != -available_signed
        || snapshot.held_units != -held_signed
        || snapshot.held_units != hold_sum
    {
        return Err(AppError::Internal(
            "market wallet reconciliation mismatch".into(),
        ));
    }
    Ok(())
}

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TransactionBehavior;

    fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("open memory DB");
        crate::schema::apply(&conn).expect("apply market schema");
        conn
    }

    #[test]
    fn exact_usd_amounts_never_use_floating_point() {
        assert_eq!(parse_usd_minor("1.00").unwrap(), 100);
        assert_eq!(parse_usd_minor("0.1").unwrap(), 10);
        assert_eq!(units_from_minor(100).unwrap(), 8_640_000);
        for invalid in ["-1", "+1", "1.001", "1.2.3", "1e2", "", ".5"] {
            assert!(parse_usd_minor(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn topup_hold_capture_and_replay_are_zero_sum() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let time = now();
        let first =
            credit_verified_topup_tx(&tx, "buyer", "order-1", 100, "receipt-1", &time).unwrap();
        assert!(first.inserted);
        assert!(
            !credit_verified_topup_tx(&tx, "buyer", "order-1", 100, "receipt-1", &time)
                .unwrap()
                .inserted
        );
        assert!(
            credit_verified_topup_tx(&tx, "buyer", "order-1", 100, "different", &time).is_err()
        );
        let hold = reserve_wallet_tx(
            &tx,
            "buyer",
            "quote",
            "q-1",
            "activation",
            4_320_000,
            None,
            &time,
        )
        .unwrap();
        assert_eq!(
            reserve_wallet_tx(
                &tx,
                "buyer",
                "quote",
                "q-1",
                "activation",
                4_320_000,
                None,
                &time
            )
            .unwrap(),
            hold
        );
        let capture = capture_hold_tx(
            &tx,
            &hold,
            2_160_000,
            "unfulfilled",
            "period-1",
            "period",
            "period-1",
            &time,
        )
        .unwrap();
        assert!(capture.inserted);
        assert!(
            !capture_hold_tx(
                &tx,
                &hold,
                2_160_000,
                "unfulfilled",
                "period-1",
                "period",
                "period-1",
                &time
            )
            .unwrap()
            .inserted
        );
        release_hold_tx(&tx, &hold, &time).unwrap();
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().available_units,
            6_480_000
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn unbalanced_and_mutated_journals_fail_closed() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let time = now();
        let error = post_journal_tx(
            &tx,
            "test",
            "bad",
            "post",
            "bad",
            &[
                Posting {
                    kind: "owner_clearing",
                    subject_id: "owner",
                    signed_units: 2,
                },
                Posting {
                    kind: "wallet_available",
                    subject_id: "buyer",
                    signed_units: -1,
                },
            ],
            &time,
        );
        assert!(error.is_err());
        let result = credit_verified_topup_tx(&tx, "buyer", "order", 1, "receipt", &time).unwrap();
        assert!(
            tx.execute(
                "UPDATE market_journal_transactions SET fingerprint = 'bad' WHERE id = ?1",
                params![result.id]
            )
            .is_err()
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
    }

    #[test]
    fn liability_refund_cannot_mint_more_than_the_original_debit() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let time = now();
        credit_verified_topup_tx(&tx, "buyer", "topup", 100, "receipt", &time).unwrap();
        let debit = debit_available_tx(
            &tx,
            "buyer",
            4_320_000,
            "unfulfilled",
            "period",
            "period",
            "period",
            "debit",
            &time,
        )
        .unwrap();
        assert!(debit.inserted);
        let refund = credit_wallet_from_liability_tx(
            &tx,
            "buyer",
            4_320_000,
            "unfulfilled",
            "period",
            "refund",
            "refund-1",
            "credit",
            &time,
        )
        .unwrap();
        assert!(refund.inserted);
        assert!(
            !credit_wallet_from_liability_tx(
                &tx,
                "buyer",
                4_320_000,
                "unfulfilled",
                "period",
                "refund",
                "refund-1",
                "credit",
                &time,
            )
            .unwrap()
            .inserted
        );
        assert!(
            credit_wallet_from_liability_tx(
                &tx,
                "buyer",
                1,
                "unfulfilled",
                "period",
                "refund",
                "refund-2",
                "credit",
                &time,
            )
            .is_err()
        );
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            8_640_000
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
    }
}
