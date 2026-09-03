use std::sync::Arc;

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::AppError;

const NONCE_LEN: usize = 24;

#[derive(Clone)]
pub struct CredentialCipher {
    key: Arc<Zeroizing<[u8; 32]>>,
    fingerprint_key: Arc<Zeroizing<[u8; 32]>>,
    version: i64,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct BinanceCredentials {
    pub api_key: String,
    pub api_secret: String,
}

impl std::fmt::Debug for BinanceCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BinanceCredentials")
            .field("api_key", &"[REDACTED]")
            .field("api_secret", &"[REDACTED]")
            .finish()
    }
}

impl CredentialCipher {
    pub fn new(mut key: [u8; 32], version: i64) -> Self {
        let protected_key = Zeroizing::new(key);
        key.zeroize();
        Self::from_zeroizing(protected_key, version)
    }

    pub fn from_zeroizing(key: Zeroizing<[u8; 32]>, version: i64) -> Self {
        let mut derivation = <Hmac<Sha256> as Mac>::new_from_slice(&*key)
            .expect("HMAC-SHA256 accepts a 32-byte key");
        derivation.update(b"cc-switch-router:binance-counterparty-fingerprint:v1");
        let mut fingerprint_key: [u8; 32] = derivation.finalize().into_bytes().into();
        let protected_fingerprint_key = Zeroizing::new(fingerprint_key);
        fingerprint_key.zeroize();
        Self {
            key: Arc::new(key),
            fingerprint_key: Arc::new(protected_fingerprint_key),
            version,
        }
    }

    pub fn version(&self) -> i64 {
        self.version
    }

    pub fn fingerprint(&self, context: &[u8], value: &[u8]) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&**self.fingerprint_key)
            .expect("HMAC-SHA256 accepts a 32-byte key");
        mac.update(context);
        mac.update(&[0]);
        mac.update(value);
        hex::encode(mac.finalize().into_bytes())
    }

    pub fn seal_json<T: Serialize>(
        &self,
        value: &T,
        associated_data: &[u8],
    ) -> Result<(String, String), AppError> {
        let mut plaintext = serde_json::to_vec(value)
            .map_err(|_| AppError::Internal("encode protected Binance data failed".into()))?;
        let result = self.seal(&plaintext, associated_data);
        plaintext.zeroize();
        result
    }

    pub fn open_json<T: for<'de> Deserialize<'de>>(
        &self,
        ciphertext: &str,
        nonce: &str,
        associated_data: &[u8],
    ) -> Result<T, AppError> {
        let mut plaintext = self.open(ciphertext, nonce, associated_data)?;
        let decoded = serde_json::from_slice(&plaintext)
            .map_err(|_| AppError::Internal("decode protected Binance data failed".into()));
        plaintext.zeroize();
        decoded
    }

    fn seal(&self, plaintext: &[u8], associated_data: &[u8]) -> Result<(String, String), AppError> {
        let cipher = XChaCha20Poly1305::new((&**self.key).into());
        let mut nonce = [0_u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let encrypted = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: associated_data,
                },
            )
            .map_err(|_| AppError::Internal("encrypt protected Binance data failed".into()))?;
        let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        Ok((encoder.encode(encrypted), encoder.encode(nonce)))
    }

    fn open(
        &self,
        ciphertext: &str,
        nonce: &str,
        associated_data: &[u8],
    ) -> Result<Vec<u8>, AppError> {
        let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let ciphertext = encoder
            .decode(ciphertext)
            .map_err(|_| AppError::Internal("stored Binance ciphertext is invalid".into()))?;
        let nonce = encoder
            .decode(nonce)
            .map_err(|_| AppError::Internal("stored Binance nonce is invalid".into()))?;
        if nonce.len() != NONCE_LEN {
            return Err(AppError::Internal(
                "stored Binance nonce has an invalid length".into(),
            ));
        }
        let cipher = XChaCha20Poly1305::new((&**self.key).into());
        cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: associated_data,
                },
            )
            .map_err(|_| AppError::Internal("decrypt protected Binance data failed".into()))
    }
}

pub fn credential_aad(account_id: &str, supplier_user_id: &str, revision: i64) -> String {
    format!("binance-credentials:{account_id}:{supplier_user_id}:{revision}")
}

pub fn transaction_aad(account_id: &str, transaction_id: &str) -> String {
    format!("binance-transaction:{account_id}:{transaction_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciphertext_is_bound_to_its_account_context() {
        let cipher = CredentialCipher::new([7; 32], 1);
        let credentials = BinanceCredentials {
            api_key: "api-key".into(),
            api_secret: "api-secret".into(),
        };
        let (sealed, nonce) = cipher
            .seal_json(&credentials, b"account-a")
            .expect("seal credentials");
        let decoded: BinanceCredentials = cipher
            .open_json(&sealed, &nonce, b"account-a")
            .expect("open credentials");
        assert_eq!(decoded.api_key, "api-key");
        assert!(
            cipher
                .open_json::<BinanceCredentials>(&sealed, &nonce, b"account-b")
                .is_err()
        );
        assert!(!sealed.contains("api-key"));
        assert!(!sealed.contains("api-secret"));
        let fingerprint = cipher.fingerprint(b"account-a", b"123456789");
        assert_eq!(fingerprint, cipher.fingerprint(b"account-a", b"123456789"));
        assert_ne!(fingerprint, cipher.fingerprint(b"account-b", b"123456789"));
        assert!(!fingerprint.contains("123456789"));
    }
}
