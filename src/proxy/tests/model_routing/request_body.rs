use super::*;

fn request(f: &QueueFixture, host: &str, body: Body) -> Request {
    Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("host", format!("{host}.{}", f.config.tunnel_domain))
        .header("authorization", format!("Bearer {}", f.token))
        .header("content-type", "application/json")
        .header("session_id", "same-long-session")
        .body(body)
        .unwrap()
}

fn peer() -> ConnectInfo<SocketAddr> {
    ConnectInfo("127.0.0.1:12345".parse().unwrap())
}

#[tokio::test]
async fn ingress_long_session_timeout_then_same_history_retry_does_not_dispatch_twice() {
    let mut f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    f.state
        .config
        .proxy_stream
        .request_body_first_byte_timeout_secs = 0;
    let body = Body::from_stream(futures_util::stream::pending::<
        Result<Bytes, std::convert::Infallible>,
    >());
    let response =
        user_model_proxy_handler(State(f.state.clone()), peer(), request(&f, "api", body)).await;
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(response.headers().get(header::CONNECTION).unwrap(), "close");
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .unwrap(),
        "*"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let error: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error["code"], "user_model_route_request_body_timeout");
    assert_eq!(error["error"]["code"], error["code"]);
    assert_eq!(error["details"]["phase"], "first_byte_timeout");
    assert_eq!(error["details"]["upstreamSent"], false);
    assert_eq!(f.counts(), vec![0; 6]);
    assert!(f.state.proxy.inflight_by_share().await.is_empty());
    assert_eq!(f.state.proxy.request_body_metrics().memory_used_bytes, 0);
    for index in 0..6 {
        assert!(
            !f.state
                .route_breakers
                .is_open(&f.route_id, &format!("queue-{index}"))
        );
    }
    f.state
        .config
        .proxy_stream
        .request_body_first_byte_timeout_secs = 30;
    let body =
        serde_json::json!({"model":"fixture-model","input":"history".repeat(100_000)}).to_string();
    let response = user_model_proxy_handler(
        State(f.state.clone()),
        peer(),
        request(&f, "api", Body::from(body)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(f.counts(), vec![1, 0, 0, 0, 0, 0]);
    assert_eq!(f.state.proxy.request_body_metrics().memory_used_bytes, 0);
    assert_eq!(f.state.proxy.request_body_metrics().completed_total, 1);
}

#[tokio::test]
async fn ingress_unified_and_direct_uploads_share_the_same_timeout_policy() {
    for host in ["api", "queue-0"] {
        let mut f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
        f.state
            .config
            .proxy_stream
            .request_body_first_byte_timeout_secs = 1;
        tokio::time::pause();
        let start = tokio::time::Instant::now();
        let body = Body::from_stream(async_stream::stream! {
            yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"{\"model\":\"fixture-model\",\"input\":\""));
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_secs(20)).await;
                yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"history "));
            }
            yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"\"}"));
        });
        // Freeze only the upload; resume before libSQL/socket operations.
        let mut req = request(&f, host, body);
        let context = BodyContext::attach(&mut req, peer().0);
        let (parts, body) = req.into_parts();
        let wire = read_proxy_request_body(
            &f.state,
            &context,
            &parts.headers,
            body,
            "/v1/responses",
            "integration",
        )
        .await
        .unwrap();
        assert!(start.elapsed() >= Duration::from_secs(60));
        assert!(start.elapsed() < Duration::from_secs(61));
        tokio::time::resume();
        let req = Request::from_parts(parts, Body::from(wire));
        let response = if host == "api" {
            user_model_proxy_handler(State(f.state.clone()), peer(), req).await
        } else {
            proxy_handler(State(f.state.clone()), peer(), req).await
        };
        assert_eq!(response.status(), StatusCode::OK);
        let _ = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(f.counts(), vec![1, 0, 0, 0, 0, 0]);
        assert_eq!(f.state.proxy.request_body_metrics().completed_total, 1);
        drop(context);
        assert_eq!(f.state.proxy.request_body_metrics().memory_used_bytes, 0);
    }
}

#[tokio::test]
async fn ingress_unified_compressed_body_routes_then_preserves_wire_for_backend() {
    use std::io::Write;
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(any(move |req: Request| {
        let tx = tx.clone();
        async move {
            let (parts, body) = req.into_parts();
            let body = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
            tx.send((parts.headers, body)).await.unwrap();
            StatusCode::OK
        }
    }));
    let backend = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let original = f.state.proxy.route_by_share_id("queue-0").await.unwrap();
    f.state
        .proxy
        .set_route_with_kind(
            "queue-0".into(),
            address.to_string(),
            RouteKind::Share,
            original.installation_id.clone(),
            None,
            Some("queue-0".into()),
            None,
            false,
            -1,
            None,
        )
        .await;
    let json = br#"{"model":"fixture-model","input":[{"role":"assistant","content":"full history preserved"}]}"#;
    let mut compressor = flate2::write::GzEncoder::new(Vec::new(), Default::default());
    compressor.write_all(json).unwrap();
    let wire = compressor.finish().unwrap();
    let mut req = request(&f, "api", Body::from(wire.clone()));
    req.headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    req.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&wire.len().to_string()).unwrap(),
    );
    let response = user_model_proxy_handler(State(f.state.clone()), peer(), req).await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let (headers, received) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(headers.get(header::CONTENT_ENCODING).unwrap(), "gzip");
    assert_eq!(
        headers
            .get(header::CONTENT_LENGTH)
            .unwrap()
            .to_str()
            .unwrap(),
        wire.len().to_string()
    );
    assert_eq!(received.as_ref(), wire);
    assert_eq!(f.state.proxy.request_body_metrics().completed_total, 1);
    assert_eq!(f.state.proxy.request_body_metrics().memory_used_bytes, 0);
    backend.abort();
}

#[tokio::test]
async fn ingress_direct_compressed_model_block_cannot_be_bypassed() {
    use std::io::Write;
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    f.state
        .store
        .replace_share_requested_model_blocks(
            "queue-0",
            "owner@example.com",
            0,
            std::collections::BTreeMap::from([("codex".into(), vec!["fixture-model".into()])]),
        )
        .await
        .unwrap();
    let mut compressor = flate2::write::GzEncoder::new(Vec::new(), Default::default());
    compressor
        .write_all(br#"{"model":"fixture-model","input":"preserved"}"#)
        .unwrap();
    let mut req = request(&f, "queue-0", Body::from(compressor.finish().unwrap()));
    req.headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    let response = proxy_handler(State(f.state.clone()), peer(), req).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(f.counts(), vec![0; 6]);
    assert_eq!(f.state.proxy.request_body_metrics().completed_total, 1);
    assert_eq!(f.state.proxy.request_body_metrics().memory_used_bytes, 0);
}

#[tokio::test]
async fn ingress_local_capacity_errors_never_open_breakers_or_try_other_accounts() {
    let mut f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    f.state.config.proxy_stream.request_memory_budget_mb = 0; // test-only full budget
    f.state
        .store
        .replace_share_requested_model_blocks(
            "queue-0",
            "owner@example.com",
            0,
            std::collections::BTreeMap::from([("codex".into(), vec!["another-model".into()])]),
        )
        .await
        .unwrap();
    for _ in 0..4 {
        let response = user_model_proxy_handler(
            State(f.state.clone()),
            peer(),
            request(&f, "api", Body::from(r#"{"model":"fixture-model"}"#)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
    assert_eq!(f.counts(), vec![0; 6]);
    assert!(!f.state.route_breakers.is_open(&f.route_id, "queue-0"));
    assert!(f.state.proxy.inflight_by_share().await.is_empty());
    assert_eq!(f.state.proxy.request_body_metrics().memory_used_bytes, 0);
}

#[tokio::test]
async fn ingress_gateway_uses_forwarded_media_limit_before_signature_authentication() {
    let mut config = proxy_test_config("gateway-upload-limits");
    config.proxy_stream.request_body_limit_mb = 1;
    config.proxy_stream.image_request_body_limit_mb = 2;
    let state = proxy_test_state(&config, Arc::new(ProxyRegistry::default()));
    for (endpoint, expected) in [
        ("images/generations", StatusCode::UNAUTHORIZED),
        ("responses", StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let req = Request::builder()
            .method("POST")
            .uri(format!("/_gateway/proxy/test-share/v1/{endpoint}"))
            .header("host", "gateway.router.test")
            .header("content-length", (1024 * 1024 + 1).to_string())
            .body(Body::from(vec![b' '; 1024 * 1024 + 1]))
            .unwrap();
        let response = gateway_proxy_handler(State(state.clone()), peer(), req).await;
        assert_eq!(response.status(), expected);
        assert_eq!(state.proxy.request_body_metrics().memory_used_bytes, 0);
    }
    let _ = std::fs::remove_file(&config.database.path);
}

#[tokio::test]
async fn ingress_image_and_model_block_prechecks_read_the_body_only_once() {
    let f = QueueFixture::new([true; 6], [StatusCode::OK; 6]).await;
    f.state
        .store
        .replace_share_requested_model_blocks(
            "queue-0",
            "owner@example.com",
            0,
            std::collections::BTreeMap::from([("codex".into(), vec!["another-model".into()])]),
        )
        .await
        .unwrap();
    let mut req = request(
        &f,
        "queue-0",
        Body::from(br#"{"model":"fixture-model","stream":true,"prompt":"preserve"}"#.as_slice()),
    );
    *req.uri_mut() = "/v1/images/generations".parse().unwrap();
    let response = proxy_handler(State(f.state.clone()), peer(), req).await;
    // Fixture deliberately has no image-generation provider. All body checks
    // and the bounded rewrite ran, but no provider/Share request was admitted.
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(f.state.proxy.request_body_metrics().completed_total, 1);
    assert_eq!(f.state.proxy.request_body_metrics().memory_used_bytes, 0);
    assert_eq!(f.counts(), vec![0; 6]);
    assert!(f.state.proxy.inflight_by_share().await.is_empty());
}
