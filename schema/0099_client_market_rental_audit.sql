-- Durable identity only: rental/cleanup states and money remain in their
-- existing authoritative tables. Never cascade this history with a Host/Client.
CREATE TABLE client_market_rental_identities (
    id TEXT PRIMARY KEY,
    job_id TEXT UNIQUE,
    installation_id TEXT UNIQUE,
    host_id TEXT,
    buyer_user_id TEXT,
    provider_user_id TEXT,
    buyer_email TEXT,
    provider_email TEXT,
    client_subdomain TEXT,
    host_ip TEXT,
    host_port INTEGER,
    country_code TEXT,
    provenance TEXT NOT NULL CHECK (provenance IN ('order_snapshot', 'historical_record')),
    created_at TEXT NOT NULL
);

-- Also bound the original-job backfill and each row's latest-cleanup lookup.
-- Install before backfill so upgrading a large history is not quadratic.
CREATE INDEX idx_client_rental_audit_jobs
    ON provisioning_jobs(installation_id,type,created_at DESC,id DESC,status);

-- Subscription identities survive installation purges. Prefer the original
-- provisioning event over an actively synchronized Provider email. Historical
-- IP/region cannot be established from today's Host, so leave them unknown.
INSERT INTO client_market_rental_identities
    (id,job_id,installation_id,host_id,buyer_user_id,provider_user_id,
     buyer_email,provider_email,client_subdomain,provenance,created_at)
SELECT CASE WHEN j.id IS NULL THEN 'rental:'||s.installation_id ELSE 'job:'||j.id END,
       j.id,s.installation_id,s.host_id,s.client_user_id,
       COALESCE((SELECT json_extract(e.detail_json,'$.providerId')
                 FROM client_market_subscription_events e
                 WHERE e.installation_id=s.installation_id AND e.event_type='client_provisioned'
                   AND json_valid(e.detail_json)
                 ORDER BY e.created_at,e.id LIMIT 1),s.provider_id),
       COALESCE(j.client_owner_email,s.client_owner_email),
       COALESCE((SELECT json_extract(e.detail_json,'$.hostOwnerEmail')
                 FROM client_market_subscription_events e
                 WHERE e.installation_id=s.installation_id AND e.event_type='client_provisioned'
                   AND json_valid(e.detail_json)
                 ORDER BY e.created_at,e.id LIMIT 1),j.host_owner_email,s.host_owner_email),
       COALESCE(NULLIF(s.client_subdomain,''),j.subdomain), 'historical_record',
       COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ',COALESCE(j.created_at,s.created_at)),s.created_at)
FROM client_market_subscriptions s
LEFT JOIN provisioning_jobs j ON j.id=(
    SELECT original.id FROM provisioning_jobs original
    WHERE original.type='create' AND original.installation_id=s.installation_id
    ORDER BY original.created_at,original.id LIMIT 1
);

-- Include committed requests that never delivered a Client. A retried task for
-- an already represented installation must not become a second rental row.
INSERT INTO client_market_rental_identities
    (id,job_id,installation_id,host_id,buyer_user_id,provider_user_id,
     buyer_email,provider_email,client_subdomain,provenance,created_at)
SELECT 'job:'||j.id,j.id,j.installation_id,j.host_id,j.client_owner_user_id,
       (SELECT qi.provider_id FROM client_market_allocation_quote_items qi
        WHERE qi.quote_id=j.quote_id AND qi.host_id=j.host_id LIMIT 1),
       j.client_owner_email,j.host_owner_email,j.subdomain,'historical_record',
       COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ',j.created_at),j.created_at)
FROM provisioning_jobs j
WHERE j.type='create'
  AND NOT EXISTS (SELECT 1 FROM client_market_rental_identities a WHERE a.job_id=j.id)
  AND (j.installation_id IS NULL OR (
      NOT EXISTS (SELECT 1 FROM client_market_rental_identities a WHERE a.installation_id=j.installation_id)
      AND j.id=(SELECT original.id FROM provisioning_jobs original
                WHERE original.type='create' AND original.installation_id=j.installation_id
                ORDER BY original.created_at,original.id LIMIT 1)
  ));

CREATE INDEX idx_client_rental_audit_created ON client_market_rental_identities(created_at DESC,id DESC);
CREATE INDEX idx_client_rental_audit_buyer ON client_market_rental_identities(buyer_email COLLATE NOCASE,created_at DESC,id DESC);
CREATE INDEX idx_client_rental_audit_provider ON client_market_rental_identities(provider_email COLLATE NOCASE,created_at DESC,id DESC);
CREATE INDEX idx_client_rental_audit_host ON client_market_rental_identities(host_id,created_at DESC,id DESC);
CREATE INDEX idx_client_rental_audit_subdomain ON client_market_rental_identities(client_subdomain COLLATE NOCASE);
CREATE INDEX idx_client_rental_audit_ip ON client_market_rental_identities(host_ip);
CREATE INDEX idx_client_rental_audit_contract ON client_market_subscriptions(platform_contract_id);

-- The only permitted change is binding an initially unassigned installation.
CREATE TRIGGER trg_client_rental_identity_immutable BEFORE UPDATE ON client_market_rental_identities
WHEN NEW.id IS NOT OLD.id OR NEW.job_id IS NOT OLD.job_id OR NEW.host_id IS NOT OLD.host_id
 OR NEW.buyer_user_id IS NOT OLD.buyer_user_id OR NEW.provider_user_id IS NOT OLD.provider_user_id
 OR NEW.buyer_email IS NOT OLD.buyer_email OR NEW.provider_email IS NOT OLD.provider_email
 OR NEW.client_subdomain IS NOT OLD.client_subdomain OR NEW.host_ip IS NOT OLD.host_ip
 OR NEW.host_port IS NOT OLD.host_port OR NEW.country_code IS NOT OLD.country_code
 OR NEW.provenance IS NOT OLD.provenance OR NEW.created_at IS NOT OLD.created_at
 OR (OLD.installation_id IS NOT NULL AND NEW.installation_id IS NOT OLD.installation_id)
BEGIN SELECT RAISE(ABORT,'Rental audit identity is immutable'); END;
CREATE TRIGGER trg_client_rental_identity_no_delete BEFORE DELETE ON client_market_rental_identities
BEGIN SELECT RAISE(ABORT,'Rental audit identity cannot be deleted'); END;
