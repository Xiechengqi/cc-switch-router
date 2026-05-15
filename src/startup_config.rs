use std::collections::BTreeMap;
use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

use anyhow::{Context, Result};

use crate::admin::settings::{read_env_file, write_env_file_atomic};
use crate::config::tunnel_domain_host;

const REQUIRED_STARTUP_KEYS: &[StartupField] = &[
    StartupField {
        key: "CC_SWITCH_ROUTER_TUNNEL_DOMAIN",
        label: "Public tunnel domain",
        prompt: "Public tunnel domain, e.g. router.example.com",
        secret: false,
    },
    StartupField {
        key: "CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR",
        label: "Public SSH address",
        prompt: "Public SSH address sent to clients, e.g. router.example.com:2222",
        secret: false,
    },
    StartupField {
        key: "CC_SWITCH_ROUTER_RESEND_API_KEY",
        label: "Resend API key",
        prompt: "Resend API key, e.g. re_xxx",
        secret: true,
    },
];

struct StartupField {
    key: &'static str,
    label: &'static str,
    prompt: &'static str,
    secret: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum StartupConfigMode {
    Start,
    SetupOnly,
    CheckOnly,
}

pub fn ensure_startup_config(env_path: &Path, mode: StartupConfigMode) -> Result<()> {
    let mut env_file = read_env_file(env_path).map_err(anyhow::Error::msg)?;
    let needs_input = fields_requiring_input(&env_file);
    if needs_input.is_empty() {
        if matches!(
            mode,
            StartupConfigMode::CheckOnly | StartupConfigMode::SetupOnly
        ) {
            println!("startup config OK: {}", env_path.display());
        }
        return Ok(());
    }

    if matches!(mode, StartupConfigMode::CheckOnly) {
        anyhow::bail!("{}", missing_config_message(&needs_input, env_path));
    }

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("{}", missing_config_message(&needs_input, env_path));
    }

    println!("cc-switch-router setup required\n");
    println!("Missing or invalid:");
    for field in &needs_input {
        println!("- {} ({})", field.key, field.label);
    }
    println!();

    for field in needs_input {
        let value = prompt_required(field)?;
        env_file.insert(field.key.to_string(), value.clone());
        unsafe {
            env::set_var(field.key, value);
        }
    }

    let sorted = env_file.into_iter().collect::<BTreeMap<String, String>>();
    write_env_file_atomic(env_path, &sorted).map_err(anyhow::Error::msg)?;
    println!("Saved startup config to {}", env_path.display());
    if matches!(mode, StartupConfigMode::SetupOnly) {
        println!("Setup complete. Run cc-switch-router to start the service.");
    }
    Ok(())
}

pub fn default_resend_from(tunnel_domain: &str) -> Option<String> {
    tunnel_domain_host(tunnel_domain).map(|host| format!("noreply@{host}"))
}

fn fields_requiring_input(
    env_file: &std::collections::HashMap<String, String>,
) -> Vec<&'static StartupField> {
    REQUIRED_STARTUP_KEYS
        .iter()
        .filter(|field| {
            let Some(value) = env::var(field.key)
                .ok()
                .or_else(|| env_file.get(field.key).cloned())
            else {
                return true;
            };
            let value = value.trim();
            value.is_empty() || validate_startup_value(field.key, value).is_err()
        })
        .collect()
}

fn missing_config_message(missing: &[&StartupField], env_path: &Path) -> String {
    let keys = missing
        .iter()
        .map(|field| field.key)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "missing required startup config: {keys}; run `cc-switch-router setup` or edit {}",
        env_path.display()
    )
}

fn prompt_required(field: &StartupField) -> Result<String> {
    loop {
        let raw = if field.secret {
            rpassword::prompt_password(format!("{}: ", field.prompt))
                .with_context(|| format!("read {} failed", field.key))?
        } else {
            print!("{}: ", field.prompt);
            io::stdout().flush().ok();
            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .with_context(|| format!("read {} failed", field.key))?;
            input
        };
        let value = raw.trim().to_string();
        if value.is_empty() {
            println!("{} cannot be empty.", field.key);
            continue;
        }
        if let Err(message) = validate_startup_value(field.key, &value) {
            println!("{message}");
            continue;
        }
        return Ok(value);
    }
}

fn validate_startup_value(key: &str, value: &str) -> std::result::Result<(), String> {
    match key {
        "CC_SWITCH_ROUTER_TUNNEL_DOMAIN" => {
            let host = tunnel_domain_host(value)
                .ok_or_else(|| format!("{key} must contain a public host name"))?;
            if host == "0.0.0.0" || host == "127.0.0.1" || host == "::1" {
                return Err(format!("{key} must be a public host, got {host}"));
            }
            Ok(())
        }
        "CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR" => {
            if !value.contains(':') {
                return Err(format!(
                    "{key} must include a host and port, e.g. router.example.com:2222"
                ));
            }
            Ok(())
        }
        "CC_SWITCH_ROUTER_RESEND_API_KEY" => {
            if !value.starts_with("re_") {
                return Err(format!("{key} should start with re_"));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_resend_from_strips_tunnel_port() {
        assert_eq!(
            default_resend_from("router.example.com:8787").as_deref(),
            Some("noreply@router.example.com")
        );
    }

    #[test]
    fn default_resend_from_rejects_empty_domain() {
        assert_eq!(default_resend_from("  "), None);
    }
}
