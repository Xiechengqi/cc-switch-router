-- A mirror of Server-issued conversion receipts, independent of Share/grant
-- replacement. This is evidence, not a Router-owned balance or exchange rate.
CREATE TABLE share_quota_conversion_receipts (
  installation_id TEXT NOT NULL,
  share_id TEXT NOT NULL,
  operation_id TEXT NOT NULL,
  email TEXT NOT NULL,
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  received_at_ms INTEGER NOT NULL,
  PRIMARY KEY(installation_id,share_id,operation_id,email)
);
