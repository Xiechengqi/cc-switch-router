-- Proofs and credentials outlive login sessions so historical financial evidence
-- remains verifiable after logout, credential rotation and account recovery.
CREATE TABLE market_financial_request_proofs (
 id TEXT PRIMARY KEY, user_id TEXT NOT NULL, session_id TEXT NOT NULL,
 device_id TEXT NOT NULL, nonce TEXT NOT NULL, binding TEXT NOT NULL,
 canonical TEXT NOT NULL, signature TEXT NOT NULL, public_key TEXT NOT NULL,
 stepup_id TEXT, created_at TEXT NOT NULL, expires_at TEXT NOT NULL,
 UNIQUE(session_id, nonce)
);
CREATE TRIGGER market_financial_proofs_immutable_update BEFORE UPDATE ON market_financial_request_proofs
 BEGIN SELECT RAISE(ABORT,'financial request proof is immutable'); END;
CREATE INDEX idx_financial_proofs_user_time ON market_financial_request_proofs(user_id,created_at);
CREATE INDEX idx_financial_proofs_expiry ON market_financial_request_proofs(expires_at);
ALTER TABLE market_financial_authorizations ADD COLUMN request_proof_id TEXT;
CREATE TABLE market_passkeys (
 id TEXT PRIMARY KEY, user_id TEXT NOT NULL, credential_id TEXT NOT NULL UNIQUE,
 credential_json TEXT NOT NULL, version INTEGER NOT NULL DEFAULT 1,
 usable_after TEXT NOT NULL, created_at TEXT NOT NULL, revoked_at TEXT
);
CREATE INDEX idx_market_passkeys_user ON market_passkeys(user_id, revoked_at);
CREATE TABLE market_security_challenges (
 id TEXT PRIMARY KEY, user_id TEXT NOT NULL, session_id TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('register','authenticate')),
 binding TEXT NOT NULL, state_json TEXT NOT NULL,
 expires_at TEXT NOT NULL, completed_at TEXT, used_proof_id TEXT,
 credential_id TEXT, created_at TEXT NOT NULL
);
CREATE INDEX idx_market_security_challenges_expiry ON market_security_challenges(expires_at);
CREATE TABLE market_security_events (
 id TEXT PRIMARY KEY, user_id TEXT NOT NULL, actor TEXT NOT NULL,
 kind TEXT NOT NULL, object_id TEXT NOT NULL, details_json TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE TRIGGER market_security_events_immutable_update BEFORE UPDATE ON market_security_events
 BEGIN SELECT RAISE(ABORT,'security evidence is immutable'); END;
CREATE TRIGGER market_security_events_immutable_delete BEFORE DELETE ON market_security_events
 BEGIN SELECT RAISE(ABORT,'security evidence is immutable'); END;
