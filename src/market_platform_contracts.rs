//! Wallet-funded contracts. These records intentionally do not reuse the
//! Provider-prepaid contract tables: the two rails must never settle one
//! service interval twice. Callers supply observed control-plane evidence and
//! run every transition inside an IMMEDIATE database transaction.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, header};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ServerState;
use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use crate::error::AppError;
use crate::market_wallet::{
    capture_hold_tx, credit_wallet_from_liability_tx, debit_available_tx, release_hold_tx,
    reserve_wallet_tx, units_from_minor,
};
use crate::models::AuthSession;

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AppError::BadRequest("invalid market service timestamp".into()))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WalletRenewalView {
    id: String,
    product_kind: String,
    billing_model: String,
    cycle_price_minor: i64,
    renewal_policy: String,
    status: String,
    period_end: Option<String>,
    trial_seconds_remaining: i64,
}

/// Buyer-only lifecycle projection shared by rental details and wallet controls.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletLifecycleView {
    pub id: String,
    pub status: String,
    pub trial_seconds_remaining: i64,
    pub renewal_policy: String,
    pub period_end: Option<String>,
}

pub(crate) fn wallet_lifecycle_view_tx(
    conn: &Connection,
    id: &str,
    buyer: &str,
) -> Result<Option<WalletLifecycleView>, AppError> {
    conn.query_row(
        "SELECT status, trial_seconds_remaining, renewal_policy,
                (SELECT period_end FROM market_platform_periods
                 WHERE contract_id = contract.id AND sequence = contract.current_period_sequence)
         FROM market_platform_contracts contract WHERE id = ?1 AND buyer_user_id = ?2",
        params![id, buyer],
        |row| {
            Ok(WalletLifecycleView {
                id: id.to_string(),
                status: row.get(0)?,
                trial_seconds_remaining: row.get(1)?,
                renewal_policy: row.get(2)?,
                period_end: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(db_error("read wallet service lifecycle"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateWalletRenewalRequest {
    auto_renew: bool,
    expected_policy: String,
}

fn wallet_renewal_view_tx(
    conn: &Connection,
    contract_id: &str,
    buyer: &str,
) -> Result<WalletRenewalView, AppError> {
    conn.query_row(
        "SELECT id, product_kind, billing_model, cycle_price_minor, renewal_policy, status,
                (SELECT period_end FROM market_platform_periods
                 WHERE contract_id = contract.id AND sequence = contract.current_period_sequence),
                trial_seconds_remaining
         FROM market_platform_contracts contract WHERE id = ?1 AND buyer_user_id = ?2
           AND billing_model = 'calendar_month'",
        params![contract_id, buyer],
        |row| {
            Ok(WalletRenewalView {
                id: row.get(0)?,
                product_kind: row.get(1)?,
                billing_model: row.get(2)?,
                cycle_price_minor: row.get(3)?,
                renewal_policy: row.get(4)?,
                status: row.get(5)?,
                period_end: row.get(6)?,
                trial_seconds_remaining: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(db_error("read wallet renewal policy"))?
    .ok_or_else(|| AppError::NotFound("monthly wallet contract not found".into()))
}

fn set_wallet_renewal_tx(
    conn: &Connection,
    contract_id: &str,
    buyer: &str,
    request: &UpdateWalletRenewalRequest,
    at: &str,
) -> Result<WalletRenewalView, AppError> {
    if !matches!(request.expected_policy.as_str(), "manual" | "automatic") {
        return Err(AppError::BadRequest(
            "invalid expected renewal policy".into(),
        ));
    }
    let current = wallet_renewal_view_tx(conn, contract_id, buyer)?;
    if current.renewal_policy != request.expected_policy {
        return Err(AppError::Conflict(
            "renewal policy changed; reload before saving".into(),
        ));
    }
    // A suspended buyer may still turn automatic renewal off; turning it on
    // waits until the service is running again.
    if !(matches!(current.status.as_str(), "trialing" | "active")
        || (current.status == "suspended" && !request.auto_renew))
    {
        return Err(AppError::Conflict(
            "this contract cannot change renewal policy".into(),
        ));
    }
    if request.auto_renew {
        if let Some(end) = current.period_end.as_deref() {
            if parse_time(end)? <= parse_time(at)? {
                return Err(AppError::Conflict(
                    "the current month has ended; renew from a fresh offer".into(),
                ));
            }
        }
    }
    let policy = if request.auto_renew {
        "automatic"
    } else {
        "manual"
    };
    if policy != current.renewal_policy {
        let changed = conn
            .execute(
                "UPDATE market_platform_contracts SET renewal_policy = ?3, updated_at = ?4
             WHERE id = ?1 AND buyer_user_id = ?2 AND renewal_policy = ?5
               AND (status IN ('trialing', 'active') OR (status = 'suspended' AND ?3 = 'manual'))",
                params![contract_id, buyer, policy, at, request.expected_policy],
            )
            .map_err(db_error("update wallet renewal policy"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "renewal policy changed; reload before saving".into(),
            ));
        }
    }
    wallet_renewal_view_tx(conn, contract_id, buyer)
}

async fn require_buyer(state: &ServerState, headers: &HeaderMap) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}

async fn get_wallet_renewal(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let buyer = require_buyer(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(wallet_renewal_view_tx(&conn, &id, &buyer.user_id)?),
    ))
}

async fn update_wallet_renewal(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<UpdateWalletRenewalRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let buyer = require_buyer(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin wallet renewal update"))?;
    let view = set_wallet_renewal_tx(&tx, &id, &buyer.user_id, &request, &Utc::now().to_rfc3339())?;
    tx.commit()
        .map_err(db_error("commit wallet renewal update"))?;
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(view)))
}

pub fn router() -> Router<ServerState> {
    Router::new().route(
        "/v1/market-wallet/contracts/:id/renewal",
        get(get_wallet_renewal).put(update_wallet_renewal),
    )
}

/// Buyer cancellation of a paid month stops renewal, keeping already-paid access
/// until its boundary. Trials and suspended services go through immediate cleanup.
pub(crate) fn cancel_at_period_end_tx(
    conn: &Connection,
    contract_id: &str,
    buyer: &str,
    now: &str,
) -> Result<bool, AppError> {
    let model: Option<String> = conn.query_row(
        "SELECT billing_model FROM market_platform_contracts WHERE id = ?1 AND buyer_user_id = ?2",
        params![contract_id, buyer], |row| row.get(0),
    ).optional().map_err(db_error("read cancellation billing model"))?;
    let Some(model) = model else {
        return Err(AppError::NotFound("wallet contract not found".into()));
    };
    if model != "calendar_month" {
        return Ok(false);
    }
    let current = wallet_renewal_view_tx(conn, contract_id, buyer)?;
    if current.billing_model != "calendar_month" || current.status != "active" {
        return Ok(false);
    }
    set_wallet_renewal_tx(
        conn,
        contract_id,
        buyer,
        &UpdateWalletRenewalRequest {
            expected_policy: current.renewal_policy,
            auto_renew: false,
        },
        now,
    )?;
    Ok(true)
}

pub(crate) fn service_can_resume_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
) -> Result<bool, AppError> {
    let Some(id) = contract_for_product_tx(conn, product_kind, product_ref)? else {
        // Only an actual free service may run without a wallet contract.
        // An archived paid record must never regain access via this branch.
        let query = match product_kind {
            "share" => {
                "SELECT daily_rate_minor IS NULL AND cycle_price_minor IS NULL FROM share_market_subscriptions WHERE id = ?1"
            }
            "client_host" => {
                "SELECT daily_rate_minor IS NULL AND cycle_price_minor IS NULL FROM client_market_subscriptions WHERE installation_id = ?1"
            }
            _ => return Ok(false),
        };
        return conn
            .query_row(query, params![product_ref], |row| row.get(0))
            .optional()
            .map(|free| free.unwrap_or(false))
            .map_err(db_error("check free service without wallet contract"));
    };
    conn.query_row(
        "SELECT status IN ('trialing', 'active') FROM market_platform_contracts WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )
    .map_err(db_error("check wallet service resume"))
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

#[derive(Debug)]
struct DueFundingClaim {
    contract_id: String,
    due_at: DateTime<Utc>,
    renewal_priority: i64,
    amount_units: i64,
}

/// Holds are already excluded from wallet available balance. These are the
/// *unreserved* obligations of existing services which must precede a new
/// order or a later-due service when one buyer shares a wallet. Enumerating
/// all of one buyer's claims inside the same IMMEDIATE transaction avoids a
/// worker LIMIT or HashMap iteration from deciding who gets the last funds.
fn due_funding_claims_tx(
    conn: &Connection,
    buyer: &str,
    at: DateTime<Utc>,
) -> Result<Vec<DueFundingClaim>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT c.id, c.billing_model, c.status, c.renewal_policy,
                    c.daily_rate_minor, c.cycle_price_minor,
                    c.trial_seconds_remaining, c.current_period_sequence,
                    c.last_evaluated_at, c.updated_at, c.renewal_priority,
                    c.paid_started_at, COALESCE(h.remaining_units, 0),
                    (SELECT p.period_end FROM market_platform_periods p
                     WHERE p.contract_id = c.id AND p.sequence = c.current_period_sequence)
             FROM market_platform_contracts c
             LEFT JOIN market_wallet_holds h ON h.id = c.budget_hold_id
             WHERE c.buyer_user_id = ?1
               AND c.status IN ('trialing', 'active', 'suspended', 'ending')",
        )
        .map_err(db_error("prepare buyer wallet funding claims"))?;
    let rows = statement
        .query_map(params![buyer], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, i64>(12)?,
                row.get::<_, Option<String>>(13)?,
            ))
        })
        .map_err(db_error("query buyer wallet funding claims"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read buyer wallet funding claims"))?;
    let mut claims = Vec::new();
    for (
        id,
        model,
        status,
        policy,
        daily,
        monthly,
        trial_remaining,
        sequence,
        last_evaluated,
        updated,
        priority,
        paid_started,
        held,
        period_end,
    ) in rows
    {
        let trial_conversion = paid_started.is_none()
            && trial_remaining == 0
            && matches!(status.as_str(), "trialing" | "suspended");
        let monthly_due = period_end
            .as_deref()
            .map(parse_time)
            .transpose()?
            .is_some_and(|date| date <= at);
        let (due, amount_minor) = if trial_conversion {
            (
                last_evaluated.as_deref().unwrap_or(&updated),
                monthly.or(daily),
            )
        } else if model == "calendar_month"
            && policy == "automatic"
            && sequence > 0
            && matches!(status.as_str(), "active" | "suspended")
            && monthly_due
        {
            (period_end.as_deref().unwrap(), monthly)
        } else if model == "metered"
            && matches!(status.as_str(), "active" | "suspended" | "ending")
            // 1 cent = 86,400 units and one day = 86,400 seconds, so a
            // cents/day rate is also the units needed for one billable second.
            && daily.is_some_and(|rate| held < rate)
            && (status != "ending" || pending_metered_count_tx(conn, &id)? > 0)
        {
            (last_evaluated.as_deref().unwrap_or(&updated), daily)
        } else {
            continue;
        };
        let amount = units_from_minor(
            amount_minor
                .ok_or_else(|| AppError::Internal("wallet funding claim has no price".into()))?,
        )?;
        claims.push(DueFundingClaim {
            contract_id: id,
            due_at: parse_time(due)?,
            renewal_priority: priority,
            amount_units: amount,
        });
    }
    claims.sort_by(|a, b| {
        a.due_at
            .cmp(&b.due_at)
            .then_with(|| b.renewal_priority.cmp(&a.renewal_priority))
            .then_with(|| a.contract_id.cmp(&b.contract_id))
    });
    Ok(claims)
}

/// For a new order, `candidate` is None and all existing due claims take
/// precedence. For a renewal/replenishment, only earlier claims do.
pub(crate) fn prior_due_funding_units_tx(
    conn: &Connection,
    buyer: &str,
    candidate: Option<&str>,
    at: &str,
) -> Result<i64, AppError> {
    let claims = due_funding_claims_tx(conn, buyer, parse_time(at)?)?;
    let mut prior = 0_i64;
    for claim in claims {
        if Some(claim.contract_id.as_str()) == candidate {
            return Ok(prior);
        }
        prior = prior
            .checked_add(claim.amount_units)
            .ok_or_else(|| AppError::Internal("wallet funding commitments overflow".into()))?;
    }
    if candidate.is_some() {
        return Err(AppError::Conflict("wallet funding claim is not due".into()));
    }
    Ok(prior)
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
    crate::market_payments::require_new_wallet_orders_open_tx(conn)?;
    crate::market_reconcile::ensure_finance_scopes_safe_tx(
        conn,
        &[
            crate::market_reconcile::FinanceScope::User(&input.buyer_user_id),
            crate::market_reconcile::FinanceScope::Provider(&input.provider_user_id),
        ],
    )?;
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
    if input.trial_seconds == 0 {
        let prior = prior_due_funding_units_tx(conn, input.buyer_user_id, None, now)?;
        let needed = prior
            .checked_add(amount_units)
            .ok_or_else(|| AppError::Internal("wallet funding commitments overflow".into()))?;
        if crate::market_wallet::wallet_snapshot_tx(conn, input.buyer_user_id)?.available_units
            < needed
        {
            return Err(AppError::coded_conflict(
                "MARKET_WALLET_REQUIRED",
                "wallet balance changed; refresh the quote",
                serde_json::to_value(crate::market_wallet::wallet_quote_funding_tx(
                    conn,
                    input.buyer_user_id,
                    input.price_minor,
                    0,
                    now,
                )?)
                .map_err(|error| AppError::Internal(error.to_string()))?,
            ));
        }
    }
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
    crate::market_commission::freeze_contract_tx(
        conn,
        &id,
        input.quote_id,
        input.provider_user_id,
        now,
    )?;
    if input.billing_model == BillingModel::Metered
        && let Some(hold) = hold_id.as_deref()
    {
        crate::market_commission::freeze_period_tx(
            conn,
            &id,
            "metered_budget",
            hold,
            true,
            now,
            now,
        )?;
    }
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
                     last_evaluated_at = ?2, health_fence_at = ?2, updated_at = ?3
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
        let end = crate::market_common::next_calendar_month_at(start, start.day())?;
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
        crate::market_commission::freeze_period_tx(
            conn,
            contract_id,
            "monthly_period",
            &period_id,
            true,
            activated_at,
            now,
        )?;
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
                 last_evaluated_at = ?2, health_fence_at = ?2,
                 current_period_sequence = ?3,
                 renewal_anchor_day = CASE WHEN ?3 = 1 THEN ?5 ELSE 0 END,
                 updated_at = ?4 WHERE id = ?1 AND status = 'pending_activation'",
            params![contract_id, activated_at, sequence, now, start.day()],
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
    crate::market_reconcile::ensure_contract_finance_safe_tx(conn, contract_id)?;
    let units = units_from_minor(price)?;
    let prior = prior_due_funding_units_tx(conn, &buyer, Some(contract_id), now)?;
    if crate::market_wallet::wallet_snapshot_tx(conn, &buyer)?.available_units
        < prior
            .checked_add(units)
            .ok_or_else(|| AppError::Internal("wallet funding commitments overflow".into()))?
    {
        if conn
            .execute(
                "UPDATE market_platform_contracts SET status = 'suspended', updated_at = ?2
             WHERE id = ?1 AND status = 'trialing' AND trial_seconds_remaining = 0",
                params![contract_id, now],
            )
            .map_err(db_error("suspend unfunded trial conversion"))?
            == 1
        {
            notify_contract_tx(
                conn,
                contract_id,
                "contract_suspended",
                "insufficient_funds",
                now,
            )?;
        }
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
        let end = crate::market_common::next_calendar_month_at(start, start.day())?;
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
        crate::market_commission::freeze_period_tx(
            conn,
            contract_id,
            "monthly_period",
            &period_id,
            true,
            at,
            now,
        )?;
        let changed = conn
            .execute(
                "UPDATE market_platform_contracts
             SET status = 'active', activation_hold_id = ?2, paid_started_at = ?3,
                 current_period_sequence = 1, renewal_anchor_day = ?5, updated_at = ?4
             WHERE id = ?1 AND status = 'trialing' AND trial_seconds_remaining = 0",
                params![contract_id, hold_id, at, now, start.day()],
            )
            .map_err(db_error("activate paid monthly service after trial"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "monthly trial conversion changed".into(),
            ));
        }
    } else if model == "metered" {
        crate::market_commission::freeze_period_tx(
            conn,
            contract_id,
            "metered_budget",
            &hold_id,
            true,
            at,
            now,
        )?;
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
    let prior = prior_due_funding_units_tx(conn, buyer, Some(contract_id), now)?;
    let available = available.saturating_sub(prior);
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
         WHERE id = ?1 AND budget_sequence = ?5 AND status IN ('active', 'ending', 'suspended')",
            params![contract_id, hold_id, sequence, now, prior_sequence],
        )
        .map_err(db_error("advance metered budget"))?;
    if changed != 1 {
        return Err(AppError::Conflict("metered budget changed".into()));
    }
    // Backlogs use the earliest service actually funded by this budget, not
    // today's policy merely because the worker was delayed.
    let anchor: Option<String> = conn.query_row(
        "SELECT strftime('%Y-%m-%dT%H:%M:%S+00:00', started_at, '+' || captured_seconds || ' seconds')
         FROM market_platform_metered_observations WHERE contract_id = ?1
           AND captured_seconds + unfunded_seconds < billable_seconds ORDER BY julianday(started_at), id LIMIT 1",
        params![contract_id], |row| row.get(0)).optional().map_err(db_error("read budget service anchor"))?;
    crate::market_commission::freeze_period_tx(
        conn,
        contract_id,
        "metered_budget",
        &hold_id,
        false,
        anchor.as_deref().unwrap_or(now),
        now,
    )?;
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
    crate::market_reconcile::ensure_contract_finance_safe_tx(conn, contract_id)?;
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
            if conn
                .execute(
                    "UPDATE market_platform_contracts SET status = 'suspended', updated_at = ?2
                 WHERE id = ?1 AND status = 'active'",
                    params![contract_id, now],
                )
                .map_err(db_error("suspend unfunded metered service"))?
                == 1
            {
                notify_contract_tx(
                    conn,
                    contract_id,
                    "contract_suspended",
                    "insufficient_funds",
                    now,
                )?;
            }
            let pending = pending_metered_count_tx(conn, contract_id)?;
            // A later top-up must never debit service delivered without a
            // reserved budget while remote suspension was still in flight.
            conn.execute(
                "UPDATE market_platform_metered_observations
                 SET unfunded_seconds = billable_seconds - captured_seconds
                 WHERE contract_id = ?1
                   AND captured_seconds + unfunded_seconds < billable_seconds",
                params![contract_id],
            )
            .map_err(db_error("isolate unfunded metered time"))?;
            if pending > 0 {
                conn.execute(
                    "INSERT OR IGNORE INTO market_finance_reconciliation_cases
                        (id, code, object_kind, object_id, status, detail_code, created_at)
                     VALUES (?1, 'metered_unfunded_delivery', 'platform_contract', ?2,
                             'open', 'budget_exhausted_before_control_pause', ?3)",
                    params![Uuid::new_v4().to_string(), contract_id, now],
                )
                .map_err(db_error("record unfunded metered service"))?;
            }
            return Ok(MeteredBatchResult {
                captured_units: 0,
                pending_observations: 0,
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
    let was_ended: bool = conn
        .query_row(
            "SELECT status = 'ended' FROM market_platform_contracts WHERE id = ?1",
            params![contract_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read contract before termination"))?
        .unwrap_or(true);
    let result = terminate_after_cleanup_inner_tx(conn, contract_id, cleanup_confirmed, now)?;
    if result.finalized && !was_ended {
        notify_contract_tx(conn, contract_id, "contract_terminated", "ended", now)?;
    }
    Ok(result)
}

fn terminate_after_cleanup_inner_tx(
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
    if status == "ended" || status == "activation_failed" {
        return Ok(ContractTermination {
            refunded_units: 0,
            unfunded_observations: 0,
            cleanup_confirmed: true,
            finalized: true,
        });
    }
    if status == "pending_activation" || (status == "review_required" && paid_started_at.is_none())
    {
        fail_activation_tx(conn, contract_id, cleanup_confirmed, now)?;
        return Ok(ContractTermination {
            refunded_units: 0,
            unfunded_observations: 0,
            cleanup_confirmed,
            finalized: cleanup_confirmed,
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
        crate::market_finance::hold_contract_earnings_tx(
            conn,
            contract_id,
            "cleanup_uncertain",
            now,
        )?;
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
    if amount > 0 {
        crate::market_reconcile::ensure_contract_finance_safe_tx(conn, contract_id)?;
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
    let (buyer, policy, status, price, sequence, anchor, stored_anchor): (
        String,
        String,
        String,
        i64,
        i64,
        String,
        i64,
    ) = conn
        .query_row(
            "SELECT buyer_user_id, renewal_policy, status, cycle_price_minor,
                current_period_sequence, paid_started_at, renewal_anchor_day
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
                    row.get(6)?,
                ))
            },
        )
        .map_err(db_error("read monthly contract for renewal"))?;
    if policy != "automatic" || !matches!(status.as_str(), "active" | "suspended") || sequence < 1 {
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
    let previous_end = parse_time(&end)?;
    if period_status != "closed" || previous_end > at {
        return Ok(false);
    }
    // A stopped worker or funding gap is never retroactively sold as service.
    // Restart the paid calendar month when funds are actually captured.
    let start = if status == "active" && (at - previous_end).num_seconds() <= 20 {
        previous_end
    } else {
        at
    };
    let anchor_day = if start > previous_end {
        start.day()
    } else if stored_anchor > 0 {
        u32::try_from(stored_anchor)
            .map_err(|_| AppError::Internal("invalid renewal anchor".into()))?
    } else {
        parse_time(&anchor)?.day()
    };
    let next_end = crate::market_common::next_calendar_month_at(start, anchor_day)?;
    let next_sequence = sequence + 1;
    let next_id = Uuid::new_v4().to_string();
    let amount = units_from_minor(price)?;
    crate::market_reconcile::ensure_contract_finance_safe_tx(conn, contract_id)?;
    let prior = prior_due_funding_units_tx(conn, &buyer, Some(contract_id), now)?;
    let needed = prior
        .checked_add(amount)
        .ok_or_else(|| AppError::Internal("wallet funding commitments overflow".into()))?;
    if crate::market_wallet::wallet_snapshot_tx(conn, &buyer)?.available_units < needed {
        if conn
            .execute(
                "UPDATE market_platform_contracts SET status = 'suspended', updated_at = ?2
             WHERE id = ?1 AND status = 'active'",
                params![contract_id, now],
            )
            .map_err(db_error("suspend lower-priority wallet renewal"))?
            == 1
        {
            notify_contract_tx(
                conn,
                contract_id,
                "contract_suspended",
                "insufficient_funds",
                now,
            )?;
        }
        return Ok(false);
    }
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
            if conn
                .execute(
                    "UPDATE market_platform_contracts SET status = 'suspended', updated_at = ?2
                 WHERE id = ?1 AND status = 'active'",
                    params![contract_id, now],
                )
                .map_err(db_error("suspend unfunded monthly renewal"))?
                == 1
            {
                notify_contract_tx(
                    conn,
                    contract_id,
                    "contract_suspended",
                    "insufficient_funds",
                    now,
                )?;
            }
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
            start.to_rfc3339(),
            next_end.to_rfc3339(),
            amount,
            debit.id,
            now
        ],
    )
    .map_err(db_error("create renewed monthly period"))?;
    crate::market_commission::freeze_period_tx(
        conn,
        contract_id,
        "monthly_period",
        &next_id,
        false,
        &start.to_rfc3339(),
        now,
    )?;
    conn.execute(
        "UPDATE market_platform_contracts SET current_period_sequence = ?2,
                renewal_anchor_day = ?5, status = 'active',
                last_evaluated_at = ?6,
                health_fence_at = CASE WHEN ?7 = 1 THEN ?3 ELSE health_fence_at END,
                updated_at = ?3
         WHERE id = ?1 AND current_period_sequence = ?4
           AND status IN ('active', 'suspended')",
        params![
            contract_id,
            next_sequence,
            now,
            sequence,
            anchor_day,
            start.to_rfc3339(),
            i64::from(start > previous_end || status == "suspended"),
        ],
    )
    .map_err(db_error("advance monthly contract"))?;
    Ok(true)
}

/// A credited wallet top-up can restart an already suspended service, but
/// never pays for the unfunded gap. The external entitlement remains paused
/// until the caller successfully reapplies its control-plane grant.
/// Hourly reminders: automatic monthly renewals due within 72 hours that the
/// free balance cannot cover, and metered services with less than a day of
/// funded runway. Dedupe keys allow one reminder per window and period.
pub(crate) fn queue_funding_reminders_tx(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<usize, AppError> {
    let now_text = now.to_rfc3339();
    let mut queued = 0;
    let renewals = {
        let mut statement = conn
            .prepare(
                "SELECT c.id, c.buyer_user_id, c.cycle_price_minor, p.period_end
                 FROM market_platform_contracts c
                 JOIN market_platform_periods p
                   ON p.contract_id = c.id AND p.sequence = c.current_period_sequence
                 WHERE c.status = 'active' AND c.billing_model = 'calendar_month'
                   AND c.renewal_policy = 'automatic' AND c.cycle_price_minor IS NOT NULL
                   AND p.period_end > ?1 AND p.period_end <= ?2",
            )
            .map_err(db_error("prepare renewal reminders"))?;
        statement
            .query_map(
                params![now_text, (now + chrono::Duration::hours(72)).to_rfc3339()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(db_error("query renewal reminders"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read renewal reminders"))?
    };
    for (contract_id, buyer, price_minor, period_end) in renewals {
        let needed = units_from_minor(price_minor)?
            + prior_due_funding_units_tx(conn, &buyer, Some(&contract_id), &period_end)?;
        let available = crate::market_wallet::wallet_snapshot_tx(conn, &buyer)?.available_units;
        if available >= needed {
            continue;
        }
        let window = if parse_time(&period_end)? - now <= chrono::Duration::hours(24) {
            "24h"
        } else {
            "72h"
        };
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "buyer",
            &buyer,
            "renewal_upcoming_shortfall",
            "platform_contract",
            &contract_id,
            &format!("renewal_upcoming_shortfall:{contract_id}:{period_end}:{window}"),
            serde_json::json!({
                "contractId": contract_id,
                "renewsAt": period_end,
                "amountMinor": (needed - available.max(0)) / crate::market_common::MONEY_UNITS_PER_MINOR,
            }),
            &now_text,
        )?;
        queued += 1;
    }
    let metered = {
        let mut statement = conn
            .prepare(
                "SELECT c.id, c.buyer_user_id, c.daily_rate_minor,
                        COALESCE((SELECT remaining_units FROM market_wallet_holds h
                                  WHERE h.id = c.budget_hold_id AND h.status = 'active'), 0)
                 FROM market_platform_contracts c
                 WHERE c.status = 'active' AND c.billing_model = 'metered'
                   AND c.daily_rate_minor IS NOT NULL AND c.daily_rate_minor > 0",
            )
            .map_err(db_error("prepare runway reminders"))?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(db_error("query runway reminders"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read runway reminders"))?
    };
    for (contract_id, buyer, rate_minor, budget) in metered {
        let per_day = units_from_minor(rate_minor)?;
        let available = crate::market_wallet::wallet_snapshot_tx(conn, &buyer)?
            .available_units
            .max(0);
        if budget + available >= per_day {
            continue;
        }
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "buyer",
            &buyer,
            "wallet_low_runway",
            "platform_contract",
            &contract_id,
            &format!("wallet_low_runway:{contract_id}:{}", now.format("%Y-%m-%d")),
            serde_json::json!({
                "contractId": contract_id,
                "runwaySeconds": (budget + available) / rate_minor.max(1),
            }),
            &now_text,
        )?;
        queued += 1;
    }
    Ok(queued)
}

/// Revalidate a queued funding reminder against the current period and money.
/// A delayed email must not ask a user who already topped up to pay again.
pub(crate) fn funding_notice_current_tx(
    conn: &Connection,
    buyer: &str,
    id: &str,
    kind: &str,
    expected_end: Option<&str>,
    now: DateTime<Utc>,
) -> Result<bool, AppError> {
    let row = conn.query_row(
        "SELECT c.status,c.billing_model,c.renewal_policy,c.cycle_price_minor,c.daily_rate_minor,
            p.period_end,COALESCE(h.remaining_units,0)
         FROM market_platform_contracts c
         LEFT JOIN market_platform_periods p ON p.contract_id=c.id AND p.sequence=c.current_period_sequence
         LEFT JOIN market_wallet_holds h ON h.id=c.budget_hold_id AND h.status='active'
         WHERE c.id=?1 AND c.buyer_user_id=?2",
        params![id,buyer], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,
            r.get::<_,String>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,Option<i64>>(4)?,
            r.get::<_,Option<String>>(5)?,r.get::<_,i64>(6)?)))
        .optional().map_err(db_error("recheck funding reminder"))?;
    let Some((status, model, renewal, price, rate, end, budget)) = row else {
        return Ok(false);
    };
    if status != "active" {
        return Ok(false);
    }
    let available = crate::market_wallet::wallet_snapshot_tx(conn, buyer)?
        .available_units
        .max(0);
    if kind == "wallet_low_runway" && model == "metered" {
        let Some(rate) = rate.filter(|n| *n > 0) else {
            return Ok(false);
        };
        return Ok(i128::from(budget) + i128::from(available) < i128::from(units_from_minor(rate)?));
    }
    if kind != "renewal_upcoming_shortfall" || model != "calendar_month" || renewal != "automatic" {
        return Ok(false);
    }
    let (Some(price), Some(end)) = (price, end) else {
        return Ok(false);
    };
    if expected_end != Some(end.as_str()) {
        return Ok(false);
    }
    let at = parse_time(&end)?;
    if at <= now || at > now + chrono::Duration::hours(72) {
        return Ok(false);
    }
    let needed = units_from_minor(price)?
        .checked_add(prior_due_funding_units_tx(conn, buyer, Some(id), &end)?)
        .ok_or_else(|| AppError::Internal("funding reminder amount overflow".into()))?;
    Ok(available < needed)
}

/// Queue a buyer notice for a contract state change. `reason` is a short
/// machine code (for example `insufficient_funds`, `finance_hold`).
pub(crate) fn notify_contract_tx(
    conn: &Connection,
    contract_id: &str,
    kind: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    let (buyer, product_kind, status, updated_at): (String, String, String, String) = conn
        .query_row(
            "SELECT buyer_user_id, product_kind, status, updated_at
             FROM market_platform_contracts WHERE id = ?1",
            params![contract_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(db_error("read contract for notification"))?;
    crate::market_notifications::enqueue_user_notification_tx(
        conn,
        "buyer",
        &buyer,
        kind,
        "platform_contract",
        contract_id,
        &format!("{kind}:{contract_id}:{updated_at}"),
        serde_json::json!({
            "contractId": contract_id,
            "productKind": product_kind,
            "status": status,
            "reason": reason,
        }),
        now,
    )
}

pub(crate) fn resume_suspended_contract_tx(
    conn: &Connection,
    contract_id: &str,
    now: &str,
) -> Result<bool, AppError> {
    let resumed = resume_suspended_contract_inner_tx(conn, contract_id, now)?;
    if resumed {
        notify_contract_tx(conn, contract_id, "contract_resumed", "funded", now)?;
    }
    Ok(resumed)
}

fn resume_suspended_contract_inner_tx(
    conn: &Connection,
    contract_id: &str,
    now: &str,
) -> Result<bool, AppError> {
    parse_time(now)?;
    let (status, model, paid_started, trial_remaining, buyer, rate, hold_id, sequence): (
        String,
        String,
        Option<String>,
        i64,
        String,
        Option<i64>,
        Option<String>,
        i64,
    ) = conn
        .query_row(
            "SELECT status, billing_model, paid_started_at, trial_seconds_remaining,
                    buyer_user_id, daily_rate_minor, budget_hold_id, budget_sequence
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
                    row.get(7)?,
                ))
            },
        )
        .map_err(db_error("read suspended wallet contract"))?;
    if status != "suspended" {
        return Ok(false);
    }
    if crate::market_reconcile::ensure_contract_finance_safe_tx(conn, contract_id).is_err() {
        return Ok(false);
    }
    if paid_started.is_none() && trial_remaining == 0 {
        conn.execute(
            "UPDATE market_platform_contracts SET status = 'trialing',
                    last_evaluated_at = ?2, health_fence_at = ?2, updated_at = ?2
             WHERE id = ?1 AND status = 'suspended'",
            params![contract_id, now],
        )
        .map_err(db_error("retry wallet trial conversion"))?;
        return start_paid_trial_tx(conn, contract_id, now, now);
    }
    if model == "calendar_month" {
        return renew_monthly_contract_tx(conn, contract_id, now);
    }
    if model != "metered" {
        return Err(AppError::Internal(
            "stored wallet billing model is invalid".into(),
        ));
    }
    if let Some(old_hold_id) = hold_id {
        release_hold_tx(conn, &old_hold_id, now)?;
    }
    let rate = rate.ok_or_else(|| AppError::Internal("metered wallet rate is missing".into()))?;
    let Some(_) = replenish_metered_budget_tx(conn, contract_id, &buyer, rate, sequence, now)?
    else {
        return Ok(false);
    };
    let changed = conn
        .execute(
            "UPDATE market_platform_contracts
             SET status = 'active', last_evaluated_at = ?2,
                 health_fence_at = ?2, updated_at = ?2
             WHERE id = ?1 AND status = 'suspended'",
            params![contract_id, now],
        )
        .map_err(db_error("resume funded metered wallet contract"))?;
    if changed != 1 {
        return Err(AppError::Conflict("metered wallet resume changed".into()));
    }
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlatformControlKind {
    Suspend,
    Resume,
    Terminate,
}

#[derive(Debug, Clone)]
struct PlatformControlAction {
    contract_id: String,
    product_kind: String,
    product_ref: String,
    service_ref: String,
    kind: PlatformControlKind,
}

fn service_expiry_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
    service_ref: &str,
) -> Result<Option<DateTime<Utc>>, AppError> {
    let expiry: Option<String> = if product_kind == "share" {
        conn.query_row(
            "SELECT expires_at FROM share_market_subscriptions WHERE id = ?1",
            params![product_ref],
            |row| row.get(0),
        )
    } else {
        conn.query_row(
            "SELECT expires_at FROM client_market_subscriptions WHERE installation_id = ?1",
            params![service_ref],
            |row| row.get(0),
        )
    }
    .optional()
    .map_err(db_error("read wallet service expiry"))?
    .flatten();
    expiry.as_deref().map(parse_time).transpose()
}

fn record_service_evidence_tx(
    conn: &Connection,
    contract_id: &str,
    kind: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    state: &str,
    available: bool,
    reason: &str,
    now: &str,
) -> Result<String, AppError> {
    let id = format!("{contract_id}:{}:{}", start.timestamp(), end.timestamp());
    let (started_at, ended_at) = (start.to_rfc3339(), end.to_rfc3339());
    conn.execute(
        "INSERT OR IGNORE INTO market_platform_service_evidence
            (id, contract_id, source_kind, started_at, ended_at,
             observed_state, service_available, reason_code, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            contract_id,
            kind,
            started_at,
            ended_at,
            state,
            i64::from(available),
            reason,
            now
        ],
    )
    .map_err(db_error("persist wallet service evidence"))?;
    let saved: (String, String, String, i64) = conn
        .query_row(
            "SELECT started_at, ended_at, observed_state, service_available
             FROM market_platform_service_evidence WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(db_error("verify wallet service evidence"))?;
    if saved
        != (
            started_at,
            ended_at,
            state.to_string(),
            i64::from(available),
        )
    {
        return Err(AppError::Conflict("wallet service evidence changed".into()));
    }
    Ok(id)
}

const SUSPENSION_WARNING_DAYS: i64 = 5;

fn suspension_terminate_days() -> i64 {
    std::env::var("CC_SWITCH_ROUTER_MARKET_SUSPENDED_TERMINATE_DAYS")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|days| (1..=90).contains(days))
        .unwrap_or(7)
}

/// End a service the buyer has left unfunded for too long, after a warning.
/// A suspension caused by a finance hold is the Router's problem, not the
/// buyer's, and never times out. Returns whether the contract was marked for
/// cleanup.
fn enforce_suspension_timeout_tx(
    conn: &Connection,
    contract_id: &str,
    now: DateTime<Utc>,
) -> Result<bool, AppError> {
    let (suspended_at, cause, buyer): (Option<String>, Option<String>, String) = conn
        .query_row(
            "SELECT suspended_at, suspension_cause, buyer_user_id
             FROM market_platform_contracts WHERE id = ?1",
            params![contract_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db_error("read contract suspension"))?;
    let Some(suspended_at) = suspended_at else {
        return Ok(false);
    };
    if crate::market_reconcile::ensure_contract_finance_safe_tx(conn, contract_id).is_err() {
        return Ok(false);
    }
    if cause.as_deref() == Some("finance_hold") {
        // The hold has cleared but the service is still suspended, so it now
        // waits on funding. The buyer's window starts only from this moment.
        conn.execute(
            "UPDATE market_platform_contracts
             SET suspension_cause = NULL, suspended_at = ?2
             WHERE id = ?1 AND status = 'suspended'",
            params![contract_id, now.to_rfc3339()],
        )
        .map_err(db_error("restart contract suspension clock"))?;
        return Ok(false);
    }
    let age = now - parse_time(&suspended_at)?;
    let terminate_days = suspension_terminate_days();
    let now_text = now.to_rfc3339();
    if age >= chrono::Duration::days(terminate_days) {
        mark_for_confirmed_cleanup_tx(conn, contract_id, &now_text)?;
        return Ok(true);
    }
    if age >= chrono::Duration::days(SUSPENSION_WARNING_DAYS.min(terminate_days - 1).max(0)) {
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "buyer",
            &buyer,
            "contract_terminating_soon",
            "platform_contract",
            contract_id,
            &format!("contract_terminating_soon:{contract_id}:{suspended_at}"),
            serde_json::json!({
                "contractId": contract_id,
                "terminatesAt": (parse_time(&suspended_at)? + chrono::Duration::days(terminate_days)).to_rfc3339(),
            }),
            &now_text,
        )?;
    }
    Ok(false)
}

fn mark_for_confirmed_cleanup_tx(
    conn: &Connection,
    contract_id: &str,
    now: &str,
) -> Result<(), AppError> {
    // Normal expiry is not a dispute. The earned slices may mature, while
    // unfulfilled money stays held until the remote grant/Host is cleaned.
    // If cleanup becomes uncertain, the existing cleanup path explicitly
    // freezes the lots and opens a reconciliation case.
    conn.execute(
        "UPDATE market_platform_contracts
         SET status = 'ending', updated_at = ?2
         WHERE id = ?1 AND status IN ('active', 'trialing', 'suspended')",
        params![contract_id, now],
    )
    .map_err(db_error("end wallet service term"))?;
    Ok(())
}

fn service_control_available_tx(
    conn: &Connection,
    product_kind: &str,
    product_ref: &str,
    service_ref: &str,
) -> Result<bool, AppError> {
    if product_kind == "share" {
        let status = conn
            .query_row(
                "SELECT status FROM share_market_subscriptions WHERE id = ?1",
                params![product_ref],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read wallet Share grant state"))?;
        // Only an acknowledged grant is a service entitlement. In particular,
        // billing_resume_pending is not proof that a revoke has been undone.
        Ok(matches!(
            status.as_deref(),
            Some("active_free" | "active_postpaid" | "active_prepaid")
        ))
    } else if product_kind == "client_host" {
        let state = conn
            .query_row(
                "SELECT subscription.status, tunnel.enabled
                 FROM client_market_subscriptions subscription
                 JOIN installation_client_tunnels tunnel
                   ON tunnel.installation_id = subscription.installation_id
                 WHERE subscription.installation_id = ?1",
                params![service_ref],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()
            .map_err(db_error("read wallet Client access state"))?;
        Ok(matches!(state, Some((ref status, true)) if status == "active"))
    } else {
        Err(AppError::Internal("invalid wallet service kind".into()))
    }
}

fn health_observed_after_control_fence_tx(
    conn: &Connection,
    contract_id: &str,
    product_kind: &str,
    product_ref: &str,
    service_ref: &str,
    through: DateTime<Utc>,
) -> Result<bool, AppError> {
    let fence: Option<String> = conn
        .query_row(
            "SELECT COALESCE(health_fence_at, activated_at, paid_started_at)
             FROM market_platform_contracts WHERE id = ?1",
            params![contract_id],
            |row| row.get(0),
        )
        .map_err(db_error("read wallet health evidence fence"))?;
    let Some(fence) = fence else { return Ok(false) };
    let checked_at: Option<i64> = if product_kind == "share" {
        conn.query_row(
            "SELECT MAX(health.checked_at) FROM share_health_checks health
             JOIN share_market_subscriptions subscription
               ON subscription.share_id = health.share_id
             WHERE subscription.id = ?1 AND health.checked_at <= ?2",
            params![product_ref, through.timestamp()],
            |row| row.get(0),
        )
    } else if product_kind == "client_host" {
        conn.query_row(
            "SELECT MAX(checked_at) FROM installation_health_checks
             WHERE installation_id = ?1 AND checked_at <= ?2",
            params![service_ref, through.timestamp()],
            |row| row.get(0),
        )
    } else {
        return Err(AppError::Internal("invalid wallet service kind".into()));
    }
    .map_err(db_error("read post-control wallet health evidence"))?;
    // Health checks are persisted to whole seconds. Requiring strictly newer
    // seconds for a subsecond control ACK prevents an earlier check from the
    // same second from being reused across a pause/resume boundary.
    let fence = parse_time(&fence)?;
    let earliest = fence.timestamp() + i64::from(fence.timestamp_subsec_nanos() > 0);
    Ok(checked_at.is_some_and(|checked_at| checked_at >= earliest))
}

fn reconcile_service_interval_tx(
    conn: &Connection,
    contract_id: &str,
    product_kind: &str,
    product_ref: &str,
    service_ref: &str,
    billing_model: &str,
    status: &str,
    cursor_text: &str,
    trial_remaining: i64,
    now: DateTime<Utc>,
    expiry: Option<DateTime<Utc>>,
) -> Result<(), AppError> {
    if !matches!(status, "trialing" | "active") {
        return Ok(());
    }
    let cursor = parse_time(cursor_text)?;
    let mut end = expiry.map_or(now, |expires| expires.min(now));
    if billing_model == "calendar_month" && status == "active" {
        let period_end: String = conn
            .query_row(
                "SELECT period.period_end FROM market_platform_periods period
                 JOIN market_platform_contracts contract ON contract.id = period.contract_id
                  AND contract.current_period_sequence = period.sequence
                 WHERE contract.id = ?1 AND period.status = 'open'",
                params![contract_id],
                |row| row.get(0),
            )
            .map_err(db_error("read open wallet month boundary"))?;
        end = end.min(parse_time(&period_end)?);
    }
    if status == "trialing" {
        end = end.min(cursor + chrono::Duration::hours(1));
    }
    // Both the trial and metered ledgers use whole service seconds.
    end = cursor + chrono::Duration::seconds((end - cursor).num_seconds());
    if end <= cursor {
        return Ok(());
    }
    if billing_model == "metered" && status == "active" {
        // Settle any older persisted seconds first. A new observation must not
        // be created against a budget that has already run out; otherwise the
        // control-plane pause races one more unfunded billing interval.
        if settle_metered_batch_tx(conn, contract_id, &now.to_rfc3339())?.needs_pause {
            return Ok(());
        }
        let (buyer, rate, hold_id, sequence): (String, i64, String, i64) = conn
            .query_row(
                "SELECT buyer_user_id, daily_rate_minor, budget_hold_id, budget_sequence
                 FROM market_platform_contracts WHERE id = ?1 AND status = 'active'",
                params![contract_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(db_error("read metered wallet budget before delivery"))?;
        let remaining: i64 = conn
            .query_row(
                "SELECT remaining_units FROM market_wallet_holds
                 WHERE id = ?1 AND status = 'active'",
                params![hold_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error("read remaining metered wallet budget"))?
            .unwrap_or(0);
        if remaining < rate {
            crate::market_reconcile::ensure_contract_finance_safe_tx(conn, contract_id)?;
            release_hold_tx(conn, &hold_id, &now.to_rfc3339())?;
            if replenish_metered_budget_tx(
                conn,
                contract_id,
                &buyer,
                rate,
                sequence,
                &now.to_rfc3339(),
            )?
            .is_none()
            {
                let control_available =
                    service_control_available_tx(conn, product_kind, product_ref, service_ref)?;
                let source_kind = if product_kind == "share" {
                    "share_control"
                } else {
                    "client_host_health"
                };
                record_service_evidence_tx(
                    conn,
                    contract_id,
                    source_kind,
                    cursor,
                    end,
                    "unknown",
                    false,
                    "budget_not_reserved",
                    &now.to_rfc3339(),
                )?;
                if conn
                    .execute(
                        "UPDATE market_platform_contracts
                     SET status = 'suspended', last_evaluated_at = ?2, updated_at = ?3
                     WHERE id = ?1 AND status = 'active' AND last_evaluated_at = ?4",
                        params![contract_id, end.to_rfc3339(), now.to_rfc3339(), cursor_text],
                    )
                    .map_err(db_error("suspend metered service before unfunded delivery"))?
                    == 1
                {
                    notify_contract_tx(
                        conn,
                        contract_id,
                        "contract_suspended",
                        "insufficient_funds",
                        &now.to_rfc3339(),
                    )?;
                }
                conn.execute(
                    "UPDATE market_platform_metered_observations
                     SET unfunded_seconds = billable_seconds - captured_seconds
                     WHERE contract_id = ?1
                       AND captured_seconds + unfunded_seconds < billable_seconds",
                    params![contract_id],
                )
                .map_err(db_error("isolate older unfunded metered evidence"))?;
                if control_available {
                    conn.execute(
                        "INSERT OR IGNORE INTO market_finance_reconciliation_cases
                            (id, code, object_kind, object_id, status, detail_code, created_at)
                         VALUES (?1, 'metered_unfunded_delivery', 'platform_contract', ?2,
                                 'open', 'budget_exhausted_before_control_pause', ?3)",
                        params![Uuid::new_v4().to_string(), contract_id, now.to_rfc3339()],
                    )
                    .map_err(db_error("record metered control pause lag"))?;
                }
                return Ok(());
            }
        }
    }
    let health_ref = if product_kind == "client_host" {
        service_ref
    } else {
        product_ref
    };
    let (mut observed_state, _) = crate::market_common::effective_service_health_tx(
        conn,
        product_kind,
        health_ref,
        service_ref,
        cursor,
        end,
    )?;
    let control_available =
        service_control_available_tx(conn, product_kind, product_ref, service_ref)?;
    let post_control_health = health_observed_after_control_fence_tx(
        conn,
        contract_id,
        product_kind,
        product_ref,
        service_ref,
        end,
    )?;
    if !post_control_health && observed_state == "healthy" {
        observed_state = "unknown".into();
    }
    let available = observed_state == "healthy" && control_available;
    let seconds = (end - cursor).num_seconds();
    let split_end = if status == "trialing" && available && trial_remaining > 0 {
        cursor + chrono::Duration::seconds(seconds.min(trial_remaining))
    } else {
        end
    };
    let kind = if product_kind == "share" {
        "share_control"
    } else {
        "client_host_health"
    };
    let reason = if seconds > 20 {
        "worker_gap"
    } else if !control_available {
        "control_not_active"
    } else if !post_control_health {
        "health_before_control_fence"
    } else if available {
        "service_healthy"
    } else {
        "service_unverified"
    };
    let source_id = record_service_evidence_tx(
        conn,
        contract_id,
        kind,
        cursor,
        split_end,
        &observed_state,
        available,
        reason,
        &now.to_rfc3339(),
    )?;
    if status == "trialing" {
        if trial_remaining == 0 {
            return Err(AppError::Internal(
                "trial has no remaining service time".into(),
            ));
        }
        advance_trial_interval_tx(
            conn,
            contract_id,
            kind,
            &source_id,
            &cursor.to_rfc3339(),
            &split_end.to_rfc3339(),
            available,
            &now.to_rfc3339(),
        )?;
    } else if billing_model == "metered" {
        if available {
            observe_metered_interval_tx(
                conn,
                contract_id,
                kind,
                &source_id,
                &cursor.to_rfc3339(),
                &split_end.to_rfc3339(),
                &now.to_rfc3339(),
            )?;
        }
        let changed = conn
            .execute(
                "UPDATE market_platform_contracts
             SET last_evaluated_at = ?2, updated_at = ?3
             WHERE id = ?1 AND last_evaluated_at = ?4 AND status = 'active'",
                params![
                    contract_id,
                    split_end.to_rfc3339(),
                    now.to_rfc3339(),
                    cursor_text
                ],
            )
            .map_err(db_error("advance wallet metered observation cursor"))?;
        if changed != 1 {
            return Err(AppError::Conflict("wallet metered cursor changed".into()));
        }
        // Previously persisted healthy time remains payable from its reserved
        // budget even if the latest health check has since failed.
        settle_metered_batch_tx(conn, contract_id, &now.to_rfc3339())?;
    } else {
        settle_monthly_segment_tx(
            conn,
            contract_id,
            &source_id,
            &cursor.to_rfc3339(),
            &split_end.to_rfc3339(),
            available,
            &now.to_rfc3339(),
        )?;
        let changed = conn
            .execute(
                "UPDATE market_platform_contracts
             SET last_evaluated_at = ?2, updated_at = ?3
             WHERE id = ?1 AND last_evaluated_at = ?4 AND status = 'active'",
                params![
                    contract_id,
                    split_end.to_rfc3339(),
                    now.to_rfc3339(),
                    cursor_text
                ],
            )
            .map_err(db_error("advance wallet monthly observation cursor"))?;
        if changed != 1 {
            return Err(AppError::Conflict("wallet monthly cursor changed".into()));
        }
    }
    Ok(())
}

fn reconcile_platform_contracts_tx(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<Vec<PlatformControlAction>, AppError> {
    let now_text = now.to_rfc3339();
    let mut statement = conn
        .prepare(
            "SELECT id, product_kind, product_ref, COALESCE(service_ref, ''),
                billing_model, status, COALESCE(last_evaluated_at, ''),
                trial_seconds_remaining, renewal_policy
         FROM market_platform_contracts
         WHERE status IN ('trialing', 'active', 'suspended', 'ending', 'review_required')
         ORDER BY updated_at, id LIMIT 100",
        )
        .map_err(db_error("prepare wallet service reconciliation"))?;
    let contracts = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(db_error("query wallet service reconciliation"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read wallet service reconciliation"))?;
    let mut actions = Vec::new();
    for (id, kind, product_ref, service_ref, model, status, cursor, remaining, policy) in contracts
    {
        // A corrupt or stale contract must not roll back every other buyer's
        // service and financial progress in this bounded worker pass.
        conn.execute_batch("SAVEPOINT wallet_service_contract")
            .map_err(db_error("begin wallet contract savepoint"))?;
        let outcome = (|| -> Result<Option<PlatformControlAction>, AppError> {
            if kind == "client_host" && service_ref.is_empty() {
                return Err(AppError::Internal(
                    "active Client wallet contract has no installation".into(),
                ));
            }
            let subscription_exists: i64 = if kind == "share" {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM share_market_subscriptions WHERE id = ?1)",
                    params![product_ref],
                    |row| row.get(0),
                )
            } else {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM client_market_subscriptions
                     WHERE installation_id = ?1)",
                    params![service_ref],
                    |row| row.get(0),
                )
            }
            .map_err(db_error("verify wallet service subscription"))?;
            if subscription_exists == 0 {
                crate::market_finance::hold_contract_earnings_tx(
                    conn,
                    &id,
                    "service_missing",
                    &now_text,
                )?;
                conn.execute(
                    "UPDATE market_platform_contracts
                     SET status = 'review_required', updated_at = ?2
                     WHERE id = ?1 AND status != 'review_required'",
                    params![id, now_text],
                )
                .map_err(db_error("isolate missing wallet service"))?;
                conn.execute(
                    "INSERT OR IGNORE INTO market_finance_reconciliation_cases
                        (id, code, object_kind, object_id, status, detail_code, created_at)
                     VALUES (?1, 'wallet_service_missing', 'platform_contract', ?2,
                             'open', 'subscription_requires_review', ?3)",
                    params![Uuid::new_v4().to_string(), id, now_text],
                )
                .map_err(db_error("record missing wallet service"))?;
                return Ok(None);
            }
            if status == "suspended" && enforce_suspension_timeout_tx(conn, &id, now)? {
                // Marked for cleanup; the next pass dispatches termination.
                conn.execute(
                    "UPDATE market_platform_contracts SET updated_at = ?2 WHERE id = ?1",
                    params![id, now_text],
                )
                .map_err(db_error("rotate expired suspension"))?;
                return Ok(None);
            }
            let expiry = service_expiry_tx(conn, &kind, &product_ref, &service_ref)?;
            if matches!(status.as_str(), "trialing" | "active") {
                reconcile_service_interval_tx(
                    conn,
                    &id,
                    &kind,
                    &product_ref,
                    &service_ref,
                    &model,
                    &status,
                    &cursor,
                    remaining,
                    now,
                    expiry,
                )?;
            }
            let (current_status, current_cursor, sequence): (String, Option<String>, i64) = conn
                .query_row(
                    "SELECT status, last_evaluated_at, current_period_sequence
             FROM market_platform_contracts WHERE id = ?1",
                    params![id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(db_error("read reconciled wallet service state"))?;
            if matches!(current_status.as_str(), "active" | "trialing" | "suspended")
                && expiry.is_some_and(|expires| {
                    expires <= now
                        && current_cursor
                            .as_deref()
                            .and_then(|cursor| parse_time(cursor).ok())
                            .is_some_and(|cursor| cursor >= expires)
                })
            {
                mark_for_confirmed_cleanup_tx(conn, &id, &now_text)?;
            } else if model == "calendar_month" && current_status == "active" && sequence > 0 {
                let (end, period_status): (String, String) = conn
                    .query_row(
                        "SELECT period_end, status FROM market_platform_periods
                 WHERE contract_id = ?1 AND sequence = ?2",
                        params![id, sequence],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(db_error("read wallet renewal boundary"))?;
                if period_status == "closed" && parse_time(&end)? <= now {
                    if policy == "automatic" {
                        renew_monthly_contract_tx(conn, &id, &now_text)?;
                    } else {
                        mark_for_confirmed_cleanup_tx(conn, &id, &now_text)?;
                    }
                }
            } else if current_status == "suspended" {
                resume_suspended_contract_tx(conn, &id, &now_text)?;
            }
            let final_status: String = conn
                .query_row(
                    "SELECT status FROM market_platform_contracts WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .map_err(db_error("read final wallet control state"))?;
            let subscription_status: Option<String> = if kind == "share" {
                conn.query_row(
                    "SELECT status FROM share_market_subscriptions WHERE id = ?1",
                    params![product_ref],
                    |row| row.get(0),
                )
            } else {
                conn.query_row(
                    "SELECT status FROM client_market_subscriptions WHERE installation_id = ?1",
                    params![service_ref],
                    |row| row.get(0),
                )
            }
            .optional()
            .map_err(db_error("read wallet service control state"))?;
            let action = match (final_status.as_str(), subscription_status.as_deref()) {
                (
                    "suspended",
                    Some(
                        "active"
                        | "active_free"
                        | "active_postpaid"
                        | "active_prepaid"
                        | "billing_resume_pending",
                    ),
                ) => Some(PlatformControlKind::Suspend),
                ("active", Some("billing_suspended" | "billing_control_failed")) => {
                    Some(PlatformControlKind::Resume)
                }
                (
                    "review_required" | "ending",
                    Some(
                        "active"
                        | "active_free"
                        | "active_postpaid"
                        | "active_prepaid"
                        | "billing_suspended"
                        | "billing_suspend_pending"
                        | "billing_resume_pending"
                        | "billing_control_failed",
                    ),
                ) => Some(PlatformControlKind::Terminate),
                _ => None,
            };
            let control = action.map(|kind_of_action| PlatformControlAction {
                contract_id: id.clone(),
                product_kind: kind.clone(),
                product_ref: product_ref.clone(),
                service_ref: service_ref.clone(),
                kind: kind_of_action,
            });
            conn.execute(
                "UPDATE market_platform_contracts SET updated_at = ?2 WHERE id = ?1",
                params![id, now_text],
            )
            .map_err(db_error("rotate wallet reconciliation cursor"))?;
            Ok(control)
        })();
        match outcome {
            Ok(control) => {
                conn.execute_batch("RELEASE SAVEPOINT wallet_service_contract")
                    .map_err(db_error("commit wallet contract savepoint"))?;
                if let Some(control) = control {
                    actions.push(control);
                }
            }
            Err(error) if crate::market_reconcile::is_finance_hold(&error) => {
                // A scoped finance hold is not a broken contract. Suspend the
                // service so it cannot keep running unbilled; the regular
                // resume path restarts it once the hold is cleared.
                conn.execute_batch(
                    "ROLLBACK TO SAVEPOINT wallet_service_contract;
                     RELEASE SAVEPOINT wallet_service_contract",
                )
                .map_err(db_error("rollback wallet contract savepoint"))?;
                let suspended = conn
                    .execute(
                        "UPDATE market_platform_contracts
                         SET status = 'suspended', suspension_cause = 'finance_hold', updated_at = ?2
                         WHERE id = ?1 AND status IN ('active', 'trialing')",
                        params![id, now_text],
                    )
                    .map_err(db_error("suspend held wallet contract"))?;
                if suspended == 1 {
                    notify_contract_tx(conn, &id, "contract_suspended", "finance_hold", &now_text)?;
                    actions.push(PlatformControlAction {
                        contract_id: id.clone(),
                        product_kind: kind.clone(),
                        product_ref: product_ref.clone(),
                        service_ref: service_ref.clone(),
                        kind: PlatformControlKind::Suspend,
                    });
                }
                tracing::warn!(contract_id = %id, "wallet contract suspended by finance hold");
            }
            Err(error) => {
                conn.execute_batch(
                    "ROLLBACK TO SAVEPOINT wallet_service_contract;
                     RELEASE SAVEPOINT wallet_service_contract",
                )
                .map_err(db_error("rollback wallet contract savepoint"))?;
                conn.execute(
                    "INSERT OR IGNORE INTO market_finance_reconciliation_cases
                        (id, code, object_kind, object_id, status, detail_code, created_at)
                     VALUES (?1, 'wallet_service_reconcile_failed', 'platform_contract', ?2,
                             'open', 'contract_requires_review', ?3)",
                    params![Uuid::new_v4().to_string(), id, now_text],
                )
                .map_err(db_error("record wallet contract reconciliation failure"))?;
                conn.execute(
                    "UPDATE market_platform_contracts SET updated_at = ?2 WHERE id = ?1",
                    params![id, now_text],
                )
                .map_err(db_error("rotate failed wallet contract"))?;
                tracing::warn!(contract_id = %id, error = %error, "wallet contract requires review");
            }
        }
    }
    Ok(actions)
}

async fn dispatch_platform_controls(
    state: &crate::ServerState,
    actions: Vec<PlatformControlAction>,
) {
    let _guard = state.market_wallet_controls.lock().await;
    for action in actions {
        let current = {
            let conn = state.store.conn.lock().await;
            conn.query_row(
                "SELECT status FROM market_platform_contracts WHERE id = ?1",
                params![action.contract_id],
                |row| row.get::<_, String>(0),
            )
        };
        let Ok(status) = current else { continue };
        if !match action.kind {
            PlatformControlKind::Suspend => status == "suspended",
            PlatformControlKind::Resume => status == "active",
            PlatformControlKind::Terminate => {
                matches!(status.as_str(), "review_required" | "ending")
            }
        } {
            continue;
        }
        let result = match (action.kind, action.product_kind.as_str()) {
            (PlatformControlKind::Suspend, "share") => {
                crate::share_market::suspend_for_billing(
                    state,
                    &action.product_ref,
                    "wallet_funding_required",
                )
                .await
            }
            (PlatformControlKind::Resume, "share") => {
                crate::share_market::resume_after_billing(state, &action.product_ref).await
            }
            (PlatformControlKind::Terminate, "share") => {
                crate::share_market::terminate_for_billing(
                    state,
                    &action.product_ref,
                    "wallet_term_ended",
                )
                .await
            }
            (PlatformControlKind::Suspend, "client_host") => {
                let _lock = state.client_market_actions.lock(&action.service_ref).await;
                match state
                    .store
                    .client_market_set_billing_suspended(&action.service_ref, true)
                    .await
                {
                    Ok(subdomain) => {
                        if let Some(subdomain) = subdomain {
                            state.proxy.remove_route(&subdomain).await;
                        }
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            (PlatformControlKind::Resume, "client_host") => {
                let _lock = state.client_market_actions.lock(&action.service_ref).await;
                state
                    .store
                    .client_market_set_billing_suspended(&action.service_ref, false)
                    .await
                    .map(|_| ())
            }
            (PlatformControlKind::Terminate, "client_host") => {
                crate::client_market::terminate_for_billing(
                    state,
                    &action.service_ref,
                    "wallet_term_ended",
                )
                .await
            }
            _ => Err(AppError::Internal("invalid wallet control rail".into())),
        };
        if let Err(error) = result {
            tracing::warn!(contract_id = %action.contract_id, error = %error, "wallet service control failed");
        }
    }
}

pub(crate) async fn run_service(state: crate::ServerState) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let result = {
            let conn = state.store.conn.lock().await;
            let tx = conn.transaction_with_behavior(crate::db::TransactionBehavior::Immediate);
            match tx {
                Ok(tx) => match reconcile_platform_contracts_tx(&tx, Utc::now()) {
                    Ok(actions) => tx
                        .commit()
                        .map(|_| actions)
                        .map_err(db_error("commit wallet service reconciliation")),
                    Err(error) => Err(error),
                },
                Err(error) => Err(db_error("begin wallet service reconciliation")(error)),
            }
        };
        match result {
            Ok(actions) => dispatch_platform_controls(&state, actions).await,
            Err(error) => tracing::warn!(error = %error, "wallet service reconciliation failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TransactionBehavior;
    use crate::market_finance::{provider_summary_tx, unlock_due_earnings_tx};
    use crate::market_wallet::{credit_verified_topup_tx, reconcile_wallet_tx, wallet_snapshot_tx};

    fn memory() -> Connection {
        crate::test_support::database().unwrap()
    }

    #[test]
    fn monthly_wallet_renewal_is_buyer_scoped_and_cannot_change_a_terminal_contract() {
        let conn = memory();
        let at = "2026-10-16T00:00:00Z";
        conn.execute(
            "INSERT INTO market_platform_contracts (
                id, product_kind, product_ref, trial_scope_ref, quote_id,
                request_fingerprint, buyer_user_id, provider_user_id,
                offer_revision, billing_model, cycle_price_minor,
                renewal_policy, status, created_at, updated_at
             ) VALUES ('monthly', 'share', 'subscription', 'share', 'quote',
                       'request', 'buyer', 'provider', 1, 'calendar_month', 100,
                       'manual', 'trialing', ?1, ?1)",
            params![at],
        )
        .unwrap();
        assert!(
            wallet_lifecycle_view_tx(&conn, "monthly", "other")
                .unwrap()
                .is_none()
        );
        conn.execute("UPDATE market_platform_contracts SET trial_seconds_remaining = 1234 WHERE id = 'monthly'", []).unwrap();
        let lifecycle = wallet_lifecycle_view_tx(&conn, "monthly", "buyer")
            .unwrap()
            .unwrap();
        assert_eq!(lifecycle.status, "trialing");
        assert_eq!(lifecycle.trial_seconds_remaining, 1234);
        assert!(lifecycle.period_end.is_none());
        assert_eq!(
            wallet_renewal_view_tx(&conn, "monthly", "buyer")
                .unwrap()
                .trial_seconds_remaining,
            1234
        );
        let enable = UpdateWalletRenewalRequest {
            auto_renew: true,
            expected_policy: "manual".into(),
        };
        assert!(matches!(
            set_wallet_renewal_tx(&conn, "monthly", "other", &enable, at),
            Err(AppError::NotFound(_))
        ));
        let current = set_wallet_renewal_tx(&conn, "monthly", "buyer", &enable, at).unwrap();
        assert_eq!(current.renewal_policy, "automatic");
        assert!(matches!(
            set_wallet_renewal_tx(&conn, "monthly", "buyer", &enable, at),
            Err(AppError::Conflict(_))
        ));
        conn.execute(
            "UPDATE market_platform_contracts SET status = 'ended' WHERE id = 'monthly'",
            [],
        )
        .unwrap();
        assert!(matches!(
            set_wallet_renewal_tx(
                &conn,
                "monthly",
                "buyer",
                &UpdateWalletRenewalRequest {
                    auto_renew: false,
                    expected_policy: "automatic".into(),
                },
                at,
            ),
            Err(AppError::Conflict(_))
        ));
    }

    #[test]
    fn existing_due_services_precede_new_wallet_orders_and_sort_stably() {
        let conn = memory();
        let time = "2026-10-03T00:00:00+00:00";
        crate::market_wallet::credit_verified_topup_tx(
            &conn, "buyer", "topup", 250, "receipt", time,
        )
        .unwrap();
        for (id, due, priority) in [
            ("later", "2026-10-02T00:00:00+00:00", 10),
            ("low", "2026-10-01T00:00:00+00:00", 0),
            ("high-z", "2026-10-01T00:00:00+00:00", 5),
            ("high-a", "2026-10-01T00:00:00+00:00", 5),
        ] {
            conn.execute(
                "INSERT INTO market_platform_contracts (
                    id, product_kind, product_ref, trial_scope_ref, quote_id,
                    request_fingerprint, buyer_user_id, provider_user_id,
                    offer_revision, billing_model, cycle_price_minor,
                    renewal_policy, status, created_at, updated_at,
                    last_evaluated_at, renewal_priority
                 ) VALUES (?1, 'share', ?1, ?1, ?1, ?1, 'buyer', 'provider',
                           1, 'calendar_month', 50, 'manual', 'trialing', ?2, ?2, ?2, ?3)",
                params![id, due, priority],
            )
            .unwrap();
        }
        let unit = units_from_minor(50).unwrap();
        assert_eq!(
            prior_due_funding_units_tx(&conn, "buyer", Some("high-a"), time).unwrap(),
            0
        );
        assert_eq!(
            prior_due_funding_units_tx(&conn, "buyer", Some("high-z"), time).unwrap(),
            unit
        );
        assert_eq!(
            prior_due_funding_units_tx(&conn, "buyer", Some("low"), time).unwrap(),
            2 * unit
        );
        assert_eq!(
            prior_due_funding_units_tx(&conn, "buyer", Some("later"), time).unwrap(),
            3 * unit
        );
        let funding =
            crate::market_wallet::wallet_quote_funding_tx(&conn, "buyer", 100, 0, time).unwrap();
        assert_eq!(funding.committed_units, 4 * unit);
        assert_eq!(funding.shortfall_units, units_from_minor(50).unwrap());
        let order = PrepareContract {
            product_kind: "share",
            product_ref: "new-order",
            trial_scope_ref: "new-share",
            quote_id: "new-quote",
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: BillingModel::CalendarMonth,
            price_minor: 100,
            auto_renew: false,
            trial_seconds: 0,
        };
        let error = prepare_contract_tx(&conn, &order, time).unwrap_err();
        assert_eq!(error.code(), Some("MARKET_WALLET_REQUIRED"));
        if let AppError::Coded { details, .. } = error {
            assert_eq!(
                details["shortfallUnits"],
                units_from_minor(50).unwrap().to_string()
            );
        }
        crate::market_wallet::credit_verified_topup_tx(
            &conn,
            "buyer",
            "topup-more",
            50,
            "receipt-more",
            time,
        )
        .unwrap();
        prepare_contract_tx(&conn, &order, time).unwrap();
        crate::market_wallet::reconcile_wallet_tx(&conn, "buyer").unwrap();
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
    fn buyer_release_only_schedules_a_paid_month_and_suspended_renewal_can_be_disabled() {
        let conn = memory();
        let at = "2026-10-01T00:00:00Z";
        credit_verified_topup_tx(&conn, "buyer", "cancel-topup", 2000, "cancel-receipt", at)
            .unwrap();
        let id = prepare_contract_tx(&conn, &monthly("cancel-quote", true), at).unwrap();
        assert!(!cancel_at_period_end_tx(&conn, &id, "buyer", at).unwrap());
        assert!(matches!(
            cancel_at_period_end_tx(&conn, &id, "other", at),
            Err(AppError::NotFound(_))
        ));
        activate_contract_tx(&conn, &id, at, at).unwrap();
        assert!(cancel_at_period_end_tx(&conn, &id, "buyer", at).unwrap());
        assert!(cancel_at_period_end_tx(&conn, &id, "buyer", at).unwrap());
        let view = wallet_renewal_view_tx(&conn, &id, "buyer").unwrap();
        assert_eq!(view.status, "active");
        assert_eq!(view.renewal_policy, "manual");
        assert!(view.period_end.is_some());
        conn.execute("UPDATE market_platform_contracts SET status = 'suspended', renewal_policy = 'automatic' WHERE id = ?1", params![id]).unwrap();
        assert!(!cancel_at_period_end_tx(&conn, &id, "buyer", at).unwrap());
        let result = set_wallet_renewal_tx(
            &conn,
            &id,
            "buyer",
            &UpdateWalletRenewalRequest {
                auto_renew: false,
                expected_policy: "automatic".into(),
            },
            at,
        )
        .unwrap();
        assert_eq!(result.renewal_policy, "manual");
        reconcile_wallet_tx(&conn, "buyer").unwrap();
    }

    #[test]
    fn metered_release_is_immediate_and_pending_cleanup_releases_a_hold_once() {
        let conn = memory();
        let at = "2026-10-01T00:00:00Z";
        credit_verified_topup_tx(&conn, "buyer", "pending-topup", 2000, "pending-receipt", at)
            .unwrap();
        let input = PrepareContract {
            billing_model: BillingModel::Metered,
            auto_renew: false,
            ..monthly("pending-quote", false)
        };
        let id = prepare_contract_tx(&conn, &input, at).unwrap();
        assert!(!cancel_at_period_end_tx(&conn, &id, "buyer", at).unwrap());
        let before = wallet_snapshot_tx(&conn, "buyer").unwrap();
        assert!(before.held_units > 0);
        assert!(
            !terminate_after_cleanup_tx(&conn, &id, false, at)
                .unwrap()
                .finalized
        );
        assert_eq!(
            wallet_snapshot_tx(&conn, "buyer").unwrap().held_units,
            before.held_units
        );
        for _ in 0..2 {
            assert!(
                terminate_after_cleanup_tx(&conn, &id, true, at)
                    .unwrap()
                    .finalized
            );
        }
        let after = wallet_snapshot_tx(&conn, "buyer").unwrap();
        assert_eq!(after.held_units, 0);
        assert_eq!(after.posted_units, before.posted_units);
        reconcile_wallet_tx(&conn, "buyer").unwrap();
    }

    fn client_monthly_fixture(conn: &Connection, start: &str, trial_seconds: i64) -> String {
        credit_verified_topup_tx(conn, "buyer", "fixture-topup", 1_000, "receipt", start).unwrap();
        let input = PrepareContract {
            product_kind: "client_host",
            product_ref: "provision-job-1",
            trial_scope_ref: "host-1",
            quote_id: "quote-1",
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: BillingModel::CalendarMonth,
            price_minor: 1_000,
            auto_renew: true,
            trial_seconds,
        };
        let id = prepare_contract_tx(conn, &input, start).unwrap();
        bind_client_installation_tx(
            conn,
            &id,
            "provision-job-1",
            "installation-1",
            "buyer",
            "provider",
            start,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO client_market_subscriptions
                (installation_id, host_id, provider_id, host_owner_email,
                 client_user_id, client_owner_email, status, currency,
                 offer_revision, activated_at, created_at, updated_at,
                 platform_contract_id)
             VALUES ('installation-1', 'host-1', 'provider', 'provider@example.test',
                     'buyer', 'buyer@example.test', 'active', 'USD',
                     1, ?1, ?1, ?1, ?2)",
            params![start, id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO installation_client_tunnels
                (installation_id, owner_email, subdomain, enabled, created_at, updated_at)
             VALUES ('installation-1', 'buyer@example.test', 'client-1', 1, ?1, ?1)",
            params![start],
        )
        .unwrap();
        activate_contract_tx(conn, &id, start, start).unwrap();
        id
    }

    #[test]
    fn active_free_share_trial_can_observe_health_and_convert_once() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let end = parse_time(start).unwrap() + chrono::Duration::seconds(5);
        credit_verified_topup_tx(&conn, "buyer", "share-trial-topup", 100, "receipt", start)
            .unwrap();
        conn.execute(
            "INSERT INTO share_market_listings
                (id, share_id, installation_id, owner_user_id, owner_email,
                 status, created_at, updated_at)
             VALUES ('listing-1', 'share-1', 'installation-1', 'provider',
                     'provider@example.test', 'active', ?1, ?1)",
            params![start],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO share_market_seats
                (id, listing_id, position, status, token_period_json,
                 offer_revision, created_at, updated_at)
             VALUES ('seat-1', 'listing-1', 1, 'occupied', '{}', 1, ?1, ?1)",
            params![start],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO share_market_subscriptions
                (id, seat_id, listing_id, share_id, installation_id, entitlement_id,
                 owner_user_id, owner_email, renter_user_id, renter_email,
                 status, token_period_json, offer_revision, created_at, updated_at)
             VALUES ('subscription-1', 'seat-1', 'listing-1', 'share-1',
                     'installation-1', 'entitlement-1', 'provider', 'provider@example.test',
                     'buyer', 'buyer@example.test', 'active_free', '{}', 1, ?1, ?1)",
            params![start],
        )
        .unwrap();
        let contract_id = prepare_contract_tx(
            &conn,
            &PrepareContract {
                product_kind: "share",
                product_ref: "subscription-1",
                trial_scope_ref: "share-1",
                quote_id: "share-quote",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::CalendarMonth,
                price_minor: 100,
                auto_renew: false,
                trial_seconds: 5,
            },
            start,
        )
        .unwrap();
        activate_contract_tx(&conn, &contract_id, start, start).unwrap();
        conn.execute(
            "INSERT INTO share_health_checks
                (share_id, checked_at, is_healthy, status, reason)
             VALUES ('share-1', ?1, 1, 'healthy', 'route_healthy')",
            params![end.timestamp()],
        )
        .unwrap();
        reconcile_platform_contracts_tx(&conn, end).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT status, trial_seconds_remaining FROM market_platform_contracts
                 WHERE id = ?1",
                params![contract_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            ("active".into(), 0),
        );
        reconcile_platform_contracts_tx(&conn, end).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_platform_periods WHERE contract_id = ?1",
                params![contract_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1,
        );
    }

    #[test]
    fn runtime_uses_persisted_client_health_and_replays_without_double_earning() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let end = parse_time(start).unwrap() + chrono::Duration::seconds(5);
        let id = client_monthly_fixture(&conn, start, 0);
        conn.execute(
            "INSERT INTO installation_health_checks
                (installation_id, checked_at, is_healthy, status, reason)
             VALUES ('installation-1', ?1, 1, 'healthy', 'route_healthy')",
            params![end.timestamp()],
        )
        .unwrap();
        assert!(
            reconcile_platform_contracts_tx(&conn, end)
                .unwrap()
                .is_empty()
        );
        let earned: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(amount_units), 0) FROM market_provider_earning_lots",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(earned > 0);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_platform_service_evidence WHERE contract_id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        reconcile_platform_contracts_tx(&conn, end).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COALESCE(SUM(amount_units), 0) FROM market_provider_earning_lots",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            earned
        );
        reconcile_wallet_tx(&conn, "buyer").unwrap();
    }

    #[test]
    fn health_from_before_a_control_resume_cannot_create_new_income() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let start_at = parse_time(start).unwrap();
        let id = client_monthly_fixture(&conn, start, 0);
        conn.execute(
            "INSERT INTO installation_health_checks
                (installation_id, checked_at, is_healthy, status, reason)
             VALUES ('installation-1', ?1, 1, 'healthy', 'route_healthy')",
            params![(start_at + chrono::Duration::seconds(5)).timestamp()],
        )
        .unwrap();
        reconcile_platform_contracts_tx(&conn, start_at + chrono::Duration::seconds(10)).unwrap();
        let earned_before: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(amount_units), 0) FROM market_provider_earning_lots",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(earned_before > 0);
        // Represent the control ACK boundary after a temporary suspension.
        // The prior healthy sample remains fresh by age but is not a valid
        // post-resume proof.
        conn.execute(
            "UPDATE market_platform_contracts SET health_fence_at = ?2 WHERE id = ?1",
            params![id, (start_at + chrono::Duration::seconds(10)).to_rfc3339()],
        )
        .unwrap();
        reconcile_platform_contracts_tx(&conn, start_at + chrono::Duration::seconds(15)).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COALESCE(SUM(amount_units), 0) FROM market_provider_earning_lots",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            earned_before,
        );
        assert_eq!(
            conn.query_row(
                "SELECT reason_code FROM market_platform_service_evidence
                 WHERE contract_id = ?1 AND started_at = ?2",
                params![id, (start_at + chrono::Duration::seconds(10)).to_rfc3339()],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "health_before_control_fence",
        );
        conn.execute(
            "INSERT INTO installation_health_checks
                (installation_id, checked_at, is_healthy, status, reason)
             VALUES ('installation-1', ?1, 1, 'healthy', 'route_healthy')",
            params![(start_at + chrono::Duration::seconds(16)).timestamp()],
        )
        .unwrap();
        reconcile_platform_contracts_tx(&conn, start_at + chrono::Duration::seconds(20)).unwrap();
        assert!(
            conn.query_row(
                "SELECT COALESCE(SUM(amount_units), 0) FROM market_provider_earning_lots",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
                > earned_before
        );
        reconcile_wallet_tx(&conn, "buyer").unwrap();
    }

    #[test]
    fn exhausted_metered_budget_suspends_before_observing_more_paid_service() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        credit_verified_topup_tx(
            &conn,
            "buyer",
            "metered-budget-topup",
            100_000,
            "receipt",
            start,
        )
        .unwrap();
        let id = prepare_contract_tx(
            &conn,
            &PrepareContract {
                product_kind: "client_host",
                product_ref: "metered-job",
                trial_scope_ref: "metered-host",
                quote_id: "metered-quote",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::Metered,
                price_minor: 100_000,
                auto_renew: false,
                trial_seconds: 0,
            },
            start,
        )
        .unwrap();
        bind_client_installation_tx(
            &conn,
            &id,
            "metered-job",
            "metered-installation",
            "buyer",
            "provider",
            start,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO client_market_subscriptions
                (installation_id, host_id, provider_id, host_owner_email,
                 client_user_id, client_owner_email, status, currency,
                 offer_revision, activated_at, created_at, updated_at,
                 platform_contract_id)
             VALUES ('metered-installation', 'metered-host', 'provider',
                     'provider@example.test', 'buyer', 'buyer@example.test',
                     'active', 'USD', 1, ?1, ?1, ?1, ?2)",
            params![start, id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO installation_client_tunnels
                (installation_id, owner_email, subdomain, enabled, created_at, updated_at)
             VALUES ('metered-installation', 'buyer@example.test', 'metered-client', 1, ?1, ?1)",
            params![start],
        )
        .unwrap();
        activate_contract_tx(&conn, &id, start, start).unwrap();
        // Consume nearly all of the reserved hold through the journal to
        // exercise the pre-delivery fence without constructing 86,400
        // individual second-by-second metered observations in this test.
        let hold_id: String = conn
            .query_row(
                "SELECT budget_hold_id FROM market_platform_contracts WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        capture_hold_tx(
            &conn,
            &hold_id,
            units_from_minor(100_000).unwrap() - 40_000,
            "provider_pending",
            "provider",
            "test_budget_spend",
            &id,
            start,
        )
        .unwrap();
        let next = parse_time(start).unwrap() + chrono::Duration::seconds(5);
        reconcile_platform_contracts_tx(&conn, next).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT status FROM market_platform_contracts WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "suspended",
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_platform_metered_observations WHERE contract_id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0,
        );
        assert_eq!(
            conn.query_row(
                "SELECT reason_code FROM market_platform_service_evidence WHERE contract_id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "budget_not_reserved",
        );
        assert_eq!(
            wallet_snapshot_tx(&conn, "buyer").unwrap().available_units,
            40_000
        );
        reconcile_wallet_tx(&conn, "buyer").unwrap();
    }

    #[test]
    fn runtime_refunds_unverified_month_instead_of_creating_provider_income() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let end = parse_time(start).unwrap() + chrono::Duration::seconds(5);
        let id = client_monthly_fixture(&conn, start, 0);
        reconcile_platform_contracts_tx(&conn, end).unwrap();
        let (refunded, fulfilled): (i64, i64) = conn
            .query_row(
                "SELECT refunded_units, fulfilled_units FROM market_platform_periods
             WHERE contract_id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(refunded > 0);
        assert_eq!(fulfilled, 0);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_provider_earning_lots",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        reconcile_wallet_tx(&conn, "buyer").unwrap();
    }

    #[test]
    fn healthy_host_without_buyer_tunnel_is_not_paid_delivery() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let end = parse_time(start).unwrap() + chrono::Duration::seconds(5);
        let id = client_monthly_fixture(&conn, start, 0);
        conn.execute(
            "UPDATE installation_client_tunnels SET enabled = 0
             WHERE installation_id = 'installation-1'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO installation_health_checks
                (installation_id, checked_at, is_healthy, status, reason)
             VALUES ('installation-1', ?1, 1, 'healthy', 'route_healthy')",
            params![end.timestamp()],
        )
        .unwrap();
        reconcile_platform_contracts_tx(&conn, end).unwrap();
        let (refunded, fulfilled): (i64, i64) = conn
            .query_row(
                "SELECT refunded_units, fulfilled_units FROM market_platform_periods
                 WHERE contract_id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(refunded > 0);
        assert_eq!(fulfilled, 0);
    }

    #[test]
    fn healthy_host_with_pending_billing_control_is_not_paid_delivery() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let end = parse_time(start).unwrap() + chrono::Duration::seconds(5);
        let id = client_monthly_fixture(&conn, start, 0);
        conn.execute(
            "UPDATE client_market_subscriptions SET status = 'billing_suspended'
             WHERE installation_id = 'installation-1'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO installation_health_checks
                (installation_id, checked_at, is_healthy, status, reason)
             VALUES ('installation-1', ?1, 1, 'healthy', 'route_healthy')",
            params![end.timestamp()],
        )
        .unwrap();
        reconcile_platform_contracts_tx(&conn, end).unwrap();
        let (refunded, fulfilled): (i64, i64) = conn
            .query_row(
                "SELECT refunded_units, fulfilled_units FROM market_platform_periods
                 WHERE contract_id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(refunded > 0);
        assert_eq!(fulfilled, 0);
    }

    #[test]
    fn malformed_contract_does_not_starve_other_wallet_delivery() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let end = parse_time(start).unwrap() + chrono::Duration::seconds(5);
        let valid_id = client_monthly_fixture(&conn, start, 0);
        let invalid_id = prepare_contract_tx(
            &conn,
            &PrepareContract {
                product_kind: "client_host",
                product_ref: "unbound-job",
                trial_scope_ref: "unbound-host",
                quote_id: "unbound-quote",
                buyer_user_id: "buyer",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::Metered,
                price_minor: 100,
                auto_renew: false,
                trial_seconds: 5,
            },
            start,
        )
        .unwrap();
        conn.execute(
            "UPDATE market_platform_contracts
             SET status = 'trialing', activated_at = ?2, last_evaluated_at = ?2
             WHERE id = ?1",
            params![invalid_id, start],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO installation_health_checks
                (installation_id, checked_at, is_healthy, status, reason)
             VALUES ('installation-1', ?1, 1, 'healthy', 'route_healthy')",
            params![end.timestamp()],
        )
        .unwrap();
        reconcile_platform_contracts_tx(&conn, end).unwrap();
        assert!(
            conn.query_row(
                "SELECT fulfilled_units FROM market_platform_periods WHERE contract_id = ?1",
                params![valid_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
                > 0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases
                 WHERE code = 'wallet_service_reconcile_failed'
                   AND object_id = ?1 AND status = 'open'",
                params![invalid_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1,
        );
    }

    #[test]
    fn missing_subscription_freezes_wallet_contract_instead_of_renewing_it() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let end = parse_time(start).unwrap() + chrono::Duration::seconds(5);
        let id = client_monthly_fixture(&conn, start, 0);
        conn.execute(
            "DELETE FROM client_market_subscriptions WHERE installation_id = 'installation-1'",
            [],
        )
        .unwrap();
        reconcile_platform_contracts_tx(&conn, end).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT status FROM market_platform_contracts WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "review_required",
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases
                 WHERE code = 'wallet_service_missing' AND object_id = ?1 AND status = 'open'",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1,
        );
    }

    #[test]
    fn finance_hold_suspension_restarts_the_buyer_clock_once_the_hold_clears() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let id = client_monthly_fixture(&conn, start, 0);
        conn.execute(
            "UPDATE market_platform_contracts
             SET status = 'suspended', suspension_cause = 'finance_hold', updated_at = ?2
             WHERE id = ?1",
            params![id, start],
        )
        .unwrap();
        let cleared = parse_time(start).unwrap() + chrono::Duration::days(30);
        // A month under a Router hold is not the buyer's unfunded time.
        assert!(!enforce_suspension_timeout_tx(&conn, &id, cleared).unwrap());
        let (cause, suspended_at): (Option<String>, String) = conn
            .query_row(
                "SELECT suspension_cause, suspended_at FROM market_platform_contracts WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cause, None);
        assert_eq!(parse_time(&suspended_at).unwrap(), cleared);
        assert!(
            !enforce_suspension_timeout_tx(&conn, &id, cleared + chrono::Duration::days(6))
                .unwrap()
        );
        assert!(
            enforce_suspension_timeout_tx(&conn, &id, cleared + chrono::Duration::days(7)).unwrap()
        );
    }

    #[test]
    fn expired_suspended_wallet_service_terminates_instead_of_waiting_for_topup() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let id = client_monthly_fixture(&conn, start, 0);
        conn.execute(
            "UPDATE market_platform_contracts SET status = 'suspended' WHERE id = ?1",
            params![id],
        )
        .unwrap();
        conn.execute(
            "UPDATE client_market_subscriptions SET expires_at = ?1
             WHERE installation_id = 'installation-1'",
            params![start],
        )
        .unwrap();
        let actions = reconcile_platform_contracts_tx(
            &conn,
            parse_time(start).unwrap() + chrono::Duration::seconds(5),
        )
        .unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, PlatformControlKind::Terminate);
        assert_eq!(actions[0].contract_id, id);
        assert_eq!(
            conn.query_row(
                "SELECT status FROM market_platform_contracts WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "ending",
        );
    }

    #[test]
    fn runtime_converts_only_observed_trial_seconds_then_starts_paid_month() {
        let conn = memory();
        let start = "2026-01-01T00:00:00+00:00";
        let end = parse_time(start).unwrap() + chrono::Duration::seconds(5);
        let id = client_monthly_fixture(&conn, start, 5);
        conn.execute(
            "INSERT INTO installation_health_checks
                (installation_id, checked_at, is_healthy, status, reason)
             VALUES ('installation-1', ?1, 1, 'healthy', 'route_healthy')",
            params![end.timestamp()],
        )
        .unwrap();
        reconcile_platform_contracts_tx(&conn, end).unwrap();
        let (status, paid_start): (String, String) = conn
            .query_row(
                "SELECT status, paid_started_at FROM market_platform_contracts WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "active");
        assert_eq!(parse_time(&paid_start).unwrap(), end);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_provider_earning_lots",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        reconcile_wallet_tx(&conn, "buyer").unwrap();
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
        tx.execute(
            "UPDATE market_finance_control SET new_orders_paused = 1 WHERE id = 1",
            [],
        )
        .unwrap();
        assert_eq!(
            prepare_contract_tx(&tx, &monthly("quote-1", false), start).unwrap(),
            id
        );
        let mut another = monthly("quote-2", false);
        another.product_ref = "subscription-2";
        assert!(matches!(
            prepare_contract_tx(&tx, &another, start),
            Err(AppError::ServiceUnavailable(_))
        ));
        let contract_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM market_platform_contracts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(contract_count, 1);
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
    fn later_monthly_renewal_cannot_take_an_earlier_services_last_balance() {
        let conn = memory();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let first_start = "2026-01-01T00:00:00+00:00";
        let second_start = "2026-01-02T00:00:00+00:00";
        let first_end = "2026-02-01T00:00:00+00:00";
        let second_end = "2026-02-02T00:00:00+00:00";
        credit_verified_topup_tx(&tx, "buyer", "topup", 3_000, "receipt", first_start).unwrap();
        let first = PrepareContract {
            product_ref: "first-service",
            trial_scope_ref: "first-share",
            ..monthly("first-quote", true)
        };
        let second = PrepareContract {
            product_ref: "second-service",
            trial_scope_ref: "second-share",
            ..monthly("second-quote", true)
        };
        let first_id = prepare_contract_tx(&tx, &first, first_start).unwrap();
        activate_contract_tx(&tx, &first_id, first_start, first_start).unwrap();
        let second_id = prepare_contract_tx(&tx, &second, second_start).unwrap();
        activate_contract_tx(&tx, &second_id, second_start, second_start).unwrap();
        settle_monthly_segment_tx(
            &tx,
            &first_id,
            "first-month",
            first_start,
            first_end,
            true,
            first_end,
        )
        .unwrap();
        settle_monthly_segment_tx(
            &tx,
            &second_id,
            "second-month",
            second_start,
            second_end,
            true,
            second_end,
        )
        .unwrap();
        assert!(!renew_monthly_contract_tx(&tx, &second_id, second_end).unwrap());
        assert!(renew_monthly_contract_tx(&tx, &first_id, second_end).unwrap());
        let later_status: String = tx
            .query_row(
                "SELECT status FROM market_platform_contracts WHERE id = ?1",
                params![second_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(later_status, "suspended");
        assert_eq!(wallet_snapshot_tx(&tx, "buyer").unwrap().posted_units, 0);
        reconcile_wallet_tx(&tx, "buyer").unwrap();
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

impl crate::store::AppStore {
    pub async fn client_market_set_billing_suspended(
        &self,
        installation_id: &str,
        suspended: bool,
    ) -> Result<Option<String>, AppError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin Client billing suspension"))?;
        let tunnel = tx
            .query_row(
                "SELECT tunnel.subdomain, subscription.status
                 FROM client_market_subscriptions subscription
                 LEFT JOIN installation_client_tunnels tunnel
                   ON tunnel.installation_id = subscription.installation_id
                 WHERE subscription.installation_id = ?1",
                params![installation_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()
            .map_err(db_error("read Client tunnel for billing suspension"))?
            .unwrap_or_default();
        let terminal = tunnel
            .1
            .as_deref()
            .is_none_or(|status| matches!(status, "released" | "releasing" | "release_failed"));
        if terminal {
            tx.commit()
                .map_err(db_error("commit skipped Client billing control"))?;
            return Ok(None);
        }
        tx.execute(
            "UPDATE installation_client_tunnels SET enabled = ?2, updated_at = ?3
             WHERE installation_id = ?1",
            params![installation_id, i64::from(!suspended), now],
        )
        .map_err(db_error("update Client tunnel billing suspension"))?;
        tx.execute(
            "UPDATE client_market_subscriptions
             SET status = ?2, updated_at = ?3
             WHERE installation_id = ?1 AND status NOT IN ('released', 'releasing', 'release_failed')",
            params![
                installation_id,
                if suspended { "billing_suspended" } else { "active" },
                now,
            ],
        )
        .map_err(db_error("update Client subscription billing suspension"))?;
        tx.commit()
            .map_err(db_error("commit Client billing suspension"))?;
        Ok(tunnel.0)
    }
}
