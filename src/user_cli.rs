//! Local deployment-administrator rescue path. Never exposed through HTTP.
use crate::{
    config::{Config, DatabaseMode},
    db::{Connection, TransactionBehavior, params},
    store::{
        AppStore,
        password_auth::{self, PasswordStatus},
    },
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;
use zeroize::Zeroizing;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

const MAX_FRAME: u64 = 8192;

#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    HostHealthBootstrap {
        host_id: String,
        expected_installation_id: String,
        expected_machine_epoch: i64,
    },
    FinanceSecurity {
        email: String,
        operation: String,
        key_id: Option<String>,
        session_id: Option<String>,
        operation_id: String,
    },
    Status {
        email: String,
    },
    Set {
        email: String,
        password: Zeroizing<String>,
        expected_version: i64,
        operation_id: String,
    },
    Result {
        operation_id: String,
    },
}

#[derive(Serialize, Deserialize)]
struct Reply {
    status: Option<PasswordStatus>,
    operation_id: Option<String>,
    error: Option<String>,
}

fn socket_path(config: &Config) -> PathBuf {
    config.data_dir.join("admin").join("user-password.sock")
}

#[cfg(unix)]
fn uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
pub(crate) struct AdminListener {
    listener: UnixListener,
    path: PathBuf,
    inode: u64,
}

#[cfg(unix)]
impl Drop for AdminListener {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.ino() == self.inode && m.file_type().is_socket())
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
pub(crate) fn bind(config: &Config) -> Result<AdminListener> {
    let path = socket_path(config);
    let parent = path.parent().context("missing admin socket parent")?;
    match std::fs::DirBuilder::new().mode(0o700).create(parent) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    let meta = std::fs::symlink_metadata(parent)?;
    anyhow::ensure!(
        meta.is_dir()
            && !meta.file_type().is_symlink()
            && meta.uid() == uid()
            && meta.permissions().mode() & 0o777 == 0o700,
        "admin directory must be owned by Router and have mode 0700"
    );
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        anyhow::ensure!(
            meta.file_type().is_socket() && meta.uid() == uid(),
            "refusing to replace an unsafe admin socket path"
        );
        // The caller already owns the exclusive Router process lock.
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(AdminListener {
        listener,
        inode: std::fs::symlink_metadata(&path)?.ino(),
        path,
    })
}

#[cfg(unix)]
pub(crate) async fn serve(listener: AdminListener, store: AppStore, config: Config) -> Result<()> {
    let mut jobs = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted=listener.listener.accept()=>{
                let (stream,_)=accepted?;
                let peer=stream.peer_cred()?;
                if !matches!(peer.uid(),v if v==0 || v==uid()) || jobs.len()>=8 { continue; }
                let store=store.clone();
                let config=config.clone();
                jobs.spawn(async move {
                    let _=tokio::time::timeout(std::time::Duration::from_secs(15),async {
                        let mut stream=stream;
                        let mut bytes=Zeroizing::new(Vec::new());
                        (&mut stream).take(MAX_FRAME+1).read_to_end(&mut bytes).await?;
                        anyhow::ensure!(bytes.len()<=MAX_FRAME as usize,"admin request too large");
                        let request:Request=serde_json::from_slice(&bytes)?;
                        let actor=format!("uid:{}",peer.uid());
                        let result=handle(&store,&config,request,&actor).await;
                        let reply=match result {Ok(v)=>v,Err(e)=>Reply {status:None,operation_id:None,error:Some(e.to_string())}};
                        stream.write_all(&serde_json::to_vec(&reply)?).await?;
                        stream.shutdown().await?;
                        Ok::<_,anyhow::Error>(())
                    }).await;
                });
            },
            _=jobs.join_next(),if !jobs.is_empty()=>{}
        }
    }
}

async fn handle(
    store: &AppStore,
    _config: &Config,
    request: Request,
    actor: &str,
) -> Result<Reply> {
    match request {
        Request::HostHealthBootstrap {
            host_id,
            expected_installation_id,
            expected_machine_epoch,
        } => {
            crate::client_market::health::bootstrap::enqueue_operator(
                store,
                &host_id,
                &crate::client_market::health::bootstrap::BootstrapRequest {
                    expected_installation_id,
                    expected_machine_epoch,
                },
                actor,
            )
            .await?;
            Ok(Reply {
                status: None,
                operation_id: Some(host_id),
                error: None,
            })
        }
        Request::FinanceSecurity {
            email,
            operation,
            key_id,
            session_id,
            operation_id,
        } => {
            let conn = store.conn.lock().await;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            financial_recovery(
                &tx,
                &email,
                &operation,
                key_id.as_deref(),
                session_id.as_deref(),
                actor,
                &operation_id,
            )?;
            tx.commit()?;
            Ok(Reply {
                status: None,
                operation_id: Some(operation_id),
                error: None,
            })
        }
        Request::Status { email } => {
            let conn = store.conn.lock().await;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let user = password_auth::status(&tx, &email)?;
            tx.commit()?;
            Ok(Reply {
                status: Some(user),
                operation_id: None,
                error: None,
            })
        }
        Request::Result { operation_id } => {
            let conn = store.conn.lock().await;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let user = operation_result(&tx, &operation_id, actor)?
                .context("operation not found for this administrator")?;
            tx.commit()?;
            Ok(Reply {
                status: Some(user),
                operation_id: Some(operation_id),
                error: None,
            })
        }
        Request::Set {
            email,
            password,
            expected_version,
            operation_id,
        } => {
            Uuid::parse_str(&operation_id).context("invalid operation ID")?;
            {
                let conn = store.conn.lock().await;
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                if let Some(user) = operation_result(&tx, &operation_id, actor)? {
                    anyhow::ensure!(
                        user.email.eq_ignore_ascii_case(email.trim()),
                        "operation ID belongs to another user"
                    );
                    tx.commit()?;
                    return Ok(Reply {
                        status: Some(user),
                        operation_id: Some(operation_id),
                        error: None,
                    });
                }
                tx.commit()?;
            }
            let hash = password_auth::hash_password(password.clone()).await?;
            let conn = store.conn.lock().await;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(user) = operation_result(&tx, &operation_id, actor)? {
                anyhow::ensure!(
                    user.email.eq_ignore_ascii_case(email.trim()),
                    "operation ID belongs to another user"
                );
                tx.commit()?;
                return Ok(Reply {
                    status: Some(user),
                    operation_id: Some(operation_id),
                    error: None,
                });
            }
            let user = password_auth::replace_tx(
                &tx,
                &email,
                &hash,
                expected_version,
                "local_cli",
                actor,
                &operation_id,
                Utc::now(),
            )?;
            tx.commit()?;
            Ok(Reply {
                status: Some(user),
                operation_id: Some(operation_id),
                error: None,
            })
        }
    }
}

fn operation_result(conn: &Connection, id: &str, actor: &str) -> Result<Option<PasswordStatus>> {
    use crate::db::OptionalExtension;
    Uuid::parse_str(id).context("invalid operation ID")?;
    let event: Option<(String, String, String)> = conn
        .query_row(
            "SELECT u.email_normalized,e.actor,e.source FROM auth_password_events e
        JOIN users u ON u.id=e.user_id WHERE e.id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    event
        .map(|(email, owner, source)| {
            anyhow::ensure!(
                source == "local_cli" && (actor == owner || actor == "uid:0"),
                "operation not found for this administrator"
            );
            Ok(password_auth::status(conn, &email)?)
        })
        .transpose()
}

struct Options {
    action: String,
    email: Option<String>,
    env_file: Option<PathBuf>,
    stdin: bool,
    yes: bool,
    offline: bool,
    operation: Option<String>,
}

fn parse(args: Vec<String>) -> Result<Options> {
    let mut args = args.into_iter();
    anyhow::ensure!(
        args.next().as_deref() == Some("password"),
        "usage: cc-switch-router user password <set|status|result> --email EMAIL"
    );
    let action = args
        .next()
        .context("password action is required: set, status or result")?;
    anyhow::ensure!(
        matches!(action.as_str(), "set" | "status" | "result"),
        "unknown password action"
    );
    let mut out = Options {
        action,
        email: None,
        env_file: None,
        stdin: false,
        yes: false,
        offline: false,
        operation: None,
    };
    let mut seen = std::collections::HashSet::new();
    while let Some(arg) = args.next() {
        anyhow::ensure!(
            seen.insert(arg.clone()),
            "duplicate password command option"
        );
        match arg.as_str() {
            "--email" => out.email = Some(args.next().context("--email requires a value")?),
            "--env-file" => {
                out.env_file = Some(PathBuf::from(
                    args.next().context("--env-file requires a path")?,
                ))
            }
            "--operation-id" => {
                out.operation = Some(args.next().context("--operation-id requires a UUID")?)
            }
            "--password-stdin" => out.stdin = true,
            "--yes" => out.yes = true,
            "--offline" => out.offline = true,
            _ => bail!(
                "unsupported password command option (passwords must not be passed as arguments)"
            ),
        }
    }
    anyhow::ensure!(
        out.action == "result" || out.email.is_some(),
        "--email is required"
    );
    anyhow::ensure!(
        out.action != "result" || out.operation.is_some(),
        "--operation-id is required"
    );
    anyhow::ensure!(
        out.action == "set" || (!out.stdin && !out.yes),
        "password input flags require set"
    );
    anyhow::ensure!(!out.stdin || out.yes, "--password-stdin requires --yes");
    anyhow::ensure!(
        (out.action == "result") == out.operation.is_some(),
        "--operation-id is only valid for result"
    );
    anyhow::ensure!(
        out.action != "result" || out.email.is_none(),
        "result does not accept --email"
    );
    if let Some(id) = out.operation.as_deref() {
        Uuid::parse_str(id).context("invalid operation ID")?;
    }
    Ok(out)
}

fn read_password(stdin: bool) -> Result<Zeroizing<String>> {
    if stdin {
        let mut bytes = Zeroizing::new(Vec::new());
        io::stdin().take(1025).read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 1024, "password input is too large");
        let mut password =
            Zeroizing::new(String::from_utf8(bytes.to_vec()).context("password must be UTF-8")?);
        if password.ends_with('\n') {
            password.pop();
            if password.ends_with('\r') {
                password.pop();
            }
        }
        password_auth::validate_password(&password)?;
        return Ok(password);
    }
    anyhow::ensure!(
        io::stdin().is_terminal(),
        "use --password-stdin --yes for noninteractive input"
    );
    eprintln!("{}", password_auth::PASSWORD_POLICY_HINT);
    let password = Zeroizing::new(rpassword::prompt_password("New password: ")?);
    let confirm = Zeroizing::new(rpassword::prompt_password("Confirm new password: ")?);
    anyhow::ensure!(*password == *confirm, "passwords do not match");
    password_auth::validate_password(&password)?;
    Ok(password)
}

fn open_offline(config: &Config) -> Result<Connection> {
    anyhow::ensure!(
        config.database.path.is_file(),
        "database does not exist; refusing to create a new database"
    );
    let conn = match config.database.mode {
        DatabaseMode::Local => Connection::open(&config.database.path)?,
        DatabaseMode::Turso => Connection::open_remote_replica(
            &config.database.path,
            config
                .database
                .turso_url
                .clone()
                .context("missing Turso URL")?,
            config
                .database
                .turso_auth_token
                .clone()
                .context("missing Turso token")?,
        )?,
    };
    crate::schema::check_compatibility(&conn)?;
    let version: i64 = conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
        r.get(0)
    })?;
    anyhow::ensure!(
        version >= 90,
        "password schema is not installed; start the updated Router once before using this command"
    );
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000_i64)?;
    Ok(conn)
}

#[cfg(unix)]
fn check_offline_target(config: &Config) -> Result<()> {
    let directory = std::fs::symlink_metadata(&config.data_dir)
        .context("Router data directory does not exist; refusing to create it")?;
    let database = std::fs::symlink_metadata(&config.database.path)
        .context("Router database does not exist")?;
    anyhow::ensure!(
        directory.is_dir()
            && !directory.file_type().is_symlink()
            && database.is_file()
            && !database.file_type().is_symlink(),
        "offline target must be an existing real directory and database, not symlinks"
    );
    anyhow::ensure!(
        (uid() == 0 || uid() == directory.uid()) && directory.uid() == database.uid(),
        "only root or the Router service user may manage passwords offline"
    );
    let lock = config.data_dir.join("cc-switch-router.lock");
    match std::fs::symlink_metadata(lock) {
        Ok(meta) => anyhow::ensure!(
            meta.is_file() && !meta.file_type().is_symlink() && meta.uid() == directory.uid(),
            "unsafe Router process lock path"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => anyhow::ensure!(
            uid() == directory.uid(),
            "Router process lock is missing; run once as the service user to avoid creating a root-owned lock"
        ),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(unix)]
async fn call_online(config: &Config, request: Request) -> Result<Reply> {
    let path = socket_path(config);
    let parent = std::fs::symlink_metadata(path.parent().context("admin directory missing")?)
        .context(
            "Router admin channel unavailable; if Router is stopped, explicitly use --offline",
        )?;
    anyhow::ensure!(
        parent.is_dir() && !parent.file_type().is_symlink() && parent.mode() & 0o777 == 0o700,
        "unsafe admin directory"
    );
    let meta = std::fs::symlink_metadata(&path)?;
    anyhow::ensure!(
        meta.file_type().is_socket() && meta.uid() == parent.uid() && meta.mode() & 0o777 == 0o600,
        "unsafe admin socket"
    );
    anyhow::ensure!(
        uid() == 0 || uid() == meta.uid(),
        "only root or the Router service user may manage passwords"
    );
    tokio::time::timeout(std::time::Duration::from_secs(20),async {
        let mut stream=UnixStream::connect(path).await?;
        anyhow::ensure!(stream.peer_cred()?.uid()==meta.uid(),"unexpected Router process identity");
        let bytes=Zeroizing::new(serde_json::to_vec(&request)?);
        stream.write_all(&bytes).await?;
        stream.shutdown().await?;
        let mut response=Vec::new();
        stream.take(MAX_FRAME+1).read_to_end(&mut response).await?;
        anyhow::ensure!(response.len()<=MAX_FRAME as usize,"admin response too large");
        Ok::<_,anyhow::Error>(serde_json::from_slice::<Reply>(&response)?)
    }).await.context("admin request timed out; check the operation ID before retrying; do not switch to offline writes")?
}

/// The existing UID-authenticated admin channel keeps all writes inside Router.
pub(crate) async fn run_host_health(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    anyhow::ensure!(
        args.next().as_deref() == Some("bootstrap"),
        "use host-health bootstrap --host-id ID --expected-installation-id ID --expected-machine-epoch N"
    );
    let mut host = None;
    let mut installation = None;
    let mut epoch = None;
    let mut env_file = None;
    let mut seen = std::collections::HashSet::new();
    while let Some(flag) = args.next() {
        anyhow::ensure!(
            seen.insert(flag.clone()),
            "duplicate monitor bootstrap option"
        );
        let value = args
            .next()
            .context("monitor bootstrap option requires a value")?;
        match flag.as_str() {
            "--host-id" => host = Some(value),
            "--expected-installation-id" => installation = Some(value),
            "--expected-machine-epoch" => {
                epoch = Some(value.parse::<i64>().context("invalid machine epoch")?)
            }
            "--env-file" => env_file = Some(PathBuf::from(value)),
            _ => bail!("unsupported monitor bootstrap option"),
        }
    }
    let host = host.context("--host-id is required")?;
    let installation = installation.context("--expected-installation-id is required")?;
    let epoch = epoch.context("--expected-machine-epoch is required")?;
    anyhow::ensure!(
        !host.is_empty() && !installation.is_empty() && epoch > 0,
        "invalid monitor bootstrap context"
    );
    crate::config::load_env_file(&env_file.unwrap_or_else(crate::config::default_env_path))?;
    let config = Config::from_env();
    #[cfg(unix)]
    {
        let reply = call_online(
            &config,
            Request::HostHealthBootstrap {
                host_id: host.clone(),
                expected_installation_id: installation,
                expected_machine_epoch: epoch,
            },
        )
        .await?;
        if let Some(error) = reply.error {
            bail!("{error}")
        }
        anyhow::ensure!(
            reply.operation_id.as_deref() == Some(host.as_str()),
            "unexpected monitor bootstrap acknowledgement"
        );
        println!("{}", serde_json::json!({"hostId":host,"queued":true}));
    }
    #[cfg(not(unix))]
    bail!("monitor bootstrap administration requires Unix");
    Ok(())
}

pub(crate) async fn run(args: Vec<String>) -> Result<()> {
    if args.first().is_some_and(|s| s == "finance-passkey") {
        return run_financial(args.into_iter().skip(1).collect()).await;
    }
    let options = parse(args)?;
    let env_file = options
        .env_file
        .unwrap_or_else(crate::config::default_env_path);
    anyhow::ensure!(
        env_file.is_file(),
        "configuration file not found; use --env-file to select the deployed Router"
    );
    crate::config::load_env_file(&env_file)?;
    let config = Config::from_env();
    config
        .validate_database_config()
        .map_err(anyhow::Error::msg)?;
    eprintln!(
        "Router: {}\nDatabase: {} ({})",
        config.tunnel_domain,
        config.database.path.display(),
        config.database.mode.as_str()
    );
    let _lock = if options.offline {
        #[cfg(unix)]
        check_offline_target(&config)?;
        #[cfg(not(unix))]
        bail!("password administration requires Unix");
        Some(crate::process_lock::ProcessLock::acquire(&config.data_dir)?)
    } else {
        None
    };
    let offline = if options.offline {
        Some(open_offline(&config)?)
    } else {
        None
    };
    async fn perform(
        config: &Config,
        conn: Option<&Connection>,
        request: Request,
    ) -> Result<Reply> {
        if let Some(conn) = conn {
            #[cfg(unix)]
            let actor = format!("uid:{}", uid());
            #[cfg(not(unix))]
            let actor = "unsupported";
            match request {
                Request::HostHealthBootstrap { .. } => {
                    bail!("monitor bootstrap requires the online Router admin channel")
                }
                Request::FinanceSecurity {
                    email,
                    operation,
                    key_id,
                    session_id,
                    operation_id,
                } => {
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    financial_recovery(
                        &tx,
                        &email,
                        &operation,
                        key_id.as_deref(),
                        session_id.as_deref(),
                        &actor,
                        &operation_id,
                    )?;
                    tx.commit()?;
                    Ok(Reply {
                        status: None,
                        operation_id: Some(operation_id),
                        error: None,
                    })
                }
                Request::Status { email } => {
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let status = password_auth::status(&tx, &email)?;
                    tx.commit()?;
                    Ok(Reply {
                        status: Some(status),
                        operation_id: None,
                        error: None,
                    })
                }
                Request::Result { operation_id } => {
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let status = operation_result(&tx, &operation_id, &actor)?
                        .context("operation not found for this administrator")?;
                    tx.commit()?;
                    Ok(Reply {
                        status: Some(status),
                        operation_id: Some(operation_id),
                        error: None,
                    })
                }
                Request::Set {
                    email,
                    password,
                    expected_version,
                    operation_id,
                } => {
                    let hash = password_auth::hash_password(password.clone()).await?;
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let status = password_auth::replace_tx(
                        &tx,
                        &email,
                        &hash,
                        expected_version,
                        "local_cli",
                        &actor,
                        &operation_id,
                        Utc::now(),
                    )?;
                    tx.commit()?;
                    Ok(Reply {
                        status: Some(status),
                        operation_id: Some(operation_id),
                        error: None,
                    })
                }
            }
        } else {
            #[cfg(unix)]
            {
                call_online(config, request).await
            }
            #[cfg(not(unix))]
            {
                let _ = (config, request);
                bail!("online password management requires Unix")
            }
        }
    }
    let request = if options.action == "result" {
        Request::Result {
            operation_id: options.operation.context("missing operation ID")?,
        }
    } else {
        Request::Status {
            email: options.email.clone().context("missing email")?,
        }
    };
    let reply = perform(&config, offline.as_ref(), request).await?;
    if let Some(error) = reply.error {
        bail!("{error}");
    }
    let status = reply.status.context("missing password status")?;
    if options.action != "set" {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    eprintln!(
        "User: {}\nAccount: {}\nExisting password: {}\nAll previous login sessions will be revoked.\nA confirmation email (without the password) will be sent. A password set here can confirm wallet payments at once.",
        status.email, status.status, status.has_password
    );
    if !options.yes {
        anyhow::ensure!(
            io::stdin().is_terminal(),
            "confirmation required; use --password-stdin --yes for automation"
        );
        eprint!("Set/replace this user's password? [y/N] ");
        io::stderr().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        anyhow::ensure!(
            line.trim().eq_ignore_ascii_case("y"),
            "cancelled; no password changed"
        );
    }
    let password = read_password(options.stdin)?;
    let operation_id = Uuid::new_v4().to_string();
    eprintln!("Operation ID: {operation_id}");
    let reply = perform(
        &config,
        offline.as_ref(),
        Request::Set {
            email: status.email,
            password,
            expected_version: status.version,
            operation_id,
        },
    )
    .await?;
    if let Some(error) = reply.error {
        bail!("{error}");
    }
    println!("Password updated. All previous login sessions have been revoked.");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn financial_recovery(
    _conn: &Connection,
    _email: &str,
    _operation: &str,
    _key: Option<&str>,
    _session: Option<&str>,
    _actor: &str,
    _id: &str,
) -> Result<()> {
    // Wallet Passkeys were replaced by login-password confirmation. A user who
    // lost their password signs in with an email code and sets a new one; an
    // administrator can use `user password set`.
    bail!(
        "finance-passkey was retired: wallet operations are confirmed with the login password; use `user password set` to reset a lost password"
    )
}
async fn run_financial(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let operation = args
        .next()
        .context("use finance-passkey <approve|recover>")?;
    anyhow::ensure!(
        matches!(operation.as_str(), "approve" | "recover"),
        "unknown wallet security operation"
    );
    let mut email = None;
    let mut key = None;
    let mut session = None;
    let mut env_file = None;
    let mut offline = false;
    let mut id = None;
    let mut seen = std::collections::HashSet::new();
    while let Some(arg) = args.next() {
        anyhow::ensure!(seen.insert(arg.clone()), "duplicate wallet security option");
        match arg.as_str() {
            "--email" => email = Some(args.next().context("--email requires a value")?),
            "--key-id" => key = Some(args.next().context("--key-id requires a value")?),
            "--session-id" => session = Some(args.next().context("--session-id requires a value")?),
            "--env-file" => {
                env_file = Some(PathBuf::from(
                    args.next().context("--env-file requires a path")?,
                ))
            }
            "--operation-id" => id = Some(args.next().context("--operation-id requires a UUID")?),
            "--offline" => offline = true,
            _ => bail!("unsupported wallet security option"),
        }
    }
    anyhow::ensure!(email.is_some(), "--email is required");
    anyhow::ensure!(
        if operation == "approve" {
            key.is_some() && session.is_none()
        } else {
            session.is_some() && key.is_none()
        },
        "identify exactly one pending key for approve or fresh login session for recover"
    );
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());
    Uuid::parse_str(&id).context("invalid operation UUID")?;
    let env_file = env_file.unwrap_or_else(crate::config::default_env_path);
    anyhow::ensure!(
        env_file.is_file(),
        "configuration file not found; use --env-file"
    );
    crate::config::load_env_file(&env_file)?;
    let config = Config::from_env();
    config
        .validate_database_config()
        .map_err(anyhow::Error::msg)?;
    eprintln!(
        "Wallet security operation: {}\nRouter: {}\nDatabase: {}",
        id,
        config.tunnel_domain,
        config.database.path.display()
    );
    let request = Request::FinanceSecurity {
        email: email.context("missing email")?,
        operation,
        key_id: key,
        session_id: session,
        operation_id: id.clone(),
    };
    #[cfg(unix)]
    {
        if offline {
            check_offline_target(&config)?;
            let _lock = crate::process_lock::ProcessLock::acquire(&config.data_dir)?;
            let conn = open_offline(&config)?;
            let Request::FinanceSecurity {
                email,
                operation,
                key_id,
                session_id,
                operation_id,
            } = request
            else {
                unreachable!()
            };
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            financial_recovery(
                &tx,
                &email,
                &operation,
                key_id.as_deref(),
                session_id.as_deref(),
                &format!("uid:{}", uid()),
                &operation_id,
            )?;
            tx.commit()?;
        } else {
            let reply = call_online(&config, request).await?;
            if let Some(e) = reply.error {
                bail!("{e}")
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (request, config, offline);
        bail!("wallet security administration requires Unix");
    }
    println!("Wallet security operation completed: {id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    struct Fixture {
        config: Config,
        store: AppStore,
        directory: PathBuf,
    }

    #[cfg(unix)]
    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("router-pass-cli-{}", Uuid::new_v4()));
            std::fs::create_dir(&directory).unwrap();
            let mut config = crate::store::tests::test_config("user-cli");
            config.data_dir = directory.clone();
            config.database.path = directory.join("router.sqlite");
            crate::test_support::database_at(&config.database.path).unwrap();
            let store = AppStore::new(&config).unwrap();
            Self {
                config,
                store,
                directory,
            }
        }

        async fn seed(&self) {
            let conn = self.store.conn.lock().await;
            conn.execute("INSERT INTO users(id,email_normalized,status,created_at,last_login_at,email_verified_at)
                VALUES('cli-user','cli@example.test','active',?1,?1,?1)", params![Utc::now().to_rfc3339()]).unwrap();
        }
    }

    #[cfg(unix)]
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn parser_refuses_plaintext_and_unsafe_automation() {
        for args in [
            vec![
                "password",
                "set",
                "--email",
                "a@b.test",
                "--password",
                "secret",
            ],
            vec!["password", "set", "--email", "a@b.test", "--password-stdin"],
            vec!["password", "status", "--email", "a@b.test", "--yes"],
            vec![
                "password", "status", "--email", "a@b.test", "--email", "c@d.test",
            ],
            vec![
                "password",
                "set",
                "--email",
                "a@b.test",
                "--operation-id",
                "ignored",
            ],
            vec!["password", "result", "--operation-id", "not-a-uuid"],
        ] {
            assert!(parse(args.into_iter().map(String::from).collect()).is_err());
        }
        assert!(
            parse(
                [
                    "password",
                    "set",
                    "--email",
                    "a@b.test",
                    "--password-stdin",
                    "--yes"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            )
            .is_ok()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_socket_bootstraps_only_the_expected_rental_without_other_writes() {
        let fixture = Fixture::new();
        fixture.seed().await;
        {
            let conn = fixture.store.conn.lock().await;
            let now = Utc::now().to_rfc3339();
            conn.execute("INSERT INTO router_ssh_hosts(id,ip,port,host_owner_email,provider_id,status,created_at,updated_at) VALUES('socket-host','192.0.2.46',22,'cli@example.test','cli-user','onboarding',?1,?1)", params![now]).unwrap();
            conn.execute(
                "UPDATE client_host_agents SET legacy=1 WHERE host_id='socket-host'",
                [],
            )
            .unwrap();
            conn.execute("UPDATE router_ssh_hosts SET status='allocated',installation_id='socket-lease' WHERE id='socket-host'", []).unwrap();
            conn.execute("INSERT INTO client_market_subscriptions(installation_id,host_id,provider_id,host_owner_email,client_user_id,client_owner_email,status,offer_revision,created_at,updated_at) VALUES('socket-lease','socket-host','cli-user','cli@example.test','cli-user','cli@example.test','active',7,?1,?1)", params![now]).unwrap();
        }
        let unchanged = |conn: &Connection| {
            conn.query_row("SELECT h.status,h.installation_id,s.status,s.offer_revision,(SELECT COUNT(*) FROM users),(SELECT COUNT(*) FROM auth_password_events),(SELECT COUNT(*) FROM market_wallet_accounts),(SELECT COUNT(*) FROM market_wallet_holds) FROM router_ssh_hosts h JOIN client_market_subscriptions s ON s.host_id=h.id WHERE h.id='socket-host'", [], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, i64>(7)?,
                    ))
                })
                .unwrap()
        };
        let before = unchanged(&*fixture.store.conn.lock().await);
        let _lock = crate::process_lock::ProcessLock::acquire(&fixture.config.data_dir).unwrap();
        let listener = bind(&fixture.config).unwrap();
        let service = tokio::spawn(serve(
            listener,
            fixture.store.clone(),
            fixture.config.clone(),
        ));
        for (installation, epoch) in [("wrong-lease", 1), ("socket-lease", 2)] {
            let reply = call_online(
                &fixture.config,
                Request::HostHealthBootstrap {
                    host_id: "socket-host".into(),
                    expected_installation_id: installation.into(),
                    expected_machine_epoch: epoch,
                },
            )
            .await
            .unwrap();
            assert!(reply.error.is_some());
        }
        for _ in 0..2 {
            let reply = call_online(
                &fixture.config,
                Request::HostHealthBootstrap {
                    host_id: "socket-host".into(),
                    expected_installation_id: "socket-lease".into(),
                    expected_machine_epoch: 1,
                },
            )
            .await
            .unwrap();
            assert!(reply.error.is_none());
            assert_eq!(reply.operation_id.as_deref(), Some("socket-host"));
        }
        {
            let conn = fixture.store.conn.lock().await;
            assert_eq!(unchanged(&conn), before);
            let task: (String, String, Option<String>, i64) = conn.query_row("SELECT status,requested_by,owner_user_id,operation_sequence FROM client_host_agent_bootstraps WHERE host_id='socket-host'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
            assert_eq!(task, ("pending".into(), format!("uid:{}", uid()), None, 1));
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM client_host_hour_reports", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                0
            );
        }
        service.abort();
        let _ = service.await;
        assert!(!socket_path(&fixture.config).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_socket_sets_existing_users_idempotently_and_supports_operation_recovery() {
        let _hash = password_auth::HASH_TEST_LOCK.lock().await;
        let fixture = Fixture::new();
        fixture.seed().await;
        let _lock = crate::process_lock::ProcessLock::acquire(&fixture.config.data_dir).unwrap();
        let listener = bind(&fixture.config).unwrap();
        assert_eq!(
            std::fs::metadata(socket_path(&fixture.config))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
        let service = tokio::spawn(serve(
            listener,
            fixture.store.clone(),
            fixture.config.clone(),
        ));
        let status = call_online(
            &fixture.config,
            Request::Status {
                email: "cli@example.test".into(),
            },
        )
        .await
        .unwrap()
        .status
        .unwrap();
        assert!(!status.has_password);
        let operation_id = Uuid::new_v4().to_string();
        let set_request = || Request::Set {
            email: "cli@example.test".into(),
            password: Zeroizing::new("cli123".into()),
            expected_version: 0,
            operation_id: operation_id.clone(),
        };
        let (first, second) = tokio::join!(
            call_online(&fixture.config, set_request()),
            call_online(&fixture.config, set_request())
        );
        for reply in [first.unwrap(), second.unwrap()] {
            assert!(reply.error.is_none());
            assert_eq!(reply.status.unwrap().version, 1);
        }
        let recovered = call_online(
            &fixture.config,
            Request::Result {
                operation_id: operation_id.clone(),
            },
        )
        .await
        .unwrap();
        assert!(recovered.error.is_none());
        assert_eq!(recovered.status.unwrap().version, 1);
        {
            let conn = fixture.store.conn.lock().await;
            // The confirmation email no longer carries the password.
            let sealed: Option<String> = conn
                .query_row(
                    "SELECT envelope_json FROM auth_password_notifications",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(sealed, None);
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM auth_password_events", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM auth_password_notifications",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM user_api_tokens", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert!(operation_result(&conn, &operation_id, "uid:123456").is_err());
        }
        let missing = call_online(
            &fixture.config,
            Request::Status {
                email: "unknown@example.test".into(),
            },
        )
        .await
        .unwrap();
        assert!(missing.error.unwrap().contains("does not create accounts"));
        let oversized = call_online(
            &fixture.config,
            Request::Set {
                email: "cli@example.test".into(),
                password: Zeroizing::new("x".repeat(9000)),
                expected_version: 1,
                operation_id: Uuid::new_v4().to_string(),
            },
        )
        .await;
        assert!(oversized.is_err());
        service.abort();
        let _ = service.await;
        assert!(!socket_path(&fixture.config).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn offline_requires_existing_safe_targets_and_exclusive_process_lock() {
        let fixture = Fixture::new();
        fixture.seed().await;
        check_offline_target(&fixture.config).unwrap();
        let lock = crate::process_lock::ProcessLock::acquire(&fixture.config.data_dir).unwrap();
        assert!(crate::process_lock::ProcessLock::acquire(&fixture.config.data_dir).is_err());
        drop(lock);
        let _lock = crate::process_lock::ProcessLock::acquire(&fixture.config.data_dir).unwrap();
        let conn = open_offline(&fixture.config).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let result = password_auth::replace_tx(
            &tx,
            "cli@example.test",
            "hash",
            0,
            "local_cli",
            &format!("uid:{}", uid()),
            &Uuid::new_v4().to_string(),
            Utc::now(),
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(result.version, 1);
        assert!(
            call_online(
                &fixture.config,
                Request::Status {
                    email: "cli@example.test".into()
                }
            )
            .await
            .is_err(),
            "offline writes must never be an automatic online fallback"
        );
        let mut wrong = fixture.config.clone();
        wrong.data_dir = fixture.directory.join("missing");
        assert!(check_offline_target(&wrong).is_err());
        assert!(!wrong.data_dir.exists());
        wrong.database.path = fixture.directory.join("link.sqlite");
        wrong.data_dir = fixture.directory.clone();
        std::os::unix::fs::symlink(&fixture.config.database.path, &wrong.database.path).unwrap();
        assert!(check_offline_target(&wrong).is_err());
        assert!(
            open_offline(&Config {
                database: crate::config::DatabaseConfig {
                    path: fixture.directory.join("absent.sqlite"),
                    ..fixture.config.database.clone()
                },
                ..fixture.config.clone()
            })
            .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn socket_refuses_unsafe_directories_files_and_modes() {
        let fixture = Fixture::new();
        let admin = fixture.directory.join("admin");
        std::fs::create_dir(&admin).unwrap();
        std::fs::set_permissions(&admin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(bind(&fixture.config).is_err());
        std::fs::set_permissions(&admin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = socket_path(&fixture.config);
        std::fs::write(&socket, "must not overwrite a regular file").unwrap();
        assert!(bind(&fixture.config).is_err());
        assert_eq!(
            std::fs::read_to_string(&socket).unwrap(),
            "must not overwrite a regular file"
        );
        std::fs::remove_file(&socket).unwrap();
        let listener = bind(&fixture.config).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(
            call_online(
                &fixture.config,
                Request::Status {
                    email: "cli@example.test".into()
                }
            )
            .await
            .is_err()
        );
        drop(listener);
    }
}
