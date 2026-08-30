ALTER TABLE share_market_seats ADD COLUMN trial_hours INTEGER;
ALTER TABLE share_market_seats ADD COLUMN trial_token_limit INTEGER;
ALTER TABLE share_market_subscriptions ADD COLUMN trial_hours INTEGER;
ALTER TABLE share_market_subscriptions ADD COLUMN trial_token_limit INTEGER;

UPDATE share_market_seats
   SET trial_hours = 12
 WHERE daily_rate_minor IS NOT NULL
   AND trial_hours IS NULL;

UPDATE share_market_subscriptions
   SET trial_hours = 12
 WHERE daily_rate_minor IS NOT NULL
   AND trial_hours IS NULL;
