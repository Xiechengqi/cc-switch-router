CREATE TABLE client_market_provider_terminal_authorizations (
    installation_id TEXT PRIMARY KEY,
    host_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    client_user_id TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (installation_id) REFERENCES client_market_subscriptions(installation_id) ON DELETE CASCADE,
    FOREIGN KEY (host_id) REFERENCES router_ssh_hosts(id) ON DELETE CASCADE
);

CREATE INDEX idx_client_market_terminal_authorizations_provider
    ON client_market_provider_terminal_authorizations(provider_id, expires_at)
    WHERE revoked_at IS NULL;

CREATE INDEX idx_client_market_terminal_authorizations_client
    ON client_market_provider_terminal_authorizations(client_user_id, updated_at DESC);
