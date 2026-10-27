-- NULL inherits the current Router default for new quotes. Zero disables trials.
-- Existing quote/contract trial allowances remain immutable.
ALTER TABLE market_wallet_policy ADD COLUMN default_client_trial_hours INTEGER NOT NULL
    DEFAULT 12 CHECK (default_client_trial_hours BETWEEN 0 AND 720);
ALTER TABLE router_ssh_hosts ADD COLUMN trial_hours INTEGER
    CHECK (trial_hours IS NULL OR trial_hours BETWEEN 0 AND 720);

-- Old Client quotes used the fixed 12-hour allowance. New quotes freeze the
-- full offered allowance separately from the buyer's remaining trial time.
ALTER TABLE client_market_allocation_quote_items
    ADD COLUMN wallet_trial_allowance_seconds INTEGER NOT NULL DEFAULT 43200
    CHECK (wallet_trial_allowance_seconds BETWEEN 0 AND 2592000);
