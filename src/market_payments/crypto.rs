use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use zeroize::{Zeroize, Zeroizing};

use crate::error::AppError;

const KEY_FILE: &str = "market-payment-master-key";
const NONCE_LEN: usize = 24;

#[derive(Clone)]
pub(super) struct PaymentCipher {
    key: Arc<Zeroizing<[u8; 32]>>,
    pub(super) version: i64,
}

impl PaymentCipher {
    pub(super) fn load(data_dir: &Path) -> Result<Self> {
        let key =
            if let Ok(configured) = std::env::var("CC_SWITCH_ROUTER_MARKET_PAYMENT_MASTER_KEY") {
                parse_key(&configured).context("invalid market payment master key override")?
            } else {
                let path = data_dir.join(KEY_FILE);
                if !path.exists() {
                    let mut generated = Zeroizing::new([0_u8; 32]);
                    rand::rngs::OsRng.fill_bytes(generated.as_mut());
                    let encoded = Zeroizing::new(format!("{}\n", hex::encode(generated.as_ref())));
                    crate::secure_file::atomic_create_file_mode(&path, encoded.as_bytes(), 0o600)
                        .with_context(|| format!("create market payment key: {}", path.display()))?;
                }
                crate::secure_file::enforce_file_mode(&path, 0o600)
                    .with_context(|| format!("protect market payment key: {}", path.display()))?;
                let encoded =
                    Zeroizing::new(fs::read_to_string(&path).context("read market payment key")?);
                parse_key(&encoded).context("invalid market payment key file")?
            };
        Ok(Self {
            key: Arc::new(key),
            version: 1,
        })
    }

    #[cfg(test)]
    pub(super) fn for_test() -> Self {
        Self {
            key: Arc::new(Zeroizing::new([23_u8; 32])),
            version: 1,
        }
    }

    pub(super) fn seal(&self, secret: &str, aad: &str) -> Result<(String, String), AppError> {
        let cipher = XChaCha20Poly1305::new((&**self.key).into());
        let mut nonce = [0_u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: secret.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| AppError::Internal("encrypt market payment secret failed".into()))?;
        let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        Ok((encoder.encode(ciphertext), encoder.encode(nonce)))
    }

    pub(super) fn open(
        &self,
        ciphertext: &str,
        nonce: &str,
        aad: &str,
        key_version: i64,
    ) -> Result<Zeroizing<String>, AppError> {
        if key_version != self.version {
            return Err(AppError::Internal(
                "market payment key version unavailable".into(),
            ));
        }
        let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let encrypted = encoder
            .decode(ciphertext)
            .map_err(|_| AppError::Internal("invalid market payment ciphertext".into()))?;
        let nonce = encoder
            .decode(nonce)
            .map_err(|_| AppError::Internal("invalid market payment nonce".into()))?;
        if nonce.len() != NONCE_LEN {
            return Err(AppError::Internal(
                "invalid market payment nonce length".into(),
            ));
        }
        let cipher = XChaCha20Poly1305::new((&**self.key).into());
        let mut plaintext = cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &encrypted,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| {
                AppError::Internal("market payment secret authentication failed".into())
            })?;
        let decoded = String::from_utf8(plaintext.clone())
            .map(Zeroizing::new)
            .map_err(|_| AppError::Internal("invalid market payment secret encoding".into()));
        plaintext.zeroize();
        decoded
    }
}

fn parse_key(value: &str) -> Result<Zeroizing<[u8; 32]>> {
    let raw = value.trim();
    let mut decoded = Zeroizing::new(
        if raw.len() == 64 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            hex::decode(raw).context("invalid hex key")?
        } else {
            base64::engine::general_purpose::STANDARD
                .decode(raw)
                .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw))
                .context("key must be hex or base64")?
        },
    );
    if decoded.len() != 32 {
        bail!("market payment key must contain 32 bytes");
    }
    let mut key = Zeroizing::new([0_u8; 32]);
    key.copy_from_slice(&decoded);
    decoded.zeroize();
    Ok(key)
}

pub(super) fn aad(
    channel_id: &str,
    revision: i64,
    merchant_fingerprint: &str,
    kind: &str,
) -> String {
    format!("market-payment:v1:{channel_id}:{revision}:{merchant_fingerprint}:{kind}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_bound_to_merchant_and_revision() {
        let cipher = PaymentCipher::for_test();
        let context = aad("channel", 1, "merchant-1", "api");
        let (ciphertext, nonce) = cipher.seal("secret", &context).unwrap();
        assert_eq!(
            cipher
                .open(&ciphertext, &nonce, &context, 1)
                .unwrap()
                .as_str(),
            "secret"
        );
        assert!(
            cipher
                .open(
                    &ciphertext,
                    &nonce,
                    &aad("channel", 2, "merchant-1", "api"),
                    1
                )
                .is_err()
        );
        assert!(
            cipher
                .open(
                    &ciphertext,
                    &nonce,
                    &aad("channel", 1, "merchant-2", "api"),
                    1
                )
                .is_err()
        );
        assert!(cipher.open(&ciphertext, &nonce, &context, 2).is_err());
    }
}
