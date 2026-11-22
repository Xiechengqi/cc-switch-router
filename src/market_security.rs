//! Financial request possession proofs and password confirmation.
//! The signed bytes bind the origin, session, route, body and idempotency key.
//! Risky operations additionally need the account's login password, confirmed
//! once per request. Confirmations are consumed inside the final SQLite
//! transaction, never by an HTTP preflight.
//!
//! Fixed rules (no user configuration):
//! - Top-ups and spending never ask for confirmation.
//! - A transfer to a new recipient asks for the password and is then held for
//!   [`NEW_RECIPIENT_DELAY_HOURS`] before it credits; the sender can withdraw it.
//! - Operations that move money immediately and irreversibly (US$500+ to a known
//!   recipient, US$500+ C2C release, Owner finance writes) ask for the password
//!   and are unavailable for [`PASSWORD_COOLDOWN_HOURS`] after the password was
//!   set or changed, because any live session may set a password.
use crate::{
    ServerState,
    db::{Connection, OptionalExtension, TransactionBehavior, params},
    error::AppError,
    market_wallet_policy::db_error,
    models::AuthSession,
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use uuid::Uuid;
use zeroize::Zeroizing;

const PROOF_HEADER: &str = "x-router-verified-financial-proof";
const REQUEST_TTL_SECONDS: i64 = 300;
const CONFIRMATION_TTL_MINUTES: i64 = 5;
/// Hours after a password is set or changed before it may confirm money that
/// moves immediately.
pub(crate) const PASSWORD_COOLDOWN_HOURS: i64 = 24;
/// Hours a transfer to a new recipient waits before it credits.
pub(crate) const NEW_RECIPIENT_DELAY_HOURS: i64 = 24;
/// Transfers and C2C releases at or above this many US cents need confirmation.
pub(crate) const LARGE_AMOUNT_MINOR: i64 = 50_000;

fn hash(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}
fn denied(code: &'static str, message: &str) -> AppError {
    AppError::coded_forbidden(code, message, json!({}))
}
pub(crate) fn origin(state: &ServerState) -> String {
    format!(
        "{}://{}",
        if state.config.use_localhost {
            "http"
        } else {
            "https"
        },
        state.config.tunnel_domain.trim_end_matches('/')
    )
}
pub(crate) fn financial_write(method: &Method, path: &str) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
        && (path.starts_with("/v1/market-wallet/")
            || path.starts_with("/v1/admin/market-")
            || path.starts_with("/v1/market-provider-finance/"))
}
fn field<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, AppError> {
    let mut values = headers.get_all(name).iter();
    let v = values
        .next()
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 2048)
        .ok_or_else(|| {
            denied(
                "FINANCIAL_DEVICE_PROOF_REQUIRED",
                "Sign in on this device to authorize financial operations",
            )
        })?;
    if values.next().is_some() || v.contains(['\n', '\r']) {
        return Err(denied(
            "FINANCIAL_SIGNATURE_INVALID",
            "ambiguous signed header",
        ));
    }
    Ok(v)
}
pub(crate) fn verify_signature(
    key: &str,
    signature: &str,
    canonical: &str,
) -> Result<(), AppError> {
    let key: [u8; 32] = STANDARD
        .decode(key)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| denied("FINANCIAL_SIGNATURE_INVALID", "invalid device key"))?;
    let signature: [u8; 64] = STANDARD
        .decode(signature)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| denied("FINANCIAL_SIGNATURE_INVALID", "invalid device signature"))?;
    VerifyingKey::from_bytes(&key)
        .map_err(|_| denied("FINANCIAL_SIGNATURE_INVALID", "invalid device key"))?
        .verify(canonical.as_bytes(), &Signature::from_bytes(&signature))
        .map_err(|_| {
            denied(
                "FINANCIAL_SIGNATURE_INVALID",
                "financial request signature mismatch",
            )
        })
}
fn binding(
    origin: &str,
    device: &str,
    access_hash: &str,
    method: &str,
    path: &str,
    body: &[u8],
    idem: &str,
) -> String {
    format!(
        "market-finance-v1\n{origin}\n{device}\n{access_hash}\n{method}\n{path}\n{}\n{idem}",
        hash(body)
    )
}

/// How much confirmation a signed financial write needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Risk {
    None,
    /// Password confirmation; the money does not move immediately.
    Confirm,
    /// Password confirmation outside the post-change cooldown.
    Immediate,
}
impl Risk {
    fn as_str(self) -> &'static str {
        match self {
            Risk::None => "none",
            Risk::Confirm => "confirm",
            Risk::Immediate => "immediate",
        }
    }
}

fn decoded_segment(raw: &str) -> Result<String, AppError> {
    // Axum decodes dynamic path parameters before the handler sees them.
    // Classify the same object while still signing the exact raw URI.
    percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map(|v| v.into_owned())
        .map_err(|_| AppError::BadRequest("invalid resource path".into()))
}

/// A recipient is known once the sender has sent them money that credited.
/// Only transfers the sender originated count: money received from someone
/// does not make them a trusted destination, otherwise anyone could "warm up"
/// a victim's account by sending it a cent.
pub(crate) fn known_recipient_tx(
    conn: &Connection,
    sender: &str,
    recipient: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM market_wallet_transfers WHERE sender_user_id=?1 AND recipient_user_id=?2)",
        params![sender, recipient],
        |r| r.get(0),
    )
    .map_err(db_error("check known wallet recipient"))
}

/// A financial body must be a JSON object. Typed handlers would also accept a
/// positional array, which field-based classification cannot read.
fn body_object(body: &[u8]) -> Result<serde_json::Map<String, Value>, AppError> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(AppError::BadRequest(
            "financial request body must be a JSON object".into(),
        )),
    }
}

pub(crate) fn risk_tx(
    conn: &Connection,
    actor: &AuthSession,
    path: &str,
    body: &[u8],
) -> Result<Risk, AppError> {
    if path.starts_with("/v1/market-wallet/security/") {
        return Ok(Risk::None);
    }
    if path == "/v1/admin/market-finance/pause" {
        // Pausing is protective and must stay available right after a
        // password change; anything that could lift a pause is immediate.
        let body = body_object(body)?;
        let protective = body.get("newOrdersPaused") == Some(&Value::Bool(true))
            && body
                .get("newTopupsPaused")
                .is_none_or(|v| v == &Value::Bool(true));
        return Ok(if protective {
            Risk::Confirm
        } else {
            Risk::Immediate
        });
    }
    if path.starts_with("/v1/admin/market-") {
        return Ok(Risk::Immediate);
    }
    let large = crate::market_wallet::units_from_minor(LARGE_AMOUNT_MINOR)?;
    if path == "/v1/market-wallet/transfers" {
        let value = body_object(body)?;
        let quote = value
            .get("quoteId")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::BadRequest("quoteId required".into()))?;
        // Committed business replay must succeed even after a policy change or
        // challenge expiry. It still needs a fresh valid device-signed request.
        let prior: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_wallet_transfers WHERE sender_user_id=?1 AND quote_id=?2)
                 OR EXISTS(SELECT 1 FROM market_wallet_pending_transfers WHERE sender_user_id=?1 AND quote_id=?2)",
                params![actor.user_id, quote],
                |r| r.get(0),
            )
            .map_err(db_error("read transfer replay"))?;
        if prior {
            return Ok(Risk::None);
        }
        let terms: Option<(String, i64, bool)> = conn
            .query_row(
                "SELECT recipient_user_id, amount_units, first_recipient FROM market_wallet_transfer_quotes WHERE id=?1 AND sender_user_id=?2",
                params![quote, actor.user_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(db_error("classify transfer risk"))?;
        let Some((recipient, amount, first)) = terms else {
            // The handler rejects an unknown quote.
            return Ok(Risk::None);
        };
        // A quote shown as delayed stays delayed, even if the recipient became
        // known while the person was reviewing it.
        return Ok(
            if first || !known_recipient_tx(conn, &actor.user_id, &recipient)? {
                Risk::Confirm
            } else if amount >= large {
                Risk::Immediate
            } else {
                Risk::None
            },
        );
    }
    if path.ends_with("/release") && path.contains("/c2c/orders/") {
        let id = decoded_segment(
            path.trim_end_matches("/release")
                .rsplit('/')
                .next()
                .unwrap_or(""),
        )?;
        let high: bool = conn
            .query_row(
                "SELECT amount_units>=?2 AND status!='released' FROM market_c2c_orders WHERE id=?1",
                params![id, large],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("classify C2C risk"))?
            .unwrap_or(false);
        return Ok(if high { Risk::Immediate } else { Risk::None });
    }
    if let Some(rest) = path.strip_suffix("/return/approve") {
        // Retired feature: requests created before the retirement can still be
        // approved by their recipient, which moves money immediately.
        let id = decoded_segment(rest.rsplit('/').next().unwrap_or(""))?;
        let committed: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_transfer_returns r JOIN market_wallet_transfers t ON t.id=r.transfer_id
                 WHERE r.transfer_id=?1 AND t.recipient_user_id=?2 AND r.status='approved')",
                params![id, actor.user_id],
                |r| r.get(0),
            )
            .map_err(db_error("classify return replay"))?;
        return Ok(if committed {
            Risk::None
        } else {
            Risk::Immediate
        });
    }
    Ok(Risk::None)
}

/// When the account's password was last set; `None` without a password.
fn password_set_at_tx(conn: &Connection, user: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT updated_at FROM user_password_credentials WHERE user_id=?1",
        params![user],
        |r| r.get(0),
    )
    .optional()
    .map_err(db_error("read password credential age"))
}
/// The time the post-change cooldown counts from, or `None` when no cooldown
/// applies. A password set by the host operator through the local CLI (root or
/// the service user on the Router host) is trusted at once; only a password
/// set from a web session, which any live session can do, must wait.
fn cooldown_start_tx(
    conn: &Connection,
    user: &str,
    set_at: &str,
) -> Result<Option<String>, AppError> {
    let source: Option<String> = conn
        .query_row(
            "SELECT source FROM auth_password_events WHERE user_id=?1
             ORDER BY auth_version DESC LIMIT 1",
            params![user],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("read password change source"))?;
    Ok((source.as_deref() != Some("local_cli")).then(|| set_at.to_string()))
}
/// The moment a freshly set password may confirm immediate money movement, if
/// that moment is still in the future.
fn cooldown_until(set_at: &str, time: &str) -> Option<String> {
    let set_at = DateTime::parse_from_rfc3339(set_at)
        .ok()?
        .with_timezone(&Utc);
    let now = DateTime::parse_from_rfc3339(time).ok()?.with_timezone(&Utc);
    let until = set_at + Duration::hours(PASSWORD_COOLDOWN_HOURS);
    (until > now).then(|| until.to_rfc3339())
}
fn require_password_ready_tx(
    conn: &Connection,
    user: &str,
    risk: Risk,
    time: &str,
) -> Result<(), AppError> {
    let Some(set_at) = password_set_at_tx(conn, user)? else {
        return Err(denied(
            "FINANCIAL_PASSWORD_REQUIRED",
            "Set a login password to confirm this operation",
        ));
    };
    if risk == Risk::Immediate {
        if let Some(until) = cooldown_start_tx(conn, user, &set_at)?
            .as_deref()
            .and_then(|start| cooldown_until(start, time))
        {
            return Err(AppError::coded_forbidden(
                "FINANCIAL_PASSWORD_COOLING",
                "A newly set password can confirm this operation after the security wait",
                json!({ "usableAt": until }),
            ));
        }
    }
    Ok(())
}
/// Business-transaction check that this HTTP execution carried a password
/// confirmation of at least `risk`. Classification happens in the middleware;
/// this independent check keeps a misclassified request from moving money.
/// Internal callers without a request proof (background jobs, tests) skip it.
pub(crate) fn require_confirmed_tx(
    conn: &Connection,
    actor: &AuthSession,
    risk: Risk,
) -> Result<(), AppError> {
    let Some(proof) = actor.financial_proof_id.as_deref() else {
        return Ok(());
    };
    let confirmed: Option<String> = conn
        .query_row(
            "SELECT c.risk FROM market_financial_request_proofs p JOIN market_password_confirmations c ON c.id=p.stepup_id
             WHERE p.id=?1 AND p.user_id=?2 AND c.user_id=?2 AND c.completed_at IS NOT NULL",
            params![proof, actor.user_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error("check financial confirmation"))?;
    let enough = match (risk, confirmed.as_deref()) {
        (Risk::None, _) => true,
        (Risk::Confirm, Some(_)) => true,
        (Risk::Immediate, Some("immediate")) => true,
        _ => false,
    };
    if enough {
        Ok(())
    } else {
        Err(denied(
            "FINANCIAL_STEPUP_INVALID",
            "this operation needs a password confirmation; retry",
        ))
    }
}

/// Business-transaction check for money that moves immediately: a password
/// exists and is older than the cooldown.
pub(crate) fn require_immediate_ready_tx(
    conn: &Connection,
    user: &str,
    time: &str,
) -> Result<(), AppError> {
    require_password_ready_tx(conn, user, Risk::Immediate, time)
}
fn confirmation_tx(
    conn: &Connection,
    actor: &AuthSession,
    binding: &str,
    risk: Risk,
    time: &str,
) -> Result<String, AppError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM market_password_confirmations WHERE user_id=?1 AND julianday(created_at)>julianday(?2)-1.0/1440",
            params![actor.user_id, time],
            |r| r.get(0),
        )
        .map_err(db_error("count financial confirmations"))?;
    if count >= 12 {
        return Err(AppError::TooManyRequests(
            "too many confirmation requests; retry in a minute".into(),
        ));
    }
    let id = Uuid::new_v4().to_string();
    let expires = (DateTime::parse_from_rfc3339(time)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
        + Duration::minutes(CONFIRMATION_TTL_MINUTES))
    .to_rfc3339();
    conn.execute(
        "INSERT INTO market_password_confirmations(id,user_id,session_id,binding,risk,expires_at,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![id, actor.user_id, actor.session_id, binding, risk.as_str(), expires, time],
    )
    .map_err(db_error("save financial confirmation"))?;
    Ok(id)
}

pub(crate) async fn middleware(
    State(state): State<ServerState>,
    mut req: Request,
    next: Next,
) -> Response {
    req.headers_mut().remove(PROOF_HEADER); // Never trust a caller's internal handle.
    if !financial_write(req.method(), req.uri().path()) {
        return next.run(req).await;
    }
    match authorize_request(&state, req).await {
        Ok(req) => next.run(req).await,
        Err(e) => e.into_response(),
    }
}
async fn authorize_request(state: &ServerState, req: Request) -> Result<Request, AppError> {
    let (mut parts, body) = req.into_parts();
    let actor = crate::market_finance_auth::write_session(state, &parts.headers).await?;
    let raw = to_bytes(body, 128 * 1024)
        .await
        .map_err(|_| AppError::BadRequest("financial request body too large".into()))?;
    let device = field(&parts.headers, "x-router-device")?;
    if actor.auth_source_kind != "auth_device" || actor.auth_source_id != device {
        return Err(denied(
            "FINANCIAL_DEVICE_MISMATCH",
            "financial request must be signed by the login device",
        ));
    }
    let timestamp = field(&parts.headers, "x-router-request-time")?;
    let millis: i64 = timestamp
        .parse()
        .map_err(|_| denied("FINANCIAL_SIGNATURE_INVALID", "invalid request time"))?;
    if Utc::now().timestamp_millis().abs_diff(millis) > REQUEST_TTL_SECONDS as u64 * 1000 {
        return Err(denied(
            "FINANCIAL_SIGNATURE_EXPIRED",
            "financial request expired; retry",
        ));
    }
    let nonce = field(&parts.headers, "x-router-request-nonce")?;
    if nonce.len() > 128 {
        return Err(denied(
            "FINANCIAL_SIGNATURE_INVALID",
            "invalid request nonce",
        ));
    }
    let signature = field(&parts.headers, "x-router-request-signature")?;
    let idem = parts
        .headers
        .get("idempotency-key")
        .map(|_| field(&parts.headers, "idempotency-key"))
        .transpose()?
        .unwrap_or("");
    let path = parts
        .uri
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or(parts.uri.path());
    let payload = binding(
        &origin(state),
        device,
        // The store encodes token hashes as Base64. The public signature
        // protocol uses lowercase hex, matching WebCrypto in the browser.
        &hash(
            crate::market_finance_auth::bearer_token(&parts.headers)
                .ok_or_else(|| AppError::Unauthorized("login required".into()))?,
        ),
        parts.method.as_str(),
        path,
        &raw,
        idem,
    );
    let canonical = format!("{payload}\n{timestamp}\n{nonce}");
    let bound = hash(&payload);
    let time = crate::market_wallet::now();
    let id = Uuid::new_v4().to_string();
    {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin financial proof"))?;
        crate::market_finance_auth::require_live_session_tx(&tx, &actor, &time)?;
        let key: String = tx
            .query_row(
                "SELECT public_key FROM auth_devices WHERE id=?1 AND status='active'",
                params![device],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("read financial device"))?
            .ok_or_else(|| denied("FINANCIAL_DEVICE_MISMATCH", "login device was revoked"))?;
        verify_signature(&key, signature, &canonical)?;
        let requests:i64=tx.query_row("SELECT COUNT(*) FROM market_financial_request_proofs WHERE user_id=?1 AND julianday(created_at)>julianday(?2)-1.0/1440",params![actor.user_id,time],|r|r.get(0)).map_err(db_error("bound financial request work"))?;
        if requests >= 120 {
            return Err(AppError::TooManyRequests(
                "too many financial requests; retry in a minute".into(),
            ));
        }
        let seen:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM market_financial_request_proofs WHERE session_id=?1 AND nonce=?2)",params![actor.session_id,nonce],|r|r.get(0)).map_err(db_error("check financial replay"))?;
        if seen {
            return Err(denied(
                "FINANCIAL_REQUEST_REPLAY",
                "signed request was already used; retry with a fresh signature",
            ));
        }
        let risk = risk_tx(&tx, &actor, parts.uri.path(), &raw)?;
        let stepup = parts
            .headers
            .get("x-router-stepup")
            .and_then(|v| v.to_str().ok());
        if risk != Risk::None {
            require_password_ready_tx(&tx, &actor.user_id, risk, &time)?;
            let Some(stepup) = stepup else {
                let challenge = confirmation_tx(&tx, &actor, &bound, risk, &time)?;
                tx.commit()
                    .map_err(db_error("commit financial confirmation request"))?;
                return Err(AppError::Coded {
                    status: StatusCode::PRECONDITION_REQUIRED,
                    code: "FINANCIAL_STEPUP_REQUIRED",
                    message: "Enter your login password to confirm this operation".into(),
                    details: json!({ "challengeId": challenge, "method": "password", "risk": risk.as_str() }),
                });
            };
            let valid: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM market_password_confirmations WHERE id=?1 AND user_id=?2 AND session_id=?3
                     AND binding=?4 AND completed_at IS NOT NULL AND used_proof_id IS NULL AND julianday(expires_at)>julianday(?5))",
                    params![stepup, actor.user_id, actor.session_id, bound, time],
                    |r| r.get(0),
                )
                .map_err(db_error("check financial confirmation"))?;
            if !valid {
                return Err(denied(
                    "FINANCIAL_STEPUP_INVALID",
                    "confirmation expired or does not match this operation",
                ));
            }
        }
        let stepup = if risk != Risk::None { stepup } else { None };
        tx.execute("INSERT INTO market_financial_request_proofs(id,user_id,session_id,device_id,nonce,binding,canonical,signature,public_key,stepup_id,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![id,actor.user_id,actor.session_id,device,nonce,bound,canonical,signature,key,stepup,time,(Utc::now()+Duration::seconds(REQUEST_TTL_SECONDS)).to_rfc3339()]).map_err(db_error("save financial request proof"))?;
        tx.commit()
            .map_err(db_error("commit financial request proof"))?;
    }
    parts.headers.insert(
        PROOF_HEADER,
        id.parse()
            .map_err(|_| AppError::Internal("invalid proof handle".into()))?,
    );
    Ok(Request::from_parts(parts, Body::from(raw)))
}

pub(crate) fn attach_proof(session: &mut AuthSession, headers: &HeaderMap) {
    session.financial_proof_id = headers
        .get(PROOF_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
}
/// Called within the business transaction. One confirmation can authorize
/// exactly one signed HTTP execution; repeated checks inside that execution are
/// permitted.
pub(crate) fn validate_proof_tx(
    conn: &Connection,
    actor: &AuthSession,
    time: &str,
) -> Result<(), AppError> {
    let Some(id) = actor.financial_proof_id.as_deref() else {
        return Ok(());
    };
    let row: Option<(Option<String>,String)>=conn.query_row("SELECT p.stepup_id,p.binding FROM market_financial_request_proofs p JOIN auth_devices d ON d.id=p.device_id AND d.status='active' WHERE p.id=?1 AND p.user_id=?2 AND p.session_id=?3 AND p.device_id=?4 AND julianday(p.expires_at)>julianday(?5)",params![id,actor.user_id,actor.session_id,actor.auth_source_id,time],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db_error("validate financial request proof"))?;
    let (stepup, bound) = row.ok_or_else(|| {
        denied(
            "FINANCIAL_SIGNATURE_EXPIRED",
            "financial request proof expired or device revoked",
        )
    })?;
    if let Some(stepup) = stepup {
        // Immediate operations re-check the password age at commit time: the
        // password may have been replaced after the confirmation was granted.
        let risk: Option<String> = conn
            .query_row(
                "SELECT risk FROM market_password_confirmations WHERE id=?1 AND user_id=?2",
                params![stepup, actor.user_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("read financial confirmation risk"))?;
        if risk.as_deref() == Some("immediate")
            && require_password_ready_tx(conn, &actor.user_id, Risk::Immediate, time).is_err()
        {
            return Err(denied(
                "FINANCIAL_STEPUP_INVALID",
                "financial confirmation already used or the password changed",
            ));
        }
        let updated = conn
            .execute(
                "UPDATE market_password_confirmations SET used_proof_id=?1
                 WHERE id=?2 AND user_id=?3 AND session_id=?4 AND binding=?5 AND completed_at IS NOT NULL
                   AND (used_proof_id IS NULL OR used_proof_id=?1) AND julianday(expires_at)>julianday(?6)
                   AND EXISTS(SELECT 1 FROM user_password_credentials c WHERE c.user_id=?3)",
                params![id, stepup, actor.user_id, actor.session_id, bound, time],
            )
            .map_err(db_error("consume financial confirmation"))?;
        if updated != 1 {
            return Err(denied(
                "FINANCIAL_STEPUP_INVALID",
                "financial confirmation already used or the password changed",
            ));
        }
    }
    Ok(())
}
fn event_tx(
    conn: &Connection,
    user: &str,
    actor: &str,
    kind: &str,
    object: &str,
    details: Value,
    time: &str,
) -> Result<(), AppError> {
    conn.execute("INSERT INTO market_security_events(id,user_id,actor,kind,object_id,details_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![Uuid::new_v4().to_string(),user,actor,kind,object,details.to_string(),time]).map_err(db_error("record wallet security event"))?;
    Ok(())
}
pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/v1/market-wallet/security", get(status))
        .route("/v1/market-wallet/security/confirm", post(confirm))
        // Wallet Passkeys were replaced by password confirmation. Old pages get
        // an explicit answer instead of a generic 404.
        .route(
            "/v1/market-wallet/security/passkeys/registration",
            post(passkeys_retired),
        )
        .route(
            "/v1/market-wallet/security/passkeys/registration/finish",
            post(passkeys_retired),
        )
        .route(
            "/v1/market-wallet/security/authenticate",
            post(passkeys_retired),
        )
        .route(
            "/v1/market-wallet/security/passkeys/:id/revoke",
            post(passkeys_retired),
        )
}
fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
}
async fn passkeys_retired() -> AppError {
    AppError::Gone(
        "Wallet Passkeys were replaced by login password confirmation; reload the page".into(),
    )
}
async fn status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let actor = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    let conn = state.store.conn.lock().await;
    let time = crate::market_wallet::now();
    let set_at = password_set_at_tx(&conn, &actor.user_id)?;
    let immediate_usable_at = match set_at.as_deref() {
        Some(at) => cooldown_start_tx(&conn, &actor.user_id, at)?
            .as_deref()
            .and_then(|start| cooldown_until(start, &time)),
        None => None,
    };
    Ok((
        no_store(),
        Json(json!({
            "hasPassword": set_at.is_some(),
            "passwordSetAt": set_at,
            "immediateUsableAt": immediate_usable_at,
            "canManageRiskPolicy": crate::api::is_finance_owner(&state.config, &actor.email),
            "largeAmountMinor": LARGE_AMOUNT_MINOR,
            "newRecipientDelayHours": NEW_RECIPIENT_DELAY_HOURS,
            "passwordCooldownHours": PASSWORD_COOLDOWN_HOURS,
        })),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfirmInput {
    challenge_id: String,
    password: Zeroizing<String>,
}
/// Verifies the login password for one pending confirmation. Attempts share
/// the persistent password-login budget, so confirmation cannot be used to
/// guess a password faster than signing in.
async fn confirm(
    State(state): State<ServerState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<ConfirmInput>,
) -> Result<impl IntoResponse, AppError> {
    let actor = crate::market_finance_auth::write_session(&state, &headers).await?;
    if input.challenge_id.len() > 64 || input.password.len() > 512 {
        return Err(denied(
            "FINANCIAL_PASSWORD_INVALID",
            "password is incorrect",
        ));
    }
    let ip = crate::client_meta::extract_client_metadata(&headers, peer)
        .ip
        .unwrap_or_else(|| peer.ip().to_string());
    let hash = {
        let conn = state.store.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error("begin financial confirmation"))?;
        let time = crate::market_wallet::now();
        crate::market_finance_auth::require_live_session_tx(&tx, &actor, &time)?;
        let pending: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_password_confirmations WHERE id=?1 AND user_id=?2 AND session_id=?3
                 AND completed_at IS NULL AND julianday(expires_at)>julianday(?4))",
                params![input.challenge_id, actor.user_id, actor.session_id, time],
                |r| r.get(0),
            )
            .map_err(db_error("load financial confirmation"))?;
        if !pending {
            return Err(denied(
                "FINANCIAL_CHALLENGE_EXPIRED",
                "confirmation expired; retry the operation",
            ));
        }
        crate::store::password_auth::reserve_attempt(
            &tx,
            &actor.email,
            &ip,
            &actor.auth_source_id,
            Utc::now(),
        )?;
        let hash: Option<String> = tx
            .query_row(
                "SELECT password_hash FROM user_password_credentials WHERE user_id=?1",
                params![actor.user_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("read password credential"))?;
        tx.commit()
            .map_err(db_error("commit confirmation attempt"))?;
        hash
    };
    let Some(hash) = hash else {
        return Err(denied(
            "FINANCIAL_PASSWORD_REQUIRED",
            "Set a login password to confirm this operation",
        ));
    };
    if !crate::store::password_auth::verify_password_hash(input.password, hash).await? {
        return Err(denied(
            "FINANCIAL_PASSWORD_INVALID",
            "password is incorrect",
        ));
    }
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin financial confirmation finish"))?;
    let time = crate::market_wallet::now();
    // A password change during verification revokes this session.
    crate::market_finance_auth::require_live_session_tx(&tx, &actor, &time)?;
    let done = tx
        .execute(
            "UPDATE market_password_confirmations SET completed_at=?4 WHERE id=?1 AND user_id=?2 AND session_id=?3
             AND completed_at IS NULL AND julianday(expires_at)>julianday(?4)",
            params![input.challenge_id, actor.user_id, actor.session_id, time],
        )
        .map_err(db_error("complete financial confirmation"))?;
    if done != 1 {
        return Err(denied(
            "FINANCIAL_CHALLENGE_EXPIRED",
            "confirmation expired; retry the operation",
        ));
    }
    event_tx(
        &tx,
        &actor.user_id,
        &actor.session_id,
        "operation_confirmed",
        &input.challenge_id,
        json!({"method":"password","proofId":actor.financial_proof_id}),
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit financial confirmation"))?;
    Ok((no_store(), Json(json!({"stepupId":input.challenge_id}))))
}

pub(crate) fn prune_tx(conn: &Connection, time: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM market_financial_request_proofs WHERE id IN (SELECT p.id FROM market_financial_request_proofs p WHERE julianday(p.expires_at)<julianday(?1)-30 AND NOT EXISTS(SELECT 1 FROM market_financial_authorizations a WHERE a.request_proof_id=p.id) AND NOT EXISTS(SELECT 1 FROM market_transfer_return_consents c WHERE c.request_proof_id=p.id) AND NOT EXISTS(SELECT 1 FROM market_security_challenges c WHERE c.used_proof_id=p.id) AND NOT EXISTS(SELECT 1 FROM market_password_confirmations c WHERE c.used_proof_id=p.id) AND NOT EXISTS(SELECT 1 FROM market_security_events e WHERE json_extract(e.details_json,'$.proofId')=p.id) AND NOT EXISTS(SELECT 1 FROM market_financial_risk_events e WHERE json_extract(e.details_json,'$.proofId')=p.id) AND NOT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases c WHERE c.code='financial_request_proof_invalid' AND c.object_id=p.id) LIMIT 1000)",params![time]).map_err(db_error("prune unused financial request proofs"))?;
    conn.execute("DELETE FROM market_security_challenges WHERE id IN (SELECT c.id FROM market_security_challenges c WHERE julianday(c.expires_at)<julianday(?1)-30 AND c.completed_at IS NULL AND NOT EXISTS(SELECT 1 FROM market_financial_request_proofs p WHERE p.stepup_id=c.id) LIMIT 1000)",params![time]).map_err(db_error("prune expired security challenges"))?;
    conn.execute("DELETE FROM market_password_confirmations WHERE id IN (SELECT c.id FROM market_password_confirmations c WHERE julianday(c.expires_at)<julianday(?1)-30 AND c.used_proof_id IS NULL AND NOT EXISTS(SELECT 1 FROM market_financial_request_proofs p WHERE p.stepup_id=c.id) LIMIT 1000)",params![time]).map_err(db_error("prune expired financial confirmations"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn proof(conn: &Connection, a: &AuthSession, id: &str, stepup: Option<&str>) {
        conn.execute("INSERT INTO market_financial_request_proofs(id,user_id,session_id,device_id,nonce,binding,canonical,signature,public_key,stepup_id,created_at,expires_at) VALUES(?1,?2,?3,?4,?1,'binding','','','',?5,'2020-01-01','2100-01-01')",params![id,a.user_id,a.session_id,a.auth_source_id,stepup]).unwrap();
    }
    fn device(conn: &Connection, id: &str) {
        conn.execute("INSERT INTO auth_devices(id,public_key,kind,platform,app_version,status,created_at,last_seen_at) VALUES(?1,'key','browser','test','test','active','2020-01-01','2020-01-01')",params![id]).unwrap();
    }
    fn password(conn: &Connection, user: &str, set_at: &str) {
        conn.execute("INSERT INTO user_password_credentials(user_id,password_hash,updated_at) VALUES(?1,'hash',?2) ON CONFLICT(user_id) DO UPDATE SET updated_at=excluded.updated_at",params![user,set_at]).unwrap();
    }
    fn completed(conn: &Connection, a: &AuthSession, id: &str, risk: &str) {
        conn.execute("INSERT INTO market_password_confirmations(id,user_id,session_id,binding,risk,expires_at,completed_at,created_at) VALUES(?1,?2,?3,'binding',?4,'2100-01-01','2020-01-01','2020-01-01')",params![id,a.user_id,a.session_id,risk]).unwrap();
    }

    #[test]
    fn encoded_order_ids_cannot_skip_large_release_confirmation() {
        let conn = crate::test_support::database().unwrap();
        let actor = crate::market_finance_auth::test_session(&conn, "encoded-path");
        conn.execute_batch("CREATE TEMP TABLE market_c2c_orders(id TEXT PRIMARY KEY, amount_units INTEGER, status TEXT);
            INSERT INTO market_c2c_orders VALUES('c2c-large',4320000000,'paid'),('c2c-small',8640000,'paid');").unwrap();
        for id in ["c2c-large", "%632c-large", "c2c-%6carge"] {
            assert_eq!(
                risk_tx(
                    &conn,
                    &actor,
                    &format!("/v1/market-wallet/c2c/orders/{id}/release"),
                    b"{}"
                )
                .unwrap(),
                Risk::Immediate
            );
        }
        assert_eq!(
            risk_tx(
                &conn,
                &actor,
                "/v1/market-wallet/c2c/orders/%632c-small/release",
                b"{}"
            )
            .unwrap(),
            Risk::None
        );
        assert_eq!(
            risk_tx(
                &conn,
                &actor,
                "/v1/admin/market-payment-channels/security/settings",
                b"{}"
            )
            .unwrap(),
            Risk::Immediate,
            "a resource id cannot select the wallet-security exemption"
        );
        assert_eq!(
            risk_tx(&conn, &actor, "/v1/market-wallet/security/confirm", b"{}").unwrap(),
            Risk::None
        );
    }

    #[test]
    fn transfers_to_new_recipients_confirm_and_large_known_transfers_are_immediate() {
        let conn = crate::test_support::database().unwrap();
        let a = crate::market_finance_auth::test_session(&conn, "risk-sender");
        let b = crate::market_finance_auth::test_session(&conn, "risk-recipient");
        let large = crate::market_wallet::units_from_minor(LARGE_AMOUNT_MINOR).unwrap();
        for (id, amount) in [("q-small", large - 1), ("q-large", large)] {
            conn.execute("INSERT INTO market_wallet_transfer_quotes(id,sender_user_id,recipient_user_id,sender_email,recipient_email,amount_units,expires_at,created_at) VALUES(?1,?2,?3,?4,?5,?6,'2100-01-01','2020-01-01')",params![id,a.user_id,b.user_id,a.email,b.email,amount]).unwrap();
        }
        let body = |q: &str| format!("{{\"quoteId\":\"{q}\"}}");
        for q in ["q-small", "q-large"] {
            assert_eq!(
                risk_tx(&conn, &a, "/v1/market-wallet/transfers", body(q).as_bytes()).unwrap(),
                Risk::Confirm
            );
        }
        assert!(!known_recipient_tx(&conn, &a.user_id, &b.user_id).unwrap());
        for user in [&a.user_id, &b.user_id] {
            crate::market_wallet::credit_verified_topup_tx(
                &conn,
                user,
                &format!("risk-topup-{user}"),
                1000,
                "receipt",
                "2026-11-17T00:00:00Z",
            )
            .unwrap();
        }
        // Money received from someone does not make them a trusted destination.
        crate::market_wallet_transfers::verified_transfer_for_test(
            &conn,
            &b.user_id,
            &a.user_id,
            1,
            "2026-11-17T00:00:00Z",
        )
        .unwrap();
        assert!(!known_recipient_tx(&conn, &a.user_id, &b.user_id).unwrap());
        // Money the sender sent that credited does.
        crate::market_wallet_transfers::verified_transfer_for_test(
            &conn,
            &a.user_id,
            &b.user_id,
            1,
            "2026-11-17T00:00:00Z",
        )
        .unwrap();
        assert!(known_recipient_tx(&conn, &a.user_id, &b.user_id).unwrap());
        // A quote shown as delayed stays a confirmation, whatever happened since.
        assert_eq!(
            risk_tx(
                &conn,
                &a,
                "/v1/market-wallet/transfers",
                body("q-large").as_bytes()
            )
            .unwrap(),
            Risk::Confirm
        );
        conn.execute("UPDATE market_wallet_transfer_quotes SET first_recipient=0 WHERE id IN ('q-small','q-large')", []).unwrap();
        assert_eq!(
            risk_tx(
                &conn,
                &a,
                "/v1/market-wallet/transfers",
                body("q-small").as_bytes()
            )
            .unwrap(),
            Risk::None
        );
        assert_eq!(
            risk_tx(
                &conn,
                &a,
                "/v1/market-wallet/transfers",
                body("q-large").as_bytes()
            )
            .unwrap(),
            Risk::Immediate
        );
        // A positional body that a typed handler would still read is refused.
        for bad in [&b"[\"q-large\"]"[..], b"{}", b"null", b"{\"quoteId\":1}"] {
            assert!(risk_tx(&conn, &a, "/v1/market-wallet/transfers", bad).is_err());
        }
    }

    #[test]
    fn money_paths_require_a_matching_confirmation_inside_the_transaction() {
        let conn = crate::test_support::database().unwrap();
        let mut a = crate::market_finance_auth::test_session(&conn, "confirmed-tx");
        a.auth_source_id = "device".into();
        device(&conn, "device");
        // Internal callers without a request proof are not affected.
        require_confirmed_tx(&conn, &a, Risk::Immediate).unwrap();
        proof(&conn, &a, "plain", None);
        a.financial_proof_id = Some("plain".into());
        assert!(require_confirmed_tx(&conn, &a, Risk::Confirm).is_err());
        require_confirmed_tx(&conn, &a, Risk::None).unwrap();
        completed(&conn, &a, "low", "confirm");
        proof(&conn, &a, "with-confirm", Some("low"));
        a.financial_proof_id = Some("with-confirm".into());
        require_confirmed_tx(&conn, &a, Risk::Confirm).unwrap();
        assert!(require_confirmed_tx(&conn, &a, Risk::Immediate).is_err());
        completed(&conn, &a, "high", "immediate");
        proof(&conn, &a, "with-immediate", Some("high"));
        a.financial_proof_id = Some("with-immediate".into());
        require_confirmed_tx(&conn, &a, Risk::Immediate).unwrap();
    }

    #[test]
    fn only_a_protective_pause_skips_the_password_cooldown() {
        let conn = crate::test_support::database().unwrap();
        let a = crate::market_finance_auth::test_session(&conn, "pause");
        let path = "/v1/admin/market-finance/pause";
        for (body, risk) in [
            (
                &br#"{"newOrdersPaused":true,"expectedVersion":1}"#[..],
                Risk::Confirm,
            ),
            (
                br#"{"newOrdersPaused":true,"newTopupsPaused":true,"expectedVersion":1}"#,
                Risk::Confirm,
            ),
            (
                br#"{"newOrdersPaused":true,"newTopupsPaused":false,"expectedVersion":1}"#,
                Risk::Immediate,
            ),
            (
                br#"{"newOrdersPaused":false,"expectedVersion":1}"#,
                Risk::Immediate,
            ),
        ] {
            assert_eq!(risk_tx(&conn, &a, path, body).unwrap(), risk);
        }
    }

    #[test]
    fn password_cooldown_blocks_only_immediate_operations() {
        let conn = crate::test_support::database().unwrap();
        let a = crate::market_finance_auth::test_session(&conn, "cooldown");
        let time = "2026-11-17T12:00:00+00:00";
        let missing =
            require_password_ready_tx(&conn, &a.user_id, Risk::Confirm, time).unwrap_err();
        assert_eq!(missing.code(), Some("FINANCIAL_PASSWORD_REQUIRED"));
        password(&conn, &a.user_id, "2026-11-17T00:00:00+00:00");
        require_password_ready_tx(&conn, &a.user_id, Risk::Confirm, time).unwrap();
        let cooling =
            require_password_ready_tx(&conn, &a.user_id, Risk::Immediate, time).unwrap_err();
        assert_eq!(cooling.code(), Some("FINANCIAL_PASSWORD_COOLING"));
        require_password_ready_tx(
            &conn,
            &a.user_id,
            Risk::Immediate,
            "2026-11-18T00:00:00+00:00",
        )
        .unwrap();
        // The host operator's local CLI is trusted immediately.
        conn.execute("INSERT INTO auth_password_events(id,user_id,auth_version,source,actor,created_at) VALUES('cli-set',?1,(SELECT auth_version FROM users WHERE id=?1),'local_cli','uid:0',?2)", params![a.user_id, time]).unwrap();
        require_password_ready_tx(&conn, &a.user_id, Risk::Immediate, time).unwrap();
    }

    #[test]
    fn financial_signature_binds_every_operation_dimension() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let payload = binding(
            "https://router.test",
            "device",
            "session",
            "POST",
            "/v1/market-wallet/transfers",
            b"{\"quoteId\":\"q\"}",
            "idem",
        );
        let canonical = format!("{payload}\n123\nnonce");
        let public = STANDARD.encode(key.verifying_key().as_bytes());
        let signature = STANDARD.encode(key.sign(canonical.as_bytes()).to_bytes());
        verify_signature(&public, &signature, &canonical).unwrap();
        for changed in [
            canonical.replace("router.test", "share.router.test"),
            canonical.replace("POST", "PUT"),
            canonical.replace("session", "other"),
            canonical.replace("transfers", "transfer-quotes"),
            canonical.replace("idem", "new"),
            canonical.replace("nonce", "new"),
        ] {
            assert!(verify_signature(&public, &signature, &changed).is_err())
        }
        assert!(financial_write(
            &Method::POST,
            "/v1/admin/market-payments/channels"
        ));
        assert!(!financial_write(
            &Method::POST,
            "/v1/market-payments/webhook/c"
        ));
        assert!(financial_write(
            &Method::POST,
            "/v1/admin/market-payment-channels/c/webhook-secret"
        ));
    }

    #[test]
    fn a_reused_or_password_changed_confirmation_cannot_authorize_money() {
        let conn = crate::test_support::database().unwrap();
        let mut a = crate::market_finance_auth::test_session(&conn, "proof-test");
        a.auth_source_id = "device".into();
        device(&conn, "device");
        password(&conn, &a.user_id, "2020-01-01T00:00:00+00:00");
        completed(&conn, &a, "step", "immediate");
        proof(&conn, &a, "p1", Some("step"));
        proof(&conn, &a, "p2", Some("step"));
        let time = "2026-11-17T00:00:00+00:00";
        a.financial_proof_id = Some("p1".into());
        validate_proof_tx(&conn, &a, time).unwrap();
        validate_proof_tx(&conn, &a, time).unwrap();
        a.financial_proof_id = Some("p2".into());
        assert!(
            validate_proof_tx(&conn, &a, time).is_err(),
            "one confirmation, one execution"
        );
        // A password set after the confirmation re-enters the cooldown.
        completed(&conn, &a, "step-2", "immediate");
        proof(&conn, &a, "p3", Some("step-2"));
        password(&conn, &a.user_id, "2026-11-16T12:00:00+00:00");
        a.financial_proof_id = Some("p3".into());
        assert!(validate_proof_tx(&conn, &a, time).is_err());
        // A delayed-delivery confirmation is not subject to the cooldown.
        completed(&conn, &a, "step-3", "confirm");
        proof(&conn, &a, "p4", Some("step-3"));
        a.financial_proof_id = Some("p4".into());
        validate_proof_tx(&conn, &a, time).unwrap();
    }

    #[test]
    fn failed_money_transaction_does_not_consume_confirmation_and_pruning_keeps_used_evidence() {
        let c = crate::test_support::database().unwrap();
        let mut a = crate::market_finance_auth::test_session(&c, "grant-rollback");
        a.auth_source_id = "rollback-device".into();
        a.financial_proof_id = Some("rollback-proof".into());
        device(&c, "rollback-device");
        password(&c, &a.user_id, "2020-01-01T00:00:00+00:00");
        c.execute("INSERT INTO market_password_confirmations(id,user_id,session_id,binding,risk,expires_at,completed_at,created_at) VALUES('rollback-step',?1,?2,'binding','confirm','2030-01-01','2020-01-01','2020-01-01')",params![a.user_id,a.session_id]).unwrap();
        for id in ["rollback-proof", "unused-proof"] {
            c.execute("INSERT INTO market_financial_request_proofs(id,user_id,session_id,device_id,nonce,binding,canonical,signature,public_key,stepup_id,created_at,expires_at) VALUES(?1,?2,?3,'rollback-device',?1,'binding','','','','rollback-step','2020-01-01','2030-01-01')",params![id,a.user_id,a.session_id]).unwrap();
        }
        {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            validate_proof_tx(&tx, &a, "2026-11-17T00:00:00Z").unwrap();
            // A later business failure drops the whole transaction.
        }
        let used: Option<String> = c
            .query_row(
                "SELECT used_proof_id FROM market_password_confirmations WHERE id='rollback-step'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(used, None);
        {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            validate_proof_tx(&tx, &a, "2026-11-17T00:00:00Z").unwrap();
            tx.commit().unwrap();
        }
        prune_tx(&c, "2030-03-01T00:00:00Z").unwrap();
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM market_financial_request_proofs",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert!(
            c.execute(
                "UPDATE market_financial_request_proofs SET signature='tampered'",
                []
            )
            .is_err()
        );
    }

    #[test]
    fn passkeys_cannot_be_added_after_retirement() {
        let c = crate::test_support::database().unwrap();
        let a = crate::market_finance_auth::test_session(&c, "retired-passkey");
        assert!(c.execute("INSERT INTO market_passkeys(id,user_id,credential_id,credential_json,usable_after,created_at) VALUES('k',?1,'c','{}','2020-01-01','2020-01-01')",params![a.user_id]).is_err());
    }
}
