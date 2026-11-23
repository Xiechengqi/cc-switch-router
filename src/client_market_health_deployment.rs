use super::*;

pub(crate) async fn uninstall(
    state: &ServerState,
    host: &RouterSshHostRecord,
    sequence: i64,
) -> Result<(), AppError> {
    require_pinned_host_fingerprint(host, &known_hosts_path(&state.config)).await?;
    let command = format!(
        "{}\n{}\nmkdir -p \"$control/retired\"\ntouch \"$control/retired/$expected_host\"\n",
        remote_fence(&host.id, sequence),
        REMOVE
    );
    ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts_path(&state.config),
        &host.ip,
        host.port,
        &command,
        None,
        SSH_INSTALL_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await?;
    Ok(())
}

pub(super) const REMOVE: &str = r#"
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  systemctl disable --now cc-switch-host-standby.service 2>/dev/null || true
fi
if command -v rc-service >/dev/null 2>&1; then rc-service cc-switch-host-standby stop 2>/dev/null || true; fi
if command -v rc-update >/dev/null 2>&1; then rc-update del cc-switch-host-standby default 2>/dev/null || true; fi
if command -v supervisorctl >/dev/null 2>&1 && supervisorctl pid >/dev/null 2>&1; then
  supervisorctl stop cc-switch-host-standby >/dev/null 2>&1 || true
  supervisorctl remove cc-switch-host-standby >/dev/null 2>&1 || true
fi
rm -f /etc/supervisor/conf.d/cc-switch-host-standby.conf
state=/var/lib/cc-switch-host-standby
binary=/usr/local/bin/cc-switch-host-standby
if [ -f "$state/standby.pid" ]; then
  pid=$(cat "$state/standby.pid")
  case "$pid" in ''|*[!0-9]*) exit 1 ;; esac
  exe=$(readlink "/proc/$pid/exe" 2>/dev/null || true)
  # A reused PID after reboot or container restart is stale, not a foreign owner.
  if [ "$exe" = "$binary" ] || [ "$exe" = "$binary (deleted)" ]; then
    kill "$pid"
    count=0
    while kill -0 "$pid" 2>/dev/null; do count=$((count+1)); [ "$count" -lt 30 ] || exit 1; sleep 1; done
  fi
  rm -f "$state/standby.pid"
fi
# The sender lock proves that no managed process can still use the old state.
if [ -f "$state/sender.lock" ]; then exec 7>"$state/sender.lock"; flock -n 7 || exit 1; fi
rm -f /etc/cc-switch-host-standby/config.json /etc/systemd/system/cc-switch-host-standby.service /etc/init.d/cc-switch-host-standby /usr/local/libexec/cc-switch-host-standby-run
rm -f "$state"/key-* "$state"/send-state.json "$state"/standby.pid "$state"/standby.log
rm -f "$binary"
"#;

const FIRST_DUE_MIN_SECS: i64 = 60;
const FIRST_DUE_MAX_SECS: i64 = 300;

pub(crate) async fn claim_preparation(
    store: &AppStore,
    host: &str,
) -> Result<Option<i64>, AppError> {
    let conn = store.conn.lock().await;
    let tx = conn.transaction().map_err(database)?;
    let now = Utc::now().to_rfc3339();
    let status:Option<String>=tx.query_row(
        "SELECT h.status FROM router_ssh_hosts h JOIN client_host_agents a ON a.host_id=h.id JOIN client_binary_preparations p ON p.host_id=h.id
         WHERE h.id=?1 AND h.installation_id IS NULL AND (a.revoked=0 OR a.maintenance_pending=1) AND p.attempts<3
         AND (h.status='onboarding' OR (h.status IN ('idle','maintaining') AND a.maintenance_pending=1))
         AND (p.status='pending' OR (p.status='ready' AND a.maintenance_pending=1)
              OR (p.status IN ('running','failed') AND julianday(p.updated_at)<julianday(?2)-15.0/1440))",
        params![host,now],|row|row.get(0)).optional().map_err(database)?;
    let Some(status) = status else {
        return Ok(None);
    };
    if status == "idle" {
        if tx.execute("UPDATE router_ssh_hosts SET status='maintaining',updated_at=?2 WHERE id=?1 AND status='idle' AND installation_id IS NULL",
            params![host,now]).map_err(database)?!=1 {return Ok(None)}
    }
    tx.execute("UPDATE client_binary_preparations SET status='running',attempts=attempts+1,updated_at=?2 WHERE host_id=?1",params![host,now]).map_err(database)?;
    tx.execute(
        "UPDATE client_host_agents SET operation_sequence=operation_sequence+1 WHERE host_id=?1",
        params![host],
    )
    .map_err(database)?;
    let sequence = tx
        .query_row(
            "SELECT operation_sequence FROM client_host_agents WHERE host_id=?1",
            params![host],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database)?;
    tx.commit().map_err(database)?;
    Ok(Some(sequence))
}

pub(crate) fn remote_fence(host: &str, sequence: i64) -> String {
    remote_fence_taking_over(host, sequence, &[])
}

/// `takeover` lists deleted Host IDs that were registered at the same address. A
/// re-added machine may still carry one of them in its control directory when the
/// uninstall at deletion never reached it; only those IDs may be retired remotely.
fn remote_fence_taking_over(host: &str, sequence: i64, takeover: &[String]) -> String {
    let takeover = takeover
        .iter()
        .map(|id| shell_quote(id))
        .collect::<Vec<_>>()
        .join(" ");
    REMOTE_FENCE
        .replace("@HOST@", &shell_quote(host))
        .replace("@SEQ@", &sequence.to_string())
        .replace("@TAKEOVER@", &takeover)
}

/// Each enrollment step may take up to `SSH_INSTALL_TIMEOUT`; refreshing the claim
/// between steps keeps the stale-claim recovery from returning a Host to the pool
/// while its remote work is still running.
async fn refresh_claim(store: &AppStore, host: &str, sequence: i64) -> Result<(), AppError> {
    store
        .conn
        .lock()
        .await
        .execute(
            "UPDATE client_binary_preparations SET updated_at=?3 WHERE host_id=?1 AND status='running'
             AND EXISTS(SELECT 1 FROM client_host_agents WHERE host_id=?1 AND operation_sequence=?2)",
            params![host, sequence, Utc::now().to_rfc3339()],
        )
        .map_err(database)?;
    Ok(())
}

async fn retired_ids_at_address(
    store: &AppStore,
    host: &RouterSshHostRecord,
) -> Result<Vec<String>, AppError> {
    let conn = store.conn.lock().await;
    let mut query = conn
        .prepare(
            "SELECT host_id FROM client_host_retired_ids WHERE ip=?1 AND port=?2 AND host_id<>?3
             AND host_id NOT IN (SELECT id FROM router_ssh_hosts) ORDER BY retired_at DESC LIMIT 16",
        )
        .map_err(database)?;
    query
        .query_map(params![host.ip, host.port, host.id], |row| row.get(0))
        .map_err(database)?
        .collect::<Result<Vec<String>, _>>()
        .map_err(database)
}

const REMOTE_FENCE: &str = r#"
set -eu
control=/var/lib/cc-switch-host-control
mkdir -p "$control"
chmod 700 "$control"
exec 8>"$control/action.lock"
flock -w 600 8
expected_host=@HOST@
expected_seq=@SEQ@
[ ! -f "$control/retired/$expected_host" ] || { echo 'retired Host operation' >&2; exit 1; }
if [ -f "$control/host" ]; then
  previous_host=$(cat "$control/host")
  if [ "$previous_host" != "$expected_host" ]; then
    for retired_host in @TAKEOVER@ ''; do
      if [ -n "$retired_host" ] && [ "$retired_host" = "$previous_host" ]; then
        mkdir -p "$control/retired"
        touch "$control/retired/$previous_host"
      fi
    done
    [ -f "$control/retired/$previous_host" ] || { echo 'managed Host identity mismatch' >&2; exit 1; }
    rm -f "$control/sequence"
  fi
fi
previous_seq=$(cat "$control/sequence" 2>/dev/null || echo 0)
[ "$previous_seq" -le "$expected_seq" ] || { echo 'stale Host operation' >&2; exit 1; }
printf '%s' "$expected_host" > "$control/host.next"
mv -f "$control/host.next" "$control/host"
printf '%s' "$expected_seq" > "$control/sequence.next"
mv -f "$control/sequence.next" "$control/sequence"
"#;

pub(crate) async fn install_operation(
    state: &ServerState,
    host: &str,
    job_id: &str,
) -> Result<i64, AppError> {
    let conn = state.store.conn.lock().await;
    let changed=conn.execute(
        "UPDATE client_host_agents SET operation_sequence=operation_sequence+1 WHERE host_id=?1
         AND EXISTS(SELECT 1 FROM router_ssh_hosts h JOIN provisioning_jobs j ON j.host_id=h.id WHERE h.id=?1 AND h.status='locked' AND j.id=?2 AND j.type='create' AND j.status='running')",
        params![host,job_id]).map_err(database)?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "Host is no longer assigned to this installation".into(),
        ));
    }
    conn.query_row(
        "SELECT operation_sequence FROM client_host_agents WHERE host_id=?1",
        params![host],
        |row| row.get(0),
    )
    .map_err(database)
}

pub(crate) async fn enroll(
    state: &ServerState,
    host: &RouterSshHostRecord,
    release: &crate::client_server_release::InstallationRelease,
    sequence: i64,
) -> Result<(), AppError> {
    let takeover = retired_ids_at_address(&state.store, host).await?;
    // Download before choosing the first slot or anchoring the managed clock.
    ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts_path(&state.config),
        &host.ip,
        host.port,
        &binary::prepare_command(release, false),
        None,
        SSH_INSTALL_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await?;
    let replacement: bool = state
        .store
        .conn
        .lock()
        .await
        .query_row(
            "SELECT replacement_pending FROM client_host_agents WHERE host_id=?1",
            params![host.id],
            |row| row.get(0),
        )
        .map_err(database)?;
    refresh_claim(&state.store, &host.id, sequence).await?;
    if replacement {
        let command = format!(
            "{}\n{}",
            remote_fence_taking_over(&host.id, sequence, &takeover),
            REMOVE
        );
        ssh_run_remote_with_input(
            &state.provision_ssh_key_path,
            &known_hosts_path(&state.config),
            &host.ip,
            host.port,
            &command,
            None,
            SSH_INSTALL_TIMEOUT,
            SshHostKeyPolicy::RequireKnown,
        )
        .await?;
        let conn = state.store.conn.lock().await;
        let tx = conn.transaction().map_err(database)?;
        let next_hour = Utc::now().timestamp() / 3600 + 1;
        let changed=tx.execute("UPDATE client_host_agents SET machine_epoch=machine_epoch+1,key_generation=1,public_key=NULL,admitted_hour=NULL,legacy=0,readmission_pending=0,first_due_at=?3,latest_hour=NULL,latest_sample_json=NULL,latest_received_at=NULL,latest_public=0,latest_lease_id=NULL,summary_json=NULL,revoked=0,replacement_pending=0,last_error=NULL,next_deadline=0 WHERE host_id=?1 AND operation_sequence=?2 AND replacement_pending=1",
            params![host.id,sequence,wire::due_at(&host.id,next_hour)]).map_err(database)?;
        if changed != 1 {
            return Err(AppError::Conflict("Host replacement superseded".into()));
        }
        tx.execute("UPDATE router_ssh_hosts SET status='onboarding',updated_at=?2 WHERE id=?1 AND status='maintaining' AND installation_id IS NULL",params![host.id,Utc::now().to_rfc3339()]).map_err(database)?;
        tx.commit().map_err(database)?;
    }
    let now = Utc::now().timestamp();
    let (epoch, generation, first, reason): (i64, i64, Option<i64>, Option<String>) = {
        let conn = state.store.conn.lock().await;
        conn.query_row("SELECT machine_epoch,key_generation,first_due_at,last_error FROM client_host_agents WHERE host_id=?1 AND operation_sequence=?2",
            params![host.id,sequence],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(database)?
    };
    // The standby accepts a late start for this slot (see Server `FIRST_SEND_WINDOW_MS`),
    // so the lead only has to cover the remaining STAGE and START round trips.
    let first = first.unwrap_or_else(|| {
        now + rand::thread_rng().gen_range(FIRST_DUE_MIN_SECS..=FIRST_DUE_MAX_SECS)
    });
    let generation = if matches!(
        reason.as_deref(),
        Some("hour_conflict" | "rotation_requested")
    ) {
        generation + 1
    } else {
        generation
    };
    {
        let conn = state.store.conn.lock().await;
        conn.execute("UPDATE client_host_agents SET first_due_at=COALESCE(first_due_at,?3),next_deadline=0 WHERE host_id=?1 AND operation_sequence=?2",
            params![host.id,sequence,first]).map_err(database)?;
        refresh_summary(&conn, &host.id, now)?;
    }
    let config = json!({"hostId":host.id,"routerUrl":router_public_url(&state.config),
        "machineEpoch":epoch,"keyGeneration":generation,"firstDueAt":iso(first),"routerTime":iso(now),
        "stateDir":"/var/lib/cc-switch-host-standby","supervision":"unknown","operationSequence":sequence});
    let encoded = base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_vec(&config).map_err(database)?);
    let challenge = Uuid::new_v4().to_string();
    let command = format!(
        "{}\n{}\n{}",
        binary::prepare_command(release, false),
        remote_fence_taking_over(&host.id, sequence, &takeover),
        STAGE
            .replace("@CONFIG@", &shell_quote(&encoded))
            .replace("@CHALLENGE@", &shell_quote(&challenge))
    );
    refresh_claim(&state.store, &host.id, sequence).await?;
    let output = ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts_path(&state.config),
        &host.ip,
        host.port,
        &command,
        None,
        SSH_INSTALL_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await?;
    let probe: serde_json::Value = output
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .ok_or_else(|| {
            AppError::ServiceUnavailable("standby probe returned no valid identity".into())
        })?;
    let public = probe["publicKey"]
        .as_str()
        .ok_or_else(|| AppError::ServiceUnavailable("standby public key missing".into()))?;
    let key: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(public)
        .map_err(database)?
        .try_into()
        .map_err(|_| AppError::BadRequest("invalid public key".into()))?;
    let signature = Signature::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(probe["proof"].as_str().unwrap_or(""))
            .map_err(database)?,
    )
    .map_err(database)?;
    VerifyingKey::from_bytes(&key)
        .map_err(database)?
        .verify_strict(challenge.as_bytes(), &signature)
        .map_err(database)?;
    let supervision = probe["supervision"]
        .as_str()
        .filter(|s| matches!(*s, "systemd" | "openrc" | "supervisor" | "nohup"))
        .unwrap_or("nohup");
    let clock_synchronized = probe["clockSynchronized"].as_bool().unwrap_or(true);
    {
        let conn = state.store.conn.lock().await;
        let tx = conn.transaction().map_err(database)?;
        let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM client_host_agents a JOIN router_ssh_hosts h ON h.id=a.host_id WHERE a.host_id=?1 AND a.operation_sequence=?2 AND h.status IN ('onboarding','maintaining') AND h.installation_id IS NULL)",
            params![host.id,sequence],|row|row.get(0)).map_err(database)?;
        if !valid {
            return Err(AppError::Conflict(
                "Host enrollment changed concurrently".into(),
            ));
        }
        tx.execute("INSERT OR IGNORE INTO client_host_revoked_keys(host_id,machine_epoch,key_generation,public_key,revoked_at)
            SELECT host_id,machine_epoch,key_generation,public_key,?3 FROM client_host_agents WHERE host_id=?1 AND key_generation<>?2 AND public_key IS NOT NULL",
            params![host.id,generation,now]).map_err(database)?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT public_key FROM client_host_agents WHERE host_id=?1 AND key_generation=?2",
                params![host.id, generation],
                |row| row.get(0),
            )
            .optional()
            .map_err(database)?
            .flatten();
        if previous
            .as_deref()
            .is_some_and(|previous| previous != public)
        {
            return Err(AppError::Conflict(
                "standby key changed without a generation change".into(),
            ));
        }
        tx.execute("UPDATE client_host_agents SET public_key=?3,key_generation=?4,supervision=?5,clock_synchronized=?6,last_error=NULL,revoked=0 WHERE host_id=?1 AND operation_sequence=?2",
            params![host.id,sequence,public,generation,supervision,clock_synchronized]).map_err(database)?;
        refresh_summary(&tx, &host.id, now)?;
        tx.commit().map_err(database)?;
    }
    refresh_claim(&state.store, &host.id, sequence).await?;
    let command = format!(
        "{}\n{}",
        remote_fence_taking_over(&host.id, sequence, &takeover),
        START
    );
    ssh_run_remote_with_input(
        &state.provision_ssh_key_path,
        &known_hosts_path(&state.config),
        &host.ip,
        host.port,
        &command,
        None,
        SSH_INSTALL_TIMEOUT,
        SshHostKeyPolicy::RequireKnown,
    )
    .await?;
    let conn = state.store.conn.lock().await;
    let tx = conn.transaction().map_err(database)?;
    let changed=tx.execute("UPDATE client_host_agents SET maintenance_pending=0,last_error=NULL WHERE host_id=?1 AND operation_sequence=?2 AND revoked=0",
        params![host.id,sequence]).map_err(database)?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "Host enrollment completion was superseded".into(),
        ));
    }
    tx.execute("UPDATE router_ssh_hosts SET status=CASE WHEN EXISTS(SELECT 1 FROM client_host_agents WHERE host_id=?1 AND (admitted_hour IS NOT NULL OR legacy=1) AND revoked=0 AND readmission_pending=0) THEN 'idle' ELSE 'onboarding' END,updated_at=?3
        WHERE id=?1 AND status='maintaining' AND installation_id IS NULL AND EXISTS(SELECT 1 FROM client_host_agents WHERE host_id=?1 AND operation_sequence=?2)",
        params![host.id,sequence,Utc::now().to_rfc3339()]).map_err(database)?;
    tx.execute("UPDATE client_binary_preparations SET status='ready',attempts=0,selector=?2,commit_id=?3,last_error=NULL,updated_at=?4 WHERE host_id=?1",
        params![host.id,release.selector,release.commit,Utc::now().to_rfc3339()]).map_err(database)?;
    tx.commit().map_err(database)
}

const STAGE: &str = r#"
binary=/usr/local/bin/cc-switch-host-standby
config=/etc/cc-switch-host-standby/config.json
state=/var/lib/cc-switch-host-standby
"$prepared_binary" version --json | tr -d ' \n' | grep -Eq '"hostHealthProtocolVersion":1[,}]' || { echo 'Server does not support Host health v1' >&2; exit 1; }
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  supervisor=systemd
  systemctl stop cc-switch-host-standby.service 2>/dev/null || true
elif [ -f /run/openrc/softlevel ] && command -v rc-service >/dev/null 2>&1; then
  supervisor=openrc
  rc-service cc-switch-host-standby stop 2>/dev/null || true
elif [ -d /etc/supervisor/conf.d ] && command -v supervisorctl >/dev/null 2>&1 && [ "$(supervisorctl pid 2>/dev/null)" = "1" ]; then
  supervisor=supervisor
  supervisorctl stop cc-switch-host-standby >/dev/null 2>&1 || true
else
  supervisor=nohup
fi
if [ -f "$state/standby.pid" ]; then
  pid=$(cat "$state/standby.pid")
  case "$pid" in ''|*[!0-9]*) exit 1 ;; esac
  exe=$(readlink "/proc/$pid/exe" 2>/dev/null || true)
  # A reused PID after reboot or container restart is stale, not a foreign owner.
  if [ "$exe" = "$binary" ] || [ "$exe" = "$binary (deleted)" ]; then
    kill "$pid"
    count=0
    while kill -0 "$pid" 2>/dev/null; do
      count=$((count+1)); [ "$count" -lt 30 ] || exit 1
      sleep 1
    done
  fi
  rm -f "$state/standby.pid"
fi
if ! id cc-switch-host >/dev/null 2>&1; then
  if command -v useradd >/dev/null 2>&1; then useradd --system --no-create-home --shell /usr/sbin/nologin cc-switch-host
  else adduser -S -H -s /sbin/nologin cc-switch-host; fi
fi
mkdir -p /etc/cc-switch-host-standby "$state"
chmod 755 /etc/cc-switch-host-standby
chmod 700 "$state"
cp "$prepared_binary" "$binary.next"
chmod 755 "$binary.next"
mv -f "$binary.next" "$binary"
printf '%s' @CONFIG@ | base64 -d | sed "s/\"supervision\":\"unknown\"/\"supervision\":\"$supervisor\"/" | "$binary" standby --config "$config" init
chown -R cc-switch-host:cc-switch-host "$state"
chmod 644 "$config"
"$binary" standby --config "$config" probe --challenge @CHALLENGE@
"#;

const START: &str = r#"
binary=/usr/local/bin/cc-switch-host-standby
config=/etc/cc-switch-host-standby/config.json
state=/var/lib/cc-switch-host-standby
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  cat > /etc/systemd/system/cc-switch-host-standby.service <<'UNIT'
[Unit]
Description=Client Market Host health reporter
After=network-online.target
Wants=network-online.target
[Service]
User=cc-switch-host
Group=cc-switch-host
ExecStart=/usr/local/bin/cc-switch-host-standby standby run
Restart=on-failure
RestartSec=30
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=/var/lib/cc-switch-host-standby
[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable --now cc-switch-host-standby.service
elif [ -f /run/openrc/softlevel ] && command -v rc-service >/dev/null 2>&1; then
  mkdir -p /usr/local/libexec
  cat > /usr/local/libexec/cc-switch-host-standby-run <<'RUN'
#!/bin/sh
child=
trap 'test -z "$child" || kill "$child" 2>/dev/null; exit 0' TERM INT
while :; do
  /usr/local/bin/cc-switch-host-standby standby run &
  child=$!
  wait "$child" && exit 0
  child=
  sleep 30 &
  child=$!
  wait "$child"
done
RUN
  chmod 755 /usr/local/libexec/cc-switch-host-standby-run
  cat > /etc/init.d/cc-switch-host-standby <<'INIT'
#!/sbin/openrc-run
name="Client Market Host health"
command="/usr/local/libexec/cc-switch-host-standby-run"
command_user="cc-switch-host:cc-switch-host"
command_background=true
pidfile="/var/lib/cc-switch-host-standby/standby.pid"
depend() { need net; }
INIT
  chmod 755 /etc/init.d/cc-switch-host-standby
  rc-update add cc-switch-host-standby default
  rc-service cc-switch-host-standby start
elif [ -d /etc/supervisor/conf.d ] && command -v supervisorctl >/dev/null 2>&1 && [ "$(supervisorctl pid 2>/dev/null)" = "1" ]; then
  cat > /etc/supervisor/conf.d/cc-switch-host-standby.conf <<'SUPERVISOR'
[program:cc-switch-host-standby]
command=/usr/local/bin/cc-switch-host-standby standby run
user=cc-switch-host
autostart=true
autorestart=unexpected
exitcodes=0
startsecs=0
stopasgroup=true
killasgroup=true
stdout_logfile=/var/log/cc-switch-host-standby.log
stdout_logfile_maxbytes=65536
stdout_logfile_backups=1
redirect_stderr=true
SUPERVISOR
  supervisorctl reread
  supervisorctl update cc-switch-host-standby
  if ! supervisorctl status cc-switch-host-standby | grep -q 'RUNNING'; then supervisorctl start cc-switch-host-standby; fi
else
  su -s /bin/sh cc-switch-host -c 'nohup /usr/local/bin/cc-switch-host-standby standby run </dev/null >>/var/lib/cc-switch-host-standby/standby.log 2>&1 8>&- 9>&- & echo $! >/var/lib/cc-switch-host-standby/standby.pid'
fi
"#;

#[cfg(test)]
mod shell_tests {
    use super::*;

    #[test]
    fn supervisor_autostart_requires_a_persistent_container_entrypoint() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!("host-supervisor-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("supervisorctl");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$HOST_HEALTH_TEST_SUPERVISOR_PID\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        for script in [STAGE, START] {
            let condition = script
                .lines()
                .find(|line| line.starts_with("elif ") && line.contains("/etc/supervisor/conf.d"))
                .unwrap()
                .strip_prefix("elif ")
                .unwrap()
                .strip_suffix("; then")
                .unwrap()
                .replace("/etc/supervisor/conf.d", directory.to_str().unwrap());
            for (pid, expected) in [("1", "supervisor"), ("42", "nohup"), ("0", "nohup")] {
                let result = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(format!(
                        "if {condition}; then printf supervisor; else printf nohup; fi"
                    ))
                    .env(
                        "PATH",
                        format!(
                            "{}:{}",
                            directory.display(),
                            std::env::var("PATH").unwrap_or_default()
                        ),
                    )
                    .env("HOST_HEALTH_TEST_SUPERVISOR_PID", pid)
                    .output()
                    .unwrap();
                assert!(result.status.success());
                assert_eq!(String::from_utf8_lossy(&result.stdout), expected);
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn remote_operations_reject_old_tokens_and_foreign_hosts() {
        let directory = std::env::temp_dir().join(format!("host-fence-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let run = |host: &str, seq: i64| {
            let command = remote_fence(host, seq).replace(
                "/var/lib/cc-switch-host-control",
                directory.to_str().unwrap(),
            );
            std::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
                .unwrap()
        };
        assert!(run("host-a", 2).status.success());
        assert!(run("host-a", 2).status.success());
        assert!(!run("host-a", 1).status.success());
        assert!(!run("host-b", 3).status.success());
        for script in [STAGE, START, REMOVE] {
            let child = std::process::Command::new("bash")
                .arg("-n")
                .arg("-c")
                .arg(script)
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}",
                String::from_utf8_lossy(&child.stderr)
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
