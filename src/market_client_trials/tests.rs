use super::*;
use crate::market_platform_contracts::{self as contracts, BillingModel, PrepareContract};

const START: &str = "2026-01-01T00:00:00+00:00";
const FIVE: &str = "2026-01-01T00:00:05+00:00";
const END: &str = "2026-01-01T00:00:20+00:00";

fn fixture() -> Connection {
    let conn = crate::test_support::database().unwrap();
    conn.execute(
        "UPDATE market_finance_control SET mode='enabled',cutover_approved_at=?1",
        params![START],
    )
    .unwrap();
    crate::market_wallet::credit_verified_topup_tx(
        &conn, "buyer", "topup", 100_000, "receipt", START,
    )
    .unwrap();
    conn
}

fn prepare(conn: &Connection, job: &str, trial: i64, model: BillingModel) -> String {
    contracts::prepare_contract_tx(
        conn,
        &PrepareContract {
            product_kind: "client_host",
            product_ref: job,
            trial_scope_ref: "host",
            quote_id: job,
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: model,
            price_minor: 1000,
            auto_renew: model == BillingModel::CalendarMonth,
            trial_seconds: trial,
            trial_allowance_seconds: 20,
        },
        START,
    )
    .unwrap()
}

fn state(conn: &Connection) -> TrialState {
    state_tx(conn, "buyer", "provider", "host", 20).unwrap()
}

fn consume(conn: &Connection, id: &str, source: &str, from: &str, to: &str, healthy: bool) {
    contracts::advance_trial_interval_tx(
        conn,
        id,
        "client_host_health",
        source,
        from,
        to,
        healthy,
        to,
    )
    .unwrap();
}

fn end_trial(conn: &Connection, id: &str) {
    conn.execute(
        "UPDATE market_platform_contracts SET status='ended',ended_at=?2 WHERE id=?1",
        params![id, FIVE],
    )
    .unwrap();
}

#[test]
fn unpaid_rerentals_share_consumption_but_outages_and_topups_do_not_reset_it() {
    let conn = fixture();
    let first = prepare(&conn, "first", 20, BillingModel::CalendarMonth);
    contracts::activate_contract_tx(&conn, &first, START, START).unwrap();
    consume(&conn, &first, "healthy", START, FIVE, true);
    consume(&conn, &first, "outage", FIVE, END, false);
    assert_eq!(
        (state(&conn).epoch, state(&conn).remaining_seconds),
        (0, 15)
    );
    end_trial(&conn, &first);
    let second = prepare(&conn, "second", 15, BillingModel::CalendarMonth);
    contracts::activate_contract_tx(&conn, &second, START, START).unwrap();
    consume(&conn, &second, "healthy", START, FIVE, true);
    crate::market_wallet::credit_verified_topup_tx(
        &conn,
        "buyer",
        "another-topup",
        1000,
        "another-receipt",
        FIVE,
    )
    .unwrap();
    let s = state(&conn);
    assert_eq!(
        (s.epoch, s.consumed_seconds, s.remaining_seconds),
        (0, 10, 10)
    );
    assert!(s.last_reset_at.is_none());
    assert_eq!(
        state_tx(&conn, "other", "provider", "host", 20)
            .unwrap()
            .remaining_seconds,
        20
    );
    assert_eq!(
        state_tx(&conn, "buyer", "other", "host", 20)
            .unwrap()
            .remaining_seconds,
        20
    );
    assert_eq!(
        state_tx(&conn, "buyer", "provider", "other", 20)
            .unwrap()
            .remaining_seconds,
        20
    );
}

#[test]
fn conversion_restores_only_next_rental_and_old_evidence_never_consumes_the_new_epoch() {
    let conn = fixture();
    let first = prepare(&conn, "first", 20, BillingModel::CalendarMonth);
    contracts::activate_contract_tx(&conn, &first, START, START).unwrap();
    consume(&conn, &first, "healthy", START, END, true);
    let s = state(&conn);
    assert_eq!(
        (s.epoch, s.consumed_seconds, s.remaining_seconds),
        (1, 0, 20)
    );
    let active: (String, i64) = conn
        .query_row(
            "SELECT status,trial_seconds_remaining FROM market_platform_contracts WHERE id=?1",
            params![first],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(active, ("active".into(), 0));
    let period: String = conn
        .query_row(
            "SELECT id FROM market_platform_periods WHERE contract_id=?1",
            params![first],
            |r| r.get(0),
        )
        .unwrap();
    end_trial(&conn, &first);
    let second = prepare(&conn, "second", 20, BillingModel::CalendarMonth);
    contracts::activate_contract_tx(&conn, &second, START, START).unwrap();
    consume(&conn, &second, "new-healthy", START, FIVE, true);
    paid_month_tx(&conn, &first, &period, END).unwrap();
    conn.execute("INSERT INTO market_platform_trial_segments
        (id,contract_id,source_kind,source_id,started_at,ended_at,service_available,consumed_seconds,created_at)
        VALUES ('late',?1,'client_host_health','late',?2,?3,1,5,?3)",params![first,START,FIVE]).unwrap();
    assert_eq!(
        (state(&conn).epoch, state(&conn).remaining_seconds),
        (1, 15)
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM client_market_trial_resets", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(validate_quote_epoch_tx(&conn, "buyer", "provider", "host", 0).is_err());
    validate_quote_epoch_tx(&conn, "buyer", "provider", "host", 1).unwrap();
    // Disabling or reducing an offer does not manufacture another reset.
    assert_eq!(
        state_tx(&conn, "buyer", "provider", "host", 0)
            .unwrap()
            .remaining_seconds,
        0
    );
    assert_eq!(
        state_tx(&conn, "buyer", "provider", "host", 3)
            .unwrap()
            .remaining_seconds,
        0
    );
    assert_eq!(state(&conn).epoch, 1);
}

#[test]
fn direct_paid_activation_and_each_monthly_renewal_restore_without_stacking_or_reentering_trial() {
    let conn = fixture();
    let id = prepare(&conn, "paid", 0, BillingModel::CalendarMonth);
    assert_eq!(state(&conn).epoch, 0); // Reservation is not payment.
    contracts::activate_contract_tx(&conn, &id, START, START).unwrap();
    contracts::activate_contract_tx(&conn, &id, START, START).unwrap();
    assert_eq!(
        (state(&conn).epoch, state(&conn).remaining_seconds),
        (1, 20)
    );
    let end: String = conn
        .query_row(
            "SELECT period_end FROM market_platform_periods WHERE contract_id=?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    contracts::settle_monthly_segment_tx(&conn, &id, "month", START, &end, true, &end).unwrap();
    assert!(contracts::renew_monthly_contract_tx(&conn, &id, &end).unwrap());
    assert!(!contracts::renew_monthly_contract_tx(&conn, &id, &end).unwrap());
    assert_eq!(
        (state(&conn).epoch, state(&conn).remaining_seconds),
        (2, 20)
    );
    let current: (String, i64) = conn
        .query_row(
            "SELECT status,trial_seconds_remaining FROM market_platform_contracts WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(current, ("active".into(), 0));
    assert!(
        conn.execute(
            "UPDATE market_platform_contracts SET client_trial_epoch=2 WHERE id=?1",
            params![id]
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "UPDATE client_market_trial_resets SET created_at=?1",
            params![END]
        )
        .is_err()
    );
    assert!(
        conn.execute("DELETE FROM client_market_trial_resets", [])
            .is_err()
    );
}

#[test]
fn a_failed_first_payment_does_not_restore_and_no_minimum_fee_is_required() {
    let conn = fixture();
    let id = prepare(&conn, "unfunded", 20, BillingModel::CalendarMonth);
    contracts::activate_contract_tx(&conn, &id, START, START).unwrap();
    let available = crate::market_wallet::wallet_snapshot_tx(&conn, "buyer")
        .unwrap()
        .available_units;
    crate::market_wallet::reserve_wallet_tx(
        &conn, "buyer", "test", "spend", "hold", available, None, START,
    )
    .unwrap();
    consume(&conn, &id, "healthy", START, END, true);
    assert_eq!((state(&conn).epoch, state(&conn).remaining_seconds), (0, 0));
    assert!(paid_month_tx(&conn, &id, "not-a-paid-period", END).is_err());
    let conn = fixture();
    let id = contracts::prepare_contract_tx(
        &conn,
        &PrepareContract {
            product_kind: "client_host",
            product_ref: "tiny",
            trial_scope_ref: "host",
            quote_id: "tiny",
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: BillingModel::CalendarMonth,
            price_minor: 1,
            auto_renew: false,
            trial_seconds: 0,
            trial_allowance_seconds: 20,
        },
        START,
    )
    .unwrap();
    contracts::activate_contract_tx(&conn, &id, START, START).unwrap();
    assert_eq!(state(&conn).remaining_seconds, 20);
    assert_eq!(state(&conn).epoch, 1);
}

#[test]
fn capture_and_restoration_rollback_together() {
    let conn = fixture();
    let id = prepare(&conn, "rollback", 0, BillingModel::CalendarMonth);
    let before = crate::market_wallet::wallet_snapshot_tx(&conn, "buyer")
        .unwrap()
        .posted_units;
    {
        let tx = conn
            .transaction_with_behavior(crate::db::TransactionBehavior::Immediate)
            .unwrap();
        contracts::activate_contract_tx(&tx, &id, START, START).unwrap();
        assert_eq!(state(&tx).epoch, 1);
        // Dropping an uncommitted transaction must roll both changes back.
    }
    assert_eq!(state(&conn).epoch, 0);
    assert_eq!(
        crate::market_wallet::wallet_snapshot_tx(&conn, "buyer")
            .unwrap()
            .posted_units,
        before
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM market_platform_periods", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn metered_holds_do_not_restore_but_actual_capture_restores_once_per_continuous_rental() {
    let conn = fixture();
    let id = prepare(&conn, "metered", 0, BillingModel::Metered);
    contracts::activate_contract_tx(&conn, &id, START, START).unwrap();
    assert_eq!(state(&conn).epoch, 0);
    for (source, from, to) in [
        ("one", START, "2026-01-01T00:01:00+00:00"),
        (
            "two",
            "2026-01-01T00:01:00+00:00",
            "2026-01-01T00:02:00+00:00",
        ),
    ] {
        contracts::observe_metered_interval_tx(
            &conn,
            &id,
            "client_host_health",
            source,
            from,
            to,
            to,
        )
        .unwrap();
        assert!(
            contracts::settle_metered_batch_tx(&conn, &id, to)
                .unwrap()
                .captured_units
                > 0
        );
        assert_eq!(
            (state(&conn).epoch, state(&conn).remaining_seconds),
            (1, 20)
        );
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM client_market_trial_resets", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn metered_trial_conversion_waits_for_real_usage_capture_not_the_new_budget_hold() {
    let conn = fixture();
    let id = prepare(&conn, "metered-trial", 20, BillingModel::Metered);
    contracts::activate_contract_tx(&conn, &id, START, START).unwrap();
    consume(&conn, &id, "trial", START, END, true);
    assert_eq!((state(&conn).epoch, state(&conn).remaining_seconds), (0, 0));
    let current: (String, i64) = conn
        .query_row(
            "SELECT status,trial_seconds_remaining FROM market_platform_contracts WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(current, ("active".into(), 0));
    let later = "2026-01-01T00:01:20+00:00";
    contracts::observe_metered_interval_tx(
        &conn,
        &id,
        "client_host_health",
        "paid-usage",
        END,
        later,
        later,
    )
    .unwrap();
    assert!(
        contracts::settle_metered_batch_tx(&conn, &id, later)
            .unwrap()
            .captured_units
            > 0
    );
    assert_eq!(
        (state(&conn).epoch, state(&conn).remaining_seconds),
        (1, 20)
    );
}

#[test]
fn share_lifetime_trials_are_not_restored_by_rent_payment() {
    let conn = fixture();
    let id = contracts::prepare_contract_tx(
        &conn,
        &PrepareContract {
            product_kind: "share",
            product_ref: "share-rent",
            trial_scope_ref: "share",
            quote_id: "share-quote",
            buyer_user_id: "buyer",
            provider_user_id: "provider",
            offer_revision: 1,
            billing_model: BillingModel::CalendarMonth,
            price_minor: 1000,
            auto_renew: false,
            trial_seconds: 0,
            trial_allowance_seconds: 20,
        },
        START,
    )
    .unwrap();
    contracts::activate_contract_tx(&conn, &id, START, START).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM client_market_trial_resets", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(for_contract_tx(&conn, &id).unwrap().is_none());
}
