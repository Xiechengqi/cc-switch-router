ALTER TABLE client_market_subscriptions
    ADD COLUMN client_subdomain TEXT;
UPDATE client_market_subscriptions
   SET client_subdomain = (
        SELECT subdomain
          FROM installation_client_tunnels
         WHERE installation_id = client_market_subscriptions.installation_id
    )
 WHERE client_subdomain IS NULL;
UPDATE client_market_subscriptions
   SET client_subdomain = (
        SELECT label
          FROM public_hosts
         WHERE kind = 'client'
           AND subject_id = client_market_subscriptions.installation_id
         ORDER BY updated_at DESC
         LIMIT 1
    )
 WHERE client_subdomain IS NULL;
UPDATE client_market_subscriptions
   SET client_subdomain = (
        SELECT subdomain
          FROM provisioning_jobs
         WHERE installation_id = client_market_subscriptions.installation_id
           AND subdomain IS NOT NULL
           AND TRIM(subdomain) != ''
         ORDER BY updated_at DESC
         LIMIT 1
    )
 WHERE client_subdomain IS NULL;
CREATE INDEX idx_client_market_subscriptions_host_activated
    ON client_market_subscriptions(host_id, activated_at DESC);
CREATE INDEX idx_client_market_subscriptions_host_created
    ON client_market_subscriptions(host_id, created_at DESC);
