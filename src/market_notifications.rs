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
        subject_en: "Wallet transfer sent",
        subject_zh: "余额已转出",
    },
    NotificationKind {
        kind: "wallet_transfer_received",
        mandatory: true,
        subject_en: "Wallet transfer received",
        subject_zh: "余额已转入",
    },
    NotificationKind {
        kind: "topup_credited",
        mandatory: false,
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
        mandatory: false,
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
        kind: "refund_status",
        mandatory: true,
        subject_en: "Balance refund updated",
        subject_zh: "余额退款状态更新",
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
        "earning_hold_legacy" => ("warning", "Earnings hold needs review"),
        "earning_unlock_failed" => ("warning", "Provider earning lot failed to unlock"),
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
    let expires = (Utc::now() + chrono::Duration::seconds(CLAIM_SECS)).to_rfc3339();
    let ids = {
        let mut statement = conn
            .prepare(
                "SELECT id FROM market_notification_outbox
                 WHERE (status = 'queued' AND next_attempt_at <= ?1)
                    OR (status = 'claimed' AND claim_expires_at <= ?1)
                 ORDER BY next_attempt_at, id LIMIT ?2",
            )
            .map_err(db_error("prepare notification claim"))?;
        statement
            .query_map(params![now, BATCH], |row| row.get::<_, String>(0))
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
                "SELECT recipient_user_id, event_kind, payload_json, mandatory, attempts
                 FROM market_notification_outbox WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .map_err(db_error("read claimed notification"))?;
        claimed.push(Claimed {
            id,
            recipient_user_id: row.0,
            event_kind: row.1,
            payload: serde_json::from_str(&row.2).unwrap_or_else(|_| json!({})),
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
            "SELECT email_normalized FROM users WHERE id = ?1",
            params![item.recipient_user_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read notification recipient"))?
        .flatten();
    let telegram_chat: Option<String> = conn
        .query_row(
            "SELECT target FROM user_notification_channels
             WHERE user_id = ?1 AND channel = 'telegram' AND enabled = 1 AND state = 'ready'",
            params![item.recipient_user_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("read notification telegram binding"))?
        .flatten();
    let opted_out = !item.mandatory
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
    if let Some(units) = payload
        .get("amountUnits")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<i64>().ok())
    {
        return Some(format!(
            "${:.8} ({} ledger units)",
            units as f64 / 8_640_000.0,
            units
        ));
    }
    payload
        .get("amountMinor")
        .and_then(Value::as_i64)
        .map(|minor| format!("${}.{:02}", minor / 100, minor % 100))
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
            "{subject_zh}：{}.{:02}%；生效时间：{effective}。仅适用于新的计费周期，已承诺周期不变；收益自动到账不再扣费。\n{subject_en}: {}.{:02}%. Effective: {effective}. Applies to new service periods only; committed periods stay unchanged. Automatic income credit has no additional service fee.\n\n{}/account/earnings/",
            rate / 100,
            rate % 100,
            rate / 100,
            rate % 100,
            dashboard.trim_end_matches('/')
        );
        return (subject, body);
    }
    if item.event_kind == "c2c_seller_changed" {
        return (
            format!("{subject_zh} / {subject_en}"),
            format!(
                "C2C 出售设置已更新，请在平台余额页核对。已有订单仍需处理；平台不能自动核实线下付款。\nC2C seller settings changed. Review your wallet; existing orders remain actionable. Offline payments are not verified by the platform.\n\n{}/account/wallet/",
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
            "awaiting_payment" => "待付款",
            "paid" => "买家已标记付款，待卖家核实",
            "disputed" => "争议处理中",
            "released" => "余额已放行",
            "cancelled" => "已取消",
            "expired" => "已超时",
            _ => "设置已更新",
        };
        return (
            format!("{subject_zh} / {subject_en}"),
            format!(
                "{subject_zh}：{zh_status}。金额：{}。平台不能自动核实线下付款，请在订单内核对；请勿向他人透露放行验证码。\n{subject_en}: {status}. Offline payment is not verified by the platform. Review the order; never share release codes.\n\n{}/account/wallet/?c2cOrder={id}",
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
                "{subject_zh}：{amount}。可消费或转账，不可提现或转回收益。转换编号：{id}。\n\
             {subject_en}: {amount}. Available for services or wallet transfers, not withdrawal or conversion back to earnings. Reference: {id}.\n\n{}/account/wallet/",
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

/// Top-up emails use the same English document and branding as sign-in codes.
/// Other delivery channels retain their existing bilingual messages.
fn render_email(item: &Claimed, dashboard: &str) -> (String, String, String) {
    let (introduction, status) = match item.event_kind.as_str() {
        "topup_credited" => (
            "Your top-up has been credited to your platform wallet. Your balance is ready to use.",
            "Credited",
        ),
        "topup_closed_unpaid" => (
            "This top-up was closed without a confirmed payment. No funds were added to your wallet.",
            "Closed without payment",
        ),
        "topup_review_required" => (
            "The payment for this top-up needs to be checked. Do not pay for this order again. Check your wallet for updates or contact your Router administrator.",
            "Payment under review",
        ),
        "topup_reversed" => (
            "An external refund has been recorded for this top-up. The corresponding wallet credit has been adjusted.",
            "Credit reversed",
        ),
        _ => {
            let (subject, text) = render(item, dashboard);
            let html = format!("<p>{}</p>", escape_html(&text).replace('\n', "<br>"));
            return (subject, html, text);
        }
    };
    let title = kind(&item.event_kind)
        .expect("known top-up notification")
        .subject_en;
    let mut rows = Vec::new();
    if let Some(amount) = amount_text(&item.payload) {
        let label = if item.event_kind == "topup_reversed" {
            "Reversed amount"
        } else {
            "Top-up amount"
        };
        rows.push((label, amount));
    }
    if let Some(fee) = item.payload.get("feeMinor").and_then(Value::as_i64) {
        rows.push((
            "Maintenance fee",
            format!("${}.{:02}", fee / 100, fee % 100),
        ));
    }
    if let Some(pay) = item.payload.get("payMinor").and_then(Value::as_i64) {
        rows.push(("Payment total", format!("${}.{:02}", pay / 100, pay % 100)));
    }
    rows.push(("Status", status.to_string()));
    if let Some(order_id) = item.payload.get("orderId").and_then(Value::as_str) {
        rows.push(("Order", order_id.to_string()));
    }
    let rows_html = rows.iter().map(|(label, value)| format!(
        "<tr><th align=\"left\" valign=\"top\" style=\"padding:8px 16px 8px 0;color:#64748b;font-size:13px;font-weight:500\">{}</th><td style=\"padding:8px 0;color:#0f172a;font-size:14px;word-break:break-all\">{}</td></tr>",
        escape_html(label), escape_html(value),
    )).collect::<Vec<_>>().join("");
    let wallet_url = format!("{}/account/wallet/", dashboard.trim_end_matches('/'));
    let body_html = format!(
        "<p style=\"margin:0 0 18px;color:#475569;font-size:15px;line-height:1.6\">{}</p>\
         <table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\" style=\"margin:0 0 22px\">{rows_html}</table>\
         <a href=\"{}\" style=\"display:inline-block;padding:12px 20px;border-radius:8px;background:#2563eb;color:#ffffff;text-decoration:none;font-size:14px;font-weight:600\">Open platform wallet</a>",
        escape_html(introduction),
        escape_html(&wallet_url),
    );
    let html = crate::store::render_email_layout(title, introduction, &body_html);
    let details = rows
        .iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!(
        "TokenSwitch\n\n{title}\n\n{introduction}\n\n{details}\n\nOpen platform wallet: {wallet_url}"
    );
    (format!("TokenSwitch: {title}"), html, text)
}

async fn deliver(
    state: &ServerState,
    http: &reqwest::Client,
    item: &Claimed,
) -> Result<String, String> {
    let recipient = {
        let conn = state.store.conn.lock().await;
        recipient_tx(&conn, item).map_err(|error| error.to_string())?
    };
    if recipient.opted_out {
        return Ok("suppressed".into());
    }
    let scheme = if state.config.use_localhost {
        "http"
    } else {
        "https"
    };
    let dashboard = format!(
        "{scheme}://{}",
        state.config.tunnel_domain.trim_end_matches('/')
    );
    let (_, body) = render(item, &dashboard);
    let mut delivered = Vec::new();
    let mut failures = Vec::new();
    if let (Some(email), Some(api_key), Some(from)) = (
        recipient.email.as_deref(),
        state.config.resend_api_key.as_deref(),
        crate::notifications::notification_sender(&state.config),
    ) {
        let (subject, html, text) = render_email(item, &dashboard);
        let key = format!("market-notification-{}", item.id);
        match crate::notifications::send_resend_frozen_email(
            http,
            api_key,
            crate::notifications::FrozenEmailEnvelope {
                from: &from,
                recipient: email,
                subject: &subject,
                html: &html,
                text: &text,
                reply_to: state.config.resend_reply_to.as_deref(),
                idempotency_key: &key,
            },
        )
        .await
        {
            Ok(_) => delivered.push("email"),
            Err(error) => failures.push(error.message),
        }
    }
    if let Some(chat) = recipient.telegram_chat.as_deref() {
        let token = state
            .dynamic
            .read()
            .await
            .telegram_bot
            .token()
            .map(str::to_string);
        if let Some(token) = token {
            match crate::telegram::send_message(
                http,
                &token,
                chat,
                None,
                &body,
                crate::telegram::TelegramParseMode::Plain,
            )
            .await
            {
                Ok(_) => delivered.push("telegram"),
                Err(error) => failures.push(error.message),
            }
        }
    }
    if delivered.is_empty() && !failures.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(if delivered.is_empty() {
        // No channel is configured: the in-app feed still holds the notice.
        "feed_only".into()
    } else {
        delivered.join(",")
    })
}

fn finish_tx(
    conn: &Connection,
    item: &Claimed,
    result: &Result<String, String>,
    now: &str,
) -> Result<(), AppError> {
    match result {
        Ok(channels) => conn.execute(
            "UPDATE market_notification_outbox
             SET status = CASE WHEN ?2 = 'suppressed' THEN 'suppressed' ELSE 'sent' END,
                 delivered_channels = ?2, sent_at = ?3, claimed_by = NULL,
                 claim_expires_at = NULL, last_error = NULL
             WHERE id = ?1",
            params![item.id, channels, now],
        ),
        Err(error) => {
            let delay = 60_i64 * 2_i64.pow(item.attempts.clamp(0, 10) as u32);
            let next = (Utc::now() + chrono::Duration::seconds(delay.min(21_600))).to_rfc3339();
            let message: String = crate::notifications::sanitize_delivery_error(error);
            conn.execute(
                "UPDATE market_notification_outbox
                 SET status = CASE WHEN attempts >= ?4 THEN 'failed' ELSE 'queued' END,
                     next_attempt_at = ?3, last_error = ?2, claimed_by = NULL,
                     claim_expires_at = NULL
                 WHERE id = ?1",
                params![item.id, message, next, MAX_ATTEMPTS],
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
    loop {
        tokio::select! {
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
                let now = Utc::now().to_rfc3339();
                let claimed = {
                    let conn = state.store.conn.lock().await;
                    claim_tx(&conn, &worker, &now)
                };
                let claimed = match claimed {
                    Ok(claimed) => claimed,
                    Err(_) => {
                        tracing::warn!("market notification queue unavailable");
                        continue;
                    }
                };
                for item in claimed {
                    let result = deliver(&state, &http, &item).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("open notification DB");
        crate::schema::apply(&conn).expect("apply schema");
        conn
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
    fn mandatory_kinds_ignore_opt_out_and_render_amounts() {
        let conn = memory();
        conn.execute(
            "INSERT INTO market_notification_preferences VALUES ('u', 'topup_credited', 0, 't')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_notification_preferences VALUES ('u', 'earnings_held', 0, 't')",
            [],
        )
        .unwrap();
        let optional = Claimed {
            id: "1".into(),
            recipient_user_id: "u".into(),
            event_kind: "topup_credited".into(),
            payload: json!({ "amountMinor": 1234 }),
            mandatory: false,
            attempts: 1,
        };
        assert!(recipient_tx(&conn, &optional).unwrap().opted_out);
        let mandatory = Claimed {
            event_kind: "earnings_held".into(),
            mandatory: true,
            ..optional
        };
        assert!(!recipient_tx(&conn, &mandatory).unwrap().opted_out);
        let (subject, body) = render(&mandatory, "https://router.example");
        assert!(subject.contains("收入已冻结"));
        assert!(body.contains("$12.34"));
        assert!(body.ends_with("https://router.example/account/wallet/"));
    }

    #[test]
    fn conversion_notice_names_wallet_credit_and_irreversible_terms() {
        let item = Claimed {
            id: "conversion-notice".into(),
            recipient_user_id: "provider".into(),
            event_kind: "earnings_converted".into(),
            payload: json!({"conversionId":"conversion-1","amountUnits":"43200000"}),
            mandatory: true,
            attempts: 1,
        };
        assert!(kind("earnings_converted").unwrap().mandatory);
        let (subject, body) = render(&item, "https://router.example/");
        assert!(subject.contains("收益已转入平台余额"));
        assert!(body.contains("$5.00000000"));
        assert!(body.contains("43200000 ledger units"));
        assert!(body.contains("conversion-1"));
        assert!(body.contains("不可提现或转回收益"));
        assert!(body.ends_with("https://router.example/account/wallet/"));
    }

    #[test]
    fn c2c_notices_are_mandatory_scoped_and_never_copy_private_payment_data() {
        let item = Claimed {
            id: "c2c-notice".into(),
            recipient_user_id: "seller".into(),
            event_kind: "c2c_order_updated".into(),
            payload: json!({
                "orderId": "order-1", "amountUnits": "86400000", "status": "paid",
                "contact": "PRIVATE_CONTACT", "paymentDetails": "PRIVATE_ACCOUNT",
                "code": "SECRET_RELEASE_CODE", "note": "PRIVATE_DISPUTE_EVIDENCE",
            }),
            mandatory: true,
            attempts: 1,
        };
        assert!(kind("c2c_order_updated").unwrap().mandatory);
        assert!(kind("c2c_seller_changed").unwrap().mandatory);
        let (subject, html, text) = render_email(&item, "https://router.example/");
        assert!(subject.contains("C2C"));
        assert!(text.contains("$10.00000000"));
        assert!(text.contains("买家已标记付款，待卖家核实"));
        assert!(text.contains("Offline payment is not verified by the platform"));
        assert!(text.contains("请勿向他人透露放行验证码"));
        assert!(text.ends_with("https://router.example/account/wallet/?c2cOrder=order-1"));
        for private in [
            "PRIVATE_CONTACT",
            "PRIVATE_ACCOUNT",
            "SECRET_RELEASE_CODE",
            "PRIVATE_DISPUTE_EVIDENCE",
        ] {
            assert!(!text.contains(private));
            assert!(!html.contains(private));
        }
        let seller_notice = Claimed {
            event_kind: "c2c_seller_changed".into(),
            payload: json!({}),
            ..item
        };
        let (_, _, text) = render_email(&seller_notice, "https://router.example/");
        assert!(text.contains("已有订单仍需处理"));
        assert!(text.ends_with("https://router.example/account/wallet/"));
        assert!(!text.contains("c2cOrder="));
    }

    #[test]
    fn commission_notice_links_to_earnings_and_does_not_repeat_bilingual_copy() {
        let item = Claimed {
            id: "commission-notice".into(),
            recipient_user_id: "provider".into(),
            event_kind: "commission_policy_changed".into(),
            payload: json!({"rateBps": 1025, "effectiveAt": "2026-11-01T00:00:00Z"}),
            mandatory: true,
            attempts: 1,
        };
        let (_, html, text) = render_email(&item, "https://router.example/");
        assert_eq!(text.matches("10.25%").count(), 2);
        assert_eq!(
            text.matches("Automatic income credit has no additional service fee.")
                .count(),
            1
        );
        assert!(!text.contains("Withdrawals"));
        assert!(text.ends_with("https://router.example/account/earnings/"));
        assert!(html.contains("account/earnings/"));
        assert!(!text.contains("account/wallet/"));
    }

    #[test]
    fn topup_emails_are_english_and_share_the_sign_in_layout() {
        for (event_kind, status) in [
            ("topup_credited", "Credited"),
            ("topup_closed_unpaid", "Closed without payment"),
            ("topup_review_required", "Payment under review"),
            ("topup_reversed", "Credit reversed"),
        ] {
            let item = Claimed {
                id: "notice-1".into(),
                recipient_user_id: "buyer".into(),
                event_kind: event_kind.into(),
                payload: json!({
                    "amountMinor": 1234,
                    "orderId": "order-1",
                    "status": "raw_internal_status",
                    "checkoutToken": "SECRET_CHECKOUT_TOKEN",
                }),
                mandatory: false,
                attempts: 1,
            };
            let (subject, html, text) = render_email(&item, "https://router.example/");
            assert!(subject.starts_with("TokenSwitch: "));
            assert!(html.contains("<html lang=\"en\">"));
            assert!(html.contains("x-apple-disable-message-reformatting"));
            assert!(html.matches("max-width:560px").count() >= 3);
            assert!(html.contains("href=\"https://router.example/account/wallet/\""));
            for content in [&subject, &html, &text] {
                assert!(content.is_ascii(), "{event_kind} must use English");
                assert!(!content.contains("raw_internal_status"));
                assert!(!content.contains("SECRET_CHECKOUT_TOKEN"));
            }
            for content in [&html, &text] {
                assert!(content.contains("$12.34"));
                assert!(content.contains(status));
                assert!(content.contains("order-1"));
                assert!(content.contains("Open platform wallet"));
            }
            if event_kind == "topup_review_required" {
                assert!(text.contains("Do not pay for this order again"));
            }
        }
    }

    #[test]
    fn topup_email_escapes_values_and_link_attributes() {
        let item = Claimed {
            id: "notice-1".into(),
            recipient_user_id: "buyer".into(),
            event_kind: "topup_review_required".into(),
            payload: json!({ "orderId": "<img src=x onerror='alert(1)'>" }),
            mandatory: false,
            attempts: 1,
        };
        let (_, html, _) = render_email(&item, "https://router.example/\" onmouseover=\"test");
        assert!(!html.contains("<img"));
        assert!(html.contains("&lt;img src=x onerror=&#39;alert(1)&#39;&gt;"));
        assert!(html.contains("&quot; onmouseover=&quot;test/account/wallet/"));
        assert!(!html.contains("\" onmouseover=\"test"));
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
