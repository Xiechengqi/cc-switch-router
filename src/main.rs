mod api;
mod cf;
mod client_meta;
mod config;
mod error;
mod geo;
mod models;
mod proxy;
mod recent_traffic;
mod ssh;
mod store;

use std::collections::HashSet;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use proxy::ProxyRegistry;
use resend_rs::Resend;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

use crate::config::{Config, ensure_default_env_file, load_env_file};
use crate::recent_traffic::RecentTraffic;
use crate::store::{AppStore, ShareRouteTarget, fetch_share_runtime_snapshot_from_route};

const APP_NAME: &str = "cc-switch-router";

#[derive(Clone)]
pub struct ServerState {
    pub config: Config,
    pub server_geo: ServerGeo,
    pub store: AppStore,
    pub proxy: Arc<ProxyRegistry>,
    pub resend: Option<Arc<Resend>>,
    pub resend_usage_cache: Arc<Mutex<Option<ResendUsageCache>>>,
    /// SSH host key 指纹（`SHA256:<base64-nopad>` 格式），在 /lease 响应中回传给客户端。
    pub ssh_host_fingerprint: Option<String>,
    /// In-memory rolling tracker of proxy traffic by user origin. Drives the dashboard
    /// "demand" overlay and burst-arc animation; not persisted across restarts.
    pub recent_traffic: RecentTraffic,
}

#[derive(Debug, Clone)]
pub struct ResendUsageCache {
    pub fetched_at_unix_secs: i64,
    pub value: crate::models::ResendUsageResponse,
}

#[derive(Debug, Clone)]
pub struct ServerGeo {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
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
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let config = Config::from_env();
    let server_geo = resolve_server_geo().await;
    info!(
        api_addr = %config.api_addr,
        ssh_addr = %config.ssh_addr,
        tunnel_domain = %config.tunnel_domain,
        ssh_public_addr = %config.effective_ssh_public_addr(),
        server_label = "server",
        server_lat = server_geo.lat,
        server_lon = server_geo.lon,
        db_path = %config.db_path.display(),
        env_path = %env_path.display(),
        use_localhost = config.use_localhost,
        cleanup_interval_secs = config.cleanup_interval_secs,
        lease_retention_secs = config.lease_retention_secs,
        client_stale_secs = config.client_stale_secs,
        "starting cc-switch-router"
    );
    // 预加载 SSH host key 并计算指纹，提前失败在配置错误；也作为 lease 响应返回给客户端。
    let ssh_host_key = ssh::load_or_generate_host_key(&config.host_key_path)?;
    let ssh_host_fingerprint = ssh::host_key_fingerprint(&ssh_host_key).ok();
    let resend = config
        .resend_api_key
        .as_deref()
        .map(Resend::new)
        .map(Arc::new);
    if let Some(ref fp) = ssh_host_fingerprint {
        info!("ssh host key fingerprint: {}", fp);
    }

    let state = ServerState {
        config: config.clone(),
        server_geo: server_geo.clone(),
        store: AppStore::new(&config)?,
        proxy: Arc::new(ProxyRegistry::default()),
        resend,
        resend_usage_cache: Arc::new(Mutex::new(None)),
        ssh_host_fingerprint: ssh_host_fingerprint.clone(),
        recent_traffic: RecentTraffic::new(),
    };

    let ssh_server = ssh::SshServer {
        store: state.store.clone(),
        proxy: state.proxy.clone(),
        host_key: ssh_host_key,
    };
    let cleanup_store = state.store.clone();
    let cleanup_config = config.clone();
    let cleanup_proxy = state.proxy.clone();
    let probe_store = state.store.clone();
    let probe_proxy = state.proxy.clone();
    let probe_config = config.clone();
    let runtime_store = state.store.clone();
    let runtime_proxy = state.proxy.clone();
    let runtime_config = config.clone();
    let resend_usage_cache = state.resend_usage_cache.clone();
    let resend_usage_api_key = config.resend_api_key.clone();

    let http_listener = TcpListener::bind(config.api_addr).await?;
    let ssh_listener = TcpListener::bind(config.ssh_addr).await?;
    info!("http listening on {}", config.api_addr);
    info!("ssh listener bound on {}", config.ssh_addr);

    let cleanup_task = tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(cleanup_config.cleanup_interval_secs));
        loop {
            interval.tick().await;
            match cleanup_store
                .cleanup_expired_data(&cleanup_config, &cleanup_proxy)
                .await
            {
                Ok(result) if result.has_changes() => {
                    info!(
                        leases = result.deleted_leases,
                        shares = result.deleted_shares,
                        installations = result.deleted_installations,
                        routes = result.removed_routes,
                        "cleanup removed stale data"
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!("cleanup failed: {err}");
                }
            }
        }
    });
    let probe_task = tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .user_agent("cc-switch-router/0.1 route-probe")
            .timeout(Duration::from_secs(5))
            .build()?;

        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(err) =
                run_route_health_probe_cycle(&probe_store, &probe_proxy, &probe_config, &client)
                    .await
            {
                tracing::warn!("route health probe failed: {err}");
            }
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    });
    let runtime_task = tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .user_agent("cc-switch-router/0.1 share-runtime")
            .timeout(Duration::from_secs(5))
            .build()?;

        let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(err) = run_share_runtime_refresh_cycle(
                &runtime_store,
                &runtime_proxy,
                &runtime_config,
                &client,
            )
            .await
            {
                tracing::warn!("share runtime refresh failed: {err}");
            }
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    });
    let resend_usage_task = tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .user_agent("cc-switch-router/0.1 resend-usage")
            .timeout(Duration::from_secs(10))
            .build()?;

        let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
        loop {
            interval.tick().await;
            match refresh_resend_usage_cache(
                resend_usage_cache.clone(),
                resend_usage_api_key.as_deref(),
                &client,
            )
            .await
            {
                Ok(Some(label)) => info!(resend_daily_usage = %label, "updated resend daily usage"),
                Ok(None) => info!("resend daily quota header missing, footer hidden"),
                Err(err) => tracing::warn!("refresh resend usage failed: {err}"),
            }
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    });
    let ssh_task = tokio::spawn(async move { ssh_server.run_with_listener(ssh_listener).await });
    let http_task = tokio::spawn(async move {
        axum::serve(
            http_listener,
            api::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await?;
        Ok::<_, anyhow::Error>(())
    });

    tokio::select! {
        ssh_result = ssh_task => {
            cleanup_task.abort();
            probe_task.abort();
            runtime_task.abort();
            resend_usage_task.abort();
            ssh_result??;
            Ok(())
        }
        http_result = http_task => {
            cleanup_task.abort();
            probe_task.abort();
            runtime_task.abort();
            resend_usage_task.abort();
            http_result??;
            Ok(())
        }
    }
}

async fn refresh_resend_usage_cache(
    cache: Arc<Mutex<Option<ResendUsageCache>>>,
    api_key: Option<&str>,
    client: &reqwest::Client,
) -> Result<Option<String>> {
    let value = fetch_resend_usage(api_key, client).await?;
    let label = if value.available && !value.daily_usage_label.is_empty() {
        Some(value.daily_usage_label.clone())
    } else {
        None
    };
    let mut guard = cache.lock().await;
    *guard = Some(ResendUsageCache {
        fetched_at_unix_secs: chrono::Utc::now().timestamp(),
        value,
    });
    Ok(label)
}

async fn fetch_resend_usage(
    api_key: Option<&str>,
    client: &reqwest::Client,
) -> Result<crate::models::ResendUsageResponse> {
    let Some(api_key) = api_key.filter(|value| !value.trim().is_empty()) else {
        return Ok(crate::models::ResendUsageResponse {
            available: false,
            daily_usage_percent: None,
            daily_usage_label: String::new(),
            quota_header: None,
        });
    };

    let response = client
        .get("https://api.resend.com/domains")
        .bearer_auth(api_key)
        .send()
        .await
        .context("request resend domains failed")?;

    let headers = response.headers().clone();
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("resend usage request failed: HTTP {status} {body}");
    }

    let quota_header = headers
        .get("x-resend-daily-quota")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let Some(quota_header) = quota_header else {
        return Ok(crate::models::ResendUsageResponse {
            available: false,
            daily_usage_percent: None,
            daily_usage_label: String::new(),
            quota_header: None,
        });
    };

    let used_quota: f64 = quota_header
        .parse()
        .with_context(|| format!("parse x-resend-daily-quota failed: {quota_header}"))?;
    let percent = used_quota;
    let label = format!("{percent:.0}%");

    Ok(crate::models::ResendUsageResponse {
        available: true,
        daily_usage_percent: Some(percent),
        daily_usage_label: label,
        quota_header: Some(quota_header),
    })
}

async fn run_route_health_probe_cycle(
    store: &AppStore,
    proxy: &ProxyRegistry,
    config: &Config,
    client: &reqwest::Client,
) -> Result<()> {
    let active = proxy
        .active_subdomains()
        .await
        .into_iter()
        .collect::<HashSet<_>>();
    let targets = store.list_share_route_targets().await?;
    for target in targets {
        let is_healthy = if active.contains(&target.subdomain) {
            probe_share_route(config, client, &target).await
        } else {
            false
        };
        if let Err(err) = store
            .record_share_route_health(&target.share_id, is_healthy)
            .await
        {
            tracing::warn!(share_id = %target.share_id, "record route health failed: {err}");
        }
    }
    Ok(())
}

async fn run_share_runtime_refresh_cycle(
    store: &AppStore,
    proxy: &ProxyRegistry,
    config: &Config,
    client: &reqwest::Client,
) -> Result<()> {
    let targets = filter_registered_route_targets(
        store.list_share_route_targets().await?,
        proxy.active_subdomains().await,
    );
    for target in targets {
        match fetch_share_runtime_snapshot_from_route(config, client, &target.subdomain).await {
            Ok(snapshot) => {
                if let Err(err) = store.record_share_runtime_snapshot(snapshot).await {
                    tracing::warn!(share_id = %target.share_id, "record share runtime failed: {err}");
                }
            }
            Err(err) => {
                tracing::warn!(share_id = %target.share_id, "fetch share runtime failed: {err}");
            }
        }
    }
    Ok(())
}

fn filter_registered_route_targets(
    targets: Vec<ShareRouteTarget>,
    active_subdomains: Vec<String>,
) -> Vec<ShareRouteTarget> {
    let active = active_subdomains.into_iter().collect::<HashSet<_>>();
    targets
        .into_iter()
        .filter(|target| active.contains(&target.subdomain))
        .collect()
}

async fn probe_share_route(
    config: &Config,
    client: &reqwest::Client,
    target: &ShareRouteTarget,
) -> bool {
    let url = format!(
        "{}/_share-router/health",
        config.tunnel_url(&target.subdomain)
    );
    match client
        .get(&url)
        .header("X-Share-Router-Probe", "1")
        .header("X-Portr-Probe", "1")
        .send()
        .await
    {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

async fn resolve_server_geo() -> ServerGeo {
    let client = match reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1")
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            return ServerGeo {
                lat: None,
                lon: None,
            };
        }
    };

    if let Some(geo) = resolve_server_geo_from_json(&client).await {
        return geo;
    }
    if let Some(geo) = resolve_server_geo_from_ip_im(&client).await {
        return geo;
    }
    ServerGeo {
        lat: None,
        lon: None,
    }
}

#[derive(serde::Deserialize)]
struct JsonServerGeoResponse {
    latitude: Option<f64>,
    longitude: Option<f64>,
}

async fn resolve_server_geo_from_json(client: &reqwest::Client) -> Option<ServerGeo> {
    let response = client.get("http://3.0.3.0/ips").send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload: JsonServerGeoResponse = response.json().await.ok()?;
    Some(ServerGeo {
        lat: payload.latitude,
        lon: payload.longitude,
    })
    .filter(|geo| geo.lat.is_some() && geo.lon.is_some())
}

async fn resolve_server_geo_from_ip_im(client: &reqwest::Client) -> Option<ServerGeo> {
    let response = client.get("https://ip.im/info").send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().await.ok()?;
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if let Some(value) = line.strip_prefix("Loc:") {
            if let Some((lat, lon)) = value.trim().split_once(',') {
                return Some(ServerGeo {
                    lat: lat.trim().parse().ok(),
                    lon: lon.trim().parse().ok(),
                });
            }
        }
    }
    None
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
        other => anyhow::bail!("unknown command: {other}\n\nRun `{APP_NAME} help` for usage."),
    }
}

fn print_help() {
    println!(
        "\
cc-switch-router

Usage:
  cc-switch-router
  cc-switch-router help
  cc-switch-router --help
  cc-switch-router -h

Environment:
  CC_SWITCH_ROUTER_API_ADDR              HTTP listen address, default 0.0.0.0:8787
  CC_SWITCH_ROUTER_SSH_ADDR              SSH listen address, default 0.0.0.0:2222
  CC_SWITCH_ROUTER_TUNNEL_DOMAIN         Public tunnel domain, default 0.0.0.0:8787
  CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR       SSH address sent to clients, default TUNNEL_DOMAIN:SSH_PORT
  CC_SWITCH_ROUTER_USE_LOCALHOST         Use http for localhost-style domains, default true
  CC_SWITCH_ROUTER_LEASE_TTL_SECS        Tunnel lease ttl, default 60
  CC_SWITCH_ROUTER_DB_PATH               SQLite path, default $HOME/.config/cc-switch-router/cc-switch-router.db
  CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS Cleanup interval, default 300
  CC_SWITCH_ROUTER_LEASE_RETENTION_SECS  Lease retention period, default 604800
  CC_SWITCH_ROUTER_CLIENT_STALE_SECS     Delete clients and shares after no report, default 3600

Legacy compatibility:
  `PORTR_RS_*` and `$HOME/.config/portr-rs/.env` are still accepted during migration.

Default env file:
  $HOME/.config/cc-switch-router/.env
  The file is auto-created on first start when missing.
"
    );
}

#[cfg(test)]
mod tests {
    use super::filter_registered_route_targets;
    use crate::store::ShareRouteTarget;

    #[test]
    fn filter_registered_route_targets_only_keeps_active_subdomains() {
        let filtered = filter_registered_route_targets(
            vec![
                ShareRouteTarget {
                    share_id: "share-1".into(),
                    subdomain: "aaa".into(),
                },
                ShareRouteTarget {
                    share_id: "share-2".into(),
                    subdomain: "bbb".into(),
                },
            ],
            vec!["bbb".into()],
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].share_id, "share-2");
        assert_eq!(filtered[0].subdomain, "bbb");
    }
}
