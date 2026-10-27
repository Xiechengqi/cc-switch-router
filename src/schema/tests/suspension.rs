use super::*;

fn legacy_trial_suspension(conn: &Connection, id: &str) -> Vec<String> {
    let start = "2026-10-26T00:00:00+00:00";
    conn.execute(
        "INSERT INTO market_platform_contracts
         (id,product_kind,product_ref,trial_scope_ref,quote_id,request_fingerprint,
          buyer_user_id,provider_user_id,offer_revision,billing_model,cycle_price_minor,
          renewal_policy,status,activated_at,last_evaluated_at,health_fence_at,created_at,updated_at)
         VALUES(?1,'client_host',?1,?1,?1,?1,'buyer','provider',1,'calendar_month',1000,
                'manual','trialing',?2,?2,?2,?2,?2)",
        params![id,start],
    ).unwrap();
    conn.execute(
        "INSERT INTO market_platform_trial_segments
         (id,contract_id,source_kind,source_id,started_at,ended_at,service_available,consumed_seconds,created_at)
         VALUES(?1,?1,'client_host_health',?1,?2,'2026-10-26T00:00:05+00:00',1,5,?2)",
        params![id,start],
    ).unwrap();
    let mut ids = Vec::new();
    for at in [
        "2026-10-26T00:00:05+00:00",
        "2026-10-26T00:00:10+00:00",
        "2026-10-26T00:00:15+00:00",
    ] {
        // Reproduce the old bug, including the suspension timestamp trigger.
        conn.execute(
            "UPDATE market_platform_contracts SET status='trialing',last_evaluated_at=?2,health_fence_at=?2,updated_at=?2 WHERE id=?1",
            params![id, at],
        )
        .unwrap();
        conn.execute(
            "UPDATE market_platform_contracts SET status='suspended',updated_at=?2 WHERE id=?1",
            params![id, at],
        )
        .unwrap();
        let key = format!("contract_suspended:{id}:{at}");
        crate::market_notifications::enqueue_user_notification_tx(
            conn,"buyer","buyer","contract_suspended","platform_contract",id,&key,
            serde_json::json!({"contractId":id,"productKind":"client_host","reason":"insufficient_funds","status":"suspended"}),at,
        ).unwrap();
        ids.push(
            conn.query_row(
                "SELECT id FROM market_notification_outbox WHERE dedupe_key=?1",
                params![key],
                |r| r.get(0),
            )
            .unwrap(),
        );
    }
    ids
}

#[test]
fn migration_89_restores_only_proven_never_paid_trial_suspensions() {
    let conn = Connection::open_in_memory().unwrap();
    install_schema_through(&conn, 88);
    for id in [
        "proven",
        "finance-held",
        "paid",
        "really-resumed",
        "unknown",
        "malformed",
        "unbound",
    ] {
        legacy_trial_suspension(&conn, id);
    }
    conn.execute("UPDATE market_platform_contracts SET suspension_cause='finance_hold' WHERE id='finance-held'",[]).unwrap();
    conn.execute(
        "UPDATE market_platform_contracts SET paid_started_at=activated_at WHERE id='paid'",
        [],
    )
    .unwrap();
    crate::market_notifications::enqueue_user_notification_tx(
        &conn,
        "buyer",
        "buyer",
        "contract_resumed",
        "platform_contract",
        "really-resumed",
        "real-recovery",
        serde_json::json!({"contractId":"really-resumed"}),
        "2026-10-26T00:00:07+00:00",
    )
    .unwrap();
    conn.execute(
        "UPDATE market_notification_outbox SET payload_json='{}' WHERE object_id='unknown'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE market_notification_outbox SET payload_json='{' WHERE object_id='malformed'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE market_notification_outbox SET dedupe_key='unbound:'||id WHERE object_id='unbound'",
        [],
    )
    .unwrap();
    let snapshot = || {
        let mut s=conn.prepare("SELECT id,dedupe_key,payload_json,created_at FROM market_notification_outbox ORDER BY id").unwrap();
        s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    let before = snapshot();
    apply(&conn).unwrap();
    apply(&conn).unwrap();
    assert_eq!(snapshot(), before);
    for id in [
        "proven",
        "finance-held",
        "paid",
        "really-resumed",
        "unknown",
        "malformed",
        "unbound",
    ] {
        let at: String = conn
            .query_row(
                "SELECT suspended_at FROM market_platform_contracts WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            at,
            if id == "proven" {
                "2026-10-26T00:00:05+00:00"
            } else {
                "2026-10-26T00:00:15+00:00"
            },
            "{id}"
        );
        let cursor: String = conn
            .query_row(
                "SELECT last_evaluated_at FROM market_platform_contracts WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            cursor,
            if id == "proven" {
                "2026-10-26T00:00:05+00:00"
            } else {
                "2026-10-26T00:00:15+00:00"
            },
            "{id}"
        );
    }
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM market_journal_transactions",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn migration_89_suppresses_duplicate_backlog_without_erasing_receipts_or_frozen_attempts() {
    let conn = Connection::open_in_memory().unwrap();
    install_schema_through(&conn, 88);
    let ids = legacy_trial_suspension(&conn, "spammed");
    // The first notice is still queued, but a later duplicate was already
    // accepted. The canonical event must not send another email on upgrade.
    for (i, status) in ["queued", "sent", "claimed"].iter().enumerate() {
        conn.execute(
            "INSERT INTO market_notification_deliveries
             (id,event_id,recipient_user_id,channel,purpose,semantic_key,status,envelope_json,
              idempotency_key,provider_message_id,attempts,first_attempt_at,next_attempt_at,
              claim_token,claim_expires_at,created_at,sent_at)
             VALUES(?1,?2,'buyer','email','service_paused',?3,?4,'{\"text\":\"original frozen body\"}',
                    ?1,?5,3,'2026-10-26T00:00:16+00:00','2026-10-26T00:02:00+00:00',
                    ?6,?7,'2026-10-26T00:00:16+00:00',?8)",
            params![format!("original-key-{i}"),ids[i],format!("service_paused:event-{i}"),status,
                if *status=="sent"{Some("accepted-receipt")}else{None},
                if *status=="claimed"{Some("old-worker-token")}else{None},
                if *status=="claimed"{Some("2026-10-26T00:02:16+00:00")}else{None},
                if *status=="sent"{Some("2026-10-26T00:00:17+00:00")}else{None}],
        ).unwrap();
    }
    conn.execute("UPDATE market_notification_outbox SET status='sent',delivered_channels='email:sent',sent_at='2026-10-26T00:00:17+00:00' WHERE id=?1",params![ids[1]]).unwrap();
    conn.execute("UPDATE market_notification_outbox SET status='claimed',claimed_by='old-worker',claim_expires_at='2026-10-26T00:02:16+00:00' WHERE id=?1",params![ids[2]]).unwrap();
    crate::market_notifications::enqueue_user_notification_tx(
        &conn,
        "buyer",
        "buyer",
        "topup_credited",
        "topup",
        "unrelated",
        "unrelated",
        serde_json::json!({"amountMinor":100}),
        "2026-10-26T00:00:20+00:00",
    )
    .unwrap();
    let frozen = || {
        let mut s=conn.prepare("SELECT id,event_id,semantic_key,envelope_json,idempotency_key,provider_message_id,attempts,first_attempt_at,sent_at FROM market_notification_deliveries ORDER BY id").unwrap();
        s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    let before = frozen();
    apply(&conn).unwrap();
    apply(&conn).unwrap();
    assert_eq!(frozen(), before);
    for (i, status) in ["suppressed", "sent", "uncertain"].iter().enumerate() {
        assert_eq!(
            conn.query_row(
                "SELECT status FROM market_notification_deliveries WHERE id=?1",
                params![format!("original-key-{i}")],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            *status
        );
    }
    for (i, status) in ["suppressed", "sent", "failed"].iter().enumerate() {
        assert_eq!(
            conn.query_row(
                "SELECT status FROM market_notification_outbox WHERE id=?1",
                params![ids[i]],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            *status
        );
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM market_notification_outbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        conn.query_row(
            "SELECT status FROM market_notification_outbox WHERE dedupe_key='unrelated'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "queued"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM market_journal_transactions",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn migration_89_keeps_one_unsent_current_notice_and_suppresses_obsolete_episodes() {
    let conn = Connection::open_in_memory().unwrap();
    install_schema_through(&conn, 88);
    let ids = legacy_trial_suspension(&conn, "never-sent");
    let recovered = legacy_trial_suspension(&conn, "recovered");
    conn.execute(
        "UPDATE market_platform_contracts SET status='active' WHERE id='recovered'",
        [],
    )
    .unwrap();
    apply(&conn).unwrap();
    apply(&conn).unwrap();
    for (i, status) in ["queued", "suppressed", "suppressed"].iter().enumerate() {
        assert_eq!(
            conn.query_row(
                "SELECT status FROM market_notification_outbox WHERE id=?1",
                params![ids[i]],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            *status
        );
    }
    for id in recovered {
        assert_eq!(
            conn.query_row(
                "SELECT status FROM market_notification_outbox WHERE id=?1",
                params![id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "suppressed"
        );
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM market_notification_outbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        6
    );
}
