use std::io::Cursor;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration as StdDuration;

use crate::db::{Connection, OptionalExtension, Transaction, params, params_from_iter};
use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use futures_util::StreamExt;
use image::{ImageFormat, ImageReader};
use rand::seq::SliceRandom;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::ServerState;
use crate::error::AppError;
use crate::models::AuthSession;
use crate::store::AppStore;

pub const HOST_STATUS_RESERVED: &str = "reserved";
const QUOTE_TTL_SECS: i64 = 120;
const MAX_CREATE_COUNT: usize = 2;
const MAX_PAYMENT_METHODS: usize = 20;
const MAX_CUSTOM_PAYMENT_CHARS: usize = 2_000;
const MAX_QR_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_QR_PIXELS: u64 = 4_000_000;
const MAX_QR_DIMENSION: u32 = 4_096;
const MAX_QR_STORED_BYTES: usize = 4 * 1024 * 1024;
const TRADE_RECONCILE_CYCLE_SECS: u64 = 20;
pub(crate) const MAX_FREE_DURATION_DAYS: u32 = 365;

const SUBSCRIPTION_ACTIVE: &str = "active";
const SUBSCRIPTION_RELEASING: &str = "releasing";
const SUBSCRIPTION_RELEASE_FAILED: &str = "release_failed";
const SUBSCRIPTION_RELEASED: &str = "released";
const MAX_PAYMENT_CONTACTS: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PaymentContact {
    /// `wechat` | `telegram` | `custom`
    pub channel: String,
    pub handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PaymentMethod {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qr_image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentProfileView {
    pub provider_id: String,
    pub owner_email: String,
    pub methods: Vec<PaymentMethod>,
    #[serde(default)]
    pub contacts: Vec<PaymentContact>,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePaymentProfileRequest {
    #[serde(default)]
    pub methods: Vec<PaymentMethod>,
    /// When omitted, existing contacts are preserved (backward compatible).
    #[serde(default)]
    pub contacts: Option<Vec<PaymentContact>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSummary {
    pub provider_id: String,
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
    pub successful_allocations: i64,
    pub payment_method_kinds: Vec<String>,
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
    pub providers: Vec<ProviderSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateHostOfferRequest {
    pub daily_rate_minor: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub free_duration_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostOfferView {
    pub host_id: String,
    pub daily_rate_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_duration_days: Option<u32>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_duration_days: Option<u32>,
    pub offer_revision: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationQuoteView {
    pub id: String,
    pub status: String,
    pub expires_at: String,
    pub items: Vec<QuoteItemView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitQuoteRequest {
    pub items: Vec<CommitQuoteItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitClientBatchRequest {
    pub quote_id: String,
    pub items: Vec<CommitQuoteItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitQuoteItem {
    pub quote_item_id: String,
    pub offer_revision: i64,
    pub subdomain: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitQuoteResponse {
    pub batch_id: String,
    pub job_ids: Vec<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_duration_days: Option<u32>,
    pub offer_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub payment_method_kinds: Vec<String>,
    #[serde(default)]
    pub contacts: Vec<PaymentContact>,
    pub is_client_owner: bool,
    pub can_release: bool,
    /// Latest pending/running cleanup job for this installation (page-refresh resume).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_cleanup_job_id: Option<String>,
    pub updated_at: String,
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/account/payment-profile",
            get(get_payment_profile).put(update_payment_profile),
        )
        .route("/v1/account/payment-assets/:id", get(get_payment_asset))
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
        .route("/v1/client-market/my-rentals", get(list_my_rentals))
        .route("/v1/client-market/clients", post(commit_client_batch))
        .route(
            "/v1/client-market/clients/:installation_id/rental",
            get(get_client_rental),
        )
        .route(
            "/v1/admin/client-market/subscriptions/:installation_id/force-release",
            post(admin_force_release_subscription),
        )
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

pub(crate) const PAYMENT_PROFILE_REQUIRED_FOR_OFFER: &str =
    "configure payment details on the Account page before setting a Host offer";

pub(crate) fn validate_offer(daily_rate_minor: Option<i64>) -> Result<Option<i64>, AppError> {
    match daily_rate_minor {
        None | Some(0) => Ok(None),
        Some(rate) if (1..=crate::market_billing::MAX_DAILY_RATE_MINOR).contains(&rate) => {
            Ok(Some(rate))
        }
        _ => Err(AppError::BadRequest(
            "paid Hosts require dailyRateMinor between 1 and 100000000; omit it or use zero for free".into(),
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
        (Some(_), None) => Ok(Some(crate::market_billing::MARKET_CURRENCY.into())),
        (Some(_), Some(crate::market_billing::MARKET_CURRENCY)) => Ok(normalized),
        (Some(_), Some(_)) => Err(AppError::BadRequest("currency must be USD".into())),
    }
}

fn payment_profile_has_methods(conn: &Connection, provider_id: &str) -> Result<bool, AppError> {
    let methods_json: Option<String> = conn
        .query_row(
            "SELECT methods_json FROM account_payment_profiles WHERE user_id = ?1",
            params![provider_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("read payment profile for offer failed: {error}"))
        })?;
    let Some(methods_json) = methods_json else {
        return Ok(false);
    };
    let methods: Vec<PaymentMethod> = serde_json::from_str(&methods_json).unwrap_or_default();
    Ok(!methods.is_empty())
}

pub(crate) fn require_payment_profile_for_offer(
    conn: &Connection,
    provider_id: &str,
) -> Result<(), AppError> {
    if payment_profile_has_methods(conn, provider_id)? {
        return Ok(());
    }
    Err(AppError::BadRequest(
        PAYMENT_PROFILE_REQUIRED_FOR_OFFER.into(),
    ))
}

fn ensure_payment_profile_can_be_cleared(
    conn: &Connection,
    supplier_user_id: &str,
) -> Result<(), AppError> {
    let has_dependency =
        conn.query_row(
            "SELECT
                EXISTS(
                    SELECT 1 FROM router_ssh_hosts
                    WHERE provider_id = ?1 AND daily_rate_minor IS NOT NULL
                ) OR EXISTS(
                    SELECT 1
                    FROM share_market_seats seat
                    JOIN share_market_listings listing ON listing.id = seat.listing_id
                    WHERE listing.owner_user_id = ?1
                      AND listing.status = 'active' AND listing.deleted_at IS NULL
                      AND seat.daily_rate_minor IS NOT NULL AND seat.retired_at IS NULL
                      AND seat.status != 'deleted'
                ) OR EXISTS(
                    SELECT 1 FROM market_service_contracts
                    WHERE supplier_user_id = ?1 AND status != 'terminated'
                ) OR EXISTS(
                    SELECT 1 FROM market_credit_accounts
                    WHERE supplier_user_id = ?1 AND balance_units > 0
                ) OR EXISTS(
                    SELECT 1
                    FROM market_invoices invoice
                    JOIN market_credit_accounts account ON account.id = invoice.account_id
                    WHERE account.supplier_user_id = ?1
                      AND invoice.status IN ('open', 'payment_declared', 'overdue', 'disputed')
                )",
            params![supplier_user_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "check payment profile market dependencies failed: {error}"
            ))
        })? != 0;
    if has_dependency {
        return Err(AppError::Conflict(
            "payment methods are required while paid offers, service contracts, accrued balances, or unsettled bills exist"
                .into(),
        ));
    }
    Ok(())
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

fn normalize_payment_method(mut method: PaymentMethod) -> Result<PaymentMethod, AppError> {
    method.kind = method.kind.trim().to_ascii_lowercase();
    method.account = clean_optional(method.account, 200)?;
    method.qr_image_url = clean_optional(method.qr_image_url, 2_000)?;
    method.asset_url = None;
    method.token = clean_optional(method.token, 10)?.map(|value| value.to_ascii_uppercase());
    method.chain = clean_optional(method.chain, 20)?.map(|value| value.to_ascii_lowercase());
    method.address = clean_optional(method.address, 300)?;
    method.instructions = clean_payment_instructions(method.instructions)?;
    match method.kind.as_str() {
        "alipay" => {
            if method.account.is_none() && method.qr_image_url.is_none() {
                return Err(AppError::BadRequest(
                    "Alipay requires a phone/account or QR image URL".into(),
                ));
            }
            method.token = None;
            method.chain = None;
            method.address = None;
            method.instructions = None;
        }
        "wechat" => {
            if method.qr_image_url.is_none() {
                return Err(AppError::BadRequest(
                    "WeChat requires a QR image URL".into(),
                ));
            }
            method.account = None;
            method.token = None;
            method.chain = None;
            method.address = None;
            method.instructions = None;
        }
        "binance" => {
            if method.account.is_none() && method.qr_image_url.is_none() {
                return Err(AppError::BadRequest(
                    "Binance requires a user ID or QR image URL".into(),
                ));
            }
            method.token = None;
            method.chain = None;
            method.address = None;
            method.instructions = None;
        }
        "crypto" => {
            let token = method.token.as_deref().unwrap_or_default();
            let chain = method.chain.as_deref().unwrap_or_default();
            if !matches!(token, "USDT" | "USDC")
                || !matches!(chain, "bsc" | "base" | "eth" | "tron")
                || method.address.is_none()
            {
                return Err(AppError::BadRequest(
                    "crypto requires USDT or USDC, a bsc/base/eth/tron chain, and an address"
                        .into(),
                ));
            }
            validate_crypto_address(chain, method.address.as_deref().unwrap_or_default())?;
            method.account = None;
            method.qr_image_url = None;
            method.instructions = None;
        }
        "custom" => {
            if method.instructions.is_none() {
                return Err(AppError::BadRequest(
                    "custom payment requires instructions".into(),
                ));
            }
            method.account = None;
            method.qr_image_url = None;
            method.token = None;
            method.chain = None;
            method.address = None;
        }
        _ => {
            return Err(AppError::BadRequest(
                "unsupported payment method kind".into(),
            ));
        }
    }
    Ok(method)
}

fn normalize_payment_contact(mut contact: PaymentContact) -> Result<PaymentContact, AppError> {
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

fn normalize_payment_contacts(
    contacts: Vec<PaymentContact>,
) -> Result<Vec<PaymentContact>, AppError> {
    if contacts.len() > MAX_PAYMENT_CONTACTS {
        return Err(AppError::BadRequest(
            "at most 10 contact entries are allowed".into(),
        ));
    }
    let mut output = Vec::with_capacity(contacts.len());
    for contact in contacts {
        let contact = normalize_payment_contact(contact)?;
        if output.iter().any(|existing: &PaymentContact| {
            existing.channel == contact.channel && existing.handle == contact.handle
        }) {
            return Err(AppError::BadRequest("duplicate contact entry".into()));
        }
        output.push(contact);
    }
    Ok(output)
}

fn validate_crypto_address(chain: &str, address: &str) -> Result<(), AppError> {
    let valid = if matches!(chain, "bsc" | "base" | "eth") {
        address.len() == 42
            && address.starts_with("0x")
            && address[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
            && address[2..].bytes().any(|byte| byte != b'0')
    } else {
        bs58::decode(address)
            .with_check(None)
            .into_vec()
            .is_ok_and(|decoded| {
                decoded.len() == 21
                    && decoded[0] == 0x41
                    && decoded[1..].iter().any(|byte| *byte != 0)
            })
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "invalid {chain} payment address"
        )))
    }
}

fn clean_optional(value: Option<String>, max: usize) -> Result<Option<String>, AppError> {
    let value = value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty());
    if value
        .as_ref()
        .is_some_and(|item| item.chars().count() > max || item.chars().any(char::is_control))
    {
        return Err(AppError::BadRequest(
            "payment field is invalid or too long".into(),
        ));
    }
    Ok(value)
}

fn clean_payment_instructions(value: Option<String>) -> Result<Option<String>, AppError> {
    let value = value
        .map(|item| item.replace("\r\n", "\n").replace('\r', "\n"))
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty());
    if value.as_ref().is_some_and(|item| {
        item.chars().count() > MAX_CUSTOM_PAYMENT_CHARS
            || item
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
    }) {
        return Err(AppError::BadRequest(
            "custom payment instructions are invalid or too long".into(),
        ));
    }
    Ok(value)
}

async fn get_payment_profile(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<PaymentProfileView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_payment_profile(&session.user_id, &session.email)
            .await?,
    ))
}

async fn update_payment_profile(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdatePaymentProfileRequest>,
) -> Result<Json<PaymentProfileView>, AppError> {
    let session = require_session(&state, &headers).await?;
    if input.methods.len() > MAX_PAYMENT_METHODS {
        return Err(AppError::BadRequest(
            "at most 20 payment methods are allowed".into(),
        ));
    }
    let mut methods = Vec::with_capacity(input.methods.len());
    let mut singleton_kinds = std::collections::HashSet::new();
    let mut crypto_count = 0_usize;
    for method in input.methods {
        let mut method = normalize_payment_method(method)?;
        if method.kind == "crypto" {
            crypto_count += 1;
            if crypto_count > 8 {
                return Err(AppError::BadRequest(
                    "at most 8 crypto payment addresses are allowed".into(),
                ));
            }
        } else if !singleton_kinds.insert(method.kind.clone()) {
            return Err(AppError::BadRequest(format!(
                "only one {} payment method is allowed",
                method.kind
            )));
        }
        if methods.iter().any(|existing: &PaymentMethod| {
            existing.kind == method.kind
                && existing.token == method.token
                && existing.chain == method.chain
                && existing.address == method.address
        }) {
            return Err(AppError::BadRequest("duplicate payment method".into()));
        }
        if let Some(source_url) = method.qr_image_url.as_deref() {
            let asset_id = cache_qr_asset(&state, &session.user_id, source_url).await?;
            method.asset_url = Some(format!("/v1/account/payment-assets/{asset_id}"));
        }
        methods.push(method);
    }
    let contacts = match input.contacts {
        Some(contacts) => Some(normalize_payment_contacts(contacts)?),
        None => None,
    };
    Ok(Json(
        state
            .store
            .client_market_update_payment_profile(&session, &methods, contacts.as_deref())
            .await?,
    ))
}

async fn cache_qr_asset(
    state: &ServerState,
    user_id: &str,
    raw_url: &str,
) -> Result<String, AppError> {
    let url = validate_public_https_url(raw_url).await?;
    let host = url
        .host_str()
        .ok_or_else(|| AppError::BadRequest("QR image URL must have a host".into()))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let resolved = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| AppError::BadRequest("QR image host could not be resolved".into()))?
        .find(|address| is_public_ip(address.ip()))
        .ok_or_else(|| {
            AppError::BadRequest("QR image URL resolves to a private or reserved address".into())
        })?;
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .connect_timeout(StdDuration::from_secs(8))
        .timeout(StdDuration::from_secs(15))
        .resolve(host, SocketAddr::new(resolved.ip(), port))
        .build()
        .map_err(|error| AppError::Internal(format!("build QR fetch client failed: {error}")))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| AppError::BadRequest("QR image could not be fetched".into()))?;
    if !response.status().is_success() {
        return Err(AppError::BadRequest(
            "QR image URL did not return a successful response".into(),
        ));
    }
    let content_length = response.content_length().unwrap_or(0);
    if content_length > MAX_QR_SOURCE_BYTES as u64 {
        return Err(AppError::BadRequest(
            "QR image exceeds the 2 MB limit".into(),
        ));
    }
    let mut bytes = Vec::with_capacity((content_length as usize).min(MAX_QR_SOURCE_BYTES));
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk =
            chunk.map_err(|_| AppError::BadRequest("QR image body could not be read".into()))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_QR_SOURCE_BYTES {
            return Err(AppError::BadRequest(
                "QR image exceeds the 2 MB limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let reader = ImageReader::new(Cursor::new(bytes.as_slice()))
        .with_guessed_format()
        .map_err(|_| AppError::BadRequest("QR image format is invalid".into()))?;
    if !reader.format().is_some_and(is_supported_qr_format) {
        return Err(AppError::BadRequest(
            "QR image must be PNG, JPEG, or WebP".into(),
        ));
    }
    let dimensions = reader
        .into_dimensions()
        .map_err(|_| AppError::BadRequest("QR image is not a supported raster image".into()))?;
    if dimensions.0 > MAX_QR_DIMENSION
        || dimensions.1 > MAX_QR_DIMENSION
        || u64::from(dimensions.0) * u64::from(dimensions.1) > MAX_QR_PIXELS
    {
        return Err(AppError::BadRequest(
            "QR image dimensions are too large".into(),
        ));
    }
    let image = ImageReader::new(Cursor::new(bytes.as_slice()))
        .with_guessed_format()
        .map_err(|_| AppError::BadRequest("QR image format is invalid".into()))?
        .decode()
        .map_err(|_| AppError::BadRequest("QR image is not a supported raster image".into()))?;
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|error| AppError::Internal(format!("encode QR image failed: {error}")))?;
    if png.get_ref().len() > MAX_QR_STORED_BYTES {
        return Err(AppError::BadRequest(
            "normalized QR image exceeds the 4 MB limit".into(),
        ));
    }
    state
        .store
        .client_market_store_payment_asset(user_id, raw_url, png.get_ref())
        .await
}

fn is_supported_qr_format(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    )
}

async fn validate_public_https_url(raw: &str) -> Result<Url, AppError> {
    let url =
        Url::parse(raw).map_err(|_| AppError::BadRequest("QR image URL is invalid".into()))?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::BadRequest(
            "QR image URL must use HTTPS without credentials".into(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AppError::BadRequest("QR image URL must have a host".into()))?
        .to_string();
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(AppError::BadRequest("QR image host is not public".into()));
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let mut addresses = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| AppError::BadRequest("QR image host could not be resolved".into()))?;
    if !addresses.any(|address| is_public_ip(address.ip())) {
        return Err(AppError::BadRequest(
            "QR image URL resolves to a private or reserved address".into(),
        ));
    }
    Ok(url)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [first, second, third, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && first != 0
                && !(first == 100 && (64..=127).contains(&second))
                && !(first == 192 && second == 0 && third == 0)
                && !(first == 192 && second == 88 && third == 99)
                && !(first == 198 && matches!(second, 18 | 19))
                && first < 240
        }
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            (first & 0xe000) == 0x2000
                && !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
                && !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
        }
    }
}

async fn get_payment_asset(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, AppError> {
    let session = crate::api::resolve_router_session(&state, &headers).await?;
    let bytes = state
        .store
        .client_market_payment_asset_for_viewer(&id, session.as_ref())
        .await?;
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

async fn get_provider_supply(
    State(state): State<ServerState>,
) -> Result<Json<ProviderSupplyResponse>, AppError> {
    Ok(Json(
        state
            .store
            .client_market_provider_supply(state.config.official_provider_email())
            .await?,
    ))
}

async fn get_provider(
    State(state): State<ServerState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ProviderSummary>, AppError> {
    let supply = state
        .store
        .client_market_provider_supply(state.config.official_provider_email())
        .await?;
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
    let daily_rate_minor = validate_offer(input.daily_rate_minor)?;
    let currency = normalize_offer_currency(daily_rate_minor, input.currency)?;
    let free_duration_days =
        validate_free_duration_days(daily_rate_minor, input.free_duration_days)?;
    Ok(Json(
        state
            .store
            .client_market_update_host_offer(
                &id,
                &session,
                daily_rate_minor,
                currency,
                free_duration_days,
            )
            .await?,
    ))
}

async fn create_quote(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateQuoteRequest>,
) -> Result<Json<AllocationQuoteView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_create_quote(&session, input)
            .await?,
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
            CommitQuoteRequest { items: input.items },
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
        ));
    }
    let response = state
        .store
        .client_market_commit_quote(id, session, &prepared)
        .await?;
    {
        let mut secrets = state.client_market_job_secrets.lock().await;
        for (job_id, (_, _, password, _)) in response.job_ids.iter().zip(prepared.iter()) {
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
) -> Result<Json<Vec<RentalView>>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_list_rentals_for_viewer(&session)
            .await?,
    ))
}

async fn get_client_rental(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<Json<RentalView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_rental_for_viewer(&installation_id, &session)
            .await?,
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
        let conn = self.conn.lock().await;
        let now = Utc::now();
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Internal(format!("begin force release failed: {error}")))?;

        let (previous_status, host_id): (String, Option<String>) = tx
            .query_row(
                "SELECT status, host_id FROM client_market_subscriptions WHERE installation_id = ?1",
                params![installation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("load subscription failed: {error}")))?
            .ok_or_else(|| AppError::NotFound("subscription not found".into()))?;

        if previous_status != SUBSCRIPTION_RELEASE_FAILED
            && previous_status != SUBSCRIPTION_RELEASING
        {
            return Err(AppError::Conflict(format!(
                "subscription is {previous_status}; only releasing or release_failed can be force released"
            )));
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
            "subscription_force_released",
            serde_json::json!({
                "previousStatus": previous_status,
                "hostStatus": host_status,
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
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit Provider sync failed: {error}")))?;
        Ok(user_id.to_string())
    }

    pub async fn client_market_payment_profile(
        &self,
        user_id: &str,
        owner_email: &str,
    ) -> Result<PaymentProfileView, AppError> {
        let email = normalize_email(owner_email)?;
        let conn = self.conn.lock().await;
        let row: Option<(String, String, String, String)> = conn
            .query_row(
                "SELECT owner_email, methods_json, COALESCE(contacts_json, '[]'), updated_at
                 FROM account_payment_profiles WHERE user_id = ?1",
                params![user_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read payment profile failed: {error}")))?;
        if let Some((owner_email, methods_json, contacts_json, updated_at)) = row {
            return Ok(PaymentProfileView {
                provider_id: user_id.to_string(),
                owner_email,
                methods: serde_json::from_str(&methods_json).unwrap_or_default(),
                contacts: serde_json::from_str(&contacts_json).unwrap_or_default(),
                updated_at,
            });
        }
        Ok(PaymentProfileView {
            provider_id: user_id.to_string(),
            owner_email: email,
            methods: Vec::new(),
            contacts: Vec::new(),
            updated_at: Utc::now().to_rfc3339(),
        })
    }

    pub async fn client_market_update_payment_profile(
        &self,
        session: &AuthSession,
        methods: &[PaymentMethod],
        contacts: Option<&[PaymentContact]>,
    ) -> Result<PaymentProfileView, AppError> {
        let email = normalize_email(&session.email)?;
        let methods_json = serde_json::to_string(methods).map_err(|error| {
            AppError::Internal(format!("encode payment methods failed: {error}"))
        })?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin payment profile update failed: {error}"))
        })?;
        if methods.is_empty() {
            ensure_payment_profile_can_be_cleared(&tx, &session.user_id)?;
        }
        let existing_contacts_json: String = tx
            .query_row(
                "SELECT COALESCE(contacts_json, '[]') FROM account_payment_profiles WHERE user_id = ?1",
                params![session.user_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read existing contacts failed: {error}")))?
            .unwrap_or_else(|| "[]".to_string());
        let contacts_json = match contacts {
            Some(contacts) => serde_json::to_string(contacts).map_err(|error| {
                AppError::Internal(format!("encode payment contacts failed: {error}"))
            })?,
            None => existing_contacts_json,
        };
        tx.execute(
            "INSERT INTO account_payment_profiles (user_id, owner_email, methods_json, contacts_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(user_id) DO UPDATE SET
                owner_email = excluded.owner_email,
                methods_json = excluded.methods_json,
                contacts_json = excluded.contacts_json,
                updated_at = excluded.updated_at",
            params![session.user_id, email, methods_json, contacts_json, now],
        )
        .map_err(|error| AppError::Internal(format!("save payment profile failed: {error}")))?;
        tx.execute(
            "DELETE FROM account_payment_assets
             WHERE user_id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM json_each(?2) method
                   WHERE json_extract(method.value, '$.assetUrl') =
                         '/v1/account/payment-assets/' || account_payment_assets.id
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM market_invoices invoice
                   JOIN market_credit_accounts account ON account.id = invoice.account_id
                   WHERE account.supplier_user_id = ?1
                     AND EXISTS (
                         SELECT 1 FROM json_each(invoice.payment_methods_json) method
                         WHERE json_extract(method.value, '$.assetUrl') =
                               '/v1/account/payment-assets/' || account_payment_assets.id
                     )
               )
               AND NOT EXISTS (
                   SELECT 1 FROM chat_public_payment_assets published
                   WHERE published.asset_id = account_payment_assets.id
               )",
            params![session.user_id, methods_json],
        )
        .map_err(|error| {
            AppError::Internal(format!("remove unused payment assets failed: {error}"))
        })?;
        tx.execute(
            "DELETE FROM account_payment_methods WHERE profile_user_id = ?1",
            params![session.user_id],
        )
        .map_err(|error| AppError::Internal(format!("replace payment methods failed: {error}")))?;
        for (position, method) in methods.iter().enumerate() {
            let method_json = serde_json::to_string(method).map_err(|error| {
                AppError::Internal(format!("encode payment method failed: {error}"))
            })?;
            tx.execute(
                "INSERT INTO account_payment_methods (
                    id, profile_user_id, position, kind, method_json, enabled, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                params![
                    Uuid::new_v4().to_string(),
                    session.user_id,
                    position as i64,
                    method.kind,
                    method_json,
                    now
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("save normalized payment method failed: {error}"))
            })?;
        }
        tx.execute(
            "INSERT INTO host_provider_profiles (provider_id, owner_email, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(provider_id) DO UPDATE SET owner_email = excluded.owner_email, updated_at = excluded.updated_at",
            params![session.user_id, email, now],
        )
        .map_err(|error| AppError::Internal(format!("sync Provider profile failed: {error}")))?;
        tx.execute(
            "UPDATE router_ssh_hosts SET host_owner_email = ?2, updated_at = ?3
             WHERE provider_id = ?1",
            params![session.user_id, email, now],
        )
        .map_err(|error| AppError::Internal(format!("sync Host Provider email failed: {error}")))?;
        tx.execute(
            "UPDATE client_market_subscriptions SET host_owner_email = ?2, updated_at = ?3
             WHERE provider_id = ?1 AND status != 'released'",
            params![session.user_id, email, now],
        )
        .map_err(|error| {
            AppError::Internal(format!("sync subscription Provider email failed: {error}"))
        })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit payment profile update failed: {error}"))
        })?;
        Ok(PaymentProfileView {
            provider_id: session.user_id.clone(),
            owner_email: email,
            methods: methods.to_vec(),
            contacts: serde_json::from_str(&contacts_json).unwrap_or_default(),
            updated_at: now,
        })
    }

    pub async fn client_market_store_payment_asset(
        &self,
        user_id: &str,
        source_url: &str,
        png: &[u8],
    ) -> Result<String, AppError> {
        use sha2::{Digest, Sha256};
        let id = Uuid::new_v4().to_string();
        let digest = format!("{:x}", Sha256::digest(png));
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO account_payment_assets
                (id, user_id, source_url, media_type, content, sha256, created_at)
             VALUES (?1, ?2, ?3, 'image/png', ?4, ?5, ?6)
             ON CONFLICT(user_id, source_url, sha256) DO NOTHING",
            params![
                id,
                user_id,
                source_url,
                png,
                digest,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|error| AppError::Internal(format!("store payment asset failed: {error}")))?;
        conn.query_row(
            "SELECT id FROM account_payment_assets
             WHERE user_id = ?1 AND source_url = ?2 AND sha256 = ?3",
            params![user_id, source_url, digest],
            |row| row.get(0),
        )
        .map_err(|error| AppError::Internal(format!("read stored payment asset failed: {error}")))
    }

    pub async fn client_market_payment_asset_for_viewer(
        &self,
        id: &str,
        viewer: Option<&AuthSession>,
    ) -> Result<Vec<u8>, AppError> {
        let conn = self.conn.lock().await;
        let asset: Option<(String, Vec<u8>)> = conn
            .query_row(
                "SELECT user_id, content FROM account_payment_assets WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read payment asset failed: {error}")))?;
        let (owner_user_id, content) =
            asset.ok_or_else(|| AppError::NotFound("payment asset not found".into()))?;
        let publicly_published = conn
            .query_row(
                "SELECT 1 FROM chat_public_payment_assets WHERE asset_id = ?1 LIMIT 1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("authorize public payment asset failed: {error}"))
            })?
            .is_some();
        let invoiced_for_viewer = if let Some(viewer) = viewer {
            conn.query_row(
                "SELECT 1
                     FROM market_invoices invoice
                     JOIN market_credit_accounts account ON account.id = invoice.account_id
                     WHERE account.supplier_user_id = ?1
                       AND account.buyer_user_id = ?2
                       AND EXISTS (
                           SELECT 1 FROM json_each(invoice.payment_methods_json) method
                           WHERE json_extract(method.value, '$.assetUrl') =
                                 '/v1/account/payment-assets/' || ?3
                       )
                     LIMIT 1",
                params![owner_user_id, viewer.user_id, id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("authorize invoiced payment asset failed: {error}"))
            })?
            .is_some()
        } else {
            false
        };
        let allowed = publicly_published
            || viewer.is_some_and(|viewer| owner_user_id == viewer.user_id)
            || invoiced_for_viewer;
        if !allowed {
            return Err(AppError::Forbidden(
                "not allowed to view this payment asset".into(),
            ));
        }
        Ok(content)
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
        let mut statement = conn
            .prepare(
                "SELECT p.provider_id, p.owner_email, p.created_at,
                        COUNT(h.id) AS host_total,
                        COALESCE(SUM(CASE WHEN h.status = 'idle' THEN 1 ELSE 0 END), 0) AS idle_total,
                        COALESCE(SUM(CASE WHEN h.status = 'allocated' THEN 1 ELSE 0 END), 0) AS allocated_total,
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.daily_rate_minor IS NULL THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.daily_rate_minor IS NULL AND h.status = 'allocated' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.daily_rate_minor IS NOT NULL THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.daily_rate_minor IS NOT NULL AND h.status = 'allocated' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.last_error IS NOT NULL AND TRIM(h.last_error) != '' THEN 1 ELSE 0 END), 0),
                        MIN(h.daily_rate_minor), MAX(h.daily_rate_minor),
                        (SELECT COUNT(*) FROM provisioning_jobs j
                         WHERE j.host_id IN (SELECT id FROM router_ssh_hosts WHERE provider_id = p.provider_id)
                           AND j.type = 'create' AND j.status = 'succeeded') AS successful_allocations,
                        COALESCE((SELECT methods_json FROM account_payment_profiles payment
                                  WHERE payment.user_id = p.provider_id), '[]'),
                        COALESCE(
                          (SELECT MAX(e.created_at) FROM client_market_audit_events e
                           WHERE e.event_type = 'host_offer_updated'
                             AND e.host_id IN (SELECT id FROM router_ssh_hosts WHERE provider_id = p.provider_id)),
                          MIN(h.created_at), p.created_at)
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
                    row.get::<_, i64>(13)?,
                    row.get::<_, String>(14)?,
                    row.get::<_, String>(15)?,
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
                successful,
                methods_json,
                offer_stable_since,
            ) = row.map_err(|error| {
                AppError::Internal(format!("read Provider supply failed: {error}"))
            })?;
            let methods: Vec<PaymentMethod> =
                serde_json::from_str(&methods_json).unwrap_or_default();
            let mut payment_method_kinds = methods
                .into_iter()
                .map(|method| method.kind)
                .collect::<Vec<_>>();
            payment_method_kinds.sort();
            payment_method_kinds.dedup();
            let mut country_statement = conn
                .prepare(
                    "SELECT country_code,
                            SUM(CASE WHEN status = 'idle' THEN 1 ELSE 0 END),
                            COUNT(*),
                            SUM(CASE WHEN status = 'idle' AND daily_rate_minor IS NULL THEN 1 ELSE 0 END),
                            SUM(CASE WHEN daily_rate_minor IS NULL THEN 1 ELSE 0 END)
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
            providers.push(ProviderSummary {
                official: official_email.as_deref() == Some(owner_email.as_str()),
                provider_id,
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
                successful_allocations: successful,
                payment_method_kinds,
                countries,
            });
        }
        providers.sort_by(|left, right| {
            right
                .official
                .cmp(&left.official)
                .then_with(|| {
                    right
                        .external_clients_over_30_days
                        .cmp(&left.external_clients_over_30_days)
                })
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
        });
        let official_provider_id = providers
            .iter()
            .find(|provider| provider.official)
            .map(|provider| provider.provider_id.clone());
        Ok(ProviderSupplyResponse {
            router_owner_email: official_email,
            official_provider_id,
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
                    COUNT(DISTINCT CASE WHEN s.status != 'released' AND i.last_seen_at >= ?2
                                        THEN s.installation_id END),
                    COUNT(DISTINCT CASE WHEN s.status != 'released' THEN s.installation_id END),
                    COUNT(DISTINCT CASE WHEN h.last_error IS NOT NULL AND TRIM(h.last_error) != ''
                                        THEN h.id END),
                    COUNT(DISTINCT h.id), ?3
             FROM host_provider_profiles p
             LEFT JOIN router_ssh_hosts h ON h.provider_id = p.provider_id
             LEFT JOIN client_market_subscriptions s ON s.provider_id = p.provider_id
             LEFT JOIN installations i ON i.id = s.installation_id
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
            Option<String>,
            Option<i64>,
            i64,
            String,
        )> = tx
            .query_row(
                "SELECT provider_id, host_owner_email, daily_rate_minor, currency,
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
                    ))
                },
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read host offer failed: {error}")))?;
        let (
            provider_id,
            host_owner_email,
            old_price,
            old_currency,
            old_free_duration_days,
            old_revision,
            host_status,
        ) = host.ok_or_else(|| AppError::NotFound("host not found".into()))?;
        if !crate::client_market::session_is_host_owner(
            session,
            provider_id.as_deref(),
            &host_owner_email,
        ) {
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
        if old_price == daily_rate_minor
            && old_currency_norm == currency
            && old_free_duration_days == free_duration_days.map(i64::from)
        {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit unchanged Host offer failed: {error}"))
            })?;
            return Ok(HostOfferView {
                host_id: host_id.to_string(),
                daily_rate_minor,
                currency,
                free_duration_days,
                offer_revision: old_revision,
            });
        }
        if host_status == "locked" {
            return Err(AppError::Conflict(
                "the Host offer is locked while provisioning is in progress; retry after it completes"
                    .into(),
            ));
        }
        // Paid offers require the Host owner's Account payment details so renters
        // have a way to pay. Free offers do not require a payment profile.
        if daily_rate_minor.is_some() {
            require_payment_profile_for_offer(&tx, &session.user_id)?;
            crate::market_billing::require_supplier_profile_tx(
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
             SET daily_rate_minor = ?2, currency = ?3,
                 free_duration_days = ?4, offer_revision = ?5, updated_at = ?6
             WHERE id = ?1",
            params![
                host_id,
                daily_rate_minor,
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
                "oldCurrency": old_currency,
                "oldFreeDurationDays": old_free_duration_days,
                "dailyRateMinor": daily_rate_minor,
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
            currency,
            free_duration_days,
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
                            h.ip, h.daily_rate_minor,
                            CASE WHEN h.daily_rate_minor IS NULL THEN NULL
                                 ELSE COALESCE(NULLIF(TRIM(h.currency), ''), 'USD') END,
                            h.free_duration_days, h.offer_revision
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
                        h.ip, h.daily_rate_minor,
                        CASE WHEN h.daily_rate_minor IS NULL THEN NULL
                             ELSE COALESCE(NULLIF(TRIM(h.currency), ''), 'USD') END,
                        h.free_duration_days, h.offer_revision
                 FROM router_ssh_hosts h
                 WHERE h.status = 'idle'
                   AND h.daily_rate_minor IS NULL
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
                    match crate::market_access::product_access_allowed_tx(
                        &tx,
                        &candidate.provider_id,
                        &session.user_id,
                        &session.email,
                        crate::market_access::PRODUCT_CLIENT_HOST,
                        crate::market_access::PRICING_FREE,
                    ) {
                        Ok(true) => Some(Ok(candidate)),
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
        for candidate in &mut candidates {
            let pricing_kind =
                crate::market_access::pricing_kind_for_rate(candidate.daily_rate_minor);
            if candidate.provider_id == session.user_id {
                candidate.daily_rate_minor = None;
                candidate.currency = None;
                candidate.free_duration_days = None;
            }
            crate::market_access::ensure_product_access_tx(
                &tx,
                &candidate.provider_id,
                &session.user_id,
                &session.email,
                crate::market_access::PRODUCT_CLIENT_HOST,
                pricing_kind,
            )?;
            if candidate.daily_rate_minor.is_some() {
                crate::market_billing::ensure_credit_allowed_tx(
                    &tx,
                    &session.user_id,
                    &session.email,
                    &candidate.provider_id,
                    crate::market_access::PRODUCT_CLIENT_HOST,
                    candidate.currency.as_deref().ok_or_else(|| {
                        AppError::Internal("paid quoted Host currency is missing".into())
                    })?,
                )?;
            }
        }
        let quote_id = Uuid::new_v4().to_string();
        let expires_at = now + Duration::seconds(QUOTE_TTL_SECS);
        let client_owner_email = normalize_email(&session.email)?;
        tx.execute(
            "INSERT INTO client_market_allocation_quotes
                (id, client_user_id, client_owner_email, status, fixed_host_id,
                 expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?6, ?6)",
            params![
                quote_id,
                session.user_id,
                client_owner_email,
                input.host_id,
                expires_at.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )
        .map_err(|error| AppError::Internal(format!("insert allocation quote failed: {error}")))?;
        let mut items = Vec::with_capacity(candidates.len());
        for (position, candidate) in candidates.into_iter().enumerate() {
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
                     free_duration_days, offer_revision)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                currency: candidate.currency,
                free_duration_days: candidate.free_duration_days,
                offer_revision: candidate.offer_revision,
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
        })
    }

    pub async fn client_market_commit_quote(
        &self,
        quote_id: &str,
        session: &AuthSession,
        prepared: &[(String, String, String, i64)],
    ) -> Result<CommitQuoteResponse, AppError> {
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Internal(format!("begin quote commit failed: {error}")))?;
        expire_quotes_tx(&tx, now)?;
        let quote: Option<(String, String, String)> = tx
            .query_row(
                "SELECT client_user_id, status, expires_at
                 FROM client_market_allocation_quotes WHERE id = ?1",
                params![quote_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read quote failed: {error}")))?;
        let (owner_user_id, status, expires_at) =
            quote.ok_or_else(|| AppError::NotFound("allocation quote not found".into()))?;
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "allocation quote belongs to another account".into(),
            ));
        }
        if status != "active" || parse_time(&expires_at)? <= now {
            return Err(AppError::Gone(
                "allocation quote expired; request a new quote".into(),
            ));
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
        for (item_id, subdomain, _, _) in prepared {
            if !unique_item_ids.insert(item_id.to_string())
                || !unique_subdomains.insert(subdomain.to_ascii_lowercase())
            {
                return Err(AppError::BadRequest(
                    "quote items and subdomains must be unique".into(),
                ));
            }
            let unavailable: i64 = tx
                .query_row(
                    "SELECT
                       EXISTS(SELECT 1 FROM public_hosts WHERE label = ?1 COLLATE NOCASE)
                       OR EXISTS(SELECT 1 FROM subdomain_reservations WHERE subdomain = ?1 COLLATE NOCASE)",
                    params![subdomain],
                    |row| row.get(0),
                )
                .map_err(|error| AppError::Internal(format!("check quoted subdomain failed: {error}")))?;
            if unavailable != 0 {
                return Err(AppError::Conflict(format!(
                    "subdomain {subdomain} is already in use"
                )));
            }
        }
        let batch_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO client_market_batches
                (id, quote_id, client_user_id, client_owner_email, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?5)",
            params![
                batch_id,
                quote_id,
                session.user_id,
                normalize_email(&session.email)?,
                now.to_rfc3339()
            ],
        )
        .map_err(|error| AppError::Internal(format!("insert Client batch failed: {error}")))?;
        let mut job_ids = Vec::with_capacity(prepared.len());
        for (item_id, subdomain, _, confirmed_revision) in prepared {
            let item = tx
                .query_row(
                    "SELECT host_id, provider_id, host_owner_email, country_code,
                            daily_rate_minor, currency, offer_revision
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
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| AppError::Internal(format!("read quote item failed: {error}")))?
                .ok_or_else(|| {
                    AppError::BadRequest("quote item does not belong to this quote".into())
                })?;
            if *confirmed_revision != item.6 {
                return Err(AppError::Conflict(
                    "the confirmed Host offer revision does not match the allocation quote; review the current offer"
                        .into(),
                ));
            }
            crate::market_access::ensure_product_access_tx(
                &tx,
                &item.1,
                &session.user_id,
                &session.email,
                crate::market_access::PRODUCT_CLIENT_HOST,
                crate::market_access::pricing_kind_for_rate(item.4),
            )?;
            if item.4.is_some() {
                crate::market_billing::ensure_credit_allowed_tx(
                    &tx,
                    &session.user_id,
                    &session.email,
                    &item.1,
                    crate::market_access::PRODUCT_CLIENT_HOST,
                    item.5.as_deref().ok_or_else(|| {
                        AppError::Internal("paid quote item currency is missing".into())
                    })?,
                )?;
            }
            let changed = tx
                .execute(
                    "UPDATE router_ssh_hosts SET status = 'locked', updated_at = ?2
                     WHERE id = ?1 AND status = ?3 AND offer_revision = ?4",
                    params![item.0, now.to_rfc3339(), HOST_STATUS_RESERVED, item.6],
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
            job_ids.push(job_id);
        }
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
        Ok(CommitQuoteResponse { batch_id, job_ids })
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
        let batch_id: Option<String> = conn
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
        let counts: (i64, i64, i64) = conn
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
        conn.execute(
            "UPDATE client_market_batches SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![batch_id, status, Utc::now().to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("update Client batch failed: {error}")))?;
        Ok(())
    }

    pub async fn client_market_list_rentals_for_viewer(
        &self,
        session: &AuthSession,
    ) -> Result<Vec<RentalView>, AppError> {
        let conn = self.conn.lock().await;
        load_rental_views(&conn, None, session)
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
    currency: Option<String>,
    free_duration_days: Option<u32>,
    offer_revision: i64,
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
        currency: row.get(7)?,
        free_duration_days: row
            .get::<_, Option<i64>>(8)?
            .and_then(|value| u32::try_from(value).ok()),
        offer_revision: row.get(9)?,
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
    currency: Option<String>,
    free_duration_days: Option<u32>,
    offer_revision: i64,
    activated_at: Option<String>,
    expires_at: Option<String>,
    methods_json: String,
    contacts_json: String,
    active_cleanup_job_id: Option<String>,
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
                COALESCE((SELECT methods_json FROM account_payment_profiles p
                          WHERE p.user_id = s.provider_id), '[]'),
                COALESCE((SELECT contacts_json FROM account_payment_profiles p
                          WHERE p.user_id = s.provider_id), '[]'),
                (SELECT j.id FROM provisioning_jobs j
                 WHERE j.installation_id = s.installation_id
                   AND j.type = 'cleanup'
                   AND j.status IN ('pending', 'running')
                 ORDER BY j.updated_at DESC, j.created_at DESC
                 LIMIT 1),
                s.updated_at
         FROM client_market_subscriptions s"
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
            methods_json: row.get(13)?,
            contacts_json: row.get(14)?,
            active_cleanup_job_id: row.get(15)?,
            updated_at: row.get(16)?,
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
        let methods: Vec<PaymentMethod> =
            serde_json::from_str(&row.methods_json).unwrap_or_default();
        let mut kinds = methods
            .iter()
            .map(|method| method.kind.clone())
            .collect::<Vec<_>>();
        kinds.sort();
        kinds.dedup();
        output.push(RentalView {
            installation_id: row.installation_id,
            host_id: row.host_id,
            provider_id: row.provider_id,
            host_owner_email: row.host_owner_email,
            client_owner_email: row.client_owner_email,
            status: row.status.clone(),
            daily_rate_minor: row.daily_rate_minor,
            currency: row.currency,
            free_duration_days: row.free_duration_days,
            offer_revision: row.offer_revision,
            activated_at: row.activated_at,
            expires_at: row.expires_at,
            payment_method_kinds: kinds,
            contacts: serde_json::from_str(&row.contacts_json).unwrap_or_default(),
            is_client_owner: client_role,
            can_release: (client_role || provider_role)
                && row.status != SUBSCRIPTION_RELEASED
                && row.status != SUBSCRIPTION_RELEASING,
            active_cleanup_job_id: row.active_cleanup_job_id,
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
            | "client_recovery_succeeded"
            | "client_recovery_failed"
            | "client_recovery_blocked"
            | "subscription_force_released"
    )
}

type ClientMarketChatContext = (
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
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
        "cleanup_finished" | "subscription_force_released" => SUBSCRIPTION_RELEASED,
        "cleanup_failed" => SUBSCRIPTION_RELEASE_FAILED,
        "client_recovery_succeeded" | "client_recovery_failed" | "client_recovery_blocked" => {
            SUBSCRIPTION_ACTIVE
        }
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
                    s.daily_rate_minor, s.currency, s.offer_revision, s.status,
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
                    h.daily_rate_minor,
                    CASE WHEN h.daily_rate_minor IS NULL THEN NULL
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
        currency,
        offer_revision,
        status,
        hostname,
    )) = context
    else {
        return Ok(());
    };
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
        "hostId": host_id,
        "hostname": hostname,
        "status": status,
        "dailyRateMinor": daily_rate_minor,
        "currency": currency,
        "offerRevision": offer_revision,
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
        "recoveryMethod",
        "attemptLevel",
        "nextAttemptAt",
    ] {
        if event_type == "client_recovery_blocked" && field == "failureCode" {
            continue;
        }
        if let Some(value) = detail.get(field) {
            output.insert(field.into(), value.clone());
        }
    }
    if let Some((methods_json, contacts_json)) = tx
        .query_row(
            "SELECT methods_json, COALESCE(contacts_json, '[]')
             FROM account_payment_profiles WHERE user_id = ?1",
            params![provider_user_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!(
                "read Client Market chat payment profile failed: {error}"
            ))
        })?
    {
        output.insert(
            "paymentMethods".into(),
            serde_json::from_str(&methods_json).map_err(|error| {
                AppError::Internal(format!(
                    "parse Client Market chat payment methods failed: {error}"
                ))
            })?,
        );
        output.insert(
            "paymentContacts".into(),
            serde_json::from_str(&contacts_json).map_err(|error| {
                AppError::Internal(format!(
                    "parse Client Market chat payment contacts failed: {error}"
                ))
            })?,
        );
    }
    let mut followers = vec![client_user_id, provider_user_id];
    if let Some(actor_user_id) = actor_user_id {
        followers.push(actor_user_id.to_string());
    }
    crate::store::client_chat::enqueue_client_system_event_tx(
        tx,
        installation_id,
        "client_market",
        source_event_id,
        event_type,
        payload,
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
        Option<String>,
        Option<i64>,
        i64,
    )> = tx
        .query_row(
            "SELECT h.provider_id, h.host_owner_email, j.client_owner_email,
                    COALESCE(j.client_owner_user_id,
                             (SELECT u.id FROM users u WHERE u.email_normalized = LOWER(j.client_owner_email)),
                             'email:' || LOWER(j.client_owner_email)),
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.daily_rate_minor
                         ELSE h.daily_rate_minor END,
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.currency
                         WHEN h.daily_rate_minor IS NULL THEN NULL
                         ELSE COALESCE(NULLIF(TRIM(h.currency), ''), 'USD') END,
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.free_duration_days
                         ELSE h.free_duration_days END,
                    CASE WHEN j.quote_id IS NOT NULL THEN qi.offer_revision
                         ELSE h.offer_revision END
             FROM provisioning_jobs j
             JOIN router_ssh_hosts h ON h.id = j.host_id
             LEFT JOIN client_market_allocation_quote_items qi
               ON qi.quote_id = j.quote_id AND qi.host_id = j.host_id
             WHERE j.id = ?1 AND h.id = ?2 AND h.provider_id IS NOT NULL
               AND (j.quote_id IS NULL OR qi.id IS NOT NULL)",
            params![job_id, host_id],
            |row| {
                Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                    row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?,
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
        price,
        currency,
        free_duration_days,
        revision,
    )) = context
    else {
        return Err(AppError::Internal(
            "provisioned Host has no stable Provider identity".into(),
        ));
    };
    let (price, currency, free_duration_days) = if provider_id == client_user_id {
        (None, None, None)
    } else {
        (price, currency, free_duration_days)
    };
    let expires_at = if price.is_none() {
        free_duration_days.map(|days| now + Duration::days(days))
    } else {
        None
    };
    let status = SUBSCRIPTION_ACTIVE;
    tx.execute(
        "INSERT INTO client_market_subscriptions
            (installation_id, host_id, provider_id, host_owner_email,
             client_user_id, client_owner_email, status, daily_rate_minor,
             currency, free_duration_days, offer_revision, activated_at, expires_at,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?12, ?12)",
        params![
            installation_id,
            host_id,
            provider_id,
            host_email,
            client_user_id,
            client_email,
            status,
            price,
            currency,
            free_duration_days,
            revision,
            now.to_rfc3339(),
            expires_at.map(|value| value.to_rfc3339()),
        ],
    )
    .map_err(|error| {
        AppError::Internal(format!("create Client Market subscription failed: {error}"))
    })?;
    let label = client_label_tx(tx, installation_id)?;
    if let Some(daily_rate_minor) = price {
        let currency = currency
            .as_deref()
            .ok_or_else(|| AppError::Internal("paid Client Host currency is missing".into()))?;
        crate::market_billing::activate_contract_tx(
            tx,
            crate::market_billing::ActivateContractInput {
                product_kind: "client_host",
                product_ref: installation_id,
                service_ref: installation_id,
                service_label: &label,
                buyer_user_id: &client_user_id,
                buyer_email: &client_email,
                supplier_user_id: &provider_id,
                supplier_email: &host_email,
                currency,
                daily_rate_minor,
                offer_revision: revision,
                replacement_of: None,
            },
            &now.to_rfc3339(),
        )?;
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
            "dailyRateMinor": price,
            "currency": currency,
            "freeDurationDays": free_duration_days,
            "offerRevision": revision,
            "activatedAt": now.to_rfc3339(),
            "expiresAt": expires_at.map(|value| value.to_rfc3339()),
            "trialHours": crate::market_billing::TRIAL_SECONDS / 3_600,
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
    let subscription: Option<(String, String, String, String, Option<i64>, Option<String>)> = tx
        .query_row(
            "SELECT s.provider_id, s.client_user_id, s.client_owner_email,
                    COALESCE(p.owner_email, s.host_owner_email), s.daily_rate_minor,
                    s.expires_at
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
    )) = subscription
    {
        crate::market_billing::terminate_contract_tx(
            tx,
            "client_host",
            installation_id,
            reason,
            &now.to_rfc3339(),
        )?;
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
                crate::market_access::pricing_kind_for_rate(daily_rate_minor),
                crate::market_access::DECISION_DENY,
                actor_user_id.unwrap_or(&provider_id),
                &now.to_rfc3339(),
            )?;
        }
    }
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
    tx.execute(
        "UPDATE client_market_subscriptions
         SET status = ?2, released_at = ?3, updated_at = ?3
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
    tx.execute(
        "UPDATE client_market_subscriptions
         SET status = ?2, updated_at = ?3 WHERE installation_id = ?1 AND status != 'released'",
        params![
            installation_id,
            SUBSCRIPTION_RELEASE_FAILED,
            now.to_rfc3339()
        ],
    )
    .map_err(|error| AppError::Internal(format!("mark subscription release failed: {error}")))?;
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
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    fn session(user_id: &str, email: &str) -> AuthSession {
        let now = Utc::now();
        AuthSession {
            session_id: format!("session-{user_id}"),
            user_id: user_id.into(),
            email: email.into(),
            installation_id: format!("browser-{user_id}"),
            access_token_hash: format!("access-{user_id}"),
            refresh_token_hash: format!("refresh-{user_id}"),
            access_expires_at: now + Duration::hours(1),
            refresh_expires_at: now + Duration::days(30),
            created_at: now,
            last_used_at: now,
        }
    }

    fn custom_payment_method() -> PaymentMethod {
        PaymentMethod {
            kind: "custom".into(),
            account: None,
            qr_image_url: None,
            asset_url: None,
            token: None,
            chain: None,
            address: None,
            instructions: Some("Wire transfer details".into()),
        }
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

    #[test]
    fn payment_asset_network_policy_rejects_reserved_addresses() {
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(is_public_ip(IpAddr::V6(
            "2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap()
        )));

        for address in [
            "0.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "192.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "ff02::1",
        ] {
            assert!(
                !is_public_ip(address.parse().unwrap()),
                "reserved address was accepted: {address}"
            );
        }
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

    #[test]
    fn payment_assets_accept_only_planned_raster_formats() {
        assert!(is_supported_qr_format(ImageFormat::Png));
        assert!(is_supported_qr_format(ImageFormat::Jpeg));
        assert!(is_supported_qr_format(ImageFormat::WebP));
        assert!(!is_supported_qr_format(ImageFormat::Gif));
    }

    #[test]
    fn crypto_payment_addresses_are_chain_specific() {
        assert!(
            validate_crypto_address("bsc", "0x1111111111111111111111111111111111111111").is_ok()
        );
        assert!(
            validate_crypto_address("eth", "1111111111111111111111111111111111111111").is_err()
        );
        assert!(validate_crypto_address("base", "0x1234").is_err());
        assert!(
            validate_crypto_address("bsc", "0x0000000000000000000000000000000000000000").is_err()
        );

        assert!(validate_crypto_address("tron", "TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdj").is_ok());
        let bad_checksum = "TXLAQ63Xg1NAzckPwKHvzw7CSEmLMEqcdk";
        assert!(validate_crypto_address("tron", bad_checksum).is_err());
        assert!(validate_crypto_address("tron", &format!("T{}", "0".repeat(33))).is_err());
    }

    #[test]
    fn custom_payment_instructions_preserve_safe_multiline_text() {
        let method = normalize_payment_method(PaymentMethod {
            kind: "custom".into(),
            account: None,
            qr_image_url: None,
            asset_url: None,
            token: None,
            chain: None,
            address: None,
            instructions: Some(" Bank transfer\r\nReference: client\tID ".into()),
        })
        .expect("normalize multiline custom payment instructions");
        assert_eq!(
            method.instructions.as_deref(),
            Some("Bank transfer\nReference: client\tID")
        );
        assert!(
            normalize_payment_method(PaymentMethod {
                kind: "custom".into(),
                account: None,
                qr_image_url: None,
                asset_url: None,
                token: None,
                chain: None,
                address: None,
                instructions: Some("unsafe\0text".into()),
            })
            .is_err()
        );
    }

    #[tokio::test]
    async fn payment_methods_cannot_be_cleared_with_market_dependencies() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let supplier = session(
            "supplier-profile-guard",
            "supplier-profile-guard@example.com",
        );
        let buyer = session("buyer-profile-guard", "buyer-profile-guard@example.com");
        store
            .client_market_update_payment_profile(&supplier, &[custom_payment_method()], None)
            .await
            .expect("configure guarded payment profile");
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(
                &conn,
                &supplier,
                "USD",
                50_000,
                &now.to_rfc3339(),
            );
            conn.execute(
                "INSERT INTO router_ssh_hosts (
                    id, ip, port, host_owner_email, status, created_at, updated_at,
                    provider_id, daily_rate_minor, currency, offer_revision
                 ) VALUES ('guarded-host', '203.0.113.10', 22, ?1, 'idle', ?2, ?2,
                           ?3, 500, 'USD', 1)",
                params![supplier.email, now.to_rfc3339(), supplier.user_id],
            )
            .expect("insert paid Host offer");
        }
        assert!(matches!(
            store
                .client_market_update_payment_profile(&supplier, &[], None)
                .await,
            Err(AppError::Conflict(_))
        ));
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE router_ssh_hosts SET daily_rate_minor = NULL, currency = NULL
                 WHERE id = 'guarded-host'",
                [],
            )
            .expect("withdraw paid Host offer");
        store
            .market_billing_update_supplier_profile(&supplier, "USD", 24)
            .await
            .expect("configure supplier credit profile");
        store
            .market_billing_activate_contract(
                crate::market_billing::ActivateContractInput {
                    product_kind: "client_host",
                    product_ref: "guarded-client",
                    service_ref: "guarded-client",
                    service_label: "Guarded Client",
                    buyer_user_id: &buyer.user_id,
                    buyer_email: &buyer.email,
                    supplier_user_id: &supplier.user_id,
                    supplier_email: &supplier.email,
                    currency: "USD",
                    daily_rate_minor: 500,
                    offer_revision: 1,
                    replacement_of: None,
                },
                now,
            )
            .await
            .expect("activate guarded contract");
        assert!(matches!(
            store
                .client_market_update_payment_profile(&supplier, &[], None)
                .await,
            Err(AppError::Conflict(_))
        ));
        store
            .market_billing_terminate_contract(
                "client_host",
                "guarded-client",
                "test_cleanup",
                now + Duration::seconds(1),
            )
            .await
            .expect("terminate guarded contract");
        let account_id: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT id FROM market_credit_accounts
                 WHERE buyer_user_id = ?1 AND supplier_user_id = ?2",
                params![buyer.user_id, supplier.user_id],
                |row| row.get(0),
            )
            .expect("read guarded credit account");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE market_credit_accounts SET balance_units = 86400 WHERE id = ?1",
                params![account_id],
            )
            .expect("seed guarded accrued balance");
        assert!(matches!(
            store
                .client_market_update_payment_profile(&supplier, &[], None)
                .await,
            Err(AppError::Conflict(_))
        ));
        {
            let conn = store.conn.lock().await;
            let timestamp = (now + Duration::seconds(2)).to_rfc3339();
            conn.execute(
                "INSERT INTO market_invoices (
                    id, account_id, sequence, amount_minor, amount_cny_minor,
                    usd_cny_rate_micros, amount_units, currency,
                    payment_methods_json, payment_contacts_json, payment_profile_updated_at,
                    status, due_at, deadline_at, opened_at
                 ) VALUES ('guarded-invoice', ?1, 1, 500, 3500, 7000000, 500000, 'USD',
                           '[]', '[]', ?2, 'open', ?2, ?2, ?2)",
                params![account_id, timestamp],
            )
            .expect("insert unsettled guarded invoice");
        }
        assert!(matches!(
            store
                .client_market_update_payment_profile(&supplier, &[], None)
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn invoice_snapshots_retain_and_authorize_their_payment_assets() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let supplier = session("supplier-asset-snapshot", "supplier@example.com");
        let buyer = session("buyer-asset-snapshot", "buyer@example.com");
        let now = Utc::now().to_rfc3339();
        let source_url = "https://example.com/qr.png";
        let snapshot_asset_id = store
            .client_market_store_payment_asset(&supplier.user_id, source_url, &[1, 2, 3])
            .await
            .expect("store snapshotted payment asset");
        let snapshot_method = PaymentMethod {
            kind: "custom".into(),
            account: None,
            qr_image_url: None,
            asset_url: Some(format!("/v1/account/payment-assets/{snapshot_asset_id}")),
            token: None,
            chain: None,
            address: None,
            instructions: Some("Snapshot instructions".into()),
        };
        store
            .client_market_update_payment_profile(
                &supplier,
                std::slice::from_ref(&snapshot_method),
                None,
            )
            .await
            .expect("configure snapshotted payment asset");
        store
            .market_billing_update_supplier_profile(&supplier, "USD", 24)
            .await
            .expect("configure snapshot supplier credit profile");
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(&conn, &supplier, "USD", 50_000, &now);
        }
        store
            .market_billing_activate_contract(
                crate::market_billing::ActivateContractInput {
                    product_kind: "client_host",
                    product_ref: "asset-snapshot-client",
                    service_ref: "asset-snapshot-client",
                    service_label: "Asset Snapshot Client",
                    buyer_user_id: &buyer.user_id,
                    buyer_email: &buyer.email,
                    supplier_user_id: &supplier.user_id,
                    supplier_email: &supplier.email,
                    currency: "USD",
                    daily_rate_minor: 500,
                    offer_revision: 1,
                    replacement_of: None,
                },
                Utc::now(),
            )
            .await
            .expect("activate snapshot billing contract");
        let snapshot_json =
            serde_json::to_string(&[snapshot_method]).expect("encode snapshotted payment method");
        {
            let conn = store.conn.lock().await;
            let account_id: String = conn
                .query_row(
                    "SELECT id FROM market_credit_accounts
                     WHERE buyer_user_id = ?1 AND supplier_user_id = ?2",
                    params![buyer.user_id, supplier.user_id],
                    |row| row.get(0),
                )
                .expect("read snapshot credit account");
            conn.execute(
                "INSERT INTO market_invoices (
                    id, account_id, sequence, amount_minor, amount_cny_minor,
                    usd_cny_rate_micros, amount_units, currency,
                    payment_methods_json, payment_contacts_json, payment_profile_updated_at,
                    status, due_at, deadline_at, opened_at
                 ) VALUES ('asset-snapshot-invoice', ?1, 1, 500, 3500, 7000000, 500000, 'USD',
                           ?2, '[]', ?3, 'open', ?3, ?3, ?3)",
                params![account_id, snapshot_json, now],
            )
            .expect("insert payment asset invoice snapshot");
        }

        let replacement_asset_id = store
            .client_market_store_payment_asset(&supplier.user_id, source_url, &[4, 5, 6])
            .await
            .expect("store replacement payment asset");
        assert_ne!(snapshot_asset_id, replacement_asset_id);
        let mut replacement_method = custom_payment_method();
        replacement_method.asset_url =
            Some(format!("/v1/account/payment-assets/{replacement_asset_id}"));
        store
            .client_market_update_payment_profile(&supplier, &[replacement_method], None)
            .await
            .expect("replace live payment profile");
        assert_eq!(
            store
                .client_market_payment_asset_for_viewer(&snapshot_asset_id, Some(&buyer))
                .await
                .expect("invoice buyer reads snapshotted payment asset"),
            vec![1, 2, 3]
        );
        assert_eq!(
            store
                .client_market_payment_asset_for_viewer(&replacement_asset_id, Some(&supplier))
                .await
                .expect("supplier reads replacement payment asset"),
            vec![4, 5, 6]
        );
    }
}
