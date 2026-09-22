-- Calendar-month prepaid billing is intentionally additive. Existing paid
-- offers and service contracts keep their metered-daily meaning until a
-- Provider explicitly republishes them with a monthly price.

ALTER TABLE router_ssh_hosts ADD COLUMN pricing_model TEXT NOT NULL DEFAULT 'free'
    CHECK (pricing_model IN ('free', 'legacy_metered_daily', 'prepaid_calendar_month'));
ALTER TABLE router_ssh_hosts ADD COLUMN cycle_price_minor INTEGER
    CHECK (cycle_price_minor IS NULL OR cycle_price_minor > 0);
ALTER TABLE router_ssh_hosts ADD COLUMN billing_interval TEXT
    CHECK (billing_interval IS NULL OR billing_interval = 'calendar_month');
UPDATE router_ssh_hosts
SET pricing_model = CASE WHEN daily_rate_minor IS NULL THEN 'free' ELSE 'legacy_metered_daily' END;

ALTER TABLE client_market_allocation_quote_items ADD COLUMN pricing_model TEXT NOT NULL DEFAULT 'free'
    CHECK (pricing_model IN ('free', 'legacy_metered_daily', 'prepaid_calendar_month'));
ALTER TABLE client_market_allocation_quote_items ADD COLUMN cycle_price_minor INTEGER
    CHECK (cycle_price_minor IS NULL OR cycle_price_minor > 0);
ALTER TABLE client_market_allocation_quote_items ADD COLUMN billing_interval TEXT
    CHECK (billing_interval IS NULL OR billing_interval = 'calendar_month');
UPDATE client_market_allocation_quote_items
SET pricing_model = CASE WHEN daily_rate_minor IS NULL THEN 'free' ELSE 'legacy_metered_daily' END;

ALTER TABLE client_market_subscriptions ADD COLUMN pricing_model TEXT NOT NULL DEFAULT 'free'
    CHECK (pricing_model IN ('free', 'legacy_metered_daily', 'prepaid_calendar_month'));
ALTER TABLE client_market_subscriptions ADD COLUMN cycle_price_minor INTEGER
    CHECK (cycle_price_minor IS NULL OR cycle_price_minor > 0);
ALTER TABLE client_market_subscriptions ADD COLUMN billing_interval TEXT
    CHECK (billing_interval IS NULL OR billing_interval = 'calendar_month');
ALTER TABLE client_market_subscriptions ADD COLUMN recurring_contract_id TEXT;
UPDATE client_market_subscriptions
SET pricing_model = CASE WHEN daily_rate_minor IS NULL THEN 'free' ELSE 'legacy_metered_daily' END;
CREATE INDEX idx_client_market_recurring_contract
    ON client_market_subscriptions(recurring_contract_id)
    WHERE recurring_contract_id IS NOT NULL;

ALTER TABLE share_market_seats ADD COLUMN pricing_model TEXT NOT NULL DEFAULT 'free'
    CHECK (pricing_model IN ('free', 'legacy_metered_daily', 'prepaid_calendar_month'));
ALTER TABLE share_market_seats ADD COLUMN cycle_price_minor INTEGER
    CHECK (cycle_price_minor IS NULL OR cycle_price_minor > 0);
ALTER TABLE share_market_seats ADD COLUMN billing_interval TEXT
    CHECK (billing_interval IS NULL OR billing_interval = 'calendar_month');
UPDATE share_market_seats
SET pricing_model = CASE WHEN daily_rate_minor IS NULL THEN 'free' ELSE 'legacy_metered_daily' END;

ALTER TABLE share_market_rent_quotes ADD COLUMN pricing_model TEXT NOT NULL DEFAULT 'free'
    CHECK (pricing_model IN ('free', 'legacy_metered_daily', 'prepaid_calendar_month'));
ALTER TABLE share_market_rent_quotes ADD COLUMN cycle_price_minor INTEGER
    CHECK (cycle_price_minor IS NULL OR cycle_price_minor > 0);
ALTER TABLE share_market_rent_quotes ADD COLUMN billing_interval TEXT
    CHECK (billing_interval IS NULL OR billing_interval = 'calendar_month');
UPDATE share_market_rent_quotes
SET pricing_model = CASE
    WHEN json_valid(snapshot_json) THEN CASE
        WHEN json_extract(snapshot_json, '$.dailyRateMinor') IS NULL THEN 'free'
        ELSE 'legacy_metered_daily'
    END
    ELSE 'free'
END;
-- Keep quotes issued immediately before the upgrade usable for their short
-- remaining TTL. New readers require the explicit pricing model in the frozen
-- snapshot, while malformed historical snapshots must not block migration.
UPDATE share_market_rent_quotes
SET snapshot_json = json_set(
    snapshot_json,
    '$.pricingModel',
    CASE WHEN json_extract(snapshot_json, '$.dailyRateMinor') IS NULL
         THEN 'free' ELSE 'legacy_metered_daily' END
)
WHERE json_valid(snapshot_json)
  AND COALESCE(json_type(snapshot_json, '$.pricingModel'), 'null') = 'null';

ALTER TABLE share_market_subscriptions ADD COLUMN pricing_model TEXT NOT NULL DEFAULT 'free'
    CHECK (pricing_model IN ('free', 'legacy_metered_daily', 'prepaid_calendar_month'));
ALTER TABLE share_market_subscriptions ADD COLUMN cycle_price_minor INTEGER
    CHECK (cycle_price_minor IS NULL OR cycle_price_minor > 0);
ALTER TABLE share_market_subscriptions ADD COLUMN billing_interval TEXT
    CHECK (billing_interval IS NULL OR billing_interval = 'calendar_month');
ALTER TABLE share_market_subscriptions ADD COLUMN recurring_contract_id TEXT;
UPDATE share_market_subscriptions
SET pricing_model = CASE WHEN daily_rate_minor IS NULL THEN 'free' ELSE 'legacy_metered_daily' END;
CREATE INDEX idx_share_market_recurring_contract
    ON share_market_subscriptions(recurring_contract_id)
    WHERE recurring_contract_id IS NOT NULL;

-- Access requests freeze enough offer metadata for Providers to distinguish
-- prepaid monthly access from legacy daily access. Both are still the same
-- coarse `paid` permission scope, but only the legacy path can require credit.
ALTER TABLE market_access_requests ADD COLUMN pricing_model TEXT NOT NULL DEFAULT 'free'
    CHECK (pricing_model IN ('free', 'legacy_metered_daily', 'prepaid_calendar_month'));
ALTER TABLE market_access_requests ADD COLUMN cycle_price_minor INTEGER
    CHECK (cycle_price_minor IS NULL OR cycle_price_minor > 0);
ALTER TABLE market_access_requests ADD COLUMN billing_interval TEXT
    CHECK (billing_interval IS NULL OR billing_interval = 'calendar_month');
UPDATE market_access_requests
SET pricing_model = CASE WHEN daily_rate_minor IS NULL THEN 'free' ELSE 'legacy_metered_daily' END;

CREATE TABLE market_recurring_contracts (
    id TEXT PRIMARY KEY,
    prepaid_account_id TEXT NOT NULL,
    product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
    product_ref TEXT NOT NULL,
    activation_ref TEXT NOT NULL,
    prepare_fingerprint TEXT NOT NULL,
    service_ref TEXT NOT NULL,
    service_label TEXT NOT NULL,
    buyer_user_id TEXT NOT NULL,
    buyer_email TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    supplier_email TEXT NOT NULL,
    currency TEXT NOT NULL CHECK (currency = 'USD'),
    pricing_model TEXT NOT NULL CHECK (pricing_model = 'prepaid_calendar_month'),
    billing_interval TEXT NOT NULL CHECK (billing_interval = 'calendar_month'),
    cycle_price_minor INTEGER NOT NULL CHECK (cycle_price_minor > 0),
    offer_revision INTEGER NOT NULL CHECK (offer_revision > 0),
    status TEXT NOT NULL CHECK (status IN (
        'pending_activation', 'trial', 'active', 'recovery',
        'ended', 'activation_failed'
    )),
    renewal_policy TEXT NOT NULL CHECK (renewal_policy IN ('manual', 'automatic')),
    renewal_status TEXT NOT NULL CHECK (renewal_status IN (
        'initial_funded', 'funded', 'funding_required',
        'cancel_at_period_end', 'ended'
    )),
    auto_renew_max_price_minor INTEGER
        CHECK (auto_renew_max_price_minor IS NULL OR auto_renew_max_price_minor > 0),
    renewal_priority INTEGER NOT NULL DEFAULT 0,
    anchor_day INTEGER CHECK (anchor_day IS NULL OR anchor_day BETWEEN 1 AND 31),
    anchor_at TEXT,
    current_period_sequence INTEGER NOT NULL DEFAULT 0 CHECK (current_period_sequence >= 0),
    current_period_start TEXT,
    current_period_end TEXT,
    trial_allowance_seconds INTEGER NOT NULL DEFAULT 0 CHECK (trial_allowance_seconds >= 0),
    trial_seconds_remaining INTEGER NOT NULL DEFAULT 0
        CHECK (trial_seconds_remaining >= 0
               AND trial_seconds_remaining <= trial_allowance_seconds),
    trial_last_evaluated_at TEXT,
    trial_health_state TEXT NOT NULL DEFAULT 'unknown'
        CHECK (trial_health_state IN ('unknown', 'healthy', 'unhealthy')),
    trial_ends_at TEXT,
    trial_deadline_at TEXT,
    cancel_at_period_end INTEGER NOT NULL DEFAULT 0 CHECK (cancel_at_period_end IN (0, 1)),
    recovery_deadline TEXT,
    desired_control_state TEXT NOT NULL DEFAULT 'active'
        CHECK (desired_control_state IN ('active', 'suspended', 'terminated')),
    applied_control_state TEXT NOT NULL DEFAULT 'active'
        CHECK (applied_control_state IN ('active', 'suspended', 'terminated')),
    control_error TEXT,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    activated_at TEXT,
    ended_at TEXT,
    end_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(prepaid_account_id) REFERENCES market_prepaid_accounts(id)
);

CREATE UNIQUE INDEX uq_market_recurring_active_product
    ON market_recurring_contracts(product_kind, product_ref)
    WHERE status NOT IN ('ended', 'activation_failed');
CREATE UNIQUE INDEX uq_market_recurring_active_activation
    ON market_recurring_contracts(product_kind, activation_ref)
    WHERE status NOT IN ('ended', 'activation_failed');
CREATE INDEX idx_market_recurring_buyer
    ON market_recurring_contracts(buyer_user_id, status, updated_at DESC);
CREATE INDEX idx_market_recurring_supplier
    ON market_recurring_contracts(supplier_user_id, status, updated_at DESC);
CREATE INDEX idx_market_recurring_reconcile
    ON market_recurring_contracts(
        status, trial_last_evaluated_at, current_period_end, recovery_deadline
    );

CREATE TABLE market_recurring_periods (
    id TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    period_start TEXT,
    period_end TEXT,
    amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
    offer_revision INTEGER NOT NULL CHECK (offer_revision > 0),
    status TEXT NOT NULL CHECK (status IN (
        'reserved', 'paid', 'forfeited', 'partially_refunded',
        'refunded', 'failed'
    )),
    debit_ledger_entry_id TEXT,
    refunded_units INTEGER NOT NULL DEFAULT 0 CHECK (refunded_units >= 0),
    idempotency_key TEXT NOT NULL UNIQUE,
    paid_at TEXT,
    failed_at TEXT,
    failure_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(contract_id, sequence),
    FOREIGN KEY(contract_id) REFERENCES market_recurring_contracts(id),
    FOREIGN KEY(debit_ledger_entry_id) REFERENCES market_prepaid_ledger_entries(id)
);
CREATE INDEX idx_market_recurring_period_contract
    ON market_recurring_periods(contract_id, sequence DESC);

CREATE TABLE market_recurring_holds (
    id TEXT PRIMARY KEY,
    prepaid_account_id TEXT NOT NULL,
    contract_id TEXT NOT NULL,
    period_sequence INTEGER NOT NULL CHECK (period_sequence > 0),
    purpose TEXT NOT NULL CHECK (purpose IN (
        'initial_activation', 'manual_renewal', 'automatic_renewal'
    )),
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    status TEXT NOT NULL CHECK (status IN ('active', 'captured', 'released')),
    idempotency_key TEXT NOT NULL UNIQUE,
    release_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    captured_at TEXT,
    released_at TEXT,
    FOREIGN KEY(prepaid_account_id) REFERENCES market_prepaid_accounts(id),
    FOREIGN KEY(contract_id) REFERENCES market_recurring_contracts(id)
);
CREATE UNIQUE INDEX uq_market_recurring_live_hold
    ON market_recurring_holds(contract_id, period_sequence)
    WHERE status = 'active';
CREATE INDEX idx_market_recurring_hold_account
    ON market_recurring_holds(prepaid_account_id, status, created_at);

-- Trial allowance is scarce per buyer/supplier/product/service. A quote only
-- previews it; committing a recurring rental atomically claims the allowance
-- so concurrent rentals cannot each receive the same remaining trial.
CREATE TABLE market_recurring_trial_claims (
    contract_id TEXT PRIMARY KEY,
    buyer_user_id TEXT NOT NULL,
    supplier_user_id TEXT NOT NULL,
    product_kind TEXT NOT NULL,
    service_ref TEXT NOT NULL,
    currency TEXT NOT NULL CHECK (currency = 'USD'),
    claimed_seconds INTEGER NOT NULL CHECK (claimed_seconds > 0),
    status TEXT NOT NULL CHECK (status IN ('reserved', 'active', 'settled', 'released')),
    activated_at TEXT,
    settled_seconds INTEGER NOT NULL DEFAULT 0 CHECK (settled_seconds >= 0),
    release_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    settled_at TEXT,
    FOREIGN KEY(contract_id) REFERENCES market_recurring_contracts(id)
);
CREATE INDEX idx_market_recurring_trial_scope
    ON market_recurring_trial_claims(
        buyer_user_id, supplier_user_id, product_kind, service_ref, currency, status
    );
