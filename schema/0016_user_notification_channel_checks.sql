CREATE TABLE user_notification_channel_checks (
    id TEXT PRIMARY KEY,
    channel TEXT NOT NULL
        CHECK(length(channel) BETWEEN 1 AND 32)
        CHECK(channel NOT GLOB '*[^a-z0-9_]*'),
    config_fingerprint TEXT NOT NULL
        CHECK(length(config_fingerprint) = 64)
        CHECK(config_fingerprint NOT GLOB '*[^0-9a-f]*'),
    provider_identity TEXT,
    status TEXT NOT NULL CHECK(status IN ('success', 'failed')),
    actor_email TEXT NOT NULL,
    target_label TEXT,
    provider_message_id TEXT,
    http_status INTEGER CHECK(http_status IS NULL OR http_status BETWEEN 100 AND 599),
    error_message TEXT,
    tested_at TEXT NOT NULL
);

CREATE INDEX idx_user_notification_channel_checks_channel_time
    ON user_notification_channel_checks(
        channel, config_fingerprint, tested_at DESC, id DESC
    );
