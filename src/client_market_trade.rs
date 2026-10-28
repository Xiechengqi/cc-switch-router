use std::time::Duration as StdDuration;

use crate::db::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params, params_from_iter,
};
use axum::extract::{Path as AxumPath, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::ServerState;
use crate::error::AppError;
use crate::models::AuthSession;
use crate::store::AppStore;

pub const HOST_STATUS_RESERVED: &str = "reserved";
const QUOTE_TTL_SECS: i64 = 120;
const MAX_CREATE_COUNT: usize = 2;
const TRADE_RECONCILE_CYCLE_SECS: u64 = 20;
pub(crate) const MAX_FREE_DURATION_DAYS: u32 = 365;

const SUBSCRIPTION_ACTIVE: &str = "active";
const SUBSCRIPTION_RELEASING: &str = "releasing";
const SUBSCRIPTION_RELEASE_FAILED: &str = "release_failed";
const SUBSCRIPTION_RELEASED: &str = "released";
const MAX_PAYMENT_CONTACTS: usize = 10;
const MIN_PROVIDER_TERMINAL_AUTH_MINUTES: u32 = 5;
const MAX_PROVIDER_TERMINAL_AUTH_MINUTES: u32 = 24 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderContact {
    /// `wechat` | `telegram` | `custom`
    pub channel: String,
    pub handle: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderContactsView {
    pub provider_id: String,
    pub owner_email: String,
    #[serde(default)]
    pub contacts: Vec<ProviderContact>,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateProviderContactsRequest {
    contacts: Vec<ProviderContact>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSummary {
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_provider_id: Option<String>,
    pub rank_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank_position: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_score_bps: Option<i64>,
    pub owner_email: String,
    pub official: bool,
    pub joined_at: String,
    pub offer_stable_since: String,
    pub host_total: i64,
    pub idle_total: i64,
    pub allocated_total: i64,
    pub allocation_rate: f64,
    pub free_host_total: i64,
    pub free_allocated_total: i64,
    pub paid_host_total: i64,
    pub paid_allocated_total: i64,
    pub external_client_owner_total: i64,
    pub external_clients_over_3_days: i64,
    pub external_clients_over_30_days: i64,
    pub online_rate_30d: Option<f64>,
    pub anomalous_host_rate: f64,
    pub min_daily_rate_minor: Option<i64>,
    pub max_daily_rate_minor: Option<i64>,
    pub min_cycle_price_minor: Option<i64>,
    pub max_cycle_price_minor: Option<i64>,
    pub successful_allocations: i64,
    pub countries: Vec<ProviderCountrySummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCountrySummary {
    pub code: String,
    pub idle: i64,
    pub total: i64,
    pub free_idle: i64,
    pub free_total: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSupplyResponse {
    pub router_owner_email: Option<String>,
    pub official_provider_id: Option<String>,
    pub recommendation_mode: String,
    pub providers: Vec<ProviderSummary>,
}

fn legacy_provider_order(left: &ProviderSummary, right: &ProviderSummary) -> std::cmp::Ordering {
    right
        .official
        .cmp(&left.official)
        .then_with(|| neutral_provider_order(left, right))
}

fn neutral_provider_order(left: &ProviderSummary, right: &ProviderSummary) -> std::cmp::Ordering {
    right
        .external_clients_over_30_days
        .cmp(&left.external_clients_over_30_days)
        .then_with(|| {
            right
                .external_clients_over_3_days
                .cmp(&left.external_clients_over_3_days)
        })
        .then_with(|| {
            right
                .online_rate_30d
                .partial_cmp(&left.online_rate_30d)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| left.joined_at.cmp(&right.joined_at))
        .then_with(|| left.provider_id.cmp(&right.provider_id))
}

fn ranked_provider_order(left: &ProviderSummary, right: &ProviderSummary) -> std::cmp::Ordering {
    let left_ranked = left.rank_state == "ranked";
    let right_ranked = right.rank_state == "ranked";
    right_ranked
        .cmp(&left_ranked)
        .then_with(|| {
            left.rank_position
                .unwrap_or(i64::MAX)
                .cmp(&right.rank_position.unwrap_or(i64::MAX))
        })
        // Official identity is informative only. It retains the historical
        // position in off/shadow mode, but never breaks a ranked-mode tie.
        .then_with(|| neutral_provider_order(left, right))
}

fn apply_provider_recommendation_mode(
    response: &mut ProviderSupplyResponse,
    mode: crate::config::MarketProviderRecommendationMode,
) {
    response.recommendation_mode = mode.as_str().to_string();
    match mode {
        crate::config::MarketProviderRecommendationMode::Off => {}
        crate::config::MarketProviderRecommendationMode::Shadow => {
            let mut projected = response.providers.clone();
            projected.sort_by(ranked_provider_order);
            let moved = projected
                .iter()
                .zip(&response.providers)
                .filter(|(projected, current)| projected.provider_id != current.provider_id)
                .count();
            tracing::debug!(
                market = "client",
                providers = response.providers.len(),
                moved,
                "Market Provider recommendation shadow evaluated"
            );
        }
        crate::config::MarketProviderRecommendationMode::On => {
            response.providers.sort_by(ranked_provider_order);
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateHostOfferRequest {
    /// Legacy metered-daily prices are read-only. Kept in the request shape so
    /// stale clients receive a useful validation error instead of silently
    /// reinterpreting a daily price as a monthly one.
    #[serde(default)]
    pub daily_rate_minor: Option<i64>,
    #[serde(default)]
    pub cycle_price_minor: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub free_duration_days: Option<u32>,
    #[serde(default)]
    pub trial_hours: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostOfferView {
    pub host_id: String,
    pub daily_rate_minor: Option<i64>,
    pub pricing_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_price_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing_interval: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_duration_days: Option<u32>,
    pub trial_hours: Option<i64>,
    pub offer_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateQuoteRequest {
    #[serde(default)]
    pub provider_ids: Vec<String>,
    #[serde(default)]
    pub country_codes: Vec<String>,
    pub count: usize,
    pub host_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteItemView {
    pub id: String,
    pub host_id: String,
    pub provider_id: String,
    pub host_owner_email: String,
    pub country_code: Option<String>,
    pub hostname: Option<String>,
    pub ip: Option<String>,
    pub daily_rate_minor: Option<i64>,
    pub pricing_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_price_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing_interval: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_duration_days: Option<u32>,
    pub offer_revision: i64,
    pub trial_seconds_remaining: i64,
    pub trial_allowance_seconds: i64,
    pub trial_seconds_consumed: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationQuoteView {
    pub id: String,
    pub status: String,
    pub expires_at: String,
    pub items: Vec<QuoteItemView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_funding: Option<crate::market_wallet::WalletQuoteFunding>,
    pub funding_rail: String,
    pub termination_policy: Option<crate::client_market_termination::Policy>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitQuoteRequest {
    #[serde(default)]
    pub idempotency_key: Option<String>,
    pub items: Vec<CommitQuoteItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitClientBatchRequest {
    pub quote_id: String,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    pub items: Vec<CommitQuoteItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitQuoteItem {
    pub quote_item_id: String,
    pub offer_revision: i64,
    pub subdomain: String,
    pub password: String,
    #[serde(default)]
    pub auto_renew: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitQuoteResponse {
    pub batch_id: String,
    pub job_ids: Vec<String>,
    #[serde(skip)]
    pub replayed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientBatchView {
    pub id: String,
    pub quote_id: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub jobs: Vec<crate::client_market::JobView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RentalView {
    pub installation_id: String,
    pub host_id: String,
    pub provider_id: String,
    pub host_owner_email: String,
    pub client_owner_email: String,
    pub status: String,
    pub daily_rate_minor: Option<i64>,
    pub pricing_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_price_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing_interval: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_contract_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_contract: Option<crate::market_platform_contracts::WalletLifecycleView>,
    pub open_wallet_dispute: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_duration_days: Option<u32>,
    pub offer_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub contacts: Vec<ProviderContact>,
    pub is_client_owner: bool,
    pub can_release: bool,
    /// The renter may end a wedged release without changing the Host disposition.
    pub can_finalize_release: bool,
    /// The contract is ended (or ending) while its Host remains quarantined.
    pub host_cleanup_pending: bool,
    /// A stable, detail-free failure category; never expose raw SSH output here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_failure_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cleanup_probe_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_probe_attempts: Option<i64>,
    /// Latest pending/running cleanup job for this installation (page-refresh resume).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_cleanup_job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_terminal_authorized_until: Option<String>,
    pub provider_terminal_access_active: bool,
    pub can_manage_provider_terminal: bool,
    pub updated_at: String,
}

const HOST_USAGE_HISTORY_LIMIT: i64 = 10;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostUsageHistoryEntry {
    pub installation_id: String,
    pub client_owner_email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_subdomain: Option<String>,
    pub status: String,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_rate_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_price_minor: Option<i64>,
    pub pricing_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    pub charges_minor: i64,
    pub refunded_minor: i64,
    pub net_charges_minor: i64,
    #[serde(serialize_with = "crate::market_wallet::serialize_units")]
    pub service_units: i64,
    #[serde(serialize_with = "crate::market_wallet::serialize_units")]
    pub compensation_units: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProviderTerminalAuthorizationRequest {
    duration_minutes: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTerminalAuthorizationView {
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub updated_at: String,
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/account/provider-contacts",
            get(get_provider_contacts).put(update_provider_contacts),
        )
        .route("/v1/client-market/providers", get(get_provider_supply))
        .route("/v1/client-market/providers/:id", get(get_provider))
        .route(
            "/v1/client-market/creation-defaults",
            get(get_provider_supply),
        )
        .route(
            "/v1/client-market/creation-eligibility",
            get(get_creation_eligibility),
        )
        .route(
            "/v1/client-market/hosts/:id/offer",
            patch(update_host_offer),
        )
        .route("/v1/client-market/quotes", post(create_quote))
        .route("/v1/client-market/allocation-quotes", post(create_quote))
        .route(
            "/v1/client-market/allocation-quotes/:id",
            delete(cancel_quote),
        )
        .route("/v1/client-market/quotes/:id/cancel", post(cancel_quote))
        .route("/v1/client-market/quotes/:id/commit", post(commit_quote))
        .route("/v1/client-market/batches/:id", get(get_client_batch))
        .route("/v1/client-market/my-rentals", get(list_my_rentals))
        .route(
            "/v1/client-market/hosts/:id/usage-history",
            get(list_host_usage_history),
        )
        .route("/v1/client-market/clients", post(commit_client_batch))
        .route(
            "/v1/client-market/clients/:installation_id/rental",
            get(get_client_rental),
        )
        .route(
            "/v1/client-market/clients/:installation_id/provider-terminal-authorization",
            put(grant_provider_terminal_authorization)
                .delete(revoke_provider_terminal_authorization),
        )
        .route(
            "/v1/client-market/clients/:installation_id/finalize-release",
            post(finalize_failed_release),
        )
        .route(
            "/v1/admin/client-market/subscriptions/:installation_id/force-release",
            post(admin_force_release_subscription),
        )
}

async fn grant_provider_terminal_authorization(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
    Json(request): Json<UpdateProviderTerminalAuthorizationRequest>,
) -> Result<Json<ProviderTerminalAuthorizationView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_grant_provider_terminal_authorization(
                &installation_id,
                &session,
                request.duration_minutes,
            )
            .await?,
    ))
}

async fn revoke_provider_terminal_authorization(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<Json<ProviderTerminalAuthorizationView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_revoke_provider_terminal_authorization(&installation_id, &session)
            .await?,
    ))
}

async fn finalize_failed_release(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<Json<ForceReleaseResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let outcome = state
        .store
        .client_market_finalize_release_for_owner(&installation_id, &session)
        .await?;
    Ok(Json(outcome))
}

/// Operator escape hatch for a subscription wedged in teardown.
///
/// `release_failed` is otherwise terminal: nothing in the codebase transitions a
/// subscription out of it, while `ensure_creation_allowed_tx` treats it as a hard
/// block. A single failed remote cleanup — a Host rebooting mid-teardown is enough
/// — therefore locks the renter out of creating any future Client, permanently.
///
/// This deliberately touches only the subscription. Billing termination already
/// happens when cleanup starts. Host disposition is left to the existing reverify / auto-heal path, because forcing
/// a Host back to `idle` here could hand a renter a box that still has a live
/// `cc-switch-server` on it.
async fn admin_force_release_subscription(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<Json<ForceReleaseResponse>, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    let outcome = state
        .store
        .client_market_force_release_subscription(
            &installation_id,
            &session.user_id,
            &session.email,
        )
        .await?;
    Ok(Json(outcome))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForceReleaseResponse {
    pub installation_id: String,
    pub previous_status: String,
    pub status: String,
    /// Present when the Host still carries this installation, so the operator
    /// knows a reverify is still required before it re-enters the pool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_status: Option<String>,
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated owner session required".into()))
}

pub(crate) fn validate_offer(daily_rate_minor: Option<i64>) -> Result<Option<i64>, AppError> {
    match daily_rate_minor {
        None | Some(0) => Ok(None),
        Some(rate) if (1..=crate::market_common::MAX_DAILY_RATE_MINOR).contains(&rate) => {
            Ok(Some(rate))
        }
        _ => Err(AppError::BadRequest(
            "paid Hosts require dailyRateMinor between 1 and 100000000; omit it or use zero for free".into(),
        )),
    }
}

pub(crate) fn validate_monthly_offer(
    cycle_price_minor: Option<i64>,
) -> Result<Option<i64>, AppError> {
    match cycle_price_minor {
        None | Some(0) => Ok(None),
        Some(price)
            if (1..=crate::market_common::MAX_DAILY_RATE_MINOR).contains(&price) =>
        {
            Ok(Some(price))
        }
        _ => Err(AppError::BadRequest(
            "paid Hosts require cyclePriceMinor between 1 and 100000000; omit it or use zero for free"
                .into(),
        )),
    }
}

pub(crate) fn validate_free_duration_days(
    daily_rate_minor: Option<i64>,
    free_duration_days: Option<u32>,
) -> Result<Option<u32>, AppError> {
    if daily_rate_minor.is_some() {
        if free_duration_days.is_some() {
            return Err(AppError::BadRequest(
                "paid Hosts cannot set freeDurationDays".into(),
            ));
        }
        return Ok(None);
    }
    match free_duration_days {
        None => Ok(None),
        Some(days) if (1..=MAX_FREE_DURATION_DAYS).contains(&days) => Ok(Some(days)),
        Some(_) => Err(AppError::BadRequest(format!(
            "freeDurationDays must be between 1 and {MAX_FREE_DURATION_DAYS}, or null for permanent access"
        ))),
    }
}

pub(crate) fn normalize_offer_currency(
    daily_rate_minor: Option<i64>,
    currency: Option<String>,
) -> Result<Option<String>, AppError> {
    let normalized = currency
        .map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| !value.is_empty());
    match (daily_rate_minor, normalized.as_deref()) {
        (None, _) => Ok(None),
        (Some(_), None) => Ok(Some(crate::market_common::MARKET_CURRENCY.into())),
        (Some(_), Some(crate::market_common::MARKET_CURRENCY)) => Ok(normalized),
        (Some(_), Some(_)) => Err(AppError::BadRequest("currency must be USD".into())),
    }
}

pub(crate) fn require_paid_offer_setup(
    conn: &Connection,
    provider_id: &str,
    currency: &str,
) -> Result<(), AppError> {
    if currency != "USD" {
        return Err(AppError::Conflict(
            "wallet-funded Hosts must be priced in USD".into(),
        ));
    }
    crate::market_provider_finance::require_paid_offer_setup_tx(conn, provider_id)
}

/// Reattach hosts that belong to this owner email onto the canonical provider id.
/// Free hosts often keep a drifted `provider_id` because offer edits (which
/// heal identity) are never applied to them.
fn heal_hosts_onto_provider_tx(
    tx: &Transaction<'_>,
    provider_id: &str,
    owner_email: &str,
    email_keyed_provider_id: &str,
    now: &str,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE router_ssh_hosts
         SET provider_id = ?1, host_owner_email = ?2, updated_at = ?3
         WHERE provider_id = ?1
            OR provider_id = ?4
            OR lower(host_owner_email) = lower(?2)",
        params![provider_id, owner_email, now, email_keyed_provider_id],
    )
    .map_err(|error| AppError::Internal(format!("heal Host Provider bindings failed: {error}")))?;
    Ok(())
}

fn heal_all_provider_host_bindings_tx(tx: &Transaction<'_>, now: &str) -> Result<(), AppError> {
    // Prefer stable (non email:) profiles when multiple could match.
    //
    // `provider_id IS NULL` must be handled explicitly: SQLite treats
    // `NULL != '<id>'` as unknown, so a bare inequality skips every legacy Host
    // whose provider_id column was added nullable and never backfilled. Those
    // free idle Hosts then disappear from Create Client supply even
    // though their owner_email already matches a Provider profile.
    tx.execute(
        "UPDATE router_ssh_hosts
         SET provider_id = (
                SELECT p.provider_id
                FROM host_provider_profiles p
                WHERE lower(p.owner_email) = lower(router_ssh_hosts.host_owner_email)
                ORDER BY CASE WHEN p.provider_id LIKE 'email:%' THEN 1 ELSE 0 END, p.created_at
                LIMIT 1
             ),
             updated_at = ?1
         WHERE EXISTS (
                SELECT 1 FROM host_provider_profiles p
                WHERE lower(p.owner_email) = lower(router_ssh_hosts.host_owner_email)
             )
           AND (
                provider_id IS NULL
                OR provider_id != (
                    SELECT p.provider_id
                    FROM host_provider_profiles p
                    WHERE lower(p.owner_email) = lower(router_ssh_hosts.host_owner_email)
                    ORDER BY CASE WHEN p.provider_id LIKE 'email:%' THEN 1 ELSE 0 END, p.created_at
                    LIMIT 1
                )
             )",
        params![now],
    )
    .map_err(|error| {
        AppError::Internal(format!("heal all Host Provider bindings failed: {error}"))
    })?;
    Ok(())
}

/// Create Provider profiles for Host owners that have rows in `router_ssh_hosts`
/// but never got a `host_provider_profiles` entry (common for hosts added before
/// Provider identity, or with `provider_id` left NULL). Without this, Create Client
/// supply only lists owners who already have a profile — typically just the official one.
fn ensure_provider_profiles_for_orphan_hosts_tx(
    tx: &Transaction<'_>,
    now: &str,
) -> Result<(), AppError> {
    let mut statement = tx
        .prepare(
            "SELECT DISTINCT lower(trim(h.host_owner_email))
             FROM router_ssh_hosts h
             WHERE trim(h.host_owner_email) != ''
               AND (
                    h.provider_id IS NULL
                    OR NOT EXISTS (
                        SELECT 1 FROM host_provider_profiles p
                        WHERE p.provider_id = h.provider_id
                    )
               )
               AND NOT EXISTS (
                    SELECT 1 FROM host_provider_profiles p
                    WHERE lower(p.owner_email) = lower(h.host_owner_email)
               )",
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "prepare orphan Host Provider emails failed: {error}"
            ))
        })?;
    let emails = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| {
            AppError::Internal(format!("query orphan Host Provider emails failed: {error}"))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            AppError::Internal(format!("read orphan Host Provider emails failed: {error}"))
        })?;
    drop(statement);

    for email in emails {
        let user_id: Option<String> = tx
            .query_row(
                "SELECT id FROM users WHERE email_normalized = ?1",
                params![email],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("resolve orphan Host owner user failed: {error}"))
            })?;
        let mut provider_id = user_id.unwrap_or_else(|| format!("email:{email}"));
        if let Some(existing_email) = tx
            .query_row(
                "SELECT lower(owner_email) FROM host_provider_profiles WHERE provider_id = ?1",
                params![provider_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!(
                    "lookup orphan Provider id collision failed: {error}"
                ))
            })?
        {
            if existing_email != email {
                provider_id = format!("email:{email}");
            }
        }
        tx.execute(
            "INSERT INTO host_provider_profiles (provider_id, owner_email, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(provider_id) DO UPDATE SET
                owner_email = excluded.owner_email,
                updated_at = excluded.updated_at",
            params![provider_id, email, now],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "ensure orphan Host Provider profile failed: {error}"
            ))
        })?;
    }
    Ok(())
}

fn normalize_provider_contact(mut contact: ProviderContact) -> Result<ProviderContact, AppError> {
    contact.channel = contact.channel.trim().to_ascii_lowercase();
    contact.handle = contact.handle.trim().to_string();
    if !matches!(contact.channel.as_str(), "wechat" | "telegram" | "custom") {
        return Err(AppError::BadRequest(
            "contact channel must be wechat, telegram, or custom".into(),
        ));
    }
    if contact.handle.is_empty() || contact.handle.chars().count() > 200 {
        return Err(AppError::BadRequest(
            "contact handle is required and must be at most 200 characters".into(),
        ));
    }
    Ok(contact)
}

fn normalize_provider_contacts(
    contacts: Vec<ProviderContact>,
) -> Result<Vec<ProviderContact>, AppError> {
    if contacts.len() > MAX_PAYMENT_CONTACTS {
        return Err(AppError::BadRequest(
            "at most 10 contact entries are allowed".into(),
        ));
    }
    let mut output = Vec::with_capacity(contacts.len());
    for contact in contacts {
        let contact = normalize_provider_contact(contact)?;
        if output.iter().any(|existing: &ProviderContact| {
            existing.channel == contact.channel && existing.handle == contact.handle
        }) {
            return Err(AppError::BadRequest("duplicate contact entry".into()));
        }
        output.push(contact);
    }
    Ok(output)
}

async fn get_provider_contacts(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let profile = state
        .store
        .client_market_provider_contacts(&session.user_id, &session.email)
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({ "contacts": profile.contacts, "updatedAt": profile.updated_at })),
    ))
}

async fn update_provider_contacts(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateProviderContactsRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let contacts = normalize_provider_contacts(input.contacts)?;
    let profile = state
        .store
        .client_market_update_provider_contacts(&session, &contacts)
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({ "contacts": profile.contacts, "updatedAt": profile.updated_at })),
    ))
}

async fn get_provider_supply(
    State(state): State<ServerState>,
) -> Result<Json<ProviderSupplyResponse>, AppError> {
    let mode = state
        .dynamic
        .read()
        .await
        .market_provider_recommendation_mode;
    let mut response = state
        .store
        .client_market_provider_supply(state.config.official_provider_email())
        .await?;
    apply_provider_recommendation_mode(&mut response, mode);
    Ok(Json(response))
}

async fn get_provider(
    State(state): State<ServerState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ProviderSummary>, AppError> {
    let mode = state
        .dynamic
        .read()
        .await
        .market_provider_recommendation_mode;
    let mut supply = state
        .store
        .client_market_provider_supply(state.config.official_provider_email())
        .await?;
    apply_provider_recommendation_mode(&mut supply, mode);
    supply
        .providers
        .into_iter()
        .find(|provider| provider.provider_id == id)
        .map(Json)
        .ok_or_else(|| AppError::NotFound("Provider not found".into()))
}

async fn get_creation_eligibility(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .client_market_assert_creation_allowed(&session)
        .await?;
    Ok(Json(serde_json::json!({ "allowed": true })))
}

async fn update_host_offer(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(input): Json<UpdateHostOfferRequest>,
) -> Result<Json<HostOfferView>, AppError> {
    let session = require_session(&state, &headers).await?;
    if input.daily_rate_minor.is_some() {
        return Err(AppError::BadRequest(
            "dailyRateMinor is legacy and read-only; use cyclePriceMinor for a calendar-month offer"
                .into(),
        ));
    }
    let cycle_price_minor = validate_monthly_offer(input.cycle_price_minor)?;
    let currency = normalize_offer_currency(cycle_price_minor, input.currency)?;
    let free_duration_days =
        validate_free_duration_days(cycle_price_minor, input.free_duration_days)?;
    Ok(Json(
        state
            .store
            .client_market_update_host_monthly_offer(
                &id,
                &session,
                cycle_price_minor,
                currency,
                free_duration_days,
                input.trial_hours,
            )
            .await?,
    ))
}

async fn create_quote(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateQuoteRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let quote = state
        .store
        .client_market_create_quote(&session, input)
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
        Json(quote),
    ))
}

async fn commit_quote(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
    Json(input): Json<CommitQuoteRequest>,
) -> Result<Json<CommitQuoteResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        commit_quote_for_session(&state, &id, &session, input).await?,
    ))
}

async fn commit_client_batch(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CommitClientBatchRequest>,
) -> Result<Json<CommitQuoteResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        commit_quote_for_session(
            &state,
            &input.quote_id,
            &session,
            CommitQuoteRequest {
                idempotency_key: input.idempotency_key,
                items: input.items,
            },
        )
        .await?,
    ))
}

async fn commit_quote_for_session(
    state: &ServerState,
    id: &str,
    session: &AuthSession,
    input: CommitQuoteRequest,
) -> Result<CommitQuoteResponse, AppError> {
    let idempotency_key = input
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("quote:{id}"));
    if idempotency_key.len() > 128 || idempotency_key.chars().any(char::is_control) {
        return Err(AppError::BadRequest(
            "idempotencyKey must contain at most 128 non-control characters".into(),
        ));
    }
    if input.items.is_empty() || input.items.len() > MAX_CREATE_COUNT {
        return Err(AppError::BadRequest(
            "a quote commit requires one or two items".into(),
        ));
    }
    let mut prepared = Vec::with_capacity(input.items.len());
    for item in input.items {
        let subdomain = crate::namespace::normalize_client_subdomain(&item.subdomain)
            .map_err(|message| AppError::BadRequest(message.into()))?;
        validate_client_password(&item.password)?;
        prepared.push((
            item.quote_item_id,
            subdomain,
            item.password,
            item.offer_revision,
            item.auto_renew,
        ));
    }
    let request_fingerprint = commit_request_fingerprint_with_renewal(&prepared)?;
    let response = state
        .store
        .client_market_commit_quote_with_renewal_idempotent(
            id,
            session,
            &idempotency_key,
            &request_fingerprint,
            &prepared,
        )
        .await?;
    if response.replayed {
        return Ok(response);
    }
    {
        let mut secrets = state.client_market_job_secrets.lock().await;
        for (job_id, (_, _, password, _, _)) in response.job_ids.iter().zip(prepared.iter()) {
            secrets.insert_pending_password(job_id.clone(), password.clone());
        }
    }
    for job_id in response.job_ids.iter().cloned() {
        let runner_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) =
                crate::client_market::run_create_job(runner_state, job_id.clone()).await
            {
                tracing::error!(job_id = %job_id, error = %error, "quoted Client Market create job failed");
            }
        });
    }
    Ok(response)
}

async fn get_client_batch(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ClientBatchView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let mut batch = state.store.client_market_batch(&id, &session).await?;
    for job in &mut batch.jobs {
        job.client_url = job
            .subdomain
            .as_deref()
            .map(|subdomain| crate::client_market::client_public_url(&state.config, subdomain));
    }
    Ok(Json(batch))
}

async fn cancel_quote(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .client_market_cancel_quote(&id, &session)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn validate_client_password(password: &str) -> Result<(), AppError> {
    if password.chars().count() < 8
        || password.len() > 1_024
        || password.chars().any(char::is_control)
    {
        return Err(AppError::BadRequest(
            "password must be at least 8 characters, at most 1024 bytes, and contain no control characters".into(),
        ));
    }
    Ok(())
}

async fn list_my_rentals(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
        Json(
            state
                .store
                .client_market_list_rentals_for_viewer(&session)
                .await?,
        ),
    ))
}

async fn list_host_usage_history(
    State(state): State<ServerState>,
    AxumPath(host_id): AxumPath<String>,
) -> Result<Json<Vec<HostUsageHistoryEntry>>, AppError> {
    Ok(Json(
        state
            .store
            .client_market_host_usage_history(&host_id)
            .await?,
    ))
}

async fn get_client_rental(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
        Json(
            state
                .store
                .client_market_rental_for_viewer(&installation_id, &session)
                .await?,
        ),
    ))
}

impl AppStore {
    /// Record a Client Market audit event outside of an existing transaction.
    /// Used by surfaces that are not themselves transactional — notably the web
    /// terminal, whose root sessions previously left no durable trace at all.
    pub async fn client_market_record_audit_event(
        &self,
        installation_id: Option<&str>,
        host_id: Option<&str>,
        actor_user_id: Option<&str>,
        actor_email: Option<&str>,
        event_type: &str,
        detail: serde_json::Value,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Internal(format!("begin audit write failed: {error}")))?;
        insert_audit_tx(
            &tx,
            installation_id,
            host_id,
            actor_user_id,
            actor_email,
            event_type,
            detail,
            Utc::now(),
        )?;
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit audit write failed: {error}")))
    }

    /// Transition a wedged subscription to `released` so the renter's creation gate
    /// clears. Accepts `release_failed` (terminal — no other exit exists) and
    /// `releasing` (can stall if its cleanup job died before reporting).
    pub async fn client_market_force_release_subscription(
        &self,
        installation_id: &str,
        actor_user_id: &str,
        actor_email: &str,
    ) -> Result<ForceReleaseResponse, AppError> {
        self.client_market_finalize_stuck_subscription(
            installation_id,
            actor_user_id,
            actor_email,
            None,
            "subscription_force_released",
        )
        .await
    }

    pub async fn client_market_finalize_release_for_owner(
        &self,
        installation_id: &str,
        session: &AuthSession,
    ) -> Result<ForceReleaseResponse, AppError> {
        self.client_market_finalize_stuck_subscription(
            installation_id,
            &session.user_id,
            &session.email,
            Some(&session.user_id),
            "subscription_release_finalized",
        )
        .await
    }

    /// Bring pre-existing cleanup failures onto the split rental/Host
    /// lifecycle. The original cleanup already terminated billing and disabled
    /// the tunnel; this only removes the renter's creation lock. The Host stays
    /// bound and quarantined until a separate verified cleanup succeeds.
    pub async fn client_market_finalize_failed_rentals(&self) -> Result<usize, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin failed rental reconcile failed: {error}"))
            })?;
        let candidates = {
            let mut statement = tx
                .prepare(
                    "SELECT s.installation_id, s.host_id
                 FROM client_market_subscriptions s
                 JOIN router_ssh_hosts h ON h.id = s.host_id
                   AND h.installation_id = s.installation_id
                   AND h.status IN ('unreachable', 'abnormal')
                 JOIN installation_client_tunnels t ON t.installation_id = s.installation_id
                   AND t.enabled = 0
                 WHERE s.status = 'release_failed'
                   AND EXISTS (
                     SELECT 1 FROM provisioning_jobs j
                     WHERE j.installation_id = s.installation_id
                       AND j.host_id = s.host_id AND j.type = 'cleanup'
                       AND j.status = 'failed'
                   )
                   AND NOT EXISTS (
                     SELECT 1 FROM provisioning_jobs j
                     WHERE j.installation_id = s.installation_id
                       AND j.type = 'cleanup' AND j.status IN ('pending', 'running')
                   )
                 LIMIT 100",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare failed rental reconcile failed: {error}"))
                })?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| {
                    AppError::Internal(format!("query failed rentals failed: {error}"))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!("read failed rentals failed: {error}"))
                })?
        };
        let now = Utc::now();
        let mut finalized = 0;
        for (installation_id, host_id) in candidates {
            let changed = tx
                .execute(
                    "UPDATE client_market_subscriptions
                 SET status = 'released',
                     released_at = COALESCE(released_at, updated_at, ?2),
                     updated_at = ?2
                 WHERE installation_id = ?1 AND status = 'release_failed'",
                    params![installation_id, now.to_rfc3339()],
                )
                .map_err(|error| {
                    AppError::Internal(format!("finalize failed rental failed: {error}"))
                })?;
            if changed == 1 {
                insert_audit_tx(
                    &tx,
                    Some(&installation_id),
                    Some(&host_id),
                    None,
                    None,
                    "subscription_release_finalized",
                    serde_json::json!({
                        "reason": "cleanup_failure_reconcile",
                        "remoteCleanupBypassed": true,
                        "hostRemainsQuarantined": true,
                    }),
                    now,
                )?;
                finalized += 1;
            }
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit failed rental reconcile failed: {error}"))
        })?;
        Ok(finalized)
    }

    async fn client_market_finalize_stuck_subscription(
        &self,
        installation_id: &str,
        actor_user_id: &str,
        actor_email: &str,
        required_client_user_id: Option<&str>,
        event_type: &str,
    ) -> Result<ForceReleaseResponse, AppError> {
        let conn = self.conn.lock().await;
        let now = Utc::now();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| AppError::Internal(format!("begin force release failed: {error}")))?;

        let (previous_status, host_id, client_user_id): (String, Option<String>, String) = tx
            .query_row(
                "SELECT status, host_id, client_user_id
                 FROM client_market_subscriptions WHERE installation_id = ?1",
                params![installation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("load subscription failed: {error}")))?
            .ok_or_else(|| AppError::NotFound("subscription not found".into()))?;

        if required_client_user_id.is_some_and(|expected| expected != client_user_id) {
            return Err(AppError::Forbidden(
                "only the Client owner may finalize this failed release".into(),
            ));
        }

        if previous_status != SUBSCRIPTION_RELEASE_FAILED
            && previous_status != SUBSCRIPTION_RELEASING
        {
            return Err(AppError::Conflict(format!(
                "subscription is {previous_status}; only releasing or release_failed can be force released"
            )));
        }

        let active_cleanup_jobs: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM provisioning_jobs
                 WHERE installation_id = ?1 AND type = 'cleanup'
                   AND status IN ('pending', 'running')",
                params![installation_id],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "check active cleanup before release failed: {error}"
                ))
            })?;
        if active_cleanup_jobs > 0 {
            return Err(AppError::Conflict(
                "cleanup is still running; wait for it to finish before finalizing the release"
                    .into(),
            ));
        }

        // Compare-and-set on the status we actually read, so a concurrent cleanup
        // finishing normally wins instead of being silently overwritten.
        let changed = tx
            .execute(
                "UPDATE client_market_subscriptions
                 SET status = ?2, released_at = ?3, updated_at = ?3
                 WHERE installation_id = ?1 AND status = ?4",
                params![
                    installation_id,
                    SUBSCRIPTION_RELEASED,
                    now.to_rfc3339(),
                    previous_status
                ],
            )
            .map_err(|error| AppError::Internal(format!("force release failed: {error}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "subscription changed concurrently; retry".into(),
            ));
        }

        let host_status: Option<String> = match host_id.as_deref() {
            Some(host) => tx
                .query_row(
                    "SELECT status FROM router_ssh_hosts WHERE id = ?1",
                    params![host],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| AppError::Internal(format!("load host status failed: {error}")))?,
            None => None,
        };

        insert_audit_tx(
            &tx,
            Some(installation_id),
            host_id.as_deref(),
            Some(actor_user_id),
            Some(actor_email),
            event_type,
            serde_json::json!({
                "previousStatus": previous_status,
                "hostStatus": host_status,
                "remoteCleanupBypassed": true,
            }),
            now,
        )?;

        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit force release failed: {error}")))?;

        Ok(ForceReleaseResponse {
            installation_id: installation_id.to_string(),
            previous_status,
            status: SUBSCRIPTION_RELEASED.to_string(),
            host_id,
            host_status,
        })
    }

    pub async fn client_market_assert_creation_allowed(
        &self,
        session: &AuthSession,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin creation gate check failed: {error}"))
        })?;
        expire_quotes_tx(&tx, Utc::now())?;
        ensure_creation_allowed_tx(&tx, &session.user_id, &session.email)?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit creation gate check failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn client_market_bind_job_user(
        &self,
        job_id: &str,
        user_id: &str,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let changed = conn.execute(
            "UPDATE provisioning_jobs SET client_owner_user_id = ?2 WHERE id = ?1 AND type = 'create'",
            params![job_id, user_id],
        ).map_err(|error| AppError::Internal(format!("bind provisioning user failed: {error}")))?;
        if changed != 1 {
            return Err(AppError::NotFound("provisioning job not found".into()));
        }
        Ok(())
    }

    pub async fn client_market_ensure_provider(
        &self,
        user_id: &str,
        owner_email: &str,
    ) -> Result<String, AppError> {
        let email = normalize_email(owner_email)?;
        let email_key = format!("email:{email}");
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Internal(format!("begin Provider sync failed: {error}")))?;
        // owner_email is UNIQUE. Prefer re-keying a legacy email: profile onto the
        // stable user id instead of failing the upsert.
        let existing_by_email: Option<String> = tx
            .query_row(
                "SELECT provider_id FROM host_provider_profiles WHERE owner_email = ?1",
                params![email],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("lookup Provider by email failed: {error}"))
            })?;
        if let Some(existing_id) = existing_by_email {
            if existing_id != user_id {
                let target_exists: bool = tx
                    .query_row(
                        "SELECT 1 FROM host_provider_profiles WHERE provider_id = ?1",
                        params![user_id],
                        |_| Ok(true),
                    )
                    .optional()
                    .map_err(|error| {
                        AppError::Internal(format!("lookup target Provider failed: {error}"))
                    })?
                    .unwrap_or(false);
                if target_exists {
                    tx.execute(
                        "UPDATE router_ssh_hosts
                         SET provider_id = ?1, host_owner_email = ?2, updated_at = ?3
                         WHERE provider_id = ?4",
                        params![user_id, email, now, existing_id],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("merge legacy Host Provider failed: {error}"))
                    })?;
                    tx.execute(
                        "UPDATE client_market_subscriptions
                         SET provider_id = ?1, host_owner_email = ?2, updated_at = ?3
                         WHERE provider_id = ?4 AND status != 'released'",
                        params![user_id, email, now, existing_id],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!(
                            "merge legacy subscription Provider failed: {error}"
                        ))
                    })?;
                    tx.execute(
                        "DELETE FROM host_provider_profiles WHERE provider_id = ?1",
                        params![existing_id],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!(
                            "remove legacy Provider profile failed: {error}"
                        ))
                    })?;
                } else {
                    tx.execute(
                        "UPDATE host_provider_profiles
                         SET provider_id = ?1, updated_at = ?2
                         WHERE provider_id = ?3",
                        params![user_id, now, existing_id],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("re-key Provider profile failed: {error}"))
                    })?;
                    tx.execute(
                        "UPDATE router_ssh_hosts
                         SET provider_id = ?1, host_owner_email = ?2, updated_at = ?3
                         WHERE provider_id = ?4",
                        params![user_id, email, now, existing_id],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("re-key Host Provider failed: {error}"))
                    })?;
                    tx.execute(
                        "UPDATE client_market_subscriptions
                         SET provider_id = ?1, host_owner_email = ?2, updated_at = ?3
                         WHERE provider_id = ?4 AND status != 'released'",
                        params![user_id, email, now, existing_id],
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("re-key subscription Provider failed: {error}"))
                    })?;
                }
            } else {
                tx.execute(
                    "UPDATE host_provider_profiles
                     SET owner_email = ?2, updated_at = ?3
                     WHERE provider_id = ?1",
                    params![user_id, email, now],
                )
                .map_err(|error| {
                    AppError::Internal(format!("refresh Provider email failed: {error}"))
                })?;
            }
        } else {
            tx.execute(
                "INSERT INTO host_provider_profiles (provider_id, owner_email, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT(provider_id) DO UPDATE SET
                    owner_email = excluded.owner_email,
                    updated_at = excluded.updated_at",
                params![user_id, email, now],
            )
            .map_err(|error| {
                if error.to_string().contains("UNIQUE constraint failed") {
                    AppError::Conflict(
                        "this email is already bound to a different Provider identity".into(),
                    )
                } else {
                    AppError::Internal(format!("upsert Provider profile failed: {error}"))
                }
            })?;
        }
        // Pull free hosts (and any other drifted rows) whose owner email
        // matches this Provider back onto the stable provider_id. Paid hosts are
        // often healed via offer edits; free hosts otherwise stay orphaned and
        // disappear from Create Client idle capacity.
        heal_hosts_onto_provider_tx(&tx, user_id, &email, &email_key, &now)?;
        tx.execute(
            "UPDATE client_market_subscriptions SET host_owner_email = ?2, updated_at = ?3
             WHERE provider_id = ?1 AND status != 'released'",
            params![user_id, email, now],
        )
        .map_err(|error| {
            AppError::Internal(format!("sync subscription Provider email failed: {error}"))
        })?;
        match crate::market_provider_identity::ensure_market_provider_identity_tx(
            &tx,
            Some(user_id),
            &email,
            &now,
        )? {
            crate::market_provider_identity::MarketProviderIdentityResolution::Resolved(_) => {}
            crate::market_provider_identity::MarketProviderIdentityResolution::Conflict(
                conflict,
            ) => {
                drop(tx);
                crate::market_provider_identity::persist_market_provider_identity_conflict(
                    &conn, &conflict, &now,
                )?;
                return Err(AppError::Conflict(
                    "the Host Provider identity conflicts with an existing Market Provider".into(),
                ));
            }
        }
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit Provider sync failed: {error}")))?;
        Ok(user_id.to_string())
    }

    pub async fn client_market_provider_contacts(
        &self,
        user_id: &str,
        owner_email: &str,
    ) -> Result<ProviderContactsView, AppError> {
        let email = normalize_email(owner_email)?;
        let conn = self.conn.lock().await;
        let row: Option<(String, String, String)> = conn
            .query_row(
                "SELECT owner_email, contacts_json, updated_at
                 FROM market_provider_contacts WHERE user_id = ?1",
                params![user_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read provider contacts failed: {error}"))
            })?;
        if let Some((owner_email, contacts_json, updated_at)) = row {
            return Ok(ProviderContactsView {
                provider_id: user_id.to_string(),
                owner_email,
                contacts: serde_json::from_str(&contacts_json).unwrap_or_default(),
                updated_at,
            });
        }
        Ok(ProviderContactsView {
            provider_id: user_id.to_string(),
            owner_email: email,
            contacts: Vec::new(),
            updated_at: Utc::now().to_rfc3339(),
        })
    }

    pub async fn client_market_update_provider_contacts(
        &self,
        session: &AuthSession,
        contacts: &[ProviderContact],
    ) -> Result<ProviderContactsView, AppError> {
        let email = normalize_email(&session.email)?;
        let now = Utc::now().to_rfc3339();
        let contacts_json = serde_json::to_string(contacts).map_err(|error| {
            AppError::Internal(format!("encode Provider contacts failed: {error}"))
        })?;
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO market_provider_contacts (user_id, owner_email, contacts_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET
                owner_email = excluded.owner_email,
                contacts_json = excluded.contacts_json,
                updated_at = excluded.updated_at",
            params![session.user_id, email, contacts_json, now],
        )
        .map_err(|error| AppError::Internal(format!("save Provider contacts failed: {error}")))?;
        drop(conn);
        self.client_market_provider_contacts(&session.user_id, &session.email)
            .await
    }

    pub async fn client_market_provider_supply(
        &self,
        official_email: Option<&str>,
    ) -> Result<ProviderSupplyResponse, AppError> {
        let official_email = official_email.map(|value| value.trim().to_ascii_lowercase());
        let conn = self.conn.lock().await;
        {
            let tx = conn.transaction().map_err(|error| {
                AppError::Internal(format!("begin Provider supply heal failed: {error}"))
            })?;
            let now = Utc::now().to_rfc3339();
            ensure_provider_profiles_for_orphan_hosts_tx(&tx, &now)?;
            heal_all_provider_host_bindings_tx(&tx, &now)?;
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit Provider supply heal failed: {error}"))
            })?;
        }
        let rank_projections = crate::market_provider_rank::latest_rank_projections(&conn)?;
        let mut statement = conn
            .prepare(
                "SELECT p.provider_id, p.owner_email, p.created_at,
                        COUNT(h.id) AS host_total,
                        COALESCE(SUM(CASE WHEN h.status = 'idle' THEN 1 ELSE 0 END), 0) AS idle_total,
                        COALESCE(SUM(CASE WHEN h.status = 'allocated' THEN 1 ELSE 0 END), 0) AS allocated_total,
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.daily_rate_minor IS NULL AND h.cycle_price_minor IS NULL THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.daily_rate_minor IS NULL AND h.cycle_price_minor IS NULL AND h.status = 'allocated' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND (h.daily_rate_minor IS NOT NULL OR h.cycle_price_minor IS NOT NULL) THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND (h.daily_rate_minor IS NOT NULL OR h.cycle_price_minor IS NOT NULL) AND h.status = 'allocated' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.last_error IS NOT NULL AND TRIM(h.last_error) != '' THEN 1 ELSE 0 END), 0),
                        MIN(h.daily_rate_minor), MAX(h.daily_rate_minor),
                        MIN(h.cycle_price_minor), MAX(h.cycle_price_minor),
                        (SELECT COUNT(*) FROM provisioning_jobs j
                         WHERE j.host_id IN (SELECT id FROM router_ssh_hosts WHERE provider_id = p.provider_id)
                           AND j.type = 'create' AND j.status = 'succeeded') AS successful_allocations,
                        COALESCE(
                          (SELECT MAX(e.created_at) FROM client_market_audit_events e
                           WHERE e.event_type = 'host_offer_updated'
                             AND e.host_id IN (SELECT id FROM router_ssh_hosts WHERE provider_id = p.provider_id)),
                          MIN(h.created_at), p.created_at),
                        p.market_provider_id
                 FROM host_provider_profiles p
                 LEFT JOIN router_ssh_hosts h ON h.provider_id = p.provider_id
                 GROUP BY p.provider_id, p.owner_email, p.created_at
                 ORDER BY p.owner_email",
            )
            .map_err(|error| AppError::Internal(format!("prepare Provider supply failed: {error}")))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, String>(16)?,
                    row.get::<_, Option<String>>(17)?,
                ))
            })
            .map_err(|error| {
                AppError::Internal(format!("query Provider supply failed: {error}"))
            })?;
        let mut providers = Vec::new();
        for row in rows {
            let (
                provider_id,
                owner_email,
                joined_at,
                host_total,
                idle_total,
                allocated_total,
                free_host_total,
                free_allocated_total,
                paid_host_total,
                paid_allocated_total,
                anomalous_host_total,
                min_daily_rate_minor,
                max_daily_rate_minor,
                min_cycle_price_minor,
                max_cycle_price_minor,
                successful,
                offer_stable_since,
                market_provider_id,
            ) = row.map_err(|error| {
                AppError::Internal(format!("read Provider supply failed: {error}"))
            })?;
            let mut country_statement = conn
                .prepare(
                    "SELECT country_code,
                            SUM(CASE WHEN status = 'idle' THEN 1 ELSE 0 END),
                            COUNT(*),
                            SUM(CASE WHEN status = 'idle' AND daily_rate_minor IS NULL AND cycle_price_minor IS NULL THEN 1 ELSE 0 END),
                            SUM(CASE WHEN daily_rate_minor IS NULL AND cycle_price_minor IS NULL THEN 1 ELSE 0 END)
                     FROM router_ssh_hosts
                     WHERE provider_id = ?1 AND country_code IS NOT NULL
                     GROUP BY country_code ORDER BY country_code",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare Provider countries failed: {error}"))
                })?;
            let country_rows = country_statement
                .query_map(params![provider_id], |row| {
                    Ok(ProviderCountrySummary {
                        code: row.get(0)?,
                        idle: row.get(1)?,
                        total: row.get(2)?,
                        free_idle: row.get(3)?,
                        free_total: row.get(4)?,
                    })
                })
                .map_err(|error| {
                    AppError::Internal(format!("query Provider countries failed: {error}"))
                })?;
            let countries = country_rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!("read Provider countries failed: {error}"))
                })?;
            let (
                external_client_owner_total,
                external_clients_over_3_days,
                external_clients_over_30_days,
            ) = conn
                .query_row(
                    "SELECT COUNT(DISTINCT client_user_id),
                            COALESCE(SUM(CASE WHEN created_at <= ?2 THEN 1 ELSE 0 END), 0),
                            COALESCE(SUM(CASE WHEN created_at <= ?3 THEN 1 ELSE 0 END), 0)
                     FROM client_market_subscriptions
                     WHERE provider_id = ?1 AND client_user_id != provider_id
                       AND status != 'released'",
                    params![
                        provider_id,
                        (Utc::now() - Duration::days(3)).to_rfc3339(),
                        (Utc::now() - Duration::days(30)).to_rfc3339(),
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|error| {
                    AppError::Internal(format!(
                        "read Provider external rental observations failed: {error}"
                    ))
                })?;
            let (online_samples, observed_samples): (i64, i64) = conn
                .query_row(
                    "SELECT COALESCE(SUM(online_samples), 0), COALESCE(SUM(observed_samples), 0)
                     FROM host_provider_daily_stats
                     WHERE provider_id = ?1 AND stat_date >= ?2",
                    params![
                        provider_id,
                        (Utc::now() - Duration::days(30))
                            .format("%Y-%m-%d")
                            .to_string()
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| {
                    AppError::Internal(format!("read Provider uptime observations failed: {error}"))
                })?;
            let rank = market_provider_id
                .as_ref()
                .and_then(|id| rank_projections.get(id));
            providers.push(ProviderSummary {
                official: official_email.as_deref() == Some(owner_email.as_str()),
                provider_id,
                market_provider_id,
                rank_state: rank
                    .map(|projection| projection.rank_state.clone())
                    .unwrap_or_else(|| "unranked".into()),
                rank_position: rank.and_then(|projection| projection.rank_position),
                provider_score_bps: rank.and_then(|projection| projection.score_bps),
                owner_email,
                joined_at,
                offer_stable_since,
                host_total,
                idle_total,
                allocated_total,
                allocation_rate: ratio(allocated_total, host_total),
                free_host_total,
                free_allocated_total,
                paid_host_total,
                paid_allocated_total,
                external_client_owner_total,
                external_clients_over_3_days,
                external_clients_over_30_days,
                online_rate_30d: (observed_samples > 0)
                    .then(|| ratio(online_samples, observed_samples)),
                anomalous_host_rate: ratio(anomalous_host_total, host_total),
                min_daily_rate_minor,
                max_daily_rate_minor,
                min_cycle_price_minor,
                max_cycle_price_minor,
                successful_allocations: successful,
                countries,
            });
        }
        providers.sort_by(legacy_provider_order);
        let official_provider_id = providers
            .iter()
            .find(|provider| provider.official)
            .map(|provider| provider.provider_id.clone());
        Ok(ProviderSupplyResponse {
            router_owner_email: official_email,
            official_provider_id,
            recommendation_mode: "off".into(),
            providers,
        })
    }

    pub async fn client_market_record_provider_daily_stats(
        &self,
        now: DateTime<Utc>,
        client_stale_secs: i64,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let stat_date = now.format("%Y-%m-%d").to_string();
        let online_cutoff = (now - Duration::seconds(client_stale_secs.max(1))).to_rfc3339();
        conn.execute(
            "INSERT INTO host_provider_daily_stats
                (provider_id, stat_date, host_total, idle_total, allocated_total,
                 external_client_total, online_samples, observed_samples,
                 anomalous_host_samples, host_samples, updated_at)
             SELECT p.provider_id, ?1,
                    COUNT(DISTINCT h.id),
                    COUNT(DISTINCT CASE WHEN h.status = 'idle' THEN h.id END),
                    COUNT(DISTINCT CASE WHEN h.status = 'allocated' THEN h.id END),
                    COUNT(DISTINCT CASE WHEN s.status != 'released' AND s.client_user_id != s.provider_id
                                        THEN s.installation_id END),
                    COUNT(DISTINCT CASE WHEN s.status != 'released'
                                             AND i.client_activated_at IS NOT NULL
                                             AND ns.last_heartbeat_at >= ?2
                                        THEN s.installation_id END),
                    COUNT(DISTINCT CASE WHEN s.status != 'released' THEN s.installation_id END),
                    COUNT(DISTINCT CASE WHEN h.last_error IS NOT NULL AND TRIM(h.last_error) != ''
                                        THEN h.id END),
                    COUNT(DISTINCT h.id), ?3
             FROM host_provider_profiles p
             LEFT JOIN router_ssh_hosts h ON h.provider_id = p.provider_id
             LEFT JOIN client_market_subscriptions s ON s.provider_id = p.provider_id
             LEFT JOIN installations i ON i.id = s.installation_id
             LEFT JOIN installation_notification_state ns ON ns.installation_id = s.installation_id
             GROUP BY p.provider_id
             ON CONFLICT(provider_id, stat_date) DO UPDATE SET
                host_total = excluded.host_total,
                idle_total = excluded.idle_total,
                allocated_total = excluded.allocated_total,
                external_client_total = excluded.external_client_total,
                online_samples = host_provider_daily_stats.online_samples + excluded.online_samples,
                observed_samples = host_provider_daily_stats.observed_samples + excluded.observed_samples,
                anomalous_host_samples = host_provider_daily_stats.anomalous_host_samples + excluded.anomalous_host_samples,
                host_samples = host_provider_daily_stats.host_samples + excluded.host_samples,
                updated_at = excluded.updated_at",
            params![stat_date, online_cutoff, now.to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("record Provider daily observations failed: {error}")))?;
        Ok(())
    }

    pub async fn client_market_update_host_offer(
        &self,
        host_id: &str,
        session: &AuthSession,
        daily_rate_minor: Option<i64>,
        currency: Option<String>,
        free_duration_days: Option<u32>,
    ) -> Result<HostOfferView, AppError> {
        let daily_rate_minor = validate_offer(daily_rate_minor)?;
        let currency = normalize_offer_currency(daily_rate_minor, currency)?;
        let free_duration_days = validate_free_duration_days(daily_rate_minor, free_duration_days)?;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Internal(format!("begin offer update failed: {error}")))?;
        let host: Option<(
            Option<String>,
            String,
            Option<i64>,
            String,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<i64>,
            i64,
            String,
        )> = tx
            .query_row(
                "SELECT provider_id, host_owner_email, daily_rate_minor, pricing_model,
                        cycle_price_minor, billing_interval, currency,
                        free_duration_days, offer_revision, status
                 FROM router_ssh_hosts WHERE id = ?1",
                params![host_id],
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
                    ))
                },
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read host offer failed: {error}")))?;
        let (
            provider_id,
            _host_owner_email,
            old_price,
            old_pricing_model,
            old_cycle_price,
            old_billing_interval,
            old_currency,
            old_free_duration_days,
            old_revision,
            host_status,
        ) = host.ok_or_else(|| AppError::NotFound("host not found".into()))?;
        if !crate::client_market::session_is_host_owner(session, provider_id.as_deref()) {
            return Err(AppError::Forbidden(
                "not allowed to edit this Host offer".into(),
            ));
        }
        {
            let email = normalize_email(&session.email)?;
            tx.execute(
                "INSERT INTO host_provider_profiles
                    (provider_id, owner_email, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT(provider_id) DO UPDATE SET
                    owner_email = excluded.owner_email,
                    updated_at = excluded.updated_at",
                params![session.user_id, email, Utc::now().to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("sync offer Provider profile failed: {error}"))
            })?;
            // Heal legacy email-keyed / drifted provider_id onto the session user.
            if provider_id.as_deref() != Some(session.user_id.as_str()) {
                tx.execute(
                    "UPDATE router_ssh_hosts
                     SET provider_id = ?2, host_owner_email = ?3, updated_at = ?4
                     WHERE id = ?1",
                    params![host_id, session.user_id, email, Utc::now().to_rfc3339()],
                )
                .map_err(|error| {
                    AppError::Internal(format!("heal Host provider identity failed: {error}"))
                })?;
            }
        }
        let old_currency_norm = old_currency
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_uppercase());
        let pricing_model = if daily_rate_minor.is_some() {
            crate::market_common::PRICING_LEGACY_METERED_DAILY
        } else {
            crate::market_common::PRICING_FREE
        };
        if old_price == daily_rate_minor
            && old_pricing_model == pricing_model
            && old_cycle_price.is_none()
            && old_billing_interval.is_none()
            && old_currency_norm == currency
            && old_free_duration_days == free_duration_days.map(i64::from)
        {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit unchanged Host offer failed: {error}"))
            })?;
            return Ok(HostOfferView {
                host_id: host_id.to_string(),
                daily_rate_minor,
                pricing_model: if daily_rate_minor.is_some() {
                    crate::market_common::PRICING_LEGACY_METERED_DAILY.into()
                } else {
                    crate::market_common::PRICING_FREE.into()
                },
                cycle_price_minor: None,
                billing_interval: None,
                currency,
                free_duration_days,
                trial_hours: None,
                offer_revision: old_revision,
            });
        }
        if matches!(host_status.as_str(), "reserved" | "locked") {
            return Err(AppError::Conflict(
                "the Host offer is reserved or locked by an active order; retry after it completes"
                    .into(),
            ));
        }
        // Paid publication requires a verified, claimed Provider identity.
        if daily_rate_minor.is_some() {
            require_paid_offer_setup(
                &tx,
                &session.user_id,
                currency
                    .as_deref()
                    .ok_or_else(|| AppError::Internal("paid Host currency is missing".into()))?,
            )?;
        }
        let revision = old_revision + 1;
        tx.execute(
            "UPDATE router_ssh_hosts
             SET daily_rate_minor = ?2, pricing_model = ?3,
                 cycle_price_minor = NULL, billing_interval = NULL, currency = ?4,
                 free_duration_days = ?5, offer_revision = ?6, updated_at = ?7
             WHERE id = ?1",
            params![
                host_id,
                daily_rate_minor,
                pricing_model,
                currency,
                free_duration_days.map(i64::from),
                revision,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|error| AppError::Internal(format!("update Host offer failed: {error}")))?;
        insert_audit_tx(
            &tx,
            None,
            Some(host_id),
            Some(&session.user_id),
            Some(&session.email),
            "host_offer_updated",
            serde_json::json!({
                "oldDailyRateMinor": old_price,
                "oldPricingModel": old_pricing_model,
                "oldCyclePriceMinor": old_cycle_price,
                "oldBillingInterval": old_billing_interval,
                "oldCurrency": old_currency,
                "oldFreeDurationDays": old_free_duration_days,
                "dailyRateMinor": daily_rate_minor,
                "pricingModel": pricing_model,
                "cyclePriceMinor": null,
                "billingInterval": null,
                "currency": currency,
                "freeDurationDays": free_duration_days,
                "offerRevision": revision,
            }),
            Utc::now(),
        )?;
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit Host offer failed: {error}")))?;
        Ok(HostOfferView {
            host_id: host_id.to_string(),
            daily_rate_minor,
            pricing_model: if daily_rate_minor.is_some() {
                crate::market_common::PRICING_LEGACY_METERED_DAILY.into()
            } else {
                crate::market_common::PRICING_FREE.into()
            },
            cycle_price_minor: None,
            billing_interval: None,
            currency,
            free_duration_days,
            trial_hours: None,
            offer_revision: revision,
        })
    }

    pub async fn client_market_update_host_monthly_offer(
        &self,
        host_id: &str,
        session: &AuthSession,
        cycle_price_minor: Option<i64>,
        currency: Option<String>,
        free_duration_days: Option<u32>,
        trial_hours: Option<i64>,
    ) -> Result<HostOfferView, AppError> {
        let trial_hours = crate::market_common::validate_client_trial_hours(trial_hours)?;
        let cycle_price_minor = validate_monthly_offer(cycle_price_minor)?;
        let currency = normalize_offer_currency(cycle_price_minor, currency)?;
        let free_duration_days =
            validate_free_duration_days(cycle_price_minor, free_duration_days)?;
        let pricing_model = if cycle_price_minor.is_some() {
            crate::market_common::PRICING_PREPAID_CALENDAR_MONTH
        } else {
            crate::market_common::PRICING_FREE
        };
        let billing_interval = cycle_price_minor
            .map(|_| crate::market_common::BILLING_INTERVAL_CALENDAR_MONTH.to_string());
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin monthly offer update failed: {error}"))
            })?;
        let host = tx
            .query_row(
                "SELECT provider_id, daily_rate_minor, pricing_model, cycle_price_minor,
                        billing_interval, currency, free_duration_days,
                        offer_revision, status, trial_hours
                 FROM router_ssh_hosts WHERE id = ?1",
                params![host_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, Option<i64>>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read monthly Host offer failed: {error}"))
            })?
            .ok_or_else(|| AppError::NotFound("host not found".into()))?;
        if !crate::client_market::session_is_host_owner(session, host.0.as_deref()) {
            return Err(AppError::Forbidden(
                "not allowed to edit this Host offer".into(),
            ));
        }
        let email = normalize_email(&session.email)?;
        tx.execute(
            "INSERT INTO host_provider_profiles
                (provider_id, owner_email, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(provider_id) DO UPDATE SET
                owner_email = excluded.owner_email, updated_at = excluded.updated_at",
            params![session.user_id, email, Utc::now().to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("sync monthly offer Provider failed: {error}"))
        })?;
        if host.0.as_deref() != Some(session.user_id.as_str()) {
            tx.execute(
                "UPDATE router_ssh_hosts
                 SET provider_id = ?2, host_owner_email = ?3, updated_at = ?4
                 WHERE id = ?1",
                params![host_id, session.user_id, email, Utc::now().to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("heal monthly offer Provider failed: {error}"))
            })?;
        }
        let normalized_old_currency = host
            .5
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_uppercase);
        if host.1.is_none()
            && host.2 == pricing_model
            && host.3 == cycle_price_minor
            && host.4 == billing_interval
            && normalized_old_currency == currency
            && host.6 == free_duration_days.map(i64::from)
            && host.9 == trial_hours
        {
            tx.commit().map_err(|error| {
                AppError::Internal(format!(
                    "commit unchanged monthly Host offer failed: {error}"
                ))
            })?;
            return Ok(HostOfferView {
                host_id: host_id.to_string(),
                daily_rate_minor: None,
                pricing_model: pricing_model.into(),
                cycle_price_minor,
                billing_interval,
                currency,
                free_duration_days,
                trial_hours,
                offer_revision: host.7,
            });
        }
        if matches!(host.8.as_str(), "reserved" | "locked") {
            return Err(AppError::Conflict(
                "the Host offer is reserved or locked by an active order; retry after it completes"
                    .into(),
            ));
        }
        if cycle_price_minor.is_some() {
            require_paid_offer_setup(
                &tx,
                &session.user_id,
                currency
                    .as_deref()
                    .ok_or_else(|| AppError::Internal("paid Host currency is missing".into()))?,
            )?;
        }
        let revision = host.7 + 1;
        let now = Utc::now();
        tx.execute(
            "UPDATE router_ssh_hosts
             SET daily_rate_minor = NULL, pricing_model = ?2,
                 cycle_price_minor = ?3, billing_interval = ?4,
                 currency = ?5, free_duration_days = ?6,
                 offer_revision = ?7, updated_at = ?8, trial_hours = ?9
             WHERE id = ?1",
            params![
                host_id,
                pricing_model,
                cycle_price_minor,
                billing_interval,
                currency,
                free_duration_days.map(i64::from),
                revision,
                now.to_rfc3339(),
                trial_hours,
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("update monthly Host offer failed: {error}"))
        })?;
        insert_audit_tx(
            &tx,
            None,
            Some(host_id),
            Some(&session.user_id),
            Some(&session.email),
            "host_offer_updated",
            serde_json::json!({
                "oldDailyRateMinor": host.1,
                "oldPricingModel": host.2,
                "oldCyclePriceMinor": host.3,
                "oldBillingInterval": host.4,
                "oldCurrency": host.5,
                "oldFreeDurationDays": host.6,
                "oldTrialHours": host.9,
                "dailyRateMinor": null,
                "pricingModel": pricing_model,
                "cyclePriceMinor": cycle_price_minor,
                "billingInterval": billing_interval,
                "currency": currency,
                "freeDurationDays": free_duration_days,
                "trialHours": trial_hours,
                "offerRevision": revision,
            }),
            now,
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit monthly Host offer failed: {error}"))
        })?;
        Ok(HostOfferView {
            host_id: host_id.to_string(),
            daily_rate_minor: None,
            pricing_model: pricing_model.into(),
            cycle_price_minor,
            billing_interval,
            currency,
            free_duration_days,
            trial_hours,
            offer_revision: revision,
        })
    }

    pub async fn client_market_create_quote(
        &self,
        session: &AuthSession,
        input: CreateQuoteRequest,
    ) -> Result<AllocationQuoteView, AppError> {
        if input.count == 0 || input.count > MAX_CREATE_COUNT {
            return Err(AppError::BadRequest("count must be 1 or 2".into()));
        }
        if input.host_id.is_some() && input.count != 1 {
            return Err(AppError::BadRequest(
                "a fixed Host quote must have count=1".into(),
            ));
        }
        let mut provider_ids = input
            .provider_ids
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        provider_ids.sort();
        provider_ids.dedup();
        let mut countries = input
            .country_codes
            .into_iter()
            .map(|value| value.trim().to_ascii_uppercase())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        countries.sort();
        countries.dedup();
        if input.host_id.is_none()
            && (provider_ids.is_empty()
                || countries.is_empty()
                || provider_ids.len() > 100
                || countries.len() > 100)
        {
            return Err(AppError::BadRequest(
                "providerIds and countryCodes must each contain 1 to 100 values".into(),
            ));
        }
        if countries
            .iter()
            .any(|code| code.len() != 2 || !code.bytes().all(|byte| byte.is_ascii_uppercase()))
        {
            return Err(AppError::BadRequest(
                "country codes must be two ASCII letters".into(),
            ));
        }
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin allocation quote failed: {error}"))
        })?;
        expire_quotes_tx(&tx, now)?;
        let now_rfc = now.to_rfc3339();
        ensure_provider_profiles_for_orphan_hosts_tx(&tx, &now_rfc)?;
        heal_all_provider_host_bindings_tx(&tx, &now_rfc)?;
        ensure_creation_allowed_tx(&tx, &session.user_id, &session.email)?;
        let active_quote: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM client_market_allocation_quotes
                 WHERE client_user_id = ?1 AND status = 'active' AND expires_at > ?2",
                params![session.user_id, now.to_rfc3339()],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Internal(format!("count active quotes failed: {error}")))?;
        if active_quote > 0 {
            return Err(AppError::Conflict(
                "finish or wait for the current allocation quote to expire before creating another"
                    .into(),
            ));
        }
        // Providers may allocate their own Hosts (self-host then self-use is a
        // supported workflow). Unified access policy is checked below.
        let mut candidates = if let Some(host_id) = input.host_id.as_deref() {
            let candidate = tx
                .query_row(
                    "SELECT h.id, h.provider_id, h.host_owner_email, h.country_code, h.hostname,
                            h.ip, h.daily_rate_minor, h.pricing_model,
                            h.cycle_price_minor, h.billing_interval,
                            CASE WHEN h.daily_rate_minor IS NULL AND h.cycle_price_minor IS NULL THEN NULL
                                 ELSE COALESCE(NULLIF(TRIM(h.currency), ''), 'USD') END,
                            h.free_duration_days, h.offer_revision, h.funding_rail,
                            COALESCE(h.trial_hours, (SELECT client_hours FROM market_trial_policy WHERE id = 1))
                     FROM router_ssh_hosts h
                     WHERE h.id = ?1 AND h.status = 'idle' AND h.provider_id IS NOT NULL",
                    params![host_id],
                    map_quote_candidate,
                )
                .optional()
                .map_err(|error| AppError::Internal(format!("select fixed Host failed: {error}")))?
                .ok_or_else(|| {
                    AppError::Conflict(
                        "the selected Host is no longer idle or this Provider does not accept this account".into(),
                    )
                })?;
            vec![candidate]
        } else {
            let provider_vars = sql_vars(provider_ids.len());
            let country_vars = sql_vars(countries.len());
            // Fetch the full idle free pool, then shuffle in Rust. SQLite
            // `ORDER BY RANDOM() LIMIT n` was observed to keep returning the same
            // Host on live Routers; application-side sampling is explicit and testable.
            let sql = format!(
                "SELECT h.id, h.provider_id, h.host_owner_email, h.country_code, h.hostname,
                        h.ip, h.daily_rate_minor, h.pricing_model,
                        h.cycle_price_minor, h.billing_interval,
                        CASE WHEN h.daily_rate_minor IS NULL AND h.cycle_price_minor IS NULL THEN NULL
                             ELSE COALESCE(NULLIF(TRIM(h.currency), ''), 'USD') END,
                        h.free_duration_days, h.offer_revision, h.funding_rail,
                        COALESCE(h.trial_hours, (SELECT client_hours FROM market_trial_policy WHERE id = 1))
                 FROM router_ssh_hosts h
                 WHERE h.status = 'idle'
                   AND h.daily_rate_minor IS NULL AND h.cycle_price_minor IS NULL
                   AND h.provider_id IN ({provider_vars})
                   AND h.country_code IN ({country_vars})"
            );
            let mut values = provider_ids.clone();
            values.extend(countries.clone());
            let mut statement = tx.prepare(&sql).map_err(|error| {
                AppError::Internal(format!("prepare random Host quote failed: {error}"))
            })?;
            let rows = statement
                .query_map(params_from_iter(values.iter()), map_quote_candidate)
                .map_err(|error| {
                    AppError::Internal(format!("query random Host quote failed: {error}"))
                })?;
            let pool = rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
                AppError::Internal(format!("read random Host quote failed: {error}"))
            })?;
            drop(statement);
            let mut pool = pool
                .into_iter()
                .filter_map(|candidate| {
                    let access_allowed = crate::market_access::product_access_allowed_tx(
                        &tx,
                        &candidate.provider_id,
                        &session.user_id,
                        &session.email,
                        crate::market_access::PRODUCT_CLIENT_HOST,
                        crate::market_access::PRICING_FREE,
                    );
                    match access_allowed {
                        Ok(true) => {
                            let free_available = candidate
                                .free_duration_days
                                .filter(|_| candidate.provider_id != session.user_id)
                                .map(|days| {
                                    crate::market_common::free_usage_available_tx(
                                        &tx,
                                        &session.user_id,
                                        &candidate.provider_id,
                                        crate::market_access::PRODUCT_CLIENT_HOST,
                                        &candidate.host_id,
                                        days,
                                        &now_rfc,
                                    )
                                })
                                .transpose();
                            match free_available {
                                Ok(Some(true) | None) => Some(Ok(candidate)),
                                Ok(Some(false)) => None,
                                Err(error) => Some(Err(error)),
                            }
                        }
                        Ok(false) => None,
                        Err(error) => Some(Err(error)),
                    }
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            if pool.len() < input.count {
                return Err(AppError::ServiceUnavailable(
                    "not enough idle free Hosts match the selected Providers and regions".into(),
                ));
            }
            pool.shuffle(&mut rand::thread_rng());
            pool.truncate(input.count);
            pool
        };
        if candidates.len() != input.count {
            return Err(AppError::ServiceUnavailable(
                "not enough idle free Hosts match the selected Providers and regions".into(),
            ));
        }
        // Official recharge mode never changes the rail of a new market quote.
        for candidate in &mut candidates {
            if candidate.provider_id == session.user_id {
                candidate.daily_rate_minor = None;
                candidate.pricing_model = crate::market_common::PRICING_FREE.into();
                candidate.cycle_price_minor = None;
                candidate.billing_interval = None;
                candidate.currency = None;
            }
            let price_minor = candidate.cycle_price_minor.or(candidate.daily_rate_minor);
            if price_minor.is_some() {
                crate::market_payments::require_current_offer_rail(
                    &candidate.funding_rail,
                    "Client Host",
                )?;
                if candidate.currency.as_deref() != Some("USD") {
                    return Err(AppError::Conflict(
                        "wallet-funded Hosts must be priced in USD".into(),
                    ));
                }
                let trial = crate::market_client_trials::state_tx(
                    &tx,
                    &session.user_id,
                    &candidate.provider_id,
                    &candidate.host_id,
                    candidate.trial_hours * 3_600,
                )?;
                candidate.wallet_trial_seconds = trial.remaining_seconds;
                candidate.wallet_trial_consumed_seconds = trial.consumed_seconds;
                candidate.wallet_trial_epoch = trial.epoch;
            } else if candidate.provider_id != session.user_id
                && let Some(days) = candidate.free_duration_days
                && !crate::market_common::free_usage_available_tx(
                    &tx,
                    &session.user_id,
                    &candidate.provider_id,
                    crate::market_access::PRODUCT_CLIENT_HOST,
                    &candidate.host_id,
                    days,
                    &now_rfc,
                )?
            {
                return Err(AppError::Conflict(
                    "the free experience for this Provider has already been used".into(),
                ));
            }
            crate::market_access::ensure_product_access_tx(
                &tx,
                &candidate.provider_id,
                &session.user_id,
                &session.email,
                crate::market_access::PRODUCT_CLIENT_HOST,
                crate::market_access::pricing_kind_for_rate(price_minor),
            )?;
        }
        let mut wallet_price_minor = 0_i64;
        let mut wallet_paid_items = false;
        for candidate in &candidates {
            if let Some(price) = candidate.cycle_price_minor.or(candidate.daily_rate_minor) {
                wallet_paid_items = true;
                wallet_price_minor = wallet_price_minor.checked_add(price).ok_or_else(|| {
                    AppError::BadRequest("quoted wallet price is too large".into())
                })?;
            }
        }
        let wallet_funding = if wallet_paid_items {
            Some(crate::market_wallet::wallet_quote_funding_tx(
                &tx,
                &session.user_id,
                wallet_price_minor,
                0,
                &now_rfc,
            )?)
        } else {
            None
        };
        let funding_rail = "platform_wallet";
        let quote_id = Uuid::new_v4().to_string();
        let expires_at = now + Duration::seconds(QUOTE_TTL_SECS);
        let client_owner_email = normalize_email(&session.email)?;
        tx.execute(
            "INSERT INTO client_market_allocation_quotes
                (id, client_user_id, client_owner_email, status, fixed_host_id,
                 expires_at, created_at, updated_at, funding_rail)
             VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?6, ?6, ?7)",
            params![
                quote_id,
                session.user_id,
                client_owner_email,
                input.host_id,
                expires_at.to_rfc3339(),
                now.to_rfc3339(),
                funding_rail,
            ],
        )
        .map_err(|error| AppError::Internal(format!("insert allocation quote failed: {error}")))?;
        let termination_policy = if funding_rail == "platform_wallet" {
            crate::client_market_termination::freeze_quote_tx(&tx, &quote_id)?;
            Some(crate::client_market_termination::policy_tx(&tx)?)
        } else {
            None
        };
        let mut items = Vec::with_capacity(candidates.len());
        for (position, candidate) in candidates.into_iter().enumerate() {
            if funding_rail == "platform_wallet" {
                crate::market_commission::freeze_quote_tx(
                    &tx,
                    &quote_id,
                    &candidate.provider_id,
                    &now_rfc,
                )?;
            }
            let changed = tx
                .execute(
                    "UPDATE router_ssh_hosts SET status = ?2, updated_at = ?3
                     WHERE id = ?1 AND status = 'idle'",
                    params![candidate.host_id, HOST_STATUS_RESERVED, now.to_rfc3339()],
                )
                .map_err(|error| {
                    AppError::Internal(format!("reserve quoted Host failed: {error}"))
                })?;
            if changed != 1 {
                return Err(AppError::Conflict(
                    "Host reservation raced; request a new quote".into(),
                ));
            }
            let item_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO client_market_allocation_quote_items
                    (id, quote_id, position, host_id, provider_id, host_owner_email,
                     country_code, hostname, daily_rate_minor, currency,
                     free_duration_days, offer_revision, pricing_model,
                     cycle_price_minor, billing_interval, wallet_trial_seconds,
                     wallet_trial_allowance_seconds, wallet_trial_epoch, wallet_trial_consumed_seconds)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                         ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                params![
                    item_id,
                    quote_id,
                    position as i64,
                    candidate.host_id,
                    candidate.provider_id,
                    candidate.host_owner_email,
                    candidate.country_code,
                    candidate.hostname,
                    candidate.daily_rate_minor,
                    candidate.currency,
                    candidate.free_duration_days.map(i64::from),
                    candidate.offer_revision,
                    candidate.pricing_model,
                    candidate.cycle_price_minor,
                    candidate.billing_interval,
                    candidate.wallet_trial_seconds,
                    candidate.trial_hours * 3_600,
                    candidate.wallet_trial_epoch,
                    candidate.wallet_trial_consumed_seconds,
                ],
            )
            .map_err(|error| AppError::Internal(format!("insert quote item failed: {error}")))?;
            items.push(QuoteItemView {
                id: item_id,
                host_id: candidate.host_id,
                provider_id: candidate.provider_id,
                host_owner_email: candidate.host_owner_email,
                country_code: candidate.country_code,
                hostname: candidate.hostname,
                ip: candidate.ip,
                daily_rate_minor: candidate.daily_rate_minor,
                pricing_model: candidate.pricing_model,
                cycle_price_minor: candidate.cycle_price_minor,
                billing_interval: candidate.billing_interval,
                currency: candidate.currency,
                free_duration_days: candidate.free_duration_days,
                offer_revision: candidate.offer_revision,
                trial_seconds_remaining: candidate.wallet_trial_seconds,
                trial_allowance_seconds: candidate.trial_hours * 3_600,
                trial_seconds_consumed: candidate.wallet_trial_consumed_seconds,
            });
        }
        insert_audit_tx(
            &tx,
            None,
            None,
            Some(&session.user_id),
            Some(&session.email),
            "allocation_quote_created",
            serde_json::json!({ "quoteId": quote_id, "count": items.len(), "fixedHostId": input.host_id }),
            now,
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit allocation quote failed: {error}"))
        })?;
        Ok(AllocationQuoteView {
            id: quote_id,
            status: "active".into(),
            expires_at: expires_at.to_rfc3339(),
            items,
            wallet_funding,
            funding_rail: funding_rail.into(),
            termination_policy,
        })
    }

    pub async fn client_market_commit_quote(
        &self,
        quote_id: &str,
        session: &AuthSession,
        prepared: &[(String, String, String, i64)],
    ) -> Result<CommitQuoteResponse, AppError> {
        let request_fingerprint = commit_request_fingerprint(prepared)?;
        self.client_market_commit_quote_idempotent(
            quote_id,
            session,
            &format!("quote:{quote_id}"),
            &request_fingerprint,
            prepared,
        )
        .await
    }

    pub async fn client_market_commit_quote_idempotent(
        &self,
        quote_id: &str,
        session: &AuthSession,
        idempotency_key: &str,
        request_fingerprint: &str,
        prepared: &[(String, String, String, i64)],
    ) -> Result<CommitQuoteResponse, AppError> {
        let prepared = prepared
            .iter()
            .map(|(item, subdomain, password, revision)| {
                (
                    item.clone(),
                    subdomain.clone(),
                    password.clone(),
                    *revision,
                    false,
                )
            })
            .collect::<Vec<_>>();
        self.client_market_commit_quote_with_renewal_idempotent(
            quote_id,
            session,
            idempotency_key,
            request_fingerprint,
            &prepared,
        )
        .await
    }

    pub async fn client_market_commit_quote_with_renewal_idempotent(
        &self,
        quote_id: &str,
        session: &AuthSession,
        idempotency_key: &str,
        request_fingerprint: &str,
        prepared: &[(String, String, String, i64, bool)],
    ) -> Result<CommitQuoteResponse, AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| AppError::Internal(format!("begin quote commit failed: {error}")))?;
        expire_quotes_tx(&tx, now)?;
        let quote: Option<(String, String, String, String)> = tx
            .query_row(
                "SELECT client_user_id, status, expires_at, funding_rail
                 FROM client_market_allocation_quotes WHERE id = ?1",
                params![quote_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read quote failed: {error}")))?;
        let (owner_user_id, status, expires_at, funding_rail) =
            quote.ok_or_else(|| AppError::NotFound("allocation quote not found".into()))?;
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "allocation quote belongs to another account".into(),
            ));
        }
        let existing_batch = tx
            .query_row(
                "SELECT id, quote_id, request_fingerprint
                 FROM client_market_batches
                 WHERE quote_id = ?1 OR (client_user_id = ?2 AND idempotency_key = ?3)
                 ORDER BY CASE WHEN quote_id = ?1 THEN 0 ELSE 1 END LIMIT 1",
                params![quote_id, session.user_id, idempotency_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read idempotent Client batch failed: {error}"))
            })?;
        if let Some((batch_id, stored_quote_id, stored_fingerprint)) = existing_batch {
            if stored_quote_id != quote_id || stored_fingerprint != request_fingerprint {
                return Err(AppError::Conflict(
                    "idempotency key was already used for a different Client batch request".into(),
                ));
            }
            let job_ids = tx
                .prepare(
                    "SELECT id FROM provisioning_jobs
                     WHERE batch_id = ?1 ORDER BY created_at, id",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map(params![batch_id], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()
                })
                .map_err(|error| {
                    AppError::Internal(format!("read idempotent Client batch jobs failed: {error}"))
                })?;
            tx.commit().map_err(|error| {
                AppError::Internal(format!(
                    "commit idempotent Client batch read failed: {error}"
                ))
            })?;
            return Ok(CommitQuoteResponse {
                batch_id,
                job_ids,
                replayed: true,
            });
        }
        if status != "active" || parse_time(&expires_at)? <= now {
            return Err(AppError::Gone(
                "allocation quote expired; request a new quote".into(),
            ));
        }
        if !matches!(funding_rail.as_str(), "legacy_provider" | "platform_wallet") {
            return Err(AppError::Internal(
                "stored Client funding rail is invalid".into(),
            ));
        }
        let paid_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM client_market_allocation_quote_items
             WHERE quote_id = ?1 AND (daily_rate_minor IS NOT NULL OR cycle_price_minor IS NOT NULL)",
            params![quote_id],
            |row| row.get(0),
        ).map_err(|error| AppError::Internal(format!("check Client quote funding rail failed: {error}")))?;
        if paid_count > 0 {
            crate::market_payments::require_current_offer_rail(&funding_rail, "Client quote")?;
            let price: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(COALESCE(cycle_price_minor, daily_rate_minor, 0)), 0)
                 FROM client_market_allocation_quote_items WHERE quote_id = ?1",
                    params![quote_id],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    AppError::Internal(format!("read Client batch funding failed: {error}"))
                })?;
            crate::market_wallet::require_order_funding_tx(
                &tx,
                &session.user_id,
                price,
                &now.to_rfc3339(),
            )?;
        }
        ensure_creation_allowed_tx(&tx, &session.user_id, &session.email)?;
        let item_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM client_market_allocation_quote_items WHERE quote_id = ?1",
                params![quote_id],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Internal(format!("count quote items failed: {error}")))?;
        if item_count as usize != prepared.len() {
            return Err(AppError::BadRequest(
                "quote commit items do not match the quote".into(),
            ));
        }
        let mut unique_item_ids = std::collections::HashSet::new();
        let mut unique_subdomains = std::collections::HashSet::new();
        for (item_id, subdomain, _, _, _) in prepared {
            if !unique_item_ids.insert(item_id.to_string())
                || !unique_subdomains.insert(subdomain.to_ascii_lowercase())
            {
                return Err(AppError::BadRequest(
                    "quote items and subdomains must be unique".into(),
                ));
            }
            let reserved: i64 = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM subdomain_reservations
                                 WHERE subdomain = ?1 COLLATE NOCASE AND expires_at_ms > ?2)",
                    params![subdomain, now.timestamp_millis()],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    AppError::Internal(format!(
                        "check quoted subdomain reservation failed: {error}"
                    ))
                })?;
            if reserved != 0
                || crate::client_market::client_market_label_blocks_new_client(
                    &tx,
                    subdomain,
                    &session.email,
                    Some(&session.user_id),
                )?
            {
                return Err(AppError::Conflict(format!(
                    "subdomain {subdomain} is already in use"
                )));
            }
        }
        let batch_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO client_market_batches
                (id, quote_id, client_user_id, client_owner_email, status, created_at, updated_at,
                 idempotency_key, request_fingerprint)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?5, ?6, ?7)",
            params![
                batch_id,
                quote_id,
                session.user_id,
                normalize_email(&session.email)?,
                now.to_rfc3339(),
                idempotency_key,
                request_fingerprint,
            ],
        )
        .map_err(|error| AppError::Internal(format!("insert Client batch failed: {error}")))?;
        // Every item is funded by the same wallet. Preserve request order.
        let mut job_ids_by_request = vec![None; prepared.len()];
        for prepared_index in 0..prepared.len() {
            let (item_id, subdomain, _, confirmed_revision, auto_renew) = &prepared[prepared_index];
            let item = tx
                .query_row(
                    "SELECT host_id, provider_id, host_owner_email, country_code,
                            daily_rate_minor, pricing_model, cycle_price_minor,
                            billing_interval, currency, offer_revision,
                            wallet_trial_seconds, wallet_trial_allowance_seconds, wallet_trial_epoch
                     FROM client_market_allocation_quote_items
                     WHERE id = ?1 AND quote_id = ?2",
                    params![item_id, quote_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, Option<i64>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, i64>(9)?,
                            row.get::<_, i64>(10)?,
                            row.get::<_, i64>(11)?,
                            row.get::<_, i64>(12)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| AppError::Internal(format!("read quote item failed: {error}")))?
                .ok_or_else(|| {
                    AppError::BadRequest("quote item does not belong to this quote".into())
                })?;
            if *confirmed_revision != item.9 {
                return Err(AppError::Conflict(
                    "the confirmed Host offer revision does not match the allocation quote; review the current offer"
                        .into(),
                ));
            }
            if item.6.or(item.4).is_some() {
                crate::market_client_trials::validate_quote_epoch_tx(
                    &tx,
                    &session.user_id,
                    &item.1,
                    &item.0,
                    item.12,
                )?;
                let current_rail: String = tx
                    .query_row(
                        "SELECT funding_rail FROM router_ssh_hosts WHERE id = ?1",
                        params![item.0],
                        |row| row.get(0),
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("read quoted Host funding rail failed: {error}"))
                    })?;
                crate::market_payments::require_current_offer_rail(&current_rail, "Client Host")?;
            }
            crate::market_access::ensure_product_access_tx(
                &tx,
                &item.1,
                &session.user_id,
                &session.email,
                crate::market_access::PRODUCT_CLIENT_HOST,
                crate::market_access::pricing_kind_for_rate(item.6.or(item.4)),
            )?;
            let changed = tx
                .execute(
                    "UPDATE router_ssh_hosts SET status = 'locked', updated_at = ?2
                     WHERE id = ?1 AND status = ?3 AND offer_revision = ?4
                       AND provider_id = ?5",
                    params![
                        item.0,
                        now.to_rfc3339(),
                        HOST_STATUS_RESERVED,
                        item.9,
                        item.1
                    ],
                )
                .map_err(|error| AppError::Internal(format!("lock quoted Host failed: {error}")))?;
            if changed != 1 {
                return Err(AppError::Conflict(
                    "the quoted Host is no longer reserved or its offer changed; review the current offer"
                        .into(),
                ));
            }
            let job_id = Uuid::new_v4().to_string();
            let owners_json =
                serde_json::to_string(&vec![item.2.clone()]).unwrap_or_else(|_| "[]".into());
            let regions_json =
                serde_json::to_string(&item.3.clone().into_iter().collect::<Vec<_>>())
                    .unwrap_or_else(|_| "[]".into());
            tx.execute(
                "INSERT INTO provisioning_jobs (
                    id, type, host_id, host_owner_email, client_owner_email,
                    selection_owners_json, selection_regions_json, subdomain, installation_id,
                    status, phase, log_blob, secret_ref, failure_code, created_at, updated_at,
                    batch_id, quote_id, client_owner_user_id
                 ) VALUES (?1, 'create', ?2, ?3, ?4, ?5, ?6, ?7, NULL,
                           'pending', 'locked', '', NULL, NULL, ?8, ?8, ?9, ?10, ?11)",
                params![
                    job_id,
                    item.0,
                    item.2,
                    normalize_email(&session.email)?,
                    owners_json,
                    regions_json,
                    subdomain,
                    now.to_rfc3339(),
                    batch_id,
                    quote_id,
                    session.user_id,
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("insert quoted provisioning job failed: {error}"))
            })?;
            if funding_rail == "platform_wallet"
                && let Some(price_minor) = item.6.or(item.4)
            {
                if item.8.as_deref() != Some("USD") {
                    return Err(AppError::Conflict(
                        "wallet-funded Hosts must be priced in USD".into(),
                    ));
                }
                crate::market_platform_contracts::prepare_contract_tx(
                    &tx,
                    &crate::market_platform_contracts::PrepareContract {
                        product_kind: "client_host",
                        product_ref: &job_id,
                        trial_scope_ref: &item.0,
                        quote_id,
                        buyer_user_id: &session.user_id,
                        provider_user_id: &item.1,
                        offer_revision: item.9,
                        billing_model: if item.6.is_some() {
                            crate::market_platform_contracts::BillingModel::CalendarMonth
                        } else {
                            crate::market_platform_contracts::BillingModel::Metered
                        },
                        price_minor,
                        auto_renew: item.6.is_some() && *auto_renew,
                        trial_seconds: item.10,
                        trial_allowance_seconds: item.11,
                    },
                    &now.to_rfc3339(),
                )?;
            }
            tx.execute(
                "INSERT INTO subdomain_reservations
                    (subdomain, job_id, host_id, client_owner_email, installation_id, expires_at_ms)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                params![
                    subdomain,
                    job_id,
                    item.0,
                    normalize_email(&session.email)?,
                    now.timestamp_millis() + 30 * 60 * 1_000,
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("reserve quoted subdomain failed: {error}"))
            })?;
            job_ids_by_request[prepared_index] = Some(job_id);
        }
        let job_ids = job_ids_by_request
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| AppError::Internal("Client quote job ordering is incomplete".into()))?;
        tx.execute(
            "UPDATE client_market_allocation_quotes
             SET status = 'committed', updated_at = ?2 WHERE id = ?1 AND status = 'active'",
            params![quote_id, now.to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("commit quote state failed: {error}")))?;
        insert_audit_tx(
            &tx,
            None,
            None,
            Some(&session.user_id),
            Some(&session.email),
            "allocation_quote_committed",
            serde_json::json!({ "quoteId": quote_id, "batchId": batch_id, "jobIds": job_ids }),
            now,
        )?;
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit Client batch failed: {error}")))?;
        Ok(CommitQuoteResponse {
            batch_id,
            job_ids,
            replayed: false,
        })
    }

    pub async fn client_market_cancel_quote(
        &self,
        quote_id: &str,
        session: &AuthSession,
    ) -> Result<(), AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin quote cancellation failed: {error}"))
        })?;
        expire_quotes_tx(&tx, now)?;
        let quote: Option<(String, String)> = tx
            .query_row(
                "SELECT client_user_id, status
                 FROM client_market_allocation_quotes WHERE id = ?1",
                params![quote_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read quote for cancellation failed: {error}"))
            })?;
        let (owner_user_id, status) =
            quote.ok_or_else(|| AppError::NotFound("allocation quote not found".into()))?;
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "allocation quote belongs to another account".into(),
            ));
        }
        if matches!(status.as_str(), "cancelled" | "expired") {
            tx.commit().map_err(|error| {
                AppError::Internal(format!(
                    "commit idempotent quote cancellation failed: {error}"
                ))
            })?;
            return Ok(());
        }
        if status != "active" {
            return Err(AppError::Conflict(
                "an allocation quote can only be cancelled before it is committed".into(),
            ));
        }
        tx.execute(
            "UPDATE router_ssh_hosts
             SET status = 'idle', updated_at = ?2
             WHERE status = ?3 AND id IN (
                 SELECT host_id FROM client_market_allocation_quote_items WHERE quote_id = ?1
             )",
            params![quote_id, now.to_rfc3339(), HOST_STATUS_RESERVED],
        )
        .map_err(|error| {
            AppError::Internal(format!("release cancelled quote Hosts failed: {error}"))
        })?;
        let changed = tx
            .execute(
                "UPDATE client_market_allocation_quotes
                 SET status = 'cancelled', updated_at = ?2
                 WHERE id = ?1 AND status = 'active'",
                params![quote_id, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("cancel allocation quote failed: {error}"))
            })?;
        if changed != 1 {
            return Err(AppError::Conflict("allocation quote state changed".into()));
        }
        insert_audit_tx(
            &tx,
            None,
            None,
            Some(&session.user_id),
            Some(&session.email),
            "allocation_quote_cancelled",
            serde_json::json!({ "quoteId": quote_id }),
            now,
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit quote cancellation failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn client_market_sync_batch_for_job(&self, job_id: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin Client batch sync failed: {error}"))
            })?;
        let batch_id: Option<String> = tx
            .query_row(
                "SELECT batch_id FROM provisioning_jobs WHERE id = ?1",
                params![job_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read job batch failed: {error}")))?
            .flatten();
        let Some(batch_id) = batch_id else {
            return Ok(());
        };
        let counts: (i64, i64, i64) = tx
            .query_row(
                "SELECT COUNT(*),
                        SUM(CASE WHEN status = 'succeeded' THEN 1 ELSE 0 END),
                        SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END)
                 FROM provisioning_jobs WHERE batch_id = ?1",
                params![batch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| AppError::Internal(format!("count batch jobs failed: {error}")))?;
        if counts.1 + counts.2 < counts.0 {
            return Ok(());
        }
        let status = if counts.1 == counts.0 {
            "succeeded"
        } else if counts.1 > 0 {
            "partial_failed"
        } else {
            "failed"
        };
        tx.execute(
            "UPDATE client_market_batches SET status = ?2, updated_at = ?3
             WHERE id = ?1 AND status = 'running'",
            params![batch_id, status, Utc::now().to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("update Client batch failed: {error}")))?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Client batch sync failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn client_market_batch(
        &self,
        batch_id: &str,
        session: &AuthSession,
    ) -> Result<ClientBatchView, AppError> {
        let conn = self.conn.lock().await;
        let header = conn
            .query_row(
                "SELECT id, quote_id, client_user_id, status, created_at, updated_at
                 FROM client_market_batches WHERE id = ?1",
                params![batch_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read Client batch failed: {error}")))?
            .ok_or_else(|| AppError::NotFound("Client batch not found".into()))?;
        if header.2 != session.user_id {
            return Err(AppError::Forbidden(
                "Client batch belongs to another account".into(),
            ));
        }
        let jobs = conn
            .prepare(
                "SELECT j.id, j.type, j.host_id, j.host_owner_email, j.client_owner_email,
                        j.subdomain, j.installation_id, j.status, j.phase, j.failure_code,
                        h.country_code, j.log_blob, j.created_at, j.updated_at
                 FROM provisioning_jobs j
                 LEFT JOIN router_ssh_hosts h ON h.id = j.host_id
                 WHERE j.batch_id = ?1 ORDER BY j.created_at, j.id",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![batch_id], |row| {
                        Ok(crate::client_market::JobView {
                            id: row.get(0)?,
                            job_type: row.get(1)?,
                            host_id: row.get(2)?,
                            host_owner_email: row.get(3)?,
                            client_owner_email: row.get(4)?,
                            subdomain: row.get(5)?,
                            installation_id: row.get(6)?,
                            status: row.get(7)?,
                            phase: row.get(8)?,
                            failure_code: row.get(9)?,
                            country_code: row.get(10)?,
                            client_url: None,
                            log: row.get(11)?,
                            created_at: row.get(12)?,
                            updated_at: row.get(13)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|error| {
                AppError::Internal(format!("read Client batch jobs failed: {error}"))
            })?;
        Ok(ClientBatchView {
            id: header.0,
            quote_id: header.1,
            status: header.3,
            created_at: header.4,
            updated_at: header.5,
            jobs,
        })
    }

    pub async fn client_market_list_rentals_for_viewer(
        &self,
        session: &AuthSession,
    ) -> Result<Vec<RentalView>, AppError> {
        let conn = self.conn.lock().await;
        load_rental_views(&conn, None, session)
    }

    pub async fn client_market_host_usage_history(
        &self,
        host_id: &str,
    ) -> Result<Vec<HostUsageHistoryEntry>, AppError> {
        let conn = self.conn.lock().await;
        let host_exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM router_ssh_hosts WHERE id = ?1",
                params![host_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read Host for usage history failed: {error}"))
            })?;
        if host_exists.is_none() {
            return Err(AppError::NotFound("host not found".into()));
        }
        let mut statement = conn
            .prepare(
                "SELECT s.installation_id, s.client_owner_email, s.status,
                        COALESCE(NULLIF(TRIM(s.client_subdomain), ''),
                                 (SELECT t.subdomain FROM installation_client_tunnels t
                                  WHERE t.installation_id = s.installation_id),
                                 (SELECT p.label FROM public_hosts p
                                  WHERE p.kind = 'client' AND p.subject_id = s.installation_id
                                  ORDER BY p.updated_at DESC LIMIT 1)),
                        COALESCE(s.activated_at, s.created_at),
                        CASE
                            WHEN s.status = 'released' THEN COALESCE(s.released_at, s.updated_at)
                            ELSE NULL
                        END,
                        s.daily_rate_minor, s.cycle_price_minor, s.pricing_model, s.currency,
                        COALESCE((
                            SELECT SUM(period.amount_units)
                            FROM market_platform_periods period
                            JOIN market_platform_contracts contract ON contract.id = period.contract_id
                            WHERE contract.product_kind = 'client_host' AND contract.service_ref = s.installation_id
                        ), 0) + COALESCE((
                            SELECT SUM(batch.amount_units)
                            FROM market_usage_posting_batches batch
                            JOIN market_platform_contracts contract ON contract.id = batch.contract_id
                            WHERE contract.product_kind = 'client_host' AND contract.service_ref = s.installation_id
                        ), 0),
                        COALESCE((
                            SELECT SUM(period.refunded_units)
                            FROM market_platform_periods period
                            JOIN market_platform_contracts contract ON contract.id = period.contract_id
                            WHERE contract.product_kind = 'client_host' AND contract.service_ref = s.installation_id
                        ), 0),
                        COALESCE((SELECT SUM(d.amount_units) FROM market_delivery_slices d
                            JOIN market_platform_contracts c ON c.id=d.contract_id
                            WHERE c.product_kind='client_host' AND c.service_ref=s.installation_id),0),
                        COALESCE((SELECT SUM(c.amount_units-c.reversed_units) FROM client_termination_compensations c
                            JOIN client_market_terminations t ON t.id=c.id WHERE t.installation_id=s.installation_id),0)
                 FROM client_market_subscriptions s
                 WHERE s.host_id = ?1
                 ORDER BY datetime(COALESCE(s.activated_at, s.created_at)) DESC,
                          datetime(COALESCE(s.released_at, s.updated_at)) DESC,
                          s.installation_id DESC
                 LIMIT ?2",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare Host usage history failed: {error}"))
            })?;
        let rows = statement
            .query_map(params![host_id, HOST_USAGE_HISTORY_LIMIT,], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                ))
            })
            .map_err(|error| {
                AppError::Internal(format!("query Host usage history failed: {error}"))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AppError::Internal(format!("read Host usage history failed: {error}"))
            })?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    installation_id,
                    client_owner_email,
                    status,
                    client_subdomain,
                    started_at,
                    ended_at,
                    daily_rate_minor,
                    cycle_price_minor,
                    pricing_model,
                    currency,
                    charge_units,
                    refunded_units,
                    service_units,
                    compensation_units,
                )| HostUsageHistoryEntry {
                    service_units,
                    compensation_units,
                    installation_id,
                    client_owner_email,
                    client_subdomain: client_subdomain
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty()),
                    status,
                    started_at,
                    ended_at,
                    daily_rate_minor,
                    cycle_price_minor,
                    pricing_model,
                    currency,
                    charges_minor: crate::market_common::ceil_minor_units(charge_units),
                    refunded_minor: crate::market_common::floor_minor_units(refunded_units),
                    net_charges_minor: crate::market_common::ceil_minor_units(
                        charge_units.saturating_sub(refunded_units),
                    ),
                },
            )
            .collect())
    }

    pub async fn client_market_grant_provider_terminal_authorization(
        &self,
        installation_id: &str,
        session: &AuthSession,
        duration_minutes: u32,
    ) -> Result<ProviderTerminalAuthorizationView, AppError> {
        if !(MIN_PROVIDER_TERMINAL_AUTH_MINUTES..=MAX_PROVIDER_TERMINAL_AUTH_MINUTES)
            .contains(&duration_minutes)
        {
            return Err(AppError::BadRequest(format!(
                "provider terminal authorization must last between {MIN_PROVIDER_TERMINAL_AUTH_MINUTES} and {MAX_PROVIDER_TERMINAL_AUTH_MINUTES} minutes"
            )));
        }
        let now = Utc::now();
        let expires_at = now + Duration::minutes(i64::from(duration_minutes));
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!(
                    "begin Provider terminal authorization failed: {error}"
                ))
            })?;
        let subscription = tx
            .query_row(
                "SELECT host_id, provider_id, client_user_id, status
                 FROM client_market_subscriptions WHERE installation_id = ?1",
                params![installation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!(
                    "read rental for Provider terminal authorization failed: {error}"
                ))
            })?
            .ok_or_else(|| AppError::NotFound("Client Market rental not found".into()))?;
        if subscription.2 != session.user_id {
            return Err(AppError::Forbidden(
                "only the renter can authorize Provider terminal access".into(),
            ));
        }
        if !matches!(subscription.3.as_str(), "active" | "billing_suspended") {
            return Err(AppError::Conflict(
                "Provider terminal access can only be authorized for an active rental".into(),
            ));
        }
        let now_text = now.to_rfc3339();
        let expires_at_text = expires_at.to_rfc3339();
        tx.execute(
            "INSERT INTO client_market_provider_terminal_authorizations (
                installation_id, host_id, provider_id, client_user_id, expires_at,
                revoked_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?6)
             ON CONFLICT(installation_id) DO UPDATE SET
                host_id = excluded.host_id,
                provider_id = excluded.provider_id,
                client_user_id = excluded.client_user_id,
                expires_at = excluded.expires_at,
                revoked_at = NULL,
                updated_at = excluded.updated_at",
            params![
                installation_id,
                subscription.0,
                subscription.1,
                session.user_id,
                expires_at_text,
                now_text,
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "save Provider terminal authorization failed: {error}"
            ))
        })?;
        insert_audit_tx(
            &tx,
            Some(installation_id),
            Some(&subscription.0),
            Some(&session.user_id),
            Some(&session.email),
            "provider_terminal_authorized",
            serde_json::json!({
                "providerId": subscription.1,
                "expiresAt": expires_at_text,
                "durationMinutes": duration_minutes,
            }),
            now,
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!(
                "commit Provider terminal authorization failed: {error}"
            ))
        })?;
        Ok(ProviderTerminalAuthorizationView {
            active: true,
            expires_at: Some(expires_at_text),
            updated_at: now_text,
        })
    }

    pub async fn client_market_revoke_provider_terminal_authorization(
        &self,
        installation_id: &str,
        session: &AuthSession,
    ) -> Result<ProviderTerminalAuthorizationView, AppError> {
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!(
                    "begin Provider terminal revocation failed: {error}"
                ))
            })?;
        let subscription = tx
            .query_row(
                "SELECT host_id, provider_id, client_user_id
                 FROM client_market_subscriptions WHERE installation_id = ?1",
                params![installation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!(
                    "read rental for Provider terminal revocation failed: {error}"
                ))
            })?
            .ok_or_else(|| AppError::NotFound("Client Market rental not found".into()))?;
        if subscription.2 != session.user_id {
            return Err(AppError::Forbidden(
                "only the renter can revoke Provider terminal access".into(),
            ));
        }
        tx.execute(
            "UPDATE client_market_provider_terminal_authorizations
             SET revoked_at = ?2, updated_at = ?2
             WHERE installation_id = ?1 AND revoked_at IS NULL",
            params![installation_id, now_text],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "revoke Provider terminal authorization failed: {error}"
            ))
        })?;
        insert_audit_tx(
            &tx,
            Some(installation_id),
            Some(&subscription.0),
            Some(&session.user_id),
            Some(&session.email),
            "provider_terminal_revoked",
            serde_json::json!({ "providerId": subscription.1 }),
            now,
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!(
                "commit Provider terminal revocation failed: {error}"
            ))
        })?;
        Ok(ProviderTerminalAuthorizationView {
            active: false,
            expires_at: None,
            updated_at: now_text,
        })
    }

    pub async fn client_market_provider_terminal_authorized_until(
        &self,
        installation_id: &str,
        host_id: &str,
        provider_id: &str,
    ) -> Result<Option<DateTime<Utc>>, AppError> {
        let conn = self.conn.lock().await;
        let expires_at = conn
            .query_row(
                "SELECT a.expires_at
             FROM client_market_provider_terminal_authorizations a
             JOIN client_market_subscriptions s
               ON s.installation_id = a.installation_id
              AND s.host_id = a.host_id
              AND s.provider_id = a.provider_id
              AND s.client_user_id = a.client_user_id
             WHERE a.installation_id = ?1
               AND a.host_id = ?2
               AND a.provider_id = ?3
               AND a.revoked_at IS NULL
               AND a.expires_at > ?4
               AND s.status IN ('active', 'billing_suspended')",
                params![
                    installation_id,
                    host_id,
                    provider_id,
                    Utc::now().to_rfc3339()
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!(
                    "check Provider terminal authorization failed: {error}"
                ))
            })?;
        expires_at
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(|error| {
                        AppError::Internal(format!(
                            "read Provider terminal authorization expiry failed: {error}"
                        ))
                    })
            })
            .transpose()
    }

    pub async fn client_market_provider_terminal_authorized_host_ids(
        &self,
        provider_id: &str,
    ) -> Result<std::collections::HashSet<String>, AppError> {
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT a.host_id
                 FROM client_market_provider_terminal_authorizations a
                 JOIN client_market_subscriptions s
                   ON s.installation_id = a.installation_id
                  AND s.host_id = a.host_id
                  AND s.provider_id = a.provider_id
                  AND s.client_user_id = a.client_user_id
                 WHERE a.provider_id = ?1
                   AND a.revoked_at IS NULL
                   AND a.expires_at > ?2
                   AND s.status IN ('active', 'billing_suspended')",
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "prepare authorized Provider terminal Hosts failed: {error}"
                ))
            })?;
        statement
            .query_map(params![provider_id, Utc::now().to_rfc3339()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| {
                AppError::Internal(format!(
                    "query authorized Provider terminal Hosts failed: {error}"
                ))
            })?
            .collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|error| {
                AppError::Internal(format!(
                    "read authorized Provider terminal Hosts failed: {error}"
                ))
            })
    }

    pub async fn client_market_rental_for_viewer(
        &self,
        installation_id: &str,
        session: &AuthSession,
    ) -> Result<RentalView, AppError> {
        let conn = self.conn.lock().await;
        load_rental_views(&conn, Some(installation_id), session)?
            .into_iter()
            .next()
            .ok_or_else(|| AppError::NotFound("Client Market rental not found".into()))
    }

    pub async fn client_market_reconcile_trade_state(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!(
                "begin Client Market quote reconcile failed: {error}"
            ))
        })?;
        expire_quotes_tx(&tx, now)?;
        let expiring = {
            let mut statement = tx
                .prepare(
                    "SELECT s.installation_id, s.host_id, s.expires_at
                     FROM client_market_subscriptions s
                     WHERE s.status = 'active' AND s.daily_rate_minor IS NULL
                       AND s.cycle_price_minor IS NULL
                       AND s.expires_at > ?1 AND s.expires_at <= ?2
                       AND NOT EXISTS (
                           SELECT 1 FROM client_market_subscription_events e
                           WHERE e.installation_id = s.installation_id
                             AND e.event_type = 'free_period_expiring'
                       )",
                )
                .map_err(|error| {
                    AppError::Internal(format!(
                        "prepare expiring free Client subscriptions failed: {error}"
                    ))
                })?;
            statement
                .query_map(
                    params![now.to_rfc3339(), (now + Duration::hours(24)).to_rfc3339()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(|error| {
                    AppError::Internal(format!(
                        "query expiring free Client subscriptions failed: {error}"
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!(
                        "read expiring free Client subscriptions failed: {error}"
                    ))
                })?
        };
        for (installation_id, host_id, expires_at) in expiring {
            insert_audit_tx(
                &tx,
                Some(&installation_id),
                Some(&host_id),
                None,
                None,
                "free_period_expiring",
                serde_json::json!({ "expiresAt": expires_at }),
                now,
            )?;
        }
        let expired_installations = {
            let mut statement = tx
                .prepare(
                    "SELECT installation_id
                     FROM client_market_subscriptions
                     WHERE status = 'active' AND daily_rate_minor IS NULL
                       AND cycle_price_minor IS NULL
                       AND expires_at IS NOT NULL AND expires_at <= ?1
                     ORDER BY expires_at, installation_id",
                )
                .map_err(|error| {
                    AppError::Internal(format!(
                        "prepare expired free Client subscriptions failed: {error}"
                    ))
                })?;
            statement
                .query_map(params![now.to_rfc3339()], |row| row.get(0))
                .map_err(|error| {
                    AppError::Internal(format!(
                        "query expired free Client subscriptions failed: {error}"
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!(
                        "read expired free Client subscriptions failed: {error}"
                    ))
                })?
        };
        tx.commit().map_err(|error| {
            AppError::Internal(format!(
                "commit Client Market quote reconcile failed: {error}"
            ))
        })?;
        Ok(expired_installations)
    }
}

#[derive(Debug)]
struct QuoteCandidate {
    host_id: String,
    provider_id: String,
    host_owner_email: String,
    country_code: Option<String>,
    hostname: Option<String>,
    ip: Option<String>,
    daily_rate_minor: Option<i64>,
    pricing_model: String,
    cycle_price_minor: Option<i64>,
    billing_interval: Option<String>,
    currency: Option<String>,
    free_duration_days: Option<u32>,
    offer_revision: i64,
    funding_rail: String,
    wallet_trial_seconds: i64,
    wallet_trial_consumed_seconds: i64,
    wallet_trial_epoch: i64,
    trial_hours: i64,
}

fn map_quote_candidate(row: &crate::db::Row<'_>) -> crate::db::Result<QuoteCandidate> {
    Ok(QuoteCandidate {
        host_id: row.get(0)?,
        provider_id: row.get(1)?,
        host_owner_email: row.get(2)?,
        country_code: row.get(3)?,
        hostname: row.get(4)?,
        ip: row.get(5)?,
        daily_rate_minor: row.get(6)?,
        pricing_model: row.get(7)?,
        cycle_price_minor: row.get(8)?,
        billing_interval: row.get(9)?,
        currency: row.get(10)?,
        free_duration_days: row
            .get::<_, Option<i64>>(11)?
            .and_then(|value| u32::try_from(value).ok()),
        offer_revision: row.get(12)?,
        funding_rail: row.get(13)?,
        wallet_trial_seconds: 0,
        wallet_trial_consumed_seconds: 0,
        wallet_trial_epoch: 0,
        trial_hours: row.get(14)?,
    })
}

fn normalize_email(value: &str) -> Result<String, AppError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 320
        || value.chars().any(char::is_control)
        || !value.contains('@')
    {
        return Err(AppError::BadRequest("invalid account email".into()));
    }
    Ok(value)
}

fn commit_request_fingerprint(
    prepared: &[(String, String, String, i64)],
) -> Result<String, AppError> {
    let fingerprint_payload = prepared
        .iter()
        .map(|(item_id, subdomain, _, revision)| (item_id, subdomain, revision))
        .collect::<Vec<_>>();
    let encoded = serde_json::to_vec(&fingerprint_payload).map_err(|error| {
        AppError::Internal(format!("encode quote commit fingerprint failed: {error}"))
    })?;
    Ok(hex::encode(Sha256::digest(&encoded)))
}

fn commit_request_fingerprint_with_renewal(
    prepared: &[(String, String, String, i64, bool)],
) -> Result<String, AppError> {
    let fingerprint_payload = prepared
        .iter()
        .map(|(item_id, subdomain, _, revision, auto_renew)| {
            (item_id, subdomain, revision, auto_renew)
        })
        .collect::<Vec<_>>();
    let encoded = serde_json::to_vec(&fingerprint_payload).map_err(|error| {
        AppError::Internal(format!("encode quote commit fingerprint failed: {error}"))
    })?;
    Ok(hex::encode(Sha256::digest(&encoded)))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| AppError::Internal(format!("invalid Client Market timestamp: {error}")))
}

fn sql_vars(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

fn ratio(numerator: i64, denominator: i64) -> f64 {
    if denominator <= 0 {
        0.0
    } else {
        numerator.max(0) as f64 / denominator as f64
    }
}

fn expire_quotes_tx(tx: &Transaction<'_>, now: DateTime<Utc>) -> Result<(), AppError> {
    tx.execute(
        "UPDATE router_ssh_hosts
         SET status = 'idle', updated_at = ?1
         WHERE status = ?2 AND id IN (
             SELECT qi.host_id
             FROM client_market_allocation_quote_items qi
             JOIN client_market_allocation_quotes q ON q.id = qi.quote_id
             WHERE q.status = 'active' AND q.expires_at <= ?1
         )",
        params![now.to_rfc3339(), HOST_STATUS_RESERVED],
    )
    .map_err(|error| AppError::Internal(format!("release expired quote Hosts failed: {error}")))?;
    tx.execute(
        "UPDATE client_market_allocation_quotes
         SET status = 'expired', updated_at = ?1
         WHERE status = 'active' AND expires_at <= ?1",
        params![now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("expire allocation quotes failed: {error}")))?;
    tx.execute(
        "UPDATE router_ssh_hosts
         SET status = 'idle', updated_at = ?1
         WHERE status = ?2 AND NOT EXISTS (
             SELECT 1
             FROM client_market_allocation_quote_items qi
             JOIN client_market_allocation_quotes q ON q.id = qi.quote_id
             WHERE qi.host_id = router_ssh_hosts.id
               AND q.status = 'active' AND q.expires_at > ?1
         )",
        params![now.to_rfc3339(), HOST_STATUS_RESERVED],
    )
    .map_err(|error| {
        AppError::Internal(format!("release orphaned reserved Hosts failed: {error}"))
    })?;
    Ok(())
}

fn ensure_creation_allowed_tx(
    tx: &Transaction<'_>,
    user_id: &str,
    _email: &str,
) -> Result<(), AppError> {
    let blocked_subscription: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM client_market_subscriptions
             WHERE client_user_id = ?1
               AND status IN ('releasing', 'release_failed')",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(|error| {
            AppError::Internal(format!("check Client cleanup gate failed: {error}"))
        })?;
    if blocked_subscription > 0 {
        return Err(AppError::Conflict(
            "finish the current Client cleanup before creating another".into(),
        ));
    }
    let active_jobs: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM provisioning_jobs
             WHERE client_owner_user_id = ?1
               AND type = 'create' AND status IN ('pending', 'running')",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(|error| {
            AppError::Internal(format!("check active Client batch failed: {error}"))
        })?;
    if active_jobs > 0 {
        return Err(AppError::Conflict(
            "wait for the current Client creation batch to finish before creating another".into(),
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct RentalRow {
    installation_id: String,
    host_id: String,
    provider_id: String,
    host_owner_email: String,
    client_user_id: String,
    client_owner_email: String,
    status: String,
    daily_rate_minor: Option<i64>,
    pricing_model: String,
    cycle_price_minor: Option<i64>,
    billing_interval: Option<String>,
    currency: Option<String>,
    free_duration_days: Option<u32>,
    offer_revision: i64,
    activated_at: Option<String>,
    expires_at: Option<String>,
    contacts_json: String,
    active_cleanup_job_id: Option<String>,
    latest_cleanup_failure_code: Option<String>,
    next_cleanup_probe_at: Option<String>,
    cleanup_probe_attempts: Option<i64>,
    bound_host_status: Option<String>,
    wallet_contract_id: Option<String>,
    wallet_lifecycle_id: Option<String>,
    open_wallet_dispute: bool,
    provider_terminal_expires_at: Option<String>,
    provider_terminal_revoked_at: Option<String>,
    updated_at: String,
}

fn load_rental_views(
    conn: &Connection,
    installation_id: Option<&str>,
    session: &AuthSession,
) -> Result<Vec<RentalView>, AppError> {
    let mut sql = "SELECT s.installation_id, s.host_id, s.provider_id, s.host_owner_email,
                s.client_user_id, s.client_owner_email, s.status, s.daily_rate_minor,
                s.currency, s.free_duration_days, s.offer_revision,
                s.activated_at, s.expires_at,
                COALESCE((SELECT contacts_json FROM market_provider_contacts p
                          WHERE p.user_id = s.provider_id), '[]'),
                (SELECT j.id FROM provisioning_jobs j
                 WHERE j.installation_id = s.installation_id
                   AND j.type = 'cleanup'
                   AND j.status IN ('pending', 'running')
                 ORDER BY j.updated_at DESC, j.created_at DESC
                 LIMIT 1),
                a.expires_at,
                a.revoked_at,
                s.updated_at, s.pricing_model, s.cycle_price_minor,
                s.billing_interval,
                (SELECT j.failure_code FROM provisioning_jobs j
                 WHERE j.installation_id = s.installation_id
                   AND j.host_id = s.host_id
                   AND j.type = 'cleanup' AND j.status = 'failed'
                 ORDER BY j.updated_at DESC, j.created_at DESC LIMIT 1),
                (SELECT CASE WHEN r.last_outcome = 'probing' THEN NULL
                             ELSE r.next_attempt_at END
                 FROM client_market_cleanup_retry_state r
                 WHERE r.host_id = s.host_id AND r.installation_id = s.installation_id),
                (SELECT CASE WHEN r.last_outcome = 'probing' THEN NULL
                             ELSE r.attempt_count END
                 FROM client_market_cleanup_retry_state r
                 WHERE r.host_id = s.host_id AND r.installation_id = s.installation_id),
                (SELECT h.status FROM router_ssh_hosts h
                 WHERE h.id = s.host_id AND h.installation_id = s.installation_id),
                (SELECT contract.id FROM market_platform_contracts contract
                 WHERE contract.id = s.platform_contract_id
                   AND contract.paid_started_at IS NOT NULL),
                EXISTS(SELECT 1 FROM market_service_disputes dispute
                       WHERE dispute.contract_id = s.platform_contract_id
                         AND dispute.status = 'open'),
                s.platform_contract_id
         FROM client_market_subscriptions s
         LEFT JOIN client_market_provider_terminal_authorizations a
           ON a.installation_id = s.installation_id
          AND a.host_id = s.host_id
          AND a.provider_id = s.provider_id
          AND a.client_user_id = s.client_user_id"
        .to_string();
    if installation_id.is_some() {
        sql.push_str(" WHERE s.installation_id = ?1");
    }
    sql.push_str(" ORDER BY s.updated_at DESC, s.installation_id");
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| AppError::Internal(format!("prepare rental list failed: {error}")))?;
    let mapper = |row: &crate::db::Row<'_>| {
        Ok(RentalRow {
            installation_id: row.get(0)?,
            host_id: row.get(1)?,
            provider_id: row.get(2)?,
            host_owner_email: row.get(3)?,
            client_user_id: row.get(4)?,
            client_owner_email: row.get(5)?,
            status: row.get(6)?,
            daily_rate_minor: row.get(7)?,
            currency: row.get(8)?,
            free_duration_days: row
                .get::<_, Option<i64>>(9)?
                .and_then(|value| u32::try_from(value).ok()),
            offer_revision: row.get(10)?,
            activated_at: row.get(11)?,
            expires_at: row.get(12)?,
            contacts_json: row.get(13)?,
            active_cleanup_job_id: row.get(14)?,
            provider_terminal_expires_at: row.get(15)?,
            provider_terminal_revoked_at: row.get(16)?,
            updated_at: row.get(17)?,
            pricing_model: row.get(18)?,
            cycle_price_minor: row.get(19)?,
            billing_interval: row.get(20)?,
            latest_cleanup_failure_code: row.get(21)?,
            next_cleanup_probe_at: row.get(22)?,
            cleanup_probe_attempts: row.get(23)?,
            bound_host_status: row.get(24)?,
            wallet_contract_id: row.get(25)?,
            wallet_lifecycle_id: row.get(27)?,
            open_wallet_dispute: row.get::<_, i64>(26)? != 0,
        })
    };
    let rows = if let Some(installation_id) = installation_id {
        statement
            .query_map(params![installation_id], mapper)
            .map_err(|error| AppError::Internal(format!("query rental failed: {error}")))?
            .collect::<Result<Vec<_>, _>>()
    } else {
        statement
            .query_map([], mapper)
            .map_err(|error| AppError::Internal(format!("query rental list failed: {error}")))?
            .collect::<Result<Vec<_>, _>>()
    }
    .map_err(|error| AppError::Internal(format!("read rental rows failed: {error}")))?;
    let mut output = Vec::new();
    for row in rows {
        let client_role = row.client_user_id == session.user_id;
        let provider_role = row.provider_id == session.user_id;
        if !client_role && !provider_role {
            continue;
        }
        let provider_terminal_access_active = row.provider_terminal_revoked_at.is_none()
            && row
                .provider_terminal_expires_at
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .is_some_and(|expires_at| expires_at > Utc::now());
        let host_cleanup_pending = row
            .bound_host_status
            .as_deref()
            .is_some_and(|status| matches!(status, "unreachable" | "draining" | "abnormal"));
        let cleanup_failure_kind = row
            .latest_cleanup_failure_code
            .as_deref()
            .filter(|_| host_cleanup_pending)
            .map(|code| code.split(':').next().unwrap_or(code))
            .map(|kind| match kind {
                "cleanup_ssh_unreachable"
                | "cleanup_ssh_timeout"
                | "cleanup_stop_failed"
                | "cleanup_wipe_failed"
                | "cleanup_purge_failed"
                | "cleanup_fingerprint_mismatch"
                | "cleanup_host_binding_mismatch" => kind.to_string(),
                _ => "cleanup_failed".to_string(),
            });
        output.push(RentalView {
            installation_id: row.installation_id,
            host_id: row.host_id,
            provider_id: row.provider_id,
            host_owner_email: row.host_owner_email,
            client_owner_email: row.client_owner_email,
            status: row.status.clone(),
            daily_rate_minor: row.daily_rate_minor,
            pricing_model: row.pricing_model,
            cycle_price_minor: row.cycle_price_minor,
            billing_interval: row.billing_interval,
            wallet_contract: if client_role {
                row.wallet_lifecycle_id
                    .as_deref()
                    .map(|id| {
                        crate::market_platform_contracts::wallet_lifecycle_view_tx(
                            conn,
                            id,
                            &session.user_id,
                        )
                    })
                    .transpose()?
                    .flatten()
            } else {
                None
            },
            wallet_contract_id: client_role.then_some(row.wallet_contract_id).flatten(),
            open_wallet_dispute: client_role && row.open_wallet_dispute,
            currency: row.currency,
            free_duration_days: row.free_duration_days,
            offer_revision: row.offer_revision,
            activated_at: row.activated_at,
            expires_at: row.expires_at,
            contacts: serde_json::from_str(&row.contacts_json).unwrap_or_default(),
            is_client_owner: client_role,
            can_release: (client_role || provider_role)
                && row.status != SUBSCRIPTION_RELEASED
                && row.status != SUBSCRIPTION_RELEASING,
            can_finalize_release: client_role
                && matches!(
                    row.status.as_str(),
                    SUBSCRIPTION_RELEASING | SUBSCRIPTION_RELEASE_FAILED
                )
                && row.active_cleanup_job_id.is_none(),
            host_cleanup_pending,
            cleanup_failure_kind,
            next_cleanup_probe_at: row.next_cleanup_probe_at,
            cleanup_probe_attempts: row.cleanup_probe_attempts,
            active_cleanup_job_id: row.active_cleanup_job_id,
            provider_terminal_authorized_until: provider_terminal_access_active
                .then_some(row.provider_terminal_expires_at)
                .flatten(),
            provider_terminal_access_active,
            can_manage_provider_terminal: client_role
                && matches!(row.status.as_str(), "active" | "billing_suspended"),
            updated_at: row.updated_at,
        });
    }
    Ok(output)
}

fn client_market_event_is_chat_visible(event_type: &str) -> bool {
    matches!(
        event_type,
        "client_provisioned"
            | "free_period_expiring"
            | "free_period_expired"
            | "cleanup_started"
            | "cleanup_finished"
            | "cleanup_failed"
            | "subscription_force_released"
            | "subscription_release_finalized"
    )
}

type ClientMarketChatContext = (
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
    i64,
    String,
    Option<String>,
);

fn client_market_event_fallback_status(event_type: &str) -> &'static str {
    match event_type {
        "client_provisioned" => SUBSCRIPTION_ACTIVE,
        "free_period_expiring" => SUBSCRIPTION_ACTIVE,
        "free_period_expired" => SUBSCRIPTION_RELEASING,
        "cleanup_started" => SUBSCRIPTION_RELEASING,
        "cleanup_finished" | "subscription_force_released" | "subscription_release_finalized" => {
            SUBSCRIPTION_RELEASED
        }
        // Cleanup failure now quarantines the Host without reopening the rental.
        "cleanup_failed" => SUBSCRIPTION_RELEASED,
        _ => "unknown",
    }
}

#[allow(clippy::too_many_arguments)]
fn enqueue_client_market_chat_event_tx(
    tx: &Transaction<'_>,
    source_event_id: &str,
    installation_id: &str,
    host_id: Option<&str>,
    actor_user_id: Option<&str>,
    actor_email: Option<&str>,
    event_type: &str,
    detail: &serde_json::Value,
    now: &str,
) -> Result<(), AppError> {
    if !client_market_event_is_chat_visible(event_type) {
        return Ok(());
    }
    let context: Option<ClientMarketChatContext> = tx
        .query_row(
            "SELECT COALESCE(NULLIF(t.subdomain, ''), s.installation_id),
                    s.client_user_id, s.client_owner_email, s.provider_id,
                    COALESCE(p.owner_email, s.host_owner_email),
                    s.daily_rate_minor, s.pricing_model, s.cycle_price_minor,
                    s.billing_interval, s.currency, s.offer_revision, s.status,
                    h.hostname
             FROM client_market_subscriptions s
             LEFT JOIN installation_client_tunnels t ON t.installation_id = s.installation_id
             LEFT JOIN host_provider_profiles p ON p.provider_id = s.provider_id
             LEFT JOIN router_ssh_hosts h ON h.id = s.host_id
             WHERE s.installation_id = ?1",
            params![installation_id],
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
                ))
            },
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("read Client Market chat context failed: {error}"))
        })?;
    let context = if context.is_some() {
        context
    } else {
        tx.query_row(
            "SELECT COALESCE(NULLIF(t.subdomain, ''), i.id),
                    COALESCE(
                        (SELECT u.id FROM users u
                         WHERE u.email_normalized = lower(trim(COALESCE(NULLIF(i.owner_email, ''), t.owner_email)))),
                        'email:' || lower(trim(COALESCE(NULLIF(i.owner_email, ''), t.owner_email)))
                    ),
                    COALESCE(NULLIF(i.owner_email, ''), t.owner_email),
                    COALESCE(
                        NULLIF(h.provider_id, ''),
                        (SELECT profile.provider_id FROM host_provider_profiles profile
                         WHERE lower(profile.owner_email) = lower(h.host_owner_email)
                         ORDER BY CASE WHEN profile.provider_id LIKE 'email:%' THEN 1 ELSE 0 END,
                                  profile.created_at
                         LIMIT 1),
                        (SELECT u.id FROM users u
                         WHERE u.email_normalized = lower(trim(h.host_owner_email))),
                        'email:' || lower(trim(h.host_owner_email))
                    ),
                    COALESCE(p.owner_email, h.host_owner_email),
                    h.daily_rate_minor, h.pricing_model, h.cycle_price_minor,
                    h.billing_interval,
                    CASE WHEN h.daily_rate_minor IS NULL AND h.cycle_price_minor IS NULL THEN NULL
                         ELSE COALESCE(NULLIF(trim(h.currency), ''), 'USD') END,
                    COALESCE(h.offer_revision, 1), ?3, h.hostname
             FROM installations i
             LEFT JOIN installation_client_tunnels t ON t.installation_id = i.id
             LEFT JOIN router_ssh_hosts h ON h.id = COALESCE(?2, i.provision_host_id)
             LEFT JOIN host_provider_profiles p ON p.provider_id = h.provider_id
             WHERE i.id = ?1
               AND h.id IS NOT NULL
               AND COALESCE(NULLIF(i.owner_email, ''), NULLIF(t.owner_email, '')) IS NOT NULL
               AND NULLIF(h.host_owner_email, '') IS NOT NULL",
            params![
                installation_id,
                host_id,
                client_market_event_fallback_status(event_type)
            ],
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
                ))
            },
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!(
                "read fallback Client Market chat context failed: {error}"
            ))
        })?
    };
    let Some((
        client_label,
        client_user_id,
        client_owner_email,
        provider_user_id,
        provider_email,
        daily_rate_minor,
        pricing_model,
        cycle_price_minor,
        billing_interval,
        currency,
        offer_revision,
        status,
        hostname,
    )) = context
    else {
        return Ok(());
    };
    let contacts_json: Option<String> = tx
        .query_row(
            "SELECT contacts_json FROM market_provider_contacts WHERE user_id = ?1",
            params![provider_user_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!(
                "read Client Market support contacts failed: {error}"
            ))
        })?;
    let contacts: Vec<ProviderContact> = contacts_json
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default();
    let summary = format!("{client_label}: {}", event_type.replace('_', " "));
    let mut payload = serde_json::json!({
        "summary": summary,
        "marketKind": "client",
        "installationId": installation_id,
        "clientLabel": client_label,
        "clientUserId": client_user_id,
        "clientOwnerEmail": client_owner_email,
        "providerUserId": provider_user_id,
        "providerEmail": provider_email,
        "supplierEmail": provider_email,
        "hostId": host_id,
        "hostname": hostname,
        "status": status,
        "dailyRateMinor": daily_rate_minor,
        "pricingModel": pricing_model,
        "cyclePriceMinor": cycle_price_minor,
        "billingInterval": billing_interval,
        "currency": currency,
        "offerRevision": offer_revision,
        "contacts": contacts,
        "actorUserId": actor_user_id,
        "actorEmail": actor_email,
    });
    let output = payload
        .as_object_mut()
        .expect("Client Market chat payload is an object");
    for field in [
        "previousStatus",
        "hostStatus",
        "reason",
        "failureCode",
        "providerDeniedClientAccess",
        "trialHours",
        "freeDurationDays",
        "activatedAt",
        "expiresAt",
    ] {
        if let Some(value) = detail.get(field) {
            output.insert(field.into(), value.clone());
        }
    }
    let mut followers = vec![client_user_id, provider_user_id];
    if let Some(actor_user_id) = actor_user_id {
        followers.push(actor_user_id.to_string());
    }
    let public_payload = crate::store::client_chat::public_market_event_payload(&payload);
    crate::store::client_chat::enqueue_client_system_event_tx(
        tx,
        installation_id,
        "client_market",
        source_event_id,
        event_type,
        public_payload,
        &followers,
        now,
    )
}

pub(crate) fn insert_audit_tx(
    tx: &Transaction<'_>,
    installation_id: Option<&str>,
    host_id: Option<&str>,
    actor_user_id: Option<&str>,
    actor_email: Option<&str>,
    event_type: &str,
    detail: serde_json::Value,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let detail = crate::store::client_chat::sanitize_system_event_payload(detail)?;
    let event_id = Uuid::new_v4().to_string();
    let detail_json = detail.to_string();
    let created_at = now.to_rfc3339();
    tx.execute(
        "INSERT INTO client_market_audit_events
            (id, installation_id, host_id, actor_user_id, actor_email,
             event_type, detail_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event_id,
            installation_id,
            host_id,
            actor_user_id,
            actor_email,
            event_type,
            detail_json,
            created_at,
        ],
    )
    .map_err(|error| {
        AppError::Internal(format!("insert Client Market audit event failed: {error}"))
    })?;
    if event_type == "host_offer_updated" {
        let offer_revision = detail
            .get("offerRevision")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                AppError::Internal("Host offer audit is missing offerRevision".into())
            })?;
        tx.execute(
            "INSERT INTO client_market_offer_events (
                id, host_id, offer_revision, actor_user_id, actor_email, detail_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event_id,
                host_id,
                offer_revision,
                actor_user_id,
                actor_email,
                detail_json,
                created_at
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("insert Client Market offer event failed: {error}"))
        })?;
    } else if let Some(installation_id) = installation_id {
        tx.execute(
            "INSERT INTO client_market_subscription_events (
                id, installation_id, host_id, actor_user_id, actor_email,
                event_type, detail_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event_id,
                installation_id,
                host_id,
                actor_user_id,
                actor_email,
                event_type,
                detail_json,
                created_at
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "insert Client Market subscription event failed: {error}"
            ))
        })?;
        enqueue_client_market_chat_event_tx(
            tx,
            &event_id,
            installation_id,
            host_id,
            actor_user_id,
            actor_email,
            event_type,
            &detail,
            &created_at,
        )?;
    }
    Ok(())
}

fn client_label_tx(tx: &Transaction<'_>, installation_id: &str) -> Result<String, AppError> {
    Ok(tx
        .query_row(
            "SELECT subdomain FROM installation_client_tunnels WHERE installation_id = ?1",
            params![installation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read Client label failed: {error}")))?
        .unwrap_or_else(|| installation_id.to_string()))
}

pub(crate) fn persist_subscription_client_subdomain_tx(
    tx: &Transaction<'_>,
    installation_id: &str,
) -> Result<(), AppError> {
    let subdomain = client_label_tx(tx, installation_id)?;
    if subdomain == installation_id {
        return Ok(());
    }
    tx.execute(
        "UPDATE client_market_subscriptions
         SET client_subdomain = COALESCE(NULLIF(TRIM(client_subdomain), ''), ?2)
         WHERE installation_id = ?1",
        params![installation_id, subdomain],
    )
    .map_err(|error| {
        AppError::Internal(format!(
            "persist Client subscription subdomain failed: {error}"
        ))
    })?;
    Ok(())
}

pub(crate) fn complete_provisioning_tx(
    tx: &Transaction<'_>,
    job_id: &str,
    host_id: &str,
    installation_id: &str,
    _dashboard_url: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let context: Option<(
        String,
        String,
        String,
        String,
        Option<i64>,
        String,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<i64>,
        i64,
        String,
    )> = tx
        .query_row(
            "SELECT h.provider_id, h.host_owner_email, j.client_owner_email,
                    COALESCE(j.client_owner_user_id,
                             (SELECT u.id FROM users u WHERE u.email_normalized = LOWER(j.client_owner_email)),
                             'email:' || LOWER(j.client_owner_email)),
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.daily_rate_minor
                         ELSE h.daily_rate_minor END,
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.pricing_model
                         ELSE h.pricing_model END,
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.cycle_price_minor
                         ELSE h.cycle_price_minor END,
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.billing_interval
                         ELSE h.billing_interval END,
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.currency
                         WHEN h.daily_rate_minor IS NULL AND h.cycle_price_minor IS NULL THEN NULL
                         ELSE COALESCE(NULLIF(TRIM(h.currency), ''), 'USD') END,
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.free_duration_days
                         ELSE h.free_duration_days END,
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.offer_revision
                         ELSE h.offer_revision END,
                    COALESCE(q.funding_rail, 'legacy_provider')
             FROM provisioning_jobs j
             JOIN router_ssh_hosts h ON h.id = j.host_id
             LEFT JOIN client_market_allocation_quotes q ON q.id = j.quote_id
             LEFT JOIN client_market_allocation_quote_items qi
               ON qi.quote_id = j.quote_id AND qi.host_id = j.host_id
             WHERE j.id = ?1 AND h.id = ?2 AND h.provider_id IS NOT NULL
               AND (j.quote_id IS NULL OR
                    (qi.id IS NOT NULL AND q.id IS NOT NULL
                     AND qi.provider_id = h.provider_id))",
            params![job_id, host_id],
            |row| {
                Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                    row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?,
                    row.get(8)?, row.get(9)?, row.get(10)?, row.get(11)?,
                ))
            },
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read completed provisioning billing context failed: {error}")))?;
    let Some((
        provider_id,
        host_email,
        client_email,
        client_user_id,
        daily_rate_minor,
        pricing_model,
        cycle_price_minor,
        billing_interval,
        currency,
        free_duration_days,
        revision,
        funding_rail,
    )) = context
    else {
        return Err(AppError::Internal(
            "provisioned Host has no stable Provider identity".into(),
        ));
    };
    let (
        daily_rate_minor,
        pricing_model,
        cycle_price_minor,
        billing_interval,
        currency,
        free_duration_days,
    ) = if provider_id == client_user_id {
        (
            None,
            crate::market_common::PRICING_FREE.to_string(),
            None,
            None,
            None,
            free_duration_days,
        )
    } else {
        (
            daily_rate_minor,
            pricing_model,
            cycle_price_minor,
            billing_interval,
            currency,
            free_duration_days,
        )
    };
    let wallet_contract_id =
        crate::market_platform_contracts::contract_for_product_tx(tx, "client_host", job_id)?;
    let is_paid = cycle_price_minor.or(daily_rate_minor).is_some();
    if is_paid {
        crate::market_payments::require_current_offer_rail(&funding_rail, "Client Host")?;
    }
    if (funding_rail == "platform_wallet" && is_paid) != wallet_contract_id.is_some()
        || (funding_rail != "platform_wallet" && wallet_contract_id.is_some())
    {
        return Err(AppError::Conflict(
            "Client provisioning funding rail does not match its contract".into(),
        ));
    }
    let free_usage_seconds = if daily_rate_minor.is_none() && cycle_price_minor.is_none() {
        match free_duration_days {
            Some(days) if provider_id != client_user_id => {
                let days = u32::try_from(days).map_err(|_| {
                    AppError::Internal("free Client Host duration is invalid".into())
                })?;
                let seconds = crate::market_common::claim_free_usage_tx(
                    tx,
                    &client_user_id,
                    &provider_id,
                    crate::market_access::PRODUCT_CLIENT_HOST,
                    host_id,
                    days,
                    &now.to_rfc3339(),
                )?;
                if seconds == 0 {
                    return Err(AppError::Conflict(
                        "the free experience for this Provider has already been used".into(),
                    ));
                }
                Some(seconds)
            }
            Some(days) => Some(days.checked_mul(86_400).ok_or_else(|| {
                AppError::Internal("free Client Host duration overflowed".into())
            })?),
            None => None,
        }
    } else {
        None
    };
    let expires_at = free_usage_seconds.map(|seconds| now + Duration::seconds(seconds));
    let status = SUBSCRIPTION_ACTIVE;
    tx.execute(
        "INSERT INTO client_market_subscriptions
            (installation_id, host_id, provider_id, host_owner_email,
             client_user_id, client_owner_email, status, daily_rate_minor,
             currency, free_duration_days, offer_revision, activated_at, expires_at,
             created_at, updated_at, free_usage_seconds, pricing_model,
             cycle_price_minor, billing_interval)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?12, ?12, ?14, ?15, ?16, ?17)",
        params![
            installation_id,
            host_id,
            provider_id,
            host_email,
            client_user_id,
            client_email,
            status,
            daily_rate_minor,
            currency,
            free_duration_days,
            revision,
            now.to_rfc3339(),
            expires_at.map(|value| value.to_rfc3339()),
            free_usage_seconds,
            pricing_model,
            cycle_price_minor,
            billing_interval,
        ],
    )
    .map_err(|error| {
        AppError::Internal(format!("create Client Market subscription failed: {error}"))
    })?;
    persist_subscription_client_subdomain_tx(tx, installation_id)?;
    let mut activated_trial_seconds = 0_i64;
    if funding_rail == "platform_wallet"
        && let Some(_price_minor) = cycle_price_minor.or(daily_rate_minor)
    {
        if currency.as_deref() != Some("USD") {
            return Err(AppError::Conflict(
                "wallet-funded Client installation is not priced in USD".into(),
            ));
        }
        let contract_id = wallet_contract_id.ok_or_else(|| {
            AppError::Conflict("Client provisioning has no wallet reservation".into())
        })?;
        crate::market_platform_contracts::bind_client_installation_tx(
            tx,
            &contract_id,
            job_id,
            installation_id,
            &client_user_id,
            &provider_id,
            &now.to_rfc3339(),
        )?;
        crate::market_platform_contracts::activate_contract_tx(
            tx,
            &contract_id,
            &now.to_rfc3339(),
            &now.to_rfc3339(),
        )?;
        activated_trial_seconds = tx
            .query_row(
                "SELECT trial_seconds_remaining FROM market_platform_contracts WHERE id = ?1",
                params![contract_id],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("read activated Client trial failed: {error}"))
            })?;
        tx.execute(
            "UPDATE client_market_subscriptions
             SET platform_contract_id = ?2 WHERE installation_id = ?1",
            params![installation_id, contract_id],
        )
        .map_err(|error| {
            AppError::Internal(format!("link Client wallet contract failed: {error}"))
        })?;
    }
    insert_audit_tx(
        tx,
        Some(installation_id),
        Some(host_id),
        Some(&client_user_id),
        Some(&client_email),
        "client_provisioned",
        serde_json::json!({
            "providerId": provider_id,
            "hostOwnerEmail": host_email,
            "dailyRateMinor": daily_rate_minor,
            "pricingModel": pricing_model,
            "cyclePriceMinor": cycle_price_minor,
            "billingInterval": billing_interval,
            "currency": currency,
            "freeDurationDays": free_duration_days,
            "offerRevision": revision,
            "activatedAt": now.to_rfc3339(),
            "expiresAt": expires_at.map(|value| value.to_rfc3339()),
            "trialHours": activated_trial_seconds as f64 / 3_600.0,
            "trialSeconds": activated_trial_seconds,
        }),
        now,
    )?;
    Ok(())
}

pub(crate) fn cleanup_started_tx(
    tx: &Transaction<'_>,
    installation_id: &str,
    host_id: &str,
    actor_user_id: Option<&str>,
    actor_email: Option<&str>,
    reason: &str,
    deny_client_access: bool,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let subscription: Option<(
        String,
        String,
        String,
        String,
        Option<i64>,
        Option<String>,
        Option<i64>,
    )> = tx
        .query_row(
            "SELECT s.provider_id, s.client_user_id, s.client_owner_email,
                    COALESCE(p.owner_email, s.host_owner_email), s.daily_rate_minor,
                    s.expires_at, s.cycle_price_minor
             FROM client_market_subscriptions s
             LEFT JOIN host_provider_profiles p ON p.provider_id = s.provider_id
             WHERE s.installation_id = ?1",
            params![installation_id],
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
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("read cleanup subscription failed: {error}"))
        })?;
    if let Some((
        provider_id,
        client_user_id,
        client_owner_email,
        provider_email,
        daily_rate_minor,
        expires_at,
        cycle_price_minor,
    )) = subscription
    {
        let wallet_contract_id: Option<String> = tx
            .query_row(
                "SELECT platform_contract_id FROM client_market_subscriptions
                 WHERE installation_id = ?1",
                params![installation_id],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("read Client wallet contract failed: {error}"))
            })?;
        if wallet_contract_id.is_some() {
            crate::client_market_termination::accept_tx(
                tx,
                installation_id,
                actor_user_id,
                reason,
                None,
                &now.to_rfc3339(),
            )?;
        }
        tx.execute(
            "UPDATE client_market_subscriptions
             SET status = ?2, updated_at = ?3 WHERE installation_id = ?1 AND status != 'released'",
            params![installation_id, SUBSCRIPTION_RELEASING, now.to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("mark subscription releasing failed: {error}"))
        })?;
        if reason == "free_period_expired" {
            insert_audit_tx(
                tx,
                Some(installation_id),
                Some(host_id),
                None,
                None,
                "free_period_expired",
                serde_json::json!({ "expiresAt": expires_at }),
                now,
            )?;
        }
        if deny_client_access {
            crate::market_access::set_product_access_decision_tx(
                tx,
                &provider_id,
                &provider_email,
                &client_user_id,
                &client_owner_email,
                crate::market_access::PRODUCT_CLIENT_HOST,
                crate::market_access::pricing_kind_for_rate(cycle_price_minor.or(daily_rate_minor)),
                crate::market_access::DECISION_DENY,
                actor_user_id.unwrap_or(&provider_id),
                &now.to_rfc3339(),
            )?;
        }
    }
    persist_subscription_client_subdomain_tx(tx, installation_id)?;
    tx.execute(
        "UPDATE installation_client_tunnels SET enabled = 0, updated_at = ?2
         WHERE installation_id = ?1",
        params![installation_id, now.to_rfc3339()],
    )
    .map_err(|error| {
        AppError::Internal(format!("disable releasing Client tunnel failed: {error}"))
    })?;
    insert_audit_tx(
        tx,
        Some(installation_id),
        Some(host_id),
        actor_user_id,
        actor_email,
        "cleanup_started",
        serde_json::json!({
            "reason": reason,
            "providerDeniedClientAccess": deny_client_access,
        }),
        now,
    )?;
    Ok(())
}

pub(crate) fn cleanup_finished_tx(
    tx: &Transaction<'_>,
    installation_id: &str,
    host_id: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    crate::client_market_termination::record_cleanup_confirmed_tx(
        tx,
        installation_id,
        host_id,
        &now.to_rfc3339(),
    )?;
    tx.execute(
        "UPDATE client_market_subscriptions
         SET status = ?2, released_at = COALESCE(released_at, ?3), updated_at = ?3
         WHERE installation_id = ?1",
        params![installation_id, SUBSCRIPTION_RELEASED, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("finish subscription release failed: {error}")))?;
    insert_audit_tx(
        tx,
        Some(installation_id),
        Some(host_id),
        None,
        None,
        "cleanup_finished",
        serde_json::json!({}),
        now,
    )?;
    Ok(())
}

pub(crate) fn cleanup_failed_tx(
    tx: &Transaction<'_>,
    installation_id: &str,
    host_id: &str,
    failure_code: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    crate::client_market_termination::cleanup_failed_tx(tx, installation_id, &now.to_rfc3339())?;
    // Billing and the Client tunnel were already stopped when cleanup began.
    // A remote, network, or safety failure cannot keep the renter locked to an
    // ended contract. The Host remains bound and quarantined until verified clean.
    let changed = tx
        .execute(
            "UPDATE client_market_subscriptions
         SET status = ?2, released_at = ?3, updated_at = ?3
         WHERE installation_id = ?1 AND status != 'released'",
            params![installation_id, SUBSCRIPTION_RELEASED, now.to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("mark subscription release failed: {error}"))
        })?;
    if changed == 1 {
        insert_audit_tx(
            tx,
            Some(installation_id),
            Some(host_id),
            None,
            None,
            "subscription_release_finalized",
            serde_json::json!({
                "reason": "cleanup_failed",
                "remoteCleanupBypassed": true,
                "hostRemainsQuarantined": true,
            }),
            now,
        )?;
    }
    insert_audit_tx(
        tx,
        Some(installation_id),
        Some(host_id),
        None,
        None,
        "cleanup_failed",
        serde_json::json!({
            "failureCode": failure_code.split(':').next().unwrap_or(failure_code),
            "rawError": failure_code,
        }),
        now,
    )?;
    Ok(())
}

pub async fn run_trade_service(state: ServerState) -> anyhow::Result<()> {
    crate::client_market::reconcile_interrupted_host_import_jobs(state.clone()).await?;
    run_trade_reconcile_worker(state).await
}

async fn run_trade_reconcile_worker(state: ServerState) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(StdDuration::from_secs(TRADE_RECONCILE_CYCLE_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let expired_installations = match state
            .store
            .client_market_reconcile_trade_state(Utc::now())
            .await
        {
            Ok(installations) => installations,
            Err(error) => {
                tracing::warn!(error = %error, "Client Market trade reconcile failed");
                Vec::new()
            }
        };
        for installation_id in expired_installations {
            if let Err(error) = crate::client_market::terminate_for_billing(
                &state,
                &installation_id,
                "free_period_expired",
            )
            .await
            {
                tracing::warn!(
                    installation_id,
                    error = %error,
                    "expired free Client cleanup could not start"
                );
            }
        }
        if let Err(error) = state
            .store
            .client_market_record_provider_daily_stats(Utc::now(), state.config.client_stale_secs)
            .await
        {
            tracing::warn!(error = %error, "Client Market Provider observation snapshot failed");
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn session(user_id: &str, email: &str) -> AuthSession {
        let now = Utc::now();
        AuthSession {
            session_id: format!("session-{user_id}"),
            user_id: user_id.into(),
            email: email.into(),
            auth_source_kind: "auth_device".into(),
            auth_source_id: format!("browser-{user_id}"),
            access_token_hash: format!("access-{user_id}"),
            refresh_token_hash: format!("refresh-{user_id}"),
            access_expires_at: now + Duration::hours(1),
            refresh_expires_at: now + Duration::days(30),
            created_at: now,
            last_used_at: now,
        }
    }

    fn recommendation_provider(
        provider_id: &str,
        official: bool,
        rank_position: Option<i64>,
        external_clients_over_30_days: i64,
    ) -> ProviderSummary {
        ProviderSummary {
            provider_id: provider_id.into(),
            market_provider_id: Some(format!("mp_{provider_id}")),
            rank_state: if rank_position.is_some() {
                "ranked".into()
            } else {
                "collecting".into()
            },
            rank_position,
            provider_score_bps: rank_position.map(|position| 10_000 - position),
            owner_email: format!("{provider_id}@example.com"),
            official,
            joined_at: "2026-01-01T00:00:00Z".into(),
            offer_stable_since: "2026-01-01T00:00:00Z".into(),
            host_total: 1,
            idle_total: 1,
            allocated_total: 0,
            allocation_rate: 0.0,
            free_host_total: 0,
            free_allocated_total: 0,
            paid_host_total: 1,
            paid_allocated_total: 0,
            external_client_owner_total: external_clients_over_30_days,
            external_clients_over_3_days: external_clients_over_30_days,
            external_clients_over_30_days,
            online_rate_30d: Some(1.0),
            anomalous_host_rate: 0.0,
            min_daily_rate_minor: Some(100),
            max_daily_rate_minor: Some(100),
            min_cycle_price_minor: None,
            max_cycle_price_minor: None,
            successful_allocations: 0,
            countries: Vec::new(),
        }
    }

    fn recommendation_response() -> ProviderSupplyResponse {
        ProviderSupplyResponse {
            router_owner_email: Some("official@example.com".into()),
            official_provider_id: Some("official".into()),
            recommendation_mode: "off".into(),
            // This is the legacy order: official first, even though the ranked
            // Provider has more historical clients.
            providers: vec![
                recommendation_provider("official", true, None, 0),
                recommendation_provider("ranked", false, Some(1), 20),
            ],
        }
    }

    #[test]
    fn recommendation_modes_preserve_legacy_order_until_explicitly_enabled() {
        for mode in [
            crate::config::MarketProviderRecommendationMode::Off,
            crate::config::MarketProviderRecommendationMode::Shadow,
        ] {
            let mut response = recommendation_response();
            apply_provider_recommendation_mode(&mut response, mode);
            assert_eq!(
                response
                    .providers
                    .iter()
                    .map(|provider| provider.provider_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["official", "ranked"]
            );
            assert_eq!(response.recommendation_mode, mode.as_str());
        }

        let mut response = recommendation_response();
        apply_provider_recommendation_mode(
            &mut response,
            crate::config::MarketProviderRecommendationMode::On,
        );
        assert_eq!(
            response
                .providers
                .iter()
                .map(|provider| provider.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["ranked", "official"]
        );
    }

    #[test]
    fn official_identity_does_not_break_ranked_mode_ties() {
        let mut providers = [
            recommendation_provider("official", true, Some(1), 0),
            recommendation_provider("chosen", false, Some(1), 20),
        ];
        providers.sort_by(ranked_provider_order);
        assert_eq!(providers[0].provider_id, "chosen");

        providers.sort_by(legacy_provider_order);
        assert_eq!(providers[0].provider_id, "official");
    }

    #[tokio::test]
    async fn wallet_contacts_are_independent_of_payment_and_finance_mode() {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let supplier = session("contact-provider", "contact@example.com");
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(
                &conn,
                &supplier,
                "USD",
                0,
                &Utc::now().to_rfc3339(),
            );
        }
        for mode in ["disabled", "shadow", "enabled"] {
            store
                .conn
                .lock()
                .await
                .execute(
                    "UPDATE market_finance_control SET mode = ?1 WHERE id = 1",
                    params![mode],
                )
                .unwrap();
            let updated = store
                .client_market_update_provider_contacts(
                    &supplier,
                    &[ProviderContact {
                        channel: "telegram".into(),
                        handle: format!("@{mode}"),
                    }],
                )
                .await
                .unwrap();
            assert_eq!(updated.contacts[0].handle, format!("@{mode}"));
            assert!(
                serde_json::to_value(updated)
                    .unwrap()
                    .get("methods")
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn paid_host_requires_verified_claimed_provider_not_legacy_supplier_terms() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let supplier = session("supplier-with-payment", "supplier@example.com");

        let conn = store.conn.lock().await;
        assert!(matches!(
            require_paid_offer_setup(&conn, &supplier.user_id, "USD"),
            Err(AppError::Forbidden(_))
        ));
        crate::market_access::configure_open_test_policy(
            &conn,
            &supplier,
            "USD",
            50_000,
            &Utc::now().to_rfc3339(),
        );
        require_paid_offer_setup(&conn, &supplier.user_id, "USD").unwrap();
    }

    #[tokio::test]
    async fn client_market_audit_redacts_credential_fragments_before_persistence() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let conn = store.conn.lock().await;
        let tx = conn
            .transaction()
            .expect("begin audit redaction transaction");
        insert_audit_tx(
            &tx,
            None,
            None,
            None,
            None,
            "safety_audit",
            serde_json::json!({ "rawError": "Authorization: Bearer fake-audit-secret" }),
            Utc::now(),
        )
        .expect("insert sanitized Client Market audit");
        tx.commit().expect("commit sanitized Client Market audit");
        let detail: String = conn
            .query_row(
                "SELECT detail_json FROM client_market_audit_events
                 WHERE event_type = 'safety_audit'",
                [],
                |row| row.get(0),
            )
            .expect("read sanitized Client Market audit");
        assert!(!detail.contains("fake-audit-secret"));
        assert!(detail.contains("[credential omitted]"));
    }

    #[tokio::test]
    async fn client_market_audit_producer_enqueues_only_the_public_market_projection() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let provider = session("provider-chat-projection", "provider-chat@example.com");
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(
                &conn,
                &provider,
                "USD",
                0,
                &Utc::now().to_rfc3339(),
            );
        }
        let contacts = [ProviderContact {
            channel: "telegram".into(),
            handle: "@public-provider".into(),
        }];
        store
            .client_market_update_provider_contacts(&provider, &contacts)
            .await
            .expect("configure projected Provider support contacts");

        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO client_market_subscriptions (
                installation_id, host_id, provider_id, host_owner_email,
                client_user_id, client_owner_email, status, daily_rate_minor,
                currency, offer_revision, created_at, updated_at
             ) VALUES ('client-chat-projection', 'host-chat-projection', ?1, ?2,
                       'private-client-user', 'private-client@example.com',
                       'release_failed', 125, 'USD', 7, ?3, ?3)",
            params![provider.user_id, provider.email, now_text],
        )
        .expect("insert projected Client Market subscription");
        let tx = conn
            .transaction()
            .expect("begin projected Client Market event");
        insert_audit_tx(
            &tx,
            Some("client-chat-projection"),
            Some("host-chat-projection"),
            Some("private-actor-user"),
            Some("private-actor@example.com"),
            "cleanup_failed",
            serde_json::json!({
                "failureCode": "cleanup_timeout",
                "reason": "cleanup did not finish",
                "buyerBalanceMinor": 900,
                "paymentReference": "private-payment-reference",
                "evidenceUrl": "https://evidence.example.com/private-receipt",
                "rawError": "private raw cleanup diagnostics",
            }),
            now,
        )
        .expect("produce Client Market chat event");
        tx.commit().expect("commit projected Client Market event");

        let payload_json: String = conn
            .query_row(
                "SELECT payload_json FROM client_chat_system_outbox
                 WHERE source_kind = 'client_market'
                   AND installation_id = 'client-chat-projection'",
                [],
                |row| row.get(0),
            )
            .expect("read Client Market chat outbox payload");
        let payload: serde_json::Value =
            serde_json::from_str(&payload_json).expect("parse Client Market chat payload");
        assert_eq!(payload["marketKind"], "client");
        assert_eq!(payload["supplierEmail"], provider.email);
        assert_eq!(payload["providerEmail"], provider.email);
        assert_eq!(payload["dailyRateMinor"], 125);
        assert_eq!(payload["offerRevision"], 7);
        assert_eq!(payload["failureCode"], "cleanup_timeout");
        assert!(payload.get("paymentMethods").is_none());
        assert_eq!(payload["contacts"], serde_json::to_value(contacts).unwrap());
        for private_field in [
            "clientUserId",
            "clientOwnerEmail",
            "providerUserId",
            "actorUserId",
            "actorEmail",
            "hostId",
            "buyerBalanceMinor",
            "paymentReference",
            "evidenceUrl",
            "rawError",
            "paymentMethods",
            "paymentContacts",
        ] {
            assert!(
                payload.get(private_field).is_none(),
                "private field leaked from Client Market producer: {private_field}"
            );
        }
    }

    #[tokio::test]
    async fn renter_controls_expiring_provider_terminal_authorization() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO router_ssh_hosts (
                    id, ip, port, host_owner_email, status, installation_id,
                    created_at, updated_at, provider_id, offer_revision
                 ) VALUES (
                    'host-terminal', '203.0.113.20', 22, 'provider@example.com',
                    'allocated', 'installation-terminal', ?1, ?1, 'provider-terminal', 1
                 )",
                params![now],
            )
            .expect("insert terminal Host");
            conn.execute(
                "INSERT INTO client_market_subscriptions (
                    installation_id, host_id, provider_id, host_owner_email,
                    client_user_id, client_owner_email, status, offer_revision,
                    created_at, updated_at
                 ) VALUES (
                    'installation-terminal', 'host-terminal', 'provider-terminal',
                    'provider@example.com', 'renter-terminal', 'renter@example.com',
                    'active', 1, ?1, ?1
                 )",
                params![now],
            )
            .expect("insert terminal rental");
        }

        let renter = session("renter-terminal", "renter@example.com");
        let provider = session("provider-terminal", "provider@example.com");
        assert!(
            store
                .client_market_grant_provider_terminal_authorization(
                    "installation-terminal",
                    &provider,
                    60,
                )
                .await
                .is_err()
        );

        let authorization = store
            .client_market_grant_provider_terminal_authorization(
                "installation-terminal",
                &renter,
                60,
            )
            .await
            .expect("grant terminal authorization");
        assert!(authorization.active);
        assert!(authorization.expires_at.is_some());
        assert!(
            store
                .client_market_provider_terminal_authorized_until(
                    "installation-terminal",
                    "host-terminal",
                    "provider-terminal",
                )
                .await
                .expect("check terminal authorization")
                .is_some()
        );
        assert!(
            store
                .client_market_provider_terminal_authorized_host_ids("provider-terminal")
                .await
                .expect("list authorized terminal Hosts")
                .contains("host-terminal")
        );
        let rental = store
            .client_market_rental_for_viewer("installation-terminal", &renter)
            .await
            .expect("read authorized rental");
        assert!(rental.provider_terminal_access_active);
        assert!(rental.can_manage_provider_terminal);

        store
            .client_market_revoke_provider_terminal_authorization("installation-terminal", &renter)
            .await
            .expect("revoke terminal authorization");
        assert!(
            store
                .client_market_provider_terminal_authorized_until(
                    "installation-terminal",
                    "host-terminal",
                    "provider-terminal",
                )
                .await
                .expect("check revoked terminal authorization")
                .is_none()
        );
        let rental = store
            .client_market_rental_for_viewer("installation-terminal", &renter)
            .await
            .expect("read revoked rental");
        assert!(!rental.provider_terminal_access_active);
    }

    #[tokio::test]
    async fn provider_daily_online_samples_require_activation_and_fresh_heartbeat() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let stale_heartbeat = (now - Duration::seconds(901)).to_rfc3339();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO host_provider_profiles (
                    provider_id, owner_email, created_at, updated_at
                 ) VALUES ('provider-heartbeat', 'provider@example.com', ?1, ?1)",
                params![now_text],
            )
            .expect("insert heartbeat Provider");

            for (installation_id, activated_at, heartbeat_at) in [
                ("inst-online", Some(now_text.as_str()), now_text.as_str()),
                (
                    "inst-stale",
                    Some(now_text.as_str()),
                    stale_heartbeat.as_str(),
                ),
                ("inst-unactivated", None, now_text.as_str()),
            ] {
                conn.execute(
                    "INSERT INTO installations (
                        id, public_key, platform, app_version, owner_email, owner_verified_at,
                        created_at, last_seen_at, client_activated_at, control_secret_b64
                     ) VALUES (?1, ?2, 'linux', '1.0.0', 'client@example.com', ?3,
                               ?3, ?3, ?4, ?5)",
                    params![
                        installation_id,
                        format!("key-{installation_id}"),
                        now_text,
                        activated_at,
                        format!("secret-{installation_id}"),
                    ],
                )
                .expect("insert Provider sample installation");
                conn.execute(
                    "INSERT INTO installation_notification_state (
                        installation_id, monitoring_enabled, presence_state, last_heartbeat_at,
                        created_at, updated_at
                     ) VALUES (?1, 1, 'online', ?2, ?3, ?3)",
                    params![installation_id, heartbeat_at, now_text],
                )
                .expect("insert Provider sample heartbeat");
                conn.execute(
                    "INSERT INTO client_market_subscriptions (
                        installation_id, host_id, provider_id, host_owner_email,
                        client_user_id, client_owner_email, status, offer_revision,
                        created_at, updated_at
                     ) VALUES (?1, ?2, 'provider-heartbeat', 'provider@example.com',
                               'external-client', 'client@example.com', 'active', 1, ?3, ?3)",
                    params![installation_id, format!("host-{installation_id}"), now_text,],
                )
                .expect("insert Provider sample subscription");
            }
        }

        store
            .client_market_record_provider_daily_stats(now, 900)
            .await
            .expect("record Provider daily stats");

        let conn = store.conn.lock().await;
        let (online_samples, observed_samples): (i64, i64) = conn
            .query_row(
                "SELECT online_samples, observed_samples
                 FROM host_provider_daily_stats
                 WHERE provider_id = 'provider-heartbeat'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read Provider daily stats");
        assert_eq!(online_samples, 1);
        assert_eq!(observed_samples, 3);
    }

    #[test]
    fn paid_host_offer_defaults_to_usd_and_rejects_other_currencies() {
        assert_eq!(
            normalize_offer_currency(Some(500), None).expect("default paid Host currency"),
            Some("USD".into())
        );
        assert_eq!(
            normalize_offer_currency(Some(500), Some(" usd ".into()))
                .expect("normalize USD paid Host currency"),
            Some("USD".into())
        );
        assert!(normalize_offer_currency(Some(500), Some("CNY".into())).is_err());
    }
}
