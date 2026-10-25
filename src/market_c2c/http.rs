//! Authenticated, no-store C2C projections. Every financial mutation uses one
//! IMMEDIATE transaction; email delivery never authorizes or blocks money.
use super::*;
use crate::{ServerState, db::TransactionBehavior};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, header},
    routing::{get, post},
};
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
pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-wallet/c2c/policy", get(policy))
        .route("/v1/market-wallet/c2c/seller", get(seller).put(save_seller))
        .route(
            "/v1/market-wallet/c2c/seller/heartbeat",
            post(retired_presence),
        )
        .route("/v1/market-wallet/c2c/seller/pause", post(pause_seller))
        .route("/v1/market-wallet/c2c/seller/resume", post(resume_seller))
        .route("/v1/market-wallet/c2c/sellers", get(offers))
        .route("/v1/market-wallet/c2c/quotes", post(quote))
        .route("/v1/market-wallet/c2c/orders", get(orders).post(create))
        .route("/v1/market-wallet/c2c/orders/:id", get(detail))
        .route("/v1/market-wallet/c2c/orders/:id/paid", post(paid))
        .route("/v1/market-wallet/c2c/orders/:id/accept", post(accept))
        .route("/v1/market-wallet/c2c/orders/:id/reject", post(reject))
        .route("/v1/market-wallet/c2c/orders/:id/cancel", post(cancel))
        .route("/v1/market-wallet/c2c/orders/:id/dispute", post(dispute))
        .route("/v1/market-wallet/c2c/orders/:id/late-payment", post(late))
        .route("/v1/market-wallet/c2c/orders/:id/notes", post(note))
        .route("/v1/market-wallet/c2c/orders/:id/events", get(events))
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
            "/v1/admin/market-finance/c2c/dispute-handlers",
            get(handlers).put(save_handler),
        )
        .route(
            "/v1/admin/market-finance/c2c/sellers/:id/suspension",
            post(suspend),
        )
}
async fn policy(State(state): State<ServerState>, headers: HeaderMap) -> Reply<serde_json::Value> {
    let u = session(&state, &headers).await?;
    let admins = state.dynamic.read().await.admin_emails.clone();
    let conn = state.store.conn.lock().await;
    let blockers = participation_blockers_tx(&conn, &u.user_id)?;
    let verified = match verified_tx(&conn, &u.user_id) {
        Ok(_) => true,
        Err(error) if error.code() == Some("C2C_IDENTITY") => false,
        Err(error) => return Err(error),
    };
    Ok(reply(
        serde_json::json!({"policy":policy_tx(&conn)?,"available":blockers.is_empty(),"blockers":blockers,
        "canManagePolicy":crate::api::is_finance_owner(&state.config,&u.email),
        "verified":verified,"termsVersion":TERMS_VERSION,"claimGraceSeconds":120,"firstPurchaseMaxMinor":10000,
        "independentDisputeHandlerAvailable":independent_handler_tx(&conn,&admins,state.config.router_owner_email.as_deref())?}),
    ))
}
async fn seller(State(state): State<ServerState>, headers: HeaderMap) -> Reply<serde_json::Value> {
    let u = session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let blockers = seller_blockers_tx(&conn, &u.user_id, &crate::market_wallet::now())?;
    let time = crate::market_wallet::now();
    let pending:i64=conn.query_row("SELECT COUNT(*) FROM market_c2c_orders WHERE seller_id=?1 AND status IN ('awaiting_seller','awaiting_payment','paid','disputed')",params![u.user_id],|r|r.get(0)).map_err(db_error("count C2C seller work"))?;
    let p = policy_tx(&conn)?;
    let own = seller_tx(&conn, &u.user_id)?;
    let available = stock_tx(&conn, &u.user_id, &time)?;
    let state_code = match &own {
        None => "not_selling",
        Some(s) if s.suspended => "suspended",
        Some(s) if !s.enabled => "paused",
        Some(s)
            if s.auto_accept_terms_version != TERMS_VERSION
                || s.auto_accept_consented_at.is_none() =>
        {
            "confirmation_required"
        }
        Some(s) if s.auto_pause_reason.is_some() => "auto_paused",
        Some(_) if !blockers.is_empty() => "unavailable",
        Some(s) if available < units_from_minor(s.min_minor.max(p.min_minor))? => {
            "insufficient_stock"
        }
        Some(_) if pending >= 10 => "order_limit",
        Some(_) => "selling",
    };
    Ok(reply(
        serde_json::json!({"seller":seller_tx(&conn,&u.user_id)?,
        "blockers":blockers,"canEnable":blockers.is_empty(),
        "state":state_code,"pendingOrders":pending,
        "admissionBalanceUnits":admission_balance_tx(&conn,&u.user_id,&crate::market_wallet::now())?.to_string(),
        "availableUnits":stock_tx(&conn,&u.user_id,&crate::market_wallet::now())?.to_string()}),
    ))
}
async fn save_seller(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<SellerInput>,
) -> Reply<Seller> {
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
    Ok(reply(
        write(&state, |conn| {
            save_seller_tx(conn, &u.user_id, &input, &crate::market_wallet::now())
        })
        .await?,
    ))
}
async fn retired_presence() -> StatusCode {
    StatusCode::GONE
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
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
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
async fn resume_seller(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PauseSeller>,
) -> Reply<Seller> {
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            resume_seller_tx(
                c,
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
    // Explain only public availability/ranges, never another seller's holds.
    let empty_reason = if items.is_empty() && next.is_none() {
        if !opening_blockers_tx(&conn)?.is_empty() {
            Some("platform_paused")
        } else {
            let others:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_c2c_sellers s JOIN users u ON u.id=s.user_id WHERE s.enabled=1 AND s.suspended=0 AND s.auto_pause_reason IS NULL AND s.auto_accept_terms_version=3 AND s.auto_accept_consented_at IS NOT NULL AND s.user_id!=?1 AND u.status='active' AND u.email_verified_at IS NOT NULL)",params![u.user_id],|r|r.get(0)).map_err(db_error("explain public C2C availability"))?;
            Some(if others {
                "no_matching_sellers"
            } else {
                "no_other_sellers"
            })
        }
    } else {
        None
    };
    Ok(reply(
        serde_json::json!({"items":items,"next":next,"emptyReason":empty_reason}),
    ))
}
async fn quote(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<QuoteInput>,
) -> Reply<Quote> {
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
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
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
    let key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ip = crate::client_meta::extract_client_metadata(&headers, peer)
        .ip
        .unwrap_or_else(|| peer.ip().to_string());
    // A successful replay remains available after finance was paused.
    Ok(reply(
        write(&state, |conn| {
            create_tx(
                conn,
                &u.user_id,
                &input,
                key,
                &ip,
                &crate::market_wallet::now(),
            )
        })
        .await?,
    ))
}
fn list_tx(conn: &Connection, user: Option<&str>, p: &Page) -> Result<serde_json::Value, AppError> {
    if !matches!(
        p.status.as_str(),
        "" | "active"
            | "exceptions"
            | "awaiting_seller"
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
        AND (?2='' OR o.status=?2 OR (?2='active' AND o.status IN ('awaiting_seller','awaiting_payment','paid','disputed')) OR (?2='exceptions' AND (o.status='disputed' OR (o.late_payment_reported=1 AND o.late_payment_resolved=0))))
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
async fn accept(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Reply<Order> {
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            let o = owned_tx(c, &id, &u.user_id)?;
            accept_tx(
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
async fn reject(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Reply<Order> {
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            let o = owned_tx(c, &id, &u.user_id)?;
            if o.seller_id != u.user_id {
                return Err(AppError::Forbidden(
                    "only the seller may reject an application".into(),
                ));
            }
            current(&o, &a)?;
            if o.status != "awaiting_seller" {
                return Err(problem(
                    "C2C_PAYMENT_HOLD",
                    "only an unpaid application may be rejected",
                ));
            }
            cancel_tx(c, &o, &u.user_id, false, &crate::market_wallet::now())
        })
        .await?,
    ))
}
async fn paid(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<Action>,
) -> Reply<Order> {
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
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
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
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
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
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
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
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
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReleaseInput {
    pub(super) expected_version: i64,
    pub(super) acknowledge_received: bool,
}
pub(super) fn release_authorized_tx(
    c: &Connection,
    authorization: &crate::models::AuthSession,
    user: &str,
    id: &str,
    a: &ReleaseInput,
    time: &str,
) -> Result<Order, AppError> {
    let o = owned_tx(c, id, user)?;
    if authorization.user_id != user || o.seller_id != user || !a.acknowledge_received {
        return Err(AppError::Forbidden(
            "only the logged-in seller who verified receipt may release".into(),
        ));
    }
    verified_tx(c, user)?;
    // Retry a committed release without requiring the old order version.
    if o.status == "released" {
        return Ok(o);
    }
    if o.version != a.expected_version {
        return Err(problem(
            "C2C_STALE",
            "order changed; review it before releasing",
        ));
    }
    if !matches!(o.status.as_str(), "paid" | "disputed")
        || (o.flow_version == 2 && o.accepted_at.is_none())
    {
        return Err(problem(
            "C2C_PAYMENT_HOLD",
            "payment must be declared for an accepted order before release",
        ));
    }
    let fingerprint = format!(
        "{}:{}:{}:{}",
        o.seller_id, o.buyer_id, o.amount_units, o.quote_id
    );
    let proof = crate::market_finance_auth::record_tx(
        c,
        authorization,
        user,
        "c2c_release",
        &o.id,
        &o.buyer_id,
        o.amount_units,
        o.version,
        &fingerprint,
        "receipt_confirmed",
        time,
    )?;
    let changed=c.execute("UPDATE market_c2c_orders SET release_authorization_id=?3 WHERE id=?1 AND version=?2 AND release_authorization_id IS NULL",
        params![o.id,o.version,proof]).map_err(db_error("bind C2C session authorization"))?;
    if changed != 1 {
        return Err(problem(
            "C2C_STALE",
            "order changed; review it before releasing",
        ));
    }
    release_tx(c, &o, user, time)
}
async fn release(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(a): Json<ReleaseInput>,
) -> Reply<Order> {
    let u = crate::market_finance_auth::write_session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            release_authorized_tx(c, &u, &u.user_id, &id, &a, &crate::market_wallet::now())
        })
        .await?,
    ))
}
async fn admin_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Reply<serde_json::Value> {
    let u = crate::api::require_admin_session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    Ok(reply(
        serde_json::json!({"policy":policy_tx(&c)?,"canManage":crate::api::is_finance_owner(&state.config,&u.email),"canResolve":crate::api::is_finance_owner(&state.config,&u.email)||dispute_handler_tx(&c,&u.user_id)?}),
    ))
}
fn independent_handler_tx(
    c: &Connection,
    admins: &std::collections::HashSet<String>,
    owner: Option<&str>,
) -> Result<bool, AppError> {
    let mut stmt=c.prepare("SELECT u.email_normalized FROM market_c2c_dispute_handlers h JOIN users u ON u.id=h.user_id WHERE h.enabled=1 AND u.status='active' AND u.email_verified_at IS NOT NULL").map_err(db_error("prepare independent C2C handler"))?;
    let emails = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(db_error("query independent C2C handler"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read independent C2C handler"))?;
    Ok(emails
        .iter()
        .any(|email| admins.contains(email) && owner != Some(email.as_str())))
}
async fn handlers(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(p): Query<Page>,
) -> Reply<serde_json::Value> {
    crate::api::require_owner_session(&state, &headers).await?;
    let admins = state.dynamic.read().await.admin_emails.clone();
    let c = state.store.conn.lock().await;
    let mut stmt=c.prepare("SELECT h.user_id,u.email_normalized,h.version,h.enabled,h.updated_at FROM market_c2c_dispute_handlers h JOIN users u ON u.id=h.user_id WHERE h.user_id>?1 ORDER BY h.user_id LIMIT 51").map_err(db_error("prepare C2C handlers"))?;
    let mut items=stmt.query_map(params![p.before],|r|{let email:String=r.get(1)?;Ok(serde_json::json!({"userId":r.get::<_,String>(0)?,"email":email,"version":r.get::<_,i64>(2)?,"enabled":r.get::<_,bool>(3)?,"updatedAt":r.get::<_,String>(4)?,"isAdmin":admins.contains(&email)}))}).map_err(db_error("query C2C handlers"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read C2C handlers"))?;
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
struct HandlerInput {
    email: String,
    expected_version: i64,
    enabled: bool,
}
async fn save_handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<HandlerInput>,
) -> Reply<serde_json::Value> {
    let u = crate::market_finance_auth::write_owner_session(&state, &headers).await?;
    let email = input.email.trim().to_ascii_lowercase();
    if input.enabled
        && (!state.dynamic.read().await.is_admin(&email)
            || crate::api::is_finance_owner(&state.config, &email))
    {
        return Err(AppError::BadRequest(
            "grant only to another existing administrator".into(),
        ));
    }
    write(&state, |c| {
        let user: String = c
            .query_row(
                "SELECT id FROM users WHERE email_normalized=?1",
                params![email],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("find C2C dispute administrator"))?
            .ok_or_else(|| AppError::NotFound("administrator must first sign in".into()))?;
        set_dispute_handler_tx(
            c,
            &u.user_id,
            &user,
            input.expected_version,
            input.enabled,
            &crate::market_wallet::now(),
        )
    })
    .await?;
    Ok(reply(serde_json::json!({"ok":true})))
}
async fn save_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(p): Json<Policy>,
) -> Reply<Policy> {
    let u = crate::market_finance_auth::write_owner_session(&state, &headers).await?;
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
    let u = crate::api::require_admin_session(&state, &headers).await?;
    let c = state.store.conn.lock().await;
    let o = order_tx(&c, &id)?;
    let can_resolve = (crate::api::is_finance_owner(&state.config, &u.email)
        || dispute_handler_tx(&c, &u.user_id)?)
        && u.user_id != o.buyer_id
        && u.user_id != o.seller_id;
    Ok(reply(
        serde_json::json!({"order":o,"events":events_tx(&c,&id,"")?,"evidenceValid":evidence_valid_tx(&c,&id)?,"canResolve":can_resolve}),
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
    let u = crate::market_finance_auth::write_admin_session(&state, &headers).await?;
    Ok(reply(
        write(&state, |c| {
            if !crate::api::is_finance_owner(&state.config, &u.email)
                && !dispute_handler_tx(c, &u.user_id)?
            {
                return Err(AppError::Forbidden(
                    "C2C dispute-handler authorization required".into(),
                ));
            }
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
    let mut s=c.prepare("SELECT user_id,version,display_name,enabled,suspended,auto_pause_reason FROM market_c2c_sellers WHERE user_id>?1 ORDER BY user_id LIMIT 51").map_err(db_error("prepare admin C2C sellers"))?;
    let mut items=s.query_map(params![p.before],|r|Ok(serde_json::json!({"userId":r.get::<_,String>(0)?,"version":r.get::<_,i64>(1)?,"displayName":r.get::<_,String>(2)?,"enabled":r.get::<_,bool>(3)?,"suspended":r.get::<_,bool>(4)?,"autoPauseReason":r.get::<_,Option<String>>(5)?}))).map_err(db_error("query admin C2C sellers"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read admin C2C sellers"))?;
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
    let u = crate::market_finance_auth::write_owner_session(&state, &headers).await?;
    write(&state,|c| {
        let reason=text(&a.reason,500)?;
        let count=c.execute("UPDATE market_c2c_sellers SET suspended=?3,enabled=0,auto_pause_reason=NULL,auto_paused_at=NULL,auto_pause_order_id=NULL,version=version+1,updated_at=?4 WHERE user_id=?1 AND version=?2",params![id,a.expected_version,a.suspended,crate::market_wallet::now()]).map_err(db_error("suspend C2C seller"))?;
        if count!=1{return Err(problem("C2C_STALE","seller changed; reload"));}
        audit_tx(c,&u.user_id,if a.suspended{"suspend"}else{"unsuspend"},"c2c_seller",&id,&reason,&crate::market_wallet::now())?;
        crate::market_notifications::enqueue_user_notification_tx(c,"buyer",&id,"c2c_seller_changed","c2c_seller",&id,&format!("c2c-suspension:{id}:{}",a.expected_version+1),serde_json::json!({"reason":if a.suspended{"suspended"}else{"unsuspended"}}),&crate::market_wallet::now())?;Ok(())
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
