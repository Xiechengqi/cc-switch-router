-- Nullable columns deliberately leave legacy second-precision rows unknown.
ALTER TABLE share_request_logs ADD COLUMN started_at_ms INTEGER;
ALTER TABLE share_request_logs ADD COLUMN quota_source_ref TEXT;
ALTER TABLE share_request_logs ADD COLUMN quota_source_epoch INTEGER;
ALTER TABLE share_request_logs ADD COLUMN quota_bucket_id TEXT;
CREATE INDEX idx_share_request_logs_exact_quota ON share_request_logs(
  share_id, user_email, quota_source_ref, quota_source_epoch, quota_bucket_id, started_at_ms
);
CREATE TABLE share_quota_retired_streams (
  share_id TEXT NOT NULL,
  stream_incarnation TEXT NOT NULL,
  PRIMARY KEY (share_id, stream_incarnation)
);
CREATE TABLE share_quota_active_streams (
  share_id TEXT PRIMARY KEY,
  stream_incarnation TEXT NOT NULL
);
