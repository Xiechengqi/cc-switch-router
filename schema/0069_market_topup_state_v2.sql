-- Top-up state machine v2.
--
-- * Checkout visibility is derived from explicit buyer-stop / cancel markers.
--   A stored checkout URL is never erased by those actions, so "pending with no
--   URL" again means only "the create result is still unknown".
-- * The first create request body is frozen so every retry and idempotent
--   replay is byte-identical, even after a tunnel-domain change.
-- * A permanently rejected create (proven by an idempotent replay) closes the
--   order locally without a new status value: status = 'cancelled' together
--   with local_close_reason = 'never_created'.
-- * Gateway payment terms are kept for paid-order verification and for the
--   in-site direct checkout.
ALTER TABLE market_topup_orders ADD COLUMN cancel_requested_at TEXT;
ALTER TABLE market_topup_orders ADD COLUMN create_request_body TEXT;
ALTER TABLE market_topup_orders ADD COLUMN create_attempts INTEGER NOT NULL DEFAULT 0
    CHECK (create_attempts >= 0);
ALTER TABLE market_topup_orders ADD COLUMN create_last_error_code TEXT;
ALTER TABLE market_topup_orders ADD COLUMN local_close_reason TEXT
    CHECK (local_close_reason IS NULL OR local_close_reason IN ('never_created'));
ALTER TABLE market_topup_orders ADD COLUMN reconcile_failures INTEGER NOT NULL DEFAULT 0
    CHECK (reconcile_failures >= 0);
ALTER TABLE market_topup_orders ADD COLUMN gateway_chain TEXT;
ALTER TABLE market_topup_orders ADD COLUMN gateway_token_id TEXT;
ALTER TABLE market_topup_orders ADD COLUMN payable_amount TEXT;
ALTER TABLE market_topup_orders ADD COLUMN fx_rate TEXT;
ALTER TABLE market_topup_orders ADD COLUMN gateway_expires_at TEXT;
ALTER TABLE market_topup_orders ADD COLUMN pay_address TEXT;
ALTER TABLE market_topup_orders ADD COLUMN order_mode TEXT NOT NULL DEFAULT 'cashier'
    CHECK (order_mode IN ('cashier', 'direct'));

UPDATE market_topup_orders
SET cancel_requested_at = (
    SELECT MIN(audit.created_at) FROM market_finance_audit_events audit
    WHERE audit.object_kind = 'topup' AND audit.object_id = market_topup_orders.id
      AND audit.action = 'topup_gateway_cancel_requested'
);

-- Events covered by a later authenticated lookup are closed by reconciliation.
ALTER TABLE market_topup_events ADD COLUMN processed_by TEXT
    CHECK (processed_by IS NULL OR processed_by IN ('webhook', 'reconcile'));

-- Pausing top-ups is separate from pausing wallet purchases, and is available
-- in shadow mode as well.
ALTER TABLE market_finance_control ADD COLUMN new_topups_paused INTEGER NOT NULL DEFAULT 0
    CHECK (new_topups_paused IN (0, 1));

-- Orders stopped or cancel-requested before v2 had their URL erased and could
-- be misreported as "create unknown" although their gateway order is bound.
UPDATE market_topup_orders
SET status = 'pending'
WHERE status = 'create_unknown' AND gateway_order_id IS NOT NULL
  AND (buyer_abandoned_at IS NOT NULL OR cancel_requested_at IS NOT NULL);
