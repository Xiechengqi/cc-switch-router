ALTER TABLE share_control_operations ADD COLUMN next_attempt_at TEXT;
ALTER TABLE share_control_operations ADD COLUMN dead_lettered_at TEXT;

ALTER TABLE share_edit_requests ADD COLUMN expires_at TEXT;
ALTER TABLE share_edit_requests ADD COLUMN dead_lettered_at TEXT;
ALTER TABLE share_edit_requests ADD COLUMN error_code TEXT;

UPDATE share_control_operations
SET next_attempt_at = updated_at
WHERE next_attempt_at IS NULL AND status = 'pending';

CREATE INDEX idx_share_control_ready
    ON share_control_operations(status, dead_lettered_at, next_attempt_at, share_id, share_sequence);
CREATE INDEX idx_share_edit_expiry
    ON share_edit_requests(status, expires_at)
    WHERE status = 'pending' AND retired_at IS NULL;
