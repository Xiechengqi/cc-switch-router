-- The notification center stores business events, not email/Telegram deliveries.
-- Market events continue to use their transactional outbox and existing read_at.
-- Client events and Owner-only incident transitions have independent read state.
CREATE TABLE user_notification_events (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK (scope IN ('user', 'owner')),
    recipient_email TEXT,
    category TEXT NOT NULL CHECK (category IN ('client', 'system')),
    event_kind TEXT NOT NULL,
    object_kind TEXT NOT NULL,
    object_id TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at_ms INTEGER NOT NULL,
    CHECK ((scope = 'user' AND recipient_email IS NOT NULL AND recipient_email <> '')
        OR (scope = 'owner' AND recipient_email IS NULL))
);
CREATE INDEX idx_user_notification_events_recipient
    ON user_notification_events(recipient_email, created_at_ms DESC, id DESC);
CREATE INDEX idx_user_notification_events_owner
    ON user_notification_events(created_at_ms DESC, id DESC) WHERE scope = 'owner';

CREATE TABLE user_notification_reads (
    user_id TEXT NOT NULL,
    notification_id TEXT NOT NULL REFERENCES user_notification_events(id) ON DELETE CASCADE,
    read_at TEXT NOT NULL,
    PRIMARY KEY (user_id, notification_id)
);
