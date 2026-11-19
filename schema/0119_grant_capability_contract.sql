-- A v14 descriptor carries per-grant authority instead of the v13 Share policy.
CREATE TRIGGER IF NOT EXISTS prevent_share_grant_capability_downgrade
BEFORE UPDATE OF share_access_policy_version ON shares
WHEN OLD.share_access_policy_version >= 14
 AND NEW.share_access_policy_version < OLD.share_access_policy_version
BEGIN
    SELECT RAISE(ABORT, 'Share grant capability contract cannot be downgraded');
END;

ALTER TABLE share_edit_requests ADD COLUMN request_fingerprint TEXT;
