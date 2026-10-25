-- Financial authorization now uses the existing Router login, not email OTP.
-- Snapshot only non-secret session evidence; never reference expiring sessions.
CREATE TABLE market_financial_authorizations (
    id TEXT PRIMARY KEY, actor_user_id TEXT NOT NULL, session_reference TEXT NOT NULL CHECK(length(session_reference)>0),
    auth_source_kind TEXT NOT NULL, session_authenticated_at TEXT NOT NULL, session_access_expires_at TEXT NOT NULL,
    operation_kind TEXT NOT NULL CHECK(operation_kind IN ('wallet_transfer','c2c_release')),
    object_id TEXT NOT NULL, counterparty_user_id TEXT NOT NULL, amount_units INTEGER NOT NULL CHECK(amount_units>0),
    expected_version INTEGER NOT NULL CHECK(expected_version>=0), fingerprint TEXT NOT NULL,
    acknowledgement TEXT NOT NULL CHECK(acknowledgement IN ('transfer_confirmed','receipt_confirmed')),
    created_at TEXT NOT NULL, UNIQUE(operation_kind,object_id), CHECK(actor_user_id!=counterparty_user_id),
    CHECK(julianday(session_authenticated_at) IS NOT NULL AND julianday(created_at) IS NOT NULL
        AND julianday(session_authenticated_at)<=julianday(created_at)),
    CHECK(julianday(session_access_expires_at) IS NOT NULL
        AND julianday(session_access_expires_at)>julianday(created_at))
);
CREATE TRIGGER trg_market_financial_authorizations_no_update BEFORE UPDATE ON market_financial_authorizations
BEGIN SELECT RAISE(ABORT,'financial authorization is immutable'); END;
CREATE TRIGGER trg_market_financial_authorizations_no_delete BEFORE DELETE ON market_financial_authorizations
BEGIN SELECT RAISE(ABORT,'financial authorization is immutable'); END;

DROP VIEW market_verified_wallet_transfer_evidence;
CREATE TEMP TABLE wallet85_transfers AS SELECT * FROM market_wallet_transfers;
DROP TABLE market_wallet_transfers;
CREATE TABLE market_wallet_transfers (
    id TEXT PRIMARY KEY, sender_user_id TEXT NOT NULL, recipient_user_id TEXT NOT NULL,
    sender_email TEXT NOT NULL, recipient_email TEXT NOT NULL, amount_units INTEGER NOT NULL CHECK(amount_units>0),
    quote_id TEXT NOT NULL UNIQUE REFERENCES market_wallet_transfer_quotes(id),
    challenge_id TEXT UNIQUE REFERENCES market_wallet_transfer_challenges(id),
    authorization_id TEXT UNIQUE REFERENCES market_financial_authorizations(id),
    idempotency_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
    journal_id TEXT NOT NULL UNIQUE REFERENCES market_journal_transactions(id), created_at TEXT NOT NULL,
    UNIQUE(sender_user_id,idempotency_key), CHECK(sender_user_id!=recipient_user_id),
    CHECK((challenge_id IS NULL)!=(authorization_id IS NULL))
);
INSERT INTO market_wallet_transfers(id,sender_user_id,recipient_user_id,sender_email,recipient_email,amount_units,quote_id,challenge_id,idempotency_key,fingerprint,journal_id,created_at)
SELECT * FROM wallet85_transfers;
DROP TABLE wallet85_transfers;
CREATE INDEX idx_market_wallet_transfers_sender ON market_wallet_transfers(sender_user_id,created_at,id);
CREATE INDEX idx_market_wallet_transfers_recipient ON market_wallet_transfers(recipient_user_id,created_at,id);
CREATE TRIGGER trg_market_wallet_transfers_no_update BEFORE UPDATE ON market_wallet_transfers
BEGIN SELECT RAISE(ABORT,'wallet transfer is immutable'); END;
CREATE TRIGGER trg_market_wallet_transfers_no_delete BEFORE DELETE ON market_wallet_transfers
BEGIN SELECT RAISE(ABORT,'wallet transfer is immutable'); END;
-- Historical proofs remain readable, but no new transfer can use old OTP.
CREATE TRIGGER trg_market_wallet_transfers_session_only BEFORE INSERT ON market_wallet_transfers
WHEN NEW.authorization_id IS NULL BEGIN SELECT RAISE(ABORT,'login authorization required'); END;
CREATE TRIGGER trg_market_transfer_codes_retired_insert BEFORE INSERT ON market_wallet_transfer_challenges
BEGIN SELECT RAISE(ABORT,'financial email codes retired'); END;
CREATE TRIGGER trg_market_transfer_codes_retired_update BEFORE UPDATE ON market_wallet_transfer_challenges
BEGIN SELECT RAISE(ABORT,'historical email evidence is read-only'); END;
CREATE TRIGGER trg_market_transfer_codes_retired_delete BEFORE DELETE ON market_wallet_transfer_challenges
BEGIN SELECT RAISE(ABORT,'historical email evidence is read-only'); END;
CREATE TRIGGER trg_market_c2c_codes_retired_insert BEFORE INSERT ON market_c2c_challenges
BEGIN SELECT RAISE(ABORT,'financial email codes retired'); END;
CREATE TRIGGER trg_market_c2c_codes_retired_update BEFORE UPDATE ON market_c2c_challenges
BEGIN SELECT RAISE(ABORT,'historical email evidence is read-only'); END;
CREATE TRIGGER trg_market_c2c_codes_retired_delete BEFORE DELETE ON market_c2c_challenges
BEGIN SELECT RAISE(ABORT,'historical email evidence is read-only'); END;

CREATE VIEW market_verified_wallet_transfer_evidence AS
SELECT t.id FROM market_wallet_transfers t
JOIN market_wallet_transfer_quotes q ON q.id=t.quote_id
 AND q.sender_user_id=t.sender_user_id AND q.recipient_user_id=t.recipient_user_id
 AND q.sender_email=t.sender_email AND q.recipient_email=t.recipient_email AND q.amount_units=t.amount_units
JOIN market_journal_transactions j ON j.id=t.journal_id AND j.source_kind='wallet_transfer'
 AND j.source_id=t.id AND j.action='transfer' AND j.asset_code='USD' AND j.fingerprint=t.fingerprint
 AND t.fingerprint=t.sender_user_id||':'||t.recipient_user_id||':'||t.amount_units||':'||t.quote_id
JOIN market_journal_seals s ON s.journal_id=j.id AND s.posting_count=2
WHERE q.expires_at>t.created_at
 AND ((t.authorization_id IS NOT NULL AND EXISTS(SELECT 1 FROM market_financial_authorizations a
   WHERE a.id=t.authorization_id AND a.actor_user_id=t.sender_user_id AND a.counterparty_user_id=t.recipient_user_id
   AND a.operation_kind='wallet_transfer' AND a.object_id=t.id AND a.amount_units=t.amount_units
   AND a.expected_version=0 AND a.fingerprint=t.fingerprint AND a.acknowledgement='transfer_confirmed' AND a.created_at=t.created_at))
 OR (t.authorization_id IS NULL AND EXISTS(SELECT 1 FROM market_wallet_transfer_challenges c
   WHERE c.id=t.challenge_id AND c.quote_id=q.id AND c.delivered_at IS NOT NULL AND c.consumed_at IS NOT NULL
   AND c.attempts BETWEEN 1 AND 5 AND c.expires_at>t.created_at)))
 AND (SELECT COUNT(*) FROM market_journal_postings WHERE journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_available' AND p.subject_id=t.sender_user_id AND p.signed_units=t.amount_units AND p.asset_code='USD')
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_available' AND p.subject_id=t.recipient_user_id AND p.signed_units=-t.amount_units AND p.asset_code='USD');

ALTER TABLE market_c2c_orders ADD COLUMN release_authorization_id TEXT REFERENCES market_financial_authorizations(id);
CREATE UNIQUE INDEX idx_market_c2c_release_authorization ON market_c2c_orders(release_authorization_id) WHERE release_authorization_id IS NOT NULL;
CREATE TRIGGER trg_market_c2c_release_authorization_immutable BEFORE UPDATE OF release_authorization_id ON market_c2c_orders
WHEN OLD.release_authorization_id IS NOT NULL AND NEW.release_authorization_id IS NOT OLD.release_authorization_id
BEGIN SELECT RAISE(ABORT,'C2C release authorization is immutable'); END;

DROP VIEW market_verified_c2c_release_evidence;
CREATE VIEW market_verified_c2c_release_evidence AS
 SELECT o.id FROM market_c2c_orders o JOIN market_c2c_quotes q ON q.id=o.quote_id AND q.buyer_id=o.buyer_id AND q.seller_id=o.seller_id
 JOIN market_wallet_holds h ON h.id=o.hold_id AND h.user_id=o.seller_id AND h.source_kind='c2c_order' AND h.source_id=o.id AND h.purpose='c2c' AND h.amount_units=o.amount_units AND h.status='captured' AND h.remaining_units=0
 JOIN market_journal_transactions j ON j.id=o.release_journal_id AND j.source_kind='c2c_order' AND j.source_id=o.id AND j.action='release' AND j.fingerprint=o.seller_id||':'||o.buyer_id||':'||o.amount_units||':'||o.quote_id
 JOIN market_journal_seals s ON s.journal_id=j.id AND s.posting_count=2
 WHERE o.status='released' AND o.amount_units=q.credit_minor*86400 AND o.terms_version=q.flow_version AND o.paid_at IS NOT NULL AND o.released_at IS NOT NULL
 AND (SELECT COUNT(*) FROM market_journal_postings p WHERE p.journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_held' AND p.subject_id=o.seller_id AND p.signed_units=o.amount_units AND p.asset_code='USD')
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_available' AND p.subject_id=o.buyer_id AND p.signed_units=-o.amount_units AND p.asset_code='USD')
 AND (o.release_authorization_id IS NULL OR EXISTS(SELECT 1 FROM market_financial_authorizations a
   WHERE a.id=o.release_authorization_id AND a.actor_user_id=o.seller_id AND a.counterparty_user_id=o.buyer_id
   AND a.operation_kind='c2c_release' AND a.object_id=o.id AND a.amount_units=o.amount_units
   AND a.expected_version<o.version AND a.fingerprint=j.fingerprint AND a.acknowledgement='receipt_confirmed'
   AND a.created_at=o.released_at));
