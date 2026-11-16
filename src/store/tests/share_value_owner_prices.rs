// Share owner model prices: CRUD/CAS, catalog caps, per-installation tariff
// overlays and the owner-priced usage breakdown. Filter: `share_value`.
use super::*;
use crate::store::share_value_quotas::owner_prices::{OwnerPriceEdit, OwnerRates};

fn rates(input: i64, output: i64) -> OwnerRates {
    OwnerRates {
        input,
        output,
        cache_read: 0,
        cache_write_5m: 0,
        cache_write_1h: None,
    }
}
fn edit(key: &str, rates: OwnerRates, expected_version: Option<i64>) -> OwnerPriceEdit {
    OwnerPriceEdit {
        model_key: key.into(),
        display_name: None,
        rates,
        expected_version,
    }
}
fn coded(error: AppError) -> (&'static str, serde_json::Value) {
    match error {
        AppError::Coded { code, details, .. } => (code, details),
        other => panic!("expected a coded error, got {other:?}"),
    }
}
async fn priced_store(name: &str) -> (AppStore, Config) {
    let (store, config) = setup_store(name).await;
    store.reload_pricing_catalog().await.unwrap();
    insert_installation(&store, "inst-a").await;
    insert_installation(&store, "inst-b").await;
    insert_installation(&store, "inst-c").await;
    insert_share(&store, "inst-a", "share-a", "owner-price-a", "active").await;
    insert_share(&store, "inst-b", "share-b", "owner-price-b", "active").await;
    insert_share(&store, "inst-c", "share-c", "owner-price-c", "active").await;
    // The global tariff exists before any overlay is derived.
    store.sync_value_tariff_from_catalog().await.unwrap();
    (store, config)
}
async fn served(store: &AppStore, installation: &str) -> Option<String> {
    let conn = store.conn.lock().await;
    crate::store::share_value_quotas::owner_prices::installation_tariff_revision(
        &conn,
        installation,
        false,
    )
    .unwrap()
}
async fn global_revision(store: &AppStore) -> String {
    crate::store::share_value_quotas::tests_settings_meta(&*store.conn.lock().await)
        .1
        .unwrap()
}

#[tokio::test]
async fn share_value_owner_prices_crud_uses_cas_and_versions_never_restart() {
    let (store, config) = priced_store("owner-price-crud").await;
    let (installation, prices) = store.owner_model_prices("share-a").await.unwrap();
    assert_eq!((installation.as_str(), prices.len()), ("inst-a", 0));

    let (created, effective) = store
        .upsert_owner_model_price(
            "share-a",
            edit("Kimi-K3-1", rates(1_000_000, 2_000_000), None),
            "owner@example.com",
        )
        .await
        .unwrap();
    assert_eq!(created.model_key, "kimi-k3-1");
    assert_eq!(created.display_name, "kimi-k3-1");
    assert_eq!(created.version, 1);
    assert_eq!(created.rates.input_usd_per1m, "1");
    assert!(effective >= Utc::now().timestamp_millis() + 50_000);

    // Absent expectedVersion means "must not exist"; a stale number conflicts.
    for expected in [None, Some(7)] {
        let (code, details) = coded(
            store
                .upsert_owner_model_price("share-a", edit("kimi-k3-1", rates(1, 1), expected), "x")
                .await
                .unwrap_err(),
        );
        assert_eq!(code, "MODEL_PRICE_VERSION_CONFLICT");
        assert_eq!(details["currentVersion"], 1);
    }
    let (updated, _) = store
        .upsert_owner_model_price(
            "share-a",
            edit("kimi-k3-1", rates(1_500_000, 2_000_000), Some(1)),
            "owner@example.com",
        )
        .await
        .unwrap();
    assert_eq!(
        (updated.version, updated.rates.input_usd_per1m.as_str()),
        (2, "1.5")
    );

    // Every Share on the installation sees the same prices.
    assert!(matches!(
        store
            .delete_owner_model_price("share-a", "kimi-k3-1", 1, "x")
            .await,
        Err(AppError::Coded {
            code: "MODEL_PRICE_VERSION_CONFLICT",
            ..
        })
    ));
    store
        .delete_owner_model_price("share-a", "kimi-k3-1", 2, "owner@example.com")
        .await
        .unwrap();
    assert!(
        store
            .owner_model_prices("share-a")
            .await
            .unwrap()
            .1
            .is_empty()
    );
    let (recreated, _) = store
        .upsert_owner_model_price(
            "share-a",
            edit("kimi-k3-1", rates(1, 1), None),
            "owner@example.com",
        )
        .await
        .unwrap();
    // upsert v1, upsert v2, delete v3: a re-created price continues at v4.
    assert_eq!(recreated.version, 4);
    assert!(matches!(
        store.owner_model_prices("missing-share").await,
        Err(AppError::Coded {
            code: "SHARE_NOT_FOUND",
            ..
        })
    ));
    let _ = std::fs::remove_file(&config.database.path);
}

#[tokio::test]
async fn share_value_owner_prices_are_capped_by_the_catalog_and_bounded_in_count() {
    let (store, config) = priced_store("owner-price-caps").await;
    let max_input = store
        .pricing_catalog()
        .effective_models(Utc::now().timestamp())
        .into_iter()
        .flat_map(|(_, price)| price.rates.values().map(|set| set.input_micros_per_1m))
        .max()
        .unwrap();
    for (rates, reason) in [
        (rates(max_input + 1, 1), "rate_above_catalog_max"),
        (rates(0, 0), "rate_required"),
        (
            OwnerRates {
                cache_write_5m: 2,
                cache_write_1h: Some(1),
                ..rates(1, 1)
            },
            "cache_write_1h_below_5m",
        ),
    ] {
        let (code, details) = coded(
            store
                .upsert_owner_model_price("share-a", edit("custom-model", rates, None), "x")
                .await
                .unwrap_err(),
        );
        assert_eq!(
            (code, details["reason"].as_str()),
            ("MODEL_PRICE_INVALID", Some(reason))
        );
    }
    store
        .upsert_owner_model_price(
            "share-a",
            edit("custom-model", rates(max_input, 0), None),
            "x",
        )
        .await
        .unwrap();
    {
        let conn = store.conn.lock().await;
        for index in 1..crate::store::share_value_quotas::owner_prices::MAX_OWNER_PRICES {
            conn.execute(
                "INSERT INTO share_value_owner_model_prices VALUES('inst-a',?1,?1,1,1,0,0,NULL,1,0,0,'x')",
                params![format!("filler-{index}")],
            )
            .unwrap();
        }
    }
    let (code, details) = coded(
        store
            .upsert_owner_model_price("share-a", edit("one-too-many", rates(1, 1), None), "x")
            .await
            .unwrap_err(),
    );
    assert_eq!(
        (code, details["reason"].as_str()),
        ("MODEL_PRICE_INVALID", Some("too_many_prices"))
    );
    let _ = std::fs::remove_file(&config.database.path);
}

#[tokio::test]
async fn share_value_owner_overlays_are_deterministic_and_leave_other_installations_global() {
    let (store, config) = priced_store("owner-price-overlay").await;
    let global = global_revision(&store).await;
    for installation in ["inst-a", "inst-b", "inst-c"] {
        assert_eq!(
            served(&store, installation).await.as_deref(),
            Some(global.as_str())
        );
    }
    for share in ["share-a", "share-b"] {
        store
            .upsert_owner_model_price(
                share,
                edit("owner-only-model", rates(1_000_000, 4_000_000), None),
                "x",
            )
            .await
            .unwrap();
    }
    let overlay_a = served(&store, "inst-a").await.unwrap();
    // Same prices, same content, same revision; untouched installations keep
    // receiving the global revision unchanged.
    assert_ne!(overlay_a, global);
    assert_eq!(served(&store, "inst-b").await.unwrap(), overlay_a);
    assert_eq!(
        served(&store, "inst-c").await.as_deref(),
        Some(global.as_str())
    );
    {
        let conn = store.conn.lock().await;
        let json: String = conn
            .query_row(
                "SELECT payload_json FROM share_value_pricing_snapshots WHERE revision=?1",
                params![overlay_a],
                |r| r.get(0),
            )
            .unwrap();
        let snapshot: crate::share_value_pricing::PricingSnapshot =
            serde_json::from_str(&json).unwrap();
        assert_eq!(snapshot.revision().unwrap(), overlay_a);
        let model = &snapshot.models["owner-only-model"];
        for tier in ["standard/base", "priority/base", "flex/base"] {
            assert_eq!(model.rates[tier].input.as_deref(), Some("1000000"));
        }
    }
    // A global re-derivation with unchanged inputs is a no-op for overlays.
    store.sync_value_tariff_from_catalog().await.unwrap();
    assert_eq!(served(&store, "inst-a").await.unwrap(), overlay_a);
    let readiness = store.value_pricing_readiness("inst-a").await.unwrap();
    assert_eq!(readiness["publishedRevision"], overlay_a.as_str());

    // Removing the last price returns to the global revision with lead time.
    let version = store.owner_model_prices("share-a").await.unwrap().1[0].version;
    let back = store
        .delete_owner_model_price("share-a", "owner-only-model", version, "x")
        .await
        .unwrap();
    assert!(back >= Utc::now().timestamp_millis() + 50_000);
    assert_eq!(
        served(&store, "inst-a").await.as_deref(),
        Some(global.as_str())
    );
    let readiness = store.value_pricing_readiness("inst-a").await.unwrap();
    assert_eq!(readiness["effectiveFromMs"], back);
    assert_eq!(served(&store, "inst-b").await.unwrap(), overlay_a);
    let _ = std::fs::remove_file(&config.database.path);
}

#[tokio::test]
async fn share_value_owner_overlay_is_acknowledged_only_by_its_installation() {
    use crate::store::share_value_quotas::SignedValueRequest;
    let (store, config) = setup_store("owner-price-ack").await;
    store.reload_pricing_catalog().await.unwrap();
    let owner = insert_signed_installation(&store, "ack-owner").await;
    let other = insert_signed_installation(&store, "ack-other").await;
    insert_share(
        &store,
        "ack-owner",
        "ack-share",
        "owner-price-ack",
        "active",
    )
    .await;
    store.sync_value_tariff_from_catalog().await.unwrap();
    store
        .upsert_owner_model_price(
            "ack-share",
            edit("owner-only-model", rates(1, 1), None),
            "x",
        )
        .await
        .unwrap();
    let overlay = served(&store, "ack-owner").await.unwrap();
    let ack = |installation: &str, signer: &SigningKey, nonce: &str| {
        let payload = serde_json::json!({"revision":overlay,"storageSchema":3});
        let timestamp_ms = Utc::now().timestamp_millis();
        SignedValueRequest {
            protocol_epoch: PROTOCOL_EPOCH.into(),
            installation_id: installation.into(),
            timestamp_ms,
            nonce: nonce.into(),
            signature: sign_test_payload(
                signer,
                installation,
                "share_value_pricing_ack",
                &payload,
                timestamp_ms,
                nonce,
            ),
            payload,
        }
    };
    assert!(matches!(
        store
            .signed_value_operation("share_value_pricing_ack", ack("ack-other", &other, "n1"))
            .await,
        Err(AppError::NotFound(_))
    ));
    store
        .signed_value_operation("share_value_pricing_ack", ack("ack-owner", &owner, "n2"))
        .await
        .unwrap();
    let _ = std::fs::remove_file(&config.database.path);
}

#[tokio::test]
async fn share_value_breakdown_prices_owner_models_and_flags_unpriced_usage() {
    let (store, config) = priced_store("owner-price-breakdown").await;
    let now = Utc::now().timestamp();
    {
        let conn = store.conn.lock().await;
        set_share_grants(
            &conn,
            "share-a",
            BTreeMap::from([(
                "user@example.com".into(),
                lifetime_grant("user@example.com", "shareto", 1_000_000),
            )]),
        );
        for (id, model) in [
            ("owner-priced", "owner-only-model"),
            ("catalog-priced", "gpt-5"),
            ("unpriced", "nobody-prices-this"),
        ] {
            let mut log = test_share_request_log_entry(id, "share-a", now);
            log.user_email = Some("user@example.com".into());
            log.model = model.into();
            log.actual_model = model.into();
            log.input_tokens = 1_000;
            log.output_tokens = 500;
            upsert_share_request_log_tx(&conn, "inst-a", log).unwrap();
        }
    }
    store
        .upsert_owner_model_price(
            "share-a",
            edit("owner-only-model", rates(2_000_000, 4_000_000), None),
            "x",
        )
        .await
        .unwrap();
    // Owner prices take effect after the publication lead time; backdate it.
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE share_value_owner_model_price_history SET effective_from_ms=0",
            [],
        )
        .unwrap();
    let breakdown = store
        .share_user_usage_breakdown("share-a", Some("user@example.com"))
        .await
        .unwrap();
    assert!(!breakdown.can_set_price && !breakdown.can_set_global_price);
    let row = &breakdown.rows[0];
    let model = |key: &str| row.by_model.iter().find(|m| m.model_key == key).unwrap();
    let owner = model("owner-only-model");
    assert!(owner.priced && !owner.unpriced);
    assert_eq!(owner.price_source, Some("owner"));
    // 1000 × $2/M + 500 × $4/M = $0.004.
    assert_eq!(owner.equivalent_usd_micros.as_deref(), Some("4000"));
    assert_eq!(model("gpt-5").price_source, Some("catalog"));
    let unpriced = model("nobody-prices-this");
    assert!(!unpriced.priced && unpriced.unpriced);
    assert_eq!(unpriced.price_source, None);
    assert!(unpriced.equivalent_usd_micros.is_none());
    assert_eq!(row.unpriced_tokens, 1_500);
    assert_eq!(row.unpriced_models, vec!["nobody-prices-this".to_string()]);
    // Another installation's Share does not see these owner prices.
    let json = serde_json::to_value(&breakdown).unwrap();
    assert_eq!(json["rows"][0]["unpricedTokens"], 1_500);
    assert_eq!(json["canSetPrice"], false);
    assert!(json["rows"][0]["byModel"][0].get("priceSource").is_some());
    let _ = std::fs::remove_file(&config.database.path);
}

#[test]
fn share_value_status_unpriced_fields_round_trip_without_changing_old_payloads() {
    use crate::share_value_pricing::ValueQuotaStatus;
    let old = serde_json::json!({
        "email":"user@example.test","policyEpoch":"e","windowId":"Lifetime:0:0",
        "windowStartsAtMs":0,"resetsAtMs":null,"meteredFromMs":0,
        "activePricingRevision":"r","usagePricingRevisionsDigest":"d","usageWatermark":1,
        "limitUsdMicros":"1","usedPicos":"0","observedPicos":"0","manualOffsetPicos":"0",
        "pendingRiskPicos":"0","meteringStatus":"observed","syncedAtMs":0});
    let status: ValueQuotaStatus = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(status.unpriced_requests, None);
    // Absent fields stay absent, so stored projections serialize as before.
    assert_eq!(serde_json::to_value(&status).unwrap(), old);
    assert!(!serde_json::to_string(&status).unwrap().contains("unpriced"));
    let mut new = old;
    new["unpricedRequests"] = 2.into();
    new["unpricedTokens"] = 300.into();
    new["unpricedModels"] = serde_json::json!(["a-model", "b-model"]);
    let status: ValueQuotaStatus = serde_json::from_value(new.clone()).unwrap();
    assert_eq!(status.unpriced_tokens, Some(300));
    assert_eq!(serde_json::to_value(&status).unwrap(), new);
    new["unpricedSomethingElse"] = 1.into();
    assert!(serde_json::from_value::<ValueQuotaStatus>(new).is_err());
}

#[test]
fn share_value_owner_price_suggestions_rank_similar_catalog_names() {
    use crate::model_pricing::{ContextTier, ModelPrice, RateSet, ServiceTier};
    let price = |key: &str| ModelPrice {
        price_key: key.into(),
        display_name: String::new(),
        long_context_threshold: None,
        long_context_inclusive: false,
        supports_cache_breakdown: true,
        rates: BTreeMap::from([(
            (ServiceTier::Standard, ContextTier::Base),
            RateSet {
                input_micros_per_1m: 1,
                output_micros_per_1m: 2,
                ..Default::default()
            },
        )]),
    };
    let catalog = crate::model_price_catalog::PricingCatalog::for_tests(
        [
            "kimi-k3.1-pro",
            "kimi-k3",
            "kimi-k2",
            "gpt-5",
            "claude-sonnet-4-5",
        ]
        .into_iter()
        .map(price)
        .collect(),
    );
    let keys: Vec<String> =
        crate::store::share_value_quotas::owner_prices::model_price_suggestions(
            &catalog,
            "kimi-k3-1",
            0,
        )
        .into_iter()
        .map(|s| s.model_key)
        .collect();
    assert_eq!(
        keys[..2],
        ["kimi-k3.1-pro".to_string(), "kimi-k3".to_string()]
    );
    assert!(!keys.contains(&"gpt-5".to_string()));
    assert!(keys.len() <= 5);
}

#[tokio::test]
async fn share_value_owner_prices_only_fill_models_the_catalog_cannot_price() {
    let (store, config) = priced_store("owner-price-catalog-wins").await;
    // The catalog prices `gpt-5` (also through its wire spellings).
    for key in ["gpt-5", "GPT-5"] {
        let (code, details) = coded(
            store
                .upsert_owner_model_price("share-a", edit(key, rates(1, 1), None), "x")
                .await
                .unwrap_err(),
        );
        assert_eq!(
            (code, details["reason"].as_str()),
            ("MODEL_PRICE_INVALID", Some("catalog_priced"))
        );
    }
    store
        .upsert_owner_model_price(
            "share-a",
            edit("house-model", rates(1_000_000, 2_000_000), None),
            "x",
        )
        .await
        .unwrap();
    let overlay = served(&store, "inst-a").await.unwrap();
    assert_ne!(overlay, global_revision(&store).await);
    assert!(!store.owner_model_prices("share-a").await.unwrap().1[0].shadowed);

    // The catalog later prices the model: its price wins, the owner price
    // turns inert (still listed, flagged) and the installation is served the
    // global revision again.
    store
        .upsert_admin_model_price(crate::model_price_catalog::AdminModelPriceInput {
            price_key: "house-model".into(),
            display_name: "House Model".into(),
            input: 3_000_000,
            output: 6_000_000,
            cache_read: 0,
            cache_write_5m: 0,
            cache_write_1h: None,
            expected_effective_from: None,
            expect_unpriced: true,
        })
        .await
        .unwrap();
    let global = global_revision(&store).await;
    assert_eq!(
        served(&store, "inst-a").await.as_deref(),
        Some(global.as_str())
    );
    let prices = store.owner_model_prices("share-a").await.unwrap().1;
    assert_eq!(
        (prices[0].model_key.as_str(), prices[0].shadowed),
        ("house-model", true)
    );
    let json = serde_json::to_value(&prices[0]).unwrap();
    assert_eq!(json["shadowed"], true);
    // Editing an inert price is refused; removing it still works.
    let (_, details) = coded(
        store
            .upsert_owner_model_price(
                "share-a",
                edit("house-model", rates(1, 1), Some(prices[0].version)),
                "x",
            )
            .await
            .unwrap_err(),
    );
    assert_eq!(details["reason"], "catalog_priced");
    store
        .delete_owner_model_price("share-a", "house-model", prices[0].version, "x")
        .await
        .unwrap();
    let _ = std::fs::remove_file(&config.database.path);
}

#[tokio::test]
async fn share_value_breakdown_resolves_models_like_the_served_overlay() {
    let (store, config) = priced_store("owner-price-resolution").await;
    store
        .upsert_owner_model_price("share-a", edit("owner-only-model", rates(1, 1), None), "x")
        .await
        .unwrap();
    let overlay_revision = served(&store, "inst-a").await.unwrap();
    let overlay: crate::share_value_pricing::PricingSnapshot = {
        let conn = store.conn.lock().await;
        let json: String = conn
            .query_row(
                "SELECT payload_json FROM share_value_pricing_snapshots WHERE revision=?1",
                params![overlay_revision],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&json).unwrap()
    };
    let effective = store.installation_effective_prices("inst-a").await.unwrap();
    for wire in [
        "owner-only-model",
        "Owner-Only-Model",
        "owner-only-model-thinking",
        "gpt-5",
        "claude-sonnet-4-5-thinking",
        "anthropic.claude-sonnet-4-5-20250514-v1:0",
        "nobody-prices-this",
    ] {
        let materialized = overlay.resolve(wire, |key| {
            overlay.models.contains_key(key).then(|| key.to_string())
        });
        let layered = effective.resolve(wire);
        assert_eq!(
            layered.as_ref().map(|(key, _)| key.clone()),
            materialized,
            "{wire}"
        );
        assert_eq!(
            layered.is_some_and(|(key, owner)| owner == (key == "owner-only-model")),
            materialized.is_some(),
            "{wire}"
        );
    }
    // Another installation never sees these owner prices.
    let other = store.installation_effective_prices("inst-c").await.unwrap();
    assert_eq!(other.resolve("owner-only-model"), None);
    let _ = std::fs::remove_file(&config.database.path);
}
