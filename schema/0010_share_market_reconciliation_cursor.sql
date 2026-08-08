ALTER TABLE share_market_subscriptions ADD COLUMN last_reconciled_at TEXT;

CREATE INDEX idx_share_market_subscriptions_reconcile
    ON share_market_subscriptions(status, last_reconciled_at, created_at, id);
