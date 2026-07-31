use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ServerState;
use crate::error::AppError;
#[cfg(test)]
use crate::models::AuthSession;
use crate::store::AppStore;

pub const PRODUCT_SHARE: &str = "share";
pub const PRODUCT_CLIENT_HOST: &str = "client_host";
pub const PRICING_FREE: &str = "free";
pub const PRICING_PAID: &str = "paid";
pub const MODE_WHITELIST: &str = "whitelist";
pub const MODE_BLACKLIST: &str = "blacklist";
pub const DECISION_INHERIT: &str = "inherit";
pub const DECISION_ALLOW: &str = "allow";
pub const DECISION_DENY: &str = "deny";
pub const CREDIT_NONE: &str = "none";
pub const CREDIT_LIMITED: &str = "limited";
pub const CREDIT_UNLIMITED: &str = "unlimited";

const MAX_CREDIT_LIMIT_MINOR: i64 = 100_000_000;

const CREATE_SUPPLIER_ACCESS_POLICIES_TABLE: &str =
    "CREATE TABLE IF NOT EXISTS market_supplier_access_policies (
        supplier_user_id TEXT NOT NULL,
        supplier_email TEXT NOT NULL,
        product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
        pricing_kind TEXT NOT NULL CHECK (pricing_kind IN ('free', 'paid')),
        mode TEXT NOT NULL CHECK (mode IN ('whitelist', 'blacklist')),
        revision INTEGER NOT NULL DEFAULT 1,
        risk_acknowledged_at TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (supplier_user_id, product_kind, pricing_kind)
    );";

const CREATE_COUNTERPARTY_ACCESS_RULES_TABLE: &str =
    "CREATE TABLE IF NOT EXISTS market_counterparty_access_rules (
        counterparty_id TEXT NOT NULL,
        product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
        pricing_kind TEXT NOT NULL CHECK (pricing_kind IN ('free', 'paid')),
        decision TEXT NOT NULL CHECK (decision IN ('inherit', 'allow', 'deny')),
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (counterparty_id, product_kind, pricing_kind)
    );";

#[derive(Debug, Clone)]
pub struct MarketAccessActor {
    pub user_id: String,
    pub email: String,
}

#[derive(Debug, Clone)]
pub struct EffectiveCreditGrant {
    pub kind: String,
    pub limit_minor: Option<i64>,
    pub source: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessPolicyView {
    pub product_kind: String,
    pub pricing_kind: String,
    pub mode: String,
    pub revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_acknowledged_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterpartyAccessRuleView {
    pub product_kind: String,
    pub pricing_kind: String,
    pub decision: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditLineView {
    pub currency: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_minor: Option<i64>,
    pub revision: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterpartyExposureView {
    pub currency: String,
    pub balance_minor: i64,
    pub status: String,
    pub active_service_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CounterpartyView {
    pub id: String,
    pub buyer_email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buyer_user_id: Option<String>,
    pub status: String,
    pub revision: i64,
    pub access_rules: Vec<CounterpartyAccessRuleView>,
    pub credit_lines: Vec<CreditLineView>,
    pub exposures: Vec<CounterpartyExposureView>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicCreditLineView {
    pub currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_minor: Option<i64>,
    pub enabled: bool,
    pub revision: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketAccessDashboardView {
    pub policies: Vec<AccessPolicyView>,
    pub counterparties: Vec<CounterpartyView>,
    pub public_credit_lines: Vec<PublicCreditLineView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdatePolicyRequest {
    mode: String,
    #[serde(default)]
    risk_acknowledged: bool,
    expected_revision: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRuleInput {
    pub product_kind: String,
    pub pricing_kind: String,
    pub decision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreditLineInput {
    pub currency: String,
    pub kind: String,
    #[serde(default)]
    pub limit_minor: Option<i64>,
    #[serde(default)]
    pub risk_acknowledged: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpsertCounterpartyRequest {
    email: String,
    #[serde(default)]
    access_rules: Vec<AccessRuleInput>,
    #[serde(default)]
    credit_lines: Vec<CreditLineInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateCounterpartyRequest {
    #[serde(default)]
    access_rules: Vec<AccessRuleInput>,
    #[serde(default)]
    status: Option<String>,
    expected_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateCreditLineRequest {
    kind: String,
    #[serde(default)]
    limit_minor: Option<i64>,
    #[serde(default)]
    risk_acknowledged: bool,
    expected_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdatePublicCreditLineRequest {
    enabled: bool,
    #[serde(default)]
    limit_minor: Option<i64>,
    #[serde(default)]
    risk_acknowledged: bool,
    expected_revision: i64,
}

fn pricing_scope_rebuild_source(
    conn: &Connection,
    table: &str,
    expected_primary_key: &[&str],
) -> Result<Option<bool>, rusqlite::Error> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let schema = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if schema.is_empty() {
        return Ok(None);
    }

    let has_pricing_kind = schema.iter().any(|(name, _)| name == "pricing_kind");
    let mut primary_key = schema
        .iter()
        .filter(|(_, position)| *position > 0)
        .map(|(name, position)| (*position, name.as_str()))
        .collect::<Vec<_>>();
    primary_key.sort_unstable_by_key(|(position, _)| *position);
    let primary_key = primary_key
        .into_iter()
        .map(|(_, name)| name)
        .collect::<Vec<_>>();

    Ok((!has_pricing_kind || primary_key != expected_primary_key).then_some(has_pricing_kind))
}

fn migrate_pricing_scoped_access_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    let supplier_policy_source = pricing_scope_rebuild_source(
        conn,
        "market_supplier_access_policies",
        &["supplier_user_id", "product_kind", "pricing_kind"],
    )?;
    let counterparty_rule_source = pricing_scope_rebuild_source(
        conn,
        "market_counterparty_access_rules",
        &["counterparty_id", "product_kind", "pricing_kind"],
    )?;
    if supplier_policy_source.is_none() && counterparty_rule_source.is_none() {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;
    if let Some(source_has_pricing_kind) = supplier_policy_source {
        tx.execute_batch(
            "ALTER TABLE market_supplier_access_policies
                 RENAME TO market_supplier_access_policies_legacy_pricing_scope;",
        )?;
        tx.execute_batch(CREATE_SUPPLIER_ACCESS_POLICIES_TABLE)?;
        if source_has_pricing_kind {
            tx.execute_batch(
                "INSERT INTO market_supplier_access_policies (
                    supplier_user_id, supplier_email, product_kind, pricing_kind, mode,
                    revision, risk_acknowledged_at, created_at, updated_at
                 )
                 SELECT supplier_user_id, supplier_email, product_kind, pricing_kind, mode,
                        revision, risk_acknowledged_at, created_at, updated_at
                 FROM market_supplier_access_policies_legacy_pricing_scope;",
            )?;
        } else {
            tx.execute_batch(
                "INSERT INTO market_supplier_access_policies (
                    supplier_user_id, supplier_email, product_kind, pricing_kind, mode,
                    revision, risk_acknowledged_at, created_at, updated_at
                 )
                 SELECT legacy.supplier_user_id, legacy.supplier_email, legacy.product_kind,
                        scope.pricing_kind, legacy.mode, legacy.revision,
                        legacy.risk_acknowledged_at, legacy.created_at, legacy.updated_at
                 FROM market_supplier_access_policies_legacy_pricing_scope AS legacy
                 CROSS JOIN (
                    SELECT 'free' AS pricing_kind
                    UNION ALL SELECT 'paid'
                 ) AS scope;",
            )?;
        }
        tx.execute_batch("DROP TABLE market_supplier_access_policies_legacy_pricing_scope;")?;
    }

    if let Some(source_has_pricing_kind) = counterparty_rule_source {
        tx.execute_batch(
            "ALTER TABLE market_counterparty_access_rules
                 RENAME TO market_counterparty_access_rules_legacy_pricing_scope;",
        )?;
        tx.execute_batch(CREATE_COUNTERPARTY_ACCESS_RULES_TABLE)?;
        if source_has_pricing_kind {
            tx.execute_batch(
                "INSERT INTO market_counterparty_access_rules (
                    counterparty_id, product_kind, pricing_kind, decision, created_at, updated_at
                 )
                 SELECT counterparty_id, product_kind, pricing_kind, decision,
                        created_at, updated_at
                 FROM market_counterparty_access_rules_legacy_pricing_scope;",
            )?;
        } else {
            tx.execute_batch(
                "INSERT INTO market_counterparty_access_rules (
                    counterparty_id, product_kind, pricing_kind, decision, created_at, updated_at
                 )
                 SELECT legacy.counterparty_id, legacy.product_kind, scope.pricing_kind,
                        legacy.decision, legacy.created_at, legacy.updated_at
                 FROM market_counterparty_access_rules_legacy_pricing_scope AS legacy
                 CROSS JOIN (
                    SELECT 'free' AS pricing_kind
                    UNION ALL SELECT 'paid'
                 ) AS scope;",
            )?;
        }
        tx.execute_batch("DROP TABLE market_counterparty_access_rules_legacy_pricing_scope;")?;
    }
    tx.commit()
}

pub fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    migrate_pricing_scoped_access_tables(conn)?;
    conn.execute_batch(CREATE_SUPPLIER_ACCESS_POLICIES_TABLE)?;
    conn.execute_batch(CREATE_COUNTERPARTY_ACCESS_RULES_TABLE)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS market_counterparties (
            id TEXT PRIMARY KEY,
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            buyer_user_id TEXT,
            buyer_email TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
            revision INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            revoked_at TEXT,
            UNIQUE (supplier_user_id, buyer_email)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS uq_market_counterparty_bound_user
            ON market_counterparties(supplier_user_id, buyer_user_id)
            WHERE buyer_user_id IS NOT NULL;
        CREATE TABLE IF NOT EXISTS market_credit_grants (
            counterparty_id TEXT NOT NULL,
            currency TEXT NOT NULL,
            kind TEXT NOT NULL CHECK (kind IN ('none', 'limited', 'unlimited')),
            limit_minor INTEGER,
            revision INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (counterparty_id, currency)
        );
        CREATE TABLE IF NOT EXISTS market_public_credit_policies (
            supplier_user_id TEXT NOT NULL,
            supplier_email TEXT NOT NULL,
            currency TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 0,
            limit_minor INTEGER,
            revision INTEGER NOT NULL DEFAULT 1,
            risk_acknowledged_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (supplier_user_id, currency)
        );
        CREATE TABLE IF NOT EXISTS market_access_events (
            id TEXT PRIMARY KEY,
            supplier_user_id TEXT NOT NULL,
            counterparty_id TEXT,
            actor_user_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            detail_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_market_counterparties_supplier_status
            ON market_counterparties(supplier_user_id, status, updated_at);
        CREATE INDEX IF NOT EXISTS idx_market_access_events_supplier
            ON market_access_events(supplier_user_id, created_at);",
    )?;
    Ok(())
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-access/dashboard", get(get_dashboard))
        .route(
            "/v1/market-access/policies/:product_kind/:pricing_kind",
            put(update_policy),
        )
        .route(
            "/v1/market-access/counterparties",
            post(upsert_counterparty),
        )
        .route(
            "/v1/market-access/counterparties/:id",
            put(update_counterparty),
        )
        .route(
            "/v1/market-access/counterparties/:id/credit-lines/:currency",
            put(update_credit_line),
        )
        .route(
            "/v1/market-access/public-credit-lines/:currency",
            put(update_public_credit_line),
        )
}

fn map_db(context: &'static str) -> impl FnOnce(rusqlite::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn normalize_product_kind(value: &str) -> Result<String, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        PRODUCT_SHARE => Ok(PRODUCT_SHARE.into()),
        PRODUCT_CLIENT_HOST => Ok(PRODUCT_CLIENT_HOST.into()),
        _ => Err(AppError::BadRequest(
            "productKind must be share or client_host".into(),
        )),
    }
}

fn normalize_pricing_kind(value: &str) -> Result<String, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        PRICING_FREE => Ok(PRICING_FREE.into()),
        PRICING_PAID => Ok(PRICING_PAID.into()),
        _ => Err(AppError::BadRequest(
            "pricingKind must be free or paid".into(),
        )),
    }
}

fn default_access_mode(pricing_kind: &str) -> &'static str {
    if pricing_kind == PRICING_FREE {
        MODE_BLACKLIST
    } else {
        MODE_WHITELIST
    }
}

pub(crate) fn pricing_kind_for_rate(daily_rate_minor: Option<i64>) -> &'static str {
    if daily_rate_minor.is_some() {
        PRICING_PAID
    } else {
        PRICING_FREE
    }
}

fn normalize_mode(value: &str) -> Result<String, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        MODE_WHITELIST => Ok(MODE_WHITELIST.into()),
        MODE_BLACKLIST => Ok(MODE_BLACKLIST.into()),
        _ => Err(AppError::BadRequest(
            "mode must be whitelist or blacklist".into(),
        )),
    }
}

fn validate_policy_transition(
    current_mode: &str,
    requested_mode: &str,
    risk_acknowledged: bool,
) -> Result<(), AppError> {
    if current_mode != requested_mode && requested_mode == MODE_BLACKLIST && !risk_acknowledged {
        return Err(AppError::BadRequest(
            "switching to blacklist mode requires riskAcknowledged=true".into(),
        ));
    }
    Ok(())
}

fn normalize_decision(value: &str) -> Result<String, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        DECISION_INHERIT => Ok(DECISION_INHERIT.into()),
        DECISION_ALLOW => Ok(DECISION_ALLOW.into()),
        DECISION_DENY => Ok(DECISION_DENY.into()),
        _ => Err(AppError::BadRequest(
            "decision must be inherit, allow, or deny".into(),
        )),
    }
}

pub(crate) fn normalize_currency(value: &str) -> Result<String, AppError> {
    let currency = value.trim().to_ascii_uppercase();
    if matches!(currency.as_str(), "CNY" | "USD") {
        Ok(currency)
    } else {
        Err(AppError::BadRequest("currency must be CNY or USD".into()))
    }
}

fn normalize_email(value: &str) -> Result<String, AppError> {
    let email = value.trim().to_ascii_lowercase();
    if !crate::notifications::is_basic_email(&email) {
        return Err(AppError::BadRequest("invalid account email".into()));
    }
    Ok(email)
}

fn validate_credit_line(
    kind: &str,
    limit_minor: Option<i64>,
    risk_acknowledged: bool,
) -> Result<(String, Option<i64>), AppError> {
    let kind = kind.trim().to_ascii_lowercase();
    match kind.as_str() {
        CREDIT_NONE => Ok((kind, None)),
        CREDIT_LIMITED => {
            let limit = limit_minor
                .ok_or_else(|| AppError::BadRequest("limited credit requires limitMinor".into()))?;
            if !(1..=MAX_CREDIT_LIMIT_MINOR).contains(&limit) {
                return Err(AppError::BadRequest(format!(
                    "limitMinor must be between 1 and {MAX_CREDIT_LIMIT_MINOR}"
                )));
            }
            Ok((kind, Some(limit)))
        }
        CREDIT_UNLIMITED => {
            if !risk_acknowledged {
                return Err(AppError::BadRequest(
                    "unlimited credit requires riskAcknowledged=true".into(),
                ));
            }
            Ok((kind, None))
        }
        _ => Err(AppError::BadRequest(
            "credit kind must be none, limited, or unlimited".into(),
        )),
    }
}

fn validate_public_credit_line(
    enabled: bool,
    limit_minor: Option<i64>,
    risk_acknowledged: bool,
) -> Result<Option<i64>, AppError> {
    if !enabled {
        return Ok(None);
    }
    if !risk_acknowledged {
        return Err(AppError::BadRequest(
            "public credit requires riskAcknowledged=true".into(),
        ));
    }
    let limit = limit_minor
        .ok_or_else(|| AppError::BadRequest("enabled public credit requires limitMinor".into()))?;
    if !(1..=MAX_CREDIT_LIMIT_MINOR).contains(&limit) {
        return Err(AppError::BadRequest(format!(
            "limitMinor must be between 1 and {MAX_CREDIT_LIMIT_MINOR}"
        )));
    }
    Ok(Some(limit))
}

async fn require_actor(
    state: &ServerState,
    headers: &HeaderMap,
    required_scope: &str,
) -> Result<MarketAccessActor, AppError> {
    if let Some(session) = crate::api::resolve_router_session(state, headers).await? {
        return Ok(MarketAccessActor {
            user_id: session.user_id,
            email: session.email,
        });
    }
    let token = crate::api::extract_router_api_token(headers)
        .ok_or_else(|| AppError::Unauthorized("authenticated user token required".into()))?;
    let principal = state
        .store
        .resolve_user_api_token(token, required_scope)
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid user api token".into()))?;
    Ok(MarketAccessActor {
        user_id: principal.user_id,
        email: principal.email,
    })
}

fn record_event_tx(
    tx: &Connection,
    supplier_user_id: &str,
    counterparty_id: Option<&str>,
    actor_user_id: &str,
    event_type: &str,
    detail: serde_json::Value,
    now: &str,
) -> Result<(), AppError> {
    tx.execute(
        "INSERT INTO market_access_events (
            id, supplier_user_id, counterparty_id, actor_user_id,
            event_type, detail_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            Uuid::new_v4().to_string(),
            supplier_user_id,
            counterparty_id,
            actor_user_id,
            event_type,
            detail.to_string(),
            now,
        ],
    )
    .map_err(map_db("record market access event"))?;
    Ok(())
}

fn policy_view_tx(
    conn: &Connection,
    supplier_user_id: &str,
    product_kind: &str,
    pricing_kind: &str,
) -> Result<AccessPolicyView, AppError> {
    conn.query_row(
        "SELECT mode, revision, risk_acknowledged_at, updated_at
         FROM market_supplier_access_policies
         WHERE supplier_user_id = ?1 AND product_kind = ?2 AND pricing_kind = ?3",
        params![supplier_user_id, product_kind, pricing_kind],
        |row| {
            Ok(AccessPolicyView {
                product_kind: product_kind.to_string(),
                pricing_kind: pricing_kind.to_string(),
                mode: row.get(0)?,
                revision: row.get(1)?,
                risk_acknowledged_at: row.get(2)?,
                updated_at: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(map_db("read market access policy"))?
    .map(Ok)
    .unwrap_or_else(|| {
        Ok(AccessPolicyView {
            product_kind: product_kind.to_string(),
            pricing_kind: pricing_kind.to_string(),
            mode: default_access_mode(pricing_kind).into(),
            revision: 0,
            risk_acknowledged_at: None,
            updated_at: String::new(),
        })
    })
}

fn counterparty_view_tx(conn: &Connection, id: &str) -> Result<CounterpartyView, AppError> {
    let mut view = conn
        .query_row(
            "SELECT id, buyer_email, buyer_user_id, status, revision, created_at, updated_at
             FROM market_counterparties WHERE id = ?1",
            params![id],
            |row| {
                Ok(CounterpartyView {
                    id: row.get(0)?,
                    buyer_email: row.get(1)?,
                    buyer_user_id: row.get(2)?,
                    status: row.get(3)?,
                    revision: row.get(4)?,
                    access_rules: Vec::new(),
                    credit_lines: Vec::new(),
                    exposures: Vec::new(),
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(map_db("read market counterparty"))?
        .ok_or_else(|| AppError::NotFound("market counterparty not found".into()))?;
    view.access_rules = conn
        .prepare(
            "SELECT product_kind, pricing_kind, decision FROM market_counterparty_access_rules
             WHERE counterparty_id = ?1 ORDER BY product_kind, pricing_kind",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![id], |row| {
                    Ok(CounterpartyAccessRuleView {
                        product_kind: row.get(0)?,
                        pricing_kind: row.get(1)?,
                        decision: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read counterparty access rules"))?;
    view.credit_lines = conn
        .prepare(
            "SELECT currency, kind, limit_minor, revision, updated_at
             FROM market_credit_grants WHERE counterparty_id = ?1 ORDER BY currency",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![id], |row| {
                    Ok(CreditLineView {
                        currency: row.get(0)?,
                        kind: row.get(1)?,
                        limit_minor: row.get(2)?,
                        revision: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read counterparty credit lines"))?;
    if let Some(buyer_user_id) = view.buyer_user_id.as_deref() {
        view.exposures = conn
            .prepare(
                "SELECT account.currency,
                        (account.balance_units + 86399) / 86400,
                        account.status,
                        (SELECT COUNT(*) FROM market_service_contracts contract
                         WHERE contract.account_id = account.id
                           AND contract.status IN ('trial', 'active', 'billing_suspended'))
                 FROM market_credit_accounts account
                 JOIN market_counterparties counterparty ON counterparty.supplier_user_id = account.supplier_user_id
                 WHERE counterparty.id = ?1 AND account.buyer_user_id = ?2
                 ORDER BY account.currency",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![id, buyer_user_id], |row| {
                        Ok(CounterpartyExposureView {
                            currency: row.get(0)?,
                            balance_minor: row.get(1)?,
                            status: row.get(2)?,
                            active_service_count: row.get(3)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read counterparty exposure"))?;
    }
    Ok(view)
}

impl AppStore {
    pub async fn market_access_dashboard(
        &self,
        actor: &MarketAccessActor,
    ) -> Result<MarketAccessDashboardView, AppError> {
        let conn = self.conn.lock().await;
        let policies = [PRODUCT_SHARE, PRODUCT_CLIENT_HOST]
            .into_iter()
            .flat_map(|product_kind| {
                [PRICING_FREE, PRICING_PAID]
                    .into_iter()
                    .map(move |pricing_kind| (product_kind, pricing_kind))
            })
            .map(|(product_kind, pricing_kind)| {
                policy_view_tx(&conn, &actor.user_id, product_kind, pricing_kind)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ids = conn
            .prepare(
                "SELECT id FROM market_counterparties WHERE supplier_user_id = ?1
                 ORDER BY updated_at DESC, id",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![actor.user_id], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("list market counterparties"))?;
        let counterparties = ids
            .iter()
            .map(|id| counterparty_view_tx(&conn, id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut public_credit_lines = conn
            .prepare(
                "SELECT currency, enabled, limit_minor, revision, updated_at
                 FROM market_public_credit_policies WHERE supplier_user_id = ?1
                 ORDER BY currency",
            )
            .and_then(|mut statement| {
                statement
                    .query_map(params![actor.user_id], |row| {
                        Ok(PublicCreditLineView {
                            currency: row.get(0)?,
                            enabled: row.get::<_, i64>(1)? != 0,
                            limit_minor: row.get(2)?,
                            revision: row.get(3)?,
                            updated_at: row.get(4)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("list public credit policies"))?;
        for currency in ["CNY", "USD"] {
            if !public_credit_lines
                .iter()
                .any(|line| line.currency == currency)
            {
                public_credit_lines.push(PublicCreditLineView {
                    currency: currency.into(),
                    enabled: false,
                    limit_minor: None,
                    revision: 0,
                    updated_at: String::new(),
                });
            }
        }
        public_credit_lines.sort_by(|left, right| left.currency.cmp(&right.currency));
        Ok(MarketAccessDashboardView {
            policies,
            counterparties,
            public_credit_lines,
        })
    }
}

async fn get_dashboard(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<MarketAccessDashboardView>, AppError> {
    let actor = require_actor(&state, &headers, "market:access:read").await?;
    Ok(Json(state.store.market_access_dashboard(&actor).await?))
}

async fn update_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((product_kind, pricing_kind)): Path<(String, String)>,
    Json(input): Json<UpdatePolicyRequest>,
) -> Result<Json<MarketAccessDashboardView>, AppError> {
    let actor = require_actor(&state, &headers, "market:access:write").await?;
    let product_kind = normalize_product_kind(&product_kind)?;
    let pricing_kind = normalize_pricing_kind(&pricing_kind)?;
    let mode = normalize_mode(&input.mode)?;
    {
        let now = Utc::now().to_rfc3339();
        let mut conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin access policy update"))?;
        let current = policy_view_tx(&tx, &actor.user_id, &product_kind, &pricing_kind)?;
        if input.expected_revision != current.revision {
            return Err(AppError::Conflict(
                "market access policy revision changed; reload before saving".into(),
            ));
        }
        if current.mode != mode {
            validate_policy_transition(&current.mode, &mode, input.risk_acknowledged)?;
            tx.execute(
                "INSERT INTO market_supplier_access_policies (
            supplier_user_id, supplier_email, product_kind, pricing_kind, mode, revision,
            risk_acknowledged_at, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?7)
         ON CONFLICT(supplier_user_id, product_kind, pricing_kind) DO UPDATE SET
            supplier_email = excluded.supplier_email,
            mode = excluded.mode,
            revision = market_supplier_access_policies.revision + 1,
            risk_acknowledged_at = excluded.risk_acknowledged_at,
            updated_at = excluded.updated_at",
                params![
                    actor.user_id,
                    actor.email,
                    product_kind,
                    pricing_kind,
                    mode,
                    if mode == MODE_BLACKLIST {
                        Some(now.as_str())
                    } else {
                        None
                    },
                    now,
                ],
            )
            .map_err(map_db("upsert market access policy"))?;
            record_event_tx(
                &tx,
                &actor.user_id,
                None,
                &actor.user_id,
                "access_policy_updated",
                serde_json::json!({
                    "productKind": product_kind,
                    "pricingKind": pricing_kind,
                    "mode": mode,
                }),
                &now,
            )?;
        }
        tx.commit().map_err(map_db("commit access policy update"))?;
    }
    Ok(Json(state.store.market_access_dashboard(&actor).await?))
}

fn upsert_access_rule_tx(
    tx: &Connection,
    counterparty_id: &str,
    input: &AccessRuleInput,
    now: &str,
) -> Result<(), AppError> {
    let product_kind = normalize_product_kind(&input.product_kind)?;
    let pricing_kind = normalize_pricing_kind(&input.pricing_kind)?;
    let decision = normalize_decision(&input.decision)?;
    tx.execute(
        "INSERT INTO market_counterparty_access_rules (
            counterparty_id, product_kind, pricing_kind, decision, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(counterparty_id, product_kind, pricing_kind) DO UPDATE SET
            decision = excluded.decision, updated_at = excluded.updated_at",
        params![counterparty_id, product_kind, pricing_kind, decision, now],
    )
    .map_err(map_db("upsert counterparty access rule"))?;
    Ok(())
}

fn upsert_credit_line_tx(
    tx: &Connection,
    counterparty_id: &str,
    input: &CreditLineInput,
    expected_revision: Option<i64>,
    now: &str,
) -> Result<(), AppError> {
    let currency = normalize_currency(&input.currency)?;
    let (kind, limit_minor) =
        validate_credit_line(&input.kind, input.limit_minor, input.risk_acknowledged)?;
    let current_revision = tx
        .query_row(
            "SELECT revision FROM market_credit_grants
             WHERE counterparty_id = ?1 AND currency = ?2",
            params![counterparty_id, currency],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(map_db("read credit grant revision"))?
        .unwrap_or(0);
    if expected_revision.is_some_and(|revision| revision != current_revision) {
        return Err(AppError::Conflict(
            "credit line revision changed; reload before saving".into(),
        ));
    }
    tx.execute(
        "INSERT INTO market_credit_grants (
            counterparty_id, currency, kind, limit_minor, revision, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)
         ON CONFLICT(counterparty_id, currency) DO UPDATE SET
            kind = excluded.kind, limit_minor = excluded.limit_minor,
            revision = market_credit_grants.revision + 1,
            updated_at = excluded.updated_at",
        params![counterparty_id, currency, kind, limit_minor, now],
    )
    .map_err(map_db("upsert counterparty credit line"))?;
    Ok(())
}

fn apply_counterparty_credit_line_to_accounts_tx(
    tx: &Connection,
    supplier_user_id: &str,
    counterparty_id: &str,
    currency: &str,
    now: &str,
) -> Result<(String, Option<i64>, i64), AppError> {
    let currency = normalize_currency(currency)?;
    let grant = tx
        .query_row(
            "SELECT kind, limit_minor, revision FROM market_credit_grants
             WHERE counterparty_id = ?1 AND currency = ?2",
            params![counterparty_id, currency],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(map_db("read updated credit grant"))?;
    tx.execute(
        "UPDATE market_credit_accounts
         SET credit_kind = ?3, credit_limit_minor = ?4,
             credit_source = 'counterparty', credit_revision = ?5, updated_at = ?6
         WHERE supplier_user_id = ?1 AND currency = ?2
           AND (buyer_user_id = (SELECT buyer_user_id FROM market_counterparties WHERE id = ?7)
                OR buyer_email = (SELECT buyer_email FROM market_counterparties WHERE id = ?7))",
        params![
            supplier_user_id,
            currency,
            grant.0,
            grant.1,
            grant.2,
            now,
            counterparty_id,
        ],
    )
    .map_err(map_db("apply credit grant to active accounts"))?;
    Ok(grant)
}

fn revoke_counterparty_credit_accounts_tx(
    tx: &Connection,
    supplier_user_id: &str,
    counterparty_id: &str,
    now: &str,
) -> Result<usize, AppError> {
    tx.execute(
        "UPDATE market_credit_accounts
         SET credit_kind = 'none', credit_limit_minor = NULL,
             credit_source = 'counterparty',
             credit_revision = credit_revision + 1, updated_at = ?2
         WHERE supplier_user_id = ?1
           AND (buyer_user_id = (
                    SELECT buyer_user_id FROM market_counterparties WHERE id = ?3
                )
                OR buyer_email = (
                    SELECT buyer_email FROM market_counterparties WHERE id = ?3
                ))",
        params![supplier_user_id, now, counterparty_id],
    )
    .map_err(map_db("revoke counterparty credit accounts"))
}

async fn upsert_counterparty(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpsertCounterpartyRequest>,
) -> Result<Json<CounterpartyView>, AppError> {
    let actor = require_actor(&state, &headers, "market:access:write").await?;
    let email = normalize_email(&input.email)?;
    if email == actor.email.trim().to_ascii_lowercase() {
        return Err(AppError::BadRequest(
            "cannot add your own account as a market counterparty".into(),
        ));
    }
    let credit_lines_changed = !input.credit_lines.is_empty();
    let id = {
        let now = Utc::now().to_rfc3339();
        let mut conn = state.store.conn.lock().await;
        let buyer_user_id = conn
            .query_row(
                "SELECT id FROM users WHERE email_normalized = ?1",
                params![email],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("resolve counterparty account"))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin counterparty upsert"))?;
        let existing = tx
            .query_row(
                "SELECT id FROM market_counterparties
             WHERE supplier_user_id = ?1 AND buyer_email = ?2",
                params![actor.user_id, email],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("read existing counterparty"))?;
        let id = existing.unwrap_or_else(|| Uuid::new_v4().to_string());
        tx.execute(
            "INSERT INTO market_counterparties (
            id, supplier_user_id, supplier_email, buyer_user_id, buyer_email,
            status, revision, created_at, updated_at, revoked_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', 1, ?6, ?6, NULL)
         ON CONFLICT(supplier_user_id, buyer_email) DO UPDATE SET
            supplier_email = excluded.supplier_email,
            buyer_user_id = COALESCE(market_counterparties.buyer_user_id, excluded.buyer_user_id),
            status = 'active', revision = market_counterparties.revision + 1,
            updated_at = excluded.updated_at, revoked_at = NULL",
            params![id, actor.user_id, actor.email, buyer_user_id, email, now],
        )
        .map_err(map_db("upsert market counterparty"))?;
        for rule in &input.access_rules {
            upsert_access_rule_tx(&tx, &id, rule, &now)?;
        }
        for line in &input.credit_lines {
            upsert_credit_line_tx(&tx, &id, line, None, &now)?;
            apply_counterparty_credit_line_to_accounts_tx(
                &tx,
                &actor.user_id,
                &id,
                &line.currency,
                &now,
            )?;
        }
        record_event_tx(
            &tx,
            &actor.user_id,
            Some(&id),
            &actor.user_id,
            "counterparty_upserted",
            serde_json::json!({ "buyerEmail": email }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit counterparty upsert"))?;
        id
    };
    if credit_lines_changed {
        let actions = state.store.market_billing_reconcile(Utc::now()).await?;
        crate::market_billing::dispatch_actions(&state, actions).await;
    }
    let conn = state.store.conn.lock().await;
    Ok(Json(counterparty_view_tx(&conn, &id)?))
}

async fn update_counterparty(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateCounterpartyRequest>,
) -> Result<Json<CounterpartyView>, AppError> {
    let actor = require_actor(&state, &headers, "market:access:write").await?;
    let credit_revoked = {
        let now = Utc::now().to_rfc3339();
        let mut conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin counterparty update"))?;
        let current: (String, i64, String) = tx
            .query_row(
                "SELECT supplier_user_id, revision, status
             FROM market_counterparties WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(map_db("read counterparty owner"))?
            .ok_or_else(|| AppError::NotFound("market counterparty not found".into()))?;
        if current.0 != actor.user_id {
            return Err(AppError::Forbidden(
                "market counterparty belongs to another supplier".into(),
            ));
        }
        if current.1 != input.expected_revision {
            return Err(AppError::Conflict(
                "market counterparty revision changed; reload before saving".into(),
            ));
        }
        let status = input
            .status
            .as_deref()
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or(current.2);
        if !matches!(status.as_str(), "active" | "revoked") {
            return Err(AppError::BadRequest(
                "counterparty status must be active or revoked".into(),
            ));
        }
        tx.execute(
            "UPDATE market_counterparties
         SET status = ?2, revision = revision + 1, updated_at = ?3,
             revoked_at = CASE WHEN ?2 = 'revoked' THEN ?3 ELSE NULL END
         WHERE id = ?1",
            params![id, status, now],
        )
        .map_err(map_db("update market counterparty"))?;
        let credit_revoked = status == "revoked";
        if credit_revoked {
            revoke_counterparty_credit_accounts_tx(&tx, &actor.user_id, &id, &now)?;
        }
        for rule in &input.access_rules {
            upsert_access_rule_tx(&tx, &id, rule, &now)?;
        }
        record_event_tx(
            &tx,
            &actor.user_id,
            Some(&id),
            &actor.user_id,
            "counterparty_updated",
            serde_json::json!({ "status": status, "accessRules": input.access_rules }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit counterparty update"))?;
        credit_revoked
    };
    if credit_revoked {
        let actions = state.store.market_billing_reconcile(Utc::now()).await?;
        crate::market_billing::dispatch_actions(&state, actions).await;
    }
    let conn = state.store.conn.lock().await;
    Ok(Json(counterparty_view_tx(&conn, &id)?))
}

async fn update_credit_line(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path((id, currency)): Path<(String, String)>,
    Json(input): Json<UpdateCreditLineRequest>,
) -> Result<Json<CounterpartyView>, AppError> {
    let actor = require_actor(&state, &headers, "market:access:write").await?;
    let currency = normalize_currency(&currency)?;
    {
        let now = Utc::now().to_rfc3339();
        let mut conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin credit line update"))?;
        let supplier_user_id = tx
            .query_row(
                "SELECT supplier_user_id FROM market_counterparties WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("read credit line counterparty"))?
            .ok_or_else(|| AppError::NotFound("market counterparty not found".into()))?;
        if supplier_user_id != actor.user_id {
            return Err(AppError::Forbidden(
                "market counterparty belongs to another supplier".into(),
            ));
        }
        upsert_credit_line_tx(
            &tx,
            &id,
            &CreditLineInput {
                currency: currency.clone(),
                kind: input.kind,
                limit_minor: input.limit_minor,
                risk_acknowledged: input.risk_acknowledged,
            },
            Some(input.expected_revision),
            &now,
        )?;
        let grant = apply_counterparty_credit_line_to_accounts_tx(
            &tx,
            &actor.user_id,
            &id,
            &currency,
            &now,
        )?;
        record_event_tx(
            &tx,
            &actor.user_id,
            Some(&id),
            &actor.user_id,
            "credit_line_updated",
            serde_json::json!({
                "currency": currency,
                "kind": grant.0,
                "limitMinor": grant.1,
                "revision": grant.2,
            }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit credit line update"))?;
    }
    let actions = state.store.market_billing_reconcile(Utc::now()).await?;
    crate::market_billing::dispatch_actions(&state, actions).await;
    let conn = state.store.conn.lock().await;
    Ok(Json(counterparty_view_tx(&conn, &id)?))
}

async fn update_public_credit_line(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(currency): Path<String>,
    Json(input): Json<UpdatePublicCreditLineRequest>,
) -> Result<Json<MarketAccessDashboardView>, AppError> {
    let actor = require_actor(&state, &headers, "market:access:write").await?;
    let currency = normalize_currency(&currency)?;
    let limit_minor =
        validate_public_credit_line(input.enabled, input.limit_minor, input.risk_acknowledged)?;
    {
        let now = Utc::now().to_rfc3339();
        let mut conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin public credit update"))?;
        let revision = tx
            .query_row(
                "SELECT revision FROM market_public_credit_policies
             WHERE supplier_user_id = ?1 AND currency = ?2",
                params![actor.user_id, currency],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_db("read public credit revision"))?
            .unwrap_or(0);
        if input.expected_revision != revision {
            return Err(AppError::Conflict(
                "public credit revision changed; reload before saving".into(),
            ));
        }
        tx.execute(
            "INSERT INTO market_public_credit_policies (
            supplier_user_id, supplier_email, currency, enabled, limit_minor,
            revision, risk_acknowledged_at, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?7)
         ON CONFLICT(supplier_user_id, currency) DO UPDATE SET
            supplier_email = excluded.supplier_email,
            enabled = excluded.enabled, limit_minor = excluded.limit_minor,
            revision = market_public_credit_policies.revision + 1,
            risk_acknowledged_at = excluded.risk_acknowledged_at,
            updated_at = excluded.updated_at",
            params![
                actor.user_id,
                actor.email,
                currency,
                i64::from(input.enabled),
                limit_minor,
                if input.enabled {
                    Some(now.as_str())
                } else {
                    None
                },
                now,
            ],
        )
        .map_err(map_db("upsert public credit policy"))?;
        tx.execute(
            "UPDATE market_credit_accounts
         SET credit_kind = CASE WHEN ?3 = 1 THEN 'limited' ELSE 'none' END,
             credit_limit_minor = ?4, credit_source = 'public',
             credit_revision = (SELECT revision FROM market_public_credit_policies
                                WHERE supplier_user_id = ?1 AND currency = ?2),
             updated_at = ?5
         WHERE supplier_user_id = ?1 AND currency = ?2 AND credit_source = 'public'",
            params![
                actor.user_id,
                currency,
                i64::from(input.enabled),
                limit_minor,
                now,
            ],
        )
        .map_err(map_db("apply public credit policy to active accounts"))?;
        record_event_tx(
            &tx,
            &actor.user_id,
            None,
            &actor.user_id,
            "public_credit_updated",
            serde_json::json!({
                "currency": currency,
                "enabled": input.enabled,
                "limitMinor": limit_minor,
            }),
            &now,
        )?;
        tx.commit().map_err(map_db("commit public credit update"))?;
    }
    let actions = state.store.market_billing_reconcile(Utc::now()).await?;
    crate::market_billing::dispatch_actions(&state, actions).await;
    Ok(Json(state.store.market_access_dashboard(&actor).await?))
}

fn relationship_for_buyer_tx(
    conn: &Connection,
    supplier_user_id: &str,
    buyer_user_id: &str,
    buyer_email: &str,
) -> Result<Option<(String, Option<String>, String)>, AppError> {
    conn.query_row(
        "SELECT id, buyer_user_id, status FROM market_counterparties
         WHERE supplier_user_id = ?1
           AND (buyer_user_id = ?2 OR buyer_email = ?3)
         ORDER BY CASE WHEN buyer_user_id = ?2 THEN 0 ELSE 1 END LIMIT 1",
        params![supplier_user_id, buyer_user_id, buyer_email],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(map_db("resolve market counterparty"))
}

fn access_mode_tx(
    conn: &Connection,
    supplier_user_id: &str,
    product_kind: &str,
    pricing_kind: &str,
) -> Result<String, AppError> {
    conn.query_row(
        "SELECT mode FROM market_supplier_access_policies
         WHERE supplier_user_id = ?1 AND product_kind = ?2 AND pricing_kind = ?3",
        params![supplier_user_id, product_kind, pricing_kind],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map(|value| value.unwrap_or_else(|| default_access_mode(pricing_kind).into()))
    .map_err(map_db("read supplier access mode"))
}

pub(crate) fn product_access_allowed_tx(
    conn: &Connection,
    supplier_user_id: &str,
    buyer_user_id: &str,
    buyer_email: &str,
    product_kind: &str,
    pricing_kind: &str,
) -> Result<bool, AppError> {
    let product_kind = normalize_product_kind(product_kind)?;
    let pricing_kind = normalize_pricing_kind(pricing_kind)?;
    if supplier_user_id == buyer_user_id && product_kind == PRODUCT_CLIENT_HOST {
        return Ok(true);
    }
    let buyer_email = normalize_email(buyer_email)?;
    let mode = access_mode_tx(conn, supplier_user_id, &product_kind, &pricing_kind)?;
    let relationship =
        relationship_for_buyer_tx(conn, supplier_user_id, buyer_user_id, &buyer_email)?;
    let Some((relationship_id, _, status)) = relationship else {
        return Ok(mode == MODE_BLACKLIST);
    };
    if status != "active" {
        return Ok(false);
    }
    let decision = conn
        .query_row(
            "SELECT decision FROM market_counterparty_access_rules
             WHERE counterparty_id = ?1 AND product_kind = ?2 AND pricing_kind = ?3",
            params![relationship_id, product_kind, pricing_kind],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read counterparty access decision"))?
        .unwrap_or_else(|| DECISION_INHERIT.into());
    Ok(match decision.as_str() {
        DECISION_ALLOW => true,
        DECISION_DENY => false,
        _ => mode == MODE_BLACKLIST,
    })
}

pub(crate) fn ensure_product_access_tx(
    tx: &Transaction<'_>,
    supplier_user_id: &str,
    buyer_user_id: &str,
    buyer_email: &str,
    product_kind: &str,
    pricing_kind: &str,
) -> Result<(), AppError> {
    if !product_access_allowed_tx(
        tx,
        supplier_user_id,
        buyer_user_id,
        buyer_email,
        product_kind,
        pricing_kind,
    )? {
        return Err(AppError::Forbidden(
            "seller approval is required before renting this market service".into(),
        ));
    }
    let buyer_email = normalize_email(buyer_email)?;
    if let Some((relationship_id, bound_user_id, _)) =
        relationship_for_buyer_tx(tx, supplier_user_id, buyer_user_id, &buyer_email)?
    {
        if let Some(bound_user_id) = bound_user_id {
            if bound_user_id != buyer_user_id {
                return Err(AppError::Conflict(
                    "seller approval is bound to another Router account".into(),
                ));
            }
        } else {
            tx.execute(
                "UPDATE market_counterparties
                 SET buyer_user_id = ?2, revision = revision + 1, updated_at = ?3
                 WHERE id = ?1 AND buyer_user_id IS NULL",
                params![relationship_id, buyer_user_id, Utc::now().to_rfc3339()],
            )
            .map_err(map_db("bind approved counterparty account"))?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn set_product_access_decision_tx(
    tx: &Connection,
    supplier_user_id: &str,
    supplier_email: &str,
    buyer_user_id: &str,
    buyer_email: &str,
    product_kind: &str,
    pricing_kind: &str,
    decision: &str,
    actor_user_id: &str,
    now: &str,
) -> Result<String, AppError> {
    if supplier_user_id == buyer_user_id {
        return Err(AppError::BadRequest(
            "cannot configure market access for your own account".into(),
        ));
    }
    let product_kind = normalize_product_kind(product_kind)?;
    let pricing_kind = normalize_pricing_kind(pricing_kind)?;
    let decision = normalize_decision(decision)?;
    if decision == DECISION_INHERIT {
        return Err(AppError::BadRequest(
            "an explicit allow or deny decision is required".into(),
        ));
    }
    let supplier_email = normalize_email(supplier_email)?;
    let buyer_email = normalize_email(buyer_email)?;
    let existing = relationship_for_buyer_tx(tx, supplier_user_id, buyer_user_id, &buyer_email)?;
    let relationship_id = existing
        .as_ref()
        .map(|relationship| relationship.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if existing.is_some() {
        tx.execute(
            "UPDATE market_counterparties
             SET supplier_email = ?2, buyer_user_id = ?3, buyer_email = ?4,
                 revision = revision + 1, updated_at = ?5
             WHERE id = ?1",
            params![
                relationship_id,
                supplier_email,
                buyer_user_id,
                buyer_email,
                now,
            ],
        )
        .map_err(map_db("update market access relationship"))?;
    } else {
        tx.execute(
            "INSERT INTO market_counterparties (
                id, supplier_user_id, supplier_email, buyer_user_id, buyer_email,
                status, revision, created_at, updated_at, revoked_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', 1, ?6, ?6, NULL)",
            params![
                relationship_id,
                supplier_user_id,
                supplier_email,
                buyer_user_id,
                buyer_email,
                now,
            ],
        )
        .map_err(map_db("create market access relationship"))?;
    }
    upsert_access_rule_tx(
        tx,
        &relationship_id,
        &AccessRuleInput {
            product_kind: product_kind.clone(),
            pricing_kind: pricing_kind.clone(),
            decision: decision.clone(),
        },
        now,
    )?;
    record_event_tx(
        tx,
        supplier_user_id,
        Some(&relationship_id),
        actor_user_id,
        "product_access_decision_updated",
        serde_json::json!({
            "productKind": product_kind,
            "pricingKind": pricing_kind,
            "decision": decision,
            "buyerEmail": buyer_email,
        }),
        now,
    )?;
    Ok(relationship_id)
}

pub(crate) fn effective_credit_grant_tx(
    conn: &Connection,
    supplier_user_id: &str,
    buyer_user_id: &str,
    buyer_email: &str,
    product_kind: &str,
    currency: &str,
) -> Result<EffectiveCreditGrant, AppError> {
    let product_kind = normalize_product_kind(product_kind)?;
    let currency = normalize_currency(currency)?;
    let buyer_email = normalize_email(buyer_email)?;
    let mode = access_mode_tx(conn, supplier_user_id, &product_kind, PRICING_PAID)?;
    if let Some((relationship_id, _, status)) =
        relationship_for_buyer_tx(conn, supplier_user_id, buyer_user_id, &buyer_email)?
    {
        if status != "active" {
            return Err(AppError::Forbidden(
                "seller revoked this market relationship".into(),
            ));
        }
        if let Some((kind, limit_minor, revision)) = conn
            .query_row(
                "SELECT kind, limit_minor, revision FROM market_credit_grants
                 WHERE counterparty_id = ?1 AND currency = ?2",
                params![relationship_id, currency],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read counterparty credit grant"))?
        {
            if kind == CREDIT_NONE {
                return Err(AppError::Forbidden(
                    "seller has not granted paid market credit in this currency".into(),
                ));
            }
            return Ok(EffectiveCreditGrant {
                kind,
                limit_minor,
                source: "counterparty".into(),
                revision,
            });
        }
    }
    if mode == MODE_BLACKLIST {
        if let Some((limit_minor, revision)) = conn
            .query_row(
                "SELECT limit_minor, revision FROM market_public_credit_policies
                 WHERE supplier_user_id = ?1 AND currency = ?2 AND enabled = 1",
                params![supplier_user_id, currency],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(map_db("read public credit policy"))?
        {
            return Ok(EffectiveCreditGrant {
                kind: CREDIT_LIMITED.into(),
                limit_minor: Some(limit_minor),
                source: "public".into(),
                revision,
            });
        }
    }
    Err(AppError::Forbidden(
        "seller has not granted paid market credit in this currency".into(),
    ))
}

#[cfg(test)]
pub(crate) fn configure_open_test_policy(
    conn: &Connection,
    supplier: &AuthSession,
    currency: &str,
    limit_minor: i64,
    now: &str,
) {
    for product_kind in [PRODUCT_SHARE, PRODUCT_CLIENT_HOST] {
        for pricing_kind in [PRICING_FREE, PRICING_PAID] {
            conn.execute(
                "INSERT INTO market_supplier_access_policies (
                    supplier_user_id, supplier_email, product_kind, pricing_kind, mode, revision,
                    risk_acknowledged_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, 'blacklist', 1, ?5, ?5, ?5)
                 ON CONFLICT(supplier_user_id, product_kind, pricing_kind)
                 DO UPDATE SET mode = 'blacklist'",
                params![
                    supplier.user_id,
                    supplier.email,
                    product_kind,
                    pricing_kind,
                    now
                ],
            )
            .expect("configure open test access policy");
        }
    }
    conn.execute(
        "INSERT INTO market_public_credit_policies (
            supplier_user_id, supplier_email, currency, enabled, limit_minor,
            revision, risk_acknowledged_at, created_at, updated_at
         ) VALUES (?1, ?2, ?3, 1, ?4, 1, ?5, ?5, ?5)
         ON CONFLICT(supplier_user_id, currency) DO UPDATE SET
            enabled = 1, limit_minor = excluded.limit_minor",
        params![supplier.user_id, supplier.email, currency, limit_minor, now],
    )
    .expect("configure public test credit");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access_connection() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory database");
        init_schema(&conn).expect("initialize market access schema");
        conn
    }

    #[test]
    fn legacy_unscoped_access_tables_migrate_to_both_pricing_scopes() {
        let conn = Connection::open_in_memory().expect("open legacy in-memory database");
        conn.execute_batch(
            "CREATE TABLE market_supplier_access_policies (
                supplier_user_id TEXT NOT NULL,
                supplier_email TEXT NOT NULL,
                product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
                mode TEXT NOT NULL CHECK (mode IN ('whitelist', 'blacklist')),
                revision INTEGER NOT NULL DEFAULT 1,
                risk_acknowledged_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (supplier_user_id, product_kind)
            );
            CREATE TABLE market_counterparty_access_rules (
                counterparty_id TEXT NOT NULL,
                product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
                decision TEXT NOT NULL CHECK (decision IN ('inherit', 'allow', 'deny')),
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (counterparty_id, product_kind)
            );
            INSERT INTO market_supplier_access_policies (
                supplier_user_id, supplier_email, product_kind, mode, revision,
                risk_acknowledged_at, created_at, updated_at
            ) VALUES
                ('supplier', 'supplier@example.com', 'share', 'whitelist', 7,
                 NULL, '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z'),
                ('supplier', 'supplier@example.com', 'client_host', 'blacklist', 3,
                 '2026-01-03T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-03T00:00:00Z');
            INSERT INTO market_counterparty_access_rules (
                counterparty_id, product_kind, decision, created_at, updated_at
            ) VALUES
                ('counterparty', 'share', 'allow',
                 '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z'),
                ('counterparty', 'client_host', 'deny',
                 '2026-01-01T00:00:00Z', '2026-01-03T00:00:00Z');",
        )
        .expect("initialize legacy access schema");

        init_schema(&conn).expect("migrate legacy access schema");

        let policy_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_supplier_access_policies",
                [],
                |row| row.get(0),
            )
            .expect("count migrated policies");
        assert_eq!(policy_count, 4);
        for pricing_kind in [PRICING_FREE, PRICING_PAID] {
            let share_policy: (String, i64, Option<String>, String, String) = conn
                .query_row(
                    "SELECT mode, revision, risk_acknowledged_at, created_at, updated_at
                     FROM market_supplier_access_policies
                     WHERE supplier_user_id = 'supplier' AND product_kind = 'share'
                       AND pricing_kind = ?1",
                    params![pricing_kind],
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
                .expect("read migrated Share policy");
            assert_eq!(
                share_policy,
                (
                    MODE_WHITELIST.into(),
                    7,
                    None,
                    "2026-01-01T00:00:00Z".into(),
                    "2026-01-02T00:00:00Z".into(),
                )
            );

            let client_host_rule: String = conn
                .query_row(
                    "SELECT decision FROM market_counterparty_access_rules
                     WHERE counterparty_id = 'counterparty' AND product_kind = 'client_host'
                       AND pricing_kind = ?1",
                    params![pricing_kind],
                    |row| row.get(0),
                )
                .expect("read migrated Client Host rule");
            assert_eq!(client_host_rule, DECISION_DENY);
        }

        assert_eq!(
            pricing_scope_rebuild_source(
                &conn,
                "market_supplier_access_policies",
                &["supplier_user_id", "product_kind", "pricing_kind"],
            )
            .expect("inspect migrated policy schema"),
            None
        );
        assert_eq!(
            pricing_scope_rebuild_source(
                &conn,
                "market_counterparty_access_rules",
                &["counterparty_id", "product_kind", "pricing_kind"],
            )
            .expect("inspect migrated rule schema"),
            None
        );

        init_schema(&conn).expect("rerun migrated access schema initialization");
        let policy_count_after_rerun: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_supplier_access_policies",
                [],
                |row| row.get(0),
            )
            .expect("count policies after rerun");
        assert_eq!(policy_count_after_rerun, 4);
        conn.execute(
            "INSERT INTO market_supplier_access_policies (
                supplier_user_id, supplier_email, product_kind, pricing_kind, mode,
                revision, created_at, updated_at
             ) VALUES ('supplier', 'supplier@example.com', 'share', 'free',
                       'whitelist', 8, '2026-01-01T00:00:00Z', '2026-01-04T00:00:00Z')
             ON CONFLICT(supplier_user_id, product_kind, pricing_kind)
             DO UPDATE SET revision = excluded.revision, updated_at = excluded.updated_at",
            [],
        )
        .expect("upsert migrated pricing-scoped policy");
        let revision: i64 = conn
            .query_row(
                "SELECT revision FROM market_supplier_access_policies
                 WHERE supplier_user_id = 'supplier' AND product_kind = 'share'
                   AND pricing_kind = 'free'",
                [],
                |row| row.get(0),
            )
            .expect("read updated migrated policy");
        assert_eq!(revision, 8);
    }

    #[test]
    fn access_defaults_are_split_and_preapproved_email_binds_per_scope() {
        let mut conn = access_connection();
        let now = Utc::now().to_rfc3339();
        assert!(
            product_access_allowed_tx(
                &conn,
                "supplier",
                "buyer",
                "buyer@example.com",
                PRODUCT_SHARE,
                PRICING_FREE,
            )
            .expect("check default free Share access")
        );
        assert!(
            !product_access_allowed_tx(
                &conn,
                "supplier",
                "buyer",
                "buyer@example.com",
                PRODUCT_SHARE,
                PRICING_PAID,
            )
            .expect("check default paid Share access")
        );
        conn.execute(
            "INSERT INTO market_counterparties (
                id, supplier_user_id, supplier_email, buyer_user_id, buyer_email,
                status, revision, created_at, updated_at
             ) VALUES ('relationship', 'supplier', 'supplier@example.com', NULL,
                       'buyer@example.com', 'active', 1, ?1, ?1)",
            params![now],
        )
        .expect("insert email-only counterparty");
        upsert_access_rule_tx(
            &conn,
            "relationship",
            &AccessRuleInput {
                product_kind: PRODUCT_SHARE.into(),
                pricing_kind: PRICING_PAID.into(),
                decision: DECISION_ALLOW.into(),
            },
            &now,
        )
        .expect("allow preapproved Share access");
        assert!(
            product_access_allowed_tx(
                &conn,
                "supplier",
                "buyer",
                "BUYER@example.com",
                PRODUCT_SHARE,
                PRICING_PAID,
            )
            .expect("check preapproved Share access")
        );

        let tx = conn.transaction().expect("begin account binding");
        ensure_product_access_tx(
            &tx,
            "supplier",
            "buyer",
            "buyer@example.com",
            PRODUCT_SHARE,
            PRICING_PAID,
        )
        .expect("bind preapproved account");
        tx.commit().expect("commit account binding");
        let bound_user_id: String = conn
            .query_row(
                "SELECT buyer_user_id FROM market_counterparties WHERE id = 'relationship'",
                [],
                |row| row.get(0),
            )
            .expect("read bound account");
        assert_eq!(bound_user_id, "buyer");
    }

    #[test]
    fn risky_open_policies_require_explicit_acknowledgement() {
        assert_eq!(default_access_mode(PRICING_FREE), MODE_BLACKLIST);
        assert_eq!(default_access_mode(PRICING_PAID), MODE_WHITELIST);
        assert!(validate_policy_transition(MODE_WHITELIST, MODE_BLACKLIST, false).is_err());
        validate_policy_transition(MODE_WHITELIST, MODE_BLACKLIST, true)
            .expect("acknowledge blacklist transition");
        validate_policy_transition(MODE_BLACKLIST, MODE_BLACKLIST, false)
            .expect("implicit free blacklist needs no acknowledgement");
        assert!(validate_public_credit_line(true, Some(10_000), false).is_err());
        assert!(validate_public_credit_line(true, None, true).is_err());
        assert_eq!(
            validate_public_credit_line(true, Some(10_000), true)
                .expect("acknowledge finite public credit"),
            Some(10_000)
        );
        assert_eq!(
            validate_public_credit_line(false, Some(10_000), false)
                .expect("disabled public credit ignores stale input"),
            None
        );
    }

    #[test]
    fn product_decision_update_does_not_reactivate_a_revoked_relationship() {
        let conn = access_connection();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO market_counterparties (
                id, supplier_user_id, supplier_email, buyer_user_id, buyer_email,
                status, revision, created_at, updated_at, revoked_at
             ) VALUES ('relationship', 'supplier', 'supplier@example.com', 'buyer',
                       'buyer@example.com', 'revoked', 2, ?1, ?1, ?1)",
            params![now],
        )
        .expect("insert revoked counterparty");

        set_product_access_decision_tx(
            &conn,
            "supplier",
            "supplier@example.com",
            "buyer",
            "buyer@example.com",
            PRODUCT_SHARE,
            PRICING_FREE,
            DECISION_DENY,
            "supplier",
            &now,
        )
        .expect("deny future Share access");

        let relationship: (String, Option<String>, i64) = conn
            .query_row(
                "SELECT status, revoked_at, revision FROM market_counterparties
                 WHERE id = 'relationship'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read preserved counterparty state");
        assert_eq!(relationship, ("revoked".into(), Some(now), 3));
        assert!(
            !product_access_allowed_tx(
                &conn,
                "supplier",
                "buyer",
                "buyer@example.com",
                PRODUCT_SHARE,
                PRICING_FREE,
            )
            .expect("keep revoked Share access denied")
        );
    }

    #[test]
    fn explicit_and_public_credit_grants_are_resolved_without_public_unlimited_credit() {
        let conn = access_connection();
        let now = Utc::now().to_rfc3339();
        let relationship_id = set_product_access_decision_tx(
            &conn,
            "supplier",
            "supplier@example.com",
            "buyer",
            "buyer@example.com",
            PRODUCT_CLIENT_HOST,
            PRICING_PAID,
            DECISION_ALLOW,
            "supplier",
            &now,
        )
        .expect("create trusted buyer");
        upsert_credit_line_tx(
            &conn,
            &relationship_id,
            &CreditLineInput {
                currency: "USD".into(),
                kind: CREDIT_LIMITED.into(),
                limit_minor: Some(25_000),
                risk_acknowledged: false,
            },
            Some(0),
            &now,
        )
        .expect("grant limited credit");
        let grant = effective_credit_grant_tx(
            &conn,
            "supplier",
            "buyer",
            "buyer@example.com",
            PRODUCT_CLIENT_HOST,
            "USD",
        )
        .expect("resolve limited credit");
        assert_eq!(grant.kind, CREDIT_LIMITED);
        assert_eq!(grant.limit_minor, Some(25_000));
        assert_eq!(grant.source, "counterparty");
        assert!(validate_credit_line(CREDIT_UNLIMITED, None, false).is_err());
        assert_eq!(
            validate_credit_line(CREDIT_UNLIMITED, Some(99), true)
                .expect("acknowledge unlimited private credit"),
            (CREDIT_UNLIMITED.into(), None)
        );

        conn.execute(
            "INSERT INTO market_supplier_access_policies (
                supplier_user_id, supplier_email, product_kind, pricing_kind, mode, revision,
                risk_acknowledged_at, created_at, updated_at
             ) VALUES ('public-supplier', 'public@example.com', 'share', 'paid', 'blacklist',
                       1, ?1, ?1, ?1)",
            params![now],
        )
        .expect("enable acknowledged blacklist policy");
        conn.execute(
            "INSERT INTO market_public_credit_policies (
                supplier_user_id, supplier_email, currency, enabled, limit_minor,
                revision, risk_acknowledged_at, created_at, updated_at
             ) VALUES ('public-supplier', 'public@example.com', 'CNY', 1, 5000,
                       1, ?1, ?1, ?1)",
            params![now],
        )
        .expect("configure finite public credit");
        let public_grant = effective_credit_grant_tx(
            &conn,
            "public-supplier",
            "unknown-buyer",
            "unknown@example.com",
            PRODUCT_SHARE,
            "CNY",
        )
        .expect("resolve finite public credit");
        assert_eq!(public_grant.kind, CREDIT_LIMITED);
        assert_eq!(public_grant.limit_minor, Some(5_000));
        assert_eq!(public_grant.source, "public");
    }

    #[test]
    fn revoking_counterparty_credit_only_updates_matching_accounts() {
        let conn = access_connection();
        crate::market_billing::init_schema(&conn).expect("initialize market billing schema");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO market_counterparties (
                id, supplier_user_id, supplier_email, buyer_user_id, buyer_email,
                status, revision, created_at, updated_at
             ) VALUES ('relationship', 'supplier', 'supplier@example.com', 'buyer',
                       'buyer@example.com', 'revoked', 2, ?1, ?1)",
            params![now],
        )
        .expect("insert revoked counterparty");
        for (id, buyer_user_id, buyer_email) in [
            ("matching", "buyer", "buyer@example.com"),
            ("other", "other-buyer", "other@example.com"),
        ] {
            conn.execute(
                "INSERT INTO market_credit_accounts (
                    id, buyer_user_id, buyer_email, supplier_user_id, supplier_email,
                    currency, status, balance_units, credit_kind, credit_limit_minor,
                    credit_source, credit_revision, version, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'supplier', 'supplier@example.com', 'USD',
                           'active', 0, 'limited', 10000, 'counterparty', 4, 1, ?4, ?4)",
                params![id, buyer_user_id, buyer_email, now],
            )
            .expect("insert credit account");
        }

        assert_eq!(
            revoke_counterparty_credit_accounts_tx(&conn, "supplier", "relationship", &now)
                .expect("revoke matching credit"),
            1
        );
        let matching: (String, Option<i64>, i64) = conn
            .query_row(
                "SELECT credit_kind, credit_limit_minor, credit_revision
                 FROM market_credit_accounts WHERE id = 'matching'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read revoked account");
        assert_eq!(matching, (CREDIT_NONE.into(), None, 5));
        let other: String = conn
            .query_row(
                "SELECT credit_kind FROM market_credit_accounts WHERE id = 'other'",
                [],
                |row| row.get(0),
            )
            .expect("read unrelated account");
        assert_eq!(other, CREDIT_LIMITED);
    }

    #[test]
    fn applying_private_credit_refreshes_an_existing_public_account() {
        let conn = access_connection();
        crate::market_billing::init_schema(&conn).expect("initialize market billing schema");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO market_counterparties (
                id, supplier_user_id, supplier_email, buyer_user_id, buyer_email,
                status, revision, created_at, updated_at
             ) VALUES ('relationship', 'supplier', 'supplier@example.com', 'buyer',
                       'buyer@example.com', 'active', 1, ?1, ?1)",
            params![now],
        )
        .expect("insert active counterparty");
        conn.execute(
            "INSERT INTO market_credit_accounts (
                id, buyer_user_id, buyer_email, supplier_user_id, supplier_email,
                currency, status, balance_units, credit_kind, credit_limit_minor,
                credit_source, credit_revision, version, created_at, updated_at
             ) VALUES ('account', 'buyer', 'buyer@example.com', 'supplier',
                       'supplier@example.com', 'USD', 'active', 0, 'limited', 5000,
                       'public', 3, 1, ?1, ?1)",
            params![now],
        )
        .expect("insert existing public credit account");
        upsert_credit_line_tx(
            &conn,
            "relationship",
            &CreditLineInput {
                currency: "USD".into(),
                kind: CREDIT_UNLIMITED.into(),
                limit_minor: None,
                risk_acknowledged: true,
            },
            Some(0),
            &now,
        )
        .expect("grant private unlimited credit");

        let grant = apply_counterparty_credit_line_to_accounts_tx(
            &conn,
            "supplier",
            "relationship",
            "USD",
            &now,
        )
        .expect("apply private credit to existing account");
        assert_eq!(grant, (CREDIT_UNLIMITED.into(), None, 1));
        let account: (String, Option<i64>, String, i64) = conn
            .query_row(
                "SELECT credit_kind, credit_limit_minor, credit_source, credit_revision
                 FROM market_credit_accounts WHERE id = 'account'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read refreshed credit account");
        assert_eq!(
            account,
            (CREDIT_UNLIMITED.into(), None, "counterparty".into(), 1)
        );
    }

    #[test]
    fn update_requests_require_expected_revision() {
        assert!(
            serde_json::from_value::<UpdatePolicyRequest>(serde_json::json!({
                "mode": "whitelist"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<UpdateCounterpartyRequest>(serde_json::json!({
                "accessRules": []
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<UpdateCreditLineRequest>(serde_json::json!({
                "kind": "none"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<UpdatePublicCreditLineRequest>(serde_json::json!({
                "enabled": false
            }))
            .is_err()
        );
    }

    #[test]
    fn counterparty_email_uses_deliverable_address_validation() {
        assert_eq!(
            normalize_email(" Buyer+Market@Example.com ").expect("normalize valid email"),
            "buyer+market@example.com"
        );
        for invalid in [
            "buyer@",
            "buyer@example",
            "buyer@@example.com",
            "买家@example.com",
        ] {
            assert!(normalize_email(invalid).is_err(), "accepted {invalid}");
        }
    }
}
