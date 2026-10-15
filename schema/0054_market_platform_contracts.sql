-- The platform-wallet contract rail is separate from legacy Provider funding.
-- No legacy service or balance is implicitly promoted into these tables.

-- The quote carries its own funding rail so a mode change cannot silently
-- reinterpret a legacy Provider-funded reservation as a wallet purchase.
ALTER TABLE client_market_allocation_quotes
    ADD COLUMN funding_rail TEXT NOT NULL DEFAULT 'legacy_provider'
    CHECK (funding_rail IN ('legacy_provider', 'platform_wallet'));

-- A multi-Host quote freezes each Host's remaining trial allowance. An item
-- with a trial has no purchase-time wallet hold, while other items may have one.
ALTER TABLE client_market_allocation_quote_items
    ADD COLUMN wallet_trial_seconds INTEGER NOT NULL DEFAULT 0
    CHECK (wallet_trial_seconds >= 0);

-- Private identity fence; the public Share quote continues to show only the
-- Provider profile, never the historical financial owner user ID.
ALTER TABLE share_market_rent_quotes ADD COLUMN provider_user_id TEXT;

CREATE TABLE market_platform_contracts (
    id TEXT PRIMARY KEY,
    product_kind TEXT NOT NULL CHECK (product_kind IN ('share', 'client_host')),
    product_ref TEXT NOT NULL,
    trial_scope_ref TEXT NOT NULL CHECK (trial_scope_ref != ''),
    service_ref TEXT,
    quote_id TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    buyer_user_id TEXT NOT NULL,
    provider_user_id TEXT NOT NULL,
    offer_revision INTEGER NOT NULL CHECK (offer_revision > 0),
    billing_model TEXT NOT NULL CHECK (billing_model IN ('metered', 'calendar_month')),
    daily_rate_minor INTEGER,
    cycle_price_minor INTEGER,
    renewal_policy TEXT NOT NULL CHECK (renewal_policy IN ('manual', 'automatic')),
    status TEXT NOT NULL CHECK (status IN (
        'pending_activation', 'trialing', 'active', 'suspended', 'ending', 'ended',
        'activation_failed', 'review_required'
    )),
    activation_hold_id TEXT REFERENCES market_wallet_holds(id),
    budget_hold_id TEXT REFERENCES market_wallet_holds(id),
    budget_sequence INTEGER NOT NULL DEFAULT 0 CHECK (budget_sequence >= 0),
    current_period_sequence INTEGER NOT NULL DEFAULT 0 CHECK (current_period_sequence >= 0),
    trial_seconds_remaining INTEGER NOT NULL DEFAULT 0 CHECK (trial_seconds_remaining >= 0),
    activated_at TEXT,
    paid_started_at TEXT,
    last_evaluated_at TEXT,
    ended_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (product_kind, product_ref),
    -- One Client quote can allocate several Hosts. Idempotency is per product,
    -- not per quote; product_kind/product_ref is unique above.
    CHECK (buyer_user_id != provider_user_id),
    CHECK ((billing_model = 'metered' AND daily_rate_minor > 0
            AND cycle_price_minor IS NULL AND renewal_policy = 'manual'
            AND ((budget_hold_id IS NULL AND budget_sequence = 0)
                 OR (budget_hold_id IS NOT NULL AND budget_sequence > 0)))
        OR (billing_model = 'calendar_month' AND cycle_price_minor > 0
            AND daily_rate_minor IS NULL AND budget_hold_id IS NULL
            AND budget_sequence = 0)),
    CHECK ((status = 'pending_activation' AND activated_at IS NULL)
        OR status != 'pending_activation')
);
CREATE INDEX idx_market_platform_contract_buyer
    ON market_platform_contracts(buyer_user_id, status, created_at DESC);
CREATE INDEX idx_market_platform_contract_provider
    ON market_platform_contracts(provider_user_id, status, created_at DESC);
CREATE UNIQUE INDEX uq_market_platform_contract_service
    ON market_platform_contracts(product_kind, service_ref)
    WHERE service_ref IS NOT NULL;

ALTER TABLE client_market_subscriptions
    ADD COLUMN platform_contract_id TEXT REFERENCES market_platform_contracts(id);
CREATE UNIQUE INDEX uq_market_platform_pending_trial_scope
    ON market_platform_contracts(
        buyer_user_id, provider_user_id, product_kind, trial_scope_ref
    ) WHERE trial_seconds_remaining > 0
        AND status IN ('pending_activation', 'trialing');

CREATE TABLE market_platform_trial_segments (
    id TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL REFERENCES market_platform_contracts(id),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('share_control', 'client_host_health')),
    source_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT NOT NULL,
    service_available INTEGER NOT NULL CHECK (service_available IN (0, 1)),
    consumed_seconds INTEGER NOT NULL CHECK (consumed_seconds >= 0),
    created_at TEXT NOT NULL,
    UNIQUE (contract_id, source_id)
);

CREATE TABLE market_platform_periods (
    id TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL REFERENCES market_platform_contracts(id),
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    period_start TEXT NOT NULL,
    period_end TEXT NOT NULL,
    settled_through_at TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    fulfilled_units INTEGER NOT NULL DEFAULT 0 CHECK (fulfilled_units >= 0),
    refunded_units INTEGER NOT NULL DEFAULT 0 CHECK (refunded_units >= 0),
    debit_journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    debited_at TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'closed')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (contract_id, sequence),
    CHECK (fulfilled_units + refunded_units <= amount_units)
);
CREATE INDEX idx_market_platform_period_open
    ON market_platform_periods(status, period_end, contract_id);

CREATE TABLE market_platform_monthly_segments (
    id TEXT PRIMARY KEY,
    period_id TEXT NOT NULL REFERENCES market_platform_periods(id),
    source_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT NOT NULL,
    service_available INTEGER NOT NULL CHECK (service_available IN (0, 1)),
    amount_units INTEGER NOT NULL CHECK (amount_units >= 0),
    journal_id TEXT REFERENCES market_journal_transactions(id),
    created_at TEXT NOT NULL,
    UNIQUE (period_id, source_id),
    CHECK ((amount_units = 0 AND journal_id IS NULL)
        OR (amount_units > 0 AND journal_id IS NOT NULL))
);

CREATE TABLE market_platform_metered_observations (
    id TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL REFERENCES market_platform_contracts(id),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('share_control', 'client_host_health')),
    source_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT NOT NULL,
    billable_seconds INTEGER NOT NULL CHECK (billable_seconds > 0),
    captured_seconds INTEGER NOT NULL DEFAULT 0 CHECK (captured_seconds >= 0),
    unfunded_seconds INTEGER NOT NULL DEFAULT 0 CHECK (unfunded_seconds >= 0),
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    created_at TEXT NOT NULL,
    UNIQUE (source_kind, source_id),
    CHECK (captured_seconds + unfunded_seconds <= billable_seconds)
);
CREATE INDEX idx_market_platform_metered_pending
    ON market_platform_metered_observations(contract_id, started_at, id);

CREATE TABLE market_platform_usage_batch_items (
    batch_id TEXT NOT NULL REFERENCES market_usage_posting_batches(id),
    observation_id TEXT NOT NULL REFERENCES market_platform_metered_observations(id),
    first_second INTEGER NOT NULL CHECK (first_second >= 0),
    seconds INTEGER NOT NULL CHECK (seconds > 0),
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    PRIMARY KEY (batch_id, observation_id)
);
