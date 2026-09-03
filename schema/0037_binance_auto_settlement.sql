CREATE TABLE binance_payment_accounts (
    id TEXT PRIMARY KEY,
    supplier_user_id TEXT NOT NULL,
    binance_uid TEXT NOT NULL,
    masked_api_key TEXT NOT NULL,
    credential_fingerprint TEXT NOT NULL,
    credentials_ciphertext TEXT NOT NULL,
    credential_nonce TEXT NOT NULL,
    encryption_key_version INTEGER NOT NULL CHECK (encryption_key_version > 0),
    credential_revision INTEGER NOT NULL DEFAULT 1 CHECK (credential_revision > 0),
    status TEXT NOT NULL CHECK (status IN ('verifying', 'verified', 'degraded', 'disabled')),
    automation_mode TEXT NOT NULL CHECK (automation_mode IN ('shadow', 'enabled')),
    payment_home_region TEXT NOT NULL,
    permissions_json TEXT NOT NULL DEFAULT '{}',
    permissions_verified_at TEXT,
    uid_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (uid_confirmed IN (0, 1)),
    uid_confirmation_source TEXT CHECK (
        uid_confirmation_source IS NULL OR
        uid_confirmation_source IN ('receiver_history', 'payment_observation')
    ),
    last_poll_success_at TEXT,
    last_poll_error_code TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    poll_cursor_at TEXT,
    next_poll_at TEXT,
    lease_owner TEXT,
    lease_until TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (uid_confirmed = 0 AND uid_confirmation_source IS NULL) OR
        (uid_confirmed = 1 AND uid_confirmation_source IS NOT NULL)
    )
);

CREATE UNIQUE INDEX uq_binance_payment_account_supplier_region
    ON binance_payment_accounts(supplier_user_id, payment_home_region);
CREATE UNIQUE INDEX uq_binance_payment_account_uid
    ON binance_payment_accounts(binance_uid);
CREATE UNIQUE INDEX uq_binance_payment_account_credential
    ON binance_payment_accounts(credential_fingerprint)
    WHERE credential_fingerprint != '';
CREATE INDEX idx_binance_payment_accounts_poll
    ON binance_payment_accounts(payment_home_region, status, next_poll_at, lease_until);

CREATE TABLE market_payment_intents (
    id TEXT PRIMARY KEY,
    invoice_id TEXT NOT NULL,
    payment_account_id TEXT NOT NULL,
    buyer_user_id TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    receiver_uid TEXT NOT NULL,
    asset TEXT NOT NULL CHECK (asset = 'USDT'),
    base_amount_units INTEGER NOT NULL CHECK (base_amount_units > 0),
    pay_amount_units INTEGER NOT NULL CHECK (
        pay_amount_units > base_amount_units AND
        pay_amount_units <= base_amount_units + 99
    ),
    amount_scale INTEGER NOT NULL CHECK (amount_scale = 10000),
    note_code TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'paid', 'expired', 'cancelled', 'review_required')),
    expires_at TEXT NOT NULL,
    late_grace_until TEXT NOT NULL,
    matched_transaction_id TEXT,
    cancellation_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    paid_at TEXT,
    cancelled_at TEXT
);

CREATE UNIQUE INDEX uq_market_payment_intent_active_invoice
    ON market_payment_intents(invoice_id)
    WHERE status = 'pending';
CREATE INDEX idx_market_payment_intents_account_pending
    ON market_payment_intents(payment_account_id, status, expires_at);
CREATE INDEX idx_market_payment_intents_status_expiry
    ON market_payment_intents(status, expires_at);
CREATE INDEX idx_market_payment_intents_invoice_created
    ON market_payment_intents(invoice_id, created_at DESC);

CREATE TABLE market_payment_amount_reservations (
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

CREATE UNIQUE INDEX uq_market_payment_amount_live
    ON market_payment_amount_reservations(payment_account_id, asset, pay_amount_units)
    WHERE status IN ('reserved', 'cooldown');
CREATE INDEX idx_market_payment_amount_cooldown
    ON market_payment_amount_reservations(status, cooldown_until);

CREATE TABLE binance_pay_transactions (
    id TEXT PRIMARY KEY,
    payment_account_id TEXT NOT NULL,
    account_credential_revision INTEGER NOT NULL CHECK (account_credential_revision > 0),
    account_binance_uid TEXT NOT NULL,
    encryption_key_version INTEGER NOT NULL CHECK (encryption_key_version > 0),
    transaction_id TEXT NOT NULL,
    order_id TEXT,
    order_type TEXT,
    transaction_time TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('incoming', 'outgoing', 'unknown')),
    currency TEXT NOT NULL,
    amount_units INTEGER NOT NULL,
    amount_scale INTEGER NOT NULL CHECK (amount_scale = 10000),
    receiver_uid TEXT,
    counterparty_fingerprint TEXT,
    raw_payload_ciphertext TEXT NOT NULL,
    raw_payload_nonce TEXT NOT NULL,
    ingestion_status TEXT NOT NULL CHECK (ingestion_status IN ('accepted', 'ignored', 'review_required')),
    match_status TEXT NOT NULL CHECK (match_status IN ('unmatched', 'matched', 'review_required', 'ignored')),
    match_reason TEXT,
    payment_intent_id TEXT,
    observed_at TEXT NOT NULL,
    matched_at TEXT
);

CREATE UNIQUE INDEX uq_binance_pay_transaction_account
    ON binance_pay_transactions(payment_account_id, transaction_id);
CREATE INDEX idx_binance_pay_transactions_match
    ON binance_pay_transactions(payment_account_id, match_status, transaction_time);

CREATE TABLE market_external_payment_receipts (
    id TEXT PRIMARY KEY,
    invoice_id TEXT NOT NULL UNIQUE,
    payment_intent_id TEXT NOT NULL UNIQUE,
    payment_account_id TEXT NOT NULL,
    transaction_id TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('binance_auto', 'admin_reconciliation')),
    matched_by TEXT NOT NULL,
    asset TEXT NOT NULL CHECK (asset = 'USDT'),
    expected_amount_units INTEGER NOT NULL CHECK (expected_amount_units > 0),
    actual_amount_units INTEGER NOT NULL CHECK (actual_amount_units > 0),
    confirmed_at TEXT NOT NULL
);

CREATE UNIQUE INDEX uq_market_external_receipt_transaction
    ON market_external_payment_receipts(payment_account_id, transaction_id);

CREATE TABLE market_payment_reconciliation_cases (
    id TEXT PRIMARY KEY,
    invoice_id TEXT,
    payment_intent_id TEXT,
    payment_account_id TEXT NOT NULL,
    transaction_id TEXT NOT NULL,
    case_kind TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'settled', 'ignored')),
    detail_json TEXT NOT NULL,
    resolution TEXT,
    resolved_by_user_id TEXT,
    created_at TEXT NOT NULL,
    resolved_at TEXT
);

CREATE UNIQUE INDEX uq_market_payment_reconciliation_transaction
    ON market_payment_reconciliation_cases(payment_account_id, transaction_id)
    WHERE status = 'open';
CREATE INDEX idx_market_payment_reconciliation_status
    ON market_payment_reconciliation_cases(status, created_at);
