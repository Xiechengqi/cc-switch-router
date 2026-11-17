use super::*;

#[tokio::test]
async fn share_account_identity_market_uses_current_owner_and_separates_etags() {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let owner = session("account-owner", "owner@example.test");
    let renter = session("account-renter", "renter@example.test");
    let next = session("account-next", "next@example.test");
    let share_id = "account-privacy-share";
    let account = "private-market-account@example.test";
    insert_share(&store, share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
    let runtime = serde_json::json!({"codex": {
        "kind": "codex_oauth", "app": "codex", "providerType": "codex_oauth",
        "providerName": "OpenAI Official", "accountEmail": account,
        "modelPolicy": {"mode": "passthrough"}, "available": true
    }})
    .to_string();
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE shares SET app_runtimes_json=?2 WHERE share_id=?1",
            params![share_id, runtime],
        )
        .unwrap();
    let (listing_id, seat_id) = create_listing(&store, &owner, share_id, free_seat()).await;
    let domains = vec![format!("{share_id}-route")];
    let owned = store.share_market_owned_shares(&owner).await.unwrap();
    assert_eq!(
        owned[0].app_capabilities[0].account_hint.as_deref(),
        Some(account)
    );
    let owner_catalog = store
        .share_market_catalog_with_scope(Some(&owner), &domains, ShareMarketCatalogScope::Owner)
        .await
        .unwrap();
    assert_eq!(
        owner_catalog.listings[0].app_capabilities[0]
            .account_hint
            .as_deref(),
        Some(account)
    );
    let owner_response = private_etag_json(&HeaderMap::new(), &owner_catalog).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::IF_NONE_MATCH,
        owner_response.headers()[header::ETAG].clone(),
    );
    for viewer in [None, Some(&renter)] {
        let catalog = store
            .share_market_catalog_with_scope(viewer, &domains, ShareMarketCatalogScope::Public)
            .await
            .unwrap();
        assert!(!serde_json::to_string(&catalog).unwrap().contains(account));
        assert_eq!(
            catalog.listings[0].app_capabilities[0]
                .account_hint
                .as_deref(),
            Some("p***t@example.test")
        );
        let response = private_etag_json(&headers, &catalog).unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "an owner validator cannot reuse a public body"
        );
        assert_eq!(response.headers()[header::VARY], "authorization");
    }
    let subscription_id = store
        .rent_fixture_share(&renter, &seat_id, RentSeatRequest { offer_revision: 1 })
        .await
        .unwrap();
    let rented = store
        .share_market_catalog_with_scope(
            Some(&renter),
            &domains,
            ShareMarketCatalogScope::RenterListings,
        )
        .await
        .unwrap();
    assert!(!rented.listings.is_empty());
    assert!(!serde_json::to_string(&rented).unwrap().contains(account));
    assert!(
        !rented.listings[0].can_view_account_identity,
        "a pending subscription alone is not invocation authority"
    );
    set_entitlement(&store, &subscription_id, Utc::now()).await;
    let granted = store
        .share_market_catalog_with_scope(
            Some(&renter),
            &domains,
            ShareMarketCatalogScope::RenterListings,
        )
        .await
        .unwrap();
    assert!(granted.listings[0].can_view_account_identity);
    assert_eq!(
        granted.listings[0].app_capabilities[0]
            .account_hint
            .as_deref(),
        Some(account)
    );
    let full_response = private_etag_json(&HeaderMap::new(), &granted).unwrap();
    let mut full_headers = HeaderMap::new();
    full_headers.insert(
        header::IF_NONE_MATCH,
        full_response.headers()[header::ETAG].clone(),
    );
    clear_entitlements(&store, share_id).await;
    let revoked = store
        .share_market_catalog_with_scope(
            Some(&renter),
            &domains,
            ShareMarketCatalogScope::RenterListings,
        )
        .await
        .unwrap();
    assert!(!revoked.listings[0].can_view_account_identity);
    assert!(!serde_json::to_string(&revoked).unwrap().contains(account));
    assert_eq!(
        private_etag_json(&full_headers, &revoked).unwrap().status(),
        StatusCode::OK
    );
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE shares SET owner_email=?2 WHERE share_id=?1",
            params![share_id, next.email],
        )
        .unwrap();
    let historical = store
        .share_market_catalog_with_scope(Some(&owner), &domains, ShareMarketCatalogScope::Owner)
        .await
        .unwrap();
    assert_eq!(historical.listings[0].id, listing_id);
    assert!(
        historical.listings[0].is_owner,
        "listing ownership remains historical"
    );
    assert!(
        !serde_json::to_string(&historical)
            .unwrap()
            .contains(account)
    );
    let current = store.share_market_owned_shares(&next).await.unwrap();
    assert_eq!(
        current[0].app_capabilities[0].account_hint.as_deref(),
        Some(account)
    );
    let raw: String = store
        .conn
        .lock()
        .await
        .query_row(
            "SELECT app_runtimes_json FROM shares WHERE share_id=?1",
            params![share_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(raw.contains(account));
}

#[tokio::test]
async fn share_account_identity_market_rechecks_direct_grants_and_free_historical_access() {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let owner = session("identity-free-owner", "owner@example.test");
    let viewer = session("identity-free-viewer", "viewer@example.test");
    let share_id = "identity-free-market";
    insert_share(&store, share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
    let (listing_id, _) = create_listing(&store, &owner, share_id, free_seat()).await;
    let runtime = serde_json::json!({"codex": {
        "app": "codex", "kind": "codex_oauth", "accountLabel": "邵义强",
        "providerName": "OpenAI 邵义强", "available": true,
    }})
    .to_string();
    let now = Utc::now().timestamp_millis();
    for (active, expiry, authenticated, expected) in [
        (false, None, true, false),
        (true, None, true, true),
        (true, Some(now - 1), true, false),
        (true, None, false, false),
        (false, None, true, false),
    ] {
        let grants = BTreeMap::from([(
            viewer.email.clone(),
            ShareUserGrant {
                email: viewer.email.clone(),
                role: "shareto".into(),
                active,
                policy: ShareUserPolicy {
                    expires_at: expiry,
                    ..Default::default()
                },
                ..Default::default()
            },
        )]);
        store
            .conn
            .lock()
            .await
            .execute(
                "UPDATE shares SET user_grants_json=?2, app_runtimes_json=?3 WHERE share_id=?1",
                params![share_id, serde_json::to_string(&grants).unwrap(), runtime],
            )
            .unwrap();
        let catalog = store
            .share_market_catalog_with_scope(
                authenticated.then_some(&viewer),
                &[],
                ShareMarketCatalogScope::Public,
            )
            .await
            .unwrap();
        let listing = &catalog.listings[0];
        assert_eq!(listing.can_view_account_identity, expected);
        assert_eq!(
            listing.app_capabilities[0].account_hint.as_deref(),
            Some(if expected { "邵义强" } else { "邵****强" })
        );
        if !expected {
            assert!(!serde_json::to_string(listing).unwrap().contains("邵义强"));
        }
    }
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE shares SET user_grants_json='{' WHERE share_id=?1",
            params![share_id],
        )
        .unwrap();
    let malformed = store
        .share_market_catalog_with_scope(Some(&viewer), &[], ShareMarketCatalogScope::Public)
        .await
        .unwrap();
    assert!(!malformed.listings[0].can_view_account_identity);
    assert!(
        !serde_json::to_string(&malformed)
            .unwrap()
            .contains("邵义强")
    );
    // A free Share cannot have active market listings. Closed history can
    // remain after an owner transfer, and must use the current free ACL.
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE share_market_listings SET status='closed' WHERE id=?1",
            params![listing_id],
        )
        .unwrap();
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE shares SET owner_email='next@example.test', free_access=1 WHERE share_id=?1",
            params![share_id],
        )
        .unwrap();
    let historical = store
        .share_market_catalog_with_scope(Some(&owner), &[], ShareMarketCatalogScope::Owner)
        .await
        .unwrap();
    assert!(historical.listings[0].can_view_account_identity);
    assert_eq!(
        historical.listings[0].app_capabilities[0]
            .account_hint
            .as_deref(),
        Some("邵义强")
    );
}
