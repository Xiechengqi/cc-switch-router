-- Withdrawal and refund hardening, and suspension lifetime.
--
-- * mark_paid records when the transfer happened, what actually arrived and
--   the network fee, typed by the Owner (never prefilled). A different
--   arrived amount needs a variance reason.
-- * The approver is kept so a self-approved transfer is visible.
-- * Withdrawal address changes need an emailed one-time code.
-- * suspended_at lets the worker end services suspended for too long.
--   suspension_cause = 'finance_hold' marks a suspension the Router caused;
--   it never times out, and its clock restarts once the hold clears.
--   Contracts already suspended start a fresh clock at migration time so
--   that every buyer gets the full window and its warning.
ALTER TABLE market_payout_requests ADD COLUMN transferred_at TEXT;
ALTER TABLE market_payout_requests ADD COLUMN actual_amount_minor INTEGER
    CHECK (actual_amount_minor IS NULL OR actual_amount_minor >= 0);
ALTER TABLE market_payout_requests ADD COLUMN network_fee_minor INTEGER
    CHECK (network_fee_minor IS NULL OR network_fee_minor >= 0);
ALTER TABLE market_payout_requests ADD COLUMN variance_reason TEXT;
ALTER TABLE market_payout_requests ADD COLUMN approved_by TEXT;

ALTER TABLE market_buyer_refund_requests ADD COLUMN transferred_at TEXT;
ALTER TABLE market_buyer_refund_requests ADD COLUMN actual_amount_minor INTEGER
    CHECK (actual_amount_minor IS NULL OR actual_amount_minor >= 0);
ALTER TABLE market_buyer_refund_requests ADD COLUMN network_fee_minor INTEGER
    CHECK (network_fee_minor IS NULL OR network_fee_minor >= 0);
ALTER TABLE market_buyer_refund_requests ADD COLUMN variance_reason TEXT;
ALTER TABLE market_buyer_refund_requests ADD COLUMN approved_by TEXT;

CREATE TABLE market_payout_address_challenges (
    id TEXT PRIMARY KEY,
    provider_user_id TEXT NOT NULL,
    network TEXT NOT NULL,
    address TEXT NOT NULL,
    code_sha256 TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_market_payout_address_challenges_provider
    ON market_payout_address_challenges(provider_user_id, created_at DESC);

ALTER TABLE market_platform_contracts ADD COLUMN suspended_at TEXT;
ALTER TABLE market_platform_contracts ADD COLUMN suspension_cause TEXT
    CHECK (suspension_cause IS NULL OR suspension_cause = 'finance_hold');
UPDATE market_platform_contracts
SET suspended_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE status = 'suspended';
CREATE TRIGGER trg_market_platform_contract_suspended_at
AFTER UPDATE OF status ON market_platform_contracts
WHEN (NEW.status = 'suspended') != (OLD.status = 'suspended')
BEGIN
    UPDATE market_platform_contracts
    SET suspended_at = CASE WHEN NEW.status = 'suspended' THEN NEW.updated_at ELSE NULL END,
        suspension_cause = CASE WHEN NEW.status = 'suspended' THEN suspension_cause ELSE NULL END
    WHERE id = NEW.id;
END;
