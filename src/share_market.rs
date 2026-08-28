use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Duration as StdDuration;

use crate::db::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params, params_from_iter,
};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ServerState;
use crate::client_market_trade::PaymentContact;
use crate::client_market_trade::PaymentMethod;
use crate::error::AppError;
use crate::models::{
    AuthSession, SHARE_CONTRACT_VERSION, ShareEditAvailableEvent, ShareGrantManager,
    ShareManagedGrantAction, ShareManagedGrantOperation, ShareSettingsPatch, ShareSupport,
    ShareTokenPeriod, ShareUserGrant, ShareUserPolicy,
};
use crate::store::AppStore;

const TRIAL_HOURS: i64 = crate::market_billing::TRIAL_SECONDS / 3_600;
const SERVICE_CYCLE_SECS: u64 = 5;
const MAX_SEATS_PER_LISTING: usize = 20;
pub(crate) const ERROR_SHARE_MARKET_PAYMENT_PROFILE_REQUIRED: &str =
    "SHARE_MARKET_PAYMENT_PROFILE_REQUIRED";
const MAX_SERVICE_DURATION_DAYS: u32 = 365;
const MAX_CONTROL_ATTEMPTS: i64 = 8;
const MAX_SUBSCRIPTIONS_PER_RECONCILE: usize = 200;
const MARKET_AGGREGATE_BATCH_SIZE: usize = 200;
const MARKET_PERFORMANCE_MAX_SAMPLES: i64 = 500;
const MARKET_PERFORMANCE_MIN_GENERATION_MS: i64 = 100;
const HEALTH_WINDOW_MINUTES: u32 = 24 * 60;
const CONTROL_DISPATCH_WAKE_RETRY_SECS: i64 = 30;
const CONTROL_EDIT_TTL_SECS: i64 = 5 * 60;
const CONTROL_RETRY_BASE_SECS: i64 = 15;
const CONTROL_RETRY_MAX_SECS: i64 = 15 * 60;
const RENT_QUOTE_TTL_SECS: i64 = 2 * 60;
const TERMINATION_QUOTE_TTL_SECS: i64 = 2 * 60;
const CONTRACT_REPAIR_WINDOW_HOURS: i64 = 24;
const RENT_QUOTE_RETENTION_DAYS: i64 = 7;
const MARKET_PERFORMANCE_WINDOW_HOURS: i64 = 24;
const MARKET_RELIABILITY_MIN_OBSERVED_MINUTES: u32 = 72;
const MANAGED_GRANT_MAX_FUTURE_SKEW_SECS: i64 = 5 * 60;
const SHARE_RUNTIME_FRESHNESS_SECS: i64 = 15 * 60;
const SHARE_MODEL_HEALTH_FRESHNESS_SECS: i64 = 20 * 60;
const SHARE_ROUTE_HEALTH_FRESHNESS_SECS: i64 = 90;
const SHARE_NEAR_EXPIRY_DAYS: i64 = 7;
const MARKET_ALLOWED_APPS: [&str; 3] = ["claude", "codex", "gemini"];
pub(crate) const SHARE_MARKET_CONTROL_ACTOR_EMAIL: &str = "share-market@router.internal";
pub(crate) const SHARE_REVISION_CONFLICT_CODE: &str = "cc_switch_share_revision_conflict";
const SHARE_GRANT_CONTRACT_VIOLATION_CODE: &str = "share_market_grant_contract_violation";

const SEAT_AVAILABLE: &str = "available";
const SEAT_RESERVED: &str = "reserved";
const SEAT_OCCUPIED: &str = "occupied";
const SEAT_REVOKING: &str = "revoking";
const SEAT_DISABLED: &str = "disabled";
const SEAT_DELETED: &str = "deleted";
const SEAT_RETIRED_VIEW: &str = "retired";

const ERROR_REOPEN_REQUIRED: &str = "share_market_reopen_required";
const ERROR_OTHER_ACTIVE_LISTING: &str = "share_market_other_active_listing";
const ERROR_LISTING_NOT_CLOSED: &str = "share_market_listing_not_closed";
const ERROR_SEAT_NOT_REOPENABLE: &str = "share_market_seat_not_reopenable";
const ERROR_OFFER_CHANGED: &str = "share_market_offer_changed";

const SUB_GRANT_PENDING: &str = "grant_pending";
const SUB_ACTIVE_FREE: &str = "active_free";
const SUB_ACTIVE_POSTPAID: &str = "active_postpaid";
const SUB_REVOKE_PENDING: &str = "revoke_pending";
const SUB_REVOKE_FAILED: &str = "revoke_failed";
const SUB_GRANT_FAILED: &str = "grant_failed";
const SUB_RELEASED: &str = "released";
const SUB_BILLING_SUSPEND_PENDING: &str = "billing_suspend_pending";
const SUB_BILLING_SUSPENDED: &str = "billing_suspended";
const SUB_BILLING_RESUME_PENDING: &str = "billing_resume_pending";
const SUB_BILLING_CONTROL_FAILED: &str = "billing_control_failed";

const PRICE_CHANGE_PENDING: &str = "pending";
const PRICE_CHANGE_ACCEPTED: &str = "accepted";
const PRICE_CHANGE_REJECTED: &str = "rejected";
const PRICE_CHANGE_CANCELLED: &str = "cancelled";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketCatalog {
    pub listings: Vec<ListingView>,
    #[serde(skip)]
    pub my_subscriptions: Vec<SubscriptionView>,
    pub trial_hours: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketOwnedListings {
    pub listings: Vec<ListingView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketSubscriptions {
    pub subscriptions: Vec<SubscriptionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareControlOperationSummary {
    pub pending: i64,
    pub dispatched: i64,
    pub dead_lettered: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareControlDeadLetterView {
    pub id: String,
    pub share_id: String,
    pub subscription_id: String,
    pub action: String,
    pub email: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub dead_lettered_at: String,
    pub can_requeue: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareControlDeadLetterPage {
    pub operations: Vec<ShareControlDeadLetterView>,
    pub summary: ShareControlOperationSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubscriptionPageQuery {
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlDeadLetterQuery {
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DescendingCursor {
    timestamp: String,
    id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketProviderQuotaTier {
    pub label: String,
    pub utilization: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketProviderQuota {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_period_end: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<ShareMarketProviderQuotaTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketAppCapability {
    pub app: String,
    pub provider_family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_level: Option<String>,
    pub model_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    pub health_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<ShareMarketProviderQuota>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketPerformance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_ttft_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_tps: Option<f64>,
    pub recent_request_count: u32,
    pub ttft_sample_count: u32,
    pub tps_sample_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_sample_at: Option<String>,
    pub window_hours: u32,
}

impl Default for ShareMarketPerformance {
    fn default() -> Self {
        Self {
            average_ttft_ms: None,
            average_tps: None,
            recent_request_count: 0,
            ttft_sample_count: 0,
            tps_sample_count: 0,
            latest_sample_at: None,
            window_hours: u32::try_from(MARKET_PERFORMANCE_WINDOW_HOURS).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketReliability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_rate_24h: Option<f64>,
    pub observed_minutes_24h: u32,
    pub observation_coverage_24h: f64,
    pub sufficient_coverage: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_observed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingView {
    pub id: String,
    pub share_id: String,
    pub installation_id: String,
    pub share_name: String,
    pub app_type: String,
    pub supported_apps: Vec<String>,
    pub provider_family: String,
    pub provider_families: Vec<String>,
    pub app_capabilities: Vec<ShareMarketAppCapability>,
    pub owner_email: String,
    pub status: String,
    pub share_status: String,
    pub subdomain: String,
    pub share_online: bool,
    pub service_state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_reasons: Vec<String>,
    pub is_owner: bool,
    pub can_delete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_blocked_reason: Option<String>,
    pub can_reopen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopen_blocked_reason: Option<String>,
    pub reopenable_seat_count: u32,
    #[serde(skip)]
    pub publicly_listed: bool,
    #[serde(default)]
    pub contacts: Vec<PaymentContact>,
    #[serde(default)]
    pub payment_method_kinds: Vec<String>,
    pub performance: ShareMarketPerformance,
    pub reliability: ShareMarketReliability,
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
    pub service_duration_days: Option<u32>,
    pub offer_revision: i64,
    pub is_free: bool,
    pub can_rent: bool,
    pub rent_prerequisites_met: bool,
    pub seller_approval_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rent_block_reason: Option<String>,
    pub eligibility: crate::market_access::MarketEligibilityView,
    pub read_only: bool,
    pub can_delete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_blocked_reason: Option<String>,
    pub can_republish: bool,
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
    pub apps: Vec<String>,
    pub subdomain: String,
    pub share_online: bool,
    pub owner_email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub renter_email: String,
    pub status: String,
    pub integrity_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity_violated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination_adjustment: Option<TerminationAdjustmentSummaryView>,
    pub seat_position: i64,
    pub parallel_limit: Option<u32>,
    pub token_limit: Option<u64>,
    pub token_period: ShareTokenPeriod,
    pub daily_rate_minor: Option<i64>,
    pub currency: Option<String>,
    pub service_duration_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub offer_revision: i64,
    pub payment_method_kinds: Vec<String>,
    #[serde(default)]
    pub contacts: Vec<PaymentContact>,
    pub can_release: bool,
    pub can_force_revoke: bool,
    pub can_retry_grant: bool,
    pub can_propose_price_change: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_change: Option<PriceChangeView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminationAdjustmentSummaryView {
    pub id: String,
    pub status: String,
    pub currency: String,
    pub elapsed_bps: i64,
    pub refund_bps: i64,
    pub amount_minor: i64,
    pub unbilled_credit_minor: i64,
    pub invoice_credit_minor: i64,
    pub external_refund_minor: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_obligation_status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceChangeView {
    pub id: String,
    pub previous_daily_rate_minor: i64,
    pub proposed_daily_rate_minor: i64,
    pub currency: String,
    pub base_offer_revision: i64,
    pub status: String,
    pub can_accept: bool,
    pub can_reject: bool,
    pub can_cancel: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responded_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedShareView {
    pub share_id: String,
    pub share_name: String,
    pub app_type: String,
    pub subdomain: String,
    pub owner_email: String,
    pub supported_apps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub app_capabilities: Vec<ShareMarketAppCapability>,
    pub share_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<u64>,
    pub already_listed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_listing_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reopen_listing_id: Option<String>,
    pub has_active_rentals: bool,
    pub market_state: String,
    pub can_create_listing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_blocked_reason: Option<String>,
    pub free_access: bool,
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
    pub service_duration_days: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateListingRequest {
    pub share_id: String,
    pub seats: Vec<SeatInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReopenExistingSeatRequest {
    pub seat_id: String,
    pub offer_revision: i64,
    pub seat: SeatInput,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReopenListingRequest {
    #[serde(default)]
    pub existing_seats: Vec<ReopenExistingSeatRequest>,
    #[serde(default)]
    pub new_seats: Vec<SeatInput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReopenListingResponse {
    pub ok: bool,
    pub listing_id: String,
    pub reopened_seat_ids: Vec<String>,
    pub new_seat_ids: Vec<String>,
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

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuoteSeatRequest {
    #[serde(default)]
    pub required_app: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RentAppServiceSnapshot {
    pub app: String,
    pub provider_family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_binding_fingerprint: Option<String>,
    pub model_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RentServiceSnapshot {
    pub schema_version: u32,
    /// Compatibility-only primary App for v1 consumers. It never narrows the
    /// Share-wide market entitlement in schema v2.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub required_app: String,
    #[serde(default)]
    pub supported_apps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<RentAppServiceSnapshot>,
    /// Compatibility projection of the primary App's service fields.
    #[serde(default)]
    pub provider_family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    /// Opaque, Router-internal identity for the exact bound Provider. It is
    /// persisted in the quote but removed from the public response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_binding_fingerprint: Option<String>,
    #[serde(default)]
    pub model_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_parallel_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_token_limit: Option<u64>,
    pub share_tokens_used: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShareMarketServiceAvailability {
    pub state: String,
    pub blocking_reason: Option<String>,
    pub reasons: Vec<String>,
}

impl ShareMarketServiceAvailability {
    fn available(&self) -> bool {
        self.blocking_reason.is_none()
    }
}

#[derive(Debug, Clone)]
struct ShareAvailabilityRecord {
    share_status: String,
    expires_at: String,
    contract_version: i64,
    app_type: String,
    parallel_limit: i64,
    runtime_refreshed_at: Option<String>,
    bindings_json: String,
    app_runtimes_json: Option<String>,
    app_providers_json: Option<String>,
    support: ShareSupport,
    token_limit: i64,
    tokens_used: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RentQuoteSnapshot {
    pub seat_id: String,
    pub listing_id: String,
    pub share_id: String,
    pub share_name: String,
    pub owner_email: String,
    pub seat_position: i64,
    pub parallel_limit: Option<u32>,
    pub token_limit: Option<u64>,
    pub token_period: ShareTokenPeriod,
    pub daily_rate_minor: Option<i64>,
    pub currency: Option<String>,
    pub service_duration_days: Option<u32>,
    pub offer_revision: i64,
    pub service: RentServiceSnapshot,
}

#[derive(Debug, Clone, PartialEq)]
struct RentOfferTerms {
    owner_email: String,
    seat_position: i64,
    parallel_limit: Option<u32>,
    token_limit: Option<u64>,
    token_period: ShareTokenPeriod,
    daily_rate_minor: Option<i64>,
    currency: Option<String>,
    service_duration_days: Option<u32>,
}

impl RentOfferTerms {
    fn from_snapshot(snapshot: &RentQuoteSnapshot) -> Self {
        Self {
            owner_email: snapshot.owner_email.to_ascii_lowercase(),
            seat_position: snapshot.seat_position,
            parallel_limit: snapshot.parallel_limit,
            token_limit: snapshot.token_limit,
            token_period: snapshot.token_period,
            daily_rate_minor: snapshot.daily_rate_minor,
            currency: snapshot.currency.clone(),
            service_duration_days: snapshot.service_duration_days,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RentQuoteView {
    pub id: String,
    pub status: String,
    pub expires_at: String,
    pub trial_seconds_remaining: i64,
    pub offer: RentQuoteSnapshot,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitRentQuoteRequest {
    pub quote_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRentQuoteResponse {
    pub ok: bool,
    pub subscription_id: String,
    pub replayed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForceRevokeRequest {
    #[serde(default)]
    pub deny_future_access: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminationQuoteView {
    pub id: String,
    pub subscription_id: String,
    pub status: String,
    pub expires_at: String,
    pub calculation: crate::market_billing::SupplierTerminationCalculation,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitTerminationRequest {
    pub quote_id: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub deny_future_access: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitTerminationResponse {
    pub ok: bool,
    pub subscription_id: String,
    pub replayed: bool,
    pub adjustment: crate::market_billing::SupplierTerminationAdjustment,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposePriceChangeRequest {
    pub daily_rate_minor: i64,
    pub offer_revision: i64,
}

#[derive(Debug, Clone)]
struct NormalizedSeat {
    parallel_limit: Option<u32>,
    token_limit: Option<u64>,
    token_period: ShareTokenPeriod,
    daily_rate_minor: Option<i64>,
    currency: Option<String>,
    service_duration_days: Option<u32>,
}

impl NormalizedSeat {
    fn is_free(&self) -> bool {
        self.daily_rate_minor.is_none()
    }
}

#[derive(Debug, Clone, Copy)]
enum PriceChangeAction {
    Accept,
    Reject,
    Cancel,
}

impl PriceChangeAction {
    fn target_status(self) -> &'static str {
        match self {
            Self::Accept => PRICE_CHANGE_ACCEPTED,
            Self::Reject => PRICE_CHANGE_REJECTED,
            Self::Cancel => PRICE_CHANGE_CANCELLED,
        }
    }

    fn event_type(self) -> &'static str {
        match self {
            Self::Accept => "price_change_accepted",
            Self::Reject => "price_change_rejected",
            Self::Cancel => "price_change_cancelled",
        }
    }

    fn resolution_reason(self) -> Option<&'static str> {
        match self {
            Self::Accept => None,
            Self::Reject => Some("renter_rejected"),
            Self::Cancel => Some("owner_cancelled"),
        }
    }
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/share-market/listings",
            get(list_catalog).post(create_listing),
        )
        .route("/v1/share-market/me/listings", get(list_my_listings))
        .route(
            "/v1/share-market/me/subscriptions",
            get(list_my_subscriptions),
        )
        .route("/v1/share-market/owned-shares", get(list_owned_shares))
        .route("/v1/share-market/listings/:id", delete(close_listing))
        .route("/v1/share-market/listings/:id/delete", post(delete_listing))
        .route("/v1/share-market/listings/:id/reopen", post(reopen_listing))
        .route("/v1/share-market/listings/:id/seats", post(add_seat))
        .route(
            "/v1/share-market/seats/:id",
            patch(update_seat).delete(delete_seat),
        )
        .route("/v1/share-market/seats/:id/quote", post(quote_seat))
        .route("/v1/share-market/seats/:id/rent", post(rent_seat))
        .route(
            "/v1/share-market/subscriptions/:id/release",
            post(release_subscription),
        )
        .route(
            "/v1/share-market/subscriptions/:id/force-revoke",
            post(force_revoke_subscription),
        )
        .route(
            "/v1/share-market/subscriptions/:id/retry-grant",
            post(retry_grant_subscription),
        )
        .route(
            "/v1/share-market/subscriptions/:id/termination-quote",
            post(quote_subscription_termination),
        )
        .route(
            "/v1/share-market/subscriptions/:id/terminate",
            post(commit_subscription_termination),
        )
        .route(
            "/v1/share-market/subscriptions/:id/price-changes",
            post(propose_price_change),
        )
        .route(
            "/v1/share-market/price-changes/:id/accept",
            post(accept_price_change),
        )
        .route(
            "/v1/share-market/price-changes/:id/reject",
            post(reject_price_change),
        )
        .route(
            "/v1/share-market/price-changes/:id/cancel",
            post(cancel_price_change),
        )
        .route(
            "/v1/admin/share-market/control-operations/dead-letters",
            get(list_control_dead_letters),
        )
        .route(
            "/v1/admin/share-market/control-operations/:id/requeue",
            post(requeue_control_dead_letter),
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
        .service_duration_days
        .is_some_and(|days| !(1..=MAX_SERVICE_DURATION_DAYS).contains(&days))
    {
        return Err(AppError::BadRequest(format!(
            "serviceDurationDays must be between 1 and {MAX_SERVICE_DURATION_DAYS}, or null for permanent"
        )));
    }
    let token_period = if input.token_limit.is_some() {
        input.token_period
    } else {
        ShareTokenPeriod::Lifetime
    };
    let currency = input
        .currency
        .map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| !value.is_empty());
    let pricing_empty = daily_rate_minor.is_none() && currency.is_none();
    if pricing_empty {
        return Ok(NormalizedSeat {
            parallel_limit: input.parallel_limit,
            token_limit: input.token_limit,
            token_period,
            daily_rate_minor: None,
            currency: None,
            service_duration_days: input.service_duration_days,
        });
    }
    let daily_rate_minor = daily_rate_minor.ok_or_else(|| {
        AppError::BadRequest("daily price and currency must both be set or both be empty".into())
    })?;
    if daily_rate_minor <= 0 || daily_rate_minor > crate::market_billing::MAX_DAILY_RATE_MINOR {
        return Err(AppError::BadRequest(
            "paid seat daily price is outside the supported range".into(),
        ));
    }
    let currency = currency.unwrap_or_else(|| crate::market_billing::MARKET_CURRENCY.into());
    if currency != crate::market_billing::MARKET_CURRENCY {
        return Err(AppError::BadRequest("currency must be USD".into()));
    }
    Ok(NormalizedSeat {
        parallel_limit: input.parallel_limit,
        token_limit: input.token_limit,
        token_period,
        daily_rate_minor: Some(daily_rate_minor),
        currency: Some(currency),
        service_duration_days: input.service_duration_days,
    })
}

fn validate_seat_share_capacity(
    seat: &NormalizedSeat,
    share_parallel_limit: i64,
) -> Result<(), AppError> {
    let Some(seat_parallel_limit) = seat.parallel_limit else {
        return Ok(());
    };
    if share_parallel_limit >= 0
        && u64::from(seat_parallel_limit) > u64::try_from(share_parallel_limit).unwrap_or_default()
    {
        return Err(AppError::BadRequest(format!(
            "seat concurrency {seat_parallel_limit} exceeds Share concurrency {share_parallel_limit}"
        )));
    }
    Ok(())
}

fn ensure_share_market_app_scope_supported_tx(
    conn: &Connection,
    share_id: &str,
) -> Result<(), AppError> {
    let version = conn
        .query_row(
            "SELECT COALESCE(share_access_policy_version, 0) FROM shares WHERE share_id = ?1",
            params![share_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(map_db("read Share contract version"))?
        .ok_or_else(|| AppError::NotFound("Share not found".into()))?;
    if version < i64::from(SHARE_CONTRACT_VERSION) {
        return Err(AppError::coded_conflict(
            "share_market_client_upgrade_required",
            "the Share owner must upgrade cc-switch-server before accepting market rentals",
            serde_json::json!({
                "reason": "share_contract_upgrade_required",
                "currentVersion": version,
                "requiredVersion": SHARE_CONTRACT_VERSION,
            }),
        ));
    }
    Ok(())
}

fn ensure_share_total_tokens_available(token_limit: i64, tokens_used: i64) -> Result<(), AppError> {
    if token_limit >= 0 && tokens_used.max(0) >= token_limit {
        return Err(AppError::coded_conflict(
            "share_market_share_tokens_exhausted",
            "the Share total token pool is exhausted",
            serde_json::json!({
                "reason": "share_tokens_exhausted",
                "shareTokenLimit": token_limit,
                "shareTokensUsed": tokens_used.max(0),
            }),
        ));
    }
    Ok(())
}

pub(crate) fn validate_market_protected_share_patch_tx(
    conn: &Connection,
    share_id: &str,
    patch: &ShareSettingsPatch,
) -> Result<(), AppError> {
    let rows = conn
        .prepare(
            "SELECT required_app, contract_apps_json, service_snapshot_json,
                    parallel_limit, expires_at
             FROM share_market_subscriptions
             WHERE share_id = ?1 AND status NOT IN ('released', 'grant_failed')",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![share_id], |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read protected Share Market contracts"))?;
    if rows.is_empty() {
        return Ok(());
    }
    let protected = |field: &str, reason: &str, app: Option<&str>| {
        AppError::coded_conflict(
            "share_market_contract_settings_protected",
            "this Share setting is protected by an active rental contract",
            serde_json::json!({ "field": field, "reason": reason, "app": app }),
        )
    };
    for (
        required_app,
        contract_apps_json,
        snapshot_json,
        seat_parallel_limit,
        subscription_expires_at,
    ) in rows
    {
        let snapshot = serde_json::from_str::<RentServiceSnapshot>(&snapshot_json).ok();
        let mut contract_apps = serde_json::from_str::<Vec<String>>(&contract_apps_json)
            .map(canonical_market_apps)
            .unwrap_or_default();
        if contract_apps.is_empty()
            && let Some(required_app) =
                required_app.filter(|app| MARKET_ALLOWED_APPS.contains(&app.as_str()))
        {
            contract_apps.push(required_app);
        }
        if let Some(support) = patch.support.as_ref()
            && let Some(app) = contract_apps
                .iter()
                .find(|app| !share_supports_app(support, app))
        {
            return Err(protected("support", "contract_app_in_use", Some(app)));
        }
        if let Some(next_parallel_limit) = patch.parallel_limit
            && next_parallel_limit >= 0
        {
            let required_parallel_limit = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.share_parallel_limit.map(i64::from))
                .or(seat_parallel_limit);
            if required_parallel_limit.is_none_or(|required| next_parallel_limit < required) {
                return Err(protected(
                    "parallelLimit",
                    "contract_capacity_reduction",
                    None,
                ));
            }
        }
        if let Some(next_token_limit) = patch.token_limit
            && next_token_limit >= 0
        {
            let required_token_limit = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.share_token_limit)
                .and_then(|value| i64::try_from(value).ok());
            if required_token_limit.is_none_or(|required| next_token_limit < required) {
                return Err(protected("tokenLimit", "contract_capacity_reduction", None));
            }
        }
        if let Some(next_expires_at) = patch.expires_at.as_deref() {
            let next_expires_at = DateTime::parse_from_rfc3339(next_expires_at)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|_| AppError::BadRequest("expiresAt must be RFC3339".into()))?;
            let Some(contract_expires_at) = subscription_expires_at
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
            else {
                return Err(protected("expiresAt", "permanent_contract_in_use", None));
            };
            if next_expires_at < contract_expires_at {
                return Err(protected("expiresAt", "fixed_term_not_covered", None));
            }
        }
    }
    Ok(())
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated user session required".into()))
}

fn map_db(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
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

fn market_allowed_apps() -> Vec<String> {
    MARKET_ALLOWED_APPS
        .iter()
        .map(|app| (*app).to_string())
        .collect()
}

fn canonical_market_apps(apps: impl IntoIterator<Item = String>) -> Vec<String> {
    let apps = apps
        .into_iter()
        .map(|app| app.trim().to_ascii_lowercase())
        .collect::<HashSet<_>>();
    MARKET_ALLOWED_APPS
        .iter()
        .filter(|app| apps.contains(**app))
        .map(|app| (*app).to_string())
        .collect()
}

fn supported_share_apps(bindings_json: &str, fallback_app: &str) -> Vec<String> {
    let bindings =
        serde_json::from_str::<BTreeMap<String, String>>(bindings_json).unwrap_or_default();
    let mut apps = ["claude", "codex", "gemini"]
        .into_iter()
        .filter(|app| {
            bindings
                .get(*app)
                .is_some_and(|provider| !provider.trim().is_empty())
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    if apps.is_empty() && matches!(fallback_app, "claude" | "codex" | "gemini") {
        apps.push(fallback_app.to_string());
    }
    apps
}

fn share_supports_app(support: &ShareSupport, app: &str) -> bool {
    match app {
        "claude" => support.claude,
        "codex" => support.codex,
        "gemini" => support.gemini,
        _ => false,
    }
}

fn enabled_share_apps(
    bindings_json: &str,
    fallback_app: &str,
    support: &ShareSupport,
) -> Vec<String> {
    supported_share_apps(bindings_json, fallback_app)
        .into_iter()
        .filter(|app| share_supports_app(support, app))
        .collect()
}

fn normalized_provider_family_parts(
    app: &str,
    kind: &str,
    provider_type: Option<&str>,
    provider_name: Option<&str>,
) -> String {
    let typed_identity = [kind, provider_type.unwrap_or_default()]
        .join(" ")
        .to_ascii_lowercase();
    let named_identity = provider_name.unwrap_or_default().to_ascii_lowercase();
    let classify = |identity: &str| {
        let third_party_api = [
            "openai_compatible",
            "openai compatible",
            "openrouter",
            "ollama",
            "nvidia",
            "deepseek",
            "bedrock",
            "custom",
        ]
        .iter()
        .any(|marker| identity.contains(marker));
        if third_party_api {
            Some("api")
        } else if identity.contains("cursor") {
            Some("cursor")
        } else if identity.contains("kiro") {
            Some("kiro")
        } else if identity.contains("copilot") || identity.contains("github") {
            Some("copilot")
        } else if identity.contains("grok") || identity.contains("xai") || identity.contains("x.ai")
        {
            Some("xai")
        } else if identity.contains("anthropic") || identity.contains("claude") {
            Some("anthropic")
        } else if identity.contains("gemini")
            || identity.contains("google")
            || identity.contains("antigravity")
        {
            Some("google")
        } else if identity.contains("openai") || identity.contains("codex") {
            Some("openai")
        } else if identity.contains("compatible") {
            Some("api")
        } else {
            None
        }
    };
    if let Some(family) = classify(&typed_identity) {
        family
    } else if typed_identity.contains("official_oauth") {
        match app {
            "claude" => "anthropic",
            "codex" => "openai",
            "gemini" => "google",
            _ => "other",
        }
    } else {
        classify(&named_identity).unwrap_or("other")
    }
    .to_string()
}

fn normalized_provider_family(provider: &crate::models::ShareUpstreamProvider) -> String {
    normalized_provider_family_parts(
        &provider.app,
        &provider.kind,
        provider.provider_type.as_deref(),
        provider.provider_name.as_deref(),
    )
}

fn app_runtime<'a>(
    runtimes: &'a crate::models::ShareAppRuntimes,
    app: &str,
) -> Option<&'a crate::models::ShareUpstreamProvider> {
    match app {
        "claude" => runtimes.claude.as_ref(),
        "codex" => runtimes.codex.as_ref(),
        "gemini" => runtimes.gemini.as_ref(),
        _ => None,
    }
}

fn app_provider_candidates<'a>(
    providers: &'a crate::models::ShareAppProviders,
    app: &str,
) -> &'a [crate::models::ShareAppProvider] {
    match app {
        "claude" => providers.claude.as_slice(),
        "codex" => providers.codex.as_slice(),
        "gemini" => providers.gemini.as_slice(),
        _ => &[],
    }
}

fn provider_auth_failed(health: Option<&crate::models::ShareProviderHealth>) -> bool {
    health.is_some_and(|health| {
        matches!(health.last_status_code, Some(401 | 403))
            || health.reason.as_deref().is_some_and(|reason| {
                let reason = reason.to_ascii_lowercase();
                [
                    "unauthorized",
                    "forbidden",
                    "authentication_failed",
                    "authentication failed",
                    "invalid_api_key",
                    "invalid api key",
                    "invalid_token",
                    "expired_token",
                ]
                .iter()
                .any(|marker| reason.contains(marker))
            })
    })
}

fn required_app_hard_failure(
    required_app: &str,
    bindings_json: &str,
    app_runtimes_json: Option<&str>,
    app_providers_json: Option<&str>,
) -> Option<&'static str> {
    let bindings = match serde_json::from_str::<BTreeMap<String, String>>(bindings_json) {
        Ok(bindings) => bindings,
        Err(_) => return Some("binding_unresolved"),
    };
    let Some(binding_id) = bindings
        .get(required_app)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Some("binding_missing");
    };
    let runtimes = app_runtimes_json
        .and_then(|value| serde_json::from_str::<crate::models::ShareAppRuntimes>(value).ok())
        .unwrap_or_default();
    let runtime = app_runtime(&runtimes, required_app);
    let providers = app_providers_json
        .and_then(|value| serde_json::from_str::<crate::models::ShareAppProviders>(value).ok())
        .unwrap_or_default();
    let candidates = app_provider_candidates(&providers, required_app);
    let bound_provider = candidates.iter().find(|provider| provider.id == binding_id);
    if !candidates.is_empty() && bound_provider.is_none() {
        return Some("binding_unresolved");
    }
    if bound_provider.is_some_and(|provider| !provider.enabled) {
        return Some("provider_disabled");
    }
    if runtime.and_then(|provider| provider.quota_blocked) == Some(true)
        || bound_provider.and_then(|provider| provider.quota_blocked) == Some(true)
    {
        return Some("quota_blocked");
    }
    if provider_auth_failed(runtime.and_then(|provider| provider.health.as_ref()))
        || provider_auth_failed(bound_provider.and_then(|provider| provider.health.as_ref()))
    {
        return Some("authentication_failed");
    }
    None
}

fn load_share_availability_record_tx(
    conn: &Connection,
    share_id: &str,
) -> Result<Option<ShareAvailabilityRecord>, AppError> {
    conn.query_row(
        "SELECT share_status, expires_at, COALESCE(share_access_policy_version, 0),
                app_type, parallel_limit, runtime_refreshed_at,
                COALESCE(bindings_json, '{}'), app_runtimes_json,
                app_providers_json, COALESCE(enabled_claude, 0),
                COALESCE(enabled_codex, 0), COALESCE(enabled_gemini, 0),
                token_limit, COALESCE(tokens_used, 0)
         FROM shares WHERE share_id = ?1",
        params![share_id],
        |row| {
            Ok(ShareAvailabilityRecord {
                share_status: row.get(0)?,
                expires_at: row.get(1)?,
                contract_version: row.get(2)?,
                app_type: row.get(3)?,
                parallel_limit: row.get(4)?,
                runtime_refreshed_at: row.get(5)?,
                bindings_json: row.get(6)?,
                app_runtimes_json: row.get(7)?,
                app_providers_json: row.get(8)?,
                support: ShareSupport {
                    claude: row.get::<_, i64>(9)? != 0,
                    codex: row.get::<_, i64>(10)? != 0,
                    gemini: row.get::<_, i64>(11)? != 0,
                },
                token_limit: row.get(12)?,
                tokens_used: row.get(13)?,
            })
        },
    )
    .optional()
    .map_err(map_db("read Share Market service availability"))
}

fn parse_optional_runtime_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn confirmed_model_unavailable_tx(
    conn: &Connection,
    share_id: &str,
    required_app: &str,
    service: Option<&RentServiceSnapshot>,
    now: DateTime<Utc>,
) -> Result<(bool, bool), AppError> {
    let rows = conn
        .prepare(
            "SELECT requested_model, actual_model, last_status, recent_results_json,
                    last_checked_at
             FROM share_model_health_state
             WHERE share_id = ?1 AND app_type = ?2
               AND last_checked_at >= ?3",
        )
        .and_then(|mut statement| {
            statement
                .query_map(
                    params![
                        share_id,
                        required_app,
                        now.timestamp() - SHARE_MODEL_HEALTH_FRESHNESS_SECS
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read required App model health"))?;
    if rows.is_empty() {
        return Ok((false, false));
    }

    let relevant_models = service
        .map(|service| {
            let app_service = service.apps.iter().find(|item| item.app == required_app);
            let upstream_model = app_service
                .and_then(|item| item.upstream_model.as_ref())
                .or_else(|| {
                    (service.required_app == required_app)
                        .then_some(service.upstream_model.as_ref())
                        .flatten()
                });
            let models = app_service
                .map(|item| item.models.as_slice())
                .unwrap_or_else(|| {
                    if service.required_app == required_app {
                        service.models.as_slice()
                    } else {
                        &[]
                    }
                });
            upstream_model
                .into_iter()
                .chain(models.iter())
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let mut relevant = Vec::new();
    let mut app_level_unavailable = false;
    for (requested, actual, last_status, recent_json) in rows {
        let recent = serde_json::from_str::<Vec<String>>(&recent_json).unwrap_or_default();
        let unavailable = recent.len() >= 3
            && recent
                .iter()
                .all(|status| status.eq_ignore_ascii_case("failed"));
        let degraded = last_status.eq_ignore_ascii_case("failed")
            || recent
                .iter()
                .any(|status| status.eq_ignore_ascii_case("failed"));
        let requested_key = requested.trim().to_ascii_lowercase();
        let actual_key = actual.trim().to_ascii_lowercase();
        let app_level = requested_key == required_app && actual_key == required_app;
        if app_level && unavailable {
            app_level_unavailable = true;
        }
        if app_level
            || relevant_models.is_empty()
            || relevant_models.contains(&requested_key)
            || relevant_models.contains(&actual_key)
        {
            relevant.push((unavailable, degraded));
        }
    }
    if app_level_unavailable {
        return Ok((true, true));
    }
    if relevant.is_empty() {
        return Ok((false, false));
    }
    Ok((
        relevant.iter().all(|(unavailable, _)| *unavailable),
        relevant.iter().any(|(_, degraded)| *degraded),
    ))
}

fn push_reason(reasons: &mut Vec<String>, reason: impl Into<String>) {
    let reason = reason.into();
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

fn block_availability(
    blocking_reason: &mut Option<String>,
    reasons: &mut Vec<String>,
    reason: &str,
) {
    if blocking_reason.is_none() {
        *blocking_reason = Some(reason.to_string());
    }
    push_reason(reasons, reason);
}

fn evaluate_share_market_service_availability_tx(
    conn: &Connection,
    share_id: &str,
    required_app: &str,
    route_online: Option<bool>,
    integrity_state: Option<&str>,
    service: Option<&RentServiceSnapshot>,
    now: DateTime<Utc>,
) -> Result<ShareMarketServiceAvailability, AppError> {
    let Some(record) = load_share_availability_record_tx(conn, share_id)? else {
        return Ok(ShareMarketServiceAvailability {
            state: "unavailable".into(),
            blocking_reason: Some("share_missing".into()),
            reasons: vec!["share_missing".into()],
        });
    };
    let mut blocking_reason = None;
    let mut reasons = Vec::new();
    if route_online == Some(false) {
        block_availability(&mut blocking_reason, &mut reasons, "share_offline");
    }
    if record.share_status != "active" {
        block_availability(&mut blocking_reason, &mut reasons, "share_inactive");
    }
    let share_expires_at = DateTime::parse_from_rfc3339(&record.expires_at)
        .ok()
        .map(|value| value.with_timezone(&Utc));
    match share_expires_at {
        Some(expires_at) if expires_at <= now => {
            block_availability(&mut blocking_reason, &mut reasons, "share_expired")
        }
        Some(expires_at) if expires_at <= now + Duration::days(SHARE_NEAR_EXPIRY_DAYS) => {
            push_reason(&mut reasons, "share_near_expiry")
        }
        None => block_availability(&mut blocking_reason, &mut reasons, "share_expiry_invalid"),
        _ => {}
    }
    if record.contract_version < i64::from(SHARE_CONTRACT_VERSION) {
        block_availability(
            &mut blocking_reason,
            &mut reasons,
            "share_contract_upgrade_required",
        );
    }
    if !share_supports_app(&record.support, required_app) {
        block_availability(&mut blocking_reason, &mut reasons, "required_app_disabled");
    }
    let runtime_refreshed_at = parse_optional_runtime_time(record.runtime_refreshed_at.as_deref());
    if runtime_refreshed_at.is_none_or(|refreshed_at| {
        refreshed_at > now + Duration::minutes(5)
            || now - refreshed_at > Duration::seconds(SHARE_RUNTIME_FRESHNESS_SECS)
    }) {
        block_availability(&mut blocking_reason, &mut reasons, "runtime_stale");
    }
    if record.token_limit >= 0 && record.tokens_used.max(0) >= record.token_limit {
        block_availability(&mut blocking_reason, &mut reasons, "share_tokens_exhausted");
    }
    if let Some(reason) = required_app_hard_failure(
        required_app,
        &record.bindings_json,
        record.app_runtimes_json.as_deref(),
        record.app_providers_json.as_deref(),
    ) {
        block_availability(&mut blocking_reason, &mut reasons, reason);
    }
    let (model_unavailable, model_degraded) =
        confirmed_model_unavailable_tx(conn, share_id, required_app, service, now)?;
    if model_unavailable {
        block_availability(&mut blocking_reason, &mut reasons, "model_unavailable");
    } else if model_degraded {
        push_reason(&mut reasons, "model_degraded");
    }
    if integrity_state.is_some_and(|state| state != "compatible") {
        block_availability(&mut blocking_reason, &mut reasons, "contract_incompatible");
    }

    let capabilities = enabled_public_app_capabilities(
        &record.bindings_json,
        record.app_runtimes_json.as_deref(),
        record.app_providers_json.as_deref(),
        required_app,
        &record.support,
    );
    if capabilities
        .iter()
        .find(|capability| capability.app == required_app)
        .is_some_and(|capability| {
            matches!(capability.health_state.as_str(), "degraded" | "unavailable")
        })
    {
        push_reason(&mut reasons, "provider_degraded");
    }
    let state = if blocking_reason.is_some() {
        "unavailable"
    } else if reasons.is_empty() {
        "available"
    } else {
        "degraded"
    };
    Ok(ShareMarketServiceAvailability {
        state: state.into(),
        blocking_reason,
        reasons,
    })
}

fn availability_error(app: &str, availability: &ShareMarketServiceAvailability) -> AppError {
    let reason = availability
        .blocking_reason
        .as_deref()
        .unwrap_or("share_unavailable");
    let code = match reason {
        "share_offline" => "share_market_share_offline",
        "share_expired" | "share_expiry_invalid" => "share_market_share_expired",
        "share_contract_upgrade_required" => "share_market_client_upgrade_required",
        "runtime_stale" => "share_market_runtime_stale",
        "share_tokens_exhausted" => "share_market_share_tokens_exhausted",
        "model_unavailable" => "share_market_model_unavailable",
        "contract_incompatible" => "share_market_contract_incompatible",
        _ => "share_market_required_app_unavailable",
    };
    AppError::coded_conflict(
        code,
        "the selected Share service is currently unavailable",
        serde_json::json!({
            "reason": reason,
            "app": app,
            "serviceState": availability.state,
            "serviceReasons": availability.reasons,
        }),
    )
}

fn parse_provider_expiry(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn required_app_upstream_expiry(
    record: &ShareAvailabilityRecord,
    required_app: &str,
) -> Option<DateTime<Utc>> {
    let bindings = serde_json::from_str::<BTreeMap<String, String>>(&record.bindings_json).ok()?;
    let binding_id = bindings.get(required_app)?.trim();
    if binding_id.is_empty() {
        return None;
    }
    let runtimes = record
        .app_runtimes_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<crate::models::ShareAppRuntimes>(value).ok())
        .unwrap_or_default();
    let providers = record
        .app_providers_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<crate::models::ShareAppProviders>(value).ok())
        .unwrap_or_default();
    let runtime = app_runtime(&runtimes, required_app);
    let provider = app_provider_candidates(&providers, required_app)
        .iter()
        .find(|provider| provider.id == binding_id);
    let mut expiries = Vec::new();
    for expiry in [
        runtime.and_then(|value| parse_provider_expiry(value.subscription_expires_at.as_deref())),
        runtime.and_then(|value| {
            parse_provider_expiry(
                value
                    .quota
                    .as_ref()
                    .and_then(|quota| quota.subscription_period_end.as_deref()),
            )
        }),
        provider.and_then(|value| parse_provider_expiry(value.subscription_expires_at.as_deref())),
        provider.and_then(|value| {
            parse_provider_expiry(
                value
                    .quota
                    .as_ref()
                    .and_then(|quota| quota.subscription_period_end.as_deref()),
            )
        }),
    ]
    .into_iter()
    .flatten()
    {
        expiries.push(expiry);
    }
    let refreshed_at = parse_optional_runtime_time(record.runtime_refreshed_at.as_deref());
    for remaining_ms in [
        runtime.and_then(|value| value.subscription_remaining_ms),
        provider.and_then(|value| value.subscription_remaining_ms),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(refreshed_at) = refreshed_at
            && let Some(expiry) =
                refreshed_at.checked_add_signed(Duration::milliseconds(remaining_ms))
        {
            expiries.push(expiry);
        }
    }
    expiries.into_iter().min()
}

fn ensure_fixed_service_term_fulfillable_tx(
    conn: &Connection,
    share_id: &str,
    required_app: &str,
    service_duration_days: Option<u32>,
    service_starts_at: DateTime<Utc>,
) -> Result<(), AppError> {
    let Some(days) = service_duration_days else {
        return Ok(());
    };
    let service_ends_at = service_starts_at
        .checked_add_signed(Duration::days(i64::from(days)))
        .ok_or_else(|| AppError::Internal("Share service term overflowed".into()))?;
    let record = load_share_availability_record_tx(conn, share_id)?
        .ok_or_else(|| AppError::NotFound("Share not found".into()))?;
    let share_expires_at = DateTime::parse_from_rfc3339(&record.expires_at)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AppError::Internal("stored Share expiry is invalid".into()))?;
    let upstream_expires_at = required_app_upstream_expiry(&record, required_app);
    let blocking_expiry = std::iter::once(("share", share_expires_at))
        .chain(upstream_expires_at.map(|expiry| ("upstream_subscription", expiry)))
        .filter(|(_, expiry)| *expiry < service_ends_at)
        .min_by_key(|(_, expiry)| *expiry);
    if let Some((source, available_until)) = blocking_expiry {
        return Err(AppError::coded_conflict(
            "share_market_service_term_unfulfillable",
            "the Share cannot currently guarantee the complete fixed service term",
            serde_json::json!({
                "reason": "service_term_unfulfillable",
                "source": source,
                "requiredApp": required_app,
                "serviceStartsAt": service_starts_at.to_rfc3339(),
                "serviceEndsAt": service_ends_at.to_rfc3339(),
                "availableUntil": available_until.to_rfc3339(),
            }),
        ));
    }
    Ok(())
}

fn contract_policy(record: &SubscriptionRecord) -> Result<ShareUserPolicy, AppError> {
    let token_period: ShareTokenPeriod = serde_json::from_str(&record.token_period_json)
        .map_err(|_| AppError::Internal("stored seat token period is invalid".into()))?;
    let service_started_at = record
        .service_started_at
        .as_deref()
        .or(record.activated_at.as_deref())
        .map(parse_time)
        .transpose()?;
    Ok(ShareUserPolicy {
        parallel_limit: record
            .parallel_limit
            .and_then(|value| u32::try_from(value).ok()),
        token_limit: record
            .token_limit
            .and_then(|value| u64::try_from(value).ok()),
        token_period,
        token_period_anchor_at_ms: service_started_at
            .and_then(|started_at| token_period_anchor_at_ms(token_period, started_at)),
        expires_at: record
            .expires_at
            .as_deref()
            .map(parse_time)
            .transpose()?
            .map(|value| value.timestamp_millis()),
        allowed_apps: market_allowed_apps(),
    })
}

fn active_grant_has_contract_scope(grants_json: Option<&str>, record: &SubscriptionRecord) -> bool {
    active_entitlement_grant(grants_json, &record.entitlement_id)
        .is_some_and(|grant| grant.policy.allowed_apps == market_allowed_apps())
}

fn subscription_integrity_violations_tx(
    conn: &Connection,
    record: &SubscriptionRecord,
    now: DateTime<Utc>,
) -> Result<Vec<String>, AppError> {
    let Some(share) = load_share_availability_record_tx(conn, &record.share_id)? else {
        return Ok(vec!["share_missing".into()]);
    };
    let mut violations = Vec::new();
    if share.share_status != "active" {
        push_reason(&mut violations, "share_inactive");
    }
    if share.contract_version < i64::from(SHARE_CONTRACT_VERSION) {
        push_reason(&mut violations, "share_contract_upgrade_required");
    }
    let contract_apps = subscription_contract_apps(record);
    if contract_apps.is_empty() {
        push_reason(&mut violations, "contract_apps_missing");
    }
    if share.parallel_limit >= 0
        && record
            .parallel_limit
            .is_some_and(|limit| limit > share.parallel_limit)
    {
        push_reason(&mut violations, "share_parallel_capacity_reduced");
    }
    if let Some(service_ends_at) = record.expires_at.as_deref().map(parse_time).transpose()? {
        let share_expires_at = DateTime::parse_from_rfc3339(&share.expires_at)
            .ok()
            .map(|value| value.with_timezone(&Utc));
        if share_expires_at.is_none_or(|expires_at| expires_at < service_ends_at) {
            push_reason(&mut violations, "fixed_term_not_covered");
        }
        for app in &contract_apps {
            if required_app_upstream_expiry(&share, app)
                .is_some_and(|expires_at| expires_at < service_ends_at)
            {
                push_reason(&mut violations, "upstream_fixed_term_not_covered");
            }
        }
    }

    let bindings =
        serde_json::from_str::<BTreeMap<String, String>>(&share.bindings_json).unwrap_or_default();
    let providers = share
        .app_providers_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<crate::models::ShareAppProviders>(value).ok())
        .unwrap_or_default();
    let snapshot = serde_json::from_str::<RentServiceSnapshot>(
        &conn
            .query_row(
                "SELECT service_snapshot_json FROM share_market_subscriptions WHERE id = ?1",
                params![record.id],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_db("read immutable Share service snapshot"))?,
    )
    .ok();
    let capabilities = enabled_public_app_capabilities(
        &share.bindings_json,
        share.app_runtimes_json.as_deref(),
        share.app_providers_json.as_deref(),
        &share.app_type,
        &share.support,
    );
    if snapshot.as_ref().is_some_and(|snapshot| {
        snapshot.schema_version >= 2
            && canonical_market_apps(snapshot.supported_apps.clone()) != contract_apps
    }) {
        push_reason(&mut violations, "contract_apps_changed");
    }
    for app in &contract_apps {
        if !share_supports_app(&share.support, app) {
            push_reason(&mut violations, "required_app_disabled");
        }
        let binding_id = bindings
            .get(app)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        if binding_id.is_none() {
            push_reason(&mut violations, "binding_missing");
        }
        if let Some(binding_id) = binding_id {
            let candidates = app_provider_candidates(&providers, app);
            if !candidates.is_empty() {
                match candidates.iter().find(|provider| provider.id == binding_id) {
                    Some(provider) if !provider.enabled => {
                        push_reason(&mut violations, "provider_disabled")
                    }
                    None => push_reason(&mut violations, "binding_unresolved"),
                    _ => {}
                }
            }
        }
        let capability = capabilities
            .iter()
            .find(|capability| capability.app == *app);
        let current_binding_fingerprint = binding_id.map(|binding_id| {
            crate::api::sha256_hex(
                format!("share-market-provider-binding-v1\n{app}\n{binding_id}").as_bytes(),
            )
        });
        let expected = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.apps.iter().find(|service| service.app == *app));
        if let Some(expected) = expected {
            if expected.provider_binding_fingerprint != current_binding_fingerprint {
                push_reason(&mut violations, "provider_binding_changed");
            }
            match capability {
                Some(capability)
                    if expected.provider_family == capability.provider_family
                        && expected.provider_type == capability.provider_type
                        && expected.model_mode == capability.model_mode
                        && expected.upstream_model == capability.upstream_model
                        && expected.models == capability.models => {}
                Some(_) => push_reason(&mut violations, "provider_model_changed"),
                None => push_reason(&mut violations, "required_app_unavailable"),
            }
        } else if let Some(snapshot) = snapshot
            .as_ref()
            .filter(|snapshot| snapshot.schema_version == 1 && snapshot.required_app == *app)
        {
            if snapshot.provider_binding_fingerprint != current_binding_fingerprint {
                push_reason(&mut violations, "provider_binding_changed");
            }
            match capability {
                Some(capability)
                    if snapshot.provider_family == capability.provider_family
                        && snapshot.provider_type == capability.provider_type
                        && snapshot.model_mode == capability.model_mode
                        && snapshot.upstream_model == capability.upstream_model
                        && snapshot.models == capability.models => {}
                Some(_) => push_reason(&mut violations, "provider_model_changed"),
                None => push_reason(&mut violations, "required_app_unavailable"),
            }
        } else if snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.schema_version >= 2)
        {
            push_reason(&mut violations, "contract_apps_changed");
        } else if capability.is_none() {
            push_reason(&mut violations, "required_app_unavailable");
        }
    }

    if matches!(
        record.status.as_str(),
        SUB_ACTIVE_FREE | SUB_ACTIVE_POSTPAID
    ) {
        let grants_json = conn
            .query_row(
                "SELECT user_grants_json FROM shares WHERE share_id = ?1",
                params![record.share_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(map_db("read Share grant App scope"))?
            .flatten();
        if active_entitlement_grant(grants_json.as_deref(), &record.entitlement_id).is_none() {
            push_reason(&mut violations, "entitlement_missing");
        } else if share.contract_version >= i64::from(SHARE_CONTRACT_VERSION)
            && !active_grant_has_contract_scope(grants_json.as_deref(), record)
        {
            push_reason(&mut violations, "app_scope_not_enforced");
        }
    }
    let _ = now;
    Ok(violations)
}

fn begin_subscription_integrity_remediation_tx(
    conn: &Connection,
    record: &SubscriptionRecord,
    violations: &[String],
    now_dt: DateTime<Utc>,
) -> Result<(), AppError> {
    if violations.is_empty() || matches!(record.status.as_str(), SUB_RELEASED | SUB_GRANT_FAILED) {
        return Ok(());
    }
    let now = now_dt.to_rfc3339();
    let reason = violations.join(",");
    let first_violation = record.integrity_state == "compatible";
    conn.execute(
        "UPDATE share_market_subscriptions
         SET integrity_state = 'violated', integrity_reason = ?2,
             integrity_checked_at = ?3,
             integrity_violated_at = COALESCE(integrity_violated_at, ?3),
             updated_at = ?3 WHERE id = ?1 AND integrity_state != 'terminated'",
        params![record.id, reason, now],
    )
    .map_err(map_db("mark Share contract integrity violation"))?;
    if record.status == SUB_GRANT_PENDING {
        reject_unfulfillable_activation_tx(conn, record, &reason, &now)?;
        return Ok(());
    }
    crate::market_billing::suspend_contract_for_integrity_tx(
        conn, "share", &record.id, &reason, now_dt,
    )?;
    if matches!(
        record.status.as_str(),
        SUB_ACTIVE_FREE | SUB_ACTIVE_POSTPAID
    ) {
        enqueue_control_operation_tx(
            conn,
            &record.share_id,
            &record.id,
            &record.entitlement_id,
            "revoke",
            &record.renter_email,
            None,
            &now,
        )?;
        conn.execute(
            "UPDATE share_market_subscriptions
             SET status = 'billing_suspend_pending', release_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND status IN ('active_free', 'active_postpaid')",
            params![record.id, reason, now],
        )
        .map_err(map_db("isolate violated Share contract"))?;
    }
    if first_violation {
        event_tx(
            conn,
            Some(&record.listing_id),
            Some(&record.seat_id),
            Some(&record.id),
            None,
            "contract_integrity_violated",
            serde_json::json!({
                "reasons": violations,
                "repairDeadlineAt": (now_dt + Duration::hours(CONTRACT_REPAIR_WINDOW_HOURS)).to_rfc3339(),
            }),
            &now,
        )?;
    }
    Ok(())
}

fn is_pending_all_app_scope_upgrade(record: &SubscriptionRecord, violations: &[String]) -> bool {
    record.integrity_state == "compatible"
        && record.app_scope_enforced_at.is_none()
        && matches!(
            record.status.as_str(),
            SUB_ACTIVE_FREE | SUB_ACTIVE_POSTPAID
        )
        && violations == ["app_scope_not_enforced"]
}

fn enqueue_non_disruptive_app_scope_upgrade_tx(
    conn: &Connection,
    record: &SubscriptionRecord,
    now: &str,
) -> Result<(), AppError> {
    let policy = contract_policy(record)?;
    let operation_id = enqueue_control_operation_tx(
        conn,
        &record.share_id,
        &record.id,
        &record.entitlement_id,
        "upsert",
        &record.renter_email,
        Some(&policy),
        now,
    )?;
    let policy_json = serde_json::to_string(&policy)
        .map_err(|error| AppError::Internal(format!("encode all-App grant failed: {error}")))?;
    conn.execute(
        "UPDATE share_control_operations
         SET policy_json = ?2, updated_at = ?3
         WHERE id = ?1 AND status = 'pending'",
        params![operation_id, policy_json, now],
    )
    .map_err(map_db("refresh pending all-App grant"))?;
    conn.execute(
        "UPDATE share_market_subscriptions
         SET integrity_checked_at = ?2, updated_at = ?2
         WHERE id = ?1 AND integrity_state = 'compatible'",
        params![record.id, now],
    )
    .map_err(map_db("record all-App grant upgrade attempt"))?;
    Ok(())
}

pub(crate) fn reconcile_share_contract_integrity_tx(
    conn: &Connection,
    share_id: &str,
    now_dt: DateTime<Utc>,
) -> Result<(), AppError> {
    let subscription_ids = conn
        .prepare(
            "SELECT id FROM share_market_subscriptions
             WHERE share_id = ?1 AND status NOT IN ('released', 'grant_failed')",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![share_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read Share contracts for integrity check"))?;
    for subscription_id in subscription_ids {
        let Some(record) = subscription_record(conn, &subscription_id)? else {
            continue;
        };
        let violations = subscription_integrity_violations_tx(conn, &record, now_dt)?;
        if is_pending_all_app_scope_upgrade(&record, &violations) {
            enqueue_non_disruptive_app_scope_upgrade_tx(conn, &record, &now_dt.to_rfc3339())?;
            continue;
        }
        begin_subscription_integrity_remediation_tx(conn, &record, &violations, now_dt)?;
    }
    Ok(())
}

fn reconcile_subscription_integrity_tx(
    conn: &Connection,
    record: &SubscriptionRecord,
    now_dt: DateTime<Utc>,
) -> Result<bool, AppError> {
    let violations = subscription_integrity_violations_tx(conn, record, now_dt)?;
    if is_pending_all_app_scope_upgrade(record, &violations) {
        enqueue_non_disruptive_app_scope_upgrade_tx(conn, record, &now_dt.to_rfc3339())?;
        return Ok(true);
    }
    if record.integrity_state == "terminated" {
        return Ok(true);
    }
    if record.integrity_state != "compatible" {
        let violated_at = record
            .integrity_violated_at
            .as_deref()
            .map(parse_time)
            .transpose()?
            .unwrap_or(now_dt);
        if now_dt >= violated_at + Duration::hours(CONTRACT_REPAIR_WINDOW_HOURS) {
            let reason = "contract_integrity_repair_timeout";
            if record.daily_rate_minor.is_some()
                && record.service_duration_days.is_some()
                && record.service_started_at.is_some()
            {
                crate::market_billing::apply_supplier_early_termination_refund_tx(
                    conn,
                    "share",
                    &record.id,
                    reason,
                    &format!(
                        "share-integrity-timeout:{}:{}",
                        record.id,
                        violated_at.to_rfc3339()
                    ),
                    violated_at,
                )?;
            }
            terminate_subscription_for_supplier_tx(
                conn,
                &record.id,
                reason,
                None,
                false,
                &now_dt.to_rfc3339(),
            )?;
            return Ok(true);
        }
    }
    if !violations.is_empty() {
        begin_subscription_integrity_remediation_tx(conn, record, &violations, now_dt)?;
        return Ok(true);
    }
    if record.integrity_state == "compatible" {
        let grants_json = conn
            .query_row(
                "SELECT user_grants_json FROM shares WHERE share_id = ?1",
                params![record.share_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(map_db("read compatible Share App scope"))?
            .flatten();
        if active_grant_has_contract_scope(grants_json.as_deref(), record) {
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET integrity_checked_at = ?2,
                     app_scope_enforced_at = COALESCE(app_scope_enforced_at, ?2)
                 WHERE id = ?1",
                params![record.id, now_dt.to_rfc3339()],
            )
            .map_err(map_db("confirm Share App scope"))?;
        }
        return Ok(false);
    }
    if record.status == SUB_BILLING_SUSPENDED && !record.has_active_control_work {
        if record.daily_rate_minor.is_some()
            && !crate::market_billing::request_contract_resume_after_integrity_tx(
                conn,
                "share",
                &record.id,
                &now_dt.to_rfc3339(),
            )?
        {
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET integrity_state = 'compatible', integrity_reason = NULL,
                     integrity_checked_at = ?2, integrity_violated_at = NULL,
                     release_reason = 'billing_resume_blocked', updated_at = ?2
                 WHERE id = ?1 AND status = 'billing_suspended'",
                params![record.id, now_dt.to_rfc3339()],
            )
            .map_err(map_db(
                "complete Share integrity repair while billing remains suspended",
            ))?;
            event_tx(
                conn,
                Some(&record.listing_id),
                Some(&record.seat_id),
                Some(&record.id),
                None,
                "contract_integrity_restored",
                serde_json::json!({ "serviceRemainsSuspended": true }),
                &now_dt.to_rfc3339(),
            )?;
            return Ok(true);
        }
        let policy = contract_policy(record)?;
        enqueue_control_operation_tx(
            conn,
            &record.share_id,
            &record.id,
            &record.entitlement_id,
            "upsert",
            &record.renter_email,
            Some(&policy),
            &now_dt.to_rfc3339(),
        )?;
        conn.execute(
            "UPDATE share_market_subscriptions
             SET status = 'billing_resume_pending', integrity_state = 'remediating',
                 release_reason = NULL, integrity_checked_at = ?2, updated_at = ?2
             WHERE id = ?1 AND status = 'billing_suspended'",
            params![record.id, now_dt.to_rfc3339()],
        )
        .map_err(map_db("resume remediated Share contract"))?;
        event_tx(
            conn,
            Some(&record.listing_id),
            Some(&record.seat_id),
            Some(&record.id),
            None,
            "contract_integrity_repair_started",
            serde_json::json!({}),
            &now_dt.to_rfc3339(),
        )?;
        return Ok(true);
    }
    if matches!(
        record.status.as_str(),
        SUB_ACTIVE_FREE | SUB_ACTIVE_POSTPAID
    ) {
        let grants_json = conn
            .query_row(
                "SELECT user_grants_json FROM shares WHERE share_id = ?1",
                params![record.share_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(map_db("read repaired Share App scope"))?
            .flatten();
        if active_grant_has_contract_scope(grants_json.as_deref(), record) {
            if record.daily_rate_minor.is_some()
                && !crate::market_billing::complete_contract_resume_after_integrity_tx(
                    conn,
                    "share",
                    &record.id,
                    &now_dt.to_rfc3339(),
                )?
            {
                enqueue_control_operation_tx(
                    conn,
                    &record.share_id,
                    &record.id,
                    &record.entitlement_id,
                    "revoke",
                    &record.renter_email,
                    None,
                    &now_dt.to_rfc3339(),
                )?;
                conn.execute(
                    "UPDATE share_market_subscriptions
                     SET status = 'billing_suspend_pending', integrity_state = 'compatible',
                         integrity_reason = NULL, integrity_checked_at = ?2,
                         integrity_violated_at = NULL, release_reason = 'billing_resume_blocked',
                         updated_at = ?2 WHERE id = ?1",
                    params![record.id, now_dt.to_rfc3339()],
                )
                .map_err(map_db(
                    "re-isolate repaired Share after billing resume was blocked",
                ))?;
                event_tx(
                    conn,
                    Some(&record.listing_id),
                    Some(&record.seat_id),
                    Some(&record.id),
                    None,
                    "contract_integrity_restored",
                    serde_json::json!({ "serviceRemainsSuspended": true }),
                    &now_dt.to_rfc3339(),
                )?;
                return Ok(true);
            }
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET integrity_state = 'compatible', integrity_reason = NULL,
                     integrity_checked_at = ?2, integrity_violated_at = NULL,
                     app_scope_enforced_at = COALESCE(app_scope_enforced_at, ?2),
                     updated_at = ?2 WHERE id = ?1",
                params![record.id, now_dt.to_rfc3339()],
            )
            .map_err(map_db("complete Share contract integrity repair"))?;
            event_tx(
                conn,
                Some(&record.listing_id),
                Some(&record.seat_id),
                Some(&record.id),
                None,
                "contract_integrity_restored",
                serde_json::json!({}),
                &now_dt.to_rfc3339(),
            )?;
        }
    }
    Ok(false)
}

pub(crate) fn share_market_billing_observation_tx(
    conn: &Connection,
    subscription_id: &str,
    observed_at: DateTime<Utc>,
) -> Result<Option<(i64, String, String)>, AppError> {
    let subscription = conn
        .query_row(
            "SELECT share_id, required_app, contract_apps_json,
                    service_snapshot_json, integrity_state
             FROM share_market_subscriptions
             WHERE id = ?1 AND status = 'active_postpaid'",
            params![subscription_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read billable Share Market subscription"))?;
    let Some((share_id, required_app, contract_apps_json, snapshot_json, integrity_state)) =
        subscription
    else {
        return Ok(None);
    };
    let mut contract_apps = serde_json::from_str::<Vec<String>>(&contract_apps_json)
        .map(canonical_market_apps)
        .unwrap_or_default();
    if contract_apps.is_empty()
        && let Some(required_app) =
            required_app.filter(|app| MARKET_ALLOWED_APPS.contains(&app.as_str()))
    {
        contract_apps.push(required_app);
    }
    if contract_apps.is_empty() {
        // Legacy contracts predate App-scoped availability and retain the old,
        // route-only conservative billing behavior until they naturally end.
        return conn
            .query_row(
                "SELECT checked_at, status, reason FROM share_health_checks
                 WHERE share_id = ?1 AND checked_at <= ?2
                 ORDER BY checked_at DESC LIMIT 1",
                params![share_id, observed_at.timestamp()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(map_db("read legacy Share billing observation"));
    };
    let route = conn
        .query_row(
            "SELECT checked_at, status FROM share_health_checks
             WHERE share_id = ?1 AND checked_at <= ?2
             ORDER BY checked_at DESC LIMIT 1",
            params![share_id, observed_at.timestamp()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(map_db("read Share route billing observation"))?;
    let route_online = route.as_ref().is_some_and(|(checked_at, status)| {
        status == "healthy"
            && observed_at.timestamp().saturating_sub(*checked_at)
                <= SHARE_ROUTE_HEALTH_FRESHNESS_SECS
    });
    let service = serde_json::from_str::<RentServiceSnapshot>(&snapshot_json).ok();
    let mut combined_status = "healthy";
    let mut combined_reason = "share_market_service_available".to_string();
    for app in contract_apps {
        let availability = evaluate_share_market_service_availability_tx(
            conn,
            &share_id,
            &app,
            Some(route_online),
            Some(&integrity_state),
            service.as_ref(),
            observed_at,
        )?;
        let reason = availability
            .blocking_reason
            .clone()
            .or_else(|| availability.reasons.first().cloned())
            .unwrap_or_else(|| "share_market_service_available".into());
        if !availability.available()
            && !matches!(
                availability.blocking_reason.as_deref(),
                Some("runtime_stale" | "share_offline")
            )
        {
            combined_status = "unhealthy";
            combined_reason = reason;
            break;
        }
        if !availability.available() && combined_status == "healthy" {
            combined_status = "unknown";
            combined_reason = reason;
        }
    }
    Ok(Some((
        observed_at.timestamp(),
        combined_status.into(),
        combined_reason,
    )))
}

fn first_nonempty<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn public_provider_quota(
    quota: Option<&crate::models::ShareUpstreamQuota>,
    subscription_period_end: Option<&str>,
) -> Option<ShareMarketProviderQuota> {
    let status = quota
        .map(|value| value.status.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let plan = quota
        .and_then(|value| value.plan.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let subscription_period_end = quota
        .and_then(|value| value.subscription_period_end.as_deref())
        .or(subscription_period_end)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let tiers = quota
        .map(|value| {
            value
                .tiers
                .iter()
                .filter(|tier| !tier.label.trim().is_empty())
                .map(|tier| ShareMarketProviderQuotaTier {
                    label: tier.label.trim().to_string(),
                    utilization: tier.utilization,
                    resets_at: tier.resets_at.clone(),
                    used: tier.used,
                    limit: tier.limit,
                    unit: tier.unit.clone(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if status.is_none() && plan.is_none() && subscription_period_end.is_none() && tiers.is_empty() {
        return None;
    }
    Some(ShareMarketProviderQuota {
        status,
        plan,
        subscription_period_end,
        tiers,
    })
}

fn public_provider_health_state(
    available: Option<bool>,
    quota_blocked: Option<bool>,
    healthy: Option<bool>,
    enabled: Option<bool>,
    auth_failed: bool,
) -> String {
    if auth_failed
        || available == Some(false)
        || quota_blocked == Some(true)
        || enabled == Some(false)
    {
        return "unavailable".into();
    }
    if healthy == Some(false) {
        return "degraded".into();
    }
    if available == Some(true) || healthy == Some(true) {
        return "healthy".into();
    }
    "unknown".into()
}

#[allow(clippy::too_many_arguments)]
fn build_rent_service_snapshot(
    bindings_json: &str,
    app_runtimes_json: Option<&str>,
    app_providers_json: Option<&str>,
    fallback_app: &str,
    support: &ShareSupport,
    share_parallel_limit: i64,
    share_token_limit: i64,
    share_tokens_used: i64,
) -> Result<RentServiceSnapshot, AppError> {
    let capabilities = enabled_public_app_capabilities(
        bindings_json,
        app_runtimes_json,
        app_providers_json,
        fallback_app,
        support,
    );
    if capabilities.is_empty() {
        return Err(AppError::coded_conflict(
            "share_market_required_app_unavailable",
            "the Share has no enabled App API to include in this seat",
            serde_json::json!({
                "reason": "enabled_apps_missing",
                "supportedApps": [],
            }),
        ));
    }
    let bindings =
        serde_json::from_str::<BTreeMap<String, String>>(bindings_json).map_err(|_| {
            AppError::coded_conflict(
                "share_market_required_app_unavailable",
                "the Share App bindings are invalid",
                serde_json::json!({ "reason": "binding_unresolved" }),
            )
        })?;
    let mut apps = Vec::with_capacity(capabilities.len());
    for capability in capabilities {
        let app = capability.app.clone();
        if let Some(reason) =
            required_app_hard_failure(&app, bindings_json, app_runtimes_json, app_providers_json)
        {
            return Err(AppError::coded_conflict(
                "share_market_required_app_unavailable",
                "an enabled Share App currently has a hard availability failure",
                serde_json::json!({
                    "reason": reason,
                    "app": app,
                }),
            ));
        }
        let binding_id = bindings
            .get(&app)
            .map(|binding| binding.trim())
            .filter(|binding| !binding.is_empty())
            .ok_or_else(|| {
                AppError::coded_conflict(
                    "share_market_required_app_unavailable",
                    "an enabled Share App has no Provider binding",
                    serde_json::json!({ "reason": "binding_missing", "app": app }),
                )
            })?;
        apps.push(RentAppServiceSnapshot {
            app: app.clone(),
            provider_family: capability.provider_family,
            provider_type: capability.provider_type,
            provider_binding_fingerprint: Some(crate::api::sha256_hex(
                format!("share-market-provider-binding-v1\n{app}\n{binding_id}").as_bytes(),
            )),
            model_mode: capability.model_mode,
            upstream_model: capability.upstream_model,
            models: capability.models,
        });
    }
    let supported_apps = apps.iter().map(|app| app.app.clone()).collect::<Vec<_>>();
    let primary = apps
        .first()
        .cloned()
        .ok_or_else(|| AppError::Internal("Share App bundle is unexpectedly empty".into()))?;
    Ok(RentServiceSnapshot {
        schema_version: 2,
        required_app: primary.app,
        supported_apps,
        apps,
        provider_family: primary.provider_family,
        provider_type: primary.provider_type,
        provider_binding_fingerprint: primary.provider_binding_fingerprint,
        model_mode: primary.model_mode,
        upstream_model: primary.upstream_model,
        models: primary.models,
        share_parallel_limit: (share_parallel_limit >= 0)
            .then(|| u32::try_from(share_parallel_limit).ok())
            .flatten(),
        share_token_limit: (share_token_limit >= 0)
            .then(|| u64::try_from(share_token_limit).ok())
            .flatten(),
        share_tokens_used: u64::try_from(share_tokens_used.max(0)).unwrap_or_default(),
    })
}

fn validate_service_snapshot_unchanged(
    quoted: &RentServiceSnapshot,
    current: &RentServiceSnapshot,
) -> Result<(), AppError> {
    let unchanged = if quoted.schema_version >= 2 {
        quoted.supported_apps == current.supported_apps
            && quoted.apps == current.apps
            && quoted.share_parallel_limit == current.share_parallel_limit
            && quoted.share_token_limit == current.share_token_limit
    } else {
        quoted.required_app == current.required_app
            && current.supported_apps.contains(&quoted.required_app)
            && quoted.provider_family == current.provider_family
            && quoted.provider_type == current.provider_type
            && quoted.provider_binding_fingerprint == current.provider_binding_fingerprint
            && quoted.model_mode == current.model_mode
            && quoted.upstream_model == current.upstream_model
            && quoted.models == current.models
            && quoted.share_parallel_limit == current.share_parallel_limit
            && quoted.share_token_limit == current.share_token_limit
    };
    if unchanged {
        return Ok(());
    }
    Err(AppError::coded_conflict(
        "share_market_service_changed",
        "the Share service contract changed; request a new quote",
        serde_json::json!({
            "reason": "service_changed",
            "supportedApps": quoted.supported_apps,
        }),
    ))
}

fn validate_offer_terms_unchanged(
    quoted: &RentQuoteSnapshot,
    current: &RentOfferTerms,
) -> Result<(), AppError> {
    if RentOfferTerms::from_snapshot(quoted) == *current {
        return Ok(());
    }
    Err(AppError::coded_conflict(
        "share_market_offer_changed",
        "the seat terms changed; request a new quote",
        serde_json::json!({
            "reason": "offer_terms_changed",
            "seatId": quoted.seat_id,
            "offerRevision": quoted.offer_revision,
        }),
    ))
}

fn public_app_capabilities(
    bindings_json: &str,
    app_runtimes_json: Option<&str>,
    app_providers_json: Option<&str>,
    fallback_app: &str,
) -> Vec<ShareMarketAppCapability> {
    let apps = supported_share_apps(bindings_json, fallback_app);
    let bindings =
        serde_json::from_str::<BTreeMap<String, String>>(bindings_json).unwrap_or_default();
    let runtimes = app_runtimes_json
        .and_then(|value| serde_json::from_str::<crate::models::ShareAppRuntimes>(value).ok())
        .unwrap_or_default();
    let providers = app_providers_json
        .and_then(|value| serde_json::from_str::<crate::models::ShareAppProviders>(value).ok())
        .unwrap_or_default();
    apps.into_iter()
        .map(|app| {
            let runtime = app_runtime(&runtimes, &app);
            let candidates = app_provider_candidates(&providers, &app);
            let binding_id = bindings
                .get(&app)
                .map(|provider_id| provider_id.trim())
                .filter(|provider_id| !provider_id.is_empty());
            let bound_provider = binding_id.and_then(|provider_id| {
                candidates
                    .iter()
                    .find(|provider| provider.id == provider_id)
            });
            let binding_failed =
                binding_id.is_none() || (!candidates.is_empty() && bound_provider.is_none());
            let runtime = (!binding_failed).then_some(runtime).flatten();
            let bound_provider = (!binding_failed).then_some(bound_provider).flatten();
            let model_policy = runtime
                .and_then(|value| value.model_policy.as_ref())
                .or_else(|| bound_provider.and_then(|provider| provider.model_policy.as_ref()));
            let (model_mode, upstream_model) = match model_policy {
                Some(crate::models::ShareProviderModelPolicy::Passthrough) => {
                    ("passthrough".to_string(), None)
                }
                Some(crate::models::ShareProviderModelPolicy::Single { upstream_model }) => {
                    ("fixed".to_string(), Some(upstream_model.clone()))
                }
                None => ("unknown".to_string(), None),
            };
            let collect_models = |models: &[crate::models::ShareUpstreamModel]| {
                models
                    .iter()
                    .map(|model| model.actual_model.trim().to_string())
                    .filter(|model| !model.is_empty())
                    .collect::<Vec<_>>()
            };
            let mut models = runtime
                .map(|value| collect_models(&value.models))
                .unwrap_or_default();
            if models.is_empty() {
                models = bound_provider
                    .map(|provider| collect_models(&provider.models))
                    .unwrap_or_default();
            }
            models.sort();
            models.dedup();
            let bound_provider_family = || {
                bound_provider.map(|provider| {
                    normalized_provider_family_parts(
                        &app,
                        provider.kind.as_deref().unwrap_or_default(),
                        provider.provider_type.as_deref(),
                        Some(&provider.name),
                    )
                })
            };
            let provider_family = match runtime.map(normalized_provider_family) {
                Some(family) if family != "other" => family,
                _ => bound_provider_family().unwrap_or_else(|| "other".into()),
            };
            let runtime_quota = runtime.and_then(|value| value.quota.as_ref());
            let bound_quota = bound_provider.and_then(|value| value.quota.as_ref());
            let available = if binding_failed {
                Some(false)
            } else {
                runtime
                    .and_then(|value| {
                        value
                            .available
                            .or_else(|| value.health.as_ref().map(|health| health.healthy))
                    })
                    .or_else(|| {
                        bound_provider.and_then(|provider| {
                            provider
                                .available
                                .or_else(|| provider.health.as_ref().map(|health| health.healthy))
                        })
                    })
                    .or_else(|| {
                        bound_provider
                            .filter(|provider| !provider.enabled)
                            .map(|_| false)
                    })
            };
            let explicit_available = runtime
                .and_then(|value| value.available)
                .or_else(|| bound_provider.and_then(|value| value.available));
            let quota_blocked = runtime
                .and_then(|value| value.quota_blocked)
                .or_else(|| bound_provider.and_then(|value| value.quota_blocked));
            let healthy = runtime
                .and_then(|value| value.health.as_ref().map(|health| health.healthy))
                .or_else(|| {
                    bound_provider
                        .and_then(|value| value.health.as_ref().map(|health| health.healthy))
                });
            ShareMarketAppCapability {
                app,
                provider_family,
                provider_name: first_nonempty([
                    runtime.and_then(|value| value.provider_name.as_deref()),
                    bound_provider.map(|provider| provider.name.as_str()),
                ]),
                provider_type: first_nonempty([
                    runtime.and_then(|value| value.provider_type.as_deref()),
                    runtime.map(|value| value.kind.as_str()),
                    bound_provider.and_then(|provider| provider.provider_type.as_deref()),
                    bound_provider.and_then(|provider| provider.kind.as_deref()),
                ]),
                subscription_level: first_nonempty([
                    runtime.and_then(|value| value.subscription_level.as_deref()),
                    runtime.and_then(|value| {
                        value.quota.as_ref().and_then(|quota| quota.plan.as_deref())
                    }),
                    bound_provider.and_then(|provider| provider.subscription_level.as_deref()),
                ]),
                model_mode,
                upstream_model,
                models,
                available,
                health_state: if binding_failed {
                    "unavailable".into()
                } else {
                    public_provider_health_state(
                        explicit_available,
                        quota_blocked,
                        healthy,
                        bound_provider.map(|value| value.enabled),
                        provider_auth_failed(runtime.and_then(|value| value.health.as_ref()))
                            || provider_auth_failed(
                                bound_provider.and_then(|value| value.health.as_ref()),
                            ),
                    )
                },
                account_hint: first_nonempty([
                    runtime.and_then(|value| value.account_label.as_deref()),
                    runtime.and_then(|value| value.account_email.as_deref()),
                    bound_provider.and_then(|value| value.account_label.as_deref()),
                    bound_provider.and_then(|value| value.account_email.as_deref()),
                ]),
                quota: public_provider_quota(
                    runtime_quota.or(bound_quota),
                    runtime
                        .and_then(|value| value.subscription_expires_at.as_deref())
                        .or_else(|| {
                            bound_provider
                                .and_then(|value| value.subscription_expires_at.as_deref())
                        }),
                ),
            }
        })
        .collect()
}

fn enabled_public_app_capabilities(
    bindings_json: &str,
    app_runtimes_json: Option<&str>,
    app_providers_json: Option<&str>,
    fallback_app: &str,
    support: &ShareSupport,
) -> Vec<ShareMarketAppCapability> {
    public_app_capabilities(
        bindings_json,
        app_runtimes_json,
        app_providers_json,
        fallback_app,
    )
    .into_iter()
    .filter(|capability| share_supports_app(support, &capability.app))
    .collect()
}

fn listing_provider_families(capabilities: &[ShareMarketAppCapability]) -> (String, Vec<String>) {
    let mut families = capabilities
        .iter()
        .map(|capability| capability.provider_family.clone())
        .collect::<Vec<_>>();
    families.sort();
    families.dedup();
    let family = match families.as_slice() {
        [] => "other".to_string(),
        [family] => family.clone(),
        _ => "multi".to_string(),
    };
    (family, families)
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
                    daily_rate_minor, currency, service_duration_days, offer_revision, retired_at
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
            service_duration_days,
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
                "serviceDurationDays": service_duration_days,
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
                    currency, service_duration_days, offer_revision, release_reason, created_at,
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
            service_duration_days,
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
                "serviceDurationDays": service_duration_days,
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
    let public_detail = crate::store::client_chat::public_market_event_payload(&detail);
    crate::store::client_chat::enqueue_client_system_event_tx(
        conn,
        &target.installation_id,
        "share_market",
        event_id,
        event_type,
        public_detail,
        &followers,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
fn event_tx(
    tx: &Connection,
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

pub(crate) fn cancel_open_price_changes_tx(
    conn: &Connection,
    subscription_id: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    let proposal_ids = {
        let mut statement = conn
            .prepare(
                "SELECT id FROM share_market_price_changes
                 WHERE subscription_id = ?1 AND status IN ('pending', 'accepted')
                 ORDER BY created_at",
            )
            .map_err(map_db("prepare open Share price changes for cancellation"))?;
        statement
            .query_map(params![subscription_id], |row| row.get::<_, String>(0))
            .map_err(map_db("query open Share price changes for cancellation"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read open Share price changes for cancellation"))?
    };
    for proposal_id in proposal_ids {
        let changed = conn
            .execute(
                "UPDATE share_market_price_changes
                 SET status = 'cancelled', resolution_reason = ?2,
                     responded_at = COALESCE(responded_at, ?3), updated_at = ?3
                 WHERE id = ?1 AND status IN ('pending', 'accepted')",
                params![proposal_id, reason, now],
            )
            .map_err(map_db("cancel open Share price change"))?;
        if changed == 1 {
            event_tx(
                conn,
                None,
                None,
                Some(subscription_id),
                None,
                "price_change_cancelled",
                serde_json::json!({
                    "proposalId": proposal_id,
                    "reason": reason,
                }),
                now,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn apply_accepted_price_changes_tx(
    tx: &Transaction<'_>,
    now: &str,
) -> Result<(), AppError> {
    let proposals = {
        let mut statement = tx
            .prepare(
                "SELECT change.id, change.subscription_id, sub.seat_id, sub.listing_id,
                        sub.status, sub.daily_rate_minor, sub.currency, sub.offer_revision,
                        change.previous_daily_rate_minor,
                        change.proposed_daily_rate_minor, change.currency,
                        change.base_offer_revision, contract.id, contract.account_id,
                        contract.status, contract.daily_rate_minor, contract.offer_revision
                 FROM share_market_price_changes change
                 JOIN share_market_subscriptions sub ON sub.id = change.subscription_id
                 LEFT JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
                 WHERE change.status = 'accepted'
                 ORDER BY change.created_at, change.id",
            )
            .map_err(map_db("prepare accepted Share price changes"))?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                    row.get::<_, Option<i64>>(16)?,
                ))
            })
            .map_err(map_db("query accepted Share price changes"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read accepted Share price changes"))?
    };

    for (
        proposal_id,
        subscription_id,
        seat_id,
        listing_id,
        subscription_status,
        subscription_rate,
        subscription_currency,
        subscription_revision,
        previous_rate,
        proposed_rate,
        proposal_currency,
        base_revision,
        contract_id,
        account_id,
        contract_status,
        contract_rate,
        contract_revision,
    ) in proposals
    {
        if matches!(
            subscription_status.as_str(),
            SUB_RELEASED | SUB_GRANT_FAILED | SUB_REVOKE_PENDING | SUB_REVOKE_FAILED
        ) || contract_status.as_deref() == Some("terminated")
            || contract_id.is_none()
        {
            cancel_open_price_changes_tx(tx, &subscription_id, "subscription_inactive", now)?;
            continue;
        }
        if subscription_rate != Some(previous_rate)
            || subscription_revision != base_revision
            || subscription_currency.as_deref() != Some(proposal_currency.as_str())
            || proposal_currency != crate::market_billing::MARKET_CURRENCY
            || contract_rate != Some(previous_rate)
            || contract_revision != Some(base_revision)
        {
            return Err(AppError::Internal(format!(
                "accepted Share price change {proposal_id} no longer matches its contract"
            )));
        }
        let applied_revision = base_revision.checked_add(1).ok_or_else(|| {
            AppError::Internal("Share price change offer revision overflowed".into())
        })?;
        let subscription_changed = tx
            .execute(
                "UPDATE share_market_subscriptions
                 SET daily_rate_minor = ?2, offer_revision = ?3, updated_at = ?4
                 WHERE id = ?1 AND daily_rate_minor = ?5 AND offer_revision = ?6",
                params![
                    subscription_id,
                    proposed_rate,
                    applied_revision,
                    now,
                    previous_rate,
                    base_revision,
                ],
            )
            .map_err(map_db("apply Share subscription price change"))?;
        let seat_changed = tx
            .execute(
                "UPDATE share_market_seats
                 SET daily_rate_minor = ?2, offer_revision = ?3, updated_at = ?4
                 WHERE id = ?1 AND current_subscription_id = ?5
                   AND daily_rate_minor = ?6 AND offer_revision = ?7",
                params![
                    seat_id,
                    proposed_rate,
                    applied_revision,
                    now,
                    subscription_id,
                    previous_rate,
                    base_revision,
                ],
            )
            .map_err(map_db("apply occupied Share seat price change"))?;
        let contract_id = contract_id.expect("checked active Share price change contract");
        let contract_changed = tx
            .execute(
                "UPDATE market_service_contracts
                 SET daily_rate_minor = ?2, offer_revision = ?3, updated_at = ?4
                 WHERE id = ?1 AND status != 'terminated'
                   AND daily_rate_minor = ?5 AND offer_revision = ?6",
                params![
                    contract_id,
                    proposed_rate,
                    applied_revision,
                    now,
                    previous_rate,
                    base_revision,
                ],
            )
            .map_err(map_db("apply Share billing contract price change"))?;
        if subscription_changed != 1 || seat_changed != 1 || contract_changed != 1 {
            return Err(AppError::Internal(format!(
                "accepted Share price change {proposal_id} lost its atomic update race"
            )));
        }
        tx.execute(
            "UPDATE share_market_price_changes
             SET status = 'applied', applied_offer_revision = ?2,
                 applied_at = ?3, updated_at = ?3
             WHERE id = ?1 AND status = 'accepted'",
            params![proposal_id, applied_revision, now],
        )
        .map_err(map_db("complete Share price change"))?;
        let detail = serde_json::json!({
            "proposalId": proposal_id,
            "previousDailyRateMinor": previous_rate,
            "dailyRateMinor": proposed_rate,
            "currency": proposal_currency,
            "previousOfferRevision": base_revision,
            "offerRevision": applied_revision,
            "effectiveAt": now,
        });
        event_tx(
            tx,
            Some(&listing_id),
            Some(&seat_id),
            Some(&subscription_id),
            None,
            "price_change_applied",
            detail.clone(),
            now,
        )?;
        crate::market_billing::record_event_tx(
            tx,
            account_id.as_deref(),
            Some(&contract_id),
            None,
            None,
            "service_contract_price_changed",
            detail,
            &format!("contract-price-changed:{proposal_id}"),
            now,
        )?;
    }
    Ok(())
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
    integrity_state: String,
    integrity_reason: Option<String>,
    integrity_violated_at: Option<String>,
    app_scope_enforced_at: Option<String>,
    seat_position: i64,
    parallel_limit: Option<i64>,
    token_limit: Option<i64>,
    token_period_json: String,
    daily_rate_minor: Option<i64>,
    currency: Option<String>,
    service_duration_days: Option<u32>,
    offer_revision: i64,
    release_reason: Option<String>,
    failure_code: Option<String>,
    grant_attempts: Option<i64>,
    has_active_control_work: bool,
    has_active_billing_contract: bool,
    activated_at: Option<String>,
    service_started_at: Option<String>,
    termination_adjustment_json: Option<String>,
    expires_at: Option<String>,
    released_at: Option<String>,
    created_at: String,
    updated_at: String,
    contract_apps_json: String,
    service_schema_version: u32,
}

fn subscription_contract_apps(record: &SubscriptionRecord) -> Vec<String> {
    let apps = serde_json::from_str::<Vec<String>>(&record.contract_apps_json)
        .map(canonical_market_apps)
        .unwrap_or_default();
    if apps.is_empty() && MARKET_ALLOWED_APPS.contains(&record.app_type.as_str()) {
        vec![record.app_type.clone()]
    } else {
        apps
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShareMarketCatalogScope {
    Visible,
    Public,
    Owner,
    Renter,
}

fn catalog_visibility_predicate(scope: ShareMarketCatalogScope, share_alias: &str) -> String {
    let public_listing = format!(
        "listing.status = 'active'
         AND lower(COALESCE({share_alias}.owner_email, '')) = lower(listing.owner_email)"
    );
    match scope {
        ShareMarketCatalogScope::Visible => format!(
            "({public_listing})
             OR listing.owner_user_id = ?1
             OR EXISTS (
                 SELECT 1 FROM share_market_subscriptions viewer_sub
                 WHERE viewer_sub.listing_id = listing.id
                   AND viewer_sub.renter_user_id = ?1
             )"
        ),
        ShareMarketCatalogScope::Public => format!("?1 = ?1 AND ({public_listing})"),
        ShareMarketCatalogScope::Owner => "listing.owner_user_id = ?1".into(),
        ShareMarketCatalogScope::Renter => "?1 = ?1 AND 0".into(),
    }
}

fn subscription_record(
    conn: &Connection,
    subscription_id: &str,
) -> Result<Option<SubscriptionRecord>, AppError> {
    conn.query_row(
        "SELECT sub.id, sub.seat_id, sub.listing_id, sub.share_id, sub.installation_id,
                COALESCE(s.share_name, sub.share_id),
                COALESCE(sub.required_app, s.app_type, ''),
                COALESCE(s.subdomain, ''),
                sub.entitlement_id, sub.owner_user_id, sub.owner_email,
                sub.renter_user_id, sub.renter_email, sub.status,
                sub.integrity_state, sub.integrity_reason, sub.integrity_violated_at,
                sub.daily_rate_minor, sub.currency, sub.service_duration_days,
                sub.offer_revision, sub.release_reason, sub.activated_at, sub.expires_at,
                sub.released_at,
                sub.created_at, sub.updated_at,
                (SELECT edit.error_code
                 FROM share_control_operations operation
                 LEFT JOIN share_edit_requests edit ON edit.id = operation.edit_id
                 WHERE operation.subscription_id = sub.id AND operation.action = 'upsert'
                 ORDER BY operation.share_sequence DESC LIMIT 1),
                (SELECT operation.attempts
                 FROM share_control_operations operation
                 WHERE operation.subscription_id = sub.id AND operation.action = 'upsert'
                 ORDER BY operation.share_sequence DESC LIMIT 1),
                EXISTS (
                    SELECT 1 FROM share_control_operations operation
                    WHERE operation.subscription_id = sub.id
                      AND operation.status IN ('pending', 'dispatched')
                ),
                EXISTS (
                    SELECT 1 FROM market_service_contracts contract
                    WHERE contract.product_kind = 'share' AND contract.product_ref = sub.id
                      AND contract.status != 'terminated'
                ),
                COALESCE((SELECT position FROM share_market_seats WHERE id = sub.seat_id), 0),
                sub.parallel_limit, sub.token_limit, sub.token_period_json,
                sub.service_started_at,
                (SELECT json_object(
                    'id', adjustment.id,
                    'status', adjustment.status,
                    'currency', adjustment.currency,
                    'elapsedBps', adjustment.elapsed_bps,
                    'refundBps', adjustment.refund_bps,
                    'amountMinor', adjustment.amount_minor,
                    'unbilledCreditMinor', COALESCE((
                        SELECT SUM(amount_minor) FROM market_adjustment_allocations allocation
                        WHERE allocation.adjustment_id = adjustment.id
                          AND allocation.target_kind = 'unbilled_accrual'
                    ), 0),
                    'invoiceCreditMinor', COALESCE((
                        SELECT SUM(amount_minor) FROM market_adjustment_allocations allocation
                        WHERE allocation.adjustment_id = adjustment.id
                          AND allocation.target_kind = 'invoice_credit'
                    ), 0),
                    'externalRefundMinor', COALESCE((
                        SELECT SUM(amount_minor) FROM market_adjustment_allocations allocation
                        WHERE allocation.adjustment_id = adjustment.id
                          AND allocation.target_kind = 'external_refund'
                    ), 0),
                    'refundObligationStatus', CASE
                        WHEN EXISTS(SELECT 1 FROM market_refund_obligations obligation
                            WHERE obligation.adjustment_id = adjustment.id
                              AND obligation.status != 'recorded') THEN 'pending'
                        WHEN EXISTS(SELECT 1 FROM market_refund_obligations obligation
                            WHERE obligation.adjustment_id = adjustment.id) THEN 'recorded'
                        ELSE NULL END
                 ) FROM market_contract_adjustments adjustment
                 WHERE adjustment.product_kind = 'share' AND adjustment.product_ref = sub.id
                 ORDER BY adjustment.created_at DESC LIMIT 1),
                sub.contract_apps_json, sub.app_scope_enforced_at,
                COALESCE(CASE WHEN json_valid(sub.service_snapshot_json)
                    THEN CAST(json_extract(sub.service_snapshot_json, '$.schemaVersion') AS INTEGER)
                    END, 0)
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
                integrity_state: row.get(14)?,
                integrity_reason: row.get(15)?,
                integrity_violated_at: row.get(16)?,
                daily_rate_minor: row.get(17)?,
                currency: row.get(18)?,
                service_duration_days: row
                    .get::<_, Option<i64>>(19)?
                    .and_then(|value| u32::try_from(value).ok()),
                offer_revision: row.get(20)?,
                release_reason: row.get(21)?,
                activated_at: row.get(22)?,
                expires_at: row.get(23)?,
                released_at: row.get(24)?,
                created_at: row.get(25)?,
                updated_at: row.get(26)?,
                failure_code: row.get(27)?,
                grant_attempts: row.get(28)?,
                has_active_control_work: row.get::<_, i64>(29)? != 0,
                has_active_billing_contract: row.get::<_, i64>(30)? != 0,
                seat_position: row.get(31)?,
                parallel_limit: row.get(32)?,
                token_limit: row.get(33)?,
                token_period_json: row.get(34)?,
                service_started_at: row.get(35)?,
                termination_adjustment_json: row.get(36)?,
                contract_apps_json: row.get(37)?,
                app_scope_enforced_at: row.get(38)?,
                service_schema_version: row.get::<_, i64>(39)?.try_into().unwrap_or_default(),
            })
        },
    )
    .optional()
    .map_err(map_db("read Share Market subscription"))
}

fn catalog_subscription_records(
    conn: &Connection,
    viewer_user_id: &str,
    scope: ShareMarketCatalogScope,
) -> Result<HashMap<String, SubscriptionRecord>, AppError> {
    let visibility = catalog_visibility_predicate(scope, "share");
    let (cte, filter) = match scope {
        ShareMarketCatalogScope::Visible => (
            format!(
                "WITH visible_listings AS (
            SELECT listing.id
            FROM share_market_listings listing
            LEFT JOIN shares share ON share.share_id = listing.share_id
            WHERE listing.deleted_at IS NULL
              AND ({visibility})
         ), referenced_subscriptions AS (
            SELECT seat.current_subscription_id AS id
            FROM share_market_seats seat
            WHERE seat.listing_id IN (SELECT id FROM visible_listings)
              AND seat.current_subscription_id IS NOT NULL
            UNION
            SELECT seat.retired_subscription_id AS id
            FROM share_market_seats seat
            WHERE seat.listing_id IN (SELECT id FROM visible_listings)
              AND seat.retired_subscription_id IS NOT NULL
         )"
            ),
            "sub.id IN (SELECT id FROM referenced_subscriptions)
             OR (?1 != '' AND sub.renter_user_id = ?1)",
        ),
        ShareMarketCatalogScope::Owner => (
            format!(
                "WITH visible_listings AS (
            SELECT listing.id
            FROM share_market_listings listing
            LEFT JOIN shares share ON share.share_id = listing.share_id
            WHERE listing.deleted_at IS NULL
              AND ({visibility})
         ), referenced_subscriptions AS (
            SELECT seat.current_subscription_id AS id
            FROM share_market_seats seat
            WHERE seat.listing_id IN (SELECT id FROM visible_listings)
              AND seat.current_subscription_id IS NOT NULL
            UNION
            SELECT seat.retired_subscription_id AS id
            FROM share_market_seats seat
            WHERE seat.listing_id IN (SELECT id FROM visible_listings)
              AND seat.retired_subscription_id IS NOT NULL
         )"
            ),
            "sub.id IN (SELECT id FROM referenced_subscriptions)",
        ),
        ShareMarketCatalogScope::Public => (
            format!(
                "WITH visible_listings AS (
            SELECT listing.id
            FROM share_market_listings listing
            LEFT JOIN shares share ON share.share_id = listing.share_id
            WHERE listing.deleted_at IS NULL
              AND ({visibility})
         ), referenced_subscriptions AS (
            SELECT seat.current_subscription_id AS id
            FROM share_market_seats seat
            WHERE seat.listing_id IN (SELECT id FROM visible_listings)
              AND seat.current_subscription_id IS NOT NULL
         )"
            ),
            "sub.id IN (SELECT id FROM referenced_subscriptions)",
        ),
        ShareMarketCatalogScope::Renter => (String::new(), "?1 != '' AND sub.renter_user_id = ?1"),
    };
    let sql = format!(
        "{cte}
         SELECT sub.id, sub.seat_id, sub.listing_id, sub.share_id, sub.installation_id,
                COALESCE(share.share_name, sub.share_id),
                COALESCE(sub.required_app, share.app_type, ''),
                COALESCE(share.subdomain, ''), sub.entitlement_id,
                sub.owner_user_id, sub.owner_email, sub.renter_user_id,
                sub.renter_email, sub.status,
                sub.integrity_state, sub.integrity_reason, sub.integrity_violated_at,
                sub.daily_rate_minor, sub.currency,
                sub.service_duration_days, sub.offer_revision,
                sub.release_reason, sub.activated_at, sub.expires_at, sub.released_at,
                sub.created_at, sub.updated_at,
                (SELECT edit.error_code
                 FROM share_control_operations operation
                 LEFT JOIN share_edit_requests edit ON edit.id = operation.edit_id
                 WHERE operation.subscription_id = sub.id AND operation.action = 'upsert'
                 ORDER BY operation.share_sequence DESC LIMIT 1),
                (SELECT operation.attempts
                 FROM share_control_operations operation
                 WHERE operation.subscription_id = sub.id AND operation.action = 'upsert'
                 ORDER BY operation.share_sequence DESC LIMIT 1),
                EXISTS (
                    SELECT 1 FROM share_control_operations operation
                    WHERE operation.subscription_id = sub.id
                      AND operation.status IN ('pending', 'dispatched')
                ),
                EXISTS (
                    SELECT 1 FROM market_service_contracts contract
                    WHERE contract.product_kind = 'share' AND contract.product_ref = sub.id
                      AND contract.status != 'terminated'
                ),
                COALESCE((SELECT position FROM share_market_seats WHERE id = sub.seat_id), 0),
                sub.parallel_limit, sub.token_limit, sub.token_period_json,
                sub.service_started_at,
                (SELECT json_object(
                    'id', adjustment.id,
                    'status', adjustment.status,
                    'currency', adjustment.currency,
                    'elapsedBps', adjustment.elapsed_bps,
                    'refundBps', adjustment.refund_bps,
                    'amountMinor', adjustment.amount_minor,
                    'unbilledCreditMinor', COALESCE((
                        SELECT SUM(amount_minor) FROM market_adjustment_allocations allocation
                        WHERE allocation.adjustment_id = adjustment.id
                          AND allocation.target_kind = 'unbilled_accrual'
                    ), 0),
                    'invoiceCreditMinor', COALESCE((
                        SELECT SUM(amount_minor) FROM market_adjustment_allocations allocation
                        WHERE allocation.adjustment_id = adjustment.id
                          AND allocation.target_kind = 'invoice_credit'
                    ), 0),
                    'externalRefundMinor', COALESCE((
                        SELECT SUM(amount_minor) FROM market_adjustment_allocations allocation
                        WHERE allocation.adjustment_id = adjustment.id
                          AND allocation.target_kind = 'external_refund'
                    ), 0),
                    'refundObligationStatus', CASE
                        WHEN EXISTS(SELECT 1 FROM market_refund_obligations obligation
                            WHERE obligation.adjustment_id = adjustment.id
                              AND obligation.status != 'recorded') THEN 'pending'
                        WHEN EXISTS(SELECT 1 FROM market_refund_obligations obligation
                            WHERE obligation.adjustment_id = adjustment.id) THEN 'recorded'
                        ELSE NULL END
                 ) FROM market_contract_adjustments adjustment
                 WHERE adjustment.product_kind = 'share' AND adjustment.product_ref = sub.id
                 ORDER BY adjustment.created_at DESC LIMIT 1),
                sub.contract_apps_json, sub.app_scope_enforced_at,
                COALESCE(CASE WHEN json_valid(sub.service_snapshot_json)
                    THEN CAST(json_extract(sub.service_snapshot_json, '$.schemaVersion') AS INTEGER)
                    END, 0)
         FROM share_market_subscriptions sub
         LEFT JOIN shares share ON share.share_id = sub.share_id
         WHERE {filter}"
    );
    conn.prepare(&sql)
        .and_then(|mut statement| {
            statement
                .query_map(params![viewer_user_id], |row| {
                    let record = SubscriptionRecord {
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
                        integrity_state: row.get(14)?,
                        integrity_reason: row.get(15)?,
                        integrity_violated_at: row.get(16)?,
                        daily_rate_minor: row.get(17)?,
                        currency: row.get(18)?,
                        service_duration_days: row
                            .get::<_, Option<i64>>(19)?
                            .and_then(|value| u32::try_from(value).ok()),
                        offer_revision: row.get(20)?,
                        release_reason: row.get(21)?,
                        activated_at: row.get(22)?,
                        expires_at: row.get(23)?,
                        released_at: row.get(24)?,
                        created_at: row.get(25)?,
                        updated_at: row.get(26)?,
                        failure_code: row.get(27)?,
                        grant_attempts: row.get(28)?,
                        has_active_control_work: row.get::<_, i64>(29)? != 0,
                        has_active_billing_contract: row.get::<_, i64>(30)? != 0,
                        seat_position: row.get(31)?,
                        parallel_limit: row.get(32)?,
                        token_limit: row.get(33)?,
                        token_period_json: row.get(34)?,
                        service_started_at: row.get(35)?,
                        termination_adjustment_json: row.get(36)?,
                        contract_apps_json: row.get(37)?,
                        app_scope_enforced_at: row.get(38)?,
                        service_schema_version: row
                            .get::<_, i64>(39)?
                            .try_into()
                            .unwrap_or_default(),
                    };
                    Ok((record.id.clone(), record))
                })?
                .collect::<Result<HashMap<_, _>, _>>()
        })
        .map_err(map_db("read Share Market catalog subscriptions"))
}

#[derive(Debug)]
struct CatalogSeatRecord {
    id: String,
    position: i64,
    status: String,
    parallel_limit: Option<i64>,
    token_limit: Option<i64>,
    token_period_json: String,
    daily_rate_minor: Option<i64>,
    currency: Option<String>,
    service_duration_days: Option<i64>,
    offer_revision: i64,
    current_subscription_id: Option<String>,
    retired_subscription_id: Option<String>,
    retired_at: Option<String>,
    subscription_count: i64,
}

#[derive(Debug, Clone, Copy)]
struct DeleteCapability {
    can_delete: bool,
    blocked_reason: Option<&'static str>,
}

impl DeleteCapability {
    const fn allowed() -> Self {
        Self {
            can_delete: true,
            blocked_reason: None,
        }
    }

    const fn blocked(reason: &'static str) -> Self {
        Self {
            can_delete: false,
            blocked_reason: Some(reason),
        }
    }
}

#[derive(Debug, Clone)]
struct ReopenCapability {
    can_reopen: bool,
    blocked_reason: Option<&'static str>,
    reopenable_seat_count: u32,
}

fn listing_reopen_capability_tx(
    conn: &Connection,
    listing_id: &str,
    listing_status: &str,
    is_owner: bool,
) -> Result<ReopenCapability, AppError> {
    let state = conn
        .query_row(
            "SELECT listing.share_id, listing.owner_email, share.owner_email,
                    share.share_status, COALESCE(share.free_access, 0),
                    COALESCE(share.share_access_policy_version, 0),
                    (SELECT COUNT(*) FROM share_market_seats seat
                     WHERE seat.listing_id = listing.id AND seat.status = 'disabled'
                       AND seat.retired_at IS NULL
                       AND seat.current_subscription_id IS NULL),
                    (SELECT COUNT(*) FROM share_market_seats seat
                     WHERE seat.listing_id = listing.id AND seat.retired_at IS NULL
                       AND seat.status IN ('available', 'reserved', 'occupied', 'revoking')),
                    (SELECT id FROM share_market_listings active
                     WHERE active.share_id = listing.share_id AND active.id != listing.id
                       AND active.status = 'active' AND active.deleted_at IS NULL
                     ORDER BY active.updated_at DESC LIMIT 1)
             FROM share_market_listings listing
             JOIN shares share ON share.share_id = listing.share_id
             WHERE listing.id = ?1 AND listing.deleted_at IS NULL",
            params![listing_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)? != 0,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read Share listing reopen capability"))?;
    let reopenable_seat_count = state
        .as_ref()
        .and_then(|state| u32::try_from(state.6.max(0)).ok())
        .unwrap_or_default();
    let blocked_reason = if !is_owner {
        Some("owner_only")
    } else if listing_status != "closed" {
        Some("listing_not_closed")
    } else {
        match state.as_ref() {
            None => Some("share_missing"),
            Some(state) if !state.1.eq_ignore_ascii_case(&state.2) => Some("owner_changed"),
            Some(state) if state.3 != "active" => Some("share_inactive"),
            Some(state) if state.4 => Some("public_access_enabled"),
            Some(state) if pending_edit_enables_free_access_tx(conn, &state.0)? => {
                Some("pending_share_edit")
            }
            Some(state) if state.5 < i64::from(SHARE_CONTRACT_VERSION) => {
                Some("client_upgrade_required")
            }
            Some(state) if state.8.is_some() => Some("another_listing_active"),
            Some(state) if state.7 >= MAX_SEATS_PER_LISTING as i64 => Some("seat_limit"),
            Some(_) => None,
        }
    };
    Ok(ReopenCapability {
        can_reopen: blocked_reason.is_none(),
        blocked_reason,
        reopenable_seat_count,
    })
}

fn seat_delete_capability(
    is_owner: bool,
    seat_status: &str,
    retired_at: Option<&str>,
    subscription_count: i64,
    subscription: Option<&SubscriptionRecord>,
) -> DeleteCapability {
    if !is_owner {
        return DeleteCapability::blocked("owner_only");
    }
    if subscription_count == 0 && retired_at.is_none() {
        return if matches!(seat_status, SEAT_AVAILABLE | SEAT_DISABLED) {
            DeleteCapability::allowed()
        } else {
            DeleteCapability::blocked("seat_not_reclaimable")
        };
    }
    let Some(subscription) = subscription else {
        return DeleteCapability::blocked("rental_history");
    };
    if subscription.status != SUB_GRANT_FAILED {
        return DeleteCapability::blocked("rental_history");
    }
    if subscription.has_active_control_work {
        return DeleteCapability::blocked("control_pending");
    }
    if subscription.has_active_billing_contract {
        return DeleteCapability::blocked("billing_active");
    }
    DeleteCapability::allowed()
}

fn listing_delete_capability_tx(
    conn: &Connection,
    listing_id: &str,
    listing_status: &str,
    is_owner: bool,
) -> Result<DeleteCapability, AppError> {
    if !is_owner {
        return Ok(DeleteCapability::blocked("owner_only"));
    }
    if listing_status != "closed" {
        return Ok(DeleteCapability::blocked("listing_must_be_closed"));
    }
    let (has_nonterminal_subscription, has_active_control_work, has_active_billing_contract) = conn
        .query_row(
            "SELECT
                EXISTS (
                    SELECT 1 FROM share_market_subscriptions subscription
                    WHERE subscription.listing_id = ?1
                      AND subscription.status NOT IN ('released', 'grant_failed')
                ),
                EXISTS (
                    SELECT 1
                    FROM share_control_operations operation
                    JOIN share_market_subscriptions subscription
                      ON subscription.id = operation.subscription_id
                    WHERE subscription.listing_id = ?1
                      AND operation.status IN ('pending', 'dispatched')
                ),
                EXISTS (
                    SELECT 1
                    FROM market_service_contracts contract
                    JOIN share_market_subscriptions subscription
                      ON subscription.id = contract.product_ref
                    WHERE subscription.listing_id = ?1
                      AND contract.product_kind = 'share'
                      AND contract.status != 'terminated'
                )",
            params![listing_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? != 0,
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, i64>(2)? != 0,
                ))
            },
        )
        .map_err(map_db("read Share listing delete capability"))?;
    if has_nonterminal_subscription {
        return Ok(DeleteCapability::blocked("active_rentals"));
    }
    if has_active_control_work {
        return Ok(DeleteCapability::blocked("control_pending"));
    }
    if has_active_billing_contract {
        return Ok(DeleteCapability::blocked("billing_active"));
    }
    Ok(DeleteCapability::allowed())
}

fn catalog_seats(
    conn: &Connection,
    viewer_user_id: &str,
    scope: ShareMarketCatalogScope,
) -> Result<HashMap<String, Vec<CatalogSeatRecord>>, AppError> {
    let visibility = catalog_visibility_predicate(scope, "share");
    let public_seat_filter = if scope == ShareMarketCatalogScope::Public {
        "AND seat.retired_at IS NULL AND seat.status IN ('available', 'reserved', 'occupied', 'revoking')"
    } else {
        ""
    };
    let sql = format!(
        "SELECT seat.listing_id, seat.id, seat.position, seat.status,
                seat.parallel_limit, seat.token_limit, seat.token_period_json,
                seat.daily_rate_minor, seat.currency, seat.service_duration_days,
                seat.offer_revision, seat.current_subscription_id,
                seat.retired_subscription_id, seat.retired_at,
                (SELECT COUNT(*) FROM share_market_subscriptions subscription
                 WHERE subscription.seat_id = seat.id)
         FROM share_market_seats seat
         JOIN share_market_listings listing ON listing.id = seat.listing_id
         LEFT JOIN shares share ON share.share_id = listing.share_id
         WHERE seat.status != 'deleted' AND listing.deleted_at IS NULL
           AND ({visibility})
           {public_seat_filter}
         ORDER BY seat.listing_id,
                  CASE WHEN seat.retired_at IS NULL THEN 0 ELSE 1 END,
                  seat.position"
    );
    let rows = conn
        .prepare(&sql)
        .and_then(|mut statement| {
            statement
                .query_map(params![viewer_user_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        CatalogSeatRecord {
                            id: row.get(1)?,
                            position: row.get(2)?,
                            status: row.get(3)?,
                            parallel_limit: row.get(4)?,
                            token_limit: row.get(5)?,
                            token_period_json: row.get(6)?,
                            daily_rate_minor: row.get(7)?,
                            currency: row.get(8)?,
                            service_duration_days: row.get(9)?,
                            offer_revision: row.get(10)?,
                            current_subscription_id: row.get(11)?,
                            retired_subscription_id: row.get(12)?,
                            retired_at: row.get(13)?,
                            subscription_count: row.get(14)?,
                        },
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read Share Market catalog seats"))?;
    let mut by_listing = HashMap::<String, Vec<CatalogSeatRecord>>::new();
    for (listing_id, seat) in rows {
        by_listing.entry(listing_id).or_default().push(seat);
    }
    Ok(by_listing)
}

fn active_rented_share_ids(
    conn: &Connection,
    viewer_user_id: &str,
) -> Result<HashSet<String>, AppError> {
    if viewer_user_id.is_empty() {
        return Ok(HashSet::new());
    }
    conn.prepare(
        "SELECT DISTINCT share_id
         FROM share_market_subscriptions
         WHERE renter_user_id = ?1
           AND status NOT IN ('released', 'grant_failed')",
    )
    .and_then(|mut statement| {
        statement
            .query_map(params![viewer_user_id], |row| row.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()
    })
    .map_err(map_db("read active Share Market rentals"))
}

#[derive(Debug, Default)]
struct PerformanceAccumulator {
    recent_request_count: u32,
    ttft_samples_ms: Vec<f64>,
    tps_samples: Vec<f64>,
    latest_sample_at: Option<i64>,
}

fn median(samples: &mut [f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(f64::total_cmp);
    let middle = samples.len() / 2;
    if samples.len() % 2 == 0 {
        Some((samples[middle - 1] + samples[middle]) / 2.0)
    } else {
        Some(samples[middle])
    }
}

fn reliable_generation_window_ms(latency_ms: i64, first_token_ms: i64) -> Option<i64> {
    let generation_ms = latency_ms.checked_sub(first_token_ms)?;
    let one_percent_ms = latency_ms.saturating_add(99) / 100;
    let minimum_ms = MARKET_PERFORMANCE_MIN_GENERATION_MS.max(one_percent_ms);
    (generation_ms >= minimum_ms).then_some(generation_ms)
}

fn share_market_performance(
    conn: &Connection,
    share_ids: &[String],
) -> Result<HashMap<String, ShareMarketPerformance>, AppError> {
    let mut performance = HashMap::new();
    for share_ids in share_ids.chunks(MARKET_AGGREGATE_BATCH_SIZE) {
        performance.extend(share_market_performance_batch(conn, share_ids)?);
    }
    Ok(performance)
}

fn share_market_performance_batch(
    conn: &Connection,
    share_ids: &[String],
) -> Result<HashMap<String, ShareMarketPerformance>, AppError> {
    let placeholders = std::iter::repeat_n("?", share_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT share_id, status_code, latency_ms, first_token_ms, output_tokens,
                usage_state, is_streaming, COALESCE(stream_status, ''), created_at
         FROM (
             SELECT share_id, status_code, latency_ms, first_token_ms, output_tokens,
                    usage_state, is_streaming, stream_status, created_at,
                    ROW_NUMBER() OVER (
                        PARTITION BY share_id ORDER BY created_at DESC, request_id DESC
                    ) AS row_num
             FROM share_request_logs
             WHERE COALESCE(is_health_check, 0) = 0
               AND share_id IN ({placeholders})
               AND created_at >= ?
         )
         WHERE row_num <= ?"
    );
    let mut values = share_ids
        .iter()
        .cloned()
        .map(crate::db::types::Value::Text)
        .collect::<Vec<_>>();
    values.push(crate::db::types::Value::Integer(
        Utc::now().timestamp() - MARKET_PERFORMANCE_WINDOW_HOURS * 3_600,
    ));
    values.push(crate::db::types::Value::Integer(
        MARKET_PERFORMANCE_MAX_SAMPLES,
    ));
    let rows = conn
        .prepare(&sql)
        .and_then(|mut statement| {
            statement
                .query_map(values, |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)? != 0,
                        row.get::<_, String>(7)?,
                        row.get::<_, i64>(8)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read Share Market performance samples"))?;
    let mut accumulators = HashMap::<String, PerformanceAccumulator>::new();
    for (
        share_id,
        status_code,
        latency_ms,
        first_token_ms,
        output_tokens,
        usage_state,
        is_streaming,
        stream_status,
        created_at,
    ) in rows
    {
        let entry = accumulators.entry(share_id).or_default();
        entry.recent_request_count += 1;
        entry.latest_sample_at = Some(
            entry
                .latest_sample_at
                .map_or(created_at, |latest| latest.max(created_at)),
        );
        let Some(first_token_ms) = first_token_ms else {
            continue;
        };
        if !is_streaming
            || !(200..300).contains(&status_code)
            || !stream_status.eq_ignore_ascii_case("completed")
            || first_token_ms <= 0
            || latency_ms <= first_token_ms
        {
            continue;
        }
        entry.ttft_samples_ms.push(first_token_ms as f64);
        if usage_state.eq_ignore_ascii_case("observed") && output_tokens > 0 {
            let Some(generation_ms) = reliable_generation_window_ms(latency_ms, first_token_ms)
            else {
                continue;
            };
            let generation_seconds = generation_ms as f64 / 1_000.0;
            let tps = output_tokens as f64 / generation_seconds;
            if tps.is_finite() && tps > 0.0 {
                entry.tps_samples.push(tps);
            }
        }
    }
    Ok(accumulators
        .into_iter()
        .map(|(share_id, mut entry)| {
            let ttft_sample_count = u32::try_from(entry.ttft_samples_ms.len()).unwrap_or(u32::MAX);
            let tps_sample_count = u32::try_from(entry.tps_samples.len()).unwrap_or(u32::MAX);
            (
                share_id,
                ShareMarketPerformance {
                    average_ttft_ms: median(&mut entry.ttft_samples_ms),
                    average_tps: median(&mut entry.tps_samples),
                    recent_request_count: entry.recent_request_count,
                    ttft_sample_count,
                    tps_sample_count,
                    latest_sample_at: entry.latest_sample_at.and_then(|timestamp| {
                        DateTime::<Utc>::from_timestamp(timestamp, 0)
                            .map(|value| value.to_rfc3339())
                    }),
                    window_hours: u32::try_from(MARKET_PERFORMANCE_WINDOW_HOURS)
                        .unwrap_or_default(),
                },
            )
        })
        .collect())
}

fn share_market_reliability(
    conn: &Connection,
    share_ids: &[String],
) -> Result<HashMap<String, (u32, u32, i64)>, AppError> {
    let mut reliability = HashMap::new();
    for share_ids in share_ids.chunks(MARKET_AGGREGATE_BATCH_SIZE) {
        reliability.extend(share_market_reliability_batch(conn, share_ids)?);
    }
    Ok(reliability)
}

fn share_market_reliability_batch(
    conn: &Connection,
    share_ids: &[String],
) -> Result<HashMap<String, (u32, u32, i64)>, AppError> {
    let cutoff = Utc::now().timestamp() - i64::from(HEALTH_WINDOW_MINUTES) * 60;
    let placeholders = std::iter::repeat_n("?", share_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "WITH ranked AS (
             SELECT share_id, status, checked_at,
                    ROW_NUMBER() OVER (
                        PARTITION BY share_id, checked_at / 60
                        ORDER BY checked_at DESC, id DESC
                    ) AS row_num
             FROM share_health_checks
             WHERE share_id IN ({placeholders}) AND checked_at >= ?
               AND status IN ('healthy', 'unhealthy')
         )
         SELECT share_id,
                SUM(CASE WHEN status = 'healthy' THEN 1 ELSE 0 END),
                COUNT(*),
                MAX(checked_at)
         FROM ranked
         WHERE row_num = 1
         GROUP BY share_id"
    );
    let mut values = share_ids
        .iter()
        .cloned()
        .map(crate::db::types::Value::Text)
        .collect::<Vec<_>>();
    values.push(crate::db::types::Value::Integer(cutoff));
    conn.prepare(&sql)
        .and_then(|mut statement| {
            statement
                .query_map(values, |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        (
                            row.get::<_, i64>(1)?
                                .clamp(0, i64::from(HEALTH_WINDOW_MINUTES))
                                as u32,
                            row.get::<_, i64>(2)?
                                .clamp(0, i64::from(HEALTH_WINDOW_MINUTES))
                                as u32,
                            row.get::<_, i64>(3)?,
                        ),
                    ))
                })?
                .collect::<Result<HashMap<_, _>, _>>()
        })
        .map_err(map_db("read Share Market reliability"))
}

fn public_catalog_seat_visible(seat: &SeatView) -> bool {
    !seat.read_only
        && matches!(
            seat.status.as_str(),
            SEAT_AVAILABLE | SEAT_RESERVED | SEAT_OCCUPIED | SEAT_REVOKING
        )
}

fn redact_public_subscription(subscription: &mut SubscriptionView) {
    subscription.renter_email.clear();
    subscription.contacts.clear();
    subscription.payment_method_kinds.clear();
    subscription.can_release = false;
    subscription.can_force_revoke = false;
    subscription.can_retry_grant = false;
    subscription.can_propose_price_change = false;
    subscription.price_change = None;
    subscription.failure_code = None;
    subscription.grant_attempts = None;
    subscription.release_reason = None;
    subscription.integrity_reason = None;
    subscription.integrity_violated_at = None;
    subscription.termination_adjustment = None;
}

fn retain_public_catalog(catalog: &mut ShareMarketCatalog) {
    catalog.listings.retain_mut(|listing| {
        if !listing.publicly_listed {
            return false;
        }
        listing.seats.retain_mut(|seat| {
            if !public_catalog_seat_visible(seat) {
                return false;
            }
            if let Some(subscription) = seat.subscription.as_mut() {
                redact_public_subscription(subscription);
            }
            true
        });
        !listing.seats.is_empty()
    });
}

fn reliability_view(sample: Option<(u32, u32, i64)>) -> ShareMarketReliability {
    let (healthy, observed, latest_observed_at) = sample.unwrap_or_default();
    ShareMarketReliability {
        online_rate_24h: if observed == 0 {
            None
        } else {
            Some(f64::from(healthy) / f64::from(observed) * 100.0)
        },
        observed_minutes_24h: observed,
        observation_coverage_24h: f64::from(observed) / f64::from(HEALTH_WINDOW_MINUTES) * 100.0,
        sufficient_coverage: observed >= MARKET_RELIABILITY_MIN_OBSERVED_MINUTES,
        latest_observed_at: (latest_observed_at > 0)
            .then(|| {
                DateTime::<Utc>::from_timestamp(latest_observed_at, 0)
                    .map(|value| value.to_rfc3339())
            })
            .flatten(),
    }
}

type PaymentProfileSnapshot = (Vec<PaymentMethod>, Vec<PaymentContact>, String);

fn payment_profiles(
    conn: &Connection,
    user_ids: &[String],
) -> Result<HashMap<String, PaymentProfileSnapshot>, AppError> {
    let mut profiles = HashMap::new();
    for user_ids in user_ids.chunks(MARKET_AGGREGATE_BATCH_SIZE) {
        let placeholders = std::iter::repeat_n("?", user_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT user_id, methods_json, COALESCE(contacts_json, '[]'), updated_at
             FROM account_payment_profiles
             WHERE user_id IN ({placeholders})"
        );
        let values = user_ids
            .iter()
            .cloned()
            .map(crate::db::types::Value::Text)
            .collect::<Vec<_>>();
        profiles.extend(
            conn.prepare(&sql)
                .and_then(|mut statement| {
                    statement
                        .query_map(values, |row| {
                            let methods = row.get::<_, String>(1)?;
                            let contacts = row.get::<_, String>(2)?;
                            Ok((
                                row.get::<_, String>(0)?,
                                (
                                    serde_json::from_str::<Vec<PaymentMethod>>(&methods)
                                        .unwrap_or_default(),
                                    serde_json::from_str::<Vec<PaymentContact>>(&contacts)
                                        .unwrap_or_default(),
                                    row.get::<_, String>(3)?,
                                ),
                            ))
                        })?
                        .collect::<Result<HashMap<_, _>, _>>()
                })
                .map_err(map_db("read Share Market payment profiles"))?,
        );
    }
    Ok(profiles)
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

#[derive(Debug, Clone)]
struct ActivePriceChangeRecord {
    id: String,
    previous_daily_rate_minor: i64,
    proposed_daily_rate_minor: i64,
    currency: String,
    base_offer_revision: i64,
    status: String,
    created_at: String,
    updated_at: String,
    responded_at: Option<String>,
}

fn active_price_changes(
    conn: &Connection,
    subscription_ids: &[String],
) -> Result<HashMap<String, ActivePriceChangeRecord>, AppError> {
    let mut changes = HashMap::new();
    for subscription_ids in subscription_ids.chunks(MARKET_AGGREGATE_BATCH_SIZE) {
        let placeholders = std::iter::repeat_n("?", subscription_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT subscription_id, id, previous_daily_rate_minor,
                    proposed_daily_rate_minor, currency, base_offer_revision,
                    status, created_at, updated_at, responded_at
             FROM share_market_price_changes
             WHERE status IN ('pending', 'accepted')
               AND subscription_id IN ({placeholders})"
        );
        let values = subscription_ids
            .iter()
            .cloned()
            .map(crate::db::types::Value::Text)
            .collect::<Vec<_>>();
        changes.extend(
            conn.prepare(&sql)
                .and_then(|mut statement| {
                    statement
                        .query_map(values, |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                ActivePriceChangeRecord {
                                    id: row.get(1)?,
                                    previous_daily_rate_minor: row.get(2)?,
                                    proposed_daily_rate_minor: row.get(3)?,
                                    currency: row.get(4)?,
                                    base_offer_revision: row.get(5)?,
                                    status: row.get(6)?,
                                    created_at: row.get(7)?,
                                    updated_at: row.get(8)?,
                                    responded_at: row.get(9)?,
                                },
                            ))
                        })?
                        .collect::<Result<HashMap<_, _>, _>>()
                })
                .map_err(map_db("read active Share price changes"))?,
        );
    }
    Ok(changes)
}

fn active_price_change_view(
    record: Option<&ActivePriceChangeRecord>,
    is_owner: bool,
    is_renter: bool,
) -> Option<PriceChangeView> {
    if !is_owner && !is_renter {
        return None;
    }
    record.map(|record| PriceChangeView {
        id: record.id.clone(),
        previous_daily_rate_minor: record.previous_daily_rate_minor,
        proposed_daily_rate_minor: record.proposed_daily_rate_minor,
        currency: record.currency.clone(),
        base_offer_revision: record.base_offer_revision,
        can_accept: is_renter && record.status == PRICE_CHANGE_PENDING,
        can_reject: is_renter && record.status == PRICE_CHANGE_PENDING,
        can_cancel: is_owner && record.status == PRICE_CHANGE_PENDING,
        status: record.status.clone(),
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
        responded_at: record.responded_at.clone(),
    })
}

fn subscription_view(
    record: SubscriptionRecord,
    viewer: Option<&AuthSession>,
    active_subdomains: &HashSet<String>,
    payment_profile: Option<&PaymentProfileSnapshot>,
    price_change: Option<&ActivePriceChangeRecord>,
) -> SubscriptionView {
    let is_renter = viewer.is_some_and(|session| session.user_id == record.renter_user_id);
    let is_owner = viewer.is_some_and(|session| session.user_id == record.owner_user_id);
    let (payment_method_kinds, contacts) = if is_renter || is_owner {
        payment_profile
            .map(|(methods, contacts, _)| (payment_method_kinds(methods), contacts.clone()))
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
    let can_retry_grant = is_owner
        && record.status == SUB_GRANT_FAILED
        && record.failure_code.as_deref() != Some(SHARE_GRANT_CONTRACT_VIOLATION_CODE)
        && !record.has_active_control_work
        && !record.has_active_billing_contract;
    let price_change = active_price_change_view(price_change, is_owner, is_renter);
    let can_propose_price_change = is_owner
        && record.status == SUB_ACTIVE_POSTPAID
        && record.daily_rate_minor.is_some()
        && price_change.is_none();
    let share_online =
        !record.subdomain.is_empty() && active_subdomains.contains(&record.subdomain);
    let apps = subscription_contract_apps(&record);
    let contract_violation = record.failure_code.as_deref()
        == Some(SHARE_GRANT_CONTRACT_VIOLATION_CODE)
        || record.release_reason.as_deref() == Some(SHARE_GRANT_CONTRACT_VIOLATION_CODE);
    let private_failure = record.status == SUB_GRANT_FAILED || contract_violation;
    let show_failure_details = (is_owner || is_renter) && private_failure;
    let release_reason = if private_failure && !show_failure_details {
        None
    } else {
        record.release_reason
    };
    let failure_code = show_failure_details
        .then(|| record.failure_code.clone())
        .flatten();
    let grant_attempts = show_failure_details
        .then_some(record.grant_attempts)
        .flatten()
        .and_then(|attempts| u32::try_from(attempts).ok());
    let termination_adjustment = record
        .termination_adjustment_json
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok());
    SubscriptionView {
        id: record.id,
        seat_id: record.seat_id,
        listing_id: record.listing_id,
        share_id: record.share_id,
        installation_id: record.installation_id,
        share_name: record.share_name,
        app_type: record.app_type,
        apps,
        subdomain: record.subdomain,
        share_online,
        owner_email: record.owner_email,
        renter_email: record.renter_email,
        status: record.status,
        integrity_state: record.integrity_state,
        integrity_reason: record.integrity_reason,
        integrity_violated_at: record.integrity_violated_at,
        termination_adjustment,
        seat_position: record.seat_position,
        parallel_limit: record
            .parallel_limit
            .and_then(|value| u32::try_from(value).ok()),
        token_limit: record
            .token_limit
            .and_then(|value| u64::try_from(value).ok()),
        token_period: serde_json::from_str(&record.token_period_json)
            .unwrap_or(ShareTokenPeriod::Lifetime),
        daily_rate_minor: record.daily_rate_minor,
        currency: record.currency,
        service_duration_days: record.service_duration_days,
        activated_at: record.activated_at,
        service_started_at: record.service_started_at,
        expires_at: record.expires_at,
        offer_revision: record.offer_revision,
        payment_method_kinds,
        contacts,
        can_release,
        can_force_revoke,
        can_retry_grant,
        can_propose_price_change,
        price_change,
        release_reason,
        failure_code,
        grant_attempts,
        released_at: record.released_at,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

impl AppStore {
    pub async fn share_market_owned_shares(
        &self,
        session: &AuthSession,
    ) -> Result<Vec<OwnedShareView>, AppError> {
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT s.share_id, s.share_name, s.app_type,
                        COALESCE(s.subdomain, ''), COALESCE(s.owner_email, ''),
                        COALESCE(s.bindings_json, '{}'), s.share_status,
                        s.parallel_limit, s.token_limit,
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
                        COALESCE(s.free_access, 0),
                        COALESCE(s.supported_user_token_periods_json, '[]'),
                        COALESCE(s.enabled_claude, 0), COALESCE(s.enabled_codex, 0),
                        COALESCE(s.enabled_gemini, 0),
                        (SELECT listing.id FROM share_market_listings listing
                         WHERE listing.share_id = s.share_id
                           AND listing.owner_user_id = ?2
                           AND lower(listing.owner_email) = lower(s.owner_email)
                           AND listing.status = 'active' AND listing.deleted_at IS NULL
                         ORDER BY listing.updated_at DESC LIMIT 1),
                        (SELECT listing.id FROM share_market_listings listing
                         WHERE listing.share_id = s.share_id
                           AND listing.owner_user_id = ?2
                           AND lower(listing.owner_email) = lower(s.owner_email)
                           AND listing.status = 'closed' AND listing.deleted_at IS NULL
                         ORDER BY listing.updated_at DESC, listing.created_at DESC LIMIT 1),
                        EXISTS (
                            SELECT 1 FROM share_market_subscriptions sub
                            WHERE sub.share_id = s.share_id
                              AND sub.status NOT IN ('released', 'grant_failed')
                        ),
                        COALESCE(s.share_access_policy_version, 0),
                        (SELECT edit.patch_json FROM share_edit_requests edit
                         WHERE edit.share_id = s.share_id AND edit.status = 'pending'
                           AND edit.retired_at IS NULL
                         ORDER BY edit.revision DESC LIMIT 1),
                        s.expires_at, s.app_runtimes_json, s.app_providers_json
                 FROM shares s
                 WHERE lower(s.owner_email) = lower(?1)
                 ORDER BY s.share_name, s.share_id",
            )
            .map_err(map_db("prepare owned Share list"))?;
        let rows = statement
            .query_map(params![session.email, session.user_id], |row| {
                let app_type: String = row.get(2)?;
                let bindings_json: String = row.get(5)?;
                let share_status: String = row.get(6)?;
                let parallel_limit: i64 = row.get(7)?;
                let token_limit: i64 = row.get(8)?;
                let periods_json: String = row.get(11)?;
                let free_access = row.get::<_, i64>(10)? != 0;
                let active_listing_id: Option<String> = row.get(15)?;
                let reopen_listing_id: Option<String> = row.get(16)?;
                let has_active_rentals = row.get::<_, i64>(17)? != 0;
                let contract_version: i64 = row.get(18)?;
                let pending_free_access = row
                    .get::<_, Option<String>>(19)?
                    .as_deref()
                    .and_then(|value| serde_json::from_str::<ShareSettingsPatch>(value).ok())
                    .and_then(|patch| patch.free_access)
                    .unwrap_or(false);
                let expires_at: String = row.get(20)?;
                let app_runtimes_json: Option<String> = row.get(21)?;
                let app_providers_json: Option<String> = row.get(22)?;
                let support = ShareSupport {
                    claude: row.get::<_, i64>(12)? != 0,
                    codex: row.get::<_, i64>(13)? != 0,
                    gemini: row.get::<_, i64>(14)? != 0,
                };
                let app_capabilities = enabled_public_app_capabilities(
                    &bindings_json,
                    app_runtimes_json.as_deref(),
                    app_providers_json.as_deref(),
                    &app_type,
                    &support,
                );
                let create_blocked_reason = if active_listing_id.is_some() {
                    Some("active_listing")
                } else if reopen_listing_id.is_some() {
                    Some("reopen_required")
                } else if has_active_rentals {
                    Some("active_rentals")
                } else if free_access {
                    Some("public_access_enabled")
                } else if share_status != "active" {
                    Some("share_inactive")
                } else if pending_free_access {
                    Some("pending_share_edit")
                } else if contract_version < i64::from(SHARE_CONTRACT_VERSION) {
                    Some("client_upgrade_required")
                } else {
                    None
                };
                let market_state = if active_listing_id.is_some() {
                    "listed"
                } else if reopen_listing_id.is_some() {
                    "stopped"
                } else if has_active_rentals {
                    "rented"
                } else if free_access {
                    "public_access"
                } else if share_status != "active" {
                    "inactive"
                } else {
                    "available"
                };
                Ok(OwnedShareView {
                    share_id: row.get(0)?,
                    share_name: row.get(1)?,
                    app_type: app_type.clone(),
                    subdomain: row.get(3)?,
                    owner_email: row.get(4)?,
                    supported_apps: enabled_share_apps(&bindings_json, &app_type, &support),
                    app_capabilities,
                    share_status,
                    expires_at: (!expires_at.trim().is_empty()).then_some(expires_at),
                    parallel_limit: (parallel_limit >= 0)
                        .then(|| u32::try_from(parallel_limit).ok())
                        .flatten(),
                    token_limit: (token_limit >= 0)
                        .then(|| u64::try_from(token_limit).ok())
                        .flatten(),
                    already_listed: row.get::<_, i64>(9)? != 0,
                    active_listing_id,
                    reopen_listing_id,
                    has_active_rentals,
                    market_state: market_state.into(),
                    can_create_listing: create_blocked_reason.is_none(),
                    create_blocked_reason: create_blocked_reason.map(str::to_string),
                    free_access,
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
        self.share_market_catalog_with_scope(
            viewer,
            active_subdomains,
            ShareMarketCatalogScope::Visible,
        )
        .await
    }

    pub async fn share_market_subscriptions_page(
        &self,
        session: &AuthSession,
        active_subdomains: &[String],
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ShareMarketSubscriptions, AppError> {
        let cursor = decode_descending_cursor(cursor)?;
        let conn = self.conn.lock().await;
        let active_ids = conn
            .prepare(
                "SELECT id FROM share_market_subscriptions
                 WHERE renter_user_id = ?1 AND status NOT IN ('released', 'grant_failed')
                 ORDER BY updated_at DESC, id DESC",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![session.user_id], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read active Share rental subscriptions"))?;
        let cursor_timestamp = cursor.as_ref().map(|value| value.timestamp.as_str());
        let cursor_id = cursor.as_ref().map(|value| value.id.as_str());
        let mut terminal_rows = conn
            .prepare(
                "SELECT id, released_at FROM share_market_subscriptions
                 WHERE renter_user_id = ?1 AND status IN ('released', 'grant_failed')
                   AND released_at IS NOT NULL
                   AND (?2 IS NULL OR released_at < ?2 OR (released_at = ?2 AND id < ?3))
                 ORDER BY released_at DESC, id DESC LIMIT ?4",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(
                        params![
                            session.user_id,
                            cursor_timestamp,
                            cursor_id,
                            i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX),
                        ],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read Share rental history page"))?;
        let has_more = terminal_rows.len() > limit;
        terminal_rows.truncate(limit);
        let next_cursor = if has_more {
            terminal_rows
                .last()
                .map(|(id, timestamp)| {
                    encode_descending_cursor(&DescendingCursor {
                        timestamp: timestamp.clone(),
                        id: id.clone(),
                    })
                })
                .transpose()?
        } else {
            None
        };
        let ids = active_ids
            .into_iter()
            .chain(terminal_rows.into_iter().map(|(id, _)| id))
            .collect::<Vec<_>>();
        let mut records = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(record) = subscription_record(&conn, &id)? {
                records.push(record);
            }
        }
        let owner_ids = records
            .iter()
            .map(|record| record.owner_user_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let subscription_ids = records
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        let profiles = payment_profiles(&conn, &owner_ids)?;
        let price_changes = active_price_changes(&conn, &subscription_ids)?;
        let active_subdomains = active_subdomains.iter().cloned().collect::<HashSet<_>>();
        let subscriptions = records
            .into_iter()
            .map(|record| {
                let profile = profiles.get(&record.owner_user_id);
                let price_change = price_changes.get(&record.id);
                subscription_view(
                    record,
                    Some(session),
                    &active_subdomains,
                    profile,
                    price_change,
                )
            })
            .collect();
        Ok(ShareMarketSubscriptions {
            subscriptions,
            next_cursor,
        })
    }

    async fn share_market_catalog_with_scope(
        &self,
        viewer: Option<&AuthSession>,
        active_subdomains: &[String],
        scope: ShareMarketCatalogScope,
    ) -> Result<ShareMarketCatalog, AppError> {
        let conn = self.conn.lock().await;
        let visibility = catalog_visibility_predicate(scope, "s");
        let listings_sql = format!(
            "SELECT listing.id, listing.share_id, COALESCE(s.share_name, listing.share_id),
                    COALESCE(s.app_type, ''), listing.owner_user_id, listing.owner_email,
                    listing.status, COALESCE(s.share_status, 'missing'),
                    COALESCE(s.subdomain, ''), listing.created_at, listing.updated_at,
                    COALESCE(s.supported_user_token_periods_json, '[]'),
                    COALESCE(s.owner_email, ''), COALESCE(s.user_grants_json, '{{}}'),
                    COALESCE(s.bindings_json, '{{}}'),
                    COALESCE(s.enabled_claude, 0), COALESCE(s.enabled_codex, 0),
                    COALESCE(s.enabled_gemini, 0), s.app_runtimes_json,
                    s.app_providers_json, s.token_limit, s.parallel_limit,
                    COALESCE(s.tokens_used, 0), listing.installation_id
             FROM share_market_listings listing
             LEFT JOIN shares s ON s.share_id = listing.share_id
             WHERE listing.deleted_at IS NULL
               AND ({visibility})
             ORDER BY listing.created_at DESC"
        );
        let mut listings_statement = conn
            .prepare(&listings_sql)
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
                    row.get::<_, String>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, i64>(16)?,
                    row.get::<_, i64>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, Option<String>>(19)?,
                    row.get::<_, Option<i64>>(20)?,
                    row.get::<_, Option<i64>>(21)?,
                    row.get::<_, i64>(22)?,
                    row.get::<_, String>(23)?,
                ))
            })
            .map_err(map_db("query Share Market catalog"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read Share Market catalog"))?;
        drop(listings_statement);

        let active_subdomains = active_subdomains.iter().cloned().collect::<HashSet<_>>();
        let catalog_share_ids = listing_rows
            .iter()
            .map(|row| row.1.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut seats_by_listing = catalog_seats(&conn, viewer_user_id, scope)?;
        let subscription_records = if scope == ShareMarketCatalogScope::Public {
            HashMap::new()
        } else {
            catalog_subscription_records(&conn, viewer_user_id, scope)?
        };
        let related_user_ids = listing_rows
            .iter()
            .map(|row| row.4.clone())
            .chain(
                subscription_records
                    .values()
                    .map(|record| record.owner_user_id.clone()),
            )
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let subscription_ids = subscription_records.keys().cloned().collect::<Vec<_>>();
        let payment_profiles = payment_profiles(&conn, &related_user_ids)?;
        let price_changes = active_price_changes(&conn, &subscription_ids)?;
        let mut performance_by_share = share_market_performance(&conn, &catalog_share_ids)?;
        let reliability_by_share = share_market_reliability(&conn, &catalog_share_ids)?;
        let viewer_active_share_ids = active_rented_share_ids(&conn, viewer_user_id)?;
        let mut eligibility_by_supplier_pricing = HashMap::<
            (String, String, Option<String>),
            crate::market_access::MarketEligibilityView,
        >::new();
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
            bindings_json,
            enabled_claude,
            enabled_codex,
            enabled_gemini,
            app_runtimes_json,
            app_providers_json,
            token_limit,
            parallel_limit,
            tokens_used,
            installation_id,
        ) in listing_rows
        {
            let is_owner = viewer.is_some_and(|value| value.user_id == owner_user_id);
            let share_online = !subdomain.is_empty() && active_subdomains.contains(&subdomain);
            let support = ShareSupport {
                claude: enabled_claude != 0,
                codex: enabled_codex != 0,
                gemini: enabled_gemini != 0,
            };
            let app_capabilities = enabled_public_app_capabilities(
                &bindings_json,
                app_runtimes_json.as_deref(),
                app_providers_json.as_deref(),
                &app_type,
                &support,
            );
            let supported_apps = app_capabilities
                .iter()
                .map(|capability| capability.app.clone())
                .collect::<Vec<_>>();
            let mut app_availability = Vec::with_capacity(supported_apps.len());
            for app in &supported_apps {
                app_availability.push(evaluate_share_market_service_availability_tx(
                    &conn,
                    &share_id,
                    app,
                    Some(share_online),
                    None,
                    None,
                    Utc::now(),
                )?);
            }
            let service_available = !app_availability.is_empty()
                && app_availability
                    .iter()
                    .all(ShareMarketServiceAvailability::available);
            let service_state = if service_available {
                if app_availability
                    .iter()
                    .all(|availability| availability.state == "available")
                {
                    "available"
                } else {
                    "degraded"
                }
            } else {
                "unavailable"
            };
            let mut service_reasons = Vec::new();
            for reason in app_availability
                .iter()
                .flat_map(|availability| availability.reasons.iter())
            {
                push_reason(&mut service_reasons, reason.clone());
            }
            let (provider_family, provider_families) = listing_provider_families(&app_capabilities);
            let viewer_already_renting = viewer_active_share_ids.contains(&share_id);
            let viewer_has_direct_grant = viewer.is_some_and(|session| {
                let grants: BTreeMap<String, ShareUserGrant> =
                    serde_json::from_str(&grants_json).unwrap_or_default();
                grants
                    .get(&session.email.to_ascii_lowercase())
                    .is_some_and(|grant| grant.active)
            });
            let seat_rows = seats_by_listing.remove(&id).unwrap_or_default();
            let mut seats = Vec::with_capacity(seat_rows.len());
            for seat in seat_rows {
                let pricing_kind =
                    crate::market_access::pricing_kind_for_rate(seat.daily_rate_minor);
                let eligibility_key = (
                    owner_user_id.clone(),
                    pricing_kind.to_string(),
                    seat.currency.clone(),
                );
                let eligibility = match viewer {
                    Some(session) if session.user_id == owner_user_id => {
                        crate::market_access::MarketEligibilityView::allowed()
                    }
                    Some(session) => {
                        if let Some(eligibility) =
                            eligibility_by_supplier_pricing.get(&eligibility_key)
                        {
                            eligibility.clone()
                        } else {
                            let eligibility = crate::market_access::market_eligibility_tx(
                                &conn,
                                &owner_user_id,
                                &session.user_id,
                                &session.email,
                                crate::market_access::PRODUCT_SHARE,
                                seat.daily_rate_minor,
                                seat.currency.as_deref(),
                            )?;
                            eligibility_by_supplier_pricing
                                .insert(eligibility_key, eligibility.clone());
                            eligibility
                        }
                    }
                    None => crate::market_access::MarketEligibilityView::login_required(),
                };
                let subscription_id = seat
                    .current_subscription_id
                    .as_ref()
                    .or(seat.retired_subscription_id.as_ref());
                let subscription_record = subscription_id
                    .and_then(|subscription_id| subscription_records.get(subscription_id))
                    .cloned();
                let delete_capability = seat_delete_capability(
                    is_owner,
                    &seat.status,
                    seat.retired_at.as_deref(),
                    seat.subscription_count,
                    subscription_record.as_ref(),
                );
                let subscription = subscription_record.map(|record| {
                    let payment_profile = payment_profiles.get(&record.owner_user_id);
                    let price_change = price_changes.get(&record.id);
                    subscription_view(
                        record,
                        viewer,
                        &active_subdomains,
                        payment_profile,
                        price_change,
                    )
                });
                let base_rent_prerequisites = viewer.is_some_and(|session| {
                    status == "active"
                        && share_status == "active"
                        && share_online
                        && service_available
                        && current_owner_email.eq_ignore_ascii_case(&owner_email)
                        && seat.status == SEAT_AVAILABLE
                        && seat.retired_at.is_none()
                        && session.user_id != owner_user_id
                        && !viewer_already_renting
                        && !viewer_has_direct_grant
                });
                let seller_approval_required =
                    base_rent_prerequisites && eligibility.status == "access_required";
                let can_rent = base_rent_prerequisites && eligibility.allowed;
                let rent_block_reason = if can_rent {
                    None
                } else if is_owner {
                    Some("owner".to_string())
                } else if viewer_already_renting {
                    Some("already_renting".to_string())
                } else if viewer_has_direct_grant {
                    Some("direct_access".to_string())
                } else if viewer.is_none() {
                    Some("login_required".to_string())
                } else if status != "active" || share_status != "active" || !service_available {
                    Some("share_unavailable".to_string())
                } else if !share_online {
                    Some("share_offline".to_string())
                } else if seat.status != SEAT_AVAILABLE || seat.retired_at.is_some() {
                    Some("seat_unavailable".to_string())
                } else if seller_approval_required {
                    Some("approval_required".to_string())
                } else if !eligibility.allowed {
                    Some(eligibility.status.clone())
                } else {
                    Some("unavailable".to_string())
                };
                let read_only = seat.retired_at.is_some();
                let can_republish = is_owner
                    && status == "closed"
                    && seat.status == SEAT_DISABLED
                    && seat.retired_at.is_none()
                    && seat.current_subscription_id.is_none();
                seats.push(SeatView {
                    id: seat.id,
                    position: seat.position,
                    status: if read_only {
                        SEAT_RETIRED_VIEW.to_string()
                    } else {
                        seat.status
                    },
                    parallel_limit: seat
                        .parallel_limit
                        .and_then(|value| u32::try_from(value).ok()),
                    token_limit: seat.token_limit.and_then(|value| u64::try_from(value).ok()),
                    token_period: serde_json::from_str(&seat.token_period_json)
                        .unwrap_or(ShareTokenPeriod::Lifetime),
                    daily_rate_minor: seat.daily_rate_minor,
                    currency: seat.currency,
                    service_duration_days: seat
                        .service_duration_days
                        .and_then(|value| u32::try_from(value).ok()),
                    offer_revision: seat.offer_revision,
                    is_free: seat.daily_rate_minor.is_none(),
                    can_rent,
                    rent_prerequisites_met: base_rent_prerequisites,
                    seller_approval_required,
                    rent_block_reason,
                    eligibility,
                    read_only,
                    can_delete: delete_capability.can_delete,
                    delete_blocked_reason: delete_capability.blocked_reason.map(str::to_string),
                    can_republish,
                    retired_at: seat.retired_at,
                    subscription,
                });
            }
            let (payment_method_kinds, contacts) = payment_profiles
                .get(&owner_user_id)
                .map(|(methods, contacts, _)| (payment_method_kinds(methods), contacts.clone()))
                .unwrap_or_default();
            let publicly_listed = status == "active"
                && share_status == "active"
                && current_owner_email.eq_ignore_ascii_case(&owner_email);
            let performance = performance_by_share.remove(&share_id).unwrap_or_default();
            let reliability = reliability_view(reliability_by_share.get(&share_id).copied());
            let delete_capability = listing_delete_capability_tx(&conn, &id, &status, is_owner)?;
            let reopen_capability = listing_reopen_capability_tx(&conn, &id, &status, is_owner)?;
            listings.push(ListingView {
                id,
                share_id,
                installation_id,
                share_name,
                app_type,
                supported_apps,
                provider_family,
                provider_families,
                app_capabilities,
                owner_email,
                status,
                share_status,
                subdomain: subdomain.clone(),
                share_online,
                service_state: service_state.into(),
                service_reasons,
                is_owner,
                can_delete: delete_capability.can_delete,
                delete_blocked_reason: delete_capability.blocked_reason.map(str::to_string),
                can_reopen: reopen_capability.can_reopen,
                reopen_blocked_reason: reopen_capability.blocked_reason.map(str::to_string),
                reopenable_seat_count: reopen_capability.reopenable_seat_count,
                publicly_listed,
                contacts,
                payment_method_kinds,
                performance,
                reliability,
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

        let mut my_subscription_records = if scope == ShareMarketCatalogScope::Public {
            Vec::new()
        } else {
            viewer
                .map(|viewer| {
                    subscription_records
                        .values()
                        .filter(|record| record.renter_user_id == viewer.user_id)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        my_subscription_records.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        let my_subscriptions = my_subscription_records
            .into_iter()
            .map(|record| {
                let payment_profile = payment_profiles.get(&record.owner_user_id);
                let price_change = price_changes.get(&record.id);
                subscription_view(
                    record,
                    viewer,
                    &active_subdomains,
                    payment_profile,
                    price_change,
                )
            })
            .collect();
        Ok(ShareMarketCatalog {
            listings,
            my_subscriptions,
            trial_hours: TRIAL_HOURS,
        })
    }
}

fn if_none_match_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
            })
        })
}

fn private_etag_json<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<Response, AppError> {
    let body = serde_json::to_vec(value).map_err(|error| {
        AppError::Internal(format!("encode Share Market response failed: {error}"))
    })?;
    let etag = format!("\"{}\"", crate::api::sha256_hex(&body));
    let not_modified = if_none_match_matches(headers, &etag);
    Response::builder()
        .status(if not_modified {
            StatusCode::NOT_MODIFIED
        } else {
            StatusCode::OK
        })
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, "private, no-cache")
        .header(header::VARY, header::AUTHORIZATION.as_str())
        .header(header::CONTENT_TYPE, "application/json")
        .body(if not_modified {
            Body::empty()
        } else {
            Body::from(body)
        })
        .map_err(|error| AppError::Internal(format!("build Share Market response failed: {error}")))
}

fn encode_descending_cursor(cursor: &DescendingCursor) -> Result<String, AppError> {
    serde_json::to_vec(cursor)
        .map(|value| URL_SAFE_NO_PAD.encode(value))
        .map_err(|error| AppError::Internal(format!("encode Share Market cursor failed: {error}")))
}

fn decode_descending_cursor(value: Option<&str>) -> Result<Option<DescendingCursor>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AppError::BadRequest("invalid Share Market cursor".into()))?;
    let cursor = serde_json::from_slice::<DescendingCursor>(&bytes)
        .map_err(|_| AppError::BadRequest("invalid Share Market cursor".into()))?;
    if cursor.timestamp.trim().is_empty() || cursor.id.trim().is_empty() {
        return Err(AppError::BadRequest("invalid Share Market cursor".into()));
    }
    Ok(Some(cursor))
}

async fn list_catalog(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let viewer = crate::api::resolve_router_session(&state, &headers).await?;
    let active_subdomains = state.proxy.active_subdomains().await;
    let mut catalog = state
        .store
        .share_market_catalog_with_scope(
            viewer.as_ref(),
            &active_subdomains,
            ShareMarketCatalogScope::Public,
        )
        .await?;
    retain_public_catalog(&mut catalog);
    private_etag_json(&headers, &catalog)
}

async fn list_my_listings(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let session = require_session(&state, &headers).await?;
    let active_subdomains = state.proxy.active_subdomains().await;
    let catalog = state
        .store
        .share_market_catalog_with_scope(
            Some(&session),
            &active_subdomains,
            ShareMarketCatalogScope::Owner,
        )
        .await?;
    private_etag_json(
        &headers,
        &ShareMarketOwnedListings {
            listings: catalog.listings,
        },
    )
}

async fn list_my_subscriptions(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<SubscriptionPageQuery>,
) -> Result<Response, AppError> {
    let session = require_session(&state, &headers).await?;
    let active_subdomains = state.proxy.active_subdomains().await;
    let page = state
        .store
        .share_market_subscriptions_page(
            &session,
            &active_subdomains,
            query.cursor.as_deref(),
            query.limit.unwrap_or(8).clamp(1, 50),
        )
        .await?;
    private_etag_json(&headers, &page)
}

async fn list_control_dead_letters(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ControlDeadLetterQuery>,
) -> Result<Json<ShareControlDeadLetterPage>, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .share_market_control_dead_letters(
                query.cursor.as_deref(),
                query.limit.unwrap_or(20).clamp(1, 50),
            )
            .await?,
    ))
}

async fn requeue_control_dead_letter(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    state
        .store
        .share_market_requeue_control_dead_letter(&session, &operation_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
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
        return Err(AppError::coded_conflict(
            ERROR_SHARE_MARKET_PAYMENT_PROFILE_REQUIRED,
            "configure Account payment details before adding a paid Share seat",
            serde_json::json!({ "productKind": "share" }),
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
            token_period_json, daily_rate_minor, currency, service_duration_days,
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
            seat.service_duration_days.map(i64::from),
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
         SET status = 'disabled', offer_revision = offer_revision + 1, updated_at = ?3
         WHERE listing_id IN (
             SELECT id FROM share_market_listings
             WHERE share_id = ?1 AND lower(owner_email) != lower(?2) AND status = 'closed'
         ) AND status = 'available'",
        params![share_id, current_owner_email, now],
    )
    .map_err(map_db("disable stale Share listing seats"))?;
    tx.execute(
        "UPDATE share_market_rent_quotes
         SET status = 'expired', updated_at = ?3
         WHERE status = 'active' AND seat_id IN (
             SELECT seat.id FROM share_market_seats seat
             JOIN share_market_listings listing ON listing.id = seat.listing_id
             WHERE listing.share_id = ?1
               AND lower(listing.owner_email) != lower(?2)
         )",
        params![share_id, current_owner_email, now],
    )
    .map_err(map_db("expire stale Share listing quotes"))?;
    Ok(())
}

fn ensure_no_pending_free_access_edit_tx(
    conn: &Connection,
    share_id: &str,
) -> Result<(), AppError> {
    if pending_edit_enables_free_access_tx(conn, share_id)? {
        return Err(AppError::Conflict(
            "wait for the pending public free access edit before listing this Share".into(),
        ));
    }
    Ok(())
}

fn pending_edit_enables_free_access_tx(
    conn: &Connection,
    share_id: &str,
) -> Result<bool, AppError> {
    let patch_json = conn
        .query_row(
            "SELECT patch_json FROM share_edit_requests
             WHERE share_id = ?1 AND status = 'pending' AND retired_at IS NULL
             ORDER BY revision DESC LIMIT 1",
            params![share_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read pending Share access edit"))?;
    let Some(patch_json) = patch_json else {
        return Ok(false);
    };
    let patch: ShareSettingsPatch = serde_json::from_str(&patch_json).map_err(|error| {
        AppError::Internal(format!("decode pending Share edit failed: {error}"))
    })?;
    Ok(patch.free_access.unwrap_or(false))
}

#[derive(Debug)]
struct ListingPublishContext {
    share_id: String,
    listing_status: String,
    supported_periods: Vec<ShareTokenPeriod>,
    share_parallel_limit: i64,
}

fn listing_publish_context_tx(
    tx: &Transaction<'_>,
    session: &AuthSession,
    listing_id: &str,
) -> Result<ListingPublishContext, AppError> {
    let listing = tx
        .query_row(
            "SELECT listing.owner_user_id, listing.owner_email, share.owner_email,
                    share.share_status,
                    COALESCE(share.supported_user_token_periods_json, '[]'),
                    listing.share_id, listing.status, COALESCE(share.free_access, 0),
                    share.parallel_limit
             FROM share_market_listings listing
             JOIN shares share ON share.share_id = listing.share_id
             WHERE listing.id = ?1 AND listing.deleted_at IS NULL",
            params![listing_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)? != 0,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read Share listing publish context"))?
        .ok_or_else(|| AppError::NotFound("listing not found".into()))?;
    if listing.0 != session.user_id {
        return Err(AppError::Forbidden(
            "only listing owner can publish seats".into(),
        ));
    }
    if listing.3 != "active" || !listing.1.eq_ignore_ascii_case(&listing.2) {
        return Err(AppError::Conflict(
            "listing Share is no longer active or owned by this account".into(),
        ));
    }
    ensure_share_market_app_scope_supported_tx(tx, &listing.5)?;
    if listing.7 {
        return Err(AppError::Conflict(
            "disable public free access before adding or reopening Share Market seats".into(),
        ));
    }
    ensure_no_pending_free_access_edit_tx(tx, &listing.5)?;
    Ok(ListingPublishContext {
        share_id: listing.5,
        listing_status: listing.6,
        supported_periods: serde_json::from_str(&listing.4)
            .unwrap_or_else(|_| vec![ShareTokenPeriod::Lifetime]),
        share_parallel_limit: listing.8,
    })
}

fn validate_publish_seats_tx(
    tx: &Transaction<'_>,
    session: &AuthSession,
    context: &ListingPublishContext,
    seats: &[NormalizedSeat],
) -> Result<(), AppError> {
    if seats.iter().any(|seat| {
        seat.token_limit.is_some() && !context.supported_periods.contains(&seat.token_period)
    }) {
        return Err(AppError::BadRequest(
            "a seat uses a token period unsupported by this Server".into(),
        ));
    }
    for seat in seats {
        validate_seat_share_capacity(seat, context.share_parallel_limit)?;
    }
    if seats.iter().any(|seat| !seat.is_free()) {
        ensure_payment_profile_tx(tx, &session.user_id)?;
        for currency in seats
            .iter()
            .filter_map(|seat| seat.currency.as_deref())
            .collect::<BTreeSet<_>>()
        {
            crate::market_billing::require_supplier_profile_tx(tx, &session.user_id, currency)?;
        }
    }
    Ok(())
}

fn other_active_listing_tx(
    tx: &Transaction<'_>,
    share_id: &str,
    listing_id: &str,
) -> Result<Option<String>, AppError> {
    tx.query_row(
        "SELECT id FROM share_market_listings
         WHERE share_id = ?1 AND id != ?2 AND status = 'active'
           AND deleted_at IS NULL
         ORDER BY updated_at DESC LIMIT 1",
        params![share_id, listing_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(map_db("check other active Share listing"))
}

fn other_active_listing_error(listing_id: &str) -> AppError {
    AppError::coded_conflict(
        ERROR_OTHER_ACTIVE_LISTING,
        "another listing for this Share is already active",
        serde_json::json!({ "listingId": listing_id }),
    )
}

fn expire_listing_rent_quotes_tx(
    tx: &Transaction<'_>,
    listing_id: &str,
    now: &str,
) -> Result<usize, AppError> {
    tx.execute(
        "UPDATE share_market_rent_quotes
         SET status = 'expired', updated_at = ?2
         WHERE status = 'active' AND seat_id IN (
             SELECT id FROM share_market_seats WHERE listing_id = ?1
         )",
        params![listing_id, now],
    )
    .map_err(map_db("expire Share listing rent quotes"))
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
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Share Market listing"))?;
        let share: Option<(String, String, String, String, bool, i64)> = tx
            .query_row(
                "SELECT owner_email, share_status,
                        COALESCE(supported_user_token_periods_json, '[]'), installation_id,
                        COALESCE(free_access, 0), parallel_limit
                 FROM shares WHERE share_id = ?1",
                params![share_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get::<_, i64>(4)? != 0,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read listing Share"))?;
        let Some((
            owner_email,
            share_status,
            periods_json,
            installation_id,
            free_access,
            share_parallel_limit,
        )) = share
        else {
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
        ensure_share_market_app_scope_supported_tx(&tx, share_id)?;
        if free_access {
            return Err(AppError::Conflict(
                "disable public free access before listing the Share in Share Market".into(),
            ));
        }
        ensure_no_pending_free_access_edit_tx(&tx, share_id)?;
        close_reclaimable_stale_listings_tx(&tx, share_id, &session.email, &now)?;
        let active_listing_id = tx
            .query_row(
                "SELECT id FROM share_market_listings
                 WHERE share_id = ?1 AND status = 'active' AND deleted_at IS NULL
                 ORDER BY updated_at DESC LIMIT 1",
                params![share_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("check active Share listing"))?;
        if let Some(active_listing_id) = active_listing_id {
            return Err(other_active_listing_error(&active_listing_id));
        }
        let reopen_listing_id = tx
            .query_row(
                "SELECT id FROM share_market_listings
                 WHERE share_id = ?1 AND owner_user_id = ?2
                   AND lower(owner_email) = lower(?3)
                   AND status = 'closed' AND deleted_at IS NULL
                 ORDER BY updated_at DESC, created_at DESC LIMIT 1",
                params![share_id, session.user_id, session.email],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("check stopped Share listing"))?;
        if let Some(reopen_listing_id) = reopen_listing_id {
            return Err(AppError::coded_conflict(
                ERROR_REOPEN_REQUIRED,
                "this Share has a stopped listing that must be reopened",
                serde_json::json!({ "listingId": reopen_listing_id }),
            ));
        }
        let supported: Vec<ShareTokenPeriod> = serde_json::from_str(&periods_json)
            .unwrap_or_else(|_| vec![ShareTokenPeriod::Lifetime]);
        if seats
            .iter()
            .any(|seat| seat.token_limit.is_some() && !supported.contains(&seat.token_period))
        {
            return Err(AppError::BadRequest(
                "a seat uses a token period unsupported by this Server".into(),
            ));
        }
        for seat in &seats {
            validate_seat_share_capacity(seat, share_parallel_limit)?;
        }
        if seats.iter().any(|seat| !seat.is_free()) {
            ensure_payment_profile_tx(&tx, &session.user_id)?;
            for currency in seats
                .iter()
                .filter_map(|seat| seat.currency.as_deref())
                .collect::<BTreeSet<_>>()
            {
                crate::market_billing::require_supplier_profile_tx(
                    &tx,
                    &session.user_id,
                    currency,
                )?;
            }
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
                AppError::coded_conflict(
                    ERROR_OTHER_ACTIVE_LISTING,
                    "another listing for this Share is already active",
                    serde_json::json!({}),
                )
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
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin add Share seat"))?;
        let context = listing_publish_context_tx(&tx, session, listing_id)?;
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
        validate_publish_seats_tx(&tx, session, &context, std::slice::from_ref(&seat))?;
        close_reclaimable_stale_listings_tx(&tx, &context.share_id, &session.email, &now)?;
        if let Some(active_listing_id) =
            other_active_listing_tx(&tx, &context.share_id, listing_id)?
        {
            return Err(other_active_listing_error(&active_listing_id));
        }
        if context.listing_status != "active" {
            let changed = tx
                .execute(
                    "UPDATE share_market_listings
                     SET status = 'active', updated_at = ?2
                     WHERE id = ?1 AND status = 'closed' AND deleted_at IS NULL",
                    params![listing_id, now],
                )
                .map_err(|error| {
                    if error.to_string().contains("UNIQUE constraint failed") {
                        AppError::coded_conflict(
                            ERROR_OTHER_ACTIVE_LISTING,
                            "another listing for this Share is already active",
                            serde_json::json!({}),
                        )
                    } else {
                        AppError::Internal(format!("reopen Share listing failed: {error}"))
                    }
                })?;
            if changed != 1 {
                return Err(AppError::coded_conflict(
                    ERROR_LISTING_NOT_CLOSED,
                    "listing is no longer stopped",
                    serde_json::json!({ "listingId": listing_id }),
                ));
            }
            expire_listing_rent_quotes_tx(&tx, listing_id, &now)?;
        }
        let position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM share_market_seats WHERE listing_id = ?1",
                params![listing_id],
                |row| row.get(0),
            )
            .map_err(map_db("choose Share seat position"))?;
        let seat_id = insert_seat_tx(&tx, listing_id, position, &seat, &now)?;
        if context.listing_status != "active" {
            event_tx(
                &tx,
                Some(listing_id),
                None,
                None,
                Some(session),
                "listing_relisted",
                serde_json::json!({
                    "reopenedSeatIds": [],
                    "newSeatIds": [&seat_id],
                }),
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

    pub async fn share_market_reopen_listing(
        &self,
        session: &AuthSession,
        listing_id: &str,
        input: ReopenListingRequest,
    ) -> Result<ReopenListingResponse, AppError> {
        let requested_count = input.existing_seats.len() + input.new_seats.len();
        if requested_count == 0 || requested_count > MAX_SEATS_PER_LISTING {
            return Err(AppError::BadRequest(format!(
                "reopening requires 1-{MAX_SEATS_PER_LISTING} existing or new seats"
            )));
        }
        let mut seen_seat_ids = HashSet::new();
        let existing_seats = input
            .existing_seats
            .into_iter()
            .map(|item| {
                let seat_id = item.seat_id.trim().to_string();
                if seat_id.is_empty() {
                    return Err(AppError::BadRequest("seatId is required".into()));
                }
                if !seen_seat_ids.insert(seat_id.clone()) {
                    return Err(AppError::BadRequest(
                        "a seat can be selected only once when reopening".into(),
                    ));
                }
                Ok((seat_id, item.offer_revision, normalize_seat(item.seat)?))
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let new_seats = input
            .new_seats
            .into_iter()
            .map(normalize_seat)
            .collect::<Result<Vec<_>, _>>()?;
        let all_seats = existing_seats
            .iter()
            .map(|(_, _, seat)| seat.clone())
            .chain(new_seats.iter().cloned())
            .collect::<Vec<_>>();

        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin reopen Share listing"))?;
        let context = listing_publish_context_tx(&tx, session, listing_id)?;
        if context.listing_status != "closed" {
            return Err(AppError::coded_conflict(
                ERROR_LISTING_NOT_CLOSED,
                "only a stopped listing can be reopened",
                serde_json::json!({ "listingId": listing_id }),
            ));
        }
        validate_publish_seats_tx(&tx, session, &context, &all_seats)?;
        close_reclaimable_stale_listings_tx(&tx, &context.share_id, &session.email, &now)?;
        if let Some(active_listing_id) =
            other_active_listing_tx(&tx, &context.share_id, listing_id)?
        {
            return Err(other_active_listing_error(&active_listing_id));
        }
        let active_seat_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM share_market_seats
                 WHERE listing_id = ?1 AND retired_at IS NULL
                   AND status IN ('available', 'reserved', 'occupied', 'revoking')",
                params![listing_id],
                |row| row.get(0),
            )
            .map_err(map_db("count active Share seats before reopen"))?;
        if active_seat_count + i64::try_from(requested_count).unwrap_or(i64::MAX)
            > MAX_SEATS_PER_LISTING as i64
        {
            return Err(AppError::Conflict("listing seat limit reached".into()));
        }

        for (seat_id, expected_revision, _) in &existing_seats {
            let state = tx
                .query_row(
                    "SELECT status, offer_revision, retired_at, current_subscription_id
                     FROM share_market_seats
                     WHERE id = ?1 AND listing_id = ?2",
                    params![seat_id, listing_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(map_db("read stopped Share seat"))?;
            let Some((status, revision, retired_at, subscription_id)) = state else {
                return Err(AppError::coded_conflict(
                    ERROR_SEAT_NOT_REOPENABLE,
                    "seat does not belong to this listing",
                    serde_json::json!({ "seatId": seat_id, "reason": "not_found" }),
                ));
            };
            if revision != *expected_revision {
                return Err(AppError::coded_conflict(
                    ERROR_OFFER_CHANGED,
                    "seat offer changed; reload and retry",
                    serde_json::json!({
                        "seatId": seat_id,
                        "expectedOfferRevision": expected_revision,
                        "currentOfferRevision": revision,
                    }),
                ));
            }
            if status != SEAT_DISABLED || retired_at.is_some() || subscription_id.is_some() {
                return Err(AppError::coded_conflict(
                    ERROR_SEAT_NOT_REOPENABLE,
                    "seat is not eligible to be republished",
                    serde_json::json!({ "seatId": seat_id, "reason": "seat_not_reopenable" }),
                ));
            }
        }

        expire_listing_rent_quotes_tx(&tx, listing_id, &now)?;
        let mut reopened_seat_ids = Vec::with_capacity(existing_seats.len());
        for (seat_id, expected_revision, seat) in &existing_seats {
            let token_period_json = serde_json::to_string(&seat.token_period).map_err(|error| {
                AppError::Internal(format!("encode reopened seat token period failed: {error}"))
            })?;
            let changed = tx
                .execute(
                    "UPDATE share_market_seats
                     SET status = 'available', parallel_limit = ?3, token_limit = ?4,
                         token_period_json = ?5, daily_rate_minor = ?6, currency = ?7,
                         service_duration_days = ?8, offer_revision = offer_revision + 1,
                         updated_at = ?9
                     WHERE id = ?1 AND listing_id = ?2 AND status = 'disabled'
                       AND retired_at IS NULL AND current_subscription_id IS NULL
                       AND offer_revision = ?10",
                    params![
                        seat_id,
                        listing_id,
                        seat.parallel_limit.map(i64::from),
                        seat.token_limit.and_then(|value| i64::try_from(value).ok()),
                        token_period_json,
                        seat.daily_rate_minor,
                        seat.currency,
                        seat.service_duration_days.map(i64::from),
                        now,
                        expected_revision,
                    ],
                )
                .map_err(map_db("republish stopped Share seat"))?;
            if changed != 1 {
                return Err(AppError::coded_conflict(
                    ERROR_OFFER_CHANGED,
                    "seat offer changed; reload and retry",
                    serde_json::json!({ "seatId": seat_id }),
                ));
            }
            reopened_seat_ids.push(seat_id.clone());
        }

        let mut next_position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM share_market_seats
                 WHERE listing_id = ?1",
                params![listing_id],
                |row| row.get(0),
            )
            .map_err(map_db("choose reopened Share seat position"))?;
        let mut new_seat_ids = Vec::with_capacity(new_seats.len());
        for seat in &new_seats {
            new_seat_ids.push(insert_seat_tx(&tx, listing_id, next_position, seat, &now)?);
            next_position += 1;
        }
        let activated = tx
            .execute(
                "UPDATE share_market_listings
                 SET status = 'active', updated_at = ?2
                 WHERE id = ?1 AND status = 'closed' AND deleted_at IS NULL",
                params![listing_id, now],
            )
            .map_err(|error| {
                if error.to_string().contains("UNIQUE constraint failed") {
                    AppError::coded_conflict(
                        ERROR_OTHER_ACTIVE_LISTING,
                        "another listing for this Share is already active",
                        serde_json::json!({}),
                    )
                } else {
                    AppError::Internal(format!("activate reopened Share listing failed: {error}"))
                }
            })?;
        if activated != 1 {
            return Err(AppError::coded_conflict(
                ERROR_LISTING_NOT_CLOSED,
                "listing is no longer stopped",
                serde_json::json!({ "listingId": listing_id }),
            ));
        }
        event_tx(
            &tx,
            Some(listing_id),
            None,
            None,
            Some(session),
            "listing_relisted",
            serde_json::json!({
                "reopenedSeatIds": reopened_seat_ids,
                "newSeatIds": new_seat_ids,
            }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit reopen Share listing"))?;
        Ok(ReopenListingResponse {
            ok: true,
            listing_id: listing_id.to_string(),
            reopened_seat_ids,
            new_seat_ids,
        })
    }

    pub async fn share_market_update_seat(
        &self,
        session: &AuthSession,
        seat_id: &str,
        input: UpdateSeatRequest,
    ) -> Result<(), AppError> {
        let seat = normalize_seat(input.seat)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin update Share seat"))?;
        let row: Option<(
            String,
            String,
            i64,
            String,
            String,
            String,
            Option<String>,
            i64,
        )> = tx
            .query_row(
                "SELECT listing.owner_user_id, seat.status, seat.offer_revision,
                        listing.owner_email, s.owner_email,
                        COALESCE(s.supported_user_token_periods_json, '[]'), seat.retired_at,
                        s.parallel_limit
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
                        row.get(7)?,
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
            share_parallel_limit,
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
            return Err(AppError::coded_conflict(
                ERROR_OFFER_CHANGED,
                "seat offer changed; reload and retry",
                serde_json::json!({
                    "seatId": seat_id,
                    "expectedOfferRevision": input.offer_revision,
                    "currentOfferRevision": offer_revision,
                }),
            ));
        }
        let supported: Vec<ShareTokenPeriod> = serde_json::from_str(&periods_json)
            .unwrap_or_else(|_| vec![ShareTokenPeriod::Lifetime]);
        if seat.token_limit.is_some() && !supported.contains(&seat.token_period) {
            return Err(AppError::BadRequest(
                "token period is unsupported by this Server".into(),
            ));
        }
        validate_seat_share_capacity(&seat, share_parallel_limit)?;
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
        let changed = tx
            .execute(
                "UPDATE share_market_seats
             SET parallel_limit = ?2, token_limit = ?3, token_period_json = ?4,
                 daily_rate_minor = ?5, currency = ?6, service_duration_days = ?7,
                 offer_revision = offer_revision + 1, updated_at = ?8
             WHERE id = ?1 AND status = 'available' AND offer_revision = ?9",
                params![
                    seat_id,
                    seat.parallel_limit.map(i64::from),
                    seat.token_limit.and_then(|value| i64::try_from(value).ok()),
                    token_period_json,
                    seat.daily_rate_minor,
                    seat.currency,
                    seat.service_duration_days.map(i64::from),
                    now,
                    input.offer_revision,
                ],
            )
            .map_err(map_db("update Share seat"))?;
        if changed != 1 {
            return Err(AppError::coded_conflict(
                ERROR_OFFER_CHANGED,
                "seat offer changed; reload and retry",
                serde_json::json!({ "seatId": seat_id }),
            ));
        }
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

    pub async fn share_market_propose_price_change(
        &self,
        session: &AuthSession,
        subscription_id: &str,
        input: ProposePriceChangeRequest,
    ) -> Result<String, AppError> {
        if input.daily_rate_minor <= 0
            || input.daily_rate_minor > crate::market_billing::MAX_DAILY_RATE_MINOR
        {
            return Err(AppError::BadRequest(
                "proposed daily price is outside the supported range".into(),
            ));
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Share price change proposal"))?;
        let row = tx
            .query_row(
                "SELECT sub.seat_id, sub.listing_id, sub.owner_user_id, sub.renter_user_id,
                        sub.status, sub.daily_rate_minor, sub.currency, sub.offer_revision,
                        contract.id, contract.status, contract.daily_rate_minor,
                        contract.offer_revision
                 FROM share_market_subscriptions sub
                 LEFT JOIN market_service_contracts contract
                   ON contract.product_kind = 'share'
                 AND contract.product_ref = sub.id
                  AND contract.status != 'terminated'
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                        row.get::<_, Option<i64>>(11)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Share subscription for price change"))?;
        let Some((
            seat_id,
            listing_id,
            owner_user_id,
            _renter_user_id,
            subscription_status,
            current_rate,
            currency,
            offer_revision,
            contract_id,
            contract_status,
            contract_rate,
            contract_revision,
        )) = row
        else {
            return Err(AppError::NotFound("Share subscription not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only the Share owner can propose a price change".into(),
            ));
        }
        if subscription_status != SUB_ACTIVE_POSTPAID {
            return Err(AppError::Conflict(
                "only active paid Share subscriptions can be repriced".into(),
            ));
        }
        if offer_revision != input.offer_revision {
            return Err(AppError::Conflict(
                "Share subscription price changed; reload and retry".into(),
            ));
        }
        let current_rate = current_rate.ok_or_else(|| {
            AppError::Conflict("free Share subscriptions cannot be repriced".into())
        })?;
        let currency = currency
            .filter(|value| value == crate::market_billing::MARKET_CURRENCY)
            .ok_or_else(|| AppError::Internal("paid Share currency is invalid".into()))?;
        if current_rate == input.daily_rate_minor {
            return Err(AppError::BadRequest(
                "proposed daily price must differ from the current price".into(),
            ));
        }
        if contract_id.is_none()
            || !matches!(contract_status.as_deref(), Some("trial" | "active"))
            || contract_rate != Some(current_rate)
            || contract_revision != Some(offer_revision)
        {
            return Err(AppError::Conflict(
                "the active billing contract is not available for repricing".into(),
            ));
        }
        let open_exists = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM share_market_price_changes
                 WHERE subscription_id = ?1 AND status IN ('pending', 'accepted'))",
                params![subscription_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(map_db("check open Share price change"))?
            != 0;
        if open_exists {
            return Err(AppError::Conflict(
                "this Share subscription already has an open price change".into(),
            ));
        }
        let proposal_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO share_market_price_changes (
                id, subscription_id, previous_daily_rate_minor,
                proposed_daily_rate_minor, currency, base_offer_revision,
                applied_offer_revision, status, proposed_by_user_id,
                responded_by_user_id, resolution_reason, created_at, updated_at,
                responded_at, applied_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 'pending', ?7,
                       NULL, NULL, ?8, ?8, NULL, NULL)",
            params![
                proposal_id,
                subscription_id,
                current_rate,
                input.daily_rate_minor,
                currency,
                offer_revision,
                session.user_id,
                now,
            ],
        )
        .map_err(map_db("create Share price change"))?;
        event_tx(
            &tx,
            Some(&listing_id),
            Some(&seat_id),
            Some(subscription_id),
            Some(session),
            "price_change_proposed",
            serde_json::json!({
                "proposalId": proposal_id,
                "previousDailyRateMinor": current_rate,
                "proposedDailyRateMinor": input.daily_rate_minor,
                "currency": crate::market_billing::MARKET_CURRENCY,
                "baseOfferRevision": offer_revision,
            }),
            &now,
        )?;
        tx.commit()
            .map_err(map_db("commit Share price change proposal"))?;
        Ok(proposal_id)
    }

    async fn share_market_resolve_price_change(
        &self,
        session: &AuthSession,
        proposal_id: &str,
        action: PriceChangeAction,
    ) -> Result<(), AppError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Share price change response"))?;
        let row = tx
            .query_row(
                "SELECT change.subscription_id, sub.seat_id, sub.listing_id,
                        sub.owner_user_id, sub.renter_user_id, sub.status,
                        sub.daily_rate_minor, sub.offer_revision,
                        change.previous_daily_rate_minor,
                        change.proposed_daily_rate_minor, change.currency,
                        change.base_offer_revision, change.status,
                        contract.status, contract.daily_rate_minor, contract.offer_revision
                 FROM share_market_price_changes change
                 JOIN share_market_subscriptions sub ON sub.id = change.subscription_id
                 LEFT JOIN market_service_contracts contract
                   ON contract.product_kind = 'share'
                  AND contract.product_ref = sub.id
                  AND contract.status != 'terminated'
                 WHERE change.id = ?1",
                params![proposal_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, String>(12)?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, Option<i64>>(14)?,
                        row.get::<_, Option<i64>>(15)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Share price change response"))?;
        let Some((
            subscription_id,
            seat_id,
            listing_id,
            owner_user_id,
            renter_user_id,
            subscription_status,
            subscription_rate,
            subscription_revision,
            previous_rate,
            proposed_rate,
            currency,
            base_revision,
            current_status,
            contract_status,
            contract_rate,
            contract_revision,
        )) = row
        else {
            return Err(AppError::NotFound("Share price change not found".into()));
        };
        let authorized = match action {
            PriceChangeAction::Accept | PriceChangeAction::Reject => {
                renter_user_id == session.user_id
            }
            PriceChangeAction::Cancel => owner_user_id == session.user_id,
        };
        if !authorized {
            return Err(AppError::Forbidden(match action {
                PriceChangeAction::Accept | PriceChangeAction::Reject => {
                    "only the renter can respond to this Share price change".into()
                }
                PriceChangeAction::Cancel => {
                    "only the Share owner can cancel this price change".into()
                }
            }));
        }
        if current_status == action.target_status() {
            tx.commit()
                .map_err(map_db("commit idempotent Share price change response"))?;
            return Ok(());
        }
        if current_status != PRICE_CHANGE_PENDING {
            return Err(AppError::Conflict(
                "this Share price change is no longer pending".into(),
            ));
        }
        if matches!(action, PriceChangeAction::Accept)
            && (subscription_status != SUB_ACTIVE_POSTPAID
                || subscription_rate != Some(previous_rate)
                || subscription_revision != base_revision
                || !matches!(contract_status.as_deref(), Some("trial" | "active"))
                || contract_rate != Some(previous_rate)
                || contract_revision != Some(base_revision))
        {
            return Err(AppError::Conflict(
                "the active Share price changed or its billing contract is unavailable".into(),
            ));
        }
        tx.execute(
            "UPDATE share_market_price_changes
             SET status = ?2, responded_by_user_id = ?3, resolution_reason = ?4,
                 responded_at = ?5, updated_at = ?5
             WHERE id = ?1 AND status = 'pending'",
            params![
                proposal_id,
                action.target_status(),
                session.user_id,
                action.resolution_reason(),
                now,
            ],
        )
        .map_err(map_db("resolve Share price change"))?;
        event_tx(
            &tx,
            Some(&listing_id),
            Some(&seat_id),
            Some(&subscription_id),
            Some(session),
            action.event_type(),
            serde_json::json!({
                "proposalId": proposal_id,
                "previousDailyRateMinor": previous_rate,
                "proposedDailyRateMinor": proposed_rate,
                "currency": currency,
                "baseOfferRevision": base_revision,
            }),
            &now,
        )?;
        tx.commit()
            .map_err(map_db("commit Share price change response"))?;
        Ok(())
    }

    pub async fn share_market_accept_price_change(
        &self,
        session: &AuthSession,
        proposal_id: &str,
    ) -> Result<(), AppError> {
        self.share_market_resolve_price_change(session, proposal_id, PriceChangeAction::Accept)
            .await
    }

    pub async fn share_market_reject_price_change(
        &self,
        session: &AuthSession,
        proposal_id: &str,
    ) -> Result<(), AppError> {
        self.share_market_resolve_price_change(session, proposal_id, PriceChangeAction::Reject)
            .await
    }

    pub async fn share_market_cancel_price_change(
        &self,
        session: &AuthSession,
        proposal_id: &str,
    ) -> Result<(), AppError> {
        self.share_market_resolve_price_change(session, proposal_id, PriceChangeAction::Cancel)
            .await
    }

    pub async fn share_market_delete_seat(
        &self,
        session: &AuthSession,
        seat_id: &str,
    ) -> Result<(), AppError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin delete Share seat"))?;
        let row: Option<(
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
        )> = tx
            .query_row(
                "SELECT listing.owner_user_id, seat.status, seat.listing_id, seat.retired_at,
                        seat.current_subscription_id, seat.retired_subscription_id,
                        (SELECT COUNT(*) FROM share_market_subscriptions sub WHERE sub.seat_id = seat.id)
                 FROM share_market_seats seat
                 JOIN share_market_listings listing ON listing.id = seat.listing_id
                 WHERE seat.id = ?1",
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
            .map_err(map_db("read Share seat for delete"))?;
        let Some((
            owner_user_id,
            status,
            listing_id,
            retired_at,
            current_subscription_id,
            retired_subscription_id,
            subscription_count,
        )) = row
        else {
            return Err(AppError::NotFound("seat not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only listing owner can delete seat".into(),
            ));
        }
        if status == SEAT_DELETED {
            return Ok(());
        }
        let subscription_id = current_subscription_id
            .as_ref()
            .or(retired_subscription_id.as_ref());
        let subscription = subscription_id
            .map(|subscription_id| subscription_record(&tx, subscription_id))
            .transpose()?
            .flatten();
        let capability = seat_delete_capability(
            true,
            &status,
            retired_at.as_deref(),
            subscription_count,
            subscription.as_ref(),
        );
        if !capability.can_delete {
            return Err(AppError::Conflict(format!(
                "Share seat cannot be deleted: {}",
                capability.blocked_reason.unwrap_or("delete_blocked")
            )));
        }
        let cleanup_reason = if subscription
            .as_ref()
            .is_some_and(|subscription| subscription.status == SUB_GRANT_FAILED)
        {
            "grant_failed_cleanup"
        } else {
            "unused_seat_cleanup"
        };
        tx.execute(
            "UPDATE share_market_seats
             SET status = ?2,
                 retired_subscription_id = COALESCE(retired_subscription_id, current_subscription_id),
                 retired_at = CASE
                     WHEN ?4 > 0 THEN COALESCE(retired_at, ?3)
                     ELSE retired_at
                 END,
                 current_subscription_id = NULL, updated_at = ?3
             WHERE id = ?1",
            params![seat_id, SEAT_DELETED, now, subscription_count],
        )
        .map_err(map_db("delete Share seat"))?;
        event_tx(
            &tx,
            Some(&listing_id),
            Some(seat_id),
            None,
            Some(session),
            "seat_deleted",
            serde_json::json!({ "reason": cleanup_reason }),
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
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin close Share listing"))?;
        let listing: Option<(String, String)> = tx
            .query_row(
                "SELECT owner_user_id, status FROM share_market_listings
                 WHERE id = ?1 AND deleted_at IS NULL",
                params![listing_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_db("read Share listing"))?;
        let Some((owner_user_id, listing_status)) = listing else {
            return Err(AppError::NotFound("listing not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only listing owner can close listing".into(),
            ));
        }
        if listing_status == "closed" {
            return Ok(());
        }
        let disabled_seat_count = tx
            .execute(
                "UPDATE share_market_seats
             SET status = 'disabled', offer_revision = offer_revision + 1, updated_at = ?2
             WHERE listing_id = ?1 AND status = 'available'",
                params![listing_id, now],
            )
            .map_err(map_db("disable open Share seats"))?;
        let expired_quote_count = expire_listing_rent_quotes_tx(&tx, listing_id, &now)?;
        let changed = tx
            .execute(
                "UPDATE share_market_listings SET status = 'closed', updated_at = ?2
                 WHERE id = ?1 AND status = 'active' AND deleted_at IS NULL",
                params![listing_id, now],
            )
            .map_err(map_db("close Share listing"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "listing state changed while stopping it".into(),
            ));
        }
        event_tx(
            &tx,
            Some(listing_id),
            None,
            None,
            Some(session),
            "listing_closed",
            serde_json::json!({
                "disabledSeatCount": disabled_seat_count,
                "expiredQuoteCount": expired_quote_count,
            }),
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
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin delete Share listing"))?;
        let row: Option<(String, String, Option<String>)> = tx
            .query_row(
                "SELECT owner_user_id, status, deleted_at FROM share_market_listings
                 WHERE id = ?1",
                params![listing_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(map_db("read Share listing for delete"))?;
        let Some((owner_user_id, status, deleted_at)) = row else {
            return Err(AppError::NotFound("listing not found".into()));
        };
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only listing owner can delete listing".into(),
            ));
        }
        if deleted_at.is_some() {
            return Ok(());
        }
        let capability = listing_delete_capability_tx(&tx, listing_id, &status, true)?;
        if !capability.can_delete {
            return Err(AppError::Conflict(format!(
                "Share listing cannot be deleted: {}",
                capability.blocked_reason.unwrap_or("delete_blocked")
            )));
        }
        tx.execute(
            "UPDATE share_market_seats
             SET status = 'deleted',
                 retired_subscription_id = COALESCE(retired_subscription_id, current_subscription_id),
                 retired_at = CASE
                     WHEN current_subscription_id IS NOT NULL THEN COALESCE(retired_at, ?2)
                     ELSE retired_at
                 END,
                 current_subscription_id = NULL, updated_at = ?2
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

async fn reopen_listing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(listing_id): Path<String>,
    Json(input): Json<ReopenListingRequest>,
) -> Result<Json<ReopenListingResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .share_market_reopen_listing(&session, &listing_id, input)
            .await?,
    ))
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
    tx: &Connection,
    share_id: &str,
    subscription_id: &str,
    entitlement_id: &str,
    action: &str,
    email: &str,
    policy: Option<&ShareUserPolicy>,
    now: &str,
) -> Result<String, AppError> {
    if action == "revoke" && has_terminal_revoke_operation_tx(tx, subscription_id)? {
        return Err(AppError::Conflict(
            "Share revoke is dead-lettered and requires operator intervention".into(),
        ));
    }
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
    if action == "revoke" {
        let retry_id = tx
            .query_row(
                "SELECT id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'revoke' AND status = 'rejected'
                   AND dead_lettered_at IS NULL AND attempts < ?2
                 ORDER BY share_sequence DESC LIMIT 1",
                params![subscription_id, MAX_CONTROL_ATTEMPTS],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("read retryable rejected Share revoke"))?;
        if let Some(retry_id) = retry_id {
            tx.execute(
                "UPDATE share_control_operations
                 SET status = 'pending', edit_id = NULL, next_attempt_at = ?2,
                     last_error = NULL, updated_at = ?2
                 WHERE id = ?1 AND status = 'rejected' AND dead_lettered_at IS NULL",
                params![retry_id, now],
            )
            .map_err(map_db("requeue rejected Share revoke"))?;
            return Ok(retry_id);
        }
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
            created_at, updated_at, applied_at, next_attempt_at, dead_lettered_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', NULL, 0, NULL,
                   ?9, ?9, NULL, ?9, NULL)",
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

fn has_terminal_revoke_operation_tx(
    conn: &Connection,
    subscription_id: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM share_control_operations
             WHERE subscription_id = ?1 AND action = 'revoke' AND status = 'rejected'
               AND (dead_lettered_at IS NOT NULL OR attempts >= ?2)
         )",
        params![subscription_id, MAX_CONTROL_ATTEMPTS],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .map_err(map_db("check terminal Share revoke"))
}

impl AppStore {
    pub async fn share_market_control_dead_letters(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ShareControlDeadLetterPage, AppError> {
        let cursor = decode_descending_cursor(cursor)?;
        let conn = self.conn.lock().await;
        let summary = conn
            .query_row(
                "SELECT
                    SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'dispatched' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN dead_lettered_at IS NOT NULL THEN 1 ELSE 0 END)
                 FROM share_control_operations",
                [],
                |row| {
                    Ok(ShareControlOperationSummary {
                        pending: row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                        dispatched: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                        dead_lettered: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    })
                },
            )
            .map_err(map_db("read Share control operation summary"))?;
        let cursor_timestamp = cursor.as_ref().map(|value| value.timestamp.as_str());
        let cursor_id = cursor.as_ref().map(|value| value.id.as_str());
        let mut operations = conn
            .prepare(
                "SELECT operation.id, operation.share_id, operation.subscription_id,
                        operation.action, operation.email, operation.attempts,
                        operation.last_error, edit.error_code, operation.created_at,
                        operation.updated_at, operation.dead_lettered_at
                 FROM share_control_operations operation
                 LEFT JOIN share_edit_requests edit ON edit.id = operation.edit_id
                 WHERE operation.dead_lettered_at IS NOT NULL
                   AND (?1 IS NULL OR operation.dead_lettered_at < ?1
                        OR (operation.dead_lettered_at = ?1 AND operation.id < ?2))
                 ORDER BY operation.dead_lettered_at DESC, operation.id DESC LIMIT ?3",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(
                        params![
                            cursor_timestamp,
                            cursor_id,
                            i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX),
                        ],
                        |row| {
                            let action = row.get::<_, String>(3)?;
                            Ok(ShareControlDeadLetterView {
                                id: row.get(0)?,
                                share_id: row.get(1)?,
                                subscription_id: row.get(2)?,
                                can_requeue: action == "revoke",
                                action,
                                email: row.get(4)?,
                                attempts: row.get(5)?,
                                last_error: row.get(6)?,
                                error_code: row.get(7)?,
                                created_at: row.get(8)?,
                                updated_at: row.get(9)?,
                                dead_lettered_at: row.get(10)?,
                            })
                        },
                    )?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read Share control dead letters"))?;
        let has_more = operations.len() > limit;
        operations.truncate(limit);
        let next_cursor = if has_more {
            operations
                .last()
                .map(|operation| {
                    encode_descending_cursor(&DescendingCursor {
                        timestamp: operation.dead_lettered_at.clone(),
                        id: operation.id.clone(),
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(ShareControlDeadLetterPage {
            operations,
            summary,
            next_cursor,
        })
    }

    pub async fn share_market_requeue_control_dead_letter(
        &self,
        session: &AuthSession,
        operation_id: &str,
    ) -> Result<(), AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Share control dead-letter requeue"))?;
        let operation = tx
            .query_row(
                "SELECT operation.share_id, operation.subscription_id, operation.action,
                        operation.status, operation.dead_lettered_at,
                        subscription.listing_id, subscription.seat_id,
                        subscription.status, operation.edit_id
                 FROM share_control_operations operation
                 JOIN share_market_subscriptions subscription
                   ON subscription.id = operation.subscription_id
                 WHERE operation.id = ?1",
                params![operation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Share control dead letter for requeue"))?
            .ok_or_else(|| AppError::NotFound("Share control operation not found".into()))?;
        if operation.2 != "revoke" || operation.3 != "rejected" || operation.4.is_none() {
            return Err(AppError::Conflict(
                "only a dead-lettered Share revoke can be requeued".into(),
            ));
        }
        let changed = tx
            .execute(
                "UPDATE share_control_operations
                 SET status = 'pending', edit_id = NULL, attempts = 0,
                     last_error = NULL, next_attempt_at = ?2,
                     dead_lettered_at = NULL, applied_at = NULL, updated_at = ?2
                 WHERE id = ?1 AND action = 'revoke' AND status = 'rejected'
                   AND dead_lettered_at IS NOT NULL",
                params![operation_id, now],
            )
            .map_err(map_db("requeue Share control dead letter"))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "Share control operation was already requeued".into(),
            ));
        }
        tx.execute(
            "UPDATE share_market_subscriptions
             SET status = CASE
                    WHEN status = 'revoke_failed' THEN 'revoke_pending'
                    WHEN status = 'billing_control_failed' THEN 'billing_suspend_pending'
                    ELSE status END,
                 release_reason = CASE
                    WHEN status IN ('revoke_failed', 'billing_control_failed') THEN NULL
                    ELSE release_reason END,
                 updated_at = ?2 WHERE id = ?1",
            params![operation.1, now],
        )
        .map_err(map_db("restore Share subscription control state"))?;
        event_tx(
            &tx,
            Some(&operation.5),
            Some(&operation.6),
            Some(&operation.1),
            Some(session),
            "control_dead_letter_requeued",
            serde_json::json!({
                "operationId": operation_id,
                "previousEditId": operation.8,
                "previousSubscriptionStatus": operation.7,
            }),
            &now,
        )?;
        crate::store::enqueue_operator_alert_signal_tx(
            &tx,
            &format!("share-control-dead-letter-requeued:{operation_id}"),
            &format!("share_control_dead_letter:share:{}", operation.0),
            "resolved",
            "share_control_dead_letter",
            "share",
            Some(&operation.0),
            "critical",
            "Share control dead letter requeued",
            "An administrator safely requeued the failed Share revoke operation.",
            serde_json::json!({
                "operationId": operation_id,
                "subscriptionId": operation.1,
                "shareId": operation.0,
            }),
            now_dt,
        )?;
        tx.commit()
            .map_err(map_db("commit Share control dead-letter requeue"))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct RentCommitMetadata {
    quote_id: String,
    idempotency_key: String,
    request_fingerprint: String,
}

impl AppStore {
    pub async fn share_market_create_rent_quote(
        &self,
        session: &AuthSession,
        seat_id: &str,
        _required_app: Option<&str>,
    ) -> Result<RentQuoteView, AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let expires_at = (now_dt + Duration::seconds(RENT_QUOTE_TTL_SECS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin Share rent quote"))?;
        tx.execute(
            "UPDATE share_market_rent_quotes
             SET status = 'expired', updated_at = ?1
             WHERE status = 'active' AND expires_at <= ?1",
            params![now],
        )
        .map_err(map_db("expire Share rent quotes"))?;
        let retention_cutoff = (now_dt - Duration::days(RENT_QUOTE_RETENTION_DAYS)).to_rfc3339();
        tx.execute(
            "DELETE FROM share_market_rent_quotes
             WHERE status IN ('expired', 'consumed') AND updated_at < ?1",
            params![retention_cutoff],
        )
        .map_err(map_db("prune retained Share rent quotes"))?;
        #[allow(clippy::type_complexity)]
        let row: Option<(
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            Option<i64>,
            Option<i64>,
            String,
            Option<i64>,
            Option<String>,
            Option<i64>,
            String,
            String,
            i64,
            i64,
            i64,
            Option<String>,
            Option<String>,
            String,
            i64,
            i64,
            i64,
        )> = tx
            .query_row(
                "SELECT seat.listing_id, listing.share_id, COALESCE(share.share_name, listing.share_id),
                        listing.owner_user_id, listing.owner_email, listing.status,
                        seat.position, seat.offer_revision, seat.parallel_limit, seat.token_limit,
                        seat.token_period_json, seat.daily_rate_minor, seat.currency,
                        seat.service_duration_days, COALESCE(share.user_grants_json, '{}'),
                        COALESCE(share.bindings_json, '{}'),
                        COALESCE(share.enabled_claude, 0), COALESCE(share.enabled_codex, 0),
                        COALESCE(share.enabled_gemini, 0), share.app_runtimes_json,
                        share.app_providers_json, share.app_type, share.parallel_limit,
                        share.token_limit, share.tokens_used
                 FROM share_market_seats seat
                 JOIN share_market_listings listing ON listing.id = seat.listing_id
                 JOIN shares share ON share.share_id = listing.share_id
                 WHERE seat.id = ?1 AND seat.status = 'available' AND seat.retired_at IS NULL
                   AND share.share_status = 'active'
                   AND lower(share.owner_email) = lower(listing.owner_email)",
                params![seat_id],
                |row| {
                    Ok((
                        row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                        row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                        row.get(10)?, row.get(11)?, row.get(12)?, row.get(13)?, row.get(14)?,
                        row.get(15)?, row.get(16)?, row.get(17)?, row.get(18)?, row.get(19)?,
                        row.get(20)?, row.get(21)?, row.get(22)?, row.get(23)?, row.get(24)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Share seat quote"))?;
        let Some((
            listing_id,
            share_id,
            share_name,
            owner_user_id,
            owner_email,
            listing_status,
            seat_position,
            offer_revision,
            parallel_limit,
            token_limit,
            token_period_json,
            daily_rate_minor,
            currency,
            service_duration_days,
            grants_json,
            bindings_json,
            enabled_claude,
            enabled_codex,
            enabled_gemini,
            app_runtimes_json,
            app_providers_json,
            fallback_app,
            share_parallel_limit,
            share_token_limit,
            share_tokens_used,
        )) = row
        else {
            return Err(AppError::coded_conflict(
                "share_market_seat_unavailable",
                "seat is no longer available",
                serde_json::json!({ "reason": "seat_unavailable" }),
            ));
        };
        if listing_status != "active" {
            return Err(AppError::Conflict("listing is no longer active".into()));
        }
        ensure_share_market_app_scope_supported_tx(&tx, &share_id)?;
        ensure_share_total_tokens_available(share_token_limit, share_tokens_used)?;
        if owner_user_id == session.user_id || owner_email.eq_ignore_ascii_case(&session.email) {
            return Err(AppError::BadRequest(
                "Share owner cannot rent their own seat".into(),
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
            .map_err(map_db("check quoted Share rental"))?
            .is_some();
        if already_renting {
            return Err(AppError::coded_conflict(
                "share_market_already_renting",
                "one account can rent only one seat on the same Share",
                serde_json::json!({ "reason": "already_renting" }),
            ));
        }
        let grants: BTreeMap<String, ShareUserGrant> =
            serde_json::from_str(&grants_json).unwrap_or_default();
        if grants
            .get(&session.email.to_ascii_lowercase())
            .is_some_and(|grant| grant.active)
        {
            return Err(AppError::coded_conflict(
                "share_market_direct_access",
                "this account already has direct Share access",
                serde_json::json!({ "reason": "direct_access" }),
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
        let trial_seconds_remaining = if daily_rate_minor.is_some() {
            let quote_currency = currency
                .as_deref()
                .ok_or_else(|| AppError::Internal("paid Share currency is missing".into()))?;
            crate::market_billing::ensure_credit_allowed_tx(
                &tx,
                &session.user_id,
                &session.email,
                &owner_user_id,
                crate::market_access::PRODUCT_SHARE,
                quote_currency,
            )?;
            crate::market_billing::trial_seconds_remaining_tx(
                &tx,
                &session.user_id,
                &owner_user_id,
                crate::market_access::PRODUCT_SHARE,
                &share_id,
                quote_currency,
            )?
        } else {
            0
        };
        let service = build_rent_service_snapshot(
            &bindings_json,
            app_runtimes_json.as_deref(),
            app_providers_json.as_deref(),
            &fallback_app,
            &ShareSupport {
                claude: enabled_claude != 0,
                codex: enabled_codex != 0,
                gemini: enabled_gemini != 0,
            },
            share_parallel_limit,
            share_token_limit,
            share_tokens_used,
        )?;
        for app in &service.supported_apps {
            let availability = evaluate_share_market_service_availability_tx(
                &tx,
                &share_id,
                app,
                None,
                None,
                Some(&service),
                now_dt,
            )?;
            if !availability.available() {
                return Err(availability_error(app, &availability));
            }
            ensure_fixed_service_term_fulfillable_tx(
                &tx,
                &share_id,
                app,
                service_duration_days.and_then(|value| u32::try_from(value).ok()),
                now_dt,
            )?;
        }
        if let (Some(seat_limit), Some(share_limit)) = (
            parallel_limit.and_then(|value| u32::try_from(value).ok()),
            service.share_parallel_limit,
        ) && seat_limit > share_limit
        {
            return Err(AppError::coded_conflict(
                "share_market_seat_capacity_invalid",
                "seat concurrency exceeds the Share's total concurrency",
                serde_json::json!({
                    "reason": "seat_capacity_invalid",
                    "seatParallelLimit": seat_limit,
                    "shareParallelLimit": share_limit,
                }),
            ));
        }
        let snapshot = RentQuoteSnapshot {
            seat_id: seat_id.to_string(),
            listing_id: listing_id.clone(),
            share_id: share_id.clone(),
            share_name,
            owner_email,
            seat_position,
            parallel_limit: parallel_limit.and_then(|value| u32::try_from(value).ok()),
            token_limit: token_limit.and_then(|value| u64::try_from(value).ok()),
            token_period: serde_json::from_str(&token_period_json)
                .unwrap_or(ShareTokenPeriod::Lifetime),
            daily_rate_minor,
            currency,
            service_duration_days: service_duration_days
                .and_then(|value| u32::try_from(value).ok()),
            offer_revision,
            service: service.clone(),
        };
        let quote_id = Uuid::new_v4().to_string();
        let snapshot_json = serde_json::to_string(&snapshot).map_err(|error| {
            AppError::Internal(format!("encode Share rent quote failed: {error}"))
        })?;
        let contract_apps_json =
            serde_json::to_string(&service.supported_apps).map_err(|error| {
                AppError::Internal(format!("encode Share contract Apps failed: {error}"))
            })?;
        tx.execute(
            "UPDATE share_market_rent_quotes
             SET status = 'expired', updated_at = ?3
             WHERE renter_user_id = ?1 AND seat_id = ?2 AND status = 'active'",
            params![session.user_id, seat_id, now],
        )
        .map_err(map_db("replace Share rent quote"))?;
        tx.execute(
            "INSERT INTO share_market_rent_quotes (
                id, seat_id, listing_id, share_id, renter_user_id, renter_email,
                offer_revision, snapshot_json, trial_seconds_remaining, status,
                required_app, contract_apps_json,
                expires_at, consumed_subscription_id, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active', ?10, ?11, ?12, NULL, ?13, ?13)",
            params![
                quote_id,
                seat_id,
                listing_id,
                share_id,
                session.user_id,
                session.email,
                offer_revision,
                snapshot_json,
                trial_seconds_remaining,
                service.required_app,
                contract_apps_json,
                expires_at,
                now,
            ],
        )
        .map_err(map_db("insert Share rent quote"))?;
        tx.commit().map_err(map_db("commit Share rent quote"))?;
        let mut public_snapshot = snapshot;
        public_snapshot.service.provider_binding_fingerprint = None;
        for app in &mut public_snapshot.service.apps {
            app.provider_binding_fingerprint = None;
        }
        Ok(RentQuoteView {
            id: quote_id,
            status: "active".into(),
            expires_at,
            trial_seconds_remaining,
            offer: public_snapshot,
        })
    }

    pub async fn share_market_commit_rent_quote(
        &self,
        session: &AuthSession,
        seat_id: &str,
        quote_id: &str,
        idempotency_key: &str,
    ) -> Result<CommitRentQuoteResponse, AppError> {
        self.share_market_commit_rent_quote_with_online_state(
            session,
            seat_id,
            quote_id,
            idempotency_key,
            true,
        )
        .await
    }

    async fn share_market_commit_rent_quote_with_online_state(
        &self,
        session: &AuthSession,
        seat_id: &str,
        quote_id: &str,
        idempotency_key: &str,
        share_online: bool,
    ) -> Result<CommitRentQuoteResponse, AppError> {
        let quote_id = quote_id.trim();
        let idempotency_key = idempotency_key.trim();
        if quote_id.is_empty() {
            return Err(AppError::BadRequest("quoteId is required".into()));
        }
        if idempotency_key.is_empty()
            || idempotency_key.len() > 128
            || idempotency_key.chars().any(char::is_control)
        {
            return Err(AppError::BadRequest(
                "idempotencyKey must contain 1-128 non-control characters".into(),
            ));
        }
        let fingerprint = format!("share-rent:{quote_id}:{seat_id}");
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin commit Share rent quote"))?;
        let existing = tx
            .query_row(
                "SELECT id, rent_quote_id, request_fingerprint
                 FROM share_market_subscriptions
                 WHERE renter_user_id = ?1 AND idempotency_key = ?2",
                params![session.user_id, idempotency_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read idempotent Share rental"))?;
        if let Some((subscription_id, stored_quote_id, stored_fingerprint)) = existing {
            if stored_quote_id.as_deref() != Some(quote_id)
                || stored_fingerprint.as_deref() != Some(fingerprint.as_str())
            {
                return Err(AppError::Conflict(
                    "idempotency key was already used for another Share rental".into(),
                ));
            }
            tx.commit()
                .map_err(map_db("commit idempotent Share rental"))?;
            return Ok(CommitRentQuoteResponse {
                ok: true,
                subscription_id,
                replayed: true,
            });
        }
        if !share_online {
            return Err(share_market_offline_error());
        }
        let (
            quoted_seat_id,
            offer_revision,
            status,
            expires_at,
            quote_user_id,
            snapshot_json,
            trial_seconds_remaining,
            required_app,
            contract_apps_json,
        ) = tx
            .query_row(
                "SELECT seat_id, offer_revision, status, expires_at, renter_user_id,
                        snapshot_json, trial_seconds_remaining, required_app,
                        contract_apps_json
                 FROM share_market_rent_quotes WHERE id = ?1",
                params![quote_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Share rent quote"))?
            .ok_or_else(|| AppError::NotFound("Share rent quote not found".into()))?;
        if quote_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "Share rent quote belongs to another account".into(),
            ));
        }
        if quoted_seat_id != seat_id {
            return Err(AppError::Conflict(
                "Share rent quote belongs to another seat".into(),
            ));
        }
        if status != "active" || parse_time(&expires_at)? <= now_dt {
            return Err(AppError::Gone(
                "Share rent quote expired; request a new quote".into(),
            ));
        }
        let snapshot: RentQuoteSnapshot = serde_json::from_str(&snapshot_json)
            .map_err(|_| AppError::Internal("stored Share rent quote is invalid".into()))?;
        if snapshot.service.schema_version < 2 {
            return Err(AppError::Gone(
                "Share rent quote uses retired single-App terms; request a new quote".into(),
            ));
        }
        if required_app.as_deref() != Some(snapshot.service.required_app.as_str()) {
            return Err(AppError::Conflict(
                "Share rent quote compatibility App is inconsistent".into(),
            ));
        }
        let stored_contract_apps = serde_json::from_str::<Vec<String>>(&contract_apps_json)
            .map(canonical_market_apps)
            .map_err(|_| AppError::Internal("stored Share quote App bundle is invalid".into()))?;
        if stored_contract_apps != snapshot.service.supported_apps {
            return Err(AppError::Conflict(
                "Share rent quote App bundle is inconsistent".into(),
            ));
        }
        let metadata = RentCommitMetadata {
            quote_id: quote_id.to_string(),
            idempotency_key: idempotency_key.to_string(),
            request_fingerprint: fingerprint,
        };
        let subscription_id = self.share_market_rent_seat_tx(
            &tx,
            session,
            seat_id,
            RentSeatRequest { offer_revision },
            Some(&metadata),
            Some(&snapshot),
            Some(trial_seconds_remaining),
            &now,
        )?;
        consume_share_rent_quote_tx(&tx, quote_id, &subscription_id, &session.user_id, &now)?;
        tx.commit().map_err(map_db("commit Share rent quote"))?;
        Ok(CommitRentQuoteResponse {
            ok: true,
            subscription_id,
            replayed: false,
        })
    }
}

fn consume_share_rent_quote_tx(
    conn: &Connection,
    quote_id: &str,
    subscription_id: &str,
    renter_user_id: &str,
    now: &str,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE share_market_rent_quotes
             SET status = 'consumed', consumed_subscription_id = ?2, updated_at = ?4
             WHERE id = ?1 AND renter_user_id = ?3
               AND (status = 'active' OR consumed_subscription_id = ?2)",
            params![quote_id, subscription_id, renter_user_id, now],
        )
        .map_err(map_db("consume Share rent quote"))?;
    if changed == 0 {
        return Err(AppError::Conflict(
            "Share rent quote was consumed by another rental".into(),
        ));
    }
    Ok(())
}

impl AppStore {
    pub async fn share_market_rent_seat(
        &self,
        session: &AuthSession,
        seat_id: &str,
        input: RentSeatRequest,
    ) -> Result<String, AppError> {
        self.share_market_rent_seat_with_metadata(session, seat_id, input, None)
            .await
    }

    async fn share_market_rent_seat_with_metadata(
        &self,
        session: &AuthSession,
        seat_id: &str,
        input: RentSeatRequest,
        metadata: Option<&RentCommitMetadata>,
    ) -> Result<String, AppError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin rent Share seat"))?;
        let subscription_id = self
            .share_market_rent_seat_tx(&tx, session, seat_id, input, metadata, None, None, &now)?;
        tx.commit().map_err(map_db("commit Share seat rental"))?;
        Ok(subscription_id)
    }

    pub async fn share_market_retry_grant(
        &self,
        session: &AuthSession,
        subscription_id: &str,
    ) -> Result<(), AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Share grant retry"))?;
        let record = subscription_record(&tx, subscription_id)?
            .ok_or_else(|| AppError::NotFound("Share subscription not found".into()))?;
        if record.owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only the Share owner can retry this grant".into(),
            ));
        }
        if record.status == SUB_GRANT_PENDING {
            let already_requeued = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM share_control_operations
                        WHERE subscription_id = ?1 AND action = 'upsert'
                          AND status IN ('pending', 'dispatched')
                    ) AND EXISTS(
                        SELECT 1 FROM share_market_seats
                        WHERE id = ?2 AND current_subscription_id = ?1
                          AND status = 'reserved'
                    )",
                    params![subscription_id, record.seat_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_db("check idempotent Share grant retry"))?
                != 0;
            if already_requeued {
                tx.commit()
                    .map_err(map_db("commit idempotent Share grant retry"))?;
                return Ok(());
            }
        }
        if record.status != SUB_GRANT_FAILED {
            return Err(AppError::Conflict(
                "only a failed initial Share grant can be retried".into(),
            ));
        }
        if record.failure_code.as_deref() == Some(SHARE_GRANT_CONTRACT_VIOLATION_CODE) {
            return Err(AppError::Conflict(
                "a contract-violating Share grant cannot be retried".into(),
            ));
        }
        if record.has_active_control_work || record.has_active_billing_contract {
            return Err(AppError::Conflict(
                "this Share subscription still has active control or billing work".into(),
            ));
        }
        let violations = subscription_integrity_violations_tx(&tx, &record, now_dt)?;
        if !violations.is_empty() {
            return Err(AppError::coded_conflict(
                "share_market_contract_incompatible",
                "the frozen Share rental contract is no longer compatible",
                serde_json::json!({ "reasons": violations }),
            ));
        }
        let (seat_status, current_subscription_id, retired_subscription_id) = tx
            .query_row(
                "SELECT status, current_subscription_id, retired_subscription_id
                 FROM share_market_seats WHERE id = ?1",
                params![record.seat_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Share seat for grant retry"))?
            .ok_or_else(|| AppError::Conflict("the original Share seat no longer exists".into()))?;
        if seat_status == SEAT_DELETED
            || current_subscription_id.is_some()
            || retired_subscription_id.as_deref() != Some(subscription_id)
        {
            return Err(AppError::Conflict(
                "the original Share seat is no longer available for recovery".into(),
            ));
        }
        let subscription_conflict = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM share_market_subscriptions
                    WHERE id != ?1 AND status NOT IN ('released', 'grant_failed')
                      AND (seat_id = ?2 OR (renter_user_id = ?3 AND share_id = ?4))
                )",
                params![
                    subscription_id,
                    record.seat_id,
                    record.renter_user_id,
                    record.share_id,
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(map_db("check Share grant retry conflicts"))?
            != 0;
        if subscription_conflict {
            return Err(AppError::Conflict(
                "another active rental now conflicts with this Share grant".into(),
            ));
        }
        let grants_json = tx
            .query_row(
                "SELECT COALESCE(user_grants_json, '{}') FROM shares WHERE share_id = ?1",
                params![record.share_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("read direct Share access for grant retry"))?
            .ok_or_else(|| AppError::Conflict("the Share no longer exists".into()))?;
        let grants = serde_json::from_str::<BTreeMap<String, ShareUserGrant>>(&grants_json)
            .unwrap_or_default();
        if grants
            .get(&record.renter_email.to_ascii_lowercase())
            .is_some_and(|grant| {
                grant.active && grant.entitlement_id.as_deref() != Some(&record.entitlement_id)
            })
        {
            return Err(AppError::Conflict(
                "the renter now has another direct Share entitlement".into(),
            ));
        }
        let (operation_id, old_edit_id) = tx
            .query_row(
                "SELECT id, edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'
                 ORDER BY share_sequence DESC LIMIT 1",
                params![subscription_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(map_db("read failed Share grant operation"))?
            .ok_or_else(|| AppError::Conflict("the failed Share grant is missing".into()))?;
        if let Some(daily_rate_minor) = record.daily_rate_minor {
            let currency = record.currency.as_deref().ok_or_else(|| {
                AppError::Internal("paid Share subscription currency is missing".into())
            })?;
            let replacement_contract_id = tx
                .query_row(
                    "SELECT id FROM market_service_contracts
                     WHERE product_kind = 'share' AND product_ref = ?1 AND status = 'terminated'
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    params![subscription_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_db("read failed Share billing reservation"))?
                .ok_or_else(|| {
                    AppError::Conflict("the failed Share billing reservation is missing".into())
                })?;
            crate::market_billing::reserve_contract_tx(
                &tx,
                crate::market_billing::ActivateContractInput {
                    product_kind: "share",
                    product_ref: subscription_id,
                    service_ref: &record.share_id,
                    service_label: &record.share_name,
                    buyer_user_id: &record.renter_user_id,
                    buyer_email: &record.renter_email,
                    supplier_user_id: &record.owner_user_id,
                    supplier_email: &record.owner_email,
                    currency,
                    daily_rate_minor,
                    offer_revision: record.offer_revision,
                    replacement_of: Some(&replacement_contract_id),
                },
                i64::MAX,
                &now,
            )?;
        }
        let policy = contract_policy(&record)?;
        let policy_json = serde_json::to_string(&policy).map_err(|error| {
            AppError::Internal(format!("encode retried Share grant policy failed: {error}"))
        })?;
        let operation_changed = tx
            .execute(
                "UPDATE share_control_operations
                 SET status = 'pending', edit_id = NULL, attempts = 0,
                     last_error = NULL, policy_json = ?2, applied_at = NULL,
                     next_attempt_at = ?3, dead_lettered_at = NULL, updated_at = ?3
                 WHERE id = ?1 AND action = 'upsert' AND status = 'rejected'",
                params![operation_id, policy_json, now],
            )
            .map_err(map_db("requeue failed Share grant"))?;
        if operation_changed != 1 {
            return Err(AppError::Conflict(
                "the failed Share grant operation is no longer retryable".into(),
            ));
        }
        tx.execute(
            "UPDATE share_market_subscriptions
             SET status = 'grant_pending', release_reason = NULL,
                 released_at = NULL, integrity_state = 'compatible',
                 integrity_reason = NULL, integrity_checked_at = ?2,
                 integrity_violated_at = NULL, updated_at = ?2
             WHERE id = ?1 AND status = 'grant_failed'",
            params![subscription_id, now],
        )
        .map_err(map_db("restore failed Share subscription"))?;
        let seat_changed = tx
            .execute(
                "UPDATE share_market_seats
                 SET status = 'reserved', current_subscription_id = ?2,
                     retired_subscription_id = NULL, retired_at = NULL, updated_at = ?3
                 WHERE id = ?1 AND status != 'deleted' AND current_subscription_id IS NULL
                   AND retired_subscription_id = ?2",
                params![record.seat_id, subscription_id, now],
            )
            .map_err(map_db("restore failed Share seat"))?;
        if seat_changed != 1 {
            return Err(AppError::Conflict(
                "the original Share seat changed during grant recovery".into(),
            ));
        }
        event_tx(
            &tx,
            Some(&record.listing_id),
            Some(&record.seat_id),
            Some(subscription_id),
            Some(session),
            "entitlement_grant_retried",
            serde_json::json!({
                "operationId": operation_id,
                "previousEditId": old_edit_id,
            }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit Share grant retry"))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn share_market_rent_seat_tx(
        &self,
        tx: &Transaction<'_>,
        session: &AuthSession,
        seat_id: &str,
        input: RentSeatRequest,
        metadata: Option<&RentCommitMetadata>,
        quote_snapshot: Option<&RentQuoteSnapshot>,
        quoted_trial_seconds: Option<i64>,
        now: &str,
    ) -> Result<String, AppError> {
        #[allow(clippy::type_complexity)]
        let row: Option<(
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
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
            String,
            i64,
            i64,
            i64,
            Option<String>,
            Option<String>,
            String,
            i64,
            i64,
            i64,
        )> = tx
            .query_row(
                "SELECT seat.listing_id, listing.share_id, listing.owner_user_id,
                        listing.owner_email, listing.status, seat.status, seat.position,
                        seat.offer_revision,
                        seat.parallel_limit, seat.token_limit, seat.token_period_json,
                        seat.daily_rate_minor, seat.currency, seat.service_duration_days,
                        COALESCE(s.user_grants_json, '{}'),
                        COALESCE(s.share_name, listing.share_id), listing.installation_id,
                        COALESCE(s.bindings_json, '{}'),
                        COALESCE(s.enabled_claude, 0), COALESCE(s.enabled_codex, 0),
                        COALESCE(s.enabled_gemini, 0), s.app_runtimes_json,
                        s.app_providers_json, s.app_type, s.parallel_limit,
                        s.token_limit, s.tokens_used
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
                        row.get(16)?,
                        row.get(17)?,
                        row.get(18)?,
                        row.get(19)?,
                        row.get(20)?,
                        row.get(21)?,
                        row.get(22)?,
                        row.get(23)?,
                        row.get(24)?,
                        row.get(25)?,
                        row.get(26)?,
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
            seat_position,
            offer_revision,
            parallel_limit,
            token_limit,
            token_period_json,
            daily_rate_minor,
            currency,
            service_duration_days,
            grants_json,
            share_name,
            installation_id,
            bindings_json,
            enabled_claude,
            enabled_codex,
            enabled_gemini,
            app_runtimes_json,
            app_providers_json,
            fallback_app,
            share_parallel_limit,
            share_token_limit,
            share_tokens_used,
        )) = row
        else {
            return Err(AppError::Conflict(
                "seat or its active Share is unavailable".into(),
            ));
        };
        if listing_status != "active" || seat_status != SEAT_AVAILABLE {
            return Err(AppError::Conflict("seat is no longer available".into()));
        }
        ensure_share_market_app_scope_supported_tx(tx, &share_id)?;
        ensure_share_total_tokens_available(share_token_limit, share_tokens_used)?;
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
        let service_duration_days = service_duration_days
            .map(|days| {
                let days = u32::try_from(days)
                    .map_err(|_| AppError::Internal("Share service duration is invalid".into()))?;
                if !(1..=MAX_SERVICE_DURATION_DAYS).contains(&days) {
                    return Err(AppError::Internal(
                        "Share service duration is outside the supported range".into(),
                    ));
                }
                Ok(days)
            })
            .transpose()?;
        let free_usage_seconds = if daily_rate_minor.is_none() {
            service_duration_days.map(|days| i64::from(days) * 86_400)
        } else {
            None
        };
        let token_period: ShareTokenPeriod = serde_json::from_str(&token_period_json)
            .map_err(|_| AppError::Internal("stored seat token period is invalid".into()))?;
        let current_offer_terms = RentOfferTerms {
            owner_email: owner_email.to_ascii_lowercase(),
            seat_position,
            parallel_limit: parallel_limit.and_then(|value| u32::try_from(value).ok()),
            token_limit: token_limit.and_then(|value| u64::try_from(value).ok()),
            token_period,
            daily_rate_minor,
            currency: currency.clone(),
            service_duration_days,
        };
        let current_service = build_rent_service_snapshot(
            &bindings_json,
            app_runtimes_json.as_deref(),
            app_providers_json.as_deref(),
            &fallback_app,
            &ShareSupport {
                claude: enabled_claude != 0,
                codex: enabled_codex != 0,
                gemini: enabled_gemini != 0,
            },
            share_parallel_limit,
            share_token_limit,
            share_tokens_used,
        )?;
        let now_dt = parse_time(now)?;
        for app in &current_service.supported_apps {
            let availability = evaluate_share_market_service_availability_tx(
                tx,
                &share_id,
                app,
                None,
                None,
                Some(&current_service),
                now_dt,
            )?;
            if !availability.available() {
                return Err(availability_error(app, &availability));
            }
            ensure_fixed_service_term_fulfillable_tx(
                tx,
                &share_id,
                app,
                service_duration_days,
                now_dt,
            )?;
        }
        if let Some(snapshot) = quote_snapshot {
            if snapshot.seat_id != seat_id
                || snapshot.listing_id != listing_id
                || snapshot.share_id != share_id
                || snapshot.offer_revision != offer_revision
            {
                return Err(AppError::Conflict(
                    "Share rent quote no longer matches this seat".into(),
                ));
            }
            validate_offer_terms_unchanged(snapshot, &current_offer_terms)?;
            validate_service_snapshot_unchanged(&snapshot.service, &current_service)?;
        }
        if let (Some(seat_limit), Some(share_limit)) = (
            parallel_limit.and_then(|value| u32::try_from(value).ok()),
            current_service.share_parallel_limit,
        ) && seat_limit > share_limit
        {
            return Err(AppError::coded_conflict(
                "share_market_seat_capacity_invalid",
                "seat concurrency exceeds the Share's total concurrency",
                serde_json::json!({ "reason": "seat_capacity_invalid" }),
            ));
        }
        let service_snapshot_json = serde_json::to_string(&current_service).map_err(|error| {
            AppError::Internal(format!(
                "encode Share rental service snapshot failed: {error}"
            ))
        })?;
        let contract_apps_json =
            serde_json::to_string(&current_service.supported_apps).map_err(|error| {
                AppError::Internal(format!("encode Share contract Apps failed: {error}"))
            })?;
        let policy = ShareUserPolicy {
            parallel_limit: parallel_limit.and_then(|value| u32::try_from(value).ok()),
            token_limit: token_limit.and_then(|value| u64::try_from(value).ok()),
            token_period,
            token_period_anchor_at_ms: None,
            expires_at: None,
            allowed_apps: market_allowed_apps(),
        };
        let subscription_id = Uuid::new_v4().to_string();
        let entitlement_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO share_market_subscriptions (
                id, seat_id, listing_id, share_id, installation_id, entitlement_id,
                owner_user_id, owner_email, renter_user_id, renter_email, status,
                parallel_limit, token_limit, token_period_json, daily_rate_minor, currency,
                service_duration_days, offer_revision, release_reason,
                activated_at, expires_at, created_at, updated_at, released_at,
                free_usage_seconds, rent_quote_id, idempotency_key, request_fingerprint,
                required_app, service_snapshot_json, contract_apps_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'grant_pending',
                       ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                       NULL, NULL, ?18, ?19, ?19, NULL, ?20, ?21, ?22, ?23,
                       ?24, ?25, ?26)",
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
                service_duration_days.map(i64::from),
                offer_revision,
                Option::<String>::None,
                now,
                free_usage_seconds,
                metadata.map(|value| value.quote_id.as_str()),
                metadata.map(|value| value.idempotency_key.as_str()),
                metadata.map(|value| value.request_fingerprint.as_str()),
                current_service.required_app,
                service_snapshot_json,
                contract_apps_json,
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
            crate::market_billing::reserve_contract_tx(
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
                quoted_trial_seconds.unwrap_or(i64::MAX),
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
        let conn = self.conn.lock().await;
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
            Option<i64>,
            Option<String>,
        )> = tx
            .query_row(
                "SELECT share_id, seat_id, listing_id, entitlement_id, owner_user_id,
                        renter_user_id, renter_email, status, daily_rate_minor,
                        service_duration_days, service_started_at
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
                        row.get(9)?,
                        row.get(10)?,
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
            service_duration_days,
            service_started_at,
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
        if owner_override
            && daily_rate_minor.is_some()
            && service_duration_days.is_some()
            && service_started_at.is_some()
        {
            return Err(AppError::coded_conflict(
                "share_market_termination_quote_required",
                "preview and confirm the fixed-term refund before terminating this rental",
                serde_json::json!({
                    "reason": "termination_quote_required",
                    "subscriptionId": subscription_id,
                }),
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
            for pricing_kind in [
                crate::market_access::PRICING_FREE,
                crate::market_access::PRICING_PAID,
            ] {
                crate::market_access::set_product_access_decision_tx(
                    &tx,
                    &owner_user_id,
                    &session.email,
                    &renter_user_id,
                    &renter_email,
                    crate::market_access::PRODUCT_SHARE,
                    pricing_kind,
                    crate::market_access::DECISION_DENY,
                    &session.user_id,
                    &now,
                )?;
            }
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

    pub async fn share_market_create_termination_quote(
        &self,
        session: &AuthSession,
        subscription_id: &str,
    ) -> Result<TerminationQuoteView, AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let expires_at = (now_dt + Duration::seconds(TERMINATION_QUOTE_TTL_SECS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Share termination quote"))?;
        let owner_user_id = tx
            .query_row(
                "SELECT owner_user_id FROM share_market_subscriptions
                 WHERE id = ?1 AND daily_rate_minor IS NOT NULL
                   AND service_duration_days IS NOT NULL
                   AND service_started_at IS NOT NULL
                   AND status NOT IN ('released', 'grant_failed')",
                params![subscription_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db(
                "read fixed Share subscription for termination quote",
            ))?
            .ok_or_else(|| {
                AppError::coded_conflict(
                    "share_market_fixed_term_required",
                    "only an active paid fixed-term rental uses the refund termination flow",
                    serde_json::json!({ "reason": "fixed_term_required" }),
                )
            })?;
        if owner_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only the Share owner can terminate this rental".into(),
            ));
        }
        let calculation = crate::market_billing::supplier_termination_calculation_tx(
            &tx,
            "share",
            subscription_id,
            now_dt,
        )?;
        let quote_id = Uuid::new_v4().to_string();
        let snapshot_json = serde_json::to_string(&calculation).map_err(|error| {
            AppError::Internal(format!("encode Share termination quote failed: {error}"))
        })?;
        tx.execute(
            "UPDATE share_market_termination_quotes
             SET status = 'expired', updated_at = ?3
             WHERE subscription_id = ?1 AND owner_user_id = ?2 AND status = 'active'",
            params![subscription_id, session.user_id, now],
        )
        .map_err(map_db("expire previous Share termination quote"))?;
        tx.execute(
            "INSERT INTO share_market_termination_quotes (
                id, subscription_id, owner_user_id, snapshot_json, status,
                expires_at, consumed_adjustment_id, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'active', ?5, NULL, ?6, ?6)",
            params![
                quote_id,
                subscription_id,
                session.user_id,
                snapshot_json,
                expires_at,
                now,
            ],
        )
        .map_err(map_db("create Share termination quote"))?;
        tx.commit()
            .map_err(map_db("commit Share termination quote"))?;
        Ok(TerminationQuoteView {
            id: quote_id,
            subscription_id: subscription_id.to_string(),
            status: "active".into(),
            expires_at,
            calculation,
        })
    }

    pub async fn share_market_commit_termination(
        &self,
        session: &AuthSession,
        subscription_id: &str,
        quote_id: &str,
        idempotency_key: &str,
        deny_future_access: bool,
    ) -> Result<CommitTerminationResponse, AppError> {
        let quote_id = quote_id.trim();
        let idempotency_key = idempotency_key.trim();
        if quote_id.is_empty() {
            return Err(AppError::BadRequest("quoteId is required".into()));
        }
        if idempotency_key.is_empty()
            || idempotency_key.len() > 128
            || idempotency_key.chars().any(char::is_control)
        {
            return Err(AppError::BadRequest(
                "idempotencyKey must contain 1-128 non-control characters".into(),
            ));
        }
        let adjustment_key = format!("share-termination:{subscription_id}:{idempotency_key}");
        let request_fingerprint = crate::api::sha256_hex(
            format!(
                "share-market-termination-v1\n{subscription_id}\n{quote_id}\n{deny_future_access}"
            )
            .as_bytes(),
        );
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Share supplier termination"))?;
        let replayed_adjustment_id = tx
            .query_row(
                "SELECT id FROM market_contract_adjustments
                 WHERE idempotency_key = ?1 AND product_kind = 'share' AND product_ref = ?2",
                params![adjustment_key, subscription_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("check idempotent Share supplier termination"))?;
        let replayed = replayed_adjustment_id.is_some();
        let quote = tx
            .query_row(
                "SELECT owner_user_id, status, expires_at, snapshot_json,
                        consumed_adjustment_id, commit_fingerprint
                 FROM share_market_termination_quotes
                 WHERE id = ?1 AND subscription_id = ?2",
                params![quote_id, subscription_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Share termination quote"))?
            .ok_or_else(|| AppError::NotFound("Share termination quote not found".into()))?;
        if quote.0 != session.user_id {
            return Err(AppError::Forbidden(
                "Share termination quote belongs to another account".into(),
            ));
        }
        let quoted: crate::market_billing::SupplierTerminationCalculation =
            serde_json::from_str(&quote.3).map_err(|_| {
                AppError::Internal("stored Share termination quote is invalid".into())
            })?;
        if quoted.product_kind != "share" || quoted.product_ref != subscription_id {
            return Err(AppError::Conflict(
                "Share termination quote no longer matches this rental".into(),
            ));
        }
        if let Some(adjustment_id) = replayed_adjustment_id.as_deref() {
            if quote.1 != "consumed"
                || quote.4.as_deref() != Some(adjustment_id)
                || quote.5.as_deref() != Some(request_fingerprint.as_str())
            {
                return Err(AppError::Conflict(
                    "termination idempotency key was reused with a different request".into(),
                ));
            }
        } else if quote.1 != "active" || parse_time(&quote.2)? <= now_dt {
            return Err(AppError::Gone(
                "Share termination quote expired; request a new quote".into(),
            ));
        }
        // The quote locks the reviewed refund bracket and base. Service delivered
        // after that snapshot remains billable until terminate_contract_tx below
        // records the final interval at commit time.
        let adjustment = crate::market_billing::apply_quoted_supplier_early_termination_refund_tx(
            &tx,
            &quoted,
            "owner_early_termination",
            &adjustment_key,
            now_dt,
        )?;
        if !replayed {
            terminate_subscription_for_supplier_tx(
                &tx,
                subscription_id,
                "owner_early_termination",
                Some(session),
                deny_future_access,
                &now,
            )?;
            let consumed = tx
                .execute(
                    "UPDATE share_market_termination_quotes
                 SET status = 'consumed', consumed_adjustment_id = ?2,
                     commit_fingerprint = ?3, updated_at = ?4
                 WHERE id = ?1 AND status = 'active'",
                    params![quote_id, adjustment.id, request_fingerprint, now],
                )
                .map_err(map_db("consume Share termination quote"))?;
            if consumed != 1 {
                return Err(AppError::Conflict(
                    "Share termination quote was already consumed".into(),
                ));
            }
        }
        tx.commit()
            .map_err(map_db("commit Share supplier termination"))?;
        Ok(CommitTerminationResponse {
            ok: true,
            subscription_id: subscription_id.to_string(),
            replayed,
            adjustment,
        })
    }
}

fn terminate_subscription_for_supplier_tx(
    tx: &Connection,
    subscription_id: &str,
    reason: &str,
    actor: Option<&AuthSession>,
    deny_future_access: bool,
    now: &str,
) -> Result<(), AppError> {
    let record = subscription_record(tx, subscription_id)?
        .ok_or_else(|| AppError::NotFound("Share subscription not found".into()))?;
    if matches!(record.status.as_str(), SUB_RELEASED | SUB_GRANT_FAILED) {
        return Ok(());
    }
    crate::market_billing::terminate_contract_tx(tx, "share", subscription_id, reason, now)?;
    let retired = retire_unconfirmed_grant_tx(tx, subscription_id, reason, now)?;
    let grants_json = tx
        .query_row(
            "SELECT user_grants_json FROM shares WHERE share_id = ?1",
            params![record.share_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(map_db("read Share grants during supplier termination"))?
        .flatten();
    let has_entitlement = active_entitlement(grants_json.as_deref(), &record.entitlement_id);
    let grant_never_reached_client = !entitlement_was_activated_tx(tx, subscription_id)?
        && !has_entitlement
        && !retired.had_dispatched;
    if grant_never_reached_client {
        finish_release_tx(
            tx,
            subscription_id,
            &record.seat_id,
            &record.listing_id,
            reason,
            now,
        )?;
    } else if has_terminal_revoke_operation_tx(tx, subscription_id)? {
        tx.execute(
            "UPDATE share_market_subscriptions
             SET status = 'revoke_failed', release_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND status NOT IN ('released', 'grant_failed')",
            params![subscription_id, reason, now],
        )
        .map_err(map_db("retain supplier termination revoke dead-letter"))?;
        tx.execute(
            "UPDATE share_market_seats SET status = 'revoking', updated_at = ?2
             WHERE id = ?1 AND current_subscription_id = ?3",
            params![record.seat_id, now, subscription_id],
        )
        .map_err(map_db("retain supplier termination seat isolation"))?;
    } else {
        request_revoke_tx(
            tx,
            subscription_id,
            &record.share_id,
            &record.seat_id,
            &record.entitlement_id,
            &record.renter_email,
            reason,
            now,
        )?;
    }
    tx.execute(
        "UPDATE share_market_subscriptions
         SET integrity_state = 'terminated', integrity_reason = ?2,
             integrity_checked_at = ?3, updated_at = ?3 WHERE id = ?1",
        params![subscription_id, reason, now],
    )
    .map_err(map_db("mark supplier-terminated Share contract"))?;
    if deny_future_access {
        let actor = actor.ok_or_else(|| {
            AppError::BadRequest("an authenticated owner is required to deny future access".into())
        })?;
        for pricing_kind in [
            crate::market_access::PRICING_FREE,
            crate::market_access::PRICING_PAID,
        ] {
            crate::market_access::set_product_access_decision_tx(
                tx,
                &record.owner_user_id,
                &actor.email,
                &record.renter_user_id,
                &record.renter_email,
                crate::market_access::PRODUCT_SHARE,
                pricing_kind,
                crate::market_access::DECISION_DENY,
                &actor.user_id,
                now,
            )?;
        }
    }
    event_tx(
        tx,
        Some(&record.listing_id),
        Some(&record.seat_id),
        Some(subscription_id),
        actor,
        "supplier_termination_requested",
        serde_json::json!({
            "reason": reason,
            "futureAccessDenied": deny_future_access,
        }),
        now,
    )?;
    Ok(())
}

async fn ensure_share_market_seat_online(
    state: &ServerState,
    seat_id: &str,
) -> Result<(), AppError> {
    match share_market_seat_online(state, seat_id).await? {
        Some(true) => Ok(()),
        Some(false) => Err(share_market_offline_error()),
        None => Err(AppError::NotFound("Share Market seat not found".into())),
    }
}

fn share_market_offline_error() -> AppError {
    AppError::coded_conflict(
        "share_market_share_offline",
        "the Share is offline; retry after its owner restores service",
        serde_json::json!({ "reason": "share_offline" }),
    )
}

async fn share_market_seat_online(
    state: &ServerState,
    seat_id: &str,
) -> Result<Option<bool>, AppError> {
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
    };
    let Some(subdomain) = subdomain else {
        return Ok(None);
    };
    if subdomain.is_empty() {
        return Ok(Some(false));
    }
    Ok(Some(
        state
            .proxy
            .active_subdomains()
            .await
            .iter()
            .any(|active| active.eq_ignore_ascii_case(&subdomain)),
    ))
}

async fn quote_seat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(seat_id): Path<String>,
    input: Option<Json<QuoteSeatRequest>>,
) -> Result<Json<RentQuoteView>, AppError> {
    let session = require_session(&state, &headers).await?;
    ensure_share_market_seat_online(&state, &seat_id).await?;
    Ok(Json(
        state
            .store
            .share_market_create_rent_quote(
                &session,
                &seat_id,
                input
                    .as_ref()
                    .and_then(|input| input.required_app.as_deref()),
            )
            .await?,
    ))
}

async fn rent_seat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(seat_id): Path<String>,
    Json(input): Json<CommitRentQuoteRequest>,
) -> Result<Json<CommitRentQuoteResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let share_online = share_market_seat_online(&state, &seat_id)
        .await?
        .unwrap_or(false);
    let response = state
        .store
        .share_market_commit_rent_quote_with_online_state(
            &session,
            &seat_id,
            &input.quote_id,
            &input.idempotency_key,
            share_online,
        )
        .await?;
    Ok(Json(response))
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
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn retry_grant_subscription(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(subscription_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_retry_grant(&session, &subscription_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn quote_subscription_termination(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(subscription_id): Path<String>,
) -> Result<Json<TerminationQuoteView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .share_market_create_termination_quote(&session, &subscription_id)
            .await?,
    ))
}

async fn commit_subscription_termination(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(subscription_id): Path<String>,
    Json(input): Json<CommitTerminationRequest>,
) -> Result<Json<CommitTerminationResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .share_market_commit_termination(
                &session,
                &subscription_id,
                &input.quote_id,
                &input.idempotency_key,
                input.deny_future_access,
            )
            .await?,
    ))
}

async fn propose_price_change(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(subscription_id): Path<String>,
    Json(input): Json<ProposePriceChangeRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    let proposal_id = state
        .store
        .share_market_propose_price_change(&session, &subscription_id, input)
        .await?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "proposalId": proposal_id,
    })))
}

async fn accept_price_change(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(proposal_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_accept_price_change(&session, &proposal_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn reject_price_change(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(proposal_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_reject_price_change(&session, &proposal_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn cancel_price_change(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(proposal_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = require_session(&state, &headers).await?;
    state
        .store
        .share_market_cancel_price_change(&session, &proposal_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn active_entitlement(grants_json: Option<&str>, entitlement_id: &str) -> bool {
    active_entitlement_grant(grants_json, entitlement_id).is_some()
}

fn active_entitlement_grant(
    grants_json: Option<&str>,
    entitlement_id: &str,
) -> Option<ShareUserGrant> {
    grants_json
        .and_then(|value| serde_json::from_str::<BTreeMap<String, ShareUserGrant>>(value).ok())
        .and_then(|grants| {
            grants.into_values().find(|grant| {
                grant.active
                    && grant.manager == ShareGrantManager::RouterShareMarket
                    && grant.entitlement_id.as_deref() == Some(entitlement_id)
            })
        })
}

fn validate_managed_grant_applied_at(
    subscription_created_at: &str,
    applied_at_ms: i64,
    observed_at: &str,
) -> Result<(), AppError> {
    let applied_at = DateTime::<Utc>::from_timestamp_millis(applied_at_ms).ok_or_else(|| {
        AppError::BadRequest("managed grant appliedAtMs is outside the supported range".into())
    })?;
    let subscription_created_at = parse_time(subscription_created_at)?;
    let latest_allowed = parse_time(observed_at)?
        .checked_add_signed(Duration::seconds(MANAGED_GRANT_MAX_FUTURE_SKEW_SECS))
        .ok_or_else(|| AppError::Internal("managed grant clock-skew boundary overflowed".into()))?;
    if applied_at < subscription_created_at || applied_at > latest_allowed {
        return Err(AppError::Conflict(
            "client reported a managed grant application time outside the rental lifetime".into(),
        ));
    }
    Ok(())
}

fn materialize_initial_policy(
    record: &SubscriptionRecord,
    applied_at_ms: i64,
    policy: &ShareUserPolicy,
) -> Result<ShareUserPolicy, AppError> {
    let applied_at = DateTime::<Utc>::from_timestamp_millis(applied_at_ms).ok_or_else(|| {
        AppError::BadRequest("managed grant appliedAtMs is outside the supported range".into())
    })?;
    let token_period: ShareTokenPeriod = serde_json::from_str(&record.token_period_json)
        .map_err(|_| AppError::Internal("stored seat token period is invalid".into()))?;
    let expires_at = record
        .service_duration_days
        .map(|days| {
            i64::from(days)
                .checked_mul(86_400_000)
                .and_then(|duration| applied_at_ms.checked_add(duration))
                .ok_or_else(|| AppError::Internal("Share service expiry overflowed".into()))
        })
        .transpose()?;
    let mut materialized = policy.clone();
    materialized.token_period_anchor_at_ms = token_period_anchor_at_ms(token_period, applied_at);
    materialized.expires_at = expires_at;
    Ok(materialized)
}

fn persist_effective_control_policy(
    conn: &Connection,
    subscription_id: &str,
    policy: &ShareUserPolicy,
) -> Result<(), AppError> {
    let policy_json = serde_json::to_string(policy).map_err(|error| {
        AppError::Internal(format!(
            "encode effective Share grant policy failed: {error}"
        ))
    })?;
    conn.execute(
        "UPDATE share_control_operations
         SET policy_json = ?2
         WHERE subscription_id = ?1 AND action = 'upsert'
           AND status IN ('pending', 'dispatched')",
        params![subscription_id, policy_json],
    )
    .map_err(map_db("persist effective Share grant policy"))?;
    Ok(())
}

fn effective_policy_has_contract_app_scope(
    record: &SubscriptionRecord,
    effective_policy: &ShareUserPolicy,
) -> bool {
    effective_policy.allowed_apps == market_allowed_apps()
        || (record.service_schema_version < 2
            && record.app_scope_enforced_at.is_none()
            && effective_policy.allowed_apps == [record.app_type.clone()])
}

fn validate_effective_initial_policy(
    record: &SubscriptionRecord,
    applied_at_ms: i64,
    effective_policy: &ShareUserPolicy,
) -> Result<(), AppError> {
    let token_period: ShareTokenPeriod = serde_json::from_str(&record.token_period_json)
        .map_err(|_| AppError::Internal("stored seat token period is invalid".into()))?;
    let expected_anchor = token_period_anchor_at_ms(
        token_period,
        DateTime::<Utc>::from_timestamp_millis(applied_at_ms).ok_or_else(|| {
            AppError::BadRequest("managed grant appliedAtMs is outside the supported range".into())
        })?,
    );
    let expected_expiry = record
        .service_duration_days
        .map(|days| {
            i64::from(days)
                .checked_mul(86_400_000)
                .and_then(|duration| applied_at_ms.checked_add(duration))
                .ok_or_else(|| AppError::Internal("Share service expiry overflowed".into()))
        })
        .transpose()?;
    let expected_parallel = record
        .parallel_limit
        .and_then(|value| u32::try_from(value).ok());
    let expected_tokens = record
        .token_limit
        .and_then(|value| u64::try_from(value).ok());
    if effective_policy.parallel_limit != expected_parallel
        || effective_policy.token_limit != expected_tokens
        || effective_policy.token_period != token_period
        || effective_policy.token_period_anchor_at_ms != expected_anchor
        || effective_policy.expires_at != expected_expiry
        || !effective_policy_has_contract_app_scope(record, effective_policy)
    {
        return Err(AppError::Conflict(
            "client applied a managed grant policy that differs from the rented terms".into(),
        ));
    }
    Ok(())
}

fn validate_effective_resume_policy(
    record: &SubscriptionRecord,
    effective_policy: &ShareUserPolicy,
) -> Result<(), AppError> {
    let token_period: ShareTokenPeriod = serde_json::from_str(&record.token_period_json)
        .map_err(|_| AppError::Internal("stored seat token period is invalid".into()))?;
    let service_started_at = record.service_started_at.as_deref().ok_or_else(|| {
        AppError::Internal("billing-suspended Share service start is missing".into())
    })?;
    let expected_anchor = token_period_anchor_at_ms(token_period, parse_time(service_started_at)?);
    let expected_expiry = record
        .expires_at
        .as_deref()
        .map(parse_time)
        .transpose()?
        .map(|value| value.timestamp_millis());
    let expected_parallel = record
        .parallel_limit
        .and_then(|value| u32::try_from(value).ok());
    let expected_tokens = record
        .token_limit
        .and_then(|value| u64::try_from(value).ok());
    if effective_policy.parallel_limit != expected_parallel
        || effective_policy.token_limit != expected_tokens
        || effective_policy.token_period != token_period
        || effective_policy.token_period_anchor_at_ms != expected_anchor
        || effective_policy.expires_at != expected_expiry
        || !effective_policy_has_contract_app_scope(record, effective_policy)
    {
        return Err(AppError::Conflict(
            "client restored a managed grant policy that differs from the active rental contract"
                .into(),
        ));
    }
    Ok(())
}

fn quarantine_invalid_effective_grant_tx(
    conn: &Connection,
    record: &SubscriptionRecord,
    operation_id: &str,
    edit_id: Option<&str>,
    violation: &str,
    now: &str,
) -> Result<(), AppError> {
    let violation = crate::store::client_chat::sanitize_system_event_text(violation);
    if let Some(edit_id) = edit_id {
        conn.execute(
            "UPDATE share_edit_requests
             SET status = CASE WHEN status = 'pending' THEN 'rejected' ELSE status END,
                 error_message = ?2, error_code = ?3, updated_at = ?4,
                 retired_at = COALESCE(retired_at, ?4)
             WHERE id = ?1",
            params![edit_id, violation, SHARE_GRANT_CONTRACT_VIOLATION_CODE, now],
        )
        .map_err(map_db("record invalid Share grant acknowledgement"))?;
    }
    retire_unconfirmed_grant_tx(conn, &record.id, &violation, now)?;
    conn.execute(
        "UPDATE share_control_operations
         SET dead_lettered_at = COALESCE(dead_lettered_at, ?2), last_error = ?3
         WHERE id = ?1 AND status = 'rejected'",
        params![operation_id, now, violation],
    )
    .map_err(map_db("dead-letter invalid Share grant operation"))?;
    request_revoke_tx(
        conn,
        &record.id,
        &record.share_id,
        &record.seat_id,
        &record.entitlement_id,
        &record.renter_email,
        SHARE_GRANT_CONTRACT_VIOLATION_CODE,
        now,
    )?;
    event_tx(
        conn,
        Some(&record.listing_id),
        Some(&record.seat_id),
        Some(&record.id),
        None,
        "entitlement_contract_violation",
        serde_json::json!({
            "operationId": operation_id,
            "errorCode": SHARE_GRANT_CONTRACT_VIOLATION_CODE,
            "error": violation,
        }),
        now,
    )?;
    crate::store::enqueue_operator_alert_signal_tx(
        conn,
        &format!("share-grant-contract-violation:{operation_id}"),
        &format!("share_grant_contract_violation:share:{}", record.share_id),
        "firing",
        "share_grant_contract_violation",
        "share",
        Some(&record.share_id),
        "critical",
        "Share grant violated the rental contract",
        &format!(
            "Share {} applied an invalid entitlement for subscription {}. Automatic revocation was queued.",
            record.share_id, record.id
        ),
        serde_json::json!({
            "operationId": operation_id,
            "subscriptionId": record.id,
            "shareId": record.share_id,
            "errorCode": SHARE_GRANT_CONTRACT_VIOLATION_CODE,
            "error": violation,
        }),
        parse_time(now)?,
    )?;
    Ok(())
}

fn latest_upsert_control_context(
    conn: &Connection,
    subscription_id: &str,
) -> Result<(String, Option<String>), AppError> {
    conn.query_row(
        "SELECT id, edit_id FROM share_control_operations
         WHERE subscription_id = ?1 AND action = 'upsert'
         ORDER BY share_sequence DESC LIMIT 1",
        params![subscription_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(map_db("read Share grant control context"))?
    .ok_or_else(|| AppError::Internal("Share grant control operation is missing".into()))
}

fn reject_unfulfillable_activation_tx(
    conn: &Connection,
    record: &SubscriptionRecord,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    crate::market_billing::terminate_contract_tx(conn, "share", &record.id, reason, now)?;
    let retired = retire_unconfirmed_grant_tx(conn, &record.id, reason, now)?;
    let grants_json = conn
        .query_row(
            "SELECT user_grants_json FROM shares WHERE share_id = ?1",
            params![record.share_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(map_db("read Share grants during rejected activation"))?
        .flatten();
    let has_entitlement = active_entitlement(grants_json.as_deref(), &record.entitlement_id);
    conn.execute(
        "UPDATE share_market_subscriptions
         SET integrity_state = 'terminated', integrity_reason = ?2,
             integrity_checked_at = ?3, integrity_violated_at = COALESCE(integrity_violated_at, ?3),
             release_reason = ?2, updated_at = ?3
         WHERE id = ?1",
        params![record.id, reason, now],
    )
    .map_err(map_db("record unfulfillable Share activation"))?;
    if !has_entitlement && !retired.had_dispatched {
        finish_release_tx(
            conn,
            &record.id,
            &record.seat_id,
            &record.listing_id,
            reason,
            now,
        )?;
    } else {
        request_revoke_tx(
            conn,
            &record.id,
            &record.share_id,
            &record.seat_id,
            &record.entitlement_id,
            &record.renter_email,
            reason,
            now,
        )?;
    }
    event_tx(
        conn,
        Some(&record.listing_id),
        Some(&record.seat_id),
        Some(&record.id),
        None,
        "service_activation_rejected",
        serde_json::json!({ "reason": reason }),
        now,
    )?;
    Ok(())
}

fn activate_granted_subscription_tx(
    conn: &Connection,
    record: &SubscriptionRecord,
    applied_at_ms: i64,
    effective_policy: &ShareUserPolicy,
    now: &str,
) -> Result<(), AppError> {
    if record.status != SUB_GRANT_PENDING {
        return Ok(());
    }
    validate_managed_grant_applied_at(&record.created_at, applied_at_ms, now)?;
    validate_effective_initial_policy(record, applied_at_ms, effective_policy)?;
    let service_started_at = DateTime::<Utc>::from_timestamp_millis(applied_at_ms)
        .ok_or_else(|| {
            AppError::BadRequest("managed grant appliedAtMs is outside the supported range".into())
        })?
        .to_rfc3339();
    let expires_at = effective_policy
        .expires_at
        .map(|expires_at| {
            DateTime::<Utc>::from_timestamp_millis(expires_at)
                .map(|value| value.to_rfc3339())
                .ok_or_else(|| {
                    AppError::BadRequest(
                        "managed grant effective expiry is outside the supported range".into(),
                    )
                })
        })
        .transpose()?;
    let status = if record.daily_rate_minor.is_some() {
        SUB_ACTIVE_POSTPAID
    } else {
        SUB_ACTIVE_FREE
    };
    let (service_snapshot_json, integrity_state): (String, String) = conn
        .query_row(
            "SELECT service_snapshot_json, integrity_state
             FROM share_market_subscriptions WHERE id = ?1",
            params![record.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_db("read Share activation service contract"))?;
    let service_snapshot = serde_json::from_str::<RentServiceSnapshot>(&service_snapshot_json).ok();
    for app in subscription_contract_apps(record) {
        let availability = evaluate_share_market_service_availability_tx(
            conn,
            &record.share_id,
            &app,
            Some(true),
            Some(&integrity_state),
            service_snapshot.as_ref(),
            parse_time(&service_started_at)?,
        )?;
        if let Some(reason) = availability.blocking_reason.as_deref() {
            reject_unfulfillable_activation_tx(conn, record, reason, now)?;
            return Ok(());
        }
        if let Err(error) = ensure_fixed_service_term_fulfillable_tx(
            conn,
            &record.share_id,
            &app,
            record.service_duration_days,
            parse_time(&service_started_at)?,
        ) {
            tracing::warn!(
                subscription_id = %record.id,
                share_id = %record.share_id,
                app,
                error = %error,
                "Share fixed service term became unfulfillable before activation"
            );
            reject_unfulfillable_activation_tx(
                conn,
                record,
                "service_term_unfulfillable_before_activation",
                now,
            )?;
            return Ok(());
        }
    }
    if record.daily_rate_minor.is_some()
        && !crate::market_billing::activate_reserved_contract_tx(
            conn,
            "share",
            &record.id,
            &service_started_at,
            now,
        )?
    {
        request_revoke_tx(
            conn,
            &record.id,
            &record.share_id,
            &record.seat_id,
            &record.entitlement_id,
            &record.renter_email,
            "billing_account_unavailable_before_activation",
            now,
        )?;
        return Ok(());
    }
    let app_scope_enforced_at =
        (effective_policy.allowed_apps == market_allowed_apps()).then_some(now);
    let changed = conn
        .execute(
            "UPDATE share_market_subscriptions
             SET status = ?2, activated_at = ?3, service_started_at = ?3,
                 expires_at = ?4, release_reason = NULL,
                 app_scope_enforced_at = COALESCE(app_scope_enforced_at, ?5),
                 updated_at = ?6
             WHERE id = ?1 AND status = 'grant_pending' AND service_started_at IS NULL",
            params![
                record.id,
                status,
                service_started_at,
                expires_at,
                app_scope_enforced_at,
                now
            ],
        )
        .map_err(map_db("activate granted Share subscription"))?;
    if changed == 0 {
        return Ok(());
    }
    conn.execute(
        "UPDATE share_market_seats SET status = 'occupied', updated_at = ?2
         WHERE id = ?1 AND current_subscription_id = ?3",
        params![record.seat_id, now, record.id],
    )
    .map_err(map_db("occupy activated Share seat"))?;
    event_tx(
        conn,
        Some(&record.listing_id),
        Some(&record.seat_id),
        Some(&record.id),
        None,
        "entitlement_activated",
        serde_json::json!({
            "free": record.daily_rate_minor.is_none(),
            "serviceDurationDays": record.service_duration_days,
            "serviceStartedAt": service_started_at,
            "expiresAt": expires_at,
        }),
        now,
    )?;
    Ok(())
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
    tx: &Connection,
    subscription_id: &str,
    reason: &str,
    now: &str,
) -> Result<RetireUnconfirmedGrant, AppError> {
    let message = format!("grant canceled before confirmation: {reason}");
    let had_dispatched = tx
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM share_control_operations
                WHERE subscription_id = ?1 AND action = 'upsert' AND attempts > 0
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

fn entitlement_was_activated_tx(tx: &Connection, subscription_id: &str) -> Result<bool, AppError> {
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

fn has_applied_revoke_after_latest_upsert_tx(
    tx: &Connection,
    subscription_id: &str,
) -> Result<bool, AppError> {
    tx.query_row(
        "SELECT EXISTS (
             SELECT 1
             FROM share_control_operations revoke
             LEFT JOIN share_edit_requests edit ON edit.id = revoke.edit_id
             WHERE revoke.subscription_id = ?1 AND revoke.action = 'revoke'
               AND revoke.share_sequence > COALESCE((
                   SELECT MAX(upsert.share_sequence)
                   FROM share_control_operations upsert
                   WHERE upsert.subscription_id = ?1 AND upsert.action = 'upsert'
               ), 0)
               AND (revoke.status = 'applied' OR edit.status = 'applied')
         )",
        params![subscription_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(map_db("confirm post-violation Share revoke"))
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
                     next_attempt_at = ?2,
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

fn expire_stale_control_edits_tx(
    tx: &Transaction<'_>,
    now_dt: DateTime<Utc>,
) -> Result<(), AppError> {
    let now = now_dt.to_rfc3339();
    let expired = {
        let mut statement = tx
            .prepare(
                "SELECT edit.id
                 FROM share_edit_requests edit
                 INNER JOIN share_control_operations operation ON operation.edit_id = edit.id
                 WHERE edit.status = 'pending' AND edit.retired_at IS NULL
                   AND edit.expires_at IS NOT NULL AND edit.expires_at <= ?1
                   AND operation.status = 'dispatched'
                 ORDER BY edit.expires_at, edit.created_at",
            )
            .map_err(map_db("prepare expired Share control edits"))?;
        statement
            .query_map(params![now], |row| row.get::<_, String>(0))
            .map_err(map_db("query expired Share control edits"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read expired Share control edits"))?
    };
    for edit_id in expired {
        let changed = tx
            .execute(
                "UPDATE share_edit_requests
                 SET status = 'rejected', retired_at = ?2, updated_at = ?2,
                     error_message = 'Share control acknowledgement timed out',
                     error_code = 'control_ack_timeout'
                 WHERE id = ?1 AND status = 'pending' AND retired_at IS NULL",
                params![edit_id, now],
            )
            .map_err(map_db("expire Share control edit"))?;
        if changed == 1 {
            handle_control_edit_ack_with_metadata(
                tx,
                &edit_id,
                "rejected",
                Some("Share control acknowledgement timed out"),
                Some("control_ack_timeout"),
                Some(true),
                &now,
            )?;
        }
    }
    Ok(())
}

fn control_retry_at(now: &str, attempts: i64) -> Result<String, AppError> {
    let exponent = u32::try_from(attempts.saturating_sub(1).clamp(0, 16)).unwrap_or(0);
    let delay = CONTROL_RETRY_BASE_SECS
        .saturating_mul(2_i64.saturating_pow(exponent))
        .min(CONTROL_RETRY_MAX_SECS);
    Ok((parse_time(now)? + Duration::seconds(delay)).to_rfc3339())
}

fn finish_release_tx(
    tx: &Connection,
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
    let recycled = recycle_released_seat(tx, seat_id, subscription_id, now)?;
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
    if recycled {
        event_tx(
            tx,
            Some(listing_id),
            Some(seat_id),
            Some(subscription_id),
            None,
            "seat_available",
            serde_json::json!({ "reason": reason }),
            now,
        )?;
    }
    Ok(())
}

fn recycle_released_seat(
    conn: &Connection,
    seat_id: &str,
    subscription_id: &str,
    now: &str,
) -> Result<bool, AppError> {
    let reusable = conn
        .query_row(
            "SELECT EXISTS (
                SELECT 1
                FROM share_market_seats seat
                INNER JOIN share_market_listings listing ON listing.id = seat.listing_id
                INNER JOIN shares share ON share.share_id = listing.share_id
                INNER JOIN share_market_subscriptions subscription
                  ON subscription.id = seat.current_subscription_id
                WHERE seat.id = ?1 AND seat.current_subscription_id = ?2
                  AND seat.status NOT IN ('disabled', 'deleted')
                  AND listing.status = 'active' AND listing.deleted_at IS NULL
                  AND share.share_status = 'active'
                  AND lower(listing.owner_email) = lower(share.owner_email)
                  AND COALESCE(subscription.release_reason, '') NOT IN
                      ('share_missing', 'share_unavailable', 'installation_takeover',
                       'share_deleted', 'listing_deleted')
             )",
            params![seat_id, subscription_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("check released Share seat reuse"))?
        != 0;
    if reusable {
        let changed = conn
            .execute(
                "UPDATE share_market_seats
                 SET status = 'available', current_subscription_id = NULL,
                     retired_subscription_id = NULL, retired_at = NULL, updated_at = ?3
                 WHERE id = ?1 AND current_subscription_id = ?2
                   AND status NOT IN ('disabled', 'deleted')",
                params![seat_id, subscription_id, now],
            )
            .map_err(map_db("recycle released Share seat"))?;
        return Ok(changed > 0);
    }
    retire_seat(conn, seat_id, subscription_id, now)
}

pub(crate) fn terminate_installation_for_takeover_tx(
    tx: &crate::db::Connection,
    installation_id: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE market_access_requests
         SET status = 'cancelled', revision = revision + 1, resolved_at = ?2,
             resolution_reason = ?3
         WHERE status = 'requested' AND target_kind = 'share_seat'
           AND target_id IN (
               SELECT seat.id
               FROM share_market_seats seat
               INNER JOIN share_market_listings listing ON listing.id = seat.listing_id
               WHERE listing.installation_id = ?1
           )",
        params![installation_id, now, reason],
    )
    .map_err(map_db("cancel takeover Share access requests"))?;
    let subscriptions = {
        let mut statement = tx
            .prepare(
                "SELECT id, seat_id
                 FROM share_market_subscriptions
                 WHERE installation_id = ?1
                   AND status NOT IN ('released', 'grant_failed')",
            )
            .map_err(map_db("prepare takeover Share subscriptions"))?;
        let rows = statement
            .query_map(params![installation_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_db("query takeover Share subscriptions"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read takeover Share subscriptions"))?
    };
    for (subscription_id, seat_id) in subscriptions {
        apply_automatic_supplier_termination_refund_tx(tx, &subscription_id, reason, now)?;
        crate::market_billing::terminate_contract_tx(tx, "share", &subscription_id, reason, now)?;
        tx.execute(
            "UPDATE share_control_operations
             SET status = 'rejected', last_error = ?2, updated_at = ?3
             WHERE subscription_id = ?1 AND status IN ('pending', 'dispatched')",
            params![subscription_id, reason, now],
        )
        .map_err(map_db("retire takeover Share control operations"))?;
        tx.execute(
            "UPDATE share_market_subscriptions
             SET status = 'released', release_reason = ?2, updated_at = ?3, released_at = ?3
             WHERE id = ?1 AND status NOT IN ('released', 'grant_failed')",
            params![subscription_id, reason, now],
        )
        .map_err(map_db("release takeover Share subscription"))?;
        tx.execute(
            "UPDATE share_market_seats
             SET status = 'deleted', current_subscription_id = NULL,
                 retired_subscription_id = COALESCE(retired_subscription_id, ?2),
                 retired_at = COALESCE(retired_at, ?3), updated_at = ?3
             WHERE id = ?1",
            params![seat_id, subscription_id, now],
        )
        .map_err(map_db("retire takeover Share seat"))?;
    }

    tx.execute(
        "UPDATE share_market_seats
         SET status = 'deleted', current_subscription_id = NULL,
             retired_at = COALESCE(retired_at, ?2), updated_at = ?2
         WHERE listing_id IN (
             SELECT id FROM share_market_listings WHERE installation_id = ?1
         ) AND status != 'deleted'",
        params![installation_id, now],
    )
    .map_err(map_db("delete remaining takeover Share seats"))?;
    tx.execute(
        "UPDATE share_market_listings
         SET status = 'closed', deleted_at = COALESCE(deleted_at, ?2), updated_at = ?2
         WHERE installation_id = ?1 AND status != 'closed'",
        params![installation_id, now],
    )
    .map_err(map_db("close takeover Share listings"))?;
    Ok(())
}

pub(crate) fn retire_deleted_share_market_tx(
    conn: &Connection,
    share_id: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE market_access_requests
         SET status = 'cancelled', revision = revision + 1, resolved_at = ?2,
             resolution_reason = ?3
         WHERE status = 'requested' AND target_kind = 'share_seat'
           AND target_id IN (
               SELECT seat.id
               FROM share_market_seats seat
               INNER JOIN share_market_listings listing ON listing.id = seat.listing_id
               WHERE listing.share_id = ?1
           )",
        params![share_id, now, reason],
    )
    .map_err(map_db("cancel deleted Share access requests"))?;

    let subscriptions = {
        let mut statement = conn
            .prepare(
                "SELECT id, seat_id, listing_id
                 FROM share_market_subscriptions
                 WHERE share_id = ?1 AND status NOT IN ('released', 'grant_failed')",
            )
            .map_err(map_db("prepare deleted Share subscriptions"))?;
        statement
            .query_map(params![share_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(map_db("query deleted Share subscriptions"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read deleted Share subscriptions"))?
    };
    for (subscription_id, seat_id, listing_id) in subscriptions {
        apply_automatic_supplier_termination_refund_tx(conn, &subscription_id, reason, now)?;
        crate::market_billing::terminate_contract_tx(conn, "share", &subscription_id, reason, now)?;
        conn.execute(
            "UPDATE share_control_operations
             SET status = 'rejected', last_error = ?2, updated_at = ?3
             WHERE subscription_id = ?1 AND status IN ('pending', 'dispatched')",
            params![subscription_id, reason, now],
        )
        .map_err(map_db("retire deleted Share control operations"))?;
        conn.execute(
            "UPDATE share_market_subscriptions
             SET status = 'released', release_reason = ?2, updated_at = ?3, released_at = ?3
             WHERE id = ?1 AND status NOT IN ('released', 'grant_failed')",
            params![subscription_id, reason, now],
        )
        .map_err(map_db("release deleted Share subscription"))?;
        conn.execute(
            "UPDATE share_market_seats
             SET status = 'deleted', current_subscription_id = NULL,
                 retired_subscription_id = COALESCE(retired_subscription_id, ?2),
                 retired_at = COALESCE(retired_at, ?3), updated_at = ?3
             WHERE id = ?1",
            params![seat_id, subscription_id, now],
        )
        .map_err(map_db("retire deleted Share seat"))?;
        event_tx(
            conn,
            Some(&listing_id),
            Some(&seat_id),
            Some(&subscription_id),
            None,
            "subscription_released",
            serde_json::json!({ "reason": reason }),
            now,
        )?;
    }
    conn.execute(
        "UPDATE share_market_seats
         SET status = 'deleted', current_subscription_id = NULL,
             retired_at = COALESCE(retired_at, ?2), updated_at = ?2
         WHERE listing_id IN (
             SELECT id FROM share_market_listings WHERE share_id = ?1
         ) AND status != 'deleted'",
        params![share_id, now],
    )
    .map_err(map_db("delete remaining Share listing seats"))?;
    conn.execute(
        "UPDATE share_market_listings
         SET status = 'closed', deleted_at = COALESCE(deleted_at, ?2), updated_at = ?2
         WHERE share_id = ?1",
        params![share_id, now],
    )
    .map_err(map_db("close deleted Share listings"))?;
    Ok(())
}

pub(crate) fn rebind_share_market_owner_tx(
    conn: &Connection,
    share_id: &str,
    new_owner_user_id: &str,
    new_owner_email: &str,
    now: &str,
) -> Result<(), AppError> {
    let listings = {
        let mut statement = conn
            .prepare(
                "SELECT id FROM share_market_listings
                 WHERE share_id = ?1 AND deleted_at IS NULL",
            )
            .map_err(map_db("prepare transferred Share listings"))?;
        statement
            .query_map(params![share_id], |row| row.get::<_, String>(0))
            .map_err(map_db("query transferred Share listings"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read transferred Share listings"))?
    };
    if listings.is_empty() {
        return Ok(());
    }

    conn.execute(
        "UPDATE market_access_requests
         SET status = 'cancelled', revision = revision + 1, resolved_at = ?2,
             resolution_reason = 'Share ownership changed; request access from the new owner'
         WHERE status = 'requested' AND target_kind = 'share_seat'
           AND target_id IN (
               SELECT seat.id FROM share_market_seats seat
               INNER JOIN share_market_listings listing ON listing.id = seat.listing_id
               WHERE listing.share_id = ?1
           )",
        params![share_id, now],
    )
    .map_err(map_db("cancel transferred Share access requests"))?;
    conn.execute(
        "UPDATE share_market_listings
         SET owner_user_id = ?2, owner_email = ?3, updated_at = ?4
         WHERE share_id = ?1 AND deleted_at IS NULL",
        params![share_id, new_owner_user_id, new_owner_email, now],
    )
    .map_err(map_db("rebind transferred Share listings"))?;

    let grants_json = conn
        .query_row(
            "SELECT user_grants_json FROM shares WHERE share_id = ?1",
            params![share_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(map_db("read transferred Share grants"))?
        .flatten();
    let subscriptions = {
        let mut statement = conn
            .prepare(
                "SELECT id, seat_id, listing_id, entitlement_id, renter_email
                 FROM share_market_subscriptions
                 WHERE share_id = ?1 AND status NOT IN ('released', 'grant_failed')",
            )
            .map_err(map_db("prepare transferred Share subscriptions"))?;
        statement
            .query_map(params![share_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(map_db("query transferred Share subscriptions"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db("read transferred Share subscriptions"))?
    };
    for (subscription_id, seat_id, listing_id, entitlement_id, renter_email) in subscriptions {
        apply_automatic_supplier_termination_refund_tx(
            conn,
            &subscription_id,
            "share_owner_changed",
            now,
        )?;
        crate::market_billing::terminate_contract_tx(
            conn,
            "share",
            &subscription_id,
            "share_owner_changed",
            now,
        )?;
        let retired =
            retire_unconfirmed_grant_tx(conn, &subscription_id, "share_owner_changed", now)?;
        let has_entitlement = active_entitlement(grants_json.as_deref(), &entitlement_id);
        let grant_never_reached_client = !entitlement_was_activated_tx(conn, &subscription_id)?
            && !has_entitlement
            && !retired.had_dispatched;
        if grant_never_reached_client {
            finish_release_tx(
                conn,
                &subscription_id,
                &seat_id,
                &listing_id,
                "share_owner_changed",
                now,
            )?;
        } else {
            request_revoke_tx(
                conn,
                &subscription_id,
                share_id,
                &seat_id,
                &entitlement_id,
                &renter_email,
                "share_owner_changed",
                now,
            )?;
        }
    }
    conn.execute(
        "UPDATE share_market_subscriptions
         SET owner_user_id = ?2, owner_email = ?3, updated_at = ?4
         WHERE share_id = ?1 AND status NOT IN ('released', 'grant_failed')",
        params![share_id, new_owner_user_id, new_owner_email, now],
    )
    .map_err(map_db("rebind transferred Share subscriptions"))?;
    for listing_id in listings {
        event_tx(
            conn,
            Some(&listing_id),
            None,
            None,
            None,
            "listing_owner_rebound",
            serde_json::json!({
                "newOwnerUserId": new_owner_user_id,
                "newOwnerEmail": new_owner_email,
            }),
            now,
        )?;
    }
    Ok(())
}

fn apply_automatic_supplier_termination_refund_tx(
    conn: &Connection,
    subscription_id: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    let terms = conn
        .query_row(
            "SELECT daily_rate_minor, service_duration_days, service_started_at, expires_at
             FROM share_market_subscriptions WHERE id = ?1",
            params![subscription_id],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read automatic Share termination refund terms"))?;
    let Some((Some(daily_rate_minor), Some(_), Some(_), Some(expires_at))) = terms else {
        return Ok(());
    };
    let now = parse_time(now)?;
    if daily_rate_minor <= 0 || now >= parse_time(&expires_at)? {
        return Ok(());
    }
    crate::market_billing::apply_supplier_early_termination_refund_tx(
        conn,
        "share",
        subscription_id,
        reason,
        &format!("share-automatic-supplier-termination:{subscription_id}"),
        now,
    )?;
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
    tx: &Connection,
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

fn expire_service_term_tx(
    tx: &Transaction<'_>,
    record: &SubscriptionRecord,
    grants_json: Option<&str>,
    expires_at: &str,
    now: &str,
) -> Result<(), AppError> {
    const REASON: &str = "service_term_expired";

    crate::market_billing::terminate_contract_tx(tx, "share", &record.id, REASON, expires_at)?;
    let retired = retire_unconfirmed_grant_tx(tx, &record.id, REASON, now)?;
    let has_entitlement = active_entitlement(grants_json, &record.entitlement_id);

    if !has_entitlement && !retired.had_dispatched {
        if can_confirm_absent_entitlement_tx(tx, &record.id)? {
            confirm_control_effect_tx(tx, &record.id, "revoke", now)?;
        }
        finish_release_tx(
            tx,
            &record.id,
            &record.seat_id,
            &record.listing_id,
            REASON,
            now,
        )?;
    } else if has_terminal_revoke_operation_tx(tx, &record.id)? {
        tx.execute(
            "UPDATE share_market_subscriptions
             SET status = 'revoke_failed', release_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND status NOT IN ('released', 'grant_failed')",
            params![record.id, REASON, now],
        )
        .map_err(map_db("mark expired Share revoke dead-letter"))?;
        tx.execute(
            "UPDATE share_market_seats SET status = 'revoking', updated_at = ?2
             WHERE id = ?1 AND current_subscription_id = ?3",
            params![record.seat_id, now, record.id],
        )
        .map_err(map_db("retain expired Share seat for operator recovery"))?;
    } else {
        request_revoke_tx(
            tx,
            &record.id,
            &record.share_id,
            &record.seat_id,
            &record.entitlement_id,
            &record.renter_email,
            REASON,
            now,
        )?;
    }

    event_tx(
        tx,
        Some(&record.listing_id),
        Some(&record.seat_id),
        Some(&record.id),
        None,
        "service_term_expired",
        serde_json::json!({
            "expiresAt": expires_at,
            "serviceDurationDays": record.service_duration_days,
        }),
        now,
    )?;
    Ok(())
}

fn next_reconcile_subscription_ids(conn: &Connection) -> Result<Vec<String>, AppError> {
    conn.prepare(
        "SELECT id FROM share_market_subscriptions
         WHERE status NOT IN ('released', 'grant_failed')
         ORDER BY COALESCE(last_reconciled_at, ''), created_at, id
         LIMIT ?1",
    )
    .and_then(|mut statement| {
        statement
            .query_map(params![MAX_SUBSCRIPTIONS_PER_RECONCILE], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(map_db("read Share subscriptions for reconciliation"))
}

fn advance_reconcile_subscription_cursor(
    conn: &Connection,
    subscription_ids: &[String],
    reconciled_at: &str,
) -> Result<(), AppError> {
    if subscription_ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", subscription_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "UPDATE share_market_subscriptions SET last_reconciled_at = ?
         WHERE id IN ({placeholders})"
    );
    conn.execute(
        &sql,
        params_from_iter(
            std::iter::once(reconciled_at.to_string()).chain(subscription_ids.iter().cloned()),
        ),
    )
    .map_err(map_db("advance Share reconciliation cursor"))?;
    Ok(())
}

impl AppStore {
    pub async fn share_market_reconcile_and_dispatch(
        &self,
        now_dt: DateTime<Utc>,
    ) -> Result<Vec<ShareEditAvailableEvent>, AppError> {
        let now = now_dt.to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin Share Market reconciliation"))?;
        expire_stale_control_edits_tx(&tx, now_dt)?;
        recover_orphaned_control_edits_tx(&tx, &now)?;

        let subscription_ids = next_reconcile_subscription_ids(&tx)?;
        advance_reconcile_subscription_cursor(&tx, &subscription_ids, &now)?;
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
            let entitlement_grant = share.as_ref().and_then(|(_, _, grants)| {
                active_entitlement_grant(grants.as_deref(), &record.entitlement_id)
            });
            let has_entitlement = entitlement_grant.is_some();

            if !matches!(
                record.status.as_str(),
                SUB_REVOKE_PENDING | SUB_REVOKE_FAILED
            ) && let Some(expires_at) = record.expires_at.as_deref()
            {
                let expires_at_dt = parse_time(expires_at)?;
                if expires_at_dt <= now_dt {
                    expire_service_term_tx(
                        &tx,
                        &record,
                        share.as_ref().and_then(|(_, _, grants)| grants.as_deref()),
                        expires_at,
                        &now,
                    )?;
                    continue;
                }
                if expires_at_dt <= now_dt + Duration::hours(24) {
                    let warned: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM share_market_events
                             WHERE subscription_id = ?1 AND event_type = 'service_term_expiring')",
                            params![record.id],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(map_db("check Share service expiry warning"))?
                        != 0;
                    if !warned {
                        event_tx(
                            &tx,
                            Some(&record.listing_id),
                            Some(&record.seat_id),
                            Some(&record.id),
                            None,
                            "service_term_expiring",
                            serde_json::json!({
                                "expiresAt": expires_at,
                                "serviceDurationDays": record.service_duration_days,
                            }),
                            &now,
                        )?;
                    }
                }
            }

            if !matches!(
                record.status.as_str(),
                SUB_REVOKE_PENDING | SUB_REVOKE_FAILED | SUB_RELEASED | SUB_GRANT_FAILED
            ) && reconcile_subscription_integrity_tx(&tx, &record, now_dt)?
            {
                continue;
            }

            if matches!(
                record.status.as_str(),
                SUB_REVOKE_PENDING | SUB_REVOKE_FAILED
            ) {
                if share.is_none() {
                    confirm_control_effect_tx(&tx, &record.id, "revoke", &now)?;
                    finish_release_tx(
                        &tx,
                        &record.id,
                        &record.seat_id,
                        &record.listing_id,
                        "share_missing",
                        &now,
                    )?;
                } else if !has_entitlement
                    && if record.release_reason.as_deref()
                        == Some(SHARE_GRANT_CONTRACT_VIOLATION_CODE)
                    {
                        has_applied_revoke_after_latest_upsert_tx(&tx, &record.id)?
                    } else {
                        can_confirm_absent_entitlement_tx(&tx, &record.id)?
                    }
                {
                    confirm_control_effect_tx(&tx, &record.id, "revoke", &now)?;
                    let release_reason = record
                        .release_reason
                        .as_deref()
                        .unwrap_or("entitlement_revoked");
                    finish_release_tx(
                        &tx,
                        &record.id,
                        &record.seat_id,
                        &record.listing_id,
                        release_reason,
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
                } else if record.status == SUB_BILLING_CONTROL_FAILED
                    && !has_terminal_revoke_operation_tx(&tx, &record.id)?
                {
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
                if let Some(grant) = entitlement_grant.as_ref() {
                    let applied_at_ms = match i64::try_from(grant.updated_at_ms) {
                        Ok(value) => value,
                        Err(_) => {
                            let (operation_id, edit_id) =
                                latest_upsert_control_context(&tx, &record.id)?;
                            quarantine_invalid_effective_grant_tx(
                                &tx,
                                &record,
                                &operation_id,
                                edit_id.as_deref(),
                                "managed grant applied time is outside the supported range",
                                &now,
                            )?;
                            continue;
                        }
                    };
                    let validation =
                        validate_managed_grant_applied_at(&record.created_at, applied_at_ms, &now)
                            .and_then(|_| validate_effective_resume_policy(&record, &grant.policy));
                    if let Err(error) = validation {
                        if matches!(&error, AppError::Internal(_)) {
                            return Err(error);
                        }
                        let (operation_id, edit_id) =
                            latest_upsert_control_context(&tx, &record.id)?;
                        quarantine_invalid_effective_grant_tx(
                            &tx,
                            &record,
                            &operation_id,
                            edit_id.as_deref(),
                            &error.to_string(),
                            &now,
                        )?;
                        continue;
                    }
                    confirm_control_effect_tx(&tx, &record.id, "upsert", &now)?;
                    let active_status = if record.daily_rate_minor.is_some() {
                        SUB_ACTIVE_POSTPAID
                    } else {
                        SUB_ACTIVE_FREE
                    };
                    let app_scope_enforced_at = (grant.policy.allowed_apps
                        == market_allowed_apps())
                    .then_some(now.as_str());
                    tx.execute(
                        "UPDATE share_market_subscriptions
                         SET status = ?2, release_reason = NULL,
                             app_scope_enforced_at = COALESCE(app_scope_enforced_at, ?3),
                             updated_at = ?4
                         WHERE id = ?1",
                        params![record.id, active_status, app_scope_enforced_at, now],
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
                if let Some(grant) = entitlement_grant.as_ref() {
                    let applied_at_ms = match i64::try_from(grant.updated_at_ms) {
                        Ok(value) => value,
                        Err(_) => {
                            let (operation_id, edit_id) =
                                latest_upsert_control_context(&tx, &record.id)?;
                            quarantine_invalid_effective_grant_tx(
                                &tx,
                                &record,
                                &operation_id,
                                edit_id.as_deref(),
                                "managed grant applied time is outside the supported range",
                                &now,
                            )?;
                            continue;
                        }
                    };
                    let validation =
                        validate_managed_grant_applied_at(&record.created_at, applied_at_ms, &now)
                            .and_then(|_| {
                                validate_effective_initial_policy(
                                    &record,
                                    applied_at_ms,
                                    &grant.policy,
                                )
                            });
                    if let Err(error) = validation {
                        if matches!(&error, AppError::Internal(_)) {
                            return Err(error);
                        }
                        let (operation_id, edit_id) =
                            latest_upsert_control_context(&tx, &record.id)?;
                        quarantine_invalid_effective_grant_tx(
                            &tx,
                            &record,
                            &operation_id,
                            edit_id.as_deref(),
                            &error.to_string(),
                            &now,
                        )?;
                        continue;
                    }
                    activate_granted_subscription_tx(
                        &tx,
                        &record,
                        applied_at_ms,
                        &grant.policy,
                        &now,
                    )?;
                    persist_effective_control_policy(&tx, &record.id, &grant.policy)?;
                    confirm_control_effect_tx(&tx, &record.id, "upsert", &now)?;
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
                    finish_release_tx(
                        &tx,
                        &record.id,
                        &record.seat_id,
                        &record.listing_id,
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
                       AND op.dead_lettered_at IS NULL
                       AND COALESCE(op.next_attempt_at, op.created_at) <= ?2
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
                       AND NOT EXISTS (
                           SELECT 1
                           FROM share_edit_requests conflict_edit
                           JOIN shares current_share ON current_share.share_id = op.share_id
                           WHERE conflict_edit.id = op.edit_id
                             AND conflict_edit.status = 'rejected'
                             AND conflict_edit.error_code = ?3
                             AND (
                                 COALESCE(json_type(
                                     conflict_edit.patch_json,
                                     '$.managedGrant.expectedConfigRevision'
                                 ), '') != 'integer'
                                 OR CAST(json_extract(
                                     conflict_edit.patch_json,
                                     '$.managedGrant.expectedConfigRevision'
                                 ) AS INTEGER) >= current_share.config_revision
                             )
                       )
                     ORDER BY op.created_at, op.share_sequence",
                )
                .map_err(map_db("prepare Share control dispatch"))?;
            statement
                .query_map(
                    params![MAX_CONTROL_ATTEMPTS, now, SHARE_REVISION_CONFLICT_CODE],
                    |row| row.get::<_, String>(0),
                )
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
                    tx.execute(
                        "UPDATE share_control_operations
                         SET status = 'applied', applied_at = ?2, updated_at = ?2,
                             last_error = NULL
                         WHERE id = ?1 AND status = 'pending'",
                        params![operation_id, now],
                    )
                    .map_err(map_db("complete missing Share revoke operation"))?;
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
            let duration_seconds = if action == "upsert" {
                let timing: Option<(String, Option<i64>, Option<String>)> = tx
                    .query_row(
                        "SELECT status, service_duration_days, service_started_at
                         FROM share_market_subscriptions
                         WHERE id = ?1",
                        params![subscription_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .map_err(map_db("read initial Share grant timing"))?;
                timing
                    .filter(|(status, _, service_started_at)| {
                        status == SUB_GRANT_PENDING && service_started_at.is_none()
                    })
                    .and_then(|(_, days, _)| days)
                    .map(|days| {
                        u64::try_from(days)
                            .ok()
                            .and_then(|days| days.checked_mul(86_400))
                            .ok_or_else(|| {
                                AppError::Internal(
                                    "Share service duration cannot be represented in seconds"
                                        .into(),
                                )
                            })
                    })
                    .transpose()?
            } else {
                None
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
                    duration_seconds,
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
            let edit_expires_at = (now_dt + Duration::seconds(CONTROL_EDIT_TTL_SECS)).to_rfc3339();
            tx.execute(
                "INSERT INTO share_edit_requests (
                    id, share_id, installation_id, owner_email, revision, status,
                    patch_json, created_by_email, created_at, updated_at,
                    applied_at, error_message, retired_at, expires_at,
                    dead_lettered_at, error_code
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending',
                           ?6, ?4, ?7, ?7, NULL, NULL, NULL, ?8, NULL, NULL)",
                params![
                    edit_id,
                    share_id,
                    installation_id,
                    SHARE_MARKET_CONTROL_ACTOR_EMAIL,
                    edit_revision,
                    patch_json,
                    now,
                    edit_expires_at,
                ],
            )
            .map_err(map_db("dispatch Share control edit"))?;
            tx.execute(
                "UPDATE share_control_operations
                 SET status = 'dispatched', edit_id = ?2, attempts = attempts + 1,
                     next_attempt_at = NULL, last_error = NULL, updated_at = ?3
                 WHERE id = ?1 AND status = 'pending'",
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
                       AND (edit.expires_at IS NULL OR edit.expires_at > ?1)
                     ORDER BY op.updated_at, op.created_at",
                )
                .map_err(map_db("prepare dispatched Share control wake retries"))?;
            statement
                .query_map(params![now], |row| {
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

#[cfg(test)]
pub(crate) fn handle_control_edit_ack(
    conn: &Connection,
    edit_id: &str,
    status: &str,
    error_message: Option<&str>,
    now: &str,
) -> Result<(), AppError> {
    handle_control_edit_ack_with_metadata(conn, edit_id, status, error_message, None, None, now)
}

pub(crate) fn handle_control_edit_ack_with_metadata(
    conn: &Connection,
    edit_id: &str,
    status: &str,
    error_message: Option<&str>,
    error_code: Option<&str>,
    retryable: Option<bool>,
    now: &str,
) -> Result<(), AppError> {
    handle_control_edit_ack_with_effect(
        conn,
        edit_id,
        status,
        error_message,
        error_code,
        retryable,
        None,
        None,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_control_edit_ack_with_effect(
    conn: &Connection,
    edit_id: &str,
    status: &str,
    error_message: Option<&str>,
    error_code: Option<&str>,
    retryable: Option<bool>,
    applied_at_ms: Option<i64>,
    effective_policy: Option<&ShareUserPolicy>,
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
             SET retired_at = COALESCE(retired_at, ?2), error_code = COALESCE(?3, error_code)
             WHERE id = ?1 AND status = 'rejected'",
            params![edit_id, now, error_code],
        )
        .map_err(map_db("retire rejected Share control edit"))?;
    }
    if operation_status != "dispatched" {
        return Ok(());
    }
    if status == "applied" {
        let stored_policy = policy_json
            .as_deref()
            .map(serde_json::from_str::<ShareUserPolicy>)
            .transpose()
            .map_err(|_| AppError::Internal("stored Share grant policy is invalid".into()))?;
        let applied_at_ms = applied_at_ms.unwrap_or(parse_time(now)?.timestamp_millis());
        let upsert_record = if action == "upsert" {
            subscription_record(conn, &subscription_id)?
        } else {
            None
        };
        let pending_record = upsert_record
            .as_ref()
            .filter(|record| record.status == SUB_GRANT_PENDING);
        let resume_record = upsert_record
            .as_ref()
            .filter(|record| record.status == SUB_BILLING_RESUME_PENDING);
        if let Some(record) = pending_record.or(resume_record)
            && let Err(error) =
                validate_managed_grant_applied_at(&record.created_at, applied_at_ms, now)
        {
            if matches!(&error, AppError::Internal(_)) {
                return Err(error);
            }
            quarantine_invalid_effective_grant_tx(
                conn,
                record,
                &operation_id,
                Some(edit_id),
                &error.to_string(),
                now,
            )?;
            return Ok(());
        }
        let materialized_policy = if effective_policy.is_none() {
            pending_record
                .zip(stored_policy.as_ref())
                .map(|(record, policy)| materialize_initial_policy(record, applied_at_ms, policy))
                .transpose()?
        } else {
            None
        };
        let policy = effective_policy
            .or(materialized_policy.as_ref())
            .or(stored_policy.as_ref());
        if let Some(record) = pending_record {
            let validation = policy.map_or_else(
                || {
                    Err(AppError::BadRequest(
                        "applied managed grant ACK is missing effectivePolicy".into(),
                    ))
                },
                |policy| validate_effective_initial_policy(record, applied_at_ms, policy),
            );
            if let Err(error) = validation {
                if matches!(&error, AppError::Internal(_)) {
                    return Err(error);
                }
                quarantine_invalid_effective_grant_tx(
                    conn,
                    record,
                    &operation_id,
                    Some(edit_id),
                    &error.to_string(),
                    now,
                )?;
                return Ok(());
            }
        }
        if let Some(record) = resume_record {
            let validation = policy.map_or_else(
                || {
                    Err(AppError::BadRequest(
                        "applied billing resume ACK is missing effectivePolicy".into(),
                    ))
                },
                |policy| validate_effective_resume_policy(record, policy),
            );
            if let Err(error) = validation {
                if matches!(&error, AppError::Internal(_)) {
                    return Err(error);
                }
                quarantine_invalid_effective_grant_tx(
                    conn,
                    record,
                    &operation_id,
                    Some(edit_id),
                    &error.to_string(),
                    now,
                )?;
                return Ok(());
            }
        }
        if action == "upsert"
            && let Some(policy) = policy
        {
            persist_effective_control_policy(conn, &subscription_id, policy)?;
        }
        apply_control_grant_effect(
            conn,
            &share_id,
            &action,
            &email,
            &entitlement_id,
            policy,
            Some(applied_at_ms),
            now,
        )?;
        if let Some(record) = upsert_record.filter(|record| record.status == SUB_GRANT_PENDING) {
            let policy = policy.ok_or_else(|| {
                AppError::BadRequest("applied managed grant ACK is missing effectivePolicy".into())
            })?;
            activate_granted_subscription_tx(conn, &record, applied_at_ms, policy, now)?;
        }
        conn.execute(
            "UPDATE share_control_operations
             SET status = 'applied', updated_at = ?2, applied_at = ?2, last_error = NULL
             WHERE id = ?1 AND status = 'dispatched'",
            params![operation_id, now],
        )
        .map_err(map_db("complete Share control operation"))?;
        return Ok(());
    }
    let retry_requested = retryable == Some(true);
    let explicitly_nonretryable = retryable == Some(false);
    let should_retry = retry_requested && attempts < MAX_CONTROL_ATTEMPTS;
    let revision_conflict = error_code == Some(SHARE_REVISION_CONFLICT_CODE);
    let sanitized_error = error_message.map(crate::store::client_chat::sanitize_system_event_text);
    let error_message = sanitized_error.as_deref();
    if should_retry {
        if revision_conflict {
            conn.execute(
                "UPDATE share_control_operations
                 SET status = 'pending', updated_at = ?2, last_error = ?3,
                     next_attempt_at = ?2
                 WHERE id = ?1 AND status = 'dispatched'",
                params![operation_id, now, error_message],
            )
            .map_err(map_db(
                "wait for Share revision refresh before control retry",
            ))?;
        } else {
            let next_attempt_at = control_retry_at(now, attempts)?;
            conn.execute(
                "UPDATE share_control_operations
                 SET status = 'pending', edit_id = NULL, updated_at = ?2, last_error = ?3,
                     next_attempt_at = ?4
                 WHERE id = ?1 AND status = 'dispatched'",
                params![operation_id, now, error_message, next_attempt_at],
            )
            .map_err(map_db("retry Share control operation"))?;
        }
        return Ok(());
    }
    let dead_letter =
        action == "revoke" && (explicitly_nonretryable || attempts >= MAX_CONTROL_ATTEMPTS);
    conn.execute(
        "UPDATE share_control_operations
         SET status = 'rejected', updated_at = ?2, last_error = ?3,
             dead_lettered_at = CASE WHEN ?4 THEN ?2 ELSE dead_lettered_at END,
             next_attempt_at = NULL
         WHERE id = ?1 AND status = 'dispatched'",
        params![operation_id, now, error_message, dead_letter],
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
    if dead_letter {
        conn.execute(
            "UPDATE share_edit_requests SET dead_lettered_at = ?2 WHERE id = ?1",
            params![edit_id, now],
        )
        .map_err(map_db("dead-letter Share control edit"))?;
        let occurred_at = parse_time(now)?;
        crate::store::enqueue_operator_alert_signal_tx(
            conn,
            &format!("share-control-dead-letter:{operation_id}"),
            &format!("share_control_dead_letter:share:{share_id}"),
            "firing",
            "share_control_dead_letter",
            "share",
            Some(&share_id),
            if action == "revoke" {
                "critical"
            } else {
                "warning"
            },
            "Share control operation exhausted retries",
            &format!(
                "Share {share_id} {action} control operation failed after {attempts} attempts."
            ),
            serde_json::json!({
                "operationId": operation_id,
                "subscriptionId": subscription_id,
                "shareId": share_id,
                "action": action,
                "attempts": attempts,
                "errorCode": error_code,
                "error": error_message,
            }),
            occurred_at,
        )?;
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
    applied_at_ms: Option<i64>,
    now: &str,
) -> Result<(), AppError> {
    let grants_json: Option<String> = conn
        .query_row(
            "SELECT COALESCE(user_grants_json, '{}')
             FROM shares WHERE share_id = ?1",
            params![share_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_db("read Share grants for control effect"))?;
    let Some(grants_json) = grants_json else {
        return Ok(());
    };
    let mut grants: BTreeMap<String, ShareUserGrant> = serde_json::from_str(&grants_json)
        .map_err(|error| AppError::Internal(format!("stored Share grants are invalid: {error}")))?;
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() {
        return Ok(());
    }
    let now_ms = applied_at_ms
        .unwrap_or_else(|| Utc::now().timestamp_millis())
        .max(0) as u128;
    match action {
        "upsert" => {
            let Some(policy) = policy.cloned() else {
                return Err(AppError::Internal(
                    "Share Market grant upsert is missing policy".into(),
                ));
            };
            let previous = grants.get(&email).cloned();
            let usage_rebase = previous
                .as_ref()
                .and_then(|grant| grant.usage_rebase.clone())
                .filter(|rebase| {
                    rebase.period == policy.token_period
                        && rebase.anchor_at_ms == policy.token_period_anchor_at_ms
                });
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
                    usage_rebase,
                    usage_quota: previous.as_ref().and_then(|grant| grant.usage_quota),
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
        }
        _ => return Ok(()),
    }
    let grants_json = serde_json::to_string(&grants).map_err(|error| {
        AppError::Internal(format!(
            "encode Share grants after control effect failed: {error}"
        ))
    })?;
    conn.execute(
        "UPDATE shares
         SET user_grants_json = ?2, shared_with_emails_json = '[]', updated_at = ?3
         WHERE share_id = ?1",
        params![share_id, grants_json, now],
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
        let conn = state.store.conn.lock().await;
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
    Ok(())
}

pub async fn resume_after_billing(
    state: &ServerState,
    subscription_id: &str,
) -> Result<(), AppError> {
    let now_dt = Utc::now();
    let now = now_dt.to_rfc3339();
    {
        let conn = state.store.conn.lock().await;
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
                     ORDER BY operation.created_at DESC LIMIT 1),
                    sub.expires_at
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
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Share subscription for billing resume"))?
            .ok_or_else(|| AppError::NotFound("Share subscription not found".into()))?;
        let expired_at = row
            .7
            .as_deref()
            .map(parse_time)
            .transpose()?
            .filter(|expires_at| *expires_at <= now_dt);
        if let Some(expires_at) = expired_at {
            let record = subscription_record(&tx, subscription_id)?
                .ok_or_else(|| AppError::NotFound("Share subscription not found".into()))?;
            let grants_json = tx
                .query_row(
                    "SELECT user_grants_json FROM shares WHERE share_id = ?1",
                    params![record.share_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(map_db("read Share grants during expired billing resume"))?
                .flatten();
            expire_service_term_tx(
                &tx,
                &record,
                grants_json.as_deref(),
                &expires_at.to_rfc3339(),
                &now,
            )?;
            tx.commit()
                .map_err(map_db("commit expired Share billing resume"))?;
        } else {
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
            let record = subscription_record(&tx, subscription_id)?
                .ok_or_else(|| AppError::NotFound("Share subscription not found".into()))?;
            let policy = contract_policy(&record)?;
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
    }
    Ok(())
}

pub async fn terminate_for_billing(
    state: &ServerState,
    subscription_id: &str,
    reason: &str,
) -> Result<(), AppError> {
    let now = Utc::now().to_rfc3339();
    {
        let conn = state.store.conn.lock().await;
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
            let message = error.client_message();
            let error_code = error.error_code().map(str::to_string);
            let retryable = error.retryable();
            let current_config_revision = error.current_config_revision();
            let current_share = error.current_share().cloned();
            tracing::warn!(
                share_id = %event.share_id,
                edit_id = %edit.id,
                error = %message,
                "Share Market control RPC rejected managed grant"
            );
            state
                .store
                .mark_share_edit_rejected_with_metadata(
                    &edit.id,
                    &message,
                    error_code.as_deref(),
                    Some(retryable),
                    current_config_revision,
                    current_share.as_ref(),
                )
                .await?;
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

    #[tokio::test]
    async fn paid_share_requires_payment_profile_with_stable_error_code() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let conn = store.conn.lock().await;
        let tx = conn.transaction().expect("start payment profile check");
        let error = ensure_payment_profile_tx(&tx, "missing-owner")
            .expect_err("missing payment profile must block a paid Share offer");
        assert_eq!(
            error.code(),
            Some(ERROR_SHARE_MARKET_PAYMENT_PROFILE_REQUIRED)
        );
    }

    #[test]
    fn private_etag_json_revalidates_matching_strong_and_weak_tags() {
        let response = private_etag_json(&HeaderMap::new(), &serde_json::json!({ "value": 1 }))
            .expect("build initial ETag response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-cache"
        );
        let etag = response.headers().get(header::ETAG).unwrap().clone();

        for validator in [
            etag.to_str().unwrap().to_string(),
            format!("W/{}", etag.to_str().unwrap()),
            format!("\"unrelated\", {}", etag.to_str().unwrap()),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::IF_NONE_MATCH,
                validator.parse().expect("valid If-None-Match"),
            );
            let response = private_etag_json(&headers, &serde_json::json!({ "value": 1 }))
                .expect("build revalidated ETag response");
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
            assert_eq!(response.headers().get(header::ETAG), Some(&etag));
        }

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, "\"unrelated\"".parse().unwrap());
        let response = private_etag_json(&headers, &serde_json::json!({ "value": 2 }))
            .expect("build changed ETag response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_ne!(response.headers().get(header::ETAG), Some(&etag));
    }

    fn session(user_id: &str, email: &str) -> AuthSession {
        let now = Utc::now();
        AuthSession {
            session_id: format!("session-{user_id}"),
            user_id: user_id.to_string(),
            email: email.to_ascii_lowercase(),
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

    fn free_seat() -> SeatInput {
        SeatInput {
            parallel_limit: Some(2),
            token_limit: Some(10_000),
            token_period: ShareTokenPeriod::Day,
            daily_rate_minor: None,
            currency: None,
            service_duration_days: Some(1),
        }
    }

    fn paid_seat() -> SeatInput {
        SeatInput {
            daily_rate_minor: Some(1_200),
            currency: Some("USD".into()),
            service_duration_days: None,
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
                    app_type, enabled_codex, token_limit, parallel_limit, tokens_used, requests_count,
                    share_status, created_at, expires_at, user_grants_json, bindings_json,
                    supported_user_token_periods_json, config_revision, updated_at,
                    share_access_policy_version, runtime_refreshed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'codex', 1, -1, 3, 0, 0,
                           'active', ?7, '9999-12-31T23:59:59Z', '{}',
                           '{\"codex\":\"provider-test\"}', ?8, 1, ?7, ?9, ?7)",
                params![
                    share_id,
                    format!("pool-{share_id}"),
                    format!("installation-{share_id}"),
                    format!("Share {share_id}"),
                    owner_email,
                    format!("{share_id}-route"),
                    now,
                    periods,
                    i64::from(SHARE_CONTRACT_VERSION),
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
             ) VALUES (?1, ?2, 'USD', 24, 1, ?3, ?3)
             ON CONFLICT(supplier_user_id, currency) DO UPDATE SET
                supplier_email = excluded.supplier_email,
                settlement_grace_hours = excluded.settlement_grace_hours,
                revision = supplier_billing_profiles.revision + 1,
                updated_at = excluded.updated_at",
            params![owner.user_id, owner.email, updated_at],
        )
        .expect("configure USD payment grace");
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
            crate::market_access::configure_open_test_policy(&conn, owner, "USD", 50_000, &now);
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

    fn reopen_existing_request(
        seat_id: &str,
        offer_revision: i64,
        seat: SeatInput,
    ) -> ReopenListingRequest {
        ReopenListingRequest {
            existing_seats: vec![ReopenExistingSeatRequest {
                seat_id: seat_id.into(),
                offer_revision,
                seat,
            }],
            new_seats: Vec::new(),
        }
    }

    async fn stopped_listing_state(
        store: &AppStore,
        listing_id: &str,
        seat_id: &str,
    ) -> (String, String, i64) {
        store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT listing.status, seat.status, seat.offer_revision
                 FROM share_market_listings listing
                 JOIN share_market_seats seat ON seat.listing_id = listing.id
                 WHERE listing.id = ?1 AND seat.id = ?2",
                params![listing_id, seat_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read stopped listing state")
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

    async fn set_entitlement(store: &AppStore, subscription_id: &str, applied_at: DateTime<Utc>) {
        let (record, stored_policy) = {
            let conn = store.conn.lock().await;
            let record = subscription_record(&conn, subscription_id)
                .expect("read subscription")
                .expect("subscription");
            let policy_json: String = conn
                .query_row(
                    "SELECT policy_json FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = 'upsert'
                     ORDER BY share_sequence DESC LIMIT 1",
                    params![subscription_id],
                    |row| row.get(0),
                )
                .expect("read managed grant policy");
            let policy = serde_json::from_str(&policy_json).expect("decode managed grant policy");
            (record, policy)
        };
        let applied_at_ms = applied_at.timestamp_millis();
        let policy = materialize_initial_policy(&record, applied_at_ms, &stored_policy)
            .expect("materialize managed grant policy");
        let grant = ShareUserGrant {
            email: record.renter_email.clone(),
            role: "shareto".into(),
            active: true,
            policy,
            usage: Default::default(),
            usage_rebase: None,
            usage_quota: None,
            created_at_ms: applied_at_ms.max(0) as u128,
            updated_at_ms: applied_at_ms.max(0) as u128,
            revoked_at_ms: None,
            revision: 1,
            manager: ShareGrantManager::RouterShareMarket,
            entitlement_id: Some(record.entitlement_id),
        };
        let grants = serde_json::to_string(&BTreeMap::from([(record.renter_email, grant)]))
            .expect("serialize managed grant");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET user_grants_json = ?2,
                    config_revision = config_revision + 1, updated_at = ?3
                 WHERE share_id = ?1",
                params![record.share_id, grants, applied_at.to_rfc3339()],
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
        if dispatched.len() != 1 {
            let state: (String, String, Option<String>, String, i64) = store
                .conn
                .lock()
                .await
                .query_row(
                    "SELECT subscription.status, operation.status, operation.next_attempt_at,
                            share.share_status,
                            (SELECT COUNT(*) FROM share_edit_requests edit
                             WHERE edit.share_id = subscription.share_id
                               AND edit.status = 'pending' AND edit.retired_at IS NULL)
                     FROM share_market_subscriptions subscription
                     JOIN share_control_operations operation
                       ON operation.subscription_id = subscription.id
                      AND operation.action = 'upsert'
                     JOIN shares share ON share.share_id = subscription.share_id
                     WHERE subscription.id = ?1
                     ORDER BY operation.share_sequence DESC LIMIT 1",
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
                .expect("read failed test grant dispatch state");
            panic!(
                "expected one managed grant dispatch, got {}; state: {state:?}",
                dispatched.len()
            );
        }
        set_entitlement(store, subscription_id, now).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(now)
                .await
                .expect("confirm managed grant")
                .is_empty()
        );
    }

    async fn record_share_health(store: &AppStore, share_id: &str, now: DateTime<Utc>) {
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO share_health_checks (
                    share_id, checked_at, is_healthy, status, reason, router_epoch
                 ) VALUES (?1, ?2, 1, 'healthy', 'test_observation', 'test')",
            params![share_id, now.timestamp()],
        )
        .expect("insert Share health observation");
        conn.execute(
            "UPDATE shares SET runtime_refreshed_at = ?2 WHERE share_id = ?1",
            params![share_id, now.to_rfc3339()],
        )
        .expect("refresh Share runtime observation");
    }

    async fn set_entitlement_from_stored_policy(
        store: &AppStore,
        subscription_id: &str,
        applied_at: DateTime<Utc>,
    ) {
        let (record, policy) = {
            let conn = store.conn.lock().await;
            let record = subscription_record(&conn, subscription_id)
                .expect("read subscription")
                .expect("subscription");
            let policy_json: String = conn
                .query_row(
                    "SELECT policy_json FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = 'upsert'
                     ORDER BY share_sequence DESC LIMIT 1",
                    params![subscription_id],
                    |row| row.get(0),
                )
                .expect("read stored managed grant policy");
            let policy = serde_json::from_str(&policy_json).expect("decode stored grant policy");
            (record, policy)
        };
        let applied_at_ms = applied_at.timestamp_millis();
        let grant = ShareUserGrant {
            email: record.renter_email.clone(),
            role: "shareto".into(),
            active: true,
            policy,
            created_at_ms: applied_at_ms.max(0) as u128,
            updated_at_ms: applied_at_ms.max(0) as u128,
            revision: 2,
            manager: ShareGrantManager::RouterShareMarket,
            entitlement_id: Some(record.entitlement_id),
            ..ShareUserGrant::default()
        };
        let grants = serde_json::to_string(&BTreeMap::from([(record.renter_email, grant)]))
            .expect("serialize restored managed grant");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET user_grants_json = ?2,
                    config_revision = config_revision + 1, updated_at = ?3
                 WHERE share_id = ?1",
                params![record.share_id, grants, applied_at.to_rfc3339()],
            )
            .expect("publish restored managed grant descriptor");
    }

    async fn exhaust_control_operation(
        store: &AppStore,
        subscription_id: &str,
        action: &str,
        mut dispatched_at: DateTime<Utc>,
    ) -> (String, Vec<String>) {
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(dispatched_at)
                .await
                .expect("dispatch control operation")
                .len(),
            1
        );
        let mut edit_ids = Vec::new();
        for attempt in 1..=MAX_CONTROL_ATTEMPTS {
            let (operation_id, edit_id): (String, String) = store
                .conn
                .lock()
                .await
                .query_row(
                    "SELECT id, edit_id FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = ?2
                     ORDER BY share_sequence DESC LIMIT 1",
                    params![subscription_id, action],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("read dispatched control operation");
            edit_ids.push(edit_id.clone());
            let rejected_at = dispatched_at + Duration::seconds(1);
            {
                let conn = store.conn.lock().await;
                conn.execute(
                    "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                    params![edit_id],
                )
                .expect("reject control edit");
                handle_control_edit_ack_with_metadata(
                    &conn,
                    edit_ids.last().expect("control edit id"),
                    "rejected",
                    Some("temporary control failure"),
                    Some("temporary_control_failure"),
                    Some(true),
                    &rejected_at.to_rfc3339(),
                )
                .expect("record retryable control rejection");
            }
            if attempt == MAX_CONTROL_ATTEMPTS {
                return (operation_id, edit_ids);
            }
            let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(0);
            let delay = CONTROL_RETRY_BASE_SECS
                .saturating_mul(2_i64.saturating_pow(exponent))
                .min(CONTROL_RETRY_MAX_SECS);
            dispatched_at = rejected_at + Duration::seconds(delay);
            assert_eq!(
                store
                    .share_market_reconcile_and_dispatch(dispatched_at)
                    .await
                    .expect("dispatch retried control operation")
                    .len(),
                1
            );
        }
        unreachable!("control operation attempts are nonzero")
    }

    async fn create_active_paid_fixed_subscription(
        store: &AppStore,
        owner: &AuthSession,
        renter: &AuthSession,
        share_id: &str,
    ) -> (String, String, DateTime<Utc>) {
        configure_payment_profile(store, owner, "payment-account", "profile-v1").await;
        insert_share(store, share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
        let mut seat = paid_seat();
        seat.service_duration_days = Some(10);
        let (_, seat_id) = create_listing(store, owner, share_id, seat).await;
        let subscription_id = store
            .share_market_rent_seat(renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent fixed paid seat");
        let activated_at = Utc::now() + Duration::seconds(1);
        record_share_health(store, share_id, activated_at).await;
        activate_subscription(store, &subscription_id, activated_at).await;
        (subscription_id, seat_id, activated_at)
    }

    #[tokio::test]
    async fn near_expiry_is_degraded_but_stale_runtime_is_blocking() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        insert_share(
            &store,
            "share-availability-policy",
            "owner@example.com",
            &[ShareTokenPeriod::Day],
        )
        .await;
        let now = Utc::now();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET expires_at = ?2, runtime_refreshed_at = ?3
                 WHERE share_id = ?1",
                params![
                    "share-availability-policy",
                    (now + Duration::days(3)).to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )
            .expect("configure near-expiry Share");
            let availability = evaluate_share_market_service_availability_tx(
                &conn,
                "share-availability-policy",
                "codex",
                Some(true),
                Some("compatible"),
                None,
                now,
            )
            .expect("evaluate near-expiry Share");
            assert!(availability.available());
            assert_eq!(availability.state, "degraded");
            assert!(
                availability
                    .reasons
                    .contains(&"share_near_expiry".to_string())
            );

            conn.execute(
                "UPDATE shares SET runtime_refreshed_at = ?2 WHERE share_id = ?1",
                params![
                    "share-availability-policy",
                    (now - Duration::minutes(16)).to_rfc3339(),
                ],
            )
            .expect("make Share runtime stale");
            let availability = evaluate_share_market_service_availability_tx(
                &conn,
                "share-availability-policy",
                "codex",
                Some(true),
                Some("compatible"),
                None,
                now,
            )
            .expect("evaluate stale Share runtime");
            assert!(!availability.available());
            assert_eq!(
                availability.blocking_reason.as_deref(),
                Some("runtime_stale")
            );
        }
    }

    fn insert_performance_sample(
        conn: &Connection,
        request_id: &str,
        share_id: &str,
        created_at: i64,
        status_code: i64,
        latency_ms: i64,
        first_token_ms: Option<i64>,
        output_tokens: i64,
        usage_state: &str,
        is_streaming: bool,
        stream_status: Option<&str>,
        is_health_check: bool,
    ) {
        conn.execute(
            "INSERT INTO share_request_logs (
                request_id, installation_id, share_id, share_name, provider_id,
                provider_name, app_type, model, request_model, usage_state,
                stream_status, status_code, latency_ms, first_token_ms,
                input_tokens, output_tokens, cache_read_tokens,
                cache_creation_tokens, is_streaming, is_health_check, created_at
             ) VALUES (
                ?1, 'installation-performance', ?2, 'Performance Share',
                'provider-performance', 'Performance Provider', 'codex', 'gpt-test',
                'gpt-test', ?3, ?4, ?5, ?6, ?7, 10, ?8, 0, 0, ?9, ?10, ?11
             )",
            params![
                request_id,
                share_id,
                usage_state,
                stream_status,
                status_code,
                latency_ms,
                first_token_ms,
                output_tokens,
                i64::from(is_streaming as u8),
                i64::from(is_health_check as u8),
                created_at,
            ],
        )
        .expect("insert Share Market performance sample");
    }

    #[test]
    fn public_capabilities_cover_all_bound_apps_and_provider_families() {
        let bindings = serde_json::json!({
            "claude": "anthropic-provider",
            "codex": "openai-provider",
            "gemini": "google-provider",
        })
        .to_string();
        let providers = crate::models::ShareAppProviders {
            claude: vec![crate::models::ShareAppProvider {
                id: "anthropic-provider".into(),
                name: "Anthropic Official".into(),
                app: "claude".into(),
                kind: Some("official_oauth".into()),
                enabled: true,
                model_policy: Some(crate::models::ShareProviderModelPolicy::Single {
                    upstream_model: "claude-opus-test".into(),
                }),
                models: vec![crate::models::ShareUpstreamModel {
                    slot: "default".into(),
                    actual_model: "claude-opus-test".into(),
                }],
                ..Default::default()
            }],
            codex: vec![crate::models::ShareAppProvider {
                id: "openai-provider".into(),
                name: "OpenAI Official".into(),
                app: "codex".into(),
                kind: Some("official_oauth".into()),
                enabled: true,
                model_policy: Some(crate::models::ShareProviderModelPolicy::Passthrough),
                ..Default::default()
            }],
            gemini: vec![crate::models::ShareAppProvider {
                id: "google-provider".into(),
                name: "Google Gemini".into(),
                app: "gemini".into(),
                kind: Some("official_oauth".into()),
                enabled: true,
                ..Default::default()
            }],
        };
        let providers_json = serde_json::to_string(&providers).expect("encode providers");

        let capabilities = public_app_capabilities(&bindings, None, Some(&providers_json), "codex");
        assert_eq!(
            capabilities
                .iter()
                .map(|capability| capability.app.as_str())
                .collect::<Vec<_>>(),
            ["claude", "codex", "gemini"]
        );
        assert_eq!(capabilities[0].provider_family, "anthropic");
        assert_eq!(capabilities[0].model_mode, "fixed");
        assert_eq!(
            capabilities[0].upstream_model.as_deref(),
            Some("claude-opus-test")
        );
        assert_eq!(capabilities[1].provider_family, "openai");
        assert_eq!(capabilities[1].model_mode, "passthrough");
        assert_eq!(capabilities[2].provider_family, "google");
        assert_eq!(
            listing_provider_families(&capabilities),
            (
                "multi".into(),
                vec!["anthropic".into(), "google".into(), "openai".into()]
            )
        );
    }

    #[test]
    fn market_capabilities_include_only_enabled_share_app_apis() {
        let bindings = serde_json::json!({
            "claude": "grok-provider",
            "codex": "grok-provider",
            "gemini": "grok-provider",
        })
        .to_string();
        let support = ShareSupport {
            codex: true,
            ..Default::default()
        };

        assert_eq!(
            enabled_share_apps(&bindings, "codex", &support),
            vec!["codex"]
        );
        assert_eq!(
            enabled_public_app_capabilities(&bindings, None, None, "codex", &support)
                .into_iter()
                .map(|capability| capability.app)
                .collect::<Vec<_>>(),
            vec!["codex"]
        );
    }

    #[test]
    fn public_capabilities_fall_back_to_bound_provider_without_leaking_secrets() {
        let bindings = serde_json::json!({ "codex": "provider-secret" }).to_string();
        let runtimes = crate::models::ShareAppRuntimes {
            codex: Some(crate::models::ShareUpstreamProvider {
                app: "codex".into(),
                kind: String::new(),
                models: Vec::new(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let providers = crate::models::ShareAppProviders {
            codex: vec![crate::models::ShareAppProvider {
                id: "provider-secret".into(),
                name: "OpenAI Official".into(),
                app: "codex".into(),
                kind: Some("official_oauth".into()),
                provider_type: Some("codex_oauth".into()),
                enabled: true,
                account_email: Some("secret-account@example.com".into()),
                api_url: Some("https://secret.invalid/v1".into()),
                subscription_level: Some("Pro".into()),
                model_policy: Some(crate::models::ShareProviderModelPolicy::Single {
                    upstream_model: "gpt-secret".into(),
                }),
                models: vec![crate::models::ShareUpstreamModel {
                    slot: "default".into(),
                    actual_model: "gpt-secret".into(),
                }],
                available: Some(true),
                ..Default::default()
            }],
            ..Default::default()
        };
        let runtimes_json = serde_json::to_string(&runtimes).expect("encode runtimes");
        let providers_json = serde_json::to_string(&providers).expect("encode providers");

        let capabilities = public_app_capabilities(
            &bindings,
            Some(&runtimes_json),
            Some(&providers_json),
            "codex",
        );
        assert_eq!(capabilities.len(), 1);
        let capability = &capabilities[0];
        assert_eq!(capability.provider_family, "openai");
        assert_eq!(capability.provider_name.as_deref(), Some("OpenAI Official"));
        assert_eq!(capability.provider_type.as_deref(), Some("codex_oauth"));
        assert_eq!(capability.subscription_level.as_deref(), Some("Pro"));
        assert_eq!(capability.model_mode, "fixed");
        assert_eq!(capability.models, ["gpt-secret"]);
        assert_eq!(capability.available, Some(true));
        assert_eq!(capability.health_state, "healthy");
        assert_eq!(
            capability.account_hint.as_deref(),
            Some("secret-account@example.com")
        );

        let public_json = serde_json::to_string(&capabilities).expect("encode public capability");
        assert!(public_json.contains("secret-account@example.com"));
        assert!(!public_json.contains("https://secret.invalid/v1"));
        assert!(!public_json.contains("accountEmail"));
        assert!(!public_json.contains("apiUrl"));
    }

    #[test]
    fn public_capabilities_include_structured_quota_and_full_account_email() {
        let bindings = serde_json::json!({ "codex": "openai-provider" }).to_string();
        let runtimes_json = serde_json::json!({
            "codex": {
                "kind": "official_oauth",
                "app": "codex",
                "providerName": "OpenAI Official",
                "providerType": "codex_oauth",
                "accountEmail": "private@example.com",
                "available": true,
                "quota": {
                    "status": "ok",
                    "plan": "Plus",
                    "subscriptionPeriodEnd": "2030-01-01T00:00:00Z",
                    "tiers": [{
                        "label": "weekly",
                        "utilization": 0.55,
                        "resetsAt": "2030-01-02T00:00:00Z",
                        "used": 55.0,
                        "limit": 100.0,
                        "unit": "requests"
                    }]
                }
            }
        })
        .to_string();

        let capabilities = public_app_capabilities(&bindings, Some(&runtimes_json), None, "codex");
        let capability = &capabilities[0];
        let quota = capability.quota.as_ref().expect("public quota");
        assert_eq!(
            capability.account_hint.as_deref(),
            Some("private@example.com")
        );
        assert_eq!(capability.health_state, "healthy");
        assert_eq!(quota.status.as_deref(), Some("ok"));
        assert_eq!(quota.plan.as_deref(), Some("Plus"));
        assert_eq!(quota.tiers[0].label, "weekly");
        assert_eq!(quota.tiers[0].utilization, 0.55);

        let public_json = serde_json::to_string(capability).expect("encode public capability");
        assert!(public_json.contains("private@example.com"));
        assert!(!public_json.contains("accountEmail"));
        assert!(!public_json.contains("apiUrl"));
    }

    #[test]
    fn public_capabilities_do_not_substitute_an_unbound_provider() {
        let bindings = serde_json::json!({ "codex": "missing-provider" }).to_string();
        let runtimes = crate::models::ShareAppRuntimes {
            codex: Some(crate::models::ShareUpstreamProvider {
                app: "codex".into(),
                kind: "codex_oauth".into(),
                provider_name: Some("Wrong runtime".into()),
                available: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let providers = crate::models::ShareAppProviders {
            codex: vec![crate::models::ShareAppProvider {
                id: "other-provider".into(),
                name: "Wrong fallback".into(),
                app: "codex".into(),
                kind: Some("codex_oauth".into()),
                is_current: true,
                enabled: true,
                available: Some(true),
                ..Default::default()
            }],
            ..Default::default()
        };
        let runtimes_json = serde_json::to_string(&runtimes).expect("encode runtimes");
        let providers_json = serde_json::to_string(&providers).expect("encode providers");

        let capabilities = public_app_capabilities(
            &bindings,
            Some(&runtimes_json),
            Some(&providers_json),
            "codex",
        );
        let capability = &capabilities[0];
        assert_eq!(capability.provider_family, "other");
        assert_eq!(capability.provider_name, None);
        assert_eq!(capability.available, Some(false));
        assert_eq!(capability.health_state, "unavailable");
        assert_eq!(capability.model_mode, "unknown");
    }

    #[test]
    fn required_app_hard_failures_are_narrow_and_explicit() {
        let bindings = serde_json::json!({ "codex": "provider-1" }).to_string();
        let provider = crate::models::ShareAppProvider {
            id: "provider-1".into(),
            name: "OpenAI Official".into(),
            app: "codex".into(),
            kind: Some("official_oauth".into()),
            enabled: true,
            available: Some(false),
            subscription_remaining_ms: Some(2 * 86_400_000),
            health: Some(crate::models::ShareProviderHealth {
                healthy: false,
                requests: 10,
                successes: 9,
                failures: 1,
                last_status_code: Some(500),
                reason: Some("latency warning threshold exceeded".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let providers_json = |provider| {
            serde_json::to_string(&crate::models::ShareAppProviders {
                codex: vec![provider],
                ..Default::default()
            })
            .expect("encode app providers")
        };
        let degraded_json = providers_json(provider.clone());

        assert_eq!(
            required_app_hard_failure("codex", &bindings, None, Some(&degraded_json)),
            None
        );
        assert!(
            build_rent_service_snapshot(
                &bindings,
                None,
                Some(&degraded_json),
                "codex",
                &ShareSupport {
                    codex: true,
                    ..Default::default()
                },
                3,
                -1,
                0,
            )
            .is_ok(),
            "latency, ordinary health degradation, availability, and near-expiry signals must not block a quote"
        );
        assert_eq!(
            required_app_hard_failure("codex", "{", None, None),
            Some("binding_unresolved")
        );
        assert_eq!(
            required_app_hard_failure("codex", "{}", None, None),
            Some("binding_missing")
        );
        assert_eq!(
            required_app_hard_failure(
                "codex",
                r#"{"codex":"unknown"}"#,
                None,
                Some(&degraded_json),
            ),
            Some("binding_unresolved")
        );

        let mut disabled = provider.clone();
        disabled.enabled = false;
        assert_eq!(
            required_app_hard_failure("codex", &bindings, None, Some(&providers_json(disabled)),),
            Some("provider_disabled")
        );
        let mut blocked = provider.clone();
        blocked.quota_blocked = Some(true);
        assert_eq!(
            required_app_hard_failure("codex", &bindings, None, Some(&providers_json(blocked)),),
            Some("quota_blocked")
        );
        let mut unauthorized = provider;
        unauthorized.health.as_mut().unwrap().last_status_code = Some(401);
        assert_eq!(
            required_app_hard_failure(
                "codex",
                &bindings,
                None,
                Some(&providers_json(unauthorized)),
            ),
            Some("authentication_failed")
        );
    }

    #[test]
    fn public_provider_health_distinguishes_degraded_from_unavailable() {
        assert_eq!(
            public_provider_health_state(None, None, Some(false), Some(true), false),
            "degraded"
        );
        assert_eq!(
            public_provider_health_state(Some(false), None, Some(false), Some(true), false),
            "unavailable"
        );
        assert_eq!(
            public_provider_health_state(Some(true), None, None, None, false),
            "healthy"
        );
        assert_eq!(
            public_provider_health_state(None, None, None, None, false),
            "unknown"
        );
        assert_eq!(
            public_provider_health_state(None, None, None, Some(true), false),
            "unknown"
        );
        assert_eq!(
            public_provider_health_state(Some(true), None, Some(true), Some(true), true),
            "unavailable"
        );
    }

    #[test]
    fn unknown_provider_identity_does_not_fall_back_to_app_family() {
        let bindings = serde_json::json!({ "codex": "unknown-provider" }).to_string();
        let providers = crate::models::ShareAppProviders {
            codex: vec![crate::models::ShareAppProvider {
                id: "unknown-provider".into(),
                name: "Acme Compute".into(),
                app: "codex".into(),
                kind: Some("mystery_transport".into()),
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let providers_json = serde_json::to_string(&providers).expect("encode providers");

        let capabilities = public_app_capabilities(&bindings, None, Some(&providers_json), "codex");
        assert_eq!(capabilities[0].provider_family, "other");
        assert_eq!(listing_provider_families(&capabilities).0, "other");
    }

    #[test]
    fn provider_family_prioritizes_explicit_third_party_api_types() {
        for provider_type in [
            "openai_compatible",
            "openrouter",
            "ollama_cloud",
            "nvidia",
            "deepseek_api",
            "aws_bedrock",
            "custom",
        ] {
            assert_eq!(
                normalized_provider_family_parts(
                    "codex",
                    provider_type,
                    Some(provider_type),
                    Some("OpenAI compatible provider"),
                ),
                "api",
                "provider type {provider_type} should be grouped as an API provider"
            );
        }
    }

    #[test]
    fn provider_family_keeps_first_party_provider_types() {
        assert_eq!(
            normalized_provider_family_parts(
                "codex",
                "official_oauth",
                Some("codex_oauth"),
                Some("OpenAI Official"),
            ),
            "openai"
        );
        assert_eq!(
            normalized_provider_family_parts(
                "gemini",
                "api_key",
                Some("gemini_api_key"),
                Some("Google Gemini"),
            ),
            "google"
        );
        assert_eq!(
            normalized_provider_family_parts(
                "claude",
                "api_key",
                Some("anthropic_api_key"),
                Some("Anthropic"),
            ),
            "anthropic"
        );
        assert_eq!(
            normalized_provider_family_parts(
                "codex",
                "official_oauth",
                Some("codex_oauth"),
                Some("Custom OpenAI compatible label"),
            ),
            "openai"
        );
    }

    #[test]
    fn performance_uses_24h_p50_for_valid_stream_samples() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let conn = store.conn.blocking_lock();
        let share_id = "share-performance";
        let now = Utc::now().timestamp();
        insert_performance_sample(
            &conn,
            "health-newest",
            share_id,
            now,
            200,
            1_000,
            Some(100),
            100,
            "observed",
            true,
            Some("completed"),
            true,
        );
        insert_performance_sample(
            &conn,
            "valid-a",
            share_id,
            now - 1,
            200,
            3_000,
            Some(1_000),
            40,
            "observed",
            true,
            Some("completed"),
            false,
        );
        insert_performance_sample(
            &conn,
            "valid-b",
            share_id,
            now - 2,
            201,
            2_500,
            Some(500),
            20,
            "observed",
            true,
            Some("completed"),
            false,
        );
        insert_performance_sample(
            &conn,
            "pending-usage",
            share_id,
            now - 3,
            200,
            2_700,
            Some(700),
            20,
            "pending",
            true,
            Some("completed"),
            false,
        );
        insert_performance_sample(
            &conn,
            "zero-output",
            share_id,
            now - 4,
            200,
            1_600,
            Some(600),
            0,
            "observed",
            true,
            Some("completed"),
            false,
        );
        insert_performance_sample(
            &conn,
            "interrupted",
            share_id,
            now - 5,
            200,
            2_000,
            Some(500),
            20,
            "observed",
            true,
            Some("interrupted"),
            false,
        );
        insert_performance_sample(
            &conn,
            "non-stream",
            share_id,
            now - 6,
            200,
            2_000,
            Some(500),
            20,
            "observed",
            false,
            Some("completed"),
            false,
        );
        insert_performance_sample(
            &conn,
            "failed",
            share_id,
            now - 7,
            500,
            2_000,
            Some(500),
            20,
            "observed",
            true,
            Some("completed"),
            false,
        );
        insert_performance_sample(
            &conn,
            "no-first-token",
            share_id,
            now - 8,
            200,
            2_000,
            None,
            20,
            "observed",
            true,
            Some("completed"),
            false,
        );
        insert_performance_sample(
            &conn,
            "invalid-latency",
            share_id,
            now - 9,
            200,
            500,
            Some(500),
            20,
            "observed",
            true,
            Some("completed"),
            false,
        );
        insert_performance_sample(
            &conn,
            "not-completed",
            share_id,
            now - 10,
            200,
            2_000,
            Some(500),
            20,
            "observed",
            true,
            None,
            false,
        );
        insert_performance_sample(
            &conn,
            "outside-window",
            share_id,
            now - 25 * 60 * 60,
            200,
            1_500,
            Some(500),
            100,
            "observed",
            true,
            Some("completed"),
            false,
        );

        let performance = share_market_performance(&conn, &[share_id.into()])
            .expect("aggregate performance")
            .remove(share_id)
            .expect("Share performance");
        assert_eq!(performance.recent_request_count, 10);
        assert_eq!(performance.ttft_sample_count, 4);
        assert_eq!(performance.tps_sample_count, 2);
        assert_eq!(performance.average_ttft_ms, Some(650.0));
        assert_eq!(performance.average_tps, Some(15.0));
        assert_eq!(performance.window_hours, 24);
        assert_eq!(
            performance.latest_sample_at,
            DateTime::<Utc>::from_timestamp(now - 1, 0).map(|value| value.to_rfc3339())
        );
    }

    #[test]
    fn performance_generation_window_rejects_terminal_flush_timing() {
        assert_eq!(reliable_generation_window_ms(10_000, 9_999), None);
        assert_eq!(reliable_generation_window_ms(20_000, 19_850), None);
        assert_eq!(reliable_generation_window_ms(20_000, 19_800), Some(200));
        assert_eq!(reliable_generation_window_ms(1_000, 900), Some(100));
    }

    #[test]
    fn performance_without_samples_still_describes_the_24_hour_window() {
        let performance = ShareMarketPerformance::default();
        assert_eq!(performance.recent_request_count, 0);
        assert_eq!(performance.window_hours, 24);
    }

    #[test]
    fn reliability_uses_last_observation_per_minute_within_24_hours() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let conn = store.conn.blocking_lock();
        let share_id = "share-reliability";
        let minute = Utc::now().timestamp().div_euclid(60) * 60;
        for (checked_at, status) in [
            (minute - 179, "healthy"),
            (minute - 141, "unhealthy"),
            (minute - 119, "unhealthy"),
            (minute - 81, "healthy"),
            (minute - 59, "healthy"),
            (minute - 40, "unknown"),
            (minute - 25 * 60 * 60, "healthy"),
        ] {
            conn.execute(
                "INSERT INTO share_health_checks (
                    share_id, checked_at, is_healthy, status, reason, router_epoch
                 ) VALUES (?1, ?2, ?3, ?4, 'test', 'test')",
                params![share_id, checked_at, i64::from(status == "healthy"), status],
            )
            .expect("insert reliability observation");
        }

        let samples =
            share_market_reliability(&conn, &[share_id.into()]).expect("aggregate reliability");
        assert_eq!(samples.get(share_id), Some(&(2, 3, minute - 59)));
        let reliability = reliability_view(samples.get(share_id).copied());
        assert!(
            (reliability.online_rate_24h.expect("observed rate") - 66.666_666_666_666_66).abs()
                < 1e-9
        );
        assert_eq!(reliability.observed_minutes_24h, 3);
        assert!((reliability.observation_coverage_24h - (3.0 / 14.4)).abs() < 1e-9);
        assert!(!reliability.sufficient_coverage);
        assert!(reliability.latest_observed_at.is_some());
        let unknown = reliability_view(None);
        assert_eq!(unknown.online_rate_24h, None);
        assert_eq!(unknown.observed_minutes_24h, 0);
        assert!(!unknown.sufficient_coverage);
        assert_eq!(unknown.latest_observed_at, None);
    }

    #[test]
    fn market_aggregates_span_query_batches() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let conn = store.conn.blocking_lock();
        let share_ids = (0..=MARKET_AGGREGATE_BATCH_SIZE)
            .map(|index| format!("share-batch-{index}"))
            .collect::<Vec<_>>();
        let now = Utc::now().timestamp();
        for (index, share_id) in [share_ids.first(), share_ids.last()]
            .into_iter()
            .flatten()
            .enumerate()
        {
            insert_performance_sample(
                &conn,
                &format!("request-batch-{index}"),
                share_id,
                now + index as i64,
                200,
                2_000,
                Some(500),
                30,
                "observed",
                true,
                Some("completed"),
                false,
            );
            conn.execute(
                "INSERT INTO share_health_checks (
                    share_id, checked_at, is_healthy, status, reason, router_epoch
                 ) VALUES (?1, ?2, 1, 'healthy', 'test', 'test')",
                params![share_id, now + index as i64],
            )
            .expect("insert batched reliability observation");
        }

        let performance =
            share_market_performance(&conn, &share_ids).expect("aggregate batched performance");
        let reliability =
            share_market_reliability(&conn, &share_ids).expect("aggregate batched reliability");
        for share_id in [share_ids.first(), share_ids.last()].into_iter().flatten() {
            assert_eq!(
                performance
                    .get(share_id)
                    .map(|sample| sample.recent_request_count),
                Some(1)
            );
            assert_eq!(
                reliability
                    .get(share_id)
                    .map(|(healthy, observed, _)| (*healthy, *observed)),
                Some((1, 1))
            );
        }
        assert!(
            share_market_performance(&conn, &[])
                .expect("empty performance")
                .is_empty()
        );
        assert!(
            share_market_reliability(&conn, &[])
                .expect("empty reliability")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn public_catalog_keeps_occupancy_without_loading_renter_identity() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-public", "owner-public@example.com");
        let renter = session("renter-public", "renter-secret@example.com");
        insert_share(
            &store,
            "share-public",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(&conn, &owner, "USD", 50_000, &now);
        }
        let listing_id = store
            .share_market_create_listing(
                &owner,
                CreateListingRequest {
                    share_id: "share-public".into(),
                    seats: vec![free_seat(), free_seat()],
                },
            )
            .await
            .expect("create public listing");
        let rented_seat: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT id FROM share_market_seats WHERE listing_id = ?1 AND position = 1",
                params![listing_id],
                |row| row.get(0),
            )
            .expect("read first public seat");
        store
            .share_market_rent_seat(&renter, &rented_seat, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent public seat");

        let catalog = store
            .share_market_catalog(Some(&renter), &["share-public-route".into()])
            .await
            .expect("read catalog before public filtering");
        assert_eq!(catalog.listings[0].seats.len(), 2);
        assert_eq!(catalog.my_subscriptions.len(), 1);
        assert!(
            catalog.listings[0]
                .seats
                .iter()
                .any(|seat| seat.subscription.is_some())
        );

        let mut catalog = store
            .share_market_catalog_with_scope(
                Some(&renter),
                &["share-public-route".into()],
                ShareMarketCatalogScope::Public,
            )
            .await
            .expect("read public catalog");
        retain_public_catalog(&mut catalog);
        assert_eq!(catalog.listings.len(), 1);
        assert_eq!(catalog.listings[0].seats.len(), 2);
        let rented = catalog.listings[0]
            .seats
            .iter()
            .find(|seat| seat.id == rented_seat)
            .expect("rented seat stays in the public catalog");
        let idle = catalog.listings[0]
            .seats
            .iter()
            .find(|seat| seat.id != rented_seat)
            .expect("idle seat stays in the public catalog");
        assert_eq!(idle.status, SEAT_AVAILABLE);
        assert!(idle.subscription.is_none());
        assert_ne!(rented.status, SEAT_AVAILABLE);
        assert!(rented.subscription.is_none());
        let public_json = serde_json::to_string(&catalog).expect("encode public catalog");
        assert!(!public_json.contains("renter-secret@example.com"));
        assert!(!public_json.contains("mySubscriptions"));
    }

    #[tokio::test]
    async fn rent_quote_reports_persisted_trial_balance() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-quote-trial", "owner-quote-trial@example.com");
        let renter = session("renter-quote-trial", "renter-quote-trial@example.com");
        configure_payment_profile(&store, &owner, "quote-trial-account", "quote-trial-profile")
            .await;
        insert_share(
            &store,
            "share-quote-trial",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) = create_listing(&store, &owner, "share-quote-trial", paid_seat()).await;
        let expected_remaining = crate::market_billing::TRIAL_SECONDS - 7_200;
        let now = Utc::now().to_rfc3339();
        store
            .conn
            .lock()
            .await
            .execute(
                "INSERT INTO market_trial_ledgers (
                    buyer_user_id, supplier_user_id, product_kind, service_ref, currency,
                    allowance_seconds, consumed_seconds, created_at, updated_at
                 ) VALUES (?1, ?2, 'share', ?3, 'USD', ?4, 7200, ?5, ?5)",
                params![
                    renter.user_id,
                    owner.user_id,
                    "share-quote-trial",
                    crate::market_billing::TRIAL_SECONDS,
                    now,
                ],
            )
            .expect("seed persisted Share trial balance");

        let quote = store
            .share_market_create_rent_quote(&renter, &seat_id, None)
            .await
            .expect("quote paid Share seat");
        assert_eq!(quote.trial_seconds_remaining, expected_remaining);
        assert_eq!(quote.offer.seat_id, seat_id);
        assert_eq!(quote.offer.daily_rate_minor, Some(1_200));
    }

    #[tokio::test]
    async fn multi_app_rental_ignores_legacy_app_selection_and_grants_the_bundle() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-required-app", "owner-required-app@example.com");
        let renter = session("renter-required-app", "renter-required-app@example.com");
        insert_share(
            &store,
            "share-required-app",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET app_type = 'claude', enabled_claude = 1,
                    bindings_json = '{\"claude\":\"provider-claude\",\"codex\":\"provider-codex\"}'
                 WHERE share_id = 'share-required-app'",
                [],
            )
            .expect("make Share multi-app");
        let (_, seat_id) = create_listing(&store, &owner, "share-required-app", free_seat()).await;
        let quote = store
            .share_market_create_rent_quote(&renter, &seat_id, Some("gemini"))
            .await
            .expect("ignore an unavailable legacy App selection");
        assert_eq!(quote.offer.service.required_app, "claude");
        assert_eq!(quote.offer.service.supported_apps, ["claude", "codex"]);
        assert_eq!(
            quote
                .offer
                .service
                .apps
                .iter()
                .map(|service| service.app.as_str())
                .collect::<Vec<_>>(),
            ["claude", "codex"]
        );
        let committed = store
            .share_market_commit_rent_quote(&renter, &seat_id, &quote.id, "required-app-view")
            .await
            .expect("commit Codex rental");

        let catalog = store
            .share_market_catalog_with_scope(Some(&renter), &[], ShareMarketCatalogScope::Renter)
            .await
            .expect("read renter subscriptions");
        let subscription = catalog
            .my_subscriptions
            .iter()
            .find(|subscription| subscription.id == committed.subscription_id)
            .expect("committed subscription");
        assert_eq!(subscription.app_type, "claude");
        assert_eq!(subscription.apps, ["claude", "codex"]);
    }

    #[tokio::test]
    async fn legacy_single_app_grant_is_widened_without_interrupting_the_subscription() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-app-widen", "owner-app-widen@example.com");
        let renter = session("renter-app-widen", "renter-app-widen@example.com");
        insert_share(
            &store,
            "share-app-widen",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) = create_listing(&store, &owner, "share-app-widen", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent legacy App scope seat");
        let activated_at = Utc::now();
        activate_subscription(&store, &subscription_id, activated_at).await;

        let upgrade_at = activated_at + Duration::seconds(1);
        {
            let conn = store.conn.lock().await;
            let grants_json: String = conn
                .query_row(
                    "SELECT user_grants_json FROM shares WHERE share_id = 'share-app-widen'",
                    [],
                    |row| row.get(0),
                )
                .expect("read all-App managed grant");
            let mut grants: BTreeMap<String, ShareUserGrant> =
                serde_json::from_str(&grants_json).expect("decode all-App managed grant");
            grants
                .get_mut(&renter.email)
                .expect("managed renter grant")
                .policy
                .allowed_apps = vec!["codex".into()];
            conn.execute(
                "UPDATE shares SET user_grants_json = ?2,
                    config_revision = config_revision + 1, updated_at = ?3
                 WHERE share_id = ?1",
                params![
                    "share-app-widen",
                    serde_json::to_string(&grants).expect("encode legacy single-App grant"),
                    upgrade_at.to_rfc3339(),
                ],
            )
            .expect("seed legacy single-App grant");
            conn.execute(
                "UPDATE share_market_subscriptions SET app_scope_enforced_at = NULL
                 WHERE id = ?1",
                params![subscription_id],
            )
            .expect("schedule App scope upgrade");

            reconcile_share_contract_integrity_tx(&conn, "share-app-widen", upgrade_at)
                .expect("enqueue non-disruptive App scope upgrade");
            let state: (String, String, i64, i64) = conn
                .query_row(
                    "SELECT status, integrity_state,
                            (SELECT COUNT(*) FROM share_control_operations operation
                             WHERE operation.subscription_id = sub.id
                               AND operation.action = 'upsert'
                               AND operation.status = 'pending'),
                            (SELECT COUNT(*) FROM share_control_operations operation
                             WHERE operation.subscription_id = sub.id
                               AND operation.action = 'revoke')
                     FROM share_market_subscriptions sub WHERE id = ?1",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read uninterrupted App scope upgrade state");
            assert_eq!(state, (SUB_ACTIVE_FREE.into(), "compatible".into(), 1, 0));

            let policy_json: String = conn
                .query_row(
                    "SELECT policy_json FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = 'upsert' AND status = 'pending'",
                    params![subscription_id],
                    |row| row.get(0),
                )
                .expect("read widened grant policy");
            let policy: ShareUserPolicy =
                serde_json::from_str(&policy_json).expect("decode widened grant policy");
            assert_eq!(policy.allowed_apps, ["claude", "codex", "gemini"]);
        }

        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(upgrade_at)
                .await
                .expect("dispatch non-disruptive App scope upgrade")
                .len(),
            1
        );
        set_entitlement_from_stored_policy(&store, &subscription_id, upgrade_at).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(upgrade_at)
                .await
                .expect("confirm widened App scope")
                .is_empty()
        );

        let final_state: (String, String, Option<String>, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, integrity_state, app_scope_enforced_at,
                        (SELECT COUNT(*) FROM share_control_operations operation
                         WHERE operation.subscription_id = sub.id
                           AND operation.action = 'revoke'),
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = sub.id
                           AND event.event_type = 'contract_integrity_violated')
                 FROM share_market_subscriptions sub WHERE id = ?1",
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
            .expect("read completed App scope upgrade");
        assert_eq!(final_state.0, SUB_ACTIVE_FREE);
        assert_eq!(final_state.1, "compatible");
        assert!(final_state.2.is_some());
        assert_eq!((final_state.3, final_state.4), (0, 0));
    }

    #[tokio::test]
    async fn legacy_inflight_single_app_grant_activates_then_widens_without_quarantine() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-inflight-widen", "owner-inflight-widen@example.com");
        let renter = session("renter-inflight-widen", "renter-inflight-widen@example.com");
        insert_share(
            &store,
            "share-inflight-widen",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) =
            create_listing(&store, &owner, "share-inflight-widen", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent seat before simulating the upgrade boundary");

        {
            let conn = store.conn.lock().await;
            let policy_json: String = conn
                .query_row(
                    "SELECT policy_json FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = 'upsert'",
                    params![subscription_id],
                    |row| row.get(0),
                )
                .expect("read pre-migration grant policy");
            let mut policy: ShareUserPolicy =
                serde_json::from_str(&policy_json).expect("decode pre-migration grant policy");
            policy.allowed_apps = vec!["codex".into()];
            conn.execute(
                "UPDATE share_control_operations SET policy_json = ?2 WHERE subscription_id = ?1",
                params![
                    subscription_id,
                    serde_json::to_string(&policy).expect("encode pre-migration grant policy")
                ],
            )
            .expect("seed a pre-migration grant policy");
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET service_snapshot_json = json_remove(
                         json_set(service_snapshot_json, '$.schemaVersion', 1), '$.apps'
                     ),
                     app_scope_enforced_at = NULL
                 WHERE id = ?1",
                params![subscription_id],
            )
            .expect("seed a v1 grant that was in flight during migration");
        }

        let applied_at = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(applied_at)
                .await
                .expect("dispatch legacy in-flight grant")
                .len(),
            1
        );
        set_entitlement(&store, &subscription_id, applied_at).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(applied_at)
                .await
                .expect("accept and activate legacy in-flight grant")
                .is_empty()
        );
        let activated: (String, Option<String>, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, app_scope_enforced_at,
                        (SELECT COUNT(*) FROM share_control_operations operation
                         WHERE operation.subscription_id = sub.id
                           AND operation.action = 'revoke')
                 FROM share_market_subscriptions sub WHERE id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read legacy activation state");
        assert_eq!(activated, (SUB_ACTIVE_FREE.into(), None, 0));

        let upgrade_at = applied_at + Duration::seconds(1);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(upgrade_at)
                .await
                .expect("dispatch post-activation all-App widening")
                .len(),
            1
        );
        set_entitlement_from_stored_policy(&store, &subscription_id, upgrade_at).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(upgrade_at)
                .await
                .expect("confirm post-activation all-App widening")
                .is_empty()
        );
        let final_state: (String, String, Option<String>, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, integrity_state, app_scope_enforced_at,
                        (SELECT COUNT(*) FROM share_control_operations operation
                         WHERE operation.subscription_id = sub.id
                           AND operation.action = 'revoke'),
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = sub.id
                           AND event.event_type = 'entitlement_contract_violation')
                 FROM share_market_subscriptions sub WHERE id = ?1",
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
            .expect("read widened in-flight grant state");
        assert_eq!(final_state.0, SUB_ACTIVE_FREE);
        assert_eq!(final_state.1, "compatible");
        assert!(final_state.2.is_some());
        assert_eq!((final_state.3, final_state.4), (0, 0));
    }

    #[tokio::test]
    async fn rent_quote_commit_detects_term_drift_without_a_revision_change() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-quote-drift", "owner-quote-drift@example.com");
        let renter = session("renter-quote-drift", "renter-quote-drift@example.com");
        insert_share(
            &store,
            "share-quote-drift",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) = create_listing(&store, &owner, "share-quote-drift", free_seat()).await;
        let quote = store
            .share_market_create_rent_quote(&renter, &seat_id, None)
            .await
            .expect("quote Share seat before drift");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE share_market_seats SET parallel_limit = 3 WHERE id = ?1",
                params![seat_id],
            )
            .expect("simulate term drift without revision bump");

        let error = store
            .share_market_commit_rent_quote(&renter, &seat_id, &quote.id, "quote-term-drift")
            .await
            .expect_err("term drift must invalidate a quote independently of its revision");
        assert_eq!(error.code(), Some("share_market_offer_changed"));

        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE share_market_seats SET parallel_limit = 2 WHERE id = ?1",
                params![seat_id],
            )
            .expect("restore quoted terms");
        store
            .share_market_commit_rent_quote(
                &renter,
                &seat_id,
                &quote.id,
                "quote-term-drift-restored",
            )
            .await
            .expect("failed drift check must leave the quote usable");
    }

    #[tokio::test]
    async fn rent_quote_detects_provider_rebinding_without_exposing_its_identity() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-provider-drift", "owner-provider-drift@example.com");
        let renter = session("renter-provider-drift", "renter-provider-drift@example.com");
        insert_share(
            &store,
            "share-provider-drift",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) =
            create_listing(&store, &owner, "share-provider-drift", free_seat()).await;
        let quote = store
            .share_market_create_rent_quote(&renter, &seat_id, Some("codex"))
            .await
            .expect("quote Share before Provider rebinding");
        assert_eq!(quote.offer.service.provider_binding_fingerprint, None);

        let stored_snapshot_json: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT snapshot_json FROM share_market_rent_quotes WHERE id = ?1",
                params![quote.id],
                |row| row.get(0),
            )
            .expect("read private quote snapshot");
        let stored_snapshot: RentQuoteSnapshot =
            serde_json::from_str(&stored_snapshot_json).expect("decode private quote snapshot");
        assert!(
            stored_snapshot
                .service
                .provider_binding_fingerprint
                .is_some()
        );

        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET bindings_json = '{\"codex\":\"replacement-provider\"}'
                 WHERE share_id = 'share-provider-drift'",
                [],
            )
            .expect("replace the bound Provider without changing its public type");
        let error = store
            .share_market_commit_rent_quote(&renter, &seat_id, &quote.id, "quote-provider-drift")
            .await
            .expect_err("Provider rebinding must invalidate the frozen quote");
        assert_eq!(error.code(), Some("share_market_service_changed"));
    }

    #[tokio::test]
    async fn rent_quote_commit_binds_seat_expires_and_replays_idempotently() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-quote", "owner-quote@example.com");
        let renter = session("renter-quote", "renter-quote@example.com");
        for share_id in ["share-quote-a", "share-quote-b"] {
            insert_share(&store, share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
        }
        let (_, seat_a) = create_listing(&store, &owner, "share-quote-a", free_seat()).await;
        let (_, seat_b) = create_listing(&store, &owner, "share-quote-b", free_seat()).await;
        let quote_a = store
            .share_market_create_rent_quote(&renter, &seat_a, None)
            .await
            .expect("quote first Share seat");

        let wrong_seat = store
            .share_market_commit_rent_quote(&renter, &seat_b, &quote_a.id, "quote-seat-mismatch")
            .await
            .expect_err("quote must remain bound to its seat");
        assert!(matches!(wrong_seat, AppError::Conflict(_)));

        let committed = store
            .share_market_commit_rent_quote(&renter, &seat_a, &quote_a.id, "quote-replay")
            .await
            .expect("commit frozen Share quote");
        assert!(!committed.replayed);
        let replayed = store
            .share_market_commit_rent_quote(&renter, &seat_a, &quote_a.id, "quote-replay")
            .await
            .expect("replay committed Share quote");
        assert!(replayed.replayed);
        assert_eq!(replayed.subscription_id, committed.subscription_id);

        store
            .conn
            .lock()
            .await
            .execute(
                "DELETE FROM share_market_rent_quotes WHERE id = ?1",
                params![quote_a.id],
            )
            .expect("prune retained Share quote");
        let replayed_after_quote_cleanup = store
            .share_market_commit_rent_quote_with_online_state(
                &renter,
                &seat_a,
                &quote_a.id,
                "quote-replay",
                false,
            )
            .await
            .expect("replay committed Share quote after cleanup and disconnect");
        assert!(replayed_after_quote_cleanup.replayed);
        assert_eq!(
            replayed_after_quote_cleanup.subscription_id,
            committed.subscription_id
        );

        let quote_b = store
            .share_market_create_rent_quote(&renter, &seat_b, None)
            .await
            .expect("quote second Share seat");
        let offline = store
            .share_market_commit_rent_quote_with_online_state(
                &renter,
                &seat_b,
                &quote_b.id,
                "quote-offline",
                false,
            )
            .await
            .expect_err("a new rental must not commit while the Share is offline");
        assert_eq!(offline.code(), Some("share_market_share_offline"));
        let reused_key = store
            .share_market_commit_rent_quote(&renter, &seat_b, &quote_b.id, "quote-replay")
            .await
            .expect_err("idempotency key must not identify another quote");
        assert!(matches!(reused_key, AppError::Conflict(_)));

        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE share_market_rent_quotes SET expires_at = ?2 WHERE id = ?1",
                params![quote_b.id, (Utc::now() - Duration::seconds(1)).to_rfc3339()],
            )
            .expect("expire second Share quote");
        let expired = store
            .share_market_commit_rent_quote(&renter, &seat_b, &quote_b.id, "quote-expired")
            .await
            .expect_err("expired quote must be rejected");
        assert!(matches!(expired, AppError::Gone(_)));
    }

    #[tokio::test]
    async fn catalog_scopes_isolate_public_owner_and_renter_data() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner_a = session("owner-scope-a", "owner-scope-a@example.com");
        let owner_b = session("owner-scope-b", "owner-scope-b@example.com");
        let renter = session("renter-scope", "renter-scope@example.com");
        for (share_id, owner) in [("share-scope-a", &owner_a), ("share-scope-b", &owner_b)] {
            insert_share(&store, share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
        }
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(&conn, &owner_a, "USD", 50_000, &now);
        }
        let listing_a = store
            .share_market_create_listing(
                &owner_a,
                CreateListingRequest {
                    share_id: "share-scope-a".into(),
                    seats: vec![free_seat(), free_seat()],
                },
            )
            .await
            .expect("create owner A listing");
        store
            .share_market_create_listing(
                &owner_b,
                CreateListingRequest {
                    share_id: "share-scope-b".into(),
                    seats: vec![free_seat()],
                },
            )
            .await
            .expect("create owner B listing");
        let seat_a: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT id FROM share_market_seats WHERE listing_id = ?1",
                params![listing_a],
                |row| row.get(0),
            )
            .expect("read owner A seat");
        store
            .share_market_rent_seat(&renter, &seat_a, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent owner A seat");
        let active_subdomains = vec!["share-scope-a-route".into(), "share-scope-b-route".into()];

        let mut public = store
            .share_market_catalog_with_scope(
                Some(&renter),
                &active_subdomains,
                ShareMarketCatalogScope::Public,
            )
            .await
            .expect("read public scope");
        let renter_share = public
            .listings
            .iter()
            .find(|listing| listing.share_id == "share-scope-a")
            .expect("public scope keeps the Share including occupied seats");
        assert_eq!(renter_share.seats.len(), 2);
        assert!(renter_share.seats.iter().all(|seat| !seat.can_rent));
        assert!(
            renter_share
                .seats
                .iter()
                .all(|seat| seat.subscription.is_none())
        );
        retain_public_catalog(&mut public);
        let mut public_share_ids = public
            .listings
            .iter()
            .map(|listing| listing.share_id.as_str())
            .collect::<Vec<_>>();
        public_share_ids.sort_unstable();
        assert_eq!(public_share_ids, ["share-scope-a", "share-scope-b"]);
        assert!(public.my_subscriptions.is_empty());

        let owned = store
            .share_market_catalog_with_scope(
                Some(&owner_a),
                &active_subdomains,
                ShareMarketCatalogScope::Owner,
            )
            .await
            .expect("read owner scope");
        assert_eq!(owned.listings.len(), 1);
        assert_eq!(owned.listings[0].share_id, "share-scope-a");
        assert!(owned.listings[0].seats[0].subscription.is_some());
        assert!(owned.my_subscriptions.is_empty());

        let rented = store
            .share_market_catalog_with_scope(
                Some(&renter),
                &active_subdomains,
                ShareMarketCatalogScope::Renter,
            )
            .await
            .expect("read renter scope");
        assert!(rented.listings.is_empty());
        assert_eq!(rented.my_subscriptions.len(), 1);
        assert_eq!(rented.my_subscriptions[0].share_id, "share-scope-a");
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
    async fn reconciliation_cursor_rotates_past_the_first_batch() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let created_at = "2026-01-01T00:00:00+00:00";
        let first_reconciled_at = "2026-01-01T00:01:00+00:00";
        let conn = store.conn.lock().await;
        for index in 0..=MAX_SUBSCRIPTIONS_PER_RECONCILE {
            let listing_id = format!("listing-{index:03}");
            let seat_id = format!("seat-{index:03}");
            let subscription_id = format!("subscription-{index:03}");
            let share_id = format!("share-{index:03}");
            conn.execute(
                "INSERT INTO share_market_listings (
                    id, share_id, installation_id, owner_user_id, owner_email,
                    status, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'owner', 'owner@example.com',
                           'active', ?4, ?4)",
                params![
                    listing_id,
                    share_id,
                    format!("installation-{index:03}"),
                    created_at
                ],
            )
            .expect("insert reconciliation listing");
            conn.execute(
                "INSERT INTO share_market_seats (
                    id, listing_id, position, status, token_period_json,
                    offer_revision, current_subscription_id, created_at, updated_at
                 ) VALUES (?1, ?2, 1, 'occupied', '\"day\"', 1, ?3, ?4, ?4)",
                params![seat_id, listing_id, subscription_id, created_at],
            )
            .expect("insert reconciliation seat");
            conn.execute(
                "INSERT INTO share_market_subscriptions (
                    id, seat_id, listing_id, share_id, installation_id,
                    entitlement_id, owner_user_id, owner_email, renter_user_id,
                    renter_email, status, token_period_json, offer_revision,
                    created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'owner',
                           'owner@example.com', ?7, ?8, 'grant_pending',
                           '\"day\"', 1, ?9, ?9)",
                params![
                    subscription_id,
                    seat_id,
                    listing_id,
                    share_id,
                    format!("installation-{index:03}"),
                    format!("entitlement-{index:03}"),
                    format!("renter-{index:03}"),
                    format!("renter-{index:03}@example.com"),
                    created_at,
                ],
            )
            .expect("insert reconciliation subscription");
        }

        let first_batch = next_reconcile_subscription_ids(&conn).expect("load first batch");
        assert_eq!(first_batch.len(), MAX_SUBSCRIPTIONS_PER_RECONCILE);
        assert!(!first_batch.contains(&"subscription-200".to_string()));
        advance_reconcile_subscription_cursor(&conn, &first_batch, first_reconciled_at)
            .expect("advance first batch");

        let second_batch = next_reconcile_subscription_ids(&conn).expect("load second batch");
        assert_eq!(second_batch.len(), MAX_SUBSCRIPTIONS_PER_RECONCILE);
        assert_eq!(
            second_batch.first().map(String::as_str),
            Some("subscription-200")
        );
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
        let now = Utc::now();

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
        assert_eq!(seat.service_duration_days, Some(1));
    }

    #[test]
    fn rent_quote_offer_comparison_covers_every_frozen_term() {
        let quoted = RentQuoteSnapshot {
            seat_id: "seat-terms".into(),
            listing_id: "listing-terms".into(),
            share_id: "share-terms".into(),
            share_name: "Terms".into(),
            owner_email: "owner@example.com".into(),
            seat_position: 2,
            parallel_limit: Some(3),
            token_limit: Some(40_000),
            token_period: ShareTokenPeriod::SevenDays,
            daily_rate_minor: Some(1_200),
            currency: Some("USD".into()),
            service_duration_days: Some(30),
            offer_revision: 7,
            service: RentServiceSnapshot {
                schema_version: 1,
                required_app: "codex".into(),
                supported_apps: vec!["codex".into()],
                apps: Vec::new(),
                provider_family: "openai".into(),
                provider_type: Some("oauth".into()),
                provider_binding_fingerprint: Some("binding-fingerprint".into()),
                model_mode: "passthrough".into(),
                upstream_model: None,
                models: vec!["gpt-5".into()],
                share_parallel_limit: Some(8),
                share_token_limit: Some(100_000),
                share_tokens_used: 1_000,
            },
        };
        let current = RentOfferTerms::from_snapshot(&quoted);
        validate_offer_terms_unchanged(&quoted, &current).expect("matching terms");

        macro_rules! assert_changed {
            ($field:ident, $value:expr) => {{
                let mut changed = current.clone();
                changed.$field = $value;
                let error = validate_offer_terms_unchanged(&quoted, &changed)
                    .expect_err(concat!(stringify!($field), " must be frozen"));
                assert_eq!(error.code(), Some("share_market_offer_changed"));
            }};
        }
        assert_changed!(owner_email, "other@example.com".into());
        assert_changed!(seat_position, 3);
        assert_changed!(parallel_limit, Some(4));
        assert_changed!(token_limit, Some(40_001));
        assert_changed!(token_period, ShareTokenPeriod::ThirtyDays);
        assert_changed!(daily_rate_minor, Some(1_201));
        assert_changed!(currency, Some("CNY".into()));
        assert_changed!(service_duration_days, Some(31));
    }

    #[test]
    fn managed_grant_application_time_is_bounded_by_the_rental_and_clock_skew() {
        let created_at = "2026-01-01T00:00:00Z";
        let observed_at = "2026-01-01T00:01:00Z";
        let created_ms = parse_time(created_at).unwrap().timestamp_millis();
        let observed_ms = parse_time(observed_at).unwrap().timestamp_millis();
        validate_managed_grant_applied_at(created_at, created_ms, observed_at)
            .expect("grant may apply at subscription creation");
        validate_managed_grant_applied_at(
            created_at,
            observed_ms + MANAGED_GRANT_MAX_FUTURE_SKEW_SECS * 1_000,
            observed_at,
        )
        .expect("configured future clock skew is accepted");
        assert!(
            validate_managed_grant_applied_at(created_at, created_ms - 1, observed_at).is_err()
        );
        assert!(
            validate_managed_grant_applied_at(
                created_at,
                observed_ms + MANAGED_GRANT_MAX_FUTURE_SKEW_SECS * 1_000 + 1,
                observed_at,
            )
            .is_err()
        );
    }

    #[test]
    fn service_duration_is_bounded_for_free_and_paid_seats() {
        let mut invalid = free_seat();
        invalid.service_duration_days = Some(MAX_SERVICE_DURATION_DAYS + 1);
        assert!(normalize_seat(invalid).is_err());

        let mut permanent = free_seat();
        permanent.service_duration_days = None;
        assert_eq!(
            normalize_seat(permanent)
                .expect("permanent free seat")
                .service_duration_days,
            None
        );

        let mut paid = paid_seat();
        paid.service_duration_days = Some(7);
        assert_eq!(
            normalize_seat(paid)
                .expect("fixed paid service")
                .service_duration_days,
            Some(7)
        );
    }

    #[test]
    fn unlimited_token_quota_ignores_the_submitted_period() {
        let mut seat = free_seat();
        seat.token_limit = None;
        seat.token_period = ShareTokenPeriod::ThirtyDays;
        let normalized = normalize_seat(seat).expect("unlimited token seat");
        assert_eq!(normalized.token_limit, None);
        assert_eq!(normalized.token_period, ShareTokenPeriod::Lifetime);
    }

    #[tokio::test]
    async fn owned_share_options_include_domain_owner_and_enabled_bound_apps() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-options", "owner-options@example.com");
        insert_share(
            &store,
            "share-options",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares
                 SET bindings_json = ?2, enabled_claude = 1, enabled_codex = 0,
                     enabled_gemini = 1
                 WHERE share_id = ?1",
                params![
                    "share-options",
                    serde_json::json!({
                        "claude": "provider-claude",
                        "gemini": "provider-gemini",
                    })
                    .to_string(),
                ],
            )
            .expect("set Share bindings");

        let shares = store
            .share_market_owned_shares(&owner)
            .await
            .expect("list owned Share options");
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].subdomain, "share-options-route");
        assert_eq!(shares[0].owner_email, owner.email);
        assert_eq!(shares[0].supported_apps, vec!["claude", "gemini"]);
        assert_eq!(shares[0].parallel_limit, Some(3));
        assert_eq!(shares[0].token_limit, None);
        assert_eq!(
            shares[0].expires_at.as_deref(),
            Some("9999-12-31T23:59:59Z")
        );
        assert!(!shares[0].app_capabilities.is_empty());
        assert!(!shares[0].free_access);
    }

    #[tokio::test]
    async fn owned_share_options_expose_free_access_for_listing_candidate_filtering() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-free-options", "owner-free-options@example.com");
        insert_share(
            &store,
            "share-free-options",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET free_access = 1 WHERE share_id = ?1",
                params!["share-free-options"],
            )
            .expect("make Share public free");

        let shares = store
            .share_market_owned_shares(&owner)
            .await
            .expect("list owned Share options");
        assert_eq!(shares.len(), 1);
        assert!(shares[0].free_access);
    }

    #[tokio::test]
    async fn pending_free_access_edit_blocks_share_market_listing_race() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-pending-free", "owner-pending-free@example.com");
        insert_share(
            &store,
            "share-pending-free",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        store
            .create_share_settings_edit(
                "share-pending-free",
                &owner.email,
                ShareSettingsPatch {
                    free_access: Some(true),
                    ..ShareSettingsPatch::default()
                },
            )
            .await
            .expect("queue public free access edit");

        let error = store
            .share_market_create_listing(
                &owner,
                CreateListingRequest {
                    share_id: "share-pending-free".to_string(),
                    seats: vec![free_seat()],
                },
            )
            .await
            .expect_err("pending Free edit must block listing");
        assert!(matches!(error, AppError::Conflict(_)));
        assert!(error.to_string().contains("pending public free access"));
    }

    #[tokio::test]
    async fn finite_subscription_does_not_consume_service_term_before_grant_dispatch() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-pending-expiry", "owner-pending-expiry@example.com");
        let renter = session("renter-pending-expiry", "renter-pending-expiry@example.com");
        insert_share(
            &store,
            "share-pending-expiry",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) =
            create_listing(&store, &owner, "share-pending-expiry", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent finite seat");
        let pending_timing: (Option<String>, Option<String>) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT service_started_at, expires_at
                 FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read pending service timing");
        assert_eq!(pending_timing, (None, None));

        let dispatched_at = Utc::now() + Duration::days(30);
        let dispatched = store
            .share_market_reconcile_and_dispatch(dispatched_at)
            .await
            .expect("dispatch delayed finite service");
        assert_eq!(dispatched.len(), 1);
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_GRANT_PENDING
        );
        let state: (String, String, Option<String>, Option<String>, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT seat.status, operation.status,
                        subscription.service_started_at, subscription.expires_at,
                        (SELECT COUNT(*) FROM share_market_events
                         WHERE subscription_id = ?1 AND event_type = 'service_term_expired')
                 FROM share_market_seats seat
                 JOIN share_market_subscriptions subscription ON subscription.id = ?1
                 JOIN share_control_operations operation
                   ON operation.subscription_id = ?1 AND operation.action = 'upsert'
                 WHERE seat.id = ?2",
                params![subscription_id, seat_id],
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
            .expect("read delayed finite service state");
        assert_eq!(state.0, SEAT_RESERVED);
        assert_eq!(state.1, "dispatched");
        assert_eq!(state.2, None);
        assert_eq!(state.3, None);
        assert_eq!(state.4, 0);
        let patch_json: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit.patch_json
                 FROM share_control_operations operation
                 JOIN share_edit_requests edit ON edit.id = operation.edit_id
                 WHERE operation.subscription_id = ?1 AND operation.action = 'upsert'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read relative grant patch");
        let patch: ShareSettingsPatch =
            serde_json::from_str(&patch_json).expect("decode relative grant patch");
        let grant = patch.managed_grant.expect("relative managed grant");
        assert_eq!(grant.duration_seconds, Some(86_400));
        let policy = grant.policy.expect("relative grant policy");
        assert_eq!(policy.expires_at, None);
        assert_eq!(policy.token_period_anchor_at_ms, None);
    }

    #[tokio::test]
    async fn finite_free_subscription_expires_once_from_first_grant_dispatch() {
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
        let expected_activated_at =
            DateTime::<Utc>::from_timestamp_millis(activated_at.timestamp_millis()).unwrap();

        let (stored_activated_at, service_started_at, expires_at): (String, String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT activated_at, service_started_at, expires_at
                 FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read service term");
        assert_eq!(
            parse_time(&stored_activated_at).unwrap(),
            expected_activated_at
        );
        let service_started_at = parse_time(&service_started_at).unwrap();
        let expires_at = parse_time(&expires_at).unwrap();
        assert_eq!(service_started_at, expected_activated_at);
        assert_eq!(expires_at, service_started_at + Duration::days(1));

        store
            .share_market_reconcile_and_dispatch(expires_at)
            .await
            .expect("expire fixed service");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );
        store
            .share_market_reconcile_and_dispatch(expires_at + Duration::days(1))
            .await
            .expect("repeat expiry reconciliation");
        let expiry_events: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT COUNT(*) FROM share_market_events
                 WHERE subscription_id = ?1 AND event_type = 'service_term_expired'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("count expiry events");
        assert_eq!(expiry_events, 1);

        clear_entitlements(&store, "share-expiry").await;
        store
            .share_market_reconcile_and_dispatch(expires_at + Duration::days(1))
            .await
            .expect("finish expired service release");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_RELEASED
        );
    }

    #[tokio::test]
    async fn finite_paid_subscription_expires_and_terminates_billing_contract() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-paid-expiry", "owner-paid-expiry@example.com");
        let renter = session("renter-paid-expiry", "renter-paid-expiry@example.com");
        insert_share(
            &store,
            "share-paid-expiry",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        configure_payment_profile(
            &store,
            &owner,
            "owner-paid-expiry",
            &Utc::now().to_rfc3339(),
        )
        .await;
        let mut seat = paid_seat();
        seat.service_duration_days = Some(1);
        let (_, seat_id) = create_listing(&store, &owner, "share-paid-expiry", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent fixed paid seat");
        let dispatched_at = Utc::now();
        activate_subscription(&store, &subscription_id, dispatched_at).await;
        let expected_dispatched_at =
            DateTime::<Utc>::from_timestamp_millis(dispatched_at.timestamp_millis()).unwrap();
        let (service_started_at, expires_at, policy_json): (String, String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT subscription.service_started_at, subscription.expires_at,
                        operation.policy_json
                 FROM share_market_subscriptions subscription
                 JOIN share_control_operations operation
                   ON operation.subscription_id = subscription.id AND operation.action = 'upsert'
                 WHERE subscription.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read fixed service policy");
        assert_eq!(
            parse_time(&service_started_at).unwrap(),
            expected_dispatched_at
        );
        let policy: ShareUserPolicy = serde_json::from_str(&policy_json).expect("decode policy");
        assert_eq!(
            policy.expires_at,
            Some(parse_time(&expires_at).unwrap().timestamp_millis())
        );

        let expires_at = parse_time(&expires_at).unwrap();
        let contract_id: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT id FROM market_service_contracts
                 WHERE product_kind = 'share' AND product_ref = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read expiring paid contract");
        let accrual_started_at = expires_at - Duration::seconds(10);
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE market_service_contracts
                 SET status = 'active', trial_seconds_remaining = 0,
                     last_evaluated_at = ?2, updated_at = ?2 WHERE id = ?1",
                params![contract_id, accrual_started_at.to_rfc3339()],
            )
            .expect("prepare final paid accrual");
        record_share_health(
            &store,
            "share-paid-expiry",
            expires_at - Duration::seconds(1),
        )
        .await;
        store
            .share_market_reconcile_and_dispatch(expires_at + Duration::seconds(5))
            .await
            .expect("expire paid service");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );
        clear_entitlements(&store, "share-paid-expiry").await;
        store
            .share_market_reconcile_and_dispatch(expires_at + Duration::seconds(1))
            .await
            .expect("release expired paid service");
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_RELEASED
        );
        let contract: (String, String, i64, i64, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT contract.status, contract.termination_reason, account.balance_units,
                        accrual.billable_seconds, interval.ended_at
                 FROM market_service_contracts contract
                 JOIN market_credit_accounts account ON account.id = contract.account_id
                 JOIN market_accrual_entries accrual ON accrual.contract_id = contract.id
                 JOIN market_service_intervals interval ON interval.id = accrual.interval_id
                 WHERE contract.id = ?1",
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
            .expect("read terminated paid contract");
        assert_eq!(contract.0, "terminated");
        assert_eq!(contract.1, "service_term_expired");
        assert_eq!(contract.2, 1_200_i64 * 10_i64);
        assert_eq!(contract.3, 10);
        assert_eq!(parse_time(&contract.4).unwrap(), expires_at);
    }

    #[tokio::test]
    async fn billing_reconcile_never_accrues_past_share_service_expiry() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-billing-cap", "owner-billing-cap@example.com");
        let renter = session("renter-billing-cap", "renter-billing-cap@example.com");
        insert_share(
            &store,
            "share-billing-cap",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        configure_payment_profile(
            &store,
            &owner,
            "owner-billing-cap",
            &Utc::now().to_rfc3339(),
        )
        .await;
        let mut seat = paid_seat();
        seat.service_duration_days = Some(1);
        let (_, seat_id) = create_listing(&store, &owner, "share-billing-cap", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent billing-capped paid seat");
        activate_subscription(&store, &subscription_id, Utc::now()).await;
        let (contract_id, expires_at): (String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT contract.id, subscription.expires_at
                 FROM market_service_contracts contract
                 JOIN share_market_subscriptions subscription
                   ON subscription.id = contract.product_ref
                 WHERE subscription.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read billing-capped contract");
        let expires_at = parse_time(&expires_at).unwrap();
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE market_service_contracts
                 SET status = 'active', trial_seconds_remaining = 0,
                     last_evaluated_at = ?2, updated_at = ?2 WHERE id = ?1",
                params![
                    contract_id,
                    (expires_at - Duration::seconds(10)).to_rfc3339()
                ],
            )
            .expect("prepare capped billing interval");
        record_share_health(
            &store,
            "share-billing-cap",
            expires_at - Duration::seconds(1),
        )
        .await;

        store
            .market_billing_reconcile(expires_at + Duration::seconds(5))
            .await
            .expect("reconcile after Share service expiry");
        store
            .market_billing_reconcile(expires_at + Duration::seconds(10))
            .await
            .expect("repeat capped billing reconciliation");

        let (balance_units, billable_seconds, interval_end): (i64, i64, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT account.balance_units, accrual.billable_seconds, interval.ended_at
                 FROM market_service_contracts contract
                 JOIN market_credit_accounts account ON account.id = contract.account_id
                 JOIN market_accrual_entries accrual ON accrual.contract_id = contract.id
                 JOIN market_service_intervals interval ON interval.id = accrual.interval_id
                 WHERE contract.id = ?1",
                params![contract_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read capped market accrual");
        assert_eq!(balance_units, 1_200_i64 * 10_i64);
        assert_eq!(billable_seconds, 10);
        assert_eq!(parse_time(&interval_end).unwrap(), expires_at);
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
        seat.service_duration_days = None;
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
        input.currency = Some("USD".into());
        assert!(normalize_seat(input).is_err());

        let mut input = free_seat();
        input.daily_rate_minor = Some(0);
        input.currency = Some("USD".into());
        assert!(normalize_seat(input).is_err());

        let mut input = free_seat();
        input.token_limit = Some(i64::MAX as u64 + 1);
        assert!(normalize_seat(input).is_err());
    }

    #[test]
    fn paid_seat_defaults_to_usd_and_rejects_other_currencies() {
        let mut defaulted = paid_seat();
        defaulted.currency = None;
        assert_eq!(
            normalize_seat(defaulted)
                .expect("default paid seat currency")
                .currency
                .as_deref(),
            Some("USD")
        );

        let mut cny = paid_seat();
        cny.currency = Some("CNY".into());
        assert!(normalize_seat(cny).is_err());
    }

    #[tokio::test]
    async fn fixed_token_period_is_anchored_to_the_first_grant_dispatch_minute() {
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

        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent fixed-period seat");
        let pending_policy_json: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT policy_json FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read pending fixed-period policy");
        let pending_policy: ShareUserPolicy =
            serde_json::from_str(&pending_policy_json).expect("decode pending policy");
        assert!(pending_policy.token_period_anchor_at_ms.is_none());

        let dispatched_at = Utc::now() + Duration::hours(3);
        record_share_health(&store, "share-anchor", dispatched_at).await;
        activate_subscription(&store, &subscription_id, dispatched_at).await;
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
        assert_eq!(
            anchor,
            dispatched_at.timestamp_millis().div_euclid(60_000) * 60_000
        );
    }

    #[test]
    fn schema_enforces_one_active_renter_per_share() {
        let conn = Connection::open_in_memory().expect("memory database");
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        crate::schema::apply(&conn).expect("schema");
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
        crate::schema::apply(&conn).expect("schema");

        for table in ["share_market_seats", "share_market_subscriptions"] {
            let columns = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .expect("prepare Share Market columns")
                .query_map([], |row| row.get::<_, String>(1))
                .expect("query Share Market columns")
                .collect::<Result<Vec<_>, _>>()
                .expect("read Share Market columns");
            assert!(columns.iter().any(|column| column == "daily_rate_minor"));
            assert!(
                columns
                    .iter()
                    .any(|column| column == "service_duration_days")
            );
            assert!(!columns.iter().any(|column| column == "free_duration_days"));
            assert!(!columns.iter().any(|column| column == "period_unit"));
            assert!(!columns.iter().any(|column| column == "period_count"));
        }
    }

    #[tokio::test]
    async fn listing_seat_parallel_limits_cannot_exceed_share_capacity() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-seat-capacity", "owner-seat-capacity@example.com");
        insert_share(
            &store,
            "share-seat-capacity",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let mut oversized = free_seat();
        oversized.parallel_limit = Some(4);

        let create_error = store
            .share_market_create_listing(
                &owner,
                CreateListingRequest {
                    share_id: "share-seat-capacity".into(),
                    seats: vec![oversized.clone()],
                },
            )
            .await
            .expect_err("oversized initial seat must be rejected");
        assert!(matches!(create_error, AppError::BadRequest(_)));

        let (listing_id, seat_id) =
            create_listing(&store, &owner, "share-seat-capacity", free_seat()).await;
        let add_error = store
            .share_market_add_seat(&owner, &listing_id, oversized.clone())
            .await
            .expect_err("oversized added seat must be rejected");
        assert!(matches!(add_error, AppError::BadRequest(_)));
        let update_error = store
            .share_market_update_seat(
                &owner,
                &seat_id,
                UpdateSeatRequest {
                    seat: oversized,
                    offer_revision: 1,
                },
            )
            .await
            .expect_err("oversized edited seat must be rejected");
        assert!(matches!(update_error, AppError::BadRequest(_)));

        let unchanged: (Option<i64>, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT parallel_limit, offer_revision FROM share_market_seats WHERE id = ?1",
                params![seat_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read unchanged seat offer");
        assert_eq!(unchanged, (Some(2), 1));

        let second_seat = store
            .share_market_add_seat(&owner, &listing_id, free_seat())
            .await
            .expect("second burst-pool seat may reuse Share capacity");
        let third_seat = store
            .share_market_add_seat(&owner, &listing_id, free_seat())
            .await
            .expect("aggregate seat concurrency is not a reserved-capacity sum");
        assert_ne!(second_seat, third_seat);
        let configured_total: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT SUM(parallel_limit) FROM share_market_seats
                 WHERE listing_id = ?1 AND status != 'deleted'",
                params![listing_id],
                |row| row.get(0),
            )
            .expect("sum burst-pool seat limits");
        assert_eq!(configured_total, 6);
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
    async fn released_seats_are_reused_without_expanding_listing_capacity() {
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
                "USD",
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

        assert!(matches!(
            store
                .share_market_add_seat(&owner, &listing_id, free_seat())
                .await,
            Err(AppError::Conflict(_))
        ));
        let (active_count, available_count, retired_count): (i64, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT
                    SUM(CASE WHEN retired_at IS NULL AND status IN
                        ('available', 'reserved', 'occupied', 'revoking') THEN 1 ELSE 0 END),
                    SUM(CASE WHEN id = ?2 AND status = 'available'
                                  AND current_subscription_id IS NULL THEN 1 ELSE 0 END),
                    SUM(CASE WHEN retired_at IS NOT NULL THEN 1 ELSE 0 END)
                 FROM share_market_seats WHERE listing_id = ?1",
                params![listing_id, first_seat],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read effective listing capacity");
        assert_eq!(active_count, MAX_SEATS_PER_LISTING as i64);
        assert_eq!(available_count, 1);
        assert_eq!(retired_count, 0);

        let next_renter = session("renter-capacity-next", "renter-capacity-next@example.com");
        store
            .share_market_rent_seat(
                &next_renter,
                &first_seat,
                RentSeatRequest { offer_revision: 1 },
            )
            .await
            .expect("reuse released capacity seat");
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
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
                .await
                .expect("dispatch ordered revoke")
                .len(),
            1
        );
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );

        let (upsert_status, upsert_edit_status, revoke_edit): (String, String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT upsert.status, upsert_edit.status, revoke.edit_id
                 FROM share_control_operations upsert
                 JOIN share_edit_requests upsert_edit ON upsert_edit.id = upsert.edit_id
                 JOIN share_control_operations revoke
                   ON revoke.subscription_id = upsert.subscription_id
                  AND revoke.action = 'revoke'
                 WHERE upsert.subscription_id = ?1 AND upsert.action = 'upsert'",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read ordered grant and revoke controls");
        assert_eq!(upsert_status, "rejected");
        assert_eq!(upsert_edit_status, "cancelled");
        assert!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(2))
                .await
                .expect("do not infer revoke from an unconfirmed empty snapshot")
                .is_empty()
        );
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
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );
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
                &(now + Duration::seconds(3)).to_rfc3339(),
            )
            .expect("complete revoke operation");
        }
        store
            .share_market_reconcile_and_dispatch(now + Duration::seconds(4))
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
    async fn missing_share_terminates_active_subscription_without_waiting_for_descriptor() {
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
        let (_listing_id, seat_id) =
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
        let (status_after_missing, seat_status, retired_subscription_id): (
            String,
            String,
            Option<String>,
        ) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, seat.status, seat.retired_subscription_id
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read terminated missing Share");
        assert_eq!(status_after_missing, SUB_RELEASED);
        assert_eq!(seat_status, SEAT_DISABLED);
        assert_eq!(
            retired_subscription_id.as_deref(),
            Some(subscription_id.as_str())
        );
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
        assert_eq!(revoke_status, "applied");
        assert_eq!(subscription_status, SUB_RELEASED);
    }

    #[tokio::test]
    async fn free_rental_descriptor_confirmation_recovers_lost_acks_and_allows_repeat_rental() {
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
        assert_eq!(seat_status, "available");
        assert!(retired_subscription_id.is_none());
        assert!(retired_at.is_none());
        assert_eq!(revoke_status, "applied");
        assert_eq!(revoke_attempts, 0);
        store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("same renter can reuse released free seat");

        let owner_catalog = store
            .share_market_catalog(Some(&owner), &[])
            .await
            .expect("read owner catalog after release");
        let reused = owner_catalog.listings[0]
            .seats
            .iter()
            .find(|seat| seat.id == seat_id)
            .expect("reused owner seat");
        assert_eq!(reused.status, "reserved");
        assert!(!reused.read_only);
        assert_eq!(
            reused
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
        assert_eq!(renter_catalog.listings[0].seats[0].status, "reserved");
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
            assert_eq!(payload["parallelLimit"], 2);
            assert_eq!(payload["tokenLimit"], 10_000);
            assert_eq!(payload["tokenPeriod"], "day");
            assert_eq!(payload["dailyRateMinor"], 1_200);
            assert_eq!(payload["currency"], "USD");
            assert_eq!(payload["paymentMethodKinds"], serde_json::json!(["alipay"]));
            assert!(payload.get("renterEmail").is_none());
            assert!(payload.get("paymentMethods").is_none());
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
            assert_eq!(pending_contract.0, "pending_activation");
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
        assert_eq!(contract.5, "USD");
    }

    #[tokio::test]
    async fn paid_grant_is_revoked_when_credit_closes_before_activation() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-pre-activation-close", "owner-close@example.com");
        let renter = session("renter-pre-activation-close", "renter-close@example.com");
        configure_payment_profile(&store, &owner, "account-close", "profile-close").await;
        insert_share(
            &store,
            "share-pre-activation-close",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-pre-activation-close", paid_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent paid seat before credit closure");
        let applied_at = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(applied_at)
                .await
                .expect("dispatch pending paid grant")
                .len(),
            1
        );
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_credit_accounts SET status = 'closed', updated_at = ?2
                 WHERE id = (SELECT account_id FROM market_service_contracts
                             WHERE product_ref = ?1)",
                params![subscription_id, applied_at.to_rfc3339()],
            )
            .expect("close credit before grant acknowledgement");
        }
        set_entitlement(&store, &subscription_id, applied_at).await;

        let revoke_events = store
            .share_market_reconcile_and_dispatch(applied_at + Duration::seconds(1))
            .await
            .expect("reject activation and dispatch revoke");
        assert_eq!(revoke_events.len(), 1);
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );
        let state: (String, Option<String>, String, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT contract.status, contract.termination_reason,
                        subscription.release_reason, revoke.status,
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = subscription.id
                           AND event.event_type = 'entitlement_activated')
                 FROM market_service_contracts contract
                 JOIN share_market_subscriptions subscription
                   ON subscription.id = contract.product_ref
                 JOIN share_control_operations revoke
                   ON revoke.subscription_id = subscription.id AND revoke.action = 'revoke'
                 WHERE subscription.id = ?1",
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
            .expect("read rejected paid activation state");
        assert_eq!(state.0, "terminated");
        assert_eq!(
            state.1.as_deref(),
            Some("billing_account_unavailable_before_activation")
        );
        assert_eq!(state.2, "billing_account_unavailable_before_activation");
        assert_eq!(state.3, "dispatched");
        assert_eq!(state.4, 0);
    }

    #[tokio::test]
    async fn accepted_price_change_applies_after_old_rate_accrual_boundary() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-reprice", "owner-reprice@example.com");
        let renter = session("renter-reprice", "renter-reprice@example.com");
        configure_payment_profile(&store, &owner, "account-reprice", "profile-reprice").await;
        insert_share(
            &store,
            "share-reprice",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-reprice", paid_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent paid Share for repricing");
        let activated_at = Utc::now();
        activate_subscription(&store, &subscription_id, activated_at).await;
        let contract_id: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT id FROM market_service_contracts
                 WHERE product_kind = 'share' AND product_ref = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read repriced Share contract");
        let rate_started_at = activated_at + Duration::seconds(1);
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE market_service_contracts
                 SET status = 'active', trial_seconds_remaining = 0,
                     last_evaluated_at = ?2, updated_at = ?2 WHERE id = ?1",
                params![contract_id, rate_started_at.to_rfc3339()],
            )
            .expect("finish repriced Share trial");

        let proposal_id = store
            .share_market_propose_price_change(
                &owner,
                &subscription_id,
                ProposePriceChangeRequest {
                    daily_rate_minor: 2_400,
                    offer_revision: 1,
                },
            )
            .await
            .expect("propose Share price change");
        store
            .share_market_accept_price_change(&renter, &proposal_id)
            .await
            .expect("accept Share price change");
        let before_reconcile: (i64, i64, i64, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.daily_rate_minor, seat.daily_rate_minor,
                        contract.daily_rate_minor, change.status
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
                 JOIN share_market_price_changes change ON change.subscription_id = sub.id
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read accepted unapplied Share price change");
        assert_eq!(before_reconcile, (1_200, 1_200, 1_200, "accepted".into()));

        let first_boundary = rate_started_at + Duration::seconds(10);
        record_share_health(&store, "share-reprice", first_boundary).await;
        store
            .market_billing_reconcile(first_boundary)
            .await
            .expect("apply Share price change after old-rate accrual");
        let after_reconcile: (i64, i64, i64, i64, i64, i64, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.daily_rate_minor, sub.offer_revision,
                        seat.daily_rate_minor, seat.offer_revision,
                        contract.daily_rate_minor, contract.offer_revision, change.status
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
                 JOIN share_market_price_changes change ON change.subscription_id = sub.id
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
                        row.get(6)?,
                    ))
                },
            )
            .expect("read applied Share price change");
        assert_eq!(
            after_reconcile,
            (2_400, 2, 2_400, 2, 2_400, 2, "applied".into())
        );

        let second_boundary = first_boundary + Duration::seconds(10);
        record_share_health(&store, "share-reprice", second_boundary).await;
        store
            .market_billing_reconcile(second_boundary)
            .await
            .expect("accrue at new Share price");
        let (balance_units, accruals): (i64, Vec<(i64, i64, i64)>) = {
            let conn = store.conn.lock().await;
            let balance_units = conn
                .query_row(
                    "SELECT account.balance_units
                     FROM market_credit_accounts account
                     JOIN market_service_contracts contract ON contract.account_id = account.id
                     WHERE contract.id = ?1",
                    params![contract_id],
                    |row| row.get(0),
                )
                .expect("read repriced Share balance");
            let accruals = conn
                .prepare(
                    "SELECT daily_rate_minor, billable_seconds, amount_units
                     FROM market_accrual_entries WHERE contract_id = ?1
                     ORDER BY created_at, id",
                )
                .expect("prepare repriced Share accruals")
                .query_map(params![contract_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .expect("query repriced Share accruals")
                .collect::<Result<Vec<_>, _>>()
                .expect("read repriced Share accruals");
            (balance_units, accruals)
        };
        assert_eq!(balance_units, 1_200_i64 * 10_i64 + 2_400_i64 * 10_i64);
        assert_eq!(
            accruals,
            vec![
                (1_200, 10, 1_200_i64 * 10_i64),
                (2_400, 10, 2_400_i64 * 10_i64),
            ]
        );
        let event_count: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT COUNT(*) FROM market_billing_events
                 WHERE contract_id = ?1 AND event_type = 'service_contract_price_changed'",
                params![contract_id],
                |row| row.get(0),
            )
            .expect("count Share contract price events");
        assert_eq!(event_count, 1);
    }

    #[tokio::test]
    async fn price_change_roles_and_terminal_cancellation_are_enforced() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-price-flow", "owner-price-flow@example.com");
        let renter = session("renter-price-flow", "renter-price-flow@example.com");
        let stranger = session("stranger-price-flow", "stranger-price-flow@example.com");
        configure_payment_profile(&store, &owner, "account-price-flow", "profile-price-flow").await;
        insert_share(
            &store,
            "share-price-flow",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-price-flow", paid_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent Share for price flow");
        activate_subscription(&store, &subscription_id, Utc::now()).await;

        let proposal_id = store
            .share_market_propose_price_change(
                &owner,
                &subscription_id,
                ProposePriceChangeRequest {
                    daily_rate_minor: 1_500,
                    offer_revision: 1,
                },
            )
            .await
            .expect("propose rejected Share price");
        assert!(matches!(
            store
                .share_market_accept_price_change(&stranger, &proposal_id)
                .await,
            Err(AppError::Forbidden(_))
        ));
        store
            .share_market_reject_price_change(&renter, &proposal_id)
            .await
            .expect("reject Share price change");

        let cancelled_id = store
            .share_market_propose_price_change(
                &owner,
                &subscription_id,
                ProposePriceChangeRequest {
                    daily_rate_minor: 1_800,
                    offer_revision: 1,
                },
            )
            .await
            .expect("propose cancelled Share price");
        store
            .share_market_cancel_price_change(&owner, &cancelled_id)
            .await
            .expect("cancel Share price change");

        let accepted_id = store
            .share_market_propose_price_change(
                &owner,
                &subscription_id,
                ProposePriceChangeRequest {
                    daily_rate_minor: 2_100,
                    offer_revision: 1,
                },
            )
            .await
            .expect("propose terminal Share price");
        store
            .share_market_accept_price_change(&renter, &accepted_id)
            .await
            .expect("accept terminal Share price");
        store
            .share_market_request_release(&renter, &subscription_id, false, false)
            .await
            .expect("release Share with accepted price change");
        let statuses = store
            .conn
            .lock()
            .await
            .prepare(
                "SELECT status FROM share_market_price_changes
                 WHERE subscription_id = ?1 ORDER BY created_at, id",
            )
            .expect("prepare Share price flow statuses")
            .query_map(params![subscription_id], |row| row.get::<_, String>(0))
            .expect("query Share price flow statuses")
            .collect::<Result<Vec<_>, _>>()
            .expect("read Share price flow statuses");
        assert_eq!(statuses, vec!["rejected", "cancelled", "cancelled"]);
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
        for stored in [release_reason, event_detail] {
            assert!(!stored.contains("fake-managed-grant-secret"));
            assert!(stored.contains("[credential omitted]"));
        }
        assert!(!outbox_payload.contains("fake-managed-grant-secret"));
        assert!(!outbox_payload.contains("credential omitted"));
        assert!(!outbox_payload.contains("error"));
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
    async fn invalid_observed_grant_is_quarantined_without_blocking_other_subscriptions() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-invalid-grant", "owner-invalid-grant@example.com");
        let bad_renter = session("renter-invalid-grant", "bad-renter@example.com");
        let good_renter = session("renter-valid-grant", "good-renter@example.com");
        for share_id in ["share-invalid-grant", "share-valid-grant"] {
            insert_share(&store, share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
        }
        let (_, bad_seat) =
            create_listing(&store, &owner, "share-invalid-grant", free_seat()).await;
        let (_, good_seat) = create_listing(&store, &owner, "share-valid-grant", free_seat()).await;
        let bad_subscription = store
            .share_market_rent_seat(
                &bad_renter,
                &bad_seat,
                RentSeatRequest { offer_revision: 1 },
            )
            .await
            .expect("rent invalid-grant seat");
        let good_subscription = store
            .share_market_rent_seat(
                &good_renter,
                &good_seat,
                RentSeatRequest { offer_revision: 1 },
            )
            .await
            .expect("rent valid-grant seat");
        let applied_at = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(applied_at)
                .await
                .expect("dispatch both grants")
                .len(),
            2
        );
        set_entitlement(&store, &bad_subscription, applied_at).await;
        set_entitlement(&store, &good_subscription, applied_at).await;
        {
            let conn = store.conn.lock().await;
            let grants_json: String = conn
                .query_row(
                    "SELECT user_grants_json FROM shares WHERE share_id = 'share-invalid-grant'",
                    [],
                    |row| row.get(0),
                )
                .expect("read invalid Share grant");
            let mut grants: BTreeMap<String, ShareUserGrant> =
                serde_json::from_str(&grants_json).expect("decode invalid Share grants");
            grants
                .values_mut()
                .next()
                .expect("managed grant")
                .policy
                .parallel_limit = Some(3);
            conn.execute(
                "UPDATE shares SET user_grants_json = ?2 WHERE share_id = ?1",
                params![
                    "share-invalid-grant",
                    serde_json::to_string(&grants).expect("encode divergent grant")
                ],
            )
            .expect("publish divergent managed grant");
        }

        let dispatched = store
            .share_market_reconcile_and_dispatch(applied_at + Duration::seconds(1))
            .await
            .expect("quarantine invalid grant and continue reconciliation");
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].share_id, "share-invalid-grant");
        assert_eq!(
            subscription_status(&store, &bad_subscription).await,
            SUB_REVOKE_PENDING
        );
        assert_eq!(
            subscription_status(&store, &good_subscription).await,
            SUB_ACTIVE_FREE
        );
        let invalid_control: (String, Option<String>, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT upsert.status, upsert.dead_lettered_at, revoke.status
                 FROM share_control_operations upsert
                 JOIN share_control_operations revoke
                   ON revoke.subscription_id = upsert.subscription_id
                  AND revoke.action = 'revoke'
                 WHERE upsert.subscription_id = ?1 AND upsert.action = 'upsert'",
                params![bad_subscription],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read quarantined grant controls");
        assert_eq!(invalid_control.0, "rejected");
        assert!(invalid_control.1.is_some());
        assert_eq!(invalid_control.2, "dispatched");

        let owner_catalog = store
            .share_market_catalog_with_scope(Some(&owner), &[], ShareMarketCatalogScope::Owner)
            .await
            .expect("read quarantined grant as owner");
        let owner_subscription = owner_catalog
            .listings
            .iter()
            .flat_map(|listing| &listing.seats)
            .find(|seat| seat.id == bad_seat)
            .and_then(|seat| seat.subscription.as_ref())
            .expect("owner sees quarantined subscription");
        assert_eq!(
            owner_subscription.failure_code.as_deref(),
            Some(SHARE_GRANT_CONTRACT_VIOLATION_CODE)
        );
        assert_eq!(
            owner_subscription.release_reason.as_deref(),
            Some(SHARE_GRANT_CONTRACT_VIOLATION_CODE)
        );

        let outsider = session(
            "outsider-invalid-grant",
            "outsider-invalid-grant@example.com",
        );
        let outsider_catalog = store
            .share_market_catalog_with_scope(Some(&outsider), &[], ShareMarketCatalogScope::Visible)
            .await
            .expect("read quarantined grant as unrelated user");
        let outsider_subscription = outsider_catalog
            .listings
            .iter()
            .flat_map(|listing| &listing.seats)
            .find(|seat| seat.id == bad_seat)
            .and_then(|seat| seat.subscription.as_ref())
            .expect("quarantined subscription remains visible");
        assert!(outsider_subscription.failure_code.is_none());
        assert!(outsider_subscription.release_reason.is_none());
    }

    #[tokio::test]
    async fn billing_resume_ack_must_preserve_the_active_contract_policy() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-resume-policy", "owner-resume-policy@example.com");
        let renter = session("renter-resume-policy", "renter-resume-policy@example.com");
        configure_payment_profile(
            &store,
            &owner,
            "account-resume-policy",
            &Utc::now().to_rfc3339(),
        )
        .await;
        insert_share(
            &store,
            "share-resume-policy",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let mut seat = paid_seat();
        seat.service_duration_days = Some(2);
        let (_, seat_id) = create_listing(&store, &owner, "share-resume-policy", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent paid seat for billing resume");
        let activated_at = Utc::now();
        activate_subscription(&store, &subscription_id, activated_at).await;

        let (service_started_at, expires_at, expected_policy): (String, String, ShareUserPolicy) = {
            let conn = store.conn.lock().await;
            let (service_started_at, expires_at, policy_json): (String, String, String) = conn
                .query_row(
                    "SELECT subscription.service_started_at, subscription.expires_at,
                            operation.policy_json
                     FROM share_market_subscriptions subscription
                     JOIN share_control_operations operation
                       ON operation.subscription_id = subscription.id
                      AND operation.action = 'upsert'
                     WHERE subscription.id = ?1
                     ORDER BY operation.share_sequence DESC LIMIT 1",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("read active paid policy");
            (
                service_started_at,
                expires_at,
                serde_json::from_str(&policy_json).expect("decode active paid policy"),
            )
        };
        clear_entitlements(&store, "share-resume-policy").await;

        let resume_requested_at = activated_at + Duration::seconds(1);
        {
            let conn = store.conn.lock().await;
            let record = subscription_record(&conn, &subscription_id)
                .expect("read suspended subscription")
                .expect("suspended subscription");
            let tx = conn.transaction().expect("begin billing resume seed");
            enqueue_control_operation_tx(
                &tx,
                &record.share_id,
                &record.id,
                &record.entitlement_id,
                "upsert",
                &record.renter_email,
                Some(&expected_policy),
                &resume_requested_at.to_rfc3339(),
            )
            .expect("enqueue billing resume");
            tx.execute(
                "UPDATE share_market_subscriptions
                 SET status = 'billing_resume_pending' WHERE id = ?1",
                params![subscription_id],
            )
            .expect("mark billing resume pending");
            tx.commit().expect("commit billing resume seed");
        }
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(resume_requested_at)
                .await
                .expect("dispatch billing resume")
                .len(),
            1
        );

        let edit_id: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert' AND status = 'dispatched'
                 ORDER BY share_sequence DESC LIMIT 1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read billing resume edit");
        let ack_at = resume_requested_at + Duration::seconds(1);
        {
            let conn = store.conn.lock().await;
            let mut divergent = Vec::new();
            let mut policy = expected_policy.clone();
            policy.parallel_limit = Some(policy.parallel_limit.unwrap_or_default() + 1);
            divergent.push(policy);
            let mut policy = expected_policy.clone();
            policy.token_limit = Some(policy.token_limit.unwrap_or_default() + 1);
            divergent.push(policy);
            let mut policy = expected_policy.clone();
            policy.token_period = ShareTokenPeriod::Week;
            divergent.push(policy);
            let mut policy = expected_policy.clone();
            policy.token_period_anchor_at_ms = Some(
                policy
                    .token_period_anchor_at_ms
                    .unwrap_or_default()
                    .saturating_add(1),
            );
            divergent.push(policy);
            let mut policy = expected_policy.clone();
            policy.expires_at = Some(policy.expires_at.unwrap_or_default().saturating_add(1));
            divergent.push(policy);
            let mut policy = expected_policy.clone();
            policy.allowed_apps = vec!["codex".into()];
            divergent.push(policy);

            let record = subscription_record(&conn, &subscription_id)
                .expect("read pending billing resume")
                .expect("pending billing resume");
            for policy in &divergent {
                let error = validate_effective_resume_policy(&record, policy)
                    .expect_err("billing resume cannot rewrite the active contract");
                assert!(
                    error
                        .to_string()
                        .contains("differs from the active rental contract")
                );
            }
            conn.execute(
                "UPDATE share_edit_requests SET status = 'applied', applied_at = ?2
                 WHERE id = ?1",
                params![edit_id, ack_at.to_rfc3339()],
            )
            .expect("record divergent billing resume edit");
            handle_control_edit_ack_with_effect(
                &conn,
                &edit_id,
                "applied",
                None,
                None,
                None,
                Some(ack_at.timestamp_millis()),
                divergent.first(),
                &ack_at.to_rfc3339(),
            )
            .expect("quarantine divergent billing resume policy");
        }
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_REVOKE_PENDING
        );
        let dispatched = store
            .share_market_reconcile_and_dispatch(ack_at + Duration::seconds(1))
            .await
            .expect("dispatch safety revoke after divergent billing resume");
        assert_eq!(dispatched.len(), 1);

        let (stored_start, stored_expiry, stored_policy, upsert_status, revoke_status): (
            String,
            String,
            String,
            String,
            String,
        ) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT subscription.service_started_at, subscription.expires_at,
                        operation.policy_json, operation.status, revoke.status
                 FROM share_market_subscriptions subscription
                 JOIN share_control_operations operation
                   ON operation.subscription_id = subscription.id
                  AND operation.action = 'upsert'
                 JOIN share_control_operations revoke
                   ON revoke.subscription_id = subscription.id AND revoke.action = 'revoke'
                 WHERE subscription.id = ?1
                 ORDER BY operation.share_sequence DESC LIMIT 1",
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
            .expect("read quarantined resume contract policy");
        assert_eq!(stored_start, service_started_at);
        assert_eq!(stored_expiry, expires_at);
        assert_eq!(
            serde_json::from_str::<ShareUserPolicy>(&stored_policy).unwrap(),
            expected_policy
        );
        assert_eq!(upsert_status, "rejected");
        assert_eq!(revoke_status, "dispatched");
    }

    #[tokio::test]
    async fn legacy_billing_resume_ack_reuses_the_frozen_effective_policy() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session(
            "owner-resume-legacy-policy",
            "owner-resume-legacy-policy@example.com",
        );
        let renter = session(
            "renter-resume-legacy-policy",
            "renter-resume-legacy-policy@example.com",
        );
        configure_payment_profile(
            &store,
            &owner,
            "account-resume-legacy-policy",
            &Utc::now().to_rfc3339(),
        )
        .await;
        insert_share(
            &store,
            "share-resume-legacy-policy",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let mut seat = paid_seat();
        seat.service_duration_days = Some(2);
        let (_, seat_id) = create_listing(&store, &owner, "share-resume-legacy-policy", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent paid seat for a legacy billing resume");
        let activated_at = Utc::now();
        activate_subscription(&store, &subscription_id, activated_at).await;

        let expected_policy: ShareUserPolicy = {
            let conn = store.conn.lock().await;
            let policy_json: String = conn
                .query_row(
                    "SELECT operation.policy_json
                     FROM share_control_operations operation
                     WHERE operation.subscription_id = ?1
                       AND operation.action = 'upsert'
                     ORDER BY operation.share_sequence DESC LIMIT 1",
                    params![subscription_id],
                    |row| row.get(0),
                )
                .expect("read frozen active policy for a legacy billing resume");
            let mut policy: ShareUserPolicy =
                serde_json::from_str(&policy_json).expect("decode frozen legacy resume policy");
            policy.allowed_apps = vec!["codex".into()];
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET service_snapshot_json = json_remove(
                         json_set(service_snapshot_json, '$.schemaVersion', 1), '$.apps'
                     ),
                     app_scope_enforced_at = NULL
                 WHERE id = ?1",
                params![subscription_id],
            )
            .expect("mark the billing resume contract as migrated from v1");
            policy
        };
        clear_entitlements(&store, "share-resume-legacy-policy").await;

        let resume_requested_at = activated_at + Duration::seconds(1);
        {
            let conn = store.conn.lock().await;
            let record = subscription_record(&conn, &subscription_id)
                .expect("read legacy suspended subscription")
                .expect("legacy suspended subscription");
            let tx = conn
                .transaction()
                .expect("begin legacy billing resume seed");
            enqueue_control_operation_tx(
                &tx,
                &record.share_id,
                &record.id,
                &record.entitlement_id,
                "upsert",
                &record.renter_email,
                Some(&expected_policy),
                &resume_requested_at.to_rfc3339(),
            )
            .expect("enqueue legacy billing resume");
            tx.execute(
                "UPDATE share_market_subscriptions
                 SET status = 'billing_resume_pending' WHERE id = ?1",
                params![subscription_id],
            )
            .expect("mark legacy billing resume pending");
            tx.commit().expect("commit legacy billing resume seed");
        }
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(resume_requested_at)
                .await
                .expect("dispatch legacy billing resume")
                .len(),
            1
        );

        let edit_id: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert' AND status = 'dispatched'
                 ORDER BY share_sequence DESC LIMIT 1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read legacy billing resume edit");
        let ack_at = resume_requested_at + Duration::seconds(1);
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'applied', applied_at = ?2
                 WHERE id = ?1",
                params![edit_id, ack_at.to_rfc3339()],
            )
            .expect("record legacy billing resume edit");
            handle_control_edit_ack_with_effect(
                &conn,
                &edit_id,
                "applied",
                None,
                None,
                None,
                Some(ack_at.timestamp_millis()),
                None,
                &ack_at.to_rfc3339(),
            )
            .expect("accept legacy billing resume without effectivePolicy");
        }
        assert!(
            store
                .share_market_reconcile_and_dispatch(ack_at + Duration::seconds(1))
                .await
                .expect("confirm legacy billing resume")
                .is_empty()
        );
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_ACTIVE_POSTPAID
        );
        let (operation_status, stored_policy): (String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT operation.status, operation.policy_json
                 FROM share_control_operations operation
                 WHERE operation.subscription_id = ?1 AND operation.action = 'upsert'
                 ORDER BY operation.share_sequence DESC LIMIT 1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read completed legacy billing resume");
        assert_eq!(operation_status, "applied");
        assert_eq!(
            serde_json::from_str::<ShareUserPolicy>(&stored_policy).unwrap(),
            expected_policy
        );

        let upgrade_at = ack_at + Duration::seconds(2);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(upgrade_at)
                .await
                .expect("dispatch all-App widening after legacy billing resume")
                .len(),
            1
        );
        set_entitlement_from_stored_policy(&store, &subscription_id, upgrade_at).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(upgrade_at)
                .await
                .expect("confirm all-App widening after legacy billing resume")
                .is_empty()
        );
        let final_state: (String, Option<String>, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, app_scope_enforced_at,
                        (SELECT COUNT(*) FROM share_control_operations operation
                         WHERE operation.subscription_id = sub.id
                           AND operation.action = 'revoke')
                 FROM share_market_subscriptions sub WHERE id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read widened legacy billing resume state");
        assert_eq!(final_state.0, SUB_ACTIVE_POSTPAID);
        assert!(final_state.1.is_some());
        assert_eq!(final_state.2, 0);
    }

    #[tokio::test]
    async fn nonretryable_billing_revoke_is_dead_lettered_without_recreation() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session(
            "owner-billing-terminal",
            "owner-billing-terminal@example.com",
        );
        let renter = session(
            "renter-billing-terminal",
            "renter-billing-terminal@example.com",
        );
        insert_share(
            &store,
            "share-billing-terminal",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let mut seat = free_seat();
        seat.service_duration_days = None;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-billing-terminal", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent terminal billing seat");
        let now = Utc::now();
        activate_subscription(&store, &subscription_id, now).await;
        {
            let conn = store.conn.lock().await;
            let (share_id, entitlement_id, renter_email): (String, String, String) = conn
                .query_row(
                    "SELECT share_id, entitlement_id, renter_email
                     FROM share_market_subscriptions WHERE id = ?1",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("read subscription for terminal billing suspension");
            let tx = conn
                .transaction()
                .expect("begin terminal billing suspension");
            enqueue_control_operation_tx(
                &tx,
                &share_id,
                &subscription_id,
                &entitlement_id,
                "revoke",
                &renter_email,
                None,
                &now.to_rfc3339(),
            )
            .expect("enqueue terminal billing revoke");
            tx.execute(
                "UPDATE share_market_subscriptions
                 SET status = 'billing_suspend_pending' WHERE id = ?1",
                params![subscription_id],
            )
            .expect("request terminal billing suspension");
            tx.commit().expect("commit terminal billing suspension");
        }
        let dispatched_at = now + Duration::seconds(1);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(dispatched_at)
                .await
                .expect("dispatch terminal billing revoke")
                .len(),
            1
        );
        {
            let conn = store.conn.lock().await;
            let edit_id: String = conn
                .query_row(
                    "SELECT edit_id FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = 'revoke'",
                    params![subscription_id],
                    |row| row.get(0),
                )
                .expect("read terminal billing revoke edit");
            conn.execute(
                "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                params![edit_id],
            )
            .expect("reject terminal billing revoke edit");
            handle_control_edit_ack_with_metadata(
                &conn,
                &edit_id,
                "rejected",
                Some("permission denied"),
                Some("permission_denied"),
                Some(false),
                &(dispatched_at + Duration::seconds(1)).to_rfc3339(),
            )
            .expect("record nonretryable billing revoke failure");
        }

        assert!(
            store
                .share_market_reconcile_and_dispatch(dispatched_at + Duration::days(1))
                .await
                .expect("reconcile terminal billing revoke")
                .is_empty()
        );
        let terminal: (String, Option<String>, String, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT operation.status, operation.dead_lettered_at, sub.status,
                        COUNT(operation.id),
                        (SELECT COUNT(*) FROM operator_alert_signal_outbox signal
                         WHERE signal.source_event_id =
                               'share-control-dead-letter:' || operation.id)
                 FROM share_control_operations operation
                 INNER JOIN share_market_subscriptions sub ON sub.id = operation.subscription_id
                 WHERE operation.subscription_id = ?1 AND operation.action = 'revoke'
                 GROUP BY sub.id",
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
            .expect("read terminal nonretryable billing revoke");
        assert_eq!(terminal.0, "rejected");
        assert!(terminal.1.is_some());
        assert_eq!(terminal.2, SUB_BILLING_CONTROL_FAILED);
        assert_eq!(terminal.3, 1);
        assert_eq!(terminal.4, 1);

        let conn = store.conn.lock().await;
        let (entitlement_id, renter_email): (String, String) = conn
            .query_row(
                "SELECT entitlement_id, renter_email FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read terminal revoke identity");
        let error = enqueue_control_operation_tx(
            &conn,
            "share-billing-terminal",
            &subscription_id,
            &entitlement_id,
            "revoke",
            &renter_email,
            None,
            &(dispatched_at + Duration::days(2)).to_rfc3339(),
        )
        .expect_err("terminal revoke cannot be recreated through another entry point");
        assert!(error.to_string().contains("requires operator intervention"));
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
        configure_payment_profile(&store, &owner, "account-control", "profile-control").await;
        let (listing_a, seat_a) =
            create_listing(&store, &owner, "share-control-a", free_seat()).await;
        let (_listing_b, seat_b) =
            create_listing(&store, &owner, "share-control-b", paid_seat()).await;
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
        let error = store
            .share_market_rent_seat(&renter, &seat_b, RentSeatRequest { offer_revision: 1 })
            .await
            .expect_err("deny another Share rental after future access is revoked");
        assert_eq!(
            error.code(),
            Some(crate::market_access::ERROR_MARKET_ACCESS_REQUIRED)
        );
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
            assert!(
                !crate::market_access::product_access_allowed_tx(
                    &conn,
                    &owner.user_id,
                    &renter.user_id,
                    &renter.email,
                    crate::market_access::PRODUCT_SHARE,
                    crate::market_access::PRICING_PAID,
                )
                .expect("read denied paid Share access")
            );
            crate::market_access::set_product_access_decision_tx(
                &conn,
                &owner.user_id,
                &owner.email,
                &renter.user_id,
                &renter.email,
                crate::market_access::PRODUCT_SHARE,
                crate::market_access::PRICING_PAID,
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
    async fn config_revision_retry_waits_for_refresh_and_uses_each_new_revision() {
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
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET config_revision = 3 WHERE share_id = 'share-retry'",
                [],
            )
            .expect("seed Share config revision");
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
        let (first_edit, first_patch): (String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT operation.edit_id, edit.patch_json
                 FROM share_control_operations operation
                 JOIN share_edit_requests edit ON edit.id = operation.edit_id
                 WHERE operation.subscription_id = ?1 AND operation.action = 'upsert'",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read first grant edit");
        let first_patch: serde_json::Value =
            serde_json::from_str(&first_patch).expect("parse first grant patch");
        assert_eq!(
            first_patch
                .pointer("/managedGrant/expectedConfigRevision")
                .and_then(serde_json::Value::as_u64),
            Some(3)
        );
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                params![first_edit],
            )
            .expect("reject first edit");
            handle_control_edit_ack_with_metadata(
                &conn,
                &first_edit,
                "rejected",
                Some("managed grant expected config revision 3, current revision is 4"),
                Some(SHARE_REVISION_CONFLICT_CODE),
                Some(true),
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
        }
        assert!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
                .await
                .expect("wait for authoritative Share refresh")
                .is_empty()
        );
        let blocked_retry: (String, i64, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, attempts, edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read revision-gated retry");
        assert_eq!(blocked_retry.0, "pending");
        assert_eq!(blocked_retry.1, 1);
        assert_eq!(blocked_retry.2, first_edit);
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET config_revision = 4 WHERE share_id = 'share-retry'",
                [],
            )
            .expect("apply authoritative revision four");
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(2))
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
            Some(4)
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
                "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                params![second_edit],
            )
            .expect("reject second edit");
            handle_control_edit_ack_with_metadata(
                &conn,
                &second_edit,
                "rejected",
                Some("managed grant expected config revision 4, current revision is 5"),
                Some(SHARE_REVISION_CONFLICT_CODE),
                Some(true),
                &(now + Duration::seconds(3)).to_rfc3339(),
            )
            .expect("retry second config conflict");
            conn.execute(
                "UPDATE shares SET config_revision = 5 WHERE share_id = 'share-retry'",
                [],
            )
            .expect("apply authoritative revision five");
        }
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(4))
                .await
                .expect("dispatch third grant")
                .len(),
            1
        );
        let (attempts, third_edit, third_patch): (i64, String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT operation.attempts, operation.edit_id, edit.patch_json
                 FROM share_control_operations operation
                 JOIN share_edit_requests edit ON edit.id = operation.edit_id
                 WHERE operation.subscription_id = ?1 AND operation.action = 'upsert'",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read third grant");
        assert_eq!(attempts, 3);
        assert_ne!(third_edit, second_edit);
        let third_patch: serde_json::Value =
            serde_json::from_str(&third_patch).expect("parse third grant patch");
        assert_eq!(
            third_patch
                .pointer("/managedGrant/expectedConfigRevision")
                .and_then(serde_json::Value::as_u64),
            Some(5)
        );

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'applied' WHERE id = ?1",
                params![third_edit],
            )
            .expect("apply retried edit");
            handle_control_edit_ack(
                &conn,
                &third_edit,
                "applied",
                None,
                &(now + Duration::seconds(5)).to_rfc3339(),
            )
            .expect("apply retried control operation");
            handle_control_edit_ack(
                &conn,
                &third_edit,
                "rejected",
                Some("late duplicate rejection"),
                &(now + Duration::seconds(6)).to_rfc3339(),
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
        assert_eq!(delayed_subscription_status, SUB_ACTIVE_FREE);
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_ACTIVE_FREE
        );
    }

    #[tokio::test]
    async fn applied_ack_rolls_back_descriptor_and_edit_when_control_update_fails() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-ack-rollback", "owner-ack-rollback@example.com");
        let renter = session("renter-ack-rollback", "renter-ack-rollback@example.com");
        insert_share(
            &store,
            "share-ack-rollback",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-ack-rollback", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent rollback seat");
        let now = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now)
                .await
                .expect("dispatch rollback grant")
                .len(),
            1
        );
        let entitlement_id = subscription_entitlement(&store, &subscription_id).await;
        let conn = store.conn.lock().await;
        let edit_id: String = conn
            .query_row(
                "SELECT edit_id FROM share_control_operations
                 WHERE subscription_id = ?1 AND action = 'upsert'",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read rollback grant edit");
        conn.execute_batch(
            "CREATE TEMP TRIGGER fail_share_control_ack
             BEFORE UPDATE OF status ON share_control_operations
             WHEN NEW.status = 'applied'
             BEGIN
                 SELECT RAISE(ABORT, 'injected Share control ACK failure');
             END;",
        )
        .expect("install ACK failure trigger");
        {
            let tx = conn
                .unchecked_transaction()
                .expect("begin failing ACK transaction");
            tx.execute(
                "UPDATE share_edit_requests
                 SET status = 'applied', updated_at = ?2, applied_at = ?2
                 WHERE id = ?1 AND status = 'pending'",
                params![edit_id, now.to_rfc3339()],
            )
            .expect("stage applied edit before injected failure");
            let error = handle_control_edit_ack(&tx, &edit_id, "applied", None, &now.to_rfc3339())
                .expect_err("control operation update must fail");
            assert!(
                error
                    .to_string()
                    .contains("injected Share control ACK failure")
            );
        }
        conn.execute_batch("DROP TRIGGER fail_share_control_ack;")
            .expect("remove ACK failure trigger");
        let (edit_status, operation_status, grants_json): (String, String, Option<String>) = conn
            .query_row(
                "SELECT edit.status, operation.status, share.user_grants_json
                 FROM share_edit_requests edit
                 JOIN share_control_operations operation ON operation.edit_id = edit.id
                 JOIN shares share ON share.share_id = operation.share_id
                 WHERE edit.id = ?1",
                params![edit_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read rolled back ACK state");
        assert_eq!(edit_status, "pending");
        assert_eq!(operation_status, "dispatched");
        assert!(!active_entitlement(grants_json.as_deref(), &entitlement_id));
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
            handle_control_edit_ack_with_metadata(
                &conn,
                &first_edit,
                "rejected",
                Some("managed grant rejected"),
                Some("grant_policy_rejected"),
                Some(false),
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
        let owner_catalog = store
            .share_market_catalog_with_scope(Some(&owner), &[], ShareMarketCatalogScope::Owner)
            .await
            .expect("read failed seat cleanup capability");
        let failed_seat = owner_catalog
            .listings
            .iter()
            .find(|listing| listing.id == listing_id)
            .and_then(|listing| listing.seats.iter().find(|seat| seat.id == seat_id))
            .expect("failed seat remains visible to owner");
        assert!(failed_seat.can_delete);
        let failed_subscription = failed_seat
            .subscription
            .as_ref()
            .expect("failed subscription");
        assert_eq!(
            failed_subscription.failure_code.as_deref(),
            Some("grant_policy_rejected")
        );
        assert_eq!(failed_subscription.grant_attempts, Some(1));
        assert_eq!(
            failed_subscription.release_reason.as_deref(),
            Some("managed grant rejected")
        );
        store
            .share_market_delete_seat(&owner, &seat_id)
            .await
            .expect("soft-delete failed grant seat");
        store
            .share_market_delete_seat(&owner, &seat_id)
            .await
            .expect("repeat failed seat cleanup is idempotent");
        let preserved: (String, Option<String>, String, Option<i64>, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT seat.status, seat.retired_subscription_id, subscription.status,
                        subscription.free_usage_seconds,
                        (SELECT COUNT(*) FROM share_control_operations operation
                         WHERE operation.subscription_id = subscription.id),
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = subscription.id)
                 FROM share_market_seats seat
                 JOIN share_market_subscriptions subscription
                   ON subscription.id = seat.retired_subscription_id
                 WHERE seat.id = ?1",
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
            .expect("read preserved failed seat history");
        assert_eq!(preserved.0, SEAT_DELETED);
        assert_eq!(preserved.1.as_deref(), Some(first_subscription.as_str()));
        assert_eq!(preserved.2, SUB_GRANT_FAILED);
        assert_eq!(preserved.3, Some(86_400));
        assert_eq!(preserved.4, 1);
        assert!(preserved.5 >= 2);
        let renter_history = store
            .share_market_catalog_with_scope(
                Some(&first_renter),
                &[],
                ShareMarketCatalogScope::Renter,
            )
            .await
            .expect("read renter history after owner cleanup");
        assert!(
            renter_history
                .my_subscriptions
                .iter()
                .any(|subscription| subscription.id == first_subscription)
        );
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
        let replacement_now = Utc::now() + Duration::seconds(1);
        activate_subscription(&store, &second_subscription, replacement_now).await;
        assert!(matches!(
            store
                .share_market_delete_seat(&owner, &replacement_seat)
                .await,
            Err(AppError::Conflict(_))
        ));

        store
            .share_market_request_release(&owner, &second_subscription, true, false)
            .await
            .expect("request revoke");
        assert!(matches!(
            store
                .share_market_delete_seat(&owner, &replacement_seat)
                .await,
            Err(AppError::Conflict(_))
        ));
        store
            .share_market_reconcile_and_dispatch(replacement_now + Duration::seconds(1))
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
                &(replacement_now + Duration::seconds(2)).to_rfc3339(),
            )
            .expect("record failed revoke");
        }
        assert_eq!(
            subscription_status(&store, &second_subscription).await,
            SUB_REVOKE_FAILED
        );
        assert!(matches!(
            store
                .share_market_delete_seat(&owner, &replacement_seat)
                .await,
            Err(AppError::Conflict(_))
        ));
        store
            .share_market_request_release(&owner, &second_subscription, true, false)
            .await
            .expect("retry failed revoke");
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(replacement_now + Duration::seconds(3))
                .await
                .expect("redispatch revoke")
                .len(),
            1
        );
        clear_entitlements(&store, "share-failure").await;
        store
            .share_market_reconcile_and_dispatch(replacement_now + Duration::seconds(4))
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
        assert_eq!(replacement_retirement.0, "available");
        assert!(replacement_retirement.1.is_none());
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
    async fn stopped_listing_must_reopen_the_original_listing() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-relist", "owner-relist@example.com");
        insert_share(
            &store,
            "share-relist",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, seat_id) =
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
        assert_eq!(owned[0].market_state, "stopped");
        assert!(!owned[0].can_create_listing);
        assert_eq!(
            owned[0].reopen_listing_id.as_deref(),
            Some(listing_id.as_str())
        );

        let error = store
            .share_market_create_listing(
                &owner,
                CreateListingRequest {
                    share_id: "share-relist".into(),
                    seats: vec![paid_seat()],
                },
            )
            .await
            .expect_err("a stopped listing must be selected before paid-offer validation");
        assert_eq!(error.code(), Some(ERROR_REOPEN_REQUIRED));

        let mut revised = free_seat();
        revised.parallel_limit = Some(1);
        let response = store
            .share_market_reopen_listing(
                &owner,
                &listing_id,
                ReopenListingRequest {
                    existing_seats: vec![ReopenExistingSeatRequest {
                        seat_id: seat_id.clone(),
                        offer_revision: 2,
                        seat: revised,
                    }],
                    new_seats: vec![free_seat()],
                },
            )
            .await
            .expect("reopen original listing with old and new seats");
        assert_eq!(response.listing_id, listing_id);
        assert_eq!(
            response.reopened_seat_ids.as_slice(),
            std::slice::from_ref(&seat_id)
        );
        assert_eq!(response.new_seat_ids.len(), 1);

        let state: (String, i64, i64, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT listing.status,
                        (SELECT COUNT(*) FROM share_market_seats current
                         WHERE current.listing_id = listing.id),
                        seat.offer_revision, seat.parallel_limit,
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.listing_id = listing.id
                           AND event.event_type = 'listing_relisted')
                 FROM share_market_listings listing
                 JOIN share_market_seats seat ON seat.id = ?2
                 WHERE listing.id = ?1",
                params![listing_id, seat_id],
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
            .expect("read reopened listing");
        assert_eq!(state, ("active".into(), 2, 3, 1, 1));

        let owned_after = store
            .share_market_owned_shares(&owner)
            .await
            .expect("owned shares after relist");
        assert!(owned_after[0].already_listed);
        assert_eq!(
            owned_after[0].active_listing_id.as_deref(),
            Some(listing_id.as_str())
        );
        assert!(owned_after[0].reopen_listing_id.is_none());
        assert_eq!(owned_after[0].market_state, "listed");
    }

    #[tokio::test]
    async fn stopping_listing_expires_quotes_bumps_revision_and_is_idempotent() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-stop-quote", "owner-stop-quote@example.com");
        let renter = session("renter-stop-quote", "renter-stop-quote@example.com");
        insert_share(
            &store,
            "share-stop-quote",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, seat_id) =
            create_listing(&store, &owner, "share-stop-quote", free_seat()).await;
        let quote = store
            .share_market_create_rent_quote(&renter, &seat_id, None)
            .await
            .expect("create quote before stopping listing");

        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("stop listing");
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            ("closed".into(), SEAT_DISABLED.into(), 2),
        );
        let quote_status: String = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status FROM share_market_rent_quotes WHERE id = ?1",
                params![quote.id],
                |row| row.get(0),
            )
            .expect("read expired quote");
        assert_eq!(quote_status, "expired");

        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("stopping an already stopped listing is idempotent");
        let (status, seat_status, revision) =
            stopped_listing_state(&store, &listing_id, &seat_id).await;
        assert_eq!(
            (status.as_str(), seat_status.as_str(), revision),
            ("closed", SEAT_DISABLED, 2)
        );
        let close_events: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT COUNT(*) FROM share_market_events
                 WHERE listing_id = ?1 AND event_type = 'listing_closed'",
                params![listing_id],
                |row| row.get(0),
            )
            .expect("count close events");
        assert_eq!(close_events, 1);
    }

    #[tokio::test]
    async fn reopening_listing_preserves_active_rentals() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-reopen-rented", "owner-reopen-rented@example.com");
        let renter = session("renter-reopen-rented", "renter-reopen-rented@example.com");
        insert_share(
            &store,
            "share-reopen-rented",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, rented_seat_id) =
            create_listing(&store, &owner, "share-reopen-rented", free_seat()).await;
        let idle_seat_id = store
            .share_market_add_seat(&owner, &listing_id, free_seat())
            .await
            .expect("add idle seat");
        let subscription_id = store
            .share_market_rent_seat(
                &renter,
                &rented_seat_id,
                RentSeatRequest { offer_revision: 1 },
            )
            .await
            .expect("rent first seat");
        let before: (String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, seat.status
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read active rental before stop");
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("stop listing with active rental");
        store
            .share_market_reopen_listing(
                &owner,
                &listing_id,
                reopen_existing_request(&idle_seat_id, 2, free_seat()),
            )
            .await
            .expect("restore idle seat around active rental");
        let after: (String, String, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, rented.status, idle.status, idle.offer_revision
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats rented ON rented.id = sub.seat_id
                 JOIN share_market_seats idle ON idle.id = ?2
                 WHERE sub.id = ?1",
                params![subscription_id, idle_seat_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read active rental after reopen");
        assert_eq!((after.0, after.1), before);
        assert_eq!((after.2.as_str(), after.3), (SEAT_AVAILABLE, 3));
    }

    #[tokio::test]
    async fn reopen_validation_failures_leave_the_listing_fully_stopped() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-reopen-rollback", "owner-reopen-rollback@example.com");
        insert_share(
            &store,
            "share-reopen-rollback",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, seat_id) =
            create_listing(&store, &owner, "share-reopen-rollback", free_seat()).await;
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("stop listing before validation tests");
        let expected_state = ("closed".into(), SEAT_DISABLED.into(), 2);

        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET free_access = 1 WHERE share_id = 'share-reopen-rollback'",
                [],
            )
            .expect("enable public access");
        assert!(
            store
                .share_market_reopen_listing(
                    &owner,
                    &listing_id,
                    reopen_existing_request(&seat_id, 2, free_seat()),
                )
                .await
                .is_err()
        );
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            expected_state
        );
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET free_access = 0, share_status = 'paused'
                 WHERE share_id = 'share-reopen-rollback'",
                [],
            )
            .expect("make Share inactive");
        assert!(
            store
                .share_market_reopen_listing(
                    &owner,
                    &listing_id,
                    reopen_existing_request(&seat_id, 2, free_seat()),
                )
                .await
                .is_err()
        );
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            expected_state
        );
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET share_status = 'active', owner_email = 'other@example.com'
                 WHERE share_id = 'share-reopen-rollback'",
                [],
            )
            .expect("change Share owner");
        assert!(
            store
                .share_market_reopen_listing(
                    &owner,
                    &listing_id,
                    reopen_existing_request(&seat_id, 2, free_seat()),
                )
                .await
                .is_err()
        );
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            expected_state
        );
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET owner_email = ?2, share_access_policy_version = 0
                 WHERE share_id = ?1",
                params!["share-reopen-rollback", owner.email],
            )
            .expect("downgrade Share contract");
        let upgrade_error = store
            .share_market_reopen_listing(
                &owner,
                &listing_id,
                reopen_existing_request(&seat_id, 2, free_seat()),
            )
            .await
            .expect_err("old Share contract must block reopen");
        assert_eq!(
            upgrade_error.code(),
            Some("share_market_client_upgrade_required")
        );
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            expected_state
        );
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET share_access_policy_version = ?2 WHERE share_id = ?1",
                params!["share-reopen-rollback", i64::from(SHARE_CONTRACT_VERSION)],
            )
            .expect("restore Share contract");

        let now = Utc::now().to_rfc3339();
        let patch_json = serde_json::to_string(&ShareSettingsPatch {
            free_access: Some(true),
            ..ShareSettingsPatch::default()
        })
        .expect("encode pending public access edit");
        store
            .conn
            .lock()
            .await
            .execute(
                "INSERT INTO share_edit_requests (
                    id, share_id, installation_id, owner_email, revision, status,
                    patch_json, created_by_email, created_at, updated_at,
                    applied_at, error_message, retired_at
                 ) VALUES ('reopen-pending-edit', 'share-reopen-rollback',
                           'installation-share-reopen-rollback', ?1, 1, 'pending',
                           ?2, ?1, ?3, ?3, NULL, NULL, NULL)",
                params![owner.email, patch_json, now],
            )
            .expect("insert pending public access edit");
        assert!(
            store
                .share_market_reopen_listing(
                    &owner,
                    &listing_id,
                    reopen_existing_request(&seat_id, 2, free_seat()),
                )
                .await
                .is_err()
        );
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            expected_state
        );
        store
            .conn
            .lock()
            .await
            .execute(
                "DELETE FROM share_edit_requests WHERE id = 'reopen-pending-edit'",
                [],
            )
            .expect("remove pending edit");

        let mut unsupported_period = free_seat();
        unsupported_period.token_period = ShareTokenPeriod::Week;
        assert!(
            store
                .share_market_reopen_listing(
                    &owner,
                    &listing_id,
                    reopen_existing_request(&seat_id, 2, unsupported_period),
                )
                .await
                .is_err()
        );
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            expected_state
        );
        let mut over_capacity = free_seat();
        over_capacity.parallel_limit = Some(4);
        assert!(
            store
                .share_market_reopen_listing(
                    &owner,
                    &listing_id,
                    reopen_existing_request(&seat_id, 2, over_capacity),
                )
                .await
                .is_err()
        );
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            expected_state
        );
        let payment_error = store
            .share_market_reopen_listing(
                &owner,
                &listing_id,
                reopen_existing_request(&seat_id, 2, paid_seat()),
            )
            .await
            .expect_err("paid reopen without payment profile must fail");
        assert_eq!(
            payment_error.code(),
            Some(ERROR_SHARE_MARKET_PAYMENT_PROFILE_REQUIRED)
        );
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            expected_state
        );

        store
            .share_market_reopen_listing(
                &owner,
                &listing_id,
                reopen_existing_request(&seat_id, 2, free_seat()),
            )
            .await
            .expect("valid reopen still succeeds after rejected attempts");
    }

    #[tokio::test]
    async fn retired_seat_cannot_be_republished() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-retired-reopen", "owner-retired-reopen@example.com");
        insert_share(
            &store,
            "share-retired-reopen",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, seat_id) =
            create_listing(&store, &owner, "share-retired-reopen", free_seat()).await;
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("stop listing");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE share_market_seats SET retired_at = ?2 WHERE id = ?1",
                params![seat_id, Utc::now().to_rfc3339()],
            )
            .expect("retire stopped seat");
        let error = store
            .share_market_reopen_listing(
                &owner,
                &listing_id,
                reopen_existing_request(&seat_id, 2, free_seat()),
            )
            .await
            .expect_err("retired seat must not be republished");
        assert_eq!(error.code(), Some(ERROR_SEAT_NOT_REOPENABLE));
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            ("closed".into(), SEAT_DISABLED.into(), 2),
        );
    }

    #[tokio::test]
    async fn old_listing_reopen_and_add_seat_return_coded_conflict_when_another_is_active() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-reopen-conflict", "owner-reopen-conflict@example.com");
        insert_share(
            &store,
            "share-reopen-conflict",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, seat_id) =
            create_listing(&store, &owner, "share-reopen-conflict", free_seat()).await;
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("stop old listing");
        let now = Utc::now().to_rfc3339();
        store
            .conn
            .lock()
            .await
            .execute(
                "INSERT INTO share_market_listings (
                    id, share_id, installation_id, owner_user_id, owner_email,
                    status, created_at, updated_at
                 ) VALUES ('replacement-active', 'share-reopen-conflict',
                           'installation-share-reopen-conflict', ?1, ?2,
                           'active', ?3, ?3)",
                params![owner.user_id, owner.email, now],
            )
            .expect("seed replacement active listing");

        let reopen_error = store
            .share_market_reopen_listing(
                &owner,
                &listing_id,
                reopen_existing_request(&seat_id, 2, free_seat()),
            )
            .await
            .expect_err("old listing must not collide with active replacement");
        assert_eq!(reopen_error.code(), Some(ERROR_OTHER_ACTIVE_LISTING));
        let add_error = store
            .share_market_add_seat(&owner, &listing_id, free_seat())
            .await
            .expect_err("legacy add-seat reopen must return a coded conflict");
        assert_eq!(add_error.code(), Some(ERROR_OTHER_ACTIVE_LISTING));
        assert_eq!(
            stopped_listing_state(&store, &listing_id, &seat_id).await,
            ("closed".into(), SEAT_DISABLED.into(), 2),
        );
    }

    #[tokio::test]
    async fn concurrent_reopen_attempts_activate_the_listing_once() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-reopen-race", "owner-reopen-race@example.com");
        insert_share(
            &store,
            "share-reopen-race",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, seat_id) =
            create_listing(&store, &owner, "share-reopen-race", free_seat()).await;
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("stop listing before concurrent reopen");

        let left = store.share_market_reopen_listing(
            &owner,
            &listing_id,
            reopen_existing_request(&seat_id, 2, free_seat()),
        );
        let right = store.share_market_reopen_listing(
            &owner,
            &listing_id,
            reopen_existing_request(&seat_id, 2, free_seat()),
        );
        let (left, right) = tokio::join!(left, right);
        assert_ne!(left.is_ok(), right.is_ok());
        let error = left.err().or_else(|| right.err()).expect("one conflict");
        assert_eq!(error.code(), Some(ERROR_LISTING_NOT_CLOSED));
        let relisted_events: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT COUNT(*) FROM share_market_events
                 WHERE listing_id = ?1 AND event_type = 'listing_relisted'",
                params![listing_id],
                |row| row.get(0),
            )
            .expect("count relist events");
        assert_eq!(relisted_events, 1);
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
    async fn owner_can_soft_delete_closed_listing_with_only_failed_grants() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-delete-failed", "owner-delete-failed@example.com");
        let renter = session("renter-delete-failed", "renter-delete-failed@example.com");
        insert_share(
            &store,
            "share-delete-failed",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (listing_id, seat_id) =
            create_listing(&store, &owner, "share-delete-failed", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent failed listing seat");
        let now = Utc::now();
        store
            .share_market_reconcile_and_dispatch(now)
            .await
            .expect("dispatch failed listing grant");
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
            .expect("read failed listing edit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                params![edit_id],
            )
            .expect("reject failed listing edit");
            handle_control_edit_ack_with_metadata(
                &conn,
                &edit_id,
                "rejected",
                Some("client rejected managed grant"),
                Some("grant_rejected"),
                Some(false),
                &(now + Duration::seconds(1)).to_rfc3339(),
            )
            .expect("finish failed listing grant");
        }
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("close failed listing");
        let owner_catalog = store
            .share_market_catalog_with_scope(Some(&owner), &[], ShareMarketCatalogScope::Owner)
            .await
            .expect("read failed listing delete capability");
        assert!(
            owner_catalog
                .listings
                .iter()
                .find(|listing| listing.id == listing_id)
                .expect("closed failed listing")
                .can_delete
        );

        store
            .share_market_delete_listing(&owner, &listing_id)
            .await
            .expect("soft-delete failed listing");
        store
            .share_market_delete_listing(&owner, &listing_id)
            .await
            .expect("repeat failed listing cleanup is idempotent");
        let preserved: (Option<String>, String, String, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT listing.deleted_at, seat.status, subscription.status,
                        operation.status,
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = subscription.id)
                 FROM share_market_listings listing
                 JOIN share_market_seats seat ON seat.listing_id = listing.id
                 JOIN share_market_subscriptions subscription
                   ON subscription.id = seat.retired_subscription_id
                 JOIN share_control_operations operation
                   ON operation.subscription_id = subscription.id
                 WHERE listing.id = ?1",
                params![listing_id],
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
            .expect("read preserved failed listing history");
        assert!(preserved.0.is_some());
        assert_eq!(preserved.1, SEAT_DELETED);
        assert_eq!(preserved.2, SUB_GRANT_FAILED);
        assert_eq!(preserved.3, "rejected");
        assert!(preserved.4 >= 2);
        let renter_history = store
            .share_market_catalog_with_scope(Some(&renter), &[], ShareMarketCatalogScope::Renter)
            .await
            .expect("read renter history after listing cleanup");
        assert!(
            renter_history
                .my_subscriptions
                .iter()
                .any(|subscription| subscription.id == subscription_id)
        );
    }

    #[tokio::test]
    async fn closed_listing_with_active_rental_requires_reopen_and_blocks_delete() {
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
        let owner_catalog = store
            .share_market_catalog_with_scope(Some(&owner), &[], ShareMarketCatalogScope::Owner)
            .await
            .expect("read active listing delete capability");
        assert!(
            !owner_catalog
                .listings
                .iter()
                .find(|listing| listing.id == listing_id)
                .expect("closed occupied listing")
                .can_delete
        );
        assert!(matches!(
            store.share_market_delete_listing(&owner, &listing_id).await,
            Err(AppError::Conflict(_))
        ));

        let owned = store
            .share_market_owned_shares(&owner)
            .await
            .expect("owned shares while rental active");
        assert!(owned[0].already_listed);
        assert!(owned[0].has_active_rentals);
        assert_eq!(
            owned[0].reopen_listing_id.as_deref(),
            Some(listing_id.as_str())
        );
        let create_error = store
            .share_market_create_listing(
                &owner,
                CreateListingRequest {
                    share_id: "share-hold".into(),
                    seats: vec![free_seat()],
                },
            )
            .await
            .expect_err("active rental must not allow a replacement listing");
        assert_eq!(create_error.code(), Some(ERROR_REOPEN_REQUIRED));

        let reopened = store
            .share_market_reopen_listing(
                &owner,
                &listing_id,
                ReopenListingRequest {
                    existing_seats: Vec::new(),
                    new_seats: vec![free_seat()],
                },
            )
            .await
            .expect("active rental does not block restoring the original listing");
        assert_eq!(reopened.listing_id, listing_id);
        assert_eq!(reopened.new_seat_ids.len(), 1);
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_ACTIVE_FREE
        );
    }

    #[tokio::test]
    async fn catalog_blocks_rent_when_all_share_app_apis_are_disabled() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-disabled-apps", "owner-disabled-apps@example.com");
        let renter = session("renter-disabled-apps", "renter-disabled-apps@example.com");
        insert_share(
            &store,
            "share-disabled-apps",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) = create_listing(&store, &owner, "share-disabled-apps", free_seat()).await;
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET enabled_codex = 0 WHERE share_id = 'share-disabled-apps'",
                [],
            )
            .expect("disable the only Share App API");

        let catalog = store
            .share_market_catalog(Some(&renter), &["share-disabled-apps-route".into()])
            .await
            .expect("catalog with disabled Share App APIs");
        let listing = &catalog.listings[0];
        assert!(listing.supported_apps.is_empty());
        assert!(listing.app_capabilities.is_empty());
        assert!(!listing.seats[0].can_rent);
        assert_eq!(
            listing.seats[0].rent_block_reason.as_deref(),
            Some("share_unavailable")
        );
        assert!(
            store
                .share_market_create_rent_quote(&renter, &seat_id, None)
                .await
                .is_err()
        );
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
                            usage_rebase: None,
                            usage_quota: None,
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
        assert!(
            blocked_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-b")
                .expect("listing b for blocked prerequisites")
                .seats[0]
                .rent_prerequisites_met
        );
        assert_eq!(
            blocked_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-b")
                .expect("listing b for blocked eligibility")
                .seats[0]
                .eligibility
                .status,
            "access_required"
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
        assert!(
            !granted_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-b")
                .expect("listing b for direct grant prerequisites")
                .seats[0]
                .rent_prerequisites_met
        );
        assert_eq!(
            granted_catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-canrent-b")
                .expect("listing b for direct grant eligibility")
                .seats[0]
                .eligibility
                .status,
            "allowed"
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
        assert_eq!(
            catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-paid-canrent-b")
                .expect("eligible second paid listing")
                .seats[0]
                .eligibility
                .status,
            "allowed"
        );

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_public_credit_policies
                 SET enabled = 0, revision = revision + 1, updated_at = ?2
                 WHERE supplier_user_id = ?1 AND currency = 'USD'",
                params![owner.user_id, now],
            )
            .expect("remove paid credit");
        }
        let catalog = store
            .share_market_catalog(Some(&renter), &active_subdomains)
            .await
            .expect("catalog without credit");
        assert!(!can_rent_second(&catalog));
        assert_eq!(
            catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-paid-canrent-b")
                .expect("second paid listing without credit")
                .seats[0]
                .eligibility
                .status,
            "credit_required"
        );

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_public_credit_policies
                 SET enabled = 1, limit_minor = 50000, revision = revision + 1,
                     updated_at = ?2
                 WHERE supplier_user_id = ?1 AND currency = 'USD'",
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
        assert_eq!(
            catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-paid-canrent-b")
                .expect("second paid listing for overdue buyer")
                .seats[0]
                .eligibility
                .status,
            "buyer_restricted"
        );

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
                 WHERE supplier_user_id = ?1 AND currency = 'USD'",
                params![owner.user_id, now],
            )
            .expect("lower paid credit");
            conn.execute(
                "UPDATE market_credit_accounts
                 SET status = 'active', balance_units = 8640000, updated_at = ?3
                 WHERE buyer_user_id = ?1 AND supplier_user_id = ?2 AND currency = 'USD'",
                params![renter.user_id, owner.user_id, now],
            )
            .expect("raise accrued balance to lowered limit");
        }
        let catalog = store
            .share_market_catalog(Some(&renter), &active_subdomains)
            .await
            .expect("catalog after credit reduction");
        assert!(!can_rent_second(&catalog));
        assert_eq!(
            catalog
                .listings
                .iter()
                .find(|listing| listing.share_id == "share-paid-canrent-b")
                .expect("second paid listing at credit limit")
                .seats[0]
                .eligibility
                .status,
            "credit_limit_reached"
        );
    }

    #[tokio::test]
    async fn public_free_access_blocks_share_market_listing_and_reopen() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-federated", "owner-federated@example.com");
        insert_share(
            &store,
            "share-federated",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        {
            let conn = store.conn.lock().await;
            crate::market_access::configure_open_test_policy(
                &conn,
                &owner,
                "USD",
                50_000,
                &Utc::now().to_rfc3339(),
            );
            conn.execute(
                "UPDATE shares SET free_access = 1 WHERE share_id = 'share-federated'",
                [],
            )
            .expect("enable public free access");
        }
        assert!(matches!(
            store
                .share_market_create_listing(
                    &owner,
                    CreateListingRequest {
                        share_id: "share-federated".into(),
                        seats: vec![free_seat()],
                    },
                )
                .await,
            Err(AppError::Conflict(_))
        ));

        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET free_access = 0 WHERE share_id = 'share-federated'",
                [],
            )
            .expect("disable public free access");
        let (listing_id, _) = create_listing(&store, &owner, "share-federated", free_seat()).await;
        store
            .share_market_close_listing(&owner, &listing_id)
            .await
            .expect("close listing before reopen test");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET free_access = 1 WHERE share_id = 'share-federated'",
                [],
            )
            .expect("re-enable public free access");
        assert!(matches!(
            store
                .share_market_add_seat(&owner, &listing_id, free_seat())
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn share_retirement_cascades_market_state_and_billing() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-delete", "owner-delete@example.com");
        let renter = session("renter-delete", "renter-delete@example.com");
        configure_payment_profile(&store, &owner, "account-delete", "profile-delete").await;
        insert_share(
            &store,
            "share-delete",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let mut seat = paid_seat();
        seat.service_duration_days = Some(10);
        let (listing_id, seat_id) = create_listing(&store, &owner, "share-delete", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent Share before deletion");
        activate_subscription(&store, &subscription_id, Utc::now()).await;

        {
            let conn = store.conn.lock().await;
            crate::store::retire_share_tx(&conn, "share-delete", "installation-share-delete")
                .expect("retire Share transactionally");
        }
        let state: (i64, String, Option<String>, String, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM shares WHERE share_id = 'share-delete'),
                    listing.status, listing.deleted_at, seat.status, sub.status,
                    (SELECT COUNT(*) FROM share_control_operations operation
                     WHERE operation.subscription_id = sub.id
                       AND operation.status IN ('pending', 'dispatched'))
                 FROM share_market_listings listing
                 INNER JOIN share_market_seats seat ON seat.listing_id = listing.id
                 INNER JOIN share_market_subscriptions sub ON sub.seat_id = seat.id
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
            .expect("read retired Share market state");
        assert_eq!(state.0, 0);
        assert_eq!(state.1, "closed");
        assert!(state.2.is_some());
        assert_eq!(state.3, SEAT_DELETED);
        assert_eq!(state.4, SUB_RELEASED);
        assert_eq!(state.5, 0);
        let contract_state: (String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT contract.status, adjustment.refund_bps
                 FROM market_service_contracts contract
                 INNER JOIN market_contract_adjustments adjustment
                   ON adjustment.contract_id = contract.id
                  AND adjustment.kind = 'supplier_early_termination_refund'
                 WHERE contract.product_kind = 'share' AND contract.product_ref = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read terminated Share contract refund");
        assert_eq!(contract_state.0, "terminated");
        assert_eq!(contract_state.1, 10_000);
    }

    #[tokio::test]
    async fn installation_takeover_refunds_active_paid_fixed_rental_once() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-takeover-refund", "owner-takeover-refund@example.com");
        let renter = session(
            "renter-takeover-refund",
            "renter-takeover-refund@example.com",
        );
        let (subscription_id, seat_id, activated_at) =
            create_active_paid_fixed_subscription(&store, &owner, &renter, "share-takeover-refund")
                .await;
        let interrupted_at = activated_at + Duration::days(2);
        {
            let conn = store.conn.lock().await;
            terminate_installation_for_takeover_tx(
                &conn,
                "installation-share-takeover-refund",
                "client_subdomain_takeover",
                &interrupted_at.to_rfc3339(),
            )
            .expect("terminate takeover Share market rental");
            terminate_installation_for_takeover_tx(
                &conn,
                "installation-share-takeover-refund",
                "client_subdomain_takeover",
                &interrupted_at.to_rfc3339(),
            )
            .expect("repeat takeover Share market termination");
        }
        let state: (String, String, String, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT subscription.status, seat.status, contract.status,
                        adjustment.refund_bps,
                        (SELECT COUNT(*) FROM market_contract_adjustments duplicate
                         WHERE duplicate.product_kind = 'share'
                           AND duplicate.product_ref = subscription.id)
                 FROM share_market_subscriptions subscription
                 INNER JOIN share_market_seats seat ON seat.id = subscription.seat_id
                 INNER JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = subscription.id
                 INNER JOIN market_contract_adjustments adjustment
                   ON adjustment.contract_id = contract.id
                  AND adjustment.kind = 'supplier_early_termination_refund'
                 WHERE subscription.id = ?1 AND seat.id = ?2",
                params![subscription_id, seat_id],
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
            .expect("read takeover termination refund");
        assert_eq!(state.0, SUB_RELEASED);
        assert_eq!(state.1, SEAT_DELETED);
        assert_eq!(state.2, "terminated");
        assert_eq!(state.3, 10_000);
        assert_eq!(state.4, 1);
    }

    #[tokio::test]
    async fn share_owner_rebind_ends_old_billing_and_transfers_control() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-transfer", "owner-transfer@example.com");
        let new_owner = session("new-owner-transfer", "new-owner-transfer@example.com");
        let renter = session("renter-transfer", "renter-transfer@example.com");
        configure_payment_profile(&store, &owner, "account-transfer", "profile-transfer").await;
        insert_share(
            &store,
            "share-transfer",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let mut seat = paid_seat();
        seat.service_duration_days = Some(10);
        let (listing_id, seat_id) = create_listing(&store, &owner, "share-transfer", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent Share before owner transfer");
        activate_subscription(&store, &subscription_id, Utc::now()).await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET owner_email = ?2 WHERE share_id = ?1",
                params!["share-transfer", new_owner.email],
            )
            .expect("transfer Share descriptor owner");
            rebind_share_market_owner_tx(
                &conn,
                "share-transfer",
                &new_owner.user_id,
                &new_owner.email,
                &now,
            )
            .expect("rebind Share market owner");
        }
        let state: (String, String, String, String, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT listing.owner_user_id, listing.owner_email, sub.owner_user_id,
                        sub.status, contract.status, adjustment.refund_bps
                 FROM share_market_listings listing
                 INNER JOIN share_market_subscriptions sub ON sub.listing_id = listing.id
                 INNER JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
                 INNER JOIN market_contract_adjustments adjustment
                   ON adjustment.contract_id = contract.id
                  AND adjustment.kind = 'supplier_early_termination_refund'
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
            .expect("read rebound Share market state");
        assert_eq!(state.0, new_owner.user_id);
        assert_eq!(state.1, new_owner.email);
        assert_eq!(state.2, new_owner.user_id);
        assert_eq!(state.3, SUB_REVOKE_PENDING);
        assert_eq!(state.4, "terminated");
        assert_eq!(state.5, 10_000);
    }

    #[tokio::test]
    async fn grant_retry_exhaustion_exposes_failure_code_attempts_and_cleanup() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-grant-exhausted", "owner-grant-exhausted@example.com");
        let renter = session(
            "renter-grant-exhausted",
            "renter-grant-exhausted@example.com",
        );
        let outsider = session(
            "outsider-grant-exhausted",
            "outsider-grant-exhausted@example.com",
        );
        insert_share(
            &store,
            "share-grant-exhausted",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-grant-exhausted", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent exhausted grant seat");
        let mut dispatched_at = Utc::now();
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(dispatched_at)
                .await
                .expect("dispatch initial exhausted grant")
                .len(),
            1
        );
        for attempt in 1..=MAX_CONTROL_ATTEMPTS {
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
                .expect("read exhausted grant edit");
            let rejected_at = dispatched_at + Duration::seconds(1);
            {
                let conn = store.conn.lock().await;
                conn.execute(
                    "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                    params![edit_id],
                )
                .expect("reject exhausted grant edit");
                handle_control_edit_ack_with_metadata(
                    &conn,
                    &edit_id,
                    "rejected",
                    Some("temporary grant failure"),
                    Some("temporary_grant_failure"),
                    Some(true),
                    &rejected_at.to_rfc3339(),
                )
                .expect("record retryable grant failure");
            }
            if attempt == MAX_CONTROL_ATTEMPTS {
                break;
            }
            let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(0);
            let delay = CONTROL_RETRY_BASE_SECS
                .saturating_mul(2_i64.saturating_pow(exponent))
                .min(CONTROL_RETRY_MAX_SECS);
            dispatched_at = rejected_at + Duration::seconds(delay);
            assert_eq!(
                store
                    .share_market_reconcile_and_dispatch(dispatched_at)
                    .await
                    .expect("dispatch retried grant")
                    .len(),
                1
            );
        }

        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_GRANT_FAILED
        );
        let catalog = store
            .share_market_catalog_with_scope(Some(&owner), &[], ShareMarketCatalogScope::Owner)
            .await
            .expect("read exhausted grant failure details");
        let failed_seat = catalog
            .listings
            .iter()
            .flat_map(|listing| &listing.seats)
            .find(|seat| seat.id == seat_id)
            .expect("exhausted grant seat");
        assert!(failed_seat.can_delete);
        let subscription = failed_seat.subscription.as_ref().expect("failed grant");
        assert_eq!(
            subscription.failure_code.as_deref(),
            Some("temporary_grant_failure")
        );
        assert_eq!(
            subscription.grant_attempts,
            Some(u32::try_from(MAX_CONTROL_ATTEMPTS).unwrap())
        );
        assert_eq!(
            subscription.release_reason.as_deref(),
            Some("temporary grant failure")
        );
        let outsider_catalog = store
            .share_market_catalog_with_scope(Some(&outsider), &[], ShareMarketCatalogScope::Visible)
            .await
            .expect("read failed grant as unrelated buyer");
        let outsider_subscription = outsider_catalog
            .listings
            .iter()
            .flat_map(|listing| &listing.seats)
            .find(|seat| seat.id == seat_id)
            .and_then(|seat| seat.subscription.as_ref())
            .expect("failed grant remains visible without private diagnostics");
        assert!(outsider_subscription.failure_code.is_none());
        assert!(outsider_subscription.grant_attempts.is_none());
        assert!(outsider_subscription.release_reason.is_none());
    }

    #[tokio::test]
    async fn revoke_retries_back_off_then_dead_letter_and_alert() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-dead-letter", "owner-dead-letter@example.com");
        let renter = session("renter-dead-letter", "renter-dead-letter@example.com");
        insert_share(
            &store,
            "share-dead-letter",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let mut seat = free_seat();
        seat.service_duration_days = None;
        let (_listing_id, seat_id) =
            create_listing(&store, &owner, "share-dead-letter", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent dead-letter Share seat");
        activate_subscription(&store, &subscription_id, Utc::now()).await;
        store
            .share_market_request_release(&owner, &subscription_id, true, false)
            .await
            .expect("request revoke before retry exhaustion");
        let mut dispatched_at = Utc::now() + Duration::seconds(1);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(dispatched_at)
                .await
                .expect("dispatch initial revoke")
                .len(),
            1
        );

        for attempt in 1..=MAX_CONTROL_ATTEMPTS {
            let edit_id: String = store
                .conn
                .lock()
                .await
                .query_row(
                    "SELECT edit_id FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = 'revoke'",
                    params![subscription_id],
                    |row| row.get(0),
                )
                .expect("read retry revoke edit");
            let rejected_at = dispatched_at + Duration::seconds(1);
            {
                let conn = store.conn.lock().await;
                conn.execute(
                    "UPDATE share_edit_requests SET status = 'rejected' WHERE id = ?1",
                    params![edit_id],
                )
                .expect("reject retry revoke edit");
                handle_control_edit_ack_with_metadata(
                    &conn,
                    &edit_id,
                    "rejected",
                    Some("temporary control failure"),
                    Some("temporary_failure"),
                    Some(true),
                    &rejected_at.to_rfc3339(),
                )
                .expect("record retryable revoke failure");
            }
            if attempt == MAX_CONTROL_ATTEMPTS {
                break;
            }
            let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(0);
            let delay = CONTROL_RETRY_BASE_SECS
                .saturating_mul(2_i64.saturating_pow(exponent))
                .min(CONTROL_RETRY_MAX_SECS);
            assert!(
                store
                    .share_market_reconcile_and_dispatch(
                        rejected_at + Duration::seconds(delay - 1),
                    )
                    .await
                    .expect("respect revoke retry backoff")
                    .is_empty()
            );
            dispatched_at = rejected_at + Duration::seconds(delay);
            assert_eq!(
                store
                    .share_market_reconcile_and_dispatch(dispatched_at)
                    .await
                    .expect("dispatch backed-off revoke")
                    .len(),
                1
            );
        }

        let terminal: (String, i64, Option<String>, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT operation.status, operation.attempts, operation.dead_lettered_at,
                        sub.status,
                        (SELECT COUNT(*) FROM operator_alert_signal_outbox signal
                         WHERE signal.source_event_id = 'share-control-dead-letter:' || operation.id)
                 FROM share_control_operations operation
                 INNER JOIN share_market_subscriptions sub ON sub.id = operation.subscription_id
                 WHERE operation.subscription_id = ?1 AND operation.action = 'revoke'",
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
            .expect("read dead-lettered revoke");
        assert_eq!(terminal.0, "rejected");
        assert_eq!(terminal.1, MAX_CONTROL_ATTEMPTS);
        assert!(terminal.2.is_some());
        assert_eq!(terminal.3, SUB_REVOKE_FAILED);
        assert_eq!(terminal.4, 1);

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET status = 'billing_control_failed' WHERE id = ?1",
                params![subscription_id],
            )
            .expect("seed exhausted billing suspension");
        }
        assert!(
            store
                .share_market_reconcile_and_dispatch(dispatched_at + Duration::days(1))
                .await
                .expect("reconcile exhausted billing suspension")
                .is_empty()
        );
        let preserved: (String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, COUNT(operation.id)
                 FROM share_market_subscriptions sub
                 LEFT JOIN share_control_operations operation
                   ON operation.subscription_id = sub.id AND operation.action = 'revoke'
                 WHERE sub.id = ?1
                 GROUP BY sub.id",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read preserved dead-lettered billing suspension");
        assert_eq!(preserved.0, SUB_BILLING_CONTROL_FAILED);
        assert_eq!(preserved.1, 1);

        let expires_at = dispatched_at + Duration::days(2);
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE share_market_subscriptions
                 SET service_duration_days = 1, expires_at = ?2 WHERE id = ?1",
                params![subscription_id, expires_at.to_rfc3339()],
            )
            .expect("set service expiry after revoke dead-letter");
        assert!(
            store
                .share_market_reconcile_and_dispatch(expires_at)
                .await
                .expect("expire dead-lettered Share revoke")
                .is_empty()
        );
        let expired: (String, String, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, sub.release_reason, seat.status,
                        (SELECT COUNT(*) FROM share_market_events
                         WHERE subscription_id = sub.id AND event_type = 'service_term_expired')
                 FROM share_market_subscriptions sub
                 JOIN share_market_seats seat ON seat.id = sub.seat_id
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read expired dead-lettered Share revoke");
        assert_eq!(
            expired,
            (
                SUB_REVOKE_FAILED.into(),
                "service_term_expired".into(),
                "revoking".into(),
                1,
            )
        );
    }

    #[tokio::test]
    async fn failed_free_and_paid_grants_recover_original_operation_seat_and_contract() {
        for paid in [false, true] {
            let store = AppStore::new_in_memory_for_tests().expect("test store");
            let suffix = if paid { "paid" } else { "free" };
            let owner = session(
                &format!("owner-retry-{suffix}"),
                &format!("owner-retry-{suffix}@example.com"),
            );
            let renter = session(
                &format!("renter-retry-{suffix}"),
                &format!("renter-retry-{suffix}@example.com"),
            );
            if paid {
                configure_payment_profile(&store, &owner, "refund-account", "profile-v1").await;
            }
            let share_id = format!("share-retry-{suffix}");
            insert_share(&store, &share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
            let (_, seat_id) = create_listing(
                &store,
                &owner,
                &share_id,
                if paid { paid_seat() } else { free_seat() },
            )
            .await;
            let subscription_id = store
                .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
                .await
                .expect("rent recoverable seat");
            let (operation_id, edit_ids) =
                exhaust_control_operation(&store, &subscription_id, "upsert", Utc::now()).await;
            assert_eq!(
                subscription_status(&store, &subscription_id).await,
                SUB_GRANT_FAILED
            );
            assert!(matches!(
                store
                    .share_market_requeue_control_dead_letter(&owner, &operation_id)
                    .await,
                Err(AppError::Conflict(_))
            ));

            let terminated_contract = if paid {
                Some(
                    store
                        .conn
                        .lock()
                        .await
                        .query_row(
                            "SELECT id FROM market_service_contracts
                             WHERE product_kind = 'share' AND product_ref = ?1
                               AND status = 'terminated'",
                            params![subscription_id],
                            |row| row.get::<_, String>(0),
                        )
                        .expect("read terminated initial billing reservation"),
                )
            } else {
                None
            };

            store
                .share_market_retry_grant(&owner, &subscription_id)
                .await
                .expect("retry failed grant");
            store
                .share_market_retry_grant(&owner, &subscription_id)
                .await
                .expect("repeat grant retry idempotently");

            let state: (
                String,
                String,
                Option<String>,
                Option<String>,
                String,
                i64,
                Option<String>,
                Option<String>,
            ) = store
                .conn
                .lock()
                .await
                .query_row(
                    "SELECT sub.status, seat.status, seat.current_subscription_id,
                            seat.retired_subscription_id, operation.status,
                            operation.attempts, operation.edit_id, operation.dead_lettered_at
                     FROM share_market_subscriptions sub
                     JOIN share_market_seats seat ON seat.id = sub.seat_id
                     JOIN share_control_operations operation
                       ON operation.subscription_id = sub.id AND operation.action = 'upsert'
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
                            row.get(6)?,
                            row.get(7)?,
                        ))
                    },
                )
                .expect("read restored grant state");
            assert_eq!(state.0, SUB_GRANT_PENDING);
            assert_eq!(state.1, SEAT_RESERVED);
            assert_eq!(state.2.as_deref(), Some(subscription_id.as_str()));
            assert!(state.3.is_none());
            assert_eq!(state.4, "pending");
            assert_eq!(state.5, 0);
            assert!(state.6.is_none());
            assert!(state.7.is_none());

            let final_old_edit = edit_ids.last().expect("final failed edit");
            let old_edit: (String, Option<String>) = store
                .conn
                .lock()
                .await
                .query_row(
                    "SELECT status, dead_lettered_at FROM share_edit_requests WHERE id = ?1",
                    params![final_old_edit],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("read retained failed edit audit");
            assert_eq!(old_edit.0, "rejected");
            assert!(old_edit.1.is_none());

            let contracts: Vec<(String, String, Option<String>)> = store
                .conn
                .lock()
                .await
                .prepare(
                    "SELECT id, status, replacement_of FROM market_service_contracts
                     WHERE product_kind = 'share' AND product_ref = ?1 ORDER BY created_at, id",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map(params![subscription_id], |row| {
                            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                        })?
                        .collect()
                })
                .expect("read retried billing contracts");
            if let Some(terminated_contract) = terminated_contract {
                assert_eq!(contracts.len(), 2);
                assert!(contracts.iter().any(|contract| {
                    contract.0 == terminated_contract && contract.1 == "terminated"
                }));
                let replacement = contracts
                    .iter()
                    .find(|contract| contract.1 != "terminated")
                    .expect("active replacement billing reservation");
                assert_eq!(replacement.1, "pending_activation");
                assert_eq!(replacement.2.as_deref(), Some(terminated_contract.as_str()));
            } else {
                assert!(contracts.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn failed_grant_retry_rechecks_frozen_contract_and_direct_access_conflicts() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-retry-conflict", "owner-retry-conflict@example.com");
        let renter = session("renter-retry-conflict", "renter-retry-conflict@example.com");
        insert_share(
            &store,
            "share-retry-conflict",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) =
            create_listing(&store, &owner, "share-retry-conflict", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent conflict test seat");
        let (operation_id, _) =
            exhaust_control_operation(&store, &subscription_id, "upsert", Utc::now()).await;

        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET parallel_limit = 1 WHERE share_id = 'share-retry-conflict'",
                [],
            )
            .expect("reduce Share capacity");
        let incompatible = store
            .share_market_retry_grant(&owner, &subscription_id)
            .await
            .expect_err("incompatible frozen contract must not recover");
        assert_eq!(
            incompatible.code(),
            Some("share_market_contract_incompatible")
        );

        let direct_grant = ShareUserGrant {
            email: renter.email.clone(),
            role: "shareto".into(),
            active: true,
            entitlement_id: Some("different-entitlement".into()),
            ..ShareUserGrant::default()
        };
        let grants = serde_json::to_string(&BTreeMap::from([(renter.email.clone(), direct_grant)]))
            .expect("serialize conflicting direct grant");
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET parallel_limit = 3, user_grants_json = ?2
                 WHERE share_id = ?1",
                params!["share-retry-conflict", grants],
            )
            .expect("seed conflicting direct grant");
        assert!(matches!(
            store
                .share_market_retry_grant(&owner, &subscription_id)
                .await,
            Err(AppError::Conflict(_))
        ));
        let unchanged: (String, i64, Option<String>) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, attempts, dead_lettered_at FROM share_control_operations
                 WHERE id = ?1",
                params![operation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read unchanged failed operation");
        assert_eq!(unchanged.0, "rejected");
        assert_eq!(unchanged.1, MAX_CONTROL_ATTEMPTS);
        assert!(unchanged.2.is_none());
    }

    #[tokio::test]
    async fn revoke_dead_letter_page_requeues_safely_and_preserves_old_edit_audit() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-requeue", "owner-requeue@example.com");
        let renter = session("renter-requeue", "renter-requeue@example.com");
        let admin = session("admin-requeue", "admin-requeue@example.com");
        insert_share(
            &store,
            "share-requeue",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let mut seat = free_seat();
        seat.service_duration_days = None;
        let (_, seat_id) = create_listing(&store, &owner, "share-requeue", seat).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent requeue test seat");
        let base = Utc::now();
        activate_subscription(&store, &subscription_id, base).await;
        store
            .share_market_request_release(&owner, &subscription_id, true, false)
            .await
            .expect("request revoke");
        let dispatched_at = Utc::now() + Duration::seconds(1);
        let (operation_id, edit_ids) =
            exhaust_control_operation(&store, &subscription_id, "revoke", dispatched_at).await;

        let page = store
            .share_market_control_dead_letters(None, 1)
            .await
            .expect("read revoke dead letters");
        assert_eq!(page.summary.dead_lettered, 1);
        assert_eq!(page.operations.len(), 1);
        assert_eq!(page.operations[0].id, operation_id);
        assert!(page.operations[0].can_requeue);
        assert_eq!(
            page.operations[0].error_code.as_deref(),
            Some("temporary_control_failure")
        );

        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE share_market_subscriptions SET status = 'billing_control_failed'
                 WHERE id = ?1",
                params![subscription_id],
            )
            .expect("seed failed billing suspension state");
        store
            .share_market_requeue_control_dead_letter(&admin, &operation_id)
            .await
            .expect("requeue revoke dead letter");

        let state: (String, i64, Option<String>, Option<String>, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT operation.status, operation.attempts, operation.edit_id,
                        operation.dead_lettered_at, sub.status
                 FROM share_control_operations operation
                 JOIN share_market_subscriptions sub ON sub.id = operation.subscription_id
                 WHERE operation.id = ?1",
                params![operation_id],
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
            .expect("read requeued revoke");
        assert_eq!(state.0, "pending");
        assert_eq!(state.1, 0);
        assert!(state.2.is_none());
        assert!(state.3.is_none());
        assert_eq!(state.4, SUB_BILLING_SUSPEND_PENDING);

        let old_edit: (String, Option<String>) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, dead_lettered_at FROM share_edit_requests WHERE id = ?1",
                params![edit_ids.last().expect("last rejected revoke edit")],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read retained revoke edit");
        assert_eq!(old_edit.0, "rejected");
        assert!(old_edit.1.is_some());
        let resolved_alerts: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT COUNT(*) FROM operator_alert_signal_outbox
                 WHERE source_event_id = ?1 AND transition = 'resolved'",
                params![format!("share-control-dead-letter-requeued:{operation_id}")],
                |row| row.get(0),
            )
            .expect("count resolved dead-letter alerts");
        assert_eq!(resolved_alerts, 1);
        assert!(matches!(
            store
                .share_market_requeue_control_dead_letter(&admin, &operation_id)
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn subscription_history_cursor_keeps_all_active_and_pages_terminal_rows_once() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-history", "owner-history@example.com");
        let renter = session("renter-history", "renter-history@example.com");
        let base = Utc::now() - Duration::days(1);
        let mut expected_terminal = Vec::new();
        for index in 0..5 {
            let share_id = format!("share-history-{index}");
            insert_share(&store, &share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
            let (_, seat_id) = create_listing(&store, &owner, &share_id, free_seat()).await;
            let subscription_id = store
                .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
                .await
                .expect("rent terminal history seat");
            let created_at = base - Duration::minutes(i64::from(index / 2));
            store
                .conn
                .lock()
                .await
                .execute(
                    "UPDATE share_market_subscriptions
                     SET status = 'released', created_at = ?2, updated_at = ?2, released_at = ?2
                     WHERE id = ?1",
                    params![subscription_id, created_at.to_rfc3339()],
                )
                .expect("seed terminal history subscription");
            expected_terminal.push((created_at.to_rfc3339(), subscription_id));
        }
        insert_share(
            &store,
            "share-history-newly-terminal",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, newly_terminal_seat) =
            create_listing(&store, &owner, "share-history-newly-terminal", free_seat()).await;
        let newly_terminal_id = store
            .share_market_rent_seat(
                &renter,
                &newly_terminal_seat,
                RentSeatRequest { offer_revision: 1 },
            )
            .await
            .expect("rent newly terminal history seat");
        let newly_released_at = base + Duration::hours(1);
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE share_market_subscriptions
                 SET status = 'released', created_at = ?2, updated_at = ?3, released_at = ?3
                 WHERE id = ?1",
                params![
                    newly_terminal_id,
                    (base - Duration::days(30)).to_rfc3339(),
                    newly_released_at.to_rfc3339()
                ],
            )
            .expect("seed old subscription that only recently became terminal");
        expected_terminal.push((newly_released_at.to_rfc3339(), newly_terminal_id));
        expected_terminal.sort_by(|left, right| right.cmp(left));

        insert_share(
            &store,
            "share-history-active",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, active_seat) =
            create_listing(&store, &owner, "share-history-active", free_seat()).await;
        let active_id = store
            .share_market_rent_seat(&renter, &active_seat, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent active history seat");

        let mut cursor = None;
        let mut observed_terminal = Vec::new();
        loop {
            let page = store
                .share_market_subscriptions_page(&renter, &[], cursor.as_deref(), 2)
                .await
                .expect("read subscription history page");
            assert_eq!(
                page.subscriptions
                    .iter()
                    .filter(|subscription| subscription.id == active_id)
                    .count(),
                1
            );
            for subscription in page
                .subscriptions
                .iter()
                .filter(|subscription| subscription.status == SUB_RELEASED)
            {
                assert!(!observed_terminal.contains(&subscription.id));
                observed_terminal.push(subscription.id.clone());
            }
            let Some(next) = page.next_cursor else {
                break;
            };
            cursor = Some(next);
        }
        assert_eq!(
            observed_terminal,
            expected_terminal
                .into_iter()
                .map(|(_, id)| id)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn supplier_termination_commit_honors_quote_snapshot_and_request_fingerprint() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session(
            "owner-termination-quote",
            "owner-termination-quote@example.com",
        );
        let renter = session(
            "renter-termination-quote",
            "renter-termination-quote@example.com",
        );
        let (subscription_id, _, _) = create_active_paid_fixed_subscription(
            &store,
            &owner,
            &renter,
            "share-termination-quote",
        )
        .await;
        let evaluated_near = Utc::now();
        let original_start = evaluated_near - Duration::days(2);
        let original_end = original_start + Duration::days(10);
        {
            let conn = store.conn.lock().await;
            let (contract_id, account_id): (String, String) = conn
                .query_row(
                    "SELECT id, account_id FROM market_service_contracts
                     WHERE product_kind = 'share' AND product_ref = ?1
                       AND status != 'terminated'",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("read quoted termination contract");
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET service_started_at = ?2, activated_at = ?2, expires_at = ?3
                 WHERE id = ?1",
                params![
                    subscription_id,
                    original_start.to_rfc3339(),
                    original_end.to_rfc3339()
                ],
            )
            .expect("set quoted termination interval");
            conn.execute(
                "UPDATE market_service_contracts
                 SET status = 'billing_suspended', trial_seconds_remaining = 0,
                     service_started_at = ?2, activated_at = ?2,
                     last_evaluated_at = ?3, suspended_at = ?3, updated_at = ?3
                 WHERE id = ?1",
                params![
                    contract_id,
                    original_start.to_rfc3339(),
                    evaluated_near.to_rfc3339()
                ],
            )
            .expect("freeze quoted termination billing");
            conn.execute(
                "INSERT INTO market_accrual_entries (
                    id, account_id, contract_id, interval_id, currency,
                    daily_rate_minor, billable_seconds, amount_units, status,
                    invoice_id, created_at, updated_at, credited_units
                 ) VALUES ('quoted-termination-accrual', ?1, ?2,
                           'quoted-termination-interval', 'USD', 1200, 2,
                           172800, 'unbilled', NULL, ?3, ?3, 0)",
                params![account_id, contract_id, evaluated_near.to_rfc3339()],
            )
            .expect("seed quoted refundable balance");
            conn.execute(
                "UPDATE market_credit_accounts SET balance_units = 172800 WHERE id = ?1",
                params![account_id],
            )
            .expect("seed quoted account balance");
        }

        let superseded_quote = store
            .share_market_create_termination_quote(&owner, &subscription_id)
            .await
            .expect("create first termination quote");
        let quote = store
            .share_market_create_termination_quote(&owner, &subscription_id)
            .await
            .expect("replace termination quote");
        assert_eq!(quote.calculation.refund_bps, 10_000);
        assert_eq!(quote.calculation.amount_units, 172_800);

        let crossed_start = evaluated_near - Duration::days(6);
        let crossed_end = crossed_start + Duration::days(10);
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET service_started_at = ?2, activated_at = ?2, expires_at = ?3
                 WHERE id = ?1",
                params![
                    subscription_id,
                    crossed_start.to_rfc3339(),
                    crossed_end.to_rfc3339()
                ],
            )
            .expect("cross refund threshold after quote");
            conn.execute(
                "UPDATE market_service_contracts
                 SET service_started_at = ?2, activated_at = ?2 WHERE product_ref = ?1",
                params![subscription_id, crossed_start.to_rfc3339()],
            )
            .expect("cross billing refund threshold after quote");
        }

        let committed = store
            .share_market_commit_termination(
                &owner,
                &subscription_id,
                &quote.id,
                "quoted-termination-request",
                false,
            )
            .await
            .expect("commit quoted supplier termination");
        assert!(!committed.replayed);
        assert_eq!(committed.adjustment.calculation, quote.calculation);
        assert_eq!(committed.adjustment.calculation.refund_bps, 10_000);
        assert_eq!(committed.adjustment.calculation.amount_units, 172_800);

        let replayed = store
            .share_market_commit_termination(
                &owner,
                &subscription_id,
                &quote.id,
                "quoted-termination-request",
                false,
            )
            .await
            .expect("replay exact supplier termination request");
        assert!(replayed.replayed);
        assert_eq!(replayed.adjustment.id, committed.adjustment.id);
        assert!(matches!(
            store
                .share_market_commit_termination(
                    &owner,
                    &subscription_id,
                    &quote.id,
                    "quoted-termination-request",
                    true,
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            store
                .share_market_commit_termination(
                    &owner,
                    &subscription_id,
                    &superseded_quote.id,
                    "quoted-termination-request",
                    false,
                )
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn contract_integrity_drift_suspends_billing_and_repairs_within_window() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session(
            "owner-integrity-repair",
            "owner-integrity-repair@example.com",
        );
        let renter = session(
            "renter-integrity-repair",
            "renter-integrity-repair@example.com",
        );
        let (subscription_id, _, base) = create_active_paid_fixed_subscription(
            &store,
            &owner,
            &renter,
            "share-integrity-repair",
        )
        .await;
        let violated_at = base + Duration::minutes(1);
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_service_contracts
                 SET status = 'active', trial_seconds_remaining = 0,
                     last_evaluated_at = ?2, updated_at = ?2
                 WHERE product_kind = 'share' AND product_ref = ?1",
                params![subscription_id, base.to_rfc3339()],
            )
            .expect("prepare billable integrity contract");
            conn.execute(
                "UPDATE shares SET parallel_limit = 1
                 WHERE share_id = 'share-integrity-repair'",
                [],
            )
            .expect("drift Share capacity below frozen contract");
            reconcile_share_contract_integrity_tx(&conn, "share-integrity-repair", violated_at)
                .expect("detect Share contract drift");
            let state: (String, String, String, String) = conn
                .query_row(
                    "SELECT sub.status, sub.integrity_state, contract.status,
                            contract.desired_control_state
                     FROM share_market_subscriptions sub
                     JOIN market_service_contracts contract
                       ON contract.product_kind = 'share' AND contract.product_ref = sub.id
                     WHERE sub.id = ?1",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("read integrity suspension state");
            assert_eq!(state.0, SUB_BILLING_SUSPEND_PENDING);
            assert_eq!(state.1, "violated");
            assert_eq!(state.2, "billing_suspended");
            assert_eq!(state.3, "suspended");
        }

        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(violated_at + Duration::seconds(1))
                .await
                .expect("dispatch integrity revoke")
                .len(),
            1
        );
        clear_entitlements(&store, "share-integrity-repair").await;
        let repair_at = violated_at + Duration::hours(1);
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET parallel_limit = 3
                 WHERE share_id = 'share-integrity-repair'",
                [],
            )
            .expect("repair Share capacity");
        assert!(
            store
                .share_market_reconcile_and_dispatch(repair_at)
                .await
                .expect("confirm integrity isolation")
                .is_empty()
        );
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_BILLING_SUSPENDED
        );
        let resume_at = repair_at + Duration::seconds(1);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(resume_at)
                .await
                .expect("dispatch repaired grant")
                .len(),
            1
        );
        let suspended_before_grant: (String, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT contract.status, contract.last_evaluated_at, account.balance_units
                 FROM market_service_contracts contract
                 JOIN market_credit_accounts account ON account.id = contract.account_id
                 WHERE contract.product_kind = 'share' AND contract.product_ref = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read billing state before restored grant");
        assert_eq!(suspended_before_grant.0, "billing_suspended");
        set_entitlement_from_stored_policy(&store, &subscription_id, resume_at).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(resume_at)
                .await
                .expect("confirm repaired grant")
                .is_empty()
        );
        let completed_at = resume_at + Duration::seconds(1);
        assert!(
            store
                .share_market_reconcile_and_dispatch(completed_at)
                .await
                .expect("complete integrity remediation")
                .is_empty()
        );

        let final_state: (String, String, String, String, i64, i64, i64, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, sub.integrity_state, contract.status,
                        contract.desired_control_state,
                        (SELECT COUNT(*) FROM market_contract_adjustments adjustment
                         WHERE adjustment.product_ref = sub.id),
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = sub.id
                           AND event.event_type = 'contract_integrity_violated'),
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = sub.id
                           AND event.event_type = 'contract_integrity_restored'),
                        contract.last_evaluated_at, account.balance_units
                 FROM share_market_subscriptions sub
                 JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
                  AND contract.status != 'terminated'
                 JOIN market_credit_accounts account ON account.id = contract.account_id
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
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .expect("read repaired contract state");
        assert_eq!(final_state.0, SUB_ACTIVE_POSTPAID);
        assert_eq!(final_state.1, "compatible");
        assert!(matches!(final_state.2.as_str(), "trial" | "active"));
        assert_eq!(final_state.3, "active");
        assert_eq!((final_state.4, final_state.5, final_state.6), (0, 1, 1));
        assert_eq!(parse_time(&final_state.7).unwrap(), completed_at);
        assert_eq!(final_state.8, suspended_before_grant.2);

        let billed_at = completed_at + Duration::seconds(10);
        record_share_health(&store, "share-integrity-repair", billed_at).await;
        store
            .market_billing_reconcile(billed_at)
            .await
            .expect("bill only service time after integrity restoration");
        let resumed_balance: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT account.balance_units
                 FROM market_service_contracts contract
                 JOIN market_credit_accounts account ON account.id = contract.account_id
                 WHERE contract.product_kind = 'share' AND contract.product_ref = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read post-remediation account balance");
        assert_eq!(
            resumed_balance - suspended_before_grant.2,
            1_200_i64 * 10_i64
        );
    }

    #[tokio::test]
    async fn missing_active_entitlement_is_repaired_without_releasing_paid_contract() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session(
            "owner-integrity-missing-grant",
            "owner-integrity-missing-grant@example.com",
        );
        let renter = session(
            "renter-integrity-missing-grant",
            "renter-integrity-missing-grant@example.com",
        );
        let (subscription_id, _, base) = create_active_paid_fixed_subscription(
            &store,
            &owner,
            &renter,
            "share-integrity-missing-grant",
        )
        .await;
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE market_service_contracts
                 SET status = 'active', trial_seconds_remaining = 0,
                     last_evaluated_at = ?2, updated_at = ?2
                 WHERE product_kind = 'share' AND product_ref = ?1",
                params![subscription_id, base.to_rfc3339()],
            )
            .expect("prepare billable missing-grant contract");
        clear_entitlements(&store, "share-integrity-missing-grant").await;

        let violated_at = base + Duration::minutes(1);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(violated_at)
                .await
                .expect("isolate missing active entitlement")
                .len(),
            1
        );
        let isolated: (String, String, String, String) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, sub.integrity_state, sub.integrity_reason,
                        contract.status
                 FROM share_market_subscriptions sub
                 JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
                 WHERE sub.id = ?1",
                params![subscription_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read missing-grant isolation");
        assert_eq!(isolated.0, SUB_BILLING_SUSPEND_PENDING);
        assert_eq!(isolated.1, "violated");
        assert!(isolated.2.contains("entitlement_missing"));
        assert_eq!(isolated.3, "billing_suspended");

        assert!(
            store
                .share_market_reconcile_and_dispatch(violated_at + Duration::seconds(1))
                .await
                .expect("confirm missing-grant isolation")
                .is_empty()
        );
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_BILLING_SUSPENDED
        );

        let restored_at = violated_at + Duration::seconds(2);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(restored_at)
                .await
                .expect("dispatch replacement entitlement")
                .len(),
            1
        );
        set_entitlement_from_stored_policy(&store, &subscription_id, restored_at).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(restored_at)
                .await
                .expect("confirm replacement entitlement")
                .is_empty()
        );
        assert!(
            store
                .share_market_reconcile_and_dispatch(restored_at + Duration::seconds(1))
                .await
                .expect("complete missing-grant repair")
                .is_empty()
        );

        let repaired: (String, String, String, i64, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, sub.integrity_state, contract.status,
                        (SELECT COUNT(*) FROM market_contract_adjustments adjustment
                         WHERE adjustment.product_ref = sub.id),
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = sub.id
                           AND event.event_type = 'subscription_released'),
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = sub.id
                           AND event.event_type = 'contract_integrity_restored')
                 FROM share_market_subscriptions sub
                 JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
                  AND contract.status != 'terminated'
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
            .expect("read repaired missing-grant contract");
        assert_eq!(repaired.0, SUB_ACTIVE_POSTPAID);
        assert_eq!(repaired.1, "compatible");
        assert_eq!(repaired.2, "active");
        assert_eq!((repaired.3, repaired.4, repaired.5), (0, 0, 1));
    }

    #[tokio::test]
    async fn integrity_repair_does_not_override_a_billing_account_suspension() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session(
            "owner-integrity-billing-block",
            "owner-integrity-billing-block@example.com",
        );
        let renter = session(
            "renter-integrity-billing-block",
            "renter-integrity-billing-block@example.com",
        );
        let (subscription_id, _, activated_at) = create_active_paid_fixed_subscription(
            &store,
            &owner,
            &renter,
            "share-integrity-billing-block",
        )
        .await;
        let violated_at = activated_at + Duration::minutes(1);
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET parallel_limit = 1
                 WHERE share_id = 'share-integrity-billing-block'",
                [],
            )
            .expect("violate Share capacity before billing block");
            reconcile_share_contract_integrity_tx(
                &conn,
                "share-integrity-billing-block",
                violated_at,
            )
            .expect("start integrity remediation before billing block");
        }
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(violated_at + Duration::seconds(1))
                .await
                .expect("dispatch integrity isolation before billing block")
                .len(),
            1
        );
        clear_entitlements(&store, "share-integrity-billing-block").await;
        let isolation_confirmed_at = violated_at + Duration::hours(1);
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET parallel_limit = 3
                 WHERE share_id = 'share-integrity-billing-block'",
                [],
            )
            .expect("repair Share capacity before confirming isolation");
        assert!(
            store
                .share_market_reconcile_and_dispatch(isolation_confirmed_at)
                .await
                .expect("confirm integrity isolation before billing block")
                .is_empty()
        );
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_BILLING_SUSPENDED
        );

        let repaired_at = isolation_confirmed_at + Duration::seconds(1);
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_credit_accounts
                 SET status = 'overdue', updated_at = ?2
                 WHERE id = (SELECT account_id FROM market_service_contracts
                             WHERE product_kind = 'share' AND product_ref = ?1)",
                params![subscription_id, repaired_at.to_rfc3339()],
            )
            .expect("block billing account during integrity remediation");
            conn.execute(
                "UPDATE market_service_contracts
                 SET desired_control_state = 'suspended',
                     control_error = 'payment_overdue', updated_at = ?2
                 WHERE product_kind = 'share' AND product_ref = ?1",
                params![subscription_id, repaired_at.to_rfc3339()],
            )
            .expect("retain billing suspension decision");
        }
        assert!(
            store
                .share_market_reconcile_and_dispatch(repaired_at)
                .await
                .expect("reconcile repaired integrity with blocked billing")
                .is_empty()
        );

        let state: (String, String, String, String, String, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, sub.integrity_state, sub.release_reason,
                        contract.status, contract.desired_control_state,
                        (SELECT COUNT(*) FROM share_control_operations operation
                         WHERE operation.subscription_id = sub.id
                           AND operation.status IN ('pending', 'dispatched')),
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = sub.id
                           AND event.event_type = 'contract_integrity_restored')
                 FROM share_market_subscriptions sub
                 JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
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
                        row.get(6)?,
                    ))
                },
            )
            .expect("read integrity repair blocked by billing state");
        assert_eq!(state.0, SUB_BILLING_SUSPENDED);
        assert_eq!(state.1, "compatible");
        assert_eq!(state.2, "billing_resume_blocked");
        assert_eq!(state.3, "billing_suspended");
        assert_eq!(state.4, "suspended");
        assert_eq!((state.5, state.6), (0, 1));
    }

    #[tokio::test]
    async fn integrity_resume_rechecks_billing_after_the_grant_is_restored() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session(
            "owner-integrity-late-block",
            "owner-integrity-late-block@example.com",
        );
        let renter = session(
            "renter-integrity-late-block",
            "renter-integrity-late-block@example.com",
        );
        let (subscription_id, _, activated_at) = create_active_paid_fixed_subscription(
            &store,
            &owner,
            &renter,
            "share-integrity-late-block",
        )
        .await;
        let violated_at = activated_at + Duration::minutes(1);
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET parallel_limit = 1
                 WHERE share_id = 'share-integrity-late-block'",
                [],
            )
            .expect("violate Share capacity before late billing block");
            reconcile_share_contract_integrity_tx(&conn, "share-integrity-late-block", violated_at)
                .expect("start integrity remediation before late billing block");
        }
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(violated_at + Duration::seconds(1))
                .await
                .expect("dispatch isolation before late billing block")
                .len(),
            1
        );
        clear_entitlements(&store, "share-integrity-late-block").await;
        let repaired_at = violated_at + Duration::hours(1);
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET parallel_limit = 3
                 WHERE share_id = 'share-integrity-late-block'",
                [],
            )
            .expect("repair Share before late billing block");
        assert!(
            store
                .share_market_reconcile_and_dispatch(repaired_at)
                .await
                .expect("confirm isolation before late billing block")
                .is_empty()
        );
        let resume_at = repaired_at + Duration::seconds(1);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(resume_at)
                .await
                .expect("dispatch grant before late billing block")
                .len(),
            1
        );
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_credit_accounts
                 SET status = 'overdue', updated_at = ?2
                 WHERE id = (SELECT account_id FROM market_service_contracts
                             WHERE product_kind = 'share' AND product_ref = ?1)",
                params![subscription_id, resume_at.to_rfc3339()],
            )
            .expect("block billing after repaired grant dispatch");
            conn.execute(
                "UPDATE market_service_contracts
                 SET desired_control_state = 'suspended',
                     control_error = 'payment_overdue', updated_at = ?2
                 WHERE product_kind = 'share' AND product_ref = ?1",
                params![subscription_id, resume_at.to_rfc3339()],
            )
            .expect("replace pending integrity resume with billing suspension");
        }
        set_entitlement_from_stored_policy(&store, &subscription_id, resume_at).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(resume_at)
                .await
                .expect("confirm repaired grant before late billing recheck")
                .is_empty()
        );
        let revoke_at = resume_at + Duration::seconds(1);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(revoke_at)
                .await
                .expect("re-isolate grant after late billing block")
                .len(),
            1
        );
        let pending: (String, String, String, String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.status, sub.integrity_state, contract.status,
                        contract.desired_control_state,
                        (SELECT COUNT(*) FROM share_control_operations operation
                         WHERE operation.subscription_id = sub.id
                           AND operation.action = 'revoke'
                           AND operation.status = 'dispatched')
                 FROM share_market_subscriptions sub
                 JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
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
            .expect("read re-isolated late billing block");
        assert_eq!(pending.0, SUB_BILLING_SUSPEND_PENDING);
        assert_eq!(pending.1, "compatible");
        assert_eq!(pending.2, "billing_suspended");
        assert_eq!(pending.3, "suspended");
        assert_eq!(pending.4, 1);

        clear_entitlements(&store, "share-integrity-late-block").await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(revoke_at + Duration::hours(1))
                .await
                .expect("confirm late billing re-isolation")
                .is_empty()
        );
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_BILLING_SUSPENDED
        );
    }

    #[tokio::test]
    async fn v2_market_subscription_is_isolated_until_share_upgrades_to_v3() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-scope-upgrade", "owner-scope-upgrade@example.com");
        let renter = session("renter-scope-upgrade", "renter-scope-upgrade@example.com");
        insert_share(
            &store,
            "share-scope-upgrade",
            &owner.email,
            &[ShareTokenPeriod::Day],
        )
        .await;
        let (_, seat_id) = create_listing(&store, &owner, "share-scope-upgrade", free_seat()).await;
        let subscription_id = store
            .share_market_rent_seat(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
            .await
            .expect("rent scope upgrade seat");
        let now = Utc::now();
        activate_subscription(&store, &subscription_id, now).await;
        {
            let conn = store.conn.lock().await;
            let grants_json: String = conn
                .query_row(
                    "SELECT user_grants_json FROM shares
                     WHERE share_id = 'share-scope-upgrade'",
                    [],
                    |row| row.get(0),
                )
                .expect("read managed grant for v2 downgrade");
            let mut grants: BTreeMap<String, ShareUserGrant> =
                serde_json::from_str(&grants_json).expect("decode managed grants");
            grants
                .get_mut(&renter.email)
                .expect("renter grant")
                .policy
                .allowed_apps
                .clear();
            conn.execute(
                "UPDATE shares SET share_access_policy_version = 2,
                    user_grants_json = ?2 WHERE share_id = ?1",
                params![
                    "share-scope-upgrade",
                    serde_json::to_string(&grants).expect("encode legacy grant")
                ],
            )
            .expect("seed legacy v2 App scope");
            reconcile_share_contract_integrity_tx(&conn, "share-scope-upgrade", now)
                .expect("isolate legacy v2 market grant");
            let legacy_state: (String, String, String) = conn
                .query_row(
                    "SELECT status, integrity_state, integrity_reason
                     FROM share_market_subscriptions
                     WHERE id = ?1",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("read legacy v2 contract state");
            assert_eq!(legacy_state.0, SUB_BILLING_SUSPEND_PENDING);
            assert_eq!(legacy_state.1, "violated");
            assert!(legacy_state.2.contains("share_contract_upgrade_required"));
        }

        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(1))
                .await
                .expect("dispatch legacy v2 grant revoke")
                .len(),
            1
        );
        clear_entitlements(&store, "share-scope-upgrade").await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(2))
                .await
                .expect("keep legacy v2 contract isolated")
                .is_empty()
        );
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_BILLING_SUSPEND_PENDING
        );

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE shares SET share_access_policy_version = ?2 WHERE share_id = ?1",
                params!["share-scope-upgrade", i64::from(SHARE_CONTRACT_VERSION)],
            )
            .expect("upgrade Share contract version");
        }
        assert!(
            store
                .share_market_reconcile_and_dispatch(now + Duration::seconds(3))
                .await
                .expect("confirm upgraded contract isolation")
                .is_empty()
        );
        assert_eq!(
            subscription_status(&store, &subscription_id).await,
            SUB_BILLING_SUSPENDED
        );

        let restored_at = now + Duration::seconds(4);
        assert_eq!(
            store
                .share_market_reconcile_and_dispatch(restored_at)
                .await
                .expect("dispatch scoped v3 grant")
                .len(),
            1
        );
        {
            let conn = store.conn.lock().await;
            let policy_json: String = conn
                .query_row(
                    "SELECT policy_json FROM share_control_operations
                     WHERE subscription_id = ?1 AND action = 'upsert'
                     ORDER BY share_sequence DESC LIMIT 1",
                    params![subscription_id],
                    |row| row.get(0),
                )
                .expect("read restored scoped policy");
            let policy: ShareUserPolicy =
                serde_json::from_str(&policy_json).expect("decode restored scoped policy");
            assert_eq!(policy.allowed_apps, ["claude", "codex", "gemini"]);
        }
        set_entitlement_from_stored_policy(&store, &subscription_id, restored_at).await;
        assert!(
            store
                .share_market_reconcile_and_dispatch(restored_at)
                .await
                .expect("confirm restored scoped grant")
                .is_empty()
        );
        assert!(
            store
                .share_market_reconcile_and_dispatch(restored_at + Duration::seconds(1))
                .await
                .expect("complete v2 market contract remediation")
                .is_empty()
        );

        let final_state: (String, String, Option<String>, Option<String>, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT status, integrity_state, integrity_reason, app_scope_enforced_at,
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = sub.id
                           AND event.event_type = 'contract_integrity_violated'),
                        (SELECT COUNT(*) FROM share_market_events event
                         WHERE event.subscription_id = sub.id
                           AND event.event_type = 'contract_integrity_restored')
                 FROM share_market_subscriptions sub WHERE id = ?1",
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
            .expect("read remediated v3 contract");
        assert_eq!(final_state.0, SUB_ACTIVE_FREE);
        assert_eq!(final_state.1, "compatible");
        assert!(final_state.2.is_none());
        assert!(final_state.3.is_some());
        assert_eq!((final_state.4, final_state.5), (1, 1));
    }

    #[tokio::test]
    async fn persistent_integrity_drift_times_out_once_and_refunds_from_violation_time() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session(
            "owner-integrity-timeout",
            "owner-integrity-timeout@example.com",
        );
        let renter = session(
            "renter-integrity-timeout",
            "renter-integrity-timeout@example.com",
        );
        let (subscription_id, _, activated_at) = create_active_paid_fixed_subscription(
            &store,
            &owner,
            &renter,
            "share-integrity-timeout",
        )
        .await;
        let violated_at = activated_at + Duration::minutes(1);
        let service_started_at = violated_at - Duration::days(2);
        let service_ends_at = service_started_at + Duration::days(10);
        {
            let conn = store.conn.lock().await;
            let (contract_id, account_id): (String, String) = conn
                .query_row(
                    "SELECT id, account_id FROM market_service_contracts
                     WHERE product_kind = 'share' AND product_ref = ?1
                       AND status != 'terminated'",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("read timeout billing contract");
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET service_started_at = ?2, activated_at = ?2, expires_at = ?3
                 WHERE id = ?1",
                params![
                    subscription_id,
                    service_started_at.to_rfc3339(),
                    service_ends_at.to_rfc3339()
                ],
            )
            .expect("set timeout service interval");
            conn.execute(
                "UPDATE market_service_contracts
                 SET status = 'active', trial_seconds_remaining = 0,
                     service_started_at = ?2, activated_at = ?2,
                     last_evaluated_at = ?3, updated_at = ?3
                 WHERE id = ?1",
                params![
                    contract_id,
                    service_started_at.to_rfc3339(),
                    violated_at.to_rfc3339()
                ],
            )
            .expect("set timeout billing interval");
            conn.execute(
                "INSERT INTO market_accrual_entries (
                    id, account_id, contract_id, interval_id, currency,
                    daily_rate_minor, billable_seconds, amount_units, status,
                    invoice_id, created_at, updated_at, credited_units
                 ) VALUES ('timeout-accrual', ?1, ?2, 'timeout-interval', 'USD',
                           1200, 1, 86400, 'unbilled', NULL, ?3, ?3, 0)",
                params![account_id, contract_id, violated_at.to_rfc3339()],
            )
            .expect("seed refundable timeout accrual");
            conn.execute(
                "UPDATE market_credit_accounts SET balance_units = 86400 WHERE id = ?1",
                params![account_id],
            )
            .expect("seed timeout account balance");
            conn.execute(
                "UPDATE shares SET parallel_limit = 1
                 WHERE share_id = 'share-integrity-timeout'",
                [],
            )
            .expect("persist timeout contract drift");
            reconcile_share_contract_integrity_tx(&conn, "share-integrity-timeout", violated_at)
                .expect("start timeout remediation");
        }

        let deadline = violated_at + Duration::hours(CONTRACT_REPAIR_WINDOW_HOURS);
        let _ = store
            .share_market_reconcile_and_dispatch(deadline)
            .await
            .expect("terminate timed-out integrity remediation");
        let timed_out: (String, String, String, i64, i64, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT sub.integrity_state, sub.release_reason, contract.status,
                        adjustment.refund_bps, adjustment.amount_minor,
                        adjustment.unbilled_credit_units, account.balance_units
                 FROM share_market_subscriptions sub
                 JOIN market_service_contracts contract
                   ON contract.product_kind = 'share' AND contract.product_ref = sub.id
                 JOIN market_contract_adjustments adjustment
                   ON adjustment.contract_id = contract.id
                 JOIN market_credit_accounts account ON account.id = contract.account_id
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
                        row.get(6)?,
                    ))
                },
            )
            .expect("read timed-out contract refund");
        assert_eq!(timed_out.0, "terminated");
        assert_eq!(timed_out.1, "contract_integrity_repair_timeout");
        assert_eq!(timed_out.2, "terminated");
        assert_eq!(timed_out.3, 10_000);
        assert_eq!(timed_out.4, 1);
        assert_eq!(timed_out.5, 86_400);
        assert_eq!(timed_out.6, 0);

        let _ = store
            .share_market_reconcile_and_dispatch(deadline + Duration::hours(1))
            .await
            .expect("repeat timed-out reconciliation idempotently");
        let adjustment_count: i64 = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT COUNT(*) FROM market_contract_adjustments WHERE product_ref = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("count timeout adjustments");
        assert_eq!(adjustment_count, 1);
    }

    #[tokio::test]
    async fn supplier_refund_allocates_all_ledger_states_and_closes_obligation_idempotently() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("owner-refund-ledger", "owner-refund-ledger@example.com");
        let renter = session("renter-refund-ledger", "renter-refund-ledger@example.com");
        let outsider = session(
            "outsider-refund-ledger",
            "outsider-refund-ledger@example.com",
        );
        let (subscription_id, _, now) =
            create_active_paid_fixed_subscription(&store, &owner, &renter, "share-refund-ledger")
                .await;
        let service_started_at = now - Duration::days(2);
        let service_ends_at = service_started_at + Duration::days(10);
        let (contract_id, account_id, obligation_id, adjustment_id) = {
            let conn = store.conn.lock().await;
            let tx = conn.transaction().expect("begin refund ledger fixture");
            let (contract_id, account_id): (String, String) = tx
                .query_row(
                    "SELECT id, account_id FROM market_service_contracts
                     WHERE product_kind = 'share' AND product_ref = ?1
                       AND status != 'terminated'",
                    params![subscription_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("read refund ledger contract");
            tx.execute(
                "UPDATE share_market_subscriptions
                 SET service_started_at = ?2, activated_at = ?2, expires_at = ?3
                 WHERE id = ?1",
                params![
                    subscription_id,
                    service_started_at.to_rfc3339(),
                    service_ends_at.to_rfc3339()
                ],
            )
            .expect("set refund service interval");
            tx.execute(
                "UPDATE market_service_contracts
                 SET status = 'billing_suspended', service_started_at = ?2,
                     activated_at = ?2, last_evaluated_at = ?3, updated_at = ?3
                 WHERE id = ?1",
                params![
                    contract_id,
                    service_started_at.to_rfc3339(),
                    now.to_rfc3339()
                ],
            )
            .expect("freeze refund contract accrual");
            for (invoice_id, sequence, units, minor, status) in [
                ("refund-open-invoice", 1_i64, 172_800_i64, 2_i64, "open"),
                ("refund-paid-invoice", 2_i64, 259_200_i64, 3_i64, "paid"),
            ] {
                tx.execute(
                    "INSERT INTO market_invoices (
                        id, account_id, sequence, amount_minor, amount_cny_minor,
                        usd_cny_rate_micros, amount_units, currency,
                        payment_methods_json, payment_contacts_json,
                        payment_profile_updated_at, status, due_at, deadline_at,
                        opened_at, paid_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 7000000, ?6, 'USD',
                               '[]', '[]', ?7, ?8, ?9, ?9, ?7,
                               CASE WHEN ?8 = 'paid' THEN ?7 ELSE NULL END)",
                    params![
                        invoice_id,
                        account_id,
                        sequence,
                        minor,
                        minor * 7,
                        units,
                        now.to_rfc3339(),
                        status,
                        (now + Duration::days(1)).to_rfc3339(),
                    ],
                )
                .expect("insert refund invoice");
                tx.execute(
                    "INSERT INTO market_invoice_lines (
                        id, invoice_id, contract_id, product_kind, product_ref,
                        service_ref, service_label, daily_rate_minor,
                        billable_seconds, amount_minor, amount_cny_minor,
                        amount_units, service_started_at, service_ended_at,
                        evidence_json, created_at
                     ) VALUES (?1, ?2, ?3, 'share', ?4, ?5, ?5, 1200,
                               1, ?6, ?7, ?8, ?9, ?10, '{}', ?10)",
                    params![
                        format!("line-{invoice_id}"),
                        invoice_id,
                        contract_id,
                        subscription_id,
                        "share-refund-ledger",
                        minor,
                        minor * 7,
                        units,
                        service_started_at.to_rfc3339(),
                        now.to_rfc3339(),
                    ],
                )
                .expect("insert refund invoice line");
            }
            for (id, interval_id, units, status, invoice_id) in [
                (
                    "refund-unbilled-accrual",
                    "refund-unbilled-interval",
                    86_400_i64,
                    "unbilled",
                    None,
                ),
                (
                    "refund-open-accrual",
                    "refund-open-interval",
                    172_800_i64,
                    "invoiced",
                    Some("refund-open-invoice"),
                ),
                (
                    "refund-paid-accrual",
                    "refund-paid-interval",
                    259_200_i64,
                    "invoiced",
                    Some("refund-paid-invoice"),
                ),
            ] {
                tx.execute(
                    "INSERT INTO market_accrual_entries (
                        id, account_id, contract_id, interval_id, currency,
                        daily_rate_minor, billable_seconds, amount_units,
                        status, invoice_id, created_at, updated_at, credited_units
                     ) VALUES (?1, ?2, ?3, ?4, 'USD', 1200, 1, ?5,
                               ?6, ?7, ?8, ?8, 0)",
                    params![
                        id,
                        account_id,
                        contract_id,
                        interval_id,
                        units,
                        status,
                        invoice_id,
                        now.to_rfc3339(),
                    ],
                )
                .expect("insert refundable accrual");
            }
            tx.execute(
                "UPDATE market_credit_accounts
                 SET balance_units = 259200, open_invoice_id = 'refund-open-invoice'
                 WHERE id = ?1",
                params![account_id],
            )
            .expect("seed refundable account balance");

            let adjustment = crate::market_billing::apply_supplier_early_termination_refund_tx(
                &tx,
                "share",
                &subscription_id,
                "owner_early_termination",
                "refund-ledger-key",
                now,
            )
            .expect("apply supplier refund");
            assert_eq!(adjustment.status, "refund_due");
            assert_eq!(adjustment.calculation.refund_bps, 10_000);
            assert_eq!(adjustment.calculation.amount_minor, 6);
            assert_eq!(adjustment.unbilled_credit_minor, 1);
            assert_eq!(adjustment.invoice_credit_minor, 2);
            assert_eq!(adjustment.external_refund_minor, 3);
            assert_eq!(adjustment.obligations.len(), 1);
            let replay = crate::market_billing::apply_supplier_early_termination_refund_tx(
                &tx,
                "share",
                &subscription_id,
                "owner_early_termination",
                "refund-ledger-key",
                now + Duration::seconds(1),
            )
            .expect("replay supplier refund");
            assert_eq!(replay.id, adjustment.id);
            assert!(matches!(
                crate::market_billing::apply_supplier_early_termination_refund_tx(
                    &tx,
                    "share",
                    "another-contract",
                    "owner_early_termination",
                    "refund-ledger-key",
                    now,
                ),
                Err(AppError::Conflict(_))
            ));
            let obligation_id = adjustment.obligations[0].id.clone();
            let adjustment_id = adjustment.id.clone();
            tx.commit().expect("commit refund ledger fixture");
            (contract_id, account_id, obligation_id, adjustment_id)
        };

        let ledger_state: (i64, String, i64, String, i64, i64, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT account.balance_units, open_invoice.status, open_invoice.amount_units,
                        paid_invoice.status, paid_invoice.amount_units,
                        (SELECT COUNT(*) FROM market_credit_notes note
                         WHERE note.adjustment_id = ?3 AND note.kind = 'service_credit'),
                        (SELECT SUM(credited_units) FROM market_accrual_entries
                         WHERE contract_id = ?1)
                 FROM market_credit_accounts account
                 JOIN market_invoices open_invoice ON open_invoice.id = 'refund-open-invoice'
                 JOIN market_invoices paid_invoice ON paid_invoice.id = 'refund-paid-invoice'
                 WHERE account.id = ?2",
                params![contract_id, account_id, adjustment_id],
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
            .expect("read allocated refund ledger");
        assert_eq!(ledger_state.0, 0);
        assert_eq!(ledger_state.1, "void");
        assert_eq!(ledger_state.2, 0);
        assert_eq!(ledger_state.3, "paid");
        assert_eq!(ledger_state.4, 259_200);
        assert_eq!(ledger_state.5, 1);
        assert_eq!(ledger_state.6, 518_400);

        let owner_dashboard = store
            .market_billing_dashboard(&owner)
            .await
            .expect("read supplier refund dashboard");
        let buyer_dashboard = store
            .market_billing_dashboard(&renter)
            .await
            .expect("read buyer refund dashboard");
        let outsider_dashboard = store
            .market_billing_dashboard(&outsider)
            .await
            .expect("read outsider billing dashboard");
        assert!(owner_dashboard.refund_obligations[0].can_record);
        assert!(!buyer_dashboard.refund_obligations[0].can_record);
        assert!(outsider_dashboard.refund_obligations.is_empty());

        store
            .market_billing_reconcile(now + Duration::days(8))
            .await
            .expect("mark refund obligation overdue");
        let overdue: (String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT obligation.status,
                        (SELECT COUNT(*) FROM operator_alert_signal_outbox signal
                         WHERE signal.source_event_id = 'market-refund-overdue:' || obligation.id
                           AND signal.transition = 'firing')
                 FROM market_refund_obligations obligation WHERE obligation.id = ?1",
                params![obligation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read overdue refund obligation");
        assert_eq!(overdue, ("overdue".into(), 1));
        assert!(matches!(
            store
                .market_billing_record_refund_obligation(
                    &renter,
                    &obligation_id,
                    "refund-reference-1",
                )
                .await,
            Err(AppError::Forbidden(_))
        ));
        let recorded = store
            .market_billing_record_refund_obligation(&owner, &obligation_id, "refund-reference-1")
            .await
            .expect("record external refund");
        assert_eq!(recorded.status, "recorded");
        assert!(!recorded.can_record);
        let replay = store
            .market_billing_record_refund_obligation(&owner, &obligation_id, "refund-reference-1")
            .await
            .expect("replay same refund reference");
        assert_eq!(
            replay.external_reference.as_deref(),
            Some("refund-reference-1")
        );
        assert!(matches!(
            store
                .market_billing_record_refund_obligation(
                    &owner,
                    &obligation_id,
                    "different-reference",
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let settled: (String, i64) = store
            .conn
            .lock()
            .await
            .query_row(
                "SELECT adjustment.status,
                        (SELECT COUNT(*) FROM operator_alert_signal_outbox signal
                         WHERE signal.source_event_id = 'market-refund-recorded:' || ?2
                           AND signal.transition = 'resolved')
                 FROM market_contract_adjustments adjustment WHERE adjustment.id = ?1",
                params![adjustment_id, obligation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read settled refund adjustment");
        assert_eq!(settled, ("settled".into(), 1));
    }
}
