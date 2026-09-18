-- Merchant-scoped prepaid funding is deliberately separate from Market
-- credit.  Existing market_credit_accounts.balance_units remains the buyer's
-- outstanding receivable; no historical debt is reinterpreted as cash.

ALTER TABLE market_public_credit_policies
    ADD COLUMN legacy_only INTEGER NOT NULL DEFAULT 1 CHECK (legacy_only IN (0, 1));

CREATE TABLE market_prepaid_accounts (
    id TEXT PRIMARY KEY,
    buyer_user_id TEXT NOT NULL,
    buyer_email TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    supplier_email TEXT NOT NULL,
    currency TEXT NOT NULL CHECK (currency = 'USD'),
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'refund_pending', 'closed')),
    posted_balance_units INTEGER NOT NULL DEFAULT 0
        CHECK (posted_balance_units >= 0),
    held_balance_units INTEGER NOT NULL DEFAULT 0
        CHECK (held_balance_units >= 0 AND held_balance_units <= posted_balance_units),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (buyer_user_id, supplier_user_id, currency)
);

CREATE INDEX idx_market_prepaid_accounts_buyer
    ON market_prepaid_accounts(buyer_user_id, status, updated_at DESC);
CREATE INDEX idx_market_prepaid_accounts_supplier
    ON market_prepaid_accounts(supplier_user_id, status, updated_at DESC);

CREATE TABLE market_prepaid_ledger_entries (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    entry_kind TEXT NOT NULL CHECK (entry_kind IN (
        'topup_credit', 'usage_debit', 'refund_debit',
        'service_credit', 'manual_adjustment'
    )),
    direction TEXT NOT NULL CHECK (direction IN ('credit', 'debit')),
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    balance_after_units INTEGER NOT NULL CHECK (balance_after_units >= 0),
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    detail_json TEXT NOT NULL DEFAULT '{}',
    actor_user_id TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY(account_id) REFERENCES market_prepaid_accounts(id)
);

CREATE INDEX idx_market_prepaid_ledger_account
    ON market_prepaid_ledger_entries(account_id, created_at DESC, id DESC);
CREATE INDEX idx_market_prepaid_ledger_source
    ON market_prepaid_ledger_entries(source_kind, source_id);

CREATE TABLE market_funding_intents (
    id TEXT PRIMARY KEY,
    prepaid_account_id TEXT NOT NULL,
    payment_account_id TEXT NOT NULL,
    buyer_user_id TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    receiver_uid TEXT NOT NULL,
    asset TEXT NOT NULL CHECK (asset = 'USDT'),
    base_amount_units INTEGER NOT NULL CHECK (base_amount_units > 0),
    pay_amount_units INTEGER NOT NULL CHECK (
        pay_amount_units > base_amount_units AND
        pay_amount_units <= base_amount_units + 99
    ),
    amount_scale INTEGER NOT NULL CHECK (amount_scale = 10000),
    credited_money_units INTEGER,
    note_code TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN (
        'pending', 'credited', 'expired', 'cancelled', 'review_required'
    )),
    expires_at TEXT NOT NULL,
    late_grace_until TEXT NOT NULL,
    matched_transaction_id TEXT,
    cancellation_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    credited_at TEXT,
    cancelled_at TEXT,
    FOREIGN KEY(prepaid_account_id) REFERENCES market_prepaid_accounts(id)
);

CREATE UNIQUE INDEX uq_market_funding_intent_idempotency
    ON market_funding_intents(buyer_user_id, idempotency_key);
CREATE UNIQUE INDEX uq_market_funding_intent_pending_account
    ON market_funding_intents(prepaid_account_id) WHERE status = 'pending';
CREATE INDEX idx_market_funding_intents_payment_pending
    ON market_funding_intents(payment_account_id, status, expires_at);
CREATE INDEX idx_market_funding_intents_buyer
    ON market_funding_intents(buyer_user_id, created_at DESC);

CREATE TABLE market_funding_amount_reservations (
    id TEXT PRIMARY KEY,
    payment_account_id TEXT NOT NULL,
    asset TEXT NOT NULL CHECK (asset = 'USDT'),
    pay_amount_units INTEGER NOT NULL CHECK (pay_amount_units > 0),
    intent_id TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('reserved', 'cooldown', 'released')),
    reserved_at TEXT NOT NULL,
    cooldown_until TEXT,
    released_at TEXT
);

CREATE UNIQUE INDEX uq_market_funding_amount_live
    ON market_funding_amount_reservations(payment_account_id, asset, pay_amount_units)
    WHERE status IN ('reserved', 'cooldown');
CREATE INDEX idx_market_funding_amount_cooldown
    ON market_funding_amount_reservations(status, cooldown_until);

CREATE TABLE market_funding_receipts (
    id TEXT PRIMARY KEY,
    prepaid_account_id TEXT NOT NULL,
    funding_intent_id TEXT NOT NULL UNIQUE,
    payment_account_id TEXT NOT NULL,
    transaction_id TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('binance_auto', 'admin_reconciliation')),
    matched_by TEXT NOT NULL,
    asset TEXT NOT NULL CHECK (asset = 'USDT'),
    actual_amount_units INTEGER NOT NULL CHECK (actual_amount_units > 0),
    credited_money_units INTEGER NOT NULL CHECK (credited_money_units > 0),
    confirmed_at TEXT NOT NULL,
    FOREIGN KEY(prepaid_account_id) REFERENCES market_prepaid_accounts(id),
    FOREIGN KEY(funding_intent_id) REFERENCES market_funding_intents(id)
);

CREATE UNIQUE INDEX uq_market_funding_receipt_transaction
    ON market_funding_receipts(payment_account_id, transaction_id);
CREATE INDEX idx_market_funding_receipts_account
    ON market_funding_receipts(prepaid_account_id, confirmed_at DESC);

CREATE TABLE market_funding_reservations (
    id TEXT PRIMARY KEY,
    prepaid_account_id TEXT,
    buyer_user_id TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    currency TEXT NOT NULL CHECK (currency = 'USD'),
    product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
    product_ref TEXT NOT NULL,
    prepaid_units INTEGER NOT NULL DEFAULT 0 CHECK (prepaid_units >= 0),
    credit_units INTEGER NOT NULL DEFAULT 0 CHECK (credit_units >= 0),
    status TEXT NOT NULL CHECK (status IN ('active', 'captured', 'released', 'expired')),
    expires_at TEXT NOT NULL,
    release_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    released_at TEXT,
    UNIQUE (product_kind, product_ref),
    FOREIGN KEY(prepaid_account_id) REFERENCES market_prepaid_accounts(id)
);

CREATE INDEX idx_market_funding_reservations_account
    ON market_funding_reservations(prepaid_account_id, status, expires_at);
CREATE INDEX idx_market_funding_reservations_buyer_supplier
    ON market_funding_reservations(
        buyer_user_id, supplier_user_id, currency, status, expires_at
    );

CREATE TABLE market_accrual_allocations (
    id TEXT PRIMARY KEY,
    accrual_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('prepaid', 'credit')),
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    prepaid_ledger_entry_id TEXT,
    idempotency_key TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    FOREIGN KEY(accrual_id) REFERENCES market_accrual_entries(id),
    FOREIGN KEY(prepaid_ledger_entry_id) REFERENCES market_prepaid_ledger_entries(id)
);

CREATE INDEX idx_market_accrual_allocations_accrual
    ON market_accrual_allocations(accrual_id, source_kind);
CREATE INDEX idx_market_accrual_allocations_account
    ON market_accrual_allocations(account_id, source_kind, created_at);

CREATE TABLE market_prepaid_refund_requests (
    id TEXT PRIMARY KEY,
    prepaid_account_id TEXT NOT NULL,
    buyer_user_id TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
    currency TEXT NOT NULL CHECK (currency = 'USD'),
    status TEXT NOT NULL CHECK (status IN (
        'requested', 'approved', 'rejected', 'recorded', 'cancelled'
    )),
    reason TEXT,
    resolution_note TEXT,
    external_reference TEXT,
    requested_at TEXT NOT NULL,
    resolved_at TEXT,
    recorded_at TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(prepaid_account_id) REFERENCES market_prepaid_accounts(id)
);

CREATE UNIQUE INDEX uq_market_prepaid_refund_active
    ON market_prepaid_refund_requests(prepaid_account_id)
    WHERE status IN ('requested', 'approved');
CREATE INDEX idx_market_prepaid_refund_supplier
    ON market_prepaid_refund_requests(supplier_user_id, status, requested_at);

ALTER TABLE market_contract_adjustments
    ADD COLUMN prepaid_credit_units INTEGER NOT NULL DEFAULT 0
        CHECK (prepaid_credit_units >= 0);

CREATE TABLE market_prepaid_adjustment_credits (
    id TEXT PRIMARY KEY,
    adjustment_id TEXT NOT NULL UNIQUE,
    prepaid_account_id TEXT NOT NULL,
    ledger_entry_id TEXT NOT NULL UNIQUE,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    amount_minor INTEGER NOT NULL CHECK (amount_minor >= 0),
    created_at TEXT NOT NULL,
    FOREIGN KEY(adjustment_id) REFERENCES market_contract_adjustments(id),
    FOREIGN KEY(prepaid_account_id) REFERENCES market_prepaid_accounts(id),
    FOREIGN KEY(ledger_entry_id) REFERENCES market_prepaid_ledger_entries(id)
);

CREATE INDEX idx_market_prepaid_adjustment_account
    ON market_prepaid_adjustment_credits(prepaid_account_id, created_at DESC);

ALTER TABLE market_payment_reconciliation_cases ADD COLUMN funding_intent_id TEXT;
ALTER TABLE market_payment_reconciliation_cases ADD COLUMN prepaid_account_id TEXT;
ALTER TABLE binance_pay_transactions ADD COLUMN funding_intent_id TEXT;
