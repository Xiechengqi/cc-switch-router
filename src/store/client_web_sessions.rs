//! Client-scoped browser delegation. Long-lived Router credentials never leave its origin.
use super::*;
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};

pub(crate) const WINDOW_HEADER: &str = "x-cc-switch-client-web-session";
const MAX_SESSIONS: usize = 16_384;

#[derive(Clone)]
struct WindowGrant {
    session_id: String,
    installation_id: String,
    subdomain: String,
    owner: String,
    revision: i64,
    binding_epoch: String,
    user_id: String,
    delegate: bool,
    terminal: bool,
    nonce: String,
    expires_at: DateTime<Utc>,
}

struct ClientWindowTarget {
    buyer_id: Option<String>,
    subdomain: String,
    owner: String,
    revision: i64,
    binding_epoch: String,
}

#[derive(Default)]
pub(crate) struct WindowSessions {
    tickets: HashMap<String, WindowGrant>,
    sessions: HashMap<String, WindowGrant>,
}

impl WindowSessions {
    fn prune(&mut self) {
        let now = Utc::now();
        self.tickets.retain(|_, grant| grant.expires_at > now);
        self.sessions.retain(|_, grant| grant.expires_at > now);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TicketRequest {
    installation_id: String,
    kind: String,
    nonce: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeRequest {
    ticket: String,
    nonce: String,
}

pub(crate) fn router() -> Router<crate::ServerState> {
    Router::new()
        .route("/v1/auth/client-web-tickets", post(issue_ticket))
        .route("/web-api/auth/router-exchange", post(exchange_ticket))
        .route("/web-api/auth/router-logout", post(logout))
}

fn authority(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)?
        .to_str()
        .ok()
        .map(str::to_ascii_lowercase)
}

fn origin_for(config: &Config, subdomain: &str) -> Result<String, AppError> {
    url::Url::parse(&config.tunnel_url(subdomain))
        .map(|url| url.origin().ascii_serialization())
        .map_err(|_| AppError::Internal("invalid Client public URL".into()))
}

fn host_matches(config: &Config, headers: &HeaderMap, subdomain: &str) -> bool {
    let Ok(url) = url::Url::parse(&config.tunnel_url(subdomain)) else {
        return false;
    };
    let expected = url[url::Position::BeforeHost..url::Position::AfterPort].to_ascii_lowercase();
    authority(headers).as_deref() == Some(expected.as_str())
}

impl AppStore {
    async fn client_window_target(
        &self,
        installation_id: &str,
    ) -> Result<ClientWindowTarget, AppError> {
        let conn = self.conn.lock().await;
        let online=conn.query_row("SELECT r.alias,r.tenant_email,p.revision,r.id,r.buyer_id FROM online_client_leases r JOIN public_hosts p ON p.label=r.alias AND p.subject_id=r.id
            WHERE r.id=?1 AND r.state='active' AND r.grant_until>unixepoch() AND p.lifecycle='active'",params![installation_id],
            |row| Ok(ClientWindowTarget{buyer_id:row.get(4)?,subdomain:row.get(0)?,owner:row.get(1)?,revision:row.get(2)?,binding_epoch:format!("lease:{}",row.get::<_,String>(3)?)})).optional()
            .map_err(|e|AppError::Internal(format!("read lease window: {e}")))?;
        if let Some(online) = online {
            return Ok(online);
        }
        conn.query_row(
            "SELECT t.subdomain, LOWER(TRIM(i.owner_email)), p.revision,
                    i.owner_verified_at
             FROM installations i
             JOIN installation_client_tunnels t ON t.installation_id = i.id AND t.enabled = 1
             JOIN public_hosts p ON p.kind = 'client' AND p.subject_id = i.id
                  AND p.label = t.subdomain AND p.lifecycle = 'active'
             WHERE i.id = ?1 AND i.lifecycle = 'active' AND i.client_activated_at IS NOT NULL
               AND i.owner_verified_at IS NOT NULL AND i.owner_email IS NOT NULL",
            params![installation_id],
            |row| {
                Ok(ClientWindowTarget {
                    buyer_id: None,
                    subdomain: row.get(0)?,
                    owner: row.get(1)?,
                    revision: row.get(2)?,
                    binding_epoch: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("read Client login target: {e}")))?
        .ok_or_else(|| AppError::Forbidden("Client login is unavailable".into()))
    }

    async fn validate_window(&self, grant: &WindowGrant) -> Result<AuthSession, AppError> {
        if grant.expires_at <= Utc::now() {
            return Err(AppError::Unauthorized("Client session expired".into()));
        }
        let actor = {
            let conn = self.conn.lock().await;
            let hash: Option<String> = conn
                .query_row(
                    "SELECT access_token_hash FROM user_sessions WHERE id = ?1",
                    params![grant.session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| AppError::Internal(format!("read parent session: {e}")))?;
            hash.map(|hash| get_session_by_access_hash(&conn, &hash))
                .transpose()?
                .flatten()
        }
        .filter(|actor| {
            actor.user_id == grant.user_id
                && actor.access_expires_at > Utc::now()
                && actor.refresh_expires_at > Utc::now()
        })
        .ok_or_else(|| AppError::Unauthorized("Router login expired".into()))?;
        let ClientWindowTarget {
            buyer_id,
            subdomain,
            owner,
            revision,

            binding_epoch,
        } = self.client_window_target(&grant.installation_id).await?;
        if buyer_id.as_deref().is_some_and(|id| id != actor.user_id)
            || subdomain != grant.subdomain
            || owner != grant.owner
            || revision != grant.revision
            || binding_epoch != grant.binding_epoch
            || !owner.eq_ignore_ascii_case(&actor.email)
            || (grant.terminal && buyer_id.is_some())
        {
            return Err(AppError::Forbidden("Client owner or route changed".into()));
        }
        Ok(actor)
    }

    pub(crate) async fn client_window_principal(
        &self,
        config: &Config,
        headers: &HeaderMap,
        installation_id: &str,
        subdomain: &str,
    ) -> Result<Option<(String, bool)>, AppError> {
        let Some(token) = headers.get(WINDOW_HEADER) else {
            return Ok(None);
        };
        let token = token
            .to_str()
            .ok()
            .filter(|token| token.len() <= 256)
            .ok_or_else(|| AppError::Unauthorized("invalid Client session".into()))?;
        let grant = {
            let mut windows = self.client_web_sessions.lock().await;
            windows.prune();
            windows.sessions.get(&hash_token(token)).cloned()
        }
        .ok_or_else(|| AppError::Unauthorized("Client session expired".into()))?;
        if grant.installation_id != installation_id
            || grant.subdomain != subdomain
            || !host_matches(config, headers, subdomain)
        {
            return Err(AppError::Forbidden("Client session target mismatch".into()));
        }
        let actor = self
            .validate_window(&grant)
            .await
            .map_err(|error| match error {
                AppError::Forbidden(_) => {
                    AppError::Unauthorized("Client session binding changed".into())
                }
                error => error,
            })?;
        Ok(grant.delegate.then_some((actor.email, false)))
    }
}

async fn issue_ticket(
    State(state): State<crate::ServerState>,
    headers: HeaderMap,
    Json(input): Json<TicketRequest>,
) -> Result<Response, AppError> {
    let actor = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    if !matches!(input.kind.as_str(), "console" | "terminal")
        || Uuid::parse_str(&input.nonce).is_err()
    {
        return Err(AppError::BadRequest("invalid Client login request".into()));
    }
    let ClientWindowTarget {
        buyer_id,
        subdomain,
        owner,
        revision,

        binding_epoch,
    } = state
        .store
        .client_window_target(&input.installation_id)
        .await?;
    let delegate = actor.email.eq_ignore_ascii_case(&owner)
        && buyer_id.as_deref().is_none_or(|id| id == actor.user_id);
    if !delegate || (input.kind == "terminal" && buyer_id.is_some()) {
        return Err(AppError::Forbidden("Client owner login required".into()));
    }
    let terminal = buyer_id.is_none();
    let grant = WindowGrant {
        session_id: actor.session_id,
        installation_id: input.installation_id,
        subdomain,
        owner,
        revision,
        binding_epoch,
        user_id: actor.user_id,
        delegate,
        terminal,
        nonce: input.nonce,
        expires_at: Utc::now() + Duration::seconds(60),
    };
    state.store.validate_window(&grant).await?;
    let origin = origin_for(&state.config, &grant.subdomain)?;
    let mut windows = state.store.client_web_sessions.lock().await;
    windows.prune();
    if windows
        .tickets
        .values()
        .chain(windows.sessions.values())
        .filter(|item| item.session_id == grant.session_id)
        .count()
        >= 64
    {
        return Err(AppError::TooManyRequests(
            "too many Client windows for this login".into(),
        ));
    }
    if windows.tickets.len() + windows.sessions.len() >= MAX_SESSIONS {
        return Err(AppError::TooManyRequests("too many Client windows".into()));
    }
    let ticket = generate_secret(48);
    windows.tickets.insert(hash_token(&ticket), grant);
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"ticket":ticket,"origin":origin})),
    )
        .into_response())
}

async fn exchange_ticket(
    State(state): State<crate::ServerState>,
    headers: HeaderMap,
    Json(input): Json<ExchangeRequest>,
) -> Result<Response, AppError> {
    if input.ticket.len() > 256 {
        return Err(AppError::Unauthorized("invalid ticket".into()));
    }
    let key = hash_token(&input.ticket);
    let mut grant = {
        let windows = state.store.client_web_sessions.lock().await;
        windows.tickets.get(&key).cloned()
    }
    .ok_or_else(|| AppError::Unauthorized("ticket expired or already used".into()))?;
    if grant.nonce != input.nonce
        || !host_matches(&state.config, &headers, &grant.subdomain)
        || headers
            .get(header::ORIGIN)
            .and_then(|origin| origin.to_str().ok())
            != Some(origin_for(&state.config, &grant.subdomain)?.as_str())
    {
        return Err(AppError::Forbidden("ticket target mismatch".into()));
    }
    let actor = state.store.validate_window(&grant).await?;
    let mut windows = state.store.client_web_sessions.lock().await;
    windows.prune();
    if windows.tickets.remove(&key).is_none() {
        return Err(AppError::Unauthorized("ticket already used".into()));
    }
    grant.expires_at = std::cmp::min(actor.access_expires_at, Utc::now() + Duration::minutes(10));
    let token = generate_secret(48);
    let expires_at = grant.expires_at;
    windows.sessions.insert(hash_token(&token), grant);
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"token":token,"expiresAt":expires_at})),
    )
        .into_response())
}

async fn logout(State(state): State<crate::ServerState>, headers: HeaderMap) -> StatusCode {
    if let Some(token) = headers
        .get(WINDOW_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        state
            .store
            .client_web_sessions
            .lock()
            .await
            .sessions
            .remove(&hash_token(token));
    }
    StatusCode::NO_CONTENT
}

/// Recheck delegated event streams so logout, lease termination and ownership changes
/// also close already-open terminal output, instead of just denying later input.
pub(crate) async fn middleware(
    State(state): State<crate::ServerState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let headers = request.headers().clone();
    let mut response = next.run(request).await;
    if !headers.contains_key(WINDOW_HEADER)
        || !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"))
    {
        return response;
    }
    let Some(token) = headers.get(WINDOW_HEADER).and_then(|v| v.to_str().ok()) else {
        return response;
    };
    let token_hash = hash_token(token);
    let body = std::mem::replace(response.body_mut(), Body::empty());
    let stream = async_stream::stream! {
        let mut body = body.into_data_stream();
        let mut interval = tokio::time::interval(StdDuration::from_secs(5));
        loop {
            tokio::select! {
                item = body.next() => match item { Some(item) => yield item, None => break },
                _ = interval.tick() => {
                    let grant = state.store.client_web_sessions.lock().await.sessions.get(&token_hash).cloned();
                    let Some(grant) = grant else { break };
                    if state.store.validate_window(&grant).await.is_err() { break; }
                }
            }
        }
    };
    *response.body_mut() = Body::from_stream(stream);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (AppStore, Config, WindowGrant) {
        let store = AppStore::new_in_memory_for_tests().unwrap();
        let mut config = Config::from_env();
        config.use_localhost = false;
        config.tunnel_domain = "router.test".into();
        config.router_owner_email = Some("operator@example.test".into());
        let now = Utc::now().to_rfc3339();
        let actor = {
            let conn = store.conn.lock().await;
            let actor = crate::market_finance_auth::test_session(&conn, "owner");
            conn.execute("INSERT INTO installations(id,public_key,platform,app_version,owner_email,owner_verified_at,created_at,last_seen_at,client_activated_at,control_secret_b64) VALUES('installation','public','linux','1',?1,?2,?2,?2,?2,'control')", params![actor.email, now]).unwrap();
            conn.execute("INSERT INTO installation_client_tunnels(installation_id,owner_email,subdomain,enabled,created_at,updated_at,last_seen_at) VALUES('installation',?1,'client-alpha',1,?2,?2,?2)", params![actor.email,now]).unwrap();
            conn.execute("INSERT INTO public_hosts(label,route_id,kind,subject_id,installation_id,target_lane_id,lifecycle,revision,created_at,updated_at) VALUES('client-alpha','client:installation','client','installation','installation','installation','active',1,?1,?1)",params![now]).unwrap();
            actor
        };
        let grant = WindowGrant {
            session_id: actor.session_id,
            installation_id: "installation".into(),
            subdomain: "client-alpha".into(),
            owner: actor.email,
            revision: 1,
            binding_epoch: now,
            user_id: actor.user_id,
            delegate: true,
            terminal: true,
            nonce: Uuid::new_v4().to_string(),
            expires_at: Utc::now() + Duration::minutes(10),
        };
        (store, config, grant)
    }

    #[tokio::test]
    async fn client_window_checks_owner_session_route_and_host() {
        let (store, config, grant) = fixture().await;
        assert!(store.validate_window(&grant).await.is_ok());
        store
            .client_web_sessions
            .lock()
            .await
            .sessions
            .insert(hash_token("window-token"), grant.clone());
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "client-alpha.router.test".parse().unwrap());
        headers.insert(WINDOW_HEADER, "window-token".parse().unwrap());
        assert_eq!(
            store
                .client_window_principal(&config, &headers, "installation", "client-alpha")
                .await
                .unwrap(),
            Some(("owner@example.test".into(), false))
        );
        assert!(
            store
                .client_window_principal(&config, &headers, "other-installation", "client-alpha")
                .await
                .is_err()
        );
        headers.insert(header::HOST, "another.router.test".parse().unwrap());
        assert!(
            store
                .client_window_principal(&config, &headers, "installation", "client-alpha")
                .await
                .is_err()
        );
        store
            .conn
            .lock()
            .await
            .execute("UPDATE public_hosts SET revision=revision+1", [])
            .unwrap();
        assert!(store.validate_window(&grant).await.is_err());
    }

    #[tokio::test]
    async fn client_window_parent_revocation_and_owner_change_are_immediate() {
        let (store, _, grant) = fixture().await;
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE user_sessions SET revoked_at=?1 WHERE id=?2",
                params![Utc::now().to_rfc3339(), grant.session_id],
            )
            .unwrap();
        assert!(store.validate_window(&grant).await.is_err());
        let (store, _, grant) = fixture().await;
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE installations SET owner_email='new@example.test' WHERE id='installation'",
                [],
            )
            .unwrap();
        assert!(store.validate_window(&grant).await.is_err());
    }

    #[tokio::test]
    async fn client_window_old_binding_cannot_revive_after_owner_returns() {
        let (store, _, grant) = fixture().await;
        store.conn.lock().await.execute("UPDATE installations SET owner_email='next@example.test',owner_verified_at='2026-11-21T00:00:00Z' WHERE id='installation'", []).unwrap();
        assert!(store.validate_window(&grant).await.is_err());
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE installations SET owner_email='owner@example.test' WHERE id='installation'",
                [],
            )
            .unwrap();
        assert!(store.validate_window(&grant).await.is_err());
    }

    #[tokio::test]
    async fn client_window_lease_is_bound_to_buyer_and_revoked_before_cleanup() {
        let (store, _, mut grant) = fixture().await;
        let conn = store.conn.lock().await;
        conn.execute_batch("INSERT INTO online_client_listings(installation_id,seller_id,owner_email,state,revision,sequence,cycle_price_minor,updated_at)
            VALUES ('physical','seller','seller@example.test','listed',1,1,0,0);
            INSERT INTO online_client_quotes(id,installation_id,buyer_id,revision,cycle_price_minor,projection_json,expires_at)
            VALUES ('quote','physical','owner',1,0,'{}',0);
            INSERT INTO online_client_leases(id,installation_id,quote_id,buyer_id,tenant_email,seller_id,alias,state,sequence,grant_until,created_at)
            VALUES ('lease','physical','quote','owner','owner@example.test','seller','rent-test','active',2,unixepoch()+600,0);
            INSERT INTO public_hosts(label,route_id,subject_id,installation_id,target_lane_id,kind,lifecycle,revision,created_at,updated_at)
            SELECT 'rent-test','lease:lease','lease','physical','client:physical','client','active',revision,created_at,updated_at FROM public_hosts WHERE label='client-alpha'").unwrap();
        drop(conn);
        grant.installation_id = "lease".into();
        grant.binding_epoch = "lease:lease".into();
        grant.subdomain = "rent-test".into();
        assert!(store.validate_window(&grant).await.is_err());
        grant.terminal = false;
        assert!(store.validate_window(&grant).await.is_ok());
        store
            .conn
            .lock()
            .await
            .execute("UPDATE online_client_leases SET state='ending'", [])
            .unwrap();
        assert!(store.validate_window(&grant).await.is_err());
    }

    #[tokio::test]
    async fn client_window_expired_tickets_and_credentials_are_removed() {
        let (_, _, mut grant) = fixture().await;
        grant.expires_at = Utc::now() - Duration::seconds(1);
        let mut windows = WindowSessions::default();
        windows.tickets.insert("ticket".into(), grant.clone());
        windows.sessions.insert("session".into(), grant);
        windows.prune();
        assert!(windows.tickets.is_empty());
        assert!(windows.sessions.is_empty());
    }
}
