-- Client-only termination terms. Existing paid periods deliberately have no
-- snapshot: a new fee must never be imposed retrospectively.
CREATE TABLE client_termination_policies (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000),
    actor_user_id TEXT,
    created_at TEXT NOT NULL
);
INSERT INTO client_termination_policies VALUES
    (1, 1000, NULL, strftime('%Y-%m-%dT%H:%M:%f+00:00','now'));
CREATE TABLE client_termination_quote_terms (
    quote_id TEXT PRIMARY KEY,
    policy_version INTEGER NOT NULL REFERENCES client_termination_policies(version),
    rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000)
);
CREATE TABLE client_termination_contract_terms (
    contract_id TEXT PRIMARY KEY REFERENCES market_platform_contracts(id),
    policy_version INTEGER NOT NULL REFERENCES client_termination_policies(version),
    rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000)
);
CREATE TABLE client_termination_period_terms (
    period_id TEXT PRIMARY KEY REFERENCES market_platform_periods(id),
    policy_version INTEGER NOT NULL REFERENCES client_termination_policies(version),
    rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000)
);
CREATE TRIGGER trg_client_termination_policy_immutable_update BEFORE UPDATE ON client_termination_policies
BEGIN SELECT RAISE(ABORT, 'Client termination policy is immutable'); END;
CREATE TRIGGER trg_client_termination_policy_immutable_delete BEFORE DELETE ON client_termination_policies
BEGIN SELECT RAISE(ABORT, 'Client termination policy is immutable'); END;
CREATE TRIGGER trg_client_termination_quote_immutable_update BEFORE UPDATE ON client_termination_quote_terms
BEGIN SELECT RAISE(ABORT, 'Client termination quote terms are immutable'); END;
CREATE TRIGGER trg_client_termination_quote_immutable_delete BEFORE DELETE ON client_termination_quote_terms
BEGIN SELECT RAISE(ABORT, 'Client termination quote terms are immutable'); END;
CREATE TRIGGER trg_client_termination_contract_immutable_update BEFORE UPDATE ON client_termination_contract_terms
BEGIN SELECT RAISE(ABORT, 'Client termination contract terms are immutable'); END;
CREATE TRIGGER trg_client_termination_contract_immutable_delete BEFORE DELETE ON client_termination_contract_terms
BEGIN SELECT RAISE(ABORT, 'Client termination contract terms are immutable'); END;
CREATE TRIGGER trg_client_termination_period_immutable_update BEFORE UPDATE ON client_termination_period_terms
BEGIN SELECT RAISE(ABORT, 'Client termination period terms are immutable'); END;
CREATE TRIGGER trg_client_termination_period_immutable_delete BEFORE DELETE ON client_termination_period_terms
BEGIN SELECT RAISE(ABORT, 'Client termination period terms are immutable'); END;
ALTER TABLE market_platform_periods ADD COLUMN termination_compensation_units
    INTEGER NOT NULL DEFAULT 0 CHECK (termination_compensation_units >= 0);
CREATE TRIGGER trg_client_termination_period_conservation
BEFORE UPDATE ON market_platform_periods
WHEN NEW.fulfilled_units + NEW.refunded_units + NEW.termination_compensation_units > NEW.amount_units
BEGIN SELECT RAISE(ABORT, 'Client period conservation violated'); END;

CREATE TABLE client_termination_previews (
    id TEXT PRIMARY KEY,
    installation_id TEXT NOT NULL,
    buyer_user_id TEXT NOT NULL,
    period_id TEXT,
    policy_version INTEGER NOT NULL,
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_client_termination_preview_expiry ON client_termination_previews(expires_at);
CREATE TABLE client_market_terminations (
    id TEXT PRIMARY KEY,
    installation_id TEXT NOT NULL UNIQUE,
    host_id TEXT NOT NULL,
    contract_id TEXT UNIQUE REFERENCES market_platform_contracts(id),
    period_id TEXT REFERENCES market_platform_periods(id),
    buyer_user_id TEXT NOT NULL,
    provider_user_id TEXT NOT NULL,
    policy_version INTEGER NOT NULL CHECK (policy_version >= 0),
    rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000),
    period_amount_units INTEGER NOT NULL CHECK (period_amount_units >= 0),
    remaining_units INTEGER NOT NULL CHECK (remaining_units >= 0),
    compensation_cap_units INTEGER NOT NULL CHECK (compensation_cap_units >= 0),
    returned_units INTEGER NOT NULL DEFAULT 0 CHECK (returned_units >= 0),
    compensation_units INTEGER NOT NULL DEFAULT 0 CHECK (compensation_units >= 0),
    held_units INTEGER NOT NULL DEFAULT 0 CHECK (held_units >= 0),
    reason TEXT NOT NULL,
    claim TEXT NOT NULL DEFAULT '',
    responsibility TEXT NOT NULL CHECK (responsibility IN ('buyer','provider','platform','undetermined','none')),
    status TEXT NOT NULL CHECK (status IN ('cleanup_pending','cleanup_failed','finance_pending','disputed','completed')),
    cutoff_at TEXT NOT NULL,
    cleanup_confirmed_at TEXT,
    cleanup_job_id TEXT,
    dispute_opened_at TEXT,
    provider_responded_at TEXT,
    dispute_used INTEGER NOT NULL DEFAULT 0 CHECK (dispute_used IN (0,1)),
    version INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    CHECK (compensation_cap_units <= remaining_units),
    CHECK (returned_units + compensation_units + held_units <= remaining_units),
    CHECK (dispute_used=0 OR dispute_opened_at IS NOT NULL),
    CHECK (provider_responded_at IS NULL OR (dispute_used=1 AND dispute_opened_at IS NOT NULL))
);
CREATE INDEX idx_client_termination_work ON client_market_terminations(status, updated_at, id);
CREATE INDEX idx_client_termination_buyer ON client_market_terminations(buyer_user_id, cutoff_at DESC);
CREATE INDEX idx_client_termination_provider ON client_market_terminations(provider_user_id, cutoff_at DESC);
CREATE INDEX idx_client_termination_dispute_due ON client_market_terminations(dispute_opened_at, id)
    WHERE responsibility='undetermined' AND provider_responded_at IS NULL;
CREATE TRIGGER trg_client_termination_snapshot_immutable BEFORE UPDATE ON client_market_terminations
WHEN NEW.id IS NOT OLD.id OR NEW.installation_id IS NOT OLD.installation_id OR NEW.host_id IS NOT OLD.host_id
 OR NEW.contract_id IS NOT OLD.contract_id OR NEW.period_id IS NOT OLD.period_id
 OR NEW.buyer_user_id IS NOT OLD.buyer_user_id OR NEW.provider_user_id IS NOT OLD.provider_user_id
 OR NEW.policy_version IS NOT OLD.policy_version OR NEW.rate_bps IS NOT OLD.rate_bps
 OR NEW.period_amount_units IS NOT OLD.period_amount_units OR NEW.remaining_units IS NOT OLD.remaining_units
 OR NEW.cutoff_at IS NOT OLD.cutoff_at OR NEW.reason IS NOT OLD.reason
 OR NEW.compensation_cap_units > OLD.compensation_cap_units
 OR (OLD.dispute_used=1 AND NEW.dispute_used!=1)
 OR (OLD.dispute_opened_at IS NOT NULL AND NEW.dispute_opened_at IS NOT OLD.dispute_opened_at)
 OR (OLD.provider_responded_at IS NOT NULL AND NEW.provider_responded_at IS NOT OLD.provider_responded_at)
BEGIN SELECT RAISE(ABORT, 'Client termination snapshot is immutable'); END;
CREATE TABLE client_cleanup_receipts (
    installation_id TEXT PRIMARY KEY,
    host_id TEXT NOT NULL,
    confirmed_at TEXT NOT NULL
);
-- These are awards, NOT delivery slices. No fabricated service evidence or
-- service commission is involved. The journal remains the source of money.
CREATE TABLE client_termination_compensations (
    id TEXT PRIMARY KEY REFERENCES client_market_terminations(id),
    provider_user_id TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0),
    reversed_units INTEGER NOT NULL DEFAULT 0 CHECK (reversed_units >= 0),
    credited_units INTEGER NOT NULL DEFAULT 0 CHECK (credited_units >= 0),
    disputed INTEGER NOT NULL DEFAULT 0 CHECK (disputed IN (0,1)),
    awarded_at TEXT NOT NULL,
    unlock_at TEXT NOT NULL,
    journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    credit_journal_id TEXT REFERENCES market_journal_transactions(id),
    CHECK (reversed_units + credited_units <= amount_units)
);
CREATE INDEX idx_client_compensation_due ON client_termination_compensations(unlock_at, id);
CREATE TRIGGER trg_client_compensation_award_immutable BEFORE UPDATE ON client_termination_compensations
WHEN NEW.id IS NOT OLD.id OR NEW.provider_user_id IS NOT OLD.provider_user_id
 OR NEW.amount_units IS NOT OLD.amount_units OR NEW.awarded_at IS NOT OLD.awarded_at
 OR NEW.unlock_at IS NOT OLD.unlock_at OR NEW.journal_id IS NOT OLD.journal_id
BEGIN SELECT RAISE(ABORT, 'Client compensation award is immutable'); END;
CREATE TABLE client_termination_events (
    id TEXT PRIMARY KEY,
    termination_id TEXT NOT NULL REFERENCES client_market_terminations(id),
    version INTEGER NOT NULL,
    actor_user_id TEXT,
    action TEXT NOT NULL,
    detail_json TEXT NOT NULL,
    journal_id TEXT REFERENCES market_journal_transactions(id),
    created_at TEXT NOT NULL
);
CREATE INDEX idx_client_termination_events ON client_termination_events(termination_id, version);
CREATE TRIGGER trg_client_termination_events_immutable_update BEFORE UPDATE ON client_termination_events
BEGIN SELECT RAISE(ABORT, 'Client termination audit is immutable'); END;
CREATE TRIGGER trg_client_termination_events_immutable_delete BEFORE DELETE ON client_termination_events
BEGIN SELECT RAISE(ABORT, 'Client termination audit is immutable'); END;
ALTER TABLE market_finance_control ADD COLUMN client_compensation_cursor TEXT NOT NULL DEFAULT '';
ALTER TABLE market_finance_control ADD COLUMN client_cleanup_finance_cursor TEXT NOT NULL DEFAULT '';
ALTER TABLE market_finance_control ADD COLUMN client_dispute_timeout_cursor TEXT NOT NULL DEFAULT '';

-- Read projection only. Awards are not inserted into delivery-backed lots.
CREATE VIEW market_provider_income_view AS
    SELECT l.id,l.provider_user_id,l.created_at,l.net_amount_units,l.amount_units,
        l.commission_units,l.commission_reversed_units,COALESCE(p.rate_bps,0) AS commission_rate_bps,
        COALESCE(p.policy_version,0) AS commission_policy_version,l.reversed_units,l.unlocked_units,
        l.held_units,l.converted_units,l.fulfilled_at,l.unlock_at,l.disputed,'service' AS income_kind,l.paid_units
    FROM market_provider_earning_lots l
    LEFT JOIN market_commission_splits s ON s.lot_id=l.id
    LEFT JOIN market_commission_periods p ON p.id=s.period_id
    UNION ALL
    SELECT c.id,c.provider_user_id,c.awarded_at,c.amount_units,c.amount_units,0,0,0,0,
        c.reversed_units,c.credited_units,0,c.credited_units,c.awarded_at,c.unlock_at,c.disputed,'client_termination',0
    FROM client_termination_compensations c;
