//! Client Market Web Terminal: short-lived tickets + WebSocket ↔ OpenSSH PTY bridge.
//! Protocol is a simplified gotty/webtty subset (input/output/resize/ping).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine;
use chrono::{DateTime, Utc};
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
const AUTHORIZATION_RECHECK_INTERVAL: Duration = Duration::from_secs(10);
const PTY_READ_CHUNK: usize = 8192;
const WEBTTY_PROTOCOL: &str = "webtty";
const TICKET_PROTOCOL_PREFIX: &str = "ticket.";

#[derive(Debug, Clone)]
struct TerminalTicket {
    host_id: String,
    installation_id: Option<String>,
    owner_user_id: String,
    /// Carried purely so the audit trail names a human, not just an opaque id.
    owner_email: String,
    ip: String,
    port: u16,
    expires_at: Instant,
    authorization_expires_at: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct TerminalSessionManager {
    tickets: HashMap<String, TerminalTicket>,
    /// Stable owner user id -> active websocket session count.
    active_sessions: HashMap<String, usize>,
}

struct ActiveTerminalSession {
    manager: Arc<Mutex<TerminalSessionManager>>,
    owner_user_id: Option<String>,
}

impl ActiveTerminalSession {
    fn new(manager: Arc<Mutex<TerminalSessionManager>>, owner_user_id: String) -> Self {
        Self {
            manager,
            owner_user_id: Some(owner_user_id),
        }
    }
}

impl Drop for ActiveTerminalSession {
    fn drop(&mut self) {
        let Some(owner_user_id) = self.owner_user_id.take() else {
            return;
        };
        let manager = Arc::clone(&self.manager);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                manager.lock().await.end_session(&owner_user_id);
            });
        }
    }
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

    fn try_begin_session(&mut self, owner_user_id: &str) -> Result<(), AppError> {
        let count = self
            .active_sessions
            .get(owner_user_id)
            .copied()
            .unwrap_or(0);
        if count >= MAX_SESSIONS_PER_OWNER {
            return Err(AppError::TooManyRequests(
                "too many active web terminal sessions".into(),
            ));
        }
        self.active_sessions
            .insert(owner_user_id.to_string(), count + 1);
        Ok(())
    }

    fn end_session(&mut self, owner_user_id: &str) {
        let Some(count) = self.active_sessions.get_mut(owner_user_id) else {
            return;
        };
        if *count <= 1 {
            self.active_sessions.remove(owner_user_id);
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

async fn create_terminal_session(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(host_id): AxumPath<String>,
) -> Result<Json<TerminalSessionResponse>, AppError> {
    let session = require_session(&state, &headers).await?;
    let host = state
        .store
        .client_market_get_host(&host_id)
        .await?
        .ok_or_else(|| AppError::NotFound("host not found".into()))?;
    authorize_web_terminal(&state, &host, &session).await?;
    let host = crate::client_market::prepare_web_terminal_host(&state, &host).await?;
    let authorization_expires_at = authorize_web_terminal(&state, &host, &session)
        .await?
        .and_then(|expires_at| {
            (expires_at - Utc::now())
                .to_std()
                .ok()
                .map(|remaining| Instant::now() + remaining)
        });

    let mut manager = state.client_market_terminal.lock().await;
    let ticket = manager.issue_ticket(TerminalTicket {
        host_id: host.id.clone(),
        installation_id: host.installation_id.clone(),
        owner_user_id: session.user_id.clone(),
        owner_email: session.email.clone(),
        ip: host.ip.clone(),
        port: host.port,
        expires_at: Instant::now() + TICKET_TTL,
        authorization_expires_at,
    });
    info!(
        host_id = %host.id,
        owner_user_id = %session.user_id,
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
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let ticket_id = terminal_ticket_from_protocols(&headers)?;
    let ticket = {
        let mut manager = state.client_market_terminal.lock().await;
        manager.redeem_ticket(&ticket_id)?
    };

    Ok(ws
        .protocols([WEBTTY_PROTOCOL])
        .on_upgrade(move |socket| async move {
            let session_result = {
                let mut manager = state.client_market_terminal.lock().await;
                manager.try_begin_session(&ticket.owner_user_id)
            };
            if let Err(error) = session_result {
                let _ = send_ws_notice(socket, &error.to_string()).await;
                return;
            }
            let lease = ActiveTerminalSession::new(
                Arc::clone(&state.client_market_terminal),
                ticket.owner_user_id.clone(),
            );
            run_terminal_session(state, socket, ticket, lease).await;
        }))
}

fn terminal_ticket_from_protocols(headers: &HeaderMap) -> Result<String, AppError> {
    let mut has_webtty = false;
    let mut ticket_id = None;
    for value in headers.get_all(header::SEC_WEBSOCKET_PROTOCOL) {
        let value = value
            .to_str()
            .map_err(|_| AppError::BadRequest("invalid websocket protocol header".into()))?;
        for protocol in value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if protocol == WEBTTY_PROTOCOL {
                has_webtty = true;
                continue;
            }
            if let Some(candidate) = protocol.strip_prefix(TICKET_PROTOCOL_PREFIX) {
                if ticket_id.is_some() || Uuid::parse_str(candidate).is_err() {
                    return Err(AppError::BadRequest(
                        "invalid terminal ticket protocol".into(),
                    ));
                }
                ticket_id = Some(candidate.to_string());
            }
        }
    }
    if !has_webtty {
        return Err(AppError::BadRequest(
            "webtty websocket protocol is required".into(),
        ));
    }
    ticket_id.ok_or_else(|| AppError::Unauthorized("terminal ticket is required".into()))
}

async fn run_terminal_session(
    state: ServerState,
    socket: WebSocket,
    ticket: TerminalTicket,
    _lease: ActiveTerminalSession,
) {
    let owner = ticket.owner_user_id.clone();
    let owner_email = ticket.owner_email.clone();
    let host_id = ticket.host_id.clone();
    let started = Instant::now();
    info!(host_id = %host_id, owner = %owner, "client market terminal session started");

    // The terminal hands out an unrestricted root shell with no transcript. A durable
    // start/end record is the minimum needed to answer "who was on this box, when".
    if let Err(error) = state
        .store
        .client_market_record_audit_event(
            ticket.installation_id.as_deref(),
            Some(&host_id),
            Some(&owner),
            Some(&owner_email),
            "terminal_session_started",
            serde_json::json!({ "hostIp": ticket.ip, "hostPort": ticket.port }),
        )
        .await
    {
        warn!(host_id = %host_id, error = %error, "failed to record terminal session start");
    }

    let result = bridge_ssh_session(&state, socket, &ticket).await;
    let failure = match &result {
        Err(error) => {
            warn!(
                host_id = %host_id,
                owner = %owner,
                error = %error,
                "client market terminal session ended with error"
            );
            Some(error.clone())
        }
        Ok(()) => None,
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    if let Err(error) = state
        .store
        .client_market_record_audit_event(
            ticket.installation_id.as_deref(),
            Some(&host_id),
            Some(&owner),
            Some(&owner_email),
            "terminal_session_ended",
            serde_json::json!({ "durationMs": duration_ms, "error": failure }),
        )
        .await
    {
        warn!(host_id = %host_id, error = %error, "failed to record terminal session end");
    }
    info!(
        host_id = %host_id,
        owner = %owner,
        duration_ms,
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
    let session_deadline = ticket
        .authorization_expires_at
        .map(|expires_at| expires_at.min(Instant::now() + MAX_SESSION_DURATION))
        .unwrap_or_else(|| Instant::now() + MAX_SESSION_DURATION);
    let mut last_activity = Instant::now();
    let mut authorization_timer = tokio::time::interval(AUTHORIZATION_RECHECK_INTERVAL);
    authorization_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    authorization_timer.tick().await;

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
                _ = authorization_timer.tick(), if ticket.installation_id.is_some() => {
                    let installation_id = ticket.installation_id.as_deref().unwrap_or_default();
                    let authorized = state
                        .store
                        .client_market_provider_terminal_authorized_until(
                            installation_id,
                            &ticket.host_id,
                            &ticket.owner_user_id,
                        )
                        .await
                        .map_err(|error| format!("check terminal authorization failed: {error}"))?
                        .is_some();
                    if !authorized {
                        return Err("renter authorization for Provider terminal access ended".into());
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
            writer
                .flush()
                .map_err(|e| format!("flush pty failed: {e}"))?;
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

async fn authorize_web_terminal(
    state: &ServerState,
    host: &RouterSshHostRecord,
    session: &crate::models::AuthSession,
) -> Result<Option<DateTime<Utc>>, AppError> {
    if !crate::client_market::session_is_host_owner(session, host.provider_id.as_deref()) {
        return Err(AppError::Forbidden(
            "web terminal is only available to the host Provider".into(),
        ));
    }
    if crate::client_market::host_is_unallocated_for_terminal(host) {
        return Ok(None);
    }
    let installation_id = host.installation_id.as_deref().ok_or_else(|| {
        AppError::Forbidden(
            "web terminal is unavailable while this Host is being allocated or cleaned".into(),
        )
    })?;
    state
        .store
        .client_market_provider_terminal_authorized_until(
            installation_id,
            &host.id,
            &session.user_id,
        )
        .await?
        .map(Some)
        .ok_or_else(|| {
            AppError::Forbidden(
                "the renter has not authorized Provider terminal access to this Host".into(),
            )
        })
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::models::AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
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
            provider_id: Some("provider-1".into()),
            ip: "203.0.113.10".into(),
            port: 22,
            host_owner_email: "host@example.com".into(),
            daily_rate_minor: Some(500),
            currency: Some("USD".into()),
            free_duration_days: None,
            offer_revision: 1,
            payment_method_kinds: vec!["alipay".into()],
            contacts: vec![],
            country_code: Some("US".into()),
            hostname: Some("box".into()),
            ssh_host_key_fingerprint: None,
            status: if installation.is_some() {
                "allocated".into()
            } else {
                "idle".into()
            },
            client_subdomain: Some("demo".into()),
            client_owner_email: client_owner.map(str::to_string),
            installation_id: installation.map(str::to_string),
            client_owner_user_id: client_owner.map(|_| "client-1".to_string()),
            last_verified_at: None,
            last_error: None,
            note: None,
            ip_intel_json: None,
            created_at: "t0".into(),
            updated_at: "t0".into(),
        }
    }

    fn session(user_id: &str, email: &str) -> crate::models::AuthSession {
        use chrono::Utc;
        crate::models::AuthSession {
            session_id: "s".into(),
            user_id: user_id.into(),
            email: email.into(),
            auth_source_kind: "auth_device".into(),
            auth_source_id: String::new(),
            access_token_hash: String::new(),
            refresh_token_hash: String::new(),
            access_expires_at: Utc::now(),
            refresh_expires_at: Utc::now(),
            created_at: Utc::now(),
            last_used_at: Utc::now(),
        }
    }

    #[test]
    fn unallocated_terminal_access_uses_stable_provider_identity() {
        let idle = sample_host(None, None);
        assert!(crate::client_market::host_is_unallocated_for_terminal(
            &idle
        ));
        assert!(crate::client_market::session_is_host_owner(
            &session("provider-1", "host@example.com"),
            idle.provider_id.as_deref(),
        ));
        assert!(!crate::client_market::session_is_host_owner(
            &session("uuid-host", "host@example.com"),
            idle.provider_id.as_deref(),
        ));

        let allocated = sample_host(Some("client@example.com"), Some("inst-1"));
        assert!(!crate::client_market::host_is_unallocated_for_terminal(
            &allocated
        ));
    }

    #[test]
    fn ticket_is_single_use_and_expires() {
        let mut manager = TerminalSessionManager::default();
        let id = manager.issue_ticket(TerminalTicket {
            host_id: "h".into(),
            installation_id: None,
            owner_user_id: "user-a".into(),
            owner_email: "user-a@example.com".into(),
            ip: "1.2.3.4".into(),
            port: 22,
            expires_at: Instant::now() + Duration::from_secs(30),
            authorization_expires_at: None,
        });
        assert!(manager.redeem_ticket(&id).is_ok());
        assert!(manager.redeem_ticket(&id).is_err());

        let expired = manager.issue_ticket(TerminalTicket {
            host_id: "h".into(),
            installation_id: None,
            owner_user_id: "user-a".into(),
            owner_email: "user-a@example.com".into(),
            ip: "1.2.3.4".into(),
            port: 22,
            expires_at: Instant::now() - Duration::from_secs(1),
            authorization_expires_at: None,
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

    #[tokio::test]
    async fn session_lease_releases_counter_when_dropped() {
        let manager = Arc::new(Mutex::new(TerminalSessionManager::default()));
        manager
            .lock()
            .await
            .try_begin_session("provider-1")
            .unwrap();
        let lease = ActiveTerminalSession::new(Arc::clone(&manager), "provider-1".into());
        drop(lease);
        tokio::task::yield_now().await;
        for _ in 0..10 {
            if !manager
                .lock()
                .await
                .active_sessions
                .contains_key("provider-1")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("terminal session counter was not released");
    }

    #[test]
    fn websocket_ticket_is_carried_in_subprotocol_header() {
        let ticket = Uuid::new_v4().to_string();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            format!("webtty, ticket.{ticket}").parse().unwrap(),
        );
        assert_eq!(terminal_ticket_from_protocols(&headers).unwrap(), ticket);

        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            "webtty, ticket.not-a-uuid".parse().unwrap(),
        );
        assert!(terminal_ticket_from_protocols(&headers).is_err());
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
