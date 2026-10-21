//! Authenticated gateway sweeps.
//!
//! Webhooks and per-order reconciliation can both miss a payment: a webhook
//! may never arrive and an old terminal order leaves the polling horizon after
//! 90 days. Once a day each channel revision that still has recent orders is
//! asked for its `paid` orders of the last seven days; once a week the whole
//! paid history is walked. Every paid order is then
//!
//! * matched to a local top-up and, if not yet credited, queued for an
//!   immediate authenticated lookup (crediting still happens only there), or
//! * reported to the Owner as an orphan paid order when no top-up matches.
//!
//! A sweep is resumable: its offset is persisted after every page.

use chrono::Utc;
use uuid::Uuid;

use super::dujiaopay::{self, ListOrdersQuery};
use super::{
    Credentials, ServerState, db_error, enqueue_owner_signal_tx, load_revision, now,
    open_reconciliation_case_tx, queue_signed_event_reconcile_tx,
};
use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use crate::error::AppError;

const DAILY_WINDOW_DAYS: i64 = 7;
const DAILY_INTERVAL_HOURS: i64 = 24;
const WEEKLY_INTERVAL_DAYS: i64 = 7;
const PAGES_PER_RUN: u32 = 20;
const REVISION_HORIZON_DAYS: i64 = 120;

struct SweepTarget {
    channel_id: String,
    revision: i64,
    merchant_fingerprint: String,
}

/// Current revisions plus older revisions with orders in the last 120 days,
/// one per merchant identity (a sweep lists the merchant's whole project).
fn sweep_targets_tx(conn: &Connection) -> Result<Vec<SweepTarget>, AppError> {
    let mut statement = conn
        .prepare(
            "SELECT r.channel_id, MAX(r.revision), r.merchant_fingerprint
             FROM market_payment_channel_revisions r
             JOIN market_payment_channels c ON c.id = r.channel_id
             WHERE c.archived_at IS NULL
               AND (r.revision = c.current_revision OR EXISTS (
                   SELECT 1 FROM market_topup_orders t
                   WHERE t.channel_id = r.channel_id AND t.channel_revision = r.revision
                     AND t.created_at >= ?1))
             GROUP BY r.channel_id, r.merchant_fingerprint",
        )
        .map_err(db_error("prepare gateway sweep targets"))?;
    statement
        .query_map(
            params![(Utc::now() - chrono::Duration::days(REVISION_HORIZON_DAYS)).to_rfc3339()],
            |row| {
                Ok(SweepTarget {
                    channel_id: row.get(0)?,
                    revision: row.get(1)?,
                    merchant_fingerprint: row.get(2)?,
                })
            },
        )
        .map_err(db_error("query gateway sweep targets"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("read gateway sweep targets"))
}

struct SweepRun {
    id: String,
    window_kind: &'static str,
    next_offset: u32,
    started_at: String,
}

/// Resume an unfinished sweep or start a new one when its interval elapsed.
fn due_sweep_tx(
    conn: &Connection,
    target: &SweepTarget,
    window_kind: &'static str,
) -> Result<Option<SweepRun>, AppError> {
    let last = conn
        .query_row(
            "SELECT id, started_at, finished_at, next_offset FROM market_gateway_sweeps
             WHERE channel_id = ?1 AND revision = ?2 AND window_kind = ?3
             ORDER BY started_at DESC LIMIT 1",
            params![target.channel_id, target.revision, window_kind],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("read last gateway sweep"))?;
    let interval = if window_kind == "daily" {
        chrono::Duration::hours(DAILY_INTERVAL_HOURS)
    } else {
        chrono::Duration::days(WEEKLY_INTERVAL_DAYS)
    };
    if let Some((id, started_at, finished_at, offset)) = last {
        if finished_at.is_none() {
            return Ok(Some(SweepRun {
                id,
                window_kind,
                next_offset: offset.max(0) as u32,
                started_at,
            }));
        }
        let started = chrono::DateTime::parse_from_rfc3339(&started_at)
            .map(|time| time.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now() - interval);
        if Utc::now() - started < interval {
            return Ok(None);
        }
    }
    let run = SweepRun {
        id: Uuid::new_v4().to_string(),
        window_kind,
        next_offset: 0,
        started_at: now(),
    };
    conn.execute(
        "INSERT INTO market_gateway_sweeps (id, channel_id, revision, window_kind, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            run.id,
            target.channel_id,
            target.revision,
            window_kind,
            run.started_at
        ],
    )
    .map_err(db_error("start gateway sweep"))?;
    Ok(Some(run))
}

/// Act on one paid gateway order. Returns (queued, orphan).
fn handle_paid_order_tx(
    conn: &Connection,
    target: &SweepTarget,
    order: &dujiaopay::ListedOrder,
    time: &str,
) -> Result<(bool, bool), AppError> {
    let local = conn
        .query_row(
            "SELECT id, status, local_close_reason FROM market_topup_orders
             WHERE merchant_fingerprint = ?1
               AND (gateway_order_id = ?2 OR merchant_order_id = ?3)
             LIMIT 1",
            params![
                target.merchant_fingerprint,
                order.order_id,
                order.merchant_order_id.as_deref().unwrap_or("")
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(db_error("match swept gateway order"))?;
    match local {
        Some((_, status, _)) if status == "credited" => Ok((false, false)),
        Some((order_id, _, close_reason)) => {
            if close_reason.as_deref() == Some("never_created") {
                reopen_contradicted_never_created_tx(conn, &order_id, &order.order_id, time)?;
            }
            queue_signed_event_reconcile_tx(conn, &order_id, time)?;
            Ok((true, false))
        }
        None => {
            // Only Router-shaped merchant order IDs belong to this Router; a
            // merchant sharing the project with another shop is not an orphan.
            let ours = order
                .merchant_order_id
                .as_deref()
                .is_none_or(dujiaopay::is_router_merchant_order_id);
            if !ours {
                return Ok((false, false));
            }
            open_reconciliation_case_tx(
                conn,
                "gateway_orphan_paid_order",
                "gateway_order",
                &format!("{}:{}", target.merchant_fingerprint, order.order_id),
                time,
            )?;
            enqueue_owner_signal_tx(
                conn,
                "gateway_orphan_paid_order",
                "gateway_order",
                &order.order_id,
                serde_json::json!({
                    "channelId": target.channel_id,
                    "gatewayOrderId": order.order_id,
                    "createdAt": order.created_at,
                }),
                time,
            )?;
            Ok((false, true))
        }
    }
}

/// The authenticated order list shows a paid gateway order for a top-up that
/// was closed as never created, so that proof was wrong. Bind the gateway
/// identity and clear the local close reason; the normal late-payment path
/// then verifies and credits it. The Owner is told because the proof failed.
fn reopen_contradicted_never_created_tx(
    conn: &Connection,
    order_id: &str,
    gateway_order_id: &str,
    time: &str,
) -> Result<(), AppError> {
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
             SET gateway_order_id = ?2, local_close_reason = NULL, updated_at = ?3
             WHERE id = ?1 AND local_close_reason = 'never_created'
               AND gateway_order_id IS NULL",
            params![order_id, gateway_order_id, time],
        )
        .map_err(db_error("reopen never-created top-up"))?;
    if changed == 0 {
        return Ok(());
    }
    open_reconciliation_case_tx(
        conn,
        "topup_never_created_contradicted",
        "topup",
        order_id,
        time,
    )?;
    enqueue_owner_signal_tx(
        conn,
        "gateway_orphan_paid_order",
        "topup",
        order_id,
        serde_json::json!({
            "orderId": order_id,
            "gatewayOrderId": gateway_order_id,
            "reason": "never_created_contradicted",
        }),
        time,
    )
}

async fn run_sweep(
    state: &ServerState,
    target: &SweepTarget,
    credentials: &Credentials,
    run: SweepRun,
) -> Result<(), AppError> {
    let created_after = (run.window_kind == "daily").then(|| {
        let started = chrono::DateTime::parse_from_rfc3339(&run.started_at)
            .map(|time| time.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        (started - chrono::Duration::days(DAILY_WINDOW_DAYS))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    });
    let mut offset = run.next_offset;
    for _ in 0..PAGES_PER_RUN {
        let page = state
            .market_payments
            .client
            .list_orders(
                credentials,
                &ListOrdersQuery {
                    status: Some("paid".into()),
                    created_after: created_after.clone(),
                    limit: dujiaopay::LIST_PAGE_LIMIT,
                    offset,
                    ..Default::default()
                },
            )
            .await;
        let page = match page {
            Ok(page) => page,
            Err(error) => {
                let conn = state.store.conn.lock().await;
                conn.execute(
                    "UPDATE market_gateway_sweeps SET error_code = ?2 WHERE id = ?1",
                    params![
                        run.id,
                        dujiaopay::gateway_error_code(&error)
                            .or_else(|| error.code().map(str::to_string))
                    ],
                )
                .map_err(db_error("record gateway sweep error"))?;
                return Err(error);
            }
        };
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin gateway sweep page"))?;
        let time = now();
        let (mut queued, mut orphans) = (0_i64, 0_i64);
        for order in page.orders.iter().filter(|order| order.status == "paid") {
            let (was_queued, orphan) = handle_paid_order_tx(&tx, target, order, &time)?;
            queued += i64::from(was_queued);
            orphans += i64::from(orphan);
        }
        let scanned = page.returned;
        offset += scanned;
        let finished = page.is_last(u64::from(offset));
        tx.execute(
            "UPDATE market_gateway_sweeps
             SET next_offset = ?2, scanned = scanned + ?3, queued = queued + ?4,
                 orphans = orphans + ?5, error_code = NULL,
                 finished_at = CASE WHEN ?6 THEN ?7 ELSE NULL END
             WHERE id = ?1",
            params![run.id, offset, scanned, queued, orphans, finished, time],
        )
        .map_err(db_error("advance gateway sweep"))?;
        tx.commit().map_err(db_error("commit gateway sweep page"))?;
        if queued > 0 {
            state.market_payments.wake_reconciler();
        }
        if finished {
            return Ok(());
        }
    }
    Ok(())
}

pub(super) async fn run_due_sweeps(state: &ServerState) {
    let targets = {
        let conn = state.store.conn.lock().await;
        sweep_targets_tx(&conn)
    };
    let Ok(targets) = targets else {
        tracing::warn!("gateway sweep targets unavailable");
        return;
    };
    for target in targets {
        for window_kind in ["daily", "weekly_full"] {
            let prepared = {
                let conn = state.store.conn.lock().await;
                due_sweep_tx(&conn, &target, window_kind).and_then(|run| match run {
                    Some(run) => load_revision(
                        &conn,
                        &state.market_payments,
                        &target.channel_id,
                        target.revision,
                    )
                    .map(|loaded| Some((run, loaded.credentials))),
                    None => Ok(None),
                })
            };
            match prepared {
                Ok(Some((run, credentials))) => {
                    if run_sweep(state, &target, &credentials, run).await.is_err() {
                        tracing::warn!(channel_id = %target.channel_id, window_kind, "gateway sweep deferred");
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    tracing::warn!(channel_id = %target.channel_id, "gateway sweep unavailable")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        conn
    }

    fn target() -> SweepTarget {
        SweepTarget {
            channel_id: "channel".into(),
            revision: 1,
            merchant_fingerprint: "merchant-fingerprint".into(),
        }
    }

    fn listed(order_id: &str, merchant_order_id: Option<&str>) -> dujiaopay::ListedOrder {
        dujiaopay::ListedOrder {
            order_id: order_id.into(),
            merchant_order_id: merchant_order_id.map(str::to_string),
            status: "paid".into(),
            created_at: None,
        }
    }

    fn seed(conn: &Connection) {
        conn.execute(
            "INSERT INTO market_payment_channels
             (id, gateway_kind, display_name, enabled, current_revision, created_at, updated_at)
             VALUES ('channel', 'dujiaopay', 'Gateway', 1, 1, '2026-10-01', '2026-10-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_payment_channel_revisions
             (channel_id, revision, merchant_id, merchant_fingerprint, api_key_id,
              api_secret_ciphertext, api_secret_nonce, webhook_secret_ciphertext,
              webhook_secret_nonce, key_version, verified_at, created_at)
             VALUES ('channel', 1, 'merchant', 'merchant-fingerprint', 'key',
                     'c', 'n', 'c', 'n', 1, '2026-10-01', '2026-10-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO market_topup_orders
             (id, user_id, channel_id, channel_revision, merchant_fingerprint,
              merchant_order_id, idempotency_key, request_fingerprint, amount_minor,
              status, created_at, updated_at, gateway_order_id)
             VALUES ('order-1', 'buyer', 'channel', 1, 'merchant-fingerprint',
                     'router-00000000000000000000000000000001', 'k', 'channel:100', 100,
                     'expired', '2026-01-01', '2026-01-01', 'do_known')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn paid_order_for_a_local_topup_queues_a_lookup_and_unknown_router_order_is_an_orphan() {
        let conn = memory();
        seed(&conn);
        let time = "2026-10-21T00:00:00Z";
        assert_eq!(
            handle_paid_order_tx(&conn, &target(), &listed("do_known", None), time).unwrap(),
            (true, false)
        );
        let pending: i64 = conn
            .query_row(
                "SELECT reconcile_hint_pending FROM market_topup_orders WHERE id = 'order-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1);
        let orphan = listed("do_lost", Some("router-0000000000000000000000000000000f"));
        assert_eq!(
            handle_paid_order_tx(&conn, &target(), &orphan, time).unwrap(),
            (false, true)
        );
        // Another shop's order in the same merchant project is not ours.
        assert_eq!(
            handle_paid_order_tx(&conn, &target(), &listed("do_x", Some("shop-42")), time)
                .unwrap(),
            (false, false)
        );
        let cases: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases
                 WHERE code = 'gateway_orphan_paid_order'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cases, 1);
    }

    #[test]
    fn sweeps_resume_unfinished_runs_and_respect_their_interval() {
        let conn = memory();
        seed(&conn);
        let first = due_sweep_tx(&conn, &target(), "daily").unwrap().unwrap();
        let resumed = due_sweep_tx(&conn, &target(), "daily").unwrap().unwrap();
        assert_eq!(first.id, resumed.id);
        conn.execute(
            "UPDATE market_gateway_sweeps SET finished_at = started_at WHERE id = ?1",
            params![first.id],
        )
        .unwrap();
        assert!(due_sweep_tx(&conn, &target(), "daily").unwrap().is_none());
        assert!(due_sweep_tx(&conn, &target(), "weekly_full").unwrap().is_some());
    }
}
