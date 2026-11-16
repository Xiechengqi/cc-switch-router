-- Share owners' text-token prices for one installation (client). They extend
-- or override the Router catalog only inside that installation's USD tariff.
CREATE TABLE share_value_owner_model_prices (
  installation_id TEXT NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
  model_key TEXT NOT NULL,
  display_name TEXT NOT NULL,
  input_micros_per_1m INTEGER NOT NULL CHECK(input_micros_per_1m >= 0),
  output_micros_per_1m INTEGER NOT NULL CHECK(output_micros_per_1m >= 0),
  cache_read_micros_per_1m INTEGER NOT NULL CHECK(cache_read_micros_per_1m >= 0),
  cache_write_5m_micros_per_1m INTEGER NOT NULL CHECK(cache_write_5m_micros_per_1m >= 0),
  cache_write_1h_micros_per_1m INTEGER CHECK(cache_write_1h_micros_per_1m >= 0),
  version INTEGER NOT NULL CHECK(version > 0),
  effective_from_ms INTEGER NOT NULL CHECK(effective_from_ms >= 0),
  updated_at_ms INTEGER NOT NULL,
  updated_by TEXT NOT NULL,
  PRIMARY KEY(installation_id, model_key)
) STRICT;
-- Append-only evidence. The usage breakdown reads the generation effective at
-- a window's pricing time from here; deletes are generations without rates.
CREATE TABLE share_value_owner_model_price_history (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  installation_id TEXT NOT NULL,
  model_key TEXT NOT NULL,
  action TEXT NOT NULL CHECK(action IN ('upsert','delete')),
  display_name TEXT,
  input_micros_per_1m INTEGER,
  output_micros_per_1m INTEGER,
  cache_read_micros_per_1m INTEGER,
  cache_write_5m_micros_per_1m INTEGER,
  cache_write_1h_micros_per_1m INTEGER,
  version INTEGER NOT NULL CHECK(version > 0),
  effective_from_ms INTEGER NOT NULL,
  actor TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  CHECK(action = 'delete' OR (input_micros_per_1m IS NOT NULL AND output_micros_per_1m IS NOT NULL
    AND cache_read_micros_per_1m IS NOT NULL AND cache_write_5m_micros_per_1m IS NOT NULL))
) STRICT;
CREATE INDEX share_value_owner_model_price_history_idx
  ON share_value_owner_model_price_history(installation_id, model_key, effective_from_ms);
-- An installation whose tariff differs from the global one (owner prices, or
-- a scheduled return to the global revision). Absent rows mean "global".
CREATE TABLE share_value_pricing_installation_tariffs (
  installation_id TEXT NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
  tariff_kind TEXT NOT NULL CHECK(tariff_kind IN ('text','resource')),
  base_revision TEXT NOT NULL,
  owner_digest TEXT NOT NULL,
  revision TEXT NOT NULL REFERENCES share_value_pricing_snapshots(revision),
  effective_from_ms INTEGER NOT NULL CHECK(effective_from_ms >= 0),
  version INTEGER NOT NULL CHECK(version > 0),
  PRIMARY KEY(installation_id, tariff_kind)
) STRICT;
-- Every revision issued to one installation, with that installation's own
-- activation time: content-addressed revisions can recur with a new lead time.
CREATE TABLE share_value_pricing_installation_revisions (
  installation_id TEXT NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
  revision TEXT NOT NULL REFERENCES share_value_pricing_snapshots(revision),
  effective_from_ms INTEGER NOT NULL CHECK(effective_from_ms >= 0),
  issued_at_ms INTEGER NOT NULL,
  PRIMARY KEY(installation_id, revision)
) STRICT;
