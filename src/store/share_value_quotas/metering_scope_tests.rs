async fn market_metering_scope_fixture() -> (AppStore, ShareDescriptor) {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    insert_installation(&store, "metering-installation").await;
    insert_share(
        &store,
        "metering-installation",
        "metering-share",
        "metering-share",
        "active",
    )
    .await;
    let mut grant = test_share_user_grant("buyer@example.com", "shareto", 100);
    grant.manager = crate::models::ShareGrantManager::RouterShareMarket;
    grant.entitlement_id = Some("old-entitlement".into());
    grant.active = false;
    grant.policy.token_limit = None;
    grant.policy.value_quota = Some(crate::share_value_pricing::ValueQuota {
        limit_usd_micros: "1000000".into(),
        tariff_policy_id: crate::share_value_pricing::TARIFF_POLICY.into(),
    });
    grant.value_metering = Some(crate::share_value_pricing::ValueMeteringBinding {
        quota_policy_epoch: "old-epoch".into(),
        value_metered_from_ms: 1000,
        effective_from_ms: 1000,
        pricing_revision: "pricing-revision".into(),
    });
    let mut share = test_share_descriptor("metering-share", "metering-share");
    share.contract_version = 10;
    share.user_grants = BTreeMap::from([(grant.email.clone(), grant)]);
    {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE shares SET user_grants_json=?1 WHERE share_id='metering-share'",
            params![serde_json::to_string(&share.user_grants).unwrap()],
        )
        .unwrap();
        let at = "2026-11-02T00:00:00Z";
        conn.execute("INSERT INTO share_market_listings(id,share_id,installation_id,owner_user_id,owner_email,status,created_at,updated_at)
            VALUES('metering-listing','metering-share','metering-installation','provider','owner@example.com','active',?1,?1)", params![at]).unwrap();
        conn.execute("INSERT INTO share_market_seats(id,listing_id,position,status,token_period_json,current_subscription_id,created_at,updated_at)
            VALUES('metering-seat','metering-listing',1,'occupied','\"day\"','new-rental',?1,?1)", params![at]).unwrap();
        for (id, entitlement, status) in [
            ("old-rental", "old-entitlement", "released"),
            ("new-rental", "new-entitlement", "revoke_pending"),
        ] {
            conn.execute("INSERT INTO share_market_subscriptions(id,seat_id,listing_id,share_id,installation_id,entitlement_id,
                owner_user_id,owner_email,renter_user_id,renter_email,status,token_period_json,offer_revision,created_at,updated_at)
                VALUES(?1,'metering-seat','metering-listing','metering-share','metering-installation',?2,
                'provider','owner@example.com','buyer','buyer@example.com',?3,'\"day\"',1,?4,?4)", params![id,entitlement,status,at]).unwrap();
        }
        // Exactly the production failure: the first upsert reached the Server,
        // but was retired before its ACK; the buyer is already returning it.
        conn.execute("INSERT INTO share_control_operations(id,share_id,share_sequence,entitlement_id,subscription_id,action,email,status,attempts,created_at,updated_at)
            VALUES('new-upsert','metering-share',1,'new-entitlement','new-rental','upsert','buyer@example.com','rejected',1,?1,?1)", params![at]).unwrap();
    }
    let grant = share.user_grants.get_mut("buyer@example.com").unwrap();
    grant.entitlement_id = Some("new-entitlement".into());
    grant.value_metering.as_mut().unwrap().value_metered_from_ms = 2000;
    grant.value_metering.as_mut().unwrap().effective_from_ms = 2000;
    grant.value_metering.as_mut().unwrap().quota_policy_epoch = "new-epoch".into();
    (store, share)
}

#[tokio::test]
async fn share_value_market_metering_origin_accepts_issued_rerent_and_inactive_cleanup_snapshot() {
    let (store, mut next) = market_metering_scope_fixture().await;
    let conn = store.conn.lock().await;
    // The grant may already be inactive after local revoke. Its new binding
    // must still sync so a revision-conflict ACK can unblock Router cleanup.
    assert!(!next.user_grants["buyer@example.com"].active);
    merge_account_quota_projections(&conn, &mut next).unwrap();
    next.user_grants
        .get_mut("buyer@example.com")
        .unwrap()
        .active = true;
    merge_account_quota_projections(&conn, &mut next).unwrap();
    conn.execute(
        "UPDATE shares SET user_grants_json=?1 WHERE share_id='metering-share'",
        params![serde_json::to_string(&next.user_grants).unwrap()],
    )
    .unwrap();
    // Suspend/resume and trial-to-paid changes keep the same origin.
    let grant = next.user_grants.get_mut("buyer@example.com").unwrap();
    grant.value_metering.as_mut().unwrap().quota_policy_epoch = "paid-epoch".into();
    grant.value_metering.as_mut().unwrap().effective_from_ms = 3000;
    merge_account_quota_projections(&conn, &mut next).unwrap();
    next.user_grants
        .get_mut("buyer@example.com")
        .unwrap()
        .value_metering
        .as_mut()
        .unwrap()
        .value_metered_from_ms = 3000;
    assert!(matches!(
        merge_account_quota_projections(&conn, &mut next),
        Err(AppError::Conflict(message)) if message == "value metering origin is immutable"
    ));
}

#[tokio::test]
async fn share_value_market_metering_origin_rejects_unissued_scopes_and_same_rental_resets() {
    let (store, next) = market_metering_scope_fixture().await;
    let conn = store.conn.lock().await;
    for entitlement in [Some("old-entitlement"), Some("invented-entitlement"), None] {
        let mut invalid = next.clone();
        invalid
            .user_grants
            .get_mut("buyer@example.com")
            .unwrap()
            .entitlement_id = entitlement.map(str::to_owned);
        assert!(matches!(
            merge_account_quota_projections(&conn, &mut invalid),
            Err(AppError::Conflict(_))
        ));
    }
    let mut manual = next.clone();
    manual
        .user_grants
        .get_mut("buyer@example.com")
        .unwrap()
        .manager = crate::models::ShareGrantManager::Manual;
    assert!(matches!(
        merge_account_quota_projections(&conn, &mut manual),
        Err(AppError::Conflict(_))
    ));
    conn.execute(
        "UPDATE share_control_operations SET attempts=0 WHERE id='new-upsert'",
        [],
    )
    .unwrap();
    assert!(matches!(
        merge_account_quota_projections(&conn, &mut next.clone()),
        Err(AppError::Conflict(_))
    ));
}
