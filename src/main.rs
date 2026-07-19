mod abuse;
mod admin;
mod api;
mod board_telegram;
mod cf;
mod client_chat;
mod client_meta;
mod config;
mod ctl_client;
mod dynamic_settings;
mod error;
mod geo;
mod ingress_context;
mod ip_blacklist_stats;
mod metrics;
mod models;
mod namespace;
mod notifications;
mod proxy;
mod public_hosts;
mod recent_traffic;
mod registration_admission;
mod scheduling_signals;
mod server_state;
mod ssh;
mod startup_config;
mod store;

use std::collections::HashSet;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use anyhow::Result;
use proxy::{ProxyRegistry, RouteAvailability};
use resend_rs::Resend;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock, broadcast, watch};
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use uuid::Uuid;

use crate::abuse::AbuseTracker;
use crate::board_telegram::TelegramNotifier;
use crate::config::{Config, ensure_default_env_file, load_env_file};
use crate::dynamic_settings::DynamicSettings;
use crate::ip_blacklist_stats::{IpBlacklistStats, format_top_counts};
use crate::metrics::MetricsRegistry;
use crate::models::ShareRuntimeSnapshotResponse;
use crate::recent_traffic::RecentTraffic;
use crate::registration_admission::RegistrationAdmissionLimiter;
use crate::scheduling_signals::OverrideStore;
use crate::startup_config::{StartupConfigMode, ensure_startup_config};
use crate::store::{
    AppStore, ClientTunnelRouteTarget, RouteHealthStatus, RouteIntentKind, ShareRouteTarget,
    fetch_share_runtime_snapshot_from_route,
};

pub use crate::server_state::{ResendUsageCache, ServerGeo, ServerState};

const APP_NAME: &str = "cc-switch-router";
const HTTP_SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const SSH_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    if try_handle_cli()? {
        return Ok(());
    }

    let env_path = ensure_default_env_file()?;
    load_env_file(&env_path)?;
    ensure_startup_config(&env_path, StartupConfigMode::Start)?;

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
        client_installation_retention_secs = config.client_installation_retention_secs,
        paused_share_stale_secs = config.paused_share_stale_secs,
        client_email_notifications_enabled = config.client_notifications.enabled,
        client_notification_recipient_mode = "owner_email",
        client_offline_alert_secs = config.client_notifications.offline_alert_secs,
        db_exists = config.db_path.exists(),
        host_key_path = %config.host_key_path.display(),
        host_key_exists = config.host_key_path.exists(),
        env_exists = env_path.exists(),
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
    let telegram = TelegramNotifier::from_config(&config);
    let default_admin_email = config.default_admin_email();
    info!(
        admin_emails = config.admin_emails.len(),
        default_admin = default_admin_email.as_deref().unwrap_or("-"),
        telegram_enabled = telegram.is_some(),
        "legacy board compatibility configured"
    );
    if let Some(ref fp) = ssh_host_fingerprint {
        info!("ssh host key fingerprint: {}", fp);
    }
    info!("router dashboard branding enabled: Switch Router logo + favicon");
    let proxy_http = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 proxy")
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(64)
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .context("build proxy http client failed")?;
    let metrics = MetricsRegistry::new(config.metrics.clone());

    let state = ServerState {
        config: config.clone(),
        server_geo: server_geo.clone(),
        store: AppStore::new(&config)?,
        proxy: Arc::new(ProxyRegistry::default()),
        proxy_http,
        resend,
        resend_usage_cache: Arc::new(Mutex::new(None)),
        dynamic: Arc::new(RwLock::new(DynamicSettings::from_config(&config))),
        ssh_host_fingerprint: ssh_host_fingerprint.clone(),
        recent_traffic: RecentTraffic::new(),
        abuse: Arc::new(AbuseTracker::new()),
        ip_blacklist_stats: Arc::new(IpBlacklistStats::new()),
        telegram: Arc::new(RwLock::new(telegram)),
        upgrade_registry: Arc::new(crate::admin::upgrade::UpgradeRegistry::new()),
        share_edit_events: broadcast::channel(512).0,
        env_path: env_path.clone(),
        start_instant: Instant::now(),
        scheduling_overrides: OverrideStore::new(),
        metrics: metrics.clone(),
        payout_profile_read_limiter: Arc::new(
            crate::server_state::PublicPayoutProfileReadLimiter::default(),
        ),
        registration_admission: Arc::new(RegistrationAdmissionLimiter::from_env()),
    };
    let startup_reconnect_grace =
        crate::notifications::route_reconnect_grace(&config.client_notifications);

    let route_intents = state.store.list_route_intents().await?;
    let share_route_count = route_intents
        .iter()
        .filter(|intent| intent.kind == RouteIntentKind::Share)
        .count();
    let client_route_count = route_intents
        .iter()
        .filter(|intent| intent.kind == RouteIntentKind::Client)
        .count();
    let market_route_count = route_intents
        .iter()
        .filter(|intent| intent.kind == RouteIntentKind::Market)
        .count();
    state
        .proxy
        .declare_known_routes(
            route_intents
                .iter()
                .map(|intent| intent.subdomain.trim().to_ascii_lowercase()),
        )
        .await;
    info!(
        total = route_intents.len(),
        shares = share_route_count,
        clients = client_route_count,
        markets = market_route_count,
        "restored known route intentions"
    );

    let ssh_server = ssh::SshServer {
        store: state.store.clone(),
        proxy: state.proxy.clone(),
        host_key: ssh_host_key,
        metrics: state.metrics.clone(),
    };
    let cleanup_store = state.store.clone();
    let cleanup_config = config.clone();
    let cleanup_dynamic = state.dynamic.clone();
    let cleanup_proxy = state.proxy.clone();
    let cleanup_overrides = state.scheduling_overrides.clone();
    let ip_blacklist_stats = state.ip_blacklist_stats.clone();
    let probe_store = state.store.clone();
    let probe_proxy = state.proxy.clone();
    let probe_config = config.clone();
    let probe_dynamic = state.dynamic.clone();
    let router_epoch = Uuid::new_v4().to_string();
    let runtime_store = state.store.clone();
    let runtime_proxy = state.proxy.clone();
    let runtime_config = config.clone();
    let runtime_traffic = state.recent_traffic.clone();
    let resend_usage_cache = state.resend_usage_cache.clone();
    let resend_usage_api_key = config.resend_api_key.clone();
    let metrics_config = config.clone();
    let metrics_proxy = state.proxy.clone();
    let metrics_registry = state.metrics.clone();
    let notification_store = state.store.clone();
    let notification_dynamic = state.dynamic.clone();
    let notification_config = config.clone();
    let chat_notification_store = state.store.clone();
    let chat_notification_config = config.clone();

    let http_listener = TcpListener::bind(config.api_addr).await?;
    let ssh_listener = TcpListener::bind(config.ssh_addr).await?;
    info!("http listening on {}", config.api_addr);
    info!("ssh listener bound on {}", config.ssh_addr);

    let ip_blacklist_log_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(600)).await;
            if let Some(summary) = ip_blacklist_stats.flush() {
                tracing::warn!(
                    blocked = summary.blocked,
                    unique_ips = summary.unique_ips,
                    window_secs = summary.window_secs,
                    top_ips = %format_top_counts(&summary.top_ips),
                    top_paths = %format_top_counts(&summary.top_paths),
                    "IP blacklist summary"
                );
            }
        }
    });

    let cleanup_task = tokio::spawn(async move {
        tokio::time::sleep(startup_reconnect_grace).await;
        let mut interval =
            tokio::time::interval(Duration::from_secs(cleanup_config.cleanup_interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            cleanup_overrides.cleanup_expired();
            let mut cycle_config = cleanup_config.clone();
            cycle_config.client_notifications =
                cleanup_dynamic.read().await.client_notifications.clone();
            match cleanup_store
                .cleanup_expired_data(&cycle_config, &cleanup_proxy)
                .await
            {
                Ok(result) if result.has_changes() => {
                    info!(
                        leases = result.deleted_leases,
                        shares = result.deleted_shares,
                        installations = result.deleted_installations,
                        notification_batches = result.deleted_notification_batches,
                        notification_events = result.deleted_notification_events,
                        notification_send_logs = result.deleted_notification_send_logs,
                        chat_rooms = result.deleted_chat_rooms,
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
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let reconnect_grace = crate::notifications::route_reconnect_grace(
                &probe_dynamic.read().await.client_notifications,
            );
            if let Err(err) = run_route_health_probe_cycle(
                &probe_store,
                &probe_proxy,
                &probe_config,
                &client,
                reconnect_grace,
                &router_epoch,
            )
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
                &runtime_traffic,
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
    let metrics_task = tokio::spawn(async move {
        crate::metrics::run_collector(metrics_registry, metrics_config, metrics_proxy).await;
        Ok::<_, anyhow::Error>(())
    });
    let notification_task = tokio::spawn(async move {
        let result = crate::notifications::run_client_notification_service(
            notification_store,
            notification_dynamic,
            notification_config,
            startup_reconnect_grace,
        )
        .await;
        if let Err(error) = &result {
            tracing::error!(error = %error, "client notification service stopped");
        }
        result
    });
    let chat_notification_task = tokio::spawn(async move {
        let result = crate::client_chat::run_client_chat_email_service(
            chat_notification_store,
            chat_notification_config,
        )
        .await;
        if let Err(error) = &result {
            tracing::error!(error = %error, "client chat email service stopped");
        }
        result
    });
    let (http_shutdown_tx, http_shutdown_rx) = watch::channel(false);
    let (ssh_shutdown_tx, ssh_shutdown_rx) = watch::channel(false);
    let mut ssh_task = tokio::spawn(async move {
        ssh_server
            .run_with_listener(ssh_listener, ssh_shutdown_rx)
            .await
    });
    let mut http_task = tokio::spawn(async move {
        axum::serve(
            http_listener,
            api::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(wait_for_shutdown(http_shutdown_rx))
        .await?;
        Ok::<_, anyhow::Error>(())
    });

    enum ServiceExit {
        Signal(&'static str),
        Http(Result<Result<()>, tokio::task::JoinError>),
        Ssh(Result<Result<()>, tokio::task::JoinError>),
    }

    let exit = tokio::select! {
        signal = shutdown_signal() => ServiceExit::Signal(signal?),
        result = &mut ssh_task => ServiceExit::Ssh(result),
        result = &mut http_task => ServiceExit::Http(result),
    };

    let service_result = match exit {
        ServiceExit::Signal(signal) => {
            info!(signal, "graceful shutdown started");
            let _ = http_shutdown_tx.send(true);
            let http_result =
                stop_service_task("http", &mut http_task, HTTP_SHUTDOWN_DRAIN_TIMEOUT).await;
            let _ = ssh_shutdown_tx.send(true);
            let ssh_result = stop_service_task("ssh", &mut ssh_task, SSH_SHUTDOWN_TIMEOUT).await;
            info!("graceful shutdown completed");
            combine_service_results(http_result, ssh_result)
        }
        ServiceExit::Http(result) => {
            let _ = ssh_shutdown_tx.send(true);
            let http_result = service_task_result("http", result);
            let ssh_result = stop_service_task("ssh", &mut ssh_task, SSH_SHUTDOWN_TIMEOUT).await;
            combine_service_results(http_result, ssh_result)
        }
        ServiceExit::Ssh(result) => {
            let _ = http_shutdown_tx.send(true);
            let ssh_result = service_task_result("ssh", result);
            let http_result =
                stop_service_task("http", &mut http_task, HTTP_SHUTDOWN_DRAIN_TIMEOUT).await;
            combine_service_results(ssh_result, http_result)
        }
    };

    cleanup_task.abort();
    ip_blacklist_log_task.abort();
    probe_task.abort();
    runtime_task.abort();
    resend_usage_task.abort();
    metrics_task.abort();
    notification_task.abort();
    chat_notification_task.abort();
    service_result
}

fn service_task_result(
    service: &str,
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    match result {
        Ok(result) => result.with_context(|| format!("{service} service stopped with an error")),
        Err(error) => {
            Err(anyhow::Error::new(error)).with_context(|| format!("{service} service task failed"))
        }
    }
}

async fn stop_service_task(
    service: &str,
    task: &mut tokio::task::JoinHandle<Result<()>>,
    deadline: Duration,
) -> Result<()> {
    match tokio::time::timeout(deadline, &mut *task).await {
        Ok(result) => service_task_result(service, result),
        Err(_) => {
            tracing::warn!(
                service,
                timeout_secs = deadline.as_secs(),
                "service shutdown deadline reached"
            );
            task.abort();
            let _ = task.await;
            Ok(())
        }
    }
}

fn combine_service_results(primary: Result<()>, secondary: Result<()>) -> Result<()> {
    if let Err(error) = secondary {
        tracing::error!(error = %error, "secondary service shutdown failed");
        if primary.is_ok() {
            return Err(error);
        }
    }
    primary
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

async fn shutdown_signal() -> Result<&'static str> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("install SIGTERM handler failed")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.context("install Ctrl-C handler failed")?;
                Ok("ctrl-c")
            }
            _ = terminate.recv() => Ok("sigterm"),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .context("install Ctrl-C handler failed")?;
        Ok("ctrl-c")
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
    reconnect_grace: Duration,
    router_epoch: &str,
) -> Result<()> {
    let targets = store.list_share_route_targets().await?;
    for target in targets {
        let (status, reason) = match proxy
            .route_availability(&target.subdomain, reconnect_grace)
            .await
            .map(|snapshot| snapshot.state)
        {
            Some(RouteAvailability::Active) => {
                route_probe_observation(probe_share_route(store, config, client, &target).await)
            }
            Some(RouteAvailability::Reconnecting) => {
                (RouteHealthStatus::Unknown, "route_reconnecting")
            }
            Some(RouteAvailability::Offline) => (RouteHealthStatus::Unhealthy, "route_offline"),
            None => (RouteHealthStatus::Unknown, "route_not_hydrated"),
        };
        if let Err(err) = store
            .record_share_route_health(&target.share_id, status, reason, router_epoch)
            .await
        {
            tracing::warn!(share_id = %target.share_id, "record route health failed: {err}");
        }
    }
    let client_targets = store.list_client_tunnel_route_targets().await?;
    for target in client_targets {
        let (status, reason) = match proxy
            .route_availability(&target.subdomain, reconnect_grace)
            .await
            .map(|snapshot| snapshot.state)
        {
            Some(RouteAvailability::Active) => route_probe_observation(
                probe_client_tunnel_route(store, config, client, &target).await,
            ),
            Some(RouteAvailability::Reconnecting) => {
                (RouteHealthStatus::Unknown, "route_reconnecting")
            }
            Some(RouteAvailability::Offline) => (RouteHealthStatus::Unhealthy, "route_offline"),
            None => (RouteHealthStatus::Unknown, "route_not_hydrated"),
        };
        if let Err(err) = store
            .record_installation_route_health(&target.installation_id, status, reason, router_epoch)
            .await
        {
            tracing::warn!(
                installation_id = %target.installation_id,
                "record client tunnel route health failed: {err}"
            );
        }
    }
    Ok(())
}

async fn run_share_runtime_refresh_cycle(
    store: &AppStore,
    proxy: &ProxyRegistry,
    config: &Config,
    recent_traffic: &RecentTraffic,
    client: &reqwest::Client,
) -> Result<()> {
    let targets = filter_registered_route_targets(
        store.list_share_route_targets().await?,
        proxy.active_subdomains().await,
    );
    for target in targets {
        match fetch_share_runtime_snapshot_from_route(
            store,
            config,
            client,
            &target.subdomain,
            &target.share_id,
            &target.installation_id,
        )
        .await
        {
            Ok(snapshot) => {
                record_runtime_model_health_traffic(recent_traffic, &target, &snapshot).await;
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

async fn record_runtime_model_health_traffic(
    recent_traffic: &RecentTraffic,
    target: &ShareRouteTarget,
    snapshot: &ShareRuntimeSnapshotResponse,
) {
    for summary in snapshot
        .model_health
        .claude
        .iter()
        .chain(snapshot.model_health.codex.iter())
        .chain(snapshot.model_health.gemini.iter())
    {
        let checked_at = summary.last_checked_at.unwrap_or(snapshot.queried_at);
        let model = if summary.actual_model.trim().is_empty() {
            summary.requested_model.clone()
        } else {
            summary.actual_model.clone()
        };
        let request_id = format!(
            "cc-switch-health:{}:{}:{}:{}",
            snapshot.share_id, summary.app_type, model, checked_at
        );
        recent_traffic
            .record_health_check(
                request_id,
                target.share_id.clone(),
                Some(target.share_name.clone()),
                Some(target.subdomain.clone()),
                summary.status.clone(),
                summary.app_type.clone(),
                model,
            )
            .await;
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TunnelRouteProbe {
    Healthy,
    Unhealthy,
    Unavailable,
}

#[cfg(test)]
fn active_route_is_healthy(probe: TunnelRouteProbe) -> bool {
    !matches!(probe, TunnelRouteProbe::Unhealthy)
}

fn route_probe_observation(probe: TunnelRouteProbe) -> (RouteHealthStatus, &'static str) {
    match probe {
        TunnelRouteProbe::Healthy => (RouteHealthStatus::Healthy, "probe_succeeded"),
        TunnelRouteProbe::Unhealthy => (RouteHealthStatus::Unhealthy, "probe_failed"),
        TunnelRouteProbe::Unavailable => (RouteHealthStatus::Healthy, "active_route_unprobeable"),
    }
}

async fn probe_share_route(
    store: &AppStore,
    config: &Config,
    client: &reqwest::Client,
    target: &ShareRouteTarget,
) -> TunnelRouteProbe {
    probe_tunnel_route_health(
        store,
        config,
        client,
        &target.subdomain,
        &target.installation_id,
    )
    .await
}

async fn probe_client_tunnel_route(
    store: &AppStore,
    config: &Config,
    client: &reqwest::Client,
    target: &ClientTunnelRouteTarget,
) -> TunnelRouteProbe {
    probe_tunnel_route_health(
        store,
        config,
        client,
        &target.subdomain,
        &target.installation_id,
    )
    .await
}

async fn probe_tunnel_route_health(
    store: &AppStore,
    config: &Config,
    client: &reqwest::Client,
    subdomain: &str,
    installation_id: &str,
) -> TunnelRouteProbe {
    const PATH: &str = "/_share-router/health";
    let control_secret = match store.installation_control_secret(installation_id).await {
        Ok(Some(secret)) if !secret.trim().is_empty() => secret,
        Ok(_) => return TunnelRouteProbe::Unavailable,
        Err(err) => {
            tracing::warn!(
                installation_id,
                subdomain,
                "read control secret for route health probe failed: {err}"
            );
            return TunnelRouteProbe::Unhealthy;
        }
    };
    let url = format!("{}{PATH}", config.tunnel_url(subdomain));
    let request = crate::ctl_client::authorize_control_request(
        client.get(&url).header("X-Share-Router-Probe", "1"),
        "GET",
        PATH,
        installation_id,
        &control_secret,
        &[],
    );
    match request.send().await {
        Ok(response) if response.status().is_success() => TunnelRouteProbe::Healthy,
        Ok(_) | Err(_) => TunnelRouteProbe::Unhealthy,
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
        "setup" => {
            let env_path = ensure_default_env_file()?;
            load_env_file(&env_path)?;
            ensure_startup_config(&env_path, StartupConfigMode::SetupOnly)?;
            Ok(true)
        }
        "check-config" => {
            let env_path = ensure_default_env_file()?;
            load_env_file(&env_path)?;
            ensure_startup_config(&env_path, StartupConfigMode::CheckOnly)?;
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
  cc-switch-router setup
  cc-switch-router check-config
  cc-switch-router help
  cc-switch-router --help
  cc-switch-router -h

Environment:
  CC_SWITCH_ROUTER_API_ADDR              HTTP listen address, default 0.0.0.0:80
  CC_SWITCH_ROUTER_SSH_ADDR              SSH listen address, default 0.0.0.0:2222
  CC_SWITCH_ROUTER_TUNNEL_DOMAIN         Public tunnel domain, required
  CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR       SSH address sent to clients, required
  CC_SWITCH_ROUTER_RESEND_API_KEY        Resend API key for email login, required
  CC_SWITCH_ROUTER_RESEND_FROM           Sender email, default noreply@[TUNNEL_DOMAIN]
  CC_SWITCH_ROUTER_USE_LOCALHOST         Use http for localhost-style domains, default false
  CC_SWITCH_ROUTER_LEASE_TTL_SECS        Tunnel lease ttl, default 60
  CC_SWITCH_ROUTER_DB_PATH               SQLite path, default $HOME/.cc-switch-router/cc-switch-router.db
  CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS Cleanup interval, default 300
  CC_SWITCH_ROUTER_LEASE_RETENTION_SECS  Lease retention period, default 86400
  CC_SWITCH_ROUTER_CLIENT_STALE_SECS     Mark clients offline and purge shares after no heartbeat, default 3600
  CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS Delete installation records after offline retention, default 21600
  CC_SWITCH_ROUTER_PAUSED_SHARE_STALE_SECS Delete paused shares after no update, default 3600
Default env file:
  $HOME/.cc-switch-router/.env
  The file is auto-created on first start when missing.
"
    );
}

#[cfg(test)]
mod tests {
    use super::{TunnelRouteProbe, active_route_is_healthy, filter_registered_route_targets};
    use crate::store::ShareRouteTarget;

    #[test]
    fn filter_registered_route_targets_only_keeps_active_subdomains() {
        let filtered = filter_registered_route_targets(
            vec![
                ShareRouteTarget {
                    share_id: "share-1".into(),
                    installation_id: "inst-1".into(),
                    share_name: "Share 1".into(),
                    subdomain: "aaa".into(),
                    app_runtimes: Default::default(),
                },
                ShareRouteTarget {
                    share_id: "share-2".into(),
                    installation_id: "inst-2".into(),
                    share_name: "Share 2".into(),
                    subdomain: "bbb".into(),
                    app_runtimes: Default::default(),
                },
            ],
            vec!["bbb".into()],
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].share_id, "share-2");
        assert_eq!(filtered[0].subdomain, "bbb");
    }

    #[test]
    fn active_route_is_available_when_legacy_control_secret_is_missing() {
        assert!(active_route_is_healthy(TunnelRouteProbe::Healthy));
        assert!(active_route_is_healthy(TunnelRouteProbe::Unavailable));
        assert!(!active_route_is_healthy(TunnelRouteProbe::Unhealthy));
    }
}
