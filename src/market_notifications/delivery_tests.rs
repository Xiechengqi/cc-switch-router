use super::delivery::{
    Attempt, Prepared, failure, finish_attempt_tx, prepare_tx, purpose, semantic_key,
};
use super::*;

const AT: &str = "2026-10-25T12:00:00+00:00";
fn item(kind: &str, payload: Value) -> Claimed {
    let mut payload = payload;
    payload["notificationTime"] = json!(AT);
    payload["notificationEventKey"] = json!(format!("{kind}:event"));
    Claimed {
        id: "notice".into(),
        recipient_user_id: "user".into(),
        event_kind: kind.into(),
        payload,
        mandatory: false,
        attempts: 1,
    }
}
fn db() -> Connection {
    let conn = crate::test_support::database().unwrap();
    enqueue_user_notification_tx(
        &conn,
        "buyer",
        "user",
        "wallet_transfer_sent",
        "transfer",
        "t",
        "event",
        json!({}),
        AT,
    )
    .unwrap();
    conn
}
fn task(conn: &Connection) -> Claimed {
    let mut item = item(
        "wallet_transfer_sent",
        json!({"amountMinor":100,"transferId":"t"}),
    );
    item.id = conn
        .query_row("SELECT id FROM market_notification_outbox", [], |r| {
            r.get(0)
        })
        .unwrap();
    item
}
fn envelope() -> Value {
    json!({"target":"buyer@example.test","subject":"Transfer sent","html":"<p>USD 1.00</p>","text":"USD 1.00","credentialFingerprint":"one-account"})
}
fn sending(value: Prepared) -> Attempt {
    match value {
        Prepared::Send(a) => a,
        _ => panic!("expected a channel attempt"),
    }
}

#[test]
fn client_termination_receipts_have_a_delivery_purpose_exact_amounts_and_role_specific_links() {
    for role in ["buyer", "provider"] {
        let i = item(
            "client_termination_settled",
            json!({
                "role":role,"amountUnits":"43200","walletReturnUnits":"388800",
                "compensationUnits":"43200","clientReference":"sample-client",
                "claim":"PRIVATE_CLAIM","note":"PRIVATE_NOTE",
            }),
        );
        assert_eq!(purpose(&i), Some("client_termination_receipt"));
        let (subject, html, text) = render_email(&i, "https://router.example");
        assert!(subject.is_ascii() && html.is_ascii() && text.is_ascii());
        assert!(text.contains("Total Provider compensation: USD 0.005"));
        assert!(text.contains("Total wallet return: USD 0.045"));
        assert!(text.contains("sample-client"));
        assert!(!html.contains("PRIVATE_CLAIM") && !text.contains("PRIVATE_NOTE"));
        assert!(html.contains(if role == "provider" {
            "/account/client/?tab=provider"
        } else {
            "/account/client/?tab=user"
        }));
        let key = semantic_key(&i, "client_termination_receipt");
        let mut retry = i;
        retry.attempts += 1;
        assert_eq!(key, semantic_key(&retry, "client_termination_receipt"));
    }
}

#[test]
fn client_termination_claim_notices_are_actionable_private_and_have_a_delivery_purpose() {
    for role in ["buyer", "provider"] {
        let i = item(
            "client_termination_claim",
            json!({"role":role,"action":"opened",
            "responseDeadline":"2026-10-28T12:00:00Z","clientReference":"test-client",
            "claim":"PRIVATE_CLAIM","note":"PRIVATE_NOTE"}),
        );
        assert_eq!(purpose(&i), Some("client_termination_claim"));
        let (subject, html, text) = render_email(&i, "https://router.example");
        assert!(subject.is_ascii() && html.is_ascii() && text.is_ascii());
        assert!(text.contains("72 hours") && text.contains("2026-10-28T12:00:00Z"));
        assert!(!html.contains("PRIVATE_CLAIM") && !text.contains("PRIVATE_NOTE"));
        assert!(html.contains(if role == "provider" {
            "/account/client/?tab=provider"
        } else {
            "/account/client/?tab=user"
        }));
        let mut response = i;
        response.payload["action"] = json!("responded");
        let (_, _, text) = render_email(&response, "https://router.example");
        assert!(text.contains("Owner decision"));
    }
}

#[test]
fn client_termination_emails_link_to_the_exact_case_without_private_text_or_untrusted_urls() {
    for role in ["buyer", "provider", "handler"] {
        for kind in ["client_termination_claim", "client_termination_settled"] {
            let i = item(
                kind,
                json!({"role":role,"terminationId":"old/id?other=case#fragment",
                "claim":"PRIVATE_CLAIM","note":"PRIVATE_NOTE","url":"https://untrusted.example/"}),
            );
            let (_, html, text) =
                render_email(&i, "https://router.example/settings/?stale=1#income");
            let link = "https://router.example/client-market/?terminationId=old%2Fid%3Fother%3Dcase%23fragment";
            assert!(html.contains(link) && text.contains(link));
            assert!(!html.contains("PRIVATE_CLAIM") && !text.contains("PRIVATE_NOTE"));
            assert!(!html.contains("untrusted.example") && !text.contains("stale=1"));
        }
    }
}

#[test]
fn every_financial_email_is_english_and_uses_the_shared_layout() {
    for kind in KINDS {
        let item = item(
            kind.kind,
            json!({"amountMinor":1234,"feeMinor":66,"payMinor":1300,"orderId":"order",
            "status":"内部原始状态","note":"中文私密备注","description":"SECRET_CONTACT","code":"SECRET_CODE",
            "reason":"中文详情","recipientEmail":"other@example.test","senderEmail":"sender@example.test",
            "notificationTime":"2026-10-25T12:00:00Z","rateBps":1025,"effectiveAt":"2026-11-01T00:00:00Z"}),
        );
        let (subject, html, text) = render_email(&item, "https://router.example");
        for output in [&subject, &html, &text] {
            assert!(output.is_ascii(), "{}", kind.kind);
            for secret in [
                "内部原始状态",
                "中文私密备注",
                "SECRET_CONTACT",
                "SECRET_CODE",
            ] {
                assert!(!output.contains(secret));
            }
        }
        assert!(subject.starts_with("TokenSwitch: "));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("x-apple-disable-message-reformatting"));
        assert!(html.matches("max-width:560px").count() >= 3);
        assert!(text.contains("USD 12.34"));
        assert!(text.contains("Maintenance fee: USD 0.66"));
        assert!(text.contains(if kind.kind.starts_with("c2c_") {
            "External payment total: USD 13.00"
        } else {
            "Payment total: USD 13.00"
        }));
        assert!(!html.contains("If this email was unexpected, no action is required"));
    }
}
#[test]
fn mail_has_escaped_structured_facts_safe_links_and_honest_receipt_instructions() {
    let i = item(
        "wallet_transfer_sent",
        json!({"amountUnits":"8640001","recipientEmail":"<img onerror='oops'>","transferId":"transfer-1"}),
    );
    let (_, html, text) = render_email(&i, "https://router.example");
    assert!(html.contains("&lt;img onerror=&#39;oops&#39;&gt;"));
    assert!(!html.contains("<img"));
    assert!(text.contains("USD 1.00"));
    assert!(!text.contains("ledger units"));
    assert!(text.contains("If you did not authorize this transfer"));
    let mut i = item(
        "c2c_order_updated",
        json!({"orderId":"o&x=1","status":"paid","action":"paid","role":"seller","amountUnits":"86400000","payMinor":7000,"currency":"CNY"}),
    );
    let (subject, html, text) = render_email(&i, "https://router.example");
    assert!(subject.contains("buyer reported payment"));
    assert!(text.contains("not proof of receipt"));
    assert!(text.contains("External payment total: CNY 70.00"));
    assert!(html.contains("c2cOrder=o%26x%3D1"));
    assert!(!text.contains("release code"));
    i.payload["action"] = json!("overdue_escalated");
    assert!(
        render_email(&i, "https://router.example")
            .0
            .contains("escalated")
    );
    i.payload["role"] = json!("handler");
    i.payload["status"] = json!("disputed");
    let (_, html, _) = render_email(&i, "https://router.example");
    assert!(html.contains("/settings/?panel=market_finance&amp;c2cOrder="));
    for bad in ["javascript:alert(1)", "https://user:pass@router.example/"] {
        assert!(!render_email(&i, bad).1.contains("href="));
    }
}
#[test]
fn integer_amounts_preserve_useful_precision_without_raw_ledger_units() {
    for (units, expected) in [
        ("0", "USD 0.00"),
        ("1", "<USD 0.000001"),
        ("8", "<USD 0.000001"),
        ("9", "USD 0.000001"),
        ("43200000", "USD 5.00"),
        ("8726400", "USD 1.01"),
        ("-8640000", "-USD 1.00"),
    ] {
        assert_eq!(
            email::amount(&json!({"amountUnits":units})).unwrap(),
            expected
        );
    }
    assert_eq!(
        email::amount(&json!({"amountMinor":i64::MIN})).unwrap(),
        "-USD 92233720368547758.08"
    );
}
#[test]
fn recipient_and_purpose_policy_suppresses_noise_without_erasing_history() {
    for kind in ["topup_closed_unpaid", "earnings_converted", "refund_status"] {
        assert!(purpose(&item(kind, json!({}))).is_none());
    }
    for action in ["created", "note", "cancelled", "expired", "accepted"] {
        for role in ["buyer", "seller"] {
            assert!(
                purpose(&item(
                    "c2c_order_updated",
                    json!({"action":action,"role":role})
                ))
                .is_none()
            );
        }
    }
    assert!(purpose(&item("c2c_seller_changed", json!({"reason":"saved"}))).is_none());
    assert!(purpose(&item("c2c_seller_changed", json!({"reason":"suspended"}))).is_some());
    assert!(
        purpose(&item(
            "c2c_order_updated",
            json!({"action":"paid","role":"buyer"})
        ))
        .is_none()
    );
    assert_eq!(
        purpose(&item(
            "c2c_order_updated",
            json!({"action":"paid","role":"seller"})
        )),
        Some("c2c_payment_reported")
    );
    assert!(
        purpose(&item(
            "c2c_order_updated",
            json!({"action":"disputed","role":"buyer","actorId":"user"})
        ))
        .is_none()
    );
    for role in ["buyer", "seller"] {
        assert_eq!(
            purpose(&item(
                "c2c_order_updated",
                json!({"action":"released","role":role})
            )),
            Some("c2c_receipt")
        );
    }
    let conn = db();
    let first = task(&conn);
    assert_eq!(feed_tx(&conn, "user", None).unwrap().unread, 1);
    let a = sending(prepare_tx(&conn, &first, "email", "receipt", envelope(), true, AT).unwrap());
    finish_attempt_tx(&conn, &a, &Ok("provider-message".into()), AT).unwrap();
    assert_eq!(feed_tx(&conn, "user", None).unwrap().unread, 1);
}
#[test]
fn mandatory_receipts_ignore_historical_preferences_and_optional_alerts_do_not() {
    let conn = db();
    for kind in [
        "topup_credited",
        "topup_review_required",
        "wallet_low_runway",
    ] {
        conn.execute(
            "INSERT INTO market_notification_preferences VALUES('user',?1,0,?2)",
            params![kind, AT],
        )
        .unwrap();
    }
    for kind in ["topup_credited", "topup_review_required"] {
        assert!(
            !recipient_tx(&conn, &item(kind, json!({})))
                .unwrap()
                .opted_out
        );
    }
    assert!(
        recipient_tx(&conn, &item("wallet_low_runway", json!({})))
            .unwrap()
            .opted_out
    );
}
#[test]
fn semantic_dedup_is_per_recipient_channel_and_action_not_mutable_order_version() {
    let conn = db();
    let mut one = task(&conn);
    one.event_kind = "c2c_order_updated".into();
    one.payload = json!({"orderId":"o","action":"paid","role":"seller"});
    let mut other = task(&conn);
    other.payload =
        json!({"orderId":"o","action":"paid","role":"seller","note":"later","version":12});
    assert_eq!(
        semantic_key(&one, "c2c_payment_reported"),
        semantic_key(&other, "c2c_payment_reported")
    );
    let a = sending(
        prepare_tx(
            &conn,
            &one,
            "email",
            "c2c_payment_reported",
            envelope(),
            true,
            AT,
        )
        .unwrap(),
    );
    finish_attempt_tx(&conn, &a, &Ok("message-1".into()), AT).unwrap();
    assert!(
        matches!(prepare_tx(&conn,&other,"email","c2c_payment_reported",envelope(),true,AT).unwrap(),Prepared::Done(s) if s=="sent")
    );
    assert!(matches!(
        prepare_tx(
            &conn,
            &other,
            "telegram",
            "c2c_payment_reported",
            envelope(),
            true,
            AT
        )
        .unwrap(),
        Prepared::Send(_)
    ));
}
#[test]
fn channel_retry_keeps_frozen_body_key_and_successful_sibling_result() {
    let conn = db();
    let i = task(&conn);
    let mail = sending(prepare_tx(&conn, &i, "email", "receipt", envelope(), true, AT).unwrap());
    let tg = sending(prepare_tx(&conn, &i, "telegram", "receipt", envelope(), true, AT).unwrap());
    finish_attempt_tx(&conn, &mail, &Ok("resend-123".into()), AT).unwrap();
    let at = DateTime::parse_from_rfc3339("2026-10-25T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    finish_attempt_tx(
        &conn,
        &tg,
        &Err(failure("rate limited", true, Some(at))),
        AT,
    )
    .unwrap();
    assert!(
        matches!(prepare_tx(&conn,&i,"email","receipt",envelope(),true,AT).unwrap(),Prepared::Done(s) if s=="sent")
    );
    assert!(
        matches!(prepare_tx(&conn,&i,"telegram","receipt",envelope(),true,AT).unwrap(),Prepared::Deferred(v) if v==at)
    );
    let mut rerender = envelope();
    rerender["text"] = json!("changed template");
    let retry = sending(
        prepare_tx(
            &conn,
            &i,
            "telegram",
            "receipt",
            rerender,
            true,
            "2026-10-25T12:10:01+00:00",
        )
        .unwrap(),
    );
    assert_eq!(retry.envelope["text"], json!("USD 1.00"));
    assert_eq!(tg.key, retry.key);
    finish_attempt_tx(
        &conn,
        &retry,
        &Ok("telegram-45".into()),
        "2026-10-25T12:10:02+00:00",
    )
    .unwrap();
    let ids: Vec<String> = conn
        .prepare("SELECT provider_message_id FROM market_notification_deliveries ORDER BY channel")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(ids, vec!["resend-123", "telegram-45"]);
    assert!(
        conn.execute(
            "UPDATE market_notification_deliveries SET envelope_json='{}'",
            []
        )
        .is_err()
    );
}
#[test]
fn expired_claims_cannot_overwrite_new_attempts_and_ambiguity_has_a_time_bound() {
    let conn = db();
    let i = task(&conn);
    let old = sending(prepare_tx(&conn, &i, "email", "receipt", envelope(), true, AT).unwrap());
    let new = sending(
        prepare_tx(
            &conn,
            &i,
            "email",
            "receipt",
            envelope(),
            true,
            "2026-10-25T12:02:01+00:00",
        )
        .unwrap(),
    );
    assert_eq!(old.key, new.key);
    assert!(!finish_attempt_tx(&conn, &old, &Ok("old".into()), AT).unwrap());
    assert!(finish_attempt_tx(&conn, &new, &Ok("new".into()), AT).unwrap());
    let mut another = task(&conn);
    another.payload["notificationEventKey"] = json!("another-receipt");
    sending(prepare_tx(&conn, &another, "email", "receipt", envelope(), true, AT).unwrap());
    assert!(
        matches!(prepare_tx(&conn,&another,"email","receipt",envelope(),true,"2026-10-26T11:00:01+00:00").unwrap(),Prepared::Done(s) if s=="uncertain")
    );
}
#[test]
fn changed_recipient_or_account_and_unconfigured_channels_never_blindly_resend() {
    let conn = db();
    let i = task(&conn);
    let first = sending(prepare_tx(&conn, &i, "email", "receipt", envelope(), true, AT).unwrap());
    finish_attempt_tx(&conn, &first, &Err(failure("temporary", true, None)), AT).unwrap();
    let mut changed = envelope();
    changed["target"] = json!("different@example.test");
    assert!(
        matches!(prepare_tx(&conn,&i,"email","receipt",changed,true,"2026-10-25T13:00:00+00:00").unwrap(),Prepared::Done(s) if s=="suppressed")
    );
    let mut j = task(&conn);
    j.payload["notificationEventKey"] = json!("new");
    let first = sending(prepare_tx(&conn, &j, "email", "receipt", envelope(), true, AT).unwrap());
    finish_attempt_tx(&conn, &first, &Err(failure("temporary", true, None)), AT).unwrap();
    let mut changed = envelope();
    changed["credentialFingerprint"] = json!("different-merchant");
    assert!(
        matches!(prepare_tx(&conn,&j,"email","receipt",changed,true,"2026-10-25T13:00:00+00:00").unwrap(),Prepared::Done(s) if s=="uncertain")
    );
    j.payload["notificationEventKey"] = json!("no-config");
    assert!(
        matches!(prepare_tx(&conn,&j,"email","receipt",envelope(),false,AT).unwrap(),Prepared::Done(s) if s=="blocked_config")
    );
}
#[test]
fn delivery_is_bounded_and_business_completion_uses_claim_generation() {
    let conn = db();
    let i = task(&conn);
    let mut at = DateTime::parse_from_rfc3339(AT).unwrap();
    for n in 1..=MAX_ATTEMPTS {
        let a = sending(
            prepare_tx(
                &conn,
                &i,
                "email",
                "receipt",
                envelope(),
                true,
                &at.to_rfc3339(),
            )
            .unwrap(),
        );
        assert_eq!(a.attempts, n);
        finish_attempt_tx(
            &conn,
            &a,
            &Err(failure("temporary", true, None)),
            &at.to_rfc3339(),
        )
        .unwrap();
        at += chrono::Duration::hours(3);
    }
    assert!(
        matches!(prepare_tx(&conn,&i,"email","receipt",envelope(),true,&at.to_rfc3339()).unwrap(),Prepared::Done(s) if s=="failed")
    );
    let first = claim_tx(&conn, "one", AT).unwrap().remove(0);
    let second = claim_tx(&conn, "two", "2026-10-25T12:02:01+00:00")
        .unwrap()
        .remove(0);
    finish_tx(&conn, &first, &Ok("email:sent".into()), AT).unwrap();
    assert_eq!(
        conn.query_row("SELECT status FROM market_notification_outbox", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "claimed"
    );
    finish_tx(&conn, &second, &Ok("email:sent".into()), AT).unwrap();
    assert_eq!(
        conn.query_row("SELECT status FROM market_notification_outbox", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "sent"
    );
}
#[test]
fn aggregate_delivery_status_only_records_real_channel_success() {
    for (channels, expected) in [
        ("suppressed", "suppressed"),
        ("superseded", "suppressed"),
        ("email:blocked_config", "suppressed"),
        ("email:failed", "failed"),
        ("email:blocked_config,telegram:uncertain", "failed"),
        ("email:sent,telegram:failed", "sent"),
        ("email:failed,telegram:sent", "sent"),
    ] {
        let conn = db();
        let item = claim_tx(&conn, "worker", AT).unwrap().remove(0);
        finish_tx(&conn, &item, &Ok(channels.into()), AT).unwrap();
        let (status, saved, sent_at): (String, String, Option<String>) = conn
            .query_row(
                "SELECT status,delivered_channels,sent_at FROM market_notification_outbox",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, expected, "{channels}");
        assert_eq!(saved, channels);
        assert_eq!(sent_at.is_some(), expected == "sent");
    }

    // Superseding an unneeded retry never erases an already-sent receipt.
    let conn = db();
    let item = claim_tx(&conn, "worker", AT).unwrap().remove(0);
    let attempt =
        sending(prepare_tx(&conn, &item, "email", "receipt", envelope(), true, AT).unwrap());
    finish_attempt_tx(&conn, &attempt, &Ok("message-1".into()), AT).unwrap();
    finish_tx(&conn, &item, &Ok("superseded".into()), AT).unwrap();
    let (status, channels): (String, String) = conn
        .query_row(
            "SELECT status,delivered_channels FROM market_notification_outbox",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "sent");
    assert_eq!(channels, "superseded,email:sent");
}

#[test]
fn provider_fee_receipt_has_one_rate_and_no_duplicate_language_or_code_instructions() {
    let i = item(
        "commission_policy_changed",
        json!({"rateBps":1025,"effectiveAt":"2026-11-01T00:00:00Z"}),
    );
    let (_, html, text) = render_email(&i, "https://router.example");
    assert_eq!(text.matches("10.25%").count(), 1);
    assert!(html.contains("/account/provider-settings/?tab=income"));
    assert!(text.contains("2026-11-01 00:00:00 UTC"));
    assert!(!text.contains("account/wallet"));
    assert_eq!(
        delivery::english_sender("中文 <mail@example.test>").as_deref(),
        Some("TokenSwitch <mail@example.test>")
    );
}

#[test]
fn service_warning_includes_an_unambiguous_utc_deadline() {
    let item = item(
        "contract_terminating_soon",
        json!({"contractId":"service", "terminatesAt":"2026-10-26T08:00:00+08:00"}),
    );
    let (_, html, text) = render_email(&item, "https://router.example");
    assert!(html.contains("Service deadline"));
    assert!(text.contains("Service deadline: 2026-10-26 00:00:00 UTC"));
}

#[test]
fn resolved_payment_cases_and_changed_service_states_do_not_send_stale_instructions() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE market_topup_orders(id TEXT,user_id TEXT,status TEXT);
        CREATE TABLE market_completed_topup_evidence(id TEXT);
        CREATE TABLE market_finance_reconciliation_cases(id TEXT,code TEXT,object_kind TEXT,object_id TEXT,status TEXT,detail_code TEXT);
        CREATE TABLE market_platform_contracts(id TEXT,buyer_user_id TEXT,status TEXT,suspended_at TEXT);
        INSERT INTO market_topup_orders VALUES('o','user','review_required');
        INSERT INTO market_finance_reconciliation_cases VALUES('case','topup_verification_failed','topup','o','open','paid_amount_short');
        INSERT INTO market_platform_contracts VALUES('service','user','suspended','2026-10-24T00:00:00Z');").unwrap();
    let mut i = item(
        "topup_review_required",
        json!({"orderId":"o","reviewCaseId":"case"}),
    );
    assert!(notice_current_tx(&conn, &i, AT).unwrap());
    conn.execute(
        "UPDATE market_finance_reconciliation_cases SET status='resolved'",
        [],
    )
    .unwrap();
    assert!(!notice_current_tx(&conn, &i, AT).unwrap());
    conn.execute("UPDATE market_topup_orders SET status='credited'", [])
        .unwrap();
    conn.execute(
        "INSERT INTO market_completed_topup_evidence VALUES('o')",
        [],
    )
    .unwrap();
    i.event_kind = "topup_credited".into();
    assert!(notice_current_tx(&conn, &i, AT).unwrap());
    i.recipient_user_id = "another-user".into();
    assert!(!notice_current_tx(&conn, &i, AT).unwrap());
    i = item("contract_suspended", json!({"contractId":"service"}));
    i.payload["notificationEventKey"] = json!("contract_suspended:service:2026-10-24T00:00:00Z");
    assert!(notice_current_tx(&conn, &i, AT).unwrap());
    // Still suspended is not enough: this notice must belong to the current
    // uninterrupted suspension, not an old episode or a retry timestamp.
    i.payload["notificationEventKey"] = json!("contract_suspended:service:2026-10-24T00:00:05Z");
    assert!(!notice_current_tx(&conn, &i, AT).unwrap());
    i.payload["notificationEventKey"] = json!("contract_suspended:service:2026-10-24T00:00:00Z");
    i.payload["suspensionStartedAt"] = json!("2026-10-23T00:00:00Z");
    assert!(!notice_current_tx(&conn, &i, AT).unwrap());
    i.payload["suspensionStartedAt"] = Value::Null;
    i.event_kind = "contract_terminating_soon".into();
    i.payload["notificationEventKey"] =
        json!("contract_terminating_soon:service:2026-10-24T00:00:00Z");
    assert!(notice_current_tx(&conn, &i, AT).unwrap());
    conn.execute(
        "UPDATE market_platform_contracts SET suspended_at='2026-10-25T00:00:00Z'",
        [],
    )
    .unwrap();
    assert!(!notice_current_tx(&conn, &i, AT).unwrap());
    conn.execute("UPDATE market_platform_contracts SET status='active'", [])
        .unwrap();
    assert!(!notice_current_tx(&conn, &i, AT).unwrap());
    i.event_kind = "contract_resumed".into();
    assert!(notice_current_tx(&conn, &i, AT).unwrap());
    conn.execute("UPDATE market_platform_contracts SET status='ended'", [])
        .unwrap();
    assert!(!notice_current_tx(&conn, &i, AT).unwrap());
    i.event_kind = "contract_terminated".into();
    assert!(notice_current_tx(&conn, &i, AT).unwrap());
}

#[test]
fn pause_delivery_identity_is_stable_and_reuses_the_historical_provider_key() {
    let conn = db();
    let mut first = task(&conn);
    first.event_kind = "contract_suspended".into();
    first.payload = json!({
        "contractId":"service", "suspensionStartedAt":AT,
        "notificationTime":AT,
        "notificationEventKey":format!("contract_suspended:service:{AT}")
    });
    let mut legacy = task(&conn);
    legacy.event_kind = first.event_kind.clone();
    legacy.payload = first.payload.clone();
    legacy
        .payload
        .as_object_mut()
        .unwrap()
        .remove("suspensionStartedAt");
    assert_eq!(
        semantic_key(&first, "service_paused"),
        semantic_key(&legacy, "service_paused")
    );
    let a = sending(
        prepare_tx(
            &conn,
            &legacy,
            "email",
            "service_paused",
            envelope(),
            true,
            AT,
        )
        .unwrap(),
    );
    finish_attempt_tx(&conn, &a, &Ok("pause-receipt".into()), AT).unwrap();
    first.payload["notificationTime"] = json!("2026-10-25T12:05:00+00:00");
    first.payload["notificationEventKey"] = json!("changed-by-a-retry");
    assert!(matches!(
        prepare_tx(&conn, &first, "email", "service_paused", envelope(), true, AT).unwrap(),
        Prepared::Done(status) if status == "sent"
    ));
    assert_eq!(
        conn.query_row(
            "SELECT idempotency_key FROM market_notification_deliveries",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        a.key
    );
    first.payload["suspensionStartedAt"] = json!("2026-10-26T12:00:00+00:00");
    assert_ne!(
        semantic_key(&first, "service_paused"),
        semantic_key(&legacy, "service_paused")
    );
}

#[test]
fn obsolete_channel_cleanup_preserves_sent_receipts_and_live_or_uncertain_attempts() {
    let conn = db();
    let notice = task(&conn);
    let mut live = None;
    for (purpose, finish) in [
        ("sent", true),
        ("queued", false),
        ("unattempted", false),
        ("live", false),
        ("expired", false),
    ] {
        if purpose == "unattempted" {
            assert!(
                matches!(prepare_tx(&conn,&notice,"email",purpose,envelope(),false,AT).unwrap(),Prepared::Done(s) if s=="blocked_config")
            );
            continue;
        }
        let a =
            sending(prepare_tx(&conn, &notice, "email", purpose, envelope(), true, AT).unwrap());
        if finish {
            finish_attempt_tx(&conn, &a, &Ok("known-receipt".into()), AT).unwrap();
        } else if purpose == "queued" {
            finish_attempt_tx(&conn, &a, &Err(failure("response lost", true, None)), AT).unwrap();
        } else if purpose == "expired" {
            conn.execute(
                "UPDATE market_notification_deliveries SET claim_expires_at=?2 WHERE id=?1",
                params![a.id, AT],
            )
            .unwrap();
        } else if purpose == "live" {
            live = Some(a);
        }
    }
    assert_eq!(
        policy::suppress_obsolete_deliveries_tx(&conn, &notice, AT).unwrap(),
        Some(
            DateTime::parse_from_rfc3339(AT)
                .unwrap()
                .with_timezone(&Utc)
                + chrono::Duration::seconds(CLAIM_SECS)
        ),
    );
    for (purpose, expected) in [
        ("sent", "sent"),
        ("queued", "uncertain"),
        ("unattempted", "suppressed"),
        ("live", "claimed"),
        ("expired", "uncertain"),
    ] {
        assert_eq!(
            conn.query_row(
                "SELECT status FROM market_notification_deliveries WHERE purpose=?1",
                params![purpose],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            expected
        );
    }
    finish_attempt_tx(&conn, &live.unwrap(), &Ok("late-receipt".into()), AT).unwrap();
    assert!(
        policy::suppress_obsolete_deliveries_tx(&conn, &notice, AT)
            .unwrap()
            .is_none()
    );
    conn.execute(
        "UPDATE market_notification_outbox SET status='claimed',attempts=1",
        [],
    )
    .unwrap();
    finish_tx(&conn, &notice, &Ok("superseded".into()), AT).unwrap();
    assert_eq!(
        conn.query_row("SELECT status FROM market_notification_outbox", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "sent"
    );
}

#[test]
fn grouped_funding_reminders_recheck_current_wallet_and_current_contracts() {
    use crate::market_platform_contracts::{
        BillingModel, PrepareContract, activate_contract_tx, prepare_contract_tx,
    };
    let conn = db();
    let start = "2026-09-27T12:00:00+00:00";
    crate::market_wallet::credit_verified_topup_tx(
        &conn, "user", "initial", 2000, "receipt", start,
    )
    .unwrap();
    for (product, quote) in [("one", "q1"), ("two", "q2")] {
        let id = prepare_contract_tx(
            &conn,
            &PrepareContract {
                product_kind: "share",
                product_ref: product,
                trial_scope_ref: product,
                quote_id: quote,
                buyer_user_id: "user",
                provider_user_id: "provider",
                offer_revision: 1,
                billing_model: BillingModel::CalendarMonth,
                price_minor: 1000,
                auto_renew: true,
                trial_seconds: 0,
                trial_allowance_seconds: 0,
            },
            start,
        )
        .unwrap();
        activate_contract_tx(&conn, &id, start, start).unwrap();
    }
    let at = DateTime::parse_from_rfc3339(AT)
        .unwrap()
        .with_timezone(&Utc);
    crate::market_platform_contracts::queue_funding_reminders_tx(&conn, at).unwrap();
    let rows = {
        let mut s=conn.prepare("SELECT id,payload_json FROM market_notification_outbox WHERE event_kind='renewal_upcoming_shortfall' ORDER BY id").unwrap();
        s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(rows.len(), 2);
    let mut i = item(
        "renewal_upcoming_shortfall",
        serde_json::from_str(&rows[0].1).unwrap(),
    );
    i.id = rows[0].0.clone();
    assert!(notice_current_tx(&conn, &i, AT).unwrap());
    assert_eq!(
        policy::related_count_tx(&conn, &i, "funding_summary", AT).unwrap(),
        2
    );
    let other = item(
        "renewal_upcoming_shortfall",
        serde_json::from_str(&rows[1].1).unwrap(),
    );
    assert_eq!(
        semantic_key(&i, "funding_summary"),
        semantic_key(&other, "funding_summary")
    );
    crate::market_wallet::credit_verified_topup_tx(
        &conn,
        "user",
        "new-credit",
        2000,
        "new-receipt",
        AT,
    )
    .unwrap();
    assert!(!notice_current_tx(&conn, &i, AT).unwrap());
    assert_eq!(
        policy::related_count_tx(&conn, &i, "funding_summary", AT).unwrap(),
        0
    );
}

#[tokio::test]
async fn mocked_email_transport_preserves_body_and_key_after_rate_limit_and_saves_message_id() {
    use std::sync::{Arc, Mutex};
    let received = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let app = Router::new().route(
        "/emails",
        post({
            let received = received.clone();
            move |headers: HeaderMap, Json(body): Json<Value>| {
                let received = received.clone();
                async move {
                    let mut calls = received.lock().unwrap();
                    calls.push((
                        headers
                            .get("Idempotency-Key")
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .to_string(),
                        body,
                    ));
                    if calls.len() == 1 {
                        (
                            axum::http::StatusCode::TOO_MANY_REQUESTS,
                            [("Retry-After", "60")],
                            Json(json!({"name":"rate_limit_exceeded"})),
                        )
                    } else {
                        (
                            axum::http::StatusCode::OK,
                            [("Retry-After", "0")],
                            Json(json!({"id":"mail-provider-id"})),
                        )
                    }
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/emails", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let conn = db();
    let i = task(&conn);
    let http = reqwest::Client::new();
    let now = Utc::now().to_rfc3339();
    let first = sending(prepare_tx(&conn, &i, "email", "receipt", envelope(), true, &now).unwrap());
    fn send(a: &Attempt) -> crate::notifications::FrozenEmailEnvelope<'_> {
        crate::notifications::FrozenEmailEnvelope {
            from: "TokenSwitch <router@example.test>",
            recipient: "buyer@example.test",
            subject: "Transfer sent",
            html: "<p>USD 1.00</p>",
            text: "USD 1.00",
            reply_to: None,
            idempotency_key: &a.key,
        }
    }
    let failed = crate::notifications::send_resend_frozen_email_to(
        &http,
        "mock-api-key",
        send(&first),
        &endpoint,
    )
    .await;
    let Err(ref error) = failed else {
        panic!("expected rate limit")
    };
    assert!(error.retryable);
    assert!(error.retry_at.is_some());
    finish_attempt_tx(&conn, &first, &failed, &now).unwrap();
    let next = (error.retry_at.unwrap() + chrono::Duration::seconds(1)).to_rfc3339();
    let retry =
        sending(prepare_tx(&conn, &i, "email", "receipt", envelope(), true, &next).unwrap());
    let sent = crate::notifications::send_resend_frozen_email_to(
        &http,
        "mock-api-key",
        send(&retry),
        &endpoint,
    )
    .await;
    assert_eq!(sent.as_ref().unwrap(), "mail-provider-id");
    finish_attempt_tx(&conn, &retry, &sent, &next).unwrap();
    assert!(
        matches!(prepare_tx(&conn,&i,"email","receipt",envelope(),true,&next).unwrap(),Prepared::Done(s) if s=="sent")
    );
    let calls = received.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    assert_eq!(
        conn.query_row(
            "SELECT provider_message_id FROM market_notification_deliveries",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "mail-provider-id"
    );
    server.abort();
}
