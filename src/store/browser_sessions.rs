//! Host-only browser credentials. Access and refresh tokens are never cookies.
use super::*;

fn resolve_tx(conn: &Connection, cookie: &str) -> Result<Option<AuthSession>, AppError> {
    if !cookie.starts_with("browser_") || cookie.len() > 256 {
        return Ok(None);
    }
    let hash: Option<String> = conn
        .query_row(
            "SELECT s.access_token_hash FROM router_browser_sessions b
         JOIN user_sessions s ON s.id=b.session_id
         WHERE b.cookie_hash=?1 AND julianday(b.expires_at)>julianday(?2)",
            params![hash_token(cookie), Utc::now().to_rfc3339()],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| AppError::Internal(format!("resolve browser credential failed: {e}")))?;
    let Some(hash) = hash else { return Ok(None) };
    Ok(get_session_by_access_hash(conn, &hash)?
        .filter(|s| s.access_expires_at > Utc::now() && s.refresh_expires_at > Utc::now()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_credentials_are_separate_and_follow_session_revocation() {
        let conn = crate::test_support::database().unwrap();
        let session = crate::market_finance_auth::test_session(&conn, "browser-user");
        let cookie = "browser_a_distinct_random_credential";
        conn.execute("INSERT INTO router_browser_sessions VALUES(?1,?2,'2100-01-01T00:00:00Z','2026-10-25T00:00:00Z')", params![hash_token(cookie),session.session_id]).unwrap();
        assert_eq!(
            resolve_tx(&conn, cookie).unwrap().unwrap().user_id,
            session.user_id
        );
        assert!(
            get_session_by_access_hash(&conn, &hash_token(cookie))
                .unwrap()
                .is_none()
        );
        assert!(
            resolve_tx(&conn, "test-access-browser-user")
                .unwrap()
                .is_none()
        );
        conn.execute(
            "UPDATE user_sessions SET revoked_at='2026-10-25' WHERE id=?1",
            params![session.session_id],
        )
        .unwrap();
        assert!(resolve_tx(&conn, cookie).unwrap().is_none());
    }
}

impl AppStore {
    pub(crate) async fn issue_browser_cookie(&self, access: &str) -> Result<String, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| AppError::Internal(format!("begin browser credential failed: {e}")))?;
        let session = get_session_by_access_hash(&tx, &hash_token(access))?
            .filter(|s| s.access_expires_at > Utc::now())
            .ok_or_else(|| AppError::Unauthorized("login session expired".into()))?;
        let cookie = format!("browser_{}", generate_secret(48));
        tx.execute(
            "DELETE FROM router_browser_sessions WHERE session_id=?1 OR expires_at<=?2",
            params![session.session_id, Utc::now().to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("prune browser credentials failed: {e}")))?;
        tx.execute("INSERT INTO router_browser_sessions(cookie_hash,session_id,expires_at,created_at) VALUES(?1,?2,?3,?4)",
            params![hash_token(&cookie), session.session_id, session.access_expires_at.to_rfc3339(), Utc::now().to_rfc3339()])
            .map_err(|e| AppError::Internal(format!("issue browser credential failed: {e}")))?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit browser credential failed: {e}")))?;
        Ok(cookie)
    }

    pub(crate) async fn resolve_browser_session(
        &self,
        cookie: &str,
    ) -> Result<Option<AuthSession>, AppError> {
        let conn = self.conn.lock().await;
        // An IMMEDIATE transaction reads authoritative state with remote libSQL.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| AppError::Internal(format!("begin browser resolution failed: {e}")))?;
        let session = resolve_tx(&tx, cookie)?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit browser resolution failed: {e}")))?;
        Ok(session)
    }

    pub(crate) async fn revoke_browser_session(&self, cookie: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| AppError::Internal(format!("begin browser logout failed: {e}")))?;
        tx.execute(
            "UPDATE user_sessions SET revoked_at=COALESCE(revoked_at,?2),rotated_at=NULL
            WHERE id IN(SELECT session_id FROM router_browser_sessions WHERE cookie_hash=?1)",
            params![hash_token(cookie), Utc::now().to_rfc3339()],
        )
        .map_err(|e| AppError::Internal(format!("revoke browser session failed: {e}")))?;
        tx.execute(
            "DELETE FROM router_browser_sessions WHERE cookie_hash=?1",
            params![hash_token(cookie)],
        )
        .map_err(|e| AppError::Internal(format!("remove browser credential failed: {e}")))?;
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit browser logout failed: {e}")))
    }

    pub(crate) async fn resolved_session_status(
        &self,
        session: &AuthSession,
    ) -> Result<SessionStatusResponse, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| AppError::Internal(format!("begin resolved status failed: {e}")))?;
        let current = get_session_by_access_hash(&tx, &session.access_token_hash)?
            .filter(|s| s.access_expires_at > Utc::now())
            .ok_or_else(|| AppError::Unauthorized("login session expired".into()))?;
        let password = Some(password_auth::session_state(&tx, &current.session_id)?);
        tx.commit()
            .map_err(|e| AppError::Internal(format!("commit resolved status failed: {e}")))?;
        Ok(SessionStatusResponse {
            password,
            authenticated: true,
            user: Some(AuthUser {
                id: current.user_id,
                email: current.email,
            }),
            expires_at: Some(current.access_expires_at),
            is_admin: false,
            can_audit_client_market: false,
        })
    }
}
