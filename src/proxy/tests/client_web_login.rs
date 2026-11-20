use super::*;
use serde_json::{Value, json};

#[tokio::test]
async fn client_window_http_exchange_is_single_use_target_bound_and_revocable() {
    let mut config = proxy_test_config("client-window-http");
    config.use_localhost = false;
    config.router_owner_email = Some("visitor@example.test".into());
    let state = proxy_test_state(&config, Arc::new(ProxyRegistry::default()));
    let access = "owner-access-token";
    let now = Utc::now().to_rfc3339();
    {
        let conn = state.store.conn.lock().await;
        let actor = crate::market_finance_auth::test_session(&conn, "owner");
        let visitor = crate::market_finance_auth::test_session(&conn, "visitor");
        conn.execute(
            "UPDATE user_sessions SET access_token_hash=?1 WHERE id=?2",
            crate::db::params![
                base64::engine::general_purpose::STANDARD
                    .encode(Sha256::digest("visitor-access-token")),
                visitor.session_id
            ],
        )
        .unwrap();
        conn.execute(
            "UPDATE user_sessions SET access_token_hash=?1 WHERE id=?2",
            crate::db::params![
                base64::engine::general_purpose::STANDARD.encode(Sha256::digest(access)),
                actor.session_id
            ],
        )
        .unwrap();
        conn.execute("INSERT INTO installations(id,public_key,platform,app_version,owner_email,owner_verified_at,created_at,last_seen_at,client_activated_at,control_secret_b64) VALUES('installation','public','linux','1',?1,?2,?2,?2,?2,'control-secret-at-least-thirty-two-bytes')",crate::db::params![actor.email,now]).unwrap();
        conn.execute("INSERT INTO installation_client_tunnels(installation_id,owner_email,subdomain,enabled,created_at,updated_at,last_seen_at) VALUES('installation',?1,'client-alpha',1,?2,?2,?2)",crate::db::params![actor.email,now]).unwrap();
        conn.execute("INSERT INTO public_hosts(label,route_id,kind,subject_id,installation_id,target_lane_id,lifecycle,revision,created_at,updated_at) VALUES('client-alpha','client:installation','client','installation','installation','installation','active',1,?1,?1)",crate::db::params![now]).unwrap();
    }
    let backend_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        axum::serve(
            backend_listener,
            axum::Router::new().fallback(any(|uri: axum::http::Uri, headers: HeaderMap| async move {
                assert!(!headers.contains_key(crate::store::client_web_sessions::WINDOW_HEADER));
                let encoded = headers
                    .get(crate::ingress_context::INGRESS_CONTEXT_HEADER)
                    .unwrap()
                    .to_str()
                    .unwrap();
                let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(encoded)
                    .unwrap();
                let context: crate::ingress_context::IngressContext =
                    serde_json::from_slice(&decoded).unwrap();
                if uri.path() == "/web-api/events" {
                    let stream = async_stream::stream! {
                        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"event: connected\ndata: {}\n\n"));
                        std::future::pending::<()>().await;
                    };
                    return ([(header::CONTENT_TYPE, "text/event-stream")], Body::from_stream(stream)).into_response();
                }
                axum::Json(json!({"email":context.user_email,"role":context.user_role})).into_response()
            })),
        )
        .await
        .unwrap();
    });
    state
        .proxy
        .set_route_with_kind(
            "client-alpha".into(),
            backend_addr.to_string(),
            RouteKind::ClientWeb,
            Some("installation".into()),
            None,
            None,
            None,
            false,
            0,
            None,
        )
        .await;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let app = crate::api::router(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let nonce = Uuid::new_v4().to_string();
    let input = json!({"installationId":"installation","kind":"console","nonce":nonce});
    assert_eq!(
        client
            .post(format!("http://{addr}/v1/auth/client-web-tickets"))
            .header("host", "router.test")
            .bearer_auth("visitor-access-token")
            .json(&input)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .post(format!("http://{addr}/v1/auth/client-web-tickets"))
            .header("host", "client-alpha.router.test")
            .bearer_auth(access)
            .json(&input)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let issue = client
        .post(format!("http://{addr}/v1/auth/client-web-tickets"))
        .header("host", "router.test")
        .bearer_auth(access)
        .json(&json!({"installationId":"installation","kind":"console","nonce":nonce}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        issue.status(),
        StatusCode::OK,
        "{}",
        issue.text().await.unwrap()
    );
    let issued: Value = issue.json().await.unwrap();
    let exchange = |host: &str| {
        client
            .post(format!("http://{addr}/web-api/auth/router-exchange"))
            .header("host", host)
            .header("origin", format!("https://{host}"))
            .json(&json!({"ticket":issued["ticket"],"nonce":nonce}))
    };
    assert_eq!(
        exchange("other.router.test").send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .post(format!("http://{addr}/web-api/auth/router-exchange"))
            .header("host", "client-alpha.router.test")
            .header("origin", "https://client-alpha.router.test")
            .json(&json!({"ticket":issued["ticket"],"nonce":Uuid::new_v4().to_string()}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let exchanged = exchange("client-alpha.router.test").send().await.unwrap();
    assert_eq!(exchanged.status(), StatusCode::OK);
    let credential: Value = exchanged.json().await.unwrap();
    assert_eq!(
        exchange("client-alpha.router.test")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let context = || {
        client
            .get(format!("http://{addr}/web-api/context"))
            .header("host", "client-alpha.router.test")
            .header(
                crate::store::client_web_sessions::WINDOW_HEADER,
                credential["token"].as_str().unwrap(),
            )
    };
    let response = context().send().await.unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["email"], "owner@example.test");
    assert_eq!(body["role"], "owner");
    let mut events = client
        .get(format!("http://{addr}/web-api/events"))
        .header("host", "client-alpha.router.test")
        .header(
            crate::store::client_web_sessions::WINDOW_HEADER,
            credential["token"].as_str().unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    assert!(events.chunk().await.unwrap().is_some());
    state
        .store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE user_sessions SET revoked_at=?1 WHERE id='session-owner'",
            crate::db::params![now],
        )
        .unwrap();
    assert_eq!(
        context().send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(6), events.chunk())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    server.abort();
    backend.abort();
}
