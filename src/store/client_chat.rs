use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::AppStore;
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

pub(super) fn init_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS client_chat_rooms (
            id TEXT PRIMARY KEY,
            installation_id TEXT NOT NULL UNIQUE,
            client_label_snapshot TEXT NOT NULL,
            owner_email_snapshot TEXT NOT NULL,
            owner_generation INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL DEFAULT 'active'
                CHECK (status IN ('active', 'archived')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_message_at TEXT,
            archived_at TEXT,
            delete_after TEXT
        );

        CREATE TABLE IF NOT EXISTS client_chat_messages (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            room_id TEXT NOT NULL,
            author_user_id TEXT NOT NULL,
            author_email TEXT NOT NULL,
            author_label TEXT NOT NULL,
            client_message_id TEXT NOT NULL,
            body TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'visible'
                CHECK (status IN ('visible', 'deleted')),
            deleted_by TEXT,
            deleted_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE (room_id, author_user_id, client_message_id),
            FOREIGN KEY (room_id) REFERENCES client_chat_rooms(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS client_chat_visits (
            user_id TEXT NOT NULL,
            room_id TEXT NOT NULL,
            first_opened_at TEXT NOT NULL,
            last_opened_at TEXT NOT NULL,
            last_read_seq INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (user_id, room_id),
            FOREIGN KEY (room_id) REFERENCES client_chat_rooms(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS client_chat_rate_limit (
            scope TEXT NOT NULL,
            bucket_start INTEGER NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (scope, bucket_start)
        );

        CREATE TABLE IF NOT EXISTS client_chat_email_events (
            id TEXT PRIMARY KEY,
            message_id TEXT NOT NULL UNIQUE,
            room_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            owner_generation INTEGER NOT NULL,
            recipient TEXT NOT NULL,
            status TEXT NOT NULL,
            window_started_at TEXT NOT NULL,
            window_ends_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (message_id) REFERENCES client_chat_messages(id) ON DELETE CASCADE,
            FOREIGN KEY (room_id) REFERENCES client_chat_rooms(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS client_chat_email_deliveries (
            id TEXT PRIMARY KEY,
            room_id TEXT NOT NULL,
            installation_id TEXT NOT NULL,
            client_label TEXT NOT NULL,
            owner_generation INTEGER NOT NULL,
            recipient TEXT NOT NULL,
            from_address TEXT NOT NULL,
            reply_to TEXT,
            subject TEXT NOT NULL,
            html_body TEXT NOT NULL,
            text_body TEXT NOT NULL,
            idempotency_key TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            not_before TEXT NOT NULL,
            next_attempt_at TEXT,
            claim_owner TEXT,
            claim_expires_at TEXT,
            provider_message_id TEXT,
            error_message TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            sent_at TEXT,
            FOREIGN KEY (room_id) REFERENCES client_chat_rooms(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS client_chat_email_delivery_items (
            delivery_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            PRIMARY KEY (delivery_id, event_id),
            FOREIGN KEY (delivery_id) REFERENCES client_chat_email_deliveries(id) ON DELETE CASCADE,
            FOREIGN KEY (event_id) REFERENCES client_chat_email_events(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_client_chat_rooms_status
            ON client_chat_rooms(status, delete_after);
        CREATE INDEX IF NOT EXISTS idx_client_chat_messages_room_seq
            ON client_chat_messages(room_id, seq DESC);
        CREATE INDEX IF NOT EXISTS idx_client_chat_messages_author
            ON client_chat_messages(author_user_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_client_chat_visits_user_recent
            ON client_chat_visits(user_id, last_opened_at DESC);
        CREATE INDEX IF NOT EXISTS idx_client_chat_events_pending
            ON client_chat_email_events(status, window_ends_at, room_id, owner_generation);
        CREATE INDEX IF NOT EXISTS idx_client_chat_delivery_claim
            ON client_chat_email_deliveries(status, next_attempt_at, not_before, claim_expires_at);
        CREATE INDEX IF NOT EXISTS idx_client_chat_delivery_recent
            ON client_chat_email_deliveries(created_at DESC, id DESC);
        ",
    )
    .map_err(|error| {
        AppError::Internal(format!("initialize client chat schema failed: {error}"))
    })?;

    let now = Utc::now();
    conn.execute(
        "INSERT OR IGNORE INTO client_chat_rooms (
            id, installation_id, client_label_snapshot, owner_email_snapshot,
            owner_generation, status, created_at, updated_at
         )
         SELECT lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
                substr(lower(hex(randomblob(2))), 2) || '-' ||
                substr('89ab', abs(random()) % 4 + 1, 1) ||
                substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))),
                i.id,
                COALESCE(NULLIF(t.subdomain, ''), i.platform || ' · ' || substr(i.id, 1, 8)),
                lower(trim(i.owner_email)), 1, 'active', ?1, ?1
         FROM installations i
         LEFT JOIN installation_client_tunnels t ON t.installation_id = i.id
         WHERE i.owner_verified_at IS NOT NULL
           AND i.owner_email IS NOT NULL
           AND trim(i.owner_email) != ''",
        params![now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("backfill client chat rooms failed: {error}")))?;

    let verified_clients = {
        let mut statement = conn
            .prepare(
                "SELECT id, lower(trim(owner_email))
                 FROM installations
                 WHERE owner_verified_at IS NOT NULL
                   AND owner_email IS NOT NULL
                   AND trim(owner_email) != ''",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare verified chat clients failed: {error}"))
            })?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| {
                AppError::Internal(format!("query verified chat clients failed: {error}"))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AppError::Internal(format!("read verified chat clients failed: {error}"))
            })?
    };
    for (installation_id, owner_email) in verified_clients {
        ensure_room_for_verified_owner_tx(conn, &installation_id, &owner_email, now)?;
    }

    let ownerless_active_installations = {
        let mut statement = conn
            .prepare(
                "SELECT r.installation_id
                 FROM client_chat_rooms r
                 LEFT JOIN installations i ON i.id = r.installation_id
                 WHERE r.status = 'active'
                   AND (i.id IS NULL OR i.owner_verified_at IS NULL
                        OR i.owner_email IS NULL OR trim(i.owner_email) = '')",
            )
            .map_err(|error| {
                AppError::Internal(format!("prepare ownerless chat rooms failed: {error}"))
            })?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| {
                AppError::Internal(format!("query ownerless chat rooms failed: {error}"))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AppError::Internal(format!("read ownerless chat rooms failed: {error}"))
            })?
    };
    for installation_id in ownerless_active_installations {
        archive_room_for_installation_tx(conn, &installation_id, now)?;
    }
    Ok(())
}

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
            "SELECT id, owner_email_snapshot FROM client_chat_rooms WHERE installation_id = ?1",
            params![installation_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read client chat room failed: {error}")))?;
    if let Some((room_id, existing_owner)) = existing {
        let owner_changed = !existing_owner.eq_ignore_ascii_case(&owner_email);
        conn.execute(
            "UPDATE client_chat_rooms
             SET client_label_snapshot = ?2,
                 owner_email_snapshot = ?3,
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
        "INSERT INTO client_chat_rooms (
            id, installation_id, client_label_snapshot, owner_email_snapshot,
            owner_generation, status, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, 1, 'active', ?5, ?5)",
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
            "SELECT id FROM client_chat_rooms WHERE installation_id = ?1",
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
        "UPDATE client_chat_rooms
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

pub(super) fn cleanup_expired_rooms_tx(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<usize, AppError> {
    let deleted = conn
        .execute(
            "DELETE FROM client_chat_rooms
         WHERE status = 'archived' AND delete_after IS NOT NULL AND delete_after <= ?1",
            params![now.to_rfc3339()],
        )
        .map_err(|error| {
            AppError::Internal(format!("delete expired chat rooms failed: {error}"))
        })?;
    conn.execute(
        "DELETE FROM client_chat_rate_limit WHERE bucket_start < ?1",
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
        let mut conn = self.conn.lock().await;
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

    pub async fn lookup_client_chat_rooms(
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

    pub async fn list_visited_client_chat_rooms(
        &self,
        user_id: &str,
    ) -> Result<ClientChatRoomListResponse, AppError> {
        let conn = self.conn.lock().await;
        let room_ids = {
            let mut statement = conn
                .prepare(
                    "SELECT v.room_id
                     FROM client_chat_visits v
                     INNER JOIN client_chat_rooms r ON r.id = v.room_id
                     WHERE v.user_id = ?1
                     ORDER BY COALESCE(r.last_message_at, v.last_opened_at) DESC, r.id DESC
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
        let exists = conn
            .query_row(
                "SELECT 1 FROM client_chat_rooms WHERE id = ?1",
                params![room_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("check chat room failed: {error}")))?
            .is_some();
        if !exists {
            return Err(AppError::NotFound("client chat room not found".into()));
        }
        upsert_visit_tx(&conn, user_id, room_id, None, now)?;
        load_room_by_id(&conn, room_id, Some(user_id))?
            .ok_or_else(|| AppError::NotFound("client chat room not found".into()))
    }

    pub async fn import_client_chat_visits(
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
        let mut conn = self.conn.lock().await;
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
                    "SELECT id, COALESCE((SELECT MAX(seq) FROM client_chat_messages WHERE room_id = r.id), 0)
                     FROM client_chat_rooms r WHERE installation_id = ?1",
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
        let latest_seq = conn
            .query_row(
                "SELECT COALESCE((
                    SELECT MAX(m.seq) FROM client_chat_messages m WHERE m.room_id = r.id
                 ), 0)
                 FROM client_chat_rooms r WHERE r.id = ?1",
                params![room_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read latest chat sequence failed: {error}"))
            })?
            .ok_or_else(|| AppError::NotFound("client chat room not found".into()))?;
        let next = last_read_seq.clamp(0, latest_seq);
        upsert_visit_tx(&conn, user_id, room_id, Some(next), now)?;
        Ok(ClientChatReadResponse {
            ok: true,
            last_read_seq: next,
        })
    }

    pub async fn list_client_chat_messages(
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
        let exists = conn
            .query_row(
                "SELECT 1 FROM client_chat_rooms WHERE id = ?1",
                params![room_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("check chat room failed: {error}")))?
            .is_some();
        if !exists {
            return Err(AppError::NotFound("client chat room not found".into()));
        }
        let limit = limit.clamp(1, CHAT_MESSAGE_PAGE_MAX);
        let fetch_limit = (limit + 1) as i64;
        let mut messages = if let Some(after_seq) = after_seq {
            query_messages(
                &conn,
                "SELECT id, seq, body, author_user_id, author_label, status, created_at
                 FROM client_chat_messages
                 WHERE room_id = ?1 AND seq > ?2
                 ORDER BY seq ASC LIMIT ?3",
                params![room_id, after_seq.max(0), fetch_limit],
                viewer_user_id,
            )?
        } else if let Some(before_seq) = before_seq {
            let mut rows = query_messages(
                &conn,
                "SELECT id, seq, body, author_user_id, author_label, status, created_at
                 FROM client_chat_messages
                 WHERE room_id = ?1 AND seq < ?2
                 ORDER BY seq DESC LIMIT ?3",
                params![room_id, before_seq.max(0), fetch_limit],
                viewer_user_id,
            )?;
            rows.reverse();
            rows
        } else {
            let mut rows = query_messages(
                &conn,
                "SELECT id, seq, body, author_user_id, author_label, status, created_at
                 FROM client_chat_messages
                 WHERE room_id = ?1
                 ORDER BY seq DESC LIMIT ?2",
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
        let latest_seq = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM client_chat_messages WHERE room_id = ?1",
                params![room_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| {
                AppError::Internal(format!("read latest chat sequence failed: {error}"))
            })?;
        Ok(ClientChatMessageListResponse {
            messages,
            latest_seq,
            has_more,
        })
    }

    pub async fn get_client_chat_room_latest_seq(&self, room_id: &str) -> Result<i64, AppError> {
        validate_public_id(room_id, "room id")?;
        let conn = self.conn.lock().await;
        let exists = conn
            .query_row(
                "SELECT 1 FROM client_chat_rooms WHERE id = ?1",
                params![room_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| AppError::Internal(format!("check chat room failed: {error}")))?
            .is_some();
        if !exists {
            return Err(AppError::NotFound("client chat room not found".into()));
        }
        conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM client_chat_messages WHERE room_id = ?1",
            params![room_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| AppError::Internal(format!("read latest chat sequence failed: {error}")))
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
        let mut conn = self.conn.lock().await;
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
                 FROM client_chat_rooms r
                 LEFT JOIN installations i ON i.id = r.installation_id
                 WHERE r.id = ?1 AND r.status = 'active'",
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
            .ok_or_else(|| {
                AppError::Conflict("client chat room is archived or unavailable".into())
            })?;
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
            "INSERT INTO client_chat_messages (
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
            "UPDATE client_chat_rooms SET last_message_at = ?2, updated_at = ?2 WHERE id = ?1",
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
            is_mine: true,
            status: "visible".into(),
            created_at: now,
        })
    }

    pub async fn delete_client_chat_message(
        &self,
        message_id: &str,
        deleted_by: &str,
    ) -> Result<ClientChatMessageView, AppError> {
        validate_public_id(message_id, "message id")?;
        let now = Utc::now();
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| AppError::Internal(format!("begin chat delete failed: {error}")))?;
        let message = tx
            .query_row(
                "SELECT id, seq, body, author_label, status, created_at
                 FROM client_chat_messages WHERE id = ?1",
                params![message_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read chat message for delete failed: {error}"))
            })?
            .ok_or_else(|| AppError::NotFound("chat message not found".into()))?;
        if message.4 != "deleted" {
            tx.execute(
                "UPDATE client_chat_messages
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
            is_mine: false,
            status: "deleted".into(),
            created_at: parse_timestamp(message.5, "chat message")?,
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
        let mut conn = self.conn.lock().await;
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
                     FROM client_chat_email_events
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
                     FROM client_chat_rooms r
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
                         FROM client_chat_email_events e
                         INNER JOIN client_chat_messages m ON m.id = e.message_id
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
                    "UPDATE client_chat_email_events
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
                    "SELECT id FROM client_chat_email_deliveries WHERE idempotency_key = ?1",
                    params![idempotency_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| {
                    AppError::Internal(format!("read existing chat delivery failed: {error}"))
                })?
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            tx.execute(
                "INSERT OR IGNORE INTO client_chat_email_deliveries (
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
                    "INSERT OR IGNORE INTO client_chat_email_delivery_items (delivery_id, event_id)
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
                    "UPDATE client_chat_email_events SET status = 'batched', updated_at = ?2
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
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat delivery claim failed: {error}"))
            })?;
        let id = tx
            .query_row(
                "SELECT id FROM client_chat_email_deliveries
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
                "UPDATE client_chat_email_deliveries
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
                 FROM client_chat_email_deliveries WHERE id = ?1",
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
        let mut conn = self.conn.lock().await;
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
                 FROM client_chat_email_deliveries d
                 INNER JOIN client_chat_rooms r ON r.id = d.room_id
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
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat delivery finish failed: {error}"))
            })?;
        let changed = tx
            .execute(
                "UPDATE client_chat_email_deliveries
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
                "UPDATE client_chat_email_events
                 SET status = 'sent', updated_at = ?2
                 WHERE status = 'batched' AND id IN (
                     SELECT event_id FROM client_chat_email_delivery_items WHERE delivery_id = ?1
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
                   FROM client_chat_email_deliveries WHERE id = ?1",
                params![delivery_id, now.to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!("record sent chat email failed: {error}"))
            })?;
        } else if status == "dead_letter" {
            tx.execute(
                "UPDATE client_chat_email_events
                 SET status = 'dead_letter', updated_at = ?2
                 WHERE status = 'batched' AND id IN (
                     SELECT event_id FROM client_chat_email_delivery_items WHERE delivery_id = ?1
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
                 FROM client_chat_email_deliveries d
                 LEFT JOIN client_chat_email_delivery_items di ON di.delivery_id = d.id
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
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!("begin chat delivery requeue failed: {error}"))
            })?;
        let changed = tx
            .execute(
                "UPDATE client_chat_email_deliveries
                 SET status = 'retry', attempts = 0, next_attempt_at = ?2,
                     claim_owner = NULL, claim_expires_at = NULL, error_message = NULL,
                     updated_at = ?2
                 WHERE id = ?1 AND status = 'dead_letter'
                   AND EXISTS (
                       SELECT 1
                       FROM client_chat_rooms r
                       INNER JOIN installations i ON i.id = r.installation_id
                       WHERE r.id = client_chat_email_deliveries.room_id
                         AND r.status = 'active'
                         AND r.owner_generation = client_chat_email_deliveries.owner_generation
                         AND lower(r.owner_email_snapshot) = lower(client_chat_email_deliveries.recipient)
                         AND i.owner_verified_at IS NOT NULL
                         AND lower(trim(i.owner_email)) = lower(client_chat_email_deliveries.recipient)
                   )
                   AND EXISTS (
                       SELECT 1 FROM client_chat_email_delivery_items di
                       WHERE di.delivery_id = client_chat_email_deliveries.id
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM client_chat_email_delivery_items di
                       INNER JOIN client_chat_email_events e ON e.id = di.event_id
                       INNER JOIN client_chat_messages m ON m.id = e.message_id
                       WHERE di.delivery_id = client_chat_email_deliveries.id
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
            "UPDATE client_chat_email_events
             SET status = 'batched', updated_at = ?2
             WHERE id IN (
                 SELECT event_id FROM client_chat_email_delivery_items WHERE delivery_id = ?1
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
            "SELECT id FROM client_chat_email_events WHERE message_id = ?1",
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
                 FROM client_chat_email_deliveries d
                 INNER JOIN client_chat_email_delivery_items di ON di.delivery_id = d.id
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
            "UPDATE client_chat_email_deliveries
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
            "UPDATE client_chat_email_events
             SET status = 'pending', window_started_at = ?2, window_ends_at = ?2, updated_at = ?2
             WHERE status IN ('batched', 'dead_letter') AND id != ?3 AND id IN (
                 SELECT event_id FROM client_chat_email_delivery_items WHERE delivery_id = ?1
             ) AND EXISTS (
                 SELECT 1 FROM client_chat_messages m
                 WHERE m.id = message_id AND m.status = 'visible'
             )",
            params![delivery_id, now.to_rfc3339(), event_id],
        )
        .map_err(|error| {
            AppError::Internal(format!("requeue chat delivery siblings failed: {error}"))
        })?;
    }
    conn.execute(
        "UPDATE client_chat_email_events
         SET status = 'cancelled_message_deleted', updated_at = ?2
         WHERE id = ?1 AND status != 'sent'",
        params![event_id, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("cancel deleted chat event failed: {error}")))?;
    Ok(())
}

fn room_id_installation(conn: &Connection, room_id: &str) -> Result<String, AppError> {
    conn.query_row(
        "SELECT installation_id FROM client_chat_rooms WHERE id = ?1",
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
            "SELECT id FROM client_chat_rooms WHERE installation_id = ?1",
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
                    r.status, r.last_message_at, r.archived_at,
                    COALESCE((SELECT MAX(m.seq) FROM client_chat_messages m WHERE m.room_id = r.id), 0)
             FROM client_chat_rooms r
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
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read chat room summary failed: {error}")))?;
    let Some((id, installation_id, client_label, status, last_message_at, archived_at, latest_seq)) =
        row
    else {
        return Ok(None);
    };
    let last_message = conn
        .query_row(
            "SELECT seq, body, author_label, status, created_at
             FROM client_chat_messages WHERE room_id = ?1 ORDER BY seq DESC LIMIT 1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| AppError::Internal(format!("read chat message preview failed: {error}")))?
        .map(|(seq, body, author_label, message_status, created_at)| {
            Ok(ClientChatMessagePreview {
                seq,
                body: if message_status == "visible" {
                    body
                } else {
                    String::new()
                },
                author_label,
                created_at: parse_timestamp(created_at, "chat message")?,
            })
        })
        .transpose()?;
    let unread_count = if let Some(user_id) = viewer_user_id {
        conn.query_row(
            "SELECT COUNT(*)
             FROM client_chat_messages m
             WHERE m.room_id = ?1
               AND m.status = 'visible'
               AND m.author_user_id != ?2
               AND m.seq > COALESCE((
                   SELECT v.last_read_seq FROM client_chat_visits v
                   WHERE v.user_id = ?2 AND v.room_id = ?1
               ), 0)",
            params![id, user_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| AppError::Internal(format!("count unread chat messages failed: {error}")))?
        .max(0) as usize
    } else {
        0
    };
    Ok(Some(ClientChatRoomView {
        id,
        installation_id,
        client_label,
        status,
        latest_seq,
        unread_count,
        last_message_at: parse_optional_timestamp(last_message_at, "chat room activity")?,
        last_message,
        archived_at: parse_optional_timestamp(archived_at, "chat room archive")?,
    }))
}

fn count_visible_messages_after(
    conn: &Connection,
    room_id: &str,
    last_read_seq: i64,
) -> Result<usize, AppError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM client_chat_messages
         WHERE room_id = ?1 AND status = 'visible' AND seq > ?2",
        params![room_id, last_read_seq.max(0)],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count.max(0) as usize)
    .map_err(|error| {
        AppError::Internal(format!("count public unread chat messages failed: {error}"))
    })
}

fn query_messages<P: rusqlite::Params>(
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
            ))
        })
        .map_err(|error| AppError::Internal(format!("query chat messages failed: {error}")))?;
    rows.map(|row| {
        let (id, seq, body, author_user_id, author_label, status, created_at) =
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
        "INSERT INTO client_chat_visits (
            user_id, room_id, first_opened_at, last_opened_at, last_read_seq, updated_at
         ) VALUES (?1, ?2, ?3, ?3, ?4, ?3)
         ON CONFLICT(user_id, room_id) DO UPDATE SET
            last_opened_at = excluded.last_opened_at,
            last_read_seq = MAX(client_chat_visits.last_read_seq, excluded.last_read_seq),
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
        "INSERT INTO client_chat_rate_limit (scope, bucket_start, count)
         VALUES (?1, ?2, 1)
         ON CONFLICT(scope, bucket_start) DO UPDATE SET count = count + 1",
        params![scope, bucket_start],
    )
    .map_err(|error| AppError::Internal(format!("update chat rate limit failed: {error}")))?;
    let count = conn
        .query_row(
            "SELECT count FROM client_chat_rate_limit WHERE scope = ?1 AND bucket_start = ?2",
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
        "SELECT id, seq, body, author_label, status, created_at
         FROM client_chat_messages
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
            ))
        },
    )
    .optional()
    .map_err(|error| AppError::Internal(format!("read idempotent chat message failed: {error}")))?
    .map(|(id, seq, body, author_label, status, created_at)| {
        Ok(ClientChatMessageView {
            id,
            seq,
            body: if status == "visible" {
                body
            } else {
                String::new()
            },
            author_label,
            is_mine: true,
            status,
            created_at: parse_timestamp(created_at, "chat message")?,
        })
    })
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
             FROM client_chat_email_events
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
        "INSERT INTO client_chat_email_events (
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
        "UPDATE client_chat_email_deliveries
         SET status = ?2, claim_owner = NULL, claim_expires_at = NULL, updated_at = ?3
         WHERE room_id = ?1
           AND status IN ('pending', 'retry', 'claimed', 'dead_letter')",
        params![room_id, status, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("cancel chat email deliveries failed: {error}")))?;
    conn.execute(
        "UPDATE client_chat_email_events
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
             FROM client_chat_rooms r WHERE r.id = ?1",
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
        "UPDATE client_chat_email_deliveries
         SET status = 'cancelled_owner_changed', claim_owner = NULL,
             claim_expires_at = NULL, updated_at = ?2
         WHERE room_id = ?1
           AND status IN ('pending', 'retry', 'claimed', 'dead_letter')",
        params![room_id, now.to_rfc3339()],
    )
    .map_err(|error| AppError::Internal(format!("cancel stale chat deliveries failed: {error}")))?;
    conn.execute(
        "UPDATE client_chat_email_events
         SET recipient = ?2, owner_generation = ?3,
             status = CASE
                 WHEN (SELECT m.status FROM client_chat_messages m WHERE m.id = message_id) != 'visible'
                 THEN 'cancelled_message_deleted'
                 WHEN lower((SELECT m.author_email FROM client_chat_messages m WHERE m.id = message_id)) = lower(?2)
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
