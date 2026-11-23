-- Share trial-only rentals and seller-controlled early termination.
--
-- A seat may forbid voluntary early termination and may set its own
-- compensation rate (NULL follows the Router owner's platform default at quote
-- time). Both are frozen into the quote, contract and period terms, so a seller
-- change only affects new rentals. Existing rows keep today's behaviour.
ALTER TABLE share_market_seats ADD COLUMN early_termination TEXT NOT NULL DEFAULT 'allowed'
    CHECK (early_termination IN ('allowed', 'forbidden'));
ALTER TABLE share_market_seats ADD COLUMN termination_rate_bps INTEGER
    CHECK (termination_rate_bps IS NULL OR termination_rate_bps BETWEEN 0 AND 5000);

ALTER TABLE share_termination_quote_terms ADD COLUMN early_termination TEXT NOT NULL DEFAULT 'allowed'
    CHECK (early_termination IN ('allowed', 'forbidden'));
ALTER TABLE share_termination_quote_terms ADD COLUMN rate_source TEXT NOT NULL DEFAULT 'router'
    CHECK (rate_source IN ('router', 'seat'));
ALTER TABLE share_termination_contract_terms ADD COLUMN early_termination TEXT NOT NULL DEFAULT 'allowed'
    CHECK (early_termination IN ('allowed', 'forbidden'));
ALTER TABLE share_termination_contract_terms ADD COLUMN rate_source TEXT NOT NULL DEFAULT 'router'
    CHECK (rate_source IN ('router', 'seat'));
ALTER TABLE share_termination_period_terms ADD COLUMN early_termination TEXT NOT NULL DEFAULT 'allowed'
    CHECK (early_termination IN ('allowed', 'forbidden'));
ALTER TABLE share_termination_period_terms ADD COLUMN rate_source TEXT NOT NULL DEFAULT 'router'
    CHECK (rate_source IN ('router', 'seat'));

-- `end`: a Share trial-only rental. It needs no wallet balance and returns the
-- seat when the trial runs out instead of converting to paid service. The
-- buyer may switch while the trial is still running.
ALTER TABLE market_platform_contracts ADD COLUMN after_trial TEXT NOT NULL DEFAULT 'continue'
    CHECK (after_trial IN ('continue', 'end'));
CREATE TRIGGER trg_platform_contract_after_trial_kind
BEFORE INSERT ON market_platform_contracts
WHEN NEW.after_trial = 'end' AND (NEW.product_kind != 'share' OR NEW.trial_seconds_remaining <= 0)
BEGIN SELECT RAISE(ABORT, 'trial-only applies to Share trials'); END;
CREATE TRIGGER trg_platform_contract_after_trial_update
BEFORE UPDATE OF after_trial ON market_platform_contracts
WHEN NEW.after_trial != OLD.after_trial
  AND (OLD.product_kind != 'share' OR OLD.status NOT IN ('pending_activation', 'trialing')
       OR OLD.paid_started_at IS NOT NULL)
BEGIN SELECT RAISE(ABORT, 'after-trial choice is fixed once the trial ends'); END;
CREATE INDEX idx_platform_contracts_trial_only
    ON market_platform_contracts(buyer_user_id, status) WHERE after_trial = 'end';
