-- Track a buyer-requested authenticated status check separately from the
-- financial disposition of the order. A queued check is not a payment result.
ALTER TABLE market_topup_orders ADD COLUMN buyer_recheck_requested_at TEXT;
ALTER TABLE market_topup_orders ADD COLUMN buyer_recheck_completed_at TEXT;
ALTER TABLE market_topup_orders ADD COLUMN buyer_recheck_result TEXT
    CHECK (buyer_recheck_result IS NULL OR buyer_recheck_result IN
           ('queued', 'retrying', 'pending', 'verifying', 'review_required',
            'credited', 'expired', 'cancelled'));
CREATE INDEX idx_market_topup_gateway_observations_latest
    ON market_topup_gateway_observations(order_id, last_observed_at DESC, id DESC);
