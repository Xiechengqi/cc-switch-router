use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use russh::keys::key::KeyPair;
use russh::keys::{encode_pkcs8_pem, load_secret_key};
use russh::server::Msg;
use russh::server::{Auth, Session};
use russh::{Channel, ChannelId, server};
use tokio::io;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::metrics::MetricsRegistry;
use crate::proxy::{ProxyRegistry, RouteKind, RouteShutdown};
use crate::store::AppStore;

#[derive(Clone)]
pub struct SshServer {
    pub store: AppStore,
    pub proxy: Arc<ProxyRegistry>,
    pub host_key: KeyPair,
    pub metrics: Arc<MetricsRegistry>,
}

/// 加载持久化的 SSH host key；不存在则生成并写入磁盘。
///
/// Why: 每次进程启动都 `generate_ed25519()` 会让所有客户端的 known_hosts / 指纹
/// 绑定失效，中间人攻击无法被发现。持久化 host key 后客户端可通过 `ssh_host_fingerprint`
/// 租约字段（P0-3b）进行首次 TOFU + 后续校验。
pub fn load_or_generate_host_key(path: &Path) -> Result<KeyPair> {
    if path.exists() {
        match load_secret_key(path, None) {
            Ok(key) => {
                info!("loaded ssh host key from {}", path.display());
                return Ok(key);
            }
            Err(err) => {
                warn!(
                    "failed to load ssh host key from {}: {}, will regenerate",
                    path.display(),
                    err
                );
            }
        }
    }

    let keypair = KeyPair::generate_ed25519();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create host key dir failed: {}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .with_context(|| format!("create host key file failed: {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = file.set_permissions(perms);
    }
    encode_pkcs8_pem(&keypair, &mut file)
        .with_context(|| format!("write host key failed: {}", path.display()))?;
    info!("generated new ssh host key at {}", path.display());
    Ok(keypair)
}

/// 计算 KeyPair 对应 PublicKey 的 SHA256 指纹字符串（与 OpenSSH 输出一致：`SHA256:<base64-nopad>`）。
pub fn host_key_fingerprint(key: &KeyPair) -> Result<String> {
    let public = key
        .clone_public_key()
        .context("derive public key for fingerprint")?;
    Ok(format!("SHA256:{}", public.fingerprint()))
}

struct ClientHandler {
    store: AppStore,
    proxy: Arc<ProxyRegistry>,
    metrics: Arc<MetricsRegistry>,
    lease: Option<crate::models::TunnelLease>,
    backend: Option<String>,
    forward: Option<ForwardHandle>,
}

impl Clone for ClientHandler {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            proxy: self.proxy.clone(),
            metrics: self.metrics.clone(),
            lease: self.lease.clone(),
            backend: self.backend.clone(),
            forward: None,
        }
    }
}

struct ForwardHandle {
    task: Option<JoinHandle<()>>,
    shutdown: RouteShutdown,
    proxy: Arc<ProxyRegistry>,
    metrics: Arc<MetricsRegistry>,
    subdomain: String,
    connection_id: String,
    generation: u64,
    closed: bool,
}

impl ForwardHandle {
    fn new(
        task: JoinHandle<()>,
        shutdown: RouteShutdown,
        proxy: Arc<ProxyRegistry>,
        metrics: Arc<MetricsRegistry>,
        subdomain: String,
        connection_id: String,
        generation: u64,
    ) -> Self {
        Self {
            task: Some(task),
            shutdown,
            proxy,
            metrics,
            subdomain,
            connection_id,
            generation,
            closed: false,
        }
    }

    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.shutdown.shutdown();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let proxy = self.proxy.clone();
        let subdomain = self.subdomain.clone();
        let connection_id = self.connection_id.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            proxy
                .remove_route_target_if_generation(&subdomain, &connection_id, generation)
                .await;
        });
        self.metrics.forward_listener_shutdown();
    }
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl ClientHandler {
    fn shutdown_forward(&mut self) {
        if let Some(mut forward) = self.forward.take() {
            forward.shutdown();
        }
    }
}

impl SshServer {
    pub async fn run_with_listener(self, listener: TcpListener) -> Result<()> {
        let mut config = server::Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(300)),
            auth_rejection_time: std::time::Duration::from_secs(1),
            ..Default::default()
        };
        config.keys.push(self.host_key.clone());
        let config = Arc::new(config);
        info!("ssh listening on {}", listener.local_addr()?);
        loop {
            let (socket, peer) = listener.accept().await?;
            let config = config.clone();
            let handler = ClientHandler {
                store: self.store.clone(),
                proxy: self.proxy.clone(),
                metrics: self.metrics.clone(),
                lease: None,
                backend: None,
                forward: None,
            };
            tokio::spawn(async move {
                let _session_guard = handler.metrics.ssh_session_started();
                if let Err(err) = server::run_stream(config, socket, handler).await {
                    error!("ssh client {peer} failed: {err}");
                }
            });
        }
    }
}

impl server::Server for ClientHandler {
    type Handler = Self;

    fn new_client(&mut self, _peer_addr: Option<SocketAddr>) -> Self {
        self.clone()
    }
}

#[async_trait]
impl server::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if !is_valid_lease_username(user) {
            debug!("ssh auth rejected for invalid lease username: {user}");
            return Ok(Auth::Reject {
                proceed_with_methods: None,
            });
        }

        match self.store.consume_lease(user, password).await {
            Ok(lease) => {
                self.lease = Some(lease);
                Ok(Auth::Accept)
            }
            Err(err) => {
                error!("ssh auth failed for {user}: {err}");
                Ok(Auth::Reject {
                    proceed_with_methods: None,
                })
            }
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(false)
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let Some(lease) = self.lease.clone() else {
            return Ok(false);
        };
        if self.forward.is_some() {
            warn!(
                subdomain = %lease.subdomain,
                connection_id = %lease.connection_id,
                generation = lease.generation,
                "duplicate tcpip-forward request rejected for existing SSH session"
            );
            return Ok(false);
        }

        let host = normalize_backend_host(address);
        let listener = match TcpListener::bind((host, *port as u16)).await {
            Ok(listener) => listener,
            Err(err) => {
                error!("failed to bind forwarded port {}:{}: {}", host, *port, err);
                self.metrics.forward_bind_error(&err.to_string());
                return Ok(false);
            }
        };
        let bound_port = listener.local_addr()?.port();
        *port = bound_port as u32;
        let backend = format!("{host}:{port}");
        let share_id = lease.share.as_ref().map(|s| s.share_id.clone());
        let is_free_share = lease
            .share
            .as_ref()
            .map(|s| s.for_sale == "Free")
            .unwrap_or(false);
        let parallel_limit = lease.share.as_ref().map(|s| s.parallel_limit).unwrap_or(-1);
        let route_kind = if lease.tunnel_type == "client-web-http" {
            RouteKind::ClientWeb
        } else if share_id.is_some() {
            RouteKind::Share
        } else {
            RouteKind::Market
        };
        let (route_shutdown, shutdown_rx) = RouteShutdown::new();
        self.proxy
            .register_candidate_with_kind(
                lease.subdomain.clone(),
                backend.clone(),
                route_kind,
                Some(lease.installation_id.clone()),
                Some(lease.connection_id.clone()),
                share_id,
                lease.share.as_ref().map(|s| s.share_name.clone()),
                is_free_share,
                parallel_limit,
                Some(route_shutdown.clone()),
                lease.generation,
                lease.rotation_id.clone(),
            )
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        if let Err(error) = self
            .store
            .mark_lease_ready(&lease.connection_id, &lease.route_id, lease.generation)
            .await
        {
            self.proxy
                .remove_route_target_if_generation(
                    &lease.subdomain,
                    &lease.connection_id,
                    lease.generation,
                )
                .await;
            return Err(anyhow::anyhow!(error));
        }
        self.backend = Some(backend.clone());
        let handle = session.handle();
        let connected_address = address.to_string();
        let proxy = self.proxy.clone();
        let metrics = self.metrics.clone();
        let subdomain = lease.subdomain.clone();
        let connection_id = lease.connection_id.clone();
        let generation = lease.generation;
        let task = tokio::spawn(async move {
            if let Err(err) = serve_forward_listener(
                listener,
                handle,
                connected_address,
                bound_port,
                proxy,
                metrics,
                subdomain,
                connection_id,
                generation,
                shutdown_rx,
            )
            .await
            {
                error!("forward listener failed on port {}: {}", bound_port, err);
            }
        });
        self.metrics.forward_listener_started();
        self.forward = Some(ForwardHandle::new(
            task,
            route_shutdown,
            self.proxy.clone(),
            self.metrics.clone(),
            lease.subdomain.clone(),
            lease.connection_id.clone(),
            lease.generation,
        ));
        if lease.tunnel_type == "market-http" {
            if let Err(error) = self
                .proxy
                .promote_candidate(
                    &lease.subdomain,
                    &lease.connection_id,
                    &lease.rotation_id,
                    lease.generation,
                    lease.expected_generation,
                )
                .await
            {
                self.shutdown_forward();
                return Err(anyhow::anyhow!(error));
            }
        }
        info!(
            "registered backend candidate for subdomain={} connection_id={} generation={} backend={}",
            lease.subdomain, lease.connection_id, lease.generation, backend
        );
        Ok(true)
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // A forwarded TCP channel closes after every proxied HTTP connection.
        // It is not the lifetime signal for the session-level reverse forward.
        Ok(())
    }

    async fn cancel_tcpip_forward(
        &mut self,
        _address: &str,
        _port: u32,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.shutdown_forward();
        Ok(true)
    }
}

impl Drop for ClientHandler {
    fn drop(&mut self) {
        self.shutdown_forward();
    }
}

fn is_valid_lease_username(user: &str) -> bool {
    Uuid::parse_str(user.trim()).is_ok()
}

async fn serve_forward_listener(
    listener: TcpListener,
    handle: russh::server::Handle,
    connected_address: String,
    connected_port: u16,
    proxy: Arc<ProxyRegistry>,
    metrics: Arc<MetricsRegistry>,
    subdomain: String,
    connection_id: String,
    generation: u64,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        let accepted = tokio::select! {
            biased;

            changed = shutdown_rx.changed() => {
                match changed {
                    Ok(()) if *shutdown_rx.borrow() => return Ok(()),
                    Ok(()) => continue,
                    Err(_) => return Ok(()),
                }
            }
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(err) => {
                metrics.forward_accept_error(&err.to_string());
                proxy
                    .remove_route_target_if_generation(&subdomain, &connection_id, generation)
                    .await;
                return Err(err.into());
            }
        };
        let handle = handle.clone();
        let connected_address = connected_address.clone();
        let originator_address = peer.ip().to_string();
        let originator_port = peer.port() as u32;
        let channel = match handle
            .channel_open_forwarded_tcpip(
                connected_address.clone(),
                connected_port as u32,
                originator_address,
                originator_port,
            )
            .await
        {
            Ok(channel) => channel,
            Err(err) => {
                warn!(
                    "failed to open one forwarded tcp channel: {} subdomain={} connection_id={} generation={}; listener remains registered",
                    err, subdomain, connection_id, generation
                );
                continue;
            }
        };

        tokio::spawn(async move {
            let mut ssh_stream = channel.into_stream();
            let mut stream = stream;
            if let Err(err) = io::copy_bidirectional(&mut stream, &mut ssh_stream).await {
                error!("forwarded tcp bridge failed: {}", err);
            }
        });
    }
}

fn normalize_backend_host(address: &str) -> &str {
    match address.trim() {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::is_valid_lease_username;

    #[test]
    fn lease_username_must_be_uuid() {
        assert!(is_valid_lease_username(
            "5222754f-d960-47d5-8fd1-7f5e90aaac93"
        ));
        assert!(is_valid_lease_username(
            " 5222754f-d960-47d5-8fd1-7f5e90aaac93 "
        ));

        assert!(!is_valid_lease_username("root"));
        assert!(!is_valid_lease_username("admin"));
        assert!(!is_valid_lease_username("ubuntu"));
        assert!(!is_valid_lease_username(""));
    }
}
