use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use chrono::{TimeZone, Utc};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, Semaphore};

use crate::ServerState;
use crate::error::AppError;

pub const INSTALLATION_AUDIT_BATCH_ACTION: &str = "installation_audit_batch_v1";
const LOG_BATCH_BODY_LIMIT_BYTES: usize = 1024 * 1024;
const MAX_BATCH_EVENTS: usize = 256;
const MAX_FIELDS_BYTES: usize = 32 * 1024;
const MAX_QUERY_LIMIT: usize = 500;
const DEFAULT_QUERY_LIMIT: usize = 200;
const PUBLIC_WINDOW_MS: i64 = 5 * 60 * 1_000;
const PUBLIC_CLOCK_SKEW_MS: i64 = 60 * 1_000;
const SEGMENT_MAX_BYTES: u64 = 16 * 1024 * 1024;
const CURSOR_KEY_FILE: &str = ".cursor-key";
const SEGMENT_MANIFEST_FILE: &str = "segments.json";
const SEGMENT_MANIFEST_VERSION: u8 = 1;
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(15 * 60);
const MAINTENANCE_DEBOUNCE: Duration = Duration::from_millis(500);
const PUBLIC_INSTALLATION_CACHE_TTL: Duration = Duration::from_secs(2);

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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationAuditEvent {
    pub schema_version: u8,
    pub sequence: u64,
    pub timestamp_ms: i64,
    pub boot_id: String,
    pub level: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_provider_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationAuditBatchPayload {
    pub protocol_version: u8,
    pub boot_id: String,
    pub server_version: String,
    pub commit_id: String,
    pub events: Vec<InstallationAuditEvent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationAuditBatchRequest {
    pub protocol_epoch: String,
    pub installation_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
    #[serde(flatten)]
    pub payload: InstallationAuditBatchPayload,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallationAuditBatchResponse {
    ok: bool,
    accepted: usize,
    boot_id: String,
    sequence: u64,
    gap_detected: bool,
    restart_detected: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredServerLogEvent {
    event_id: String,
    installation_id: String,
    client_alias: String,
    stream_id: String,
    sequence: u64,
    #[serde(default)]
    ingest_order: u64,
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
    cursor_dirty: bool,
    catalog_dirty: bool,
}

#[derive(Debug, Default)]
struct StreamRegistry {
    streams: HashMap<(String, String), Arc<Mutex<StreamState>>>,
}

#[derive(Debug, Clone)]
struct PublicInstallationCache {
    computed_at: Instant,
    installation_ids: HashSet<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SegmentMeta {
    path: String,
    installation_id: String,
    stream_id: String,
    event_count: u64,
    bytes: u64,
    modified_at_ms: i64,
    min_received_at_ms: i64,
    max_received_at_ms: i64,
    min_occurred_at_ms: i64,
    max_occurred_at_ms: i64,
    min_ingest_order: u64,
    max_ingest_order: u64,
    max_sequence: u64,
    active: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SegmentManifest {
    version: u8,
    generated_at_ms: i64,
    segments: Vec<SegmentMeta>,
}

pub struct ServerLogStore {
    config: ServerLogRuntimeConfig,
    streams: Mutex<StreamRegistry>,
    segments: RwLock<Vec<SegmentMeta>>,
    files_guard: RwLock<()>,
    file_leases: Mutex<HashMap<PathBuf, usize>>,
    public_installation_cache: Mutex<Option<PublicInstallationCache>>,
    cursor_key: [u8; 32],
    next_ingest_order: AtomicU64,
    stored_event_bytes: AtomicU64,
    maintenance_requested: AtomicBool,
    maintenance_notify: Notify,
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
        let stored_event_bytes = cleanup_files(&config, &HashSet::new())?;
        let cursor_key = load_or_create_cursor_key(&config.root)?;
        let mut streams = StreamRegistry::default();
        load_stream_cursors(&config.root, &mut streams)?;
        let (segments, max_ingest_order) =
            recover_catalog_from_event_files(&config.root, &mut streams)?;
        write_segment_manifest(&config.root, &segments)?;
        Ok(Self {
            config,
            streams: Mutex::new(streams),
            segments: RwLock::new(segments),
            files_guard: RwLock::new(()),
            file_leases: Mutex::new(HashMap::new()),
            public_installation_cache: Mutex::new(None),
            cursor_key,
            next_ingest_order: AtomicU64::new(max_ingest_order),
            stored_event_bytes: AtomicU64::new(stored_event_bytes),
            maintenance_requested: AtomicBool::new(false),
            maintenance_notify: Notify::new(),
            ingest_slots: Arc::new(Semaphore::new(4)),
            query_slots: Arc::new(Semaphore::new(4)),
        })
    }

    pub fn config(&self) -> &ServerLogRuntimeConfig {
        &self.config
    }

    pub(crate) fn installation_ids_with_log_state(&self) -> Result<HashSet<String>, AppError> {
        let _files = self
            .files_guard
            .read()
            .map_err(|_| AppError::Internal("server log file guard poisoned".into()))?;
        Ok(self
            .segments
            .read()
            .map_err(|_| AppError::Internal("server log segment catalog poisoned".into()))?
            .iter()
            .filter(|segment| segment.path(&self.config.root).is_file())
            .map(|segment| segment.installation_id.clone())
            .collect())
    }

    pub(crate) async fn public_recent_installation_ids(
        self: &Arc<Self>,
    ) -> Result<HashSet<String>, AppError> {
        let permit = Arc::clone(&self.query_slots)
            .acquire_owned()
            .await
            .map_err(|_| AppError::ServiceUnavailable("server log query service stopped".into()))?;
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            store.public_recent_installation_ids_sync()
        })
        .await
        .map_err(|error| AppError::Internal(format!("join server log query failed: {error}")))?
    }

    fn public_recent_installation_ids_sync(&self) -> Result<HashSet<String>, AppError> {
        let mut cache = self.public_installation_cache.lock().map_err(|_| {
            AppError::Internal("server log public installation cache poisoned".into())
        })?;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.computed_at.elapsed() <= PUBLIC_INSTALLATION_CACHE_TTL)
        {
            return Ok(cached.installation_ids.clone());
        }
        let now_ms = Utc::now().timestamp_millis();
        let _files = self
            .files_guard
            .read()
            .map_err(|_| AppError::Internal("server log file guard poisoned".into()))?;
        let segments = self
            .segments
            .read()
            .map_err(|_| AppError::Internal("server log segment catalog poisoned".into()))?
            .iter()
            .filter(|segment| {
                segment.max_occurred_at_ms >= now_ms.saturating_sub(PUBLIC_WINDOW_MS)
                    && segment.min_occurred_at_ms <= now_ms.saturating_add(PUBLIC_CLOCK_SKEW_MS)
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut installation_ids = HashSet::new();
        for segment in segments {
            let mut records = Vec::new();
            read_records_file(&segment.path(&self.config.root), &mut records)?;
            if records
                .iter()
                .any(|event| is_public_window_event(event, now_ms))
            {
                installation_ids.insert(segment.installation_id);
            }
        }
        *cache = Some(PublicInstallationCache {
            computed_at: Instant::now(),
            installation_ids: installation_ids.clone(),
        });
        Ok(installation_ids)
    }

    pub fn spawn_maintenance(self: &Arc<Self>) {
        let store = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(MAINTENANCE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = store.maintenance_notify.notified() => {
                        tokio::time::sleep(MAINTENANCE_DEBOUNCE).await;
                    }
                }
                store.maintenance_requested.store(false, Ordering::Release);
                let store = Arc::clone(&store);
                let result =
                    tokio::task::spawn_blocking(move || store.run_maintenance_sync()).await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::warn!(%error, "server log maintenance failed"),
                    Err(error) => tracing::warn!(%error, "join server log maintenance failed"),
                }
            }
        });
    }

    fn request_maintenance(&self) {
        if !self.maintenance_requested.swap(true, Ordering::AcqRel) {
            self.maintenance_notify.notify_one();
        }
    }

    fn update_active_segment(
        &self,
        path: &Path,
        records: &[StoredServerLogEvent],
    ) -> Result<(), AppError> {
        let Some(first) = records.first() else {
            return Ok(());
        };
        let relative_path = relative_event_path(&self.config.root, path)?;
        let metadata = path.metadata().map_err(|error| {
            AppError::Internal(format!("inspect active server log segment failed: {error}"))
        })?;
        let modified_at_ms = system_time_ms(metadata.modified().unwrap_or(SystemTime::now()));
        let mut segments = self
            .segments
            .write()
            .map_err(|_| AppError::Internal("server log segment catalog poisoned".into()))?;
        if let Some(segment) = segments
            .iter_mut()
            .find(|segment| segment.path == relative_path)
        {
            segment.extend(records, metadata.len(), modified_at_ms);
        } else {
            segments.push(SegmentMeta::from_records(
                relative_path,
                metadata.len(),
                modified_at_ms,
                true,
                records,
            )?);
            self.request_maintenance();
        }
        debug_assert_eq!(first.installation_id, records[0].installation_id);
        Ok(())
    }

    fn run_maintenance_sync(&self) -> Result<(), AppError> {
        let stream_slots = {
            let streams = self.streams.lock().map_err(|_| {
                AppError::Internal("server log stream registry lock poisoned".into())
            })?;
            streams
                .streams
                .iter()
                .map(|(key, slot)| (key.clone(), Arc::clone(slot)))
                .collect::<Vec<_>>()
        };
        for ((installation_id, stream_id), slot) in stream_slots {
            let stream = slot
                .lock()
                .map_err(|_| AppError::Internal("server log stream lock poisoned".into()))?;
            self.rotate_stream_files(&installation_id, &stream_id, &stream.active_day)?;
        }

        let stored_event_bytes = {
            let _files = self
                .files_guard
                .write()
                .map_err(|_| AppError::Internal("server log file guard poisoned".into()))?;
            let leased_files = self.leased_file_paths()?;
            let stored_event_bytes = cleanup_files(&self.config, &leased_files)?;
            let mut segments = self
                .segments
                .write()
                .map_err(|_| AppError::Internal("server log segment catalog poisoned".into()))?;
            segments.retain(|segment| segment.path(&self.config.root).is_file());
            write_segment_manifest(&self.config.root, &segments)?;
            stored_event_bytes
        };
        self.stored_event_bytes
            .store(stored_event_bytes, Ordering::Release);
        let mut streams = self
            .streams
            .lock()
            .map_err(|_| AppError::Internal("server log stream registry lock poisoned".into()))?;
        streams.streams.retain(|(installation_id, stream_id), _| {
            stream_has_persistent_state(&self.config.root, installation_id, stream_id)
        });
        Ok(())
    }

    fn rotate_stream_files(
        &self,
        installation_id: &str,
        stream_id: &str,
        active_day: &str,
    ) -> Result<(), AppError> {
        let directory = stream_directory(&self.config.root, installation_id, stream_id);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(AppError::Internal(format!(
                    "scan active server log segments failed: {error}"
                )));
            }
        };
        for entry in entries {
            let path = entry
                .map_err(|error| {
                    AppError::Internal(format!("read active server log segment failed: {error}"))
                })?
                .path();
            let Some(day) = active_segment_day(&path) else {
                continue;
            };
            let bytes = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            if day == active_day && bytes < SEGMENT_MAX_BYTES {
                continue;
            }
            let Some(prepared) = prepare_active_file_rotation(&directory, day)? else {
                continue;
            };
            let _files = self
                .files_guard
                .write()
                .map_err(|_| AppError::Internal("server log file guard poisoned".into()))?;
            if self.is_file_leased(&prepared.source)? {
                let _ = fs::remove_file(&prepared.temporary);
                continue;
            }
            publish_active_file_rotation(&prepared)?;
            let source_relative = relative_event_path(&self.config.root, &prepared.source)?;
            let destination_relative =
                relative_event_path(&self.config.root, &prepared.destination)?;
            let destination_metadata = prepared.destination.metadata().map_err(|error| {
                AppError::Internal(format!(
                    "inspect rotated server log segment failed: {error}"
                ))
            })?;
            let mut segments = self
                .segments
                .write()
                .map_err(|_| AppError::Internal("server log segment catalog poisoned".into()))?;
            if let Some(mut segment) = segments
                .iter()
                .find(|segment| segment.path == source_relative)
                .cloned()
            {
                segment.path = destination_relative;
                segment.bytes = destination_metadata.len();
                segment.modified_at_ms =
                    system_time_ms(destination_metadata.modified().unwrap_or(SystemTime::now()));
                segment.active = false;
                segments.retain(|candidate| candidate.path != source_relative);
                segments.push(segment);
            } else {
                let mut records = Vec::new();
                read_records_file(&prepared.destination, &mut records)?;
                segments.push(SegmentMeta::from_records(
                    destination_relative,
                    destination_metadata.len(),
                    system_time_ms(destination_metadata.modified().unwrap_or(SystemTime::now())),
                    false,
                    &records,
                )?);
            }
        }
        Ok(())
    }

    async fn ingest(
        self: &Arc<Self>,
        installation_id: String,
        payload: InstallationAuditBatchPayload,
    ) -> Result<InstallationAuditBatchResponse, AppError> {
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
        payload: InstallationAuditBatchPayload,
    ) -> Result<InstallationAuditBatchResponse, AppError> {
        let received_at_ms = Utc::now().timestamp_millis();
        let day = utc_day(received_at_ms);
        let key = (installation_id.clone(), payload.boot_id.clone());
        let (stream_slot, restart_detected) = {
            let mut streams = self.streams.lock().map_err(|_| {
                AppError::Internal("server log stream registry lock poisoned".into())
            })?;
            let restart_detected = !streams.streams.contains_key(&key)
                && streams
                    .streams
                    .keys()
                    .any(|(stored_installation_id, _)| stored_installation_id == &installation_id);
            let stream_slot = Arc::clone(streams.streams.entry(key).or_insert_with(|| {
                Arc::new(Mutex::new(StreamState {
                    last_sequence: 0,
                    active_day: day.clone(),
                    cursor_dirty: false,
                    catalog_dirty: false,
                }))
            }));
            (stream_slot, restart_detected)
        };
        let mut stream = stream_slot
            .lock()
            .map_err(|_| AppError::Internal("server log stream lock poisoned".into()))?;
        let next_sequence = stream.last_sequence.saturating_add(1);
        let requested_sequence = payload
            .events
            .last()
            .map(|event| event.sequence)
            .unwrap_or(stream.last_sequence);
        let new_events = payload
            .events
            .iter()
            .filter(|event| event.sequence > stream.last_sequence)
            .cloned()
            .collect::<Vec<_>>();
        if new_events.is_empty() {
            self.repair_stream_metadata_if_needed(
                &installation_id,
                &payload.boot_id,
                &mut stream,
                received_at_ms,
            )?;
            return Ok(InstallationAuditBatchResponse {
                ok: true,
                accepted: 0,
                boot_id: payload.boot_id,
                sequence: requested_sequence,
                gap_detected: false,
                restart_detected: false,
            });
        }
        let gap_detected = new_events[0].sequence != next_sequence;
        for window in new_events.windows(2) {
            if window[1].sequence != window[0].sequence.saturating_add(1) {
                return Err(AppError::coded_conflict(
                    "SERVER_AUDIT_SEQUENCE_GAP",
                    "server audit batch sequences must be contiguous",
                    serde_json::json!({
                        "expectedSequence": window[0].sequence.saturating_add(1)
                    }),
                ));
            }
        }

        let directory = stream_directory(&self.config.root, &installation_id, &payload.boot_id);
        if stream.active_day != day {
            self.request_maintenance();
        }
        let active_path = directory.join(format!("active-{day}.jsonl"));

        let alias = client_alias(&self.cursor_key, &installation_id);
        let mut records = Vec::with_capacity(new_events.len());
        let ingest_order_start = self
            .next_ingest_order
            .fetch_add(new_events.len() as u64, Ordering::AcqRel)
            .saturating_add(1);
        for (index, event) in new_events.into_iter().enumerate() {
            let fields = audit_event_fields(&event)?;
            records.push(StoredServerLogEvent {
                event_id: event_id(&installation_id, &payload.boot_id, event.sequence),
                installation_id: installation_id.clone(),
                client_alias: alias.clone(),
                stream_id: payload.boot_id.clone(),
                sequence: event.sequence,
                ingest_order: ingest_order_start.saturating_add(index as u64),
                occurred_at_ms: event.timestamp_ms,
                received_at_ms,
                level: "info".to_string(),
                target: "cc_switch_server::audit".to_string(),
                message: event.event,
                fields,
                file: None,
                line: None,
                server_version: bounded_text(&payload.server_version, 128),
                commit_id: bounded_text(&payload.commit_id, 128),
            });
        }
        let _files = self
            .files_guard
            .read()
            .map_err(|_| AppError::Internal("server log file guard poisoned".into()))?;
        create_private_directory(&directory).map_err(|error| {
            AppError::Internal(format!(
                "create server log stream directory failed: {error}"
            ))
        })?;
        let appended_bytes = append_records(&active_path, &records)?;
        let approximate_stored_bytes = self
            .stored_event_bytes
            .fetch_add(appended_bytes, Ordering::AcqRel)
            .saturating_add(appended_bytes);
        let last_sequence = records
            .last()
            .map(|event| event.sequence)
            .unwrap_or(stream.last_sequence);
        if let Err(error) = write_json_atomic(
            &directory.join("cursor.json"),
            &StreamCursorFile {
                installation_id: installation_id.clone(),
                stream_id: payload.boot_id.clone(),
                last_sequence,
                active_day: day.clone(),
                updated_at_ms: received_at_ms,
            },
        ) {
            stream.last_sequence = last_sequence;
            stream.active_day.clone_from(&day);
            stream.cursor_dirty = true;
            stream.catalog_dirty = self.update_active_segment(&active_path, &records).is_err();
            self.request_maintenance();
            return Err(error);
        }
        if let Err(error) = self.update_active_segment(&active_path, &records) {
            stream.last_sequence = last_sequence;
            stream.active_day.clone_from(&day);
            stream.cursor_dirty = false;
            stream.catalog_dirty = true;
            self.request_maintenance();
            return Err(error);
        }
        stream.last_sequence = last_sequence;
        stream.active_day = day;
        stream.cursor_dirty = false;
        stream.catalog_dirty = false;
        drop(_files);
        if approximate_stored_bytes > self.config.max_total_bytes
            || active_path
                .metadata()
                .map(|metadata| metadata.len() >= SEGMENT_MAX_BYTES)
                .unwrap_or(false)
        {
            self.request_maintenance();
        }
        drop(stream);
        if gap_detected {
            tracing::warn!(
                installation_id = %records[0].installation_id,
                boot_id = %payload.boot_id,
                expected_sequence = next_sequence,
                received_sequence = records[0].sequence,
                "accepted server audit sequence gap after local spool loss"
            );
        }
        Ok(InstallationAuditBatchResponse {
            ok: true,
            accepted: records.len(),
            boot_id: payload.boot_id,
            sequence: requested_sequence,
            gap_detected,
            restart_detected,
        })
    }

    fn repair_stream_metadata_if_needed(
        &self,
        installation_id: &str,
        stream_id: &str,
        stream: &mut StreamState,
        updated_at_ms: i64,
    ) -> Result<(), AppError> {
        if !stream.cursor_dirty && !stream.catalog_dirty {
            return Ok(());
        }
        let _files = self
            .files_guard
            .read()
            .map_err(|_| AppError::Internal("server log file guard poisoned".into()))?;
        let directory = stream_directory(&self.config.root, installation_id, stream_id);
        if stream.cursor_dirty {
            write_json_atomic(
                &directory.join("cursor.json"),
                &StreamCursorFile {
                    installation_id: installation_id.to_string(),
                    stream_id: stream_id.to_string(),
                    last_sequence: stream.last_sequence,
                    active_day: stream.active_day.clone(),
                    updated_at_ms,
                },
            )?;
            stream.cursor_dirty = false;
        }
        if stream.catalog_dirty {
            let path = directory.join(format!("active-{}.jsonl", stream.active_day));
            self.refresh_active_segment(&path)?;
            stream.catalog_dirty = false;
        }
        Ok(())
    }

    fn refresh_active_segment(&self, path: &Path) -> Result<(), AppError> {
        let mut records = Vec::new();
        read_records_file(path, &mut records)?;
        if records.is_empty() {
            return Ok(());
        }
        let relative_path = relative_event_path(&self.config.root, path)?;
        let metadata = path.metadata().map_err(|error| {
            AppError::Internal(format!("inspect active server log segment failed: {error}"))
        })?;
        let segment = SegmentMeta::from_records(
            relative_path.clone(),
            metadata.len(),
            system_time_ms(metadata.modified().unwrap_or(SystemTime::now())),
            true,
            &records,
        )?;
        let mut segments = self
            .segments
            .write()
            .map_err(|_| AppError::Internal("server log segment catalog poisoned".into()))?;
        segments.retain(|candidate| candidate.path != relative_path);
        segments.push(segment);
        Ok(())
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

    async fn export_stream(
        self: &Arc<Self>,
        access: QueryAccess,
        query: ServerLogQuery,
        client_subdomains: HashMap<String, String>,
    ) -> Result<tokio::sync::mpsc::Receiver<Result<Bytes, io::Error>>, AppError> {
        let permit = Arc::clone(&self.query_slots)
            .acquire_owned()
            .await
            .map_err(|_| AppError::ServiceUnavailable("server log query service stopped".into()))?;
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        let store = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if let Err(error) = store.export_sync(access, query, &client_subdomains, &sender) {
                let _ = sender.blocking_send(Err(io::Error::other(error.to_string())));
            }
        });
        Ok(receiver)
    }

    fn export_sync(
        &self,
        access: QueryAccess,
        query: ServerLogQuery,
        client_subdomains: &HashMap<String, String>,
        sender: &tokio::sync::mpsc::Sender<Result<Bytes, io::Error>>,
    ) -> Result<(), AppError> {
        let server_time_ms = Utc::now().timestamp_millis();
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
        let _files = self
            .files_guard
            .read()
            .map_err(|_| AppError::Internal("server log file guard poisoned".into()))?;
        let mut segments = query_segments(&self.segments, &access, &query, None, server_time_ms)?;
        segments.sort_by(|left, right| compare_segments_descending(right, left));
        let mut leases = self.lease_segment_files(&segments)?;
        drop(_files);
        for segment in segments {
            let path = segment.path(&self.config.root);
            let mut records = Vec::new();
            let read_result = read_records_file(&path, &mut records);
            leases.release_path(&path);
            read_result?;
            records.retain(|event| {
                event_matches_query(
                    event,
                    &access,
                    &query,
                    None,
                    level.as_deref(),
                    search.as_deref(),
                    server_time_ms,
                )
            });
            records.sort_by(compare_events_descending);
            for record in records {
                let mut event = ServerLogEventView::from_stored(record, false);
                event.client_subdomain = client_subdomains
                    .get(&event.lookup_installation_id)
                    .cloned();
                let line = serialize_export_event(&event)
                    .map_err(|error| AppError::Internal(error.to_string()))?;
                if sender.blocking_send(Ok(line)).is_err() {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn lease_segment_files(
        &self,
        segments: &[SegmentMeta],
    ) -> Result<ServerLogFileLease<'_>, AppError> {
        let paths = segments
            .iter()
            .map(|segment| segment.path(&self.config.root))
            .collect::<Vec<_>>();
        let mut leases = self
            .file_leases
            .lock()
            .map_err(|_| AppError::Internal("server log file lease registry poisoned".into()))?;
        for path in &paths {
            *leases.entry(path.clone()).or_default() += 1;
        }
        Ok(ServerLogFileLease { store: self, paths })
    }

    fn leased_file_paths(&self) -> Result<HashSet<PathBuf>, AppError> {
        Ok(self
            .file_leases
            .lock()
            .map_err(|_| AppError::Internal("server log file lease registry poisoned".into()))?
            .keys()
            .cloned()
            .collect())
    }

    fn is_file_leased(&self, path: &Path) -> Result<bool, AppError> {
        Ok(self
            .file_leases
            .lock()
            .map_err(|_| AppError::Internal("server log file lease registry poisoned".into()))?
            .contains_key(path))
    }

    fn query_sync(
        &self,
        access: QueryAccess,
        query: ServerLogQuery,
    ) -> Result<ServerLogEventsResponse, AppError> {
        let server_time_ms = Utc::now().timestamp_millis();
        let limit = query
            .limit
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .clamp(1, MAX_QUERY_LIMIT);
        let cursor_query_fingerprint = query_cursor_fingerprint(&access, &query)?;
        let cursor = query
            .cursor
            .as_deref()
            .map(|value| decode_cursor(value, &self.cursor_key))
            .transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.query_fingerprint != cursor_query_fingerprint)
        {
            return Err(AppError::BadRequest(
                "server log cursor does not match the current query".into(),
            ));
        }
        let _files = self
            .files_guard
            .read()
            .map_err(|_| AppError::Internal("server log file guard poisoned".into()))?;
        let mut records = read_query_records(
            &self.config.root,
            &self.segments,
            &access,
            &query,
            cursor.as_ref(),
            limit,
            server_time_ms,
        )?;
        records.sort_by(compare_events_descending);
        let has_more = records.len() > limit;
        records.truncate(limit);
        let next_cursor = has_more
            .then(|| records.last())
            .flatten()
            .map(|event| {
                encode_cursor(
                    &CursorPayload {
                        ingest_order: event.ingest_order,
                        received_at_ms: event.received_at_ms,
                        event_id: event.event_id.clone(),
                        query_fingerprint: cursor_query_fingerprint.clone(),
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
            public_window_seconds: access
                .public_only
                .then_some(u32::try_from(PUBLIC_WINDOW_MS / 1_000).unwrap_or(300)),
            server_time_ms,
        })
    }

    pub(crate) async fn client_text_tail(
        self: &Arc<Self>,
        installation_id: &str,
        public_only: bool,
        limit: usize,
    ) -> Result<ServerLogTextTail, AppError> {
        let installation_id = installation_id.trim().to_string();
        let query = ServerLogQuery {
            installation_id: Some(installation_id.clone()),
            client_alias: None,
            limit: Some(limit),
            ..ServerLogQuery::default()
        };
        let access = QueryAccess {
            public_only,
            all_installations: false,
            installation_ids: (!public_only)
                .then(|| HashSet::from([installation_id]))
                .unwrap_or_default(),
        };
        let response = self.query(access, query).await?;
        let truncated = response.next_cursor.is_some();
        let mut events = response.events;
        events.reverse();
        let content = events
            .iter()
            .map(format_text_event)
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ServerLogTextTail {
            content,
            lines: events.len(),
            truncated,
        })
    }
}

pub(crate) struct ServerLogTextTail {
    pub content: String,
    pub lines: usize,
    pub truncated: bool,
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
    client_alias: Option<String>,
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
    #[serde(skip)]
    lookup_installation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_subdomain: Option<String>,
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
        Self {
            event_id: event.event_id,
            client_alias: event.client_alias,
            lookup_installation_id: event.installation_id.clone(),
            client_subdomain: None,
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
    server_time_ms: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerLogClientView {
    #[serde(skip_serializing_if = "Option::is_none")]
    installation_id: Option<String>,
    client_alias: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    subdomain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_email: Option<String>,
    platform: String,
    app_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    country_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<String>,
    created_at: String,
    last_seen_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel_enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerLogMetaResponse {
    ingest_enabled: bool,
    public_enabled: bool,
    authenticated: bool,
    is_router_owner: bool,
    scopes: Vec<String>,
    clients: Vec<ServerLogClientView>,
    retention_days: u32,
    public_window_seconds: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveClientLogTailResponse {
    installation_id: String,
    content: String,
    lines: usize,
    truncated: bool,
    fetched_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorPayload {
    ingest_order: u64,
    received_at_ms: i64,
    event_id: String,
    query_fingerprint: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorQueryIdentity<'a> {
    public_only: bool,
    all_installations: bool,
    installation_ids: Vec<&'a str>,
    installation_id: Option<&'a str>,
    client_alias: Option<&'a str>,
    level: Option<String>,
    search: Option<String>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route(
            "/v1/installations/audit-events/batch",
            post(ingest_installation_audit_events)
                .layer(DefaultBodyLimit::max(LOG_BATCH_BODY_LIMIT_BYTES)),
        )
        .route("/v1/server-logs/meta", get(server_log_meta))
        .route("/v1/server-logs/events", get(server_log_events))
        .route("/v1/server-logs/export", get(server_log_export))
        .route(
            "/v1/server-logs/clients/:installation_id/live-tail",
            get(live_client_log_tail),
        )
}

async fn ingest_installation_audit_events(
    State(state): State<ServerState>,
    Json(input): Json<InstallationAuditBatchRequest>,
) -> Result<Json<InstallationAuditBatchResponse>, AppError> {
    validate_batch(&input.payload)?;
    state
        .store
        .authenticate_installation_audit_batch(&input)
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
) -> Result<Response, AppError> {
    let session = crate::api::resolve_router_session(&state, &headers).await?;
    let public_enabled = state.dynamic.read().await.server_log_public_enabled;
    let authenticated = session.is_some();
    let is_router_owner = session
        .as_ref()
        .is_some_and(|session| session_is_router_owner(&state, &session.email));
    let log_installation_ids = state.server_logs.installation_ids_with_log_state()?;
    let owned_installation_ids = if let Some(session) = session.as_ref() {
        state
            .store
            .list_verified_installation_ids_for_owner(&session.email)
            .await?
            .into_iter()
            .filter(|installation_id| log_installation_ids.contains(installation_id))
            .collect()
    } else {
        HashSet::new()
    };
    let public_installation_ids = if public_enabled && !is_router_owner {
        state.server_logs.public_recent_installation_ids().await?
    } else {
        HashSet::new()
    };
    let installation_ids = if is_router_owner {
        log_installation_ids.clone()
    } else {
        owned_installation_ids
            .union(&public_installation_ids)
            .cloned()
            .collect()
    };
    let records = state
        .store
        .list_server_log_client_records(&installation_ids)
        .await?;
    let records_by_id = records
        .into_iter()
        .map(|record| (record.installation_id.clone(), record))
        .collect::<HashMap<_, _>>();
    let mut clients = installation_ids
        .into_iter()
        .map(|installation_id| {
            let client_alias = client_alias(&state.server_logs.cursor_key, &installation_id);
            let expose_private =
                is_router_owner || owned_installation_ids.contains(&installation_id);
            if let Some(record) = records_by_id.get(&installation_id) {
                ServerLogClientView {
                    installation_id: expose_private.then_some(installation_id),
                    client_alias,
                    subdomain: record.subdomain.clone(),
                    owner_email: expose_private.then(|| record.owner_email.clone()).flatten(),
                    platform: expose_private
                        .then(|| record.platform.clone())
                        .unwrap_or_default(),
                    app_version: expose_private
                        .then(|| record.app_version.clone())
                        .unwrap_or_default(),
                    country_code: expose_private
                        .then(|| record.country_code.clone())
                        .flatten(),
                    region: expose_private.then(|| record.region.clone()).flatten(),
                    created_at: expose_private
                        .then(|| record.created_at.to_rfc3339())
                        .unwrap_or_default(),
                    last_seen_at: expose_private
                        .then(|| record.last_seen_at.to_rfc3339())
                        .unwrap_or_default(),
                    tunnel_enabled: expose_private.then_some(record.tunnel_enabled).flatten(),
                }
            } else {
                ServerLogClientView {
                    installation_id: expose_private.then_some(installation_id),
                    client_alias,
                    subdomain: None,
                    owner_email: None,
                    platform: String::new(),
                    app_version: String::new(),
                    country_code: None,
                    region: None,
                    created_at: String::new(),
                    last_seen_at: String::new(),
                    tunnel_enabled: None,
                }
            }
        })
        .collect::<Vec<_>>();
    clients.sort_by(|left, right| {
        left.subdomain
            .as_deref()
            .unwrap_or(&left.client_alias)
            .cmp(right.subdomain.as_deref().unwrap_or(&right.client_alias))
    });
    let mut scopes = Vec::new();
    if public_enabled {
        scopes.push("public".to_string());
    }
    if authenticated {
        scopes.push("mine".to_string());
    }
    if is_router_owner {
        scopes.push("all".to_string());
    }
    Ok(no_store_json(ServerLogMetaResponse {
        ingest_enabled: state.server_logs.config.enabled,
        public_enabled,
        authenticated,
        is_router_owner,
        scopes,
        clients,
        retention_days: state.server_logs.config.retention_days,
        public_window_seconds: 300,
    }))
}

async fn enrich_client_subdomains(
    store: &crate::store::AppStore,
    response: &mut ServerLogEventsResponse,
) -> Result<(), AppError> {
    let installation_ids = response
        .events
        .iter()
        .map(|event| event.lookup_installation_id.clone())
        .collect::<HashSet<_>>();
    let subdomains = store
        .list_client_tunnel_subdomains_for_installations(&installation_ids)
        .await?;
    for event in &mut response.events {
        event.client_subdomain = subdomains.get(&event.lookup_installation_id).cloned();
    }
    Ok(())
}

fn session_is_router_owner(state: &ServerState, email: &str) -> bool {
    state
        .config
        .official_provider_email()
        .is_some_and(|owner_email| owner_email.eq_ignore_ascii_case(email.trim()))
}

async fn server_log_events(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ServerLogQuery>,
) -> Result<Response, AppError> {
    let access = resolve_query_access(&state, &headers, query.scope.as_deref()).await?;
    validate_requested_installation(&access, query.installation_id.as_deref())?;
    let mut response = state.server_logs.query(access, query).await?;
    enrich_client_subdomains(&state.store, &mut response).await?;
    Ok(no_store_json(response))
}

async fn live_client_log_tail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    AxumPath(installation_id): AxumPath<String>,
) -> Result<Response, AppError> {
    let installation_id = installation_id.trim();
    if installation_id.is_empty() || installation_id.len() > 128 {
        return Err(AppError::BadRequest(
            "invalid Client installation id".into(),
        ));
    }
    let session = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    if !session_is_router_owner(&state, &session.email) {
        return Err(AppError::Forbidden(
            "Router owner access is required for live diagnostics".into(),
        ));
    }
    if !state
        .store
        .client_log_installation_exists(installation_id)
        .await?
    {
        return Err(AppError::NotFound("Client not found".into()));
    }
    let _permit = state.client_logs.try_acquire(installation_id).await?;
    let route = state
        .proxy
        .active_client_route_for_installation(installation_id)
        .await
        .ok_or_else(|| AppError::ServiceUnavailable("Client is offline or reconnecting".into()))?;
    let control_secret = state
        .store
        .installation_control_secret(installation_id)
        .await?
        .ok_or_else(|| AppError::ServiceUnavailable("Client control is unavailable".into()))?;
    let reply = crate::ctl_client::fetch_client_log_tail(
        route.route_target(),
        installation_id,
        &control_secret,
        100,
    )
    .await
    .map_err(|error| match error {
        crate::ctl_client::CtlError::Rejected { status: 403, .. } => AppError::Conflict(
            "Client log collection is disabled; enable INFO log collection on the Server".into(),
        ),
        error if error.is_transport() => {
            AppError::ServiceUnavailable("Client live diagnostics are unavailable".into())
        }
        error => AppError::Internal(format!("read Client live diagnostics failed: {error}")),
    })?;
    Ok(no_store_json(LiveClientLogTailResponse {
        installation_id: installation_id.to_string(),
        content: reply.content,
        lines: reply.lines,
        truncated: reply.truncated,
        fetched_at: Utc::now().to_rfc3339(),
    }))
}

fn no_store_json<T: Serialize>(value: T) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}

async fn server_log_export(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ServerLogQuery>,
) -> Result<Response, AppError> {
    let access = resolve_query_access(&state, &headers, query.scope.as_deref()).await?;
    if access.public_only {
        return Err(AppError::Unauthorized(
            "login required to export server logs".into(),
        ));
    }
    validate_requested_installation(&access, query.installation_id.as_deref())?;
    build_server_log_export_response(state.server_logs, state.store, access, query).await
}

async fn build_server_log_export_response(
    server_logs: Arc<ServerLogStore>,
    store: crate::store::AppStore,
    access: QueryAccess,
    mut query: ServerLogQuery,
) -> Result<Response, AppError> {
    query.limit = None;
    query.cursor = None;
    let installation_ids = if let Some(installation_id) = query.installation_id.as_ref() {
        HashSet::from([installation_id.clone()])
    } else if access.all_installations {
        server_logs.installation_ids_with_log_state()?
    } else {
        access.installation_ids.clone()
    };
    let client_subdomains = store
        .list_client_tunnel_subdomains_for_installations(&installation_ids)
        .await?;
    let mut receiver = server_logs
        .export_stream(access, query, client_subdomains)
        .await?;
    let stream = async_stream::stream! {
        while let Some(item) = receiver.recv().await {
            yield item;
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=cc-switch-server-logs.jsonl",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .map_err(|error| AppError::Internal(format!("build log export response: {error}")))
}

fn serialize_export_event(event: &ServerLogEventView) -> io::Result<Bytes> {
    let mut line = serde_json::to_vec(event)
        .map_err(|error| io::Error::other(format!("serialize log export: {error}")))?;
    line.push(b'\n');
    Ok(Bytes::from(line))
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
    let is_router_owner = session
        .as_ref()
        .is_some_and(|session| session_is_router_owner(state, &session.email));
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
        is_router_owner,
        installation_ids,
        scope,
    )
}

fn authorize_query_access(
    public_enabled: bool,
    authenticated: bool,
    is_router_owner: bool,
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
            if !is_router_owner {
                return Err(AppError::Forbidden(
                    "Router owner privilege required for all server logs".into(),
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

fn validate_batch(payload: &InstallationAuditBatchPayload) -> Result<(), AppError> {
    if payload.protocol_version != 1 {
        return Err(AppError::BadRequest(
            "unsupported server audit protocol version".into(),
        ));
    }
    if !valid_identifier(&payload.boot_id, 128) {
        return Err(AppError::BadRequest(
            "server audit bootId is invalid".into(),
        ));
    }
    if payload.server_version.len() > 128
        || payload.commit_id.len() > 128
        || contains_sensitive_audit_value(&payload.server_version)
        || contains_sensitive_audit_value(&payload.commit_id)
    {
        return Err(AppError::BadRequest(
            "server audit build fields are invalid".into(),
        ));
    }
    if payload.events.is_empty() || payload.events.len() > MAX_BATCH_EVENTS {
        return Err(AppError::BadRequest(format!(
            "server audit batch must contain 1-{MAX_BATCH_EVENTS} events"
        )));
    }
    for (index, event) in payload.events.iter().enumerate() {
        if event.schema_version != 1 || event.level != "info" {
            return Err(AppError::BadRequest(
                "server audit events must use schemaVersion 1 and INFO level".into(),
            ));
        }
        if event.boot_id != payload.boot_id || event.sequence == 0 {
            return Err(AppError::BadRequest(
                "server audit event stream does not match the batch".into(),
            ));
        }
        if index > 0 && event.sequence != payload.events[index - 1].sequence.saturating_add(1) {
            return Err(AppError::BadRequest(
                "server audit event sequences must be contiguous".into(),
            ));
        }
        if !valid_event_name(&event.event) {
            return Err(AppError::BadRequest(
                "server audit event name is invalid".into(),
            ));
        }
        for value in [
            event.request_id.as_deref(),
            event.transport_request_id.as_deref(),
            event.parent_request_id.as_deref(),
            event.connection_id.as_deref(),
            event.turn_id.as_deref(),
            event.app.as_deref(),
            event.surface.as_deref(),
            event.operation.as_deref(),
            event.route.as_deref(),
            event.method.as_deref(),
            event.provider_type.as_deref(),
            event.provider_ref.as_deref(),
            event.previous_provider_ref.as_deref(),
            event.account_ref.as_deref(),
            event.outcome.as_deref(),
            event.stage.as_deref(),
            event.error_code.as_deref(),
            event.error_class.as_deref(),
            event.component.as_deref(),
            event.failure_kind.as_deref(),
            event.network_error_kind.as_deref(),
            event.retry_decision.as_deref(),
            event.stream_status.as_deref(),
        ] {
            if value.is_some_and(|value| {
                value.len() > 256
                    || value.contains(['\r', '\n'])
                    || contains_sensitive_audit_value(value)
            }) {
                return Err(AppError::BadRequest(
                    "server audit string field is invalid".into(),
                ));
            }
        }
        for value in [
            event.requested_model.as_deref(),
            event.actual_model.as_deref(),
        ] {
            if value.is_some_and(|value| {
                value.len() > 512
                    || value.contains(['\r', '\n'])
                    || contains_sensitive_audit_value(value)
            }) {
                return Err(AppError::BadRequest(
                    "server audit model field is invalid".into(),
                ));
            }
        }
        if event.body_sha256.as_deref().is_some_and(|value| {
            value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Err(AppError::BadRequest(
                "server audit bodySha256 is invalid".into(),
            ));
        }
        if event
            .provider_ref
            .as_deref()
            .is_some_and(|value| !valid_opaque_ref(value, "provider"))
            || event
                .previous_provider_ref
                .as_deref()
                .is_some_and(|value| !valid_opaque_ref(value, "provider"))
            || event
                .account_ref
                .as_deref()
                .is_some_and(|value| !valid_opaque_ref(value, "account"))
            || event
                .error_fingerprint
                .as_deref()
                .is_some_and(|value| !valid_opaque_ref(value, "error"))
        {
            return Err(AppError::BadRequest(
                "server audit identity reference is not opaque".into(),
            ));
        }
        if event
            .status_code
            .is_some_and(|status| !(100..=599).contains(&status))
            || event
                .upstream_status
                .is_some_and(|status| !(100..=599).contains(&status))
        {
            return Err(AppError::BadRequest(
                "server audit status code is invalid".into(),
            ));
        }
        let fields_bytes = serde_json::to_vec(event).map_err(|error| {
            AppError::BadRequest(format!("invalid server audit event: {error}"))
        })?;
        if fields_bytes.len() > MAX_FIELDS_BYTES {
            return Err(AppError::BadRequest(
                "server audit event is too large".into(),
            ));
        }
    }
    Ok(())
}

fn audit_event_fields(event: &InstallationAuditEvent) -> Result<BTreeMap<String, Value>, AppError> {
    let Value::Object(mut fields) = serde_json::to_value(event)
        .map_err(|error| AppError::Internal(format!("encode server audit event: {error}")))?
    else {
        return Err(AppError::Internal(
            "server audit event did not encode as an object".into(),
        ));
    };
    for key in [
        "schemaVersion",
        "sequence",
        "timestampMs",
        "bootId",
        "level",
        "event",
    ] {
        fields.remove(key);
    }
    Ok(fields.into_iter().collect())
}

fn valid_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn valid_event_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_')
        })
}

fn valid_opaque_ref(value: &str, kind: &str) -> bool {
    value
        .strip_prefix(kind)
        .and_then(|value| value.strip_prefix('_'))
        .is_some_and(|digest| {
            digest.len() == 16
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn contains_sensitive_audit_value(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    value.contains('@')
        || [
            "bearer ",
            "sk-",
            "sk_",
            "xai-",
            "ksk_",
            "ghp_",
            "github_pat_",
            "ya29.",
            "aiza",
            "eyj",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        || [
            "authorization:",
            "authorization=",
            "api_key:",
            "api_key=",
            "apikey:",
            "apikey=",
            "access_token:",
            "access_token=",
            "refresh_token:",
            "refresh_token=",
            "password:",
            "password=",
            "secret:",
            "secret=",
            "cookie:",
            "cookie=",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
}

fn append_records(path: &Path, records: &[StoredServerLogEvent]) -> Result<u64, AppError> {
    let mut encoded = Vec::new();
    for record in records {
        serde_json::to_writer(&mut encoded, record).map_err(|error| {
            AppError::Internal(format!("encode server log event failed: {error}"))
        })?;
        encoded.push(b'\n');
    }
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
    let previous_len = file
        .metadata()
        .map_err(|error| AppError::Internal(format!("inspect server log file failed: {error}")))?
        .len();
    if let Err(error) = file.write_all(&encoded).and_then(|_| file.sync_data()) {
        let rollback = file.set_len(previous_len).and_then(|_| file.sync_data());
        return Err(match rollback {
            Ok(()) => AppError::Internal(format!("append server log event failed: {error}")),
            Err(rollback_error) => AppError::Internal(format!(
                "append server log event failed: {error}; rollback failed: {rollback_error}"
            )),
        });
    }
    Ok(u64::try_from(encoded.len()).unwrap_or(u64::MAX))
}

struct PreparedActiveFileRotation {
    source: PathBuf,
    temporary: PathBuf,
    destination: PathBuf,
}

fn prepare_active_file_rotation(
    directory: &Path,
    day: &str,
) -> Result<Option<PreparedActiveFileRotation>, AppError> {
    let source = directory.join(format!("active-{day}.jsonl"));
    if !source.is_file() || source.metadata().map(|meta| meta.len()).unwrap_or(0) == 0 {
        return Ok(None);
    }
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
    Ok(Some(PreparedActiveFileRotation {
        source,
        temporary,
        destination,
    }))
}

fn publish_active_file_rotation(prepared: &PreparedActiveFileRotation) -> Result<(), AppError> {
    if let Err(error) = fs::rename(&prepared.temporary, &prepared.destination) {
        let _ = fs::remove_file(&prepared.temporary);
        return Err(AppError::Internal(format!(
            "publish compressed server log segment failed: {error}"
        )));
    }
    if let Err(error) = sync_parent_directory(&prepared.destination) {
        let _ = fs::remove_file(&prepared.destination);
        return Err(AppError::Internal(format!(
            "sync compressed server log segment directory failed: {error}"
        )));
    }
    if let Err(error) = fs::remove_file(&prepared.source) {
        let _ = fs::remove_file(&prepared.destination);
        let _ = sync_parent_directory(&prepared.destination);
        return Err(AppError::Internal(format!(
            "remove rotated server log file failed: {error}"
        )));
    }
    if let Err(error) = sync_parent_directory(&prepared.source) {
        tracing::warn!(
            %error,
            path = %prepared.source.display(),
            "rotated server log source removal directory sync failed; duplicate recovery may be required after a crash"
        );
    }
    Ok(())
}

fn active_segment_day(path: &Path) -> Option<&str> {
    path.file_name()?
        .to_str()?
        .strip_prefix("active-")?
        .strip_suffix(".jsonl")
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
    segment_catalog: &RwLock<Vec<SegmentMeta>>,
    access: &QueryAccess,
    query: &ServerLogQuery,
    cursor: Option<&CursorPayload>,
    limit: usize,
    server_time_ms: i64,
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
    let mut segments = query_segments(segment_catalog, access, query, cursor, server_time_ms)?;
    segments.sort_by(|left, right| compare_segments_descending(right, left));

    let mut records = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let mut from_file = Vec::new();
        read_records_file(&segment.path(root), &mut from_file)?;
        from_file.retain(|event| {
            event_matches_query(
                event,
                access,
                query,
                cursor,
                level.as_deref(),
                search.as_deref(),
                server_time_ms,
            )
        });
        records.extend(from_file);
        records.sort_by(compare_events_descending);
        let mut seen = HashSet::with_capacity(records.len());
        records.retain(|event| seen.insert(event.event_id.clone()));
        records.truncate(limit.saturating_add(1));

        if records.len() > limit {
            let oldest_selected = records.last().map(event_order_key);
            let next_segment_upper_bound = segments.get(index + 1).map(segment_order_upper_bound);
            if next_segment_upper_bound < oldest_selected {
                break;
            }
        }
    }
    Ok(records)
}

fn query_segments(
    segment_catalog: &RwLock<Vec<SegmentMeta>>,
    access: &QueryAccess,
    query: &ServerLogQuery,
    cursor: Option<&CursorPayload>,
    server_time_ms: i64,
) -> Result<Vec<SegmentMeta>, AppError> {
    Ok(segment_catalog
        .read()
        .map_err(|_| AppError::Internal("server log segment catalog poisoned".into()))?
        .iter()
        .filter(|segment| {
            segment_matches_query_bounds(segment, access, query, cursor, server_time_ms)
        })
        .cloned()
        .collect())
}

fn segment_matches_query_bounds(
    segment: &SegmentMeta,
    access: &QueryAccess,
    query: &ServerLogQuery,
    cursor: Option<&CursorPayload>,
    server_time_ms: i64,
) -> bool {
    if !access.public_only
        && !access.all_installations
        && !access.installation_ids.contains(&segment.installation_id)
    {
        return false;
    }
    if query
        .installation_id
        .as_deref()
        .is_some_and(|installation_id| installation_id != segment.installation_id)
    {
        return false;
    }
    if query
        .from_ms
        .is_some_and(|from| segment.max_received_at_ms < from)
        || query
            .to_ms
            .is_some_and(|to| segment.min_received_at_ms > to)
    {
        return false;
    }
    if access.public_only
        && (segment.max_occurred_at_ms < server_time_ms.saturating_sub(PUBLIC_WINDOW_MS)
            || segment.min_occurred_at_ms > server_time_ms.saturating_add(PUBLIC_CLOCK_SKEW_MS))
    {
        return false;
    }
    if let Some(cursor) = cursor {
        let cursor_key = cursor_order_key(cursor);
        if segment_order_lower_bound(segment) >= cursor_key {
            return false;
        }
    }
    true
}

fn event_matches_query(
    event: &StoredServerLogEvent,
    access: &QueryAccess,
    query: &ServerLogQuery,
    cursor: Option<&CursorPayload>,
    level: Option<&str>,
    search: Option<&str>,
    server_time_ms: i64,
) -> bool {
    if !access.public_only
        && !access.all_installations
        && !access.installation_ids.contains(&event.installation_id)
    {
        return false;
    }
    if let Some(requested) = query.installation_id.as_deref() {
        if event.installation_id != requested {
            return false;
        }
    }
    if query
        .client_alias
        .as_deref()
        .is_some_and(|alias| event.client_alias != alias)
    {
        return false;
    }
    if access.public_only && !is_public_window_event(event, server_time_ms) {
        return false;
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
        let searchable = if access.public_only {
            format!(
                "{} {}",
                public_message(&event.target),
                public_message(&event.message)
            )
        } else {
            format!(
                "{} {} {}",
                event.target,
                event.message,
                serde_json::to_string(&event.fields).unwrap_or_default()
            )
        };
        if !searchable.to_ascii_lowercase().contains(search) {
            return false;
        }
    }
    if let Some(cursor) = cursor {
        if event_order_key(event) >= cursor_order_key(cursor) {
            return false;
        }
    }
    true
}

fn compare_events_descending(
    left: &StoredServerLogEvent,
    right: &StoredServerLogEvent,
) -> std::cmp::Ordering {
    event_order_key(right).cmp(&event_order_key(left))
}

fn event_order_key(event: &StoredServerLogEvent) -> (u64, i64, &str) {
    (
        event.ingest_order,
        event.received_at_ms,
        event.event_id.as_str(),
    )
}

fn cursor_order_key(cursor: &CursorPayload) -> (u64, i64, &str) {
    (
        cursor.ingest_order,
        cursor.received_at_ms,
        cursor.event_id.as_str(),
    )
}

fn segment_order_lower_bound(segment: &SegmentMeta) -> (u64, i64, &str) {
    (segment.min_ingest_order, segment.min_received_at_ms, "")
}

fn segment_order_upper_bound(segment: &SegmentMeta) -> (u64, i64, &str) {
    (
        segment.max_ingest_order,
        segment.max_received_at_ms,
        "\u{10ffff}",
    )
}

fn compare_segments_descending(left: &SegmentMeta, right: &SegmentMeta) -> std::cmp::Ordering {
    segment_order_upper_bound(left).cmp(&segment_order_upper_bound(right))
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

impl SegmentMeta {
    fn path(&self, root: &Path) -> PathBuf {
        root.join(&self.path)
    }

    fn from_records(
        path: String,
        bytes: u64,
        modified_at_ms: i64,
        active: bool,
        records: &[StoredServerLogEvent],
    ) -> Result<Self, AppError> {
        let first = records
            .first()
            .ok_or_else(|| AppError::Internal(format!("server log segment is empty: {path}")))?;
        if records.iter().any(|record| {
            record.installation_id != first.installation_id || record.stream_id != first.stream_id
        }) {
            return Err(AppError::Internal(format!(
                "server log segment mixes installation streams: {path}"
            )));
        }
        Ok(Self {
            path,
            installation_id: first.installation_id.clone(),
            stream_id: first.stream_id.clone(),
            event_count: records.len() as u64,
            bytes,
            modified_at_ms,
            min_received_at_ms: records
                .iter()
                .map(|record| record.received_at_ms)
                .min()
                .unwrap_or_default(),
            max_received_at_ms: records
                .iter()
                .map(|record| record.received_at_ms)
                .max()
                .unwrap_or_default(),
            min_occurred_at_ms: records
                .iter()
                .map(|record| record.occurred_at_ms)
                .min()
                .unwrap_or_default(),
            max_occurred_at_ms: records
                .iter()
                .map(|record| record.occurred_at_ms)
                .max()
                .unwrap_or_default(),
            min_ingest_order: records
                .iter()
                .map(|record| record.ingest_order)
                .min()
                .unwrap_or_default(),
            max_ingest_order: records
                .iter()
                .map(|record| record.ingest_order)
                .max()
                .unwrap_or_default(),
            max_sequence: records
                .iter()
                .map(|record| record.sequence)
                .max()
                .unwrap_or_default(),
            active,
        })
    }

    fn extend(&mut self, records: &[StoredServerLogEvent], bytes: u64, modified_at_ms: i64) {
        if records.is_empty() {
            return;
        }
        self.event_count = self.event_count.saturating_add(records.len() as u64);
        self.bytes = bytes;
        self.modified_at_ms = modified_at_ms;
        self.min_received_at_ms = self.min_received_at_ms.min(
            records
                .iter()
                .map(|record| record.received_at_ms)
                .min()
                .unwrap_or(self.min_received_at_ms),
        );
        self.max_received_at_ms = self.max_received_at_ms.max(
            records
                .iter()
                .map(|record| record.received_at_ms)
                .max()
                .unwrap_or(self.max_received_at_ms),
        );
        self.min_occurred_at_ms = self.min_occurred_at_ms.min(
            records
                .iter()
                .map(|record| record.occurred_at_ms)
                .min()
                .unwrap_or(self.min_occurred_at_ms),
        );
        self.max_occurred_at_ms = self.max_occurred_at_ms.max(
            records
                .iter()
                .map(|record| record.occurred_at_ms)
                .max()
                .unwrap_or(self.max_occurred_at_ms),
        );
        self.min_ingest_order = self.min_ingest_order.min(
            records
                .iter()
                .map(|record| record.ingest_order)
                .min()
                .unwrap_or(self.min_ingest_order),
        );
        self.max_ingest_order = self.max_ingest_order.max(
            records
                .iter()
                .map(|record| record.ingest_order)
                .max()
                .unwrap_or(self.max_ingest_order),
        );
        self.max_sequence = self.max_sequence.max(
            records
                .iter()
                .map(|record| record.sequence)
                .max()
                .unwrap_or(self.max_sequence),
        );
    }
}

fn relative_event_path(root: &Path, path: &Path) -> Result<String, AppError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        AppError::Internal(format!(
            "server log segment is outside its storage root: {}",
            path.display()
        ))
    })?;
    if relative.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::RootDir
        )
    }) {
        return Err(AppError::Internal(
            "server log segment path is invalid".into(),
        ));
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn system_time_ms(value: SystemTime) -> i64 {
    value
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn load_segment_manifest(root: &Path) -> Result<Vec<SegmentMeta>, AppError> {
    let bytes = match fs::read(root.join(SEGMENT_MANIFEST_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(AppError::Internal(format!(
                "read server log segment manifest failed: {error}"
            )));
        }
    };
    let manifest = serde_json::from_slice::<SegmentManifest>(&bytes).map_err(|error| {
        AppError::Internal(format!(
            "decode server log segment manifest failed: {error}"
        ))
    })?;
    if manifest.version != SEGMENT_MANIFEST_VERSION {
        return Ok(Vec::new());
    }
    Ok(manifest.segments)
}

fn write_segment_manifest(root: &Path, segments: &[SegmentMeta]) -> Result<(), AppError> {
    let mut segments = segments.to_vec();
    segments.sort_by(|left, right| left.path.cmp(&right.path));
    write_json_atomic(
        &root.join(SEGMENT_MANIFEST_FILE),
        &SegmentManifest {
            version: SEGMENT_MANIFEST_VERSION,
            generated_at_ms: Utc::now().timestamp_millis(),
            segments,
        },
    )
}

fn load_stream_cursors(root: &Path, registry: &mut StreamRegistry) -> Result<(), AppError> {
    fn visit(directory: &Path, registry: &mut StreamRegistry) -> Result<(), AppError> {
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
                visit(&path, registry)?;
            } else if path.file_name().and_then(|value| value.to_str()) == Some("cursor.json") {
                if let Ok(bytes) = fs::read(&path) {
                    if let Ok(cursor) = serde_json::from_slice::<StreamCursorFile>(&bytes) {
                        registry.streams.insert(
                            (cursor.installation_id, cursor.stream_id),
                            Arc::new(Mutex::new(StreamState {
                                last_sequence: cursor.last_sequence,
                                active_day: cursor.active_day,
                                cursor_dirty: false,
                                catalog_dirty: false,
                            })),
                        );
                    }
                }
            }
        }
        Ok(())
    }
    visit(root, registry)
}

fn recover_catalog_from_event_files(
    root: &Path,
    registry: &mut StreamRegistry,
) -> Result<(Vec<SegmentMeta>, u64), AppError> {
    let cached = load_segment_manifest(root)
        .unwrap_or_default()
        .into_iter()
        .map(|segment| (segment.path.clone(), segment))
        .collect::<HashMap<_, _>>();
    let mut segments = Vec::new();
    let mut max_ingest_order = 0_u64;
    for path in event_files(root)? {
        let is_active = path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("active-"));
        if is_active {
            repair_trailing_partial_line(&path)?;
        }
        let metadata = match path.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(AppError::Internal(format!(
                    "inspect server log segment failed: {error}"
                )));
            }
        };
        if metadata.len() == 0 {
            continue;
        }
        let relative_path = relative_event_path(root, &path)?;
        let modified_at_ms = system_time_ms(metadata.modified().unwrap_or(SystemTime::now()));
        let segment = if let Some(cached) = cached.get(&relative_path).filter(|cached| {
            cached.bytes == metadata.len() && cached.modified_at_ms == modified_at_ms
        }) {
            cached.clone()
        } else {
            let mut records = Vec::new();
            read_records_file(&path, &mut records)?;
            SegmentMeta::from_records(
                relative_path,
                metadata.len(),
                modified_at_ms,
                is_active,
                &records,
            )?
        };
        max_ingest_order = max_ingest_order.max(segment.max_ingest_order);
        let key = (segment.installation_id.clone(), segment.stream_id.clone());
        let slot = registry.streams.entry(key).or_insert_with(|| {
            Arc::new(Mutex::new(StreamState {
                last_sequence: 0,
                active_day: utc_day(segment.max_received_at_ms),
                cursor_dirty: false,
                catalog_dirty: false,
            }))
        });
        let mut state = slot
            .lock()
            .map_err(|_| AppError::Internal("server log stream lock poisoned".into()))?;
        if segment.max_sequence >= state.last_sequence {
            state.last_sequence = segment.max_sequence;
            state.active_day = utc_day(segment.max_received_at_ms);
        }
        segments.push(segment);
    }
    Ok((segments, max_ingest_order))
}

#[cfg(test)]
fn read_last_active_record(path: &Path) -> Result<Option<StoredServerLogEvent>, AppError> {
    const RECOVERY_TAIL_BYTES: u64 = 128 * 1024;

    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AppError::Internal(format!(
                "open active server log tail failed: {error}"
            )));
        }
    };
    let length = file
        .metadata()
        .map_err(|error| AppError::Internal(format!("inspect active log tail failed: {error}")))?
        .len();
    if length == 0 {
        return Ok(None);
    }
    let start = length.saturating_sub(RECOVERY_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|error| AppError::Internal(format!("seek active log tail failed: {error}")))?;
    let mut tail = Vec::with_capacity(usize::try_from(length - start).unwrap_or_default());
    file.read_to_end(&mut tail)
        .map_err(|error| AppError::Internal(format!("read active log tail failed: {error}")))?;
    if start > 0 {
        if let Some(first_newline) = tail.iter().position(|byte| *byte == b'\n') {
            tail.drain(..=first_newline);
        } else {
            return Ok(None);
        }
    }
    Ok(tail
        .split(|byte| *byte == b'\n')
        .rev()
        .filter(|line| !line.is_empty())
        .find_map(|line| serde_json::from_slice::<StoredServerLogEvent>(line).ok()))
}

struct ServerLogFileLease<'a> {
    store: &'a ServerLogStore,
    paths: Vec<PathBuf>,
}

impl Drop for ServerLogFileLease<'_> {
    fn drop(&mut self) {
        while let Some(path) = self.paths.pop() {
            self.release(&path);
        }
    }
}

impl ServerLogFileLease<'_> {
    fn release_path(&mut self, path: &Path) {
        let Some(index) = self.paths.iter().position(|candidate| candidate == path) else {
            return;
        };
        let path = self.paths.swap_remove(index);
        self.release(&path);
    }

    fn release(&self, path: &Path) {
        if let Ok(mut leases) = self.store.file_leases.lock() {
            if leases.get(path).copied().unwrap_or_default() > 1 {
                if let Some(count) = leases.get_mut(path) {
                    *count -= 1;
                }
            } else {
                leases.remove(path);
            }
        }
        self.store.request_maintenance();
    }
}

fn cleanup_files(
    config: &ServerLogRuntimeConfig,
    leased_files: &HashSet<PathBuf>,
) -> Result<u64, AppError> {
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
        if !leased_files.contains(path)
            && now
                .duration_since(*modified)
                .is_ok_and(|age| age > retention)
        {
            let _ = fs::remove_file(path);
        }
    }
    files.retain(|(path, _, _)| path.exists());
    let cached = load_segment_manifest(&config.root)
        .unwrap_or_default()
        .into_iter()
        .map(|segment| (segment.path.clone(), segment))
        .collect::<HashMap<_, _>>();
    let mut files = files
        .into_iter()
        .map(|(path, bytes, modified)| {
            let relative = relative_event_path(&config.root, &path)?;
            let modified_at_ms = system_time_ms(modified);
            let max_ingest_order = if let Some(segment) = cached.get(&relative).filter(|segment| {
                segment.bytes == bytes && segment.modified_at_ms == modified_at_ms
            }) {
                segment.max_ingest_order
            } else {
                let mut records = Vec::new();
                read_records_file(&path, &mut records)?;
                records
                    .iter()
                    .map(|record| record.ingest_order)
                    .max()
                    .unwrap_or_default()
            };
            Ok((path, bytes, modified, max_ingest_order))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    files.sort_by(|left, right| {
        left.3
            .cmp(&right.3)
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.0.cmp(&right.0))
    });
    let protected = files.last().map(|(path, _, _, _)| path.clone());
    let mut total = files.iter().map(|(_, bytes, _, _)| *bytes).sum::<u64>();
    for (path, bytes, _, _) in files {
        if total <= config.max_total_bytes {
            break;
        }
        if protected.as_ref() == Some(&path) || leased_files.contains(&path) {
            continue;
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
    let bytes = serde_json::to_vec(value).map_err(|error| {
        AppError::Internal(format!("encode server log metadata failed: {error}"))
    })?;
    {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|error| {
            AppError::Internal(format!(
                "create temporary server log metadata failed: {error}"
            ))
        })?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                AppError::Internal(format!("write server log metadata failed: {error}"))
            })?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        AppError::Internal(format!("replace server log metadata failed: {error}"))
    })?;
    sync_parent_directory(path).map_err(|error| {
        AppError::Internal(format!(
            "sync server log metadata directory failed: {error}"
        ))
    })?;
    Ok(())
}

fn sync_parent_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
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
            sync_parent_directory(&path).map_err(|error| {
                AppError::Internal(format!("sync log cursor key directory failed: {error}"))
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

fn query_cursor_fingerprint(
    access: &QueryAccess,
    query: &ServerLogQuery,
) -> Result<String, AppError> {
    let mut installation_ids = access
        .installation_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    installation_ids.sort_unstable();
    let identity = CursorQueryIdentity {
        public_only: access.public_only,
        all_installations: access.all_installations,
        installation_ids,
        installation_id: query.installation_id.as_deref(),
        client_alias: query.client_alias.as_deref(),
        level: normalized_query_text(query.level.as_deref()),
        search: normalized_query_text(query.search.as_deref()),
        from_ms: query.from_ms,
        to_ms: query.to_ms,
    };
    let bytes = serde_json::to_vec(&identity)
        .map_err(|error| AppError::Internal(format!("encode server log query: {error}")))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn normalized_query_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
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
    format!("client-{}", hex::encode(&digest[..10]))
}

fn utc_day(timestamp_ms: i64) -> String {
    Utc.timestamp_millis_opt(timestamp_ms)
        .single()
        .unwrap_or_else(Utc::now)
        .format("%Y-%m-%d")
        .to_string()
}

fn is_public_window_event(event: &StoredServerLogEvent, now_ms: i64) -> bool {
    event.occurred_at_ms >= now_ms.saturating_sub(PUBLIC_WINDOW_MS)
        && event.occurred_at_ms <= now_ms.saturating_add(PUBLIC_CLOCK_SKEW_MS)
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

fn format_text_event(event: &ServerLogEventView) -> String {
    let timestamp = Utc
        .timestamp_millis_opt(event.occurred_at_ms)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut parts = vec![
        timestamp,
        event.level.to_ascii_uppercase(),
        event.message.clone(),
    ];
    if let Some(fields) = event.fields.as_ref() {
        for (key, value) in fields {
            parts.push(format!("{key}={}", compact_json_value(value)));
        }
    }
    parts.join(" ")
}

fn compact_json_value(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
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
    env_bool("CC_SWITCH_ROUTER_SERVER_LOG_PUBLIC_ENABLED", true)
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

    fn payload(sequence: u64) -> InstallationAuditBatchPayload {
        InstallationAuditBatchPayload {
            protocol_version: 1,
            boot_id: "stream-a".into(),
            server_version: "0.1.0".into(),
            commit_id: "abc".into(),
            events: vec![InstallationAuditEvent {
                schema_version: 1,
                sequence,
                timestamp_ms: Utc::now().timestamp_millis(),
                boot_id: "stream-a".into(),
                level: "info".into(),
                event: "inference.request.accepted".into(),
                app: Some("codex".into()),
                operation: Some("responses".into()),
                ..InstallationAuditEvent::default()
            }],
        }
    }

    fn payload_range(start: u64, end: u64) -> InstallationAuditBatchPayload {
        let mut batch = payload(start);
        let template = batch.events[0].clone();
        batch.events = (start..=end)
            .map(|sequence| InstallationAuditEvent {
                sequence,
                ..template.clone()
            })
            .collect();
        batch
    }

    fn payload_for_stream(stream_id: &str, sequence: u64) -> InstallationAuditBatchPayload {
        let mut batch = payload(sequence);
        batch.boot_id = stream_id.to_string();
        for event in &mut batch.events {
            event.boot_id = stream_id.to_string();
        }
        batch
    }

    #[test]
    fn audit_batch_payload_matches_cross_repository_schema_fixture() {
        use sha2::Digest;

        let payload = InstallationAuditBatchPayload {
            protocol_version: 1,
            boot_id: "boot-fixture".to_string(),
            server_version: "1.2.3".to_string(),
            commit_id: "abcdef0".to_string(),
            events: vec![InstallationAuditEvent {
                schema_version: 1,
                sequence: 7,
                timestamp_ms: 1_700_000_000_123,
                boot_id: "boot-fixture".to_string(),
                level: "info".to_string(),
                event: "inference.provider.failover".to_string(),
                request_id: Some("request-fixture".to_string()),
                transport_request_id: Some("transport-fixture".to_string()),
                parent_request_id: Some("parent-fixture".to_string()),
                connection_id: Some("connection-fixture".to_string()),
                turn_id: Some("turn-fixture".to_string()),
                app: Some("codex".to_string()),
                surface: Some("openai".to_string()),
                operation: Some("responses_websocket_turn".to_string()),
                route: Some("/v1/responses".to_string()),
                method: Some("WS".to_string()),
                body_sha256: Some(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
                ),
                provider_type: Some("codex_oauth".to_string()),
                provider_ref: Some("provider_0123456789abcdef".to_string()),
                previous_provider_ref: Some("provider_fedcba9876543210".to_string()),
                account_ref: Some("account_0123456789abcdef".to_string()),
                requested_model: Some("gpt-requested".to_string()),
                actual_model: Some("gpt-actual".to_string()),
                status_code: Some(502),
                upstream_status: Some(503),
                outcome: Some("failed_over".to_string()),
                stage: Some("provider_selection".to_string()),
                error_code: Some("upstream_unavailable".to_string()),
                error_class: Some("upstream_error".to_string()),
                component: Some("provider_selection".to_string()),
                failure_kind: Some("upstream_unavailable".to_string()),
                network_error_kind: Some("timeout".to_string()),
                error_fingerprint: Some("error_0123456789abcdef".to_string()),
                retry_decision: Some("failover".to_string()),
                backoff_ms: Some(250),
                retryable: Some(true),
                duration_ms: Some(1_234),
                first_token_ms: Some(321),
                streaming: Some(true),
                stream_status: Some("interrupted".to_string()),
                attempt: Some(2),
                retry_count: Some(1),
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: Some(30),
            }],
        };

        let encoded = serde_json::to_vec(&payload).unwrap();
        assert_eq!(
            hex::encode(sha2::Sha256::digest(encoded)),
            "3fb028aa02ae49462857380e25a2c3aedd9ecebc2d56c857193bc58955300640"
        );
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

        let response = store
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
        assert_eq!(response.events.len(), 1);
        assert!(response.events[0].installation_id.is_none());
        assert!(response.events[0].fields.is_none());
        assert_eq!(response.events[0].message, "inference.request.accepted");
        assert_eq!(response.events[0].target, "cc_switch_server::audit");

        let side_channel = store
            .query(
                QueryAccess {
                    public_only: true,
                    all_installations: false,
                    installation_ids: HashSet::new(),
                },
                ServerLogQuery {
                    search: Some("request-secret".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("query redacted public logs");
        assert!(side_channel.events.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn public_installation_scan_uses_a_short_lived_cache() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload(1))
            .await
            .expect("ingest public event");

        let first = store
            .public_recent_installation_ids()
            .await
            .expect("scan public installations");
        assert!(first.contains("installation-a"));

        let active_path = stream_directory(&root, "installation-a", "stream-a")
            .join(format!("active-{}.jsonl", Utc::now().format("%Y-%m-%d")));
        fs::remove_file(active_path).expect("remove source after cache fill");
        let cached = store
            .public_recent_installation_ids()
            .await
            .expect("read cached public installations");
        assert!(cached.contains("installation-a"));

        store
            .public_installation_cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .computed_at = Instant::now()
            .checked_sub(PUBLIC_INSTALLATION_CACHE_TTL + Duration::from_millis(1))
            .unwrap();
        let refreshed = store
            .public_recent_installation_ids()
            .await
            .expect("refresh public installations");
        assert!(refreshed.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn duplicate_retry_repairs_cursor_and_segment_catalog() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload(1))
            .await
            .expect("first ingest");
        let directory = stream_directory(&root, "installation-a", "stream-a");
        fs::remove_file(directory.join("cursor.json")).unwrap();
        store.segments.write().unwrap().clear();
        {
            let streams = store.streams.lock().unwrap();
            let mut stream = streams
                .streams
                .get(&("installation-a".to_string(), "stream-a".to_string()))
                .unwrap()
                .lock()
                .unwrap();
            stream.cursor_dirty = true;
            stream.catalog_dirty = true;
        }

        let duplicate = store
            .ingest("installation-a".into(), payload(1))
            .await
            .expect("repair duplicate ingest");
        assert_eq!(duplicate.accepted, 0);
        assert!(directory.join("cursor.json").is_file());
        assert_eq!(store.segments.read().unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn public_window_excludes_old_events_uploaded_from_a_backlog() {
        let (store, root) = test_store();
        let mut stale = payload(1);
        stale.events[0].timestamp_ms = Utc::now().timestamp_millis() - PUBLIC_WINDOW_MS - 1;
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
    async fn export_streams_every_event_across_query_pages() {
        let (server_logs, root) = test_store();
        for (start, end) in [(1, MAX_BATCH_EVENTS as u64), (257, 501)] {
            server_logs
                .ingest("installation-a".into(), payload_range(start, end))
                .await
                .expect("ingest export fixture");
        }

        let response = build_server_log_export_response(
            Arc::clone(&server_logs),
            crate::store::AppStore::new_in_memory_for_tests().expect("metadata store"),
            QueryAccess {
                public_only: false,
                all_installations: false,
                installation_ids: HashSet::from(["installation-a".to_string()]),
            },
            ServerLogQuery::default(),
        )
        .await
        .expect("build export response");
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .expect("collect export body");
        let events = body
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).expect("parse exported event"))
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 501);
        let sequences = events
            .iter()
            .map(|event| {
                event["sequence"]
                    .as_u64()
                    .expect("private export includes sequence")
            })
            .collect::<HashSet<_>>();
        assert_eq!(sequences.len(), 501);
        assert!((1..=501).all(|sequence| sequences.contains(&sequence)));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn export_page_boundary_does_not_skip_or_repeat_installations() {
        let (server_logs, root) = test_store();
        server_logs
            .ingest("installation-a".into(), payload_range(1, 251))
            .await
            .expect("ingest first installation");
        server_logs
            .ingest("installation-b".into(), payload_range(1, 250))
            .await
            .expect("ingest second installation");

        let response = build_server_log_export_response(
            Arc::clone(&server_logs),
            crate::store::AppStore::new_in_memory_for_tests().expect("metadata store"),
            QueryAccess {
                public_only: false,
                all_installations: true,
                installation_ids: HashSet::new(),
            },
            ServerLogQuery::default(),
        )
        .await
        .expect("build cross-installation export response");
        let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .expect("collect cross-installation export body");
        let events = body
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).expect("parse exported event"))
            .collect::<Vec<_>>();

        assert_eq!(events.len(), 501);
        assert_eq!(
            events
                .iter()
                .filter_map(|event| event["eventId"].as_str())
                .collect::<HashSet<_>>()
                .len(),
            501
        );
        assert_eq!(
            events
                .iter()
                .filter_map(|event| event["installationId"].as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["installation-a", "installation-b"])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ingest_accepts_a_first_sequence_gap_after_spool_loss() {
        let (store, root) = test_store();
        let response = store
            .ingest("installation-a".into(), payload(2))
            .await
            .expect("first sequence after spool loss");
        assert_eq!(response.accepted, 1);
        assert!(response.gap_detected);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ingest_rejects_non_contiguous_events_inside_a_batch() {
        let (store, root) = test_store();
        let mut invalid = payload(1);
        let mut third = invalid.events[0].clone();
        third.sequence = 3;
        invalid.events.push(third);
        assert!(matches!(
            store.ingest("installation-a".into(), invalid).await,
            Err(AppError::BadRequest(_))
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cursor_is_signed() {
        let key = [7u8; 32];
        let encoded = encode_cursor(
            &CursorPayload {
                ingest_order: 10,
                received_at_ms: 10,
                event_id: "event".into(),
                query_fingerprint: "query-a".into(),
            },
            &key,
        )
        .expect("encode cursor");
        let decoded = decode_cursor(&encoded, &key).unwrap();
        assert_eq!(decoded.event_id, "event");
        assert_eq!(decoded.query_fingerprint, "query-a");
        assert!(decode_cursor(&(encoded + "x"), &key).is_err());
    }

    #[tokio::test]
    async fn cursor_cannot_be_reused_for_another_installation() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload_range(1, 2))
            .await
            .unwrap();
        store
            .ingest("installation-b".into(), payload(1))
            .await
            .unwrap();
        let access = QueryAccess {
            public_only: false,
            all_installations: false,
            installation_ids: HashSet::from([
                "installation-a".to_string(),
                "installation-b".to_string(),
            ]),
        };
        let first = store
            .query(
                access.clone(),
                ServerLogQuery {
                    installation_id: Some("installation-a".into()),
                    limit: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let cursor = first.next_cursor.expect("first installation cursor");

        assert!(matches!(
            store
                .query(
                    access,
                    ServerLogQuery {
                        installation_id: Some("installation-b".into()),
                        cursor: Some(cursor),
                        limit: Some(1),
                        ..Default::default()
                    },
                )
                .await,
            Err(AppError::BadRequest(_))
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cursor_pagination_preserves_batch_sequence_when_timestamps_tie() {
        let (store, root) = test_store();
        store
            .ingest(
                "installation-a".into(),
                payload_range(1, MAX_BATCH_EVENTS as u64),
            )
            .await
            .expect("ingest tied timestamp batch");
        let access = QueryAccess {
            public_only: false,
            all_installations: false,
            installation_ids: HashSet::from(["installation-a".to_string()]),
        };
        let mut cursor = None;
        let mut sequences = Vec::new();
        loop {
            let page = store
                .query(
                    access.clone(),
                    ServerLogQuery {
                        cursor: cursor.clone(),
                        limit: Some(37),
                        ..ServerLogQuery::default()
                    },
                )
                .await
                .expect("query stable cursor page");
            sequences.extend(page.events.iter().filter_map(|event| event.sequence));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(
            sequences,
            (1..=MAX_BATCH_EVENTS as u64).rev().collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn independent_streams_use_distinct_serialization_slots() {
        let (store, root) = test_store();
        let (first, second) = tokio::join!(
            store.ingest("installation-a".into(), payload_for_stream("stream-a", 1)),
            store.ingest("installation-b".into(), payload_for_stream("stream-b", 1))
        );
        assert_eq!(first.expect("first stream ingest").accepted, 1);
        assert_eq!(second.expect("second stream ingest").accepted, 1);
        let streams = store.streams.lock().expect("stream registry");
        let first = streams
            .streams
            .get(&("installation-a".to_string(), "stream-a".to_string()))
            .expect("first stream slot");
        let second = streams
            .streams
            .get(&("installation-b".to_string(), "stream-b".to_string()))
            .expect("second stream slot");
        assert!(!Arc::ptr_eq(first, second));
        drop(streams);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn segment_manifest_rebuilds_ingest_order_after_restart() {
        let (store, root) = test_store();
        store
            .ingest("installation-a".into(), payload(1))
            .await
            .expect("ingest first event");
        store
            .run_maintenance_sync()
            .expect("checkpoint segment manifest");
        let manifest = serde_json::from_slice::<SegmentManifest>(
            &fs::read(root.join(SEGMENT_MANIFEST_FILE)).expect("read segment manifest"),
        )
        .expect("decode segment manifest");
        assert_eq!(manifest.version, SEGMENT_MANIFEST_VERSION);
        assert_eq!(manifest.segments.len(), 1);
        assert_eq!(manifest.segments[0].event_count, 1);
        drop(store);

        let restarted = Arc::new(
            ServerLogStore::open(ServerLogRuntimeConfig {
                enabled: true,
                root: root.clone(),
                retention_days: 7,
                max_total_bytes: 16 * 1024 * 1024,
            })
            .expect("restart log store"),
        );
        restarted
            .ingest("installation-a".into(), payload(2))
            .await
            .expect("ingest after restart");
        let response = restarted
            .query(
                QueryAccess {
                    public_only: false,
                    all_installations: false,
                    installation_ids: HashSet::from(["installation-a".to_string()]),
                },
                ServerLogQuery::default(),
            )
            .await
            .expect("query restarted log store");
        assert_eq!(
            response
                .events
                .iter()
                .filter_map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn historical_active_recovery_reads_only_the_last_complete_record() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-server-log-tail-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("active-2026-08-09.jsonl");
        let mut first = stored_event("installation-a", 1);
        first.message = "first".into();
        let mut second = stored_event("installation-a", 2);
        second.message = "second".into();
        append_records(&path, &[first, second.clone()]).unwrap();

        let recovered = read_last_active_record(&path).unwrap().unwrap();
        assert_eq!(recovered.sequence, 2);
        assert_eq!(recovered.message, "second");
        let _ = fs::remove_dir_all(root);
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
    fn batch_validation_rejects_raw_identity_references() {
        let mut invalid = payload(1);
        invalid.events[0].provider_ref = Some("provider-secret-name".into());
        assert!(matches!(
            validate_batch(&invalid),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn batch_validation_rejects_raw_error_fingerprints() {
        let mut invalid = payload(1);
        invalid.events[0].error_fingerprint = Some("connection refused at secret host".into());
        assert!(matches!(
            validate_batch(&invalid),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn batch_validation_rejects_sensitive_string_values() {
        let mut email = payload(1);
        email.events[0].requested_model = Some("owner@example.com".into());
        assert!(matches!(
            validate_batch(&email),
            Err(AppError::BadRequest(_))
        ));

        let mut credential = payload(1);
        credential.events[0].actual_model = Some("sk-secret-looking-value".into());
        assert!(matches!(
            validate_batch(&credential),
            Err(AppError::BadRequest(_))
        ));
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
        cleanup_files(&cleanup_config, &HashSet::new()).expect("clean expired logs");

        assert!(!directory.exists());
        assert!(!stream_has_persistent_state(
            &root,
            "installation-a",
            "stream-a"
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ingest_capacity_keeps_the_newest_acknowledged_file() {
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
        assert_eq!(event_files(&root).unwrap().len(), 1);
        assert!(store.stored_event_bytes.load(Ordering::Acquire) > 0);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn slow_export_leases_files_without_holding_the_global_file_lock() {
        let (store, root) = test_store();
        store
            .ingest("installation-b".into(), payload(1))
            .await
            .expect("ingest older export event");
        store
            .ingest("installation-a".into(), payload_range(1, 64))
            .await
            .expect("ingest export events");
        let receiver = store
            .export_stream(
                QueryAccess {
                    public_only: false,
                    all_installations: true,
                    installation_ids: HashSet::new(),
                },
                ServerLogQuery::default(),
                HashMap::new(),
            )
            .await
            .expect("start export");

        let deadline = Instant::now() + Duration::from_secs(2);
        while store.file_leases.lock().unwrap().is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let leased_paths = store
            .file_leases
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        assert_eq!(leased_paths.len(), 1);
        assert!(
            leased_paths
                .iter()
                .any(|path| path.starts_with(stream_directory(
                    &root,
                    "installation-b",
                    "stream-a"
                )))
        );
        assert!(store.files_guard.try_write().is_ok());

        drop(receiver);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !store.file_leases.lock().unwrap().is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(store.file_leases.lock().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capacity_cleanup_uses_ingest_order_instead_of_file_mtime() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-server-log-capacity-order-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let newest_path = root.join("active-2026-08-09.jsonl");
        let older_path = root.join("active-2026-08-10.jsonl");
        let mut newest = stored_event("installation-a", 2);
        newest.ingest_order = 2;
        append_records(&newest_path, &[newest]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let mut older = stored_event("installation-a", 1);
        older.ingest_order = 1;
        append_records(&older_path, &[older]).unwrap();

        cleanup_files(
            &ServerLogRuntimeConfig {
                enabled: true,
                root: root.clone(),
                retention_days: 7,
                max_total_bytes: 1,
            },
            &HashSet::new(),
        )
        .expect("clean capacity-limited logs");

        assert!(newest_path.is_file());
        assert!(!older_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capacity_cleanup_removes_oldest_logical_segment_before_newer_segments() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-server-log-capacity-full-order-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let middle_path = root.join("active-2026-08-08.jsonl");
        let newest_path = root.join("active-2026-08-09.jsonl");
        let oldest_path = root.join("active-2026-08-10.jsonl");

        let mut middle = stored_event("installation-a", 2);
        middle.ingest_order = 2;
        append_records(&middle_path, &[middle]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let mut newest = stored_event("installation-a", 3);
        newest.ingest_order = 3;
        append_records(&newest_path, &[newest]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let mut oldest = stored_event("installation-a", 1);
        oldest.ingest_order = 1;
        append_records(&oldest_path, &[oldest]).unwrap();

        let total = [&oldest_path, &middle_path, &newest_path]
            .into_iter()
            .map(|path| path.metadata().unwrap().len())
            .sum::<u64>();
        let oldest_bytes = oldest_path.metadata().unwrap().len();
        cleanup_files(
            &ServerLogRuntimeConfig {
                enabled: true,
                root: root.clone(),
                retention_days: 7,
                max_total_bytes: total.saturating_sub(oldest_bytes),
            },
            &HashSet::new(),
        )
        .expect("clean capacity-limited logs in logical order");

        assert!(!oldest_path.exists());
        assert!(middle_path.is_file());
        assert!(newest_path.is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capacity_cleanup_preserves_leased_export_files() {
        let root = std::env::temp_dir().join(format!(
            "cc-switch-router-server-log-capacity-lease-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let oldest_path = root.join("active-2026-08-08.jsonl");
        let middle_path = root.join("active-2026-08-09.jsonl");
        let newest_path = root.join("active-2026-08-10.jsonl");
        for (path, ingest_order) in [
            (&oldest_path, 1_u64),
            (&middle_path, 2_u64),
            (&newest_path, 3_u64),
        ] {
            let mut event = stored_event("installation-a", ingest_order);
            event.ingest_order = ingest_order;
            append_records(path, &[event]).unwrap();
        }
        let config = ServerLogRuntimeConfig {
            enabled: true,
            root: root.clone(),
            retention_days: 7,
            max_total_bytes: newest_path.metadata().unwrap().len(),
        };

        cleanup_files(&config, &HashSet::from([oldest_path.clone()]))
            .expect("clean logs while export lease is active");
        assert!(oldest_path.is_file());
        assert!(!middle_path.exists());
        assert!(newest_path.is_file());

        cleanup_files(&config, &HashSet::new()).expect("clean logs after export lease release");
        assert!(!oldest_path.exists());
        assert!(newest_path.is_file());
        let _ = fs::remove_dir_all(root);
    }

    fn stored_event(installation_id: &str, sequence: u64) -> StoredServerLogEvent {
        StoredServerLogEvent {
            event_id: event_id(installation_id, "stream-a", sequence),
            installation_id: installation_id.into(),
            client_alias: "client-test".into(),
            stream_id: "stream-a".into(),
            sequence,
            ingest_order: sequence,
            occurred_at_ms: Utc::now().timestamp_millis(),
            received_at_ms: Utc::now().timestamp_millis(),
            level: "info".into(),
            target: "cc_switch_server::audit".into(),
            message: "inference.request.completed".into(),
            fields: BTreeMap::new(),
            file: None,
            line: None,
            server_version: "0.1.0".into(),
            commit_id: "abc".into(),
        }
    }
}
