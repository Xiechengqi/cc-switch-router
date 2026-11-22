-- Wallet simplification: the login password replaces wallet Passkeys as the
-- financial confirmation factor, and transfers to a new recipient are held for
-- 24 hours before they credit, during which the sender may withdraw them.
-- Passkey tables and their evidence stay readable; no new Passkey can be added.

CREATE TABLE market_password_confirmations (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    binding TEXT NOT NULL,
    risk TEXT NOT NULL CHECK (risk IN ('confirm', 'immediate')),
    expires_at TEXT NOT NULL,
    completed_at TEXT,
    used_proof_id TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_market_password_confirmations_user ON market_password_confirmations(user_id, created_at);
CREATE INDEX idx_market_password_confirmations_expiry ON market_password_confirmations(expires_at);

CREATE TRIGGER trg_market_passkeys_retired_insert BEFORE INSERT ON market_passkeys
BEGIN SELECT RAISE(ABORT, 'wallet Passkeys were replaced by password confirmation'); END;

-- A delayed transfer reserves the sender's balance at confirmation. Release
-- creates the ordinary immutable wallet transfer (same id, confirmation time and
-- authorization) so historical transfer evidence keeps one shape.
CREATE TABLE market_wallet_pending_transfers (
    id TEXT PRIMARY KEY,
    sender_user_id TEXT NOT NULL,
    recipient_user_id TEXT NOT NULL,
    sender_email TEXT NOT NULL,
    recipient_email TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    quote_id TEXT NOT NULL UNIQUE REFERENCES market_wallet_transfer_quotes(id),
    authorization_id TEXT NOT NULL UNIQUE REFERENCES market_financial_authorizations(id),
    hold_id TEXT NOT NULL UNIQUE REFERENCES market_wallet_holds(id),
    idempotency_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'released', 'cancelled')),
    release_at TEXT NOT NULL,
    resolved_at TEXT,
    cancel_reason TEXT CHECK (cancel_reason IS NULL OR cancel_reason IN ('sender_withdrew', 'recipient_unavailable', 'sender_unavailable')),
    created_at TEXT NOT NULL,
    UNIQUE (sender_user_id, idempotency_key),
    CHECK (sender_user_id != recipient_user_id),
    CHECK ((status = 'pending') = (resolved_at IS NULL)),
    CHECK ((status = 'cancelled') = (cancel_reason IS NOT NULL))
);
CREATE INDEX idx_market_wallet_pending_transfers_due ON market_wallet_pending_transfers(status, release_at);
CREATE INDEX idx_market_wallet_pending_transfers_sender ON market_wallet_pending_transfers(sender_user_id, created_at);
CREATE INDEX idx_market_wallet_pending_transfers_recipient ON market_wallet_pending_transfers(recipient_user_id, created_at);
CREATE TRIGGER trg_market_wallet_pending_transfers_terms_immutable
BEFORE UPDATE OF id, sender_user_id, recipient_user_id, sender_email, recipient_email, amount_units,
    quote_id, authorization_id, hold_id, idempotency_key, fingerprint, release_at, created_at
ON market_wallet_pending_transfers
BEGIN SELECT RAISE(ABORT, 'pending wallet transfer terms are immutable'); END;
CREATE TRIGGER trg_market_wallet_pending_transfers_final
BEFORE UPDATE OF status ON market_wallet_pending_transfers
WHEN OLD.status != 'pending'
BEGIN SELECT RAISE(ABORT, 'resolved pending wallet transfer is final'); END;
CREATE TRIGGER trg_market_wallet_pending_transfers_no_delete
BEFORE DELETE ON market_wallet_pending_transfers
BEGIN SELECT RAISE(ABORT, 'pending wallet transfer evidence is immutable'); END;

-- Daily outgoing and rolling incoming limits count a delayed transfer from the
-- moment it is confirmed; a released one appears once, as the ordinary transfer.
CREATE VIEW market_wallet_transfer_commitments AS
SELECT sender_user_id, recipient_user_id, amount_units, created_at FROM market_wallet_transfers
UNION ALL
SELECT sender_user_id, recipient_user_id, amount_units, created_at
FROM market_wallet_pending_transfers WHERE status = 'pending';
