-- Repair monthly contracts whose managed trial conversion left the billing
-- cursor at the exhausted trial end while the first paid period starts at the
-- later paid activation acknowledged by Server. Such a contract cannot settle
-- (every segment must start at the period's settled cursor) and cannot accept a
-- termination. Only untouched first periods are repaired: nothing has been
-- settled yet, so moving the cursor forward changes no money. The skipped wait
-- is recorded as audit-only, unbilled evidence, matching new conversions.
INSERT OR IGNORE INTO market_platform_service_evidence
    (id, contract_id, source_kind, started_at, ended_at,
     observed_state, service_available, reason_code, created_at)
SELECT contract.id || ':trial-conversion-gap', contract.id,
       CASE contract.product_kind WHEN 'share' THEN 'share_control' ELSE 'client_host_health' END,
       contract.last_evaluated_at, contract.paid_started_at,
       'unknown', 0, 'trial_conversion_gap', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM market_platform_contracts contract
JOIN market_platform_periods period ON period.contract_id = contract.id
 AND period.sequence = contract.current_period_sequence
WHERE contract.billing_model = 'calendar_month'
  AND contract.status IN ('active', 'ending')
  AND contract.current_period_sequence = 1
  AND contract.last_evaluated_at IS NOT NULL
  AND contract.paid_started_at IS NOT NULL
  AND julianday(contract.last_evaluated_at) < julianday(contract.paid_started_at)
  AND period.status = 'open'
  AND period.period_start = contract.paid_started_at
  AND period.settled_through_at = period.period_start
  AND NOT EXISTS (SELECT 1 FROM market_platform_monthly_segments segment
                  WHERE segment.period_id = period.id);

UPDATE market_platform_contracts
SET last_evaluated_at = paid_started_at,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE billing_model = 'calendar_month'
  AND status IN ('active', 'ending')
  AND current_period_sequence = 1
  AND last_evaluated_at IS NOT NULL
  AND paid_started_at IS NOT NULL
  AND julianday(last_evaluated_at) < julianday(paid_started_at)
  AND EXISTS (
      SELECT 1 FROM market_platform_periods period
      WHERE period.contract_id = market_platform_contracts.id
        AND period.sequence = 1
        AND period.status = 'open'
        AND period.period_start = market_platform_contracts.paid_started_at
        AND period.settled_through_at = period.period_start
        AND NOT EXISTS (SELECT 1 FROM market_platform_monthly_segments segment
                        WHERE segment.period_id = period.id)
  );
