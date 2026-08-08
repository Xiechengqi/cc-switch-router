ALTER TABLE provisioning_jobs ADD COLUMN started_at TEXT;
ALTER TABLE provisioning_jobs ADD COLUMN heartbeat_at TEXT;
ALTER TABLE provisioning_jobs ADD COLUMN deadline_at TEXT;
ALTER TABLE provisioning_jobs ADD COLUMN worker_id TEXT;

CREATE INDEX idx_provisioning_jobs_running_lease
    ON provisioning_jobs(status, deadline_at, heartbeat_at)
    WHERE status = 'running';

CREATE TABLE client_market_host_reprobe_state (
    host_id TEXT PRIMARY KEY,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT,
    lease_until TEXT,
    last_outcome TEXT,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_client_market_host_reprobe_due
    ON client_market_host_reprobe_state(next_attempt_at, lease_until);
