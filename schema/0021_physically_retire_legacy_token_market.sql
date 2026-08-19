-- Final repository-side retirement of the former Router-local Token Market.
-- Migration 19 supplied a rollback archive; the Rust migration hook verifies
-- its checksum immediately before this migration executes.  Only usage rows
-- that still belong to a known Share and an identified Share user are retained
-- in the canonical Share request log.  A user is considered identified only
-- when the address is the current Share owner, appears in the canonical grant
-- history, or already exists in Router's users table.  Registry, pricing,
-- settlement, API-key and Market identity data are deliberately not carried
-- forward.

INSERT OR IGNORE INTO share_request_logs (
    request_id, installation_id, share_id, share_name, provider_id, provider_name,
    app_type, model, request_model, request_agent, requested_model, actual_model,
    actual_model_source, requested_reasoning_effort, effective_reasoning_effort,
    client_service_tier, effective_service_tier, service_tier_decision,
    usage_state, stream_status, usage_revision, status_code, latency_ms,
    first_token_ms, input_tokens, output_tokens, cache_read_tokens,
    cache_creation_tokens, quota_tokens, is_streaming, session_id, user_country,
    user_country_iso3, user_email, is_health_check, created_at
)
SELECT
    observation.request_id,
    share.installation_id,
    observation.share_id,
    share.share_name,
    COALESCE(
        NULLIF((
            SELECT binding.provider_id
              FROM share_bindings binding
             WHERE binding.share_id = observation.share_id
               AND binding.app_type = lower(trim(observation.request_agent))
             LIMIT 1
        ), ''),
        NULLIF(share.provider_id, ''),
        'retired-external-observation'
    ),
    COALESCE(
        NULLIF((
            SELECT binding.provider_id
              FROM share_bindings binding
             WHERE binding.share_id = observation.share_id
               AND binding.app_type = lower(trim(observation.request_agent))
             LIMIT 1
        ), ''),
        NULLIF(share.provider_id, ''),
        'retired external observation'
    ),
    CASE
        WHEN lower(trim(observation.request_agent)) IN ('claude', 'codex', 'gemini')
        THEN lower(trim(observation.request_agent))
        ELSE share.app_type
    END,
    COALESCE(
        NULLIF(trim(observation.model), ''),
        NULLIF(trim(observation.actual_model), ''),
        NULLIF(trim(observation.requested_model), ''),
        'unknown'
    ),
    COALESCE(
        NULLIF(trim(observation.requested_model), ''),
        NULLIF(trim(observation.model), ''),
        'unknown'
    ),
    COALESCE(observation.request_agent, ''),
    COALESCE(observation.requested_model, ''),
    COALESCE(observation.actual_model, ''),
    COALESCE(observation.actual_model_source, ''),
    NULL, NULL, NULL, NULL, NULL,
    'migrated', NULL, 0,
    MAX(COALESCE(observation.status_code, 0), 0),
    MAX(COALESCE(observation.latency_ms, 0), 0),
    NULL,
    CASE
        WHEN lower(trim(COALESCE(observation.request_agent, ''))) = 'codex'
        THEN MAX(
            COALESCE(observation.input_tokens, 0)
                - COALESCE(observation.cache_read_tokens, 0),
            0
        )
        ELSE MAX(COALESCE(observation.input_tokens, 0), 0)
    END,
    MAX(COALESCE(observation.output_tokens, 0), 0),
    MAX(COALESCE(observation.cache_read_tokens, 0), 0),
    MAX(COALESCE(observation.cache_creation_tokens, 0), 0),
    CASE
        WHEN lower(trim(COALESCE(observation.request_agent, ''))) = 'codex'
        THEN MAX(
                 COALESCE(observation.input_tokens, 0)
                     - COALESCE(observation.cache_read_tokens, 0),
                 0
             )
             + MAX(COALESCE(observation.output_tokens, 0), 0)
             + MAX(COALESCE(observation.cache_read_tokens, 0), 0)
             + MAX(COALESCE(observation.cache_creation_tokens, 0), 0)
        ELSE MAX(COALESCE(observation.input_tokens, 0), 0)
             + MAX(COALESCE(observation.output_tokens, 0), 0)
             + MAX(COALESCE(observation.cache_read_tokens, 0), 0)
             + MAX(COALESCE(observation.cache_creation_tokens, 0), 0)
    END,
    0, NULL,
    observation.user_country,
    observation.user_country_iso3,
    lower(trim(observation.user_email)),
    0,
    CASE
        WHEN trim(observation.created_at) != ''
         AND trim(observation.created_at) NOT GLOB '*[^0-9]*'
        THEN CAST(trim(observation.created_at) AS INTEGER)
        ELSE COALESCE(unixepoch(observation.created_at), 0)
    END
FROM legacy_token_market_request_logs observation
JOIN shares share ON share.share_id = observation.share_id
WHERE observation.share_id IS NOT NULL
  AND trim(observation.share_id) != ''
  AND observation.user_email IS NOT NULL
  AND trim(observation.user_email) != ''
  AND (
      lower(trim(observation.user_email)) = lower(trim(COALESCE(share.owner_email, '')))
      OR EXISTS (
          SELECT 1
            FROM json_each(
                CASE
                    WHEN json_valid(COALESCE(share.user_grants_json, '{}'))
                    THEN COALESCE(share.user_grants_json, '{}')
                    ELSE '{}'
                END
            ) grant_entry
           WHERE lower(trim(COALESCE(
                       json_extract(grant_entry.value, '$.email'),
                       grant_entry.key
                   ))) = lower(trim(observation.user_email))
      )
      OR EXISTS (
          SELECT 1
            FROM users known_user
           WHERE known_user.email_normalized = lower(trim(observation.user_email))
      )
  );

-- Keep a non-identifying retirement receipt.  It records only aggregate row
-- counts and time, never email, host, credential, price or settlement data.
CREATE TABLE IF NOT EXISTS data_retirement_audit (
    component TEXT PRIMARY KEY,
    source_rows INTEGER NOT NULL CHECK(source_rows >= 0),
    retained_rows INTEGER NOT NULL CHECK(retained_rows >= 0),
    retired_at TEXT NOT NULL
);
INSERT OR REPLACE INTO data_retirement_audit
    (component, source_rows, retained_rows, retired_at)
SELECT
    'router-local-capacity-market-v1',
    (SELECT COUNT(*) FROM legacy_token_market_request_logs),
    (
        SELECT COUNT(*)
          FROM legacy_token_market_request_logs observation
          JOIN shares share ON share.share_id = observation.share_id
         WHERE observation.share_id IS NOT NULL
           AND trim(observation.share_id) != ''
           AND observation.user_email IS NOT NULL
           AND trim(observation.user_email) != ''
           AND (
               lower(trim(observation.user_email)) = lower(trim(COALESCE(share.owner_email, '')))
               OR EXISTS (
                   SELECT 1
                     FROM json_each(
                         CASE
                             WHEN json_valid(COALESCE(share.user_grants_json, '{}'))
                             THEN COALESCE(share.user_grants_json, '{}')
                             ELSE '{}'
                         END
                     ) grant_entry
                    WHERE lower(trim(COALESCE(
                                json_extract(grant_entry.value, '$.email'),
                                grant_entry.key
                            ))) = lower(trim(observation.user_email))
               )
               OR EXISTS (
                   SELECT 1
                     FROM users known_user
                    WHERE known_user.email_normalized = lower(trim(observation.user_email))
               )
           )
    ),
    datetime('now');

-- The compatibility view becomes Gateway-only.  No former Market identity,
-- API-key prefix, USD amount or settlement state survives this boundary.  Do
-- not map gateway_id to user_email: Gateway is an observation principal, not
-- a terminal user, and must not enter user quota/identity aggregation.
DROP VIEW IF EXISTS capacity_request_observations;
CREATE VIEW capacity_request_observations AS
SELECT request_id, gateway_id, NULL AS tenant_id, NULL AS consumer_ref,
       NULL AS user_email, NULL AS api_key_prefix,
       router_id, share_id, share_subdomain, model,
       request_agent, requested_model, actual_model, actual_model_source,
       status, status_code, error_message, latency_ms, input_tokens,
       output_tokens, cache_read_tokens, cache_creation_tokens,
       NULL AS usage_amount_usd, created_at, NULL AS settled_at, user_country,
       user_country_iso3, observed_at, 'gateway' AS source_kind
  FROM gateway_request_observations;

DROP TABLE legacy_token_market_archive_manifest;
DROP TABLE legacy_token_market_router_markets;
DROP TABLE legacy_token_market_public_hosts;
DROP TABLE legacy_token_market_notification_emails;
DROP TABLE legacy_token_market_request_logs;
DROP TABLE legacy_token_market_disabled_shares;
DROP TABLE legacy_token_market_share_model_failure_state;
DROP TABLE legacy_token_market_share_runtime_states;

DROP TABLE router_markets;
DROP TABLE market_notification_emails;
DROP TABLE market_request_logs;
DROP TABLE market_disabled_shares;
DROP TABLE market_share_model_failure_state;
DROP TABLE market_share_runtime_states;
