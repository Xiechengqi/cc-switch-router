//! Short-lived password email payloads never enter the database as plaintext.
use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use base64::Engine;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use rand::RngCore;
use zeroize::Zeroizing;

pub(super) const SEALED_PREFIX: &str = "password-notice:v1:";
const KEY_FILE: &str = "password-notice-key";

pub(super) struct NoticeCipher(Zeroizing<[u8; 32]>);

impl NoticeCipher {
    pub(super) fn load(data_dir: &Path, allow_create: bool) -> Result<Self> {
        let path = data_dir.join(KEY_FILE);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "password notice key must be a regular file"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_create => {
                let mut generated = Zeroizing::new([0_u8; 32]);
                rand::rngs::OsRng.fill_bytes(generated.as_mut());
                let encoded = Zeroizing::new(hex::encode(generated.as_ref()));
                // Read back the winning file even if another caller created it first.
                crate::secure_file::atomic_create_file_mode(&path, encoded.as_bytes(), 0o600)?;
            }
            Err(_) => anyhow::bail!("password notice key unavailable; restore its key file"),
        }
        crate::secure_file::enforce_file_mode(&path, 0o600)?;
        let encoded =
            Zeroizing::new(fs::read_to_string(&path).context("read password notice key")?);
        let decoded =
            Zeroizing::new(hex::decode(encoded.trim()).context("invalid password notice key")?);
        ensure!(decoded.len() == 32, "invalid password notice key length");
        let mut key = Zeroizing::new([0_u8; 32]);
        key.copy_from_slice(&decoded);
        Ok(Self(key))
    }

    pub(super) fn seal(&self, plaintext: &str, event: &str, email: &str) -> Result<String> {
        let mut nonce = [0_u8; 24];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let aad = format!("{SEALED_PREFIX}{event}:{email}");
        let ciphertext = XChaCha20Poly1305::new((&*self.0).into())
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("encrypt password notice failed"))?;
        let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        Ok(format!(
            "{SEALED_PREFIX}{}:{}",
            encoder.encode(nonce),
            encoder.encode(ciphertext)
        ))
    }

    pub(super) fn open(&self, sealed: &str, event: &str, email: &str) -> Result<Zeroizing<String>> {
        let (nonce, ciphertext) = sealed
            .strip_prefix(SEALED_PREFIX)
            .and_then(|value| value.split_once(':'))
            .context("invalid password notice envelope")?;
        let decoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let nonce = decoder
            .decode(nonce)
            .context("invalid password notice nonce")?;
        ensure!(nonce.len() == 24, "invalid password notice nonce length");
        let ciphertext = decoder
            .decode(ciphertext)
            .context("invalid password notice ciphertext")?;
        let aad = format!("{SEALED_PREFIX}{event}:{email}");
        let plaintext = Zeroizing::new(
            XChaCha20Poly1305::new((&*self.0).into())
                .decrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &ciphertext,
                        aad: aad.as_bytes(),
                    },
                )
                .map_err(|_| anyhow::anyhow!("password notice authentication failed"))?,
        );
        String::from_utf8(plaintext.to_vec())
            .map(Zeroizing::new)
            .map_err(|_| anyhow::anyhow!("invalid password notice encoding"))
    }
}
