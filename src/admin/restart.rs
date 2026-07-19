use std::process::{Command, Stdio};

use crate::admin::version::{BINARY_INSTALL_PATH, SERVICE_LOG_PATH, SERVICE_UNIT, ServiceManager};
use crate::error::AppError;

#[derive(Debug, Clone, Copy)]
pub enum RestartStrategy {
    Systemd,
    Nohup,
}

impl RestartStrategy {
    pub fn from_manager(manager: ServiceManager) -> Self {
        match manager {
            ServiceManager::Systemd => RestartStrategy::Systemd,
            _ => RestartStrategy::Nohup,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            RestartStrategy::Systemd => "systemd",
            RestartStrategy::Nohup => "nohup",
        }
    }
}

/// Schedule a restart that runs *after* the HTTP response has gone back.
///
/// Both strategies use a detached child that waits ~1 s, then proceeds:
/// - **systemd**: shell out to `systemctl restart` (which sends SIGTERM and
///   then re-executes the unit). The current process is killed by the
///   service manager.
/// - **nohup**: kill the current PID and re-exec the binary, redirecting
///   stdout/stderr to `/var/log/cc-switch-router.log`. `setsid` detaches
///   from the controlling tty / parent stdio so the new process survives.
///
/// Restart output is appended. Rotation belongs to systemd/logrotate so a
/// restart cannot erase the evidence needed to diagnose a failed recovery.
///
/// Returns the literal shell command (for logging / dry-run tests).
pub fn schedule_restart(strategy: RestartStrategy) -> Result<String, AppError> {
    let script = render_restart_script(strategy);
    spawn_detached(&script)?;
    Ok(script)
}

fn render_restart_script(strategy: RestartStrategy) -> String {
    let pid = std::process::id();
    match strategy {
        RestartStrategy::Systemd => format!(
            "sleep 1; /bin/systemctl restart {unit}",
            unit = SERVICE_UNIT,
        ),
        RestartStrategy::Nohup => format!(
            "sleep 1; \
             mkdir -p $(dirname {log}) 2>/dev/null || true; \
             kill -TERM {pid} 2>/dev/null; \
             for i in $(seq 1 180); do \
                 if ! kill -0 {pid} 2>/dev/null; then break; fi; \
                 sleep 0.25; \
             done; \
             if kill -0 {pid} 2>/dev/null; then kill -KILL {pid} 2>/dev/null || true; fi; \
             nohup {bin} >> {log} 2>&1 &",
            pid = pid,
            log = SERVICE_LOG_PATH,
            bin = BINARY_INSTALL_PATH,
        ),
    }
}

fn spawn_detached(script: &str) -> Result<(), AppError> {
    // setsid detaches from the controlling terminal; closing stdio prevents
    // the child from receiving SIGHUP when this process exits.
    let result = Command::new("setsid")
        .args(["-f", "bash", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match result {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // Some minimal images lack `setsid`; fall back to plain bash
            // backgrounding with disown. Still detaches stdio.
            Command::new("bash")
                .args(["-c", &format!("({script}) </dev/null >/dev/null 2>&1 &")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|err| AppError::Internal(format!("spawn restart child failed: {err}")))?;
            Ok(())
        }
        Err(err) => Err(AppError::Internal(format!(
            "spawn setsid restart child failed: {err}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_script_references_unit() {
        let script = render_restart_script(RestartStrategy::Systemd);
        assert!(script.contains("systemctl restart cc-switch-router.service"));
        assert!(!script.contains(": >"));
    }

    #[test]
    fn nohup_script_kills_and_reexecs() {
        let script = render_restart_script(RestartStrategy::Nohup);
        assert!(script.contains("kill -TERM"));
        assert!(script.contains("/usr/local/bin/cc-switch-router"));
        assert!(script.contains("/var/log/cc-switch-router.log"));
        assert!(script.contains("kill -KILL"));
        assert!(!script.contains(": >"));
    }
}
