CREATE TABLE client_host_agents (
    host_id TEXT PRIMARY KEY REFERENCES router_ssh_hosts(id) ON DELETE CASCADE,
    machine_epoch INTEGER NOT NULL DEFAULT 1,
    key_generation INTEGER NOT NULL DEFAULT 1,
    public_key TEXT,
    first_due_at INTEGER,
    admitted_hour INTEGER,
    supervision TEXT NOT NULL DEFAULT 'unknown',
    clock_synchronized INTEGER NOT NULL DEFAULT 1,
    revoked INTEGER NOT NULL DEFAULT 0,
    operation_sequence INTEGER NOT NULL DEFAULT 0,
    maintenance_pending INTEGER NOT NULL DEFAULT 0,
    replacement_pending INTEGER NOT NULL DEFAULT 0,
    next_deadline INTEGER NOT NULL DEFAULT 0,
    summary_json TEXT,
    last_error TEXT,
    latest_hour INTEGER,
    latest_sample_json TEXT,
    latest_received_at INTEGER,
    latest_public INTEGER NOT NULL DEFAULT 0,
    latest_lease_id TEXT,
    legacy INTEGER NOT NULL DEFAULT 0,
    readmission_pending INTEGER NOT NULL DEFAULT 0,
    resume_status TEXT CHECK(resume_status IS NULL OR resume_status IN ('disabled','abnormal'))
);
CREATE INDEX idx_client_host_agents_deadline ON client_host_agents(next_deadline);
-- Hosts listed before this migration keep their current availability. They are
-- queued for reporter installation and lose the legacy exemption at first report.
INSERT INTO client_host_agents(host_id,legacy,maintenance_pending)
SELECT id,1,1 FROM router_ssh_hosts;
INSERT INTO client_binary_preparations(host_id,status,updated_at)
SELECT id,'pending',strftime('%Y-%m-%dT%H:%M:%fZ','now') FROM router_ssh_hosts WHERE true
ON CONFLICT(host_id) DO UPDATE SET status='pending',attempts=0,last_error=NULL,updated_at=excluded.updated_at;
CREATE TABLE client_host_hour_reports (
    host_id TEXT NOT NULL REFERENCES router_ssh_hosts(id) ON DELETE CASCADE,
    utc_hour INTEGER NOT NULL,
    machine_epoch INTEGER NOT NULL,
    key_generation INTEGER NOT NULL,
    body_digest TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    sampled_at INTEGER NOT NULL,
    report_json TEXT NOT NULL,
    public_sample INTEGER NOT NULL DEFAULT 0,
    lease_id TEXT,
    PRIMARY KEY(host_id, utc_hour)
);
CREATE INDEX idx_client_host_reports_retention ON client_host_hour_reports(received_at);
CREATE TABLE client_host_revoked_keys (
    host_id TEXT NOT NULL,
    machine_epoch INTEGER NOT NULL,
    key_generation INTEGER NOT NULL,
    public_key TEXT NOT NULL,
    revoked_at INTEGER NOT NULL,
    PRIMARY KEY(host_id, machine_epoch, key_generation)
);
-- Deleted Host IDs by address. When the same machine is added again, its next managed
-- operation may take over a remote control directory that still names the old ID
-- because the best-effort uninstall at deletion did not reach the Host.
CREATE TABLE client_host_retired_ids (
    host_id TEXT PRIMARY KEY,
    ip TEXT NOT NULL,
    port INTEGER NOT NULL,
    retired_at INTEGER NOT NULL
);
CREATE INDEX idx_client_host_retired_ids_address ON client_host_retired_ids(ip,port);
CREATE TABLE client_host_report_conflicts (
    host_id TEXT NOT NULL REFERENCES router_ssh_hosts(id) ON DELETE CASCADE,
    utc_hour INTEGER NOT NULL,
    machine_epoch INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    PRIMARY KEY(host_id, utc_hour)
);
CREATE TABLE client_host_receiver (
    id INTEGER PRIMARY KEY CHECK(id=1),
    instance_id TEXT NOT NULL,
    last_seen_at INTEGER NOT NULL,
    stopped_at INTEGER
);
CREATE TABLE client_host_receiver_outages (
    started_at INTEGER NOT NULL,
    ended_at INTEGER NOT NULL,
    PRIMARY KEY(started_at, ended_at)
);
CREATE TABLE client_host_tenancies (
    installation_id TEXT PRIMARY KEY,
    host_id TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER
);
CREATE INDEX idx_client_host_tenancies_history ON client_host_tenancies(host_id,started_at,ended_at);
INSERT OR IGNORE INTO client_host_tenancies(installation_id,host_id,started_at,ended_at)
SELECT installation_id,host_id,COALESCE(unixepoch(created_at),0),unixepoch(released_at)
FROM client_market_subscriptions WHERE installation_id IS NOT NULL AND host_id IS NOT NULL;
CREATE TRIGGER client_host_tenancy_insert
AFTER INSERT ON client_market_subscriptions
BEGIN
    INSERT OR IGNORE INTO client_host_tenancies(installation_id,host_id,started_at,ended_at)
    VALUES(NEW.installation_id,NEW.host_id,COALESCE(unixepoch(NEW.created_at),0),unixepoch(NEW.released_at));
END;
CREATE TRIGGER client_host_tenancy_release
AFTER UPDATE OF released_at ON client_market_subscriptions
WHEN NEW.released_at IS NOT NULL
BEGIN
    UPDATE client_host_tenancies SET ended_at=unixepoch(NEW.released_at)
    WHERE installation_id=NEW.installation_id;
END;
CREATE TRIGGER client_host_tenancy_delete
BEFORE DELETE ON client_market_subscriptions
BEGIN
    UPDATE client_host_tenancies SET ended_at=COALESCE(ended_at,unixepoch())
    WHERE installation_id=OLD.installation_id;
END;
ALTER TABLE client_market_allocation_quote_items ADD COLUMN host_health_json TEXT;
ALTER TABLE client_market_allocation_quotes ADD COLUMN projection_json TEXT;

-- `released_at` is written before cleanup is confirmed; the tenant may still run on
-- the Host. Keep the last tenancy open until the Host returns to the pool so samples
-- taken in between are never classified as idle and published.
CREATE TRIGGER client_host_tenancy_pool_return
AFTER UPDATE OF status ON router_ssh_hosts
WHEN NEW.status IN ('idle','maintaining','onboarding')
  AND OLD.status IN ('allocated','locked','draining','unreachable','abnormal')
BEGIN
    UPDATE client_host_tenancies SET ended_at=MAX(COALESCE(ended_at,0),unixepoch())
    WHERE installation_id=(SELECT installation_id FROM client_host_tenancies
                           WHERE host_id=NEW.id ORDER BY started_at DESC LIMIT 1);
END;

DROP TRIGGER queue_client_binary_after_host_insert;
CREATE TRIGGER queue_client_binary_after_host_insert
AFTER INSERT ON router_ssh_hosts
WHEN NEW.status='onboarding' AND NEW.installation_id IS NULL
BEGIN
    INSERT INTO client_binary_preparations(host_id,status,updated_at) VALUES(NEW.id,'pending',NEW.updated_at);
    INSERT INTO client_host_agents(host_id) VALUES(NEW.id);
END;
CREATE TRIGGER client_host_idle_admission_insert
BEFORE INSERT ON router_ssh_hosts WHEN NEW.status='idle'
BEGIN
    SELECT RAISE(ABORT, 'Host requires onboarding and an admitted health report');
END;
CREATE TRIGGER client_host_idle_admission_update
BEFORE UPDATE OF status ON router_ssh_hosts
WHEN NEW.status='idle' AND NOT EXISTS (
    SELECT 1 FROM client_host_agents a WHERE a.host_id=NEW.id
      AND (a.admitted_hour IS NOT NULL OR a.legacy=1)
      AND a.revoked=0 AND a.readmission_pending=0
)
BEGIN
    SELECT RAISE(ABORT, 'Host requires an admitted current-generation health report');
END;
CREATE TRIGGER client_host_revoke_before_delete
BEFORE DELETE ON router_ssh_hosts
BEGIN
    INSERT OR IGNORE INTO client_host_revoked_keys(host_id,machine_epoch,key_generation,public_key,revoked_at)
    SELECT host_id,machine_epoch,key_generation,public_key,unixepoch() FROM client_host_agents
    WHERE host_id=OLD.id AND public_key IS NOT NULL;
    INSERT OR REPLACE INTO client_host_retired_ids(host_id,ip,port,retired_at)
    VALUES(OLD.id,OLD.ip,OLD.port,unixepoch());
END;

