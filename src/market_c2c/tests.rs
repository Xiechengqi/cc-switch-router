use super::*;
use crate::db::TransactionBehavior;
const AT: &str = "2026-10-22T12:00:00+00:00";
const LATER: &str = "2026-10-22T12:33:00+00:00";
fn fixture() -> Connection {
    fixture_on(Connection::open_in_memory().unwrap())
}
fn fixture_on(c: Connection) -> Connection {
    initialize_fixture(&c);
    c
}
fn initialize_fixture(c: &Connection) {
    crate::schema::apply(c).unwrap();
    c.execute("UPDATE market_c2c_policy SET enabled=1", [])
        .unwrap();
    c.execute(
        "UPDATE market_finance_control SET mode='enabled',cutover_approved_at=?1",
        params![AT],
    )
    .unwrap();
    for id in ["seller", "buyer", "other", "owner"] {
        c.execute("INSERT INTO users(id,email_normalized,status,email_verified_at,created_at,last_login_at) VALUES(?1,?2,'active',?3,?3,?3)",params![id,format!("{id}@example.test"),AT]).unwrap();
    }
    crate::market_wallet::credit_verified_topup_tx(c, "seller", "fixture", 20000, "receipt", AT)
        .unwrap();
}
fn terms() -> SellerInput {
    SellerInput {
        expected_version: 0,
        enabled: true,
        description: "Bank transfer · contact seller@example.test".into(),
        currency: "USD".into(),
        price_bps: 10000,
        min_minor: 100,
        max_minor: 20000,
        acknowledge_external_payment: true,
        auto_accept_terms_version: TERMS_VERSION,
    }
}
fn offer(c: &Connection) {
    save_seller_tx(c, "seller", &terms(), AT).unwrap();
}
#[test]
fn c2c_uses_shared_transfer_controls_not_official_topup_modes_or_market_pauses() {
    let c = fixture();
    for (index, mode) in ["disabled", "shadow", "enabled"].iter().enumerate() {
        c.execute("UPDATE market_finance_control SET mode=?1,cutover_approved_at=NULL,new_orders_paused=1,new_topups_paused=1", params![mode]).unwrap();
        assert!(participation_blockers_tx(&c, "seller").unwrap().is_empty());
        let mut settings = terms();
        settings.expected_version = index as i64;
        save_seller_tx(&c, "seller", &settings, AT).unwrap();
        let o = order(&c, 100);
        c.execute("UPDATE market_wallet_policy SET transfers_paused=1", [])
            .unwrap();
        assert_eq!(
            opening_blockers_tx(&c).unwrap(),
            vec!["C2C_TRANSFERS_PAUSED"]
        );
        assert!(preview_result(&c).is_err());
        let o = paid(&c, &o);
        // Pauses fence only new obligations: an existing reservation still delivers.
        transaction(&c, |c| release_tx(c, &o, "seller", AT)).unwrap();
        c.execute(
            "UPDATE market_wallet_policy SET transfers_paused=0,transfers_enabled=0",
            [],
        )
        .unwrap();
        assert_eq!(
            opening_blockers_tx(&c).unwrap(),
            vec!["C2C_TRANSFERS_DISABLED"]
        );
        assert!(preview_result(&c).is_err());
        c.execute("UPDATE market_wallet_policy SET transfers_enabled=1", [])
            .unwrap();
        crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
        crate::market_wallet::reconcile_wallet_tx(&c, "buyer").unwrap();
    }
    c.execute("UPDATE market_c2c_policy SET enabled=0", [])
        .unwrap();
    assert_eq!(opening_blockers_tx(&c).unwrap(), vec!["C2C_DISABLED"]);
    assert!(offers_tx(&c, "buyer", "", AT).unwrap().0.is_empty());
}

#[test]
fn seller_capability_and_publication_share_exact_admission_and_hold_rules() {
    let c = fixture();
    c.execute("UPDATE market_c2c_policy SET admission_minor=20001", [])
        .unwrap();
    assert_eq!(
        seller_blockers_tx(&c, "seller", AT).unwrap(),
        vec!["C2C_ADMISSION"]
    );
    assert_eq!(
        save_seller_tx(&c, "seller", &terms(), AT)
            .unwrap_err()
            .code(),
        Some("C2C_ADMISSION")
    );
    c.execute("UPDATE market_c2c_policy SET admission_minor=20000", [])
        .unwrap();
    assert!(seller_blockers_tx(&c, "seller", AT).unwrap().is_empty());
    offer(&c); // equality qualifies; no admission reserve is created.
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    let o = order(&c, 1000);
    assert!(
        seller_blockers_tx(&c, "seller", AT).unwrap().is_empty(),
        "existing sellers can edit below admission"
    );
    let reserved = wallet_snapshot_tx(&c, "seller").unwrap().held_units;
    crate::market_reconcile::case_tx(&c, "wallet_projection_mismatch", "wallet", "seller", AT)
        .unwrap();
    assert!(
        seller_blockers_tx(&c, "seller", AT)
            .unwrap()
            .contains(&"C2C_FINANCE_HOLD")
    );
    let mut edit = terms();
    edit.expected_version = 1;
    assert_eq!(
        save_seller_tx(&c, "seller", &edit, AT).unwrap_err().code(),
        Some("C2C_FINANCE_HOLD")
    );
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        reserved
    );
    assert!(
        participation_blockers_tx(&c, "buyer").unwrap().is_empty(),
        "a seller hold does not block unrelated users"
    );
    c.execute(
        "UPDATE market_c2c_sellers SET suspended=1 WHERE user_id='seller'",
        [],
    )
    .unwrap();
    assert!(
        seller_blockers_tx(&c, "seller", AT)
            .unwrap()
            .contains(&"C2C_SUSPENDED")
    );
    c.execute("UPDATE users SET status='blocked' WHERE id='seller'", [])
        .unwrap();
    assert!(
        seller_blockers_tx(&c, "seller", AT)
            .unwrap()
            .contains(&"C2C_ACCOUNT_INACTIVE")
    );
    assert_eq!(
        save_seller_tx(&c, "seller", &edit, AT).unwrap_err().code(),
        Some("C2C_ACCOUNT_INACTIVE")
    );
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "awaiting_payment");
}
#[test]
fn one_public_description_is_required_and_seller_identity_is_server_owned() {
    let c = fixture();
    let mut input = terms();
    input.description = "  支持微信\n联系 @seller\t https://example.test  ".into();
    let first = save_seller_tx(&c, "seller", &input, AT).unwrap();
    assert_eq!(
        first.description,
        "支持微信 联系 @seller https://example.test"
    );
    assert!(first.display_name.starts_with("C2C-"));
    assert!(!first.display_name.contains("seller"));
    input.expected_version = first.version;
    input.description = "New contact".into();
    let second = save_seller_tx(&c, "seller", &input, AT).unwrap();
    assert_eq!(first.display_name, second.display_name);
    input.expected_version = second.version;
    for value in [
        "".to_string(),
        " \n ".to_string(),
        "字".repeat(301),
        "bad\0data".to_string(),
    ] {
        input.description = value;
        assert!(save_seller_tx(&c, "seller", &input, AT).is_err());
    }
    input.description = "字".repeat(300);
    assert!(save_seller_tx(&c, "seller", &input, AT).is_ok());
    let stale_shape = serde_json::json!({"expectedVersion":0,"enabled":true,"displayName":"name","contact":"private","paymentMethod":"bank","paymentDetails":"account","currency":"USD","priceBps":10000,"minMinor":100,"maxMinor":10000,"acknowledgeExternalPayment":true});
    assert!(serde_json::from_value::<SellerInput>(stale_shape).is_err());
}
#[test]
fn pausing_needs_only_ownership_and_version_and_never_changes_existing_reservations() {
    let c = fixture();
    offer(&c);
    let o = order(&c, 1000);
    let before = wallet_snapshot_tx(&c, "seller").unwrap();
    c.execute(
        "UPDATE market_c2c_sellers SET description='',suspended=1 WHERE user_id='seller'",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE market_c2c_policy SET enabled=0,admission_minor=1000000",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE users SET email_verified_at=NULL WHERE id='seller'",
        [],
    )
    .unwrap();
    assert!(pause_seller_tx(&c, "other", 1, AT).is_err());
    assert!(pause_seller_tx(&c, "seller", 99, AT).is_err());
    let paused = pause_seller_tx(&c, "seller", 1, AT).unwrap();
    assert!(!paused.enabled);
    assert_eq!(paused.version, 2);
    assert_eq!(pause_seller_tx(&c, "seller", 2, AT).unwrap().version, 2);
    let after = wallet_snapshot_tx(&c, "seller").unwrap();
    assert_eq!(
        (before.posted_units, before.held_units, before.version),
        (after.posted_units, after.held_units, after.version)
    );
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "awaiting_payment");
    assert!(offers_tx(&c, "buyer", "", AT).unwrap().0.is_empty());
    cancel_tx(&c, &o, "buyer", false, AT).unwrap();
    crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
}
fn preview(c: &Connection, buyer: &str, minor: i64) -> Quote {
    quote_tx(
        c,
        buyer,
        &QuoteInput {
            seller_id: "seller".into(),
            amount_minor: minor,
        },
        AT,
    )
    .unwrap()
}
fn input(q: &Quote) -> CreateInput {
    CreateInput {
        quote_id: q.id.clone(),
        terms_version: TERMS_VERSION,
        acknowledge_external_payment: true,
    }
}
fn order(c: &Connection, minor: i64) -> Order {
    application(c, minor)
}
fn application(c: &Connection, minor: i64) -> Order {
    let q = preview(c, "buyer", minor);
    create_tx(
        c,
        "buyer",
        &input(&q),
        &Uuid::new_v4().to_string(),
        "127.0.0.1",
        AT,
    )
    .unwrap()
}
// Historical v2 applications are still honoured, but cannot be created by
// the current API. Seed their original frozen quote/reservation as migration
// fixtures rather than mutating an immutable v3 quote.
fn legacy_application(c: &Connection, minor: i64) -> Order {
    let q = preview(c, "buyer", minor);
    let quote_id = Uuid::new_v4().to_string();
    let id = Uuid::new_v4().to_string();
    c.execute("INSERT INTO market_c2c_quotes(id,buyer_id,seller_id,seller_version,policy_version,credit_minor,pay_minor,currency,fx_micros,price_bps,seller_name,description,contact,payment_method,payment_details,payment_minutes,expires_at,created_at,flow_version,acceptance_minutes)
        SELECT ?1,buyer_id,seller_id,seller_version,policy_version,credit_minor,pay_minor,currency,fx_micros,price_bps,seller_name,description,contact,payment_method,payment_details,payment_minutes,expires_at,created_at,2,10 FROM market_c2c_quotes WHERE id=?2",params![quote_id,q.id]).unwrap();
    let amount = units_from_minor(minor).unwrap();
    let hold = crate::market_wallet::reserve_wallet_tx(
        c,
        "seller",
        "c2c_order",
        &id,
        "c2c",
        amount,
        None,
        AT,
    )
    .unwrap();
    let deadline = (at(AT).unwrap() + Duration::minutes(10)).to_rfc3339();
    c.execute("INSERT INTO market_c2c_orders(id,quote_id,buyer_id,seller_id,idempotency_key,amount_units,hold_id,status,terms_version,acceptance_deadline,created_ip,created_at,updated_at)
        VALUES(?1,?2,'buyer','seller',?1,?3,?4,'awaiting_seller',2,?5,'127.0.0.1',?6,?6)",params![id,quote_id,amount,hold,deadline,AT]).unwrap();
    let o = order_tx(c, &id).unwrap();
    event_tx(
        c,
        &o,
        "buyer",
        "created",
        "terms_v2_acceptance_required",
        AT,
    )
    .unwrap();
    o
}
fn paid(c: &Connection, o: &Order) -> Order {
    let accepted = if o.status == "awaiting_seller" {
        accept_tx(c, o, "seller", o.version, AT).unwrap()
    } else {
        o.clone()
    };
    let o = &accepted;
    mark_paid_tx(c, o, "buyer", o.version, AT).unwrap()
}

#[test]
fn automatic_creation_reserves_credit_and_starts_the_frozen_payment_window_atomically() {
    let c = fixture();
    offer(&c);
    let q = preview(&c, "buyer", 1000);
    assert_eq!((q.flow_version, q.payment_minutes), (3, 30));
    let created = "2026-10-22T12:04:00Z";
    let o = transaction(&c, |c| {
        create_tx(
            c,
            "buyer",
            &input(&q),
            "automatic-key",
            "127.0.0.1",
            created,
        )
    })
    .unwrap();
    assert_eq!((o.status.as_str(), o.flow_version), ("awaiting_payment", 3));
    assert!(o.accepted_at.is_none() && o.acceptance_deadline.is_none());
    assert_eq!(
        at(o.payment_deadline.as_deref().unwrap()).unwrap(),
        at(created).unwrap() + Duration::minutes(30)
    );
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        o.amount_units
    );
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
    let authorization: String = c
        .query_row(
            "SELECT auto_accept_authorized_at FROM market_c2c_quotes WHERE id=?1",
            params![q.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(authorization, AT);
    assert_eq!(
        c.query_row(
            "SELECT action,note FROM market_c2c_events WHERE order_id=?1",
            params![o.id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        )
        .unwrap(),
        (
            "created".into(),
            "terms_v3_standing_auto_accept_authorization".into()
        )
    );
    assert!(accept_tx(&c, &o, "seller", o.version, created).is_err());
    assert!(transaction(&c, |c| cancel_tx(c, &o, "seller", false, created)).is_err());
    c.execute(
        "UPDATE market_c2c_policy SET payment_minutes=5,version=version+1",
        [],
    )
    .unwrap();
    let replay = create_tx(&c, "buyer", &input(&q), "automatic-key", "127.0.0.1", LATER).unwrap();
    assert_eq!(
        (replay.id, replay.version, replay.payment_deadline),
        (o.id.clone(), o.version, o.payment_deadline.clone())
    );
    assert!(
        c.execute(
            "UPDATE market_c2c_orders SET payment_deadline='2026-10-22T13:00:00Z' WHERE id=?1",
            params![o.id]
        )
        .is_err()
    );
    assert!(
        c.execute(
            "UPDATE market_c2c_orders SET accepted_at=?2 WHERE id=?1",
            params![o.id, created]
        )
        .is_err()
    );
    let o = mark_paid_tx(&c, &o, "buyer", o.version, created).unwrap();
    let s = seller_tx(&c, "seller").unwrap().unwrap();
    pause_seller_tx(&c, "seller", s.version, created).unwrap();
    c.execute(
        "UPDATE market_c2c_sellers SET suspended=1 WHERE user_id='seller'",
        [],
    )
    .unwrap();
    transaction(&c, |c| release_tx(c, &o, "seller", created)).unwrap();
    crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
    crate::market_wallet::reconcile_wallet_tx(&c, "buyer").unwrap();
}

#[test]
fn unpaid_automatic_expiry_returns_credit_once_without_pausing_the_seller() {
    let c = fixture();
    offer(&c);
    let o = order(&c, 1000);
    transaction(&c, |c| tick_tx(c, "2026-10-22T12:31:59Z")).unwrap();
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "awaiting_payment");
    for _ in 0..2 {
        transaction(&c, |c| tick_tx(c, LATER)).unwrap();
    }
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "expired");
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    let s = seller_tx(&c, "seller").unwrap().unwrap();
    assert!(s.enabled && s.auto_pause_reason.is_none());
    assert_eq!(s.version, 1);
    assert_eq!(offers_tx(&c, "buyer", "", LATER).unwrap().0.len(), 1);
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM market_c2c_events WHERE order_id=?1 AND action='expired'",
            params![o.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
    crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
}

#[test]
fn automatic_consent_is_explicit_and_reconfirmation_preserves_safety_pauses() {
    let c = fixture();
    let mut settings = terms();
    settings.auto_accept_terms_version = 2;
    assert!(save_seller_tx(&c, "seller", &settings, AT).is_err());
    let mut json = serde_json::json!({"expectedVersion":0,"enabled":true,"description":"Bank","currency":"USD","priceBps":10000,"minMinor":100,"maxMinor":20000,"acknowledgeExternalPayment":true});
    assert!(serde_json::from_value::<SellerInput>(json.clone()).is_err());
    json["autoAcceptTermsVersion"] = 3.into();
    assert!(serde_json::from_value::<SellerInput>(json).is_ok());
    offer(&c);
    let q = preview(&c, "buyer", 1000);
    c.execute("UPDATE market_c2c_sellers SET auto_accept_terms_version=0,auto_accept_consented_at=NULL WHERE user_id='seller'",[]).unwrap();
    assert!(offers_tx(&c, "buyer", "", AT).unwrap().0.is_empty());
    assert!(preview_result(&c).is_err());
    assert!(
        transaction(&c, |c| create_tx(
            c,
            "buyer",
            &input(&q),
            "no-consent-key",
            "ip",
            AT
        ))
        .is_err()
    );
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    c.execute("UPDATE market_c2c_policy SET admission_minor=1000000", [])
        .unwrap();
    c.execute("UPDATE market_c2c_sellers SET auto_pause_reason='acceptance_timeout',auto_paused_at=?1,auto_pause_order_id='old-order' WHERE user_id='seller'",params![AT]).unwrap();
    assert_eq!(
        resume_seller_tx(&c, "seller", 1, AT).unwrap_err().code(),
        Some("C2C_AUTO_ACCEPT_CONSENT")
    );
    settings = terms();
    settings.expected_version = 1;
    let s = save_seller_tx(&c, "seller", &settings, LATER).unwrap();
    assert!(s.enabled && s.auto_pause_reason.is_some());
    assert_eq!(s.auto_accept_terms_version, 3);
    assert_eq!(s.auto_accept_consented_at.as_deref(), Some(LATER));
    assert!(offers_tx(&c, "buyer", "", LATER).unwrap().0.is_empty());
    let s = resume_seller_tx(&c, "seller", s.version, LATER).unwrap();
    assert!(s.auto_pause_reason.is_none());
    assert_eq!(offers_tx(&c, "buyer", "", LATER).unwrap().0.len(), 1);
    assert!(
        transaction(&c, |c| create_tx(
            c,
            "buyer",
            &input(&q),
            "stale-consent-key",
            "ip",
            AT
        ))
        .is_err()
    );
    let s = pause_seller_tx(&c, "seller", s.version, LATER).unwrap();
    settings.expected_version = s.version;
    assert_eq!(
        save_seller_tx(&c, "seller", &settings, LATER)
            .unwrap_err()
            .code(),
        Some("C2C_ADMISSION")
    );
    assert!(!seller_tx(&c, "seller").unwrap().unwrap().enabled);
}

#[test]
fn historical_terms_can_only_recover_an_existing_order_not_create_new_obligations() {
    let c = fixture();
    offer(&c);
    let q = preview(&c, "buyer", 1000);
    for terms in [1, 2] {
        let mut old = input(&q);
        old.terms_version = terms;
        assert_eq!(
            transaction(&c, |c| create_tx(
                c,
                "buyer",
                &old,
                "old-terms-key",
                "ip",
                AT
            ))
            .unwrap_err()
            .code(),
            Some("C2C_TERMS")
        );
    }
    let o = legacy_application(&c, 1000);
    let old = CreateInput {
        quote_id: o.quote_id.clone(),
        terms_version: 2,
        acknowledge_external_payment: true,
    };
    let recovered = create_tx(&c, "buyer", &old, &o.id, "ip", LATER).unwrap();
    assert_eq!(
        (recovered.id, recovered.acceptance_deadline),
        (o.id.clone(), o.acceptance_deadline)
    );
    let mut unconsumed = old;
    unconsumed.quote_id = q.id;
    assert!(create_tx(&c, "buyer", &unconsumed, "other-old-key", "ip", AT).is_err());
}

#[test]
fn legacy_application_keeps_its_original_acceptance_and_frozen_payment_window() {
    let c = fixture();
    offer(&c);
    let o = legacy_application(&c, 1000);
    assert_eq!(o.status, "awaiting_seller");
    assert!(o.payment_deadline.is_none() && o.accepted_at.is_none());
    assert_eq!(
        at(o.acceptance_deadline.as_deref().unwrap()).unwrap(),
        at(AT).unwrap() + Duration::minutes(10)
    );
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        o.amount_units
    );
    assert!(mark_paid_tx(&c, &o, "buyer", o.version, AT).is_err());
    assert!(accept_tx(&c, &o, "buyer", o.version, AT).is_err());
    assert!(accept_tx(&c, &o, "seller", o.version + 1, AT).is_err());
    c.execute(
        "UPDATE market_c2c_policy SET payment_minutes=5,version=version+1",
        [],
    )
    .unwrap();
    let accepted_at = "2026-10-22T12:09:00Z";
    let accepted = transaction(&c, |c| accept_tx(c, &o, "seller", o.version, accepted_at)).unwrap();
    assert_eq!(accepted.status, "awaiting_payment");
    assert_eq!(
        at(accepted.payment_deadline.as_deref().unwrap()).unwrap(),
        at(accepted_at).unwrap() + Duration::minutes(30)
    );
    let replay = accept_tx(
        &c,
        &order_tx(&c, &o.id).unwrap(),
        "seller",
        o.version,
        LATER,
    )
    .unwrap();
    assert_eq!(
        (replay.version, replay.payment_deadline),
        (accepted.version, accepted.payment_deadline)
    );
    assert!(
        c.execute(
            "UPDATE market_c2c_orders SET payment_deadline='2026-10-22T14:00:00Z' WHERE id=?1",
            params![o.id]
        )
        .is_err()
    );
    assert!(
        c.execute(
            "UPDATE market_c2c_orders SET acceptance_deadline='2026-10-22T14:00:00Z' WHERE id=?1",
            params![o.id]
        )
        .is_err()
    );
    crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
}

#[test]
fn unpaid_application_withdrawal_rejection_and_acceptance_races_do_not_double_release() {
    let c = fixture();
    offer(&c);
    for actor in ["buyer", "seller"] {
        let o = legacy_application(&c, 1000);
        let cancelled = transaction(&c, |c| cancel_tx(c, &o, actor, false, AT)).unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(transaction(&c, |c| accept_tx(c, &o, "seller", o.version, AT)).is_err());
        let again = cancel_tx(&c, &cancelled, actor, false, AT).unwrap();
        assert_eq!(again.version, cancelled.version);
        assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
        assert!(
            seller_tx(&c, "seller")
                .unwrap()
                .unwrap()
                .auto_pause_reason
                .is_none()
        );
    }
    let o = legacy_application(&c, 1000);
    let accepted = transaction(&c, |c| accept_tx(c, &o, "seller", o.version, AT)).unwrap();
    assert!(transaction(&c, |c| cancel_tx(c, &o, "seller", false, AT)).is_err());
    assert_eq!(order_tx(&c, &o.id).unwrap().version, accepted.version);
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        o.amount_units
    );
    crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
}

#[test]
fn acceptance_timeout_returns_credit_once_and_explicit_resume_does_not_reapply_admission() {
    let c = fixture();
    offer(&c);
    let o = legacy_application(&c, 1000);
    let deadline = o.acceptance_deadline.as_deref().unwrap();
    assert!(accept_tx(&c, &o, "seller", o.version, deadline).is_err());
    transaction(&c, |c| tick_tx(c, deadline)).unwrap();
    transaction(&c, |c| tick_tx(c, LATER)).unwrap();
    let expired = order_tx(&c, &o.id).unwrap();
    assert_eq!(expired.status, "expired");
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    let s = seller_tx(&c, "seller").unwrap().unwrap();
    assert!(s.enabled);
    assert_eq!(s.auto_pause_reason.as_deref(), Some("acceptance_timeout"));
    assert!(offers_tx(&c, "buyer", "", LATER).unwrap().0.is_empty());
    c.execute("UPDATE market_c2c_policy SET admission_minor=1000000", [])
        .unwrap();
    let s = transaction(&c, |c| resume_seller_tx(c, "seller", s.version, LATER)).unwrap();
    assert!(s.enabled && s.auto_pause_reason.is_none());
    assert_eq!(offers_tx(&c, "buyer", "", LATER).unwrap().0.len(), 1);
    let events: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM market_c2c_events WHERE order_id=?1 AND action='seller_timeout'",
            params![o.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(events, 1);
    let s = pause_seller_tx(&c, "seller", s.version, LATER).unwrap();
    assert!(resume_seller_tx(&c, "seller", s.version, LATER).is_err());
    assert!(!s.enabled);
}

#[test]
fn timeout_never_overrides_a_manual_pause_or_admin_suspension() {
    let c = fixture();
    offer(&c);
    let o = application(&c, 1000);
    let s = pause_seller_tx(&c, "seller", 1, AT).unwrap();
    transaction(&c, |c| tick_tx(c, LATER)).unwrap();
    let after = seller_tx(&c, "seller").unwrap().unwrap();
    assert_eq!(after.version, s.version);
    assert!(!after.enabled && after.auto_pause_reason.is_none());
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "expired");
    let mut settings = terms();
    settings.expected_version = s.version;
    save_seller_tx(&c, "seller", &settings, LATER).unwrap();
    let q = quote_tx(
        &c,
        "buyer",
        &QuoteInput {
            seller_id: "seller".into(),
            amount_minor: 1000,
        },
        LATER,
    )
    .unwrap();
    let o = create_tx(&c, "buyer", &input(&q), "suspended-key", "127.0.0.1", LATER).unwrap();
    c.execute(
        "UPDATE market_c2c_sellers SET suspended=1 WHERE user_id='seller'",
        [],
    )
    .unwrap();
    transaction(&c, |c| tick_tx(c, "2026-10-22T14:00:00Z")).unwrap();
    let s = seller_tx(&c, "seller").unwrap().unwrap();
    assert!(s.suspended && s.auto_pause_reason.is_none());
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "expired");
}

#[test]
fn paid_nonresponse_reminders_pause_and_escalation_never_release_reserved_credit() {
    let c = fixture();
    offer(&c);
    let o = paid(&c, &order(&c, 1000));
    for (time, paused) in [
        ("2026-10-22T12:29:59Z", false),
        ("2026-10-22T12:30:01Z", false),
        ("2026-10-22T13:00:01Z", true),
        ("2026-10-23T12:00:01Z", true),
        ("2026-10-24T12:00:01Z", true),
    ] {
        transaction(&c, |c| tick_tx(c, time)).unwrap();
        assert_eq!(order_tx(&c, &o.id).unwrap().status, "paid");
        assert_eq!(
            wallet_snapshot_tx(&c, "seller").unwrap().held_units,
            o.amount_units
        );
        assert_eq!(
            seller_tx(&c, "seller")
                .unwrap()
                .unwrap()
                .auto_pause_reason
                .is_some(),
            paused
        );
    }
    for action in ["release_reminder", "overdue_escalated"] {
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM market_c2c_events WHERE order_id=?1 AND action=?2",
                params![o.id, action],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
    let s = seller_tx(&c, "seller").unwrap().unwrap();
    assert_eq!(
        resume_seller_tx(&c, "seller", s.version, "2026-10-24T12:00:01Z")
            .unwrap_err()
            .code(),
        Some("C2C_OVERDUE_ORDERS")
    );
    let paused = pause_seller_tx(&c, "seller", s.version, "2026-10-24T12:00:01Z").unwrap();
    let mut settings = terms();
    settings.expected_version = paused.version;
    assert_eq!(
        save_seller_tx(&c, "seller", &settings, "2026-10-24T12:00:01Z")
            .unwrap_err()
            .code(),
        Some("C2C_OVERDUE_ORDERS")
    );
    let o = transaction(&c, |c| release_tx(c, &o, "seller", "2026-10-24T12:00:01Z")).unwrap();
    assert_eq!(o.status, "released");
    assert!(save_seller_tx(&c, "seller", &settings, "2026-10-24T12:00:01Z").is_ok());
    crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
    crate::market_wallet::reconcile_wallet_tx(&c, "buyer").unwrap();
}

#[test]
fn early_external_payment_requires_independent_resolution_and_keeps_the_hold() {
    let c = fixture();
    offer(&c);
    let o = legacy_application(&c, 1000);
    let o = transaction(&c, |c| {
        late_payment_tx(
            c,
            &o,
            "buyer",
            "Payment was sent before acceptance; reference kept by parties",
            AT,
        )
    })
    .unwrap();
    assert_eq!(o.status, "disputed");
    assert!(o.accepted_at.is_none() && o.payment_deadline.is_none());
    transaction(&c, |c| tick_tx(c, "2026-10-24T12:00:00Z")).unwrap();
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        o.amount_units
    );
    assert!(transaction(&c, |c| release_tx(c, &o, "seller", LATER)).is_err());
    assert!(
        transaction(&c, |c| resolve_tx(
            c, &o, "buyer", o.version, "release", "Evidence", true, LATER
        ))
        .is_err()
    );
    let o = transaction(&c, |c| {
        resolve_tx(
            c,
            &o,
            "owner",
            o.version,
            "release",
            "Independent confirmation from both parties",
            true,
            LATER,
        )
    })
    .unwrap();
    assert!(o.late_payment_resolved && o.status == "released");
    assert_eq!(
        wallet_snapshot_tx(&c, "buyer").unwrap().available_units,
        o.amount_units
    );
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
}

#[test]
fn narrow_dispute_grants_are_versioned_revocable_and_never_allow_self_delegation() {
    let c = fixture();
    assert!(!dispute_handler_tx(&c, "other").unwrap());
    assert!(set_dispute_handler_tx(&c, "owner", "owner", 0, true, AT).is_err());
    set_dispute_handler_tx(&c, "owner", "other", 0, true, AT).unwrap();
    assert!(dispute_handler_tx(&c, "other").unwrap());
    assert!(set_dispute_handler_tx(&c, "owner", "other", 0, false, AT).is_err());
    c.execute("UPDATE users SET status='blocked' WHERE id='other'", [])
        .unwrap();
    assert!(!dispute_handler_tx(&c, "other").unwrap());
    set_dispute_handler_tx(&c, "owner", "other", 1, false, AT).unwrap();
    assert!(!dispute_handler_tx(&c, "other").unwrap());
    assert_eq!(c.query_row("SELECT COUNT(*) FROM market_finance_audit_events WHERE object_kind='c2c_dispute_handler'", [], |r|r.get::<_,i64>(0)).unwrap(), 2);
}

#[test]
fn persistent_listings_survive_restart_and_orders_keep_their_own_deadlines() {
    let directory = std::env::temp_dir().join(format!("router-c2c-persistence-{}", Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("finance.sqlite");
    let c = fixture_on(Connection::open(&path).unwrap());
    offer(&c);
    let o = application(&c, 1000);
    drop(c);
    let c = Connection::open(&path).unwrap();
    crate::schema::apply(&c).unwrap();
    assert_eq!(
        order_tx(&c, &o.id).unwrap().payment_deadline,
        o.payment_deadline
    );
    transaction(&c, |c| tick_tx(c, LATER)).unwrap();
    let s = seller_tx(&c, "seller").unwrap().unwrap();
    assert!(s.enabled && s.auto_pause_reason.is_none());
    drop(c);
    let c = Connection::open(&path).unwrap();
    crate::schema::apply(&c).unwrap();
    let distant = "2026-11-22T12:00:00Z";
    assert_eq!(offers_tx(&c, "buyer", "", distant).unwrap().0.len(), 1);
    assert_eq!(offers_tx(&c, "owner", "", distant).unwrap().0.len(), 1);
    assert!(offers_tx(&c, "seller", "", distant).unwrap().0.is_empty());
    let q = quote_tx(
        &c,
        "other",
        &QuoteInput {
            seller_id: "seller".into(),
            amount_minor: 1000,
        },
        distant,
    )
    .unwrap();
    let o = transaction(&c, |c| {
        create_tx(
            c,
            "other",
            &input(&q),
            "persistent-buy-key",
            "127.0.0.1",
            distant,
        )
    })
    .unwrap();
    assert_eq!(o.status, "awaiting_payment");
    drop(c);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn independent_connections_serialize_acceptance_against_application_cancellation() {
    let directory = std::env::temp_dir().join(format!("router-c2c-accept-race-{}", Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("finance.sqlite");
    let c = fixture_on(Connection::open(&path).unwrap());
    offer(&c);
    let o = legacy_application(&c, 1000);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let tasks = (0..2)
        .map(|i| {
            let db = Connection::open(&path).unwrap();
            let o = o.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let Ok(tx) = db.transaction_with_behavior(TransactionBehavior::Immediate) else {
                    return false;
                };
                let result = if i == 0 {
                    accept_tx(&tx, &o, "seller", o.version, AT)
                } else {
                    cancel_tx(&tx, &o, "buyer", false, AT)
                };
                if result.is_err() {
                    return false;
                }
                tx.commit().unwrap();
                true
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .filter(|ok| *ok)
            .count(),
        1
    );
    let final_order = order_tx(&c, &o.id).unwrap();
    assert!(matches!(
        final_order.status.as_str(),
        "awaiting_payment" | "cancelled"
    ));
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        if final_order.status == "awaiting_payment" {
            o.amount_units
        } else {
            0
        }
    );
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
    crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
    drop(c);
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn automatic_last_stock_and_payment_cancellation_races_preserve_one_reservation() {
    let directory =
        std::env::temp_dir().join(format!("router-c2c-automatic-races-{}", Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("finance.sqlite");
    let c = fixture_on(Connection::open(&path).unwrap());
    offer(&c);
    crate::market_wallet_transfers::verified_transfer_for_test(
        &c,
        "seller",
        "other",
        units_from_minor(10000).unwrap(),
        AT,
    )
    .unwrap();
    let quotes = [preview(&c, "buyer", 10000), preview(&c, "other", 10000)];
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let tasks = quotes
        .into_iter()
        .enumerate()
        .map(|(i, q)| {
            let db = Connection::open(&path).unwrap();
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                let Ok(tx) = db.transaction_with_behavior(TransactionBehavior::Immediate) else {
                    return false;
                };
                let buyer = if i == 0 { "buyer" } else { "other" };
                if create_tx(&tx, buyer, &input(&q), "last-stock-key", "127.0.0.1", AT).is_err() {
                    return false;
                }
                tx.commit().unwrap();
                true
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .filter(|ok| *ok)
            .count(),
        1
    );
    let id: String = c
        .query_row("SELECT id FROM market_c2c_orders", [], |r| r.get(0))
        .unwrap();
    let o = order_tx(&c, &id).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM market_c2c_orders", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        o.amount_units
    );
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let tasks = (0..2)
        .map(|i| {
            let db = Connection::open(&path).unwrap();
            let gate = gate.clone();
            let o = o.clone();
            std::thread::spawn(move || {
                gate.wait();
                let Ok(tx) = db.transaction_with_behavior(TransactionBehavior::Immediate) else {
                    return false;
                };
                let result = if i == 0 {
                    mark_paid_tx(&tx, &o, &o.buyer_id, o.version, AT)
                } else {
                    cancel_tx(&tx, &o, &o.buyer_id, false, AT)
                };
                if result.is_err() {
                    return false;
                }
                tx.commit().unwrap();
                true
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .filter(|ok| *ok)
            .count(),
        1
    );
    let final_order = order_tx(&c, &o.id).unwrap();
    assert!(matches!(final_order.status.as_str(), "paid" | "cancelled"));
    transaction(&c, |c| tick_tx(c, "2026-10-24T12:00:00Z")).unwrap();
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        if final_order.status == "paid" {
            o.amount_units
        } else {
            0
        }
    );
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
    for user in ["seller", "buyer", "other"] {
        crate::market_wallet::reconcile_wallet_tx(&c, user).unwrap();
    }
    drop(c);
    std::fs::remove_dir_all(directory).unwrap();
}

fn transaction<T>(
    c: &Connection,
    f: impl FnOnce(&Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let v = f(&tx)?;
    tx.commit().unwrap();
    Ok(v)
}
#[test]
fn automatically_credited_earnings_can_open_c2c_and_deliver_existing_balance() {
    let c = fixture();
    c.execute("INSERT INTO market_provider_profiles(id,canonical_email,user_id,display_name,claim_state,status,created_at,updated_at)
        VALUES('profile','other@example.test','other','Provider','claimed','active',?1,?1)",params![AT]).unwrap();
    crate::market_finance::record_metered_delivery_tx(
        &c,
        "share",
        "historical-service",
        "earned",
        "seller",
        "other",
        "2026-10-22T11:59:00Z",
        "2026-10-22T12:00:00Z",
        units_from_minor(12000).unwrap(),
        AT,
    )
    .unwrap();
    assert!(save_seller_tx(&c, "other", &terms(), AT).is_err());
    let settled = "2026-10-30T12:00:00Z";
    crate::market_finance::unlock_due_earnings_isolated(&c, settled, 200).unwrap();
    assert_eq!(
        crate::market_earning_conversions::credit_due_earnings_isolated(&c, settled, 200).unwrap(),
        1
    );
    save_seller_tx(&c, "other", &terms(), settled).unwrap();
    let q = quote_tx(
        &c,
        "buyer",
        &QuoteInput {
            seller_id: "other".into(),
            amount_minor: 10000,
        },
        settled,
    )
    .unwrap();
    let o = create_tx(
        &c,
        "buyer",
        &input(&q),
        "income-c2c-order",
        "127.0.0.1",
        settled,
    )
    .unwrap();
    let o = mark_paid_tx(&c, &o, "buyer", o.version, settled).unwrap();
    release_tx(&c, &o, "other", settled).unwrap();
    assert_eq!(
        wallet_snapshot_tx(&c, "other").unwrap().available_units,
        units_from_minor(2000).unwrap()
    );
    assert_eq!(
        wallet_snapshot_tx(&c, "buyer").unwrap().available_units,
        units_from_minor(10000).unwrap()
    );
    crate::market_wallet::reconcile_wallet_tx(&c, "other").unwrap();
    crate::market_wallet::reconcile_wallet_tx(&c, "buyer").unwrap();
    assert_eq!(
        crate::market_reconcile::audit_finance_tx(&c, settled).unwrap(),
        0
    );
}
#[test]
fn ordinary_users_can_sell_without_provider_identity_and_admission_is_not_a_reserve() {
    let c = fixture();
    offer(&c);
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM market_provider_profiles", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    let o = order(&c, 10000);
    let o = paid(&c, &o);
    release_tx(&c, &o, "seller", AT).unwrap();
    let o = order(&c, 5000);
    let o = paid(&c, &o);
    release_tx(&c, &o, "seller", AT).unwrap();
    assert_eq!(
        offers_tx(&c, "buyer", "", AT).unwrap().0[0].available_minor,
        5000
    );
    let mut s = terms();
    s.expected_version = 1;
    s.enabled = false;
    save_seller_tx(&c, "seller", &s, AT).unwrap();
    s.enabled = true;
    s.expected_version = 2;
    assert_eq!(
        save_seller_tx(&c, "seller", &s, AT).unwrap_err().code(),
        Some("C2C_ADMISSION")
    );
}
#[test]
fn admission_does_not_depend_on_per_transfer_limit_and_does_not_count_held_credit() {
    let c = fixture();
    c.execute(
        "UPDATE market_wallet_policy SET transfer_max_minor=1000",
        [],
    )
    .unwrap();
    offer(&c);
    let mut s = terms();
    s.expected_version = 1;
    s.enabled = false;
    save_seller_tx(&c, "seller", &s, AT).unwrap();
    crate::market_wallet::reserve_wallet_tx(
        &c,
        "seller",
        "test",
        "held",
        "hold",
        units_from_minor(11000).unwrap(),
        None,
        AT,
    )
    .unwrap();
    s.expected_version = 2;
    s.enabled = true;
    assert_eq!(
        save_seller_tx(&c, "seller", &s, AT).unwrap_err().code(),
        Some("C2C_ADMISSION")
    );
}
#[test]
fn admission_identity_suspension_and_self_purchase_fail_closed_without_presence_gate() {
    let c = fixture();
    assert!(save_seller_tx(&c, "buyer", &terms(), AT).is_err());
    offer(&c);
    assert!(
        quote_tx(
            &c,
            "seller",
            &QuoteInput {
                seller_id: "seller".into(),
                amount_minor: 100
            },
            AT
        )
        .is_err()
    );
    assert_eq!(offers_tx(&c, "buyer", "", LATER).unwrap().0.len(), 1);
    c.execute(
        "UPDATE market_c2c_sellers SET suspended=1 WHERE user_id='seller'",
        [],
    )
    .unwrap();
    assert!(preview_result(&c).is_err());
    c.execute("UPDATE market_c2c_sellers SET suspended=0", [])
        .unwrap();
    c.execute(
        "UPDATE users SET email_verified_at=NULL WHERE id='buyer'",
        [],
    )
    .unwrap();
    assert!(preview_result(&c).is_err());
}
fn preview_result(c: &Connection) -> Result<Quote, AppError> {
    quote_tx(
        c,
        "buyer",
        &QuoteInput {
            seller_id: "seller".into(),
            amount_minor: 100,
        },
        AT,
    )
}
#[test]
fn immutable_quote_freezes_terms_and_stale_seller_preview_cannot_commit() {
    let c = fixture();
    offer(&c);
    let q = preview(&c, "buyer", 1000);
    assert!(
        c.execute(
            "UPDATE market_c2c_quotes SET pay_minor=1 WHERE id=?1",
            params![q.id]
        )
        .is_err()
    );
    let mut s = terms();
    s.expected_version = 1;
    s.description = "New contact".into();
    save_seller_tx(&c, "seller", &s, AT).unwrap();
    assert_eq!(
        create_tx(&c, "buyer", &input(&q), "stale-key", "127.0.0.1", AT)
            .unwrap_err()
            .code(),
        Some("C2C_STALE")
    );
    let o = order(&c, 1000);
    assert_eq!(o.description.as_deref(), Some("New contact"));
    assert!(o.legacy_payment.is_none());
    s.expected_version = 2;
    s.description = "Newest".into();
    save_seller_tx(&c, "seller", &s, AT).unwrap();
    assert_eq!(order_tx(&c, &o.id).unwrap().description, o.description);
}
#[test]
fn cny_quotes_need_fresh_rate_and_integer_rounding_is_exact() {
    let c = fixture();
    let mut s = terms();
    s.currency = "CNY".into();
    save_seller_tx(&c, "seller", &s, AT).unwrap();
    assert_eq!(
        preview_result(&c).unwrap_err().code(),
        Some("C2C_FX_UNAVAILABLE")
    );
    let unix = at(AT).unwrap().timestamp();
    c.execute("INSERT INTO market_fx_rate_cache(id,rate_micros,source_timestamp,fetched_at) VALUES(1,7123456,?1,?1)",params![unix]).unwrap();
    let q = preview(&c, "buyer", 1000);
    assert_eq!(q.pay_minor, 7123);
    c.execute("UPDATE market_fx_rate_cache SET rate_micros=8000000", [])
        .unwrap();
    let o = create_tx(&c, "buyer", &input(&q), "frozen-cny-quote", "127.0.0.1", AT).unwrap();
    assert_eq!(o.fx_micros, 7123456);
    assert_eq!(o.pay_minor, 7123);
    // USD pricing also exercises non-integer minor rounding without floats.
    s.expected_version = 1;
    s.currency = "USD".into();
    s.price_bps = 10050;
    save_seller_tx(&c, "seller", &s, AT).unwrap();
    assert_eq!(preview(&c, "buyer", 100).pay_minor, 101);
}
#[test]
fn reservation_and_delivery_are_atomic_conserve_credit_and_replay_once() {
    let c = fixture();
    offer(&c);
    let q = preview(&c, "buyer", 1000);
    let i = input(&q);
    let o = transaction(&c, |c| {
        create_tx(c, "buyer", &i, "same-key-123", "127.0.0.1", AT)
    })
    .unwrap();
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        units_from_minor(1000).unwrap()
    );
    c.execute("UPDATE market_c2c_policy SET enabled=0", [])
        .unwrap();
    assert_eq!(
        create_tx(&c, "buyer", &i, "same-key-123", "127.0.0.1", LATER)
            .unwrap()
            .id,
        o.id
    );
    let o = paid(&c, &o);
    let result = transaction(&c, |c| release_tx(c, &o, "seller", AT)).unwrap();
    assert_eq!(release_tx(&c, &result, "seller", LATER).unwrap().id, o.id);
    let a = wallet_snapshot_tx(&c, "seller").unwrap();
    let b = wallet_snapshot_tx(&c, "buyer").unwrap();
    assert_eq!(
        a.posted_units + b.posted_units,
        units_from_minor(20000).unwrap()
    );
    assert_eq!(a.held_units, 0);
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
    assert_eq!(audit_tx_cases(&c, AT).unwrap(), 0);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM market_journal_transactions WHERE source_kind='c2c_order' AND action='release'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
}
#[test]
fn unpaid_cancellation_and_expiration_release_only_the_reservation() {
    let c = fixture();
    offer(&c);
    let o = order(&c, 1000);
    assert!(cancel_tx(&c, &o, "seller", false, AT).is_err());
    assert!(cancel_tx(&c, &o, "system", true, "2026-10-22T12:31:00+00:00").is_err());
    tick_tx(&c, LATER).unwrap();
    let o = order_tx(&c, &o.id).unwrap();
    assert_eq!(o.status, "expired");
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    assert_eq!(wallet_snapshot_tx(&c, "buyer").unwrap().posted_units, 0);
    let o = order(&c, 1000);
    let o = cancel_tx(&c, &o, "buyer", false, AT).unwrap();
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
}

#[tokio::test]
async fn housekeeping_expires_reservations_without_a_gateway_or_live_buyer() {
    let store = crate::store::AppStore::new_in_memory_for_tests().unwrap();
    let id = {
        let c = store.conn.lock().await;
        initialize_fixture(&c);
        let time = (Utc::now() - Duration::hours(1)).to_rfc3339();
        save_seller_tx(&c, "seller", &terms(), &time).unwrap();
        let q = quote_tx(
            &c,
            "buyer",
            &QuoteInput {
                seller_id: "seller".into(),
                amount_minor: 1000,
            },
            &time,
        )
        .unwrap();
        transaction(&c, |c| {
            create_tx(
                c,
                "buyer",
                &input(&q),
                "offline-housekeeping",
                "127.0.0.1",
                &time,
            )
        })
        .unwrap()
        .id
    };
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    // The worker has only a store, no gateway client, configuration or HTTP
    // state. A missing/blocked payment provider cannot hold this timer up.
    let worker = tokio::spawn(run_housekeeping(store.clone(), shutdown));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let expired = {
                let conn = store.conn.lock().await;
                order_tx(&conn, &id).unwrap().status == "expired"
            };
            if expired {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("C2C housekeeping must not wait for gateway reconciliation");
    stop.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    let c = store.conn.lock().await;
    assert!(evidence_valid_tx(&c, &id).unwrap());
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().posted_units,
        units_from_minor(20000).unwrap()
    );
    assert_eq!(wallet_snapshot_tx(&c, "buyer").unwrap().posted_units, 0);
}
#[test]
fn marked_payment_and_dispute_never_expire_and_owner_cannot_adjudicate_own_order() {
    let c = fixture();
    offer(&c);
    let o = paid(&c, &order(&c, 1000));
    tick_tx(&c, LATER).unwrap();
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "paid");
    assert!(cancel_tx(&c, &o, "buyer", false, LATER).is_err());
    let o = dispute_tx(&c, &o, "seller", "No money received", AT).unwrap();
    tick_tx(&c, LATER).unwrap();
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "disputed");
    assert!(
        resolve_tx(
            &c,
            &o,
            "seller",
            o.version,
            "return_to_seller",
            "evidence",
            true,
            LATER
        )
        .is_err()
    );
    assert!(
        resolve_tx(
            &c, &o, "owner", o.version, "release", "evidence", false, LATER
        )
        .is_err()
    );
    let o = transaction(&c, |c| {
        resolve_tx(
            c,
            &o,
            "owner",
            o.version,
            "return_to_seller",
            "Both parties confirmed no payment",
            true,
            LATER,
        )
    })
    .unwrap();
    assert_eq!(o.status, "cancelled");
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
}
#[test]
fn late_reports_never_reopen_reservations_or_mint_credit() {
    let c = fixture();
    offer(&c);
    let o = order(&c, 1000);
    let o = cancel_tx(&c, &o, "buyer", false, AT).unwrap();
    let o = late_payment_tx(&c, &o, "buyer", "Paid after cancellation", LATER).unwrap();
    assert!(release_tx(&c, &o, "seller", LATER).is_err());
    let o = resolve_tx(
        &c,
        &o,
        "owner",
        o.version,
        "record_external_resolution",
        "Seller confirms external resolution",
        true,
        LATER,
    )
    .unwrap();
    assert!(o.late_payment_resolved);
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    assert_eq!(wallet_snapshot_tx(&c, "buyer").unwrap().posted_units, 0);
}
#[test]
fn overlapping_previews_cannot_oversell_or_bypass_shared_daily_budget() {
    let c = fixture();
    offer(&c);
    c.execute(
        "UPDATE market_wallet_policy SET transfer_daily_max_minor=1000,transfer_max_minor=1000",
        [],
    )
    .unwrap();
    let q1 = preview(&c, "buyer", 1000);
    let q2 = preview(&c, "other", 1000);
    transaction(&c, |c| {
        create_tx(c, "buyer", &input(&q1), "race-one-123", "127.0.0.1", AT)
    })
    .unwrap();
    assert!(
        transaction(&c, |c| create_tx(
            c,
            "other",
            &input(&q2),
            "race-two-123",
            "127.0.0.1",
            AT
        ))
        .is_err()
    );
    assert_eq!(
        committed_transfer_units_tx(&c, "seller", AT).unwrap(),
        units_from_minor(1000).unwrap()
    );
    assert_eq!(stock_tx(&c, "seller", "2026-10-23T00:01:00Z").unwrap(), 0); // unresolved commitment survives midnight
    assert!(
        crate::market_wallet_transfers::verified_transfer_for_test(
            &c,
            "seller",
            "other",
            units_from_minor(1).unwrap(),
            AT
        )
        .is_err()
    );
}
#[test]
fn seller_release_codes_are_bound_persist_failed_attempts_and_are_idempotent() {
    let c = fixture();
    offer(&c);
    let o = paid(&c, &order(&c, 1000));
    let runtime = crate::market_payments::MarketPaymentsRuntime::for_test();
    let bind = http::binding_for_test(&o);
    let (key, mac) = runtime.transfer_mac("code-id", &bind, "012345").unwrap();
    c.execute("INSERT INTO market_c2c_challenges(id,order_id,seller_id,email,binding,code_mac,key_version,delivered_at,expires_at,created_ip,created_at) VALUES('code-id',?1,'seller','seller@example.test',?2,?3,?4,?5,'2026-10-22T12:10:00Z','127.0.0.1',?5)",params![o.id,bind,mac,key,AT]).unwrap();
    let mut a = http::ReleaseInput {
        expected_version: o.version,
        challenge_id: "code-id".into(),
        code: "000000".into(),
        acknowledge_received: true,
    };
    assert!(
        transaction(&c, |c| http::release_authorized_tx(
            c, &runtime, "buyer", &o.id, &a, AT
        ))
        .is_err()
    );
    for _ in 0..5 {
        assert!(
            transaction(&c, |c| http::release_authorized_tx(
                c, &runtime, "seller", &o.id, &a, AT
            ))
            .unwrap()
            .is_none()
        );
    }
    a.code = "012345".into();
    assert!(
        transaction(&c, |c| http::release_authorized_tx(
            c, &runtime, "seller", &o.id, &a, AT
        ))
        .unwrap()
        .is_none()
    );
    c.execute("UPDATE market_c2c_challenges SET attempts=0", [])
        .unwrap();
    let completed = transaction(&c, |c| {
        http::release_authorized_tx(c, &runtime, "seller", &o.id, &a, AT)
    })
    .unwrap()
    .unwrap();
    assert_eq!(completed.status, "released");
    assert!(
        http::release_authorized_tx(&c, &runtime, "seller", &o.id, &a, LATER)
            .unwrap()
            .is_some()
    );
}
#[test]
fn discovery_exposes_only_the_published_description_and_history_stays_owned() {
    let c = fixture();
    offer(&c);
    let public = serde_json::to_string(&offers_tx(&c, "buyer", "", AT).unwrap().0).unwrap();
    assert!(public.contains(&terms().description));
    assert!(!public.contains("paymentDetails"));
    let o = order(&c, 1000);
    assert!(owned_tx(&c, &o.id, "other").is_err());
    let list = http::test_list(&c, Some("buyer"), "", "").unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["description"], terms().description);
    assert!(list["items"][0]["legacyPayment"].is_null());
    assert!(
        http::test_list(&c, Some("other"), "", "").unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn historical_private_quote_stays_party_only_and_can_finish_after_seller_changes() {
    let c = fixture();
    offer(&c);
    let q = preview(&c, "buyer", 1000);
    // Historical immutable quote shape: the migration leaves its private
    // evidence intact and does not make it the seller's public description.
    c.execute("INSERT INTO market_c2c_quotes(id,buyer_id,seller_id,seller_version,policy_version,credit_minor,pay_minor,currency,fx_micros,price_bps,seller_name,contact,payment_method,payment_details,payment_minutes,expires_at,created_at)
        SELECT 'historical-private',buyer_id,seller_id,seller_version,policy_version,credit_minor,pay_minor,currency,fx_micros,price_bps,'Historical seller','PRIVATE CONTACT','Bank','PRIVATE ACCOUNT',payment_minutes,expires_at,created_at
        FROM market_c2c_quotes WHERE id=?1",params![q.id]).unwrap();
    let hold = crate::market_wallet::reserve_wallet_tx(
        &c,
        "seller",
        "c2c_order",
        "historical-order",
        "c2c",
        units_from_minor(1000).unwrap(),
        None,
        AT,
    )
    .unwrap();
    c.execute("INSERT INTO market_c2c_orders(id,quote_id,buyer_id,seller_id,idempotency_key,amount_units,hold_id,status,terms_version,payment_deadline,created_ip,created_at,updated_at) VALUES('historical-order','historical-private','buyer','seller','historical',?1,?2,'awaiting_payment',1,'2026-10-22T12:30:00+00:00','127.0.0.1',?3,?3)",params![units_from_minor(1000).unwrap(),hold,AT]).unwrap();
    let o = order_tx(&c, "historical-order").unwrap();
    pause_seller_tx(&c, "seller", 1, AT).unwrap();
    assert!(owned_tx(&c, &o.id, "other").is_err());
    for participant in ["buyer", "seller"] {
        let detail = owned_tx(&c, &o.id, participant).unwrap();
        assert!(detail.description.is_none());
        assert_eq!(
            detail.legacy_payment.unwrap().payment_details,
            "PRIVATE ACCOUNT"
        );
    }
    for viewer in [Some("buyer"), Some("seller"), None] {
        let list = http::test_list(&c, viewer, "", "").unwrap();
        assert!(list["items"][0]["description"].is_null());
        assert!(list["items"][0]["legacyPayment"].is_null());
        assert!(!list.to_string().contains("PRIVATE"));
    }
    let paid = paid(&c, &o);
    release_tx(&c, &paid, "seller", AT).unwrap();
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "released");
    crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
    crate::market_wallet::reconcile_wallet_tx(&c, "buyer").unwrap();
}
#[test]
fn notification_write_failure_rolls_back_freeze_and_order_together() {
    let c = fixture();
    offer(&c);
    let q = preview(&c, "buyer", 1000);
    c.execute_batch("CREATE TRIGGER test_c2c_notification_fail BEFORE INSERT ON market_notification_outbox BEGIN SELECT RAISE(ABORT,'forced failure'); END;").unwrap();
    assert!(
        transaction(&c, |c| create_tx(
            c,
            "buyer",
            &input(&q),
            "rollback-key",
            "127.0.0.1",
            AT
        ))
        .is_err()
    );
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM market_c2c_orders", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn malformed_reservations_open_scoped_critical_cases_and_cannot_be_dismissed() {
    let c = fixture();
    offer(&c);
    let o = order(&c, 1000);
    c.execute(
        "UPDATE market_wallet_holds SET remaining_units=remaining_units-1 WHERE id=?1",
        params![o.hold_id],
    )
    .unwrap();
    assert!(!evidence_valid_tx(&c, &o.id).unwrap());
    assert_eq!(audit_tx_cases(&c, AT).unwrap(), 1);
    assert!(safe_tx(&c, "seller", "other").is_err());
    let case: String = c
        .query_row(
            "SELECT id FROM market_finance_reconciliation_cases WHERE code='c2c_evidence_mismatch'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(crate::market_reconcile::resolve_case_tx(&c, &case, "owner", "ignored", AT).is_err());
}

#[test]
fn recording_payment_during_an_evidence_hold_still_protects_it_from_expiration() {
    let c = fixture();
    offer(&c);
    let o = order(&c, 1000);
    c.execute(
        "UPDATE market_wallet_holds SET remaining_units=remaining_units-1 WHERE id=?1",
        params![o.hold_id],
    )
    .unwrap();
    let o = paid(&c, &o);
    tick_tx(&c, LATER).unwrap();
    assert_eq!(order_tx(&c, &o.id).unwrap().status, "paid");
    assert!(release_tx(&c, &o, "seller", AT).is_err());
    assert!(c.query_row("SELECT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases WHERE code='c2c_evidence_mismatch' AND object_id=?1 AND status='open')",params![o.id],|r|r.get::<_,bool>(0)).unwrap());
}

#[test]
fn independent_connections_cannot_double_spend_between_c2c_and_ordinary_transfers() {
    let directory = std::env::temp_dir().join(format!("router-c2c-race-{}", Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("finance.sqlite");
    let c = fixture_on(Connection::open(&path).unwrap());
    offer(&c);
    let q = preview(&c, "buyer", 10000);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let tasks = (0..2)
        .map(|i| {
            let db = Connection::open(&path).unwrap();
            let gate = barrier.clone();
            let q = q.clone();
            std::thread::spawn(move || {
                gate.wait();
                let Ok(tx) = db.transaction_with_behavior(TransactionBehavior::Immediate) else {
                    return false;
                };
                let result = if i == 0 {
                    create_tx(&tx, "buyer", &input(&q), "racing-c2c-key", "127.0.0.1", AT)
                        .map(|_| ())
                } else {
                    crate::market_wallet_transfers::verified_transfer_for_test(
                        &tx,
                        "seller",
                        "other",
                        units_from_minor(15000).unwrap(),
                        AT,
                    )
                };
                if result.is_ok() {
                    tx.commit().unwrap();
                    true
                } else {
                    false
                }
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .filter(|succeeded| *succeeded)
            .count(),
        1
    );
    let total = ["seller", "buyer", "other"]
        .iter()
        .map(|u| wallet_snapshot_tx(&c, u).unwrap().posted_units)
        .sum::<i64>();
    assert_eq!(total, units_from_minor(20000).unwrap());
    crate::market_wallet::reconcile_wallet_tx(&c, "seller").unwrap();
    drop(c);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn history_pages_and_audit_cursor_reach_orders_beyond_the_first_two_hundred() {
    let c = fixture();
    offer(&c);
    for i in 0..205 {
        let time = (at(AT).unwrap() + Duration::hours(i)).to_rfc3339();
        transaction(&c, |c| {
            let q = quote_tx(
                c,
                "buyer",
                &QuoteInput {
                    seller_id: "seller".into(),
                    amount_minor: 100,
                },
                &time,
            )?;
            let o = create_tx(
                c,
                "buyer",
                &input(&q),
                &format!("page-key-{i}"),
                "127.0.0.1",
                &time,
            )?;
            cancel_tx(c, &o, "buyer", false, &time)
        })
        .unwrap();
    }
    let mut ids = std::collections::HashSet::new();
    let mut cursor = String::new();
    loop {
        let page = http::test_list(&c, Some("buyer"), "", &cursor).unwrap();
        for o in page["items"].as_array().unwrap() {
            assert!(ids.insert(o["id"].as_str().unwrap().to_owned()));
        }
        let Some(next) = page["next"].as_str() else {
            break;
        };
        cursor = next.into();
    }
    assert_eq!(ids.len(), 205);
    let (id, hold): (String, String) = c
        .query_row(
            "SELECT id,hold_id FROM market_c2c_orders ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    c.execute(
        "UPDATE market_wallet_holds SET amount_units=amount_units+1 WHERE id=?1",
        params![hold],
    )
    .unwrap();
    assert_eq!(audit_tx_cases(&c, AT).unwrap(), 0);
    assert_eq!(audit_tx_cases(&c, AT).unwrap(), 1);
    assert!(!evidence_valid_tx(&c, &id).unwrap());
    assert!(
        http::test_list(&c, Some("other"), "", "").unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn orphan_journals_freeze_every_money_scope_and_cannot_be_dismissed() {
    let c = fixture();
    let j = post_journal_tx(
        &c,
        "c2c_order",
        "missing-order",
        "reserve:c2c",
        "seller:86400:c2c",
        &[
            Posting {
                kind: "wallet_available",
                subject_id: "seller",
                signed_units: 86400,
            },
            Posting {
                kind: "wallet_held",
                subject_id: "seller",
                signed_units: -86400,
            },
        ],
        AT,
    )
    .unwrap();
    assert!(orphan_journal_tx(&c, &j.id).unwrap());
    assert_eq!(audit_tx_cases(&c, AT).unwrap(), 1);
    assert!(safe_tx(&c, "other", "buyer").is_err());
    let id: String = c
        .query_row(
            "SELECT id FROM market_finance_reconciliation_cases WHERE code='c2c_orphan_journal'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(crate::market_reconcile::resolve_case_tx(&c, &id, "owner", "dismiss", AT).is_err());
}

#[test]
fn old_and_new_policy_versions_do_not_reprice_existing_orders() {
    let c = fixture();
    offer(&c);
    let o = order(&c, 1000);
    let q = preview(&c, "other", 1000);
    let mut p = policy_tx(&c).unwrap();
    p.enabled = false;
    p.payment_minutes = 10;
    p.admission_minor = 20000;
    let p = save_policy_tx(&c, "owner", &p, AT).unwrap();
    assert!(p.version > 1);
    assert!(create_tx(&c, "other", &input(&q), "changed-policy", "127.0.0.1", AT).is_err());
    let o = paid(&c, &o);
    assert_eq!(
        o.payment_deadline.as_deref(),
        Some("2026-10-22T12:30:00+00:00")
    );
    let o = release_tx(&c, &o, "seller", AT).unwrap();
    assert_eq!(o.status, "released");
    assert!(evidence_valid_tx(&c, &o.id).unwrap());
}

#[test]
fn voluntary_seller_release_resolves_the_owner_dispute_signal() {
    let c = fixture();
    offer(&c);
    let o = paid(&c, &order(&c, 1000));
    let o = dispute_tx(&c, &o, "buyer", "Please verify the receipt", AT).unwrap();
    let resolved = release_tx(&c, &o, "seller", AT).unwrap();
    assert_eq!(resolved.status, "released");
    let (firing, closed):(i64,i64) = c.query_row("SELECT SUM(transition='firing'),SUM(transition='resolved') FROM operator_alert_signal_outbox WHERE kind='market_c2c_dispute' AND entity_id=?1",params![o.id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!((firing, closed), (1, 1));
}

#[test]
fn evidence_history_pages_are_complete_and_cannot_cross_order_boundaries() {
    let c = fixture();
    offer(&c);
    let o = order(&c, 1000);
    let other = order(&c, 1000);
    for i in 0..55 {
        event_tx(&c, &o, "buyer", "note", &format!("Evidence {i}"), AT).unwrap();
    }
    let first = http::test_events(&c, &o.id, "").unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 50);
    let cursor = first["next"].as_str().unwrap();
    let last = http::test_events(&c, &o.id, cursor).unwrap();
    assert_eq!(last["items"].as_array().unwrap().len(), 6);
    assert!(last["next"].is_null());
    assert!(
        http::test_events(&c, &other.id, cursor).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unfinished_order_and_creation_limits_cannot_be_evaded_with_new_previews() {
    let c = fixture();
    offer(&c);
    let a = order(&c, 1000);
    let b = order(&c, 1000);
    let q = preview(&c, "buyer", 100);
    assert_eq!(
        create_tx(
            &c,
            "buyer",
            &input(&q),
            "third-active-order",
            "127.0.0.1",
            AT
        )
        .unwrap_err()
        .code(),
        Some("C2C_ORDER_LIMIT")
    );
    cancel_tx(&c, &a, "buyer", false, AT).unwrap();
    cancel_tx(&c, &b, "buyer", false, AT).unwrap();
    for i in 2..10 {
        let q = preview(&c, "buyer", 100);
        let o = create_tx(
            &c,
            "buyer",
            &input(&q),
            &format!("bounded-create-{i}"),
            "127.0.0.1",
            AT,
        )
        .unwrap();
        cancel_tx(&c, &o, "buyer", false, AT).unwrap();
    }
    let q = preview(&c, "other", 100);
    assert!(matches!(
        create_tx(
            &c,
            "other",
            &input(&q),
            "same-ip-eleventh-order",
            "127.0.0.1",
            AT
        ),
        Err(AppError::TooManyRequests(_))
    ));
    let q = preview(&c, "buyer", 100);
    assert!(matches!(
        create_tx(&c, "buyer", &input(&q), "same-user-new-ip", "127.0.0.2", AT),
        Err(AppError::TooManyRequests(_))
    ));
    assert_eq!(wallet_snapshot_tx(&c, "seller").unwrap().held_units, 0);
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().posted_units,
        units_from_minor(20000).unwrap()
    );
}

#[test]
fn first_purchase_and_shared_ip_active_caps_are_enforced_before_reservation() {
    let c = fixture();
    offer(&c);
    let q = preview(&c, "buyer", 10001);
    assert_eq!(
        create_tx(
            &c,
            "buyer",
            &input(&q),
            "first-purchase-limit",
            "127.0.0.1",
            AT
        )
        .unwrap_err()
        .code(),
        Some("C2C_FIRST_ORDER_LIMIT")
    );
    for i in 0..7 {
        let user = format!("buyer-{i}");
        c.execute("INSERT INTO users(id,email_normalized,status,email_verified_at,created_at,last_login_at) VALUES(?1,?2,'active',?3,?3,?3)",params![user,format!("{user}@example.test"),AT]).unwrap();
        let q = preview(&c, &user, 100);
        let result = create_tx(
            &c,
            &user,
            &input(&q),
            &format!("ip-active-cap-{i}"),
            "127.0.0.2",
            AT,
        );
        if i < 6 {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code(), Some("C2C_ORDER_LIMIT"));
        }
    }
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        units_from_minor(600).unwrap()
    );
}

#[test]
fn undelivered_expired_changed_identity_and_stale_version_codes_never_release() {
    let c = fixture();
    offer(&c);
    let o = paid(&c, &order(&c, 1000));
    let runtime = crate::market_payments::MarketPaymentsRuntime::for_test();
    let bind = http::binding_for_test(&o);
    let (key, mac) = runtime
        .transfer_mac("delivery-code", &bind, "012345")
        .unwrap();
    c.execute("INSERT INTO market_c2c_challenges(id,order_id,seller_id,email,binding,code_mac,key_version,expires_at,created_ip,created_at) VALUES('delivery-code',?1,'seller','seller@example.test',?2,?3,?4,'2026-10-22T12:10:00Z','127.0.0.1',?5)",params![o.id,bind,mac,key,AT]).unwrap();
    let a = http::ReleaseInput {
        expected_version: o.version,
        challenge_id: "delivery-code".into(),
        code: "012345".into(),
        acknowledge_received: true,
    };
    assert!(
        http::release_authorized_tx(&c, &runtime, "seller", &o.id, &a, AT)
            .unwrap()
            .is_none()
    );
    c.execute(
        "UPDATE market_c2c_challenges SET delivered_at=?1",
        params![AT],
    )
    .unwrap();
    assert!(
        http::release_authorized_tx(&c, &runtime, "seller", &o.id, &a, "2026-10-22T12:10:00Z")
            .unwrap()
            .is_none()
    );
    c.execute(
        "UPDATE users SET email_normalized='new@example.test' WHERE id='seller'",
        [],
    )
    .unwrap();
    assert!(
        http::release_authorized_tx(&c, &runtime, "seller", &o.id, &a, AT)
            .unwrap()
            .is_none()
    );
    c.execute(
        "UPDATE users SET email_normalized='seller@example.test' WHERE id='seller'",
        [],
    )
    .unwrap();
    dispute_tx(&c, &o, "buyer", "New evidence changes the version", AT).unwrap();
    assert_eq!(
        http::release_authorized_tx(&c, &runtime, "seller", &o.id, &a, AT)
            .unwrap_err()
            .code(),
        Some("C2C_STALE")
    );
    assert_eq!(
        wallet_snapshot_tx(&c, "seller").unwrap().held_units,
        units_from_minor(1000).unwrap()
    );
    assert_eq!(wallet_snapshot_tx(&c, "buyer").unwrap().posted_units, 0);
}

#[test]
fn wallet_history_links_reservations_returns_and_both_delivery_sides_to_the_order() {
    let c = fixture();
    offer(&c);
    let cancelled = order(&c, 1000);
    cancel_tx(&c, &cancelled, "buyer", false, AT).unwrap();
    let released = paid(&c, &order(&c, 2000));
    release_tx(&c, &released, "seller", AT).unwrap();
    let seller = crate::market_payments::wallet_transactions_for_test(&c, "seller").unwrap();
    let linked = seller["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| !v["c2cOrderId"].is_null())
        .collect::<Vec<_>>();
    assert_eq!(linked.len(), 4);
    assert_eq!(
        linked
            .iter()
            .filter(|v| v["c2cOrderId"] == cancelled.id)
            .count(),
        2
    );
    assert_eq!(
        linked
            .iter()
            .filter(|v| v["c2cOrderId"] == released.id)
            .count(),
        2
    );
    let debit = linked
        .iter()
        .find(|v| v["sourceKind"] == "c2c_order" && v["action"] == "release")
        .unwrap();
    assert_eq!(
        debit["deltaUnits"],
        (-units_from_minor(2000).unwrap()).to_string()
    );
    let buyer = crate::market_payments::wallet_transactions_for_test(&c, "buyer").unwrap();
    assert_eq!(buyer["items"].as_array().unwrap().len(), 1);
    assert_eq!(buyer["items"][0]["c2cOrderId"], released.id);
    assert_eq!(
        buyer["items"][0]["deltaUnits"],
        units_from_minor(2000).unwrap().to_string()
    );
    assert!(
        crate::market_payments::wallet_transactions_for_test(&c, "other").unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
