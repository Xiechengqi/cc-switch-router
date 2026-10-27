use super::*;
use ed25519_dalek::{Signer, SigningKey};

fn fixture() -> (AppStore, Config, SigningKey) {
    let config = crate::store::tests::test_config("password-auth");
    crate::test_support::database_at(&config.database.path).unwrap();
    let store = AppStore::new(&config).unwrap();
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    (store, config, key)
}

async fn seed(store: &AppStore, key: &SigningKey) -> AuthSession {
    let now = Utc::now();
    let conn = store.conn.lock().await;
    conn.execute("INSERT INTO auth_devices(id,public_key,kind,platform,app_version,status,created_at,last_seen_at)
        VALUES('browser',?1,'browser','test','test','active',?2,?2)",
        params![base64::engine::general_purpose::STANDARD.encode(key.verifying_key().as_bytes()),now.to_rfc3339()]).unwrap();
    let user = upsert_user_by_email(&conn, "buyer@example.test", now).unwrap();
    record_verified_email_user(&conn, &user.id, now).unwrap();
    let session = AuthSession {
        session_id: "email-session".into(),
        user_id: user.id,
        email: user.email,
        auth_source_kind: AUTH_SOURCE_AUTH_DEVICE.into(),
        auth_source_id: "browser".into(),
        access_token_hash: hash_token("email-access"),
        refresh_token_hash: hash_token("email-refresh"),
        access_expires_at: now + Duration::hours(1),
        refresh_expires_at: now + Duration::days(1),
        created_at: now,
        last_used_at: now,
    };
    persist_session(&conn, &session).unwrap();
    record_email_login(&conn, &session.session_id, now).unwrap();
    session
}

fn request(key: &SigningKey, email: &str, password: &str) -> PasswordLoginRequest {
    let now = Utc::now().timestamp_millis();
    let nonce = Uuid::new_v4().to_string();
    let body = serde_json::json!({"email":email,"password":password}).to_string();
    let payload = format!("{PROTOCOL_EPOCH}\nbrowser\nauth_password_login\n{body}\n{now}\n{nonce}");
    PasswordLoginRequest {
        email: email.into(),
        password: Zeroizing::new(password.to_owned()),
        auth_device_id: "browser".into(),
        timestamp_ms: now,
        nonce,
        signature: base64::engine::general_purpose::STANDARD
            .encode(key.sign(payload.as_bytes()).to_bytes()),
    }
}

fn metadata() -> ClientMetadata {
    ClientMetadata {
        ip: Some("192.0.2.1".into()),
        country_code: None,
    }
}

#[test]
fn password_policy_preserves_spaces_and_accepts_passphrases() {
    for password in [
        "  long password  ",
        "correct horse battery staple",
        "密".repeat(12).as_str(),
    ] {
        assert!(validate_password(password).is_ok());
    }
    for password in [
        "short",
        "            ",
        "invalid\nlong password",
        &"x".repeat(129),
    ] {
        assert!(validate_password(password).is_err());
    }
}

#[tokio::test]
async fn setup_login_change_cli_reset_and_refresh_revocation() {
    let _hash = HASH_TEST_LOCK.lock().await;
    let (store, config, key) = fixture();
    let original = seed(&store, &key).await;
    let setup = store
        .update_password(
            &config,
            &original,
            PasswordUpdateRequest {
                new_password: Zeroizing::new("first secure password".into()),
                current_password: None,
                expected_version: 0,
            },
            "192.0.2.1",
        )
        .await
        .unwrap();
    assert!(setup.password.as_ref().unwrap().has_password);
    assert_eq!(setup.password.as_ref().unwrap().version, 1);
    assert!(
        store
            .resolve_session_by_access_token("email-access")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .refresh_session(
                &config,
                RefreshSessionRequest {
                    refresh_token: "email-refresh".into()
                }
            )
            .await
            .is_err()
    );
    let password_login = store
        .password_login(
            &config,
            request(&key, "buyer@example.test", "first secure password"),
            metadata(),
        )
        .await
        .unwrap();
    assert!(
        !password_login
            .password
            .as_ref()
            .unwrap()
            .can_reset_without_password
    );
    let session = store
        .resolve_session_by_access_token(&password_login.access_token)
        .await
        .unwrap()
        .unwrap();
    let error = store
        .update_password(
            &config,
            &session,
            PasswordUpdateRequest {
                new_password: Zeroizing::new("next secure password".into()),
                current_password: Some(Zeroizing::new("incorrect password".into())),
                expected_version: 1,
            },
            "192.0.2.1",
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("password_current_invalid"));
    let changed = store
        .update_password(
            &config,
            &session,
            PasswordUpdateRequest {
                new_password: Zeroizing::new("next secure password".into()),
                current_password: Some(Zeroizing::new("first secure password".into())),
                expected_version: 1,
            },
            "192.0.2.1",
        )
        .await
        .unwrap();
    let rotated = store
        .refresh_session(
            &config,
            RefreshSessionRequest {
                refresh_token: changed.refresh_token.clone(),
            },
        )
        .await
        .unwrap();
    {
        let conn = store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let status = replace_tx(
            &tx,
            "buyer@example.test",
            "not-used-in-this-test",
            2,
            "local_cli",
            "uid:0",
            "cli-reset",
            Utc::now(),
        )
        .unwrap();
        assert_eq!(status.version, 3);
        tx.commit().unwrap();
    }
    for refresh in [changed.refresh_token, rotated.refresh_token] {
        assert!(
            store
                .refresh_session(
                    &config,
                    RefreshSessionRequest {
                        refresh_token: refresh
                    }
                )
                .await
                .is_err()
        );
    }
    assert!(
        store
            .resolve_session_by_access_token(&rotated.access_token)
            .await
            .unwrap()
            .is_none()
    );
    let conn = store.conn.lock().await;
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM auth_password_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM auth_password_notifications",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM user_api_tokens", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn refresh_preserves_real_email_proof_and_does_not_renew_it() {
    let (store, config, key) = fixture();
    let session = seed(&store, &key).await;
    let event = {
        let conn = store.conn.lock().await;
        let old = (Utc::now() - Duration::minutes(11)).to_rfc3339();
        conn.execute(
            "UPDATE user_sessions SET email_authenticated_at=?1 WHERE id=?2",
            params![old, session.session_id],
        )
        .unwrap();
        session_state(&conn, &session.session_id, Utc::now())
            .unwrap()
            .login_event_id
    };
    let refreshed = store
        .refresh_session(
            &config,
            RefreshSessionRequest {
                refresh_token: "email-refresh".into(),
            },
        )
        .await
        .unwrap();
    let state = refreshed.password.unwrap();
    assert_eq!(state.login_event_id, event);
    assert!(!state.can_reset_without_password);
    let session = store
        .resolve_session_by_access_token(&refreshed.access_token)
        .await
        .unwrap()
        .unwrap();
    let error = store
        .update_password(
            &config,
            &session,
            PasswordUpdateRequest {
                new_password: Zeroizing::new("long enough password".into()),
                current_password: None,
                expected_version: 0,
            },
            "192.0.2.1",
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("password_email_required"));
}

#[tokio::test]
async fn cli_cannot_create_verify_or_enable_accounts_and_stale_versions_fail() {
    let (store, _, key) = fixture();
    let session = seed(&store, &key).await;
    let conn = store.conn.lock().await;
    conn.execute(
        "UPDATE users SET status='disabled',email_verified_at=NULL WHERE id=?1",
        params![session.user_id],
    )
    .unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    assert!(
        replace_tx(
            &tx,
            "missing@example.test",
            "hash",
            0,
            "local_cli",
            "uid:0",
            "unknown",
            Utc::now()
        )
        .is_err()
    );
    let user = replace_tx(
        &tx,
        &session.email,
        "hash",
        0,
        "local_cli",
        "uid:0",
        "first",
        Utc::now(),
    )
    .unwrap();
    assert_eq!(user.status, "disabled");
    assert!(!user.email_verified);
    assert!(
        replace_tx(
            &tx,
            &session.email,
            "another",
            0,
            "local_cli",
            "uid:0",
            "stale",
            Utc::now()
        )
        .is_err()
    );
    tx.commit().unwrap();
    assert_eq!(status(&conn, &session.email).unwrap().version, 1);
}

#[test]
fn limits_bind_accounts_ips_devices_and_do_not_block_other_sources() {
    let conn = crate::test_support::database().unwrap();
    let now = Utc::now();
    for _ in 0..10 {
        reserve_attempt(&conn, "a@example.test", "ip-a", "device-a", now).unwrap();
    }
    assert!(reserve_attempt(&conn, "a@example.test", "ip-a", "new-device", now).is_err());
    assert!(reserve_attempt(&conn, "a@example.test", "ip-b", "device-b", now).is_ok());
    assert!(reserve_attempt(&conn, "b@example.test", "ip-a", "device-b", now).is_ok());
}

#[tokio::test]
async fn missing_password_login_is_uniform_and_signed_requests_cannot_be_replayed() {
    let _hash = HASH_TEST_LOCK.lock().await;
    let (store, config, key) = fixture();
    seed(&store, &key).await;
    for email in ["buyer@example.test", "unknown@example.test"] {
        let input = request(&key, email, "any long password");
        let mut replay = request(&key, email, "any long password");
        replay.nonce = input.nonce.clone();
        replay.timestamp_ms = input.timestamp_ms;
        replay.signature = input.signature.clone();
        let error = store
            .password_login(&config, input, metadata())
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("password_unavailable"));
        assert!(matches!(
            store.password_login(&config, replay, metadata()).await,
            Err(AppError::Unauthorized(_))
        ));
    }
    let mut tampered = request(&key, "buyer@example.test", "any long password");
    tampered.password = Zeroizing::new("another password".into());
    assert!(matches!(
        store.password_login(&config, tampered, metadata()).await,
        Err(AppError::Unauthorized(_))
    ));
    let mut stale = request(&key, "buyer@example.test", "any long password");
    stale.timestamp_ms -= SIGNED_REQUEST_MAX_SKEW_MS + 1;
    assert!(matches!(
        store.password_login(&config, stale, metadata()).await,
        Err(AppError::Unauthorized(_))
    ));
    let conn = store.conn.lock().await;
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM auth_device_nonces WHERE action='auth_password_login'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM auth_password_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM users WHERE email_normalized='unknown@example.test'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn passwords_never_verify_or_enable_an_account_and_reject_wrong_passwords() {
    let _hash = HASH_TEST_LOCK.lock().await;
    let (store, config, key) = fixture();
    let session = seed(&store, &key).await;
    let hash = hash_password(Zeroizing::new("  correct long password  ".into()))
        .await
        .unwrap();
    {
        let conn = store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        replace_tx(
            &tx,
            &session.email,
            &hash,
            0,
            "local_cli",
            "uid:0",
            "set-for-login",
            Utc::now(),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    for (account_status, verified, password) in [
        ("active", true, "correct long password"), // Do not trim password spaces.
        ("disabled", true, "  correct long password  "),
        ("active", false, "  correct long password  "),
    ] {
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE users SET status=?1,email_verified_at=?2 WHERE id=?3",
                params![
                    account_status,
                    verified.then(|| Utc::now().to_rfc3339()),
                    session.user_id
                ],
            )
            .unwrap();
        }
        let error = store
            .password_login(&config, request(&key, &session.email, password), metadata())
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("password_invalid"));
    }
    {
        let conn = store.conn.lock().await;
        assert!(!status(&conn, &session.email).unwrap().email_verified);
        conn.execute(
            "UPDATE users SET email_verified_at=?1 WHERE id=?2",
            params![Utc::now().to_rfc3339(), session.user_id],
        )
        .unwrap();
    }
    let result = store
        .password_login(
            &config,
            request(&key, &session.email, "  correct long password  "),
            metadata(),
        )
        .await
        .unwrap();
    assert_eq!(result.user.id, session.user_id);
    assert!(!result.password.unwrap().can_reset_without_password);
}

#[tokio::test]
async fn legacy_or_future_sessions_are_not_recent_email_proof_and_versions_are_authoritative() {
    let (store, config, key) = fixture();
    let session = seed(&store, &key).await;
    for proof in [None, Some((Utc::now() + Duration::minutes(1)).to_rfc3339())] {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE user_sessions SET auth_method='legacy',email_authenticated_at=?1 WHERE id=?2",
            params![proof, session.session_id],
        )
        .unwrap();
        assert!(
            !session_state(&conn, &session.session_id, Utc::now())
                .unwrap()
                .can_reset_without_password
        );
    }
    {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE users SET auth_version=1 WHERE id=?1",
            params![session.user_id],
        )
        .unwrap();
    }
    assert!(
        store
            .resolve_session_by_access_token("email-access")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .refresh_session(
                &config,
                RefreshSessionRequest {
                    refresh_token: "email-refresh".into()
                }
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn credential_audit_failure_rolls_back_hash_version_revocation_and_outbox() {
    let (store, _, key) = fixture();
    let session = seed(&store, &key).await;
    {
        let conn = store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        replace_tx(
            &tx,
            &session.email,
            "hash-one",
            0,
            "web",
            "test",
            "collision",
            Utc::now(),
        )
        .unwrap();
        tx.commit().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(
            replace_tx(
                &tx,
                &session.email,
                "hash-two",
                1,
                "web",
                "test",
                "collision",
                Utc::now()
            )
            .is_err()
        );
        // No commit on a failed credential mutation.
    }
    let conn = store.conn.lock().await;
    assert_eq!(status(&conn, &session.email).unwrap().version, 1);
    assert_eq!(
        conn.query_row(
            "SELECT password_hash FROM user_password_credentials WHERE user_id=?1",
            params![session.user_id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "hash-one"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM auth_password_notifications",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn security_notices_are_frozen_leased_bounded_english_and_logged_once() {
    let (store, config, key) = fixture();
    let session = seed(&store, &key).await;
    let now = Utc::now();
    let conn = store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    replace_tx(
        &tx,
        &session.email,
        "hash",
        0,
        "local_cli",
        "uid:0",
        "notice-one",
        now,
    )
    .unwrap();
    tx.commit().unwrap();
    let template = NotificationTemplateContext::from_config(&config);
    let first = claim_notice(&conn, "sender@example.test", &template, now)
        .unwrap()
        .unwrap();
    assert!(
        first
            .envelope
            .text
            .contains("All previous login sessions were revoked")
    );
    assert!(first.envelope.html.contains("<html"));
    assert!(
        !first
            .envelope
            .html
            .chars()
            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    );
    assert!(
        claim_notice(&conn, "sender@example.test", &template, now)
            .unwrap()
            .is_none()
    );
    let second = claim_notice(
        &conn,
        "changed@example.test",
        &template,
        now + Duration::minutes(3),
    )
    .unwrap()
    .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.envelope.from, "sender@example.test");
    assert_eq!(first.envelope.html, second.envelope.html);
    // An expired worker cannot overwrite the newer delivery lease.
    finish_notice(
        &conn,
        &first,
        Ok("late-old-message".into()),
        now + Duration::minutes(3),
    )
    .unwrap();
    assert_eq!(
        conn.query_row("SELECT status FROM auth_password_notifications", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "sending"
    );
    finish_notice(
        &conn,
        &second,
        Err(crate::notifications::DeliveryFailure {
            retryable: true,
            retry_at: None,
            message: "sensitive transport diagnostics".into(),
            failure_code: None,
            failure_hint: None,
            failure_details: None,
        }),
        now + Duration::minutes(3),
    )
    .unwrap();
    assert!(
        claim_notice(
            &conn,
            "changed@example.test",
            &template,
            now + Duration::minutes(4)
        )
        .unwrap()
        .is_none()
    );
    let retry = claim_notice(
        &conn,
        "changed@example.test",
        &template,
        now + Duration::hours(2),
    )
    .unwrap()
    .unwrap();
    finish_notice(
        &conn,
        &retry,
        Ok("one-message".into()),
        now + Duration::hours(2),
    )
    .unwrap();
    finish_notice(
        &conn,
        &retry,
        Ok("one-message".into()),
        now + Duration::hours(2),
    )
    .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM email_send_logs WHERE email_type='password_changed'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    replace_tx(
        &tx,
        &session.email,
        "hash-two",
        1,
        "web",
        "session",
        "notice-old",
        now - Duration::hours(13),
    )
    .unwrap();
    tx.commit().unwrap();
    assert!(
        claim_notice(&conn, "changed@example.test", &template, now)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        conn.query_row(
            "SELECT status FROM auth_password_notifications WHERE event_id='notice-old'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "failed"
    );
}

#[test]
fn distributed_attempts_and_persistent_storage_have_bounded_budgets() {
    let conn = crate::test_support::database().unwrap();
    let now = Utc::now();
    for i in 0..100 {
        reserve_attempt(
            &conn,
            "target@example.test",
            &format!("ip-{i}"),
            &format!("device-{i}"),
            now,
        )
        .unwrap();
    }
    assert!(reserve_attempt(&conn, "target@example.test", "new-ip", "new-device", now).is_err());
    conn.execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<1900)
        INSERT INTO auth_password_attempts SELECT 'bulk-'||x,'email-'||x,'ip-'||x,'device-'||x,?1 FROM n", params![now.to_rfc3339()]).unwrap();
    assert!(reserve_attempt(&conn, "another@example.test", "new-ip", "new-device", now).is_err());
    reserve_attempt(
        &conn,
        "target@example.test",
        "new-ip",
        "new-device",
        now + Duration::minutes(16),
    )
    .unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM auth_password_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
