-- C2C transfers circulate existing wallet liabilities, never mint top-up credit.
CREATE TABLE market_c2c_policy (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
    admission_minor INTEGER NOT NULL DEFAULT 10000 CHECK (admission_minor BETWEEN 100 AND 10000000),
    min_minor INTEGER NOT NULL DEFAULT 100 CHECK (min_minor > 0),
    max_minor INTEGER NOT NULL DEFAULT 500000 CHECK (max_minor >= min_minor AND max_minor <= 10000000),
    payment_minutes INTEGER NOT NULL DEFAULT 30 CHECK (payment_minutes BETWEEN 5 AND 60),
    audit_cursor TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
);
INSERT INTO market_c2c_policy(id,updated_at) VALUES(1,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
CREATE TABLE market_c2c_sellers (
    user_id TEXT PRIMARY KEY REFERENCES users(id),
    version INTEGER NOT NULL CHECK (version > 0),
    enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
    suspended INTEGER NOT NULL DEFAULT 0 CHECK (suspended IN (0,1)),
    display_name TEXT NOT NULL,
    contact TEXT NOT NULL,
    payment_method TEXT NOT NULL,
    payment_details TEXT NOT NULL,
    currency TEXT NOT NULL CHECK (currency IN ('USD','CNY')),
    price_bps INTEGER NOT NULL CHECK (price_bps BETWEEN 5000 AND 20000),
    min_minor INTEGER NOT NULL CHECK (min_minor > 0),
    max_minor INTEGER NOT NULL CHECK (max_minor >= min_minor AND max_minor <= 10000000),
    heartbeat_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_market_c2c_sellers_online ON market_c2c_sellers(enabled,suspended,heartbeat_at);
CREATE TABLE market_c2c_quotes (
    id TEXT PRIMARY KEY,
    buyer_id TEXT NOT NULL REFERENCES users(id),
    seller_id TEXT NOT NULL REFERENCES users(id),
    seller_version INTEGER NOT NULL,
    policy_version INTEGER NOT NULL,
    credit_minor INTEGER NOT NULL CHECK (credit_minor > 0),
    pay_minor INTEGER NOT NULL CHECK (pay_minor > 0),
    currency TEXT NOT NULL CHECK (currency IN ('USD','CNY')),
    fx_micros INTEGER NOT NULL CHECK (fx_micros > 0),
    price_bps INTEGER NOT NULL CHECK (price_bps BETWEEN 5000 AND 20000),
    seller_name TEXT NOT NULL,
    contact TEXT NOT NULL,
    payment_method TEXT NOT NULL,
    payment_details TEXT NOT NULL,
    payment_minutes INTEGER NOT NULL CHECK (payment_minutes BETWEEN 5 AND 60),
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    CHECK (buyer_id != seller_id)
);
CREATE INDEX idx_market_c2c_quotes_buyer ON market_c2c_quotes(buyer_id,created_at);
CREATE TABLE market_c2c_orders (
    id TEXT PRIMARY KEY,
    quote_id TEXT NOT NULL UNIQUE REFERENCES market_c2c_quotes(id),
    buyer_id TEXT NOT NULL REFERENCES users(id),
    seller_id TEXT NOT NULL REFERENCES users(id),
    idempotency_key TEXT NOT NULL,
    amount_units INTEGER NOT NULL CHECK (amount_units > 0 AND amount_units % 86400 = 0),
    hold_id TEXT NOT NULL UNIQUE REFERENCES market_wallet_holds(id),
    status TEXT NOT NULL CHECK (status IN ('awaiting_payment','paid','disputed','released','cancelled','expired')),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    terms_version INTEGER NOT NULL CHECK (terms_version = 1),
    payment_deadline TEXT NOT NULL,
    paid_at TEXT,
    released_at TEXT,
    release_journal_id TEXT UNIQUE REFERENCES market_journal_transactions(id),
    late_payment_reported INTEGER NOT NULL DEFAULT 0 CHECK (late_payment_reported IN (0,1)),
    late_payment_resolved INTEGER NOT NULL DEFAULT 0 CHECK (late_payment_resolved IN (0,1)),
    reminded_at TEXT,
    created_ip TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(buyer_id,idempotency_key),
    CHECK (buyer_id != seller_id),
    CHECK ((status = 'released' AND release_journal_id IS NOT NULL AND released_at IS NOT NULL)
        OR (status != 'released' AND release_journal_id IS NULL AND released_at IS NULL))
);
CREATE INDEX idx_market_c2c_orders_buyer ON market_c2c_orders(buyer_id,created_at,id);
CREATE INDEX idx_market_c2c_orders_seller ON market_c2c_orders(seller_id,status,created_at,id);
CREATE INDEX idx_market_c2c_orders_due ON market_c2c_orders(status,payment_deadline);
CREATE INDEX idx_market_c2c_orders_history ON market_c2c_orders(created_at,id);
CREATE INDEX idx_market_c2c_orders_buyer_active ON market_c2c_orders(buyer_id)
 WHERE status IN ('awaiting_payment','paid','disputed');
CREATE INDEX idx_market_c2c_orders_ip_active ON market_c2c_orders(created_ip)
 WHERE status IN ('awaiting_payment','paid','disputed');
CREATE TABLE market_c2c_events (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES market_c2c_orders(id),
    actor_id TEXT NOT NULL,
    action TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
);
CREATE INDEX idx_market_c2c_events_order ON market_c2c_events(order_id,created_at,id);
CREATE INDEX idx_market_c2c_events_actor ON market_c2c_events(actor_id,action,created_at);
CREATE TABLE market_c2c_challenges (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL REFERENCES market_c2c_orders(id),
    seller_id TEXT NOT NULL REFERENCES users(id),
    email TEXT NOT NULL,
    binding TEXT NOT NULL,
    code_mac TEXT NOT NULL,
    key_version INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    delivered_at TEXT,
    consumed_at TEXT,
    expires_at TEXT NOT NULL,
    created_ip TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_market_c2c_challenges_seller ON market_c2c_challenges(seller_id,created_at);
CREATE INDEX idx_market_c2c_challenges_ip ON market_c2c_challenges(created_ip,created_at);
CREATE TABLE market_c2c_attempts (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    ip TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_market_c2c_attempts_user ON market_c2c_attempts(user_id,created_at);
CREATE INDEX idx_market_c2c_attempts_ip ON market_c2c_attempts(ip,created_at);
CREATE TRIGGER trg_market_c2c_quote_no_update BEFORE UPDATE ON market_c2c_quotes
BEGIN SELECT RAISE(ABORT,'C2C quote is immutable'); END;
CREATE TRIGGER trg_market_c2c_quote_no_delete BEFORE DELETE ON market_c2c_quotes
WHEN EXISTS(SELECT 1 FROM market_c2c_orders WHERE quote_id = OLD.id)
BEGIN SELECT RAISE(ABORT,'consumed C2C quote is immutable'); END;
CREATE TRIGGER trg_market_c2c_order_terms BEFORE UPDATE OF
 id,quote_id,buyer_id,seller_id,idempotency_key,amount_units,hold_id,terms_version,payment_deadline,created_ip,created_at
 ON market_c2c_orders
BEGIN SELECT RAISE(ABORT,'C2C order terms are immutable'); END;
CREATE TRIGGER trg_market_c2c_order_no_delete BEFORE DELETE ON market_c2c_orders
BEGIN SELECT RAISE(ABORT,'C2C order is immutable'); END;
CREATE TRIGGER trg_market_c2c_order_transition BEFORE UPDATE OF status ON market_c2c_orders
WHEN NEW.status != OLD.status AND NOT (
 (OLD.status = 'awaiting_payment' AND NEW.status IN ('paid','cancelled','expired')) OR
 (OLD.status = 'paid' AND NEW.status IN ('disputed','released')) OR
 (OLD.status = 'disputed' AND NEW.status IN ('released','cancelled')))
BEGIN SELECT RAISE(ABORT,'invalid C2C transition'); END;
CREATE TRIGGER trg_market_c2c_event_no_update BEFORE UPDATE ON market_c2c_events
BEGIN SELECT RAISE(ABORT,'C2C event is immutable'); END;
CREATE TRIGGER trg_market_c2c_event_no_delete BEFORE DELETE ON market_c2c_events
BEGIN SELECT RAISE(ABORT,'C2C event is immutable'); END;

CREATE VIEW market_verified_c2c_reservation_evidence AS
SELECT o.id FROM market_c2c_orders o
JOIN market_wallet_holds h ON h.id=o.hold_id AND h.user_id=o.seller_id
 AND h.asset_code='USD' AND h.source_kind='c2c_order' AND h.source_id=o.id
 AND h.purpose='c2c' AND h.amount_units=o.amount_units
JOIN market_journal_transactions j ON j.source_kind='c2c_order' AND j.source_id=o.id
 AND j.action='reserve:c2c' AND j.fingerprint=o.seller_id||':'||o.amount_units||':c2c'
JOIN market_journal_seals s ON s.journal_id=j.id AND s.posting_count=2
WHERE (SELECT COUNT(*) FROM market_journal_postings p WHERE p.journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id
   AND p.account_kind='wallet_available' AND p.subject_id=o.seller_id AND p.signed_units=o.amount_units AND p.asset_code='USD')
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id
   AND p.account_kind='wallet_held' AND p.subject_id=o.seller_id AND p.signed_units=-o.amount_units AND p.asset_code='USD');

CREATE VIEW market_verified_c2c_return_evidence AS
SELECT o.id FROM market_c2c_orders o
JOIN market_wallet_holds h ON h.id=o.hold_id AND h.status='released' AND h.remaining_units=0
JOIN market_journal_transactions j ON j.source_kind='hold' AND j.source_id=h.id AND j.action='release'
 AND j.fingerprint=h.id||':'||o.amount_units||':release'
JOIN market_journal_seals s ON s.journal_id=j.id AND s.posting_count=2
WHERE o.status IN ('cancelled','expired')
 AND (SELECT COUNT(*) FROM market_journal_postings p WHERE p.journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id
   AND p.account_kind='wallet_held' AND p.subject_id=o.seller_id AND p.signed_units=o.amount_units AND p.asset_code='USD')
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id
   AND p.account_kind='wallet_available' AND p.subject_id=o.seller_id AND p.signed_units=-o.amount_units AND p.asset_code='USD');

CREATE VIEW market_verified_c2c_release_evidence AS
SELECT o.id FROM market_c2c_orders o
JOIN market_c2c_quotes q ON q.id=o.quote_id AND q.buyer_id=o.buyer_id AND q.seller_id=o.seller_id
JOIN market_wallet_holds h ON h.id=o.hold_id AND h.user_id=o.seller_id
 AND h.source_kind='c2c_order' AND h.source_id=o.id AND h.purpose='c2c'
 AND h.amount_units=o.amount_units AND h.status='captured' AND h.remaining_units=0
JOIN market_journal_transactions j ON j.id=o.release_journal_id
 AND j.source_kind='c2c_order' AND j.source_id=o.id AND j.action='release'
 AND j.fingerprint=o.seller_id||':'||o.buyer_id||':'||o.amount_units||':'||o.quote_id
JOIN market_journal_seals s ON s.journal_id=j.id AND s.posting_count=2
WHERE o.status='released' AND o.amount_units=q.credit_minor*86400 AND o.terms_version=1
 AND o.paid_at IS NOT NULL AND o.released_at IS NOT NULL
 AND (SELECT COUNT(*) FROM market_journal_postings p WHERE p.journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id
   AND p.account_kind='wallet_held' AND p.subject_id=o.seller_id AND p.signed_units=o.amount_units AND p.asset_code='USD')
 AND EXISTS(SELECT 1 FROM market_journal_postings p WHERE p.journal_id=j.id
   AND p.account_kind='wallet_available' AND p.subject_id=o.buyer_id AND p.signed_units=-o.amount_units AND p.asset_code='USD');
