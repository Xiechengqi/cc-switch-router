use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{TimeZone, Utc};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
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
const MAX_QUERY_LIMIT: usize = 500;
const DEFAULT_QUERY_LIMIT: usize = 200;
const PUBLIC_WINDOW_MS: i64 = 5 * 60 * 1_000;
const PUBLIC_CLOCK_SKEW_MS: i64 = 60 * 1_000;
const SEGMENT_MAX_BYTES: u64 = 4 * 1024 * 1024;
const CURSOR_KEY_FILE: &str = ".cursor-key";
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(15 * 60);

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone)]
pub struct ServerLogRuntimeConfig {
    pub enabled: bool,
    pub root: PathBuf,
    pub retention_days: u32,
    pub max_total_bytes: u64,
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
        let retention_days = env_u32("CC_SWITCH_ROUTER_SERVER_LOG_RETENTION_DAYS", 7, 1, 90)?;
        let max_total_mib = env_u64(
            "CC_SWITCH_ROUTER_SERVER_LOG_MAX_TOTAL_MIB",
            1_024,
            16,
            1_048_576,
        )?;
        Ok(Self {
            enabled,
            root,
            retention_days,
            max_total_bytes: max_total_mib.saturating_mul(1024 * 1024),
        })
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
    active_day: String,
    updated_at_ms: i64,
}

#[derive(Debug, Clone)]
struct StreamState {
    last_sequence: u64,
    active_day: String,
}

#[derive(Debug, Default)]
struct Catalog {
    streams: HashMap<(String, String), StreamState>,
    public_recent: VecDeque<StoredServerLogEvent>,
}

pub struct ServerLogStore {
    config: ServerLogRuntimeConfig,
    catalog: Mutex<Catalog>,
    cursor_key: [u8; 32],
    stored_event_bytes: AtomicU64,
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
            retention_days: 7,
            max_total_bytes: 16 * 1024 * 1024,
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
        let stored_event_bytes = cleanup_files(&config)?;
        let cursor_key = load_or_create_cursor_key(&config.root)?;
        let mut catalog = Catalog::default();
        load_stream_cursors(&config.root, &mut catalog)?;
        recover_catalog_from_event_files(&config.root, &mut catalog, cursor_key)?;
        prune_public_window(&mut catalog.public_recent, Utc::now().timestamp_millis());
        Ok(Self {
            config,
            catalog: Mutex::new(catalog),
            cursor_key,
            stored_event_bytes: AtomicU64::new(stored_event_bytes),
            ingest_slots: Arc::new(Semaphore::new(4)),
            query_slots: Arc::new(Semaphore::new(4)),
        })
    }

    pub fn config(&self) -> &ServerLogRuntimeConfig {
        &self.config
    }

    pub fn spawn_maintenance(self: &Arc<Self>) {
        let store = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(MAINTENANCE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let store = Arc::clone(&store);
                let result = tokio::task::spawn_blocking(move || {
                    let mut catalog = store.catalog.lock().map_err(|_| {
                        AppError::Internal("server log catalog lock poisoned".into())
                    })?;
                    let stored_event_bytes = cleanup_files(&store.config)?;
                    store
                        .stored_event_bytes
                        .store(stored_event_bytes, Ordering::Release);
                    catalog.streams.retain(|(installation_id, stream_id), _| {
                        stream_has_persistent_state(&store.config.root, installation_id, stream_id)
                    });
                    Ok::<(), AppError>(())
                })
                .await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::warn!(%error, "server log maintenance failed"),
                    Err(error) => tracing::warn!(%error, "join server log maintenance failed"),
                }
            }
        });
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
        let received_at_ms = Utc::now().timestamp_millis();
        let day = utc_day(received_at_ms);
        let key = (installation_id.clone(), payload.stream_id.clone());
        let mut catalog = self
            .catalog
            .lock()
            .map_err(|_| AppError::Internal("server log catalog lock poisoned".into()))?;
        let stream = catalog.streams.entry(key).or_insert_with(|| StreamState {
            last_sequence: 0,
            active_day: day.clone(),
        });
        let next_sequence = stream.last_sequence.saturating_add(1);
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

        let directory = stream_directory(&self.config.root, &installation_id, &payload.stream_id);
        create_private_directory(&directory).map_err(|error| {
            AppError::Internal(format!(
                "create server log stream directory failed: {error}"
            ))
        })?;
        if stream.active_day != day {
            rotate_active_file(&directory, &stream.active_day)?;
            stream.active_day = day.clone();
        }
        let active_path = directory.join(format!("active-{day}.jsonl"));
        repair_trailing_partial_line(&active_path)?;
        if active_path.metadata().map(|meta| meta.len()).unwrap_or(0) >= SEGMENT_MAX_BYTES {
            rotate_active_file(&directory, &day)?;
        }

        let alias = client_alias(&self.cursor_key, &installation_id);
        let mut records = Vec::with_capacity(new_events.len());
        for event in new_events {
            records.push(StoredServerLogEvent {
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
                fields: sanitize_fields(event.fields),
                file: event.file.map(|value| bounded_text(&value, 1_024)),
                line: event.line,
                server_version: bounded_text(&payload.server_version, 128),
                commit_id: bounded_text(&payload.commit_id, 128),
            });
        }
        let appended_bytes = append_records(&active_path, &records)?;
        let approximate_stored_bytes = self
            .stored_event_bytes
            .fetch_add(appended_bytes, Ordering::AcqRel)
            .saturating_add(appended_bytes);
        let last_sequence = records
            .last()
            .map(|event| event.sequence)
            .unwrap_or(stream.last_sequence);
        stream.last_sequence = last_sequence;
        write_json_atomic(
            &directory.join("cursor.json"),
            &StreamCursorFile {
                installation_id,
                stream_id: payload.stream_id,
                last_sequence,
                active_day: stream.active_day.clone(),
                updated_at_ms: received_at_ms,
            },
        )?;
        for record in &records {
            catalog.public_recent.push_back(record.clone());
        }
        prune_public_window(&mut catalog.public_recent, received_at_ms);
        if approximate_stored_bytes > self.config.max_total_bytes {
            match cleanup_files(&self.config) {
                Ok(stored_event_bytes) => self
                    .stored_event_bytes
                    .store(stored_event_bytes, Ordering::Release),
                Err(error) => tracing::warn!(%error, "server log capacity cleanup failed"),
            }
        }
        drop(catalog);
        Ok(InstallationLogBatchResponse {
            ok: true,
            accepted: records.len(),
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
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .clamp(1, MAX_QUERY_LIMIT);
        let cursor = query
            .cursor
            .as_deref()
            .map(|value| decode_cursor(value, &self.cursor_key))
            .transpose()?;
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let level = query
            .level
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);

        let mut records = if access.public_only {
            let now_ms = Utc::now().timestamp_millis();
            let mut catalog = self
                .catalog
                .lock()
                .map_err(|_| AppError::Internal("server log catalog lock poisoned".into()))?;
            prune_public_window(&mut catalog.public_recent, now_ms);
            catalog
                .public_recent
                .iter()
                .filter(|event| is_public_window_event(event, now_ms))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            read_query_records(&self.config.root, &access, &query, cursor.as_ref(), limit)?
        };
        records.retain(|event| {
            event_matches_query(
                event,
                &access,
                &query,
                cursor.as_ref(),
                level.as_deref(),
                search.as_deref(),
            )
        });
        records.sort_by(|left, right| {
            right
                .received_at_ms
                .cmp(&left.received_at_ms)
                .then_with(|| right.event_id.cmp(&left.event_id))
        });
        let has_more = records.len() > limit;
        records.truncate(limit);
        let next_cursor = has_more
            .then(|| records.last())
            .flatten()
            .map(|event| {
                encode_cursor(
                    &CursorPayload {
                        received_at_ms: event.received_at_ms,
                        event_id: event.event_id.clone(),
                    },
                    &self.cursor_key,
                )
            })
            .transpose()?;
        let events = records
            .into_iter()
            .map(|event| ServerLogEventView::from_stored(event, access.public_only))
            .collect();
        Ok(ServerLogEventsResponse {
            events,
            next_cursor,
            public_window_seconds: access.public_only.then_some(300),
        })
    }
}

#[derive(Debug, Clone)]
struct QueryAccess {
    public_only: bool,
    all_installations: bool,
    installation_ids: HashSet<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ServerLogQuery {
    scope: Option<String>,
    installation_id: Option<String>,
    level: Option<String>,
    search: Option<String>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    limit: Option<usize>,
    cursor: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    public_window_seconds: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerLogClientView {
    installation_id: String,
    client_alias: String,
    owned: bool,
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
    created_at: chrono::DateTime<Utc>,
    last_seen_at: chrono::DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel_enabled: Option<bool>,
}

impl ServerLogClientView {
    fn from_record(
        record: crate::store::ServerLogClientRecord,
        state: &ServerState,
        owned: bool,
    ) -> Self {
        let tunnel_url = record
            .subdomain
            .as_deref()
            .map(|subdomain| state.config.tunnel_url(subdomain));
        Self {
            client_alias: client_alias(&state.server_logs.cursor_key, &record.installation_id),
            installation_id: record.installation_id,
            owned,
            subdomain: record.subdomain,
            tunnel_url,
            owner_email: record.owner_email,
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
    public_enabled: bool,
    authenticated: bool,
    is_admin: bool,
    scopes: Vec<String>,
    clients: Vec<ServerLogClientView>,
    retention_days: u32,
    public_window_seconds: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorPayload {
    received_at_ms: i64,
    event_id: String,
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
    let dynamic = state.dynamic.read().await;
    let public_enabled = dynamic.server_log_public_enabled;
    let authenticated = session.is_some();
    let is_admin = session
        .as_ref()
        .is_some_and(|session| dynamic.is_admin(&session.email));
    drop(dynamic);
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
    let clients = if authenticated {
        let visible_installation_ids = (!is_admin).then_some(&owned_installation_ids);
        state
            .store
            .list_server_log_client_records(visible_installation_ids)
            .await?
            .into_iter()
            .filter(|record| is_admin || owned_installation_ids.contains(&record.installation_id))
            .map(|record| {
                let owned = owned_installation_ids.contains(&record.installation_id);
                ServerLogClientView::from_record(record, &state, owned)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut scopes = Vec::new();
    if public_enabled {
        scopes.push("public".to_string());
    }
    if authenticated {
        scopes.push("mine".to_string());
    }
    if is_admin {
        scopes.push("all".to_string());
    }
    Ok(Json(ServerLogMetaResponse {
        ingest_enabled: state.server_logs.config.enabled,
        public_enabled,
        authenticated,
        is_admin,
        scopes,
        clients,
        retention_days: state.server_logs.config.retention_days,
        public_window_seconds: 300,
    }))
}

async fn server_log_events(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ServerLogQuery>,
) -> Result<Json<ServerLogEventsResponse>, AppError> {
    let access = resolve_query_access(&state, &headers, query.scope.as_deref()).await?;
    validate_requested_installation(&access, query.installation_id.as_deref())?;
    let mut response = state.server_logs.query(access, query).await?;
    enrich_client_subdomains(&state, &mut response).await?;
    Ok(Json(response))
}

async fn server_log_export(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(mut query): Query<ServerLogQuery>,
) -> Result<Response, AppError> {
    let access = resolve_query_access(&state, &headers, query.scope.as_deref()).await?;
    if access.public_only {
        return Err(AppError::Unauthorized(
            "login required to export server logs".into(),
        ));
    }
    validate_requested_installation(&access, query.installation_id.as_deref())?;
    query.limit = Some(MAX_QUERY_LIMIT);
    query.cursor = None;
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
    requested_scope: Option<&str>,
) -> Result<QueryAccess, AppError> {
    let session = crate::api::resolve_router_session(state, headers).await?;
    let public_enabled = state.dynamic.read().await.server_log_public_enabled;
    let scope = requested_scope
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(if session.is_some() { "mine" } else { "public" });
    let is_admin = if let Some(session) = session.as_ref() {
        state.dynamic.read().await.is_admin(&session.email)
    } else {
        false
    };
    let installation_ids = match (scope, session.as_ref()) {
        ("mine", Some(session)) => state
            .store
            .list_verified_installation_ids_for_owner(&session.email)
            .await?
            .into_iter()
            .collect(),
        _ => HashSet::new(),
    };
    authorize_query_access(
        public_enabled,
        session.is_some(),
        is_admin,
        installation_ids,
        scope,
    )
}

fn authorize_query_access(
    public_enabled: bool,
    authenticated: bool,
    is_admin: bool,
    installation_ids: HashSet<String>,
    scope: &str,
) -> Result<QueryAccess, AppError> {
    match scope {
        "public" if public_enabled => Ok(QueryAccess {
            public_only: true,
            all_installations: false,
            installation_ids: HashSet::new(),
        }),
        "public" => Err(AppError::Forbidden(
            "public server logs are disabled".into(),
        )),
        "mine" => {
            if !authenticated {
                return Err(AppError::Unauthorized(
                    "login required for server logs".into(),
                ));
            }
            Ok(QueryAccess {
                public_only: false,
                all_installations: false,
                installation_ids,
            })
        }
        "all" => {
            if !authenticated {
                return Err(AppError::Unauthorized(
                    "login required for server logs".into(),
                ));
            }
            if !is_admin {
                return Err(AppError::Forbidden(
                    "admin privilege required for all server logs".into(),
                ));
            }
            Ok(QueryAccess {
                public_only: false,
                all_installations: true,
                installation_ids: HashSet::new(),
            })
        }
        _ => Err(AppError::BadRequest(
            "server log scope must be public, mine, or all".into(),
        )),
    }
}

fn validate_requested_installation(
    access: &QueryAccess,
    requested: Option<&str>,
) -> Result<(), AppError> {
    if requested.is_some_and(|installation_id| {
        access.public_only
            || (!access.all_installations && !access.installation_ids.contains(installation_id))
    }) {
        return Err(AppError::NotFound("server log client not found".into()));
    }
    Ok(())
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

fn append_records(path: &Path, records: &[StoredServerLogEvent]) -> Result<u64, AppError> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| AppError::Internal(format!("open server log file failed: {error}")))?;
    let previous_len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    for record in records {
        serde_json::to_writer(&mut file, record).map_err(|error| {
            AppError::Internal(format!("encode server log event failed: {error}"))
        })?;
        file.write_all(b"\n").map_err(|error| {
            AppError::Internal(format!("append server log event failed: {error}"))
        })?;
    }
    file.sync_data()
        .map_err(|error| AppError::Internal(format!("sync server log file failed: {error}")))?;
    Ok(file
        .metadata()
        .map(|metadata| metadata.len().saturating_sub(previous_len))
        .unwrap_or(0))
}

fn rotate_active_file(directory: &Path, day: &str) -> Result<(), AppError> {
    let source = directory.join(format!("active-{day}.jsonl"));
    if !source.is_file() || source.metadata().map(|meta| meta.len()).unwrap_or(0) == 0 {
        return Ok(());
    }
    repair_trailing_partial_line(&source)?;
    let suffix = format!(
        "{}-{:016x}",
        Utc::now().timestamp_millis(),
        rand::thread_rng().next_u64()
    );
    let destination = directory.join(format!("segment-{day}-{suffix}.jsonl.gz"));
    let temporary = directory.join(format!(".segment-{day}-{suffix}.jsonl.gz.new"));
    let mut input = File::open(&source)
        .map_err(|error| AppError::Internal(format!("open log segment failed: {error}")))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let output = options.open(&temporary).map_err(|error| {
        AppError::Internal(format!("create compressed log segment failed: {error}"))
    })?;
    let mut encoder = GzEncoder::new(output, Compression::default());
    let compression_result = (|| {
        std::io::copy(&mut input, &mut encoder).map_err(|error| {
            AppError::Internal(format!("compress server log segment failed: {error}"))
        })?;
        let output = encoder.finish().map_err(|error| {
            AppError::Internal(format!("finish server log segment failed: {error}"))
        })?;
        output
            .sync_all()
            .map_err(|error| AppError::Internal(format!("sync server log segment failed: {error}")))
    })();
    if let Err(error) = compression_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        return Err(AppError::Internal(format!(
            "publish compressed server log segment failed: {error}"
        )));
    }
    fs::remove_file(&source).map_err(|error| {
        AppError::Internal(format!("remove rotated server log file failed: {error}"))
    })?;
    Ok(())
}

fn repair_trailing_partial_line(path: &Path) -> Result<(), AppError> {
    let Ok(mut file) = OpenOptions::new().read(true).write(true).open(path) else {
        return Ok(());
    };
    let length = file
        .metadata()
        .map_err(|error| AppError::Internal(format!("inspect server log file failed: {error}")))?
        .len();
    if length == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1))
        .map_err(|error| AppError::Internal(format!("seek server log file failed: {error}")))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)
        .map_err(|error| AppError::Internal(format!("read server log tail failed: {error}")))?;
    if last[0] == b'\n' {
        return Ok(());
    }
    let mut position = length;
    let mut buffer = [0u8; 4096];
    while position > 0 {
        let chunk = position.min(buffer.len() as u64) as usize;
        position -= chunk as u64;
        file.seek(SeekFrom::Start(position)).map_err(|error| {
            AppError::Internal(format!("seek server log recovery failed: {error}"))
        })?;
        file.read_exact(&mut buffer[..chunk]).map_err(|error| {
            AppError::Internal(format!("read server log recovery failed: {error}"))
        })?;
        if let Some(index) = buffer[..chunk].iter().rposition(|byte| *byte == b'\n') {
            file.set_len(position + index as u64 + 1).map_err(|error| {
                AppError::Internal(format!("truncate partial server log event failed: {error}"))
            })?;
            return Ok(());
        }
    }
    file.set_len(0).map_err(|error| {
        AppError::Internal(format!("truncate invalid server log file failed: {error}"))
    })
}

fn read_query_records(
    root: &Path,
    access: &QueryAccess,
    query: &ServerLogQuery,
    cursor: Option<&CursorPayload>,
    limit: usize,
) -> Result<Vec<StoredServerLogEvent>, AppError> {
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
    let mut files = event_files_for_access(root, access, query.installation_id.as_deref())?
        .into_iter()
        .map(|path| {
            let modified_at_ms = path
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
                .unwrap_or(i64::MAX);
            (path, modified_at_ms)
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.1.cmp(&left.1));

    let mut records = Vec::new();
    for (index, (path, _)) in files.iter().enumerate() {
        let mut from_file = Vec::new();
        read_records_file(path, &mut from_file)?;
        from_file.retain(|event| {
            event_matches_query(
                event,
                access,
                query,
                cursor,
                level.as_deref(),
                search.as_deref(),
            )
        });
        records.extend(from_file);
        records.sort_by(compare_events_descending);
        records.dedup_by(|left, right| left.event_id == right.event_id);
        records.truncate(limit.saturating_add(1));

        if records.len() > limit {
            let oldest_selected = records
                .last()
                .map(|event| event.received_at_ms)
                .unwrap_or(i64::MIN);
            let next_file_upper_bound = files
                .get(index + 1)
                .map(|(_, modified_at_ms)| *modified_at_ms)
                .unwrap_or(i64::MIN);
            if next_file_upper_bound.saturating_add(2_000) < oldest_selected {
                break;
            }
        }
    }
    Ok(records)
}

fn event_files_for_access(
    root: &Path,
    access: &QueryAccess,
    requested_installation: Option<&str>,
) -> Result<Vec<PathBuf>, AppError> {
    let mut files = Vec::new();
    if let Some(installation_id) = requested_installation {
        collect_event_files(&root.join(hash_component(installation_id)), &mut files).map_err(
            |error| AppError::Internal(format!("scan server log directory failed: {error}")),
        )?;
    } else if access.all_installations {
        collect_event_files(root, &mut files).map_err(|error| {
            AppError::Internal(format!("scan server log directory failed: {error}"))
        })?;
    } else {
        for installation_id in &access.installation_ids {
            collect_event_files(&root.join(hash_component(installation_id)), &mut files).map_err(
                |error| AppError::Internal(format!("scan server log directory failed: {error}")),
            )?;
        }
    }
    Ok(files)
}

fn event_matches_query(
    event: &StoredServerLogEvent,
    access: &QueryAccess,
    query: &ServerLogQuery,
    cursor: Option<&CursorPayload>,
    level: Option<&str>,
    search: Option<&str>,
) -> bool {
    if !access.public_only
        && !access.all_installations
        && !access.installation_ids.contains(&event.installation_id)
    {
        return false;
    }
    if let Some(requested) = query.installation_id.as_deref() {
        if access.public_only || event.installation_id != requested {
            return false;
        }
    }
    if query
        .from_ms
        .is_some_and(|from| event.received_at_ms < from)
        || query.to_ms.is_some_and(|to| event.received_at_ms > to)
        || level.is_some_and(|level| event.level != level)
    {
        return false;
    }
    if let Some(search) = search {
        let (target, message) = if access.public_only {
            (
                public_message(&event.target),
                public_message(&event.message),
            )
        } else {
            (event.target.clone(), event.message.clone())
        };
        if !format!("{target} {message}")
            .to_ascii_lowercase()
            .contains(search)
        {
            return false;
        }
    }
    if let Some(cursor) = cursor {
        if (event.received_at_ms, event.event_id.as_str())
            >= (cursor.received_at_ms, cursor.event_id.as_str())
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
        .then_with(|| right.event_id.cmp(&left.event_id))
}

fn read_records_file(path: &Path, records: &mut Vec<StoredServerLogEvent>) -> Result<(), AppError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(AppError::Internal(format!(
                "open server log segment failed: {error}"
            )));
        }
    };
    if path.extension().and_then(|value| value.to_str()) == Some("gz") {
        read_record_lines(BufReader::new(GzDecoder::new(file)), records)
    } else {
        read_record_lines(BufReader::new(file), records)
    }
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
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_event_files(&path, files)?;
        } else if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| {
                (name.starts_with("active-") && name.ends_with(".jsonl"))
                    || (name.starts_with("segment-") && name.ends_with(".jsonl.gz"))
            })
        {
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
            } else if path.file_name().and_then(|value| value.to_str()) == Some("cursor.json") {
                if let Ok(bytes) = fs::read(&path) {
                    if let Ok(cursor) = serde_json::from_slice::<StreamCursorFile>(&bytes) {
                        catalog.streams.insert(
                            (cursor.installation_id, cursor.stream_id),
                            StreamState {
                                last_sequence: cursor.last_sequence,
                                active_day: cursor.active_day,
                            },
                        );
                    }
                }
            }
        }
        Ok(())
    }
    visit(root, catalog)
}

fn recover_catalog_from_event_files(
    root: &Path,
    catalog: &mut Catalog,
    cursor_key: [u8; 32],
) -> Result<(), AppError> {
    let cutoff = Utc::now().timestamp_millis() - PUBLIC_WINDOW_MS;
    for path in event_files(root)? {
        let is_active = path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("active-"));
        let recently_modified = path
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
            .is_some_and(|duration| duration.as_millis().min(i64::MAX as u128) as i64 >= cutoff);
        if !is_active && !recently_modified {
            continue;
        }
        let mut records = Vec::new();
        read_records_file(&path, &mut records)?;
        for mut record in records {
            record.client_alias = client_alias(&cursor_key, &record.installation_id);
            let key = (record.installation_id.clone(), record.stream_id.clone());
            let state = catalog.streams.entry(key).or_insert_with(|| StreamState {
                last_sequence: 0,
                active_day: utc_day(record.received_at_ms),
            });
            state.last_sequence = state.last_sequence.max(record.sequence);
            if record.received_at_ms >= cutoff {
                catalog.public_recent.push_back(record);
            }
        }
    }
    catalog
        .public_recent
        .make_contiguous()
        .sort_by_key(|event| event.received_at_ms);
    Ok(())
}

fn cleanup_files(config: &ServerLogRuntimeConfig) -> Result<u64, AppError> {
    let mut files = event_files(&config.root)?
        .into_iter()
        .filter_map(|path| {
            let metadata = path.metadata().ok()?;
            let modified = metadata.modified().ok()?;
            Some((path, metadata.len(), modified))
        })
        .collect::<Vec<_>>();
    let now = SystemTime::now();
    let retention = Duration::from_secs(u64::from(config.retention_days) * 24 * 60 * 60);
    for (path, _, modified) in &files {
        if now
            .duration_since(*modified)
            .is_ok_and(|age| age > retention)
        {
            let _ = fs::remove_file(path);
        }
    }
    files.retain(|(path, _, _)| path.exists());
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut total = files.iter().map(|(_, bytes, _)| *bytes).sum::<u64>();
    for (path, bytes, _) in files {
        if total <= config.max_total_bytes {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(bytes);
        }
    }
    cleanup_stale_metadata(&config.root, &config.root, now, retention).map_err(|error| {
        AppError::Internal(format!("clean server log cursor metadata failed: {error}"))
    })?;
    Ok(total)
}

fn cleanup_stale_metadata(
    root: &Path,
    directory: &Path,
    now: SystemTime,
    retention: Duration,
) -> std::io::Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(directory)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    for path in &entries {
        if path.is_dir() {
            cleanup_stale_metadata(root, path, now, retention)?;
        }
    }

    let remaining = fs::read_dir(directory)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    let has_event_files = remaining.iter().any(|path| is_event_file(path));
    for path in remaining.iter().filter(|path| path.is_file()) {
        let name = path.file_name().and_then(|value| value.to_str());
        let is_cursor = name == Some("cursor.json");
        let is_temporary =
            name.is_some_and(|name| name.ends_with(".json.new") || name.ends_with(".gz.new"));
        if (!is_cursor && !is_temporary) || (is_cursor && has_event_files) {
            continue;
        }
        let stale = path
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > retention);
        if stale {
            let _ = fs::remove_file(path);
        }
    }

    if directory != root && fs::read_dir(directory)?.next().is_none() {
        let _ = fs::remove_dir(directory);
    }
    Ok(())
}

fn stream_has_persistent_state(root: &Path, installation_id: &str, stream_id: &str) -> bool {
    let directory = stream_directory(root, installation_id, stream_id);
    if directory.join("cursor.json").is_file() {
        return true;
    }
    fs::read_dir(directory).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| is_event_file(&entry.path()))
    })
}

fn is_event_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| {
            (name.starts_with("active-") && name.ends_with(".jsonl"))
                || (name.starts_with("segment-") && name.ends_with(".jsonl.gz"))
        })
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
    if let Ok(bytes) = fs::read(&path) {
        if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(key);
        }
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

fn encode_cursor(payload: &CursorPayload, key: &[u8; 32]) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(payload)
        .map_err(|error| AppError::Internal(format!("encode server log cursor: {error}")))?;
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| AppError::Internal("initialize server log cursor signer".into()))?;
    mac.update(&bytes);
    let signature = mac.finalize().into_bytes();
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(bytes),
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

fn decode_cursor(value: &str, key: &[u8; 32]) -> Result<CursorPayload, AppError> {
    let (payload, signature) = value
        .split_once('.')
        .ok_or_else(|| AppError::BadRequest("invalid server log cursor".into()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| AppError::BadRequest("invalid server log cursor".into()))?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| AppError::BadRequest("invalid server log cursor".into()))?;
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| AppError::Internal("initialize server log cursor verifier".into()))?;
    mac.update(&bytes);
    mac.verify_slice(&signature)
        .map_err(|_| AppError::BadRequest("invalid server log cursor signature".into()))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::BadRequest("invalid server log cursor payload".into()))
}

fn stream_directory(root: &Path, installation_id: &str, stream_id: &str) -> PathBuf {
    root.join(hash_component(installation_id))
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
    format!("client-{}", hex::encode(&digest[..5]))
}

fn utc_day(timestamp_ms: i64) -> String {
    Utc.timestamp_millis_opt(timestamp_ms)
        .single()
        .unwrap_or_else(Utc::now)
        .format("%Y-%m-%d")
        .to_string()
}

fn prune_public_window(events: &mut VecDeque<StoredServerLogEvent>, now_ms: i64) {
    let cutoff = now_ms - PUBLIC_WINDOW_MS;
    while events
        .front()
        .is_some_and(|event| event.received_at_ms < cutoff)
    {
        events.pop_front();
    }
    while events.len() > 10_000 {
        events.pop_front();
    }
}

fn is_public_window_event(event: &StoredServerLogEvent, now_ms: i64) -> bool {
    event.occurred_at_ms >= now_ms.saturating_sub(PUBLIC_WINDOW_MS)
        && event.occurred_at_ms <= now_ms.saturating_add(PUBLIC_CLOCK_SKEW_MS)
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
            if let Some(start) = lower.find("bearer ") {
                return format!("{}Bearer [REDACTED]", &line[..start]);
            }
            for key in KEYS {
                let Some(start) = lower.find(key) else {
                    continue;
                };
                let after_key = start + key.len();
                let suffix = &line[after_key..];
                let Some(separator) = suffix
                    .char_indices()
                    .find(|(index, character)| *index <= 3 && matches!(character, ':' | '='))
                    .map(|(index, _)| index)
                else {
                    continue;
                };
                let end = after_key + separator + 1;
                return format!("{} [REDACTED]", line[..end].trim_end());
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
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

pub fn public_enabled_from_env() -> bool {
    env_bool("CC_SWITCH_ROUTER_SERVER_LOG_PUBLIC_ENABLED", false)
}

fn env_u32(key: &str, default: u32, min: u32, max: u32) -> Result<u32, AppError> {
    let value = std::env::var(key)
        .ok()
        .map(|value| value.trim().parse::<u32>())
        .transpose()
        .map_err(|_| AppError::Internal(format!("{key} must be an integer")))?
        .unwrap_or(default);
    if !(min..=max).contains(&value) {
        return Err(AppError::Internal(format!(
            "{key} must be between {min} and {max}"
        )));
    }
    Ok(value)
}

fn env_u64(key: &str, default: u64, min: u64, max: u64) -> Result<u64, AppError> {
    let value = std::env::var(key)
        .ok()
        .map(|value| value.trim().parse::<u64>())
        .transpose()
        .map_err(|_| AppError::Internal(format!("{key} must be an integer")))?
        .unwrap_or(default);
    if !(min..=max).contains(&value) {
        return Err(AppError::Internal(format!(
            "{key} must be between {min} and {max}"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (Arc<ServerLogStore>, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-server-logs-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = ServerLogStore::open(ServerLogRuntimeConfig {
            enabled: true,
            root: root.clone(),
            retention_days: 7,
            max_total_bytes: 16 * 1024 * 1024,
        })
        .expect("open log store");
        (Arc::new(store), root)
    }

    fn payload(sequence: u64) -> InstallationLogBatchPayload {
        InstallationLogBatchPayload {
            protocol_version: 1,
            stream_id: "stream-a".into(),
            server_version: "0.1.0".into(),
            commit_id: "abc".into(),
            events: vec![InstallationLogEvent {
                sequence,
                occurred_at_ms: Utc::now().timestamp_millis(),
                level: "info".into(),
                target: "cc_switch_server::state remote=203.0.113.2".into(),
                message: "started token=secret user@example.com 203.0.113.2".into(),
                fields: BTreeMap::from([
                    ("operation".into(), Value::String("start".into())),
                    ("access_token".into(), Value::String("secret".into())),
                ]),
                file: Some("src/state.rs".into()),
                line: Some(42),
            }],
        }
    }

    #[tokio::test]
    async fn ingest_is_idempotent_and_public_projection_is_redacted() {
        let (store, root) = test_store();
        let first = store
            .ingest("installation-a".into(), payload(1))
            .await
            .expect("first ingest");
        assert_eq!(first.accepted, 1);
        let duplicate = store
            .ingest("installation-a".into(), payload(1))
            .await
            .expect("duplicate ingest");
        assert_eq!(duplicate.accepted, 0);

        let mut response = store
            .query(
                QueryAccess {
                    public_only: true,
                    all_installations: false,
                    installation_ids: HashSet::new(),
                },
                ServerLogQuery::default(),
            )
            .await
            .expect("query public logs");
        apply_client_subdomains(
            &mut response,
            &HashMap::from([("installation-a".to_string(), "client-sub".to_string())]),
        );
        assert_eq!(response.events.len(), 1);
        assert!(response.events[0].installation_id.is_none());
        assert_eq!(
            response.events[0].client_subdomain.as_deref(),
            Some("client-sub")
        );
        assert!(response.events[0].fields.is_none());
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
                QueryAccess {
                    public_only: true,
                    all_installations: false,
                    installation_ids: HashSet::new(),
                },
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
    async fn public_window_excludes_old_events_uploaded_from_a_backlog() {
        let (store, root) = test_store();
        let mut stale = payload(1);
        stale.events[0].occurred_at_ms = Utc::now().timestamp_millis() - PUBLIC_WINDOW_MS - 1;
        store
            .ingest("installation-a".into(), stale)
            .await
            .expect("ingest stale event");

        let public = store
            .query(
                QueryAccess {
                    public_only: true,
                    all_installations: false,
                    installation_ids: HashSet::new(),
                },
                ServerLogQuery::default(),
            )
            .await
            .expect("query public logs");
        assert!(public.events.is_empty());

        let private = store
            .query(
                QueryAccess {
                    public_only: false,
                    all_installations: false,
                    installation_ids: HashSet::from(["installation-a".to_string()]),
                },
                ServerLogQuery::default(),
            )
            .await
            .expect("query retained logs");
        assert_eq!(private.events.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ingest_rejects_sequence_gaps() {
        let (store, root) = test_store();
        let error = store
            .ingest("installation-a".into(), payload(2))
            .await
            .expect_err("sequence gap");
        assert_eq!(error.code(), Some("SERVER_LOG_SEQUENCE_GAP"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cursor_is_signed() {
        let key = [7u8; 32];
        let encoded = encode_cursor(
            &CursorPayload {
                received_at_ms: 10,
                event_id: "event".into(),
            },
            &key,
        )
        .expect("encode cursor");
        assert_eq!(decode_cursor(&encoded, &key).unwrap().event_id, "event");
        assert!(decode_cursor(&(encoded + "x"), &key).is_err());
    }

    #[test]
    fn query_authorization_covers_public_user_and_admin_scopes() {
        let owned = HashSet::from(["installation-a".to_string()]);
        let public = authorize_query_access(false, false, false, HashSet::new(), "public");
        assert!(matches!(public, Err(AppError::Forbidden(_))));

        let mine = authorize_query_access(true, false, false, HashSet::new(), "mine");
        assert!(matches!(mine, Err(AppError::Unauthorized(_))));

        let mine = authorize_query_access(true, true, false, owned.clone(), "mine").unwrap();
        assert_eq!(mine.installation_ids, owned);
        assert!(!mine.public_only);
        assert!(!mine.all_installations);

        let all = authorize_query_access(true, true, false, HashSet::new(), "all");
        assert!(matches!(all, Err(AppError::Forbidden(_))));
        let all = authorize_query_access(true, true, true, HashSet::new(), "all").unwrap();
        assert!(all.all_installations);
    }

    #[tokio::test]
    async fn owned_client_visibility_follows_current_access_set() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload(1))
            .await
            .unwrap();
        store
            .ingest("installation-b".into(), payload(1))
            .await
            .unwrap();

        let first_access = QueryAccess {
            public_only: false,
            all_installations: false,
            installation_ids: HashSet::from(["installation-a".to_string()]),
        };
        let first = store
            .query(first_access, ServerLogQuery::default())
            .await
            .unwrap();
        assert_eq!(first.events.len(), 1);
        assert_eq!(
            first.events[0].installation_id.as_deref(),
            Some("installation-a")
        );

        let changed_access = QueryAccess {
            public_only: false,
            all_installations: false,
            installation_ids: HashSet::from(["installation-b".to_string()]),
        };
        assert!(matches!(
            validate_requested_installation(&changed_access, Some("installation-a")),
            Err(AppError::NotFound(_))
        ));
        let changed = store
            .query(changed_access, ServerLogQuery::default())
            .await
            .unwrap();
        assert_eq!(changed.events.len(), 1);
        assert_eq!(
            changed.events[0].installation_id.as_deref(),
            Some("installation-b")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn batch_validation_rejects_non_info_family_levels() {
        let mut invalid = payload(1);
        invalid.events[0].level = "debug".into();
        assert!(matches!(
            validate_batch(&invalid),
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
    }

    #[tokio::test]
    async fn cleanup_removes_expired_stream_metadata_and_empty_directories() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload(1))
            .await
            .expect("ingest event");
        let directory = stream_directory(&root, "installation-a", "stream-a");
        assert!(directory.join("cursor.json").is_file());
        std::thread::sleep(Duration::from_millis(10));

        let mut cleanup_config = store.config.clone();
        cleanup_config.retention_days = 0;
        cleanup_files(&cleanup_config).expect("clean expired logs");

        assert!(!directory.exists());
        assert!(!stream_has_persistent_state(
            &root,
            "installation-a",
            "stream-a"
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ingest_enforces_capacity_when_the_approximate_counter_crosses_the_limit() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-server-log-capacity-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = Arc::new(
            ServerLogStore::open(ServerLogRuntimeConfig {
                enabled: true,
                root: root.clone(),
                retention_days: 7,
                max_total_bytes: 1,
            })
            .expect("open capacity-limited log store"),
        );

        let response = store
            .ingest("installation-a".into(), payload(1))
            .await
            .expect("ingest over capacity");
        assert_eq!(response.accepted, 1);
        assert!(event_files(&root).unwrap().is_empty());
        assert_eq!(store.stored_event_bytes.load(Ordering::Acquire), 0);
        let _ = fs::remove_dir_all(root);
    }
}
