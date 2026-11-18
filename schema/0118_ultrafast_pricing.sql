-- Keep released migration checksums intact. SQLite requires rebuilding these
-- tables to admit an independent Ultrafast tariff and retain its usage tier.
CREATE TABLE model_price_rates_new (
    price_key                    TEXT    NOT NULL,
    effective_from               INTEGER NOT NULL,
    service_tier                 TEXT    NOT NULL,
    context_tier                 TEXT    NOT NULL,
    input_micros_per_1m          INTEGER NOT NULL,
    output_micros_per_1m         INTEGER NOT NULL,
    cache_read_micros_per_1m     INTEGER NOT NULL,
    cache_write_5m_micros_per_1m INTEGER NOT NULL,
    cache_write_1h_micros_per_1m INTEGER,
    PRIMARY KEY (price_key, effective_from, service_tier, context_tier),
    FOREIGN KEY (price_key, effective_from)
        REFERENCES model_price_catalog(price_key, effective_from) ON DELETE CASCADE,
    CHECK (service_tier IN ('standard', 'priority', 'flex', 'ultrafast')),
    CHECK (context_tier IN ('base', 'long')),
    CHECK (input_micros_per_1m >= 0),
    CHECK (output_micros_per_1m >= 0),
    CHECK (cache_read_micros_per_1m >= 0),
    CHECK (cache_write_5m_micros_per_1m >= 0),
    CHECK (cache_write_1h_micros_per_1m IS NULL
           OR cache_write_1h_micros_per_1m >= cache_write_5m_micros_per_1m)
);

INSERT INTO model_price_rates_new (
    price_key, effective_from, service_tier, context_tier,
    input_micros_per_1m, output_micros_per_1m, cache_read_micros_per_1m,
    cache_write_5m_micros_per_1m, cache_write_1h_micros_per_1m
)
SELECT price_key, effective_from, service_tier, context_tier,
       input_micros_per_1m, output_micros_per_1m, cache_read_micros_per_1m,
       cache_write_5m_micros_per_1m, cache_write_1h_micros_per_1m
FROM model_price_rates;

DROP TABLE model_price_rates;
ALTER TABLE model_price_rates_new RENAME TO model_price_rates;

CREATE TABLE share_listing_usage_rollup_new (
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
    CHECK (service_tier IN ('standard', 'priority', 'flex', 'ultrafast')),
    CHECK (context_tier IN ('base', 'long')),
    CHECK (input_tokens >= 0 AND output_tokens >= 0),
    CHECK (cache_read_tokens >= 0 AND cache_write_tokens >= 0),
    CHECK (unattributed_tokens >= 0),
    CHECK (distinct_users >= 0 AND request_count >= 0)
);

INSERT INTO share_listing_usage_rollup_new (
    share_id, bucket_start, model_key, service_tier, context_tier,
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
    unattributed_tokens, distinct_users, request_count, computed_at
)
SELECT share_id, bucket_start, model_key, service_tier, context_tier,
       input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
       unattributed_tokens, distinct_users, request_count, computed_at
FROM share_listing_usage_rollup;

DROP TABLE share_listing_usage_rollup;
ALTER TABLE share_listing_usage_rollup_new RENAME TO share_listing_usage_rollup;

CREATE INDEX idx_share_listing_usage_rollup_window
    ON share_listing_usage_rollup(share_id, bucket_start DESC);
