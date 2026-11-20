CREATE TABLE client_binary_preparations (
    host_id TEXT PRIMARY KEY REFERENCES router_ssh_hosts(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('pending', 'running', 'ready', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    selector TEXT,
    commit_id TEXT,
    last_error TEXT,
    updated_at TEXT NOT NULL
);

INSERT INTO client_binary_preparations(host_id,status,updated_at)
SELECT id,'pending',strftime('%Y-%m-%dT%H:%M:%fZ','now') FROM router_ssh_hosts
WHERE status='idle' AND installation_id IS NULL;

CREATE TRIGGER queue_client_binary_after_host_insert
AFTER INSERT ON router_ssh_hosts
WHEN NEW.status='idle' AND NEW.installation_id IS NULL
BEGIN
    INSERT INTO client_binary_preparations(host_id,status,updated_at)
    VALUES(NEW.id,'pending',NEW.updated_at);
END;
