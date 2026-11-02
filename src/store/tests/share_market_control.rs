// End-to-end storage tests using the existing signed installation and namespace
// fixtures, without a browser or real Server.
use super::*;
async fn managed_control_fixture(usd: bool) -> (AppStore, SigningKey, ShareDescriptor) {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let signer = insert_signed_installation(&store, "managed-install").await;
    insert_client_tunnel(
        &store,
        "managed-install",
        "owner@example.com",
        TEST_CLIENT_SUBDOMAIN,
    )
    .await;
    let subdomain = test_share_host("managed");
    insert_share(
        &store,
        "managed-install",
        "managed-share",
        &subdomain,
        "active",
    )
    .await;
    let now = Utc::now();
    let mut returned = test_share_descriptor("managed-share", &subdomain);
    returned.config_revision = 5;
    returned.descriptor_generation = 1;
    returned.descriptor_fingerprint = "a".repeat(64);
    returned
        .supported_user_token_periods
        .push(ShareTokenPeriod::ThirtyDays);
    let pricing = share_value_test_snapshot();
    let pricing_revision = pricing.revision().unwrap();
    let policy: ShareUserPolicy = serde_json::from_value(serde_json::json!({
        "parallelLimit":1,"tokenPeriod":"thirtyDays","allowedApps":["claude","codex","gemini"],
        "tokenLimit": if usd { None } else { Some(1_000) },
        "valueQuota": if usd { Some(serde_json::json!({"limitUsdMicros":"100000000","tariffPolicyId":"api-equivalent-v1"})) } else { None }
    })).unwrap();
    let patch: ShareSettingsPatch = serde_json::from_value(serde_json::json!({
        "managedGrant":{"operationId":"managed-operation","entitlementId":"managed-entitlement",
            "shareSequence":1,"expectedConfigRevision":5,"action":"upsert","email":"buyer@example.com","policy":policy}
    })).unwrap();
    {
        let conn = store.conn.lock().await;
        // Seed the same current contract, bound App, and fresh runtime that
        // the real checkout requires before dispatching a managed grant.
        upsert_share_tx(&conn, "managed-install", returned.clone()).unwrap();
        conn.execute(
            "UPDATE shares SET runtime_refreshed_at=?1 WHERE share_id='managed-share'",
            params![now.to_rfc3339()],
        )
        .unwrap();
        if usd {
            conn.execute(
                "INSERT INTO share_value_pricing_snapshots VALUES(?1,?2,0,'fixture',0)",
                params![pricing_revision, serde_json::to_string(&pricing).unwrap()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO share_value_pricing_acks VALUES('managed-install',?1,?2)",
                params![pricing_revision, now.timestamp_millis()],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO share_market_listings(id,share_id,installation_id,owner_user_id,owner_email,status,created_at,updated_at)
            VALUES('managed-listing','managed-share','managed-install','provider','owner@example.com','active',?1,?1)", params![now.to_rfc3339()]).unwrap();
        conn.execute("INSERT INTO share_market_seats(id,listing_id,position,status,parallel_limit,token_limit,value_limit_usd_micros,token_period_json,current_subscription_id,created_at,updated_at)
            VALUES('managed-seat','managed-listing',1,'reserved',1,?1,?2,'\"thirtyDays\"','managed-sub',?3,?3)", params![policy.token_limit,policy.value_quota.as_ref().map(|q|&q.limit_usd_micros),now.to_rfc3339()]).unwrap();
        conn.execute("INSERT INTO share_market_subscriptions(id,seat_id,listing_id,share_id,installation_id,entitlement_id,owner_user_id,owner_email,renter_user_id,renter_email,status,parallel_limit,token_limit,value_limit_usd_micros,token_period_json,offer_revision,created_at,updated_at,required_app)
            VALUES('managed-sub','managed-seat','managed-listing','managed-share','managed-install','managed-entitlement','provider','owner@example.com','buyer','buyer@example.com','grant_pending',1,?1,?2,'\"thirtyDays\"',1,?3,?3,'codex')", params![policy.token_limit,policy.value_quota.as_ref().map(|q|&q.limit_usd_micros),(now-Duration::seconds(1)).to_rfc3339()]).unwrap();
        let snapshot = serde_json::json!({
            "schemaVersion":2,"requiredApp":"codex","supportedApps":["codex"],
            "shareTokensUsed":0,"apps":[{
                "app":"codex","providerFamily":"other","modelMode":"unknown","models":[],
                "providerBindingFingerprint":crate::api::sha256_hex(b"share-market-provider-binding-v1\ncodex\nprovider-test")
            }]
        });
        conn.execute(
            "UPDATE share_market_subscriptions SET contract_apps_json='[\"codex\"]',service_snapshot_json=?1 WHERE id='managed-sub'",
            params![snapshot.to_string()],
        ).unwrap();
        conn.execute("INSERT INTO share_edit_requests(id,share_id,installation_id,owner_email,revision,status,patch_json,created_by_email,created_at,updated_at,expires_at)
            VALUES('managed-edit','managed-share','managed-install','owner@example.com',1,'pending',?1,?2,?3,?3,?4)", params![serde_json::to_string(&patch).unwrap(),crate::share_market::SHARE_MARKET_CONTROL_ACTOR_EMAIL,now.to_rfc3339(),(now+Duration::minutes(5)).to_rfc3339()]).unwrap();
        conn.execute("INSERT INTO share_control_operations(id,share_id,share_sequence,entitlement_id,subscription_id,action,email,policy_json,status,edit_id,attempts,created_at,updated_at,last_attempt_edit_id,last_attempt_config_revision)
            VALUES('managed-operation','managed-share',1,'managed-entitlement','managed-sub','upsert','buyer@example.com',?1,'dispatched','managed-edit',1,?2,?2,'managed-edit',5)", params![serde_json::to_string(&policy).unwrap(),now.to_rfc3339()]).unwrap();
    }
    let mut effective = policy;
    effective.token_period_anchor_at_ms = Some(now.timestamp_millis().div_euclid(60_000) * 60_000);
    returned.config_revision = 6;
    returned.descriptor_generation = 2;
    returned.descriptor_fingerprint = "b".repeat(64);
    returned.user_grants.insert(
        "buyer@example.com".into(),
        ShareUserGrant {
            email: "buyer@example.com".into(),
            role: "shareto".into(),
            active: true,
            policy: effective,
            manager: crate::models::ShareGrantManager::RouterShareMarket,
            entitlement_id: Some("managed-entitlement".into()),
            created_at_ms: now.timestamp_millis() as u128,
            updated_at_ms: now.timestamp_millis() as u128,
            revision: 1,
            value_metering: usd.then(|| crate::share_value_pricing::ValueMeteringBinding {
                quota_policy_epoch: "managed-epoch".into(),
                value_metered_from_ms: now.timestamp_millis(),
                effective_from_ms: now.timestamp_millis(),
                pricing_revision,
            }),
            ..ShareUserGrant::default()
        },
    );
    (store, signer, returned)
}

async fn signed_managed_ack(
    store: &AppStore,
    signer: &SigningKey,
    ack: serde_json::Value,
) -> Result<(), AppError> {
    let timestamp = Utc::now().timestamp_millis();
    let nonce = Uuid::new_v4().to_string();
    let ack_json = ack.to_string();
    let payload = serde_json::json!({"ack":ack}).to_string();
    let signature = sign_test_payload_json(
        signer,
        "managed-install",
        "share_edit_ack",
        &payload,
        timestamp,
        &nonce,
    );
    store
        .ack_share_edit(
            share_edit_ack_request_from_wire(
                "managed-install",
                timestamp,
                &nonce,
                &signature,
                &ack_json,
            ),
            ClientMetadata {
                ip: None,
                country_code: None,
            },
        )
        .await
}

#[tokio::test]
async fn managed_control_direct_reply_accepts_materialized_token_and_usd_anchors_idempotently() {
    for usd in [false, true] {
        let (store, _, returned) = managed_control_fixture(usd).await;
        let anchor = returned.user_grants["buyer@example.com"]
            .policy
            .token_period_anchor_at_ms;
        let binding = returned.user_grants["buyer@example.com"]
            .value_metering
            .clone();
        store
            .apply_share_edit_with_receipt("managed-edit", returned.clone(), None)
            .await
            .unwrap();
        store
            .apply_share_edit_with_receipt("managed-edit", returned, None)
            .await
            .unwrap();
        let conn = store.conn.lock().await;
        assert_eq!(
            conn.query_row(
                "SELECT status FROM share_market_subscriptions WHERE id='managed-sub'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "active_free",
            "subscription: {:?}", conn.query_row("SELECT release_reason,integrity_reason FROM share_market_subscriptions WHERE id='managed-sub'", [], |r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?))).unwrap()
        );
        let grants: String = conn
            .query_row(
                "SELECT user_grants_json FROM shares WHERE share_id='managed-share'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let grants: BTreeMap<String, ShareUserGrant> = serde_json::from_str(&grants).unwrap();
        assert_eq!(
            grants["buyer@example.com"].policy.token_period_anchor_at_ms,
            anchor
        );
        assert_eq!(grants["buyer@example.com"].value_metering, binding);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM share_market_events WHERE subscription_id='managed-sub' AND event_type='entitlement_activated'", [], |r|r.get::<_,i64>(0)).unwrap(), 1);
    }
}

#[tokio::test]
async fn managed_control_invalid_direct_reply_queues_cleanup_even_with_an_unrelated_settings_receipt()
 {
    let (store, _, mut returned) = managed_control_fixture(true).await;
    returned
        .user_grants
        .get_mut("buyer@example.com")
        .unwrap()
        .policy
        .value_quota
        .as_mut()
        .unwrap()
        .limit_usd_micros = "200000000".into();
    let receipt = crate::models::ShareSettingsReceipt {
        operation_id: "unrelated".into(),
        patch_fingerprint: "unrelated".into(),
        config_revision: 1,
    };
    let error = store
        .apply_share_edit_with_receipt("managed-edit", returned, Some(receipt))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("managedGrant"), "{error}");
    let conn = store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT status FROM share_market_subscriptions WHERE id='managed-sub'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "revoke_pending"
    );
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM share_control_operations WHERE subscription_id='managed-sub' AND action='revoke' AND status='pending'", [], |r|r.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(
        conn.query_row(
            "SELECT config_revision FROM shares WHERE share_id='managed-share'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        5
    );
}

#[tokio::test]
async fn managed_control_signed_ack_requires_execution_evidence_and_does_not_restart_the_period() {
    let (store, signer, returned) = managed_control_fixture(true).await;
    let grant = &returned.user_grants["buyer@example.com"];
    let mut ack = serde_json::json!({"editId":"managed-edit","revision":1,"status":"applied"});
    assert!(
        signed_managed_ack(&store, &signer, ack.clone())
            .await
            .is_err()
    );
    assert!(
        store
            .is_share_edit_pending("managed-edit", 1)
            .await
            .unwrap()
    );
    ack["appliedAtMs"] = serde_json::json!(grant.updated_at_ms);
    ack["effectivePolicy"] = serde_json::to_value(&grant.policy).unwrap();
    signed_managed_ack(&store, &signer, ack.clone())
        .await
        .unwrap();
    signed_managed_ack(&store, &signer, ack).await.unwrap();
    let conn = store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT status FROM share_market_subscriptions WHERE id='managed-sub'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "active_free",
        "subscription: {:?}", conn.query_row("SELECT release_reason,integrity_reason FROM share_market_subscriptions WHERE id='managed-sub'", [], |r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?))).unwrap()
    );
    let grants: String = conn
        .query_row(
            "SELECT user_grants_json FROM shares WHERE share_id='managed-share'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let grants: BTreeMap<String, ShareUserGrant> = serde_json::from_str(&grants).unwrap();
    assert_eq!(
        grants["buyer@example.com"].updated_at_ms,
        grant.updated_at_ms
    );
    assert_eq!(grants["buyer@example.com"].policy, grant.policy);
}

#[tokio::test]
async fn managed_control_signed_ack_accepts_matching_descriptor_execution_evidence() {
    let (store, signer, returned) = managed_control_fixture(true).await;
    let expected = returned.user_grants["buyer@example.com"].clone();
    signed_managed_ack(
        &store,
        &signer,
        serde_json::json!({
            "editId":"managed-edit","revision":1,"status":"applied","currentShare":returned,
        }),
    )
    .await
    .unwrap();
    let conn = store.conn.lock().await;
    let grants: String = conn
        .query_row(
            "SELECT user_grants_json FROM shares WHERE share_id='managed-share'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let grants: BTreeMap<String, ShareUserGrant> = serde_json::from_str(&grants).unwrap();
    assert_eq!(grants["buyer@example.com"].policy, expected.policy);
    assert_eq!(
        grants["buyer@example.com"].updated_at_ms,
        expected.updated_at_ms
    );
    assert_eq!(
        conn.query_row(
            "SELECT status FROM share_market_subscriptions WHERE id='managed-sub'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
        "active_free"
    );
}

#[tokio::test]
async fn managed_control_signed_ack_quarantines_tampered_execution_policy() {
    let (store, signer, returned) = managed_control_fixture(true).await;
    let mut grant = returned.user_grants["buyer@example.com"].clone();
    grant.policy.value_quota.as_mut().unwrap().limit_usd_micros = "200000000".into();
    signed_managed_ack(
        &store,
        &signer,
        serde_json::json!({
            "editId":"managed-edit","revision":1,"status":"applied",
            "appliedAtMs":grant.updated_at_ms,"effectivePolicy":grant.policy,
        }),
    )
    .await
    .unwrap();
    let conn = store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT status FROM share_market_subscriptions WHERE id='managed-sub'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
        "revoke_pending"
    );
    // Preserve the signed fact that Server applied it, but record the
    // contractual failure. Router must not confuse that receipt with a
    // successful rental delivery or keep retrying the invalid upsert.
    assert_eq!(
        conn.query_row(
            "SELECT status,error_code FROM share_edit_requests WHERE id='managed-edit'",
            [],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .unwrap(),
        (
            "applied".into(),
            "share_market_grant_contract_violation".into()
        )
    );
    assert_eq!(conn.query_row(
        "SELECT status,dead_lettered_at IS NOT NULL FROM share_control_operations WHERE id='managed-operation'",
        [], |r| Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?)),
    ).unwrap(), ("rejected".into(), true));
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM share_market_events WHERE event_type='entitlement_activated'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    assert_eq!(conn.query_row(
        "SELECT COUNT(*) FROM share_control_operations WHERE action='revoke' AND status='pending'",
        [], |r| r.get::<_,i64>(0),
    ).unwrap(), 1);
}

#[tokio::test]
async fn managed_control_validated_descriptor_wakes_revocation_but_stale_or_repeated_sync_does_not()
{
    let (store, _, returned) = managed_control_fixture(false).await;
    let now = Utc::now();
    {
        let conn = store.conn.lock().await;
        crate::share_market::request_termination_revoke_tx(&conn, "managed-sub", &now.to_rfc3339())
            .unwrap();
    }
    store
        .share_market_reconcile_and_dispatch(now)
        .await
        .unwrap();
    {
        let conn = store.conn.lock().await;
        let edit: String = conn.query_row("SELECT edit_id FROM share_control_operations WHERE subscription_id='managed-sub' AND action='revoke'", [], |r|r.get(0)).unwrap();
        conn.execute("UPDATE share_edit_requests SET status='rejected',error_code='control_ack_timeout' WHERE id=?1", params![edit]).unwrap();
        crate::share_market::handle_control_edit_ack_with_metadata(
            &conn,
            &edit,
            "rejected",
            Some("timeout"),
            Some("control_ack_timeout"),
            Some(true),
            &now.to_rfc3339(),
        )
        .unwrap();
        conn.execute(
            "UPDATE share_control_operations SET next_attempt_at=?1 WHERE action='revoke'",
            params![(now + Duration::minutes(15)).to_rfc3339()],
        )
        .unwrap();
        let mut newer = returned.clone();
        newer.config_revision = 7;
        newer.descriptor_generation += 1;
        newer.descriptor_fingerprint = "c".repeat(64);
        upsert_share_tx(&conn, "managed-install", newer.clone()).unwrap();
        let accelerated: String = conn
            .query_row(
                "SELECT next_attempt_at FROM share_control_operations WHERE action='revoke'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            DateTime::parse_from_rfc3339(&accelerated)
                .unwrap()
                .with_timezone(&Utc)
                < now + Duration::minutes(1)
        );
        let later = (now + Duration::minutes(10)).to_rfc3339();
        conn.execute(
            "UPDATE share_control_operations SET next_attempt_at=?1 WHERE action='revoke'",
            params![later],
        )
        .unwrap();
        upsert_share_tx(&conn, "managed-install", newer).unwrap();
        upsert_share_tx(&conn, "managed-install", returned).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT next_attempt_at FROM share_control_operations WHERE action='revoke'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            later
        );
    }
}
