-- Provider earning holds with an explicit cause.
--
-- Before this migration a lot was frozen by setting `disputed = 1`, and only a
-- service-dispute resolution could clear the flag. Lots frozen for an
-- uncertain cleanup or a missing service therefore stayed frozen forever.
-- Every hold now records its cause; `disputed` is kept as the derived
-- projection "has at least one active hold" so existing readers keep working.
CREATE TABLE market_provider_earning_holds (
    id TEXT PRIMARY KEY,
    lot_id TEXT NOT NULL REFERENCES market_provider_earning_lots(id),
    reason_kind TEXT NOT NULL CHECK (reason_kind IN (
        'dispute', 'cleanup_uncertain', 'service_missing', 'manual', 'legacy_unknown'
    )),
    reason_ref TEXT NOT NULL,
    created_at TEXT NOT NULL,
    released_at TEXT,
    released_by TEXT,
    release_reason TEXT
);
CREATE UNIQUE INDEX uq_market_provider_earning_hold_active
    ON market_provider_earning_holds(lot_id, reason_kind, reason_ref)
    WHERE released_at IS NULL;
CREATE INDEX idx_market_provider_earning_holds_reason
    ON market_provider_earning_holds(reason_kind, reason_ref, released_at);

-- Lots frozen by a still-open service dispute keep that cause.
INSERT INTO market_provider_earning_holds (id, lot_id, reason_kind, reason_ref, created_at)
SELECT lower(hex(randomblob(16))), link.earning_lot_id, 'dispute', dispute.id, dispute.created_at
FROM market_service_dispute_lots link
JOIN market_service_disputes dispute ON dispute.id = link.dispute_id
JOIN market_provider_earning_lots lot ON lot.id = link.earning_lot_id
WHERE dispute.status = 'open' AND lot.disputed = 1;

-- Lots of a contract with an open cleanup/missing-service case keep that cause.
INSERT OR IGNORE INTO market_provider_earning_holds (id, lot_id, reason_kind, reason_ref, created_at)
SELECT lower(hex(randomblob(16))), lot.id,
       CASE review.code WHEN 'wallet_cleanup_uncertain' THEN 'cleanup_uncertain'
                        ELSE 'service_missing' END,
       review.object_id, review.created_at
FROM market_finance_reconciliation_cases review
JOIN market_delivery_slices slice ON slice.contract_id = review.object_id
JOIN market_provider_earning_lots lot ON lot.delivery_slice_id = slice.id
WHERE review.status = 'open' AND review.object_kind = 'platform_contract'
  AND review.code IN ('wallet_cleanup_uncertain', 'wallet_service_missing')
  AND lot.disputed = 1;

-- Anything else that is frozen has no recorded cause: keep it frozen and ask
-- the Owner to review it explicitly.
INSERT INTO market_provider_earning_holds (id, lot_id, reason_kind, reason_ref, created_at)
SELECT lower(hex(randomblob(16))), lot.id, 'legacy_unknown', lot.provider_user_id, lot.created_at
FROM market_provider_earning_lots lot
WHERE lot.disputed = 1
  AND NOT EXISTS (SELECT 1 FROM market_provider_earning_holds hold
                  WHERE hold.lot_id = lot.id AND hold.released_at IS NULL);
INSERT OR IGNORE INTO market_finance_reconciliation_cases
    (id, code, object_kind, object_id, status, detail_code, created_at)
SELECT lower(hex(randomblob(16))), 'earning_hold_legacy', 'provider', provider_user_id,
       'open', 'legacy_unknown_hold', MIN(created_at)
FROM market_provider_earning_lots
WHERE id IN (SELECT lot_id FROM market_provider_earning_holds
             WHERE reason_kind = 'legacy_unknown' AND released_at IS NULL)
GROUP BY provider_user_id;
