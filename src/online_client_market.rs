//! Online Client supply. Installation IDs address the physical control plane.
//! Lease IDs scope every customer URL, wallet contract and cleanup decision.
use crate::market_platform_contracts as wallet;
use crate::{
    ServerState,
    db::{Connection, OptionalExtension, Transaction, params},
    error::AppError,
    models::AuthSession,
};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::{Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

mod lifecycle;
mod reconcile;
pub(crate) use reconcile::run;
mod seller;
mod shares;
mod upgrades;
pub(crate) use shares::{
    allow_traffic, bind_share, check_nonce, check_share_id, namespace, share_owner,
};

trait Database<T> {
    fn market(self) -> Result<T, AppError>;
}
impl<T> Database<T> for crate::db::Result<T> {
    fn market(self) -> Result<T, AppError> {
        self.map_err(|e| AppError::Internal(format!("online Client Market database: {e}")))
    }
}
type Reply = Result<Json<Value>, AppError>;
fn conflict(s: &str) -> AppError {
    AppError::Conflict(s.into())
}
fn missing() -> AppError {
    AppError::NotFound("online Client rental not found".into())
}
async fn actor(s: &ServerState, h: &HeaderMap) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(s, h)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}
pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/client-market/online/listings", get(listings))
        .route(
            "/v1/client-market/online/installations/:id/preflight",
            post(preflight),
        )
        .route(
            "/v1/client-market/online/installations/:id/verify-provider",
            post(verify_listing_provider),
        )
        .route(
            "/v1/client-market/online/installations/:id/retry",
            post(retry),
        )
        .route(
            "/v1/client-market/online/installations/:id/listing",
            get(listing_status)
                .post(list)
                .delete(unlist)
                .patch(seller::update_listing),
        )
        .route(
            "/v1/client-market/online/installations/:id/listing/pause",
            post(seller::pause),
        )
        .route(
            "/v1/client-market/online/installations/:id/listing/resume",
            post(seller::resume),
        )
        .route(
            "/v1/client-market/online/installations/:id/quote",
            post(quote),
        )
        .route("/v1/client-market/online/quotes/:id/commit", post(commit))
        .route("/v1/client-market/online/leases", get(mine))
        .route(
            "/v1/client-market/online/seller-inventory",
            get(seller_inventory),
        )
        .route(
            "/v1/client-market/online/leases/:id/upgrade",
            post(upgrades::start),
        )
        .route(
            "/v1/client-market/online/leases/:id/upgrade/status",
            get(upgrades::status),
        )
        .route("/v1/client-market/online/leases/:id/end", post(end))
        .route(
            "/v1/client-market/online/leases/:id/reclaim",
            post(seller::reclaim),
        )
        .route(
            "/v1/client-market/online/leases/:id/reclaim/cancel",
            post(seller::cancel_reclaim),
        )
        .route("/v1/client-market/online/egress/:challenge", post(egress))
        .layer(axum::middleware::from_fn(
            crate::client_market_audit::private_response,
        ))
}
#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Hardware {
    arch: String,
    cpu: Option<f64>,
    memory_bytes: Option<f64>,
    disk_bytes: Option<f64>,
}
impl Hardware {
    fn validate(&self) -> Result<(), AppError> {
        if !matches!(self.arch.as_str(), "x86_64" | "aarch64")
            || [self.cpu, self.memory_bytes, self.disk_bytes]
                .into_iter()
                .flatten()
                .any(|x| !x.is_finite() || x <= 0.0 || x > 9_007_199_254_740_991.0)
        {
            return Err(conflict("invalid hardware snapshot"));
        }
        Ok(())
    }
}
#[derive(Deserialize, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Grant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    buyer_id: Option<String>,
    lease_id: String,
    tenant_email: String,
    alias: String,
    expires_at: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientStatus {
    #[serde(default)]
    exclusive_management: bool,
    #[serde(default)]
    upgrade_busy: bool,
    awaiting_return: Option<String>,
    capability: String,
    #[serde(default)]
    owner_rental: bool,
    owner_email: String,
    listed_owner: Option<String>,
    sequence: i64,
    grant: Option<Grant>,
    running_lease: Option<String>,
    switching: bool,
    fenced: bool,
    cleanup_safe: bool,
    hardware: Option<Hardware>,
    events: Vec<AccessEvent>,
    version: String,
}
#[derive(Deserialize)]
struct AccessEvent {
    #[serde(default)]
    occurrences: Option<u32>,
    id: String,
    kind: String,
    at: i64,
}
async fn rpc(s: &ServerState, id: &str, path: &str, body: Value) -> Result<ClientStatus, AppError> {
    let timeout = match path {
        "/_ctl/market/command" => 15,
        _ => 3,
    };
    let status: ClientStatus = control(s, id, path, body, timeout).await?;
    if status.capability != "marketLeaseV1" {
        return Err(conflict("Client does not support marketLeaseV1"));
    }
    Ok(status)
}
pub(crate) async fn management_protected(s: &ServerState, id: &str) -> Result<bool, AppError> {
    let c = s.store.conn.lock().await;
    c.query_row("SELECT EXISTS(SELECT 1 FROM online_client_protection WHERE installation_id=?1) OR EXISTS(SELECT 1 FROM online_client_listings WHERE installation_id=?1 AND unlist_pending=1)",
        params![id],|r|r.get(0)).market()
}
pub(crate) async fn control<T: serde::de::DeserializeOwned>(
    s: &ServerState,
    id: &str,
    path: &str,
    body: Value,
    timeout: u64,
) -> Result<T, AppError> {
    let route = s
        .proxy
        .active_client_route_for_installation(id)
        .await
        .ok_or_else(|| AppError::ServiceUnavailable("Client is offline".into()))?;
    let secret = s
        .store
        .installation_control_secret(id)
        .await?
        .ok_or_else(|| conflict("missing Client control identity"))?;
    crate::ctl_client::post_control_with_timeout(
        route.route_target(),
        id,
        &secret,
        path,
        body.to_string(),
        std::time::Duration::from_secs(timeout),
    )
    .await
    .map_err(|error| {
        AppError::ServiceUnavailable(format!(
            "Client did not confirm marketLeaseV1 operation: {error}"
        ))
    })
}
fn stamp() -> i64 {
    Utc::now().timestamp()
}
fn queue(c: &Connection, id: &str, seq: i64, action: Value) -> Result<(), AppError> {
    c.execute("INSERT INTO online_client_operations(installation_id,sequence,request_json,next_attempt) VALUES(?1,?2,?3,0)
        ON CONFLICT(installation_id) DO UPDATE SET sequence=excluded.sequence,request_json=excluded.request_json,next_attempt=0,attempts=0,delivery_attempts=0,waiting_reconnect=0,last_attempt_at=NULL,last_error=NULL",
        params![id,seq,json!({"sequence":seq,"action":action}).to_string()]).market()?;
    Ok(())
}
fn advance(c: &Connection, id: &str) -> Result<i64, AppError> {
    c.query_row("UPDATE online_client_listings SET sequence=sequence+1,updated_at=unixepoch() WHERE installation_id=?1 RETURNING sequence",params![id],|r|r.get(0)).market()
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Offer {
    cycle_price_minor: i64,
    free_days: Option<u32>,
    #[serde(default)]
    trial_days: u32,
    #[serde(default)]
    title: Option<String>,
    acknowledged: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Readiness {
    owner_verified: bool,
    activated: bool,
    capable: bool,
    share_clear: bool,
    unlisted: bool,
    #[serde(skip_serializing)]
    host_sync_pending: bool,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ListingBlocker {
    AlreadyListed,
    ListingChanged,
    OwnerUnverified,
    Inactive,
    UpgradeRequired,
    Offline,
    ShareInUse,
    WorkspaceBusy,
    ControlUnavailable,
    EgressUnverified,
    HardwareUnavailable,
    ProviderVerification,
    EmailVerification,
    ProviderPaused,
    ProviderConflict,
}

impl ListingBlocker {
    fn error(self) -> AppError {
        use ListingBlocker::*;
        let (status, message) = match self {
            AlreadyListed => (
                StatusCode::CONFLICT,
                "Client already listed or being listed",
            ),
            ListingChanged => (
                StatusCode::CONFLICT,
                "the listing changed during verification",
            ),
            OwnerUnverified => (
                StatusCode::FORBIDDEN,
                "verified installation owner required",
            ),
            Inactive => (
                StatusCode::CONFLICT,
                "activate this Client and enable its tunnel",
            ),
            UpgradeRequired => (
                StatusCode::CONFLICT,
                "upgrade this Client to support exclusive rentals",
            ),
            Offline => (StatusCode::CONFLICT, "Client must be online"),
            ShareInUse => (
                StatusCode::CONFLICT,
                "finish existing Share Market listings and seat rentals first",
            ),
            WorkspaceBusy => (StatusCode::CONFLICT, "Client workspace is busy"),
            ControlUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Client did not confirm the check",
            ),
            EgressUnverified => (StatusCode::CONFLICT, "Client egress could not be verified"),
            HardwareUnavailable => (
                StatusCode::CONFLICT,
                "Client hardware report is unavailable",
            ),
            ProviderVerification => (
                StatusCode::FORBIDDEN,
                "a verified Provider account is required for paid rentals",
            ),
            EmailVerification => (
                StatusCode::FORBIDDEN,
                "sign in with a verified account email",
            ),
            ProviderPaused => (
                StatusCode::FORBIDDEN,
                "paid rentals are paused for this Provider",
            ),
            ProviderConflict => (StatusCode::CONFLICT, "conflicting Market Provider identity"),
        };
        AppError::Coded {
            status,
            code: "CLIENT_LISTING_BLOCKED",
            message: message.into(),
            details: json!({"reason": self}),
        }
    }
}

impl Readiness {
    fn initial_blocker(&self) -> Option<ListingBlocker> {
        (!self.unlisted)
            .then_some(ListingBlocker::AlreadyListed)
            .or_else(|| self.seller_blocker())
    }

    fn seller_blocker(&self) -> Option<ListingBlocker> {
        if !self.owner_verified {
            Some(ListingBlocker::OwnerUnverified)
        } else if !self.activated {
            Some(ListingBlocker::Inactive)
        } else if !self.capable {
            Some(ListingBlocker::UpgradeRequired)
        } else if !self.share_clear {
            Some(ListingBlocker::ShareInUse)
        } else if self.host_sync_pending {
            Some(ListingBlocker::ControlUnavailable)
        } else {
            None
        }
    }
}

impl ClientStatus {
    fn workspace_blocker(&self, owner: &str) -> Option<ListingBlocker> {
        if self.capability != "marketLeaseV1" || !self.exclusive_management {
            Some(ListingBlocker::UpgradeRequired)
        } else if !self.owner_email.eq_ignore_ascii_case(owner) {
            Some(ListingBlocker::OwnerUnverified)
        } else if self.grant.is_some()
            || self.running_lease.is_some()
            || self.awaiting_return.is_some()
            || self.switching
            || self.upgrade_busy
            || !self.cleanup_safe
        {
            Some(ListingBlocker::WorkspaceBusy)
        } else {
            None
        }
    }
}

async fn probe_listing(
    s: &ServerState,
    id: &str,
    challenge: &str,
) -> Result<ClientStatus, AppError> {
    control(
        s,
        id,
        "/_ctl/market/check",
        json!({"challenge": challenge}),
        10,
    )
    .await
}

fn paid_listing_blocker(
    c: &Connection,
    a: &AuthSession,
) -> Result<Option<ListingBlocker>, AppError> {
    match crate::market_provider_finance::require_paid_offer_setup_tx(c, &a.user_id) {
        Ok(()) => return Ok(None),
        Err(AppError::Forbidden(_)) => (),
        Err(e) => return Err(e),
    }
    let account: Option<(bool, Option<String>)> = c
        .query_row(
            "SELECT u.email_verified_at IS NOT NULL, p.status FROM users u
         LEFT JOIN market_provider_profiles p ON p.user_id=u.id
         WHERE u.id=?1 AND u.email_normalized=lower(trim(?2)) AND u.status='active'",
            params![a.user_id, a.email],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .market()?;
    Ok(Some(match account {
        Some((true, status)) => match status.as_deref() {
            Some("paused") => ListingBlocker::ProviderPaused,
            Some("retired" | "identity_conflict") => ListingBlocker::ProviderConflict,
            _ => ListingBlocker::ProviderVerification,
        },
        _ => ListingBlocker::EmailVerification,
    }))
}

fn readiness(c: &Connection, id: &str, email: &str) -> Result<Readiness, AppError> {
    c.query_row("SELECT i.owner_verified_at IS NOT NULL,
        i.lifecycle='active' AND i.client_activated_at IS NOT NULL AND coalesce(t.enabled,0)=1,
        i.market_lease_v1=1, NOT EXISTS(SELECT 1 FROM share_market_listings l WHERE l.installation_id=i.id AND
            (l.status='active' OR EXISTS(SELECT 1 FROM share_market_subscriptions sub WHERE sub.listing_id=l.id AND sub.released_at IS NULL))),
        NOT EXISTS(SELECT 1 FROM online_client_listings l WHERE l.installation_id=i.id AND l.state!='unlisted'
            AND NOT (l.state='locking' AND l.check_json IS NULL AND l.challenge_until IS NOT NULL AND l.challenge_until<unixepoch())),
        EXISTS(SELECT 1 FROM online_client_listings l WHERE l.installation_id=i.id AND l.unlist_pending=1)
        FROM installations i LEFT JOIN installation_client_tunnels t ON t.installation_id=i.id
        WHERE i.id=?1 AND lower(trim(i.owner_email))=lower(?2)",params![id,email],
        |r|Ok(Readiness {owner_verified:r.get(0)?,activated:r.get(1)?,capable:r.get(2)?,share_clear:r.get(3)?,unlisted:r.get(4)?,host_sync_pending:r.get(5)?}))
        .optional().market()?.ok_or_else(missing)
}
fn listing_provider_transaction<'c>(
    c: &'c Connection,
    id: &str,
    a: &AuthSession,
    paid: bool,
) -> Result<Transaction<'c>, AppError> {
    let tx = c.transaction().market()?;
    if let Some(reason) = readiness(&tx, id, &a.email)?.initial_blocker() {
        return Err(reason.error());
    }
    if paid {
        return ensure_paid_provider(c, tx, a);
    }
    let now = Utc::now().to_rfc3339();
    if let crate::market_provider_identity::MarketProviderIdentityResolution::Conflict(conflict) =
        crate::market_provider_identity::ensure_market_provider_identity_tx(
            &tx,
            Some(&a.user_id),
            &a.email,
            &now,
        )?
    {
        drop(tx);
        crate::market_provider_identity::persist_market_provider_identity_conflict(
            c, &conflict, &now,
        )?;
        return Err(ListingBlocker::ProviderConflict.error());
    }
    Ok(tx)
}

/// Paid offers need the seller's provider identity. A first paid offer claims
/// it here; any other paid blocker (email, paused, conflict) is returned.
fn ensure_paid_provider<'c>(
    c: &'c Connection,
    tx: Transaction<'c>,
    a: &AuthSession,
) -> Result<Transaction<'c>, AppError> {
    let needs_claim = match paid_listing_blocker(&tx, a)? {
        None => false,
        Some(ListingBlocker::ProviderVerification) => true,
        Some(reason) => return Err(reason.error()),
    };
    let now = Utc::now().to_rfc3339();
    if let crate::market_provider_identity::MarketProviderIdentityResolution::Conflict(conflict) =
        crate::market_provider_identity::ensure_market_provider_identity_tx(
            &tx,
            Some(&a.user_id),
            &a.email,
            &now,
        )?
    {
        drop(tx);
        crate::market_provider_identity::persist_market_provider_identity_conflict(
            c, &conflict, &now,
        )?;
        return Err(ListingBlocker::ProviderConflict.error());
    }
    if needs_claim && let Some(reason) = paid_listing_blocker(&tx, a)? {
        return Err(reason.error());
    }
    Ok(tx)
}

async fn verify_listing_provider(
    State(s): State<ServerState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    listing_provider_transaction(&c, &id, &a, true)?
        .commit()
        .market()?;
    Ok(Json(json!({"paidReady": true})))
}

#[derive(Deserialize)]
struct Preflight {
    paid: bool,
}

async fn preflight(
    State(s): State<ServerState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(input): Json<Preflight>,
) -> Reply {
    let a = actor(&s, &h).await?;
    let online = s
        .proxy
        .active_client_route_for_installation(&id)
        .await
        .is_some();
    let (checks, paid_blocker) = {
        let c = s.store.conn.lock().await;
        let checks = readiness(&c, &id, &a.email)?;
        (checks, paid_listing_blocker(&c, &a)?)
    };
    let blocker = checks
        .initial_blocker()
        .or((!online).then_some(ListingBlocker::Offline))
        .or(paid_blocker.filter(|reason| matches!(reason, ListingBlocker::ProviderConflict)));
    // Keep the original flags for older dashboards. A skipped live check is
    // distinguishable from an attempted one even when its flags are all false.
    let mut result = json!({
        "checks": checks, "online": online, "paidReady": paid_blocker.is_none(), "paidBlocker": paid_blocker, "workspace": false,
        "hardware": false, "egress": false, "ready": false, "blocker": blocker,
        "probeStatus": "skipped", "countryCode": null,
    });
    if blocker.is_some() {
        return Ok(Json(result));
    }
    let challenge = Uuid::new_v4().simple().to_string();
    {
        let c = s.store.conn.lock().await;
        c.execute(
            "DELETE FROM online_client_preflight WHERE expires_at<unixepoch()",
            [],
        )
        .market()?;
        c.execute("INSERT INTO online_client_preflight(challenge,installation_id,expires_at) VALUES(?1,?2,unixepoch()+30)",params![challenge,id]).market()?;
    }
    let status = probe_listing(&s, &id, &challenge).await;
    let c = s.store.conn.lock().await;
    let (ip, country): (Option<String>, Option<String>) = c.query_row(
        "DELETE FROM online_client_preflight WHERE challenge=?1 AND expires_at>=unixepoch() RETURNING egress_ip,country_code",
        params![challenge], |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional().market()?.unwrap_or_default();
    let egress = ip.is_some();
    result["egress"] = json!(egress);
    result["egressIp"] = json!(ip);
    result["countryCode"] = json!(country);
    match status {
        Ok(st) => {
            result["probeStatus"] = json!("completed");
            result["exclusiveManagement"] = json!(st.exclusive_management);
            let workspace_blocker = st.workspace_blocker(&a.email);
            let hardware = st.hardware.as_ref().is_some_and(|h| h.validate().is_ok());
            if hardware {
                result["resource"] = json!(st.hardware);
            }
            result["workspace"] = json!(workspace_blocker.is_none());
            result["hardware"] = json!(hardware);
            let blocker = workspace_blocker
                .or((!hardware).then_some(ListingBlocker::HardwareUnavailable))
                .or((!egress).then_some(ListingBlocker::EgressUnverified))
                .or(if input.paid { paid_blocker } else { None });
            result["ready"] = json!(blocker.is_none());
            result["blocker"] = json!(blocker);
        }
        Err(AppError::ServiceUnavailable(_) | AppError::Conflict(_)) => {
            result["probeStatus"] = json!("unreachable");
            result["blocker"] = json!(ListingBlocker::ControlUnavailable);
        }
        Err(error) => return Err(error),
    }
    Ok(Json(result))
}

async fn retry(State(s): State<ServerState>, Path(id): Path<String>, headers: HeaderMap) -> Reply {
    let a = actor(&s, &headers).await?;
    let c = s.store.conn.lock().await;
    let rows=c.execute("UPDATE online_client_operations SET next_attempt=0 WHERE installation_id=?1 AND EXISTS
      (SELECT 1 FROM online_client_listings l WHERE l.installation_id=?1 AND l.seller_id=?2 AND (l.state='locking' OR (l.unlist_pending=1 AND json_extract(online_client_operations.request_json,'$.action.operation')='unlist')) AND l.sequence=online_client_operations.sequence)",params![id,a.user_id]).market()?;
    if rows != 1 {
        return Err(conflict("no pending listing operation"));
    }
    Ok(Json(json!({"queued":true})))
}
async fn list(
    State(s): State<ServerState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(v): Json<Offer>,
) -> Reply {
    let a = actor(&s, &h).await?;
    if !v.acknowledged {
        return Err(AppError::BadRequest(
            "invalid Client rental terms or missing acknowledgement".into(),
        ));
    }
    seller::validate_terms(v.cycle_price_minor, v.free_days, v.trial_days)?;
    let title = seller::normalize_title(v.title.as_deref())?;
    if s.proxy
        .active_client_route_for_installation(&id)
        .await
        .is_none()
    {
        let c = s.store.conn.lock().await;
        if let Some(reason) = readiness(&c, &id, &a.email)?.initial_blocker() {
            return Err(reason.error());
        }
        return Err(ListingBlocker::Offline.error());
    }
    let challenge = Uuid::new_v4().simple().to_string();
    {
        let c = s.store.conn.lock().await;
        let tx = listing_provider_transaction(&c, &id, &a, v.cycle_price_minor > 0)?;
        let changed=tx.execute("INSERT INTO online_client_listings(installation_id,seller_id,owner_email,state,revision,sequence,cycle_price_minor,free_days,trial_days,title,updated_at,challenge,challenge_until)
            VALUES(?1,?2,lower(?3),'locking',1,1,?4,?5,?6,?8,unixepoch(),?7,unixepoch()+30)
            ON CONFLICT(installation_id) DO UPDATE SET state='locking',revision=revision+1,sequence=sequence+1,seller_id=excluded.seller_id,owner_email=excluded.owner_email,
            cycle_price_minor=excluded.cycle_price_minor,free_days=excluded.free_days,trial_days=excluded.trial_days,title=excluded.title,paused_at=NULL,challenge=excluded.challenge,challenge_until=excluded.challenge_until,egress_ip=NULL,country_code=NULL,check_json=NULL,healthy=0,egress_checked_at=NULL,unlist_pending=0,unlisted_at=NULL
            WHERE state='unlisted' OR (state='locking' AND challenge_until<unixepoch() AND check_json IS NULL)",
            params![id,a.user_id,a.email,v.cycle_price_minor,v.free_days,v.trial_days,challenge,title]).market()?;
        if changed != 1 {
            return Err(ListingBlocker::AlreadyListed.error());
        }
        tx.commit().market()?;
    }
    let checked = probe_listing(&s, &id, &challenge).await;
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    tx.execute_batch("SAVEPOINT verified_listing").market()?;
    let result = (|| {
        if let Some(reason) = readiness(&tx, &id, &a.email)?.seller_blocker() {
            return Err(reason.error());
        }
        let st = checked.map_err(|err| match err {
            AppError::ServiceUnavailable(_) | AppError::Conflict(_) => {
                ListingBlocker::ControlUnavailable.error()
            }
            err => err,
        })?;
        if let Some(reason) = st.workspace_blocker(&a.email) {
            return Err(reason.error());
        }
        let hardware = st
            .hardware
            .filter(|h| h.validate().is_ok())
            .ok_or_else(|| ListingBlocker::HardwareUnavailable.error())?;
        let snapshot = json!({"exclusiveManagement":st.exclusive_management, "ownerRental":st.owner_rental,
            "hardware":hardware, "version":st.version.chars().take(80).collect::<String>()});
        let (sequence, egress_verified): (i64, bool) = tx.query_row(
            "UPDATE online_client_listings SET sequence=max(sequence,?3+1),challenge=NULL,
             challenge_until=NULL,check_json=?4,hardware_at=unixepoch(),egress_checked_at=unixepoch()
             WHERE installation_id=?1 AND challenge=?2 AND challenge_until>=unixepoch() AND state='locking'
             RETURNING sequence,egress_ip IS NOT NULL",
            params![id, challenge, st.sequence, snapshot.to_string()], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional().market()?.ok_or_else(|| ListingBlocker::ListingChanged.error())?;
        if !egress_verified {
            return Err(ListingBlocker::EgressUnverified.error());
        }
        if v.cycle_price_minor > 0
            && let Some(reason) = paid_listing_blocker(&tx, &a)?
        {
            return Err(reason.error());
        }
        queue(
            &tx,
            &id,
            sequence,
            json!({"operation":"list","ownerEmail":a.email}),
        )?;
        Ok(())
    })();
    if let Err(e) = result {
        tx.execute_batch("ROLLBACK TO verified_listing; RELEASE verified_listing")
            .market()?;
        tx.execute("UPDATE online_client_listings SET state='unlisted',challenge=NULL WHERE installation_id=?1 AND challenge=?2",params![id,challenge]).market()?;
        tx.commit().market()?;
        return Err(e);
    }
    tx.execute_batch("RELEASE verified_listing").market()?;
    tx.commit().market()?;
    Ok(Json(json!({"state":"locking"})))
}

async fn egress(
    State(s): State<ServerState>,
    Path(challenge): Path<String>,
    h: HeaderMap,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
) -> Result<StatusCode, AppError> {
    if challenge.len() != 32 || !challenge.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(missing());
    }
    let meta = crate::client_meta::extract_client_metadata(&h, peer);
    let ip = meta
        .ip
        .ok_or_else(|| conflict("unavailable network source"))?;
    let valid: bool = {
        let c = s.store.conn.lock().await;
        c.query_row("SELECT EXISTS(SELECT 1 FROM online_client_listings WHERE challenge=?1 AND challenge_until>=unixepoch() AND egress_ip IS NULL UNION ALL SELECT 1 FROM online_client_preflight WHERE challenge=?1 AND expires_at>=unixepoch() AND egress_ip IS NULL)",params![challenge],|r|r.get(0)).market()?
    };
    if !valid {
        return Err(missing());
    }
    let country = match meta.country_code {
        Some(v) => Some(v),
        None => s.store.lookup_geo_country_code_for_ip(&ip).await,
    };
    let c = s.store.conn.lock().await;
    if c.execute("UPDATE online_client_listings SET egress_ip=?2,country_code=?3 WHERE challenge=?1 AND challenge_until>=unixepoch() AND egress_ip IS NULL",params![challenge,ip,country]).market()?!=1 && c.execute("UPDATE online_client_preflight SET egress_ip=?2,country_code=?3 WHERE challenge=?1 AND expires_at>=unixepoch() AND egress_ip IS NULL",params![challenge,ip,country]).market()?!=1{return Err(missing())}
    Ok(StatusCode::NO_CONTENT)
}
async fn unlist(State(s): State<ServerState>, Path(id): Path<String>, h: HeaderMap) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    let owned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM online_client_listings WHERE installation_id=?1 AND seller_id=?2)",params![id,a.user_id],|r|r.get(0)).market()?;
    if !owned {
        return Err(missing());
    }
    lifecycle::request_unlist(&tx, &id, stamp())?;
    tx.commit().market()?;
    Ok(Json(json!({"state":"unlisted"})))
}

/// Owner-only intent/delivery status, available even after runtime GC. It never
/// brings an unlisted Client back into the public or current selling inventory.
async fn listing_status(
    State(s): State<ServerState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Reply {
    let a = actor(&s, &h).await?;
    let (state, pending, rented, unlisted_at, attempts, last_error, last_attempt) = {
        let c = s.store.conn.lock().await;
        c.query_row("SELECT l.state,l.unlist_pending,
            EXISTS(SELECT 1 FROM online_client_leases r WHERE r.installation_id=l.installation_id AND r.released_at IS NULL),
            l.unlisted_at,coalesce(o.delivery_attempts,0),o.last_error,o.last_attempt_at
            FROM online_client_listings l LEFT JOIN online_client_operations o USING(installation_id)
            WHERE l.installation_id=?1 AND l.seller_id=?2",params![id,a.user_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?,r.get::<_,bool>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<i64>>(6)?))).optional().market()?.ok_or_else(missing)?
    };
    let connected = s
        .proxy
        .active_client_route_for_installation(&id)
        .await
        .is_some();
    let sync = if !pending && state != "locking" {
        "confirmed"
    } else if rented {
        "after_rental"
    } else if connected {
        "pending"
    } else {
        "waiting_reconnect"
    };
    Ok(Json(
        json!({"state":state,"hostSyncState":sync,"unlistedAt":unlisted_at,"commandAttempts":attempts,"lastError":last_error,"lastAttemptAt":last_attempt}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Quote {
    revision: i64,
    #[serde(default)]
    auto_renew: bool,
}
struct Terms {
    seller: String,
    revision: i64,
    listing_price: i64,
    self_rental: bool,
    price: i64,
    days: Option<u32>,
    trial: i64,
}
fn buyable(
    tx: &Transaction<'_>,
    id: &str,
    a: &AuthSession,
    revision: i64,
) -> Result<Terms, AppError> {
    let (seller, listing_price, days, trial, owner_capable): (String, i64, Option<u32>, i64, bool) =
        tx.query_row(
            "SELECT seller_id,cycle_price_minor,free_days,trial_days*86400,owner_rental
               FROM online_client_supply
              WHERE installation_id=?1 AND revision=?2 AND availability='available'",
            params![id, revision],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .market()?
        .ok_or_else(|| conflict("Client availability or offer changed"))?;
    let self_rental = seller == a.user_id;
    let t = Terms {
        seller,
        revision,
        listing_price,
        self_rental,
        price: if self_rental { 0 } else { listing_price },
        days: if self_rental { None } else { days },
        trial: if self_rental { 0 } else { trial },
    };
    if self_rental {
        if !owner_capable {
            return Err(conflict("upgrade this Client to rent your own listing"));
        }
        return Ok(t);
    }
    crate::market_access::ensure_product_access_tx(
        tx,
        &t.seller,
        &a.user_id,
        &a.email,
        "client_host",
        crate::market_access::pricing_kind_for_rate((t.price > 0).then_some(t.price)),
    )?;
    if t.price > 0 {
        crate::market_provider_finance::require_paid_offer_setup_tx(tx, &t.seller)?
    }
    if let Some(d) = t.days {
        if !crate::market_common::free_usage_available_tx(
            tx,
            &a.user_id,
            &t.seller,
            "client_host",
            id,
            d,
            &Utc::now().to_rfc3339(),
        )? {
            return Err(conflict("free rental allowance exhausted"));
        }
    }
    Ok(t)
}
async fn quote(
    State(s): State<ServerState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(v): Json<Quote>,
) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    let result = create_quote(&tx, &id, &a, &v)?;
    tx.commit().market()?;
    Ok(Json(result))
}

fn create_quote(
    tx: &Transaction<'_>,
    installation: &str,
    buyer: &AuthSession,
    request: &Quote,
) -> Result<Value, AppError> {
    let terms = buyable(tx, installation, buyer, request.revision)?;
    let quote_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let expires_at = now.timestamp() + 120;
    let auto_renew = request.auto_renew && terms.price > 0;
    let termination = crate::client_market_termination::freeze_quote_tx(tx, &quote_id)?;
    let (trial_epoch, trial_seconds, funding) = if terms.price > 0 {
        let trial = crate::market_client_trials::state_tx(
            tx,
            &buyer.user_id,
            &terms.seller,
            installation,
            terms.trial,
        )?;
        let at = now.to_rfc3339();
        crate::market_commission::freeze_quote_tx(tx, &quote_id, &terms.seller, &at)?;
        let funding = crate::market_wallet::wallet_quote_funding_tx(
            tx,
            &buyer.user_id,
            terms.price,
            trial.remaining_seconds,
            &at,
        )?;
        (trial.epoch, trial.remaining_seconds, Some(funding))
    } else {
        (0, 0, None)
    };
    let projection = json!({
        "managementVersion":2, "revision": terms.revision, "listingPriceMinor": terms.listing_price, "selfRental": terms.self_rental,
        "cyclePriceMinor": terms.price, "trialSeconds": trial_seconds,
        "freeDays": terms.days, "autoRenew": auto_renew, "currency": "USD",
        "healthPolicy": "utc_hour_10_of_12_v1", "terminationPolicy": termination,
    });
    tx.execute(
        "INSERT INTO online_client_quotes
            (id, installation_id, buyer_id, revision, cycle_price_minor, free_days, trial_epoch,
             projection_json, expires_at, auto_renew)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            quote_id,
            installation,
            buyer.user_id,
            terms.revision,
            terms.price,
            terms.days,
            trial_epoch,
            projection.to_string(),
            expires_at,
            auto_renew
        ],
    )
    .market()?;
    Ok(
        json!({"id": quote_id, "expiresAt": expires_at, "terms": projection, "walletFunding": funding}),
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Confirm {
    acknowledged: bool,
    /// `end` rents for the trial only: no balance is required and the rental
    /// ends without a charge when the trial runs out. Older clients omit it.
    #[serde(default)]
    after_trial: Option<String>,
}
async fn commit(
    State(s): State<ServerState>,
    Path(q): Path<String>,
    h: HeaderMap,
    Json(v): Json<Confirm>,
) -> Reply {
    let a = actor(&s, &h).await?;
    if !v.acknowledged {
        return Err(AppError::BadRequest(
            "confirm the rental terms before renting".into(),
        ));
    }
    let trial_only = match v.after_trial.as_deref() {
        None | Some("continue") => false,
        Some("end") => true,
        Some(_) => return Err(AppError::BadRequest("invalid after-trial choice".into())),
    };
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    let receipt = commit_quote(&tx, &q, &a, trial_only)?;
    tx.commit().market()?;
    Ok(Json(receipt))
}

fn commit_quote(
    tx: &Transaction<'_>,
    q: &str,
    a: &AuthSession,
    trial_only: bool,
) -> Result<Value, AppError> {
    let replay = tx
        .query_row(
            "SELECT id,state FROM online_client_leases WHERE quote_id=?1 AND buyer_id=?2",
            params![q, a.user_id],
            |r| Ok(json!({"leaseId":r.get::<_,String>(0)?,"state":r.get::<_,String>(1)?})),
        )
        .optional()
        .market()?;
    if let Some(v) = replay {
        return Ok(v);
    }
    let (id, rev, auto, trial_epoch, trial, price, days, self_rental, listing_price) = tx
        .query_row(
            "SELECT installation_id, revision, auto_renew, trial_epoch, json_extract(projection_json,'$.trialSeconds'),
                    cycle_price_minor, free_days, coalesce(json_extract(projection_json,'$.selfRental'),0),
                    coalesce(json_extract(projection_json,'$.listingPriceMinor'),cycle_price_minor)
               FROM online_client_quotes WHERE id=?1 AND buyer_id=?2 AND expires_at>unixepoch() AND json_extract(projection_json,'$.managementVersion')=2",
            params![q, a.user_id],
            |r| Ok((r.get::<_,String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_,i64>(4)?,
                r.get::<_,i64>(5)?, r.get::<_,Option<u32>>(6)?, r.get::<_,bool>(7)?, r.get::<_,i64>(8)?)),
        )
        .optional()
        .market()?
        .ok_or_else(|| conflict("quote expired"))?;
    let t = buyable(tx, &id, a, rev)?;
    if (price, days, self_rental, listing_price)
        != (t.price, t.days, t.self_rental, t.listing_price)
    {
        return Err(conflict("Client offer changed"));
    }
    // Trial-only needs a paid offer with trial time left and no renewal.
    let trial_only = trial_only && t.price > 0;
    if trial_only && (trial <= 0 || auto) {
        return Err(AppError::coded_conflict(
            "CLIENT_TRIAL_ONLY_UNAVAILABLE",
            "no trial remains for this Client, or automatic renewal is on",
            json!({}),
        ));
    }
    let now = Utc::now();
    let date = now.to_rfc3339();
    let lease = Uuid::new_v4().to_string();
    let alias = format!("rent-{}", &Uuid::new_v4().simple().to_string()[..20]);
    if t.price > 0 {
        crate::market_client_trials::state_tx(tx, &a.user_id, &t.seller, &id, t.trial)?
            .validate_quote(trial_epoch, trial)?;
        let contract = wallet::prepare_contract_tx(
            tx,
            &wallet::PrepareContract {
                product_kind: "client_host",
                product_ref: &lease,
                trial_scope_ref: &id,
                quote_id: &q,
                buyer_user_id: &a.user_id,
                provider_user_id: &t.seller,
                offer_revision: t.revision,
                billing_model: wallet::BillingModel::CalendarMonth,
                price_minor: t.price,
                auto_renew: auto,
                trial_only,
                trial_seconds: trial,
                trial_allowance_seconds: t.trial,
            },
            &date,
        )?;
        wallet::bind_client_installation_tx(
            tx, &contract, &lease, &lease, &a.user_id, &t.seller, &date,
        )?;
    }
    let until = if t.price > 0 && trial > 0 {
        now.timestamp() + trial.max(86400)
    } else if t.price > 0 {
        crate::market_common::next_calendar_month_at(now, now.day())?.timestamp()
    } else if let Some(days) = t.days {
        now.timestamp()
            + crate::market_common::free_usage_remaining_tx(
                tx,
                &a.user_id,
                &t.seller,
                "client_host",
                &id,
                days,
                &date,
            )?
    } else {
        (now + Duration::days(30)).timestamp()
    };
    let seq = advance(tx, &id)?;
    tx.execute("INSERT INTO online_client_leases(id,installation_id,quote_id,buyer_id,tenant_email,seller_id,alias,state,sequence,grant_until,created_at,management_version) VALUES(?1,?2,?3,?4,?5,?6,?7,'starting',?8,?9,?10,2)",params![lease,id,q,a.user_id,a.email,t.seller,alias,seq,until,now.timestamp()]).market()?;
    crate::public_hosts::claim(
        tx,
        crate::public_hosts::NewPublicHost {
            label: &alias,
            route_id: &format!("lease:{lease}"),
            subject_id: &lease,
            installation_id: Some(&id),
            target_lane_id: &format!("client:{id}"),
            kind: crate::namespace::PublicHostKind::Client,
        },
    )
    .map_err(|e| conflict(&e.to_string()))?;
    tx.execute(
        "UPDATE public_hosts SET lifecycle='disabled' WHERE label=?1",
        params![alias],
    )
    .market()?;
    queue(
        tx,
        &id,
        seq,
        json!({"operation":"begin","grant":Grant{buyer_id:Some(a.user_id.clone()),lease_id:lease.clone(),tenant_email:a.email.clone(),alias,expires_at:until}}),
    )?;
    Ok(json!({"leaseId":lease,"state":"starting"}))
}

async fn end(
    State(s): State<ServerState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<crate::client_market_termination::TerminationRequest>,
) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    let (installation, state, reclaiming, seller): (String, String, bool, String) = tx
        .query_row(
            "SELECT installation_id,state,reclaim_effective_at IS NOT NULL AND reclaim_cancelled_at IS NULL,seller_id
               FROM online_client_leases WHERE id=?1 AND buyer_id=?2",
            params![id, a.user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .market()?
        .ok_or_else(missing)?;
    if matches!(state.as_str(), "starting" | "active") {
        let now = stamp();
        let at = chrono::DateTime::from_timestamp(now, 0)
            .unwrap()
            .to_rfc3339();
        if reclaiming {
            // The seller already asked for the Client back: leaving early is
            // never charged as the buyer's choice. Recorded exactly like the
            // reclaim executing on schedule (the seller's responsibility).
            seller::deny_buyer_if_requested_tx(&tx, &id)?;
            crate::client_market_termination::accept_tx(
                &tx,
                &id,
                Some(&seller),
                "provider_reclaim",
                None,
                &at,
            )?;
        } else {
            crate::client_market_termination::accept_tx(
                &tx,
                &id,
                Some(&a.user_id),
                "client_release",
                Some(&request),
                &at,
            )?;
        }
        reconcile::fence(&tx, &installation, &id, now)?;
    }
    let termination_id: Option<String> = tx
        .query_row(
            "SELECT id FROM client_market_terminations WHERE installation_id=?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .market()?;
    tx.commit().market()?;
    Ok(Json(
        json!({"leaseId":id,"terminationId":termination_id,"state":if matches!(state.as_str(),"active"|"starting"){"ending"}else{&state}}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListingsQuery {
    include_available_hours24: Option<bool>,
}

// Previous shop bundles expect a count of completed UTC hours on the unqualified URL.
fn legacy_available_hours(
    c: &Connection,
    viewer: Option<&str>,
) -> Result<HashMap<String, i64>, AppError> {
    let mut stmt = c.prepare(
        "SELECT installation_id, count(*) FROM (
             SELECT p.installation_id
             FROM online_client_probes p JOIN online_client_listings l ON l.installation_id=p.installation_id
             WHERE p.healthy=1 AND p.bucket >= (unixepoch()/3600-24)*12 AND p.bucket < (unixepoch()/3600)*12
               AND (l.state='listed' OR (l.seller_id=?1 AND l.state!='unlisted'))
             GROUP BY p.installation_id, p.bucket/12 HAVING count(DISTINCT p.bucket)>=10
         ) GROUP BY installation_id"
    ).market()?;
    stmt.query_map(params![viewer], |r| Ok((r.get(0)?, r.get(1)?)))
        .market()?
        .collect::<crate::db::Result<_>>()
        .market()
}

async fn listings(
    State(s): State<ServerState>,
    Query(query): Query<ListingsQuery>,
    h: HeaderMap,
) -> Reply {
    let me = crate::api::resolve_router_session(&s, &h)
        .await?
        .map(|s| s.user_id);
    let c = s.store.conn.lock().await;
    let health = crate::store::installation_health_minutes_24h(&c)?;
    let month = online_minutes_30d(&c)?;
    let legacy_hours = query
        .include_available_hours24
        .unwrap_or(true)
        .then(|| legacy_available_hours(&c, me.as_deref()))
        .transpose()?;
    let mut stmt = c.prepare(
        "SELECT l.installation_id, l.revision, l.state, l.cycle_price_minor, l.free_days, l.trial_days,
                l.country_code, l.egress_ip, l.check_json, l.probe_healthy, l.observed_at, l.seller_id IS ?1,
                l.occupied, t.subdomain, coalesce(i.last_seen_ip!=l.egress_ip,0), mp.id,
                l.probe_error, o.delivery_attempts, i.created_at, l.availability, l.exclusive_management, l.owner_rental,
                l.title, l.owner_email
         FROM online_client_supply l
         JOIN installations i ON i.id=l.installation_id
         JOIN installation_client_tunnels t ON t.installation_id=l.installation_id
         LEFT JOIN market_provider_profiles mp ON mp.user_id=l.seller_id
         LEFT JOIN online_client_operations o ON o.installation_id=l.installation_id
         WHERE l.state='listed' OR (l.seller_id=?1 AND (l.state!='unlisted' OR EXISTS(SELECT 1 FROM online_client_leases active WHERE active.installation_id=l.installation_id AND active.released_at IS NULL)))
         ORDER BY (l.availability='available') DESC, i.created_at, l.installation_id"
    ).market()?;
    let rows = stmt.query_map(params![me], |r| {
        let installation: String = r.get(0)?;
        let check = r.get::<_, Option<String>>(8)?
            .and_then(|value| serde_json::from_str::<Value>(&value).ok()).unwrap_or_default();
        let owned: bool = r.get(11)?;
        let label = if owned { r.get(13)? } else { public_label(&installation) };
        let (healthy, observed) = health.get(&installation).copied().unwrap_or_default();
        let rate = (observed > 0).then(|| crate::store::observed_online_rate(healthy, observed));
        let (online_30d, observed_30d, observed_days_30d) = month.get(&installation).copied().unwrap_or_default();
        let rate_30d = (observed_30d > 0).then(|| {
            crate::store::observed_online_rate(online_30d as usize, observed_30d as usize)
        });
        let mut item = json!({
            "installationId": installation, "revision": r.get::<_, i64>(1)?, "state": r.get::<_, String>(2)?,
            "cyclePriceMinor": r.get::<_, i64>(3)?, "freeDays": r.get::<_, Option<i64>>(4)?,
            "trialDays": r.get::<_, i64>(5)?, "country": r.get::<_, Option<String>>(6)?,
            "egressIp": crate::store::visible_host_ip(r.get(7)?, owned), "egressDiffers": r.get::<_, bool>(14)?,
            "hardware": check["hardware"], "healthy": r.get::<_, bool>(9)?, "observedAt": r.get::<_, Option<i64>>(10)?,
            "owned": owned, "occupied": r.get::<_, bool>(12)?, "label": label,
            "clientUrl": owned.then(|| s.config.tunnel_url(&label)), "providerId": r.get::<_, Option<String>>(15)?,
            "lastError": if owned { r.get::<_, Option<String>>(16)? } else { None },
            "attempts": if owned { r.get::<_, Option<i64>>(17)? } else { None },
            "registeredAt": r.get::<_, String>(18)?, "availability": r.get::<_, String>(19)?,
            "exclusiveManagement": r.get::<_, bool>(20)?, "ownerRental": r.get::<_, bool>(21)?,
            "onlineRate24h": rate, "observedMinutes24h": observed,
            "observationCoverage24h": crate::store::observation_coverage(observed),
            "onlineRate30d": rate_30d, "observedMinutes30d": observed_30d, "observedDays30d": observed_days_30d,
            "title": r.get::<_, Option<String>>(22)?, "sellerEmail": r.get::<_, String>(23)?,
        });
        if let Some(hours) = &legacy_hours {
            item["availableHours24"] = json!(hours.get(&installation).copied().unwrap_or_default());
        }
        Ok(item)
    }).market()?.collect::<crate::db::Result<Vec<_>>>().market()?;
    Ok(Json(json!({"listings":rows})))
}
/// Online and observed minutes over the last 30 UTC days, and how many of
/// those days have any observation, from the retained daily rollup.
fn online_minutes_30d(c: &Connection) -> Result<HashMap<String, (i64, i64, i64)>, AppError> {
    let mut stmt = c
        .prepare(
            "SELECT installation_id, SUM(online_minutes), SUM(observed_minutes), SUM(observed_minutes>0)
               FROM installation_online_days WHERE day_utc>=date('now','-29 days')
              GROUP BY installation_id",
        )
        .market()?;
    stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
    })
    .market()?
    .collect::<crate::db::Result<_>>()
    .market()
}

fn public_label(installation: &str) -> String {
    let short: String = installation
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("client-{short}")
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SellerItem {
    id: String,
    listing_id: Option<String>,
    label: String,
    state: String,
    cycle_price_minor: i64,
    tenant_email: Option<String>,
    expires_at: Option<i64>,
    termination_id: Option<String>,
    title: Option<String>,
    paused: bool,
    /// A scheduled reclaim that has not been cancelled.
    reclaim_effective_at: Option<i64>,
    reclaim_mode: Option<String>,
    /// The seller may reclaim this running rental now.
    reclaimable: bool,
    started_at: Option<i64>,
    ended_at: Option<i64>,
    end_reason: Option<String>,
    #[serde(skip)]
    billing_from: Option<i64>,
    #[serde(skip)]
    contract_id: Option<String>,
    /// Service time of the current period as settlement recorded it.
    available_seconds: Option<i64>,
    unavailable_seconds: Option<i64>,
    settled_through: Option<String>,
    /// What this rental earned the seller so far, in wallet units: rent net
    /// of the platform fee plus any termination compensation. Same figures
    /// as the Owner audit. `None` for free rentals.
    income_units: Option<String>,
}

async fn seller_inventory(State(s): State<ServerState>, headers: HeaderMap) -> Reply {
    let seller = actor(&s, &headers).await?;
    let conn = s.store.conn.lock().await;
    Ok(Json(
        json!({ "items": seller_items(&conn, &seller.user_id)? }),
    ))
}

fn seller_items(conn: &Connection, seller_id: &str) -> Result<Vec<SellerItem>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.alias, r.state, q.cycle_price_minor, r.tenant_email, r.grant_until, r.created_at,
                CASE WHEN l.seller_id=r.seller_id THEN l.installation_id END,(SELECT t.id FROM client_market_terminations t WHERE t.installation_id=r.id),
                l.title, l.paused_at IS NOT NULL,
                CASE WHEN r.reclaim_cancelled_at IS NULL AND r.state IN ('starting','active') AND r.cutoff_at IS NULL THEN r.reclaim_effective_at END,
                CASE WHEN r.reclaim_cancelled_at IS NULL AND r.state IN ('starting','active') AND r.cutoff_at IS NULL THEN r.reclaim_mode END,
                r.state IN ('starting','active') AND r.cutoff_at IS NULL AND r.released_at IS NULL AND r.buyer_id!=r.seller_id,
                coalesce(r.activated_at,r.created_at), coalesce(r.released_at,r.cutoff_at),
                (SELECT t.reason FROM client_market_terminations t WHERE t.installation_id=r.id),
                CASE WHEN r.released_at IS NULL AND r.activated_at IS NOT NULL THEN coalesce((
                    SELECT max(unixepoch(p.period_start),r.activated_at) FROM market_platform_contracts k
                      JOIN market_platform_periods p ON p.contract_id=k.id AND p.sequence=k.current_period_sequence
                     WHERE k.product_kind='client_host' AND k.product_ref=r.id),r.activated_at) END,
                (SELECT k.id FROM market_platform_contracts k WHERE k.product_kind='client_host' AND k.product_ref=r.id)
           FROM online_client_leases r JOIN online_client_quotes q ON q.id=r.quote_id
           JOIN online_client_listings l ON l.installation_id=r.installation_id
          WHERE r.seller_id=?1
          UNION ALL
         SELECT l.installation_id, t.subdomain, l.availability,
                l.cycle_price_minor, NULL, NULL, l.updated_at, l.installation_id,NULL,
                l.title, l.paused_at IS NOT NULL, NULL, NULL, 0, NULL, NULL, NULL, NULL, NULL
           FROM online_client_supply l JOIN installation_client_tunnels t ON t.installation_id=l.installation_id
          WHERE l.seller_id=?1 AND l.state!='unlisted' AND NOT EXISTS (
                SELECT 1 FROM online_client_leases r WHERE r.installation_id=l.installation_id AND r.released_at IS NULL)
          ORDER BY 7 DESC, 1"
    ).market()?;
    let items = stmt
        .query_map(params![seller_id], |row| {
            Ok(SellerItem {
                id: row.get(0)?,
                listing_id: row.get(7)?,
                label: row.get(1)?,
                state: row.get(2)?,
                cycle_price_minor: row.get(3)?,
                tenant_email: row.get(4)?,
                expires_at: row.get(5)?,
                termination_id: row.get(8)?,
                title: row.get(9)?,
                paused: row.get(10)?,
                reclaim_effective_at: row.get(11)?,
                reclaim_mode: row.get(12)?,
                reclaimable: row.get(13)?,
                started_at: row.get(14)?,
                ended_at: row.get(15)?,
                end_reason: row.get(16)?,
                billing_from: row.get(17)?,
                contract_id: row.get(18)?,
                available_seconds: None,
                unavailable_seconds: None,
                settled_through: None,
                income_units: None,
            })
        })
        .market()?
        .collect::<crate::db::Result<Vec<SellerItem>>>()
        .market()?;
    let mut items = items;
    for item in items.iter_mut() {
        if let Some(from) = item.billing_from {
            if let Some(billed) = crate::client_market_audit::billed_time_tx(conn, &item.id, from)?
            {
                item.available_seconds = Some(billed.available_seconds);
                item.unavailable_seconds = Some(billed.unavailable_seconds);
                item.settled_through = billed.settled_through;
            }
        }
        if let Some(contract) = item.contract_id.as_deref() {
            let income: i64 = conn
                .query_row(
                    "SELECT coalesce((SELECT SUM(l.net_amount_units-l.reversed_units)
                         FROM market_delivery_slices d JOIN market_provider_earning_lots l ON l.delivery_slice_id=d.id
                        WHERE d.contract_kind='client_host' AND d.contract_id=?1),0)
                      + coalesce((SELECT SUM(c.amount_units-c.reversed_units)
                         FROM client_market_terminations t JOIN client_termination_compensations c ON c.id=t.id
                        WHERE t.contract_id=?1),0)",
                    params![contract],
                    |r| r.get(0),
                )
                .market()?;
            item.income_units = Some(income.to_string());
        }
    }
    Ok(items)
}

#[derive(Deserialize)]
struct LeaseListQuery {
    #[serde(default)]
    current: bool,
    focus: Option<String>,
}

async fn mine(
    State(s): State<ServerState>,
    Query(query): Query<LeaseListQuery>,
    h: HeaderMap,
) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let mut stmt=c.prepare("SELECT r.id,r.installation_id,r.alias,r.state,r.grant_until,r.activated_at,r.cutoff_at,r.released_at,
        COALESCE((SELECT json_group_array(json_object('kind',kind,'at',recorded_at,'count',occurrences)) FROM (SELECT kind,recorded_at,occurrences FROM online_client_events WHERE lease_id=r.id ORDER BY id DESC LIMIT 50)),'[]'),
        (SELECT id FROM client_market_terminations t WHERE t.installation_id=r.id),q.cycle_price_minor,coalesce(c.renewal_policy='automatic',q.auto_renew),l.country_code,
        unixepoch(p.period_end),h.status,r.created_at,
        l.title,
        CASE WHEN r.state='active' AND r.cutoff_at IS NULL THEN l.egress_ip END,
        CASE WHEN r.reclaim_cancelled_at IS NULL AND r.state IN ('starting','active') AND r.cutoff_at IS NULL THEN r.reclaim_effective_at END,
        CASE WHEN r.reclaim_cancelled_at IS NULL AND r.state IN ('starting','active') AND r.cutoff_at IS NULL THEN r.reclaim_mode END,
        CASE WHEN r.reclaim_cancelled_at IS NULL AND r.state IN ('starting','active') AND r.cutoff_at IS NULL THEN r.reclaim_reason END,
        c.id,c.after_trial,c.trial_seconds_remaining,c.status,c.paid_started_at IS NOT NULL,
        (SELECT s.subdomain FROM online_client_share_bindings b JOIN shares s ON s.share_id=b.share_id
          WHERE b.lease_id=r.id AND s.subdomain IS NOT NULL ORDER BY s.created_at LIMIT 1),
        r.buyer_id=r.seller_id,l.owner_email,
        CASE WHEN r.reclaim_cancelled_at IS NULL AND r.reclaim_effective_at IS NOT NULL AND r.reclaim_note!='' THEN r.reclaim_note END,
        CASE WHEN r.reclaim_cancelled_at IS NULL AND r.reclaim_effective_at IS NOT NULL THEN r.reclaim_reason END,
        (SELECT t.reason FROM client_market_terminations t WHERE t.installation_id=r.id)='provider_reclaim',
        coalesce(i.last_seen_ip!=l.egress_ip,0)
        FROM online_client_leases r JOIN online_client_quotes q ON q.id=r.quote_id
        JOIN online_client_listings l ON l.installation_id=r.installation_id LEFT JOIN installations i ON i.id=r.installation_id
        LEFT JOIN market_platform_contracts c ON c.product_kind='client_host' AND c.product_ref=r.id
        LEFT JOIN market_platform_periods p ON p.contract_id=c.id AND p.sequence=c.current_period_sequence
        LEFT JOIN market_wallet_holds h ON h.id=c.activation_hold_id WHERE r.buyer_id=?1
        AND (NOT ?2 OR r.released_at IS NULL OR r.id=?3)
        ORDER BY r.released_at IS NULL DESC,r.created_at DESC LIMIT CASE WHEN ?2 THEN -1 ELSE 200 END").market()?;
    let rows = stmt
        .query_map(params![a.user_id, query.current, query.focus], |r| {
            let alias: String = r.get(2)?;
            let share: Option<String> = r.get(26)?;
            Ok(json!({
                "leaseId": r.get::<_, String>(0)?, "installationId": r.get::<_, String>(1)?, "alias": alias,
                "state": r.get::<_, String>(3)?, "expiresAt": r.get::<_, i64>(4)?, "activatedAt": r.get::<_, Option<i64>>(5)?,
                "cutoffAt": r.get::<_, Option<i64>>(6)?, "releasedAt": r.get::<_, Option<i64>>(7)?,
                "terminationId": r.get::<_, Option<String>>(9)?, "clientUrl": s.config.tunnel_url(&alias),
                "events": serde_json::from_str::<Value>(&r.get::<_, String>(8)?).unwrap_or_default(),
                "cyclePriceMinor": r.get::<_, i64>(10)?, "autoRenew": r.get::<_, bool>(11)?,
                "country": r.get::<_, Option<String>>(12)?, "periodEnd": r.get::<_, Option<i64>>(13)?,
                "holdStatus": r.get::<_, Option<String>>(14)?, "createdAt": r.get::<_, i64>(15)?,
                "title": r.get::<_, Option<String>>(16)?,
                // The tenant judges the IP quality; only while the rental runs.
                "egressIp": r.get::<_, Option<String>>(17)?,
                "reclaimEffectiveAt": r.get::<_, Option<i64>>(18)?, "reclaimMode": r.get::<_, Option<String>>(19)?,
                // The reason stays visible after the reclaim executes.
                "reclaimReason": r.get::<_, Option<String>>(30)?,
                "contractId": r.get::<_, Option<String>>(21)?, "afterTrial": r.get::<_, Option<String>>(22)?,
                "trialSecondsRemaining": r.get::<_, Option<i64>>(23)?, "contractStatus": r.get::<_, Option<String>>(24)?,
                "paidStarted": r.get::<_, Option<bool>>(25)?.unwrap_or(false),
                "shareUrl": share.map(|label| s.config.tunnel_url(&label)),
                "selfRental": r.get::<_, bool>(27)?, "sellerEmail": r.get::<_, String>(28)?,
                // Shown before and after a reclaim; the tenant must see why.
                "reclaimNote": r.get::<_, Option<String>>(29)?,
                "reclaimed": r.get::<_, Option<bool>>(31)?.unwrap_or(false),
                "egressDiffers": r.get::<_, bool>(32)?,
            }))
        })
        .market()?
        .collect::<crate::db::Result<Vec<_>>>()
        .market()?;
    Ok(Json(json!({"leases":rows})))
}

pub(crate) fn rental_label(host: &str) -> bool {
    let label = host.split('.').next().unwrap_or("");
    label
        .strip_prefix("rent-")
        .is_some_and(|s| s.len() == 20 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
pub(crate) async fn alias(
    s: &ServerState,
    label: &str,
) -> Result<Option<(String, String, String, String)>, AppError> {
    let c = s.store.conn.lock().await;
    c.query_row("SELECT r.id,r.installation_id,t.subdomain,CASE WHEN r.state='starting' THEN 'starting' WHEN r.state='active' AND r.grant_until>unixepoch() AND p.lifecycle='active' AND (r.management_version<2 OR l.exclusive_management) THEN 'active' ELSE 'ended' END
        FROM online_client_leases r JOIN online_client_listings l USING(installation_id) LEFT JOIN installation_client_tunnels t ON t.installation_id=r.installation_id
        JOIN public_hosts p ON p.label=r.alias AND p.subject_id=r.id WHERE r.alias=?1",params![label],|r|Ok((r.get(0)?,r.get(1)?,r.get::<_,Option<String>>(2)?.unwrap_or_default(),r.get(3)?))).optional().market()
}

#[cfg(test)]
mod inventory_tests {
    use super::*;

    #[test]
    fn public_listing_label_hides_the_physical_subdomain() {
        assert_eq!(
            public_label("8F3A21C0-1111-4222-8333-444455556666"),
            "client-8f3a21c0"
        );
    }

    #[test]
    fn public_addresses_only_expose_networks() {
        let visible = |ip: &str, reveal| crate::store::visible_host_ip(Some(ip.to_owned()), reveal);
        for (address, network) in [
            ("203.0.113.42", "203.0.113.xx"),
            ("::ffff:203.0.113.42", "203.0.113.xx"),
            ("2001:db8::1", "2001:db8::/64"),
            ("2001:db8:0:0:0:0:0:1", "2001:db8::/64"),
            ("2001::ab:cd:ef", "2001::/64"),
        ] {
            assert_eq!(visible(address, false).as_deref(), Some(network));
            assert_eq!(visible(address, true).as_deref(), Some(address));
        }
        assert!(visible("invalid address", false).is_none());
    }

    #[test]
    fn supply_distinguishes_recent_tunnel_and_maintenance_evidence_from_stale_probes() {
        let conn = crate::test_support::database().unwrap();
        crate::test_support::online_listing(&conn, "physical", "seller", "seller@example.test");
        for (error, age, healthy, expected) in [
            (None, 0, true, "available"),
            (Some("tunnel_offline"), 0, false, "offline"),
            (Some("upgrading"), 0, false, "upgrading"),
            (Some("switching"), 0, false, "switching"),
            (Some("control_unavailable"), 0, false, "unavailable"),
            (Some("tunnel_offline"), 601, false, "unavailable"),
            (Some("upgrading"), 601, false, "unavailable"),
            (Some("switching"), 601, false, "unavailable"),
            (None, 601, true, "unavailable"),
        ] {
            conn.execute(
                "UPDATE online_client_listings SET probe_error=?1, observed_at=unixepoch()-?2, healthy=?3 WHERE installation_id='physical'",
                params![error, age, healthy],
            ).unwrap();
            let (availability, idle): (String, bool) = conn.query_row(
                "SELECT availability, supply_status='idle' FROM online_client_supply WHERE installation_id='physical'", [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
            assert_eq!(availability, expected);
            assert_eq!(idle, expected == "available");
        }
    }

    #[test]
    fn legacy_hours_count_distinct_successful_buckets_per_visible_listing() {
        let conn = crate::test_support::database().unwrap();
        for id in ["public", "private"] {
            crate::test_support::online_listing(&conn, id, "seller", "seller@example.test");
        }
        conn.execute(
            "UPDATE online_client_listings SET state='unlocking' WHERE installation_id='private'",
            [],
        )
        .unwrap();
        let current_hour = stamp() / 3600;
        for (installation, lease, hour_offset, count) in [
            ("public", "", -2, 10),
            ("public", "", -3, 9),
            ("public", "earlier-lease", -3, 9),
            ("public", "", -26, 12),
            ("public", "", 2, 12),
            ("private", "", -2, 10),
        ] {
            for offset in 0..count {
                let bucket = (current_hour + hour_offset) * 12 + offset;
                conn.execute(
                    "INSERT INTO online_client_probes(installation_id,lease_id,bucket,checked_at,healthy,seller_id)
                     VALUES(?1,?2,?3,?4,1,'seller')",
                    params![installation, lease, bucket, bucket * 300],
                ).unwrap();
            }
        }
        assert_eq!(
            legacy_available_hours(&conn, None).unwrap(),
            HashMap::from([("public".into(), 1)])
        );
        assert_eq!(
            legacy_available_hours(&conn, Some("seller")).unwrap(),
            HashMap::from([("public".into(), 1), ("private".into(), 1)])
        );
    }

    #[test]
    fn seller_history_keeps_the_frozen_price_and_tenant_after_relisting() {
        let conn = crate::test_support::database().unwrap();
        crate::test_support::online_listing(&conn, "physical", "seller", "seller@example.test");
        conn.execute("INSERT INTO installation_client_tunnels(installation_id,owner_email,subdomain,enabled,created_at,updated_at)
            VALUES('physical','seller@example.test','physical-label',1,'2026-01-01','2026-01-01')",[]).unwrap();
        crate::test_support::online_lease(
            &conn,
            "lease",
            "physical",
            "seller",
            "buyer",
            "2026-01-01T00:00:00Z",
        );
        conn.execute("UPDATE online_client_quotes SET cycle_price_minor=350", [])
            .unwrap();
        let rows = seller_items(&conn, "seller").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].listing_id.as_deref(), Some("physical"));
        assert_eq!(rows[0].tenant_email.as_deref(), Some("buyer@example.test"));
        conn.execute(
            "UPDATE online_client_leases SET state='released',released_at=unixepoch()",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE online_client_listings SET seller_id='next',cycle_price_minor=800",
            [],
        )
        .unwrap();
        let old = seller_items(&conn, "seller").unwrap();
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].state, "released");
        assert_eq!(old[0].cycle_price_minor, 350);
        assert!(old[0].listing_id.is_none());
        let new = seller_items(&conn, "next").unwrap();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].id, "physical");
        assert_eq!(new[0].cycle_price_minor, 800);
        assert!(new[0].tenant_email.is_none());
        assert!(seller_items(&conn, "buyer").unwrap().is_empty());
    }
}
