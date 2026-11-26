-- Keep the probe samples used for billing independent of the requirements
-- for a new rental. Removing or upgrading a listing must not rewrite them.
DROP VIEW online_client_supply;

-- Both authorization and the public inventory read the same signed capability flags.
ALTER TABLE online_client_listings ADD COLUMN exclusive_management INTEGER GENERATED ALWAYS AS (
    CASE WHEN json_valid(check_json) THEN json_type(check_json, '$.exclusiveManagement') IS 'true' ELSE 0 END
) VIRTUAL;
ALTER TABLE online_client_listings ADD COLUMN owner_rental INTEGER GENERATED ALWAYS AS (
    CASE WHEN json_valid(check_json) THEN json_type(check_json, '$.ownerRental') IS 'true' ELSE 0 END
) VIRTUAL;

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
