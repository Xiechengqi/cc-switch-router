-- Retire credentials that may have crossed a user-controlled tunnel. This
-- migration runs once; refresh rotation grace must not restore an old session.
UPDATE users SET auth_version = auth_version + 1
 WHERE EXISTS (SELECT 1 FROM user_sessions s WHERE s.user_id = users.id);
UPDATE user_sessions SET revoked_at = COALESCE(revoked_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')),
 rotated_at = NULL;

-- Browser cookies are a separate credential class, never an access bearer.
CREATE TABLE router_browser_sessions (
 cookie_hash TEXT PRIMARY KEY,
 session_id TEXT NOT NULL REFERENCES user_sessions(id) ON DELETE CASCADE,
 expires_at TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE INDEX idx_router_browser_sessions_session ON router_browser_sessions(session_id);
CREATE INDEX idx_router_browser_sessions_expiry ON router_browser_sessions(expires_at);
