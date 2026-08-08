CREATE TABLE market_trial_ledgers (
    buyer_user_id TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    product_kind TEXT NOT NULL,
    service_ref TEXT NOT NULL,
    currency TEXT NOT NULL,
    allowance_seconds INTEGER NOT NULL CHECK (allowance_seconds >= 0),
    consumed_seconds INTEGER NOT NULL DEFAULT 0 CHECK (consumed_seconds >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (buyer_user_id, supplier_user_id, product_kind, service_ref, currency)
);

CREATE TABLE market_free_usage_ledgers (
    buyer_user_id TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    product_kind TEXT NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('service', 'supplier')),
    scope_ref TEXT NOT NULL,
    allowance_seconds INTEGER NOT NULL CHECK (allowance_seconds >= 0),
    granted_seconds INTEGER NOT NULL DEFAULT 0 CHECK (granted_seconds >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (buyer_user_id, supplier_user_id, product_kind, scope_kind, scope_ref)
);

ALTER TABLE client_market_subscriptions ADD COLUMN free_usage_seconds INTEGER;
ALTER TABLE share_market_subscriptions ADD COLUMN free_usage_seconds INTEGER;

ALTER TABLE market_billing_disputes ADD COLUMN respond_by TEXT;
ALTER TABLE market_billing_disputes ADD COLUMN escalated_at TEXT;
ALTER TABLE market_billing_disputes ADD COLUMN auto_resolve_at TEXT;

CREATE INDEX idx_market_billing_disputes_sla
    ON market_billing_disputes(status, respond_by, auto_resolve_at);

CREATE TABLE market_credit_notes (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    invoice_id TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('service_credit', 'external_refund')),
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
    currency TEXT NOT NULL,
    reason TEXT NOT NULL,
    external_reference TEXT,
    status TEXT NOT NULL CHECK (status IN ('applied', 'recorded')),
    created_by_user_id TEXT NOT NULL,
    created_by_email TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_market_credit_notes_invoice
    ON market_credit_notes(invoice_id, created_at);
CREATE INDEX idx_market_credit_notes_account
    ON market_credit_notes(account_id, created_at);
