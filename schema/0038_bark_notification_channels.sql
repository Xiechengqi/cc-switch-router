-- Bark user bindings reuse the channel-neutral user_notification_channels row.
-- The `target` value is an authenticated encrypted credential envelope; only
-- the masked label and provider fingerprint are safe to display.

ALTER TABLE client_notification_runtime
    ADD COLUMN bark_recipient_hourly_limit INTEGER NOT NULL DEFAULT 10;
ALTER TABLE client_notification_runtime
    ADD COLUMN bark_global_hourly_limit INTEGER NOT NULL DEFAULT 50;

-- `revision` belongs to channel selection/delivery fencing and therefore moves
-- whenever a user switches away from or back to Bark.  Bark ciphertext must
-- instead be bound to the credential revision, which changes only when the
-- encrypted target changes.  Existing pre-release v1 envelopes used the then
-- current channel revision as AAD, so copy it when upgrading a development DB.
ALTER TABLE user_notification_channels
    ADD COLUMN credential_revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE user_notification_channels
    ADD COLUMN credential_key_fingerprint TEXT;
UPDATE user_notification_channels
   SET credential_revision = revision
 WHERE channel = 'bark';

-- A delivery freezes both revisions: `target_revision` rejects a stale channel
-- selection, while `credential_revision` remains suitable for decrypting the
-- frozen Bark envelope after harmless selection changes.
ALTER TABLE notification_deliveries
    ADD COLUMN credential_revision INTEGER NOT NULL DEFAULT 1;
UPDATE notification_deliveries
   SET credential_revision = target_revision
 WHERE channel = 'bark';

CREATE TABLE bark_binding_attempts (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    source_ip TEXT,
    provider_identity TEXT NOT NULL,
    target_fingerprint TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('started', 'success', 'failed')),
    http_status INTEGER CHECK(http_status IS NULL OR http_status BETWEEN 100 AND 599),
    failure_code TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_bark_binding_attempts_user_time
    ON bark_binding_attempts(user_id, created_at DESC);
CREATE INDEX idx_bark_binding_attempts_ip_time
    ON bark_binding_attempts(source_ip, created_at DESC)
    WHERE source_ip IS NOT NULL;
-- Periodic audit retention deletes are global rather than scoped by user/IP.
CREATE INDEX idx_bark_binding_attempts_created_at
    ON bark_binding_attempts(created_at);

-- Telegram bind-token audit rows now share the same seven-day retention job.
-- Its older indexes all lead with user/IP or expiry and cannot serve that
-- global created_at range delete efficiently.
CREATE INDEX idx_telegram_bind_tokens_created_at
    ON telegram_bind_tokens(created_at);

CREATE TABLE bark_provider_runtime (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    config_fingerprint TEXT,
    binding_config_fingerprint TEXT,
    generation INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0, 1)),
    status TEXT NOT NULL DEFAULT 'disabled'
        CHECK(status IN ('disabled', 'ready', 'degraded')),
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    circuit_open_until TEXT,
    last_error TEXT,
    last_failure_code TEXT,
    last_failure_hint TEXT,
    last_failure_at TEXT,
    last_success_at TEXT,
    updated_at TEXT NOT NULL
);
INSERT INTO bark_provider_runtime (id, generation, enabled, status, updated_at)
VALUES (1, 0, 0, 'disabled', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
