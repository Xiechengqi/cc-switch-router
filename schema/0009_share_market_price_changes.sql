CREATE TABLE share_market_price_changes (
    id TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL,
    previous_daily_rate_minor INTEGER NOT NULL CHECK (previous_daily_rate_minor > 0),
    proposed_daily_rate_minor INTEGER NOT NULL CHECK (proposed_daily_rate_minor > 0),
    currency TEXT NOT NULL CHECK (currency = 'USD'),
    base_offer_revision INTEGER NOT NULL CHECK (base_offer_revision > 0),
    applied_offer_revision INTEGER,
    status TEXT NOT NULL CHECK (status IN ('pending', 'accepted', 'rejected', 'cancelled', 'applied')),
    proposed_by_user_id TEXT NOT NULL,
    responded_by_user_id TEXT,
    resolution_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    responded_at TEXT,
    applied_at TEXT,
    FOREIGN KEY(subscription_id) REFERENCES share_market_subscriptions(id)
);

CREATE UNIQUE INDEX uq_share_market_open_price_change
    ON share_market_price_changes(subscription_id)
    WHERE status IN ('pending', 'accepted');

CREATE INDEX idx_share_market_price_changes_subscription
    ON share_market_price_changes(subscription_id, created_at DESC);
