//! Owner money actions on top-ups that automatic verification cannot settle.
//!
//! Both actions accept a dry run that previews the outcome without writing,
//! and the committed call must match what the Owner reviewed. A manual credit
//! re-authenticates the gateway order and is optimistic on the order's
//! `updated_at`. A reversal acts on a recorded gateway refund case and must
//! repeat the previewed shortfall, because the buyer's balance can change
//! between preview and commit. They are Router-owner only.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use uuid::Uuid;

use super::{
    GatewayOrder, LoadedRevision, ServerState, audit_tx, db_error, get_bound_gateway_order,
    load_order_revision, no_store, notify_topup_tx, now, token_rail, valid_reason_code,
    validate_gateway_identity_tx,
};
use crate::db::{Connection, OptionalExtension, TransactionBehavior, params};
use crate::error::AppError;

const MANUAL_SOURCES: &[&str] = &["merchant_claim", "platform_manual", "merchant_manual"];

fn valid_reference(value: &str) -> bool {
    (8..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

struct OrderSnapshot {
    user_id: String,
    channel_id: String,
    revision: i64,
    status: String,
    updated_at: String,
    amount_minor: i64,
    gateway_order_id: Option<String>,
}

fn snapshot_tx(conn: &Connection, order_id: &str) -> Result<OrderSnapshot, AppError> {
    conn.query_row(
        "SELECT user_id, channel_id, channel_revision, status, updated_at, amount_minor,
                gateway_order_id
         FROM market_topup_orders WHERE id = ?1",
        params![order_id],
        |row| {
            Ok(OrderSnapshot {
                user_id: row.get(0)?,
                channel_id: row.get(1)?,
                revision: row.get(2)?,
                status: row.get(3)?,
                updated_at: row.get(4)?,
                amount_minor: row.get(5)?,
                gateway_order_id: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(db_error("read top-up for owner action"))?
    .ok_or_else(|| AppError::NotFound("top-up order not found".into()))
}

async fn authenticated_gateway(
    state: &ServerState,
    order_id: &str,
) -> Result<(OrderSnapshot, LoadedRevision, GatewayOrder), AppError> {
    let (snapshot, loaded) = {
        let conn = state.store.conn.lock().await;
        let snapshot = snapshot_tx(&conn, order_id)?;
        let loaded = load_order_revision(
            &conn,
            &state.market_payments,
            &snapshot.channel_id,
            snapshot.revision,
        )?;
        (snapshot, loaded)
    };
    let gateway_id = snapshot
        .gateway_order_id
        .clone()
        .ok_or_else(|| AppError::Conflict("top-up has no gateway order".into()))?;
    let gateway = get_bound_gateway_order(
        state,
        &snapshot.channel_id,
        snapshot.revision,
        &loaded,
        &gateway_id,
    )
    .await?;
    Ok((snapshot, loaded, gateway))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ManualCreditRequest {
    expected_updated_at: String,
    /// The transfer hash the merchant matched to this order.
    tx_hash: String,
    /// Used only when the gateway response omits the rail.
    token_id: Option<String>,
    evidence_reference: String,
    reason_code: String,
    dry_run: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ManualCreditView {
    order_id: String,
    gateway_status: String,
    paid_source: Option<String>,
    amount_minor: i64,
    credit_minor: i64,
    fee_minor: i64,
    token_id: String,
    credited: bool,
}

/// Credit a top-up whose gateway order is paid through a merchant claim or
/// manual reconciliation.
#[allow(clippy::too_many_arguments)]
fn apply_manual_credit_tx(
    conn: &Connection,
    order_id: &str,
    actor: &str,
    input: &ManualCreditRequest,
    loaded: &LoadedRevision,
    gateway: &GatewayOrder,
    time: &str,
) -> Result<ManualCreditView, AppError> {
    let snapshot = snapshot_tx(conn, order_id)?;
    if snapshot.updated_at != input.expected_updated_at {
        return Err(AppError::Conflict(
            "top-up changed; reload before crediting".into(),
        ));
    }
    if snapshot.status == "credited" {
        return Err(AppError::Conflict("top-up is already credited".into()));
    }
    let gateway_id = snapshot
        .gateway_order_id
        .as_deref()
        .ok_or_else(|| AppError::Conflict("top-up has no gateway order".into()))?;
    validate_gateway_identity_tx(
        conn,
        order_id,
        &snapshot.channel_id,
        snapshot.revision,
        loaded,
        gateway_id,
        gateway,
    )?;
    if gateway.status != "paid" {
        return Err(AppError::Conflict(
            "DujiaoPay does not report this order paid".into(),
        ));
    }
    if !gateway
        .paid_source
        .as_deref()
        .is_some_and(|source| MANUAL_SOURCES.contains(&source))
    {
        return Err(AppError::Conflict(
            "only merchant-claimed or manually reconciled payments use this action".into(),
        ));
    }
    let token = gateway
        .token_id
        .clone()
        .or_else(|| input.token_id.clone())
        .ok_or_else(|| AppError::BadRequest("payment token is required".into()))?;
    let rail =
        token_rail(&token).ok_or_else(|| AppError::BadRequest("unsupported token".into()))?;
    let chain = gateway.chain.clone().unwrap_or_else(|| rail.chain.into());
    if chain != rail.chain {
        return Err(AppError::Conflict(
            "payment chain does not match its token".into(),
        ));
    }
    let tx_hash = gateway
        .tx_id
        .clone()
        .unwrap_or_else(|| input.tx_hash.clone());
    if tx_hash != input.tx_hash {
        return Err(AppError::Conflict(
            "transfer hash differs from the gateway's recorded hash".into(),
        ));
    }
    let receipt_fingerprint = hex::encode(sha2::Sha256::digest(
        format!("{}:{chain}:{token}:{tx_hash}", loaded.merchant_fingerprint).as_bytes(),
    ));
    let reused: Option<String> = conn
        .query_row(
            "SELECT order_id FROM market_topup_receipts WHERE receipt_fingerprint = ?1",
            params![receipt_fingerprint],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error("check manual receipt reuse"))?;
    if reused.is_some() {
        return Err(AppError::Conflict(
            "this transfer was already credited".into(),
        ));
    }
    let (credit_minor, fee_minor) =
        crate::market_wallet::topup_credit_terms_tx(conn, order_id, snapshot.amount_minor)?;
    let view = ManualCreditView {
        order_id: order_id.into(),
        gateway_status: gateway.status.clone(),
        paid_source: gateway.paid_source.clone(),
        amount_minor: snapshot.amount_minor,
        credit_minor,
        fee_minor,
        token_id: token.clone(),
        credited: !input.dry_run,
    };
    if input.dry_run {
        return Ok(view);
    }
    let settled = format!(
        "{}.{:02}",
        snapshot.amount_minor / 100,
        snapshot.amount_minor % 100
    );
    conn.execute(
        "INSERT INTO market_topup_receipts
         (order_id, chain, token_id, tx_id, settled_amount, confirmations,
          receipt_fingerprint, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            order_id,
            chain,
            token,
            tx_hash,
            gateway.settled_amount.clone().unwrap_or(settled),
            gateway.confirmations.unwrap_or(0),
            receipt_fingerprint,
            time
        ],
    )
    .map_err(db_error("record manual top-up receipt"))?;
    let journal = crate::market_wallet::credit_manual_topup_tx(
        conn,
        &snapshot.user_id,
        order_id,
        snapshot.amount_minor,
        &receipt_fingerprint,
        time,
    )?;
    if !journal.inserted {
        return Err(AppError::Internal(
            "manual top-up journal already exists".into(),
        ));
    }
    let changed = conn
        .execute(
            "UPDATE market_topup_orders
             SET status = 'credited', credited_journal_id = ?2, paid_at = ?3, updated_at = ?4,
                 next_reconcile_at = NULL, reconcile_hint_pending = 0
             WHERE id = ?1 AND updated_at = ?5 AND status != 'credited'",
            params![
                order_id,
                journal.id,
                gateway.paid_at,
                time,
                snapshot.updated_at
            ],
        )
        .map_err(db_error("mark manual top-up credited"))?;
    if changed != 1 {
        return Err(AppError::Conflict(
            "top-up changed during manual credit".into(),
        ));
    }
    resolve_order_cases_tx(conn, order_id, actor, &input.reason_code, time)?;
    record_action_tx(
        conn,
        order_id,
        "credit_manual_claim",
        actor,
        snapshot.amount_minor,
        0,
        &input.evidence_reference,
        &input.reason_code,
        &journal.id,
        time,
    )?;
    audit_tx(
        conn,
        actor,
        "topup_manual_claim_credited",
        "topup",
        order_id,
        Some(&input.reason_code),
        time,
    )?;
    notify_topup_tx(conn, order_id, "topup_credited", time)?;
    Ok(view)
}

#[allow(clippy::too_many_arguments)]
fn record_action_tx(
    conn: &Connection,
    order_id: &str,
    action: &str,
    actor: &str,
    amount_minor: i64,
    shortfall_minor: i64,
    evidence: &str,
    reason: &str,
    journal_id: &str,
    time: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO market_topup_owner_actions
         (id, order_id, action, actor_user_id, amount_minor, shortfall_minor,
          evidence_reference, reason_code, journal_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            Uuid::new_v4().to_string(),
            order_id,
            action,
            actor,
            amount_minor,
            shortfall_minor,
            evidence,
            reason,
            journal_id,
            time
        ],
    )
    .map_err(|error| {
        if error.to_string().contains("UNIQUE") {
            AppError::Conflict("this owner action was already taken".into())
        } else {
            AppError::Internal(format!("record owner top-up action failed: {error}"))
        }
    })?;
    Ok(())
}

/// Close the top-up's open review cases. They are not critical cases, so the
/// direct update mirrors `resolve_case_tx` without its invariant recheck.
fn resolve_order_cases_tx(
    conn: &Connection,
    order_id: &str,
    actor: &str,
    reason: &str,
    time: &str,
) -> Result<(), AppError> {
    let ids = {
        let mut statement = conn
            .prepare(
                "SELECT id FROM market_finance_reconciliation_cases
                 WHERE object_kind = 'topup' AND object_id = ?1 AND status = 'open'",
            )
            .map_err(db_error("prepare top-up cases"))?;
        statement
            .query_map(params![order_id], |row| row.get::<_, String>(0))
            .map_err(db_error("query top-up cases"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error("read top-up cases"))?
    };
    for id in ids {
        crate::market_reconcile::resolve_case_tx(conn, &id, actor, reason, time)?;
    }
    Ok(())
}

pub(super) async fn credit_manual_claim(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
    Json(input): Json<ManualCreditRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    if !valid_reason_code(&input.reason_code)
        || !valid_reference(&input.evidence_reference)
        || !valid_reference(&input.tx_hash)
    {
        return Err(AppError::BadRequest(
            "reason code, evidence reference and transfer hash are required".into(),
        ));
    }
    let (_, loaded, gateway) = authenticated_gateway(&state, &order_id).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin manual top-up credit"))?;
    let view = apply_manual_credit_tx(
        &tx,
        &order_id,
        &session.user_id,
        &input,
        &loaded,
        &gateway,
        &now(),
    )?;
    tx.commit()
        .map_err(db_error("commit manual top-up credit"))?;
    Ok((no_store(), Json(view)))
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReverseRequest {
    amount_minor: i64,
    evidence_reference: String,
    reason_code: String,
    dry_run: bool,
    /// The shortfall the dry run showed. Required to commit: a different
    /// value means the balance moved and the Owner must review again.
    #[serde(default)]
    expected_shortfall_minor: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReverseView {
    order_id: String,
    amount_minor: i64,
    principal_minor: i64,
    fee_minor: i64,
    from_wallet_minor: i64,
    shortfall_minor: i64,
    reversed: bool,
}

fn apply_reversal_tx(
    conn: &Connection,
    order_id: &str,
    actor: &str,
    input: &ReverseRequest,
    time: &str,
) -> Result<ReverseView, AppError> {
    let snapshot = snapshot_tx(conn, order_id)?;
    if snapshot.status != "credited" {
        return Err(AppError::Conflict(
            "only a credited top-up can be reversed".into(),
        ));
    }
    if input.amount_minor <= 0 || input.amount_minor > snapshot.amount_minor {
        return Err(AppError::BadRequest(
            "reversal must be positive and at most the credited amount".into(),
        ));
    }
    let reused: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM market_topup_owner_actions WHERE order_id = ?1 AND action = 'reverse_external_refund' AND evidence_reference = ?2)",
        params![order_id, input.evidence_reference], |row| row.get(0)).map_err(db_error("check reversal reference"))?;
    if reused {
        return Err(AppError::Conflict(
            "this external refund was already reversed".into(),
        ));
    }
    let refund_recorded: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases
             WHERE object_kind = 'topup' AND object_id = ?1
               AND code = 'external_refund_recorded' AND status = 'open')",
            params![order_id],
            |row| row.get(0),
        )
        .map_err(db_error("check recorded gateway refund"))?;
    if !refund_recorded {
        return Err(AppError::Conflict(
            "no DujiaoPay refund has been recorded for this top-up".into(),
        ));
    }
    let available_minor = crate::market_wallet::wallet_snapshot_tx(conn, &snapshot.user_id)?
        .available_units
        .max(0)
        / crate::market_common::MONEY_UNITS_PER_MINOR;
    let (principal_minor, fee_minor) =
        crate::market_wallet::topup_reversal_terms_tx(conn, order_id, input.amount_minor)?;
    let from_wallet_minor = principal_minor.min(available_minor);
    let mut view = ReverseView {
        order_id: order_id.into(),
        amount_minor: input.amount_minor,
        principal_minor,
        fee_minor,
        from_wallet_minor,
        shortfall_minor: principal_minor - from_wallet_minor,
        reversed: false,
    };
    if input.dry_run {
        return Ok(view);
    }
    if input.expected_shortfall_minor != Some(view.shortfall_minor) {
        return Err(AppError::Conflict(
            "the buyer balance changed since the preview; preview the reversal again".into(),
        ));
    }
    let (journal, shortfall) = crate::market_wallet::reverse_topup_credit_tx(
        conn,
        &snapshot.user_id,
        order_id,
        input.amount_minor,
        &input.evidence_reference,
        time,
    )?;
    if !journal.inserted {
        return Err(AppError::Conflict(
            "this top-up was already reversed".into(),
        ));
    }
    view.shortfall_minor = shortfall;
    view.from_wallet_minor = principal_minor - shortfall;
    view.reversed = true;
    record_action_tx(
        conn,
        order_id,
        "reverse_external_refund",
        actor,
        input.amount_minor,
        shortfall,
        &input.evidence_reference,
        &input.reason_code,
        &journal.id,
        time,
    )?;
    resolve_order_cases_tx(conn, order_id, actor, &input.reason_code, time)?;
    audit_tx(
        conn,
        actor,
        "topup_external_refund_reversed",
        "topup",
        order_id,
        Some(&input.reason_code),
        time,
    )?;
    crate::market_notifications::enqueue_user_notification_tx(
        conn,
        "buyer",
        &snapshot.user_id,
        "topup_reversed",
        "topup",
        order_id,
        &format!("topup_reversed:{order_id}:{}", input.evidence_reference),
        serde_json::json!({
            "orderId": order_id,
            "amountMinor": input.amount_minor,
            "status": "reversed",
        }),
        time,
    )?;
    Ok(view)
}

pub(super) async fn reverse_external_refund(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
    Json(input): Json<ReverseRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = crate::api::require_owner_session(&state, &headers).await?;
    if !valid_reason_code(&input.reason_code) || !valid_reference(&input.evidence_reference) {
        return Err(AppError::BadRequest(
            "reason code and evidence reference are required".into(),
        ));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin top-up reversal"))?;
    let view = apply_reversal_tx(&tx, &order_id, &session.user_id, &input, &now())?;
    tx.commit().map_err(db_error("commit top-up reversal"))?;
    Ok((no_store(), Json(view)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        crate::test_support::database().unwrap()
    }

    fn seed(conn: &Connection, status: &str) {
        seed_terms(conn, status, 500, 0);
    }

    fn seed_terms(conn: &Connection, status: &str, credit: i64, fee: i64) {
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
              status, created_at, updated_at, gateway_order_id, credit_amount_minor, platform_fee_minor)
             VALUES ('order', 'buyer', 'channel', 1, 'merchant-fingerprint',
                     'router-00000000000000000000000000000001', 'k', 'channel:500', ?2 + ?3,
                     ?1, '2026-10-01', 'v1', 'gateway-1', ?2, ?3)",
            params![status, credit, fee],
        )
        .unwrap();
    }

    fn loaded() -> LoadedRevision {
        LoadedRevision {
            merchant_id: "merchant".into(),
            merchant_fingerprint: "merchant-fingerprint".into(),
            allowed_methods: Vec::new(),
            credentials: super::super::Credentials {
                key_id: "key".into(),
                api_secret: zeroize::Zeroizing::new("secret".into()),
                webhook_secret: None,
            },
        }
    }

    fn claimed_gateway() -> GatewayOrder {
        GatewayOrder {
            order_id: "gateway-1".into(),
            merchant_order_id: "router-00000000000000000000000000000001".into(),
            status: "paid".into(),
            fiat_currency: "USD".into(),
            fiat_amount_minor: 500,
            paid_source: Some("merchant_claim".into()),
            chain: Some("tron".into()),
            token_id: Some("tron-usdt".into()),
            tx_id: Some("claimedtransfer01".into()),
            settled_amount: Some("5.0003".into()),
            confirmations: Some(0),
            paid_at: Some("2026-10-02T00:00:00Z".into()),
            checkout_url: None,
            expires_at: None,
            selection_deadline: None,
            pay_address: None,
            payable_amount: None,
            fx_rate: None,
        }
    }

    fn credit_request(dry_run: bool) -> ManualCreditRequest {
        ManualCreditRequest {
            expected_updated_at: "v1".into(),
            tx_hash: "claimedtransfer01".into(),
            token_id: None,
            evidence_reference: "djp-claim-123".into(),
            reason_code: "merchant_claim_verified".into(),
            dry_run,
        }
    }

    #[test]
    fn manual_claim_credits_once_and_never_counts_as_chain_evidence() {
        let conn = memory();
        seed(&conn, "review_required");
        let preview = apply_manual_credit_tx(
            &conn,
            "order",
            "owner",
            &credit_request(true),
            &loaded(),
            &claimed_gateway(),
            "2026-10-03T00:00:00Z",
        )
        .unwrap();
        assert!(!preview.credited);
        assert_eq!(
            crate::market_wallet::wallet_snapshot_tx(&conn, "buyer")
                .unwrap()
                .posted_units,
            0
        );
        apply_manual_credit_tx(
            &conn,
            "order",
            "owner",
            &credit_request(false),
            &loaded(),
            &claimed_gateway(),
            "2026-10-03T00:00:00Z",
        )
        .unwrap();
        assert_eq!(
            crate::market_wallet::wallet_snapshot_tx(&conn, "buyer")
                .unwrap()
                .posted_units,
            500 * crate::market_common::MONEY_UNITS_PER_MINOR
        );
        let evidence: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_verified_topup_evidence",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(evidence, 0);
        assert_eq!(
            crate::market_reconcile::audit_finance_tx(&conn, "2026-10-03T00:00:00Z").unwrap(),
            0
        );
        // A second credit, or a chain gateway answer, is refused.
        assert!(
            apply_manual_credit_tx(
                &conn,
                "order",
                "owner",
                &credit_request(false),
                &loaded(),
                &claimed_gateway(),
                "2026-10-03T00:00:00Z",
            )
            .is_err()
        );
        // Manual money is valid financial evidence only while its receipt
        // still matches the fingerprint frozen into the sealed journal.
        conn.execute("UPDATE market_topup_receipts SET receipt_fingerprint = 'corrupt-receipt' WHERE order_id = 'order'", []).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM market_completed_topup_evidence",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert!(
            crate::market_reconcile::audit_finance_tx(&conn, "2026-10-04T00:00:00Z").unwrap() > 0
        );
    }

    #[test]
    fn manual_credit_uses_frozen_principal_and_cumulative_reversals_restore_exact_fee() {
        let conn = memory();
        seed_terms(&conn, "review_required", 475, 25);
        let view = apply_manual_credit_tx(
            &conn,
            "order",
            "owner",
            &credit_request(false),
            &loaded(),
            &claimed_gateway(),
            "2026-10-03T00:00:00Z",
        )
        .unwrap();
        assert_eq!((view.credit_minor, view.fee_minor), (475, 25));
        // Changing the current fee has no effect on existing credit or reversal.
        conn.execute(
            "UPDATE market_wallet_policy SET maintenance_fee_bps = 10000",
            [],
        )
        .unwrap();
        let mut fee_total = 0;
        for (index, amount) in [1, 99, 400].into_iter().enumerate() {
            conn.execute("INSERT INTO market_finance_reconciliation_cases (id, code, object_kind, object_id, status, created_at)
                VALUES (?1, 'external_refund_recorded', 'topup', 'order', 'open', 't')", params![format!("partial-case-{index}")]).unwrap();
            let request = ReverseRequest {
                amount_minor: amount,
                evidence_reference: format!("partial-{index}"),
                reason_code: "gateway_refund".into(),
                dry_run: true,
                expected_shortfall_minor: None,
            };
            let preview =
                apply_reversal_tx(&conn, "order", "owner", &request, "2026-10-04T00:00:00Z")
                    .unwrap();
            let commit = ReverseRequest {
                dry_run: false,
                expected_shortfall_minor: Some(preview.shortfall_minor),
                ..request
            };
            fee_total +=
                apply_reversal_tx(&conn, "order", "owner", &commit, "2026-10-04T00:00:00Z")
                    .unwrap()
                    .fee_minor;
            assert!(
                apply_reversal_tx(&conn, "order", "owner", &commit, "2026-10-04T00:00:00Z")
                    .is_err()
            );
        }
        assert_eq!(fee_total, 25);
        assert_eq!(
            crate::market_wallet::wallet_snapshot_tx(&conn, "buyer")
                .unwrap()
                .posted_units,
            0
        );
        assert_eq!(conn.query_row("SELECT SUM(signed_units) FROM market_journal_postings WHERE account_kind = 'platform_fee_revenue'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert!(crate::market_wallet::topup_reversal_terms_tx(&conn, "order", 1).is_err());
        crate::market_wallet::reconcile_wallet_tx(&conn, "buyer").unwrap();
    }

    #[test]
    fn chain_paid_orders_cannot_use_the_manual_path() {
        let conn = memory();
        seed(&conn, "review_required");
        let mut gateway = claimed_gateway();
        gateway.paid_source = Some("chain".into());
        assert!(
            apply_manual_credit_tx(
                &conn,
                "order",
                "owner",
                &credit_request(false),
                &loaded(),
                &gateway,
                "2026-10-03T00:00:00Z",
            )
            .is_err()
        );
    }

    #[test]
    fn reversal_takes_the_available_balance_and_books_the_rest_as_owner_loss() {
        let conn = memory();
        seed(&conn, "credited");
        crate::market_wallet::credit_verified_topup_tx(
            &conn,
            "buyer",
            "order",
            500,
            "receipt",
            "2026-10-01T00:00:00Z",
        )
        .unwrap();
        let request = ReverseRequest {
            amount_minor: 500,
            evidence_reference: "djp-refund-1".into(),
            reason_code: "gateway_refund".into(),
            dry_run: false,
            expected_shortfall_minor: Some(300),
        };
        // No recorded gateway refund: refused.
        assert!(
            apply_reversal_tx(&conn, "order", "owner", &request, "2026-10-04T00:00:00Z").is_err()
        );
        conn.execute(
            "INSERT INTO market_finance_reconciliation_cases
             (id, code, object_kind, object_id, status, created_at)
             VALUES ('refund-case', 'external_refund_recorded', 'topup', 'order', 'open', 't')",
            [],
        )
        .unwrap();
        // The buyer already spent $3 of the $5.
        crate::market_wallet::reserve_wallet_tx(
            &conn,
            "buyer",
            "test",
            "spent",
            "test",
            300 * crate::market_common::MONEY_UNITS_PER_MINOR,
            None,
            "2026-10-02T00:00:00Z",
        )
        .unwrap();
        // A commit that does not repeat the reviewed shortfall is refused.
        let stale = ReverseRequest {
            expected_shortfall_minor: Some(0),
            ..request.clone()
        };
        assert!(matches!(
            apply_reversal_tx(&conn, "order", "owner", &stale, "2026-10-04T00:00:00Z"),
            Err(AppError::Conflict(_))
        ));
        let view =
            apply_reversal_tx(&conn, "order", "owner", &request, "2026-10-04T00:00:00Z").unwrap();
        assert_eq!((view.from_wallet_minor, view.shortfall_minor), (200, 300));
        let wallet = crate::market_wallet::wallet_snapshot_tx(&conn, "buyer").unwrap();
        assert_eq!(wallet.available_units, 0);
        crate::market_wallet::reconcile_wallet_tx(&conn, "buyer").unwrap();
        let open: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_finance_reconciliation_cases WHERE status = 'open'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(open, 0);
        assert!(
            apply_reversal_tx(&conn, "order", "owner", &request, "2026-10-04T00:00:00Z").is_err()
        );
    }
}
