-- Fresh installations use the existing inbound tunnel for all lease operations.
DROP TRIGGER cancel_host_bootstrap_on_owner_change;
DROP TRIGGER cancel_host_bootstrap_on_binding_change;
DROP TRIGGER cancel_host_bootstrap_on_rental_end;
DROP TRIGGER cancel_host_bootstrap_on_identity_change;
DROP TRIGGER client_host_tenancy_insert;
DROP TRIGGER client_host_tenancy_release;
DROP TRIGGER client_host_tenancy_delete;
DROP TRIGGER client_host_tenancy_pool_return;
DROP TRIGGER queue_client_binary_after_host_insert;
DROP TRIGGER client_host_idle_admission_insert;
DROP TRIGGER client_host_idle_admission_update;
DROP TRIGGER client_host_revoke_before_delete;
DROP TABLE client_host_agent_bootstraps;
DROP TABLE client_host_health_days;
DROP TABLE client_host_report_conflicts;
DROP TABLE client_host_revoked_keys;
DROP TABLE client_host_retired_ids;
DROP TABLE client_host_receiver_outages;
DROP TABLE client_host_receiver;
DROP TABLE client_host_tenancies;
DROP TABLE client_host_hour_reports;
DROP TABLE client_host_agents;
DROP TABLE client_binary_preparations;
DROP TABLE client_market_provider_terminal_authorizations;
DROP TABLE client_market_host_import_items;
DROP TABLE client_market_host_import_jobs;
DROP TABLE client_market_host_reprobe_state;
DROP TABLE subdomain_reservations;
DROP TABLE client_market_cleanup_retry_state;
DROP TABLE client_market_subscription_events;
DROP TABLE client_market_offer_events;
DROP TABLE client_market_audit_events;
DROP TABLE provisioning_jobs;
DROP TABLE client_market_subscriptions;
DROP TABLE client_market_allocation_quote_items;
DROP TABLE client_market_allocation_quotes;
DROP TABLE client_market_batches;
DROP TABLE router_ssh_hosts;

CREATE TABLE online_client_listings (
    installation_id TEXT PRIMARY KEY,
    seller_id TEXT NOT NULL,
    owner_email TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('locking','listed','unlocking','unlisted')),
    revision INTEGER NOT NULL CHECK(revision>0),
    sequence INTEGER NOT NULL CHECK(sequence>0),
    cycle_price_minor INTEGER NOT NULL CHECK(cycle_price_minor>=0),
    free_days INTEGER CHECK(free_days BETWEEN 1 AND 365),
    check_json TEXT,
    hardware_at INTEGER,
    updated_at INTEGER NOT NULL
);
CREATE INDEX online_client_listed ON online_client_listings(state,installation_id);
CREATE TABLE online_client_quotes (
    id TEXT PRIMARY KEY,
    installation_id TEXT NOT NULL REFERENCES online_client_listings,
    buyer_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    cycle_price_minor INTEGER NOT NULL,
    free_days INTEGER,
    trial_epoch INTEGER NOT NULL DEFAULT 0 CHECK(trial_epoch>=0),
    projection_json TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX online_client_quote_expiry ON online_client_quotes(expires_at);
CREATE TABLE online_client_leases (
    id TEXT PRIMARY KEY,
    installation_id TEXT NOT NULL REFERENCES online_client_listings,
    quote_id TEXT NOT NULL UNIQUE REFERENCES online_client_quotes,
    buyer_id TEXT NOT NULL,
    tenant_email TEXT NOT NULL,
    seller_id TEXT NOT NULL,
    alias TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK(state IN ('starting','active','ending','cleaning','released')),
    sequence INTEGER NOT NULL,
    grant_until INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    activated_at INTEGER,
    cutoff_at INTEGER,
    released_at INTEGER
);
CREATE UNIQUE INDEX online_client_exclusive ON online_client_leases(installation_id) WHERE released_at IS NULL;
CREATE INDEX online_client_seller ON online_client_leases(seller_id,created_at);
CREATE INDEX online_client_buyer ON online_client_leases(buyer_id,created_at);
CREATE INDEX online_client_pending ON online_client_leases(state,grant_until);
CREATE INDEX online_client_ending ON online_client_leases(id) WHERE state='ending';
CREATE TABLE online_client_operations (
    installation_id TEXT PRIMARY KEY REFERENCES online_client_listings,
    sequence INTEGER NOT NULL,
    request_json TEXT NOT NULL,
    next_attempt INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);
CREATE INDEX online_client_operations_due ON online_client_operations(next_attempt);
CREATE TABLE online_client_events (
    id INTEGER PRIMARY KEY,
    lease_id TEXT NOT NULL REFERENCES online_client_leases,
    event_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    recorded_at INTEGER NOT NULL,
    UNIQUE(lease_id,event_key)
);

ALTER TABLE online_client_listings ADD COLUMN next_probe INTEGER NOT NULL DEFAULT 0;
ALTER TABLE online_client_listings ADD COLUMN observed_at INTEGER;
ALTER TABLE online_client_listings ADD COLUMN healthy INTEGER NOT NULL DEFAULT 0;
ALTER TABLE online_client_listings ADD COLUMN trial_days INTEGER NOT NULL DEFAULT 0 CHECK(trial_days BETWEEN 0 AND 30);
ALTER TABLE online_client_listings ADD COLUMN egress_ip TEXT;
ALTER TABLE online_client_listings ADD COLUMN country_code TEXT;
ALTER TABLE online_client_listings ADD COLUMN challenge TEXT;
ALTER TABLE online_client_listings ADD COLUMN challenge_until INTEGER;
CREATE UNIQUE INDEX online_client_egress_challenge ON online_client_listings(challenge) WHERE challenge IS NOT NULL;
ALTER TABLE online_client_quotes ADD COLUMN auto_renew INTEGER NOT NULL DEFAULT 0;
CREATE TABLE online_client_probes (
    installation_id TEXT NOT NULL, lease_id TEXT NOT NULL, bucket INTEGER NOT NULL,
    checked_at INTEGER NOT NULL, healthy INTEGER NOT NULL CHECK(healthy IN (0,1)), seller_id TEXT NOT NULL,
    PRIMARY KEY(installation_id,lease_id,bucket)
);
CREATE INDEX online_client_rental_probes ON online_client_probes(lease_id,bucket);
CREATE INDEX online_client_successful_probes ON online_client_probes(installation_id,bucket) WHERE healthy=1;
CREATE INDEX online_client_provider_probes ON online_client_probes(seller_id,checked_at);
CREATE INDEX online_client_probe_retention ON online_client_probes(bucket);
CREATE TABLE online_client_share_bindings (
    share_id TEXT PRIMARY KEY, lease_id TEXT NOT NULL REFERENCES online_client_leases
);
CREATE INDEX online_client_share_lease ON online_client_share_bindings(lease_id,share_id);

CREATE TRIGGER online_client_owner_lock BEFORE UPDATE OF owner_email,owner_verified_at,lifecycle ON installations
WHEN EXISTS (SELECT 1 FROM online_client_listings l WHERE l.installation_id=OLD.id AND l.state!='unlisted')
 AND (lower(NEW.owner_email) IS NOT lower(OLD.owner_email) OR NEW.owner_verified_at IS NULL OR NEW.lifecycle!='active')
BEGIN SELECT RAISE(ABORT,'listed Client identity is locked'); END;
CREATE TRIGGER online_client_delete_lock BEFORE DELETE ON installations
WHEN EXISTS (SELECT 1 FROM online_client_listings l WHERE l.installation_id=OLD.id AND l.state!='unlisted')
BEGIN SELECT RAISE(ABORT,'unlist Client before deletion'); END;
CREATE TRIGGER online_client_tunnel_lock BEFORE UPDATE OF subdomain,enabled ON installation_client_tunnels
WHEN (NEW.subdomain!=OLD.subdomain OR NEW.enabled=0) AND EXISTS
(SELECT 1 FROM online_client_listings l WHERE l.installation_id=OLD.installation_id AND l.state!='unlisted')
BEGIN SELECT RAISE(ABORT,'listed Client route is locked'); END;
CREATE TRIGGER online_client_share_listing_lock BEFORE INSERT ON share_market_listings
WHEN NEW.status='active' AND EXISTS (SELECT 1 FROM online_client_listings l WHERE l.installation_id=NEW.installation_id AND l.state!='unlisted')
 AND NOT EXISTS (SELECT 1 FROM online_client_share_bindings b JOIN online_client_leases r ON r.id=b.lease_id
 WHERE b.share_id=NEW.share_id AND r.installation_id=NEW.installation_id AND r.state='active' AND r.grant_until>unixepoch() AND r.buyer_id=NEW.owner_user_id)
BEGIN SELECT RAISE(ABORT,'Client is locked for rental'); END;
CREATE TRIGGER online_client_share_reopen_lock BEFORE UPDATE OF status ON share_market_listings
WHEN NEW.status='active' AND EXISTS (SELECT 1 FROM online_client_listings l WHERE l.installation_id=NEW.installation_id AND l.state!='unlisted')
 AND NOT EXISTS (SELECT 1 FROM online_client_share_bindings b JOIN online_client_leases r ON r.id=b.lease_id
 WHERE b.share_id=NEW.share_id AND r.installation_id=NEW.installation_id AND r.state='active' AND r.grant_until>unixepoch() AND r.buyer_id=NEW.owner_user_id)
BEGIN SELECT RAISE(ABORT,'Client is locked for rental'); END;

ALTER TABLE installations ADD COLUMN market_lease_v1 INTEGER NOT NULL DEFAULT 0;

-- Every inventory surface, including checkout, uses the same capability and
-- freshness rules.
CREATE INDEX online_client_pending_finance ON client_market_terminations(host_id) WHERE status='finance_pending';
CREATE VIEW online_client_supply AS
SELECT snapshot.*,
       CASE WHEN occupied THEN 'allocated' WHEN probe_healthy THEN 'idle' ELSE 'maintaining' END AS supply_status
FROM (
    SELECT l.*, (r.id IS NOT NULL OR EXISTS(
               SELECT 1 FROM client_market_terminations t JOIN online_client_leases prior ON prior.id=t.installation_id
               WHERE t.host_id=l.installation_id AND t.status='finance_pending' AND prior.installation_id=l.installation_id
           )) AS occupied,
           coalesce(i.market_lease_v1=1 AND l.healthy=1 AND l.observed_at>=unixepoch()-600,0) AS probe_healthy
    FROM online_client_listings l
    JOIN installations i ON i.id=l.installation_id
    LEFT JOIN online_client_leases r ON r.installation_id=l.installation_id AND r.released_at IS NULL
) snapshot;

-- Freeze the original parties and egress at checkout, before an installation can be relisted.
CREATE TRIGGER online_client_capture_rental AFTER INSERT ON online_client_leases
BEGIN
    INSERT INTO client_market_rental_identities
        (id,installation_id,host_id,buyer_user_id,provider_user_id,buyer_email,provider_email,
         client_subdomain,host_ip,country_code,provenance,created_at)
    SELECT 'lease:'||NEW.id,NEW.id,NEW.installation_id,NEW.buyer_id,NEW.seller_id,NEW.tenant_email,
           l.owner_email,NEW.alias,l.egress_ip,l.country_code,'order_snapshot',
           strftime('%Y-%m-%dT%H:%M:%fZ',NEW.created_at,'unixepoch')
      FROM online_client_listings l WHERE l.installation_id=NEW.installation_id;
END;
