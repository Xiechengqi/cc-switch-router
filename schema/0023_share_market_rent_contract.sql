CREATE TABLE share_market_rent_quotes (
    id TEXT PRIMARY KEY,
    seat_id TEXT NOT NULL,
    listing_id TEXT NOT NULL,
    share_id TEXT NOT NULL,
    renter_user_id TEXT NOT NULL,
    renter_email TEXT NOT NULL,
    offer_revision INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL,
    trial_seconds_remaining INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL CHECK (status IN ('active', 'consumed', 'expired')),
    expires_at TEXT NOT NULL,
    consumed_subscription_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(seat_id) REFERENCES share_market_seats(id),
    FOREIGN KEY(listing_id) REFERENCES share_market_listings(id),
    FOREIGN KEY(consumed_subscription_id) REFERENCES share_market_subscriptions(id)
);

CREATE INDEX idx_share_market_rent_quotes_buyer
    ON share_market_rent_quotes(renter_user_id, status, expires_at);
CREATE INDEX idx_share_market_rent_quotes_seat
    ON share_market_rent_quotes(seat_id, status, expires_at);

ALTER TABLE share_market_subscriptions ADD COLUMN rent_quote_id TEXT;
ALTER TABLE share_market_subscriptions ADD COLUMN idempotency_key TEXT;
ALTER TABLE share_market_subscriptions ADD COLUMN request_fingerprint TEXT;
ALTER TABLE share_market_subscriptions ADD COLUMN service_started_at TEXT;

CREATE UNIQUE INDEX uq_share_market_rent_idempotency
    ON share_market_subscriptions(renter_user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE UNIQUE INDEX uq_share_market_rent_quote_subscription
    ON share_market_subscriptions(rent_quote_id)
    WHERE rent_quote_id IS NOT NULL;
