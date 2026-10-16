-- An Owner-reviewed service dispute freezes all earning lots for one wallet
-- contract. Refunds are new journals against unpaid Provider liabilities;
-- historical earnings and external payout receipts are never rewritten.
CREATE TABLE market_service_disputes (
    id TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL REFERENCES market_platform_contracts(id),
    buyer_user_id TEXT NOT NULL,
    provider_user_id TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'resolved')),
    resolution_code TEXT,
    refunded_units INTEGER NOT NULL DEFAULT 0 CHECK (refunded_units >= 0),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TEXT NOT NULL,
    resolved_at TEXT
);
CREATE UNIQUE INDEX uq_market_service_open_dispute
    ON market_service_disputes(contract_id) WHERE status = 'open';
CREATE INDEX idx_market_service_disputes_buyer
    ON market_service_disputes(buyer_user_id, created_at DESC, id DESC);
CREATE TABLE market_service_dispute_lots (
    dispute_id TEXT NOT NULL REFERENCES market_service_disputes(id),
    earning_lot_id TEXT NOT NULL REFERENCES market_provider_earning_lots(id),
    PRIMARY KEY (dispute_id, earning_lot_id)
);
