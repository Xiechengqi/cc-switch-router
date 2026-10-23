-- Keep historical journals and payout evidence immutable and readable. New
-- external payout obligations can no longer be created, even by stale code.
CREATE TRIGGER trg_market_retired_payout_request BEFORE INSERT ON market_payout_requests
BEGIN SELECT RAISE(ABORT, 'Provider payouts are retired'); END;
CREATE TRIGGER trg_market_retired_payout_destination BEFORE INSERT ON market_payout_destinations
BEGIN SELECT RAISE(ABORT, 'Provider payouts are retired'); END;
CREATE TRIGGER trg_market_retired_payout_challenge BEFORE INSERT ON market_payout_address_challenges
BEGIN SELECT RAISE(ABORT, 'Provider payouts are retired'); END;

ALTER TABLE market_earning_wallet_conversions ADD COLUMN automatic INTEGER NOT NULL DEFAULT 0 CHECK (automatic IN (0, 1));
-- Persistent round-robin cursor: a frozen account/contract must not starve
-- unrelated earnings, including after a process restart.
ALTER TABLE market_finance_control ADD COLUMN earning_settlement_cursor TEXT NOT NULL DEFAULT '';
-- The old manual conversion switch has no authority over automatic settlement.
UPDATE market_wallet_policy SET earnings_conversions_paused = 0;
