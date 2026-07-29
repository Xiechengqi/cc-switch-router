use std::collections::{BTreeMap, HashSet};
use std::time::Duration as StdDuration;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Months, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ServerState;
use crate::client_market_trade::PaymentContact;
use crate::client_market_trade::PaymentMethod;
use crate::error::AppError;
use crate::models::{
    AuthSession, ShareEditAvailableEvent, ShareGrantManager, ShareManagedGrantAction,
    ShareManagedGrantOperation, ShareSettingsPatch, ShareTokenPeriod, ShareUserGrant,
    ShareUserPolicy,
};
use crate::store::AppStore;

const TRIAL_HOURS: i64 = 72;
const RENEWAL_GRACE_HOURS: i64 = 72;
const SERVICE_CYCLE_SECS: u64 = 5;
const MAX_SEATS_PER_LISTING: usize = 20;
const MAX_CONTROL_ATTEMPTS: i64 = 8;

const SEAT_AVAILABLE: &str = "available";
const SEAT_DISABLED: &str = "disabled";
const SEAT_DELETED: &str = "deleted";

const SUB_GRANT_PENDING: &str = "grant_pending";
const SUB_TRIAL_PAYMENT_DUE: &str = "trial_payment_due";
const SUB_ACTIVE_PAID: &str = "active_paid";
const SUB_RENEWAL_DUE: &str = "renewal_due";
const SUB_REVOKE_PENDING: &str = "revoke_pending";
const SUB_REVOKE_FAILED: &str = "revoke_failed";
const SUB_GRANT_FAILED: &str = "grant_failed";
const SUB_RELEASED: &str = "released";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketCatalog {
    pub listings: Vec<ListingView>,
    pub my_subscriptions: Vec<SubscriptionView>,
    pub owner_blocks: Vec<OwnerBlockView>,
    pub trial_hours: i64,
    pub renewal_grace_hours: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingView {
    pub id: String,
    pub share_id: String,
    pub share_name: String,
    pub app_type: String,
    pub owner_email: String,
    pub status: String,
    pub share_status: String,
    pub subdomain: String,
    pub share_online: bool,
    pub is_owner: bool,
    #[serde(default)]
    pub contacts: Vec<PaymentContact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<crate::models::ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<i64>,
    #[serde(default)]
    pub tokens_used: i64,
    pub supported_user_token_periods: Vec<ShareTokenPeriod>,
    pub seats: Vec<SeatView>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatView {
    pub id: String,
    pub position: i64,
    pub status: String,
    pub parallel_limit: Option<u32>,
    pub token_limit: Option<u64>,
    pub token_period: ShareTokenPeriod,
    pub price_minor: Option<i64>,
    pub currency: Option<String>,
    pub period_unit: Option<String>,
    pub period_count: Option<u32>,
    pub offer_revision: i64,
    pub is_free: bool,
    pub can_rent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<SubscriptionView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionView {
    pub id: String,
    pub seat_id: String,
    pub listing_id: String,
    pub share_id: String,
    pub share_name: String,
    pub app_type: String,
    pub subdomain: String,
    pub share_online: bool,
    pub owner_email: String,
    pub renter_email: String,
    pub status: String,
    pub price_minor: Option<i64>,
    pub currency: Option<String>,
    pub period_unit: Option<String>,
    pub period_count: Option<u32>,
    pub offer_revision: i64,
    pub trial_ends_at: Option<String>,
    pub current_period_end: Option<String>,
    pub payment_deadline: Option<String>,
    pub open_invoice: Option<InvoiceView>,
    pub payment_methods: Option<Vec<PaymentMethod>>,
    #[serde(default)]
    pub contacts: Vec<PaymentContact>,
    pub payment_profile_updated_at: Option<String>,
    pub can_declare_paid: bool,
    pub can_release: bool,
    pub can_force_revoke: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceView {
    pub id: String,
    pub sequence: i64,
    pub amount_minor: i64,
    pub currency: String,
    pub period_unit: String,
    pub period_count: u32,
    pub status: String,
    pub due_at: String,
    pub deadline_at: String,
    pub opened_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerBlockView {
    pub blocked_user_id: String,
    pub blocked_email: String,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedShareView {
    pub share_id: String,
    pub share_name: String,
    pub app_type: String,
    pub share_status: String,
    pub already_listed: bool,
    pub supported_user_token_periods: Vec<ShareTokenPeriod>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeatInput {
    pub parallel_limit: Option<u32>,
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub token_period: ShareTokenPeriod,
    pub price_minor: Option<i64>,
    pub currency: Option<String>,
    pub period_unit: Option<String>,
    pub period_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateListingRequest {
    pub share_id: String,
    pub seats: Vec<SeatInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSeatRequest {
    pub seat: SeatInput,
    pub offer_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RentSeatRequest {
    pub offer_revision: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeclarePaidRequest {
    pub invoice_id: String,
    pub offer_revision: i64,
    pub amount_minor_confirmed: i64,
    pub payment_profile_updated_at: String,
    #[serde(default)]
    pub confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForceRevokeRequest {
    #[serde(default)]
    pub block_user: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
struct NormalizedSeat {
    parallel_limit: Option<u32>,
    token_limit: Option<u64>,
    token_period: ShareTokenPeriod,
    price_minor: Option<i64>,
    currency: Option<String>,
    period_unit: Option<String>,
    period_count: Option<u32>,
}

impl NormalizedSeat {
    fn is_free(&self) -> bool {
        self.price_minor.is_none()
    }
}

pub fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS share_market_listings (
            id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            owner_user_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'closed')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_share_market_active_listing
            ON share_market_listings(share_id) WHERE status = 'active';
        CREATE TABLE IF NOT EXISTS share_market_seats (
            id TEXT PRIMARY KEY,
            listing_id TEXT NOT NULL,
            position INTEGER NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('available', 'reserved', 'occupied', 'revoking', 'disabled', 'deleted')),
            parallel_limit INTEGER,
            token_limit INTEGER,
            token_period_json TEXT NOT NULL,
            price_minor INTEGER,
            currency TEXT,
            period_unit TEXT,
            period_count INTEGER,
            offer_revision INTEGER NOT NULL DEFAULT 1,
            current_subscription_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(listing_id, position),
            FOREIGN KEY(listing_id) REFERENCES share_market_listings(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS share_market_subscriptions (
            id TEXT PRIMARY KEY,
            seat_id TEXT NOT NULL,
            listing_id TEXT NOT NULL,
            share_id TEXT NOT NULL,
            entitlement_id TEXT NOT NULL UNIQUE,
            owner_user_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            renter_user_id TEXT NOT NULL,
            renter_email TEXT NOT NULL,
            status TEXT NOT NULL,
            parallel_limit INTEGER,
            token_limit INTEGER,
            token_period_json TEXT NOT NULL,
            price_minor INTEGER,
            currency TEXT,
            period_unit TEXT,
            period_count INTEGER,
            offer_revision INTEGER NOT NULL,
            trial_ends_at TEXT,
            current_period_end TEXT,
            payment_deadline TEXT,
            release_reason TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            released_at TEXT,
            FOREIGN KEY(seat_id) REFERENCES share_market_seats(id),
            FOREIGN KEY(listing_id) REFERENCES share_market_listings(id)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_share_market_active_seat
            ON share_market_subscriptions(seat_id) WHERE status NOT IN ('released', 'grant_failed');
        CREATE UNIQUE INDEX IF NOT EXISTS idx_share_market_active_renter_share
            ON share_market_subscriptions(renter_user_id, share_id)
            WHERE status NOT IN ('released', 'grant_failed');
        CREATE TABLE IF NOT EXISTS share_market_invoices (
            id TEXT PRIMARY KEY,
            subscription_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            amount_minor INTEGER NOT NULL,
            currency TEXT NOT NULL,
            period_unit TEXT NOT NULL,
            period_count INTEGER NOT NULL,
            offer_revision INTEGER NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('open', 'declared', 'canceled')),
            due_at TEXT NOT NULL,
            deadline_at TEXT NOT NULL,
            opened_at TEXT NOT NULL,
            declared_at TEXT,
            canceled_at TEXT,
            UNIQUE(subscription_id, sequence),
            FOREIGN KEY(subscription_id) REFERENCES share_market_subscriptions(id)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_share_market_open_invoice
            ON share_market_invoices(subscription_id) WHERE status = 'open';
        CREATE TABLE IF NOT EXISTS share_market_payment_declarations (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL UNIQUE,
            subscription_id TEXT NOT NULL,
            renter_user_id TEXT NOT NULL,
            renter_email TEXT NOT NULL,
            declared_at TEXT NOT NULL,
            FOREIGN KEY(invoice_id) REFERENCES share_market_invoices(id),
            FOREIGN KEY(subscription_id) REFERENCES share_market_subscriptions(id)
        );
        CREATE TABLE IF NOT EXISTS share_market_owner_blocks (
            owner_user_id TEXT NOT NULL,
            blocked_user_id TEXT NOT NULL,
            blocked_email TEXT NOT NULL,
            reason TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            lifted_at TEXT,
            PRIMARY KEY(owner_user_id, blocked_user_id)
        );
        CREATE TABLE IF NOT EXISTS share_control_operations (
            id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            share_sequence INTEGER NOT NULL,
            entitlement_id TEXT NOT NULL,
            subscription_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('upsert', 'revoke')),
            email TEXT NOT NULL,
            policy_json TEXT,
            status TEXT NOT NULL CHECK (status IN ('pending', 'dispatched', 'applied', 'rejected')),
            edit_id TEXT UNIQUE,
            attempts INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            applied_at TEXT,
            UNIQUE(share_id, share_sequence),
            FOREIGN KEY(subscription_id) REFERENCES share_market_subscriptions(id)
        );
        CREATE INDEX IF NOT EXISTS idx_share_control_dispatch
            ON share_control_operations(status, share_id, share_sequence);
        CREATE TABLE IF NOT EXISTS share_market_events (
            id TEXT PRIMARY KEY,
            listing_id TEXT,
            seat_id TEXT,
            subscription_id TEXT,
            actor_user_id TEXT,
            actor_email TEXT,
            event_type TEXT NOT NULL,
            detail_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_share_market_subscriptions_reconcile
            ON share_market_subscriptions(status, payment_deadline, current_period_end);
        CREATE INDEX IF NOT EXISTS idx_share_market_subscriptions_owner
            ON share_market_subscriptions(owner_user_id, status);
        CREATE INDEX IF NOT EXISTS idx_share_market_subscriptions_renter
            ON share_market_subscriptions(renter_user_id, status);",
    )?;
    Ok(())
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/share-market/listings",
            get(list_catalog).post(create_listing),
        )
        .route("/v1/share-market/owned-shares", get(list_owned_shares))
        .route("/v1/share-market/listings/:id", delete(close_listing))
        .route("/v1/share-market/listings/:id/seats", post(add_seat))
        .route(
            "/v1/share-market/seats/:id",
            patch(update_seat).delete(delete_seat),
        )
        .route("/v1/share-market/seats/:id/rent", post(rent_seat))
        .route(
            "/v1/share-market/subscriptions/:id/declare-paid",
            post(declare_paid),
        )
        .route(
            "/v1/share-market/subscriptions/:id/release",
            post(release_subscription),
        )
        .route(
            "/v1/share-market/subscriptions/:id/force-revoke",
            post(force_revoke_subscription),
        )
        .route("/v1/share-market/blocks", get(list_owner_blocks))
        .route("/v1/share-market/blocks/:user_id", delete(lift_owner_block))
}

fn normalize_seat(input: SeatInput) -> Result<NormalizedSeat, AppError> {
    if input.parallel_limit == Some(0) || input.token_limit == Some(0) {
        return Err(AppError::BadRequest(
            "seat limits must be positive or empty for unlimited".into(),
        ));
    }
    if input
        .token_limit
        .is_some_and(|value| value > i64::MAX as u64)
    {
        return Err(AppError::BadRequest("seat token limit is too large".into()));
    }
    let price_minor = input.price_minor;
    let currency = input
        .currency
        .map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| !value.is_empty());
    let period_unit = input
        .period_unit
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let period_count = input.period_count;
    let pricing_empty = price_minor.is_none()
        && currency.is_none()
        && period_unit.is_none()
        && period_count.is_none();
    if pricing_empty {
        return Ok(NormalizedSeat {
            parallel_limit: input.parallel_limit,
            token_limit: input.token_limit,
            token_period: input.token_period,
            price_minor: None,
            currency: None,
            period_unit: None,
            period_count: None,
        });
    }
    let price_minor = price_minor.ok_or_else(|| {
        AppError::BadRequest("price and billing period must both be set or both be empty".into())
    })?;
    if price_minor <= 0 || price_minor > 1_000_000_000_000 {
        return Err(AppError::BadRequest(
            "paid seat price must be a positive minor-unit amount".into(),
        ));
    }
    let currency =
        currency.ok_or_else(|| AppError::BadRequest("paid seat currency is required".into()))?;
    if currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(AppError::BadRequest(
            "currency must be a three-letter ISO code".into(),
        ));
    }
    let period_unit = period_unit.ok_or_else(|| {
        AppError::BadRequest("price and billing period must both be set or both be empty".into())
    })?;
    if !matches!(period_unit.as_str(), "day" | "week" | "month") {
        return Err(AppError::BadRequest(
            "billing period unit must be day, week, or month".into(),
        ));
    }
    let period_count = period_count.ok_or_else(|| {
        AppError::BadRequest("price and billing period must both be set or both be empty".into())
    })?;
    if !(1..=365).contains(&period_count) {
        return Err(AppError::BadRequest(
            "billing period count must be between 1 and 365".into(),
        ));
    }
    Ok(NormalizedSeat {
        parallel_limit: input.parallel_limit,
        token_limit: input.token_limit,
        token_period: input.token_period,
        price_minor: Some(price_minor),
        currency: Some(currency),
        period_unit: Some(period_unit),
        period_count: Some(period_count),
    })
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated user session required".into()))
}

fn map_db(context: &'static str) -> impl FnOnce(rusqlite::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AppError::Internal("stored Share Market timestamp is invalid".into()))
}

fn add_billing_period(
    start: DateTime<Utc>,
    unit: &str,
    count: u32,
) -> Result<DateTime<Utc>, AppError> {
    match unit {
        "day" => Ok(start + Duration::days(i64::from(count))),
        "week" => Ok(start + Duration::weeks(i64::from(count))),
        "month" => start
            .checked_add_months(Months::new(count))
            .ok_or_else(|| AppError::Internal("billing period overflow".into())),
        _ => Err(AppError::Internal(
            "stored billing period is invalid".into(),
        )),
    }
}

fn token_period_anchor_at_ms(period: ShareTokenPeriod, now: DateTime<Utc>) -> Option<i64> {
    matches!(
        period,
        ShareTokenPeriod::SevenDays | ShareTokenPeriod::ThirtyDays
    )
    .then(|| now.timestamp_millis().div_euclid(60_000) * 60_000)
}

#[allow(clippy::too_many_arguments)]
fn event_tx(
    tx: &Transaction<'_>,
    listing_id: Option<&str>,
    seat_id: Option<&str>,
    subscription_id: Option<&str>,
    actor: Option<&AuthSession>,
    event_type: &str,
    detail: serde_json::Value,
    now: &str,
) -> Result<(), AppError> {
    tx.execute(
        "INSERT INTO share_market_events (
            id, listing_id, seat_id, subscription_id, actor_user_id, actor_email,
            event_type, detail_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            Uuid::new_v4().to_string(),
            listing_id,
            seat_id,
            subscription_id,
            actor.map(|value| value.user_id.as_str()),
            actor.map(|value| value.email.as_str()),
            event_type,
            detail.to_string(),
            now,
        ],
    )
    .map_err(map_db("record Share Market event"))?;
    Ok(())
}

#[derive(Debug, Clone)]
struct SubscriptionRecord {
    id: String,
    seat_id: String,
    listing_id: String,
    share_id: String,
    share_name: String,
    app_type: String,
    subdomain: String,
    entitlement_id: String,
    owner_user_id: String,
    owner_email: String,
    renter_user_id: String,
    renter_email: String,
    status: String,
    price_minor: Option<i64>,
    currency: Option<String>,
    period_unit: Option<String>,
    period_count: Option<u32>,
    offer_revision: i64,
    trial_ends_at: Option<String>,
    current_period_end: Option<String>,
    payment_deadline: Option<String>,
    created_at: String,
    updated_at: String,
}

fn subscription_record(
    conn: &Connection,
    subscription_id: &str,
) -> Result<Option<SubscriptionRecord>, AppError> {
    conn.query_row(
        "SELECT sub.id, sub.seat_id, sub.listing_id, sub.share_id,
                COALESCE(s.share_name, sub.share_id), COALESCE(s.app_type, ''),
                COALESCE(s.subdomain, ''),
                sub.entitlement_id, sub.owner_user_id, sub.owner_email,
                sub.renter_user_id, sub.renter_email, sub.status,
                sub.price_minor, sub.currency, sub.period_unit, sub.period_count,
                sub.offer_revision, sub.trial_ends_at, sub.current_period_end,
                sub.payment_deadline, sub.created_at, sub.updated_at
         FROM share_market_subscriptions sub
         LEFT JOIN shares s ON s.share_id = sub.share_id
         WHERE sub.id = ?1",
        params![subscription_id],
        |row| {
            Ok(SubscriptionRecord {
                id: row.get(0)?,
                seat_id: row.get(1)?,
                listing_id: row.get(2)?,
                share_id: row.get(3)?,
                share_name: row.get(4)?,
                app_type: row.get(5)?,
                subdomain: row.get(6)?,
                entitlement_id: row.get(7)?,
                owner_user_id: row.get(8)?,
                owner_email: row.get(9)?,
                renter_user_id: row.get(10)?,
                renter_email: row.get(11)?,
                status: row.get(12)?,
                price_minor: row.get(13)?,
                currency: row.get(14)?,
                period_unit: row.get(15)?,
                period_count: row
                    .get::<_, Option<i64>>(16)?
                    .and_then(|value| u32::try_from(value).ok()),
                offer_revision: row.get(17)?,
                trial_ends_at: row.get(18)?,
                current_period_end: row.get(19)?,
                payment_deadline: row.get(20)?,
                created_at: row.get(21)?,
                updated_at: row.get(22)?,
            })
        },
    )
    .optional()
    .map_err(map_db("read Share Market subscription"))
}

fn open_invoice(conn: &Connection, subscription_id: &str) -> Result<Option<InvoiceView>, AppError> {
    conn.query_row(
        "SELECT id, sequence, amount_minor, currency, period_unit, period_count,
                status, due_at, deadline_at, opened_at
         FROM share_market_invoices
         WHERE subscription_id = ?1 AND status = 'open'
         LIMIT 1",
        params![subscription_id],
        |row| {
            Ok(InvoiceView {
                id: row.get(0)?,
                sequence: row.get(1)?,
                amount_minor: row.get(2)?,
                currency: row.get(3)?,
                period_unit: row.get(4)?,
                period_count: u32::try_from(row.get::<_, i64>(5)?).unwrap_or(1),
                status: row.get(6)?,
                due_at: row.get(7)?,
                deadline_at: row.get(8)?,
                opened_at: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(map_db("read Share Market invoice"))
}

fn payment_profile(
    conn: &Connection,
    owner_user_id: &str,
) -> Result<Option<(Vec<PaymentMethod>, Vec<PaymentContact>, String)>, AppError> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT methods_json, COALESCE(contacts_json, '[]'), updated_at
             FROM account_payment_profiles WHERE user_id = ?1",
            params![owner_user_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(map_db("read Share Market payment profile"))?;
    Ok(row.map(|(methods, contacts, updated_at)| {
        (
            serde_json::from_str::<Vec<PaymentMethod>>(&methods).unwrap_or_default(),
            serde_json::from_str::<Vec<PaymentContact>>(&contacts).unwrap_or_default(),
            updated_at,
        )
    }))
}

fn subscription_view(
    conn: &Connection,
    record: SubscriptionRecord,
    viewer: Option<&AuthSession>,
    active_subdomains: &[String],
) -> Result<SubscriptionView, AppError> {
    let is_renter = viewer.is_some_and(|session| session.user_id == record.renter_user_id);
    let is_owner = viewer.is_some_and(|session| session.user_id == record.owner_user_id);
    let open_invoice = if is_renter || is_owner {
        open_invoice(conn, &record.id)?
    } else {
        None
    };
    let (payment_methods, contacts, payment_profile_updated_at) =
        if is_renter && open_invoice.is_some() && record.price_minor.is_some() {
            payment_profile(conn, &record.owner_user_id)?
                .map(|(methods, contacts, updated_at)| {
                    (Some(methods), contacts, Some(updated_at))
                })
                .unwrap_or((Some(Vec::new()), Vec::new(), None))
        } else if is_renter || is_owner {
            payment_profile(conn, &record.owner_user_id)?
                .map(|(_, contacts, _)| (None, contacts, None))
                .unwrap_or((None, Vec::new(), None))
        } else {
            (None, Vec::new(), None)
        };
    let can_declare_paid = is_renter
        && open_invoice.is_some()
        && matches!(
            record.status.as_str(),
            SUB_TRIAL_PAYMENT_DUE | SUB_RENEWAL_DUE
        );
    let can_release = is_renter
        && !matches!(
            record.status.as_str(),
            SUB_RELEASED | SUB_GRANT_FAILED | SUB_REVOKE_PENDING
        );
    // Allow retry while revoke is stuck (e.g. earlier grant edit blocked dispatch).
    let can_force_revoke = is_owner
        && !matches!(record.status.as_str(), SUB_RELEASED | SUB_GRANT_FAILED);
    let share_online =
        !record.subdomain.is_empty() && active_subdomains.contains(&record.subdomain);
    Ok(SubscriptionView {
        id: record.id,
        seat_id: record.seat_id,
        listing_id: record.listing_id,
        share_id: record.share_id,
        share_name: record.share_name,
        app_type: record.app_type,
        subdomain: record.subdomain,
        share_online,
        owner_email: record.owner_email,
        renter_email: record.renter_email,
        status: record.status,
        price_minor: record.price_minor,
        currency: record.currency,
        period_unit: record.period_unit,
        period_count: record.period_count,
        offer_revision: record.offer_revision,
        trial_ends_at: record.trial_ends_at,
        current_period_end: record.current_period_end,
        payment_deadline: record.payment_deadline,
        open_invoice,
        payment_methods,
        contacts,
        payment_profile_updated_at,
        can_declare_paid,
        can_release,
        can_force_revoke,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

impl AppStore {
    pub async fn share_market_owned_shares(
        &self,
        session: &AuthSession,
    ) -> Result<Vec<OwnedShareView>, AppError> {
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT s.share_id, s.share_name, s.app_type, s.share_status,
                        CASE WHEN EXISTS (
                            SELECT 1 FROM share_market_listings listing
                            WHERE listing.share_id = s.share_id
                              AND listing.status = 'active'
                              AND lower(listing.owner_email) = lower(s.owner_email)
                        ) OR EXISTS (
                            SELECT 1 FROM share_market_subscriptions sub
                            WHERE sub.share_id = s.share_id
                              AND sub.status NOT IN ('released', 'grant_failed')
                        ) THEN 1 ELSE 0 END,
                        COALESCE(s.supported_user_token_periods_json, '[]')
                 FROM shares s
                 WHERE lower(s.owner_email) = lower(?1)
                 ORDER BY s.share_name, s.share_id",
            )
            .map_err(map_db("prepare owned Share list"))?;
        let rows = statement
            .query_map(params![session.email], |row| {
                let periods_json: String = row.get(5)?;
                Ok(OwnedShareView {
                    share_id: row.get(0)?,
                    share_name: row.get(1)?,
                    app_type: row.get(2)?,
                    share_status: row.get(3)?,
                    already_listed: row.get::<_, i64>(4)? != 0,
                    supported_user_token_periods: serde_json::from_str(&periods_json)
                        .unwrap_or_else(|_| vec![ShareTokenPeriod::Lifetime]),
                })
            })
            .map_err(map_db("query owned Share list"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read owned Share list"))
    }

    pub async fn share_market_catalog(
        &self,
        viewer: Option<&AuthSession>,
        active_subdomains: &[String],
    ) -> Result<ShareMarketCatalog, AppError> {
        let conn = self.conn.lock().await;
        let mut listings_statement = conn
            .prepare(
                "SELECT listing.id, listing.share_id, COALESCE(s.share_name, listing.share_id),
                        COALESCE(s.app_type, ''), listing.owner_user_id, listing.owner_email,
                        listing.status, COALESCE(s.share_status, 'missing'),
                        COALESCE(s.subdomain, ''), listing.created_at, listing.updated_at,
                        COALESCE(s.supported_user_token_periods_json, '[]'),
                        COALESCE(s.owner_email, ''), COALESCE(s.user_grants_json, '{}'),
                        s.upstream_provider_json, s.token_limit, s.parallel_limit,
                        COALESCE(s.tokens_used, 0)
                 FROM share_market_listings listing
                 LEFT JOIN shares s ON s.share_id = listing.share_id
                 WHERE (listing.status = 'active'
                        AND lower(COALESCE(s.owner_email, '')) = lower(listing.owner_email))
                    OR listing.owner_user_id = ?1
                    OR EXISTS (
                        SELECT 1 FROM share_market_subscriptions sub
                        WHERE sub.listing_id = listing.id AND sub.renter_user_id = ?1
                          AND sub.status NOT IN ('released', 'grant_failed')
                    )
                 ORDER BY listing.created_at DESC",
            )
            .map_err(map_db("prepare Share Market catalog"))?;
        let viewer_user_id = viewer.map(|value| value.user_id.as_str()).unwrap_or("");
        let listing_rows = listings_statement
            .query_map(params![viewer_user_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                    row.get::<_, Option<i64>>(16)?,
                    row.get::<_, i64>(17)?,
                ))
            })
            .map_err(map_db("query Share Market catalog"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read Share Market catalog"))?;
        drop(listings_statement);

        let mut listings = Vec::with_capacity(listing_rows.len());
        for (
            id,
            share_id,
            share_name,
            app_type,
            owner_user_id,
            owner_email,
            status,
            share_status,
            subdomain,
            created_at,
            updated_at,
            supported_periods_json,
            current_owner_email,
            grants_json,
            upstream_provider_json,
            token_limit,
            parallel_limit,
            tokens_used,
        ) in listing_rows
        {
            let is_owner = viewer.is_some_and(|value| value.user_id == owner_user_id);
            let viewer_blocked = if let Some(session) = viewer {
                conn.query_row(
                    "SELECT 1 FROM share_market_owner_blocks
                     WHERE owner_user_id = ?1 AND blocked_user_id = ?2 AND lifted_at IS NULL",
                    params![owner_user_id, session.user_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(map_db("check Share Market catalog block"))?
                .is_some()
            } else {
                false
            };
            let viewer_already_renting = if let Some(session) = viewer {
                conn.query_row(
                    "SELECT 1 FROM share_market_subscriptions
                     WHERE renter_user_id = ?1 AND share_id = ?2
                       AND status NOT IN ('released', 'grant_failed')
                     LIMIT 1",
                    params![session.user_id, share_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(map_db("check Share Market catalog rental"))?
                .is_some()
            } else {
                false
            };
            let viewer_has_direct_grant = viewer.is_some_and(|session| {
                let grants: BTreeMap<String, ShareUserGrant> =
                    serde_json::from_str(&grants_json).unwrap_or_default();
                grants
                    .get(&session.email.to_ascii_lowercase())
                    .is_some_and(|grant| grant.active)
            });
            let mut seat_statement = conn
                .prepare(
                    "SELECT id, position, status, parallel_limit, token_limit,
                            token_period_json, price_minor, currency, period_unit,
                            period_count, offer_revision, current_subscription_id
                     FROM share_market_seats
                     WHERE listing_id = ?1 AND status != 'deleted'
                     ORDER BY position",
                )
                .map_err(map_db("prepare Share Market seats"))?;
            let seat_rows = seat_statement
                .query_map(params![id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<i64>>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, Option<String>>(11)?,
                    ))
                })
                .map_err(map_db("query Share Market seats"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_db("read Share Market seats"))?;
            drop(seat_statement);
            let mut seats = Vec::with_capacity(seat_rows.len());
            for (
                seat_id,
                position,
                seat_status,
                parallel_limit,
                token_limit,
                token_period_json,
                price_minor,
                currency,
                period_unit,
                period_count,
                offer_revision,
                subscription_id,
            ) in seat_rows
            {
                let subscription = match subscription_id {
                    Some(subscription_id) => subscription_record(&conn, &subscription_id)?
                        .filter(|record| {
                            is_owner
                                || viewer
                                    .is_some_and(|session| session.user_id == record.renter_user_id)
                        })
                        .map(|record| {
                            subscription_view(&conn, record, viewer, active_subdomains)
                        })
                        .transpose()?,
                    None => None,
                };
                let can_rent = viewer.is_some_and(|session| {
                    status == "active"
                        && share_status == "active"
                        && current_owner_email.eq_ignore_ascii_case(&owner_email)
                        && seat_status == SEAT_AVAILABLE
                        && session.user_id != owner_user_id
                        && !viewer_blocked
                        && !viewer_already_renting
                        && !viewer_has_direct_grant
                });
                seats.push(SeatView {
                    id: seat_id,
                    position,
                    status: seat_status,
                    parallel_limit: parallel_limit.and_then(|value| u32::try_from(value).ok()),
                    token_limit: token_limit.and_then(|value| u64::try_from(value).ok()),
                    token_period: serde_json::from_str(&token_period_json)
                        .unwrap_or(ShareTokenPeriod::Lifetime),
                    price_minor,
                    currency,
                    period_unit,
                    period_count: period_count.and_then(|value| u32::try_from(value).ok()),
                    offer_revision,
                    is_free: price_minor.is_none(),
                    can_rent,
                    subscription,
                });
            }
            let contacts = payment_profile(&conn, &owner_user_id)?
                .map(|(_, contacts, _)| contacts)
                .unwrap_or_default();
            listings.push(ListingView {
                id,
                share_id,
                share_name,
                app_type,
                owner_email,
                status,
                share_status,
                subdomain: subdomain.clone(),
                share_online: !subdomain.is_empty() && active_subdomains.contains(&subdomain),
                is_owner,
                contacts,
                upstream_provider: upstream_provider_json
                    .as_deref()
                    .and_then(|value| serde_json::from_str(value).ok()),
                token_limit,
                parallel_limit,
                tokens_used,
                supported_user_token_periods: serde_json::from_str(&supported_periods_json)
                    .unwrap_or_else(|_| vec![ShareTokenPeriod::Lifetime]),
                seats,
                created_at,
                updated_at,
            });
        }

        let mut my_subscriptions = Vec::new();
        let mut owner_blocks = Vec::new();
        if let Some(viewer) = viewer {
            let mut statement = conn
                .prepare(
                    "SELECT id FROM share_market_subscriptions
                     WHERE renter_user_id = ?1 AND status NOT IN ('released', 'grant_failed')
                     ORDER BY created_at DESC",
                )
                .map_err(map_db("prepare renter subscriptions"))?;
            let ids = statement
                .query_map(params![viewer.user_id], |row| row.get::<_, String>(0))
                .map_err(map_db("query renter subscriptions"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_db("read renter subscriptions"))?;
            drop(statement);
            for id in ids {
                if let Some(record) = subscription_record(&conn, &id)? {
                    my_subscriptions.push(subscription_view(
                        &conn,
                        record,
                        Some(viewer),
                        active_subdomains,
                    )?);
                }
            }
            owner_blocks = owner_blocks_for(&conn, &viewer.user_id)?;
        }
        Ok(ShareMarketCatalog {
            listings,
            my_subscriptions,
            owner_blocks,
            trial_hours: TRIAL_HOURS,
            renewal_grace_hours: RENEWAL_GRACE_HOURS,
        })
    }
}

fn owner_blocks_for(
    conn: &Connection,
    owner_user_id: &str,
) -> Result<Vec<OwnerBlockView>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT blocked_user_id, blocked_email, reason, created_at
             FROM share_market_owner_blocks
             WHERE owner_user_id = ?1 AND lifted_at IS NULL
             ORDER BY created_at DESC",
        )
        .map_err(map_db("prepare Share Market block list"))?;
    let rows = statement
        .query_map(params![owner_user_id], |row| {
            Ok(OwnerBlockView {
                blocked_user_id: row.get(0)?,
                blocked_email: row.get(1)?,
                reason: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(map_db("query Share Market block list"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(map_db("read Share Market block list"))
}

async fn list_catalog(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ShareMarketCatalog>, AppError> {
    let viewer = crate::api::resolve_router_session(&state, &headers).await?;
    let active_subdomains = state.proxy.active_subdomains().await;
    Ok(Json(
        state
            .store
            .share_market_catalog(viewer.as_ref(), &active_subdomains)
            .await?,
    ))
}

async fn list_owned_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<OwnedShareView>>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(state.store.share_market_owned_shares(&session).await?))
}

fn ensure_payment_profile_tx(tx: &Transaction<'_>, owner_user_id: &str) -> Result<(), AppError> {
    let methods_json: Option<String> = tx
        .query_row(
            "SELECT methods_json FROM account_payment_profiles WHERE user_id = ?1",
            params![owner_user_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_db("read payment profile for paid seat"))?;
    let has_methods = methods_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<Vec<PaymentMethod>>(value).ok())
        .is_some_and(|methods| !methods.is_empty());
    if !has_methods {
        return Err(AppError::Conflict(
            "configure Account payment details before adding a paid Share seat".into(),
        ));
    }
    Ok(())
}

fn insert_seat_tx(
    tx: &Transaction<'_>,
    listing_id: &str,
    position: i64,
    seat: &NormalizedSeat,
    now: &str,
) -> Result<String, AppError> {
    let id = Uuid::new_v4().to_string();
    let token_period_json = serde_json::to_string(&seat.token_period)
        .map_err(|error| AppError::Internal(format!("encode seat token period failed: {error}")))?;
    tx.execute(
        "INSERT INTO share_market_seats (
            id, listing_id, position, status, parallel_limit, token_limit,
            token_period_json, price_minor, currency, period_unit, period_count,
            offer_revision, current_subscription_id, created_at, updated_at
         ) VALUES (?1, ?2, ?3, 'available', ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, NULL, ?11, ?11)",
        params![
            id,
            listing_id,
            position,
            seat.parallel_limit.map(i64::from),
            seat.token_limit.and_then(|value| i64::try_from(value).ok()),
            token_period_json,
            seat.price_minor,
            seat.currency,
            seat.period_unit,
            seat.period_count.map(i64::from),
            now,
        ],
    )
    .map_err(map_db("insert Share Market seat"))?;
    Ok(id)
}

fn close_reclaimable_stale_listings_tx(
    tx: &Transaction<'_>,
    share_id: &str,
    current_owner_email: &str,
    now: &str,
) -> Result<(), AppError> {
    let active_subscriptions: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM share_market_subscriptions sub
             JOIN share_market_listings listing ON listing.id = sub.listing_id
             WHERE listing.share_id = ?1
               AND lower(listing.owner_email) != lower(?2)
               AND sub.status NOT IN ('released', 'grant_failed')",
            params![share_id, current_owner_email],
            |row| row.get(0),
        )
        .map_err(map_db("count stale Share listing subscriptions"))?;
    if active_subscriptions > 0 {
        return Err(AppError::Conflict(
            "the previous Share owner still has seats being reclaimed".into(),
        ));
    }
    tx.execute(
        "UPDATE share_market_listings
         SET status = 'closed', updated_at = ?3
         WHERE share_id = ?1 AND lower(owner_email) != lower(?2) AND status = 'active'",
        params![share_id, current_owner_email, now],
    )
    .map_err(map_db("close stale Share listings"))?;
    tx.execute(
        "UPDATE share_market_seats
         SET status = 'disabled', updated_at = ?3
         WHERE listing_id IN (
             SELECT id FROM share_market_listings
             WHERE share_id = ?1 AND lower(owner_email) != lower(?2) AND status = 'closed'
         ) AND status = 'available'",
        params![share_id, current_owner_email, now],
    )
    .map_err(map_db("disable stale Share listing seats"))?;
    Ok(())
}

impl AppStore {
    pub async fn share_market_create_listing(
        &self,
        session: &AuthSession,
        input: CreateListingRequest,
    ) -> Result<String, AppError> {
        if input.seats.is_empty() || input.seats.len() > MAX_SEATS_PER_LISTING {
            return Err(AppError::BadRequest(format!(
                "a listing requires 1-{MAX_SEATS_PER_LISTING} seats"
            )));
        }
        let seats = input
            .seats
            .into_iter()
            .map(normalize_seat)
            .collect::<Result<Vec<_>, _>>()?;
        let share_id = input.share_id.trim();
        if share_id.is_empty() {
            return Err(AppError::BadRequest("shareId is required".into()));
        }
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin Share Market listing"))?;
        let share: Option<(String, String, String)> = tx
            .query_row(
                "SELECT owner_email, share_status, COALESCE(supported_user_token_periods_json, '[]')
                 FROM shares WHERE share_id = ?1",
                params![share_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(map_db("read listing Share"))?;
        let Some((owner_email, share_status, periods_json)) = share else {
            return Err(AppError::NotFound("Share not found".into()));
        };
        if !owner_email.eq_ignore_ascii_case(&session.email) {
            return Err(AppError::Forbidden(
                "only the Share owner can list it".into(),
            ));
        }
        if share_status != "active" {
            return Err(AppError::Conflict(
                "Share must be active before it can be listed".into(),
            ));
        }
        let supported: Vec<ShareTokenPeriod> = serde_json::from_str(&periods_json)
            .unwrap_or_else(|_| vec![ShareTokenPeriod::Lifetime]);
        if seats
            .iter()
            .any(|seat| !supported.contains(&seat.token_period))
        {
            return Err(AppError::BadRequest(
                "a seat uses a token period unsupported by this Server".into(),
            ));
        }
        if seats.iter().any(|seat| !seat.is_free()) {
            ensure_payment_profile_tx(&tx, &session.user_id)?;
        }
        close_reclaimable_stale_listings_tx(&tx, share_id, &session.email, &now)?;
        let active_listing_exists = tx
            .query_row(
                "SELECT 1 FROM share_market_listings
                 WHERE share_id = ?1 AND status = 'active' LIMIT 1",
                params![share_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(map_db("check active Share listing"))?
            .is_some();
        if active_listing_exists {
            return Err(AppError::Conflict(
                "Share is already listed in Share Market".into(),
            ));
        }
        let active_subscription_exists = tx
            .query_row(
                "SELECT 1 FROM share_market_subscriptions
                 WHERE share_id = ?1
                   AND status NOT IN ('released', 'grant_failed')
                 LIMIT 1",
                params![share_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(map_db("check active Share Market subscriptions"))?
            .is_some();
        if active_subscription_exists {
            return Err(AppError::Conflict(
                "Share still has active Share Market rentals; wait until they end before relisting"
                    .into(),
            ));
        }
        let listing_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO share_market_listings (
                id, share_id, owner_user_id, owner_email, status, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?5)",
            params![listing_id, share_id, session.user_id, session.email, now],
        )
        .map_err(|error| {
            if error.to_string().contains("UNIQUE constraint failed") {
                AppError::Conflict("Share is already listed in Share Market".into())
            } else {
                AppError::Internal(format!("insert Share Market listing failed: {error}"))
            }
        })?;
        for (position, seat) in seats.iter().enumerate() {
            insert_seat_tx(&tx, &listing_id, position as i64 + 1, seat, &now)?;
        }
        event_tx(
            &tx,
            Some(&listing_id),
            None,
            None,
            Some(session),
            "listing_created",
            serde_json::json!({ "shareId": share_id, "seatCount": seats.len() }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit Share Market listing"))?;
        Ok(listing_id)
    }

    pub async fn share_market_add_seat(
        &self,
        session: &AuthSession,
        listing_id: &str,
        input: SeatInput,
    ) -> Result<String, AppError> {
        let seat = normalize_seat(input)?;
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().map_err(map_db("begin add Share seat"))?;
        let owner: Option<(String, String, String, String, String, String)> = tx
            .query_row(
                "SELECT listing.owner_user_id, listing.owner_email, s.owner_email,
                        s.share_status,
                        COALESCE(s.supported_user_token_periods_json, '[]'), listing.share_id
                 FROM share_market_listings listing
                 JOIN shares s ON s.share_id = listing.share_id
                 WHERE listing.id = ?1",
                params![listing_id],
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
            .map_err(map_db("read Share listing owner"))?;
        let Some((
            owner_user_id,
            listing_owner_email,
            share_owner_email,
            share_status,
            periods_json,
            share_id,
        )) = owner
        else {
            return Err(AppError::NotFound("listing not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only listing owner can add seats".into(),
            ));
        }
        if share_status != "active" || !listing_owner_email.eq_ignore_ascii_case(&share_owner_email)
        {
            return Err(AppError::Conflict(
                "listing Share is no longer active or owned by this account".into(),
            ));
        }
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM share_market_seats
                 WHERE listing_id = ?1 AND status != 'deleted'",
                params![listing_id],
                |row| row.get(0),
            )
            .map_err(map_db("count Share seats"))?;
        if count >= MAX_SEATS_PER_LISTING as i64 {
            return Err(AppError::Conflict("listing seat limit reached".into()));
        }
        let supported: Vec<ShareTokenPeriod> = serde_json::from_str(&periods_json)
            .unwrap_or_else(|_| vec![ShareTokenPeriod::Lifetime]);
        if !supported.contains(&seat.token_period) {
            return Err(AppError::BadRequest(
                "token period is unsupported by this Server".into(),
            ));
        }
        if !seat.is_free() {
            ensure_payment_profile_tx(&tx, &session.user_id)?;
        }
        close_reclaimable_stale_listings_tx(&tx, &share_id, &session.email, &now)?;
        let position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM share_market_seats WHERE listing_id = ?1",
                params![listing_id],
                |row| row.get(0),
            )
            .map_err(map_db("choose Share seat position"))?;
        let seat_id = insert_seat_tx(&tx, listing_id, position, &seat, &now)?;
        tx.execute(
            "UPDATE share_market_listings SET status = 'active', updated_at = ?2 WHERE id = ?1",
            params![listing_id, now],
        )
        .map_err(map_db("reopen Share listing"))?;
        event_tx(
            &tx,
            Some(listing_id),
            Some(&seat_id),
            None,
            Some(session),
            "seat_added",
            serde_json::json!({ "position": position }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit add Share seat"))?;
        Ok(seat_id)
    }

    pub async fn share_market_update_seat(
        &self,
        session: &AuthSession,
        seat_id: &str,
        input: UpdateSeatRequest,
    ) -> Result<(), AppError> {
        let seat = normalize_seat(input.seat)?;
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin update Share seat"))?;
        let row: Option<(String, String, i64, String, String, String)> = tx
            .query_row(
                "SELECT listing.owner_user_id, seat.status, seat.offer_revision,
                        listing.owner_email, s.owner_email,
                        COALESCE(s.supported_user_token_periods_json, '[]')
                 FROM share_market_seats seat
                 JOIN share_market_listings listing ON listing.id = seat.listing_id
                 JOIN shares s ON s.share_id = listing.share_id
                 WHERE seat.id = ?1 AND s.share_status = 'active'",
                params![seat_id],
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
            .map_err(map_db("read Share seat"))?;
        let Some((
            owner_user_id,
            status,
            offer_revision,
            listing_owner_email,
            share_owner_email,
            periods_json,
        )) = row
        else {
            return Err(AppError::NotFound("seat not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only listing owner can edit seat".into(),
            ));
        }
        if !listing_owner_email.eq_ignore_ascii_case(&share_owner_email) {
            return Err(AppError::Conflict(
                "listing Share is no longer owned by this account".into(),
            ));
        }
        if status != SEAT_AVAILABLE {
            return Err(AppError::Conflict(
                "an occupied or pending seat must be reclaimed before editing".into(),
            ));
        }
        if offer_revision != input.offer_revision {
            return Err(AppError::Conflict(
                "seat offer changed; reload and retry".into(),
            ));
        }
        let supported: Vec<ShareTokenPeriod> = serde_json::from_str(&periods_json)
            .unwrap_or_else(|_| vec![ShareTokenPeriod::Lifetime]);
        if !supported.contains(&seat.token_period) {
            return Err(AppError::BadRequest(
                "token period is unsupported by this Server".into(),
            ));
        }
        if !seat.is_free() {
            ensure_payment_profile_tx(&tx, &session.user_id)?;
        }
        let token_period_json = serde_json::to_string(&seat.token_period)
            .map_err(|error| AppError::Internal(format!("encode token period failed: {error}")))?;
        tx.execute(
            "UPDATE share_market_seats
             SET parallel_limit = ?2, token_limit = ?3, token_period_json = ?4,
                 price_minor = ?5, currency = ?6, period_unit = ?7, period_count = ?8,
                 offer_revision = offer_revision + 1, updated_at = ?9
             WHERE id = ?1 AND status = 'available' AND offer_revision = ?10",
            params![
                seat_id,
                seat.parallel_limit.map(i64::from),
                seat.token_limit.and_then(|value| i64::try_from(value).ok()),
                token_period_json,
                seat.price_minor,
                seat.currency,
                seat.period_unit,
                seat.period_count.map(i64::from),
                now,
                input.offer_revision,
            ],
        )
        .map_err(map_db("update Share seat"))?;
        event_tx(
            &tx,
            None,
            Some(seat_id),
            None,
            Some(session),
            "seat_updated",
            serde_json::json!({ "previousOfferRevision": input.offer_revision }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit update Share seat"))?;
        Ok(())
    }

    pub async fn share_market_delete_seat(
        &self,
        session: &AuthSession,
        seat_id: &str,
    ) -> Result<(), AppError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin delete Share seat"))?;
        let row: Option<(String, String, String)> = tx
            .query_row(
                "SELECT listing.owner_user_id, seat.status, seat.listing_id
                 FROM share_market_seats seat
                 JOIN share_market_listings listing ON listing.id = seat.listing_id
                 WHERE seat.id = ?1",
                params![seat_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(map_db("read Share seat for delete"))?;
        let Some((owner_user_id, status, listing_id)) = row else {
            return Err(AppError::NotFound("seat not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only listing owner can delete seat".into(),
            ));
        }
        if status != SEAT_AVAILABLE && status != SEAT_DISABLED {
            return Err(AppError::Conflict(
                "reclaim the occupied seat before deleting it".into(),
            ));
        }
        tx.execute(
            "UPDATE share_market_seats
             SET status = ?2, current_subscription_id = NULL, updated_at = ?3
             WHERE id = ?1",
            params![seat_id, SEAT_DELETED, now],
        )
        .map_err(map_db("delete Share seat"))?;
        event_tx(
            &tx,
            Some(&listing_id),
            Some(seat_id),
            None,
            Some(session),
            "seat_deleted",
            serde_json::json!({}),
            &now,
        )?;
        tx.commit().map_err(map_db("commit delete Share seat"))?;
        Ok(())
    }

    pub async fn share_market_close_listing(
        &self,
        session: &AuthSession,
        listing_id: &str,
    ) -> Result<(), AppError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin close Share listing"))?;
        let owner_user_id: Option<String> = tx
            .query_row(
                "SELECT owner_user_id FROM share_market_listings WHERE id = ?1",
                params![listing_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db("read Share listing"))?;
        let Some(owner_user_id) = owner_user_id else {
            return Err(AppError::NotFound("listing not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only listing owner can close listing".into(),
            ));
        }
        tx.execute(
            "UPDATE share_market_listings SET status = 'closed', updated_at = ?2 WHERE id = ?1",
            params![listing_id, now],
        )
        .map_err(map_db("close Share listing"))?;
        tx.execute(
            "UPDATE share_market_seats SET status = 'disabled', updated_at = ?2
             WHERE listing_id = ?1 AND status = 'available'",
            params![listing_id, now],
        )
        .map_err(map_db("disable open Share seats"))?;
        event_tx(
            &tx,
            Some(listing_id),
            None,
            None,
            Some(session),
            "listing_closed",
            serde_json::json!({}),
            &now,
        )?;
        tx.commit().map_err(map_db("commit close Share listing"))?;
        Ok(())
    }
}

async fn create_listing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CreateListingRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    let id = state
        .store
        .share_market_create_listing(&session, input)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "listingId": id })))
}

async fn add_seat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(listing_id): Path<String>,
    Json(input): Json<SeatInput>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    let id = state
        .store
        .share_market_add_seat(&session, &listing_id, input)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "seatId": id })))
}

async fn update_seat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(seat_id): Path<String>,
    Json(input): Json<UpdateSeatRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_update_seat(&session, &seat_id, input)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_seat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(seat_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_delete_seat(&session, &seat_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn close_listing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(listing_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_close_listing(&session, &listing_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[allow(clippy::too_many_arguments)]
fn enqueue_control_operation_tx(
    tx: &Transaction<'_>,
    share_id: &str,
    subscription_id: &str,
    entitlement_id: &str,
    action: &str,
    email: &str,
    policy: Option<&ShareUserPolicy>,
    now: &str,
) -> Result<String, AppError> {
    if tx
        .query_row(
            "SELECT 1 FROM share_control_operations
             WHERE subscription_id = ?1 AND action = ?2 AND status IN ('pending', 'dispatched')
             LIMIT 1",
            params![subscription_id, action],
            |_| Ok(()),
        )
        .optional()
        .map_err(map_db("check pending Share control operation"))?
        .is_some()
    {
        return tx
            .query_row(
                "SELECT id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = ?2 AND status IN ('pending', 'dispatched')
                 ORDER BY share_sequence LIMIT 1",
                params![subscription_id, action],
                |row| row.get(0),
            )
            .map_err(map_db("read pending Share control operation"));
    }
    let sequence: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(share_sequence), 0) + 1
             FROM share_control_operations WHERE share_id = ?1",
            params![share_id],
            |row| row.get(0),
        )
        .map_err(map_db("allocate Share control sequence"))?;
    let id = Uuid::new_v4().to_string();
    let policy_json = policy
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| {
            AppError::Internal(format!("encode Share grant policy failed: {error}"))
        })?;
    tx.execute(
        "INSERT INTO share_control_operations (
            id, share_id, share_sequence, entitlement_id, subscription_id,
            action, email, policy_json, status, edit_id, attempts, last_error,
            created_at, updated_at, applied_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', NULL, 0, NULL, ?9, ?9, NULL)",
        params![
            id,
            share_id,
            sequence,
            entitlement_id,
            subscription_id,
            action,
            email,
            policy_json,
            now,
        ],
    )
    .map_err(map_db("enqueue Share control operation"))?;
    Ok(id)
}

fn block_owner_user_tx(
    tx: &Transaction<'_>,
    owner_user_id: &str,
    blocked_user_id: &str,
    blocked_email: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    if owner_user_id == blocked_user_id {
        return Err(AppError::BadRequest("cannot block your own account".into()));
    }
    tx.execute(
        "INSERT INTO share_market_owner_blocks (
            owner_user_id, blocked_user_id, blocked_email, reason, created_at, lifted_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, NULL)
         ON CONFLICT(owner_user_id, blocked_user_id) DO UPDATE SET
            blocked_email = excluded.blocked_email,
            reason = excluded.reason,
            created_at = excluded.created_at,
            lifted_at = NULL",
        params![owner_user_id, blocked_user_id, blocked_email, reason, now],
    )
    .map_err(map_db("block Share Market renter"))?;
    Ok(())
}

impl AppStore {
    pub async fn share_market_rent_seat(
        &self,
        session: &AuthSession,
        seat_id: &str,
        input: RentSeatRequest,
    ) -> Result<String, AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin rent Share seat"))?;
        #[allow(clippy::type_complexity)]
        let row: Option<(
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            Option<i64>,
            Option<i64>,
            String,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<i64>,
            String,
        )> = tx
            .query_row(
                "SELECT seat.listing_id, listing.share_id, listing.owner_user_id,
                        listing.owner_email, listing.status, seat.status, seat.offer_revision,
                        seat.parallel_limit, seat.token_limit, seat.token_period_json,
                        seat.price_minor, seat.currency, seat.period_unit, seat.period_count,
                        COALESCE(s.user_grants_json, '{}')
                 FROM share_market_seats seat
                 JOIN share_market_listings listing ON listing.id = seat.listing_id
                 JOIN shares s ON s.share_id = listing.share_id
                 WHERE seat.id = ?1 AND s.share_status = 'active'
                   AND lower(s.owner_email) = lower(listing.owner_email)",
                params![seat_id],
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
                        row.get(14)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read rentable Share seat"))?;
        let Some((
            listing_id,
            share_id,
            owner_user_id,
            owner_email,
            listing_status,
            seat_status,
            offer_revision,
            parallel_limit,
            token_limit,
            token_period_json,
            price_minor,
            currency,
            period_unit,
            period_count,
            grants_json,
        )) = row
        else {
            return Err(AppError::Conflict(
                "seat or its active Share is unavailable".into(),
            ));
        };
        if listing_status != "active" || seat_status != SEAT_AVAILABLE {
            return Err(AppError::Conflict("seat is no longer available".into()));
        }
        if offer_revision != input.offer_revision {
            return Err(AppError::Conflict(
                "seat offer changed; reload and retry".into(),
            ));
        }
        if owner_user_id == session.user_id || owner_email.eq_ignore_ascii_case(&session.email) {
            return Err(AppError::BadRequest(
                "Share owner cannot rent their own seat".into(),
            ));
        }
        let blocked = tx
            .query_row(
                "SELECT 1 FROM share_market_owner_blocks
                 WHERE owner_user_id = ?1 AND blocked_user_id = ?2 AND lifted_at IS NULL",
                params![owner_user_id, session.user_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(map_db("check Share Market block"))?
            .is_some();
        if blocked {
            return Err(AppError::Forbidden(
                "Share owner has blocked this account from their listings".into(),
            ));
        }
        let already_renting = tx
            .query_row(
                "SELECT 1 FROM share_market_subscriptions
                 WHERE renter_user_id = ?1 AND share_id = ?2
                   AND status NOT IN ('released', 'grant_failed') LIMIT 1",
                params![session.user_id, share_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(map_db("check existing Share rental"))?
            .is_some();
        if already_renting {
            return Err(AppError::Conflict(
                "one account can rent only one seat on the same Share".into(),
            ));
        }
        let grants: BTreeMap<String, ShareUserGrant> =
            serde_json::from_str(&grants_json).unwrap_or_default();
        if grants
            .get(&session.email.to_ascii_lowercase())
            .is_some_and(|grant| grant.active)
        {
            return Err(AppError::Conflict(
                "this account already has direct Share access".into(),
            ));
        }
        if price_minor.is_some() {
            ensure_payment_profile_tx(&tx, &owner_user_id)?;
        }
        let token_period: ShareTokenPeriod = serde_json::from_str(&token_period_json)
            .map_err(|_| AppError::Internal("stored seat token period is invalid".into()))?;
        let policy = ShareUserPolicy {
            parallel_limit: parallel_limit.and_then(|value| u32::try_from(value).ok()),
            token_limit: token_limit.and_then(|value| u64::try_from(value).ok()),
            token_period,
            token_period_anchor_at_ms: token_period_anchor_at_ms(token_period, now_dt),
            expires_at: None,
        };
        let subscription_id = Uuid::new_v4().to_string();
        let entitlement_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO share_market_subscriptions (
                id, seat_id, listing_id, share_id, entitlement_id,
                owner_user_id, owner_email, renter_user_id, renter_email, status,
                parallel_limit, token_limit, token_period_json, price_minor, currency,
                period_unit, period_count, offer_revision, trial_ends_at,
                current_period_end, payment_deadline, release_reason,
                created_at, updated_at, released_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'grant_pending',
                       ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                       NULL, NULL, NULL, NULL, ?18, ?18, NULL)",
            params![
                subscription_id,
                seat_id,
                listing_id,
                share_id,
                entitlement_id,
                owner_user_id,
                owner_email,
                session.user_id,
                session.email.to_ascii_lowercase(),
                parallel_limit,
                token_limit,
                token_period_json,
                price_minor,
                currency,
                period_unit,
                period_count,
                offer_revision,
                now,
            ],
        )
        .map_err(|error| {
            if error.to_string().contains("UNIQUE constraint failed") {
                AppError::Conflict("seat was rented by another account".into())
            } else {
                AppError::Internal(format!("create Share subscription failed: {error}"))
            }
        })?;
        let changed = tx
            .execute(
                "UPDATE share_market_seats
                 SET status = 'reserved', current_subscription_id = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'available' AND offer_revision = ?4",
                params![seat_id, subscription_id, now, offer_revision],
            )
            .map_err(map_db("reserve Share seat"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "seat was rented by another account".into(),
            ));
        }
        enqueue_control_operation_tx(
            &tx,
            &share_id,
            &subscription_id,
            &entitlement_id,
            "upsert",
            &session.email.to_ascii_lowercase(),
            Some(&policy),
            &now,
        )?;
        event_tx(
            &tx,
            Some(&listing_id),
            Some(seat_id),
            Some(&subscription_id),
            Some(session),
            "seat_rented",
            serde_json::json!({ "free": price_minor.is_none() }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit Share seat rental"))?;
        Ok(subscription_id)
    }

    async fn share_market_request_release(
        &self,
        session: &AuthSession,
        subscription_id: &str,
        owner_override: bool,
        block_user: bool,
        reason: Option<&str>,
    ) -> Result<(), AppError> {
        if reason.is_some_and(|value| value.chars().count() > 500) {
            return Err(AppError::BadRequest("release reason is too long".into()));
        }
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin Share seat release"))?;
        #[allow(clippy::type_complexity)]
        let row: Option<(
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        )> = tx
            .query_row(
                "SELECT share_id, seat_id, listing_id, entitlement_id, owner_user_id,
                        renter_user_id, renter_email, status
                 FROM share_market_subscriptions
                 WHERE id = ?1 AND status NOT IN ('released', 'grant_failed')",
                params![subscription_id],
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
            .optional()
            .map_err(map_db("read Share subscription for release"))?;
        let Some((
            share_id,
            seat_id,
            listing_id,
            entitlement_id,
            owner_user_id,
            renter_user_id,
            renter_email,
            _subscription_status,
        )) = row
        else {
            return Err(AppError::NotFound("active subscription not found".into()));
        };
        let authorized = if owner_override {
            owner_user_id == session.user_id
        } else {
            renter_user_id == session.user_id
        };
        if !authorized {
            return Err(AppError::Forbidden(
                "subscription does not belong to this account".into(),
            ));
        }
        let reason = reason.unwrap_or(if owner_override {
            "owner_force_revoke"
        } else {
            "renter_release"
        });
        // Retire stuck pending/dispatched grant edits so revoke can dispatch, or so
        // never-dispatched grants can finish without waiting on an offline Client.
        let retired = retire_unconfirmed_grant_tx(&tx, subscription_id, reason, &now)?;
        let grants_json: Option<String> = tx
            .query_row(
                "SELECT user_grants_json FROM shares WHERE share_id = ?1",
                params![share_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(map_db("read Share grants during release"))?
            .flatten();
        let has_entitlement = active_entitlement(grants_json.as_deref(), &entitlement_id);
        let grant_never_reached_client = !entitlement_was_activated_tx(&tx, subscription_id)?
            && !has_entitlement
            && !retired.had_dispatched;
        if grant_never_reached_client {
            finish_release_tx(&tx, subscription_id, &seat_id, &listing_id, reason, &now)?;
        } else {
            request_revoke_tx(
                &tx,
                subscription_id,
                &share_id,
                &seat_id,
                &entitlement_id,
                &renter_email,
                reason,
                &now,
            )?;
        }
        if block_user {
            block_owner_user_tx(
                &tx,
                &owner_user_id,
                &renter_user_id,
                &renter_email,
                reason,
                &now,
            )?;
        }
        event_tx(
            &tx,
            Some(&listing_id),
            Some(&seat_id),
            Some(subscription_id),
            Some(session),
            if owner_override {
                "owner_revoke_requested"
            } else {
                "renter_release_requested"
            },
            serde_json::json!({ "blocked": block_user, "reason": reason }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit Share seat release"))?;
        Ok(())
    }

    pub async fn share_market_declare_paid(
        &self,
        session: &AuthSession,
        subscription_id: &str,
        input: DeclarePaidRequest,
    ) -> Result<(), AppError> {
        if !input.confirmed {
            return Err(AppError::BadRequest(
                "payment declaration requires explicit confirmation".into(),
            ));
        }
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin payment declaration"))?;
        let resolved_invoice: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT invoice.status, declaration.renter_user_id
                 FROM share_market_invoices invoice
                 LEFT JOIN share_market_payment_declarations declaration
                   ON declaration.invoice_id = invoice.id
                 WHERE invoice.id = ?1 AND invoice.subscription_id = ?2",
                params![input.invoice_id, subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_db("read resolved Share invoice"))?;
        if let Some((status, declared_by)) = resolved_invoice {
            if status == "declared" && declared_by.as_deref() == Some(session.user_id.as_str()) {
                tx.commit()
                    .map_err(map_db("commit idempotent Share payment declaration"))?;
                return Ok(());
            }
            if status != "open" {
                return Err(AppError::Conflict("invoice was already resolved".into()));
            }
        }
        #[allow(clippy::type_complexity)]
        let row: Option<(
            String,
            String,
            String,
            i64,
            i64,
            String,
            String,
            i64,
            String,
            Option<String>,
            String,
            String,
            String,
        )> = tx
            .query_row(
                "SELECT sub.renter_user_id, sub.owner_user_id, sub.status,
                        invoice.amount_minor, invoice.offer_revision,
                        invoice.period_unit, invoice.deadline_at, invoice.period_count,
                        profile.updated_at, sub.current_period_end, sub.owner_email,
                        COALESCE(share.owner_email, ''),
                        COALESCE(share.share_status, 'missing')
                 FROM share_market_subscriptions sub
                 JOIN share_market_invoices invoice ON invoice.subscription_id = sub.id
                 JOIN account_payment_profiles profile ON profile.user_id = sub.owner_user_id
                 LEFT JOIN shares share ON share.share_id = sub.share_id
                 WHERE sub.id = ?1 AND invoice.id = ?2 AND invoice.status = 'open'",
                params![subscription_id, input.invoice_id],
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
            .map_err(map_db("read payable Share invoice"))?;
        let Some((
            renter_user_id,
            owner_user_id,
            status,
            amount_minor,
            offer_revision,
            period_unit,
            deadline_at,
            period_count,
            profile_updated_at,
            current_period_end,
            subscription_owner_email,
            current_share_owner_email,
            share_status,
        )) = row
        else {
            return Err(AppError::NotFound("open invoice not found".into()));
        };
        if renter_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "invoice does not belong to this account".into(),
            ));
        }
        if !matches!(status.as_str(), SUB_TRIAL_PAYMENT_DUE | SUB_RENEWAL_DUE) {
            return Err(AppError::Conflict(
                "subscription is not awaiting payment".into(),
            ));
        }
        if share_status != "active"
            || !current_share_owner_email.eq_ignore_ascii_case(&subscription_owner_email)
        {
            return Err(AppError::Conflict(
                "Share is no longer active or owned by this payment recipient".into(),
            ));
        }
        ensure_payment_profile_tx(&tx, &owner_user_id)?;
        if offer_revision != input.offer_revision
            || amount_minor != input.amount_minor_confirmed
            || profile_updated_at != input.payment_profile_updated_at
        {
            return Err(AppError::Conflict(
                "invoice or payment details changed; reload before declaring payment".into(),
            ));
        }
        if parse_time(&deadline_at)? <= now_dt {
            return Err(AppError::Gone(
                "payment declaration deadline has passed".into(),
            ));
        }
        let period_count = u32::try_from(period_count)
            .map_err(|_| AppError::Internal("stored billing period count is invalid".into()))?;
        let period_start = if status == SUB_TRIAL_PAYMENT_DUE {
            parse_time(&deadline_at)?
        } else {
            current_period_end
                .as_deref()
                .map(parse_time)
                .transpose()?
                .map(|value| value.max(now_dt))
                .unwrap_or(now_dt)
        };
        let period_end = add_billing_period(period_start, &period_unit, period_count)?;
        tx.execute(
            "INSERT INTO share_market_payment_declarations (
                id, invoice_id, subscription_id, renter_user_id, renter_email, declared_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                Uuid::new_v4().to_string(),
                input.invoice_id,
                subscription_id,
                session.user_id,
                session.email,
                now,
            ],
        )
        .map_err(map_db("record Share payment declaration"))?;
        tx.execute(
            "UPDATE share_market_invoices
             SET status = 'declared', declared_at = ?2 WHERE id = ?1 AND status = 'open'",
            params![input.invoice_id, now],
        )
        .map_err(map_db("declare Share invoice paid"))?;
        tx.execute(
            "UPDATE share_market_subscriptions
             SET status = 'active_paid', current_period_end = ?2,
                 payment_deadline = NULL, updated_at = ?3
             WHERE id = ?1",
            params![subscription_id, period_end.to_rfc3339(), now],
        )
        .map_err(map_db("activate paid Share subscription"))?;
        event_tx(
            &tx,
            None,
            None,
            Some(subscription_id),
            Some(session),
            "payment_declared",
            serde_json::json!({ "invoiceId": input.invoice_id, "amountMinor": amount_minor }),
            &now,
        )?;
        tx.commit()
            .map_err(map_db("commit Share payment declaration"))?;
        Ok(())
    }

    pub async fn share_market_lift_block(
        &self,
        session: &AuthSession,
        blocked_user_id: &str,
    ) -> Result<(), AppError> {
        let changed = self
            .conn
            .lock()
            .await
            .execute(
                "UPDATE share_market_owner_blocks SET lifted_at = ?3
                 WHERE owner_user_id = ?1 AND blocked_user_id = ?2 AND lifted_at IS NULL",
                params![session.user_id, blocked_user_id, Utc::now().to_rfc3339()],
            )
            .map_err(map_db("lift Share Market block"))?;
        if changed == 0 {
            return Err(AppError::NotFound("active block not found".into()));
        }
        Ok(())
    }
}

async fn rent_seat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(seat_id): Path<String>,
    Json(input): Json<RentSeatRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    let subscription_id = state
        .store
        .share_market_rent_seat(&session, &seat_id, input)
        .await?;
    run_once(&state).await?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "subscriptionId": subscription_id
    })))
}

async fn declare_paid(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(subscription_id): Path<String>,
    Json(input): Json<DeclarePaidRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_declare_paid(&session, &subscription_id, input)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn release_subscription(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(subscription_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_request_release(&session, &subscription_id, false, false, None)
        .await?;
    run_once(&state).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn force_revoke_subscription(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(subscription_id): Path<String>,
    Json(input): Json<ForceRevokeRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_request_release(
            &session,
            &subscription_id,
            true,
            input.block_user,
            input.reason.as_deref(),
        )
        .await?;
    run_once(&state).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn list_owner_blocks(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<OwnerBlockView>>, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok(Json(owner_blocks_for(&conn, &session.user_id)?))
}

async fn lift_owner_block(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_lift_block(&session, &user_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn active_entitlement(grants_json: Option<&str>, entitlement_id: &str) -> bool {
    grants_json
        .and_then(|value| serde_json::from_str::<BTreeMap<String, ShareUserGrant>>(value).ok())
        .is_some_and(|grants| {
            grants.values().any(|grant| {
                grant.active
                    && grant.manager == ShareGrantManager::RouterShareMarket
                    && grant.entitlement_id.as_deref() == Some(entitlement_id)
            })
        })
}

fn confirm_control_effect_tx(
    tx: &Transaction<'_>,
    subscription_id: &str,
    action: &str,
    now: &str,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE share_edit_requests
         SET status = 'applied', updated_at = ?3, applied_at = ?3,
             error_message = NULL
         WHERE status = 'pending' AND id IN (
             SELECT edit_id FROM share_control_operations
             WHERE subscription_id = ?1 AND action = ?2
               AND status IN ('pending', 'dispatched')
         )",
        params![subscription_id, action, now],
    )
    .map_err(map_db("confirm observed Share control edit"))?;
    tx.execute(
        "UPDATE share_control_operations
         SET status = 'applied', updated_at = ?3, applied_at = ?3,
             last_error = NULL
         WHERE subscription_id = ?1 AND action = ?2
           AND status IN ('pending', 'dispatched')",
        params![subscription_id, action, now],
    )
    .map_err(map_db("confirm observed Share control operation"))?;
    Ok(())
}

fn cancel_pending_grant_tx(
    tx: &Transaction<'_>,
    subscription_id: &str,
    reason: &str,
    now: &str,
) -> Result<bool, AppError> {
    let message = format!("grant canceled before dispatch: {reason}");
    let changed = tx
        .execute(
            "UPDATE share_control_operations
             SET status = 'rejected', updated_at = ?3, last_error = ?2
             WHERE subscription_id = ?1 AND action = 'upsert' AND status = 'pending'",
            params![subscription_id, message, now],
        )
        .map_err(map_db("cancel pending Share grant"))?;
    Ok(changed > 0)
}

#[derive(Debug, Clone, Copy, Default)]
struct RetireUnconfirmedGrant {
    had_dispatched: bool,
}

/// Retires unconfirmed upsert control work (pending or dispatched) so a later
/// revoke can dispatch. Callers that know the grant never reached the Client can
/// finish without a revoke when `had_dispatched` is false.
fn retire_unconfirmed_grant_tx(
    tx: &Transaction<'_>,
    subscription_id: &str,
    reason: &str,
    now: &str,
) -> Result<RetireUnconfirmedGrant, AppError> {
    let message = format!("grant canceled before confirmation: {reason}");
    let had_dispatched = tx
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM share_control_operations
                WHERE subscription_id = ?1 AND action = 'upsert' AND status = 'dispatched'
             )",
            params![subscription_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("check dispatched Share grant"))?
        != 0;
    let edit_ids: Vec<String> = {
        let mut statement = tx
            .prepare(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'
                   AND status IN ('pending', 'dispatched')
                   AND edit_id IS NOT NULL",
            )
            .map_err(map_db("prepare unconfirmed Share grant edits"))?;
        statement
            .query_map(params![subscription_id], |row| row.get(0))
            .map_err(map_db("query unconfirmed Share grant edits"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read unconfirmed Share grant edits"))?
    };
    for edit_id in edit_ids {
        tx.execute(
            "UPDATE share_edit_requests
             SET status = 'cancelled', retired_at = ?2, updated_at = ?2,
                 error_message = COALESCE(error_message, ?3)
             WHERE id = ?1 AND status = 'pending'",
            params![edit_id, now, message],
        )
        .map_err(map_db("retire unconfirmed Share grant edit"))?;
    }
    tx.execute(
            "UPDATE share_control_operations
             SET status = 'rejected', updated_at = ?3, last_error = ?2
             WHERE subscription_id = ?1 AND action = 'upsert'
               AND status IN ('pending', 'dispatched')",
            params![subscription_id, message, now],
        )
        .map_err(map_db("reject unconfirmed Share grant"))?;
    Ok(RetireUnconfirmedGrant { had_dispatched })
}

fn entitlement_was_activated_tx(
    tx: &Transaction<'_>,
    subscription_id: &str,
) -> Result<bool, AppError> {
    let exists: i64 = tx
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM share_market_events
                WHERE subscription_id = ?1 AND event_type = 'entitlement_activated'
             )",
            params![subscription_id],
            |row| row.get(0),
        )
        .map_err(map_db("read Share entitlement activation"))?;
    Ok(exists != 0)
}

fn can_confirm_absent_entitlement_tx(
    tx: &Transaction<'_>,
    subscription_id: &str,
) -> Result<bool, AppError> {
    let (entitlement_was_observed, revoke_was_applied): (i64, i64) = tx
        .query_row(
            "SELECT
                EXISTS (
                    SELECT 1 FROM share_market_events
                    WHERE subscription_id = ?1 AND event_type = 'entitlement_activated'
                ),
                EXISTS (
                    SELECT 1
                    FROM share_control_operations operation
                    LEFT JOIN share_edit_requests edit ON edit.id = operation.edit_id
                    WHERE operation.subscription_id = ?1 AND operation.action = 'revoke'
                      AND (operation.status = 'applied' OR edit.status = 'applied')
                )",
            params![subscription_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_db("confirm Share revoke ordering"))?;
    Ok(entitlement_was_observed != 0 || revoke_was_applied != 0)
}

fn recover_orphaned_control_edits_tx(tx: &Transaction<'_>, now: &str) -> Result<(), AppError> {
    let orphaned = {
        let mut statement = tx
            .prepare(
                "SELECT operation.id, operation.action
                 FROM share_control_operations operation
                 LEFT JOIN share_edit_requests edit ON edit.id = operation.edit_id
                 WHERE operation.status = 'dispatched'
                   AND (edit.id IS NULL OR edit.status = 'cancelled')",
            )
            .map_err(map_db("prepare orphaned Share control edits"))?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_db("query orphaned Share control edits"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read orphaned Share control edits"))?
    };
    for (operation_id, action) in orphaned {
        if action == "revoke" {
            tx.execute(
                "UPDATE share_control_operations
                 SET status = 'pending', edit_id = NULL,
                     attempts = CASE WHEN attempts > 0 THEN attempts - 1 ELSE 0 END,
                     last_error = 'Share control edit was retired before acknowledgement',
                     updated_at = ?2
                 WHERE id = ?1 AND status = 'dispatched'",
                params![operation_id, now],
            )
            .map_err(map_db("recover orphaned Share revoke"))?;
        } else {
            tx.execute(
                "UPDATE share_control_operations
                 SET status = 'rejected',
                     last_error = 'Share grant edit was retired before acknowledgement',
                     updated_at = ?2
                 WHERE id = ?1 AND status = 'dispatched'",
                params![operation_id, now],
            )
            .map_err(map_db("fence orphaned Share grant"))?;
        }
    }
    Ok(())
}

fn finish_release_tx(
    tx: &Transaction<'_>,
    subscription_id: &str,
    seat_id: &str,
    listing_id: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE share_market_subscriptions
         SET status = 'released', release_reason = COALESCE(release_reason, ?2),
             updated_at = ?3, released_at = ?3
         WHERE id = ?1 AND status != 'released'",
        params![subscription_id, reason, now],
    )
    .map_err(map_db("release Share subscription"))?;
    tx.execute(
        "UPDATE share_market_invoices
         SET status = 'canceled', canceled_at = ?2
         WHERE subscription_id = ?1 AND status = 'open'",
        params![subscription_id, now],
    )
    .map_err(map_db("cancel released Share invoices"))?;
    tx.execute(
        "UPDATE share_market_seats
         SET status = CASE
                WHEN (SELECT status FROM share_market_listings WHERE id = ?2) = 'active'
                    THEN 'available'
                ELSE 'disabled'
             END,
             current_subscription_id = NULL, updated_at = ?3
         WHERE id = ?1 AND current_subscription_id = ?4",
        params![seat_id, listing_id, now, subscription_id],
    )
    .map_err(map_db("release Share seat"))?;
    event_tx(
        tx,
        Some(listing_id),
        Some(seat_id),
        Some(subscription_id),
        None,
        "subscription_released",
        serde_json::json!({ "reason": reason }),
        now,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn request_revoke_tx(
    tx: &Transaction<'_>,
    subscription_id: &str,
    share_id: &str,
    seat_id: &str,
    entitlement_id: &str,
    renter_email: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE share_market_subscriptions
         SET status = 'revoke_pending', release_reason = ?2, updated_at = ?3
         WHERE id = ?1 AND status NOT IN ('released', 'grant_failed')",
        params![subscription_id, reason, now],
    )
    .map_err(map_db("request automatic Share revoke"))?;
    tx.execute(
        "UPDATE share_market_seats SET status = 'revoking', updated_at = ?2 WHERE id = ?1",
        params![seat_id, now],
    )
    .map_err(map_db("mark automatic Share seat revoke"))?;
    tx.execute(
        "UPDATE share_market_invoices
         SET status = 'canceled', canceled_at = ?2
         WHERE subscription_id = ?1 AND status = 'open'",
        params![subscription_id, now],
    )
    .map_err(map_db("cancel overdue Share invoice"))?;
    enqueue_control_operation_tx(
        tx,
        share_id,
        subscription_id,
        entitlement_id,
        "revoke",
        renter_email,
        None,
        now,
    )?;
    Ok(())
}

fn open_initial_invoice_tx(
    tx: &Transaction<'_>,
    record: &SubscriptionRecord,
    trial_ends_at: &str,
    now: &str,
) -> Result<(), AppError> {
    let amount = record
        .price_minor
        .ok_or_else(|| AppError::Internal("paid subscription amount is missing".into()))?;
    let currency = record
        .currency
        .as_deref()
        .ok_or_else(|| AppError::Internal("paid subscription currency is missing".into()))?;
    let unit = record
        .period_unit
        .as_deref()
        .ok_or_else(|| AppError::Internal("paid subscription period is missing".into()))?;
    let count = record
        .period_count
        .ok_or_else(|| AppError::Internal("paid subscription period count is missing".into()))?;
    tx.execute(
        "INSERT INTO share_market_invoices (
            id, subscription_id, sequence, amount_minor, currency, period_unit,
            period_count, offer_revision, status, due_at, deadline_at, opened_at,
            declared_at, canceled_at
         ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, 'open', ?8, ?8, ?9, NULL, NULL)",
        params![
            Uuid::new_v4().to_string(),
            record.id,
            amount,
            currency,
            unit,
            i64::from(count),
            record.offer_revision,
            trial_ends_at,
            now,
        ],
    )
    .map_err(map_db("open initial Share invoice"))?;
    Ok(())
}

fn open_renewal_invoice_tx(
    tx: &Transaction<'_>,
    record: &SubscriptionRecord,
    due_at: DateTime<Utc>,
    now: &str,
) -> Result<(), AppError> {
    let amount = record
        .price_minor
        .ok_or_else(|| AppError::Internal("paid subscription amount is missing".into()))?;
    let currency = record
        .currency
        .as_deref()
        .ok_or_else(|| AppError::Internal("paid subscription currency is missing".into()))?;
    let unit = record
        .period_unit
        .as_deref()
        .ok_or_else(|| AppError::Internal("paid subscription period is missing".into()))?;
    let count = record
        .period_count
        .ok_or_else(|| AppError::Internal("paid subscription period count is missing".into()))?;
    let sequence: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM share_market_invoices WHERE subscription_id = ?1",
            params![record.id],
            |row| row.get(0),
        )
        .map_err(map_db("allocate Share invoice sequence"))?;
    let deadline = due_at + Duration::hours(RENEWAL_GRACE_HOURS);
    tx.execute(
        "INSERT INTO share_market_invoices (
            id, subscription_id, sequence, amount_minor, currency, period_unit,
            period_count, offer_revision, status, due_at, deadline_at, opened_at,
            declared_at, canceled_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open', ?9, ?10, ?11, NULL, NULL)",
        params![
            Uuid::new_v4().to_string(),
            record.id,
            sequence,
            amount,
            currency,
            unit,
            i64::from(count),
            record.offer_revision,
            due_at.to_rfc3339(),
            deadline.to_rfc3339(),
            now,
        ],
    )
    .map_err(map_db("open renewal Share invoice"))?;
    tx.execute(
        "UPDATE share_market_subscriptions
         SET status = 'renewal_due', payment_deadline = ?2, updated_at = ?3 WHERE id = ?1",
        params![record.id, deadline.to_rfc3339(), now],
    )
    .map_err(map_db("mark Share renewal due"))?;
    Ok(())
}

impl AppStore {
    pub async fn share_market_reconcile_and_dispatch(
        &self,
        now_dt: DateTime<Utc>,
    ) -> Result<Vec<ShareEditAvailableEvent>, AppError> {
        let now = now_dt.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin Share Market reconciliation"))?;
        recover_orphaned_control_edits_tx(&tx, &now)?;

        let subscription_ids = {
            let mut statement = tx
                .prepare(
                    "SELECT id FROM share_market_subscriptions
                     WHERE status NOT IN ('released', 'grant_failed') ORDER BY created_at",
                )
                .map_err(map_db("prepare Share subscriptions for reconciliation"))?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_db("query Share subscriptions for reconciliation"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_db("read Share subscriptions for reconciliation"))?
        };
        for subscription_id in subscription_ids {
            let Some(record) = subscription_record(&tx, &subscription_id)? else {
                continue;
            };
            let share: Option<(String, String, Option<String>)> = tx
                .query_row(
                    "SELECT share_status, COALESCE(owner_email, ''), user_grants_json
                     FROM shares WHERE share_id = ?1",
                    params![record.share_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(map_db("read Share during market reconciliation"))?;
            let share_valid = share.as_ref().is_some_and(|(status, owner_email, _)| {
                status == "active" && owner_email.eq_ignore_ascii_case(&record.owner_email)
            });
            let has_entitlement = share.as_ref().is_some_and(|(_, _, grants)| {
                active_entitlement(grants.as_deref(), &record.entitlement_id)
            });

            if matches!(
                record.status.as_str(),
                SUB_REVOKE_PENDING | SUB_REVOKE_FAILED
            ) {
                if share.is_some()
                    && !has_entitlement
                    && can_confirm_absent_entitlement_tx(&tx, &record.id)?
                {
                    confirm_control_effect_tx(&tx, &record.id, "revoke", &now)?;
                    finish_release_tx(
                        &tx,
                        &record.id,
                        &record.seat_id,
                        &record.listing_id,
                        "entitlement_revoked",
                        &now,
                    )?;
                }
                continue;
            }

            if record.status == SUB_GRANT_PENDING {
                if has_entitlement {
                    confirm_control_effect_tx(&tx, &record.id, "upsert", &now)?;
                    if record.price_minor.is_none() {
                        tx.execute(
                            "UPDATE share_market_subscriptions
                             SET status = 'active_free', updated_at = ?2 WHERE id = ?1",
                            params![record.id, now],
                        )
                        .map_err(map_db("activate free Share subscription"))?;
                    } else {
                        let trial_end = now_dt + Duration::hours(TRIAL_HOURS);
                        let trial_end_text = trial_end.to_rfc3339();
                        open_initial_invoice_tx(&tx, &record, &trial_end_text, &now)?;
                        tx.execute(
                            "UPDATE share_market_subscriptions
                             SET status = 'trial_payment_due', trial_ends_at = ?2,
                                 payment_deadline = ?2, updated_at = ?3 WHERE id = ?1",
                            params![record.id, trial_end_text, now],
                        )
                        .map_err(map_db("activate Share trial"))?;
                    }
                    tx.execute(
                        "UPDATE share_market_seats SET status = 'occupied', updated_at = ?2
                         WHERE id = ?1 AND current_subscription_id = ?3",
                        params![record.seat_id, now, record.id],
                    )
                    .map_err(map_db("occupy Share seat"))?;
                    event_tx(
                        &tx,
                        Some(&record.listing_id),
                        Some(&record.seat_id),
                        Some(&record.id),
                        None,
                        "entitlement_activated",
                        serde_json::json!({ "free": record.price_minor.is_none() }),
                        &now,
                    )?;
                } else if !share_valid {
                    if cancel_pending_grant_tx(&tx, &record.id, "share_unavailable", &now)? {
                        finish_release_tx(
                            &tx,
                            &record.id,
                            &record.seat_id,
                            &record.listing_id,
                            "share_unavailable",
                            &now,
                        )?;
                    } else {
                        request_revoke_tx(
                            &tx,
                            &record.id,
                            &record.share_id,
                            &record.seat_id,
                            &record.entitlement_id,
                            &record.renter_email,
                            "share_unavailable",
                            &now,
                        )?;
                    }
                }
                continue;
            }

            if !has_entitlement {
                if share.is_none() {
                    request_revoke_tx(
                        &tx,
                        &record.id,
                        &record.share_id,
                        &record.seat_id,
                        &record.entitlement_id,
                        &record.renter_email,
                        "share_missing",
                        &now,
                    )?;
                } else {
                    finish_release_tx(
                        &tx,
                        &record.id,
                        &record.seat_id,
                        &record.listing_id,
                        "entitlement_missing",
                        &now,
                    )?;
                }
                continue;
            }
            if !share_valid {
                request_revoke_tx(
                    &tx,
                    &record.id,
                    &record.share_id,
                    &record.seat_id,
                    &record.entitlement_id,
                    &record.renter_email,
                    "share_unavailable",
                    &now,
                )?;
                continue;
            }

            if matches!(
                record.status.as_str(),
                SUB_TRIAL_PAYMENT_DUE | SUB_RENEWAL_DUE
            ) && record
                .payment_deadline
                .as_deref()
                .map(parse_time)
                .transpose()?
                .is_some_and(|deadline| deadline <= now_dt)
            {
                request_revoke_tx(
                    &tx,
                    &record.id,
                    &record.share_id,
                    &record.seat_id,
                    &record.entitlement_id,
                    &record.renter_email,
                    "payment_timeout",
                    &now,
                )?;
                continue;
            }
            if record.status == SUB_ACTIVE_PAID
                && record
                    .current_period_end
                    .as_deref()
                    .map(parse_time)
                    .transpose()?
                    .is_some_and(|end| end <= now_dt)
                && open_invoice(&tx, &record.id)?.is_none()
            {
                let due_at = record
                    .current_period_end
                    .as_deref()
                    .map(parse_time)
                    .transpose()?
                    .unwrap_or(now_dt);
                open_renewal_invoice_tx(&tx, &record, due_at, &now)?;
            }
        }

        let operation_ids = {
            let mut statement = tx
                .prepare(
                    "SELECT op.id
                     FROM share_control_operations op
                     WHERE op.status = 'pending' AND op.attempts < ?1
                       AND NOT EXISTS (
                           SELECT 1 FROM share_control_operations earlier
                           WHERE earlier.share_id = op.share_id
                             AND earlier.share_sequence < op.share_sequence
                             AND earlier.status IN ('pending', 'dispatched')
                       )
                       AND NOT EXISTS (
                           SELECT 1 FROM share_edit_requests edit
                           WHERE edit.share_id = op.share_id AND edit.status = 'pending'
                             AND edit.retired_at IS NULL
                       )
                     ORDER BY op.created_at, op.share_sequence",
                )
                .map_err(map_db("prepare Share control dispatch"))?;
            statement
                .query_map(params![MAX_CONTROL_ATTEMPTS], |row| row.get::<_, String>(0))
                .map_err(map_db("query Share control dispatch"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_db("read Share control dispatch"))?
        };
        let mut dispatched_shares = HashSet::new();
        let mut events = Vec::new();
        for operation_id in operation_ids {
            #[allow(clippy::type_complexity)]
            let operation: Option<(
                String,
                i64,
                String,
                String,
                String,
                Option<String>,
                String,
            )> = tx
                .query_row(
                    "SELECT share_id, share_sequence, entitlement_id, action, email,
                            policy_json, subscription_id
                     FROM share_control_operations WHERE id = ?1 AND status = 'pending'",
                    params![operation_id],
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
                .map_err(map_db("read Share control operation"))?;
            let Some((
                share_id,
                share_sequence,
                entitlement_id,
                action,
                email,
                policy_json,
                subscription_id,
            )) = operation
            else {
                continue;
            };
            if !dispatched_shares.insert(share_id.clone()) {
                continue;
            }
            let target: Option<(String, i64)> = tx
                .query_row(
                    "SELECT installation_id, config_revision FROM shares WHERE share_id = ?1",
                    params![share_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(map_db("read Share control target"))?;
            let Some((installation_id, config_revision)) = target else {
                if action == "revoke" {
                    continue;
                }
                tx.execute(
                    "UPDATE share_control_operations
                     SET status = 'rejected', last_error = 'Share no longer exists', updated_at = ?2
                     WHERE id = ?1",
                    params![operation_id, now],
                )
                .map_err(map_db("reject missing Share operation"))?;
                if let Some(record) = subscription_record(&tx, &subscription_id)? {
                    finish_release_tx(
                        &tx,
                        &record.id,
                        &record.seat_id,
                        &record.listing_id,
                        "share_missing",
                        &now,
                    )?;
                }
                continue;
            };
            let policy = policy_json
                .as_deref()
                .map(serde_json::from_str::<ShareUserPolicy>)
                .transpose()
                .map_err(|_| AppError::Internal("stored Share grant policy is invalid".into()))?;
            let action_enum = match action.as_str() {
                "upsert" => ShareManagedGrantAction::Upsert,
                "revoke" => ShareManagedGrantAction::Revoke,
                _ => {
                    return Err(AppError::Internal(
                        "stored Share control action is invalid".into(),
                    ));
                }
            };
            let patch = ShareSettingsPatch {
                managed_grant: Some(ShareManagedGrantOperation {
                    operation_id: operation_id.clone(),
                    entitlement_id,
                    share_sequence,
                    expected_config_revision: u64::try_from(config_revision).unwrap_or(0),
                    action: action_enum,
                    email,
                    policy,
                }),
                ..ShareSettingsPatch::default()
            };
            let patch_json = serde_json::to_string(&patch).map_err(|error| {
                AppError::Internal(format!("encode Share control patch failed: {error}"))
            })?;
            let edit_revision: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(revision), 0) + 1 FROM share_edit_requests WHERE share_id = ?1",
                    params![share_id],
                    |row| row.get(0),
                )
                .map_err(map_db("allocate Share edit revision"))?;
            let edit_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO share_edit_requests (
                    id, share_id, installation_id, owner_email, revision, status,
                    patch_json, created_by_email, created_at, updated_at,
                    applied_at, error_message, retired_at
                 ) VALUES (?1, ?2, ?3, 'share-market@router.internal', ?4, 'pending',
                           ?5, 'share-market@router.internal', ?6, ?6, NULL, NULL, NULL)",
                params![
                    edit_id,
                    share_id,
                    installation_id,
                    edit_revision,
                    patch_json,
                    now
                ],
            )
            .map_err(map_db("dispatch Share control edit"))?;
            tx.execute(
                "UPDATE share_control_operations
                 SET status = 'dispatched', edit_id = ?2, attempts = attempts + 1,
                     last_error = NULL, updated_at = ?3 WHERE id = ?1 AND status = 'pending'",
                params![operation_id, edit_id, now],
            )
            .map_err(map_db("mark Share control dispatched"))?;
            events.push(ShareEditAvailableEvent {
                kind: "share_edit_available".to_string(),
                installation_id,
                share_id,
                revision: edit_revision,
            });
        }
        tx.commit()
            .map_err(map_db("commit Share Market reconciliation"))?;
        Ok(events)
    }
}

pub(crate) fn handle_control_edit_ack(
    conn: &Connection,
    edit_id: &str,
    status: &str,
    error_message: Option<&str>,
    now: &str,
) -> Result<(), AppError> {
    let operation: Option<(
        String,
        String,
        String,
        i64,
        String,
        String,
        String,
        String,
        Option<String>,
    )> = conn
        .query_row(
            "SELECT id, status, action, attempts, subscription_id, share_id,
                    entitlement_id, email, policy_json
             FROM share_control_operations WHERE edit_id = ?1",
            params![edit_id],
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
                ))
            },
        )
        .optional()
        .map_err(map_db("read acknowledged Share control operation"))?;
    let Some((
        operation_id,
        operation_status,
        action,
        attempts,
        subscription_id,
        share_id,
        entitlement_id,
        email,
        policy_json,
    )) = operation
    else {
        return Ok(());
    };
    if operation_status != "dispatched" {
        return Ok(());
    }
    if status == "applied" {
        let policy = policy_json
            .as_deref()
            .map(serde_json::from_str::<ShareUserPolicy>)
            .transpose()
            .map_err(|_| AppError::Internal("stored Share grant policy is invalid".into()))?;
        apply_control_grant_effect(
            conn,
            &share_id,
            &action,
            &email,
            &entitlement_id,
            policy.as_ref(),
            now,
        )?;
        conn.execute(
            "UPDATE share_control_operations
             SET status = 'applied', updated_at = ?2, applied_at = ?2, last_error = NULL
             WHERE id = ?1 AND status = 'dispatched'",
            params![operation_id, now],
        )
        .map_err(map_db("complete Share control operation"))?;
        return Ok(());
    }
    let retryable = error_message
        .is_some_and(|message| message.contains("expected config revision"))
        && attempts < MAX_CONTROL_ATTEMPTS;
    if retryable {
        conn.execute(
            "UPDATE share_control_operations
             SET status = 'pending', edit_id = NULL, updated_at = ?2, last_error = ?3
             WHERE id = ?1 AND status = 'dispatched'",
            params![operation_id, now, error_message],
        )
        .map_err(map_db("retry Share control operation"))?;
        return Ok(());
    }
    conn.execute(
        "UPDATE share_control_operations
         SET status = 'rejected', updated_at = ?2, last_error = ?3
         WHERE id = ?1 AND status = 'dispatched'",
        params![operation_id, now, error_message],
    )
    .map_err(map_db("reject Share control operation"))?;
    if action == "upsert" {
        let seat_id: Option<String> = conn
            .query_row(
                "SELECT seat_id FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db("read failed Share grant seat"))?;
        let grant_failed = conn
            .execute(
                "UPDATE share_market_subscriptions
                 SET status = 'grant_failed', release_reason = ?2, updated_at = ?3, released_at = ?3
                 WHERE id = ?1 AND status = 'grant_pending'",
                params![subscription_id, error_message, now],
            )
            .map_err(map_db("fail Share grant subscription"))?;
        if grant_failed == 1
            && let Some(seat_id) = seat_id
        {
            conn.execute(
                "UPDATE share_market_seats
                 SET status = CASE
                        WHEN (SELECT status FROM share_market_listings WHERE id = listing_id) = 'active'
                            THEN 'available' ELSE 'disabled' END,
                     current_subscription_id = NULL, updated_at = ?2
                 WHERE id = ?1 AND current_subscription_id = ?3",
                params![seat_id, now, subscription_id],
            )
            .map_err(map_db("release failed Share grant seat"))?;
        }
    } else {
        conn.execute(
            "UPDATE share_market_subscriptions
             SET status = 'revoke_failed', release_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND status = 'revoke_pending'",
            params![subscription_id, error_message, now],
        )
        .map_err(map_db("mark Share revoke failed"))?;
    }
    Ok(())
}

fn apply_control_grant_effect(
    conn: &Connection,
    share_id: &str,
    action: &str,
    email: &str,
    entitlement_id: &str,
    policy: Option<&ShareUserPolicy>,
    now: &str,
) -> Result<(), AppError> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT COALESCE(user_grants_json, '{}'), COALESCE(shared_with_emails_json, '[]')
             FROM shares WHERE share_id = ?1",
            params![share_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_db("read Share grants for control effect"))?;
    let Some((grants_json, shared_json)) = row else {
        return Ok(());
    };
    let mut grants: BTreeMap<String, ShareUserGrant> =
        serde_json::from_str(&grants_json).unwrap_or_default();
    let mut shared: Vec<String> = serde_json::from_str(&shared_json).unwrap_or_default();
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() {
        return Ok(());
    }
    let now_ms = Utc::now().timestamp_millis().max(0) as u128;
    match action {
        "upsert" => {
            let Some(policy) = policy.cloned() else {
                return Err(AppError::Internal(
                    "Share Market grant upsert is missing policy".into(),
                ));
            };
            let previous = grants.get(&email).cloned();
            grants.insert(
                email.clone(),
                ShareUserGrant {
                    email: email.clone(),
                    role: "shareto".to_string(),
                    active: true,
                    policy,
                    usage: previous
                        .as_ref()
                        .map(|grant| grant.usage.clone())
                        .unwrap_or_default(),
                    created_at_ms: previous
                        .as_ref()
                        .map(|grant| grant.created_at_ms)
                        .filter(|created_at| *created_at > 0)
                        .unwrap_or(now_ms),
                    updated_at_ms: now_ms,
                    revoked_at_ms: None,
                    revision: previous
                        .as_ref()
                        .map(|grant| grant.revision.saturating_add(1))
                        .unwrap_or(1)
                        .max(1),
                    manager: ShareGrantManager::RouterShareMarket,
                    entitlement_id: Some(entitlement_id.to_string()),
                },
            );
            if !shared
                .iter()
                .any(|value| value.eq_ignore_ascii_case(&email))
            {
                shared.push(email);
            }
        }
        "revoke" => {
            let target_email = grants
                .iter()
                .find(|(_, grant)| {
                    grant.manager == ShareGrantManager::RouterShareMarket
                        && grant.entitlement_id.as_deref() == Some(entitlement_id)
                })
                .map(|(key, _)| key.clone())
                .unwrap_or_else(|| email.clone());
            if let Some(grant) = grants.get_mut(&target_email) {
                grant.active = false;
                grant.updated_at_ms = now_ms;
                grant.revoked_at_ms = Some(now_ms);
                grant.revision = grant.revision.saturating_add(1).max(1);
            }
            shared.retain(|value| !value.eq_ignore_ascii_case(&target_email));
        }
        _ => return Ok(()),
    }
    let grants_json = serde_json::to_string(&grants).map_err(|error| {
        AppError::Internal(format!("encode Share grants after control effect failed: {error}"))
    })?;
    let shared_json = serde_json::to_string(&shared).map_err(|error| {
        AppError::Internal(format!(
            "encode Share ACL after control effect failed: {error}"
        ))
    })?;
    conn.execute(
        "UPDATE shares
         SET user_grants_json = ?2, shared_with_emails_json = ?3, updated_at = ?4
         WHERE share_id = ?1",
        params![share_id, grants_json, shared_json, now],
    )
    .map_err(map_db("persist Share control grant effect"))?;
    Ok(())
}

async fn try_apply_dispatched_edit_via_ctl(
    state: &ServerState,
    event: &ShareEditAvailableEvent,
) -> Result<bool, AppError> {
    let Some(edit) = state
        .store
        .pending_share_edit_for_share(&event.share_id, event.revision)
        .await?
    else {
        return Ok(false);
    };
    let route = state.proxy.route_by_share_id(&event.share_id).await;
    let secret = state
        .store
        .installation_control_secret(&event.installation_id)
        .await
        .unwrap_or(None);
    let (Some(route), Some(secret)) = (route, secret) else {
        return Ok(false);
    };
    match crate::ctl_client::apply_share_settings(
        route.route_target(),
        &event.installation_id,
        &secret,
        &event.share_id,
        &edit.patch,
    )
    .await
    {
        Ok(returned_share) => {
            state
                .store
                .apply_share_edit_directly(&edit.id, returned_share)
                .await?;
            Ok(true)
        }
        Err(error) if error.is_transport() => {
            tracing::info!(
                share_id = %event.share_id,
                edit_id = %edit.id,
                error = %error,
                "Share Market control RPC unavailable; keeping async pending edit"
            );
            Ok(false)
        }
        Err(error) => {
            let message = error.to_string();
            tracing::warn!(
                share_id = %event.share_id,
                edit_id = %edit.id,
                error = %message,
                "Share Market control RPC rejected managed grant"
            );
            let _ = state
                .store
                .mark_share_edit_rejected(&edit.id, &message)
                .await;
            Ok(false)
        }
    }
}

async fn run_once(state: &ServerState) -> Result<(), AppError> {
    let events = state
        .store
        .share_market_reconcile_and_dispatch(Utc::now())
        .await?;
    let mut applied = false;
    for event in &events {
        let _ = state.share_edit_events.send(event.clone());
        match try_apply_dispatched_edit_via_ctl(state, event).await {
            Ok(true) => applied = true,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(
                    share_id = %event.share_id,
                    error = %error,
                    "Share Market synchronous grant apply failed"
                );
            }
        }
    }
    if applied || !events.is_empty() {
        // Advance grant_pending / finish revoke_pending after ctl apply or
        // descriptor-side entitlement observation from a prior cycle.
        let follow_up = state
            .store
            .share_market_reconcile_and_dispatch(Utc::now())
            .await?;
        for event in follow_up {
            let _ = state.share_edit_events.send(event);
        }
    }
    Ok(())
}

pub async fn run_service(state: ServerState) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(StdDuration::from_secs(SERVICE_CYCLE_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if let Err(error) = run_once(&state).await {
            tracing::warn!(error = %error, "Share Market reconciliation failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone};

    fn session(user_id: &str, email: &str) -> AuthSession {
        let now = Utc::now();
        AuthSession {
            session_id: format!("session-{user_id}"),
            user_id: user_id.to_string(),
            email: email.to_ascii_lowercase(),
            installation_id: format!("browser-{user_id}"),
            access_token_hash: format!("access-{user_id}"),
            refresh_token_hash: format!("refresh-{user_id}"),
            access_expires_at: now + Duration::hours(1),
            refresh_expires_at: now + Duration::days(30),
            created_at: now,
            last_used_at: now,
        }
    }

    fn free_seat() -> SeatInput {
        SeatInput {
            parallel_limit: Some(2),
            token_limit: Some(10_000),
            token_period: ShareTokenPeriod::Day,
            price_minor: None,
            currency: None,
            period_unit: None,
            period_count: None,
        }
    }

    fn paid_seat() -> SeatInput {
        SeatInput {
            price_minor: Some(1_200),
            currency: Some("CNY".into()),
            period_unit: Some("day".into()),
            period_count: Some(30),
            ..free_seat()
        }
    }

    async fn insert_share(
        store: &AppStore,
        share_id: &str,
        owner_email: &str,
        supported_periods: &[ShareTokenPeriod],
    ) {
        let now = Utc::now().to_rfc3339();
        let periods = serde_json::to_string(supported_periods).expect("serialize periods");
        store
            .conn
            .lock()
            .await
            .execute(
                "INSERT INTO shares (
                    share_id, installation_id, share_name, owner_email, subdomain,
                    app_type, token_limit, parallel_limit, tokens_used, requests_count,
                    share_status, created_at, expires_at, user_grants_json,
                    supported_user_token_periods_json, config_revision, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'codex', -1, 3, 0, 0,
                           'active', ?6, '9999-12-31T23:59:59Z', '{}', ?7, 1, ?6)",
                params![
                    share_id,
                    format!("installation-{share_id}"),
                    format!("Share {share_id}"),
                    owner_email,
                    format!("{share_id}-route"),
                    now,
                    periods,
                ],
            )
            .expect("insert Share");
    }

    async fn configure_payment_profile(
        store: &AppStore,
        owner: &AuthSession,
        account: &str,
        updated_at: &str,
    ) {
        let methods = serde_json::to_string(&vec![PaymentMethod {
            kind: "alipay".into(),
            account: Some(account.into()),
            qr_image_url: None,
            asset_url: None,
            token: None,
            chain: None,
            address: None,
            instructions: None,
        }])
        .expect("serialize payment methods");
        store
            .conn
            .lock()
            .await
            .execute(
                "INSERT INTO account_payment_profiles (user_id, owner_email, methods_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(user_id) DO UPDATE SET methods_json = excluded.methods_json,
                    updated_at = excluded.updated_at",
                params![owner.user_id, owner.email, methods, updated_at],
            )
            .expect("configure payment profile");
    }

    async fn create_listing(
        store: &AppStore,
        owner: &AuthSession,
        share_id: &str,
        seat: SeatInput,
    ) -> (String, String) {
        let listing_id = store
            .share_market_create_listing(
                owner,
                CreateListingRequest {
                    share_id: share_id.into(),
                    seats: vec![seat],
                },
            )
            .await
            .expect("create listing");
        let seat_id = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT id FROM share_market_seats WHERE listing_id = ?1",
                params![listing_id],
                |row| row.get(0),
            )
            .expect("read listing seat");
        (listing_id, seat_id)
    }

    async fn subscription_entitlement(store: &AppStore, subscription_id: &str) -> String {
        store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT entitlement_id FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read entitlement")
    }

    async fn set_entitlement(store: &AppStore, subscription_id: &str) {
        let (share_id, email, entitlement_id): (String, String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT share_id, renter_email, entitlement_id
                 FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read subscription grant identity");
        let grant = ShareUserGrant {
            email: email.clone(),
            role: "shareto".into(),
            active: true,
            policy: ShareUserPolicy::default(),
            usage: Default::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            revoked_at_ms: None,
            revision: 1,
            manager: ShareGrantManager::RouterShareMarket,
            entitlement_id: Some(entitlement_id),
        };
        let grants = serde_json::to_string(&BTreeMap::from([(email, grant)]))
            .expect("serialize managed grant");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET user_grants_json = ?2,
                    config_revision = config_revision + 1, updated_at = ?3
                 WHERE share_id = ?1",
                params![share_id, grants, Utc::now().to_rfc3339()],
            )
            .expect("publish managed grant descriptor");
    }

    async fn clear_entitlements(store: &AppStore, share_id: &str) {
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET user_grants_json = '{}',
                    config_revision = config_revision + 1, updated_at = ?2
                 WHERE share_id = ?1",
                params![share_id, Utc::now().to_rfc3339()],
            )
            .expect("clear managed grant descriptor");
    }

    async fn activate_subscription(store: &AppStore, subscription_id: &str, now: DateTime<Utc>) {
        let dispatched = store
            .share_market_reconcile_and_dispatch(now)
            .await
            .expect("dispatch managed grant");
        assert_eq!(dispatched.len(), 1);
        set_entitlement(store, subscription_id).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(now)
                .await
                .expect("confirm managed grant")
                .is_empty()
        );
    }

    async fn subscription_status(store: &AppStore, subscription_id: &str) -> String {
        store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read subscription status")
    }

    #[test]
    fn empty_price_and_period_is_free() {
        let seat = normalize_seat(free_seat()).expect("free seat");
        assert!(seat.is_free());
    }

    #[test]
    fn partial_or_zero_pricing_is_rejected() {
        let mut input = free_seat();
        input.price_minor = Some(100);
        assert!(normalize_seat(input).is_err());

        let mut input = free_seat();
        input.price_minor = Some(0);
        input.currency = Some("CNY".into());
        input.period_unit = Some("month".into());
        input.period_count = Some(1);
        assert!(normalize_seat(input).is_err());

        let mut input = free_seat();
        input.token_limit = Some(i64::MAX as u64 + 1);
        assert!(normalize_seat(input).is_err());
    }

    #[test]
    fn billing_month_uses_calendar_arithmetic() {
        let start = Utc.with_ymd_and_hms(2026, 1, 31, 12, 0, 0).unwrap();
        let end = add_billing_period(start, "month", 1).expect("period");
        assert_eq!(end.month(), 2);
        assert_eq!(end.day(), 28);
    }

    #[tokio::test]
    async fn fixed_token_period_is_anchored_to_the_rental_minute() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-anchor", "owner-anchor@example.com");
        let renter = session("renter-anchor", "renter-anchor@example.com");
        insert_share(
            &store,
            "share-anchor",
            &owner.email,
            &[ShareTokenPeriod::SevenDays],
        )
        .await;
        let mut seat = free_seat();
        seat.token_period = ShareTokenPeriod::SevenDays;
        let (_, seat_id) = create_listing(&store, &owner, "share-anchor", seat).await;

        let before = Utc::now().timestamp_millis();
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent fixed-period seat");
        let after = Utc::now().timestamp_millis();
        let policy_json: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT policy_json FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read fixed-period policy");
        let policy: ShareUserPolicy =
            serde_json::from_str(&policy_json).expect("decode fixed-period policy");
        let anchor = policy
            .token_period_anchor_at_ms
            .expect("fixed period requires anchor");

        assert_eq!(policy.token_period, ShareTokenPeriod::SevenDays);
        assert_eq!(anchor % 60_000, 0);
        assert!(anchor <= after);
        assert!(anchor >= before - 60_000);
    }

    #[test]
    fn schema_enforces_one_active_renter_per_share() {
        let conn = Connection::open_in_memory().expect("memory database");
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        init_schema(&conn).expect("schema");
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'share_market_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(tables >= 6);
    }

    #[tokio::test]
    async fn undispatched_grants_are_canceled_without_issuing_revokes() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-cancel", "owner-cancel@example.com");
        let renter_a = session("renter-cancel-a", "renter-cancel-a@example.com");
        let renter_b = session("renter-cancel-b", "renter-cancel-b@example.com");
        insert_share(
            &store,
            "share-cancel",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-cancel", free_seat()).await;

        let first_subscription = store
            .share_market_rent_seat(&renter_a, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent seat before Share becomes unavailable");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET share_status = 'paused' WHERE share_id = 'share-cancel'",
                [],
            )
            .expect("pause Share before grant dispatch");
        assert!(
            store
                .share_market_reconcile_and_dispatch(Utc::now())
                .await
                .expect("cancel unavailable undispatched grant")
                .is_empty()
        );
        let (subscription_status, seat_status, upsert_status, revoke_count): (
            String,
            String,
            String,
            i64,
        ) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, seat.status, operation.status,
                        (SELECT COUNT(*) FROM share_control_operations revoke
                         WHERE revoke.subscription_id = sub.id AND revoke.action = 'revoke')
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 JOIN share_control_operations operation
                   ON operation.subscription_id = sub.id AND operation.action = 'upsert'
                 WHERE sub.id = ?1",
                params![first_subscription],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read canceled unavailable grant");
        assert_eq!(subscription_status, SUB_RELEASED);
        assert_eq!(seat_status, SEAT_AVAILABLE);
        assert_eq!(upsert_status, "rejected");
        assert_eq!(revoke_count, 0);

        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET share_status = 'active' WHERE share_id = 'share-cancel'",
                [],
            )
            .expect("reactivate Share");
        let second_subscription = store
            .share_market_rent_seat(&renter_b, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent seat before immediate release");
        store
            .share_market_request_release(&renter_b, &second_subscription, false, false, None)
            .await
            .expect("release before grant dispatch");
        let (status, revoke_count): (String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status,
                        (SELECT COUNT(*) FROM share_control_operations operation
                         WHERE operation.subscription_id = sub.id AND operation.action = 'revoke')
                 FROM share_market_subscriptions sub WHERE sub.id = ?1",
                params![second_subscription],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read immediately released grant");
        assert_eq!(status, SUB_RELEASED);
        assert_eq!(revoke_count, 0);
    }

    #[tokio::test]
    async fn dispatched_grant_waits_for_confirmed_revoke_before_releasing() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-fence", "owner-fence@example.com");
        let renter = session("renter-fence", "renter-fence@example.com");
        insert_share(
            &store,
            "share-fence",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-fence", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent fenced seat");
        let now = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now)
                .await
                .expect("dispatch grant before Share loss")
                .len(),
            1
        );
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET share_status = 'paused' WHERE share_id = 'share-fence'",
                [],
            )
            .expect("pause Share after grant dispatch");
        assert!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
                .await
                .expect("queue ordered revoke")
                .is_empty()
        );
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(2))
            .await
            .expect("do not infer revoke from an unconfirmed empty snapshot");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );

        let upsert_edit: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read dispatched grant edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                params![upsert_edit],
            )
            .expect("reject grant edit after revoke was requested");
            handle_control_edit_ack(
                &conn,
                &upsert_edit,
                "rejected",
                Some("Share is unavailable"),
                &(now + Duration::seconds(3)).to_rfc3339(),
            )
            .expect("complete failed grant operation without releasing the revoking seat");
        }
        let seat_status: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status FROM share_market_seats WHERE id = ?1",
                params![seat_id],
                |row| row.get(0),
            )
            .expect("read fenced seat after late grant failure");
        assert_eq!(seat_status, "revoking");
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(4))
                .await
                .expect("dispatch revoke after grant resolves")
                .len(),
            1
        );
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );
        let revoke_edit: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'revoke'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read dispatched revoke edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'applied' WHERE id = ?1",
                params![revoke_edit],
            )
            .expect("ack revoke edit");
            handle_control_edit_ack(
                &conn,
                &revoke_edit,
                "applied",
                None,
                &(now + Duration::seconds(5)).to_rfc3339(),
            )
            .expect("complete revoke operation");
        }
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(6))
            .await
            .expect("release after confirmed revoke");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_RELEASED
        );
    }

    #[tokio::test]
    async fn force_revoke_unblocks_when_stuck_behind_dispatched_unconfirmed_grant() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-stuck-revoke", "owner-stuck-revoke@example.com");
        let renter = session("renter-stuck-revoke", "renter-stuck-revoke@example.com");
        insert_share(
            &store,
            "share-stuck-revoke",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-stuck-revoke", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent stuck-revoke seat");
        let now = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now)
                .await
                .expect("dispatch grant")
                .len(),
            1
        );
        store
            .share_market_request_release(&owner, &subscription_id, true, false, None)
            .await
            .expect("force revoke while grant still unconfirmed");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );
        let (upsert_status, revoke_status): (String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT
                    (SELECT status FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = 'upsert'),
                    (SELECT status FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = 'revoke')",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read control ops after force revoke");
        assert_eq!(upsert_status, "rejected");
        assert_eq!(revoke_status, "pending");
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
                .await
                .expect("dispatch revoke after retiring stuck grant")
                .len(),
            1
        );
        let revoke_edit: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'revoke'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read dispatched revoke edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'applied' WHERE id = ?1",
                params![revoke_edit],
            )
            .expect("mark revoke applied");
            handle_control_edit_ack(
                &conn,
                &revoke_edit,
                "applied",
                None,
                &(now + Duration::seconds(2)).to_rfc3339(),
            )
            .expect("ack revoke");
        }
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(3))
            .await
            .expect("finish after revoke ack");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_RELEASED
        );
    }

    #[tokio::test]
    async fn missing_share_keeps_an_active_entitlement_fenced_until_a_fresh_descriptor_returns() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-missing", "owner-missing@example.com");
        let renter = session("renter-missing", "renter-missing@example.com");
        insert_share(
            &store,
            "share-missing",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, seat_id) =
            create_listing(&store, &owner, "share-missing", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent missing Share seat");
        let now = Utc::now();
        activate_subscription(&store, &subscription_id, now).await;

        store
            .conn
            .lock()
            .await
            .execute("DELETE FROM shares WHERE share_id = 'share-missing'", [])
            .expect("remove Share descriptor");
        assert!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
                .await
                .expect("fence missing active Share")
                .is_empty()
        );
        let (status_after_missing, seat_status, revoke_status): (String, String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, seat.status, operation.status
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 JOIN share_control_operations operation
                   ON operation.subscription_id = sub.id AND operation.action = 'revoke'
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read fenced missing Share");
        assert_eq!(status_after_missing, SUB_REVOKE_PENDING);
        assert_eq!(seat_status, "revoking");
        assert_eq!(revoke_status, "pending");

        insert_share(
            &store,
            "share-missing",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(2))
            .await
            .expect("confirm absent entitlement from fresh descriptor");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_RELEASED
        );
        let seat_status: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status FROM share_market_seats WHERE id = ?1 AND listing_id = ?2",
                params![seat_id, listing_id],
                |row| row.get(0),
            )
            .expect("read released missing Share seat");
        assert_eq!(seat_status, SEAT_AVAILABLE);
    }

    #[tokio::test]
    async fn retired_dispatched_grant_does_not_block_revoke_when_share_returns() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-retired", "owner-retired@example.com");
        let renter = session("renter-retired", "renter-retired@example.com");
        insert_share(
            &store,
            "share-retired",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-retired", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent retired Share seat");
        let now = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now)
                .await
                .expect("dispatch grant before Share retirement")
                .len(),
            1
        );
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests
                 SET status = 'cancelled', retired_at = ?1
                 WHERE share_id = 'share-retired' AND status = 'pending'",
                params![(now + Duration::seconds(1)).to_rfc3339()],
            )
            .expect("retire dispatched grant edit");
            conn.execute("DELETE FROM shares WHERE share_id = 'share-retired'", [])
                .expect("remove retired Share descriptor");
        }
        assert!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(2))
                .await
                .expect("recover orphaned grant and queue revoke")
                .is_empty()
        );
        let (upsert_status, revoke_status, subscription_status): (String, String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT upsert.status, revoke.status, sub.status
                 FROM share_market_subscriptions sub
                 JOIN share_control_operations upsert
                   ON upsert.subscription_id = sub.id AND upsert.action = 'upsert'
                 JOIN share_control_operations revoke
                   ON revoke.subscription_id = sub.id AND revoke.action = 'revoke'
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read recovered orphaned grant");
        assert_eq!(upsert_status, "rejected");
        assert_eq!(revoke_status, "pending");
        assert_eq!(subscription_status, SUB_REVOKE_PENDING);

        insert_share(
            &store,
            "share-retired",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        set_entitlement(&store, &subscription_id).await;
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(3))
                .await
                .expect("dispatch revoke after Share returns")
                .len(),
            1
        );
        let revoke_status: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'revoke'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read dispatched recovered revoke");
        assert_eq!(revoke_status, "dispatched");
    }

    #[tokio::test]
    async fn free_rental_has_no_invoice_and_descriptor_confirmation_recovers_lost_acks() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner", "owner@example.com");
        let renter = session("renter", "renter@example.com");
        insert_share(&store, "share-free", &owner.email, &[ShareTokenPeriod::Day]).await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-free", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent free seat");
        let now = Utc::now();
        activate_subscription(&store, &subscription_id, now).await;

        let (subscription_status, seat_status, invoice_count, upsert_status, edit_status): (
            String,
            String,
            i64,
            String,
            String,
        ) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, seat.status,
                        (SELECT COUNT(*) FROM share_market_invoices invoice
                         WHERE invoice.subscription_id = sub.id),
                        operation.status, edit.status
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 JOIN share_control_operations operation
                   ON operation.subscription_id = sub.id AND operation.action = 'upsert'
                 JOIN share_edit_requests edit ON edit.id = operation.edit_id
                 WHERE sub.id = ?1",
                params![subscription_id],
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
            .expect("read active free rental");
        assert_eq!(subscription_status, "active_free");
        assert_eq!(seat_status, "occupied");
        assert_eq!(invoice_count, 0);
        assert_eq!(upsert_status, "applied");
        assert_eq!(edit_status, "applied");

        store
            .share_market_request_release(&renter, &subscription_id, false, false, None)
            .await
            .expect("request free rental release");
        clear_entitlements(&store, "share-free").await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
                .await
                .expect("confirm revoke without ack")
                .is_empty()
        );
        let (subscription_status, seat_status, revoke_status, revoke_attempts): (
            String,
            String,
            String,
            i64,
        ) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, seat.status, operation.status, operation.attempts
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 JOIN share_control_operations operation
                   ON operation.subscription_id = sub.id AND operation.action = 'revoke'
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read released free rental");
        assert_eq!(subscription_status, "released");
        assert_eq!(seat_status, "available");
        assert_eq!(revoke_status, "applied");
        assert_eq!(revoke_attempts, 0);

        store
            .share_market_delete_seat(&owner, &seat_id)
            .await
            .expect("delete released seat without deleting subscription history");
        let (stored_status, subscription_count): (String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT seat.status,
                        (SELECT COUNT(*) FROM share_market_subscriptions sub
                         WHERE sub.seat_id = seat.id)
                 FROM share_market_seats seat WHERE seat.id = ?1",
                params![seat_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read soft-deleted Share seat");
        assert_eq!(stored_status, SEAT_DELETED);
        assert_eq!(subscription_count, 1);
        let catalog = store
            .share_market_catalog(Some(&owner), &[])
            .await
            .expect("read catalog after deleting released seat");
        assert!(catalog.listings[0].seats.is_empty());
    }

    #[tokio::test]
    async fn paid_rental_trials_declares_idempotently_renews_and_times_out() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-paid", "owner-paid@example.com");
        let renter = session("renter-paid", "renter-paid@example.com");
        configure_payment_profile(&store, &owner, "account-v1", "profile-v1").await;
        insert_share(&store, "share-paid", &owner.email, &[ShareTokenPeriod::Day]).await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-paid", paid_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent paid seat");
        let activated_at = Utc::now();
        activate_subscription(&store, &subscription_id, activated_at).await;

        let catalog = store
            .share_market_catalog(Some(&renter), &[])
            .await
            .expect("load paid rental");
        let subscription = catalog
            .my_subscriptions
            .iter()
            .find(|item| item.id == subscription_id)
            .expect("paid subscription view");
        let invoice = subscription.open_invoice.as_ref().expect("initial invoice");
        assert_eq!(subscription.status, SUB_TRIAL_PAYMENT_DUE);
        assert_eq!(invoice.sequence, 1);
        assert_eq!(invoice.amount_minor, 1_200);
        assert_eq!(
            subscription.payment_profile_updated_at.as_deref(),
            Some("profile-v1")
        );
        assert_eq!(
            subscription
                .payment_methods
                .as_ref()
                .and_then(|methods| methods.first())
                .and_then(|method| method.account.as_deref()),
            Some("account-v1")
        );
        let trial_end = parse_time(subscription.trial_ends_at.as_deref().expect("trial end"))
            .expect("parse trial end");
        assert_eq!(trial_end, activated_at + Duration::hours(TRIAL_HOURS));

        let stale_declaration = DeclarePaidRequest {
            invoice_id: invoice.id.clone(),
            offer_revision: invoice.sequence,
            amount_minor_confirmed: invoice.amount_minor,
            payment_profile_updated_at: "profile-v1".into(),
            confirmed: true,
        };
        configure_payment_profile(&store, &owner, "account-v2", "profile-v2").await;
        assert!(matches!(
            store
                .share_market_declare_paid(&renter, &subscription_id, stale_declaration)
                .await,
            Err(AppError::Conflict(_))
        ));

        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE account_payment_profiles
                 SET methods_json = '[]', updated_at = 'profile-empty'
                 WHERE user_id = ?1",
                params![owner.user_id],
            )
            .expect("clear payment methods");
        assert!(matches!(
            store
                .share_market_declare_paid(
                    &renter,
                    &subscription_id,
                    DeclarePaidRequest {
                        invoice_id: invoice.id.clone(),
                        offer_revision: 1,
                        amount_minor_confirmed: invoice.amount_minor,
                        payment_profile_updated_at: "profile-empty".into(),
                        confirmed: true,
                    },
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        configure_payment_profile(&store, &owner, "account-v2", "profile-v2").await;

        let refreshed = store
            .share_market_catalog(Some(&renter), &[])
            .await
            .expect("reload payment details");
        let refreshed = refreshed
            .my_subscriptions
            .iter()
            .find(|item| item.id == subscription_id)
            .expect("refreshed subscription");
        let invoice = refreshed.open_invoice.as_ref().expect("refreshed invoice");
        let declaration = DeclarePaidRequest {
            invoice_id: invoice.id.clone(),
            offer_revision: refreshed.offer_revision,
            amount_minor_confirmed: invoice.amount_minor,
            payment_profile_updated_at: "profile-v2".into(),
            confirmed: true,
        };
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET owner_email = 'replacement@example.com'
                 WHERE share_id = 'share-paid'",
                [],
            )
            .expect("change Share owner before payment");
        assert!(matches!(
            store
                .share_market_declare_paid(&renter, &subscription_id, declaration.clone())
                .await,
            Err(AppError::Conflict(_))
        ));
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET owner_email = ?2 WHERE share_id = ?1",
                params!["share-paid", owner.email],
            )
            .expect("restore Share owner after payment rejection");
        store
            .share_market_declare_paid(&renter, &subscription_id, declaration)
            .await
            .expect("declare initial payment");
        store
            .share_market_declare_paid(
                &renter,
                &subscription_id,
                DeclarePaidRequest {
                    invoice_id: invoice.id.clone(),
                    offer_revision: refreshed.offer_revision,
                    amount_minor_confirmed: invoice.amount_minor,
                    payment_profile_updated_at: "profile-v2".into(),
                    confirmed: true,
                },
            )
            .await
            .expect("repeat initial payment declaration");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_ACTIVE_PAID
        );

        let first_period_end: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT current_period_end FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read first period end");
        let first_period_end = parse_time(&first_period_end).expect("parse first period end");
        assert_eq!(first_period_end, trial_end + Duration::days(30));

        store
            .share_market_reconcile_and_dispatch(first_period_end)
            .await
            .expect("open renewal invoice");
        let renewal = store
            .share_market_catalog(Some(&renter), &[])
            .await
            .expect("load renewal");
        let renewal = renewal
            .my_subscriptions
            .iter()
            .find(|item| item.id == subscription_id)
            .expect("renewal subscription");
        let renewal_invoice = renewal.open_invoice.as_ref().expect("renewal invoice");
        assert_eq!(renewal.status, SUB_RENEWAL_DUE);
        assert_eq!(renewal_invoice.sequence, 2);
        store
            .share_market_declare_paid(
                &renter,
                &subscription_id,
                DeclarePaidRequest {
                    invoice_id: renewal_invoice.id.clone(),
                    offer_revision: renewal.offer_revision,
                    amount_minor_confirmed: renewal_invoice.amount_minor,
                    payment_profile_updated_at: "profile-v2".into(),
                    confirmed: true,
                },
            )
            .await
            .expect("declare renewal payment");

        let second_period_end: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT current_period_end FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read second period end");
        let second_period_end = parse_time(&second_period_end).expect("parse second period end");
        assert_eq!(second_period_end, first_period_end + Duration::days(30));
        store
            .share_market_reconcile_and_dispatch(second_period_end)
            .await
            .expect("open second renewal invoice");
        let revoke_events = store
            .share_market_reconcile_and_dispatch(
                second_period_end + Duration::hours(RENEWAL_GRACE_HOURS) + Duration::seconds(1),
            )
            .await
            .expect("revoke overdue paid rental");
        assert_eq!(revoke_events.len(), 1);
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );
        clear_entitlements(&store, "share-paid").await;
        store
            .share_market_reconcile_and_dispatch(
                second_period_end + Duration::hours(RENEWAL_GRACE_HOURS) + Duration::seconds(2),
            )
            .await
            .expect("confirm overdue revoke");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_RELEASED
        );
    }

    #[tokio::test]
    async fn concurrent_rent_allows_only_one_subscription() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-race", "owner-race@example.com");
        let renter_a = session("renter-a", "renter-a@example.com");
        let renter_b = session("renter-b", "renter-b@example.com");
        insert_share(&store, "share-race", &owner.email, &[ShareTokenPeriod::Day]).await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-race", free_seat()).await;
        let (result_a, result_b) = tokio::join!(
            store.share_market_rent_seat(
                &renter_a,
                &seat_id,
                RentSeatRequest { offer_revision: 1 }
            ),
            store.share_market_rent_seat(
                &renter_b,
                &seat_id,
                RentSeatRequest { offer_revision: 1 }
            )
        );
        assert_ne!(result_a.is_ok(), result_b.is_ok());
        let loser = if result_a.is_err() {
            result_a
        } else {
            result_b
        };
        assert!(matches!(loser, Err(AppError::Conflict(_))));
        let subscription_count: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT COUNT(*) FROM share_market_subscriptions WHERE seat_id = ?1",
                params![seat_id],
                |row| row.get(0),
            )
            .expect("count raced subscriptions");
        assert_eq!(subscription_count, 1);
    }

    #[tokio::test]
    async fn owner_can_close_force_revoke_block_and_lift_without_interrupting_early() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-control", "owner-control@example.com");
        let renter = session("renter-control", "renter-control@example.com");
        insert_share(
            &store,
            "share-control-a",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        insert_share(
            &store,
            "share-control-b",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_a, seat_a) =
            create_listing(&store, &owner, "share-control-a", free_seat()).await;
        let (_listing_b, seat_b) =
            create_listing(&store, &owner, "share-control-b", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_a, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent first owner seat");
        let now = Utc::now();
        activate_subscription(&store, &subscription_id, now).await;
        store
            .share_market_close_listing(&owner, &listing_a)
            .await
            .expect("close listing");
        let (listing_status, seat_status, subscription_status): (String, String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT listing.status, seat.status, sub.status
                 FROM share_market_listings listing
                 JOIN share_market_seats seat ON seat.listing_id = listing.id
                 JOIN share_market_subscriptions sub ON sub.id = seat.current_subscription_id
                 WHERE listing.id = ?1",
                params![listing_a],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read closed occupied listing");
        assert_eq!(listing_status, "closed");
        assert_eq!(seat_status, "occupied");
        assert_eq!(subscription_status, "active_free");

        store
            .share_market_request_release(
                &owner,
                &subscription_id,
                true,
                true,
                Some("false payment declaration"),
            )
            .await
            .expect("force revoke and block");
        assert!(matches!(
            store
                .share_market_rent_seat(&renter, &seat_b, RentSeatRequest { offer_revision: 1 })
                .await,
            Err(AppError::Forbidden(_))
        ));
        let blocks = {
            let conn = store.conn.lock().await;
            owner_blocks_for(&conn, &owner.user_id).expect("list owner blocks")
        };
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].blocked_user_id, renter.user_id);
        store
            .share_market_lift_block(&owner, &renter.user_id)
            .await
            .expect("lift renter block");
        store
            .share_market_rent_seat(&renter, &seat_b, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent after block lifted");

        clear_entitlements(&store, "share-control-a").await;
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
            .await
            .expect("confirm forced revoke");
        let seat_status: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status FROM share_market_seats WHERE id = ?1",
                params![seat_a],
                |row| row.get(0),
            )
            .expect("read closed released seat");
        assert_eq!(seat_status, SEAT_DISABLED);
    }

    #[tokio::test]
    async fn config_revision_retry_and_terminal_ack_are_idempotent() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-retry", "owner-retry@example.com");
        let renter = session("renter-retry", "renter-retry@example.com");
        insert_share(
            &store,
            "share-retry",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-retry", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent retry seat");
        let now = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now)
                .await
                .expect("dispatch first grant")
                .len(),
            1
        );
        let first_edit: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read first grant edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                params![first_edit],
            )
            .expect("reject first edit");
            handle_control_edit_ack(
                &conn,
                &first_edit,
                "rejected",
                Some("expected config revision 1 but found 2"),
                &now.to_rfc3339(),
            )
            .expect("retry config conflict");
            conn.execute(
                "UPDATE shares SET config_revision = 9 WHERE share_id = 'share-retry'",
                [],
            )
            .expect("advance Share config revision");
        }
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
                .await
                .expect("redispatch grant")
                .len(),
            1
        );
        let (operation_status, attempts, second_edit, patch_json): (String, i64, String, String) =
            store
                .conn
                .lock()
                .await
                .query_row(
                    "SELECT operation.status, operation.attempts, operation.edit_id,
                            edit.patch_json
                     FROM share_control_operations operation
                     JOIN share_edit_requests edit ON edit.id = operation.edit_id
                     WHERE operation.subscription_id = ?1 AND operation.action = 'upsert'",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read retried grant");
        assert_eq!(operation_status, "dispatched");
        assert_eq!(attempts, 2);
        assert_ne!(second_edit, first_edit);
        let patch: serde_json::Value =
            serde_json::from_str(&patch_json).expect("parse grant patch");
        assert_eq!(
            patch
                .pointer("/managedGrant/expectedConfigRevision")
                .and_then(serde_json::Value::as_u64),
            Some(9)
        );
        assert_eq!(
            patch
                .pointer("/managedGrant/entitlementId")
                .and_then(serde_json::Value::as_str),
            Some(
                subscription_entitlement(&store, &subscription_id)
                    .await
                    .as_str()
            )
        );

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'applied' WHERE id = ?1",
                params![second_edit],
            )
            .expect("apply retried edit");
            handle_control_edit_ack(
                &conn,
                &second_edit,
                "applied",
                None,
                &(now + Duration::seconds(2)).to_rfc3339(),
            )
            .expect("apply retried control operation");
            handle_control_edit_ack(
                &conn,
                &second_edit,
                "rejected",
                Some("late duplicate rejection"),
                &(now + Duration::seconds(3)).to_rfc3339(),
            )
            .expect("ignore late terminal ack");
        }
        let (operation_status, delayed_subscription_status): (String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT operation.status, sub.status
                 FROM share_control_operations operation
                 JOIN share_market_subscriptions sub ON sub.id = operation.subscription_id
                 WHERE operation.subscription_id = ?1 AND operation.action = 'upsert'",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read terminal ack state");
        assert_eq!(operation_status, "applied");
        assert_eq!(delayed_subscription_status, SUB_GRANT_PENDING);
        set_entitlement(&store, &subscription_id).await;
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(4))
            .await
            .expect("confirm delayed descriptor");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            "active_free"
        );
    }

    #[tokio::test]
    async fn applied_ack_writes_entitlement_so_reconcile_can_activate_without_descriptor_sync() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-ack-grant", "owner-ack-grant@example.com");
        let renter = session("renter-ack-grant", "renter-ack-grant@example.com");
        insert_share(
            &store,
            "share-ack-grant",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-ack-grant", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent ack-grant seat");
        let now = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now)
                .await
                .expect("dispatch grant")
                .len(),
            1
        );
        let edit_id: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read grant edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'applied' WHERE id = ?1",
                params![edit_id],
            )
            .expect("mark edit applied");
            handle_control_edit_ack(&conn, &edit_id, "applied", None, &now.to_rfc3339())
                .expect("ack writes managed grant into Share descriptor");
        }
        let entitlement_id = subscription_entitlement(&store, &subscription_id).await;
        let grants_json: Option<String> = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT user_grants_json FROM shares WHERE share_id = 'share-ack-grant'",
                [],
                |row| row.get(0),
            )
            .expect("read grants");
        assert!(
            active_entitlement(grants_json.as_deref(), &entitlement_id),
            "applied ack should materialize routerShareMarket grant"
        );
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
            .await
            .expect("activate from ack-written entitlement");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            "active_free"
        );

        store
            .share_market_request_release(&owner, &subscription_id, true, false, None)
            .await
            .expect("force revoke");
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(2))
                .await
                .expect("dispatch revoke")
                .len(),
            1
        );
        let revoke_edit: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'revoke'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read revoke edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'applied' WHERE id = ?1",
                params![revoke_edit],
            )
            .expect("mark revoke applied");
            handle_control_edit_ack(
                &conn,
                &revoke_edit,
                "applied",
                None,
                &(now + Duration::seconds(3)).to_rfc3339(),
            )
            .expect("ack clears managed grant");
        }
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(4))
            .await
            .expect("release after ack-cleared entitlement");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_RELEASED
        );
    }

    #[tokio::test]
    async fn grant_and_revoke_rejections_have_recoverable_states() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-failure", "owner-failure@example.com");
        let first_renter = session("renter-failure-a", "renter-failure-a@example.com");
        let second_renter = session("renter-failure-b", "renter-failure-b@example.com");
        insert_share(
            &store,
            "share-failure",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-failure", free_seat()).await;
        let first_subscription = store
            .share_market_rent_seat(
                &first_renter,
                &seat_id,
                RentSeatRequest { offer_revision: 1 },
            )
            .await
            .expect("rent failure seat");
        let now = Utc::now();
        store
            .share_market_reconcile_and_dispatch(now)
            .await
            .expect("dispatch failing grant");
        let first_edit: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'",
                params![first_subscription],
                |row| row.get(0),
            )
            .expect("read failing grant edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                params![first_edit],
            )
            .expect("reject grant edit");
            handle_control_edit_ack(
                &conn,
                &first_edit,
                "rejected",
                Some("managed grant rejected"),
                &(now + Duration::seconds(1)).to_rfc3339(),
            )
            .expect("record failed grant");
        }
        assert_eq!(
            subscription_status(&store, &first_subscription).await,
            SUB_GRANT_FAILED
        );
        let second_subscription = store
            .share_market_rent_seat(
                &second_renter,
                &seat_id,
                RentSeatRequest { offer_revision: 1 },
            )
            .await
            .expect("reuse seat after failed grant");
        activate_subscription(&store, &second_subscription, now + Duration::seconds(2)).await;

        store
            .share_market_request_release(
                &owner,
                &second_subscription,
                true,
                false,
                Some("owner retry test"),
            )
            .await
            .expect("request revoke");
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(3))
            .await
            .expect("dispatch failing revoke");
        let first_revoke_edit: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'revoke'
                 ORDER BY share_sequence DESC LIMIT 1",
                params![second_subscription],
                |row| row.get(0),
            )
            .expect("read failing revoke edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                params![first_revoke_edit],
            )
            .expect("reject revoke edit");
            handle_control_edit_ack(
                &conn,
                &first_revoke_edit,
                "rejected",
                Some("managed revoke rejected"),
                &(now + Duration::seconds(4)).to_rfc3339(),
            )
            .expect("record failed revoke");
        }
        assert_eq!(
            subscription_status(&store, &second_subscription).await,
            SUB_REVOKE_FAILED
        );
        store
            .share_market_request_release(
                &owner,
                &second_subscription,
                true,
                false,
                Some("retry failed revoke"),
            )
            .await
            .expect("retry failed revoke");
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(5))
                .await
                .expect("redispatch revoke")
                .len(),
            1
        );
        clear_entitlements(&store, "share-failure").await;
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(6))
            .await
            .expect("confirm retried revoke");
        assert_eq!(
            subscription_status(&store, &second_subscription).await,
            SUB_RELEASED
        );
    }

    #[tokio::test]
    async fn seat_updates_reject_server_unsupported_periods_and_recover_after_owner_transfer() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-seat", "owner-seat@example.com");
        let new_owner = session("new-owner-seat", "new-owner@example.com");
        let renter = session("renter-seat", "renter-seat@example.com");
        insert_share(&store, "share-seat", &owner.email, &[ShareTokenPeriod::Day]).await;
        let (listing_id, seat_id) = create_listing(&store, &owner, "share-seat", free_seat()).await;
        let catalog = store
            .share_market_catalog(Some(&renter), &[])
            .await
            .expect("catalog before owner change");
        assert!(catalog.listings[0].seats[0].can_rent);
        let mut unsupported = free_seat();
        unsupported.token_period = ShareTokenPeriod::Week;
        assert!(matches!(
            store
                .share_market_update_seat(
                    &owner,
                    &seat_id,
                    UpdateSeatRequest {
                        seat: unsupported,
                        offer_revision: 1,
                    },
                )
                .await,
            Err(AppError::BadRequest(_))
        ));
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET owner_email = 'new-owner@example.com' WHERE share_id = 'share-seat'",
                [],
            )
            .expect("change Share owner");
        let renter_catalog = store
            .share_market_catalog(Some(&renter), &[])
            .await
            .expect("renter catalog after owner change");
        assert!(renter_catalog.listings.is_empty());
        let owner_catalog = store
            .share_market_catalog(Some(&owner), &[])
            .await
            .expect("former owner catalog after owner change");
        assert!(!owner_catalog.listings[0].seats[0].can_rent);
        assert!(matches!(
            store
                .share_market_add_seat(&owner, &listing_id, free_seat())
                .await,
            Err(AppError::Conflict(_))
        ));
        let owned = store
            .share_market_owned_shares(&new_owner)
            .await
            .expect("list transferred Share for new owner");
        assert_eq!(owned.len(), 1);
        assert!(!owned[0].already_listed);
        let replacement_listing = store
            .share_market_create_listing(
                &new_owner,
                CreateListingRequest {
                    share_id: "share-seat".into(),
                    seats: vec![free_seat()],
                },
            )
            .await
            .expect("new owner can relist after stale seats are clear");
        assert_ne!(replacement_listing, listing_id);
        let (old_status, old_seat_status, active_listings): (String, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT listing.status, seat.status,
                        (SELECT COUNT(*) FROM share_market_listings current
                         WHERE current.share_id = listing.share_id AND current.status = 'active')
                 FROM share_market_listings listing
                 JOIN share_market_seats seat ON seat.listing_id = listing.id
                 WHERE listing.id = ?1",
                params![listing_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read stale listing after ownership transfer");
        assert_eq!(old_status, "closed");
        assert_eq!(old_seat_status, SEAT_DISABLED);
        assert_eq!(active_listings, 1);
    }

    #[tokio::test]
    async fn closed_listing_without_active_rentals_can_relist_via_add_share() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-relist", "owner-relist@example.com");
        insert_share(
            &store,
            "share-relist",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, _seat_id) =
            create_listing(&store, &owner, "share-relist", free_seat()).await;
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("close empty listing");

        let owned = store
            .share_market_owned_shares(&owner)
            .await
            .expect("owned shares after close");
        assert_eq!(owned.len(), 1);
        assert!(!owned[0].already_listed);

        let replacement = store
            .share_market_create_listing(
                &owner,
                CreateListingRequest {
                    share_id: "share-relist".into(),
                    seats: vec![free_seat()],
                },
            )
            .await
            .expect("relist after close with no active rentals");
        assert_ne!(replacement, listing_id);

        let owned_after = store
            .share_market_owned_shares(&owner)
            .await
            .expect("owned shares after relist");
        assert!(owned_after[0].already_listed);
        assert!(matches!(
            store
                .share_market_create_listing(
                    &owner,
                    CreateListingRequest {
                        share_id: "share-relist".into(),
                        seats: vec![free_seat()],
                    },
                )
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn closed_listing_with_active_rental_blocks_relist() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-hold", "owner-hold@example.com");
        let renter = session("renter-hold", "renter-hold@example.com");
        insert_share(
            &store,
            "share-hold",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, seat_id) =
            create_listing(&store, &owner, "share-hold", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent seat");
        activate_subscription(&store, &subscription_id, Utc::now()).await;
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("close occupied listing");

        let owned = store
            .share_market_owned_shares(&owner)
            .await
            .expect("owned shares while rental active");
        assert!(owned[0].already_listed);
        assert!(matches!(
            store
                .share_market_create_listing(
                    &owner,
                    CreateListingRequest {
                        share_id: "share-hold".into(),
                        seats: vec![free_seat()],
                    },
                )
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn catalog_can_rent_respects_block_existing_rental_and_direct_grant() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-canrent", "owner-canrent@example.com");
        let renter = session("renter-canrent", "renter-canrent@example.com");
        let blocked = session("blocked-canrent", "blocked-canrent@example.com");
        let granted = session("granted-canrent", "granted-canrent@example.com");
        insert_share(
            &store,
            "share-canrent-a",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        insert_share(
            &store,
            "share-canrent-b",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_a, seat_a) =
            create_listing(&store, &owner, "share-canrent-a", free_seat()).await;
        create_listing(&store, &owner, "share-canrent-b", free_seat()).await;

        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_a, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent first share");
        activate_subscription(&store, &subscription_id, Utc::now()).await;

        store
            .conn
            .lock()
            .await
            .execute(
                "INSERT INTO share_market_owner_blocks (
                    owner_user_id, blocked_user_id, blocked_email, reason, created_at, lifted_at
                 ) VALUES (?1, ?2, ?3, 'test_block', ?4, NULL)",
                params![
                    owner.user_id,
                    blocked.user_id,
                    blocked.email,
                    Utc::now().to_rfc3339()
                ],
            )
            .expect("insert owner block");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET user_grants_json = ?2 WHERE share_id = ?1",
                params![
                    "share-canrent-b",
                    serde_json::to_string(&BTreeMap::from([(
                        granted.email.clone(),
                        ShareUserGrant {
                            email: granted.email.clone(),
                            role: "shareto".into(),
                            active: true,
                            policy: ShareUserPolicy::default(),
                            usage: Default::default(),
                            created_at_ms: 1,
                            updated_at_ms: 1,
                            revoked_at_ms: None,
                            revision: 1,
                            manager: ShareGrantManager::Manual,
                            entitlement_id: None,
                        },
                    )]))
                    .expect("encode grant")
                ],
            )
            .expect("seed direct grant");

        let renter_catalog = store
            .share_market_catalog(Some(&renter), &[])
            .await
            .expect("renter catalog");
        assert!(!renter_catalog
            .listings
            .iter()
            .find(|listing| listing.share_id == "share-canrent-a")
            .expect("listing a")
            .seats[0]
            .can_rent);
        assert!(renter_catalog
            .listings
            .iter()
            .find(|listing| listing.share_id == "share-canrent-b")
            .expect("listing b")
            .seats[0]
            .can_rent);

        let blocked_catalog = store
            .share_market_catalog(Some(&blocked), &[])
            .await
            .expect("blocked catalog");
        assert!(!blocked_catalog
            .listings
            .iter()
            .find(|listing| listing.share_id == "share-canrent-b")
            .expect("listing b for blocked")
            .seats[0]
            .can_rent);

        let granted_catalog = store
            .share_market_catalog(Some(&granted), &[])
            .await
            .expect("granted catalog");
        assert!(!granted_catalog
            .listings
            .iter()
            .find(|listing| listing.share_id == "share-canrent-b")
            .expect("listing b for granted")
            .seats[0]
            .can_rent);
    }
}
