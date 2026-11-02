-- Execution-price delivery evidence only; no balances or usage are rewritten.
CREATE TABLE share_value_pricing_activations (
  installation_id TEXT PRIMARY KEY REFERENCES installations(id) ON DELETE CASCADE,
  revision TEXT NOT NULL REFERENCES share_value_pricing_snapshots(revision),
  reported_at_ms INTEGER NOT NULL CHECK(reported_at_ms >= 0),
  confirmed_at_ms INTEGER NOT NULL CHECK(confirmed_at_ms >= 0)
) STRICT;
CREATE TABLE share_value_pricing_publication (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  target_catalog_revision TEXT NOT NULL,
  published_catalog_revision TEXT,
  updated_at_ms INTEGER NOT NULL
) STRICT;
