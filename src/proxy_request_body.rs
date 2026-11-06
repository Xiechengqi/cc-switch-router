//! Bounded ingress buffering. Bytes own their memory lease, including clones
//! retained by a retry or by reqwest after the handler's local variables drop.
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, Version, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http_body::Body as _;
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::{OnceCell, Semaphore};
use tokio::time::Instant;
use tracing::warn;

use crate::config::ProxyStreamConfig;

const MIB: usize = 1024 * 1024;
const DECODE_SCRATCH: usize = 16 * MIB;
const DECODE_MAX_DURATION: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    FirstByteTimeout,
    IdleTimeout,
    HardTimeout,
    TooLarge,
    ReadError,
    InvalidLength,
    MemoryBudget,
    UnsupportedEncoding,
    InvalidEncoding,
    DecodedTooLarge,
    DecodeBusy,
    DecodeTimeout,
    InvalidJson,
}

impl FailureKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::FirstByteTimeout => "first_byte_timeout",
            Self::IdleTimeout => "idle_timeout",
            Self::HardTimeout => "hard_timeout",
            Self::TooLarge => "wire_too_large",
            Self::ReadError => "read_error",
            Self::InvalidLength => "invalid_content_length",
            Self::MemoryBudget => "memory_budget_exhausted",
            Self::UnsupportedEncoding => "unsupported_content_encoding",
            Self::InvalidEncoding => "invalid_content_encoding",
            Self::DecodedTooLarge => "decoded_too_large",
            Self::DecodeBusy => "decode_capacity_exhausted",
            Self::DecodeTimeout => "decode_timeout",
            Self::InvalidJson => "invalid_json",
        }
    }

    pub fn is_timeout(self) -> bool {
        matches!(
            self,
            Self::FirstByteTimeout | Self::IdleTimeout | Self::HardTimeout
        )
    }

    pub fn status(self) -> StatusCode {
        match self {
            Self::FirstByteTimeout | Self::IdleTimeout | Self::HardTimeout => {
                StatusCode::REQUEST_TIMEOUT
            }
            Self::TooLarge | Self::DecodedTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::MemoryBudget | Self::DecodeBusy | Self::DecodeTimeout => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::UnsupportedEncoding => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    fn code(self, unified: bool) -> String {
        let suffix = if self.is_timeout() {
            "timeout"
        } else if matches!(self, Self::TooLarge | Self::DecodedTooLarge) {
            "too_large"
        } else {
            self.name()
        };
        format!(
            "{}request_body_{suffix}",
            if unified {
                "user_model_route_"
            } else {
                "proxy_"
            }
        )
    }

    fn message(self) -> &'static str {
        match self {
            Self::FirstByteTimeout | Self::IdleTimeout | Self::HardTimeout => {
                "request body timed out"
            }
            Self::TooLarge | Self::DecodedTooLarge => "request body is too large",
            Self::MemoryBudget | Self::DecodeBusy => {
                "request body capacity is temporarily exhausted"
            }
            Self::DecodeTimeout => "request body decoding exceeded its time budget",
            Self::UnsupportedEncoding => "unsupported request Content-Encoding",
            Self::InvalidEncoding => "invalid compressed request body",
            Self::InvalidJson => "invalid request JSON object",
            _ => "incomplete or invalid request body",
        }
    }
}

#[derive(Debug)]
pub struct BodyFailure {
    pub kind: FailureKind,
    pub received: usize,
    pub declared: Option<u64>,
    pub inspected_bytes: Option<usize>,
    pub elapsed_ms: u64,
    pub last_progress_age_ms: u64,
}

#[derive(Clone)]
pub(crate) struct LocalIngressFailure;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyMetrics {
    pub memory_limit_bytes: usize,
    pub memory_used_bytes: usize,
    pub memory_peak_bytes: usize,
    pub completed_total: u64,
    pub first_byte_timeout_total: u64,
    pub idle_timeout_total: u64,
    pub hard_timeout_total: u64,
    pub too_large_total: u64,
    pub read_error_total: u64,
    pub capacity_rejected_total: u64,
    pub decode_rejected_total: u64,
    pub cancelled_total: u64,
}

#[derive(Debug)]
pub struct BodyBudget {
    used: AtomicUsize,
    peak: AtomicUsize,
    limit: AtomicUsize,
    completed: AtomicU64,
    failures: [AtomicU64; 8],
    decode_slots: Arc<Semaphore>,
}

impl Default for BodyBudget {
    fn default() -> Self {
        Self {
            used: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            limit: AtomicUsize::new(
                crate::config::DEFAULT_PROXY_REQUEST_MEMORY_BUDGET_MB as usize * MIB,
            ),
            completed: AtomicU64::new(0),
            failures: Default::default(),
            decode_slots: Arc::new(Semaphore::new(4)),
        }
    }
}

impl BodyBudget {
    pub fn configure(&self, limit: usize) {
        self.limit.store(limit, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> BodyMetrics {
        let n = |i: usize| self.failures[i].load(Ordering::Relaxed);
        BodyMetrics {
            memory_limit_bytes: self.limit.load(Ordering::Relaxed),
            memory_used_bytes: self.used.load(Ordering::Relaxed),
            memory_peak_bytes: self.peak.load(Ordering::Relaxed),
            completed_total: self.completed.load(Ordering::Relaxed),
            first_byte_timeout_total: n(0),
            idle_timeout_total: n(1),
            hard_timeout_total: n(2),
            too_large_total: n(3),
            read_error_total: n(4),
            capacity_rejected_total: n(5),
            decode_rejected_total: n(6),
            cancelled_total: n(7),
        }
    }

    fn record(&self, kind: FailureKind) {
        let index = match kind {
            FailureKind::FirstByteTimeout => 0,
            FailureKind::IdleTimeout => 1,
            FailureKind::HardTimeout => 2,
            FailureKind::TooLarge | FailureKind::DecodedTooLarge => 3,
            FailureKind::ReadError | FailureKind::InvalidLength => 4,
            FailureKind::MemoryBudget | FailureKind::DecodeBusy => 5,
            _ => 6,
        };
        self.failures[index].fetch_add(1, Ordering::Relaxed);
    }

    fn reserve(self: &Arc<Self>, amount: usize, limit: usize) -> Result<Lease, FailureKind> {
        self.limit.store(limit, Ordering::Relaxed);
        let previous = self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |used| {
                used.checked_add(amount).filter(|next| *next <= limit)
            })
            .map_err(|_| FailureKind::MemoryBudget)?;
        self.peak.fetch_max(previous + amount, Ordering::Relaxed);
        Ok(Lease {
            budget: self.clone(),
            amount,
        })
    }
}

#[derive(Debug)]
struct Lease {
    budget: Arc<BodyBudget>,
    amount: usize,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.amount, Ordering::AcqRel);
    }
}

struct Buffer {
    data: Vec<u8>,
    _lease: Lease,
}
impl AsRef<[u8]> for Buffer {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}
impl Buffer {
    fn new(
        budget: &Arc<BodyBudget>,
        capacity: usize,
        memory_limit: usize,
    ) -> Result<Self, FailureKind> {
        let lease = budget.reserve(capacity, memory_limit)?;
        let mut data = Vec::new();
        data.try_reserve_exact(capacity)
            .map_err(|_| FailureKind::MemoryBudget)?;
        // Vec may round a reservation up; charge any extra capacity too.
        let extra = data.capacity().saturating_sub(capacity);
        let mut lease = lease;
        if extra > 0 {
            let mut extra_lease = budget.reserve(extra, memory_limit)?;
            lease.amount += extra_lease.amount;
            extra_lease.amount = 0;
        }
        Ok(Self {
            data,
            _lease: lease,
        })
    }

    fn append(
        &mut self,
        bytes: &[u8],
        maximum: usize,
        memory_limit: usize,
    ) -> Result<(), FailureKind> {
        let length = self
            .data
            .len()
            .checked_add(bytes.len())
            .ok_or(FailureKind::TooLarge)?;
        if length > maximum {
            return Err(FailureKind::TooLarge);
        }
        if length > self.data.capacity() {
            let capacity = length
                .max(self.data.capacity().saturating_mul(2))
                .min(maximum);
            // Charge BOTH allocations while copying, not only the size delta.
            let mut replacement = Self::new(&self._lease.budget, capacity, memory_limit)?;
            replacement.data.extend_from_slice(&self.data);
            *self = replacement;
        }
        self.data.extend_from_slice(bytes);
        Ok(())
    }

    fn bytes(self) -> Bytes {
        Bytes::from_owner(self)
    }
}

/// Internal extension, never derived from an externally supplied header.
#[derive(Debug)]
pub struct BodyContext {
    pub request_id: String,
    version: Version,
    had_body: bool,
    complete: AtomicBool,
    wire: OnceCell<Bytes>,
    decoded: OnceCell<Bytes>,
    cf_ray: Option<String>,
    session_hash: Option<String>,
    endpoint: &'static str,
    started: Instant,
    last_progress: OnceLock<Instant>,
}

impl BodyContext {
    pub fn attach(req: &mut Request<Body>, peer: SocketAddr) -> Arc<Self> {
        if let Some(context) = req.extensions().get::<Arc<Self>>() {
            return context.clone();
        }
        let headers = req.headers();
        let cf_ray = crate::cf::is_cloudflare_peer(peer.ip())
            .then(|| headers.get("cf-ray"))
            .flatten()
            .and_then(|v| v.to_str().ok())
            .filter(|s| s.len() <= 64 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
            .map(str::to_owned);
        let session_hash = ["session_id", "x-codex-session-id", "x-session-id"]
            .iter()
            .find_map(|name| headers.get(*name))
            .filter(|v| v.as_bytes().len() <= 1024)
            .map(|v| crate::api::sha256_hex(v.as_bytes())[..16].to_string());
        let endpoint = match req.uri().path().trim_end_matches('/') {
            "/v1/responses" | "/responses" => "responses",
            "/v1/messages" | "/messages" => "messages",
            "/v1/images/generations" | "/images/generations" => "image_generation",
            path if path.starts_with("/_gateway/proxy/") => "gateway",
            _ => "other",
        };
        let context = Arc::new(Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            version: req.version(),
            had_body: !req.body().is_end_stream(),
            complete: AtomicBool::new(false),
            wire: OnceCell::new(),
            decoded: OnceCell::new(),
            cf_ray,
            session_hash,
            endpoint,
            started: Instant::now(),
            last_progress: OnceLock::new(),
        });
        req.extensions_mut().insert(context.clone());
        context
    }

    pub fn finish(&self, mut response: Response) -> Response {
        response.headers_mut().insert(
            "x-cc-switch-ingress-request-id",
            HeaderValue::from_str(&self.request_id).unwrap(),
        );
        // HTTP/2 only resets the rejected stream; never send Connection there.
        if self.had_body
            && !self.complete.load(Ordering::Acquire)
            && matches!(self.version, Version::HTTP_10 | Version::HTTP_11)
            && response.status() != StatusCode::SWITCHING_PROTOCOLS
        {
            response
                .headers_mut()
                .insert(header::CONNECTION, HeaderValue::from_static("close"));
        }
        response
    }

    fn log(&self, error: &BodyFailure, surface: &str) {
        warn!(request_id = %self.request_id, surface, endpoint = self.endpoint,
            cf_ray = self.cf_ray.as_deref().unwrap_or("-"),
            session_hash = self.session_hash.as_deref().unwrap_or("-"),
            phase = error.kind.name(), declared_bytes = error.declared,
            inspected_bytes = error.inspected_bytes,
            received_bytes = error.received, elapsed_ms = error.elapsed_ms,
            last_progress_age_ms = error.last_progress_age_ms, upstream_sent = false,
            "proxy ingress request body rejected");
    }

    pub fn error_response(&self, error: &BodyFailure, unified: bool) -> Response {
        let details = serde_json::json!({ "phase": error.kind.name(), "requestId": self.request_id,
            "declaredBytes": error.declared, "receivedBytes": error.received, "inspectedBytes": error.inspected_bytes,
            "elapsedMs": error.elapsed_ms, "upstreamSent": false });
        let mut value = serde_json::json!({
            "error": { "message": error.kind.message(), "code": error.kind.code(unified),
                "type": "invalid_request_error", "details": details }
        });
        if unified {
            value["error"]["type"] = error.kind.code(true).into();
            value["message"] = error.kind.message().into();
            value["code"] = error.kind.code(true).into();
            value["details"] = details;
        }
        let mut response = (error.kind.status(), axum::Json(value)).into_response();
        response.extensions_mut().insert(LocalIngressFailure);
        response
            .headers_mut()
            .insert("x-share-router-error", HeaderValue::from_static("true"));
        response.headers_mut().insert(
            "x-cc-switch-error-scope",
            HeaderValue::from_static("router"),
        );
        response.headers_mut().insert(
            "x-cc-switch-error-code",
            HeaderValue::from_str(&error.kind.code(unified)).unwrap(),
        );
        if error.kind.status() == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        self.finish(response)
    }

    pub async fn collect(
        &self,
        body: Body,
        headers: &HeaderMap,
        limit: usize,
        config: &ProxyStreamConfig,
        budget: &Arc<BodyBudget>,
        surface: &str,
    ) -> Result<Bytes, BodyFailure> {
        let result = self
            .wire
            .get_or_try_init(|| async {
                let mut progress = Progress::new(headers);
                let mut cancellation = UploadCancellation {
                    context: self,
                    budget,
                    progress: &mut progress,
                    done: false,
                };
                let result =
                    collect_wire(body, headers, limit, config, budget, cancellation.progress).await;
                cancellation.done = true;
                if result.is_ok() {
                    let _ = self.last_progress.set(cancellation.progress.last);
                    self.complete.store(true, Ordering::Release);
                    budget.completed.fetch_add(1, Ordering::Relaxed);
                }
                result
            })
            .await;
        match result {
            Ok(bytes) if bytes.len() <= limit => Ok(bytes.clone()),
            Ok(bytes) => self.reject(FailureKind::TooLarge, bytes.len(), headers, budget, surface),
            Err(error) => {
                budget.record(error.kind);
                self.log(&error, surface);
                Err(error)
            }
        }
    }

    fn reject<T>(
        &self,
        kind: FailureKind,
        received: usize,
        headers: &HeaderMap,
        budget: &BodyBudget,
        surface: &str,
    ) -> Result<T, BodyFailure> {
        let mut progress = Progress::new(headers);
        progress.received = received;
        let mut error = progress.failure(kind);
        error.received = self.wire.get().map_or(received, Bytes::len);
        error.inspected_bytes = (received != error.received).then_some(received);
        error.elapsed_ms = self.started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        error.last_progress_age_ms = self
            .last_progress
            .get()
            .unwrap_or(&self.started)
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        budget.record(kind);
        self.log(&error, surface);
        Err(error)
    }

    /// Decode only a bounded inspection view. Original bytes/CL/CE are kept for
    /// forwarding and ingress signatures, not silently recompressed or changed.
    pub async fn decoded(
        &self,
        wire: &Bytes,
        headers: &HeaderMap,
        limit: usize,
        config: &ProxyStreamConfig,
        budget: &Arc<BodyBudget>,
        surface: &str,
    ) -> Result<Bytes, BodyFailure> {
        let result = self
            .decoded
            .get_or_try_init(|| async {
                let encoding = content_encoding(headers)?;
                if encoding == "identity" {
                    return Ok(wire.clone());
                }
                let slot = budget
                    .decode_slots
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| FailureKind::DecodeBusy)?;
                let scratch = budget.reserve(
                    if encoding == "zstd" {
                        DECODE_SCRATCH
                    } else {
                        512 * 1024
                    },
                    config.request_memory_budget_bytes(),
                )?;
                let wire = wire.clone();
                let budget = budget.clone();
                let memory_limit = config.request_memory_budget_bytes();
                // The job owns all leases. Cancellation of the HTTP future does
                // not release them while a blocking decoder is still running.
                let deadline = std::time::Instant::now() + DECODE_MAX_DURATION;
                let mut job = AbortJob(tokio::task::spawn_blocking(move || {
                    let (_slot, _scratch) = (slot, scratch);
                    decode(wire, encoding, limit, memory_limit, &budget, deadline)
                }));
                tokio::time::timeout(DECODE_MAX_DURATION, &mut job.0)
                    .await
                    .map_err(|_| FailureKind::DecodeTimeout)?
                    .map_err(|_| FailureKind::InvalidEncoding)?
            })
            .await;
        match result {
            Ok(bytes) if bytes.len() <= limit => Ok(bytes.clone()),
            Ok(bytes) => self.reject(
                FailureKind::DecodedTooLarge,
                bytes.len(),
                headers,
                budget,
                surface,
            ),
            Err(kind) => self.reject(kind, wire.len(), headers, budget, surface),
        }
    }

    pub fn json_workspace(
        &self,
        bytes: &[u8],
        headers: &HeaderMap,
        config: &ProxyStreamConfig,
        budget: &Arc<BodyBudget>,
        surface: &str,
    ) -> Result<JsonWorkspace, BodyFailure> {
        match budget.reserve(
            bytes.len().saturating_mul(3).saturating_add(65536),
            config.request_memory_budget_bytes(),
        ) {
            Ok(lease) => Ok(JsonWorkspace { _lease: lease }),
            Err(kind) => self.reject(kind, bytes.len(), headers, budget, surface),
        }
    }

    pub fn rewrite_image(
        &self,
        bytes: &[u8],
        headers: &HeaderMap,
        config: &ProxyStreamConfig,
        budget: &Arc<BodyBudget>,
    ) -> Result<(Bytes, String, String), BodyFailure> {
        rewrite_image_request(bytes, budget, config.request_memory_budget_bytes())
            .or_else(|kind| self.reject(kind, bytes.len(), headers, budget, "image_rewrite"))
    }
}

// Kept alive around JSON inspection/rewrite, which uses borrowed RawValue
// fields rather than expanding an attacker-controlled JSON DOM.
pub struct JsonWorkspace {
    _lease: Lease,
}

struct Progress {
    started: Instant,
    last: Instant,
    declared: Option<u64>,
    received: usize,
}
impl Progress {
    fn new(headers: &HeaderMap) -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last: now,
            declared: headers.get(header::CONTENT_LENGTH).and_then(|v| {
                let raw = v.to_str().ok()?;
                (!raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| raw.parse().ok())
                    .flatten()
            }),
            received: 0,
        }
    }
    fn failure(&self, kind: FailureKind) -> BodyFailure {
        BodyFailure {
            kind,
            received: self.received,
            declared: self.declared,
            inspected_bytes: None,
            elapsed_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            last_progress_age_ms: self.last.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }
    }
}

struct UploadCancellation<'a> {
    context: &'a BodyContext,
    budget: &'a BodyBudget,
    progress: &'a mut Progress,
    done: bool,
}
impl Drop for UploadCancellation<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.budget.failures[7].fetch_add(1, Ordering::Relaxed);
            let error = self.progress.failure(FailureKind::ReadError);
            warn!(request_id = %self.context.request_id, received_bytes = error.received,
                elapsed_ms = error.elapsed_ms, phase = "cancelled", upstream_sent = false,
                "proxy ingress request body cancelled");
        }
    }
}

async fn collect_wire(
    body: Body,
    headers: &HeaderMap,
    limit: usize,
    config: &ProxyStreamConfig,
    budget: &Arc<BodyBudget>,
    progress: &mut Progress,
) -> Result<Bytes, BodyFailure> {
    let result = async {
        if headers.get_all(header::CONTENT_LENGTH).iter().count() > 1 {
            return Err(FailureKind::InvalidLength);
        }
        if headers.contains_key(header::CONTENT_LENGTH) && progress.declared.is_none() {
            return Err(FailureKind::InvalidLength);
        }
        if progress
            .declared
            .is_some_and(|length| length > limit as u64)
        {
            return Err(FailureKind::TooLarge);
        }
        let initial = progress.declared.map_or(0, |length| length as usize);
        let memory_limit = config.request_memory_budget_bytes();
        let mut buffer = Buffer::new(budget, initial, memory_limit)?;
        let mut body = body;
        let hard_deadline =
            progress.started + Duration::from_secs(config.request_body_timeout_secs);
        loop {
            if body.is_end_stream() {
                break;
            }
            let (phase, deadline) = if progress.received == 0 {
                (
                    FailureKind::FirstByteTimeout,
                    progress.started
                        + Duration::from_secs(config.request_body_first_byte_timeout_secs),
                )
            } else {
                (
                    FailureKind::IdleTimeout,
                    progress.last + Duration::from_secs(config.request_body_idle_timeout_secs),
                )
            };
            // Timeout branches win even when empty frames/data are always ready.
            let frame = tokio::select! { biased;
                _ = tokio::time::sleep_until(hard_deadline) => return Err(FailureKind::HardTimeout),
                _ = tokio::time::sleep_until(deadline) => return Err(phase),
                frame = body.frame() => frame,
            };
            let Some(frame) = frame else {
                break;
            };
            let frame = frame.map_err(|_| FailureKind::ReadError)?;
            if let Ok(data) = frame.into_data()
                && !data.is_empty()
            {
                progress.received = progress.received.saturating_add(data.len());
                if progress.received > limit {
                    return Err(FailureKind::TooLarge);
                }
                if progress
                    .declared
                    .is_some_and(|declared| progress.received as u64 > declared)
                {
                    return Err(FailureKind::InvalidLength);
                }
                // Transport frames exist before admission. Charge them for
                // the duration of the copy as well as the retained buffer.
                let _frame_lease = budget.reserve(data.len(), memory_limit)?;
                buffer.append(&data, limit, memory_limit)?;
                progress.last = Instant::now();
            }
            tokio::task::yield_now().await;
        }
        if progress
            .declared
            .is_some_and(|declared| progress.received as u64 != declared)
        {
            return Err(FailureKind::InvalidLength);
        }
        Ok(buffer.bytes())
    }
    .await;
    result.map_err(|kind| progress.failure(kind))
}

fn content_encoding(headers: &HeaderMap) -> Result<&'static str, FailureKind> {
    let mut values = headers.get_all(header::CONTENT_ENCODING).iter();
    let Some(value) = values.next() else {
        return Ok("identity");
    };
    if values.next().is_some() {
        return Err(FailureKind::UnsupportedEncoding);
    }
    match value.to_str().map(str::trim) {
        Ok(value) if value.eq_ignore_ascii_case("identity") => Ok("identity"),
        Ok(value) if value.eq_ignore_ascii_case("gzip") => Ok("gzip"),
        Ok(value) if value.eq_ignore_ascii_case("deflate") => Ok("deflate"),
        Ok(value) if value.eq_ignore_ascii_case("zstd") => Ok("zstd"),
        _ => Err(FailureKind::UnsupportedEncoding),
    }
}

fn decode(
    wire: Bytes,
    encoding: &str,
    limit: usize,
    memory_limit: usize,
    budget: &Arc<BodyBudget>,
    deadline: std::time::Instant,
) -> Result<Bytes, FailureKind> {
    if std::time::Instant::now() >= deadline {
        return Err(FailureKind::DecodeTimeout);
    }
    let mut deflate_input = wire.as_ref();
    let mut decoder: Box<dyn Read> = match encoding {
        "gzip" => Box::new(flate2::read::MultiGzDecoder::new(wire.as_ref())),
        "deflate" => Box::new(flate2::bufread::ZlibDecoder::new(&mut deflate_input)),
        "zstd" => {
            let mut decoder = zstd::stream::read::Decoder::new(wire.as_ref())
                .map_err(|_| FailureKind::InvalidEncoding)?;
            decoder
                .window_log_max(23)
                .map_err(|_| FailureKind::InvalidEncoding)?;
            Box::new(decoder)
        }
        _ => return Err(FailureKind::UnsupportedEncoding),
    };
    let mut buffer = Buffer::new(budget, 0, memory_limit)?;
    let mut chunk = [0_u8; 65536];
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(FailureKind::DecodeTimeout);
        }
        let count = decoder
            .read(&mut chunk)
            .map_err(|_| FailureKind::InvalidEncoding)?;
        if count == 0 {
            break;
        }
        buffer
            .append(&chunk[..count], limit, memory_limit)
            .map_err(|kind| {
                if kind == FailureKind::TooLarge {
                    FailureKind::DecodedTooLarge
                } else {
                    kind
                }
            })?;
    }
    drop(decoder);
    if encoding == "deflate" && !deflate_input.is_empty() {
        return Err(FailureKind::InvalidEncoding);
    }
    Ok(buffer.bytes())
}

struct AbortJob<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortJob<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Deserialize, Default)]
pub struct JsonFields<'a> {
    #[serde(borrow)]
    pub model: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    pub stream: Option<&'a serde_json::value::RawValue>,
}

pub fn json_fields(body: &[u8]) -> Option<JsonFields<'_>> {
    serde_json::from_slice(body).ok()
}

pub fn json_model(fields: &JsonFields<'_>) -> Option<String> {
    let raw = fields.model?.get();
    if raw.len() > 4096 {
        return None;
    }
    let model: String = serde_json::from_str(raw).ok()?;
    let model = model.trim();
    (!model.is_empty() && model.chars().count() <= 200 && !model.chars().any(char::is_control))
        .then(|| model.to_string())
}

/// Rewrite the two image control fields without expanding history, inline
/// images or arbitrary arrays into a DOM. The caller owns JSON scratch budget.
pub fn rewrite_image_request(
    body: &[u8],
    budget: &Arc<BodyBudget>,
    memory_limit: usize,
) -> Result<(Bytes, String, String), FailureKind> {
    use serde::de::{MapAccess, Visitor};
    struct BoundedWriter<'a>(&'a mut Vec<u8>);
    impl Write for BoundedWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0.capacity() - self.0.len() {
                return Err(std::io::ErrorKind::OutOfMemory.into());
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    struct ImageVisitor {
        output: Buffer,
        model: String,
        format: Option<String>,
        fallback_format: Option<String>,
    }
    impl<'de> Visitor<'de> for ImageVisitor {
        type Value = (Bytes, String, String);
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an image request object")
        }
        fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<Self::Value, A::Error> {
            self.output.data.push(b'{');
            while let Some(key) = map.next_key::<String>()? {
                let value = map.next_value::<&'de serde_json::value::RawValue>()?;
                if key == "model"
                    && let Some(model) = json_model(&JsonFields {
                        model: Some(value),
                        stream: None,
                    })
                {
                    self.model = model;
                }
                if (key == "output_format" || key == "format")
                    && value.get().len() < 64
                    && let Ok(format) = serde_json::from_str::<String>(value.get())
                {
                    if key == "output_format" {
                        self.format = Some(format);
                    } else {
                        self.fallback_format = Some(format);
                    }
                }
                if key == "stream" || key == "response_format" {
                    continue;
                }
                // Serialization may grow escaped keys, but their canonical
                // encoding is no larger than the original JSON plus headroom.
                let mut writer = BoundedWriter(&mut self.output.data);
                serde_json::to_writer(&mut writer, &key).map_err(serde::de::Error::custom)?;
                writer.write_all(b":").map_err(serde::de::Error::custom)?;
                writer
                    .write_all(value.get().as_bytes())
                    .map_err(serde::de::Error::custom)?;
                writer.write_all(b",").map_err(serde::de::Error::custom)?;
            }
            BoundedWriter(&mut self.output.data)
                .write_all(b"\"stream\":true,\"response_format\":\"b64_json\"}")
                .map_err(serde::de::Error::custom)?;
            Ok((
                self.output.bytes(),
                self.model,
                self.format
                    .or(self.fallback_format)
                    .unwrap_or_else(|| "png".into()),
            ))
        }
    }
    let output = Buffer::new(budget, body.len().saturating_add(128), memory_limit)?;
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let result = serde::Deserializer::deserialize_map(
        &mut deserializer,
        ImageVisitor {
            output,
            model: "gpt-5.5".into(),
            format: None,
            fallback_format: None,
        },
    )
    .map_err(|_| FailureKind::InvalidJson)?;
    deserializer.end().map_err(|_| FailureKind::InvalidJson)?;
    Ok(result)
}

#[cfg(test)]
mod tests;
