-- Existing revisions keep signature verification. New poll-only revisions use
-- empty legacy ciphertext slots; the explicit mode is the authority.
ALTER TABLE market_payment_channel_revisions
    ADD COLUMN webhook_mode TEXT NOT NULL DEFAULT 'signed'
    CHECK (webhook_mode IN ('signed', 'poll_only'));

-- A webhook hint only advances this persisted schedule. It never changes a
-- payment state or journal, and a separate cooldown limits repeated hints.
ALTER TABLE market_topup_orders ADD COLUMN next_reconcile_at TEXT;
ALTER TABLE market_topup_orders ADD COLUMN next_hint_at TEXT;
ALTER TABLE market_topup_orders ADD COLUMN hint_reconcile_until TEXT;
ALTER TABLE market_topup_orders
    ADD COLUMN reconcile_hint_pending INTEGER NOT NULL DEFAULT 0
    CHECK (reconcile_hint_pending IN (0, 1));
UPDATE market_topup_orders
SET next_reconcile_at = updated_at
WHERE status != 'credited';
CREATE INDEX idx_market_topup_reconcile_due
    ON market_topup_orders(status, COALESCE(next_reconcile_at, updated_at), created_at);
CREATE INDEX idx_market_topup_channel_status
    ON market_topup_orders(channel_id, status);
