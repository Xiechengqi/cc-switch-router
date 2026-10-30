-- API-equivalent quota counters, not platform money.
ALTER TABLE share_request_logs ADD COLUMN cache_creation_5m_tokens INTEGER CHECK(cache_creation_5m_tokens >= 0);
ALTER TABLE share_request_logs ADD COLUMN cache_creation_1h_tokens INTEGER CHECK(cache_creation_1h_tokens >= 0);
CREATE TABLE share_value_pricing_snapshots (
  revision TEXT PRIMARY KEY,
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  effective_from_ms INTEGER NOT NULL CHECK(effective_from_ms >= 0),
  published_by TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE share_value_pricing_config (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  revision TEXT NOT NULL REFERENCES share_value_pricing_snapshots(revision),
  version INTEGER NOT NULL CHECK(version>0)
) STRICT;
CREATE TABLE share_value_pricing_acks (
  installation_id TEXT NOT NULL REFERENCES installations(id),
  revision TEXT NOT NULL REFERENCES share_value_pricing_snapshots(revision),
  acknowledged_at_ms INTEGER NOT NULL,
  PRIMARY KEY(installation_id,revision)
) STRICT;
CREATE TABLE share_value_quota_projections (
  installation_id TEXT NOT NULL REFERENCES installations(id),
  share_id TEXT NOT NULL,
  email TEXT NOT NULL,
  policy_epoch TEXT NOT NULL,
  window_id TEXT NOT NULL,
  usage_watermark INTEGER NOT NULL CHECK(usage_watermark>=0),
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  synced_at_ms INTEGER NOT NULL,
  PRIMARY KEY(installation_id,share_id,email,window_id)
) STRICT;
CREATE INDEX share_value_quota_email_idx ON share_value_quota_projections(email,share_id,synced_at_ms);
