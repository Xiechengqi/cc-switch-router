use super::*;

const PRIVATE_ACCOUNT: &str = "private-upstream@example.test";
const PRIVATE_LABEL: &str = "account-private-327d03ae";
const PRIVATE_PROVIDER_LABEL: &str = "bound-provider-private-8899";

async fn fixture() -> (AppStore, Config) {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let config = test_config("share-account-identity");
    insert_installation(&store, "identity-installation").await;
    insert_client_tunnel(
        &store,
        "identity-installation",
        "owner@example.com",
        "identity-client",
    )
    .await;
    insert_share(
        &store,
        "identity-installation",
        "identity-share",
        "identity-route",
        "active",
    )
    .await;
    let runtime = serde_json::json!({
        "app": "codex", "kind": "codex_oauth", "providerType": "codex_oauth",
        "providerName": format!("OpenAI {PRIVATE_ACCOUNT} / {PRIVATE_PROVIDER_LABEL}"),
        "accountLabel": PRIVATE_LABEL, "accountEmail": PRIVATE_ACCOUNT,
        "quota": { "status": "ok", "credentialMessage": format!("Plus {PRIVATE_PROVIDER_LABEL}") },
        "modelPolicy": { "mode": "single", "upstreamModel": "model-kept" }
    });
    let mut runtimes = serde_json::Map::new();
    for app in [
        "claude",
        "codex",
        "gemini",
        "kiro",
        "cursor",
        "antigravity",
        "copilot",
    ] {
        runtimes.insert(app.into(), runtime.clone());
    }
    let mut providers = serde_json::Map::new();
    for app in ["claude", "codex", "gemini"] {
        providers.insert(app.into(), serde_json::json!([
            { "id": "provider-test", "app": app, "name": format!("Account {PRIVATE_PROVIDER_LABEL}"),
              "kind": "codex_oauth", "enabled": true, "accountLabel": PRIVATE_PROVIDER_LABEL,
              "accountEmail": PRIVATE_ACCOUNT },
            { "id": "unbound-provider", "app": app, "name": "Unbound",
              "accountEmail": "unbound-private@example.test" }
        ]));
    }
    let grants = BTreeMap::from([(
        "shared@example.com".to_string(),
        ShareUserGrant {
            email: "shared@example.com".into(),
            role: "shareto".into(),
            active: true,
            ..Default::default()
        },
    )]);
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE shares SET upstream_provider_json=?2, app_runtimes_json=?3,
         app_providers_json=?4, user_grants_json=?5 WHERE share_id=?1",
            params![
                "identity-share",
                runtime.to_string(),
                serde_json::Value::Object(runtimes).to_string(),
                serde_json::Value::Object(providers).to_string(),
                serde_json::to_string(&grants).unwrap()
            ],
        )
        .unwrap();
    (store, config)
}

fn assert_masked(value: &impl serde::Serialize) {
    let wire = serde_json::to_string(value).unwrap();
    for private in [
        PRIVATE_ACCOUNT,
        PRIVATE_LABEL,
        PRIVATE_PROVIDER_LABEL,
        "unbound-private@example.test",
    ] {
        assert!(!wire.contains(private), "private account leaked: {private}");
    }
    assert!(wire.contains("****"));
}

#[tokio::test]
async fn share_account_identity_browser_views_are_owner_only_and_never_rewrite_storage() {
    let (store, mut config) = fixture().await;
    config.admin_emails.insert("admin@example.com".into());
    let proxy = ProxyRegistry::default();
    let geo = ServerGeo {
        lat: None,
        lon: None,
    };
    for viewer in [
        None,
        Some("shared@example.com"),
        Some("admin@example.com"),
        Some("stranger@example.com"),
    ] {
        let dashboard = store
            .dashboard_snapshot(&config, &geo, &proxy, viewer)
            .await
            .unwrap();
        let share = dashboard
            .shares
            .iter()
            .find(|s| s.share_id == "identity-share")
            .unwrap();
        assert!(!share.can_manage);
        if viewer == Some("shared@example.com") {
            assert!(
                share.can_view_secret,
                "ShareTo access must not reveal the account"
            );
        }
        assert_masked(share);
        assert_eq!(share.app_runtimes.codex.as_ref().unwrap().models.len(), 0);
        assert_eq!(
            share
                .app_runtimes
                .codex
                .as_ref()
                .unwrap()
                .account_email
                .as_deref(),
            Some("p***m@example.test")
        );
        let standalone = store
            .share_view_for_share_url("identity-share", &HashSet::new(), &HashMap::new(), viewer)
            .await
            .unwrap();
        assert_masked(&standalone);
    }
    let owner = store
        .dashboard_snapshot(&config, &geo, &proxy, Some("Owner@Example.com"))
        .await
        .unwrap();
    assert!(owner.shares[0].can_manage);
    assert_eq!(
        owner.shares[0]
            .app_runtimes
            .codex
            .as_ref()
            .unwrap()
            .account_email
            .as_deref(),
        Some(PRIVATE_ACCOUNT)
    );
    let standalone = store
        .share_view_for_share_url(
            "identity-share",
            &HashSet::new(),
            &HashMap::new(),
            Some("owner@example.com"),
        )
        .await
        .unwrap();
    assert_eq!(
        standalone.app_providers.codex[0].account_label.as_deref(),
        Some(PRIVATE_PROVIDER_LABEL)
    );
    let conn = store.conn.lock().await;
    let raw: (String, String, String) = conn.query_row(
        "SELECT upstream_provider_json, app_runtimes_json, app_providers_json FROM shares WHERE share_id='identity-share'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    for stored in [&raw.0, &raw.1, &raw.2] {
        assert!(stored.contains(PRIVATE_ACCOUNT));
    }
    assert!(raw.0.contains(PRIVATE_LABEL));
    assert!(raw.1.contains(PRIVATE_LABEL));
    assert!(raw.2.contains(PRIVATE_PROVIDER_LABEL));
}

#[tokio::test]
async fn model_routing_share_account_identity_masks_shared_free_and_transferred_owners() {
    let (store, config) = fixture().await;
    let shared = store
        .get_user_model_routing(&config, "shared@example.com", &HashSet::new())
        .await
        .unwrap();
    assert_eq!(shared.eligible_shares.len(), 1);
    assert_eq!(shared.eligible_shares[0].access, "shared");
    assert_masked(&shared.eligible_shares[0]);
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE shares SET free_access=1 WHERE share_id='identity-share'",
            [],
        )
        .unwrap();
    let free = store
        .get_user_model_routing(&config, "stranger@example.com", &HashSet::new())
        .await
        .unwrap();
    assert_eq!(free.eligible_shares[0].access, "free");
    assert_masked(&free.eligible_shares[0]);
    let owner = store
        .get_user_model_routing(&config, "owner@example.com", &HashSet::new())
        .await
        .unwrap();
    assert_eq!(
        owner.eligible_shares[0].app_capabilities[0]
            .account_email
            .as_deref(),
        Some(PRIVATE_ACCOUNT)
    );
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE shares SET owner_email='next@example.com' WHERE share_id='identity-share'",
            [],
        )
        .unwrap();
    let previous = store
        .get_user_model_routing(&config, "owner@example.com", &HashSet::new())
        .await
        .unwrap();
    assert_masked(&previous.eligible_shares[0]);
    let next = store
        .get_user_model_routing(&config, "next@example.com", &HashSet::new())
        .await
        .unwrap();
    assert_eq!(next.eligible_shares[0].access, "owner");
    assert_eq!(
        next.eligible_shares[0].app_capabilities[0]
            .account_email
            .as_deref(),
        Some(PRIVATE_ACCOUNT)
    );
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE shares SET owner_email=NULL WHERE share_id='identity-share'",
            [],
        )
        .unwrap();
    let missing = store
        .get_user_model_routing(&config, "next@example.com", &HashSet::new())
        .await
        .unwrap();
    assert_masked(&missing.eligible_shares[0]);
}
