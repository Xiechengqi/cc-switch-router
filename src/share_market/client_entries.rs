//! Read-only Client-page projections. Occupancy and an old settlement are
//! deliberately independent; neither projection can grant or charge anything.
use super::*;

const MAX_IDS: usize = 100;
const PAGE_SIZE: usize = 20;
// Keep this predicate shared by summaries, recovery cards and detail pages.
const RENTAL_CONTEXT: &str = "(sub.status NOT IN ('released','grant_failed')
 OR EXISTS(SELECT 1 FROM share_market_terminations t
     WHERE t.subscription_id=sub.id AND t.buyer_user_id=sub.renter_user_id AND t.status!='completed')
 OR EXISTS(SELECT 1 FROM market_platform_contracts c
     JOIN market_service_disputes d ON d.contract_id=c.id AND d.status='open'
     WHERE c.product_kind='share' AND c.product_ref=sub.id))";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClientEntry {
    pub share_id: String,
    pub entry_kind: String,
    pub vacant_seats: i64,
    pub subdomain: Option<String>,
    pub resource_state: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EntryPage {
    pub items: Vec<ClientEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SeatContext {
    pub share_id: String,
    pub resource_state: String,
    pub listing: Option<ListingView>,
    pub subscriptions: Vec<SubscriptionView>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OwnerSeatContext {
    pub share_id: String,
    pub listings: Vec<ListingView>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EntryQuery {
    #[serde(default)]
    share_ids: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContextQuery {
    cursor: Option<String>,
    subscription_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContextCursor {
    actor: String,
    scope: String,
    after: String,
}

fn cursor_after(cursor: Option<&str>, actor: &str, scope: &str) -> Result<String, AppError> {
    let Some(cursor) = cursor else {
        return Ok(String::new());
    };
    let invalid = || AppError::BadRequest("invalid Client rental cursor".into());
    if cursor.len() > 2048 {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
    let value: ContextCursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if value.actor != actor
        || value.scope != scope
        || value.after.is_empty()
        || value.after.len() > 256
    {
        return Err(invalid());
    }
    Ok(value.after)
}

fn next_cursor(actor: &str, scope: &str, after: &str) -> Result<String, AppError> {
    serde_json::to_vec(&ContextCursor {
        actor: actor.into(),
        scope: scope.into(),
        after: after.into(),
    })
    .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
    .map_err(|e| AppError::Internal(format!("encode Client rental cursor: {e}")))
}

pub(super) fn parse_share_ids(raw: &str) -> Result<Vec<String>, AppError> {
    if raw.len() > 26_000 {
        return Err(AppError::BadRequest("too many Share IDs".into()));
    }
    let ids = raw.split(',').filter(|s| !s.is_empty()).collect::<Vec<_>>();
    if ids.len() > MAX_IDS
        || ids
            .iter()
            .any(|id| id.len() > 256 || id.trim() != *id || id.chars().any(char::is_control))
    {
        return Err(AppError::BadRequest(
            "invalid Share IDs (maximum 100)".into(),
        ));
    }
    Ok(ids
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

pub(super) fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/share-market/client-entries", get(entries))
        .route("/v1/share-market/me/client-rental-contexts", get(contexts))
        .route("/v1/share-market/shares/:id/seat-context", get(seats))
        .route(
            "/v1/share-market/shares/:id/owner-seat-context",
            get(owner_seats),
        )
}

async fn entries(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(q): Query<EntryQuery>,
) -> Result<Response, AppError> {
    let viewer = crate::api::resolve_router_session(&state, &headers).await?;
    let ids = parse_share_ids(&q.share_ids)?;
    let items = state
        .store
        .client_share_entries(viewer.as_ref(), &ids)
        .await?;
    client_context_json(
        &headers,
        &EntryPage {
            items,
            next_cursor: None,
        },
    )
}

async fn contexts(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(q): Query<ContextQuery>,
) -> Result<Response, AppError> {
    let viewer = require_session(&state, &headers).await?;
    let page = state
        .store
        .client_rental_contexts(&viewer, q.cursor.as_deref())
        .await?;
    client_context_json(&headers, &page)
}

async fn seats(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ContextQuery>,
) -> Result<Response, AppError> {
    let viewer = crate::api::resolve_router_session(&state, &headers).await?;
    let online = state.proxy.active_subdomains().await;
    let context = state
        .store
        .client_seat_context(
            viewer.as_ref(),
            &id,
            &online,
            q.cursor.as_deref(),
            q.subscription_id.as_deref(),
        )
        .await?;
    client_context_json(&headers, &context)
}

async fn owner_seats(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let viewer = require_session(&state, &headers).await?;
    let online = state.proxy.active_subdomains().await;
    let context = state
        .store
        .client_owner_seat_context(&viewer, &id, &online)
        .await?;
    client_context_json(&headers, &context)
}

fn client_context_json<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<Response, AppError> {
    let mut response = private_etag_json(headers, value)?;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

fn entries_tx(
    conn: &Connection,
    viewer: Option<&AuthSession>,
    ids: &[String],
) -> Result<Vec<ClientEntry>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids_json = serde_json::to_string(ids).map_err(|e| AppError::Internal(e.to_string()))?;
    let actor = viewer.map(|v| v.user_id.as_str()).unwrap_or("");
    let public = publicly_listed_sql("s");
    let sql = format!("SELECT requested.value, s.subdomain, s.share_status, s.user_grants_json,
        EXISTS(SELECT 1 FROM share_market_subscriptions sub WHERE sub.share_id=requested.value
            AND sub.renter_user_id=?2 AND sub.status NOT IN ('released','grant_failed')),
        EXISTS(SELECT 1 FROM share_market_subscriptions sub WHERE sub.share_id=requested.value
            AND sub.renter_user_id=?2 AND {RENTAL_CONTEXT}),
        (SELECT COUNT(*) FROM share_market_seats seat JOIN share_market_listings listing ON listing.id=seat.listing_id
            WHERE listing.share_id=requested.value AND listing.deleted_at IS NULL AND ({public})
            AND seat.status='available' AND seat.retired_at IS NULL AND seat.current_subscription_id IS NULL
            AND (COALESCE(seat.cycle_price_minor,seat.daily_rate_minor) IS NULL OR seat.funding_rail='platform_wallet')),
        EXISTS(SELECT 1 FROM share_market_listings listing WHERE listing.share_id=requested.value
            AND listing.deleted_at IS NULL AND ({public})),
        EXISTS(SELECT 1 FROM share_market_listings listing WHERE listing.share_id=requested.value
            AND (listing.owner_user_id=?2 OR lower(listing.owner_email)=lower(?3))),
        EXISTS(SELECT 1 FROM share_market_listings listing WHERE listing.share_id=requested.value
            AND listing.deleted_at IS NULL AND listing.owner_user_id=?2
            AND lower(listing.owner_email)=lower(?3) AND lower(COALESCE(s.owner_email,''))=lower(?3))
        FROM json_each(?1) requested LEFT JOIN shares s ON s.share_id=requested.value ORDER BY requested.value");
    conn.prepare(&sql)
        .and_then(|mut st| {
            st.query_map(
                params![
                    ids_json,
                    actor,
                    viewer.map(|v| v.email.as_str()).unwrap_or("")
                ],
                |r| {
                    let live: bool = r.get(4)?;
                    let pending: bool = r.get(5)?;
                    let vacant: i64 = r.get(6)?;
                    let public: bool = r.get(7)?;
                    let owner: bool = r.get(8)?;
                    let manage = viewer.is_some() && r.get::<_, bool>(9)?;
                    let grants: BTreeMap<String, ShareUserGrant> = r
                        .get::<_, Option<String>>(3)?
                        .and_then(|v| serde_json::from_str(&v).ok())
                        .unwrap_or_default();
                    let direct = viewer.is_some_and(|v| {
                        grants
                            .get(&v.email.to_ascii_lowercase())
                            .is_some_and(|g| g.active)
                    });
                    let kind = if manage {
                        "manage"
                    } else if live {
                        "rental"
                    } else if pending {
                        "settlement"
                    } else if public && vacant > 0 && !owner && !direct {
                        "rent"
                    } else {
                        "none"
                    };
                    let status: Option<String> = r.get(2)?;
                    // Private resource existence/identity is not exposed just by guessing IDs.
                    let related = live || pending || manage;
                    Ok(ClientEntry {
                        share_id: r.get(0)?,
                        entry_kind: kind.into(),
                        vacant_seats: if public { vacant } else { 0 },
                        subdomain: if public || related { r.get(1)? } else { None },
                        resource_state: if public {
                            "listed"
                        } else if related {
                            match status.as_deref() {
                                None => "missing",
                                Some("active") => "unlisted",
                                _ => "inactive",
                            }
                        } else {
                            "unavailable"
                        }
                        .into(),
                    })
                },
            )?
            .collect::<crate::db::Result<Vec<_>>>()
        })
        .map_err(map_db("read Client Share entries"))
}

impl AppStore {
    pub(super) async fn client_owner_seat_context(
        &self,
        viewer: &AuthSession,
        id: &str,
        online: &[String],
    ) -> Result<OwnerSeatContext, AppError> {
        if id.is_empty() || id.len() > 256 {
            return Err(AppError::BadRequest("invalid Share ID".into()));
        }
        let ids = [id.to_owned()];
        let verify = |conn: &Connection| -> Result<(), AppError> {
            if entries_tx(conn, Some(viewer), &ids)?
                .iter()
                .any(|entry| entry.entry_kind == "manage")
            {
                Ok(())
            } else {
                Err(AppError::NotFound("Share seats not found".into()))
            }
        };
        {
            let conn = self.conn.lock().await;
            verify(&conn)?;
        }
        // The public seat context intentionally strips subscriptions and mutation
        // capabilities. Use the existing Owner projection, scoped to one Share.
        let catalog = self
            .share_market_catalog_filtered(
                Some(viewer),
                online,
                ShareMarketCatalogScope::Owner,
                Some(&ids),
            )
            .await?;
        // Ownership may have changed while building the projection.
        let conn = self.conn.lock().await;
        verify(&conn)?;
        Ok(OwnerSeatContext {
            share_id: id.into(),
            listings: catalog
                .listings
                .into_iter()
                .filter(|listing| listing.owner_email.eq_ignore_ascii_case(&viewer.email))
                .collect(),
        })
    }

    pub(super) async fn client_share_entries(
        &self,
        viewer: Option<&AuthSession>,
        ids: &[String],
    ) -> Result<Vec<ClientEntry>, AppError> {
        if ids.len() > MAX_IDS {
            return Err(AppError::BadRequest("too many Share IDs".into()));
        }
        let conn = self.conn.lock().await;
        entries_tx(&conn, viewer, ids)
    }

    pub(super) async fn client_rental_contexts(
        &self,
        viewer: &AuthSession,
        cursor: Option<&str>,
    ) -> Result<EntryPage, AppError> {
        let after = cursor_after(cursor, &viewer.user_id, "shares")?;
        let conn = self.conn.lock().await;
        let sql = format!(
            "SELECT DISTINCT sub.share_id FROM share_market_subscriptions sub
            WHERE sub.renter_user_id=?1 AND {RENTAL_CONTEXT} AND sub.share_id>?2
            ORDER BY sub.share_id LIMIT ?3"
        );
        let mut ids = conn
            .prepare(&sql)
            .and_then(|mut st| {
                st.query_map(params![viewer.user_id, after, PAGE_SIZE + 1], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<crate::db::Result<Vec<_>>>()
            })
            .map_err(map_db("read private Client rental contexts"))?;
        let more = ids.len() > PAGE_SIZE;
        ids.truncate(PAGE_SIZE);
        let next = if more {
            ids.last()
                .map(|id| next_cursor(&viewer.user_id, "shares", id))
                .transpose()?
        } else {
            None
        };
        Ok(EntryPage {
            items: entries_tx(&conn, Some(viewer), &ids)?,
            next_cursor: next,
        })
    }

    pub(super) async fn client_seat_context(
        &self,
        viewer: Option<&AuthSession>,
        id: &str,
        online: &[String],
        cursor: Option<&str>,
        selected: Option<&str>,
    ) -> Result<SeatContext, AppError> {
        if id.is_empty() || id.len() > 256 {
            return Err(AppError::BadRequest("invalid Share ID".into()));
        }
        let actor = viewer.map(|v| v.user_id.as_str()).unwrap_or("");
        let after = cursor_after(cursor, actor, id)?;
        let mut catalog = self
            .share_market_catalog_filtered(
                viewer,
                online,
                ShareMarketCatalogScope::Public,
                Some(&[id.to_owned()]),
            )
            .await?;
        retain_public_catalog(&mut catalog);
        let listing = catalog.listings.into_iter().next().map(|mut l| {
            l.can_delete = false;
            l.can_reopen = false;
            for seat in &mut l.seats {
                seat.subscription = None;
                seat.can_delete = false;
                seat.can_republish = false;
            }
            l
        });
        let conn = self.conn.lock().await;
        let own_sql = format!(
            "SELECT sub.id FROM share_market_subscriptions sub WHERE sub.renter_user_id=?1 AND sub.share_id=?2 AND {RENTAL_CONTEXT} AND sub.id>?3 ORDER BY sub.id LIMIT ?4"
        );
        let mut ids = conn
            .prepare(&own_sql)
            .and_then(|mut st| {
                st.query_map(params![actor, id, after, PAGE_SIZE + 1], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<crate::db::Result<Vec<_>>>()
            })
            .map_err(map_db("read private seat contexts"))?;
        let more = ids.len() > PAGE_SIZE;
        ids.truncate(PAGE_SIZE);
        let next = if more {
            ids.last()
                .map(|sub| next_cursor(actor, id, sub))
                .transpose()?
        } else {
            None
        };
        // Occupancy must be visible even if many older unresolved settlements
        // sort before it. Paging history must never enable a second rental.
        let live: Option<String> = conn.query_row("SELECT id FROM share_market_subscriptions WHERE renter_user_id=?1 AND share_id=?2 AND status NOT IN ('released','grant_failed') LIMIT 1", params![actor, id], |r| r.get(0)).optional().map_err(map_db("read current Client seat"))?;
        if let Some(live) = live {
            if !ids.contains(&live) {
                ids.insert(0, live);
            }
        }
        if let Some(selected) = selected {
            let own: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM share_market_subscriptions WHERE id=?1 AND share_id=?2 AND renter_user_id=?3)", params![selected, id, actor], |r| r.get(0)).map_err(map_db("verify selected rental"))?;
            if !own {
                return Err(AppError::NotFound("rental not found".into()));
            }
            if !ids.iter().any(|s| s == selected) {
                ids.insert(0, selected.into());
            }
        }
        let entry = entries_tx(&conn, viewer, &[id.to_owned()])?
            .into_iter()
            .next()
            .expect("one requested entry");
        if listing.is_none() && entry.entry_kind == "none" && ids.is_empty() {
            return Err(AppError::NotFound("Share seats not found".into()));
        }
        let active = online.iter().cloned().collect();
        let mut records = Vec::new();
        for sub in ids {
            if let Some(record) = subscription_record(&conn, &sub)? {
                records.push(record);
            }
        }
        let profiles = provider_contacts(
            &conn,
            &records
                .iter()
                .map(|r| r.owner_user_id.clone())
                .collect::<Vec<_>>(),
        )?;
        let subscriptions = records
            .into_iter()
            .map(|r| {
                let profile = profiles.get(&r.owner_user_id);
                subscription_view(&conn, r, viewer, &active, profile)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SeatContext {
            share_id: id.into(),
            resource_state: entry.resource_state,
            listing,
            subscriptions,
            next_cursor: next,
        })
    }
}
