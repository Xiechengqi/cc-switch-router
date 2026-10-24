//! Authenticated, no-store C2C projections. Every financial mutation uses one
//! IMMEDIATE transaction; external email delivery never holds the DB lock.
use super::*;
use crate::{ServerState, db::TransactionBehavior};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, header},
    routing::{get, post},
};
use rand::Rng;
use std::net::SocketAddr;

type Reply<T> = Result<([(header::HeaderName, String); 2], Json<T>), AppError>;
fn reply<T>(value: T) -> ([(header::HeaderName, String); 2], Json<T>) {
    (
        [
            (header::CACHE_CONTROL, "private, no-store".into()),
            (
                header::HeaderName::from_static("x-router-server-time"),
                crate::market_wallet::now(),
            ),
        ],
        Json(value),
    )
}
async fn session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::models::AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}
async fn write<T>(
    state: &ServerState,
    f: impl FnOnce(&Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin C2C write"))?;
    let value = f(&tx)?;
    tx.commit().map_err(db_error("commit C2C write"))?;
    Ok(value)
}
fn email_ready(state: &ServerState) -> bool {
    state
        .config
        .resend_api_key
        .as_deref()
        .is_some_and(|v| !v.trim().is_empty())
        && crate::notifications::notification_sender(&state.config).is_some()
}
fn require_email(state: &ServerState) -> Result<(), AppError> {
    if !email_ready(state) {
        return Err(problem(
            "C2C_EMAIL_UNAVAILABLE",
            "release verification email is not configured",
        ));
    }
    Ok(())
}
pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-wallet/c2c/policy", get(policy))
        .route("/v1/market-wallet/c2c/seller", get(seller).put(save_seller))
        .route("/v1/market-wallet/c2c/seller/heartbeat", post(heartbeat))
        .route("/v1/market-wallet/c2c/seller/pause", post(pause_seller))
        .route("/v1/market-wallet/c2c/sellers", get(offers))
        .route("/v1/market-wallet/c2c/quotes", post(quote))
        .route("/v1/market-wallet/c2c/orders", get(orders).post(create))
        .route("/v1/market-wallet/c2c/orders/:id", get(detail))
        .route("/v1/market-wallet/c2c/orders/:id/paid", post(paid))
        .route("/v1/market-wallet/c2c/orders/:id/cancel", post(cancel))
        .route("/v1/market-wallet/c2c/orders/:id/dispute", post(dispute))
        .route("/v1/market-wallet/c2c/orders/:id/late-payment", post(late))
        .route("/v1/market-wallet/c2c/orders/:id/notes", post(note))
        .route("/v1/market-wallet/c2c/orders/:id/events", get(events))
        .route(
            "/v1/market-wallet/c2c/orders/:id/release-code",
            post(send_code),
        )
        .route("/v1/market-wallet/c2c/orders/:id/release", post(release))
        .route(
            "/v1/admin/market-finance/c2c/policy",
            get(admin_policy).put(save_policy),
        )
        .route("/v1/admin/market-finance/c2c/orders", get(admin_orders))
        .route("/v1/admin/market-finance/c2c/orders/:id", get(admin_detail))
        .route(
            "/v1/admin/market-finance/c2c/orders/:id/events",
            get(admin_events),
        )
        .route(
            "/v1/admin/market-finance/c2c/orders/:id/resolve",
            post(resolve),
        )
        .route("/v1/admin/market-finance/c2c/sellers", get(admin_sellers))
        .route(
            "/v1/admin/market-finance/c2c/sellers/:id/suspension",
            post(suspend),
        )
}
async fn policy(State(state): State<ServerState>, headers: HeaderMap) -> Reply<serde_json::Value> {
    let u = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut blockers = participation_blockers_tx(&conn, &u.user_id)?;
    let verified = match verified_tx(&conn, &u.user_id) {
        Ok(_) => true,
        Err(error) if error.code() == Some("C2C_IDENTITY") => false,
        Err(error) => return Err(error),
    };
    if !email_ready(&state) {
        blockers.push("C2C_EMAIL_UNAVAILABLE");
    }
    Ok(reply(
        serde_json::json!({"policy":policy_tx(&conn)?,"available":blockers.is_empty(),"blockers":blockers,
        "canManagePolicy":crate::api::is_finance_owner(&state.config,&u.email),
        "verified":verified,"termsVersion":TERMS_VERSION,"claimGraceSeconds":120,"firstPurchaseMaxMinor":10000}),
    ))
}
async fn seller(State(state): State<ServerState>, headers: HeaderMap) -> Reply<serde_json::Value> {
    let u = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let mut blockers = seller_blockers_tx(&conn, &u.user_id, &crate::market_wallet::now())?;
    if !email_ready(&state) {
        blockers.push("C2C_EMAIL_UNAVAILABLE");
    }
    Ok(reply(
        serde_json::json!({"seller":seller_tx(&conn,&u.user_id)?,
        "blockers":blockers,"canEnable":blockers.is_empty(),
        "admissionBalanceUnits":admission_balance_tx(&conn,&u.user_id,&crate::market_wallet::now())?.to_string(),
        "availableUnits":stock_tx(&conn,&u.user_id,&crate::market_wallet::now())?.to_string()}),
    ))
}
async fn save_seller(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<SellerInput>,
) -> Reply<Seller> {
    let u = session(&state, &headers).await?;
    if input.enabled {
        require_email(&state)?;
    }
    Ok(reply(
        write(&state, |conn| {
            save_seller_tx(conn, &u.user_id, &input, &crate::market_wallet::now())
        })
        .await?,
    ))
}
async fn heartbeat(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Reply<serde_json::Value> {
    let u = session(&state, &headers).await?;
    write(&state,|conn| {
        verified_tx(conn,&u.user_id)?;
        conn.execute("UPDATE market_c2c_sellers SET heartbeat_at=?2 WHERE user_id=?1 AND enabled=1 AND suspended=0",params![u.user_id,crate::market_wallet::now()]).map_err(db_error("C2C seller heartbeat"))?;
        Ok(())
    }).await?;
    Ok(reply(serde_json::json!({"ok":true})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PauseSeller {
    expected_version: i64,
}
async fn pause_seller(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PauseSeller>,
) -> Reply<Seller> {
    let u = session(&state, &headers).await?;
    Ok(reply(
        write(&state, |conn| {
            pause_seller_tx(
                conn,
                &u.user_id,
                input.expected_version,
                &crate::market_wallet::now(),
            )
        })
        .await?,
    ))
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Page {
    #[serde(default)]
    before: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    quote_id: String,
}
async fn offers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(p): Query<Page>,
) -> Reply<serde_json::Value> {
    let u = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let (items, next) = offers_tx(&conn, &u.user_id, &p.before, &crate::market_wallet::now())?;
    Ok(reply(serde_json::json!({"items":items,"next":next})))
}
async fn quote(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<QuoteInput>,
) -> Reply<Quote> {
    let u = session(&state, &headers).await?;
    require_email(&state)?;
    Ok(reply(
        write(&state, |conn| {
            quote_tx(conn, &u.user_id, &input, &crate::market_wallet::now())
        })
        .await?,
    ))
}
async fn create(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<CreateInput>,
) -> Reply<Order> {
    let u = session(&state, &headers).await?;
    let key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ip = crate::client_meta::extract_client_metadata(&headers, peer)
        .ip
        .unwrap_or_else(|| peer.ip().to_string());
    // Recovery must work even after email configuration or finance was paused.
    Ok(reply(write(&state,|conn| {
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_c2c_orders WHERE buyer_id=?1 AND (quote_id=?2 OR idempotency_key=?3))",params![u.user_id,input.quote_id,key],|r|r.get(0)).map_err(db_error("check C2C replay"))?;
        if !exists {require_email(&state)?;}
        create_tx(conn,&u.user_id,&input,key,&ip,&crate::market_wallet::now())
    }).await?))
}
fn list_tx(conn: &Connection, user: Option<&str>, p: &Page) -> Result<serde_json::Value, AppError> {
    if !matches!(
        p.status.as_str(),
        "" | "active"
            | "exceptions"
            | "awaiting_payment"
            | "paid"
            | "disputed"
            | "released"
            | "cancelled"
            | "expired"
    ) || !matches!(p.role.as_str(), "" | "buyer" | "seller")
    {
        return Err(AppError::BadRequest("invalid C2C filter".into()));
    }
    let mut stmt=conn.prepare("SELECT o.id FROM market_c2c_orders o
       WHERE (?1 IS NULL OR ((?3='' AND (o.buyer_id=?1 OR o.seller_id=?1)) OR (?3='buyer' AND o.buyer_id=?1) OR (?3='seller' AND o.seller_id=?1)))
        AND (?2='' OR o.status=?2 OR (?2='active' AND o.status IN ('awaiting_payment','paid','disputed')) OR (?2='exceptions' AND (o.status='disputed' OR (o.late_payment_reported=1 AND o.late_payment_resolved=0))))
        AND (?4='' OR (o.created_at,o.id)<(SELECT created_at,id FROM market_c2c_orders WHERE id=?4))
        AND (?5='' OR o.quote_id=?5)
       ORDER BY o.created_at DESC,o.id DESC LIMIT 51").map_err(db_error("prepare C2C order history"))?;
    let mut ids = stmt
        .query_map(params![user, p.status, p.role, p.before, p.quote_id], |r| {
            r.get::<_, String>(0)
        })
        .map_err(db_error("query C2C order history"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read C2C order history"))?;
    let more = ids.len() > 50;
    ids.truncate(50);
    let next = if more { ids.last().cloned() } else { None };
    let items = ids
        .iter()
        .map(|id| {
            let mut o = order_tx(conn, id)?;
            o.legacy_payment = None;
            Ok(o)
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(serde_json::json!({"items":items,"next":next}))
}
async fn orders(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(p): Query<Page>,
) -> Reply<serde_json::Value> {
    let u = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok(reply(list_tx(&conn, Some(&u.user_id), &p)?))
}
async fn detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Reply<Order> {
    let u = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok(reply(owned_tx(&conn, &id, &u.user_id)?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Action {
    expected_version: i64,
    #[serde(default)]
    note: String,
    #[serde(default)]
    acknowledge_unpaid: bool,
}
fn current(o: &Order, input: &Action) -> Result<(), AppError> {
    if o.version != input.expected_version {
        return Err(problem("C2C_STALE", "order changed; reload before acting"));
    }
    Ok(())
}
async fn paid(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Reply<Order> {
    let u = session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            let o = owned_tx(c, &id, &u.user_id)?;
            mark_paid_tx(
                c,
                &o,
                &u.user_id,
                a.expected_version,
                &crate::market_wallet::now(),
            )
        })
        .await?,
    ))
}
async fn cancel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Reply<Order> {
    let u = session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            let o = owned_tx(c, &id, &u.user_id)?;
            if u.user_id != o.buyer_id || !a.acknowledge_unpaid {
                return Err(AppError::Forbidden(
                    "only the buyer who has not paid may cancel".into(),
                ));
            }
            if !matches!(o.status.as_str(), "cancelled" | "expired") {
                current(&o, &a)?;
            }
            cancel_tx(c, &o, &u.user_id, false, &crate::market_wallet::now())
        })
        .await?,
    ))
}
async fn dispute(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Reply<Order> {
    let u = session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            let o = owned_tx(c, &id, &u.user_id)?;
            current(&o, &a)?;
            dispute_tx(c, &o, &u.user_id, &a.note, &crate::market_wallet::now())
        })
        .await?,
    ))
}
async fn late(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Reply<Order> {
    let u = session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            let o = owned_tx(c, &id, &u.user_id)?;
            current(&o, &a)?;
            late_payment_tx(c, &o, &u.user_id, &a.note, &crate::market_wallet::now())
        })
        .await?,
    ))
}
async fn note(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Reply<Order> {
    let u = session(&state, &headers).await?;
    Ok(reply(write(&state,|c| {
        let o=owned_tx(c,&id,&u.user_id)?;current(&o,&a)?;
        let time=crate::market_wallet::now();
        let count:i64=c.query_row("SELECT COUNT(*) FROM market_c2c_events WHERE actor_id=?1 AND action='note' AND julianday(created_at)>=julianday(?2)-1.0/24",params![u.user_id,time],|r|r.get(0)).map_err(db_error("limit C2C notes"))?;
        if count>=20 {return Err(AppError::TooManyRequests("too many C2C notes".into()));}
        let o=change_tx(c,&o,&o.status,&time)?;
        event_tx(c,&o,&u.user_id,"note",&text(&a.note,1500)?,&time)?; Ok(o)
    }).await?))
}
fn events_tx(c: &Connection, id: &str, before: &str) -> Result<serde_json::Value, AppError> {
    let mut s=c.prepare("SELECT id,actor_id,action,note,created_at FROM market_c2c_events WHERE order_id=?1
        AND (?2='' OR (created_at,id)<(SELECT created_at,id FROM market_c2c_events WHERE id=?2 AND order_id=?1)) ORDER BY created_at DESC,id DESC LIMIT 51").map_err(db_error("prepare C2C events"))?;
    let mut items=s.query_map(params![id,before],|r|Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"actorId":r.get::<_,String>(1)?,"action":r.get::<_,String>(2)?,"note":r.get::<_,String>(3)?,"createdAt":r.get::<_,String>(4)?}))).map_err(db_error("query C2C events"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read C2C events"))?;
    let more = items.len() > 50;
    items.truncate(50);
    let next = if more {
        items.last().and_then(|v| v.get("id")).cloned()
    } else {
        None
    };
    Ok(serde_json::json!({"items":items,"next":next}))
}
#[cfg(test)]
pub(super) fn test_events(
    c: &Connection,
    id: &str,
    before: &str,
) -> Result<serde_json::Value, AppError> {
    events_tx(c, id, before)
}
async fn events(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(p): Query<Page>,
) -> Reply<serde_json::Value> {
    let u = session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    owned_tx(&c, &id, &u.user_id)?;
    Ok(reply(events_tx(&c, &id, &p.before)?))
}
fn binding(o: &Order) -> String {
    format!(
        "c2c-release:{}:{}:{}:{}:{}",
        o.id, o.seller_id, o.buyer_id, o.amount_units, o.version
    )
}
#[cfg(test)]
pub(super) fn binding_for_test(o: &Order) -> String {
    binding(o)
}
async fn send_code(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Reply<serde_json::Value> {
    let u = session(&state, &headers).await?;
    require_email(&state)?;
    let code = zeroize::Zeroizing::new(format!(
        "{:06}",
        rand::rngs::OsRng.gen_range(0..1_000_000_u32)
    ));
    let cid = Uuid::new_v4().to_string();
    let time = crate::market_wallet::now();
    let expires = (at(&time)? + Duration::minutes(10)).to_rfc3339();
    let ip = crate::client_meta::extract_client_metadata(&headers, peer)
        .ip
        .unwrap_or_else(|| peer.ip().to_string());
    let (email,o)=write(&state,|c| {
        let o=owned_tx(c,&id,&u.user_id)?;current(&o,&a)?;
        if o.seller_id!=u.user_id || !matches!(o.status.as_str(),"paid"|"disputed") {return Err(AppError::Forbidden("only the seller may authorize release after payment was declared".into()));}
        safe_tx(c,&o.seller_id,&o.buyer_id)?;
        let email=verified_tx(c,&u.user_id)?;
        let (soon,count):(bool,i64)=c.query_row("SELECT EXISTS(SELECT 1 FROM market_c2c_challenges WHERE seller_id=?1 AND julianday(created_at)>=julianday(?3)-60.0/86400),
            (SELECT COUNT(*) FROM market_c2c_challenges WHERE (seller_id=?1 OR created_ip=?2) AND julianday(created_at)>=julianday(?3)-1.0/24)",params![u.user_id,ip,time],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db_error("limit C2C release codes"))?;
        if soon||count>=10 {return Err(AppError::TooManyRequests("wait before requesting another release code".into()));}
        let bind=binding(&o);let (key,mac)=state.market_payments.transfer_mac(&cid,&bind,&code)?;
        c.execute("UPDATE market_c2c_challenges SET consumed_at=?2 WHERE order_id=?1 AND consumed_at IS NULL",params![id,time]).map_err(db_error("retire C2C codes"))?;
        c.execute("INSERT INTO market_c2c_challenges(id,order_id,seller_id,email,binding,code_mac,key_version,expires_at,created_ip,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![cid,id,u.user_id,email,bind,mac,key,expires,ip,time]).map_err(db_error("save C2C code"))?;
        Ok((email,o))
    }).await?;
    let amount = format!(
        "US${}.{:02}",
        o.amount_units / 8_640_000,
        (o.amount_units / 86_400) % 100
    );
    let text = format!(
        "C2C 余额放行验证码：{}。订单 {}，放行金额 {amount}。请亲自核实已收到线下款项后再输入；验证码有效 10 分钟，切勿提供给买家。平台不能自动核实线下付款，放行后不可撤回。",
        &*code, o.id
    );
    let html = crate::store::render_email_layout(
        "确认 C2C 余额放行",
        "请先核实您的收款账户，再确认放行。",
        &format!(
            "<p>订单 {} · 放行金额 <strong>{amount}</strong></p><p style=\"font-size:30px;font-weight:700;letter-spacing:6px\">{}</p><p>10 分钟内有效。请亲自核实已收到线下款项；不要将验证码提供给买家。放行后不可撤回。</p>",
            o.id, &*code
        ),
    );
    let from = crate::notifications::notification_sender(&state.config)
        .ok_or_else(|| problem("C2C_EMAIL_UNAVAILABLE", "email sender is missing"))?;
    crate::notifications::send_resend_frozen_email(
        &state.proxy_http,
        state.config.resend_api_key.as_deref().unwrap_or(""),
        crate::notifications::FrozenEmailEnvelope {
            from: &from,
            recipient: &email,
            subject: "确认 C2C 余额放行 · TokenSwitch",
            html: &html,
            text: &text,
            reply_to: state.config.resend_reply_to.as_deref(),
            idempotency_key: &format!("c2c-release-code:{cid}"),
        },
    )
    .await
    .map_err(|_| {
        problem(
            "C2C_EMAIL_UNAVAILABLE",
            "release code was not delivered; the reservation is unchanged",
        )
    })?;
    write(&state, |c| {
        c.execute(
            "UPDATE market_c2c_challenges SET delivered_at=?2 WHERE id=?1 AND consumed_at IS NULL",
            params![cid, crate::market_wallet::now()],
        )
        .map_err(db_error("record C2C code delivery"))?;
        Ok(())
    })
    .await?;
    Ok(reply(
        serde_json::json!({"id":cid,"expiresAt":expires,"resendAfterSeconds":60}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReleaseInput {
    pub(super) expected_version: i64,
    pub(super) challenge_id: String,
    pub(super) code: String,
    pub(super) acknowledge_received: bool,
}
pub(super) fn release_authorized_tx(
    c: &Connection,
    runtime: &crate::market_payments::MarketPaymentsRuntime,
    user: &str,
    id: &str,
    a: &ReleaseInput,
    time: &str,
) -> Result<Option<Order>, AppError> {
    let o = owned_tx(c, id, user)?;
    if o.seller_id != user || !a.acknowledge_received {
        return Err(AppError::Forbidden(
            "only the seller who verified receipt may release".into(),
        ));
    }
    if o.status == "released" {
        return Ok(Some(o));
    }
    if o.version != a.expected_version {
        return Err(problem(
            "C2C_STALE",
            "order changed; request a new release code",
        ));
    }
    let saved:Option<(String,String,i64,i64,String,bool,bool,String)>=c.query_row("SELECT binding,code_mac,key_version,attempts,expires_at,delivered_at IS NOT NULL,consumed_at IS NOT NULL,email FROM market_c2c_challenges WHERE id=?1 AND order_id=?2 AND seller_id=?3",params![a.challenge_id,id,user],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional().map_err(db_error("read C2C release code"))?;
    let Some((bind, mac, key, attempts, expires, delivered, consumed, email)) = saved else {
        return Ok(None);
    };
    if consumed
        || !delivered
        || attempts >= 5
        || at(&expires)? <= at(time)?
        || bind != binding(&o)
        || verified_tx(c, user)? != email
    {
        return Ok(None);
    }
    c.execute(
        "UPDATE market_c2c_challenges SET attempts=attempts+1 WHERE id=?1",
        params![a.challenge_id],
    )
    .map_err(db_error("count C2C code attempt"))?;
    if a.code.len() != 6
        || !a.code.bytes().all(|b| b.is_ascii_digit())
        || !runtime.verify_transfer_mac(key, &a.challenge_id, &bind, &a.code, &mac)?
    {
        return Ok(None);
    }
    let result = release_tx(c, &o, user, time)?;
    c.execute(
        "UPDATE market_c2c_challenges SET consumed_at=?2 WHERE id=?1",
        params![a.challenge_id, time],
    )
    .map_err(db_error("consume C2C code"))?;
    Ok(Some(result))
}
async fn release(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<ReleaseInput>,
) -> Reply<Order> {
    let u = session(&state, &headers).await?;
    // Commit failed-code attempts too. Returning an error inside write would
    // roll them back and permit unlimited brute force.
    let result = write(&state, |c| {
        release_authorized_tx(
            c,
            &state.market_payments,
            &u.user_id,
            &id,
            &a,
            &crate::market_wallet::now(),
        )
    })
    .await?;
    Ok(reply(result.ok_or_else(|| {
        problem(
            "C2C_CODE_INVALID",
            "release code is invalid or expired; request a new code",
        )
    })?))
}
async fn admin_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Reply<serde_json::Value> {
    let u = crate::api::require_admin_session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    Ok(reply(
        serde_json::json!({"policy":policy_tx(&c)?,"canManage":crate::api::is_finance_owner(&state.config,&u.email),"emailReady":email_ready(&state)}),
    ))
}
async fn save_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(p): Json<Policy>,
) -> Reply<Policy> {
    let u = crate::api::require_owner_session(&state, &headers).await?;
    if p.enabled {
        require_email(&state)?;
    }
    Ok(reply(
        write(&state, |c| {
            save_policy_tx(c, &u.user_id, &p, &crate::market_wallet::now())
        })
        .await?,
    ))
}
async fn admin_orders(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(p): Query<Page>,
) -> Reply<serde_json::Value> {
    crate::api::require_admin_session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    Ok(reply(list_tx(&c, None, &p)?))
}
async fn admin_detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Reply<serde_json::Value> {
    crate::api::require_admin_session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    Ok(reply(
        serde_json::json!({"order":order_tx(&c,&id)?,"events":events_tx(&c,&id,"")?,"evidenceValid":evidence_valid_tx(&c,&id)?}),
    ))
}
async fn admin_events(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(p): Query<Page>,
) -> Reply<serde_json::Value> {
    crate::api::require_admin_session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    order_tx(&c, &id)?;
    Ok(reply(events_tx(&c, &id, &p.before)?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Resolution {
    expected_version: i64,
    action: String,
    evidence: String,
    attest_external_evidence: bool,
}
async fn resolve(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Resolution>,
) -> Reply<Order> {
    let u = crate::api::require_owner_session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            let o = order_tx(c, &id)?;
            resolve_tx(
                c,
                &o,
                &u.user_id,
                a.expected_version,
                &a.action,
                &a.evidence,
                a.attest_external_evidence,
                &crate::market_wallet::now(),
            )
        })
        .await?,
    ))
}
async fn admin_sellers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(p): Query<Page>,
) -> Reply<serde_json::Value> {
    crate::api::require_admin_session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    let mut s=c.prepare("SELECT user_id,version,display_name,enabled,suspended,heartbeat_at FROM market_c2c_sellers WHERE user_id>?1 ORDER BY user_id LIMIT 51").map_err(db_error("prepare admin C2C sellers"))?;
    let mut items=s.query_map(params![p.before],|r|Ok(serde_json::json!({"userId":r.get::<_,String>(0)?,"version":r.get::<_,i64>(1)?,"displayName":r.get::<_,String>(2)?,"enabled":r.get::<_,bool>(3)?,"suspended":r.get::<_,bool>(4)?,"heartbeatAt":r.get::<_,String>(5)?}))).map_err(db_error("query admin C2C sellers"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read admin C2C sellers"))?;
    let more = items.len() > 50;
    items.truncate(50);
    let next = if more {
        items.last().and_then(|v| v.get("userId")).cloned()
    } else {
        None
    };
    Ok(reply(serde_json::json!({"items":items,"next":next})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Suspension {
    expected_version: i64,
    suspended: bool,
    reason: String,
}
async fn suspend(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Suspension>,
) -> Reply<serde_json::Value> {
    let u = crate::api::require_owner_session(&state, &headers).await?;
    write(&state,|c| {
        let reason=text(&a.reason,500)?;
        let count=c.execute("UPDATE market_c2c_sellers SET suspended=?3,enabled=0,version=version+1,updated_at=?4 WHERE user_id=?1 AND version=?2",params![id,a.expected_version,a.suspended,crate::market_wallet::now()]).map_err(db_error("suspend C2C seller"))?;
        if count!=1{return Err(problem("C2C_STALE","seller changed; reload"));}
        audit_tx(c,&u.user_id,if a.suspended{"suspend"}else{"unsuspend"},"c2c_seller",&id,&reason,&crate::market_wallet::now())?;
        crate::market_notifications::enqueue_user_notification_tx(c,"buyer",&id,"c2c_seller_changed","c2c_seller",&id,&format!("c2c-suspension:{id}:{}",a.expected_version+1),serde_json::json!({}),&crate::market_wallet::now())?;Ok(())
    }).await?;
    Ok(reply(serde_json::json!({"ok":true})))
}

#[cfg(test)]
pub(super) fn test_list(
    c: &Connection,
    user: Option<&str>,
    status: &str,
    before: &str,
) -> Result<serde_json::Value, AppError> {
    list_tx(
        c,
        user,
        &Page {
            status: status.into(),
            before: before.into(),
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn replies_are_private_uncacheable_and_supply_router_time() {
        let response = reply(serde_json::json!({"ok": true})).into_response();
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert!(at(response.headers()["x-router-server-time"].to_str().unwrap()).is_ok());
    }
}
