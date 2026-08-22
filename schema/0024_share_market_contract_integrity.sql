ALTER TABLE share_market_rent_quotes ADD COLUMN required_app TEXT;

ALTER TABLE share_market_subscriptions ADD COLUMN required_app TEXT;
ALTER TABLE share_market_subscriptions
    ADD COLUMN service_snapshot_json TEXT NOT NULL DEFAULT '{}';

ALTER TABLE market_service_contracts ADD COLUMN service_started_at TEXT;

-- Quotes issued by an older Router do not identify the app whose service is
-- being purchased. They cannot be committed against the stronger contract.
UPDATE share_market_rent_quotes
SET status = 'expired', updated_at = expires_at
WHERE required_app IS NULL AND status = 'active';

-- Preserve readable history for rentals created before the service snapshot
-- became mandatory. New rentals always write a complete immutable snapshot.
UPDATE share_market_subscriptions
SET required_app = COALESCE(
        NULLIF((SELECT share.app_type
                FROM shares share
                WHERE share.share_id = share_market_subscriptions.share_id), ''),
        'unknown'
    ),
    service_snapshot_json = json_object(
        'schemaVersion', 0,
        'requiredApp', COALESCE(
            NULLIF((SELECT share.app_type
                    FROM shares share
                    WHERE share.share_id = share_market_subscriptions.share_id), ''),
            'unknown'
        ),
        'legacy', json('true')
    )
WHERE required_app IS NULL;

UPDATE market_service_contracts
SET service_started_at = activated_at
WHERE service_started_at IS NULL
  AND status IN ('trial', 'active', 'billing_suspended');

CREATE INDEX idx_share_market_rent_quotes_status_expiry
    ON share_market_rent_quotes(status, expires_at);
CREATE INDEX idx_share_market_rent_quotes_cleanup
    ON share_market_rent_quotes(status, updated_at);
CREATE INDEX idx_share_model_health_billing
    ON share_model_health_checks(share_id, app_type, checked_at DESC);
