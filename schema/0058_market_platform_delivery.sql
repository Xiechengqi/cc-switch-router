-- Persist the control-plane observation used by each wallet-funded service
-- interval. These rows are evidence, not an alternative source of funds.
ALTER TABLE market_platform_contracts
    ADD COLUMN renewal_anchor_day INTEGER NOT NULL DEFAULT 0
    CHECK (renewal_anchor_day BETWEEN 0 AND 31);

CREATE TABLE market_platform_service_evidence (
    id TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL REFERENCES market_platform_contracts(id),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('share_control', 'client_host_health')),
    started_at TEXT NOT NULL,
    ended_at TEXT NOT NULL,
    observed_state TEXT NOT NULL CHECK (observed_state IN ('healthy', 'unhealthy', 'unknown')),
    service_available INTEGER NOT NULL CHECK (service_available IN (0, 1)),
    reason_code TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (contract_id, started_at, ended_at),
    CHECK (started_at < ended_at),
    CHECK (service_available = 0 OR observed_state = 'healthy')
);
CREATE INDEX idx_market_platform_service_evidence_contract
    ON market_platform_service_evidence(contract_id, started_at);
