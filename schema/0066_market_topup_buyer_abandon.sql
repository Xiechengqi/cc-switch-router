-- A buyer may stop using Router's checkout link without claiming that the
-- DujiaoPay order has been cancelled. Preserve the finance status and evidence.
ALTER TABLE market_topup_orders ADD COLUMN buyer_abandoned_at TEXT;
CREATE INDEX idx_market_topup_buyer_abandon
    ON market_topup_orders(user_id, buyer_abandoned_at, status);
