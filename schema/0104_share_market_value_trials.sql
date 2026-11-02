-- A Share Market seat's one-time trial budget follows the seat's quota unit:
-- Token seats keep trial_token_limit, USD-equivalent seats use a USD budget.
-- Amounts are canonical decimal strings checked in application code with
-- i128 arithmetic; SQL never sums them.

-- Router default USD trial budget (micros). '0' means time-only for USD seats.
ALTER TABLE market_trial_policy ADD COLUMN share_value_usd_micros TEXT NOT NULL
  DEFAULT '1000000'
  CHECK(length(share_value_usd_micros) BETWEEN 1 AND 16
    AND CAST(share_value_usd_micros AS INTEGER) <= 1000000000000000
    AND share_value_usd_micros NOT GLOB '*[^0-9]*'
    AND (share_value_usd_micros = '0' OR share_value_usd_micros NOT GLOB '0*'));

-- Seat override; NULL inherits the Router default and '0' is time-only. Only a
-- USD-equivalent seat may carry it, and never together with a Token trial cap.
ALTER TABLE share_market_seats ADD COLUMN trial_value_limit_usd_micros TEXT
  CHECK(trial_value_limit_usd_micros IS NULL OR (value_limit_usd_micros IS NOT NULL
    AND trial_token_limit IS NULL
    AND length(trial_value_limit_usd_micros) BETWEEN 1 AND 16
    AND CAST(trial_value_limit_usd_micros AS INTEGER) <= 1000000000000000
    AND trial_value_limit_usd_micros NOT GLOB '*[^0-9]*'
    AND (trial_value_limit_usd_micros = '0' OR trial_value_limit_usd_micros NOT GLOB '0*')));

-- The first trial contract of a scope fixes its unit. A later seat in the
-- other unit gets only the remaining trial time, never a second budget.
ALTER TABLE market_share_trial_scopes ADD COLUMN unit TEXT
  CHECK(unit IS NULL OR unit IN ('tokens', 'usd'));
-- Every existing trial contract was Token-denominated.
UPDATE market_share_trial_scopes SET unit = 'tokens'
  WHERE EXISTS (SELECT 1 FROM market_share_trial_contracts trial
                WHERE trial.scope_id = market_share_trial_scopes.id);

-- Frozen USD trial terms of one contract. NULL allowance marks a Token
-- contract; '0' a time-only USD contract. Used picos are Server-reported and
-- monotonic like tokens_used.
ALTER TABLE market_share_trial_contracts ADD COLUMN value_allowance_usd_micros TEXT
  CHECK(value_allowance_usd_micros IS NULL OR (
    token_allowance = 0 AND token_budget = 0
    AND length(value_allowance_usd_micros) BETWEEN 1 AND 16
    AND CAST(value_allowance_usd_micros AS INTEGER) <= 1000000000000000
    AND value_allowance_usd_micros NOT GLOB '*[^0-9]*'
    AND (value_allowance_usd_micros = '0' OR value_allowance_usd_micros NOT GLOB '0*')));
ALTER TABLE market_share_trial_contracts ADD COLUMN value_budget_usd_micros TEXT
  CHECK((value_budget_usd_micros IS NULL AND value_allowance_usd_micros IS NULL)
    OR (value_budget_usd_micros IS NOT NULL AND value_allowance_usd_micros IS NOT NULL
    AND length(value_budget_usd_micros) BETWEEN 1 AND 16
    AND CAST(value_budget_usd_micros AS INTEGER) <= CAST(value_allowance_usd_micros AS INTEGER)
    AND value_budget_usd_micros NOT GLOB '*[^0-9]*'
    AND (value_budget_usd_micros = '0' OR value_budget_usd_micros NOT GLOB '0*')));
ALTER TABLE market_share_trial_contracts ADD COLUMN value_used_picos TEXT NOT NULL DEFAULT '0'
  CHECK(length(value_used_picos) BETWEEN 1 AND 39
    AND (length(value_used_picos) < 39 OR value_used_picos <= '170141183460469231731687303715884105727')
    AND value_used_picos NOT GLOB '*[^0-9]*'
    AND (value_used_picos = '0' OR value_used_picos NOT GLOB '0*'));
