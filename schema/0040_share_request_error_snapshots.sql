-- Last-N committed non-2xx snapshots for Share edit/view. Bodies live here, not
-- in share_request_logs. Application code keeps three rows per Share.

CREATE TABLE share_request_error_snapshots (
    id TEXT PRIMARY KEY,
    share_id TEXT NOT NULL,
    request_id TEXT,
    captured_at TEXT NOT NULL,
    status_code INTEGER NOT NULL,
    method TEXT,
    path TEXT,
    content_type TEXT,
    caller_email TEXT,
    body_text TEXT NOT NULL,
    body_truncated INTEGER NOT NULL DEFAULT 0 CHECK (body_truncated IN (0, 1)),
    body_capture_reason TEXT NOT NULL CHECK (
        body_capture_reason IN (
            'buffered',
            'sse_not_buffered',
            'empty_body',
            'read_failed',
            'router_local'
        )
    ),
    FOREIGN KEY (share_id) REFERENCES shares(share_id) ON DELETE CASCADE
);

CREATE INDEX idx_share_request_error_snapshots_share_captured
    ON share_request_error_snapshots(share_id, captured_at DESC, id DESC);
