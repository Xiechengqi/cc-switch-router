use super::*;
use crate::client_market_termination::tests::{HOST, START, accept, deliver, fixture};

#[test]
fn workbench_uses_original_parties_and_separates_reply_from_owner_decision() {
    let (conn, contract) = fixture(1000, true, 0);
    let at = deliver(&conn, &contract, 10);
    let t = accept(&conn, &at, "provider_issue");
    let f = fact_tx(&conn, &t.id).unwrap();
    let provider = project(&f, "provider", false, &at).unwrap();
    assert!(provider.action_required && provider.permissions.respond);
    assert_eq!(provider.attention, "provider_response");
    let owner = project(&f, "owner", true, &at).unwrap();
    assert!(!owner.action_required && owner.permissions.resolve);
    assert_eq!(owner.attention, "waiting_provider");
    assert!(detail_tx(&conn, &t.id, "stranger", false, &at, None).is_err());
    let response = ProviderResponse {
        expected_version: t.version,
        note: "Request a decision".into(),
    };
    respond_tx(&conn, &t.id, "provider", &response, &at).unwrap();
    let f = fact_tx(&conn, &t.id).unwrap();
    assert_eq!(
        project(&f, "owner", true, &at).unwrap().attention,
        "owner_decision"
    );
    assert!(!project(&f, "provider", false, &at).unwrap().action_required);
    let d = detail_tx(&conn, &t.id, "owner", true, &at, None).unwrap();
    assert_eq!(d["events"][0]["note"], "Request a decision");
    assert_eq!(
        summary_tx(&conn, "owner", true, &at).unwrap()["actionRequiredCount"],
        1
    );
    assert_eq!(
        summary_tx(&conn, "stranger", false, &at).unwrap()["hosts"],
        json!([])
    );
    let records = records_tx(
        &conn,
        "owner",
        true,
        &RecordsQuery {
            pending_only: true,
            ..Default::default()
        },
        &at,
    )
    .unwrap();
    assert_eq!(records["items"][0]["id"], t.id);
    assert_eq!(records["items"][0]["hostId"], HOST);
}

#[test]
fn workbench_deadlines_finance_holds_and_inapp_notices_do_not_move_money() {
    let (conn, contract) = fixture(1000, true, 0);
    let at = deliver(&conn, &contract, 10);
    let t = accept(&conn, &at, "provider_issue");
    let mut f = fact_tx(&conn, &t.id).unwrap();
    let deadline = (time(&at).unwrap() + Duration::hours(72)).to_rfc3339();
    let before = (time(&deadline).unwrap() - Duration::nanoseconds(1)).to_rfc3339();
    assert!(
        project(&f, "provider", false, &before)
            .unwrap()
            .permissions
            .respond
    );
    assert_eq!(
        project(&f, "provider", false, &deadline).unwrap().attention,
        "timeout_pending"
    );
    assert!(
        !project(&f, "owner", true, &deadline)
            .unwrap()
            .permissions
            .resolve
    );
    f.status = "finance_pending".into();
    let p = project(&f, "provider", false, &at).unwrap();
    assert!(p.permissions.respond && !p.permissions.waive);
    assert_eq!(
        project(&f, "owner", true, &at).unwrap().attention,
        "finance_review"
    );
    notify_owner_ready_tx(&conn, &t, &at).unwrap();
    notify_owner_ready_tx(&conn, &t, &at).unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM user_notification_events WHERE event_kind='client_termination_decision_required' AND scope='owner' AND category='client'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    let (conn, _) = fixture(1000, true, 3600);
    let t = accept(&conn, START, "voluntary");
    assert!(
        !project(&fact_tx(&conn, &t.id).unwrap(), "provider", false, START)
            .unwrap()
            .action_required
    );
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM user_notification_events WHERE event_kind='client_termination_started'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM market_notification_outbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

fn stub(conn: &Connection, id: &str, host: &str, provider: &str, at: &str, responded: bool) {
    conn.execute(
        "INSERT INTO client_market_terminations
        (id,installation_id,host_id,buyer_user_id,provider_user_id,policy_version,rate_bps,
         period_amount_units,remaining_units,compensation_cap_units,reason,responsibility,status,
         cutoff_at,dispute_opened_at,provider_responded_at,dispute_used,updated_at)
        VALUES (?1,?1,?2,'buyer',?3,0,1000,1000,1000,1000,'provider_issue','undetermined',
         'cleanup_pending',?4,?4,CASE WHEN ?5=1 THEN ?4 END,1,?4)",
        params![id, host, provider, at, responded],
    )
    .unwrap();
}

#[test]
fn workbench_survives_host_reuse_deletion_and_does_not_give_a_new_provider_old_claims() {
    let (conn, contract) = fixture(1000, true, 0);
    let at = deliver(&conn, &contract, 10);
    let t = accept(&conn, &at, "provider_issue");
    conn.execute(
        "INSERT INTO router_ssh_hosts
        (id,ip,port,host_owner_email,status,installation_id,provider_id,created_at,updated_at)
        VALUES (?1,'192.0.2.10',22,'provider@example.test','draining',?2,'provider',?3,?3)",
        params![HOST, t.installation_id, at],
    )
    .unwrap();
    cleanup_failed_tx(&conn, &t.installation_id, &at).unwrap();
    assert!(fact_tx(&conn, &t.id).unwrap().retry_cleanup);
    conn.execute("UPDATE router_ssh_hosts SET installation_id='new-install',provider_id='new-provider',status='allocated' WHERE id=?1",params![HOST]).unwrap();
    assert!(!fact_tx(&conn, &t.id).unwrap().retry_cleanup);
    assert!(detail_tx(&conn, &t.id, "new-provider", false, &at, None).is_err());
    assert_eq!(
        summary_tx(&conn, "new-provider", false, &at).unwrap()["hosts"],
        json!([])
    );
    assert_eq!(
        summary_tx(&conn, "provider", false, &at).unwrap()["hosts"][0]["terminationId"],
        t.id
    );
    conn.execute("DELETE FROM router_ssh_hosts WHERE id=?1", params![HOST])
        .unwrap();
    assert_eq!(
        detail_tx(&conn, &t.id, "provider", false, &at, None).unwrap()["hostId"],
        HOST
    );
    assert_eq!(
        records_tx(
            &conn,
            "provider",
            false,
            &RecordsQuery {
                pending_only: true,
                ..Default::default()
            },
            &at
        )
        .unwrap()["items"][0]["id"],
        t.id
    );
}

#[test]
fn workbench_pages_all_actions_and_history_even_after_fifty_newer_nonactionable_records() {
    let (conn, _) = fixture(1000, true, 3600);
    // Every Host can be absent: the work queue is not the public Host listing.
    for n in 0..65 {
        stub(
            &conn,
            &format!("work-{n:03}"),
            "deleted-host",
            "provider",
            START,
            true,
        );
    }
    for n in 0..60 {
        stub(
            &conn,
            &format!("waiting-{n:03}"),
            "other-host",
            "provider",
            "2026-01-01T00:01:00+00:00",
            false,
        );
    }
    let summary = summary_tx(&conn, "owner", true, START).unwrap();
    assert_eq!(summary["actionRequiredCount"], 65);
    let first = records_tx(
        &conn,
        "owner",
        true,
        &RecordsQuery {
            pending_only: true,
            ..Default::default()
        },
        START,
    )
    .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 50);
    let second = records_tx(
        &conn,
        "owner",
        true,
        &RecordsQuery {
            pending_only: true,
            cursor: first["nextCursor"].as_str().map(String::from),
            ..Default::default()
        },
        START,
    )
    .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 15);
    assert!(second["nextCursor"].is_null());
    let ids = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["items"].as_array().unwrap())
        .map(|v| v["id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 65);
    assert!(
        records_tx(
            &conn,
            "stranger",
            false,
            &RecordsQuery {
                cursor: Some("work-001".into()),
                ..Default::default()
            },
            START
        )
        .is_err()
    );
    let by_host = records_tx(
        &conn,
        "provider",
        false,
        &RecordsQuery {
            host_id: Some("deleted-host".into()),
            ..Default::default()
        },
        START,
    )
    .unwrap();
    assert_eq!(by_host["items"].as_array().unwrap().len(), 50);
    assert!(
        by_host["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["hostId"] == "deleted-host")
    );
}

#[test]
fn workbench_event_history_is_scoped_and_paginated_without_duplicating_notes() {
    let (conn, _) = fixture(1000, true, 3600);
    let t = accept(&conn, START, "voluntary");
    for n in 0..60 {
        conn.execute("INSERT INTO client_termination_events (id,termination_id,version,actor_user_id,action,detail_json,created_at)
            VALUES (?1,?2,?3,'provider','provider_response',?4,?5)",
            params![format!("event-{n:03}"),t.id,n+2,json!({"note":format!("Private note {n}")}).to_string(),START]).unwrap();
    }
    let first = detail_tx(&conn, &t.id, "provider", false, START, None).unwrap();
    assert_eq!(first["events"].as_array().unwrap().len(), 50);
    let second = detail_tx(
        &conn,
        &t.id,
        "provider",
        false,
        START,
        first["eventsNextCursor"].as_str(),
    )
    .unwrap();
    assert_eq!(second["events"].as_array().unwrap().len(), 11);
    assert!(second["eventsNextCursor"].is_null());
    let ids = first["events"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["events"].as_array().unwrap())
        .map(|v| v["id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 61);
    let other = accept(&fixture(1000, true, 3600).0, START, "voluntary");
    assert!(detail_tx(&conn, &t.id, "provider", false, START, Some(&other.id)).is_err());
}
