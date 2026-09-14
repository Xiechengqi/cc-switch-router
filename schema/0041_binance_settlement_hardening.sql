-- Binance Pay polling has no cursor and returns at most 100 rows. Persist the
-- in-progress time scan separately from the last fully covered watermark so a
-- busy account can resume a split window without skipping or restarting it.
-- Rebuild the account table to also admit the real outgoing-history UID proof
-- source; migration 0037 intentionally cannot be edited after release.

DROP INDEX idx_binance_payment_accounts_poll;
DROP INDEX uq_binance_payment_account_credential;
DROP INDEX uq_binance_payment_account_uid;
DROP INDEX uq_binance_payment_account_supplier_region;

CREATE TABLE binance_payment_accounts_v41 (
    id TEXT PRIMARY KEY,
    supplier_user_id TEXT NOT NULL,
    binance_uid TEXT NOT NULL,
    masked_api_key TEXT NOT NULL,
    credential_fingerprint TEXT NOT NULL,
    credentials_ciphertext TEXT NOT NULL,
    credential_nonce TEXT NOT NULL,
    encryption_key_version INTEGER NOT NULL CHECK (encryption_key_version > 0),
    credential_revision INTEGER NOT NULL DEFAULT 1 CHECK (credential_revision > 0),
    status TEXT NOT NULL CHECK (status IN ('verifying', 'verified', 'degraded', 'disabled')),
    automation_mode TEXT NOT NULL CHECK (automation_mode IN ('shadow', 'enabled')),
    payment_home_region TEXT NOT NULL,
    permissions_json TEXT NOT NULL DEFAULT '{}',
    permissions_verified_at TEXT,
    uid_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (uid_confirmed IN (0, 1)),
    uid_confirmation_source TEXT CHECK (
        uid_confirmation_source IS NULL OR
        uid_confirmation_source IN ('receiver_history', 'payer_history', 'payment_observation')
    ),
    last_poll_success_at TEXT,
    last_poll_error_code TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    degraded_since TEXT,
    poll_cursor_at TEXT,
    poll_scan_cursor_at TEXT,
    poll_scan_target_at TEXT,
    next_poll_at TEXT,
    lease_owner TEXT,
    lease_until TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (uid_confirmed = 0 AND uid_confirmation_source IS NULL) OR
        (uid_confirmed = 1 AND uid_confirmation_source IS NOT NULL)
    ),
    CHECK (
        (poll_scan_cursor_at IS NULL AND poll_scan_target_at IS NULL) OR
        (poll_scan_cursor_at IS NOT NULL AND poll_scan_target_at IS NOT NULL)
    )
);

INSERT INTO binance_payment_accounts_v41 (
    id, supplier_user_id, binance_uid, masked_api_key, credential_fingerprint,
    credentials_ciphertext, credential_nonce, encryption_key_version,
    credential_revision, status, automation_mode, payment_home_region,
    permissions_json, permissions_verified_at, uid_confirmed,
    uid_confirmation_source, last_poll_success_at, last_poll_error_code,
    consecutive_failures, degraded_since, poll_cursor_at, poll_scan_cursor_at,
    poll_scan_target_at, next_poll_at, lease_owner, lease_until, created_at,
    updated_at
)
SELECT
    id, supplier_user_id, binance_uid, masked_api_key, credential_fingerprint,
    credentials_ciphertext, credential_nonce, encryption_key_version,
    credential_revision, status, automation_mode, payment_home_region,
    permissions_json, permissions_verified_at, uid_confirmed,
    uid_confirmation_source, last_poll_success_at, last_poll_error_code,
    consecutive_failures,
    CASE WHEN status = 'degraded' THEN updated_at ELSE NULL END,
    poll_cursor_at, NULL, NULL, next_poll_at, lease_owner, lease_until,
    created_at, updated_at
FROM binance_payment_accounts;

DROP TABLE binance_payment_accounts;
ALTER TABLE binance_payment_accounts_v41 RENAME TO binance_payment_accounts;

CREATE UNIQUE INDEX uq_binance_payment_account_supplier_region
    ON binance_payment_accounts(supplier_user_id, payment_home_region);
CREATE UNIQUE INDEX uq_binance_payment_account_uid
    ON binance_payment_accounts(binance_uid);
CREATE UNIQUE INDEX uq_binance_payment_account_credential
    ON binance_payment_accounts(credential_fingerprint)
    WHERE credential_fingerprint != '';
CREATE INDEX idx_binance_payment_accounts_poll
    ON binance_payment_accounts(payment_home_region, status, next_poll_at, lease_until);
CREATE INDEX idx_binance_payment_accounts_degraded
    ON binance_payment_accounts(status, degraded_since, id);

-- Keep the two upstream identity namespaces separate. Existing fingerprints
-- predate that distinction and are explicitly labelled rather than silently
-- pretending that they all came from counterpartyId.
ALTER TABLE binance_pay_transactions
    ADD COLUMN payer_binance_id_fingerprint TEXT;
ALTER TABLE binance_pay_transactions
    ADD COLUMN counterparty_fingerprint_source TEXT CHECK (
        counterparty_fingerprint_source IS NULL OR
        counterparty_fingerprint_source IN ('counterparty_id', 'legacy_mixed')
    );
UPDATE binance_pay_transactions
   SET counterparty_fingerprint_source = 'legacy_mixed'
 WHERE counterparty_fingerprint IS NOT NULL;
