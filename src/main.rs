mod api;
mod config;
mod error;
mod models;
mod proxy;
mod ssh;
mod store;

use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use proxy::ProxyRegistry;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

use crate::config::{Config, ensure_default_env_file, load_env_file};
use crate::store::AppStore;

#[derive(Clone)]
pub struct ServerState {
    pub config: Config,
    pub store: AppStore,
    pub proxy: Arc<ProxyRegistry>,
}

#[tokio::main]
async fn main() -> Result<()> {
    if try_handle_cli()? {
        return Ok(());
    }

    let env_path = ensure_default_env_file()?;
    load_env_file(&env_path)?;

    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .init();

    let config = Config::from_env();
    info!(
        api_addr = %config.api_addr,
        ssh_addr = %config.ssh_addr,
        tunnel_domain = %config.tunnel_domain,
        ssh_public_addr = %config.effective_ssh_public_addr(),
        db_path = %config.db_path.display(),
        env_path = %env_path.display(),
        use_localhost = config.use_localhost,
        cleanup_interval_secs = config.cleanup_interval_secs,
        lease_retention_secs = config.lease_retention_secs,
        "starting portr-rs"
    );
    let state = ServerState {
        config: config.clone(),
        store: AppStore::new(&config)?,
        proxy: Arc::new(ProxyRegistry::default()),
    };

    let ssh_server = ssh::SshServer {
        store: state.store.clone(),
        proxy: state.proxy.clone(),
    };
    let cleanup_store = state.store.clone();
    let cleanup_config = config.clone();

    let http_listener = TcpListener::bind(config.api_addr).await?;
    let ssh_listener = TcpListener::bind(config.ssh_addr).await?;
    info!("http listening on {}", config.api_addr);
    info!("ssh listener bound on {}", config.ssh_addr);

    let cleanup_task = tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(cleanup_config.cleanup_interval_secs));
        loop {
            interval.tick().await;
            match cleanup_store.cleanup_expired_data(&cleanup_config).await {
                Ok((leases, shares)) if leases > 0 || shares > 0 => {
                    info!("cleanup removed {leases} leases and {shares} shares");
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!("cleanup failed: {err}");
                }
            }
        }
    });
    let ssh_task = tokio::spawn(async move { ssh_server.run_with_listener(ssh_listener).await });
    let http_task = tokio::spawn(async move {
        axum::serve(http_listener, api::router(state)).await?;
        Ok::<_, anyhow::Error>(())
    });

    tokio::select! {
        ssh_result = ssh_task => {
            cleanup_task.abort();
            ssh_result??;
            Ok(())
        }
        http_result = http_task => {
            cleanup_task.abort();
            http_result??;
            Ok(())
        }
    }
}

fn try_handle_cli() -> Result<bool> {
    let mut args = env::args().skip(1);
    let Some(arg) = args.next() else {
        return Ok(false);
    };

    match arg.as_str() {
        "help" | "--help" | "-h" => {
            print_help();
            Ok(true)
        }
        other => anyhow::bail!("unknown command: {other}\n\nRun `portr-rs help` for usage."),
    }
}

fn print_help() {
    println!(
        "\
portr-rs

Usage:
  portr-rs
  portr-rs help
  portr-rs --help
  portr-rs -h

Environment:
  PORTR_RS_API_ADDR              HTTP listen address, default 0.0.0.0:8787
  PORTR_RS_SSH_ADDR              SSH listen address, default 0.0.0.0:2222
  PORTR_RS_TUNNEL_DOMAIN         Public tunnel domain, default 0.0.0.0:8787
  PORTR_RS_SSH_PUBLIC_ADDR       SSH address sent to clients, default TUNNEL_DOMAIN:SSH_PORT
  PORTR_RS_USE_LOCALHOST         Use http for localhost-style domains, default true
  PORTR_RS_LEASE_TTL_SECS        Tunnel lease ttl, default 60
  PORTR_RS_DB_PATH               SQLite path, default $HOME/.config/portr-rs/portr-rs.db
  PORTR_RS_CLEANUP_INTERVAL_SECS Cleanup interval, default 300
  PORTR_RS_LEASE_RETENTION_SECS  Lease retention period, default 604800

Default env file:
  $HOME/.config/portr-rs/.env
  The file is auto-created on first start when missing.
"
    );
}
