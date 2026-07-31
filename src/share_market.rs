use std::collections::{BTreeMap, HashSet};
use std::time::Duration as StdDuration;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
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

const TRIAL_HOURS: i64 = crate::market_billing::TRIAL_SECONDS / 3_600;
const SERVICE_CYCLE_SECS: u64 = 5;
const MAX_SEATS_PER_LISTING: usize = 20;
const MAX_FREE_DURATION_DAYS: u32 = 365;
const MAX_CONTROL_ATTEMPTS: i64 = 8;
const CONTROL_DISPATCH_WAKE_RETRY_SECS: i64 = 30;
pub(crate) const SHARE_MARKET_CONTROL_ACTOR_EMAIL: &str = "share-market@router.internal";

const SEAT_AVAILABLE: &str = "available";
const SEAT_DISABLED: &str = "disabled";
const SEAT_DELETED: &str = "deleted";
const SEAT_RETIRED_VIEW: &str = "retired";

const SUB_GRANT_PENDING: &str = "grant_pending";
const SUB_ACTIVE_POSTPAID: &str = "active_postpaid";
const SUB_REVOKE_PENDING: &str = "revoke_pending";
const SUB_REVOKE_FAILED: &str = "revoke_failed";
const SUB_GRANT_FAILED: &str = "grant_failed";
const SUB_RELEASED: &str = "released";
const SUB_BILLING_SUSPEND_PENDING: &str = "billing_suspend_pending";
const SUB_BILLING_SUSPENDED: &str = "billing_suspended";
const SUB_BILLING_RESUME_PENDING: &str = "billing_resume_pending";
const SUB_BILLING_CONTROL_FAILED: &str = "billing_control_failed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketCatalog {
    pub listings: Vec<ListingView>,
    pub my_subscriptions: Vec<SubscriptionView>,
    pub trial_hours: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingView {
    pub id: String,
    pub share_id: String,
    pub installation_id: String,
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
    #[serde(default)]
    pub payment_method_kinds: Vec<String>,
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
    pub daily_rate_minor: Option<i64>,
    pub currency: Option<String>,
    pub free_duration_days: Option<u32>,
    pub offer_revision: i64,
    pub is_free: bool,
    pub can_rent: bool,
    pub seller_approval_required: bool,
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
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
    pub installation_id: String,
    pub share_name: String,
    pub app_type: String,
    pub subdomain: String,
    pub share_online: bool,
    pub owner_email: String,
    pub renter_email: String,
    pub status: String,
    pub daily_rate_minor: Option<i64>,
    pub currency: Option<String>,
    pub free_duration_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub offer_revision: i64,
    pub payment_method_kinds: Vec<String>,
    #[serde(default)]
    pub contacts: Vec<PaymentContact>,
    pub can_release: bool,
    pub can_force_revoke: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
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
    pub daily_rate_minor: Option<i64>,
    pub currency: Option<String>,
    #[serde(default)]
    pub free_duration_days: Option<u32>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForceRevokeRequest {
    #[serde(default)]
    pub deny_future_access: bool,
}

#[derive(Debug, Clone)]
struct NormalizedSeat {
    parallel_limit: Option<u32>,
    token_limit: Option<u64>,
    token_period: ShareTokenPeriod,
    daily_rate_minor: Option<i64>,
    currency: Option<String>,
    free_duration_days: Option<u32>,
}

impl NormalizedSeat {
    fn is_free(&self) -> bool {
        self.daily_rate_minor.is_none()
    }
}

pub fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "        CREATE TABLE IF NOT EXISTS share_market_listings (
            id TEXT PRIMARY KEY,
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            owner_user_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'closed')),
            deleted_at TEXT,
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
            daily_rate_minor INTEGER,
            currency TEXT,
            free_duration_days INTEGER,
            offer_revision INTEGER NOT NULL DEFAULT 1,
            current_subscription_id TEXT,
            retired_subscription_id TEXT,
            retired_at TEXT,
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
            installation_id TEXT NOT NULL,
            entitlement_id TEXT NOT NULL UNIQUE,
            owner_user_id TEXT NOT NULL,
            owner_email TEXT NOT NULL,
            renter_user_id TEXT NOT NULL,
            renter_email TEXT NOT NULL,
            status TEXT NOT NULL,
            parallel_limit INTEGER,
            token_limit INTEGER,
            token_period_json TEXT NOT NULL,
            daily_rate_minor INTEGER,
            currency TEXT,
            free_duration_days INTEGER,
            offer_revision INTEGER NOT NULL,
            release_reason TEXT,
            activated_at TEXT,
            expires_at TEXT,
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
            share_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            listing_id TEXT,
            seat_id TEXT,
            subscription_id TEXT,
            actor_user_id TEXT,
            actor_email TEXT,
            event_type TEXT NOT NULL,
            dedupe_key TEXT NOT NULL UNIQUE,
            detail_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_share_market_subscriptions_owner
            ON share_market_subscriptions(owner_user_id, status);
        CREATE INDEX IF NOT EXISTS idx_share_market_subscriptions_renter
            ON share_market_subscriptions(renter_user_id, status);",
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_share_market_seats_listing_lifecycle
             ON share_market_seats(listing_id, retired_at, position);",
    )?;
    crate::market_billing::init_schema(conn)?;
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
        .route("/v1/share-market/listings/:id/delete", post(delete_listing))
        .route("/v1/share-market/listings/:id/seats", post(add_seat))
        .route(
            "/v1/share-market/seats/:id",
            patch(update_seat).delete(delete_seat),
        )
        .route("/v1/share-market/seats/:id/rent", post(rent_seat))
        .route(
            "/v1/share-market/subscriptions/:id/release",
            post(release_subscription),
        )
        .route(
            "/v1/share-market/subscriptions/:id/force-revoke",
            post(force_revoke_subscription),
        )
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
    let daily_rate_minor = input.daily_rate_minor;
    if input
        .free_duration_days
        .is_some_and(|days| !(1..=MAX_FREE_DURATION_DAYS).contains(&days))
    {
        return Err(AppError::BadRequest(format!(
            "freeDurationDays must be between 1 and {MAX_FREE_DURATION_DAYS}, or null for permanent"
        )));
    }
    let currency = input
        .currency
        .map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| !value.is_empty());
    let pricing_empty = daily_rate_minor.is_none() && currency.is_none();
    if pricing_empty {
        return Ok(NormalizedSeat {
            parallel_limit: input.parallel_limit,
            token_limit: input.token_limit,
            token_period: input.token_period,
            daily_rate_minor: None,
            currency: None,
            free_duration_days: input.free_duration_days,
        });
    }
    if input.free_duration_days.is_some() {
        return Err(AppError::BadRequest(
            "paid Share seats cannot set freeDurationDays".into(),
        ));
    }
    let daily_rate_minor = daily_rate_minor.ok_or_else(|| {
        AppError::BadRequest("daily price and currency must both be set or both be empty".into())
    })?;
    if daily_rate_minor <= 0 || daily_rate_minor > crate::market_billing::MAX_DAILY_RATE_MINOR {
        return Err(AppError::BadRequest(
            "paid seat daily price is outside the supported range".into(),
        ));
    }
    let currency =
        currency.ok_or_else(|| AppError::BadRequest("paid seat currency is required".into()))?;
    if currency != "CNY" && currency != "USD" {
        return Err(AppError::BadRequest("currency must be CNY or USD".into()));
    }
    Ok(NormalizedSeat {
        parallel_limit: input.parallel_limit,
        token_limit: input.token_limit,
        token_period: input.token_period,
        daily_rate_minor: Some(daily_rate_minor),
        currency: Some(currency),
        free_duration_days: None,
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

fn token_period_anchor_at_ms(period: ShareTokenPeriod, now: DateTime<Utc>) -> Option<i64> {
    matches!(
        period,
        ShareTokenPeriod::SevenDays | ShareTokenPeriod::ThirtyDays
    )
    .then(|| now.timestamp_millis().div_euclid(60_000) * 60_000)
}

fn share_event_summary(event_type: &str, share_name: &str) -> String {
    format!("{share_name}: {}", event_type.replace('_', " "))
}

#[derive(Debug)]
struct ShareEventTarget {
    installation_id: String,
    share_name: String,
    app_type: String,
    subdomain: String,
    owner_email: String,
    owner_user_id: Option<String>,
}

#[derive(Debug)]
struct ShareSubscriptionEventSnapshot {
    owner_user_id: String,
    renter_user_id: String,
    fields: serde_json::Map<String, serde_json::Value>,
}

fn parse_event_json(value: &str, field: &str) -> Result<serde_json::Value, AppError> {
    serde_json::from_str(value)
        .map_err(|error| AppError::Internal(format!("parse {field} failed: {error}")))
}

fn share_seat_event_snapshot_tx(
    conn: &Connection,
    seat_id: &str,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>, AppError> {
    let row = conn
        .query_row(
            "SELECT position, status, parallel_limit, token_limit, token_period_json,
                    daily_rate_minor, currency, free_duration_days, offer_revision, retired_at
             FROM share_market_seats WHERE id = ?1",
            params![seat_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("resolve Share Market seat event snapshot"))?;
    row.map(
        |(
            position,
            status,
            parallel_limit,
            token_limit,
            token_period,
            daily_rate_minor,
            currency,
            free_duration_days,
            offer_revision,
            retired_at,
        )| {
            let value = serde_json::json!({
                "seatPosition": position,
                "seatStatus": status,
                "parallelLimit": parallel_limit,
                "tokenLimit": token_limit,
                "tokenPeriod": parse_event_json(&token_period, "Share seat token period")?,
                "dailyRateMinor": daily_rate_minor,
                "currency": currency,
                "freeDurationDays": free_duration_days,
                "offerRevision": offer_revision,
                "retiredAt": retired_at,
            });
            Ok(value
                .as_object()
                .expect("Share seat event snapshot is an object")
                .clone())
        },
    )
    .transpose()
}

fn share_subscription_event_snapshot_tx(
    conn: &Connection,
    subscription_id: &str,
) -> Result<Option<ShareSubscriptionEventSnapshot>, AppError> {
    let row = conn
        .query_row(
            "SELECT owner_user_id, owner_email, renter_user_id, renter_email, status,
                    parallel_limit, token_limit, token_period_json, daily_rate_minor,
                    currency, free_duration_days, offer_revision, release_reason, created_at,
                    activated_at, expires_at, released_at
             FROM share_market_subscriptions WHERE id = ?1",
            params![subscription_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<String>>(16)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("resolve Share Market subscription event snapshot"))?;
    row.map(
        |(
            owner_user_id,
            owner_email,
            renter_user_id,
            renter_email,
            status,
            parallel_limit,
            token_limit,
            token_period,
            daily_rate_minor,
            currency,
            free_duration_days,
            offer_revision,
            release_reason,
            created_at,
            activated_at,
            expires_at,
            released_at,
        )| {
            let payment = conn
                .query_row(
                    "SELECT methods_json, COALESCE(contacts_json, '[]')
                     FROM account_payment_profiles WHERE user_id = ?1",
                    params![owner_user_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(map_db("resolve Share Market event payment profile"))?;
            let (payment_methods, payment_contacts) = payment
                .map(|(methods, contacts)| {
                    Ok((
                        parse_event_json(&methods, "Share payment methods")?,
                        parse_event_json(&contacts, "Share payment contacts")?,
                    ))
                })
                .transpose()?
                .unwrap_or_else(|| (serde_json::json!([]), serde_json::json!([])));
            let value = serde_json::json!({
                "ownerUserId": owner_user_id,
                "ownerEmail": owner_email,
                "renterUserId": renter_user_id,
                "renterEmail": renter_email,
                "subscriptionStatus": status,
                "parallelLimit": parallel_limit,
                "tokenLimit": token_limit,
                "tokenPeriod": parse_event_json(&token_period, "Share subscription token period")?,
                "dailyRateMinor": daily_rate_minor,
                "currency": currency,
                "freeDurationDays": free_duration_days,
                "offerRevision": offer_revision,
                "releaseReason": release_reason,
                "createdAt": created_at,
                "activatedAt": activated_at,
                "expiresAt": expires_at,
                "releasedAt": released_at,
                "paymentMethods": payment_methods,
                "paymentContacts": payment_contacts,
            });
            Ok(ShareSubscriptionEventSnapshot {
                owner_user_id,
                renter_user_id,
                fields: value
                    .as_object()
                    .expect("Share subscription event snapshot is an object")
                    .clone(),
            })
        },
    )
    .transpose()
}

fn resolve_share_installation_tx(
    conn: &Connection,
    share_id: &str,
    listing_id: Option<&str>,
    subscription_id: Option<&str>,
    event_id: Option<&str>,
) -> Result<String, AppError> {
    let mut installation_id = conn
        .query_row(
            "SELECT installation_id FROM shares WHERE share_id = ?1",
            params![share_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("resolve Share Client"))?;
    if installation_id.is_none()
        && let Some(id) = subscription_id
    {
        installation_id = conn
            .query_row(
                "SELECT installation_id FROM share_market_subscriptions WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("resolve Share subscription Client"))?;
    }
    if installation_id.is_none()
        && let Some(id) = listing_id
    {
        installation_id = conn
            .query_row(
                "SELECT installation_id FROM share_market_listings WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("resolve Share listing Client"))?;
    }
    if installation_id.is_none()
        && let Some(id) = event_id
    {
        installation_id = conn
            .query_row(
                "SELECT installation_id FROM share_market_events WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("resolve Share event Client"))?;
    }
    if installation_id.is_none() {
        installation_id = conn
            .query_row(
                "SELECT installation_id FROM share_market_subscriptions
                 WHERE share_id = ?1 ORDER BY created_at DESC LIMIT 1",
                params![share_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("resolve historical Share subscription Client"))?;
    }
    if installation_id.is_none() {
        installation_id = conn
            .query_row(
                "SELECT installation_id FROM share_market_listings
                 WHERE share_id = ?1 ORDER BY created_at DESC LIMIT 1",
                params![share_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("resolve historical Share listing Client"))?;
    }
    installation_id.ok_or_else(|| {
        AppError::Internal("Share Market event has no Client installation snapshot".into())
    })
}

fn share_event_target_tx(
    conn: &Connection,
    event_id: &str,
    share_id: &str,
    listing_id: Option<&str>,
    subscription_id: Option<&str>,
) -> Result<ShareEventTarget, AppError> {
    if let Some(target) = conn
        .query_row(
            "SELECT s.installation_id,
                    COALESCE(NULLIF(s.share_name, ''), NULLIF(s.subdomain, ''), s.share_id),
                    s.app_type, COALESCE(s.subdomain, ''), lower(trim(s.owner_email)),
                    (SELECT id FROM users WHERE email_normalized = lower(trim(s.owner_email)))
             FROM shares s WHERE s.share_id = ?1",
            params![share_id],
            |row| {
                Ok(ShareEventTarget {
                    installation_id: row.get(0)?,
                    share_name: row.get(1)?,
                    app_type: row.get(2)?,
                    subdomain: row.get(3)?,
                    owner_email: row.get(4)?,
                    owner_user_id: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(map_db("resolve Share Market Client chat target"))?
    {
        return Ok(target);
    }

    let installation_id =
        resolve_share_installation_tx(conn, share_id, listing_id, subscription_id, Some(event_id))?;
    let mut participant = None;
    if let Some(id) = subscription_id {
        participant = conn
            .query_row(
                "SELECT owner_email, owner_user_id FROM share_market_subscriptions WHERE id = ?1",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_db("resolve Share subscription chat participant"))?;
    }
    if participant.is_none()
        && let Some(id) = listing_id
    {
        participant = conn
            .query_row(
                "SELECT owner_email, owner_user_id FROM share_market_listings WHERE id = ?1",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_db("resolve Share listing chat participant"))?;
    }
    if participant.is_none() {
        participant = conn
            .query_row(
                "SELECT owner_email, owner_user_id FROM share_market_subscriptions
                 WHERE share_id = ?1 ORDER BY created_at DESC LIMIT 1",
                params![share_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_db(
                "resolve historical Share subscription chat participant",
            ))?;
    }
    if participant.is_none() {
        participant = conn
            .query_row(
                "SELECT owner_email, owner_user_id FROM share_market_listings
                 WHERE share_id = ?1 ORDER BY created_at DESC LIMIT 1",
                params![share_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_db("resolve historical Share listing chat participant"))?;
    }
    let (owner_email, owner_user_id) = if let Some((email, user_id)) = participant {
        (email, Some(user_id))
    } else {
        conn.query_row(
            "SELECT lower(trim(i.owner_email)),
                    (SELECT id FROM users WHERE email_normalized = lower(trim(i.owner_email)))
             FROM installations i WHERE i.id = ?1",
            params![installation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(map_db("resolve fallback Share Market chat owner"))?
    };
    Ok(ShareEventTarget {
        installation_id,
        share_name: share_id.to_string(),
        app_type: String::new(),
        subdomain: String::new(),
        owner_email,
        owner_user_id,
    })
}

#[allow(clippy::too_many_arguments)]
fn enqueue_share_market_system_event_tx(
    conn: &Connection,
    event_id: &str,
    share_id: &str,
    listing_id: Option<&str>,
    seat_id: Option<&str>,
    subscription_id: Option<&str>,
    actor: Option<&AuthSession>,
    event_type: &str,
    mut detail: serde_json::Value,
    now: &str,
) -> Result<(), AppError> {
    let target = share_event_target_tx(conn, event_id, share_id, listing_id, subscription_id)?;
    if !detail.is_object() {
        return Err(AppError::Internal(
            "Share Market event detail must be an object".into(),
        ));
    }
    let mut followers = Vec::new();
    if let Some(user_id) = target.owner_user_id {
        followers.push(user_id);
    }
    let subscription = subscription_id
        .map(|subscription_id| share_subscription_event_snapshot_tx(conn, subscription_id))
        .transpose()?
        .flatten();
    let object = detail
        .as_object_mut()
        .expect("Share Market event detail checked as object");
    object.insert(
        "summary".into(),
        share_event_summary(event_type, &target.share_name).into(),
    );
    object.insert("marketKind".into(), "share".into());
    object.insert(
        "installationId".into(),
        target.installation_id.clone().into(),
    );
    object.insert("shareId".into(), share_id.into());
    object.insert("shareName".into(), target.share_name.into());
    object.insert("appType".into(), target.app_type.into());
    object.insert("subdomain".into(), target.subdomain.into());
    object.insert("ownerEmail".into(), target.owner_email.into());
    if let Some(value) = listing_id {
        object.insert("listingId".into(), value.into());
    }
    if let Some(value) = seat_id {
        object.insert("seatId".into(), value.into());
        if let Some(snapshot) = share_seat_event_snapshot_tx(conn, value)? {
            object.extend(snapshot);
        }
    }
    if let Some(value) = subscription_id {
        object.insert("subscriptionId".into(), value.into());
    }
    if let Some(subscription) = subscription {
        followers.push(subscription.owner_user_id);
        followers.push(subscription.renter_user_id);
        object.extend(subscription.fields);
    }
    if let Some(actor) = actor {
        followers.push(actor.user_id.clone());
        object.insert("actorUserId".into(), actor.user_id.clone().into());
        object.insert("actorEmail".into(), actor.email.clone().into());
    }
    crate::store::client_chat::enqueue_client_system_event_tx(
        conn,
        &target.installation_id,
        "share_market",
        event_id,
        event_type,
        detail,
        &followers,
        now,
    )
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
    let detail = crate::store::client_chat::sanitize_system_event_payload(detail)?;
    let share_id = if let Some(subscription_id) = subscription_id {
        tx.query_row(
            "SELECT share_id FROM share_market_subscriptions WHERE id = ?1",
            params![subscription_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("resolve Share Market event subscription"))?
    } else if let Some(listing_id) = listing_id {
        tx.query_row(
            "SELECT share_id FROM share_market_listings WHERE id = ?1",
            params![listing_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("resolve Share Market event listing"))?
    } else if let Some(seat_id) = seat_id {
        tx.query_row(
            "SELECT listing.share_id
             FROM share_market_seats seat
             JOIN share_market_listings listing ON listing.id = seat.listing_id
             WHERE seat.id = ?1",
            params![seat_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("resolve Share Market event seat"))?
    } else {
        detail
            .get("shareId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }
    .ok_or_else(|| AppError::Internal("Share Market event has no Share identity".into()))?;
    let installation_id =
        resolve_share_installation_tx(tx, &share_id, listing_id, subscription_id, None)?;
    let event_id = Uuid::new_v4().to_string();
    let dedupe_key = format!("share-market:{event_id}");
    tx.execute(
        "INSERT INTO share_market_events (
            id, share_id, installation_id, listing_id, seat_id, subscription_id,
            actor_user_id, actor_email, event_type, dedupe_key, detail_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            event_id,
            share_id,
            installation_id,
            listing_id,
            seat_id,
            subscription_id,
            actor.map(|value| value.user_id.as_str()),
            actor.map(|value| value.email.as_str()),
            event_type,
            dedupe_key,
            detail.to_string(),
            now,
        ],
    )
    .map_err(map_db("record Share Market event"))?;
    enqueue_share_market_system_event_tx(
        tx,
        &event_id,
        &share_id,
        listing_id,
        seat_id,
        subscription_id,
        actor,
        event_type,
        detail,
        now,
    )?;
    Ok(())
}

pub(crate) fn enqueue_share_lifecycle_event_tx(
    conn: &Connection,
    share_id: &str,
    event_type: &str,
    detail: serde_json::Value,
    dedupe_key: &str,
    now: DateTime<Utc>,
) -> Result<String, AppError> {
    let detail = crate::store::client_chat::sanitize_system_event_payload(detail)?;
    let event_id = Uuid::new_v4().to_string();
    let installation_id = resolve_share_installation_tx(conn, share_id, None, None, None)?;
    conn.execute(
        "INSERT OR IGNORE INTO share_market_events (
            id, share_id, installation_id, listing_id, seat_id, subscription_id,
            actor_user_id, actor_email, event_type, dedupe_key, detail_json, created_at
         ) VALUES (?1, ?2, ?3, NULL, NULL, NULL, NULL, NULL, ?4, ?5, ?6, ?7)",
        params![
            event_id,
            share_id,
            installation_id,
            event_type,
            dedupe_key,
            detail.to_string(),
            now.to_rfc3339(),
        ],
    )
    .map_err(map_db("record Share lifecycle event"))?;
    let stored_event_id = conn
        .query_row(
            "SELECT id FROM share_market_events WHERE dedupe_key = ?1",
            params![dedupe_key],
            |row| row.get::<_, String>(0),
        )
        .map_err(map_db("read Share lifecycle event"))?;
    enqueue_share_market_system_event_tx(
        conn,
        &stored_event_id,
        share_id,
        None,
        None,
        None,
        None,
        event_type,
        detail,
        &now.to_rfc3339(),
    )?;
    Ok(stored_event_id)
}

pub(crate) fn enqueue_subscription_lifecycle_event_tx(
    conn: &Connection,
    subscription_id: &str,
    event_type: &str,
    detail: serde_json::Value,
    dedupe_key: &str,
    now: &str,
) -> Result<String, AppError> {
    let detail = crate::store::client_chat::sanitize_system_event_payload(detail)?;
    let share_id = conn
        .query_row(
            "SELECT share_id FROM share_market_subscriptions WHERE id = ?1",
            params![subscription_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(map_db("read Share lifecycle subscription"))?;
    let installation_id =
        resolve_share_installation_tx(conn, &share_id, None, Some(subscription_id), None)?;
    let event_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO share_market_events (
            id, share_id, installation_id, listing_id, seat_id, subscription_id,
            actor_user_id, actor_email, event_type, dedupe_key, detail_json, created_at
         ) VALUES (?1, ?2, ?3, NULL, NULL, ?4, NULL, NULL, ?5, ?6, ?7, ?8)",
        params![
            event_id,
            share_id,
            installation_id,
            subscription_id,
            event_type,
            dedupe_key,
            detail.to_string(),
            now,
        ],
    )
    .map_err(map_db("record subscription chat event"))?;
    let stored_event_id = conn
        .query_row(
            "SELECT id FROM share_market_events WHERE dedupe_key = ?1",
            params![dedupe_key],
            |row| row.get::<_, String>(0),
        )
        .map_err(map_db("read subscription chat event"))?;
    enqueue_share_market_system_event_tx(
        conn,
        &stored_event_id,
        &share_id,
        None,
        None,
        Some(subscription_id),
        None,
        event_type,
        detail,
        now,
    )?;
    Ok(stored_event_id)
}

#[derive(Debug, Clone)]
struct SubscriptionRecord {
    id: String,
    seat_id: String,
    listing_id: String,
    share_id: String,
    installation_id: String,
    share_name: String,
    app_type: String,
    subdomain: String,
    entitlement_id: String,
    owner_user_id: String,
    owner_email: String,
    renter_user_id: String,
    renter_email: String,
    status: String,
    daily_rate_minor: Option<i64>,
    currency: Option<String>,
    free_duration_days: Option<u32>,
    offer_revision: i64,
    release_reason: Option<String>,
    activated_at: Option<String>,
    expires_at: Option<String>,
    released_at: Option<String>,
    created_at: String,
    updated_at: String,
}

fn subscription_record(
    conn: &Connection,
    subscription_id: &str,
) -> Result<Option<SubscriptionRecord>, AppError> {
    conn.query_row(
        "SELECT sub.id, sub.seat_id, sub.listing_id, sub.share_id, sub.installation_id,
                COALESCE(s.share_name, sub.share_id), COALESCE(s.app_type, ''),
                COALESCE(s.subdomain, ''),
                sub.entitlement_id, sub.owner_user_id, sub.owner_email,
                sub.renter_user_id, sub.renter_email, sub.status,
                sub.daily_rate_minor, sub.currency, sub.free_duration_days,
                sub.offer_revision, sub.release_reason, sub.activated_at, sub.expires_at,
                sub.released_at,
                sub.created_at, sub.updated_at
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
                installation_id: row.get(4)?,
                share_name: row.get(5)?,
                app_type: row.get(6)?,
                subdomain: row.get(7)?,
                entitlement_id: row.get(8)?,
                owner_user_id: row.get(9)?,
                owner_email: row.get(10)?,
                renter_user_id: row.get(11)?,
                renter_email: row.get(12)?,
                status: row.get(13)?,
                daily_rate_minor: row.get(14)?,
                currency: row.get(15)?,
                free_duration_days: row
                    .get::<_, Option<i64>>(16)?
                    .and_then(|value| u32::try_from(value).ok()),
                offer_revision: row.get(17)?,
                release_reason: row.get(18)?,
                activated_at: row.get(19)?,
                expires_at: row.get(20)?,
                released_at: row.get(21)?,
                created_at: row.get(22)?,
                updated_at: row.get(23)?,
            })
        },
    )
    .optional()
    .map_err(map_db("read Share Market subscription"))
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

fn payment_method_kinds(methods: &[PaymentMethod]) -> Vec<String> {
    let mut kinds = methods
        .iter()
        .map(|method| method.kind.clone())
        .collect::<Vec<_>>();
    kinds.sort();
    kinds.dedup();
    kinds
}

fn subscription_view(
    conn: &Connection,
    record: SubscriptionRecord,
    viewer: Option<&AuthSession>,
    active_subdomains: &[String],
) -> Result<SubscriptionView, AppError> {
    let is_renter = viewer.is_some_and(|session| session.user_id == record.renter_user_id);
    let is_owner = viewer.is_some_and(|session| session.user_id == record.owner_user_id);
    let (payment_method_kinds, contacts) = if is_renter || is_owner {
        payment_profile(conn, &record.owner_user_id)?
            .map(|(methods, contacts, _)| (payment_method_kinds(&methods), contacts))
            .unwrap_or_default()
    } else {
        (Vec::new(), Vec::new())
    };
    let can_release = is_renter
        && !matches!(
            record.status.as_str(),
            SUB_RELEASED | SUB_GRANT_FAILED | SUB_REVOKE_PENDING
        );
    // Allow retry while revoke is stuck (e.g. earlier grant edit blocked dispatch).
    let can_force_revoke =
        is_owner && !matches!(record.status.as_str(), SUB_RELEASED | SUB_GRANT_FAILED);
    let share_online =
        !record.subdomain.is_empty() && active_subdomains.contains(&record.subdomain);
    Ok(SubscriptionView {
        id: record.id,
        seat_id: record.seat_id,
        listing_id: record.listing_id,
        share_id: record.share_id,
        installation_id: record.installation_id,
        share_name: record.share_name,
        app_type: record.app_type,
        subdomain: record.subdomain,
        share_online,
        owner_email: record.owner_email,
        renter_email: record.renter_email,
        status: record.status,
        daily_rate_minor: record.daily_rate_minor,
        currency: record.currency,
        free_duration_days: record.free_duration_days,
        activated_at: record.activated_at,
        expires_at: record.expires_at,
        offer_revision: record.offer_revision,
        payment_method_kinds,
        contacts,
        can_release,
        can_force_revoke,
        release_reason: record.release_reason,
        released_at: record.released_at,
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
                              AND listing.deleted_at IS NULL
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
                        COALESCE(s.tokens_used, 0), listing.installation_id
                 FROM share_market_listings listing
                 LEFT JOIN shares s ON s.share_id = listing.share_id
                 WHERE listing.deleted_at IS NULL
                   AND ((listing.status = 'active'
                        AND lower(COALESCE(s.owner_email, '')) = lower(listing.owner_email))
                    OR listing.owner_user_id = ?1
                    OR EXISTS (
                        SELECT 1 FROM share_market_subscriptions sub
                        WHERE sub.listing_id = listing.id AND sub.renter_user_id = ?1
                    ))
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
                    row.get::<_, String>(18)?,
                ))
            })
            .map_err(map_db("query Share Market catalog"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read Share Market catalog"))?;
        drop(listings_statement);

        let mut listings = Vec::with_capacity(listing_rows.len());
        let mut paid_credit_allowed_by_supplier_currency = BTreeMap::new();
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
            installation_id,
        ) in listing_rows
        {
            let is_owner = viewer.is_some_and(|value| value.user_id == owner_user_id);
            let share_online = !subdomain.is_empty() && active_subdomains.contains(&subdomain);
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
                            token_period_json, daily_rate_minor, currency, free_duration_days,
                            offer_revision, current_subscription_id,
                            retired_subscription_id, retired_at
                     FROM share_market_seats
                     WHERE listing_id = ?1 AND status != 'deleted'
                     ORDER BY CASE WHEN retired_at IS NULL THEN 0 ELSE 1 END, position",
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
                        row.get::<_, Option<i64>>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
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
                daily_rate_minor,
                currency,
                free_duration_days,
                offer_revision,
                current_subscription_id,
                retired_subscription_id,
                retired_at,
            ) in seat_rows
            {
                let viewer_has_access = if let Some(session) = viewer {
                    crate::market_access::product_access_allowed_tx(
                        &conn,
                        &owner_user_id,
                        &session.user_id,
                        &session.email,
                        crate::market_access::PRODUCT_SHARE,
                        crate::market_access::pricing_kind_for_rate(daily_rate_minor),
                    )?
                } else {
                    false
                };
                let subscription = match current_subscription_id.or(retired_subscription_id) {
                    Some(subscription_id) => subscription_record(&conn, &subscription_id)?
                        .map(|record| subscription_view(&conn, record, viewer, active_subdomains))
                        .transpose()?,
                    None => None,
                };
                let base_rent_prerequisites = viewer.is_some_and(|session| {
                    status == "active"
                        && share_status == "active"
                        && share_online
                        && current_owner_email.eq_ignore_ascii_case(&owner_email)
                        && seat_status == SEAT_AVAILABLE
                        && retired_at.is_none()
                        && session.user_id != owner_user_id
                        && !viewer_already_renting
                        && !viewer_has_direct_grant
                });
                let seller_approval_required = base_rent_prerequisites && !viewer_has_access;
                let base_can_rent = base_rent_prerequisites && viewer_has_access;
                let paid_credit_allowed = if daily_rate_minor.is_none() {
                    true
                } else if !base_can_rent {
                    false
                } else if let (Some(session), Some(currency)) = (viewer, currency.as_deref()) {
                    let key = (owner_user_id.clone(), currency.to_string());
                    if let Some(allowed) = paid_credit_allowed_by_supplier_currency.get(&key) {
                        *allowed
                    } else {
                        let allowed = crate::market_billing::credit_allowed_tx(
                            &conn,
                            &session.user_id,
                            &session.email,
                            &owner_user_id,
                            crate::market_access::PRODUCT_SHARE,
                            currency,
                        )?;
                        paid_credit_allowed_by_supplier_currency.insert(key, allowed);
                        allowed
                    }
                } else {
                    false
                };
                let can_rent = base_can_rent && paid_credit_allowed;
                let read_only = retired_at.is_some();
                seats.push(SeatView {
                    id: seat_id,
                    position,
                    status: if read_only {
                        SEAT_RETIRED_VIEW.to_string()
                    } else {
                        seat_status
                    },
                    parallel_limit: parallel_limit.and_then(|value| u32::try_from(value).ok()),
                    token_limit: token_limit.and_then(|value| u64::try_from(value).ok()),
                    token_period: serde_json::from_str(&token_period_json)
                        .unwrap_or(ShareTokenPeriod::Lifetime),
                    daily_rate_minor,
                    currency,
                    free_duration_days: free_duration_days
                        .and_then(|value| u32::try_from(value).ok()),
                    offer_revision,
                    is_free: daily_rate_minor.is_none(),
                    can_rent,
                    seller_approval_required,
                    read_only,
                    retired_at,
                    subscription,
                });
            }
            let (payment_method_kinds, contacts) = payment_profile(&conn, &owner_user_id)?
                .map(|(methods, contacts, _)| (payment_method_kinds(&methods), contacts))
                .unwrap_or_default();
            listings.push(ListingView {
                id,
                share_id,
                installation_id,
                share_name,
                app_type,
                owner_email,
                status,
                share_status,
                subdomain: subdomain.clone(),
                share_online,
                is_owner,
                contacts,
                payment_method_kinds,
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
        if let Some(viewer) = viewer {
            let mut statement = conn
                .prepare(
                    "SELECT id FROM share_market_subscriptions
                     WHERE renter_user_id = ?1
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
        }
        Ok(ShareMarketCatalog {
            listings,
            my_subscriptions,
            trial_hours: TRIAL_HOURS,
        })
    }
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
            token_period_json, daily_rate_minor, currency, free_duration_days,
            offer_revision, current_subscription_id, created_at, updated_at
         ) VALUES (?1, ?2, ?3, 'available', ?4, ?5, ?6, ?7, ?8, ?9, 1, NULL, ?10, ?10)",
        params![
            id,
            listing_id,
            position,
            seat.parallel_limit.map(i64::from),
            seat.token_limit.and_then(|value| i64::try_from(value).ok()),
            token_period_json,
            seat.daily_rate_minor,
            seat.currency,
            seat.free_duration_days.map(i64::from),
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
        let share: Option<(String, String, String, String)> = tx
            .query_row(
                "SELECT owner_email, share_status,
                        COALESCE(supported_user_token_periods_json, '[]'), installation_id
                 FROM shares WHERE share_id = ?1",
                params![share_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(map_db("read listing Share"))?;
        let Some((owner_email, share_status, periods_json, installation_id)) = share else {
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
            for currency in seats
                .iter()
                .filter_map(|seat| seat.currency.as_deref())
                .collect::<std::collections::BTreeSet<_>>()
            {
                crate::market_billing::require_supplier_profile_tx(
                    &tx,
                    &session.user_id,
                    currency,
                )?;
            }
        }
        close_reclaimable_stale_listings_tx(&tx, share_id, &session.email, &now)?;
        let active_listing_exists = tx
            .query_row(
                "SELECT 1 FROM share_market_listings
                 WHERE share_id = ?1 AND status = 'active' AND deleted_at IS NULL LIMIT 1",
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
                id, share_id, installation_id, owner_user_id, owner_email,
                status, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6)",
            params![
                listing_id,
                share_id,
                installation_id,
                session.user_id,
                session.email,
                now
            ],
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
        let owner: Option<(String, String, String, String, String, String, String)> = tx
            .query_row(
                "SELECT listing.owner_user_id, listing.owner_email, s.owner_email,
                        s.share_status,
                        COALESCE(s.supported_user_token_periods_json, '[]'), listing.share_id,
                        listing.status
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
                        row.get(6)?,
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
            listing_status,
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
                 WHERE listing_id = ?1
                   AND retired_at IS NULL
                   AND status IN ('available', 'reserved', 'occupied', 'revoking')",
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
            crate::market_billing::require_supplier_profile_tx(
                &tx,
                &session.user_id,
                seat.currency
                    .as_deref()
                    .ok_or_else(|| AppError::Internal("paid Share currency is missing".into()))?,
            )?;
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
        if listing_status != "active" {
            event_tx(
                &tx,
                Some(listing_id),
                None,
                None,
                Some(session),
                "listing_relisted",
                serde_json::json!({}),
                &now,
            )?;
        }
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
        let row: Option<(String, String, i64, String, String, String, Option<String>)> = tx
            .query_row(
                "SELECT listing.owner_user_id, seat.status, seat.offer_revision,
                        listing.owner_email, s.owner_email,
                        COALESCE(s.supported_user_token_periods_json, '[]'), seat.retired_at
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
                        row.get(6)?,
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
            retired_at,
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
        if status != SEAT_AVAILABLE || retired_at.is_some() {
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
            crate::market_billing::require_supplier_profile_tx(
                &tx,
                &session.user_id,
                seat.currency
                    .as_deref()
                    .ok_or_else(|| AppError::Internal("paid Share currency is missing".into()))?,
            )?;
        }
        let token_period_json = serde_json::to_string(&seat.token_period)
            .map_err(|error| AppError::Internal(format!("encode token period failed: {error}")))?;
        tx.execute(
            "UPDATE share_market_seats
             SET parallel_limit = ?2, token_limit = ?3, token_period_json = ?4,
                 daily_rate_minor = ?5, currency = ?6, free_duration_days = ?7,
                 offer_revision = offer_revision + 1, updated_at = ?8
             WHERE id = ?1 AND status = 'available' AND offer_revision = ?9",
            params![
                seat_id,
                seat.parallel_limit.map(i64::from),
                seat.token_limit.and_then(|value| i64::try_from(value).ok()),
                token_period_json,
                seat.daily_rate_minor,
                seat.currency,
                seat.free_duration_days.map(i64::from),
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
        let row: Option<(String, String, String, Option<String>, i64)> = tx
            .query_row(
                "SELECT listing.owner_user_id, seat.status, seat.listing_id, seat.retired_at,
                        (SELECT COUNT(*) FROM share_market_subscriptions sub WHERE sub.seat_id = seat.id)
                 FROM share_market_seats seat
                 JOIN share_market_listings listing ON listing.id = seat.listing_id
                 WHERE seat.id = ?1",
                params![seat_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()
            .map_err(map_db("read Share seat for delete"))?;
        let Some((owner_user_id, status, listing_id, retired_at, subscription_count)) = row else {
            return Err(AppError::NotFound("seat not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only listing owner can delete seat".into(),
            ));
        }
        if retired_at.is_some() || subscription_count > 0 {
            return Err(AppError::Conflict(
                "a seat with rental history is read-only".into(),
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
                "SELECT owner_user_id FROM share_market_listings
                 WHERE id = ?1 AND deleted_at IS NULL",
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

    pub async fn share_market_delete_listing(
        &self,
        session: &AuthSession,
        listing_id: &str,
    ) -> Result<(), AppError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin delete Share listing"))?;
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT owner_user_id, status FROM share_market_listings
                 WHERE id = ?1 AND deleted_at IS NULL",
                params![listing_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_db("read Share listing for delete"))?;
        let Some((owner_user_id, status)) = row else {
            return Err(AppError::NotFound("listing not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only listing owner can delete listing".into(),
            ));
        }
        if status != "closed" {
            return Err(AppError::BadRequest(
                "only closed listings can be deleted".into(),
            ));
        }
        let rental_history: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM share_market_subscriptions
                 WHERE listing_id = ?1",
                params![listing_id],
                |row| row.get(0),
            )
            .map_err(map_db("count active Share rentals before delete"))?;
        if rental_history > 0 {
            return Err(AppError::Conflict(
                "cannot delete a listing after a seat has been rented".into(),
            ));
        }
        tx.execute(
            "UPDATE share_market_seats SET status = 'deleted', updated_at = ?2
             WHERE listing_id = ?1 AND status != 'deleted'",
            params![listing_id, now],
        )
        .map_err(map_db("delete seats for Share listing"))?;
        tx.execute(
            "UPDATE share_market_listings
             SET deleted_at = ?2, updated_at = ?2
             WHERE id = ?1 AND deleted_at IS NULL",
            params![listing_id, now],
        )
        .map_err(map_db("soft-delete Share listing"))?;
        event_tx(
            &tx,
            Some(listing_id),
            None,
            None,
            Some(session),
            "listing_deleted",
            serde_json::json!({}),
            &now,
        )?;
        tx.commit().map_err(map_db("commit delete Share listing"))?;
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

async fn delete_listing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(listing_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_delete_listing(&session, &listing_id)
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
            Option<i64>,
            String,
            String,
            String,
        )> = tx
            .query_row(
                "SELECT seat.listing_id, listing.share_id, listing.owner_user_id,
                        listing.owner_email, listing.status, seat.status, seat.offer_revision,
                        seat.parallel_limit, seat.token_limit, seat.token_period_json,
                        seat.daily_rate_minor, seat.currency, seat.free_duration_days,
                        COALESCE(s.user_grants_json, '{}'),
                        COALESCE(s.share_name, listing.share_id), listing.installation_id
                 FROM share_market_seats seat
                 JOIN share_market_listings listing ON listing.id = seat.listing_id
                 JOIN shares s ON s.share_id = listing.share_id
                 WHERE seat.id = ?1 AND s.share_status = 'active'
                   AND seat.retired_at IS NULL
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
                        row.get(15)?,
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
            daily_rate_minor,
            currency,
            free_duration_days,
            grants_json,
            share_name,
            installation_id,
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
        crate::market_access::ensure_product_access_tx(
            &tx,
            &owner_user_id,
            &session.user_id,
            &session.email,
            crate::market_access::PRODUCT_SHARE,
            crate::market_access::pricing_kind_for_rate(daily_rate_minor),
        )?;
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
        if daily_rate_minor.is_some() {
            ensure_payment_profile_tx(&tx, &owner_user_id)?;
            let currency = currency
                .as_deref()
                .ok_or_else(|| AppError::Internal("paid Share currency is missing".into()))?;
            crate::market_billing::ensure_credit_allowed_tx(
                &tx,
                &session.user_id,
                &session.email,
                &owner_user_id,
                crate::market_access::PRODUCT_SHARE,
                currency,
            )?;
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
                id, seat_id, listing_id, share_id, installation_id, entitlement_id,
                owner_user_id, owner_email, renter_user_id, renter_email, status,
                parallel_limit, token_limit, token_period_json, daily_rate_minor, currency,
                free_duration_days, offer_revision, release_reason,
                activated_at, expires_at, created_at, updated_at, released_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'grant_pending',
                       ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                       NULL, NULL, NULL, ?18, ?18, NULL)",
            params![
                subscription_id,
                seat_id,
                listing_id,
                share_id,
                installation_id,
                entitlement_id,
                owner_user_id,
                owner_email,
                session.user_id,
                session.email.to_ascii_lowercase(),
                parallel_limit,
                token_limit,
                token_period_json,
                daily_rate_minor,
                currency,
                free_duration_days,
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
        if let Some(daily_rate_minor) = daily_rate_minor {
            let currency = currency
                .as_deref()
                .ok_or_else(|| AppError::Internal("paid Share currency is missing".into()))?;
            crate::market_billing::activate_contract_tx(
                &tx,
                crate::market_billing::ActivateContractInput {
                    product_kind: "share",
                    product_ref: &subscription_id,
                    service_ref: &share_id,
                    service_label: &share_name,
                    buyer_user_id: &session.user_id,
                    buyer_email: &session.email,
                    supplier_user_id: &owner_user_id,
                    supplier_email: &owner_email,
                    currency,
                    daily_rate_minor,
                    offer_revision,
                    replacement_of: None,
                },
                &now,
            )?;
        }
        let changed = tx
            .execute(
                "UPDATE share_market_seats
                 SET status = 'reserved', current_subscription_id = ?2, updated_at = ?3
                 WHERE id = ?1 AND status = 'available' AND retired_at IS NULL
                   AND offer_revision = ?4",
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
            serde_json::json!({ "free": daily_rate_minor.is_none() }),
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
        deny_future_access: bool,
    ) -> Result<(), AppError> {
        if deny_future_access && !owner_override {
            return Err(AppError::BadRequest(
                "only the Share owner can deny future renter access".into(),
            ));
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
            Option<i64>,
        )> = tx
            .query_row(
                "SELECT share_id, seat_id, listing_id, entitlement_id, owner_user_id,
                        renter_user_id, renter_email, status, daily_rate_minor
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
                        row.get(8)?,
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
            daily_rate_minor,
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
        let reason = if owner_override {
            "owner_force_revoke"
        } else {
            "renter_release"
        };
        crate::market_billing::terminate_contract_tx(&tx, "share", subscription_id, reason, &now)?;
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
        if deny_future_access {
            crate::market_access::set_product_access_decision_tx(
                &tx,
                &owner_user_id,
                &session.email,
                &renter_user_id,
                &renter_email,
                crate::market_access::PRODUCT_SHARE,
                crate::market_access::pricing_kind_for_rate(daily_rate_minor),
                crate::market_access::DECISION_DENY,
                &session.user_id,
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
            serde_json::json!({
                "futureAccessDenied": deny_future_access,
                "reason": reason,
            }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit Share seat release"))?;
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
    let subdomain = {
        let conn = state.store.conn.lock().await;
        conn.query_row(
            "SELECT COALESCE(share.subdomain, '')
             FROM share_market_seats seat
             JOIN share_market_listings listing ON listing.id = seat.listing_id
             JOIN shares share ON share.share_id = listing.share_id
             WHERE seat.id = ?1",
            params![seat_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read Share route before rental"))?
        .ok_or_else(|| AppError::NotFound("Share Market seat not found".into()))?
    };
    if subdomain.is_empty()
        || !state
            .proxy
            .active_subdomains()
            .await
            .iter()
            .any(|active| active.eq_ignore_ascii_case(&subdomain))
    {
        return Err(AppError::Conflict(
            "the Share is offline; retry after its owner restores service".into(),
        ));
    }
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

async fn release_subscription(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(subscription_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_request_release(&session, &subscription_id, false, false)
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
        .share_market_request_release(&session, &subscription_id, true, input.deny_future_access)
        .await?;
    run_once(&state).await?;
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
    crate::market_billing::terminate_contract_tx(tx, "share", subscription_id, reason, now)?;
    let released = tx
        .execute(
            "UPDATE share_market_subscriptions
         SET status = 'released', release_reason = COALESCE(release_reason, ?2),
             updated_at = ?3, released_at = ?3
         WHERE id = ?1 AND status != 'released'",
            params![subscription_id, reason, now],
        )
        .map_err(map_db("release Share subscription"))?;
    let retired = retire_seat(tx, seat_id, subscription_id, now)?;
    if released > 0 {
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
    }
    if retired {
        event_tx(
            tx,
            Some(listing_id),
            Some(seat_id),
            Some(subscription_id),
            None,
            "seat_retired",
            serde_json::json!({ "reason": reason }),
            now,
        )?;
    }
    Ok(())
}

fn retire_seat(
    conn: &Connection,
    seat_id: &str,
    subscription_id: &str,
    now: &str,
) -> Result<bool, AppError> {
    let changed = conn
        .execute(
            "UPDATE share_market_seats
             SET status = 'disabled', current_subscription_id = NULL,
                 retired_subscription_id = COALESCE(retired_subscription_id, ?2),
                 retired_at = COALESCE(retired_at, ?3), updated_at = ?3
             WHERE id = ?1 AND current_subscription_id = ?2",
            params![seat_id, subscription_id, now],
        )
        .map_err(map_db("retire Share seat"))?;
    Ok(changed > 0)
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
    let changed = tx
        .execute(
            "UPDATE share_market_subscriptions
         SET status = 'revoke_pending', release_reason = ?2, updated_at = ?3
         WHERE id = ?1 AND status NOT IN ('released', 'grant_failed')",
            params![subscription_id, reason, now],
        )
        .map_err(map_db("request Share revoke"))?;
    tx.execute(
        "UPDATE share_market_seats SET status = 'revoking', updated_at = ?2 WHERE id = ?1",
        params![seat_id, now],
    )
    .map_err(map_db("mark automatic Share seat revoke"))?;
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
    if changed > 0 {
        event_tx(
            tx,
            None,
            Some(seat_id),
            Some(subscription_id),
            None,
            "entitlement_revoke_requested",
            serde_json::json!({ "reason": reason }),
            now,
        )?;
    }
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

            if record.status == "active_free"
                && let Some(expires_at) = record.expires_at.as_deref()
            {
                let expires_at_dt = parse_time(expires_at)?;
                if expires_at_dt <= now_dt {
                    request_revoke_tx(
                        &tx,
                        &record.id,
                        &record.share_id,
                        &record.seat_id,
                        &record.entitlement_id,
                        &record.renter_email,
                        "free_period_expired",
                        &now,
                    )?;
                    event_tx(
                        &tx,
                        Some(&record.listing_id),
                        Some(&record.seat_id),
                        Some(&record.id),
                        None,
                        "free_period_expired",
                        serde_json::json!({ "expiresAt": expires_at }),
                        &now,
                    )?;
                    continue;
                }
                if expires_at_dt <= now_dt + Duration::hours(24) {
                    let warned: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM share_market_events
                             WHERE subscription_id = ?1 AND event_type = 'free_period_expiring')",
                            params![record.id],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(map_db("check Share free expiry warning"))?
                        != 0;
                    if !warned {
                        event_tx(
                            &tx,
                            Some(&record.listing_id),
                            Some(&record.seat_id),
                            Some(&record.id),
                            None,
                            "free_period_expiring",
                            serde_json::json!({ "expiresAt": expires_at }),
                            &now,
                        )?;
                    }
                }
            }

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

            if matches!(
                record.status.as_str(),
                SUB_BILLING_SUSPEND_PENDING | SUB_BILLING_CONTROL_FAILED
            ) {
                if !has_entitlement && can_confirm_absent_entitlement_tx(&tx, &record.id)? {
                    confirm_control_effect_tx(&tx, &record.id, "revoke", &now)?;
                    tx.execute(
                        "UPDATE share_market_subscriptions
                         SET status = 'billing_suspended', updated_at = ?2
                         WHERE id = ?1",
                        params![record.id, now],
                    )
                    .map_err(map_db("confirm Share billing suspension"))?;
                    event_tx(
                        &tx,
                        Some(&record.listing_id),
                        Some(&record.seat_id),
                        Some(&record.id),
                        None,
                        "billing_suspended",
                        serde_json::json!({}),
                        &now,
                    )?;
                } else if record.status == SUB_BILLING_CONTROL_FAILED {
                    enqueue_control_operation_tx(
                        &tx,
                        &record.share_id,
                        &record.id,
                        &record.entitlement_id,
                        "revoke",
                        &record.renter_email,
                        None,
                        &now,
                    )?;
                    tx.execute(
                        "UPDATE share_market_subscriptions
                         SET status = 'billing_suspend_pending', updated_at = ?2
                         WHERE id = ?1 AND status = 'billing_control_failed'",
                        params![record.id, now],
                    )
                    .map_err(map_db("retry Share billing suspension"))?;
                }
                continue;
            }

            if record.status == SUB_BILLING_SUSPENDED {
                if has_entitlement {
                    enqueue_control_operation_tx(
                        &tx,
                        &record.share_id,
                        &record.id,
                        &record.entitlement_id,
                        "revoke",
                        &record.renter_email,
                        None,
                        &now,
                    )?;
                    tx.execute(
                        "UPDATE share_market_subscriptions
                         SET status = 'billing_suspend_pending', updated_at = ?2 WHERE id = ?1",
                        params![record.id, now],
                    )
                    .map_err(map_db("repair Share billing suspension"))?;
                }
                continue;
            }

            if record.status == SUB_BILLING_RESUME_PENDING {
                if has_entitlement {
                    confirm_control_effect_tx(&tx, &record.id, "upsert", &now)?;
                    tx.execute(
                        "UPDATE share_market_subscriptions
                         SET status = 'active_postpaid', release_reason = NULL, updated_at = ?2
                         WHERE id = ?1",
                        params![record.id, now],
                    )
                    .map_err(map_db("confirm Share billing resume"))?;
                    event_tx(
                        &tx,
                        Some(&record.listing_id),
                        Some(&record.seat_id),
                        Some(&record.id),
                        None,
                        "billing_resumed",
                        serde_json::json!({}),
                        &now,
                    )?;
                }
                continue;
            }

            if record.status == SUB_GRANT_PENDING {
                if has_entitlement {
                    confirm_control_effect_tx(&tx, &record.id, "upsert", &now)?;
                    if record.daily_rate_minor.is_none() {
                        let expires_at = record
                            .free_duration_days
                            .map(|days| (now_dt + Duration::days(i64::from(days))).to_rfc3339());
                        tx.execute(
                            "UPDATE share_market_subscriptions
                             SET status = 'active_free', activated_at = ?2,
                                 expires_at = ?3, updated_at = ?2 WHERE id = ?1",
                            params![record.id, now, expires_at],
                        )
                        .map_err(map_db("activate free Share subscription"))?;
                    } else {
                        let daily_rate_minor = record.daily_rate_minor.ok_or_else(|| {
                            AppError::Internal("paid Share daily rate is missing".into())
                        })?;
                        let currency = record.currency.as_deref().ok_or_else(|| {
                            AppError::Internal("paid Share currency is missing".into())
                        })?;
                        crate::market_billing::activate_contract_tx(
                            &tx,
                            crate::market_billing::ActivateContractInput {
                                product_kind: "share",
                                product_ref: &record.id,
                                service_ref: &record.share_id,
                                service_label: &record.share_name,
                                buyer_user_id: &record.renter_user_id,
                                buyer_email: &record.renter_email,
                                supplier_user_id: &record.owner_user_id,
                                supplier_email: &record.owner_email,
                                currency,
                                daily_rate_minor,
                                offer_revision: record.offer_revision,
                                replacement_of: None,
                            },
                            &now,
                        )?;
                        tx.execute(
                            "UPDATE share_market_subscriptions
                             SET status = 'active_postpaid', activated_at = ?2,
                                 updated_at = ?2 WHERE id = ?1",
                            params![record.id, now],
                        )
                        .map_err(map_db("activate postpaid Share subscription"))?;
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
                        serde_json::json!({
                            "free": record.daily_rate_minor.is_none(),
                            "freeDurationDays": record.free_duration_days,
                        }),
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
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending',
                           ?6, ?4, ?7, ?7, NULL, NULL, NULL)",
                params![
                    edit_id,
                    share_id,
                    installation_id,
                    SHARE_MARKET_CONTROL_ACTOR_EMAIL,
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

        let retry_candidates = {
            let mut statement = tx
                .prepare(
                    "SELECT op.id, edit.id, edit.installation_id, edit.share_id,
                            edit.revision, op.updated_at
                     FROM share_control_operations op
                     JOIN share_edit_requests edit ON edit.id = op.edit_id
                     WHERE op.status = 'dispatched'
                       AND edit.status = 'pending'
                       AND edit.retired_at IS NULL
                     ORDER BY op.updated_at, op.created_at",
                )
                .map_err(map_db("prepare dispatched Share control wake retries"))?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })
                .map_err(map_db("query dispatched Share control wake retries"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_db("read dispatched Share control wake retries"))?
        };
        let retry_cutoff = now_dt - Duration::seconds(CONTROL_DISPATCH_WAKE_RETRY_SECS);
        for (operation_id, edit_id, installation_id, share_id, revision, updated_at) in
            retry_candidates
        {
            if dispatched_shares.contains(&share_id) || parse_time(&updated_at)? > retry_cutoff {
                continue;
            }
            let refreshed = tx
                .execute(
                    "UPDATE share_control_operations
                     SET updated_at = ?4
                     WHERE id = ?1 AND status = 'dispatched' AND edit_id = ?2
                       AND updated_at = ?3",
                    params![operation_id, edit_id, updated_at, now],
                )
                .map_err(map_db("refresh dispatched Share control wake retry"))?;
            if refreshed != 1 || !dispatched_shares.insert(share_id.clone()) {
                continue;
            }
            events.push(ShareEditAvailableEvent {
                kind: "share_edit_available".to_string(),
                installation_id,
                share_id,
                revision,
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
    if status == "rejected" {
        conn.execute(
            "UPDATE share_edit_requests
             SET retired_at = COALESCE(retired_at, ?2)
             WHERE id = ?1 AND status = 'rejected'",
            params![edit_id, now],
        )
        .map_err(map_db("retire rejected Share control edit"))?;
    }
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
    let sanitized_error = error_message.map(crate::store::client_chat::sanitize_system_event_text);
    let error_message = sanitized_error.as_deref();
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
            crate::market_billing::terminate_contract_tx(
                conn,
                "share",
                &subscription_id,
                "entitlement_grant_failed",
                now,
            )?;
            retire_seat(conn, &seat_id, &subscription_id, now)?;
            enqueue_subscription_lifecycle_event_tx(
                conn,
                &subscription_id,
                "entitlement_failed",
                serde_json::json!({ "error": error_message }),
                &format!("share-control:{operation_id}:grant-failed"),
                now,
            )?;
        }
        let resume_failed = conn
            .execute(
                "UPDATE share_market_subscriptions
             SET status = 'billing_suspended', release_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND status = 'billing_resume_pending'",
                params![subscription_id, error_message, now],
            )
            .map_err(map_db("fail Share billing resume"))?;
        if resume_failed == 1 {
            enqueue_subscription_lifecycle_event_tx(
                conn,
                &subscription_id,
                "billing_resume_failed",
                serde_json::json!({ "error": error_message }),
                &format!("share-control:{operation_id}:billing-resume-failed"),
                now,
            )?;
        }
    } else {
        let revoke_failed = conn
            .execute(
                "UPDATE share_market_subscriptions
             SET status = 'revoke_failed', release_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND status = 'revoke_pending'",
                params![subscription_id, error_message, now],
            )
            .map_err(map_db("mark Share revoke failed"))?;
        if revoke_failed == 1 {
            enqueue_subscription_lifecycle_event_tx(
                conn,
                &subscription_id,
                "revoke_failed",
                serde_json::json!({ "error": error_message }),
                &format!("share-control:{operation_id}:revoke-failed"),
                now,
            )?;
        }
        let suspension_failed = conn
            .execute(
                "UPDATE share_market_subscriptions
             SET status = 'billing_control_failed', release_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND status = 'billing_suspend_pending'",
                params![subscription_id, error_message, now],
            )
            .map_err(map_db("fail Share billing suspension"))?;
        if suspension_failed == 1 {
            enqueue_subscription_lifecycle_event_tx(
                conn,
                &subscription_id,
                "billing_suspension_failed",
                serde_json::json!({ "error": error_message }),
                &format!("share-control:{operation_id}:billing-suspension-failed"),
                now,
            )?;
        }
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
        AppError::Internal(format!(
            "encode Share grants after control effect failed: {error}"
        ))
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

pub async fn suspend_for_billing(
    state: &ServerState,
    subscription_id: &str,
    reason: &str,
) -> Result<(), AppError> {
    let now = Utc::now().to_rfc3339();
    {
        let mut conn = state.store.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin Share billing suspension"))?;
        let row = tx
            .query_row(
                "SELECT share_id, seat_id, listing_id, entitlement_id, renter_email, status
             FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
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
            .map_err(map_db("read Share subscription for billing suspension"))?
            .ok_or_else(|| AppError::NotFound("Share subscription not found".into()))?;
        if matches!(
            row.5.as_str(),
            SUB_BILLING_SUSPENDED | SUB_BILLING_SUSPEND_PENDING
        ) {
            tx.commit()
                .map_err(map_db("commit idempotent Share billing suspension"))?;
            return Ok(());
        }
        if matches!(row.5.as_str(), SUB_RELEASED | SUB_GRANT_FAILED) {
            return Err(AppError::Conflict(
                "released Share subscription cannot be suspended".into(),
            ));
        }
        enqueue_control_operation_tx(
            &tx,
            &row.0,
            subscription_id,
            &row.3,
            "revoke",
            &row.4,
            None,
            &now,
        )?;
        tx.execute(
            "UPDATE share_market_subscriptions
         SET status = 'billing_suspend_pending', release_reason = ?2,
             updated_at = ?3 WHERE id = ?1",
            params![subscription_id, reason, now],
        )
        .map_err(map_db("request Share billing suspension"))?;
        event_tx(
            &tx,
            Some(&row.2),
            Some(&row.1),
            Some(subscription_id),
            None,
            "billing_suspension_requested",
            serde_json::json!({ "reason": reason }),
            &now,
        )?;
        tx.commit()
            .map_err(map_db("commit Share billing suspension"))?;
    }
    run_once(state).await?;
    Ok(())
}

pub async fn resume_after_billing(
    state: &ServerState,
    subscription_id: &str,
) -> Result<(), AppError> {
    let now = Utc::now().to_rfc3339();
    {
        let mut conn = state.store.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin Share billing resume"))?;
        let row = tx
            .query_row(
                "SELECT sub.share_id, sub.seat_id, sub.listing_id, sub.entitlement_id,
                    sub.renter_email, sub.status,
                    (SELECT operation.policy_json
                     FROM share_control_operations operation
                     WHERE operation.subscription_id = sub.id
                       AND operation.action = 'upsert' AND operation.policy_json IS NOT NULL
                     ORDER BY operation.created_at DESC LIMIT 1)
             FROM share_market_subscriptions sub WHERE sub.id = ?1",
                params![subscription_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Share subscription for billing resume"))?
            .ok_or_else(|| AppError::NotFound("Share subscription not found".into()))?;
        if row.5 == SUB_BILLING_RESUME_PENDING || row.5 == SUB_ACTIVE_POSTPAID {
            tx.commit()
                .map_err(map_db("commit idempotent Share billing resume"))?;
            return Ok(());
        }
        if row.5 != SUB_BILLING_SUSPENDED && row.5 != SUB_BILLING_CONTROL_FAILED {
            return Err(AppError::Conflict(
                "Share subscription is not suspended for billing".into(),
            ));
        }
        let policy = row
            .6
            .as_deref()
            .map(serde_json::from_str::<ShareUserPolicy>)
            .transpose()
            .map_err(|_| AppError::Internal("stored Share billing policy is invalid".into()))?
            .ok_or_else(|| AppError::Internal("Share billing policy is missing".into()))?;
        enqueue_control_operation_tx(
            &tx,
            &row.0,
            subscription_id,
            &row.3,
            "upsert",
            &row.4,
            Some(&policy),
            &now,
        )?;
        tx.execute(
            "UPDATE share_market_subscriptions
         SET status = 'billing_resume_pending', release_reason = NULL, updated_at = ?2
         WHERE id = ?1",
            params![subscription_id, now],
        )
        .map_err(map_db("request Share billing resume"))?;
        event_tx(
            &tx,
            Some(&row.2),
            Some(&row.1),
            Some(subscription_id),
            None,
            "billing_resume_requested",
            serde_json::json!({}),
            &now,
        )?;
        tx.commit().map_err(map_db("commit Share billing resume"))?;
    }
    run_once(state).await?;
    Ok(())
}

pub async fn terminate_for_billing(
    state: &ServerState,
    subscription_id: &str,
    reason: &str,
) -> Result<(), AppError> {
    let now = Utc::now().to_rfc3339();
    {
        let mut conn = state.store.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin Share billing termination"))?;
        let row = tx
            .query_row(
                "SELECT share_id, seat_id, listing_id, entitlement_id, renter_email, status
                 FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
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
            .map_err(map_db("read Share subscription for billing termination"))?;
        let Some((share_id, seat_id, listing_id, entitlement_id, renter_email, status)) = row
        else {
            return Ok(());
        };
        if matches!(
            status.as_str(),
            SUB_RELEASED | SUB_GRANT_FAILED | SUB_REVOKE_PENDING
        ) {
            tx.commit()
                .map_err(map_db("commit idempotent Share billing termination"))?;
            return Ok(());
        }
        let retired = retire_unconfirmed_grant_tx(&tx, subscription_id, reason, &now)?;
        let grants_json = tx
            .query_row(
                "SELECT user_grants_json FROM shares WHERE share_id = ?1",
                params![share_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(map_db("read Share grants during billing termination"))?
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
        tx.commit()
            .map_err(map_db("commit Share billing termination"))?;
    }
    run_once(state).await
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
    use chrono::TimeZone;

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
            daily_rate_minor: None,
            currency: None,
            free_duration_days: Some(1),
        }
    }

    fn paid_seat() -> SeatInput {
        SeatInput {
            daily_rate_minor: Some(1_200),
            currency: Some("CNY".into()),
            free_duration_days: None,
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
                    share_id, capacity_pool_id, installation_id, share_name, owner_email, subdomain,
                    app_type, token_limit, parallel_limit, tokens_used, requests_count,
                    share_status, created_at, expires_at, user_grants_json,
                    supported_user_token_periods_json, config_revision, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'codex', -1, 3, 0, 0,
                           'active', ?7, '9999-12-31T23:59:59Z', '{}', ?8, 1, ?7)",
                params![
                    share_id,
                    format!("pool-{share_id}"),
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
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO account_payment_profiles (user_id, owner_email, methods_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(user_id) DO UPDATE SET methods_json = excluded.methods_json,
                    updated_at = excluded.updated_at",
            params![owner.user_id, owner.email, methods, updated_at],
        )
        .expect("configure payment profile");
        conn.execute(
            "INSERT INTO supplier_billing_profiles (
                supplier_user_id, supplier_email, currency,
                settlement_grace_hours, revision, created_at, updated_at
             ) VALUES (?1, ?2, 'CNY', 24, 1, ?3, ?3)
             ON CONFLICT(supplier_user_id, currency) DO UPDATE SET
                supplier_email = excluded.supplier_email,
                settlement_grace_hours = excluded.settlement_grace_hours,
                revision = supplier_billing_profiles.revision + 1,
                updated_at = excluded.updated_at",
            params![owner.user_id, owner.email, updated_at],
        )
        .expect("configure CNY payment grace");
    }

    async fn create_listing(
        store: &AppStore,
        owner: &AuthSession,
        share_id: &str,
        seat: SeatInput,
    ) -> (String, String) {
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(&conn, owner, "CNY", 50_000, &now);
        }
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

    #[tokio::test]
    async fn stale_dispatched_control_edit_is_reawakened_without_consuming_attempts() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-wake-retry", "owner-wake-retry@example.com");
        let renter = session("renter-wake-retry", "renter-wake-retry@example.com");
        insert_share(
            &store,
            "share-wake-retry",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-wake-retry", free_seat()).await;
        store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent wake-retry seat");
        let now = Utc.with_ymd_and_hms(2026, 7, 29, 12, 0, 0).unwrap();

        let first = store
            .share_market_reconcile_and_dispatch(now)
            .await
            .expect("dispatch managed grant");
        assert_eq!(first.len(), 1);
        assert!(
            store
                .share_market_reconcile_and_dispatch(
                    now + Duration::seconds(CONTROL_DISPATCH_WAKE_RETRY_SECS - 1),
                )
                .await
                .expect("skip early wake retry")
                .is_empty()
        );

        let retried = store
            .share_market_reconcile_and_dispatch(
                now + Duration::seconds(CONTROL_DISPATCH_WAKE_RETRY_SECS),
            )
            .await
            .expect("retry stale dispatched edit");
        assert_eq!(retried.len(), 1);
        assert_eq!(retried[0].share_id, first[0].share_id);
        assert_eq!(retried[0].revision, first[0].revision);
        let attempts: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT attempts FROM share_control_operations
                 WHERE share_id = 'share-wake-retry' AND status = 'dispatched'",
                [],
                |row| row.get(0),
            )
            .expect("read retry attempts");
        assert_eq!(attempts, 1);
        assert!(
            store
                .share_market_reconcile_and_dispatch(
                    now + Duration::seconds(CONTROL_DISPATCH_WAKE_RETRY_SECS + 1),
                )
                .await
                .expect("throttle repeated wake retry")
                .is_empty()
        );
    }

    #[test]
    fn empty_price_and_period_is_free() {
        let seat = normalize_seat(free_seat()).expect("free seat");
        assert!(seat.is_free());
        assert_eq!(seat.free_duration_days, Some(1));
    }

    #[test]
    fn free_duration_is_bounded_and_paid_seats_reject_it() {
        let mut invalid = free_seat();
        invalid.free_duration_days = Some(MAX_FREE_DURATION_DAYS + 1);
        assert!(normalize_seat(invalid).is_err());

        let mut permanent = free_seat();
        permanent.free_duration_days = None;
        assert_eq!(
            normalize_seat(permanent)
                .expect("permanent free seat")
                .free_duration_days,
            None
        );

        let mut paid = paid_seat();
        paid.free_duration_days = Some(7);
        assert!(normalize_seat(paid).is_err());
    }

    #[tokio::test]
    async fn finite_free_subscription_expires_once_from_entitlement_activation() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-expiry", "owner-expiry@example.com");
        let renter = session("renter-expiry", "renter-expiry@example.com");
        insert_share(
            &store,
            "share-expiry",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) = create_listing(&store, &owner, "share-expiry", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent finite free seat");
        let activated_at = Utc::now();
        activate_subscription(&store, &subscription_id, activated_at).await;

        let (stored_activated_at, expires_at): (String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT activated_at, expires_at FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read free term");
        assert_eq!(parse_time(&stored_activated_at).unwrap(), activated_at);
        assert_eq!(
            parse_time(&expires_at).unwrap(),
            activated_at + Duration::days(1)
        );

        store
            .share_market_reconcile_and_dispatch(activated_at + Duration::days(1))
            .await
            .expect("expire free subscription");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );
        store
            .share_market_reconcile_and_dispatch(activated_at + Duration::days(2))
            .await
            .expect("repeat expiry reconciliation");
        let expiry_events: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT COUNT(*) FROM share_market_events
                 WHERE subscription_id = ?1 AND event_type = 'free_period_expired'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("count expiry events");
        assert_eq!(expiry_events, 1);

        clear_entitlements(&store, "share-expiry").await;
        store
            .share_market_reconcile_and_dispatch(activated_at + Duration::days(2))
            .await
            .expect("finish expired free release");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_RELEASED
        );
    }

    #[tokio::test]
    async fn permanent_free_subscription_does_not_expire() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-permanent", "owner-permanent@example.com");
        let renter = session("renter-permanent", "renter-permanent@example.com");
        insert_share(
            &store,
            "share-permanent",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let mut seat = free_seat();
        seat.free_duration_days = None;
        let (_, seat_id) = create_listing(&store, &owner, "share-permanent", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent permanent free seat");
        let activated_at = Utc::now();
        activate_subscription(&store, &subscription_id, activated_at).await;
        store
            .share_market_reconcile_and_dispatch(activated_at + Duration::days(400))
            .await
            .expect("reconcile permanent free subscription");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            "active_free"
        );
    }

    #[test]
    fn partial_or_zero_pricing_is_rejected() {
        let mut input = free_seat();
        input.daily_rate_minor = Some(100);
        assert!(normalize_seat(input).is_err());

        let mut input = free_seat();
        input.daily_rate_minor = Some(0);
        input.currency = Some("CNY".into());
        assert!(normalize_seat(input).is_err());

        let mut input = free_seat();
        input.token_limit = Some(i64::MAX as u64 + 1);
        assert!(normalize_seat(input).is_err());
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
        let active_renter_index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_share_market_active_renter_share'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_renter_index, 1);
    }

    #[test]
    fn schema_excludes_legacy_pricing_period_columns() {
        let conn = Connection::open_in_memory().expect("memory database");
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        init_schema(&conn).expect("schema");

        for table in ["share_market_seats", "share_market_subscriptions"] {
            let columns = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .expect("prepare Share Market columns")
                .query_map([], |row| row.get::<_, String>(1))
                .expect("query Share Market columns")
                .collect::<Result<Vec<_>, _>>()
                .expect("read Share Market columns");
            assert!(columns.iter().any(|column| column == "daily_rate_minor"));
            assert!(!columns.iter().any(|column| column == "period_unit"));
            assert!(!columns.iter().any(|column| column == "period_count"));
        }
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
        let (listing_id, seat_id) =
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
        let (
            subscription_status,
            seat_status,
            retired_subscription_id,
            retired_at,
            upsert_status,
            revoke_count,
        ): (String, String, Option<String>, Option<String>, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, seat.status, seat.retired_subscription_id,
                        seat.retired_at, operation.status,
                        (SELECT COUNT(*) FROM share_control_operations revoke
                         WHERE revoke.subscription_id = sub.id AND revoke.action = 'revoke')
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 JOIN share_control_operations operation
                   ON operation.subscription_id = sub.id AND operation.action = 'upsert'
                 WHERE sub.id = ?1",
                params![first_subscription],
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
            .expect("read canceled unavailable grant");
        assert_eq!(subscription_status, SUB_RELEASED);
        assert_eq!(seat_status, SEAT_DISABLED);
        assert_eq!(
            retired_subscription_id.as_deref(),
            Some(first_subscription.as_str())
        );
        assert!(retired_at.is_some());
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
        assert!(matches!(
            store
                .share_market_rent_seat(&renter_b, &seat_id, RentSeatRequest { offer_revision: 1 })
                .await,
            Err(AppError::Conflict(_))
        ));
        let replacement_seat = store
            .share_market_add_seat(&owner, &listing_id, free_seat())
            .await
            .expect("create replacement seat after retirement");
        let second_subscription = store
            .share_market_rent_seat(
                &renter_b,
                &replacement_seat,
                RentSeatRequest { offer_revision: 1 },
            )
            .await
            .expect("rent seat before immediate release");
        store
            .share_market_request_release(&renter_b, &second_subscription, false, false)
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
    async fn retired_seats_do_not_consume_listing_capacity() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-capacity", "owner-capacity@example.com");
        let renter = session("renter-capacity", "renter-capacity@example.com");
        insert_share(
            &store,
            "share-capacity",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(
                &conn,
                &owner,
                "CNY",
                50_000,
                &Utc::now().to_rfc3339(),
            );
        }
        let listing_id = store
            .share_market_create_listing(
                &owner,
                CreateListingRequest {
                    share_id: "share-capacity".into(),
                    seats: vec![free_seat(); MAX_SEATS_PER_LISTING],
                },
            )
            .await
            .expect("create full listing");
        assert!(matches!(
            store
                .share_market_add_seat(&owner, &listing_id, free_seat())
                .await,
            Err(AppError::Conflict(_))
        ));
        let first_seat: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT id FROM share_market_seats
                 WHERE listing_id = ?1 AND position = 1",
                params![listing_id],
                |row| row.get(0),
            )
            .expect("read first full-listing seat");
        let subscription_id = store
            .share_market_rent_seat(&renter, &first_seat, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent capacity seat");
        store
            .share_market_request_release(&renter, &subscription_id, false, false)
            .await
            .expect("release undispatched capacity seat");

        let replacement = store
            .share_market_add_seat(&owner, &listing_id, free_seat())
            .await
            .expect("retired seat frees effective capacity");
        let (active_count, retired_count, replacement_position): (i64, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT
                    SUM(CASE WHEN retired_at IS NULL AND status IN
                        ('available', 'reserved', 'occupied', 'revoking') THEN 1 ELSE 0 END),
                    SUM(CASE WHEN retired_at IS NOT NULL THEN 1 ELSE 0 END),
                    MAX(CASE WHEN id = ?2 THEN position ELSE 0 END)
                 FROM share_market_seats WHERE listing_id = ?1",
                params![listing_id, replacement],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read effective listing capacity");
        assert_eq!(active_count, MAX_SEATS_PER_LISTING as i64);
        assert_eq!(retired_count, 1);
        assert_eq!(replacement_position, MAX_SEATS_PER_LISTING as i64 + 1);
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
            .share_market_request_release(&owner, &subscription_id, true, false)
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
        let (seat_status, retired_subscription_id, retired_at): (
            String,
            Option<String>,
            Option<String>,
        ) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, retired_subscription_id, retired_at
                 FROM share_market_seats WHERE id = ?1 AND listing_id = ?2",
                params![seat_id, listing_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read released missing Share seat");
        assert_eq!(seat_status, SEAT_DISABLED);
        assert_eq!(
            retired_subscription_id.as_deref(),
            Some(subscription_id.as_str())
        );
        assert!(retired_at.is_some());
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
    async fn free_rental_descriptor_confirmation_recovers_lost_acks() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner", "owner@example.com");
        let renter = session("renter", "renter@example.com");
        insert_share(&store, "share-free", &owner.email, &[ShareTokenPeriod::Day]).await;
        let (listing_id, seat_id) = create_listing(&store, &owner, "share-free", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent free seat");
        let now = Utc::now();
        activate_subscription(&store, &subscription_id, now).await;

        let (subscription_status, seat_status, upsert_status, edit_status): (
            String,
            String,
            String,
            String,
        ) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, seat.status, operation.status, edit.status
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 JOIN share_control_operations operation
                   ON operation.subscription_id = sub.id AND operation.action = 'upsert'
                 JOIN share_edit_requests edit ON edit.id = operation.edit_id
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read active free rental");
        assert_eq!(subscription_status, "active_free");
        assert_eq!(seat_status, "occupied");
        assert_eq!(upsert_status, "applied");
        assert_eq!(edit_status, "applied");

        store
            .share_market_request_release(&renter, &subscription_id, false, false)
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
        let (
            subscription_status,
            seat_status,
            retired_subscription_id,
            retired_at,
            revoke_status,
            revoke_attempts,
        ): (String, String, Option<String>, Option<String>, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, seat.status, seat.retired_subscription_id,
                        seat.retired_at, operation.status, operation.attempts
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 JOIN share_control_operations operation
                   ON operation.subscription_id = sub.id AND operation.action = 'revoke'
                 WHERE sub.id = ?1",
                params![subscription_id],
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
            .expect("read released free rental");
        assert_eq!(subscription_status, "released");
        assert_eq!(seat_status, SEAT_DISABLED);
        assert_eq!(
            retired_subscription_id.as_deref(),
            Some(subscription_id.as_str())
        );
        assert!(retired_at.is_some());
        assert_eq!(revoke_status, "applied");
        assert_eq!(revoke_attempts, 0);
        assert!(matches!(
            store.share_market_delete_seat(&owner, &seat_id).await,
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            store
                .share_market_rent_seat(
                    &session("other", "other@example.com"),
                    &seat_id,
                    RentSeatRequest { offer_revision: 1 },
                )
                .await,
            Err(AppError::Conflict(_))
        ));

        let owner_catalog = store
            .share_market_catalog(Some(&owner), &[])
            .await
            .expect("read owner catalog after release");
        let retired = owner_catalog.listings[0]
            .seats
            .iter()
            .find(|seat| seat.id == seat_id)
            .expect("retired owner seat");
        assert_eq!(retired.status, SEAT_RETIRED_VIEW);
        assert!(retired.read_only);
        assert_eq!(
            retired
                .subscription
                .as_ref()
                .map(|subscription| subscription.renter_email.as_str()),
            Some(renter.email.as_str())
        );

        let unrelated = session("unrelated", "unrelated@example.com");
        let unrelated_catalog = store
            .share_market_catalog(Some(&unrelated), &[])
            .await
            .expect("read unrelated catalog after release");
        let public_retired = unrelated_catalog.listings[0]
            .seats
            .iter()
            .find(|seat| seat.id == seat_id)
            .expect("retired public seat");
        assert_eq!(
            public_retired
                .subscription
                .as_ref()
                .map(|subscription| subscription.renter_email.as_str()),
            Some(renter.email.as_str())
        );
        let anonymous_catalog = store
            .share_market_catalog(None, &[])
            .await
            .expect("read anonymous catalog after release");
        assert_eq!(
            anonymous_catalog.listings[0].seats[0]
                .subscription
                .as_ref()
                .map(|subscription| subscription.renter_email.as_str()),
            Some(renter.email.as_str())
        );

        let renter_catalog = store
            .share_market_catalog(Some(&renter), &[])
            .await
            .expect("read renter history after release");
        assert!(
            renter_catalog
                .my_subscriptions
                .iter()
                .any(|subscription| subscription.id == subscription_id)
        );
        assert_eq!(
            renter_catalog.listings[0].seats[0]
                .subscription
                .as_ref()
                .map(|subscription| subscription.renter_email.as_str()),
            Some(renter.email.as_str())
        );

        let replacement_seat = store
            .share_market_add_seat(&owner, &listing_id, free_seat())
            .await
            .expect("add replacement seat");
        let catalog_after_add = store
            .share_market_catalog(Some(&owner), &[])
            .await
            .expect("read lifecycle ordering");
        assert_eq!(catalog_after_add.listings[0].seats.len(), 2);
        assert_eq!(catalog_after_add.listings[0].seats[0].id, replacement_seat);
        assert_eq!(catalog_after_add.listings[0].seats[0].position, 2);
        assert_eq!(catalog_after_add.listings[0].seats[1].id, seat_id);
        assert!(catalog_after_add.listings[0].seats[1].read_only);
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("close listing with rental history");
        assert!(matches!(
            store.share_market_delete_listing(&owner, &listing_id).await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn catalog_only_exposes_payment_kinds_and_never_authorizes_assets() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-contact", "owner-contact@example.com");
        let viewer = session("viewer-contact", "viewer-contact@example.com");
        configure_payment_profile(&store, &owner, "account-contact", "profile-contact").await;
        let asset_id = store
            .client_market_store_payment_asset(&owner.user_id, "contact-qr", b"png")
            .await
            .expect("store payment asset");

        assert!(matches!(
            store
                .client_market_payment_asset_for_viewer(&asset_id, Some(&viewer))
                .await,
            Err(AppError::Forbidden(_))
        ));

        insert_share(
            &store,
            "share-contact",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, _) = create_listing(&store, &owner, "share-contact", paid_seat()).await;

        let anonymous = store
            .share_market_catalog(None, &[])
            .await
            .expect("load anonymous catalog");
        assert_eq!(anonymous.listings[0].payment_method_kinds, vec!["alipay"]);

        let authenticated = store
            .share_market_catalog(Some(&viewer), &[])
            .await
            .expect("load authenticated catalog");
        assert_eq!(
            authenticated.listings[0].payment_method_kinds,
            vec!["alipay"]
        );
        assert!(matches!(
            store
                .client_market_payment_asset_for_viewer(&asset_id, Some(&viewer))
                .await,
            Err(AppError::Forbidden(_))
        ));

        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("close listing");
        assert!(matches!(
            store
                .client_market_payment_asset_for_viewer(&asset_id, Some(&viewer))
                .await,
            Err(AppError::Forbidden(_))
        ));
    }

    #[tokio::test]
    async fn paid_rental_creates_one_unified_contract_before_grant_without_accruing() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-postpaid", "owner-postpaid@example.com");
        let renter = session("renter-postpaid", "renter-postpaid@example.com");
        configure_payment_profile(&store, &owner, "account-v1", "profile-v1").await;
        insert_share(
            &store,
            "share-postpaid",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-postpaid", paid_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent paid seat");
        {
            let conn = store.conn.lock().await;
            let payload_json: String = conn
                .query_row(
                    "SELECT payload_json FROM client_chat_system_outbox
                     WHERE event_type = 'seat_rented' ORDER BY created_at DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .expect("read Share rental chat event");
            let payload: serde_json::Value =
                serde_json::from_str(&payload_json).expect("parse Share rental chat event");
            assert_eq!(payload["ownerEmail"], owner.email);
            assert_eq!(payload["renterEmail"], renter.email);
            assert_eq!(payload["parallelLimit"], 2);
            assert_eq!(payload["tokenLimit"], 10_000);
            assert_eq!(payload["tokenPeriod"], "day");
            assert_eq!(payload["dailyRateMinor"], 1_200);
            assert_eq!(payload["currency"], "CNY");
            assert_eq!(payload["paymentMethods"][0]["account"], "account-v1");
        }
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_GRANT_PENDING
        );
        let check_at = Utc::now() + Duration::seconds(1);
        store
            .market_billing_reconcile(check_at)
            .await
            .expect("reconcile pending Share contract");
        {
            let conn = store.conn.lock().await;
            let pending_contract: (String, i64, String, i64) = conn
                .query_row(
                    "SELECT contract.status, contract.trial_seconds_remaining,
                            contract.service_label, account.balance_units
                     FROM market_service_contracts contract
                     JOIN market_credit_accounts account ON account.id = contract.account_id
                     WHERE contract.product_ref = ?1",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read pending unified Share contract");
            assert_eq!(pending_contract.0, "trial");
            assert_eq!(pending_contract.1, crate::market_billing::TRIAL_SECONDS);
            assert_eq!(pending_contract.2, "Share share-postpaid");
            assert_eq!(pending_contract.3, 0);
        }

        activate_subscription(&store, &subscription_id, check_at + Duration::seconds(1)).await;

        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_ACTIVE_POSTPAID
        );
        let conn = store.conn.lock().await;
        let contract: (String, String, String, i64, i64, String) = conn
            .query_row(
                "SELECT product_kind, product_ref, status, trial_seconds_remaining,
                        daily_rate_minor, currency
                 FROM market_service_contracts WHERE product_ref = ?1",
                params![subscription_id],
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
            .expect("read unified Share contract");
        assert_eq!(contract.0, "share");
        assert_eq!(contract.1, subscription_id);
        assert_eq!(contract.2, "trial");
        assert_eq!(contract.3, crate::market_billing::TRIAL_SECONDS);
        assert_eq!(contract.4, 1_200);
        assert_eq!(contract.5, "CNY");
    }

    #[tokio::test]
    async fn rejected_paid_grant_terminates_precreated_billing_contract() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-paid-reject", "owner-paid-reject@example.com");
        let renter = session("renter-paid-reject", "renter-paid-reject@example.com");
        configure_payment_profile(&store, &owner, "account-reject", "profile-reject").await;
        insert_share(
            &store,
            "share-paid-reject",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-paid-reject", paid_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent paid seat before rejected grant");
        let now = Utc::now();
        store
            .share_market_reconcile_and_dispatch(now)
            .await
            .expect("dispatch paid grant");
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
            .expect("read paid grant edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                params![edit_id],
            )
            .expect("reject paid grant edit");
            handle_control_edit_ack(
                &conn,
                &edit_id,
                "rejected",
                Some("x-api-key: fake-managed-grant-secret"),
                &(now + Duration::seconds(1)).to_rfc3339(),
            )
            .expect("record rejected paid grant");
        }
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_GRANT_FAILED
        );
        let contract_status: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status FROM market_service_contracts
                 WHERE product_kind = 'share' AND product_ref = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read terminated rejected-grant contract");
        assert_eq!(contract_status, "terminated");
        let conn = store.conn.lock().await;
        let release_reason: String = conn
            .query_row(
                "SELECT release_reason FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read sanitized rejected-grant reason");
        let event_detail: String = conn
            .query_row(
                "SELECT detail_json FROM share_market_events
                 WHERE subscription_id = ?1 AND event_type = 'entitlement_failed'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read sanitized rejected-grant event");
        let outbox_payload: String = conn
            .query_row(
                "SELECT payload_json FROM client_chat_system_outbox
                 WHERE source_kind = 'share_market'
                   AND event_type = 'entitlement_failed'",
                [],
                |row| row.get(0),
            )
            .expect("read sanitized rejected-grant outbox");
        for stored in [release_reason, event_detail, outbox_payload] {
            assert!(!stored.contains("fake-managed-grant-secret"));
            assert!(stored.contains("[credential omitted]"));
        }
    }

    #[tokio::test]
    async fn failed_billing_suspension_requeues_revoke() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-billing-retry", "owner-billing-retry@example.com");
        let renter = session("renter-billing-retry", "renter-billing-retry@example.com");
        insert_share(
            &store,
            "share-billing-retry",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-billing-retry", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent billing retry seat");
        let now = Utc::now();
        activate_subscription(&store, &subscription_id, now).await;
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE share_market_subscriptions
                 SET status = 'billing_control_failed' WHERE id = ?1",
                params![subscription_id],
            )
            .expect("seed failed billing suspension");

        let retried = store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
            .await
            .expect("retry billing suspension");
        assert_eq!(retried.len(), 1);
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_BILLING_SUSPEND_PENDING
        );
        let action: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT action FROM share_control_operations
                 WHERE subscription_id = ?1 AND status = 'dispatched'
                 ORDER BY share_sequence DESC LIMIT 1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read retried billing revoke");
        assert_eq!(action, "revoke");
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
    async fn owner_can_close_force_revoke_deny_and_allow_without_interrupting_early() {
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
            .share_market_request_release(&owner, &subscription_id, true, true)
            .await
            .expect("force revoke and deny future access");
        assert!(matches!(
            store
                .share_market_rent_seat(&renter, &seat_b, RentSeatRequest { offer_revision: 1 })
                .await,
            Err(AppError::Forbidden(_))
        ));
        {
            let conn = store.conn.lock().await;
            assert!(
                !crate::market_access::product_access_allowed_tx(
                    &conn,
                    &owner.user_id,
                    &renter.user_id,
                    &renter.email,
                    crate::market_access::PRODUCT_SHARE,
                    crate::market_access::PRICING_FREE,
                )
                .expect("read denied Share access")
            );
            crate::market_access::set_product_access_decision_tx(
                &conn,
                &owner.user_id,
                &owner.email,
                &renter.user_id,
                &renter.email,
                crate::market_access::PRODUCT_SHARE,
                crate::market_access::PRICING_FREE,
                crate::market_access::DECISION_ALLOW,
                &owner.user_id,
                &Utc::now().to_rfc3339(),
            )
            .expect("allow renter again");
        }
        store
            .share_market_rent_seat(&renter, &seat_b, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent after access allowed");

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
            let retired_at: Option<String> = conn
                .query_row(
                    "SELECT retired_at FROM share_edit_requests WHERE id = ?1",
                    params![first_edit],
                    |row| row.get(0),
                )
                .expect("read retired retry edit");
            assert!(retired_at.is_some());
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
            .share_market_request_release(&owner, &subscription_id, true, false)
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
        let (listing_id, seat_id) =
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
        let (failed_seat_status, retired_subscription_id, retired_at): (
            String,
            Option<String>,
            Option<String>,
        ) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, retired_subscription_id, retired_at
                 FROM share_market_seats WHERE id = ?1",
                params![seat_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read failed grant retirement");
        assert_eq!(failed_seat_status, SEAT_DISABLED);
        assert_eq!(
            retired_subscription_id.as_deref(),
            Some(first_subscription.as_str())
        );
        assert!(retired_at.is_some());
        assert!(matches!(
            store
                .share_market_rent_seat(
                    &second_renter,
                    &seat_id,
                    RentSeatRequest { offer_revision: 1 },
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let replacement_seat = store
            .share_market_add_seat(&owner, &listing_id, free_seat())
            .await
            .expect("create seat after failed grant");
        let second_subscription = store
            .share_market_rent_seat(
                &second_renter,
                &replacement_seat,
                RentSeatRequest { offer_revision: 1 },
            )
            .await
            .expect("rent replacement seat after failed grant");
        activate_subscription(&store, &second_subscription, now + Duration::seconds(2)).await;

        store
            .share_market_request_release(&owner, &second_subscription, true, false)
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
            .share_market_request_release(&owner, &second_subscription, true, false)
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
        let replacement_retirement: (String, Option<String>) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, retired_subscription_id FROM share_market_seats WHERE id = ?1",
                params![replacement_seat],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read retried revoke retirement");
        assert_eq!(replacement_retirement.0, SEAT_DISABLED);
        assert_eq!(
            replacement_retirement.1.as_deref(),
            Some(second_subscription.as_str())
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
            .share_market_catalog(Some(&renter), &["share-seat-route".into()])
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
            .share_market_catalog(
                Some(&renter),
                &[
                    "share-canrent-a-route".into(),
                    "share-canrent-b-route".into(),
                ],
            )
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
    async fn owner_can_delete_closed_listing_without_active_rentals() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-delete-listing", "owner-delete-listing@example.com");
        insert_share(
            &store,
            "share-delete-listing",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, _seat_id) =
            create_listing(&store, &owner, "share-delete-listing", free_seat()).await;
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("close listing before delete");
        store
            .share_market_delete_listing(&owner, &listing_id)
            .await
            .expect("delete closed listing");
        let catalog = store
            .share_market_catalog(Some(&owner), &[])
            .await
            .expect("catalog after delete");
        assert!(
            catalog
                .listings
                .iter()
                .all(|listing| listing.id != listing_id)
        );
        let owned = store
            .share_market_owned_shares(&owner)
            .await
            .expect("owned shares after delete");
        assert!(!owned[0].already_listed);
    }

    #[tokio::test]
    async fn closed_listing_with_active_rental_blocks_relist() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-hold", "owner-hold@example.com");
        let renter = session("renter-hold", "renter-hold@example.com");
        insert_share(&store, "share-hold", &owner.email, &[ShareTokenPeriod::Day]).await;
        let (listing_id, seat_id) = create_listing(&store, &owner, "share-hold", free_seat()).await;
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
    async fn catalog_can_rent_respects_access_existing_rental_and_direct_grant() {
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

        {
            let conn = store.conn.lock().await;
            crate::market_access::set_product_access_decision_tx(
                &conn,
                &owner.user_id,
                &owner.email,
                &blocked.user_id,
                &blocked.email,
                crate::market_access::PRODUCT_SHARE,
                crate::market_access::PRICING_FREE,
                crate::market_access::DECISION_DENY,
                &owner.user_id,
                &Utc::now().to_rfc3339(),
            )
            .expect("deny Share access");
        }
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
            .share_market_catalog(
                Some(&renter),
                &[
                    "share-canrent-a-route".into(),
                    "share-canrent-b-route".into(),
                ],
            )
            .await
            .expect("renter catalog");
        assert!(
            !renter_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-a")
                .expect("listing a")
                .seats[0]
                .can_rent
        );
        assert!(
            renter_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-b")
                .expect("listing b")
                .seats[0]
                .can_rent
        );

        let blocked_catalog = store
            .share_market_catalog(
                Some(&blocked),
                &[
                    "share-canrent-a-route".into(),
                    "share-canrent-b-route".into(),
                ],
            )
            .await
            .expect("blocked catalog");
        assert!(
            !blocked_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-b")
                .expect("listing b for blocked")
                .seats[0]
                .can_rent
        );
        assert!(
            blocked_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-b")
                .expect("listing b for blocked approval state")
                .seats[0]
                .seller_approval_required
        );

        let granted_catalog = store
            .share_market_catalog(
                Some(&granted),
                &[
                    "share-canrent-a-route".into(),
                    "share-canrent-b-route".into(),
                ],
            )
            .await
            .expect("granted catalog");
        assert!(
            !granted_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-b")
                .expect("listing b for granted")
                .seats[0]
                .can_rent
        );
        assert!(
            !granted_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-b")
                .expect("listing b for direct grant approval state")
                .seats[0]
                .seller_approval_required
        );
    }

    #[tokio::test]
    async fn catalog_paid_can_rent_requires_current_credit_eligibility() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-paid-canrent", "owner-paid-canrent@example.com");
        let renter = session("renter-paid-canrent", "renter-paid-canrent@example.com");
        let now = Utc::now().to_rfc3339();
        configure_payment_profile(&store, &owner, "owner-payment", &now).await;
        for share_id in ["share-paid-canrent-a", "share-paid-canrent-b"] {
            insert_share(&store, share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
        }
        let (_, rented_seat) =
            create_listing(&store, &owner, "share-paid-canrent-a", paid_seat()).await;
        create_listing(&store, &owner, "share-paid-canrent-b", paid_seat()).await;
        store
            .share_market_rent_seat(&renter, &rented_seat, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent first paid seat");

        let active_subdomains = [
            "share-paid-canrent-a-route".to_string(),
            "share-paid-canrent-b-route".to_string(),
        ];
        let can_rent_second = |catalog: &ShareMarketCatalog| {
            catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-paid-canrent-b")
                .expect("second paid listing")
                .seats[0]
                .can_rent
        };
        let catalog = store
            .share_market_catalog(Some(&renter), &active_subdomains)
            .await
            .expect("catalog with available credit");
        assert!(can_rent_second(&catalog));

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_public_credit_policies
                 SET enabled = 0, revision = revision + 1, updated_at = ?2
                 WHERE supplier_user_id = ?1 AND currency = 'CNY'",
                params![owner.user_id, now],
            )
            .expect("remove paid credit");
        }
        let catalog = store
            .share_market_catalog(Some(&renter), &active_subdomains)
            .await
            .expect("catalog without credit");
        assert!(!can_rent_second(&catalog));

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_public_credit_policies
                 SET enabled = 1, limit_minor = 50000, revision = revision + 1,
                     updated_at = ?2
                 WHERE supplier_user_id = ?1 AND currency = 'CNY'",
                params![owner.user_id, now],
            )
            .expect("restore paid credit");
            conn.execute(
                "INSERT INTO market_credit_restrictions (
                    id, buyer_user_id, invoice_id, reason, status, created_at
                 ) VALUES ('catalog-overdue', ?1, 'catalog-overdue-invoice',
                           'payment_overdue', 'active', ?2)",
                params![renter.user_id, now],
            )
            .expect("restrict overdue buyer");
        }
        let catalog = store
            .share_market_catalog(Some(&renter), &active_subdomains)
            .await
            .expect("catalog for overdue buyer");
        assert!(!can_rent_second(&catalog));

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_credit_restrictions
                 SET status = 'lifted', lifted_at = ?2 WHERE buyer_user_id = ?1",
                params![renter.user_id, now],
            )
            .expect("lift overdue restriction");
            conn.execute(
                "UPDATE market_public_credit_policies
                 SET limit_minor = 100, revision = revision + 1, updated_at = ?2
                 WHERE supplier_user_id = ?1 AND currency = 'CNY'",
                params![owner.user_id, now],
            )
            .expect("lower paid credit");
            conn.execute(
                "UPDATE market_credit_accounts
                 SET status = 'active', balance_units = 8640000, updated_at = ?3
                 WHERE buyer_user_id = ?1 AND supplier_user_id = ?2 AND currency = 'CNY'",
                params![renter.user_id, owner.user_id, now],
            )
            .expect("raise accrued balance to lowered limit");
        }
        let catalog = store
            .share_market_catalog(Some(&renter), &active_subdomains)
            .await
            .expect("catalog after credit reduction");
        assert!(!can_rent_second(&catalog));
    }
}
