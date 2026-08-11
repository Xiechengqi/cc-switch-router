CREATE TABLE share_request_log_recovery_cursors (
    share_id TEXT PRIMARY KEY REFERENCES shares(share_id) ON DELETE CASCADE,
    installation_id TEXT NOT NULL,
    export_sequence INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_share_request_log_recovery_installation
    ON share_request_log_recovery_cursors(installation_id, export_sequence);
