use super::*;

#[test]
fn migration_96_preserves_consumption_and_bootstraps_only_proven_live_rent_payments() {
    let conn = memory_connection();
    install_schema_through(&conn, 95);
    let at = "2026-10-01T00:00:00+00:00";
    crate::market_wallet::credit_verified_topup_tx(&conn, "buyer", "fund", 1000, "receipt", at)
        .unwrap();
    for (id, status, paid) in [
        ("live", "active", true),
        ("past", "ended", true),
        ("trial", "trialing", false),
        ("reserved", "pending_activation", false),
    ] {
        conn.execute("INSERT INTO market_platform_contracts
            (id,product_kind,product_ref,trial_scope_ref,quote_id,request_fingerprint,buyer_user_id,provider_user_id,
             offer_revision,billing_model,cycle_price_minor,renewal_policy,status,created_at,updated_at,
             paid_started_at,trial_seconds_remaining,current_period_sequence)
            VALUES (?1,'client_host',?1,?1,?1,?1,'buyer','provider',1,'calendar_month',1,'manual',?2,?3,?3,?4,?5,?6)",
            params![id,status,at,if paid {Some(at)} else {None},if paid {0} else {20},i64::from(paid)]).unwrap();
        if paid {
            let debit = crate::market_wallet::debit_available_tx(
                &conn,
                "buyer",
                86_400,
                "unfulfilled",
                id,
                "platform_period",
                id,
                "debit",
                at,
            )
            .unwrap();
            conn.execute("INSERT INTO market_platform_periods
                (id,contract_id,sequence,period_start,period_end,settled_through_at,amount_units,debit_journal_id,
                 debited_at,status,created_at,updated_at)
                VALUES (?1,?1,1,?2,'2026-11-01T00:00:00+00:00',?2,86400,?3,?2,'open',?2,?2)",
                params![id,at,debit.id]).unwrap();
        }
        conn.execute("INSERT INTO market_platform_trial_segments
            (id,contract_id,source_kind,source_id,started_at,ended_at,service_available,consumed_seconds,created_at)
            VALUES (?1,?1,'client_host_health',?1,?2,'2026-10-01T00:00:05+00:00',1,5,?2)",params![id,at]).unwrap();
    }
    // An activated metered contract has a budget hold, not necessarily payment.
    for captured in [false, true] {
        let id = if captured {
            "metered-paid"
        } else {
            "metered-held"
        };
        let hold = crate::market_wallet::reserve_wallet_tx(
            &conn,
            "buyer",
            "platform_contract",
            id,
            "activation",
            86_400,
            None,
            at,
        )
        .unwrap();
        conn.execute("INSERT INTO market_platform_contracts
            (id,product_kind,product_ref,trial_scope_ref,quote_id,request_fingerprint,buyer_user_id,provider_user_id,
             offer_revision,billing_model,daily_rate_minor,renewal_policy,status,created_at,updated_at,
             paid_started_at,budget_hold_id,budget_sequence)
            VALUES (?1,'client_host',?1,?1,?1,?1,'buyer','provider',1,'metered',1,'manual','active',?2,?2,?2,?3,1)",
            params![id,at,hold]).unwrap();
        if captured {
            let journal = crate::market_wallet::capture_hold_tx(
                &conn,
                &hold,
                1000,
                "provider_pending",
                "provider",
                "metered_batch",
                id,
                at,
            )
            .unwrap();
            conn.execute("INSERT INTO market_usage_posting_batches
                (id,contract_id,first_accrual_id,last_accrual_id,started_at,ended_at,amount_units,journal_id,created_at)
                VALUES (?1,?1,'first','last',?2,'2026-10-01T00:00:01+00:00',1000,?3,?2)",params![id,at,journal.id]).unwrap();
        }
    }
    let wallet_before = crate::market_wallet::wallet_snapshot_tx(&conn, "buyer")
        .unwrap()
        .posted_units;
    apply(&conn).unwrap();
    apply(&conn).unwrap();
    for (host, epoch, remaining) in [
        ("live", 1, 20),
        ("past", 0, 15),
        ("trial", 0, 15),
        ("reserved", 0, 15),
        ("metered-paid", 1, 20),
        ("metered-held", 0, 20),
    ] {
        let state =
            crate::market_client_trials::state_tx(&conn, "buyer", "provider", host, 20).unwrap();
        assert_eq!(
            (state.epoch, state.remaining_seconds),
            (epoch, remaining),
            "{host}"
        );
    }
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*),SUM(consumed_seconds) FROM market_platform_trial_segments",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        )
        .unwrap(),
        (4, 20)
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM client_market_trial_resets", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        crate::market_wallet::wallet_snapshot_tx(&conn, "buyer")
            .unwrap()
            .posted_units,
        wallet_before
    );
    crate::market_wallet::reconcile_wallet_tx(&conn, "buyer").unwrap();
}
