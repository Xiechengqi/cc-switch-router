-- Transactional outbox for buyer/Provider wallet notifications.
--
-- Rows are written in the same transaction as the money event that caused
-- them, so a committed credit, suspension or payout always has its notice.
-- Payloads hold only amounts, states and object references: never secrets,
-- full addresses or checkout tokens. Owner alerts use the existing
-- operator_alert_signal_outbox instead.
CREATE TABLE market_notification_outbox (
    id TEXT PRIMARY KEY,
    audience TEXT NOT NULL CHECK (audience IN ('buyer', 'provider')),
    recipient_user_id TEXT NOT NULL,
    event_kind TEXT NOT NULL,
    object_kind TEXT NOT NULL,
    object_id TEXT NOT NULL,
    dedupe_key TEXT NOT NULL UNIQUE,
    payload_json TEXT NOT NULL DEFAULT '{}',
    mandatory INTEGER NOT NULL DEFAULT 0 CHECK (mandatory IN (0, 1)),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'claimed', 'sent', 'suppressed', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TEXT NOT NULL,
    claimed_by TEXT,
    claim_expires_at TEXT,
    delivered_channels TEXT,
    last_error TEXT,
    read_at TEXT,
    created_at TEXT NOT NULL,
    sent_at TEXT
);
CREATE INDEX idx_market_notification_outbox_due
    ON market_notification_outbox(status, next_attempt_at);
CREATE INDEX idx_market_notification_outbox_recipient
    ON market_notification_outbox(recipient_user_id, created_at DESC, id DESC);

-- Per-user opt-out for optional kinds. Mandatory (security and contract
-- state) notices ignore this table but still respect channel availability.
CREATE TABLE market_notification_preferences (
    user_id TEXT NOT NULL,
    event_kind TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, event_kind)
);
