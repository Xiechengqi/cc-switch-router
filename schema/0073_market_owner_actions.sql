-- Owner money actions on top-ups that automatic verification cannot settle.
--
-- * credit_manual_claim: the gateway reports the order paid through a
--   merchant claim or manual reconciliation (`paid_source` other than
--   `chain`). The Owner records the transfer reference; the receipt joins the
--   same uniqueness index as on-chain receipts.
-- * reverse_external_refund: the merchant recorded a refund in DujiaoPay for
--   an already credited top-up. The credit is reversed from the buyer's
--   available balance; any shortfall is absorbed as owner_loss.
-- Each action happens at most once per order.
CREATE TABLE market_topup_owner_actions (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES market_topup_orders(id),
    action TEXT NOT NULL CHECK (action IN ('credit_manual_claim', 'reverse_external_refund')),
    actor_user_id TEXT NOT NULL,
    amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
    shortfall_minor INTEGER NOT NULL DEFAULT 0 CHECK (shortfall_minor >= 0),
    evidence_reference TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    created_at TEXT NOT NULL,
    UNIQUE (order_id, action)
);
