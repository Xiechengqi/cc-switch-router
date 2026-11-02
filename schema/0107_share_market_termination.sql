-- Additive: old paid periods acquire neither fees nor a consumption tariff.
ALTER TABLE shares ADD COLUMN termination_archive_since_ms INTEGER;
ALTER TABLE market_platform_periods ADD COLUMN share_quota_units INTEGER NOT NULL DEFAULT 0 CHECK(share_quota_units>=0);
CREATE TRIGGER trg_share_period_conservation BEFORE UPDATE ON market_platform_periods
WHEN NEW.fulfilled_units+NEW.refunded_units+NEW.termination_compensation_units+NEW.share_quota_units>NEW.amount_units
BEGIN SELECT RAISE(ABORT,'Share period conservation violated'); END;
CREATE TABLE share_termination_policies (
    version INTEGER PRIMARY KEY, rate_bps INTEGER NOT NULL CHECK(rate_bps BETWEEN 0 AND 10000),
    actor_user_id TEXT NOT NULL, created_at TEXT NOT NULL
);
INSERT INTO share_termination_policies VALUES(1,1000,'system',strftime('%Y-%m-%dT%H:%M:%fZ','now'));
CREATE TRIGGER trg_share_termination_policy_update BEFORE UPDATE ON share_termination_policies
BEGIN SELECT RAISE(ABORT,'Share termination policy is immutable'); END;
CREATE TRIGGER trg_share_termination_policy_delete BEFORE DELETE ON share_termination_policies
BEGIN SELECT RAISE(ABORT,'Share termination policy is immutable'); END;
CREATE TABLE share_termination_quote_terms(quote_id TEXT PRIMARY KEY, policy_version INTEGER NOT NULL REFERENCES share_termination_policies(version), rate_bps INTEGER NOT NULL CHECK(rate_bps BETWEEN 0 AND 10000));
CREATE TABLE share_termination_contract_terms(contract_id TEXT PRIMARY KEY REFERENCES market_platform_contracts(id),policy_version INTEGER NOT NULL REFERENCES share_termination_policies(version),rate_bps INTEGER NOT NULL CHECK(rate_bps BETWEEN 0 AND 10000));
CREATE TABLE share_termination_period_terms(period_id TEXT PRIMARY KEY REFERENCES market_platform_periods(id),policy_version INTEGER NOT NULL REFERENCES share_termination_policies(version),rate_bps INTEGER NOT NULL CHECK(rate_bps BETWEEN 0 AND 10000));
CREATE TRIGGER trg_share_quote_terms_update BEFORE UPDATE ON share_termination_quote_terms BEGIN SELECT RAISE(ABORT,'immutable Share quote terms'); END;
CREATE TRIGGER trg_share_quote_terms_delete BEFORE DELETE ON share_termination_quote_terms BEGIN SELECT RAISE(ABORT,'immutable Share quote terms'); END;
CREATE TRIGGER trg_share_contract_terms_update BEFORE UPDATE ON share_termination_contract_terms BEGIN SELECT RAISE(ABORT,'immutable Share contract terms'); END;
CREATE TRIGGER trg_share_contract_terms_delete BEFORE DELETE ON share_termination_contract_terms BEGIN SELECT RAISE(ABORT,'immutable Share contract terms'); END;
CREATE TRIGGER trg_share_period_terms_update BEFORE UPDATE ON share_termination_period_terms BEGIN SELECT RAISE(ABORT,'immutable Share period terms'); END;
CREATE TRIGGER trg_share_period_terms_delete BEFORE DELETE ON share_termination_period_terms BEGIN SELECT RAISE(ABORT,'immutable Share period terms'); END;
CREATE TABLE share_termination_previews(
    id TEXT PRIMARY KEY,subscription_id TEXT NOT NULL,buyer_user_id TEXT NOT NULL,
    period_id TEXT,policy_version INTEGER NOT NULL,expires_at TEXT NOT NULL
);
CREATE INDEX idx_share_preview_expiry ON share_termination_previews(expires_at);
CREATE TABLE share_market_terminations(
    id TEXT PRIMARY KEY,subscription_id TEXT NOT NULL UNIQUE,share_id TEXT NOT NULL,
    contract_id TEXT UNIQUE,period_id TEXT,buyer_user_id TEXT NOT NULL,provider_user_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN('revoke_pending','finance_pending','disputed','completed')),
    cutoff_at TEXT NOT NULL,updated_at TEXT NOT NULL,version INTEGER NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    entitlement_id TEXT NOT NULL UNIQUE,
    CHECK(json_extract(payload_json,'$.id') IS id
      AND json_extract(payload_json,'$.subscriptionId') IS subscription_id
      AND json_extract(payload_json,'$.shareId') IS share_id
      AND json_extract(payload_json,'$.entitlementId') IS entitlement_id
      AND json_extract(payload_json,'$.buyerUserId') IS buyer_user_id
      AND json_extract(payload_json,'$.providerUserId') IS provider_user_id
      AND json_extract(payload_json,'$.contractId') IS contract_id
      AND json_extract(payload_json,'$.periodId') IS period_id
      AND json_extract(payload_json,'$.cutoffAt') IS cutoff_at
      AND json_extract(payload_json,'$.version') IS version
      AND json_extract(payload_json,'$.status') IS status)
);
CREATE INDEX idx_share_termination_buyer ON share_market_terminations(buyer_user_id,cutoff_at DESC,id DESC);
CREATE INDEX idx_share_termination_provider ON share_market_terminations(provider_user_id,status,cutoff_at DESC,id DESC);
CREATE INDEX idx_share_termination_resource ON share_market_terminations(share_id,status,id);
CREATE INDEX idx_share_termination_work ON share_market_terminations(status,id);
CREATE TRIGGER trg_share_termination_identity BEFORE UPDATE ON share_market_terminations
WHEN NEW.id IS NOT OLD.id OR NEW.subscription_id IS NOT OLD.subscription_id OR NEW.share_id IS NOT OLD.share_id
 OR NEW.contract_id IS NOT OLD.contract_id OR NEW.period_id IS NOT OLD.period_id
 OR NEW.buyer_user_id IS NOT OLD.buyer_user_id OR NEW.provider_user_id IS NOT OLD.provider_user_id
 OR NEW.cutoff_at IS NOT OLD.cutoff_at
 OR NEW.entitlement_id IS NOT OLD.entitlement_id
 OR json_extract(NEW.payload_json,'$.policyVersion') IS NOT json_extract(OLD.payload_json,'$.policyVersion')
 OR json_extract(NEW.payload_json,'$.rateBps') IS NOT json_extract(OLD.payload_json,'$.rateBps')
 OR json_extract(NEW.payload_json,'$.periodAmountUnits') IS NOT json_extract(OLD.payload_json,'$.periodAmountUnits')
 OR json_extract(NEW.payload_json,'$.entitlementId') IS NOT json_extract(OLD.payload_json,'$.entitlementId')
 OR json_extract(NEW.payload_json,'$.installationId') IS NOT json_extract(OLD.payload_json,'$.installationId')
 OR json_extract(NEW.payload_json,'$.policy') IS NOT json_extract(OLD.payload_json,'$.policy')
 OR json_extract(NEW.payload_json,'$.periodStartMs') IS NOT json_extract(OLD.payload_json,'$.periodStartMs')
 OR json_extract(NEW.payload_json,'$.periodEndMs') IS NOT json_extract(OLD.payload_json,'$.periodEndMs')
 OR json_extract(NEW.payload_json,'$.seatId') IS NOT json_extract(OLD.payload_json,'$.seatId')
 OR json_extract(NEW.payload_json,'$.buyerEmail') IS NOT json_extract(OLD.payload_json,'$.buyerEmail')
 OR json_extract(NEW.payload_json,'$.providerEmail') IS NOT json_extract(OLD.payload_json,'$.providerEmail')
 OR json_extract(NEW.payload_json,'$.timeEarnedUnits') IS NOT json_extract(OLD.payload_json,'$.timeEarnedUnits')
 OR json_extract(NEW.payload_json,'$.priorRefundedUnits') IS NOT json_extract(OLD.payload_json,'$.priorRefundedUnits')
BEGIN SELECT RAISE(ABORT,'Share termination identity is immutable'); END;
CREATE TABLE share_termination_receipts(
    termination_id TEXT PRIMARY KEY REFERENCES share_market_terminations(id),
    operation_id TEXT NOT NULL UNIQUE,payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),confirmed_at TEXT NOT NULL
);
CREATE TRIGGER trg_share_receipt_update BEFORE UPDATE ON share_termination_receipts BEGIN SELECT RAISE(ABORT,'immutable Share revoke receipt'); END;
CREATE TRIGGER trg_share_receipt_delete BEFORE DELETE ON share_termination_receipts BEGIN SELECT RAISE(ABORT,'immutable Share revoke receipt'); END;
CREATE TABLE share_termination_income(
    id TEXT PRIMARY KEY,termination_id TEXT NOT NULL REFERENCES share_market_terminations(id),
    provider_user_id TEXT NOT NULL,kind TEXT NOT NULL CHECK(kind='share_termination'),
    amount_units INTEGER NOT NULL CHECK(amount_units>0),commission_units INTEGER NOT NULL CHECK(commission_units=0),
    credited_units INTEGER NOT NULL DEFAULT 0 CHECK(credited_units IN(0,amount_units)),
    awarded_at TEXT NOT NULL,unlock_at TEXT NOT NULL,journal_id TEXT NOT NULL REFERENCES market_journal_transactions(id),
    credit_journal_id TEXT REFERENCES market_journal_transactions(id),
    UNIQUE(termination_id,kind),CHECK(commission_units+credited_units<=amount_units),
    CHECK((credited_units=0 AND credit_journal_id IS NULL) OR (credited_units=amount_units AND credit_journal_id IS NOT NULL))
);
CREATE INDEX idx_share_income_due ON share_termination_income(unlock_at,id);
CREATE TRIGGER trg_share_income_immutable BEFORE UPDATE ON share_termination_income
WHEN NEW.id IS NOT OLD.id OR NEW.termination_id IS NOT OLD.termination_id OR NEW.kind IS NOT OLD.kind
 OR NEW.provider_user_id IS NOT OLD.provider_user_id OR NEW.amount_units IS NOT OLD.amount_units
 OR NEW.commission_units IS NOT OLD.commission_units OR NEW.awarded_at IS NOT OLD.awarded_at
 OR NEW.unlock_at IS NOT OLD.unlock_at OR NEW.journal_id IS NOT OLD.journal_id
BEGIN SELECT RAISE(ABORT,'Share income award is immutable'); END;
CREATE TRIGGER trg_share_income_delete BEFORE DELETE ON share_termination_income
BEGIN SELECT RAISE(ABORT,'Share income award is immutable'); END;
CREATE TABLE share_termination_events(
    id TEXT PRIMARY KEY,termination_id TEXT NOT NULL REFERENCES share_market_terminations(id),version INTEGER NOT NULL,
    actor_user_id TEXT,action TEXT NOT NULL,detail_json TEXT NOT NULL,journal_id TEXT REFERENCES market_journal_transactions(id),created_at TEXT NOT NULL
);
CREATE INDEX idx_share_termination_events ON share_termination_events(termination_id,created_at,id);
CREATE TRIGGER trg_share_events_update BEFORE UPDATE ON share_termination_events BEGIN SELECT RAISE(ABORT,'immutable Share termination audit'); END;
CREATE TRIGGER trg_share_events_delete BEFORE DELETE ON share_termination_events BEGIN SELECT RAISE(ABORT,'immutable Share termination audit'); END;
ALTER TABLE market_finance_control ADD COLUMN share_termination_cursor TEXT NOT NULL DEFAULT '';
ALTER TABLE market_finance_control ADD COLUMN share_income_cursor TEXT NOT NULL DEFAULT '';
ALTER TABLE market_finance_control ADD COLUMN share_termination_audit_cursor TEXT NOT NULL DEFAULT '';
DROP VIEW market_provider_income_view;
CREATE VIEW market_provider_income_view AS
 SELECT l.id,l.provider_user_id,l.created_at,l.net_amount_units,l.amount_units,l.commission_units,l.commission_reversed_units,
 COALESCE(p.rate_bps,0) AS commission_rate_bps,COALESCE(p.policy_version,0) AS commission_policy_version,
 l.reversed_units,l.unlocked_units,l.held_units,l.converted_units,l.fulfilled_at,l.unlock_at,l.disputed,'service' AS income_kind,l.paid_units
 FROM market_provider_earning_lots l LEFT JOIN market_commission_splits s ON s.lot_id=l.id LEFT JOIN market_commission_periods p ON p.id=s.period_id
 UNION ALL SELECT c.id,c.provider_user_id,c.awarded_at,c.amount_units,c.amount_units,0,0,0,0,c.reversed_units,c.credited_units,0,c.credited_units,c.awarded_at,c.unlock_at,c.disputed,'client_termination',0
 FROM client_termination_compensations c
 UNION ALL SELECT i.id,i.provider_user_id,i.awarded_at,i.amount_units-i.commission_units,i.amount_units,i.commission_units,0,0,0,0,i.credited_units,0,i.credited_units,i.awarded_at,i.unlock_at,0,i.kind,0
 FROM share_termination_income i;
