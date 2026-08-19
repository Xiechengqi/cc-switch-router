-- Canonical Share access policy. Historical federated sale (`Yes`) is
-- intentionally migrated to private; historical `Free` remains public only
-- when no Share Market entitlement is active.
ALTER TABLE shares ADD COLUMN free_access INTEGER NOT NULL DEFAULT 0
    CHECK(free_access IN (0, 1));
ALTER TABLE shares ADD COLUMN share_access_policy_version INTEGER NOT NULL DEFAULT 0
    CHECK(share_access_policy_version >= 0);

UPDATE shares
   SET free_access = CASE
           WHEN for_sale = 'Free'
            AND NOT EXISTS (
                SELECT 1 FROM share_market_listings listing
                 WHERE listing.share_id = shares.share_id
                   AND listing.status = 'active'
                   AND listing.deleted_at IS NULL
            )
            AND NOT EXISTS (
                SELECT 1 FROM share_market_subscriptions subscription
                 WHERE subscription.share_id = shares.share_id
                   AND subscription.status NOT IN ('released', 'grant_failed')
            )
           THEN 1 ELSE 0 END,
       share_access_policy_version = 1,
       for_sale = 'No',
       market_access_mode = 'selected',
       access_by_app_json = '{}',
       app_settings_json = '{}',
       for_sale_official_price_percent_by_app_json = '{}';

CREATE TRIGGER share_free_access_blocks_market_entitlements_update
BEFORE UPDATE OF free_access ON shares
WHEN NEW.free_access = 1 AND (
    EXISTS (
        SELECT 1 FROM share_market_listings listing
         WHERE listing.share_id = NEW.share_id
           AND listing.status = 'active'
           AND listing.deleted_at IS NULL
    ) OR EXISTS (
        SELECT 1 FROM share_market_subscriptions subscription
         WHERE subscription.share_id = NEW.share_id
           AND subscription.status NOT IN ('released', 'grant_failed')
    )
)
BEGIN
    SELECT RAISE(ABORT, 'public free Share cannot have active Share Market entitlements');
END;

CREATE TRIGGER share_market_listing_blocks_free_access_insert
BEFORE INSERT ON share_market_listings
WHEN NEW.status = 'active' AND EXISTS (
    SELECT 1 FROM shares
     WHERE share_id = NEW.share_id AND free_access = 1
)
BEGIN
    SELECT RAISE(ABORT, 'public free Share cannot be listed in Share Market');
END;

CREATE TRIGGER share_market_listing_blocks_free_access_update
BEFORE UPDATE OF share_id, status, deleted_at ON share_market_listings
WHEN NEW.status = 'active' AND NEW.deleted_at IS NULL AND EXISTS (
    SELECT 1 FROM shares
     WHERE share_id = NEW.share_id AND free_access = 1
)
BEGIN
    SELECT RAISE(ABORT, 'public free Share cannot be listed in Share Market');
END;

CREATE TRIGGER share_market_subscription_blocks_free_access_insert
BEFORE INSERT ON share_market_subscriptions
WHEN NEW.status NOT IN ('released', 'grant_failed') AND EXISTS (
    SELECT 1 FROM shares
     WHERE share_id = NEW.share_id AND free_access = 1
)
BEGIN
    SELECT RAISE(ABORT, 'public free Share cannot have a Share Market subscription');
END;

CREATE TRIGGER share_market_subscription_blocks_free_access_update
BEFORE UPDATE OF share_id, status ON share_market_subscriptions
WHEN NEW.status NOT IN ('released', 'grant_failed') AND EXISTS (
    SELECT 1 FROM shares
     WHERE share_id = NEW.share_id AND free_access = 1
)
BEGIN
    SELECT RAISE(ABORT, 'public free Share cannot have a Share Market subscription');
END;
