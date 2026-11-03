use super::*;
use crate::share_market::client_entries::parse_share_ids;

async fn fixture() -> (AppStore, AuthSession, AuthSession, String, String, String) {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let owner = session("entry-owner", "entry-owner@example.test");
    let buyer = session("entry-buyer", "entry-buyer@example.test");
    insert_share(
        &store,
        "entry-share",
        &owner.email,
        &[ShareTokenPeriod::Day],
    )
    .await;
    let (listing, seat) = create_listing(&store, &owner, "entry-share", free_seat()).await;
    let sub = store
        .rent_fixture_share(&buyer, &seat, RentSeatRequest { offer_revision: 1 })
        .await
        .unwrap();
    (store, owner, buyer, listing, seat, sub)
}

#[test]
fn client_entry_ids_are_bounded_and_deduplicated() {
    assert_eq!(parse_share_ids("b,a,b").unwrap(), vec!["a", "b"]);
    assert!(parse_share_ids("").unwrap().is_empty());
    assert!(parse_share_ids("a, bad").is_err());
    assert!(parse_share_ids(&"s".repeat(257)).is_err());
    assert!(
        parse_share_ids(
            &(0..101)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
        .is_err()
    );
}

#[tokio::test]
async fn client_entries_preserve_full_unlisted_and_missing_rentals_without_exposing_private_resources()
 {
    let (store, _, buyer, listing, _, sub) = fixture().await;
    let ids = vec!["entry-share".into(), "missing-share".into()];
    let mine = store
        .client_share_entries(Some(&buyer), &ids)
        .await
        .unwrap();
    assert_eq!(mine[0].entry_kind, "rental");
    assert_eq!(mine[0].vacant_seats, 0);
    assert_eq!(mine[1].resource_state, "unavailable");
    let full = store
        .client_seat_context(Some(&buyer), "entry-share", &[], None, None)
        .await
        .unwrap();
    assert_eq!(full.subscriptions[0].id, sub);
    let public = store
        .client_seat_context(None, "entry-share", &[], None, None)
        .await
        .unwrap();
    assert!(public.subscriptions.is_empty());
    assert!(
        public
            .listing
            .unwrap()
            .seats
            .iter()
            .all(|s| s.subscription.is_none() && !s.can_delete)
    );
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE share_market_listings SET status='closed' WHERE id=?1",
            params![listing],
        )
        .unwrap();
    let own = store
        .client_seat_context(Some(&buyer), "entry-share", &[], None, None)
        .await
        .unwrap();
    assert_eq!(own.resource_state, "unlisted");
    assert!(own.listing.is_none());
    assert_eq!(own.subscriptions.len(), 1);
    assert!(
        store
            .client_seat_context(None, "entry-share", &[], None, None)
            .await
            .is_err()
    );
    let hidden = store.client_share_entries(None, &ids).await.unwrap();
    assert_eq!(hidden[0].resource_state, "unavailable");
    assert!(hidden[0].subdomain.is_none());
    store
        .conn
        .lock()
        .await
        .execute("DELETE FROM shares WHERE share_id='entry-share'", [])
        .unwrap();
    let missing = store
        .client_seat_context(Some(&buyer), "entry-share", &[], None, None)
        .await
        .unwrap();
    assert_eq!(missing.resource_state, "missing");
    assert_eq!(missing.subscriptions[0].id, sub);
}

#[tokio::test]
async fn client_entry_vacancies_do_not_offer_a_second_seat_to_owners_or_direct_grantees() {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let owner = session("o", "owner@example.test");
    let buyer = session("b", "buyer@example.test");
    insert_share(&store, "available", &owner.email, &[ShareTokenPeriod::Day]).await;
    create_listing(&store, &owner, "available", free_seat()).await;
    let ids = vec!["available".into()];
    assert_eq!(
        store.client_share_entries(None, &ids).await.unwrap()[0].entry_kind,
        "rent"
    );
    assert_eq!(
        store
            .client_share_entries(Some(&owner), &ids)
            .await
            .unwrap()[0]
            .entry_kind,
        "none"
    );
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE shares SET user_grants_json=?1 WHERE share_id='available'",
            params![
                serde_json::json!({buyer.email.clone(): {"email":buyer.email,"active":true}})
                    .to_string()
            ],
        )
        .unwrap();
    assert_eq!(
        store
            .client_share_entries(Some(&buyer), &ids)
            .await
            .unwrap()[0]
            .entry_kind,
        "none"
    );
}

#[tokio::test]
async fn client_seat_context_never_turns_a_released_rental_into_ownership_of_a_new_tenants_seat() {
    let (store, _, buyer, _, seat, sub) = fixture().await;
    {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE share_market_subscriptions SET status='released' WHERE id=?1",
            params![sub],
        )
        .unwrap();
        conn.execute("UPDATE share_market_seats SET status='available',current_subscription_id=NULL WHERE id=?1", params![seat]).unwrap();
    }
    let next_buyer = session("next-buyer", "next@example.test");
    let next_sub = store
        .rent_fixture_share(&next_buyer, &seat, RentSeatRequest { offer_revision: 1 })
        .await
        .unwrap();
    let context = store
        .client_seat_context(Some(&buyer), "entry-share", &[], None, Some(&sub))
        .await
        .unwrap();
    assert_eq!(context.subscriptions.len(), 1);
    assert_eq!(context.subscriptions[0].status, "released");
    assert!(context.listing.unwrap().seats[0].subscription.is_none());
    assert!(
        store
            .client_seat_context(Some(&buyer), "entry-share", &[], None, Some(&next_sub))
            .await
            .is_err()
    );
    assert!(
        store
            .client_rental_contexts(&buyer, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn client_contexts_page_beyond_five_and_twenty_and_bind_cursors_to_actor_and_scope() {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let owner = session("pages-owner", "pages-owner@example.test");
    let buyer = session("pages-buyer", "pages-buyer@example.test");
    for i in 0..23 {
        let id = format!("page-{i:02}");
        insert_share(&store, &id, &owner.email, &[ShareTokenPeriod::Day]).await;
        let (_, seat) = create_listing(&store, &owner, &id, free_seat()).await;
        store
            .rent_fixture_share(&buyer, &seat, RentSeatRequest { offer_revision: 1 })
            .await
            .unwrap();
    }
    let first = store.client_rental_contexts(&buyer, None).await.unwrap();
    assert_eq!(first.items.len(), 20);
    let cursor = first.next_cursor.unwrap();
    let second = store
        .client_rental_contexts(&buyer, Some(&cursor))
        .await
        .unwrap();
    assert_eq!(second.items.len(), 3);
    assert!(second.next_cursor.is_none());
    assert!(
        !first
            .items
            .iter()
            .any(|a| second.items.iter().any(|b| a.share_id == b.share_id))
    );
    assert!(
        store
            .client_rental_contexts(&owner, Some(&cursor))
            .await
            .is_err()
    );
    assert!(
        store
            .client_seat_context(Some(&buyer), "page-00", &[], Some(&cursor), None)
            .await
            .is_err()
    );
    let filtered = store
        .client_seat_context(None, "page-22", &[], None, None)
        .await
        .unwrap();
    assert_eq!(filtered.listing.unwrap().share_id, "page-22");
    assert!(filtered.subscriptions.is_empty());
}

fn insert_pending_settlement(conn: &Connection, record: &SubscriptionRecord) {
    use crate::share_market_termination::Termination;
    let at = Utc::now().to_rfc3339();
    let t = Termination {
        id: format!("termination-{}", record.id),
        subscription_id: record.id.clone(),
        share_id: record.share_id.clone(),
        seat_id: record.seat_id.clone(),
        listing_id: record.listing_id.clone(),
        entitlement_id: record.entitlement_id.clone(),
        installation_id: record.installation_id.clone(),
        share_name: record.share_name.clone(),
        subdomain: record.subdomain.clone(),
        buyer_user_id: record.renter_user_id.clone(),
        provider_user_id: record.owner_user_id.clone(),
        buyer_email: record.renter_email.clone(),
        provider_email: record.owner_email.clone(),
        contract_id: None,
        period_id: None,
        policy_version: 1,
        rate_bps: 1000,
        period_amount_units: 0,
        time_earned_units: 0,
        prior_refunded_units: 0,
        quota_earned_units: 0,
        remaining_units: 0,
        compensation_cap_units: 0,
        returned_units: 0,
        compensation_units: 0,
        held_units: 0,
        policy: ShareUserPolicy::default(),
        period_start_ms: 0,
        period_end_ms: 0,
        cutoff_at: at.clone(),
        revoked_at: Some(at.clone()),
        status: "finance_pending".into(),
        reason: "provider_issue".into(),
        claim: String::new(),
        responsibility: "pending".into(),
        dispute_opened_at: Some(at.clone()),
        provider_responded_at: None,
        decision_note: None,
        decided_award_units: None,
        version: 1,
        updated_at: at.clone(),
    };
    conn.execute("INSERT INTO share_market_terminations(id,subscription_id,share_id,contract_id,period_id,buyer_user_id,provider_user_id,status,cutoff_at,updated_at,version,payload_json,entitlement_id) VALUES(?1,?2,?3,NULL,NULL,?4,?5,?6,?7,?7,1,?8,?9)",params![t.id,t.subscription_id,t.share_id,t.buyer_user_id,t.provider_user_id,t.status,at,serde_json::to_string(&t).unwrap(),t.entitlement_id]).unwrap();
}

#[tokio::test]
async fn client_settlement_context_survives_release_and_deletion_and_reads_never_write_money_or_usage()
 {
    let (store, owner, buyer, listing, seat, sub) = fixture().await;
    {
        let conn = store.conn.lock().await;
        insert_pending_settlement(&conn, &subscription_record(&conn, &sub).unwrap().unwrap());
        conn.execute(
            "UPDATE share_market_subscriptions SET status='released' WHERE id=?1",
            params![sub],
        )
        .unwrap();
        conn.execute("UPDATE share_market_seats SET status='available',current_subscription_id=NULL WHERE id=?1", params![seat]).unwrap();
        conn.execute(
            "UPDATE share_market_listings SET deleted_at=?2,status='closed' WHERE id=?1",
            params![listing, Utc::now().to_rfc3339()],
        )
        .unwrap();
        conn.execute("DELETE FROM shares WHERE share_id='entry-share'", [])
            .unwrap();
    }
    let before: i64 = store
        .conn
        .lock()
        .await
        .query_row("SELECT total_changes()", [], |r| r.get(0))
        .unwrap();
    let entries = store
        .client_share_entries(Some(&buyer), &["entry-share".into()])
        .await
        .unwrap();
    assert_eq!(entries[0].entry_kind, "settlement");
    assert_eq!(entries[0].resource_state, "missing");
    let recovery = store.client_rental_contexts(&buyer, None).await.unwrap();
    assert_eq!(recovery.items.len(), 1);
    let detail = store
        .client_seat_context(Some(&buyer), "entry-share", &[], None, None)
        .await
        .unwrap();
    assert!(detail.listing.is_none());
    assert_eq!(
        detail.subscriptions[0].termination_id,
        Some(format!("termination-{sub}"))
    );
    assert_eq!(detail.subscriptions[0].status, "released");
    assert!(
        store
            .client_seat_context(Some(&owner), "entry-share", &[], None, None)
            .await
            .is_err()
    );
    let after: i64 = store
        .conn
        .lock()
        .await
        .query_row("SELECT total_changes()", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after, "all new projections must be read-only");
}
