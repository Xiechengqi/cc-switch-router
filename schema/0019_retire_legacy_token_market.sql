-- Retire the Router-local Token Market integration without touching the
-- built-in Share/Client Market tables.  This migration is intentionally
-- append-only: legacy rows are copied to immutable, clearly named archive
-- tables so an operator can verify and restore them during the rollback
-- window.  No active request path is allowed to write those tables.

CREATE TABLE IF NOT EXISTS legacy_token_market_archive_manifest (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_table TEXT NOT NULL,
    archive_table TEXT NOT NULL,
    row_count INTEGER NOT NULL,
    checksum TEXT NOT NULL DEFAULT 'pending:legacy-token-market-archive-v1',
    archived_at TEXT NOT NULL,
    retention_until TEXT,
    notes TEXT,
    UNIQUE (source_table, archive_table)
);

CREATE TABLE IF NOT EXISTS legacy_token_market_router_markets AS
    SELECT *, '' AS archived_at FROM router_markets WHERE 0;
CREATE TABLE IF NOT EXISTS legacy_token_market_public_hosts AS
    SELECT *, '' AS archived_at FROM public_hosts WHERE 0;
CREATE TABLE IF NOT EXISTS legacy_token_market_notification_emails AS
    SELECT *, '' AS archived_at FROM market_notification_emails WHERE 0;
CREATE TABLE IF NOT EXISTS legacy_token_market_request_logs AS
    SELECT *, '' AS archived_at FROM market_request_logs WHERE 0;
CREATE TABLE IF NOT EXISTS legacy_token_market_disabled_shares AS
    SELECT *, '' AS archived_at FROM market_disabled_shares WHERE 0;
CREATE TABLE IF NOT EXISTS legacy_token_market_share_model_failure_state AS
    SELECT *, '' AS archived_at FROM market_share_model_failure_state WHERE 0;
CREATE TABLE IF NOT EXISTS legacy_token_market_share_runtime_states AS
    SELECT *, '' AS archived_at FROM market_share_runtime_states WHERE 0;

INSERT INTO legacy_token_market_router_markets
    SELECT router_markets.*, datetime('now') FROM router_markets;
INSERT INTO legacy_token_market_public_hosts
    SELECT public_hosts.*, datetime('now') FROM public_hosts WHERE kind = 'market';
INSERT INTO legacy_token_market_notification_emails
    SELECT market_notification_emails.*, datetime('now') FROM market_notification_emails;
INSERT INTO legacy_token_market_request_logs
    SELECT market_request_logs.*, datetime('now') FROM market_request_logs;
INSERT INTO legacy_token_market_disabled_shares
    SELECT market_disabled_shares.*, datetime('now') FROM market_disabled_shares;
INSERT INTO legacy_token_market_share_model_failure_state
    SELECT market_share_model_failure_state.*, datetime('now')
      FROM market_share_model_failure_state;
INSERT INTO legacy_token_market_share_runtime_states
    SELECT market_share_runtime_states.*, datetime('now')
      FROM market_share_runtime_states;

INSERT INTO legacy_token_market_archive_manifest
    (source_table, archive_table, row_count, archived_at, notes)
VALUES
    ('router_markets', 'legacy_token_market_router_markets',
        (SELECT COUNT(*) FROM router_markets), datetime('now'), 'legacy registry; no active reader'),
    ('public_hosts(kind=market)', 'legacy_token_market_public_hosts',
        (SELECT COUNT(*) FROM public_hosts WHERE kind = 'market'), datetime('now'), 'legacy public namespace'),
    ('market_notification_emails', 'legacy_token_market_notification_emails',
        (SELECT COUNT(*) FROM market_notification_emails), datetime('now'), 'legacy notification audit'),
    ('market_request_logs', 'legacy_token_market_request_logs',
        (SELECT COUNT(*) FROM market_request_logs), datetime('now'), 'legacy request observations'),
    ('market_disabled_shares', 'legacy_token_market_disabled_shares',
        (SELECT COUNT(*) FROM market_disabled_shares), datetime('now'), 'legacy per-market visibility'),
    ('market_share_model_failure_state', 'legacy_token_market_share_model_failure_state',
        (SELECT COUNT(*) FROM market_share_model_failure_state), datetime('now'), 'legacy market model state'),
    ('market_share_runtime_states', 'legacy_token_market_share_runtime_states',
        (SELECT COUNT(*) FROM market_share_runtime_states), datetime('now'), 'legacy market runtime state');

-- Legacy source tables remain in the rollback window but are immutable.  Any
-- code path that still attempts to write one fails loudly instead of silently
-- recreating the retired Token Market integration.
CREATE TRIGGER IF NOT EXISTS legacy_token_market_router_markets_read_only_insert
    BEFORE INSERT ON router_markets
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table router_markets is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_router_markets_read_only_update
    BEFORE UPDATE ON router_markets
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table router_markets is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_router_markets_read_only_delete
    BEFORE DELETE ON router_markets
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table router_markets is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_notification_emails_read_only_insert
    BEFORE INSERT ON market_notification_emails
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_notification_emails is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_notification_emails_read_only_update
    BEFORE UPDATE ON market_notification_emails
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_notification_emails is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_notification_emails_read_only_delete
    BEFORE DELETE ON market_notification_emails
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_notification_emails is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_request_logs_read_only_insert
    BEFORE INSERT ON market_request_logs
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_request_logs is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_request_logs_read_only_update
    BEFORE UPDATE ON market_request_logs
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_request_logs is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_request_logs_read_only_delete
    BEFORE DELETE ON market_request_logs
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_request_logs is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_disabled_shares_read_only_insert
    BEFORE INSERT ON market_disabled_shares
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_disabled_shares is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_disabled_shares_read_only_update
    BEFORE UPDATE ON market_disabled_shares
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_disabled_shares is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_disabled_shares_read_only_delete
    BEFORE DELETE ON market_disabled_shares
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_disabled_shares is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_model_failure_read_only_insert
    BEFORE INSERT ON market_share_model_failure_state
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_share_model_failure_state is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_model_failure_read_only_update
    BEFORE UPDATE ON market_share_model_failure_state
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_share_model_failure_state is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_model_failure_read_only_delete
    BEFORE DELETE ON market_share_model_failure_state
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_share_model_failure_state is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_runtime_states_read_only_insert
    BEFORE INSERT ON market_share_runtime_states
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_share_runtime_states is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_runtime_states_read_only_update
    BEFORE UPDATE ON market_share_runtime_states
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_share_runtime_states is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_runtime_states_read_only_delete
    BEFORE DELETE ON market_share_runtime_states
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market table market_share_runtime_states is read-only'); END;

-- Archive copies are immutable as well.  Restoring during the rollback window
-- is an explicit operator action performed in a maintenance transaction.
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_router_markets_read_only
    BEFORE INSERT ON legacy_token_market_router_markets
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_router_markets_read_only_update
    BEFORE UPDATE ON legacy_token_market_router_markets
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_router_markets_read_only_delete
    BEFORE DELETE ON legacy_token_market_router_markets
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_public_hosts_read_only
    BEFORE INSERT ON legacy_token_market_public_hosts
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_public_hosts_read_only_update
    BEFORE UPDATE ON legacy_token_market_public_hosts
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_public_hosts_read_only_delete
    BEFORE DELETE ON legacy_token_market_public_hosts
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_notification_emails_read_only
    BEFORE INSERT ON legacy_token_market_notification_emails
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_notification_emails_read_only_update
    BEFORE UPDATE ON legacy_token_market_notification_emails
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_notification_emails_read_only_delete
    BEFORE DELETE ON legacy_token_market_notification_emails
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_request_logs_read_only
    BEFORE INSERT ON legacy_token_market_request_logs
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_request_logs_read_only_update
    BEFORE UPDATE ON legacy_token_market_request_logs
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_request_logs_read_only_delete
    BEFORE DELETE ON legacy_token_market_request_logs
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_disabled_shares_read_only
    BEFORE INSERT ON legacy_token_market_disabled_shares
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_disabled_shares_read_only_update
    BEFORE UPDATE ON legacy_token_market_disabled_shares
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_disabled_shares_read_only_delete
    BEFORE DELETE ON legacy_token_market_disabled_shares
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_model_failure_read_only
    BEFORE INSERT ON legacy_token_market_share_model_failure_state
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_model_failure_read_only_update
    BEFORE UPDATE ON legacy_token_market_share_model_failure_state
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_model_failure_read_only_delete
    BEFORE DELETE ON legacy_token_market_share_model_failure_state
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_runtime_states_read_only
    BEFORE INSERT ON legacy_token_market_share_runtime_states
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_runtime_states_read_only_update
    BEFORE UPDATE ON legacy_token_market_share_runtime_states
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;
CREATE TRIGGER IF NOT EXISTS legacy_token_market_archive_runtime_states_read_only_delete
    BEFORE DELETE ON legacy_token_market_share_runtime_states
    BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;

-- The baseline schema allowed a `market` public-host kind.  Rebuild the
-- catalog so new rows can only be Client or Share hosts; historical market
-- rows were copied above before being removed from the live namespace.
DROP INDEX IF EXISTS idx_public_hosts_live_subject;
DROP INDEX IF EXISTS idx_public_hosts_live_route;
DROP INDEX IF EXISTS idx_public_hosts_target_lane;
CREATE TABLE public_hosts_without_legacy_market (
    label TEXT PRIMARY KEY COLLATE NOCASE,
    route_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('client', 'share')),
    subject_id TEXT NOT NULL,
    installation_id TEXT,
    target_lane_id TEXT NOT NULL,
    lifecycle TEXT NOT NULL CHECK(lifecycle IN ('active', 'disabled', 'tombstoned')),
    revision INTEGER NOT NULL CHECK(revision > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
INSERT INTO public_hosts_without_legacy_market
    (label, route_id, kind, subject_id, installation_id, target_lane_id,
     lifecycle, revision, created_at, updated_at)
SELECT label, route_id, kind, subject_id, installation_id, target_lane_id,
       lifecycle, revision, created_at, updated_at
  FROM public_hosts
 WHERE kind IN ('client', 'share');
DROP TABLE public_hosts;
ALTER TABLE public_hosts_without_legacy_market RENAME TO public_hosts;
CREATE UNIQUE INDEX idx_public_hosts_live_subject
    ON public_hosts(kind, subject_id)
    WHERE lifecycle != 'tombstoned';
CREATE UNIQUE INDEX idx_public_hosts_live_route
    ON public_hosts(route_id)
    WHERE lifecycle != 'tombstoned';
CREATE INDEX idx_public_hosts_target_lane
    ON public_hosts(target_lane_id, lifecycle);

-- Neutral Gateway-scoped state.  Gateway IDs, rather than email addresses or
-- Router-local Market rows, are the authorization/idempotency boundary.
CREATE TABLE IF NOT EXISTS gateway_share_disabled (
    gateway_id TEXT NOT NULL,
    share_id TEXT NOT NULL,
    disabled_by_gateway_id TEXT NOT NULL,
    reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (gateway_id, share_id)
);

CREATE TABLE IF NOT EXISTS gateway_share_model_failure_state (
    gateway_id TEXT NOT NULL,
    share_id TEXT NOT NULL,
    app_type TEXT NOT NULL,
    requested_model TEXT NOT NULL,
    actual_model TEXT NOT NULL DEFAULT '',
    last_status TEXT NOT NULL,
    last_success_at INTEGER,
    last_failed_at INTEGER,
    last_checked_at INTEGER NOT NULL,
    recent_results_json TEXT NOT NULL DEFAULT '[]',
    error_message TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (gateway_id, share_id, app_type, requested_model)
);

CREATE TABLE IF NOT EXISTS gateway_share_runtime_states (
    gateway_id TEXT NOT NULL,
    share_id TEXT NOT NULL,
    router_id TEXT,
    scope TEXT NOT NULL,
    kind TEXT NOT NULL,
    app_type TEXT,
    model_id TEXT,
    model_name TEXT,
    reason_kind TEXT,
    reason TEXT,
    failure_count INTEGER,
    expires_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_gateway_share_runtime_states_expiry
    ON gateway_share_runtime_states (expires_at);

CREATE TABLE IF NOT EXISTS gateway_request_observations (
    request_id TEXT PRIMARY KEY,
    gateway_id TEXT NOT NULL,
    router_id TEXT,
    share_id TEXT,
    share_subdomain TEXT,
    model TEXT,
    request_agent TEXT NOT NULL DEFAULT '',
    requested_model TEXT NOT NULL DEFAULT '',
    actual_model TEXT NOT NULL DEFAULT '',
    actual_model_source TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    status_code INTEGER,
    error_message TEXT,
    latency_ms INTEGER,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    user_country TEXT,
    user_country_iso3 TEXT,
    observed_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_gateway_request_observations_share_time
    ON gateway_request_observations (share_id, created_at);
CREATE INDEX IF NOT EXISTS idx_gateway_request_observations_gateway_time
    ON gateway_request_observations (gateway_id, created_at);

-- Unified read view used by usage/dashboard compatibility code.  New writes
-- land in gateway_request_observations; rows from the archived table remain
-- visible for reconciliation but are explicitly marked legacy.  A Gateway is
-- not a terminal user: keep its identity in gateway_id and never project it
-- into the user_email column consumed by user quota/identity queries.
CREATE VIEW IF NOT EXISTS capacity_request_observations AS
SELECT request_id, gateway_id, NULL AS tenant_id, NULL AS consumer_ref,
       NULL AS user_email, NULL AS api_key_prefix,
       router_id, share_id, share_subdomain, model,
       request_agent, requested_model, actual_model, actual_model_source,
       status, status_code, error_message, latency_ms, input_tokens,
       output_tokens, cache_read_tokens, cache_creation_tokens,
       NULL AS usage_amount_usd, created_at, NULL AS settled_at, user_country,
       user_country_iso3, observed_at, 'gateway' AS source_kind
  FROM gateway_request_observations
UNION ALL
SELECT request_id, NULL AS gateway_id, NULL AS tenant_id, NULL AS consumer_ref,
       user_email, api_key_prefix, router_id, share_id, share_subdomain, model,
       request_agent, requested_model, actual_model, actual_model_source,
       status, status_code, error_message, latency_ms, input_tokens,
       output_tokens, cache_read_tokens, cache_creation_tokens,
       usage_amount_usd, created_at, settled_at, user_country,
       user_country_iso3, synced_at AS observed_at, 'legacy_token_market' AS source_kind
  FROM legacy_token_market_request_logs;
