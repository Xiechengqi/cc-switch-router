-- Persist only the last coarse alert state. Exact balances and runway remain
-- private billing data; this state is used to de-duplicate buyer reminders.
ALTER TABLE market_credit_accounts
    ADD COLUMN funding_runway_alert_level TEXT NOT NULL DEFAULT 'none'
        CHECK (funding_runway_alert_level IN ('none', 'warning', 'critical'));
