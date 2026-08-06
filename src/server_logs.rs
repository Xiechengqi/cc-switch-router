use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::ServerState;
use crate::error::AppError;

pub const INSTALLATION_LOG_BATCH_ACTION: &str = "installation_log_batch_v1";
const LOG_BATCH_BODY_LIMIT_BYTES: usize = 256 * 1024;
const MAX_BATCH_EVENTS: usize = 200;
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
const MAX_TARGET_BYTES: usize = 512;
const MAX_FIELDS_BYTES: usize = 32 * 1024;
const MAX_RAW_LINE_BYTES: usize = 32 * 1024;
const MAX_RETAINED_LINES_PER_CLIENT: usize = 100;
const PUBLIC_VISIBLE_LINES_PER_CLIENT: usize = 10;
const DEFAULT_POLL_INTERVAL_SECONDS: u32 = 3;
const CURSOR_KEY_FILE: &str = ".cursor-key";
const EVENTS_FILE: &str = "events.jsonl";

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct ServerLogRuntimeConfig {
    pub enabled: bool,
    pub root: PathBuf,
}

impl ServerLogRuntimeConfig {
    fn from_env(data_dir: &Path) -> Result<Self, AppError> {
        let enabled = env_bool("CC_SWITCH_ROUTER_SERVER_LOG_INGEST_ENABLED", true);
        let root = std::env::var("CC_SWITCH_ROUTER_SERVER_LOG_DATA_DIR")
            .ok()
            .and_then(|value| {
                let value = value.trim();
                (!value.is_empty()).then(|| PathBuf::from(value))
            })
            .unwrap_or_else(|| data_dir.join("server-logs"));
        Ok(Self { enabled, root })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationLogEvent {
    pub sequence: u64,
    pub occurred_at_ms: i64,
    pub level: String,
    pub target: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_line: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationLogBatchPayload {
    pub protocol_version: u8,
    pub stream_id: String,
    pub server_version: String,
    pub commit_id: String,
    pub events: Vec<InstallationLogEvent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationLogBatchRequest {
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub payload: InstallationLogBatchPayload,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallationLogBatchResponse {
    ok: bool,
    accepted: usize,
    next_sequence: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredServerLogEvent {
    event_id: String,
    installation_id: String,
    client_alias: String,
    stream_id: String,
    sequence: u64,
    occurred_at_ms: i64,
    received_at_ms: i64,
    level: String,
    target: String,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw_line: Option<String>,
    fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    server_version: String,
    commit_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamCursorFile {
    installation_id: String,
    stream_id: String,
    last_sequence: u64,
    updated_at_ms: i64,
}

#[derive(Debug, Clone, Default)]
struct StreamState {
    last_sequence: u64,
}

#[derive(Debug, Default)]
struct Catalog {
    streams: HashMap<(String, String), StreamState>,
    last_received_at_ms: i64,
}

pub struct ServerLogStore {
    config: ServerLogRuntimeConfig,
    catalog: Mutex<Catalog>,
    cursor_key: [u8; 32],
    ingest_slots: Arc<Semaphore>,
    query_slots: Arc<Semaphore>,
}

impl ServerLogStore {
    pub fn from_env(data_dir: &Path) -> Result<Self, AppError> {
        Self::open(ServerLogRuntimeConfig::from_env(data_dir)?)
    }

    #[cfg(test)]
    pub fn disabled_for_tests(root: PathBuf) -> Self {
        Self::open(ServerLogRuntimeConfig {
            enabled: false,
            root,
        })
        .expect("create disabled server log store")
    }

    fn open(config: ServerLogRuntimeConfig) -> Result<Self, AppError> {
        create_private_directory(&config.root).map_err(|error| {
            AppError::Internal(format!(
                "create server log directory failed: {error}: {}",
                config.root.display()
            ))
        })?;
        let cursor_key = load_or_create_cursor_key(&config.root)?;
        let mut catalog = Catalog::default();
        load_stream_cursors(&config.root, &mut catalog)?;
        let recovered_cursors = recover_catalog_from_event_files(&config.root, &mut catalog)?;
        persist_recovered_stream_cursors(&config.root, &catalog, recovered_cursors)?;
        Ok(Self {
            config,
            catalog: Mutex::new(catalog),
            cursor_key,
            ingest_slots: Arc::new(Semaphore::new(4)),
            query_slots: Arc::new(Semaphore::new(4)),
        })
    }

    pub(crate) fn installation_ids_with_log_state(&self) -> Result<HashSet<String>, AppError> {
        let catalog = self
            .catalog
            .lock()
            .map_err(|_| AppError::Internal("server log catalog lock poisoned".into()))?;
        Ok(catalog
            .streams
            .keys()
            .map(|(installation_id, _)| installation_id.clone())
            .collect())
    }

    pub(crate) async fn remove_installations(
        self: &Arc<Self>,
        installation_ids: HashSet<String>,
    ) -> Result<usize, AppError> {
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || store.remove_installations_sync(&installation_ids))
            .await
            .map_err(|error| {
                AppError::Internal(format!("join server log orphan cleanup failed: {error}"))
            })?
    }

    fn remove_installations_sync(
        &self,
        installation_ids: &HashSet<String>,
    ) -> Result<usize, AppError> {
        let mut catalog = self
            .catalog
            .lock()
            .map_err(|_| AppError::Internal("server log catalog lock poisoned".into()))?;
        let mut removed = 0;
        for installation_id in installation_ids {
            if !catalog
                .streams
                .keys()
                .any(|(candidate, _)| candidate == installation_id)
            {
                continue;
            }
            match fs::remove_dir_all(installation_directory(&self.config.root, installation_id)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(AppError::Internal(format!(
                        "remove orphaned server log installation failed: {error}"
                    )));
                }
            }
            catalog
                .streams
                .retain(|(candidate, _), _| candidate != installation_id);
            removed += 1;
        }
        Ok(removed)
    }

    async fn ingest(
        self: &Arc<Self>,
        installation_id: String,
        payload: InstallationLogBatchPayload,
    ) -> Result<InstallationLogBatchResponse, AppError> {
        if !self.config.enabled {
            return Err(AppError::ServiceUnavailable(
                "server log ingestion is disabled".into(),
            ));
        }
        validate_batch(&payload)?;
        let permit = Arc::clone(&self.ingest_slots)
            .acquire_owned()
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable("server log ingest service stopped".into())
            })?;
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            store.ingest_sync(installation_id, payload)
        })
        .await
        .map_err(|error| AppError::Internal(format!("join server log ingest failed: {error}")))?
    }

    fn ingest_sync(
        &self,
        installation_id: String,
        payload: InstallationLogBatchPayload,
    ) -> Result<InstallationLogBatchResponse, AppError> {
        let stream_key = (installation_id.clone(), payload.stream_id.clone());
        let mut catalog = self
            .catalog
            .lock()
            .map_err(|_| AppError::Internal("server log catalog lock poisoned".into()))?;
        let received_at_ms = Utc::now()
            .timestamp_millis()
            .max(catalog.last_received_at_ms.saturating_add(1));
        let last_sequence = catalog
            .streams
            .get(&stream_key)
            .map(|stream| stream.last_sequence)
            .unwrap_or_default();
        let next_sequence = last_sequence.saturating_add(1);
        let new_events = payload
            .events
            .iter()
            .filter(|event| event.sequence >= next_sequence)
            .cloned()
            .collect::<Vec<_>>();
        if new_events.is_empty() {
            return Ok(InstallationLogBatchResponse {
                ok: true,
                accepted: 0,
                next_sequence,
            });
        }
        if new_events[0].sequence != next_sequence {
            return Err(AppError::coded_conflict(
                "SERVER_LOG_SEQUENCE_GAP",
                "server log sequence gap",
                serde_json::json!({ "expectedSequence": next_sequence }),
            ));
        }
        for window in new_events.windows(2) {
            if window[1].sequence != window[0].sequence.saturating_add(1) {
                return Err(AppError::coded_conflict(
                    "SERVER_LOG_SEQUENCE_GAP",
                    "server log batch sequences must be contiguous",
                    serde_json::json!({
                        "expectedSequence": window[0].sequence.saturating_add(1)
                    }),
                ));
            }
        }

        let installation_directory = installation_directory(&self.config.root, &installation_id);
        let stream_directory =
            stream_directory(&self.config.root, &installation_id, &payload.stream_id);
        create_private_directory(&installation_directory).map_err(|error| {
            AppError::Internal(format!(
                "create server log installation directory failed: {error}"
            ))
        })?;
        create_private_directory(&stream_directory).map_err(|error| {
            AppError::Internal(format!(
                "create server log stream directory failed: {error}"
            ))
        })?;

        let alias = client_alias(&self.cursor_key, &installation_id);
        let records = new_events
            .into_iter()
            .map(|event| StoredServerLogEvent {
                event_id: event_id(&installation_id, &payload.stream_id, event.sequence),
                installation_id: installation_id.clone(),
                client_alias: alias.clone(),
                stream_id: payload.stream_id.clone(),
                sequence: event.sequence,
                occurred_at_ms: event.occurred_at_ms,
                received_at_ms,
                level: event.level.to_ascii_lowercase(),
                target: redact_sensitive_text(&bounded_text(&event.target, MAX_TARGET_BYTES)),
                message: redact_sensitive_text(&bounded_text(&event.message, MAX_MESSAGE_BYTES)),
                raw_line: event
                    .raw_line
                    .map(|value| bounded_text(&redact_sensitive_text(&value), MAX_RAW_LINE_BYTES)),
                fields: sanitize_fields(event.fields),
                file: event.file.map(|value| bounded_text(&value, 1_024)),
                line: event.line,
                server_version: bounded_text(&payload.server_version, 128),
                commit_id: bounded_text(&payload.commit_id, 128),
            })
            .collect::<Vec<_>>();

        append_bounded_records(
            &installation_directory.join(EVENTS_FILE),
            &records,
            MAX_RETAINED_LINES_PER_CLIENT,
        )?;
        let accepted = records.len();
        let last_sequence = records
            .last()
            .map(|event| event.sequence)
            .unwrap_or(last_sequence);
        write_json_atomic(
            &stream_directory.join("cursor.json"),
            &StreamCursorFile {
                installation_id,
                stream_id: payload.stream_id,
                last_sequence,
                updated_at_ms: received_at_ms,
            },
        )?;
        catalog.streams.entry(stream_key).or_default().last_sequence = last_sequence;
        catalog.last_received_at_ms = received_at_ms;
        drop(catalog);

        Ok(InstallationLogBatchResponse {
            ok: true,
            accepted,
            next_sequence: last_sequence.saturating_add(1),
        })
    }

    async fn query(
        self: &Arc<Self>,
        access: QueryAccess,
        query: ServerLogQuery,
    ) -> Result<ServerLogEventsResponse, AppError> {
        let permit = Arc::clone(&self.query_slots)
            .acquire_owned()
            .await
            .map_err(|_| AppError::ServiceUnavailable("server log query service stopped".into()))?;
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            store.query_sync(access, query)
        })
        .await
        .map_err(|error| AppError::Internal(format!("join server log query failed: {error}")))?
    }

    fn query_sync(
        &self,
        access: QueryAccess,
        query: ServerLogQuery,
    ) -> Result<ServerLogEventsResponse, AppError> {
        let limit = query
            .limit
            .unwrap_or(access.visible_line_limit)
            .clamp(1, access.visible_line_limit);
        let level = query
            .level
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let mut records = Vec::new();
        read_records_file(
            &installation_directory(&self.config.root, &access.installation_id).join(EVENTS_FILE),
            &mut records,
        )?;
        records.retain(|event| event.installation_id == access.installation_id);
        records.sort_by(compare_events_descending);
        if access.public_only {
            records.truncate(PUBLIC_VISIBLE_LINES_PER_CLIENT);
        }
        records.retain(|event| {
            event_matches_query(
                event,
                access.public_only,
                &query,
                level.as_deref(),
                search.as_deref(),
            )
        });
        records.truncate(limit);
        let events = records
            .into_iter()
            .map(|event| ServerLogEventView::from_stored(event, access.public_only))
            .collect();
        Ok(ServerLogEventsResponse {
            events,
            visible_line_limit: access.visible_line_limit,
            retained_line_limit: MAX_RETAINED_LINES_PER_CLIENT,
        })
    }
}

#[derive(Debug, Clone)]
struct QueryAccess {
    public_only: bool,
    installation_id: String,
    visible_line_limit: usize,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ServerLogQuery {
    client_alias: Option<String>,
    level: Option<String>,
    search: Option<String>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerLogEventView {
    event_id: String,
    client_alias: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_subdomain: Option<String>,
    #[serde(skip)]
    lookup_installation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    installation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sequence: Option<u64>,
    occurred_at_ms: i64,
    received_at_ms: i64,
    level: String,
    target: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_id: Option<String>,
}

impl ServerLogEventView {
    fn from_stored(event: StoredServerLogEvent, public: bool) -> Self {
        let lookup_installation_id = event.installation_id.clone();
        Self {
            event_id: event.event_id,
            client_alias: event.client_alias,
            client_subdomain: None,
            lookup_installation_id,
            installation_id: (!public).then_some(event.installation_id),
            stream_id: (!public).then_some(event.stream_id),
            sequence: (!public).then_some(event.sequence),
            occurred_at_ms: event.occurred_at_ms,
            received_at_ms: event.received_at_ms,
            level: event.level,
            target: if public {
                public_message(&event.target)
            } else {
                event.target
            },
            message: if public {
                public_message(&event.message)
            } else {
                event.message
            },
            raw_line: if public { None } else { event.raw_line },
            fields: (!public).then_some(event.fields),
            file: (!public).then_some(event.file).flatten(),
            line: (!public).then_some(event.line).flatten(),
            server_version: (!public).then_some(event.server_version),
            commit_id: (!public).then_some(event.commit_id),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerLogEventsResponse {
    events: Vec<ServerLogEventView>,
    visible_line_limit: usize,
    retained_line_limit: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerLogClientView {
    #[serde(skip_serializing_if = "Option::is_none")]
    installation_id: Option<String>,
    client_alias: String,
    owned: bool,
    full_log_access: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    subdomain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_email: Option<String>,
    platform: String,
    app_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    country_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<String>,
    created_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel_enabled: Option<bool>,
}

impl ServerLogClientView {
    fn from_record(
        record: crate::store::ServerLogClientRecord,
        state: &ServerState,
        owned: bool,
        full_log_access: bool,
    ) -> Self {
        let client_alias = client_alias(&state.server_logs.cursor_key, &record.installation_id);
        let tunnel_url = full_log_access
            .then(|| {
                record
                    .subdomain
                    .as_deref()
                    .map(|subdomain| state.config.tunnel_url(subdomain))
            })
            .flatten();
        Self {
            installation_id: full_log_access.then_some(record.installation_id),
            client_alias,
            owned,
            full_log_access,
            subdomain: record.subdomain,
            tunnel_url,
            owner_email: full_log_access.then_some(record.owner_email).flatten(),
            platform: record.platform,
            app_version: record.app_version,
            country_code: record.country_code,
            region: record.region,
            created_at: record.created_at,
            last_seen_at: record.last_seen_at,
            tunnel_enabled: record.tunnel_enabled,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerLogMetaResponse {
    ingest_enabled: bool,
    authenticated: bool,
    is_router_owner: bool,
    clients: Vec<ServerLogClientView>,
    retained_line_limit: usize,
    public_line_limit: usize,
    poll_interval_seconds: u32,
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/installations/logs/batch",
            post(ingest_installation_logs).layer(DefaultBodyLimit::max(LOG_BATCH_BODY_LIMIT_BYTES)),
        )
        .route("/v1/server-logs/meta", get(server_log_meta))
        .route("/v1/server-logs/events", get(server_log_events))
        .route("/v1/server-logs/export", get(server_log_export))
}

async fn ingest_installation_logs(
    State(state): State<ServerState>,
    Json(input): Json<InstallationLogBatchRequest>,
) -> Result<Json<InstallationLogBatchResponse>, AppError> {
    validate_batch(&input.payload)?;
    state
        .store
        .authenticate_installation_log_batch(&input)
        .await?;
    Ok(Json(
        state
            .server_logs
            .ingest(input.installation_id, input.payload)
            .await?,
    ))
}

async fn server_log_meta(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ServerLogMetaResponse>, AppError> {
    let session = crate::api::resolve_router_session(&state, &headers).await?;
    let authenticated = session.is_some();
    let is_router_owner = session
        .as_ref()
        .is_some_and(|session| session_is_router_owner(&state, &session.email));
    let owned_installation_ids = if let Some(session) = session.as_ref() {
        state
            .store
            .list_verified_installation_ids_for_owner(&session.email)
            .await?
            .into_iter()
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    let log_installation_ids = state.server_logs.installation_ids_with_log_state()?;
    let clients = state
        .store
        .list_server_log_client_records(Some(&log_installation_ids))
        .await?
        .into_iter()
        .map(|record| {
            let owned = owned_installation_ids.contains(&record.installation_id);
            let full_log_access = has_full_log_access(
                is_router_owner,
                &owned_installation_ids,
                &record.installation_id,
            );
            ServerLogClientView::from_record(record, &state, owned, full_log_access)
        })
        .collect();
    Ok(Json(ServerLogMetaResponse {
        ingest_enabled: state.server_logs.config.enabled,
        authenticated,
        is_router_owner,
        clients,
        retained_line_limit: MAX_RETAINED_LINES_PER_CLIENT,
        public_line_limit: PUBLIC_VISIBLE_LINES_PER_CLIENT,
        poll_interval_seconds: DEFAULT_POLL_INTERVAL_SECONDS,
    }))
}

async fn server_log_events(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ServerLogQuery>,
) -> Result<Json<ServerLogEventsResponse>, AppError> {
    let access = resolve_query_access(&state, &headers, query.client_alias.as_deref()).await?;
    let mut response = state.server_logs.query(access, query).await?;
    enrich_client_subdomains(&state, &mut response).await?;
    Ok(Json(response))
}

async fn server_log_export(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(mut query): Query<ServerLogQuery>,
) -> Result<Response, AppError> {
    let access = resolve_query_access(&state, &headers, query.client_alias.as_deref()).await?;
    ensure_export_allowed(&access)?;
    query.limit = Some(MAX_RETAINED_LINES_PER_CLIENT);
    let mut response = state.server_logs.query(access, query).await?;
    enrich_client_subdomains(&state, &mut response).await?;
    let mut body = String::new();
    for event in response.events {
        body.push_str(
            &serde_json::to_string(&event)
                .map_err(|error| AppError::Internal(format!("serialize log export: {error}")))?,
        );
        body.push('\n');
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=cc-switch-server-logs.jsonl",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .map_err(|error| AppError::Internal(format!("build log export response: {error}")))
}

async fn enrich_client_subdomains(
    state: &ServerState,
    response: &mut ServerLogEventsResponse,
) -> Result<(), AppError> {
    let installation_ids = response
        .events
        .iter()
        .map(|event| event.lookup_installation_id.clone())
        .collect::<HashSet<_>>();
    let subdomains = state
        .store
        .list_client_tunnel_subdomains_for_installations(&installation_ids)
        .await?;
    apply_client_subdomains(response, &subdomains);
    Ok(())
}

fn apply_client_subdomains(
    response: &mut ServerLogEventsResponse,
    subdomains: &HashMap<String, String>,
) {
    for event in &mut response.events {
        event.client_subdomain = subdomains.get(&event.lookup_installation_id).cloned();
    }
}

async fn resolve_query_access(
    state: &ServerState,
    headers: &HeaderMap,
    requested_alias: Option<&str>,
) -> Result<QueryAccess, AppError> {
    let requested_alias = requested_alias
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("server log clientAlias is required".into()))?;
    let log_installation_ids = state.server_logs.installation_ids_with_log_state()?;
    let installation_id = state
        .store
        .list_server_log_client_records(Some(&log_installation_ids))
        .await?
        .into_iter()
        .map(|record| record.installation_id)
        .find(|installation_id| {
            client_alias(&state.server_logs.cursor_key, installation_id) == requested_alias
        })
        .ok_or_else(|| AppError::NotFound("server log client not found".into()))?;
    let session = crate::api::resolve_router_session(state, headers).await?;
    let is_router_owner = session
        .as_ref()
        .is_some_and(|session| session_is_router_owner(state, &session.email));
    let owned_installation_ids = if let Some(session) = session.as_ref() {
        state
            .store
            .list_verified_installation_ids_for_owner(&session.email)
            .await?
            .into_iter()
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    Ok(query_access_for_installation(
        installation_id.clone(),
        has_full_log_access(is_router_owner, &owned_installation_ids, &installation_id),
    ))
}

fn session_is_router_owner(state: &ServerState, email: &str) -> bool {
    state
        .config
        .official_provider_email()
        .is_some_and(|owner_email| owner_email.eq_ignore_ascii_case(email.trim()))
}

fn has_full_log_access(
    is_router_owner: bool,
    owned_installation_ids: &HashSet<String>,
    installation_id: &str,
) -> bool {
    is_router_owner || owned_installation_ids.contains(installation_id)
}

fn query_access_for_installation(installation_id: String, full_log_access: bool) -> QueryAccess {
    QueryAccess {
        public_only: !full_log_access,
        installation_id,
        visible_line_limit: if full_log_access {
            MAX_RETAINED_LINES_PER_CLIENT
        } else {
            PUBLIC_VISIBLE_LINES_PER_CLIENT
        },
    }
}

fn ensure_export_allowed(access: &QueryAccess) -> Result<(), AppError> {
    if access.public_only {
        Err(AppError::Forbidden(
            "full Client log access is required to export logs".into(),
        ))
    } else {
        Ok(())
    }
}

fn validate_batch(payload: &InstallationLogBatchPayload) -> Result<(), AppError> {
    if payload.protocol_version != 1 {
        return Err(AppError::BadRequest(
            "unsupported server log protocol version".into(),
        ));
    }
    if payload.stream_id.trim().is_empty() || payload.stream_id.len() > 128 {
        return Err(AppError::BadRequest(
            "server log streamId must be 1-128 characters".into(),
        ));
    }
    if payload.events.is_empty() || payload.events.len() > MAX_BATCH_EVENTS {
        return Err(AppError::BadRequest(format!(
            "server log batch must contain 1-{MAX_BATCH_EVENTS} events"
        )));
    }
    for event in &payload.events {
        if !matches!(
            event.level.to_ascii_lowercase().as_str(),
            "info" | "warn" | "error"
        ) {
            return Err(AppError::BadRequest(
                "server log level must be info, warn, or error".into(),
            ));
        }
        if event.target.is_empty() || event.target.len() > MAX_TARGET_BYTES {
            return Err(AppError::BadRequest(
                "server log target length is invalid".into(),
            ));
        }
        if event.message.len() > MAX_MESSAGE_BYTES {
            return Err(AppError::BadRequest(
                "server log message is too large".into(),
            ));
        }
        if let Some(raw_line) = event.raw_line.as_deref()
            && (raw_line.len() > MAX_RAW_LINE_BYTES
                || raw_line.contains('\r')
                || raw_line.contains('\n'))
        {
            return Err(AppError::BadRequest(
                "server log rawLine must be a single line within the size limit".into(),
            ));
        }
        let fields_bytes = serde_json::to_vec(&event.fields)
            .map_err(|error| AppError::BadRequest(format!("invalid server log fields: {error}")))?;
        if fields_bytes.len() > MAX_FIELDS_BYTES {
            return Err(AppError::BadRequest(
                "server log fields are too large".into(),
            ));
        }
    }
    Ok(())
}

fn append_bounded_records(
    path: &Path,
    incoming: &[StoredServerLogEvent],
    max_records: usize,
) -> Result<(), AppError> {
    let mut records = Vec::new();
    read_records_file(path, &mut records)?;
    let mut event_ids = HashSet::new();
    records.retain(|record| event_ids.insert(record.event_id.clone()));
    records.extend(
        incoming
            .iter()
            .filter(|record| event_ids.insert(record.event_id.clone()))
            .cloned(),
    );
    if records.len() > max_records {
        records.drain(..records.len() - max_records);
    }
    write_records_atomic(path, &records)
}

fn write_records_atomic(path: &Path, records: &[StoredServerLogEvent]) -> Result<(), AppError> {
    let temporary = path.with_extension("jsonl.new");
    {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|error| {
            AppError::Internal(format!("create bounded server log file failed: {error}"))
        })?;
        for record in records {
            serde_json::to_writer(&mut file, record).map_err(|error| {
                AppError::Internal(format!("encode server log event failed: {error}"))
            })?;
            file.write_all(b"\n").map_err(|error| {
                AppError::Internal(format!("write server log event failed: {error}"))
            })?;
        }
        file.sync_all().map_err(|error| {
            AppError::Internal(format!("sync bounded server log file failed: {error}"))
        })?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        AppError::Internal(format!("replace bounded server log file failed: {error}"))
    })
}

fn read_records_file(path: &Path, records: &mut Vec<StoredServerLogEvent>) -> Result<(), AppError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(AppError::Internal(format!(
                "open server log file failed: {error}"
            )));
        }
    };
    read_record_lines(BufReader::new(file), records)
}

fn read_record_lines<R: BufRead>(
    reader: R,
    records: &mut Vec<StoredServerLogEvent>,
) -> Result<(), AppError> {
    for line in reader.lines() {
        let line = line.map_err(|error| {
            AppError::Internal(format!("read server log event failed: {error}"))
        })?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<StoredServerLogEvent>(&line) {
            records.push(record);
        }
    }
    Ok(())
}

fn event_matches_query(
    event: &StoredServerLogEvent,
    public: bool,
    query: &ServerLogQuery,
    level: Option<&str>,
    search: Option<&str>,
) -> bool {
    if query
        .from_ms
        .is_some_and(|from| event.received_at_ms < from)
        || query.to_ms.is_some_and(|to| event.received_at_ms > to)
        || level.is_some_and(|level| event.level != level)
    {
        return false;
    }
    if let Some(search) = search {
        let (target, message) = if public {
            (
                public_message(&event.target),
                public_message(&event.message),
            )
        } else {
            (event.target.clone(), event.message.clone())
        };
        let raw_line = if public {
            ""
        } else {
            event.raw_line.as_deref().unwrap_or_default()
        };
        if !format!("{target} {message} {raw_line}")
            .to_ascii_lowercase()
            .contains(search)
        {
            return false;
        }
    }
    true
}

fn compare_events_descending(
    left: &StoredServerLogEvent,
    right: &StoredServerLogEvent,
) -> std::cmp::Ordering {
    right
        .received_at_ms
        .cmp(&left.received_at_ms)
        .then_with(|| {
            if left.stream_id == right.stream_id {
                right.sequence.cmp(&left.sequence)
            } else {
                right.event_id.cmp(&left.event_id)
            }
        })
}

fn event_files(root: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut files = Vec::new();
    collect_event_files(root, &mut files).map_err(|error| {
        AppError::Internal(format!("scan server log directory failed: {error}"))
    })?;
    Ok(files)
}

fn collect_event_files(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_event_files(&path, files)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some(EVENTS_FILE) {
            files.push(path);
        }
    }
    Ok(())
}

fn load_stream_cursors(root: &Path, catalog: &mut Catalog) -> Result<(), AppError> {
    fn visit(directory: &Path, catalog: &mut Catalog) -> Result<(), AppError> {
        if !directory.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(directory)
            .map_err(|error| AppError::Internal(format!("scan log cursors failed: {error}")))?
        {
            let path = entry
                .map_err(|error| {
                    AppError::Internal(format!("read log cursor entry failed: {error}"))
                })?
                .path();
            if path.is_dir() {
                visit(&path, catalog)?;
            } else if path.file_name().and_then(|value| value.to_str()) == Some("cursor.json")
                && let Ok(bytes) = fs::read(&path)
                && let Ok(cursor) = serde_json::from_slice::<StreamCursorFile>(&bytes)
            {
                catalog.streams.insert(
                    (cursor.installation_id, cursor.stream_id),
                    StreamState {
                        last_sequence: cursor.last_sequence,
                    },
                );
            }
        }
        Ok(())
    }
    visit(root, catalog)
}

fn recover_catalog_from_event_files(
    root: &Path,
    catalog: &mut Catalog,
) -> Result<HashMap<(String, String), i64>, AppError> {
    let mut recovered_cursors = HashMap::new();
    for path in event_files(root)? {
        let mut records = Vec::new();
        read_records_file(&path, &mut records)?;
        if records.len() > MAX_RETAINED_LINES_PER_CLIENT {
            records.drain(..records.len() - MAX_RETAINED_LINES_PER_CLIENT);
            write_records_atomic(&path, &records)?;
        }
        for record in records {
            catalog.last_received_at_ms = catalog.last_received_at_ms.max(record.received_at_ms);
            let stream_key = (record.installation_id, record.stream_id);
            let state = catalog.streams.entry(stream_key.clone()).or_default();
            if record.sequence > state.last_sequence {
                state.last_sequence = record.sequence;
                recovered_cursors.insert(stream_key, record.received_at_ms);
            }
        }
    }
    Ok(recovered_cursors)
}

fn persist_recovered_stream_cursors(
    root: &Path,
    catalog: &Catalog,
    recovered_cursors: HashMap<(String, String), i64>,
) -> Result<(), AppError> {
    for ((installation_id, stream_id), updated_at_ms) in recovered_cursors {
        let directory = stream_directory(root, &installation_id, &stream_id);
        create_private_directory(&directory).map_err(|error| {
            AppError::Internal(format!(
                "create recovered server log stream directory failed: {error}"
            ))
        })?;
        let last_sequence = catalog
            .streams
            .get(&(installation_id.clone(), stream_id.clone()))
            .map(|state| state.last_sequence)
            .ok_or_else(|| AppError::Internal("recovered server log cursor is missing".into()))?;
        write_json_atomic(
            &directory.join("cursor.json"),
            &StreamCursorFile {
                installation_id,
                stream_id,
                last_sequence,
                updated_at_ms,
            },
        )?;
    }
    Ok(())
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    let temporary = path.with_extension("json.new");
    let bytes = serde_json::to_vec(value)
        .map_err(|error| AppError::Internal(format!("encode server log cursor failed: {error}")))?;
    {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|error| {
            AppError::Internal(format!("create server log cursor failed: {error}"))
        })?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                AppError::Internal(format!("write server log cursor failed: {error}"))
            })?;
    }
    fs::rename(&temporary, path)
        .map_err(|error| AppError::Internal(format!("replace server log cursor failed: {error}")))
}

fn load_or_create_cursor_key(root: &Path) -> Result<[u8; 32], AppError> {
    let path = root.join(CURSOR_KEY_FILE);
    if let Ok(bytes) = fs::read(&path)
        && let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        return Ok(key);
    }
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(&key)
                .and_then(|_| file.sync_all())
                .map_err(|error| {
                    AppError::Internal(format!("write log cursor key failed: {error}"))
                })?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = fs::read(&path).map_err(|error| {
                AppError::Internal(format!("read log cursor key failed: {error}"))
            })?;
            <[u8; 32]>::try_from(bytes.as_slice())
                .map_err(|_| AppError::Internal("invalid log cursor key length".into()))
        }
        Err(error) => Err(AppError::Internal(format!(
            "create log cursor key failed: {error}"
        ))),
    }
}

fn installation_directory(root: &Path, installation_id: &str) -> PathBuf {
    root.join(hash_component(installation_id))
}

fn stream_directory(root: &Path, installation_id: &str, stream_id: &str) -> PathBuf {
    installation_directory(root, installation_id)
        .join("streams")
        .join(hash_component(stream_id))
}

fn hash_component(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn event_id(installation_id: &str, stream_id: &str, sequence: u64) -> String {
    let value = format!("{installation_id}\n{stream_id}\n{sequence}");
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn client_alias(key: &[u8; 32], installation_id: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(installation_id.as_bytes());
    let digest = mac.finalize().into_bytes();
    format!("client-{}", hex::encode(&digest[..16]))
}

fn sanitize_fields(fields: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    fields
        .into_iter()
        .filter_map(|(key, value)| {
            if is_sensitive_key(&key) {
                None
            } else {
                Some((key, sanitize_value(value)))
            }
        })
        .collect()
}

fn sanitize_value(value: Value) -> Value {
    match value {
        Value::String(value) => Value::String(redact_sensitive_text(&bounded_text(&value, 4_096))),
        Value::Array(values) => {
            Value::Array(values.into_iter().take(64).map(sanitize_value).collect())
        }
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .filter_map(|(key, value)| {
                    (!is_sensitive_key(&key)).then(|| (key, sanitize_value(value)))
                })
                .collect(),
        ),
        other => other,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "authorization",
        "cookie",
        "password",
        "secret",
        "token",
        "api_key",
        "apikey",
        "private_key",
        "credential",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

fn redact_sensitive_text(input: &str) -> String {
    const KEYS: &[&str] = &[
        "authorization",
        "api_key",
        "apikey",
        "api-key",
        "access_token",
        "refresh_token",
        "cookie",
        "password",
        "secret",
        "token",
    ];
    let input = mask_prefixed_secret(input, "ksk_");
    input
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            let bearer_start = lower.find("bearer ");
            let sensitive_assignment = KEYS
                .iter()
                .filter_map(|key| find_sensitive_assignment(&lower, key))
                .min_by_key(|(start, _)| *start);
            if let Some(start) = bearer_start
                && sensitive_assignment
                    .is_none_or(|(assignment_start, _)| start <= assignment_start)
            {
                return format!("{}Bearer [REDACTED]", &line[..start]);
            }
            if let Some((_, end)) = sensitive_assignment {
                return format!("{} [REDACTED]", line[..end].trim_end());
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_sensitive_assignment(line: &str, key: &str) -> Option<(usize, usize)> {
    let mut search_start = 0;
    while let Some(relative_start) = line[search_start..].find(key) {
        let start = search_start + relative_start;
        let after_key = start + key.len();
        let separator = line[after_key..]
            .char_indices()
            .take_while(|(index, _)| *index <= 3)
            .find(|(_, character)| matches!(character, ':' | '='))
            .map(|(index, character)| (index, character.len_utf8()));
        if let Some((separator, separator_len)) = separator {
            return Some((start, after_key + separator + separator_len));
        }
        search_start = after_key;
    }
    None
}

fn mask_prefixed_secret(input: &str, prefix: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative_start) = input[cursor..].find(prefix) {
        let start = cursor + relative_start;
        output.push_str(&input[cursor..start]);
        let mut end = start + prefix.len();
        while end < input.len() {
            let byte = input.as_bytes()[end];
            if !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) {
                break;
            }
            end += 1;
        }
        output.push_str("[REDACTED]");
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    output
}

fn public_message(input: &str) -> String {
    input
        .split_whitespace()
        .map(|token| {
            let trimmed = token.trim_matches(|character: char| {
                matches!(
                    character,
                    ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
                )
            });
            let value = trimmed
                .rsplit_once('=')
                .map(|(_, value)| value)
                .unwrap_or(trimmed)
                .trim_matches(|character: char| {
                    matches!(
                        character,
                        ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
                    )
                });
            if value.contains('@') {
                "[email]"
            } else if value.parse::<std::net::IpAddr>().is_ok()
                || value.parse::<std::net::SocketAddr>().is_ok()
            {
                "[ip]"
            } else if value.contains("http://") || value.contains("https://") {
                "[url]"
            } else if value.len() > 96 {
                "[value]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cc-switch-router-server-logs-{label}-{}-{:016x}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            rand::thread_rng().next_u64()
        ))
    }

    fn test_store() -> (Arc<ServerLogStore>, PathBuf) {
        let root = test_root("store");
        let store = ServerLogStore::open(ServerLogRuntimeConfig {
            enabled: true,
            root: root.clone(),
        })
        .expect("open log store");
        (Arc::new(store), root)
    }

    fn payload_range(
        stream_id: &str,
        first_sequence: u64,
        count: usize,
    ) -> InstallationLogBatchPayload {
        InstallationLogBatchPayload {
            protocol_version: 1,
            stream_id: stream_id.into(),
            server_version: "0.1.0".into(),
            commit_id: "abc".into(),
            events: (0..count)
                .map(|offset| InstallationLogEvent {
                    sequence: first_sequence + offset as u64,
                    occurred_at_ms: Utc::now().timestamp_millis(),
                    level: "info".into(),
                    target: "cc_switch_server::state remote=203.0.113.2".into(),
                    message: format!(
                        "event={} token=secret user@example.com 203.0.113.2",
                        first_sequence + offset as u64
                    ),
                    raw_line: Some(format!(
                        "2026-08-05T23:21:25.141Z  INFO raw-only-event-{}",
                        first_sequence + offset as u64
                    )),
                    fields: BTreeMap::from([
                        ("operation".into(), Value::String("start".into())),
                        ("access_token".into(), Value::String("secret".into())),
                    ]),
                    file: Some("src/state.rs".into()),
                    line: Some(42),
                })
                .collect(),
        }
    }

    fn access(installation_id: &str, full: bool) -> QueryAccess {
        query_access_for_installation(installation_id.to_string(), full)
    }

    #[tokio::test]
    async fn ingest_is_idempotent_and_public_projection_is_redacted() {
        let (store, root) = test_store();
        let first = store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 1))
            .await
            .expect("first ingest");
        assert_eq!(first.accepted, 1);
        let duplicate = store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 1))
            .await
            .expect("duplicate ingest");
        assert_eq!(duplicate.accepted, 0);

        let mut response = store
            .query(access("installation-a", false), ServerLogQuery::default())
            .await
            .expect("query public logs");
        apply_client_subdomains(
            &mut response,
            &HashMap::from([("installation-a".to_string(), "client-sub".to_string())]),
        );
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.visible_line_limit, PUBLIC_VISIBLE_LINES_PER_CLIENT);
        assert!(response.events[0].installation_id.is_none());
        assert_eq!(
            response.events[0].client_subdomain.as_deref(),
            Some("client-sub")
        );
        assert!(response.events[0].fields.is_none());
        assert!(response.events[0].raw_line.is_none());
        assert!(!response.events[0].message.contains("user@example.com"));
        assert!(!response.events[0].message.contains("203.0.113.2"));
        assert!(!response.events[0].message.contains("secret"));
        assert!(!response.events[0].target.contains("203.0.113.2"));
        let public_json = serde_json::to_value(&response.events[0]).expect("serialize public log");
        assert_eq!(public_json["clientSubdomain"], "client-sub");
        assert!(public_json.get("installationId").is_none());
        assert!(public_json.get("lookupInstallationId").is_none());

        let side_channel = store
            .query(
                access("installation-a", false),
                ServerLogQuery {
                    search: Some("user@example.com".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("query redacted public logs");
        assert!(side_channel.events.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn full_projection_returns_and_searches_raw_lines_without_public_side_channel() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 1))
            .await
            .unwrap();

        let full = store
            .query(
                access("installation-a", true),
                ServerLogQuery {
                    search: Some("raw-only-event-1".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(full.events.len(), 1);
        assert_eq!(
            full.events[0].raw_line.as_deref(),
            Some("2026-08-05T23:21:25.141Z  INFO raw-only-event-1")
        );

        let public = store
            .query(
                access("installation-a", false),
                ServerLogQuery {
                    search: Some("raw-only-event-1".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(public.events.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn retention_is_latest_one_hundred_lines_per_client() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 125))
            .await
            .expect("ingest logs");

        let private = store
            .query(access("installation-a", true), ServerLogQuery::default())
            .await
            .expect("query retained logs");
        assert_eq!(private.events.len(), 100);
        assert_eq!(
            private.events.first().and_then(|event| event.sequence),
            Some(125)
        );
        assert_eq!(
            private.events.last().and_then(|event| event.sequence),
            Some(26)
        );

        let mut on_disk = Vec::new();
        read_records_file(
            &installation_directory(&root, "installation-a").join(EVENTS_FILE),
            &mut on_disk,
        )
        .unwrap();
        assert_eq!(on_disk.len(), 100);
        assert_eq!(on_disk.first().map(|event| event.sequence), Some(26));
        assert_eq!(on_disk.last().map(|event| event.sequence), Some(125));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn log_state_only_lists_installations_that_have_uploaded_logs() {
        let (store, root) = test_store();
        assert!(store.installation_ids_with_log_state().unwrap().is_empty());
        store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 1))
            .await
            .unwrap();
        assert_eq!(
            store.installation_ids_with_log_state().unwrap(),
            HashSet::from(["installation-a".to_string()])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn orphan_cleanup_preserves_existing_inactive_client_logs() {
        let (store, root) = test_store();
        for installation_id in ["installation-a", "installation-b"] {
            store
                .ingest(installation_id.into(), payload_range("stream-a", 1, 1))
                .await
                .unwrap();
        }

        let removed = store
            .remove_installations(HashSet::from(["installation-b".to_string()]))
            .await
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(
            store.installation_ids_with_log_state().unwrap(),
            HashSet::from(["installation-a".to_string()])
        );
        assert!(
            installation_directory(&root, "installation-a")
                .join(EVENTS_FILE)
                .is_file()
        );
        assert!(!installation_directory(&root, "installation-b").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn public_and_full_access_limits_are_enforced_server_side() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 100))
            .await
            .unwrap();
        let request_all = ServerLogQuery {
            limit: Some(usize::MAX),
            ..Default::default()
        };
        let public = store
            .query(access("installation-a", false), request_all.clone())
            .await
            .unwrap();
        let full = store
            .query(access("installation-a", true), request_all)
            .await
            .unwrap();
        assert_eq!(public.events.len(), 10);
        assert!(public.events.iter().all(|event| event.sequence.is_none()));
        assert_eq!(full.events.len(), 100);
        assert!(full.events.iter().all(|event| event.sequence.is_some()));
        assert!(ensure_export_allowed(&access("installation-a", false)).is_err());
        assert!(ensure_export_allowed(&access("installation-a", true)).is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn public_filters_cannot_reach_beyond_the_latest_ten_lines() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 20))
            .await
            .unwrap();

        let hidden = store
            .query(
                access("installation-a", false),
                ServerLogQuery {
                    search: Some("event=1 token=".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(hidden.events.is_empty());

        let visible = store
            .query(
                access("installation-a", false),
                ServerLogQuery {
                    search: Some("event=20 token=".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(visible.events.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn old_inactive_logs_survive_reopen() {
        let (store, root) = test_store();
        let mut old = payload_range("stream-a", 1, 1);
        old.events[0].occurred_at_ms = 946_684_800_000;
        store
            .ingest("installation-a".into(), old)
            .await
            .expect("ingest old event");
        drop(store);

        let reopened = Arc::new(
            ServerLogStore::open(ServerLogRuntimeConfig {
                enabled: true,
                root: root.clone(),
            })
            .expect("reopen log store"),
        );
        let response = reopened
            .query(access("installation-a", false), ServerLogQuery::default())
            .await
            .expect("query old event");
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].occurred_at_ms, 946_684_800_000);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn multiple_streams_share_cap_and_keep_independent_cursors() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 70))
            .await
            .unwrap();
        store
            .ingest("installation-a".into(), payload_range("stream-b", 1, 70))
            .await
            .unwrap();
        assert_eq!(
            store
                .query(access("installation-a", true), ServerLogQuery::default())
                .await
                .unwrap()
                .events
                .len(),
            100
        );
        drop(store);

        let reopened = Arc::new(
            ServerLogStore::open(ServerLogRuntimeConfig {
                enabled: true,
                root: root.clone(),
            })
            .expect("reopen log store"),
        );
        for stream in ["stream-a", "stream-b"] {
            let duplicate = reopened
                .ingest("installation-a".into(), payload_range(stream, 70, 1))
                .await
                .unwrap();
            assert_eq!(duplicate.accepted, 0);
            assert_eq!(duplicate.next_sequence, 71);
            assert!(
                stream_directory(&root, "installation-a", stream)
                    .join("cursor.json")
                    .is_file()
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_trimmed_stream_cursor_still_prevents_replay_after_reopen() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 100))
            .await
            .unwrap();
        store
            .ingest("installation-a".into(), payload_range("stream-b", 1, 100))
            .await
            .unwrap();
        drop(store);

        let reopened = Arc::new(
            ServerLogStore::open(ServerLogRuntimeConfig {
                enabled: true,
                root: root.clone(),
            })
            .unwrap(),
        );
        let duplicate = reopened
            .ingest("installation-a".into(), payload_range("stream-a", 100, 1))
            .await
            .unwrap();
        assert_eq!(duplicate.accepted, 0);
        assert_eq!(duplicate.next_sequence, 101);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn recovered_cursor_is_persisted_before_its_events_are_trimmed() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload_range("stream-a", 1, 100))
            .await
            .unwrap();
        let cursor_path = stream_directory(&root, "installation-a", "stream-a").join("cursor.json");
        fs::remove_file(&cursor_path).unwrap();
        drop(store);

        let recovered = Arc::new(
            ServerLogStore::open(ServerLogRuntimeConfig {
                enabled: true,
                root: root.clone(),
            })
            .unwrap(),
        );
        assert!(cursor_path.is_file());
        recovered
            .ingest("installation-a".into(), payload_range("stream-b", 1, 100))
            .await
            .unwrap();
        drop(recovered);

        let reopened = Arc::new(
            ServerLogStore::open(ServerLogRuntimeConfig {
                enabled: true,
                root: root.clone(),
            })
            .unwrap(),
        );
        let duplicate = reopened
            .ingest("installation-a".into(), payload_range("stream-a", 100, 1))
            .await
            .unwrap();
        assert_eq!(duplicate.accepted, 0);
        assert_eq!(duplicate.next_sequence, 101);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn access_is_full_only_for_router_or_matching_client_owner() {
        let owned = HashSet::from(["installation-a".to_string()]);
        assert!(has_full_log_access(false, &owned, "installation-a"));
        assert!(!has_full_log_access(false, &owned, "installation-b"));
        assert!(has_full_log_access(true, &HashSet::new(), "installation-b"));
    }

    #[test]
    fn client_alias_is_stable_opaque_and_128_bit() {
        let key = [7u8; 32];
        let alias = client_alias(&key, "installation-a");
        assert_eq!(alias, client_alias(&key, "installation-a"));
        assert_ne!(alias, client_alias(&key, "installation-b"));
        assert!(alias.starts_with("client-"));
        assert_eq!(alias.len(), "client-".len() + 32);
        assert!(!alias.contains("installation-a"));
    }

    #[tokio::test]
    async fn ingest_rejects_sequence_gaps() {
        let (store, root) = test_store();
        let error = store
            .ingest("installation-a".into(), payload_range("stream-a", 2, 1))
            .await
            .expect_err("sequence gap");
        assert_eq!(error.code(), Some("SERVER_LOG_SEQUENCE_GAP"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn batch_validation_rejects_non_info_family_levels() {
        let mut invalid = payload_range("stream-a", 1, 1);
        invalid.events[0].level = "debug".into();
        assert!(matches!(
            validate_batch(&invalid),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn batch_validation_rejects_multiline_and_oversized_raw_lines() {
        let mut multiline = payload_range("stream-a", 1, 1);
        multiline.events[0].raw_line = Some("first\nsecond".into());
        assert!(matches!(
            validate_batch(&multiline),
            Err(AppError::BadRequest(_))
        ));

        let mut oversized = payload_range("stream-a", 1, 1);
        oversized.events[0].raw_line = Some("x".repeat(MAX_RAW_LINE_BYTES + 1));
        assert!(matches!(
            validate_batch(&oversized),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn router_redaction_removes_split_and_prefixed_credentials() {
        let redacted = redact_sensitive_text(
            "authorization: Bearer abc123\nupstream rejected ksk_abcdefghijklmnop",
        );
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("ksk_abcdefghijklmnop"));
        assert!(redacted.contains("[REDACTED]"));
        assert_eq!(
            redact_sensitive_text("token router connected"),
            "token router connected"
        );

        let repeated = redact_sensitive_text("token router connected token=secret-value");
        assert!(!repeated.contains("secret-value"));
        let reordered = redact_sensitive_text("token=first authorization: second");
        assert!(!reordered.contains("first"));
        assert!(!reordered.contains("second"));
    }
}
