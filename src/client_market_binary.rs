//! Prepares credential-free binaries independently of a tenant's installation.
use super::*;
use crate::client_server_release::InstallationRelease;

static PREPARATION_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PreparationView {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_id: Option<String>,
    updated_at: String,
}

pub(super) async fn views(
    state: &ServerState,
) -> Result<HashMap<String, PreparationView>, AppError> {
    let conn = state.store.conn.lock().await;
    let mut statement = conn
        .prepare("SELECT host_id,status,commit_id,updated_at FROM client_binary_preparations")
        .map_err(|e| AppError::Internal(e.to_string()))?;
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                PreparationView {
                    status: row.get(1)?,
                    commit_id: row.get(2)?,
                    updated_at: row.get(3)?,
                },
            ))
        })
        .map_err(|e| AppError::Internal(e.to_string()))?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|e| AppError::Internal(e.to_string()))
}

pub(super) async fn queue(state: &ServerState, host_id: &str) {
    let queued = state.store.conn.lock().await.execute(
        "INSERT INTO client_binary_preparations(host_id,status,updated_at) VALUES(?1,'pending',?2)
         ON CONFLICT(host_id) DO UPDATE SET status='pending',attempts=0,last_error=NULL,updated_at=excluded.updated_at",
        params![host_id, Utc::now().to_rfc3339()],
    );
    if let Err(error) = queued {
        warn!(host_id, error = %error, "Client binary queue update failed; persisted admission queue will retry");
        return;
    }
    let state = state.clone();
    let host_id = host_id.to_string();
    tokio::spawn(async move {
        prepare_queued(&state, &host_id).await;
    });
}

pub(super) async fn reconcile(state: &ServerState) -> Result<(), AppError> {
    let ids = {
        let conn = state.store.conn.lock().await;
        conn.execute(
            "UPDATE client_binary_preparations SET status='failed',last_error='Preparation interrupted; allocation will retry',updated_at=?1
             WHERE status='running' AND attempts>=3 AND julianday(updated_at)<julianday(?1)-10.0/1440",
            params![Utc::now().to_rfc3339()],
        ).map_err(|error| AppError::Internal(format!("finalize interrupted binary preparation: {error}")))?;
        let mut query = conn.prepare(
            "SELECT p.host_id FROM client_binary_preparations p JOIN router_ssh_hosts h ON h.id=p.host_id
             WHERE h.status='idle' AND h.installation_id IS NULL AND p.attempts < 3
               AND (p.status='pending' OR (p.status IN ('failed','running') AND julianday(p.updated_at)<julianday(?1)-10.0/1440))
             ORDER BY p.updated_at LIMIT 4"
        ).map_err(|e| AppError::Internal(format!("read binary preparation queue: {e}")))?;
        query
            .query_map(params![Utc::now().to_rfc3339()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| AppError::Internal(format!("read binary preparation queue: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Internal(e.to_string()))?
    };
    for id in ids {
        let state = state.clone();
        tokio::spawn(async move {
            prepare_queued(&state, &id).await;
        });
    }
    Ok(())
}

async fn prepare_queued(state: &ServerState, host_id: &str) {
    let Ok(_slot) = PREPARATION_SLOTS.try_acquire() else {
        return;
    };
    let _host_lock = state
        .client_market_actions
        .lock(&format!("binary:{host_id}"))
        .await;
    let result = async {
        let Some(host) = state.store.client_market_get_host(host_id).await? else { return Ok(()) };
        if host.status != HOST_STATUS_IDLE || host.installation_id.is_some() { return Ok(()) }
        let claimed = state.store.conn.lock().await.execute(
            "UPDATE client_binary_preparations SET status='running',attempts=attempts+1,updated_at=?2
             WHERE host_id=?1 AND attempts<3 AND (status='pending' OR (status IN ('running','failed') AND julianday(updated_at)<julianday(?2)-10.0/1440))",
            params![host_id, Utc::now().to_rfc3339()],
        ).map_err(|e| AppError::Internal(e.to_string()))?;
        if claimed == 0 { return Ok(()) }
        require_pinned_host_fingerprint(&host, &known_hosts_path(&state.config)).await?;
        let selector = state.dynamic.read().await.client_server_release.clone();
        let release = state.client_server_release_validator.resolve_installation(&selector).await.map_err(AppError::ServiceUnavailable)?;
        ssh_run_remote_with_input(&state.provision_ssh_key_path, &known_hosts_path(&state.config), &host.ip, host.port,
            &prepare_command(&release, false), None, SSH_INSTALL_TIMEOUT, SshHostKeyPolicy::RequireKnown).await?;
        state.store.conn.lock().await.execute(
            "UPDATE client_binary_preparations SET status='ready',selector=?2,commit_id=?3,last_error=NULL,updated_at=?4 WHERE host_id=?1",
            params![host_id, release.selector, release.commit, Utc::now().to_rfc3339()],
        ).map_err(|e| AppError::Internal(e.to_string()))?;
        Ok::<(), AppError>(())
    }.await;
    if let Err(error) = result {
        warn!(host_id, error = %error, "Client binary preparation failed; rental will retry the download");
        let _ = state.store.conn.lock().await.execute(
            "UPDATE client_binary_preparations SET status='failed',last_error=?2,updated_at=?3 WHERE host_id=?1",
            params![host_id, "Binary preparation failed; retry scheduled", Utc::now().to_rfc3339()],
        );
    }
}

/// The checksum comes from Router's fresh Release lookup, never a Host manifest.
/// Downloads are staged on the cache filesystem and shared with allocation.
fn prepare_command(release: &InstallationRelease, run_version: bool) -> String {
    let amd64 = &release.artifacts[0];
    let arm64 = &release.artifacts[1];
    format!(
        r#"{deps}
set -eu
ensure_client_market_deps
case "$(uname -m)" in
  x86_64|amd64) asset={amd64_name}; expected={amd64_sha}; download={amd64_url} ;;
  aarch64|arm64) asset={arm64_name}; expected={arm64_sha}; download={arm64_url} ;;
  *) echo 'unsupported Client architecture' >&2; exit 1 ;;
esac
cache=/var/cache/cc-switch-router/server
installed_binary=/usr/local/bin/cc-switch-server
mkdir -p "$cache"
chmod 700 "$cache"
exec 9>"$cache/download.lock"
flock -w 600 9 || exit 1
prepared_binary="$cache/{commit}-$asset-$expected"
if [ ! -f "$prepared_binary" ] || [ -L "$prepared_binary" ] || [ "$(sha256sum "$prepared_binary" | cut -d ' ' -f1)" != "$expected" ]; then
  staged=$(mktemp "$cache/.download.XXXXXX")
  trap 'rm -f "$staged"' EXIT
  if [ -f "$installed_binary" ] && [ ! -L "$installed_binary" ] && [ "$(sha256sum "$installed_binary" | cut -d ' ' -f1)" = "$expected" ]; then
    cp "$installed_binary" "$staged"
  else
    curl --fail --silent --show-error --location --connect-timeout 10 --max-time 240 "$download" -o "$staged" || {{
      curl --fail --silent --show-error --location --connect-timeout 10 --max-time 240 "https://gh-proxy.org/$download" -o "$staged" || exit 1
    }}
  fi
  [ "$(sha256sum "$staged" | cut -d ' ' -f1)" = "$expected" ] || {{ echo 'binary checksum mismatch' >&2; exit 1; }}
  chmod 755 "$staged"
  mv -f "$staged" "$prepared_binary"
  trap - EXIT
fi
touch "$prepared_binary"
previous_binary=""
for candidate in "$cache"/*-cc-switch-server-linux-*; do
  [ -f "$candidate" ] && [ ! -L "$candidate" ] && [ "$candidate" != "$prepared_binary" ] || continue
  if [ -z "$previous_binary" ] || [ "$candidate" -nt "$previous_binary" ]; then previous_binary="$candidate"; fi
done
for candidate in "$cache"/*-cc-switch-server-linux-*; do
  [ -f "$candidate" ] && [ ! -L "$candidate" ] || continue
  [ "$candidate" = "$prepared_binary" ] || [ "$candidate" = "$previous_binary" ] || rm -f -- "$candidate"
done
{version_check}
flock -u 9
export CC_SWITCH_SERVER_PREPARED_BINARY="$prepared_binary"
export CC_SWITCH_SERVER_EXPECTED_COMMIT={commit}
export CC_SWITCH_SERVER_EXPECTED_SHA256="$expected"
"#,
        deps = REMOTE_ENSURE_CLIENT_MARKET_DEPS,
        amd64_name = shell_quote(&amd64.name),
        amd64_sha = shell_quote(&amd64.sha256),
        amd64_url = shell_quote(&amd64.url),
        arm64_name = shell_quote(&arm64.name),
        arm64_sha = shell_quote(&arm64.sha256),
        arm64_url = shell_quote(&arm64.url),
        commit = release.commit,
        version_check = if run_version {
            "\"$prepared_binary\" -V >/dev/null"
        } else {
            ""
        },
    )
}

pub(super) async fn install(
    state: &ServerState,
    host: &RouterSshHostRecord,
    owner: &str,
    subdomain: &str,
    password: Vec<u8>,
) -> Result<String, AppError> {
    let _host_lock = state
        .client_market_actions
        .lock(&format!("binary:{}", host.id))
        .await;
    let mut ready_release = None;
    for _ in 0..3 {
        let selector = state.dynamic.read().await.client_server_release.clone();
        let release = state
            .client_server_release_validator
            .resolve_installation(&selector)
            .await
            .map_err(AppError::ServiceUnavailable)?;
        ssh_run_remote_with_input(
            &state.provision_ssh_key_path,
            &known_hosts_path(&state.config),
            &host.ip,
            host.port,
            &prepare_command(&release, false),
            None,
            SSH_INSTALL_TIMEOUT,
            SshHostKeyPolicy::RequireKnown,
        )
        .await?;
        let current_selector = state.dynamic.read().await.client_server_release.clone();
        let current = state
            .client_server_release_validator
            .resolve_installation(&current_selector)
            .await
            .map_err(AppError::ServiceUnavailable)?;
        if current.selector == release.selector
            && current.commit == release.commit
            && current
                .artifacts
                .iter()
                .zip(&release.artifacts)
                .all(|(a, b)| a.sha256 == b.sha256)
            && state.dynamic.read().await.client_server_release == current.selector
        {
            ready_release = Some(current);
            break;
        }
    }
    let release = ready_release.ok_or_else(|| {
        AppError::ServiceUnavailable("Client release changed repeatedly; retry allocation".into())
    })?;
    state.store.conn.lock().await.execute(
        "UPDATE client_binary_preparations SET status='ready',selector=?2,commit_id=?3,last_error=NULL,updated_at=?4 WHERE host_id=?1",
        params![host.id, release.selector, release.commit, Utc::now().to_rfc3339()],
    ).map_err(|error| AppError::Internal(format!("save prepared Client release: {error}")))?;
    let command = install_command(
        &release,
        &router_public_url(&state.config),
        owner,
        subdomain,
    )?;
    ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts_path(&state.config),
        &host.ip,
        host.port,
        &command,
        Some(password),
        SSH_INSTALL_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await
}

fn install_command(
    release: &InstallationRelease,
    router_url: &str,
    owner: &str,
    subdomain: &str,
) -> Result<String, AppError> {
    let script = crate::api::render_install_client_script(&release.selector)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(script);
    Ok(format!(
        "{}\nscript=$(mktemp); trap 'rm -f \"$script\"' EXIT\nprintf '%s' {} | base64 -d >\"$script\"\nbash \"$script\" {} {} --password-stdin {}",
        prepare_command(release, true),
        shell_quote(&encoded),
        shell_quote(router_url),
        shell_quote(owner),
        shell_quote(subdomain)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn prepared_installer_rejects_wrong_commit_before_install_and_accepts_verified_binary() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};
        let directory = std::env::temp_dir().join(format!("cc-client-install-{}", Uuid::new_v4()));
        let bin = directory.join("bin");
        let home = directory.join("tenant");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir(&home).unwrap();
        for name in ["pgrep", "pidof"] {
            let path = bin.join(name);
            std::fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let source = directory.join("prepared-server");
        let target = if std::env::consts::ARCH == "aarch64" {
            "aarch64-unknown-linux-musl"
        } else {
            "x86_64-unknown-linux-musl"
        };
        let fixture = |commit: &str| {
            format!(
                r#"#!/bin/sh
case "$1" in
  -V) echo 'fixture Server' ;;
  version) cat <<'JSON'
{{
  "name": "cc-switch-server",
  "target": "{target}",
  "commitId": "{commit}"
}}
JSON
;;
  init) read -r password; mkdir -p "$TASK_CLIENT_HOME/.cc-switch-server"; printf '%s\n' '{{"tunnelSubdomain":"client-alpha"}}' > "$TASK_CLIENT_HOME/.cc-switch-server/server.json" ;;
esac
"#
            )
        };
        let script = crate::api::render_install_client_script("latest").unwrap()
            .replace("[ -r /etc/profile ] && . /etc/profile || true", ":")
            .replace("/usr/local/bin", &bin.to_string_lossy())
            .replace("${HOME}", "${TASK_CLIENT_HOME}").replace("$HOME", "$TASK_CLIENT_HOME")
            .replace("main \"$@\"", "start_cc_switch_server() { SERVICE_MANAGER=test; touch \"$TASK_CLIENT_HOME/started\"; }\nmain \"$@\"");
        let script_path = directory.join("installer.sh");
        std::fs::write(&script_path, script).unwrap();
        let run = |content: &str| {
            std::fs::write(&source, content).unwrap();
            let digest = hex::encode(Sha256::digest(content.as_bytes()));
            let mut child = Command::new("bash")
                .arg(&script_path)
                .args([
                    "http://router.test",
                    "owner@example.test",
                    "--password-stdin",
                    "client-alpha",
                ])
                .env("TASK_CLIENT_HOME", &home)
                .env("CC_SWITCH_SERVER_PREPARED_BINARY", &source)
                .env("CC_SWITCH_SERVER_EXPECTED_COMMIT", "a".repeat(40))
                .env("CC_SWITCH_SERVER_EXPECTED_SHA256", digest)
                .env(
                    "PATH",
                    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"private-test-password\n")
                .unwrap();
            child.wait_with_output().unwrap()
        };
        let wrong = run(&fixture(&"b".repeat(40)));
        assert!(!wrong.status.success());
        assert!(!bin.join("cc-switch-server").exists());
        assert!(!home.join("started").exists());
        let valid = run(&fixture(&"a".repeat(40)));
        assert!(
            valid.status.success(),
            "{} {}",
            String::from_utf8_lossy(&valid.stdout),
            String::from_utf8_lossy(&valid.stderr)
        );
        assert!(bin.join("cc-switch-server").exists());
        assert!(home.join("started").exists());
        assert!(!String::from_utf8_lossy(&valid.stdout).contains("private-test-password"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn binary_queue_survives_database_reopen_and_follows_host_deletion() {
        let directory = std::env::temp_dir().join(format!("cc-client-queue-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("business.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            crate::schema::apply(&conn).unwrap();
            conn.execute("INSERT INTO router_ssh_hosts(id,ip,port,host_owner_email,status,created_at,updated_at) VALUES('host','203.0.113.1',22,'provider@example.test','idle',?1,?1)", params![Utc::now().to_rfc3339()]).unwrap();
        }
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT status FROM client_binary_preparations WHERE host_id='host'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
                "pending"
            );
            conn.execute("DELETE FROM router_ssh_hosts WHERE id='host'", [])
                .unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM client_binary_preparations",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn installation_keeps_password_on_stdin_and_web_terminal_enabled() {
        let release = InstallationRelease {
            selector: "latest".into(),
            commit: "a".repeat(40),
            artifacts: ["amd64", "arm64"]
                .into_iter()
                .map(|arch| crate::client_server_release::InstallationArtifact {
                    name: format!("cc-switch-server-linux-{arch}"),
                    url: "https://example.test/binary".into(),
                    sha256: "b".repeat(64),
                })
                .collect(),
        };
        let command = install_command(
            &release,
            "https://router.test",
            "owner@example.test",
            "client-alpha",
        )
        .unwrap();
        assert!(command.contains("--password-stdin"));
        assert!(!command.contains("disableWebTerminal"));
        assert!(command.contains("CC_SWITCH_SERVER_EXPECTED_COMMIT"));
    }

    #[tokio::test]
    async fn predownload_reuses_verified_cache_repairs_corruption_and_never_runs_binary() {
        let directory = std::env::temp_dir().join(format!("cc-client-binary-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let marker = directory.join("binary-was-executed");
        let binary = format!(
            "#!/bin/sh\ntouch {}\n",
            shell_quote(&marker.to_string_lossy())
        );
        let digest = hex::encode(Sha256::digest(binary.as_bytes()));
        let requests = Arc::new(AtomicUsize::new(0));
        let hits = requests.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/binary",
                    get(move || {
                        hits.fetch_add(1, Ordering::SeqCst);
                        let binary = binary.clone();
                        async move { binary }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let release = InstallationRelease {
            selector: "latest".into(),
            commit: "a".repeat(40),
            artifacts: ["amd64", "arm64"]
                .into_iter()
                .map(|arch| crate::client_server_release::InstallationArtifact {
                    name: format!("cc-switch-server-linux-{arch}"),
                    url: format!("http://{addr}/binary"),
                    sha256: digest.clone(),
                })
                .collect(),
        };
        let installed = directory.join("installed-server");
        let command = prepare_command(&release, false)
            .replace(
                REMOTE_ENSURE_CLIENT_MARKET_DEPS,
                "ensure_client_market_deps() { :; }\n",
            )
            .replace(
                "cache=/var/cache/cc-switch-router/server",
                &format!("cache={}", shell_quote(&directory.to_string_lossy())),
            )
            .replace(
                "installed_binary=/usr/local/bin/cc-switch-server",
                &format!(
                    "installed_binary={}",
                    shell_quote(&installed.to_string_lossy())
                ),
            );
        let run = || async {
            tokio::process::Command::new("bash")
                .arg("-c")
                .arg(&command)
                .output()
                .await
                .unwrap()
        };
        let first = run().await;
        assert!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        let second = run().await;
        assert!(second.status.success());
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert!(
            !marker.exists(),
            "pre-download executed the downloaded program"
        );
        let cached = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&"a".repeat(40))
            })
            .unwrap();
        std::fs::write(&cached, "corrupted").unwrap();
        let (a, b) = tokio::join!(run(), run());
        assert!(a.status.success() && b.status.success());
        assert_eq!(
            requests.load(Ordering::SeqCst),
            2,
            "concurrent repairs downloaded twice"
        );
        assert!(!marker.exists());
        std::fs::copy(&cached, &installed).unwrap();
        std::fs::remove_file(&cached).unwrap();
        assert!(run().await.status.success());
        assert_eq!(
            requests.load(Ordering::SeqCst),
            2,
            "verified installed binary was downloaded again"
        );
        assert!(!marker.exists());
        let rejecting = command.replace(&digest, &"f".repeat(64));
        let rejected = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(rejecting)
            .output()
            .await
            .unwrap();
        assert!(!rejected.status.success());
        assert!(!marker.exists());
        server.abort();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
