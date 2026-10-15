-- Platform wallet finance is additive and disabled until the cutover gate passes.
-- Historical provider-specific funding remains untouched by this migration.

CREATE TABLE market_finance_control (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    mode TEXT NOT NULL DEFAULT 'disabled'
        CHECK (mode IN ('disabled', 'shadow', 'enabled')),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_at TEXT NOT NULL
);
INSERT INTO market_finance_control (id, mode, updated_at)
VALUES (1, 'disabled', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

CREATE TABLE market_wallet_accounts (
    user_id TEXT NOT NULL,
    asset_code TEXT NOT NULL DEFAULT 'USD' CHECK (asset_code = 'USD'),
    posted_units INTEGER NOT NULL DEFAULT 0 CHECK (posted_units >= 0),
    held_units INTEGER NOT NULL DEFAULT 0 CHECK (held_units >= 0),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, asset_code),
    CHECK (posted_units >= held_units)
);

CREATE TABLE market_journal_transactions (
    id TEXT PRIMARY KEY,
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    action TEXT NOT NULL,
    asset_code TEXT NOT NULL CHECK (asset_code = 'USD'),
    fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (source_kind, source_id, action)
);
CREATE TABLE market_journal_postings (
    id TEXT PRIMARY KEY,
    journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    account_kind TEXT NOT NULL CHECK (account_kind IN (
        'owner_clearing', 'wallet_available', 'wallet_held',
        'unfulfilled', 'provider_pending', 'provider_available',
        'provider_held', 'buyer_refund_held', 'owner_loss'
    )),
    subject_id TEXT NOT NULL,
    asset_code TEXT NOT NULL CHECK (asset_code = 'USD'),
    signed_units INTEGER NOT NULL CHECK (signed_units != 0),
    created_at TEXT NOT NULL
);
CREATE TABLE market_journal_seals (
    journal_id TEXT PRIMARY KEY REFERENCES market_journal_transactions(id),
    posting_count INTEGER NOT NULL CHECK (posting_count >= 2),
    sealed_at TEXT NOT NULL
);
CREATE INDEX idx_market_journal_postings_journal
    ON market_journal_postings(journal_id);
CREATE INDEX idx_market_journal_postings_subject
    ON market_journal_postings(account_kind, subject_id, created_at);
CREATE TRIGGER trg_market_journal_transactions_no_update
BEFORE UPDATE ON market_journal_transactions BEGIN
    SELECT RAISE(ABORT, 'market journal is immutable');
END;
CREATE TRIGGER trg_market_journal_transactions_no_delete
BEFORE DELETE ON market_journal_transactions BEGIN
    SELECT RAISE(ABORT, 'market journal is immutable');
END;
CREATE TRIGGER trg_market_journal_postings_no_update
BEFORE UPDATE ON market_journal_postings BEGIN
    SELECT RAISE(ABORT, 'market journal posting is immutable');
END;
CREATE TRIGGER trg_market_journal_postings_no_delete
BEFORE DELETE ON market_journal_postings BEGIN
    SELECT RAISE(ABORT, 'market journal posting is immutable');
END;
CREATE TRIGGER trg_market_journal_postings_no_append_after_seal
BEFORE INSERT ON market_journal_postings
WHEN EXISTS (SELECT 1 FROM market_journal_seals WHERE journal_id = NEW.journal_id)
BEGIN
    SELECT RAISE(ABORT, 'market journal is sealed');
END;
CREATE TRIGGER trg_market_journal_seal_validate
BEFORE INSERT ON market_journal_seals BEGIN
    SELECT CASE WHEN (SELECT COUNT(*) FROM market_journal_postings
                      WHERE journal_id = NEW.journal_id) != NEW.posting_count
        THEN RAISE(ABORT, 'market journal posting count mismatch') END;
    SELECT CASE WHEN (SELECT COALESCE(SUM(signed_units), 0)
                      FROM market_journal_postings
                      WHERE journal_id = NEW.journal_id) != 0
        THEN RAISE(ABORT, 'market journal is unbalanced') END;
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM market_journal_postings posting
        JOIN market_journal_transactions journal ON journal.id = posting.journal_id
        WHERE posting.journal_id = NEW.journal_id
          AND posting.asset_code != journal.asset_code
    ) THEN RAISE(ABORT, 'market journal asset mismatch') END;
END;
CREATE TRIGGER trg_market_journal_seals_no_update
BEFORE UPDATE ON market_journal_seals BEGIN
    SELECT RAISE(ABORT, 'market journal seal is immutable');
END;
CREATE TRIGGER trg_market_journal_seals_no_delete
BEFORE DELETE ON market_journal_seals BEGIN
    SELECT RAISE(ABORT, 'market journal seal is immutable');
END;

CREATE TABLE market_wallet_holds (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    asset_code TEXT NOT NULL CHECK (asset_code = 'USD'),
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    purpose TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    remaining_units INTEGER NOT NULL CHECK (remaining_units >= 0),
    status TEXT NOT NULL CHECK (status IN ('active', 'captured', 'released')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    expires_at TEXT,
    UNIQUE (source_kind, source_id, purpose),
    CHECK (remaining_units <= amount_units),
    CHECK ((status = 'active' AND remaining_units > 0)
        OR (status != 'active' AND remaining_units = 0))
);
CREATE INDEX idx_market_wallet_holds_user_status
    ON market_wallet_holds(user_id, status, created_at);

CREATE TABLE market_payment_channels (
    id TEXT PRIMARY KEY,
    gateway_kind TEXT NOT NULL CHECK (gateway_kind = 'dujiaopay'),
    display_name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    sort_order INTEGER NOT NULL DEFAULT 0,
    current_revision INTEGER NOT NULL DEFAULT 0 CHECK (current_revision >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE market_payment_channel_revisions (
    channel_id TEXT NOT NULL REFERENCES market_payment_channels(id),
    revision INTEGER NOT NULL CHECK (revision > 0),
    merchant_id TEXT NOT NULL,
    merchant_fingerprint TEXT NOT NULL,
    api_key_id TEXT NOT NULL,
    api_secret_ciphertext TEXT NOT NULL,
    api_secret_nonce TEXT NOT NULL,
    webhook_secret_ciphertext TEXT NOT NULL,
    webhook_secret_nonce TEXT NOT NULL,
    key_version INTEGER NOT NULL CHECK (key_version > 0),
    allowed_methods_json TEXT NOT NULL DEFAULT '[]',
    verified_at TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (channel_id, revision)
);
CREATE TABLE market_topup_orders (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    channel_revision INTEGER NOT NULL,
    merchant_fingerprint TEXT NOT NULL,
    merchant_order_id TEXT NOT NULL UNIQUE,
    gateway_order_id TEXT,
    idempotency_key TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
    status TEXT NOT NULL CHECK (status IN (
        'created', 'creating', 'create_unknown', 'pending', 'verifying',
        'credited', 'expired', 'cancelled', 'review_required'
    )),
    checkout_url TEXT,
    expires_at TEXT,
    credited_journal_id TEXT,
    paid_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (channel_id, channel_revision)
        REFERENCES market_payment_channel_revisions(channel_id, revision),
    UNIQUE (user_id, idempotency_key),
    UNIQUE (merchant_fingerprint, gateway_order_id)
);
CREATE INDEX idx_market_topup_orders_pending
    ON market_topup_orders(status, updated_at);
CREATE INDEX idx_market_topup_orders_user
    ON market_topup_orders(user_id, created_at DESC);
CREATE TABLE market_topup_events (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES market_topup_orders(id),
    merchant_fingerprint TEXT NOT NULL,
    gateway_event_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    raw_body_sha256 TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'processed', 'review_required')),
    created_at TEXT NOT NULL,
    processed_at TEXT,
    UNIQUE (merchant_fingerprint, gateway_event_id)
);
CREATE TABLE market_topup_receipts (
    order_id TEXT PRIMARY KEY REFERENCES market_topup_orders(id),
    chain TEXT NOT NULL,
    token_id TEXT NOT NULL,
    tx_id TEXT NOT NULL,
    settled_amount TEXT NOT NULL,
    confirmations INTEGER NOT NULL CHECK (confirmations >= 0),
    receipt_fingerprint TEXT NOT NULL UNIQUE,
    observed_at TEXT NOT NULL
);

CREATE TABLE market_delivery_slices (
    id TEXT PRIMARY KEY,
    contract_kind TEXT NOT NULL CHECK (contract_kind IN ('share', 'client_host')),
    contract_id TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    period_id TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    debit_journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    earning_journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    created_at TEXT NOT NULL,
    UNIQUE (source_kind, source_id)
);
CREATE TABLE market_provider_earning_lots (
    id TEXT PRIMARY KEY,
    delivery_slice_id TEXT NOT NULL UNIQUE REFERENCES market_delivery_slices(id),
    provider_user_id TEXT NOT NULL,
    buyer_user_id TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    reversed_units INTEGER NOT NULL DEFAULT 0 CHECK (reversed_units >= 0),
    unlocked_units INTEGER NOT NULL DEFAULT 0 CHECK (unlocked_units >= 0),
    held_units INTEGER NOT NULL DEFAULT 0 CHECK (held_units >= 0),
    paid_units INTEGER NOT NULL DEFAULT 0 CHECK (paid_units >= 0),
    buyer_debited_at TEXT NOT NULL,
    fulfilled_at TEXT NOT NULL,
    unlock_at TEXT NOT NULL,
    disputed INTEGER NOT NULL DEFAULT 0 CHECK (disputed IN (0, 1)),
    created_at TEXT NOT NULL,
    CHECK (reversed_units <= amount_units),
    CHECK (unlocked_units <= amount_units - reversed_units),
    CHECK (held_units + paid_units <= unlocked_units)
);
CREATE INDEX idx_market_provider_lots_unlock
    ON market_provider_earning_lots(disputed, unlock_at, provider_user_id);
CREATE TABLE market_usage_posting_batches (
    id TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL,
    first_accrual_id TEXT NOT NULL,
    last_accrual_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    journal_id TEXT NOT NULL UNIQUE REFERENCES market_journal_transactions(id),
    created_at TEXT NOT NULL,
    UNIQUE (contract_id, last_accrual_id)
);

CREATE TABLE market_payout_destinations (
    id TEXT PRIMARY KEY,
    provider_user_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    asset_code TEXT NOT NULL CHECK (asset_code = 'USDT'),
    network TEXT NOT NULL,
    address TEXT NOT NULL,
    usable_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (provider_user_id, version)
);
CREATE TABLE market_payout_requests (
    id TEXT PRIMARY KEY,
    provider_user_id TEXT NOT NULL,
    destination_id TEXT NOT NULL REFERENCES market_payout_destinations(id),
    idempotency_key TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    status TEXT NOT NULL CHECK (status IN (
        'requested', 'approved', 'transfer_in_progress', 'paid',
        'rejected', 'transfer_failed', 'review_required'
    )),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    owner_user_id TEXT,
    external_reference TEXT UNIQUE,
    paid_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (provider_user_id, idempotency_key)
);
CREATE TABLE market_payout_allocations (
    request_id TEXT NOT NULL REFERENCES market_payout_requests(id),
    earning_lot_id TEXT NOT NULL REFERENCES market_provider_earning_lots(id),
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    PRIMARY KEY (request_id, earning_lot_id)
);
CREATE TABLE market_buyer_refund_requests (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    hold_id TEXT NOT NULL UNIQUE REFERENCES market_wallet_holds(id),
    idempotency_key TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    asset_code TEXT NOT NULL CHECK (asset_code = 'USDT'),
    network TEXT NOT NULL,
    address TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN (
        'requested', 'approved', 'transfer_in_progress', 'paid',
        'rejected', 'transfer_failed', 'review_required'
    )),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    owner_user_id TEXT,
    external_reference TEXT UNIQUE,
    paid_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (user_id, idempotency_key)
);
CREATE TABLE market_external_payout_receipts (
    id TEXT PRIMARY KEY,
    network TEXT NOT NULL,
    external_reference TEXT NOT NULL,
    object_kind TEXT NOT NULL CHECK (object_kind IN ('provider_withdrawal', 'buyer_refund')),
    object_id TEXT NOT NULL UNIQUE,
    amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
    recorded_at TEXT NOT NULL,
    UNIQUE (network, external_reference)
);
CREATE TABLE market_finance_reconciliation_cases (
    id TEXT PRIMARY KEY,
    code TEXT NOT NULL,
    object_kind TEXT NOT NULL,
    object_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'resolved')),
    detail_code TEXT,
    created_at TEXT NOT NULL,
    resolved_at TEXT
);
CREATE UNIQUE INDEX uq_market_finance_open_case
    ON market_finance_reconciliation_cases(code, object_kind, object_id)
    WHERE status = 'open';
CREATE TABLE market_finance_audit_events (
    id TEXT PRIMARY KEY,
    actor_user_id TEXT NOT NULL,
    action TEXT NOT NULL,
    object_kind TEXT NOT NULL,
    object_id TEXT NOT NULL,
    reason_code TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_market_finance_audit_object
    ON market_finance_audit_events(object_kind, object_id, created_at DESC);
