CREATE TABLE share_client_bans (
    id TEXT PRIMARY KEY,
    share_id TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    failure_count INTEGER NOT NULL CHECK (failure_count > 0),
    first_failure_at TEXT NOT NULL,
    last_failure_at TEXT NOT NULL,
    banned_at TEXT NOT NULL,
    banned_until TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'expired', 'unbanned')),
    released_at TEXT,
    released_by_email TEXT,
    released_from_ip TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (share_id) REFERENCES shares(share_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX idx_share_client_bans_active_identity
    ON share_client_bans(share_id, client_ip)
    WHERE status = 'active';

CREATE INDEX idx_share_client_bans_share_status_until
    ON share_client_bans(share_id, status, banned_until DESC, id DESC);

CREATE INDEX idx_share_client_bans_history_cleanup
    ON share_client_bans(status, updated_at);
