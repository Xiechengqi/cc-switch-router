-- Submission is not delivery. Preserve all existing receipts, envelopes and
-- idempotency keys; tracking must NEVER move accepted mail back to the queue.
ALTER TABLE market_notification_deliveries ADD COLUMN provider_status TEXT
    CHECK(provider_status IN ('accepted','delayed','delivered','failed','bounced','complained','suppressed'));
ALTER TABLE market_notification_deliveries ADD COLUMN provider_status_at TEXT;
ALTER TABLE market_notification_deliveries ADD COLUMN provider_checked_at TEXT;
ALTER TABLE market_notification_deliveries ADD COLUMN provider_next_check_at TEXT;
ALTER TABLE market_notification_deliveries ADD COLUMN provider_check_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE market_notification_deliveries ADD COLUMN provider_poll_token TEXT;
ALTER TABLE market_notification_deliveries ADD COLUMN provider_poll_expires_at TEXT;
ALTER TABLE market_notification_deliveries ADD COLUMN provider_error TEXT;
ALTER TABLE market_notification_deliveries ADD COLUMN suppressed_at TEXT;
ALTER TABLE market_notification_deliveries ADD COLUMN suppression_reason TEXT;
UPDATE market_notification_deliveries
SET provider_status='accepted', provider_status_at=sent_at,
    provider_next_check_at=strftime('%Y-%m-%dT%H:%M:%f+00:00','now')
WHERE channel='email' AND status='sent' AND provider_message_id IS NOT NULL;
CREATE INDEX idx_market_notice_budget
    ON market_notification_deliveries(channel, first_attempt_at, purpose, recipient_user_id);
CREATE INDEX idx_market_notice_recipient_budget
    ON market_notification_deliveries(recipient_user_id, channel, purpose, first_attempt_at);
CREATE INDEX idx_market_notice_provider_id
    ON market_notification_deliveries(provider_message_id) WHERE channel='email';
CREATE INDEX idx_market_notice_tracking_due
    ON market_notification_deliveries(provider_next_check_at)
    WHERE channel='email' AND status='sent' AND provider_status IN ('accepted','delayed');
CREATE INDEX idx_market_notice_suppression
    ON market_notification_deliveries(recipient_user_id, suppressed_at);

-- Only normalized delivery facts, never raw webhook bodies, addresses or keys.
-- Unmatched signed events are bounded and retained briefly to cover callbacks
-- arriving before the send API response is committed.
CREATE TABLE market_notification_provider_events (
    id TEXT PRIMARY KEY,
    provider_message_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('accepted','delayed','delivered','failed','bounced','complained','suppressed')),
    occurred_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('webhook','poll')),
    error_code TEXT
);
CREATE INDEX idx_market_notice_provider_event_message
    ON market_notification_provider_events(provider_message_id, occurred_at);
CREATE INDEX idx_market_notice_provider_event_retention
    ON market_notification_provider_events(received_at);

-- One coalesced digest per recipient, not one deferred email per noisy event.
CREATE TABLE market_notification_service_digests (
    user_id TEXT PRIMARY KEY,
    window_started_at TEXT NOT NULL,
    due_at TEXT NOT NULL,
    changed_at TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    queued_version INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE market_notification_tracking_health (
    provider TEXT PRIMARY KEY CHECK(provider='resend'),
    poll_backoff_until TEXT,
    poll_error_code TEXT,
    last_poll_at TEXT,
    last_webhook_at TEXT
);
INSERT INTO market_notification_tracking_health(provider) VALUES('resend');
