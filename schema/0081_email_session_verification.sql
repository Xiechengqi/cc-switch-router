-- A real email-login session is already proof of email ownership. Migration
-- 75 added the marker without restoring it for existing authenticated users.
-- Never infer verification from an email, API token or last_login_at alone.
UPDATE users
SET email_verified_at = (
    SELECT MIN(s.created_at) FROM user_sessions s
    WHERE s.user_id = users.id AND s.revoked_at IS NULL
      AND s.auth_source_kind IN ('auth_device', 'client_installation')
      AND julianday(s.refresh_expires_at) > julianday('now')
)
WHERE status = 'active' AND email_verified_at IS NULL
  AND EXISTS (
    SELECT 1 FROM user_sessions s
    WHERE s.user_id = users.id AND s.revoked_at IS NULL
      AND s.auth_source_kind IN ('auth_device', 'client_installation')
      AND julianday(s.refresh_expires_at) > julianday('now')
  );
