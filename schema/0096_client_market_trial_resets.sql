-- A Client trial is shared across rentals until a real rent payment opens the
-- next epoch. Share trials deliberately keep their lifetime accounting.
ALTER TABLE market_platform_contracts ADD COLUMN client_trial_epoch INTEGER NOT NULL DEFAULT 0
    CHECK (client_trial_epoch >= 0);
ALTER TABLE client_market_allocation_quote_items ADD COLUMN wallet_trial_epoch INTEGER NOT NULL DEFAULT 0
    CHECK (wallet_trial_epoch >= 0);
ALTER TABLE client_market_allocation_quote_items ADD COLUMN wallet_trial_consumed_seconds INTEGER NOT NULL DEFAULT 0
    CHECK (wallet_trial_consumed_seconds >= 0);

CREATE TABLE client_market_trial_scopes (
    buyer_user_id TEXT NOT NULL,
    provider_user_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    current_epoch INTEGER NOT NULL DEFAULT 0 CHECK (current_epoch >= 0),
    PRIMARY KEY (buyer_user_id, provider_user_id, host_id)
);
CREATE TABLE client_market_trial_resets (
    id TEXT PRIMARY KEY,
    buyer_user_id TEXT NOT NULL,
    provider_user_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    contract_id TEXT NOT NULL REFERENCES market_platform_contracts(id),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('monthly_period', 'metered_contract')),
    source_id TEXT NOT NULL,
    journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    created_at TEXT NOT NULL,
    FOREIGN KEY (buyer_user_id, provider_user_id, host_id)
        REFERENCES client_market_trial_scopes(buyer_user_id, provider_user_id, host_id),
    UNIQUE (source_kind, source_id),
    UNIQUE (buyer_user_id, provider_user_id, host_id, epoch)
);
CREATE INDEX idx_client_trial_reset_contract ON client_market_trial_resets(contract_id);
CREATE INDEX idx_client_trial_consumption
    ON market_platform_contracts(buyer_user_id, provider_user_id, trial_scope_ref, client_trial_epoch)
    WHERE product_kind = 'client_host';
CREATE TRIGGER trg_client_trial_epoch_immutable
    BEFORE UPDATE OF client_trial_epoch ON market_platform_contracts
    WHEN NEW.client_trial_epoch != OLD.client_trial_epoch
BEGIN
    SELECT RAISE(ABORT, 'Contract trial epoch is immutable');
END;

INSERT INTO client_market_trial_scopes (buyer_user_id, provider_user_id, host_id)
SELECT DISTINCT buyer_user_id, provider_user_id, trial_scope_ref
FROM market_platform_contracts WHERE product_kind = 'client_host';

-- Preserve every old consumption segment in epoch zero. Only still-running
-- rentals with a sealed, actual buyer debit receive an initial next-rental
-- entitlement; ended historical orders are not replayed as promotions.
INSERT INTO client_market_trial_resets
    (id,buyer_user_id,provider_user_id,host_id,epoch,contract_id,source_kind,source_id,journal_id,created_at)
SELECT 'upgrade:' || contract_id,buyer_user_id,provider_user_id,trial_scope_ref,1,
    contract_id,'monthly_period',period_id,journal_id,debited_at
FROM (
    SELECT c.id AS contract_id,c.buyer_user_id,c.provider_user_id,c.trial_scope_ref,
        p.id AS period_id,p.debit_journal_id AS journal_id,p.debited_at,
        ROW_NUMBER() OVER (PARTITION BY c.buyer_user_id,c.provider_user_id,c.trial_scope_ref
            ORDER BY julianday(p.debited_at) DESC,p.id DESC) AS position
    FROM market_platform_contracts c
    JOIN market_platform_periods p ON p.contract_id=c.id
    JOIN market_journal_transactions j ON j.id=p.debit_journal_id
        AND j.source_kind='platform_period' AND j.source_id=p.id
    JOIN market_journal_seals seal ON seal.journal_id=j.id
    WHERE c.product_kind='client_host' AND c.paid_started_at IS NOT NULL
        AND c.status IN ('active','suspended','ending','review_required')
        AND (SELECT COALESCE(SUM(signed_units),0) FROM market_journal_postings
            WHERE journal_id=j.id AND account_kind IN ('wallet_available','wallet_held')
                AND subject_id=c.buyer_user_id)=p.amount_units
) WHERE position=1;

INSERT INTO client_market_trial_resets
    (id,buyer_user_id,provider_user_id,host_id,epoch,contract_id,source_kind,source_id,journal_id,created_at)
SELECT 'upgrade:' || contract_id,buyer_user_id,provider_user_id,trial_scope_ref,1,
    contract_id,'metered_contract',contract_id,journal_id,paid_at
FROM (
    SELECT c.id AS contract_id,c.buyer_user_id,c.provider_user_id,c.trial_scope_ref,
        b.journal_id,b.created_at AS paid_at,
        ROW_NUMBER() OVER (PARTITION BY c.buyer_user_id,c.provider_user_id,c.trial_scope_ref
            ORDER BY julianday(b.created_at) DESC,b.id DESC) AS position
    FROM market_platform_contracts c
    JOIN market_usage_posting_batches b ON b.contract_id=c.id
    JOIN market_journal_transactions j ON j.id=b.journal_id
        AND j.source_kind='metered_batch' AND j.source_id=b.id
    JOIN market_journal_seals seal ON seal.journal_id=j.id
    WHERE c.product_kind='client_host' AND c.billing_model='metered'
        AND c.status IN ('active','suspended','ending','review_required')
        AND (SELECT COALESCE(SUM(signed_units),0) FROM market_journal_postings
            WHERE journal_id=j.id AND account_kind='wallet_held'
                AND subject_id=c.buyer_user_id)=b.amount_units
        AND NOT EXISTS (SELECT 1 FROM client_market_trial_resets r
            WHERE r.buyer_user_id=c.buyer_user_id AND r.provider_user_id=c.provider_user_id
                AND r.host_id=c.trial_scope_ref)
) WHERE position=1;

UPDATE client_market_trial_scopes SET current_epoch=1 WHERE EXISTS (
    SELECT 1 FROM client_market_trial_resets r
    WHERE r.buyer_user_id=client_market_trial_scopes.buyer_user_id
        AND r.provider_user_id=client_market_trial_scopes.provider_user_id
        AND r.host_id=client_market_trial_scopes.host_id
);

CREATE TRIGGER trg_client_trial_resets_no_update BEFORE UPDATE ON client_market_trial_resets BEGIN
    SELECT RAISE(ABORT, 'Client trial reset evidence is immutable');
END;
CREATE TRIGGER trg_client_trial_resets_no_delete BEFORE DELETE ON client_market_trial_resets BEGIN
    SELECT RAISE(ABORT, 'Client trial reset evidence is immutable');
END;
