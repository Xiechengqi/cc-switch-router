use std::collections::{BTreeMap, HashSet};

use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{AppStore, RouteHealthStatus};
use crate::client_chat::{
    CHAT_ARCHIVE_RETENTION_SECS, CHAT_EMAIL_BATCH_WINDOW_SECS, CHAT_MAX_BODY_CHARS,
    CHAT_MESSAGE_PAGE_MAX, CHAT_PUBLIC_LOOKUP_MAX_ROOMS, CHAT_ROOM_MESSAGES_PER_MINUTE,
    CHAT_USER_MESSAGES_PER_HOUR, CHAT_USER_MESSAGES_PER_MINUTE, ChatAggregateStats,
    ChatDeliveryClaim, ChatEmailData, ChatEmailMessageData, render_chat_email,
};
use crate::error::AppError;
use crate::models::{
    AuthSession, ClientChatDeliveryView, ClientChatMessageListResponse, ClientChatMessagePreview,
    ClientChatMessageView, ClientChatReadResponse, ClientChatRoomListResponse, ClientChatRoomView,
    ClientChatVisitImportItem,
};
use crate::notifications::{
    NotificationTemplateContext, mask_email_address, mask_email_like_tokens,
};

enum ChatDeliveryOutcome<'a> {
    Sent(&'a str),
    Retry {
        error: &'a str,
        next_attempt_at: DateTime<Utc>,
    },
    DeadLetter(&'a str),
}

const SHARE_OFFLINE_CONFIRM_SECS: i64 = 180;
const SHARE_RECOVERY_CONFIRM_SECS: i64 = 120;

pub(super) fn ensure_room_for_verified_owner_tx(
    conn: &Connection,
    installation_id: &str,
    owner_email: &str,
    now: DateTime<Utc>,
) -> Result<String, AppError> {
    let owner_email = owner_email.trim().to_ascii_lowercase();
    let label = conn
        .query_row(
            "SELECT COALESCE(NULLIF(t.subdomain, ''), i.platform || ' · ' || substr(i.id, 1, 8))
             FROM installations i
             LEFT JOIN installation_client_tunnels t ON t.installation_id = i.id
             WHERE i.id = ?1
               AND i.lifecycle = 'active'
               AND i.client_activated_at IS NOT NULL
               AND i.owner_verified_at IS NOT NULL
               AND lower(trim(i.owner_email)) = ?2",
            params![installation_id, owner_email],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read chat client label failed: {error}")))?
        .ok_or_else(|| AppError::Conflict("client owner is not verified".into()))?;

    let existing = conn
        .query_row(
            "SELECT id, owner_email_snapshot FROM chat_rooms
             WHERE installation_id = ?1
             ORDER BY owner_generation DESC LIMIT 1",
            params![installation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read client chat room failed: {error}")))?;
    if let Some((room_id, existing_owner)) = existing {
        let owner_changed = !existing_owner.eq_ignore_ascii_case(&owner_email);
        conn.execute(
            "UPDATE chat_rooms
             SET client_label_snapshot = ?2,
                 owner_email_snapshot = ?3,
                 owner_user_id_snapshot = (
                    SELECT id FROM users WHERE email_normalized = ?3
                 ),
                 owner_generation = owner_generation + CASE WHEN lower(owner_email_snapshot) != ?3 THEN 1 ELSE 0 END,
                 status = 'active', archived_at = NULL, delete_after = NULL, updated_at = ?4
             WHERE id = ?1",
            params![room_id, label, owner_email, now.to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("reactivate client chat room failed: {error}")))?;
        if owner_changed {
            requeue_room_deliveries_for_current_owner_tx(conn, &room_id, now)?;
        }
        return Ok(room_id);
    }

    let room_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO chat_rooms (
            id, installation_id, client_label_snapshot,
            owner_user_id_snapshot, owner_email_snapshot,
            owner_generation, status, created_at, updated_at
         ) VALUES (?1, ?2, ?3,
                   (SELECT id FROM users WHERE email_normalized = ?4),
                   ?4, 1, 'active', ?5, ?5)",
        params![
            room_id,
            installation_id,
            label,
            owner_email,
            now.to_rfc3339()
        ],
    )
    .map_err(|error| AppError::Internal(format!("create client chat room failed: {error}")))?;
    Ok(room_id)
}

pub(super) fn archive_room_for_installation_tx(
    conn: &Connection,
    installation_id: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let room_id = conn
        .query_row(
            "SELECT id FROM chat_rooms
             WHERE installation_id = ?1 AND status = 'active'",
            params![installation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("read chat room for archive failed: {error}"))
        })?;
    let Some(room_id) = room_id else {
        return Ok(());
    };
    conn.execute(
        "UPDATE chat_rooms
         SET status = 'archived', archived_at = ?2, delete_after = ?3, updated_at = ?2
         WHERE id = ?1 AND status = 'active'",
        params![
            room_id,
            now.to_rfc3339(),
            (now + Duration::seconds(CHAT_ARCHIVE_RETENTION_SECS)).to_rfc3339()
        ],
    )
    .map_err(|error| AppError::Internal(format!("archive client chat room failed: {error}")))?;
    cancel_room_deliveries_tx(conn, &room_id, "cancelled_room_archived", now)?;
    Ok(())
}

pub(super) fn update_active_room_label_for_installation_tx(
    conn: &Connection,
    installation_id: &str,
    client_label: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE chat_rooms
         SET client_label_snapshot = ?2, updated_at = ?3
         WHERE installation_id = ?1 AND status = 'active'",
        params![installation_id, client_label, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("update client chat label failed: {error}")))?;
    Ok(())
}

pub(super) fn suppress_pending_system_events_for_installation_tx(
    conn: &Connection,
    installation_id: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE client_chat_system_outbox
         SET status = 'completed', next_attempt_at = NULL, last_error = ?2,
             updated_at = ?3, completed_at = ?3
         WHERE installation_id = ?1 AND status IN ('pending', 'processing')",
        params![installation_id, reason, now.to_rfc3339()],
    )
    .map_err(|error| {
        AppError::Internal(format!(
            "suppress fenced Client chat system events failed: {error}"
        ))
    })?;
    Ok(())
}

pub(crate) fn record_share_presence_observation_tx(
    conn: &Connection,
    share_id: &str,
    status: RouteHealthStatus,
    reason: &str,
    router_epoch: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let share_lifecycle = conn
        .query_row(
            "SELECT share_status, expires_at FROM shares WHERE share_id = ?1",
            params![share_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read Share lifecycle failed: {error}")))?;
    let Some((share_status, expires_at)) = share_lifecycle else {
        return Ok(());
    };
    let expires_at_text = expires_at;
    let expires_at = DateTime::parse_from_rfc3339(&expires_at_text)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or(now);
    let lifecycle_active = share_status == "active" && expires_at > now;
    if share_status == "active" && expires_at <= now {
        crate::share_market::enqueue_share_lifecycle_event_tx(
            conn,
            share_id,
            "share_expired",
            serde_json::json!({ "expiresAt": expires_at_text }),
            &format!("share-lifecycle:{share_id}:expired:{expires_at_text}"),
            now,
        )?;
    }
    let stored = conn
        .query_row(
            "SELECT state, router_epoch, has_online_baseline, unhealthy_since,
                    healthy_since, offline_episode
             FROM share_presence_state WHERE share_id = ?1",
            params![share_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("read Share presence state failed: {error}"))
        })?;
    let now_text = now.to_rfc3339();
    if !lifecycle_active {
        conn.execute(
            "INSERT INTO share_presence_state (
                share_id, state, router_epoch, has_online_baseline, unhealthy_since,
                healthy_since, offline_episode, created_at, updated_at
             ) VALUES (?1, 'unknown', ?2, 0, NULL, NULL, 0, ?3, ?3)
             ON CONFLICT(share_id) DO UPDATE SET
                state = 'unknown', router_epoch = excluded.router_epoch,
                has_online_baseline = 0, unhealthy_since = NULL, healthy_since = NULL,
                updated_at = excluded.updated_at",
            params![share_id, router_epoch, now_text],
        )
        .map_err(|error| {
            AppError::Internal(format!("reset Share presence state failed: {error}"))
        })?;
        return Ok(());
    }
    let Some((state, previous_epoch, has_online_baseline, unhealthy_since, healthy_since, episode)) =
        stored
    else {
        let (initial_state, baseline) = if status == RouteHealthStatus::Healthy {
            ("online", 1)
        } else {
            ("unknown", 0)
        };
        conn.execute(
            "INSERT INTO share_presence_state (
                share_id, state, router_epoch, has_online_baseline, unhealthy_since,
                healthy_since, offline_episode, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, NULL, NULL, 0, ?5, ?5)",
            params![share_id, initial_state, router_epoch, baseline, now_text],
        )
        .map_err(|error| AppError::Internal(format!("baseline Share presence failed: {error}")))?;
        return Ok(());
    };

    if previous_epoch != router_epoch {
        let (next_state, next_baseline, next_healthy_since) = if state == "offline" {
            (
                "offline",
                i64::from(has_online_baseline),
                (status == RouteHealthStatus::Healthy).then_some(now_text.as_str()),
            )
        } else if status == RouteHealthStatus::Healthy {
            ("online", 1, None)
        } else {
            ("unknown", i64::from(has_online_baseline), None)
        };
        conn.execute(
            "UPDATE share_presence_state
             SET state = ?2, router_epoch = ?3, has_online_baseline = ?4,
                 unhealthy_since = NULL, healthy_since = ?5, updated_at = ?6
             WHERE share_id = ?1",
            params![
                share_id,
                next_state,
                router_epoch,
                next_baseline,
                next_healthy_since,
                now_text,
            ],
        )
        .map_err(|error| {
            AppError::Internal(format!("rebaseline Share presence epoch failed: {error}"))
        })?;
        return Ok(());
    }

    match status {
        RouteHealthStatus::Unknown => {
            conn.execute(
                "UPDATE share_presence_state
                 SET unhealthy_since = NULL, healthy_since = NULL, updated_at = ?2
                 WHERE share_id = ?1",
                params![share_id, now_text],
            )
            .map_err(|error| {
                AppError::Internal(format!("pause Share presence transition failed: {error}"))
            })?;
        }
        RouteHealthStatus::Healthy if state == "offline" => {
            let started_at = healthy_since
                .as_deref()
                .map(|value| parse_timestamp(value.to_string(), "Share recovery candidate"))
                .transpose()?
                .unwrap_or(now);
            if now.signed_duration_since(started_at).num_seconds() >= SHARE_RECOVERY_CONFIRM_SECS {
                let event_id = crate::share_market::enqueue_share_lifecycle_event_tx(
                    conn,
                    share_id,
                    "service_recovered",
                    serde_json::json!({
                        "episode": episode,
                        "recoveredAt": now_text,
                    }),
                    &format!("share-presence:{share_id}:{episode}:recovered"),
                    now,
                )?;
                conn.execute(
                    "UPDATE share_presence_state
                     SET state = 'online', has_online_baseline = 1,
                         unhealthy_since = NULL, healthy_since = NULL,
                         last_recovered_event_id = ?2, updated_at = ?3
                     WHERE share_id = ?1",
                    params![share_id, event_id, now_text],
                )
                .map_err(|error| {
                    AppError::Internal(format!("confirm Share recovery failed: {error}"))
                })?;
            } else if healthy_since.is_none() {
                conn.execute(
                    "UPDATE share_presence_state SET healthy_since = ?2, updated_at = ?2
                     WHERE share_id = ?1",
                    params![share_id, now_text],
                )
                .map_err(|error| {
                    AppError::Internal(format!("start Share recovery candidate failed: {error}"))
                })?;
            }
        }
        RouteHealthStatus::Healthy => {
            conn.execute(
                "UPDATE share_presence_state
                 SET state = 'online', has_online_baseline = 1,
                     unhealthy_since = NULL, healthy_since = NULL, updated_at = ?2
                 WHERE share_id = ?1",
                params![share_id, now_text],
            )
            .map_err(|error| {
                AppError::Internal(format!("confirm Share online baseline failed: {error}"))
            })?;
        }
        RouteHealthStatus::Unhealthy if state != "offline" && has_online_baseline => {
            let started_at = unhealthy_since
                .as_deref()
                .map(|value| parse_timestamp(value.to_string(), "Share offline candidate"))
                .transpose()?
                .unwrap_or(now);
            if now.signed_duration_since(started_at).num_seconds() >= SHARE_OFFLINE_CONFIRM_SECS {
                let next_episode = episode.saturating_add(1);
                let event_id = crate::share_market::enqueue_share_lifecycle_event_tx(
                    conn,
                    share_id,
                    "service_offline",
                    serde_json::json!({
                        "episode": next_episode,
                        "offlineSince": started_at.to_rfc3339(),
                        "reason": reason,
                    }),
                    &format!("share-presence:{share_id}:{next_episode}:offline"),
                    now,
                )?;
                conn.execute(
                    "UPDATE share_presence_state
                     SET state = 'offline', unhealthy_since = NULL, healthy_since = NULL,
                         offline_episode = ?2, last_offline_event_id = ?3, updated_at = ?4
                     WHERE share_id = ?1",
                    params![share_id, next_episode, event_id, now_text],
                )
                .map_err(|error| {
                    AppError::Internal(format!("confirm Share offline transition failed: {error}"))
                })?;
            } else if unhealthy_since.is_none() {
                conn.execute(
                    "UPDATE share_presence_state SET unhealthy_since = ?2, updated_at = ?2
                     WHERE share_id = ?1",
                    params![share_id, now_text],
                )
                .map_err(|error| {
                    AppError::Internal(format!("start Share offline candidate failed: {error}"))
                })?;
            }
        }
        RouteHealthStatus::Unhealthy => {
            conn.execute(
                "UPDATE share_presence_state SET healthy_since = NULL, updated_at = ?2
                 WHERE share_id = ?1",
                params![share_id, now_text],
            )
            .map_err(|error| {
                AppError::Internal(format!("retain Share offline state failed: {error}"))
            })?;
        }
    }
    Ok(())
}

pub(crate) fn enqueue_client_system_event_tx(
    conn: &Connection,
    installation_id: &str,
    source_kind: &str,
    source_event_id: &str,
    event_type: &str,
    payload: serde_json::Value,
    follower_user_ids: &[String],
    now: &str,
) -> Result<(), AppError> {
    validate_system_event_identity(installation_id, "installation id", 128)?;
    validate_system_event_identity(source_kind, "source kind", 64)?;
    validate_system_event_identity(source_event_id, "source event id", 256)?;
    validate_system_event_identity(event_type, "event type", 128)?;
    let fenced_without_archive =
        conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM installations i
                WHERE i.id = ?1 AND i.lifecycle = 'fenced'
                  AND NOT EXISTS (
                      SELECT 1 FROM chat_rooms r
                      WHERE r.installation_id = i.id AND r.status = 'archived'
                  )
             )",
            params![installation_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| {
            AppError::Internal(format!("check fenced Client chat target failed: {error}"))
        })? != 0;
    if fenced_without_archive {
        return Ok(());
    }
    let payload = sanitize_system_event_payload(payload)?;
    let payload = if is_public_market_source_kind(source_kind) {
        public_market_event_payload(&payload)
    } else {
        payload
    };
    let payload_json = serde_json::to_string(&payload)
        .map_err(|error| AppError::Internal(format!("encode client chat event failed: {error}")))?;
    if payload_json.len() > 64 * 1024 {
        return Err(AppError::BadRequest(
            "client chat event payload is too large".into(),
        ));
    }
    let mut followers = follower_user_ids
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    followers.sort();
    followers.dedup();
    let followers_json = serde_json::to_string(&followers)
        .map_err(|error| AppError::Internal(format!("encode chat followers failed: {error}")))?;
    conn.execute(
        "INSERT OR IGNORE INTO client_chat_system_outbox (
            id, installation_id, source_kind, source_event_id, event_type,
            payload_json, follower_user_ids_json, status, attempts,
            next_attempt_at, last_error, created_at, updated_at, completed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 0,
                   NULL, NULL, ?8, ?8, NULL)",
        params![
            Uuid::new_v4().to_string(),
            installation_id,
            source_kind,
            source_event_id,
            event_type,
            payload_json,
            followers_json,
            now,
        ],
    )
    .map_err(|error| AppError::Internal(format!("enqueue client chat event failed: {error}")))?;
    Ok(())
}

fn validate_system_event_identity(
    value: &str,
    field: &str,
    max_len: usize,
) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > max_len
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(AppError::BadRequest(format!("invalid {field}")));
    }
    Ok(())
}

pub(crate) fn sanitize_system_event_payload(
    mut payload: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    if !payload.is_object() {
        return Err(AppError::BadRequest(
            "client chat event payload must be an object".into(),
        ));
    }
    sanitize_system_event_value(&mut payload, None)?;
    Ok(payload)
}

fn is_public_market_source_kind(source_kind: &str) -> bool {
    matches!(
        source_kind,
        "share_market" | "market_billing" | "client_market"
    )
}

/// Client chat is intentionally public. Market producers keep rich private
/// events in their own tables; the outbox and materializer only accept this
/// allowlisted projection for the public room.
pub(crate) fn public_market_event_payload(payload: &serde_json::Value) -> serde_json::Value {
    let mut public = serde_json::Map::new();
    for field in [
        "summary",
        "marketKind",
        "billingEventType",
        "installationId",
        "shareId",
        "shareName",
        "appType",
        "subdomain",
        "ownerEmail",
        "supplierEmail",
    ] {
        if let Some(value) = payload.get(field) {
            public.insert(field.into(), value.clone());
        }
    }

    let market_kind = payload
        .get("marketKind")
        .and_then(serde_json::Value::as_str);
    if market_kind == Some("share") {
        for field in [
            "listingId",
            "seatId",
            "seatPosition",
            "seatStatus",
            "subscriptionStatus",
            "parallelLimit",
            "tokenLimit",
            "tokenPeriod",
            "dailyRateMinor",
            "currency",
            "serviceDurationDays",
            "offerRevision",
        ] {
            if let Some(value) = payload.get(field) {
                public.insert(field.into(), value.clone());
            }
        }
    }
    if matches!(market_kind, Some("client" | "client_host")) {
        for field in [
            "clientLabel",
            "providerEmail",
            "hostname",
            "status",
            "dailyRateMinor",
            "currency",
            "offerRevision",
            "trialHours",
            "freeDurationDays",
            "activatedAt",
            "expiresAt",
            "providerDeniedClientAccess",
            "reason",
            "failureCode",
        ] {
            if let Some(value) = payload.get(field) {
                public.insert(field.into(), value.clone());
            }
        }
        if !public.contains_key("supplierEmail")
            && let Some(value) = payload.get("providerEmail")
        {
            public.insert("supplierEmail".into(), value.clone());
        }
    }

    let method_kinds = payload
        .get("paymentMethodKinds")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            payload
                .get("paymentMethods")
                .and_then(serde_json::Value::as_array)
                .map(|methods| {
                    methods
                        .iter()
                        .filter_map(|method| method.get("kind").and_then(serde_json::Value::as_str))
                        .map(|kind| serde_json::Value::String(kind.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        });
    public.insert("paymentMethodKinds".into(), method_kinds.into());
    public.insert(
        "contacts".into(),
        payload
            .get("contacts")
            .or_else(|| payload.get("paymentContacts"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    );
    serde_json::Value::Object(public)
}

pub(crate) fn sanitize_system_event_text(value: &str) -> String {
    if contains_credential_fragment(value) {
        "[credential omitted]".into()
    } else {
        value.to_string()
    }
}

fn sanitize_system_event_value(
    value: &mut serde_json::Value,
    field_name: Option<&str>,
) -> Result<(), AppError> {
    match value {
        serde_json::Value::Object(object) => {
            let allows_payment_asset_token = object
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| kind == "crypto")
                && object
                    .get("token")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|token| matches!(token, "USDT" | "USDC"));
            for (key, value) in object {
                let normalized = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if is_forbidden_event_field(&normalized)
                    && !(normalized == "token" && allows_payment_asset_token)
                {
                    return Err(AppError::BadRequest(format!(
                        "client chat event payload contains forbidden credential field {key}"
                    )));
                }
                sanitize_system_event_value(value, Some(key))?;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                sanitize_system_event_value(value, field_name)?;
            }
        }
        serde_json::Value::String(text) => {
            let field = field_name.unwrap_or("url");
            if looks_like_event_url(text, field) {
                validate_event_url(text, field)?;
            }
            if contains_credential_fragment(text) {
                *text = sanitize_system_event_text(text);
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_forbidden_event_field(normalized: &str) -> bool {
    [
        "apikey",
        "accesstoken",
        "refreshtoken",
        "idtoken",
        "sessiontoken",
        "oauthtoken",
        "authtoken",
        "apitoken",
        "bearertoken",
        "verificationtoken",
        "provisiontoken",
        "resettoken",
        "csrftoken",
        "token",
        "jwt",
        "authorization",
        "cookie",
        "setcookie",
        "password",
        "clientwebpassword",
        "secret",
        "controlsecret",
        "sshpassword",
        "privatekey",
        "credential",
        "credentials",
        "leasecredential",
        "passphrase",
    ]
    .iter()
    .any(|forbidden| normalized == *forbidden || normalized.ends_with(forbidden))
}

fn looks_like_event_url(value: &str, field: &str) -> bool {
    value.starts_with("https://")
        || value.starts_with("http://")
        || (value.starts_with('/')
            && field
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
                .ends_with("url"))
}

fn validate_event_url(value: &str, field: &str) -> Result<(), AppError> {
    if value.starts_with('/') {
        if value.starts_with("//") || value.chars().any(char::is_control) {
            return Err(AppError::BadRequest(format!(
                "{field} must be a same-origin absolute path"
            )));
        }
        let parsed = url::Url::parse(&format!("https://router.invalid{value}"))
            .map_err(|_| AppError::BadRequest(format!("{field} must be a valid public URL")))?;
        return validate_event_url_query(&parsed, field);
    }
    validate_public_event_url(value, field)
}

pub(crate) fn validate_public_event_url(value: &str, field: &str) -> Result<(), AppError> {
    let parsed = url::Url::parse(value)
        .map_err(|_| AppError::BadRequest(format!("{field} must be a valid public URL")))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(AppError::BadRequest(format!(
            "{field} must be an HTTP or HTTPS URL"
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::BadRequest(format!(
            "{field} must not contain URL credentials"
        )));
    }
    validate_event_url_query(&parsed, field)
}

fn validate_event_url_query(parsed: &url::Url, field: &str) -> Result<(), AppError> {
    for (key, _) in parsed.query_pairs() {
        if is_forbidden_event_url_parameter(&key) {
            return Err(AppError::BadRequest(format!(
                "{field} must not contain credential query parameters"
            )));
        }
    }
    if let Some(fragment) = parsed.fragment() {
        for (key, _) in url::form_urlencoded::parse(fragment.as_bytes()) {
            if is_forbidden_event_url_parameter(&key) {
                return Err(AppError::BadRequest(format!(
                    "{field} must not contain credential fragment parameters"
                )));
            }
        }
    }
    Ok(())
}

fn is_forbidden_event_url_parameter(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "token",
        "signature",
        "secret",
        "key",
        "credential",
        "authorization",
        "password",
    ]
    .iter()
    .any(|forbidden| normalized.contains(forbidden))
}

fn contains_credential_fragment(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "authorization:",
        "authorization=",
        "proxy-authorization:",
        "proxy-authorization=",
        "bearer ",
        "x-api-key:",
        "x-api-key=",
        "x-goog-api-key:",
        "x-goog-api-key=",
        "api_key=",
        "apikey=",
        "access_token=",
        "refresh_token=",
        "id_token=",
        "session_token=",
        "oauth_token=",
        "client_secret=",
        "cookie:",
        "set-cookie:",
        "control_secret=",
        "ssh_password=",
        "private_key=",
        "password=",
        "secret=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || lower.starts_with("sk-")
        || lower.contains(" sk-")
}

pub(super) fn cleanup_expired_rooms_tx(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<usize, AppError> {
    let deleted = conn
        .execute(
            "DELETE FROM chat_rooms
         WHERE status = 'archived' AND delete_after IS NOT NULL AND delete_after <= ?1",
            params![now.to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("delete expired chat rooms failed: {error}"))
        })?;
    conn.execute(
        "DELETE FROM chat_rate_limit WHERE bucket_start < ?1",
        params![now.timestamp() - 2 * 60 * 60],
    )
    .map_err(|error| {
        AppError::Internal(format!("delete expired chat rate limits failed: {error}"))
    })?;
    conn.execute(
        "DELETE FROM email_send_logs
         WHERE email_type = 'client_chat' AND created_at < ?1",
        params![(now - Duration::seconds(CHAT_ARCHIVE_RETENTION_SECS)).to_rfc3339()],
    )
    .map_err(|error| {
        AppError::Internal(format!("delete expired chat send logs failed: {error}"))
    })?;
    Ok(deleted)
}

impl AppStore {
    pub async fn enforce_client_chat_public_read_rate(
        &self,
        client_ip: Option<&str>,
    ) -> Result<(), AppError> {
        let scope_value = client_ip.unwrap_or("unknown");
        let mut hasher = Sha256::new();
        hasher.update(self.ip_hash_salt.as_bytes());
        hasher.update(b"\0client-chat-public-read\0");
        hasher.update(scope_value.as_bytes());
        let scope = format!("public-read:{}", hex::encode(hasher.finalize()));
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin public chat read limit failed: {error}"))
            })?;
        let result =
            consume_chat_rate_limit_tx(&tx, &scope, 60, 600, now).map_err(|error| match error {
                AppError::RateLimited {
                    retry_after_secs, ..
                } => AppError::RateLimited {
                    message: "public chat read rate limit exceeded".into(),
                    retry_after_secs,
                },
                other => other,
            });
        if result.is_ok() {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit public chat read limit failed: {error}"))
            })?;
        }
        result
    }

    pub async fn get_client_chat_room_by_installation(
        &self,
        installation_id: &str,
        viewer_user_id: Option<&str>,
    ) -> Result<ClientChatRoomView, AppError> {
        validate_public_id(installation_id, "installation id")?;
        let conn = self.conn.lock().await;
        load_room_by_installation(&conn, installation_id, viewer_user_id)?
            .ok_or_else(|| AppError::NotFound("client chat room not found".into()))
    }

    pub async fn lookup_chat_rooms(
        &self,
        installation_ids: Vec<String>,
        last_read_seq_by_installation: BTreeMap<String, i64>,
        viewer_user_id: Option<&str>,
    ) -> Result<ClientChatRoomListResponse, AppError> {
        if installation_ids.len() > CHAT_PUBLIC_LOOKUP_MAX_ROOMS {
            return Err(AppError::BadRequest(format!(
                "installationIds cannot contain more than {CHAT_PUBLIC_LOOKUP_MAX_ROOMS} entries"
            )));
        }
        let mut seen = HashSet::new();
        let mut normalized = Vec::new();
        for installation_id in installation_ids {
            validate_public_id(&installation_id, "installation id")?;
            if seen.insert(installation_id.clone()) {
                normalized.push(installation_id);
            }
        }
        if last_read_seq_by_installation.len() > CHAT_PUBLIC_LOOKUP_MAX_ROOMS {
            return Err(AppError::BadRequest(format!(
                "lastReadSeqByInstallation cannot contain more than {CHAT_PUBLIC_LOOKUP_MAX_ROOMS} entries"
            )));
        }
        for installation_id in last_read_seq_by_installation.keys() {
            validate_public_id(installation_id, "installation id")?;
            if !seen.contains(installation_id) {
                return Err(AppError::BadRequest(
                    "lastReadSeqByInstallation keys must be included in installationIds".into(),
                ));
            }
        }
        let conn = self.conn.lock().await;
        let mut rooms = Vec::new();
        for installation_id in normalized {
            if let Some(mut room) =
                load_room_by_installation(&conn, &installation_id, viewer_user_id)?
            {
                if viewer_user_id.is_none() {
                    room.unread_count = count_visible_messages_after(
                        &conn,
                        &room.id,
                        last_read_seq_by_installation
                            .get(&installation_id)
                            .copied()
                            .unwrap_or(0),
                    )?;
                }
                rooms.push(room);
            }
        }
        let total_unread = rooms.iter().map(|room| room.unread_count).sum();
        Ok(ClientChatRoomListResponse {
            rooms,
            total_unread,
        })
    }

    pub async fn list_visited_chat_rooms(
        &self,
        user_id: &str,
    ) -> Result<ClientChatRoomListResponse, AppError> {
        let conn = self.conn.lock().await;
        let room_ids = {
            let mut statement = conn
                .prepare(
                    "SELECT r.id
                     FROM chat_rooms r
                     INNER JOIN chat_visits v ON v.room_id = r.id AND v.user_id = ?1
                     ORDER BY COALESCE(r.last_message_at, v.last_opened_at, r.created_at) DESC,
                              r.id DESC
                     LIMIT ?2",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare recent chat rooms failed: {error}"))
                })?;
            statement
                .query_map(
                    params![user_id, CHAT_PUBLIC_LOOKUP_MAX_ROOMS as i64],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|error| {
                    AppError::Internal(format!("query recent chat rooms failed: {error}"))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!("read recent chat rooms failed: {error}"))
                })?
        };
        let mut rooms = Vec::with_capacity(room_ids.len());
        for room_id in room_ids {
            if let Some(room) = load_room_by_id(&conn, &room_id, Some(user_id))? {
                rooms.push(room);
            }
        }
        let total_unread = rooms.iter().map(|room| room.unread_count).sum();
        Ok(ClientChatRoomListResponse {
            rooms,
            total_unread,
        })
    }

    pub async fn record_client_chat_visit(
        &self,
        room_id: &str,
        user_id: &str,
    ) -> Result<ClientChatRoomView, AppError> {
        validate_public_id(room_id, "room id")?;
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let room = load_room_by_id(&conn, room_id, Some(user_id))?
            .ok_or_else(|| AppError::NotFound("chat room not found".into()))?;
        upsert_visit_tx(&conn, user_id, room_id, None, now)?;
        Ok(room)
    }

    pub async fn remove_client_chat_visit(
        &self,
        room_id: &str,
        user_id: &str,
    ) -> Result<(), AppError> {
        validate_public_id(room_id, "room id")?;
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM chat_visits WHERE user_id = ?1 AND room_id = ?2",
            params![user_id, room_id],
        )
        .map_err(|error| AppError::Internal(format!("remove chat visit failed: {error}")))?;
        Ok(())
    }

    pub async fn import_chat_visits(
        &self,
        user_id: &str,
        visits: Vec<ClientChatVisitImportItem>,
    ) -> Result<usize, AppError> {
        if visits.len() > CHAT_PUBLIC_LOOKUP_MAX_ROOMS {
            return Err(AppError::BadRequest(format!(
                "visits cannot contain more than {CHAT_PUBLIC_LOOKUP_MAX_ROOMS} entries"
            )));
        }
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat visit import failed: {error}"))
            })?;
        let mut imported = 0;
        let mut seen = HashSet::new();
        for visit in visits {
            validate_public_id(&visit.installation_id, "installation id")?;
            if !seen.insert(visit.installation_id.clone()) {
                continue;
            }
            let room = tx
                .query_row(
                    "SELECT id, COALESCE((SELECT MAX(seq) FROM chat_messages WHERE room_id = r.id), 0)
                     FROM chat_rooms r
                     WHERE installation_id = ?1 AND status = 'active'",
                    params![visit.installation_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(|error| AppError::Internal(format!("resolve imported chat visit failed: {error}")))?;
            if let Some((room_id, latest_seq)) = room {
                upsert_visit_tx(
                    &tx,
                    user_id,
                    &room_id,
                    Some(visit.last_read_seq.clamp(0, latest_seq)),
                    now,
                )?;
                imported += 1;
            }
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit chat visit import failed: {error}"))
        })?;
        Ok(imported)
    }

    pub async fn mark_client_chat_read(
        &self,
        room_id: &str,
        user_id: &str,
        last_read_seq: i64,
    ) -> Result<ClientChatReadResponse, AppError> {
        validate_public_id(room_id, "room id")?;
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let room = load_room_by_id(&conn, room_id, Some(user_id))?
            .ok_or_else(|| AppError::NotFound("chat room not found".into()))?;
        let latest_seq = room.latest_seq;
        let next = last_read_seq.clamp(0, latest_seq);
        upsert_visit_tx(&conn, user_id, room_id, Some(next), now)?;
        Ok(ClientChatReadResponse {
            ok: true,
            last_read_seq: next,
        })
    }

    pub async fn list_chat_messages(
        &self,
        room_id: &str,
        viewer_user_id: Option<&str>,
        before_seq: Option<i64>,
        after_seq: Option<i64>,
        limit: usize,
    ) -> Result<ClientChatMessageListResponse, AppError> {
        validate_public_id(room_id, "room id")?;
        if before_seq.is_some() && after_seq.is_some() {
            return Err(AppError::BadRequest(
                "beforeSeq and afterSeq cannot be combined".into(),
            ));
        }
        let conn = self.conn.lock().await;
        let room_exists = conn
            .query_row(
                "SELECT 1 FROM chat_rooms WHERE id = ?1",
                params![room_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("check chat room failed: {error}")))?;
        if room_exists.is_none() {
            return Err(AppError::NotFound("chat room not found".into()));
        }
        let limit = limit.clamp(1, CHAT_MESSAGE_PAGE_MAX);
        let fetch_limit = (limit + 1) as i64;
        let select = "SELECT id, seq, body, author_user_id, author_label, author_kind,
                             message_kind, event_type, event_payload_json, status, created_at
                      FROM chat_messages m";
        let mut messages = if let Some(after_seq) = after_seq {
            query_messages(
                &conn,
                &format!("{select} WHERE room_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3"),
                params![room_id, after_seq.max(0), fetch_limit],
                viewer_user_id,
            )?
        } else if let Some(before_seq) = before_seq {
            let mut rows = query_messages(
                &conn,
                &format!("{select} WHERE room_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3"),
                params![room_id, before_seq.max(0), fetch_limit],
                viewer_user_id,
            )?;
            rows.reverse();
            rows
        } else {
            let mut rows = query_messages(
                &conn,
                &format!("{select} WHERE room_id = ?1 ORDER BY seq DESC LIMIT ?2"),
                params![room_id, fetch_limit],
                viewer_user_id,
            )?;
            rows.reverse();
            rows
        };
        let has_more = messages.len() > limit;
        if has_more {
            if before_seq.is_some() || (before_seq.is_none() && after_seq.is_none()) {
                messages.remove(0);
            } else {
                messages.truncate(limit);
            }
        }
        let latest_seq = latest_visible_seq(&conn, room_id)?;
        Ok(ClientChatMessageListResponse {
            messages,
            latest_seq,
            has_more,
        })
    }

    pub async fn get_chat_room_latest_seq(
        &self,
        room_id: &str,
        _viewer_user_id: Option<&str>,
    ) -> Result<i64, AppError> {
        validate_public_id(room_id, "room id")?;
        let conn = self.conn.lock().await;
        let room_exists = conn
            .query_row(
                "SELECT 1 FROM chat_rooms WHERE id = ?1",
                params![room_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("check chat room failed: {error}")))?;
        if room_exists.is_none() {
            return Err(AppError::NotFound("chat room not found".into()));
        }
        latest_visible_seq(&conn, room_id)
    }

    pub async fn create_client_chat_message(
        &self,
        room_id: &str,
        session: &AuthSession,
        body: String,
        client_message_id: String,
    ) -> Result<ClientChatMessageView, AppError> {
        validate_public_id(room_id, "room id")?;
        Uuid::parse_str(client_message_id.trim())
            .map_err(|_| AppError::BadRequest("clientMessageId must be a UUID".into()))?;
        let body = normalize_chat_body(&body)?;
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat message transaction failed: {error}"))
            })?;

        if let Some(existing) =
            load_idempotent_message(&tx, room_id, &session.user_id, client_message_id.trim())?
        {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit idempotent chat message failed: {error}"))
            })?;
            return Ok(existing);
        }

        let room = tx
            .query_row(
                "SELECT r.installation_id, r.owner_email_snapshot, r.owner_generation,
                        lower(trim(i.owner_email)), i.owner_verified_at
                 FROM chat_rooms r
                 LEFT JOIN installations i ON i.id = r.installation_id
                 WHERE r.id = ?1 AND r.status = 'active' AND i.lifecycle = 'active'",
                params![room_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("read active chat room failed: {error}")))?
            .ok_or_else(|| AppError::Conflict("chat room is archived or unavailable".into()))?;
        let (installation_id, owner_snapshot, owner_generation, owner_email, owner_verified_at) =
            room;
        let owner_email = owner_email
            .filter(|_| owner_verified_at.is_some())
            .ok_or_else(|| AppError::Conflict("client owner is not verified".into()))?;
        if !owner_snapshot.eq_ignore_ascii_case(&owner_email) {
            return Err(AppError::Conflict(
                "client owner changed; retry the message".into(),
            ));
        }

        consume_chat_rate_limit_tx(
            &tx,
            &format!("user-minute:{}", session.user_id),
            60,
            CHAT_USER_MESSAGES_PER_MINUTE,
            now,
        )?;
        consume_chat_rate_limit_tx(
            &tx,
            &format!("user-hour:{}", session.user_id),
            3_600,
            CHAT_USER_MESSAGES_PER_HOUR,
            now,
        )?;
        consume_chat_rate_limit_tx(
            &tx,
            &format!("room-minute:{room_id}"),
            60,
            CHAT_ROOM_MESSAGES_PER_MINUTE,
            now,
        )?;

        let message_id = Uuid::new_v4().to_string();
        let author_email = session.email.trim().to_ascii_lowercase();
        let author_label = email_local_part(&author_email)?;
        tx.execute(
            "INSERT INTO chat_messages (
                id, room_id, author_user_id, author_email, author_label,
                client_message_id, body, status, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'visible', ?8, ?8)",
            params![
                message_id,
                room_id,
                session.user_id,
                author_email,
                author_label,
                client_message_id.trim(),
                body,
                now.to_rfc3339(),
            ],
        )
        .map_err(|error| AppError::Internal(format!("insert chat message failed: {error}")))?;
        let seq = tx.last_insert_rowid();
        tx.execute(
            "UPDATE chat_rooms SET last_message_at = ?2, updated_at = ?2 WHERE id = ?1",
            params![room_id, now.to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("update chat room activity failed: {error}"))
        })?;

        if !author_email.eq_ignore_ascii_case(&owner_email) {
            insert_chat_email_event_tx(
                &tx,
                &message_id,
                room_id,
                &installation_id,
                owner_generation,
                &owner_email,
                now,
            )?;
        }
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit chat message failed: {error}")))?;
        Ok(ClientChatMessageView {
            id: message_id,
            seq,
            body,
            author_label,
            author_kind: "user".into(),
            message_kind: "text".into(),
            event_type: None,
            event_payload: None,
            is_mine: true,
            status: "visible".into(),
            created_at: now,
        })
    }

    pub async fn process_client_chat_system_outbox(&self, limit: usize) -> Result<usize, AppError> {
        let conn = self.conn.lock().await;
        let mut completed = 0;
        for _ in 0..limit.clamp(1, 200) {
            let now = Utc::now();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| {
                    AppError::Internal(format!("begin chat outbox transaction failed: {error}"))
                })?;
            let item = tx
                .query_row(
                    "SELECT id, attempts FROM client_chat_system_outbox
                     WHERE status = 'pending'
                       AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
                     ORDER BY created_at, id
                     LIMIT 1",
                    params![now.to_rfc3339()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(|error| AppError::Internal(format!("read chat outbox failed: {error}")))?;
            let Some((outbox_id, attempts)) = item else {
                tx.commit().map_err(|error| {
                    AppError::Internal(format!("commit empty chat outbox failed: {error}"))
                })?;
                break;
            };
            tx.execute(
                "UPDATE client_chat_system_outbox
                 SET status = 'processing', attempts = attempts + 1, updated_at = ?2
                 WHERE id = ?1 AND status = 'pending'",
                params![outbox_id, now.to_rfc3339()],
            )
            .map_err(|error| AppError::Internal(format!("claim chat outbox failed: {error}")))?;
            match materialize_client_system_event_tx(&tx, &outbox_id, now) {
                Ok(()) => {
                    tx.commit().map_err(|error| {
                        AppError::Internal(format!("commit chat outbox event failed: {error}"))
                    })?;
                    completed += 1;
                }
                Err(error) => {
                    let retry_delay = 5_i64
                        .saturating_mul(1_i64 << attempts.clamp(0, 6) as u32)
                        .min(3_600);
                    let next_status = if attempts + 1 >= 12 {
                        "dead_letter"
                    } else {
                        "pending"
                    };
                    tx.execute(
                        "UPDATE client_chat_system_outbox
                         SET status = ?2, next_attempt_at = ?3, last_error = ?4, updated_at = ?5
                         WHERE id = ?1",
                        params![
                            outbox_id,
                            next_status,
                            (now + Duration::seconds(retry_delay)).to_rfc3339(),
                            error.to_string(),
                            now.to_rfc3339(),
                        ],
                    )
                    .map_err(|update_error| {
                        AppError::Internal(format!(
                            "record chat outbox failure failed: {update_error}"
                        ))
                    })?;
                    tx.commit().map_err(|commit_error| {
                        AppError::Internal(format!(
                            "commit chat outbox failure failed: {commit_error}"
                        ))
                    })?;
                    tracing::warn!(
                        outbox_id,
                        attempts = attempts + 1,
                        error = %error,
                        "client chat system event materialization failed"
                    );
                }
            }
        }
        Ok(completed)
    }

    pub async fn delete_client_chat_message(
        &self,
        message_id: &str,
        deleted_by: &str,
    ) -> Result<ClientChatMessageView, AppError> {
        validate_public_id(message_id, "message id")?;
        let now = Utc::now();
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| AppError::Internal(format!("begin chat delete failed: {error}")))?;
        let message = tx
            .query_row(
                "SELECT id, seq, body, author_label, author_kind, message_kind,
                        event_type, event_payload_json, status, created_at
                 FROM chat_messages WHERE id = ?1",
                params![message_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read chat message for delete failed: {error}"))
            })?
            .ok_or_else(|| AppError::NotFound("chat message not found".into()))?;
        if message.4 == "system" {
            return Err(AppError::Conflict(
                "system chat events cannot be deleted".into(),
            ));
        }
        if message.8 != "deleted" {
            tx.execute(
                "UPDATE chat_messages
                 SET status = 'deleted', body = '', deleted_by = ?2,
                     deleted_at = ?3, updated_at = ?3
                 WHERE id = ?1",
                params![message_id, deleted_by, now.to_rfc3339()],
            )
            .map_err(|error| AppError::Internal(format!("delete chat message failed: {error}")))?;
            cancel_deleted_message_delivery_tx(&tx, message_id, now)?;
        }
        tx.commit()
            .map_err(|error| AppError::Internal(format!("commit chat delete failed: {error}")))?;
        Ok(ClientChatMessageView {
            id: message.0,
            seq: message.1,
            body: String::new(),
            author_label: message.3,
            author_kind: message.4,
            message_kind: message.5,
            event_type: message.6,
            event_payload: parse_event_payload(message.7)?,
            is_mine: false,
            status: "deleted".into(),
            created_at: parse_timestamp(message.9, "chat message")?,
        })
    }

    pub async fn aggregate_client_chat_deliveries(
        &self,
        template: &NotificationTemplateContext,
        now: DateTime<Utc>,
    ) -> Result<ChatAggregateStats, AppError> {
        let Some(sender) = template.sender.as_deref() else {
            return Ok(ChatAggregateStats::default());
        };
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat email aggregation failed: {error}"))
            })?;
        let windows = {
            let mut statement = tx
                .prepare(
                    "SELECT room_id, installation_id, owner_generation, recipient,
                            window_started_at, window_ends_at
                     FROM chat_email_events
                     WHERE status = 'pending' AND window_ends_at <= ?1
                     GROUP BY room_id, installation_id, owner_generation, recipient,
                              window_started_at, window_ends_at
                     ORDER BY window_ends_at ASC, room_id ASC
                     LIMIT 25",
                )
                .map_err(|error| {
                    AppError::Internal(format!("prepare chat email windows failed: {error}"))
                })?;
            statement
                .query_map(params![now.to_rfc3339()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })
                .map_err(|error| {
                    AppError::Internal(format!("query chat email windows failed: {error}"))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Internal(format!("read chat email windows failed: {error}"))
                })?
        };
        let mut stats = ChatAggregateStats::default();
        for (
            room_id,
            installation_id,
            owner_generation,
            recipient,
            window_started_at,
            window_ends_at,
        ) in windows
        {
            let room = tx
                .query_row(
                    "SELECT COALESCE(NULLIF(t.subdomain, ''), r.client_label_snapshot),
                            r.status, r.owner_generation,
                            r.owner_email_snapshot,
                            lower(trim(i.owner_email)), i.owner_verified_at
                     FROM chat_rooms r
                     LEFT JOIN installations i ON i.id = r.installation_id
                     LEFT JOIN installation_client_tunnels t ON t.installation_id = r.installation_id
                     WHERE r.id = ?1",
                    params![room_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!("validate chat email room failed: {error}"))
                })?;
            let Some((
                client_label,
                room_status,
                current_generation,
                owner_snapshot,
                installation_owner,
                owner_verified_at,
            )) = room
            else {
                continue;
            };
            let owner_matches = room_status == "active"
                && current_generation == owner_generation
                && owner_snapshot.eq_ignore_ascii_case(&recipient)
                && installation_owner
                    .as_deref()
                    .is_some_and(|owner| owner.eq_ignore_ascii_case(&recipient))
                && owner_verified_at.is_some();
            if !owner_matches {
                if let (Some(owner), Some(_)) =
                    (installation_owner.as_deref(), owner_verified_at.as_ref())
                {
                    ensure_room_for_verified_owner_tx(&tx, &installation_id, owner, now)?;
                    requeue_room_deliveries_for_current_owner_tx(&tx, &room_id, now)?;
                } else {
                    archive_room_for_installation_tx(&tx, &installation_id, now)?;
                }
                continue;
            }

            let events = {
                let mut statement = tx
                    .prepare(
                        "SELECT e.id, m.created_at, m.author_label, m.body
                         FROM chat_email_events e
                         INNER JOIN chat_messages m ON m.id = e.message_id
                         WHERE e.room_id = ?1 AND e.installation_id = ?2
                           AND e.owner_generation = ?3 AND lower(e.recipient) = lower(?4)
                           AND e.window_started_at = ?5 AND e.window_ends_at = ?6
                           AND e.status = 'pending' AND m.status = 'visible'
                         ORDER BY m.seq ASC",
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("prepare chat email events failed: {error}"))
                    })?;
                statement
                    .query_map(
                        params![
                            room_id,
                            installation_id,
                            owner_generation,
                            recipient,
                            window_started_at,
                            window_ends_at
                        ],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                            ))
                        },
                    )
                    .map_err(|error| {
                        AppError::Internal(format!("query chat email events failed: {error}"))
                    })?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        AppError::Internal(format!("read chat email events failed: {error}"))
                    })?
            };
            if events.is_empty() {
                tx.execute(
                    "UPDATE chat_email_events
                     SET status = 'cancelled_message_deleted', updated_at = ?7
                     WHERE room_id = ?1 AND installation_id = ?2
                       AND owner_generation = ?3 AND lower(recipient) = lower(?4)
                       AND window_started_at = ?5 AND window_ends_at = ?6
                       AND status = 'pending'",
                    params![
                        room_id,
                        installation_id,
                        owner_generation,
                        recipient,
                        window_started_at,
                        window_ends_at,
                        now.to_rfc3339()
                    ],
                )
                .map_err(|error| {
                    AppError::Internal(format!("cancel empty chat email window failed: {error}"))
                })?;
                continue;
            }
            let email = render_chat_email(&ChatEmailData {
                installation_id: installation_id.clone(),
                client_label: client_label.clone(),
                messages: events
                    .iter()
                    .map(|(_, created_at, author_label, body)| {
                        Ok(ChatEmailMessageData {
                            created_at: parse_timestamp(created_at.clone(), "chat email message")?,
                            author_label: author_label.clone(),
                            body: body.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>, AppError>>()?,
                dashboard_url: template.dashboard_url.clone(),
            });
            let idempotency_key = format!(
                "chat:{room_id}:{owner_generation}:{}",
                window_started_at.replace([':', '+'], "-")
            );
            let delivery_id = tx
                .query_row(
                    "SELECT id FROM chat_email_deliveries WHERE idempotency_key = ?1",
                    params![idempotency_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!("read existing chat delivery failed: {error}"))
                })?
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            tx.execute(
                "INSERT OR IGNORE INTO chat_email_deliveries (
                    id, room_id, installation_id, client_label, owner_generation,
                    recipient, from_address, reply_to, subject, html_body, text_body,
                    idempotency_key, status, attempts, not_before, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                           'pending', 0, ?13, ?14, ?14)",
                params![
                    delivery_id,
                    room_id,
                    installation_id,
                    client_label,
                    owner_generation,
                    recipient,
                    sender,
                    template.reply_to,
                    email.subject,
                    email.html,
                    email.text,
                    idempotency_key,
                    window_ends_at,
                    now.to_rfc3339(),
                ],
            )
            .map_err(|error| {
                AppError::Internal(format!("insert chat email delivery failed: {error}"))
            })?;
            for (event_id, _, _, _) in &events {
                tx.execute(
                    "INSERT OR IGNORE INTO chat_email_delivery_items (delivery_id, event_id)
                     VALUES (?1, ?2)",
                    params![delivery_id, event_id],
                )
                .map_err(|error| {
                    AppError::Internal(format!("link chat email event failed: {error}"))
                })?;
            }
            let event_ids = events
                .iter()
                .map(|event| event.0.clone())
                .collect::<Vec<_>>();
            for event_id in &event_ids {
                tx.execute(
                    "UPDATE chat_email_events SET status = 'batched', updated_at = ?2
                     WHERE id = ?1 AND status = 'pending'",
                    params![event_id, now.to_rfc3339()],
                )
                .map_err(|error| {
                    AppError::Internal(format!("mark chat event batched failed: {error}"))
                })?;
            }
            stats.deliveries_created += 1;
            stats.events_batched += event_ids.len();
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit chat email aggregation failed: {error}"))
        })?;
        Ok(stats)
    }

    pub async fn claim_client_chat_delivery(
        &self,
        worker_id: &str,
        now: DateTime<Utc>,
        lease_secs: i64,
    ) -> Result<Option<ChatDeliveryClaim>, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat delivery claim failed: {error}"))
            })?;
        let id = tx
            .query_row(
                "SELECT id FROM chat_email_deliveries
                 WHERE (
                     status IN ('pending', 'retry')
                     AND not_before <= ?1
                     AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
                 ) OR (
                     status = 'claimed' AND claim_expires_at IS NOT NULL AND claim_expires_at <= ?1
                 )
                 ORDER BY COALESCE(next_attempt_at, not_before) ASC, created_at ASC, id ASC
                 LIMIT 1",
                params![now.to_rfc3339()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("select chat delivery claim failed: {error}"))
            })?;
        let Some(id) = id else {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit empty chat claim failed: {error}"))
            })?;
            return Ok(None);
        };
        let changed = tx
            .execute(
                "UPDATE chat_email_deliveries
                 SET status = 'claimed', claim_owner = ?2, claim_expires_at = ?3,
                     attempts = attempts + 1, updated_at = ?4
                 WHERE id = ?1 AND (
                     status IN ('pending', 'retry')
                     OR (status = 'claimed' AND claim_expires_at <= ?4)
                 )",
                params![
                    id,
                    worker_id,
                    (now + Duration::seconds(lease_secs.max(1))).to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|error| AppError::Internal(format!("claim chat delivery failed: {error}")))?;
        if changed != 1 {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit lost chat claim failed: {error}"))
            })?;
            return Ok(None);
        }
        let claim = tx
            .query_row(
                "SELECT id, recipient, from_address, reply_to, subject, html_body,
                        text_body, idempotency_key, attempts
                 FROM chat_email_deliveries WHERE id = ?1",
                params![id],
                |row| {
                    Ok(ChatDeliveryClaim {
                        id: row.get(0)?,
                        recipient: row.get(1)?,
                        from: row.get(2)?,
                        reply_to: row.get(3)?,
                        subject: row.get(4)?,
                        html: row.get(5)?,
                        text: row.get(6)?,
                        idempotency_key: row.get(7)?,
                        attempts: row.get::<_, i64>(8)?.max(0) as u32,
                    })
                },
            )
            .map_err(|error| {
                AppError::Internal(format!("read claimed chat delivery failed: {error}"))
            })?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit chat delivery claim failed: {error}"))
        })?;
        Ok(Some(claim))
    }

    pub async fn validate_client_chat_delivery(
        &self,
        delivery_id: &str,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, AppError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat delivery validation failed: {error}"))
            })?;
        let delivery = tx
            .query_row(
                "SELECT d.room_id, d.owner_generation, d.recipient,
                        r.status, r.owner_generation, r.owner_email_snapshot,
                        lower(trim(i.owner_email)), i.owner_verified_at
                 FROM chat_email_deliveries d
                 INNER JOIN chat_rooms r ON r.id = d.room_id
                 LEFT JOIN installations i ON i.id = r.installation_id
                 WHERE d.id = ?1 AND d.status = 'claimed' AND d.claim_owner = ?2",
                params![delivery_id, worker_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read chat delivery validation failed: {error}"))
            })?;
        let Some((
            room_id,
            delivery_generation,
            recipient,
            room_status,
            room_generation,
            room_owner,
            installation_owner,
            owner_verified_at,
        )) = delivery
        else {
            tx.commit().map_err(|error| {
                AppError::Internal(format!("commit missing chat validation failed: {error}"))
            })?;
            return Ok(false);
        };
        let valid = room_status == "active"
            && delivery_generation == room_generation
            && room_owner.eq_ignore_ascii_case(&recipient)
            && installation_owner
                .as_deref()
                .is_some_and(|owner| owner.eq_ignore_ascii_case(&recipient))
            && owner_verified_at.is_some();
        if !valid {
            if let (Some(owner), Some(_)) = (installation_owner, owner_verified_at) {
                ensure_room_for_verified_owner_tx(
                    &tx,
                    &room_id_installation(&tx, &room_id)?,
                    &owner,
                    now,
                )?;
                requeue_room_deliveries_for_current_owner_tx(&tx, &room_id, now)?;
            } else {
                archive_room_for_installation_tx(&tx, &room_id_installation(&tx, &room_id)?, now)?;
            }
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit chat delivery validation failed: {error}"))
        })?;
        Ok(valid)
    }

    pub async fn mark_client_chat_delivery_sent(
        &self,
        delivery_id: &str,
        worker_id: &str,
        provider_message_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        self.finish_client_chat_delivery(
            delivery_id,
            worker_id,
            ChatDeliveryOutcome::Sent(provider_message_id),
            now,
        )
        .await
    }

    pub async fn mark_client_chat_delivery_retry(
        &self,
        delivery_id: &str,
        worker_id: &str,
        error: &str,
        next_attempt_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        self.finish_client_chat_delivery(
            delivery_id,
            worker_id,
            ChatDeliveryOutcome::Retry {
                error,
                next_attempt_at,
            },
            now,
        )
        .await
    }

    pub async fn mark_client_chat_delivery_dead_letter(
        &self,
        delivery_id: &str,
        worker_id: &str,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        self.finish_client_chat_delivery(
            delivery_id,
            worker_id,
            ChatDeliveryOutcome::DeadLetter(error),
            now,
        )
        .await
    }

    async fn finish_client_chat_delivery(
        &self,
        delivery_id: &str,
        worker_id: &str,
        outcome: ChatDeliveryOutcome<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let (status, provider_message_id, error, next_attempt_at) = match outcome {
            ChatDeliveryOutcome::Sent(provider_message_id) => {
                ("sent", Some(provider_message_id), None, None)
            }
            ChatDeliveryOutcome::Retry {
                error,
                next_attempt_at,
            } => ("retry", None, Some(error), Some(next_attempt_at)),
            ChatDeliveryOutcome::DeadLetter(error) => ("dead_letter", None, Some(error), None),
        };
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat delivery finish failed: {error}"))
            })?;
        let changed = tx
            .execute(
                "UPDATE chat_email_deliveries
                 SET status = ?3, provider_message_id = COALESCE(?4, provider_message_id),
                     error_message = ?5, next_attempt_at = ?6,
                     claim_owner = NULL, claim_expires_at = NULL,
                     sent_at = CASE WHEN ?3 = 'sent' THEN ?7 ELSE sent_at END,
                     updated_at = ?7
                 WHERE id = ?1 AND status = 'claimed' AND claim_owner = ?2",
                params![
                    delivery_id,
                    worker_id,
                    status,
                    provider_message_id,
                    error.map(|value| value.chars().take(1_000).collect::<String>()),
                    next_attempt_at.map(|value| value.to_rfc3339()),
                    now.to_rfc3339(),
                ],
            )
            .map_err(|error| AppError::Internal(format!("finish chat delivery failed: {error}")))?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "chat delivery claim is no longer owned by this worker".into(),
            ));
        }
        if status == "sent" {
            tx.execute(
                "UPDATE chat_email_events
                 SET status = 'sent', updated_at = ?2
                 WHERE status = 'batched' AND id IN (
                     SELECT event_id FROM chat_email_delivery_items WHERE delivery_id = ?1
                 )",
                params![delivery_id, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("mark chat events sent failed: {error}"))
            })?;
            tx.execute(
                "INSERT OR IGNORE INTO email_send_logs (
                    id, email_type, to_email, provider_message_id, status, error_message, created_at
                 ) SELECT id, 'client_chat', recipient, provider_message_id, 'sent', NULL, ?2
                   FROM chat_email_deliveries WHERE id = ?1",
                params![delivery_id, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("record sent chat email failed: {error}"))
            })?;
        } else if status == "dead_letter" {
            tx.execute(
                "UPDATE chat_email_events
                 SET status = 'dead_letter', updated_at = ?2
                 WHERE status = 'batched' AND id IN (
                     SELECT event_id FROM chat_email_delivery_items WHERE delivery_id = ?1
                 )",
                params![delivery_id, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("mark chat events dead letter failed: {error}"))
            })?;
        }
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit chat delivery finish failed: {error}"))
        })?;
        Ok(())
    }

    pub async fn list_client_chat_deliveries(
        &self,
        limit: usize,
    ) -> Result<Vec<ClientChatDeliveryView>, AppError> {
        let conn = self.conn.lock().await;
        let mut statement = conn
            .prepare(
                "SELECT d.id, d.room_id, d.installation_id, d.client_label, d.recipient,
                        COUNT(di.event_id), d.status, d.attempts, d.created_at,
                        d.next_attempt_at, d.sent_at, d.error_message
                 FROM chat_email_deliveries d
                 LEFT JOIN chat_email_delivery_items di ON di.delivery_id = d.id
                 GROUP BY d.id
                 ORDER BY d.created_at DESC, d.id DESC LIMIT ?1",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare chat deliveries failed: {error}"))
            })?;
        let rows = statement
            .query_map(params![limit.clamp(1, 100) as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            })
            .map_err(|error| {
                AppError::Internal(format!("query chat deliveries failed: {error}"))
            })?;
        rows.map(|row| {
            let (
                id,
                room_id,
                installation_id,
                client_label,
                recipient,
                count,
                status,
                attempts,
                created_at,
                next_attempt_at,
                sent_at,
                error_message,
            ) = row.map_err(|error| {
                AppError::Internal(format!("read chat delivery failed: {error}"))
            })?;
            Ok(ClientChatDeliveryView {
                id,
                room_id,
                installation_id,
                client_label,
                recipient_masked: mask_email_address(&recipient),
                message_count: count.max(0) as usize,
                status,
                attempts: attempts.max(0) as u32,
                created_at: parse_timestamp(created_at, "chat delivery")?,
                next_attempt_at: parse_optional_timestamp(next_attempt_at, "chat delivery retry")?,
                sent_at: parse_optional_timestamp(sent_at, "chat delivery sent")?,
                error_message: error_message
                    .map(|value| mask_email_like_tokens(&value).chars().take(500).collect()),
            })
        })
        .collect()
    }

    pub async fn requeue_client_chat_delivery(
        &self,
        delivery_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AppError> {
        validate_public_id(delivery_id, "delivery id")?;
        let conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat delivery requeue failed: {error}"))
            })?;
        let changed = tx
            .execute(
                "UPDATE chat_email_deliveries
                 SET status = 'retry', attempts = 0, next_attempt_at = ?2,
                     claim_owner = NULL, claim_expires_at = NULL, error_message = NULL,
                     updated_at = ?2
                 WHERE id = ?1 AND status = 'dead_letter'
                   AND EXISTS (
                       SELECT 1
                       FROM chat_rooms r
                       INNER JOIN installations i ON i.id = r.installation_id
                       WHERE r.id = chat_email_deliveries.room_id
                         AND r.status = 'active'
                         AND r.owner_generation = chat_email_deliveries.owner_generation
                         AND lower(r.owner_email_snapshot) = lower(chat_email_deliveries.recipient)
                         AND i.owner_verified_at IS NOT NULL
                         AND lower(trim(i.owner_email)) = lower(chat_email_deliveries.recipient)
                   )
                   AND EXISTS (
                       SELECT 1 FROM chat_email_delivery_items di
                       WHERE di.delivery_id = chat_email_deliveries.id
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM chat_email_delivery_items di
                       INNER JOIN chat_email_events e ON e.id = di.event_id
                       INNER JOIN chat_messages m ON m.id = e.message_id
                       WHERE di.delivery_id = chat_email_deliveries.id
                         AND (e.status != 'dead_letter' OR m.status != 'visible')
                   )",
                params![delivery_id, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("requeue chat delivery failed: {error}"))
            })?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "only current, visible dead-letter chat deliveries can be requeued".into(),
            ));
        }
        tx.execute(
            "UPDATE chat_email_events
             SET status = 'batched', updated_at = ?2
             WHERE id IN (
                 SELECT event_id FROM chat_email_delivery_items WHERE delivery_id = ?1
             ) AND status = 'dead_letter'",
            params![delivery_id, now.to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("requeue chat events failed: {error}")))?;
        tx.commit().map_err(|error| {
            AppError::Internal(format!("commit chat delivery requeue failed: {error}"))
        })?;
        Ok(())
    }
}

fn cancel_deleted_message_delivery_tx(
    conn: &Connection,
    message_id: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let event_id = conn
        .query_row(
            "SELECT id FROM chat_email_events WHERE message_id = ?1",
            params![message_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read deleted chat event failed: {error}")))?;
    let Some(event_id) = event_id else {
        return Ok(());
    };
    let delivery_ids = {
        let mut statement = conn
            .prepare(
                "SELECT d.id
                 FROM chat_email_deliveries d
                 INNER JOIN chat_email_delivery_items di ON di.delivery_id = d.id
                 WHERE di.event_id = ?1
                   AND d.status IN ('pending', 'retry', 'claimed', 'dead_letter')",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare deleted chat deliveries failed: {error}"))
            })?;
        statement
            .query_map(params![event_id], |row| row.get::<_, String>(0))
            .map_err(|error| {
                AppError::Internal(format!("query deleted chat deliveries failed: {error}"))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AppError::Internal(format!("read deleted chat deliveries failed: {error}"))
            })?
    };
    for delivery_id in delivery_ids {
        conn.execute(
            "UPDATE chat_email_deliveries
             SET status = 'cancelled_message_deleted', claim_owner = NULL,
                 claim_expires_at = NULL, updated_at = ?2
             WHERE id = ?1
               AND status IN ('pending', 'retry', 'claimed', 'dead_letter')",
            params![delivery_id, now.to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("cancel deleted chat delivery failed: {error}"))
        })?;
        conn.execute(
            "UPDATE chat_email_events
             SET status = 'pending', window_started_at = ?2, window_ends_at = ?2, updated_at = ?2
             WHERE status IN ('batched', 'dead_letter') AND id != ?3 AND id IN (
                 SELECT event_id FROM chat_email_delivery_items WHERE delivery_id = ?1
             ) AND EXISTS (
                 SELECT 1 FROM chat_messages m
                 WHERE m.id = message_id AND m.status = 'visible'
             )",
            params![delivery_id, now.to_rfc3339(), event_id],
        )
        .map_err(|error| {
            AppError::Internal(format!("requeue chat delivery siblings failed: {error}"))
        })?;
    }
    conn.execute(
        "UPDATE chat_email_events
         SET status = 'cancelled_message_deleted', updated_at = ?2
         WHERE id = ?1 AND status != 'sent'",
        params![event_id, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("cancel deleted chat event failed: {error}")))?;
    Ok(())
}

fn resolve_client_system_event_room_tx(
    conn: &Connection,
    installation_id: &str,
    now: DateTime<Utc>,
) -> Result<(String, Option<String>), AppError> {
    let owner = conn
        .query_row(
            "SELECT lower(trim(owner_email)),
                    (SELECT id FROM users WHERE email_normalized = lower(trim(i.owner_email)))
             FROM installations i
             WHERE i.id = ?1 AND i.owner_verified_at IS NOT NULL
               AND i.lifecycle = 'active'
               AND i.client_activated_at IS NOT NULL
               AND i.owner_email IS NOT NULL AND trim(i.owner_email) != ''",
            params![installation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read chat event Client failed: {error}")))?;
    if let Some((owner_email, owner_user_id)) = owner {
        let room_id = ensure_room_for_verified_owner_tx(conn, installation_id, &owner_email, now)?;
        return Ok((room_id, owner_user_id));
    }

    let installation_lifecycle = conn
        .query_row(
            "SELECT lifecycle FROM installations WHERE id = ?1",
            params![installation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("check chat event Client failed: {error}")))?;
    if installation_lifecycle.as_deref() == Some("active") {
        return Err(AppError::Conflict("client owner is not verified".into()));
    }
    if installation_lifecycle
        .as_deref()
        .is_some_and(|lifecycle| lifecycle != "fenced")
    {
        return Err(AppError::Internal(
            "Client lifecycle is invalid while resolving an archived chat event".into(),
        ));
    }

    conn.query_row(
        "SELECT id,
                COALESCE(owner_user_id_snapshot,
                         (SELECT u.id FROM users u
                          WHERE u.email_normalized = lower(trim(r.owner_email_snapshot))))
         FROM chat_rooms r
         WHERE installation_id = ?1 AND status = 'archived'
         ORDER BY owner_generation DESC LIMIT 1",
        params![installation_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    )
    .optional()
    .map_err(|error| {
        AppError::Internal(format!("read archived chat event Client failed: {error}"))
    })?
    .ok_or_else(|| AppError::Conflict("client chat room is unavailable".into()))
}

fn materialize_client_system_event_tx(
    conn: &Connection,
    outbox_id: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let event = conn
        .query_row(
            "SELECT installation_id, source_kind, source_event_id, event_type,
                    payload_json, follower_user_ids_json, created_at
             FROM client_chat_system_outbox WHERE id = ?1 AND status = 'processing'",
            params![outbox_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read client chat event failed: {error}")))?
        .ok_or_else(|| AppError::NotFound("client chat event not found".into()))?;
    let (
        installation_id,
        source_kind,
        source_event_id,
        event_type,
        payload_json,
        follower_user_ids_json,
        created_at,
    ) = event;
    let payload = serde_json::from_str::<serde_json::Value>(&payload_json)
        .map_err(|error| AppError::Internal(format!("parse client chat event failed: {error}")))?;
    let payload = sanitize_system_event_payload(payload)?;
    let payload = if is_public_market_source_kind(&source_kind) {
        public_market_event_payload(&payload)
    } else {
        payload
    };
    let payload_json = serde_json::to_string(&payload)
        .map_err(|error| AppError::Internal(format!("encode client chat event failed: {error}")))?;
    let mut follower_user_ids = serde_json::from_str::<Vec<String>>(&follower_user_ids_json)
        .map_err(|error| AppError::Internal(format!("parse chat followers failed: {error}")))?;
    let (room_id, owner_user_id) =
        resolve_client_system_event_room_tx(conn, &installation_id, now)?;
    if let Some(owner_user_id) = owner_user_id {
        follower_user_ids.push(owner_user_id);
    }
    follower_user_ids.sort();
    follower_user_ids.dedup();
    let previous_latest_seq = latest_visible_seq(conn, &room_id)?;
    for user_id in follower_user_ids
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        conn.execute(
            "INSERT INTO chat_visits (
                user_id, room_id, first_opened_at, last_opened_at, last_read_seq, updated_at
             ) VALUES (?1, ?2, ?3, ?3, ?4, ?3)
             ON CONFLICT(user_id, room_id) DO UPDATE SET
                last_opened_at = excluded.last_opened_at,
                updated_at = excluded.updated_at",
            params![user_id, room_id, now.to_rfc3339(), previous_latest_seq],
        )
        .map_err(|error| AppError::Internal(format!("follow client chat room failed: {error}")))?;
    }
    let message_source_id = format!("{source_kind}:{source_event_id}:{installation_id}");
    let payload_version = if is_public_market_source_kind(&source_kind) {
        2
    } else {
        1
    };
    let body = payload
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&event_type);
    let message_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO chat_messages (
            id, room_id, author_user_id, author_email, author_label, client_message_id,
            author_kind, message_kind, event_type, event_payload_json, payload_version,
            source_event_id, body, status, created_at, updated_at
         ) VALUES (?1, ?2, 'system', '', 'System message', ?3,
                   'system', 'market_event', ?4, ?5, ?8, ?3, ?6,
                   'visible', ?7, ?7)",
        params![
            message_id,
            room_id,
            message_source_id,
            event_type,
            payload_json,
            body,
            created_at,
            payload_version,
        ],
    )
    .map_err(|error| {
        AppError::Internal(format!("materialize client chat event failed: {error}"))
    })?;
    let stored_message_id = conn
        .query_row(
            "SELECT id FROM chat_messages WHERE source_event_id = ?1",
            params![message_source_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "read materialized client chat event failed: {error}"
            ))
        })?;
    publish_system_event_payment_assets_tx(
        conn,
        &room_id,
        &stored_message_id,
        &payload,
        &created_at,
    )?;
    conn.execute(
        "UPDATE chat_rooms
         SET last_message_at = CASE
                 WHEN last_message_at IS NULL OR last_message_at < ?2 THEN ?2
                 ELSE last_message_at END,
             updated_at = CASE WHEN updated_at < ?2 THEN ?2 ELSE updated_at END
         WHERE id = ?1",
        params![room_id, created_at],
    )
    .map_err(|error| AppError::Internal(format!("update client chat activity failed: {error}")))?;
    conn.execute(
        "UPDATE client_chat_system_outbox
         SET status = 'completed', next_attempt_at = NULL,
             last_error = NULL, updated_at = ?2, completed_at = ?2
         WHERE id = ?1",
        params![outbox_id, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("complete client chat outbox failed: {error}")))?;
    Ok(())
}

fn publish_system_event_payment_assets_tx(
    conn: &Connection,
    room_id: &str,
    message_id: &str,
    payload: &serde_json::Value,
    created_at: &str,
) -> Result<(), AppError> {
    let allowed_owners = ["ownerUserId", "providerUserId", "supplierUserId"]
        .iter()
        .filter_map(|field| payload.get(*field).and_then(serde_json::Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .collect::<HashSet<_>>();
    if allowed_owners.is_empty() {
        return Ok(());
    }
    let mut asset_ids = HashSet::new();
    collect_system_event_payment_asset_ids(payload, &mut asset_ids);
    for asset_id in asset_ids {
        let asset_owner = conn
            .query_row(
                "SELECT user_id FROM account_payment_assets WHERE id = ?1",
                params![asset_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read system event payment asset failed: {error}"))
            })?;
        if !asset_owner
            .as_deref()
            .is_some_and(|owner| allowed_owners.contains(owner))
        {
            continue;
        }
        conn.execute(
            "INSERT OR IGNORE INTO chat_public_payment_assets (
                asset_id, message_id, room_id, created_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![asset_id, message_id, room_id, created_at],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "publish system event payment asset failed: {error}"
            ))
        })?;
    }
    Ok(())
}

fn collect_system_event_payment_asset_ids<'a>(
    value: &'a serde_json::Value,
    output: &mut HashSet<&'a str>,
) {
    match value {
        serde_json::Value::Object(object) => {
            for value in object.values() {
                collect_system_event_payment_asset_ids(value, output);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_system_event_payment_asset_ids(value, output);
            }
        }
        serde_json::Value::String(value) => {
            if let Some(asset_id) = value.strip_prefix("/v1/account/payment-assets/")
                && !asset_id.is_empty()
                && asset_id.len() <= 128
                && asset_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                output.insert(asset_id);
            }
        }
        _ => {}
    }
}

fn room_id_installation(conn: &Connection, room_id: &str) -> Result<String, AppError> {
    conn.query_row(
        "SELECT installation_id FROM chat_rooms WHERE id = ?1",
        params![room_id],
        |row| row.get::<_, String>(0),
    )
    .map_err(|error| AppError::Internal(format!("read chat room installation failed: {error}")))
}

fn normalize_chat_body(body: &str) -> Result<String, AppError> {
    let normalized = body.trim();
    if normalized.is_empty() {
        return Err(AppError::BadRequest("message body is required".into()));
    }
    if normalized.chars().count() > CHAT_MAX_BODY_CHARS {
        return Err(AppError::BadRequest(format!(
            "message body cannot exceed {CHAT_MAX_BODY_CHARS} characters"
        )));
    }
    if normalized
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(AppError::BadRequest(
            "message body contains unsupported control characters".into(),
        ));
    }
    Ok(normalized.to_string())
}

fn email_local_part(email: &str) -> Result<String, AppError> {
    email
        .split_once('@')
        .map(|(local, _)| local)
        .filter(|local| !local.is_empty())
        .map(str::to_string)
        .ok_or_else(|| AppError::Internal("authenticated session has an invalid email".into()))
}

fn validate_public_id(value: &str, label: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(AppError::BadRequest(format!("invalid {label}")));
    }
    Ok(())
}

fn parse_timestamp(value: String, field: &str) -> Result<DateTime<Utc>, AppError> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| AppError::Internal(format!("invalid {field} timestamp: {error}")))
}

fn parse_optional_timestamp(
    value: Option<String>,
    field: &str,
) -> Result<Option<DateTime<Utc>>, AppError> {
    value.map(|value| parse_timestamp(value, field)).transpose()
}

fn load_room_by_installation(
    conn: &Connection,
    installation_id: &str,
    viewer_user_id: Option<&str>,
) -> Result<Option<ClientChatRoomView>, AppError> {
    let room_id = conn
        .query_row(
            "SELECT id FROM chat_rooms
             WHERE installation_id = ?1
             ORDER BY CASE status WHEN 'active' THEN 0 ELSE 1 END, owner_generation DESC
             LIMIT 1",
            params![installation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read client chat room failed: {error}")))?;
    room_id
        .map(|room_id| load_room_by_id(conn, &room_id, viewer_user_id))
        .transpose()
        .map(Option::flatten)
}

fn load_room_by_id(
    conn: &Connection,
    room_id: &str,
    viewer_user_id: Option<&str>,
) -> Result<Option<ClientChatRoomView>, AppError> {
    let row = conn
        .query_row(
            "SELECT r.id, r.installation_id,
                    COALESCE(NULLIF(t.subdomain, ''), r.client_label_snapshot),
                    r.status, r.last_message_at, r.archived_at
             FROM chat_rooms r
             LEFT JOIN installation_client_tunnels t ON t.installation_id = r.installation_id
             WHERE r.id = ?1",
            params![room_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read chat room summary failed: {error}")))?;
    let Some((id, installation_id, client_label, status, last_message_at, archived_at)) = row
    else {
        return Ok(None);
    };
    let can_post = status == "active" && viewer_user_id.is_some();
    let latest_seq = latest_visible_seq(conn, &id)?;
    let last_message = latest_visible_message_preview(conn, &id)?;
    let unread_count = if let Some(user_id) = viewer_user_id {
        count_unread_messages(conn, &id, user_id)?
    } else {
        0
    };
    Ok(Some(ClientChatRoomView {
        id,
        installation_id,
        client_label,
        status,
        can_post,
        read_only: !can_post,
        latest_seq,
        unread_count,
        last_message_at: parse_optional_timestamp(last_message_at, "chat room activity")?,
        last_message,
        archived_at: parse_optional_timestamp(archived_at, "chat room archive")?,
    }))
}

fn latest_visible_seq(conn: &Connection, room_id: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM chat_messages WHERE room_id = ?1",
        params![room_id],
        |row| row.get(0),
    )
    .map_err(|error| AppError::Internal(format!("read latest chat sequence failed: {error}")))
}

fn latest_visible_message_preview(
    conn: &Connection,
    room_id: &str,
) -> Result<Option<ClientChatMessagePreview>, AppError> {
    let sql = "SELECT seq, body, author_label, author_kind, message_kind, event_type,
                      event_payload_json, status, created_at
               FROM chat_messages WHERE room_id = ?1 ORDER BY seq DESC LIMIT 1";
    let read = |row: &crate::db::Row<'_>| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
        ))
    };
    let stored = conn
        .query_row(sql, params![room_id], read)
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("read chat message preview failed: {error}"))
        })?;
    stored
        .map(
            |(
                seq,
                body,
                author_label,
                author_kind,
                message_kind,
                event_type,
                event_payload,
                status,
                created_at,
            )| {
                Ok(ClientChatMessagePreview {
                    seq,
                    body: if status == "visible" {
                        body
                    } else {
                        String::new()
                    },
                    author_label,
                    author_kind,
                    message_kind,
                    event_type,
                    event_payload: parse_event_payload(event_payload)?,
                    created_at: parse_timestamp(created_at, "chat message")?,
                })
            },
        )
        .transpose()
}

fn count_unread_messages(
    conn: &Connection,
    room_id: &str,
    user_id: &str,
) -> Result<usize, AppError> {
    let sql = "SELECT COUNT(*) FROM chat_messages m
         WHERE m.room_id = ?1 AND m.status = 'visible' AND m.author_kind = 'user'
           AND m.author_user_id != ?2
           AND m.seq > COALESCE((SELECT last_read_seq FROM chat_visits
                                 WHERE user_id = ?2 AND room_id = ?1), 0)";
    conn.query_row(sql, params![room_id, user_id], |row| row.get::<_, i64>(0))
        .map(|count| count.max(0) as usize)
        .map_err(|error| AppError::Internal(format!("count unread chat messages failed: {error}")))
}

fn parse_event_payload(value: Option<String>) -> Result<Option<serde_json::Value>, AppError> {
    value
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| {
                AppError::Internal(format!("parse chat event payload failed: {error}"))
            })
        })
        .transpose()
}

fn count_visible_messages_after(
    conn: &Connection,
    room_id: &str,
    last_read_seq: i64,
) -> Result<usize, AppError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM chat_messages
         WHERE room_id = ?1 AND status = 'visible' AND author_kind = 'user' AND seq > ?2",
        params![room_id, last_read_seq.max(0)],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count.max(0) as usize)
    .map_err(|error| {
        AppError::Internal(format!("count public unread chat messages failed: {error}"))
    })
}

fn query_messages<P: crate::db::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
    viewer_user_id: Option<&str>,
) -> Result<Vec<ClientChatMessageView>, AppError> {
    let mut statement = conn
        .prepare(sql)
        .map_err(|error| AppError::Internal(format!("prepare chat messages failed: {error}")))?;
    let rows = statement
        .query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
            ))
        })
        .map_err(|error| AppError::Internal(format!("query chat messages failed: {error}")))?;
    rows.map(|row| {
        let (
            id,
            seq,
            body,
            author_user_id,
            author_label,
            author_kind,
            message_kind,
            event_type,
            event_payload,
            status,
            created_at,
        ) =
            row.map_err(|error| AppError::Internal(format!("read chat message failed: {error}")))?;
        Ok(ClientChatMessageView {
            id,
            seq,
            body: if status == "visible" {
                body
            } else {
                String::new()
            },
            author_label,
            author_kind,
            message_kind,
            event_type,
            event_payload: parse_event_payload(event_payload)?,
            is_mine: viewer_user_id == Some(author_user_id.as_str()),
            status,
            created_at: parse_timestamp(created_at, "chat message")?,
        })
    })
    .collect()
}

fn upsert_visit_tx(
    conn: &Connection,
    user_id: &str,
    room_id: &str,
    last_read_seq: Option<i64>,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO chat_visits (
            user_id, room_id, first_opened_at, last_opened_at, last_read_seq, updated_at
         ) VALUES (?1, ?2, ?3, ?3, ?4, ?3)
         ON CONFLICT(user_id, room_id) DO UPDATE SET
            last_opened_at = excluded.last_opened_at,
            last_read_seq = MAX(chat_visits.last_read_seq, excluded.last_read_seq),
            updated_at = excluded.updated_at",
        params![
            user_id,
            room_id,
            now.to_rfc3339(),
            last_read_seq.unwrap_or(0).max(0)
        ],
    )
    .map_err(|error| AppError::Internal(format!("record chat room visit failed: {error}")))?;
    Ok(())
}

fn consume_chat_rate_limit_tx(
    conn: &Connection,
    scope: &str,
    bucket_secs: i64,
    limit: i64,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let bucket_start = now.timestamp().div_euclid(bucket_secs) * bucket_secs;
    conn.execute(
        "INSERT INTO chat_rate_limit (scope, bucket_start, count)
         VALUES (?1, ?2, 1)
         ON CONFLICT(scope, bucket_start) DO UPDATE SET count = count + 1",
        params![scope, bucket_start],
    )
    .map_err(|error| AppError::Internal(format!("update chat rate limit failed: {error}")))?;
    let count = conn
        .query_row(
            "SELECT count FROM chat_rate_limit WHERE scope = ?1 AND bucket_start = ?2",
            params![scope, bucket_start],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| AppError::Internal(format!("read chat rate limit failed: {error}")))?;
    if count > limit {
        return Err(AppError::RateLimited {
            message: "chat message rate limit exceeded".into(),
            retry_after_secs: (bucket_start + bucket_secs - now.timestamp()).max(1) as u64,
        });
    }
    Ok(())
}

fn load_idempotent_message(
    conn: &Connection,
    room_id: &str,
    user_id: &str,
    client_message_id: &str,
) -> Result<Option<ClientChatMessageView>, AppError> {
    conn.query_row(
        "SELECT id, seq, body, author_label, author_kind, message_kind,
                event_type, event_payload_json, status, created_at
         FROM chat_messages
         WHERE room_id = ?1 AND author_user_id = ?2 AND client_message_id = ?3",
        params![room_id, user_id, client_message_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        },
    )
    .optional()
    .map_err(|error| AppError::Internal(format!("read idempotent chat message failed: {error}")))?
    .map(
        |(
            id,
            seq,
            body,
            author_label,
            author_kind,
            message_kind,
            event_type,
            event_payload,
            status,
            created_at,
        )| {
            Ok(ClientChatMessageView {
                id,
                seq,
                body: if status == "visible" {
                    body
                } else {
                    String::new()
                },
                author_label,
                author_kind,
                message_kind,
                event_type,
                event_payload: parse_event_payload(event_payload)?,
                is_mine: true,
                status,
                created_at: parse_timestamp(created_at, "chat message")?,
            })
        },
    )
    .transpose()
}

fn insert_chat_email_event_tx(
    conn: &Connection,
    message_id: &str,
    room_id: &str,
    installation_id: &str,
    owner_generation: i64,
    recipient: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let window = conn
        .query_row(
            "SELECT window_started_at, window_ends_at
             FROM chat_email_events
             WHERE room_id = ?1 AND owner_generation = ?2 AND lower(recipient) = lower(?3)
               AND status = 'pending' AND window_ends_at > ?4
             ORDER BY window_started_at DESC LIMIT 1",
            params![room_id, owner_generation, recipient, now.to_rfc3339()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| {
            AppError::Internal(format!("read open chat email window failed: {error}"))
        })?;
    let (window_started_at, window_ends_at) = window.unwrap_or_else(|| {
        (
            now.to_rfc3339(),
            (now + Duration::seconds(CHAT_EMAIL_BATCH_WINDOW_SECS)).to_rfc3339(),
        )
    });
    conn.execute(
        "INSERT INTO chat_email_events (
            id, message_id, room_id, installation_id, owner_generation,
            recipient, status, window_started_at, window_ends_at, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?9, ?9)",
        params![
            Uuid::new_v4().to_string(),
            message_id,
            room_id,
            installation_id,
            owner_generation,
            recipient,
            window_started_at,
            window_ends_at,
            now.to_rfc3339(),
        ],
    )
    .map_err(|error| AppError::Internal(format!("insert chat email event failed: {error}")))?;
    Ok(())
}

fn cancel_room_deliveries_tx(
    conn: &Connection,
    room_id: &str,
    status: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE chat_email_deliveries
         SET status = ?2, claim_owner = NULL, claim_expires_at = NULL, updated_at = ?3
         WHERE room_id = ?1
           AND status IN ('pending', 'retry', 'claimed', 'dead_letter')",
        params![room_id, status, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("cancel chat email deliveries failed: {error}")))?;
    conn.execute(
        "UPDATE chat_email_events
         SET status = ?2, updated_at = ?3
         WHERE room_id = ?1 AND status IN ('pending', 'batched', 'dead_letter')",
        params![room_id, status, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("cancel chat email events failed: {error}")))?;
    Ok(())
}

fn requeue_room_deliveries_for_current_owner_tx(
    conn: &Connection,
    room_id: &str,
    now: DateTime<Utc>,
) -> Result<(), AppError> {
    let current = conn
        .query_row(
            "SELECT r.owner_email_snapshot, r.owner_generation, r.status
             FROM chat_rooms r WHERE r.id = ?1",
            params![room_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read current chat owner failed: {error}")))?;
    let Some((owner_email, owner_generation, status)) = current else {
        return Ok(());
    };
    if status != "active" {
        return cancel_room_deliveries_tx(conn, room_id, "cancelled_room_archived", now);
    }

    conn.execute(
        "UPDATE chat_email_deliveries
         SET status = 'cancelled_owner_changed', claim_owner = NULL,
             claim_expires_at = NULL, updated_at = ?2
         WHERE room_id = ?1
           AND status IN ('pending', 'retry', 'claimed', 'dead_letter')",
        params![room_id, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("cancel stale chat deliveries failed: {error}")))?;
    conn.execute(
        "UPDATE chat_email_events
         SET recipient = ?2, owner_generation = ?3,
             status = CASE
                 WHEN (SELECT m.status FROM chat_messages m WHERE m.id = message_id) != 'visible'
                 THEN 'cancelled_message_deleted'
                 WHEN lower((SELECT m.author_email FROM chat_messages m WHERE m.id = message_id)) = lower(?2)
                 THEN 'cancelled_owner_now'
                 ELSE 'pending'
             END,
             window_started_at = ?4, window_ends_at = ?5, updated_at = ?4
         WHERE room_id = ?1 AND status IN ('pending', 'batched', 'dead_letter')",
        params![
            room_id,
            owner_email,
            owner_generation,
            now.to_rfc3339(),
            (now + Duration::seconds(CHAT_EMAIL_BATCH_WINDOW_SECS)).to_rfc3339(),
        ],
    )
    .map_err(|error| AppError::Internal(format!("requeue chat events for owner failed: {error}")))?;
    Ok(())
}
