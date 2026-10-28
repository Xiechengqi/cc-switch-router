use super::*;
use crate::market_platform_contracts::{self as contracts, BillingModel, PrepareContract};
use crate::market_wallet::{units_from_minor, wallet_snapshot_tx};
const START: &str = "2026-01-01T00:00:00+00:00";
const INSTALL: &str = "client-installation";
const HOST: &str = "client-host";

fn fixture(price: i64, terms: bool, trial: i64) -> (Connection, String) {
    fixture_with_rate(price, terms, trial, 1000)
}
fn fixture_with_rate(price: i64, terms: bool, trial: i64, rate: i64) -> (Connection, String) {
    let conn = crate::test_support::database().unwrap();
    if rate != 1000 {
        conn.execute(
            "INSERT INTO client_termination_policies VALUES (2,?1,'owner',?2)",
            params![rate, START],
        )
        .unwrap();
    }
    conn.execute(
        "UPDATE market_finance_control SET mode='enabled',cutover_approved_at=?1",
        params![START],
    )
    .unwrap();
    for (id, email) in [
        ("buyer", "buyer@example.test"),
        ("provider", "provider@example.test"),
    ] {
        conn.execute("INSERT INTO users (id,email_normalized,status,email_verified_at,created_at,last_login_at)
            VALUES (?1,?2,'active',?3,?3,?3)",params![id,email,START]).unwrap();
    }
    conn.execute("INSERT INTO market_provider_profiles (id,canonical_email,user_id,display_name,claim_state,status,created_at,updated_at)
        VALUES ('profile','provider@example.test','provider','Provider','claimed','active',?1,?1)",params![START]).unwrap();
    crate::market_wallet::credit_verified_topup_tx(
        &conn, "buyer", "topup", price, "receipt", START,
    )
    .unwrap();
    crate::market_commission::freeze_quote_tx(&conn, "quote", "provider", START).unwrap();
    if terms {
        freeze_quote_tx(&conn, "quote").unwrap();
    }
    let id = contracts::prepare_contract_tx(
        &conn,
        &PrepareContract {
            product_kind: "client_host",
            product_ref: "job",
            trial_scope_ref: HOST,
            quote_id: "quote",
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: BillingModel::CalendarMonth,
            price_minor: price,
            auto_renew: true,
            trial_seconds: trial,
            trial_allowance_seconds: trial,
        },
        START,
    )
    .unwrap();
    contracts::bind_client_installation_tx(&conn, &id, "job", INSTALL, "buyer", "provider", START)
        .unwrap();
    conn.execute("INSERT INTO client_market_subscriptions (installation_id,host_id,provider_id,host_owner_email,
        client_user_id,client_owner_email,status,currency,offer_revision,activated_at,created_at,updated_at,platform_contract_id)
        VALUES (?1,?2,'provider','provider@example.test','buyer','buyer@example.test','active','USD',1,?3,?3,?3,?4)",params![INSTALL,HOST,START,id]).unwrap();
    contracts::activate_contract_tx(&conn, &id, START, START).unwrap();
    (conn, id)
}
fn deliver(conn: &Connection, id: &str, percent: i64) -> String {
    let end: String = conn
        .query_row(
            "SELECT period_end FROM market_platform_periods WHERE contract_id=?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    let start = time(START).unwrap();
    let last = time(&end).unwrap();
    let through =
        (start + Duration::seconds((last - start).num_seconds() * percent / 100)).to_rfc3339();
    if percent > 0 {
        contracts::settle_monthly_segment_tx(
            conn,
            id,
            "delivered",
            START,
            &through,
            true,
            &through,
        )
        .unwrap();
    }
    conn.execute(
        "UPDATE market_platform_contracts SET last_evaluated_at=?2 WHERE id=?1",
        params![id, through],
    )
    .unwrap();
    through
}
fn accept(conn: &Connection, at: &str, reason: &str) -> Termination {
    let p = preview_tx(conn, INSTALL, "buyer", at).unwrap();
    accept_tx(
        conn,
        INSTALL,
        Some("buyer"),
        "client_release",
        Some(&TerminationRequest {
            preview_id: p.preview_id,
            reason: reason.into(),
            claim: if reason == "voluntary" {
                "".into()
            } else {
                "Verifiable outage evidence attached for independent review".into()
            },
        }),
        at,
    )
    .unwrap();
    read_tx(conn, INSTALL).unwrap().unwrap()
}
fn assert_clean(conn: &Connection, at: &str) {
    crate::market_wallet::reconcile_wallet_tx(conn, "buyer").unwrap();
    assert_eq!(
        crate::market_reconcile::audit_finance_tx(conn, at).unwrap(),
        0
    );
}

#[test]
fn precise_compensation_is_capped_and_never_rounded_to_cents() {
    let p = units_from_minor(10000).unwrap();
    assert_eq!(compensation(p, p * 80 / 100, 1000).unwrap(), p / 10);
    assert_eq!(compensation(p, p * 6 / 100, 1000).unwrap(), p * 6 / 100);
    assert_eq!(
        compensation(
            units_from_minor(5).unwrap(),
            units_from_minor(5).unwrap(),
            1000
        )
        .unwrap(),
        43200
    ); // $0.005
    for bps in [0, 1, 1000, 9999, 10000] {
        for r in [0, 1, p / 100, p] {
            let c = compensation(p, r, bps).unwrap();
            assert!((0..=r).contains(&c));
        }
    }
    assert!(compensation(p, p + 1, 1000).is_err());
    assert!(compensation(p, p, 10001).is_err());
}

#[test]
fn issue_explanations_are_optional_but_bounded_in_bytes() {
    for reason in ["provider_issue", "platform_issue"] {
        let (conn, id) = fixture(10000, true, 0);
        let at = deliver(&conn, &id, 20);
        let p = preview_tx(&conn, INSTALL, "buyer", &at).unwrap();
        let mut input = TerminationRequest {
            preview_id: p.preview_id,
            reason: reason.into(),
            claim: "中".repeat(667),
        };
        assert!(matches!(
            accept_tx(
                &conn,
                INSTALL,
                Some("buyer"),
                "client_release",
                Some(&input),
                &at
            ),
            Err(AppError::BadRequest(_))
        ));
        input.claim.clear();
        accept_tx(
            &conn,
            INSTALL,
            Some("buyer"),
            "client_release",
            Some(&input),
            &at,
        )
        .unwrap();
        let t = read_tx(&conn, INSTALL).unwrap().unwrap();
        assert!(t.claim.is_empty() && t.dispute_used && t.chat_available);
        assert_eq!(
            time(t.dispute_deadline_at.as_deref().unwrap()).unwrap(),
            time(&at).unwrap() + Duration::hours(72)
        );
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM market_notification_outbox WHERE event_kind='client_termination_claim'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn no_compensation_issue_does_not_create_a_financial_claim_or_owner_task() {
    for (trial, rate) in [(43200, 1000), (0, 0)] {
        let (conn, _) = fixture_with_rate(5, true, trial, rate);
        let t = accept(&conn, START, "provider_issue");
        cleanup_confirmed_tx(&conn, INSTALL, HOST, START).unwrap();
        let t = by_id_tx(&conn, &t.id).unwrap();
        assert_eq!(t.responsibility, "none");
        assert!(!t.dispute_used && t.dispute_deadline_at.is_none());
        assert_eq!(t.status, "completed");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM market_notification_outbox WHERE event_kind='client_termination_claim'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_clean(&conn, START);
    }
}

#[test]
fn unanswered_claims_waive_exact_remaining_compensation_at_72_hours_once() {
    for (rate, delivered) in [(1000, 20), (725, 20), (1000, 96)] {
        let (conn, id) = fixture_with_rate(10000, true, 0, rate);
        let at = deliver(&conn, &id, delivered);
        let t = accept(&conn, &at, "provider_issue");
        assert_eq!(t.rate_bps, rate);
        cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
        let deadline = (time(&at).unwrap() + Duration::hours(72)).to_rfc3339();
        let early = (time(&deadline).unwrap() - Duration::seconds(1)).to_rfc3339();
        assert_eq!(expire_disputes_isolated(&conn, &early, 50).unwrap(), 0);
        let before = wallet_snapshot_tx(&conn, "buyer").unwrap().available_units;
        assert_eq!(expire_disputes_isolated(&conn, &deadline, 50).unwrap(), 1);
        let result = by_id_tx(&conn, &t.id).unwrap();
        assert_eq!(
            wallet_snapshot_tx(&conn, "buyer").unwrap().available_units - before,
            t.compensation_cap_units
        );
        assert_eq!(result.returned_units, result.remaining_units);
        assert_eq!(result.compensation_units + result.held_units, 0);
        assert_eq!(result.resolution.as_deref(), Some("timeout_waiver"));
        assert_eq!(result.responsibility, "none");
        assert_eq!(expire_disputes_isolated(&conn, &deadline, 50).unwrap(), 0);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM client_termination_events WHERE action='timeout_waiver'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM market_notification_outbox WHERE event_kind='client_termination_settled'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
        assert!(
            withdraw_tx(
                &conn,
                &t.id,
                "buyer",
                &VersionedAction {
                    expected_version: result.version
                },
                &deadline
            )
            .is_err()
        );
        assert_clean(&conn, &deadline);
    }
}

#[test]
fn timeout_before_cleanup_changes_only_terms_and_returns_money_after_verified_cleanup() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    let t = accept(&conn, &at, "platform_issue");
    cleanup_failed_tx(&conn, INSTALL, &at).unwrap();
    let later = (time(&at).unwrap() + Duration::days(3)).to_rfc3339();
    assert_eq!(expire_disputes_isolated(&conn, &later, 50).unwrap(), 1);
    let result = by_id_tx(&conn, &t.id).unwrap();
    assert_eq!(result.status, "cleanup_failed");
    assert_eq!(result.compensation_cap_units, 0);
    assert_eq!(result.returned_units, 0);
    assert_eq!(
        wallet_snapshot_tx(&conn, "buyer").unwrap().available_units,
        0
    );
    assert!(result.cleanup_confirmed_at.is_none());
    for _ in 0..2 {
        record_cleanup_confirmed_tx(&conn, INSTALL, HOST, &later).unwrap();
    }
    let result = by_id_tx(&conn, &t.id).unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(result.returned_units, result.remaining_units);
    assert_clean(&conn, &later);
}

#[test]
fn timeout_reverses_an_awarded_dispute_even_after_its_original_maturity() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    let t = accept(&conn, &at, "voluntary");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = by_id_tx(&conn, &t.id).unwrap();
    let opened = (time(&at).unwrap() + Duration::days(6)).to_rfc3339();
    let t = dispute_tx(
        &conn,
        &t.id,
        "buyer",
        &Dispute {
            expected_version: t.version,
            claim: String::new(),
        },
        &opened,
    )
    .unwrap();
    let later = (time(&opened).unwrap() + Duration::days(3)).to_rfc3339();
    assert_eq!(credit_due_isolated(&conn, &later, 50).unwrap(), 0);
    assert_eq!(expire_disputes_isolated(&conn, &later, 50).unwrap(), 1);
    let result = by_id_tx(&conn, &t.id).unwrap();
    assert_eq!(result.returned_units, result.remaining_units);
    assert_eq!(result.credited_units, 0);
    assert_eq!(
        conn.query_row(
            "SELECT reversed_units FROM client_termination_compensations WHERE id=?1",
            params![t.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        t.compensation_units
    );
    assert_clean(&conn, &later);
}

#[test]
fn formal_provider_response_or_partial_waiver_stops_timeout_but_never_awards_disputed_money() {
    for partial in [false, true] {
        let (conn, id) = fixture(10000, true, 0);
        let at = deliver(&conn, &id, 20);
        let t = accept(&conn, &at, "provider_issue");
        cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
        let t = by_id_tx(&conn, &t.id).unwrap();
        let early = (time(&at).unwrap() + Duration::hours(72) - Duration::seconds(1)).to_rfc3339();
        let result = if partial {
            decide_tx(
                &conn,
                &t.id,
                "provider",
                false,
                &Decision {
                    expected_version: t.version,
                    award_units: (t.held_units / 2).to_string(),
                    responsibility: "buyer".into(),
                    note: "Partial waiver; remaining responsibility contested".into(),
                },
                &early,
            )
            .unwrap()
        } else {
            respond_tx(
                &conn,
                &t.id,
                "provider",
                &ProviderResponse {
                    expected_version: t.version,
                    note: "Disagree; requesting Owner decision".into(),
                },
                &early,
            )
            .unwrap()
        };
        assert_eq!(
            result.provider_responded_at.as_deref(),
            Some(early.as_str())
        );
        assert_eq!(result.dispute_deadline_at, t.dispute_deadline_at);
        assert!(
            conn.execute(
                "UPDATE client_market_terminations SET dispute_opened_at=?2 WHERE id=?1",
                params![t.id, early]
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE client_market_terminations SET provider_responded_at=NULL WHERE id=?1",
                params![t.id]
            )
            .is_err()
        );
        let later = (time(&at).unwrap() + Duration::days(10)).to_rfc3339();
        assert_eq!(expire_disputes_isolated(&conn, &later, 50).unwrap(), 0);
        assert_eq!(credit_due_isolated(&conn, &later, 50).unwrap(), 0);
        assert_eq!(result.status, "disputed");
        assert!(result.held_units > 0);
        assert_eq!(result.compensation_units, 0);
        assert!(
            respond_tx(
                &conn,
                &t.id,
                "provider",
                &ProviderResponse {
                    expected_version: result.version,
                    note: "Repeated response must not extend anything".into()
                },
                &later
            )
            .is_err()
        );
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM market_notification_outbox WHERE event_kind='client_termination_claim'",[],|r|r.get::<_,i64>(0)).unwrap(),4);
        assert_clean(&conn, &later);
    }
}

#[test]
fn financial_recovery_does_not_take_away_the_providers_right_to_respond_without_moving_money() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    let t = accept(&conn, &at, "provider_issue");
    conn.execute(
        "UPDATE market_wallet_accounts SET posted_units=posted_units+1 WHERE user_id='buyer'",
        [],
    )
    .unwrap();
    record_cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = by_id_tx(&conn, &t.id).unwrap();
    assert_eq!(t.status, "finance_pending");
    let response = ProviderResponse {
        expected_version: t.version,
        note: "Disagree; please review the evidence".into(),
    };
    assert!(matches!(
        respond_tx(&conn, &t.id, "buyer", &response, &at),
        Err(AppError::Forbidden(_))
    ));
    assert!(
        respond_tx(
            &conn,
            &t.id,
            "provider",
            &ProviderResponse {
                expected_version: t.version - 1,
                note: response.note.clone()
            },
            &at
        )
        .is_err()
    );
    let t = respond_tx(&conn, &t.id, "provider", &response, &at).unwrap();
    assert_eq!(t.returned_units + t.held_units + t.compensation_units, 0);
    assert!(t.provider_responded_at.is_some());
    conn.execute(
        "UPDATE market_wallet_accounts SET posted_units=posted_units-1 WHERE user_id='buyer'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE client_market_subscriptions SET status='released' WHERE installation_id=?1",
        params![INSTALL],
    )
    .unwrap();
    let later = (time(&at).unwrap() + Duration::days(3)).to_rfc3339();
    assert_eq!(expire_disputes_isolated(&conn, &later, 50).unwrap(), 0);
    assert_eq!(recover_cleanup_isolated(&conn, &later).unwrap(), 1);
    let t = by_id_tx(&conn, &t.id).unwrap();
    assert_eq!(t.status, "disputed");
    assert_eq!(t.held_units, t.compensation_cap_units);
    assert_clean(&conn, &later);
}

#[test]
fn late_provider_owner_or_buyer_actions_cannot_race_away_an_earned_timeout_waiver() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    let t = accept(&conn, &at, "provider_issue");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = by_id_tx(&conn, &t.id).unwrap();
    let later = (time(&at).unwrap() + Duration::days(3)).to_rfc3339();
    assert!(
        respond_tx(
            &conn,
            &t.id,
            "provider",
            &ProviderResponse {
                expected_version: t.version,
                note: "Too late".into()
            },
            &later
        )
        .is_err()
    );
    assert!(
        withdraw_tx(
            &conn,
            &t.id,
            "buyer",
            &VersionedAction {
                expected_version: t.version
            },
            &later
        )
        .is_err()
    );
    assert!(
        decide_tx(
            &conn,
            &t.id,
            "owner",
            true,
            &Decision {
                expected_version: t.version,
                award_units: t.held_units.to_string(),
                responsibility: "buyer".into(),
                note: "Cannot retain after automatic waiver due".into()
            },
            &later
        )
        .is_err()
    );
    assert_eq!(by_id_tx(&conn, &t.id).unwrap().version, t.version);
    assert_eq!(expire_disputes_isolated(&conn, &later, 50).unwrap(), 1);
    assert_clean(&conn, &later);
}

#[test]
fn buyer_ending_claim_is_final_preserves_returns_and_awards_only_remaining_compensation() {
    for cleanup in [false, true] {
        let (conn, id) = fixture(10000, true, 0);
        let at = deliver(&conn, &id, 20);
        let t = accept(&conn, &at, "provider_issue");
        if cleanup {
            cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
        }
        let t = by_id_tx(&conn, &t.id).unwrap();
        assert!(matches!(
            withdraw_tx(
                &conn,
                &t.id,
                "provider",
                &VersionedAction {
                    expected_version: t.version
                },
                &at
            ),
            Err(AppError::Forbidden(_))
        ));
        assert!(
            withdraw_tx(
                &conn,
                &t.id,
                "buyer",
                &VersionedAction {
                    expected_version: t.version - 1
                },
                &at
            )
            .is_err()
        );
        let t = withdraw_tx(
            &conn,
            &t.id,
            "buyer",
            &VersionedAction {
                expected_version: t.version,
            },
            &at,
        )
        .unwrap();
        assert_eq!(t.responsibility, "buyer");
        assert_eq!(t.resolution.as_deref(), Some("buyer_withdrawal"));
        if !cleanup {
            assert_eq!(t.compensation_units, 0);
            assert_eq!(
                wallet_snapshot_tx(&conn, "buyer").unwrap().available_units,
                0
            );
            cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
        }
        let t = by_id_tx(&conn, &t.id).unwrap();
        assert_eq!(t.status, "completed");
        assert_eq!(t.returned_units, units_from_minor(7000).unwrap());
        assert_eq!(t.compensation_units, units_from_minor(1000).unwrap());
        assert_eq!(t.held_units, 0);
        assert!(
            withdraw_tx(
                &conn,
                &t.id,
                "buyer",
                &VersionedAction {
                    expected_version: t.version
                },
                &at
            )
            .is_err()
        );
        assert!(
            dispute_tx(
                &conn,
                &t.id,
                "buyer",
                &Dispute {
                    expected_version: t.version,
                    claim: String::new()
                },
                &at
            )
            .is_err()
        );
        let later = (time(&at).unwrap() + Duration::days(7)).to_rfc3339();
        assert_eq!(expire_disputes_isolated(&conn, &later, 50).unwrap(), 0);
        assert_eq!(credit_due_isolated(&conn, &later, 50).unwrap(), 1);
        assert_clean(&conn, &later);
    }
}

#[test]
fn buyer_ending_an_awarded_claim_does_not_restart_its_seven_day_maturity() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    let t = accept(&conn, &at, "voluntary");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = by_id_tx(&conn, &t.id).unwrap();
    let maturity = t.unlock_at.clone();
    let t = dispute_tx(
        &conn,
        &t.id,
        "buyer",
        &Dispute {
            expected_version: t.version,
            claim: String::new(),
        },
        &at,
    )
    .unwrap();
    let later = (time(&at).unwrap() + Duration::days(1)).to_rfc3339();
    let t = decide_tx(
        &conn,
        &t.id,
        "provider",
        false,
        &Decision {
            expected_version: t.version,
            award_units: (t.compensation_units / 2).to_string(),
            responsibility: "buyer".into(),
            note: "Waive half, remaining claim still disputed".into(),
        },
        &later,
    )
    .unwrap();
    let already_returned = t.returned_units;
    let remaining = t.compensation_units;
    let t = withdraw_tx(
        &conn,
        &t.id,
        "buyer",
        &VersionedAction {
            expected_version: t.version,
        },
        &later,
    )
    .unwrap();
    assert_eq!(t.returned_units, already_returned);
    assert_eq!(t.compensation_units, remaining);
    assert_eq!(t.unlock_at, maturity);
    assert_eq!(
        credit_due_isolated(&conn, maturity.as_deref().unwrap(), 50).unwrap(),
        1
    );
    assert_clean(&conn, &later);
}

#[test]
fn timeout_worker_rolls_back_failed_money_movement_and_retries_without_starving_other_records() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    let t = accept(&conn, &at, "provider_issue");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = by_id_tx(&conn, &t.id).unwrap();
    let later = (time(&at).unwrap() + Duration::days(3)).to_rfc3339();
    conn.execute("INSERT INTO client_market_terminations(id,installation_id,host_id,buyer_user_id,provider_user_id,
        policy_version,rate_bps,period_amount_units,remaining_units,compensation_cap_units,reason,responsibility,status,
        cutoff_at,dispute_opened_at,dispute_used,updated_at) VALUES ('!bad','bad','bad','buyer','provider',0,0,0,0,0,
        'provider_issue','undetermined','finance_pending',?1,?1,1,?1)",params![at]).unwrap();
    assert_eq!(expire_disputes_isolated(&conn, &later, 1).unwrap(), 0);
    conn.execute_batch("CREATE TRIGGER fail_timeout BEFORE UPDATE OF returned_units ON client_market_terminations
        WHEN NEW.returned_units>OLD.returned_units BEGIN SELECT RAISE(ABORT,'simulated finance fault'); END;").unwrap();
    assert_eq!(expire_disputes_isolated(&conn, &later, 1).unwrap(), 0);
    let unchanged = by_id_tx(&conn, &t.id).unwrap();
    assert_eq!(unchanged.version, t.version);
    assert_eq!(unchanged.held_units, t.held_units);
    assert_eq!(unchanged.returned_units, t.returned_units);
    assert!(unchanged.resolution.is_none());
    assert_clean(&conn, &later);
    conn.execute_batch("DROP TRIGGER fail_timeout").unwrap();
    assert_eq!(expire_disputes_isolated(&conn, &later, 1).unwrap(), 0); // wrap cursor
    assert_eq!(expire_disputes_isolated(&conn, &later, 1).unwrap(), 0); // bad record isolated
    assert_eq!(expire_disputes_isolated(&conn, &later, 1).unwrap(), 1); // good record not starved
    assert_eq!(
        by_id_tx(&conn, &t.id).unwrap().returned_units,
        t.remaining_units
    );
    assert_clean(&conn, &later);
}
#[test]
fn voluntary_exit_returns_unused_less_compensation_and_replays_exactly_once() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    assert_eq!(
        wallet_snapshot_tx(&conn, "buyer").unwrap().available_units,
        0
    );
    let t = accept(&conn, &at, "voluntary");
    assert_eq!(t.compensation_cap_units, units_from_minor(1000).unwrap());
    for _ in 0..2 {
        record_cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    }
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    assert_eq!(t.returned_units, units_from_minor(7000).unwrap());
    assert_eq!(t.compensation_units, units_from_minor(1000).unwrap());
    assert_eq!(t.status, "completed");
    assert_eq!(t.cutoff_at, at);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM market_notification_outbox
        WHERE event_kind='client_termination_settled'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM client_termination_compensations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_clean(&conn, &at);
    assert_eq!(
        credit_due_isolated(
            &conn,
            &(time(&at).unwrap() + Duration::days(7) - Duration::seconds(1)).to_rfc3339(),
            10
        )
        .unwrap(),
        0
    );
    crate::market_wallet::ensure_wallet_tx(&conn, "provider", &at).unwrap();
    let maturity = (time(&at).unwrap() + Duration::days(7)).to_rfc3339();
    assert_eq!(credit_due_isolated(&conn, &maturity, 10).unwrap(), 1);
    assert_eq!(credit_due_isolated(&conn, &maturity, 10).unwrap(), 0);
    assert_eq!(
        wallet_snapshot_tx(&conn, "provider")
            .unwrap()
            .available_units,
        t.compensation_units
    );
    assert_clean(&conn, &maturity);
}
#[test]
fn residual_less_than_full_period_penalty_never_charges_extra_money() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 94);
    accept(&conn, &at, "voluntary");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    assert_eq!(t.returned_units, 0);
    assert_eq!(t.compensation_units, units_from_minor(600).unwrap());
    assert_clean(&conn, &at);
}
#[test]
fn claims_hold_only_compensation_and_owner_decision_starts_a_new_seven_day_window() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "provider_issue");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    assert_eq!(t.status, "disputed");
    assert_eq!(t.returned_units, units_from_minor(7000).unwrap());
    assert_eq!(t.held_units, units_from_minor(1000).unwrap());
    assert_eq!(t.compensation_units, 0);
    assert_clean(&conn, &at);
    let t = respond_tx(
        &conn,
        &t.id,
        "provider",
        &ProviderResponse {
            expected_version: t.version,
            note: "Evidence disputed; requesting Owner review".into(),
        },
        &at,
    )
    .unwrap();
    let decision_at = (time(&at).unwrap() + Duration::days(10)).to_rfc3339();
    let d = Decision {
        expected_version: t.version,
        award_units: (t.held_units / 2).to_string(),
        responsibility: "buyer".into(),
        note: "Evidence reviewed; reduced compensation agreed".into(),
    };
    let t = decide_tx(&conn, &t.id, "owner", true, &d, &decision_at).unwrap();
    assert_eq!(t.returned_units, units_from_minor(7500).unwrap());
    assert_eq!(t.held_units, 0);
    assert_eq!(
        time(t.unlock_at.as_deref().unwrap()).unwrap(),
        time(&decision_at).unwrap() + Duration::days(7)
    );
    assert_clean(&conn, &decision_at);
}
#[test]
fn provider_can_waive_but_not_raise_or_decide_an_unproven_claim() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "provider_issue");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    let mut d = Decision {
        expected_version: t.version,
        award_units: (t.held_units + 1).to_string(),
        responsibility: "buyer".into(),
        note: "reviewed".into(),
    };
    assert!(decide_tx(&conn, &t.id, "provider", false, &d, &at).is_err());
    d.award_units = (t.held_units / 2).to_string();
    let t = decide_tx(&conn, &t.id, "provider", false, &d, &at).unwrap();
    assert_eq!(t.status, "disputed");
    assert_eq!(t.compensation_units, 0);
    let d = Decision {
        expected_version: t.version,
        award_units: "0".into(),
        responsibility: "provider".into(),
        note: "Provider voluntarily waives compensation".into(),
    };
    let t = decide_tx(&conn, &t.id, "provider", false, &d, &at).unwrap();
    assert_eq!(t.status, "completed");
    assert_eq!(t.returned_units, units_from_minor(8000).unwrap());
    assert_clean(&conn, &at);
}
#[test]
fn disputed_awarded_compensation_stays_pending_past_maturity_until_resolved() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "voluntary");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    let t = dispute_tx(
        &conn,
        &t.id,
        "buyer",
        &Dispute {
            expected_version: t.version,
            claim: "Evidence of a Provider issue discovered after cleanup".into(),
        },
        &at,
    )
    .unwrap();
    let t = respond_tx(
        &conn,
        &t.id,
        "provider",
        &ProviderResponse {
            expected_version: t.version,
            note: "Evidence disputed; requesting Owner review".into(),
        },
        &at,
    )
    .unwrap();
    let later = (time(&at).unwrap() + Duration::days(20)).to_rfc3339();
    assert_eq!(credit_due_isolated(&conn, &later, 20).unwrap(), 0);
    let t = decide_tx(
        &conn,
        &t.id,
        "owner",
        true,
        &Decision {
            expected_version: t.version,
            award_units: "0".into(),
            responsibility: "platform".into(),
            note: "Verified platform issue; no compensation".into(),
        },
        &later,
    )
    .unwrap();
    assert_eq!(t.returned_units, units_from_minor(8000).unwrap());
    assert_eq!(t.compensation_units, 0);
    assert_clean(&conn, &later);
}
#[test]
fn existing_periods_and_trials_never_gain_a_new_fee() {
    for (terms, trial) in [(false, 0), (true, 43200)] {
        let (conn, id) = fixture(5, terms, trial);
        let at = if trial == 0 {
            deliver(&conn, &id, 20)
        } else {
            START.into()
        };
        accept(&conn, &at, "voluntary");
        cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
        let t = read_tx(&conn, INSTALL).unwrap().unwrap();
        assert_eq!(t.compensation_units, 0);
        assert_eq!(t.status, "completed");
        assert_clean(&conn, &at);
    }
}
#[test]
fn provider_reclaim_and_system_expiry_do_not_charge_a_buyer_penalty() {
    for actor in [Some("provider"), None] {
        let (conn, id) = fixture(10000, true, 0);
        let at = deliver(&conn, &id, 20);
        accept_tx(
            &conn,
            INSTALL,
            actor,
            "provider_or_system_cleanup",
            None,
            &at,
        )
        .unwrap();
        cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
        let t = read_tx(&conn, INSTALL).unwrap().unwrap();
        assert_eq!(t.compensation_units, 0);
        assert_eq!(t.returned_units, units_from_minor(8000).unwrap());
        assert_clean(&conn, &at);
    }
}
#[test]
fn preview_ownership_expiry_period_and_direct_buyer_bypass_are_rejected() {
    let (conn, _) = fixture(10000, true, 0);
    assert!(preview_tx(&conn, INSTALL, "stranger", START).is_err());
    assert!(
        accept_tx(
            &conn,
            INSTALL,
            Some("buyer"),
            "operator_release",
            None,
            START
        )
        .is_err()
    );
    let p = preview_tx(&conn, INSTALL, "buyer", START).unwrap();
    let r = TerminationRequest {
        preview_id: p.preview_id,
        reason: "voluntary".into(),
        claim: "".into(),
    };
    assert!(
        accept_tx(
            &conn,
            INSTALL,
            Some("buyer"),
            "client_release",
            Some(&r),
            &(time(START).unwrap() + Duration::seconds(120)).to_rfc3339()
        )
        .is_err()
    );
    let p = preview_tx(&conn, INSTALL, "buyer", START).unwrap();
    let r = TerminationRequest {
        preview_id: p.preview_id,
        reason: "voluntary".into(),
        claim: "".into(),
    };
    conn.execute("UPDATE client_termination_previews SET period_id=NULL", [])
        .unwrap();
    assert!(
        accept_tx(
            &conn,
            INSTALL,
            Some("buyer"),
            "client_release",
            Some(&r),
            START
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<TerminationRequest>(
            json!({"previewId":"p","reason":"voluntary","awardUnits":"0"})
        )
        .is_err()
    );
}
#[test]
fn cleanup_failure_stops_billing_without_releasing_money_or_claiming_cleanliness() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    let t = accept(&conn, &at, "voluntary");
    cleanup_failed_tx(&conn, INSTALL, &at).unwrap();
    let failed = read_tx(&conn, INSTALL).unwrap().unwrap();
    assert_eq!(failed.status, "cleanup_failed");
    assert!(failed.cleanup_confirmed_at.is_none());
    assert_eq!(
        wallet_snapshot_tx(&conn, "buyer").unwrap().available_units,
        0
    );
    accept_tx(
        &conn,
        INSTALL,
        Some("buyer"),
        "client_release",
        None,
        &(time(&at).unwrap() + Duration::days(1)).to_rfc3339(),
    )
    .unwrap();
    assert_eq!(
        read_tx(&conn, INSTALL).unwrap().unwrap().cutoff_at,
        t.cutoff_at
    );
    assert!(record_cleanup_confirmed_tx(&conn, INSTALL, "wrong-host", &at).is_err());
    record_cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    assert_clean(&conn, &at);
}
#[test]
fn resource_release_survives_financial_exception_and_recovers_from_receipt() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "voluntary");
    conn.execute(
        "UPDATE market_platform_periods SET refunded_units=refunded_units+1",
        [],
    )
    .unwrap();
    record_cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    assert_eq!(t.status, "finance_pending");
    assert!(t.cleanup_confirmed_at.is_some());
    conn.execute(
        "UPDATE client_market_subscriptions SET status='released' WHERE installation_id=?1",
        params![INSTALL],
    )
    .unwrap();
    conn.execute(
        "UPDATE market_platform_periods SET refunded_units=refunded_units-1",
        [],
    )
    .unwrap();
    assert_eq!(recover_cleanup_isolated(&conn, &at).unwrap(), 1);
    assert_eq!(recover_cleanup_isolated(&conn, &at).unwrap(), 0);
    assert_eq!(
        read_tx(&conn, INSTALL).unwrap().unwrap().status,
        "completed"
    );
    assert_clean(&conn, &at);
}
#[test]
fn period_policy_is_frozen_even_after_owner_changes_the_default() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    conn.execute(
        "INSERT INTO client_termination_policies VALUES (2,5000,'owner',?1)",
        params![at],
    )
    .unwrap();
    let p = preview_tx(&conn, INSTALL, "buyer", &at).unwrap();
    assert_eq!(p.rate_bps, 1000);
    assert_eq!(p.policy_version, 1);
    let t = accept(&conn, &at, "voluntary");
    assert_eq!(t.rate_bps, 1000);
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    assert_clean(&conn, &at);
}
#[test]
fn disabling_auto_renewal_does_not_hide_or_prevent_immediate_termination() {
    let (conn, id) = fixture(10000, true, 0);
    conn.execute(
        "UPDATE market_platform_contracts SET renewal_policy='manual' WHERE id=?1",
        params![id],
    )
    .unwrap();
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "voluntary");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    assert_clean(&conn, &at);
}
#[test]
fn waived_award_reverses_only_the_compensation_and_never_resets_service_maturity() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "voluntary");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    let service_unlock: String = conn
        .query_row(
            "SELECT unlock_at FROM market_provider_earning_lots LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let t = decide_tx(
        &conn,
        &t.id,
        "provider",
        false,
        &Decision {
            expected_version: t.version,
            award_units: (t.compensation_units / 2).to_string(),
            responsibility: "buyer".into(),
            note: "Agreed partial waiver".into(),
        },
        &at,
    )
    .unwrap();
    assert_eq!(t.compensation_units, units_from_minor(500).unwrap());
    assert_eq!(t.returned_units, units_from_minor(7500).unwrap());
    assert_eq!(
        service_unlock,
        conn.query_row(
            "SELECT unlock_at FROM market_provider_earning_lots LIMIT 1",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap()
    );
    assert_clean(&conn, &at);
}

#[test]
fn later_period_freezes_new_terms_but_a_pre_upgrade_backlog_has_no_fee() {
    for upgrade_backlog in [false, true] {
        let (conn, id) = fixture(10000, !upgrade_backlog, 0);
        if !upgrade_backlog {
            conn.execute(
                "INSERT INTO client_termination_policies VALUES (2,725,'owner',?1)",
                params![START],
            )
            .unwrap();
        }
        let end = deliver(&conn, &id, 100);
        crate::market_wallet::credit_verified_topup_tx(
            &conn,
            "buyer",
            "second-topup",
            10000,
            "second-receipt",
            &end,
        )
        .unwrap();
        assert!(contracts::renew_monthly_contract_tx(&conn, &id, &end).unwrap());
        let rate:Option<i64>=conn.query_row("SELECT t.rate_bps FROM client_termination_period_terms t
            JOIN market_platform_periods p ON p.id=t.period_id WHERE p.contract_id=?1 AND p.sequence=2",params![id],|r|r.get(0)).optional().unwrap();
        assert_eq!(rate, if upgrade_backlog { None } else { Some(725) });
        assert_clean(&conn, &end);
    }
}
#[test]
fn free_rental_cleanup_never_creates_or_debits_wallet_money() {
    let conn = crate::test_support::database().unwrap();
    conn.execute("INSERT INTO client_market_subscriptions (installation_id,host_id,provider_id,host_owner_email,
        client_user_id,client_owner_email,status,offer_revision,created_at,updated_at)
        VALUES (?1,?2,'provider','provider@example.test','buyer','buyer@example.test','active',1,?3,?3)",params![INSTALL,HOST,START]).unwrap();
    accept(&conn, START, "voluntary");
    record_cleanup_confirmed_tx(&conn, INSTALL, HOST, START).unwrap();
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    assert_eq!(t.status, "completed");
    assert_eq!(t.returned_units, 0);
    assert_eq!(t.compensation_units, 0);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM market_journal_transactions",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_clean(&conn, START);
}
#[test]
fn generic_finalization_cannot_bypass_client_compensation_and_share_is_untouched() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "voluntary");
    let result = contracts::terminate_after_cleanup_tx(&conn, &id, true, &at).unwrap();
    assert_eq!(result.refunded_units, units_from_minor(7000).unwrap());
    assert_eq!(
        contracts::terminate_after_cleanup_tx(&conn, &id, true, &at)
            .unwrap()
            .refunded_units,
        0
    );
    assert_eq!(
        finalize_recorded_month_tx(&conn, "unrelated-share-contract", &at).unwrap(),
        None
    );
    assert_clean(&conn, &at);
}
#[test]
fn compensation_journals_and_immutable_snapshots_are_independently_audited() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    let t = accept(&conn, &at, "voluntary");
    assert!(
        conn.execute(
            "UPDATE client_market_terminations SET cutoff_at=?2 WHERE id=?1",
            params![t.id, START]
        )
        .is_err()
    );
    assert!(
        conn.execute("UPDATE client_termination_period_terms SET rate_bps=0", [])
            .is_err()
    );
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    assert_clean(&conn, &at);
    assert!(
        conn.execute(
            "UPDATE client_termination_compensations SET unlock_at=awarded_at",
            []
        )
        .is_err()
    );
    conn.execute(
        "UPDATE client_termination_compensations SET credit_journal_id=journal_id",
        [],
    )
    .unwrap();
    assert!(!evidence_valid_tx(&conn, &t.id).unwrap());
    conn.execute(
        "UPDATE client_termination_compensations SET credit_journal_id=NULL",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE client_termination_compensations SET credited_units=1",
        [],
    )
    .unwrap();
    assert!(!evidence_valid_tx(&conn, &t.id).unwrap());
    assert!(crate::market_reconcile::audit_finance_tx(&conn, &at).unwrap() > 0);
    assert!(crate::market_reconcile::ensure_contract_finance_safe_tx(&conn, &id).is_err());
    use crate::market_reconcile::{FinanceScope, ensure_finance_scopes_safe_tx};
    for scope in [
        FinanceScope::User("buyer"),
        FinanceScope::User("provider"),
        FinanceScope::Provider("provider"),
    ] {
        assert!(ensure_finance_scopes_safe_tx(&conn, &[scope]).is_err());
    }
    assert!(ensure_finance_scopes_safe_tx(&conn, &[FinanceScope::User("unrelated")]).is_ok());
}

#[test]
fn responsibility_dispute_does_not_freeze_either_partys_other_wallet_money() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "provider_issue");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    use crate::market_reconcile::{FinanceScope, ensure_finance_scopes_safe_tx};
    for scope in [
        FinanceScope::User("buyer"),
        FinanceScope::User("provider"),
        FinanceScope::Provider("provider"),
    ] {
        ensure_finance_scopes_safe_tx(&conn, &[scope]).unwrap();
    }
    assert_clean(&conn, &at);
}

#[test]
fn wallet_projection_failure_defers_money_but_preserves_physical_cleanup_and_privacy() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "voluntary");
    conn.execute(
        "UPDATE market_wallet_accounts SET posted_units=posted_units+1 WHERE user_id='buyer'",
        [],
    )
    .unwrap();
    record_cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    assert_eq!(t.status, "finance_pending");
    assert_eq!(t.returned_units, 0);
    assert!(t.cleanup_confirmed_at.is_some());
    let decision = Decision {
        expected_version: t.version,
        award_units: "0".into(),
        responsibility: "provider".into(),
        note: "waive compensation".into(),
    };
    assert!(matches!(
        decide_tx(&conn, &t.id, "stranger", false, &decision, &at),
        Err(AppError::Forbidden(_))
    ));
    assert!(matches!(
        dispute_tx(
            &conn,
            &t.id,
            "stranger",
            &Dispute {
                expected_version: t.version,
                claim: "Verifiable issue supplied for independent review".into(),
            },
            &at
        ),
        Err(AppError::Forbidden(_))
    ));
    conn.execute(
        "UPDATE market_wallet_accounts SET posted_units=posted_units-1 WHERE user_id='buyer'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE client_market_subscriptions SET status='released' WHERE installation_id=?1",
        params![INSTALL],
    )
    .unwrap();
    assert_eq!(recover_cleanup_isolated(&conn, &at).unwrap(), 1);
    assert_eq!(
        read_tx(&conn, INSTALL).unwrap().unwrap().status,
        "completed"
    );
    assert_clean(&conn, &at);
}

#[test]
fn waiver_cannot_move_money_from_corrupt_award_before_the_periodic_audit() {
    let (conn, id) = fixture(10000, true, 0);
    let at = deliver(&conn, &id, 20);
    accept(&conn, &at, "voluntary");
    cleanup_confirmed_tx(&conn, INSTALL, HOST, &at).unwrap();
    let t = read_tx(&conn, INSTALL).unwrap().unwrap();
    conn.execute(
        "UPDATE client_termination_compensations SET reversed_units=1",
        [],
    )
    .unwrap();
    assert!(
        decide_tx(
            &conn,
            &t.id,
            "provider",
            false,
            &Decision {
                expected_version: t.version,
                award_units: "0".into(),
                responsibility: "provider".into(),
                note: "waive compensation".into(),
            },
            &at
        )
        .is_err()
    );
    assert_eq!(
        read_tx(&conn, INSTALL).unwrap().unwrap().returned_units,
        t.returned_units
    );
    conn.execute(
        "UPDATE client_termination_compensations SET reversed_units=0",
        [],
    )
    .unwrap();
    assert_clean(&conn, &at);
}
