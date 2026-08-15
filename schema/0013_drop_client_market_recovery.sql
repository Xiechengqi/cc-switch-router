DROP INDEX IF EXISTS idx_client_market_recovery_due;
DROP TABLE IF EXISTS client_market_recovery_state;

ALTER TABLE client_market_cleanup_recovery_state RENAME TO client_market_cleanup_retry_state;
DROP INDEX IF EXISTS idx_client_market_cleanup_recovery_due;
CREATE INDEX idx_client_market_cleanup_retry_due
    ON client_market_cleanup_retry_state(next_attempt_at);

-- The SSH recover worker is gone, so its historical jobs are no longer
-- interpretable by any code path and would violate the CHECK added below.
DELETE FROM provisioning_jobs WHERE type NOT IN ('create', 'cleanup');

CREATE TABLE provisioning_jobs_new (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL CHECK (type IN ('create', 'cleanup')),
    host_id TEXT,
    host_owner_email TEXT,
    client_owner_email TEXT,
    selection_owners_json TEXT,
    selection_regions_json TEXT,
    subdomain TEXT,
    installation_id TEXT,
    status TEXT NOT NULL,
    phase TEXT NOT NULL DEFAULT 'pending',
    log_blob TEXT NOT NULL DEFAULT '',
    secret_ref TEXT,
    failure_code TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    batch_id TEXT,
    quote_id TEXT,
    client_owner_user_id TEXT,
    cleanup_reason TEXT,
    started_at TEXT,
    heartbeat_at TEXT,
    deadline_at TEXT,
    worker_id TEXT
);

INSERT INTO provisioning_jobs_new (
    id, type, host_id, host_owner_email, client_owner_email,
    selection_owners_json, selection_regions_json, subdomain, installation_id,
    status, phase, log_blob, secret_ref, failure_code, created_at, updated_at,
    batch_id, quote_id, client_owner_user_id, cleanup_reason,
    started_at, heartbeat_at, deadline_at, worker_id
)
SELECT
    id, type, host_id, host_owner_email, client_owner_email,
    selection_owners_json, selection_regions_json, subdomain, installation_id,
    status, phase, log_blob, secret_ref, failure_code, created_at, updated_at,
    batch_id, quote_id, client_owner_user_id, cleanup_reason,
    started_at, heartbeat_at, deadline_at, worker_id
FROM provisioning_jobs;

DROP TABLE provisioning_jobs;
ALTER TABLE provisioning_jobs_new RENAME TO provisioning_jobs;

CREATE INDEX idx_provisioning_jobs_client
    ON provisioning_jobs(client_owner_email, status, updated_at DESC);
CREATE INDEX idx_provisioning_jobs_host
    ON provisioning_jobs(host_id, status);
CREATE UNIQUE INDEX idx_provisioning_jobs_active_host
    ON provisioning_jobs(host_id)
    WHERE host_id IS NOT NULL AND status IN ('pending', 'running');
CREATE INDEX idx_provisioning_jobs_batch_status
    ON provisioning_jobs(batch_id, status, created_at);
CREATE INDEX idx_provisioning_jobs_running_lease
    ON provisioning_jobs(status, deadline_at, heartbeat_at)
    WHERE status = 'running';
