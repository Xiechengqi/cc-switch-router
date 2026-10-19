-- Keep a durable, redacted record of the authenticated gateway assessment
-- that permitted an Owner to close an unpaid top-up. Neither the gateway
-- response body nor payment credentials are stored here.
CREATE TABLE market_topup_recovery_decisions (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL UNIQUE REFERENCES market_topup_orders(id),
    actor_user_id TEXT NOT NULL,
    gateway_order_id TEXT NOT NULL,
    gateway_status TEXT NOT NULL CHECK (gateway_status IN ('expired', 'canceled', 'cancelled')),
    gateway_checked_at TEXT NOT NULL,
    evidence_sha256 TEXT NOT NULL,
    review_case_codes_json TEXT NOT NULL,
    manual_reference TEXT,
    manual_confirmed INTEGER NOT NULL CHECK (manual_confirmed IN (0, 1)),
    reason_code TEXT NOT NULL,
    created_at TEXT NOT NULL
);
