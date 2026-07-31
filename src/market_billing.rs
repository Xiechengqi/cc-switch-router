use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ServerState;
use crate::client_market_trade::{PaymentContact, PaymentMethod};
use crate::error::AppError;
use crate::models::AuthSession;
use crate::store::AppStore;

pub const TRIAL_SECONDS: i64 = 12 * 60 * 60;
pub(crate) const MARKET_CURRENCY: &str = "USD";
pub(crate) const USD_CNY_RATE: i64 = 7;
const MONEY_UNITS_PER_MINOR: i64 = 86_400;
const NEAR_CREDIT_LIMIT_BPS: i64 = 8_000;
const DEFAULT_SETTLEMENT_GRACE_HOURS: i64 = 24;
const HEALTH_FRESHNESS_SECS: i64 = 90;
const MAX_ACCRUAL_GAP_SECS: i64 = 20;
const BILLING_CYCLE_SECS: u64 = 5;
pub(crate) const MAX_DAILY_RATE_MINOR: i64 = 100_000_000;

const ACCOUNT_ACTIVE: &str = "active";
const ACCOUNT_NEAR_CREDIT_LIMIT: &str = "near_credit_limit";
const ACCOUNT_SETTLEMENT_DUE: &str = "settlement_due";
const ACCOUNT_PAYMENT_DECLARED: &str = "payment_declared";
const ACCOUNT_OVERDUE: &str = "overdue";
const ACCOUNT_DISPUTED: &str = "disputed";
const ACCOUNT_CLOSED: &str = "closed";

const CONTRACT_TRIAL: &str = "trial";
const CONTRACT_ACTIVE: &str = "active";
const CONTRACT_BILLING_SUSPENDED: &str = "billing_suspended";

const INVOICE_OPEN: &str = "open";
const INVOICE_PAYMENT_DECLARED: &str = "payment_declared";
const INVOICE_PAID: &str = "paid";
const INVOICE_OVERDUE: &str = "overdue";
const INVOICE_DISPUTED: &str = "disputed";
const INVOICE_VOID: &str = "void";

#[derive(Debug, Clone)]
pub struct ActivateContractInput<'a> {
    pub product_kind: &'a str,
    pub product_ref: &'a str,
    pub service_ref: &'a str,
    pub service_label: &'a str,
    pub buyer_user_id: &'a str,
    pub buyer_email: &'a str,
    pub supplier_user_id: &'a str,
    pub supplier_email: &'a str,
    pub currency: &'a str,
    pub daily_rate_minor: i64,
    pub offer_revision: i64,
    pub replacement_of: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BillingActionKind {
    Suspend,
    Resume,
    Terminate,
}

impl BillingActionKind {
    fn control_state(&self) -> &'static str {
        match self {
            Self::Suspend => "suspended",
            Self::Resume => "active",
            Self::Terminate => "terminated",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BillingAction {
    pub contract_id: String,
    pub kind: BillingActionKind,
    pub product_kind: String,
    pub product_ref: String,
    pub service_ref: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplierBillingProfileView {
    pub currency: String,
    pub settlement_grace_hours: i64,
    pub revision: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingServiceView {
    pub id: String,
    pub product_kind: String,
    pub product_ref: String,
    pub service_ref: String,
    pub service_label: String,
    pub status: String,
    pub health_state: String,
    pub daily_rate_minor: i64,
    pub offer_revision: i64,
    pub trial_seconds_remaining: i64,
    pub activated_at: String,
    pub suspended_at: Option<String>,
    pub terminated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingInvoiceLineView {
    pub id: String,
    pub contract_id: String,
    pub product_kind: String,
    pub product_ref: String,
    pub service_ref: String,
    pub service_label: String,
    pub daily_rate_minor: i64,
    pub billable_seconds: i64,
    pub amount_minor: i64,
    pub amount_usd_minor: i64,
    pub amount_cny_minor: i64,
    pub service_started_at: String,
    pub service_ended_at: String,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentDeclarationView {
    pub id: String,
    pub status: String,
    pub payment_method_kind: Option<String>,
    pub payment_reference: Option<String>,
    pub note: Option<String>,
    pub evidence_url: Option<String>,
    pub declared_at: String,
    pub rejected_at: Option<String>,
    pub rejection_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingInvoiceView {
    pub id: String,
    pub sequence: i64,
    pub status: String,
    pub amount_minor: i64,
    pub amount_usd_minor: i64,
    pub amount_cny_minor: i64,
    pub currency: String,
    pub due_at: String,
    pub deadline_at: String,
    pub opened_at: String,
    pub declared_at: Option<String>,
    pub paid_at: Option<String>,
    pub payment_methods: Vec<PaymentMethod>,
    pub contacts: Vec<PaymentContact>,
    pub payment_profile_updated_at: String,
    pub lines: Vec<BillingInvoiceLineView>,
    pub declaration: Option<PaymentDeclarationView>,
    pub dispute: Option<BillingDisputeView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingDisputeView {
    pub id: String,
    pub reason: String,
    pub status: String,
    pub resolution: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditAccountView {
    pub id: String,
    pub buyer_user_id: String,
    pub buyer_email: String,
    pub supplier_user_id: String,
    pub supplier_email: String,
    pub currency: String,
    pub status: String,
    pub balance_minor: i64,
    pub credit_kind: String,
    pub credit_limit_minor: Option<i64>,
    pub utilization_bps: Option<i64>,
    pub daily_rate_minor: i64,
    pub estimated_settlement_at: Option<String>,
    pub is_buyer: bool,
    pub is_supplier: bool,
    pub can_settle: bool,
    pub can_close: bool,
    pub close_requested: bool,
    pub services: Vec<BillingServiceView>,
    pub open_invoice: Option<BillingInvoiceView>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditRestrictionView {
    pub id: String,
    pub invoice_id: String,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingDashboardView {
    pub accounts: Vec<CreditAccountView>,
    pub supplier_profiles: Vec<SupplierBillingProfileView>,
    pub restrictions: Vec<CreditRestrictionView>,
    pub trial_hours: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingInvoiceHistoryView {
    pub invoices: Vec<BillingInvoiceView>,
    pub next_before_sequence: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateSupplierProfileRequest {
    #[serde(default = "default_settlement_grace_hours")]
    settlement_grace_hours: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeclarePaymentRequest {
    payment_method_kind: Option<String>,
    payment_reference: Option<String>,
    note: Option<String>,
    evidence_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RejectPaymentRequest {
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenDisputeRequest {
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveDisputeRequest {
    resolution: String,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VoidInvoiceRequest {
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InvoiceHistoryQuery {
    before_sequence: Option<i64>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminBillingDisputeView {
    dispute: BillingDisputeView,
    account_id: String,
    buyer_email: String,
    supplier_email: String,
    invoice: BillingInvoiceView,
}

#[derive(Debug, Clone)]
struct ContractRow {
    id: String,
    account_id: String,
    product_kind: String,
    product_ref: String,
    service_ref: String,
    daily_rate_minor: i64,
    trial_seconds_remaining: i64,
    health_state: String,
    last_evaluated_at: String,
}

#[derive(Debug, Clone)]
struct AccrualCandidate {
    contract: ContractRow,
    observed_state: String,
    observation_reason: String,
    interval_started_at: String,
    interval_ended_at: String,
    elapsed_seconds: i64,
    trial_seconds: i64,
    billable_seconds: i64,
    requested_units: i64,
    next_trial_seconds: i64,
}

#[derive(Debug, Clone)]
struct AccountRow {
    id: String,
    buyer_user_id: String,
    supplier_user_id: String,
    currency: String,
    status: String,
    balance_units: i64,
    open_invoice_id: Option<String>,
    close_requested: bool,
    credit_kind: String,
    credit_limit_minor: Option<i64>,
    settlement_grace_hours: i64,
    version: i64,
}

fn default_settlement_grace_hours() -> i64 {
    DEFAULT_SETTLEMENT_GRACE_HOURS
}

pub fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS supplier_billing_profiles (
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            currency TEXT NOT NULL,
            settlement_grace_hours INTEGER NOT NULL DEFAULT 24,
            revision INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (supplier_user_id, currency)
        );
        CREATE TABLE IF NOT EXISTS market_credit_accounts (
            id TEXT PRIMARY KEY,
            buyer_user_id TEXT NOT NULL,
            buyer_email TEXT NOT NULL,
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            currency TEXT NOT NULL,
            status TEXT NOT NULL,
            balance_units INTEGER NOT NULL DEFAULT 0,
            open_invoice_id TEXT,
            close_requested INTEGER NOT NULL DEFAULT 0,
            credit_kind TEXT NOT NULL CHECK (credit_kind IN ('none', 'limited', 'unlimited')),
            credit_limit_minor INTEGER,
            credit_source TEXT NOT NULL CHECK (credit_source IN ('counterparty', 'public')),
            credit_revision INTEGER NOT NULL,
            version INTEGER NOT NULL DEFAULT 1,
            last_warning_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE (buyer_user_id, supplier_user_id, currency)
        );
        CREATE TABLE IF NOT EXISTS market_service_contracts (
            id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL,
            product_kind TEXT NOT NULL,
            product_ref TEXT NOT NULL,
            service_ref TEXT NOT NULL,
            service_label TEXT NOT NULL,
            buyer_user_id TEXT NOT NULL,
            buyer_email TEXT NOT NULL,
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            currency TEXT NOT NULL,
            daily_rate_minor INTEGER NOT NULL,
            offer_revision INTEGER NOT NULL,
            status TEXT NOT NULL,
            trial_seconds_remaining INTEGER NOT NULL,
            health_state TEXT NOT NULL DEFAULT 'unknown',
            desired_control_state TEXT NOT NULL DEFAULT 'active',
            applied_control_state TEXT NOT NULL DEFAULT 'active',
            control_error TEXT,
            last_evaluated_at TEXT NOT NULL,
            activated_at TEXT NOT NULL,
            suspended_at TEXT,
            terminated_at TEXT,
            termination_reason TEXT,
            replacement_of TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS uq_market_active_product_contract
            ON market_service_contracts(product_kind, product_ref)
            WHERE status != 'terminated';
        CREATE INDEX IF NOT EXISTS idx_market_contract_account_status
            ON market_service_contracts(account_id, status);
        CREATE INDEX IF NOT EXISTS idx_market_contract_reconcile
            ON market_service_contracts(status, last_evaluated_at);
        CREATE TABLE IF NOT EXISTS market_service_intervals (
            id TEXT PRIMARY KEY,
            contract_id TEXT NOT NULL,
            state TEXT NOT NULL,
            observation_reason TEXT NOT NULL,
            started_at TEXT NOT NULL,
            ended_at TEXT NOT NULL,
            elapsed_seconds INTEGER NOT NULL DEFAULT 0,
            trial_seconds INTEGER NOT NULL DEFAULT 0,
            billable_seconds INTEGER NOT NULL DEFAULT 0,
            amount_units INTEGER NOT NULL DEFAULT 0,
            invoice_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_market_intervals_contract
            ON market_service_intervals(contract_id, started_at);
        CREATE TABLE IF NOT EXISTS market_accrual_entries (
            id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL,
            contract_id TEXT NOT NULL,
            interval_id TEXT NOT NULL UNIQUE,
            currency TEXT NOT NULL,
            daily_rate_minor INTEGER NOT NULL,
            billable_seconds INTEGER NOT NULL,
            amount_units INTEGER NOT NULL,
            status TEXT NOT NULL,
            invoice_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_market_accrual_account_status
            ON market_accrual_entries(account_id, status, created_at);
        CREATE TABLE IF NOT EXISTS market_invoices (
            id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            amount_minor INTEGER NOT NULL,
            amount_units INTEGER NOT NULL,
            currency TEXT NOT NULL,
            payment_methods_json TEXT NOT NULL,
            payment_contacts_json TEXT NOT NULL,
            payment_profile_updated_at TEXT NOT NULL,
            status TEXT NOT NULL,
            due_at TEXT NOT NULL,
            deadline_at TEXT NOT NULL,
            opened_at TEXT NOT NULL,
            declared_at TEXT,
            paid_at TEXT,
            overdue_at TEXT,
            disputed_at TEXT,
            voided_at TEXT,
            UNIQUE (account_id, sequence)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS uq_market_account_open_invoice
            ON market_invoices(account_id)
            WHERE status IN ('open', 'payment_declared', 'overdue', 'disputed');
        CREATE TABLE IF NOT EXISTS market_invoice_lines (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL,
            contract_id TEXT NOT NULL,
            product_kind TEXT NOT NULL,
            product_ref TEXT NOT NULL,
            service_ref TEXT NOT NULL,
            service_label TEXT NOT NULL,
            daily_rate_minor INTEGER NOT NULL,
            billable_seconds INTEGER NOT NULL,
            amount_minor INTEGER NOT NULL,
            amount_units INTEGER NOT NULL,
            service_started_at TEXT NOT NULL,
            service_ended_at TEXT NOT NULL,
            evidence_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_market_invoice_lines_invoice
            ON market_invoice_lines(invoice_id, service_started_at);
        CREATE TABLE IF NOT EXISTS market_payment_declarations (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL,
            buyer_user_id TEXT NOT NULL,
            status TEXT NOT NULL,
            payment_method_kind TEXT,
            payment_reference TEXT,
            note TEXT,
            evidence_url TEXT,
            declared_at TEXT NOT NULL,
            rejected_at TEXT,
            rejection_reason TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS uq_market_active_payment_declaration
            ON market_payment_declarations(invoice_id)
            WHERE status = 'declared';
        CREATE TABLE IF NOT EXISTS market_payment_receipts (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL UNIQUE,
            declaration_id TEXT NOT NULL,
            supplier_user_id TEXT NOT NULL,
            confirmed_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS market_billing_disputes (
            id TEXT PRIMARY KEY,
            invoice_id TEXT NOT NULL,
            opened_by_user_id TEXT NOT NULL,
            reason TEXT NOT NULL,
            status TEXT NOT NULL,
            resolution TEXT,
            created_at TEXT NOT NULL,
            resolved_at TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS uq_market_dispute_invoice
            ON market_billing_disputes(invoice_id);
        CREATE TABLE IF NOT EXISTS market_credit_restrictions (
            id TEXT PRIMARY KEY,
            buyer_user_id TEXT NOT NULL,
            invoice_id TEXT NOT NULL UNIQUE,
            reason TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            lifted_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_market_restrictions_buyer
            ON market_credit_restrictions(buyer_user_id, status);
        CREATE TABLE IF NOT EXISTS market_billing_events (
            id TEXT PRIMARY KEY,
            account_id TEXT,
            contract_id TEXT,
            invoice_id TEXT,
            actor_user_id TEXT,
            event_type TEXT NOT NULL,
            detail_json TEXT NOT NULL,
            idempotency_key TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_market_billing_events_account
            ON market_billing_events(account_id, created_at);",
    )?;
    Ok(())
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-billing/dashboard", get(get_dashboard))
        .route(
            "/v1/market-billing/supplier-profiles/:currency",
            put(update_supplier_profile),
        )
        .route(
            "/v1/market-billing/accounts/:account_id/settle",
            post(settle_account),
        )
        .route(
            "/v1/market-billing/accounts/:account_id/request-settlement",
            post(request_supplier_settlement),
        )
        .route(
            "/v1/market-billing/accounts/:account_id/close",
            post(close_account),
        )
        .route(
            "/v1/market-billing/accounts/:account_id/invoices",
            get(invoice_history),
        )
        .route(
            "/v1/market-billing/invoices/:invoice_id/declare-payment",
            post(declare_payment),
        )
        .route(
            "/v1/market-billing/invoices/:invoice_id/confirm",
            post(confirm_payment),
        )
        .route(
            "/v1/market-billing/invoices/:invoice_id/reject",
            post(reject_payment),
        )
        .route(
            "/v1/market-billing/invoices/:invoice_id/disputes",
            post(open_dispute),
        )
        .route(
            "/v1/admin/market-billing/disputes",
            get(list_admin_disputes),
        )
        .route(
            "/v1/admin/market-billing/disputes/:dispute_id/resolve",
            post(resolve_admin_dispute),
        )
        .route(
            "/v1/admin/market-billing/invoices/:invoice_id/void",
            post(void_admin_invoice),
        )
}

fn normalize_currency(currency: &str) -> Result<String, AppError> {
    let currency = currency.trim().to_ascii_uppercase();
    if currency == MARKET_CURRENCY {
        Ok(currency)
    } else {
        Err(AppError::BadRequest("currency must be USD".into()))
    }
}

fn clean_optional(
    value: Option<String>,
    max: usize,
    field: &str,
) -> Result<Option<String>, AppError> {
    let value = value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if value.as_ref().is_some_and(|value| value.len() > max) {
        return Err(AppError::BadRequest(format!("{field} is too long")));
    }
    Ok(value)
}

fn ceil_minor(amount_units: i64) -> i64 {
    if amount_units <= 0 {
        0
    } else {
        amount_units / MONEY_UNITS_PER_MINOR + i64::from(amount_units % MONEY_UNITS_PER_MINOR != 0)
    }
}

fn usd_minor_to_cny_minor(amount_usd_minor: i64) -> i64 {
    amount_usd_minor.saturating_mul(USD_CNY_RATE)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AppError::Internal("stored billing timestamp is invalid".into()))
}

fn map_db(context: &'static str) -> impl FnOnce(rusqlite::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated user session required".into()))
}

async fn require_actor_user_id(
    state: &ServerState,
    headers: &HeaderMap,
    required_scope: &str,
) -> Result<String, AppError> {
    if let Some(session) = crate::api::resolve_router_session(state, headers).await? {
        return Ok(session.user_id);
    }
    let token = crate::api::extract_router_api_token(headers)
        .ok_or_else(|| AppError::Unauthorized("authenticated user token required".into()))?;
    state
        .store
        .resolve_user_api_token(token, required_scope)
        .await?
        .map(|principal| principal.user_id)
        .ok_or_else(|| AppError::Unauthorized("invalid user api token".into()))
}

async fn get_dashboard(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<BillingDashboardView>, AppError> {
    let session = require_session(&state, &headers).await?;
    Ok(Json(state.store.market_billing_dashboard(&session).await?))
}

async fn update_supplier_profile(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(currency): Path<String>,
    Json(input): Json<UpdateSupplierProfileRequest>,
) -> Result<Json<BillingDashboardView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let currency = normalize_currency(&currency)?;
    state
        .store
        .market_billing_update_supplier_profile(&session, &currency, input.settlement_grace_hours)
        .await?;
    Ok(Json(state.store.market_billing_dashboard(&session).await?))
}

async fn settle_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Result<Json<BillingDashboardView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let actions = state
        .store
        .market_billing_open_account_invoice(&session.user_id, &account_id, false, false)
        .await?;
    dispatch_actions(&state, actions).await;
    Ok(Json(state.store.market_billing_dashboard(&session).await?))
}

async fn request_supplier_settlement(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let actor_user_id =
        require_actor_user_id(&state, &headers, "market:billing:settlement").await?;
    let actions = state
        .store
        .market_billing_open_account_invoice(&actor_user_id, &account_id, false, true)
        .await?;
    dispatch_actions(&state, actions).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn close_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> Result<Json<BillingDashboardView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let actions = state
        .store
        .market_billing_open_account_invoice(&session.user_id, &account_id, true, false)
        .await?;
    dispatch_actions(&state, actions).await;
    Ok(Json(state.store.market_billing_dashboard(&session).await?))
}

async fn invoice_history(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
    Query(query): Query<InvoiceHistoryQuery>,
) -> Result<Json<BillingInvoiceHistoryView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    Ok(Json(
        state
            .store
            .market_billing_invoice_history(&session, &account_id, query.before_sequence, limit)
            .await?,
    ))
}

async fn declare_payment(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(invoice_id): Path<String>,
    Json(input): Json<DeclarePaymentRequest>,
) -> Result<Json<BillingDashboardView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let payment_method_kind = clean_optional(input.payment_method_kind, 40, "paymentMethodKind")?;
    let payment_reference = clean_optional(input.payment_reference, 200, "paymentReference")?;
    let note = clean_optional(input.note, 2_000, "note")?;
    let evidence_url = clean_optional(input.evidence_url, 2_000, "evidenceUrl")?;
    if let Some(value) = evidence_url.as_deref() {
        crate::store::client_chat::validate_public_event_url(value, "evidenceUrl")?;
    }
    state
        .store
        .market_billing_declare_payment(
            &session,
            &invoice_id,
            payment_method_kind,
            payment_reference,
            note,
            evidence_url,
        )
        .await?;
    Ok(Json(state.store.market_billing_dashboard(&session).await?))
}

async fn confirm_payment(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(invoice_id): Path<String>,
) -> Result<Json<BillingDashboardView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let actions = state
        .store
        .market_billing_confirm_payment(&session, &invoice_id)
        .await?;
    dispatch_actions(&state, actions).await;
    Ok(Json(state.store.market_billing_dashboard(&session).await?))
}

async fn reject_payment(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(invoice_id): Path<String>,
    Json(input): Json<RejectPaymentRequest>,
) -> Result<Json<BillingDashboardView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let reason = clean_optional(Some(input.reason), 2_000, "reason")?
        .ok_or_else(|| AppError::BadRequest("reason is required".into()))?;
    state
        .store
        .market_billing_reject_payment(&session, &invoice_id, &reason)
        .await?;
    Ok(Json(state.store.market_billing_dashboard(&session).await?))
}

async fn open_dispute(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(invoice_id): Path<String>,
    Json(input): Json<OpenDisputeRequest>,
) -> Result<Json<BillingDashboardView>, AppError> {
    let session = require_session(&state, &headers).await?;
    let reason = clean_optional(Some(input.reason), 2_000, "reason")?
        .ok_or_else(|| AppError::BadRequest("reason is required".into()))?;
    state
        .store
        .market_billing_open_dispute(&session, &invoice_id, &reason)
        .await?;
    Ok(Json(state.store.market_billing_dashboard(&session).await?))
}

async fn list_admin_disputes(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AdminBillingDisputeView>>, AppError> {
    crate::api::require_admin_session(&state, &headers).await?;
    Ok(Json(state.store.market_billing_admin_disputes().await?))
}

async fn resolve_admin_dispute(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(dispute_id): Path<String>,
    Json(input): Json<ResolveDisputeRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    let resolution = input.resolution.trim().to_ascii_lowercase();
    if !matches!(resolution.as_str(), "uphold" | "void") {
        return Err(AppError::BadRequest(
            "resolution must be uphold or void".into(),
        ));
    }
    let note = clean_optional(input.note, 2_000, "note")?;
    let actions = state
        .store
        .market_billing_resolve_dispute(&session, &dispute_id, &resolution, note.as_deref())
        .await?;
    dispatch_actions(&state, actions).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn void_admin_invoice(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(invoice_id): Path<String>,
    Json(input): Json<VoidInvoiceRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let session = crate::api::require_admin_session(&state, &headers).await?;
    let reason = clean_optional(Some(input.reason), 2_000, "reason")?
        .ok_or_else(|| AppError::BadRequest("reason is required".into()))?;
    let actions = state
        .store
        .market_billing_void_invoice(&session, &invoice_id, &reason)
        .await?;
    dispatch_actions(&state, actions).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn run_service(state: ServerState) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(StdDuration::from_secs(BILLING_CYCLE_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        match state.store.market_billing_reconcile(Utc::now()).await {
            Ok(actions) => dispatch_actions(&state, actions).await,
            Err(error) => tracing::warn!(error = %error, "market billing reconciliation failed"),
        }
    }
}

pub(crate) async fn dispatch_actions(state: &ServerState, actions: Vec<BillingAction>) {
    let _control_guard = state.market_billing_controls.lock().await;
    for action in actions {
        match state
            .store
            .market_billing_control_action_is_current(&action.contract_id, &action.kind)
            .await
        {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                tracing::warn!(contract_id = %action.contract_id, error = %error, "verify billing control action failed");
                continue;
            }
        }
        let result = match (action.kind.clone(), action.product_kind.as_str()) {
            (BillingActionKind::Suspend, "share") => {
                crate::share_market::suspend_for_billing(state, &action.product_ref, &action.reason)
                    .await
            }
            (BillingActionKind::Resume, "share") => {
                crate::share_market::resume_after_billing(state, &action.product_ref).await
            }
            (BillingActionKind::Terminate, "share") => {
                crate::share_market::terminate_for_billing(
                    state,
                    &action.product_ref,
                    &action.reason,
                )
                .await
            }
            (BillingActionKind::Suspend, "client_host") => match state
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
            },
            (BillingActionKind::Resume, "client_host") => state
                .store
                .client_market_set_billing_suspended(&action.service_ref, false)
                .await
                .map(|_| ()),
            (BillingActionKind::Terminate, "client_host") => {
                crate::client_market::terminate_for_billing(
                    state,
                    &action.service_ref,
                    &action.reason,
                )
                .await
            }
            (_, product_kind) => Err(AppError::Internal(format!(
                "unsupported billing product kind {product_kind}"
            ))),
        };
        match result {
            Ok(()) => {
                if let Err(error) = state
                    .store
                    .market_billing_mark_control_applied(&action.contract_id, &action.kind)
                    .await
                {
                    tracing::warn!(contract_id = %action.contract_id, error = %error, "record billing control completion failed");
                }
            }
            Err(error) => {
                tracing::warn!(contract_id = %action.contract_id, error = %error, "billing control action failed");
                let _ = state
                    .store
                    .market_billing_mark_control_failed(&action.contract_id, &error.to_string())
                    .await;
            }
        }
    }
}

fn record_event_tx(
    tx: &Connection,
    account_id: Option<&str>,
    contract_id: Option<&str>,
    invoice_id: Option<&str>,
    actor_user_id: Option<&str>,
    event_type: &str,
    detail: serde_json::Value,
    idempotency_key: &str,
    now: &str,
) -> Result<(), AppError> {
    let detail = crate::store::client_chat::sanitize_system_event_payload(detail)?;
    let event_id = Uuid::new_v4().to_string();
    let inserted = tx
        .execute(
            "INSERT OR IGNORE INTO market_billing_events (
            id, account_id, contract_id, invoice_id, actor_user_id, event_type,
            detail_json, idempotency_key, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event_id,
                account_id,
                contract_id,
                invoice_id,
                actor_user_id,
                event_type,
                detail.to_string(),
                idempotency_key,
                now,
            ],
        )
        .map_err(map_db("record market billing event"))?;
    if inserted == 0 {
        return Ok(());
    }
    enqueue_billing_client_chat_events_tx(
        tx,
        account_id,
        contract_id,
        invoice_id,
        event_type,
        detail,
        &event_id,
        now,
    )?;
    Ok(())
}

fn billing_client_chat_event_type(event_type: &str) -> Option<&'static str> {
    match event_type {
        "credit_limit_reached"
        | "voluntary_settlement_requested"
        | "supplier_settlement_requested"
        | "final_service_ended" => Some("payment_due"),
        "account_closure_requested" | "credit_account_closed" => Some("billing_account_closing"),
        "payment_declared" => Some("payment_declared"),
        "payment_received" => Some("billing_payment_confirmed"),
        "payment_rejected" => Some("billing_payment_rejected"),
        "invoice_overdue" => Some("billing_payment_overdue"),
        "invoice_disputed" => Some("billing_invoice_disputed"),
        "invoice_dispute_resolved" => Some("billing_dispute_resolved"),
        "invoice_voided" => Some("billing_invoice_voided"),
        "credit_limit_warning" => Some("billing_credit_limit_warning"),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BillingChatService {
    product_kind: String,
    product_ref: String,
    service_ref: String,
    service_label: String,
}

fn billing_chat_services_tx(
    tx: &Connection,
    account_id: Option<&str>,
    contract_id: Option<&str>,
    invoice_id: Option<&str>,
) -> Result<Vec<BillingChatService>, AppError> {
    if let Some(invoice_id) = invoice_id {
        return tx
            .prepare(
                "SELECT DISTINCT product_kind, product_ref, service_ref, service_label
                 FROM market_invoice_lines
                 WHERE invoice_id = ?1
                 ORDER BY product_kind, product_ref",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![invoice_id], |row| {
                        Ok(BillingChatService {
                            product_kind: row.get(0)?,
                            product_ref: row.get(1)?,
                            service_ref: row.get(2)?,
                            service_label: row.get(3)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("resolve invoiced chat services"));
    }

    if let Some(contract_id) = contract_id {
        return tx
            .query_row(
                "SELECT product_kind, product_ref, service_ref, service_label
                 FROM market_service_contracts WHERE id = ?1",
                params![contract_id],
                |row| {
                    Ok(BillingChatService {
                        product_kind: row.get(0)?,
                        product_ref: row.get(1)?,
                        service_ref: row.get(2)?,
                        service_label: row.get(3)?,
                    })
                },
            )
            .optional()
            .map(|service| service.into_iter().collect())
            .map_err(map_db("resolve billing contract chat service"));
    }

    let Some(account_id) = account_id else {
        return Ok(Vec::new());
    };
    tx.prepare(
        "SELECT DISTINCT product_kind, product_ref, service_ref, service_label
         FROM market_service_contracts
         WHERE account_id = ?1
         ORDER BY product_kind, product_ref",
    )
    .and_then(|mut statement| {
        statement
            .query_map(params![account_id], |row| {
                Ok(BillingChatService {
                    product_kind: row.get(0)?,
                    product_ref: row.get(1)?,
                    service_ref: row.get(2)?,
                    service_label: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(map_db("resolve account billing chat services"))
}

fn billing_chat_installation_tx(
    tx: &Connection,
    service: &BillingChatService,
) -> Result<Option<String>, AppError> {
    match service.product_kind.as_str() {
        "client_host" => Ok(Some(service.service_ref.clone())),
        "share" => tx
            .query_row(
                "SELECT installation_id FROM share_market_subscriptions WHERE id = ?1",
                params![service.product_ref],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("resolve billed Share Client"))
            .and_then(|installation_id| {
                if installation_id.is_some() {
                    Ok(installation_id)
                } else {
                    tx.query_row(
                        "SELECT installation_id FROM shares WHERE share_id = ?1",
                        params![service.service_ref],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(map_db("resolve billed Share fallback Client"))
                }
            }),
        _ => Err(AppError::Internal(format!(
            "unsupported billing chat product kind {}",
            service.product_kind
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn enqueue_billing_client_chat_events_tx(
    tx: &Connection,
    account_id: Option<&str>,
    contract_id: Option<&str>,
    invoice_id: Option<&str>,
    billing_event_type: &str,
    detail: serde_json::Value,
    source_event_id: &str,
    now: &str,
) -> Result<(), AppError> {
    let Some(chat_event_type) = billing_client_chat_event_type(billing_event_type) else {
        return Ok(());
    };
    let services = billing_chat_services_tx(tx, account_id, contract_id, invoice_id)?;
    if services.is_empty() {
        return Ok(());
    }
    let mut targets = BTreeMap::<String, Vec<BillingChatService>>::new();
    for service in services {
        if let Some(installation_id) = billing_chat_installation_tx(tx, &service)? {
            targets.entry(installation_id).or_default().push(service);
        }
    }
    let account = account_id
        .map(|account_id| {
            tx.query_row(
                "SELECT buyer_user_id, buyer_email, supplier_user_id, supplier_email,
                        currency, status, balance_units, credit_kind, credit_limit_minor
                 FROM market_credit_accounts WHERE id = ?1",
                params![account_id],
                |row| {
                    Ok(serde_json::json!({
                        "accountId": account_id,
                        "buyerUserId": row.get::<_, String>(0)?,
                        "buyerEmail": row.get::<_, String>(1)?,
                        "supplierUserId": row.get::<_, String>(2)?,
                        "supplierEmail": row.get::<_, String>(3)?,
                        "currency": row.get::<_, String>(4)?,
                        "accountStatus": row.get::<_, String>(5)?,
                        "balanceMinor": ceil_minor(row.get::<_, i64>(6)?),
                        "creditKind": row.get::<_, String>(7)?,
                        "creditLimitMinor": row.get::<_, Option<i64>>(8)?,
                    }))
                },
            )
            .optional()
            .map_err(map_db("read billing chat account"))
        })
        .transpose()?
        .flatten();
    let invoice = invoice_id
        .map(|invoice_id| billing_chat_invoice_payload_tx(tx, invoice_id))
        .transpose()?;
    for (installation_id, services) in targets {
        let mut payload = serde_json::json!({
            "summary": format!("Billing: {}", chat_event_type.replace('_', " ")),
            "marketKind": "billing",
            "billingEventType": billing_event_type,
            "installationId": installation_id,
            "services": services,
        });
        let object = payload
            .as_object_mut()
            .expect("billing chat payload is an object");
        if let Some(account) = account.as_ref().and_then(serde_json::Value::as_object) {
            object.extend(account.clone());
        }
        if let Some(invoice) = invoice.as_ref().and_then(serde_json::Value::as_object) {
            object.extend(invoice.clone());
        }
        for field in [
            "declarationId",
            "reason",
            "resolution",
            "note",
            "utilizationBps",
            "accountClosed",
            "creditRevoked",
        ] {
            if let Some(value) = detail.get(field) {
                object.insert(field.into(), value.clone());
            }
        }
        let followers = account
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .map(|account| {
                ["buyerUserId", "supplierUserId"]
                    .iter()
                    .filter_map(|field| account.get(*field).and_then(serde_json::Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        crate::store::client_chat::enqueue_client_system_event_tx(
            tx,
            &installation_id,
            "market_billing",
            source_event_id,
            chat_event_type,
            payload,
            &followers,
            now,
        )?;
    }
    Ok(())
}

fn billing_chat_invoice_payload_tx(
    tx: &Connection,
    invoice_id: &str,
) -> Result<serde_json::Value, AppError> {
    let mut payload = tx
        .query_row(
            "SELECT amount_minor, currency, status, due_at, deadline_at,
                    payment_methods_json, payment_contacts_json
             FROM market_invoices WHERE id = ?1",
            params![invoice_id],
            |row| {
                let amount_usd_minor = row.get::<_, i64>(0)?;
                let methods = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(5)?)
                    .unwrap_or_else(|_| serde_json::json!([]));
                let contacts = serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(6)?)
                    .unwrap_or_else(|_| serde_json::json!([]));
                Ok(serde_json::json!({
                    "invoiceId": invoice_id,
                    "amountMinor": amount_usd_minor,
                    "amountUsdMinor": amount_usd_minor,
                    "amountCnyMinor": usd_minor_to_cny_minor(amount_usd_minor),
                    "currency": row.get::<_, String>(1)?,
                    "invoiceStatus": row.get::<_, String>(2)?,
                    "dueAt": row.get::<_, String>(3)?,
                    "deadlineAt": row.get::<_, String>(4)?,
                    "paymentMethods": methods,
                    "paymentContacts": contacts,
                }))
            },
        )
        .map_err(map_db("read billing chat invoice"))?;
    if let Some(declaration) = tx
        .query_row(
            "SELECT id, status, payment_method_kind, payment_reference, note,
                    evidence_url, declared_at, rejected_at, rejection_reason
             FROM market_payment_declarations WHERE invoice_id = ?1
             ORDER BY declared_at DESC LIMIT 1",
            params![invoice_id],
            |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "status": row.get::<_, String>(1)?,
                    "paymentMethodKind": row.get::<_, Option<String>>(2)?,
                    "paymentReference": row.get::<_, Option<String>>(3)?,
                    "note": row.get::<_, Option<String>>(4)?,
                    "evidenceUrl": row.get::<_, Option<String>>(5)?,
                    "declaredAt": row.get::<_, String>(6)?,
                    "rejectedAt": row.get::<_, Option<String>>(7)?,
                    "rejectionReason": row.get::<_, Option<String>>(8)?,
                }))
            },
        )
        .optional()
        .map_err(map_db("read billing chat payment declaration"))?
    {
        payload
            .as_object_mut()
            .expect("billing invoice payload is an object")
            .insert("paymentDeclaration".into(), declaration);
    }
    if let Some(dispute) = tx
        .query_row(
            "SELECT id, reason, status, resolution, created_at, resolved_at
             FROM market_billing_disputes WHERE invoice_id = ?1",
            params![invoice_id],
            |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "reason": row.get::<_, String>(1)?,
                    "status": row.get::<_, String>(2)?,
                    "resolution": row.get::<_, Option<String>>(3)?,
                    "createdAt": row.get::<_, String>(4)?,
                    "resolvedAt": row.get::<_, Option<String>>(5)?,
                }))
            },
        )
        .optional()
        .map_err(map_db("read billing chat dispute"))?
    {
        payload
            .as_object_mut()
            .expect("billing invoice payload is an object")
            .insert("dispute".into(), dispute);
    }
    Ok(payload)
}

pub(crate) fn require_supplier_profile_tx(
    tx: &Connection,
    supplier_user_id: &str,
    currency: &str,
) -> Result<i64, AppError> {
    tx.query_row(
        "SELECT settlement_grace_hours
         FROM supplier_billing_profiles
         WHERE supplier_user_id = ?1 AND currency = ?2",
        params![supplier_user_id, currency],
        |row| row.get(0),
    )
    .optional()
    .map_err(map_db("read supplier billing profile"))?
    .ok_or_else(|| {
        AppError::Conflict(format!(
            "configure {currency} settlement terms before publishing a paid offer"
        ))
    })
}

fn ensure_credit_preconditions_tx(
    tx: &Connection,
    buyer_user_id: &str,
    supplier_user_id: &str,
    currency: &str,
) -> Result<(), AppError> {
    require_supplier_profile_tx(tx, supplier_user_id, currency)?;
    let restricted = tx
        .query_row(
            "SELECT 1 FROM market_credit_restrictions
             WHERE buyer_user_id = ?1 AND status = 'active' LIMIT 1",
            params![buyer_user_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(map_db("check market credit restriction"))?
        .is_some();
    if restricted {
        return Err(AppError::coded_forbidden(
            crate::market_access::ERROR_MARKET_BUYER_RESTRICTED,
            "new market credit is blocked until overdue bills are resolved",
            serde_json::json!({ "buyerUserId": buyer_user_id, "currency": currency }),
        ));
    }
    Ok(())
}

fn resolve_credit_grant_tx(
    tx: &Connection,
    buyer_user_id: &str,
    buyer_email: &str,
    supplier_user_id: &str,
    product_kind: &str,
    currency: &str,
) -> Result<crate::market_access::EffectiveCreditGrant, AppError> {
    let grant = crate::market_access::effective_credit_grant_tx(
        tx,
        supplier_user_id,
        buyer_user_id,
        buyer_email,
        product_kind,
        currency,
    )?;
    let account_state = tx
        .query_row(
            "SELECT status, balance_units FROM market_credit_accounts
             WHERE buyer_user_id = ?1 AND supplier_user_id = ?2 AND currency = ?3",
            params![buyer_user_id, supplier_user_id, currency],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(map_db("read market credit account status"))?;
    if let Some((status, balance_units)) = account_state {
        if status == ACCOUNT_CLOSED {
            return Err(AppError::coded_conflict(
                crate::market_access::ERROR_MARKET_RELATIONSHIP_CLOSED,
                "the supplier permanently closed this credit relationship; future paid rentals with this supplier are disabled",
                serde_json::json!({
                    "supplierUserId": supplier_user_id,
                    "currency": currency,
                }),
            ));
        }
        if matches!(
            status.as_str(),
            ACCOUNT_SETTLEMENT_DUE | ACCOUNT_PAYMENT_DECLARED | ACCOUNT_OVERDUE | ACCOUNT_DISPUTED
        ) {
            return Err(AppError::coded_conflict(
                crate::market_access::ERROR_MARKET_SETTLEMENT_REQUIRED,
                "settle the supplier credit account before renting another service",
                serde_json::json!({
                    "supplierUserId": supplier_user_id,
                    "currency": currency,
                    "accountStatus": status,
                }),
            ));
        }
        if grant.kind == crate::market_access::CREDIT_LIMITED {
            let limit_units = grant
                .limit_minor
                .ok_or_else(|| {
                    AppError::Internal("limited market credit grant is missing its limit".into())
                })?
                .checked_mul(MONEY_UNITS_PER_MINOR)
                .ok_or_else(|| AppError::Internal("market credit limit overflowed".into()))?;
            if balance_units >= limit_units {
                return Err(AppError::coded_conflict(
                    crate::market_access::ERROR_MARKET_CREDIT_LIMIT_REACHED,
                    "the supplier credit limit has been reached; settle the account before renting another service",
                    serde_json::json!({
                        "supplierUserId": supplier_user_id,
                        "currency": currency,
                        "limitMinor": grant.limit_minor,
                    }),
                ));
            }
        }
    }
    Ok(grant)
}

pub(crate) fn credit_eligibility_tx(
    tx: &Connection,
    buyer_user_id: &str,
    buyer_email: &str,
    supplier_user_id: &str,
    product_kind: &str,
    currency: &str,
) -> Result<crate::market_access::EffectiveCreditGrant, AppError> {
    ensure_credit_preconditions_tx(tx, buyer_user_id, supplier_user_id, currency)?;
    if !crate::market_access::product_access_allowed_tx(
        tx,
        supplier_user_id,
        buyer_user_id,
        buyer_email,
        product_kind,
        crate::market_access::PRICING_PAID,
    )? {
        return Err(AppError::coded_forbidden(
            crate::market_access::ERROR_MARKET_ACCESS_REQUIRED,
            "seller approval is required before renting this market service",
            serde_json::json!({
                "supplierUserId": supplier_user_id,
                "productKind": product_kind,
                "pricingKind": crate::market_access::PRICING_PAID,
            }),
        ));
    }
    resolve_credit_grant_tx(
        tx,
        buyer_user_id,
        buyer_email,
        supplier_user_id,
        product_kind,
        currency,
    )
}

pub(crate) fn ensure_credit_allowed_tx(
    tx: &Transaction<'_>,
    buyer_user_id: &str,
    buyer_email: &str,
    supplier_user_id: &str,
    product_kind: &str,
    currency: &str,
) -> Result<crate::market_access::EffectiveCreditGrant, AppError> {
    ensure_credit_preconditions_tx(tx, buyer_user_id, supplier_user_id, currency)?;
    crate::market_access::ensure_product_access_tx(
        tx,
        supplier_user_id,
        buyer_user_id,
        buyer_email,
        product_kind,
        crate::market_access::PRICING_PAID,
    )?;
    resolve_credit_grant_tx(
        tx,
        buyer_user_id,
        buyer_email,
        supplier_user_id,
        product_kind,
        currency,
    )
}

pub(crate) fn activate_contract_tx(
    tx: &Transaction<'_>,
    input: ActivateContractInput<'_>,
    now: &str,
) -> Result<String, AppError> {
    if input.buyer_user_id == input.supplier_user_id {
        return Err(AppError::BadRequest(
            "market billing does not apply when buyer and supplier are the same account".into(),
        ));
    }
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM market_service_contracts
             WHERE product_kind = ?1 AND product_ref = ?2 AND status != 'terminated'",
            params![input.product_kind, input.product_ref],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read existing market billing contract"))?
    {
        return Ok(id);
    }
    if input.daily_rate_minor <= 0 || input.daily_rate_minor > MAX_DAILY_RATE_MINOR {
        return Err(AppError::BadRequest(
            "daily market rate is outside the supported range".into(),
        ));
    }
    let currency = normalize_currency(input.currency)?;
    let normalized = ActivateContractInput {
        currency: &currency,
        ..input
    };
    let grant = ensure_credit_allowed_tx(
        tx,
        normalized.buyer_user_id,
        normalized.buyer_email,
        normalized.supplier_user_id,
        normalized.product_kind,
        &currency,
    )?;
    let account_id = ensure_account_tx(tx, &normalized, &grant, now)?;
    let trial_seconds = if let Some(replacement_of) = normalized.replacement_of {
        tx.query_row(
            "SELECT trial_seconds_remaining FROM market_service_contracts WHERE id = ?1",
            params![replacement_of],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(map_db("read replacement trial balance"))?
        .unwrap_or(0)
    } else {
        TRIAL_SECONDS
    };
    let status = if trial_seconds > 0 {
        CONTRACT_TRIAL
    } else {
        CONTRACT_ACTIVE
    };
    let id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO market_service_contracts (
            id, account_id, product_kind, product_ref, service_ref, service_label,
            buyer_user_id, buyer_email, supplier_user_id, supplier_email, currency,
            daily_rate_minor, offer_revision, status, trial_seconds_remaining,
            health_state, desired_control_state, applied_control_state,
            last_evaluated_at, activated_at, replacement_of, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                   ?12, ?13, ?14, ?15, 'unknown', 'active', 'active',
                   ?16, ?16, ?17, ?16, ?16)",
        params![
            id,
            account_id,
            normalized.product_kind,
            normalized.product_ref,
            normalized.service_ref,
            normalized.service_label,
            normalized.buyer_user_id,
            normalized.buyer_email,
            normalized.supplier_user_id,
            normalized.supplier_email,
            currency,
            normalized.daily_rate_minor,
            normalized.offer_revision,
            status,
            trial_seconds,
            now,
            normalized.replacement_of,
        ],
    )
    .map_err(map_db("activate market billing contract"))?;
    record_event_tx(
        tx,
        Some(&account_id),
        Some(&id),
        None,
        Some(normalized.buyer_user_id),
        "service_contract_activated",
        serde_json::json!({
            "productKind": normalized.product_kind,
            "productRef": normalized.product_ref,
            "dailyRateMinor": normalized.daily_rate_minor,
            "currency": currency,
            "trialSeconds": trial_seconds,
        }),
        &format!("contract-activated:{id}"),
        now,
    )?;
    Ok(id)
}

pub(crate) fn terminate_contract_tx(
    tx: &Connection,
    product_kind: &str,
    product_ref: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    let row = tx
        .query_row(
            "SELECT id, account_id FROM market_service_contracts
             WHERE product_kind = ?1 AND product_ref = ?2 AND status != 'terminated'",
            params![product_kind, product_ref],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(map_db("read market billing contract for termination"))?;
    let Some((contract_id, account_id)) = row else {
        return Ok(());
    };
    tx.execute(
        "UPDATE market_service_contracts
         SET status = 'terminated', desired_control_state = 'terminated',
             applied_control_state = 'terminated', terminated_at = ?2,
             termination_reason = ?3, last_evaluated_at = ?2, updated_at = ?2
         WHERE id = ?1",
        params![contract_id, now, reason],
    )
    .map_err(map_db("terminate market billing contract"))?;
    record_event_tx(
        tx,
        Some(&account_id),
        Some(&contract_id),
        None,
        None,
        "service_contract_terminated",
        serde_json::json!({ "reason": reason }),
        &format!("contract-terminated:{contract_id}"),
        now,
    )?;
    Ok(())
}

fn ensure_account_tx(
    tx: &Transaction<'_>,
    input: &ActivateContractInput<'_>,
    grant: &crate::market_access::EffectiveCreditGrant,
    now: &str,
) -> Result<String, AppError> {
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM market_credit_accounts
             WHERE buyer_user_id = ?1 AND supplier_user_id = ?2 AND currency = ?3",
            params![input.buyer_user_id, input.supplier_user_id, input.currency],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read market credit account"))?
    {
        tx.execute(
            "UPDATE market_credit_accounts
             SET buyer_email = ?2, supplier_email = ?3,
                 credit_kind = ?4, credit_limit_minor = ?5,
                 credit_source = ?6, credit_revision = ?7, updated_at = ?8
             WHERE id = ?1",
            params![
                id,
                input.buyer_email,
                input.supplier_email,
                grant.kind,
                grant.limit_minor,
                grant.source,
                grant.revision,
                now,
            ],
        )
        .map_err(map_db("refresh market credit account identities"))?;
        return Ok(id);
    }
    let id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO market_credit_accounts (
            id, buyer_user_id, buyer_email, supplier_user_id, supplier_email,
            currency, status, balance_units, open_invoice_id,
            credit_kind, credit_limit_minor, credit_source, credit_revision,
            version, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 0, NULL,
                   ?7, ?8, ?9, ?10, 1, ?11, ?11)",
        params![
            id,
            input.buyer_user_id,
            input.buyer_email,
            input.supplier_user_id,
            input.supplier_email,
            input.currency,
            grant.kind,
            grant.limit_minor,
            grant.source,
            grant.revision,
            now,
        ],
    )
    .map_err(map_db("create market credit account"))?;
    record_event_tx(
        tx,
        Some(&id),
        None,
        None,
        Some(input.buyer_user_id),
        "credit_account_created",
        serde_json::json!({ "currency": input.currency }),
        &format!("account-created:{id}"),
        now,
    )?;
    Ok(id)
}

impl AppStore {
    pub async fn market_billing_require_supplier_profile(
        &self,
        supplier_user_id: &str,
        currency: &str,
    ) -> Result<(), AppError> {
        let currency = normalize_currency(currency)?;
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin supplier billing profile check"))?;
        require_supplier_profile_tx(&tx, supplier_user_id, &currency)?;
        tx.commit()
            .map_err(map_db("commit supplier billing profile check"))?;
        Ok(())
    }

    pub async fn market_billing_check_credit_allowed(
        &self,
        buyer_user_id: &str,
        buyer_email: &str,
        supplier_user_id: &str,
        product_kind: &str,
        currency: &str,
    ) -> Result<(), AppError> {
        let currency = normalize_currency(currency)?;
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin market credit eligibility check"))?;
        ensure_credit_allowed_tx(
            &tx,
            buyer_user_id,
            buyer_email,
            supplier_user_id,
            product_kind,
            &currency,
        )?;
        tx.commit()
            .map_err(map_db("commit market credit eligibility check"))?;
        Ok(())
    }

    pub async fn market_billing_update_supplier_profile(
        &self,
        session: &AuthSession,
        currency: &str,
        settlement_grace_hours: i64,
    ) -> Result<(), AppError> {
        let currency = normalize_currency(currency)?;
        if !(1..=24 * 30).contains(&settlement_grace_hours) {
            return Err(AppError::BadRequest(
                "settlementGraceHours must be between 1 and 720".into(),
            ));
        }
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin supplier billing profile update"))?;
        tx.execute(
            "INSERT INTO supplier_billing_profiles (
                supplier_user_id, supplier_email, currency,
                settlement_grace_hours, revision, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)
             ON CONFLICT(supplier_user_id, currency) DO UPDATE SET
                supplier_email = excluded.supplier_email,
                settlement_grace_hours = excluded.settlement_grace_hours,
                revision = supplier_billing_profiles.revision + 1,
                updated_at = excluded.updated_at",
            params![
                session.user_id,
                session.email,
                currency,
                settlement_grace_hours,
                now,
            ],
        )
        .map_err(map_db("upsert supplier billing profile"))?;
        record_event_tx(
            &tx,
            None,
            None,
            None,
            Some(&session.user_id),
            "supplier_profile_updated",
            serde_json::json!({
                "currency": currency,
                "settlementGraceHours": settlement_grace_hours,
            }),
            &format!("supplier-profile:{}:{currency}:{now}", session.user_id),
            &now,
        )?;
        tx.commit()
            .map_err(map_db("commit supplier billing profile update"))?;
        Ok(())
    }

    pub async fn market_billing_activate_contract(
        &self,
        input: ActivateContractInput<'_>,
        now: DateTime<Utc>,
    ) -> Result<String, AppError> {
        let now = now.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin market billing contract activation"))?;
        let id = activate_contract_tx(&tx, input, &now)?;
        tx.commit()
            .map_err(map_db("commit market billing contract activation"))?;
        Ok(id)
    }

    pub async fn market_billing_terminate_contract(
        &self,
        product_kind: &str,
        product_ref: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let now = now.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin market billing contract termination"))?;
        terminate_contract_tx(&tx, product_kind, product_ref, reason, &now)?;
        tx.commit()
            .map_err(map_db("commit market billing contract termination"))?;
        Ok(())
    }

    pub async fn market_billing_mark_control_applied(
        &self,
        contract_id: &str,
        kind: &BillingActionKind,
    ) -> Result<(), AppError> {
        let state = kind.control_state();
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        let share_control_complete = conn
            .query_row(
                "SELECT contract.product_kind != 'share' OR
                        CASE ?2
                            WHEN 'suspended' THEN EXISTS (
                                SELECT 1 FROM share_market_subscriptions subscription
                                WHERE subscription.id = contract.product_ref
                                  AND subscription.status = 'billing_suspended'
                            )
                            WHEN 'active' THEN EXISTS (
                                SELECT 1 FROM share_market_subscriptions subscription
                                WHERE subscription.id = contract.product_ref
                                  AND subscription.status = 'active_postpaid'
                            )
                            WHEN 'terminated' THEN
                                contract.status = 'terminated' OR NOT EXISTS (
                                    SELECT 1 FROM share_market_subscriptions subscription
                                    WHERE subscription.id = contract.product_ref
                                      AND subscription.status NOT IN ('released', 'grant_failed')
                                )
                            ELSE 0
                        END
                 FROM market_service_contracts contract WHERE contract.id = ?1",
                params![contract_id, state],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(map_db("check market billing control completion"))?
            .ok_or_else(|| AppError::NotFound("market billing contract not found".into()))?;
        if !share_control_complete {
            return Ok(());
        }
        conn.execute(
            "UPDATE market_service_contracts
             SET applied_control_state = ?2, control_error = NULL, updated_at = ?3
             WHERE id = ?1 AND desired_control_state = ?2",
            params![contract_id, state, now],
        )
        .map_err(map_db("mark market billing control applied"))?;
        Ok(())
    }

    pub async fn market_billing_control_action_is_current(
        &self,
        contract_id: &str,
        kind: &BillingActionKind,
    ) -> Result<bool, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT desired_control_state = ?2 AND applied_control_state != ?2
             FROM market_service_contracts WHERE id = ?1",
            params![contract_id, kind.control_state()],
            |row| row.get(0),
        )
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(map_db("verify current market billing control"))
    }

    pub async fn market_billing_mark_control_failed(
        &self,
        contract_id: &str,
        error: &str,
    ) -> Result<(), AppError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE market_service_contracts
             SET control_error = ?2, updated_at = ?3 WHERE id = ?1",
            params![contract_id, error, now],
        )
        .map_err(map_db("mark market billing control failed"))?;
        Ok(())
    }

    pub async fn client_market_set_billing_suspended(
        &self,
        installation_id: &str,
        suspended: bool,
    ) -> Result<Option<String>, AppError> {
        let now = Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Client billing suspension"))?;
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
            .map_err(map_db("read Client tunnel for billing suspension"))?
            .unwrap_or_default();
        let terminal = tunnel
            .1
            .as_deref()
            .is_none_or(|status| matches!(status, "released" | "releasing" | "release_failed"));
        if terminal {
            tx.commit()
                .map_err(map_db("commit skipped Client billing control"))?;
            return Ok(None);
        }
        tx.execute(
            "UPDATE installation_client_tunnels SET enabled = ?2, updated_at = ?3
             WHERE installation_id = ?1",
            params![installation_id, i64::from(!suspended), now],
        )
        .map_err(map_db("update Client tunnel billing suspension"))?;
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
        .map_err(map_db("update Client subscription billing suspension"))?;
        tx.commit()
            .map_err(map_db("commit Client billing suspension"))?;
        Ok(tunnel.0)
    }
}

fn service_views_for_account(
    conn: &Connection,
    account_id: &str,
) -> Result<Vec<BillingServiceView>, AppError> {
    conn.prepare(
        "SELECT id, product_kind, product_ref, service_ref, service_label, status,
                health_state, daily_rate_minor, offer_revision, trial_seconds_remaining,
                activated_at, suspended_at, terminated_at
         FROM market_service_contracts
         WHERE account_id = ?1 AND status != 'terminated'
         ORDER BY activated_at, id",
    )
    .and_then(|mut statement| {
        statement
            .query_map(params![account_id], |row| {
                Ok(BillingServiceView {
                    id: row.get(0)?,
                    product_kind: row.get(1)?,
                    product_ref: row.get(2)?,
                    service_ref: row.get(3)?,
                    service_label: row.get(4)?,
                    status: row.get(5)?,
                    health_state: row.get(6)?,
                    daily_rate_minor: row.get(7)?,
                    offer_revision: row.get(8)?,
                    trial_seconds_remaining: row.get(9)?,
                    activated_at: row.get(10)?,
                    suspended_at: row.get(11)?,
                    terminated_at: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(map_db("read billing dashboard services"))
}

fn declaration_view_for_invoice(
    conn: &Connection,
    invoice_id: &str,
) -> Result<Option<PaymentDeclarationView>, AppError> {
    conn.query_row(
        "SELECT id, status, payment_method_kind, payment_reference, note,
                evidence_url, declared_at, rejected_at, rejection_reason
         FROM market_payment_declarations
         WHERE invoice_id = ?1
         ORDER BY declared_at DESC LIMIT 1",
        params![invoice_id],
        |row| {
            Ok(PaymentDeclarationView {
                id: row.get(0)?,
                status: row.get(1)?,
                payment_method_kind: row.get(2)?,
                payment_reference: row.get(3)?,
                note: row.get(4)?,
                evidence_url: row.get(5)?,
                declared_at: row.get(6)?,
                rejected_at: row.get(7)?,
                rejection_reason: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(map_db("read billing dashboard payment declaration"))
}

fn dispute_view_for_invoice(
    conn: &Connection,
    invoice_id: &str,
) -> Result<Option<BillingDisputeView>, AppError> {
    conn.query_row(
        "SELECT id, reason, status, resolution, created_at, resolved_at
         FROM market_billing_disputes
         WHERE invoice_id = ?1
         ORDER BY created_at DESC LIMIT 1",
        params![invoice_id],
        |row| {
            Ok(BillingDisputeView {
                id: row.get(0)?,
                reason: row.get(1)?,
                status: row.get(2)?,
                resolution: row.get(3)?,
                created_at: row.get(4)?,
                resolved_at: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(map_db("read billing dashboard dispute"))
}

fn invoice_view(
    conn: &Connection,
    invoice_id: &str,
) -> Result<Option<BillingInvoiceView>, AppError> {
    let header = conn
        .query_row(
            "SELECT id, sequence, status, amount_minor, currency, due_at,
                    deadline_at, opened_at, declared_at, paid_at,
                    payment_methods_json, payment_contacts_json,
                    payment_profile_updated_at
             FROM market_invoices WHERE id = ?1",
            params![invoice_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read billing dashboard invoice"))?;
    let Some(header) = header else {
        return Ok(None);
    };
    let lines = conn
        .prepare(
            "SELECT id, contract_id, product_kind, product_ref, service_ref,
                    service_label, daily_rate_minor, billable_seconds, amount_minor,
                    service_started_at, service_ended_at, evidence_json
             FROM market_invoice_lines WHERE invoice_id = ?1
             ORDER BY service_started_at, id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![invoice_id], |row| {
                    let evidence: String = row.get(11)?;
                    let amount_minor = row.get(8)?;
                    Ok(BillingInvoiceLineView {
                        id: row.get(0)?,
                        contract_id: row.get(1)?,
                        product_kind: row.get(2)?,
                        product_ref: row.get(3)?,
                        service_ref: row.get(4)?,
                        service_label: row.get(5)?,
                        daily_rate_minor: row.get(6)?,
                        billable_seconds: row.get(7)?,
                        amount_minor,
                        amount_usd_minor: amount_minor,
                        amount_cny_minor: usd_minor_to_cny_minor(amount_minor),
                        service_started_at: row.get(9)?,
                        service_ended_at: row.get(10)?,
                        evidence: serde_json::from_str(&evidence)
                            .unwrap_or_else(|_| serde_json::json!({})),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read billing dashboard invoice lines"))?;
    let currency = normalize_currency(&header.4)?;
    Ok(Some(BillingInvoiceView {
        id: header.0,
        sequence: header.1,
        status: header.2,
        amount_minor: header.3,
        amount_usd_minor: header.3,
        amount_cny_minor: usd_minor_to_cny_minor(header.3),
        currency,
        due_at: header.5,
        deadline_at: header.6,
        opened_at: header.7,
        declared_at: header.8,
        paid_at: header.9,
        payment_methods: serde_json::from_str(&header.10)
            .map_err(|_| AppError::Internal("stored invoice payment methods are invalid".into()))?,
        contacts: serde_json::from_str(&header.11).map_err(|_| {
            AppError::Internal("stored invoice payment contacts are invalid".into())
        })?,
        payment_profile_updated_at: header.12,
        declaration: declaration_view_for_invoice(conn, invoice_id)?,
        dispute: dispute_view_for_invoice(conn, invoice_id)?,
        lines,
    }))
}

impl AppStore {
    pub async fn market_billing_dashboard(
        &self,
        session: &AuthSession,
    ) -> Result<BillingDashboardView, AppError> {
        let conn = self.conn.lock().await;
        let account_rows = conn
            .prepare(
                "SELECT account.id, account.buyer_user_id, account.buyer_email,
                        account.supplier_user_id, account.supplier_email, account.currency,
                        account.status, account.balance_units, account.open_invoice_id,
                        account.close_requested, account.credit_kind,
                        account.credit_limit_minor,
                        account.created_at, account.updated_at
                 FROM market_credit_accounts account
                 JOIN supplier_billing_profiles profile
                   ON profile.supplier_user_id = account.supplier_user_id
                  AND profile.currency = account.currency
                 WHERE (account.buyer_user_id = ?1 OR account.supplier_user_id = ?1)
                   AND account.currency = 'USD'
                 ORDER BY account.updated_at DESC, account.id",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![session.user_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, i64>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, i64>(9)? != 0,
                            row.get::<_, String>(10)?,
                            row.get::<_, Option<i64>>(11)?,
                            row.get::<_, String>(12)?,
                            row.get::<_, String>(13)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read market billing dashboard accounts"))?;
        let mut accounts = Vec::with_capacity(account_rows.len());
        let now = Utc::now();
        for row in account_rows {
            let services = service_views_for_account(&conn, &row.0)?;
            let daily_rate_minor = services
                .iter()
                .filter(|service| service.status == CONTRACT_ACTIVE)
                .map(|service| service.daily_rate_minor)
                .try_fold(0_i64, |total, rate| {
                    total.checked_add(rate).ok_or_else(|| {
                        AppError::Internal("market daily exposure overflowed".into())
                    })
                })?;
            let limit_units = row
                .11
                .map(|limit| limit.saturating_mul(MONEY_UNITS_PER_MINOR));
            let utilization_bps = limit_units.map(|limit_units| {
                if limit_units <= 0 {
                    10_000
                } else {
                    i64::try_from(i128::from(row.7) * 10_000_i128 / i128::from(limit_units))
                        .unwrap_or(10_000)
                        .clamp(0, 10_000)
                }
            });
            let estimated_settlement_at = limit_units.and_then(|limit_units| {
                let remaining_units = limit_units.saturating_sub(row.7);
                if daily_rate_minor > 0 && remaining_units > 0 {
                    let seconds =
                        remaining_units.saturating_add(daily_rate_minor - 1) / daily_rate_minor;
                    Some((now + Duration::seconds(seconds)).to_rfc3339())
                } else {
                    None
                }
            });
            let open_invoice = row
                .8
                .as_deref()
                .map(|invoice_id| invoice_view(&conn, invoice_id))
                .transpose()?
                .flatten();
            let is_buyer = row.1 == session.user_id;
            let is_supplier = row.3 == session.user_id;
            accounts.push(CreditAccountView {
                id: row.0,
                buyer_user_id: row.1,
                buyer_email: row.2,
                supplier_user_id: row.3,
                supplier_email: row.4,
                currency: row.5,
                status: row.6.clone(),
                balance_minor: ceil_minor(row.7),
                credit_kind: row.10,
                credit_limit_minor: row.11,
                utilization_bps,
                daily_rate_minor,
                estimated_settlement_at,
                is_buyer,
                is_supplier,
                can_settle: is_buyer
                    && row.7 > 0
                    && matches!(row.6.as_str(), ACCOUNT_ACTIVE | ACCOUNT_NEAR_CREDIT_LIMIT),
                can_close: is_supplier && row.6 != ACCOUNT_CLOSED && !row.9,
                close_requested: row.9,
                services,
                open_invoice,
                created_at: row.12,
                updated_at: row.13,
            });
        }
        let supplier_profiles = conn
            .prepare(
                "SELECT currency, settlement_grace_hours, revision, updated_at
                 FROM supplier_billing_profiles
                 WHERE supplier_user_id = ?1 AND currency = 'USD'
                 ORDER BY currency",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![session.user_id], |row| {
                        Ok(SupplierBillingProfileView {
                            currency: row.get(0)?,
                            settlement_grace_hours: row.get(1)?,
                            revision: row.get(2)?,
                            updated_at: row.get(3)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read supplier billing profiles"))?;
        let restrictions = conn
            .prepare(
                "SELECT id, invoice_id, reason, created_at
                 FROM market_credit_restrictions
                 WHERE buyer_user_id = ?1 AND status = 'active'
                 ORDER BY created_at DESC",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![session.user_id], |row| {
                        Ok(CreditRestrictionView {
                            id: row.get(0)?,
                            invoice_id: row.get(1)?,
                            reason: row.get(2)?,
                            created_at: row.get(3)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read market credit restrictions"))?;
        Ok(BillingDashboardView {
            accounts,
            supplier_profiles,
            restrictions,
            trial_hours: TRIAL_SECONDS / 3_600,
        })
    }

    pub async fn market_billing_invoice_history(
        &self,
        session: &AuthSession,
        account_id: &str,
        before_sequence: Option<i64>,
        limit: usize,
    ) -> Result<BillingInvoiceHistoryView, AppError> {
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(map_db("begin market invoice history read"))?;
        account_for_actor_tx(&tx, account_id, &session.user_id)?;
        let fetch_limit = i64::try_from(limit.saturating_add(1))
            .map_err(|_| AppError::BadRequest("invoice history limit is invalid".into()))?;
        let invoice_ids = tx
            .prepare(
                "SELECT id FROM market_invoices
                 WHERE account_id = ?1 AND status IN ('paid', 'void')
                   AND (?2 IS NULL OR sequence < ?2)
                 ORDER BY sequence DESC LIMIT ?3",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![account_id, before_sequence, fetch_limit], |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read market invoice history"))?;
        let has_more = invoice_ids.len() > limit;
        let mut invoices = Vec::with_capacity(invoice_ids.len().min(limit));
        for invoice_id in invoice_ids.into_iter().take(limit) {
            invoices.push(invoice_view(&tx, &invoice_id)?.ok_or_else(|| {
                AppError::Internal("market invoice history entry is missing".into())
            })?);
        }
        let next_before_sequence = has_more.then(|| {
            invoices
                .last()
                .map(|invoice| invoice.sequence)
                .unwrap_or_default()
        });
        tx.commit()
            .map_err(map_db("commit market invoice history read"))?;
        Ok(BillingInvoiceHistoryView {
            invoices,
            next_before_sequence,
        })
    }
}

fn account_for_actor_tx(
    tx: &Transaction<'_>,
    account_id: &str,
    actor_user_id: &str,
) -> Result<(AccountRow, bool, bool), AppError> {
    let account = load_account_row_tx(tx, account_id)?;
    let is_buyer = account.buyer_user_id == actor_user_id;
    let is_supplier = account.supplier_user_id == actor_user_id;
    if !is_buyer && !is_supplier {
        return Err(AppError::Forbidden(
            "not allowed to manage this supplier credit account".into(),
        ));
    }
    Ok((account, is_buyer, is_supplier))
}

fn invoice_actor_tx(
    tx: &Transaction<'_>,
    invoice_id: &str,
    actor_user_id: &str,
) -> Result<(String, String, String, String, bool, bool), AppError> {
    let row = tx
        .query_row(
            "SELECT invoice.account_id, invoice.status, invoice.deadline_at,
                    account.buyer_user_id, account.supplier_user_id
             FROM market_invoices invoice
             JOIN market_credit_accounts account ON account.id = invoice.account_id
             WHERE invoice.id = ?1",
            params![invoice_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read market invoice actor"))?
        .ok_or_else(|| AppError::NotFound("market invoice not found".into()))?;
    let is_buyer = row.3 == actor_user_id;
    let is_supplier = row.4 == actor_user_id;
    if !is_buyer && !is_supplier {
        return Err(AppError::Forbidden(
            "not allowed to manage this market invoice".into(),
        ));
    }
    Ok((row.0, row.1, row.2, row.3, is_buyer, is_supplier))
}

impl AppStore {
    pub async fn market_billing_open_account_invoice(
        &self,
        actor_user_id: &str,
        account_id: &str,
        close_requested: bool,
        supplier_requested: bool,
    ) -> Result<Vec<BillingAction>, AppError> {
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin voluntary market settlement"))?;
        let (account, is_buyer, is_supplier) =
            account_for_actor_tx(&tx, account_id, actor_user_id)?;
        if close_requested && !is_supplier {
            return Err(AppError::Forbidden(
                "only the supplier can close a credit relationship".into(),
            ));
        }
        if !close_requested && supplier_requested && !is_supplier {
            return Err(AppError::Forbidden(
                "only the supplier can request supplier settlement".into(),
            ));
        }
        if !close_requested && !supplier_requested && !is_buyer {
            return Err(AppError::Forbidden(
                "only the buyer can request early settlement".into(),
            ));
        }
        if account.status == ACCOUNT_CLOSED {
            return Err(AppError::Conflict(
                "this supplier credit relationship is permanently closed".into(),
            ));
        }
        if close_requested
            && !matches!(
                account.status.as_str(),
                ACCOUNT_ACTIVE | ACCOUNT_NEAR_CREDIT_LIMIT
            )
        {
            if !account.close_requested {
                let invoice_id = account.open_invoice_id.as_deref().ok_or_else(|| {
                    AppError::Internal("settling market account is missing its open invoice".into())
                })?;
                tx.execute(
                    "UPDATE market_credit_accounts
                     SET close_requested = 1, updated_at = ?2 WHERE id = ?1",
                    params![account.id, now_text],
                )
                .map_err(map_db("latch market account closure"))?;
                tx.execute(
                    "UPDATE market_service_contracts
                     SET status = CASE WHEN status IN ('trial', 'active')
                             THEN 'billing_suspended' ELSE status END,
                         desired_control_state = 'terminated',
                         control_error = 'supplier_credit_closed',
                         suspended_at = COALESCE(suspended_at, ?2), updated_at = ?2
                     WHERE account_id = ?1 AND status != 'terminated'",
                    params![account.id, now_text],
                )
                .map_err(map_db("terminate contracts for settling account closure"))?;
                record_event_tx(
                    &tx,
                    Some(&account.id),
                    None,
                    Some(invoice_id),
                    Some(actor_user_id),
                    "account_closure_requested",
                    serde_json::json!({ "balanceMinor": ceil_minor(account.balance_units) }),
                    &format!("account-closure-latched:{}", account.id),
                    &now_text,
                )?;
            }
            let actions = pending_control_actions_tx(&tx)?;
            tx.commit()
                .map_err(map_db("commit settling market account closure"))?;
            return Ok(actions);
        }
        if !matches!(
            account.status.as_str(),
            ACCOUNT_ACTIVE | ACCOUNT_NEAR_CREDIT_LIMIT
        ) {
            return Err(AppError::Conflict(
                "this supplier account is already awaiting settlement".into(),
            ));
        }
        if account.balance_units <= 0 {
            if !close_requested {
                return Err(AppError::Conflict(
                    "this supplier account has no billable balance".into(),
                ));
            }
            tx.execute(
                "UPDATE market_credit_accounts
                 SET status = 'closed', close_requested = 1, updated_at = ?2
                 WHERE id = ?1",
                params![account.id, now_text],
            )
            .map_err(map_db("close empty market credit account"))?;
            tx.execute(
                "UPDATE market_service_contracts
                 SET status = 'billing_suspended', desired_control_state = 'terminated',
                     control_error = 'supplier_credit_closed',
                     suspended_at = COALESCE(suspended_at, ?2), updated_at = ?2
                 WHERE account_id = ?1 AND status IN ('trial', 'active')",
                params![account.id, now_text],
            )
            .map_err(map_db("suspend contracts for empty account closure"))?;
            record_event_tx(
                &tx,
                Some(&account.id),
                None,
                None,
                Some(actor_user_id),
                "credit_account_closed",
                serde_json::json!({ "balanceMinor": 0 }),
                &format!("account-closed:{}:{now_text}", account.id),
                &now_text,
            )?;
        } else {
            open_invoice_tx(
                &tx,
                &account,
                now,
                if close_requested {
                    "account_closure_requested"
                } else if supplier_requested {
                    "supplier_settlement_requested"
                } else {
                    "voluntary_settlement_requested"
                },
                Some(actor_user_id),
            )?;
            if close_requested {
                tx.execute(
                    "UPDATE market_credit_accounts SET close_requested = 1, updated_at = ?2
                     WHERE id = ?1",
                    params![account.id, now_text],
                )
                .map_err(map_db("mark market account closure requested"))?;
                tx.execute(
                    "UPDATE market_service_contracts
                     SET desired_control_state = 'terminated',
                         control_error = 'supplier_credit_closed', updated_at = ?2
                     WHERE account_id = ?1 AND status = 'billing_suspended'",
                    params![account.id, now_text],
                )
                .map_err(map_db("terminate contracts for market account closure"))?;
            }
        }
        let actions = pending_control_actions_tx(&tx)?;
        tx.commit()
            .map_err(map_db("commit voluntary market settlement"))?;
        Ok(actions)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn market_billing_declare_payment(
        &self,
        session: &AuthSession,
        invoice_id: &str,
        payment_method_kind: Option<String>,
        payment_reference: Option<String>,
        note: Option<String>,
        evidence_url: Option<String>,
    ) -> Result<(), AppError> {
        let payment_method_kind = payment_method_kind
            .map(|value| crate::store::client_chat::sanitize_system_event_text(&value));
        let payment_reference = payment_reference
            .map(|value| crate::store::client_chat::sanitize_system_event_text(&value));
        let note = note.map(|value| crate::store::client_chat::sanitize_system_event_text(&value));
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin market payment declaration"))?;
        let (account_id, invoice_status, _, _, is_buyer, _) =
            invoice_actor_tx(&tx, invoice_id, &session.user_id)?;
        if !is_buyer {
            return Err(AppError::Forbidden(
                "only the buyer can declare invoice payment".into(),
            ));
        }
        if !matches!(invoice_status.as_str(), INVOICE_OPEN | INVOICE_OVERDUE) {
            return Err(AppError::Conflict(
                "this invoice cannot accept a payment declaration".into(),
            ));
        }
        tx.execute(
            "UPDATE market_payment_declarations
             SET status = 'superseded' WHERE invoice_id = ?1 AND status = 'declared'",
            params![invoice_id],
        )
        .map_err(map_db("supersede prior payment declaration"))?;
        let declaration_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO market_payment_declarations (
                id, invoice_id, buyer_user_id, status, payment_method_kind,
                payment_reference, note, evidence_url, declared_at
             ) VALUES (?1, ?2, ?3, 'declared', ?4, ?5, ?6, ?7, ?8)",
            params![
                declaration_id,
                invoice_id,
                session.user_id,
                payment_method_kind,
                payment_reference,
                note,
                evidence_url,
                now,
            ],
        )
        .map_err(map_db("create market payment declaration"))?;
        tx.execute(
            "UPDATE market_invoices
             SET status = 'payment_declared', declared_at = ?2
             WHERE id = ?1",
            params![invoice_id, now],
        )
        .map_err(map_db("mark market invoice payment declared"))?;
        tx.execute(
            "UPDATE market_credit_accounts SET status = 'payment_declared', updated_at = ?2
             WHERE id = ?1",
            params![account_id, now],
        )
        .map_err(map_db("mark market account payment declared"))?;
        record_event_tx(
            &tx,
            Some(&account_id),
            None,
            Some(invoice_id),
            Some(&session.user_id),
            "payment_declared",
            serde_json::json!({ "declarationId": declaration_id }),
            &format!("payment-declared:{declaration_id}"),
            &now,
        )?;
        tx.commit()
            .map_err(map_db("commit market payment declaration"))?;
        Ok(())
    }

    pub async fn market_billing_confirm_payment(
        &self,
        session: &AuthSession,
        invoice_id: &str,
    ) -> Result<Vec<BillingAction>, AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin market receipt confirmation"))?;
        let (account_id, invoice_status, _, _, _, is_supplier) =
            invoice_actor_tx(&tx, invoice_id, &session.user_id)?;
        if !is_supplier {
            return Err(AppError::Forbidden(
                "only the supplier can confirm receipt".into(),
            ));
        }
        if invoice_status != INVOICE_PAYMENT_DECLARED {
            return Err(AppError::Conflict(
                "the buyer has not declared payment for this invoice".into(),
            ));
        }
        let declaration_id = tx
            .query_row(
                "SELECT id FROM market_payment_declarations
                 WHERE invoice_id = ?1 AND status = 'declared'",
                params![invoice_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("read declared payment for receipt"))?
            .ok_or_else(|| AppError::Conflict("active payment declaration is missing".into()))?;
        tx.execute(
            "INSERT INTO market_payment_receipts (
                id, invoice_id, declaration_id, supplier_user_id, confirmed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Uuid::new_v4().to_string(),
                invoice_id,
                declaration_id,
                session.user_id,
                now,
            ],
        )
        .map_err(map_db("create market payment receipt"))?;
        tx.execute(
            "UPDATE market_payment_declarations SET status = 'confirmed'
             WHERE id = ?1",
            params![declaration_id],
        )
        .map_err(map_db("confirm market payment declaration"))?;
        tx.execute(
            "UPDATE market_invoices SET status = 'paid', paid_at = ?2 WHERE id = ?1",
            params![invoice_id, now],
        )
        .map_err(map_db("mark market invoice paid"))?;
        let (close_requested, credit_kind) = tx
            .query_row(
                "SELECT close_requested, credit_kind FROM market_credit_accounts WHERE id = ?1",
                params![account_id],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, String>(1)?)),
            )
            .map_err(map_db("read market account settlement state"))?;
        let credit_revoked = credit_kind == crate::market_access::CREDIT_NONE;
        tx.execute(
            "UPDATE market_credit_accounts
             SET status = ?2, balance_units = 0, open_invoice_id = NULL,
                 version = version + 1, updated_at = ?3 WHERE id = ?1",
            params![
                account_id,
                if close_requested {
                    ACCOUNT_CLOSED
                } else {
                    ACCOUNT_ACTIVE
                },
                now,
            ],
        )
        .map_err(map_db("settle market credit account"))?;
        if !close_requested && !credit_revoked {
            tx.execute(
                "UPDATE market_service_contracts
                 SET status = CASE WHEN trial_seconds_remaining > 0 THEN 'trial' ELSE 'active' END,
                     desired_control_state = 'active', applied_control_state = 'suspended',
                     suspended_at = NULL,
                     last_evaluated_at = ?2, updated_at = ?2
                 WHERE account_id = ?1 AND status = 'billing_suspended'",
                params![account_id, now],
            )
            .map_err(map_db("resume settled market contracts"))?;
        } else {
            tx.execute(
                "UPDATE market_service_contracts
                 SET desired_control_state = 'terminated',
                     control_error = ?2, updated_at = ?3
                 WHERE account_id = ?1 AND status = 'billing_suspended'",
                params![
                    account_id,
                    if close_requested {
                        "supplier_credit_closed"
                    } else {
                        "supplier_credit_revoked"
                    },
                    now,
                ],
            )
            .map_err(map_db(
                "retain market contract termination after settlement",
            ))?;
        }
        tx.execute(
            "UPDATE market_credit_restrictions SET status = 'lifted', lifted_at = ?2
             WHERE invoice_id = ?1 AND status = 'active'",
            params![invoice_id, now],
        )
        .map_err(map_db("lift settled market credit restriction"))?;
        record_event_tx(
            &tx,
            Some(&account_id),
            None,
            Some(invoice_id),
            Some(&session.user_id),
            "payment_received",
            serde_json::json!({
                "declarationId": declaration_id,
                "accountClosed": close_requested,
                "creditRevoked": credit_revoked,
            }),
            &format!("payment-received:{invoice_id}"),
            &now,
        )?;
        let actions = pending_control_actions_tx(&tx)?;
        tx.commit()
            .map_err(map_db("commit market receipt confirmation"))?;
        Ok(actions)
    }

    pub async fn market_billing_reject_payment(
        &self,
        session: &AuthSession,
        invoice_id: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        let reason = crate::store::client_chat::sanitize_system_event_text(reason);
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin market payment rejection"))?;
        let (account_id, invoice_status, deadline_at, buyer_user_id, _, is_supplier) =
            invoice_actor_tx(&tx, invoice_id, &session.user_id)?;
        if !is_supplier {
            return Err(AppError::Forbidden(
                "only the supplier can reject a payment declaration".into(),
            ));
        }
        if invoice_status != INVOICE_PAYMENT_DECLARED {
            return Err(AppError::Conflict(
                "this invoice has no active payment declaration".into(),
            ));
        }
        tx.execute(
            "UPDATE market_payment_declarations
             SET status = 'rejected', rejected_at = ?2, rejection_reason = ?3
             WHERE invoice_id = ?1 AND status = 'declared'",
            params![invoice_id, now, reason],
        )
        .map_err(map_db("reject market payment declaration"))?;
        let overdue = parse_time(&deadline_at)? <= now_dt;
        tx.execute(
            "UPDATE market_invoices
             SET status = ?2, overdue_at = CASE WHEN ?2 = 'overdue' THEN COALESCE(overdue_at, ?3) ELSE overdue_at END
             WHERE id = ?1",
            params![
                invoice_id,
                if overdue { INVOICE_OVERDUE } else { INVOICE_OPEN },
                now,
            ],
        )
        .map_err(map_db("reopen rejected market invoice"))?;
        tx.execute(
            "UPDATE market_credit_accounts SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![
                account_id,
                if overdue {
                    ACCOUNT_OVERDUE
                } else {
                    ACCOUNT_SETTLEMENT_DUE
                },
                now,
            ],
        )
        .map_err(map_db("reopen rejected market account settlement"))?;
        if overdue {
            tx.execute(
                "INSERT INTO market_credit_restrictions (
                    id, buyer_user_id, invoice_id, reason, status, created_at
                 ) VALUES (?1, ?2, ?3, 'payment_rejected_after_deadline', 'active', ?4)
                 ON CONFLICT(invoice_id) DO UPDATE SET
                    reason = excluded.reason, status = 'active', lifted_at = NULL",
                params![Uuid::new_v4().to_string(), buyer_user_id, invoice_id, now],
            )
            .map_err(map_db(
                "restore overdue restriction after payment rejection",
            ))?;
        }
        record_event_tx(
            &tx,
            Some(&account_id),
            None,
            Some(invoice_id),
            Some(&session.user_id),
            "payment_rejected",
            serde_json::json!({ "reason": reason, "overdue": overdue }),
            &format!("payment-rejected:{invoice_id}:{now}"),
            &now,
        )?;
        tx.commit()
            .map_err(map_db("commit market payment rejection"))?;
        Ok(())
    }

    pub async fn market_billing_open_dispute(
        &self,
        session: &AuthSession,
        invoice_id: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        let reason = crate::store::client_chat::sanitize_system_event_text(reason);
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin market billing dispute"))?;
        let (account_id, invoice_status, _, _, is_buyer, _) =
            invoice_actor_tx(&tx, invoice_id, &session.user_id)?;
        if !is_buyer {
            return Err(AppError::Forbidden(
                "only the buyer can open an invoice dispute".into(),
            ));
        }
        if !matches!(
            invoice_status.as_str(),
            INVOICE_OPEN | INVOICE_OVERDUE | INVOICE_PAYMENT_DECLARED
        ) {
            return Err(AppError::Conflict("this invoice cannot be disputed".into()));
        }
        if tx
            .query_row(
                "SELECT 1 FROM market_billing_disputes
                 WHERE invoice_id = ?1 LIMIT 1",
                params![invoice_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_db("check existing market billing dispute"))?
            .is_some()
        {
            return Err(AppError::Conflict(
                "this invoice has already been disputed".into(),
            ));
        }
        let dispute_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO market_billing_disputes (
                id, invoice_id, opened_by_user_id, reason, status, created_at
             ) VALUES (?1, ?2, ?3, ?4, 'open', ?5)",
            params![dispute_id, invoice_id, session.user_id, reason, now],
        )
        .map_err(map_db("open market billing dispute"))?;
        tx.execute(
            "UPDATE market_invoices SET status = 'disputed', disputed_at = ?2 WHERE id = ?1",
            params![invoice_id, now],
        )
        .map_err(map_db("mark market invoice disputed"))?;
        tx.execute(
            "UPDATE market_credit_accounts SET status = 'disputed', updated_at = ?2 WHERE id = ?1",
            params![account_id, now],
        )
        .map_err(map_db("mark market credit account disputed"))?;
        record_event_tx(
            &tx,
            Some(&account_id),
            None,
            Some(invoice_id),
            Some(&session.user_id),
            "invoice_disputed",
            serde_json::json!({ "disputeId": dispute_id, "reason": reason }),
            &format!("invoice-disputed:{dispute_id}"),
            &now,
        )?;
        tx.commit()
            .map_err(map_db("commit market billing dispute"))?;
        Ok(())
    }

    async fn market_billing_admin_disputes(
        &self,
    ) -> Result<Vec<AdminBillingDisputeView>, AppError> {
        let conn = self.conn.lock().await;
        let rows = conn
            .prepare(
                "SELECT dispute.id, dispute.invoice_id, dispute.reason, dispute.status,
                        dispute.resolution, dispute.created_at, dispute.resolved_at,
                        account.id, account.buyer_email, account.supplier_email
                 FROM market_billing_disputes dispute
                 JOIN market_invoices invoice ON invoice.id = dispute.invoice_id
                 JOIN market_credit_accounts account ON account.id = invoice.account_id
                 WHERE dispute.status = 'open'
                 ORDER BY dispute.created_at, dispute.id",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((
                            BillingDisputeView {
                                id: row.get(0)?,
                                reason: row.get(2)?,
                                status: row.get(3)?,
                                resolution: row.get(4)?,
                                created_at: row.get(5)?,
                                resolved_at: row.get(6)?,
                            },
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, String>(9)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read open market billing disputes"))?;
        rows.into_iter()
            .map(
                |(dispute, invoice_id, account_id, buyer_email, supplier_email)| {
                    let invoice = invoice_view(&conn, &invoice_id)?.ok_or_else(|| {
                        AppError::Internal("disputed market invoice is missing".into())
                    })?;
                    Ok(AdminBillingDisputeView {
                        dispute,
                        account_id,
                        buyer_email,
                        supplier_email,
                        invoice,
                    })
                },
            )
            .collect()
    }

    pub async fn market_billing_resolve_dispute(
        &self,
        session: &AuthSession,
        dispute_id: &str,
        resolution: &str,
        note: Option<&str>,
    ) -> Result<Vec<BillingAction>, AppError> {
        let sanitized_note = note.map(crate::store::client_chat::sanitize_system_event_text);
        let note = sanitized_note.as_deref();
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin market dispute resolution"))?;
        let invoice_id = tx
            .query_row(
                "SELECT invoice_id FROM market_billing_disputes
                 WHERE id = ?1 AND status = 'open'",
                params![dispute_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("read open market billing dispute"))?
            .ok_or_else(|| AppError::NotFound("open market billing dispute not found".into()))?;
        let (account_id, invoice_status, deadline_at, overdue_at, buyer_user_id) = tx
            .query_row(
                "SELECT invoice.account_id, invoice.status, invoice.deadline_at,
                        invoice.overdue_at, account.buyer_user_id
                 FROM market_invoices invoice
                 JOIN market_credit_accounts account ON account.id = invoice.account_id
                 WHERE invoice.id = ?1",
                params![invoice_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(map_db("read disputed market invoice"))?;
        if invoice_status != INVOICE_DISPUTED {
            return Err(AppError::Conflict(
                "the disputed invoice is no longer awaiting resolution".into(),
            ));
        }
        if resolution == "void" {
            void_invoice_tx(
                &tx,
                &invoice_id,
                &account_id,
                &buyer_user_id,
                session,
                note.unwrap_or("dispute resolved in favor of buyer"),
                &now,
            )?;
        } else {
            let has_active_declaration = tx
                .query_row(
                    "SELECT 1 FROM market_payment_declarations
                     WHERE invoice_id = ?1 AND status = 'declared' LIMIT 1",
                    params![invoice_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(map_db("read disputed payment declaration"))?
                .is_some();
            let overdue = overdue_at.is_some() || parse_time(&deadline_at)? <= now_dt;
            let next_invoice_status = if has_active_declaration {
                INVOICE_PAYMENT_DECLARED
            } else if overdue {
                INVOICE_OVERDUE
            } else {
                INVOICE_OPEN
            };
            let next_account_status = if has_active_declaration {
                ACCOUNT_PAYMENT_DECLARED
            } else if overdue {
                ACCOUNT_OVERDUE
            } else {
                ACCOUNT_SETTLEMENT_DUE
            };
            tx.execute(
                "UPDATE market_invoices SET status = ?2 WHERE id = ?1 AND status = 'disputed'",
                params![invoice_id, next_invoice_status],
            )
            .map_err(map_db("uphold disputed market invoice"))?;
            tx.execute(
                "UPDATE market_credit_accounts SET status = ?2, updated_at = ?3
                 WHERE id = ?1",
                params![account_id, next_account_status, now],
            )
            .map_err(map_db("restore upheld market account settlement"))?;
            if overdue {
                tx.execute(
                    "INSERT INTO market_credit_restrictions (
                        id, buyer_user_id, invoice_id, reason, status, created_at
                     ) VALUES (?1, ?2, ?3, 'invoice_upheld_overdue', 'active', ?4)
                     ON CONFLICT(invoice_id) DO UPDATE SET
                        reason = excluded.reason, status = 'active', lifted_at = NULL",
                    params![Uuid::new_v4().to_string(), buyer_user_id, invoice_id, now],
                )
                .map_err(map_db("restore restriction for upheld invoice"))?;
            }
        }
        tx.execute(
            "UPDATE market_billing_disputes
             SET status = 'resolved', resolution = ?2, resolved_at = ?3
             WHERE id = ?1 AND status = 'open'",
            params![dispute_id, resolution, now],
        )
        .map_err(map_db("resolve market billing dispute"))?;
        record_event_tx(
            &tx,
            Some(&account_id),
            None,
            Some(&invoice_id),
            Some(&session.user_id),
            "invoice_dispute_resolved",
            serde_json::json!({
                "disputeId": dispute_id,
                "resolution": resolution,
                "note": note,
            }),
            &format!("invoice-dispute-resolved:{dispute_id}"),
            &now,
        )?;
        let actions = pending_control_actions_tx(&tx)?;
        tx.commit()
            .map_err(map_db("commit market dispute resolution"))?;
        Ok(actions)
    }

    pub async fn market_billing_void_invoice(
        &self,
        session: &AuthSession,
        invoice_id: &str,
        reason: &str,
    ) -> Result<Vec<BillingAction>, AppError> {
        let reason = crate::store::client_chat::sanitize_system_event_text(reason);
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin market invoice void"))?;
        let row = tx
            .query_row(
                "SELECT invoice.account_id, invoice.status, account.buyer_user_id
                 FROM market_invoices invoice
                 JOIN market_credit_accounts account ON account.id = invoice.account_id
                 WHERE invoice.id = ?1",
                params![invoice_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read market invoice for void"))?
            .ok_or_else(|| AppError::NotFound("market invoice not found".into()))?;
        if matches!(row.1.as_str(), INVOICE_PAID | INVOICE_VOID) {
            return Err(AppError::Conflict(
                "a paid or already voided invoice cannot be voided".into(),
            ));
        }
        void_invoice_tx(&tx, invoice_id, &row.0, &row.2, session, &reason, &now)?;
        tx.execute(
            "UPDATE market_billing_disputes
             SET status = 'resolved', resolution = 'void', resolved_at = ?2
             WHERE invoice_id = ?1 AND status = 'open'",
            params![invoice_id, now],
        )
        .map_err(map_db("resolve dispute for voided market invoice"))?;
        let actions = pending_control_actions_tx(&tx)?;
        tx.commit().map_err(map_db("commit market invoice void"))?;
        Ok(actions)
    }
}

fn void_invoice_tx(
    tx: &Transaction<'_>,
    invoice_id: &str,
    account_id: &str,
    buyer_user_id: &str,
    actor: &AuthSession,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    let (close_requested, credit_kind) = tx
        .query_row(
            "SELECT close_requested, credit_kind FROM market_credit_accounts WHERE id = ?1",
            params![account_id],
            |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, String>(1)?)),
        )
        .map_err(map_db("read market account settlement state for void"))?;
    let credit_revoked = credit_kind == crate::market_access::CREDIT_NONE;
    tx.execute(
        "UPDATE market_invoices SET status = 'void', voided_at = ?2 WHERE id = ?1",
        params![invoice_id, now],
    )
    .map_err(map_db("void market invoice"))?;
    tx.execute(
        "UPDATE market_payment_declarations SET status = 'superseded'
         WHERE invoice_id = ?1 AND status = 'declared'",
        params![invoice_id],
    )
    .map_err(map_db("supersede declarations for voided invoice"))?;
    tx.execute(
        "UPDATE market_accrual_entries SET status = 'voided', updated_at = ?2
         WHERE invoice_id = ?1",
        params![invoice_id, now],
    )
    .map_err(map_db("void market invoice accrual entries"))?;
    tx.execute(
        "UPDATE market_credit_accounts
         SET status = ?2, balance_units = 0, open_invoice_id = NULL,
             version = version + 1, updated_at = ?3 WHERE id = ?1",
        params![
            account_id,
            if close_requested {
                ACCOUNT_CLOSED
            } else {
                ACCOUNT_ACTIVE
            },
            now,
        ],
    )
    .map_err(map_db("clear voided market account balance"))?;
    tx.execute(
        "UPDATE market_credit_restrictions SET status = 'lifted', lifted_at = ?2
         WHERE invoice_id = ?1 AND buyer_user_id = ?3 AND status = 'active'",
        params![invoice_id, now, buyer_user_id],
    )
    .map_err(map_db("lift restriction for voided market invoice"))?;
    if close_requested || credit_revoked {
        tx.execute(
            "UPDATE market_service_contracts
             SET desired_control_state = 'terminated',
                 control_error = ?2, updated_at = ?3
             WHERE account_id = ?1 AND status = 'billing_suspended'",
            params![
                account_id,
                if close_requested {
                    "supplier_credit_closed"
                } else {
                    "supplier_credit_revoked"
                },
                now,
            ],
        )
        .map_err(map_db("retain service termination after invoice void"))?;
    } else {
        tx.execute(
            "UPDATE market_service_contracts
             SET status = CASE WHEN trial_seconds_remaining > 0 THEN 'trial' ELSE 'active' END,
                 desired_control_state = 'active', applied_control_state = 'suspended',
                 suspended_at = NULL,
                 last_evaluated_at = ?2, updated_at = ?2
             WHERE account_id = ?1 AND status = 'billing_suspended'",
            params![account_id, now],
        )
        .map_err(map_db("resume services after invoice void"))?;
    }
    record_event_tx(
        tx,
        Some(account_id),
        None,
        Some(invoice_id),
        Some(&actor.user_id),
        "invoice_voided",
        serde_json::json!({
            "reason": reason,
            "accountClosed": close_requested,
            "creditRevoked": credit_revoked,
        }),
        &format!("invoice-voided:{invoice_id}"),
        now,
    )?;
    Ok(())
}

fn latest_health_observation_tx(
    tx: &Transaction<'_>,
    product_kind: &str,
    product_ref: &str,
    service_ref: &str,
) -> Result<Option<(i64, String, String)>, AppError> {
    let service_active = match product_kind {
        "share" => tx
            .query_row(
                "SELECT 1 FROM share_market_subscriptions
                 WHERE id = ?1 AND status = 'active_postpaid'",
                params![product_ref],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_db("check billable Share entitlement state"))?
            .is_some(),
        "client_host" => tx
            .query_row(
                "SELECT 1 FROM client_market_subscriptions
                 WHERE installation_id = ?1 AND status = 'active'",
                params![product_ref],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_db("check billable Client subscription state"))?
            .is_some(),
        _ => false,
    };
    if !service_active {
        return Ok(None);
    }
    let (table, key) = match product_kind {
        "share" => ("share_health_checks", "share_id"),
        "client_host" => ("installation_health_checks", "installation_id"),
        _ => {
            return Err(AppError::Internal(format!(
                "unsupported billing product kind {product_kind}"
            )));
        }
    };
    tx.query_row(
        &format!(
            "SELECT checked_at, status, reason FROM {table}
             WHERE {key} = ?1 ORDER BY checked_at DESC LIMIT 1"
        ),
        params![service_ref],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(map_db("read latest service health observation"))
}

fn load_reconcile_contracts_tx(tx: &Transaction<'_>) -> Result<Vec<ContractRow>, AppError> {
    tx.prepare(
        "SELECT id, account_id, product_kind, product_ref, service_ref,
                daily_rate_minor, trial_seconds_remaining,
                health_state, last_evaluated_at
         FROM market_service_contracts
         WHERE status IN ('trial', 'active')
         ORDER BY account_id, id",
    )
    .and_then(|mut statement| {
        statement
            .query_map([], |row| {
                Ok(ContractRow {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    product_kind: row.get(2)?,
                    product_ref: row.get(3)?,
                    service_ref: row.get(4)?,
                    daily_rate_minor: row.get(5)?,
                    trial_seconds_remaining: row.get(6)?,
                    health_state: row.get(7)?,
                    last_evaluated_at: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(map_db("load market contracts for billing reconciliation"))
}

fn load_account_row_tx(tx: &Transaction<'_>, account_id: &str) -> Result<AccountRow, AppError> {
    tx.query_row(
        "SELECT account.id, account.buyer_user_id, account.supplier_user_id,
                account.currency, account.status, account.balance_units,
                account.open_invoice_id, account.close_requested,
                account.credit_kind, account.credit_limit_minor,
                profile.settlement_grace_hours,
                account.version
         FROM market_credit_accounts account
         JOIN supplier_billing_profiles profile
           ON profile.supplier_user_id = account.supplier_user_id
          AND profile.currency = account.currency
         WHERE account.id = ?1",
        params![account_id],
        |row| {
            Ok(AccountRow {
                id: row.get(0)?,
                buyer_user_id: row.get(1)?,
                supplier_user_id: row.get(2)?,
                currency: row.get(3)?,
                status: row.get(4)?,
                balance_units: row.get(5)?,
                open_invoice_id: row.get(6)?,
                close_requested: row.get::<_, i64>(7)? != 0,
                credit_kind: row.get(8)?,
                credit_limit_minor: row.get(9)?,
                settlement_grace_hours: row.get(10)?,
                version: row.get(11)?,
            })
        },
    )
    .optional()
    .map_err(map_db("read market credit account for reconciliation"))?
    .ok_or_else(|| AppError::NotFound("market credit account not found".into()))
}

fn build_accrual_candidate_tx(
    tx: &Transaction<'_>,
    contract: ContractRow,
    now: DateTime<Utc>,
) -> Result<AccrualCandidate, AppError> {
    let last = parse_time(&contract.last_evaluated_at)?;
    let gap = (now - last).num_seconds().max(0);
    let observation = latest_health_observation_tx(
        tx,
        &contract.product_kind,
        &contract.product_ref,
        &contract.service_ref,
    )?;
    let (observed_state, observation_reason) = match observation {
        Some((checked_at, status, reason))
            if status == "healthy" && now.timestamp() - checked_at <= HEALTH_FRESHNESS_SECS =>
        {
            ("healthy".to_string(), reason)
        }
        Some((_, status, reason)) if status == "unhealthy" => ("unhealthy".to_string(), reason),
        Some((_, _, reason)) => ("unknown".to_string(), reason),
        None => ("unknown".to_string(), "no_router_observation".to_string()),
    };
    let elapsed_seconds = if gap <= MAX_ACCRUAL_GAP_SECS { gap } else { 0 };
    let effective_state = if gap > MAX_ACCRUAL_GAP_SECS {
        "unknown".to_string()
    } else {
        observed_state
    };
    let trial_seconds = if effective_state == "healthy" {
        elapsed_seconds.min(contract.trial_seconds_remaining.max(0))
    } else {
        0
    };
    let billable_seconds = if effective_state == "healthy" {
        elapsed_seconds.saturating_sub(trial_seconds)
    } else {
        0
    };
    let requested_units = contract
        .daily_rate_minor
        .checked_mul(billable_seconds)
        .ok_or_else(|| AppError::Internal("market accrual amount overflowed".into()))?;
    Ok(AccrualCandidate {
        next_trial_seconds: contract
            .trial_seconds_remaining
            .saturating_sub(trial_seconds),
        contract,
        observed_state: effective_state,
        observation_reason: if gap > MAX_ACCRUAL_GAP_SECS {
            "billing_worker_gap".to_string()
        } else {
            observation_reason
        },
        interval_started_at: last.to_rfc3339(),
        interval_ended_at: now.to_rfc3339(),
        elapsed_seconds,
        trial_seconds,
        billable_seconds,
        requested_units,
    })
}

fn append_service_interval_tx(
    tx: &Transaction<'_>,
    candidate: &AccrualCandidate,
    now: &str,
) -> Result<(), AppError> {
    if candidate.elapsed_seconds == 0 && candidate.observed_state == candidate.contract.health_state
    {
        return Ok(());
    }
    let reusable = tx
        .query_row(
            "SELECT interval.id, interval.invoice_id
             FROM market_service_intervals interval
             WHERE interval.contract_id = ?1
             ORDER BY interval.updated_at DESC, interval.created_at DESC LIMIT 1",
            params![candidate.contract.id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(map_db("read current service interval"))?;
    let interval_id = if let Some((interval_id, None)) = reusable {
        let state: String = tx
            .query_row(
                "SELECT state FROM market_service_intervals WHERE id = ?1",
                params![interval_id],
                |row| row.get(0),
            )
            .map_err(map_db("read service interval state"))?;
        if state == candidate.observed_state {
            tx.execute(
                "UPDATE market_service_intervals
                 SET observation_reason = ?2, ended_at = ?3,
                     elapsed_seconds = elapsed_seconds + ?4,
                     trial_seconds = trial_seconds + ?5,
                     billable_seconds = billable_seconds + ?6,
                     amount_units = amount_units + ?7, updated_at = ?8
                 WHERE id = ?1",
                params![
                    interval_id,
                    candidate.observation_reason,
                    candidate.interval_ended_at,
                    candidate.elapsed_seconds,
                    candidate.trial_seconds,
                    candidate.billable_seconds,
                    candidate.requested_units,
                    now,
                ],
            )
            .map_err(map_db("extend market service interval"))?;
            interval_id
        } else {
            insert_service_interval_tx(tx, candidate, now)?
        }
    } else {
        insert_service_interval_tx(tx, candidate, now)?
    };
    if candidate.requested_units > 0 {
        tx.execute(
            "INSERT INTO market_accrual_entries (
                id, account_id, contract_id, interval_id, currency, daily_rate_minor,
                billable_seconds, amount_units, status, invoice_id, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4,
                       (SELECT currency FROM market_service_contracts WHERE id = ?3),
                       ?5, ?6, ?7, 'unbilled', NULL, ?8, ?8)
             ON CONFLICT(interval_id) DO UPDATE SET
                billable_seconds = market_accrual_entries.billable_seconds + excluded.billable_seconds,
                amount_units = market_accrual_entries.amount_units + excluded.amount_units,
                updated_at = excluded.updated_at",
            params![
                Uuid::new_v4().to_string(),
                candidate.contract.account_id,
                candidate.contract.id,
                interval_id,
                candidate.contract.daily_rate_minor,
                candidate.billable_seconds,
                candidate.requested_units,
                now,
            ],
        )
        .map_err(map_db("append market accrual entry"))?;
    }
    Ok(())
}

fn insert_service_interval_tx(
    tx: &Transaction<'_>,
    candidate: &AccrualCandidate,
    now: &str,
) -> Result<String, AppError> {
    let id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO market_service_intervals (
            id, contract_id, state, observation_reason, started_at, ended_at,
            elapsed_seconds, trial_seconds, billable_seconds, amount_units,
            invoice_id, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?11)",
        params![
            id,
            candidate.contract.id,
            candidate.observed_state,
            candidate.observation_reason,
            candidate.interval_started_at,
            candidate.interval_ended_at,
            candidate.elapsed_seconds,
            candidate.trial_seconds,
            candidate.billable_seconds,
            candidate.requested_units,
            now,
        ],
    )
    .map_err(map_db("insert market service interval"))?;
    Ok(id)
}

#[derive(Debug)]
struct InvoiceLineDraft {
    contract_id: String,
    product_kind: String,
    product_ref: String,
    service_ref: String,
    service_label: String,
    daily_rate_minor: i64,
    billable_seconds: i64,
    amount_units: i64,
    amount_minor: i64,
    started_at: String,
    ended_at: String,
}

fn open_invoice_tx(
    tx: &Transaction<'_>,
    account: &AccountRow,
    now: DateTime<Utc>,
    event_type: &str,
    actor_user_id: Option<&str>,
) -> Result<String, AppError> {
    if tx
        .query_row(
            "SELECT id FROM market_invoices
             WHERE account_id = ?1 AND status IN ('open', 'payment_declared', 'overdue', 'disputed')",
            params![account.id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("check existing market invoice"))?
        .is_some()
    {
        return Err(AppError::Conflict(
            "this supplier account already has an unsettled invoice".into(),
        ));
    }
    let mut drafts = tx
        .prepare(
            "SELECT contract.id, contract.product_kind, contract.product_ref,
                    contract.service_ref, contract.service_label, contract.daily_rate_minor,
                    SUM(accrual.billable_seconds), SUM(accrual.amount_units),
                    MIN(interval.started_at), MAX(interval.ended_at)
             FROM market_accrual_entries accrual
             JOIN market_service_contracts contract ON contract.id = accrual.contract_id
             JOIN market_service_intervals interval ON interval.id = accrual.interval_id
             WHERE accrual.account_id = ?1 AND accrual.status = 'unbilled'
             GROUP BY contract.id, contract.product_kind, contract.product_ref,
                      contract.service_ref, contract.service_label, contract.daily_rate_minor
             ORDER BY MIN(interval.started_at), contract.id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![account.id], |row| {
                    Ok(InvoiceLineDraft {
                        contract_id: row.get(0)?,
                        product_kind: row.get(1)?,
                        product_ref: row.get(2)?,
                        service_ref: row.get(3)?,
                        service_label: row.get(4)?,
                        daily_rate_minor: row.get(5)?,
                        billable_seconds: row.get(6)?,
                        amount_units: row.get(7)?,
                        amount_minor: 0,
                        started_at: row.get(8)?,
                        ended_at: row.get(9)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("build market invoice lines"))?;
    let amount_units = drafts.iter().try_fold(0_i64, |total, line| {
        total
            .checked_add(line.amount_units)
            .ok_or_else(|| AppError::Internal("market invoice amount overflowed".into()))
    })?;
    if amount_units <= 0 || drafts.is_empty() {
        return Err(AppError::Conflict(
            "this supplier account has no billable balance".into(),
        ));
    }
    let amount_minor = ceil_minor(amount_units);
    let mut assigned_minor = 0_i64;
    let mut remainders = Vec::with_capacity(drafts.len());
    for (index, line) in drafts.iter_mut().enumerate() {
        line.amount_minor = line.amount_units / MONEY_UNITS_PER_MINOR;
        assigned_minor = assigned_minor
            .checked_add(line.amount_minor)
            .ok_or_else(|| AppError::Internal("market invoice allocation overflowed".into()))?;
        remainders.push((line.amount_units % MONEY_UNITS_PER_MINOR, index));
    }
    remainders.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let mut minor_remaining = amount_minor - assigned_minor;
    for (_, index) in remainders {
        if minor_remaining == 0 {
            break;
        }
        drafts[index].amount_minor += 1;
        minor_remaining -= 1;
    }
    let sequence = tx
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM market_invoices WHERE account_id = ?1",
            params![account.id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("allocate market invoice sequence"))?;
    let invoice_id = Uuid::new_v4().to_string();
    let opened_at = now.to_rfc3339();
    let deadline_at = (now + Duration::hours(account.settlement_grace_hours)).to_rfc3339();
    let payment_profile = tx
        .query_row(
            "SELECT methods_json, COALESCE(contacts_json, '[]'), updated_at
             FROM account_payment_profiles WHERE user_id = ?1",
            params![account.supplier_user_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read invoice payment profile snapshot"))?
        .ok_or_else(|| {
            AppError::Conflict(
                "the supplier must restore payment methods before a bill can be issued".into(),
            )
        })?;
    let payment_methods: Vec<PaymentMethod> = serde_json::from_str(&payment_profile.0)
        .map_err(|_| AppError::Internal("stored supplier payment methods are invalid".into()))?;
    let _: Vec<PaymentContact> = serde_json::from_str(&payment_profile.1)
        .map_err(|_| AppError::Internal("stored supplier payment contacts are invalid".into()))?;
    if payment_methods.is_empty() {
        return Err(AppError::Conflict(
            "the supplier must restore payment methods before a bill can be issued".into(),
        ));
    }
    tx.execute(
        "INSERT INTO market_invoices (
            id, account_id, sequence, amount_minor, amount_units, currency,
            payment_methods_json, payment_contacts_json, payment_profile_updated_at,
            status, due_at, deadline_at, opened_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'open', ?10, ?11, ?10)",
        params![
            invoice_id,
            account.id,
            sequence,
            amount_minor,
            amount_units,
            account.currency,
            payment_profile.0,
            payment_profile.1,
            payment_profile.2,
            opened_at,
            deadline_at,
        ],
    )
    .map_err(map_db("open market invoice"))?;
    for line in drafts {
        tx.execute(
            "INSERT INTO market_invoice_lines (
                id, invoice_id, contract_id, product_kind, product_ref, service_ref,
                service_label, daily_rate_minor, billable_seconds, amount_minor,
                amount_units, service_started_at, service_ended_at, evidence_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                Uuid::new_v4().to_string(),
                invoice_id,
                line.contract_id,
                line.product_kind,
                line.product_ref,
                line.service_ref,
                line.service_label,
                line.daily_rate_minor,
                line.billable_seconds,
                line.amount_minor,
                line.amount_units,
                line.started_at,
                line.ended_at,
                serde_json::json!({
                    "source": "router_health_observation",
                    "moneyUnitsPerMinor": MONEY_UNITS_PER_MINOR,
                    "unknownTimeBillable": false,
                })
                .to_string(),
                opened_at,
            ],
        )
        .map_err(map_db("insert market invoice line"))?;
    }
    tx.execute(
        "UPDATE market_accrual_entries
         SET status = 'invoiced', invoice_id = ?2, updated_at = ?3
         WHERE account_id = ?1 AND status = 'unbilled'",
        params![account.id, invoice_id, opened_at],
    )
    .map_err(map_db("freeze market invoice accrual entries"))?;
    tx.execute(
        "UPDATE market_service_intervals
         SET invoice_id = ?2, updated_at = ?3
         WHERE contract_id IN (
             SELECT id FROM market_service_contracts WHERE account_id = ?1
         ) AND invoice_id IS NULL AND amount_units > 0",
        params![account.id, invoice_id, opened_at],
    )
    .map_err(map_db("freeze invoiced service intervals"))?;
    tx.execute(
        "UPDATE market_credit_accounts
         SET status = 'settlement_due', open_invoice_id = ?2,
             balance_units = ?3, version = version + 1, updated_at = ?4
         WHERE id = ?1",
        params![account.id, invoice_id, amount_units, opened_at],
    )
    .map_err(map_db("freeze market credit account"))?;
    tx.execute(
        "UPDATE market_service_contracts
         SET status = 'billing_suspended', desired_control_state = 'suspended',
             suspended_at = COALESCE(suspended_at, ?2), updated_at = ?2
         WHERE account_id = ?1 AND status IN ('trial', 'active')",
        params![account.id, opened_at],
    )
    .map_err(map_db("suspend market contracts for settlement"))?;
    record_event_tx(
        tx,
        Some(&account.id),
        None,
        Some(&invoice_id),
        actor_user_id,
        event_type,
        serde_json::json!({
            "amountMinor": amount_minor,
            "amountUsdMinor": amount_minor,
            "amountCnyMinor": usd_minor_to_cny_minor(amount_minor),
            "currency": account.currency,
            "deadlineAt": deadline_at,
        }),
        &format!("invoice-opened:{invoice_id}"),
        &opened_at,
    )?;
    Ok(invoice_id)
}

fn pending_control_actions_tx(tx: &Transaction<'_>) -> Result<Vec<BillingAction>, AppError> {
    tx.prepare(
        "SELECT id, desired_control_state, product_kind, product_ref, service_ref,
                COALESCE(control_error, 'settlement_required')
         FROM market_service_contracts
         WHERE desired_control_state != applied_control_state
           AND desired_control_state IN ('active', 'suspended', 'terminated')
         ORDER BY updated_at, id",
    )
    .and_then(|mut statement| {
        statement
            .query_map([], |row| {
                let desired: String = row.get(1)?;
                Ok(BillingAction {
                    contract_id: row.get(0)?,
                    kind: match desired.as_str() {
                        "active" => BillingActionKind::Resume,
                        "terminated" => BillingActionKind::Terminate,
                        _ => BillingActionKind::Suspend,
                    },
                    product_kind: row.get(2)?,
                    product_ref: row.get(3)?,
                    service_ref: row.get(4)?,
                    reason: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(map_db("load pending market billing controls"))
}

fn mark_overdue_invoices_tx(tx: &Transaction<'_>, now: &str) -> Result<(), AppError> {
    let overdue = tx
        .prepare(
            "SELECT invoice.id, invoice.account_id, account.buyer_user_id, invoice.status
             FROM market_invoices invoice
             JOIN market_credit_accounts account ON account.id = invoice.account_id
             WHERE invoice.status IN ('open', 'payment_declared', 'disputed')
               AND invoice.overdue_at IS NULL AND invoice.deadline_at <= ?1",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![now], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("load overdue market invoices"))?;
    for (invoice_id, account_id, buyer_user_id, invoice_status) in overdue {
        let next_invoice_status = if invoice_status == INVOICE_OPEN {
            INVOICE_OVERDUE
        } else {
            invoice_status.as_str()
        };
        tx.execute(
            "UPDATE market_invoices SET status = ?2, overdue_at = ?3
             WHERE id = ?1 AND overdue_at IS NULL",
            params![invoice_id, next_invoice_status, now],
        )
        .map_err(map_db("mark market invoice overdue"))?;
        if invoice_status == INVOICE_OPEN {
            tx.execute(
                "UPDATE market_credit_accounts SET status = 'overdue', updated_at = ?2
                 WHERE id = ?1",
                params![account_id, now],
            )
            .map_err(map_db("mark market credit account overdue"))?;
        }
        tx.execute(
            "INSERT INTO market_credit_restrictions (
                id, buyer_user_id, invoice_id, reason, status, created_at
             ) VALUES (?1, ?2, ?3, 'invoice_overdue', 'active', ?4)
             ON CONFLICT(invoice_id) DO UPDATE SET
                reason = excluded.reason, status = 'active', lifted_at = NULL",
            params![Uuid::new_v4().to_string(), buyer_user_id, invoice_id, now],
        )
        .map_err(map_db("apply market credit restriction"))?;
        record_event_tx(
            tx,
            Some(&account_id),
            None,
            Some(&invoice_id),
            None,
            "invoice_overdue",
            serde_json::json!({}),
            &format!("invoice-overdue:{invoice_id}"),
            now,
        )?;
    }
    Ok(())
}

fn open_final_invoices_tx(tx: &Transaction<'_>, now: DateTime<Utc>) -> Result<(), AppError> {
    let account_ids = tx
        .prepare(
            "SELECT account.id
             FROM market_credit_accounts account
             WHERE account.status IN ('active', 'near_credit_limit')
               AND account.balance_units > 0
               AND NOT EXISTS (
                   SELECT 1 FROM market_service_contracts contract
                   WHERE contract.account_id = account.id
                     AND contract.status IN ('trial', 'active')
               )
             ORDER BY account.updated_at, account.id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("load final market settlements"))?;
    for account_id in account_ids {
        let account = load_account_row_tx(tx, &account_id)?;
        open_invoice_tx(tx, &account, now, "final_service_ended", None)?;
    }
    Ok(())
}

impl AppStore {
    pub async fn market_billing_reconcile(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<BillingAction>, AppError> {
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin market billing reconciliation"))?;
        let contracts = load_reconcile_contracts_tx(&tx)?;
        let mut grouped = BTreeMap::<String, Vec<AccrualCandidate>>::new();
        for contract in contracts {
            let candidate = build_accrual_candidate_tx(&tx, contract, now)?;
            grouped
                .entry(candidate.contract.account_id.clone())
                .or_default()
                .push(candidate);
        }
        let now_text = now.to_rfc3339();
        for (account_id, candidates) in grouped {
            let account = load_account_row_tx(&tx, &account_id)?;
            if !matches!(
                account.status.as_str(),
                ACCOUNT_ACTIVE | ACCOUNT_NEAR_CREDIT_LIMIT
            ) {
                continue;
            }
            let accrued_units = candidates.iter().try_fold(0_i64, |total, candidate| {
                total
                    .checked_add(candidate.requested_units)
                    .ok_or_else(|| AppError::Internal("market account accrual overflowed".into()))
            })?;
            let next_balance = account
                .balance_units
                .checked_add(accrued_units)
                .ok_or_else(|| AppError::Internal("market account balance overflowed".into()))?;
            let credit_revoked = account.credit_kind == crate::market_access::CREDIT_NONE;
            let limit_units = match account.credit_kind.as_str() {
                crate::market_access::CREDIT_LIMITED => Some(
                    account
                        .credit_limit_minor
                        .ok_or_else(|| {
                            AppError::Internal(
                                "limited market credit account is missing its limit".into(),
                            )
                        })?
                        .checked_mul(MONEY_UNITS_PER_MINOR)
                        .ok_or_else(|| {
                            AppError::Internal("market credit limit overflowed".into())
                        })?,
                ),
                crate::market_access::CREDIT_UNLIMITED | crate::market_access::CREDIT_NONE => None,
                _ => {
                    return Err(AppError::Internal(
                        "market credit account has an invalid credit kind".into(),
                    ));
                }
            };
            let credit_limit_reached = limit_units.is_some_and(|limit| next_balance >= limit);
            for candidate in &candidates {
                append_service_interval_tx(&tx, candidate, &now_text)?;
                let next_status = if credit_revoked || credit_limit_reached {
                    CONTRACT_BILLING_SUSPENDED
                } else if candidate.next_trial_seconds > 0 {
                    CONTRACT_TRIAL
                } else {
                    CONTRACT_ACTIVE
                };
                tx.execute(
                    "UPDATE market_service_contracts
                     SET status = ?2, trial_seconds_remaining = ?3,
                         health_state = ?4, last_evaluated_at = ?5,
                         desired_control_state = CASE
                             WHEN ?6 = 1 THEN 'terminated'
                             WHEN ?2 = 'billing_suspended' THEN 'suspended'
                             ELSE desired_control_state END,
                         control_error = CASE WHEN ?6 = 1
                             THEN 'supplier_credit_revoked' ELSE control_error END,
                         suspended_at = CASE WHEN ?2 = 'billing_suspended'
                             THEN COALESCE(suspended_at, ?5) ELSE suspended_at END,
                         updated_at = ?5
                     WHERE id = ?1",
                    params![
                        candidate.contract.id,
                        next_status,
                        candidate.next_trial_seconds,
                        candidate.observed_state,
                        now_text,
                        i64::from(credit_revoked),
                    ],
                )
                .map_err(map_db("advance market billing contract"))?;
            }
            if credit_revoked {
                tx.execute(
                    "UPDATE market_credit_accounts
                     SET balance_units = ?2, status = 'active',
                         version = version + 1, updated_at = ?3 WHERE id = ?1",
                    params![account.id, next_balance, now_text],
                )
                .map_err(map_db("stop market account after credit revocation"))?;
            } else if credit_limit_reached {
                tx.execute(
                    "UPDATE market_credit_accounts
                     SET balance_units = ?2, status = 'settlement_due',
                         version = version + 1, updated_at = ?3 WHERE id = ?1",
                    params![account.id, next_balance, now_text],
                )
                .map_err(map_db("mark market credit limit reached"))?;
                let settled_account = AccountRow {
                    balance_units: next_balance,
                    ..account
                };
                open_invoice_tx(&tx, &settled_account, now, "credit_limit_reached", None)?;
            } else {
                let utilization_bps = limit_units.map(|limit_units| {
                    i64::try_from(i128::from(next_balance) * 10_000_i128 / i128::from(limit_units))
                        .unwrap_or(10_000)
                });
                let status = if utilization_bps.is_some_and(|value| value >= NEAR_CREDIT_LIMIT_BPS)
                {
                    ACCOUNT_NEAR_CREDIT_LIMIT
                } else {
                    ACCOUNT_ACTIVE
                };
                tx.execute(
                    "UPDATE market_credit_accounts
                     SET balance_units = ?2, status = ?3,
                         version = version + 1, updated_at = ?4 WHERE id = ?1",
                    params![account.id, next_balance, status, now_text],
                )
                .map_err(map_db("advance market credit account"))?;
                if status == ACCOUNT_NEAR_CREDIT_LIMIT
                    && account.status != ACCOUNT_NEAR_CREDIT_LIMIT
                {
                    let utilization_bps = utilization_bps.unwrap_or_default();
                    let warning_version = account.version.checked_add(1).ok_or_else(|| {
                        AppError::Internal("market account version overflowed".into())
                    })?;
                    record_event_tx(
                        &tx,
                        Some(&account.id),
                        None,
                        None,
                        None,
                        "credit_limit_warning",
                        serde_json::json!({ "utilizationBps": utilization_bps }),
                        &format!("credit-limit-warning:{}:{warning_version}", account.id),
                        &now_text,
                    )?;
                }
            }
        }
        tx.execute(
            "UPDATE market_service_contracts
             SET status = CASE WHEN status IN ('trial', 'active')
                     THEN 'billing_suspended' ELSE status END,
                 desired_control_state = 'terminated',
                 control_error = 'supplier_credit_revoked',
                 suspended_at = COALESCE(suspended_at, ?1), updated_at = ?1
             WHERE account_id IN (
                 SELECT id FROM market_credit_accounts WHERE credit_kind = 'none'
             ) AND status != 'terminated'",
            params![now_text],
        )
        .map_err(map_db("terminate services with revoked market credit"))?;
        open_final_invoices_tx(&tx, now)?;
        mark_overdue_invoices_tx(&tx, &now_text)?;
        let actions = pending_control_actions_tx(&tx)?;
        tx.commit()
            .map_err(map_db("commit market billing reconciliation"))?;
        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(user_id: &str, email: &str) -> AuthSession {
        let now = Utc::now();
        AuthSession {
            session_id: format!("session-{user_id}"),
            user_id: user_id.to_string(),
            email: email.to_string(),
            installation_id: format!("browser-{user_id}"),
            access_token_hash: format!("access-{user_id}"),
            refresh_token_hash: format!("refresh-{user_id}"),
            access_expires_at: now + Duration::hours(1),
            refresh_expires_at: now + Duration::days(30),
            created_at: now,
            last_used_at: now,
        }
    }

    async fn configure_supplier(store: &AppStore, supplier: &AuthSession, credit_limit_minor: i64) {
        store
            .client_market_update_payment_profile(
                supplier,
                &[PaymentMethod {
                    kind: "custom".into(),
                    account: None,
                    qr_image_url: None,
                    asset_url: None,
                    token: None,
                    chain: None,
                    address: None,
                    instructions: Some("Test payment instructions".into()),
                }],
                None,
            )
            .await
            .expect("configure supplier payment profile");
        store
            .market_billing_update_supplier_profile(supplier, "USD", 24)
            .await
            .expect("configure supplier billing profile");
        let now = Utc::now().to_rfc3339();
        let conn = store.conn.lock().await;
        crate::market_access::configure_open_test_policy(
            &conn,
            supplier,
            "USD",
            credit_limit_minor,
            &now,
        );
    }

    async fn add_client_contract(
        store: &AppStore,
        buyer: &AuthSession,
        supplier: &AuthSession,
        installation_id: &str,
        daily_rate_minor: i64,
        now: DateTime<Utc>,
    ) -> String {
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO client_market_subscriptions (
                    installation_id, host_id, provider_id, host_owner_email,
                    client_user_id, client_owner_email, status, daily_rate_minor,
                    currency, offer_revision, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7,
                           'USD', 1, ?8, ?8)",
                params![
                    installation_id,
                    format!("host-{installation_id}"),
                    supplier.user_id,
                    supplier.email,
                    buyer.user_id,
                    buyer.email,
                    daily_rate_minor,
                    now.to_rfc3339(),
                ],
            )
            .expect("insert active Client subscription");
        }
        store
            .market_billing_activate_contract(
                ActivateContractInput {
                    product_kind: "client_host",
                    product_ref: installation_id,
                    service_ref: installation_id,
                    service_label: installation_id,
                    buyer_user_id: &buyer.user_id,
                    buyer_email: &buyer.email,
                    supplier_user_id: &supplier.user_id,
                    supplier_email: &supplier.email,
                    currency: "USD",
                    daily_rate_minor,
                    offer_revision: 1,
                    replacement_of: None,
                },
                now,
            )
            .await
            .expect("activate Client billing contract")
    }

    async fn record_client_health(
        store: &AppStore,
        installation_id: &str,
        now: DateTime<Utc>,
        status: &str,
    ) {
        let conn = store.conn.lock().await;
        conn.execute(
            "INSERT INTO installation_health_checks (
                installation_id, checked_at, is_healthy, status, reason, router_epoch
             ) VALUES (?1, ?2, ?3, ?4, 'test_observation', 'test')",
            params![
                installation_id,
                now.timestamp(),
                i64::from(status == "healthy"),
                status,
            ],
        )
        .expect("insert Client health observation");
    }

    async fn force_contract_out_of_trial(store: &AppStore, contract_id: &str, now: DateTime<Utc>) {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE market_service_contracts
             SET status = 'active', trial_seconds_remaining = 0,
                 last_evaluated_at = ?2, updated_at = ?2 WHERE id = ?1",
            params![contract_id, now.to_rfc3339()],
        )
        .expect("finish test contract trial");
    }

    fn insert_share_billing_chat_fixture(
        conn: &Connection,
        now: DateTime<Utc>,
    ) -> (&'static str, &'static str) {
        let installation_id = "billing-chat-installation";
        let subscription_id = "billing-chat-share-subscription";
        let now = now.to_rfc3339();
        for (id, email) in [
            ("billing-chat-owner", "owner@example.com"),
            ("billing-chat-renter", "renter@example.com"),
            ("billing-chat-other-renter", "other@example.com"),
        ] {
            conn.execute(
                "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
                 VALUES (?1, ?2, 'active', ?3, ?3)",
                params![id, email, now],
            )
            .expect("insert billing chat user");
        }
        conn.execute(
            "INSERT INTO installations (
                id, public_key, platform, app_version, owner_email, owner_verified_at,
                created_at, last_seen_at
             ) VALUES (?1, 'test-public-key', 'linux', 'test', 'owner@example.com', ?2, ?2, ?2)",
            params![installation_id, now],
        )
        .expect("insert billing chat installation");
        conn.execute(
            "INSERT INTO share_market_listings (
                id, share_id, installation_id, owner_user_id, owner_email,
                status, created_at, updated_at
             ) VALUES ('billing-chat-listing', 'billing-chat-share', ?1,
                       'billing-chat-owner', 'owner@example.com', 'active', ?2, ?2)",
            params![installation_id, now],
        )
        .expect("insert Share billing listing");
        conn.execute(
            "INSERT INTO share_market_seats (
                id, listing_id, position, status, token_period_json, daily_rate_minor,
                currency, offer_revision, current_subscription_id, created_at, updated_at
             ) VALUES ('billing-chat-seat', 'billing-chat-listing', 1, 'occupied', '{}',
                       100, 'USD', 1, ?1, ?2, ?2)",
            params![subscription_id, now],
        )
        .expect("insert Share billing seat");
        conn.execute(
            "INSERT INTO share_market_subscriptions (
                id, seat_id, listing_id, share_id, installation_id, entitlement_id,
                owner_user_id, owner_email, renter_user_id, renter_email, status,
                token_period_json, daily_rate_minor, currency,
                offer_revision, created_at, updated_at
             ) VALUES (?1, 'billing-chat-seat', 'billing-chat-listing', 'billing-chat-share',
                       ?2, 'billing-chat-entitlement', 'billing-chat-owner', 'owner@example.com',
                       'billing-chat-renter', 'renter@example.com', 'active_postpaid', '{}',
                       100, 'USD', 1, ?3, ?3)",
            params![subscription_id, installation_id, now],
        )
        .expect("insert Share billing subscription");
        (subscription_id, installation_id)
    }

    #[test]
    fn billing_chat_event_mapping_is_explicit() {
        for (billing_event, chat_event) in [
            ("credit_limit_reached", "payment_due"),
            ("voluntary_settlement_requested", "payment_due"),
            ("supplier_settlement_requested", "payment_due"),
            ("account_closure_requested", "billing_account_closing"),
            ("credit_account_closed", "billing_account_closing"),
            ("final_service_ended", "payment_due"),
            ("payment_declared", "payment_declared"),
            ("payment_received", "billing_payment_confirmed"),
            ("payment_rejected", "billing_payment_rejected"),
            ("invoice_overdue", "billing_payment_overdue"),
            ("invoice_disputed", "billing_invoice_disputed"),
            ("invoice_dispute_resolved", "billing_dispute_resolved"),
            ("invoice_voided", "billing_invoice_voided"),
            ("credit_limit_warning", "billing_credit_limit_warning"),
        ] {
            assert_eq!(
                billing_client_chat_event_type(billing_event),
                Some(chat_event)
            );
        }
        assert_eq!(
            billing_client_chat_event_type("service_contract_activated"),
            None
        );
    }

    #[tokio::test]
    async fn invoice_events_share_one_client_chat_and_publish_full_billing_context() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let now = Utc::now();
        let (subscription_id, installation_id) = {
            let mut conn = store.conn.lock().await;
            let fixture = insert_share_billing_chat_fixture(&conn, now);
            conn.execute(
                "INSERT INTO market_credit_accounts (
                    id, buyer_user_id, buyer_email, supplier_user_id, supplier_email,
                    currency, status, balance_units, credit_kind, credit_limit_minor,
                    credit_source, credit_revision, created_at, updated_at
                 ) VALUES ('billing-chat-account', 'billing-chat-renter', 'renter@example.com',
                           'billing-chat-owner', 'owner@example.com', 'USD', 'settlement_due',
                           1234, 'limited', 5000, 'counterparty', 1, ?1, ?1)",
                params![now.to_rfc3339()],
            )
            .expect("insert billing chat account");
            let invoice_id = "billing-chat-invoice";
            conn.execute(
                "INSERT INTO market_invoices (
                    id, account_id, sequence, amount_minor, amount_units, currency,
                    payment_methods_json, payment_contacts_json, payment_profile_updated_at,
                    status, due_at, deadline_at, opened_at
                 ) VALUES (?1, 'billing-chat-account', 1, 1234, 1234, 'USD',
                           '[{\"kind\":\"custom\",\"instructions\":\"Wire to account 001\"}]',
                           '[{\"channel\":\"telegram\",\"handle\":\"@provider\"}]',
                           ?2, 'open', ?2, ?3, ?2)",
                params![
                    invoice_id,
                    now.to_rfc3339(),
                    (now + Duration::hours(24)).to_rfc3339()
                ],
            )
            .expect("insert combined billing invoice");
            for (line_id, product_kind, product_ref) in [
                ("billing-chat-share-line", "share", fixture.0),
                (
                    "billing-chat-client-line",
                    "client_host",
                    "billing-chat-client-subscription",
                ),
            ] {
                conn.execute(
                    "INSERT INTO market_invoice_lines (
                        id, invoice_id, contract_id, product_kind, product_ref,
                        service_ref, service_label, daily_rate_minor, billable_seconds,
                        amount_minor, amount_units, service_started_at, service_ended_at,
                        evidence_json, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 100, 60,
                               617, 617, ?8, ?8, '{}', ?8)",
                    params![
                        line_id,
                        invoice_id,
                        format!("contract-{line_id}"),
                        product_kind,
                        product_ref,
                        fixture.1,
                        format!("Service {line_id}"),
                        now.to_rfc3339(),
                    ],
                )
                .expect("insert combined invoice line");
            }
            conn.execute(
                "INSERT INTO market_payment_declarations (
                    id, invoice_id, buyer_user_id, status, payment_method_kind,
                    payment_reference, note, evidence_url, declared_at
                 ) VALUES ('billing-chat-declaration', ?1, 'billing-chat-renter', 'declared',
                           'wire', 'wire-reference-001', 'Paid in full',
                           'https://evidence.example.com/receipt/001', ?2)",
                params![invoice_id, now.to_rfc3339()],
            )
            .expect("insert payment declaration");
            let tx = conn
                .transaction()
                .expect("begin billing chat event transaction");
            for _ in 0..2 {
                record_event_tx(
                    &tx,
                    Some("billing-chat-account"),
                    None,
                    Some(invoice_id),
                    Some("billing-chat-renter"),
                    "payment_declared",
                    serde_json::json!({ "declarationId": "billing-chat-declaration" }),
                    "billing-chat-payment-declared",
                    &now.to_rfc3339(),
                )
                .expect("record idempotent payment declaration event");
            }
            record_event_tx(
                &tx,
                Some("billing-chat-account"),
                None,
                Some(invoice_id),
                Some("billing-chat-owner"),
                "payment_received",
                serde_json::json!({ "declarationId": "billing-chat-declaration" }),
                "billing-chat-payment-received",
                &now.to_rfc3339(),
            )
            .expect("record payment confirmation event");
            tx.commit().expect("commit billing chat events");
            fixture
        };

        {
            let conn = store.conn.lock().await;
            let event_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM market_billing_events
                     WHERE invoice_id = 'billing-chat-invoice'",
                    [],
                    |row| row.get(0),
                )
                .expect("count billing events");
            let outbox_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM client_chat_system_outbox
                     WHERE source_kind = 'market_billing'
                       AND installation_id = ?1",
                    params![installation_id],
                    |row| row.get(0),
                )
                .expect("count billing chat outbox");
            assert_eq!((event_count, outbox_count), (2, 2));
            let payload: String = conn
                .query_row(
                    "SELECT payload_json FROM client_chat_system_outbox
                     WHERE event_type = 'payment_declared'",
                    [],
                    |row| row.get(0),
                )
                .expect("read enriched billing payload");
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("parse billing payload");
            assert_eq!(payload["invoiceId"], "billing-chat-invoice");
            assert_eq!(payload["amountMinor"], 1234);
            assert_eq!(payload["amountUsdMinor"], 1234);
            assert_eq!(payload["amountCnyMinor"], 8638);
            assert_eq!(payload["currency"], "USD");
            assert_eq!(payload["billingEventType"], "payment_declared");
            assert_eq!(payload["buyerEmail"], "renter@example.com");
            assert_eq!(payload["supplierEmail"], "owner@example.com");
            assert_eq!(
                payload["paymentDeclaration"]["paymentReference"],
                "wire-reference-001"
            );
            assert_eq!(
                payload["paymentDeclaration"]["evidenceUrl"],
                "https://evidence.example.com/receipt/001"
            );
            assert_eq!(payload["services"].as_array().map(Vec::len), Some(2));
        }

        assert_eq!(
            store
                .process_client_chat_system_outbox(100)
                .await
                .expect("materialize billing chat events"),
            2
        );
        let room = store
            .get_client_chat_room_by_installation(installation_id, Some("billing-chat-renter"))
            .await
            .expect("read Client chat room");
        let owner = store
            .list_chat_messages(&room.id, Some("billing-chat-owner"), None, None, 50)
            .await
            .expect("owner reads billing messages");
        let renter = store
            .list_chat_messages(&room.id, Some("billing-chat-renter"), None, None, 50)
            .await
            .expect("renter reads billing messages");
        assert_eq!(owner.messages.len(), 2);
        assert_eq!(renter.messages.len(), 2);
        assert!(owner.messages.iter().all(|message| {
            matches!(
                message.event_type.as_deref(),
                Some("payment_declared" | "billing_payment_confirmed")
            )
        }));
        let outsider_rooms = store
            .list_visited_chat_rooms("billing-chat-other-renter")
            .await
            .expect("list unrelated renter rooms");
        assert!(outsider_rooms.rooms.is_empty());
        let conn = store.conn.lock().await;
        let chat_email_events: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_email_events", [], |row| {
                row.get(0)
            })
            .expect("count human chat email events");
        assert_eq!(chat_email_events, 0);
        let subscription_installation: String = conn
            .query_row(
                "SELECT installation_id FROM share_market_subscriptions WHERE id = ?1",
                params![subscription_id],
                |row| row.get(0),
            )
            .expect("read subscription installation");
        assert_eq!(subscription_installation, installation_id);
    }

    #[tokio::test]
    async fn trial_consumes_only_healthy_service_time_and_unknown_time_is_free() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-trial", "buyer-trial@example.com");
        let supplier = session("supplier-trial", "supplier-trial@example.com");
        configure_supplier(&store, &supplier, 10_000).await;
        let started_at = Utc::now();
        let contract_id =
            add_client_contract(&store, &buyer, &supplier, "client-trial", 600, started_at).await;

        store
            .market_billing_reconcile(started_at + Duration::seconds(5))
            .await
            .expect("reconcile unknown service time");
        {
            let conn = store.conn.lock().await;
            let state: (i64, String, i64) = conn
                .query_row(
                    "SELECT contract.trial_seconds_remaining, contract.health_state,
                            account.balance_units
                     FROM market_service_contracts contract
                     JOIN market_credit_accounts account ON account.id = contract.account_id
                     WHERE contract.id = ?1",
                    params![contract_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("read unknown-time billing state");
            assert_eq!(state, (TRIAL_SECONDS, "unknown".into(), 0));
            conn.execute(
                "UPDATE market_service_contracts SET trial_seconds_remaining = 3 WHERE id = ?1",
                params![contract_id],
            )
            .expect("shorten remaining trial for boundary test");
        }

        let healthy_at = started_at + Duration::seconds(10);
        record_client_health(&store, "client-trial", healthy_at, "healthy").await;
        store
            .market_billing_reconcile(healthy_at)
            .await
            .expect("reconcile healthy service time");
        let conn = store.conn.lock().await;
        let state: (i64, String, String, i64, i64) = conn
            .query_row(
                "SELECT contract.trial_seconds_remaining, contract.status,
                        contract.health_state, account.balance_units,
                        COALESCE(SUM(interval.billable_seconds), 0)
                 FROM market_service_contracts contract
                 JOIN market_credit_accounts account ON account.id = contract.account_id
                 LEFT JOIN market_service_intervals interval ON interval.contract_id = contract.id
                 WHERE contract.id = ?1 GROUP BY contract.id",
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
            .expect("read healthy-time billing state");
        assert_eq!(state, (0, "active".into(), "healthy".into(), 1_200, 2));
    }

    #[tokio::test]
    async fn each_service_contract_receives_its_own_healthy_time_trial() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-parallel-trial", "buyer-parallel-trial@example.com");
        let supplier = session(
            "supplier-parallel-trial",
            "supplier-parallel-trial@example.com",
        );
        configure_supplier(&store, &supplier, 10_000).await;
        let started_at = Utc::now();
        let first = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-parallel-trial-a",
            600,
            started_at,
        )
        .await;
        let second = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-parallel-trial-b",
            600,
            started_at,
        )
        .await;

        let conn = store.conn.lock().await;
        for contract_id in [first, second] {
            let state: (String, i64) = conn
                .query_row(
                    "SELECT status, trial_seconds_remaining
                     FROM market_service_contracts WHERE id = ?1",
                    params![contract_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("read independent service trial");
            assert_eq!(state, (CONTRACT_TRIAL.into(), TRIAL_SECONDS));
        }
    }

    #[tokio::test]
    async fn final_service_termination_opens_the_remaining_balance() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-final", "buyer-final@example.com");
        let supplier = session("supplier-final", "supplier-final@example.com");
        configure_supplier(&store, &supplier, 10_000).await;
        let started_at = Utc::now();
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-final",
            MONEY_UNITS_PER_MINOR,
            started_at,
        )
        .await;
        force_contract_out_of_trial(&store, &contract_id, started_at).await;
        let observed_at = started_at + Duration::seconds(1);
        record_client_health(&store, "client-final", observed_at, "healthy").await;
        store
            .market_billing_reconcile(observed_at)
            .await
            .expect("accrue final service balance");
        store
            .market_billing_terminate_contract(
                "client_host",
                "client-final",
                "buyer_release",
                observed_at,
            )
            .await
            .expect("terminate final service contract");

        let actions = store
            .market_billing_reconcile(observed_at + Duration::seconds(1))
            .await
            .expect("open final service invoice");
        assert!(actions.is_empty());
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load final service invoice");
        assert_eq!(dashboard.accounts[0].status, ACCOUNT_SETTLEMENT_DUE);
        assert!(dashboard.accounts[0].services.is_empty());
        let invoice = dashboard.accounts[0]
            .open_invoice
            .as_ref()
            .expect("remaining balance invoice");
        assert_eq!(invoice.amount_minor, 1);
        assert_eq!(invoice.amount_usd_minor, 1);
        assert_eq!(invoice.amount_cny_minor, 7);
        assert_eq!(invoice.lines.len(), 1);
        assert_eq!(invoice.lines[0].product_ref, "client-final");
    }

    #[tokio::test]
    async fn share_control_is_applied_only_after_the_entitlement_state_converges() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let now = Utc::now();
        let supplier = session("billing-chat-owner", "owner@example.com");
        let buyer = session("billing-chat-renter", "renter@example.com");
        configure_supplier(&store, &supplier, 10_000).await;
        let (subscription_id, _) = {
            let conn = store.conn.lock().await;
            insert_share_billing_chat_fixture(&conn, now)
        };
        let contract_id = store
            .market_billing_activate_contract(
                ActivateContractInput {
                    product_kind: "share",
                    product_ref: subscription_id,
                    service_ref: "billing-chat-share",
                    service_label: "Billing Chat Share",
                    buyer_user_id: &buyer.user_id,
                    buyer_email: &buyer.email,
                    supplier_user_id: &supplier.user_id,
                    supplier_email: &supplier.email,
                    currency: "USD",
                    daily_rate_minor: 100,
                    offer_revision: 1,
                    replacement_of: None,
                },
                now,
            )
            .await
            .expect("activate Share billing contract");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_service_contracts
                 SET status = 'billing_suspended', desired_control_state = 'suspended',
                     applied_control_state = 'active' WHERE id = ?1",
                params![contract_id],
            )
            .expect("request Share billing suspension");
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET status = 'billing_suspend_pending' WHERE id = ?1",
                params![subscription_id],
            )
            .expect("mark Share suspension pending");
        }

        store
            .market_billing_mark_control_applied(&contract_id, &BillingActionKind::Suspend)
            .await
            .expect("defer pending Share suspension");
        {
            let conn = store.conn.lock().await;
            let applied: String = conn
                .query_row(
                    "SELECT applied_control_state FROM market_service_contracts WHERE id = ?1",
                    params![contract_id],
                    |row| row.get(0),
                )
                .expect("read deferred Share suspension");
            assert_eq!(applied, "active");
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET status = 'billing_suspended' WHERE id = ?1",
                params![subscription_id],
            )
            .expect("confirm Share suspension");
        }
        store
            .market_billing_mark_control_applied(&contract_id, &BillingActionKind::Suspend)
            .await
            .expect("complete Share suspension");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_service_contracts
                 SET status = 'active', desired_control_state = 'active'
                 WHERE id = ?1",
                params![contract_id],
            )
            .expect("request Share billing resume");
            conn.execute(
                "UPDATE share_market_subscriptions
                 SET status = 'billing_resume_pending' WHERE id = ?1",
                params![subscription_id],
            )
            .expect("mark Share resume pending");
        }

        store
            .market_billing_mark_control_applied(&contract_id, &BillingActionKind::Resume)
            .await
            .expect("defer pending Share resume");
        {
            let conn = store.conn.lock().await;
            let applied: String = conn
                .query_row(
                    "SELECT applied_control_state FROM market_service_contracts WHERE id = ?1",
                    params![contract_id],
                    |row| row.get(0),
                )
                .expect("read deferred Share resume");
            assert_eq!(applied, "suspended");
            conn.execute(
                "UPDATE share_market_subscriptions SET status = 'active_postpaid' WHERE id = ?1",
                params![subscription_id],
            )
            .expect("confirm Share resume");
        }
        store
            .market_billing_mark_control_applied(&contract_id, &BillingActionKind::Resume)
            .await
            .expect("complete Share resume");
        let conn = store.conn.lock().await;
        let applied: String = conn
            .query_row(
                "SELECT applied_control_state FROM market_service_contracts WHERE id = ?1",
                params![contract_id],
                |row| row.get(0),
            )
            .expect("read completed Share resume");
        assert_eq!(applied, "active");
    }

    #[tokio::test]
    async fn stale_client_resume_cannot_reenable_a_releasing_tunnel() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let now = Utc::now();
        let buyer = session("buyer-stale-resume", "buyer@example.com");
        let supplier = session("supplier-stale-resume", "supplier@example.com");
        configure_supplier(&store, &supplier, 10_000).await;
        add_client_contract(&store, &buyer, &supplier, "client-stale-resume", 100, now).await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO installation_client_tunnels (
                    installation_id, owner_email, subdomain, enabled, created_at, updated_at
                 ) VALUES ('client-stale-resume', ?1, 'stale-resume', 0, ?2, ?2)",
                params![buyer.email, now.to_rfc3339()],
            )
            .expect("insert suspended Client tunnel");
            conn.execute(
                "UPDATE client_market_subscriptions SET status = 'releasing'
                 WHERE installation_id = 'client-stale-resume'",
                [],
            )
            .expect("begin Client release");
        }

        let subdomain = store
            .client_market_set_billing_suspended("client-stale-resume", false)
            .await
            .expect("ignore stale Client resume");
        assert!(subdomain.is_none());
        let conn = store.conn.lock().await;
        let enabled: i64 = conn
            .query_row(
                "SELECT enabled FROM installation_client_tunnels
                 WHERE installation_id = 'client-stale-resume'",
                [],
                |row| row.get(0),
            )
            .expect("read releasing Client tunnel");
        assert_eq!(enabled, 0);
    }

    #[tokio::test]
    async fn supplier_can_close_an_unsettled_account_without_post_payment_resume() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let now = Utc::now();
        let buyer = session("buyer-close-due", "buyer@example.com");
        let supplier = session("supplier-close-due", "supplier@example.com");
        configure_supplier(&store, &supplier, 1).await;
        let contract_id =
            add_client_contract(&store, &buyer, &supplier, "client-close-due", 86_400, now).await;
        force_contract_out_of_trial(&store, &contract_id, now).await;
        record_client_health(
            &store,
            "client-close-due",
            now + Duration::seconds(2),
            "healthy",
        )
        .await;
        store
            .market_billing_reconcile(now + Duration::seconds(2))
            .await
            .expect("open threshold invoice before supplier closure");
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load unsettled supplier account");
        let account = dashboard.accounts.first().expect("supplier account");
        let account_id = account.id.clone();
        let invoice_id = account
            .open_invoice
            .as_ref()
            .expect("unsettled invoice")
            .id
            .clone();

        let actions = store
            .market_billing_open_account_invoice(&supplier.user_id, &account_id, true, false)
            .await
            .expect("latch supplier closure on unsettled account");
        assert!(
            actions
                .iter()
                .all(|action| action.kind == BillingActionKind::Terminate)
        );
        let closing = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load closing supplier account");
        assert!(closing.accounts[0].close_requested);
        assert!(!closing.accounts[0].can_close);

        store
            .market_billing_declare_payment(&buyer, &invoice_id, None, None, None, None)
            .await
            .expect("declare final invoice payment");
        let actions = store
            .market_billing_confirm_payment(&supplier, &invoice_id)
            .await
            .expect("confirm final invoice payment");
        assert!(
            actions
                .iter()
                .all(|action| action.kind == BillingActionKind::Terminate)
        );
        let closed = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load permanently closed account");
        assert_eq!(closed.accounts[0].status, ACCOUNT_CLOSED);
        assert!(closed.accounts[0].close_requested);
    }

    #[tokio::test]
    async fn fast_payment_emits_resume_before_the_suspend_action_finishes() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let now = Utc::now();
        let buyer = session("buyer-fast-payment", "buyer@example.com");
        let supplier = session("supplier-fast-payment", "supplier@example.com");
        configure_supplier(&store, &supplier, 1).await;
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-fast-payment",
            86_400,
            now,
        )
        .await;
        force_contract_out_of_trial(&store, &contract_id, now).await;
        record_client_health(
            &store,
            "client-fast-payment",
            now + Duration::seconds(2),
            "healthy",
        )
        .await;
        let suspend_actions = store
            .market_billing_reconcile(now + Duration::seconds(2))
            .await
            .expect("open fast-payment invoice");
        assert_eq!(suspend_actions.len(), 1);
        let invoice_id = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load fast-payment invoice")
            .accounts[0]
            .open_invoice
            .as_ref()
            .expect("fast-payment invoice")
            .id
            .clone();

        store
            .market_billing_declare_payment(&buyer, &invoice_id, None, None, None, None)
            .await
            .expect("declare fast payment");
        let resume_actions = store
            .market_billing_confirm_payment(&supplier, &invoice_id)
            .await
            .expect("confirm fast payment");
        assert_eq!(resume_actions.len(), 1);
        assert_eq!(resume_actions[0].kind, BillingActionKind::Resume);
        assert!(
            !store
                .market_billing_control_action_is_current(
                    &contract_id,
                    &BillingActionKind::Suspend,
                )
                .await
                .expect("reject stale suspension action")
        );
        assert!(
            store
                .market_billing_control_action_is_current(&contract_id, &BillingActionKind::Resume,)
                .await
                .expect("accept compensating resume action")
        );
        let conn = store.conn.lock().await;
        let control: (String, String) = conn
            .query_row(
                "SELECT desired_control_state, applied_control_state
                 FROM market_service_contracts WHERE id = ?1",
                params![contract_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read compensating resume state");
        assert_eq!(control, ("active".into(), "suspended".into()));
    }

    #[tokio::test]
    async fn invoice_keeps_the_payment_profile_snapshot_used_at_issue_time() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let now = Utc::now();
        let buyer = session("buyer-payment-snapshot", "buyer@example.com");
        let supplier = session("supplier-payment-snapshot", "supplier@example.com");
        configure_supplier(&store, &supplier, 1).await;
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-payment-snapshot",
            86_400,
            now,
        )
        .await;
        force_contract_out_of_trial(&store, &contract_id, now).await;
        record_client_health(
            &store,
            "client-payment-snapshot",
            now + Duration::seconds(2),
            "healthy",
        )
        .await;
        store
            .market_billing_reconcile(now + Duration::seconds(2))
            .await
            .expect("open payment snapshot invoice");
        let issued = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load issued payment snapshot");
        let account_json =
            serde_json::to_value(&issued.accounts[0]).expect("serialize redacted credit account");
        for field in ["paymentMethods", "contacts", "paymentProfileUpdatedAt"] {
            assert!(
                account_json.get(field).is_none(),
                "credit account must not expose live {field}"
            );
        }
        let invoice = issued.accounts[0]
            .open_invoice
            .as_ref()
            .expect("payment snapshot invoice");
        assert_eq!(
            invoice.payment_methods[0].instructions.as_deref(),
            Some("Test payment instructions")
        );
        let snapshot_updated_at = invoice.payment_profile_updated_at.clone();

        store
            .client_market_update_payment_profile(
                &supplier,
                &[PaymentMethod {
                    kind: "custom".into(),
                    account: None,
                    qr_image_url: None,
                    asset_url: None,
                    token: None,
                    chain: None,
                    address: None,
                    instructions: Some("Replacement payment instructions".into()),
                }],
                None,
            )
            .await
            .expect("replace live supplier payment profile");
        let refreshed = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("reload immutable payment snapshot");
        let invoice = refreshed.accounts[0]
            .open_invoice
            .as_ref()
            .expect("reloaded payment snapshot invoice");
        assert_eq!(
            invoice.payment_methods[0].instructions.as_deref(),
            Some("Test payment instructions")
        );
        assert_eq!(invoice.payment_profile_updated_at, snapshot_updated_at);
    }

    #[tokio::test]
    async fn credit_limit_invoice_aggregates_products_and_confirmation_resumes_all_services() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-combined", "buyer-combined@example.com");
        let supplier = session("supplier-combined", "supplier-combined@example.com");
        configure_supplier(&store, &supplier, 1).await;
        let started_at = Utc::now();
        let first = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-combined-a",
            60_000,
            started_at,
        )
        .await;
        let second = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-combined-b",
            60_000,
            started_at,
        )
        .await;
        force_contract_out_of_trial(&store, &first, started_at).await;
        force_contract_out_of_trial(&store, &second, started_at).await;
        let observed_at = started_at + Duration::seconds(1);
        record_client_health(&store, "client-combined-a", observed_at, "healthy").await;
        record_client_health(&store, "client-combined-b", observed_at, "healthy").await;

        let suspend_actions = store
            .market_billing_reconcile(observed_at)
            .await
            .expect("reach combined credit limit");
        assert_eq!(suspend_actions.len(), 2);
        assert!(
            suspend_actions
                .iter()
                .all(|action| action.kind == BillingActionKind::Suspend)
        );
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load buyer billing dashboard");
        let account = dashboard
            .accounts
            .first()
            .expect("combined supplier account");
        assert_eq!(account.balance_minor, 2);
        assert_eq!(account.status, ACCOUNT_SETTLEMENT_DUE);
        let invoice = account.open_invoice.as_ref().expect("combined invoice");
        assert_eq!(invoice.amount_minor, 2);
        assert_eq!(invoice.amount_usd_minor, 2);
        assert_eq!(invoice.amount_cny_minor, 14);
        assert_eq!(invoice.lines.len(), 2);
        assert!(invoice.lines.iter().all(|line| {
            line.amount_usd_minor == line.amount_minor
                && line.amount_cny_minor == line.amount_minor * USD_CNY_RATE
        }));
        assert_eq!(
            invoice
                .lines
                .iter()
                .map(|line| line.amount_minor)
                .sum::<i64>(),
            2
        );
        assert!(invoice.lines.iter().all(|line| line.billable_seconds == 1));
        let mut product_refs = invoice
            .lines
            .iter()
            .map(|line| line.product_ref.as_str())
            .collect::<Vec<_>>();
        product_refs.sort_unstable();
        assert_eq!(product_refs, vec!["client-combined-a", "client-combined-b"]);
        let invoice_id = invoice.id.clone();
        let overdue_at = parse_time(&invoice.deadline_at).expect("parse combined invoice deadline")
            + Duration::seconds(1);
        for action in &suspend_actions {
            store
                .market_billing_mark_control_applied(&action.contract_id, &action.kind)
                .await
                .expect("mark suspension applied");
        }
        store
            .market_billing_declare_payment(
                &buyer,
                &invoice_id,
                Some("bank".into()),
                Some("test-reference".into()),
                Some("Authorization: Bearer fake-billing-secret".into()),
                None,
            )
            .await
            .expect("declare combined payment");
        {
            let conn = store.conn.lock().await;
            let stored_note: String = conn
                .query_row(
                    "SELECT note FROM market_payment_declarations
                     WHERE invoice_id = ?1 AND status = 'declared'",
                    params![invoice_id],
                    |row| row.get(0),
                )
                .expect("read sanitized payment note");
            let event_detail: String = conn
                .query_row(
                    "SELECT detail_json FROM market_billing_events
                     WHERE invoice_id = ?1 AND event_type = 'payment_declared'",
                    params![invoice_id],
                    |row| row.get(0),
                )
                .expect("read sanitized payment event");
            let outbox_payload: String = conn
                .query_row(
                    "SELECT payload_json FROM client_chat_system_outbox
                     WHERE source_kind = 'market_billing'
                       AND event_type = 'payment_declared' LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .expect("read sanitized payment outbox");
            assert_eq!(stored_note, "[credential omitted]");
            for stored in [event_detail, outbox_payload] {
                assert!(!stored.contains("fake-billing-secret"));
            }
        }
        let overdue_actions = store
            .market_billing_reconcile(overdue_at)
            .await
            .expect("restrict overdue declared payment");
        assert!(overdue_actions.is_empty());
        let error = store
            .market_billing_check_credit_allowed(
                &buyer.user_id,
                &buyer.email,
                &supplier.user_id,
                crate::market_access::PRODUCT_CLIENT_HOST,
                "USD",
            )
            .await
            .expect_err("block overdue buyer credit");
        assert_eq!(
            error.code(),
            Some(crate::market_access::ERROR_MARKET_BUYER_RESTRICTED)
        );
        let declared_dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load overdue declared payment");
        assert_eq!(declared_dashboard.restrictions.len(), 1);
        assert_eq!(
            declared_dashboard.accounts[0]
                .open_invoice
                .as_ref()
                .expect("declared combined invoice")
                .status,
            INVOICE_PAYMENT_DECLARED
        );
        let resume_actions = store
            .market_billing_confirm_payment(&supplier, &invoice_id)
            .await
            .expect("confirm combined payment");
        assert_eq!(resume_actions.len(), 2);
        assert!(
            resume_actions
                .iter()
                .all(|action| action.kind == BillingActionKind::Resume)
        );
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("reload settled billing dashboard");
        let account = dashboard
            .accounts
            .first()
            .expect("settled supplier account");
        assert_eq!(account.status, ACCOUNT_ACTIVE);
        assert_eq!(account.balance_minor, 0);
        assert!(account.open_invoice.is_none());
        assert!(dashboard.restrictions.is_empty());
        assert!(
            account
                .services
                .iter()
                .all(|service| service.status == CONTRACT_ACTIVE)
        );
        let history = store
            .market_billing_invoice_history(&buyer, &account.id, None, 20)
            .await
            .expect("load settled invoice history");
        assert_eq!(history.invoices.len(), 1);
        assert_eq!(history.invoices[0].id, invoice_id);
        assert_eq!(history.invoices[0].status, INVOICE_PAID);
        assert!(history.next_before_sequence.is_none());
        let stranger = session("billing-stranger", "billing-stranger@example.com");
        assert!(matches!(
            store
                .market_billing_invoice_history(&stranger, &account.id, None, 20)
                .await,
            Err(AppError::Forbidden(_))
        ));
        assert!(matches!(
            store
                .market_billing_invoice_history(&buyer, "missing-account", None, 20)
                .await,
            Err(AppError::NotFound(_))
        ));
        let conn = store.conn.lock().await;
        let invoice_status: String = conn
            .query_row(
                "SELECT status FROM market_invoices WHERE id = ?1",
                params![invoice_id],
                |row| row.get(0),
            )
            .expect("read paid combined invoice");
        assert_eq!(invoice_status, INVOICE_PAID);
    }

    #[tokio::test]
    async fn near_credit_limit_warning_is_emitted_again_after_a_settled_cycle() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-warning-cycle", "buyer-warning-cycle@example.com");
        let supplier = session(
            "supplier-warning-cycle",
            "supplier-warning-cycle@example.com",
        );
        configure_supplier(&store, &supplier, 100).await;
        let first_started_at = Utc::now();
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-warning-cycle",
            80 * MONEY_UNITS_PER_MINOR,
            first_started_at,
        )
        .await;
        force_contract_out_of_trial(&store, &contract_id, first_started_at).await;
        let first_observed_at = first_started_at + Duration::seconds(1);
        record_client_health(&store, "client-warning-cycle", first_observed_at, "healthy").await;
        store
            .market_billing_reconcile(first_observed_at)
            .await
            .expect("enter first near-threshold cycle");

        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load first warning cycle");
        let account_id = dashboard.accounts[0].id.clone();
        assert_eq!(dashboard.accounts[0].status, ACCOUNT_NEAR_CREDIT_LIMIT);
        let suspend_actions = store
            .market_billing_open_account_invoice(&buyer.user_id, &account_id, false, false)
            .await
            .expect("settle first warning cycle");
        for action in &suspend_actions {
            store
                .market_billing_mark_control_applied(&action.contract_id, &action.kind)
                .await
                .expect("mark first-cycle suspension applied");
        }
        let invoice_id = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load first-cycle invoice")
            .accounts[0]
            .open_invoice
            .as_ref()
            .expect("first-cycle invoice")
            .id
            .clone();
        store
            .market_billing_declare_payment(&buyer, &invoice_id, None, None, None, None)
            .await
            .expect("declare first-cycle payment");
        let resume_actions = store
            .market_billing_confirm_payment(&supplier, &invoice_id)
            .await
            .expect("confirm first-cycle payment");
        for action in &resume_actions {
            store
                .market_billing_mark_control_applied(&action.contract_id, &action.kind)
                .await
                .expect("mark first-cycle resume applied");
        }

        let second_started_at = Utc::now();
        force_contract_out_of_trial(&store, &contract_id, second_started_at).await;
        let second_observed_at = second_started_at + Duration::seconds(1);
        record_client_health(
            &store,
            "client-warning-cycle",
            second_observed_at,
            "healthy",
        )
        .await;
        store
            .market_billing_reconcile(second_observed_at)
            .await
            .expect("enter second near-threshold cycle");
        store
            .market_billing_reconcile(second_observed_at)
            .await
            .expect("repeat second-cycle reconciliation idempotently");

        let conn = store.conn.lock().await;
        let warning_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_billing_events
                 WHERE account_id = ?1 AND event_type = 'credit_limit_warning'",
                params![account_id],
                |row| row.get(0),
            )
            .expect("count warning-cycle events");
        assert_eq!(warning_count, 2);
        let distinct_keys: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT idempotency_key) FROM market_billing_events
                 WHERE account_id = ?1 AND event_type = 'credit_limit_warning'",
                params![account_id],
                |row| row.get(0),
            )
            .expect("count warning-cycle idempotency keys");
        assert_eq!(distinct_keys, 2);
        let warning_outbox_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM client_chat_system_outbox
                 WHERE source_kind = 'market_billing'
                   AND event_type = 'billing_credit_limit_warning'",
                [],
                |row| row.get(0),
            )
            .expect("count credit warning chat events");
        assert_eq!(warning_outbox_count, 2);
        let follower_ids_json: String = conn
            .query_row(
                "SELECT follower_user_ids_json FROM client_chat_system_outbox
                 WHERE source_kind = 'market_billing'
                   AND event_type = 'billing_credit_limit_warning'
                 ORDER BY created_at LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("read credit warning chat followers");
        let follower_ids: Vec<String> =
            serde_json::from_str(&follower_ids_json).expect("parse credit warning followers");
        assert!(follower_ids.contains(&buyer.user_id));
        assert!(follower_ids.contains(&supplier.user_id));
    }

    #[tokio::test]
    async fn lowered_credit_limit_blocks_new_rentals_before_reconciliation() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-lowered-credit", "buyer-lowered-credit@example.com");
        let supplier = session(
            "supplier-lowered-credit",
            "supplier-lowered-credit@example.com",
        );
        configure_supplier(&store, &supplier, 10).await;
        let started_at = Utc::now();
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-lowered-credit",
            MONEY_UNITS_PER_MINOR,
            started_at,
        )
        .await;
        force_contract_out_of_trial(&store, &contract_id, started_at).await;
        let observed_at = started_at + Duration::seconds(1);
        record_client_health(&store, "client-lowered-credit", observed_at, "healthy").await;
        store
            .market_billing_reconcile(observed_at)
            .await
            .expect("accrue below the original credit limit");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_public_credit_policies
                 SET limit_minor = 1, revision = revision + 1
                 WHERE supplier_user_id = ?1 AND currency = 'USD'",
                params![supplier.user_id],
            )
            .expect("lower effective public credit");
            conn.execute(
                "UPDATE market_credit_accounts
                 SET credit_limit_minor = 1, credit_revision = credit_revision + 1
                 WHERE buyer_user_id = ?1 AND supplier_user_id = ?2 AND currency = 'USD'",
                params![buyer.user_id, supplier.user_id],
            )
            .expect("coordinate lowered account credit");
        }

        let error = store
            .market_billing_check_credit_allowed(
                &buyer.user_id,
                &buyer.email,
                &supplier.user_id,
                crate::market_access::PRODUCT_CLIENT_HOST,
                "USD",
            )
            .await
            .expect_err("block a new rental at the lowered credit limit");
        assert_eq!(
            error.code(),
            Some(crate::market_access::ERROR_MARKET_CREDIT_LIMIT_REACHED)
        );
    }

    #[tokio::test]
    async fn revoked_credit_is_not_restored_after_invoice_payment() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-credit-revoked", "buyer-credit-revoked@example.com");
        let supplier = session(
            "supplier-credit-revoked",
            "supplier-credit-revoked@example.com",
        );
        configure_supplier(&store, &supplier, 1).await;
        let started_at = Utc::now();
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-credit-revoked",
            MONEY_UNITS_PER_MINOR,
            started_at,
        )
        .await;
        force_contract_out_of_trial(&store, &contract_id, started_at).await;
        let observed_at = started_at + Duration::seconds(1);
        record_client_health(&store, "client-credit-revoked", observed_at, "healthy").await;
        let suspend_actions = store
            .market_billing_reconcile(observed_at)
            .await
            .expect("open invoice before credit revocation");
        assert_eq!(suspend_actions.len(), 1);
        store
            .market_billing_mark_control_applied(&contract_id, &BillingActionKind::Suspend)
            .await
            .expect("apply suspension before revocation");
        let invoice_id = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load invoice before credit revocation")
            .accounts[0]
            .open_invoice
            .as_ref()
            .expect("open invoice")
            .id
            .clone();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_credit_accounts
                 SET credit_kind = 'none', credit_limit_minor = NULL,
                     credit_revision = credit_revision + 1
                 WHERE buyer_user_id = ?1 AND supplier_user_id = ?2",
                params![buyer.user_id, supplier.user_id],
            )
            .expect("revoke credit while invoice is open");
        }
        store
            .market_billing_declare_payment(&buyer, &invoice_id, None, None, None, None)
            .await
            .expect("declare payment after credit revocation");
        let actions = store
            .market_billing_confirm_payment(&supplier, &invoice_id)
            .await
            .expect("confirm payment after credit revocation");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, BillingActionKind::Terminate);
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load settled revoked account");
        assert_eq!(
            dashboard.accounts[0].credit_kind,
            crate::market_access::CREDIT_NONE
        );
        assert_eq!(
            dashboard.accounts[0].services[0].status,
            CONTRACT_BILLING_SUSPENDED
        );
    }

    #[tokio::test]
    async fn revoked_credit_is_not_restored_after_invoice_void() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-credit-voided", "buyer-credit-voided@example.com");
        let supplier = session(
            "supplier-credit-voided",
            "supplier-credit-voided@example.com",
        );
        let admin = session("admin-credit-voided", "admin-credit-voided@example.com");
        configure_supplier(&store, &supplier, 1).await;
        let started_at = Utc::now();
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-credit-voided",
            MONEY_UNITS_PER_MINOR,
            started_at,
        )
        .await;
        force_contract_out_of_trial(&store, &contract_id, started_at).await;
        let observed_at = started_at + Duration::seconds(1);
        record_client_health(&store, "client-credit-voided", observed_at, "healthy").await;
        store
            .market_billing_reconcile(observed_at)
            .await
            .expect("open invoice before credit revocation");
        store
            .market_billing_mark_control_applied(&contract_id, &BillingActionKind::Suspend)
            .await
            .expect("apply suspension before invoice void");
        let invoice_id = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load invoice before credit revocation")
            .accounts[0]
            .open_invoice
            .as_ref()
            .expect("open invoice")
            .id
            .clone();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_credit_accounts
                 SET credit_kind = 'none', credit_limit_minor = NULL,
                     credit_revision = credit_revision + 1
                 WHERE buyer_user_id = ?1 AND supplier_user_id = ?2",
                params![buyer.user_id, supplier.user_id],
            )
            .expect("revoke credit while invoice is open");
        }

        let actions = store
            .market_billing_void_invoice(&admin, &invoice_id, "supplier credit was revoked")
            .await
            .expect("void invoice after credit revocation");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, BillingActionKind::Terminate);
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load voided revoked account");
        assert_eq!(
            dashboard.accounts[0].credit_kind,
            crate::market_access::CREDIT_NONE
        );
        assert_eq!(
            dashboard.accounts[0].services[0].status,
            CONTRACT_BILLING_SUSPENDED
        );
    }

    #[tokio::test]
    async fn supplier_can_request_settlement_for_unlimited_credit() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-unlimited", "buyer-unlimited@example.com");
        let supplier = session("supplier-unlimited", "supplier-unlimited@example.com");
        configure_supplier(&store, &supplier, 10_000).await;
        let started_at = Utc::now();
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-unlimited",
            MONEY_UNITS_PER_MINOR,
            started_at,
        )
        .await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_credit_accounts
                 SET credit_kind = 'unlimited', credit_limit_minor = NULL,
                     credit_revision = credit_revision + 1",
                [],
            )
            .expect("grant unlimited account credit");
        }
        force_contract_out_of_trial(&store, &contract_id, started_at).await;
        let observed_at = started_at + Duration::seconds(1);
        record_client_health(&store, "client-unlimited", observed_at, "healthy").await;
        assert!(
            store
                .market_billing_reconcile(observed_at)
                .await
                .expect("accrue unlimited credit")
                .is_empty()
        );
        let account = store
            .market_billing_dashboard(&supplier)
            .await
            .expect("load unlimited supplier account")
            .accounts
            .into_iter()
            .next()
            .expect("unlimited account");
        assert_eq!(account.status, ACCOUNT_ACTIVE);
        assert!(account.open_invoice.is_none());
        assert!(account.balance_minor > 0);
        let actions = store
            .market_billing_open_account_invoice(&supplier.user_id, &account.id, false, true)
            .await
            .expect("supplier requests unlimited settlement");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, BillingActionKind::Suspend);
        let settled = store
            .market_billing_dashboard(&supplier)
            .await
            .expect("load supplier-requested invoice");
        assert_eq!(settled.accounts[0].status, ACCOUNT_SETTLEMENT_DUE);
        assert!(settled.accounts[0].open_invoice.is_some());
    }

    #[tokio::test]
    async fn billing_contract_rejects_self_debt() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let owner = session("self-owner", "self@example.com");
        let error = store
            .market_billing_activate_contract(
                ActivateContractInput {
                    product_kind: "client_host",
                    product_ref: "self-client",
                    service_ref: "self-client",
                    service_label: "self-client",
                    buyer_user_id: &owner.user_id,
                    buyer_email: &owner.email,
                    supplier_user_id: &owner.user_id,
                    supplier_email: &owner.email,
                    currency: "USD",
                    daily_rate_minor: 500,
                    offer_revision: 1,
                    replacement_of: None,
                },
                Utc::now(),
            )
            .await
            .expect_err("self-rental must not create debt");
        assert!(matches!(error, AppError::BadRequest(_)));
        let conn = store.conn.lock().await;
        let account_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM market_credit_accounts", [], |row| {
                row.get(0)
            })
            .expect("count self credit accounts");
        assert_eq!(account_count, 0);
    }

    #[tokio::test]
    async fn supplier_closure_permanently_blocks_future_paid_rentals() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-closed-credit", "buyer-closed-credit@example.com");
        let supplier = session(
            "supplier-closed-credit",
            "supplier-closed-credit@example.com",
        );
        configure_supplier(&store, &supplier, 10_000).await;
        let started_at = Utc::now();
        add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-closed-credit",
            500,
            started_at,
        )
        .await;
        let account_id = store
            .market_billing_dashboard(&supplier)
            .await
            .expect("load supplier credit account")
            .accounts[0]
            .id
            .clone();

        let actions = store
            .market_billing_open_account_invoice(&supplier.user_id, &account_id, true, false)
            .await
            .expect("close zero-balance credit relationship");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, BillingActionKind::Terminate);
        let dashboard = store
            .market_billing_dashboard(&supplier)
            .await
            .expect("load closed supplier credit account");
        assert_eq!(dashboard.accounts[0].status, ACCOUNT_CLOSED);
        assert!(!dashboard.accounts[0].can_close);

        let error = store
            .market_billing_check_credit_allowed(
                &buyer.user_id,
                &buyer.email,
                &supplier.user_id,
                crate::market_access::PRODUCT_CLIENT_HOST,
                "USD",
            )
            .await
            .expect_err("closed relationship must reject future paid rentals");
        assert_eq!(
            error.code(),
            Some(crate::market_access::ERROR_MARKET_RELATIONSHIP_CLOSED)
        );
        assert!(error.to_string().contains("permanently closed"));
    }

    #[tokio::test]
    async fn overdue_dispute_uphold_restores_restriction_and_cannot_repeat() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-dispute", "buyer-dispute@example.com");
        let supplier = session("supplier-dispute", "supplier-dispute@example.com");
        let admin = session("admin-billing", "admin@example.com");
        configure_supplier(&store, &supplier, 1).await;
        let started_at = Utc::now();
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-dispute",
            MONEY_UNITS_PER_MINOR,
            started_at,
        )
        .await;
        force_contract_out_of_trial(&store, &contract_id, started_at).await;
        let observed_at = started_at + Duration::seconds(1);
        record_client_health(&store, "client-dispute", observed_at, "healthy").await;
        let actions = store
            .market_billing_reconcile(observed_at)
            .await
            .expect("open disputed invoice fixture");
        assert_eq!(actions.len(), 1);
        store
            .market_billing_mark_control_applied(&contract_id, &BillingActionKind::Suspend)
            .await
            .expect("apply service suspension");
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load invoice fixture");
        let invoice = dashboard.accounts[0]
            .open_invoice
            .as_ref()
            .expect("open invoice");
        let invoice_id = invoice.id.clone();
        let overdue_at = parse_time(&invoice.deadline_at).expect("parse invoice deadline")
            + Duration::seconds(1);
        store
            .market_billing_reconcile(overdue_at)
            .await
            .expect("mark invoice overdue");
        let error = store
            .market_billing_check_credit_allowed(
                &buyer.user_id,
                &buyer.email,
                &supplier.user_id,
                crate::market_access::PRODUCT_CLIENT_HOST,
                "USD",
            )
            .await
            .expect_err("block credit after invoice becomes overdue");
        assert_eq!(
            error.code(),
            Some(crate::market_access::ERROR_MARKET_BUYER_RESTRICTED)
        );

        store
            .market_billing_open_dispute(&buyer, &invoice_id, "service evidence is incorrect")
            .await
            .expect("open first billing dispute");
        let error = store
            .market_billing_check_credit_allowed(
                &buyer.user_id,
                &buyer.email,
                &supplier.user_id,
                crate::market_access::PRODUCT_CLIENT_HOST,
                "USD",
            )
            .await
            .expect_err("keep disputed overdue buyer restricted");
        assert_eq!(
            error.code(),
            Some(crate::market_access::ERROR_MARKET_BUYER_RESTRICTED)
        );
        let disputed_dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load restricted disputed invoice");
        assert_eq!(disputed_dashboard.restrictions.len(), 1);
        assert_eq!(
            disputed_dashboard.accounts[0]
                .open_invoice
                .as_ref()
                .expect("restricted disputed invoice")
                .status,
            INVOICE_DISPUTED
        );
        let disputes = store
            .market_billing_admin_disputes()
            .await
            .expect("list admin disputes");
        assert_eq!(disputes.len(), 1);
        assert_eq!(disputes[0].invoice.id, invoice_id);
        store
            .market_billing_resolve_dispute(
                &admin,
                &disputes[0].dispute.id,
                "uphold",
                Some("health evidence confirms service"),
            )
            .await
            .expect("uphold overdue invoice");
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load upheld invoice");
        assert_eq!(dashboard.accounts[0].status, ACCOUNT_OVERDUE);
        assert_eq!(dashboard.restrictions.len(), 1);
        assert_eq!(
            dashboard.accounts[0]
                .open_invoice
                .as_ref()
                .expect("upheld invoice")
                .status,
            INVOICE_OVERDUE
        );

        assert!(matches!(
            store
                .market_billing_open_dispute(&buyer, &invoice_id, "request a second review")
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn voided_dispute_clears_debt_and_resumes_service() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let buyer = session("buyer-dispute-void", "buyer-dispute-void@example.com");
        let supplier = session("supplier-dispute-void", "supplier-dispute-void@example.com");
        let admin = session("admin-billing-void", "admin-void@example.com");
        configure_supplier(&store, &supplier, 1).await;
        let started_at = Utc::now();
        let contract_id = add_client_contract(
            &store,
            &buyer,
            &supplier,
            "client-dispute-void",
            MONEY_UNITS_PER_MINOR,
            started_at,
        )
        .await;
        force_contract_out_of_trial(&store, &contract_id, started_at).await;
        record_client_health(
            &store,
            "client-dispute-void",
            started_at + Duration::seconds(1),
            "healthy",
        )
        .await;
        store
            .market_billing_reconcile(started_at + Duration::seconds(1))
            .await
            .expect("open void-dispute invoice");
        store
            .market_billing_mark_control_applied(&contract_id, &BillingActionKind::Suspend)
            .await
            .expect("apply void-dispute suspension");
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load void-dispute invoice");
        let invoice = dashboard.accounts[0]
            .open_invoice
            .as_ref()
            .expect("void-dispute invoice");
        let invoice_id = invoice.id.clone();
        let overdue_at = parse_time(&invoice.deadline_at).expect("parse void-dispute deadline")
            + Duration::seconds(1);
        store
            .market_billing_open_dispute(&buyer, &invoice_id, "service evidence is incomplete")
            .await
            .expect("open voided billing dispute");
        store
            .market_billing_reconcile(overdue_at)
            .await
            .expect("restrict overdue disputed invoice");
        let error = store
            .market_billing_check_credit_allowed(
                &buyer.user_id,
                &buyer.email,
                &supplier.user_id,
                crate::market_access::PRODUCT_CLIENT_HOST,
                "USD",
            )
            .await
            .expect_err("restrict overdue disputed invoice");
        assert_eq!(
            error.code(),
            Some(crate::market_access::ERROR_MARKET_BUYER_RESTRICTED)
        );
        let disputed_dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load overdue disputed account");
        assert_eq!(disputed_dashboard.restrictions.len(), 1);
        assert_eq!(disputed_dashboard.accounts[0].status, ACCOUNT_DISPUTED);
        let disputes = store
            .market_billing_admin_disputes()
            .await
            .expect("list voided admin dispute");
        let resume_actions = store
            .market_billing_resolve_dispute(
                &admin,
                &disputes[0].dispute.id,
                "void",
                Some("evidence is insufficient"),
            )
            .await
            .expect("void disputed invoice");
        assert_eq!(resume_actions.len(), 1);
        assert_eq!(resume_actions[0].kind, BillingActionKind::Resume);
        let dashboard = store
            .market_billing_dashboard(&buyer)
            .await
            .expect("load voided account");
        assert_eq!(dashboard.accounts[0].status, ACCOUNT_ACTIVE);
        assert_eq!(dashboard.accounts[0].balance_minor, 0);
        assert!(dashboard.accounts[0].open_invoice.is_none());
        assert!(dashboard.restrictions.is_empty());
        assert_eq!(dashboard.accounts[0].services[0].status, CONTRACT_ACTIVE);
        let conn = store.conn.lock().await;
        let invoice_status: String = conn
            .query_row(
                "SELECT status FROM market_invoices WHERE id = ?1",
                params![invoice_id],
                |row| row.get(0),
            )
            .expect("read voided invoice");
        assert_eq!(invoice_status, INVOICE_VOID);
    }
}
