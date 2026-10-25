-- Keep the business/in-app event intact. Delivery state is channel-specific;
-- successful email is never repeated merely because Telegram failed.
CREATE TABLE market_notification_deliveries (
    id TEXT PRIMARY KEY, event_id TEXT NOT NULL REFERENCES market_notification_outbox(id),
    recipient_user_id TEXT NOT NULL, channel TEXT NOT NULL CHECK(channel IN ('email','telegram')),
    purpose TEXT NOT NULL, semantic_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued','claimed','sent','suppressed','failed','blocked_config','uncertain')),
    envelope_json TEXT NOT NULL, template_version INTEGER NOT NULL DEFAULT 1,
    idempotency_key TEXT NOT NULL UNIQUE, provider_message_id TEXT,
    attempts INTEGER NOT NULL DEFAULT 0, first_attempt_at TEXT, next_attempt_at TEXT NOT NULL,
    claim_token TEXT, claim_expires_at TEXT, last_error TEXT, created_at TEXT NOT NULL, sent_at TEXT,
    UNIQUE(recipient_user_id,channel,semantic_key)
);
CREATE INDEX idx_market_notification_deliveries_event ON market_notification_deliveries(event_id,channel);
CREATE INDEX idx_market_notification_deliveries_due ON market_notification_deliveries(status,next_attempt_at);
CREATE TRIGGER trg_market_notification_delivery_envelope_immutable
BEFORE UPDATE OF event_id,recipient_user_id,channel,purpose,semantic_key,envelope_json,template_version,idempotency_key ON market_notification_deliveries
WHEN OLD.first_attempt_at IS NOT NULL
BEGIN SELECT RAISE(ABORT,'attempted notification envelope is immutable'); END;
CREATE TRIGGER trg_market_notification_delivery_first_attempt_immutable
BEFORE UPDATE OF first_attempt_at ON market_notification_deliveries
WHEN OLD.first_attempt_at IS NOT NULL AND NEW.first_attempt_at IS NOT OLD.first_attempt_at
BEGIN SELECT RAISE(ABORT,'notification first attempt is immutable'); END;
