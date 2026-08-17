-- Channel-neutral user notifications and Telegram binding/runtime state.
--
-- This migration has not shipped. The deployment target is a clean database,
-- so it intentionally replaces the email-specific outbox contract instead of
-- preserving an intermediate multi-channel schema.

ALTER TABLE email_delivery_batches ADD COLUMN channel TEXT NOT NULL DEFAULT 'email';
ALTER TABLE email_delivery_batches ADD COLUMN channel_target TEXT;
ALTER TABLE email_delivery_batches ADD COLUMN recipient_user_id TEXT;
ALTER TABLE email_delivery_batches ADD COLUMN target_revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE email_delivery_batches ADD COLUMN provider_identity TEXT;
ALTER TABLE email_delivery_batches ADD COLUMN payload_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE email_delivery_batches ADD COLUMN payload_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE email_delivery_batches ADD COLUMN failure_kind TEXT;
ALTER TABLE email_delivery_batches ADD COLUMN blocked_reason_code TEXT;
ALTER TABLE email_delivery_batches RENAME TO notification_deliveries;

DROP INDEX IF EXISTS idx_email_delivery_batches_claim;
DROP INDEX IF EXISTS idx_email_delivery_batches_send_cap;
DROP INDEX IF EXISTS idx_email_delivery_batches_claim_cap;
DROP INDEX IF EXISTS idx_email_delivery_batches_recipient_send_cap;
DROP INDEX IF EXISTS idx_email_delivery_batches_recipient_claim_cap;
DROP INDEX IF EXISTS idx_email_delivery_batches_recent;
DROP INDEX IF EXISTS idx_email_delivery_batches_retention;
DROP INDEX IF EXISTS idx_email_delivery_batches_priority_claim;
DROP INDEX IF EXISTS idx_email_delivery_batches_lane_claim;
DROP INDEX IF EXISTS idx_email_delivery_batches_lane_send_cap;
DROP INDEX IF EXISTS idx_email_delivery_batches_lane_claim_cap;
DROP INDEX IF EXISTS idx_email_delivery_batches_recipient_lane_send_cap;
DROP INDEX IF EXISTS idx_email_delivery_batches_recipient_lane_claim_cap;
DROP INDEX IF EXISTS idx_email_delivery_batches_incident;

CREATE INDEX idx_notification_deliveries_priority_claim
    ON notification_deliveries(
        notification_lane, recipient_priority, status, next_attempt_at, not_before,
        claim_expires_at
    );
CREATE INDEX idx_notification_deliveries_channel_claim
    ON notification_deliveries(
        channel, notification_lane, status, next_attempt_at, not_before,
        claim_expires_at, recipient_priority, created_at, id
    );
CREATE INDEX idx_notification_deliveries_recipient_channel
    ON notification_deliveries(
        LOWER(recipient), channel, notification_lane, status, created_at
    );
CREATE INDEX idx_notification_deliveries_recent
    ON notification_deliveries(created_at DESC, id DESC);
CREATE INDEX idx_notification_deliveries_retention
    ON notification_deliveries(updated_at, status);
CREATE INDEX idx_notification_deliveries_incident
    ON notification_deliveries(recipient, incident_key, created_at DESC);

ALTER TABLE email_delivery_batch_items RENAME TO notification_delivery_items_legacy;
CREATE TABLE notification_delivery_items (
    batch_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    recipient TEXT NOT NULL,
    channel TEXT NOT NULL,
    PRIMARY KEY (batch_id, event_id, recipient, channel),
    UNIQUE (event_id, recipient, channel),
    FOREIGN KEY (batch_id) REFERENCES notification_deliveries(id) ON DELETE CASCADE,
    FOREIGN KEY (event_id) REFERENCES client_notification_events(id) ON DELETE CASCADE
);
INSERT INTO notification_delivery_items (batch_id, event_id, recipient, channel)
    SELECT batch_id, event_id, recipient, 'email'
    FROM notification_delivery_items_legacy;
DROP TABLE notification_delivery_items_legacy;
CREATE INDEX idx_notification_delivery_items_event
    ON notification_delivery_items(event_id, recipient, channel);

-- Enabled channels are rows, not combinations such as email/telegram/both.
-- `revision` is copied into each delivery and checked immediately before an
-- external send, so disabling or rebinding a channel invalidates queued work.
CREATE TABLE user_notification_channels (
    user_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    state TEXT NOT NULL DEFAULT 'unbound'
        CHECK (state IN ('ready', 'unbound', 'invalid')),
    target TEXT,
    target_label TEXT,
    provider_identity TEXT,
    revision INTEGER NOT NULL DEFAULT 1,
    verified_at TEXT,
    invalidated_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, channel),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX idx_user_notification_channel_target
    ON user_notification_channels(channel, provider_identity, target)
    WHERE target IS NOT NULL AND state = 'ready';
CREATE INDEX idx_user_notification_channels_enabled
    ON user_notification_channels(user_id, enabled, state, channel);

-- The verified Bot API identity is runtime state, not editable configuration.
CREATE TABLE telegram_bot_runtime (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    readiness TEXT NOT NULL DEFAULT 'disabled'
        CHECK (readiness IN ('disabled', 'reconciling', 'ready', 'error')),
    bot_id TEXT,
    username TEXT,
    config_fingerprint TEXT,
    generation INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    verified_at TEXT,
    updated_at TEXT NOT NULL
);
INSERT INTO telegram_bot_runtime (id, readiness, generation, updated_at)
VALUES (1, 'disabled', 0, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

-- Bind-token rows are retained after revocation so issuance rate limits cannot
-- be bypassed by repeatedly replacing the currently active link.
CREATE TABLE telegram_bind_tokens (
    token_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    email_normalized TEXT NOT NULL,
    bot_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT,
    consumed_at TEXT,
    consumed_chat_id TEXT,
    created_ip TEXT,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_telegram_bind_tokens_user
    ON telegram_bind_tokens(user_id, created_at DESC);
CREATE INDEX idx_telegram_bind_tokens_ip
    ON telegram_bind_tokens(created_ip, created_at DESC)
    WHERE created_ip IS NOT NULL;
CREATE INDEX idx_telegram_bind_tokens_expiry
    ON telegram_bind_tokens(expires_at);

-- Both polling and webhook ingestion durably enqueue updates here. Handler
-- failures never advance or discard the Telegram update stream.
CREATE TABLE telegram_inbound_updates (
    bot_id TEXT NOT NULL,
    update_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'claimed', 'retry', 'completed', 'dead_letter')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT,
    claim_owner TEXT,
    claim_expires_at TEXT,
    last_error TEXT,
    received_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    PRIMARY KEY (bot_id, update_id)
);
CREATE INDEX idx_telegram_inbound_updates_claim
    ON telegram_inbound_updates(bot_id, status, next_attempt_at, claim_expires_at, update_id);
CREATE TABLE telegram_poll_cursors (
    bot_id TEXT PRIMARY KEY,
    next_offset INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);

-- A row is inserted while claiming capacity, marked started immediately before
-- the provider call, and then completed with the provider outcome. Hourly caps
-- count started attempts plus live reservations instead of outbox creation.
CREATE TABLE notification_delivery_attempts (
    id TEXT PRIMARY KEY,
    delivery_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    notification_lane TEXT NOT NULL,
    recipient TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('reserved', 'started', 'sent', 'retry', 'failed', 'cancelled')),
    reserved_at TEXT NOT NULL,
    reservation_expires_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    provider_message_id TEXT,
    error_message TEXT,
    FOREIGN KEY (delivery_id) REFERENCES notification_deliveries(id) ON DELETE CASCADE
);
CREATE INDEX idx_notification_attempts_channel_cap
    ON notification_delivery_attempts(
        channel, notification_lane, status, started_at, reservation_expires_at
    );
CREATE INDEX idx_notification_attempts_recipient_cap
    ON notification_delivery_attempts(
        LOWER(recipient), channel, notification_lane, status, started_at,
        reservation_expires_at
    );
CREATE INDEX idx_notification_attempts_delivery
    ON notification_delivery_attempts(delivery_id, reserved_at DESC);
CREATE UNIQUE INDEX idx_notification_attempts_live_delivery
    ON notification_delivery_attempts(delivery_id)
    WHERE status IN ('reserved', 'started');

ALTER TABLE client_notification_runtime
    ADD COLUMN telegram_recipient_hourly_limit INTEGER NOT NULL DEFAULT 10;
ALTER TABLE client_notification_runtime
    ADD COLUMN telegram_global_hourly_limit INTEGER NOT NULL DEFAULT 50;
