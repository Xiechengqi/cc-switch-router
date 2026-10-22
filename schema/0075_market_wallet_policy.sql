-- Consumption balances are non-withdrawable. Fees and internal transfers have
-- their own immutable evidence; neither can turn wallet credit into earnings.
CREATE TABLE market_wallet_policy (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    maintenance_fee_bps INTEGER NOT NULL DEFAULT 500 CHECK (maintenance_fee_bps BETWEEN 0 AND 10000),
    transfers_enabled INTEGER NOT NULL DEFAULT 1 CHECK (transfers_enabled IN (0, 1)),
    transfers_paused INTEGER NOT NULL DEFAULT 0 CHECK (transfers_paused IN (0, 1)),
    transfer_max_minor INTEGER NOT NULL DEFAULT 500000 CHECK (transfer_max_minor BETWEEN 1 AND 10000000),
    transfer_daily_max_minor INTEGER NOT NULL DEFAULT 500000 CHECK (transfer_daily_max_minor BETWEEN 1 AND 10000000),
    updated_at TEXT NOT NULL
);
INSERT INTO market_wallet_policy (id, updated_at)
VALUES (1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
CREATE TABLE market_wallet_policy_events (
    id TEXT PRIMARY KEY, actor_user_id TEXT NOT NULL,
    before_json TEXT NOT NULL, after_json TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE TRIGGER trg_market_wallet_policy_events_no_update BEFORE UPDATE ON market_wallet_policy_events
BEGIN SELECT RAISE(ABORT, 'wallet policy audit is immutable'); END;
CREATE TRIGGER trg_market_wallet_policy_events_no_delete BEFORE DELETE ON market_wallet_policy_events
BEGIN SELECT RAISE(ABORT, 'wallet policy audit is immutable'); END;

ALTER TABLE users ADD COLUMN email_verified_at TEXT;

-- amount_minor remains the gateway's gross USD amount. Wallet credit is
-- explicit; NULL on a pre-policy order means its original fee-free face value.
ALTER TABLE market_topup_orders ADD COLUMN credit_amount_minor INTEGER CHECK (credit_amount_minor > 0);
ALTER TABLE market_topup_orders ADD COLUMN platform_fee_minor INTEGER NOT NULL DEFAULT 0 CHECK (platform_fee_minor >= 0);
ALTER TABLE market_topup_orders ADD COLUMN platform_fee_rate_bps INTEGER NOT NULL DEFAULT 0 CHECK (platform_fee_rate_bps BETWEEN 0 AND 10000);
ALTER TABLE market_topup_orders ADD COLUMN wallet_policy_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE market_topup_orders ADD COLUMN gateway_cost_minor INTEGER NOT NULL DEFAULT 0 CHECK (gateway_cost_minor >= 0);
ALTER TABLE market_topup_orders ADD COLUMN quote_id TEXT;
CREATE UNIQUE INDEX idx_market_topup_quote ON market_topup_orders(quote_id) WHERE quote_id IS NOT NULL;
CREATE TRIGGER trg_market_topup_amounts_insert BEFORE INSERT ON market_topup_orders
WHEN NEW.credit_amount_minor IS NOT NULL AND NEW.amount_minor != NEW.credit_amount_minor + NEW.platform_fee_minor
BEGIN SELECT RAISE(ABORT, 'top-up gross amount mismatch'); END;
CREATE TRIGGER trg_market_topup_amounts_immutable
BEFORE UPDATE OF amount_minor, credit_amount_minor, platform_fee_minor, platform_fee_rate_bps, wallet_policy_version, gateway_cost_minor, quote_id
ON market_topup_orders
BEGIN SELECT RAISE(ABORT, 'top-up financial terms are immutable'); END;

CREATE TABLE market_topup_quotes (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    token_id TEXT,
    channel_revision INTEGER NOT NULL,
    channel_settings_version INTEGER NOT NULL,
    policy_version INTEGER NOT NULL,
    credit_minor INTEGER NOT NULL CHECK (credit_minor > 0),
    fee_bps INTEGER NOT NULL CHECK (fee_bps BETWEEN 0 AND 10000),
    fee_minor INTEGER NOT NULL CHECK (fee_minor >= 0),
    pay_minor INTEGER NOT NULL CHECK (pay_minor = credit_minor + fee_minor),
    gateway_cost_minor INTEGER NOT NULL CHECK (gateway_cost_minor >= 0),
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_market_topup_quotes_user ON market_topup_quotes(user_id, created_at);

CREATE TABLE market_wallet_transfer_quotes (
    id TEXT PRIMARY KEY,
    sender_user_id TEXT NOT NULL,
    recipient_user_id TEXT NOT NULL,
    sender_email TEXT NOT NULL,
    recipient_email TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK (sender_user_id != recipient_user_id)
);
CREATE INDEX idx_market_wallet_transfer_quotes_sender ON market_wallet_transfer_quotes(sender_user_id, created_at);
CREATE TABLE market_wallet_transfer_attempts (
    id TEXT PRIMARY KEY, sender_user_id TEXT NOT NULL, created_ip TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX idx_market_wallet_transfer_attempts_sender ON market_wallet_transfer_attempts(sender_user_id, created_at);
CREATE INDEX idx_market_wallet_transfer_attempts_ip ON market_wallet_transfer_attempts(created_ip, created_at);
CREATE TABLE market_wallet_transfer_challenges (
    id TEXT PRIMARY KEY,
    quote_id TEXT NOT NULL REFERENCES market_wallet_transfer_quotes(id),
    code_mac TEXT NOT NULL,
    key_version INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    expires_at TEXT NOT NULL,
    delivered_at TEXT,
    consumed_at TEXT,
    created_ip TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_market_wallet_transfer_challenges_quote ON market_wallet_transfer_challenges(quote_id, created_at);
CREATE INDEX idx_market_wallet_transfer_challenges_ip ON market_wallet_transfer_challenges(created_ip, created_at);
CREATE TABLE market_wallet_transfers (
    id TEXT PRIMARY KEY,
    sender_user_id TEXT NOT NULL,
    recipient_user_id TEXT NOT NULL,
    sender_email TEXT NOT NULL,
    recipient_email TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    quote_id TEXT NOT NULL UNIQUE REFERENCES market_wallet_transfer_quotes(id),
    challenge_id TEXT NOT NULL UNIQUE REFERENCES market_wallet_transfer_challenges(id),
    idempotency_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    journal_id TEXT NOT NULL UNIQUE REFERENCES market_journal_transactions(id),
    created_at TEXT NOT NULL,
    UNIQUE(sender_user_id, idempotency_key),
    CHECK (sender_user_id != recipient_user_id)
);
CREATE INDEX idx_market_wallet_transfers_sender ON market_wallet_transfers(sender_user_id, created_at, id);
CREATE INDEX idx_market_wallet_transfers_recipient ON market_wallet_transfers(recipient_user_id, created_at, id);
CREATE TRIGGER trg_market_wallet_transfers_no_update BEFORE UPDATE ON market_wallet_transfers
BEGIN SELECT RAISE(ABORT, 'wallet transfer is immutable'); END;
CREATE TRIGGER trg_market_wallet_transfers_no_delete BEFORE DELETE ON market_wallet_transfers
BEGIN SELECT RAISE(ABORT, 'wallet transfer is immutable'); END;

-- A gateway can report several partial reversals. Bind each to its external
-- reference and keep the original one-time manual-credit invariant.
ALTER TABLE market_topup_owner_actions RENAME TO market_topup_owner_actions_before_policy;
CREATE TABLE market_topup_owner_actions (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES market_topup_orders(id),
    action TEXT NOT NULL CHECK (action IN ('credit_manual_claim', 'reverse_external_refund')),
    actor_user_id TEXT NOT NULL, amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
    shortfall_minor INTEGER NOT NULL DEFAULT 0 CHECK (shortfall_minor >= 0),
    evidence_reference TEXT NOT NULL, reason_code TEXT NOT NULL,
    journal_id TEXT NOT NULL UNIQUE REFERENCES market_journal_transactions(id), created_at TEXT NOT NULL,
    UNIQUE (order_id, action, evidence_reference)
);
INSERT INTO market_topup_owner_actions SELECT * FROM market_topup_owner_actions_before_policy;
DROP TABLE market_topup_owner_actions_before_policy;
CREATE UNIQUE INDEX idx_market_topup_manual_credit_once ON market_topup_owner_actions(order_id)
WHERE action = 'credit_manual_claim';
CREATE TRIGGER trg_market_topup_owner_actions_no_update BEFORE UPDATE ON market_topup_owner_actions
BEGIN SELECT RAISE(ABORT, 'top-up owner action is immutable'); END;
CREATE TRIGGER trg_market_topup_owner_actions_no_delete BEFORE DELETE ON market_topup_owner_actions
BEGIN SELECT RAISE(ABORT, 'top-up owner action is immutable'); END;

-- Extend the posting kind without weakening the existing seal/immutability
-- triggers. The copy is schema-only: every original journal ID is preserved.
-- SQLite rewrites view references on RENAME; rebuild this view against the
-- final table and the new principal/fee split rather than a temporary name.
DROP VIEW market_verified_topup_evidence;
DROP TRIGGER trg_market_journal_postings_no_update;
DROP TRIGGER trg_market_journal_postings_no_delete;
DROP TRIGGER trg_market_journal_postings_no_append_after_seal;
DROP TRIGGER trg_market_journal_seal_validate;
ALTER TABLE market_journal_postings RENAME TO market_journal_postings_before_policy;
CREATE TABLE market_journal_postings (
    id TEXT PRIMARY KEY,
    journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    account_kind TEXT NOT NULL CHECK (account_kind IN (
        'owner_clearing', 'wallet_available', 'wallet_held', 'unfulfilled',
        'provider_pending', 'provider_available', 'provider_held',
        'buyer_refund_held', 'owner_loss', 'platform_fee_revenue'
    )),
    subject_id TEXT NOT NULL,
    asset_code TEXT NOT NULL CHECK (asset_code = 'USD'),
    signed_units INTEGER NOT NULL CHECK (signed_units != 0),
    created_at TEXT NOT NULL
);
INSERT INTO market_journal_postings SELECT * FROM market_journal_postings_before_policy;
DROP TABLE market_journal_postings_before_policy;
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
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM market_journal_postings posting
        JOIN market_journal_transactions journal ON journal.id = posting.journal_id
        WHERE posting.journal_id = NEW.journal_id AND posting.asset_code != journal.asset_code
    ) THEN RAISE(ABORT, 'market journal asset mismatch') END;
END;

CREATE VIEW market_topup_credit_journal_evidence AS
SELECT topup.id, topup.channel_id, topup.channel_revision, journal.action, journal.fingerprint
FROM market_topup_orders topup
JOIN market_journal_transactions journal ON journal.id = topup.credited_journal_id
 AND journal.source_kind = 'topup' AND journal.source_id = topup.id
 AND journal.action IN ('credit', 'credit_manual') AND journal.asset_code = 'USD'
JOIN market_journal_seals seal ON seal.journal_id = journal.id
WHERE topup.status = 'credited'
 AND topup.amount_minor = COALESCE(topup.credit_amount_minor, topup.amount_minor) + topup.platform_fee_minor
 AND seal.posting_count = CASE WHEN topup.platform_fee_minor > 0 THEN 3 ELSE 2 END
 AND (SELECT COUNT(*) FROM market_journal_postings WHERE journal_id = journal.id) = seal.posting_count
 AND EXISTS (SELECT 1 FROM market_journal_postings p WHERE p.journal_id = journal.id
     AND p.account_kind = 'wallet_available' AND p.subject_id = topup.user_id
     AND p.signed_units = -COALESCE(topup.credit_amount_minor, topup.amount_minor) * 86400 AND p.asset_code = 'USD')
 AND EXISTS (SELECT 1 FROM market_journal_postings p WHERE p.journal_id = journal.id
     AND p.account_kind = 'owner_clearing' AND p.subject_id = 'owner'
     AND p.signed_units = topup.amount_minor * 86400 AND p.asset_code = 'USD')
 AND (topup.platform_fee_minor = 0 OR EXISTS (SELECT 1 FROM market_journal_postings p
     WHERE p.journal_id = journal.id AND p.account_kind = 'platform_fee_revenue'
     AND p.subject_id = 'owner' AND p.signed_units = -topup.platform_fee_minor * 86400 AND p.asset_code = 'USD'));

CREATE VIEW market_verified_topup_evidence AS
SELECT topup.id, topup.channel_id, topup.channel_revision
FROM market_topup_orders topup
JOIN market_topup_credit_journal_evidence evidence ON evidence.id = topup.id AND evidence.action = 'credit'
JOIN market_topup_receipts receipt ON receipt.order_id = topup.id
WHERE topup.gateway_order_id IS NOT NULL AND receipt.confirmations > 0
 AND receipt.chain != '' AND receipt.token_id != '' AND receipt.tx_id != '' AND receipt.settled_amount != ''
 AND evidence.fingerprint = topup.user_id || ':' || topup.amount_minor || ':' || receipt.receipt_fingerprint
     || CASE WHEN topup.platform_fee_minor > 0
         THEN ':credit=' || topup.credit_amount_minor || ':fee=' || topup.platform_fee_minor ELSE '' END;

-- Manual merchant claims are valid financial credits, not live chain proof.
CREATE VIEW market_completed_topup_evidence AS
SELECT id FROM market_verified_topup_evidence
UNION
SELECT e.id FROM market_topup_credit_journal_evidence e
JOIN market_topup_receipts r ON r.order_id = e.id
JOIN market_topup_owner_actions a ON a.order_id = e.id AND a.action = 'credit_manual_claim'
JOIN market_topup_orders t ON t.id = e.id AND a.journal_id = t.credited_journal_id AND a.amount_minor = t.amount_minor
WHERE e.action = 'credit_manual'
 AND r.chain != '' AND r.token_id != '' AND r.tx_id != '' AND r.receipt_fingerprint != ''
 AND (e.fingerprint = 'manual:' || t.user_id || ':' || t.amount_minor || ':' || r.receipt_fingerprint
        || ':credit=' || COALESCE(t.credit_amount_minor, t.amount_minor) || ':fee=' || t.platform_fee_minor
   OR (t.credit_amount_minor IS NULL AND t.platform_fee_minor = 0
       AND e.fingerprint = 'manual:' || t.user_id || ':' || t.amount_minor || ':' || r.receipt_fingerprint));

CREATE TRIGGER trg_market_topup_quotes_no_update BEFORE UPDATE ON market_topup_quotes
BEGIN SELECT RAISE(ABORT, 'top-up quote is immutable'); END;
CREATE TRIGGER trg_market_wallet_transfer_quotes_no_update BEFORE UPDATE ON market_wallet_transfer_quotes
BEGIN SELECT RAISE(ABORT, 'wallet transfer quote is immutable'); END;
CREATE TRIGGER trg_market_wallet_transfer_challenge_binding
BEFORE UPDATE OF quote_id, code_mac, key_version, expires_at, created_ip, created_at ON market_wallet_transfer_challenges
BEGIN SELECT RAISE(ABORT, 'wallet transfer challenge binding is immutable'); END;

CREATE VIEW market_verified_wallet_transfer_evidence AS
SELECT t.id FROM market_wallet_transfers t
JOIN market_wallet_transfer_quotes q ON q.id = t.quote_id
 AND q.sender_user_id = t.sender_user_id AND q.recipient_user_id = t.recipient_user_id
 AND q.sender_email = t.sender_email AND q.recipient_email = t.recipient_email AND q.amount_units = t.amount_units
JOIN market_wallet_transfer_challenges c ON c.id = t.challenge_id AND c.quote_id = q.id
 AND c.delivered_at IS NOT NULL AND c.consumed_at IS NOT NULL
 AND c.attempts BETWEEN 1 AND 5 AND c.expires_at > t.created_at AND q.expires_at > t.created_at
JOIN market_journal_transactions j ON j.id = t.journal_id AND j.source_kind = 'wallet_transfer'
 AND j.source_id = t.id AND j.action = 'transfer' AND j.asset_code = 'USD' AND j.fingerprint = t.fingerprint
 AND t.fingerprint = t.sender_user_id || ':' || t.recipient_user_id || ':' || t.amount_units || ':' || t.quote_id
JOIN market_journal_seals s ON s.journal_id = j.id AND s.posting_count = 2
WHERE (SELECT COUNT(*) FROM market_journal_postings WHERE journal_id = j.id) = 2
 AND EXISTS (SELECT 1 FROM market_journal_postings p WHERE p.journal_id = j.id
   AND p.account_kind = 'wallet_available' AND p.subject_id = t.sender_user_id AND p.signed_units = t.amount_units AND p.asset_code = 'USD')
 AND EXISTS (SELECT 1 FROM market_journal_postings p WHERE p.journal_id = j.id
   AND p.account_kind = 'wallet_available' AND p.subject_id = t.recipient_user_id AND p.signed_units = -t.amount_units AND p.asset_code = 'USD');
