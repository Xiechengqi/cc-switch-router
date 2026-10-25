//! Session-only financial authorization. Receipts are notifications, never a
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

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
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
    // Do not use the general resolver here: it falls back from an invalid
    // bearer to a valid cookie. Financial writes authorize this bearer only.
    state
        .store
        .resolve_session_by_access_token(
            bearer_token(headers).ok_or_else(|| AppError::Unauthorized("login required".into()))?,
        )
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
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
    let id = Uuid::new_v4().to_string();
    conn.execute("INSERT INTO market_financial_authorizations
        (id,actor_user_id,session_reference,auth_source_kind,session_authenticated_at,session_access_expires_at,operation_kind,object_id,counterparty_user_id,amount_units,expected_version,fingerprint,acknowledgement,created_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![id,actor,session.session_id,session.auth_source_kind,session.created_at.to_rfc3339(),session.access_expires_at.to_rfc3339(),operation,object,counterparty,amount,version,fingerprint,acknowledgement,time])
        .map_err(|e| AppError::Internal(format!("record financial authorization failed: {e}")))?;
    Ok(id)
}

#[cfg(test)]
pub(crate) fn test_session(user: &str) -> AuthSession {
    let start = DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    AuthSession {
        session_id: format!("session-{user}"),
        user_id: user.into(),
        email: format!("{user}@example.test"),
        auth_source_kind: "email".into(),
        auth_source_id: "test-login".into(),
        access_token_hash: String::new(),
        refresh_token_hash: String::new(),
        access_expires_at: DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        refresh_expires_at: start,
        created_at: start,
        last_used_at: start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
