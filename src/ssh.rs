use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use russh::keys::key::KeyPair;
use russh::server::Msg;
use russh::server::{Auth, Session};
use russh::{Channel, ChannelId, server};
use tokio::net::TcpListener;
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
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let Some(lease) = self.lease.as_ref() else {
            return Ok(false);
        };
        let host = normalize_backend_host(address);
        let backend = format!("{host}:{port}");
        self.proxy
            .set_route(lease.subdomain.clone(), backend.clone())
            .await;
        self.backend = Some(backend);
        info!(
            "registered backend for subdomain={} connection_id={} backend={}",
            lease.subdomain, lease.connection_id, host
        );
        Ok(true)
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
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
        if let Some(lease) = self.lease.as_ref() {
            self.proxy.remove_route(&lease.subdomain).await;
        }
        Ok(true)
    }
}

fn normalize_backend_host(address: &str) -> &str {
    match address.trim() {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        value => value,
    }
}
