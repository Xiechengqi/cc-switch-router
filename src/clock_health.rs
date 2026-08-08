use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::future::join_all;
use serde::Serialize;
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::alerting::AlertingService;
use crate::alerting::models::AlertCondition;
use crate::config::ClockHealthConfig;
use crate::metrics::MetricsRegistry;

const SOURCE_AGREEMENT_MS: i64 = 2_500;
const MAX_CACHE_AGE_SECS: u64 = 5;
const BEHIND_WARNING_MS: i64 = -15_000;
const BEHIND_CRITICAL_MS: i64 = -25_000;
const BEHIND_REJECTION_MS: i64 = -30_000;
const AHEAD_WARNING_MS: i64 = 2_000;
const AHEAD_CRITICAL_MS: i64 = 4_000;
const AHEAD_REJECTION_MS: i64 = 5_000;
const RECOVERY_ABS_MS: i64 = 2_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockSourceResult {
    pub url: String,
    pub ok: bool,
    pub offset_ms: Option<i64>,
    pub rtt_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockHealthStatus {
    pub enabled: bool,
    pub status: String,
    pub direction: String,
    pub confidence: String,
    pub offset_ms: Option<i64>,
    pub uncertainty_ms: Option<u64>,
    pub valid_sources: usize,
    pub total_sources: usize,
    pub ntp_synchronized: Option<bool>,
    pub sampled_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub probe_age_secs: Option<u64>,
    pub ingress_expired_total: u64,
    pub ingress_future_total: u64,
    pub ingress_contract_error_total: u64,
    pub sources: Vec<ClockSourceResult>,
}

impl ClockHealthStatus {
    fn initial(enabled: bool, total_sources: usize) -> Self {
        Self {
            enabled,
            status: if enabled { "unknown" } else { "disabled" }.into(),
            direction: "unknown".into(),
            confidence: "unavailable".into(),
            offset_ms: None,
            uncertainty_ms: None,
            valid_sources: 0,
            total_sources,
            ntp_synchronized: None,
            sampled_at: None,
            last_success_at: None,
            probe_age_secs: None,
            ingress_expired_total: 0,
            ingress_future_total: 0,
            ingress_contract_error_total: 0,
            sources: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Risk {
    Healthy,
    Warning,
    Critical,
}

impl Risk {
    fn severity(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone)]
struct ProbeAssessment {
    offset_ms: Option<i64>,
    uncertainty_ms: Option<u64>,
    valid_sources: usize,
    total_sources: usize,
    sampled_at: i64,
    ntp_synchronized: Option<bool>,
    sources: Vec<ClockSourceResult>,
}

#[derive(Debug, Default)]
struct ClockAlertMachine {
    risk_streak: u8,
    recovery_streak: u8,
    ntp_bad_streak: u8,
    reference_bad_streak: u8,
    active_skew: Option<(Risk, i64)>,
}

impl ClockAlertMachine {
    fn observe(&mut self, assessment: &ProbeAssessment) -> Vec<AlertCondition> {
        if let Some(offset_ms) = assessment.offset_ms {
            self.reference_bad_streak = 0;
            let raw = classify_offset(offset_ms);
            match raw {
                Risk::Healthy if offset_ms.abs() < RECOVERY_ABS_MS => {
                    self.risk_streak = 0;
                    self.recovery_streak = self.recovery_streak.saturating_add(1);
                    if self.recovery_streak >= 3 {
                        self.active_skew = None;
                    }
                }
                Risk::Healthy => {
                    self.risk_streak = 0;
                    self.recovery_streak = 0;
                }
                Risk::Warning | Risk::Critical => {
                    self.recovery_streak = 0;
                    self.risk_streak = self.risk_streak.saturating_add(1);
                    let uncertainty = assessment.uncertainty_ms.unwrap_or_default() as i64;
                    let definitely_rejected = offset_ms.saturating_sub(uncertainty)
                        > AHEAD_REJECTION_MS
                        || offset_ms.saturating_add(uncertainty) < BEHIND_REJECTION_MS;
                    let already_warning = self
                        .active_skew
                        .is_some_and(|(risk, _)| risk == Risk::Warning);
                    if definitely_rejected
                        || self.risk_streak >= 2
                        || (raw == Risk::Critical && already_warning)
                    {
                        match self.active_skew {
                            Some((Risk::Critical, _)) => {
                                self.active_skew = Some((Risk::Critical, offset_ms));
                            }
                            _ => self.active_skew = Some((raw, offset_ms)),
                        }
                    }
                }
            }
        } else {
            self.reference_bad_streak = self.reference_bad_streak.saturating_add(1);
            self.recovery_streak = 0;
        }

        match assessment.ntp_synchronized {
            Some(false) => self.ntp_bad_streak = self.ntp_bad_streak.saturating_add(1),
            Some(true) => self.ntp_bad_streak = 0,
            None => {}
        }

        if let Some((risk, offset_ms)) = self.active_skew {
            return vec![skew_condition(risk, offset_ms, assessment)];
        }

        if self.reference_bad_streak >= 2 && self.ntp_bad_streak >= 2 {
            return vec![assurance_condition("critical", assessment)];
        }
        if self.reference_bad_streak >= 5 {
            return vec![assurance_condition("warning", assessment)];
        }
        if self.ntp_bad_streak >= 5 {
            return vec![ntp_condition(assessment)];
        }
        Vec::new()
    }
}

pub struct ClockHealthService {
    config: ClockHealthConfig,
    http: reqwest::Client,
    latest: RwLock<ClockHealthStatus>,
    last_probe_instant: RwLock<Option<Instant>>,
    machine: Mutex<ClockAlertMachine>,
    ingress_expired_total: AtomicU64,
    ingress_future_total: AtomicU64,
    ingress_contract_error_total: AtomicU64,
    started_at: Instant,
    ingress_expired_last_warn: AtomicU64,
    ingress_future_last_warn: AtomicU64,
    ingress_contract_last_warn: AtomicU64,
}

impl ClockHealthService {
    pub fn new(config: ClockHealthConfig) -> anyhow::Result<Arc<Self>> {
        let http = reqwest::Client::builder()
            .user_agent("cc-switch-router/0.1 clock-health")
            .connect_timeout(Duration::from_secs(config.probe_timeout_secs.min(3)))
            .timeout(Duration::from_secs(config.probe_timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Arc::new(Self {
            latest: RwLock::new(ClockHealthStatus::initial(
                config.enabled,
                config.sources.len(),
            )),
            config,
            http,
            last_probe_instant: RwLock::new(None),
            machine: Mutex::new(ClockAlertMachine::default()),
            ingress_expired_total: AtomicU64::new(0),
            ingress_future_total: AtomicU64::new(0),
            ingress_contract_error_total: AtomicU64::new(0),
            started_at: Instant::now(),
            ingress_expired_last_warn: AtomicU64::new(0),
            ingress_future_last_warn: AtomicU64::new(0),
            ingress_contract_last_warn: AtomicU64::new(0),
        }))
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub async fn snapshot(&self) -> ClockHealthStatus {
        let mut status = self.latest.read().await.clone();
        status.probe_age_secs = self
            .last_probe_instant
            .read()
            .await
            .as_ref()
            .map(Instant::elapsed)
            .map(|duration| duration.as_secs());
        status.ingress_expired_total = self.ingress_expired_total.load(Ordering::Relaxed);
        status.ingress_future_total = self.ingress_future_total.load(Ordering::Relaxed);
        status.ingress_contract_error_total =
            self.ingress_contract_error_total.load(Ordering::Relaxed);
        status
    }

    pub fn record_ingress_rejection(&self, reason: &str) -> bool {
        let last_warn = match reason {
            "expired" => {
                self.ingress_expired_total.fetch_add(1, Ordering::Relaxed);
                &self.ingress_expired_last_warn
            }
            "future_timestamp" => {
                self.ingress_future_total.fetch_add(1, Ordering::Relaxed);
                &self.ingress_future_last_warn
            }
            _ => {
                self.ingress_contract_error_total
                    .fetch_add(1, Ordering::Relaxed);
                &self.ingress_contract_last_warn
            }
        };
        should_emit_rate_limited(last_warn, self.started_at.elapsed())
    }

    async fn probe(&self) -> ProbeAssessment {
        let source_results = join_all(
            self.config
                .sources
                .iter()
                .cloned()
                .map(|url| probe_source(self.http.clone(), url)),
        )
        .await;
        let (offset_ms, uncertainty_ms, valid_sources) = consensus(&source_results);
        ProbeAssessment {
            offset_ms,
            uncertainty_ms,
            valid_sources,
            total_sources: source_results.len(),
            sampled_at: chrono::Utc::now().timestamp(),
            ntp_synchronized: read_ntp_synchronized().await,
            sources: source_results,
        }
    }

    async fn publish(&self, assessment: &ProbeAssessment) -> Vec<AlertCondition> {
        let conditions = self.machine.lock().await.observe(assessment);
        let last_success_at = self.latest.read().await.last_success_at;
        let (status, direction, confidence) = if let Some(offset_ms) = assessment.offset_ms {
            (
                classify_offset(offset_ms).severity().to_string(),
                offset_direction(offset_ms).to_string(),
                "quorum".to_string(),
            )
        } else {
            let confidence = if assessment.valid_sources == 1 {
                "single_source"
            } else {
                "unavailable"
            };
            ("degraded".into(), "unknown".into(), confidence.into())
        };
        *self.latest.write().await = ClockHealthStatus {
            enabled: true,
            status,
            direction,
            confidence,
            offset_ms: assessment.offset_ms,
            uncertainty_ms: assessment.uncertainty_ms,
            valid_sources: assessment.valid_sources,
            total_sources: assessment.total_sources,
            ntp_synchronized: assessment.ntp_synchronized,
            sampled_at: Some(assessment.sampled_at),
            last_success_at: assessment
                .offset_ms
                .map(|_| assessment.sampled_at)
                .or(last_success_at),
            probe_age_secs: Some(0),
            ingress_expired_total: self.ingress_expired_total.load(Ordering::Relaxed),
            ingress_future_total: self.ingress_future_total.load(Ordering::Relaxed),
            ingress_contract_error_total: self.ingress_contract_error_total.load(Ordering::Relaxed),
            sources: assessment.sources.clone(),
        };
        *self.last_probe_instant.write().await = Some(Instant::now());
        conditions
    }
}

fn should_emit_rate_limited(last_emit: &AtomicU64, elapsed: Duration) -> bool {
    const WARN_INTERVAL_SECS: u64 = 60;

    let now = elapsed.as_secs().saturating_add(1);
    loop {
        let previous = last_emit.load(Ordering::Relaxed);
        if previous != 0 && now.saturating_sub(previous) < WARN_INTERVAL_SECS {
            return false;
        }
        if last_emit
            .compare_exchange_weak(previous, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
    }
}

pub async fn run_clock_health_service(
    service: Arc<ClockHealthService>,
    metrics: Arc<MetricsRegistry>,
    alerting: Arc<AlertingService>,
) {
    if !service.enabled() {
        info!("router clock health monitor disabled");
        return;
    }
    let mut interval =
        tokio::time::interval(Duration::from_secs(service.config.probe_interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let assessment = service.probe().await;
        let conditions = service.publish(&assessment).await;
        if let Some(offset_ms) = assessment.offset_ms {
            let risk = classify_offset(offset_ms);
            if risk == Risk::Healthy {
                debug!(
                    clock_offset_ms = offset_ms,
                    valid_sources = assessment.valid_sources,
                    ntp_synchronized = ?assessment.ntp_synchronized,
                    "router clock health probe completed"
                );
            } else {
                warn!(
                    clock_offset_ms = offset_ms,
                    direction = offset_direction(offset_ms),
                    severity = risk.severity(),
                    valid_sources = assessment.valid_sources,
                    uncertainty_ms = ?assessment.uncertainty_ms,
                    ntp_synchronized = ?assessment.ntp_synchronized,
                    "router clock drift is approaching the ingress freshness boundary"
                );
            }
        } else {
            warn!(
                valid_sources = assessment.valid_sources,
                total_sources = assessment.total_sources,
                ntp_synchronized = ?assessment.ntp_synchronized,
                "router clock reference quorum unavailable"
            );
        }
        if let Err(error) = metrics.record_clock_sample(service.snapshot().await).await {
            debug!(%error, "persist clock health sample failed");
        }
        if let Err(error) = alerting
            .reconcile_clock(conditions, assessment.sampled_at)
            .await
        {
            debug!(%error, "reconcile clock health incident failed");
        }
    }
}

async fn probe_source(http: reqwest::Client, url: String) -> ClockSourceResult {
    let started_wall_ms = match system_time_ms() {
        Some(value) => value,
        None => return source_error(url, "local system time is before Unix epoch"),
    };
    let started = Instant::now();
    let response = match http
        .get(&url)
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return source_error(url, &format!("request failed: {error}")),
    };
    let elapsed = started.elapsed();
    if !response.status().is_success() {
        return source_error(url, &format!("HTTP status {}", response.status()));
    }
    if response
        .headers()
        .get(reqwest::header::AGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|age| age > MAX_CACHE_AGE_SECS)
    {
        return source_error(url, "cached response is too old");
    }
    let Some(date) = response
        .headers()
        .get(reqwest::header::DATE)
        .and_then(|value| value.to_str().ok())
    else {
        return source_error(url, "response has no valid Date header");
    };
    let reference = match httpdate::parse_http_date(date) {
        Ok(value) => value,
        Err(error) => return source_error(url, &format!("invalid Date header: {error}")),
    };
    let reference_ms = match reference.duration_since(UNIX_EPOCH) {
        Ok(value) => value.as_millis().min(i64::MAX as u128) as i64 + 500,
        Err(_) => return source_error(url, "Date header is before Unix epoch"),
    };
    let rtt_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
    let local_midpoint_ms = started_wall_ms.saturating_add((rtt_ms / 2) as i64);
    ClockSourceResult {
        url,
        ok: true,
        offset_ms: Some(local_midpoint_ms.saturating_sub(reference_ms)),
        rtt_ms: Some(rtt_ms),
        error: None,
    }
}

fn source_error(url: String, error: &str) -> ClockSourceResult {
    ClockSourceResult {
        url,
        ok: false,
        offset_ms: None,
        rtt_ms: None,
        error: Some(error.chars().take(240).collect()),
    }
}

fn consensus(results: &[ClockSourceResult]) -> (Option<i64>, Option<u64>, usize) {
    let mut valid = results
        .iter()
        .filter_map(|sample| Some((sample.offset_ms?, sample.rtt_ms?)))
        .collect::<Vec<_>>();
    valid.sort_by_key(|sample| sample.0);
    let mut best: &[(i64, u64)] = &[];
    for start in 0..valid.len() {
        for end in (start + 1)..=valid.len() {
            let candidate = &valid[start..end];
            if candidate.last().unwrap().0 - candidate.first().unwrap().0 <= SOURCE_AGREEMENT_MS
                && candidate.len() > best.len()
            {
                best = candidate;
            }
        }
    }
    if best.len() < 2 {
        return (None, None, valid.len());
    }
    let offset_ms = if best.len() % 2 == 1 {
        best[best.len() / 2].0
    } else {
        best[best.len() / 2 - 1]
            .0
            .saturating_add(best[best.len() / 2].0)
            / 2
    };
    let spread = (best.last().unwrap().0 - best.first().unwrap().0).unsigned_abs();
    let max_rtt = best.iter().map(|sample| sample.1).max().unwrap_or_default();
    let uncertainty_ms = 1_000_u64
        .saturating_add(max_rtt / 2)
        .saturating_add(spread / 2);
    (Some(offset_ms), Some(uncertainty_ms), best.len())
}

async fn read_ntp_synchronized() -> Option<bool> {
    let output = tokio::time::timeout(
        Duration::from_secs(2),
        Command::new("timedatectl")
            .args(["show", "-p", "NTPSynchronized", "--value"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    match String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "yes" | "true" | "1" => Some(true),
        "no" | "false" | "0" => Some(false),
        _ => None,
    }
}

fn system_time_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
}

fn classify_offset(offset_ms: i64) -> Risk {
    if offset_ms >= AHEAD_CRITICAL_MS || offset_ms <= BEHIND_CRITICAL_MS {
        Risk::Critical
    } else if offset_ms >= AHEAD_WARNING_MS || offset_ms <= BEHIND_WARNING_MS {
        Risk::Warning
    } else {
        Risk::Healthy
    }
}

fn offset_direction(offset_ms: i64) -> &'static str {
    if offset_ms >= AHEAD_WARNING_MS {
        "ahead"
    } else if offset_ms <= -RECOVERY_ABS_MS {
        "behind"
    } else {
        "aligned"
    }
}

fn skew_condition(risk: Risk, offset_ms: i64, assessment: &ProbeAssessment) -> AlertCondition {
    let direction = offset_direction(offset_ms);
    AlertCondition {
        fingerprint: "router_clock_skew:router:router".into(),
        scope: "clock".into(),
        kind: "router_clock_skew".into(),
        entity_kind: "router".into(),
        entity_id: Some("router".into()),
        severity: risk.severity().into(),
        title: "Router clock drift".into(),
        message: format!(
            "Router clock is {direction} by {:.3} seconds",
            offset_ms.abs() as f64 / 1_000.0
        ),
        details: serde_json::json!({
            "clockOffsetMs": offset_ms,
            "direction": direction,
            "uncertaintyMs": assessment.uncertainty_ms,
            "validSources": assessment.valid_sources,
            "ntpSynchronized": assessment.ntp_synchronized,
        }),
    }
}

fn assurance_condition(severity: &str, assessment: &ProbeAssessment) -> AlertCondition {
    AlertCondition {
        fingerprint: "router_clock_assurance_lost:router:router".into(),
        scope: "clock".into(),
        kind: "router_clock_assurance_lost".into(),
        entity_kind: "router".into(),
        entity_id: Some("router".into()),
        severity: severity.into(),
        title: "Router clock assurance unavailable".into(),
        message: "Router cannot establish an external time quorum".into(),
        details: serde_json::json!({
            "validSources": assessment.valid_sources,
            "totalSources": assessment.total_sources,
            "ntpSynchronized": assessment.ntp_synchronized,
        }),
    }
}

fn ntp_condition(assessment: &ProbeAssessment) -> AlertCondition {
    AlertCondition {
        fingerprint: "router_ntp_unsynchronized:router:router".into(),
        scope: "clock".into(),
        kind: "router_ntp_unsynchronized".into(),
        entity_kind: "router".into(),
        entity_id: Some("router".into()),
        severity: "warning".into(),
        title: "Router NTP is not synchronized".into(),
        message: "External time is currently aligned, but the primary NTP guard is unavailable"
            .into(),
        details: serde_json::json!({
            "clockOffsetMs": assessment.offset_ms,
            "validSources": assessment.valid_sources,
            "ntpSynchronized": false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(offset_ms: i64, rtt_ms: u64) -> ClockSourceResult {
        ClockSourceResult {
            url: format!("https://time-{offset_ms}.example"),
            ok: true,
            offset_ms: Some(offset_ms),
            rtt_ms: Some(rtt_ms),
            error: None,
        }
    }

    fn assessment(offset_ms: Option<i64>, ntp: Option<bool>) -> ProbeAssessment {
        ProbeAssessment {
            offset_ms,
            uncertainty_ms: offset_ms.map(|_| 1_000),
            valid_sources: usize::from(offset_ms.is_some()) * 2,
            total_sources: 3,
            sampled_at: 1,
            ntp_synchronized: ntp,
            sources: Vec::new(),
        }
    }

    #[test]
    fn quorum_uses_agreeing_sources_and_rejects_an_outlier() {
        let samples = vec![
            source(-30_100, 100),
            source(-29_900, 120),
            source(80_000, 50),
        ];
        let (offset, uncertainty, count) = consensus(&samples);
        assert_eq!(offset, Some(-30_000));
        assert_eq!(count, 2);
        assert!(uncertainty.unwrap() >= 1_000);
    }

    #[test]
    fn one_source_is_not_a_clock_skew_measurement() {
        let samples = vec![
            source(40_000, 100),
            source_error("https://b.example".into(), "timeout"),
        ];
        assert_eq!(consensus(&samples).0, None);
    }

    #[test]
    fn thresholds_are_asymmetric_to_match_the_protocol() {
        assert_eq!(classify_offset(1_999), Risk::Healthy);
        assert_eq!(classify_offset(2_000), Risk::Warning);
        assert_eq!(classify_offset(4_000), Risk::Critical);
        assert_eq!(classify_offset(-14_999), Risk::Healthy);
        assert_eq!(classify_offset(-15_000), Risk::Warning);
        assert_eq!(classify_offset(-25_000), Risk::Critical);
    }

    #[test]
    fn alerts_require_confirmation_and_recovery_hysteresis() {
        let mut machine = ClockAlertMachine::default();
        assert!(
            machine
                .observe(&assessment(Some(-26_000), Some(false)))
                .is_empty()
        );
        assert_eq!(
            machine.observe(&assessment(Some(-26_000), Some(false)))[0].severity,
            "critical"
        );
        assert_eq!(machine.observe(&assessment(Some(0), Some(true))).len(), 1);
        assert_eq!(machine.observe(&assessment(Some(0), Some(true))).len(), 1);
        assert!(machine.observe(&assessment(Some(0), Some(true))).is_empty());
    }

    #[test]
    fn hard_rejection_boundary_alerts_immediately() {
        let mut machine = ClockAlertMachine::default();
        let mut sample = assessment(Some(7_000), Some(false));
        sample.uncertainty_ms = Some(1_000);
        assert_eq!(machine.observe(&sample)[0].severity, "critical");
    }

    #[test]
    fn ingress_warnings_are_limited_by_monotonic_process_time() {
        let last_emit = AtomicU64::new(0);
        assert!(should_emit_rate_limited(&last_emit, Duration::ZERO));
        assert!(!should_emit_rate_limited(
            &last_emit,
            Duration::from_secs(59)
        ));
        assert!(should_emit_rate_limited(
            &last_emit,
            Duration::from_secs(60)
        ));
    }
}
