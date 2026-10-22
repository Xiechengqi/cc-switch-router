-- A service commission is earned only by fulfilled delivery, never by a
-- recharge, wallet transfer, reservation, or withdrawal. All old lots keep 0.
CREATE TABLE market_commission_policy_control (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL CHECK (version > 0)
);
INSERT INTO market_commission_policy_control VALUES (1, 1);
CREATE TABLE market_commission_policies (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000),
    effective_at TEXT NOT NULL,
    actor_user_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);
INSERT INTO market_commission_policies VALUES
    (1, 1000, '1970-01-01T00:00:00+00:00', 'system', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
CREATE TABLE market_commission_policy_cancellations (
    policy_version INTEGER PRIMARY KEY REFERENCES market_commission_policies(version),
    replaced_by_version INTEGER NOT NULL REFERENCES market_commission_policies(version),
    created_at TEXT NOT NULL
);
CREATE TRIGGER trg_market_commission_policy_immutable_update BEFORE UPDATE ON market_commission_policies
BEGIN SELECT RAISE(ABORT, 'commission policy is immutable'); END;
CREATE TRIGGER trg_market_commission_policy_immutable_delete BEFORE DELETE ON market_commission_policies
BEGIN SELECT RAISE(ABORT, 'commission policy is immutable'); END;
CREATE TRIGGER trg_market_commission_cancellation_immutable_update BEFORE UPDATE ON market_commission_policy_cancellations
BEGIN SELECT RAISE(ABORT, 'commission policy cancellation is immutable'); END;
CREATE TRIGGER trg_market_commission_cancellation_immutable_delete BEFORE DELETE ON market_commission_policy_cancellations
BEGIN SELECT RAISE(ABORT, 'commission policy cancellation is immutable'); END;

CREATE TABLE market_commission_quote_terms (
    quote_id TEXT NOT NULL, provider_user_id TEXT NOT NULL,
    policy_version INTEGER NOT NULL, rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000),
    created_at TEXT NOT NULL, PRIMARY KEY (quote_id, provider_user_id)
);
CREATE TABLE market_commission_contract_terms (
    contract_id TEXT PRIMARY KEY REFERENCES market_platform_contracts(id),
    quote_id TEXT NOT NULL, policy_version INTEGER NOT NULL,
    rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000), created_at TEXT NOT NULL
);
CREATE TABLE market_commission_periods (
    id TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL REFERENCES market_platform_contracts(id),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('monthly_period', 'metered_budget')),
    source_id TEXT NOT NULL,
    policy_version INTEGER NOT NULL,
    rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000),
    service_anchor_at TEXT NOT NULL, created_at TEXT NOT NULL,
    UNIQUE (source_kind, source_id)
);
CREATE INDEX idx_market_commission_period_contract ON market_commission_periods(contract_id);
CREATE TRIGGER trg_market_commission_quote_update BEFORE UPDATE ON market_commission_quote_terms
BEGIN SELECT RAISE(ABORT, 'commission quote is immutable'); END;
CREATE TRIGGER trg_market_commission_quote_delete BEFORE DELETE ON market_commission_quote_terms
BEGIN SELECT RAISE(ABORT, 'commission quote is immutable'); END;
CREATE TRIGGER trg_market_commission_contract_update BEFORE UPDATE ON market_commission_contract_terms
BEGIN SELECT RAISE(ABORT, 'commission contract is immutable'); END;
CREATE TRIGGER trg_market_commission_contract_delete BEFORE DELETE ON market_commission_contract_terms
BEGIN SELECT RAISE(ABORT, 'commission contract is immutable'); END;
CREATE TRIGGER trg_market_commission_period_update BEFORE UPDATE ON market_commission_periods
BEGIN SELECT RAISE(ABORT, 'commission period is immutable'); END;
CREATE TRIGGER trg_market_commission_period_delete BEFORE DELETE ON market_commission_periods
BEGIN SELECT RAISE(ABORT, 'commission period is immutable'); END;

-- amount_units remains gross for compatibility with immutable deliveries.
-- Every Provider balance/payout uses net_amount_units, including zero net.
ALTER TABLE market_provider_earning_lots ADD COLUMN commission_units INTEGER NOT NULL DEFAULT 0 CHECK (commission_units >= 0);
ALTER TABLE market_provider_earning_lots ADD COLUMN commission_reversed_units INTEGER NOT NULL DEFAULT 0 CHECK (commission_reversed_units >= 0);
ALTER TABLE market_provider_earning_lots ADD COLUMN commission_unlocked_units INTEGER NOT NULL DEFAULT 0 CHECK (commission_unlocked_units >= 0);
ALTER TABLE market_provider_earning_lots ADD COLUMN net_amount_units INTEGER GENERATED ALWAYS AS (amount_units - commission_units) VIRTUAL;
CREATE TRIGGER trg_market_earning_split_insert BEFORE INSERT ON market_provider_earning_lots
WHEN NEW.commission_units > NEW.amount_units
 OR NEW.reversed_units + NEW.unlocked_units > NEW.net_amount_units
 OR NEW.commission_reversed_units + NEW.commission_unlocked_units > NEW.commission_units
BEGIN SELECT RAISE(ABORT, 'invalid earning split'); END;
CREATE TRIGGER trg_market_earning_split_update BEFORE UPDATE ON market_provider_earning_lots
WHEN NEW.commission_units != OLD.commission_units OR NEW.amount_units != OLD.amount_units
 OR NEW.reversed_units + NEW.unlocked_units > NEW.net_amount_units
 OR NEW.commission_reversed_units + NEW.commission_unlocked_units > NEW.commission_units
BEGIN SELECT RAISE(ABORT, 'invalid or mutable earning split'); END;

CREATE TABLE market_commission_splits (
    lot_id TEXT PRIMARY KEY REFERENCES market_provider_earning_lots(id),
    period_id TEXT NOT NULL REFERENCES market_commission_periods(id),
    gross_before_units INTEGER NOT NULL CHECK (gross_before_units >= 0),
    gross_after_units INTEGER NOT NULL CHECK (gross_after_units > gross_before_units),
    fee_before_units INTEGER NOT NULL CHECK (fee_before_units >= 0),
    fee_after_units INTEGER NOT NULL CHECK (fee_after_units >= fee_before_units),
    allocation_journal_id TEXT REFERENCES market_journal_transactions(id),
    created_at TEXT NOT NULL,
    UNIQUE (period_id, gross_before_units), UNIQUE (period_id, gross_after_units)
);
CREATE INDEX idx_market_commission_splits_period ON market_commission_splits(period_id);
CREATE UNIQUE INDEX idx_market_commission_splits_journal
    ON market_commission_splits(allocation_journal_id) WHERE allocation_journal_id IS NOT NULL;
CREATE TRIGGER trg_market_commission_split_update BEFORE UPDATE ON market_commission_splits
BEGIN SELECT RAISE(ABORT, 'commission split is immutable'); END;
CREATE TRIGGER trg_market_commission_split_delete BEFORE DELETE ON market_commission_splits
BEGIN SELECT RAISE(ABORT, 'commission split is immutable'); END;

CREATE TABLE market_commission_refunds (
    dispute_id TEXT NOT NULL REFERENCES market_service_disputes(id),
    lot_id TEXT NOT NULL REFERENCES market_provider_earning_lots(id),
    gross_units INTEGER NOT NULL CHECK (gross_units > 0),
    net_units INTEGER NOT NULL CHECK (net_units >= 0),
    fee_units INTEGER NOT NULL CHECK (fee_units >= 0),
    created_at TEXT NOT NULL,
    PRIMARY KEY (dispute_id, lot_id), CHECK (gross_units = net_units + fee_units)
);
CREATE INDEX idx_market_commission_refunds_lot ON market_commission_refunds(lot_id);
CREATE TABLE market_commission_audit_cursor (
    id INTEGER PRIMARY KEY CHECK (id = 1), last_lot_id TEXT NOT NULL DEFAULT ''
);
INSERT INTO market_commission_audit_cursor (id) VALUES (1);
CREATE TRIGGER trg_market_commission_refund_update BEFORE UPDATE ON market_commission_refunds
BEGIN SELECT RAISE(ABORT, 'commission refund is immutable'); END;
CREATE TRIGGER trg_market_commission_refund_delete BEFORE DELETE ON market_commission_refunds
BEGIN SELECT RAISE(ABORT, 'commission refund is immutable'); END;

-- Rebuild only the posting CHECK, preserving all IDs, seals, and dependent
-- views. legacy_alter_table prevents rewriting/validating the temporary gap.
DROP TRIGGER trg_market_journal_seal_validate;
PRAGMA legacy_alter_table = ON;
CREATE TABLE market_journal_postings_with_commission (
    id TEXT PRIMARY KEY,
    journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    account_kind TEXT NOT NULL CHECK (account_kind IN (
        'owner_clearing', 'wallet_available', 'wallet_held', 'unfulfilled',
        'provider_pending', 'provider_available', 'provider_held',
        'buyer_refund_held', 'owner_loss', 'platform_fee_revenue',
        'platform_commission_pending', 'platform_commission_revenue'
    )),
    subject_id TEXT NOT NULL, asset_code TEXT NOT NULL CHECK (asset_code = 'USD'),
    signed_units INTEGER NOT NULL CHECK (signed_units != 0), created_at TEXT NOT NULL
);
INSERT INTO market_journal_postings_with_commission SELECT * FROM market_journal_postings;
DROP TABLE market_journal_postings;
ALTER TABLE market_journal_postings_with_commission RENAME TO market_journal_postings;
PRAGMA legacy_alter_table = OFF;
CREATE INDEX idx_market_journal_postings_journal ON market_journal_postings(journal_id);
CREATE INDEX idx_market_journal_postings_subject ON market_journal_postings(account_kind, subject_id, created_at);
CREATE TRIGGER trg_market_journal_postings_no_update BEFORE UPDATE ON market_journal_postings
BEGIN SELECT RAISE(ABORT, 'market journal posting is immutable'); END;
CREATE TRIGGER trg_market_journal_postings_no_delete BEFORE DELETE ON market_journal_postings
BEGIN SELECT RAISE(ABORT, 'market journal posting is immutable'); END;
CREATE TRIGGER trg_market_journal_postings_no_append_after_seal BEFORE INSERT ON market_journal_postings
WHEN EXISTS (SELECT 1 FROM market_journal_seals WHERE journal_id = NEW.journal_id)
BEGIN SELECT RAISE(ABORT, 'market journal is sealed'); END;
CREATE TRIGGER trg_market_journal_seal_validate BEFORE INSERT ON market_journal_seals BEGIN
    SELECT CASE WHEN (SELECT COUNT(*) FROM market_journal_postings WHERE journal_id = NEW.journal_id) != NEW.posting_count
        THEN RAISE(ABORT, 'market journal posting count mismatch') END;
    SELECT CASE WHEN (SELECT COALESCE(SUM(signed_units), 0) FROM market_journal_postings WHERE journal_id = NEW.journal_id) != 0
        THEN RAISE(ABORT, 'market journal is unbalanced') END;
    SELECT CASE WHEN EXISTS (SELECT 1 FROM market_journal_postings p
        JOIN market_journal_transactions j ON j.id = p.journal_id
        WHERE p.journal_id = NEW.journal_id AND p.asset_code != j.asset_code)
        THEN RAISE(ABORT, 'market journal asset mismatch') END;
END;
