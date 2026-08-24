ALTER TABLE share_market_rent_quotes
    ADD COLUMN contract_apps_json TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(contract_apps_json) AND json_type(contract_apps_json) = 'array');
ALTER TABLE share_market_subscriptions
    ADD COLUMN contract_apps_json TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(contract_apps_json) AND json_type(contract_apps_json) = 'array');

-- A seat is a Share-wide entitlement. Preserve the App set advertised when
-- each quote or subscription was created, falling back to the legacy primary
-- App only when an older snapshot did not carry supportedApps.
UPDATE share_market_rent_quotes
SET contract_apps_json = CASE
        WHEN json_valid(snapshot_json) THEN CASE
            WHEN json_type(snapshot_json, '$.service.supportedApps') = 'array'
                 AND json_array_length(snapshot_json, '$.service.supportedApps') > 0
            THEN json_extract(snapshot_json, '$.service.supportedApps')
            WHEN required_app IN ('claude', 'codex', 'gemini')
            THEN json_array(required_app)
            ELSE '[]'
        END
        WHEN required_app IN ('claude', 'codex', 'gemini')
        THEN json_array(required_app)
        ELSE '[]'
    END;

UPDATE share_market_subscriptions
SET contract_apps_json = CASE
        WHEN json_valid(service_snapshot_json) THEN CASE
            WHEN json_type(service_snapshot_json, '$.supportedApps') = 'array'
                 AND json_array_length(service_snapshot_json, '$.supportedApps') > 0
            THEN json_extract(service_snapshot_json, '$.supportedApps')
            WHEN required_app IN ('claude', 'codex', 'gemini')
            THEN json_array(required_app)
            ELSE '[]'
        END
        WHEN required_app IN ('claude', 'codex', 'gemini')
        THEN json_array(required_app)
        ELSE '[]'
    END;

-- A v1 quote freezes only one App's provider terms and therefore cannot be
-- committed as the new all-App bundle.
UPDATE share_market_rent_quotes
SET status = 'expired', updated_at = expires_at
WHERE status = 'active';

-- Existing rentals stay active while reconciliation widens their managed
-- grant. This timestamp now records confirmation of the all-App policy.
UPDATE share_market_subscriptions
SET app_scope_enforced_at = NULL
WHERE status NOT IN ('released', 'grant_failed');
