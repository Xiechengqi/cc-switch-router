-- Model price catalog + listing usage rollup.
-- See docs/design-share-user-model-usage-and-pricing.md §5.1 and §8.5.
--
-- All prices are integer micro-USD per 1M tokens. No floating point anywhere:
-- libSQL has no decimal type, and the Rust kernel accumulates in i128.

CREATE TABLE model_price_catalog (
    price_key                TEXT    NOT NULL,
    effective_from           INTEGER NOT NULL,
    effective_to             INTEGER,
    display_name             TEXT    NOT NULL DEFAULT '',
    currency                 TEXT    NOT NULL DEFAULT 'USD',
    long_context_threshold   INTEGER,
    long_context_inclusive   INTEGER NOT NULL DEFAULT 0,
    supports_cache_breakdown INTEGER NOT NULL DEFAULT 0,
    source                   TEXT    NOT NULL,
    source_note              TEXT    NOT NULL DEFAULT '',
    updated_at               INTEGER NOT NULL,
    PRIMARY KEY (price_key, effective_from),
    CHECK (price_key = lower(trim(price_key)) AND price_key != ''),
    CHECK (effective_to IS NULL OR effective_to > effective_from),
    CHECK (long_context_threshold IS NULL OR long_context_threshold > 0),
    CHECK (long_context_inclusive IN (0, 1)),
    CHECK (supports_cache_breakdown IN (0, 1)),
    CHECK (source IN ('derived', 'admin')),
    CHECK (currency = 'USD')
);

CREATE INDEX idx_model_price_catalog_lookup
    ON model_price_catalog(price_key, effective_from DESC);

-- One row per (service tier, context tier). Unit: micro-USD per 1M tokens.
CREATE TABLE model_price_rates (
    price_key                    TEXT    NOT NULL,
    effective_from               INTEGER NOT NULL,
    service_tier                 TEXT    NOT NULL,
    context_tier                 TEXT    NOT NULL,
    input_micros_per_1m          INTEGER NOT NULL,
    output_micros_per_1m         INTEGER NOT NULL,
    cache_read_micros_per_1m     INTEGER NOT NULL,
    cache_write_5m_micros_per_1m INTEGER NOT NULL,
    -- NULL means upstream carries no usable 1h price for THIS rate row, so the
    -- §7.5 upper bound collapses onto the point estimate rather than inverting.
    cache_write_1h_micros_per_1m INTEGER,
    PRIMARY KEY (price_key, effective_from, service_tier, context_tier),
    FOREIGN KEY (price_key, effective_from)
        REFERENCES model_price_catalog(price_key, effective_from) ON DELETE CASCADE,
    CHECK (service_tier IN ('standard', 'priority', 'flex')),
    CHECK (context_tier IN ('base', 'long')),
    CHECK (input_micros_per_1m >= 0),
    CHECK (output_micros_per_1m >= 0),
    CHECK (cache_read_micros_per_1m >= 0),
    CHECK (cache_write_5m_micros_per_1m >= 0),
    CHECK (cache_write_1h_micros_per_1m IS NULL
           OR cache_write_1h_micros_per_1m >= cache_write_5m_micros_per_1m)
);

-- actual_model text as observed on the wire -> price_key.
-- Composite PK so the same pattern can exist as both exact and prefix
-- (exact always wins at lookup time; see model_price_catalog.rs).
CREATE TABLE model_price_aliases (
    pattern     TEXT    NOT NULL,
    match_kind  TEXT    NOT NULL,
    price_key   TEXT    NOT NULL,
    priority    INTEGER NOT NULL DEFAULT 0,
    source      TEXT    NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (pattern, match_kind),
    CHECK (pattern = lower(trim(pattern)) AND pattern != ''),
    CHECK (match_kind IN ('exact', 'prefix')),
    CHECK (price_key = lower(trim(price_key)) AND price_key != ''),
    CHECK (source IN ('derived', 'admin'))
);

CREATE INDEX idx_model_price_aliases_kind
    ON model_price_aliases(match_kind, priority DESC, length(pattern) DESC);

-- Public-surface rollup (R4). Anonymous readers must never trigger a raw window
-- scan of share_request_logs, so the aggregate half is materialised.
--
-- Deliberately stores TOKENS, not money: prices stay read-time so that fixing
-- the catalog once retroactively corrects all displayed history.
--
-- Degradation contract: this table is a cache, share_request_logs is the truth.
-- It may be DELETEd wholesale and rebuilt at any time; a missing bucket degrades
-- the public surface to "no usage mix" and MUST NOT fall back to a live scan.
CREATE TABLE share_listing_usage_rollup (
    share_id            TEXT    NOT NULL,
    bucket_start        INTEGER NOT NULL,
    model_key           TEXT    NOT NULL,
    service_tier        TEXT    NOT NULL,
    context_tier        TEXT    NOT NULL,
    input_tokens        INTEGER NOT NULL DEFAULT 0,
    output_tokens       INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
    unattributed_tokens INTEGER NOT NULL DEFAULT 0,
    distinct_users      INTEGER NOT NULL DEFAULT 0,
    request_count       INTEGER NOT NULL DEFAULT 0,
    computed_at         INTEGER NOT NULL,
    PRIMARY KEY (share_id, bucket_start, model_key, service_tier, context_tier),
    CHECK (bucket_start >= 0 AND bucket_start % 86400 = 0),
    CHECK (service_tier IN ('standard', 'priority', 'flex')),
    CHECK (context_tier IN ('base', 'long')),
    CHECK (input_tokens >= 0 AND output_tokens >= 0),
    CHECK (cache_read_tokens >= 0 AND cache_write_tokens >= 0),
    CHECK (unattributed_tokens >= 0),
    CHECK (distinct_users >= 0 AND request_count >= 0)
);

CREATE INDEX idx_share_listing_usage_rollup_window
    ON share_listing_usage_rollup(share_id, bucket_start DESC);
