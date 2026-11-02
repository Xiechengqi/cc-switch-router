-- A seat may sell a USD-equivalent quota (API list-price usage, not money)
-- instead of a Token count. The two are mutually exclusive; the period stays
-- in token_period_json. The tariff revision published at rent time is kept
-- on the contract for audit only: enforcement follows the current tariff.
ALTER TABLE share_market_seats ADD COLUMN value_limit_usd_micros TEXT
  CHECK(value_limit_usd_micros IS NULL OR (token_limit IS NULL
    AND length(value_limit_usd_micros) BETWEEN 1 AND 16
    AND CAST(value_limit_usd_micros AS INTEGER) <= 1000000000000000
    AND value_limit_usd_micros NOT GLOB '*[^0-9]*'
    AND value_limit_usd_micros NOT GLOB '0*'));
ALTER TABLE share_market_subscriptions ADD COLUMN value_limit_usd_micros TEXT
  CHECK(value_limit_usd_micros IS NULL OR (token_limit IS NULL
    AND length(value_limit_usd_micros) BETWEEN 1 AND 16
    AND CAST(value_limit_usd_micros AS INTEGER) <= 1000000000000000
    AND value_limit_usd_micros NOT GLOB '*[^0-9]*'
    AND value_limit_usd_micros NOT GLOB '0*'));
ALTER TABLE share_market_subscriptions ADD COLUMN value_pricing_revision TEXT;
