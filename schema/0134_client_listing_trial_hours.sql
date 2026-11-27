-- Client listing trials are set in hours, the same unit as Share seat trials
-- (at most 720 hours, the former 30-day cap). `trial_days` is retired: it is
-- converted once here and no longer read or written.
ALTER TABLE online_client_listings ADD COLUMN trial_hours INTEGER NOT NULL DEFAULT 0
    CHECK(trial_hours BETWEEN 0 AND 720);
UPDATE online_client_listings SET trial_hours = trial_days * 24, trial_days = 0 WHERE trial_days > 0;

-- Rebuilt unchanged from 0131 so the view's `l.*` includes `trial_hours`.
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
