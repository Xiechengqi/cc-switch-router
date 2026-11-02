use super::*;

#[test]
fn migration_108_restores_only_proven_attempt_context_without_rewriting_work_or_money() {
    let conn = memory_connection();
    install_schema_through(&conn, 107);
    let at = "2026-11-02T14:41:00+00:00";
    crate::market_wallet::credit_verified_topup_tx(
        &conn,
        "buyer",
        "recovery-topup",
        100,
        "receipt",
        at,
    )
    .unwrap();
    let hold = crate::market_wallet::reserve_wallet_tx(
        &conn,
        "buyer",
        "platform_contract",
        "contract",
        "activation",
        86_400,
        None,
        at,
    )
    .unwrap();
    let wallet =
        serde_json::to_value(crate::market_wallet::wallet_snapshot_tx(&conn, "buyer").unwrap())
            .unwrap();
    conn.execute("INSERT INTO share_market_listings(id,share_id,installation_id,owner_user_id,owner_email,status,created_at,updated_at) VALUES('listing','share','installation','provider','provider@example.com','active',?1,?1)", params![at]).unwrap();
    conn.execute("INSERT INTO share_market_seats(id,listing_id,position,status,token_period_json,created_at,updated_at) VALUES('seat','listing',1,'revoking','\"day\"',?1,?1)", params![at]).unwrap();
    conn.execute("INSERT INTO share_market_subscriptions(id,seat_id,listing_id,share_id,installation_id,entitlement_id,owner_user_id,owner_email,renter_user_id,renter_email,status,token_period_json,offer_revision,created_at,updated_at)
        VALUES('sub','seat','listing','share','installation','entitlement','provider','provider@example.com','buyer','buyer@example.com','revoke_pending','\"day\"',1,?1,?1)", params![at]).unwrap();
    for (id, seq) in [("known", 5), ("unknown", 6)] {
        conn.execute("INSERT INTO share_control_operations(id,share_id,share_sequence,entitlement_id,subscription_id,action,email,status,attempts,created_at,updated_at,next_attempt_at,last_error)
            VALUES(?1,'share',?2,'entitlement','sub','revoke','buyer@example.com','pending',7,?3,?3,'2026-11-02T15:47:07+00:00','timeout')", params![id,seq,at]).unwrap();
    }
    let patch = serde_json::json!({"managedGrant":{"operationId":"known","entitlementId":"entitlement","shareSequence":5,"expectedConfigRevision":5,"action":"revoke","email":"buyer@example.com"}});
    for (id, revision, installation, payload) in [
        ("original", 10, "installation", patch.clone()),
        ("foreign", 11, "wrong-installation", patch.clone()),
        ("wrong-entitlement", 12, "installation", {
            let mut p = patch.clone();
            p["managedGrant"]["entitlementId"] = "wrong".into();
            p
        }),
        (
            "malformed",
            13,
            "installation",
            serde_json::json!({"managedGrant":{"operationId":"known"}}),
        ),
    ] {
        conn.execute("INSERT INTO share_edit_requests(id,share_id,installation_id,owner_email,revision,status,patch_json,created_by_email,created_at,updated_at,error_code)
            VALUES(?1,'share',?2,'provider@example.com',?3,'rejected',?4,'share-market@router.internal',?5,?5,'control_ack_timeout')", params![id,installation,revision,payload.to_string(),at]).unwrap();
    }
    let history = |conn: &Connection| -> Vec<(String, String, i64, String)> {
        conn.prepare(
            "SELECT id,status,attempts,next_attempt_at FROM share_control_operations ORDER BY id",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    let before = history(&conn);
    apply(&conn).unwrap();
    apply(&conn).unwrap();
    assert_eq!(history(&conn), before);
    assert_eq!(conn.query_row("SELECT last_attempt_edit_id,last_attempt_config_revision,last_error_code,last_recovery_revision FROM share_control_operations WHERE id='known'", [],
        |r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<i64>>(3)?))).unwrap(), ("original".into(),5,"control_ack_timeout".into(),None));
    assert_eq!(
        conn.query_row(
            "SELECT last_attempt_edit_id FROM share_control_operations WHERE id='unknown'",
            [],
            |r| r.get::<_, Option<String>>(0)
        )
        .unwrap(),
        None
    );
    assert_eq!(
        serde_json::to_value(crate::market_wallet::wallet_snapshot_tx(&conn, "buyer").unwrap())
            .unwrap(),
        wallet
    );
    assert_eq!(
        conn.query_row(
            "SELECT status FROM market_wallet_holds WHERE id=?1",
            params![hold],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "active"
    );
    assert_eq!(
        conn.query_row(
            "SELECT status FROM share_market_subscriptions WHERE id='sub'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "revoke_pending"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM share_edit_requests", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
}
