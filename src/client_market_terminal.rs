//! Client Market Web Terminal: short-lived tickets + WebSocket ↔ OpenSSH PTY bridge.
//! Protocol is a simplified gotty/webtty subset (input/output/resize/ping).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use tracing::{info, warn};
use uuid::Uuid;

use crate::ServerState;
use crate::client_market::{RouterSshHostRecord, known_hosts_path};
use crate::error::AppError;

const MSG_INPUT: u8 = b'1';
const MSG_PING: u8 = b'2';
const MSG_RESIZE: u8 = b'3';
const MSG_OUTPUT: u8 = b'1';
const MSG_PONG: u8 = b'2';

const TICKET_TTL: Duration = Duration::from_secs(60);
const MAX_SESSIONS_PER_OWNER: usize = 2;
const IDLE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const MAX_SESSION_DURATION: Duration = Duration::from_secs(2 * 60 * 60);
const PTY_READ_CHUNK: usize = 8192;

#[derive(Debug, Clone)]
struct TerminalTicket {
    host_id: String,
    owner_email: String,
    ip: String,
    port: u16,
    expires_at: Instant,
}

#[derive(Debug, Default)]
pub struct TerminalSessionManager {
    tickets: HashMap<String, TerminalTicket>,
    /// owner_email -> active websocket session count
    active_sessions: HashMap<String, usize>,
}

impl TerminalSessionManager {
    fn prune_tickets(&mut self) {
        let now = Instant::now();
        self.tickets.retain(|_, ticket| ticket.expires_at > now);
    }

    fn issue_ticket(&mut self, ticket: TerminalTicket) -> String {
        self.prune_tickets();
        let id = Uuid::new_v4().to_string();
        self.tickets.insert(id.clone(), ticket);
        id
    }

    fn redeem_ticket(&mut self, ticket_id: &str) -> Result<TerminalTicket, AppError> {
        self.prune_tickets();
        let ticket = self
            .tickets
            .remove(ticket_id)
            .ok_or_else(|| AppError::Unauthorized("terminal ticket not found or expired".into()))?;
        if ticket.expires_at <= Instant::now() {
            return Err(AppError::Unauthorized(
                "terminal ticket not found or expired".into(),
            ));
        }
        Ok(ticket)
    }

    fn try_begin_session(&mut self, owner_email: &str) -> Result<(), AppError> {
        let count = self.active_sessions.get(owner_email).copied().unwrap_or(0);
        if count >= MAX_SESSIONS_PER_OWNER {
            return Err(AppError::TooManyRequests(
                "too many active web terminal sessions".into(),
            ));
        }
        self.active_sessions
            .insert(owner_email.to_string(), count + 1);
        Ok(())
    }

    fn end_session(&mut self, owner_email: &str) {
        let Some(count) = self.active_sessions.get_mut(owner_email) else {
            return;
        };
        if *count <= 1 {
            self.active_sessions.remove(owner_email);
        } else {
            *count -= 1;
        }
    }
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/client-market/hosts/:id/terminal-session",
            post(create_terminal_session),
        )
        .route("/v1/client-market/terminal/ws", get(terminal_ws))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalSessionResponse {
    ticket: String,
    expires_in_sec: u64,
}

#[derive(Debug, Deserialize)]
struct TerminalWsQuery {
    ticket: String,
}

async fn create_terminal_session(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(host_id): AxumPath<String>,
) -> Result<Json<TerminalSessionResponse>, AppError> {
    let viewer = require_session_email(&state, &headers).await?;
    let is_admin = state.dynamic.read().await.is_admin(&viewer);
    let host = state
        .store
        .client_market_get_host(&host_id)
        .await?
        .ok_or_else(|| AppError::NotFound("host not found".into()))?;
    authorize_web_terminal(&host, &viewer, is_admin)?;

    let mut manager = state.client_market_terminal.lock().await;
    let ticket = manager.issue_ticket(TerminalTicket {
        host_id: host.id.clone(),
        owner_email: viewer.clone(),
        ip: host.ip.clone(),
        port: host.port,
        expires_at: Instant::now() + TICKET_TTL,
    });
    info!(
        host_id = %host.id,
        owner = %viewer,
        "client market terminal session ticket issued"
    );
    Ok(Json(TerminalSessionResponse {
        ticket,
        expires_in_sec: TICKET_TTL.as_secs(),
    }))
}

async fn terminal_ws(
    ws: WebSocketUpgrade,
    State(state): State<ServerState>,
    Query(query): Query<TerminalWsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let ticket = {
        let mut manager = state.client_market_terminal.lock().await;
        let ticket = manager.redeem_ticket(query.ticket.trim())?;
        manager.try_begin_session(&ticket.owner_email)?;
        ticket
    };

    Ok(ws
        .protocols(["webtty"])
        .on_upgrade(move |socket| async move {
            run_terminal_session(state, socket, ticket).await;
        }))
}

async fn run_terminal_session(state: ServerState, socket: WebSocket, ticket: TerminalTicket) {
    let owner = ticket.owner_email.clone();
    let host_id = ticket.host_id.clone();
    let started = Instant::now();
    info!(host_id = %host_id, owner = %owner, "client market terminal session started");

    let result = bridge_ssh_session(&state, socket, &ticket).await;
    if let Err(error) = result {
        warn!(
            host_id = %host_id,
            owner = %owner,
            error = %error,
            "client market terminal session ended with error"
        );
    }

    state
        .client_market_terminal
        .lock()
        .await
        .end_session(&owner);
    info!(
        host_id = %host_id,
        owner = %owner,
        duration_ms = started.elapsed().as_millis() as u64,
        "client market terminal session ended"
    );
}

async fn bridge_ssh_session(
    state: &ServerState,
    socket: WebSocket,
    ticket: &TerminalTicket,
) -> Result<(), String> {
    let known_hosts = known_hosts_path(&state.config);
    if let Some(parent) = known_hosts.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create known_hosts directory failed: {e}"))?;
    }

    let pty_system = NativePtySystem::default();
    let pair = match pty_system.openpty(PtySize {
        rows: 32,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(e) => {
            let _ = send_ws_notice(socket, &format!("open pty failed: {e}")).await;
            return Err(format!("open pty failed: {e}"));
        }
    };

    let mut cmd = CommandBuilder::new("ssh");
    cmd.arg("-F");
    cmd.arg("/dev/null");
    cmd.arg("-tt");
    cmd.arg("-i");
    cmd.arg(state.provision_ssh_key_path.as_os_str());
    cmd.arg("-p");
    cmd.arg(ticket.port.to_string());
    cmd.arg("-o");
    cmd.arg("BatchMode=yes");
    cmd.arg("-o");
    cmd.arg("IdentitiesOnly=yes");
    cmd.arg("-o");
    cmd.arg("PasswordAuthentication=no");
    cmd.arg("-o");
    cmd.arg("KbdInteractiveAuthentication=no");
    cmd.arg("-o");
    cmd.arg("ChallengeResponseAuthentication=no");
    cmd.arg("-o");
    cmd.arg("PreferredAuthentications=publickey");
    cmd.arg("-o");
    cmd.arg("StrictHostKeyChecking=yes");
    cmd.arg("-o");
    cmd.arg(format!("UserKnownHostsFile={}", known_hosts.display()));
    cmd.arg("-o");
    cmd.arg("GlobalKnownHostsFile=/dev/null");
    cmd.arg("-o");
    cmd.arg("UpdateHostKeys=no");
    cmd.arg("-o");
    cmd.arg("ConnectTimeout=30");
    cmd.arg("-o");
    cmd.arg("ServerAliveInterval=15");
    cmd.arg("-o");
    cmd.arg("ServerAliveCountMax=4");
    cmd.arg("-o");
    cmd.arg("LogLevel=ERROR");
    cmd.arg(format!("root@{}", ticket.ip));

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(child) => child,
        Err(e) => {
            let _ = send_ws_notice(socket, &format!("spawn ssh failed: {e}")).await;
            return Err(format!("spawn ssh failed: {e}"));
        }
    };
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone pty reader failed: {e}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take pty writer failed: {e}"))?;
    let master = Arc::new(Mutex::new(pair.master));

    let (pty_tx, mut pty_rx) = mpsc::channel::<Vec<u8>>(64);
    let reader_task = tokio::task::spawn_blocking(move || {
        let mut buf = [0_u8; PTY_READ_CHUNK];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if pty_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let (mut ws_tx, mut ws_rx) = socket.split();
    let session_deadline = Instant::now() + MAX_SESSION_DURATION;
    let mut last_activity = Instant::now();

    let bridge_result = async {
        loop {
            let idle = IDLE_TIMEOUT.saturating_sub(last_activity.elapsed());
            let until_max = session_deadline.saturating_duration_since(Instant::now());
            let wait = idle.min(until_max);
            if wait.is_zero() {
                return Err("web terminal session timed out".to_string());
            }

            tokio::select! {
                biased;
                chunk = pty_rx.recv() => {
                    match chunk {
                        Some(bytes) => {
                            last_activity = Instant::now();
                            let payload = encode_output(&bytes);
                            ws_tx
                                .send(Message::Text(payload))
                                .await
                                .map_err(|e| format!("websocket send failed: {e}"))?;
                        }
                        None => return Ok(()),
                    }
                }
                msg = ws_rx.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            last_activity = Instant::now();
                            if text.as_bytes().first() == Some(&MSG_PING) {
                                ws_tx
                                    .send(Message::Text(String::from(MSG_PONG as char)))
                                    .await
                                    .map_err(|e| format!("websocket pong failed: {e}"))?;
                            } else {
                                handle_client_message(&text, &mut writer, &master).await?;
                            }
                        }
                        Some(Ok(Message::Binary(bin))) => {
                            last_activity = Instant::now();
                            let text = String::from_utf8_lossy(&bin);
                            if text.as_bytes().first() == Some(&MSG_PING) {
                                ws_tx
                                    .send(Message::Text(String::from(MSG_PONG as char)))
                                    .await
                                    .map_err(|e| format!("websocket pong failed: {e}"))?;
                            } else {
                                handle_client_message(&text, &mut writer, &master).await?;
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            ws_tx
                                .send(Message::Pong(data))
                                .await
                                .map_err(|e| format!("websocket pong failed: {e}"))?;
                        }
                        Some(Ok(Message::Close(_))) | None => return Ok(()),
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Err(e)) => return Err(format!("websocket receive failed: {e}")),
                    }
                }
            }
        }
    }
    .await;

    let _ = child.kill();
    let _ = child.wait();
    drop(writer);
    reader_task.abort();
    let _ = ws_tx.send(Message::Close(None)).await;

    bridge_result
}

async fn handle_client_message(
    text: &str,
    writer: &mut Box<dyn Write + Send>,
    master: &Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
) -> Result<(), String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Ok(());
    }
    match bytes[0] {
        MSG_INPUT => {
            if bytes.len() == 1 {
                return Ok(());
            }
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&bytes[1..])
                .map_err(|e| format!("decode terminal input failed: {e}"))?;
            writer
                .write_all(&decoded)
                .map_err(|e| format!("write pty failed: {e}"))?;
            writer.flush().map_err(|e| format!("flush pty failed: {e}"))?;
        }
        MSG_RESIZE => {
            if bytes.len() == 1 {
                return Ok(());
            }
            #[derive(Deserialize)]
            struct ResizeArgs {
                columns: u16,
                rows: u16,
            }
            let args: ResizeArgs = serde_json::from_slice(&bytes[1..])
                .map_err(|e| format!("invalid resize payload: {e}"))?;
            if args.columns == 0 || args.rows == 0 {
                return Ok(());
            }
            let guard = master.lock().await;
            guard
                .resize(PtySize {
                    rows: args.rows,
                    cols: args.columns,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| format!("resize pty failed: {e}"))?;
        }
        _ => {}
    }
    Ok(())
}

fn encode_output(data: &[u8]) -> String {
    let mut out = String::with_capacity(1 + data.len() * 4 / 3 + 4);
    out.push(MSG_OUTPUT as char);
    out.push_str(&base64::engine::general_purpose::STANDARD.encode(data));
    out
}

async fn send_ws_notice(mut socket: WebSocket, message: &str) -> Result<(), String> {
    let payload = encode_output(format!("\r\n{message}\r\n").as_bytes());
    socket
        .send(Message::Text(payload))
        .await
        .map_err(|e| format!("websocket notice failed: {e}"))?;
    let _ = socket.send(Message::Close(None)).await;
    Ok(())
}

fn authorize_web_terminal(
    host: &RouterSshHostRecord,
    viewer_email: &str,
    is_admin: bool,
) -> Result<(), AppError> {
    if is_admin {
        return Ok(());
    }
    let viewer = viewer_email.trim().to_ascii_lowercase();
    let is_host_owner = host.host_owner_email.trim().to_ascii_lowercase() == viewer;
    if is_host_owner {
        return Ok(());
    }
    Err(AppError::Forbidden(
        "web terminal is only available to the host owner".into(),
    ))
}

async fn require_session_email(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<String, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .map(|session| session.email)
        .ok_or_else(|| AppError::Unauthorized("authenticated owner session required".into()))
}

/// Re-export path helper used when constructing ssh options in tests/docs.
#[allow(dead_code)]
fn provision_ssh_target(ip: &str, port: u16) -> String {
    format!("root@{ip}:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_host(client_owner: Option<&str>, installation: Option<&str>) -> RouterSshHostRecord {
        RouterSshHostRecord {
            id: "host-1".into(),
            ip: "203.0.113.10".into(),
            port: 22,
            host_owner_email: "host@example.com".into(),
            country_code: Some("US".into()),
            hostname: Some("box".into()),
            ssh_host_key_fingerprint: None,
            status: "allocated".into(),
            client_subdomain: Some("demo".into()),
            client_owner_email: client_owner.map(str::to_string),
            installation_id: installation.map(str::to_string),
            last_verified_at: None,
            last_error: None,
            note: None,
            ip_intel_json: None,
            created_at: "t0".into(),
            updated_at: "t0".into(),
        }
    }

    #[test]
    fn authorize_allows_host_owner_only() {
        let host = sample_host(Some("client@example.com"), Some("inst-1"));
        assert!(authorize_web_terminal(&host, "host@example.com", false).is_ok());
        assert!(authorize_web_terminal(&host, "HOST@example.com", false).is_ok());
        assert!(authorize_web_terminal(&host, "client@example.com", false).is_err());
        assert!(authorize_web_terminal(&host, "other@example.com", false).is_err());
        assert!(authorize_web_terminal(&host, "admin@example.com", true).is_ok());

        let idle = sample_host(None, None);
        assert!(authorize_web_terminal(&idle, "host@example.com", false).is_ok());
        assert!(authorize_web_terminal(&idle, "client@example.com", false).is_err());
    }

    #[test]
    fn ticket_is_single_use_and_expires() {
        let mut manager = TerminalSessionManager::default();
        let id = manager.issue_ticket(TerminalTicket {
            host_id: "h".into(),
            owner_email: "a@b.co".into(),
            ip: "1.2.3.4".into(),
            port: 22,
            expires_at: Instant::now() + Duration::from_secs(30),
        });
        assert!(manager.redeem_ticket(&id).is_ok());
        assert!(manager.redeem_ticket(&id).is_err());

        let expired = manager.issue_ticket(TerminalTicket {
            host_id: "h".into(),
            owner_email: "a@b.co".into(),
            ip: "1.2.3.4".into(),
            port: 22,
            expires_at: Instant::now() - Duration::from_secs(1),
        });
        assert!(manager.redeem_ticket(&expired).is_err());
    }

    #[test]
    fn session_concurrency_limit_enforced() {
        let mut manager = TerminalSessionManager::default();
        assert!(manager.try_begin_session("a@b.co").is_ok());
        assert!(manager.try_begin_session("a@b.co").is_ok());
        assert!(manager.try_begin_session("a@b.co").is_err());
        manager.end_session("a@b.co");
        assert!(manager.try_begin_session("a@b.co").is_ok());
    }

    #[test]
    fn encode_output_uses_webtty_prefix() {
        let encoded = encode_output(b"hi");
        assert_eq!(encoded.as_bytes()[0], MSG_OUTPUT);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded.as_bytes()[1..])
            .unwrap();
        assert_eq!(decoded, b"hi");
    }
}
