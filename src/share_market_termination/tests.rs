use super::*;
use crate::market_platform_contracts::{self as contracts, BillingModel, PrepareContract};
use crate::share_market_termination_contract::UsageWindow;

const START: &str = "2026-01-01T00:00:00+00:00";

fn fixture(terms: bool) -> (Connection, String, String) {
    fixture_with_trial(terms, 0)
}
fn fixture_with_trial(terms: bool, trial: i64) -> (Connection, String, String) {
    let c = crate::test_support::database().unwrap();
    c.execute(
        "UPDATE market_finance_control SET mode='enabled',cutover_approved_at=?1",
        params![START],
    )
    .unwrap();
    for (id, email) in [
        ("buyer", "buyer@example.test"),
        ("provider", "provider@example.test"),
    ] {
        c.execute("INSERT INTO users(id,email_normalized,status,email_verified_at,created_at,last_login_at) VALUES(?1,?2,'active',?3,?3,?3)",params![id,email,START]).unwrap();
    }
    c.execute("INSERT INTO market_provider_profiles(id,canonical_email,user_id,display_name,claim_state,status,created_at,updated_at) VALUES('profile','provider@example.test','provider','Provider','claimed','active',?1,?1)",params![START]).unwrap();
    c.execute("INSERT INTO shares(share_id,capacity_pool_id,installation_id,share_name,owner_email,subdomain,app_type,token_limit,tokens_used,requests_count,share_status,created_at,expires_at,updated_at,termination_archive_since_ms) VALUES('share','pool','installation','Share','provider@example.test','share','codex',-1,0,0,'active',?1,'9999-12-31T00:00:00Z',?1,0)",params![START]).unwrap();
    c.execute("INSERT INTO share_market_listings(id,share_id,installation_id,owner_user_id,owner_email,status,created_at,updated_at) VALUES('listing','share','installation','provider','provider@example.test','active',?1,?1)",params![START]).unwrap();
    c.execute("INSERT INTO share_market_seats(id,listing_id,position,status,token_limit,token_period_json,current_subscription_id,created_at,updated_at) VALUES('seat','listing',1,'occupied',100,'\"lifetime\"','sub',?1,?1)",params![START]).unwrap();
    c.execute("INSERT INTO share_market_subscriptions(id,seat_id,listing_id,share_id,installation_id,entitlement_id,owner_user_id,owner_email,renter_user_id,renter_email,status,token_limit,token_period_json,offer_revision,created_at,updated_at,activated_at,service_started_at,pricing_model,cycle_price_minor) VALUES('sub','seat','listing','share','installation','entitlement','provider','provider@example.test','buyer','buyer@example.test','active',100,'\"lifetime\"',1,?1,?1,?1,?1,'prepaid_calendar_month',10000)",params![START]).unwrap();
    crate::market_wallet::credit_verified_topup_tx(&c, "buyer", "topup", 10000, "receipt", START)
        .unwrap();
    crate::market_commission::freeze_quote_tx(&c, "quote", "provider", START).unwrap();
    if terms {
        freeze_quote_tx(&c, "quote", Some(&policy_tx(&c).unwrap())).unwrap();
    }
    let contract = contracts::prepare_contract_tx(
        &c,
        &PrepareContract {
            product_kind: "share",
            product_ref: "sub",
            trial_scope_ref: "share",
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
        c.execute(
            "UPDATE shares SET share_access_policy_version=?1 WHERE share_id='share'",
            params![i64::from(crate::models::SHARE_TRIAL_CONTRACT_VERSION)],
        )
        .unwrap();
        crate::market_share_trials::prepare_tx(
            &c,
            &contract,
            "entitlement",
            &crate::market_share_trials::TrialQuote {
                version: 1,
                policy_version: 1,
                hours_inherited: false,
                tokens_inherited: false,
                token_allowance: 0,
                token_budget: 0,
                value_allowance_usd_micros: None,
                value_budget_usd_micros: None,
                value_inherited: false,
                unit_locked: false,
            },
            START,
        )
        .unwrap();
    }
    contracts::activate_contract_tx(&c, &contract, START, START).unwrap();
    if trial > 0 {
        return (
            c,
            contract,
            (time(START).unwrap() + Duration::seconds(trial / 2)).to_rfc3339(),
        );
    }
    let end: String = c
        .query_row(
            "SELECT period_end FROM market_platform_periods WHERE contract_id=?1",
            params![contract],
            |r| r.get(0),
        )
        .unwrap();
    let at = (time(START).unwrap() + (time(&end).unwrap() - time(START).unwrap()) / 4).to_rfc3339();
    contracts::settle_monthly_segment_tx(&c, &contract, "fixture-service", START, &at, true, &at)
        .unwrap();
    c.execute(
        "UPDATE market_platform_contracts SET last_evaluated_at=?2 WHERE id=?1",
        params![contract, at],
    )
    .unwrap();
    (c, contract, at)
}
fn accept(c: &Connection, at: &str, reason: &str) -> Termination {
    let p = preview_tx(c, "sub", "buyer", at).unwrap();
    accept_tx(
        c,
        "sub",
        "buyer",
        &Accept {
            preview_id: p["previewId"].as_str().unwrap().into(),
            reason: reason.into(),
            claim: String::new(),
        },
        at,
    )
    .unwrap()
}
fn receipt(c: &Connection, t: &Termination, used: &str) -> TerminationReceipt {
    let operation_id=c.query_row("SELECT id FROM share_control_operations WHERE subscription_id='sub' AND action='revoke' ORDER BY share_sequence DESC LIMIT 1",[],|r|r.get(0)).unwrap();
    TerminationReceipt {
        termination_id: t.id.clone(),
        entitlement_id: t.entitlement_id.clone(),
        share_id: t.share_id.clone(),
        email: t.buyer_email.clone(),
        operation_id,
        cutoff_at_ms: time(&t.cutoff_at).unwrap().timestamp_millis(),
        period_start_ms: t.period_start_ms,
        period_end_ms: t.period_end_ms,
        revoked_at_ms: time(&t.cutoff_at).unwrap().timestamp_millis(),
        archive_since_ms: 0,
        quota_used_before_period: "0".into(),
        windows: vec![UsageWindow {
            starts_at_ms: t.period_start_ms,
            ends_at_ms: t.period_end_ms,
            used: used.into(),
        }],
    }
}

#[test]
fn historical_periods_gain_no_terms_and_new_terms_are_immutable() {
    let (c, contract, at) = fixture(false);
    assert!(preview_tx(&c, "sub", "buyer", &at).is_err());
    assert_eq!(c.query_row("SELECT COUNT(*) FROM share_termination_period_terms WHERE period_id IN(SELECT id FROM market_platform_periods WHERE contract_id=?1)",params![contract],|r|r.get::<_,i64>(0)).unwrap(),0);
    let (c, _, at) = fixture(true);
    let t = basis(&c, "sub", &at).unwrap();
    assert_eq!(t.rate_bps, 1000);
    assert!(
        c.execute("UPDATE share_termination_period_terms SET rate_bps=0", [])
            .is_err()
    );
    assert!(
        c.execute("DELETE FROM share_termination_policies", [])
            .is_err()
    );
}
#[test]
fn no_return_without_final_original_receipt_and_replay_is_idempotent() {
    let (c, _, at) = fixture(true);
    let t = accept(&c, &at, "voluntary");
    assert!(finish_tx(&c, "sub", &at).is_err());
    let mut r = receipt(&c, &t, "60");
    r.entitlement_id = "another-rental".into();
    assert!(record_receipt_tx(&c, "installation", &r, &at).is_err());
    r.entitlement_id = t.entitlement_id.clone();
    record_receipt_tx(&c, "installation", &r, &at).unwrap();
    record_receipt_tx(&c, "installation", &r, &at).unwrap();
    r.windows[0].used = "61".into();
    assert!(record_receipt_tx(&c, "installation", &r, &at).is_err());
    finish_tx(&c, "sub", &at).unwrap();
    let settled = load(&c, &t.id).unwrap();
    assert_eq!(
        settled.quota_earned_units,
        settled.period_amount_units * 35 / 100
    );
    assert_eq!(
        settled.returned_units,
        settled.period_amount_units * 30 / 100
    );
    assert_eq!(settled.compensation_units, settled.period_amount_units / 10);
    assert!(evidence_valid_tx(&c, &t.id).unwrap());
    let n: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM market_journal_transactions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    finish_tx(&c, "sub", &at).unwrap();
    assert_eq!(
        n,
        c.query_row(
            "SELECT COUNT(*) FROM market_journal_transactions",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
    );
}
#[test]
fn problem_claim_optional_72h_waiver_and_frozen_only_compensation() {
    let (c, _, at) = fixture(true);
    let t = accept(&c, &at, "provider_issue");
    let r = receipt(&c, &t, "0");
    record_receipt_tx(&c, "installation", &r, &at).unwrap();
    finish_tx(&c, "sub", &at).unwrap();
    let t = load(&c, &t.id).unwrap();
    assert_eq!(t.held_units, t.period_amount_units / 10);
    assert_eq!(t.returned_units, t.period_amount_units * 65 / 100);
    let due = (time(&at).unwrap() + Duration::hours(72)).to_rfc3339();
    maintain_isolated(&c, &due, 200).unwrap();
    let t = load(&c, &t.id).unwrap();
    assert_eq!(t.held_units, 0);
    assert_eq!(t.compensation_units, 0);
    assert_eq!(t.returned_units, t.period_amount_units * 75 / 100);
    assert!(evidence_valid_tx(&c, &t.id).unwrap());
}
#[test]
fn party_cannot_adjudicate_and_buyer_may_close_claim() {
    let (c, _, at) = fixture(true);
    let t = accept(&c, &at, "platform_issue");
    let input = Action {
        expected_version: t.version,
        award_units: None,
        note: "Formal evidence for Owner review".into(),
    };
    let t = act_tx(&c, &t.id, "provider", false, "respond", &input, &at).unwrap();
    assert!(
        act_tx(
            &c,
            &t.id,
            "provider",
            true,
            "resolve",
            &Action {
                expected_version: t.version,
                award_units: Some("1".into()),
                note: "Decision".into()
            },
            &at
        )
        .is_err()
    );
    assert!(
        act_tx(
            &c,
            &t.id,
            "buyer",
            false,
            "withdraw",
            &Action {
                expected_version: t.version - 1,
                award_units: None,
                note: String::new()
            },
            &at
        )
        .is_err()
    );
    let t = act_tx(
        &c,
        &t.id,
        "buyer",
        false,
        "withdraw",
        &Action {
            expected_version: t.version,
            award_units: None,
            note: String::new(),
        },
        &at,
    )
    .unwrap();
    assert_eq!(t.responsibility, "buyer");
}
#[test]
fn row_actions_are_party_scoped_escalation_only_and_do_not_load_detail_history() {
    let (c, _, at) = fixture(true);
    let t = accept(&c, &at, "provider_issue");
    let actor = |id: &str| AuthSession {
        session_id: format!("session-{id}"),
        user_id: id.into(),
        email: format!("{id}@example.test"),
        auth_source_kind: "email".into(),
        auth_source_id: id.into(),
        access_token_hash: String::new(),
        refresh_token_hash: String::new(),
        access_expires_at: time(&at).unwrap(),
        refresh_expires_at: time(&at).unwrap(),
        created_at: time(&at).unwrap(),
        last_used_at: time(&at).unwrap(),
    };
    let provider = actor("provider");
    let owner = actor("owner");
    let row = view(&c, &t, &provider, false, &at, false).unwrap();
    assert_eq!(row["actionRequired"], true);
    assert_eq!(row["permissions"]["respond"], true);
    assert_eq!(row["events"], json!([]));
    assert!(row["eventsNextCursor"].is_null());
    let detail = view(&c, &t, &provider, false, &at, true).unwrap();
    assert!(!detail["events"].as_array().unwrap().is_empty());
    assert_eq!(
        view(&c, &t, &owner, true, &at, false).unwrap()["actionRequired"],
        false
    );
    assert!(view(&c, &t, &actor("other"), false, &at, true).is_err());
    let t = act_tx(
        &c,
        &t.id,
        "provider",
        false,
        "respond",
        &Action {
            expected_version: t.version,
            award_units: None,
            note: "Please review this evidence".into(),
        },
        &at,
    )
    .unwrap();
    let owner_row = view(&c, &t, &owner, true, &at, false).unwrap();
    assert_eq!(owner_row["actionRequired"], true);
    assert_eq!(owner_row["permissions"]["resolve"], true);
    assert_eq!(
        view(&c, &t, &provider, true, &at, false).unwrap()["permissions"]["resolve"],
        false,
        "a Provider who is also Owner cannot adjudicate their own transaction"
    );
    let due = (time(&at).unwrap() + Duration::days(7)).to_rfc3339();
    assert_eq!(
        view(&c, &t, &owner, true, &due, false).unwrap()["actionRequired"],
        false
    );
}
#[test]
fn failure_after_revoke_keeps_receipt_and_recovery_is_isolated() {
    let (c, _, at) = fixture(true);
    let t = accept(&c, &at, "voluntary");
    record_receipt_tx(&c, "installation", &receipt(&c, &t, "0"), &at).unwrap();
    c.execute_batch("CREATE TEMP TRIGGER fail_share_return BEFORE INSERT ON market_journal_transactions WHEN NEW.source_kind='share_termination' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    assert!(finish_tx(&c, "sub", &at).unwrap());
    let t = load(&c, &t.id).unwrap();
    assert_eq!(t.status, "finance_pending");
    assert!(t.revoked_at.is_some());
    assert_eq!(t.returned_units, 0);
    c.execute_batch("DROP TRIGGER fail_share_return;").unwrap();
    maintain_isolated(&c, &at, 200).unwrap();
    assert_eq!(load(&c, &t.id).unwrap().status, "completed");
    assert!(evidence_valid_tx(&c, &t.id).unwrap());
}
#[test]
fn normalized_recurring_and_lifetime_quota_do_not_multiply_time_earning() {
    let (c, _, at) = fixture(true);
    let mut t = basis(&c, "sub", &at).unwrap();
    let mut r = receipt_for_math(&t, "100");
    r.quota_used_before_period = "50".into();
    r.windows[0].used = "25".into();
    assert_eq!(
        settlement::validate_usage(&t, &r).unwrap(),
        t.period_amount_units / 4
    );
    r.windows.push(r.windows[0].clone());
    assert!(settlement::validate_usage(&t, &r).is_err());
    t.policy.token_period = crate::models::ShareTokenPeriod::Day;
    r.windows = vec![UsageWindow {
        starts_at_ms: t.period_start_ms,
        ends_at_ms: t.period_start_ms + 86_400_000,
        used: "100".into(),
    }];
    assert_eq!(settlement::validate_usage(&t, &r).unwrap(), 0); // one day's quota is not the whole month's rent
}

#[test]
fn uncharged_trial_ends_without_reserving_rent_or_compensation() {
    let (c, contract, at) = fixture_with_trial(true, 3600);
    assert_eq!(
        c.query_row(
            "SELECT status FROM market_platform_contracts WHERE id=?1",
            params![contract],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "trialing"
    );
    let before = crate::market_wallet::wallet_snapshot_tx(&c, "buyer").unwrap();
    assert_eq!(before.held_units, 0, "a free trial must not reserve rent");
    assert_eq!(before.available_units, before.posted_units);
    let t = accept(&c, &at, "provider_issue");
    assert!(t.period_id.is_none());
    assert_eq!(t.compensation_cap_units, 0);
    let mut r = receipt(&c, &t, "0");
    r.windows.clear();
    record_receipt_tx(&c, "installation", &r, &at).unwrap();
    finish_tx(&c, "sub", &at).unwrap();
    let t = load(&c, &t.id).unwrap();
    assert_eq!(t.status, "completed");
    assert_eq!(
        (t.compensation_units, t.returned_units, t.held_units),
        (0, 0, 0)
    );
    let wallet = crate::market_wallet::wallet_snapshot_tx(&c, "buyer").unwrap();
    assert_eq!(wallet.held_units, 0);
    assert_eq!(wallet.posted_units, before.posted_units);
    assert_eq!(wallet.available_units, before.available_units);
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM market_platform_periods WHERE contract_id=?1",
            params![contract],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "ending a free trial must not create a paid period or refund"
    );
    assert!(evidence_valid_tx(&c, &t.id).unwrap());
}

#[test]
fn uncharged_conversion_releases_the_first_payment_hold_without_compensation() {
    let (c, contract, _) = fixture_with_trial(true, 3600);
    let at = (time(START).unwrap() + Duration::hours(1)).to_rfc3339();
    crate::market_share_trials::request_conversion_tx(&c, &contract, "time", &at).unwrap();
    let hold: String = c.query_row(
        "SELECT conversion_hold_id FROM market_share_trial_contracts WHERE contract_id=?1 AND phase='activating'",
        params![contract], |r| r.get(0)
    ).unwrap();
    let before = crate::market_wallet::wallet_snapshot_tx(&c, "buyer").unwrap();
    assert_eq!(
        before.held_units,
        crate::market_wallet::units_from_minor(10000).unwrap()
    );
    let t = accept(&c, &at, "provider_issue");
    assert!(t.period_id.is_none());
    assert_eq!(t.compensation_cap_units, 0);
    assert_eq!(
        crate::market_wallet::wallet_snapshot_tx(&c, "buyer")
            .unwrap()
            .held_units,
        before.held_units,
        "the conversion hold remains until the original entitlement is confirmed revoked"
    );
    let mut r = receipt(&c, &t, "0");
    r.windows.clear();
    record_receipt_tx(&c, "installation", &r, &at).unwrap();
    finish_tx(&c, "sub", &at).unwrap();
    let t = load(&c, &t.id).unwrap();
    assert_eq!(t.status, "completed");
    assert_eq!(
        (t.compensation_units, t.returned_units, t.held_units),
        (0, 0, 0)
    );
    let wallet = crate::market_wallet::wallet_snapshot_tx(&c, "buyer").unwrap();
    assert_eq!(wallet.held_units, 0);
    assert_eq!(wallet.posted_units, before.posted_units);
    assert_eq!(wallet.available_units, before.posted_units);
    assert_eq!(
        c.query_row(
            "SELECT status FROM market_wallet_holds WHERE id=?1",
            params![hold],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "released"
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM market_platform_periods WHERE contract_id=?1",
            params![contract],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    finish_tx(&c, "sub", &at).unwrap();
    assert_eq!(
        crate::market_wallet::wallet_snapshot_tx(&c, "buyer")
            .unwrap()
            .available_units,
        wallet.available_units
    );
    assert!(evidence_valid_tx(&c, &t.id).unwrap());
}

#[test]
fn partial_waiver_returns_credit_then_owner_timeout_waives_only_the_remaining_hold() {
    let (c, _, at) = fixture(true);
    let t = accept(&c, &at, "provider_issue");
    record_receipt_tx(&c, "installation", &receipt(&c, &t, "0"), &at).unwrap();
    finish_tx(&c, "sub", &at).unwrap();
    let t = load(&c, &t.id).unwrap();
    let award = t.held_units / 2;
    let t = act_tx(
        &c,
        &t.id,
        "provider",
        false,
        "waive",
        &Action {
            expected_version: t.version,
            award_units: Some(award.to_string()),
            note: "Partial waiver; remaining amount is disputed".into(),
        },
        &at,
    )
    .unwrap();
    assert_eq!(t.held_units, award);
    assert_eq!(t.compensation_units, 0);
    assert!(t.provider_responded_at.is_some());
    // Repeated partial waivers must not duplicate or replace the Owner notice.
    notify_owner_review(
        &c,
        &t,
        &(time(&at).unwrap() + Duration::hours(1)).to_rfc3339(),
    )
    .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM user_notification_events WHERE id=?1",
            params![format!("share-termination-review:{}", t.id)],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let (scope, category, created_at_ms): (String, String, i64) = c
        .query_row(
            "SELECT scope,category,created_at_ms FROM user_notification_events WHERE id=?1",
            params![format!("share-termination-review:{}", t.id)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(scope, "owner");
    assert_eq!(category, "system");
    assert_eq!(created_at_ms, time(&at).unwrap().timestamp_millis());
    let due = (time(&at).unwrap() + Duration::days(7)).to_rfc3339();
    maintain_isolated(&c, &due, 200).unwrap();
    let t = load(&c, &t.id).unwrap();
    assert_eq!(t.status, "completed");
    assert_eq!(t.held_units, 0);
    assert_eq!(t.returned_units, t.period_amount_units * 75 / 100);
    assert!(evidence_valid_tx(&c, &t.id).unwrap());
}

#[test]
fn compensation_automatically_credits_once_after_seven_days_and_finance_holds_are_scoped() {
    let (c, _, at) = fixture(true);
    let t = accept(&c, &at, "voluntary");
    record_receipt_tx(&c, "installation", &receipt(&c, &t, "0"), &at).unwrap();
    finish_tx(&c, "sub", &at).unwrap();
    let t = load(&c, &t.id).unwrap();
    let due = (time(&at).unwrap() + Duration::days(7)).to_rfc3339();
    maintain_isolated(&c, &at, 200).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT credited_units FROM share_termination_income",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    maintain_isolated(&c, &due, 200).unwrap();
    let wallet = crate::market_wallet::wallet_snapshot_tx(&c, "provider").unwrap();
    assert_eq!(wallet.available_units, t.compensation_units);
    maintain_isolated(&c, &due, 200).unwrap();
    assert_eq!(
        crate::market_wallet::wallet_snapshot_tx(&c, "provider")
            .unwrap()
            .available_units,
        wallet.available_units
    );
    assert!(evidence_valid_tx(&c, &t.id).unwrap());
    c.execute("INSERT INTO market_finance_reconciliation_cases(id,code,object_kind,object_id,status,detail_code,created_at) VALUES('hold','share_termination_evidence_mismatch','share_termination',?1,'open','fixture',?2)",params![t.id,at]).unwrap();
    assert!(
        crate::market_reconcile::ensure_contract_finance_safe_tx(
            &c,
            t.contract_id.as_deref().unwrap()
        )
        .is_err()
    );
    assert!(
        crate::market_reconcile::ensure_finance_scopes_safe_tx(
            &c,
            &[crate::market_reconcile::FinanceScope::User("unrelated")]
        )
        .is_ok()
    );
}
fn receipt_for_math(t: &Termination, used: &str) -> TerminationReceipt {
    TerminationReceipt {
        termination_id: t.id.clone(),
        entitlement_id: t.entitlement_id.clone(),
        share_id: t.share_id.clone(),
        email: t.buyer_email.clone(),
        operation_id: "math".into(),
        cutoff_at_ms: time(&t.cutoff_at).unwrap().timestamp_millis(),
        period_start_ms: t.period_start_ms,
        period_end_ms: t.period_end_ms,
        revoked_at_ms: time(&t.cutoff_at).unwrap().timestamp_millis(),
        archive_since_ms: 0,
        quota_used_before_period: "0".into(),
        windows: vec![UsageWindow {
            starts_at_ms: t.period_start_ms,
            ends_at_ms: t.period_end_ms,
            used: used.into(),
        }],
    }
}
