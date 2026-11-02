#[tokio::test]
async fn activation_receipts_use_signed_scoped_replay_protection_not_merely_an_ack() {
    use crate::store::share_value_quotas::{PublishPricing, SignedValueRequest};
    let (store, _config) = setup_store("signed-tariff-delivery").await;
    let key = insert_signed_installation(&store, "delivery-server").await;
    let other = insert_signed_installation(&store, "delivery-other").await;
    insert_share(
        &store,
        "delivery-server",
        "delivery-share",
        "delivery-sub",
        "active",
    )
    .await;
    let revision = store
        .publish_value_pricing(
            PublishPricing {
                expected_version: 0,
                effective_from_ms: None,
                snapshot: share_value_test_snapshot(),
            },
            "owner@example.test",
        )
        .await
        .unwrap()
        .revision
        .unwrap();
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE share_value_pricing_snapshots SET effective_from_ms=1",
            [],
        )
        .unwrap();
    let sign = |installation: &str, signer: &SigningKey, action: &str, nonce: &str| {
        let payload = serde_json::json!({"revision":revision,"storageSchema":3});
        let timestamp_ms = Utc::now().timestamp_millis();
        SignedValueRequest {
            protocol_epoch: PROTOCOL_EPOCH.into(),
            installation_id: installation.into(),
            timestamp_ms,
            nonce: nonce.into(),
            signature: sign_test_payload(
                signer,
                installation,
                action,
                &payload,
                timestamp_ms,
                nonce,
            ),
            payload,
        }
    };
    assert!(
        store
            .signed_value_operation(
                "share_value_pricing_active",
                sign(
                    "delivery-server",
                    &other,
                    "share_value_pricing_active",
                    "wrong-key"
                )
            )
            .await
            .is_err()
    );
    let receipt = sign(
        "delivery-server",
        &key,
        "share_value_pricing_active",
        "activate",
    );
    assert!(
        store
            .signed_value_operation("share_value_pricing_active", receipt.clone())
            .await
            .is_err()
    );
    store
        .signed_value_operation(
            "share_value_pricing_ack",
            sign("delivery-server", &key, "share_value_pricing_ack", "ack"),
        )
        .await
        .unwrap();
    assert_eq!(
        store.value_quota_health().await.unwrap()["delivery"]["activatedInstallations"],
        0,
        "ACK is not activation"
    );
    assert!(
        store
            .signed_value_operation(
                "share_value_pricing_active",
                sign(
                    "delivery-other",
                    &other,
                    "share_value_pricing_active",
                    "wrong-install"
                )
            )
            .await
            .is_err()
    );
    store
        .signed_value_operation("share_value_pricing_active", receipt.clone())
        .await
        .unwrap();
    assert!(
        store
            .signed_value_operation("share_value_pricing_active", receipt)
            .await
            .is_err(),
        "a receipt nonce cannot be replayed"
    );
    assert_eq!(
        store.value_quota_health().await.unwrap()["delivery"]["activatedInstallations"],
        1
    );
    assert_eq!(
        store
            .value_pricing_readiness("delivery-server")
            .await
            .unwrap()["activation"]["current"],
        true
    );
}

#[tokio::test]
async fn republishing_compacted_content_restores_payload_without_rewriting_its_initial_effective_time()
 {
    let (store, _config) = setup_store("republish-compacted-tariff").await;
    let revision = store
        .sync_value_tariff_from_catalog()
        .await
        .unwrap()
        .unwrap();
    let original: (String, i64) = {
        let conn = store.conn.lock().await;
        let original=conn.query_row("SELECT payload_json,effective_from_ms FROM share_value_pricing_snapshots WHERE revision=?1",params![revision],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        conn.execute(
            "UPDATE share_value_pricing_snapshots SET payload_json='{}' WHERE revision=?1",
            params![revision],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO share_value_pricing_snapshots VALUES('temporary','{}',1,'fixture',1)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE share_value_pricing_config SET revision='temporary',version=version+1",
            [],
        )
        .unwrap();
        original
    };
    assert_eq!(
        store.sync_value_tariff_from_catalog().await.unwrap(),
        Some(revision.clone())
    );
    let conn = store.conn.lock().await;
    let restored:(String,i64)=conn.query_row("SELECT payload_json,effective_from_ms FROM share_value_pricing_snapshots WHERE revision=?1",params![revision],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(restored, original);
}
