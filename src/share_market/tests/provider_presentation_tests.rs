use super::*;

#[tokio::test]
async fn identity_backfill_and_display_updates_preserve_live_rental_contracts() {
    let store = AppStore::new_in_memory_for_tests().unwrap();
    let owner = session("identity-owner", "identity-owner@example.test");
    let buyer = session("identity-buyer", "identity-buyer@example.test");
    let share_id = "identity-rental";
    insert_share(&store, share_id, &owner.email, &[ShareTokenPeriod::Day]).await;
    let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../assets/provider-identity-fixtures.json"
    ))
    .unwrap();
    let f = fixtures
        .iter()
        .find(|f| f["name"] == "family.minimax_cn")
        .unwrap();
    let mut provider: crate::models::ShareAppProvider =
        serde_json::from_value(f["appProviders"]["codex"][0].clone()).unwrap();
    provider.id = "provider-test".into();
    provider.kind = Some("codex".into());
    provider.provider_type = Some("codex".into());
    provider.enabled = true;
    provider.model_policy = Some(crate::models::ShareProviderModelPolicy::Passthrough);
    let identity = provider.provider_identity.take().unwrap();
    let mut apps = crate::models::ShareAppProviders {
        codex: vec![provider],
        ..Default::default()
    };
    let write_apps = |conn: &Connection, apps: &crate::models::ShareAppProviders| {
        conn.execute(
            "UPDATE shares SET app_providers_json=?2 WHERE share_id=?1",
            params![share_id, serde_json::to_string(apps).unwrap()],
        )
        .unwrap();
    };
    {
        let conn = store.conn.lock().await;
        write_apps(&conn, &apps);
    }
    let (_, seat) = create_listing(&store, &owner, share_id, free_seat()).await;
    let subscription = store
        .rent_fixture_share(&buyer, &seat, RentSeatRequest { offer_revision: 1 })
        .await
        .unwrap();
    let now = Utc::now() + Duration::seconds(1);
    record_share_health(&store, share_id, now).await;
    activate_subscription(&store, &subscription, now).await;
    let conn = store.conn.lock().await;
    let read = |conn: &Connection| -> (String, String, String, i64) {
        conn.query_row(
            "SELECT status, integrity_state, service_snapshot_json,
            (SELECT COUNT(*) FROM share_market_events WHERE subscription_id=?1)
            FROM share_market_subscriptions WHERE id=?1",
            params![subscription],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    };
    let before = read(&conn);
    assert_eq!(before.1, "compatible");
    apps.codex[0].provider_identity = Some(identity);
    for display_name in ["MiniMax", "Renamed MiniMax"] {
        let identity = apps.codex[0].provider_identity.as_mut().unwrap();
        identity.display_name = display_name.into();
        identity.icon = Some("openai".into());
        identity.icon_color = Some("#123456".into());
        write_apps(&conn, &apps);
        reconcile_share_contract_integrity_tx(&conn, share_id, now).unwrap();
        assert_eq!(
            read(&conn),
            before,
            "identity updates must not change snapshots/status/events"
        );
        let public = crate::provider_presentation::identity_from_json(
            r#"{"codex":"provider-test"}"#,
            Some(&serde_json::to_string(&apps).unwrap()),
        );
        assert_eq!(public.category, "minimax");
    }
}
