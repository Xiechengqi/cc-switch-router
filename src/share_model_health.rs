use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::ctl_client::authorize_control_request;
use crate::models::ProviderModelProbe;
use crate::proxy::ProxyRegistry;
use crate::store::{
    AppStore, ShareModelHealthSlotResult, ShareModelProbeEpochInput, ShareRouteTarget,
    model_probe_actual_model,
};

const SLOT_SECONDS: i64 = 30 * 60;
const SLOT_RESULT_GRACE_SECONDS: i64 = 5 * 60;
const SLOT_RETENTION_DAYS: i64 = 400;
const BATCH_TARGET_LIMIT: usize = 256;
const MAX_CONCURRENT_INSTALLATION_BATCHES: usize = 16;
const BATCH_RESPONSE_LIMIT_BYTES: usize = 1024 * 1024;
const BATCH_V1_PATH: &str = "/_share-router/model-health/batch";
const BATCH_V2_PATH: &str = "/_share-router/model-health/batch-v2";
const MAX_SHARE_ROUTE_FALLBACKS: usize = 2;
pub(crate) const BATCH_REQUEST_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(7 * 60);

#[derive(Debug, Clone)]
struct SelectedApp {
    app_type: &'static str,
    api_type: &'static str,
    probe: Option<ProviderModelProbe>,
}

#[derive(Debug, Clone)]
struct PreparedTarget {
    target: ShareRouteTarget,
    selected: SelectedApp,
    probe_epoch_id: String,
}

#[derive(Debug, Clone)]
struct ClaimedTarget {
    target: ShareRouteTarget,
    selected: SelectedApp,
    claim_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    contract_version: Option<u16>,
    cycle_id: &'a str,
    targets: Vec<BatchTarget<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchTarget<'a> {
    share_id: &'a str,
    app_type: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchResponse {
    ok: bool,
    cycle_id: String,
    results: Vec<BatchResult>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchResult {
    share_id: String,
    app_type: String,
    requested_model: String,
    actual_model: String,
    status: String,
    status_code: Option<u16>,
    latency_ms: u64,
    checked_at: i64,
    #[serde(default)]
    retry_count: u32,
    error_category: Option<String>,
    error_message: Option<String>,
    provider_id: String,
    provider_name: String,
    policy_mode: Option<String>,
    health_fingerprint: Option<String>,
    observation_id: Option<String>,
    outcome: Option<String>,
    failure_domain: Option<String>,
    reason_code: Option<String>,
    evidence_scope: Option<String>,
    evidence_version: Option<u16>,
}

#[derive(Debug)]
struct ReceivedBatchResponse {
    status: reqwest::StatusCode,
    bytes: Vec<u8>,
    evidence_v2: bool,
}

#[derive(Debug)]
struct ResultEvidence {
    observation_id: Option<String>,
    outcome: String,
    failure_domain: Option<String>,
    reason_code: Option<String>,
    evidence_scope: String,
    evidence_version: u16,
}

#[derive(Debug)]
enum BatchResponseReadError {
    TooLarge,
    Read(String),
}

async fn read_batch_response_bytes(
    response: reqwest::Response,
) -> Result<Vec<u8>, BatchResponseReadError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| BatchResponseReadError::Read(error.to_string()))?;
        if body.len().saturating_add(chunk.len()) > BATCH_RESPONSE_LIMIT_BYTES {
            return Err(BatchResponseReadError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn send_batch_request(
    client: &reqwest::Client,
    config: &Config,
    subdomain: &str,
    path: &str,
    installation_id: &str,
    control_secret: &str,
    body: &[u8],
    timeout: std::time::Duration,
) -> reqwest::Result<reqwest::Response> {
    let url = format!("{}{path}", config.tunnel_url(subdomain));
    authorize_control_request(
        client
            .post(url)
            .header("content-type", "application/json")
            .body(body.to_vec())
            .timeout(timeout),
        "POST",
        path,
        installation_id,
        control_secret,
        body,
    )
    .send()
    .await
}

pub async fn run_cycle(
    store: &AppStore,
    proxy: &ProxyRegistry,
    config: &Config,
    client: &reqwest::Client,
    now: DateTime<Utc>,
) -> Result<()> {
    let slot_start = floor_slot(now.timestamp());
    let cycle_id = format!("utc-{slot_start}");
    let source = format!("cc-switch-router-cycle:{cycle_id}");
    let active_subdomains = proxy
        .active_subdomains()
        .await
        .into_iter()
        .collect::<HashSet<_>>();
    let client_routes = store
        .list_client_tunnel_route_targets()
        .await?
        .into_iter()
        .filter(|target| active_subdomains.contains(&target.subdomain))
        .map(|target| (target.installation_id, target.subdomain))
        .collect::<HashMap<_, _>>();
    let mut grouped = BTreeMap::<String, Vec<PreparedTarget>>::new();

    for target in store.list_share_route_targets().await? {
        let selected = select_app(&target);
        let epoch_input = selected
            .as_ref()
            .and_then(|selected| selected_epoch_input(&target, selected));
        let Some(probe_epoch_id) = store
            .sync_share_model_probe_epoch(&target.share_id, slot_start, epoch_input.as_ref())
            .await?
        else {
            continue;
        };
        let Some(selected) = selected else {
            continue;
        };
        let prepared = PreparedTarget {
            target,
            selected,
            probe_epoch_id,
        };
        grouped
            .entry(prepared.target.installation_id.clone())
            .or_default()
            .push(prepared);
    }

    stream::iter(grouped.into_iter().map(|(installation_id, targets)| {
        let cycle_id = &cycle_id;
        let source = &source;
        let mut route_candidates = Vec::new();
        if let Some(client_route) = client_routes.get(&installation_id) {
            route_candidates.push(client_route.clone());
        }
        let mut share_routes = targets
            .iter()
            .map(|target| target.target.subdomain.clone())
            .filter(|subdomain| active_subdomains.contains(subdomain))
            .collect::<Vec<_>>();
        share_routes.sort();
        share_routes.dedup();
        for subdomain in share_routes.into_iter().take(MAX_SHARE_ROUTE_FALLBACKS) {
            if !route_candidates.contains(&subdomain) {
                route_candidates.push(subdomain);
            }
        }
        async move {
            for chunk in targets.chunks(BATCH_TARGET_LIMIT) {
                let claim_now = Utc::now().timestamp();
                if !slot_accepts_new_claim(slot_start, claim_now) {
                    tracing::warn!(
                        installation_id,
                        slot_start,
                        "skipped queued Share model health targets after their UTC slot ended"
                    );
                    break;
                }
                let claimed =
                    match claim_prepared_targets(store, chunk, slot_start, claim_now, source).await
                    {
                        Ok(claimed) => claimed,
                        Err(error) => {
                            tracing::warn!(
                                installation_id,
                                slot_start,
                                %error,
                                "claim queued Share model health batch failed"
                            );
                            continue;
                        }
                    };
                if claimed.is_empty() {
                    continue;
                }
                run_installation_batch(
                    store,
                    config,
                    client,
                    &installation_id,
                    &route_candidates,
                    &claimed,
                    slot_start,
                    claim_now,
                    cycle_id,
                    source,
                )
                .await;
            }
        }
    }))
    .buffer_unordered(MAX_CONCURRENT_INSTALLATION_BATCHES)
    .collect::<Vec<_>>()
    .await;

    store
        .prune_share_model_health_slots(slot_start.saturating_sub(SLOT_RETENTION_DAYS * 86_400))
        .await?;
    Ok(())
}

async fn claim_prepared_targets(
    store: &AppStore,
    targets: &[PreparedTarget],
    slot_start: i64,
    now: i64,
    source: &str,
) -> Result<Vec<ClaimedTarget>> {
    let mut claimed = Vec::with_capacity(targets.len());
    for target in targets {
        let Some(probe) = target.selected.probe.as_ref() else {
            continue;
        };
        let Some(claim_token) = store
            .claim_share_model_health_slot(
                &target.target.share_id,
                slot_start,
                &target.probe_epoch_id,
                target.selected.app_type,
                target.selected.api_type,
                &probe.requested_model,
                source,
                now,
            )
            .await?
        else {
            continue;
        };
        claimed.push(ClaimedTarget {
            target: target.target.clone(),
            selected: target.selected.clone(),
            claim_token,
        });
    }
    Ok(claimed)
}

#[allow(clippy::too_many_arguments)]
async fn run_installation_batch(
    store: &AppStore,
    config: &Config,
    client: &reqwest::Client,
    installation_id: &str,
    route_candidates: &[String],
    targets: &[ClaimedTarget],
    slot_start: i64,
    now: i64,
    cycle_id: &str,
    source: &str,
) {
    if targets.is_empty() {
        return;
    }
    let Some(total_budget) = remaining_probe_budget(slot_start, Utc::now().timestamp()) else {
        finish_batch_failure(
            store,
            targets,
            slot_start,
            now,
            source,
            "batch_deadline_exceeded",
            "The Share model health batch missed its UTC slot result deadline",
            "router_monitor",
            None,
        )
        .await;
        return;
    };
    let deadline = tokio::time::Instant::now() + total_budget;
    if route_candidates.is_empty() {
        finish_batch_failure(
            store,
            targets,
            slot_start,
            now,
            source,
            "control_route_unavailable",
            "No active installation control route is available",
            "control_transport",
            None,
        )
        .await;
        return;
    }
    let batch_targets = || {
        targets
            .iter()
            .map(|target| BatchTarget {
                share_id: &target.target.share_id,
                app_type: target.selected.api_type,
            })
            .collect()
    };
    let v2_body = serde_json::to_vec(&BatchRequest {
        contract_version: Some(2),
        cycle_id,
        targets: batch_targets(),
    });
    let v1_body = serde_json::to_vec(&BatchRequest {
        contract_version: None,
        cycle_id,
        targets: batch_targets(),
    });
    let (v2_body, v1_body) = match (v2_body, v1_body) {
        (Ok(v2_body), Ok(v1_body)) => (v2_body, v1_body),
        (Err(error), _) => {
            finish_batch_failure(
                store,
                targets,
                slot_start,
                now,
                source,
                "request_encode",
                &error.to_string(),
                "router_monitor",
                None,
            )
            .await;
            return;
        }
        (_, Err(error)) => {
            finish_batch_failure(
                store,
                targets,
                slot_start,
                now,
                source,
                "request_encode",
                &error.to_string(),
                "router_monitor",
                None,
            )
            .await;
            return;
        }
    };
    let control_secret = match store.installation_control_secret(installation_id).await {
        Ok(Some(secret)) if !secret.trim().is_empty() => secret,
        Ok(_) => {
            finish_batch_failure(
                store,
                targets,
                slot_start,
                now,
                source,
                "control_secret_missing",
                "Installation control secret is missing",
                "control_transport",
                None,
            )
            .await;
            return;
        }
        Err(error) => {
            finish_batch_failure(
                store,
                targets,
                slot_start,
                now,
                source,
                "control_secret_read",
                &error.to_string(),
                "router_monitor",
                None,
            )
            .await;
            return;
        }
    };
    let mut received = None;
    let mut last_transport_error = None::<(String, Option<u16>)>;
    for subdomain in route_candidates {
        let Some(request_timeout) = remaining_request_timeout(deadline) else {
            last_transport_error = Some((
                "Share model health batch exhausted its total request budget".to_string(),
                None,
            ));
            break;
        };
        let response = match send_batch_request(
            client,
            config,
            subdomain,
            BATCH_V2_PATH,
            installation_id,
            &control_secret,
            &v2_body,
            request_timeout,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                last_transport_error = Some((error.to_string(), None));
                continue;
            }
        };
        let mut evidence_v2 = true;
        let response = if matches!(response.status().as_u16(), 404 | 405) {
            evidence_v2 = false;
            let Some(request_timeout) = remaining_request_timeout(deadline) else {
                last_transport_error = Some((
                    "Share model health batch exhausted its total request budget before the legacy fallback"
                        .to_string(),
                    None,
                ));
                break;
            };
            match send_batch_request(
                client,
                config,
                subdomain,
                BATCH_V1_PATH,
                installation_id,
                &control_secret,
                &v1_body,
                request_timeout,
            )
            .await
            {
                Ok(response) => response,
                Err(error) => {
                    last_transport_error = Some((error.to_string(), None));
                    // Legacy v1 has no canonical observation/idempotency contract.
                    // An ambiguous transport failure must not consume the same
                    // Provider probe again through another Share route.
                    break;
                }
            }
        } else {
            response
        };
        let status = response.status();
        if matches!(status.as_u16(), 502 | 503 | 504) {
            last_transport_error = Some((
                format!(
                    "installation control route returned HTTP {}",
                    status.as_u16()
                ),
                Some(status.as_u16()),
            ));
            if !evidence_v2 {
                break;
            }
            continue;
        }
        let bytes = match read_batch_response_bytes(response).await {
            Ok(bytes) => bytes,
            Err(BatchResponseReadError::TooLarge) => {
                finish_batch_failure(
                    store,
                    targets,
                    slot_start,
                    now,
                    source,
                    "response_too_large",
                    "Server model health response exceeded 1 MiB",
                    "router_monitor",
                    Some(status.as_u16()),
                )
                .await;
                return;
            }
            Err(BatchResponseReadError::Read(error)) => {
                if evidence_v2 {
                    last_transport_error = Some((error, Some(status.as_u16())));
                    continue;
                }
                finish_batch_failure(
                    store,
                    targets,
                    slot_start,
                    now,
                    source,
                    "response_read",
                    &error,
                    "control_transport",
                    Some(status.as_u16()),
                )
                .await;
                return;
            }
        };
        received = Some(ReceivedBatchResponse {
            status,
            bytes,
            evidence_v2,
        });
        break;
    }
    let Some(received) = received else {
        let (message, status_code) = last_transport_error.unwrap_or_else(|| {
            (
                "No installation control route completed the request".to_string(),
                None,
            )
        });
        finish_batch_failure(
            store,
            targets,
            slot_start,
            now,
            source,
            "control_transport_failed",
            &message,
            "control_transport",
            status_code,
        )
        .await;
        return;
    };
    let status = received.status;
    let bytes = received.bytes;
    let evidence_v2 = received.evidence_v2;
    if !status.is_success() {
        finish_batch_failure(
            store,
            targets,
            slot_start,
            now,
            source,
            "server_http",
            &format!(
                "Server model health endpoint returned HTTP {}",
                status.as_u16()
            ),
            "router_monitor",
            Some(status.as_u16()),
        )
        .await;
        return;
    }
    let response = match serde_json::from_slice::<BatchResponse>(&bytes) {
        Ok(response) if response.ok && response.cycle_id == cycle_id => response,
        Ok(_) => {
            finish_batch_failure(
                store,
                targets,
                slot_start,
                now,
                source,
                "response_mismatch",
                "Server model health response did not match the requested cycle",
                "router_monitor",
                Some(status.as_u16()),
            )
            .await;
            return;
        }
        Err(error) => {
            finish_batch_failure(
                store,
                targets,
                slot_start,
                now,
                source,
                "response_decode",
                &error.to_string(),
                "router_monitor",
                Some(status.as_u16()),
            )
            .await;
            return;
        }
    };
    let expected_share_ids = targets
        .iter()
        .map(|target| target.target.share_id.as_str())
        .collect::<HashSet<_>>();
    let mut results = HashMap::with_capacity(response.results.len());
    for result in response.results {
        if !expected_share_ids.contains(result.share_id.as_str())
            || results.insert(result.share_id.clone(), result).is_some()
        {
            finish_batch_failure(
                store,
                targets,
                slot_start,
                now,
                source,
                "response_targets_mismatch",
                "Server model health response contained an unexpected or duplicate Share",
                "router_monitor",
                Some(status.as_u16()),
            )
            .await;
            return;
        }
    }
    for target in targets {
        let Some(result) = results.get(&target.target.share_id) else {
            if let Err(error) = finish_failure(
                store,
                target,
                slot_start,
                now,
                source,
                "result_missing",
                "Server omitted this Share from the model health response",
                "router_monitor",
                Some(status.as_u16()),
            )
            .await
            {
                tracing::warn!(share_id = %target.target.share_id, error = %error, "record missing model health result failed");
            }
            continue;
        };
        let evidence = match validate_batch_result(
            target,
            result,
            installation_id,
            cycle_id,
            evidence_v2,
        ) {
            Ok(evidence) => evidence,
            Err(message) => {
                if let Err(error) = finish_failure(
                    store,
                    target,
                    slot_start,
                    now,
                    source,
                    "result_contract_mismatch",
                    message,
                    "router_monitor",
                    Some(status.as_u16()),
                )
                .await
                {
                    tracing::warn!(share_id = %target.target.share_id, error = %error, "record invalid model health result failed");
                }
                continue;
            }
        };
        let normalized_status = result.status.clone();
        let checked_at = normalize_checked_at(result.checked_at, slot_start);
        let error_message = result.error_message.clone().or_else(|| {
            (evidence.outcome == "failure").then(|| "Server model health check failed".to_string())
        });
        let finish = ShareModelHealthSlotResult {
            installation_id: target.target.installation_id.clone(),
            capacity_pool_id: target.target.capacity_pool_id.clone(),
            subdomain: target.target.subdomain.clone(),
            app_type: target.selected.app_type.to_string(),
            api_type: target.selected.api_type.to_string(),
            requested_model: result.requested_model.clone(),
            actual_model: result.actual_model.clone(),
            status: normalized_status,
            status_code: result.status_code,
            latency_ms: result.latency_ms,
            provider_id: Some(result.provider_id.clone()),
            provider_name: Some(result.provider_name.clone()),
            policy_mode: result.policy_mode.clone(),
            health_fingerprint: result.health_fingerprint.clone(),
            observation_id: evidence.observation_id,
            outcome: evidence.outcome,
            failure_domain: evidence.failure_domain,
            reason_code: evidence.reason_code,
            evidence_scope: evidence.evidence_scope,
            evidence_version: evidence.evidence_version,
            error_category: result.error_category.clone(),
            error_message,
            checked_at,
            source: source.to_string(),
        };
        match store
            .finish_share_model_health_slot(
                &target.target.share_id,
                slot_start,
                &target.claim_token,
                finish,
            )
            .await
        {
            Ok(true) => {
                tracing::debug!(share_id = %target.target.share_id, retries = result.retry_count, "Share model health result recorded");
            }
            Ok(false) => {
                tracing::debug!(share_id = %target.target.share_id, "ignored stale Share model health result");
            }
            Err(error) => {
                tracing::warn!(share_id = %target.target.share_id, error = %error, "record canonical model health result failed");
                if let Err(error) = finish_failure(
                    store,
                    target,
                    slot_start,
                    now,
                    source,
                    "evidence_store",
                    "Router could not persist the canonical model health evidence",
                    "router_monitor",
                    Some(status.as_u16()),
                )
                .await
                {
                    tracing::warn!(share_id = %target.target.share_id, error = %error, "record model health evidence gap failed");
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_failure(
    store: &AppStore,
    target: &ClaimedTarget,
    slot_start: i64,
    checked_at: i64,
    source: &str,
    category: &str,
    message: &str,
    failure_domain: &str,
    status_code: Option<u16>,
) -> Result<()> {
    let probe = target.selected.probe.as_ref();
    let epoch = selected_epoch_input(&target.target, &target.selected);
    store
        .finish_share_model_health_slot(
            &target.target.share_id,
            slot_start,
            &target.claim_token,
            ShareModelHealthSlotResult {
                installation_id: target.target.installation_id.clone(),
                capacity_pool_id: target.target.capacity_pool_id.clone(),
                subdomain: target.target.subdomain.clone(),
                app_type: target.selected.app_type.to_string(),
                api_type: target.selected.api_type.to_string(),
                requested_model: probe
                    .map(|probe| probe.requested_model.clone())
                    .unwrap_or_else(|| target.selected.app_type.to_string()),
                actual_model: probe
                    .map(|probe| probe.wire_model.clone())
                    .unwrap_or_default(),
                status: if category == "timeout" {
                    "timeout".to_string()
                } else if category == "route_offline" {
                    "offline".to_string()
                } else {
                    "failed".to_string()
                },
                status_code,
                latency_ms: 0,
                provider_id: epoch.as_ref().map(|epoch| epoch.provider_id.clone()),
                provider_name: epoch.as_ref().and_then(|epoch| epoch.provider_name.clone()),
                policy_mode: epoch.as_ref().and_then(|epoch| epoch.policy_mode.clone()),
                health_fingerprint: probe.map(|probe| probe.health_fingerprint.clone()),
                observation_id: None,
                outcome: "unobserved".to_string(),
                failure_domain: Some(failure_domain.to_string()),
                reason_code: Some(category.to_string()),
                evidence_scope: "share_projection".to_string(),
                evidence_version: 2,
                error_category: Some(category.to_string()),
                error_message: Some(message.chars().take(500).collect()),
                checked_at,
                source: source.to_string(),
            },
        )
        .await
        .context("finish failed Share model health slot")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn finish_batch_failure(
    store: &AppStore,
    targets: &[ClaimedTarget],
    slot_start: i64,
    checked_at: i64,
    source: &str,
    category: &str,
    message: &str,
    failure_domain: &str,
    status_code: Option<u16>,
) {
    for target in targets {
        if let Err(error) = finish_failure(
            store,
            target,
            slot_start,
            checked_at,
            source,
            category,
            message,
            failure_domain,
            status_code,
        )
        .await
        {
            tracing::warn!(share_id = %target.target.share_id, error = %error, "record model health batch failure failed");
        }
    }
}

fn validate_batch_result(
    target: &ClaimedTarget,
    result: &BatchResult,
    installation_id: &str,
    cycle_id: &str,
    evidence_v2: bool,
) -> std::result::Result<ResultEvidence, &'static str> {
    let expected = selected_epoch_input(&target.target, &target.selected)
        .ok_or("Share modelProbe configuration disappeared during the health cycle")?;
    if result.app_type != target.selected.api_type {
        return Err("Server returned a mismatched model health App");
    }
    if result.requested_model != expected.requested_model
        || result.actual_model != expected.wire_model
    {
        return Err("Server result does not match the published test model");
    }
    if result.provider_id != expected.provider_id
        || result.health_fingerprint.as_deref() != Some(expected.health_fingerprint.as_str())
        || result.policy_mode.as_deref() != expected.policy_mode.as_deref()
    {
        return Err("Server result does not match the published Provider runtime");
    }
    if result.provider_name.trim().is_empty() {
        return Err("Server result omitted the Provider name");
    }
    if result
        .status_code
        .is_some_and(|status_code| !(100..=599).contains(&status_code))
    {
        return Err("Server result contained an invalid HTTP status code");
    }
    if !matches!(
        result.status.as_str(),
        "success" | "degraded" | "quota_blocked" | "failed"
    ) {
        return Err("Server result contained an invalid model health status");
    }

    if !evidence_v2 {
        if result.observation_id.is_some()
            || result.outcome.is_some()
            || result.failure_domain.is_some()
            || result.reason_code.is_some()
            || result.evidence_scope.is_some()
            || result.evidence_version.is_some()
        {
            return Err("Legacy Server response mixed incompatible evidence fields");
        }
        let succeeded = matches!(result.status.as_str(), "success" | "degraded");
        return Ok(ResultEvidence {
            observation_id: None,
            outcome: if succeeded { "success" } else { "failure" }.to_string(),
            failure_domain: (!succeeded).then(|| "unknown".to_string()),
            reason_code: Some(
                if succeeded {
                    "legacy_probe_succeeded"
                } else {
                    "legacy_probe_failed"
                }
                .to_string(),
            ),
            evidence_scope: "share_legacy".to_string(),
            evidence_version: 1,
        });
    }

    if result.evidence_scope.as_deref() != Some("provider_runtime")
        || result.evidence_version != Some(2)
    {
        return Err("Server result contained an unsupported evidence contract");
    }
    let reason_code = result
        .reason_code
        .as_deref()
        .filter(|reason| {
            !reason.is_empty()
                && reason.len() <= 128
                && reason
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
        .ok_or("Server result contained an invalid evidence reason")?;
    let outcome = result
        .outcome
        .as_deref()
        .ok_or("Server result omitted the evidence outcome")?;
    let failure_domain = result.failure_domain.as_deref();
    let evidence_consistent = match result.status.as_str() {
        "success" | "degraded" => outcome == "success" && failure_domain.is_none(),
        "quota_blocked" => outcome == "failure" && failure_domain == Some("quota"),
        "failed" => {
            outcome == "failure"
                && failure_domain.is_some_and(|domain| {
                    matches!(domain, "upstream" | "provider_config" | "unknown")
                })
        }
        _ => false,
    };
    if !evidence_consistent {
        return Err("Server result status and evidence classification disagree");
    }
    let expected_observation_id = model_health_observation_id(
        installation_id,
        cycle_id,
        target.selected.app_type,
        &expected.provider_id,
        &expected.health_fingerprint,
    );
    if result.observation_id.as_deref() != Some(expected_observation_id.as_str()) {
        return Err("Server result contained an invalid canonical observation ID");
    }

    Ok(ResultEvidence {
        observation_id: Some(expected_observation_id),
        outcome: outcome.to_string(),
        failure_domain: failure_domain.map(str::to_string),
        reason_code: Some(reason_code.to_string()),
        evidence_scope: "provider_runtime".to_string(),
        evidence_version: 2,
    })
}

fn model_health_observation_id(
    installation_id: &str,
    cycle_id: &str,
    app_type: &str,
    provider_id: &str,
    health_fingerprint: &str,
) -> String {
    let canonical = serde_json::to_vec(&(
        "cc-switch-model-health-observation-v2",
        installation_id,
        cycle_id,
        app_type,
        provider_id,
        health_fingerprint,
    ))
    .expect("model health observation tuple is serializable");
    format!("{:x}", Sha256::digest(canonical))
}

fn selected_epoch_input(
    target: &ShareRouteTarget,
    selected: &SelectedApp,
) -> Option<ShareModelProbeEpochInput> {
    if target.contract_version < 4 {
        return None;
    }
    let runtime = match selected.app_type {
        "codex" => target.app_runtimes.codex.as_ref(),
        "claude" => target.app_runtimes.claude.as_ref(),
        "gemini" => target.app_runtimes.gemini.as_ref(),
        _ => None,
    }?;
    let probe = selected.probe.as_ref()?;
    let provider_id = target.bindings.get(selected.app_type)?.trim();
    if provider_id.is_empty()
        || target.capacity_pool_id.trim().is_empty()
        || probe.api_type != selected.api_type
        || probe.health_fingerprint.is_empty()
    {
        return None;
    }
    let policy_mode = runtime.model_policy.as_ref().map(|policy| match policy {
        crate::models::ShareProviderModelPolicy::Passthrough => "passthrough".to_string(),
        crate::models::ShareProviderModelPolicy::Single { .. } => "single".to_string(),
    });
    Some(ShareModelProbeEpochInput {
        app_type: selected.app_type.to_string(),
        api_type: selected.api_type.to_string(),
        provider_id: provider_id.to_string(),
        provider_name: runtime.provider_name.clone(),
        capacity_pool_id: target.capacity_pool_id.clone(),
        requested_model: probe.requested_model.clone(),
        wire_model: model_probe_actual_model(runtime, probe),
        policy_mode,
        health_fingerprint: probe.health_fingerprint.clone(),
    })
}

fn select_app(target: &ShareRouteTarget) -> Option<SelectedApp> {
    if target.support.codex {
        if let Some(probe) = target
            .app_runtimes
            .codex
            .as_ref()
            .and_then(|runtime| runtime.model_probe.clone())
        {
            return Some(SelectedApp {
                app_type: "codex",
                api_type: "openai",
                probe: Some(probe),
            });
        }
    }
    if target.support.claude {
        if let Some(probe) = target
            .app_runtimes
            .claude
            .as_ref()
            .and_then(|runtime| runtime.model_probe.clone())
        {
            return Some(SelectedApp {
                app_type: "claude",
                api_type: "anthropic",
                probe: Some(probe),
            });
        }
    }
    if target.support.gemini {
        if let Some(probe) = target
            .app_runtimes
            .gemini
            .as_ref()
            .and_then(|runtime| runtime.model_probe.clone())
        {
            return Some(SelectedApp {
                app_type: "gemini",
                api_type: "gemini",
                probe: Some(probe),
            });
        }
    }
    None
}

fn floor_slot(timestamp: i64) -> i64 {
    timestamp.div_euclid(SLOT_SECONDS) * SLOT_SECONDS
}

fn slot_accepts_new_claim(slot_start: i64, now: i64) -> bool {
    (slot_start..slot_start.saturating_add(SLOT_SECONDS)).contains(&now)
}

fn remaining_probe_budget(slot_start: i64, now: i64) -> Option<std::time::Duration> {
    let hard_deadline = slot_start
        .saturating_add(SLOT_SECONDS)
        .saturating_add(SLOT_RESULT_GRACE_SECONDS);
    let remaining_seconds = hard_deadline.checked_sub(now)?;
    if remaining_seconds <= 0 {
        return None;
    }
    Some(BATCH_REQUEST_TIMEOUT.min(std::time::Duration::from_secs(
        u64::try_from(remaining_seconds).ok()?,
    )))
}

fn remaining_request_timeout(deadline: tokio::time::Instant) -> Option<std::time::Duration> {
    deadline
        .checked_duration_since(tokio::time::Instant::now())
        .filter(|remaining| !remaining.is_zero())
}

fn normalize_checked_at(checked_at: i64, slot_start: i64) -> i64 {
    let earliest = slot_start.saturating_sub(SLOT_RESULT_GRACE_SECONDS);
    let latest = slot_start
        .saturating_add(SLOT_SECONDS)
        .saturating_add(SLOT_RESULT_GRACE_SECONDS);
    if (earliest..=latest).contains(&checked_at) {
        checked_at
    } else {
        slot_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ProviderModelProbe, ShareAppRuntimes, ShareProviderModelPolicy, ShareSupport,
        ShareUpstreamProvider,
    };

    fn target(support: ShareSupport) -> ShareRouteTarget {
        ShareRouteTarget {
            share_id: "share-1".into(),
            installation_id: "installation-1".into(),
            share_name: "Share 1".into(),
            subdomain: "share-1".into(),
            contract_version: 4,
            capacity_pool_id: "pool-1".into(),
            bindings: BTreeMap::from([
                ("claude".into(), "provider-claude".into()),
                ("codex".into(), "provider-codex".into()),
                ("gemini".into(), "provider-gemini".into()),
            ]),
            support,
            app_runtimes: ShareAppRuntimes {
                claude: Some(ShareUpstreamProvider::default()),
                codex: Some(ShareUpstreamProvider::default()),
                gemini: Some(ShareUpstreamProvider::default()),
                ..Default::default()
            },
        }
    }

    fn claimed_probed_target() -> ClaimedTarget {
        let mut target = target(ShareSupport {
            claude: false,
            codex: true,
            gemini: false,
        });
        let runtime = target.app_runtimes.codex.as_mut().unwrap();
        runtime.provider_name = Some("Provider Codex".into());
        runtime.model_policy = Some(ShareProviderModelPolicy::Single {
            upstream_model: "fixed-upstream-model".into(),
        });
        runtime.model_probe = Some(ProviderModelProbe {
            api_type: "openai".into(),
            requested_model: "server-test-model@low".into(),
            wire_model: "server-test-model".into(),
            method: "POST".into(),
            path: "/v1/responses".into(),
            body: serde_json::json!({"model": "server-test-model", "stream": true}),
            stream: true,
            response_mode: "responses_sse".into(),
            payload_revision: 2,
            health_fingerprint: "a".repeat(64),
        });
        let selected = select_app(&target).unwrap();
        ClaimedTarget {
            target,
            selected,
            claim_token: "claim-test".into(),
        }
    }

    fn test_probe(api_type: &str) -> ProviderModelProbe {
        ProviderModelProbe {
            api_type: api_type.into(),
            requested_model: format!("{api_type}-test-model"),
            wire_model: format!("{api_type}-test-model"),
            method: "POST".into(),
            path: "/probe".into(),
            body: serde_json::json!({"model": format!("{api_type}-test-model")}),
            stream: true,
            response_mode: "json".into(),
            payload_revision: 2,
            health_fingerprint: "a".repeat(64),
        }
    }

    fn batch_result(target: &ClaimedTarget, evidence_v2: bool) -> BatchResult {
        let observation_id = evidence_v2.then(|| {
            model_health_observation_id(
                &target.target.installation_id,
                "utc-1787529600",
                target.selected.app_type,
                "provider-codex",
                &"a".repeat(64),
            )
        });
        BatchResult {
            share_id: target.target.share_id.clone(),
            app_type: "openai".into(),
            requested_model: "server-test-model@low".into(),
            actual_model: "fixed-upstream-model".into(),
            status: "success".into(),
            status_code: Some(200),
            latency_ms: 25,
            checked_at: 1_787_529_605,
            retry_count: 0,
            error_category: None,
            error_message: None,
            provider_id: "provider-codex".into(),
            provider_name: "Provider Codex".into(),
            policy_mode: Some("single".into()),
            health_fingerprint: Some("a".repeat(64)),
            observation_id,
            outcome: evidence_v2.then(|| "success".into()),
            failure_domain: None,
            reason_code: evidence_v2.then(|| "probe_succeeded".into()),
            evidence_scope: evidence_v2.then(|| "provider_runtime".into()),
            evidence_version: evidence_v2.then_some(2),
        }
    }

    #[test]
    fn app_selection_uses_openai_then_anthropic_then_gemini() {
        let mut all = target(ShareSupport {
            claude: true,
            codex: true,
            gemini: true,
        });
        all.app_runtimes.codex.as_mut().unwrap().model_probe = Some(test_probe("openai"));
        all.app_runtimes.claude.as_mut().unwrap().model_probe = Some(test_probe("anthropic"));
        all.app_runtimes.gemini.as_mut().unwrap().model_probe = Some(test_probe("gemini"));
        let selected = select_app(&all).unwrap();
        assert_eq!(selected.app_type, "codex");
        assert_eq!(selected.api_type, "openai");

        all.support.codex = false;
        let selected = select_app(&all).unwrap();
        assert_eq!(selected.api_type, "anthropic");

        all.support.claude = false;
        let selected = select_app(&all).unwrap();
        assert_eq!(selected.api_type, "gemini");
    }

    #[test]
    fn app_selection_skips_enabled_apps_without_an_executable_probe() {
        let mut target = target(ShareSupport {
            claude: true,
            codex: true,
            gemini: true,
        });
        target.app_runtimes.claude.as_mut().unwrap().model_probe = Some(test_probe("anthropic"));
        target.app_runtimes.gemini.as_mut().unwrap().model_probe = Some(test_probe("gemini"));

        let selected = select_app(&target).unwrap();
        assert_eq!(selected.api_type, "anthropic");

        target.app_runtimes.claude.as_mut().unwrap().model_probe = None;
        assert_eq!(select_app(&target).unwrap().api_type, "gemini");

        target.app_runtimes.gemini.as_mut().unwrap().model_probe = None;
        assert!(select_app(&target).is_none());
    }

    #[test]
    fn slot_floor_is_utc_half_hour_aligned() {
        assert_eq!(floor_slot(1_787_529_599), 1_787_527_800);
        assert_eq!(floor_slot(1_787_529_600), 1_787_529_600);
        assert_eq!(MAX_CONCURRENT_INSTALLATION_BATCHES, 16);
    }

    #[test]
    fn queued_batches_cannot_claim_an_ended_slot_and_share_one_bounded_budget() {
        let slot_start = 1_787_529_600;

        assert!(slot_accepts_new_claim(slot_start, slot_start));
        assert!(slot_accepts_new_claim(
            slot_start,
            slot_start + SLOT_SECONDS - 1
        ));
        assert!(!slot_accepts_new_claim(
            slot_start,
            slot_start + SLOT_SECONDS
        ));
        assert_eq!(
            remaining_probe_budget(slot_start, slot_start),
            Some(BATCH_REQUEST_TIMEOUT)
        );
        assert_eq!(
            remaining_probe_budget(slot_start, slot_start + SLOT_SECONDS - 60),
            Some(std::time::Duration::from_secs(
                (SLOT_RESULT_GRACE_SECONDS + 60) as u64
            ))
        );
        assert_eq!(
            remaining_probe_budget(
                slot_start,
                slot_start + SLOT_SECONDS + SLOT_RESULT_GRACE_SECONDS
            ),
            None
        );
    }

    #[test]
    fn result_timestamp_must_belong_to_the_current_probe_window() {
        let slot_start = 1_787_529_600;

        assert_eq!(
            normalize_checked_at(slot_start + 60, slot_start),
            slot_start + 60
        );
        assert_eq!(
            normalize_checked_at(slot_start - 301, slot_start),
            slot_start
        );
        assert_eq!(
            normalize_checked_at(slot_start + SLOT_SECONDS + 301, slot_start),
            slot_start
        );
        assert_eq!(
            normalize_checked_at(slot_start + SLOT_SECONDS + 300, slot_start),
            slot_start + SLOT_SECONDS + 300
        );
    }

    #[test]
    fn v2_result_requires_the_expected_canonical_observation() {
        let target = claimed_probed_target();
        let result = batch_result(&target, true);
        let evidence =
            validate_batch_result(&target, &result, "installation-1", "utc-1787529600", true)
                .expect("valid v2 evidence");
        assert_eq!(evidence.outcome, "success");
        assert_eq!(evidence.evidence_scope, "provider_runtime");
        assert_eq!(evidence.evidence_version, 2);

        let mut changed = result;
        changed.observation_id = Some("b".repeat(64));
        assert!(
            validate_batch_result(&target, &changed, "installation-1", "utc-1787529600", true,)
                .is_err()
        );
    }

    #[test]
    fn v1_fallback_is_explicit_legacy_evidence() {
        let target = claimed_probed_target();
        let result = batch_result(&target, false);
        let evidence =
            validate_batch_result(&target, &result, "installation-1", "utc-1787529600", false)
                .expect("valid v1 fallback");
        assert_eq!(evidence.outcome, "success");
        assert_eq!(evidence.evidence_scope, "share_legacy");
        assert_eq!(evidence.evidence_version, 1);
        assert!(evidence.observation_id.is_none());
    }

    #[tokio::test]
    async fn batch_response_reader_rejects_oversized_bodies() {
        async fn oversized_response() -> Vec<u8> {
            vec![b'x'; BATCH_RESPONSE_LIMIT_BYTES + 1]
        }

        let app = axum::Router::new().route("/batch", axum::routing::get(oversized_response));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind response limit fixture");
        let address = listener.local_addr().expect("response limit address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve response limit fixture");
        });
        let response = reqwest::get(format!("http://{address}/batch"))
            .await
            .expect("fetch response limit fixture");

        let result = read_batch_response_bytes(response).await;
        server.abort();

        assert!(matches!(result, Err(BatchResponseReadError::TooLarge)));
    }
}
