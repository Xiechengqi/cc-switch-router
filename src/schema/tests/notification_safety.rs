use super::*;

#[test]
fn migration_91_preserves_history_envelopes_money_and_attempted_idempotency_keys() {
    let c = memory_connection();
    install_schema_through(&c, 90);
    let at = "2026-10-27T00:00:00Z";
    c.execute("INSERT INTO market_notification_outbox(id,audience,recipient_user_id,event_kind,object_kind,object_id,dedupe_key,status,next_attempt_at,created_at)
        VALUES('old-event','buyer','buyer','contract_suspended','platform_contract','service','old-key','sent',?1,?1)",params![at]).unwrap();
    c.execute("INSERT INTO market_notification_deliveries(id,event_id,recipient_user_id,channel,purpose,semantic_key,status,envelope_json,idempotency_key,attempts,first_attempt_at,next_attempt_at,created_at,sent_at,provider_message_id)
        VALUES('old-mail','old-event','buyer','email','service_paused','old-semantic','sent','{\"frozen\":true}','old-provider-key',2,?1,?1,?1,?1,'old-provider-id')",params![at]).unwrap();
    crate::market_wallet::credit_verified_topup_tx(
        &c,
        "buyer",
        "original-topup",
        100,
        "receipt",
        at,
    )
    .unwrap();
    let before = crate::market_wallet::wallet_snapshot_tx(&c, "buyer").unwrap();
    apply(&c).unwrap();
    apply(&c).unwrap();
    let row=c.query_row("SELECT status,envelope_json,idempotency_key,semantic_key,attempts,provider_message_id,provider_status FROM market_notification_deliveries WHERE id='old-mail'",[],
        |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?))).unwrap();
    assert_eq!(
        row,
        (
            "sent".into(),
            "{\"frozen\":true}".into(),
            "old-provider-key".into(),
            "old-semantic".into(),
            2,
            "old-provider-id".into(),
            "accepted".into()
        )
    );
    assert_eq!(
        crate::market_wallet::wallet_snapshot_tx(&c, "buyer")
            .unwrap()
            .posted_units,
        before.posted_units
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM market_notification_outbox WHERE status='queued'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        CURRENT_SCHEMA_TABLE_COUNT
    );
}
