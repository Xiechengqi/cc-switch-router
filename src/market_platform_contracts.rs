//! Wallet-funded contracts. These records intentionally do not reuse the
//! Provider-prepaid contract tables: the two rails must never settle one
//! service interval twice. Callers supply observed control-plane evidence and
//! run every transition inside an IMMEDIATE database transaction.

use chrono::{DateTime, Datelike, Utc};
use uuid::Uuid;

use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;
use crate::market_wallet::{
    capture_hold_tx, credit_wallet_from_liability_tx, debit_available_tx, release_hold_tx,
    reserve_wallet_tx, units_from_minor,
};

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AppError::BadRequest("invalid market service timestamp".into()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BillingModel {
    Metered,
    CalendarMonth,
}

impl BillingModel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Metered => "metered",
            Self::CalendarMonth => "calendar_month",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PrepareContract<'a> {
    pub product_kind: &'a str,
    pub product_ref: &'a str,
    /// Share ID or Host ID: the immutable unit of per-Provider free trial.
    pub trial_scope_ref: &'a str,
    pub quote_id: &'a str,
    pub buyer_user_id: &'a str,
    pub provider_user_id: &'a str,
    pub offer_revision: i64,
    pub billing_model: BillingModel,
    /// Cents per 24 hours for metered service, or cents per calendar month.
    pub price_minor: i64,
    pub auto_renew: bool,
    /// Healthy free-service seconds. A trial does not reserve buyer money.
    pub trial_seconds: i64,
}

pub(crate) fn contract_for_product_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT id FROM market_platform_contracts
         WHERE product_kind = ?1 AND product_ref = ?2",
        params![product_kind, product_ref],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_error("find platform contract"))
}

/// Bind a successful Client provisioning job to its immutable installation.
/// Both identities must match the quoted contract before any hold is captured.
pub(crate) fn bind_client_installation_tx(
    conn: &Connection,
    contract_id: &str,
    job_id: &str,
    installation_id: &str,
    buyer_user_id: &str,
    provider_user_id: &str,
    now: &str,
) -> Result<(), AppError> {
    if installation_id.is_empty() {
        return Err(AppError::BadRequest("installation ID is required".into()));
    }
    parse_time(now)?;
    let changed = conn
        .execute(
            "UPDATE market_platform_contracts
             SET service_ref = ?3, updated_at = ?6
             WHERE id = ?1 AND product_kind = 'client_host' AND product_ref = ?2
               AND buyer_user_id = ?4 AND provider_user_id = ?5
               AND status = 'pending_activation'
               AND (service_ref IS NULL OR service_ref = ?3)",
            params![
                contract_id,
                job_id,
                installation_id,
                buyer_user_id,
                provider_user_id,
                now
            ],
        )
        .map_err(db_error("bind platform Client installation"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "Client installation does not match its wallet contract".into(),
        ));
    }
    Ok(())
}

pub(crate) fn trial_seconds_remaining_tx(
    conn: &Connection,
    buyer_user_id: &str,
    provider_user_id: &str,
    product_kind: &str,
    trial_scope_ref: &str,
    offered_seconds: i64,
) -> Result<i64, AppError> {
    if !matches!(product_kind, "share" | "client_host")
        || buyer_user_id.is_empty()
        || provider_user_id.is_empty()
        || trial_scope_ref.is_empty()
        || !(0..=30 * 86_400).contains(&offered_seconds)
    {
        return Err(AppError::BadRequest("invalid wallet trial scope".into()));
    }
    if offered_seconds == 0 {
        return Ok(0);
    }
    let consumed: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(segment.consumed_seconds), 0)
             FROM market_platform_trial_segments segment
             JOIN market_platform_contracts contract ON contract.id = segment.contract_id
             WHERE contract.buyer_user_id = ?1 AND contract.provider_user_id = ?2
               AND contract.product_kind = ?3 AND contract.trial_scope_ref = ?4",
            params![
                buyer_user_id,
                provider_user_id,
                product_kind,
                trial_scope_ref
            ],
            |row| row.get(0),
        )
        .map_err(db_error("read consumed wallet trial"))?;
    Ok(offered_seconds.saturating_sub(consumed).max(0))
}

/// Reserve only the current month or one day of metered budget. The external
/// entitlement/Host remains pending until activation is independently proven.
pub(crate) fn prepare_contract_tx(
    conn: &Connection,
    input: &PrepareContract<'_>,
    now: &str,
) -> Result<String, AppError> {
    if !matches!(input.product_kind, "share" | "client_host")
        || input.product_ref.is_empty()
        || input.trial_scope_ref.is_empty()
        || input.quote_id.is_empty()
        || input.buyer_user_id.is_empty()
        || input.provider_user_id.is_empty()
        || input.buyer_user_id == input.provider_user_id
        || input.offer_revision <= 0
        || input.price_minor <= 0
        || !(0..=30 * 86_400).contains(&input.trial_seconds)
        || (input.billing_model == BillingModel::Metered && input.auto_renew)
    {
        return Err(AppError::BadRequest("invalid platform contract".into()));
    }
    parse_time(now)?;
    let amount_units = units_from_minor(input.price_minor)?;
    let fingerprint = format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:trial:{}",
        input.product_kind,
        input.product_ref,
        input.trial_scope_ref,
        input.quote_id,
        input.buyer_user_id,
        input.provider_user_id,
        input.offer_revision,
        input.billing_model.as_str(),
        input.price_minor,
        input.trial_seconds,
    ) + if input.auto_renew { ":auto" } else { ":manual" };
    let existing = conn
        .query_row(
            "SELECT id, request_fingerprint FROM market_platform_contracts
             WHERE product_kind = ?1 AND product_ref = ?2",
            params![input.product_kind, input.product_ref],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_error("read wallet contract replay"))?;
    if let Some((id, saved)) = existing {
        if saved != fingerprint {
            return Err(AppError::Conflict(
                "platform contract idempotency conflict".into(),
            ));
        }
        return Ok(id);
    }
    let legacy_rail_present: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_service_contracts
                   WHERE product_kind = ?1 AND product_ref = ?2)
                OR EXISTS(SELECT 1 FROM market_recurring_contracts
                   WHERE product_kind = ?1 AND product_ref = ?2)
                OR EXISTS(SELECT 1 FROM market_funding_reservations
                   WHERE product_kind = ?1 AND product_ref = ?2)",
            params![input.product_kind, input.product_ref],
            |row| row.get(0),
        )
        .map_err(db_error("check legacy funding rail for platform contract"))?;
    if legacy_rail_present != 0 {
        return Err(AppError::Conflict(
            "product already belongs to the legacy Provider-funded rail".into(),
        ));
    }
    if input.trial_seconds
        > trial_seconds_remaining_tx(
            conn,
            input.buyer_user_id,
            input.provider_user_id,
            input.product_kind,
            input.trial_scope_ref,
            input.trial_seconds,
        )?
    {
        return Err(AppError::Conflict(
            "wallet trial allowance changed; request a new quote".into(),
        ));
    }
    if input.trial_seconds > 0 {
        let pending: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_platform_contracts
                 WHERE buyer_user_id = ?1 AND provider_user_id = ?2
                   AND product_kind = ?3 AND trial_scope_ref = ?4
                   AND trial_seconds_remaining > 0
                   AND status IN ('pending_activation', 'trialing'))",
                params![
                    input.buyer_user_id,
                    input.provider_user_id,
                    input.product_kind,
                    input.trial_scope_ref
                ],
                |row| row.get(0),
            )
            .map_err(db_error("check concurrent wallet trial"))?;
        if pending != 0 {
            return Err(AppError::Conflict(
                "another trial is already active for this service".into(),
            ));
        }
    }
    let id = Uuid::new_v4().to_string();
    let hold_id = if input.trial_seconds == 0 {
        Some(reserve_wallet_tx(
            conn,
            input.buyer_user_id,
            "platform_contract",
            &id,
            "activation",
            amount_units,
            None,
            now,
        )?)
    } else {
        None
    };
    let (daily_rate, cycle_price) = match input.billing_model {
        BillingModel::Metered => (Some(input.price_minor), None),
        BillingModel::CalendarMonth => (None, Some(input.price_minor)),
    };
    conn.execute(
        "INSERT INTO market_platform_contracts (
            id, product_kind, product_ref, trial_scope_ref, quote_id, request_fingerprint,
            buyer_user_id, provider_user_id, offer_revision, billing_model,
            daily_rate_minor, cycle_price_minor, renewal_policy, status,
            activation_hold_id, budget_hold_id, budget_sequence,
            trial_seconds_remaining, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                   'pending_activation', ?14, ?15, ?16, ?17, ?18, ?18)",
        params![
            id,
            input.product_kind,
            input.product_ref,
            input.trial_scope_ref,
            input.quote_id,
            fingerprint,
            input.buyer_user_id,
            input.provider_user_id,
            input.offer_revision,
            input.billing_model.as_str(),
            daily_rate,
            cycle_price,
            if input.auto_renew {
                "automatic"
            } else {
                "manual"
            },
            hold_id.as_deref(),
            if input.billing_model == BillingModel::Metered {
                hold_id.as_deref()
            } else {
                None
            },
            if input.billing_model == BillingModel::Metered && hold_id.is_some() {
                1
            } else {
                0
            },
            input.trial_seconds,
            now,
        ],
    )
    .map_err(db_error("create platform contract"))?;
    Ok(id)
}

/// Grant/Host activation and money capture happen in the same local
/// transaction after a trusted control-plane receipt. The external grant is
/// revoked, not paid, if this capture cannot commit.
pub(crate) fn activate_contract_tx(
    conn: &Connection,
    contract_id: &str,
    activated_at: &str,
    now: &str,
) -> Result<(), AppError> {
    let start = parse_time(activated_at)?;
    if start > parse_time(now)? {
        return Err(AppError::BadRequest(
            "activation cannot be in the future".into(),
        ));
    }
    let (model, price, hold_id, status, old_start, trial_remaining): (
        String,
        i64,
        Option<String>,
        String,
        Option<String>,
        i64,
    ) = conn
        .query_row(
            "SELECT billing_model,
                    COALESCE(cycle_price_minor, daily_rate_minor), activation_hold_id,
                    status, activated_at, trial_seconds_remaining
             FROM market_platform_contracts WHERE id = ?1",
            params![contract_id],
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
        .map_err(db_error("read pending platform contract"))?;
    if matches!(status.as_str(), "active" | "trialing")
        && old_start.as_deref() == Some(activated_at)
    {
        return Ok(());
    }
    if status != "pending_activation" {
        return Err(AppError::Conflict(
            "platform contract is not pending activation".into(),
        ));
    }
    if trial_remaining > 0 {
        let changed = conn
            .execute(
                "UPDATE market_platform_contracts
                 SET status = 'trialing', activated_at = ?2,
                     last_evaluated_at = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'pending_activation'
                   AND activation_hold_id IS NULL",
                params![contract_id, activated_at, now],
            )
            .map_err(db_error("activate wallet trial"))?;
        if changed != 1 {
            return Err(AppError::Conflict("wallet trial activation changed".into()));
        }
        return Ok(());
    }
    let hold_id = hold_id.ok_or_else(|| {
        AppError::Internal("paid platform activation is missing its wallet hold".into())
    })?;
    let sequence = if model == "calendar_month" {
        let period_id = Uuid::new_v4().to_string();
        let end = crate::market_recurring::next_calendar_month_at(start, start.day())?;
        let amount = units_from_minor(price)?;
        let journal = capture_hold_tx(
            conn,
            &hold_id,
            amount,
            "unfulfilled",
            &period_id,
            "platform_period",
            &period_id,
            now,
        )?;
        if !journal.inserted {
            return Err(AppError::Internal(
                "new platform period capture was replayed".into(),
            ));
        }
        conn.execute(
            "INSERT INTO market_platform_periods (
                id, contract_id, sequence, period_start, period_end,
                settled_through_at, amount_units, debit_journal_id, debited_at,
                status, created_at, updated_at
             ) VALUES (?1, ?2, 1, ?3, ?4, ?3, ?5, ?6, ?7, 'open', ?7, ?7)",
            params![
                period_id,
                contract_id,
                activated_at,
                end.to_rfc3339(),
                amount,
                journal.id,
                now,
            ],
        )
        .map_err(db_error("create first wallet service period"))?;
        1
    } else if model == "metered" {
        0
    } else {
        return Err(AppError::Internal("stored billing model is invalid".into()));
    };
    let changed = conn
        .execute(
            "UPDATE market_platform_contracts
             SET status = 'active', activated_at = ?2, paid_started_at = ?2,
                 last_evaluated_at = ?2, current_period_sequence = ?3,
                 updated_at = ?4 WHERE id = ?1 AND status = 'pending_activation'",
            params![contract_id, activated_at, sequence, now],
        )
        .map_err(db_error("activate platform contract"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "platform contract activation changed".into(),
        ));
    }
    Ok(())
}

/// An uncertain external grant must retain the buyer's hold. Only confirmed
/// cleanup may return the money to available balance.
pub(crate) fn fail_activation_tx(
    conn: &Connection,
    contract_id: &str,
    cleanup_confirmed: bool,
    now: &str,
) -> Result<(), AppError> {
    let (status, hold_id): (String, Option<String>) = conn
        .query_row(
            "SELECT status, activation_hold_id FROM market_platform_contracts WHERE id = ?1",
            params![contract_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read failed platform activation"))?;
    if status == "activation_failed" && cleanup_confirmed {
        return Ok(());
    }
    if !matches!(status.as_str(), "pending_activation" | "review_required") {
        return Err(AppError::Conflict(
            "platform activation cannot be failed from this state".into(),
        ));
    }
    if cleanup_confirmed {
        if let Some(hold_id) = hold_id {
            release_hold_tx(conn, &hold_id, now)?;
        }
    }
    conn.execute(
        "UPDATE market_platform_contracts SET status = ?2, updated_at = ?3
         WHERE id = ?1 AND status IN ('pending_activation', 'review_required')",
        params![
            contract_id,
            if cleanup_confirmed {
                "activation_failed"
            } else {
                "review_required"
            },
            now,
        ],
    )
    .map_err(db_error("record failed platform activation"))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrialAdvanceResult {
    pub consumed_seconds: i64,
    pub paid_started: bool,
    pub needs_pause: bool,
    pub replayed: bool,
}

fn start_paid_trial_tx(
    conn: &Connection,
    contract_id: &str,
    at: &str,
    now: &str,
) -> Result<bool, AppError> {
    let (buyer, model, price, status, remaining): (String, String, i64, String, i64) = conn
        .query_row(
            "SELECT buyer_user_id, billing_model, COALESCE(cycle_price_minor, daily_rate_minor),
                    status, trial_seconds_remaining
             FROM market_platform_contracts WHERE id = ?1",
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
        .map_err(db_error("read trial conversion contract"))?;
    if status != "trialing" || remaining != 0 {
        return Err(AppError::Conflict(
            "wallet trial is not ready to convert".into(),
        ));
    }
    crate::market_reconcile::ensure_new_finance_writes_safe_tx(conn)?;
    let units = units_from_minor(price)?;
    if crate::market_wallet::wallet_snapshot_tx(conn, &buyer)?.available_units < units {
        conn.execute(
            "UPDATE market_platform_contracts SET status = 'suspended', updated_at = ?2
             WHERE id = ?1 AND status = 'trialing' AND trial_seconds_remaining = 0",
            params![contract_id, now],
        )
        .map_err(db_error("suspend unfunded trial conversion"))?;
        return Ok(false);
    }
    let hold_id = reserve_wallet_tx(
        conn,
        &buyer,
        "platform_contract",
        contract_id,
        "activation",
        units,
        None,
        now,
    )?;
    if model == "calendar_month" {
        let start = parse_time(at)?;
        let end = crate::market_recurring::next_calendar_month_at(start, start.day())?;
        let period_id = Uuid::new_v4().to_string();
        let journal = capture_hold_tx(
            conn,
            &hold_id,
            units,
            "unfulfilled",
            &period_id,
            "platform_period",
            &period_id,
            now,
        )?;
        if !journal.inserted {
            return Err(AppError::Internal(
                "trial period capture was replayed".into(),
            ));
        }
        conn.execute(
            "INSERT INTO market_platform_periods (
                id, contract_id, sequence, period_start, period_end,
                settled_through_at, amount_units, debit_journal_id, debited_at,
                status, created_at, updated_at
             ) VALUES (?1, ?2, 1, ?3, ?4, ?3, ?5, ?6, ?7, 'open', ?7, ?7)",
            params![
                period_id,
                contract_id,
                at,
                end.to_rfc3339(),
                units,
                journal.id,
                now
            ],
        )
        .map_err(db_error("create first paid period after trial"))?;
        let changed = conn
            .execute(
                "UPDATE market_platform_contracts
             SET status = 'active', activation_hold_id = ?2, paid_started_at = ?3,
                 current_period_sequence = 1, updated_at = ?4
             WHERE id = ?1 AND status = 'trialing' AND trial_seconds_remaining = 0",
                params![contract_id, hold_id, at, now],
            )
            .map_err(db_error("activate paid monthly service after trial"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "monthly trial conversion changed".into(),
            ));
        }
    } else if model == "metered" {
        let changed = conn
            .execute(
                "UPDATE market_platform_contracts
             SET status = 'active', activation_hold_id = ?2, budget_hold_id = ?2,
                 budget_sequence = 1, paid_started_at = ?3, updated_at = ?4
             WHERE id = ?1 AND status = 'trialing' AND trial_seconds_remaining = 0",
                params![contract_id, hold_id, at, now],
            )
            .map_err(db_error("activate paid metered service after trial"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "metered trial conversion changed".into(),
            ));
        }
    } else {
        return Err(AppError::Internal(
            "stored trial billing model is invalid".into(),
        ));
    }
    Ok(true)
}

/// Consume only observed healthy trial time. The caller splits an interval at
/// the exact trial boundary; no future paid seconds may be hidden inside a
/// free segment. Insufficient funds leaves a suspended, non-earning contract.
#[allow(clippy::too_many_arguments)]
pub(crate) fn advance_trial_interval_tx(
    conn: &Connection,
    contract_id: &str,
    source_kind: &str,
    source_id: &str,
    started_at: &str,
    ended_at: &str,
    service_available: bool,
    now: &str,
) -> Result<TrialAdvanceResult, AppError> {
    let start = parse_time(started_at)?;
    let end = parse_time(ended_at)?;
    if source_id.is_empty()
        || source_id.len() > 160
        || start >= end
        || end > parse_time(now)?
        || (end - start).num_seconds() > 3_600
        || (end - start).num_milliseconds() % 1_000 != 0
    {
        return Err(AppError::BadRequest(
            "invalid wallet trial observation".into(),
        ));
    }
    let replay = conn
        .query_row(
            "SELECT started_at, ended_at, service_available, consumed_seconds
             FROM market_platform_trial_segments
             WHERE contract_id = ?1 AND source_id = ?2",
            params![contract_id, source_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("read wallet trial replay"))?;
    if let Some((saved_start, saved_end, saved_available, consumed)) = replay {
        if parse_time(&saved_start)? != start
            || parse_time(&saved_end)? != end
            || saved_available != service_available
        {
            return Err(AppError::Conflict("wallet trial evidence changed".into()));
        }
        let status: String = conn
            .query_row(
                "SELECT status FROM market_platform_contracts WHERE id = ?1",
                params![contract_id],
                |row| row.get(0),
            )
            .map_err(db_error("read replayed wallet trial state"))?;
        return Ok(TrialAdvanceResult {
            consumed_seconds: consumed,
            paid_started: status == "active",
            needs_pause: status == "suspended",
            replayed: true,
        });
    }
    let (kind, status, remaining, cursor): (String, String, i64, String) = conn
        .query_row(
            "SELECT product_kind, status, trial_seconds_remaining, last_evaluated_at
         FROM market_platform_contracts WHERE id = ?1",
            params![contract_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(db_error("read active wallet trial"))?;
    if status != "trialing" || parse_time(&cursor)? != start {
        return Err(AppError::Conflict(
            "wallet trial is not at this observation".into(),
        ));
    }
    if (kind == "share" && source_kind != "share_control")
        || (kind == "client_host" && source_kind != "client_host_health")
    {
        return Err(AppError::BadRequest(
            "wrong wallet trial evidence rail".into(),
        ));
    }
    let seconds = (end - start).num_seconds();
    if service_available && seconds > remaining {
        return Err(AppError::Conflict(
            "split trial evidence at the paid-service boundary".into(),
        ));
    }
    let consumed = if service_available { seconds } else { 0 };
    conn.execute(
        "INSERT INTO market_platform_trial_segments (
            id, contract_id, source_kind, source_id, started_at, ended_at,
            service_available, consumed_seconds, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            Uuid::new_v4().to_string(),
            contract_id,
            source_kind,
            source_id,
            started_at,
            ended_at,
            i64::from(service_available),
            consumed,
            now
        ],
    )
    .map_err(db_error("persist wallet trial service observation"))?;
    let changed = conn
        .execute(
            "UPDATE market_platform_contracts
         SET trial_seconds_remaining = trial_seconds_remaining - ?2,
             last_evaluated_at = ?3, updated_at = ?4
         WHERE id = ?1 AND status = 'trialing'
           AND trial_seconds_remaining >= ?2 AND last_evaluated_at = ?5",
            params![contract_id, consumed, ended_at, now, cursor],
        )
        .map_err(db_error("advance wallet trial"))?;
    if changed != 1 {
        return Err(AppError::Conflict("wallet trial cursor changed".into()));
    }
    let paid_started = service_available
        && consumed == remaining
        && start_paid_trial_tx(conn, contract_id, ended_at, now)?;
    Ok(TrialAdvanceResult {
        consumed_seconds: consumed,
        paid_started,
        needs_pause: service_available && consumed == remaining && !paid_started,
        replayed: false,
    })
}

/// Persist one trusted healthy-service interval before any financial posting.
/// An overlap under another source ID is rejected to prevent duplicate billing.
pub(crate) fn observe_metered_interval_tx(
    conn: &Connection,
    contract_id: &str,
    source_kind: &str,
    source_id: &str,
    started_at: &str,
    ended_at: &str,
    now: &str,
) -> Result<String, AppError> {
    let start = parse_time(started_at)?;
    let end = parse_time(ended_at)?;
    if source_id.is_empty() || source_id.len() > 160 || start >= end || end > parse_time(now)? {
        return Err(AppError::BadRequest(
            "invalid metered service evidence".into(),
        ));
    }
    let duration_ms = (end - start).num_milliseconds();
    if duration_ms <= 0 || duration_ms % 1_000 != 0 || duration_ms > 3_600_000 {
        return Err(AppError::BadRequest(
            "metered interval must contain whole seconds within one hour".into(),
        ));
    }
    let (product_kind, buyer, provider, status, activated_at, rate): (
        String,
        String,
        String,
        String,
        String,
        i64,
    ) = conn
        .query_row(
            "SELECT product_kind, buyer_user_id, provider_user_id, status,
                paid_started_at, daily_rate_minor FROM market_platform_contracts
         WHERE id = ?1 AND billing_model = 'metered'",
            params![contract_id],
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
        .map_err(db_error("read metered wallet contract"))?;
    if status != "active" || start < parse_time(&activated_at)? {
        return Err(AppError::Conflict("metered service is not active".into()));
    }
    if (product_kind == "share" && source_kind != "share_control")
        || (product_kind == "client_host" && source_kind != "client_host_health")
    {
        return Err(AppError::BadRequest("wrong metered evidence rail".into()));
    }
    let seconds = duration_ms / 1_000;
    let units = rate
        .checked_mul(seconds)
        .ok_or_else(|| AppError::BadRequest("metered amount is too large".into()))?;
    let replay = conn
        .query_row(
            "SELECT id, contract_id, started_at, ended_at, amount_units
         FROM market_platform_metered_observations
         WHERE source_kind = ?1 AND source_id = ?2",
            params![source_kind, source_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("read metered evidence replay"))?;
    if let Some((id, saved_contract, saved_start, saved_end, saved_units)) = replay {
        if saved_contract != contract_id
            || parse_time(&saved_start)? != start
            || parse_time(&saved_end)? != end
            || saved_units != units
        {
            return Err(AppError::Conflict(
                "metered evidence idempotency conflict".into(),
            ));
        }
        return Ok(id);
    }
    let start_text = start.to_rfc3339();
    let end_text = end.to_rfc3339();
    let overlap: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_platform_metered_observations
         WHERE contract_id = ?1 AND started_at < ?3 AND ended_at > ?2)",
            params![contract_id, start_text, end_text],
            |row| row.get(0),
        )
        .map_err(db_error("check metered evidence overlap"))?;
    if overlap != 0 {
        return Err(AppError::Conflict(
            "metered evidence overlaps prior service".into(),
        ));
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO market_platform_metered_observations (
            id, contract_id, source_kind, source_id, started_at, ended_at,
            billable_seconds, amount_units, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            contract_id,
            source_kind,
            source_id,
            start_text,
            end_text,
            seconds,
            units,
            now
        ],
    )
    .map_err(db_error("persist metered service evidence"))?;
    let _ = (buyer, provider);
    Ok(id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeteredBatchResult {
    pub captured_units: i64,
    pub pending_observations: i64,
    pub needs_pause: bool,
}

fn replenish_metered_budget_tx(
    conn: &Connection,
    contract_id: &str,
    buyer: &str,
    rate: i64,
    prior_sequence: i64,
    now: &str,
) -> Result<Option<String>, AppError> {
    let available = crate::market_wallet::wallet_snapshot_tx(conn, buyer)?.available_units;
    let daily_units = units_from_minor(rate)?;
    let budget = available.min(daily_units) / rate * rate;
    if budget < rate {
        return Ok(None);
    }
    let sequence = prior_sequence + 1;
    let source = format!("{contract_id}:budget:{sequence}");
    let hold_id = reserve_wallet_tx(
        conn,
        buyer,
        "platform_metered_budget",
        &source,
        "metered_budget",
        budget,
        None,
        now,
    )?;
    let changed = conn
        .execute(
            "UPDATE market_platform_contracts SET budget_hold_id = ?2,
                budget_sequence = ?3, updated_at = ?4
         WHERE id = ?1 AND budget_sequence = ?5 AND status IN ('active', 'ending')",
            params![contract_id, hold_id, sequence, now, prior_sequence],
        )
        .map_err(db_error("advance metered budget"))?;
    if changed != 1 {
        return Err(AppError::Conflict("metered budget changed".into()));
    }
    Ok(Some(hold_id))
}

/// Move at most one minute of contiguous persisted evidence through a single
/// wallet hold. A short final budget may cover only a prefix; the remainder
/// stays visibly pending and the caller must pause the service if no new
/// budget can be reserved.
pub(crate) fn settle_metered_batch_tx(
    conn: &Connection,
    contract_id: &str,
    now: &str,
) -> Result<MeteredBatchResult, AppError> {
    parse_time(now)?;
    let (kind, buyer, provider, status, rate, mut hold_id, sequence): (
        String,
        String,
        String,
        String,
        i64,
        String,
        i64,
    ) = conn
        .query_row(
            "SELECT product_kind, buyer_user_id, provider_user_id, status,
                daily_rate_minor, budget_hold_id, budget_sequence
         FROM market_platform_contracts WHERE id = ?1 AND billing_model = 'metered'",
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
        .map_err(db_error("read wallet metered budget"))?;
    if !matches!(status.as_str(), "active" | "ending") {
        return Err(AppError::Conflict("metered contract is not active".into()));
    }
    // No observed service means no new budget or financial suspension is due.
    if pending_metered_count_tx(conn, contract_id)? == 0 {
        return Ok(MeteredBatchResult {
            captured_units: 0,
            pending_observations: 0,
            needs_pause: false,
        });
    }
    let mut remaining_hold: i64 = conn
        .query_row(
            "SELECT remaining_units FROM market_wallet_holds
         WHERE id = ?1 AND user_id = ?2 AND status = 'active'",
            params![hold_id, buyer],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read metered hold"))?
        .unwrap_or(0);
    if remaining_hold < rate {
        if let Some(next) =
            replenish_metered_budget_tx(conn, contract_id, &buyer, rate, sequence, now)?
        {
            hold_id = next;
            remaining_hold = conn
                .query_row(
                    "SELECT remaining_units FROM market_wallet_holds WHERE id = ?1",
                    params![hold_id],
                    |row| row.get(0),
                )
                .map_err(db_error("read replenished metered budget"))?;
        } else {
            conn.execute(
                "UPDATE market_platform_contracts SET status = 'suspended', updated_at = ?2
                 WHERE id = ?1 AND status = 'active'",
                params![contract_id, now],
            )
            .map_err(db_error("suspend unfunded metered service"))?;
            let pending = pending_metered_count_tx(conn, contract_id)?;
            return Ok(MeteredBatchResult {
                captured_units: 0,
                pending_observations: pending,
                needs_pause: true,
            });
        }
    }
    let mut statement = conn
        .prepare(
            "SELECT id, started_at, billable_seconds, captured_seconds
         FROM market_platform_metered_observations
         WHERE contract_id = ?1 AND captured_seconds + unfunded_seconds < billable_seconds
         ORDER BY started_at, id LIMIT 100",
        )
        .map_err(db_error("prepare metered evidence batch"))?;
    let observations = statement
        .query_map(params![contract_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(db_error("query metered evidence batch"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read metered evidence batch"))?;
    let max_seconds = (remaining_hold / rate).min(60);
    let mut used_seconds = 0_i64;
    let mut items = Vec::new();
    let mut batch_start = None;
    let mut batch_end = None;
    for (observation_id, started_at, billable, captured) in observations {
        if used_seconds >= max_seconds {
            break;
        }
        let slice_start = parse_time(&started_at)? + chrono::Duration::seconds(captured);
        if batch_end.is_some_and(|prior| prior != slice_start) {
            break;
        }
        let seconds = (billable - captured).min(max_seconds - used_seconds);
        if seconds <= 0 {
            return Err(AppError::Internal(
                "invalid pending metered evidence".into(),
            ));
        }
        let slice_end = slice_start + chrono::Duration::seconds(seconds);
        batch_start.get_or_insert(slice_start);
        batch_end = Some(slice_end);
        items.push((observation_id, captured, seconds));
        used_seconds += seconds;
    }
    if items.is_empty() {
        return Ok(MeteredBatchResult {
            captured_units: 0,
            pending_observations: 0,
            needs_pause: false,
        });
    }
    let units = used_seconds
        .checked_mul(rate)
        .ok_or_else(|| AppError::Internal("metered batch amount overflow".into()))?;
    let batch_id = Uuid::new_v4().to_string();
    let first_cursor = format!("{}:{}", items[0].0, items[0].1);
    let last_item = items.last().unwrap();
    let last_cursor = format!("{}:{}", last_item.0, last_item.1 + last_item.2);
    let (_, journal_id) = crate::market_finance::record_captured_metered_delivery_tx(
        conn,
        &kind,
        contract_id,
        &batch_id,
        &hold_id,
        &buyer,
        &provider,
        &batch_start.unwrap().to_rfc3339(),
        &batch_end.unwrap().to_rfc3339(),
        units,
        now,
    )?;
    conn.execute(
        "INSERT INTO market_usage_posting_batches (
            id, contract_id, first_accrual_id, last_accrual_id,
            started_at, ended_at, amount_units, journal_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            batch_id,
            contract_id,
            first_cursor,
            last_cursor,
            batch_start.unwrap().to_rfc3339(),
            batch_end.unwrap().to_rfc3339(),
            units,
            journal_id,
            now
        ],
    )
    .map_err(db_error("record wallet usage batch"))?;
    for (observation_id, first_second, seconds) in items {
        conn.execute(
            "INSERT INTO market_platform_usage_batch_items (
                batch_id, observation_id, first_second, seconds, amount_units
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                batch_id,
                observation_id,
                first_second,
                seconds,
                seconds * rate
            ],
        )
        .map_err(db_error("link metered batch evidence"))?;
        let changed = conn
            .execute(
                "UPDATE market_platform_metered_observations
             SET captured_seconds = captured_seconds + ?2
             WHERE id = ?1 AND captured_seconds = ?3
               AND billable_seconds >= ?3 + unfunded_seconds + ?2",
                params![observation_id, seconds, first_second],
            )
            .map_err(db_error("advance metered evidence cursor"))?;
        if changed != 1 {
            return Err(AppError::Conflict("metered evidence changed".into()));
        }
    }
    Ok(MeteredBatchResult {
        captured_units: units,
        pending_observations: pending_metered_count_tx(conn, contract_id)?,
        needs_pause: false,
    })
}

fn pending_metered_count_tx(conn: &Connection, contract_id: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COUNT(*) FROM market_platform_metered_observations
         WHERE contract_id = ?1 AND captured_seconds + unfunded_seconds < billable_seconds",
        params![contract_id],
        |row| row.get(0),
    )
    .map_err(db_error("count pending metered evidence"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContractTermination {
    pub refunded_units: i64,
    pub unfunded_observations: i64,
    pub cleanup_confirmed: bool,
    pub finalized: bool,
}

/// A failed or uncertain revoke never releases a hold or refunds unfulfilled
/// consideration. Confirmed cleanup closes the service rail first, then
/// refunds the remaining month or releases unused metered budget.
pub(crate) fn terminate_after_cleanup_tx(
    conn: &Connection,
    contract_id: &str,
    cleanup_confirmed: bool,
    now: &str,
) -> Result<ContractTermination, AppError> {
    parse_time(now)?;
    let (buyer, model, status, hold_id, sequence, paid_started_at, activation_hold_id): (
        String,
        String,
        String,
        Option<String>,
        i64,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT buyer_user_id, billing_model, status, budget_hold_id,
                    current_period_sequence, paid_started_at, activation_hold_id
             FROM market_platform_contracts WHERE id = ?1",
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
        .map_err(db_error("read platform contract termination"))?;
    if status == "ended" {
        return Ok(ContractTermination {
            refunded_units: 0,
            unfunded_observations: 0,
            cleanup_confirmed: true,
            finalized: true,
        });
    }
    if !matches!(
        status.as_str(),
        "trialing" | "active" | "suspended" | "ending" | "review_required"
    ) {
        return Err(AppError::Conflict("platform contract cannot end".into()));
    }
    if !cleanup_confirmed {
        // An uncertain remote revoke also disputes earnings from earlier
        // observed slices. Otherwise the seven-day worker could make them
        // withdrawable while access and refund liability are unresolved.
        conn.execute(
            "UPDATE market_provider_earning_lots SET disputed = 1
             WHERE delivery_slice_id IN (
                 SELECT id FROM market_delivery_slices
                 WHERE contract_id = ?1
             ) AND disputed = 0",
            params![contract_id],
        )
        .map_err(db_error("freeze earnings for uncertain cleanup"))?;
        conn.execute(
            "INSERT OR IGNORE INTO market_finance_reconciliation_cases
                (id, code, object_kind, object_id, status, detail_code, created_at)
             VALUES (?1, 'wallet_cleanup_uncertain', 'platform_contract', ?2,
                     'open', 'await_control_and_delivery_evidence', ?3)",
            params![Uuid::new_v4().to_string(), contract_id, now],
        )
        .map_err(db_error("open uncertain wallet cleanup case"))?;
        conn.execute(
            "UPDATE market_platform_contracts SET status = 'review_required',
                    updated_at = ?2 WHERE id = ?1 AND status != 'ended'",
            params![contract_id, now],
        )
        .map_err(db_error("isolate uncertain platform cleanup"))?;
        return Ok(ContractTermination {
            refunded_units: 0,
            unfunded_observations: 0,
            cleanup_confirmed: false,
            finalized: false,
        });
    }
    if paid_started_at.is_none() {
        if let Some(hold_id) = activation_hold_id {
            release_hold_tx(conn, &hold_id, now)?;
        }
        conn.execute(
            "UPDATE market_platform_contracts
             SET status = 'ended', ended_at = ?2, updated_at = ?2
             WHERE id = ?1 AND status != 'ended'",
            params![contract_id, now],
        )
        .map_err(db_error("end unbilled wallet trial"))?;
        return Ok(ContractTermination {
            refunded_units: 0,
            unfunded_observations: 0,
            cleanup_confirmed: true,
            finalized: true,
        });
    }
    let mut refunded_units = 0;
    let mut unfunded_observations = 0;
    if model == "calendar_month" {
        let (period_id, cursor, end, total, fulfilled, refunded, period_status): (
            String,
            String,
            String,
            i64,
            i64,
            i64,
            String,
        ) = conn
            .query_row(
                "SELECT id, settled_through_at, period_end, amount_units,
                    fulfilled_units, refunded_units, status
             FROM market_platform_periods WHERE contract_id = ?1 AND sequence = ?2",
                params![contract_id, sequence],
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
            .map_err(db_error("read final wallet month"))?;
        if period_status == "open" {
            refunded_units = total
                .checked_sub(fulfilled)
                .and_then(|value| value.checked_sub(refunded))
                .ok_or_else(|| AppError::Internal("monthly liability projection drift".into()))?;
            let journal_id = if refunded_units > 0 {
                Some(
                    credit_wallet_from_liability_tx(
                        conn,
                        &buyer,
                        refunded_units,
                        "unfulfilled",
                        &period_id,
                        "platform_termination",
                        contract_id,
                        "refund_current_month",
                        now,
                    )?
                    .id,
                )
            } else {
                None
            };
            if parse_time(&cursor)? < parse_time(&end)? {
                conn.execute(
                    "INSERT INTO market_platform_monthly_segments (
                        id, period_id, source_id, started_at, ended_at,
                        service_available, amount_units, journal_id, created_at
                     ) VALUES (?1, ?2, 'confirmed_cleanup', ?3, ?4, 0, ?5, ?6, ?7)",
                    params![
                        Uuid::new_v4().to_string(),
                        period_id,
                        cursor,
                        end,
                        refunded_units,
                        journal_id,
                        now
                    ],
                )
                .map_err(db_error("record unserved final month"))?;
            }
            conn.execute(
                "UPDATE market_platform_periods SET refunded_units = refunded_units + ?2,
                        settled_through_at = period_end, status = 'closed', updated_at = ?3
                 WHERE id = ?1 AND status = 'open'",
                params![period_id, refunded_units, now],
            )
            .map_err(db_error("close final wallet month"))?;
        }
    } else if model == "metered" {
        // Cleanup has been observed, but previously persisted billable time
        // still has first claim on its reserved budget. Keep the hold and the
        // contract open across bounded passes instead of silently writing
        // funded service off as an unfunded incident.
        if matches!(status.as_str(), "active" | "review_required" | "ending") {
            conn.execute(
                "UPDATE market_platform_contracts SET status = 'ending', updated_at = ?2
                 WHERE id = ?1 AND status IN ('active', 'review_required')",
                params![contract_id, now],
            )
            .map_err(db_error("begin wallet metered financial cleanup"))?;
            let mut needs_pause = false;
            for _ in 0..60 {
                let result = settle_metered_batch_tx(conn, contract_id, now)?;
                if result.pending_observations == 0 {
                    break;
                }
                if result.needs_pause {
                    needs_pause = true;
                    break;
                }
            }
            if pending_metered_count_tx(conn, contract_id)? > 0 && !needs_pause {
                return Ok(ContractTermination {
                    refunded_units: 0,
                    unfunded_observations: 0,
                    cleanup_confirmed: true,
                    finalized: false,
                });
            }
        }
        unfunded_observations = pending_metered_count_tx(conn, contract_id)?;
        if unfunded_observations > 0 {
            conn.execute(
                "UPDATE market_platform_metered_observations
                 SET unfunded_seconds = billable_seconds - captured_seconds
                 WHERE contract_id = ?1
                   AND captured_seconds + unfunded_seconds < billable_seconds",
                params![contract_id],
            )
            .map_err(db_error("write off unfunded metered evidence"))?;
            conn.execute(
                "INSERT OR IGNORE INTO market_finance_reconciliation_cases
                    (id, code, object_kind, object_id, status, detail_code, created_at)
                 VALUES (?1, 'metered_unfunded_delivery', 'platform_contract', ?2,
                         'open', 'cleanup_after_budget_exhausted', ?3)",
                params![Uuid::new_v4().to_string(), contract_id, now],
            )
            .map_err(db_error("open unfunded service case"))?;
        }
        let hold_id =
            hold_id.ok_or_else(|| AppError::Internal("metered budget hold is missing".into()))?;
        release_hold_tx(conn, &hold_id, now)?;
    } else {
        return Err(AppError::Internal("stored billing model is invalid".into()));
    }
    conn.execute(
        "UPDATE market_platform_contracts SET status = 'ended', ended_at = ?2,
                updated_at = ?2 WHERE id = ?1 AND status != 'ended'",
        params![contract_id, now],
    )
    .map_err(db_error("end wallet service contract"))?;
    Ok(ContractTermination {
        refunded_units,
        unfunded_observations,
        cleanup_confirmed: true,
        finalized: true,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonthlySegmentResult {
    pub amount_units: i64,
    pub replayed: bool,
}

/// Each segment must abut the previous one. Its source is a persisted,
/// immutable service-control observation; missing evidence is refunded, never
/// promoted to Provider income. Cumulative rounding makes the last segment
/// absorb the exact integer remainder.
pub(crate) fn settle_monthly_segment_tx(
    conn: &Connection,
    contract_id: &str,
    source_id: &str,
    started_at: &str,
    ended_at: &str,
    service_available: bool,
    now: &str,
) -> Result<MonthlySegmentResult, AppError> {
    if source_id.is_empty() || source_id.len() > 160 {
        return Err(AppError::BadRequest(
            "invalid monthly evidence source".into(),
        ));
    }
    let start = parse_time(started_at)?;
    let end = parse_time(ended_at)?;
    if start >= end {
        return Err(AppError::BadRequest(
            "empty monthly service interval".into(),
        ));
    }
    let (
        period_id,
        product_kind,
        buyer,
        provider,
        status,
        period_status,
        period_start,
        period_end,
        cursor,
        total,
        fulfilled,
        refunded,
        debit_journal,
        debited_at,
    ): (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
        i64,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT period.id, contract.product_kind, contract.buyer_user_id,
                contract.provider_user_id, contract.status, period.status,
                period.period_start, period.period_end,
                period.settled_through_at, period.amount_units, period.fulfilled_units,
                period.refunded_units, period.debit_journal_id, period.debited_at
         FROM market_platform_contracts contract
         JOIN market_platform_periods period ON period.contract_id = contract.id
          AND period.sequence = contract.current_period_sequence
         WHERE contract.id = ?1 AND contract.billing_model = 'calendar_month'",
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
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                ))
            },
        )
        .map_err(db_error("read open wallet service period"))?;
    if !matches!(status.as_str(), "active" | "ending") {
        return Err(AppError::Conflict("monthly contract is not serving".into()));
    }
    let replay = conn
        .query_row(
            "SELECT started_at, ended_at, service_available, amount_units
         FROM market_platform_monthly_segments WHERE period_id = ?1 AND source_id = ?2",
            params![period_id, source_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("read monthly service replay"))?;
    if let Some((saved_start, saved_end, saved_available, amount)) = replay {
        if parse_time(&saved_start)? != start
            || parse_time(&saved_end)? != end
            || saved_available != i64::from(service_available)
        {
            return Err(AppError::Conflict(
                "monthly evidence idempotency conflict".into(),
            ));
        }
        return Ok(MonthlySegmentResult {
            amount_units: amount,
            replayed: true,
        });
    }
    if period_status != "open" {
        return Err(AppError::Conflict(
            "monthly period is already closed".into(),
        ));
    }
    let first = parse_time(&period_start)?;
    let last = parse_time(&period_end)?;
    if start != parse_time(&cursor)? || end > last || start < first {
        return Err(AppError::Conflict(
            "monthly evidence is out of order or outside the paid period".into(),
        ));
    }
    if service_available && end > parse_time(now)? {
        return Err(AppError::BadRequest(
            "future service cannot be earned".into(),
        ));
    }
    let duration_ms = (last - first).num_milliseconds();
    let elapsed_ms = (end - first).num_milliseconds();
    if duration_ms <= 0 || elapsed_ms <= 0 {
        return Err(AppError::Internal(
            "stored monthly period is invalid".into(),
        ));
    }
    let target: i64 = (i128::from(total) * i128::from(elapsed_ms) / i128::from(duration_ms))
        .try_into()
        .map_err(|_| AppError::Internal("monthly allocation overflow".into()))?;
    let allocated = fulfilled
        .checked_add(refunded)
        .ok_or_else(|| AppError::Internal("monthly allocation overflow".into()))?;
    let amount = target - allocated;
    if amount < 0 || amount > total - allocated {
        return Err(AppError::Internal(
            "monthly allocation projection drift".into(),
        ));
    }
    let segment_id = Uuid::new_v4().to_string();
    let journal_id = if amount == 0 {
        None
    } else if service_available {
        let slice_id = crate::market_finance::record_monthly_delivery_tx(
            conn,
            &product_kind,
            contract_id,
            &period_id,
            &segment_id,
            &buyer,
            &provider,
            started_at,
            ended_at,
            amount,
            &debit_journal,
            &debited_at,
            now,
        )?;
        Some(
            conn.query_row(
                "SELECT earning_journal_id FROM market_delivery_slices WHERE id = ?1",
                params![slice_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(db_error("read monthly earning journal"))?,
        )
    } else {
        Some(
            credit_wallet_from_liability_tx(
                conn,
                &buyer,
                amount,
                "unfulfilled",
                &period_id,
                "monthly_segment",
                &segment_id,
                "refund",
                now,
            )?
            .id,
        )
    };
    conn.execute(
        "INSERT INTO market_platform_monthly_segments (
            id, period_id, source_id, started_at, ended_at,
            service_available, amount_units, journal_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            segment_id,
            period_id,
            source_id,
            started_at,
            ended_at,
            i64::from(service_available),
            amount,
            journal_id,
            now
        ],
    )
    .map_err(db_error("record monthly service evidence"))?;
    let changed = conn
        .execute(
            "UPDATE market_platform_periods
         SET settled_through_at = ?2, fulfilled_units = fulfilled_units + ?3,
             refunded_units = refunded_units + ?4,
             status = CASE WHEN ?5 = 1 THEN 'closed' ELSE status END,
             updated_at = ?6
         WHERE id = ?1 AND settled_through_at = ?7 AND status = 'open'",
            params![
                period_id,
                ended_at,
                if service_available { amount } else { 0 },
                if service_available { 0 } else { amount },
                i64::from(end == last),
                now,
                cursor
            ],
        )
        .map_err(db_error("advance monthly service cursor"))?;
    if changed != 1 {
        return Err(AppError::Conflict("monthly period changed".into()));
    }
    Ok(MonthlySegmentResult {
        amount_units: amount,
        replayed: false,
    })
}

/// Do not reserve the next month in advance. Only a closed, already-ended
/// period may auto-renew; insufficient wallet funds suspend the contract.
pub(crate) fn renew_monthly_contract_tx(
    conn: &Connection,
    contract_id: &str,
    now: &str,
) -> Result<bool, AppError> {
    let at = parse_time(now)?;
    let (buyer, policy, status, price, sequence, anchor): (
        String,
        String,
        String,
        i64,
        i64,
        String,
    ) = conn
        .query_row(
            "SELECT buyer_user_id, renewal_policy, status, cycle_price_minor,
                current_period_sequence, paid_started_at
         FROM market_platform_contracts
         WHERE id = ?1 AND billing_model = 'calendar_month'",
            params![contract_id],
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
        .map_err(db_error("read monthly contract for renewal"))?;
    if policy != "automatic" || status != "active" || sequence < 1 {
        return Ok(false);
    }
    let (end, period_status): (String, String) = conn
        .query_row(
            "SELECT period_end, status FROM market_platform_periods
         WHERE contract_id = ?1 AND sequence = ?2",
            params![contract_id, sequence],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read last monthly period"))?;
    let start = parse_time(&end)?;
    if period_status != "closed" || start > at {
        return Ok(false);
    }
    let anchor_day = parse_time(&anchor)?.day();
    let next_end = crate::market_recurring::next_calendar_month_at(start, anchor_day)?;
    let next_sequence = sequence + 1;
    let next_id = Uuid::new_v4().to_string();
    let amount = units_from_minor(price)?;
    let debit = match debit_available_tx(
        conn,
        &buyer,
        amount,
        "unfulfilled",
        &next_id,
        "platform_period",
        &next_id,
        "renew",
        now,
    ) {
        Ok(journal) => journal,
        Err(AppError::Conflict(_)) => {
            conn.execute(
                "UPDATE market_platform_contracts SET status = 'suspended', updated_at = ?2
                 WHERE id = ?1 AND status = 'active'",
                params![contract_id, now],
            )
            .map_err(db_error("suspend unfunded monthly renewal"))?;
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    conn.execute(
        "INSERT INTO market_platform_periods (
            id, contract_id, sequence, period_start, period_end,
            settled_through_at, amount_units, debit_journal_id, debited_at,
            status, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?4, ?6, ?7, ?8, 'open', ?8, ?8)",
        params![
            next_id,
            contract_id,
            next_sequence,
            end,
            next_end.to_rfc3339(),
            amount,
            debit.id,
            now
        ],
    )
    .map_err(db_error("create renewed monthly period"))?;
    conn.execute(
        "UPDATE market_platform_contracts SET current_period_sequence = ?2,
                updated_at = ?3 WHERE id = ?1 AND current_period_sequence = ?4",
        params![contract_id, next_sequence, now, sequence],
    )
    .map_err(db_error("advance monthly contract"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TransactionBehavior;
    use crate::market_finance::{provider_summary_tx, unlock_due_earnings_tx};
    use crate::market_wallet::{credit_verified_topup_tx, reconcile_wallet_tx, wallet_snapshot_tx};

    fn memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        conn
    }

    fn monthly<'a>(quote_id: &'a str, auto_renew: bool) -> PrepareContract<'a> {
        PrepareContract {
            product_kind: "share",
            product_ref: "subscription-1",
            trial_scope_ref: "share-1",
            quote_id,
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: BillingModel::CalendarMonth,
            price_minor: 1_000,
            auto_renew,
            trial_seconds: 0,
        }
    }

    #[test]
    fn one_client_quote_can_reserve_multiple_hosts_without_replaying_the_first_contract() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let now = "2026-01-01T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 200, "receipt", now).unwrap();
        let first = PrepareContract {
            product_kind: "client_host",
            product_ref: "job-1",
            trial_scope_ref: "host-1",
            quote_id: "batch-quote",
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: BillingModel::Metered,
            price_minor: 100,
            auto_renew: false,
            trial_seconds: 0,
        };
        let second = PrepareContract {
            product_ref: "job-2",
            trial_scope_ref: "host-2",
            ..first
        };
        let first_id = prepare_contract_tx(&tx, &first, now).unwrap();
        let second_id = prepare_contract_tx(&tx, &second, now).unwrap();
        assert_ne!(first_id, second_id);
        assert_eq!(prepare_contract_tx(&tx, &first, now).unwrap(), first_id);
        assert_eq!(prepare_contract_tx(&tx, &second, now).unwrap(), second_id);
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().held_units,
            200 * 86_400
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
    }

    #[test]
    fn trial_does_not_hold_money_and_suspends_if_conversion_is_unfunded() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-01-01T00:00:00+00:00";
        let gap = "2026-01-01T00:00:05+00:00";
        let end = "2026-01-01T00:00:15+00:00";
        let mut input = monthly("trial-quote", false);
        input.trial_seconds = 10;
        let id = prepare_contract_tx(&tx, &input, start).unwrap();
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().held_units, 0);
        activate_contract_tx(&tx, &id, start, start).unwrap();
        let outage = advance_trial_interval_tx(
            &tx,
            &id,
            "share_control",
            "unhealthy-1",
            start,
            gap,
            false,
            gap,
        )
        .unwrap();
        assert_eq!(outage.consumed_seconds, 0);
        assert!(!outage.needs_pause);
        let result =
            advance_trial_interval_tx(&tx, &id, "share_control", "healthy-1", gap, end, true, end)
                .unwrap();
        assert_eq!(result.consumed_seconds, 10);
        assert!(result.needs_pause);
        assert!(!result.paid_started);
        assert_eq!(
            trial_seconds_remaining_tx(&tx, "buyer", "provider", "share", "share-1", 10).unwrap(),
            0
        );
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            0
        );
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units, 0);
        let status: String = tx
            .query_row(
                "SELECT status FROM market_platform_contracts WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "suspended");
        assert!(
            terminate_after_cleanup_tx(&tx, &id, true, end)
                .unwrap()
                .finalized
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn paid_month_starts_at_observed_trial_end_not_grant_time() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-01-31T23:59:50+00:00";
        let end = "2026-02-01T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 1_000, "receipt", start).unwrap();
        let mut input = monthly("trial-quote", false);
        input.trial_seconds = 10;
        let id = prepare_contract_tx(&tx, &input, start).unwrap();
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().available_units,
            86_400_000
        );
        activate_contract_tx(&tx, &id, start, start).unwrap();
        let result = advance_trial_interval_tx(
            &tx,
            &id,
            "share_control",
            "healthy-1",
            start,
            end,
            true,
            end,
        )
        .unwrap();
        assert!(result.paid_started);
        assert!(!result.needs_pause);
        assert!(
            advance_trial_interval_tx(
                &tx,
                &id,
                "share_control",
                "healthy-1",
                start,
                end,
                true,
                end,
            )
            .unwrap()
            .replayed
        );
        let (paid_start, period_end): (String, String) = tx
            .query_row(
                "SELECT period_start, period_end FROM market_platform_periods WHERE contract_id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(parse_time(&paid_start).unwrap(), parse_time(end).unwrap());
        assert_eq!(
            parse_time(&period_end).unwrap(),
            parse_time("2026-03-01T00:00:00+00:00").unwrap()
        );
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units, 0);
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn monthly_capture_delivery_refund_and_replay_conserve_all_units() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-01-01T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 1_000, "receipt", start).unwrap();
        let id = prepare_contract_tx(&tx, &monthly("quote-1", false), start).unwrap();
        assert_eq!(
            prepare_contract_tx(&tx, &monthly("quote-1", false), start).unwrap(),
            id
        );
        assert!(prepare_contract_tx(&tx, &monthly("quote-1", true), start).is_err());
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().held_units,
            86_400_000
        );
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            0
        );
        activate_contract_tx(&tx, &id, start, start).unwrap();
        activate_contract_tx(&tx, &id, start, start).unwrap();
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units, 0);
        let mid = "2026-01-16T12:00:00+00:00";
        let served =
            settle_monthly_segment_tx(&tx, &id, "control-1", start, mid, true, mid).unwrap();
        assert_eq!(served.amount_units, 43_200_000);
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            43_200_000
        );
        assert!(
            settle_monthly_segment_tx(&tx, &id, "control-1", start, mid, true, mid,)
                .unwrap()
                .replayed
        );
        assert!(settle_monthly_segment_tx(&tx, &id, "control-1", start, mid, false, mid,).is_err());
        let end = "2026-02-01T00:00:00+00:00";
        let refunded =
            settle_monthly_segment_tx(&tx, &id, "outage-1", mid, end, false, end).unwrap();
        assert_eq!(refunded.amount_units, 43_200_000);
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            43_200_000
        );
        assert_eq!(
            unlock_due_earnings_tx(&tx, "2026-01-23T11:59:59+00:00", 10).unwrap(),
            0
        );
        assert_eq!(
            unlock_due_earnings_tx(&tx, "2026-01-23T12:00:00+00:00", 10).unwrap(),
            1
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn failed_grant_keeps_hold_until_cleanup_is_confirmed() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let now = "2026-01-01T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 1_000, "receipt", now).unwrap();
        let id = prepare_contract_tx(&tx, &monthly("quote-1", false), now).unwrap();
        fail_activation_tx(&tx, &id, false, now).unwrap();
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().available_units, 0);
        fail_activation_tx(&tx, &id, true, now).unwrap();
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().available_units,
            86_400_000
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn uncertain_cleanup_freezes_prior_earnings_before_seven_day_unlock() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-01-01T00:00:00+00:00";
        let observed = "2026-01-02T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 1_000, "receipt", start).unwrap();
        let id = prepare_contract_tx(&tx, &monthly("cleanup-quote", false), start).unwrap();
        activate_contract_tx(&tx, &id, start, start).unwrap();
        settle_monthly_segment_tx(&tx, &id, "healthy-1", start, observed, true, observed).unwrap();
        let isolated = terminate_after_cleanup_tx(&tx, &id, false, observed).unwrap();
        assert!(!isolated.finalized);
        let disputed: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM market_provider_earning_lots lot
                 JOIN market_delivery_slices slice ON slice.id = lot.delivery_slice_id
                 WHERE slice.contract_id = ?1 AND lot.disputed = 1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(disputed, 1);
        assert_eq!(
            unlock_due_earnings_tx(&tx, "2026-01-10T00:00:00+00:00", 10).unwrap(),
            0
        );
        assert_eq!(
            provider_summary_tx(&tx, "provider")
                .unwrap()
                .withdrawable_units,
            0
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
    }

    #[test]
    fn automatic_renewal_charges_only_when_next_period_begins() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-01-31T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 2_000, "receipt", start).unwrap();
        let id = prepare_contract_tx(&tx, &monthly("quote-1", true), start).unwrap();
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().held_units,
            86_400_000
        );
        activate_contract_tx(&tx, &id, start, start).unwrap();
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().available_units,
            86_400_000
        );
        let end = "2026-02-28T00:00:00+00:00";
        settle_monthly_segment_tx(&tx, &id, "control-1", start, end, true, end).unwrap();
        assert!(renew_monthly_contract_tx(&tx, &id, end).unwrap());
        let next_end: String = tx.query_row(
            "SELECT period_end FROM market_platform_periods WHERE contract_id = ?1 AND sequence = 2",
            params![id], |row| row.get(0),
        ).unwrap();
        assert_eq!(
            parse_time(&next_end).unwrap(),
            parse_time("2026-03-31T00:00:00+00:00").unwrap()
        );
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units, 0);
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn metered_intervals_are_immutable_and_posted_in_bounded_wallet_batches() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-01-01T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 200, "receipt", start).unwrap();
        let contract = prepare_contract_tx(
            &tx,
            &PrepareContract {
                product_kind: "share",
                product_ref: "metered-subscription",
                trial_scope_ref: "share-1",
                quote_id: "metered-quote",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::Metered,
                price_minor: 100,
                auto_renew: false,
                trial_seconds: 0,
            },
            start,
        )
        .unwrap();
        activate_contract_tx(&tx, &contract, start, start).unwrap();
        let minute = "2026-01-01T00:01:10+00:00";
        let first = observe_metered_interval_tx(
            &tx,
            &contract,
            "share_control",
            "control-1",
            start,
            minute,
            minute,
        )
        .unwrap();
        assert_eq!(
            observe_metered_interval_tx(
                &tx,
                &contract,
                "share_control",
                "control-1",
                start,
                minute,
                minute,
            )
            .unwrap(),
            first
        );
        assert!(
            observe_metered_interval_tx(
                &tx,
                &contract,
                "share_control",
                "overlap",
                start,
                minute,
                minute,
            )
            .is_err()
        );
        assert!(
            observe_metered_interval_tx(
                &tx,
                &contract,
                "client_host_health",
                "wrong-rail",
                minute,
                "2026-01-01T00:01:20+00:00",
                "2026-01-01T00:01:20+00:00",
            )
            .is_err()
        );
        let first_batch = settle_metered_batch_tx(&tx, &contract, minute).unwrap();
        assert_eq!(first_batch.captured_units, 6_000);
        assert_eq!(first_batch.pending_observations, 1);
        let second_batch = settle_metered_batch_tx(&tx, &contract, minute).unwrap();
        assert_eq!(second_batch.captured_units, 1_000);
        assert_eq!(second_batch.pending_observations, 0);
        assert_eq!(
            settle_metered_batch_tx(&tx, &contract, minute)
                .unwrap()
                .captured_units,
            0
        );
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            7_000
        );
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            17_273_000
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn confirmed_monthly_cleanup_refunds_only_unfulfilled_consideration() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-01-01T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 1_000, "receipt", start).unwrap();
        let id = prepare_contract_tx(&tx, &monthly("quote-1", false), start).unwrap();
        activate_contract_tx(&tx, &id, start, start).unwrap();
        let midpoint = "2026-01-16T12:00:00+00:00";
        settle_monthly_segment_tx(&tx, &id, "control-1", start, midpoint, true, midpoint).unwrap();
        let isolated = terminate_after_cleanup_tx(&tx, &id, false, midpoint).unwrap();
        assert!(!isolated.cleanup_confirmed);
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units, 0);
        let finished = terminate_after_cleanup_tx(&tx, &id, true, midpoint).unwrap();
        assert_eq!(finished.refunded_units, 43_200_000);
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units,
            43_200_000
        );
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            43_200_000
        );
        assert_eq!(
            terminate_after_cleanup_tx(&tx, &id, true, midpoint)
                .unwrap()
                .refunded_units,
            0
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn metered_cleanup_settles_funded_time_before_releasing_unused_budget() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-01-01T00:00:00+00:00";
        let end = "2026-01-01T00:02:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 100, "receipt", start).unwrap();
        let id = prepare_contract_tx(
            &tx,
            &PrepareContract {
                product_kind: "share",
                product_ref: "metered-subscription",
                trial_scope_ref: "share-1",
                quote_id: "metered-quote",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::Metered,
                price_minor: 100,
                auto_renew: false,
                trial_seconds: 0,
            },
            start,
        )
        .unwrap();
        activate_contract_tx(&tx, &id, start, start).unwrap();
        observe_metered_interval_tx(&tx, &id, "share_control", "control-1", start, end, end)
            .unwrap();
        assert_eq!(
            settle_metered_batch_tx(&tx, &id, end)
                .unwrap()
                .captured_units,
            6_000
        );
        assert!(
            !terminate_after_cleanup_tx(&tx, &id, false, end)
                .unwrap()
                .cleanup_confirmed
        );
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().available_units, 0);
        let finished = terminate_after_cleanup_tx(&tx, &id, true, end).unwrap();
        assert!(finished.finalized);
        assert_eq!(finished.unfunded_observations, 0);
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().available_units,
            8_628_000
        );
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            12_000
        );
        assert_eq!(pending_metered_count_tx(&tx, &id).unwrap(), 0);
        let cases: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases
             WHERE object_id = ?1 AND code = 'metered_unfunded_delivery'",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cases, 0);
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn confirmed_cleanup_keeps_budget_until_all_bounded_batches_are_posted() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let start = "2026-01-01T00:00:00+00:00";
        let first_end = "2026-01-01T01:00:00+00:00";
        let end = "2026-01-01T01:01:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 100, "receipt", start).unwrap();
        let id = prepare_contract_tx(
            &tx,
            &PrepareContract {
                product_kind: "share",
                product_ref: "long-metered-subscription",
                trial_scope_ref: "share-1",
                quote_id: "metered-quote",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::Metered,
                price_minor: 100,
                auto_renew: false,
                trial_seconds: 0,
            },
            start,
        )
        .unwrap();
        activate_contract_tx(&tx, &id, start, start).unwrap();
        observe_metered_interval_tx(&tx, &id, "share_control", "hour", start, first_end, end)
            .unwrap();
        observe_metered_interval_tx(&tx, &id, "share_control", "minute", first_end, end, end)
            .unwrap();
        let first = terminate_after_cleanup_tx(&tx, &id, true, end).unwrap();
        assert!(first.cleanup_confirmed);
        assert!(!first.finalized);
        assert_eq!(first.unfunded_observations, 0);
        assert_eq!(pending_metered_count_tx(&tx, &id).unwrap(), 1);
        assert_eq!(
            wallet_snapshot_tx(&tx, "buyer").unwrap().held_units,
            8_280_000
        );
        let second = terminate_after_cleanup_tx(&tx, &id, true, end).unwrap();
        assert!(second.finalized);
        assert_eq!(second.unfunded_observations, 0);
        assert_eq!(pending_metered_count_tx(&tx, &id).unwrap(), 0);
        assert_eq!(
            provider_summary_tx(&tx, "provider").unwrap().pending_units,
            366_000
        );
        reconcile_wallet_tx(&tx, "buyer").unwrap();
        tx.commit().unwrap();
    }
}
