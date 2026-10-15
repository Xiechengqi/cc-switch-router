-- The last verified USD/CNY quote survives Router restarts. Old owner-set
-- rates are deliberately not copied into this table.
CREATE TABLE market_fx_rate_cache (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    rate_micros INTEGER NOT NULL CHECK (rate_micros BETWEEN 1000000 AND 20000000),
    source_timestamp INTEGER NOT NULL,
    fetched_at INTEGER NOT NULL
);

-- A credit account that reached settlement while the external quote was
-- unavailable must be invoiced after recovery, without invoicing unrelated
-- historical settlement_due accounts.
CREATE TABLE market_fx_deferred_invoices (
    account_id TEXT PRIMARY KEY REFERENCES market_credit_accounts(id) ON DELETE CASCADE,
    reason TEXT NOT NULL CHECK (reason IN ('credit_limit_reached', 'funding_exhausted', 'final_service_ended')),
    created_at TEXT NOT NULL
);
