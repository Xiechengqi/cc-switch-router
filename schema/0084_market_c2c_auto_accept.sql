-- Opening a listing now authorizes automatic orders. Existing selling intent
-- is preserved, but no new quote/order is allowed until explicit new consent.
ALTER TABLE market_c2c_sellers ADD COLUMN auto_accept_terms_version INTEGER NOT NULL DEFAULT 0 CHECK(auto_accept_terms_version IN (0,3));
ALTER TABLE market_c2c_sellers ADD COLUMN auto_accept_consented_at TEXT
 CHECK((auto_accept_terms_version=0 AND auto_accept_consented_at IS NULL)
    OR (auto_accept_terms_version=3 AND julianday(auto_accept_consented_at) IS NOT NULL));
ALTER TABLE market_c2c_policy DROP COLUMN acceptance_minutes;

-- As in migration 83, rebuild the CHECK-constrained financial parents and
-- their children atomically with foreign keys enabled. Historical evidence,
-- deadlines, versions and challenges are copied verbatim, never reinterpreted.
CREATE TEMP TABLE c2c84_orders_backup AS SELECT * FROM market_c2c_orders;
CREATE TEMP TABLE c2c84_events_backup AS SELECT * FROM market_c2c_events;
CREATE TEMP TABLE c2c84_challenges_backup AS SELECT * FROM market_c2c_challenges;
DROP TABLE market_c2c_events;
DROP TABLE market_c2c_challenges;
DROP VIEW market_verified_c2c_reservation_evidence;
DROP VIEW market_verified_c2c_return_evidence;
DROP VIEW market_verified_c2c_release_evidence;
DROP TRIGGER trg_market_c2c_quote_no_delete;
DROP TABLE market_c2c_orders;
CREATE TABLE market_c2c_quotes_next (
 id TEXT PRIMARY KEY,
 buyer_id TEXT NOT NULL REFERENCES users(id), seller_id TEXT NOT NULL REFERENCES users(id),
 seller_version INTEGER NOT NULL, policy_version INTEGER NOT NULL,
 credit_minor INTEGER NOT NULL CHECK(credit_minor>0), pay_minor INTEGER NOT NULL CHECK(pay_minor>0),
 currency TEXT NOT NULL CHECK(currency IN ('USD','CNY')), fx_micros INTEGER NOT NULL CHECK(fx_micros>0),
 price_bps INTEGER NOT NULL CHECK(price_bps BETWEEN 5000 AND 20000), seller_name TEXT NOT NULL,
 contact TEXT NOT NULL, payment_method TEXT NOT NULL, payment_details TEXT NOT NULL,
 payment_minutes INTEGER NOT NULL CHECK(payment_minutes BETWEEN 5 AND 60), expires_at TEXT NOT NULL, created_at TEXT NOT NULL,
 description TEXT NOT NULL DEFAULT '' CHECK(length(description)<=300),
 flow_version INTEGER NOT NULL DEFAULT 1 CHECK(flow_version IN (1,2,3)),
 acceptance_minutes INTEGER NOT NULL DEFAULT 10 CHECK(acceptance_minutes BETWEEN 5 AND 30),
 auto_accept_authorized_at TEXT,
 CHECK(buyer_id!=seller_id), CHECK(flow_version!=3 OR (julianday(auto_accept_authorized_at) IS NOT NULL AND julianday(auto_accept_authorized_at)<=julianday(created_at)))
);
INSERT INTO market_c2c_quotes_next SELECT *,NULL FROM market_c2c_quotes;
DROP TABLE market_c2c_quotes;
ALTER TABLE market_c2c_quotes_next RENAME TO market_c2c_quotes;
CREATE INDEX idx_market_c2c_quotes_buyer ON market_c2c_quotes(buyer_id,created_at);
CREATE TRIGGER trg_market_c2c_quote_no_update BEFORE UPDATE ON market_c2c_quotes
 BEGIN SELECT RAISE(ABORT,'C2C quote is immutable'); END;

CREATE TABLE market_c2c_orders (
 id TEXT PRIMARY KEY, quote_id TEXT NOT NULL UNIQUE REFERENCES market_c2c_quotes(id),
 buyer_id TEXT NOT NULL REFERENCES users(id), seller_id TEXT NOT NULL REFERENCES users(id), idempotency_key TEXT NOT NULL,
 amount_units INTEGER NOT NULL CHECK(amount_units>0 AND amount_units%86400=0),
 hold_id TEXT NOT NULL UNIQUE REFERENCES market_wallet_holds(id),
 status TEXT NOT NULL CHECK(status IN ('awaiting_seller','awaiting_payment','paid','disputed','released','cancelled','expired')),
 version INTEGER NOT NULL DEFAULT 1 CHECK(version>0), terms_version INTEGER NOT NULL CHECK(terms_version IN (1,2,3)),
 payment_deadline TEXT, paid_at TEXT, released_at TEXT,
 release_journal_id TEXT UNIQUE REFERENCES market_journal_transactions(id),
 late_payment_reported INTEGER NOT NULL DEFAULT 0 CHECK(late_payment_reported IN (0,1)),
 late_payment_resolved INTEGER NOT NULL DEFAULT 0 CHECK(late_payment_resolved IN (0,1)),
 reminded_at TEXT, created_ip TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 acceptance_deadline TEXT, accepted_at TEXT, seller_paused_at TEXT, escalated_at TEXT,
 UNIQUE(buyer_id,idempotency_key), CHECK(buyer_id!=seller_id),
 CHECK((status='released' AND release_journal_id IS NOT NULL AND released_at IS NOT NULL)
    OR (status!='released' AND release_journal_id IS NULL AND released_at IS NULL)),
 CHECK((terms_version IN (1,3) AND payment_deadline IS NOT NULL AND acceptance_deadline IS NULL AND accepted_at IS NULL AND status!='awaiting_seller')
    OR (terms_version=2 AND acceptance_deadline IS NOT NULL
      AND ((accepted_at IS NULL AND payment_deadline IS NULL AND status IN ('awaiting_seller','disputed','released','cancelled','expired'))
        OR (accepted_at IS NOT NULL AND payment_deadline IS NOT NULL AND status!='awaiting_seller'))))
);
INSERT INTO market_c2c_orders SELECT * FROM c2c84_orders_backup;
DROP TABLE c2c84_orders_backup;
CREATE TABLE market_c2c_events (
 id TEXT PRIMARY KEY, order_id TEXT NOT NULL REFERENCES market_c2c_orders(id),
 actor_id TEXT NOT NULL, action TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL
);
INSERT INTO market_c2c_events SELECT * FROM c2c84_events_backup;
DROP TABLE c2c84_events_backup;
CREATE INDEX idx_market_c2c_events_order ON market_c2c_events(order_id,created_at,id);
CREATE INDEX idx_market_c2c_events_actor ON market_c2c_events(actor_id,action,created_at);
CREATE TRIGGER trg_market_c2c_event_no_update BEFORE UPDATE ON market_c2c_events
 BEGIN SELECT RAISE(ABORT,'C2C event is immutable'); END;
CREATE TRIGGER trg_market_c2c_event_no_delete BEFORE DELETE ON market_c2c_events
 BEGIN SELECT RAISE(ABORT,'C2C event is immutable'); END;
CREATE TABLE market_c2c_challenges (
 id TEXT PRIMARY KEY, order_id TEXT NOT NULL REFERENCES market_c2c_orders(id),
 seller_id TEXT NOT NULL REFERENCES users(id), email TEXT NOT NULL, binding TEXT NOT NULL,
 code_mac TEXT NOT NULL, key_version INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0),
 delivered_at TEXT, consumed_at TEXT, expires_at TEXT NOT NULL, created_ip TEXT NOT NULL, created_at TEXT NOT NULL
);
INSERT INTO market_c2c_challenges SELECT * FROM c2c84_challenges_backup;
DROP TABLE c2c84_challenges_backup;
CREATE INDEX idx_market_c2c_challenges_seller ON market_c2c_challenges(seller_id,created_at);
CREATE INDEX idx_market_c2c_challenges_ip ON market_c2c_challenges(created_ip,created_at);
CREATE TRIGGER trg_market_c2c_quote_no_delete BEFORE DELETE ON market_c2c_quotes
 WHEN EXISTS(SELECT 1 FROM market_c2c_orders WHERE quote_id=OLD.id)
 BEGIN SELECT RAISE(ABORT,'consumed C2C quote is immutable'); END;
CREATE INDEX idx_market_c2c_orders_buyer ON market_c2c_orders(buyer_id,created_at,id);
CREATE INDEX idx_market_c2c_orders_seller ON market_c2c_orders(seller_id,status,created_at,id);
CREATE INDEX idx_market_c2c_orders_due ON market_c2c_orders(status,payment_deadline);
CREATE INDEX idx_market_c2c_orders_acceptance ON market_c2c_orders(status,acceptance_deadline);
CREATE INDEX idx_market_c2c_orders_history ON market_c2c_orders(created_at,id);
CREATE INDEX idx_market_c2c_orders_buyer_active ON market_c2c_orders(buyer_id) WHERE status IN ('awaiting_seller','awaiting_payment','paid','disputed');
CREATE INDEX idx_market_c2c_orders_ip_active ON market_c2c_orders(created_ip) WHERE status IN ('awaiting_seller','awaiting_payment','paid','disputed');
CREATE TRIGGER trg_market_c2c_order_terms BEFORE UPDATE OF id,quote_id,buyer_id,seller_id,idempotency_key,amount_units,hold_id,terms_version,acceptance_deadline,created_ip,created_at ON market_c2c_orders
 BEGIN SELECT RAISE(ABORT,'C2C order terms are immutable'); END;
CREATE TRIGGER trg_market_c2c_order_auto_accept BEFORE INSERT ON market_c2c_orders
 WHEN NEW.terms_version=3 AND (NEW.status='awaiting_payment' AND NEW.paid_at IS NULL
  AND EXISTS(SELECT 1 FROM market_c2c_quotes q WHERE q.id=NEW.quote_id AND q.flow_version=3
   AND q.buyer_id=NEW.buyer_id AND q.seller_id=NEW.seller_id AND q.credit_minor*86400=NEW.amount_units
   AND julianday(q.auto_accept_authorized_at) IS NOT NULL
   AND abs((julianday(NEW.payment_deadline)-julianday(NEW.created_at))*1440-q.payment_minutes)<0.001)) IS NOT 1
 BEGIN SELECT RAISE(ABORT,'C2C automatic order must bind reserved terms and payment window'); END;
CREATE TRIGGER trg_market_c2c_order_acceptance BEFORE UPDATE OF accepted_at,payment_deadline ON market_c2c_orders
 WHEN (OLD.terms_version=2 AND OLD.status='awaiting_seller' AND NEW.status='awaiting_payment'
  AND OLD.accepted_at IS NULL AND OLD.payment_deadline IS NULL AND NEW.accepted_at IS NOT NULL
  AND julianday(NEW.accepted_at)<julianday(OLD.acceptance_deadline)
  AND julianday(NEW.payment_deadline)>julianday(NEW.accepted_at)
  AND abs((julianday(NEW.payment_deadline)-julianday(NEW.accepted_at))*1440-(SELECT payment_minutes FROM market_c2c_quotes WHERE id=OLD.quote_id))<0.001) IS NOT 1
 BEGIN SELECT RAISE(ABORT,'C2C acceptance and payment deadline are frozen once'); END;
CREATE TRIGGER trg_market_c2c_order_no_delete BEFORE DELETE ON market_c2c_orders
 BEGIN SELECT RAISE(ABORT,'C2C order is immutable'); END;
CREATE TRIGGER trg_market_c2c_order_transition BEFORE UPDATE OF status ON market_c2c_orders
 WHEN NEW.status!=OLD.status AND NOT(
  (OLD.status='awaiting_seller' AND NEW.status IN ('awaiting_payment','disputed','cancelled','expired')) OR
  (OLD.status='awaiting_payment' AND NEW.status IN ('paid','cancelled','expired')) OR
  (OLD.status='paid' AND NEW.status IN ('disputed','released')) OR
  (OLD.status='disputed' AND NEW.status IN ('released','cancelled')))
 BEGIN SELECT RAISE(ABORT,'invalid C2C transition'); END;

INSERT INTO market_notification_outbox(id,audience,recipient_user_id,event_kind,object_kind,object_id,dedupe_key,payload_json,mandatory,status,next_attempt_at,created_at)
 SELECT lower(hex(randomblob(16))),'buyer',user_id,'c2c_seller_changed','c2c_seller',user_id,'c2c-auto-accept-upgrade:'||user_id,'{"reason":"auto_accept_consent_required"}',1,'queued',strftime('%Y-%m-%dT%H:%M:%fZ','now'),strftime('%Y-%m-%dT%H:%M:%fZ','now')
 FROM market_c2c_sellers WHERE enabled=1;

CREATE VIEW market_verified_c2c_reservation_evidence AS
 SELECT o.id FROM market_c2c_orders o
 JOIN market_wallet_holds h ON h.id=o.hold_id AND h.user_id=o.seller_id AND h.asset_code='USD' AND h.source_kind='c2c_order' AND h.source_id=o.id AND h.purpose='c2c' AND h.amount_units=o.amount_units
 JOIN market_journal_transactions j ON j.source_kind='c2c_order' AND j.source_id=o.id AND j.action='reserve:c2c' AND j.fingerprint=o.seller_id||':'||o.amount_units||':c2c'
 JOIN market_journal_seals s ON s.journal_id=j.id AND s.posting_count=2
 WHERE (SELECT COUNT(*) FROM market_journal_postings p WHERE p.journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_available' AND p.subject_id=o.seller_id AND p.signed_units=o.amount_units AND p.asset_code='USD')
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_held' AND p.subject_id=o.seller_id AND p.signed_units=-o.amount_units AND p.asset_code='USD');
CREATE VIEW market_verified_c2c_return_evidence AS
 SELECT o.id FROM market_c2c_orders o
 JOIN market_wallet_holds h ON h.id=o.hold_id AND h.status='released' AND h.remaining_units=0
 JOIN market_journal_transactions j ON j.source_kind='hold' AND j.source_id=h.id AND j.action='release' AND j.fingerprint=h.id||':'||o.amount_units||':release'
 JOIN market_journal_seals s ON s.journal_id=j.id AND s.posting_count=2
 WHERE o.status IN ('cancelled','expired') AND (SELECT COUNT(*) FROM market_journal_postings p WHERE p.journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_held' AND p.subject_id=o.seller_id AND p.signed_units=o.amount_units AND p.asset_code='USD')
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_available' AND p.subject_id=o.seller_id AND p.signed_units=-o.amount_units AND p.asset_code='USD');
CREATE VIEW market_verified_c2c_release_evidence AS
 SELECT o.id FROM market_c2c_orders o JOIN market_c2c_quotes q ON q.id=o.quote_id AND q.buyer_id=o.buyer_id AND q.seller_id=o.seller_id
 JOIN market_wallet_holds h ON h.id=o.hold_id AND h.user_id=o.seller_id AND h.source_kind='c2c_order' AND h.source_id=o.id AND h.purpose='c2c' AND h.amount_units=o.amount_units AND h.status='captured' AND h.remaining_units=0
 JOIN market_journal_transactions j ON j.id=o.release_journal_id AND j.source_kind='c2c_order' AND j.source_id=o.id AND j.action='release' AND j.fingerprint=o.seller_id||':'||o.buyer_id||':'||o.amount_units||':'||o.quote_id
 JOIN market_journal_seals s ON s.journal_id=j.id AND s.posting_count=2
 WHERE o.status='released' AND o.amount_units=q.credit_minor*86400 AND o.terms_version=q.flow_version AND o.paid_at IS NOT NULL AND o.released_at IS NOT NULL
 AND (SELECT COUNT(*) FROM market_journal_postings p WHERE p.journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_held' AND p.subject_id=o.seller_id AND p.signed_units=o.amount_units AND p.asset_code='USD')
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id AND p.account_kind='wallet_available' AND p.subject_id=o.buyer_id AND p.signed_units=-o.amount_units AND p.asset_code='USD');
