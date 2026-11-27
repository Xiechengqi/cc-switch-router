-- Client Market seller controls and trial-only rentals.
--
-- * A seller may reclaim a rented Client: with a notice period by default, or
--   immediately for a tenant violation. The reclaim settles as the seller's
--   responsibility, so unused rent returns in full and nothing is awarded.
-- * A listing may carry a short public title and may pause new orders without
--   affecting the current rental.
-- * Trial-only checkout, previously Share-only, also applies to Client rentals.

ALTER TABLE online_client_leases ADD COLUMN reclaim_requested_at INTEGER;
ALTER TABLE online_client_leases ADD COLUMN reclaim_effective_at INTEGER;
ALTER TABLE online_client_leases ADD COLUMN reclaim_mode TEXT
    CHECK (reclaim_mode IS NULL OR reclaim_mode IN ('notice', 'immediate'));
ALTER TABLE online_client_leases ADD COLUMN reclaim_reason TEXT
    CHECK (reclaim_reason IS NULL OR reclaim_reason IN ('need_machine', 'tenant_violation', 'other'));
-- Up to 2000 characters, matching the dialog's counter.
ALTER TABLE online_client_leases ADD COLUMN reclaim_note TEXT NOT NULL DEFAULT ''
    CHECK (length(reclaim_note) <= 2000);
ALTER TABLE online_client_leases ADD COLUMN reclaim_deny_buyer INTEGER NOT NULL DEFAULT 0
    CHECK (reclaim_deny_buyer IN (0, 1));
ALTER TABLE online_client_leases ADD COLUMN reclaim_unlist_after INTEGER NOT NULL DEFAULT 0
    CHECK (reclaim_unlist_after IN (0, 1));
ALTER TABLE online_client_leases ADD COLUMN reclaim_cancelled_at INTEGER;
-- The tenant had automatic renewal on when the reclaim switched it off; a
-- cancelled reclaim restores it, since the tenant never chose to stop.
ALTER TABLE online_client_leases ADD COLUMN reclaim_restore_auto_renew INTEGER NOT NULL DEFAULT 0
    CHECK (reclaim_restore_auto_renew IN (0, 1));
-- Likewise, a trial set to continue as paid becomes trial-only while a reclaim
-- is pending (the tenant is never charged a month the seller already ended),
-- and goes back to continuing if the reclaim is cancelled.
ALTER TABLE online_client_leases ADD COLUMN reclaim_restore_continue INTEGER NOT NULL DEFAULT 0
    CHECK (reclaim_restore_continue IN (0, 1));
CREATE INDEX online_client_reclaim_due ON online_client_leases(reclaim_effective_at)
    WHERE reclaim_effective_at IS NOT NULL AND reclaim_cancelled_at IS NULL AND released_at IS NULL;

-- Only an unfinished lease can be scheduled. An immediate reclaim must be a
-- documented tenant violation, so it stays reviewable by the buyer and Owner.
CREATE TRIGGER online_client_reclaim_rules
BEFORE UPDATE OF reclaim_effective_at, reclaim_mode, reclaim_reason, reclaim_note, reclaim_cancelled_at
ON online_client_leases
WHEN NEW.reclaim_effective_at IS NOT NULL AND NEW.reclaim_cancelled_at IS NULL
 AND (OLD.state NOT IN ('starting', 'active') OR NEW.reclaim_requested_at IS NULL
      OR NEW.reclaim_mode IS NULL OR NEW.reclaim_reason IS NULL
      OR (NEW.reclaim_mode = 'immediate'
          AND (NEW.reclaim_reason != 'tenant_violation' OR trim(NEW.reclaim_note) = '')))
BEGIN SELECT RAISE(ABORT, 'invalid Client reclaim'); END;

ALTER TABLE online_client_listings ADD COLUMN title TEXT
    CHECK (title IS NULL OR (length(title) BETWEEN 1 AND 30 AND title = trim(title)
        AND instr(title, char(10)) = 0 AND instr(title, char(13)) = 0 AND instr(title, char(9)) = 0));
ALTER TABLE online_client_listings ADD COLUMN paused_at INTEGER;
-- Last hourly re-check of the real egress path, so a tenant learns when the
-- IP they evaluated changes during the rental.
ALTER TABLE online_client_listings ADD COLUMN egress_checked_at INTEGER;

-- Same availability rules as 0130, plus paused listings. An occupied listing
-- reports its occupation first; pausing only stops the next order.
DROP VIEW online_client_supply;
CREATE VIEW online_client_supply AS
WITH snapshot AS (
    SELECT l.*,
           (r.id IS NOT NULL OR EXISTS (
               SELECT 1 FROM client_market_terminations t
               JOIN online_client_leases prior ON prior.id = t.installation_id
               WHERE t.host_id = l.installation_id AND t.status = 'finance_pending'
                 AND prior.installation_id = l.installation_id
           )) AS occupied,
           coalesce(l.observed_at BETWEEN unixepoch() - 600 AND unixepoch(), 0) AS probe_fresh,
           (i.market_lease_v1 = 1) AS lease_capable
    FROM online_client_listings l JOIN installations i ON i.id = l.installation_id
    LEFT JOIN online_client_leases r ON r.installation_id = l.installation_id AND r.released_at IS NULL
), probes AS (
    SELECT snapshot.*, (lease_capable AND probe_fresh AND healthy = 1) AS probe_healthy
    FROM snapshot
), inventory AS (
    SELECT probes.*,
           CASE WHEN state != 'listed' THEN state
                WHEN occupied THEN 'occupied'
                WHEN paused_at IS NOT NULL THEN 'paused'
                WHEN probe_fresh AND probe_error = 'tunnel_offline' THEN 'offline'
                WHEN probe_fresh AND probe_error = 'upgrading' THEN 'upgrading'
                WHEN probe_fresh AND probe_error = 'switching' THEN 'switching'
                WHEN NOT lease_capable OR NOT exclusive_management THEN 'upgrade_required'
                WHEN probe_healthy THEN 'available'
                ELSE 'unavailable' END AS availability
    FROM probes
)
SELECT inventory.*,
       CASE availability WHEN 'available' THEN 'idle' WHEN 'occupied' THEN 'allocated'
            ELSE 'maintaining' END AS supply_status
FROM inventory;

-- Trial-only rentals: the same contract field as Share (0122), now for Client too.
DROP TRIGGER trg_platform_contract_after_trial_kind;
CREATE TRIGGER trg_platform_contract_after_trial_kind
BEFORE INSERT ON market_platform_contracts
WHEN NEW.after_trial = 'end'
 AND (NEW.product_kind NOT IN ('share', 'client_host') OR NEW.trial_seconds_remaining <= 0)
BEGIN SELECT RAISE(ABORT, 'trial-only applies to Share and Client trials'); END;
DROP TRIGGER trg_platform_contract_after_trial_update;
CREATE TRIGGER trg_platform_contract_after_trial_update
BEFORE UPDATE OF after_trial ON market_platform_contracts
WHEN NEW.after_trial != OLD.after_trial
  AND (OLD.product_kind NOT IN ('share', 'client_host')
       OR OLD.status NOT IN ('pending_activation', 'trialing')
       OR OLD.paid_started_at IS NOT NULL)
BEGIN SELECT RAISE(ABORT, 'after-trial choice is fixed once the trial ends'); END;
