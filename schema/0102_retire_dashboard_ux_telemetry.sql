-- Dashboard UX telemetry was write-only: nothing ever read these rows, and
-- the setting that fed it is gone.
DROP INDEX IF EXISTS idx_dashboard_ux_events_created;
DROP TABLE IF EXISTS dashboard_ux_events;
