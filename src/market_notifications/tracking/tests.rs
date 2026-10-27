use super::*;
use crate::market_notifications::delivery::{Prepared, finish_attempt_tx, prepare_tx};
const AT: &str = "2026-10-27T00:00:00+00:00";
const KEY: &str = "mock-resend-key";
fn time(seconds: i64) -> String {
    (DateTime::parse_from_rfc3339(AT).unwrap() + chrono::Duration::seconds(seconds)).to_rfc3339()
}
fn fingerprint() -> String {
    hex::encode(Sha256::digest(KEY.as_bytes()))
}
fn task(c: &Connection) -> Claimed {
    enqueue_user_notification_tx(
        c,
        "buyer",
        "buyer",
        "wallet_transfer_received",
        "transfer",
        "x",
        "event",
        json!({}),
        AT,
    )
    .unwrap();
    Claimed {
        id: c
            .query_row(
                "SELECT id FROM market_notification_outbox WHERE dedupe_key='event'",
                [],
                |r| r.get(0),
            )
            .unwrap(),
        recipient_user_id: "buyer".into(),
        event_kind: "wallet_transfer_received".into(),
        mandatory: true,
        attempts: 1,
        payload: json!({"notificationTime":AT,"notificationEventKey":"event"}),
    }
}
fn attempt(c: &Connection, i: &Claimed) -> super::super::delivery::Attempt {
    let Prepared::Send(a) = prepare_tx(
        c,
        i,
        "email",
        "receipt",
        json!({"credentialFingerprint":fingerprint()}),
        true,
        AT,
    )
    .unwrap() else {
        panic!("expected send")
    };
    a
}
fn accepted(c: &Connection, message: &str) -> (Claimed, String) {
    let i = task(c);
    let a = attempt(c, &i);
    finish_attempt_tx(c, &a, &Ok(message.into()), AT).unwrap();
    (i, a.id)
}
fn event(message: &str, kind: &str, seconds: i64) -> Vec<u8> {
    serde_json::to_vec(&json!({"type":format!("email.{kind}"),"created_at":time(seconds),
    "data":{"email_id":message,"to":["NEVER_STORE@example.test"],"subject":"DO_NOT_STORE","failed":{"reason":"reached_daily_quota"}}})).unwrap()
}
fn provider_status(c: &Connection, id: &str) -> String {
    c.query_row(
        "SELECT provider_status FROM market_notification_deliveries WHERE id=?1",
        params![id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn svix_signature_verifies_raw_body_timestamp_and_multiple_signatures() {
    let body = b"{\"type\":\"email.delivered\"}";
    let key = [7u8; 32];
    let secret = format!("whsec_{}", STANDARD.encode(key));
    let now = DateTime::parse_from_rfc3339(AT)
        .unwrap()
        .with_timezone(&Utc);
    let timestamp = now.timestamp().to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
    mac.update(format!("msg_event.{timestamp}.").as_bytes());
    mac.update(body);
    let signature = format!(
        "v1,{} v1,{}",
        STANDARD.encode([0u8; 32]),
        STANDARD.encode(mac.finalize().into_bytes())
    );
    let mut headers = HeaderMap::new();
    headers.insert("svix-id", "msg_event".parse().unwrap());
    headers.insert("svix-timestamp", timestamp.parse().unwrap());
    headers.insert("svix-signature", signature.parse().unwrap());
    assert_eq!(verify(&secret, &headers, body, now).unwrap(), "msg_event");
    assert!(verify(&secret, &headers, b"{}", now).is_err());
    assert!(
        verify(
            &secret,
            &headers,
            body,
            now + chrono::Duration::seconds(301)
        )
        .is_err()
    );
    assert!(
        verify(
            &secret,
            &headers,
            body,
            now - chrono::Duration::seconds(301)
        )
        .is_err()
    );
    assert!(verify("bad-secret", &headers, body, now).is_err());
    headers.remove("svix-signature");
    assert!(verify(&secret, &headers, body, now).is_err());
}
#[test]
fn provider_failure_is_separate_from_acceptance_and_never_requeues_mail() {
    let c = crate::test_support::database().unwrap();
    let message = Uuid::new_v4().to_string();
    let (i, id) = accepted(&c, &message);
    assert_eq!(provider_status(&c, &id), "accepted");
    assert!(webhook_tx(&c, "failed-event", &event(&message, "failed", 1), &time(2)).unwrap());
    assert!(!webhook_tx(&c, "failed-event", &event(&message, "failed", 1), &time(3)).unwrap());
    assert_eq!(provider_status(&c, &id), "failed");
    assert!(
        matches!(prepare_tx(&c,&i,"email","receipt",json!({}),true,&time(4)).unwrap(),Prepared::Done(s)if s=="sent")
    );
    assert_eq!(
        c.query_row(
            "SELECT attempts FROM market_notification_deliveries WHERE id=?1",
            params![id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let raw = c
        .query_row(
            "SELECT error_code FROM market_notification_provider_events",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(raw, "reached_daily_quota");
    assert!(error_code(Some("SMTP user@example.test SECRET_TOKEN")).is_none());
}
#[test]
fn duplicate_and_out_of_order_observations_do_not_regress_delivery() {
    let c = crate::test_support::database().unwrap();
    let message = Uuid::new_v4().to_string();
    let (_, id) = accepted(&c, &message);
    for (key, kind, seconds) in [
        ("delayed", "delivery_delayed", 1),
        ("delivered", "delivered", 2),
        ("old", "sent", 0),
        ("late-delay", "delivery_delayed", 3),
    ] {
        webhook_tx(&c, key, &event(&message, kind, seconds), &time(4)).unwrap();
    }
    assert_eq!(provider_status(&c, &id), "delivered");
    webhook_tx(&c, "complaint", &event(&message, "complained", 5), &time(6)).unwrap();
    assert_eq!(provider_status(&c, &id), "complained");
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM market_notification_deliveries WHERE status='sent'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}
#[test]
fn webhook_before_send_response_is_bound_when_acceptance_is_recorded() {
    let c = crate::test_support::database().unwrap();
    let message = Uuid::new_v4().to_string();
    let i = task(&c);
    let a = attempt(&c, &i);
    webhook_tx(&c, "early", &event(&message, "delivered", 1), &time(2)).unwrap();
    finish_attempt_tx(&c, &a, &Ok(message), &time(3)).unwrap();
    assert_eq!(provider_status(&c, &a.id), "delivered");
}
#[test]
fn unknown_signed_events_are_bounded_and_retained_without_private_payloads() {
    let c = crate::test_support::database().unwrap();
    let message = Uuid::new_v4().to_string();
    c.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<1000)
        INSERT INTO market_notification_provider_events(id,provider_message_id,status,occurred_at,received_at,source)
        SELECT 'unknown-'||i,'unbound-'||i,'accepted',?1,?1,'webhook' FROM n",params![AT]).unwrap();
    assert!(!webhook_tx(&c, "over-cap", &event(&message, "delivered", 0), AT).unwrap());
    cleanup_tx(&c, &time(86401)).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM market_notification_provider_events",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    webhook_tx(
        &c,
        "normalized",
        &event(&message, "delivered", 86401),
        &time(86402),
    )
    .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM market_notification_provider_events",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}
#[test]
fn polling_leases_backoff_and_webhooks_never_reopen_submission() {
    let c = crate::test_support::database().unwrap();
    let message = Uuid::new_v4().to_string();
    let (_, id) = accepted(&c, &message);
    let old = claim_poll_tx(&c, &fingerprint(), AT).unwrap().unwrap();
    assert!(claim_poll_tx(&c, &fingerprint(), AT).unwrap().is_none());
    let current = claim_poll_tx(&c, &fingerprint(), &time(61))
        .unwrap()
        .unwrap();
    finish_poll_tx(&c, &old, Ok("delivered"), &time(62)).unwrap();
    assert_eq!(provider_status(&c, &id), "accepted");
    finish_poll_tx(
        &c,
        &current,
        Err(PollFailure {
            code: "tracking_read_permission_required",
            delay: 21600,
            global: true,
        }),
        &time(62),
    )
    .unwrap();
    assert!(
        claim_poll_tx(&c, &fingerprint(), &time(1000))
            .unwrap()
            .is_none()
    );
    webhook_tx(
        &c,
        "despite-poll-permission",
        &event(&message, "delivered", 1001),
        &time(1002),
    )
    .unwrap();
    assert_eq!(provider_status(&c, &id), "delivered");
}
#[test]
fn credential_rotation_does_not_query_old_mail_with_a_new_merchant_key() {
    let c = crate::test_support::database().unwrap();
    let message = Uuid::new_v4().to_string();
    let (_, id) = accepted(&c, &message);
    assert!(
        claim_poll_tx(&c, "different-account", AT)
            .unwrap()
            .is_none()
    );
    assert_eq!(provider_status(&c, &id), "accepted");
    assert_eq!(
        c.query_row(
            "SELECT provider_error FROM market_notification_deliveries WHERE id=?1",
            params![id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "tracking_credential_changed"
    );
}

#[test]
fn webhook_ingress_is_bounded_per_source_and_globally_and_resets_after_one_minute() {
    let mut budget = WebhookBudget::default();
    let now = std::time::Instant::now();
    for n in 1..=5 {
        let ip = std::net::IpAddr::from([192, 0, 2, n]);
        for _ in 0..120 {
            assert!(budget.allow(ip, now));
        }
        assert!(!budget.allow(ip, now));
    }
    assert!(!budget.allow(std::net::IpAddr::from([192, 0, 2, 6]), now));
    assert!(budget.allow(
        std::net::IpAddr::from([192, 0, 2, 6]),
        now + Duration::from_secs(60)
    ));
}
#[tokio::test]
async fn polling_uses_get_and_checks_response_identity_and_permissions() {
    let message = Uuid::new_v4().to_string();
    for (status, value, expected) in [
        (
            200,
            json!({"id":message,"last_event":"delivery_delayed"}),
            "delayed",
        ),
        (
            200,
            json!({"id":"another-id","last_event":"delivered"}),
            "tracking_identity_mismatch",
        ),
        (
            403,
            json!({"secret":"MUST_NOT_LOG"}),
            "tracking_read_permission_required",
        ),
        (429, json!({}), "tracking_rate_limited"),
    ] {
        let app = Router::new().route(
            "/emails/:id",
            get(move |headers: HeaderMap| {
                let value = value.clone();
                async move {
                    assert_eq!(
                        headers.get("authorization").unwrap(),
                        "Bearer mock-resend-key"
                    );
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        Json(value),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/emails", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        match lookup(&reqwest::Client::new(), KEY, &message, &endpoint).await {
            Ok(s) => assert_eq!(s, expected),
            Err(e) => assert_eq!(e.code, expected),
        }
        server.abort();
    }
}
