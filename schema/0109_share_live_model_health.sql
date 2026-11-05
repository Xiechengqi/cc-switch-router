-- Display-only manual evidence. Never backfill or modify monitoring slots,
-- provider reputation, scheduling decisions, or financial records.
ALTER TABLE shares ADD COLUMN model_health_degraded_threshold_ms INTEGER
    CHECK (model_health_degraded_threshold_ms BETWEEN 1000 AND 30000);

CREATE TABLE share_model_live_observations (
    share_id TEXT NOT NULL REFERENCES shares(share_id) ON DELETE CASCADE,
    source TEXT NOT NULL CHECK (source IN ('server_manual', 'router_connection_test')),
    observation_id TEXT NOT NULL,
    -- Empty monitoring epochs can be replaced within a slot. Keep replay
    -- tombstones until retention expires, but never adopt them into a new epoch.
    probe_epoch_id TEXT REFERENCES share_model_probe_epochs(epoch_id) ON DELETE SET NULL,
    installation_id TEXT NOT NULL,
    app_type TEXT NOT NULL CHECK (app_type IN ('claude', 'codex', 'gemini')),
    provider_id TEXT NOT NULL,
    health_fingerprint TEXT NOT NULL,
    requested_model TEXT NOT NULL,
    actual_model TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('success', 'degraded', 'failed')),
    status_code INTEGER,
    latency_ms INTEGER NOT NULL CHECK (latency_ms >= 0),
    checked_at INTEGER NOT NULL CHECK (checked_at > 0),
    PRIMARY KEY (share_id, source, observation_id)
);
CREATE INDEX idx_share_live_health_current
    ON share_model_live_observations(share_id, probe_epoch_id, checked_at DESC);
CREATE INDEX idx_share_live_health_retention ON share_model_live_observations(checked_at);
