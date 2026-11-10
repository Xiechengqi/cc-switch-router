-- v1 stays independently publishable for older Servers.
CREATE TABLE resource_price_catalog (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  revision INTEGER NOT NULL CHECK(revision>=0),
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE resource_price_history (
  revision INTEGER PRIMARY KEY,
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  actor TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE share_value_resource_pricing_config (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  revision TEXT NOT NULL REFERENCES share_value_pricing_snapshots(revision),
  version INTEGER NOT NULL CHECK(version>0)
) STRICT;
-- Keep the tariff kind after historical payloads are compacted to {}. An
-- archived v2 revision must still require schema 5 for ACK and activation.
CREATE TABLE share_value_resource_pricing_revisions (
  revision TEXT PRIMARY KEY REFERENCES share_value_pricing_snapshots(revision)
) STRICT;
CREATE TABLE installation_value_capabilities (
  installation_id TEXT PRIMARY KEY REFERENCES installations(id) ON DELETE CASCADE,
  resources INTEGER NOT NULL CHECK(resources IN (0,1)),
  updated_at_ms INTEGER NOT NULL
) STRICT;

ALTER TABLE share_market_seats ADD COLUMN value_tariff_policy_id TEXT NOT NULL DEFAULT 'api-equivalent-v1'
  CHECK(value_tariff_policy_id IN ('api-equivalent-v1','api-equivalent-v2'));
ALTER TABLE share_market_subscriptions ADD COLUMN value_tariff_policy_id TEXT NOT NULL DEFAULT 'api-equivalent-v1'
  CHECK(value_tariff_policy_id IN ('api-equivalent-v1','api-equivalent-v2'));
