use super::*;
use std::convert::Infallible;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn context(body: Body, headers: HeaderMap) -> (Arc<BodyContext>, Body, HeaderMap) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .body(body)
        .unwrap();
    *request.headers_mut() = headers;
    let context = BodyContext::attach(&mut request, "127.0.0.1:1234".parse().unwrap());
    let (parts, body) = request.into_parts();
    (context, body, parts.headers)
}

fn length(length: usize) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).unwrap(),
    );
    headers
}

fn pending() -> Body {
    Body::from_stream(futures_util::stream::pending::<Result<Bytes, Infallible>>())
}

#[tokio::test(start_paused = true)]
async fn progressing_upload_exceeds_old_thirty_second_deadline() {
    let start = Instant::now();
    let body = Body::from_stream(async_stream::stream! {
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_secs(20)).await;
            yield Ok::<_, Infallible>(Bytes::from_static(b"data"));
        }
    });
    let (context, body, headers) = context(body, length(16));
    let budget = Arc::new(BodyBudget::default());
    let result = context
        .collect(body, &headers, 1024, &Default::default(), &budget, "test")
        .await
        .unwrap();
    assert_eq!(result.as_ref(), b"datadatadatadata");
    assert_eq!(start.elapsed(), Duration::from_secs(80));
    assert_eq!(budget.snapshot().completed_total, 1);
}

#[tokio::test(start_paused = true)]
async fn legacy_absolute_deadline_is_not_reinterpreted_as_idle() {
    let config = ProxyStreamConfig {
        request_body_timeout_secs: 30,
        ..Default::default()
    };
    let body = Body::from_stream(async_stream::stream! {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            yield Ok::<_, Infallible>(Bytes::from_static(b"progress"));
        }
    });
    let (context, body, headers) = context(body, HeaderMap::new());
    let budget = Arc::new(BodyBudget::default());
    let error = context
        .collect(body, &headers, 1024, &config, &budget, "legacy")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::HardTimeout);
    assert_eq!(error.elapsed_ms, 30_000);
    assert_eq!(error.received, 8);
}

#[tokio::test]
async fn malformed_and_ambiguous_declared_lengths_are_bad_requests() {
    let budget = Arc::new(BodyBudget::default());
    let mut duplicate = length(4);
    duplicate.append(header::CONTENT_LENGTH, HeaderValue::from_static("4"));
    let mut signed = HeaderMap::new();
    signed.insert(header::CONTENT_LENGTH, HeaderValue::from_static("+4"));
    for headers in [duplicate, signed] {
        let (context, body, headers) = context(Body::from("data"), headers);
        let error = context
            .collect(body, &headers, 1024, &Default::default(), &budget, "length")
            .await
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::InvalidLength);
    }
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
}

#[tokio::test(start_paused = true)]
async fn first_byte_idle_and_hard_deadlines_are_distinct() {
    let budget = Arc::new(BodyBudget::default());
    let config = ProxyStreamConfig::default();
    let (context, body, headers) = context(pending(), HeaderMap::new());
    let error = context
        .collect(body, &headers, 1024, &config, &budget, "test")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::FirstByteTimeout);
    assert_eq!(error.received, 0);
    assert_eq!(error.elapsed_ms, 30_000);

    let body = Body::from_stream(async_stream::stream! {
        yield Ok::<_, Infallible>(Bytes::from_static(b"abc"));
        futures_util::future::pending::<()>().await;
    });
    let (context, body, headers) = self::context(body, HeaderMap::new());
    let error = context
        .collect(body, &headers, 1024, &config, &budget, "test")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::IdleTimeout);
    assert_eq!(error.received, 3);
    assert_eq!(error.last_progress_age_ms, 30_000);

    let body = Body::from_stream(async_stream::stream! {
        loop {
            yield Ok::<_, Infallible>(Bytes::from_static(b"x"));
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
    });
    let (context, body, headers) = self::context(body, HeaderMap::new());
    let config = ProxyStreamConfig {
        request_body_timeout_secs: 65,
        ..config
    };
    let error = context
        .collect(body, &headers, 1024, &config, &budget, "test")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::HardTimeout);
    assert_eq!(error.elapsed_ms, 65_000);
    assert_eq!(error.received, 4);
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
    assert_eq!(budget.snapshot().first_byte_timeout_total, 1);
    assert_eq!(budget.snapshot().idle_timeout_total, 1);
    assert_eq!(budget.snapshot().hard_timeout_total, 1);
}

#[tokio::test(start_paused = true)]
async fn empty_frames_and_trailers_never_extend_progress() {
    let body = Body::from_stream(async_stream::stream! {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            yield Ok::<_, Infallible>(Bytes::new());
        }
    });
    let (context, body, headers) = context(body, HeaderMap::new());
    let budget = Arc::new(BodyBudget::default());
    let error = context
        .collect(body, &headers, 1024, &Default::default(), &budget, "test")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::FirstByteTimeout);
    assert_eq!(error.elapsed_ms, 30_000);

    let frames = async_stream::stream! {
        yield Ok::<_, Infallible>(http_body::Frame::data(Bytes::from_static(b"a")));
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            yield Ok::<_, Infallible>(http_body::Frame::trailers(HeaderMap::new()));
        }
    };
    let body = Body::new(http_body_util::StreamBody::new(frames));
    let (context, body, headers) = self::context(body, HeaderMap::new());
    let error = context
        .collect(body, &headers, 1024, &Default::default(), &budget, "test")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::IdleTimeout);
    assert_eq!(error.received, 1);
}

#[tokio::test]
async fn size_length_and_read_errors_are_not_conflated() {
    let budget = Arc::new(BodyBudget::default());
    let config = ProxyStreamConfig::default();
    for (body, headers, kind) in [
        (Body::from("12345"), HeaderMap::new(), FailureKind::TooLarge),
        (Body::from("123"), length(4), FailureKind::InvalidLength),
        (Body::from("123"), length(2), FailureKind::InvalidLength),
        (
            Body::from_stream(futures_util::stream::once(async {
                Err::<Bytes, _>(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "sensitive transport detail",
                ))
            })),
            HeaderMap::new(),
            FailureKind::ReadError,
        ),
    ] {
        let (context, body, headers) = context(body, headers);
        let error = context
            .collect(body, &headers, 4, &config, &budget, "test")
            .await
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(
            error.kind.status(),
            if kind == FailureKind::TooLarge {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            }
        );
        let response = context.error_response(&error, true);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("sensitive"));
        assert_eq!(budget.snapshot().memory_used_bytes, 0);
    }
    let polls = Arc::new(AtomicUsize::new(0));
    let observed = polls.clone();
    let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
        observed.fetch_add(1, Ordering::Relaxed);
        std::task::Poll::<Option<Result<Bytes, Infallible>>>::Pending
    }));
    let (context, body, headers) = context(body, length(5));
    let error = context
        .collect(body, &headers, 4, &config, &budget, "test")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::TooLarge);
    assert_eq!(polls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn repeated_reads_clones_and_reqwest_body_keep_one_lease() {
    let budget = Arc::new(BodyBudget::default());
    let (context, body, headers) = context(Body::from("data"), length(4));
    let wire = context
        .collect(body, &headers, 1024, &Default::default(), &budget, "test")
        .await
        .unwrap();
    let peak = budget.snapshot().memory_peak_bytes;
    let again = context
        .collect(
            pending(),
            &headers,
            1024,
            &Default::default(),
            &budget,
            "test",
        )
        .await
        .unwrap();
    assert_eq!(wire.as_ptr(), again.as_ptr());
    assert_eq!(budget.snapshot().memory_peak_bytes, peak);
    assert_eq!(budget.snapshot().completed_total, 1);
    let clone = wire.clone();
    let outgoing = reqwest::Body::from(wire);
    drop(again);
    drop(context);
    assert_eq!(budget.snapshot().memory_used_bytes, 4);
    drop(clone);
    assert_eq!(budget.snapshot().memory_used_bytes, 4);
    drop(outgoing);
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
}

#[tokio::test]
async fn budget_is_fail_fast_and_cancelled_uploads_release_capacity() {
    let budget = Arc::new(BodyBudget::default());
    let config = ProxyStreamConfig {
        request_memory_budget_mb: 16,
        ..Default::default()
    };
    let occupied = budget
        .reserve(16 * MIB, config.request_memory_budget_bytes())
        .unwrap();
    let (context, body, headers) = context(Body::from("x"), length(1));
    let error = context
        .collect(body, &headers, 1024, &config, &budget, "test")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::MemoryBudget);
    assert_eq!(error.kind.status(), StatusCode::SERVICE_UNAVAILABLE);
    drop(occupied);
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
    let (context, body, headers) = self::context(pending(), length(MIB));
    let worker_budget = budget.clone();
    let task = tokio::spawn(async move {
        context
            .collect(body, &headers, 2 * MIB, &config, &worker_budget, "test")
            .await
    });
    tokio::task::yield_now().await;
    assert_eq!(budget.snapshot().memory_used_bytes, MIB);
    task.abort();
    let _ = task.await;
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
    assert_eq!(budget.snapshot().cancelled_total, 1);
}

fn compressed(encoding: &str, bytes: &[u8]) -> Vec<u8> {
    match encoding {
        "gzip" => {
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Default::default());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        }
        "deflate" => {
            let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        }
        "zstd" => zstd::stream::encode_all(bytes, 1).unwrap(),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn bounded_decoding_preserves_wire_bytes_headers_and_signature_hash() {
    let json = br#"{"model":"gpt-6-astra","input":[{"role":"user","content":"keep history"}]}"#;
    for encoding in ["gzip", "deflate", "zstd"] {
        let raw = compressed(encoding, json);
        let hash = crate::api::sha256_hex(&raw);
        let mut headers = length(raw.len());
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
        let original_headers = headers.clone();
        let (context, body, headers) = context(Body::from(raw), headers);
        let budget = Arc::new(BodyBudget::default());
        let config = ProxyStreamConfig::default();
        let wire = context
            .collect(body, &headers, 4096, &config, &budget, "test")
            .await
            .unwrap();
        let decoded = context
            .decoded(&wire, &headers, 4096, &config, &budget, "test")
            .await
            .unwrap();
        assert_eq!(decoded.as_ref(), json);
        assert_eq!(
            json_model(&json_fields(&decoded).unwrap()).as_deref(),
            Some("gpt-6-astra")
        );
        assert_eq!(crate::api::sha256_hex(&wire), hash);
        assert_eq!(headers, original_headers);
        let peak = budget.snapshot().memory_peak_bytes;
        let again = context
            .decoded(&wire, &headers, 4096, &config, &budget, "test")
            .await
            .unwrap();
        assert_eq!(again.as_ptr(), decoded.as_ptr());
        assert_eq!(budget.snapshot().memory_peak_bytes, peak);
        drop((context, wire, decoded, again));
        assert_eq!(budget.snapshot().memory_used_bytes, 0);
    }
}

// A cached inspection must still obey the limit of a later caller.
#[tokio::test]
async fn cached_decoded_body_rechecks_each_callers_size_limit() {
    let json = br#"{"model":"gpt-6-astra"}"#;
    for encoding in ["identity", "gzip"] {
        let raw = if encoding == "identity" {
            json.to_vec()
        } else {
            compressed(encoding, json)
        };
        let mut headers = length(raw.len());
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
        let (context, body, headers) = context(Body::from(raw), headers);
        let budget = Arc::new(BodyBudget::default());
        let config = ProxyStreamConfig::default();
        let wire = context
            .collect(body, &headers, 4096, &config, &budget, "test")
            .await
            .unwrap();
        let decoded = context
            .decoded(&wire, &headers, 4096, &config, &budget, "test")
            .await
            .unwrap();
        let error = context
            .decoded(&wire, &headers, 4, &config, &budget, "test")
            .await
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::DecodedTooLarge);
        assert_eq!(error.received, wire.len());
        assert_eq!(
            error.inspected_bytes,
            (json.len() != wire.len()).then_some(json.len())
        );
        assert_eq!(budget.snapshot().too_large_total, 1);
        drop((context, wire, decoded));
        assert_eq!(budget.snapshot().memory_used_bytes, 0);
    }
}

#[tokio::test]
async fn decompression_bombs_bad_encodings_and_decode_admission_are_bounded() {
    let budget = Arc::new(BodyBudget::default());
    let config = ProxyStreamConfig::default();
    for (raw, encoding, expected) in [
        (
            compressed("gzip", &[b'a'; 16384]),
            "gzip",
            FailureKind::DecodedTooLarge,
        ),
        (
            compressed("zstd", &[b'a'; 16384]),
            "zstd",
            FailureKind::DecodedTooLarge,
        ),
        (b"broken".to_vec(), "gzip", FailureKind::InvalidEncoding),
        (b"broken".to_vec(), "zstd", FailureKind::InvalidEncoding),
        (
            b"broken".to_vec(),
            "gzip, zstd",
            FailureKind::UnsupportedEncoding,
        ),
        (b"broken".to_vec(), "br", FailureKind::UnsupportedEncoding),
    ] {
        let mut headers = length(raw.len());
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
        let (context, body, headers) = context(Body::from(raw), headers);
        let wire = context
            .collect(body, &headers, 4096, &config, &budget, "test")
            .await
            .unwrap();
        let error = context
            .decoded(&wire, &headers, 4096, &config, &budget, "test")
            .await
            .unwrap_err();
        assert_eq!(error.kind, expected);
        drop((context, wire));
        assert_eq!(budget.snapshot().memory_used_bytes, 0);
    }
    let slots = budget
        .decode_slots
        .clone()
        .try_acquire_many_owned(4)
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    let (context, _, headers) = context(Body::empty(), headers);
    let error = context
        .decoded(
            &Bytes::from(compressed("gzip", b"{}")),
            &headers,
            4096,
            &config,
            &budget,
            "test",
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::DecodeBusy);
    drop(slots);
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
}

#[test]
fn model_inspection_and_image_rewrite_do_not_expand_a_json_dom() {
    let body = br#"{"model":"gpt-6-astra","input":[{"tool":"keep","nested":[1,2,3]}],"stream":true,"response_format":"url","format":"jpeg","output_format":"webp","prompt":"preserve me"}"#;
    assert_eq!(
        json_model(&json_fields(body).unwrap()).as_deref(),
        Some("gpt-6-astra")
    );
    let budget = Arc::new(BodyBudget::default());
    let (result, model, format) = rewrite_image_request(body, &budget, 4 * MIB).unwrap();
    let expected: serde_json::Value = serde_json::from_slice(body).unwrap();
    let actual: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(model, "gpt-6-astra");
    assert_eq!(format, "webp");
    assert_eq!(actual["input"], expected["input"]);
    assert_eq!(actual["prompt"], expected["prompt"]);
    assert_eq!(actual["stream"], true);
    assert_eq!(actual["response_format"], "b64_json");
    assert_eq!(budget.snapshot().memory_used_bytes, body.len() + 128);
    drop(result);
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
    assert!(rewrite_image_request(b"[1,2]", &budget, MIB).is_err());
    assert!(rewrite_image_request(b"{} trailing", &budget, MIB).is_err());
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
}

#[test]
fn correlation_headers_are_trusted_and_session_is_hashed() {
    let mut request = Request::builder()
        .uri("/v1/responses")
        .header("cf-ray", "abc-LAX")
        .header("session_id", "private-session-id")
        .body(Body::empty())
        .unwrap();
    let context = BodyContext::attach(&mut request, "127.0.0.1:1234".parse().unwrap());
    assert!(context.cf_ray.is_none());
    assert_eq!(context.session_hash.as_ref().unwrap().len(), 16);
    assert_ne!(context.session_hash.as_deref(), Some("private-session-id"));
    let mut request = Request::builder()
        .uri("/")
        .header("cf-ray", "abc-LAX")
        .body(Body::empty())
        .unwrap();
    let trusted = BodyContext::attach(&mut request, "173.245.48.1:1234".parse().unwrap());
    assert_eq!(trusted.cf_ray.as_deref(), Some("abc-LAX"));
}

#[test]
fn buffer_growth_charges_old_and_new_allocations_and_rolls_back_failure() {
    let budget = Arc::new(BodyBudget::default());
    let mut buffer = Buffer::new(&budget, 4, 12).unwrap();
    buffer.append(b"1234", 32, 12).unwrap();
    buffer.append(b"5678", 32, 12).unwrap();
    assert_eq!(budget.snapshot().memory_used_bytes, 8);
    assert_eq!(budget.snapshot().memory_peak_bytes, 12);
    assert_eq!(
        buffer.append(b"9", 32, 12).unwrap_err(),
        FailureKind::MemoryBudget
    );
    assert_eq!(buffer.data, b"12345678");
    assert_eq!(budget.snapshot().memory_used_bytes, 8);
    drop(buffer);
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
}

#[tokio::test]
async fn concurrent_uploads_cannot_exceed_shared_budget_and_cancel_cleanly() {
    let budget = Arc::new(BodyBudget::default());
    let config = ProxyStreamConfig {
        request_memory_budget_mb: 16,
        ..Default::default()
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let budget = budget.clone();
        let config = config.clone();
        let tx = tx.clone();
        tasks.push(tokio::spawn(async move {
            let body = Body::from_stream(async_stream::stream! {
                tx.send(()).unwrap();
                yield Ok::<_, Infallible>(Bytes::from(vec![b'x'; MIB]));
                futures_util::future::pending::<()>().await;
            });
            let (context, body, headers) = context(body, length(2 * MIB));
            context
                .collect(body, &headers, 2 * MIB, &config, &budget, "concurrent")
                .await
        }));
    }
    drop(tx);
    for _ in 0..8 {
        rx.recv().await.unwrap();
    }
    tokio::task::yield_now().await;
    assert!(budget.snapshot().memory_used_bytes <= 16 * MIB);
    assert!(budget.snapshot().memory_peak_bytes <= 16 * MIB);
    assert!(budget.snapshot().capacity_rejected_total > 0);
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
}

#[tokio::test]
async fn cancelling_a_running_blocking_job_keeps_its_lease_until_it_finishes() {
    let budget = Arc::new(BodyBudget::default());
    let lease = budget.reserve(1024, 2048).unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let job = AbortJob(tokio::task::spawn_blocking(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        drop(lease);
        finished_tx.send(()).unwrap();
    }));
    started_rx.await.unwrap();
    drop(job);
    assert_eq!(budget.snapshot().memory_used_bytes, 1024);
    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
}

#[tokio::test]
async fn deflate_trailing_data_and_large_zstd_windows_are_rejected() {
    let budget = Arc::new(BodyBudget::default());
    let config = ProxyStreamConfig::default();
    let mut raw = compressed("deflate", b"{}");
    raw.extend_from_slice(b"hidden trailing payload");
    let mut headers = length(raw.len());
    headers.insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static("deflate"),
    );
    let (context, body, headers) = context(Body::from(raw), headers);
    let wire = context
        .collect(body, &headers, 4096, &config, &budget, "test")
        .await
        .unwrap();
    let error = context
        .decoded(&wire, &headers, 4096, &config, &budget, "test")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::InvalidEncoding);
    drop((context, wire));
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
    // Non-single-segment zstd frame advertising a 128 MiB window. The maximum
    // accepted window is 8 MiB; reject before allocating the advertised size.
    let wire = Bytes::from_static(&[0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x88, 0x01, 0x00, 0x00]);
    let mut headers = length(wire.len());
    headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));
    let (context, _, headers) = self::context(Body::empty(), headers);
    let error = context
        .decoded(&wire, &headers, 4096, &config, &budget, "test")
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::InvalidEncoding);
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
}

async fn local_server(
    config: ProxyStreamConfig,
    forwarded: Arc<AtomicUsize>,
) -> (SocketAddr, tokio::task::JoinHandle<()>, Arc<BodyBudget>) {
    let budget = Arc::new(BodyBudget::default());
    let handler_budget = budget.clone();
    let app =
        axum::Router::new().fallback(axum::routing::any(move |mut request: Request<Body>| {
            let budget = handler_budget.clone();
            let forwarded = forwarded.clone();
            let config = config.clone();
            async move {
                let context = BodyContext::attach(&mut request, "127.0.0.1:1234".parse().unwrap());
                let (parts, body) = request.into_parts();
                let response = match context
                    .collect(body, &parts.headers, 1024 * 1024, &config, &budget, "test")
                    .await
                {
                    Ok(_) => {
                        forwarded.fetch_add(1, Ordering::Relaxed);
                        (StatusCode::OK, "ok").into_response()
                    }
                    Err(error) => context.error_response(&error, true),
                };
                context.finish(response)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, task, budget)
}

#[tokio::test]
async fn http1_timeout_closes_unread_body_and_same_session_retry_recovers() {
    let forwarded = Arc::new(AtomicUsize::new(0));
    // Zero is test-only, to exercise the actual HTTP stack without sleeping.
    let config = ProxyStreamConfig {
        request_body_first_byte_timeout_secs: 0,
        ..Default::default()
    };
    let (address, server, budget) = local_server(config, forwarded.clone()).await;
    let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
    connection.write_all(b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\nsession_id: same-long-session\r\n\r\n").await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(3),
        connection.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    let response = String::from_utf8(response).unwrap().to_lowercase();
    assert!(response.starts_with("http/1.1 408"), "{response}");
    assert!(response.contains("connection: close"));
    assert_eq!(forwarded.load(Ordering::Relaxed), 0);
    server.abort();
    let _ = server.await;
    assert_eq!(budget.snapshot().memory_used_bytes, 0);

    let (address, server, budget) = local_server(Default::default(), forwarded.clone()).await;
    // Retry the SAME history and session with a completed upload, not a new
    // session, and verify exactly one dispatch.
    let json = serde_json::json!({"model":"gpt-6-astra","input":"a".repeat(500_000)}).to_string();
    let response = reqwest::Client::new()
        .post(format!("http://{address}/v1/responses"))
        .header("session_id", "same-long-session")
        .body(json)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(forwarded.load(Ordering::Relaxed), 1);
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
    server.abort();
}

#[tokio::test]
async fn http1_chunked_upload_and_keep_alive_remain_usable() {
    let forwarded = Arc::new(AtomicUsize::new(0));
    let (address, server, _) = local_server(Default::default(), forwarded.clone()).await;
    let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
    connection.write_all(b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ndata\r\n0\r\n\r\n").await.unwrap();
    let mut buffer = [0; 4096];
    let count = tokio::time::timeout(Duration::from_secs(3), connection.read(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&buffer[..count]).starts_with("HTTP/1.1 200"));
    connection.write_all(b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata").await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(3),
        connection.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(String::from_utf8_lossy(&response).contains("HTTP/1.1 200"));
    assert_eq!(forwarded.load(Ordering::Relaxed), 2);
    server.abort();
}

#[tokio::test]
async fn http2_early_rejection_does_not_close_the_connection() {
    let forwarded = Arc::new(AtomicUsize::new(0));
    let config = ProxyStreamConfig {
        request_body_first_byte_timeout_secs: 0,
        ..Default::default()
    };
    let (address, server, budget) = local_server(config, forwarded.clone()).await;
    let stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http2::handshake::<_, _, Body>(
        hyper_util::rt::TokioExecutor::new(),
        hyper_util::rt::TokioIo::new(stream),
    )
    .await
    .unwrap();
    let connection = tokio::spawn(connection);
    let first = sender
        .send_request(
            Request::builder()
                .method("POST")
                .uri(format!("http://{address}/v1/responses"))
                .header(header::CONTENT_LENGTH, "100")
                .body(pending())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::REQUEST_TIMEOUT);
    assert!(!first.headers().contains_key(header::CONNECTION));
    let _ = first.into_body().collect().await.unwrap();
    // Empty body completes immediately, even with a zero test-only timeout.
    let request = Request::builder()
        .uri(format!("http://{address}/v1/responses"))
        .body(Body::empty())
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!connection.is_finished());
    assert_eq!(forwarded.load(Ordering::Relaxed), 1);
    assert_eq!(budget.snapshot().memory_used_bytes, 0);
    connection.abort();
    server.abort();
}
