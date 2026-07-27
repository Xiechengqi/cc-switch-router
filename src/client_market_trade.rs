use std::io::Cursor;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration as StdDuration;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use futures_util::StreamExt;
use image::{ImageFormat, ImageReader};
use reqwest::redirect::Policy;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::ServerState;
use crate::error::AppError;
use crate::models::AuthSession;
use crate::notifications::{
    FrozenEmailEnvelope, NotificationTemplateContext, retry_delay_secs, sanitize_delivery_error,
    send_resend_frozen_email,
};
use crate::store::AppStore;

pub const HOST_STATUS_RESERVED: &str = "reserved";
const QUOTE_TTL_SECS: i64 = 120;
const PAYMENT_WINDOW_HOURS: i64 = 72;
const MAX_CREATE_COUNT: usize = 2;
const MAX_PAYMENT_METHODS: usize = 20;
const MAX_CUSTOM_PAYMENT_CHARS: usize = 2_000;
const MAX_QR_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_QR_PIXELS: u64 = 4_000_000;
const MAX_QR_DIMENSION: u32 = 4_096;
const MAX_QR_STORED_BYTES: usize = 4 * 1024 * 1024;
const EMAIL_CYCLE_SECS: u64 = 5;
const BILLING_CYCLE_SECS: u64 = 20;
const MAX_EMAIL_ATTEMPTS: u32 = 12;

const SUBSCRIPTION_ACTIVE: &str = "active";
const SUBSCRIPTION_PAYMENT_DUE: &str = "payment_due";
const SUBSCRIPTION_RELEASING: &str = "releasing";
const SUBSCRIPTION_RELEASE_FAILED: &str = "release_failed";
const SUBSCRIPTION_RELEASED: &str = "released";

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
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderClientBlockView {
    pub client_user_id: String,
    pub client_owner_email: String,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePaymentProfileRequest {
    #[serde(default)]
    pub methods: Vec<PaymentMethod>,
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
    pub min_price_cents: Option<i64>,
    pub max_price_cents: Option<i64>,
    pub min_rental_period_days: Option<i64>,
    pub max_rental_period_days: Option<i64>,
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
    pub price_cents: Option<i64>,
    pub rental_period_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostOfferView {
    pub host_id: String,
    pub price_cents: Option<i64>,
    pub rental_period_days: Option<i64>,
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
    pub price_cents: Option<i64>,
    pub rental_period_days: Option<i64>,
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
pub struct BillingView {
    pub installation_id: String,
    pub host_id: String,
    pub provider_id: String,
    pub host_owner_email: String,
    pub client_owner_email: String,
    pub status: String,
    pub price_cents: Option<i64>,
    pub rental_period_days: Option<i64>,
    pub offer_revision: i64,
    pub current_period_end: Option<String>,
    pub payment_deadline: Option<String>,
    pub open_invoice_id: Option<String>,
    pub payment_methods: Option<Vec<PaymentMethod>>,
    pub payment_method_kinds: Vec<String>,
    pub payment_profile_updated_at: Option<String>,
    pub is_client_owner: bool,
    pub can_declare_paid: bool,
    pub can_release: bool,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclarePaidRequest {
    pub invoice_id: String,
    pub offer_revision: i64,
    pub payment_profile_updated_at: Option<String>,
    /// Amount the client actually rendered to the user. `offer_revision` already
    /// proves the client re-fetched after a price change, but not that a human saw
    /// the new number — a UI that silently refreshes and resubmits would pass that
    /// check alone. Echoing the displayed amount closes that gap. Optional for
    /// backward compatibility; when present it must match the invoice exactly.
    pub amount_cents_confirmed: Option<i64>,
    #[serde(default)]
    pub confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclareInvoicePaidRequest {
    pub offer_revision: i64,
    pub payment_profile_updated_at: Option<String>,
    /// See `DeclarePaidRequest::amount_cents_confirmed`.
    pub amount_cents_confirmed: Option<i64>,
    #[serde(default)]
    pub confirmed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeclarePaidResponse {
    billing: BillingView,
}

#[derive(Debug, Clone)]
pub struct MarketEmailClaim {
    id: String,
    recipient: String,
    subject: String,
    html: String,
    text: String,
    idempotency_key: String,
    attempts: u32,
}

#[derive(Debug, Clone)]
pub struct ExpiredBillingClient {
    pub installation_id: String,
    pub subdomain: Option<String>,
}

pub fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_provider_profiles (
            provider_id TEXT PRIMARY KEY,
            owner_email TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS host_provider_client_blocks (
            provider_id TEXT NOT NULL,
            client_user_id TEXT NOT NULL,
            client_owner_email TEXT NOT NULL,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL,
            lifted_at TEXT,
            PRIMARY KEY (provider_id, client_user_id)
        );
        CREATE TABLE IF NOT EXISTS host_provider_daily_stats (
            provider_id TEXT NOT NULL,
            stat_date TEXT NOT NULL,
            host_total INTEGER NOT NULL,
            idle_total INTEGER NOT NULL,
            allocated_total INTEGER NOT NULL,
            external_client_total INTEGER NOT NULL,
            online_samples INTEGER NOT NULL,
            observed_samples INTEGER NOT NULL,
            anomalous_host_samples INTEGER NOT NULL,
            host_samples INTEGER NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (provider_id, stat_date)
        );
        CREATE TABLE IF NOT EXISTS account_payment_profiles (
            user_id TEXT PRIMARY KEY,
            owner_email TEXT NOT NULL,
            methods_json TEXT NOT NULL DEFAULT '[]',
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS account_payment_methods (
            id TEXT PRIMARY KEY,
            profile_user_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            kind TEXT NOT NULL,
            method_json TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(profile_user_id, position)
        );
        CREATE TABLE IF NOT EXISTS account_payment_assets (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            source_url TEXT NOT NULL,
            media_type TEXT NOT NULL,
            content BLOB NOT NULL,
            sha256 TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(user_id, source_url)
        );
        CREATE TABLE IF NOT EXISTS client_market_allocation_quotes (
            id TEXT PRIMARY KEY,
            client_user_id TEXT NOT NULL,
            client_owner_email TEXT NOT NULL,
            status TEXT NOT NULL,
            fixed_host_id TEXT,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS client_market_allocation_quote_items (
            id TEXT PRIMARY KEY,
            quote_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            host_id TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            host_owner_email TEXT NOT NULL,
            country_code TEXT,
            hostname TEXT,
            price_cents INTEGER,
            rental_period_days INTEGER,
            offer_revision INTEGER NOT NULL,
            UNIQUE(quote_id, position),
            UNIQUE(quote_id, host_id)
        );
        CREATE TABLE IF NOT EXISTS client_market_batches (
            id TEXT PRIMARY KEY,
            quote_id TEXT NOT NULL UNIQUE,
            client_user_id TEXT NOT NULL,
            client_owner_email TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS client_market_subscriptions (
            installation_id TEXT PRIMARY KEY,
            host_id TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            host_owner_email TEXT NOT NULL,
            client_user_id TEXT NOT NULL,
            client_owner_email TEXT NOT NULL,
            status TEXT NOT NULL,
            price_cents INTEGER,
            rental_period_days INTEGER,
            offer_revision INTEGER NOT NULL,
            last_declared_at TEXT,
            current_period_end TEXT,
            payment_deadline TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            released_at TEXT
        );
        CREATE TABLE IF NOT EXISTS client_market_invoices (
            id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            amount_cents INTEGER NOT NULL,
            rental_period_days INTEGER NOT NULL,
            offer_revision INTEGER NOT NULL,
            status TEXT NOT NULL,
            due_at TEXT,
            deadline_at TEXT NOT NULL,
            opened_at TEXT NOT NULL,
            declared_at TEXT,
            canceled_at TEXT,
            UNIQUE(installation_id, sequence)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_client_market_open_invoice
            ON client_market_invoices(installation_id) WHERE status = 'open';
        CREATE UNIQUE INDEX IF NOT EXISTS idx_client_market_active_subscription_host
            ON client_market_subscriptions(host_id) WHERE status != 'released';
        CREATE TABLE IF NOT EXISTS client_market_payment_declarations (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL UNIQUE,
            installation_id TEXT NOT NULL,
            client_user_id TEXT NOT NULL,
            client_owner_email TEXT NOT NULL,
            declared_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS client_market_audit_events (
            id TEXT PRIMARY KEY,
            installation_id TEXT,
            host_id TEXT,
            actor_user_id TEXT,
            actor_email TEXT,
            event_type TEXT NOT NULL,
            detail_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS client_market_offer_events (
            id TEXT PRIMARY KEY,
            host_id TEXT NOT NULL,
            offer_revision INTEGER NOT NULL,
            actor_user_id TEXT,
            actor_email TEXT,
            detail_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(host_id, offer_revision)
        );
        CREATE TABLE IF NOT EXISTS client_market_subscription_events (
            id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL,
            host_id TEXT,
            actor_user_id TEXT,
            actor_email TEXT,
            event_type TEXT NOT NULL,
            detail_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS client_market_email_events (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            recipient TEXT NOT NULL,
            subject TEXT NOT NULL,
            html_body TEXT NOT NULL,
            text_body TEXT NOT NULL,
            idempotency_key TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS client_market_email_deliveries (
            id TEXT PRIMARY KEY,
            event_id TEXT,
            kind TEXT NOT NULL,
            recipient TEXT NOT NULL,
            subject TEXT NOT NULL,
            html_body TEXT NOT NULL,
            text_body TEXT NOT NULL,
            idempotency_key TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            next_attempt_at TEXT NOT NULL,
            claim_owner TEXT,
            claim_expires_at TEXT,
            provider_message_id TEXT,
            error_message TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            sent_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_market_email_claim
            ON client_market_email_deliveries(status, next_attempt_at, claim_expires_at);
        CREATE INDEX IF NOT EXISTS idx_market_payment_methods_profile
            ON account_payment_methods(profile_user_id, enabled, position);
        CREATE INDEX IF NOT EXISTS idx_market_subscription_events_installation
            ON client_market_subscription_events(installation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_market_quotes_owner
            ON client_market_allocation_quotes(client_user_id, status, expires_at);
        CREATE INDEX IF NOT EXISTS idx_market_subscriptions_billing
            ON client_market_subscriptions(status, payment_deadline, current_period_end);
        CREATE INDEX IF NOT EXISTS idx_market_subscriptions_client
            ON client_market_subscriptions(client_user_id, status);
        CREATE INDEX IF NOT EXISTS idx_market_subscriptions_provider
            ON client_market_subscriptions(provider_id, status);
        CREATE INDEX IF NOT EXISTS idx_market_blocks_client
            ON host_provider_client_blocks(client_user_id, lifted_at);",
    )?;
    add_column(conn, "router_ssh_hosts", "provider_id", "TEXT")?;
    add_column(conn, "router_ssh_hosts", "price_cents", "INTEGER")?;
    add_column(conn, "router_ssh_hosts", "rental_period_days", "INTEGER")?;
    add_column(
        conn,
        "router_ssh_hosts",
        "offer_revision",
        "INTEGER NOT NULL DEFAULT 1",
    )?;
    add_column(conn, "provisioning_jobs", "batch_id", "TEXT")?;
    add_column(conn, "provisioning_jobs", "quote_id", "TEXT")?;
    add_column(conn, "provisioning_jobs", "client_owner_user_id", "TEXT")?;
    add_column(conn, "provisioning_jobs", "cleanup_reason", "TEXT")?;
    add_column(conn, "client_market_email_deliveries", "event_id", "TEXT")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_router_ssh_hosts_provider_supply
            ON router_ssh_hosts(provider_id, status, country_code);",
    )?;
    Ok(())
}

fn add_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), rusqlite::Error> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|value| value == column) {
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/account/payment-profile",
            get(get_payment_profile).put(update_payment_profile),
        )
        .route("/v1/account/payment-assets/:id", get(get_payment_asset))
        .route("/v1/account/provider-blocks", get(list_provider_blocks))
        .route(
            "/v1/account/provider-blocks/:client_user_id",
            delete(lift_provider_block),
        )
        .route(
            "/v1/client-market/provider-blocks",
            get(list_provider_blocks),
        )
        .route(
            "/v1/client-market/provider-blocks/:client_user_id",
            delete(lift_provider_block),
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
        .route("/v1/client-market/my-billing", get(list_my_billing))
        .route("/v1/client-market/billing", get(list_my_billing))
        .route("/v1/client-market/clients", post(commit_client_batch))
        .route(
            "/v1/client-market/clients/:installation_id/billing",
            get(get_client_billing),
        )
        .route(
            "/v1/client-market/clients/:installation_id/declare-paid",
            post(declare_client_paid),
        )
        .route(
            "/v1/client-market/invoices/:invoice_id/declare-paid",
            post(declare_invoice_paid),
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
/// This deliberately touches only the subscription and its open invoice. Host
/// disposition is left to the existing reverify / auto-heal path, because forcing
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
        .client_market_force_release_subscription(&installation_id, &session.user_id, &session.email)
        .await?;
    Ok(Json(outcome))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForceReleaseResponse {
    pub installation_id: String,
    pub previous_status: String,
    pub status: String,
    pub canceled_invoices: usize,
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

pub(crate) fn validate_offer(
    price_cents: Option<i64>,
    rental_period_days: Option<i64>,
) -> Result<(Option<i64>, Option<i64>), AppError> {
    match (price_cents, rental_period_days) {
        (None, None) | (Some(0), None) | (Some(0), Some(0)) => Ok((None, None)),
        (Some(price), Some(days)) if (1..=100_000_000).contains(&price) && (4..=3_650).contains(&days) => {
            Ok((Some(price), Some(days)))
        }
        _ => Err(AppError::BadRequest(
            "paid hosts require priceCents between 1 and 100000000 and rentalPeriodDays between 4 and 3650; omit both for free forever".into(),
        )),
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
        .map_err(|error| AppError::Internal(format!("read payment profile for offer failed: {error}")))?;
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
    Err(AppError::BadRequest(PAYMENT_PROFILE_REQUIRED_FOR_OFFER.into()))
}

/// Reattach hosts that belong to this owner email onto the canonical provider id.
/// Free/forever hosts often keep a drifted `provider_id` because offer edits (which
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
    // free/forever idle Hosts then disappear from Create Client supply even
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
            AppError::Internal(format!("prepare orphan Host Provider emails failed: {error}"))
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
                AppError::Internal(format!("lookup orphan Provider id collision failed: {error}"))
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
            AppError::Internal(format!("ensure orphan Host Provider profile failed: {error}"))
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

async fn list_provider_blocks(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ProviderClientBlockView>>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_provider_blocks(&session.user_id)
            .await?,
    ))
}

async fn lift_provider_block(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(client_user_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .client_market_lift_provider_block(&session, &client_user_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
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
    Ok(Json(
        state
            .store
            .client_market_update_payment_profile(&session, &methods)
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
    let session = require_session(&state, &headers).await?;
    let bytes = state
        .store
        .client_market_payment_asset_for_viewer(&id, &session)
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
    let (price, period) = validate_offer(input.price_cents, input.rental_period_days)?;
    Ok(Json(
        state
            .store
            .client_market_update_host_offer(&id, &session, price, period)
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

async fn list_my_billing(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<BillingView>>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_list_billing_for_viewer(&session)
            .await?,
    ))
}

async fn get_client_billing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<Json<BillingView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .client_market_billing_for_viewer(&installation_id, &session)
            .await?,
    ))
}

async fn declare_client_paid(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
    Json(input): Json<DeclarePaidRequest>,
) -> Result<Json<DeclarePaidResponse>, AppError> {
    if !input.confirmed {
        return Err(AppError::BadRequest(
            "payment declaration requires explicit confirmation".into(),
        ));
    }
    let session = require_session(&state, &headers).await?;
    state
        .store
        .client_market_declare_paid(
            &installation_id,
            &input.invoice_id,
            input.offer_revision,
            input.payment_profile_updated_at.as_deref(),
            input.amount_cents_confirmed,
            &session,
        )
        .await?;
    let billing = state
        .store
        .client_market_billing_for_viewer(&installation_id, &session)
        .await?;
    Ok(Json(DeclarePaidResponse { billing }))
}

async fn declare_invoice_paid(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(invoice_id): AxumPath<String>,
    Json(input): Json<DeclareInvoicePaidRequest>,
) -> Result<Json<DeclarePaidResponse>, AppError> {
    if !input.confirmed {
        return Err(AppError::BadRequest(
            "payment declaration requires explicit confirmation".into(),
        ));
    }
    let session = require_session(&state, &headers).await?;
    let installation_id = state
        .store
        .client_market_installation_for_invoice(&invoice_id)
        .await?;
    state
        .store
        .client_market_declare_paid(
            &installation_id,
            &invoice_id,
            input.offer_revision,
            input.payment_profile_updated_at.as_deref(),
            input.amount_cents_confirmed,
            &session,
        )
        .await?;
    let billing = state
        .store
        .client_market_billing_for_viewer(&installation_id, &session)
        .await?;
    Ok(Json(DeclarePaidResponse { billing }))
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
        let mut conn = self.conn.lock().await;
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
        let mut conn = self.conn.lock().await;
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

        if previous_status != SUBSCRIPTION_RELEASE_FAILED && previous_status != SUBSCRIPTION_RELEASING
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
                 SET status = ?2, payment_deadline = NULL, released_at = ?3, updated_at = ?3
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

        // An open invoice would keep the renter blocked on the billing surface even
        // after the subscription is released.
        let canceled_invoices = tx
            .execute(
                "UPDATE client_market_invoices
                 SET status = 'canceled', canceled_at = ?2
                 WHERE installation_id = ?1 AND status = 'open'",
                params![installation_id, now.to_rfc3339()],
            )
            .map_err(|error| AppError::Internal(format!("cancel open invoice failed: {error}")))?;

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

        if let Some((client_email, provider_email, label)) =
            cleanup_parties_tx(&tx, installation_id)?
        {
            for (kind, recipient) in [
                ("client_force_released", client_email),
                ("provider_force_released", provider_email),
            ] {
                enqueue_email_tx(
                    &tx,
                    kind,
                    &recipient,
                    &format!("[Client Market] {label} released by an administrator"),
                    &format!(
                        "Client {label} was stuck in {previous_status} and has been released by an administrator. Creating new Clients is no longer blocked for this account. The Host may still need reverification by its owner before it returns to the pool."
                    ),
                    &format!("force-release:{installation_id}:{}", now.timestamp()),
                    now,
                )?;
            }
        }

        insert_audit_tx(
            &tx,
            Some(installation_id),
            host_id.as_deref(),
            Some(actor_user_id),
            Some(actor_email),
            "subscription_force_released",
            serde_json::json!({
                "previousStatus": previous_status,
                "canceledInvoices": canceled_invoices,
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
            canceled_invoices,
            host_id,
            host_status,
        })
    }

    pub async fn client_market_installation_for_invoice(
        &self,
        invoice_id: &str,
    ) -> Result<String, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT installation_id FROM client_market_invoices WHERE id = ?1",
            params![invoice_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("lookup invoice failed: {error}")))?
        .ok_or_else(|| AppError::NotFound("invoice not found".into()))
    }

    pub async fn client_market_assert_creation_allowed(
        &self,
        session: &AuthSession,
    ) -> Result<(), AppError> {
        let mut conn = self.conn.lock().await;
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
        let mut conn = self.conn.lock().await;
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
                        AppError::Internal(format!("remove legacy Provider profile failed: {error}"))
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
                        AppError::Internal(format!(
                            "re-key subscription Provider failed: {error}"
                        ))
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
        // Pull free/forever hosts (and any other drifted rows) whose owner email
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
        let row: Option<(String, String, String)> = conn
            .query_row(
                "SELECT owner_email, methods_json, updated_at
                 FROM account_payment_profiles WHERE user_id = ?1",
                params![user_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read payment profile failed: {error}")))?;
        if let Some((owner_email, methods_json, updated_at)) = row {
            return Ok(PaymentProfileView {
                provider_id: user_id.to_string(),
                owner_email,
                methods: serde_json::from_str(&methods_json).unwrap_or_default(),
                updated_at,
            });
        }
        Ok(PaymentProfileView {
            provider_id: user_id.to_string(),
            owner_email: email,
            methods: Vec::new(),
            updated_at: Utc::now().to_rfc3339(),
        })
    }

    pub async fn client_market_update_payment_profile(
        &self,
        session: &AuthSession,
        methods: &[PaymentMethod],
    ) -> Result<PaymentProfileView, AppError> {
        let email = normalize_email(&session.email)?;
        let methods_json = serde_json::to_string(methods).map_err(|error| {
            AppError::Internal(format!("encode payment methods failed: {error}"))
        })?;
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin payment profile update failed: {error}"))
        })?;
        tx.execute(
            "INSERT INTO account_payment_profiles (user_id, owner_email, methods_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET
                owner_email = excluded.owner_email,
                methods_json = excluded.methods_json,
                updated_at = excluded.updated_at",
            params![session.user_id, email, methods_json, now],
        )
        .map_err(|error| AppError::Internal(format!("save payment profile failed: {error}")))?;
        tx.execute(
            "DELETE FROM account_payment_assets
             WHERE user_id = ?1 AND INSTR(?2, id) = 0",
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
            updated_at: now,
        })
    }

    pub async fn client_market_provider_blocks(
        &self,
        provider_id: &str,
    ) -> Result<Vec<ProviderClientBlockView>, AppError> {
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT client_user_id, client_owner_email, reason, created_at
                 FROM host_provider_client_blocks
                 WHERE provider_id = ?1 AND lifted_at IS NULL
                 ORDER BY created_at DESC, client_owner_email",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare Provider block list failed: {error}"))
            })?;
        statement
            .query_map(params![provider_id], |row| {
                Ok(ProviderClientBlockView {
                    client_user_id: row.get(0)?,
                    client_owner_email: row.get(1)?,
                    reason: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|error| {
                AppError::Internal(format!("query Provider block list failed: {error}"))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AppError::Internal(format!("read Provider block list failed: {error}"))
            })
    }

    pub async fn client_market_lift_provider_block(
        &self,
        session: &AuthSession,
        client_user_id: &str,
    ) -> Result<(), AppError> {
        let client_user_id = client_user_id.trim();
        if client_user_id.is_empty() || client_user_id.len() > 200 {
            return Err(AppError::BadRequest(
                "invalid blocked Client identity".into(),
            ));
        }
        let now = Utc::now();
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin Provider unblock failed: {error}"))
        })?;
        let changed = tx
            .execute(
                "UPDATE host_provider_client_blocks SET lifted_at = ?3
                 WHERE provider_id = ?1 AND client_user_id = ?2 AND lifted_at IS NULL",
                params![session.user_id, client_user_id, now.to_rfc3339()],
            )
            .map_err(|error| AppError::Internal(format!("lift Provider block failed: {error}")))?;
        if changed == 0 {
            let exists: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM host_provider_client_blocks
                     WHERE provider_id = ?1 AND client_user_id = ?2",
                    params![session.user_id, client_user_id],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    AppError::Internal(format!("check Provider block failed: {error}"))
                })?;
            if exists == 0 {
                return Err(AppError::NotFound(
                    "blocked Client account not found".into(),
                ));
            }
        } else {
            insert_audit_tx(
                &tx,
                None,
                None,
                Some(&session.user_id),
                Some(&session.email),
                "provider_client_block_lifted",
                serde_json::json!({ "clientUserId": client_user_id }),
                now,
            )?;
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit Provider unblock failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn client_market_payment_asset_id(
        &self,
        user_id: &str,
        source_url: &str,
    ) -> Result<Option<String>, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT id FROM account_payment_assets WHERE user_id = ?1 AND source_url = ?2",
            params![user_id, source_url],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("lookup payment asset failed: {error}")))
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
             ON CONFLICT(user_id, source_url) DO UPDATE SET
                content = excluded.content, sha256 = excluded.sha256, created_at = excluded.created_at",
            params![id, user_id, source_url, png, digest, Utc::now().to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("store payment asset failed: {error}")))?;
        conn.query_row(
            "SELECT id FROM account_payment_assets WHERE user_id = ?1 AND source_url = ?2",
            params![user_id, source_url],
            |row| row.get(0),
        )
        .map_err(|error| AppError::Internal(format!("read stored payment asset failed: {error}")))
    }

    pub async fn client_market_payment_asset_for_viewer(
        &self,
        id: &str,
        viewer: &AuthSession,
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
        let allowed = owner_user_id == viewer.user_id
            || conn
                .query_row(
                    "SELECT 1 FROM client_market_subscriptions
                     WHERE provider_id = ?1 AND client_user_id = ?2
                       AND status != 'released' LIMIT 1",
                    params![owner_user_id, viewer.user_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!("authorize payment asset failed: {error}"))
                })?
                .is_some();
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
        let mut conn = self.conn.lock().await;
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
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.price_cents IS NULL THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.price_cents IS NULL AND h.status = 'allocated' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.price_cents IS NOT NULL THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.id IS NOT NULL AND h.price_cents IS NOT NULL AND h.status = 'allocated' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN h.last_error IS NOT NULL AND TRIM(h.last_error) != '' THEN 1 ELSE 0 END), 0),
                        MIN(h.price_cents), MAX(h.price_cents),
                        MIN(h.rental_period_days), MAX(h.rental_period_days),
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
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, String>(16)?,
                    row.get::<_, String>(17)?,
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
                min_price_cents,
                max_price_cents,
                min_period_days,
                max_period_days,
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
                            SUM(CASE WHEN status = 'idle' THEN 1 ELSE 0 END), COUNT(*)
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
                min_price_cents,
                max_price_cents,
                min_rental_period_days: min_period_days,
                max_rental_period_days: max_period_days,
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
        price_cents: Option<i64>,
        rental_period_days: Option<i64>,
    ) -> Result<HostOfferView, AppError> {
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Internal(format!("begin offer update failed: {error}")))?;
        let host: Option<(Option<String>, String, Option<i64>, Option<i64>, i64, String)> = tx
            .query_row(
                "SELECT provider_id, host_owner_email, price_cents, rental_period_days, offer_revision, status
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
                    ))
                },
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read host offer failed: {error}")))?;
        let (provider_id, host_owner_email, old_price, old_period, old_revision, host_status) =
            host.ok_or_else(|| AppError::NotFound("host not found".into()))?;
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
                    params![
                        host_id,
                        session.user_id,
                        email,
                        Utc::now().to_rfc3339()
                    ],
                )
                .map_err(|error| {
                    AppError::Internal(format!("heal Host provider identity failed: {error}"))
                })?;
            }
        }
        if old_price == price_cents && old_period == rental_period_days {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit unchanged Host offer failed: {error}"))
            })?;
            return Ok(HostOfferView {
                host_id: host_id.to_string(),
                price_cents,
                rental_period_days,
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
        // have a way to pay. Free forever does not require a payment profile.
        if price_cents.is_some() {
            require_payment_profile_for_offer(&tx, &session.user_id)?;
        }
        let revision = old_revision + 1;
        tx.execute(
            "UPDATE router_ssh_hosts
             SET price_cents = ?2, rental_period_days = ?3, offer_revision = ?4, updated_at = ?5
             WHERE id = ?1",
            params![
                host_id,
                price_cents,
                rental_period_days,
                revision,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|error| AppError::Internal(format!("update Host offer failed: {error}")))?;
        apply_offer_to_subscriptions_tx(
            &tx,
            host_id,
            price_cents,
            rental_period_days,
            revision,
            Utc::now(),
        )?;
        insert_audit_tx(
            &tx,
            None,
            Some(host_id),
            Some(&session.user_id),
            Some(&session.email),
            "host_offer_updated",
            serde_json::json!({
                "oldPriceCents": old_price,
                "oldRentalPeriodDays": old_period,
                "priceCents": price_cents,
                "rentalPeriodDays": rental_period_days,
                "offerRevision": revision,
            }),
            Utc::now(),
        )?;
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit Host offer failed: {error}")))?;
        Ok(HostOfferView {
            host_id: host_id.to_string(),
            price_cents,
            rental_period_days,
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
        let mut conn = self.conn.lock().await;
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
        // Self-dealing guard. A Provider renting their own Host would inflate
        // `successful_allocations` and distort public supply numbers. Ownership is
        // matched the same way `session_is_host_owner` does it — by provider_id and
        // by owner email — because provider_id can drift from the account that
        // originally registered the Host.
        let self_email = normalize_email(&session.email)?;
        let candidates = if let Some(host_id) = input.host_id.as_deref() {
            let candidate = tx
                .query_row(
                    "SELECT h.id, h.provider_id, h.host_owner_email, h.country_code, h.hostname,
                            h.price_cents, h.rental_period_days, h.offer_revision
                     FROM router_ssh_hosts h
                     WHERE h.id = ?1 AND h.status = 'idle' AND h.provider_id IS NOT NULL
                       AND h.provider_id IS NOT ?2
                       AND LOWER(h.host_owner_email) IS NOT ?3
                       AND NOT EXISTS (
                           SELECT 1 FROM host_provider_client_blocks b
                           WHERE b.provider_id = h.provider_id AND b.client_user_id = ?2
                             AND b.lifted_at IS NULL
                       )",
                    params![host_id, session.user_id, self_email],
                    map_quote_candidate,
                )
                .optional()
                .map_err(|error| AppError::Internal(format!("select fixed Host failed: {error}")))?
                .ok_or_else(|| {
                    AppError::Conflict(
                        "the selected Host is no longer idle, belongs to this account, or this Provider does not accept this account".into(),
                    )
                })?;
            vec![candidate]
        } else {
            let provider_vars = sql_vars(provider_ids.len());
            let country_vars = sql_vars(countries.len());
            let sql = format!(
                "SELECT h.id, h.provider_id, h.host_owner_email, h.country_code, h.hostname,
                        h.price_cents, h.rental_period_days, h.offer_revision
                 FROM router_ssh_hosts h
                 WHERE h.status = 'idle'
                   AND h.provider_id IN ({provider_vars})
                   AND h.country_code IN ({country_vars})
                   AND h.provider_id IS NOT ?
                   AND LOWER(h.host_owner_email) IS NOT ?
                   AND NOT EXISTS (
                       SELECT 1 FROM host_provider_client_blocks b
                       WHERE b.provider_id = h.provider_id AND b.client_user_id = ?
                         AND b.lifted_at IS NULL
                   )
                 ORDER BY RANDOM() LIMIT {}",
                input.count
            );
            let mut values = provider_ids.clone();
            values.extend(countries.clone());
            values.push(session.user_id.clone());
            values.push(self_email.clone());
            values.push(session.user_id.clone());
            let mut statement = tx.prepare(&sql).map_err(|error| {
                AppError::Internal(format!("prepare random Host quote failed: {error}"))
            })?;
            let rows = statement
                .query_map(params_from_iter(values.iter()), map_quote_candidate)
                .map_err(|error| {
                    AppError::Internal(format!("query random Host quote failed: {error}"))
                })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|error| {
                AppError::Internal(format!("read random Host quote failed: {error}"))
            })?
        };
        if candidates.len() != input.count {
            return Err(AppError::ServiceUnavailable(
                "not enough idle Hosts match the selected Providers and regions".into(),
            ));
        }
        let quote_id = Uuid::new_v4().to_string();
        let expires_at = now + Duration::seconds(QUOTE_TTL_SECS);
        tx.execute(
            "INSERT INTO client_market_allocation_quotes
                (id, client_user_id, client_owner_email, status, fixed_host_id,
                 expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?6, ?6)",
            params![
                quote_id,
                session.user_id,
                self_email,
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
                     country_code, hostname, price_cents, rental_period_days, offer_revision)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    item_id,
                    quote_id,
                    position as i64,
                    candidate.host_id,
                    candidate.provider_id,
                    candidate.host_owner_email,
                    candidate.country_code,
                    candidate.hostname,
                    candidate.price_cents,
                    candidate.rental_period_days,
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
                price_cents: candidate.price_cents,
                rental_period_days: candidate.rental_period_days,
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
        let mut conn = self.conn.lock().await;
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
        let blocked_provider: Option<String> = tx
            .query_row(
                "SELECT qi.provider_id
                 FROM client_market_allocation_quote_items qi
                 JOIN host_provider_client_blocks b
                   ON b.provider_id = qi.provider_id
                  AND b.client_user_id = ?2
                  AND b.lifted_at IS NULL
                 WHERE qi.quote_id = ?1
                 LIMIT 1",
                params![quote_id, session.user_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("recheck quoted Provider block failed: {error}"))
            })?;
        if blocked_provider.is_some() {
            return Err(AppError::Conflict(
                "a quoted Provider no longer accepts this account; cancel this quote and select another Host"
                    .into(),
            ));
        }
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
                    "SELECT host_id, provider_id, host_owner_email, country_code, offer_revision
                     FROM client_market_allocation_quote_items
                     WHERE id = ?1 AND quote_id = ?2",
                    params![item_id, quote_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| AppError::Internal(format!("read quote item failed: {error}")))?
                .ok_or_else(|| {
                    AppError::BadRequest("quote item does not belong to this quote".into())
                })?;
            if *confirmed_revision != item.4 {
                return Err(AppError::Conflict(
                    "the confirmed Host offer revision does not match the allocation quote; review the current offer"
                        .into(),
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE router_ssh_hosts SET status = 'locked', updated_at = ?2
                     WHERE id = ?1 AND status = ?3 AND offer_revision = ?4",
                    params![item.0, now.to_rfc3339(), HOST_STATUS_RESERVED, item.4],
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
        let mut conn = self.conn.lock().await;
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

    pub async fn client_market_list_billing_for_viewer(
        &self,
        session: &AuthSession,
    ) -> Result<Vec<BillingView>, AppError> {
        let conn = self.conn.lock().await;
        load_billing_views(&conn, None, session)
    }

    pub async fn client_market_billing_for_viewer(
        &self,
        installation_id: &str,
        session: &AuthSession,
    ) -> Result<BillingView, AppError> {
        let conn = self.conn.lock().await;
        load_billing_views(&conn, Some(installation_id), session)?
            .into_iter()
            .next()
            .ok_or_else(|| AppError::NotFound("Client Market billing record not found".into()))
    }

    pub async fn client_market_declare_paid(
        &self,
        installation_id: &str,
        invoice_id: &str,
        expected_offer_revision: i64,
        expected_payment_profile_updated_at: Option<&str>,
        expected_amount_cents: Option<i64>,
        session: &AuthSession,
    ) -> Result<(), AppError> {
        let now = Utc::now();
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin payment declaration failed: {error}"))
        })?;
        struct DeclarationSubscription {
            client_user_id: String,
            provider_id: String,
            provider_email: String,
            current_period_end: Option<String>,
            status: String,
            payment_profile_updated_at: Option<String>,
        }
        let subscription: Option<DeclarationSubscription> = tx
            .query_row(
                "SELECT s.client_user_id, s.provider_id,
                        COALESCE(p.owner_email, s.host_owner_email),
                        s.current_period_end, s.status, payment.updated_at
                 FROM client_market_subscriptions s
                 LEFT JOIN host_provider_profiles p ON p.provider_id = s.provider_id
                 LEFT JOIN account_payment_profiles payment ON payment.user_id = s.provider_id
                 WHERE s.installation_id = ?1",
                params![installation_id],
                |row| {
                    Ok(DeclarationSubscription {
                        client_user_id: row.get(0)?,
                        provider_id: row.get(1)?,
                        provider_email: row.get(2)?,
                        current_period_end: row.get(3)?,
                        status: row.get(4)?,
                        payment_profile_updated_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read subscription failed: {error}")))?;
        let DeclarationSubscription {
            client_user_id,
            provider_id,
            provider_email,
            current_period_end: old_end,
            status,
            payment_profile_updated_at,
        } = subscription
            .ok_or_else(|| AppError::NotFound("Client Market subscription not found".into()))?;
        if client_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only the Client owner may declare payment".into(),
            ));
        }
        let invoice: Option<(String, i64, i64, i64, String, Option<String>)> = tx
            .query_row(
                "SELECT i.status, i.amount_cents, i.rental_period_days, i.offer_revision,
                        i.deadline_at, d.client_user_id
                 FROM client_market_invoices i
                 LEFT JOIN client_market_payment_declarations d ON d.invoice_id = i.id
                 WHERE i.id = ?1 AND i.installation_id = ?2",
                params![invoice_id, installation_id],
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
            .map_err(|error| AppError::Internal(format!("read invoice failed: {error}")))?;
        let (invoice_status, price, period, invoice_revision, deadline_at, declared_by) =
            invoice.ok_or_else(|| AppError::NotFound("open invoice not found".into()))?;
        if invoice_status == "declared" && declared_by.as_deref() == Some(session.user_id.as_str())
        {
            tx.commit().map_err(|error| {
                AppError::Internal(format!(
                    "commit idempotent payment declaration failed: {error}"
                ))
            })?;
            return Ok(());
        }
        if invoice_status != "open" {
            return Err(AppError::Conflict("invoice was already resolved".into()));
        }
        if invoice_revision != expected_offer_revision {
            return Err(AppError::Conflict(
                "the Host offer changed; review the current price before confirming payment".into(),
            ));
        }
        if let Some(expected_amount) = expected_amount_cents
            && expected_amount != price
        {
            return Err(AppError::Conflict(
                "the invoice amount changed; review the current price before confirming payment"
                    .into(),
            ));
        }
        if payment_profile_updated_at.as_deref() != expected_payment_profile_updated_at {
            return Err(AppError::Conflict(
                "the Provider payment details changed; review the current payment details before confirming payment"
                    .into(),
            ));
        }
        if parse_time(&deadline_at)? <= now {
            return Err(AppError::Gone(
                "the payment deadline has passed and this Client is being released".into(),
            ));
        }
        if status != SUBSCRIPTION_PAYMENT_DUE {
            return Err(AppError::Conflict(
                "this Client does not currently have an open payment".into(),
            ));
        }
        let base = old_end
            .as_deref()
            .map(parse_time)
            .transpose()?
            .filter(|end| *end > now)
            .unwrap_or(now);
        let next_end = base + Duration::days(period);
        let current_client_email = normalize_email(&session.email)?;
        let changed = tx
            .execute(
                "UPDATE client_market_invoices
                 SET status = 'declared', declared_at = ?3
                 WHERE id = ?1 AND installation_id = ?2 AND status = 'open'",
                params![invoice_id, installation_id, now.to_rfc3339()],
            )
            .map_err(|error| AppError::Internal(format!("declare invoice paid failed: {error}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "payment declaration raced with another action".into(),
            ));
        }
        tx.execute(
            "INSERT INTO client_market_payment_declarations
                (id, invoice_id, installation_id, client_user_id, client_owner_email, declared_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                Uuid::new_v4().to_string(),
                invoice_id,
                installation_id,
                session.user_id,
                current_client_email,
                now.to_rfc3339(),
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("insert payment declaration failed: {error}"))
        })?;
        tx.execute(
            "UPDATE client_market_subscriptions
             SET status = ?2, price_cents = ?3, rental_period_days = ?4,
                 offer_revision = ?5, client_owner_email = ?6,
                 last_declared_at = ?7, current_period_end = ?8,
                 payment_deadline = NULL, updated_at = ?7
             WHERE installation_id = ?1",
            params![
                installation_id,
                SUBSCRIPTION_ACTIVE,
                price,
                period,
                invoice_revision,
                current_client_email,
                now.to_rfc3339(),
                next_end.to_rfc3339(),
            ],
        )
        .map_err(|error| AppError::Internal(format!("advance subscription failed: {error}")))?;
        let client_label = client_label_tx(&tx, installation_id)?;
        let body = format!(
            "The Client owner {} declared that payment of ${:.2} for {client_label} was completed. The Router has not verified receipt. Please check your own account; if payment is missing, you may release the Client from your Host.",
            session.email,
            price as f64 / 100.0,
        );
        enqueue_email_tx(
            &tx,
            "payment_declared",
            &provider_email,
            &format!("[Client Market] Payment declared for {client_label}"),
            &body,
            &format!("payment-declared:{invoice_id}:{provider_id}"),
            now,
        )?;
        insert_audit_tx(
            &tx,
            Some(installation_id),
            None,
            Some(&session.user_id),
            Some(&session.email),
            "payment_declared",
            serde_json::json!({
                "invoiceId": invoice_id,
                "amountCents": price,
                "nextPeriodEnd": next_end.to_rfc3339(),
                "routerVerified": false,
            }),
            now,
        )?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit payment declaration failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn client_market_reconcile_trade_state(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ExpiredBillingClient>, AppError> {
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin billing reconcile failed: {error}"))
        })?;
        expire_quotes_tx(&tx, now)?;
        let renewals = {
            let mut statement = tx
                .prepare(
                    "SELECT installation_id, price_cents, rental_period_days, offer_revision,
                            current_period_end, client_owner_email
                     FROM client_market_subscriptions s
                     WHERE status = 'active' AND price_cents IS NOT NULL
                       AND rental_period_days IS NOT NULL AND current_period_end IS NOT NULL
                       AND current_period_end <= ?1
                       AND NOT EXISTS (
                           SELECT 1 FROM client_market_invoices i
                           WHERE i.installation_id = s.installation_id AND i.status = 'open'
                       )",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare renewal reconcile failed: {error}"))
                })?;
            statement
                .query_map(
                    params![(now + Duration::hours(PAYMENT_WINDOW_HOURS)).to_rfc3339()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .map_err(|error| AppError::Internal(format!("query renewals failed: {error}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::Internal(format!("read renewals failed: {error}")))?
        };
        for (installation_id, price, period, revision, due_at, client_email) in renewals {
            let due = parse_time(&due_at)?;
            open_invoice_tx(
                &tx,
                &installation_id,
                price,
                period,
                revision,
                Some(due),
                due,
                now,
            )?;
            let label = client_label_tx(&tx, &installation_id)?;
            let body = format!(
                "The next payment for {label} is due by {}. Please pay the Host Provider and declare payment in the Router before the countdown reaches zero. The Client is released automatically after the deadline.",
                due.to_rfc3339(),
            );
            enqueue_email_tx(
                &tx,
                "renewal_due",
                &client_email,
                &format!("[Client Market] Payment due for {label}"),
                &body,
                &format!("renewal-due:{installation_id}:{}", due.timestamp()),
                now,
            )?;
        }
        let expired = {
            let mut statement = tx
                .prepare(
                    "SELECT s.installation_id, t.subdomain
                     FROM client_market_subscriptions s
                     LEFT JOIN installation_client_tunnels t ON t.installation_id = s.installation_id
                     WHERE s.status = 'payment_due' AND s.payment_deadline <= ?1",
                )
                .map_err(|error| AppError::Internal(format!("prepare overdue billing failed: {error}")))?;
            statement
                .query_map(params![now.to_rfc3339()], |row| {
                    Ok(ExpiredBillingClient {
                        installation_id: row.get(0)?,
                        subdomain: row.get(1)?,
                    })
                })
                .map_err(|error| {
                    AppError::Internal(format!("query overdue billing failed: {error}"))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!("read overdue billing failed: {error}"))
                })?
        };
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit billing reconcile failed: {error}"))
        })?;
        Ok(expired)
    }

    pub async fn client_market_claim_email(
        &self,
        worker_id: &str,
        now: DateTime<Utc>,
        delivery_enabled: bool,
    ) -> Result<Option<MarketEmailClaim>, AppError> {
        if !delivery_enabled {
            return Ok(None);
        }
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(|error| {
            AppError::Internal(format!("begin market email claim failed: {error}"))
        })?;
        tx.execute(
            "UPDATE client_market_email_deliveries
             SET status = 'retry', claim_owner = NULL, claim_expires_at = NULL, updated_at = ?1
             WHERE status = 'claimed' AND claim_expires_at <= ?1",
            params![now.to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("recover market emails failed: {error}")))?;
        let id: Option<String> = tx
            .query_row(
                "SELECT id FROM client_market_email_deliveries
                 WHERE status IN ('pending', 'retry') AND next_attempt_at <= ?1
                 ORDER BY created_at, id LIMIT 1",
                params![now.to_rfc3339()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("select market email failed: {error}")))?;
        let Some(id) = id else {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit empty email claim failed: {error}"))
            })?;
            return Ok(None);
        };
        let changed = tx
            .execute(
                "UPDATE client_market_email_deliveries
                 SET status = 'claimed', attempts = attempts + 1, claim_owner = ?2,
                     claim_expires_at = ?3, updated_at = ?4
                 WHERE id = ?1 AND status IN ('pending', 'retry')",
                params![
                    id,
                    worker_id,
                    (now + Duration::seconds(90)).to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|error| AppError::Internal(format!("claim market email failed: {error}")))?;
        if changed != 1 {
            return Err(AppError::Conflict("market email claim raced".into()));
        }
        let claim = tx
            .query_row(
                "SELECT id, recipient, subject, html_body, text_body, idempotency_key, attempts
                 FROM client_market_email_deliveries WHERE id = ?1 AND claim_owner = ?2",
                params![id, worker_id],
                |row| {
                    Ok(MarketEmailClaim {
                        id: row.get(0)?,
                        recipient: row.get(1)?,
                        subject: row.get(2)?,
                        html: row.get(3)?,
                        text: row.get(4)?,
                        idempotency_key: row.get(5)?,
                        attempts: row.get::<_, i64>(6)? as u32,
                    })
                },
            )
            .map_err(|error| {
                AppError::Internal(format!("read claimed market email failed: {error}"))
            })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit market email claim failed: {error}"))
        })?;
        Ok(Some(claim))
    }

    pub async fn client_market_finish_email(
        &self,
        id: &str,
        worker_id: &str,
        provider_message_id: Option<&str>,
        error_message: Option<&str>,
        retry_at: Option<DateTime<Utc>>,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let now = Utc::now().to_rfc3339();
        let (status, next_attempt, sent_at) = if provider_message_id.is_some() {
            ("sent", None, Some(now.clone()))
        } else if let Some(retry_at) = retry_at {
            ("retry", Some(retry_at.to_rfc3339()), None)
        } else {
            ("failed", None, None)
        };
        let changed = conn
            .execute(
                "UPDATE client_market_email_deliveries
                 SET status = ?3, provider_message_id = ?4, error_message = ?5,
                     next_attempt_at = COALESCE(?6, next_attempt_at), claim_owner = NULL,
                     claim_expires_at = NULL, sent_at = ?7, updated_at = ?8
                 WHERE id = ?1 AND claim_owner = ?2 AND status = 'claimed'",
                params![
                    id,
                    worker_id,
                    status,
                    provider_message_id,
                    error_message,
                    next_attempt,
                    sent_at,
                    now
                ],
            )
            .map_err(|error| AppError::Internal(format!("finish market email failed: {error}")))?;
        if changed != 1 {
            return Err(AppError::Conflict("market email claim was lost".into()));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct QuoteCandidate {
    host_id: String,
    provider_id: String,
    host_owner_email: String,
    country_code: Option<String>,
    hostname: Option<String>,
    price_cents: Option<i64>,
    rental_period_days: Option<i64>,
    offer_revision: i64,
}

fn map_quote_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuoteCandidate> {
    Ok(QuoteCandidate {
        host_id: row.get(0)?,
        provider_id: row.get(1)?,
        host_owner_email: row.get(2)?,
        country_code: row.get(3)?,
        hostname: row.get(4)?,
        price_cents: row.get(5)?,
        rental_period_days: row.get(6)?,
        offer_revision: row.get(7)?,
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
               AND status IN ('payment_due', 'releasing', 'release_failed')",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(|error| AppError::Internal(format!("check unpaid Client gate failed: {error}")))?;
    if blocked_subscription > 0 {
        return Err(AppError::Conflict(
            "resolve the current unpaid or releasing Client before creating another".into(),
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

fn apply_offer_to_subscriptions_tx(
    tx: &Transaction<'_>,
    host_id: &str,
    price: Option<i64>,
    period: Option<i64>,
    revision: i64,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let subscriptions = {
        let mut statement = tx
            .prepare(
                "SELECT installation_id, status, current_period_end, client_owner_email
                 FROM client_market_subscriptions
                 WHERE host_id = ?1 AND status != 'released'",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare Host subscriptions failed: {error}"))
            })?;
        statement
            .query_map(params![host_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|error| {
                AppError::Internal(format!("query Host subscriptions failed: {error}"))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AppError::Internal(format!("read Host subscriptions failed: {error}"))
            })?
    };
    for (installation_id, status, current_period_end, client_owner_email) in subscriptions {
        if price.is_none() || period.is_none() {
            tx.execute(
                "UPDATE client_market_invoices SET status = 'canceled', canceled_at = ?2
                 WHERE installation_id = ?1 AND status = 'open'",
                params![installation_id, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("cancel free Host invoice failed: {error}"))
            })?;
            tx.execute(
                "UPDATE client_market_subscriptions
                 SET status = 'active', price_cents = NULL, rental_period_days = NULL,
                     offer_revision = ?2, current_period_end = NULL, payment_deadline = NULL,
                     updated_at = ?3
                 WHERE installation_id = ?1",
                params![installation_id, revision, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("make subscription free failed: {error}"))
            })?;
            enqueue_offer_changed_email_tx(
                tx,
                &installation_id,
                &client_owner_email,
                price,
                period,
                revision,
                now,
            )?;
            continue;
        }
        let price = price.unwrap_or_default();
        let period = period.unwrap_or_default();
        if status == SUBSCRIPTION_PAYMENT_DUE {
            let changed = tx
                .execute(
                    "UPDATE client_market_invoices
                 SET amount_cents = ?2, rental_period_days = ?3, offer_revision = ?4
                 WHERE installation_id = ?1 AND status = 'open'",
                    params![installation_id, price, period, revision],
                )
                .map_err(|error| {
                    AppError::Internal(format!("update open invoice offer failed: {error}"))
                })?;
            if changed != 1 {
                return Err(AppError::Internal(
                    "payment-due subscription has no open invoice".into(),
                ));
            }
            tx.execute(
                "UPDATE client_market_subscriptions
                 SET price_cents = ?2, rental_period_days = ?3, offer_revision = ?4, updated_at = ?5
                 WHERE installation_id = ?1",
                params![installation_id, price, period, revision, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("update pending subscription offer failed: {error}"))
            })?;
            enqueue_offer_changed_email_tx(
                tx,
                &installation_id,
                &client_owner_email,
                Some(price),
                Some(period),
                revision,
                now,
            )?;
            continue;
        }
        if matches!(
            status.as_str(),
            SUBSCRIPTION_RELEASING | SUBSCRIPTION_RELEASE_FAILED
        ) {
            tx.execute(
                "UPDATE client_market_subscriptions
                 SET price_cents = ?2, rental_period_days = ?3, offer_revision = ?4, updated_at = ?5
                 WHERE installation_id = ?1",
                params![installation_id, price, period, revision, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "update releasing subscription offer failed: {error}"
                ))
            })?;
            continue;
        }
        let Some(current_period_end) = current_period_end else {
            let deadline = now + Duration::hours(PAYMENT_WINDOW_HOURS);
            open_invoice_tx(
                tx,
                &installation_id,
                price,
                period,
                revision,
                None,
                deadline,
                now,
            )?;
            enqueue_offer_changed_email_tx(
                tx,
                &installation_id,
                &client_owner_email,
                Some(price),
                Some(period),
                revision,
                now,
            )?;
            continue;
        };
        let frozen_end = parse_time(&current_period_end)?;
        let should_open = frozen_end <= now + Duration::hours(PAYMENT_WINDOW_HOURS);
        if should_open {
            let deadline = if frozen_end <= now {
                now + Duration::hours(PAYMENT_WINDOW_HOURS)
            } else {
                frozen_end
            };
            open_invoice_tx(
                tx,
                &installation_id,
                price,
                period,
                revision,
                Some(frozen_end),
                deadline,
                now,
            )?;
        } else {
            tx.execute(
                "UPDATE client_market_subscriptions
                 SET price_cents = ?2, rental_period_days = ?3, offer_revision = ?4,
                     updated_at = ?5
                 WHERE installation_id = ?1",
                params![installation_id, price, period, revision, now.to_rfc3339(),],
            )
            .map_err(|error| {
                AppError::Internal(format!("update next subscription offer failed: {error}"))
            })?;
        }
        enqueue_offer_changed_email_tx(
            tx,
            &installation_id,
            &client_owner_email,
            Some(price),
            Some(period),
            revision,
            now,
        )?;
    }
    Ok(())
}

fn enqueue_offer_changed_email_tx(
    tx: &Transaction<'_>,
    installation_id: &str,
    recipient: &str,
    price: Option<i64>,
    period: Option<i64>,
    revision: i64,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let label = client_label_tx(tx, installation_id)?;
    let body = match (price, period) {
        (Some(price), Some(period)) => format!(
            "The Host Provider changed the offer for {label} to ${:.2} every {period} days. The new offer applies immediately to an unpaid invoice and to the next billing period. Any current paid period keeps its existing end date. If this Client did not yet have a paid period, a three-day payment window has started. Open Router Clients to review the deadline or release the Client.",
            price as f64 / 100.0,
        ),
        _ => format!(
            "The Host Provider changed the offer for {label} to free forever. Any open invoice and payment countdown for this Client have been canceled."
        ),
    };
    enqueue_email_tx(
        tx,
        "host_offer_changed",
        recipient,
        &format!("[Client Market] Host offer changed for {label}"),
        &body,
        &format!("host-offer-changed:{installation_id}:{revision}"),
        now,
    )
}

fn open_invoice_tx(
    tx: &Transaction<'_>,
    installation_id: &str,
    price: i64,
    period: i64,
    revision: i64,
    due_at: Option<DateTime<Utc>>,
    deadline: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<String, AppError> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM client_market_invoices
             WHERE installation_id = ?1 AND status = 'open'",
            params![installation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("check open invoice failed: {error}")))?;
    if let Some(id) = existing {
        tx.execute(
            "UPDATE client_market_invoices
             SET amount_cents = ?2, rental_period_days = ?3, offer_revision = ?4,
                 due_at = ?5, deadline_at = ?6
             WHERE id = ?1 AND status = 'open'",
            params![
                id,
                price,
                period,
                revision,
                due_at.map(|value| value.to_rfc3339()),
                deadline.to_rfc3339()
            ],
        )
        .map_err(|error| AppError::Internal(format!("refresh open invoice failed: {error}")))?;
        tx.execute(
            "UPDATE client_market_subscriptions
             SET status = ?2, price_cents = ?3, rental_period_days = ?4,
                 offer_revision = ?5, current_period_end = COALESCE(?6, current_period_end),
                 payment_deadline = ?7, updated_at = ?8
             WHERE installation_id = ?1",
            params![
                installation_id,
                SUBSCRIPTION_PAYMENT_DUE,
                price,
                period,
                revision,
                due_at.map(|value| value.to_rfc3339()),
                deadline.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("refresh payment due subscription failed: {error}"))
        })?;
        return Ok(id);
    }
    let sequence: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM client_market_invoices WHERE installation_id = ?1",
            params![installation_id],
            |row| row.get(0),
        )
        .map_err(|error| AppError::Internal(format!("allocate invoice sequence failed: {error}")))?;
    let id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO client_market_invoices
            (id, installation_id, sequence, amount_cents, rental_period_days,
             offer_revision, status, due_at, deadline_at, opened_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8, ?9)",
        params![
            id,
            installation_id,
            sequence,
            price,
            period,
            revision,
            due_at.map(|value| value.to_rfc3339()),
            deadline.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|error| AppError::Internal(format!("insert invoice failed: {error}")))?;
    tx.execute(
        "UPDATE client_market_subscriptions
         SET status = ?2, price_cents = ?3, rental_period_days = ?4,
             offer_revision = ?5, current_period_end = COALESCE(?6, current_period_end),
             payment_deadline = ?7, updated_at = ?8
         WHERE installation_id = ?1",
        params![
            installation_id,
            SUBSCRIPTION_PAYMENT_DUE,
            price,
            period,
            revision,
            due_at.map(|value| value.to_rfc3339()),
            deadline.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|error| {
        AppError::Internal(format!("mark subscription payment due failed: {error}"))
    })?;
    Ok(id)
}

#[derive(Debug)]
struct BillingRow {
    installation_id: String,
    host_id: String,
    provider_id: String,
    host_owner_email: String,
    client_user_id: String,
    client_owner_email: String,
    status: String,
    price_cents: Option<i64>,
    rental_period_days: Option<i64>,
    offer_revision: i64,
    current_period_end: Option<String>,
    payment_deadline: Option<String>,
    open_invoice_id: Option<String>,
    methods_json: String,
    payment_profile_updated_at: Option<String>,
    updated_at: String,
}

fn load_billing_views(
    conn: &Connection,
    installation_id: Option<&str>,
    session: &AuthSession,
) -> Result<Vec<BillingView>, AppError> {
    let mut sql = "SELECT s.installation_id, s.host_id, s.provider_id, s.host_owner_email,
                s.client_user_id, s.client_owner_email, s.status, s.price_cents,
                s.rental_period_days, s.offer_revision, s.current_period_end,
                s.payment_deadline,
                (SELECT id FROM client_market_invoices i
                 WHERE i.installation_id = s.installation_id AND i.status = 'open' LIMIT 1),
                COALESCE((SELECT methods_json FROM account_payment_profiles p
                          WHERE p.user_id = s.provider_id), '[]'),
                (SELECT updated_at FROM account_payment_profiles p
                 WHERE p.user_id = s.provider_id),
                s.updated_at
         FROM client_market_subscriptions s"
        .to_string();
    if installation_id.is_some() {
        sql.push_str(" WHERE s.installation_id = ?1");
    }
    sql.push_str(" ORDER BY s.updated_at DESC, s.installation_id");
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| AppError::Internal(format!("prepare billing list failed: {error}")))?;
    let mapper = |row: &rusqlite::Row<'_>| {
        Ok(BillingRow {
            installation_id: row.get(0)?,
            host_id: row.get(1)?,
            provider_id: row.get(2)?,
            host_owner_email: row.get(3)?,
            client_user_id: row.get(4)?,
            client_owner_email: row.get(5)?,
            status: row.get(6)?,
            price_cents: row.get(7)?,
            rental_period_days: row.get(8)?,
            offer_revision: row.get(9)?,
            current_period_end: row.get(10)?,
            payment_deadline: row.get(11)?,
            open_invoice_id: row.get(12)?,
            methods_json: row.get(13)?,
            payment_profile_updated_at: row.get(14)?,
            updated_at: row.get(15)?,
        })
    };
    let rows = if let Some(installation_id) = installation_id {
        statement
            .query_map(params![installation_id], mapper)
            .map_err(|error| AppError::Internal(format!("query billing record failed: {error}")))?
            .collect::<Result<Vec<_>, _>>()
    } else {
        statement
            .query_map([], mapper)
            .map_err(|error| AppError::Internal(format!("query billing list failed: {error}")))?
            .collect::<Result<Vec<_>, _>>()
    }
    .map_err(|error| AppError::Internal(format!("read billing rows failed: {error}")))?;
    let mut output = Vec::new();
    for row in rows {
        let client_role = row.client_user_id == session.user_id;
        let provider_role = row.provider_id == session.user_id;
        if !client_role && !provider_role {
            continue;
        }
        let mut methods: Vec<PaymentMethod> =
            serde_json::from_str(&row.methods_json).unwrap_or_default();
        for method in &mut methods {
            // Renters use the authenticated, Router-cached asset only. The
            // Provider's source URL may contain private query parameters.
            method.qr_image_url = None;
        }
        let mut kinds = methods
            .iter()
            .map(|method| method.kind.clone())
            .collect::<Vec<_>>();
        kinds.sort();
        kinds.dedup();
        output.push(BillingView {
            installation_id: row.installation_id,
            host_id: row.host_id,
            provider_id: row.provider_id,
            host_owner_email: row.host_owner_email,
            client_owner_email: row.client_owner_email,
            status: row.status.clone(),
            price_cents: row.price_cents,
            rental_period_days: row.rental_period_days,
            offer_revision: row.offer_revision,
            current_period_end: row.current_period_end,
            payment_deadline: row.payment_deadline,
            open_invoice_id: row.open_invoice_id,
            payment_methods: Some(methods),
            payment_method_kinds: kinds,
            payment_profile_updated_at: row.payment_profile_updated_at,
            is_client_owner: client_role,
            can_declare_paid: client_role
                && row.status == SUBSCRIPTION_PAYMENT_DUE
                && row.price_cents.is_some(),
            can_release: (client_role || provider_role) && row.status != SUBSCRIPTION_RELEASED,
            updated_at: row.updated_at,
        });
    }
    Ok(output)
}

fn insert_audit_tx(
    tx: &Transaction<'_>,
    installation_id: Option<&str>,
    host_id: Option<&str>,
    actor_user_id: Option<&str>,
    actor_email: Option<&str>,
    event_type: &str,
    detail: serde_json::Value,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
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
    }
    Ok(())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn enqueue_email_tx(
    tx: &Transaction<'_>,
    kind: &str,
    recipient: &str,
    subject: &str,
    body: &str,
    idempotency_key: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let html = format!(
        "<!doctype html><html><body style=\"margin:0;background:#f8fafc;color:#172033;font-family:Arial,sans-serif\"><div style=\"max-width:620px;margin:0 auto;padding:28px 16px\"><div style=\"background:#fff;border:1px solid #dbe3ee;border-radius:8px;padding:28px\"><div style=\"font-size:13px;font-weight:700;color:#0f766e\">CC-Switch Router · Client Market</div><h1 style=\"font-size:22px;line-height:30px;margin:14px 0\">{}</h1><p style=\"font-size:15px;line-height:24px;color:#475569;white-space:pre-wrap\">{}</p><p style=\"font-size:12px;line-height:18px;color:#64748b;margin-top:24px\">Payment declarations are self-reported. The Router does not process or verify transfers between Client owners and Host Providers.</p></div></div></body></html>",
        escape_html(subject),
        escape_html(body),
    );
    let event_id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO client_market_email_events
            (id, kind, recipient, subject, html_body, text_body, idempotency_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(idempotency_key) DO NOTHING",
        params![
            event_id,
            kind,
            recipient,
            subject,
            html,
            body,
            idempotency_key,
            now.to_rfc3339(),
        ],
    )
    .map_err(|error| {
        AppError::Internal(format!("enqueue Client Market email event failed: {error}"))
    })?;
    let event_id: String = tx
        .query_row(
            "SELECT id FROM client_market_email_events WHERE idempotency_key = ?1",
            params![idempotency_key],
            |row| row.get(0),
        )
        .map_err(|error| {
            AppError::Internal(format!("read Client Market email event failed: {error}"))
        })?;
    tx.execute(
        "INSERT INTO client_market_email_deliveries
            (id, event_id, kind, recipient, subject, html_body, text_body, idempotency_key,
             status, attempts, next_attempt_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 0, ?9, ?9, ?9)
         ON CONFLICT(idempotency_key) DO NOTHING",
        params![
            Uuid::new_v4().to_string(),
            event_id,
            kind,
            recipient,
            subject,
            html,
            body,
            idempotency_key,
            now.to_rfc3339(),
        ],
    )
    .map_err(|error| AppError::Internal(format!("enqueue Client Market email failed: {error}")))?;
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
    dashboard_url: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let context: Option<(
        String,
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        i64,
        Option<String>,
    )> = tx
        .query_row(
            "SELECT h.provider_id, h.host_owner_email, j.client_owner_email,
                    COALESCE(j.client_owner_user_id,
                             (SELECT u.id FROM users u WHERE u.email_normalized = LOWER(j.client_owner_email)),
                             'email:' || LOWER(j.client_owner_email)),
                    h.price_cents, h.rental_period_days, h.offer_revision, h.hostname
             FROM provisioning_jobs j
             JOIN router_ssh_hosts h ON h.id = j.host_id
             WHERE j.id = ?1 AND h.id = ?2 AND h.provider_id IS NOT NULL",
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
        period,
        revision,
        hostname,
    )) = context
    else {
        return Err(AppError::Internal(
            "provisioned Host has no stable Provider identity".into(),
        ));
    };
    let paid = price.is_some() && period.is_some();
    let status = if paid {
        SUBSCRIPTION_PAYMENT_DUE
    } else {
        SUBSCRIPTION_ACTIVE
    };
    let deadline = paid.then(|| now + Duration::hours(PAYMENT_WINDOW_HOURS));
    tx.execute(
        "INSERT INTO client_market_subscriptions
            (installation_id, host_id, provider_id, host_owner_email,
             client_user_id, client_owner_email, status, price_cents,
             rental_period_days, offer_revision, payment_deadline, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
        params![
            installation_id,
            host_id,
            provider_id,
            host_email,
            client_user_id,
            client_email,
            status,
            price,
            period,
            revision,
            deadline.map(|value| value.to_rfc3339()),
            now.to_rfc3339(),
        ],
    )
    .map_err(|error| {
        AppError::Internal(format!("create Client Market subscription failed: {error}"))
    })?;
    if let (Some(price), Some(period), Some(deadline)) = (price, period, deadline) {
        open_invoice_tx(
            tx,
            installation_id,
            price,
            period,
            revision,
            None,
            deadline,
            now,
        )?;
    }
    let label = client_label_tx(tx, installation_id)?;
    let client_body = if let Some(deadline) = deadline {
        format!(
            "Your Client {label} is ready at {dashboard_url}/clients/. You may evaluate it free for three days. If it suits your needs, pay the Host Provider and declare payment in the Router before {}. If you do not want it, use Release now. After the deadline the Router disables and removes the Client automatically; data loss is your responsibility.",
            deadline.to_rfc3339(),
        )
    } else {
        format!(
            "Your Client {label} is ready at {dashboard_url}/clients/. This Host is currently offered free forever, so no payment declaration or billing countdown is required. The Host Provider may change the offer or release the Client later."
        )
    };
    enqueue_email_tx(
        tx,
        "client_allocated_owner",
        &client_email,
        &format!("[Client Market] {label} is ready"),
        &client_body,
        &format!("client-ready:{installation_id}:{client_email}"),
        now,
    )?;
    let host_label = hostname.unwrap_or_else(|| host_id.to_string());
    let provider_body = if paid {
        format!(
            "Your hosted machine {host_label} is now rented by {client_email} as Client {label}. Please keep the server stable. The Client owner has a three-day evaluation/payment window. Watch your configured payment accounts; the Router records declarations but does not verify receipt."
        )
    } else {
        format!(
            "Your hosted machine {host_label} is now used by {client_email} as Client {label} under your free-forever offer. Please keep the server stable. You may update the offer or release the Client from Client Market."
        )
    };
    enqueue_email_tx(
        tx,
        "client_allocated_provider",
        &host_email,
        &format!("[Client Market] Host {host_label} was allocated"),
        &provider_body,
        &format!("host-allocated:{installation_id}:{provider_id}"),
        now,
    )?;
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
            "priceCents": price,
            "rentalPeriodDays": period,
            "offerRevision": revision,
            "paymentDeadline": deadline.map(|value| value.to_rfc3339()),
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
    block_client_for_provider: bool,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let subscription: Option<(String, String, String, String)> = tx
        .query_row(
            "SELECT s.provider_id, s.client_user_id, s.client_owner_email,
                    COALESCE(p.owner_email, s.host_owner_email)
             FROM client_market_subscriptions s
             LEFT JOIN host_provider_profiles p ON p.provider_id = s.provider_id
             WHERE s.installation_id = ?1",
            params![installation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("read cleanup subscription failed: {error}"))
        })?;
    if let Some((provider_id, client_user_id, client_owner_email, provider_email)) = subscription {
        tx.execute(
            "UPDATE client_market_subscriptions
             SET status = ?2, updated_at = ?3 WHERE installation_id = ?1 AND status != 'released'",
            params![installation_id, SUBSCRIPTION_RELEASING, now.to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("mark subscription releasing failed: {error}"))
        })?;
        tx.execute(
            "UPDATE client_market_invoices SET status = 'canceled', canceled_at = ?2
             WHERE installation_id = ?1 AND status = 'open'",
            params![installation_id, now.to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("cancel releasing invoice failed: {error}")))?;
        if block_client_for_provider {
            tx.execute(
                "INSERT INTO host_provider_client_blocks
                    (provider_id, client_user_id, client_owner_email, reason, created_at, lifted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL)
                 ON CONFLICT(provider_id, client_user_id) DO UPDATE SET
                    client_owner_email = excluded.client_owner_email,
                    reason = excluded.reason, created_at = excluded.created_at, lifted_at = NULL",
                params![
                    provider_id,
                    client_user_id,
                    client_owner_email,
                    reason,
                    now.to_rfc3339()
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("block Client from Provider failed: {error}"))
            })?;
        }
        let label = client_label_tx(tx, installation_id)?;
        let client_body = match reason {
            "payment_not_received" => format!(
                "The Host Provider started removing Client {label} because payment was not received. The Router does not verify payment receipt or arbitrate this decision. The tunnel is disabled immediately and remote data may be permanently deleted."
            ),
            "payment_deadline_expired" => format!(
                "Client {label} passed its three-day payment declaration deadline. The Router disabled the tunnel and started remote cleanup. Data may be permanently deleted."
            ),
            _ => format!(
                "Cleanup started for Client {label} ({reason}). The tunnel is disabled immediately and remote data may be permanently deleted."
            ),
        };
        enqueue_email_tx(
            tx,
            "client_cleanup_started",
            &client_owner_email,
            &format!("[Client Market] Cleanup started for {label}"),
            &client_body,
            &format!("cleanup-started:{installation_id}:{reason}:client"),
            now,
        )?;
        enqueue_email_tx(
            tx,
            "provider_cleanup_started",
            &provider_email,
            &format!("[Client Market] Cleanup started for {label}"),
            &format!(
                "Cleanup started for Client {label} on your Host ({reason}). The Router has disabled the tunnel and will report the final SSH cleanup result. The Router does not verify payment receipt."
            ),
            &format!("cleanup-started:{installation_id}:{reason}:provider"),
            now,
        )?;
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
        serde_json::json!({ "reason": reason, "providerBlockedClient": block_client_for_provider }),
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
    let parties = cleanup_parties_tx(tx, installation_id)?;
    tx.execute(
        "UPDATE client_market_subscriptions
         SET status = ?2, payment_deadline = NULL, released_at = ?3, updated_at = ?3
         WHERE installation_id = ?1",
        params![installation_id, SUBSCRIPTION_RELEASED, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("finish subscription release failed: {error}")))?;
    if let Some((client_email, provider_email, label)) = parties {
        for (kind, recipient, role) in [
            ("client_cleanup_finished", client_email, "Client owner"),
            ("provider_cleanup_finished", provider_email, "Host Provider"),
        ] {
            enqueue_email_tx(
                tx,
                kind,
                &recipient,
                &format!("[Client Market] Cleanup completed for {label}"),
                &format!(
                    "Remote cleanup completed for Client {label}. The Host is idle and available for allocation again. This final result is recorded for the {role}."
                ),
                &format!("cleanup-finished:{installation_id}:{kind}"),
                now,
            )?;
        }
    }
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
    let parties = cleanup_parties_tx(tx, installation_id)?;
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
    if let Some((client_email, provider_email, label)) = parties {
        for (kind, recipient) in [
            ("client_cleanup_failed", client_email),
            ("provider_cleanup_failed", provider_email),
        ] {
            enqueue_email_tx(
                tx,
                kind,
                &recipient,
                &format!("[Client Market] Cleanup failed for {label}"),
                &format!(
                    "Remote cleanup failed for Client {label} ({failure_code}). The Client stays in release_failed, its tunnel stays disabled, and creating new Clients remains blocked for the Client owner. This state does not clear on its own — contact the Router administrator to force release it."
                ),
                &format!("cleanup-failed:{installation_id}:{failure_code}:{kind}"),
                now,
            )?;
        }
    }
    insert_audit_tx(
        tx,
        Some(installation_id),
        Some(host_id),
        None,
        None,
        "cleanup_failed",
        serde_json::json!({ "failureCode": failure_code }),
        now,
    )?;
    Ok(())
}

fn cleanup_parties_tx(
    tx: &Transaction<'_>,
    installation_id: &str,
) -> Result<Option<(String, String, String)>, AppError> {
    tx.query_row(
        "SELECT s.client_owner_email, COALESCE(p.owner_email, s.host_owner_email),
                COALESCE(t.subdomain, s.installation_id)
         FROM client_market_subscriptions s
         LEFT JOIN host_provider_profiles p ON p.provider_id = s.provider_id
         LEFT JOIN installation_client_tunnels t ON t.installation_id = s.installation_id
         WHERE s.installation_id = ?1",
        params![installation_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(|error| {
        AppError::Internal(format!("read cleanup notification parties failed: {error}"))
    })
}

pub async fn run_trade_service(state: ServerState) -> anyhow::Result<()> {
    crate::client_market::reconcile_interrupted_host_import_jobs(state.clone()).await?;
    let email_state = state.clone();
    let billing_state = state;
    tokio::try_join!(
        run_market_email_worker(email_state),
        run_billing_worker(billing_state),
    )?;
    Ok(())
}

async fn run_market_email_worker(state: ServerState) -> anyhow::Result<()> {
    let worker_id = format!("client-market-email-{}", Uuid::new_v4());
    let http = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 client-market-email")
        .connect_timeout(StdDuration::from_secs(10))
        .timeout(StdDuration::from_secs(20))
        .build()?;
    let template = NotificationTemplateContext::from_config(&state.config);
    let credentials = state
        .config
        .resend_api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .zip(template.sender.as_deref());
    if credentials.is_none() {
        tracing::warn!(
            "Client Market email delivery is not configured; queued messages will remain pending"
        );
    }
    let mut interval = tokio::time::interval(StdDuration::from_secs(EMAIL_CYCLE_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        for _ in 0..25 {
            let claim = match state
                .store
                .client_market_claim_email(&worker_id, Utc::now(), credentials.is_some())
                .await
            {
                Ok(Some(claim)) => claim,
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(error = %error, "Client Market email claim failed");
                    break;
                }
            };
            let Some((api_key, sender)) = credentials else {
                tracing::error!("Client Market email was claimed without delivery credentials");
                break;
            };
            let envelope = FrozenEmailEnvelope {
                from: sender,
                recipient: &claim.recipient,
                subject: &claim.subject,
                html: &claim.html,
                text: &claim.text,
                reply_to: template.reply_to.as_deref(),
                idempotency_key: &claim.idempotency_key,
            };
            match send_resend_frozen_email(&http, api_key, envelope).await {
                Ok(provider_id) => {
                    if let Err(error) = state
                        .store
                        .client_market_finish_email(
                            &claim.id,
                            &worker_id,
                            Some(&provider_id),
                            None,
                            None,
                        )
                        .await
                    {
                        tracing::warn!(email_id = %claim.id, error = %error, "Client Market email completion failed");
                        break;
                    }
                }
                Err(failure) => {
                    let message = sanitize_delivery_error(&failure.message);
                    let retry = failure.retryable && claim.attempts < MAX_EMAIL_ATTEMPTS;
                    let retry_at = retry.then(|| {
                        failure.retry_at.unwrap_or_else(|| {
                            Utc::now()
                                + Duration::seconds(retry_delay_secs(claim.attempts, &claim.id))
                        })
                    });
                    if let Err(error) = state
                        .store
                        .client_market_finish_email(
                            &claim.id,
                            &worker_id,
                            None,
                            Some(&message),
                            retry_at,
                        )
                        .await
                    {
                        tracing::warn!(email_id = %claim.id, error = %error, "Client Market email retry update failed");
                        break;
                    }
                }
            }
        }
    }
}

async fn run_billing_worker(state: ServerState) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(StdDuration::from_secs(BILLING_CYCLE_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let expired = match state
            .store
            .client_market_reconcile_trade_state(Utc::now())
            .await
        {
            Ok(expired) => expired,
            Err(error) => {
                tracing::warn!(error = %error, "Client Market billing reconcile failed");
                continue;
            }
        };
        if let Err(error) = state
            .store
            .client_market_record_provider_daily_stats(Utc::now(), state.config.client_stale_secs)
            .await
        {
            tracing::warn!(error = %error, "Client Market Provider observation snapshot failed");
        }
        for client in expired {
            match state
                .store
                .client_market_begin_system_cleanup_job(
                    &client.installation_id,
                    "payment_deadline_expired",
                )
                .await
            {
                Ok(job_id) => {
                    if let Some(subdomain) = client.subdomain.as_deref() {
                        state.proxy.remove_route(subdomain).await;
                    }
                    let runner_state = state.clone();
                    tokio::spawn(async move {
                        if let Err(error) =
                            crate::client_market::run_cleanup_job(runner_state, job_id.clone())
                                .await
                        {
                            tracing::error!(job_id = %job_id, error = %error, "automatic Client Market release failed");
                        }
                    });
                }
                Err(AppError::Conflict(_)) | Err(AppError::NotFound(_)) => {}
                Err(error) => {
                    tracing::warn!(installation_id = %client.installation_id, error = %error, "could not start automatic Client Market release");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

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
}
