mod bridge;

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::task::AtomicWaker;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, HashAlg, PrivateKey, load_secret_key};
use russh::server::Msg;
use russh::server::{Auth, ChannelOpenHandle, Session};
use russh::{Channel, ChannelId, ChannelOpenFailure, Disconnect, Error as RusshError, server};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::config::SshTransportConfig;
use crate::metrics::{
    ForwardBridgeMetricOutcome, ForwardChannelOpenMetricOutcome, MetricsRegistry,
};
use crate::proxy::{ProxyRegistry, RouteKind, RouteShutdown};
use crate::secure_file::{AtomicCreateOutcome, atomic_create_file_mode, enforce_file_mode};
use crate::store::AppStore;

#[derive(Clone)]
pub struct SshServer {
    pub store: AppStore,
    pub proxy: Arc<ProxyRegistry>,
    pub host_key: PrivateKey,
    pub metrics: Arc<MetricsRegistry>,
    pub transport: SshTransportConfig,
}

/// 加载持久化的 SSH host key；不存在则生成并写入磁盘。
///
/// Why: 每次进程启动都 `generate_ed25519()` 会让所有客户端的 known_hosts / 指纹
/// 绑定失效，中间人攻击无法被发现。持久化 host key 后客户端可通过 `ssh_host_fingerprint`
/// 租约字段（P0-3b）进行首次 TOFU + 后续校验。
const SSH_HOST_KEY_MODE: u32 = 0o600;

pub fn load_or_generate_host_key(path: &Path) -> Result<PrivateKey> {
    match std::fs::metadata(path) {
        Ok(_) => return load_host_key(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read SSH host key metadata failed: {}", path.display()));
        }
    }
    let keypair = PrivateKey::random(&mut rand_0_10::rng(), Algorithm::Ed25519)
        .context("generate Ed25519 SSH host key")?;
    let encoded = keypair
        .to_openssh(LineEnding::LF)
        .context("encode SSH host key")?;
    match atomic_create_file_mode(path, encoded.as_bytes(), SSH_HOST_KEY_MODE)? {
        AtomicCreateOutcome::Created => {
            info!("generated new ssh host key at {}", path.display());
            Ok(keypair)
        }
        AtomicCreateOutcome::AlreadyExists => {
            info!(
                "another process created ssh host key first; loading {}",
                path.display()
            );
            load_host_key(path)
        }
    }
}

fn load_host_key(path: &Path) -> Result<PrivateKey> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("read SSH host key metadata failed: {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!(
            "SSH host key path is not a regular file: {}",
            path.display()
        );
    }
    enforce_file_mode(path, SSH_HOST_KEY_MODE)?;
    let key = load_secret_key(path, None)
        .with_context(|| format!("load SSH host key failed: {}", path.display()))?;
    if key.algorithm() != Algorithm::Ed25519 {
        anyhow::bail!(
            "SSH host key must use Ed25519, found {:?}: {}",
            key.algorithm(),
            path.display()
        );
    }
    info!("loaded ssh host key from {}", path.display());
    Ok(key)
}

/// 计算私钥对应公钥的 SHA256 指纹字符串（与 OpenSSH 输出一致：`SHA256:<base64-nopad>`）。
pub fn host_key_fingerprint(key: &PrivateKey) -> Result<String> {
    Ok(key.public_key().fingerprint(HashAlg::Sha256).to_string())
}

struct ClientHandler {
    store: AppStore,
    proxy: Arc<ProxyRegistry>,
    metrics: Arc<MetricsRegistry>,
    global_forward_capacity: Arc<Semaphore>,
    forward_tasks: Arc<ForwardTaskTracker>,
    session_abort: Option<IoAbortHandle>,
    transport: SshTransportConfig,
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
            global_forward_capacity: self.global_forward_capacity.clone(),
            forward_tasks: self.forward_tasks.clone(),
            session_abort: self.session_abort.clone(),
            transport: self.transport.clone(),
            lease: self.lease.clone(),
            backend: self.backend.clone(),
            forward: None,
        }
    }
}

struct ForwardHandle {
    shutdown: RouteShutdown,
    closed: bool,
}

const FORWARD_CHANNEL_FAILURE_LIMIT: u32 = 3;
const SESSION_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const SESSION_GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(1);
const FORWARD_TASK_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

struct IoAbortState {
    aborted: AtomicBool,
    read_waker: AtomicWaker,
    write_waker: AtomicWaker,
}

#[derive(Clone)]
struct IoAbortHandle {
    state: Arc<IoAbortState>,
}

impl IoAbortHandle {
    fn abort(&self) {
        if !self.state.aborted.swap(true, Ordering::AcqRel) {
            self.state.read_waker.wake();
            self.state.write_waker.wake();
        }
    }
}

struct AbortableIo<T> {
    inner: T,
    state: Arc<IoAbortState>,
}

impl<T> AbortableIo<T> {
    fn new(inner: T) -> (Self, IoAbortHandle) {
        let state = Arc::new(IoAbortState {
            aborted: AtomicBool::new(false),
            read_waker: AtomicWaker::new(),
            write_waker: AtomicWaker::new(),
        });
        (
            Self {
                inner,
                state: state.clone(),
            },
            IoAbortHandle { state },
        )
    }

    fn aborted_error() -> io::Error {
        io::Error::new(io::ErrorKind::ConnectionAborted, "SSH connection aborted")
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for AbortableIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.state.aborted.load(Ordering::Acquire) {
            return Poll::Ready(Err(Self::aborted_error()));
        }
        self.state.read_waker.register(context.waker());
        if self.state.aborted.load(Ordering::Acquire) {
            return Poll::Ready(Err(Self::aborted_error()));
        }
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for AbortableIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.state.aborted.load(Ordering::Acquire) {
            return Poll::Ready(Err(Self::aborted_error()));
        }
        self.state.write_waker.register(context.waker());
        if self.state.aborted.load(Ordering::Acquire) {
            return Poll::Ready(Err(Self::aborted_error()));
        }
        Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        if self.state.aborted.load(Ordering::Acquire) {
            return Poll::Ready(Err(Self::aborted_error()));
        }
        self.state.write_waker.register(context.waker());
        if self.state.aborted.load(Ordering::Acquire) {
            return Poll::Ready(Err(Self::aborted_error()));
        }
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<io::Result<()>> {
        if self.state.aborted.load(Ordering::Acquire) {
            return Poll::Ready(Err(Self::aborted_error()));
        }
        self.state.write_waker.register(context.waker());
        if self.state.aborted.load(Ordering::Acquire) {
            return Poll::Ready(Err(Self::aborted_error()));
        }
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

struct ForwardTaskTracker {
    tasks: StdMutex<JoinSet<()>>,
}

impl ForwardTaskTracker {
    fn new() -> Self {
        Self {
            tasks: StdMutex::new(JoinSet::new()),
        }
    }

    fn spawn<F>(&self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
        while let Some(joined) = tasks.try_join_next() {
            if let Err(error) = joined {
                warn!(%error, "forward listener task failed");
            }
        }
        tasks.spawn(task);
    }

    async fn shutdown_and_wait(&self) {
        let mut tasks = {
            let mut tracked = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
            std::mem::replace(&mut *tracked, JoinSet::new())
        };
        let drained = timeout(FORWARD_TASK_DRAIN_TIMEOUT, async {
            while let Some(joined) = tasks.join_next().await {
                if let Err(error) = joined {
                    warn!(%error, "forward listener task failed during shutdown");
                }
            }
        })
        .await;
        if drained.is_err() {
            warn!("forward listener tasks did not drain in time; aborting them");
            tasks.abort_all();
            while let Some(joined) = tasks.join_next().await {
                if let Err(error) = joined
                    && !error.is_cancelled()
                {
                    warn!(%error, "forward listener task failed while aborting");
                }
            }
        }
    }
}

struct ForwardListenerContext {
    listener: TcpListener,
    handle: russh::server::Handle,
    connected_address: String,
    connected_port: u16,
    proxy: Arc<ProxyRegistry>,
    metrics: Arc<MetricsRegistry>,
    subdomain: String,
    connection_id: String,
    generation: u64,
    transport: SshTransportConfig,
    global_forward_capacity: Arc<Semaphore>,
    session_abort: IoAbortHandle,
    route_shutdown: RouteShutdown,
    shutdown_rx: watch::Receiver<bool>,
}

impl ForwardHandle {
    fn new(shutdown: RouteShutdown) -> Self {
        Self {
            shutdown,
            closed: false,
        }
    }

    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.shutdown.shutdown();
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
    pub async fn run_with_listener(
        self,
        listener: TcpListener,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        self.transport.validate().map_err(anyhow::Error::msg)?;
        let mut config = server::Config {
            inactivity_timeout: Some(Duration::from_secs(self.transport.inactivity_timeout_secs)),
            keepalive_interval: Some(Duration::from_secs(self.transport.keepalive_interval_secs)),
            keepalive_max: self.transport.keepalive_max,
            auth_rejection_time: Duration::from_secs(1),
            nodelay: true,
            ..Default::default()
        };
        config.keys.push(self.host_key.clone());
        let config = Arc::new(config);
        let global_forward_capacity =
            Arc::new(Semaphore::new(self.transport.max_forward_connections));
        let forward_tasks = Arc::new(ForwardTaskTracker::new());
        let (session_shutdown_tx, session_shutdown_rx) = watch::channel(false);
        let mut sessions = JoinSet::new();
        let mut listener_error = None;
        info!("ssh listening on {}", listener.local_addr()?);
        loop {
            let accepted = tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    match changed {
                        Ok(()) if !*shutdown.borrow() => continue,
                        Ok(()) | Err(_) => {
                            info!("ssh listener stopped for graceful shutdown");
                            break;
                        }
                    }
                }
                joined = sessions.join_next(), if !sessions.is_empty() => {
                    if let Some(Err(error)) = joined {
                        warn!(error = %error, "SSH client task failed");
                    }
                    continue;
                }
                accepted = listener.accept() => accepted,
            };
            let (socket, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    listener_error = Some(anyhow::Error::from(error));
                    break;
                }
            };
            let config = config.clone();
            let handler = ClientHandler {
                store: self.store.clone(),
                proxy: self.proxy.clone(),
                metrics: self.metrics.clone(),
                global_forward_capacity: global_forward_capacity.clone(),
                forward_tasks: forward_tasks.clone(),
                session_abort: None,
                transport: self.transport.clone(),
                lease: None,
                backend: None,
                forward: None,
            };
            let session_shutdown = session_shutdown_rx.clone();
            sessions.spawn(async move {
                run_client_session(config, socket, handler, peer, session_shutdown).await;
            });
        }

        let _ = session_shutdown_tx.send(true);
        while let Some(joined) = sessions.join_next().await {
            if let Err(error) = joined {
                warn!(error = %error, "SSH client task failed during shutdown");
            }
        }
        forward_tasks.shutdown_and_wait().await;
        match global_forward_capacity
            .clone()
            .try_acquire_many_owned(self.transport.max_forward_connections as u32)
        {
            Ok(permits) => drop(permits),
            Err(error) => warn!(%error, "SSH bridge permits leaked after forward tasks stopped"),
        }
        listener_error.map_or(Ok(()), Err)
    }
}

async fn run_client_session(
    config: Arc<server::Config>,
    socket: tokio::net::TcpStream,
    mut handler: ClientHandler,
    peer: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
) {
    let _session_guard = handler.metrics.ssh_session_started();
    if let Err(error) = socket.set_nodelay(true) {
        warn!(%peer, %error, "failed to enable TCP_NODELAY for SSH client");
    }
    let (socket, io_abort) = AbortableIo::new(socket);
    handler.session_abort = Some(io_abort.clone());
    let setup = tokio::select! {
        biased;
        () = wait_for_route_shutdown(&mut shutdown) => return,
        result = server::run_stream(config, socket, handler) => result,
    };
    let mut session = match setup {
        Ok(session) => session,
        Err(error) => {
            error!("ssh client {peer} setup failed: {error}");
            return;
        }
    };
    let session_handle = session.handle();
    tokio::select! {
        biased;
        () = wait_for_route_shutdown(&mut shutdown) => {
            let _ = timeout(
                SESSION_DISCONNECT_TIMEOUT,
                session_handle.disconnect(
                    Disconnect::ByApplication,
                    "router shutting down".into(),
                    "en".into(),
                ),
            )
            .await;
            match timeout(SESSION_GRACEFUL_STOP_TIMEOUT, &mut session).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => debug!("ssh client {peer} stopped during shutdown: {error}"),
                Err(_) => {
                    warn!("ssh client {peer} did not stop after disconnect; aborting its socket");
                    io_abort.abort();
                    if let Err(error) = session.await {
                        debug!("ssh client {peer} stopped after socket abort: {error}");
                    }
                }
            }
        }
        result = &mut session => {
            if let Err(error) = result {
                error!("ssh client {peer} failed: {error}");
            }
        }
    }
}

impl server::Server for ClientHandler {
    type Handler = Self;

    fn new_client(&mut self, _peer_addr: Option<SocketAddr>) -> Self {
        self.clone()
    }
}

impl server::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if !is_valid_lease_username(user) {
            debug!("ssh auth rejected for invalid lease username: {user}");
            return Ok(Auth::reject());
        }

        match self.store.consume_lease(user, password).await {
            Ok(lease) => {
                self.lease = Some(lease);
                Ok(Auth::Accept)
            }
            Err(err) => {
                error!("ssh auth failed for {user}: {err}");
                Ok(Auth::reject())
            }
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply
            .reject(ChannelOpenFailure::AdministrativelyProhibited)
            .await;
        Ok(())
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
        let is_free_share = lease.share.as_ref().map(|s| s.free_access).unwrap_or(false);
        let parallel_limit = lease.share.as_ref().map(|s| s.parallel_limit).unwrap_or(-1);
        let route_kind = if lease.tunnel_type == "client-web-http" {
            RouteKind::ClientWeb
        } else if share_id.is_some() {
            RouteKind::Share
        } else {
            warn!(
                subdomain = %lease.subdomain,
                connection_id = %lease.connection_id,
                "rejecting tunnel without a Client or Share route"
            );
            return Ok(false);
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
        let transport = self.transport.clone();
        let global_forward_capacity = self.global_forward_capacity.clone();
        let session_abort = self
            .session_abort
            .clone()
            .context("SSH session abort handle is unavailable")?;
        let listener_shutdown = route_shutdown.clone();
        let listener_metrics_guard = self.metrics.forward_listener_started();
        self.forward_tasks.spawn(async move {
            let _listener_metrics_guard = listener_metrics_guard;
            if let Err(err) = serve_forward_listener(ForwardListenerContext {
                listener,
                handle,
                connected_address,
                connected_port: bound_port,
                proxy,
                metrics,
                subdomain,
                connection_id,
                generation,
                transport,
                global_forward_capacity,
                session_abort,
                route_shutdown: listener_shutdown,
                shutdown_rx,
            })
            .await
            {
                error!("forward listener failed on port {}: {}", bound_port, err);
            }
        });
        self.forward = Some(ForwardHandle::new(route_shutdown));
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

type ForwardCapacityPermits = (OwnedSemaphorePermit, OwnedSemaphorePermit);

struct OpenedForwardChannel {
    stream: TcpStream,
    peer: SocketAddr,
    channel: Channel<Msg>,
    permits: ForwardCapacityPermits,
}

struct ForwardBridgeContext {
    opened: OpenedForwardChannel,
    metrics: Arc<MetricsRegistry>,
    timeouts: bridge::BridgeTimeouts,
    shutdown_rx: watch::Receiver<bool>,
    subdomain: String,
    connection_id: String,
    generation: u64,
}

enum ForwardChannelOpenResult {
    Opened(OpenedForwardChannel),
    ExplicitFailure { peer: SocketAddr, reason: String },
    TimedOut { peer: SocketAddr },
    SessionError { peer: SocketAddr, reason: String },
    Cancelled,
}

struct ForwardChannelOpenContext {
    stream: TcpStream,
    peer: SocketAddr,
    permits: ForwardCapacityPermits,
    handle: russh::server::Handle,
    connected_address: String,
    connected_port: u16,
    timeout: Duration,
    shutdown_rx: watch::Receiver<bool>,
    metrics: Arc<MetricsRegistry>,
}

async fn open_forward_channel(context: ForwardChannelOpenContext) -> ForwardChannelOpenResult {
    let ForwardChannelOpenContext {
        stream,
        peer,
        permits,
        handle,
        connected_address,
        connected_port,
        timeout: open_timeout,
        mut shutdown_rx,
        metrics,
    } = context;
    let metrics_guard = metrics.forward_channel_open_started();
    let opened = tokio::select! {
        biased;
        () = wait_for_route_shutdown(&mut shutdown_rx) => {
            return ForwardChannelOpenResult::Cancelled;
        }
        opened = timeout(
            open_timeout,
            handle.channel_open_forwarded_tcpip(
                connected_address,
                connected_port as u32,
                peer.ip().to_string(),
                peer.port() as u32,
            ),
        ) => opened,
    };
    match opened {
        Ok(Ok(channel)) => {
            metrics_guard.finish(ForwardChannelOpenMetricOutcome::Succeeded);
            ForwardChannelOpenResult::Opened(OpenedForwardChannel {
                stream,
                peer,
                channel,
                permits,
            })
        }
        Ok(Err(RusshError::ChannelOpenFailure(reason))) => {
            metrics_guard.finish(ForwardChannelOpenMetricOutcome::ExplicitFailure);
            ForwardChannelOpenResult::ExplicitFailure {
                peer,
                reason: format!("{reason:?}"),
            }
        }
        Ok(Err(error)) => {
            metrics_guard.finish(ForwardChannelOpenMetricOutcome::SessionError);
            ForwardChannelOpenResult::SessionError {
                peer,
                reason: error.to_string(),
            }
        }
        Err(_) => {
            metrics_guard.finish(ForwardChannelOpenMetricOutcome::TimedOut);
            ForwardChannelOpenResult::TimedOut { peer }
        }
    }
}

fn spawn_forward_bridge(bridges: &mut JoinSet<()>, context: ForwardBridgeContext) {
    let ForwardBridgeContext {
        opened,
        metrics,
        timeouts,
        shutdown_rx,
        subdomain,
        connection_id,
        generation,
    } = context;
    let OpenedForwardChannel {
        stream,
        peer,
        channel,
        permits,
    } = opened;
    let channel_id = channel.id();
    bridges.spawn(async move {
        let _permits = permits;
        let metrics_guard = metrics.forward_bridge_started();
        let outcome = bridge::run(stream, channel.into_stream(), timeouts, shutdown_rx).await;
        let stats = outcome.stats();
        let metric_outcome = match &outcome {
            bridge::BridgeOutcome::Completed { .. } => {
                debug!(
                    subdomain = %subdomain,
                    connection_id = %connection_id,
                    generation,
                    channel = %channel_id,
                    peer = %peer,
                    outcome = "completed",
                    local_to_ssh_bytes = stats.local_to_ssh_bytes,
                    ssh_to_local_bytes = stats.ssh_to_local_bytes,
                    "forwarded TCP bridge finished"
                );
                ForwardBridgeMetricOutcome::Completed
            }
            bridge::BridgeOutcome::Cancelled { .. } => {
                debug!(
                    subdomain = %subdomain,
                    connection_id = %connection_id,
                    generation,
                    channel = %channel_id,
                    peer = %peer,
                    outcome = "cancelled",
                    local_to_ssh_bytes = stats.local_to_ssh_bytes,
                    ssh_to_local_bytes = stats.ssh_to_local_bytes,
                    "forwarded TCP bridge finished"
                );
                ForwardBridgeMetricOutcome::Cancelled
            }
            bridge::BridgeOutcome::WriteStall {
                direction,
                operation,
                ..
            } => {
                warn!(
                    subdomain = %subdomain,
                    connection_id = %connection_id,
                    generation,
                    channel = %channel_id,
                    peer = %peer,
                    outcome = "write_stall",
                    direction = %direction,
                    operation,
                    local_to_ssh_bytes = stats.local_to_ssh_bytes,
                    ssh_to_local_bytes = stats.ssh_to_local_bytes,
                    "forwarded TCP bridge timed out without write progress"
                );
                ForwardBridgeMetricOutcome::WriteStall
            }
            bridge::BridgeOutcome::HalfCloseIdle { waiting_for, .. } => {
                debug!(
                    subdomain = %subdomain,
                    connection_id = %connection_id,
                    generation,
                    channel = %channel_id,
                    peer = %peer,
                    outcome = "half_close_idle",
                    waiting_for = %waiting_for,
                    local_to_ssh_bytes = stats.local_to_ssh_bytes,
                    ssh_to_local_bytes = stats.ssh_to_local_bytes,
                    "forwarded TCP bridge closed after half-close idle timeout"
                );
                ForwardBridgeMetricOutcome::HalfCloseIdle
            }
            bridge::BridgeOutcome::IoError {
                direction,
                operation,
                error,
                ..
            } => {
                debug!(
                    subdomain = %subdomain,
                    connection_id = %connection_id,
                    generation,
                    channel = %channel_id,
                    peer = %peer,
                    outcome = "io_error",
                    direction = %direction,
                    operation,
                    error = %error,
                    local_to_ssh_bytes = stats.local_to_ssh_bytes,
                    ssh_to_local_bytes = stats.ssh_to_local_bytes,
                    "forwarded TCP bridge ended with an I/O error"
                );
                ForwardBridgeMetricOutcome::IoError
            }
        };
        metrics_guard.finish(metric_outcome);
    });
}

async fn serve_forward_listener(context: ForwardListenerContext) -> Result<()> {
    let ForwardListenerContext {
        listener,
        handle,
        connected_address,
        connected_port,
        proxy,
        metrics,
        subdomain,
        connection_id,
        generation,
        transport,
        global_forward_capacity,
        session_abort,
        route_shutdown,
        mut shutdown_rx,
    } = context;
    let tunnel_forward_capacity =
        Arc::new(Semaphore::new(transport.max_forward_connections_per_tunnel));
    let bridge_timeouts = bridge::BridgeTimeouts {
        write_stall: Duration::from_secs(transport.bridge_write_stall_timeout_secs),
        half_close_idle: Duration::from_secs(transport.bridge_half_close_idle_timeout_secs),
    };
    let channel_open_timeout = Duration::from_secs(transport.channel_open_timeout_secs);
    let mut pending_opens = JoinSet::new();
    let mut bridges = JoinSet::new();
    let mut consecutive_channel_failures = 0_u32;
    let mut terminal_error = None;

    'serve: loop {
        tokio::select! {
            biased;
            () = wait_for_route_shutdown(&mut shutdown_rx) => break 'serve,
            joined = pending_opens.join_next(), if !pending_opens.is_empty() => {
                match joined {
                    Some(Ok(ForwardChannelOpenResult::Opened(opened))) => {
                        consecutive_channel_failures = 0;
                        spawn_forward_bridge(
                            &mut bridges,
                            ForwardBridgeContext {
                                opened,
                                metrics: metrics.clone(),
                                timeouts: bridge_timeouts,
                                shutdown_rx: shutdown_rx.clone(),
                                subdomain: subdomain.clone(),
                                connection_id: connection_id.clone(),
                                generation,
                            },
                        );
                    }
                    Some(Ok(ForwardChannelOpenResult::ExplicitFailure { peer, reason })) => {
                        consecutive_channel_failures = consecutive_channel_failures.saturating_add(1);
                        warn!(
                            subdomain = %subdomain,
                            connection_id = %connection_id,
                            generation,
                            peer = %peer,
                            consecutive_failures = consecutive_channel_failures,
                            reason = %reason,
                            "client rejected forwarded TCP channel"
                        );
                        if consecutive_channel_failures >= FORWARD_CHANNEL_FAILURE_LIMIT {
                            terminal_error = Some(anyhow::anyhow!(
                                "client rejected {consecutive_channel_failures} consecutive forwarded TCP channels: {reason}"
                            ));
                            break 'serve;
                        }
                    }
                    Some(Ok(ForwardChannelOpenResult::TimedOut { peer })) => {
                        terminal_error = Some(anyhow::anyhow!(
                            "forwarded TCP channel open for {peer} timed out after {} seconds",
                            transport.channel_open_timeout_secs
                        ));
                        break 'serve;
                    }
                    Some(Ok(ForwardChannelOpenResult::SessionError { peer, reason })) => {
                        terminal_error = Some(anyhow::anyhow!(
                            "SSH session failed while opening forwarded TCP channel for {peer}: {reason}"
                        ));
                        break 'serve;
                    }
                    Some(Ok(ForwardChannelOpenResult::Cancelled)) | None => break 'serve,
                    Some(Err(error)) => {
                        terminal_error = Some(anyhow::anyhow!(
                            "forwarded TCP channel open task failed: {error}"
                        ));
                        break 'serve;
                    }
                }
            }
            joined = bridges.join_next(), if !bridges.is_empty() => {
                if let Some(Err(error)) = joined {
                    warn!(
                        subdomain = %subdomain,
                        connection_id = %connection_id,
                        generation,
                        %error,
                        "forwarded TCP bridge task failed"
                    );
                }
            }
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        metrics.forward_accept_error(&error.to_string());
                        terminal_error = Some(anyhow::Error::from(error));
                        break 'serve;
                    }
                };
                let permits = match try_acquire_forward_capacity(
                    &global_forward_capacity,
                    &tunnel_forward_capacity,
                ) {
                    Ok(permits) => permits,
                    Err(scope) => {
                        metrics.forward_capacity_rejected();
                        debug!(
                            subdomain = %subdomain,
                            connection_id = %connection_id,
                            generation,
                            peer = %peer,
                            capacity_scope = %scope,
                            "forwarded TCP connection rejected at capacity"
                        );
                        drop(stream);
                        continue;
                    }
                };
                pending_opens.spawn(open_forward_channel(ForwardChannelOpenContext {
                    stream,
                    peer,
                    permits,
                    handle: handle.clone(),
                    connected_address: connected_address.clone(),
                    connected_port,
                    timeout: channel_open_timeout,
                    shutdown_rx: shutdown_rx.clone(),
                    metrics: metrics.clone(),
                }));
            }
        }
    }

    route_shutdown.shutdown();
    proxy
        .remove_route_target_if_generation(&subdomain, &connection_id, generation)
        .await;
    let disconnect_message = if terminal_error.is_some() {
        "router forward session failed"
    } else {
        "router forward retired"
    };
    disconnect_and_abort_session(&handle, &session_abort, disconnect_message).await;
    pending_opens.abort_all();
    if timeout(
        FORWARD_TASK_DRAIN_TIMEOUT,
        drain_forward_work(&mut pending_opens, &mut bridges),
    )
    .await
    .is_err()
    {
        warn!(
            subdomain = %subdomain,
            connection_id = %connection_id,
            generation,
            "forward channel tasks did not drain in time; aborting them"
        );
        pending_opens.abort_all();
        bridges.abort_all();
        drain_forward_work(&mut pending_opens, &mut bridges).await;
    }

    terminal_error.map_or(Ok(()), Err)
}

async fn disconnect_and_abort_session(
    handle: &russh::server::Handle,
    session_abort: &IoAbortHandle,
    message: &str,
) {
    match timeout(
        SESSION_DISCONNECT_TIMEOUT,
        handle.disconnect(Disconnect::ByApplication, message.to_string(), "en".into()),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => debug!(%error, "failed to request SSH session disconnect"),
        Err(_) => warn!("requesting SSH session disconnect timed out"),
    }
    session_abort.abort();
}

async fn drain_forward_work(
    pending_opens: &mut JoinSet<ForwardChannelOpenResult>,
    bridges: &mut JoinSet<()>,
) {
    while !pending_opens.is_empty() || !bridges.is_empty() {
        tokio::select! {
            joined = pending_opens.join_next(), if !pending_opens.is_empty() => {
                if let Some(Err(error)) = joined
                    && !error.is_cancelled()
                {
                    warn!(%error, "forward channel open task failed during shutdown");
                }
            }
            joined = bridges.join_next(), if !bridges.is_empty() => {
                if let Some(Err(error)) = joined
                    && !error.is_cancelled()
                {
                    warn!(%error, "forward bridge task failed during shutdown");
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForwardCapacityScope {
    Global,
    Tunnel,
}

impl std::fmt::Display for ForwardCapacityScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Global => formatter.write_str("global"),
            Self::Tunnel => formatter.write_str("tunnel"),
        }
    }
}

fn try_acquire_forward_capacity(
    global: &Arc<Semaphore>,
    tunnel: &Arc<Semaphore>,
) -> std::result::Result<(OwnedSemaphorePermit, OwnedSemaphorePermit), ForwardCapacityScope> {
    let tunnel_permit = tunnel
        .clone()
        .try_acquire_owned()
        .map_err(|_| ForwardCapacityScope::Tunnel)?;
    let global_permit = global
        .clone()
        .try_acquire_owned()
        .map_err(|_| ForwardCapacityScope::Global)?;
    Ok((global_permit, tunnel_permit))
}

async fn wait_for_route_shutdown(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
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
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};

    use russh::client;
    use russh::client::{
        ChannelOpenHandle as ClientChannelOpenHandle, Msg as ClientMsg, Session as ClientSession,
    };
    use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
    use russh::{Channel, ChannelMsg, ChannelOpenFailure, Disconnect, Error as RusshError};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{mpsc, oneshot, watch};
    use tokio::task::{JoinHandle, JoinSet};
    use tokio::time::{Duration, sleep, timeout};

    use crate::config::{AlertingSettings, MetricsConfig, SshTransportConfig};
    use crate::metrics::MetricsRegistry;
    use crate::proxy::{ProxyRegistry, RouteShutdown};

    use super::{
        AbortableIo, ForwardCapacityScope, ForwardChannelOpenContext, ForwardChannelOpenResult,
        ForwardListenerContext, ForwardTaskTracker, IoAbortHandle, disconnect_and_abort_session,
        drain_forward_work, host_key_fingerprint, is_valid_lease_username,
        load_or_generate_host_key, open_forward_channel, serve_forward_listener,
        try_acquire_forward_capacity,
    };

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

    #[test]
    fn host_key_round_trips_in_openssh_format_with_stable_fingerprint() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-host-key-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = root.join("ssh_host_ed25519_key");
        let generated = load_or_generate_host_key(&path).expect("generate host key");
        let generated_fingerprint = host_key_fingerprint(&generated).expect("fingerprint host key");
        assert!(generated_fingerprint.starts_with("SHA256:"));
        assert!(
            std::fs::read_to_string(&path)
                .expect("read host key")
                .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        let loaded = load_or_generate_host_key(&path).expect("reload host key");
        assert_eq!(
            host_key_fingerprint(&loaded).expect("fingerprint loaded key"),
            generated_fingerprint
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_ed25519_host_key_is_rejected_without_replacement() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-wrong-host-key-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("ssh_host_ed25519_key");
        let key = russh::keys::PrivateKey::random(
            &mut rand_0_10::rng(),
            russh::keys::Algorithm::Ecdsa {
                curve: russh::keys::ssh_key::EcdsaCurve::NistP256,
            },
        )
        .expect("generate ECDSA test key");
        let encoded = key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        std::fs::write(&path, encoded.as_bytes()).unwrap();

        let error = load_or_generate_host_key(&path).unwrap_err();
        assert!(error.to_string().contains("must use Ed25519"));
        assert_eq!(std::fs::read(&path).unwrap(), encoded.as_bytes());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_first_start_converges_on_one_host_key() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-concurrent-host-key-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = std::sync::Arc::new(root.join("ssh_host_ed25519_key"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let key = load_or_generate_host_key(&path).expect("load or create host key");
                    host_key_fingerprint(&key).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let fingerprints = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(fingerprints[0], fingerprints[1]);
        assert_eq!(
            host_key_fingerprint(&load_or_generate_host_key(&path).unwrap()).unwrap(),
            fingerprints[0]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_existing_host_key_is_not_silently_replaced() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-invalid-host-key-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("ssh_host_ed25519_key");
        std::fs::write(&path, b"invalid-key\n").unwrap();

        let error = load_or_generate_host_key(&path).unwrap_err();
        assert!(error.to_string().contains("load SSH host key failed"));
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid-key\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn forward_capacity_is_bounded_and_permits_are_reusable() {
        let global = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let tunnel = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permits = try_acquire_forward_capacity(&global, &tunnel).unwrap();
        assert_eq!(
            try_acquire_forward_capacity(&global, &tunnel).unwrap_err(),
            ForwardCapacityScope::Tunnel
        );
        drop(permits);
        assert!(try_acquire_forward_capacity(&global, &tunnel).is_ok());

        let exhausted_global = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let available_tunnel = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        assert_eq!(
            try_acquire_forward_capacity(&exhausted_global, &available_tunnel).unwrap_err(),
            ForwardCapacityScope::Global
        );
        assert_eq!(available_tunnel.available_permits(), 1);
    }

    #[tokio::test]
    async fn abortable_io_wakes_a_blocked_read() {
        let (stream, _peer) = tokio::io::duplex(64);
        let (mut stream, abort) = AbortableIo::new(stream);
        let reader = tokio::spawn(async move {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).await
        });
        tokio::task::yield_now().await;

        abort.abort();
        let error = timeout(Duration::from_secs(1), reader)
            .await
            .expect("aborted read was not woken")
            .expect("reader task panicked")
            .expect_err("aborted read unexpectedly succeeded");
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
    }

    #[tokio::test]
    async fn abortable_io_wakes_a_blocked_write() {
        let (stream, _peer) = tokio::io::duplex(1);
        let (mut stream, abort) = AbortableIo::new(stream);
        stream.write_all(&[1]).await.unwrap();
        let writer = tokio::spawn(async move { stream.write_all(&[2]).await });
        tokio::task::yield_now().await;

        abort.abort();
        let error = timeout(Duration::from_secs(1), writer)
            .await
            .expect("aborted write was not woken")
            .expect("writer task panicked")
            .expect_err("aborted write unexpectedly succeeded");
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
    }

    #[tokio::test]
    async fn forward_task_tracker_waits_for_all_tasks() {
        let tracker = std::sync::Arc::new(ForwardTaskTracker::new());
        let (first_tx, first_rx) = oneshot::channel::<()>();
        let (second_tx, second_rx) = oneshot::channel::<()>();
        tracker.spawn(async move {
            let _ = first_rx.await;
        });
        tracker.spawn(async move {
            let _ = second_rx.await;
        });
        let shutdown_tracker = tracker.clone();
        let mut shutdown = tokio::spawn(async move {
            shutdown_tracker.shutdown_and_wait().await;
        });
        assert!(
            timeout(Duration::from_millis(20), &mut shutdown)
                .await
                .is_err()
        );

        first_tx.send(()).unwrap();
        assert!(
            timeout(Duration::from_millis(20), &mut shutdown)
                .await
                .is_err()
        );
        second_tx.send(()).unwrap();
        timeout(Duration::from_secs(1), &mut shutdown)
            .await
            .expect("tracker did not observe the final task exit")
            .expect("tracker shutdown task failed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forwarded_channel_opens_complete_out_of_order_without_head_of_line_blocking() {
        let mut session = ControlledSshSession::start().await;
        let metrics = test_metrics();
        let global = Arc::new(tokio::sync::Semaphore::new(2));
        let proxy = Arc::new(ProxyRegistry::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listener_address = listener.local_addr().unwrap();
        let (route_shutdown, shutdown_rx) = RouteShutdown::new();
        let listener_task = tokio::spawn(serve_forward_listener(ForwardListenerContext {
            listener,
            handle: session.server_handle.clone(),
            connected_address: "127.0.0.1".to_string(),
            connected_port: 1,
            proxy,
            metrics: metrics.clone(),
            subdomain: "test-forward".to_string(),
            connection_id: "test-connection".to_string(),
            generation: 1,
            transport: SshTransportConfig {
                channel_open_timeout_secs: 2,
                max_forward_connections: 2,
                max_forward_connections_per_tunnel: 2,
                ..Default::default()
            },
            global_forward_capacity: global.clone(),
            session_abort: session.server_abort.clone(),
            route_shutdown: route_shutdown.clone(),
            shutdown_rx,
        }));

        let first_peer_stream = TcpStream::connect(listener_address).await.unwrap();
        let first_peer = first_peer_stream.local_addr().unwrap();
        let first_request = session.next_request().await;
        let second_peer_stream = TcpStream::connect(listener_address).await.unwrap();
        let second_peer = second_peer_stream.local_addr().unwrap();
        let second_request = session.next_request().await;
        assert_eq!(first_request.originator_port, first_peer.port() as u32);
        assert_eq!(second_request.originator_port, second_peer.port() as u32);

        let second_client_channel = second_request.accept().await;
        let status = wait_for_forward_metrics(&metrics, |status| {
            status.ssh_channel_open_succeeded_total == 1 && status.ssh_active_bridges == 1
        })
        .await;
        assert_eq!(status.ssh_pending_channel_opens, 1);

        let first_client_channel = first_request.accept().await;
        let status = wait_for_forward_metrics(&metrics, |status| {
            status.ssh_channel_open_succeeded_total == 2 && status.ssh_active_bridges == 2
        })
        .await;
        assert_eq!(status.ssh_pending_channel_opens, 0);

        route_shutdown.shutdown();
        timeout(Duration::from_secs(2), listener_task)
            .await
            .expect("forward listener did not stop")
            .expect("forward listener task failed")
            .expect("forward listener returned an error");
        session.wait_for_stop().await;
        drop(first_client_channel);
        drop(second_client_channel);
        drop(first_peer_stream);
        drop(second_peer_stream);

        assert_eq!(global.available_permits(), 2);
        let status = metrics.router_status(&ProxyRegistry::default()).await;
        assert_eq!(status.ssh_pending_channel_opens, 0);
        assert_eq!(status.ssh_active_bridges, 0);
        assert_eq!(status.ssh_channel_open_started_total, 2);
        assert_eq!(status.ssh_channel_open_succeeded_total, 2);
        assert_eq!(status.ssh_bridge_created_total, 2);
        assert_eq!(status.ssh_bridge_cancelled_total, 2);
    }

    #[tokio::test]
    async fn forwarded_channel_explicit_rejection_and_route_cancellation_are_classified() {
        let mut session = ControlledSshSession::start().await;
        let metrics = test_metrics();
        let global = Arc::new(tokio::sync::Semaphore::new(1));
        let tunnel = Arc::new(tokio::sync::Semaphore::new(1));
        let (_peer_stream, stream, peer) = tcp_pair().await;
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let rejected_open = spawn_test_channel_open(
            &session.server_handle,
            metrics.clone(),
            global.clone(),
            tunnel.clone(),
            stream,
            peer,
            Duration::from_secs(1),
            shutdown_rx,
        );
        session
            .next_request()
            .await
            .reject(ChannelOpenFailure::ConnectFailed)
            .await;
        let rejected = timeout(Duration::from_secs(1), rejected_open)
            .await
            .expect("explicit rejection did not finish")
            .expect("explicit rejection task failed");
        assert!(matches!(
            rejected,
            ForwardChannelOpenResult::ExplicitFailure { peer: result_peer, .. }
                if result_peer == peer
        ));

        let (_peer_stream, stream, peer) = tcp_pair().await;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let cancelled_open = spawn_test_channel_open(
            &session.server_handle,
            metrics.clone(),
            global.clone(),
            tunnel.clone(),
            stream,
            peer,
            Duration::from_secs(1),
            shutdown_rx,
        );
        let pending_request = session.next_request().await;
        shutdown_tx.send(true).unwrap();
        let cancelled = timeout(Duration::from_secs(1), cancelled_open)
            .await
            .expect("route cancellation did not finish")
            .expect("route cancellation task failed");
        assert!(matches!(cancelled, ForwardChannelOpenResult::Cancelled));
        drop(pending_request);

        assert_eq!(global.available_permits(), 1);
        assert_eq!(tunnel.available_permits(), 1);
        let status = metrics.router_status(&ProxyRegistry::default()).await;
        assert_eq!(status.ssh_pending_channel_opens, 0);
        assert_eq!(status.ssh_channel_open_started_total, 2);
        assert_eq!(status.ssh_channel_open_explicit_failures_total, 1);
        assert_eq!(status.ssh_channel_open_cancelled_total, 1);
        session.shutdown().await;
    }

    #[tokio::test]
    async fn forwarded_channel_timeout_retires_session_and_releases_all_state() {
        let mut session = ControlledSshSession::start().await;
        let metrics = test_metrics();
        let global = Arc::new(tokio::sync::Semaphore::new(1));
        let tunnel = Arc::new(tokio::sync::Semaphore::new(1));
        let (_peer_stream, stream, peer) = tcp_pair().await;
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let timed_open = spawn_test_channel_open(
            &session.server_handle,
            metrics.clone(),
            global.clone(),
            tunnel.clone(),
            stream,
            peer,
            Duration::from_millis(40),
            shutdown_rx,
        );
        let mut late_request = session.next_request().await;
        let result = timeout(Duration::from_secs(1), timed_open)
            .await
            .expect("forwarded channel timeout did not finish")
            .expect("forwarded channel timeout task failed");
        assert!(matches!(
            result,
            ForwardChannelOpenResult::TimedOut { peer: result_peer } if result_peer == peer
        ));

        disconnect_and_abort_session(
            &session.server_handle,
            &session.server_abort,
            "test channel open timeout",
        )
        .await;
        session.wait_for_stop().await;
        late_request.reply.accept().await;
        assert!(matches!(
            timeout(Duration::from_secs(1), late_request.channel.wait()).await,
            Ok(None | Some(ChannelMsg::Close))
        ));
        assert!(matches!(
            session
                .server_handle
                .channel_open_forwarded_tcpip("127.0.0.1", 1, "127.0.0.1", 1)
                .await,
            Err(RusshError::SendError)
        ));

        assert_eq!(global.available_permits(), 1);
        assert_eq!(tunnel.available_permits(), 1);
        let status = metrics.router_status(&ProxyRegistry::default()).await;
        assert_eq!(status.ssh_pending_channel_opens, 0);
        assert_eq!(status.ssh_channel_open_started_total, 1);
        assert_eq!(status.ssh_channel_open_timeout_total, 1);
        assert_eq!(status.ssh_channel_open_succeeded_total, 0);
    }

    #[tokio::test]
    async fn forward_drain_reclaims_aborted_pending_open_state() {
        let mut session = ControlledSshSession::start().await;
        let metrics = test_metrics();
        let global = Arc::new(tokio::sync::Semaphore::new(1));
        let tunnel = Arc::new(tokio::sync::Semaphore::new(1));
        let (_peer_stream, stream, peer) = tcp_pair().await;
        let permits = try_acquire_forward_capacity(&global, &tunnel).unwrap();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut pending_opens = JoinSet::new();
        pending_opens.spawn(open_forward_channel(ForwardChannelOpenContext {
            stream,
            peer,
            permits,
            handle: session.server_handle.clone(),
            connected_address: "127.0.0.1".to_string(),
            connected_port: 1,
            timeout: Duration::from_secs(5),
            shutdown_rx,
            metrics: metrics.clone(),
        }));
        let pending_request = session.next_request().await;
        assert_eq!(
            metrics
                .router_status(&ProxyRegistry::default())
                .await
                .ssh_pending_channel_opens,
            1
        );

        pending_opens.abort_all();
        let mut bridges = JoinSet::new();
        timeout(
            Duration::from_secs(1),
            drain_forward_work(&mut pending_opens, &mut bridges),
        )
        .await
        .expect("aborted forward open did not drain");
        drop(pending_request);

        assert_eq!(global.available_permits(), 1);
        assert_eq!(tunnel.available_permits(), 1);
        let status = metrics.router_status(&ProxyRegistry::default()).await;
        assert_eq!(status.ssh_pending_channel_opens, 0);
        assert_eq!(status.ssh_channel_open_started_total, 1);
        assert_eq!(status.ssh_channel_open_cancelled_total, 1);
        session.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn russh_zero_window_waits_without_spinning_and_resumes() {
        const WINDOW_SIZE: u32 = 1_024;
        const PACKET_SIZE: u32 = 256;

        let payload = vec![0x5a; WINDOW_SIZE as usize * 4];
        let polls = std::sync::Arc::new(AtomicUsize::new(0));
        let (start_tx, start_rx) = oneshot::channel();
        let (completed_tx, mut completed_rx) = oneshot::channel();
        let server_handler = ZeroWindowServer {
            start: Some(start_rx),
            completed: Some(completed_tx),
            polls: polls.clone(),
            payload: payload.clone(),
        };
        let server_config = std::sync::Arc::new(russh::server::Config {
            keys: vec![
                russh::keys::PrivateKey::random(
                    &mut rand_0_10::rng(),
                    russh::keys::Algorithm::Ed25519,
                )
                .unwrap(),
            ],
            inactivity_timeout: None,
            ..Default::default()
        });
        let client_config = std::sync::Arc::new(client::Config {
            window_size: WINDOW_SIZE,
            maximum_packet_size: PACKET_SIZE,
            channel_buffer_size: 64,
            inactivity_timeout: None,
            ..Default::default()
        });
        let (server_io, client_io) = tokio::io::duplex(1 << 20);
        let (gated_client_io, read_gate) = GatedIo::new(client_io);
        let mut server_task = tokio::spawn(async move {
            let running = russh::server::run_stream(server_config, server_io, server_handler)
                .await
                .expect("start test SSH server");
            running.await.expect("run test SSH server");
        });

        let mut client = client::connect_stream(client_config, gated_client_io, TestClient)
            .await
            .expect("connect test SSH client");
        assert!(
            client
                .authenticate_none("zero-window-test")
                .await
                .expect("authenticate test client")
                .success()
        );
        let mut channel = client
            .channel_open_session()
            .await
            .expect("open test channel");

        read_gate.close();
        start_tx.send(()).unwrap();
        timeout(Duration::from_secs(2), async {
            while polls.load(Ordering::Relaxed) < 5 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("writer did not consume the advertised window");
        sleep(Duration::from_millis(50)).await;
        let settled_polls = polls.load(Ordering::Relaxed);
        sleep(Duration::from_millis(100)).await;
        let blocked_polls = polls.load(Ordering::Relaxed);
        assert!(
            blocked_polls.saturating_sub(settled_polls) <= 1,
            "writer was repeatedly polled at zero window: {settled_polls} -> {blocked_polls}"
        );
        assert!(matches!(
            completed_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        read_gate.open();
        let received = timeout(Duration::from_secs(2), async {
            let mut received = Vec::with_capacity(payload.len());
            while received.len() < payload.len() {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => received.extend_from_slice(&data),
                    Some(ChannelMsg::WindowAdjusted { .. }) => {}
                    Some(other) => panic!("channel closed before payload completed: {other:?}"),
                    None => panic!("channel ended before payload completed"),
                }
            }
            received
        })
        .await
        .expect("writer did not resume after window adjustment");
        assert_eq!(received, payload);
        timeout(Duration::from_secs(2), &mut completed_rx)
            .await
            .expect("server writer did not finish")
            .expect("server writer task dropped")
            .expect("server writer failed");

        client
            .disconnect(Disconnect::ByApplication, "test complete", "en")
            .await
            .unwrap();
        drop(channel);
        drop(client);
        if timeout(Duration::from_secs(2), &mut server_task)
            .await
            .is_err()
        {
            server_task.abort();
            panic!("test SSH server did not stop");
        }
    }

    fn test_metrics() -> Arc<MetricsRegistry> {
        MetricsRegistry::new(MetricsConfig {
            enabled: false,
            db_path: std::env::temp_dir().join(format!(
                "unused-router-ssh-test-metrics-{}.db",
                uuid::Uuid::new_v4()
            )),
            retention_days: 1,
            sample_interval_secs: 1,
            alerting: AlertingSettings::default(),
        })
    }

    async fn tcp_pair() -> (TcpStream, TcpStream, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer_stream = TcpStream::connect(address).await.unwrap();
        let (stream, peer) = listener.accept().await.unwrap();
        (peer_stream, stream, peer)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_test_channel_open(
        handle: &russh::server::Handle,
        metrics: Arc<MetricsRegistry>,
        global: Arc<tokio::sync::Semaphore>,
        tunnel: Arc<tokio::sync::Semaphore>,
        stream: TcpStream,
        peer: SocketAddr,
        open_timeout: Duration,
        shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<ForwardChannelOpenResult> {
        let permits = try_acquire_forward_capacity(&global, &tunnel).unwrap();
        tokio::spawn(open_forward_channel(ForwardChannelOpenContext {
            stream,
            peer,
            permits,
            handle: handle.clone(),
            connected_address: "127.0.0.1".to_string(),
            connected_port: 1,
            timeout: open_timeout,
            shutdown_rx,
            metrics,
        }))
    }

    async fn wait_for_forward_metrics(
        metrics: &Arc<MetricsRegistry>,
        ready: impl Fn(&crate::metrics::models::RouterMetricsStatus) -> bool,
    ) -> crate::metrics::models::RouterMetricsStatus {
        timeout(Duration::from_secs(1), async {
            loop {
                let status = metrics.router_status(&ProxyRegistry::default()).await;
                if ready(&status) {
                    return status;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("forward metrics did not reach the expected state")
    }

    struct ControlledForwardOpen {
        channel: Channel<ClientMsg>,
        reply: ClientChannelOpenHandle,
        originator_port: u32,
    }

    impl ControlledForwardOpen {
        async fn accept(self) -> Channel<ClientMsg> {
            self.reply.accept().await;
            self.channel
        }

        async fn reject(self, reason: ChannelOpenFailure) {
            self.reply.reject(reason).await;
        }
    }

    struct ControlledClient {
        requests: mpsc::UnboundedSender<ControlledForwardOpen>,
    }

    impl client::Handler for ControlledClient {
        type Error = anyhow::Error;

        async fn check_server_key(
            &mut self,
            _server_public_key: &russh::keys::ssh_key::PublicKey,
        ) -> Result<bool, Self::Error> {
            Ok(true)
        }

        async fn server_channel_open_forwarded_tcpip(
            &mut self,
            channel: Channel<ClientMsg>,
            _connected_address: &str,
            _connected_port: u32,
            _originator_address: &str,
            originator_port: u32,
            reply: ClientChannelOpenHandle,
            _session: &mut ClientSession,
        ) -> Result<(), Self::Error> {
            self.requests
                .send(ControlledForwardOpen {
                    channel,
                    reply,
                    originator_port,
                })
                .map_err(|_| anyhow::anyhow!("controlled request receiver closed"))
        }
    }

    struct ControlledServer;

    impl russh::server::Handler for ControlledServer {
        type Error = anyhow::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
            Ok(Auth::Accept)
        }
    }

    struct ControlledSshSession {
        server_handle: russh::server::Handle,
        server_abort: IoAbortHandle,
        server_task: Option<JoinHandle<Result<(), anyhow::Error>>>,
        client: Option<client::Handle<ControlledClient>>,
        requests: mpsc::UnboundedReceiver<ControlledForwardOpen>,
    }

    impl ControlledSshSession {
        async fn start() -> Self {
            let server_config = Arc::new(russh::server::Config {
                keys: vec![
                    russh::keys::PrivateKey::random(
                        &mut rand_0_10::rng(),
                        russh::keys::Algorithm::Ed25519,
                    )
                    .unwrap(),
                ],
                inactivity_timeout: None,
                ..Default::default()
            });
            let client_config = Arc::new(client::Config {
                inactivity_timeout: None,
                ..Default::default()
            });
            let (server_io, client_io) = tokio::io::duplex(1 << 20);
            let (server_io, server_abort) = AbortableIo::new(server_io);
            let (request_tx, requests) = mpsc::unbounded_channel();
            let client_connect = tokio::spawn(client::connect_stream(
                client_config,
                client_io,
                ControlledClient {
                    requests: request_tx,
                },
            ));
            let running = russh::server::run_stream(server_config, server_io, ControlledServer)
                .await
                .expect("start controlled SSH server");
            let server_handle = running.handle();
            let server_task = tokio::spawn(running);
            let mut client = client_connect
                .await
                .expect("controlled SSH client task failed")
                .expect("connect controlled SSH client");
            assert!(
                client
                    .authenticate_none("controlled-forward-test")
                    .await
                    .expect("authenticate controlled SSH client")
                    .success()
            );
            Self {
                server_handle,
                server_abort,
                server_task: Some(server_task),
                client: Some(client),
                requests,
            }
        }

        async fn next_request(&mut self) -> ControlledForwardOpen {
            timeout(Duration::from_secs(1), self.requests.recv())
                .await
                .expect("server did not send a forwarded channel request")
                .expect("controlled SSH client stopped")
        }

        async fn wait_for_stop(&mut self) {
            if let Some(server_task) = self.server_task.as_mut() {
                timeout(Duration::from_secs(1), server_task)
                    .await
                    .expect("controlled SSH server did not stop")
                    .expect("controlled SSH server task panicked")
                    .ok();
            }
            self.server_task = None;
            if let Some(client) = self.client.as_mut() {
                timeout(Duration::from_secs(1), client)
                    .await
                    .expect("controlled SSH client did not stop")
                    .ok();
            }
            self.client = None;
        }

        async fn shutdown(&mut self) {
            if let Some(client) = self.client.as_ref() {
                let _ = client
                    .disconnect(Disconnect::ByApplication, "test complete", "en")
                    .await;
            }
            self.wait_for_stop().await;
        }
    }

    struct ZeroWindowServer {
        start: Option<oneshot::Receiver<()>>,
        completed: Option<oneshot::Sender<std::io::Result<()>>>,
        polls: std::sync::Arc<AtomicUsize>,
        payload: Vec<u8>,
    }

    impl russh::server::Handler for ZeroWindowServer {
        type Error = anyhow::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
            Ok(Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<Msg>,
            reply: ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            let start = self.start.take().expect("one test channel");
            let completed = self.completed.take().expect("one test channel");
            let polls = self.polls.clone();
            let payload = self.payload.clone();
            reply.accept().await;
            tokio::spawn(async move {
                let _ = start.await;
                let mut writer = CountingWriter {
                    inner: channel.make_writer(),
                    polls,
                };
                let result = writer.write_all(&payload).await;
                let _ = completed.send(result);
                drop(channel);
            });
            Ok(())
        }
    }

    struct TestClient;

    impl client::Handler for TestClient {
        type Error = anyhow::Error;

        async fn check_server_key(
            &mut self,
            _server_public_key: &russh::keys::ssh_key::PublicKey,
        ) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    struct CountingWriter<W> {
        inner: W,
        polls: std::sync::Arc<AtomicUsize>,
    }

    impl<W: AsyncWrite + Unpin> AsyncWrite for CountingWriter<W> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.polls.fetch_add(1, Ordering::Relaxed);
            Pin::new(&mut self.inner).poll_write(context, buffer)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(context)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(context)
        }
    }

    struct ReadGate {
        open: AtomicBool,
        waiting: std::sync::Mutex<Option<Waker>>,
    }

    impl ReadGate {
        fn close(&self) {
            self.open.store(false, Ordering::Release);
        }

        fn open(&self) {
            self.open.store(true, Ordering::Release);
            if let Some(waker) = self.waiting.lock().unwrap().take() {
                waker.wake();
            }
        }
    }

    struct GatedIo<T> {
        inner: T,
        read_gate: std::sync::Arc<ReadGate>,
    }

    impl<T> GatedIo<T> {
        fn new(inner: T) -> (Self, std::sync::Arc<ReadGate>) {
            let read_gate = std::sync::Arc::new(ReadGate {
                open: AtomicBool::new(true),
                waiting: std::sync::Mutex::new(None),
            });
            (
                Self {
                    inner,
                    read_gate: read_gate.clone(),
                },
                read_gate,
            )
        }
    }

    impl<T: AsyncRead + Unpin> AsyncRead for GatedIo<T> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if !self.read_gate.open.load(Ordering::Acquire) {
                let mut waiting = self.read_gate.waiting.lock().unwrap();
                if !self.read_gate.open.load(Ordering::Acquire) {
                    *waiting = Some(context.waker().clone());
                    return Poll::Pending;
                }
            }
            Pin::new(&mut self.inner).poll_read(context, buffer)
        }
    }

    impl<T: AsyncWrite + Unpin> AsyncWrite for GatedIo<T> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Pin::new(&mut self.inner).poll_write(context, buffer)
        }

        fn poll_flush(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(context)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(context)
        }
    }
}
