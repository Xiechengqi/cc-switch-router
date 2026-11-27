-- Durable Share rental identity only: rental, revoke and termination states
-- and all money remain in their existing authoritative tables. A Share row
-- can be hard-deleted, so the audit never depends on it for identity.
CREATE TABLE share_market_rental_identities (
    id TEXT PRIMARY KEY,
    share_id TEXT NOT NULL,
    share_name TEXT,
    share_subdomain TEXT,
    app_type TEXT,
    listing_id TEXT,
    seat_id TEXT,
    seat_position INTEGER,
    installation_id TEXT,
    buyer_user_id TEXT,
    provider_user_id TEXT,
    buyer_email TEXT,
    provider_email TEXT,
    provenance TEXT NOT NULL CHECK (provenance IN ('order_snapshot', 'historical_record')),
    created_at TEXT NOT NULL
);

-- Historical rentals: the subscription row already freezes buyer identity,
-- seat and installation. A termination payload froze the Share name and both
-- emails when it was accepted, which predates any later rename or deletion, so
-- it wins over today's Share row. A live rental's owner columns are rewritten
-- on Share ownership transfer; the platform contract keeps the original
-- Provider ID, and an unprovable Provider email is left unknown. A Share that
-- was already deleted leaves its name, subdomain and app unknown (NULL).
INSERT INTO share_market_rental_identities
    (id,share_id,share_name,share_subdomain,app_type,listing_id,seat_id,seat_position,
     installation_id,buyer_user_id,provider_user_id,buyer_email,provider_email,provenance,created_at)
SELECT s.id,s.share_id,
       COALESCE(NULLIF(json_extract(t.payload_json,'$.shareName'),''),sh.share_name),
       COALESCE(NULLIF(json_extract(t.payload_json,'$.subdomain'),''),NULLIF(sh.subdomain,'')),
       COALESCE(NULLIF(NULLIF(s.required_app,''),'unknown'),NULLIF(sh.app_type,'')),
       s.listing_id,s.seat_id,seat.position,s.installation_id,s.renter_user_id,
       COALESCE(t.provider_user_id,c.provider_user_id,s.owner_user_id),
       COALESCE(NULLIF(json_extract(t.payload_json,'$.buyerEmail'),''),s.renter_email),
       CASE WHEN NULLIF(json_extract(t.payload_json,'$.providerEmail'),'') IS NOT NULL
                 THEN json_extract(t.payload_json,'$.providerEmail')
            WHEN c.provider_user_id IS NOT NULL AND c.provider_user_id IS NOT s.owner_user_id
                 THEN (SELECT u.email_normalized FROM users u WHERE u.id=c.provider_user_id)
            ELSE s.owner_email END,
       'historical_record',
       COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ',s.created_at),s.created_at)
FROM share_market_subscriptions s
LEFT JOIN shares sh ON sh.share_id=s.share_id
LEFT JOIN share_market_seats seat ON seat.id=s.seat_id
LEFT JOIN market_platform_contracts c ON c.product_kind='share' AND c.product_ref=s.id
LEFT JOIN share_market_terminations t ON t.subscription_id=s.id;

-- Every rent path inserts a subscription; snapshot it in the same statement so
-- no Rust insert path can forget the audit identity. Ignoring a duplicate ID
-- never blocks a rental: the identity of that ID is already recorded.
CREATE TRIGGER trg_share_rental_identity_snapshot AFTER INSERT ON share_market_subscriptions
BEGIN
    INSERT OR IGNORE INTO share_market_rental_identities
        (id,share_id,share_name,share_subdomain,app_type,listing_id,seat_id,seat_position,
         installation_id,buyer_user_id,provider_user_id,buyer_email,provider_email,provenance,created_at)
    VALUES (NEW.id,NEW.share_id,
        (SELECT sh.share_name FROM shares sh WHERE sh.share_id=NEW.share_id),
        (SELECT NULLIF(sh.subdomain,'') FROM shares sh WHERE sh.share_id=NEW.share_id),
        COALESCE(NULLIF(NULLIF(NEW.required_app,''),'unknown'),
                 (SELECT NULLIF(sh.app_type,'') FROM shares sh WHERE sh.share_id=NEW.share_id)),
        NEW.listing_id,NEW.seat_id,
        (SELECT seat.position FROM share_market_seats seat WHERE seat.id=NEW.seat_id),
        NEW.installation_id,NEW.renter_user_id,NEW.owner_user_id,NEW.renter_email,NEW.owner_email,
        'order_snapshot',COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ',NEW.created_at),NEW.created_at));
END;

CREATE INDEX idx_share_rental_audit_created ON share_market_rental_identities(created_at DESC,id DESC);
CREATE INDEX idx_share_rental_audit_buyer ON share_market_rental_identities(buyer_email COLLATE NOCASE,created_at DESC,id DESC);
CREATE INDEX idx_share_rental_audit_provider ON share_market_rental_identities(provider_email COLLATE NOCASE,created_at DESC,id DESC);
CREATE INDEX idx_share_rental_audit_share ON share_market_rental_identities(share_id,created_at DESC,id DESC);
CREATE INDEX idx_share_rental_audit_name ON share_market_rental_identities(share_name COLLATE NOCASE);
CREATE INDEX idx_share_rental_audit_subdomain ON share_market_rental_identities(share_subdomain COLLATE NOCASE);

-- Audit history is append-only.
CREATE TRIGGER trg_share_rental_identity_immutable BEFORE UPDATE ON share_market_rental_identities
BEGIN SELECT RAISE(ABORT,'Share rental audit identity is immutable'); END;
CREATE TRIGGER trg_share_rental_identity_no_delete BEFORE DELETE ON share_market_rental_identities
BEGIN SELECT RAISE(ABORT,'Share rental audit identity cannot be deleted'); END;
