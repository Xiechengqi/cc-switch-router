use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use russh::keys::key::KeyPair;
use russh::server::Msg;
use russh::server::{Auth, Session};
use russh::{Channel, ChannelId, server};
use tokio::io;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::proxy::ProxyRegistry;
use crate::store::AppStore;

#[derive(Clone)]
pub struct SshServer {
    pub store: AppStore,
    pub proxy: Arc<ProxyRegistry>,
}

#[derive(Clone)]
struct ClientHandler {
    store: AppStore,
    proxy: Arc<ProxyRegistry>,
    lease: Option<crate::models::TunnelLease>,
    backend: Option<String>,
    forward_task: Option<Arc<JoinHandle<()>>>,
}

impl SshServer {
    pub async fn run_with_listener(self, listener: TcpListener) -> Result<()> {
        let mut config = server::Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(300)),
            auth_rejection_time: std::time::Duration::from_secs(1),
            ..Default::default()
        };
        config.keys.push(KeyPair::generate_ed25519());
        let config = Arc::new(config);
        info!("ssh listening on {}", listener.local_addr()?);
        loop {
            let (socket, peer) = listener.accept().await?;
            let config = config.clone();
            let handler = ClientHandler {
                store: self.store.clone(),
                proxy: self.proxy.clone(),
                lease: None,
                backend: None,
                forward_task: None,
            };
            tokio::spawn(async move {
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
        let Some(lease) = self.lease.as_ref() else {
            return Ok(false);
        };
        if let Some(task) = self.forward_task.take() {
            task.abort();
        }

        let host = normalize_backend_host(address);
        let listener = match TcpListener::bind((host, *port as u16)).await {
            Ok(listener) => listener,
            Err(err) => {
                error!("failed to bind forwarded port {}:{}: {}", host, *port, err);
                return Ok(false);
            }
        };
        let bound_port = listener.local_addr()?.port();
        *port = bound_port as u32;
        let backend = format!("{host}:{port}");
        let share_token = lease.share.as_ref().map(|s| s.share_token.clone());
        self.proxy
            .set_route(lease.subdomain.clone(), backend.clone(), share_token)
            .await;
        self.backend = Some(backend.clone());
        let handle = session.handle();
        let connected_address = address.to_string();
        let proxy = self.proxy.clone();
        let subdomain = lease.subdomain.clone();
        let connection_id = lease.connection_id.clone();
        let task = tokio::spawn(async move {
            if let Err(err) = serve_forward_listener(
                listener,
                handle,
                connected_address,
                bound_port,
                proxy,
                subdomain,
                connection_id,
            )
            .await
            {
                error!("forward listener failed on port {}: {}", bound_port, err);
            }
        });
        self.forward_task = Some(Arc::new(task));
        info!(
            "registered backend for subdomain={} connection_id={} backend={}",
            lease.subdomain, lease.connection_id, backend
        );
        Ok(true)
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(task) = self.forward_task.take() {
            task.abort();
        }
        if let Some(lease) = self.lease.as_ref() {
            self.proxy.remove_route(&lease.subdomain).await;
        }
        Ok(())
    }

    async fn cancel_tcpip_forward(
        &mut self,
        _address: &str,
        _port: u32,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        if let Some(task) = self.forward_task.take() {
            task.abort();
        }
        if let Some(lease) = self.lease.as_ref() {
            self.proxy.remove_route(&lease.subdomain).await;
        }
        Ok(true)
    }
}

async fn serve_forward_listener(
    listener: TcpListener,
    handle: russh::server::Handle,
    connected_address: String,
    connected_port: u16,
    proxy: Arc<ProxyRegistry>,
    subdomain: String,
    connection_id: String,
) -> Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
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
                proxy.remove_route(&subdomain).await;
                error!(
                    "failed to open forwarded tcp channel: {} subdomain={} connection_id={}, route removed",
                    err, subdomain, connection_id
                );
                return Ok(());
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
