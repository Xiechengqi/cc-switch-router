mod abuse;
mod admin;
mod alerting;
mod api;
mod cf;
mod client_chat;
mod client_logs;
mod client_market;
mod client_market_coordination;
mod client_market_terminal;
mod client_market_trade;
mod client_meta;
mod client_subdomain_takeover;
mod clock_health;
mod config;
mod ctl_client;
mod db;
mod dynamic_settings;
mod embed_usage;
mod error;
mod geo;
mod ingress_context;
mod ip_blacklist_stats;
mod ip_iq;
mod market_access;
mod market_billing;
mod metrics;
mod models;
mod namespace;
mod notification_channels;
mod notifications;
mod process_lock;
mod provision_ssh;
mod proxy;
mod proxy_stream;
mod public_hosts;
mod recent_traffic;
mod registration_admission;
mod scheduling_signals;
mod schema;
mod secure_file;
mod server_logs;
mod server_state;
mod share_market;
mod ssh;
mod startup_config;
mod store;
mod telegram;
mod usage_account;
mod user_notification_health;

use std::collections::HashSet;
use std::env;
use std::future::Future;
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
use crate::client_market::ClientMarketJobSecrets;
use crate::config::{Config, DatabaseMode, ensure_default_env_file, load_env_file};
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
const SSH_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const BACKGROUND_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

#[tokio::main]
async fn main() -> Result<()> {
    if try_handle_cli().await? {
        return Ok(());
    }

    let env_path = ensure_default_env_file()?;
    load_env_file(&env_path)?;
    ensure_startup_config(&env_path, StartupConfigMode::Start)?;
    let settings_runtime = crate::admin::settings::SettingsRuntimeSnapshot::capture(&env_path)?;

    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let config = Config::from_env();
    validate_runtime_config(&config)?;
    let _process_lock = crate::process_lock::ProcessLock::acquire(&config.data_dir)?;
    let server_geo = resolve_server_geo().await;
    info!(
        api_addr = %config.api_addr,
        ssh_addr = %config.ssh_addr,
        tunnel_domain = %config.tunnel_domain,
        router_owner_email = config.official_provider_email().unwrap_or("-"),
        ssh_public_addr = %config.effective_ssh_public_addr(),
        ssh_inactivity_timeout_secs = config.ssh_transport.inactivity_timeout_secs,
        ssh_keepalive_interval_secs = config.ssh_transport.keepalive_interval_secs,
        ssh_keepalive_max = config.ssh_transport.keepalive_max,
        ssh_channel_open_timeout_secs = config.ssh_transport.channel_open_timeout_secs,
        ssh_bridge_write_stall_timeout_secs = config.ssh_transport.bridge_write_stall_timeout_secs,
        ssh_bridge_half_close_idle_timeout_secs = config.ssh_transport.bridge_half_close_idle_timeout_secs,
        ssh_max_forward_connections = config.ssh_transport.max_forward_connections,
        ssh_max_forward_connections_per_tunnel = config
            .ssh_transport
            .max_forward_connections_per_tunnel,
        proxy_request_body_timeout_secs = config.proxy_stream.request_body_timeout_secs,
        proxy_response_header_timeout_secs = config.proxy_stream.response_header_timeout_secs,
        proxy_stream_first_event_timeout_secs = config.proxy_stream.first_event_timeout_secs,
        proxy_stream_idle_timeout_secs = config.proxy_stream.idle_timeout_secs,
        proxy_downstream_stall_timeout_secs = config.proxy_stream.downstream_stall_timeout_secs,
        proxy_max_request_lifetime_secs = config.proxy_stream.max_request_lifetime_secs,
        proxy_request_body_limit_mb = config.proxy_stream.request_body_limit_mb,
        proxy_media_request_body_limit_mb = config.proxy_stream.media_request_body_limit_mb,
        proxy_image_request_body_limit_mb = config.proxy_stream.image_request_body_limit_mb,
        server_label = "server",
        server_lat = server_geo.lat,
        server_lon = server_geo.lon,
        data_dir = %config.data_dir.display(),
        db_mode = config.database.mode.as_str(),
        db_path = %config.database.path.display(),
        turso_url = config.database.turso_url.as_deref().unwrap_or("-"),
        db_sync_interval_secs = config.database.sync_interval_secs,
        env_path = %env_path.display(),
        use_localhost = config.use_localhost,
        cleanup_interval_secs = config.cleanup_interval_secs,
        lease_retention_secs = config.lease_retention_secs,
        request_log_retention_days = config.request_log_retention_days,
        client_stale_secs = config.client_stale_secs,
        client_installation_retention_secs = config.client_installation_retention_secs,
        paused_share_stale_secs = config.paused_share_stale_secs,
        client_email_notifications_enabled = config.client_notifications.enabled,
        client_notification_recipient_mode = "owner_email",
        client_offline_alert_secs = config.client_notifications.offline_alert_secs,
        db_exists = config.database.path.exists(),
        host_key_path = %config.host_key_path.display(),
        host_key_exists = config.host_key_path.exists(),
        provision_ssh_private_key_path = %config.provision_ssh_private_key_path.display(),
        provision_ssh_public_key_path = %config.provision_ssh_public_key_path.display(),
        env_exists = env_path.exists(),
        "starting cc-switch-router"
    );
    // 预加载 SSH host key 并计算指纹，提前失败在配置错误；也作为 lease 响应返回给客户端。
    let ssh_host_key = ssh::load_or_generate_host_key(&config.host_key_path)?;
    let ssh_host_fingerprint = ssh::host_key_fingerprint(&ssh_host_key).ok();
    // Load or generate the dedicated outbound Client Market SSH keypair.
    provision_ssh::require_provision_ssh_keys(
        &config.provision_ssh_private_key_path,
        &config.provision_ssh_public_key_path,
    )?;
    let provision_ssh_authorized_keys_line = provision_ssh::authorized_keys_line_from_public_path(
        &config.provision_ssh_public_key_path,
        "cc-switch-router-provision",
    )?;
    let provision_ssh_public_key =
        provision_ssh::public_key_openssh_from_public_path(&config.provision_ssh_public_key_path)?;
    ip_iq::warn_insecure_endpoints(&config.ip_intel_endpoints);
    let resend = config
        .resend_api_key
        .as_deref()
        .map(Resend::new)
        .map(Arc::new);
    let default_admin_email = config.default_admin_email();
    info!(
        admin_emails = config.admin_emails.len(),
        default_admin = default_admin_email.as_deref().unwrap_or("-"),
        "router administration configured"
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
    let server_logs = Arc::new(server_logs::ServerLogStore::from_env(&config.data_dir)?);
    server_logs.spawn_maintenance();
    let clock_health = crate::clock_health::ClockHealthService::new(config.clock_health.clone())?;
    let dynamic = Arc::new(RwLock::new(DynamicSettings::from_config(&config)));
    let alerting = crate::alerting::AlertingService::new(
        config.metrics.db_path.clone(),
        dynamic.clone(),
        &config,
    )?;
    let state = ServerState {
        config: config.clone(),
        server_geo: server_geo.clone(),
        store: AppStore::new(&config)?,
        server_logs,
        client_logs: Arc::new(crate::client_logs::ClientLogAccessLimiter::default()),
        proxy: Arc::new(ProxyRegistry::default()),
        proxy_http,
        resend,
        resend_usage_cache: Arc::new(Mutex::new(None)),
        dynamic,
        ssh_host_fingerprint: ssh_host_fingerprint.clone(),
        provision_ssh_key_path: config.provision_ssh_private_key_path.clone(),
        provision_ssh_authorized_keys_line,
        provision_ssh_public_key,
        client_market_job_secrets: Arc::new(Mutex::new(ClientMarketJobSecrets::default())),
        client_market_terminal: Arc::new(Mutex::new(
            crate::client_market_terminal::TerminalSessionManager::default(),
        )),
        client_market_actions: Arc::new(
            crate::client_market_coordination::ClientMarketActionLocks::default(),
        ),
        client_subdomain_takeover_recovery_running: Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        )),
        market_billing_controls: Arc::new(Mutex::new(())),
        recent_traffic: RecentTraffic::new(),
        abuse: Arc::new(AbuseTracker::new()),
        ip_blacklist_stats: Arc::new(IpBlacklistStats::new()),
        upgrade_registry: Arc::new(crate::admin::upgrade::UpgradeRegistry::new()),
        share_edit_events: broadcast::channel(512).0,
        env_path: env_path.clone(),
        settings_runtime,
        start_instant: Instant::now(),
        scheduling_overrides: OverrideStore::new(),
        metrics: metrics.clone(),
        clock_health: clock_health.clone(),
        alerting: alerting.clone(),
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
        "restored known route intentions"
    );
    crate::client_market::reconcile_interrupted_jobs(state.clone()).await?;
    crate::client_subdomain_takeover::spawn_recovery(state.clone());

    let ssh_server = ssh::SshServer {
        store: state.store.clone(),
        proxy: state.proxy.clone(),
        host_key: ssh_host_key,
        metrics: state.metrics.clone(),
        transport: config.ssh_transport.clone(),
    };
    let cleanup_store = state.store.clone();
    let cleanup_config = config.clone();
    let cleanup_dynamic = state.dynamic.clone();
    let cleanup_proxy = state.proxy.clone();
    let cleanup_overrides = state.scheduling_overrides.clone();
    let market_reconcile_state = state.clone();
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
    let request_log_recovery_store = state.store.clone();
    let request_log_recovery_proxy = state.proxy.clone();
    let request_log_recovery_config = config.clone();
    let resend_usage_cache = state.resend_usage_cache.clone();
    let resend_usage_api_key = config.resend_api_key.clone();
    let metrics_config = config.clone();
    let metrics_proxy = state.proxy.clone();
    let metrics_registry = state.metrics.clone();
    let metrics_store = state.store.clone();
    let metrics_alerting = state.alerting.clone();
    let share_request_watchdog_proxy = state.proxy.clone();
    let share_request_watchdog_config = config.proxy_stream.clone();
    let share_request_watchdog_metrics = state.metrics.clone();
    let clock_metrics = state.metrics.clone();
    let clock_alerting = state.alerting.clone();
    let alerting_service = state.alerting.clone();
    let alerting_store = state.store.clone();
    let notification_store = state.store.clone();
    let notification_dynamic = state.dynamic.clone();
    let notification_config = config.clone();
    let telegram_bot_store = state.store.clone();
    let telegram_bot_dynamic = state.dynamic.clone();
    let telegram_bot_config = config.clone();
    let chat_notification_store = state.store.clone();
    let chat_notification_config = config.clone();
    let client_market_trade_state = state.clone();
    let share_market_state = state.clone();
    let market_billing_state = state.clone();
    let database_sync_store = state.store.clone();
    let shutdown_database_sync_store = state.store.clone();
    let database_sync_interval_secs = config.database.sync_interval_secs;
    let database_sync_enabled = config.database.mode == DatabaseMode::Turso;
    let shutdown_ip_blacklist_stats = state.ip_blacklist_stats.clone();

    let http_listener = TcpListener::bind(config.api_addr).await?;
    let ssh_listener = TcpListener::bind(config.ssh_addr).await?;
    info!("http listening on {}", config.api_addr);
    info!("ssh listener bound on {}", config.ssh_addr);

    let (background_shutdown_tx, background_shutdown_rx) = watch::channel(false);

    let ip_blacklist_log_task = spawn_background_task(
        "IP blacklist logger",
        background_shutdown_rx.clone(),
        async move {
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
            #[allow(unreachable_code)]
            Ok(())
        },
    );

    let database_sync_task = database_sync_enabled.then(|| {
        spawn_background_task(
            "database sync",
            background_shutdown_rx.clone(),
            async move {
                let mut interval =
                    tokio::time::interval(Duration::from_secs(database_sync_interval_secs));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                interval.tick().await;
                loop {
                    interval.tick().await;
                    match database_sync_store.sync_database() {
                        Ok(health) => {
                            tracing::debug!(
                                frames_synced = health.last_frames_synced,
                                "Turso Embedded Replica synchronized"
                            );
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, "Turso Embedded Replica sync failed");
                        }
                    }
                }
                #[allow(unreachable_code)]
                Ok(())
            },
        )
    });

    let cleanup_task =
        spawn_background_task("cleanup", background_shutdown_rx.clone(), async move {
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
                if let Err(err) = crate::client_market::reconcile_stale_market_hosts(
                    market_reconcile_state.clone(),
                )
                .await
                {
                    tracing::warn!("client market host reconcile failed: {err}");
                }
            }
            #[allow(unreachable_code)]
            Ok(())
        });
    let share_request_watchdog_task = spawn_background_task(
        "Share request watchdog",
        background_shutdown_rx.clone(),
        async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                for released in share_request_watchdog_proxy
                    .release_stale_share_requests(&share_request_watchdog_config)
                {
                    share_request_watchdog_metrics
                        .record_share_request_watchdog_release(&released.reason);
                    tracing::warn!(
                        request_id = %released.request_id,
                        lease_id = %released.lease_id,
                        share_id = %released.share_id,
                        app = released.app.as_deref().unwrap_or("-"),
                        user_email = released.user_email.as_deref().unwrap_or("-"),
                        phase = %released.phase,
                        age_secs = released.age_secs,
                        progress_age_secs = released.progress_age_secs,
                        reason = %released.reason,
                        "Share request watchdog force-released a stale lease"
                    );
                }
            }
            #[allow(unreachable_code)]
            Ok::<_, anyhow::Error>(())
        },
    );
    let probe_task = spawn_background_task(
        "route health probe",
        background_shutdown_rx.clone(),
        async move {
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
        },
    );
    let runtime_task = spawn_background_task(
        "Share runtime refresh",
        background_shutdown_rx.clone(),
        async move {
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
        },
    );
    let request_log_recovery_task = spawn_background_task(
        "Share request log recovery",
        background_shutdown_rx.clone(),
        async move {
            let mut interval = tokio::time::interval(Duration::from_secs(15));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                match request_log_recovery_store
                    .recover_share_request_logs_cycle(
                        &request_log_recovery_config,
                        &request_log_recovery_proxy,
                    )
                    .await
                {
                    Ok(summary) if summary.recovered > 0 => info!(
                        shares = summary.shares_checked,
                        pages = summary.pages,
                        recovered = summary.recovered,
                        failed = summary.failed,
                        "Share request log recovery synchronized entries"
                    ),
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        error = %error,
                        "Share request log recovery cycle failed"
                    ),
                }
            }
            #[allow(unreachable_code)]
            Ok::<_, anyhow::Error>(())
        },
    );
    let resend_usage_task = spawn_background_task(
        "Resend usage refresh",
        background_shutdown_rx.clone(),
        async move {
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
                    Ok(Some(label)) => {
                        info!(resend_daily_usage = %label, "updated resend daily usage")
                    }
                    Ok(None) => info!("resend daily quota header missing, footer hidden"),
                    Err(err) => tracing::warn!("refresh resend usage failed: {err}"),
                }
            }
            #[allow(unreachable_code)]
            Ok::<_, anyhow::Error>(())
        },
    );
    let metrics_task = spawn_background_task(
        "metrics collector",
        background_shutdown_rx.clone(),
        async move {
            crate::metrics::run_collector(
                metrics_registry,
                metrics_config,
                metrics_proxy,
                metrics_store,
                metrics_alerting,
            )
            .await;
            Ok::<_, anyhow::Error>(())
        },
    );
    let clock_health_task =
        spawn_background_task("clock health", background_shutdown_rx.clone(), async move {
            crate::clock_health::run_clock_health_service(
                clock_health,
                clock_metrics,
                clock_alerting,
            )
            .await;
            Ok(())
        });
    let alerting_task = spawn_background_task(
        "operator alerting",
        background_shutdown_rx.clone(),
        async move {
            let result =
                crate::alerting::run_alerting_service(alerting_service, alerting_store).await;
            if let Err(error) = &result {
                tracing::error!(error = %error, "operator alert service stopped");
            }
            result
        },
    );
    let notification_task = spawn_background_task(
        "client notifications",
        background_shutdown_rx.clone(),
        async move {
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
        },
    );
    // Inbound half of the user-facing Telegram bot. Idles cheaply when the bot
    // is disabled, and picks up settings changes without a restart.
    let telegram_bot_task =
        spawn_background_task("telegram bot", background_shutdown_rx.clone(), async move {
            let result = crate::telegram::service::run_telegram_bot_service(
                telegram_bot_store,
                telegram_bot_dynamic,
                telegram_bot_config,
            )
            .await;
            if let Err(error) = &result {
                tracing::error!(error = %error, "telegram bot service stopped");
            }
            result
        });
    let chat_notification_task = spawn_background_task(
        "chat email notifications",
        background_shutdown_rx.clone(),
        async move {
            let result = crate::client_chat::run_client_chat_email_service(
                chat_notification_store,
                chat_notification_config,
            )
            .await;
            if let Err(error) = &result {
                tracing::error!(error = %error, "client chat email service stopped");
            }
            result
        },
    );
    let client_market_trade_task = spawn_background_task(
        "Client Market trade",
        background_shutdown_rx.clone(),
        async move {
            let result =
                crate::client_market_trade::run_trade_service(client_market_trade_state).await;
            if let Err(error) = &result {
                tracing::error!(error = %error, "Client Market trade service stopped");
            }
            result
        },
    );
    let share_market_task =
        spawn_background_task("Share Market", background_shutdown_rx.clone(), async move {
            let result = crate::share_market::run_service(share_market_state).await;
            if let Err(error) = &result {
                tracing::error!(error = %error, "Share Market service stopped");
            }
            result
        });
    let market_billing_task =
        spawn_background_task("Market billing", background_shutdown_rx, async move {
            let result = crate::market_billing::run_service(market_billing_state).await;
            if let Err(error) = &result {
                tracing::error!(error = %error, "Market billing service stopped");
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
    let _ = background_shutdown_tx.send(true);

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

    let mut background_tasks = vec![
        ("cleanup", cleanup_task),
        ("Share request watchdog", share_request_watchdog_task),
        ("IP blacklist logger", ip_blacklist_log_task),
        ("route health probe", probe_task),
        ("Share runtime refresh", runtime_task),
        ("Share request log recovery", request_log_recovery_task),
        ("Resend usage refresh", resend_usage_task),
        ("metrics collector", metrics_task),
        ("clock health", clock_health_task),
        ("operator alerting", alerting_task),
        ("client notifications", notification_task),
        ("telegram bot", telegram_bot_task),
        ("chat email notifications", chat_notification_task),
        ("Client Market trade", client_market_trade_task),
        ("Share Market", share_market_task),
        ("Market billing", market_billing_task),
    ];
    if let Some(task) = database_sync_task {
        background_tasks.push(("database sync", task));
    }
    let background_result =
        stop_background_tasks(background_tasks, BACKGROUND_SHUTDOWN_TIMEOUT).await;
    if database_sync_enabled && let Err(error) = shutdown_database_sync_store.sync_database() {
        tracing::warn!(error = %error, "final Turso Embedded Replica sync failed");
    }
    if let Some(summary) = shutdown_ip_blacklist_stats.flush() {
        tracing::warn!(
            blocked = summary.blocked,
            unique_ips = summary.unique_ips,
            window_secs = summary.window_secs,
            top_ips = %format_top_counts(&summary.top_ips),
            top_paths = %format_top_counts(&summary.top_paths),
            "final IP blacklist summary"
        );
    }
    combine_service_results(service_result, background_result)
}

fn spawn_background_task<F>(
    service: &'static str,
    shutdown: watch::Receiver<bool>,
    future: F,
) -> tokio::task::JoinHandle<Result<()>>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = wait_for_shutdown(shutdown) => {
                info!(service, "background service received shutdown");
                Ok(())
            }
            result = future => result.with_context(|| format!("{service} background service stopped")),
        }
    })
}

async fn stop_background_tasks(
    tasks: Vec<(&'static str, tokio::task::JoinHandle<Result<()>>)>,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut first_error = None;
    for (service, mut task) in tasks {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            tracing::warn!(service, "background shutdown deadline reached");
            task.abort();
            let _ = task.await;
            continue;
        }
        match tokio::time::timeout(remaining, &mut task).await {
            Ok(result) => {
                if let Err(error) = service_task_result(service, result)
                    && first_error.is_none()
                {
                    first_error = Some(error);
                }
            }
            Err(_) => {
                tracing::warn!(service, "background shutdown deadline reached");
                task.abort();
                let _ = task.await;
            }
        }
    }
    first_error.map_or(Ok(()), Err)
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

async fn try_handle_cli() -> Result<bool> {
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
            validate_runtime_config(&Config::from_env())?;
            Ok(true)
        }
        "check-config" => {
            let env_path = ensure_default_env_file()?;
            load_env_file(&env_path)?;
            ensure_startup_config(&env_path, StartupConfigMode::CheckOnly)?;
            validate_runtime_config(&Config::from_env())?;
            Ok(true)
        }
        "check-db" => {
            let env_path = ensure_default_env_file()?;
            load_env_file(&env_path)?;
            let config = Config::from_env();
            config
                .validate_database_config()
                .map_err(anyhow::Error::msg)?;
            check_database_compatibility(&config)?;
            println!("database schema is compatible");
            Ok(true)
        }
        "rotate-provision-ssh-key" => {
            let env_path = ensure_default_env_file()?;
            load_env_file(&env_path)?;
            let config = Config::from_env();
            config
                .validate_database_config()
                .map_err(anyhow::Error::msg)?;
            let _process_lock = crate::process_lock::ProcessLock::acquire(&config.data_dir)?;
            let store = AppStore::new(&config)?;
            let report =
                crate::client_market::rotate_provision_ssh_key_offline(&config, &store).await?;
            println!(
                "provisioning SSH key rotation completed for {} Host(s)",
                report.host_count
            );
            Ok(true)
        }
        other => anyhow::bail!("unknown command: {other}\n\nRun `{APP_NAME} help` for usage."),
    }
}

fn validate_runtime_config(config: &Config) -> Result<()> {
    config
        .validate_database_config()
        .map_err(anyhow::Error::msg)?;
    config
        .validate_official_provider_config()
        .map_err(anyhow::Error::msg)?;
    config
        .validate_clock_health_config()
        .map_err(anyhow::Error::msg)?;
    config
        .validate_ssh_transport_config()
        .map_err(anyhow::Error::msg)?;
    config
        .validate_proxy_stream_config()
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

fn print_help() {
    println!(
        "\
cc-switch-router

Usage:
  cc-switch-router
  cc-switch-router setup
  cc-switch-router check-config
  cc-switch-router check-db
  cc-switch-router rotate-provision-ssh-key
  cc-switch-router help
  cc-switch-router --help
  cc-switch-router -h

Environment:
  CC_SWITCH_ROUTER_API_ADDR              HTTP listen address, default 0.0.0.0:80
  CC_SWITCH_ROUTER_SSH_ADDR              SSH listen address, default 0.0.0.0:2222
  CC_SWITCH_ROUTER_TUNNEL_DOMAIN         Public tunnel domain, required
  CC_SWITCH_ROUTER_SSH_PUBLIC_ADDR       SSH address sent to clients, required
  CC_SWITCH_ROUTER_SSH_INACTIVITY_TIMEOUT_SECS Inbound SSH inactivity timeout, default 300
  CC_SWITCH_ROUTER_SSH_KEEPALIVE_INTERVAL_SECS SSH keepalive interval, default 30
  CC_SWITCH_ROUTER_SSH_KEEPALIVE_MAX     Unanswered keepalive limit, default 3
  CC_SWITCH_ROUTER_SSH_CHANNEL_OPEN_TIMEOUT_SECS Forward channel open timeout, default 15
  CC_SWITCH_ROUTER_SSH_BRIDGE_WRITE_STALL_TIMEOUT_SECS Bridge write-progress timeout, default 300
  CC_SWITCH_ROUTER_SSH_BRIDGE_HALF_CLOSE_IDLE_TIMEOUT_SECS Half-close idle timeout, default 300
  CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS Global pending + active forward limit, default 2048
  CC_SWITCH_ROUTER_SSH_MAX_FORWARD_CONNECTIONS_PER_TUNNEL Per-tunnel pending + active limit, default 256
  CC_SWITCH_ROUTER_PROXY_REQUEST_BODY_TIMEOUT_SECS Downstream request body timeout, default 30
  CC_SWITCH_ROUTER_PROXY_RESPONSE_HEADER_TIMEOUT_SECS Upstream response header timeout, default 120
  CC_SWITCH_ROUTER_PROXY_STREAM_FIRST_EVENT_TIMEOUT_SECS Share stream first business-event timeout, default 120
  CC_SWITCH_ROUTER_PROXY_STREAM_IDLE_TIMEOUT_SECS Share stream business-idle timeout, default 900
  CC_SWITCH_ROUTER_PROXY_DOWNSTREAM_STALL_TIMEOUT_SECS Downstream response stall timeout, default 120
  CC_SWITCH_ROUTER_PROXY_MAX_REQUEST_LIFETIME_SECS Share request hard lifetime, default 7200
  CC_SWITCH_ROUTER_RESEND_API_KEY        Resend API key for email login, required
  CC_SWITCH_ROUTER_RESEND_FROM           Sender email, default noreply@[TUNNEL_DOMAIN]
  CC_SWITCH_ROUTER_USE_LOCALHOST         Use http for localhost-style domains, default false
  CC_SWITCH_ROUTER_LEASE_TTL_SECS        Tunnel lease ttl, default 60
  CC_SWITCH_ROUTER_DATA_DIR              Router-owned local data directory
  CC_SWITCH_ROUTER_DB_MODE               Business database mode: local or turso, default local
  CC_SWITCH_ROUTER_DB_PATH               Local libSQL or Embedded Replica file path
  CC_SWITCH_ROUTER_TURSO_URL              Turso URL, required in turso mode
  CC_SWITCH_ROUTER_TURSO_AUTH_TOKEN       Turso token, required in turso mode
  CC_SWITCH_ROUTER_DB_SYNC_INTERVAL_SECS  Embedded Replica pull interval, default 60
  CC_SWITCH_ROUTER_CLEANUP_INTERVAL_SECS Cleanup interval, default 300
  CC_SWITCH_ROUTER_LEASE_RETENTION_SECS  Lease retention period, default 86400
  CC_SWITCH_ROUTER_REQUEST_LOG_RETENTION_DAYS Request history retention, default 30 (1-365)
  CC_SWITCH_ROUTER_CLIENT_STALE_SECS     Mark clients offline and purge shares after no heartbeat, default 3600
  CC_SWITCH_ROUTER_CLIENT_INSTALLATION_RETENTION_SECS Delete installation records after offline retention, default 21600
  CC_SWITCH_ROUTER_PAUSED_SHARE_STALE_SECS Delete paused shares after no update, default 3600
Default env file:
  $HOME/.cc-switch-router/.env
  The file is auto-created on first start when missing.
"
    );
}

fn check_database_compatibility(config: &Config) -> Result<()> {
    let connection =
        match config.database.mode {
            DatabaseMode::Local => crate::db::Connection::open(&config.database.path),
            DatabaseMode::Turso => crate::db::Connection::open_remote_replica(
                &config.database.path,
                config
                    .database
                    .turso_url
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("Turso database URL is not configured"))?,
                config.database.turso_auth_token.clone().ok_or_else(|| {
                    anyhow::anyhow!("Turso database auth token is not configured")
                })?,
            ),
        }?;
    crate::schema::check_compatibility(&connection)?;
    Ok(())
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
