-- Retry provenance survives clearing the currently dispatched edit. Never
-- change existing request fingerprints, attempt budgets, rental states or money.
ALTER TABLE share_control_operations ADD COLUMN last_attempt_edit_id TEXT;
ALTER TABLE share_control_operations ADD COLUMN last_attempt_config_revision INTEGER
    CHECK (last_attempt_config_revision IS NULL OR last_attempt_config_revision >= 0);
ALTER TABLE share_control_operations ADD COLUMN last_error_code TEXT;
ALTER TABLE share_control_operations ADD COLUMN last_recovery_revision INTEGER
    CHECK (last_recovery_revision IS NULL OR last_recovery_revision >= 0);

-- Old timeouts cleared edit_id. Recover only a matching Router-issued attempt,
-- including its installation, entitlement and sequence, not an inferred result.
UPDATE share_control_operations AS operation
SET (last_attempt_edit_id, last_attempt_config_revision, last_error_code) = (
    SELECT edit.id,
           json_extract(edit.patch_json, '$.managedGrant.expectedConfigRevision'),
           edit.error_code
    FROM share_edit_requests edit
    JOIN share_market_subscriptions subscription ON subscription.id = operation.subscription_id
    WHERE edit.share_id = operation.share_id
      AND edit.installation_id = subscription.installation_id
      AND edit.created_by_email = 'share-market@router.internal'
      AND CASE WHEN json_valid(edit.patch_json) THEN
          json_extract(edit.patch_json, '$.managedGrant.operationId') = operation.id
          AND json_extract(edit.patch_json, '$.managedGrant.entitlementId') = operation.entitlement_id
          AND json_extract(edit.patch_json, '$.managedGrant.shareSequence') = operation.share_sequence
          AND json_extract(edit.patch_json, '$.managedGrant.action') = operation.action
          AND lower(trim(json_extract(edit.patch_json, '$.managedGrant.email'))) = lower(trim(operation.email))
          AND json_type(edit.patch_json, '$.managedGrant.expectedConfigRevision') = 'integer'
          AND json_extract(edit.patch_json, '$.managedGrant.expectedConfigRevision') >= 0
      ELSE 0 END
    ORDER BY edit.revision DESC
    LIMIT 1
);

CREATE INDEX idx_share_control_recovery
    ON share_control_operations(share_id, last_attempt_config_revision)
    WHERE action = 'revoke' AND status = 'pending' AND dead_lettered_at IS NULL;
