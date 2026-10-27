-- Trial defaults have an independent revision: changing them must not invalidate
-- maintenance-fee quotes. Existing explicit seat overrides are not rewritten.
CREATE TABLE market_trial_policy (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL DEFAULT 1,
    client_hours INTEGER NOT NULL CHECK (client_hours BETWEEN 0 AND 720),
    share_hours INTEGER NOT NULL CHECK (share_hours BETWEEN 0 AND 720),
    share_tokens INTEGER NOT NULL CHECK (share_tokens BETWEEN 0 AND 9007199254740991),
    updated_at TEXT NOT NULL
);
INSERT INTO market_trial_policy SELECT 1, 1, default_client_trial_hours, 12, 1000000, updated_at
    FROM market_wallet_policy WHERE id = 1;
ALTER TABLE market_wallet_policy DROP COLUMN default_client_trial_hours;
CREATE TABLE market_trial_policy_events (
    id TEXT PRIMARY KEY, actor_user_id TEXT NOT NULL, before_json TEXT NOT NULL,
    after_json TEXT NOT NULL, created_at TEXT NOT NULL
);
ALTER TABLE shares ADD COLUMN market_entitlements_json TEXT NOT NULL DEFAULT '{}';
CREATE TABLE market_share_trial_scopes (
    id TEXT PRIMARY KEY, buyer_user_id TEXT NOT NULL, provider_user_id TEXT NOT NULL,
    share_id TEXT NOT NULL, ended_reason TEXT, ended_at TEXT,
    UNIQUE (buyer_user_id, provider_user_id, share_id)
);
CREATE TABLE market_share_trial_contracts (
    contract_id TEXT PRIMARY KEY REFERENCES market_platform_contracts(id),
    scope_id TEXT NOT NULL REFERENCES market_share_trial_scopes(id),
    entitlement_id TEXT NOT NULL UNIQUE,
    token_allowance INTEGER NOT NULL CHECK (token_allowance >= 0),
    token_budget INTEGER NOT NULL CHECK (token_budget >= 0),
    tokens_used INTEGER NOT NULL DEFAULT 0 CHECK (tokens_used >= 0),
    usage_revision INTEGER NOT NULL DEFAULT 0 CHECK (usage_revision >= 0),
    phase TEXT NOT NULL DEFAULT 'trial' CHECK (phase IN ('trial', 'activating', 'paid', 'revoking')),
    end_reason TEXT, conversion_hold_id TEXT, conversion_requested_at TEXT, paid_applied_at TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX market_share_trial_scope_contracts ON market_share_trial_contracts(scope_id);
