-- Emergency controls pause new wallet obligations without changing the
-- permanently approved payment rail or interrupting existing settlement.
ALTER TABLE market_finance_control ADD COLUMN new_orders_paused INTEGER NOT NULL DEFAULT 0
    CHECK (new_orders_paused IN (0, 1));
ALTER TABLE market_finance_control ADD COLUMN new_withdrawals_paused INTEGER NOT NULL DEFAULT 0
    CHECK (new_withdrawals_paused IN (0, 1));
