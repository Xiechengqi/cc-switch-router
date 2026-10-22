//! Manually settled USDT/TRON withdrawals. Approval never means that an
//! external transfer happened; uncertain transfers remain frozen.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::ServerState;
use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use crate::error::AppError;
use crate::market_finance::{ProviderFinanceSummary, provider_summary_tx};
use crate::market_wallet::{
    Posting, credit_wallet_from_liability_tx, now, post_journal_tx, serialize_units,
    units_from_minor,
};
#[cfg(test)]
use crate::market_wallet::{capture_hold_tx, reserve_wallet_tx};
use crate::models::AuthSession;

const MIN_WITHDRAW_MINOR: i64 = 100;
const MAX_WITHDRAW_MINOR: i64 = 500_000;
const MAX_OPEN_WITHDRAWALS: i64 = 3;
const ADDRESS_CODE_MINUTES: i64 = 15;
const ADDRESS_CODE_ATTEMPTS: i64 = 5;
const ADDRESS_COOLDOWN_HOURS: i64 = 24;

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, AppError> {
    headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            value.len() >= 8
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or_else(|| AppError::BadRequest("valid Idempotency-Key required".into()))
}

fn valid_tron_address(address: &str) -> bool {
    let Ok(decoded) = bs58::decode(address).into_vec() else {
        return false;
    };
    if decoded.len() != 25 || decoded[0] != 0x41 {
        return false;
    }
    let first = Sha256::digest(&decoded[..21]);
    let second = Sha256::digest(first);
    second[..4] == decoded[21..]
}

fn validate_destination(network: &str, address: &str) -> Result<(), AppError> {
    if network != "tron" || !valid_tron_address(address) {
        return Err(AppError::BadRequest(
            "only a valid USDT/TRON address is supported".into(),
        ));
    }
    Ok(())
}

fn normalize_transfer_reference(reference: Option<&str>) -> Result<String, AppError> {
    let reference = reference
        .ok_or_else(|| AppError::BadRequest("external transaction reference required".into()))?;
    if reference.len() != 64 || !reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(
            "valid TRON transaction ID required".into(),
        ));
    }
    Ok(reference.to_ascii_lowercase())
}

fn validate_reason_code(reason: Option<&str>) -> Result<&str, AppError> {
    let reason =
        reason.ok_or_else(|| AppError::BadRequest("resolution reason code required".into()))?;
    if reason.is_empty()
        || reason.len() > 80
        || !reason
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(AppError::BadRequest(
            "invalid resolution reason code".into(),
        ));
    }
    Ok(reason)
}

fn require_provider_tx(conn: &Connection, user_id: &str) -> Result<(), AppError> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM market_provider_profiles WHERE user_id = ?1
           AND status = 'active' AND claim_state = 'claimed'",
            params![user_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(db_error("read market Provider identity"))?
        .is_some();
    if !exists {
        return Err(AppError::Forbidden(
            "active Provider identity required".into(),
        ));
    }
    Ok(())
}

// Stopping sales does not forfeit earned money. Identity conflicts and disabled
// users remain blocked; scoped reconciliation/payout holds still apply below.
fn can_manage_earnings_tx(conn: &Connection, user_id: &str) -> Result<bool, AppError> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM market_provider_profiles p JOIN users u ON u.id = p.user_id
        WHERE p.user_id = ?1 AND p.claim_state = 'claimed' AND p.status != 'identity_conflict' AND u.status = 'active')",
        params![user_id], |row| row.get(0)).map_err(db_error("read earnings capabilities"))
}

fn require_earnings_identity_tx(conn: &Connection, user_id: &str) -> Result<(), AppError> {
    if !can_manage_earnings_tx(conn, user_id)? {
        return Err(AppError::Forbidden(
            "verified Provider earnings identity required".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DestinationView {
    id: String,
    version: i64,
    asset_code: &'static str,
    network: String,
    address: String,
    usable_at: String,
}

fn latest_destination_tx(
    conn: &Connection,
    provider: &str,
) -> Result<Option<DestinationView>, AppError> {
    conn.query_row(
        "SELECT id, version, network, address, usable_at FROM market_payout_destinations
         WHERE provider_user_id = ?1 ORDER BY version DESC LIMIT 1",
        params![provider],
        |row| {
            Ok(DestinationView {
                id: row.get(0)?,
                version: row.get(1)?,
                asset_code: "USDT",
                network: row.get(2)?,
                address: row.get(3)?,
                usable_at: row.get(4)?,
            })
        },
    )
    .optional()
    .map_err(db_error("read Provider payout destination"))
}

/// Paid wallet offers require a claimed Provider and a cooled-off payout
/// address. Buyer-facing payment methods are not a prerequisite on this rail.
pub(crate) fn require_payout_setup_tx(
    conn: &Connection,
    provider: &str,
    at: &str,
) -> Result<(), AppError> {
    require_provider_tx(conn, provider)?;
    let destination = latest_destination_tx(conn, provider)?.ok_or_else(|| {
        AppError::coded_conflict(
            "market_payout_destination_required",
            "configure a USDT/TRON withdrawal destination before publishing paid offers",
            serde_json::json!({ "assetCode": "USDT", "network": "tron" }),
        )
    })?;
    let usable_at = chrono::DateTime::parse_from_rfc3339(&destination.usable_at)
        .map_err(|_| AppError::Internal("stored payout destination time is invalid".into()))?;
    let at = chrono::DateTime::parse_from_rfc3339(at)
        .map_err(|_| AppError::BadRequest("invalid payout setup time".into()))?;
    if usable_at > at {
        return Err(AppError::coded_conflict(
            "market_payout_destination_cooling",
            "withdrawal destination is in its 24-hour cooling period",
            serde_json::json!({ "usableAt": destination.usable_at }),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DestinationRequest {
    network: String,
    address: String,
    confirmation: String,
}

fn save_destination_tx(
    conn: &Connection,
    provider: &str,
    request: &DestinationRequest,
    time: &str,
) -> Result<DestinationView, AppError> {
    require_earnings_identity_tx(conn, provider)?;
    validate_destination(&request.network, &request.address)?;
    if request.confirmation != "CHANGE_WITHDRAWAL_ADDRESS" {
        return Err(AppError::BadRequest(
            "withdrawal address confirmation required".into(),
        ));
    }
    if let Some(existing) = latest_destination_tx(conn, provider)? {
        if existing.network == request.network && existing.address == request.address {
            return Ok(existing);
        }
    }
    let version = latest_destination_tx(conn, provider)?.map_or(1, |prior| prior.version + 1);
    let id = Uuid::new_v4().to_string();
    let usable_at = (chrono::DateTime::parse_from_rfc3339(time)
        .map_err(|_| AppError::Internal("destination timestamp invalid".into()))?
        .with_timezone(&Utc)
        + Duration::hours(ADDRESS_COOLDOWN_HOURS))
    .to_rfc3339();
    conn.execute(
        "INSERT INTO market_payout_destinations (
            id, provider_user_id, version, asset_code, network, address, usable_at, created_at
         ) VALUES (?1, ?2, ?3, 'USDT', ?4, ?5, ?6, ?7)",
        params![
            id,
            provider,
            version,
            request.network,
            request.address,
            usable_at,
            time
        ],
    )
    .map_err(db_error("save Provider payout destination"))?;
    Ok(DestinationView {
        id,
        version,
        asset_code: "USDT",
        network: request.network.clone(),
        address: request.address.clone(),
        usable_at,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PayoutView {
    id: String,
    #[serde(serialize_with = "serialize_units")]
    amount_units: i64,
    status: String,
    network: String,
    address: String,
    version: i64,
    created_at: String,
    external_reference: Option<String>,
}

fn payout_view_tx(
    conn: &Connection,
    request_id: &str,
    provider: &str,
) -> Result<PayoutView, AppError> {
    conn.query_row(
        "SELECT request.id, request.amount_units, request.status,
                destination.network, destination.address, request.version,
                request.created_at, request.external_reference
         FROM market_payout_requests request
         JOIN market_payout_destinations destination ON destination.id = request.destination_id
         WHERE request.id = ?1 AND request.provider_user_id = ?2",
        params![request_id, provider],
        |row| {
            Ok(PayoutView {
                id: row.get(0)?,
                amount_units: row.get(1)?,
                status: row.get(2)?,
                network: row.get(3)?,
                address: row.get(4)?,
                version: row.get(5)?,
                created_at: row.get(6)?,
                external_reference: row.get(7)?,
            })
        },
    )
    .map_err(db_error("read Provider withdrawal"))
}

fn create_payout_tx(
    conn: &Connection,
    provider: &str,
    amount_minor: i64,
    idem: &str,
    time: &str,
) -> Result<PayoutView, AppError> {
    require_earnings_identity_tx(conn, provider)?;
    if amount_minor < MIN_WITHDRAW_MINOR {
        return Err(AppError::BadRequest(
            "withdrawal amount is below minimum".into(),
        ));
    }
    if amount_minor > MAX_WITHDRAW_MINOR {
        return Err(AppError::BadRequest(
            "withdrawal amount exceeds the per-request maximum".into(),
        ));
    }
    let amount_units = units_from_minor(amount_minor)?;
    // The request body contains an amount, not a destination version. A replay
    // must keep returning the original address snapshot even if the Provider
    // has subsequently changed their destination.
    let fingerprint = amount_units.to_string();
    let existing = conn
        .query_row(
            "SELECT id, request_fingerprint FROM market_payout_requests
         WHERE provider_user_id = ?1 AND idempotency_key = ?2",
            params![provider, idem],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_error("read prior Provider withdrawal"))?;
    if let Some((id, saved_fingerprint)) = existing {
        if saved_fingerprint != fingerprint {
            return Err(AppError::Conflict("withdrawal idempotency conflict".into()));
        }
        return payout_view_tx(conn, &id, provider);
    }
    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market_payout_requests WHERE provider_user_id = ?1
             AND status IN ('requested', 'approved', 'transfer_in_progress',
                            'review_required', 'transfer_failed')",
            params![provider],
            |row| row.get(0),
        )
        .map_err(db_error("count open Provider withdrawals"))?;
    if open >= MAX_OPEN_WITHDRAWALS {
        return Err(AppError::Conflict(
            "too many withdrawals are still being processed".into(),
        ));
    }
    require_new_provider_payouts_open_tx(conn)?;
    crate::market_reconcile::ensure_finance_scopes_safe_tx(
        conn,
        &[
            crate::market_reconcile::FinanceScope::Provider(provider),
            crate::market_reconcile::FinanceScope::Payouts,
        ],
    )?;
    let destination = latest_destination_tx(conn, provider)?
        .ok_or_else(|| AppError::Conflict("withdrawal destination required".into()))?;
    if chrono::DateTime::parse_from_rfc3339(&destination.usable_at)
        .map_err(|_| AppError::Internal("stored destination time invalid".into()))?
        > chrono::DateTime::parse_from_rfc3339(time)
            .map_err(|_| AppError::Internal("withdrawal time invalid".into()))?
    {
        return Err(AppError::Conflict(
            "withdrawal address cooling period has not ended".into(),
        ));
    }
    let summary = provider_summary_tx(conn, provider)?;
    if summary.withdrawable_units < amount_units {
        return Err(AppError::Conflict(
            "insufficient matured Provider earnings".into(),
        ));
    }
    let mut statement = conn
        .prepare(
            "SELECT id, unlocked_units - held_units - paid_units
         FROM market_provider_earning_lots
         WHERE provider_user_id = ?1 AND disputed = 0
           AND unlocked_units - held_units - paid_units > 0
         ORDER BY unlock_at, id",
        )
        .map_err(db_error("prepare mature Provider lots"))?;
    let lots = statement
        .query_map(params![provider], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(db_error("query mature Provider lots"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read mature Provider lots"))?;
    let request_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO market_payout_requests (
            id, provider_user_id, destination_id, idempotency_key,
            request_fingerprint, amount_units, status, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'requested', ?7, ?7)",
        params![
            request_id,
            provider,
            destination.id,
            idem,
            fingerprint,
            amount_units,
            time
        ],
    )
    .map_err(db_error("create Provider withdrawal"))?;
    let mut remaining = amount_units;
    for (lot_id, available) in lots {
        if remaining == 0 {
            break;
        }
        let allocated = remaining.min(available);
        let changed = conn
            .execute(
                "UPDATE market_provider_earning_lots SET held_units = held_units + ?2
             WHERE id = ?1 AND disputed = 0
               AND unlocked_units - held_units - paid_units >= ?2",
                params![lot_id, allocated],
            )
            .map_err(db_error("reserve Provider earning lot"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "Provider earnings changed during withdrawal".into(),
            ));
        }
        conn.execute(
            "INSERT INTO market_payout_allocations (request_id, earning_lot_id, amount_units)
             VALUES (?1, ?2, ?3)",
            params![request_id, lot_id, allocated],
        )
        .map_err(db_error("allocate Provider withdrawal"))?;
        remaining -= allocated;
    }
    if remaining != 0 {
        return Err(AppError::Internal(
            "Provider lot allocation mismatch".into(),
        ));
    }
    let journal = post_journal_tx(
        conn,
        "payout",
        &request_id,
        "reserve",
        &fingerprint,
        &[
            Posting {
                kind: "provider_available",
                subject_id: provider,
                signed_units: amount_units,
            },
            Posting {
                kind: "provider_held",
                subject_id: provider,
                signed_units: -amount_units,
            },
        ],
        time,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal(
            "new withdrawal journal already exists".into(),
        ));
    }
    payout_view_tx(conn, &request_id, provider)
}

fn require_new_provider_payouts_open_tx(conn: &Connection) -> Result<(), AppError> {
    if crate::market_payments::wallet_purchase_mode_tx(conn)? {
        crate::market_payments::require_new_wallet_withdrawals_open_tx(conn)?;
        return Ok(());
    }
    Err(AppError::ServiceUnavailable(
        "Provider withdrawals are paused while market finance is not enabled".into(),
    ))
}

fn audit_tx(
    conn: &Connection,
    actor: &str,
    action: &str,
    kind: &str,
    id: &str,
    reason: Option<&str>,
    time: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO market_finance_audit_events (id, actor_user_id, action, object_kind, object_id, reason_code, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![Uuid::new_v4().to_string(), actor, action, kind, id, reason, time],
    ).map_err(db_error("audit market payout"))?;
    Ok(())
}

fn transition_payout_tx(
    conn: &Connection,
    request_id: &str,
    actor: &str,
    action: &str,
    expected_version: i64,
    reference: Option<&str>,
    reason: Option<&str>,
    time: &str,
) -> Result<PayoutView, AppError> {
    let (provider, status, amount, version): (String, String, i64, i64) = conn.query_row(
        "SELECT provider_user_id, status, amount_units, version FROM market_payout_requests WHERE id = ?1",
        params![request_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).map_err(db_error("read Provider withdrawal for transition"))?;
    if version != expected_version {
        return Err(AppError::Conflict("withdrawal version changed".into()));
    }
    if matches!(action, "approve" | "begin_transfer") {
        require_new_provider_payouts_open_tx(conn)?;
        crate::market_reconcile::ensure_finance_scopes_safe_tx(
            conn,
            &[
                crate::market_reconcile::FinanceScope::Provider(&provider),
                crate::market_reconcile::FinanceScope::Payouts,
            ],
        )?;
        let disputed: i64 = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM market_payout_allocations allocation
                    JOIN market_provider_earning_lots lot
                      ON lot.id = allocation.earning_lot_id
                    WHERE allocation.request_id = ?1 AND lot.disputed = 1
                )",
                params![request_id],
                |row| row.get(0),
            )
            .map_err(db_error("recheck disputed withdrawal allocations"))?;
        if disputed != 0 {
            return Err(AppError::Conflict(
                "withdrawal contains disputed Provider earnings".into(),
            ));
        }
    }
    let target = match (status.as_str(), action) {
        ("requested", "approve") => "approved",
        ("approved", "begin_transfer") => "transfer_in_progress",
        ("transfer_in_progress", "mark_paid") => "paid",
        ("requested" | "approved", "reject") => "rejected",
        ("transfer_in_progress", "mark_failed") => "transfer_failed",
        ("transfer_in_progress", "review") => "review_required",
        ("review_required", "mark_paid") => "paid",
        ("review_required", "mark_failed") => "transfer_failed",
        ("transfer_failed", "approve") => "approved",
        ("transfer_failed", "reject") => "rejected",
        _ => return Err(AppError::Conflict("invalid withdrawal transition".into())),
    };
    let normalized_reference = (action == "mark_paid")
        .then(|| normalize_transfer_reference(reference))
        .transpose()?;
    if matches!(action, "reject" | "mark_failed" | "review") || reason.is_some() {
        validate_reason_code(reason)?;
    }
    if action == "mark_paid" {
        let (network, amount_minor): (String, i64) = conn
            .query_row(
                "SELECT destination.network, request.amount_units / ?2
             FROM market_payout_requests request
             JOIN market_payout_destinations destination ON destination.id = request.destination_id
             WHERE request.id = ?1",
                params![request_id, crate::market_billing::MONEY_UNITS_PER_MINOR],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(db_error("read payout receipt scope"))?;
        conn.execute(
            "INSERT INTO market_external_payout_receipts (
                id, network, external_reference, object_kind, object_id, amount_minor, recorded_at
             ) VALUES (?1, ?2, ?3, 'provider_withdrawal', ?4, ?5, ?6)",
            params![
                Uuid::new_v4().to_string(),
                network,
                normalized_reference.as_deref(),
                request_id,
                amount_minor,
                time
            ],
        )
        .map_err(db_error("record external Provider transfer"))?;
        let fingerprint = format!(
            "{provider}:{amount}:{}",
            normalized_reference.as_deref().unwrap()
        );
        let journal = post_journal_tx(
            conn,
            "payout",
            request_id,
            "paid",
            &fingerprint,
            &[
                Posting {
                    kind: "provider_held",
                    subject_id: &provider,
                    signed_units: amount,
                },
                Posting {
                    kind: "owner_clearing",
                    subject_id: "owner",
                    signed_units: -amount,
                },
            ],
            time,
        )?;
        if !journal.inserted {
            return Err(AppError::Internal(
                "withdrawal payment journal already exists".into(),
            ));
        }
        apply_payout_allocations_tx(conn, request_id, true)?;
    } else if action == "reject" {
        let fingerprint = format!("{provider}:{amount}:reject");
        let journal = post_journal_tx(
            conn,
            "payout",
            request_id,
            "reject",
            &fingerprint,
            &[
                Posting {
                    kind: "provider_held",
                    subject_id: &provider,
                    signed_units: amount,
                },
                Posting {
                    kind: "provider_available",
                    subject_id: &provider,
                    signed_units: -amount,
                },
            ],
            time,
        )?;
        if !journal.inserted {
            return Err(AppError::Internal(
                "withdrawal rejection journal already exists".into(),
            ));
        }
        apply_payout_allocations_tx(conn, request_id, false)?;
    }
    let changed = conn.execute(
        "UPDATE market_payout_requests SET status = ?2, version = version + 1,
                owner_user_id = ?3, external_reference = CASE WHEN ?2 = 'paid' THEN ?4 ELSE external_reference END,
                paid_at = CASE WHEN ?2 = 'paid' THEN ?5 ELSE paid_at END, updated_at = ?5
         WHERE id = ?1 AND version = ?6 AND status = ?7",
        params![request_id, target, actor, normalized_reference.as_deref(), time, expected_version, status],
    ).map_err(db_error("transition Provider withdrawal"))?;
    if changed != 1 {
        return Err(AppError::Conflict("withdrawal state changed".into()));
    }
    audit_tx(conn, actor, action, "payout", request_id, reason, time)?;
    payout_view_tx(conn, request_id, &provider)
}

fn apply_payout_allocations_tx(
    conn: &Connection,
    request_id: &str,
    paid: bool,
) -> Result<(), AppError> {
    let mut statement = conn
        .prepare(
            "SELECT earning_lot_id, amount_units FROM market_payout_allocations
         WHERE request_id = ?1 ORDER BY earning_lot_id",
        )
        .map_err(db_error("prepare payout allocations"))?;
    let allocations = statement
        .query_map(params![request_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(db_error("query payout allocations"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read payout allocations"))?;
    for (lot_id, amount) in allocations {
        let changed = if paid {
            conn.execute(
                "UPDATE market_provider_earning_lots SET held_units = held_units - ?2,
                        paid_units = paid_units + ?2 WHERE id = ?1 AND held_units >= ?2",
                params![lot_id, amount],
            )
        } else {
            conn.execute(
                "UPDATE market_provider_earning_lots SET held_units = held_units - ?2
                 WHERE id = ?1 AND held_units >= ?2",
                params![lot_id, amount],
            )
        }
        .map_err(db_error("settle payout allocation"))?;
        if changed != 1 {
            return Err(AppError::Internal("payout lot allocation drift".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuyerRefundRequest {
    amount_minor: i64,
    network: String,
    address: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuyerRefundView {
    id: String,
    #[serde(serialize_with = "serialize_units")]
    amount_units: i64,
    status: String,
    network: String,
    address: String,
    version: i64,
    created_at: String,
    external_reference: Option<String>,
}

fn buyer_refund_view_tx(
    conn: &Connection,
    id: &str,
    user_id: &str,
) -> Result<BuyerRefundView, AppError> {
    conn.query_row(
        "SELECT id, amount_units, status, network, address, version, created_at, external_reference
         FROM market_buyer_refund_requests WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
        |row| {
            Ok(BuyerRefundView {
                id: row.get(0)?,
                amount_units: row.get(1)?,
                status: row.get(2)?,
                network: row.get(3)?,
                address: row.get(4)?,
                version: row.get(5)?,
                created_at: row.get(6)?,
                external_reference: row.get(7)?,
            })
        },
    )
    .map_err(db_error("read buyer wallet refund"))
}

#[cfg(test)]
fn create_buyer_refund_tx(
    conn: &Connection,
    user_id: &str,
    request: &BuyerRefundRequest,
    idem: &str,
    time: &str,
) -> Result<BuyerRefundView, AppError> {
    validate_destination(&request.network, &request.address)?;
    if request.amount_minor < MIN_WITHDRAW_MINOR {
        return Err(AppError::BadRequest(
            "refund amount is below minimum".into(),
        ));
    }
    let units = units_from_minor(request.amount_minor)?;
    let fingerprint = format!("{units}:{}:{}", request.network, request.address);
    let existing = conn
        .query_row(
            "SELECT id, request_fingerprint FROM market_buyer_refund_requests
         WHERE user_id = ?1 AND idempotency_key = ?2",
            params![user_id, idem],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_error("read prior buyer refund"))?;
    if let Some((id, saved)) = existing {
        if saved != fingerprint {
            return Err(AppError::Conflict("refund idempotency conflict".into()));
        }
        return buyer_refund_view_tx(conn, &id, user_id);
    }
    crate::market_reconcile::ensure_finance_scopes_safe_tx(
        conn,
        &[
            crate::market_reconcile::FinanceScope::User(user_id),
            crate::market_reconcile::FinanceScope::Payouts,
        ],
    )?;
    // Money already promised to running services is not refundable: a refund
    // must not be the reason a renewal or metered budget fails.
    let committed =
        crate::market_platform_contracts::prior_due_funding_units_tx(conn, user_id, None, time)?;
    let free = crate::market_wallet::wallet_snapshot_tx(conn, user_id)?.available_units - committed;
    if free < units {
        return Err(AppError::Coded {
            status: axum::http::StatusCode::CONFLICT,
            code: "MARKET_REFUND_EXCEEDS_FREE_BALANCE",
            message: "refund exceeds the balance not committed to services".into(),
            details: serde_json::json!({
                "freeUnits": free.max(0).to_string(),
                "committedUnits": committed.to_string(),
            }),
        });
    }
    let id = Uuid::new_v4().to_string();
    let hold_id = reserve_wallet_tx(
        conn,
        user_id,
        "buyer_refund",
        &id,
        "refund",
        units,
        None,
        time,
    )?;
    let captured = capture_hold_tx(
        conn,
        &hold_id,
        units,
        "buyer_refund_held",
        &id,
        "buyer_refund",
        &id,
        time,
    )?;
    if !captured.inserted {
        return Err(AppError::Internal(
            "new refund hold was already captured".into(),
        ));
    }
    conn.execute(
        "INSERT INTO market_buyer_refund_requests (
            id, user_id, hold_id, idempotency_key, request_fingerprint, amount_units,
            asset_code, network, address, status, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'USDT', ?7, ?8, 'requested', ?9, ?9)",
        params![
            id,
            user_id,
            hold_id,
            idem,
            fingerprint,
            units,
            request.network,
            request.address,
            time
        ],
    )
    .map_err(db_error("create buyer wallet refund"))?;
    buyer_refund_view_tx(conn, &id, user_id)
}

fn transition_buyer_refund_tx(
    conn: &Connection,
    id: &str,
    actor: &str,
    action: &str,
    expected_version: i64,
    reference: Option<&str>,
    reason: Option<&str>,
    time: &str,
) -> Result<BuyerRefundView, AppError> {
    let (buyer, status, amount, network, version): (String, String, i64, String, i64) = conn
        .query_row(
            "SELECT user_id, status, amount_units, network, version
         FROM market_buyer_refund_requests WHERE id = ?1",
            params![id],
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
        .map_err(db_error("read buyer refund transition"))?;
    if version != expected_version {
        return Err(AppError::Conflict("refund version changed".into()));
    }
    if matches!(action, "approve" | "begin_transfer") {
        crate::market_reconcile::ensure_finance_scopes_safe_tx(
            conn,
            &[
                crate::market_reconcile::FinanceScope::User(&buyer),
                crate::market_reconcile::FinanceScope::Payouts,
            ],
        )?;
    }
    let target = match (status.as_str(), action) {
        ("requested", "approve") => "approved",
        ("approved", "begin_transfer") => "transfer_in_progress",
        ("transfer_in_progress", "mark_paid") => "paid",
        ("requested" | "approved", "reject") => "rejected",
        ("transfer_in_progress", "mark_failed") => "transfer_failed",
        ("transfer_in_progress", "review") => "review_required",
        ("review_required", "mark_paid") => "paid",
        ("review_required", "mark_failed") => "transfer_failed",
        ("transfer_failed", "approve") => "approved",
        ("transfer_failed", "reject") => "rejected",
        _ => return Err(AppError::Conflict("invalid refund transition".into())),
    };
    let normalized_reference = (action == "mark_paid")
        .then(|| normalize_transfer_reference(reference))
        .transpose()?;
    if matches!(action, "reject" | "mark_failed" | "review") || reason.is_some() {
        validate_reason_code(reason)?;
    }
    if action == "mark_paid" {
        conn.execute(
            "INSERT INTO market_external_payout_receipts (
                id, network, external_reference, object_kind, object_id, amount_minor, recorded_at
             ) VALUES (?1, ?2, ?3, 'buyer_refund', ?4, ?5, ?6)",
            params![
                Uuid::new_v4().to_string(),
                network,
                normalized_reference.as_deref(),
                id,
                amount / crate::market_billing::MONEY_UNITS_PER_MINOR,
                time
            ],
        )
        .map_err(db_error("record external buyer refund"))?;
        let fingerprint = format!(
            "{buyer}:{amount}:{}",
            normalized_reference.as_deref().unwrap()
        );
        let journal = post_journal_tx(
            conn,
            "buyer_refund",
            id,
            "paid",
            &fingerprint,
            &[
                Posting {
                    kind: "buyer_refund_held",
                    subject_id: id,
                    signed_units: amount,
                },
                Posting {
                    kind: "owner_clearing",
                    subject_id: "owner",
                    signed_units: -amount,
                },
            ],
            time,
        )?;
        if !journal.inserted {
            return Err(AppError::Internal(
                "buyer refund payment journal already exists".into(),
            ));
        }
    } else if action == "reject" {
        let journal = credit_wallet_from_liability_tx(
            conn,
            &buyer,
            amount,
            "buyer_refund_held",
            id,
            "buyer_refund",
            id,
            "reject",
            time,
        )?;
        if !journal.inserted {
            return Err(AppError::Internal(
                "buyer refund rejection journal already exists".into(),
            ));
        }
    }
    let changed = conn
        .execute(
            "UPDATE market_buyer_refund_requests
         SET status = ?2, version = version + 1, owner_user_id = ?3,
             external_reference = CASE WHEN ?2 = 'paid' THEN ?4 ELSE external_reference END,
             paid_at = CASE WHEN ?2 = 'paid' THEN ?5 ELSE paid_at END, updated_at = ?5
         WHERE id = ?1 AND status = ?6 AND version = ?7",
            params![
                id,
                target,
                actor,
                normalized_reference.as_deref(),
                time,
                status,
                expected_version
            ],
        )
        .map_err(db_error("transition buyer wallet refund"))?;
    if changed != 1 {
        return Err(AppError::Conflict("refund state changed".into()));
    }
    audit_tx(conn, actor, action, "buyer_refund", id, reason, time)?;
    buyer_refund_view_tx(conn, id, &buyer)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WithdrawalRequest {
    amount_minor: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransitionRequest {
    action: String,
    expected_version: i64,
    external_reference: Option<String>,
    reason: Option<String>,
    actual_amount_minor: Option<i64>,
    #[serde(default)]
    transferred_at: Option<String>,
    #[serde(default)]
    network_fee_minor: Option<i64>,
    #[serde(default)]
    variance_reason: Option<String>,
}

/// Transfer facts typed by the Owner at mark_paid. The nominal receipt keeps
/// the requested amount; what actually arrived and the fee are recorded beside
/// it, and a difference needs an explicit reason.
struct TransferDetails {
    transferred_at: String,
    actual_amount_minor: i64,
    network_fee_minor: Option<i64>,
    variance_reason: Option<String>,
}

fn transfer_details(
    request: &TransitionRequest,
    amount_units: i64,
    now: &str,
) -> Result<TransferDetails, AppError> {
    let actual = request
        .actual_amount_minor
        .filter(|amount| *amount >= 0)
        .ok_or_else(|| {
            AppError::BadRequest("the amount that actually arrived is required".into())
        })?;
    let transferred_at = request
        .transferred_at
        .clone()
        .ok_or_else(|| AppError::BadRequest("the transfer time is required".into()))?;
    let at = chrono::DateTime::parse_from_rfc3339(&transferred_at)
        .map_err(|_| AppError::BadRequest("transfer time must be RFC 3339".into()))?;
    let now_at = chrono::DateTime::parse_from_rfc3339(now)
        .map_err(|_| AppError::Internal("transition time invalid".into()))?;
    if at > now_at + Duration::minutes(5) || at < now_at - Duration::days(30) {
        return Err(AppError::BadRequest(
            "transfer time is outside the allowed window".into(),
        ));
    }
    if request.network_fee_minor.is_some_and(|fee| fee < 0) {
        return Err(AppError::BadRequest(
            "network fee cannot be negative".into(),
        ));
    }
    let nominal = amount_units / crate::market_billing::MONEY_UNITS_PER_MINOR;
    let variance_reason = request.variance_reason.clone();
    if actual != nominal {
        validate_reason_code(variance_reason.as_deref()).map_err(|_| {
            AppError::BadRequest("a variance reason is required when the amounts differ".into())
        })?;
    }
    Ok(TransferDetails {
        transferred_at,
        actual_amount_minor: actual,
        network_fee_minor: request.network_fee_minor,
        variance_reason,
    })
}

fn record_transfer_details_tx(
    conn: &Connection,
    table: &str,
    id: &str,
    details: &TransferDetails,
) -> Result<(), AppError> {
    conn.execute(
        &format!(
            "UPDATE {table} SET transferred_at = ?2, actual_amount_minor = ?3,
                 network_fee_minor = ?4, variance_reason = ?5 WHERE id = ?1"
        ),
        params![
            id,
            details.transferred_at,
            details.actual_amount_minor,
            details.network_fee_minor,
            details.variance_reason
        ],
    )
    .map_err(db_error("record transfer details"))?;
    Ok(())
}

/// Record the approver, flag a self-approved transfer, and notify.
#[allow(clippy::too_many_arguments)]
fn after_transition_tx(
    conn: &Connection,
    table: &str,
    id: &str,
    actor: &str,
    action: &str,
    recipient_audience: &str,
    recipient: &str,
    kind: &str,
    status: &str,
    amount_units: i64,
    time: &str,
) -> Result<(), AppError> {
    if action == "approve" {
        conn.execute(
            &format!("UPDATE {table} SET approved_by = ?2 WHERE id = ?1"),
            params![id, actor],
        )
        .map_err(db_error("record transfer approver"))?;
    }
    if action == "mark_paid" {
        let approver: Option<String> = conn
            .query_row(
                &format!("SELECT approved_by FROM {table} WHERE id = ?1"),
                params![id],
                |row| row.get(0),
            )
            .map_err(db_error("read transfer approver"))?;
        if approver.as_deref() == Some(actor) {
            audit_tx(conn, actor, "self_approved_transfer", table, id, None, time)?;
        }
    }
    crate::market_notifications::enqueue_user_notification_tx(
        conn,
        recipient_audience,
        recipient,
        kind,
        table,
        id,
        &format!("{kind}:{id}:{status}"),
        serde_json::json!({
            "status": status,
            "amountMinor": amount_units / crate::market_billing::MONEY_UNITS_PER_MINOR,
        }),
        time,
    )
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-provider-finance/me", get(get_provider_finance))
        .route("/v1/market-provider-finance/earnings", get(list_earnings))
        .route(
            "/v1/market-provider-finance/destination",
            put(set_destination),
        )
        .route(
            "/v1/market-provider-finance/destination/challenge",
            post(request_address_challenge),
        )
        .route(
            "/v1/market-provider-finance/withdrawals/:id/cancel",
            post(cancel_withdrawal),
        )
        .route(
            "/v1/market-provider-finance/holds",
            get(list_own_earning_holds),
        )
        .route(
            "/v1/market-wallet/refund-requests/:id/cancel",
            post(cancel_buyer_refund),
        )
        .route(
            "/v1/market-provider-finance/withdrawals",
            get(list_withdrawals).post(request_withdrawal),
        )
        .route(
            "/v1/market-wallet/refund-requests",
            get(list_buyer_refunds).post(request_buyer_refund),
        )
        .route(
            "/v1/admin/market-finance/withdrawals",
            get(admin_list_withdrawals),
        )
        .route("/v1/admin/market-finance/refunds", get(admin_list_refunds))
        .route(
            "/v1/admin/market-finance/withdrawals/:id/transition",
            post(admin_transition_withdrawal),
        )
        .route(
            "/v1/admin/market-finance/refunds/:id/transition",
            post(admin_transition_buyer_refund),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    cursor: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListPage<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EarningView {
    id: String,
    #[serde(serialize_with = "serialize_units")]
    amount_units: i64,
    #[serde(serialize_with = "serialize_units")]
    reversed_units: i64,
    #[serde(serialize_with = "serialize_units")]
    unlocked_units: i64,
    #[serde(serialize_with = "serialize_units")]
    held_units: i64,
    #[serde(serialize_with = "serialize_units")]
    paid_units: i64,
    fulfilled_at: String,
    unlock_at: String,
    disputed: bool,
}

fn list_earnings_tx(
    conn: &Connection,
    provider: &str,
    cursor: Option<&str>,
) -> Result<ListPage<EarningView>, AppError> {
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT created_at FROM market_provider_earning_lots
                 WHERE id = ?1 AND provider_user_id = ?2",
                params![id, provider],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read earning cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid earning cursor".into()))
        })
        .transpose()?;
    let mut statement = conn
        .prepare(
            "SELECT id, amount_units, reversed_units, unlocked_units, held_units, paid_units,
                    fulfilled_at, unlock_at, disputed, created_at
             FROM market_provider_earning_lots WHERE provider_user_id = ?1
               AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id < ?3))
             ORDER BY created_at DESC, id DESC LIMIT 51",
        )
        .map_err(db_error("prepare earning history"))?;
    let mut items = statement
        .query_map(params![provider, cursor_at, cursor], |row| {
            Ok(EarningView {
                id: row.get(0)?,
                amount_units: row.get(1)?,
                reversed_units: row.get(2)?,
                unlocked_units: row.get(3)?,
                held_units: row.get(4)?,
                paid_units: row.get(5)?,
                fulfilled_at: row.get(6)?,
                unlock_at: row.get(7)?,
                disputed: row.get::<_, i64>(8)? != 0,
            })
        })
        .map_err(db_error("query earning history"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read earning history"))?;
    let has_more = items.len() > 50;
    items.truncate(50);
    Ok(ListPage {
        next_cursor: has_more.then(|| items.last().unwrap().id.clone()),
        items,
    })
}

async fn list_earnings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(list_earnings_tx(
            &conn,
            &session.user_id,
            query.cursor.as_deref(),
        )?),
    ))
}

fn list_withdrawals_tx(
    conn: &Connection,
    provider: &str,
    cursor: Option<&str>,
) -> Result<ListPage<PayoutView>, AppError> {
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT created_at FROM market_payout_requests
                 WHERE id = ?1 AND provider_user_id = ?2",
                params![id, provider],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read withdrawal cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid withdrawal cursor".into()))
        })
        .transpose()?;
    let mut statement = conn
        .prepare(
            "SELECT request.id, request.amount_units, request.status, destination.network,
                    destination.address, request.version, request.created_at,
                    request.external_reference
             FROM market_payout_requests request
             JOIN market_payout_destinations destination ON destination.id = request.destination_id
             WHERE request.provider_user_id = ?1
               AND (?2 IS NULL OR request.created_at < ?2
                 OR (request.created_at = ?2 AND request.id < ?3))
             ORDER BY request.created_at DESC, request.id DESC LIMIT 51",
        )
        .map_err(db_error("prepare withdrawal history"))?;
    let mut items = statement
        .query_map(params![provider, cursor_at, cursor], |row| {
            Ok(PayoutView {
                id: row.get(0)?,
                amount_units: row.get(1)?,
                status: row.get(2)?,
                network: row.get(3)?,
                address: row.get(4)?,
                version: row.get(5)?,
                created_at: row.get(6)?,
                external_reference: row.get(7)?,
            })
        })
        .map_err(db_error("query withdrawal history"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read withdrawal history"))?;
    let has_more = items.len() > 50;
    items.truncate(50);
    Ok(ListPage {
        next_cursor: has_more.then(|| items.last().unwrap().id.clone()),
        items,
    })
}

async fn list_withdrawals(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(list_withdrawals_tx(
            &conn,
            &session.user_id,
            query.cursor.as_deref(),
        )?),
    ))
}

fn list_buyer_refunds_tx(
    conn: &Connection,
    buyer: &str,
    cursor: Option<&str>,
) -> Result<ListPage<BuyerRefundView>, AppError> {
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT created_at FROM market_buyer_refund_requests
                 WHERE id = ?1 AND user_id = ?2",
                params![id, buyer],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read refund cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid refund cursor".into()))
        })
        .transpose()?;
    let mut statement = conn
        .prepare(
            "SELECT id, amount_units, status, network, address, version, created_at,
                    external_reference
             FROM market_buyer_refund_requests WHERE user_id = ?1
               AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id < ?3))
             ORDER BY created_at DESC, id DESC LIMIT 51",
        )
        .map_err(db_error("prepare buyer refund history"))?;
    let mut items = statement
        .query_map(params![buyer, cursor_at, cursor], |row| {
            Ok(BuyerRefundView {
                id: row.get(0)?,
                amount_units: row.get(1)?,
                status: row.get(2)?,
                network: row.get(3)?,
                address: row.get(4)?,
                version: row.get(5)?,
                created_at: row.get(6)?,
                external_reference: row.get(7)?,
            })
        })
        .map_err(db_error("query buyer refund history"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read buyer refund history"))?;
    let has_more = items.len() > 50;
    items.truncate(50);
    Ok(ListPage {
        next_cursor: has_more.then(|| items.last().unwrap().id.clone()),
        items,
    })
}

async fn list_buyer_refunds(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(list_buyer_refunds_tx(
            &conn,
            &session.user_id,
            query.cursor.as_deref(),
        )?),
    ))
}

async fn admin_list_withdrawals(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut statement = conn
        .prepare(
            "SELECT request.id, request.provider_user_id, request.amount_units, request.status,
                    request.version, request.created_at, destination.network,
                    destination.address, destination.version
             FROM market_payout_requests request
             JOIN market_payout_destinations destination ON destination.id = request.destination_id
             WHERE request.status NOT IN ('paid', 'rejected')
             ORDER BY request.created_at, request.id LIMIT 200",
        )
        .map_err(db_error("prepare pending withdrawals"))?;
    let items = statement
        .query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?, "providerUserId": row.get::<_, String>(1)?,
                "amountUnits": row.get::<_, i64>(2)?.to_string(), "status": row.get::<_, String>(3)?,
                "version": row.get::<_, i64>(4)?, "createdAt": row.get::<_, String>(5)?,
                "network": row.get::<_, String>(6)?, "address": row.get::<_, String>(7)?,
                "destinationVersion": row.get::<_, i64>(8)?,
            }))
        })
        .map_err(db_error("query pending withdrawals"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read pending withdrawals"))?;
    Ok((no_store(), Json(items)))
}

async fn admin_list_refunds(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut statement = conn
        .prepare(
            "SELECT id, user_id, amount_units, status, version, created_at, network, address
         FROM market_buyer_refund_requests WHERE status NOT IN ('paid', 'rejected')
         ORDER BY created_at, id LIMIT 200",
        )
        .map_err(db_error("prepare pending refunds"))?;
    let items = statement
        .query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?, "userId": row.get::<_, String>(1)?,
                "amountUnits": row.get::<_, i64>(2)?.to_string(), "status": row.get::<_, String>(3)?,
                "version": row.get::<_, i64>(4)?, "createdAt": row.get::<_, String>(5)?,
                "network": row.get::<_, String>(6)?, "address": row.get::<_, String>(7)?,
            }))
        })
        .map_err(db_error("query pending refunds"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read pending refunds"))?;
    Ok((no_store(), Json(items)))
}

async fn request_buyer_refund(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_session(&state, &headers).await?;
    Err(AppError::Coded {
        status: axum::http::StatusCode::GONE,
        code: "WALLET_CASH_REFUNDS_RETIRED",
        message: "consumption balance is not cash-refundable; use it for services or transfer it to another verified user".into(),
        details: serde_json::json!({ "walletUrl": "/account/wallet/" }),
    })
}
async fn admin_transition_buyer_refund(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<TransitionRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = if request.action == "mark_paid" {
        crate::api::require_owner_session(&state, &headers).await?
    } else {
        crate::api::require_admin_session(&state, &headers).await?
    };
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin buyer refund transition"))?;
    let time = now();
    let (amount, buyer): (i64, String) = tx
        .query_row(
            "SELECT amount_units, user_id FROM market_buyer_refund_requests WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read buyer refund amount"))?;
    let details = (request.action == "mark_paid")
        .then(|| transfer_details(&request, amount, &time))
        .transpose()?;
    let result = transition_buyer_refund_tx(
        &tx,
        &id,
        &session.user_id,
        &request.action,
        request.expected_version,
        request.external_reference.as_deref(),
        request.reason.as_deref(),
        &time,
    )?;
    if let Some(details) = &details {
        record_transfer_details_tx(&tx, "market_buyer_refund_requests", &id, details)?;
    }
    after_transition_tx(
        &tx,
        "market_buyer_refund_requests",
        &id,
        &session.user_id,
        &request.action,
        "buyer",
        &buyer,
        "refund_status",
        &result.status,
        amount,
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit buyer refund transition"))?;
    Ok((no_store(), Json(result)))
}

async fn get_provider_finance(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct View {
        summary: ProviderFinanceSummary,
        destination: Option<DestinationView>,
        is_provider: bool,
        has_finance_activity: bool,
        can_withdraw: bool,
    }
    let is_provider = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_provider_profiles
             WHERE user_id = ?1 AND status = 'active' AND claim_state = 'claimed')",
            params![session.user_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(db_error("read claimed Provider identity"))?;
    Ok((
        no_store(),
        Json(View {
            summary: provider_summary_tx(&conn, &session.user_id)?,
            destination: latest_destination_tx(&conn, &session.user_id)?,
            is_provider,
            has_finance_activity: conn.query_row("SELECT EXISTS(SELECT 1 FROM market_provider_earning_lots WHERE provider_user_id = ?1)
                OR EXISTS(SELECT 1 FROM market_payout_requests WHERE provider_user_id = ?1)
                OR EXISTS(SELECT 1 FROM market_payout_destinations WHERE provider_user_id = ?1)", params![session.user_id], |row| row.get(0))
                .map_err(db_error("read historical Provider finance visibility"))?,
            can_withdraw: can_manage_earnings_tx(&conn, &session.user_id)?,
        }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetDestinationRequest {
    network: String,
    address: String,
    confirmation: String,
    challenge_id: String,
    code: String,
}

async fn set_destination(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<SetDestinationRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin destination update"))?;
    let time = now();
    let changes_address = latest_destination_tx(&tx, &session.user_id)?.is_none_or(|existing| {
        existing.network != request.network || existing.address != request.address
    });
    if changes_address
        && !consume_address_challenge_tx(
            &tx,
            &session.user_id,
            &request.challenge_id,
            request.code.trim(),
            &request.network,
            &request.address,
            &time,
        )?
    {
        // Keep the failed attempt so the code cannot be guessed.
        tx.commit()
            .map_err(db_error("commit address challenge attempt"))?;
        return Err(AppError::BadRequest("verification code is invalid".into()));
    }
    let result = save_destination_tx(
        &tx,
        &session.user_id,
        &DestinationRequest {
            network: request.network.clone(),
            address: request.address.clone(),
            confirmation: request.confirmation.clone(),
        },
        &time,
    )?;
    audit_tx(
        &tx,
        &session.user_id,
        "destination_saved",
        "payout_destination",
        &result.id,
        None,
        &time,
    )?;
    if changes_address {
        crate::market_notifications::enqueue_user_notification_tx(
            &tx,
            "provider",
            &session.user_id,
            "payout_address_changed",
            "payout_destination",
            &result.id,
            &format!("payout_address_changed:{}", result.id),
            serde_json::json!({
                "network": result.network,
                "address": mask_address(&result.address),
                "usableAt": result.usable_at,
            }),
            &time,
        )?;
    }
    tx.commit().map_err(db_error("commit destination update"))?;
    Ok((no_store(), Json(result)))
}

async fn list_own_earning_holds(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(crate::market_finance::active_earning_holds_tx(
            &conn,
            Some(&session.user_id),
        )?),
    ))
}

async fn request_withdrawal(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<WithdrawalRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let idem = idempotency_key(&headers)?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Provider withdrawal"))?;
    let time = now();
    let result = create_payout_tx(&tx, &session.user_id, request.amount_minor, idem, &time)?;
    if result.status == "requested" && result.created_at == time {
        crate::market_notifications::owner_signal_tx(
            &tx,
            "payout_requested",
            "provider_withdrawal",
            &result.id,
            serde_json::json!({ "amountMinor": request.amount_minor }),
            &time,
            false,
        )?;
    }
    tx.commit()
        .map_err(db_error("commit Provider withdrawal"))?;
    Ok((no_store(), Json(result)))
}

async fn admin_transition_withdrawal(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<TransitionRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = if request.action == "mark_paid" {
        // Marking an external transfer paid moves Owner money.
        crate::api::require_owner_session(&state, &headers).await?
    } else {
        crate::api::require_admin_session(&state, &headers).await?
    };
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin withdrawal transition"))?;
    let time = now();
    let amount: i64 = tx
        .query_row(
            "SELECT amount_units FROM market_payout_requests WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(db_error("read withdrawal amount"))?;
    let details = (request.action == "mark_paid")
        .then(|| transfer_details(&request, amount, &time))
        .transpose()?;
    let result = transition_payout_tx(
        &tx,
        &id,
        &session.user_id,
        &request.action,
        request.expected_version,
        request.external_reference.as_deref(),
        request.reason.as_deref(),
        &time,
    )?;
    if let Some(details) = &details {
        record_transfer_details_tx(&tx, "market_payout_requests", &id, details)?;
    }
    let provider: String = tx
        .query_row(
            "SELECT provider_user_id FROM market_payout_requests WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(db_error("read withdrawal Provider"))?;
    after_transition_tx(
        &tx,
        "market_payout_requests",
        &id,
        &session.user_id,
        &request.action,
        "provider",
        &provider,
        "payout_status",
        &result.status,
        amount,
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit withdrawal transition"))?;
    Ok((no_store(), Json(result)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CancelRequest {
    expected_version: i64,
}

/// A Provider may withdraw a request the Owner has not yet approved.
async fn cancel_withdrawal(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<CancelRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin withdrawal cancellation"))?;
    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM market_payout_requests WHERE id = ?1 AND provider_user_id = ?2",
            params![id, session.user_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read own withdrawal"))?;
    match status.as_deref() {
        None => return Err(AppError::NotFound("withdrawal not found".into())),
        Some("requested") => {}
        Some(_) => {
            return Err(AppError::Conflict(
                "only a withdrawal awaiting approval can be cancelled".into(),
            ));
        }
    }
    let result = transition_payout_tx(
        &tx,
        &id,
        &session.user_id,
        "reject",
        request.expected_version,
        None,
        Some("provider_cancelled"),
        &now(),
    )?;
    tx.commit()
        .map_err(db_error("commit withdrawal cancellation"))?;
    Ok((no_store(), Json(result)))
}

/// A buyer may withdraw a balance refund the Owner has not yet approved.
async fn cancel_buyer_refund(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<CancelRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin refund cancellation"))?;
    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM market_buyer_refund_requests WHERE id = ?1 AND user_id = ?2",
            params![id, session.user_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read own refund"))?;
    match status.as_deref() {
        None => return Err(AppError::NotFound("refund not found".into())),
        Some("requested") => {}
        Some(_) => {
            return Err(AppError::Conflict(
                "only a refund awaiting approval can be cancelled".into(),
            ));
        }
    }
    let result = transition_buyer_refund_tx(
        &tx,
        &id,
        &session.user_id,
        "reject",
        request.expected_version,
        None,
        Some("buyer_cancelled"),
        &now(),
    )?;
    tx.commit()
        .map_err(db_error("commit refund cancellation"))?;
    Ok((no_store(), Json(result)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddressChallengeRequest {
    network: String,
    address: String,
}

fn code_digest(challenge_id: &str, code: &str) -> String {
    hex::encode(Sha256::digest(
        format!("payout-address:{challenge_id}:{code}").as_bytes(),
    ))
}

/// Email a one-time code before a withdrawal address can change. The code is
/// bound to the exact network and address it was requested for.
async fn request_address_challenge(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<AddressChallengeRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    validate_destination(&request.network, &request.address)?;
    let challenge_id = Uuid::new_v4().to_string();
    let code = format!("{:06}", rand::random::<u32>() % 1_000_000);
    {
        let conn = state.store.conn.lock().await;
        require_earnings_identity_tx(&conn, &session.user_id)?;
        let recent: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_payout_address_challenges
                 WHERE provider_user_id = ?1 AND created_at >= ?2",
                params![
                    session.user_id,
                    (Utc::now() - Duration::hours(1)).to_rfc3339()
                ],
                |row| row.get(0),
            )
            .map_err(db_error("count address challenges"))?;
        if recent >= 5 {
            return Err(AppError::TooManyRequests(
                "too many address verification codes requested".into(),
            ));
        }
        conn.execute(
            "INSERT INTO market_payout_address_challenges
             (id, provider_user_id, network, address, code_sha256, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                challenge_id,
                session.user_id,
                request.network,
                request.address,
                code_digest(&challenge_id, &code),
                (Utc::now() + Duration::minutes(ADDRESS_CODE_MINUTES)).to_rfc3339(),
                now()
            ],
        )
        .map_err(db_error("create address challenge"))?;
    }
    let delivered = match (
        state.config.resend_api_key.as_deref(),
        crate::notifications::notification_sender(&state.config),
    ) {
        (Some(api_key), Some(from)) => {
            let http = state.proxy_http.clone();
            let masked = mask_address(&request.address);
            let text = format!(
                "提现地址变更验证码 / Withdrawal address code: {code}\n\n\
                 地址 / Address: {masked} ({network})\n\
                 {minutes} 分钟内有效。若非本人操作，请忽略并检查账户安全。\n\
                 Valid for {minutes} minutes. If you did not request this, ignore it.",
                network = request.network,
                minutes = ADDRESS_CODE_MINUTES
            );
            let html = format!("<p>{}</p>", text.replace('\n', "<br>"));
            let key = format!("payout-address-{challenge_id}");
            crate::notifications::send_resend_frozen_email(
                &http,
                api_key,
                crate::notifications::FrozenEmailEnvelope {
                    from: &from,
                    recipient: &session.email,
                    subject: "提现地址验证码 / Withdrawal address code",
                    html: &html,
                    text: &text,
                    reply_to: None,
                    idempotency_key: &key,
                },
            )
            .await
            .is_ok()
        }
        _ => false,
    };
    if !delivered && !state.config.use_localhost {
        return Err(AppError::ServiceUnavailable(
            "verification email could not be sent".into(),
        ));
    }
    if !delivered {
        // Local development has no mail transport; the code stays in logs.
        tracing::info!(challenge_id, code, "payout address code (localhost only)");
    }
    Ok((
        no_store(),
        Json(serde_json::json!({
            "challengeId": challenge_id,
            "expiresInSeconds": ADDRESS_CODE_MINUTES * 60,
        })),
    ))
}

fn mask_address(address: &str) -> String {
    if address.len() <= 10 {
        return address.to_string();
    }
    format!("{}…{}", &address[..6], &address[address.len() - 4..])
}

/// Consume a matching, unexpired challenge for exactly this address.
fn consume_address_challenge_tx(
    conn: &Connection,
    provider: &str,
    challenge_id: &str,
    code: &str,
    network: &str,
    address: &str,
    time: &str,
) -> Result<bool, AppError> {
    let row: Option<(String, String, String, i64, String, Option<String>)> = conn
        .query_row(
            "SELECT network, address, code_sha256, attempts, expires_at, consumed_at
             FROM market_payout_address_challenges WHERE id = ?1 AND provider_user_id = ?2",
            params![challenge_id, provider],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("read address challenge"))?;
    let Some((stored_network, stored_address, digest, attempts, expires_at, consumed)) = row else {
        return Err(AppError::BadRequest("verification code is invalid".into()));
    };
    if consumed.is_some()
        || attempts >= ADDRESS_CODE_ATTEMPTS
        || expires_at.as_str() < time
        || stored_network != network
        || stored_address != address
    {
        return Err(AppError::BadRequest(
            "verification code is invalid or expired".into(),
        ));
    }
    if code_digest(challenge_id, code) != digest {
        conn.execute(
            "UPDATE market_payout_address_challenges SET attempts = attempts + 1 WHERE id = ?1",
            params![challenge_id],
        )
        .map_err(db_error("count address challenge attempt"))?;
        return Ok(false);
    }
    conn.execute(
        "UPDATE market_payout_address_challenges SET consumed_at = ?2 WHERE id = ?1",
        params![challenge_id, time],
    )
    .map_err(db_error("consume address challenge"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_finance::{record_metered_delivery_tx, unlock_due_earnings_tx};
    use crate::market_wallet::{credit_verified_topup_tx, reconcile_wallet_tx, wallet_snapshot_tx};

    fn valid_address_with_byte(value: u8) -> String {
        let mut body = vec![0x41];
        body.extend_from_slice(&[value; 20]);
        let one = Sha256::digest(&body);
        let two = Sha256::digest(one);
        body.extend_from_slice(&two[..4]);
        bs58::encode(body).into_string()
    }

    fn valid_address() -> String {
        valid_address_with_byte(4)
    }

    const TX_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn retired_provider_keeps_earnings_access_but_identity_conflict_and_blocked_user_do_not() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        let at = "2026-10-01T00:00:00Z";
        conn.execute("INSERT INTO users (id, email_normalized, created_at, last_login_at) VALUES ('provider', 'provider@example.test', ?1, ?1)", params![at]).unwrap();
        conn.execute("INSERT INTO market_provider_profiles (id, canonical_email, user_id, display_name, claim_state, status, created_at, updated_at)
            VALUES ('profile', 'provider@example.test', 'provider', 'Provider', 'claimed', 'active', ?1, ?1)", params![at]).unwrap();
        for status in ["active", "paused", "retired"] {
            conn.execute(
                "UPDATE market_provider_profiles SET status = ?1",
                params![status],
            )
            .unwrap();
            assert!(can_manage_earnings_tx(&conn, "provider").unwrap());
            save_destination_tx(
                &conn,
                "provider",
                &DestinationRequest {
                    network: "tron".into(),
                    address: valid_address(),
                    confirmation: "CHANGE_WITHDRAWAL_ADDRESS".into(),
                },
                at,
            )
            .unwrap();
            if status != "active" {
                assert!(require_provider_tx(&conn, "provider").is_err());
            }
        }
        conn.execute(
            "UPDATE market_provider_profiles SET status = 'identity_conflict'",
            [],
        )
        .unwrap();
        assert!(!can_manage_earnings_tx(&conn, "provider").unwrap());
        conn.execute("UPDATE market_provider_profiles SET status = 'retired'", [])
            .unwrap();
        conn.execute("UPDATE users SET status = 'blocked'", [])
            .unwrap();
        assert!(require_earnings_identity_tx(&conn, "provider").is_err());
        assert!(!can_manage_earnings_tx(&conn, "buyer").unwrap());
    }

    #[test]
    fn tron_checksum_is_required() {
        let address = valid_address();
        assert!(valid_tron_address(&address));
        assert!(!valid_tron_address(&format!("{}1", address)));
        assert_eq!(
            normalize_transfer_reference(Some(&TX_ID.to_ascii_uppercase())).unwrap(),
            TX_ID
        );
        assert!(normalize_transfer_reference(Some("chain-tx-1")).is_err());
        assert!(validate_reason_code(Some("secret=bearer token")).is_err());
    }

    #[test]
    fn wallet_paid_offer_requires_claimed_provider_and_cooled_withdrawal_address() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-09-01T00:00:00+00:00";
        assert!(require_payout_setup_tx(&tx, "provider", start).is_err());
        tx.execute(
            "INSERT INTO users (id, email_normalized, created_at, last_login_at)
             VALUES ('provider', 'provider@example.com', ?1, ?1)",
            params![start],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO market_provider_profiles
                (id, canonical_email, user_id, display_name, claim_state, status, created_at, updated_at)
             VALUES ('mp_wallet_offer', 'provider@example.com', 'provider', 'Provider',
                     'claimed', 'active', ?1, ?1)",
            params![start],
        )
        .unwrap();
        assert!(require_payout_setup_tx(&tx, "provider", start).is_err());
        save_destination_tx(
            &tx,
            "provider",
            &DestinationRequest {
                network: "tron".into(),
                address: valid_address(),
                confirmation: "CHANGE_WITHDRAWAL_ADDRESS".into(),
            },
            start,
        )
        .unwrap();
        assert!(require_payout_setup_tx(&tx, "provider", start).is_err());
        require_payout_setup_tx(&tx, "provider", "2026-09-02T00:00:00+00:00").unwrap();
    }

    #[test]
    fn provider_cannot_withdraw_before_seven_days_or_twice() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let earlier = "2026-09-01T00:00:00+00:00";
        tx.execute(
            "UPDATE market_finance_control
             SET mode = 'enabled', cutover_approved_at = ?1 WHERE id = 1",
            params![earlier],
        )
        .unwrap();
        // Provider identity is a product prerequisite, not a payout destination.
        tx.execute(
            "INSERT INTO users (id, email_normalized, created_at, last_login_at)
                    VALUES ('provider', 'provider@example.com', ?1, ?1)",
            params![earlier],
        )
        .unwrap();
        tx.execute("INSERT INTO market_provider_profiles (
                    id, canonical_email, user_id, display_name, claim_state, status, created_at, updated_at)
                    VALUES ('mp_test', 'provider@example.com', 'provider', 'Provider', 'claimed', 'active', ?1, ?1)", params![earlier]).unwrap();
        credit_verified_topup_tx(&tx, "buyer", "order", 1000, "receipt", earlier).unwrap();
        record_metered_delivery_tx(
            &tx,
            "share",
            "contract",
            "accrual",
            "buyer",
            "provider",
            earlier,
            "2026-09-01T00:00:01+00:00",
            8_640_000,
            earlier,
        )
        .unwrap();
        let destination = save_destination_tx(
            &tx,
            "provider",
            &DestinationRequest {
                network: "tron".into(),
                address: valid_address(),
                confirmation: "CHANGE_WITHDRAWAL_ADDRESS".into(),
            },
            earlier,
        )
        .unwrap();
        assert!(!destination.id.is_empty());
        assert!(create_payout_tx(&tx, "provider", 100, "idem-key-one", earlier).is_err());
        unlock_due_earnings_tx(&tx, "2026-09-08T00:00:01+00:00", 10).unwrap();
        let time = "2026-09-09T00:00:00+00:00";
        tx.execute(
            "UPDATE market_provider_earning_lots SET disputed = 1 WHERE provider_user_id = 'provider'",
            [],
        )
        .unwrap();
        let disputed_summary = provider_summary_tx(&tx, "provider").unwrap();
        assert_eq!(disputed_summary.available_units, 8_640_000);
        assert_eq!(disputed_summary.withdrawable_units, 0);
        assert!(create_payout_tx(&tx, "provider", 100, "disputed-key", &time).is_err());
        tx.execute(
            "UPDATE market_provider_earning_lots SET disputed = 0 WHERE provider_user_id = 'provider'",
            [],
        )
        .unwrap();
        tx.execute(
            "UPDATE market_finance_control
             SET mode = 'disabled', cutover_approved_at = NULL WHERE id = 1",
            [],
        )
        .unwrap();
        assert!(matches!(
            create_payout_tx(&tx, "provider", 100, "paused-key", &time),
            Err(AppError::ServiceUnavailable(_))
        ));
        tx.execute(
            "UPDATE market_finance_control
             SET mode = 'enabled', cutover_approved_at = ?1 WHERE id = 1",
            params![time],
        )
        .unwrap();
        let request = create_payout_tx(&tx, "provider", 100, "idem-key-one", &time).unwrap();
        assert_eq!(
            list_earnings_tx(&tx, "provider", None).unwrap().items.len(),
            1
        );
        assert!(
            list_earnings_tx(&tx, "other", None)
                .unwrap()
                .items
                .is_empty()
        );
        assert_eq!(
            list_withdrawals_tx(&tx, "provider", None)
                .unwrap()
                .items
                .len(),
            1
        );
        assert!(list_withdrawals_tx(&tx, "other", Some(&request.id)).is_err());
        assert_eq!(
            create_payout_tx(&tx, "provider", 100, "idem-key-one", &time)
                .unwrap()
                .id,
            request.id
        );
        save_destination_tx(
            &tx,
            "provider",
            &DestinationRequest {
                network: "tron".into(),
                address: valid_address_with_byte(5),
                confirmation: "CHANGE_WITHDRAWAL_ADDRESS".into(),
            },
            &time,
        )
        .unwrap();
        let replay = create_payout_tx(&tx, "provider", 100, "idem-key-one", &time).unwrap();
        assert_eq!(replay.id, request.id);
        assert_eq!(replay.address, valid_address());
        tx.execute(
            "UPDATE market_finance_control SET new_withdrawals_paused = 1 WHERE id = 1",
            [],
        )
        .unwrap();
        assert!(matches!(
            create_payout_tx(&tx, "provider", 100, "new-paused-key", &time),
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(
            create_payout_tx(&tx, "provider", 100, "idem-key-one", &time)
                .unwrap()
                .id,
            request.id
        );
        assert!(matches!(
            transition_payout_tx(&tx, &request.id, "owner", "approve", 1, None, None, &time),
            Err(AppError::ServiceUnavailable(_))
        ));
        tx.execute(
            "UPDATE market_finance_control SET new_withdrawals_paused = 0 WHERE id = 1",
            [],
        )
        .unwrap();
        assert_eq!(
            provider_summary_tx(&tx, "provider")
                .unwrap()
                .available_units,
            0
        );
        tx.execute(
            "UPDATE market_finance_control
             SET mode = 'disabled', cutover_approved_at = NULL WHERE id = 1",
            [],
        )
        .unwrap();
        assert!(matches!(
            transition_payout_tx(&tx, &request.id, "owner", "approve", 1, None, None, &time),
            Err(AppError::ServiceUnavailable(_))
        ));
        tx.execute(
            "UPDATE market_finance_control
             SET mode = 'enabled', cutover_approved_at = ?1 WHERE id = 1",
            params![time],
        )
        .unwrap();
        tx.execute(
            "UPDATE market_provider_earning_lots SET disputed = 1 WHERE provider_user_id = 'provider'",
            [],
        )
        .unwrap();
        assert!(
            transition_payout_tx(&tx, &request.id, "owner", "approve", 1, None, None, &time,)
                .is_err()
        );
        tx.execute(
            "UPDATE market_provider_earning_lots SET disputed = 0 WHERE provider_user_id = 'provider'",
            [],
        )
        .unwrap();
        let approved =
            transition_payout_tx(&tx, &request.id, "owner", "approve", 1, None, None, &time)
                .unwrap();
        tx.execute(
            "UPDATE market_finance_control
             SET mode = 'disabled', cutover_approved_at = NULL WHERE id = 1",
            [],
        )
        .unwrap();
        assert!(matches!(
            transition_payout_tx(
                &tx,
                &request.id,
                "owner",
                "begin_transfer",
                approved.version,
                None,
                None,
                &time,
            ),
            Err(AppError::ServiceUnavailable(_))
        ));
        tx.execute(
            "UPDATE market_finance_control
             SET mode = 'enabled', cutover_approved_at = ?1 WHERE id = 1",
            params![time],
        )
        .unwrap();
        let sending = transition_payout_tx(
            &tx,
            &request.id,
            "owner",
            "begin_transfer",
            approved.version,
            None,
            None,
            &time,
        )
        .unwrap();
        let reviewed = transition_payout_tx(
            &tx,
            &request.id,
            "owner",
            "review",
            sending.version,
            None,
            Some("transfer_result_unknown"),
            &time,
        )
        .unwrap();
        assert_eq!(reviewed.status, "review_required");
        assert!(
            transition_payout_tx(
                &tx,
                &request.id,
                "owner",
                "reject",
                reviewed.version,
                None,
                Some("no_external_transfer"),
                time
            )
            .is_err()
        );
        tx.execute(
            "UPDATE market_finance_control
             SET mode = 'disabled', cutover_approved_at = NULL WHERE id = 1",
            [],
        )
        .unwrap();
        transition_payout_tx(
            &tx,
            &request.id,
            "owner",
            "mark_paid",
            reviewed.version,
            Some(TX_ID),
            None,
            &time,
        )
        .unwrap();
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().paid_units,
            8_640_000
        );
        assert!(
            transition_payout_tx(
                &tx,
                &request.id,
                "owner",
                "mark_paid",
                sending.version,
                Some(TX_ID),
                None,
                &time
            )
            .is_err()
        );
        tx.commit().unwrap();
    }

    #[test]
    fn buyer_refund_freezes_only_available_wallet_and_reject_restores_it() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let time = "2026-09-09T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "order", 1000, "receipt", time).unwrap();
        let address = valid_address();
        let request = BuyerRefundRequest {
            amount_minor: 100,
            network: "tron".into(),
            address,
        };
        let refund =
            create_buyer_refund_tx(&tx, "buyer", &request, "refund-idem-one", time).unwrap();
        assert_eq!(
            list_buyer_refunds_tx(&tx, "buyer", None)
                .unwrap()
                .items
                .len(),
            1
        );
        assert!(list_buyer_refunds_tx(&tx, "other", Some(&refund.id)).is_err());
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            9 * 8_640_000
        );
        assert!(
            create_buyer_refund_tx(
                &tx,
                "buyer",
                &BuyerRefundRequest {
                    amount_minor: 1000,
                    network: "tron".into(),
                    address: request.address.clone()
                },
                "refund-idem-two",
                time
            )
            .is_err()
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        transition_buyer_refund_tx(
            &tx,
            &refund.id,
            "owner",
            "reject",
            1,
            None,
            Some("no_external_transfer"),
            time,
        )
        .unwrap();
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            10 * 8_640_000
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        assert!(
            transition_buyer_refund_tx(
                &tx,
                &refund.id,
                "owner",
                "mark_paid",
                1,
                Some("tx-invalid"),
                None,
                time
            )
            .is_err()
        );
        tx.commit().unwrap();
    }
}
