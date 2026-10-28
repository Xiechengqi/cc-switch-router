//! User-scoped in-app business events. Delivery retries, reminders and channel
//! settings never generate additional inbox entries or mark an entry as read.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, header::CACHE_CONTROL};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ServerState;
use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use crate::error::AppError;

const PAGE_SIZE: usize = 50;
const NO_STORE: [(axum::http::header::HeaderName, &str); 1] =
    [(CACHE_CONTROL, "private, no-store")];

// Recipient and role checks apply to every operation, including cursor lookup,
// unread counts and mark-all. The client cannot choose an audience or recipient.
const VISIBLE: &str = "WITH visible AS (
    SELECT 'market:' || id AS id,
        CASE WHEN event_kind IN (
            'contract_suspended', 'contract_resumed', 'contract_terminating_soon',
            'contract_terminated', 'service_status_summary', 'dispute_resolved',
            'client_termination_claim', 'client_termination_settled'
        ) THEN 'service' ELSE 'wallet' END AS category,
        event_kind, object_kind, object_id, payload_json, read_at,
        CAST(ROUND((julianday(created_at) - 2440587.5) * 86400000) AS INTEGER) AS sort_at,
        'market' AS source, rowid AS source_rowid
    FROM market_notification_outbox WHERE recipient_user_id = ?1
    UNION ALL
    SELECT e.id, e.category, e.event_kind, e.object_kind, e.object_id,
        e.payload_json, r.read_at, e.created_at_ms AS sort_at, 'event' AS source, e.rowid AS source_rowid
    FROM user_notification_events e
    LEFT JOIN user_notification_reads r ON r.notification_id = e.id AND r.user_id = ?1
    WHERE (e.scope = 'user' AND e.recipient_email = ?2)
       OR (e.scope = 'owner' AND ?3 = 1 AND NOT EXISTS (
           SELECT 1 FROM user_notification_events personal
           WHERE personal.id = json_extract(e.payload_json, '$.clientEventId')
             AND personal.scope = 'user' AND personal.recipient_email = ?2
       ))
) ";

fn db_error(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

struct Viewer<'a> {
    user_id: &'a str,
    email: &'a str,
    owner: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationView {
    id: String,
    category: String,
    event_kind: String,
    object_kind: String,
    object_id: String,
    payload: Value,
    read_at: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedPage {
    items: Vec<NotificationView>,
    next_cursor: Option<String>,
    unread: i64,
    read_through: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FeedQuery {
    cursor: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadRequest {
    /// Mark-all is bounded by the newest event the user actually loaded.
    through: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadBoundary {
    user_id: String,
    market: i64,
    event: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadResult {
    unread: i64,
    read_at: String,
}

fn position_tx(conn: &Connection, viewer: &Viewer<'_>, id: &str) -> Result<i64, AppError> {
    conn.query_row(
        &format!("{VISIBLE} SELECT sort_at FROM visible WHERE id = ?4"),
        params![viewer.user_id, viewer.email, viewer.owner, id],
        |row| row.get(0),
    )
    .optional()
    .map_err(db_error("read notification position"))?
    .ok_or_else(|| AppError::BadRequest("notification is unavailable".into()))
}

fn unread_tx(conn: &Connection, viewer: &Viewer<'_>) -> Result<i64, AppError> {
    conn.query_row(
        &format!("{VISIBLE} SELECT COUNT(*) FROM visible WHERE read_at IS NULL"),
        params![viewer.user_id, viewer.email, viewer.owner],
        |row| row.get(0),
    )
    .map_err(db_error("count notification center unread"))
}

fn feed_tx(
    conn: &Connection,
    viewer: &Viewer<'_>,
    cursor: Option<&str>,
) -> Result<FeedPage, AppError> {
    let cursor_at = cursor.map(|id| position_tx(conn, viewer, id)).transpose()?;
    let mut statement = conn.prepare(&format!(
        "{VISIBLE} SELECT id, category, event_kind, object_kind, object_id, payload_json, read_at, sort_at
         FROM visible WHERE (?4 IS NULL OR sort_at < ?4 OR (sort_at = ?4 AND id < ?5))
         ORDER BY sort_at DESC, id DESC LIMIT {}", PAGE_SIZE + 1
    )).map_err(db_error("prepare notification center"))?;
    let mut items = statement
        .query_map(
            params![
                viewer.user_id,
                viewer.email,
                viewer.owner,
                cursor_at,
                cursor
            ],
            |row| {
                let payload: String = row.get(5)?;
                let millis: i64 = row.get(7)?;
                Ok(NotificationView {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    event_kind: row.get(2)?,
                    object_kind: row.get(3)?,
                    object_id: row.get(4)?,
                    payload: serde_json::from_str(&payload).unwrap_or_else(|_| json!({})),
                    read_at: row.get(6)?,
                    created_at: DateTime::from_timestamp_millis(millis)
                        .unwrap_or_default()
                        .to_rfc3339(),
                })
            },
        )
        .map_err(db_error("query notification center"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read notification center"))?;
    let more = items.len() > PAGE_SIZE;
    items.truncate(PAGE_SIZE);
    let next_cursor = more
        .then(|| items.last().map(|item| item.id.clone()))
        .flatten();
    let (market, event): (i64, i64) = conn
        .query_row(
            &format!("{VISIBLE} SELECT COALESCE(MAX(CASE WHEN source = 'market' THEN source_rowid END), 0),
                COALESCE(MAX(CASE WHEN source = 'event' THEN source_rowid END), 0) FROM visible"),
            params![viewer.user_id, viewer.email, viewer.owner],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read notification center boundary"))?;
    let read_through = if market == 0 && event == 0 {
        None
    } else {
        let boundary = serde_json::to_vec(&ReadBoundary {
            user_id: viewer.user_id.to_string(),
            market,
            event,
        })
        .map_err(|error| {
            AppError::Internal(format!("encode notification boundary failed: {error}"))
        })?;
        Some(format!("v1.{}", URL_SAFE_NO_PAD.encode(boundary)))
    };
    Ok(FeedPage {
        items,
        next_cursor,
        unread: unread_tx(conn, viewer)?,
        read_through,
    })
}

fn mark_read_tx(
    conn: &Connection,
    viewer: &Viewer<'_>,
    id: &str,
    input: &ReadRequest,
) -> Result<ReadResult, AppError> {
    let boundary = if id == "all" {
        let invalid = || AppError::BadRequest("invalid notification boundary".into());
        let encoded = input
            .through
            .as_deref()
            .filter(|s| s.len() <= 1024)
            .and_then(|s| s.strip_prefix("v1."))
            .ok_or_else(invalid)?;
        let decoded = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
        let boundary: ReadBoundary = serde_json::from_slice(&decoded).map_err(|_| invalid())?;
        if boundary.user_id != viewer.user_id || boundary.market < 0 || boundary.event < 0 {
            return Err(invalid());
        }
        boundary
    } else {
        position_tx(conn, viewer, id)?;
        ReadBoundary {
            user_id: viewer.user_id.to_string(),
            market: 0,
            event: 0,
        }
    };
    let read_at = Utc::now().to_rfc3339();
    // Source high-water marks, not event timestamps: a delayed/backdated event
    // or two inserts in the same millisecond must not be silently marked read.
    let targets = "SELECT id FROM visible WHERE read_at IS NULL AND
        (id = ?4 OR (?4 = 'all' AND ((source = 'market' AND source_rowid <= ?5)
            OR (source = 'event' AND source_rowid <= ?6))))";
    conn.execute(
        &format!(
            "{VISIBLE} UPDATE market_notification_outbox SET read_at = COALESCE(read_at, ?7)
        WHERE recipient_user_id = ?1 AND 'market:' || id IN ({targets})"
        ),
        params![
            viewer.user_id,
            viewer.email,
            viewer.owner,
            id,
            boundary.market,
            boundary.event,
            read_at
        ],
    )
    .map_err(db_error("mark market notifications read"))?;
    conn.execute(&format!("{VISIBLE} INSERT OR IGNORE INTO user_notification_reads (user_id, notification_id, read_at)
        SELECT ?1, e.id, ?7 FROM user_notification_events e WHERE e.id IN ({targets})"),
        params![viewer.user_id, viewer.email, viewer.owner, id, boundary.market, boundary.event, read_at])
        .map_err(db_error("mark notification center events read"))?;
    Ok(ReadResult {
        unread: unread_tx(conn, viewer)?,
        read_at,
    })
}

async fn session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::models::AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))
}

async fn list_feed(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<FeedQuery>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = session(&state, &headers).await?;
    let viewer = Viewer {
        user_id: &session.user_id,
        email: &session.email,
        owner: crate::api::is_finance_owner(&state.config, &session.email),
    };
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(db_error("begin notification feed snapshot"))?;
    let page = feed_tx(&tx, &viewer, query.cursor.as_deref())?;
    tx.commit()
        .map_err(db_error("complete notification feed snapshot"))?;
    Ok((NO_STORE, Json(page)))
}

async fn mark_read(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ReadRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = session(&state, &headers).await?;
    let viewer = Viewer {
        user_id: &session.user_id,
        email: &session.email,
        owner: crate::api::is_finance_owner(&state.config, &session.email),
    };
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin notification read"))?;
    let result = mark_read_tx(&tx, &viewer, &id, &input)?;
    tx.commit().map_err(db_error("commit notification read"))?;
    Ok((NO_STORE, Json(result)))
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/notifications", get(list_feed))
        .route("/v1/notifications/:id/read", post(mark_read))
}

/// Only verified Client ownership may create a personal Client notice. Never
/// copy registration snapshots: they may contain setup/password hints.
pub(crate) fn client_event_tx(
    conn: &Connection,
    installation_id: &str,
    event_kind: &str,
    episode: i64,
    time: DateTime<Utc>,
) -> Result<(), AppError> {
    let target: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT LOWER(TRIM(i.owner_email)), t.subdomain FROM installations i
         LEFT JOIN installation_client_tunnels t ON t.installation_id = i.id
         WHERE i.id = ?1 AND i.owner_verified_at IS NOT NULL
           AND i.owner_email IS NOT NULL AND TRIM(i.owner_email) <> ''",
            params![installation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_error("read Client notification recipient"))?;
    if let Some((email, subdomain)) = target {
        conn.execute("INSERT OR IGNORE INTO user_notification_events
            (id, scope, recipient_email, category, event_kind, object_kind, object_id, payload_json, created_at_ms)
            VALUES (?1, 'user', ?2, 'client', ?3, 'client', ?4, ?5, ?6)",
            params![format!("client:{event_kind}:{installation_id}:{episode}"), email, event_kind,
                installation_id, json!({ "clientLabel": subdomain.as_deref().unwrap_or(installation_id) }).to_string(), time.timestamp_millis()])
            .map_err(db_error("enqueue Client in-app notification"))?;
    }
    Ok(())
}

/// Cross-database alert mirroring is at-least-once. The transition ID is the
/// idempotency key; only the firing/escalated/resolved transitions are exported.
pub(crate) async fn store_owner_alerts(
    store: &crate::store::AppStore,
    alerts: &[crate::alerting::models::InAppAlertEvent],
) -> Result<(), AppError> {
    let conn = store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Owner notification batch"))?;
    for alert in alerts {
        tx.execute(
            "INSERT OR IGNORE INTO user_notification_events
            (id, scope, category, event_kind, object_kind, object_id, payload_json, created_at_ms)
            VALUES (?1, 'owner', 'system', ?2, 'incident', ?3, ?4, ?5)",
            params![
                format!("alert:{}", alert.id),
                if alert.transition == "resolved" {
                    "operator_alert_resolved"
                } else {
                    "operator_alert_firing"
                },
                alert.incident_id,
                json!({ "title": alert.title.chars().take(200).collect::<String>(),
                    "severity": alert.severity, "clientEventId": alert.related_user_event_id })
                .to_string(),
                alert.occurred_at.saturating_mul(1000)
            ],
        )
        .map_err(db_error("enqueue Owner in-app notification"))?;
    }
    tx.commit()
        .map_err(db_error("commit Owner notification batch"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        crate::test_support::database().unwrap()
    }
    fn buyer() -> Viewer<'static> {
        Viewer {
            user_id: "buyer",
            email: "buyer@example.com",
            owner: false,
        }
    }
    fn owner() -> Viewer<'static> {
        Viewer {
            user_id: "owner",
            email: "owner@example.com",
            owner: true,
        }
    }

    fn market(conn: &Connection, user: &str, key: &str, time: &str) -> String {
        crate::market_notifications::enqueue_user_notification_tx(
            conn,
            "buyer",
            user,
            "topup_credited",
            "topup",
            key,
            key,
            json!({ "amountMinor": 100 }),
            time,
        )
        .unwrap();
        conn.query_row(
            "SELECT 'market:' || id FROM market_notification_outbox WHERE dedupe_key = ?1",
            params![key],
            |row| row.get(0),
        )
        .unwrap()
    }
    fn personal(conn: &Connection, id: &str, email: &str, millis: i64) {
        conn.execute("INSERT INTO user_notification_events
            (id, scope, recipient_email, category, event_kind, object_kind, object_id, created_at_ms)
            VALUES (?1, 'user', ?2, 'client', 'client_offline', 'client', 'client-1', ?3)", params![id, email, millis]).unwrap();
    }
    fn owner_event(conn: &Connection, id: &str, millis: i64) {
        conn.execute(
            "INSERT INTO user_notification_events
            (id, scope, category, event_kind, object_kind, object_id, created_at_ms)
            VALUES (?1, 'owner', 'system', 'operator_alert_firing', 'incident', 'incident-1', ?2)",
            params![id, millis],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn notification_center_owner_export_retries_and_dual_role_client_events_are_deduplicated()
    {
        let store = crate::store::AppStore::new_in_memory_for_tests().unwrap();
        let alert = crate::alerting::models::InAppAlertEvent {
            id: "one".into(),
            incident_id: "incident".into(),
            transition: "firing".into(),
            severity: "critical".into(),
            title: "Client offline".into(),
            occurred_at: 1793145600,
            related_user_event_id: Some("client:mine".into()),
        };
        store_owner_alerts(&store, std::slice::from_ref(&alert))
            .await
            .unwrap();
        store_owner_alerts(&store, std::slice::from_ref(&alert))
            .await
            .unwrap();
        let c = store.conn.lock().await;
        assert_eq!(feed_tx(&c, &owner(), None).unwrap().unread, 1);
        personal(&c, "client:mine", owner().email, 1793145600000);
        let feed = feed_tx(&c, &owner(), None).unwrap();
        assert_eq!(
            feed.unread, 1,
            "Owner/Client owner receives one entry for the same offline event"
        );
        assert_eq!(feed.items[0].id, "client:mine");
        let next_owner = Viewer {
            user_id: "next-owner",
            email: "next-owner@example.com",
            owner: true,
        };
        assert_eq!(
            feed_tx(&c, &next_owner, None).unwrap().items[0].id,
            "alert:one"
        );
        assert_eq!(
            mark_read_tx(&c, &next_owner, "alert:one", &ReadRequest::default())
                .unwrap()
                .unread,
            0
        );
        assert_eq!(
            feed_tx(&c, &owner(), None).unwrap().unread,
            1,
            "Owner alerts have per-user read state, not one global read flag"
        );
        assert!(feed_tx(&c, &buyer(), None).unwrap().items.is_empty());
    }

    #[tokio::test]
    async fn notification_center_owner_batch_rolls_back_and_retries_without_partial_or_duplicate_events()
     {
        let store = crate::store::AppStore::new_in_memory_for_tests().unwrap();
        {
            let conn = store.conn.lock().await;
            conn.execute_batch("CREATE TRIGGER reject_second_notification BEFORE INSERT ON user_notification_events
                WHEN NEW.id = 'alert:second' BEGIN SELECT RAISE(ABORT, 'test batch failure'); END;").unwrap();
        }
        let first = crate::alerting::models::InAppAlertEvent {
            id: "first".into(),
            incident_id: "incident".into(),
            transition: "firing".into(),
            severity: "warning".into(),
            title: "Test alert".into(),
            occurred_at: 1793145600,
            related_user_event_id: None,
        };
        let alerts = [
            first.clone(),
            crate::alerting::models::InAppAlertEvent {
                id: "second".into(),
                ..first
            },
        ];
        assert!(store_owner_alerts(&store, &alerts).await.is_err());
        {
            let conn = store.conn.lock().await;
            assert!(feed_tx(&conn, &owner(), None).unwrap().items.is_empty());
            conn.execute_batch("DROP TRIGGER reject_second_notification;")
                .unwrap();
        }
        store_owner_alerts(&store, &alerts).await.unwrap();
        store_owner_alerts(&store, &alerts).await.unwrap();
        let conn = store.conn.lock().await;
        assert_eq!(feed_tx(&conn, &owner(), None).unwrap().unread, 2);
    }

    #[test]
    fn notification_center_unifies_events_without_leaking_other_users_or_owner_alerts() {
        let c = memory();
        market(&c, "buyer", "one", "2026-10-28T00:00:00Z");
        market(&c, "other", "two", "2026-10-28T00:00:00Z");
        personal(&c, "client:one", buyer().email, 1793145601000);
        personal(&c, "client:other", "other@example.com", 1793145602000);
        owner_event(&c, "alert:one", 1793145603000);
        let feed = feed_tx(&c, &buyer(), None).unwrap();
        assert_eq!(feed.unread, 2);
        assert_eq!(feed.items.len(), 2);
        assert!(feed.items.iter().all(|item| item.category != "system"));
        assert_eq!(
            feed.items
                .iter()
                .filter(|item| item.category == "wallet")
                .count(),
            1
        );
        let feed = feed_tx(&c, &owner(), None).unwrap();
        assert_eq!(feed.unread, 1);
        assert_eq!(feed.items[0].id, "alert:one");
        let admin_not_owner = Viewer {
            owner: false,
            ..owner()
        };
        assert!(
            feed_tx(&c, &admin_not_owner, None)
                .unwrap()
                .items
                .is_empty()
        );
        assert!(feed_tx(&c, &buyer(), Some("client:other")).is_err());
        assert!(feed_tx(&c, &buyer(), Some("alert:one")).is_err());
        assert!(mark_read_tx(&c, &buyer(), "alert:one", &ReadRequest::default()).is_err());
        assert!(mark_read_tx(&c, &buyer(), "client:other", &ReadRequest::default()).is_err());
    }

    #[test]
    fn notification_center_uses_stable_cross_source_pagination_and_preserves_market_read_state() {
        let c = memory();
        let old = market(&c, "buyer", "old", "2026-10-28T00:00:00+00:00");
        mark_read_tx(&c, &buyer(), &old, &ReadRequest::default()).unwrap();
        for index in 0..104 {
            personal(
                &c,
                &format!("client:{index:03}"),
                buyer().email,
                1793145600000,
            );
        }
        let mut seen = std::collections::HashSet::new();
        let mut cursor = None;
        loop {
            let feed = feed_tx(&c, &buyer(), cursor.as_deref()).unwrap();
            assert_eq!(feed.unread, 104);
            for item in &feed.items {
                assert!(seen.insert(item.id.clone()), "duplicate {}", item.id);
            }
            cursor = feed.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen.len(), 105);
        assert!(feed_tx(&c, &buyer(), Some("missing")).is_err());
        let (read_at, delivery): (Option<String>, String) = c
            .query_row(
                "SELECT read_at, status FROM market_notification_outbox WHERE dedupe_key = 'old'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(read_at.is_some());
        assert_eq!(
            delivery, "queued",
            "in-app reads must not change delivery state"
        );
    }

    #[test]
    fn notification_center_mark_all_has_a_visible_boundary_and_never_consumes_new_events() {
        let c = memory();
        let old = market(&c, "buyer", "one", "2026-10-28T00:00:00Z");
        personal(&c, "client:old", buyer().email, 1793145599000);
        let boundary = feed_tx(&c, &buyer(), None).unwrap().read_through;
        assert!(
            mark_read_tx(
                &c,
                &owner(),
                "all",
                &ReadRequest {
                    through: boundary.clone()
                }
            )
            .is_err(),
            "another user's read snapshot cannot be reused"
        );
        for invalid in ["", "v1.not-json", "v2.unknown"] {
            assert!(
                mark_read_tx(
                    &c,
                    &buyer(),
                    "all",
                    &ReadRequest {
                        through: Some(invalid.into())
                    }
                )
                .is_err()
            );
        }
        // Same timestamp and an older sort key still represent a NEW event.
        personal(&c, "client:aaa-new", buyer().email, 1793145599000);
        personal(&c, "client:other", "other@example.com", 1793145601000);
        owner_event(&c, "alert:new", 1793145601000);
        assert!(mark_read_tx(&c, &buyer(), "all", &ReadRequest::default()).is_err());
        let result = mark_read_tx(&c, &buyer(), "all", &ReadRequest { through: boundary }).unwrap();
        assert_eq!(result.unread, 1);
        let feed = feed_tx(&c, &buyer(), None).unwrap();
        assert!(
            feed.items
                .iter()
                .find(|item| item.id == "client:aaa-new")
                .unwrap()
                .read_at
                .is_none()
        );
        assert!(
            feed.items
                .iter()
                .find(|item| item.id == old)
                .unwrap()
                .read_at
                .is_some()
        );
        mark_read_tx(&c, &buyer(), "client:old", &ReadRequest::default()).unwrap();
        assert_eq!(
            unread_tx(&c, &buyer()).unwrap(),
            1,
            "repeated reads are idempotent"
        );
        assert_eq!(feed_tx(&c, &owner(), None).unwrap().unread, 1);
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM user_notification_reads", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn notification_center_client_events_are_verified_deduplicated_and_transactional() {
        let c = memory();
        c.execute(
            "INSERT INTO installations (id, public_key, platform, app_version, owner_email,
            created_at, last_seen_at, control_secret_b64) VALUES ('client', 'key', 'linux', 'test',
            'Buyer@Example.com', '2026-10-28T00:00:00Z', '2026-10-28T00:00:00Z', 'secret')",
            [],
        )
        .unwrap();
        client_event_tx(&c, "client", "client_offline", 1, Utc::now()).unwrap();
        assert_eq!(
            unread_tx(&c, &buyer()).unwrap(),
            0,
            "unverified owner cannot receive a notice"
        );
        c.execute("UPDATE installations SET owner_verified_at = '2026-10-28T00:00:00Z' WHERE id = 'client'", []).unwrap();
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        client_event_tx(&tx, "client", "client_offline", 1, Utc::now()).unwrap();
        drop(tx);
        assert_eq!(unread_tx(&c, &buyer()).unwrap(), 0);
        for _ in 0..3 {
            client_event_tx(&c, "client", "client_offline", 1, Utc::now()).unwrap();
        }
        client_event_tx(&c, "client", "client_recovered", 1, Utc::now()).unwrap();
        client_event_tx(&c, "client", "client_offline", 2, Utc::now()).unwrap();
        let feed = feed_tx(&c, &buyer(), None).unwrap();
        assert_eq!(feed.unread, 3);
        assert!(
            feed.items
                .iter()
                .all(|item| item.payload.as_object().unwrap().len() == 1)
        );
        assert!(!serde_json::to_string(&feed).unwrap().contains("secret"));
        c.execute(
            "UPDATE installations SET owner_email = 'other@example.com' WHERE id = 'client'",
            [],
        )
        .unwrap();
        let other = Viewer {
            user_id: "other",
            email: "other@example.com",
            owner: false,
        };
        assert_eq!(
            unread_tx(&c, &other).unwrap(),
            0,
            "historical notices never transfer with Client ownership"
        );
    }
}
