-- Payment channel rollout and channel-level settings.
--
-- rollout_state: 'off' (no new orders), 'canary' (only the Router owner may
-- create orders), 'live' (all eligible buyers). A canary revision is promoted
-- to live automatically by its first strictly verified credit. `enabled` is
-- kept as a compatibility projection: enabled = 0 <=> rollout_state = 'off'.
ALTER TABLE market_payment_channels ADD COLUMN rollout_state TEXT NOT NULL DEFAULT 'off'
    CHECK (rollout_state IN ('off', 'canary', 'live'));
ALTER TABLE market_payment_channels ADD COLUMN promoted_at TEXT;
UPDATE market_payment_channels
SET rollout_state = CASE WHEN enabled = 1 THEN 'live' ELSE 'off' END;

CREATE TRIGGER trg_market_payment_channel_disable_rollout
AFTER UPDATE OF enabled ON market_payment_channels
WHEN NEW.enabled = 0 AND NEW.rollout_state != 'off'
BEGIN
    UPDATE market_payment_channels SET rollout_state = 'off' WHERE id = NEW.id;
END;

-- A direct legacy write of enabled = 1 means a fully enabled channel.
CREATE TRIGGER trg_market_payment_channel_enable_legacy
AFTER UPDATE OF enabled ON market_payment_channels
WHEN NEW.enabled = 1 AND NEW.rollout_state = 'off'
BEGIN
    UPDATE market_payment_channels SET rollout_state = 'live' WHERE id = NEW.id;
END;

CREATE TRIGGER trg_market_payment_channel_insert_rollout
AFTER INSERT ON market_payment_channels
WHEN NEW.enabled = 1 AND NEW.rollout_state = 'off'
BEGIN
    UPDATE market_payment_channels SET rollout_state = 'live' WHERE id = NEW.id;
END;

CREATE TRIGGER trg_market_payment_channel_rollout_projection
AFTER UPDATE OF rollout_state ON market_payment_channels
WHEN (NEW.rollout_state = 'off') != (NEW.enabled = 0)
BEGIN
    UPDATE market_payment_channels
    SET enabled = CASE WHEN NEW.rollout_state = 'off' THEN 0 ELSE 1 END
    WHERE id = NEW.id;
END;

-- Settings that do not change the merchant identity or its secrets. Editing
-- them never creates a credential revision and therefore never re-enters
-- canary. Each order freezes the values it used in its create request body.
CREATE TABLE market_payment_channel_settings (
    channel_id TEXT PRIMARY KEY REFERENCES market_payment_channels(id),
    order_mode TEXT NOT NULL DEFAULT 'cashier' CHECK (order_mode IN ('cashier', 'direct')),
    allowed_methods_json TEXT NOT NULL DEFAULT '[]',
    min_topup_minor INTEGER CHECK (min_topup_minor IS NULL OR min_topup_minor > 0),
    max_topup_minor INTEGER CHECK (max_topup_minor IS NULL OR max_topup_minor > 0),
    fee_rate_bps INTEGER NOT NULL DEFAULT 0 CHECK (fee_rate_bps BETWEEN 0 AND 10000),
    fixed_fee_minor INTEGER NOT NULL DEFAULT 0 CHECK (fixed_fee_minor BETWEEN 0 AND 1000000),
    version INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    CHECK (max_topup_minor IS NULL OR min_topup_minor IS NULL OR max_topup_minor >= min_topup_minor)
);
INSERT INTO market_payment_channel_settings (channel_id, allowed_methods_json, updated_at)
SELECT channel.id, COALESCE(revision.allowed_methods_json, '[]'), channel.updated_at
FROM market_payment_channels channel
LEFT JOIN market_payment_channel_revisions revision
  ON revision.channel_id = channel.id AND revision.revision = channel.current_revision;

CREATE TRIGGER trg_market_payment_channel_default_settings
AFTER INSERT ON market_payment_channels
BEGIN
    INSERT OR IGNORE INTO market_payment_channel_settings (channel_id, updated_at)
    VALUES (NEW.id, NEW.created_at);
END;

-- Authenticated gateway sweeps (daily recent window, weekly full history).
CREATE TABLE market_gateway_sweeps (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL REFERENCES market_payment_channels(id),
    revision INTEGER NOT NULL,
    window_kind TEXT NOT NULL CHECK (window_kind IN ('daily', 'weekly_full')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    next_offset INTEGER NOT NULL DEFAULT 0 CHECK (next_offset >= 0),
    scanned INTEGER NOT NULL DEFAULT 0,
    queued INTEGER NOT NULL DEFAULT 0,
    orphans INTEGER NOT NULL DEFAULT 0,
    error_code TEXT
);
CREATE INDEX idx_market_gateway_sweeps_channel
    ON market_gateway_sweeps(channel_id, window_kind, started_at DESC);
