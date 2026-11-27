-- Identity survives ordinary runtime GC. Never infer ownership from an IP.
CREATE TABLE client_identities (
    installation_id TEXT PRIMARY KEY,
    public_key TEXT NOT NULL UNIQUE,
    owner_email TEXT,
    owner_verified_at TEXT,
    client_activated_at TEXT,
    lifecycle TEXT NOT NULL CHECK(lifecycle IN ('active','fenced')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    offline_episode INTEGER NOT NULL DEFAULT 0
);
INSERT INTO client_identities(installation_id,public_key,owner_email,owner_verified_at,client_activated_at,lifecycle,created_at,updated_at)
SELECT id,public_key,owner_email,owner_verified_at,client_activated_at,lifecycle,created_at,last_seen_at
FROM (SELECT i.*,row_number() OVER (
    PARTITION BY public_key ORDER BY (lifecycle='active') DESC,last_seen_at DESC,created_at DESC,id DESC
) AS identity_rank FROM installations i) WHERE identity_rank=1;

CREATE TABLE client_identity_aliases (
    previous_installation_id TEXT PRIMARY KEY,
    installation_id TEXT NOT NULL,
    public_key TEXT NOT NULL,
    evidence TEXT NOT NULL,
    actor_email TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX client_identity_alias_target ON client_identity_aliases(installation_id);

CREATE TRIGGER remember_client_identity_insert AFTER INSERT ON installations
BEGIN
    INSERT INTO client_identities(installation_id,public_key,owner_email,owner_verified_at,client_activated_at,lifecycle,created_at,updated_at) VALUES(NEW.id,NEW.public_key,NEW.owner_email,NEW.owner_verified_at,
        NEW.client_activated_at,NEW.lifecycle,NEW.created_at,NEW.last_seen_at)
    ON CONFLICT(public_key) DO UPDATE SET
        owner_email=excluded.owner_email,owner_verified_at=excluded.owner_verified_at,
        client_activated_at=excluded.client_activated_at,lifecycle=excluded.lifecycle,
        updated_at=excluded.updated_at
    WHERE client_identities.installation_id=excluded.installation_id;
END;
CREATE TRIGGER remember_client_identity_update
AFTER UPDATE OF owner_email,owner_verified_at,client_activated_at,lifecycle ON installations
BEGIN
    UPDATE client_identities SET owner_email=NEW.owner_email,owner_verified_at=NEW.owner_verified_at,
        client_activated_at=NEW.client_activated_at,lifecycle=NEW.lifecycle,updated_at=NEW.last_seen_at
    WHERE installation_id=NEW.id AND public_key=NEW.public_key;
END;

-- Marketplace intent is independent of local delivery. A rental command must
-- finish before an unlist can occupy the single command slot.
ALTER TABLE online_client_listings ADD COLUMN unlist_pending INTEGER NOT NULL DEFAULT 0 CHECK(unlist_pending IN (0,1));
ALTER TABLE online_client_listings ADD COLUMN unlisted_at INTEGER;
ALTER TABLE online_client_operations ADD COLUMN delivery_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE online_client_operations ADD COLUMN waiting_reconnect INTEGER NOT NULL DEFAULT 0 CHECK(waiting_reconnect IN (0,1));
ALTER TABLE online_client_operations ADD COLUMN last_attempt_at INTEGER;
UPDATE online_client_listings SET state='unlisted',unlist_pending=1,unlisted_at=unixepoch(),
    revision=revision+1,challenge=NULL,challenge_until=NULL WHERE state='unlocking';
UPDATE online_client_operations SET attempts=0,last_error=NULL
WHERE installation_id IN (SELECT installation_id FROM online_client_listings WHERE unlist_pending=1);

-- Listed state cannot be used as a proxy for rental/finance protection now that
-- an owner can stop new orders while a tenant is still using the workspace.
CREATE VIEW online_client_protection AS
SELECT installation_id FROM online_client_listings WHERE state!='unlisted'
UNION ALL SELECT installation_id FROM online_client_leases WHERE released_at IS NULL
UNION ALL SELECT host_id FROM client_market_terminations
    WHERE status='finance_pending' OR cleanup_confirmed_at IS NULL;

DROP TRIGGER online_client_owner_lock;
DROP TRIGGER online_client_delete_lock;
DROP TRIGGER online_client_tunnel_lock;
CREATE TRIGGER online_client_owner_lock BEFORE UPDATE OF owner_email,owner_verified_at,lifecycle ON installations
WHEN (EXISTS(SELECT 1 FROM online_client_protection p WHERE p.installation_id=OLD.id)
      OR EXISTS(SELECT 1 FROM online_client_listings l WHERE l.installation_id=OLD.id AND l.unlist_pending=1))
 AND (lower(NEW.owner_email) IS NOT lower(OLD.owner_email) OR NEW.owner_verified_at IS NULL OR NEW.lifecycle!='active')
BEGIN SELECT RAISE(ABORT,'Client market identity is locked'); END;
CREATE TRIGGER online_client_delete_lock BEFORE DELETE ON installations
WHEN EXISTS(SELECT 1 FROM online_client_protection p WHERE p.installation_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'finish Client rental obligations before deletion'); END;
CREATE TRIGGER online_client_tunnel_lock BEFORE UPDATE OF subdomain,enabled ON installation_client_tunnels
WHEN (NEW.subdomain!=OLD.subdomain OR NEW.enabled=0)
 AND (EXISTS(SELECT 1 FROM online_client_protection p WHERE p.installation_id=OLD.installation_id)
      OR EXISTS(SELECT 1 FROM online_client_listings l WHERE l.installation_id=OLD.installation_id AND l.unlist_pending=1))
BEGIN SELECT RAISE(ABORT,'Client market route is locked'); END;

DROP TRIGGER online_client_share_listing_lock;
DROP TRIGGER online_client_share_reopen_lock;
CREATE TRIGGER online_client_share_listing_lock BEFORE INSERT ON share_market_listings
WHEN NEW.status='active' AND EXISTS(SELECT 1 FROM online_client_protection p WHERE p.installation_id=NEW.installation_id)
 AND NOT EXISTS(SELECT 1 FROM online_client_share_bindings b JOIN online_client_leases r ON r.id=b.lease_id
    WHERE b.share_id=NEW.share_id AND r.installation_id=NEW.installation_id AND r.state='active' AND r.grant_until>unixepoch() AND r.buyer_id=NEW.owner_user_id)
BEGIN SELECT RAISE(ABORT,'Client is locked for rental'); END;
CREATE TRIGGER online_client_share_reopen_lock BEFORE UPDATE OF status ON share_market_listings
WHEN NEW.status='active' AND EXISTS(SELECT 1 FROM online_client_protection p WHERE p.installation_id=NEW.installation_id)
 AND NOT EXISTS(SELECT 1 FROM online_client_share_bindings b JOIN online_client_leases r ON r.id=b.lease_id
    WHERE b.share_id=NEW.share_id AND r.installation_id=NEW.installation_id AND r.state='active' AND r.grant_until>unixepoch() AND r.buyer_id=NEW.owner_user_id)
BEGIN SELECT RAISE(ABORT,'Client is locked for rental'); END;
