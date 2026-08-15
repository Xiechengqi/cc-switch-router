use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use crate::error::AppError;
use crate::server_state::ServerState;
use crate::store::{
    ClientSubdomainTakeoverCommit, ClientSubdomainTakeoverPlan, ClientSubdomainTakeoverRecovery,
};

const ACTIVATION_DELAY_MS: i64 = 10_000;
const PREPARING_RECOVERY_WINDOW_MS: i64 = 2 * 60 * 1_000;
const RECOVERY_POLL_INTERVAL: Duration = Duration::from_secs(5);
pub(crate) const CLIENT_SUBDOMAIN_TAKEOVER_AUTHORIZATION_ACTION: &str =
    "client_subdomain_takeover_authorization";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ClientSubdomainTakeoverRequest {
    pub target_installation_id: String,
    pub source_installation_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientSubdomainTakeoverResponse {
    pub ok: bool,
    pub takeover_id: String,
    pub status: String,
    pub target_installation_id: String,
    pub source_installation_id: String,
    pub retired_subdomain: String,
    pub adopted_subdomain: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ClientSubdomainTakeoverAuthorizationRequest {
    pub protocol_epoch: String,
    pub installation_id: String,
    pub takeover_id: String,
    pub timestamp_ms: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientSubdomainTakeoverAuthorizationResponse {
    pub ok: bool,
    pub takeover_id: String,
    pub authorized: bool,
    pub status: String,
}

#[derive(Debug)]
struct FinalizeResult {
    warning: Option<String>,
    cleanup_pending: bool,
    server_committed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparingRecoveryTiming {
    NotDue,
    Retry,
    Expired,
}

struct RecoveryRunGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for RecoveryRunGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

pub(crate) async fn execute(
    state: ServerState,
    actor_email: &str,
    input: ClientSubdomainTakeoverRequest,
) -> Result<ClientSubdomainTakeoverResponse, AppError> {
    let (first_action_guard, second_action_guard) = lock_client_market_actions(
        &state,
        &input.target_installation_id,
        &input.source_installation_id,
    )
    .await;

    let activate_at_ms = chrono::Utc::now()
        .timestamp_millis()
        .saturating_add(ACTIVATION_DELAY_MS);
    let plan = state
        .store
        .begin_client_subdomain_takeover(
            actor_email,
            &input.target_installation_id,
            &input.source_installation_id,
            activate_at_ms,
        )
        .await?;
    let route = match active_target_route(&state, &plan).await {
        Ok(route) => route,
        Err(error) => {
            settle_preparing_failure(&state, &plan, None, &error.to_string()).await?;
            return Err(error);
        }
    };
    let backend = route.route_target().to_string();

    if let Err(control_error) = prepare_server(&plan, &backend).await {
        settle_preparing_failure(&state, &plan, Some(&backend), &control_error.to_string()).await?;
        return Err(AppError::UnprocessableEntity(format!(
            "target Client takeover prepare failed: {control_error}"
        )));
    }
    if let Err(error) = state
        .store
        .mark_client_subdomain_takeover_server_prepared(&plan.id)
        .await
    {
        spawn_recovery(state.clone());
        return Err(error);
    }
    let committed = match state.store.commit_client_subdomain_takeover(&plan.id).await {
        Ok(committed) => committed,
        Err(error) => {
            if settle_terminal_router_commit_error(&state, &plan, Some(&backend), &error).await? {
                return Err(error);
            }
            spawn_recovery(state.clone());
            return Err(error);
        }
    };
    // The durable Router commit fences B and quiesces both recovery streams. Drop
    // the guards before billing cleanup, which intentionally takes the same lock.
    drop(second_action_guard);
    drop(first_action_guard);
    let finalized =
        match finalize_committed_takeover(&state, &plan, &committed, Some(&backend)).await {
            Ok(result) => result,
            Err(error) => {
                spawn_recovery(state.clone());
                return Err(error);
            }
        };
    if finalized.cleanup_pending {
        spawn_recovery(state.clone());
    }

    let status = if finalized.cleanup_pending {
        "recovery_pending"
    } else if !finalized.server_committed {
        "activation_pending"
    } else {
        "completed"
    };

    Ok(ClientSubdomainTakeoverResponse {
        ok: true,
        takeover_id: plan.id,
        status: status.to_string(),
        target_installation_id: plan.target_installation_id,
        source_installation_id: plan.source_installation_id,
        retired_subdomain: plan.target_original_subdomain,
        adopted_subdomain: plan.adopted_subdomain,
        warning: finalized.warning,
    })
}

async fn lock_client_market_actions(
    state: &ServerState,
    target_installation_id: &str,
    source_installation_id: &str,
) -> (
    tokio::sync::OwnedMutexGuard<()>,
    Option<tokio::sync::OwnedMutexGuard<()>>,
) {
    // Stable ordering prevents opposing A/B takeover requests from deadlocking.
    let target = target_installation_id.trim();
    let source = source_installation_id.trim();
    let (first, second) = if target <= source {
        (target, source)
    } else {
        (source, target)
    };
    let first_guard = state.client_market_actions.lock(first).await;
    let second_guard = if first == second {
        None
    } else {
        Some(state.client_market_actions.lock(second).await)
    };
    (first_guard, second_guard)
}

pub(crate) async fn authorization(
    state: ServerState,
    input: ClientSubdomainTakeoverAuthorizationRequest,
) -> Result<ClientSubdomainTakeoverAuthorizationResponse, AppError> {
    if input.protocol_epoch != crate::namespace::PROTOCOL_EPOCH {
        return Err(AppError::BadRequest("unsupported protocol epoch".into()));
    }
    let result = state
        .store
        .authorize_client_subdomain_takeover_activation(
            &input.installation_id,
            &input.takeover_id,
            input.timestamp_ms,
            &input.nonce,
            &input.signature,
        )
        .await?;
    Ok(ClientSubdomainTakeoverAuthorizationResponse {
        ok: true,
        takeover_id: input.takeover_id,
        authorized: result.authorized,
        status: result.status,
    })
}

fn is_terminal_router_validation_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::BadRequest(_)
            | AppError::Forbidden(_)
            | AppError::NotFound(_)
            | AppError::Conflict(_)
    )
}

async fn settle_terminal_router_commit_error(
    state: &ServerState,
    plan: &ClientSubdomainTakeoverPlan,
    target_backend: Option<&str>,
    error: &AppError,
) -> Result<bool, AppError> {
    if !is_terminal_router_validation_error(error) {
        return Ok(false);
    }
    let failed = state
        .store
        .fail_client_subdomain_takeover(&plan.id, &error.to_string())
        .await?;
    if !failed {
        return Ok(false);
    }
    abort_prepared_target(plan, target_backend).await;
    Ok(true)
}

async fn settle_preparing_failure(
    state: &ServerState,
    plan: &ClientSubdomainTakeoverPlan,
    target_backend: Option<&str>,
    error: &str,
) -> Result<bool, AppError> {
    let failed = match state
        .store
        .fail_client_subdomain_takeover_if_preparing(&plan.id, error)
        .await
    {
        Ok(failed) => failed,
        Err(db_error) => {
            spawn_recovery(state.clone());
            return Err(db_error);
        }
    };
    if failed {
        abort_prepared_target(plan, target_backend).await;
    } else {
        // Another recovery path advanced the durable job. Leave it to the single
        // reconciler instead of aborting a Server adoption that may now be committed.
        spawn_recovery(state.clone());
    }
    Ok(failed)
}

async fn abort_prepared_target(plan: &ClientSubdomainTakeoverPlan, target_backend: Option<&str>) {
    if let Some(backend) = target_backend
        && let Err(abort_error) = crate::ctl_client::abort_client_subdomain_adoption(
            backend,
            &plan.target_installation_id,
            &plan.control_secret,
            &plan.id,
        )
        .await
    {
        warn!(
            takeover_id = %plan.id,
            error = %abort_error,
            "target Client takeover abort was not acknowledged; authorization polling will clear it"
        );
    }
}

fn preparing_recovery_timing(activate_at_ms: i64, now_ms: i64) -> PreparingRecoveryTiming {
    if now_ms < activate_at_ms {
        PreparingRecoveryTiming::NotDue
    } else if now_ms >= activate_at_ms.saturating_add(PREPARING_RECOVERY_WINDOW_MS) {
        PreparingRecoveryTiming::Expired
    } else {
        PreparingRecoveryTiming::Retry
    }
}

async fn active_target_route(
    state: &ServerState,
    plan: &ClientSubdomainTakeoverPlan,
) -> Result<crate::proxy::RouteEntry, AppError> {
    let route = state
        .proxy
        .active_client_route(&plan.target_original_subdomain)
        .await
        .ok_or_else(|| {
            AppError::UnprocessableEntity("target Client must be online before takeover".into())
        })?;
    if route.installation_id() != Some(plan.target_installation_id.as_str()) {
        return Err(AppError::Conflict(
            "target Client route does not match the installation".into(),
        ));
    }
    Ok(route)
}

async fn prepare_server(
    plan: &ClientSubdomainTakeoverPlan,
    backend: &str,
) -> Result<(), crate::ctl_client::CtlError> {
    crate::ctl_client::prepare_client_subdomain_adoption(
        backend,
        &plan.target_installation_id,
        &plan.control_secret,
        &plan.id,
        &plan.target_original_subdomain,
        &plan.adopted_subdomain,
        plan.activate_at_ms,
    )
    .await?;
    Ok(())
}

async fn finalize_committed_takeover(
    state: &ServerState,
    plan: &ClientSubdomainTakeoverPlan,
    committed: &ClientSubdomainTakeoverCommit,
    target_backend: Option<&str>,
) -> Result<FinalizeResult, AppError> {
    for route in &committed.old_routes {
        if route != &committed.target_original_subdomain {
            state.proxy.remove_route(route).await;
        }
    }
    state
        .proxy
        .declare_known_routes(committed.new_routes.iter().cloned())
        .await;

    let mut warnings = Vec::new();
    let server_committed = if let Some(backend) = target_backend {
        match crate::ctl_client::commit_client_subdomain_adoption(
            backend,
            &plan.target_installation_id,
            &plan.control_secret,
            &plan.id,
        )
        .await
        {
            Ok(_) => true,
            Err(control_error) => {
                warnings.push(format!(
                    "explicit target Client commit was not acknowledged; persisted Router authorization polling remains active: {control_error}"
                ));
                false
            }
        }
    } else {
        warnings.push(
            "target Client will finish adoption through persisted Router authorization polling"
                .to_string(),
        );
        false
    };
    state
        .proxy
        .remove_route(&committed.target_original_subdomain)
        .await;

    let cleanup_pending = if let Err(cleanup_error) = crate::client_market::terminate_for_billing(
        state,
        &committed.source_installation_id,
        "client_subdomain_takeover",
    )
    .await
    {
        warnings.push(format!(
            "source Client Market cleanup will require retry: {cleanup_error}"
        ));
        true
    } else {
        false
    };
    let warning = (!warnings.is_empty()).then(|| warnings.join("; "));
    if cleanup_pending {
        state
            .store
            .defer_client_subdomain_takeover_completion(
                &plan.id,
                server_committed,
                warning
                    .as_deref()
                    .unwrap_or("source Client Market cleanup is pending"),
            )
            .await?;
    } else {
        state
            .store
            .complete_client_subdomain_takeover(&plan.id, server_committed, warning.as_deref())
            .await?;
    }
    Ok(FinalizeResult {
        warning,
        cleanup_pending,
        server_committed,
    })
}

pub(crate) fn spawn_recovery(state: ServerState) {
    if state
        .client_subdomain_takeover_recovery_running
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        return;
    }
    let running = state.client_subdomain_takeover_recovery_running.clone();
    tokio::spawn(async move {
        let _guard = RecoveryRunGuard(running);
        loop {
            match reconcile_once(&state).await {
                Ok(0) => return,
                Ok(_) => tokio::time::sleep(RECOVERY_POLL_INTERVAL).await,
                Err(recovery_error) => {
                    error!(error = %recovery_error, "Client subdomain takeover recovery failed");
                    tokio::time::sleep(RECOVERY_POLL_INTERVAL).await;
                }
            }
        }
    });
}

async fn reconcile_once(state: &ServerState) -> Result<usize, AppError> {
    let recoveries = state
        .store
        .list_client_subdomain_takeover_recovery()
        .await?;
    let pending = recoveries.len();
    for recovery in recoveries {
        let takeover_id = recovery.plan.id.clone();
        if let Err(recovery_error) = reconcile_recovery(state, recovery).await {
            warn!(
                takeover_id = %takeover_id,
                error = %recovery_error,
                "Client subdomain takeover recovery remains pending"
            );
        }
    }
    Ok(pending)
}

async fn reconcile_recovery(
    state: &ServerState,
    recovery: ClientSubdomainTakeoverRecovery,
) -> Result<(), AppError> {
    let plan = recovery.plan;
    match recovery.status.as_str() {
        "router_committed" | "server_committed" => {
            let committed = ClientSubdomainTakeoverCommit {
                id: plan.id.clone(),
                target_installation_id: plan.target_installation_id.clone(),
                source_installation_id: plan.source_installation_id.clone(),
                target_original_subdomain: plan.target_original_subdomain.clone(),
                adopted_subdomain: plan.adopted_subdomain.clone(),
                old_routes: recovery.old_routes,
                new_routes: recovery.new_routes,
            };
            finalize_committed_takeover(state, &plan, &committed, None).await?;
        }
        "server_prepared" => {
            let backend = state
                .proxy
                .active_client_route(&plan.target_original_subdomain)
                .await
                .filter(|route| {
                    route.installation_id() == Some(plan.target_installation_id.as_str())
                })
                .map(|route| route.route_target().to_string());
            commit_recovery(state, &plan, backend.as_deref()).await?;
        }
        "preparing" => {
            match preparing_recovery_timing(
                plan.activate_at_ms,
                chrono::Utc::now().timestamp_millis(),
            ) {
                PreparingRecoveryTiming::NotDue => return Ok(()),
                PreparingRecoveryTiming::Expired => {
                    let backend = active_target_route(state, &plan)
                        .await
                        .ok()
                        .map(|route| route.route_target().to_string());
                    settle_preparing_failure(
                        state,
                        &plan,
                        backend.as_deref(),
                        "target Client did not acknowledge takeover prepare before the recovery deadline",
                    )
                    .await?;
                    return Ok(());
                }
                PreparingRecoveryTiming::Retry => {}
            }
            let (first_action_guard, second_action_guard) = lock_client_market_actions(
                state,
                &plan.target_installation_id,
                &plan.source_installation_id,
            )
            .await;
            if let Ok(route) = active_target_route(state, &plan).await {
                let backend = route.route_target().to_string();
                match prepare_server(&plan, &backend).await {
                    Ok(()) => {
                        state
                            .store
                            .mark_client_subdomain_takeover_server_prepared(&plan.id)
                            .await?;
                        drop(second_action_guard);
                        drop(first_action_guard);
                        commit_recovery(state, &plan, Some(&backend)).await?;
                    }
                    Err(control_error) if !control_error.is_transport() => {
                        settle_preparing_failure(
                            state,
                            &plan,
                            Some(&backend),
                            &format!(
                                "target Client rejected takeover recovery prepare: {control_error}"
                            ),
                        )
                        .await?;
                    }
                    Err(control_error) => {
                        warn!(
                            takeover_id = %plan.id,
                            error = %control_error,
                            "Client takeover recovery prepare remains uncertain"
                        );
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparing_recovery_waits_retries_and_expires_at_bounded_deadline() {
        let activate_at_ms = 10_000;
        assert_eq!(
            preparing_recovery_timing(activate_at_ms, activate_at_ms - 1),
            PreparingRecoveryTiming::NotDue
        );
        assert_eq!(
            preparing_recovery_timing(activate_at_ms, activate_at_ms),
            PreparingRecoveryTiming::Retry
        );
        assert_eq!(
            preparing_recovery_timing(
                activate_at_ms,
                activate_at_ms + PREPARING_RECOVERY_WINDOW_MS - 1,
            ),
            PreparingRecoveryTiming::Retry
        );
        assert_eq!(
            preparing_recovery_timing(
                activate_at_ms,
                activate_at_ms + PREPARING_RECOVERY_WINDOW_MS,
            ),
            PreparingRecoveryTiming::Expired
        );
    }
}

async fn commit_recovery(
    state: &ServerState,
    plan: &ClientSubdomainTakeoverPlan,
    target_backend: Option<&str>,
) -> Result<(), AppError> {
    let (first_action_guard, second_action_guard) = lock_client_market_actions(
        state,
        &plan.target_installation_id,
        &plan.source_installation_id,
    )
    .await;
    match state.store.commit_client_subdomain_takeover(&plan.id).await {
        Ok(committed) => {
            drop(second_action_guard);
            drop(first_action_guard);
            finalize_committed_takeover(state, plan, &committed, target_backend).await?;
        }
        Err(error) => {
            if !settle_terminal_router_commit_error(state, plan, target_backend, &error).await? {
                return Err(error);
            }
        }
    }
    Ok(())
}
