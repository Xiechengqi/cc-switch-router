-- A Market Provider is the stable public identity shared by Share Market and
-- Client Market. Financial rows intentionally keep their existing
-- supplier_user_id keys: rewriting money-bearing history would be unsafe.

CREATE TABLE market_provider_profiles (
    id TEXT PRIMARY KEY,
    user_id TEXT UNIQUE,
    canonical_email TEXT NOT NULL COLLATE NOCASE UNIQUE,
    display_name TEXT NOT NULL,
    claim_state TEXT NOT NULL CHECK (claim_state IN ('claimed', 'unclaimed')),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'paused', 'retired', 'identity_conflict')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    claimed_at TEXT,
    FOREIGN KEY(user_id) REFERENCES users(id)
);

CREATE TABLE market_provider_aliases (
    market_provider_id TEXT NOT NULL,
    alias_kind TEXT NOT NULL
        CHECK (alias_kind IN ('user_id', 'email', 'host_provider_id')),
    alias_value TEXT NOT NULL COLLATE NOCASE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (alias_kind, alias_value),
    FOREIGN KEY(market_provider_id) REFERENCES market_provider_profiles(id) ON DELETE CASCADE
);
CREATE INDEX idx_market_provider_alias_profile
    ON market_provider_aliases(market_provider_id, alias_kind);

CREATE TABLE market_provider_identity_events (
    id TEXT PRIMARY KEY,
    market_provider_id TEXT,
    event_kind TEXT NOT NULL,
    alias_kind TEXT,
    alias_value TEXT,
    detail_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    FOREIGN KEY(market_provider_id) REFERENCES market_provider_profiles(id)
);
CREATE INDEX idx_market_provider_identity_events_profile
    ON market_provider_identity_events(market_provider_id, created_at DESC);

ALTER TABLE host_provider_profiles ADD COLUMN market_provider_id TEXT
    REFERENCES market_provider_profiles(id);
ALTER TABLE share_market_listings ADD COLUMN market_provider_id TEXT
    REFERENCES market_provider_profiles(id);
CREATE INDEX idx_host_provider_profiles_market_provider
    ON host_provider_profiles(market_provider_id);
CREATE INDEX idx_share_market_listings_market_provider
    ON share_market_listings(market_provider_id, status);

-- Seed claimed identities first. A random opaque id is generated only once and
-- remains stable when an earlier email-only identity is later claimed.
INSERT INTO market_provider_profiles (
    id, user_id, canonical_email, display_name, claim_state,
    status, created_at, updated_at, claimed_at
)
SELECT 'mp_' || lower(hex(randomblob(12))), u.id, lower(trim(u.email_normalized)),
       'Provider', 'claimed', 'active', u.created_at, u.last_login_at, u.created_at
FROM users u
WHERE trim(u.email_normalized) != ''
  AND (
      EXISTS (SELECT 1 FROM host_provider_profiles hp
              WHERE lower(trim(hp.owner_email)) = lower(trim(u.email_normalized)))
      OR EXISTS (SELECT 1 FROM share_market_listings listing
                 WHERE lower(trim(listing.owner_email)) = lower(trim(u.email_normalized)))
      OR EXISTS (SELECT 1 FROM market_credit_accounts account
                 WHERE account.supplier_user_id = u.id)
      OR EXISTS (SELECT 1 FROM market_prepaid_accounts prepaid
                 WHERE prepaid.supplier_user_id = u.id)
      OR EXISTS (SELECT 1 FROM market_counterparties counterparty
                 WHERE counterparty.supplier_user_id = u.id)
      OR EXISTS (SELECT 1 FROM market_service_contracts contract
                 WHERE contract.supplier_user_id = u.id)
      OR EXISTS (SELECT 1 FROM market_recurring_contracts recurring
                 WHERE recurring.supplier_user_id = u.id)
  );

-- Host rows can predate Router accounts. Preserve them as unclaimed identities
-- keyed by verified-normalized email until a matching user is created.
INSERT INTO market_provider_profiles (
    id, user_id, canonical_email, display_name, claim_state,
    status, created_at, updated_at, claimed_at
)
SELECT 'mp_' || lower(hex(randomblob(12))), NULL, source.email,
       'Provider', 'unclaimed', 'active', MIN(source.created_at), MAX(source.updated_at), NULL
FROM (
    SELECT lower(trim(owner_email)) AS email, MIN(created_at) AS created_at,
           MAX(updated_at) AS updated_at
    FROM host_provider_profiles
    WHERE trim(owner_email) != ''
    GROUP BY lower(trim(owner_email))
    UNION
    SELECT lower(trim(owner_email)) AS email, MIN(created_at) AS created_at,
           MAX(updated_at) AS updated_at
    FROM share_market_listings
    WHERE trim(owner_email) != ''
    GROUP BY lower(trim(owner_email))
    UNION
    SELECT lower(trim(supplier_email)) AS email, MIN(created_at) AS created_at,
           MAX(updated_at) AS updated_at
    FROM market_counterparties
    WHERE trim(supplier_email) != ''
    GROUP BY lower(trim(supplier_email))
    UNION
    SELECT lower(trim(supplier_email)) AS email, MIN(created_at) AS created_at,
           MAX(updated_at) AS updated_at
    FROM market_credit_accounts
    WHERE trim(supplier_email) != ''
    GROUP BY lower(trim(supplier_email))
    UNION
    SELECT lower(trim(supplier_email)) AS email, MIN(created_at) AS created_at,
           MAX(updated_at) AS updated_at
    FROM market_prepaid_accounts
    WHERE trim(supplier_email) != ''
    GROUP BY lower(trim(supplier_email))
    UNION
    SELECT lower(trim(supplier_email)) AS email, MIN(created_at) AS created_at,
           MAX(updated_at) AS updated_at
    FROM market_service_contracts
    WHERE trim(supplier_email) != ''
    GROUP BY lower(trim(supplier_email))
    UNION
    SELECT lower(trim(supplier_email)) AS email, MIN(created_at) AS created_at,
           MAX(updated_at) AS updated_at
    FROM market_recurring_contracts
    WHERE trim(supplier_email) != ''
    GROUP BY lower(trim(supplier_email))
) source
WHERE NOT EXISTS (
    SELECT 1 FROM market_provider_profiles profile
    WHERE profile.canonical_email = source.email
)
GROUP BY source.email;

INSERT OR IGNORE INTO market_provider_aliases (
    market_provider_id, alias_kind, alias_value, created_at
)
SELECT id, 'email', canonical_email, created_at FROM market_provider_profiles;

INSERT OR IGNORE INTO market_provider_aliases (
    market_provider_id, alias_kind, alias_value, created_at
)
SELECT id, 'user_id', user_id, created_at
FROM market_provider_profiles WHERE user_id IS NOT NULL;

INSERT OR IGNORE INTO market_provider_aliases (
    market_provider_id, alias_kind, alias_value, created_at
)
SELECT profile.id, 'host_provider_id', host.provider_id, host.created_at
FROM host_provider_profiles host
JOIN market_provider_profiles profile
  ON profile.canonical_email = lower(trim(host.owner_email));

UPDATE host_provider_profiles
SET market_provider_id = (
    SELECT profile.id FROM market_provider_profiles profile
    WHERE profile.canonical_email = lower(trim(host_provider_profiles.owner_email))
);

UPDATE share_market_listings
SET market_provider_id = (
    SELECT profile.id FROM market_provider_profiles profile
    WHERE profile.canonical_email = lower(trim(share_market_listings.owner_email))
);

-- A listing can be stopped, reopened, or transferred without changing its
-- listing id. Keep explicit half-open Provider ownership/publication windows
-- so historical quality and fulfillment never move to the current owner.
CREATE TABLE market_provider_share_windows (
    id TEXT PRIMARY KEY,
    listing_id TEXT NOT NULL,
    market_provider_id TEXT NOT NULL,
    starts_at TEXT NOT NULL,
    ends_at TEXT,
    created_at TEXT NOT NULL,
    CHECK (trim(starts_at) != '' AND (ends_at IS NULL OR trim(ends_at) != '')),
    FOREIGN KEY(listing_id) REFERENCES share_market_listings(id) ON DELETE CASCADE,
    FOREIGN KEY(market_provider_id) REFERENCES market_provider_profiles(id) ON DELETE CASCADE
);
-- Date/time functions in a CHECK constraint reject legacy values such as the
-- SQLite-supported literal "now" as non-deterministic. Triggers preserve the
-- same parse and interval validation while allowing those existing rows to be
-- migrated. Runtime writes use concrete RFC 3339 timestamps.
CREATE TRIGGER trg_market_provider_share_windows_timestamp_insert
BEFORE INSERT ON market_provider_share_windows
WHEN julianday(NEW.starts_at) IS NULL
  OR (NEW.ends_at IS NOT NULL AND (
      julianday(NEW.ends_at) IS NULL
      OR julianday(NEW.ends_at) < julianday(NEW.starts_at)
  ))
BEGIN
    SELECT RAISE(ABORT, 'invalid Market Provider Share window timestamp');
END;
CREATE TRIGGER trg_market_provider_share_windows_timestamp_update
BEFORE UPDATE OF starts_at, ends_at ON market_provider_share_windows
WHEN julianday(NEW.starts_at) IS NULL
  OR (NEW.ends_at IS NOT NULL AND (
      julianday(NEW.ends_at) IS NULL
      OR julianday(NEW.ends_at) < julianday(NEW.starts_at)
  ))
BEGIN
    SELECT RAISE(ABORT, 'invalid Market Provider Share window timestamp');
END;
CREATE UNIQUE INDEX idx_market_provider_share_windows_open
    ON market_provider_share_windows(listing_id) WHERE ends_at IS NULL;
CREATE INDEX idx_market_provider_share_windows_listing_time
    ON market_provider_share_windows(listing_id, starts_at, ends_at, market_provider_id);
CREATE INDEX idx_market_provider_share_windows_provider_time
    ON market_provider_share_windows(market_provider_id, starts_at, ends_at, listing_id);

-- Historical databases do not have lifecycle events for every automatic
-- closure, so migration can only seed the best known interval. All lifecycle
-- changes after migration maintain exact windows transactionally.
INSERT INTO market_provider_share_windows (
    id, listing_id, market_provider_id, starts_at, ends_at, created_at
)
SELECT 'mpw_' || lower(hex(randomblob(12))), listing.id,
       listing.market_provider_id, listing.created_at,
       CASE WHEN listing.status = 'active' AND listing.deleted_at IS NULL
            THEN NULL
            ELSE COALESCE(listing.deleted_at, listing.updated_at)
       END,
       listing.created_at
FROM share_market_listings listing
WHERE listing.market_provider_id IS NOT NULL;

CREATE TABLE market_provider_effective_selections (
    market_provider_id TEXT NOT NULL,
    buyer_user_id TEXT NOT NULL,
    first_selected_at TEXT NOT NULL,
    last_selected_at TEXT NOT NULL,
    source_mask INTEGER NOT NULL CHECK (source_mask BETWEEN 1 AND 3),
    weight_millis INTEGER NOT NULL CHECK (weight_millis BETWEEN 1 AND 1000),
    refreshed_at TEXT NOT NULL,
    PRIMARY KEY (market_provider_id, buyer_user_id),
    FOREIGN KEY(market_provider_id) REFERENCES market_provider_profiles(id) ON DELETE CASCADE
);
CREATE INDEX idx_market_provider_selections_recent
    ON market_provider_effective_selections(last_selected_at, market_provider_id);

CREATE TABLE market_provider_rank_generations (
    id TEXT PRIMARY KEY,
    algorithm_version TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('building', 'published', 'failed')),
    observed_from TEXT NOT NULL,
    observed_to TEXT NOT NULL,
    provider_count INTEGER NOT NULL DEFAULT 0 CHECK (provider_count >= 0),
    failure_summary TEXT,
    created_at TEXT NOT NULL,
    published_at TEXT
);
CREATE INDEX idx_market_provider_rank_generation_state
    ON market_provider_rank_generations(state, published_at DESC, created_at DESC);

CREATE TABLE market_provider_rank_entries (
    generation_id TEXT NOT NULL,
    market_provider_id TEXT NOT NULL,
    rank_position INTEGER CHECK (rank_position IS NULL OR rank_position > 0),
    rank_state TEXT NOT NULL CHECK (rank_state IN ('ranked', 'collecting')),
    score_bps INTEGER CHECK (score_bps IS NULL OR score_bps BETWEEN 0 AND 10000),
    service_quality_bps INTEGER NOT NULL CHECK (service_quality_bps BETWEEN 0 AND 10000),
    effective_choice_bps INTEGER NOT NULL CHECK (effective_choice_bps BETWEEN 0 AND 10000),
    fulfillment_bps INTEGER NOT NULL CHECK (fulfillment_bps BETWEEN 0 AND 10000),
    supply_breadth_bps INTEGER NOT NULL CHECK (supply_breadth_bps BETWEEN 0 AND 10000),
    independent_buyer_count INTEGER NOT NULL CHECK (independent_buyer_count >= 0),
    observation_days INTEGER NOT NULL CHECK (observation_days >= 0),
    share_probe_count INTEGER NOT NULL DEFAULT 0 CHECK (share_probe_count >= 0),
    ttft_ms REAL,
    tps REAL,
    metrics_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    PRIMARY KEY (generation_id, market_provider_id),
    FOREIGN KEY(generation_id) REFERENCES market_provider_rank_generations(id) ON DELETE CASCADE,
    FOREIGN KEY(market_provider_id) REFERENCES market_provider_profiles(id) ON DELETE CASCADE
);
CREATE INDEX idx_market_provider_rank_entries_order
    ON market_provider_rank_entries(generation_id, rank_state, rank_position, market_provider_id);
