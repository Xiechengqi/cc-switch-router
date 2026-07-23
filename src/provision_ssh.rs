use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use tracing::info;

/// Ensure a dedicated outbound provisioning keypair exists and is internally
/// consistent. A missing pair is generated as Ed25519; an existing private key
/// is never replaced.
pub fn require_provision_ssh_keys(private_key_path: &Path, public_key_path: &Path) -> Result<()> {
    if !private_key_path.is_file() {
        if public_key_path.exists() {
            bail!(
                "provisioning SSH private key is missing while its public key exists: {}",
                private_key_path.display()
            );
        }
        if let Some(parent) = private_key_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "create provisioning SSH key directory failed: {}",
                    parent.display()
                )
            })?;
        }
        let output = Command::new("ssh-keygen")
            .arg("-q")
            .arg("-t")
            .arg("ed25519")
            .arg("-N")
            .arg("")
            .arg("-C")
            .arg("cc-switch-router-provision")
            .arg("-f")
            .arg(private_key_path)
            .output()
            .context("start ssh-keygen for provisioning key failed")?;
        if !output.status.success() {
            bail!(
                "generate provisioning SSH key failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        info!(
            provision_ssh_private_key_path = %private_key_path.display(),
            "generated dedicated client market provisioning ssh key"
        );
    }

    let derived_public = derive_public_key(private_key_path)?;
    if public_key_path.is_file() {
        let configured_public = public_key_openssh_from_public_path(public_key_path)?;
        if configured_public != derived_public {
            bail!(
                "provisioning SSH public key does not match private key: {}",
                public_key_path.display()
            );
        }
    } else {
        if let Some(parent) = public_key_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "create provisioning public key directory failed: {}",
                    parent.display()
                )
            })?;
        }
        fs::write(
            public_key_path,
            format!("{derived_public} cc-switch-router-provision\n"),
        )
        .with_context(|| {
            format!(
                "write derived provisioning public key failed: {}",
                public_key_path.display()
            )
        })?;
    }

    info!(
        provision_ssh_private_key_path = %private_key_path.display(),
        provision_ssh_public_key_path = %public_key_path.display(),
        "loaded client market provisioning ssh keys"
    );
    Ok(())
}

fn derive_public_key(private_key_path: &Path) -> Result<String> {
    let output = Command::new("ssh-keygen")
        .arg("-y")
        .arg("-f")
        .arg(private_key_path)
        .output()
        .with_context(|| {
            format!(
                "derive provisioning public key failed: {}",
                private_key_path.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "derive provisioning public key failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    normalize_public_key(&String::from_utf8_lossy(&output.stdout))
}

/// OpenSSH `authorized_keys` line (public key body + a stable operator comment).
pub fn authorized_keys_line_from_public_path(
    public_key_path: &Path,
    comment: &str,
) -> Result<String> {
    let public = public_key_openssh_from_public_path(public_key_path)?;
    Ok(format!("{public} {comment}"))
}

/// Bare public key material (algorithm + base64) without a trailing comment.
pub fn public_key_openssh_from_public_path(public_key_path: &Path) -> Result<String> {
    let raw = fs::read_to_string(public_key_path).with_context(|| {
        format!(
            "read provision public key failed: {}",
            public_key_path.display()
        )
    })?;
    normalize_public_key(&raw)
}

fn normalize_public_key(raw: &str) -> Result<String> {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .ok_or_else(|| anyhow::anyhow!("provision public key is empty"))?;
    let mut parts = line.split_whitespace();
    let algorithm = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid provision public key format"))?;
    let body = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid provision public key format"))?;
    if !algorithm.starts_with("ssh-") && !algorithm.starts_with("ecdsa-") {
        bail!("unsupported provision public key algorithm");
    }
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body)
        .context("invalid provision public key base64")?;
    Ok(format!("{algorithm} {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn missing_pair_is_generated_and_matches() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-router-provision-ssh-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        let private = dir.join("provision");
        let public = dir.join("provision.pub");
        require_provision_ssh_keys(&private, &public).unwrap();
        assert!(private.is_file());
        assert!(public.is_file());
        assert_eq!(
            derive_public_key(&private).unwrap(),
            public_key_openssh_from_public_path(&public).unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_mismatched_public_key() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-router-provision-mismatch-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first");
        let first_public = dir.join("first.pub");
        let second = dir.join("second");
        let second_public = dir.join("second.pub");
        require_provision_ssh_keys(&first, &first_public).unwrap();
        require_provision_ssh_keys(&second, &second_public).unwrap();
        fs::copy(&second_public, &first_public).unwrap();
        let error = require_provision_ssh_keys(&first, &first_public).unwrap_err();
        assert!(error.to_string().contains("does not match"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_public_key_stripping_comment() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-router-provision-pub-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        let public = dir.join("id_ed25519.pub");
        let mut file = fs::File::create(&public).unwrap();
        writeln!(
            file,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGjWI8jfRRbxMZjdFDfgRlaHpRZPf7qs4odSbL41WQ1m user@host"
        )
        .unwrap();
        assert_eq!(
            public_key_openssh_from_public_path(&public).unwrap(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGjWI8jfRRbxMZjdFDfgRlaHpRZPf7qs4odSbL41WQ1m"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
