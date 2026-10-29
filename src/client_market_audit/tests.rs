use super::*;
use crate::market_platform_contracts::{self as contracts, BillingModel, PrepareContract};
use crate::market_wallet::units_from_minor;
const START: &str = "2026-01-01T00:00:00Z";

fn rental(conn: &Connection, n: usize, status: &str) -> String {
    let host = format!("host-{n:04}");
    let job = format!("job-{n:04}");
    let installation = format!("client-{n:04}");
    conn.execute("INSERT INTO router_ssh_hosts (id,ip,port,host_owner_email,provider_id,status,created_at,updated_at)
        VALUES (?1,?2,22,'provider@example.test','provider','idle',?3,?3)",params![host,format!("192.0.2.{n}"),START]).unwrap();
    conn.execute("INSERT INTO provisioning_jobs (id,type,host_id,host_owner_email,client_owner_email,client_owner_user_id,subdomain,status,phase,log_blob,created_at,updated_at)
        VALUES (?1,'create',?2,'provider@example.test','buyer@example.test','buyer',?3,'pending','locked','SECRET_LOG',?4,?4)",params![job,host,format!("client-{n:04}"),START]).unwrap();
    capture_job_identity_tx(conn, &job).unwrap();
    if status != "opening" && status != "activation_failed" {
        conn.execute("INSERT INTO client_market_subscriptions
            (installation_id,host_id,provider_id,host_owner_email,client_user_id,client_owner_email,status,offer_revision,pricing_model,activated_at,created_at,updated_at)
            VALUES (?1,?2,'provider','provider@example.test','buyer','buyer@example.test',?3,1,'free',?4,?4,?4)",params![installation,host,status,START]).unwrap();
        bind_installation_tx(conn, &job, &installation).unwrap();
        conn.execute(
            "UPDATE provisioning_jobs SET installation_id=?2,status='succeeded' WHERE id=?1",
            params![job, installation],
        )
        .unwrap();
    } else if status == "activation_failed" {
        conn.execute("UPDATE provisioning_jobs SET status='failed',failure_code='secret token raw error' WHERE id=?1",params![job]).unwrap();
    }
    format!("job:{job}")
}

fn paid(trial: i64) -> (Connection, String, String) {
    let conn = crate::test_support::database().unwrap();
    conn.execute(
        "UPDATE market_finance_control SET mode='enabled',cutover_approved_at=?1",
        params![START],
    )
    .unwrap();
    for (id, email) in [
        ("buyer", "buyer@example.test"),
        ("provider", "provider@example.test"),
    ] {
        conn.execute("INSERT INTO users (id,email_normalized,status,email_verified_at,created_at,last_login_at) VALUES (?1,?2,'active',?3,?3,?3)",params![id,email,START]).unwrap();
    }
    conn.execute("INSERT INTO market_provider_profiles (id,canonical_email,user_id,display_name,claim_state,status,created_at,updated_at)
        VALUES ('profile','provider@example.test','provider','Provider','claimed','active',?1,?1)",params![START]).unwrap();
    let audit = rental(&conn, 1, "active");
    crate::market_wallet::credit_verified_topup_tx(
        &conn, "buyer", "topup", 10000, "receipt", START,
    )
    .unwrap();
    crate::market_commission::freeze_quote_tx(&conn, "quote", "provider", START).unwrap();
    crate::client_market_termination::freeze_quote_tx(&conn, "quote").unwrap();
    let contract = contracts::prepare_contract_tx(
        &conn,
        &PrepareContract {
            product_kind: "client_host",
            product_ref: "job-0001",
            trial_scope_ref: "host-0001",
            quote_id: "quote",
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: BillingModel::CalendarMonth,
            price_minor: 1000,
            auto_renew: true,
            trial_seconds: trial,
            trial_allowance_seconds: trial,
        },
        START,
    )
    .unwrap();
    contracts::bind_client_installation_tx(
        &conn,
        &contract,
        "job-0001",
        "client-0001",
        "buyer",
        "provider",
        START,
    )
    .unwrap();
    conn.execute("UPDATE client_market_subscriptions SET platform_contract_id=?1,pricing_model='prepaid_calendar_month',cycle_price_minor=1000 WHERE installation_id='client-0001'",params![contract]).unwrap();
    contracts::activate_contract_tx(&conn, &contract, START, START).unwrap();
    (conn, contract, audit)
}

#[test]
fn audit_pages_all_history_with_stable_ties_and_no_resource_dependency() {
    let conn = crate::test_support::database().unwrap();
    for n in 0..125 {
        rental(&conn, n, "released");
    }
    conn.execute("DELETE FROM router_ssh_hosts", []).unwrap();
    let mut all = std::collections::HashSet::new();
    for number in 1..=25 {
        let page = list_tx(
            &conn,
            &AuditQuery {
                page: Some(number),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page["total"], 125);
        assert_eq!(page["page"], number);
        assert_eq!(page["pageSize"], 5);
        assert_eq!(page["totalPages"], 25);
        assert_eq!(page["items"].as_array().unwrap().len(), 5);
        assert_eq!(
            page["items"][0]["id"],
            format!("job:job-{:04}", 124 - (number - 1) * 5)
        );
        for item in page["items"].as_array().unwrap() {
            assert!(all.insert(item["id"].as_str().unwrap().to_string()));
            assert_eq!(item["hostPresent"], false);
            assert_eq!(
                item["hostIp"],
                format!(
                    "192.0.2.{}",
                    item["hostId"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("host-")
                        .parse::<usize>()
                        .unwrap()
                )
            );
        }
    }
    assert_eq!(all.len(), 125);
    assert!(
        list_tx(
            &conn,
            &AuditQuery {
                page: Some(0),
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn audit_numbered_pages_jump_directly_and_normalize_empty_partial_and_shrunk_results() {
    let conn = crate::test_support::database().unwrap();
    let empty = list_tx(
        &conn,
        &AuditQuery {
            page: Some(u32::MAX),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(empty["items"], json!([]));
    assert_eq!(empty["page"], 1);
    assert_eq!(empty["totalPages"], 1);
    assert_eq!(empty["total"], 0);
    for n in 0..23 {
        rental(&conn, n, if n == 0 { "opening" } else { "released" });
    }
    // Neither a cursor nor a visit to an earlier page is required.
    for (number, first, len) in [(3, 12, 5), (5, 2, 3), (u32::MAX, 2, 3), (1, 22, 5)] {
        let page = list_tx(
            &conn,
            &AuditQuery {
                page: Some(number),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page["total"], 23);
        assert_eq!(page["totalPages"], 5);
        assert_eq!(page["page"], number.min(5));
        assert_eq!(page["items"].as_array().unwrap().len(), len);
        assert_eq!(page["items"][0]["id"], format!("job:job-{first:04}"));
    }
    let shrunk = list_tx(
        &conn,
        &AuditQuery {
            page: Some(5),
            status: Some("opening".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(shrunk["page"], 1);
    assert_eq!(shrunk["totalPages"], 1);
    assert_eq!(shrunk["total"], 1);
    assert_eq!(shrunk["items"][0]["id"], "job:job-0000");
    for page in [
        json!(-1),
        json!(1.5),
        json!(4294967296_u64),
        json!("1; DROP TABLE users"),
    ] {
        assert!(serde_json::from_value::<AuditQuery>(json!({ "page": page })).is_err());
    }
    assert!(serde_json::from_value::<AuditQuery>(json!({ "cursor": "retired" })).is_err());
}

#[test]
fn audit_page_query_rejects_malformed_http_values() {
    for (suffix, expected) in [
        ("", None),
        ("?page=1", Some(1)),
        ("?page=4294967295", Some(u32::MAX)),
    ] {
        let uri = format!("/v1/admin/client-market/rental-audit{suffix}")
            .parse()
            .unwrap();
        let Query(query) = Query::<AuditQuery>::try_from_uri(&uri).unwrap();
        assert_eq!(query.page, expected);
    }
    for query in [
        "page=",
        "page=-1",
        "page=1.5",
        "page=1e1",
        "page=4294967296",
        "page=NaN",
        "page=1%3BDROP",
        "page=1&page=2",
        "cursor=retired",
    ] {
        let uri = format!("/v1/admin/client-market/rental-audit?{query}")
            .parse()
            .unwrap();
        assert!(Query::<AuditQuery>::try_from_uri(&uri).is_err(), "{query}");
    }
}

#[test]
fn audit_filters_are_bounded_bound_and_independent_of_general_admin() {
    let conn = crate::test_support::database().unwrap();
    rental(&conn, 1, "active");
    rental(&conn, 2, "activation_failed");
    rental(&conn, 3, "opening");
    for (q, count) in [
        (
            AuditQuery {
                buyer_email: Some("BUYER@example.test".into()),
                ..Default::default()
            },
            3,
        ),
        (
            AuditQuery {
                status: Some("free".into()),
                ..Default::default()
            },
            1,
        ),
        (
            AuditQuery {
                status: Some("activation_failed".into()),
                ..Default::default()
            },
            1,
        ),
        (
            AuditQuery {
                search: Some("192.0.2.1".into()),
                ..Default::default()
            },
            1,
        ),
        (
            AuditQuery {
                search: Some("%' OR 1=1 --".into()),
                ..Default::default()
            },
            0,
        ),
        (
            AuditQuery {
                from: Some("2026-01-01T00:00:00Z".into()),
                until: Some("2026-01-02T00:00:00Z".into()),
                ..Default::default()
            },
            3,
        ),
    ] {
        assert_eq!(list_tx(&conn, &q).unwrap()["total"], count);
    }
    for q in [
        AuditQuery {
            status: Some("bogus".into()),
            ..Default::default()
        },
        AuditQuery {
            search: Some("x".repeat(255)),
            ..Default::default()
        },
        AuditQuery {
            from: Some("not-a-date".into()),
            ..Default::default()
        },
        AuditQuery {
            attention: Some("bogus".into()),
            ..Default::default()
        },
        AuditQuery {
            from: Some(START.into()),
            until: Some(START.into()),
            ..Default::default()
        },
    ] {
        assert!(list_tx(&conn, &q).is_err());
    }
    let mut config = crate::config::Config::from_env();
    config.router_owner_email = Some("owner@example.test".into());
    config.admin_emails.clear();
    config.admin_emails.insert("admin@example.test".into());
    assert!(crate::api::is_finance_owner(
        &config,
        " OWNER@example.test "
    ));
    assert!(!config.is_admin("owner@example.test"));
    assert!(!crate::api::is_finance_owner(&config, "admin@example.test"));
    assert!(!crate::api::is_finance_owner(&config, "buyer@example.test"));
    assert_eq!(no_store()[0].1, "private, no-store");
}

#[test]
fn audit_identity_is_immutable_and_never_borrows_a_new_tenants_identity() {
    let conn = crate::test_support::database().unwrap();
    let audit = rental(&conn, 1, "released");
    conn.execute("UPDATE router_ssh_hosts SET provider_id='new-provider',host_owner_email='new@example.test',ip='192.0.2.99',installation_id='new-client' WHERE id='host-0001'",[]).unwrap();
    assert!(
        conn.execute(
            "UPDATE client_market_rental_identities SET buyer_email='other@example.test'",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute("DELETE FROM client_market_rental_identities", [])
            .is_err()
    );
    assert!(bind_installation_tx(&conn, "job-0001", "new-client").is_err());
    let value = detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    let r = &value["record"];
    assert_eq!(r["providerEmail"], "provider@example.test");
    assert_eq!(r["hostIp"], "192.0.2.1");
    assert_eq!(r["installationId"], "client-0001");
    assert!(!value.to_string().contains("SECRET_LOG"));
    assert!(!value.to_string().contains("new@example.test"));
}

#[test]
fn audit_uses_contract_trial_state_and_never_counts_a_reservation_as_payment() {
    let (conn, contract, audit) = paid(3600);
    let value = detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    assert_eq!(value["record"]["status"], "trialing");
    assert_eq!(value["record"]["trialSecondsRemaining"], 3600);
    assert_eq!(value["record"]["money"]["chargedUnits"], "0");
    assert_eq!(value["periods"], json!([]));
    conn.execute(
        "UPDATE market_platform_contracts SET status='suspended' WHERE id=?1",
        params![contract],
    )
    .unwrap();
    assert_eq!(
        list_tx(&conn, &Default::default()).unwrap()["items"][0]["status"],
        "paused"
    );
}

#[test]
fn audit_aggregates_money_exactly_once_and_reads_frozen_period_terms() {
    let (conn, contract, audit) = paid(0);
    let end = "2026-01-02T00:00:00Z";
    contracts::settle_monthly_segment_tx(&conn, &contract, "service-1", START, end, true, end)
        .unwrap();
    let first = detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    assert_eq!(first["record"]["status"], "paid");
    assert_eq!(
        first["record"]["money"]["chargedUnits"],
        units_from_minor(1000).unwrap().to_string()
    );
    let money = &first["record"]["money"];
    let n = |field: &str| money[field].as_str().unwrap().parse::<i64>().unwrap();
    assert!(n("serviceUnits") > 0 && n("platformFeeUnits") > 0);
    assert_eq!(
        n("serviceUnits"),
        n("providerNetUnits") + n("platformFeeUnits")
    );
    assert_eq!(first["periods"][0]["terminationRateBps"], 1000);
    assert_eq!(first["periods"][0]["commissionRateBps"], 1000);
    let before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market_journal_transactions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    list_tx(&conn, &Default::default()).unwrap();
    assert_eq!(
        before,
        conn.query_row(
            "SELECT COUNT(*) FROM market_journal_transactions",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
    );
    assert!(
        detail::detail_tx(
            &conn,
            &audit,
            &detail::DetailQuery {
                period_cursor: Some(99),
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        detail::detail_tx(
            &conn,
            &audit,
            &detail::DetailQuery {
                event_cursor: Some("other-rental".into()),
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn audit_event_history_pages_without_raw_logs_or_duplicate_mirrors() {
    let conn = crate::test_support::database().unwrap();
    let audit = rental(&conn, 1, "released");
    for n in 0..120 {
        conn.execute("INSERT INTO client_market_subscription_events (id,installation_id,event_type,detail_json,created_at)
            VALUES (?1,'client-0001','cleanup_started','{\"password\":\"NEVER_EXPOSE\"}',?2)",params![format!("event-{n:04}"),START]).unwrap();
    }
    let mut q = detail::DetailQuery::default();
    let mut seen = std::collections::HashSet::new();
    loop {
        let page = detail::detail_tx(&conn, &audit, &q).unwrap();
        assert!(!page.to_string().contains("NEVER_EXPOSE"));
        for e in page["events"].as_array().unwrap() {
            assert!(seen.insert(e["id"].as_str().unwrap().to_string()));
        }
        q.event_cursor = page["eventsNextCursor"].as_str().map(str::to_string);
        if q.event_cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 121);
}

#[test]
fn an_original_job_can_acquire_a_host_without_inventing_historical_ip() {
    let conn = crate::test_support::database().unwrap();
    rental(&conn, 1, "active");
    conn.execute(
        "INSERT INTO provisioning_jobs (id,type,client_owner_email,status,created_at,updated_at)
        VALUES ('previously-unassigned','create','buyer@example.test','pending',?1,?1)",
        params![START],
    )
    .unwrap();
    capture_job_identity_tx(&conn, "previously-unassigned").unwrap();
    conn.execute(
        "UPDATE provisioning_jobs SET host_id='host-0001' WHERE id='previously-unassigned'",
        [],
    )
    .unwrap();
    let page = detail::detail_tx(&conn, "job:previously-unassigned", &Default::default()).unwrap();
    assert_eq!(page["record"]["hostId"], "host-0001");
    assert_eq!(page["record"]["hostPresent"], true);
    assert!(page["record"]["hostIp"].is_null());
    assert!(page["record"]["money"].is_null());
    conn.execute("DELETE FROM router_ssh_hosts WHERE id='host-0001'", [])
        .unwrap();
    assert_eq!(
        detail::detail_tx(&conn, "job:previously-unassigned", &Default::default()).unwrap()["record"]
            ["hostId"],
        "host-0001"
    );
}

#[test]
fn failed_delivery_is_not_a_successfully_completed_rental() {
    let (conn, contract, audit) = paid(3600);
    conn.execute(
        "DELETE FROM client_market_subscriptions WHERE installation_id='client-0001'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE provisioning_jobs SET status='failed' WHERE id='job-0001'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE market_platform_contracts SET status='ended',ended_at=?2 WHERE id=?1",
        params![contract, START],
    )
    .unwrap();
    let record = detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    assert_eq!(record["record"]["status"], "activation_failed");
    assert_eq!(record["record"]["money"]["chargedUnits"], "0");
}

fn accept_voluntary_exit(conn: &Connection, at: &str, reason: &str) {
    conn.execute("INSERT INTO client_termination_previews
        SELECT 'audit-preview',s.installation_id,s.client_user_id,p.id,COALESCE(t.policy_version,0),'2026-02-01T00:00:00Z'
        FROM client_market_subscriptions s JOIN market_platform_contracts c ON c.id=s.platform_contract_id
        LEFT JOIN market_platform_periods p ON p.contract_id=c.id AND p.sequence=c.current_period_sequence
        LEFT JOIN client_termination_period_terms t ON t.period_id=p.id WHERE s.installation_id='client-0001'", []).unwrap();
    crate::client_market_termination::accept_tx(
        conn,
        "client-0001",
        Some("buyer"),
        "client_release",
        Some(&crate::client_market_termination::TerminationRequest {
            preview_id: "audit-preview".into(),
            reason: reason.into(),
            claim: String::new(),
        }),
        at,
    )
    .unwrap();
}

#[test]
fn termination_audit_separates_cleanup_returns_compensation_and_matured_credit() {
    let (conn, contract, audit) = paid(0);
    let at = "2026-01-02T00:00:00Z";
    contracts::settle_monthly_segment_tx(&conn, &contract, "delivered", START, at, true, at)
        .unwrap();
    conn.execute(
        "UPDATE market_platform_contracts SET last_evaluated_at=?2 WHERE id=?1",
        params![contract, at],
    )
    .unwrap();
    accept_voluntary_exit(&conn, at, "voluntary");
    let pending = detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    assert_eq!(pending["record"]["status"], "ended");
    assert_eq!(pending["record"]["cleanupStatus"], "pending");
    assert_eq!(pending["record"]["money"]["returnedUnits"], "0");
    crate::client_market_termination::cleanup_failed_tx(&conn, "client-0001", at).unwrap();
    assert_eq!(
        detail::detail_tx(&conn, &audit, &Default::default()).unwrap()["record"]["attention"],
        "cleanup_failed"
    );
    crate::client_market_termination::record_cleanup_confirmed_tx(
        &conn,
        "client-0001",
        "host-0001",
        at,
    )
    .unwrap();
    let settled = detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    assert_eq!(settled["record"]["cleanupStatus"], "verified");
    assert_eq!(settled["record"]["attention"], "none");
    let money = &settled["record"]["money"];
    let n = |key: &str| money[key].as_str().unwrap().parse::<i64>().unwrap();
    assert_eq!(
        n("chargedUnits"),
        n("serviceUnits") + n("returnedUnits") + n("compensationUnits")
    );
    assert_eq!(
        n("serviceUnits"),
        n("providerNetUnits") + n("platformFeeUnits")
    );
    assert_eq!(n("compensationUnits"), units_from_minor(100).unwrap());
    assert_eq!(n("unfulfilledUnits"), 0);
    assert_eq!(n("compensationCreditedUnits"), 0);
    assert!(
        settled["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "returned")
    );
    assert_eq!(
        crate::client_market_termination::credit_due_isolated(&conn, "2026-01-10T00:00:00Z", 10)
            .unwrap(),
        1
    );
    let mature = detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    assert_eq!(
        mature["record"]["money"]["compensationUnits"],
        money["compensationUnits"]
    );
    assert_eq!(
        mature["record"]["money"]["compensationCreditedUnits"],
        money["compensationUnits"]
    );
    assert!(
        mature["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "credited")
    );
}

#[test]
fn a_disputed_hold_is_a_subset_of_remaining_rent_not_an_extra_debit() {
    let (conn, _, audit) = paid(0);
    accept_voluntary_exit(&conn, START, "provider_issue");
    crate::client_market_termination::record_cleanup_confirmed_tx(
        &conn,
        "client-0001",
        "host-0001",
        START,
    )
    .unwrap();
    let page = detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    assert_eq!(page["record"]["attention"], "waiting_provider");
    let money = &page["record"]["money"];
    assert_eq!(money["compensationUnits"], "0");
    assert_eq!(money["unfulfilledUnits"], page["settlement"]["heldUnits"]);
    assert_eq!(
        money["unfulfilledUnits"],
        units_from_minor(100).unwrap().to_string()
    );
    let total = money["returnedUnits"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap()
        + money["unfulfilledUnits"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap();
    assert_eq!(total, units_from_minor(1000).unwrap());
}

#[test]
fn many_renewals_stay_in_one_rental_with_independent_period_pagination() {
    let (conn, contract, audit) = paid(0);
    crate::market_wallet::credit_verified_topup_tx(
        &conn,
        "buyer",
        "renewal-funding",
        100000,
        "renewal-receipt",
        START,
    )
    .unwrap();
    for n in 1..55 {
        let (start, end): (String, String) = conn.query_row(
            "SELECT period_start,period_end FROM market_platform_periods WHERE contract_id=?1 AND sequence=?2",
            params![contract, n], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        contracts::settle_monthly_segment_tx(
            &conn,
            &contract,
            &format!("period-{n}"),
            &start,
            &end,
            true,
            &end,
        )
        .unwrap();
        assert!(contracts::renew_monthly_contract_tx(&conn, &contract, &end).unwrap());
    }
    let list = list_tx(&conn, &Default::default()).unwrap();
    assert_eq!(list["total"], 1);
    assert_eq!(
        list["items"][0]["money"]["chargedUnits"],
        units_from_minor(55000).unwrap().to_string()
    );
    let first = detail::detail_tx(&conn, &audit, &Default::default()).unwrap();
    assert_eq!(first["periods"].as_array().unwrap().len(), 50);
    assert_eq!(first["periods"][0]["sequence"], 55);
    let last = detail::detail_tx(
        &conn,
        &audit,
        &detail::DetailQuery {
            period_cursor: first["periodsNextCursor"].as_i64(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(last["periods"].as_array().unwrap().len(), 5);
    assert_eq!(last["periods"][4]["sequence"], 1);
    assert!(last["periodsNextCursor"].is_null());
}

#[test]
fn audit_lifecycle_uses_indexed_job_lookups_before_page_scoped_money() {
    let conn = crate::test_support::database().unwrap();
    rental(&conn, 1, "released");
    let mut statement = conn
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {FACTS}SELECT {COLUMNS} FROM records
         WHERE attention!='none' ORDER BY created_at DESC,id DESC LIMIT 5 OFFSET 10"
        ))
        .unwrap();
    let plan = statement
        .query_map([], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<crate::db::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(plan.contains("idx_client_rental_audit_jobs"), "{plan}");
    assert!(!plan.contains("SCAN cleanup"), "{plan}");
    // Amount sources must only be accessed by attach_money, after LIMIT 5.
    for source in [
        "market_delivery_slices",
        "market_platform_periods",
        "market_provider_earning_lots",
        "market_usage_posting_batches",
    ] {
        assert!(
            !plan.contains(source),
            "premature global monetary aggregation: {plan}"
        );
    }
}
