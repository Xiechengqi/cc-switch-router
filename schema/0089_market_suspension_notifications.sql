-- A failed trial-to-paid retry used to leave and re-enter suspended every 5s.
-- Recover only the provable, never-paid trial case. Do not backdate a paid
-- service, a finance hold, an unknown event, or a service that really resumed.
-- Business payloads, frozen delivery envelopes and money records stay intact.
CREATE INDEX idx_market_notification_outbox_service
    ON market_notification_outbox(object_kind, object_id, event_kind, recipient_user_id, created_at);

CREATE TEMP TABLE market89_trial_suspensions AS
SELECT c.id,
       (SELECT n.created_at FROM market_notification_outbox n
        WHERE n.object_kind = 'platform_contract' AND n.object_id = c.id
          AND n.event_kind = 'contract_suspended'
        ORDER BY julianday(n.created_at), n.created_at, n.id LIMIT 1) AS started_at,
       (SELECT t.ended_at FROM market_platform_trial_segments t
        WHERE t.contract_id = c.id AND t.consumed_seconds > 0
        ORDER BY julianday(t.ended_at) DESC, t.ended_at DESC, t.id DESC LIMIT 1) AS trial_ended_at
FROM market_platform_contracts c
WHERE c.status = 'suspended' AND c.suspension_cause IS NULL
  AND c.activated_at IS NOT NULL AND c.paid_started_at IS NULL
  AND c.trial_seconds_remaining = 0 AND c.current_period_sequence = 0
  AND c.budget_sequence = 0 AND c.activation_hold_id IS NULL AND c.budget_hold_id IS NULL
  AND NOT EXISTS (SELECT 1 FROM market_platform_periods p WHERE p.contract_id = c.id)
  AND NOT EXISTS (SELECT 1 FROM market_wallet_holds h
                  WHERE h.source_kind = 'platform_contract' AND h.source_id = c.id)
  AND NOT EXISTS (SELECT 1 FROM market_notification_outbox n
                  WHERE n.object_kind = 'platform_contract' AND n.object_id = c.id
                    AND n.event_kind IN ('contract_resumed', 'contract_terminated'))
  AND (SELECT COUNT(*) FROM market_notification_outbox n
       WHERE n.object_kind = 'platform_contract' AND n.object_id = c.id
         AND n.event_kind = 'contract_suspended') > 1
  AND NOT EXISTS (
      SELECT 1 FROM market_notification_outbox n
      WHERE n.object_kind = 'platform_contract' AND n.object_id = c.id
        AND n.event_kind = 'contract_suspended'
        AND NOT (
            n.audience = 'buyer' AND n.recipient_user_id = c.buyer_user_id
            AND n.dedupe_key = 'contract_suspended:' || c.id || ':' || n.created_at
            AND COALESCE(json_extract(CASE WHEN json_valid(n.payload_json)
                THEN n.payload_json ELSE '{}' END, '$.contractId'), '') = c.id
            AND COALESCE(json_extract(CASE WHEN json_valid(n.payload_json)
                THEN n.payload_json ELSE '{}' END, '$.reason'), '') = 'insufficient_funds'
            AND COALESCE(json_extract(CASE WHEN json_valid(n.payload_json)
                THEN n.payload_json ELSE '{}' END, '$.status'), '') = 'suspended'
            AND COALESCE(julianday(n.created_at) >= julianday(c.activated_at), 0)
            AND COALESCE(julianday(n.created_at) <= julianday(c.suspended_at), 0)
        )
  )
  AND (SELECT MIN(julianday(n.created_at)) FROM market_notification_outbox n
       WHERE n.object_kind = 'platform_contract' AND n.object_id = c.id
         AND n.event_kind = 'contract_suspended') >=
      (SELECT MAX(julianday(t.ended_at)) FROM market_platform_trial_segments t
       WHERE t.contract_id = c.id AND t.consumed_seconds > 0);

UPDATE market_platform_contracts
SET suspended_at = (SELECT started_at FROM market89_trial_suspensions r WHERE r.id = market_platform_contracts.id),
    -- The old retry also pushed this buyer's due-funding priority backwards.
    -- Restore only a cursor later than the independently recorded trial end.
    last_evaluated_at = CASE
        WHEN julianday(last_evaluated_at) > (SELECT julianday(trial_ended_at)
            FROM market89_trial_suspensions r WHERE r.id = market_platform_contracts.id)
        THEN (SELECT trial_ended_at FROM market89_trial_suspensions r WHERE r.id = market_platform_contracts.id)
        ELSE last_evaluated_at END
WHERE id IN (SELECT id FROM market89_trial_suspensions);

-- Leave at most the original notice eligible for delivery. If any notice in
-- this proven continuous suspension was already sent (or might have been
-- accepted), do not send another. Normal current notices and unrelated money
-- receipts are not affected. Never mutate a historical sent/uncertain task.
CREATE TEMP TABLE market89_obsolete_notices AS
SELECT n.id
FROM market_notification_outbox n
JOIN market_platform_contracts c ON c.id = n.object_id AND c.buyer_user_id = n.recipient_user_id
WHERE n.object_kind = 'platform_contract' AND n.audience = 'buyer'
  AND n.event_kind IN ('contract_suspended', 'contract_terminating_soon')
  AND n.status IN ('queued', 'claimed')
  AND (
      c.status != 'suspended'
      OR n.dedupe_key != n.event_kind || ':' || c.id || ':' || c.suspended_at
      OR (
          n.event_kind = 'contract_suspended'
          AND c.id IN (SELECT id FROM market89_trial_suspensions)
          AND EXISTS (
              SELECT 1 FROM market_notification_outbox earlier
              WHERE earlier.object_kind = 'platform_contract' AND earlier.object_id = c.id
                AND earlier.recipient_user_id = c.buyer_user_id
                AND earlier.event_kind = 'contract_suspended'
                AND (earlier.status = 'sent' OR EXISTS (
                    SELECT 1 FROM market_notification_deliveries d WHERE d.event_id = earlier.id
                      AND (d.status IN ('sent', 'uncertain')
                           OR (d.status = 'claimed' AND d.first_attempt_at IS NOT NULL))
                ))
          )
      )
  );

UPDATE market_notification_deliveries
SET status = CASE WHEN status = 'claimed' AND first_attempt_at IS NOT NULL
                  THEN 'uncertain' ELSE 'suppressed' END,
    claim_token = NULL, claim_expires_at = NULL,
    last_error = 'obsolete_service_suspension_notice'
WHERE event_id IN (SELECT id FROM market89_obsolete_notices)
  AND status IN ('queued', 'claimed', 'blocked_config');

UPDATE market_notification_outbox
SET status = CASE
        WHEN EXISTS (SELECT 1 FROM market_notification_deliveries d
                     WHERE d.event_id = market_notification_outbox.id AND d.status = 'sent') THEN 'sent'
        WHEN EXISTS (SELECT 1 FROM market_notification_deliveries d
                     WHERE d.event_id = market_notification_outbox.id AND d.status = 'uncertain') THEN 'failed'
        ELSE 'suppressed' END,
    delivered_channels = COALESCE(
        (SELECT group_concat(d.channel || ':' || d.status, ',') FROM market_notification_deliveries d
         WHERE d.event_id = market_notification_outbox.id), delivered_channels),
    sent_at = COALESCE(sent_at,
        (SELECT MIN(d.sent_at) FROM market_notification_deliveries d
         WHERE d.event_id = market_notification_outbox.id AND d.status = 'sent')),
    claimed_by = NULL, claim_expires_at = NULL,
    last_error = 'obsolete_service_suspension_notice'
WHERE id IN (SELECT id FROM market89_obsolete_notices);

DROP TABLE market89_obsolete_notices;
DROP TABLE market89_trial_suspensions;
