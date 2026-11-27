use super::*;
use crate::market_platform_contracts::{self as contracts, BillingModel, PrepareContract};
use crate::market_wallet::units_from_minor;
// Offset form on purpose: stored rows use `to_rfc3339()`, the audit emits Z.
const START: &str = "2026-01-01T00:00:00+00:00";
const START_Z: &str = "2026-01-01T00:00:00.000Z";

/// One committed Share rental with its own Share, listing and seat.
pub(super) fn rental(conn: &Connection, n: usize, status: &str) -> String {
    let share = format!("share-{n:04}");
    let listing = format!("listing-{n:04}");
    let seat = format!("seat-{n:04}");
    let sub = format!("sub-{n:04}");
    conn.execute("INSERT INTO shares(share_id,capacity_pool_id,installation_id,share_name,owner_email,subdomain,app_type,token_limit,tokens_used,requests_count,share_status,created_at,expires_at,updated_at,termination_archive_since_ms)
        VALUES(?1,'pool','installation',?2,'provider@example.test',?1,'codex',-1,0,0,'active',?3,'9999-12-31T00:00:00Z',?3,0)",
        params![share, format!("Share {n}"), START]).unwrap();
    conn.execute("INSERT INTO share_market_listings(id,share_id,installation_id,owner_user_id,owner_email,status,created_at,updated_at)
        VALUES(?1,?2,'installation','provider','provider@example.test','active',?3,?3)",params![listing,share,START]).unwrap();
    conn.execute("INSERT INTO share_market_seats(id,listing_id,position,status,token_limit,token_period_json,current_subscription_id,created_at,updated_at)
        VALUES(?1,?2,?3,'occupied',100,'\"lifetime\"',?4,?5,?5)",params![seat,listing,(n%7+1) as i64,sub,START]).unwrap();
    conn.execute("INSERT INTO share_market_subscriptions(id,seat_id,listing_id,share_id,installation_id,entitlement_id,owner_user_id,owner_email,renter_user_id,renter_email,status,token_limit,token_period_json,offer_revision,created_at,updated_at,activated_at,service_started_at,released_at,pricing_model,required_app)
        VALUES(?1,?2,?3,?4,'installation',?5,'provider','provider@example.test','buyer','buyer@example.test',?6,100,'\"lifetime\"',1,?7,?7,
        CASE WHEN ?6 NOT IN ('grant_pending','grant_failed') THEN ?7 END,CASE WHEN ?6 NOT IN ('grant_pending','grant_failed') THEN ?7 END,
        CASE WHEN ?6 IN ('released','grant_failed') THEN ?7 END,'free','codex')",
        params![sub,seat,listing,share,format!("entitlement-{n:04}"),status,START]).unwrap();
    sub
}

/// A platform-wallet prepaid Share rental; `trial` seconds of contract trial.
pub(super) fn paid(trial: i64) -> (Connection, String, String) {
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
        conn.execute("INSERT INTO users(id,email_normalized,status,email_verified_at,created_at,last_login_at) VALUES(?1,?2,'active',?3,?3,?3)",params![id,email,START]).unwrap();
    }
    conn.execute("INSERT INTO market_provider_profiles(id,canonical_email,user_id,display_name,claim_state,status,created_at,updated_at)
        VALUES('profile','provider@example.test','provider','Provider','claimed','active',?1,?1)",params![START]).unwrap();
    let sub = rental(&conn, 1, "active_prepaid");
    conn.execute(
        "UPDATE share_market_subscriptions SET pricing_model='prepaid_calendar_month',cycle_price_minor=10000 WHERE id=?1",
        params![sub],
    )
    .unwrap();
    crate::market_wallet::credit_verified_topup_tx(
        &conn, "buyer", "topup", 10000, "receipt", START,
    )
    .unwrap();
    crate::market_commission::freeze_quote_tx(&conn, "quote", "provider", START).unwrap();
    let policy = crate::share_market_termination::policy_tx(&conn).unwrap();
    crate::share_market_termination::freeze_quote_tx(&conn, "quote", Some(&policy)).unwrap();
    let contract = contracts::prepare_contract_tx(
        &conn,
        &PrepareContract {
            trial_only: false,
            product_kind: "share",
            product_ref: &sub,
            trial_scope_ref: "share-0001",
            quote_id: "quote",
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: BillingModel::CalendarMonth,
            price_minor: 10000,
            auto_renew: true,
            trial_seconds: trial,
            trial_allowance_seconds: trial,
        },
        START,
    )
    .unwrap();
    if trial > 0 {
        conn.execute("INSERT INTO market_share_trial_scopes(id,buyer_user_id,provider_user_id,share_id,unit) VALUES('scope','buyer','provider','share-0001','tokens')",[]).unwrap();
        conn.execute("INSERT INTO market_share_trial_contracts(contract_id,scope_id,entitlement_id,token_allowance,token_budget,created_at,updated_at)
            VALUES(?1,'scope','entitlement-0001',5000,4000,?2,?2)",params![contract,START]).unwrap();
    }
    contracts::activate_contract_tx(&conn, &contract, START, START).unwrap();
    (conn, contract, sub)
}

fn page(conn: &Connection, q: AuditQuery) -> Value {
    list_tx(conn, &q).unwrap()
}

fn units(value: &Value, key: &str) -> i64 {
    value[key].as_str().unwrap().parse().unwrap()
}

/// A termination row exactly as the CHECK constraint requires.
fn terminate(conn: &Connection, contract: &str, status: &str, responsibility: &str) {
    conn.execute("INSERT INTO share_market_terminations(id,subscription_id,share_id,contract_id,period_id,buyer_user_id,provider_user_id,status,cutoff_at,updated_at,version,payload_json,entitlement_id)
        VALUES('term','sub-0001','share-0001',?1,NULL,'buyer','provider',?2,?3,?3,1,
        json_object('id','term','subscriptionId','sub-0001','shareId','share-0001','entitlementId','entitlement-0001',
          'buyerUserId','buyer','providerUserId','provider','contractId',?1,'periodId',NULL,'cutoffAt',?3,'version',1,
          'status',?2,'heldUnits',0,'returnedUnits',0,'compensationUnits',0,'responsibility',?4,'reason','voluntary',
          'shareName','Share 1','subdomain','share-0001','buyerEmail','buyer@example.test','providerEmail','provider@example.test'),
        'entitlement-0001')",params![contract,status,START,responsibility]).unwrap();
}

fn set_termination(conn: &Connection, status: &str) {
    conn.execute(
        "UPDATE share_market_terminations SET status=?1,payload_json=json_set(payload_json,'$.status',?1) WHERE id='term'",
        params![status],
    )
    .unwrap();
}

#[test]
fn audit_pages_jump_directly_and_clamp_empty_partial_and_shrunk_results() {
    let conn = crate::test_support::database().unwrap();
    let empty = page(
        &conn,
        AuditQuery {
            page: Some(u32::MAX),
            ..Default::default()
        },
    );
    assert_eq!(empty["items"], json!([]));
    assert_eq!(
        (&empty["page"], &empty["totalPages"], &empty["total"]),
        (&json!(1), &json!(1), &json!(0))
    );
    for n in 0..23 {
        rental(&conn, n, if n == 0 { "grant_pending" } else { "released" });
    }
    let mut seen = std::collections::HashSet::new();
    for number in 1..=5 {
        let p = page(
            &conn,
            AuditQuery {
                page: Some(number),
                ..Default::default()
            },
        );
        assert_eq!(p["pageSize"], 5);
        for item in p["items"].as_array().unwrap() {
            assert!(seen.insert(item["id"].as_str().unwrap().to_string()));
            assert_eq!(item["createdAt"], START_Z);
        }
    }
    assert_eq!(seen.len(), 23);
    // Same timestamp everywhere: ties break by ID descending.
    for (number, first, len) in [(3, 12, 5), (5, 2, 3), (u32::MAX, 2, 3), (1, 22, 5)] {
        let p = page(
            &conn,
            AuditQuery {
                page: Some(number),
                ..Default::default()
            },
        );
        assert_eq!(p["total"], 23);
        assert_eq!(p["totalPages"], 5);
        assert_eq!(p["page"], number.min(5));
        assert_eq!(p["items"].as_array().unwrap().len(), len);
        assert_eq!(p["items"][0]["id"], format!("sub-{first:04}"));
    }
    let shrunk = page(
        &conn,
        AuditQuery {
            page: Some(5),
            status: Some("opening".into()),
            ..Default::default()
        },
    );
    assert_eq!(shrunk["page"], 1);
    assert_eq!(shrunk["total"], 1);
    assert_eq!(shrunk["items"][0]["id"], "sub-0000");
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
    for value in [json!(-1), json!(1.5), json!(4294967296_u64), json!("1; DROP")] {
        assert!(serde_json::from_value::<AuditQuery>(json!({ "page": value })).is_err());
    }
    assert!(serde_json::from_value::<AuditQuery>(json!({ "cursor": "x" })).is_err());
    for query in ["page=", "page=-1", "page=1.5", "page=1e1", "page=4294967296", "page=1&page=2", "cursor=x"] {
        let uri = format!("/v1/admin/share-market/rental-audit?{query}")
            .parse()
            .unwrap();
        assert!(Query::<AuditQuery>::try_from_uri(&uri).is_err(), "{query}");
    }
}

#[test]
fn audit_filters_are_validated_bound_and_owner_only() {
    let conn = crate::test_support::database().unwrap();
    rental(&conn, 1, "active_free");
    rental(&conn, 2, "grant_failed");
    rental(&conn, 3, "grant_pending");
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
                status: Some("grant_failed".into()),
                ..Default::default()
            },
            1,
        ),
        (
            AuditQuery {
                search: Some("Share 2".into()),
                ..Default::default()
            },
            1,
        ),
        (
            AuditQuery {
                search: Some("seat-000".into()),
                ..Default::default()
            },
            3,
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
                search: Some("sub_".into()),
                ..Default::default()
            },
            0,
        ),
        (
            AuditQuery {
                attention: Some("any".into()),
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
    let free = &page(
        &conn,
        AuditQuery {
            status: Some("free".into()),
            ..Default::default()
        },
    )["items"][0];
    assert_eq!(free["money"]["chargedUnits"], "0");
    for q in [
        AuditQuery {
            status: Some("bogus".into()),
            ..Default::default()
        },
        AuditQuery {
            attention: Some("bogus".into()),
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
    assert!(crate::api::is_finance_owner(&config, " OWNER@example.test "));
    assert!(!crate::api::is_finance_owner(&config, "admin@example.test"));
    assert!(!crate::api::is_finance_owner(&config, "buyer@example.test"));
    assert_eq!(no_store()[0].1, "private, no-store");
}

#[test]
fn snapshot_survives_share_deletion_rename_and_owner_transfer() {
    let conn = crate::test_support::database().unwrap();
    let sub = rental(&conn, 1, "released");
    conn.execute_batch(
        "UPDATE shares SET share_name='Renamed',subdomain='renamed',app_type='claude';
         UPDATE share_market_subscriptions SET owner_email='new@example.test';
         DELETE FROM shares;",
    )
    .unwrap();
    assert!(
        conn.execute(
            "UPDATE share_market_rental_identities SET buyer_email='other@example.test'",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute("DELETE FROM share_market_rental_identities", [])
            .is_err()
    );
    let value = detail::detail_tx(&conn, &sub, &Default::default()).unwrap();
    let r = &value["record"];
    assert_eq!(r["provenance"], "order_snapshot");
    assert_eq!(r["shareName"], "Share 1");
    assert_eq!(r["shareSubdomain"], "share-0001");
    assert_eq!(r["appType"], "codex");
    assert_eq!(r["seatId"], "seat-0001");
    assert_eq!(r["seatLabel"], "2");
    assert_eq!(r["installationId"], "installation");
    assert_eq!(r["providerEmail"], "provider@example.test");
    assert_eq!(r["sharePresent"], false);
    assert_eq!(r["status"], "ended");
    assert_eq!(r["endedAt"], START_Z);
    // Legacy pricing without a contract: free is provable, paid would be null.
    assert_eq!(r["money"]["chargedUnits"], "0");
    assert!(!value.to_string().contains("new@example.test"));
    assert!(!value.to_string().contains("Renamed"));
    conn.execute(
        "UPDATE share_market_subscriptions SET pricing_model='legacy_metered_daily',daily_rate_minor=100",
        [],
    )
    .unwrap();
    let legacy = detail::detail_tx(&conn, &sub, &Default::default()).unwrap();
    assert!(legacy["record"]["money"].is_null());
}

#[test]
fn contract_decides_trial_versus_paid_and_reservation_is_not_payment() {
    let (conn, contract, sub) = paid(3600);
    let value = detail::detail_tx(&conn, &sub, &Default::default()).unwrap();
    assert_eq!(value["record"]["status"], "trialing");
    assert_eq!(value["record"]["money"]["chargedUnits"], "0");
    assert_eq!(value["periods"], json!([]));
    assert_eq!(value["trial"]["phase"], "trial");
    assert_eq!(value["trial"]["tokenAllowance"], "5000");
    assert_eq!(value["trial"]["tokenBudget"], "4000");
    assert!(value["trial"]["valueAllowanceUsdMicros"].is_null());
    conn.execute(
        "UPDATE market_platform_contracts SET status='suspended' WHERE id=?1",
        params![contract],
    )
    .unwrap();
    assert_eq!(
        page(&conn, Default::default())["items"][0]["status"],
        "paused"
    );
    let (conn, _, sub) = paid(0);
    let value = detail::detail_tx(&conn, &sub, &Default::default()).unwrap();
    assert_eq!(value["record"]["status"], "paid");
    assert!(value["trial"].is_null());
    assert_eq!(
        value["record"]["money"]["chargedUnits"],
        units_from_minor(10000).unwrap().to_string()
    );
    assert_eq!(value["periods"][0]["terminationRateBps"], 1000);
}

#[test]
fn termination_and_revoke_states_drive_attention() {
    let (conn, contract, sub) = paid(0);
    conn.execute("INSERT INTO share_control_operations(id,share_id,share_sequence,entitlement_id,subscription_id,action,email,status,created_at,updated_at)
        VALUES('op-1','share-0001',1,'entitlement-0001',?1,'revoke','buyer@example.test','rejected',?2,?2)",params![sub,START]).unwrap();
    let get = || detail::detail_tx(&conn, &sub, &Default::default()).unwrap();
    assert_eq!(get()["record"]["attention"], "revoke_failed");
    conn.execute("INSERT INTO share_control_operations(id,share_id,share_sequence,entitlement_id,subscription_id,action,email,status,created_at,updated_at)
        VALUES('op-2','share-0001',2,'entitlement-0001',?1,'revoke','buyer@example.test','pending',?2,?2)",params![sub,START]).unwrap();
    assert_eq!(get()["record"]["attention"], "revoke_pending");
    terminate(&conn, &contract, "revoke_pending", "buyer");
    let pending = get();
    assert_eq!(pending["record"]["status"], "ending");
    assert_eq!(pending["record"]["terminationId"], "term");
    assert_eq!(pending["settlement"]["status"], "revoke_pending");
    assert_eq!(pending["settlement"]["cutoffAt"], START_Z);
    assert_eq!(pending["settlement"]["heldUnits"], "0");
    conn.execute(
        "UPDATE share_market_subscriptions SET status='released',released_at=?2 WHERE id=?1",
        params![sub, START],
    )
    .unwrap();
    set_termination(&conn, "finance_pending");
    assert_eq!(get()["record"]["attention"], "finance_review");
    set_termination(&conn, "disputed");
    assert_eq!(get()["record"]["attention"], "disputed");
    set_termination(&conn, "completed");
    let settled = get();
    assert_eq!(settled["record"]["status"], "ended");
    assert_eq!(settled["record"]["attention"], "settlement_pending");
    conn.execute(
        "UPDATE market_platform_contracts SET status='ended',ended_at=?2 WHERE id=?1",
        params![contract, START],
    )
    .unwrap();
    assert_eq!(get()["record"]["attention"], "none");
    assert_eq!(
        page(
            &conn,
            AuditQuery {
                attention: Some("none".into()),
                status: Some("ended".into()),
                ..Default::default()
            }
        )["total"],
        1
    );
}

#[test]
fn money_is_aggregated_exactly_once_across_slices_periods_and_awards() {
    let (conn, contract, sub) = paid(0);
    let mid = "2026-01-02T00:00:00+00:00";
    let end = "2026-01-03T00:00:00+00:00";
    contracts::settle_monthly_segment_tx(&conn, &contract, "service-1", START, mid, true, mid)
        .unwrap();
    contracts::settle_monthly_segment_tx(&conn, &contract, "service-2", mid, end, true, end)
        .unwrap();
    terminate(&conn, &contract, "completed", "buyer");
    let journal: String = conn
        .query_row("SELECT id FROM market_journal_transactions LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    conn.execute("INSERT INTO share_termination_income(id,termination_id,provider_user_id,kind,amount_units,commission_units,awarded_at,unlock_at,journal_id)
        VALUES('term:share_termination','term','provider','share_termination',?1,0,?2,?2,?3)",params![units_from_minor(100).unwrap(),START,journal]).unwrap();
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM market_journal_transactions", [], |r| {
            r.get(0)
        })
        .unwrap();
    let value = detail::detail_tx(&conn, &sub, &Default::default()).unwrap();
    let money = &value["record"]["money"];
    assert_eq!(units(money, "chargedUnits"), units_from_minor(10000).unwrap());
    assert!(units(money, "serviceUnits") > 0 && units(money, "platformFeeUnits") > 0);
    assert_eq!(
        units(money, "serviceUnits"),
        units(money, "providerNetUnits") + units(money, "platformFeeUnits")
    );
    assert_eq!(
        units(money, "chargedUnits"),
        units(money, "serviceUnits") + units(money, "unfulfilledUnits")
    );
    assert_eq!(
        units(money, "compensationUnits"),
        units_from_minor(100).unwrap()
    );
    assert_eq!(money["compensationCreditedUnits"], "0");
    assert_eq!(value["periods"].as_array().unwrap().len(), 1);
    assert_eq!(value["periods"][0]["serviceUnits"], money["serviceUnits"]);
    let listed = page(&conn, Default::default());
    assert_eq!(&listed["items"][0]["money"], money);
    assert_eq!(
        before,
        conn.query_row("SELECT COUNT(*) FROM market_journal_transactions", [], |r| r
            .get::<_, i64>(0))
            .unwrap()
    );
    for q in [
        detail::DetailQuery {
            period_cursor: Some(99),
            ..Default::default()
        },
        detail::DetailQuery {
            event_cursor: Some("other-rental".into()),
            ..Default::default()
        },
    ] {
        assert!(detail::detail_tx(&conn, &sub, &q).is_err());
    }
}

#[test]
fn event_history_pages_without_raw_detail() {
    let conn = crate::test_support::database().unwrap();
    let sub = rental(&conn, 1, "released");
    for n in 0..60 {
        conn.execute("INSERT INTO share_market_events(id,share_id,installation_id,subscription_id,actor_email,event_type,dedupe_key,detail_json,created_at)
            VALUES(?1,'share-0001','installation',?2,'buyer@example.test',?3,?1,'{\"token\":\"NEVER_EXPOSE\"}',?4)",
            params![format!("event-{n:04}"),sub,if n == 0 { "secret_internal_kind" } else { "seat_rented" },START]).unwrap();
    }
    let mut q = detail::DetailQuery::default();
    let mut seen = std::collections::HashSet::new();
    loop {
        let value = detail::detail_tx(&conn, &sub, &q).unwrap();
        assert!(!value.to_string().contains("NEVER_EXPOSE"));
        assert!(!value.to_string().contains("secret_internal_kind"));
        for e in value["events"].as_array().unwrap() {
            assert!(seen.insert(e["id"].as_str().unwrap().to_string()));
        }
        q.event_cursor = value["eventsNextCursor"].as_str().map(str::to_string);
        if q.event_cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 61);
    assert!(seen.contains(&format!("order:{sub}")));
}

#[test]
fn lifecycle_reads_no_monetary_source_before_pagination() {
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
    for source in [
        "market_platform_periods",
        "market_delivery_slices",
        "market_provider_earning_lots",
        "market_usage_posting_batches",
        "share_termination_income",
    ] {
        assert!(!plan.contains(source), "premature aggregation: {plan}");
    }
    assert!(!FACTS.contains("SUM("));
    assert!(!plan.contains("SCAN o"), "{plan}");
}
