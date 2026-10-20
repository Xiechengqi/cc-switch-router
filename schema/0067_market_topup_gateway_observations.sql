-- Keep only redacted field-presence evidence from authenticated DujiaoPay
-- lookups. A terminal status and a payment marker must remain auditable
-- without persisting raw gateway responses or transaction identifiers.
CREATE TABLE market_topup_gateway_observations (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES market_topup_orders(id),
    source TEXT NOT NULL CHECK (source IN ('webhook', 'reconcile', 'owner_review')),
    gateway_status TEXT NOT NULL,
    evidence_flags_json TEXT NOT NULL,
    evidence_sha256 TEXT NOT NULL,
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    observation_count INTEGER NOT NULL DEFAULT 1 CHECK (observation_count > 0),
    UNIQUE (order_id, source, gateway_status, evidence_sha256)
);
CREATE INDEX idx_market_topup_gateway_observations_order
    ON market_topup_gateway_observations(order_id, first_observed_at DESC, id DESC);
CREATE INDEX idx_market_finance_case_object_status
    ON market_finance_reconciliation_cases(object_kind, object_id, status);
