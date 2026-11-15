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
    store
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
