-- Recharge availability never selects the marketplace's payment rail.
-- Existing publications/quotes/contracts stay immutable. Only new paid terms
-- (including explicit republication) use the wallet in every recharge mode.
DROP TRIGGER trg_share_seat_funding_rail_insert;
DROP TRIGGER trg_share_seat_funding_rail_republish;
DROP TRIGGER trg_client_host_funding_rail_insert;
DROP TRIGGER trg_client_host_funding_rail_republish;

CREATE TRIGGER trg_share_seat_funding_rail_insert
AFTER INSERT ON share_market_seats
WHEN NEW.daily_rate_minor IS NOT NULL OR NEW.cycle_price_minor IS NOT NULL
BEGIN
    UPDATE share_market_seats SET funding_rail =
        'platform_wallet'
    WHERE id = NEW.id;
END;
CREATE TRIGGER trg_share_seat_funding_rail_republish
AFTER UPDATE OF daily_rate_minor, cycle_price_minor ON share_market_seats
BEGIN
    UPDATE share_market_seats SET funding_rail =
        'platform_wallet'
    WHERE id = NEW.id;
END;

CREATE TRIGGER trg_client_host_funding_rail_insert
AFTER INSERT ON router_ssh_hosts
WHEN NEW.daily_rate_minor IS NOT NULL OR NEW.cycle_price_minor IS NOT NULL
BEGIN
    UPDATE router_ssh_hosts SET funding_rail =
        'platform_wallet'
    WHERE id = NEW.id;
END;
CREATE TRIGGER trg_client_host_funding_rail_republish
AFTER UPDATE OF daily_rate_minor, cycle_price_minor ON router_ssh_hosts
BEGIN
    UPDATE router_ssh_hosts SET funding_rail =
        'platform_wallet'
    WHERE id = NEW.id;
END;
