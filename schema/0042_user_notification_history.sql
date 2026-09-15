CREATE INDEX IF NOT EXISTS idx_notification_deliveries_user_history
    ON notification_deliveries(recipient_user_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_legacy_recipient_history
    ON notification_deliveries(LOWER(recipient), created_at DESC, id DESC)
    WHERE recipient_user_id IS NULL;
