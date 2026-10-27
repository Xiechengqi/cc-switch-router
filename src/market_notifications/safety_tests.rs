use super::delivery::{Attempt, Prepared, finish_attempt_tx, prepare_with_limits_tx};
use super::*;
use crate::config::ServiceNoticeLimits;
const AT: &str = "2026-10-27T00:00:00+00:00";
fn later(seconds: i64) -> String {
    (DateTime::parse_from_rfc3339(AT).unwrap() + chrono::Duration::seconds(seconds)).to_rfc3339()
}
fn notice(c: &Connection, n: usize, kind: &str, object: &str, time: &str) -> Claimed {
    let key = format!("{kind}:{n}");
    enqueue_user_notification_tx(
        c,
        "buyer",
        "buyer",
        kind,
        "platform_contract",
        object,
        &key,
        json!({"contractId":object,"suspensionStartedAt":time}),
        time,
    )
    .unwrap();
    let id = c
        .query_row(
            "SELECT id FROM market_notification_outbox WHERE dedupe_key=?1",
            params![key],
            |r| r.get(0),
        )
        .unwrap();
    Claimed {
        id,
        recipient_user_id: "buyer".into(),
        event_kind: kind.into(),
        mandatory: true,
        attempts: 1,
        payload: json!({"contractId":object,"suspensionStartedAt":time,"notificationEventKey":key,"notificationTime":time}),
    }
}
fn prepare(c: &Connection, i: &Claimed, time: &str, limits: &ServiceNoticeLimits) -> Prepared {
    prepare_with_limits_tx(
        c,
        i,
        "email",
        delivery::purpose(i).unwrap(),
        json!({"target":"buyer@example.test","credentialFingerprint":"account"}),
        true,
        time,
        limits,
    )
    .unwrap()
}
fn send(p: Prepared) -> Attempt {
    if let Prepared::Send(a) = p {
        a
    } else {
        panic!("expected send")
    }
}
fn suppressed(p: Prepared) {
    assert!(matches!(p,Prepared::Done(s) if s=="suppressed"));
}

#[test]
fn changing_keys_cannot_bypass_pause_cooldown_and_retries_keep_their_original_slot() {
    let c = crate::test_support::database().unwrap();
    let limits = ServiceNoticeLimits::default();
    let first = notice(&c, 0, "contract_suspended", "service", AT);
    let a = send(prepare(&c, &first, AT, &limits));
    finish_attempt_tx(&c, &a, &Err(delivery::failure("temporary", true, None)), AT).unwrap();
    let a2 = send(prepare(&c, &first, &later(121), &limits));
    assert_eq!(a.key, a2.key);
    assert_eq!(a.id, a2.id);
    finish_attempt_tx(&c, &a2, &Ok(Uuid::new_v4().to_string()), &later(121)).unwrap();
    for n in 1..=120 {
        let at = later(n as i64 * 5);
        let i = notice(&c, n, "contract_suspended", "service", &at);
        suppressed(prepare(&c, &i, &at, &limits));
    }
    assert_eq!(c.query_row("SELECT COUNT(*) FROM market_notification_deliveries WHERE first_attempt_at IS NOT NULL",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM market_notification_outbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        121
    );
    assert_eq!(c.query_row("SELECT COUNT(*) FROM operator_alert_signal_outbox WHERE kind='market_service_notice_circuit'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert!(matches!(prepare(&c,&first,&later(122),&limits),Prepared::Done(s) if s=="sent"));
}
#[test]
fn genuine_later_episodes_have_a_rolling_daily_cap_and_other_services_remain_independent() {
    let c = crate::test_support::database().unwrap();
    let limits = ServiceNoticeLimits::default();
    for (n, at) in [0, 1801, 3602].into_iter().enumerate() {
        let time = later(at);
        let i = notice(&c, n, "contract_suspended", "service", &time);
        send(prepare(&c, &i, &time, &limits));
    }
    let time = later(5403);
    let fourth = notice(&c, 4, "contract_suspended", "service", &time);
    suppressed(prepare(&c, &fourth, &time, &limits));
    assert_eq!(
        c.query_row(
            "SELECT suppression_reason FROM market_notification_deliveries WHERE event_id=?1",
            params![fourth.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "service_pause_daily_budget"
    );
    let another = notice(&c, 5, "contract_suspended", "other-service", &time);
    send(prepare(&c, &another, &time, &limits));
    let next_day = later(86401);
    let i = notice(&c, 6, "contract_suspended", "service", &next_day);
    send(prepare(&c, &i, &next_day, &limits));
}
#[test]
fn service_circuit_does_not_spend_or_block_financial_receipt_slots() {
    let c = crate::test_support::database().unwrap();
    let limits = ServiceNoticeLimits {
        email_daily_limit: 1,
        ..Default::default()
    };
    let first = notice(&c, 0, "contract_suspended", "a", AT);
    send(prepare(&c, &first, AT, &limits));
    let second = notice(&c, 1, "contract_suspended", "b", AT);
    suppressed(prepare(&c, &second, AT, &limits));
    for (n, kind) in [
        "topup_credited",
        "wallet_transfer_sent",
        "wallet_transfer_received",
        "c2c_order_updated",
    ]
    .iter()
    .enumerate()
    {
        let mut i = notice(&c, 20 + n, kind, "receipt", AT);
        if *kind == "c2c_order_updated" {
            i.payload["action"] = json!("released");
        }
        send(prepare(&c, &i, AT, &limits));
    }
    let third = notice(&c, 10, "contract_suspended", "c", AT);
    suppressed(prepare(&c, &third, AT, &limits));
}
#[test]
fn user_budget_catches_cross_service_storms_without_spending_another_users_budget() {
    let c = crate::test_support::database().unwrap();
    let limits = ServiceNoticeLimits {
        user_hourly_limit: 1,
        ..Default::default()
    };
    let i = notice(&c, 0, "contract_resumed", "a", AT);
    send(prepare(&c, &i, AT, &limits));
    let i = notice(&c, 1, "contract_suspended", "b", AT);
    suppressed(prepare(&c, &i, AT, &limits));
    let mut i = notice(&c, 2, "contract_suspended", "c", AT);
    i.recipient_user_id = "another-user".into();
    send(prepare(&c, &i, AT, &limits));
}
#[test]
fn expired_instructions_are_not_replayed_and_ambiguous_attempts_are_not_erased() {
    let c = crate::test_support::database().unwrap();
    let limits = ServiceNoticeLimits::default();
    let first = notice(&c, 0, "contract_suspended", "a", AT);
    suppressed(prepare(&c, &first, &later(3601), &limits));
    let second = notice(&c, 1, "contract_suspended", "b", AT);
    let a = send(prepare(&c, &second, AT, &limits));
    finish_attempt_tx(
        &c,
        &a,
        &Err(delivery::failure("connection lost", true, None)),
        AT,
    )
    .unwrap();
    assert!(matches!(prepare(&c,&second,&later(3601),&limits),Prepared::Done(s) if s=="uncertain"));
    let future = notice(&c, 2, "contract_suspended", "c", &later(301));
    suppressed(prepare(&c, &future, AT, &limits));
}
#[test]
fn storm_is_coalesced_into_one_digest_and_does_not_replay_suppressed_messages() {
    let c = crate::test_support::database().unwrap();
    let limits = ServiceNoticeLimits::default();
    let first = notice(&c, 0, "contract_suspended", "a", AT);
    send(prepare(&c, &first, AT, &limits));
    for n in 1..20 {
        let time = later(n as i64);
        let i = notice(&c, n, "contract_suspended", "a", &time);
        suppressed(prepare(&c, &i, &time, &limits));
    }
    assert_eq!(
        safety::queue_digests_tx(&c, &later(1800), &limits).unwrap(),
        0
    );
    assert_eq!(
        safety::queue_digests_tx(&c, &later(1801), &limits).unwrap(),
        1
    );
    assert_eq!(
        safety::queue_digests_tx(&c, &later(2000), &limits).unwrap(),
        0
    );
    assert_eq!(c.query_row("SELECT COUNT(*) FROM market_notification_outbox WHERE event_kind='service_status_summary'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM market_notification_deliveries WHERE status='suppressed'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        19
    );
}
#[test]
fn financial_events_are_claimed_before_a_service_backlog() {
    let c = crate::test_support::database().unwrap();
    notice(&c, 0, "contract_suspended", "a", AT);
    let receipt = notice(&c, 1, "wallet_transfer_received", "t", &later(1));
    assert_eq!(claim_tx(&c, "worker", &later(2)).unwrap()[0].id, receipt.id);
}

#[test]
fn emergency_zero_budget_stops_previously_queued_retries_but_preserves_the_key() {
    let c = crate::test_support::database().unwrap();
    let limits = ServiceNoticeLimits::default();
    let i = notice(&c, 0, "contract_suspended", "a", AT);
    let a = send(prepare(&c, &i, AT, &limits));
    finish_attempt_tx(&c, &a, &Err(delivery::failure("timeout", true, None)), AT).unwrap();
    let stopped = ServiceNoticeLimits {
        email_daily_limit: 0,
        ..Default::default()
    };
    assert!(matches!(prepare(&c,&i,&later(121),&stopped),Prepared::Done(s) if s=="uncertain"));
    assert_eq!(
        c.query_row(
            "SELECT idempotency_key FROM market_notification_deliveries WHERE id=?1",
            params![a.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        a.key
    );
}

#[test]
fn concurrent_connections_cannot_both_claim_the_final_budget_slot() {
    let c = crate::test_support::database().unwrap();
    let first = notice(&c, 0, "contract_suspended", "a", AT);
    let second = notice(&c, 1, "contract_suspended", "b", AT);
    let path: String = c
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles = [first, second]
        .into_iter()
        .map(|item| {
            let barrier = barrier.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let connection = Connection::open(path).unwrap();
                connection
                    .execute_batch("PRAGMA busy_timeout=5000")
                    .unwrap();
                barrier.wait();
                let limits = ServiceNoticeLimits {
                    email_daily_limit: 1,
                    ..Default::default()
                };
                matches!(prepare(&connection, &item, AT, &limits), Prepared::Send(_))
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>(),
        1
    );
}
#[test]
fn service_mail_explains_reason_pause_start_and_original_event_time_in_english() {
    let c = crate::test_support::database().unwrap();
    let mut i = notice(&c, 0, "contract_suspended", "a", AT);
    i.payload["reason"] = json!("insufficient_funds");
    i.payload["serviceName"] = json!("My share");
    let (_, html, text) = render_email(&i, "https://router.example");
    for s in [
        "Service name: My share",
        "Insufficient available wallet balance",
        "Pause started: 2026-10-27 00:00:00 UTC",
        "Event occurred:",
        "Notification reference:",
    ] {
        assert!(text.contains(s), "{s}");
    }
    assert!(html.contains("<html lang=\"en\">"));
    assert!(text.is_ascii());
}

#[test]
fn service_names_follow_rental_and_provisioning_identities_not_contract_ids() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE market_platform_contracts(id,product_kind,product_ref,service_ref,buyer_user_id);
        CREATE TABLE share_market_subscriptions(id,share_id);CREATE TABLE shares(share_id,share_name);
        CREATE TABLE provisioning_jobs(id,subdomain);
        INSERT INTO market_platform_contracts VALUES('share-contract','share','rental',NULL,'buyer'),('client-contract','client_host','job','installation','buyer');
        INSERT INTO share_market_subscriptions VALUES('rental','share-id');INSERT INTO shares VALUES('share-id','summit--ashhppredz');
        INSERT INTO provisioning_jobs VALUES('job','host-example');").unwrap();
    for (contract, expected) in [
        ("share-contract", "summit--ashhppredz"),
        ("client-contract", "host-example"),
    ] {
        let mut i = Claimed {
            id: "event".into(),
            recipient_user_id: "buyer".into(),
            event_kind: "contract_suspended".into(),
            mandatory: true,
            attempts: 1,
            payload: json!({"contractId":contract}),
        };
        email::enrich_service_tx(&c, &mut i).unwrap();
        assert_eq!(i.payload["serviceName"], expected);
        i.recipient_user_id = "other-buyer".into();
        i.payload.as_object_mut().unwrap().remove("serviceName");
        email::enrich_service_tx(&c, &mut i).unwrap();
        assert!(i.payload.get("serviceName").is_none());
    }
}
