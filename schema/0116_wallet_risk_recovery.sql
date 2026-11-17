CREATE TABLE market_financial_risk_policy (
 id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL DEFAULT 1,
 topup_user_day_minor INTEGER NOT NULL DEFAULT 200000 CHECK(topup_user_day_minor>0),
 topup_channel_day_minor INTEGER NOT NULL DEFAULT 2000000 CHECK(topup_channel_day_minor>0),
 topup_global_day_minor INTEGER NOT NULL DEFAULT 5000000 CHECK(topup_global_day_minor>0),
 topup_global_hour_minor INTEGER NOT NULL DEFAULT 1000000 CHECK(topup_global_hour_minor>0),
 incoming_user_day_minor INTEGER NOT NULL DEFAULT 1000000 CHECK(incoming_user_day_minor>0),
 chain_verification_required INTEGER NOT NULL DEFAULT 0 CHECK(chain_verification_required IN (0,1)),
 topup_circuit_open INTEGER NOT NULL DEFAULT 0 CHECK(topup_circuit_open IN (0,1)),
 updated_at TEXT NOT NULL
);
INSERT INTO market_financial_risk_policy(id,updated_at) VALUES(1,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
CREATE TABLE market_financial_risk_events (
 id TEXT PRIMARY KEY, kind TEXT NOT NULL, subject_id TEXT NOT NULL,
 object_id TEXT NOT NULL, details_json TEXT NOT NULL, created_at TEXT NOT NULL,
 UNIQUE(kind,subject_id,object_id)
);
CREATE TRIGGER market_risk_events_immutable_update BEFORE UPDATE ON market_financial_risk_events
 BEGIN SELECT RAISE(ABORT,'risk evidence is immutable'); END;
CREATE TRIGGER market_risk_events_immutable_delete BEFORE DELETE ON market_financial_risk_events
 BEGIN SELECT RAISE(ABORT,'risk evidence is immutable'); END;
CREATE INDEX idx_financial_risk_subject ON market_financial_risk_events(subject_id,kind,created_at);
CREATE TABLE market_c2c_payment_reviews (
 order_id TEXT PRIMARY KEY REFERENCES market_c2c_orders(id),
 opened_by TEXT NOT NULL, reason TEXT NOT NULL, evidence_due_at TEXT NOT NULL,
 buyer_evidence TEXT, evidence_at TEXT, escalated_at TEXT, resolved_at TEXT,
 created_at TEXT NOT NULL
);
ALTER TABLE market_c2c_orders ADD COLUMN payment_reference TEXT;
UPDATE market_c2c_orders SET payment_reference='C2C-'||upper(replace(id,'-',''));
CREATE UNIQUE INDEX idx_c2c_payment_reference ON market_c2c_orders(payment_reference);
CREATE TABLE market_chain_verification_evidence (
 id TEXT PRIMARY KEY, order_id TEXT NOT NULL, chain TEXT NOT NULL,
 token_id TEXT NOT NULL, tx_id TEXT NOT NULL, recipient TEXT NOT NULL,
 amount_atomic TEXT NOT NULL, anchor_json TEXT NOT NULL, observation_json TEXT NOT NULL,
 fingerprint TEXT NOT NULL UNIQUE, verified_at TEXT NOT NULL
);
CREATE INDEX idx_chain_evidence_receipt ON market_chain_verification_evidence(chain,token_id,tx_id);
CREATE INDEX idx_chain_evidence_order ON market_chain_verification_evidence(order_id);
CREATE TRIGGER market_chain_evidence_immutable_update BEFORE UPDATE ON market_chain_verification_evidence
 BEGIN SELECT RAISE(ABORT,'chain evidence is immutable'); END;
CREATE TRIGGER market_chain_evidence_immutable_delete BEFORE DELETE ON market_chain_verification_evidence
 BEGIN SELECT RAISE(ABORT,'chain evidence is immutable'); END;
CREATE TABLE market_transfer_returns (
 id TEXT PRIMARY KEY, transfer_id TEXT NOT NULL UNIQUE REFERENCES market_wallet_transfers(id),
 requested_by TEXT NOT NULL, reason TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'requested'
 CHECK(status IN ('requested','approved','declined')),
 version INTEGER NOT NULL DEFAULT 1, authorized_by TEXT, authorization_id TEXT,
 journal_id TEXT UNIQUE, created_at TEXT NOT NULL, resolved_at TEXT
);
CREATE TABLE market_financial_audit_cursors (
 name TEXT PRIMARY KEY, last_id TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL
);
CREATE TABLE market_passkey_recovery_permits (
 session_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, operation_id TEXT NOT NULL UNIQUE,
 actor TEXT NOT NULL, expires_at TEXT NOT NULL, used_at TEXT, created_at TEXT NOT NULL
);
ALTER TABLE market_wallet_transfer_quotes ADD COLUMN recipient_hint TEXT NOT NULL DEFAULT '';
ALTER TABLE market_wallet_transfer_quotes ADD COLUMN first_recipient INTEGER NOT NULL DEFAULT 1 CHECK(first_recipient IN(0,1));
CREATE TABLE market_transfer_return_consents (
 id TEXT PRIMARY KEY, return_id TEXT NOT NULL UNIQUE REFERENCES market_transfer_returns(id),
 actor_user_id TEXT NOT NULL, counterparty_user_id TEXT NOT NULL, amount_units INTEGER NOT NULL CHECK(amount_units>0),
 session_reference TEXT NOT NULL, request_proof_id TEXT, fingerprint TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE TRIGGER market_return_consents_immutable_update BEFORE UPDATE ON market_transfer_return_consents
 BEGIN SELECT RAISE(ABORT,'return consent is immutable'); END;
CREATE TRIGGER market_return_consents_immutable_delete BEFORE DELETE ON market_transfer_return_consents
 BEGIN SELECT RAISE(ABORT,'return consent is immutable'); END;
CREATE TRIGGER market_return_terminal_immutable BEFORE UPDATE ON market_transfer_returns WHEN OLD.status!='requested'
 BEGIN SELECT RAISE(ABORT,'completed return is immutable'); END;
CREATE VIEW market_verified_transfer_return_evidence AS
SELECT r.id FROM market_transfer_returns r
 JOIN market_wallet_transfers t ON t.id=r.transfer_id
 JOIN market_transfer_return_consents c ON c.id=r.authorization_id AND c.return_id=r.id
  AND c.actor_user_id=t.recipient_user_id AND c.counterparty_user_id=t.sender_user_id AND c.amount_units=t.amount_units
 JOIN market_journal_transactions j ON j.id=r.journal_id AND j.source_kind='wallet_transfer_return' AND j.source_id=r.id AND j.action='return'
  AND j.fingerprint=c.fingerprint AND c.fingerprint=t.id||':'||t.recipient_user_id||':'||t.sender_user_id||':'||t.amount_units
 JOIN market_journal_seals s ON s.journal_id=j.id AND s.posting_count=2
 WHERE r.status='approved' AND r.authorized_by=c.actor_user_id AND r.resolved_at=c.created_at AND j.created_at=c.created_at
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_available' AND p.subject_id=t.recipient_user_id AND p.signed_units=t.amount_units)
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_available' AND p.subject_id=t.sender_user_id AND p.signed_units=-t.amount_units)
 AND (SELECT COUNT(*) FROM market_journal_postings WHERE journal_id=j.id)=2;
