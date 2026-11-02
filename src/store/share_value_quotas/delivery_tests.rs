fn fixture() -> Connection {
    let conn = crate::test_support::database().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    for id in ["server-a", "server-b", "no-share"] {
        // Match production's one-public-key-per-installation constraint.
        conn.execute(
            "INSERT INTO installations(id,public_key,platform,app_version,created_at,last_seen_at,control_secret_b64) VALUES(?1,?2,'fixture','1',?3,?3,'fixture')",
            params![id, format!("fixture-key-{id}"), Utc::now().to_rfc3339()],
        )
        .unwrap();
    }
    for (id, installation) in [("share-a", "server-a"), ("share-b", "server-b")] {
        conn.execute("INSERT INTO shares(share_id,capacity_pool_id,installation_id,share_name,subdomain,app_type,enabled_claude,enabled_codex,enabled_gemini,token_limit,parallel_limit,tokens_used,requests_count,share_status,created_at,expires_at,updated_at)
            VALUES(?1,?1,?2,?1,?1,'proxy',1,1,1,0,1,0,0,'active','now','later','now')",params![id,installation]).unwrap();
    }
    for (revision, effective) in [
        ("old", 0),
        ("new", 1),
        ("future", Utc::now().timestamp_millis() + 60_000),
    ] {
        conn.execute(
            "INSERT INTO share_value_pricing_snapshots VALUES(?1,'{}',?2,'fixture',0)",
            params![revision, effective],
        )
        .unwrap();
    }
    conn
}

#[test]
fn activation_requires_this_installations_ack_and_a_due_revision() {
    let conn = fixture();
    assert!(apply_activation(&conn, "server-a", "new", 1).is_err());
    conn.execute(
        "INSERT INTO share_value_pricing_acks VALUES('server-b','new',1)",
        [],
    )
    .unwrap();
    assert!(apply_activation(&conn, "server-a", "new", 1).is_err());
    for rev in ["new", "future"] {
        conn.execute(
            "INSERT INTO share_value_pricing_acks VALUES('server-a',?1,1)",
            params![rev],
        )
        .unwrap();
    }
    assert!(apply_activation(&conn, "server-a", "future", 2).is_err());
    assert_eq!(
        delivery_health(&conn, Some("new"), Utc::now().timestamp_millis()).unwrap()["activatedInstallations"],
        0
    );
    apply_activation(&conn, "server-a", "new", 3).unwrap();
    assert_eq!(
        installation_activation(
            &conn,
            "server-a",
            Some("new"),
            Utc::now().timestamp_millis()
        )
        .unwrap()["current"],
        true
    );
}

#[test]
fn late_reports_never_replace_newer_evidence_and_stale_receipts_do_not_prove_live() {
    let conn = fixture();
    for revision in ["old", "new"] {
        conn.execute(
            "INSERT INTO share_value_pricing_acks VALUES('server-a',?1,1)",
            params![revision],
        )
        .unwrap();
    }
    apply_activation(&conn, "server-a", "new", 20).unwrap();
    apply_activation(&conn, "server-a", "old", 19).unwrap();
    let now = Utc::now().timestamp_millis();
    assert_eq!(
        installation_activation(&conn, "server-a", Some("new"), now).unwrap()["activePricingRevision"],
        "new"
    );
    let health = delivery_health(&conn, Some("new"), now).unwrap();
    assert_eq!(
        health,
        serde_json::json!({"expectedInstallations":2,"acknowledgedInstallations":1,"activatedInstallations":1,"unconfirmedInstallations":1})
    );
    assert_eq!(
        delivery_health(&conn, Some("new"), now + PROJECTION_FRESH_MS + 1).unwrap()["activatedInstallations"],
        0
    );
    conn.execute(
        "UPDATE installations SET lifecycle='fenced' WHERE id='server-b'",
        [],
    )
    .unwrap();
    assert_eq!(
        delivery_health(&conn, Some("new"), now).unwrap()["expectedInstallations"],
        1
    );
    conn.execute("DELETE FROM shares WHERE share_id='share-a'", [])
        .unwrap();
    conn.execute(
        "DELETE FROM share_value_pricing_acks WHERE installation_id='server-a'",
        [],
    )
    .unwrap();
    conn.execute("DELETE FROM installations WHERE id='server-a'", [])
        .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM share_value_pricing_activations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn publication_retry_covers_failed_edits_missing_targets_and_scheduled_catalog_generations() {
    let conn = fixture();
    assert!(needs_retry(&conn, "catalog", 100).unwrap());
    mark_target(&conn, "catalog", 100_000).unwrap();
    assert_eq!(
        publication_status(&conn, "catalog", 100).unwrap()["state"],
        "pending"
    );
    assert!(needs_retry(&conn, "catalog", 100).unwrap());
    conn.execute("UPDATE share_value_pricing_publication SET published_catalog_revision=target_catalog_revision",[]).unwrap();
    assert!(!needs_retry(&conn, "catalog", 100).unwrap());
    assert!(
        needs_retry(&conn, "another-catalog", 100).unwrap(),
        "even a failure to persist the new target must be retried"
    );
    // Force a future generation without sleeps or changes to the clock.
    conn.execute("INSERT INTO model_price_catalog(price_key,effective_from,display_name,currency,source,updated_at) VALUES('test-future',101,'Fixture','USD','admin',100)",[]).unwrap();
    assert!(!needs_retry(&conn, "catalog", 100).unwrap());
    assert!(needs_retry(&conn, "catalog", 101).unwrap());
    assert_eq!(
        publication_status(&conn, "catalog", 101).unwrap()["state"],
        "pending",
        "an effective generation not yet derived is not published"
    );
    conn.execute(
        "UPDATE share_value_pricing_publication SET updated_at_ms=?1",
        params![101_000],
    )
    .unwrap();
    assert!(!needs_retry(&conn, "catalog", 101).unwrap());
    mark_target(&conn, "new-catalog", 102_000).unwrap();
    assert_eq!(
        publication_status(&conn, "new-catalog", 102).unwrap()["state"],
        "pending"
    );
}
