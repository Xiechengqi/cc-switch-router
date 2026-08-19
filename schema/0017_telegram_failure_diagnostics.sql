-- Preserve the technical transport error separately from a stable, actionable
-- diagnosis. Runtime health is intentionally independent from bot readiness so
-- a transient polling outage can be shown without invalidating existing binds.
ALTER TABLE telegram_bot_runtime ADD COLUMN transport_status TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE telegram_bot_runtime ADD COLUMN last_failure_code TEXT;
ALTER TABLE telegram_bot_runtime ADD COLUMN last_failure_hint TEXT;
ALTER TABLE telegram_bot_runtime ADD COLUMN last_failure_details_json TEXT;
ALTER TABLE telegram_bot_runtime ADD COLUMN last_failure_at TEXT;

ALTER TABLE user_notification_channel_checks ADD COLUMN failure_code TEXT;
ALTER TABLE user_notification_channel_checks ADD COLUMN failure_hint TEXT;
ALTER TABLE user_notification_channel_checks ADD COLUMN failure_details_json TEXT;
