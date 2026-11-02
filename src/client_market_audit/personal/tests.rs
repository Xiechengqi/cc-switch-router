use super::super::tests::{accept_voluntary_exit, paid, rental};
use super::*;

fn actor(id: &str) -> AuthSession {
    let now = Utc::now();
    AuthSession {
        session_id: "personal-test".into(),
        user_id: id.into(),
        email: format!("{id}@example.test"),
        auth_source_kind: "email".into(),
        auth_source_id: "fixture".into(),
        access_token_hash: "fixture-access".into(),
        refresh_token_hash: "fixture-refresh".into(),
        access_expires_at: now,
        refresh_expires_at: now,
        created_at: now,
        last_used_at: now,
    }
}
fn page(conn: &Connection, buyer: &str, view: &str, number: u32) -> Value {
    list_tx(
        conn,
        buyer,
        &PersonalQuery {
            view: Some(view.into()),
            page: Some(number),
            focus: None,
        },
    )
    .unwrap()
}

#[test]
fn personal_history_scopes_every_page_and_survives_deleted_hosts() {
    let conn = crate::test_support::database().unwrap();
    for n in 0..23 {
        rental(&conn, n, "released");
    }
    conn.execute("DELETE FROM router_ssh_hosts", []).unwrap();
    assert_eq!(page(&conn, "stranger", "history", 1)["total"], 0);
    let mut seen = std::collections::HashSet::new();
    for number in 1..=5 {
        let value = page(&conn, "buyer", "history", number);
        assert_eq!(value["total"], 23);
        for row in value["items"].as_array().unwrap() {
            assert!(seen.insert(row["id"].as_str().unwrap().to_string()));
            assert_eq!(row["group"], "history");
            for private in [
                "hostIp",
                "hostPort",
                "buyerEmail",
                "hostId",
                "providerNetUnits",
            ] {
                assert!(row.get(private).is_none(), "{private}");
            }
        }
    }
    assert_eq!(seen.len(), 23);
    assert_eq!(page(&conn, "buyer", "history", u32::MAX)["page"], 5);
}

#[test]
fn personal_current_and_history_partition_stopped_and_failed_services_once() {
    let conn = crate::test_support::database().unwrap();
    for (n, status) in [
        "opening",
        "active",
        "billing_suspended",
        "releasing",
        "release_failed",
        "released",
        "activation_failed",
    ]
    .into_iter()
    .enumerate()
    {
        rental(&conn, n, status);
    }
    let current = page(&conn, "buyer", "current", 1);
    let history = page(&conn, "buyer", "history", 1);
    assert_eq!(current["total"], 3);
    assert_eq!(history["total"], 4);
    assert!(
        history["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["endedAt"].is_string())
    );
    assert_eq!(page(&conn, "buyer", "current", u32::MAX)["page"], 1);
}

#[test]
fn personal_focus_locates_older_pages_without_traversing_or_reading_other_users() {
    let conn = crate::test_support::database().unwrap();
    for n in 0..23 {
        rental(&conn, n, "released");
    }
    let q = PersonalQuery {
        view: Some("history".into()),
        page: Some(1),
        focus: Some("client-0000".into()),
    };
    let focused = list_tx(&conn, "buyer", &q).unwrap();
    assert_eq!(focused["page"], 5);
    assert_eq!(focused["focusedId"], "job:job-0000");
    assert!(list_tx(&conn, "stranger", &q).unwrap()["focusedId"].is_null());
    assert_eq!(
        page(&conn, "buyer", "history", 3)["items"][0]["id"],
        "job:job-0012"
    );
}

#[test]
fn current_attention_priority_and_focused_page_use_the_same_ordering() {
    let (conn, contract, id) = paid(3600);
    for n in 2..=8 {
        rental(&conn, n, "active");
    }
    conn.execute(
        "UPDATE market_platform_contracts SET status='review_required' WHERE id=?1",
        params![contract],
    )
    .unwrap();
    let first = page(&conn, "buyer", "current", 1);
    assert_eq!(first["items"][0]["id"], id);
    assert_eq!(first["items"][0]["attention"], "finance_review");
    let focused = list_tx(
        &conn,
        "buyer",
        &PersonalQuery {
            view: Some("current".into()),
            page: Some(1),
            focus: Some("client-0002".into()),
        },
    )
    .unwrap();
    assert_eq!(focused["page"], 2);
    assert_eq!(focused["focusedId"], "job:job-0002");
    assert_eq!(focused["items"][2]["id"], "job:job-0002");
}

#[test]
fn personal_detail_proves_buyer_identity_before_money_and_redacts_admin_fields() {
    let (conn, _, id) = paid(3600);
    assert!(matches!(
        detail_tx(&conn, &actor("provider"), &id, &Default::default()),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        detail_tx(&conn, &actor("stranger"), &id, &Default::default()),
        Err(AppError::NotFound(_))
    ));
    let detail = detail_tx(&conn, &actor("buyer"), &id, &Default::default()).unwrap();
    assert_eq!(detail["record"]["status"], "trialing");
    assert_eq!(detail["record"]["money"]["netUnits"], "0");
    assert_eq!(detail["rental"]["isClientOwner"], true);
    let encoded = detail.to_string();
    for secret in [
        "SECRET_LOG",
        "hostIp",
        "hostPort",
        "buyerEmail",
        "providerNetUnits",
        "platformFeeUnits",
        "commissionRateBps",
        "journalId",
        "actorEmail",
    ] {
        assert!(!encoded.contains(secret), "{secret}");
    }
}

#[test]
fn personal_released_records_have_no_live_actions_and_keep_original_subdomain() {
    let conn = crate::test_support::database().unwrap();
    let id = rental(&conn, 1, "released");
    conn.execute(
        "UPDATE router_ssh_hosts SET installation_id='new-tenant' WHERE id='host-0001'",
        [],
    )
    .unwrap();
    let value = detail_tx(&conn, &actor("buyer"), &id, &Default::default()).unwrap();
    assert_eq!(value["record"]["clientSubdomain"], "client-0001");
    assert!(value["rental"].is_null());
    conn.execute("DELETE FROM router_ssh_hosts", []).unwrap();
    assert_eq!(
        detail_tx(&conn, &actor("buyer"), &id, &Default::default()).unwrap()["record"],
        value["record"]
    );
}

#[test]
fn stopped_paid_rentals_keep_pending_returns_in_history_without_double_counting_compensation() {
    for reason in ["voluntary", "provider_issue"] {
        let (conn, _, id) = paid(0);
        accept_voluntary_exit(&conn, "2026-01-01T00:00:00Z", reason);
        assert_eq!(page(&conn, "buyer", "current", 1)["total"], 0);
        let pending = detail_tx(&conn, &actor("buyer"), &id, &Default::default()).unwrap();
        assert_eq!(pending["record"]["group"], "history");
        assert_eq!(pending["record"]["cleanupStatus"], "pending");
        assert!(pending["rental"].is_null());
        assert_eq!(
            pending["record"]["money"]["netUnits"],
            pending["record"]["money"]["chargedUnits"]
        );
        crate::client_market_termination::record_cleanup_confirmed_tx(
            &conn,
            "client-0001",
            "host-0001",
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        let settled = detail_tx(&conn, &actor("buyer"), &id, &Default::default()).unwrap();
        let money = &settled["record"]["money"];
        let amount = |key: &str| money[key].as_str().unwrap().parse::<i64>().unwrap();
        assert_eq!(
            amount("netUnits"),
            crate::market_wallet::units_from_minor(100).unwrap()
        );
        assert_eq!(
            amount("chargedUnits") - amount("returnedUnits"),
            amount("netUnits")
        );
        if reason == "voluntary" {
            assert_eq!(amount("netUnits"), amount("compensationUnits"));
            assert_eq!(settled["record"]["attention"], "none");
        } else {
            assert_eq!(amount("compensationUnits"), 0);
            assert_eq!(
                money["unfulfilledUnits"],
                settled["settlement"]["heldUnits"]
            );
            assert_eq!(settled["record"]["attention"], "waiting_provider");
        }
    }
}

#[test]
fn personal_query_validation_and_exact_net_amounts() {
    for q in [
        json!({"view":"all"}),
        json!({"page":0}),
        json!({"view":"history","focus":""}),
    ] {
        let conn = crate::test_support::database().unwrap();
        let q: PersonalQuery = serde_json::from_value(q).unwrap();
        assert!(list_tx(&conn, "buyer", &q).is_err());
    }
    assert!(serde_json::from_value::<PersonalQuery>(json!({"buyer":"stranger"})).is_err());
    let record = buyer_record(
        &json!({"money":{"chargedUnits":"432000","returnedUnits":"345600",
        "unfulfilledUnits":"0","serviceUnits":"43200","compensationUnits":"43200"}}),
    )
    .unwrap();
    assert_eq!(record["money"]["netUnits"], "86400");
    assert!(buyer_record(&json!({"money":{"chargedUnits":"0","returnedUnits":"1"}})).is_err());
}

#[test]
fn personal_buyer_identity_source_uses_its_index() {
    let conn = crate::test_support::database().unwrap();
    let facts = facts(ReadScope::Buyer);
    let mut stmt = conn
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {facts}SELECT {COLUMNS} FROM records
        WHERE rental_group=?2 ORDER BY created_at DESC,id DESC LIMIT 5"
        ))
        .unwrap();
    let plan = stmt
        .query_map(params!["buyer", "history"], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<crate::db::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(plan.contains("idx_client_rental_history_buyer"), "{plan}");
}
