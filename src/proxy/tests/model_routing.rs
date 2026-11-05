use super::*;
use crate::models::UserModelRouteInput;

/// Real proxy requests, but only to loopback fixtures; no model calls or UI.
struct QueueFixture {
    config: Config,
    state: ServerState,
    token: String,
    route_id: String,
    hits: Vec<Arc<AtomicUsize>>,
    servers: Vec<tokio::task::JoinHandle<()>>,
}

impl QueueFixture {
    async fn new(online: [bool; 6], statuses: [StatusCode; 6]) -> Self {
        let config = proxy_test_config("model-route-queue");
        let state = proxy_test_state(&config, Arc::new(ProxyRegistry::default()));
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[19_u8; 32]);
        let registered = state
            .store
            .register_installation(
                signed_registration_request(&signing_key, "nonce-model-route-queue"),
                crate::models::ClientMetadata {
                    ip: None,
                    country_code: None,
                },
            )
            .await
            .unwrap();
        {
            let conn = state.store.conn.lock().await;
            for index in 0..6 {
                let id = format!("queue-{index}");
                conn.execute(
                    "INSERT INTO shares (share_id, capacity_pool_id, installation_id, share_name,
                     owner_email, subdomain, app_type, enabled_codex, token_limit, parallel_limit,
                     tokens_used, requests_count, share_status, created_at, expires_at, updated_at)
                     VALUES (?1, ?1, ?2, 'Queue fixture', 'owner@example.com',
                     ?1, 'codex', 1, -1, -1, 0, 0, 'active', '2026-01-01T00:00:00Z',
                     '2099-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    crate::db::params![id, registered.installation_id],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO share_bindings (share_id, app_type, provider_id)
                     VALUES (?1, 'codex', 'fixture-provider')",
                    crate::db::params![id],
                )
                .unwrap();
            }
        }
        let mut hits = Vec::new();
        let mut servers = Vec::new();
        for index in 0..6 {
            let count = Arc::new(AtomicUsize::new(0));
            hits.push(count.clone());
            let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let status = statuses[index];
            let upstream = axum::Router::new().fallback(any(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, AtomicOrdering::SeqCst);
                    status
                }
            }));
            servers.push(tokio::spawn(async move {
                axum::serve(listener, upstream).await.unwrap();
            }));
            if online[index] {
                let id = format!("queue-{index}");
                state
                    .proxy
                    .set_route_with_kind(
                        id.clone(),
                        address.to_string(),
                        RouteKind::Share,
                        Some(registered.installation_id.clone()),
                        None,
                        Some(id),
                        None,
                        false,
                        -1,
                        None,
                    )
                    .await;
            }
        }
        let token = state
            .store
            .get_default_api_token("owner@example.com")
            .await
            .unwrap()
            .api_token
            .unwrap();
        let profile = state
            .store
            .replace_user_model_routing(
                &config,
                "owner@example.com",
                0,
                vec![UserModelRouteInput {
                    app_type: "codex".into(),
                    requested_model: "*".into(),
                    target_share_id: "queue-0".into(),
                    candidates: (1..6).map(|index| format!("queue-{index}")).collect(),
                }],
                &HashSet::new(),
            )
            .await
            .unwrap();
        Self {
            config,
            state,
            token,
            route_id: profile.routes[0].id.clone(),
            hits,
            servers,
        }
    }

    fn open(&self, index: usize) {
        for _ in 0..3 {
            self.state.route_breakers.record_failure(
                &self.route_id,
                &format!("queue-{index}"),
                false,
                None,
            );
        }
    }

    fn counts(&self) -> Vec<usize> {
        self.hits
            .iter()
            .map(|count| count.load(AtomicOrdering::SeqCst))
            .collect()
    }

    async fn request(&self) -> Response {
        user_model_proxy_handler(
            State(self.state.clone()),
            ConnectInfo(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345)),
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("host", format!("api.{}", self.config.tunnel_domain))
                .header("authorization", format!("Bearer {}", self.token))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"fixture-model","input":"fixture"}"#))
                .unwrap(),
        )
        .await
    }
}

impl Drop for QueueFixture {
    fn drop(&mut self) {
        for server in &self.servers {
            server.abort();
        }
        let _ = std::fs::remove_file(&self.config.database.path);
    }
}

#[tokio::test]
async fn model_route_queue_reaches_the_tail_after_offline_and_open_targets() {
    let mut statuses = [StatusCode::OK; 6];
    statuses[4] = StatusCode::SERVICE_UNAVAILABLE;
    let f = QueueFixture::new([false, true, false, true, true, true], statuses).await;
    f.open(1);
    f.open(3);
    let response = f.request().await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(f.counts(), vec![0, 0, 0, 0, 1, 1]);
}

#[tokio::test]
async fn model_route_queue_caps_actual_forwards_at_four() {
    let f = QueueFixture::new([true; 6], [StatusCode::SERVICE_UNAVAILABLE; 6]).await;
    assert_eq!(f.request().await.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(f.counts(), vec![1, 1, 1, 1, 0, 0]);
}

#[tokio::test]
async fn model_route_queue_fails_closed_when_every_target_is_skipped() {
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    for index in 0..6 {
        f.open(index);
    }
    let response = f.request().await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["code"], "user_model_route_target_unavailable");
    assert_eq!(f.counts(), vec![0; 6]);
}

#[tokio::test]
async fn model_route_queue_commits_success_or_nonretryable_errors_without_fallback() {
    for status in [StatusCode::OK, StatusCode::BAD_REQUEST] {
        let mut statuses = [StatusCode::OK; 6];
        statuses[0] = status;
        let f = QueueFixture::new([true; 6], statuses).await;
        assert_eq!(f.request().await.status(), status);
        assert_eq!(f.counts(), vec![1, 0, 0, 0, 0, 0]);
    }
}
