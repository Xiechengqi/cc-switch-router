CREATE TABLE share_model_health_slots (
    share_id TEXT NOT NULL REFERENCES shares(share_id) ON DELETE CASCADE,
    slot_start INTEGER NOT NULL CHECK (slot_start >= 0 AND slot_start % 1800 = 0),
    claim_token TEXT NOT NULL CHECK (claim_token != ''),
    claimed_at INTEGER NOT NULL CHECK (claimed_at >= 0),
    app_type TEXT NOT NULL CHECK (app_type IN ('claude', 'codex', 'gemini')),
    api_type TEXT NOT NULL CHECK (api_type IN ('anthropic', 'openai', 'gemini')),
    requested_model TEXT NOT NULL CHECK (requested_model != ''),
    actual_model TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'success', 'degraded', 'quota_blocked', 'failed', 'timeout', 'offline')),
    status_code INTEGER CHECK (status_code IS NULL OR status_code BETWEEN 100 AND 599),
    latency_ms INTEGER NOT NULL DEFAULT 0 CHECK (latency_ms >= 0),
    provider_id TEXT,
    provider_name TEXT,
    policy_mode TEXT,
    health_fingerprint TEXT,
    error_category TEXT,
    error_message TEXT,
    checked_at INTEGER CHECK (checked_at IS NULL OR checked_at >= 0),
    source TEXT NOT NULL CHECK (source != ''),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    PRIMARY KEY (share_id, slot_start),
    CHECK (
        (app_type = 'claude' AND api_type = 'anthropic') OR
        (app_type = 'codex' AND api_type = 'openai') OR
        (app_type = 'gemini' AND api_type = 'gemini')
    ),
    CHECK (
        (status = 'pending' AND checked_at IS NULL) OR
        (status != 'pending' AND checked_at IS NOT NULL)
    )
);

CREATE INDEX idx_share_model_health_slots_calendar
    ON share_model_health_slots(share_id, slot_start, status);

CREATE INDEX idx_share_model_health_slots_retention
    ON share_model_health_slots(slot_start);
