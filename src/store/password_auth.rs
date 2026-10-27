//! Optional Router passwords. All credential mutations share one versioned
//! transaction; neither API keys nor a refreshed session prove recent email auth.
use super::*;
use argon2::{
    Argon2, Params as ArgonParams, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::http::StatusCode;
use serde::Deserialize;
use std::sync::OnceLock;
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

pub(crate) const RECENT_EMAIL_SECS: i64 = 600;
const MAX_PASSWORD_BYTES: usize = 512;
#[cfg(test)]
pub(crate) static HASH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordState {
    pub has_password: bool,
    pub version: i64,
    pub can_reset_without_password: bool,
    pub login_event_id: Option<String>,
}

// Deliberately no Debug: request bodies contain credentials.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PasswordLoginRequest {
    pub email: String,
    pub password: Zeroizing<String>,
    pub auth_device_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PasswordUpdateRequest {
    pub new_password: Zeroizing<String>,
    pub current_password: Option<Zeroizing<String>>,
    pub expected_version: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PasswordStatus {
    pub user_id: String,
    pub email: String,
    pub status: String,
    pub email_verified: bool,
    pub has_password: bool,
    pub version: i64,
    pub updated_at: Option<String>,
}

pub(crate) fn coded(status: StatusCode, code: &'static str, message: &str) -> AppError {
    AppError::Coded {
        status,
        code,
        message: message.into(),
        details: serde_json::json!({}),
    }
}

fn invalid_password() -> AppError {
    coded(
        StatusCode::UNAUTHORIZED,
        "password_invalid",
        "Email or password is incorrect.",
    )
}

pub(crate) fn validate_password(password: &str) -> Result<(), AppError> {
    let chars = password.chars().count();
    if !(12..=128).contains(&chars)
        || password.len() > MAX_PASSWORD_BYTES
        || password.chars().any(char::is_control)
        || password.trim().is_empty()
    {
        return Err(coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "password_policy",
            "Use 12–128 characters without control characters.",
        ));
    }
    Ok(())
}

fn algorithm() -> Argon2<'static> {
    // Two concurrent jobs cap Argon2 working memory at 128 MiB.
    Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        ArgonParams::new(65_536, 3, 1, None).expect("constant Argon2 parameters"),
    )
}

async fn password_job<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    static JOBS: Semaphore = Semaphore::const_new(2);
    let permit = JOBS.try_acquire().map_err(|_| {
        coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "password_busy",
            "Password authentication is busy. Please retry shortly.",
        )
    })?;
    // Keep the permit even if the caller disconnects: blocking jobs cannot be cancelled.
    let owned = std::sync::Arc::new(permit);
    tokio::task::spawn_blocking(move || {
        let _permit = owned;
        job()
    })
    .await
    .map_err(|_| AppError::Internal("password worker failed".into()))?
}

pub(crate) async fn hash_password(password: Zeroizing<String>) -> Result<String, AppError> {
    validate_password(&password)?;
    password_job(move || {
        algorithm()
            .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
            .map(|v| v.to_string())
            .map_err(|_| AppError::Internal("password hashing failed".into()))
    })
    .await
}

async fn check_password(
    password: Zeroizing<String>,
    hash: Option<String>,
) -> Result<bool, AppError> {
    password_job(move || {
        // Lazily generated once, using the same algorithm and cost as real credentials.
        static DUMMY: OnceLock<String> = OnceLock::new();
        let real = hash.is_some();
        let hash = hash.as_deref().unwrap_or_else(|| {
            DUMMY.get_or_init(|| {
                algorithm()
                    .hash_password(
                        b"not-an-account-password",
                        &SaltString::generate(&mut OsRng),
                    )
                    .expect("constant dummy password")
                    .to_string()
            })
        });
        let parsed = PasswordHash::new(hash)
            .map_err(|_| AppError::Internal("invalid stored password credential".into()))?;
        Ok(algorithm()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
            && real)
    })
    .await
}

pub(crate) fn record_email_login(
    conn: &Connection,
    id: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE user_sessions SET auth_method='email', authenticated_at=?2,
        email_authenticated_at=?2, login_event_id=?3 WHERE id=?1",
        params![id, now.to_rfc3339(), Uuid::new_v4().to_string()],
    )
    .map_err(|e| AppError::Internal(format!("record primary authentication failed: {e}")))?;
    Ok(())
}

pub(crate) fn inherit_login(conn: &Connection, from: &str, to: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE user_sessions SET
        (auth_method,authenticated_at,email_authenticated_at,login_event_id)=
        (SELECT auth_method,authenticated_at,email_authenticated_at,login_event_id
         FROM user_sessions WHERE id=?1) WHERE id=?2",
        params![from, to],
    )
    .map_err(|e| AppError::Internal(format!("inherit primary authentication failed: {e}")))?;
    Ok(())
}

pub(crate) fn session_state(
    conn: &Connection,
    id: &str,
    now: DateTime<Utc>,
) -> Result<PasswordState, AppError> {
    let (has, at, event, version): (bool, Option<String>, Option<String>, i64) = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_password_credentials p WHERE p.user_id=s.user_id),
            s.email_authenticated_at,s.login_event_id,s.auth_version FROM user_sessions s WHERE s.id=?1",
        params![id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))
        .map_err(|e| AppError::Internal(format!("read password session state failed: {e}")))?;
    let recent = at
        .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
        .is_some_and(|at| {
            at <= now && now.signed_duration_since(at).num_seconds() <= RECENT_EMAIL_SECS
        });
    Ok(PasswordState {
        has_password: has,
        version,
        can_reset_without_password: recent,
        login_event_id: event,
    })
}

pub(crate) fn status(conn: &Connection, email: &str) -> Result<PasswordStatus, AppError> {
    let email = normalize_email(email)?;
    conn.query_row(
        "SELECT u.id,u.email_normalized,u.status,u.email_verified_at IS NOT NULL,
        p.user_id IS NOT NULL,u.auth_version,p.updated_at
        FROM users u LEFT JOIN user_password_credentials p ON p.user_id=u.id
        WHERE u.email_normalized=?1",
        params![email],
        |r| {
            Ok(PasswordStatus {
                user_id: r.get(0)?,
                email: r.get(1)?,
                status: r.get(2)?,
                email_verified: r.get(3)?,
                has_password: r.get(4)?,
                version: r.get(5)?,
                updated_at: r.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Internal(format!("read password status failed: {e}")))?
    .ok_or_else(|| {
        AppError::NotFound("user not found; password management does not create accounts".into())
    })
}

fn reserve_attempt(
    conn: &Connection,
    email: &str,
    source: &str,
    device: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let email = hash_token(email);
    let source = hash_token(source);
    let cutoff = (now - Duration::minutes(15)).to_rfc3339();
    let (pair, account, ip, device_count, total): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
             (SELECT COUNT(*) FROM auth_password_attempts WHERE email_hash=?1 AND source_hash=?2 AND created_at>=?4),
             (SELECT COUNT(*) FROM auth_password_attempts WHERE email_hash=?1 AND created_at>=?4),
             (SELECT COUNT(*) FROM auth_password_attempts WHERE source_hash=?2 AND created_at>=?4),
             (SELECT COUNT(*) FROM auth_password_attempts WHERE device_id=?3 AND created_at>=?4),
             (SELECT COUNT(*) FROM auth_password_attempts WHERE created_at>=?4)",
            params![email, source, device, cutoff],
            |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                ))
            },
        )
        .map_err(|e| AppError::Internal(format!("read password attempt limits failed: {e}")))?;
    // Email OTP remains usable even if a distributed attack exhausts password limits.
    // The global budget also bounds this persistent table during a flood.
    if pair >= 10 || account >= 100 || ip >= 60 || device_count >= 20 || total >= 2000 {
        return Err(AppError::RateLimited {
            message: "Too many password attempts. Try again later or use an email code.".into(),
            retry_after_secs: 900,
        });
    }
    conn.execute(
        "DELETE FROM auth_password_attempts WHERE created_at<?1",
        params![cutoff],
    )
    .map_err(|e| AppError::Internal(format!("prune password attempts failed: {e}")))?;
    conn.execute(
        "INSERT INTO auth_password_attempts VALUES(?1,?2,?3,?4,?5)",
        params![
            Uuid::new_v4().to_string(),
            email,
            source,
            device,
            now.to_rfc3339()
        ],
    )
    .map_err(|e| AppError::Internal(format!("record password attempt failed: {e}")))?;
    Ok(())
}

/// Caller owns an immediate transaction. Also used by the offline rescue CLI.
pub(crate) fn replace_tx(
    conn: &Connection,
    email: &str,
    hash: &str,
    expected: i64,
    source: &str,
    actor: &str,
    event: &str,
    now: DateTime<Utc>,
) -> Result<PasswordStatus, AppError> {
    let user = status(conn, email)?;
    if user.version != expected {
        return Err(coded(
            StatusCode::CONFLICT,
            "password_changed",
            "Password changed elsewhere. Reload and retry.",
        ));
    }
    let updated = conn
        .execute(
            "UPDATE users SET auth_version=auth_version+1 WHERE id=?1 AND auth_version=?2",
            params![user.user_id, expected],
        )
        .map_err(|e| AppError::Internal(format!("advance authentication version failed: {e}")))?;
    if updated != 1 {
        return Err(coded(
            StatusCode::CONFLICT,
            "password_changed",
            "Password changed elsewhere. Reload and retry.",
        ));
    }
    conn.execute("INSERT INTO user_password_credentials(user_id,password_hash,updated_at) VALUES(?1,?2,?3)
        ON CONFLICT(user_id) DO UPDATE SET password_hash=excluded.password_hash,updated_at=excluded.updated_at",
        params![user.user_id,hash,now.to_rfc3339()])
        .map_err(|e| AppError::Internal(format!("replace password failed: {e}")))?;
    conn.execute(
        "UPDATE user_sessions SET revoked_at=COALESCE(revoked_at,?2),rotated_at=NULL WHERE user_id=?1",
        params![user.user_id, now.to_rfc3339()],
    )
    .map_err(|e| AppError::Internal(format!("revoke password sessions failed: {e}")))?;
    conn.execute(
        "INSERT INTO auth_password_events VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            event,
            user.user_id,
            expected + 1,
            source,
            actor,
            now.to_rfc3339()
        ],
    )
    .map_err(|e| AppError::Internal(format!("record password event failed: {e}")))?;
    conn.execute(
        "INSERT INTO auth_password_notifications(event_id,next_attempt_at) VALUES(?1,?2)",
        params![event, now.to_rfc3339()],
    )
    .map_err(|e| AppError::Internal(format!("queue password notice failed: {e}")))?;
    status(conn, email)
}

fn issue_session(
    conn: &Connection,
    config: &Config,
    user: AuthUser,
    device: &str,
    now: DateTime<Utc>,
) -> Result<(AuthSession, VerifyEmailCodeResponse), AppError> {
    let access_token = generate_secret(48);
    let refresh_token = generate_secret(64);
    let session = AuthSession {
        session_id: Uuid::new_v4().to_string(),
        user_id: user.id.clone(),
        email: user.email.clone(),
        auth_source_kind: AUTH_SOURCE_AUTH_DEVICE.into(),
        auth_source_id: device.into(),
        access_token_hash: hash_token(&access_token),
        refresh_token_hash: hash_token(&refresh_token),
        access_expires_at: now + Duration::seconds(config.auth_session_ttl_secs),
        refresh_expires_at: now + Duration::seconds(config.auth_refresh_ttl_secs),
        created_at: now,
        last_used_at: now,
    };
    persist_session(conn, &session)?;
    conn.execute("UPDATE user_sessions SET auth_method='password',authenticated_at=?2,login_event_id=?3 WHERE id=?1",
        params![session.session_id,now.to_rfc3339(),Uuid::new_v4().to_string()])
        .map_err(|e| AppError::Internal(format!("record password authentication failed: {e}")))?;
    let response = VerifyEmailCodeResponse {
        password: Some(session_state(conn, &session.session_id, now)?),
        user,
        access_token,
        refresh_token,
        expires_at: session.access_expires_at,
        refresh_expires_at: session.refresh_expires_at,
        api_token: None,
        api_token_prefix: None,
    };
    Ok((session, response))
}

impl AppStore {
    pub(crate) async fn password_login(
        &self,
        config: &Config,
        input: PasswordLoginRequest,
        metadata: ClientMetadata,
    ) -> Result<VerifyEmailCodeResponse, AppError> {
        let email = normalize_email(&input.email)?;
        let password = input.password;
        if password.len() > MAX_PASSWORD_BYTES || input.auth_device_id.len() > 100 {
            return Err(invalid_password());
        }
        let now = Utc::now();
        let (user, hash) = {
            let conn = self.conn.lock().await;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| AppError::Internal(format!("begin password login failed: {e}")))?;
            let key = auth_source_public_key(&tx, AUTH_SOURCE_AUTH_DEVICE, &input.auth_device_id)?;
            validate_request_nonce(&input.nonce)?;
            if now.timestamp_millis().abs_diff(input.timestamp_ms)
                > SIGNED_REQUEST_MAX_SKEW_MS as u64
            {
                return Err(AppError::Unauthorized("stale signed request".into()));
            }
            verify_signed_payload(
                &key,
                &input.auth_device_id,
                "auth_password_login",
                &serde_json::json!({"email":email,"password":password.as_str()}),
                input.timestamp_ms,
                &input.nonce,
                &input.signature,
            )?;
            consume_auth_source_nonce(
                &tx,
                AUTH_SOURCE_AUTH_DEVICE,
                &input.auth_device_id,
                "auth_password_login",
                &input.nonce,
                now,
            )?;
            reserve_attempt(
                &tx,
                &email,
                metadata.ip.as_deref().unwrap_or("unknown"),
                &input.auth_device_id,
                now,
            )?;
            let user = match status(&tx, &email) {
                Ok(user) => Some(user),
                Err(AppError::NotFound(_)) => None,
                Err(e) => return Err(e),
            };
            let hash = tx.query_row("SELECT password_hash FROM user_password_credentials WHERE user_id=(SELECT id FROM users WHERE email_normalized=?1)",
                params![email],|r| r.get::<_,String>(0)).optional()
                .map_err(|e| AppError::Internal(format!("read password credential failed: {e}")))?;
            tx.commit()
                .map_err(|e| AppError::Internal(format!("reserve password login failed: {e}")))?;
            (user, hash)
        };
        let valid = check_password(password, hash).await?;
        let Some(user) = user.filter(|u| u.has_password) else {
            return Err(coded(
                StatusCode::UNAUTHORIZED,
                "password_unavailable",
                "This account cannot use password login. Sign in with an email code, then set a login password.",
            ));
        };
        if !valid || user.status != "active" || !user.email_verified {
            return Err(invalid_password());
        }
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| AppError::Internal(format!("begin password session failed: {e}")))?;
        let current = status(&tx, &email)?;
        if current.version != user.version || current.status != "active" || !current.email_verified
        {
            return Err(invalid_password());
        }
        auth_source_public_key(&tx, AUTH_SOURCE_AUTH_DEVICE, &input.auth_device_id)?;
        let (_, response) = issue_session(
            &tx,
            config,
            AuthUser {
                id: user.user_id,
                email: email.clone(),
            },
            &input.auth_device_id,
            now,
        )?;
        tx.execute(
            "UPDATE users SET last_login_at=?2 WHERE email_normalized=?1",
            params![email, now.to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("update password login time failed: {e}")))?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit password login failed: {e}")))?;
        Ok(response)
    }

    pub(crate) async fn update_password(
        &self,
        config: &Config,
        session: &AuthSession,
        input: PasswordUpdateRequest,
        source_ip: &str,
    ) -> Result<VerifyEmailCodeResponse, AppError> {
        let new_password = input.new_password;
        validate_password(&new_password)?;
        let old = input.current_password;
        let (recent, hash) = {
            let conn = self.conn.lock().await;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| AppError::Internal(format!("begin password proof failed: {e}")))?;
            let current = get_session_by_access_hash(&tx, &session.access_token_hash)?
                .filter(|s| s.access_expires_at > Utc::now())
                .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
            let user = status(&tx, &current.email)?;
            if user.version != input.expected_version {
                return Err(coded(
                    StatusCode::CONFLICT,
                    "password_changed",
                    "Password changed elsewhere. Reload and retry.",
                ));
            }
            let recent =
                session_state(&tx, &current.session_id, Utc::now())?.can_reset_without_password;
            let hash = tx
                .query_row(
                    "SELECT password_hash FROM user_password_credentials WHERE user_id=?1",
                    params![current.user_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| AppError::Internal(format!("read current password failed: {e}")))?;
            if !recent {
                if hash.is_none() || old.is_none() {
                    return Err(coded(
                        StatusCode::FORBIDDEN,
                        "password_email_required",
                        "Sign in again with an email code before setting or resetting a password.",
                    ));
                }
                reserve_attempt(
                    &tx,
                    &session.email,
                    source_ip,
                    &session.auth_source_id,
                    Utc::now(),
                )?;
            }
            tx.commit()
                .map_err(|e| AppError::Internal(format!("commit password proof failed: {e}")))?;
            (recent, hash)
        };
        if !recent && !check_password(old.unwrap_or_default(), hash).await? {
            return Err(coded(
                StatusCode::FORBIDDEN,
                "password_current_invalid",
                "Current password is incorrect.",
            ));
        }
        let hash = hash_password(new_password).await?;
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| AppError::Internal(format!("begin password update failed: {e}")))?;
        let current = get_session_by_access_hash(&tx, &session.access_token_hash)?
            .filter(|s| s.access_expires_at > now)
            .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
        if recent && !session_state(&tx, &current.session_id, now)?.can_reset_without_password {
            return Err(coded(
                StatusCode::FORBIDDEN,
                "password_email_required",
                "Recent email verification expired. Sign in again with an email code.",
            ));
        }
        replace_tx(
            &tx,
            &current.email,
            &hash,
            input.expected_version,
            "web",
            &current.session_id,
            &Uuid::new_v4().to_string(),
            now,
        )?;
        let (replacement, mut response) = issue_session(
            &tx,
            config,
            AuthUser {
                id: current.user_id,
                email: current.email,
            },
            &current.auth_source_id,
            now,
        )?;
        // Retain the device/source and original proof, not a new email timestamp.
        tx.execute(
            "UPDATE user_sessions SET auth_source_kind=?2 WHERE id=?1",
            params![replacement.session_id, current.auth_source_kind],
        )
        .map_err(|e| AppError::Internal(format!("retain password session source failed: {e}")))?;
        inherit_login(&tx, &current.session_id, &replacement.session_id)?;
        response.password = Some(session_state(&tx, &replacement.session_id, now)?);
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit password update failed: {e}")))?;
        Ok(response)
    }
}

#[derive(Serialize, Deserialize)]
struct NoticeEnvelope {
    from: String,
    recipient: String,
    html: String,
    text: String,
    reply_to: Option<String>,
}

/// Frozen envelopes and stable provider idempotency keys prevent retries from
/// sending multiple receipts. Six attempts stay inside Resend's 24-hour window.
pub(crate) async fn deliver_notices(store: AppStore, config: Config) -> anyhow::Result<()> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        interval.tick().await;
        let Some(key) = config.resend_api_key.as_deref().filter(|v| !v.is_empty()) else {
            continue;
        };
        let template = NotificationTemplateContext::from_config(&config);
        let Some(sender) = template.sender.as_deref() else {
            continue;
        };
        // A transient database/transport error must not kill this background worker.
        if deliver_one_notice(&store, &http, key, sender, &template)
            .await
            .is_err()
        {
            tracing::warn!("password security notice delivery deferred; will retry");
        }
    }
}

struct ClaimedNotice {
    id: String,
    attempt: i64,
    envelope: NoticeEnvelope,
}

fn claim_notice(
    conn: &Connection,
    sender: &str,
    template: &NotificationTemplateContext,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<ClaimedNotice>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let candidate: Option<(String, String, String, String, Option<String>, i64)> = tx
        .query_row(
            "SELECT e.id,u.email_normalized,e.created_at,e.source,n.envelope_json,n.attempts
                 FROM auth_password_notifications n JOIN auth_password_events e ON e.id=n.event_id
                 JOIN users u ON u.id=e.user_id WHERE n.status IN ('pending','sending')
                 AND n.next_attempt_at<=?1 ORDER BY e.created_at LIMIT 1",
            params![now.to_rfc3339()],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;
    if let Some((id, email, at, source, frozen, attempts)) = candidate {
        let age = DateTime::parse_from_rfc3339(&at)
            .ok()
            .map(|at| now.signed_duration_since(at));
        if attempts >= 6 || age.is_none_or(|age| age > Duration::hours(12)) {
            tx.execute("UPDATE auth_password_notifications SET status='failed',last_error='delivery window exhausted' WHERE event_id=?1",params![id])?;
            tx.commit()?;
            return Ok(None);
        }
        let envelope = if let Some(frozen) = frozen {
            frozen
        } else {
            let text = format!(
                "Your TokenSwitch login password was changed.\nRecorded: {at}\nSource: {source}\nAll previous login sessions were revoked.\nIf this was not you, sign in with an email code and reset your password, or contact the Router operator.\n{}",
                template.dashboard_url
            );
            let html = render_email_layout(
                "Login password changed",
                "Your account security settings changed.",
                &format!(
                    "<p>Your TokenSwitch login password was changed.</p><p>Recorded: {}</p><p>Source: {}</p><p>All previous login sessions were revoked.</p><p>If this was not you, sign in with an email code and reset your password, or contact the Router operator.</p><p><a href=\"{}\">Open TokenSwitch</a></p>",
                    escape_html(&at),
                    escape_html(&source),
                    escape_html(&template.dashboard_url)
                ),
            );
            serde_json::to_string(&NoticeEnvelope {
                from: sender.into(),
                recipient: email,
                html,
                text,
                reply_to: template.reply_to.clone(),
            })?
        };
        // A corrupt frozen envelope must not block the rest of the queue.
        let parsed: NoticeEnvelope = match serde_json::from_str(&envelope) {
            Ok(value) => value,
            Err(_) => {
                tx.execute("UPDATE auth_password_notifications SET status='failed',last_error='invalid frozen notice' WHERE event_id=?1", params![id])?;
                tx.commit()?;
                return Ok(None);
            }
        };
        tx.execute("UPDATE auth_password_notifications SET status='sending',attempts=attempts+1,next_attempt_at=?2,envelope_json=?3 WHERE event_id=?1",
                    params![id,(now+Duration::minutes(2)).to_rfc3339(),envelope])?;
        tx.commit()?;
        Ok(Some(ClaimedNotice {
            id,
            attempt: attempts + 1,
            envelope: parsed,
        }))
    } else {
        tx.commit()?;
        Ok(None)
    }
}

async fn deliver_one_notice(
    store: &AppStore,
    http: &reqwest::Client,
    key: &str,
    sender: &str,
    template: &NotificationTemplateContext,
) -> anyhow::Result<()> {
    let notice = {
        let conn = store.conn.lock().await;
        claim_notice(&conn, sender, template, Utc::now())?
    };
    let Some(notice) = notice else {
        return Ok(());
    };
    let envelope = &notice.envelope;
    let idempotency = format!("router-password-{}", notice.id);
    let sent = crate::notifications::send_resend_frozen_email(
        http,
        key,
        crate::notifications::FrozenEmailEnvelope {
            from: &envelope.from,
            recipient: &envelope.recipient,
            subject: "Your TokenSwitch login password changed",
            html: &envelope.html,
            text: &envelope.text,
            reply_to: envelope.reply_to.as_deref(),
            idempotency_key: &idempotency,
        },
    )
    .await;
    let conn = store.conn.lock().await;
    finish_notice(&conn, &notice, sent, Utc::now())
}

fn finish_notice(
    conn: &Connection,
    notice: &ClaimedNotice,
    sent: Result<String, crate::notifications::DeliveryFailure>,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let (status, next, error, message_id) = match sent {
        Ok(id) => ("sent", now, None, Some(id)),
        Err(e) if e.retryable => (
            "pending",
            e.retry_at
                .unwrap_or(now + Duration::hours(1))
                .max(now + Duration::hours(1)),
            Some("email provider temporarily unavailable"),
            None,
        ),
        Err(_) => (
            "failed",
            now,
            Some("email provider rejected notification"),
            None,
        ),
    };
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let updated = tx.execute("UPDATE auth_password_notifications SET status=?2,next_attempt_at=?3,last_error=?4 WHERE event_id=?1 AND status='sending' AND attempts=?5",
            params![notice.id,status,next.to_rfc3339(),error,notice.attempt])?;
    if updated == 1 && status == "sent" {
        tx.execute("INSERT OR IGNORE INTO email_send_logs(id,email_type,to_email,provider_message_id,status,created_at)
                VALUES(?1,'password_changed',?2,?3,'sent',?4)", params![format!("password-{}",notice.id),notice.envelope.recipient,message_id,now.to_rfc3339()])?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests;
