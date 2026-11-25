CREATE TABLE online_client_preflight (
    challenge TEXT PRIMARY KEY,
    installation_id TEXT NOT NULL REFERENCES installations(id),
    expires_at INTEGER NOT NULL,
    egress_ip TEXT,
    country_code TEXT
);
CREATE INDEX online_client_preflight_expiry ON online_client_preflight(expires_at);
ALTER TABLE online_client_listings ADD COLUMN probe_error TEXT;

ALTER TABLE online_client_events ADD COLUMN occurrences INTEGER NOT NULL DEFAULT 1 CHECK(occurrences > 0);
ALTER TABLE online_client_leases ADD COLUMN receipt_ack_json TEXT NOT NULL DEFAULT '[]';
