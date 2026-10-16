-- Wallet contracts have a stable, buyer-scoped funding tie breaker. Existing
-- contracts start neutral; changes to priority never alter a captured hold.
ALTER TABLE market_platform_contracts
    ADD COLUMN renewal_priority INTEGER NOT NULL DEFAULT 0 CHECK (renewal_priority >= 0);

CREATE INDEX idx_market_delivery_slices_contract_time
    ON market_delivery_slices(contract_id, ended_at, id);

-- Paid supply published before wallet cutover is never silently promoted to
-- the new payment rail. A new paid offer write is an explicit republication;
-- state-only release of an old occupied seat/Host leaves its old rail intact.
ALTER TABLE share_market_seats ADD COLUMN funding_rail TEXT NOT NULL
    DEFAULT 'legacy_provider' CHECK (funding_rail IN ('legacy_provider', 'platform_wallet'));
ALTER TABLE router_ssh_hosts ADD COLUMN funding_rail TEXT NOT NULL
    DEFAULT 'legacy_provider' CHECK (funding_rail IN ('legacy_provider', 'platform_wallet'));

CREATE TRIGGER trg_share_seat_funding_rail_insert
AFTER INSERT ON share_market_seats
WHEN NEW.daily_rate_minor IS NOT NULL OR NEW.cycle_price_minor IS NOT NULL
BEGIN
    UPDATE share_market_seats SET funding_rail =
        CASE WHEN (SELECT mode FROM market_finance_control WHERE id = 1) = 'enabled'
             THEN 'platform_wallet' ELSE 'legacy_provider' END
    WHERE id = NEW.id;
END;
CREATE TRIGGER trg_share_seat_funding_rail_republish
AFTER UPDATE OF daily_rate_minor, cycle_price_minor ON share_market_seats
BEGIN
    UPDATE share_market_seats SET funding_rail =
        CASE WHEN (SELECT mode FROM market_finance_control WHERE id = 1) = 'enabled'
             THEN 'platform_wallet' ELSE 'legacy_provider' END
    WHERE id = NEW.id;
END;

CREATE TRIGGER trg_client_host_funding_rail_insert
AFTER INSERT ON router_ssh_hosts
WHEN NEW.daily_rate_minor IS NOT NULL OR NEW.cycle_price_minor IS NOT NULL
BEGIN
    UPDATE router_ssh_hosts SET funding_rail =
        CASE WHEN (SELECT mode FROM market_finance_control WHERE id = 1) = 'enabled'
             THEN 'platform_wallet' ELSE 'legacy_provider' END
    WHERE id = NEW.id;
END;
CREATE TRIGGER trg_client_host_funding_rail_republish
AFTER UPDATE OF daily_rate_minor, cycle_price_minor ON router_ssh_hosts
BEGIN
    UPDATE router_ssh_hosts SET funding_rail =
        CASE WHEN (SELECT mode FROM market_finance_control WHERE id = 1) = 'enabled'
             THEN 'platform_wallet' ELSE 'legacy_provider' END
    WHERE id = NEW.id;
END;
