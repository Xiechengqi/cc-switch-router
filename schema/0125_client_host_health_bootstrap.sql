-- Monitor installation is independent of the tenant binary and rental lifecycle.
ALTER TABLE client_host_agents ADD COLUMN deployed_commit TEXT;
ALTER TABLE client_host_agents ADD COLUMN deployed_at INTEGER;

CREATE TABLE client_host_agent_bootstraps (
    host_id TEXT PRIMARY KEY REFERENCES router_ssh_hosts(id) ON DELETE CASCADE,
    installation_id TEXT NOT NULL,
    machine_epoch INTEGER NOT NULL,
    key_generation INTEGER NOT NULL,
    operation_sequence INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL CHECK(status IN ('pending','running','succeeded','failed','cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0,
    requested_by TEXT NOT NULL,
    owner_user_id TEXT,
    requested_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_error TEXT
);
CREATE INDEX idx_client_host_bootstraps_queue ON client_host_agent_bootstraps(status,updated_at);

CREATE TRIGGER cancel_host_bootstrap_on_owner_change
AFTER UPDATE OF provider_id ON router_ssh_hosts
WHEN NEW.provider_id IS NOT OLD.provider_id
BEGIN
    UPDATE client_host_agents SET operation_sequence=operation_sequence+1 WHERE host_id=NEW.id AND EXISTS (
        SELECT 1 FROM client_host_agent_bootstraps b WHERE b.host_id=NEW.id AND b.status IN ('pending','running')
        AND b.owner_user_id IS NOT NULL AND b.owner_user_id IS NOT NEW.provider_id
    );
    UPDATE client_host_agent_bootstraps SET status='cancelled',last_error='host_owner_changed',updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE host_id=NEW.id AND status IN ('pending','running') AND owner_user_id IS NOT NULL AND owner_user_id IS NOT NEW.provider_id;
END;

-- An old lease must never retain execution authority after its context changes.
CREATE TRIGGER cancel_host_bootstrap_on_binding_change
AFTER UPDATE OF status,installation_id ON router_ssh_hosts
WHEN NEW.status<>'allocated' OR NEW.installation_id IS NOT OLD.installation_id
BEGIN
    UPDATE client_host_agents SET operation_sequence=operation_sequence+1
    WHERE host_id=NEW.id AND EXISTS (
        SELECT 1 FROM client_host_agent_bootstraps b WHERE b.host_id=NEW.id AND b.status IN ('pending','running')
    );
    UPDATE client_host_agent_bootstraps
    SET status='cancelled',last_error='rental_context_changed',updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE host_id=NEW.id AND status IN ('pending','running');
END;

CREATE TRIGGER cancel_host_bootstrap_on_rental_end
AFTER UPDATE OF status ON client_market_subscriptions
WHEN NEW.status NOT IN ('active','billing_suspended')
BEGIN
    UPDATE client_host_agents SET operation_sequence=operation_sequence+1
    WHERE host_id=NEW.host_id AND EXISTS (
        SELECT 1 FROM client_host_agent_bootstraps b
        WHERE b.host_id=NEW.host_id AND b.installation_id=NEW.installation_id AND b.status IN ('pending','running')
    );
    UPDATE client_host_agent_bootstraps
    SET status='cancelled',last_error='rental_context_changed',updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE host_id=NEW.host_id AND installation_id=NEW.installation_id AND status IN ('pending','running');
END;

CREATE TRIGGER cancel_host_bootstrap_on_identity_change
AFTER UPDATE OF machine_epoch,key_generation,revoked ON client_host_agents
WHEN NEW.revoked<>0 OR NEW.machine_epoch<>OLD.machine_epoch OR NEW.key_generation<>OLD.key_generation
BEGIN
    UPDATE client_host_agent_bootstraps
    SET status='cancelled',last_error='agent_identity_changed',updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE host_id=NEW.host_id AND status IN ('pending','running');
END;
