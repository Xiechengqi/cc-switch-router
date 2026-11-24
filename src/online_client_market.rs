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
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::{Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

mod reconcile;
pub(crate) use reconcile::run;
mod shares;
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
            "/v1/client-market/online/installations/:id/listing",
            post(list).delete(unlist),
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
        .route("/v1/client-market/online/leases/:id/end", post(end))
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
    lease_id: String,
    tenant_email: String,
    alias: String,
    expires_at: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientStatus {
    capability: String,
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
    id: String,
    kind: String,
    at: i64,
}
async fn rpc(s: &ServerState, id: &str, path: &str, body: Value) -> Result<ClientStatus, AppError> {
    let timeout = match path {
        "/_ctl/market/check" => 10,
        "/_ctl/market/command" => 15,
        _ => 3,
    };
    let status: ClientStatus = control(s, id, path, body, timeout).await?;
    if status.capability != "marketLeaseV1" {
        return Err(conflict("Client does not support marketLeaseV1"));
    }
    Ok(status)
}
pub(crate) async fn listed(s: &ServerState, id: &str) -> Result<bool, AppError> {
    let c = s.store.conn.lock().await;
    c.query_row("SELECT EXISTS(SELECT 1 FROM online_client_listings WHERE installation_id=?1 AND state!='unlisted')",
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
        ON CONFLICT(installation_id) DO UPDATE SET sequence=excluded.sequence,request_json=excluded.request_json,next_attempt=0,attempts=0,last_error=NULL",
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
    acknowledged: bool,
}

fn seller_ready(c: &Connection, id: &str, email: &str) -> Result<(), AppError> {
    let owned:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM installations i JOIN installation_client_tunnels t ON t.installation_id=i.id
        WHERE i.id=?1 AND lower(trim(i.owner_email))=lower(?2) AND i.owner_verified_at IS NOT NULL AND i.market_lease_v1=1 AND i.lifecycle='active' AND i.client_activated_at IS NOT NULL
        AND t.enabled=1)",params![id,email],|r|r.get(0)).market()?;
    if !owned {
        return Err(AppError::Forbidden(
            "verified installation owner required".into(),
        ));
    }
    let overlap:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM share_market_listings l WHERE l.installation_id=?1
        AND (l.status='active' OR EXISTS(SELECT 1 FROM share_market_subscriptions sub WHERE sub.listing_id=l.id AND sub.released_at IS NULL)))",
        params![id],|r|r.get(0)).market()?;
    if overlap {
        return Err(conflict(
            "finish existing Share Market listings and rentals first",
        ));
    }
    Ok(())
}
async fn list(
    State(s): State<ServerState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(v): Json<Offer>,
) -> Reply {
    let a = actor(&s, &h).await?;
    if !v.acknowledged
        || !(0..=100_000_000).contains(&v.cycle_price_minor)
        || v.free_days.is_some_and(|n| !(1..=365).contains(&n))
        || v.trial_days > 30
        || v.cycle_price_minor > 0 && v.free_days.is_some()
        || v.cycle_price_minor == 0 && v.trial_days != 0
    {
        return Err(AppError::BadRequest(
            "invalid Client rental terms or missing acknowledgement".into(),
        ));
    }
    if s.proxy
        .active_client_route_for_installation(&id)
        .await
        .is_none()
    {
        return Err(conflict("Client must be online"));
    }
    {
        let c = s.store.conn.lock().await;
        seller_ready(&c, &id, &a.email)?;
    }
    let challenge = Uuid::new_v4().simple().to_string();
    {
        let c = s.store.conn.lock().await;
        let tx = c.transaction().market()?;
        seller_ready(&tx, &id, &a.email)?;
        let now = Utc::now().to_rfc3339();
        if let crate::market_provider_identity::MarketProviderIdentityResolution::Conflict(
            conflict,
        ) = crate::market_provider_identity::ensure_market_provider_identity_tx(
            &tx,
            Some(&a.user_id),
            &a.email,
            &now,
        )? {
            drop(tx);
            crate::market_provider_identity::persist_market_provider_identity_conflict(
                &c, &conflict, &now,
            )?;
            return Err(AppError::Conflict(
                "conflicting Market Provider identity".into(),
            ));
        }
        if v.cycle_price_minor > 0 {
            crate::market_provider_finance::require_paid_offer_setup_tx(&tx, &a.user_id)?;
        }
        let changed=tx.execute("INSERT INTO online_client_listings(installation_id,seller_id,owner_email,state,revision,sequence,cycle_price_minor,free_days,trial_days,updated_at,challenge,challenge_until)
            VALUES(?1,?2,lower(?3),'locking',1,1,?4,?5,?6,unixepoch(),?7,unixepoch()+30)
            ON CONFLICT(installation_id) DO UPDATE SET state='locking',revision=revision+1,sequence=sequence+1,seller_id=excluded.seller_id,owner_email=excluded.owner_email,
            cycle_price_minor=excluded.cycle_price_minor,free_days=excluded.free_days,trial_days=excluded.trial_days,challenge=excluded.challenge,challenge_until=excluded.challenge_until,egress_ip=NULL,country_code=NULL,check_json=NULL,healthy=0
            WHERE state='unlisted' OR (state='locking' AND challenge_until<unixepoch() AND check_json IS NULL)",
            params![id,a.user_id,a.email,v.cycle_price_minor,v.free_days,v.trial_days,challenge]).market()?;
        if changed != 1 {
            return Err(conflict(
                "listing transition already in progress or Client is listed",
            ));
        }
        tx.commit().market()?;
    }
    let checked = rpc(
        &s,
        &id,
        "/_ctl/market/check",
        json!({"challenge":challenge}),
    )
    .await;
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    tx.execute_batch("SAVEPOINT verified_listing").market()?;
    let result = (|| {
        let st = checked?;
        if !st.owner_email.eq_ignore_ascii_case(&a.email)
            || st.grant.is_some()
            || st.running_lease.is_some()
            || st.switching
            || !st.cleanup_safe
        {
            return Err(conflict("Client workspace is unavailable"));
        }
        let hardware = st
            .hardware
            .ok_or_else(|| conflict("missing hardware snapshot"))?;
        hardware.validate()?;
        let sequence=tx.query_row("UPDATE online_client_listings SET sequence=max(sequence,?3+1),challenge=NULL,challenge_until=NULL,check_json=?4,hardware_at=unixepoch()
            WHERE installation_id=?1 AND challenge=?2 AND challenge_until>=unixepoch() AND egress_ip IS NOT NULL AND state='locking' RETURNING sequence",
            params![id,challenge,st.sequence,json!({"hardware":hardware,"version":st.version.chars().take(80).collect::<String>()}).to_string()],|r|r.get::<_,i64>(0)).optional().market()?.ok_or_else(||conflict("egress verification expired"))?;
        seller_ready(&tx, &id, &a.email)?;
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
        c.query_row("SELECT EXISTS(SELECT 1 FROM online_client_listings WHERE challenge=?1 AND challenge_until>=unixepoch() AND egress_ip IS NULL)",params![challenge],|r|r.get(0)).market()?
    };
    if !valid {
        return Err(missing());
    }
    let country = match meta.country_code {
        Some(v) => Some(v),
        None => s.store.lookup_geo_country_code_for_ip(&ip).await,
    };
    let c = s.store.conn.lock().await;
    if c.execute("UPDATE online_client_listings SET egress_ip=?2,country_code=?3 WHERE challenge=?1 AND challenge_until>=unixepoch() AND egress_ip IS NULL",params![challenge,ip,country]).market()?!=1{return Err(missing())}
    Ok(StatusCode::NO_CONTENT)
}
async fn unlist(State(s): State<ServerState>, Path(id): Path<String>, h: HeaderMap) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    let seq=tx.query_row("UPDATE online_client_listings SET state='unlocking',revision=revision+1,sequence=sequence+1,challenge=NULL
        WHERE installation_id=?1 AND seller_id=?2 AND state IN ('listed','locking') AND NOT EXISTS(SELECT 1 FROM online_client_leases WHERE installation_id=?1 AND released_at IS NULL) RETURNING sequence",
        params![id,a.user_id],|r|r.get(0)).optional().market()?.ok_or_else(||conflict("finish the rental before unlisting"))?;
    queue(&tx, &id, seq, json!({"operation":"unlist"}))?;
    tx.commit().market()?;
    Ok(Json(json!({"state":"unlocking"})))
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
    let t=tx.query_row("SELECT seller_id,revision,cycle_price_minor,free_days,trial_days*86400 FROM online_client_supply
        WHERE installation_id=?1 AND state='listed' AND revision=?2 AND supply_status='idle'",params![id,revision],|r|Ok(Terms{seller:r.get(0)?,revision:r.get(1)?,price:r.get(2)?,days:r.get(3)?,trial:r.get(4)?})).optional().market()?.ok_or_else(||conflict("Client availability or offer changed"))?;
    if t.seller == a.user_id {
        return Err(conflict("cannot rent your own Client"));
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
        "revision": terms.revision, "cyclePriceMinor": terms.price, "trialSeconds": trial_seconds,
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
#[serde(deny_unknown_fields)]
struct Confirm {
    acknowledged: bool,
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
            "acknowledge owner root access before renting".into(),
        ));
    }
    let c = s.store.conn.lock().await;
    let tx = c.transaction().market()?;
    let receipt = commit_quote(&tx, &q, &a)?;
    tx.commit().market()?;
    Ok(Json(receipt))
}

fn commit_quote(tx: &Transaction<'_>, q: &str, a: &AuthSession) -> Result<Value, AppError> {
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
    let (id, rev, auto, trial_epoch, trial): (String, i64, bool, i64, i64) = tx
        .query_row(
            "SELECT installation_id, revision, auto_renew, trial_epoch, json_extract(projection_json,'$.trialSeconds')
             FROM online_client_quotes WHERE id=?1 AND buyer_id=?2 AND expires_at>unixepoch()",
            params![q, a.user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .market()?
        .ok_or_else(|| conflict("quote expired"))?;
    let t = buyable(tx, &id, a, rev)?;
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
                trial_only: false,
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
    tx.execute("INSERT INTO online_client_leases(id,installation_id,quote_id,buyer_id,tenant_email,seller_id,alias,state,sequence,grant_until,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'starting',?8,?9,?10)",params![lease,id,q,a.user_id,a.email,t.seller,alias,seq,until,now.timestamp()]).market()?;
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
        json!({"operation":"begin","grant":Grant{lease_id:lease.clone(),tenant_email:a.email.clone(),alias,expires_at:until}}),
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
    let (installation, state): (String, String) = tx
        .query_row(
            "SELECT installation_id,state FROM online_client_leases WHERE id=?1 AND buyer_id=?2",
            params![id, a.user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .market()?
        .ok_or_else(missing)?;
    if matches!(state.as_str(), "starting" | "active") {
        let now = stamp();
        let at = chrono::DateTime::from_timestamp(now, 0)
            .unwrap()
            .to_rfc3339();
        crate::client_market_termination::accept_tx(
            &tx,
            &id,
            Some(&a.user_id),
            "client_release",
            Some(&request),
            &at,
        )?;
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

async fn listings(State(s): State<ServerState>, h: HeaderMap) -> Reply {
    let me = crate::api::resolve_router_session(&s, &h)
        .await?
        .map(|s| s.user_id);
    let c = s.store.conn.lock().await;
    let mut stmt=c.prepare("SELECT l.installation_id,l.revision,l.state,l.cycle_price_minor,l.free_days,l.trial_days,l.country_code,l.egress_ip,
        l.check_json,l.probe_healthy,l.observed_at,l.seller_id=?1,l.occupied,
        (SELECT count(*) FROM (SELECT bucket/12 FROM online_client_probes p WHERE p.installation_id=l.installation_id
            AND p.healthy=1 AND bucket >= (unixepoch()/3600-24)*12 AND bucket < (unixepoch()/3600)*12
            GROUP BY bucket/12 HAVING count(DISTINCT bucket)>=10)),
        t.subdomain,coalesce(i.last_seen_ip!=l.egress_ip,0),(SELECT id FROM market_provider_profiles mp WHERE mp.user_id=l.seller_id)
        FROM online_client_supply l
        JOIN installations i ON i.id=l.installation_id JOIN installation_client_tunnels t ON t.installation_id=l.installation_id
        WHERE l.state='listed' OR (l.seller_id=?1 AND l.state!='unlisted') ORDER BY l.occupied,l.probe_healthy DESC,l.country_code,t.subdomain").market()?;
    let rows=stmt.query_map(params![me.unwrap_or_default()],|r|{
        let check=r.get::<_,Option<String>>(8)?.and_then(|v|serde_json::from_str::<Value>(&v).ok()).unwrap_or_default();
        // Only the seller sees the physical subdomain and full egress address. A
        // home seller behind NAT would otherwise publish their residential IP.
        let owned=r.get::<_,bool>(11)?;
        let installation=r.get::<_,String>(0)?;
        let label=if owned {r.get::<_,String>(14)?} else {public_label(&installation)};
        let egress=r.get::<_,Option<String>>(7)?.map(|ip| if owned {ip} else {crate::store::mask_host_ip(&ip)});
        Ok(json!({"installationId":installation,"revision":r.get::<_,i64>(1)?,"state":r.get::<_,String>(2)?,"cyclePriceMinor":r.get::<_,i64>(3)?,"freeDays":r.get::<_,Option<i64>>(4)?,"trialDays":r.get::<_,i64>(5)?,"country":r.get::<_,Option<String>>(6)?,"egressIp":egress,"hardware":check["hardware"],"healthy":r.get::<_,bool>(9)?,"observedAt":r.get::<_,Option<i64>>(10)?,"owned":owned,"occupied":r.get::<_,bool>(12)?,"availableHours24":r.get::<_,i64>(13)?,"label":label,"egressDiffers":r.get::<_,bool>(15)?,"providerId":r.get::<_,Option<String>>(16)?}))
    }).market()?.collect::<crate::db::Result<Vec<_>>>().market()?;
    Ok(Json(json!({"listings":rows})))
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
                CASE WHEN l.seller_id=r.seller_id AND l.state!='unlisted' THEN l.installation_id END
           FROM online_client_leases r JOIN online_client_quotes q ON q.id=r.quote_id
           JOIN online_client_listings l ON l.installation_id=r.installation_id
          WHERE r.seller_id=?1
          UNION ALL
         SELECT l.installation_id, t.subdomain,
                CASE WHEN l.state!='listed' THEN l.state WHEN l.occupied THEN 'occupied'
                     WHEN l.probe_healthy THEN 'available' ELSE 'offline' END,
                l.cycle_price_minor, NULL, NULL, l.updated_at, l.installation_id
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
            })
        })
        .market()?
        .collect::<crate::db::Result<Vec<_>>>()
        .market()?;
    Ok(items)
}

async fn mine(State(s): State<ServerState>, h: HeaderMap) -> Reply {
    let a = actor(&s, &h).await?;
    let c = s.store.conn.lock().await;
    let mut stmt=c.prepare("SELECT id,installation_id,alias,state,grant_until,activated_at,cutoff_at,released_at,
        COALESCE((SELECT json_group_array(json_object('kind',kind,'at',recorded_at)) FROM (SELECT kind,recorded_at FROM online_client_events WHERE lease_id=r.id ORDER BY id DESC LIMIT 50)),'[]')
         ,(SELECT id FROM client_market_terminations t WHERE t.installation_id=r.id) FROM online_client_leases r WHERE buyer_id=?1 ORDER BY created_at DESC LIMIT 200").market()?;
    let rows=stmt.query_map(params![a.user_id],|r|Ok(json!({"leaseId":r.get::<_,String>(0)?,"installationId":r.get::<_,String>(1)?,"alias":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"expiresAt":r.get::<_,i64>(4)?,"activatedAt":r.get::<_,Option<i64>>(5)?,"cutoffAt":r.get::<_,Option<i64>>(6)?,"releasedAt":r.get::<_,Option<i64>>(7)?,"terminationId":r.get::<_,Option<String>>(9)?,"clientUrl":s.config.tunnel_url(&r.get::<_,String>(2)?),"events":serde_json::from_str::<Value>(&r.get::<_,String>(8)?).unwrap_or_default()}))).market()?.collect::<crate::db::Result<Vec<_>>>().market()?;
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
) -> Result<Option<(String, String, String, bool)>, AppError> {
    let c = s.store.conn.lock().await;
    c.query_row("SELECT r.id,r.installation_id,t.subdomain,r.state='active' AND r.grant_until>unixepoch() AND p.lifecycle='active'
        FROM online_client_leases r LEFT JOIN installation_client_tunnels t ON t.installation_id=r.installation_id
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
        assert_eq!(crate::store::mask_host_ip("203.0.113.42"), "203.0.113.xx");
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
