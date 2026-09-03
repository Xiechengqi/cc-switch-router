use chrono::{DateTime, Duration, TimeZone, Utc};
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::client_market_trade::PaymentMethod;
use crate::db::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use crate::error::AppError;
use crate::market_billing::BillingAction;
use crate::models::AuthSession;
use crate::store::AppStore;

use super::client::{
    BinancePayTransaction, VerificationResult, parse_decimal_units, value_as_identifier,
};
use super::crypto::{BinanceCredentials, CredentialCipher, credential_aad, transaction_aad};

pub const PAYMENT_ASSET: &str = "USDT";
pub const PAYMENT_AMOUNT_SCALE: i64 = 10_000;
const INTENT_TTL_MINUTES: i64 = 15;
const LATE_GRACE_HOURS: i64 = 24;
const AMOUNT_COOLDOWN_HOURS: i64 = 24;
const POLL_LEASE_SECONDS: i64 = 240;
const MAX_POLL_BACKOFF_SECONDS: i64 = 3 * 24 * 60 * 60;
const IP_BAN_DEFAULT_BACKOFF_SECONDS: i64 = 60 * 60;
const RATE_LIMIT_DEFAULT_BACKOFF_SECONDS: i64 = 60;
const PAYMENT_CLOCK_SKEW_SECONDS: i64 = 120;
const INTENT_REFRESH_MIN_SECONDS: i64 = 30;
const MAX_INTENTS_PER_INVOICE_24H: i64 = 6;
const MAX_INTENTS_PER_BUYER_ACCOUNT_24H: i64 = 30;
const MIN_SUFFIX: i64 = 1;
const MAX_SUFFIX: i64 = 99;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinancePaymentAccountView {
    pub binance_uid: String,
    pub masked_api_key: String,
    pub status: String,
    pub automation_mode: String,
    pub payment_home_region: String,
    pub permissions_verified_at: Option<String>,
    pub uid_confirmed: bool,
    pub uid_confirmation_source: Option<String>,
    pub last_poll_success_at: Option<String>,
    pub last_poll_error_code: Option<String>,
    pub consecutive_failures: i64,
    pub credential_revision: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinancePaymentIntentView {
    pub id: String,
    pub invoice_id: String,
    pub status: String,
    pub asset: String,
    pub base_amount: String,
    pub pay_amount: String,
    pub receiver_uid: String,
    pub note_code: String,
    pub expires_at: String,
    pub created_at: String,
    pub paid_at: Option<String>,
    pub cancellation_reason: Option<String>,
    pub account_status: String,
    pub last_checked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceReconciliationCaseView {
    pub id: String,
    pub invoice_id: Option<String>,
    pub payment_intent_id: Option<String>,
    pub payment_account_id: String,
    pub transaction_id: String,
    pub case_kind: String,
    pub status: String,
    pub detail: serde_json::Value,
    pub supplier_user_id: String,
    pub binance_uid: String,
    pub transaction_time: String,
    pub asset: String,
    pub amount: String,
    pub invoice_status: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceSettlementAdminView {
    pub cases: Vec<BinanceReconciliationCaseView>,
    pub open_case_count: i64,
    pub pending_intent_count: i64,
    pub degraded_account_count: i64,
    pub oldest_open_case_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoredPaymentAccount {
    pub id: String,
    pub supplier_user_id: String,
    pub binance_uid: String,
    pub credentials_ciphertext: String,
    pub credential_nonce: String,
    pub encryption_key_version: i64,
    pub credential_revision: i64,
    pub automation_mode: String,
    pub permissions_verified_at: Option<String>,
    pub poll_cursor_at: Option<String>,
    pub active_intent_started_at: Option<String>,
}

#[derive(Debug)]
pub struct AccountCredentialEnvelope {
    pub account: StoredPaymentAccount,
    pub credentials: BinanceCredentials,
}

impl AppStore {
    pub async fn binance_load_payment_account(
        &self,
        supplier_user_id: &str,
        region: &str,
    ) -> Result<Option<StoredPaymentAccount>, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT id, supplier_user_id, binance_uid, credentials_ciphertext,
                    credential_nonce, encryption_key_version, credential_revision,
                    automation_mode, permissions_verified_at, poll_cursor_at, NULL
             FROM binance_payment_accounts
             WHERE supplier_user_id = ?1 AND payment_home_region = ?2
               AND status != 'disabled' AND credentials_ciphertext != ''",
            params![supplier_user_id, region],
            |row| {
                Ok(StoredPaymentAccount {
                    id: row.get(0)?,
                    supplier_user_id: row.get(1)?,
                    binance_uid: row.get(2)?,
                    credentials_ciphertext: row.get(3)?,
                    credential_nonce: row.get(4)?,
                    encryption_key_version: row.get(5)?,
                    credential_revision: row.get(6)?,
                    automation_mode: row.get(7)?,
                    permissions_verified_at: row.get(8)?,
                    poll_cursor_at: row.get(9)?,
                    active_intent_started_at: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(map_db("load Binance payment account"))
    }

    pub async fn binance_payment_account_view(
        &self,
        supplier_user_id: &str,
        region: &str,
    ) -> Result<Option<BinancePaymentAccountView>, AppError> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT binance_uid, masked_api_key, status, automation_mode,
                    payment_home_region, permissions_verified_at, uid_confirmed,
                    uid_confirmation_source, last_poll_success_at,
                    last_poll_error_code, consecutive_failures,
                    credential_revision, updated_at
             FROM binance_payment_accounts
             WHERE supplier_user_id = ?1 AND payment_home_region = ?2",
            params![supplier_user_id, region],
            |row| {
                Ok(BinancePaymentAccountView {
                    binance_uid: row.get(0)?,
                    masked_api_key: row.get(1)?,
                    status: row.get(2)?,
                    automation_mode: row.get(3)?,
                    payment_home_region: row.get(4)?,
                    permissions_verified_at: row.get(5)?,
                    uid_confirmed: row.get::<_, i64>(6)? != 0,
                    uid_confirmation_source: row.get(7)?,
                    last_poll_success_at: row.get(8)?,
                    last_poll_error_code: row.get(9)?,
                    consecutive_failures: row.get(10)?,
                    credential_revision: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            },
        )
        .optional()
        .map_err(map_db("read Binance payment account"))
    }

    pub async fn binance_prepare_account_binding(
        &self,
        supplier_user_id: &str,
        region: &str,
        binance_uid: &str,
    ) -> Result<(String, i64), AppError> {
        let conn = self.conn.lock().await;
        ensure_public_binance_method(&conn, supplier_user_id, binance_uid)?;
        let uid_owner = conn
            .query_row(
                "SELECT supplier_user_id, payment_home_region
                   FROM binance_payment_accounts WHERE binance_uid = ?1",
                params![binance_uid],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_db("check Binance UID ownership"))?;
        if uid_owner
            .as_ref()
            .is_some_and(|owner| owner.0 != supplier_user_id || owner.1 != region)
        {
            return Err(AppError::Conflict(
                "this Binance UID is already bound to another payment account".into(),
            ));
        }
        let existing = conn
            .query_row(
                "SELECT id, credential_revision FROM binance_payment_accounts
                 WHERE supplier_user_id = ?1 AND payment_home_region = ?2",
                params![supplier_user_id, region],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(map_db("read existing Binance account binding"))?;
        Ok(existing
            .map(|(id, revision)| (id, revision.saturating_add(1)))
            .unwrap_or_else(|| (Uuid::new_v4().to_string(), 1)))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn binance_save_verified_account(
        &self,
        supplier_user_id: &str,
        account_id: &str,
        region: &str,
        binance_uid: &str,
        api_key: &str,
        credentials_ciphertext: &str,
        credential_nonce: &str,
        encryption_key_version: i64,
        credential_revision: i64,
        automation_mode: &str,
        verification: &VerificationResult,
    ) -> Result<BinancePaymentAccountView, AppError> {
        ensure_safe_verification(verification, true)?;
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let cooldown_until = (now_dt + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let permissions_json = serde_json::to_string(verification)
            .map_err(|_| AppError::Internal("encode Binance permissions failed".into()))?;
        let masked = mask_api_key(api_key);
        let fingerprint = format!("{:x}", Sha256::digest(api_key.as_bytes()));
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance account binding"))?;
        ensure_public_binance_method(&tx, supplier_user_id, binance_uid)?;
        let identity_conflict = tx
            .query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM binance_payment_accounts
                     WHERE id != ?1 AND (
                        binance_uid = ?2 OR
                        (credential_fingerprint != '' AND credential_fingerprint = ?3)
                     )
                 )",
                params![account_id, binance_uid, &fingerprint],
                |row| row.get::<_, i64>(0),
            )
            .map_err(map_db("check Binance payment identity uniqueness"))?;
        if identity_conflict != 0 {
            return Err(AppError::Conflict(
                "this Binance account or API key is already bound".into(),
            ));
        }
        let existing = tx
            .query_row(
                "SELECT id, credential_revision FROM binance_payment_accounts
                 WHERE supplier_user_id = ?1 AND payment_home_region = ?2",
                params![supplier_user_id, region],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(map_db("lock Binance account binding"))?;
        match existing {
            None => {
                if credential_revision != 1 {
                    return Err(AppError::Conflict(
                        "Binance account binding changed during verification; retry".into(),
                    ));
                }
                let inserted = tx
                    .execute(
                        "INSERT INTO binance_payment_accounts (
                            id, supplier_user_id, binance_uid, masked_api_key,
                            credential_fingerprint, credentials_ciphertext, credential_nonce,
                            encryption_key_version, credential_revision, status, automation_mode,
                            payment_home_region, permissions_json, permissions_verified_at,
                            uid_confirmed, uid_confirmation_source, next_poll_at, created_at,
                            updated_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 'verified', ?9,
                                   ?10, ?11, ?14, ?12, ?13, ?14, ?14, ?14)
                         ON CONFLICT(supplier_user_id, payment_home_region) DO NOTHING",
                        params![
                            account_id,
                            supplier_user_id,
                            binance_uid,
                            masked,
                            fingerprint,
                            credentials_ciphertext,
                            credential_nonce,
                            encryption_key_version,
                            automation_mode,
                            region,
                            permissions_json,
                            i64::from(verification.uid_confirmed),
                            verification.uid_confirmed.then_some("receiver_history"),
                            now,
                        ],
                    )
                    .map_err(map_db("insert Binance payment account"))?;
                if inserted != 1 {
                    return Err(AppError::Conflict(
                        "Binance account binding changed during verification; retry".into(),
                    ));
                }
            }
            Some((current_id, current_revision)) => {
                if current_id != account_id
                    || current_revision.checked_add(1) != Some(credential_revision)
                {
                    return Err(AppError::Conflict(
                        "Binance account binding changed during verification; retry".into(),
                    ));
                }
                cancel_payment_account_intents_tx(
                    &tx,
                    account_id,
                    "payment_account_rebound",
                    &now,
                    &cooldown_until,
                )?;
                let updated = tx
                    .execute(
                        "UPDATE binance_payment_accounts
                         SET binance_uid = ?5, masked_api_key = ?6,
                             credential_fingerprint = ?7, credentials_ciphertext = ?8,
                             credential_nonce = ?9, encryption_key_version = ?10,
                             credential_revision = ?11, status = 'verified',
                             automation_mode = ?12, permissions_json = ?13,
                             permissions_verified_at = ?16, uid_confirmed = ?14,
                             uid_confirmation_source = ?15, last_poll_success_at = NULL,
                             last_poll_error_code = NULL, consecutive_failures = 0,
                             poll_cursor_at = NULL, lease_owner = NULL, lease_until = NULL,
                             next_poll_at = ?16, updated_at = ?16
                         WHERE id = ?1 AND supplier_user_id = ?2
                           AND payment_home_region = ?3 AND credential_revision = ?4",
                        params![
                            account_id,
                            supplier_user_id,
                            region,
                            current_revision,
                            binance_uid,
                            masked,
                            fingerprint,
                            credentials_ciphertext,
                            credential_nonce,
                            encryption_key_version,
                            credential_revision,
                            automation_mode,
                            permissions_json,
                            i64::from(verification.uid_confirmed),
                            verification.uid_confirmed.then_some("receiver_history"),
                            now,
                        ],
                    )
                    .map_err(map_db("rotate Binance payment account"))?;
                if updated != 1 {
                    return Err(AppError::Conflict(
                        "Binance account binding changed during verification; retry".into(),
                    ));
                }
            }
        }
        tx.commit()
            .map_err(map_db("commit Binance account binding"))?;
        drop(conn);
        self.binance_payment_account_view(supplier_user_id, region)
            .await?
            .ok_or_else(|| AppError::Internal("saved Binance account is missing".into()))
    }

    pub async fn binance_mark_account_verified(
        &self,
        supplier_user_id: &str,
        region: &str,
        account_id: &str,
        credential_revision: i64,
        expected_lease_owner: Option<&str>,
        verification: &VerificationResult,
    ) -> Result<(), AppError> {
        ensure_safe_verification(verification, false)?;
        let now = Utc::now().to_rfc3339();
        let permissions_json = serde_json::to_string(verification)
            .map_err(|_| AppError::Internal("encode Binance permissions failed".into()))?;
        let conn = self.conn.lock().await;
        let updated = conn
            .execute(
                "UPDATE binance_payment_accounts
                 SET status = 'verified', permissions_json = ?6,
                     permissions_verified_at = ?8,
                     uid_confirmed = MAX(uid_confirmed, ?7),
                     uid_confirmation_source = CASE WHEN ?7 = 1
                         THEN 'receiver_history' ELSE uid_confirmation_source END,
                     last_poll_error_code = NULL, consecutive_failures = 0,
                     next_poll_at = ?8, updated_at = ?8
                 WHERE supplier_user_id = ?1 AND payment_home_region = ?2
                   AND id = ?3 AND credential_revision = ?4
                   AND status != 'disabled' AND credentials_ciphertext != ''
                   AND (?5 IS NULL OR lease_owner = ?5)",
                params![
                    supplier_user_id,
                    region,
                    account_id,
                    credential_revision,
                    expected_lease_owner,
                    permissions_json,
                    i64::from(verification.uid_confirmed),
                    now
                ],
            )
            .map_err(map_db("mark Binance account verified"))?;
        if updated == 0 {
            return Err(AppError::Conflict(
                "Binance payment account changed while credentials were verified; retry".into(),
            ));
        }
        Ok(())
    }

    pub async fn binance_mark_account_verification_failed(
        &self,
        supplier_user_id: &str,
        region: &str,
        account_id: &str,
        credential_revision: i64,
        error_code: &str,
    ) -> Result<(), AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let cooldown_until = (now_dt + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance verification failure"))?;
        let updated = tx
            .execute(
                "UPDATE binance_payment_accounts
                 SET status = 'degraded', permissions_verified_at = NULL,
                     last_poll_error_code = ?5,
                     consecutive_failures = MIN(consecutive_failures + 1, 1000000),
                     lease_owner = NULL, lease_until = NULL,
                     next_poll_at = ?6, updated_at = ?6
                 WHERE supplier_user_id = ?1 AND payment_home_region = ?2
                   AND id = ?3 AND credential_revision = ?4
                   AND status != 'disabled' AND credentials_ciphertext != ''",
                params![
                    supplier_user_id,
                    region,
                    account_id,
                    credential_revision,
                    sanitize_code(error_code),
                    now,
                ],
            )
            .map_err(map_db("record Binance credential verification failure"))?;
        if updated == 0 {
            return Err(AppError::Conflict(
                "Binance payment account changed while credentials were verified; retry".into(),
            ));
        }
        cancel_payment_account_intents_tx(
            &tx,
            account_id,
            "payment_account_degraded",
            &now,
            &cooldown_until,
        )?;
        tx.commit()
            .map_err(map_db("commit Binance verification failure"))
    }

    pub async fn binance_disable_payment_account(
        &self,
        supplier_user_id: &str,
        region: &str,
        purge_credentials: bool,
    ) -> Result<(), AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let cooldown_until = (now_dt + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance account disable"))?;
        let account_id = tx
            .query_row(
                "SELECT id FROM binance_payment_accounts
                 WHERE supplier_user_id = ?1 AND payment_home_region = ?2",
                params![supplier_user_id, region],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("find Binance account to disable"))?
            .ok_or_else(|| AppError::NotFound("Binance payment account not found".into()))?;
        cancel_payment_account_intents_tx(
            &tx,
            &account_id,
            "payment_account_disabled",
            &now,
            &cooldown_until,
        )?;
        if purge_credentials {
            tx.execute(
                "UPDATE binance_payment_accounts
                 SET status = 'disabled', credential_revision = credential_revision + 1,
                     masked_api_key = '', credential_fingerprint = '',
                     credentials_ciphertext = '', credential_nonce = '',
                     permissions_verified_at = NULL,
                     last_poll_error_code = 'CREDENTIALS_DELETED', lease_owner = NULL,
                     lease_until = NULL, next_poll_at = NULL, updated_at = ?2
                 WHERE id = ?1",
                params![account_id, now],
            )
            .map_err(map_db("purge Binance payment credentials"))?;
        } else {
            tx.execute(
                "UPDATE binance_payment_accounts
                 SET status = 'disabled', credential_revision = credential_revision + 1,
                     permissions_verified_at = NULL, lease_owner = NULL, lease_until = NULL,
                     next_poll_at = NULL, updated_at = ?2 WHERE id = ?1",
                params![account_id, now],
            )
            .map_err(map_db("disable Binance payment account"))?;
        }
        tx.commit()
            .map_err(map_db("commit Binance account disable"))
    }

    #[cfg(test)]
    pub async fn binance_create_or_refresh_intent(
        &self,
        session: &AuthSession,
        invoice_id: &str,
        region: &str,
        refresh: bool,
    ) -> Result<BinancePaymentIntentView, AppError> {
        self.binance_create_or_refresh_intent_inner(session, invoice_id, region, 1, None, refresh)
            .await
    }

    pub async fn binance_create_or_refresh_intent_for_cipher(
        &self,
        session: &AuthSession,
        invoice_id: &str,
        region: &str,
        cipher: &CredentialCipher,
        refresh: bool,
    ) -> Result<BinancePaymentIntentView, AppError> {
        self.binance_create_or_refresh_intent_inner(
            session,
            invoice_id,
            region,
            cipher.version(),
            Some(cipher),
            refresh,
        )
        .await
    }

    #[cfg(test)]
    pub async fn binance_create_or_refresh_intent_for_key_version(
        &self,
        session: &AuthSession,
        invoice_id: &str,
        region: &str,
        expected_encryption_key_version: i64,
        refresh: bool,
    ) -> Result<BinancePaymentIntentView, AppError> {
        self.binance_create_or_refresh_intent_inner(
            session,
            invoice_id,
            region,
            expected_encryption_key_version,
            None,
            refresh,
        )
        .await
    }

    async fn binance_create_or_refresh_intent_inner(
        &self,
        session: &AuthSession,
        invoice_id: &str,
        region: &str,
        expected_encryption_key_version: i64,
        cipher: Option<&CredentialCipher>,
        refresh: bool,
    ) -> Result<BinancePaymentIntentView, AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let expires_at = (now_dt + Duration::minutes(INTENT_TTL_MINUTES)).to_rfc3339();
        let late_grace_until =
            (now_dt + Duration::minutes(INTENT_TTL_MINUTES) + Duration::hours(LATE_GRACE_HOURS))
                .to_rfc3339();
        let cooldown_until = (now_dt + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance payment intent"))?;
        release_elapsed_reservations_tx(&tx, &now)?;
        expire_due_intents_tx(&tx, &now, &cooldown_until)?;
        let invoice = load_payable_invoice_tx(&tx, invoice_id, &session.user_id)?;
        let public_uid = invoice_binance_uid(&invoice.payment_methods_json)?.ok_or_else(|| {
            AppError::Conflict("this invoice does not contain a Binance UID payment method".into())
        })?;
        let payment_account = tx
            .query_row(
                "SELECT id, binance_uid, status, automation_mode,
                        encryption_key_version, permissions_verified_at,
                        uid_confirmed, credentials_ciphertext, credential_nonce,
                        credential_revision
                 FROM binance_payment_accounts
                 WHERE supplier_user_id = ?1 AND payment_home_region = ?2",
                params![invoice.supplier_user_id, region],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, i64>(6)? != 0,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, i64>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read invoice Binance account"))?
            .ok_or_else(|| {
                AppError::ServiceUnavailable(
                    "the supplier has not enabled Binance auto-settlement".into(),
                )
            })?;
        if payment_account.1 != public_uid {
            return Err(AppError::Conflict(
                "the supplier Binance account no longer matches this invoice snapshot".into(),
            ));
        }
        if payment_account.2 != "verified" || payment_account.3 != "enabled" {
            return Err(AppError::ServiceUnavailable(
                "the supplier Binance auto-settlement account is not available".into(),
            ));
        }
        if payment_account.4 != expected_encryption_key_version {
            return Err(AppError::ServiceUnavailable(
                "the supplier must rebind Binance credentials for the active encryption key".into(),
            ));
        }
        if let Some(cipher) = cipher {
            let aad = credential_aad(
                &payment_account.0,
                &invoice.supplier_user_id,
                payment_account.9,
            );
            let decryptable = cipher
                .open_json::<BinanceCredentials>(
                    &payment_account.7,
                    &payment_account.8,
                    aad.as_bytes(),
                )
                .is_ok();
            if !decryptable {
                let updated = tx
                    .execute(
                        "UPDATE binance_payment_accounts
                         SET status = 'degraded', permissions_verified_at = NULL,
                             last_poll_error_code = 'CREDENTIAL_DECRYPT_FAILED',
                             consecutive_failures = MIN(consecutive_failures + 1, 1000000),
                             lease_owner = NULL, lease_until = NULL, next_poll_at = NULL,
                             updated_at = ?3
                         WHERE id = ?1 AND credential_revision = ?2
                           AND status != 'disabled'",
                        params![payment_account.0, payment_account.9, now],
                    )
                    .map_err(map_db("degrade undecryptable Binance payment account"))?;
                if updated != 1 {
                    return Err(AppError::Conflict(
                        "the supplier Binance account changed while creating a payment intent"
                            .into(),
                    ));
                }
                cancel_payment_account_intents_tx(
                    &tx,
                    &payment_account.0,
                    "payment_account_degraded",
                    &now,
                    &cooldown_until,
                )?;
                tx.commit()
                    .map_err(map_db("commit undecryptable Binance account degradation"))?;
                return Err(AppError::ServiceUnavailable(
                    "the supplier Binance credentials cannot be decrypted; rebind is required"
                        .into(),
                ));
            }
        }
        let permissions_are_fresh =
            super::permission_verification_is_fresh(payment_account.5.as_deref(), now_dt);
        if !permissions_are_fresh {
            return Err(AppError::ServiceUnavailable(
                "the supplier must re-verify Binance read-only permissions".into(),
            ));
        }
        if !payment_account.6 {
            return Err(AppError::ServiceUnavailable(
                "the supplier Binance UID has not been confirmed by incoming payment history"
                    .into(),
            ));
        }
        if let Some(existing) = latest_pending_intent_tx(&tx, invoice_id)? {
            if !refresh {
                tx.commit()
                    .map_err(map_db("commit reused Binance intent"))?;
                return Ok(existing);
            }
            let created_at = DateTime::parse_from_rfc3339(&existing.created_at)
                .map_err(|_| {
                    AppError::Internal("stored Binance intent timestamp is invalid".into())
                })?
                .with_timezone(&Utc);
            let elapsed = now_dt
                .signed_duration_since(created_at)
                .num_seconds()
                .max(0);
            if elapsed < INTENT_REFRESH_MIN_SECONDS {
                return Err(AppError::RateLimited {
                    message: "wait before generating another Binance payment amount".into(),
                    retry_after_secs: u64::try_from(INTENT_REFRESH_MIN_SECONDS - elapsed)
                        .unwrap_or(30),
                });
            }
            cancel_intent_tx(&tx, &existing.id, "buyer_refreshed", &now, &cooldown_until)?;
        } else if let Some(review) = latest_review_intent_tx(&tx, invoice_id)? {
            tx.commit()
                .map_err(map_db("commit reviewed Binance intent"))?;
            return Ok(review);
        } else if let Some(previous) = latest_intent_tx(&tx, invoice_id)? {
            if !refresh && matches!(previous.status.as_str(), "expired" | "cancelled") {
                tx.commit()
                    .map_err(map_db("commit previous Binance intent view"))?;
                return Ok(previous);
            }
            if refresh && previous.status == "expired" {
                cancel_intent_tx(&tx, &previous.id, "buyer_refreshed", &now, &cooldown_until)?;
            }
        }
        enforce_intent_allocation_limits_tx(
            &tx,
            invoice_id,
            &payment_account.0,
            &session.user_id,
            &now_dt,
        )?;
        let base_amount_units = invoice
            .amount_minor
            .checked_mul(PAYMENT_AMOUNT_SCALE / 100)
            .ok_or_else(|| AppError::Internal("Binance payment amount overflowed".into()))?;
        let pay_amount_units =
            allocate_payment_amount_tx(&tx, &payment_account.0, base_amount_units)?;
        let intent_id = Uuid::new_v4().to_string();
        let note_code = random_note_code();
        tx.execute(
            "INSERT INTO market_payment_intents (
                id, invoice_id, payment_account_id, buyer_user_id, supplier_user_id,
                receiver_uid, asset, base_amount_units, pay_amount_units, amount_scale,
                note_code, status, expires_at, late_grace_until, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'USDT', ?7, ?8, 10000,
                       ?9, 'pending', ?10, ?11, ?12, ?12)",
            params![
                intent_id,
                invoice_id,
                payment_account.0,
                session.user_id,
                invoice.supplier_user_id,
                public_uid,
                base_amount_units,
                pay_amount_units,
                note_code,
                expires_at,
                late_grace_until,
                now,
            ],
        )
        .map_err(map_db("create Binance payment intent"))?;
        tx.execute(
            "INSERT INTO market_payment_amount_reservations (
                id, payment_account_id, asset, pay_amount_units, intent_id,
                status, reserved_at
             ) VALUES (?1, ?2, 'USDT', ?3, ?4, 'reserved', ?5)",
            params![
                Uuid::new_v4().to_string(),
                payment_account.0,
                pay_amount_units,
                intent_id,
                now,
            ],
        )
        .map_err(map_db("reserve Binance payment amount"))?;
        tx.execute(
            "UPDATE binance_payment_accounts
             SET next_poll_at = ?2, updated_at = ?2 WHERE id = ?1",
            params![payment_account.0, now],
        )
        .map_err(map_db("schedule Binance account polling"))?;
        let view = intent_view_by_id_tx(&tx, &intent_id)?;
        tx.commit()
            .map_err(map_db("commit Binance payment intent"))?;
        Ok(view)
    }

    pub async fn binance_intent_for_invoice(
        &self,
        session: &AuthSession,
        invoice_id: &str,
    ) -> Result<Option<BinancePaymentIntentView>, AppError> {
        let conn = self.conn.lock().await;
        ensure_invoice_actor(&conn, invoice_id, &session.user_id)?;
        latest_intent_tx(&conn, invoice_id)
    }

    pub async fn binance_cancel_intent(
        &self,
        session: &AuthSession,
        invoice_id: &str,
    ) -> Result<BinancePaymentIntentView, AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let cooldown_until = (now_dt + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance intent cancellation"))?;
        let invoice = load_payable_invoice_tx(&tx, invoice_id, &session.user_id)?;
        if invoice.buyer_user_id != session.user_id {
            return Err(AppError::Forbidden(
                "only the buyer can cancel a Binance payment intent".into(),
            ));
        }
        let intent = latest_pending_intent_tx(&tx, invoice_id)?.ok_or_else(|| {
            AppError::Conflict("there is no pending Binance payment intent".into())
        })?;
        cancel_intent_tx(&tx, &intent.id, "buyer_cancelled", &now, &cooldown_until)?;
        let view = intent_view_by_id_tx(&tx, &intent.id)?;
        tx.commit()
            .map_err(map_db("commit Binance intent cancellation"))?;
        Ok(view)
    }

    pub async fn binance_expire_due_intents(&self) -> Result<(), AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let cooldown_until = (now_dt + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance intent expiry"))?;
        expire_due_intents_tx(&tx, &now, &cooldown_until)?;
        release_elapsed_reservations_tx(&tx, &now)?;
        tx.commit().map_err(map_db("commit Binance intent expiry"))
    }

    pub async fn binance_cancel_live_intents_for_global_disable(&self) -> Result<(), AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let cooldown_until = (now_dt + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance global disable"))?;
        let intent_ids = tx
            .prepare(
                "SELECT id FROM market_payment_intents
                 WHERE status IN ('pending', 'expired')",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read live Binance intents for global disable"))?;
        for intent_id in intent_ids {
            cancel_intent_tx(
                &tx,
                &intent_id,
                "global_settlement_disabled",
                &now,
                &cooldown_until,
            )?;
        }
        // Persistently demote every account as well as fencing in-flight poll
        // leases. During a rolling restart this prevents an older enabled HTTP
        // process from creating another payable intent after the disabled
        // process commits the kill switch. Re-enabling is therefore always an
        // explicit, per-account credential rebind.
        tx.execute(
            "UPDATE binance_payment_accounts
             SET automation_mode = 'shadow', lease_owner = NULL, lease_until = NULL,
                 next_poll_at = NULL, updated_at = ?1
             WHERE automation_mode != 'shadow' OR lease_owner IS NOT NULL
                OR lease_until IS NOT NULL OR next_poll_at IS NOT NULL",
            params![now],
        )
        .map_err(map_db("fence Binance poll leases for global disable"))?;
        tx.commit().map_err(map_db("commit Binance global disable"))
    }

    pub async fn binance_claim_poll_account(
        &self,
        region: &str,
        lease_owner: &str,
    ) -> Result<Option<StoredPaymentAccount>, AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let lease_until = (now_dt + Duration::seconds(POLL_LEASE_SECONDS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance poll lease"))?;
        let account = tx
            .query_row(
                "SELECT account.id, account.supplier_user_id, account.binance_uid,
                        account.credentials_ciphertext, account.credential_nonce,
                        account.encryption_key_version, account.credential_revision,
                        account.automation_mode, account.permissions_verified_at,
                        account.poll_cursor_at,
                        (SELECT MIN(intent.created_at)
                           FROM market_payment_intents intent
                          WHERE intent.payment_account_id = account.id
                            AND (
                                intent.status = 'pending' OR
                                (intent.status = 'expired' AND intent.late_grace_until >= ?2) OR
                                (intent.status = 'cancelled'
                                 AND intent.late_grace_until >= ?2
                                 AND COALESCE(intent.cancellation_reason, '') NOT IN (
                                     'payment_account_rebound', 'payment_account_disabled'
                                 ))
                            ))
                 FROM binance_payment_accounts account
                 WHERE account.payment_home_region = ?1
                   AND account.status IN ('verified', 'degraded')
                   AND account.credentials_ciphertext != ''
                   AND (account.next_poll_at IS NULL OR account.next_poll_at <= ?2)
                   AND (account.lease_until IS NULL OR account.lease_until <= ?2)
                   AND EXISTS (
                       SELECT 1 FROM market_payment_intents intent
                       WHERE intent.payment_account_id = account.id
                         AND (
                             intent.status = 'pending' OR
                             (intent.status = 'expired' AND intent.late_grace_until >= ?2) OR
                             (intent.status = 'cancelled'
                              AND intent.late_grace_until >= ?2
                              AND COALESCE(intent.cancellation_reason, '') NOT IN (
                                  'payment_account_rebound', 'payment_account_disabled'
                              ))
                         )
                   )
                 ORDER BY COALESCE(account.next_poll_at, ''), account.id
                 LIMIT 1",
                params![region, now],
                |row| {
                    Ok(StoredPaymentAccount {
                        id: row.get(0)?,
                        supplier_user_id: row.get(1)?,
                        binance_uid: row.get(2)?,
                        credentials_ciphertext: row.get(3)?,
                        credential_nonce: row.get(4)?,
                        encryption_key_version: row.get(5)?,
                        credential_revision: row.get(6)?,
                        automation_mode: row.get(7)?,
                        permissions_verified_at: row.get(8)?,
                        poll_cursor_at: row.get(9)?,
                        active_intent_started_at: row.get(10)?,
                    })
                },
            )
            .optional()
            .map_err(map_db("select Binance account poll lease"))?;
        let Some(account) = account else {
            tx.commit()
                .map_err(map_db("commit empty Binance poll lease"))?;
            return Ok(None);
        };
        let claimed = tx
            .execute(
                "UPDATE binance_payment_accounts
                 SET lease_owner = ?2, lease_until = ?3, updated_at = ?4
                 WHERE id = ?1 AND (lease_until IS NULL OR lease_until <= ?4)",
                params![account.id, lease_owner, lease_until, now],
            )
            .map_err(map_db("claim Binance account poll lease"))?;
        tx.commit().map_err(map_db("commit Binance poll lease"))?;
        Ok((claimed == 1).then_some(account))
    }

    pub async fn binance_record_poll_failure(
        &self,
        account_id: &str,
        lease_owner: &str,
        error_code: &str,
        retry_after_secs: Option<u64>,
    ) -> Result<(), AppError> {
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let cooldown_until = (now + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let force_degraded = matches!(
            error_code,
            "CREDENTIAL_DECRYPT_FAILED"
                | "BINANCE_CREDENTIALS_REJECTED"
                | "READ_PERMISSION_REQUIRED"
                | "DANGEROUS_PERMISSION_ENABLED"
                | "RECEIVER_UID_MISMATCH"
        );
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance poll failure"))?;
        let failures = tx
            .query_row(
                "SELECT consecutive_failures FROM binance_payment_accounts
                 WHERE id = ?1 AND lease_owner = ?2",
                params![account_id, lease_owner],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_db("read Binance poll failure count"))?
            .unwrap_or(0)
            .saturating_add(1);
        let exponential = 4_i64.saturating_mul(1_i64 << failures.min(6));
        let default_delay = match error_code {
            "BINANCE_IP_BANNED" => IP_BAN_DEFAULT_BACKOFF_SECONDS,
            "BINANCE_RATE_LIMITED" => exponential.max(RATE_LIMIT_DEFAULT_BACKOFF_SECONDS),
            _ => exponential,
        };
        let delay = retry_after_secs
            .map(|value| i64::try_from(value).unwrap_or(MAX_POLL_BACKOFF_SECONDS))
            .unwrap_or(default_delay)
            .clamp(4, MAX_POLL_BACKOFF_SECONDS);
        let updated = tx
            .execute(
                "UPDATE binance_payment_accounts
             SET status = CASE WHEN ?7 = 1 OR ?3 >= 3 THEN 'degraded' ELSE status END,
                 permissions_verified_at = CASE WHEN ?7 = 1
                     THEN NULL ELSE permissions_verified_at END,
                 last_poll_error_code = ?4, consecutive_failures = ?3,
                 next_poll_at = ?5, lease_owner = NULL, lease_until = NULL,
                 updated_at = ?6
             WHERE id = ?1 AND lease_owner = ?2",
                params![
                    account_id,
                    lease_owner,
                    failures,
                    sanitize_code(error_code),
                    (now + Duration::seconds(delay)).to_rfc3339(),
                    now_text,
                    i64::from(force_degraded),
                ],
            )
            .map_err(map_db("record Binance poll failure"))?;
        if updated == 1 && force_degraded {
            cancel_payment_account_intents_tx(
                &tx,
                account_id,
                "payment_account_degraded",
                &now_text,
                &cooldown_until,
            )?;
        }
        tx.commit().map_err(map_db("commit Binance poll failure"))
    }

    pub async fn binance_process_poll_success(
        &self,
        account: &StoredPaymentAccount,
        lease_owner: &str,
        transactions: &[BinancePayTransaction],
        cipher: &CredentialCipher,
        globally_enabled: bool,
        poll_interval_secs: i64,
    ) -> Result<Vec<BillingAction>, AppError> {
        let prepared = prepare_transactions(account, transactions, cipher)?;
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let cooldown_until = (now_dt + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance transaction ingestion"))?;
        let lease_is_current = tx
            .query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM binance_payment_accounts
                     WHERE id = ?1 AND lease_owner = ?2
                       AND credential_revision = ?3
                       AND status IN ('verified', 'degraded')
                       AND credentials_ciphertext != ''
                 )",
                params![account.id, lease_owner, account.credential_revision],
                |row| row.get::<_, i64>(0),
            )
            .map_err(map_db("fence Binance account poll lease"))?;
        if lease_is_current == 0 {
            return Err(AppError::Conflict(
                "Binance account poll lease is no longer current".into(),
            ));
        }
        expire_due_intents_tx(&tx, &now, &cooldown_until)?;
        let mut actions = Vec::new();
        let mut max_transaction_time = account.poll_cursor_at.clone();
        for prepared in prepared {
            if prepared.transaction_time.as_str() <= now.as_str()
                && max_transaction_time
                    .as_deref()
                    .is_none_or(|current| prepared.transaction_time.as_str() > current)
            {
                max_transaction_time = Some(prepared.transaction_time.clone());
            }
            if prepared.match_status == "ignored" {
                continue;
            }
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO binance_pay_transactions (
                        id, payment_account_id, account_credential_revision,
                        account_binance_uid, encryption_key_version, transaction_id,
                        order_id, order_type, transaction_time, direction, currency,
                        amount_units, amount_scale, receiver_uid, counterparty_fingerprint,
                        raw_payload_ciphertext, raw_payload_nonce, ingestion_status,
                        match_status, match_reason, observed_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                               ?12, 10000, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
                    params![
                        prepared.id,
                        account.id,
                        account.credential_revision,
                        account.binance_uid,
                        account.encryption_key_version,
                        prepared.transaction_id,
                        prepared.order_id,
                        prepared.order_type,
                        prepared.transaction_time,
                        prepared.direction,
                        prepared.currency,
                        prepared.amount_units,
                        prepared.receiver_uid,
                        prepared.counterparty_fingerprint,
                        prepared.raw_payload_ciphertext,
                        prepared.raw_payload_nonce,
                        prepared.ingestion_status,
                        prepared.match_status,
                        prepared.match_reason,
                        now,
                    ],
                )
                .map_err(map_db("persist Binance transaction"))?;
            if inserted == 0 {
                continue;
            }
            if prepared.match_status == "review_required" {
                create_reconciliation_case_tx(
                    &tx,
                    account,
                    &prepared,
                    prepared
                        .match_reason
                        .as_deref()
                        .unwrap_or("transaction_invalid"),
                    &now,
                    None,
                    None,
                )?;
                continue;
            }
            if prepared.match_status != "unmatched" {
                continue;
            }
            match_transaction_tx(
                &tx,
                account,
                &prepared,
                globally_enabled,
                &now,
                &cooldown_until,
                &mut actions,
            )?;
        }
        let completed = tx
            .execute(
                "UPDATE binance_payment_accounts
             SET status = 'verified', last_poll_success_at = ?3,
                 last_poll_error_code = NULL, consecutive_failures = 0,
                 poll_cursor_at = COALESCE(?4, poll_cursor_at), next_poll_at = ?5,
                 lease_owner = NULL, lease_until = NULL, updated_at = ?3
             WHERE id = ?1 AND lease_owner = ?2 AND credential_revision = ?6",
                params![
                    account.id,
                    lease_owner,
                    now,
                    max_transaction_time,
                    (now_dt + Duration::seconds(poll_interval_secs.max(1))).to_rfc3339(),
                    account.credential_revision,
                ],
            )
            .map_err(map_db("complete Binance account poll"))?;
        if completed != 1 {
            return Err(AppError::Conflict(
                "Binance account poll lease changed before completion".into(),
            ));
        }
        tx.commit()
            .map_err(map_db("commit Binance transaction ingestion"))?;
        Ok(actions)
    }

    pub async fn binance_admin_reconciliation(
        &self,
    ) -> Result<BinanceSettlementAdminView, AppError> {
        let conn = self.conn.lock().await;
        let cases = conn
            .prepare(
                "SELECT reconciliation.id, reconciliation.invoice_id,
                        reconciliation.payment_intent_id,
                        reconciliation.payment_account_id,
                        reconciliation.transaction_id, reconciliation.case_kind,
                        reconciliation.status, reconciliation.detail_json,
                        account.supplier_user_id, payment.account_binance_uid,
                        payment.transaction_time, payment.currency,
                        payment.amount_units, invoice.status,
                        reconciliation.created_at, reconciliation.resolved_at
                 FROM market_payment_reconciliation_cases reconciliation
                 JOIN binance_payment_accounts account
                   ON account.id = reconciliation.payment_account_id
                 JOIN binance_pay_transactions payment
                   ON payment.payment_account_id = reconciliation.payment_account_id
                  AND payment.transaction_id = reconciliation.transaction_id
                 LEFT JOIN market_invoices invoice ON invoice.id = reconciliation.invoice_id
                 WHERE reconciliation.status = 'open'
                 ORDER BY reconciliation.created_at, reconciliation.id
                 LIMIT 200",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        let detail_json = row.get::<_, String>(7)?;
                        Ok(BinanceReconciliationCaseView {
                            id: row.get(0)?,
                            invoice_id: row.get(1)?,
                            payment_intent_id: row.get(2)?,
                            payment_account_id: row.get(3)?,
                            transaction_id: row.get(4)?,
                            case_kind: row.get(5)?,
                            status: row.get(6)?,
                            detail: serde_json::from_str(&detail_json)
                                .unwrap_or_else(|_| serde_json::json!({})),
                            supplier_user_id: row.get(8)?,
                            binance_uid: row.get(9)?,
                            transaction_time: row.get(10)?,
                            asset: row.get(11)?,
                            amount: format_amount(row.get(12)?),
                            invoice_status: row.get(13)?,
                            created_at: row.get(14)?,
                            resolved_at: row.get(15)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_db("read Binance reconciliation cases"))?;
        let (open_case_count, pending_intent_count, degraded_account_count, oldest_open_case_at) =
            conn.query_row(
                "SELECT
                    (SELECT COUNT(*) FROM market_payment_reconciliation_cases WHERE status = 'open'),
                    (SELECT COUNT(*) FROM market_payment_intents WHERE status = 'pending'),
                    (SELECT COUNT(*) FROM binance_payment_accounts WHERE status = 'degraded'),
                    (SELECT MIN(created_at) FROM market_payment_reconciliation_cases WHERE status = 'open')",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .map_err(map_db("read Binance settlement operational summary"))?;
        Ok(BinanceSettlementAdminView {
            cases,
            open_case_count,
            pending_intent_count,
            degraded_account_count,
            oldest_open_case_at,
        })
    }

    pub async fn binance_resolve_reconciliation_case(
        &self,
        session: &AuthSession,
        case_id: &str,
        resolution: &str,
        invoice_id: Option<&str>,
        note: Option<&str>,
    ) -> Result<Vec<BillingAction>, AppError> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let cooldown_until = (now_dt + Duration::hours(AMOUNT_COOLDOWN_HOURS)).to_rfc3339();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_db("begin Binance reconciliation resolution"))?;
        let case = tx
            .query_row(
                "SELECT payment_account_id, transaction_id, invoice_id,
                        payment_intent_id, status
                 FROM market_payment_reconciliation_cases WHERE id = ?1",
                params![case_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db("read Binance reconciliation case"))?
            .ok_or_else(|| AppError::NotFound("Binance reconciliation case not found".into()))?;
        if case.4 != "open" {
            return Err(AppError::Conflict(
                "Binance reconciliation case is already resolved".into(),
            ));
        }
        let resolution_detail = serde_json::json!({
            "resolution": resolution,
            "note": note,
            "invoiceId": invoice_id,
        })
        .to_string();
        if resolution == "ignore" {
            if let Some(intent_id) = case.3.as_deref() {
                let cancelled = tx
                    .execute(
                        "UPDATE market_payment_intents
                         SET status = 'cancelled', cancellation_reason = 'admin_ignored_transaction',
                             cancelled_at = ?2, updated_at = ?2
                         WHERE id = ?1 AND status = 'review_required'",
                        params![intent_id, now],
                    )
                    .map_err(map_db("cancel ignored reviewed Binance intent"))?;
                if cancelled == 1 {
                    cool_intent_amount_tx(&tx, intent_id, &cooldown_until)?;
                }
            }
            tx.execute(
                "UPDATE market_payment_reconciliation_cases
                 SET status = 'ignored', resolution = ?2, resolved_by_user_id = ?3,
                     resolved_at = ?4 WHERE id = ?1 AND status = 'open'",
                params![case_id, resolution_detail, session.user_id, now],
            )
            .map_err(map_db("ignore Binance reconciliation case"))?;
            tx.execute(
                "UPDATE binance_pay_transactions
                 SET match_status = 'ignored', match_reason = 'admin_ignored'
                 WHERE payment_account_id = ?1 AND transaction_id = ?2
                   AND match_status != 'matched'",
                params![case.0, case.1],
            )
            .map_err(map_db("ignore reconciled Binance transaction"))?;
            tx.commit()
                .map_err(map_db("commit ignored Binance reconciliation"))?;
            return Ok(Vec::new());
        }
        if resolution != "settle" {
            return Err(AppError::BadRequest(
                "resolution must be settle or ignore".into(),
            ));
        }
        let target_invoice_id = reconciliation_target_invoice(case.2.as_deref(), invoice_id)?;
        let resolution_detail = serde_json::json!({
            "resolution": resolution,
            "note": note,
            "invoiceId": target_invoice_id,
        })
        .to_string();
        let transaction = tx
            .query_row(
                "SELECT amount_units, currency, ingestion_status
                 FROM binance_pay_transactions
                 WHERE payment_account_id = ?1 AND transaction_id = ?2",
                params![case.0, case.1],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(map_db("read reconciled Binance transaction"))?;
        if transaction.0 <= 0
            || transaction.1 != PAYMENT_ASSET
            || !matches!(transaction.2.as_str(), "accepted" | "review_required")
        {
            return Err(AppError::Conflict(
                "this Binance transaction is not eligible for settlement".into(),
            ));
        }
        let intent_id = tx
            .query_row(
                "SELECT intent.id
                 FROM market_payment_intents intent
                 JOIN market_invoices invoice ON invoice.id = intent.invoice_id
                 JOIN market_credit_accounts credit ON credit.id = invoice.account_id
                 JOIN binance_payment_accounts payment
                   ON payment.id = intent.payment_account_id
                 WHERE invoice.id = ?1 AND intent.payment_account_id = ?2
                   AND credit.supplier_user_id = payment.supplier_user_id
                   AND intent.status != 'paid'
                 ORDER BY CASE WHEN intent.id = ?3 THEN 0
                               WHEN intent.pay_amount_units = ?4 THEN 1 ELSE 2 END,
                          intent.created_at DESC, intent.id DESC
                 LIMIT 1",
                params![target_invoice_id, case.0, case.3, transaction.0],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db("select Binance intent for reconciliation"))?
            .ok_or_else(|| {
                AppError::Conflict(
                    "the selected invoice has no eligible Binance payment intent".into(),
                )
            })?;
        let actions = crate::market_billing::settle_invoice_from_binance_admin_tx(
            &tx,
            &target_invoice_id,
            &intent_id,
            &case.0,
            &case.1,
            transaction.0,
            &session.user_id,
            &now,
        )?;
        let intent_updated = tx
            .execute(
                "UPDATE market_payment_intents
             SET status = 'paid', matched_transaction_id = ?2, paid_at = ?3,
                 updated_at = ?3, cancellation_reason = NULL
             WHERE id = ?1 AND status != 'paid'",
                params![intent_id, case.1, now],
            )
            .map_err(map_db("mark reconciled Binance intent paid"))?;
        if intent_updated != 1 {
            return Err(AppError::Conflict(
                "the reconciled Binance intent is no longer payable".into(),
            ));
        }
        cancel_invoice_intents_tx(&tx, &target_invoice_id, "invoice_settled_elsewhere", &now)?;
        cool_intent_amount_tx(&tx, &intent_id, &cooldown_until)?;
        tx.execute(
            "UPDATE binance_pay_transactions
             SET match_status = 'matched', match_reason = 'admin_reconciliation',
                 payment_intent_id = ?3, matched_at = ?4
             WHERE payment_account_id = ?1 AND transaction_id = ?2
               AND match_status != 'matched'",
            params![case.0, case.1, intent_id, now],
        )
        .map_err(map_db("mark reconciled Binance transaction matched"))?;
        tx.execute(
            "UPDATE market_payment_reconciliation_cases
             SET status = 'settled', invoice_id = ?2, payment_intent_id = ?3,
                 resolution = ?4, resolved_by_user_id = ?5, resolved_at = ?6
             WHERE id = ?1 AND status = 'open'",
            params![
                case_id,
                target_invoice_id,
                intent_id,
                resolution_detail,
                session.user_id,
                now
            ],
        )
        .map_err(map_db("settle Binance reconciliation case"))?;
        tx.commit()
            .map_err(map_db("commit settled Binance reconciliation"))?;
        Ok(actions)
    }
}

fn reconciliation_target_invoice(
    linked_invoice_id: Option<&str>,
    requested_invoice_id: Option<&str>,
) -> Result<String, AppError> {
    let linked = linked_invoice_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let requested = requested_invoice_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if linked.is_some() && requested.is_some() && linked != requested {
        return Err(AppError::Conflict(
            "this reconciliation case is already linked to a different invoice".into(),
        ));
    }
    linked
        .or(requested)
        .map(str::to_string)
        .ok_or_else(|| AppError::BadRequest("invoiceId is required to settle this case".into()))
}

#[derive(Debug)]
struct PayableInvoice {
    buyer_user_id: String,
    supplier_user_id: String,
    amount_minor: i64,
    currency: String,
    payment_methods_json: String,
}

#[derive(Debug)]
struct PreparedTransaction {
    id: String,
    transaction_id: String,
    order_id: Option<String>,
    order_type: Option<String>,
    note: String,
    transaction_time: String,
    direction: String,
    currency: String,
    amount_units: i64,
    receiver_uid: Option<String>,
    counterparty_fingerprint: Option<String>,
    raw_payload_ciphertext: String,
    raw_payload_nonce: String,
    ingestion_status: String,
    match_status: String,
    match_reason: Option<String>,
}

fn prepare_transactions(
    account: &StoredPaymentAccount,
    transactions: &[BinancePayTransaction],
    cipher: &CredentialCipher,
) -> Result<Vec<PreparedTransaction>, AppError> {
    transactions
        .iter()
        .filter(|transaction| !transaction.transaction_id.trim().is_empty())
        .map(|transaction| {
            let transaction_id = transaction.transaction_id.trim().to_string();
            let raw = serde_json::to_value(transaction)
                .map_err(|_| AppError::Internal("encode Binance transaction failed".into()))?;
            let aad = transaction_aad(&account.id, &transaction_id);
            let (raw_payload_ciphertext, raw_payload_nonce) =
                cipher.seal_json(&raw, aad.as_bytes())?;
            let parsed_amount = parse_decimal_units(&transaction.amount, PAYMENT_AMOUNT_SCALE);
            let amount_units = parsed_amount.unwrap_or(0);
            let direction = if amount_units > 0 {
                "incoming"
            } else if amount_units < 0 {
                "outgoing"
            } else {
                "unknown"
            };
            let receiver_uid = transaction.receiver_info.uid();
            let payer_uid = transaction.payer_info.uid();
            let counterparty =
                value_as_identifier(&transaction.counterparty_id).or_else(|| payer_uid.clone());
            let fingerprint_context = format!("binance-counterparty:{}", account.id);
            let counterparty_fingerprint = counterparty
                .map(|value| cipher.fingerprint(fingerprint_context.as_bytes(), value.as_bytes()));
            let transaction_time = Utc
                .timestamp_millis_opt(transaction.transaction_time)
                .single()
                .ok_or_else(|| AppError::Internal("Binance transaction time is invalid".into()))?;
            let timestamp_in_future = transaction_time > Utc::now() + Duration::minutes(5);
            let transaction_time = transaction_time.to_rfc3339();
            let (ingestion_status, match_status, match_reason) = if parsed_amount.is_err() {
                (
                    "review_required",
                    "review_required",
                    Some("amount_invalid".into()),
                )
            } else if amount_units <= 0 {
                ("ignored", "ignored", Some("not_incoming".into()))
            } else if transaction.currency.trim().to_ascii_uppercase() != PAYMENT_ASSET {
                ("ignored", "ignored", Some("asset_not_supported".into()))
            } else if receiver_uid
                .as_deref()
                .is_some_and(|receiver| receiver != account.binance_uid)
            {
                ("ignored", "ignored", Some("receiver_uid_mismatch".into()))
            } else if payer_uid.as_deref() == Some(account.binance_uid.as_str())
                && receiver_uid.as_deref() != Some(account.binance_uid.as_str())
            {
                ("ignored", "ignored", Some("account_is_payer".into()))
            } else if timestamp_in_future {
                (
                    "review_required",
                    "review_required",
                    Some("transaction_time_in_future".into()),
                )
            } else if !transaction.order_type.trim().eq_ignore_ascii_case("C2C") {
                (
                    "review_required",
                    "review_required",
                    Some("order_type_unknown".into()),
                )
            } else {
                ("accepted", "unmatched", None)
            };
            Ok(PreparedTransaction {
                id: Uuid::new_v4().to_string(),
                transaction_id,
                order_id: clean_optional(&transaction.order_id),
                order_type: clean_optional(&transaction.order_type)
                    .map(|value| value.to_ascii_uppercase()),
                note: transaction.note.trim().to_ascii_uppercase(),
                transaction_time,
                direction: direction.into(),
                currency: transaction.currency.trim().to_ascii_uppercase(),
                amount_units,
                receiver_uid,
                counterparty_fingerprint,
                raw_payload_ciphertext,
                raw_payload_nonce,
                ingestion_status: ingestion_status.into(),
                match_status: match_status.into(),
                match_reason,
            })
        })
        .collect()
}

fn match_transaction_tx(
    tx: &Transaction<'_>,
    account: &StoredPaymentAccount,
    transaction: &PreparedTransaction,
    globally_enabled: bool,
    now: &str,
    cooldown_until: &str,
    actions: &mut Vec<BillingAction>,
) -> Result<(), AppError> {
    let candidates = tx
        .prepare(
            "SELECT intent.id, intent.invoice_id, intent.note_code, invoice.status
             FROM market_payment_intents intent
             JOIN market_invoices invoice ON invoice.id = intent.invoice_id
             WHERE intent.payment_account_id = ?1
               AND intent.status IN ('pending', 'expired')
               AND intent.asset = ?2 AND intent.pay_amount_units = ?3
               AND datetime(intent.created_at) <= datetime(?4, '+' || ?5 || ' seconds')
               AND datetime(intent.late_grace_until) >= datetime(?4)
             ORDER BY intent.created_at, intent.id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(
                    params![
                        account.id,
                        transaction.currency,
                        transaction.amount_units,
                        transaction.transaction_time,
                        PAYMENT_CLOCK_SKEW_SECONDS,
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("find Binance transaction payment intent"))?;
    if candidates.len() != 1 {
        let (reason, related) = if candidates.len() > 1 {
            ("ambiguous_exact_amount", None)
        } else {
            let note_candidate = tx
                .query_row(
                    "SELECT id, invoice_id FROM market_payment_intents
                     WHERE payment_account_id = ?1 AND status = 'pending'
                       AND note_code != '' AND instr(upper(?2), upper(note_code)) > 0
                     LIMIT 1",
                    params![account.id, transaction.note],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(map_db("find Binance note-code candidate"))?;
            if let Some(candidate) = note_candidate {
                ("amount_mismatch", Some(candidate))
            } else {
                (
                    "no_active_exact_amount",
                    historical_amount_intent_tx(tx, account, transaction)?,
                )
            }
        };
        if reason != "no_active_exact_amount" || related.is_some() {
            create_reconciliation_case_tx(
                tx,
                account,
                transaction,
                reason,
                now,
                related.as_ref().map(|(_, invoice_id)| invoice_id.as_str()),
                related.as_ref().map(|(intent_id, _)| intent_id.as_str()),
            )?;
            tx.execute(
                "UPDATE binance_pay_transactions
                 SET match_status = 'review_required', match_reason = ?3
                 WHERE payment_account_id = ?1 AND transaction_id = ?2",
                params![account.id, transaction.transaction_id, reason],
            )
            .map_err(map_db("mark Binance transaction for review"))?;
        } else {
            tx.execute(
                "UPDATE binance_pay_transactions
                 SET match_status = 'ignored', match_reason = 'unrelated_incoming',
                     order_id = NULL, counterparty_fingerprint = NULL,
                     raw_payload_ciphertext = '', raw_payload_nonce = ''
                 WHERE payment_account_id = ?1 AND transaction_id = ?2",
                params![account.id, transaction.transaction_id],
            )
            .map_err(map_db("minimize unrelated Binance transaction"))?;
        }
        return Ok(());
    }
    let (intent_id, invoice_id, _, invoice_status) = &candidates[0];
    if !matches!(invoice_status.as_str(), "open" | "overdue") {
        create_reconciliation_case_tx(
            tx,
            account,
            transaction,
            "invoice_not_payable",
            now,
            Some(invoice_id),
            Some(intent_id),
        )?;
        tx.execute(
            "UPDATE binance_pay_transactions
             SET match_status = 'review_required', match_reason = 'invoice_not_payable',
                 payment_intent_id = ?3
             WHERE payment_account_id = ?1 AND transaction_id = ?2",
            params![account.id, transaction.transaction_id, intent_id],
        )
        .map_err(map_db("mark non-payable Binance match for review"))?;
        return Ok(());
    }
    tx.execute(
        "UPDATE binance_payment_accounts
         SET uid_confirmed = 1,
             uid_confirmation_source = COALESCE(uid_confirmation_source, 'payment_observation'),
             updated_at = ?2
         WHERE id = ?1 AND uid_confirmed = 0",
        params![account.id, now],
    )
    .map_err(map_db("confirm Binance UID from payment observation"))?;
    if !globally_enabled || account.automation_mode != "enabled" {
        create_reconciliation_case_tx(
            tx,
            account,
            transaction,
            "shadow_exact_match",
            now,
            Some(invoice_id),
            Some(intent_id),
        )?;
        tx.execute(
            "UPDATE binance_pay_transactions
             SET match_status = 'review_required', match_reason = 'shadow_exact_match',
                 payment_intent_id = ?3
             WHERE payment_account_id = ?1 AND transaction_id = ?2",
            params![account.id, transaction.transaction_id, intent_id],
        )
        .map_err(map_db("record Binance shadow match"))?;
        let intent_updated = tx
            .execute(
                "UPDATE market_payment_intents
                 SET status = 'review_required', matched_transaction_id = ?2,
                     updated_at = ?3
                 WHERE id = ?1 AND status IN ('pending', 'expired')",
                params![intent_id, transaction.transaction_id, now],
            )
            .map_err(map_db("mark Binance shadow intent for review"))?;
        if intent_updated != 1 {
            return Err(AppError::Conflict(
                "Binance payment intent changed during shadow matching".into(),
            ));
        }
        cool_intent_amount_tx(tx, intent_id, cooldown_until)?;
        return Ok(());
    }
    let mut settlement_actions = crate::market_billing::settle_invoice_from_binance_tx(
        tx,
        invoice_id,
        intent_id,
        &account.id,
        &transaction.transaction_id,
        transaction.amount_units,
        now,
    )?;
    let intent_updated = tx
        .execute(
            "UPDATE market_payment_intents
         SET status = 'paid', matched_transaction_id = ?2, paid_at = ?3, updated_at = ?3
         WHERE id = ?1 AND status IN ('pending', 'expired')",
            params![intent_id, transaction.transaction_id, now],
        )
        .map_err(map_db("mark Binance payment intent paid"))?;
    if intent_updated != 1 {
        return Err(AppError::Conflict(
            "Binance payment intent changed during settlement".into(),
        ));
    }
    cancel_invoice_intents_tx(tx, invoice_id, "invoice_settled_elsewhere", now)?;
    cool_intent_amount_tx(tx, intent_id, cooldown_until)?;
    tx.execute(
        "UPDATE binance_pay_transactions
         SET match_status = 'matched', match_reason = 'exact_amount',
             payment_intent_id = ?3, matched_at = ?4
         WHERE payment_account_id = ?1 AND transaction_id = ?2",
        params![account.id, transaction.transaction_id, intent_id, now],
    )
    .map_err(map_db("mark Binance transaction matched"))?;
    actions.append(&mut settlement_actions);
    Ok(())
}

fn create_reconciliation_case_tx(
    tx: &Transaction<'_>,
    account: &StoredPaymentAccount,
    transaction: &PreparedTransaction,
    kind: &str,
    now: &str,
    invoice_id: Option<&str>,
    intent_id: Option<&str>,
) -> Result<(), AppError> {
    tx.execute(
        "INSERT OR IGNORE INTO market_payment_reconciliation_cases (
            id, invoice_id, payment_intent_id, payment_account_id, transaction_id,
            case_kind, status, detail_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8)",
        params![
            Uuid::new_v4().to_string(),
            invoice_id,
            intent_id,
            account.id,
            transaction.transaction_id,
            kind,
            serde_json::json!({
                "asset": transaction.currency,
                "amount": format_amount(transaction.amount_units),
                "reason": kind,
                "accountBinanceUid": account.binance_uid,
                "credentialRevision": account.credential_revision,
            })
            .to_string(),
            now,
        ],
    )
    .map_err(map_db("create Binance payment reconciliation case"))?;
    Ok(())
}

fn historical_amount_intent_tx(
    tx: &Transaction<'_>,
    account: &StoredPaymentAccount,
    transaction: &PreparedTransaction,
) -> Result<Option<(String, String)>, AppError> {
    tx.query_row(
        "SELECT id, invoice_id FROM market_payment_intents
         WHERE payment_account_id = ?1 AND asset = ?2 AND pay_amount_units = ?3
           AND NOT (
               status = 'cancelled' AND COALESCE(cancellation_reason, '') IN (
                   'payment_account_rebound', 'payment_account_disabled'
               )
           )
         ORDER BY created_at DESC, id DESC LIMIT 1",
        params![account.id, transaction.currency, transaction.amount_units],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )
    .optional()
    .map_err(map_db("inspect historical Binance payment amount"))
}

fn load_payable_invoice_tx(
    tx: &Transaction<'_>,
    invoice_id: &str,
    actor_user_id: &str,
) -> Result<PayableInvoice, AppError> {
    let invoice = tx
        .query_row(
            "SELECT account.buyer_user_id, account.supplier_user_id,
                    invoice.amount_minor, invoice.currency,
                    invoice.payment_methods_json, invoice.status
             FROM market_invoices invoice
             JOIN market_credit_accounts account ON account.id = invoice.account_id
             WHERE invoice.id = ?1",
            params![invoice_id],
            |row| {
                Ok((
                    PayableInvoice {
                        buyer_user_id: row.get(0)?,
                        supplier_user_id: row.get(1)?,
                        amount_minor: row.get(2)?,
                        currency: row.get(3)?,
                        payment_methods_json: row.get(4)?,
                    },
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read Binance payable invoice"))?
        .ok_or_else(|| AppError::NotFound("market invoice not found".into()))?;
    if invoice.0.buyer_user_id != actor_user_id {
        return Err(AppError::Forbidden(
            "only the buyer can create a Binance payment intent".into(),
        ));
    }
    if !matches!(invoice.1.as_str(), "open" | "overdue") {
        return Err(AppError::Conflict(
            "this invoice cannot accept a Binance payment".into(),
        ));
    }
    if invoice.0.currency != "USD" {
        return Err(AppError::Conflict(
            "Binance auto-settlement currently supports USD invoices only".into(),
        ));
    }
    if invoice.0.amount_minor <= 0 {
        return Err(AppError::Conflict(
            "this invoice does not have a positive payable amount".into(),
        ));
    }
    Ok(invoice.0)
}

fn ensure_invoice_actor(
    conn: &Connection,
    invoice_id: &str,
    actor_user_id: &str,
) -> Result<(), AppError> {
    let authorized = conn
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM market_invoices invoice
                JOIN market_credit_accounts account ON account.id = invoice.account_id
                WHERE invoice.id = ?1
                  AND (account.buyer_user_id = ?2 OR account.supplier_user_id = ?2)
             )",
            params![invoice_id, actor_user_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("authorize Binance payment intent view"))?;
    if authorized == 0 {
        return Err(AppError::NotFound("market invoice not found".into()));
    }
    Ok(())
}

fn ensure_public_binance_method(
    conn: &Connection,
    supplier_user_id: &str,
    expected_uid: &str,
) -> Result<(), AppError> {
    let methods_json = conn
        .query_row(
            "SELECT methods_json FROM account_payment_profiles WHERE user_id = ?1",
            params![supplier_user_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read public Binance payment method"))?
        .ok_or_else(|| {
            AppError::Conflict("save a public Binance UID before binding API credentials".into())
        })?;
    let methods: Vec<PaymentMethod> = serde_json::from_str(&methods_json)
        .map_err(|_| AppError::Internal("stored payment methods are invalid".into()))?;
    let valid = methods.iter().any(|method| {
        method.kind == "binance"
            && method.account.as_deref() == Some(expected_uid)
            && method.settlement_asset.as_deref().unwrap_or(PAYMENT_ASSET) == PAYMENT_ASSET
    });
    if !valid {
        return Err(AppError::Conflict(
            "the bound Binance UID must match the public Binance payment method".into(),
        ));
    }
    Ok(())
}

fn invoice_binance_uid(methods_json: &str) -> Result<Option<String>, AppError> {
    let methods: Vec<PaymentMethod> = serde_json::from_str(methods_json)
        .map_err(|_| AppError::Internal("stored invoice payment methods are invalid".into()))?;
    Ok(methods
        .into_iter()
        .find(|method| {
            method.kind == "binance"
                && method.account.is_some()
                && method.settlement_asset.as_deref().unwrap_or(PAYMENT_ASSET) == PAYMENT_ASSET
        })
        .and_then(|method| method.account))
}

fn latest_pending_intent_tx(
    conn: &Connection,
    invoice_id: &str,
) -> Result<Option<BinancePaymentIntentView>, AppError> {
    intent_view_query(conn, invoice_id, Some("pending"))
}

fn latest_review_intent_tx(
    conn: &Connection,
    invoice_id: &str,
) -> Result<Option<BinancePaymentIntentView>, AppError> {
    intent_view_query(conn, invoice_id, Some("review_required"))
}

fn latest_intent_tx(
    conn: &Connection,
    invoice_id: &str,
) -> Result<Option<BinancePaymentIntentView>, AppError> {
    intent_view_query(conn, invoice_id, None)
}

fn intent_view_query(
    conn: &Connection,
    invoice_id: &str,
    required_status: Option<&str>,
) -> Result<Option<BinancePaymentIntentView>, AppError> {
    conn.query_row(
        "SELECT intent.id, intent.invoice_id, intent.status, intent.asset,
                intent.base_amount_units, intent.pay_amount_units, intent.receiver_uid,
                intent.note_code, intent.expires_at, intent.created_at, intent.paid_at,
                intent.cancellation_reason, account.status, account.last_poll_success_at
         FROM market_payment_intents intent
         JOIN binance_payment_accounts account ON account.id = intent.payment_account_id
         WHERE intent.invoice_id = ?1 AND (?2 IS NULL OR intent.status = ?2)
         ORDER BY intent.created_at DESC, intent.id DESC LIMIT 1",
        params![invoice_id, required_status],
        intent_view_from_row,
    )
    .optional()
    .map_err(map_db("read Binance payment intent"))
}

fn intent_view_by_id_tx(
    conn: &Connection,
    intent_id: &str,
) -> Result<BinancePaymentIntentView, AppError> {
    conn.query_row(
        "SELECT intent.id, intent.invoice_id, intent.status, intent.asset,
                intent.base_amount_units, intent.pay_amount_units, intent.receiver_uid,
                intent.note_code, intent.expires_at, intent.created_at, intent.paid_at,
                intent.cancellation_reason, account.status, account.last_poll_success_at
         FROM market_payment_intents intent
         JOIN binance_payment_accounts account ON account.id = intent.payment_account_id
         WHERE intent.id = ?1",
        params![intent_id],
        intent_view_from_row,
    )
    .map_err(map_db("read created Binance payment intent"))
}

fn intent_view_from_row(row: &crate::db::Row<'_>) -> crate::db::Result<BinancePaymentIntentView> {
    let base_amount_units = row.get::<_, i64>(4)?;
    let pay_amount_units = row.get::<_, i64>(5)?;
    Ok(BinancePaymentIntentView {
        id: row.get(0)?,
        invoice_id: row.get(1)?,
        status: row.get(2)?,
        asset: row.get(3)?,
        base_amount: format_amount(base_amount_units),
        pay_amount: format_amount(pay_amount_units),
        receiver_uid: row.get(6)?,
        note_code: row.get(7)?,
        expires_at: row.get(8)?,
        created_at: row.get(9)?,
        paid_at: row.get(10)?,
        cancellation_reason: row.get(11)?,
        account_status: row.get(12)?,
        last_checked_at: row.get(13)?,
    })
}

fn allocate_payment_amount_tx(
    tx: &Transaction<'_>,
    payment_account_id: &str,
    base_amount_units: i64,
) -> Result<i64, AppError> {
    let span = MAX_SUFFIX - MIN_SUFFIX + 1;
    let start = i64::from(rand::random::<u8>()) % span;
    for offset in 0..span {
        let suffix = MIN_SUFFIX + (start + offset) % span;
        let candidate = base_amount_units
            .checked_add(suffix)
            .ok_or_else(|| AppError::Internal("Binance payment amount overflowed".into()))?;
        let used = tx
            .query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM market_payment_amount_reservations
                    WHERE payment_account_id = ?1 AND asset = 'USDT'
                      AND pay_amount_units = ?2 AND status IN ('reserved', 'cooldown')
                 )",
                params![payment_account_id, candidate],
                |row| row.get::<_, i64>(0),
            )
            .map_err(map_db("allocate Binance payment amount"))?;
        if used == 0 {
            return Ok(candidate);
        }
    }
    Err(AppError::ServiceUnavailable(
        "no safe Binance payment amount is currently available; use manual payment".into(),
    ))
}

fn enforce_intent_allocation_limits_tx(
    tx: &Transaction<'_>,
    invoice_id: &str,
    payment_account_id: &str,
    buyer_user_id: &str,
    now: &DateTime<Utc>,
) -> Result<(), AppError> {
    let cutoff = (*now - Duration::hours(24)).to_rfc3339();
    let invoice_count = tx
        .query_row(
            "SELECT COUNT(*) FROM market_payment_intents
             WHERE invoice_id = ?1 AND created_at >= ?2",
            params![invoice_id, cutoff],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("limit Binance intent allocations per invoice"))?;
    let buyer_account_count = tx
        .query_row(
            "SELECT COUNT(*) FROM market_payment_intents
             WHERE payment_account_id = ?1 AND buyer_user_id = ?2
               AND created_at >= ?3",
            params![payment_account_id, buyer_user_id, cutoff],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_db("limit Binance intent allocations per buyer"))?;
    if invoice_count >= MAX_INTENTS_PER_INVOICE_24H
        || buyer_account_count >= MAX_INTENTS_PER_BUYER_ACCOUNT_24H
    {
        return Err(AppError::ServiceUnavailable(
            "automatic Binance payment amount limit reached; use manual payment".into(),
        ));
    }
    Ok(())
}

fn cancel_intent_tx(
    tx: &Transaction<'_>,
    intent_id: &str,
    reason: &str,
    now: &str,
    cooldown_until: &str,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE market_payment_intents
         SET status = 'cancelled', cancellation_reason = ?2,
             cancelled_at = ?3, updated_at = ?3
         WHERE id = ?1 AND status IN ('pending', 'expired')",
        params![intent_id, reason, now],
    )
    .map_err(map_db("cancel Binance payment intent"))?;
    cool_intent_amount_tx(tx, intent_id, cooldown_until)?;
    Ok(())
}

fn cool_intent_amount_tx(
    tx: &Transaction<'_>,
    intent_id: &str,
    minimum_cooldown_until: &str,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE market_payment_amount_reservations
         SET status = 'cooldown',
             cooldown_until = CASE
                 WHEN COALESCE((SELECT late_grace_until
                                  FROM market_payment_intents
                                 WHERE id = ?1), '') > ?2
                 THEN (SELECT late_grace_until
                         FROM market_payment_intents
                        WHERE id = ?1)
                 ELSE ?2
             END
         WHERE intent_id = ?1 AND status = 'reserved'",
        params![intent_id, minimum_cooldown_until],
    )
    .map_err(map_db("cool Binance payment amount"))?;
    Ok(())
}

fn cancel_payment_account_intents_tx(
    tx: &Transaction<'_>,
    payment_account_id: &str,
    reason: &str,
    now: &str,
    cooldown_until: &str,
) -> Result<(), AppError> {
    let intent_ids = tx
        .prepare(
            "SELECT id FROM market_payment_intents
             WHERE payment_account_id = ?1 AND status IN ('pending', 'expired')",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![payment_account_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read payment-account Binance intents to cancel"))?;
    for intent_id in intent_ids {
        cancel_intent_tx(tx, &intent_id, reason, now, cooldown_until)?;
    }
    // Buyer-cancelled/refreshed intents normally remain visible to the poller
    // until their late-payment window closes. A credential rebind or account
    // disable is a stronger identity boundary: relabel every still-live
    // cancelled intent so it cannot resume polling under a future credential
    // revision (which may belong to a different Binance UID).
    tx.execute(
        "UPDATE market_payment_intents
         SET cancellation_reason = ?2, updated_at = ?3
         WHERE payment_account_id = ?1 AND status = 'cancelled'
           AND late_grace_until >= ?3",
        params![payment_account_id, reason, now],
    )
    .map_err(map_db(
        "fence cancelled Binance intents at account boundary",
    ))?;
    Ok(())
}

pub(crate) fn cancel_invoice_intents_tx(
    tx: &Transaction<'_>,
    invoice_id: &str,
    reason: &str,
    now: &str,
) -> Result<(), AppError> {
    let cooldown_until = (DateTime::parse_from_rfc3339(now)
        .map_err(|_| AppError::Internal("billing timestamp is invalid".into()))?
        .with_timezone(&Utc)
        + Duration::hours(AMOUNT_COOLDOWN_HOURS))
    .to_rfc3339();
    let intent_ids = tx
        .prepare(
            "SELECT id FROM market_payment_intents
             WHERE invoice_id = ?1 AND status IN ('pending', 'expired')",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![invoice_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read invoice Binance intents to cancel"))?;
    for intent_id in intent_ids {
        cancel_intent_tx(tx, &intent_id, reason, now, &cooldown_until)?;
    }
    Ok(())
}

fn expire_due_intents_tx(
    tx: &Transaction<'_>,
    now: &str,
    cooldown_until: &str,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE market_payment_intents
         SET status = 'expired', updated_at = ?1
         WHERE status = 'pending' AND expires_at < ?1",
        params![now],
    )
    .map_err(map_db("expire Binance payment intents"))?;
    tx.execute(
        "UPDATE market_payment_amount_reservations
         SET status = 'cooldown',
             cooldown_until = CASE
                 WHEN COALESCE((SELECT late_grace_until
                                  FROM market_payment_intents
                                 WHERE id = market_payment_amount_reservations.intent_id), '') > ?2
                 THEN (SELECT late_grace_until
                         FROM market_payment_intents
                        WHERE id = market_payment_amount_reservations.intent_id)
                 ELSE ?2
             END
         WHERE status = 'reserved' AND intent_id IN (
            SELECT id FROM market_payment_intents
            WHERE status = 'expired' AND updated_at = ?1
         )",
        params![now, cooldown_until],
    )
    .map_err(map_db("cool expired Binance payment amounts"))?;
    Ok(())
}

fn release_elapsed_reservations_tx(tx: &Transaction<'_>, now: &str) -> Result<(), AppError> {
    tx.execute(
        "UPDATE market_payment_amount_reservations
         SET status = 'released', released_at = ?1
         WHERE status = 'cooldown' AND cooldown_until <= ?1",
        params![now],
    )
    .map_err(map_db("release elapsed Binance payment amounts"))?;
    Ok(())
}

fn random_note_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0_u8; 6];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let code = bytes
        .iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect::<String>();
    format!("CC-{code}")
}

pub fn format_amount(units: i64) -> String {
    let negative = units < 0;
    let absolute = units.unsigned_abs();
    let scale = PAYMENT_AMOUNT_SCALE as u64;
    let whole = absolute / scale;
    let fraction = absolute % scale;
    let mut value = if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction:04}")
            .trim_end_matches('0')
            .to_string()
    };
    if negative {
        value.insert(0, '-');
    }
    value
}

pub fn validate_uid(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if !(6..=20).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AppError::BadRequest(
            "Binance UID must contain 6 to 20 digits".into(),
        ));
    }
    Ok(value.to_string())
}

pub fn validate_automation_mode(value: &str) -> Result<&'static str, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "enabled" => Ok("enabled"),
        "shadow" => Ok("shadow"),
        _ => Err(AppError::BadRequest(
            "automationMode must be shadow or enabled".into(),
        )),
    }
}

pub fn validate_credentials(api_key: &str, api_secret: &str) -> Result<(), AppError> {
    let api_key = api_key.trim();
    let api_secret = api_secret.trim();
    if !(16..=256).contains(&api_key.len())
        || !(16..=256).contains(&api_secret.len())
        || !api_key.bytes().all(|byte| byte.is_ascii_graphic())
        || !api_secret.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(AppError::BadRequest(
            "Binance API credentials are invalid".into(),
        ));
    }
    Ok(())
}

fn ensure_safe_verification(
    verification: &VerificationResult,
    require_uid_confirmation: bool,
) -> Result<(), AppError> {
    if !verification.reading_enabled
        || !verification.dangerous_permissions_disabled
        || (require_uid_confirmation && !verification.uid_confirmed)
    {
        return Err(AppError::Internal(
            "unsafe Binance verification result reached credential storage".into(),
        ));
    }
    Ok(())
}

pub fn decode_account_credentials(
    account: StoredPaymentAccount,
    cipher: &CredentialCipher,
) -> Result<AccountCredentialEnvelope, AppError> {
    if account.encryption_key_version != cipher.version() {
        return Err(AppError::ServiceUnavailable(
            "Binance credential key version is not available".into(),
        ));
    }
    let aad = credential_aad(
        &account.id,
        &account.supplier_user_id,
        account.credential_revision,
    );
    let credentials = cipher.open_json::<BinanceCredentials>(
        &account.credentials_ciphertext,
        &account.credential_nonce,
        aad.as_bytes(),
    )?;
    Ok(AccountCredentialEnvelope {
        account,
        credentials,
    })
}

fn mask_api_key(value: &str) -> String {
    let value = value.trim();
    if value.chars().count() <= 8 {
        return "••••••••".into();
    }
    let prefix = value.chars().take(4).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}••••••••{suffix}")
}

fn clean_optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.chars().take(200).collect())
}

fn sanitize_code(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '_')
        .take(80)
        .collect()
}

fn map_db(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SettlementFixture {
        buyer: AuthSession,
        supplier: AuthSession,
        invoice_id: String,
        payment_account_id: String,
        cipher: CredentialCipher,
        credentials: BinanceCredentials,
    }

    fn session(user_id: &str) -> AuthSession {
        let now = Utc::now();
        AuthSession {
            session_id: format!("session-{user_id}"),
            user_id: user_id.into(),
            email: format!("{user_id}@example.com"),
            auth_source_kind: "auth_device".into(),
            auth_source_id: format!("browser-{user_id}"),
            access_token_hash: format!("access-{user_id}"),
            refresh_token_hash: format!("refresh-{user_id}"),
            access_expires_at: now + Duration::hours(1),
            refresh_expires_at: now + Duration::days(30),
            created_at: now,
            last_used_at: now,
        }
    }

    fn verified_permissions() -> VerificationResult {
        VerificationResult {
            reading_enabled: true,
            dangerous_permissions_disabled: true,
            uid_confirmed: true,
        }
    }

    async fn settlement_fixture(store: &AppStore, label: &str) -> SettlementFixture {
        let buyer = session(&format!("buyer-{label}"));
        let supplier = session(&format!("supplier-{label}"));
        let account_id = format!("credit-{label}");
        let invoice_id = format!("invoice-{label}");
        let uid = "123456789";
        let now = Utc::now().to_rfc3339();
        let methods = serde_json::to_string(&[PaymentMethod {
            kind: "binance".into(),
            account: Some(uid.into()),
            qr_image_url: None,
            asset_url: None,
            token: None,
            chain: None,
            address: None,
            instructions: None,
            settlement_asset: Some(PAYMENT_ASSET.into()),
        }])
        .expect("serialize payment methods");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO account_payment_profiles (
                    user_id, owner_email, methods_json, contacts_json, updated_at
                 ) VALUES (?1, ?2, ?3, '[]', ?4)",
                params![supplier.user_id, supplier.email, methods, now],
            )
            .expect("insert payment profile");
            conn.execute(
                "INSERT INTO market_credit_accounts (
                    id, buyer_user_id, buyer_email, supplier_user_id, supplier_email,
                    currency, status, balance_units, open_invoice_id, credit_kind,
                    credit_limit_minor, credit_source, credit_revision, created_at,
                    updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'USD', 'settlement_due', 1000,
                           ?6, 'limited', 10000, 'counterparty', 1, ?7, ?7)",
                params![
                    account_id,
                    buyer.user_id,
                    buyer.email,
                    supplier.user_id,
                    supplier.email,
                    invoice_id,
                    now
                ],
            )
            .expect("insert credit account");
            conn.execute(
                "INSERT INTO market_invoices (
                    id, account_id, sequence, amount_minor, amount_cny_minor,
                    usd_cny_rate_micros, amount_units, currency, payment_methods_json,
                    payment_contacts_json, payment_profile_updated_at, status,
                    due_at, deadline_at, opened_at
                 ) VALUES (?1, ?2, 1, 1000, 7000, 7000000, 1000, 'USD', ?3,
                           '[]', ?4, 'open', ?4, ?4, ?4)",
                params![invoice_id, account_id, methods, now],
            )
            .expect("insert open invoice");
        }
        let cipher = CredentialCipher::new([3; 32], 1);
        let credentials = BinanceCredentials {
            api_key: format!("api-key-{label}-0123456789abcdef"),
            api_secret: format!("api-secret-{label}-fedcba9876543210"),
        };
        let (payment_account_id, revision) = store
            .binance_prepare_account_binding(&supplier.user_id, "test", uid)
            .await
            .expect("prepare account binding");
        let aad = credential_aad(&payment_account_id, &supplier.user_id, revision);
        let (ciphertext, nonce) = cipher
            .seal_json(&credentials, aad.as_bytes())
            .expect("encrypt credentials");
        store
            .binance_save_verified_account(
                &supplier.user_id,
                &payment_account_id,
                "test",
                uid,
                &credentials.api_key,
                &ciphertext,
                &nonce,
                cipher.version(),
                revision,
                "enabled",
                &verified_permissions(),
            )
            .await
            .expect("save verified account");
        SettlementFixture {
            buyer,
            supplier,
            invoice_id,
            payment_account_id,
            cipher,
            credentials,
        }
    }

    fn transaction(
        transaction_id: &str,
        amount: &str,
        currency: &str,
        order_type: &str,
        receiver_uid: Option<&str>,
    ) -> BinancePayTransaction {
        BinancePayTransaction {
            order_id: format!("order-{transaction_id}"),
            note: String::new(),
            order_type: order_type.into(),
            transaction_id: transaction_id.into(),
            transaction_time: Utc::now().timestamp_millis(),
            amount: amount.into(),
            currency: currency.into(),
            counterparty_id: serde_json::json!(987654321_i64),
            payer_info: super::super::client::PartyInfo::default(),
            receiver_info: super::super::client::PartyInfo {
                name: String::new(),
                binance_id: receiver_uid
                    .map(|value| serde_json::json!(value))
                    .unwrap_or(serde_json::Value::Null),
            },
        }
    }

    async fn claim_account(store: &AppStore) -> StoredPaymentAccount {
        store
            .binance_claim_poll_account("test", "test-worker")
            .await
            .expect("claim poll account")
            .expect("poll account available")
    }

    #[test]
    fn formats_payment_amounts_canonically() {
        assert_eq!(format_amount(100_037), "10.0037");
        assert_eq!(format_amount(100_000), "10");
        assert_eq!(format_amount(-12_300), "-1.23");
    }

    #[test]
    fn linked_reconciliation_invoice_cannot_be_redirected() {
        assert_eq!(
            reconciliation_target_invoice(Some("invoice-a"), None).unwrap(),
            "invoice-a"
        );
        assert_eq!(
            reconciliation_target_invoice(None, Some(" invoice-b ")).unwrap(),
            "invoice-b"
        );
        assert!(matches!(
            reconciliation_target_invoice(Some("invoice-a"), Some("invoice-b")),
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            reconciliation_target_invoice(None, None),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn uid_and_credential_validation_is_fail_closed() {
        assert_eq!(validate_uid("123456789").unwrap(), "123456789");
        assert!(validate_uid("user@example.com").is_err());
        assert!(validate_credentials("short", "also-short").is_err());
        assert!(validate_credentials("0123456789abcdef", "fedcba9876543210").is_ok());
        assert!(validate_credentials("0123456789abcde界", "fedcba9876543210").is_err());
        assert!(validate_credentials("0123456789abc def", "fedcba9876543210").is_err());
    }

    #[tokio::test]
    async fn encrypted_binding_never_stores_plaintext_credentials() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "encrypted").await;
        let conn = store.conn.lock().await;
        let (masked, ciphertext, nonce): (String, String, String) = conn
            .query_row(
                "SELECT masked_api_key, credentials_ciphertext, credential_nonce
                 FROM binance_payment_accounts WHERE id = ?1",
                params![fixture.payment_account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read protected credentials");
        assert!(!masked.contains(&fixture.credentials.api_key));
        assert!(!ciphertext.contains(&fixture.credentials.api_key));
        assert!(!ciphertext.contains(&fixture.credentials.api_secret));
        assert!(!nonce.is_empty());
        drop(conn);
        let stored = store
            .binance_load_payment_account(&fixture.supplier.user_id, "test")
            .await
            .expect("load payment account")
            .expect("payment account");
        let decoded = decode_account_credentials(stored, &fixture.cipher)
            .expect("decrypt stored credentials");
        assert_eq!(decoded.credentials.api_key, fixture.credentials.api_key);
        assert_eq!(
            decoded.credentials.api_secret,
            fixture.credentials.api_secret
        );
    }

    #[tokio::test]
    async fn payment_identity_cannot_be_shared_across_suppliers() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "identity-owner").await;
        let other = session("supplier-identity-other");
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "INSERT INTO account_payment_profiles (
                    user_id, owner_email, methods_json, contacts_json, updated_at
                 ) SELECT ?1, ?2, methods_json, '[]', ?3
                     FROM account_payment_profiles WHERE user_id = ?4",
                params![other.user_id, other.email, now, fixture.supplier.user_id],
            )
            .expect("copy Binance payment profile");
        }
        assert!(matches!(
            store
                .binance_prepare_account_binding(&other.user_id, "test", "123456789")
                .await,
            Err(AppError::Conflict(_))
        ));

        let other_uid = "987654321";
        let methods = serde_json::to_string(&[PaymentMethod {
            kind: "binance".into(),
            account: Some(other_uid.into()),
            qr_image_url: None,
            asset_url: None,
            token: None,
            chain: None,
            address: None,
            instructions: None,
            settlement_asset: Some(PAYMENT_ASSET.into()),
        }])
        .expect("serialize second payment profile");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE account_payment_profiles SET methods_json = ?2 WHERE user_id = ?1",
                params![other.user_id, methods],
            )
            .expect("replace second payment profile");
        }
        let (account_id, revision) = store
            .binance_prepare_account_binding(&other.user_id, "test", other_uid)
            .await
            .expect("prepare second account binding");
        let aad = credential_aad(&account_id, &other.user_id, revision);
        let (ciphertext, nonce) = fixture
            .cipher
            .seal_json(&fixture.credentials, aad.as_bytes())
            .expect("encrypt duplicate API key");
        assert!(matches!(
            store
                .binance_save_verified_account(
                    &other.user_id,
                    &account_id,
                    "test",
                    other_uid,
                    &fixture.credentials.api_key,
                    &ciphertext,
                    &nonce,
                    fixture.cipher.version(),
                    revision,
                    "enabled",
                    &verified_permissions(),
                )
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn credential_rotation_is_cas_fenced_and_cancels_live_intents() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "credential-cas").await;
        let previously_cancelled_intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent to cancel before rotation");
        store
            .binance_cancel_intent(&fixture.buyer, &fixture.invoice_id)
            .await
            .expect("put an old amount into late-payment monitoring");
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", true)
            .await
            .expect("create intent before rotation");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_payment_intents SET status = 'expired' WHERE id = ?1",
                params![intent.id],
            )
            .expect("put intent into the late-payment window");
            conn.execute(
                "UPDATE binance_payment_accounts SET poll_cursor_at = ?2 WHERE id = ?1",
                params![fixture.payment_account_id, Utc::now().to_rfc3339()],
            )
            .expect("seed old credential poll cursor");
        }
        let (first_id, first_revision) = store
            .binance_prepare_account_binding(&fixture.supplier.user_id, "test", "123456789")
            .await
            .expect("prepare first rotation");
        let (stale_id, stale_revision) = store
            .binance_prepare_account_binding(&fixture.supplier.user_id, "test", "123456789")
            .await
            .expect("prepare competing rotation");
        assert_eq!(
            (first_id.as_str(), first_revision),
            (stale_id.as_str(), stale_revision)
        );
        let first_credentials = BinanceCredentials {
            api_key: "rotation-first-api-key-0123456789".into(),
            api_secret: "rotation-first-secret-0123456789".into(),
        };
        let stale_credentials = BinanceCredentials {
            api_key: "rotation-stale-api-key-0123456789".into(),
            api_secret: "rotation-stale-secret-0123456789".into(),
        };
        let first_aad = credential_aad(&first_id, &fixture.supplier.user_id, first_revision);
        let stale_aad = credential_aad(&stale_id, &fixture.supplier.user_id, stale_revision);
        let (first_ciphertext, first_nonce) = fixture
            .cipher
            .seal_json(&first_credentials, first_aad.as_bytes())
            .expect("encrypt first rotation");
        let (stale_ciphertext, stale_nonce) = fixture
            .cipher
            .seal_json(&stale_credentials, stale_aad.as_bytes())
            .expect("encrypt stale rotation");
        store
            .binance_save_verified_account(
                &fixture.supplier.user_id,
                &first_id,
                "test",
                "123456789",
                &first_credentials.api_key,
                &first_ciphertext,
                &first_nonce,
                fixture.cipher.version(),
                first_revision,
                "enabled",
                &verified_permissions(),
            )
            .await
            .expect("commit first credential rotation");
        assert!(matches!(
            store
                .binance_save_verified_account(
                    &fixture.supplier.user_id,
                    &stale_id,
                    "test",
                    "123456789",
                    &stale_credentials.api_key,
                    &stale_ciphertext,
                    &stale_nonce,
                    fixture.cipher.version(),
                    stale_revision,
                    "enabled",
                    &verified_permissions(),
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let stored = store
            .binance_load_payment_account(&fixture.supplier.user_id, "test")
            .await
            .expect("load rotated account")
            .expect("rotated account exists");
        assert_eq!(stored.id, fixture.payment_account_id);
        assert_eq!(stored.credential_revision, first_revision);
        assert!(stored.poll_cursor_at.is_none());
        let decoded =
            decode_account_credentials(stored, &fixture.cipher).expect("decrypt winning rotation");
        assert_eq!(decoded.credentials.api_key, first_credentials.api_key);
        let conn = store.conn.lock().await;
        let (status, reason, cooldown_until, late_grace_until): (String, String, String, String) =
            conn.query_row(
                "SELECT intent.status, intent.cancellation_reason,
                        reservation.cooldown_until, intent.late_grace_until
                   FROM market_payment_intents intent
                   JOIN market_payment_amount_reservations reservation
                     ON reservation.intent_id = intent.id
                  WHERE intent.id = ?1",
                params![intent.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read intent cancelled by rotation");
        assert_eq!(status, "cancelled");
        assert_eq!(reason, "payment_account_rebound");
        assert!(cooldown_until >= late_grace_until);
        let old_cancellation_reason: String = conn
            .query_row(
                "SELECT cancellation_reason FROM market_payment_intents WHERE id = ?1",
                params![previously_cancelled_intent.id],
                |row| row.get(0),
            )
            .expect("read pre-cancelled intent fenced by rotation");
        assert_eq!(old_cancellation_reason, "payment_account_rebound");
        drop(conn);
        assert!(
            store
                .binance_claim_poll_account("test", "post-rotation-worker")
                .await
                .expect("check polling after credential rotation")
                .is_none()
        );
    }

    #[tokio::test]
    async fn global_disable_atomically_cancels_intents_and_fences_inflight_polling() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "global-disable").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent before global disable");
        let account = claim_account(&store).await;

        store
            .binance_cancel_live_intents_for_global_disable()
            .await
            .expect("apply global settlement kill switch");
        assert!(matches!(
            store
                .binance_process_poll_success(
                    &account,
                    "test-worker",
                    &[transaction(
                        "tx-after-global-disable",
                        &intent.pay_amount,
                        "USDT",
                        "C2C",
                        None,
                    )],
                    &fixture.cipher,
                    true,
                    4,
                )
                .await,
            Err(AppError::Conflict(_))
        ));

        let conn = store.conn.lock().await;
        let state: (String, String, String, String, Option<String>, String) = conn
            .query_row(
                "SELECT intent.status, intent.cancellation_reason,
                        reservation.status, reservation.cooldown_until,
                        account.lease_owner, account.automation_mode
                 FROM market_payment_intents intent
                 JOIN market_payment_amount_reservations reservation
                   ON reservation.intent_id = intent.id
                 JOIN binance_payment_accounts account
                   ON account.id = intent.payment_account_id
                 WHERE intent.id = ?1",
                params![intent.id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("read global-disable settlement state");
        assert_eq!(state.0, "cancelled");
        assert_eq!(state.1, "global_settlement_disabled");
        assert_eq!(state.2, "cooldown");
        assert!(state.3 >= intent.expires_at);
        assert!(state.4.is_none());
        assert_eq!(state.5, "shadow");
        drop(conn);
        assert!(matches!(
            store
                .binance_create_or_refresh_intent(
                    &fixture.buyer,
                    &fixture.invoice_id,
                    "test",
                    true,
                )
                .await,
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn disabled_account_fences_stale_poll_and_stale_verification() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "stale-poll").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent before disable");
        let (stale_binding_id, stale_binding_revision) = store
            .binance_prepare_account_binding(&fixture.supplier.user_id, "test", "123456789")
            .await
            .expect("prepare binding before disable");
        let stale_aad = credential_aad(
            &stale_binding_id,
            &fixture.supplier.user_id,
            stale_binding_revision,
        );
        let (stale_ciphertext, stale_nonce) = fixture
            .cipher
            .seal_json(&fixture.credentials, stale_aad.as_bytes())
            .expect("encrypt stale binding attempt");
        let account = claim_account(&store).await;
        store
            .binance_disable_payment_account(&fixture.supplier.user_id, "test", false)
            .await
            .expect("disable payment account");
        assert!(matches!(
            store
                .binance_mark_account_verified(
                    &fixture.supplier.user_id,
                    "test",
                    &account.id,
                    account.credential_revision,
                    None,
                    &verified_permissions(),
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            store
                .binance_process_poll_success(
                    &account,
                    "test-worker",
                    &[transaction(
                        "tx-after-disable",
                        &intent.pay_amount,
                        "USDT",
                        "C2C",
                        None,
                    )],
                    &fixture.cipher,
                    true,
                    4,
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            store
                .binance_save_verified_account(
                    &fixture.supplier.user_id,
                    &stale_binding_id,
                    "test",
                    "123456789",
                    &fixture.credentials.api_key,
                    &stale_ciphertext,
                    &stale_nonce,
                    fixture.cipher.version(),
                    stale_binding_revision,
                    "enabled",
                    &verified_permissions(),
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        assert!(
            store
                .binance_load_payment_account(&fixture.supplier.user_id, "test")
                .await
                .expect("load disabled payment account")
                .is_none()
        );
        let conn = store.conn.lock().await;
        let (invoice_status, intent_status, transaction_count, account_status, account_revision): (
            String,
            String,
            i64,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT
                    (SELECT status FROM market_invoices WHERE id = ?1),
                    (SELECT status FROM market_payment_intents WHERE id = ?2),
                    (SELECT COUNT(*) FROM binance_pay_transactions
                      WHERE payment_account_id = ?3 AND transaction_id = 'tx-after-disable'),
                    (SELECT status FROM binance_payment_accounts WHERE id = ?3),
                    (SELECT credential_revision FROM binance_payment_accounts WHERE id = ?3)",
                params![fixture.invoice_id, intent.id, fixture.payment_account_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("read fenced stale-poll outcome");
        assert_eq!(invoice_status, "open");
        assert_eq!(intent_status, "cancelled");
        assert_eq!(transaction_count, 0);
        assert_eq!(account_status, "disabled");
        assert_eq!(account_revision, account.credential_revision + 1);
    }

    #[tokio::test]
    async fn manual_verification_failure_forces_safe_reverification() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "manual-reverify-failure").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent before failed verification");
        let claimed = claim_account(&store).await;
        let stored = store
            .binance_load_payment_account(&fixture.supplier.user_id, "test")
            .await
            .expect("load payment account")
            .expect("payment account");
        store
            .binance_mark_account_verification_failed(
                &fixture.supplier.user_id,
                "test",
                &stored.id,
                stored.credential_revision,
                "DANGEROUS_PERMISSION_ENABLED",
            )
            .await
            .expect("record manual verification failure");
        assert!(matches!(
            store
                .binance_process_poll_success(
                    &claimed,
                    "test-worker",
                    &[transaction(
                        "tx-after-failed-reverify",
                        &intent.pay_amount,
                        "USDT",
                        "C2C",
                        None,
                    )],
                    &fixture.cipher,
                    true,
                    4,
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let conn = store.conn.lock().await;
        let (status, verified_at, error_code, invoice_status, intent_status): (
            String,
            Option<String>,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT status, permissions_verified_at, last_poll_error_code,
                        (SELECT status FROM market_invoices WHERE id = ?2),
                        (SELECT status FROM market_payment_intents WHERE id = ?3)
                   FROM binance_payment_accounts WHERE id = ?1",
                params![fixture.payment_account_id, fixture.invoice_id, intent.id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("read failed verification state");
        assert_eq!(status, "degraded");
        assert!(verified_at.is_none());
        assert_eq!(error_code, "DANGEROUS_PERMISSION_ENABLED");
        assert_eq!(invoice_status, "open");
        assert_eq!(intent_status, "cancelled");
    }

    #[tokio::test]
    async fn credential_poll_failure_degrades_immediately_and_clears_verification() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "fatal-poll-failure").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent before fatal poll failure");
        let account = claim_account(&store).await;
        store
            .binance_record_poll_failure(
                &account.id,
                "test-worker",
                "BINANCE_CREDENTIALS_REJECTED",
                None,
            )
            .await
            .expect("record fatal poll failure");
        let conn = store.conn.lock().await;
        let state: (String, Option<String>, String, i64, String) = conn
            .query_row(
                "SELECT status, permissions_verified_at, last_poll_error_code,
                        consecutive_failures,
                        (SELECT status FROM market_payment_intents WHERE id = ?2)
                   FROM binance_payment_accounts WHERE id = ?1",
                params![fixture.payment_account_id, intent.id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("read fatal poll failure state");
        assert_eq!(state.0, "degraded");
        assert!(state.1.is_none());
        assert_eq!(state.2, "BINANCE_CREDENTIALS_REJECTED");
        assert_eq!(state.3, 1);
        assert_eq!(state.4, "cancelled");
    }

    #[tokio::test]
    async fn encryption_key_version_mismatch_blocks_new_payment_intents() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "key-version-mismatch").await;
        assert!(matches!(
            store
                .binance_create_or_refresh_intent_for_key_version(
                    &fixture.buyer,
                    &fixture.invoice_id,
                    "test",
                    fixture.cipher.version() + 1,
                    false,
                )
                .await,
            Err(AppError::ServiceUnavailable(_))
        ));
        let conn = store.conn.lock().await;
        let intent_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM market_payment_intents", [], |row| {
                row.get(0)
            })
            .expect("count intents after key mismatch");
        assert_eq!(intent_count, 0);
    }

    #[tokio::test]
    async fn same_version_wrong_master_key_degrades_account_before_returning_an_intent() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "wrong-master-key").await;
        let intent = store
            .binance_create_or_refresh_intent_for_cipher(
                &fixture.buyer,
                &fixture.invoice_id,
                "test",
                &fixture.cipher,
                false,
            )
            .await
            .expect("create intent with the configured master key");
        let wrong_cipher = CredentialCipher::new([8; 32], fixture.cipher.version());
        assert!(matches!(
            store
                .binance_create_or_refresh_intent_for_cipher(
                    &fixture.buyer,
                    &fixture.invoice_id,
                    "test",
                    &wrong_cipher,
                    false,
                )
                .await,
            Err(AppError::ServiceUnavailable(_))
        ));
        let conn = store.conn.lock().await;
        let state: (String, Option<String>, String, String, String) = conn
            .query_row(
                "SELECT status, permissions_verified_at, last_poll_error_code,
                        (SELECT status FROM market_invoices WHERE id = ?2),
                        (SELECT status FROM market_payment_intents WHERE id = ?3)
                   FROM binance_payment_accounts WHERE id = ?1",
                params![fixture.payment_account_id, fixture.invoice_id, intent.id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("read wrong-master-key failure state");
        assert_eq!(state.0, "degraded");
        assert!(state.1.is_none());
        assert_eq!(state.2, "CREDENTIAL_DECRYPT_FAILED");
        assert_eq!(state.3, "open");
        assert_eq!(state.4, "cancelled");
    }

    #[tokio::test]
    async fn unconfirmed_uid_and_stale_permissions_block_new_payment_intents() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "stale-account-proof").await;
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE binance_payment_accounts
                 SET uid_confirmed = 0, uid_confirmation_source = NULL
                 WHERE id = ?1",
                params![fixture.payment_account_id],
            )
            .expect("remove UID confirmation");
        }
        assert!(matches!(
            store
                .binance_create_or_refresh_intent(
                    &fixture.buyer,
                    &fixture.invoice_id,
                    "test",
                    false,
                )
                .await,
            Err(AppError::ServiceUnavailable(_))
        ));
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE binance_payment_accounts
                 SET uid_confirmed = 1, uid_confirmation_source = 'receiver_history',
                     permissions_verified_at = ?2
                 WHERE id = ?1",
                params![
                    fixture.payment_account_id,
                    (Utc::now() - Duration::hours(super::super::PERMISSION_REVERIFY_HOURS + 1))
                        .to_rfc3339()
                ],
            )
            .expect("age permission verification");
        }
        assert!(matches!(
            store
                .binance_create_or_refresh_intent(
                    &fixture.buyer,
                    &fixture.invoice_id,
                    "test",
                    false,
                )
                .await,
            Err(AppError::ServiceUnavailable(_))
        ));
        let conn = store.conn.lock().await;
        let intent_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM market_payment_intents", [], |row| {
                row.get(0)
            })
            .expect("count intents after account-proof failures");
        assert_eq!(intent_count, 0);
    }

    #[tokio::test]
    async fn intent_is_idempotent_and_refresh_and_cancel_keep_amounts_cooled() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "intent-lifecycle").await;
        let first = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent");
        let reused = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("reuse intent");
        assert_eq!(first.id, reused.id);
        assert_eq!(first.pay_amount, reused.pay_amount);
        assert!(matches!(
            store
                .binance_create_or_refresh_intent(
                    &fixture.buyer,
                    &fixture.invoice_id,
                    "test",
                    true,
                )
                .await,
            Err(AppError::RateLimited { .. })
        ));

        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_payment_intents SET created_at = ?2 WHERE id = ?1",
                params![
                    first.id,
                    (Utc::now() - Duration::seconds(INTENT_REFRESH_MIN_SECONDS + 1)).to_rfc3339()
                ],
            )
            .expect("age intent before refresh");
        }

        let refreshed = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", true)
            .await
            .expect("refresh intent");
        assert_ne!(first.id, refreshed.id);
        assert_ne!(first.pay_amount, refreshed.pay_amount);
        let cancelled = store
            .binance_cancel_intent(&fixture.buyer, &fixture.invoice_id)
            .await
            .expect("cancel intent");
        assert_eq!(cancelled.status, "cancelled");
        let reopened = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("reopening must not silently allocate a replacement amount");
        assert_eq!(reopened.id, cancelled.id);
        assert_eq!(reopened.status, "cancelled");
        let conn = store.conn.lock().await;
        let lifecycle: Vec<(String, String)> = conn
            .prepare(
                "SELECT intent.status, reservation.status
                 FROM market_payment_intents intent
                 JOIN market_payment_amount_reservations reservation
                   ON reservation.intent_id = intent.id
                 WHERE intent.invoice_id = ?1 ORDER BY intent.created_at, intent.id",
            )
            .expect("prepare lifecycle query")
            .query_map(params![fixture.invoice_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("query lifecycle")
            .collect::<Result<Vec<_>, _>>()
            .expect("read lifecycle");
        assert_eq!(
            lifecycle,
            vec![
                ("cancelled".into(), "cooldown".into()),
                ("cancelled".into(), "cooldown".into())
            ]
        );
    }

    #[tokio::test]
    async fn amount_allocator_fails_closed_after_all_suffixes_are_reserved() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "allocator-full").await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().await;
            let tx = conn.transaction().expect("begin reservation fixture");
            for suffix in MIN_SUFFIX..=MAX_SUFFIX {
                tx.execute(
                    "INSERT INTO market_payment_amount_reservations (
                        id, payment_account_id, asset, pay_amount_units, intent_id,
                        status, reserved_at
                     ) VALUES (?1, ?2, 'USDT', ?3, ?4, 'reserved', ?5)",
                    params![
                        format!("reservation-{suffix}"),
                        fixture.payment_account_id,
                        100_000 + suffix,
                        format!("synthetic-intent-{suffix}"),
                        now
                    ],
                )
                .expect("reserve suffix");
            }
            tx.commit().expect("commit reservation fixture");
        }
        let error = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect_err("allocator must fail instead of reusing an amount");
        assert!(error.to_string().contains("no safe Binance payment amount"));
    }

    #[tokio::test]
    async fn exact_payment_is_atomic_idempotent_and_confirms_account_scope() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "exact").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent");
        let account = claim_account(&store).await;
        let payment = transaction("tx-exact", &intent.pay_amount, "USDT", "C2C", None);
        let actions = store
            .binance_process_poll_success(
                &account,
                "test-worker",
                &[payment.clone(), payment],
                &fixture.cipher,
                true,
                4,
            )
            .await
            .expect("process exact payment");
        assert!(actions.is_empty());
        let conn = store.conn.lock().await;
        let invoice_status: String = conn
            .query_row(
                "SELECT status FROM market_invoices WHERE id = ?1",
                params![fixture.invoice_id],
                |row| row.get(0),
            )
            .expect("read paid invoice");
        let (intent_status, transaction_count, receipt_count, uid_confirmed): (
            String,
            i64,
            i64,
            i64,
        ) = conn
            .query_row(
                "SELECT
                    (SELECT status FROM market_payment_intents WHERE id = ?1),
                    (SELECT COUNT(*) FROM binance_pay_transactions
                     WHERE payment_account_id = ?2 AND transaction_id = 'tx-exact'),
                    (SELECT COUNT(*) FROM market_external_payment_receipts
                     WHERE payment_account_id = ?2 AND transaction_id = 'tx-exact'),
                    (SELECT uid_confirmed FROM binance_payment_accounts WHERE id = ?2)",
                params![intent.id, fixture.payment_account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read settlement state");
        assert_eq!(invoice_status, "paid");
        assert_eq!(intent_status, "paid");
        assert_eq!(transaction_count, 1);
        assert_eq!(receipt_count, 1);
        assert_eq!(uid_confirmed, 1);
        let transaction_account_snapshot: (String, i64, i64) = conn
            .query_row(
                "SELECT account_binance_uid, account_credential_revision,
                        encryption_key_version
                   FROM binance_pay_transactions
                  WHERE payment_account_id = ?1 AND transaction_id = 'tx-exact'",
                params![fixture.payment_account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read transaction account snapshot");
        assert_eq!(
            transaction_account_snapshot,
            ("123456789".into(), 1, fixture.cipher.version())
        );
    }

    #[tokio::test]
    async fn shadow_exact_match_is_auditable_without_changing_billing() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "shadow-match").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create shadow-test intent");
        let account = claim_account(&store).await;
        store
            .binance_process_poll_success(
                &account,
                "test-worker",
                &[transaction(
                    "tx-shadow",
                    &intent.pay_amount,
                    "USDT",
                    "C2C",
                    None,
                )],
                &fixture.cipher,
                false,
                4,
            )
            .await
            .expect("process shadow payment");
        let conn = store.conn.lock().await;
        let (invoice_status, intent_status, match_status, case_kind, receipt_count, case_id): (
            String,
            String,
            String,
            String,
            i64,
            String,
        ) = conn
            .query_row(
                "SELECT
                    (SELECT status FROM market_invoices WHERE id = ?1),
                    (SELECT status FROM market_payment_intents WHERE id = ?2),
                    (SELECT match_status FROM binance_pay_transactions
                      WHERE payment_account_id = ?3 AND transaction_id = 'tx-shadow'),
                    (SELECT case_kind FROM market_payment_reconciliation_cases
                      WHERE payment_account_id = ?3 AND transaction_id = 'tx-shadow'),
                    (SELECT COUNT(*) FROM market_external_payment_receipts
                      WHERE payment_account_id = ?3 AND transaction_id = 'tx-shadow'),
                    (SELECT id FROM market_payment_reconciliation_cases
                      WHERE payment_account_id = ?3 AND transaction_id = 'tx-shadow')",
                params![fixture.invoice_id, intent.id, fixture.payment_account_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("read shadow-match outcome");
        assert_eq!(invoice_status, "open");
        assert_eq!(intent_status, "review_required");
        assert_eq!(match_status, "review_required");
        assert_eq!(case_kind, "shadow_exact_match");
        assert_eq!(receipt_count, 0);
        drop(conn);

        let reviewed = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("reuse reviewed intent instead of asking the buyer to pay twice");
        assert_eq!(reviewed.id, intent.id);
        assert_eq!(reviewed.status, "review_required");

        let admin = session("admin-shadow-match");
        assert!(
            store
                .binance_resolve_reconciliation_case(&admin, &case_id, "ignore", None, None)
                .await
                .expect("ignore shadow transaction")
                .is_empty()
        );
        let ignored = store
            .binance_intent_for_invoice(&fixture.buyer, &fixture.invoice_id)
            .await
            .expect("read ignored shadow intent")
            .expect("ignored shadow intent exists");
        assert_eq!(ignored.status, "cancelled");
        assert_eq!(
            ignored.cancellation_reason.as_deref(),
            Some("admin_ignored_transaction")
        );
        let replacement = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", true)
            .await
            .expect("create a fresh intent after the reviewed transaction is ignored");
        assert_ne!(replacement.id, intent.id);
    }

    #[tokio::test]
    async fn first_poll_starts_from_the_earliest_live_intent() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "initial-window").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent");
        let started_at = (Utc::now() - Duration::hours(5)).to_rfc3339();
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_payment_intents SET created_at = ?2 WHERE id = ?1",
                params![intent.id, started_at],
            )
            .expect("age intent before first poll");
            conn.execute(
                "UPDATE binance_payment_accounts
                    SET poll_cursor_at = NULL, next_poll_at = ?2 WHERE id = ?1",
                params![fixture.payment_account_id, Utc::now().to_rfc3339()],
            )
            .expect("clear initial poll cursor");
        }
        let account = claim_account(&store).await;
        assert_eq!(
            account.active_intent_started_at.as_deref(),
            Some(started_at.as_str())
        );
    }

    #[tokio::test]
    async fn expired_intent_requires_explicit_replacement_and_old_amount_enters_review() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "expired-replacement").await;
        let expired = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent to expire");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_payment_intents
                 SET status = 'expired', expires_at = ?2 WHERE id = ?1",
                params![expired.id, (Utc::now() - Duration::minutes(1)).to_rfc3339()],
            )
            .expect("expire old intent");
        }

        let reopened = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("reopen expired intent without replacing it");
        assert_eq!(reopened.id, expired.id);
        assert_eq!(reopened.status, "expired");

        let replacement = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", true)
            .await
            .expect("explicitly replace expired intent");
        assert_ne!(replacement.id, expired.id);
        assert_ne!(replacement.pay_amount, expired.pay_amount);

        let account = claim_account(&store).await;
        store
            .binance_process_poll_success(
                &account,
                "test-worker",
                &[transaction(
                    "tx-old-expired-amount",
                    &expired.pay_amount,
                    "USDT",
                    "C2C",
                    None,
                )],
                &fixture.cipher,
                true,
                4,
            )
            .await
            .expect("ingest the replaced amount as a review case");

        let conn = store.conn.lock().await;
        let state: (String, String, String, String, i64) = conn
            .query_row(
                "SELECT
                    (SELECT status FROM market_invoices WHERE id = ?1),
                    (SELECT status FROM market_payment_intents WHERE id = ?2),
                    (SELECT cancellation_reason FROM market_payment_intents WHERE id = ?2),
                    (SELECT status FROM market_payment_intents WHERE id = ?3),
                    (SELECT COUNT(*) FROM market_payment_reconciliation_cases
                      WHERE payment_account_id = ?4
                        AND transaction_id = 'tx-old-expired-amount'
                        AND status = 'open')",
                params![
                    fixture.invoice_id,
                    expired.id,
                    replacement.id,
                    fixture.payment_account_id
                ],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("read explicit replacement state");
        assert_eq!(state.0, "open");
        assert_eq!(state.1, "cancelled");
        assert_eq!(state.2, "buyer_refreshed");
        assert_eq!(state.3, "pending");
        assert_eq!(state.4, 1);
    }

    #[tokio::test]
    async fn late_payment_before_grace_end_can_settle_an_expired_intent() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "late").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE market_payment_intents SET status = 'expired', expires_at = ?2
                 WHERE id = ?1",
                params![intent.id, (Utc::now() - Duration::minutes(1)).to_rfc3339()],
            )
            .expect("expire intent");
        }
        let account = claim_account(&store).await;
        store
            .binance_process_poll_success(
                &account,
                "test-worker",
                &[transaction(
                    "tx-late",
                    &intent.pay_amount,
                    "USDT",
                    "C2C",
                    None,
                )],
                &fixture.cipher,
                true,
                4,
            )
            .await
            .expect("settle late payment");
        let conn = store.conn.lock().await;
        let status: String = conn
            .query_row(
                "SELECT status FROM market_invoices WHERE id = ?1",
                params![fixture.invoice_id],
                |row| row.get(0),
            )
            .expect("read late-settled invoice");
        assert_eq!(status, "paid");
    }

    #[tokio::test]
    async fn buyer_cancelled_amount_remains_monitored_for_late_payment_review() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "cancelled-late-review").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent");
        store
            .binance_cancel_intent(&fixture.buyer, &fixture.invoice_id)
            .await
            .expect("cancel intent");
        let account = claim_account(&store).await;
        store
            .binance_process_poll_success(
                &account,
                "test-worker",
                &[transaction(
                    "tx-after-buyer-cancel",
                    &intent.pay_amount,
                    "USDT",
                    "C2C",
                    None,
                )],
                &fixture.cipher,
                true,
                4,
            )
            .await
            .expect("ingest payment after buyer cancellation");
        let conn = store.conn.lock().await;
        let state: (String, String, String, i64) = conn
            .query_row(
                "SELECT
                    (SELECT status FROM market_invoices WHERE id = ?1),
                    (SELECT status FROM market_payment_intents WHERE id = ?2),
                    (SELECT match_status FROM binance_pay_transactions
                      WHERE payment_account_id = ?3
                        AND transaction_id = 'tx-after-buyer-cancel'),
                    (SELECT COUNT(*) FROM market_payment_reconciliation_cases
                      WHERE payment_account_id = ?3
                        AND transaction_id = 'tx-after-buyer-cancel'
                        AND status = 'open')",
                params![fixture.invoice_id, intent.id, fixture.payment_account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read late cancelled-payment state");
        assert_eq!(state.0, "open");
        assert_eq!(state.1, "cancelled");
        assert_eq!(state.2, "review_required");
        assert_eq!(state.3, 1);
    }

    #[tokio::test]
    async fn invalid_receiver_asset_and_order_type_never_auto_settle() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "invalid-transactions").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent");
        let account = claim_account(&store).await;
        let mut account_is_payer = transaction(
            "tx-account-is-payer",
            &intent.pay_amount,
            "USDT",
            "C2C",
            None,
        );
        account_is_payer.payer_info.binance_id = serde_json::json!(account.binance_uid.clone());
        store
            .binance_process_poll_success(
                &account,
                "test-worker",
                &[
                    transaction(
                        "tx-wrong-uid",
                        &intent.pay_amount,
                        "USDT",
                        "C2C",
                        Some("999999999"),
                    ),
                    transaction("tx-wrong-asset", &intent.pay_amount, "USDC", "C2C", None),
                    transaction("tx-wrong-order", &intent.pay_amount, "USDT", "PAYOUT", None),
                    account_is_payer,
                    transaction("tx-unrelated", "1.2345", "USDT", "C2C", None),
                ],
                &fixture.cipher,
                true,
                4,
            )
            .await
            .expect("ingest invalid payments");
        let conn = store.conn.lock().await;
        let invoice_status: String = conn
            .query_row(
                "SELECT status FROM market_invoices WHERE id = ?1",
                params![fixture.invoice_id],
                |row| row.get(0),
            )
            .expect("read invoice status");
        let rows: Vec<(String, String, String)> = conn
            .prepare(
                "SELECT transaction_id, match_status, raw_payload_ciphertext
                   FROM binance_pay_transactions
                 WHERE payment_account_id = ?1 ORDER BY transaction_id",
            )
            .expect("prepare transaction state query")
            .query_map(params![fixture.payment_account_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("query transaction states")
            .collect::<Result<Vec<_>, _>>()
            .expect("read transaction states");
        let cases: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_payment_reconciliation_cases
                 WHERE payment_account_id = ?1 AND status = 'open'",
                params![fixture.payment_account_id],
                |row| row.get(0),
            )
            .expect("count review cases");
        assert_eq!(invoice_status, "open");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "tx-unrelated");
        assert_eq!(rows[0].1, "ignored");
        assert!(rows[0].2.is_empty());
        assert_eq!(rows[1].0, "tx-wrong-order");
        assert_eq!(rows[1].1, "review_required");
        assert!(!rows[1].2.is_empty());
        assert_eq!(cases, 1);
    }

    #[tokio::test]
    async fn manual_declaration_wins_race_and_late_auto_match_enters_review() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "manual-race").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent");
        let account = claim_account(&store).await;
        store
            .market_billing_declare_payment(
                &fixture.buyer,
                &fixture.invoice_id,
                Some("binance".into()),
                Some("manual-reference".into()),
                None,
                None,
            )
            .await
            .expect("declare manual payment");
        store
            .binance_process_poll_success(
                &account,
                "test-worker",
                &[transaction(
                    "tx-raced",
                    &intent.pay_amount,
                    "USDT",
                    "C2C",
                    None,
                )],
                &fixture.cipher,
                true,
                4,
            )
            .await
            .expect("ingest raced payment");
        let conn = store.conn.lock().await;
        let (invoice_status, intent_status, match_status, case_count): (
            String,
            String,
            String,
            i64,
        ) = conn
            .query_row(
                "SELECT
                    (SELECT status FROM market_invoices WHERE id = ?1),
                    (SELECT status FROM market_payment_intents WHERE id = ?2),
                    (SELECT match_status FROM binance_pay_transactions
                     WHERE payment_account_id = ?3 AND transaction_id = 'tx-raced'),
                    (SELECT COUNT(*) FROM market_payment_reconciliation_cases
                     WHERE payment_account_id = ?3 AND transaction_id = 'tx-raced')",
                params![fixture.invoice_id, intent.id, fixture.payment_account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read race outcome");
        assert_eq!(invoice_status, "payment_declared");
        assert_eq!(intent_status, "cancelled");
        assert_eq!(match_status, "review_required");
        assert_eq!(case_count, 1);
    }

    #[tokio::test]
    async fn dispute_and_void_atomically_cancel_pending_intents() {
        let dispute_store = AppStore::new_in_memory_for_tests().expect("test store");
        let dispute_fixture = settlement_fixture(&dispute_store, "dispute-cancel").await;
        let dispute_intent = dispute_store
            .binance_create_or_refresh_intent(
                &dispute_fixture.buyer,
                &dispute_fixture.invoice_id,
                "test",
                false,
            )
            .await
            .expect("create dispute intent");
        dispute_store
            .market_billing_open_dispute(
                &dispute_fixture.buyer,
                &dispute_fixture.invoice_id,
                "payment issue",
            )
            .await
            .expect("open dispute");
        {
            let conn = dispute_store.conn.lock().await;
            let status: String = conn
                .query_row(
                    "SELECT status FROM market_payment_intents WHERE id = ?1",
                    params![dispute_intent.id],
                    |row| row.get(0),
                )
                .expect("read disputed intent");
            assert_eq!(status, "cancelled");
        }

        let void_store = AppStore::new_in_memory_for_tests().expect("test store");
        let void_fixture = settlement_fixture(&void_store, "void-cancel").await;
        let void_intent = void_store
            .binance_create_or_refresh_intent(
                &void_fixture.buyer,
                &void_fixture.invoice_id,
                "test",
                false,
            )
            .await
            .expect("create void intent");
        void_store
            .market_billing_void_invoice(
                &void_fixture.supplier,
                &void_fixture.invoice_id,
                "admin correction",
            )
            .await
            .expect("void invoice");
        let conn = void_store.conn.lock().await;
        let status: String = conn
            .query_row(
                "SELECT status FROM market_payment_intents WHERE id = ?1",
                params![void_intent.id],
                |row| row.get(0),
            )
            .expect("read voided intent");
        assert_eq!(status, "cancelled");
    }

    #[tokio::test]
    async fn admin_can_audit_and_settle_a_reviewed_transaction() {
        let store = AppStore::new_in_memory_for_tests().expect("test store");
        let fixture = settlement_fixture(&store, "admin-review").await;
        let intent = store
            .binance_create_or_refresh_intent(&fixture.buyer, &fixture.invoice_id, "test", false)
            .await
            .expect("create intent");
        let account = claim_account(&store).await;
        store
            .binance_process_poll_success(
                &account,
                "test-worker",
                &[transaction(
                    "tx-admin",
                    &intent.pay_amount,
                    "USDT",
                    "UNKNOWN",
                    None,
                )],
                &fixture.cipher,
                true,
                4,
            )
            .await
            .expect("ingest review transaction");
        let overview = store
            .binance_admin_reconciliation()
            .await
            .expect("load admin reconciliation");
        assert_eq!(overview.open_case_count, 1);
        let case_id = overview.cases[0].id.clone();
        let admin = session("admin-reviewer");
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE binance_pay_transactions
                 SET account_binance_uid = '987654321'
                 WHERE payment_account_id = ?1 AND transaction_id = 'tx-admin'",
                params![fixture.payment_account_id],
            )
            .expect("simulate transaction from rebound Binance UID");
        }
        let error = store
            .binance_resolve_reconciliation_case(
                &admin,
                &case_id,
                "settle",
                Some(&fixture.invoice_id),
                Some("verified against Binance statement"),
            )
            .await
            .expect_err("reject transaction from a different Binance UID");
        assert!(matches!(error, AppError::Conflict(_)));
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE binance_pay_transactions
                 SET account_binance_uid = '123456789'
                 WHERE payment_account_id = ?1 AND transaction_id = 'tx-admin'",
                params![fixture.payment_account_id],
            )
            .expect("restore transaction Binance UID");
        }
        store
            .binance_resolve_reconciliation_case(
                &admin,
                &case_id,
                "settle",
                Some(&fixture.invoice_id),
                Some("verified against Binance statement"),
            )
            .await
            .expect("settle reviewed transaction");
        let conn = store.conn.lock().await;
        let (invoice_status, case_status, source): (String, String, String) = conn
            .query_row(
                "SELECT
                    (SELECT status FROM market_invoices WHERE id = ?1),
                    (SELECT status FROM market_payment_reconciliation_cases WHERE id = ?2),
                    (SELECT source FROM market_external_payment_receipts WHERE invoice_id = ?1)",
                params![fixture.invoice_id, case_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read reconciled settlement");
        assert_eq!(invoice_status, "paid");
        assert_eq!(case_status, "settled");
        assert_eq!(source, "admin_reconciliation");
    }
}
