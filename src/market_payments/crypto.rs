use std::collections::BTreeMap;
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
const KEY_RING_ENV: &str = "CC_SWITCH_ROUTER_MARKET_PAYMENT_MASTER_KEYS";
const SINGLE_KEY_ENV: &str = "CC_SWITCH_ROUTER_MARKET_PAYMENT_MASTER_KEY";
const NONCE_LEN: usize = 24;

/// Versioned master keys. New secrets are sealed with the highest version;
/// each stored secret is opened with the version recorded beside it, so a
/// key can be rotated by adding a version, re-wrapping, then retiring the old
/// one.
#[derive(Clone)]
pub(super) struct PaymentCipher {
    keys: Arc<BTreeMap<i64, Zeroizing<[u8; 32]>>>,
    pub(super) version: i64,
}

impl PaymentCipher {
    /// Load the key ring.
    ///
    /// * `CC_SWITCH_ROUTER_MARKET_PAYMENT_MASTER_KEYS="2:<key>,1:<key>"`
    ///   provides explicit versions.
    /// * Otherwise version 1 comes from `..._MASTER_KEY` or the data-dir key
    ///   file. The file is generated only when no credentials exist yet;
    ///   regenerating it under existing secrets would silently orphan them.
    pub(super) fn load(data_dir: &Path, secrets_exist: bool) -> Result<Self> {
        let mut keys = BTreeMap::new();
        if let Ok(ring) = std::env::var(KEY_RING_ENV) {
            for entry in ring.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
                let (version, key) = entry
                    .split_once(':')
                    .context("master key ring entries must be <version>:<key>")?;
                let version = version
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .filter(|version| *version > 0)
                    .context("master key version must be a positive integer")?;
                if keys.insert(version, parse_key(key)?).is_some() {
                    bail!("duplicate market payment master key version {version}");
                }
            }
        }
        if !keys.contains_key(&1) {
            if let Ok(configured) = std::env::var(SINGLE_KEY_ENV) {
                keys.insert(
                    1,
                    parse_key(&configured).context("invalid market payment master key override")?,
                );
            } else {
                let path = data_dir.join(KEY_FILE);
                if path.exists() {
                    crate::secure_file::enforce_file_mode(&path, 0o600).with_context(|| {
                        format!("protect market payment key: {}", path.display())
                    })?;
                    let encoded = Zeroizing::new(
                        fs::read_to_string(&path).context("read market payment key")?,
                    );
                    keys.insert(1, parse_key(&encoded).context("invalid market payment key file")?);
                } else if keys.is_empty() {
                    if secrets_exist {
                        bail!(
                            "market payment master key {} is missing but payment credentials exist; \
                             restore the key file or set {SINGLE_KEY_ENV}",
                            path.display()
                        );
                    }
                    let mut generated = Zeroizing::new([0_u8; 32]);
                    rand::rngs::OsRng.fill_bytes(generated.as_mut());
                    let encoded = Zeroizing::new(format!("{}\n", hex::encode(generated.as_ref())));
                    crate::secure_file::atomic_create_file_mode(&path, encoded.as_bytes(), 0o600)
                        .with_context(|| format!("create market payment key: {}", path.display()))?;
                    keys.insert(1, generated);
                }
            }
        }
        let version = *keys.keys().next_back().context("no market payment master key")?;
        Ok(Self {
            keys: Arc::new(keys),
            version,
        })
    }

    #[cfg(test)]
    pub(super) fn for_test() -> Self {
        Self::for_test_ring(&[(1, 23)])
    }

    #[cfg(test)]
    pub(super) fn for_test_ring(entries: &[(i64, u8)]) -> Self {
        let keys = entries
            .iter()
            .map(|(version, byte)| (*version, Zeroizing::new([*byte; 32])))
            .collect::<BTreeMap<_, _>>();
        let version = *keys.keys().next_back().unwrap();
        Self {
            keys: Arc::new(keys),
            version,
        }
    }

    pub(super) fn seal(&self, secret: &str, aad: &str) -> Result<(String, String), AppError> {
        let key = self
            .keys
            .get(&self.version)
            .ok_or_else(|| AppError::Internal("market payment key unavailable".into()))?;
        let cipher = XChaCha20Poly1305::new((&**key).into());
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
        let key = self.keys.get(&key_version).ok_or_else(|| {
            AppError::Internal("market payment key version unavailable".into())
        })?;
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
        let cipher = XChaCha20Poly1305::new((&**key).into());
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

    #[test]
    fn key_ring_opens_old_versions_and_seals_with_the_newest() {
        let old = PaymentCipher::for_test_ring(&[(1, 7)]);
        let ring = PaymentCipher::for_test_ring(&[(1, 7), (2, 9)]);
        assert_eq!(ring.version, 2);
        let context = aad("channel", 1, "merchant", "api");
        let (ciphertext, nonce) = old.seal("secret", &context).unwrap();
        assert_eq!(ring.open(&ciphertext, &nonce, &context, 1).unwrap().as_str(), "secret");
        let (resealed, renonce) = ring.seal("secret", &context).unwrap();
        assert!(old.open(&resealed, &renonce, &context, 2).is_err());
        assert_eq!(ring.open(&resealed, &renonce, &context, 2).unwrap().as_str(), "secret");
    }

    #[test]
    fn missing_key_file_is_never_regenerated_under_existing_secrets() {
        let dir = std::env::temp_dir().join(format!("ccr-key-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Environment overrides would make this test non-deterministic.
        if std::env::var(KEY_RING_ENV).is_ok() || std::env::var(SINGLE_KEY_ENV).is_ok() {
            return;
        }
        assert!(PaymentCipher::load(&dir, true).is_err());
        assert!(!dir.join(KEY_FILE).exists());
        let created = PaymentCipher::load(&dir, false).unwrap();
        assert_eq!(created.version, 1);
        let reloaded = PaymentCipher::load(&dir, true).unwrap();
        let context = aad("c", 1, "m", "api");
        let (ciphertext, nonce) = created.seal("s", &context).unwrap();
        assert_eq!(reloaded.open(&ciphertext, &nonce, &context, 1).unwrap().as_str(), "s");
        let _ = std::fs::remove_dir_all(dir);
    }
}
