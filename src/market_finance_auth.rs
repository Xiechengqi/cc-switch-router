//! Session, signed request and independent Passkey financial authorization. Receipts are notifications, never a
//! prerequisite for moving money. Evidence intentionally has no FK to live
//! sessions: logout/expiry must not invalidate an immutable financial journal.
use axum::http::{HeaderMap, header};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    ServerState,
    db::{Connection, params},
    error::AppError,
    models::AuthSession,
};

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return None;
    }
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_once(' ')
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty())
        .map(|(_, token)| token.trim())
}

pub(crate) fn check_write_origin(
    headers: &HeaderMap,
    trusted_origin: &str,
) -> Result<(), AppError> {
    // Financial browser calls already use authFetch's explicit session bearer.
    // Do not authorize with a domain-wide cookie sent by an unrelated subdomain.
    if bearer_token(headers).is_none() {
        return Err(AppError::Unauthorized(
            "a Router login session bearer is required".into(),
        ));
    }
    check_browser_origin(headers, trusted_origin)
}

/// Reject browser cross-origin writes. Non-browser signed clients may omit Origin.
pub(crate) fn check_browser_origin(
    headers: &HeaderMap,
    trusted_origin: &str,
) -> Result<(), AppError> {
    if headers.get_all(header::ORIGIN).iter().count() > 1
        || headers.get_all("sec-fetch-site").iter().count() > 1
    {
        return Err(AppError::Forbidden("ambiguous browser origin".into()));
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        return Err(AppError::Forbidden(
            "cross-origin financial request rejected".into(),
        ));
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin = origin
            .to_str()
            .ok()
            .and_then(|v| reqwest::Url::parse(v).ok());
        let trusted = reqwest::Url::parse(trusted_origin).ok();
        if !matches!((origin, trusted), (Some(a), Some(b)) if a.origin() == b.origin() && a.username().is_empty() && a.password().is_none() && a.path() == "/" && a.query().is_none() && a.fragment().is_none())
        {
            return Err(AppError::Forbidden(
                "cross-origin financial request rejected".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) async fn write_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    let scheme = if state.config.use_localhost {
        "http"
    } else {
        "https"
    };
    check_write_origin(
        headers,
        &format!(
            "{scheme}://{}",
            state.config.tunnel_domain.trim_end_matches('/')
        ),
    )?;
    // Financial writes authorize this explicit bearer only. Browser cookies
    // and development bypass identities cannot authorize financial writes.
    let mut session = state
        .store
        .resolve_session_by_access_token(
            bearer_token(headers).ok_or_else(|| AppError::Unauthorized("login required".into()))?,
        )
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    crate::market_security::attach_proof(&mut session, headers);
    Ok(session)
}

pub(crate) async fn write_owner_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    let session = write_session(state, headers).await?;
    if !crate::api::is_finance_owner(&state.config, &session.email) {
        return Err(AppError::Forbidden(
            "Router owner privilege required".into(),
        ));
    }
    Ok(session)
}

pub(crate) async fn write_admin_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    let session = write_session(state, headers).await?;
    if !state.dynamic.read().await.is_admin(&session.email) {
        return Err(AppError::Forbidden("admin privilege required".into()));
    }
    Ok(session)
}

/// Recheck inside the money transaction. Resolving HTTP credentials earlier
/// cannot fence a concurrent logout, credential reset or account suspension.
pub(crate) fn require_live_session_tx(
    conn: &Connection,
    session: &AuthSession,
    time: &str,
) -> Result<(), AppError> {
    let live: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM user_sessions s JOIN users u ON u.id=s.user_id
         WHERE s.id=?1 AND s.user_id=?2 AND s.access_token_hash=?3
           AND s.revoked_at IS NULL AND s.auth_version=u.auth_version AND u.status='active'
           AND julianday(s.access_expires_at)>julianday(?4)
           AND julianday(s.refresh_expires_at)>julianday(?4))",
            params![
                session.session_id,
                session.user_id,
                session.access_token_hash,
                time
            ],
            |r| r.get(0),
        )
        .map_err(|e| AppError::Internal(format!("recheck financial session failed: {e}")))?;
    if !live {
        return Err(AppError::Unauthorized(
            "login session is no longer valid".into(),
        ));
    }
    crate::market_security::validate_proof_tx(conn, session, time)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_tx(
    conn: &Connection,
    session: &AuthSession,
    actor: &str,
    operation: &str,
    object: &str,
    counterparty: &str,
    amount: i64,
    version: i64,
    fingerprint: &str,
    acknowledgement: &str,
    time: &str,
) -> Result<String, AppError> {
    let at = DateTime::parse_from_rfc3339(time)
        .map_err(|_| AppError::Internal("invalid authorization time".into()))?
        .with_timezone(&Utc);
    if session.user_id != actor
        || session.session_id.is_empty()
        || session.created_at > at
        || session.access_expires_at <= at
    {
        return Err(AppError::Unauthorized(
            "valid login session required".into(),
        ));
    }
    require_live_session_tx(conn, session, time)?;
    let id = Uuid::new_v4().to_string();
    conn.execute("INSERT INTO market_financial_authorizations
        (id,actor_user_id,session_reference,auth_source_kind,session_authenticated_at,session_access_expires_at,operation_kind,object_id,counterparty_user_id,amount_units,expected_version,fingerprint,acknowledgement,created_at,request_proof_id)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        params![id,actor,session.session_id,session.auth_source_kind,session.created_at.to_rfc3339(),session.access_expires_at.to_rfc3339(),operation,object,counterparty,amount,version,fingerprint,acknowledgement,time,session.financial_proof_id])
        .map_err(|e| AppError::Internal(format!("record financial authorization failed: {e}")))?;
    Ok(id)
}

#[cfg(test)]
pub(crate) fn test_session(conn: &Connection, user: &str) -> AuthSession {
    let start = DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let session = AuthSession {
        financial_proof_id: None,
        session_id: format!("session-{user}"),
        user_id: user.into(),
        email: format!("{user}@example.test"),
        auth_source_kind: "email".into(),
        auth_source_id: "test-login".into(),
        access_token_hash: format!("test-access-{user}"),
        refresh_token_hash: format!("test-refresh-{user}"),
        access_expires_at: DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        refresh_expires_at: DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        created_at: start,
        last_used_at: start,
    };
    conn.execute("INSERT OR IGNORE INTO users(id,email_normalized,status,created_at,last_login_at,email_verified_at) VALUES(?1,?2,'active',?3,?3,?3)", params![user,session.email,start.to_rfc3339()]).unwrap();
    conn.execute("INSERT OR IGNORE INTO user_sessions(id,user_id,auth_source_kind,auth_source_id,access_token_hash,refresh_token_hash,access_expires_at,refresh_expires_at,created_at,last_used_at,auth_version)
        VALUES(?1,?2,'auth_device','test-login',?3,?4,?5,?5,?6,?6,(SELECT auth_version FROM users WHERE id=?2))",
        params![session.session_id,user,session.access_token_hash,session.refresh_token_hash,session.access_expires_at.to_rfc3339(),start.to_rfc3339()]).unwrap();
    session
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revocation_and_auth_version_changes_fence_an_already_resolved_financial_session() {
        let conn = crate::test_support::database().unwrap();
        let session = test_session(&conn, "finance-revocation");
        let time = "2026-10-25T12:00:00Z";
        require_live_session_tx(&conn, &session, time).unwrap();
        conn.execute(
            "UPDATE user_sessions SET revoked_at=?2 WHERE id=?1",
            params![session.session_id, time],
        )
        .unwrap();
        assert!(require_live_session_tx(&conn, &session, time).is_err());
        conn.execute(
            "UPDATE user_sessions SET revoked_at=NULL WHERE id=?1",
            params![session.session_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE users SET auth_version=auth_version+1 WHERE id=?1",
            params![session.user_id],
        )
        .unwrap();
        assert!(require_live_session_tx(&conn, &session, time).is_err());
    }
    #[test]
    fn password_login_origin_check_does_not_require_a_session_but_rejects_foreign_browsers() {
        let mut headers = HeaderMap::new();
        assert!(check_browser_origin(&headers, "https://router.test").is_ok());
        headers.insert(header::ORIGIN, "https://router.test".parse().unwrap());
        assert!(check_browser_origin(&headers, "https://router.test").is_ok());
        headers.insert(header::ORIGIN, "https://share.router.test".parse().unwrap());
        assert!(check_browser_origin(&headers, "https://router.test").is_err());
        headers.remove(header::ORIGIN);
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(check_browser_origin(&headers, "https://router.test").is_err());
    }
    #[test]
    fn financial_writes_require_an_explicit_session_and_exact_trusted_origin() {
        let mut h = HeaderMap::new();
        assert!(check_write_origin(&h, "https://router.test").is_err());
        h.insert(
            header::COOKIE,
            "router_access=valid-cookie".parse().unwrap(),
        );
        assert!(
            bearer_token(&h).is_none(),
            "cookies cannot supply a financial bearer"
        );
        assert!(check_write_origin(&h, "https://router.test").is_err());
        h.insert(header::AUTHORIZATION, "Bearer a-session".parse().unwrap());
        assert_eq!(bearer_token(&h), Some("a-session"));
        assert!(check_write_origin(&h, "https://router.test").is_ok());
        for origin in [
            "https://router.test.evil",
            "https://share.router.test",
            "null",
            "http://router.test",
            "https://router.test/path",
        ] {
            h.insert(header::ORIGIN, origin.parse().unwrap());
            assert!(
                check_write_origin(&h, "https://router.test").is_err(),
                "{origin}"
            );
        }
        h.insert(header::ORIGIN, "https://router.test".parse().unwrap());
        assert!(check_write_origin(&h, "https://router.test").is_ok());
        h.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(check_write_origin(&h, "https://router.test").is_err());
    }
}
