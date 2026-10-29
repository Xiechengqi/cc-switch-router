//! Wallet and marketplace notifications.
//!
//! * Buyer/Provider notices go through `market_notification_outbox`, written in
//!   the same transaction as the money event. A worker delivers them by email
//!   and, when the user has a ready Telegram binding, by Telegram. The same
//!   rows back an in-app feed.
//! * Owner alerts reuse `operator_alert_signal_outbox`, which the alerting
//!   service already delivers to the configured admin Telegram/Bark channels.
//!
//! Payloads never contain secrets, full payout addresses or checkout tokens.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;
use uuid::Uuid;

use crate::ServerState;
use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use crate::error::AppError;

const CLAIM_SECS: i64 = 120;
const MAX_ATTEMPTS: i64 = 8;
const BATCH: i64 = 25;
pub(crate) const RESEND_WEBHOOK_PATH: &str = "/v1/webhooks/resend";

pub(crate) fn allow_resend_webhook_ip(ip: std::net::IpAddr) -> bool {
    tracking::allow_webhook_ip(ip)
}

mod delivery;
#[cfg(test)]
mod delivery_tests;
mod diagnostics;
mod email;
mod policy;
mod safety;
#[cfg(test)]
mod safety_tests;
mod tracking;
use policy::notice_current_tx;

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

/// Every notification kind with its audience default and whether a user may
/// opt out. Security and contract-state notices are mandatory.
pub(crate) struct NotificationKind {
    pub kind: &'static str,
    pub mandatory: bool,
    pub subject_en: &'static str,
    pub subject_zh: &'static str,
}

pub(crate) const KINDS: &[NotificationKind] = &[
    NotificationKind {
        kind: "client_termination_claim",
        mandatory: true,
        subject_en: "Client termination compensation claim",
        subject_zh: "Client 退租补偿申诉",
    },
    NotificationKind {
        kind: "client_termination_settled",
        mandatory: true,
        subject_en: "Client rental termination settlement",
        subject_zh: "Client 退租结算",
    },
    NotificationKind {
        kind: "c2c_order_updated",
        mandatory: true,
        subject_en: "C2C order updated",
        subject_zh: "C2C 余额交易状态更新",
    },
    NotificationKind {
        kind: "c2c_seller_changed",
        mandatory: true,
        subject_en: "C2C seller settings updated",
        subject_zh: "C2C 出售设置已更新",
    },
    NotificationKind {
        kind: "earnings_converted",
        mandatory: true,
        subject_en: "Earnings moved to your wallet",
        subject_zh: "收益已转入平台余额",
    },
    NotificationKind {
        kind: "commission_policy_changed",
        mandatory: true,
        subject_en: "Provider service fee updated",
        subject_zh: "Provider 平台服务费已更新",
    },
    NotificationKind {
        kind: "wallet_transfer_sent",
        mandatory: true,
        subject_en: "Balance sent",
        subject_zh: "余额已转出",
    },
    NotificationKind {
        kind: "wallet_transfer_received",
        mandatory: true,
        subject_en: "Balance received",
        subject_zh: "余额已转入",
    },
    NotificationKind {
        kind: "topup_credited",
        mandatory: true,
        subject_en: "Top-up credited",
        subject_zh: "充值已到账",
    },
    NotificationKind {
        kind: "topup_closed_unpaid",
        mandatory: false,
        subject_en: "Top-up closed without payment",
        subject_zh: "充值已关闭（未付款）",
    },
    NotificationKind {
        kind: "topup_review_required",
        mandatory: true,
        subject_en: "Top-up needs review",
        subject_zh: "充值需要人工核对",
    },
    NotificationKind {
        kind: "topup_reversed",
        mandatory: true,
        subject_en: "Top-up reversed",
        subject_zh: "充值已冲正",
    },
    NotificationKind {
        kind: "wallet_low_runway",
        mandatory: false,
        subject_en: "Wallet balance is running low",
        subject_zh: "钱包余额即将不足",
    },
    NotificationKind {
        kind: "renewal_upcoming_shortfall",
        mandatory: false,
        subject_en: "Upcoming renewal needs funds",
        subject_zh: "即将续费但余额不足",
    },
    NotificationKind {
        kind: "contract_suspended",
        mandatory: true,
        subject_en: "Service suspended",
        subject_zh: "服务已暂停",
    },
    NotificationKind {
        kind: "service_status_summary",
        mandatory: true,
        subject_en: "Service status summary",
        subject_zh: "服务状态汇总",
    },
    NotificationKind {
        kind: "contract_resumed",
        mandatory: true,
        subject_en: "Service resumed",
        subject_zh: "服务已恢复",
    },
    NotificationKind {
        kind: "contract_terminating_soon",
        mandatory: true,
        subject_en: "Suspended service will end soon",
        subject_zh: "暂停的服务即将终止",
    },
    NotificationKind {
        kind: "contract_terminated",
        mandatory: true,
        subject_en: "Service ended",
        subject_zh: "服务已终止",
    },
    NotificationKind {
        kind: "dispute_resolved",
        mandatory: true,
        subject_en: "Service dispute resolved",
        subject_zh: "服务争议已处理",
    },
    NotificationKind {
        kind: "earnings_held",
        mandatory: true,
        subject_en: "Earnings placed on hold",
        subject_zh: "收入已冻结",
    },
    NotificationKind {
        kind: "earnings_released",
        mandatory: true,
        subject_en: "Earnings released",
        subject_zh: "收入已解冻",
    },
];

pub(crate) fn kind(kind: &str) -> Option<&'static NotificationKind> {
    KINDS.iter().find(|entry| entry.kind == kind)
}

/// Queue a buyer or Provider notice. `dedupe_key` makes the call idempotent
/// across retries of the surrounding money operation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn enqueue_user_notification_tx(
    conn: &Connection,
    audience: &str,
    recipient_user_id: &str,
    event_kind: &str,
    object_kind: &str,
    object_id: &str,
    dedupe_key: &str,
    payload: Value,
    time: &str,
) -> Result<(), AppError> {
    let Some(entry) = kind(event_kind) else {
        return Err(AppError::Internal(format!(
            "unknown market notification kind {event_kind}"
        )));
    };
    if !matches!(audience, "buyer" | "provider") || recipient_user_id.is_empty() {
        return Err(AppError::Internal(
            "invalid market notification recipient".into(),
        ));
    }
    conn.execute(
        "INSERT OR IGNORE INTO market_notification_outbox
         (id, audience, recipient_user_id, event_kind, object_kind, object_id, dedupe_key,
          payload_json, mandatory, status, next_attempt_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'queued', ?10, ?10)",
        params![
            Uuid::new_v4().to_string(),
            audience,
            recipient_user_id,
            event_kind,
            object_kind,
            object_id,
            dedupe_key,
            payload.to_string(),
            entry.mandatory,
            time
        ],
    )
    .map_err(db_error("enqueue market notification"))?;
    Ok(())
}

struct OwnerSignal {
    severity: &'static str,
    title: &'static str,
}

fn owner_signal(kind: &str) -> OwnerSignal {
    let (severity, title) = match kind {
        "finance_critical_case" => ("critical", "Market finance reconciliation required"),
        "finance_audit_mismatch" => ("critical", "Market finance audit found mismatches"),
        "payment_secret_unreadable" => ("critical", "Payment channel secret cannot be decrypted"),
        "payment_channel_auth_failed" => ("critical", "Payment channel credentials rejected"),
        "topup_reconcile_failing" => ("warning", "Top-up reconciliation keeps failing"),
        "topup_review_stale" => ("warning", "Top-ups need owner review"),
        "gateway_identity_conflict" => ("warning", "Payment gateway identity conflict"),
        "gateway_orphan_paid_order" => ("warning", "Paid gateway order without Router top-up"),
        "gateway_external_refund" => ("warning", "Merchant recorded a gateway refund"),
        "duplicate_external_reference" => ("critical", "Duplicate external transfer reference"),
        "channel_promoted_live" => ("info", "Payment channel promoted to live"),
        "refund_requested" => ("info", "Buyer balance refund requested"),
        "dispute_opened" => ("info", "Service dispute opened"),
        "c2c_dispute" => ("warning", "C2C balance trade needs evidence review"),
        "client_compensation_dispute" => {
            ("warning", "Client termination compensation needs review")
        }
        "client_compensation_dispute_overdue" => (
            "warning",
            "Client termination compensation decision is overdue",
        ),
        "client_cleanup_finalization_failed" => (
            "warning",
            "Client is clean but financial finalization needs review",
        ),
        "earning_hold_legacy" => ("warning", "Earnings hold needs review"),
        "earning_unlock_failed" => ("warning", "Provider earning lot failed to unlock"),
        "service_notice_circuit" => ("warning", "Service notification safety policy activated"),
        _ => ("warning", "Marketplace finance notice"),
    };
    OwnerSignal { severity, title }
}

/// Raise (or, with `resolved`, clear) an Owner alert through the operator
/// alert outbox. The fingerprint groups repeats of the same condition.
#[allow(clippy::too_many_arguments)]
pub(crate) fn owner_signal_tx(
    conn: &Connection,
    kind: &str,
    entity_kind: &str,
    entity_id: &str,
    details: Value,
    time: &str,
    resolved: bool,
) -> Result<(), AppError> {
    let signal = owner_signal(kind);
    let occurred_at = DateTime::parse_from_rfc3339(time)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let fingerprint = format!("market:{kind}:{entity_kind}:{entity_id}");
    let message = match details.get("summary").and_then(Value::as_str) {
        Some(summary) => summary.to_string(),
        None => format!("{} ({entity_kind} {entity_id})", signal.title),
    };
    crate::store::enqueue_operator_alert_signal_tx(
        conn,
        &Uuid::new_v4().to_string(),
        &fingerprint,
        if resolved { "resolved" } else { "firing" },
        &format!("market_{kind}"),
        entity_kind,
        Some(entity_id),
        signal.severity,
        signal.title,
        &message,
        details,
        occurred_at,
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationView {
    id: String,
    audience: String,
    event_kind: String,
    object_kind: String,
    object_id: String,
    payload: Value,
    mandatory: bool,
    read_at: Option<String>,
    created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FeedQuery {
    cursor: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedPage {
    items: Vec<NotificationView>,
    next_cursor: Option<String>,
    unread: i64,
}

fn feed_tx(conn: &Connection, user_id: &str, cursor: Option<&str>) -> Result<FeedPage, AppError> {
    let cursor_at = cursor
        .map(|id| {
            conn.query_row(
                "SELECT created_at FROM market_notification_outbox
                 WHERE id = ?1 AND recipient_user_id = ?2",
                params![id, user_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error("read notification cursor"))?
            .ok_or_else(|| AppError::BadRequest("invalid notification cursor".into()))
        })
        .transpose()?;
    let mut statement = conn
        .prepare(
            "SELECT id, audience, event_kind, object_kind, object_id, payload_json, mandatory,
                    read_at, created_at
             FROM market_notification_outbox WHERE recipient_user_id = ?1
               AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id < ?3))
             ORDER BY created_at DESC, id DESC LIMIT 51",
        )
        .map_err(db_error("prepare notification feed"))?;
    let mut items = statement
        .query_map(params![user_id, cursor_at, cursor], |row| {
            let payload: String = row.get(5)?;
            Ok(NotificationView {
                id: row.get(0)?,
                audience: row.get(1)?,
                event_kind: row.get(2)?,
                object_kind: row.get(3)?,
                object_id: row.get(4)?,
                payload: serde_json::from_str(&payload).unwrap_or_else(|_| json!({})),
                mandatory: row.get::<_, i64>(6)? == 1,
                read_at: row.get(7)?,
                created_at: row.get(8)?,
            })
        })
        .map_err(db_error("query notification feed"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read notification feed"))?;
    let has_more = items.len() > 50;
    items.truncate(50);
    let next_cursor = has_more
        .then(|| items.last().map(|item| item.id.clone()))
        .flatten();
    let unread: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market_notification_outbox
             WHERE recipient_user_id = ?1 AND read_at IS NULL",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(db_error("count unread notifications"))?;
    Ok(FeedPage {
        items,
        next_cursor,
        unread,
    })
}

async fn list_feed(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((
        no_store(),
        Json(feed_tx(&conn, &session.user_id, query.cursor.as_deref())?),
    ))
}

async fn mark_read(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let time = Utc::now().to_rfc3339();
    if id == "all" {
        conn.execute(
            "UPDATE market_notification_outbox SET read_at = ?2
             WHERE recipient_user_id = ?1 AND read_at IS NULL",
            params![session.user_id, time],
        )
        .map_err(db_error("mark notifications read"))?;
    } else {
        conn.execute(
            "UPDATE market_notification_outbox SET read_at = COALESCE(read_at, ?3)
             WHERE id = ?1 AND recipient_user_id = ?2",
            params![id, session.user_id, time],
        )
        .map_err(db_error("mark notification read"))?;
    }
    Ok((no_store(), Json(json!({ "ok": true }))))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreferenceView {
    event_kind: &'static str,
    mandatory: bool,
    enabled: bool,
}

fn preferences_tx(conn: &Connection, user_id: &str) -> Result<Vec<PreferenceView>, AppError> {
    KINDS
        .iter()
        .filter(|entry| !matches!(entry.kind, "topup_closed_unpaid" | "earnings_converted"))
        .map(|entry| {
            let enabled = if entry.mandatory {
                true
            } else {
                conn.query_row(
                    "SELECT enabled FROM market_notification_preferences
                     WHERE user_id = ?1 AND event_kind = ?2",
                    params![user_id, entry.kind],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(db_error("read notification preference"))?
                .is_none_or(|value| value == 1)
            };
            Ok(PreferenceView {
                event_kind: entry.kind,
                mandatory: entry.mandatory,
                enabled,
            })
        })
        .collect()
}

async fn get_preferences(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    Ok((no_store(), Json(preferences_tx(&conn, &session.user_id)?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreferenceUpdate {
    event_kind: String,
    enabled: bool,
}

async fn put_preferences(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<Vec<PreferenceUpdate>>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    if input.len() > KINDS.len() {
        return Err(AppError::BadRequest("too many preference updates".into()));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin notification preferences"))?;
    let time = Utc::now().to_rfc3339();
    for update in &input {
        let entry = kind(&update.event_kind)
            .ok_or_else(|| AppError::BadRequest("unknown notification kind".into()))?;
        if entry.mandatory && !update.enabled {
            return Err(AppError::BadRequest(
                "security and service-state notices cannot be disabled".into(),
            ));
        }
        tx.execute(
            "INSERT INTO market_notification_preferences (user_id, event_kind, enabled, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id, event_kind) DO UPDATE SET enabled = excluded.enabled,
                 updated_at = excluded.updated_at",
            params![session.user_id, entry.kind, update.enabled, time],
        )
        .map_err(db_error("save notification preference"))?;
    }
    let view = preferences_tx(&tx, &session.user_id)?;
    tx.commit()
        .map_err(db_error("commit notification preferences"))?;
    Ok((no_store(), Json(view)))
}

async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::models::AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}

fn no_store() -> [(axum::http::header::HeaderName, &'static str); 1] {
    [(axum::http::header::CACHE_CONTROL, "private, no-store")]
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .merge(tracking::router())
        .merge(diagnostics::router())
        .route("/v1/market-notifications", get(list_feed))
        .route("/v1/market-notifications/:id/read", post(mark_read))
        .route(
            "/v1/market-notifications/preferences",
            get(get_preferences).put(put_preferences),
        )
}

struct Claimed {
    id: String,
    recipient_user_id: String,
    event_kind: String,
    payload: Value,
    mandatory: bool,
    attempts: i64,
}

fn claim_tx(conn: &Connection, worker: &str, now: &str) -> Result<Vec<Claimed>, AppError> {
    let at = DateTime::parse_from_rfc3339(now)
        .map_err(|_| AppError::Internal("invalid notification claim time".into()))?;
    let expires = (at + chrono::Duration::seconds(CLAIM_SECS)).to_rfc3339();
    let ids = {
        let mut statement = conn
            .prepare(
                "SELECT id FROM market_notification_outbox
                 WHERE (status = 'queued' AND next_attempt_at <= ?1)
                    OR (status = 'claimed' AND claim_expires_at <= ?1)
                 ORDER BY CASE WHEN event_kind IN ('contract_suspended','contract_resumed',
                    'contract_terminating_soon','contract_terminated','wallet_low_runway',
                    'renewal_upcoming_shortfall','service_status_summary') THEN 1 ELSE 0 END,
                    next_attempt_at, id LIMIT ?2",
            )
            .map_err(db_error("prepare notification claim"))?;
        statement
            .query_map(params![now, 1], |row| row.get::<_, String>(0))
            .map_err(db_error("query notification claim"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read notification claim"))?
    };
    let mut claimed = Vec::new();
    for id in ids {
        let changed = conn
            .execute(
                "UPDATE market_notification_outbox
                 SET status = 'claimed', claimed_by = ?2, claim_expires_at = ?3,
                     attempts = attempts + 1
                 WHERE id = ?1 AND ((status = 'queued' AND next_attempt_at <= ?4)
                    OR (status = 'claimed' AND claim_expires_at <= ?4))",
                params![id, worker, expires, now],
            )
            .map_err(db_error("claim notification"))?;
        if changed != 1 {
            continue;
        }
        let row = conn
            .query_row(
                "SELECT recipient_user_id, event_kind, payload_json, mandatory, attempts, dedupe_key, created_at, object_id
                 FROM market_notification_outbox WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .map_err(db_error("read claimed notification"))?;
        let mut payload: Value = serde_json::from_str(&row.2).unwrap_or_else(|_| json!({}));
        payload["notificationEventKey"] = json!(row.5);
        payload["notificationTime"] = json!(row.6);
        payload["notificationObjectId"] = json!(row.7);
        claimed.push(Claimed {
            id,
            recipient_user_id: row.0,
            event_kind: row.1,
            payload,
            mandatory: row.3 == 1,
            attempts: row.4,
        });
    }
    Ok(claimed)
}

struct Recipient {
    email: Option<String>,
    telegram_chat: Option<String>,
    opted_out: bool,
}

fn recipient_tx(conn: &Connection, item: &Claimed) -> Result<Recipient, AppError> {
    let email: Option<String> = conn
        .query_row(
            "SELECT email_normalized FROM users WHERE id = ?1 AND status='active' AND email_verified_at IS NOT NULL",
            params![item.recipient_user_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read notification recipient"))?
        .flatten();
    let telegram_chat: Option<String> = conn
        .query_row(
            "SELECT target FROM user_notification_channels
             WHERE user_id = ?1 AND channel = 'telegram' AND enabled = 1 AND state = 'ready'
             AND EXISTS(SELECT 1 FROM users WHERE id=?1 AND status='active' AND email_verified_at IS NOT NULL)",
            params![item.recipient_user_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read notification telegram binding"))?
        .flatten();
    let opted_out = !kind(&item.event_kind).is_some_and(|kind| kind.mandatory)
        && conn
            .query_row(
                "SELECT enabled FROM market_notification_preferences
                 WHERE user_id = ?1 AND event_kind = ?2",
                params![item.recipient_user_id, item.event_kind],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(db_error("read notification preference"))?
            .is_some_and(|enabled| enabled == 0);
    Ok(Recipient {
        email,
        telegram_chat,
        opted_out,
    })
}

fn amount_text(payload: &Value) -> Option<String> {
    email::amount(payload)
}

/// Short bilingual text. It names the state change and the amount; detail
/// stays behind the authenticated wallet page link.
fn render(item: &Claimed, dashboard: &str) -> (String, String) {
    let entry = kind(&item.event_kind);
    let subject_en = entry.map_or("Wallet update", |entry| entry.subject_en);
    let subject_zh = entry.map_or("钱包通知", |entry| entry.subject_zh);
    let amount = amount_text(&item.payload);
    let status = item
        .payload
        .get("status")
        .and_then(Value::as_str)
        .map(|status| format!(" · {status}"))
        .unwrap_or_default();
    let mut detail = match &amount {
        Some(amount) => format!("{amount}{status}"),
        None => status.trim_start_matches(" · ").to_string(),
    };
    if item.event_kind == "commission_policy_changed" {
        let rate = item
            .payload
            .get("rateBps")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let effective = item
            .payload
            .get("effectiveAt")
            .and_then(Value::as_str)
            .unwrap_or("—");
        let subject = format!("{subject_zh} / {subject_en}");
        let body = format!(
            "{subject_zh}：{}.{:02}%；生效时间：{effective}。仅适用于新的计费周期，已承诺周期不变；收益自动到账不再扣费。\n{subject_en}: {}.{:02}%. Effective: {effective}. Applies to new service periods only; committed periods stay unchanged. Automatic income credit has no additional service fee.\n\n{}/account/provider-settings/?tab=income",
            rate / 100,
            rate % 100,
            rate / 100,
            rate % 100,
            dashboard.trim_end_matches('/')
        );
        return (subject, body);
    }
    if item.event_kind == "c2c_seller_changed" {
        let change = match item.payload.get("reason").and_then(Value::as_str) {
            Some("auto_accept_consent_required") => {
                "C2C 新订单改为下单即自动接单并冻结余额。请在平台余额页明确确认新规则后继续出售；确认前不会展示挂售，已有订单仍按原规则处理。"
            }
            Some("acceptance_timeout") => {
                "有购入申请超时未接单，已暂停接收新申请。请在平台余额页检查后手动恢复出售。"
            }
            Some("paid_order_overdue") => {
                "有已标记付款的订单超过 60 分钟未处理，已暂停新申请。已有订单的余额仍受保护，请先处理订单。"
            }
            _ => "C2C 出售设置已更新，请在平台余额页核对。",
        };
        let change_en = match item.payload.get("reason").and_then(Value::as_str) {
            Some("auto_accept_consent_required") => {
                "New orders automatically reserve your credit. Explicitly confirm the new automatic-order terms in your wallet to list again. Existing orders keep their original rules; previous pauses remain in effect."
            }
            Some("acceptance_timeout") => {
                "A historical application expired without acceptance. New orders are paused; review existing orders and explicitly resume selling."
            }
            Some("paid_order_overdue") => {
                "A payment-declared order has waited over 60 minutes. New orders are paused; handle the order first. Reserved credit stays protected."
            }
            _ => "C2C seller settings changed. Review your wallet.",
        };
        return (
            format!("{subject_zh} / {subject_en}"),
            format!(
                "{change} 已有订单仍需处理；平台不能自动核实线下付款。\n{change_en} Existing orders remain actionable. Offline payments are not verified by the platform.\n\n{}/account/wallet/",
                dashboard.trim_end_matches('/')
            ),
        );
    }
    if item.event_kind == "c2c_order_updated" {
        let id = item
            .payload
            .get("orderId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let status = item
            .payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("");
        let zh_status = match status {
            "awaiting_seller" => "等待卖家接单，请勿付款",
            "awaiting_payment" => "待付款",
            "paid" => "买家已标记付款，待卖家核实",
            "disputed" => "争议处理中",
            "released" => "余额已放行",
            "cancelled" => "已取消",
            "expired" => "已超时",
            _ => "设置已更新",
        };
        let role = item
            .payload
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("");
        let instruction = match (status, role) {
            ("awaiting_seller", "seller") => {
                "有新的购入申请。请登录平台余额页，在接单期限内接单或拒绝；超时将暂停新申请。"
            }
            ("awaiting_payment", "buyer") => {
                "订单已就绪，卖家余额已冻结。请登录订单查看付款信息和剩余时间；页面显示订单已关闭时请勿付款。"
            }
            ("awaiting_payment", "seller") => {
                "有待付款订单，余额已冻结。请留意付款通知，亲自核实实际到账后再放行；不需要另行接单。"
            }
            ("paid", "seller") => "买家已标记付款。请亲自核实收款后再放行余额；平台不会自动放行。",
            ("disputed", _) => "请在订单中补充证据，等待独立处理。争议中的余额不会自动解冻。",
            _ => "请登录订单查看当前状态，以页面为准。",
        };
        let instruction_en = match (status, role) {
            ("awaiting_seller", "seller") => {
                "Review this historical application and accept or decline before its response deadline. Do not request payment before acceptance."
            }
            ("awaiting_seller", _) => {
                "Do not pay yet. This historical application still requires seller acceptance."
            }
            ("awaiting_payment", "buyer") => {
                "Credit is reserved. Check payment instructions and time remaining in the order; pay only during its active payment window."
            }
            ("awaiting_payment", "seller") => {
                "Credit is reserved for a new order. No separate acceptance is needed. Wait for payment, then verify actual receipt before releasing credit."
            }
            ("paid", "seller") => {
                "The buyer declared payment. Verify actual receipt in your payment account before releasing credit; release is never automatic."
            }
            ("disputed", _) => {
                "Add evidence in the order for independent review. Reserved credit does not expire automatically."
            }
            _ => "Review the current order before taking action.",
        };
        let destination = if role == "handler" {
            format!("settings/?panel=market_finance&c2cOrder={id}")
        } else {
            format!("account/wallet/?c2cOrder={id}")
        };
        return (
            format!("{subject_zh} / {subject_en}"),
            format!(
                "{subject_zh}：{zh_status}。{instruction} 金额：{}。平台不能自动核实线下付款，请在订单内核对。请勿分享登录凭据。\n{subject_en}: {status}. {instruction_en} Offline payment is not verified by the platform. Never share sign-in credentials.\n\n{}/{destination}",
                amount.as_deref().unwrap_or("—"),
                dashboard.trim_end_matches('/')
            ),
        );
    }
    if item.event_kind == "earnings_converted" {
        let id = item
            .payload
            .get("conversionId")
            .and_then(Value::as_str)
            .unwrap_or("—");
        let amount = amount.as_deref().unwrap_or("—");
        return (
            format!("{subject_zh} / {subject_en}"),
            format!(
                "{subject_zh}：{amount}。可消费或转出，不可提现或转回收益。转换编号：{id}。\n\
             {subject_en}: {amount}. Available for services or sending to other users, not withdrawal or conversion back to earnings. Reference: {id}.\n\n{}/account/wallet/",
                dashboard.trim_end_matches('/')
            ),
        );
    }
    if item.event_kind == "wallet_transfer_sent" || item.event_kind == "wallet_transfer_received" {
        let counterparty = item
            .payload
            .get(if item.event_kind == "wallet_transfer_sent" {
                "recipientEmail"
            } else {
                "senderEmail"
            })
            .and_then(Value::as_str)
            .unwrap_or("");
        detail.push_str(&format!(" · {counterparty}"));
    }
    let subject = format!("{subject_zh} / {subject_en}");
    let body = format!(
        "{subject_zh}{}\n{subject_en}{}\n\n{dashboard}/account/wallet/",
        if detail.is_empty() {
            String::new()
        } else {
            format!("：{detail}")
        },
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        },
    );
    (subject, body)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn render_email(item: &Claimed, dashboard: &str) -> (String, String, String) {
    email::render(item, dashboard)
}

fn c2c_notice_current_tx(conn: &Connection, item: &Claimed, time: &str) -> Result<bool, AppError> {
    if item.event_kind == "c2c_seller_changed" {
        match item.payload.get("reason").and_then(Value::as_str) {
            Some("acceptance_upgrade") => return Ok(false),
            Some(reason @ ("suspended" | "unsuspended")) => return conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM market_c2c_sellers WHERE user_id=?1 AND suspended=?2)",
                params![item.recipient_user_id,reason=="suspended"],|r|r.get(0)).map_err(db_error("recheck C2C suspension notice")),
            Some("auto_accept_consent_required") => return conn.query_row("SELECT EXISTS(SELECT 1 FROM market_c2c_sellers WHERE user_id=?1 AND enabled=1 AND (auto_accept_terms_version!=3 OR auto_accept_consented_at IS NULL))", params![item.recipient_user_id], |r| r.get(0)).map_err(db_error("recheck automatic C2C consent notice")),
            _ => {}
        }
        if let Some(reason @ ("acceptance_timeout" | "paid_order_overdue")) =
            item.payload.get("reason").and_then(Value::as_str)
        {
            return conn.query_row("SELECT EXISTS(SELECT 1 FROM market_c2c_sellers WHERE user_id=?1 AND enabled=1 AND auto_pause_reason=?2)",params![item.recipient_user_id,reason],|r|r.get(0)).map_err(db_error("recheck C2C pause notice"));
        }
        return Ok(true);
    }
    let Some(id) = item.payload.get("orderId").and_then(Value::as_str) else {
        return Ok(false);
    };
    let Some(status) = item.payload.get("status").and_then(Value::as_str) else {
        return Ok(false);
    };
    let handler = item.payload.get("role").and_then(Value::as_str) == Some("handler");
    conn.query_row("SELECT EXISTS(SELECT 1 FROM market_c2c_orders WHERE id=?1 AND status=?2 AND ((?5=0 AND (buyer_id=?3 OR seller_id=?3)) OR (?5=1 AND buyer_id!=?3 AND seller_id!=?3 AND EXISTS(SELECT 1 FROM market_c2c_dispute_handlers WHERE user_id=?3 AND enabled=1)))
        AND EXISTS(SELECT 1 FROM users u WHERE u.id=?3 AND u.status='active' AND u.email_verified_at IS NOT NULL)
        AND (status!='awaiting_seller' OR julianday(acceptance_deadline)>julianday(?4))
        AND (status!='awaiting_payment' OR julianday(payment_deadline)>julianday(?4)))",params![id,status,item.recipient_user_id,time,handler],|r|r.get(0)).map_err(db_error("recheck C2C actionable notice"))
}

fn finish_tx(
    conn: &Connection,
    item: &Claimed,
    result: &Result<String, crate::notifications::DeliveryFailure>,
    now: &str,
) -> Result<(), AppError> {
    match result {
        Ok(channels) => {
            // A receipt may have succeeded before a sibling was retried and
            // superseded. Keep that historical delivery visible in the total.
            let mut outcomes = channels.split(',').map(str::to_string).collect::<Vec<_>>();
            let mut statement = conn
                .prepare(
                    "SELECT channel,status FROM market_notification_deliveries
                 WHERE event_id=?1 AND status IN ('sent','failed','uncertain')",
                )
                .map_err(db_error("prepare completed notification channels"))?;
            let rows = statement
                .query_map(params![item.id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(db_error("read completed notification channels"))?;
            for row in rows {
                let (channel, status) =
                    row.map_err(db_error("read notification channel status"))?;
                if !outcomes
                    .iter()
                    .any(|outcome| outcome.starts_with(&format!("{channel}:")))
                {
                    outcomes.push(format!("{channel}:{status}"));
                }
            }
            let channels = outcomes.join(",");
            let sent = channels
                .split(',')
                .any(|channel| channel.ends_with(":sent"));
            let failed = channels
                .split(',')
                .any(|channel| channel.ends_with(":failed") || channel.ends_with(":uncertain"));
            let status = if sent {
                "sent"
            } else if failed {
                "failed"
            } else {
                "suppressed"
            };
            conn.execute(
                "UPDATE market_notification_outbox
                 SET status = ?5, delivered_channels = ?2,
                     sent_at = CASE WHEN ?5='sent' THEN ?3 ELSE NULL END, claimed_by = NULL,
                     claim_expires_at = NULL, last_error = NULL
                 WHERE id = ?1 AND status='claimed' AND attempts=?4",
                params![item.id, channels, now, item.attempts, status],
            )
        }
        Err(error) => {
            let at = DateTime::parse_from_rfc3339(now)
                .map_err(|_| AppError::Internal("invalid notification finish time".into()))?
                .with_timezone(&Utc);
            let delay = 60_i64 * 2_i64.pow(item.attempts.clamp(0, 8) as u32);
            let next = error
                .retry_at
                .unwrap_or(at + chrono::Duration::seconds(delay.min(21_600)))
                .max(at + chrono::Duration::seconds(1))
                .to_rfc3339();
            // Channel tasks own their bounded retry counts. Do not exhaust the
            // aggregate event while a sibling task is leased or rate-limited.
            conn.execute(
                "UPDATE market_notification_outbox
                 SET status = CASE WHEN ?4=0 THEN 'failed' ELSE 'queued' END,
                     next_attempt_at = ?3, last_error = ?2, claimed_by = NULL,
                     claim_expires_at = NULL
                 WHERE id = ?1 AND status='claimed' AND attempts=?5",
                params![
                    item.id,
                    crate::notifications::sanitize_delivery_error(&error.message),
                    next,
                    error.retryable,
                    item.attempts
                ],
            )
        }
    }
    .map_err(db_error("finish market notification"))?;
    Ok(())
}

pub async fn run_service(
    state: ServerState,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("cc-switch-router/0.1 market-notifications")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()?;
    let worker = format!("market-notify-{}", Uuid::new_v4());
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reminders = tokio::time::interval(Duration::from_secs(3_600));
    reminders.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut digests = tokio::time::interval(Duration::from_secs(60));
    digests.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = digests.tick() => {
                let conn = state.store.conn.lock().await;
                let result = conn.transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(db_error("begin service digest queue"))
                    .and_then(|tx| {
                        safety::queue_digests_tx(&tx, &Utc::now().to_rfc3339(), &state.config.service_notice_limits)?;
                        tx.commit().map_err(db_error("commit service digests"))
                    });
                if result.is_err() { tracing::warn!("service digest queue deferred"); }
            }
            _ = reminders.tick() => {
                let conn = state.store.conn.lock().await;
                let result = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(db_error("begin funding reminders"))
                    .and_then(|tx| {
                        let queued = crate::market_platform_contracts::queue_funding_reminders_tx(&tx, Utc::now())?;
                        tx.commit().map_err(db_error("commit funding reminders"))?;
                        Ok(queued)
                    });
                if result.is_err() {
                    tracing::warn!("market funding reminders deferred");
                }
            }
            _ = interval.tick() => {
                // Claim immediately before I/O. A batch of 25 serial deliveries
                // used to outlive the lease of the later events in the batch.
                for _ in 0..BATCH {
                    if *shutdown.borrow() { break; }
                    let now = Utc::now().to_rfc3339();
                    let claimed = {
                        let conn = state.store.conn.lock().await;
                        claim_tx(&conn, &worker, &now)
                    };
                    let item = match claimed {
                        Ok(mut claimed) if !claimed.is_empty() => claimed.remove(0),
                        Ok(_) => break,
                        Err(_) => {
                            tracing::warn!("market notification queue unavailable");
                            break;
                        }
                    };
                    let result = delivery::deliver(&state, &http, &item).await;
                    let conn = state.store.conn.lock().await;
                    if finish_tx(&conn, &item, &result, &Utc::now().to_rfc3339()).is_err() {
                        tracing::warn!("market notification result could not be recorded");
                    }
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
        }
    }
    Ok(())
}

pub async fn run_tracking_service(
    state: ServerState,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    tracking::run_service(state, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        crate::test_support::database().expect("copy notification schema")
    }

    #[test]
    fn c2c_delivery_discards_stale_instructions_and_revoked_or_conflicting_handlers() {
        // Only notification projections are needed here; wallet accounting
        // and the real migration are covered by the C2C/schema suites.
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE users(id TEXT,status TEXT,email_verified_at TEXT);
            CREATE TABLE market_c2c_dispute_handlers(user_id TEXT,enabled INTEGER);
            CREATE TABLE market_c2c_sellers(user_id TEXT,enabled INTEGER,auto_pause_reason TEXT,auto_accept_terms_version INTEGER,auto_accept_consented_at TEXT);
            CREATE TABLE market_c2c_orders(id TEXT,buyer_id TEXT,seller_id TEXT,status TEXT,acceptance_deadline TEXT,payment_deadline TEXT);
            INSERT INTO users VALUES('buyer','active','verified'),('seller','active','verified'),('handler','active','verified');
            INSERT INTO market_c2c_orders VALUES('order','buyer','seller','awaiting_seller','2026-10-22T12:10:00Z',NULL);
            INSERT INTO market_c2c_dispute_handlers VALUES('handler',1),('seller',1);
            INSERT INTO market_c2c_sellers VALUES('seller',1,'acceptance_timeout',0,NULL);").unwrap();
        let mut item = Claimed {
            id: "notice".into(),
            recipient_user_id: "seller".into(),
            event_kind: "c2c_order_updated".into(),
            payload: json!({"orderId":"order","status":"awaiting_seller","role":"seller"}),
            mandatory: true,
            attempts: 3,
        };
        assert!(c2c_notice_current_tx(&c, &item, "2026-10-22T12:09:59Z").unwrap());
        assert!(!c2c_notice_current_tx(&c, &item, "2026-10-22T12:10:00Z").unwrap());
        c.execute("UPDATE market_c2c_orders SET status='awaiting_payment',payment_deadline='2026-10-22T12:35:00Z'",[]).unwrap();
        assert!(!c2c_notice_current_tx(&c, &item, "2026-10-22T12:09:59Z").unwrap());
        item.recipient_user_id = "buyer".into();
        item.payload = json!({"orderId":"order","status":"awaiting_payment","role":"buyer"});
        assert!(c2c_notice_current_tx(&c, &item, "2026-10-22T12:34:59Z").unwrap());
        assert!(!c2c_notice_current_tx(&c, &item, "2026-10-22T12:35:00Z").unwrap());
        c.execute("UPDATE market_c2c_orders SET status='disputed'", [])
            .unwrap();
        item.recipient_user_id = "handler".into();
        item.payload = json!({"orderId":"order","status":"disputed","role":"handler"});
        assert!(c2c_notice_current_tx(&c, &item, "2026-10-22T13:00:00Z").unwrap());
        c.execute(
            "UPDATE market_c2c_dispute_handlers SET enabled=0 WHERE user_id='handler'",
            [],
        )
        .unwrap();
        assert!(!c2c_notice_current_tx(&c, &item, "2026-10-22T13:00:00Z").unwrap());
        c.execute(
            "UPDATE market_c2c_dispute_handlers SET enabled=1 WHERE user_id='handler'",
            [],
        )
        .unwrap();
        c.execute("UPDATE users SET status='blocked' WHERE id='handler'", [])
            .unwrap();
        assert!(!c2c_notice_current_tx(&c, &item, "2026-10-22T13:00:00Z").unwrap());
        item.recipient_user_id = "seller".into();
        assert!(
            !c2c_notice_current_tx(&c, &item, "2026-10-22T13:00:00Z").unwrap(),
            "even a granted handler cannot receive management instructions for their own order"
        );
        item.event_kind = "c2c_seller_changed".into();
        item.payload = json!({"reason":"acceptance_timeout"});
        assert!(c2c_notice_current_tx(&c, &item, "2026-10-22T13:00:00Z").unwrap());
        c.execute("UPDATE market_c2c_sellers SET auto_pause_reason=NULL", [])
            .unwrap();
        assert!(!c2c_notice_current_tx(&c, &item, "2026-10-22T13:00:00Z").unwrap());
        item.payload = json!({"reason":"acceptance_upgrade"});
        assert!(!c2c_notice_current_tx(&c, &item, "2026-10-22T13:00:00Z").unwrap());
        item.payload = json!({"reason":"auto_accept_consent_required"});
        assert!(c2c_notice_current_tx(&c, &item, "2026-10-22T13:00:00Z").unwrap());
        c.execute("UPDATE market_c2c_sellers SET auto_accept_terms_version=3,auto_accept_consented_at='2026-10-22T13:00:00Z'",[]).unwrap();
        assert!(!c2c_notice_current_tx(&c, &item, "2026-10-22T13:00:00Z").unwrap());
    }

    #[test]
    fn c2c_acceptance_notices_use_role_specific_actions_without_external_payment_instructions() {
        let item = Claimed {
            id: "notice".into(),
            recipient_user_id: "seller".into(),
            event_kind: "c2c_order_updated".into(),
            payload: json!({"orderId":"order","status":"awaiting_seller","role":"seller"}),
            mandatory: true,
            attempts: 1,
        };
        let (_, text) = render(&item, "https://router.example");
        assert!(text.contains("接单或拒绝"));
        assert!(text.contains("请勿付款"));
        let handler = Claimed {
            payload: json!({"orderId":"order","status":"disputed","role":"handler"}),
            ..item
        };
        let (_, text) = render(&handler, "https://router.example");
        assert!(text.ends_with("/settings/?panel=market_finance&c2cOrder=order"));
    }

    #[test]
    fn c2c_automatic_notices_never_require_another_seller_acceptance() {
        let mut item = Claimed {
            id: "notice".into(),
            recipient_user_id: "buyer".into(),
            event_kind: "c2c_order_updated".into(),
            payload: json!({"orderId":"order","status":"awaiting_payment","role":"buyer"}),
            mandatory: true,
            attempts: 1,
        };
        let (_, text) = render(&item, "https://router.example");
        assert!(text.contains("卖家余额已冻结"));
        assert!(!text.contains("卖家已接单") && !text.contains("acceptance is required"));
        item.payload = json!({"orderId":"order","status":"awaiting_payment","role":"seller"});
        let (_, text) = render(&item, "https://router.example");
        assert!(text.contains("不需要另行接单") && text.contains("核实实际到账"));
        assert!(text.contains("No separate acceptance is needed"));
        item.event_kind = "c2c_seller_changed".into();
        item.payload = json!({"reason":"auto_accept_consent_required"});
        let (_, text) = render(&item, "https://router.example");
        assert!(text.contains("明确确认新规则") && text.contains("已有订单仍按原规则处理"));
        assert!(text.contains("Explicitly confirm the new automatic-order terms"));
    }

    #[test]
    fn enqueue_is_idempotent_and_rejects_unknown_kinds() {
        let conn = memory();
        for _ in 0..2 {
            enqueue_user_notification_tx(
                &conn,
                "buyer",
                "buyer-1",
                "topup_credited",
                "topup",
                "order-1",
                "topup_credited:order-1",
                json!({ "amountMinor": 2000 }),
                "2026-10-21T00:00:00Z",
            )
            .unwrap();
        }
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_notification_outbox",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert!(
            enqueue_user_notification_tx(
                &conn,
                "buyer",
                "buyer-1",
                "nope",
                "topup",
                "o",
                "k",
                json!({}),
                "2026-10-21T00:00:00Z"
            )
            .is_err()
        );
        let feed = feed_tx(&conn, "buyer-1", None).unwrap();
        assert_eq!(feed.unread, 1);
        assert_eq!(feed.items[0].event_kind, "topup_credited");
        assert!(feed_tx(&conn, "buyer-2", None).unwrap().items.is_empty());
    }

    #[test]
    fn owner_signals_reuse_the_operator_alert_outbox() {
        let conn = memory();
        owner_signal_tx(
            &conn,
            "finance_critical_case",
            "wallet",
            "user-1",
            json!({}),
            "2026-10-21T00:00:00Z",
            false,
        )
        .unwrap();
        let (kind, severity): (String, String) = conn
            .query_row(
                "SELECT kind, severity FROM operator_alert_signal_outbox",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, "market_finance_critical_case");
        assert_eq!(severity, "critical");
    }
}
