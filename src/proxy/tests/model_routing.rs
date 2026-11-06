use super::*;
use crate::models::UserModelRouteInput;

mod request_body;

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
            self.state.store.route_breakers.record_failure(
                &self.route_id,
                &format!("queue-{index}"),
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
async fn model_route_image_stream_caps_actual_forwards_and_counts_retries() {
    let f = QueueFixture::new([true; 6], [StatusCode::INTERNAL_SERVER_ERROR; 6]).await;
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE shares SET bindings_json = ?1, app_providers_json = ?2",
            crate::db::params![
                serde_json::json!({"codex": "fixture-provider"}).to_string(),
                serde_json::json!({"codex": [{
                    "id": "fixture-provider",
                    "name": "Fixture",
                    "app": "codex",
                    "enabled": true,
                    "codexImageGenerationEnabled": true
                }]})
                .to_string(),
            ],
        )
        .unwrap();
    }
    let response = user_model_proxy_handler(
        State(f.state.clone()),
        ConnectInfo(SocketAddr::from((Ipv4Addr::LOCALHOST, 12345))),
        Request::builder()
            .method("POST")
            .uri("/v1/images/generations")
            .header("host", format!("api.{}", f.config.tunnel_domain))
            .header("authorization", format!("Bearer {}", f.token))
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"model":"fixture-model","prompt":"fixture","stream":true}"#,
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(f.counts(), vec![1, 1, 1, 1, 0, 0]);
    assert_eq!(f.state.metrics.model_route_failover_total_for_tests(), 3);
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

#[tokio::test]
async fn model_route_queue_500_falls_back_but_400_never_opens_the_breaker() {
    let mut statuses = [StatusCode::OK; 6];
    statuses[0] = StatusCode::INTERNAL_SERVER_ERROR;
    let f = QueueFixture::new([true; 6], statuses).await;
    assert_eq!(f.request().await.status(), StatusCode::OK);
    assert_eq!(f.counts(), vec![1, 1, 0, 0, 0, 0]);
    statuses[0] = StatusCode::BAD_REQUEST;
    let f = QueueFixture::new([true; 6], statuses).await;
    for _ in 0..5 {
        assert_eq!(f.request().await.status(), StatusCode::BAD_REQUEST);
    }
    assert!(!f.state.store.route_breakers.is_open(&f.route_id, "queue-0"));
    assert_eq!(f.counts(), vec![5, 0, 0, 0, 0, 0]);
}

#[tokio::test]
async fn model_route_queue_deleted_primary_and_blocked_candidates_do_not_spend_budget() {
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    f.state
        .store
        .conn
        .lock()
        .await
        .execute("DELETE FROM shares WHERE share_id='queue-0'", [])
        .unwrap();
    for index in 1..5 {
        f.state
            .store
            .replace_share_requested_model_blocks(
                &format!("queue-{index}"),
                "owner@example.com",
                0,
                std::collections::BTreeMap::from([("codex".into(), vec!["fixture-model".into()])]),
            )
            .await
            .unwrap();
    }
    assert_eq!(f.request().await.status(), StatusCode::OK);
    assert_eq!(f.counts(), vec![0, 0, 0, 0, 0, 1]);
    for index in 1..5 {
        assert!(
            !f.state
                .store
                .route_breakers
                .is_open(&f.route_id, &format!("queue-{index}"))
        );
    }
}

#[tokio::test]
async fn model_route_queue_model_blocked_and_runtime_skip_have_stable_error_priority() {
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    for index in 0..6 {
        f.state
            .store
            .replace_share_requested_model_blocks(
                &format!("queue-{index}"),
                "owner@example.com",
                0,
                std::collections::BTreeMap::from([("codex".into(), vec!["fixture-model".into()])]),
            )
            .await
            .unwrap();
    }
    assert_eq!(f.request().await.status(), StatusCode::FORBIDDEN);
    f.state
        .store
        .replace_share_requested_model_blocks(
            "queue-5",
            "owner@example.com",
            1,
            std::collections::BTreeMap::new(),
        )
        .await
        .unwrap();
    f.open(5);
    assert_eq!(f.request().await.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(f.counts(), vec![0; 6]);
}

#[tokio::test]
async fn model_route_diagnostic_matches_production_candidate_loop_and_records_body_fact() {
    use crate::models::ModelRouteTestMode;
    let statuses = [
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::OK,
    ];
    let production = QueueFixture::new([true; 6], statuses).await;
    assert_eq!(production.request().await.status(), StatusCode::OK);
    let diagnostic = QueueFixture::new([true; 6], statuses).await;
    let user = diagnostic
        .state
        .store
        .ensure_user_by_email("owner@example.com")
        .await
        .unwrap();
    let probe = build_user_model_route_test_probe("codex", "fixture-model").unwrap();
    let outcome = execute_user_model_route_test(
        diagnostic.state.clone(),
        SocketAddr::from((Ipv4Addr::LOCALHOST, 9999)),
        &user.id,
        "owner@example.com",
        &probe,
        ModelRouteTestMode::SavedRoute,
        Some(1),
        tokio::time::Instant::now() + Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(outcome.success);
    assert_eq!(outcome.target_share_id.as_deref(), Some("queue-1"));
    assert_eq!(production.counts(), diagnostic.counts());
    assert_eq!(
        outcome
            .attempts
            .iter()
            .map(|attempt| attempt.position)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    let profile = diagnostic
        .state
        .store
        .get_user_model_routing(&diagnostic.config, "owner@example.com", &HashSet::new())
        .await
        .unwrap();
    assert_eq!(profile.route_recent_results.len(), 1);
    assert_eq!(profile.route_recent_results[0].phase, "body_complete");
    assert!(profile.route_recent_results[0].used_backup);
}

#[tokio::test]
async fn model_route_diagnostic_revision_and_rate_rejections_never_forward() {
    use crate::models::ModelRouteTestMode;
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    let user = f
        .state
        .store
        .ensure_user_by_email("owner@example.com")
        .await
        .unwrap();
    let probe = build_user_model_route_test_probe("codex", "fixture-model").unwrap();
    for revision in [None, Some(0)] {
        assert!(
            execute_user_model_route_test(
                f.state.clone(),
                SocketAddr::from((Ipv4Addr::LOCALHOST, 9999)),
                &user.id,
                "owner@example.com",
                &probe,
                ModelRouteTestMode::SavedRoute,
                revision,
                tokio::time::Instant::now() + Duration::from_secs(30)
            )
            .await
            .is_err()
        );
    }
    assert_eq!(f.counts(), vec![0; 6]);
    for _ in 0..3 {
        f.state
            .store
            .model_route_runtime
            .admit_diagnostic(&user.id)
            .unwrap();
    }
    let error = execute_user_model_route_test(
        f.state.clone(),
        SocketAddr::from((Ipv4Addr::LOCALHOST, 9999)),
        &user.id,
        "owner@example.com",
        &probe,
        ModelRouteTestMode::SavedRoute,
        Some(1),
        tokio::time::Instant::now() + Duration::from_secs(30),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, AppError::RateLimited { .. }));
    assert_eq!(f.counts(), vec![0; 6]);
}

#[tokio::test]
async fn model_route_test_preview_preserves_prefix_and_classifies_incomplete_bodies() {
    use crate::models::ModelRouteBodyState;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    for (size, expected) in [
        (0, ModelRouteBodyState::Complete),
        (
            USER_MODEL_ROUTE_TEST_BODY_LIMIT,
            ModelRouteBodyState::Complete,
        ),
        (
            USER_MODEL_ROUTE_TEST_BODY_LIMIT + 100,
            ModelRouteBodyState::Truncated,
        ),
    ] {
        let (bytes, state) =
            read_model_route_test_preview(Body::from(vec![b'x'; size]), deadline).await;
        assert_eq!(state, expected);
        assert_eq!(bytes.len(), size.min(USER_MODEL_ROUTE_TEST_BODY_LIMIT));
    }
    let stream = futures_util::stream::iter([
        Ok::<_, std::io::Error>(Bytes::from_static(b"prefix")),
        Err(std::io::Error::other("fixture read failure")),
    ]);
    let (bytes, state) = read_model_route_test_preview(Body::from_stream(stream), deadline).await;
    assert_eq!(bytes, b"prefix");
    assert_eq!(state, ModelRouteBodyState::ReadError);
    let body = Body::from_stream(futures_util::stream::pending::<Result<Bytes, std::io::Error>>());
    let (_, state) = read_model_route_test_preview(body, tokio::time::Instant::now()).await;
    assert_eq!(state, ModelRouteBodyState::Timeout);
}

impl QueueFixture {
    async fn session_headers(&self) -> HeaderMap {
        use sha2::Digest;
        let user = self
            .state
            .store
            .ensure_user_by_email("owner@example.com")
            .await
            .unwrap();
        let token = "fixture-router-session";
        let hash = base64::engine::general_purpose::STANDARD
            .encode(sha2::Sha256::digest(token.as_bytes()));
        let conn = self.state.store.conn.lock().await;
        let installation: String = conn
            .query_row("SELECT installation_id FROM shares LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        conn.execute("INSERT INTO user_sessions (id,user_id,auth_source_kind,auth_source_id,access_token_hash,refresh_token_hash,access_expires_at,refresh_expires_at,created_at,last_used_at)
            VALUES ('fixture-session',?1,'client_installation',?2,?3,'fixture-refresh',?4,?4,?5,?5)",
            crate::db::params![user.id, installation, hash, (Utc::now()+chrono::Duration::hours(1)).to_rfc3339(), Utc::now().to_rfc3339()]).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }
}

#[tokio::test]
async fn model_route_api_preserves_legacy_timeout_wrapper_and_get_disables_caching() {
    use crate::models::{ModelRouteTestMode, UserModelRoutingTestRequest};
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    let headers = f.session_headers().await;
    let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 9999));
    for (mode, revision, expected) in [
        (ModelRouteTestMode::Primary, None, StatusCode::OK),
        (
            ModelRouteTestMode::Primary,
            Some(1),
            StatusCode::GATEWAY_TIMEOUT,
        ),
        (
            ModelRouteTestMode::SavedRoute,
            Some(1),
            StatusCode::GATEWAY_TIMEOUT,
        ),
    ] {
        let response = match crate::api::test_my_model_routing_at(
            f.state.clone(),
            peer,
            headers.clone(),
            UserModelRoutingTestRequest {
                app_type: "codex".into(),
                requested_model: "fixture-model".into(),
                mode,
                expected_revision: revision,
            },
            tokio::time::Instant::now(),
        )
        .await
        {
            Ok(value) => value.into_response(),
            Err(error) => error.into_response(),
        };
        assert_eq!(response.status(), expected);
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["code"], "user_model_route_test_timeout");
        if revision.is_none() {
            assert_eq!(json["success"], false);
        }
    }
    assert_eq!(f.counts(), vec![0; 6]);
    let response = crate::api::get_my_model_routing(State(f.state.clone()), headers.clone())
        .await
        .unwrap()
        .into_response();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    // An unchanged save takes no new revision but returns the same per-user body.
    let response = crate::api::replace_my_model_routing(
        State(f.state.clone()),
        headers,
        axum::Json(crate::models::ReplaceUserModelRoutingRequest {
            expected_revision: 1,
            routes: vec![UserModelRouteInput {
                app_type: "codex".into(),
                requested_model: "*".into(),
                target_share_id: "queue-0".into(),
                candidates: (1..6).map(|index| format!("queue-{index}")).collect(),
            }],
        }),
    )
    .await
    .unwrap()
    .into_response();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn model_route_observation_never_waits_for_database_lock_and_keeps_completed_result() {
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    let user = f
        .state
        .store
        .ensure_user_by_email("owner@example.com")
        .await
        .unwrap();
    let resolution = f
        .state
        .store
        .resolve_user_model_route(&user.id, "owner@example.com", "codex", "fixture-model")
        .await
        .unwrap()
        .unwrap();
    let first = f.state.store.model_route_runtime.next_sequence().unwrap();
    let _connection = f.state.store.conn.lock().await;
    let operation = async {
        f.state
            .store
            .begin_user_model_route_observation(&resolution, first);
        let result = crate::models::ModelRouteRecentResult {
            route_id: resolution.route_id.clone(),
            revision: resolution.revision,
            request_sequence: first.to_string(),
            observed_at: Utc::now(),
            source: "request",
            phase: "response_headers",
            target_share_id: Some("queue-0".into()),
            position: Some(0),
            status_code: 200,
            continued_search: false,
            used_backup: false,
            body_state: None,
        };
        f.state
            .store
            .record_user_model_route_observation(&resolution, first, result);
        f.state
            .store
            .begin_user_model_route_observation(&resolution, first + 1);
        let results = f.state.store.model_route_runtime.results(
            &user.id,
            resolution.revision,
            &HashSet::from([resolution.route_id.clone()]),
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].request_sequence, first.to_string());
    };
    tokio::time::timeout(Duration::from_secs(1), operation)
        .await
        .expect("observations must not wait for Store");
}

#[tokio::test]
async fn model_route_deleted_primary_test_and_app_disabled_with_offline_backups() {
    use crate::models::ModelRouteTestMode;
    let f = QueueFixture::new([false; 6], [StatusCode::OK; 6]).await;
    let user = f
        .state
        .store
        .ensure_user_by_email("owner@example.com")
        .await
        .unwrap();
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE shares SET enabled_codex=0 WHERE share_id='queue-0'",
            [],
        )
        .unwrap();
    }
    assert_eq!(f.request().await.status(), StatusCode::SERVICE_UNAVAILABLE);
    let probe = build_user_model_route_test_probe("codex", "fixture-model").unwrap();
    let primary_test = || {
        execute_user_model_route_test(
            f.state.clone(),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 9999)),
            &user.id,
            "owner@example.com",
            &probe,
            ModelRouteTestMode::Primary,
            None,
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
    };
    // The primary test reports the primary's own failure, not the generic
    // offline error the production queue returns once backups are exhausted.
    let error = primary_test().await.unwrap_err();
    assert!(
        matches!(
            error,
            AppError::Coded {
                status: StatusCode::CONFLICT,
                code: "user_model_route_app_unavailable",
                ..
            }
        ),
        "{error:?}"
    );
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute("DELETE FROM shares WHERE share_id='queue-0'", [])
            .unwrap();
    }
    let error = primary_test().await.unwrap_err();
    assert!(
        matches!(
            &error,
            AppError::Coded {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "user_model_route_target_unavailable",
                message,
                ..
            } if message == "configured target Share is unavailable"
        ),
        "deleted primary must keep its own error, not the offline one: {error:?}"
    );
    assert_eq!(f.counts(), vec![0; 6]);
}

#[tokio::test]
async fn model_route_counter_ignores_search_without_forward_and_counts_actual_retry() {
    for (online, expected) in [
        ([true, false, false, false, false, false], 0),
        ([true; 6], 1),
    ] {
        let f = QueueFixture::new(
            online,
            [
                StatusCode::INTERNAL_SERVER_ERROR,
                StatusCode::OK,
                StatusCode::OK,
                StatusCode::OK,
                StatusCode::OK,
                StatusCode::OK,
            ],
        )
        .await;
        let _ = f.request().await;
        assert_eq!(
            f.state.metrics.model_route_failover_total_for_tests(),
            expected
        );
    }
}

#[tokio::test]
async fn model_route_diagnostic_preparation_failures_do_not_consume_admission() {
    use crate::models::ModelRouteTestMode;
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    let user = f
        .state
        .store
        .ensure_user_by_email("owner@example.com")
        .await
        .unwrap();
    let probe = build_user_model_route_test_probe("codex", "fixture-model").unwrap();
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE user_api_tokens SET token_plaintext = NULL WHERE user_id = ?1",
            crate::db::params![user.id],
        )
        .unwrap();
    }
    for _ in 0..4 {
        let error = execute_user_model_route_test(
            f.state.clone(),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 9999)),
            &user.id,
            "owner@example.com",
            &probe,
            ModelRouteTestMode::SavedRoute,
            Some(1),
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, AppError::Internal(_)));
    }
    // Already-expired requests also leave all three admission slots available.
    let error = execute_user_model_route_test(
        f.state.clone(),
        SocketAddr::from((Ipv4Addr::LOCALHOST, 9999)),
        &user.id,
        "owner@example.com",
        &probe,
        ModelRouteTestMode::SavedRoute,
        Some(1),
        tokio::time::Instant::now(),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        AppError::Coded {
            status: StatusCode::GATEWAY_TIMEOUT,
            ..
        }
    ));
    for _ in 0..3 {
        f.state
            .store
            .model_route_runtime
            .admit_diagnostic(&user.id)
            .unwrap();
    }
    assert!(
        f.state
            .store
            .model_route_runtime
            .admit_diagnostic(&user.id)
            .is_err()
    );
    assert_eq!(f.counts(), vec![0; 6]);
}

#[tokio::test]
async fn model_route_local_acl_rejections_do_not_count_as_upstream_retries() {
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    let user = f
        .state
        .store
        .ensure_user_by_email("owner@example.com")
        .await
        .unwrap();
    let resolution = f
        .state
        .store
        .resolve_user_model_route(&user.id, "owner@example.com", "codex", "fixture-model")
        .await
        .unwrap()
        .unwrap();
    // Revoke access after taking the route snapshot. The Share handler must
    // reject this target locally; the next real forward is the first one.
    {
        let conn = f.state.store.conn.lock().await;
        conn.execute(
            "UPDATE shares SET owner_email = 'another@example.test' WHERE share_id = 'queue-0'",
            [],
        )
        .unwrap();
    }
    let (parts, _) = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("authorization", format!("Bearer {}", f.token))
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap()
        .into_parts();
    let execution = execute_resolved_model_route(
        &f.state,
        SocketAddr::from((Ipv4Addr::LOCALHOST, 9999)),
        parts,
        Bytes::from_static(br#"{"model":"fixture-model","input":"fixture"}"#),
        &resolution,
        "codex",
        "fixture-model",
    )
    .await;
    assert_eq!(execution.response.status(), StatusCode::OK);
    assert_eq!(f.counts(), vec![0, 1, 0, 0, 0, 0]);
    assert_eq!(f.state.metrics.model_route_failover_total_for_tests(), 0);
}

#[tokio::test]
async fn model_route_api_control_errors_preserve_outer_status_and_do_not_forward() {
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    let headers = f.session_headers().await;
    let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, 9999));
    for (fields, status) in [
        (
            serde_json::json!({"mode":"saved_route"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            serde_json::json!({"mode":"saved_route","expectedRevision":0}),
            StatusCode::CONFLICT,
        ),
        (
            serde_json::json!({"mode":"primary","expectedRevision":0}),
            StatusCode::CONFLICT,
        ),
    ] {
        let mut value = serde_json::json!({"appType":"codex","requestedModel":"fixture-model"});
        value
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let error = crate::api::test_my_model_routing_at(
            f.state.clone(),
            peer,
            headers.clone(),
            serde_json::from_value(value).unwrap(),
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
        .await
        .unwrap_err();
        assert_eq!(error.into_response().status(), status);
    }
    let user = f
        .state
        .store
        .ensure_user_by_email("owner@example.com")
        .await
        .unwrap();
    for _ in 0..3 {
        f.state
            .store
            .model_route_runtime
            .admit_diagnostic(&user.id)
            .unwrap();
    }
    let error = crate::api::test_my_model_routing_at(
        f.state.clone(), peer, headers, serde_json::from_value(serde_json::json!({
            "appType":"codex","requestedModel":"fixture-model","mode":"saved_route","expectedRevision":1,
        })).unwrap(), tokio::time::Instant::now() + Duration::from_secs(30),
    ).await.unwrap_err().into_response();
    assert_eq!(error.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(error.headers().contains_key(header::RETRY_AFTER));
    assert_eq!(f.counts(), vec![0; 6]);
}

#[tokio::test]
async fn model_route_api_deadline_includes_waiting_for_session_store_lock() {
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    let headers = f.session_headers().await;
    let _connection = f.state.store.conn.lock().await;
    for (fields, expected) in [
        (serde_json::json!({}), StatusCode::OK),
        (
            serde_json::json!({"mode":"saved_route","expectedRevision":1}),
            StatusCode::GATEWAY_TIMEOUT,
        ),
    ] {
        let mut value = serde_json::json!({"appType":"codex","requestedModel":"fixture-model"});
        value
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            crate::api::test_my_model_routing_at(
                f.state.clone(),
                SocketAddr::from((Ipv4Addr::LOCALHOST, 9999)),
                headers.clone(),
                serde_json::from_value(value).unwrap(),
                tokio::time::Instant::now(),
            ),
        )
        .await
        .expect("expired diagnostics cannot wait for authentication SQL");
        let response = match result {
            Ok(value) => value.into_response(),
            Err(error) => error.into_response(),
        };
        assert_eq!(response.status(), expected);
    }
    assert_eq!(f.counts(), vec![0; 6]);
}
