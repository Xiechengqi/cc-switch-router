ALTER TABLE share_market_subscriptions
    ADD COLUMN integrity_state TEXT NOT NULL DEFAULT 'compatible'
        CHECK (integrity_state IN ('compatible', 'violated', 'remediating', 'terminated'));
ALTER TABLE share_market_subscriptions ADD COLUMN integrity_reason TEXT;
ALTER TABLE share_market_subscriptions ADD COLUMN integrity_checked_at TEXT;
ALTER TABLE share_market_subscriptions ADD COLUMN integrity_violated_at TEXT;
ALTER TABLE share_market_subscriptions ADD COLUMN app_scope_enforced_at TEXT;

UPDATE share_market_subscriptions
SET service_started_at = COALESCE(service_started_at, activated_at)
WHERE service_started_at IS NULL AND activated_at IS NOT NULL;

CREATE TABLE share_market_termination_quotes (
    id TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL,
    owner_user_id TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'consumed', 'expired')),
    expires_at TEXT NOT NULL,
    consumed_adjustment_id TEXT,
    commit_fingerprint TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(subscription_id) REFERENCES share_market_subscriptions(id)
);

CREATE INDEX idx_share_market_termination_quotes_owner
    ON share_market_termination_quotes(owner_user_id, status, expires_at);
CREATE INDEX idx_share_market_termination_quotes_subscription
    ON share_market_termination_quotes(subscription_id, status, expires_at);

CREATE TABLE market_contract_adjustments (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    contract_id TEXT NOT NULL,
    product_kind TEXT NOT NULL,
    product_ref TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('supplier_early_termination_refund')),
    status TEXT NOT NULL CHECK (status IN ('applied', 'refund_due', 'settled')),
    currency TEXT NOT NULL,
    elapsed_bps INTEGER NOT NULL CHECK (elapsed_bps BETWEEN 0 AND 10000),
    refund_bps INTEGER NOT NULL CHECK (refund_bps BETWEEN 0 AND 10000),
    refundable_base_units INTEGER NOT NULL CHECK (refundable_base_units >= 0),
    amount_units INTEGER NOT NULL CHECK (amount_units >= 0),
    amount_minor INTEGER NOT NULL CHECK (amount_minor >= 0),
    unbilled_credit_units INTEGER NOT NULL DEFAULT 0 CHECK (unbilled_credit_units >= 0),
    invoice_credit_units INTEGER NOT NULL DEFAULT 0 CHECK (invoice_credit_units >= 0),
    external_refund_units INTEGER NOT NULL DEFAULT 0 CHECK (external_refund_units >= 0),
    reason TEXT NOT NULL,
    calculation_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(contract_id) REFERENCES market_service_contracts(id)
);

CREATE INDEX idx_market_contract_adjustments_contract
    ON market_contract_adjustments(contract_id, created_at);
CREATE INDEX idx_market_contract_adjustments_account
    ON market_contract_adjustments(account_id, status, created_at);

CREATE TABLE market_adjustment_allocations (
    id TEXT PRIMARY KEY,
    adjustment_id TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('unbilled_accrual', 'invoice_credit', 'external_refund')),
    target_id TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    amount_minor INTEGER NOT NULL CHECK (amount_minor >= 0),
    created_at TEXT NOT NULL,
    UNIQUE(adjustment_id, target_kind, target_id),
    FOREIGN KEY(adjustment_id) REFERENCES market_contract_adjustments(id) ON DELETE CASCADE
);

CREATE INDEX idx_market_adjustment_allocations_target
    ON market_adjustment_allocations(target_kind, target_id);

CREATE TABLE market_refund_obligations (
    id TEXT PRIMARY KEY,
    adjustment_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    invoice_id TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    buyer_user_id TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
    currency TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'recorded', 'overdue')),
    due_at TEXT NOT NULL,
    external_reference TEXT,
    recorded_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(adjustment_id, invoice_id),
    FOREIGN KEY(adjustment_id) REFERENCES market_contract_adjustments(id) ON DELETE CASCADE
);

CREATE INDEX idx_market_refund_obligations_supplier
    ON market_refund_obligations(supplier_user_id, status, due_at);
CREATE INDEX idx_market_refund_obligations_buyer
    ON market_refund_obligations(buyer_user_id, status, created_at);

ALTER TABLE market_accrual_entries
    ADD COLUMN credited_units INTEGER NOT NULL DEFAULT 0 CHECK (credited_units >= 0);

-- Older invoice credits predate per-accrual net accounting. Allocate them in a
-- stable order so fixed-term refunds never include value already credited or
-- refunded. Credit-note units are capped by the original accrual balance.
WITH credit_totals AS (
    SELECT invoice_id, SUM(amount_units) AS credit_units
    FROM market_credit_notes
    WHERE (kind = 'service_credit' AND status = 'applied')
       OR (kind = 'external_refund' AND status = 'recorded')
    GROUP BY invoice_id
), ordered_accruals AS (
    SELECT accrual.id,
           accrual.amount_units,
           MAX(
               credit.credit_units - COALESCE(
                   SUM(accrual.amount_units) OVER (
                       PARTITION BY accrual.invoice_id
                       ORDER BY accrual.created_at, accrual.id
                       ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING
                   ),
                   0
               ),
               0
           ) AS remaining_credit_units
    FROM market_accrual_entries accrual
    JOIN credit_totals credit ON credit.invoice_id = accrual.invoice_id
), credit_allocations AS (
    SELECT id, MIN(amount_units, remaining_credit_units) AS credited_units
    FROM ordered_accruals
)
UPDATE market_accrual_entries
SET credited_units = MAX(
    credited_units,
    COALESCE(
        (SELECT allocation.credited_units
         FROM credit_allocations allocation
         WHERE allocation.id = market_accrual_entries.id),
        0
    )
)
WHERE id IN (SELECT id FROM credit_allocations);

ALTER TABLE market_credit_notes ADD COLUMN contract_id TEXT;
ALTER TABLE market_credit_notes ADD COLUMN adjustment_id TEXT;
ALTER TABLE market_credit_notes ADD COLUMN idempotency_key TEXT;

CREATE UNIQUE INDEX uq_market_credit_note_idempotency
    ON market_credit_notes(idempotency_key) WHERE idempotency_key IS NOT NULL;
CREATE INDEX idx_share_control_subscription_action_sequence
    ON share_control_operations(subscription_id, action, share_sequence);
CREATE INDEX idx_share_market_subscriptions_seat_history
    ON share_market_subscriptions(seat_id, created_at DESC, id DESC);
UPDATE share_market_subscriptions
SET released_at = COALESCE(released_at, updated_at, created_at)
WHERE status IN ('released', 'grant_failed') AND released_at IS NULL;
CREATE INDEX idx_share_market_subscriptions_renter_history
    ON share_market_subscriptions(renter_user_id, status, released_at DESC, id DESC);
CREATE INDEX idx_share_market_events_subscription_type
    ON share_market_events(subscription_id, event_type, created_at DESC);

-- Rewrite historical market events to the same allowlisted public projection
-- used at runtime. Supplier identity and payment contacts stay public; renter
-- identity, credit, declarations, evidence, and disputes do not.
UPDATE chat_messages
SET event_payload_json = json_remove(json_patch(
        json_object(
            'summary', COALESCE(json_extract(event_payload_json, '$.summary'), body),
            'marketKind', COALESCE(json_extract(event_payload_json, '$.marketKind'), ''),
            'billingEventType', json_extract(event_payload_json, '$.billingEventType'),
            'installationId', COALESCE(json_extract(event_payload_json, '$.installationId'), ''),
            'shareId', COALESCE(json_extract(event_payload_json, '$.shareId'), ''),
            'shareName', COALESCE(json_extract(event_payload_json, '$.shareName'), ''),
            'appType', json_extract(event_payload_json, '$.appType'),
            'subdomain', COALESCE(json_extract(event_payload_json, '$.subdomain'), ''),
            'ownerEmail', json_extract(event_payload_json, '$.ownerEmail'),
            'supplierEmail', COALESCE(
                json_extract(event_payload_json, '$.supplierEmail'),
                CASE WHEN json_extract(event_payload_json, '$.marketKind') IN ('client', 'client_host')
                     THEN json_extract(event_payload_json, '$.providerEmail') END
            ),
            'clientLabel', json_extract(event_payload_json, '$.clientLabel'),
            'providerEmail', json_extract(event_payload_json, '$.providerEmail'),
            'hostname', json_extract(event_payload_json, '$.hostname'),
            'status', json_extract(event_payload_json, '$.status'),
            'dailyRateMinor', json_extract(event_payload_json, '$.dailyRateMinor'),
            'currency', json_extract(event_payload_json, '$.currency'),
            'offerRevision', json_extract(event_payload_json, '$.offerRevision'),
            'trialHours', json_extract(event_payload_json, '$.trialHours'),
            'freeDurationDays', json_extract(event_payload_json, '$.freeDurationDays'),
            'activatedAt', json_extract(event_payload_json, '$.activatedAt'),
            'expiresAt', json_extract(event_payload_json, '$.expiresAt'),
            'reason', json_extract(event_payload_json, '$.reason'),
            'failureCode', json_extract(event_payload_json, '$.failureCode'),
            'paymentMethodKinds', json(COALESCE(
                json_extract(event_payload_json, '$.paymentMethodKinds'),
                (SELECT json_group_array(json_extract(method.value, '$.kind'))
                 FROM json_each(json_extract(event_payload_json, '$.paymentMethods')) AS method
                 WHERE json_type(method.value, '$.kind') = 'text'),
                '[]'
            )),
            'contacts', json(COALESCE(
                json_extract(event_payload_json, '$.contacts'),
                json_extract(event_payload_json, '$.paymentContacts'),
                '[]'
            ))
        ),
        json_patch(
            CASE WHEN json_extract(event_payload_json, '$.marketKind') = 'share' THEN
                json_object(
                    'listingId', json_extract(event_payload_json, '$.listingId'),
                    'seatId', json_extract(event_payload_json, '$.seatId'),
                    'seatPosition', json_extract(event_payload_json, '$.seatPosition'),
                    'seatStatus', json_extract(event_payload_json, '$.seatStatus'),
                    'subscriptionStatus', json_extract(event_payload_json, '$.subscriptionStatus'),
                    'parallelLimit', json_extract(event_payload_json, '$.parallelLimit'),
                    'tokenLimit', json_extract(event_payload_json, '$.tokenLimit'),
                    'tokenPeriod', json_extract(event_payload_json, '$.tokenPeriod'),
                    'dailyRateMinor', json_extract(event_payload_json, '$.dailyRateMinor'),
                    'currency', json_extract(event_payload_json, '$.currency'),
                    'serviceDurationDays', json_extract(event_payload_json, '$.serviceDurationDays'),
                    'offerRevision', json_extract(event_payload_json, '$.offerRevision')
                )
            ELSE json('{}') END,
            CASE json_type(event_payload_json, '$.providerDeniedClientAccess')
                WHEN 'true' THEN '{"providerDeniedClientAccess":true}'
                WHEN 'false' THEN '{"providerDeniedClientAccess":false}'
                WHEN NULL THEN '{}'
                ELSE json_object(
                    'providerDeniedClientAccess',
                    json_extract(event_payload_json, '$.providerDeniedClientAccess')
                )
            END
        )
    ),
        CASE WHEN json_type(event_payload_json, '$.billingEventType') IS NULL
             THEN '$.billingEventType' ELSE '$.__keep' END,
        CASE WHEN json_type(event_payload_json, '$.shareId') IS NULL
             THEN '$.shareId' ELSE '$.__keep' END,
        CASE WHEN json_type(event_payload_json, '$.shareName') IS NULL
             THEN '$.shareName' ELSE '$.__keep' END,
        CASE WHEN json_type(event_payload_json, '$.appType') IS NULL
             THEN '$.appType' ELSE '$.__keep' END,
        CASE WHEN json_type(event_payload_json, '$.subdomain') IS NULL
             THEN '$.subdomain' ELSE '$.__keep' END,
        CASE WHEN json_type(event_payload_json, '$.ownerEmail') IS NULL
             THEN '$.ownerEmail' ELSE '$.__keep' END,
        CASE WHEN json_type(event_payload_json, '$.supplierEmail') IS NULL
                       AND NOT (
                           json_extract(event_payload_json, '$.marketKind') IN ('client', 'client_host')
                           AND json_type(event_payload_json, '$.providerEmail') IS NOT NULL
                       )
             THEN '$.supplierEmail' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.clientLabel') IS NULL
             THEN '$.clientLabel' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.providerEmail') IS NULL
             THEN '$.providerEmail' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.hostname') IS NULL
             THEN '$.hostname' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.status') IS NULL
             THEN '$.status' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('share', 'client', 'client_host')
                       OR json_type(event_payload_json, '$.dailyRateMinor') IS NULL
             THEN '$.dailyRateMinor' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('share', 'client', 'client_host')
                       OR json_type(event_payload_json, '$.currency') IS NULL
             THEN '$.currency' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('share', 'client', 'client_host')
                       OR json_type(event_payload_json, '$.offerRevision') IS NULL
             THEN '$.offerRevision' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.trialHours') IS NULL
             THEN '$.trialHours' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.freeDurationDays') IS NULL
             THEN '$.freeDurationDays' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.activatedAt') IS NULL
             THEN '$.activatedAt' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.expiresAt') IS NULL
             THEN '$.expiresAt' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.providerDeniedClientAccess') IS NULL
             THEN '$.providerDeniedClientAccess' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.reason') IS NULL
             THEN '$.reason' ELSE '$.__keep' END
        ,CASE WHEN json_extract(event_payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(event_payload_json, '$.failureCode') IS NULL
             THEN '$.failureCode' ELSE '$.__keep' END
    ),
    payload_version = 2
WHERE message_kind = 'market_event'
  AND event_payload_json IS NOT NULL
  AND json_valid(event_payload_json)
  AND (json_extract(event_payload_json, '$.marketKind') IN ('share', 'billing', 'client', 'client_host')
       OR source_event_id LIKE 'share-market:%'
       OR source_event_id LIKE 'share_market:%'
       OR source_event_id LIKE 'market-billing:%'
       OR source_event_id LIKE 'market_billing:%'
       OR source_event_id LIKE 'client-market:%'
       OR source_event_id LIKE 'client_market:%');

UPDATE client_chat_system_outbox
SET payload_json = json_remove(json_patch(
        json_object(
            'summary', COALESCE(json_extract(payload_json, '$.summary'), ''),
            'marketKind', COALESCE(json_extract(payload_json, '$.marketKind'), ''),
            'billingEventType', json_extract(payload_json, '$.billingEventType'),
            'installationId', COALESCE(json_extract(payload_json, '$.installationId'), installation_id),
            'shareId', COALESCE(json_extract(payload_json, '$.shareId'), ''),
            'shareName', COALESCE(json_extract(payload_json, '$.shareName'), ''),
            'appType', json_extract(payload_json, '$.appType'),
            'subdomain', COALESCE(json_extract(payload_json, '$.subdomain'), ''),
            'ownerEmail', json_extract(payload_json, '$.ownerEmail'),
            'supplierEmail', COALESCE(
                json_extract(payload_json, '$.supplierEmail'),
                CASE WHEN json_extract(payload_json, '$.marketKind') IN ('client', 'client_host')
                     THEN json_extract(payload_json, '$.providerEmail') END
            ),
            'clientLabel', json_extract(payload_json, '$.clientLabel'),
            'providerEmail', json_extract(payload_json, '$.providerEmail'),
            'hostname', json_extract(payload_json, '$.hostname'),
            'status', json_extract(payload_json, '$.status'),
            'dailyRateMinor', json_extract(payload_json, '$.dailyRateMinor'),
            'currency', json_extract(payload_json, '$.currency'),
            'offerRevision', json_extract(payload_json, '$.offerRevision'),
            'trialHours', json_extract(payload_json, '$.trialHours'),
            'freeDurationDays', json_extract(payload_json, '$.freeDurationDays'),
            'activatedAt', json_extract(payload_json, '$.activatedAt'),
            'expiresAt', json_extract(payload_json, '$.expiresAt'),
            'reason', json_extract(payload_json, '$.reason'),
            'failureCode', json_extract(payload_json, '$.failureCode'),
            'paymentMethodKinds', json(COALESCE(
                json_extract(payload_json, '$.paymentMethodKinds'),
                (SELECT json_group_array(json_extract(method.value, '$.kind'))
                 FROM json_each(json_extract(payload_json, '$.paymentMethods')) AS method
                 WHERE json_type(method.value, '$.kind') = 'text'),
                '[]'
            )),
            'contacts', json(COALESCE(
                json_extract(payload_json, '$.contacts'),
                json_extract(payload_json, '$.paymentContacts'),
                '[]'
            ))
        ),
        json_patch(
            CASE WHEN json_extract(payload_json, '$.marketKind') = 'share' THEN
                json_object(
                    'listingId', json_extract(payload_json, '$.listingId'),
                    'seatId', json_extract(payload_json, '$.seatId'),
                    'seatPosition', json_extract(payload_json, '$.seatPosition'),
                    'seatStatus', json_extract(payload_json, '$.seatStatus'),
                    'subscriptionStatus', json_extract(payload_json, '$.subscriptionStatus'),
                    'parallelLimit', json_extract(payload_json, '$.parallelLimit'),
                    'tokenLimit', json_extract(payload_json, '$.tokenLimit'),
                    'tokenPeriod', json_extract(payload_json, '$.tokenPeriod'),
                    'dailyRateMinor', json_extract(payload_json, '$.dailyRateMinor'),
                    'currency', json_extract(payload_json, '$.currency'),
                    'serviceDurationDays', json_extract(payload_json, '$.serviceDurationDays'),
                    'offerRevision', json_extract(payload_json, '$.offerRevision')
                )
            ELSE json('{}') END,
            CASE json_type(payload_json, '$.providerDeniedClientAccess')
                WHEN 'true' THEN '{"providerDeniedClientAccess":true}'
                WHEN 'false' THEN '{"providerDeniedClientAccess":false}'
                WHEN NULL THEN '{}'
                ELSE json_object(
                    'providerDeniedClientAccess',
                    json_extract(payload_json, '$.providerDeniedClientAccess')
                )
            END
        )
    ),
        CASE WHEN json_type(payload_json, '$.billingEventType') IS NULL
             THEN '$.billingEventType' ELSE '$.__keep' END,
        CASE WHEN json_type(payload_json, '$.shareId') IS NULL
             THEN '$.shareId' ELSE '$.__keep' END,
        CASE WHEN json_type(payload_json, '$.shareName') IS NULL
             THEN '$.shareName' ELSE '$.__keep' END,
        CASE WHEN json_type(payload_json, '$.appType') IS NULL
             THEN '$.appType' ELSE '$.__keep' END,
        CASE WHEN json_type(payload_json, '$.subdomain') IS NULL
             THEN '$.subdomain' ELSE '$.__keep' END,
        CASE WHEN json_type(payload_json, '$.ownerEmail') IS NULL
             THEN '$.ownerEmail' ELSE '$.__keep' END,
        CASE WHEN json_type(payload_json, '$.supplierEmail') IS NULL
                       AND NOT (
                           json_extract(payload_json, '$.marketKind') IN ('client', 'client_host')
                           AND json_type(payload_json, '$.providerEmail') IS NOT NULL
                       )
             THEN '$.supplierEmail' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.clientLabel') IS NULL
             THEN '$.clientLabel' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.providerEmail') IS NULL
             THEN '$.providerEmail' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.hostname') IS NULL
             THEN '$.hostname' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.status') IS NULL
             THEN '$.status' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('share', 'client', 'client_host')
                       OR json_type(payload_json, '$.dailyRateMinor') IS NULL
             THEN '$.dailyRateMinor' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('share', 'client', 'client_host')
                       OR json_type(payload_json, '$.currency') IS NULL
             THEN '$.currency' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('share', 'client', 'client_host')
                       OR json_type(payload_json, '$.offerRevision') IS NULL
             THEN '$.offerRevision' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.trialHours') IS NULL
             THEN '$.trialHours' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.freeDurationDays') IS NULL
             THEN '$.freeDurationDays' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.activatedAt') IS NULL
             THEN '$.activatedAt' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.expiresAt') IS NULL
             THEN '$.expiresAt' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.providerDeniedClientAccess') IS NULL
             THEN '$.providerDeniedClientAccess' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.reason') IS NULL
             THEN '$.reason' ELSE '$.__keep' END
        ,CASE WHEN json_extract(payload_json, '$.marketKind') NOT IN ('client', 'client_host')
                       OR json_type(payload_json, '$.failureCode') IS NULL
             THEN '$.failureCode' ELSE '$.__keep' END
    )
WHERE json_valid(payload_json)
  AND source_kind IN ('share_market', 'market_billing', 'client_market');
