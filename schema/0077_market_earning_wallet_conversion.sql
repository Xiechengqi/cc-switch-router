-- Matured Provider net may be irreversibly assigned to the same user's
-- consumption wallet. This is neither an external payout nor a top-up.
ALTER TABLE market_provider_earning_lots ADD COLUMN converted_units INTEGER NOT NULL DEFAULT 0 CHECK (converted_units >= 0);
ALTER TABLE market_wallet_policy ADD COLUMN earnings_conversions_paused INTEGER NOT NULL DEFAULT 0 CHECK (earnings_conversions_paused IN (0, 1));
ALTER TABLE market_finance_control ADD COLUMN earning_conversion_audit_cursor TEXT NOT NULL DEFAULT '';

CREATE TRIGGER trg_market_earning_disposal_insert BEFORE INSERT ON market_provider_earning_lots
WHEN NEW.held_units + NEW.paid_units > NEW.unlocked_units - NEW.converted_units
BEGIN SELECT RAISE(ABORT, 'earning already disposed'); END;
CREATE TRIGGER trg_market_earning_disposal_update BEFORE UPDATE ON market_provider_earning_lots
WHEN NEW.converted_units < OLD.converted_units
 OR NEW.held_units + NEW.paid_units > NEW.unlocked_units - NEW.converted_units
BEGIN SELECT RAISE(ABORT, 'earning already disposed or conversion reversed'); END;
CREATE INDEX idx_market_earning_conversion_candidates
 ON market_provider_earning_lots(provider_user_id, unlock_at, id) WHERE disputed = 0;

CREATE TABLE market_earning_conversion_quotes (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    terms_version INTEGER NOT NULL CHECK (terms_version = 1),
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_market_earning_conversion_quotes_user ON market_earning_conversion_quotes(user_id, created_at);
CREATE TABLE market_earning_wallet_conversions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    quote_id TEXT NOT NULL UNIQUE REFERENCES market_earning_conversion_quotes(id),
    idempotency_key TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    terms_version INTEGER NOT NULL CHECK (terms_version = 1),
    journal_id TEXT NOT NULL UNIQUE REFERENCES market_journal_transactions(id),
    created_at TEXT NOT NULL,
    UNIQUE(user_id, idempotency_key)
);
CREATE INDEX idx_market_earning_conversions_user ON market_earning_wallet_conversions(user_id, created_at, id);
CREATE TABLE market_earning_conversion_allocations (
    conversion_id TEXT NOT NULL REFERENCES market_earning_wallet_conversions(id),
    earning_lot_id TEXT NOT NULL REFERENCES market_provider_earning_lots(id),
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    converted_before_units INTEGER NOT NULL CHECK (converted_before_units >= 0),
    PRIMARY KEY(conversion_id, earning_lot_id),
    UNIQUE(earning_lot_id, converted_before_units)
);
CREATE INDEX idx_market_earning_conversion_allocations_lot ON market_earning_conversion_allocations(earning_lot_id);
CREATE TRIGGER trg_market_earning_conversion_quote_update BEFORE UPDATE ON market_earning_conversion_quotes
BEGIN SELECT RAISE(ABORT, 'conversion quote is immutable'); END;
CREATE TRIGGER trg_market_earning_conversion_quote_delete BEFORE DELETE ON market_earning_conversion_quotes
WHEN EXISTS(SELECT 1 FROM market_earning_wallet_conversions c WHERE c.quote_id = OLD.id)
BEGIN SELECT RAISE(ABORT, 'consumed conversion quote is immutable'); END;
CREATE TRIGGER trg_market_earning_conversion_update BEFORE UPDATE ON market_earning_wallet_conversions
BEGIN SELECT RAISE(ABORT, 'earning conversion is immutable'); END;
CREATE TRIGGER trg_market_earning_conversion_delete BEFORE DELETE ON market_earning_wallet_conversions
BEGIN SELECT RAISE(ABORT, 'earning conversion is immutable'); END;
CREATE TRIGGER trg_market_earning_conversion_allocation_update BEFORE UPDATE ON market_earning_conversion_allocations
BEGIN SELECT RAISE(ABORT, 'conversion allocation is immutable'); END;
CREATE TRIGGER trg_market_earning_conversion_allocation_delete BEFORE DELETE ON market_earning_conversion_allocations
BEGIN SELECT RAISE(ABORT, 'conversion allocation is immutable'); END;

CREATE VIEW market_verified_earning_conversion_evidence AS
SELECT c.id FROM market_earning_wallet_conversions c
JOIN market_earning_conversion_quotes q ON q.id = c.quote_id
JOIN market_journal_transactions j ON j.id = c.journal_id
JOIN market_journal_seals seal ON seal.journal_id = j.id
WHERE q.user_id = c.user_id AND q.amount_units = c.amount_units
 AND q.terms_version = c.terms_version
 AND julianday(c.created_at) >= julianday(q.created_at)
 AND julianday(c.created_at) < julianday(q.expires_at)
 AND j.source_kind = 'earning_conversion' AND j.source_id = c.id AND j.action = 'convert'
 AND j.asset_code = 'USD' AND j.created_at = c.created_at
 AND j.fingerprint = c.user_id || ':' || c.quote_id || ':' || c.amount_units || ':terms=1'
 AND seal.posting_count = 2
 AND EXISTS (SELECT 1 FROM market_journal_postings p WHERE p.journal_id = j.id
   AND p.account_kind = 'provider_available' AND p.subject_id = c.user_id AND p.signed_units = c.amount_units)
 AND EXISTS (SELECT 1 FROM market_journal_postings p WHERE p.journal_id = j.id
   AND p.account_kind = 'wallet_available' AND p.subject_id = c.user_id AND p.signed_units = -c.amount_units)
 AND c.amount_units = (SELECT SUM(a.amount_units) FROM market_earning_conversion_allocations a WHERE a.conversion_id = c.id)
 AND NOT EXISTS (SELECT 1 FROM market_earning_conversion_allocations a
   LEFT JOIN market_provider_earning_lots l ON l.id = a.earning_lot_id
   WHERE a.conversion_id = c.id AND (
     l.id IS NULL OR l.provider_user_id != c.user_id OR julianday(l.unlock_at) > julianday(c.created_at)
     OR julianday(l.unlock_at) IS NULL
     OR a.amount_units > l.net_amount_units - a.converted_before_units
     OR (a.converted_before_units > 0 AND NOT EXISTS (
       SELECT 1 FROM market_earning_conversion_allocations prev
       WHERE prev.earning_lot_id = a.earning_lot_id
         AND prev.converted_before_units + prev.amount_units = a.converted_before_units))));
