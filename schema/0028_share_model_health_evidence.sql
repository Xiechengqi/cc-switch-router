ALTER TABLE share_model_health_slots
    ADD COLUMN observation_id TEXT;

ALTER TABLE share_model_health_slots
    ADD COLUMN probe_epoch_id TEXT;

ALTER TABLE share_model_health_slots
    ADD COLUMN outcome TEXT NOT NULL DEFAULT 'pending'
        CHECK (outcome IN ('pending', 'success', 'failure', 'unobserved'));

ALTER TABLE share_model_health_slots
    ADD COLUMN failure_domain TEXT
        CHECK (failure_domain IS NULL OR failure_domain IN (
            'upstream', 'quota', 'provider_config', 'control_transport',
            'router_monitor', 'unknown'
        ));

ALTER TABLE share_model_health_slots
    ADD COLUMN reason_code TEXT;

ALTER TABLE share_model_health_slots
    ADD COLUMN evidence_scope TEXT NOT NULL DEFAULT 'share_legacy'
        CHECK (evidence_scope IN ('share_legacy', 'share_projection', 'provider_runtime'));

ALTER TABLE share_model_health_slots
    ADD COLUMN evidence_version INTEGER NOT NULL DEFAULT 1
        CHECK (evidence_version IN (1, 2));

UPDATE share_model_health_slots
SET outcome = CASE
        WHEN status = 'pending' THEN 'pending'
        WHEN status IN ('success', 'degraded') THEN 'success'
        ELSE 'failure'
    END,
    failure_domain = CASE
        WHEN status IN ('success', 'degraded', 'pending') THEN NULL
        ELSE 'unknown'
    END,
    reason_code = CASE
        WHEN status IN ('success', 'degraded') THEN 'legacy_probe_succeeded'
        WHEN status = 'pending' THEN NULL
        ELSE 'legacy_probe_failed'
    END;

CREATE TABLE share_model_probe_observations (
    observation_id TEXT PRIMARY KEY CHECK (observation_id != ''),
    installation_id TEXT NOT NULL CHECK (installation_id != ''),
    cycle_id TEXT NOT NULL CHECK (cycle_id != ''),
    slot_start INTEGER NOT NULL CHECK (slot_start >= 0 AND slot_start % 1800 = 0),
    capacity_pool_id TEXT NOT NULL CHECK (capacity_pool_id != ''),
    app_type TEXT NOT NULL CHECK (app_type IN ('claude', 'codex', 'gemini')),
    api_type TEXT NOT NULL CHECK (api_type IN ('anthropic', 'openai', 'gemini')),
    provider_id TEXT NOT NULL CHECK (provider_id != ''),
    provider_name TEXT,
    health_fingerprint TEXT NOT NULL CHECK (health_fingerprint != ''),
    requested_model TEXT NOT NULL CHECK (requested_model != ''),
    actual_model TEXT NOT NULL CHECK (actual_model != ''),
    status TEXT NOT NULL CHECK (status IN ('success', 'degraded', 'quota_blocked', 'failed')),
    outcome TEXT NOT NULL CHECK (outcome IN ('success', 'failure')),
    failure_domain TEXT CHECK (failure_domain IS NULL OR failure_domain IN (
        'upstream', 'quota', 'provider_config', 'unknown'
    )),
    reason_code TEXT,
    status_code INTEGER CHECK (status_code IS NULL OR status_code BETWEEN 100 AND 599),
    latency_ms INTEGER NOT NULL CHECK (latency_ms >= 0),
    checked_at INTEGER NOT NULL CHECK (checked_at >= 0),
    evidence_scope TEXT NOT NULL DEFAULT 'provider_runtime'
        CHECK (evidence_scope = 'provider_runtime'),
    evidence_version INTEGER NOT NULL DEFAULT 2 CHECK (evidence_version = 2),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    CHECK (
        (app_type = 'claude' AND api_type = 'anthropic') OR
        (app_type = 'codex' AND api_type = 'openai') OR
        (app_type = 'gemini' AND api_type = 'gemini')
    ),
    CHECK (
        (outcome = 'success' AND failure_domain IS NULL) OR
        (outcome = 'failure' AND failure_domain IS NOT NULL)
    )
);

CREATE INDEX idx_share_model_probe_observations_cycle
    ON share_model_probe_observations(installation_id, cycle_id, app_type, provider_id);

CREATE INDEX idx_share_model_probe_observations_retention
    ON share_model_probe_observations(slot_start);

CREATE TABLE share_model_probe_epochs (
    epoch_id TEXT PRIMARY KEY CHECK (epoch_id != ''),
    share_id TEXT NOT NULL REFERENCES shares(share_id) ON DELETE CASCADE,
    starts_at_slot INTEGER NOT NULL CHECK (starts_at_slot >= 0 AND starts_at_slot % 1800 = 0),
    ends_at_slot INTEGER CHECK (
        ends_at_slot IS NULL OR
        (ends_at_slot > starts_at_slot AND ends_at_slot % 1800 = 0)
    ),
    app_type TEXT NOT NULL CHECK (app_type IN ('claude', 'codex', 'gemini')),
    api_type TEXT NOT NULL CHECK (api_type IN ('anthropic', 'openai', 'gemini')),
    provider_id TEXT NOT NULL,
    provider_name TEXT,
    capacity_pool_id TEXT NOT NULL CHECK (capacity_pool_id != ''),
    requested_model TEXT NOT NULL CHECK (requested_model != ''),
    wire_model TEXT NOT NULL CHECK (wire_model != ''),
    policy_mode TEXT CHECK (policy_mode IS NULL OR policy_mode IN ('passthrough', 'single')),
    health_fingerprint TEXT NOT NULL,
    evidence_version INTEGER NOT NULL DEFAULT 2 CHECK (evidence_version IN (1, 2)),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    UNIQUE (share_id, starts_at_slot),
    CHECK (
        (app_type = 'claude' AND api_type = 'anthropic') OR
        (app_type = 'codex' AND api_type = 'openai') OR
        (app_type = 'gemini' AND api_type = 'gemini')
    )
);

CREATE UNIQUE INDEX idx_share_model_probe_epochs_open
    ON share_model_probe_epochs(share_id)
    WHERE ends_at_slot IS NULL;

CREATE INDEX idx_share_model_probe_epochs_calendar
    ON share_model_probe_epochs(share_id, starts_at_slot, ends_at_slot);

INSERT INTO share_model_probe_epochs (
    epoch_id, share_id, starts_at_slot, ends_at_slot, app_type, api_type,
    provider_id, provider_name, capacity_pool_id, requested_model, wire_model,
    policy_mode, health_fingerprint, evidence_version, created_at, updated_at
)
SELECT
    'legacy-v1:' || slots.share_id,
    slots.share_id,
    MIN(slots.slot_start),
    CASE
        WHEN shares.share_status = 'active' THEN NULL
        ELSE MAX(slots.slot_start) + 1800
    END,
    (
        SELECT first_slot.app_type
        FROM share_model_health_slots first_slot
        WHERE first_slot.share_id = slots.share_id
        ORDER BY first_slot.slot_start ASC
        LIMIT 1
    ),
    (
        SELECT first_slot.api_type
        FROM share_model_health_slots first_slot
        WHERE first_slot.share_id = slots.share_id
        ORDER BY first_slot.slot_start ASC
        LIMIT 1
    ),
    COALESCE((
        SELECT first_slot.provider_id
        FROM share_model_health_slots first_slot
        WHERE first_slot.share_id = slots.share_id
        ORDER BY first_slot.slot_start ASC
        LIMIT 1
    ), ''),
    (
        SELECT first_slot.provider_name
        FROM share_model_health_slots first_slot
        WHERE first_slot.share_id = slots.share_id
        ORDER BY first_slot.slot_start ASC
        LIMIT 1
    ),
    shares.capacity_pool_id,
    (
        SELECT first_slot.requested_model
        FROM share_model_health_slots first_slot
        WHERE first_slot.share_id = slots.share_id
        ORDER BY first_slot.slot_start ASC
        LIMIT 1
    ),
    COALESCE(NULLIF((
        SELECT first_slot.actual_model
        FROM share_model_health_slots first_slot
        WHERE first_slot.share_id = slots.share_id
        ORDER BY first_slot.slot_start ASC
        LIMIT 1
    ), ''), (
        SELECT first_slot.requested_model
        FROM share_model_health_slots first_slot
        WHERE first_slot.share_id = slots.share_id
        ORDER BY first_slot.slot_start ASC
        LIMIT 1
    )),
    (
        SELECT first_slot.policy_mode
        FROM share_model_health_slots first_slot
        WHERE first_slot.share_id = slots.share_id
        ORDER BY first_slot.slot_start ASC
        LIMIT 1
    ),
    COALESCE((
        SELECT first_slot.health_fingerprint
        FROM share_model_health_slots first_slot
        WHERE first_slot.share_id = slots.share_id
        ORDER BY first_slot.slot_start ASC
        LIMIT 1
    ), ''),
    1,
    strftime('%s', 'now'),
    strftime('%s', 'now')
FROM share_model_health_slots slots
JOIN shares ON shares.share_id = slots.share_id
GROUP BY slots.share_id;

UPDATE share_model_health_slots
SET probe_epoch_id = 'legacy-v1:' || share_id
WHERE probe_epoch_id IS NULL;

CREATE INDEX idx_share_model_health_slots_observation
    ON share_model_health_slots(observation_id)
    WHERE observation_id IS NOT NULL;

CREATE INDEX idx_share_model_health_slots_epoch
    ON share_model_health_slots(probe_epoch_id, slot_start)
    WHERE probe_epoch_id IS NOT NULL;
