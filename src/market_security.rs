//! Financial request possession proofs and independent, user-verified Passkeys.
//! The signed bytes bind the origin, session, route, body and idempotency key.
//! Passkey grants are consumed inside the final SQLite transaction, never by
//! an HTTP preflight. Authentication material is not financial authorization.
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
    extract::{Path, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{Duration, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use webauthn_rs::prelude::*;

const PROOF_HEADER: &str = "x-router-verified-financial-proof";
const REQUEST_TTL_SECONDS: i64 = 300;
const COOLING_HOURS: i64 = 24;
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
fn webauthn(state: &ServerState) -> Result<Webauthn, AppError> {
    let url = Url::parse(&origin(state))
        .map_err(|_| AppError::Internal("invalid Router origin".into()))?;
    WebauthnBuilder::new(
        url.host_str()
            .ok_or_else(|| AppError::Internal("Router host required".into()))?,
        &url,
    )
    .map_err(|e| AppError::Internal(format!("configure Passkeys: {e}")))?
    .rp_name("TokenSwitch Wallet")
    .build()
    .map_err(|e| AppError::Internal(format!("configure Passkeys: {e}")))
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
fn passkeys_tx(
    conn: &Connection,
    user: &str,
    time: &str,
    usable_only: bool,
) -> Result<Vec<(String, i64, Passkey)>, AppError> {
    let mut s=conn.prepare("SELECT id,version,credential_json FROM market_passkeys WHERE user_id=?1 AND revoked_at IS NULL AND (?3=0 OR julianday(usable_after)<=julianday(?2)) ORDER BY id").map_err(db_error("prepare Passkeys"))?;
    let rows = s
        .query_map(params![user, time, usable_only], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(db_error("read Passkeys"))?
        .collect::<crate::db::Result<Vec<_>>>()
        .map_err(db_error("read Passkeys"))?;
    rows.into_iter()
        .map(|(id, v, j)| {
            Ok((
                id,
                v,
                serde_json::from_str(&j)
                    .map_err(|e| AppError::Internal(format!("decode Passkey: {e}")))?,
            ))
        })
        .collect()
}
fn high_risk_tx(
    conn: &Connection,
    actor: &AuthSession,
    path: &str,
    body: &[u8],
) -> Result<bool, AppError> {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    if path == "/v1/market-wallet/security/passkeys/registration" {
        return Ok(conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_passkeys WHERE user_id=?1 AND revoked_at IS NULL)",
                params![actor.user_id],
                |r| r.get(0),
            )
            .map_err(db_error("check enrolled Passkeys"))?);
    }
    if path.starts_with("/v1/market-wallet/security/passkeys/")
        && path != "/v1/market-wallet/security/passkeys/registration/finish"
    {
        return Ok(true);
    }
    if path.starts_with("/v1/market-wallet/security/") {
        return Ok(false);
    }
    if path.starts_with("/v1/admin/market-") {
        return Ok(true);
    }
    if path == "/v1/market-wallet/transfers" {
        let quote = value["quoteId"].as_str().unwrap_or("");
        // Committed business replay must succeed even after a policy change or
        // challenge expiry. It still needs a fresh valid device-signed request.
        let prior: bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_wallet_transfers WHERE sender_user_id=?1 AND quote_id=?2)",params![actor.user_id,quote],|r|r.get(0)).map_err(db_error("read transfer replay"))?;
        if prior {
            return Ok(false);
        }
        let high: Option<bool>=conn.query_row("SELECT q.amount_units>=?3 OR NOT EXISTS(SELECT 1 FROM market_wallet_transfers t WHERE t.sender_user_id=q.sender_user_id AND t.recipient_user_id=q.recipient_user_id) FROM market_wallet_transfer_quotes q WHERE q.id=?1 AND q.sender_user_id=?2",params![quote,actor.user_id,crate::market_wallet::units_from_minor(50000)?],|r|r.get(0)).optional().map_err(db_error("classify transfer risk"))?;
        return Ok(high.unwrap_or(false));
    }
    if path.ends_with("/release") && path.contains("/c2c/orders/") {
        let segment = path
            .trim_end_matches("/release")
            .rsplit('/')
            .next()
            .unwrap_or("");
        // Axum decodes dynamic path parameters before the handler sees them.
        // Classify the same object while still signing the exact raw URI.
        let id = percent_encoding::percent_decode_str(segment)
            .decode_utf8()
            .map_err(|_| AppError::BadRequest("invalid order path".into()))?
            .into_owned();
        return Ok(conn
            .query_row(
                "SELECT amount_units>=?2 AND status!='released' FROM market_c2c_orders WHERE id=?1",
                params![id, crate::market_wallet::units_from_minor(50000)?],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error("classify C2C risk"))?
            .unwrap_or(false));
    }
    if let Some(path) = path.strip_suffix("/return/approve") {
        let id = percent_encoding::percent_decode_str(path.rsplit('/').next().unwrap_or(""))
            .decode_utf8()
            .map_err(|_| AppError::BadRequest("invalid transfer path".into()))?
            .into_owned();
        let committed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM market_transfer_returns r JOIN market_wallet_transfers t ON t.id=r.transfer_id WHERE r.transfer_id=?1 AND t.recipient_user_id=?2 AND r.status='approved')",params![id,actor.user_id],|r|r.get(0)).map_err(db_error("classify return replay"))?;
        return Ok(!committed);
    }
    Ok(false)
}
fn challenge_tx(
    conn: &Connection,
    actor: &AuthSession,
    kind: &str,
    binding: &str,
    state: &impl serde::Serialize,
    time: &str,
) -> Result<String, AppError> {
    let id = Uuid::new_v4().to_string();
    let count:i64=conn.query_row("SELECT COUNT(*) FROM market_security_challenges WHERE user_id=?1 AND julianday(created_at)>julianday(?2)-1.0/1440",params![actor.user_id,time],|r|r.get(0)).map_err(db_error("count security challenges"))?;
    if count >= 12 {
        return Err(AppError::TooManyRequests(
            "too many security challenges; retry in a minute".into(),
        ));
    }
    conn.execute("INSERT INTO market_security_challenges(id,user_id,session_id,kind,binding,state_json,expires_at,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,actor.user_id,actor.session_id,kind,binding,serde_json::to_string(state).map_err(|e|AppError::Internal(e.to_string()))?,(Utc::now()+Duration::minutes(5)).to_rfc3339(),time]).map_err(db_error("save security challenge"))?;
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
        let high = high_risk_tx(&tx, &actor, parts.uri.path(), &raw)?;
        let stepup = parts
            .headers
            .get("x-router-stepup")
            .and_then(|v| v.to_str().ok());
        if high && stepup.is_none() {
            let keys = passkeys_tx(&tx, &actor.user_id, &time, true)?
                .into_iter()
                .map(|(_, _, k)| k)
                .collect::<Vec<_>>();
            if keys.is_empty() {
                return Err(denied(
                    "FINANCIAL_PASSKEY_REQUIRED",
                    "Enroll a wallet Passkey and wait for its security cooling period, or ask the deployment administrator to approve it",
                ));
            }
            let (options, auth) = webauthn(state)?
                .start_passkey_authentication(&keys)
                .map_err(|e| AppError::Internal(format!("start Passkey: {e}")))?;
            let challenge = challenge_tx(&tx, &actor, "authenticate", &bound, &auth, &time)?;
            tx.commit()
                .map_err(db_error("commit financial challenge"))?;
            return Err(AppError::Coded {
                status: StatusCode::PRECONDITION_REQUIRED,
                code: "FINANCIAL_STEPUP_REQUIRED",
                message: "Confirm this financial operation with your Passkey".into(),
                details: json!({"challengeId":challenge,"options":options}),
            });
        }
        if high {
            let valid: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM market_security_challenges c JOIN market_passkeys p ON p.id=c.credential_id WHERE c.id=?1 AND c.user_id=?2 AND c.session_id=?3 AND c.binding=?4 AND c.kind='authenticate' AND c.completed_at IS NOT NULL AND c.used_proof_id IS NULL AND julianday(c.expires_at)>julianday(?5) AND p.revoked_at IS NULL AND julianday(p.usable_after)<=julianday(?5))",params![stepup,actor.user_id,actor.session_id,bound,time],|r|r.get(0)).map_err(db_error("check financial Passkey"))?;
            if !valid {
                return Err(denied(
                    "FINANCIAL_STEPUP_INVALID",
                    "confirmation expired or does not match this operation",
                ));
            }
        }
        tx.execute("INSERT INTO market_financial_request_proofs(id,user_id,session_id,device_id,nonce,binding,canonical,signature,public_key,stepup_id,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![id,actor.user_id,actor.session_id,device,nonce,bound,canonical,signature,key,if high{stepup}else{None},time,(Utc::now()+Duration::seconds(REQUEST_TTL_SECONDS)).to_rfc3339()]).map_err(db_error("save financial request proof"))?;
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
/// Called within the business transaction. One grant can authorize exactly one
/// signed HTTP execution; repeated checks inside that execution are permitted.
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
        let updated=conn.execute("UPDATE market_security_challenges SET used_proof_id=?1 WHERE id=?2 AND user_id=?3 AND session_id=?4 AND binding=?5 AND completed_at IS NOT NULL AND (used_proof_id IS NULL OR used_proof_id=?1) AND julianday(expires_at)>julianday(?6) AND EXISTS(SELECT 1 FROM market_passkeys p WHERE p.id=market_security_challenges.credential_id AND p.revoked_at IS NULL AND julianday(p.usable_after)<=julianday(?6))",params![id,stepup,actor.user_id,actor.session_id,bound,time]).map_err(db_error("consume financial confirmation"))?;
        if updated != 1 {
            return Err(denied(
                "FINANCIAL_STEPUP_INVALID",
                "financial confirmation already used or credential revoked",
            ));
        }
    }
    Ok(())
}
fn require_confirmation_tx(
    conn: &Connection,
    actor: &AuthSession,
    time: &str,
) -> Result<(), AppError> {
    let confirmed: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_financial_request_proofs
         WHERE id=?1 AND user_id=?2 AND session_id=?3 AND stepup_id IS NOT NULL)",
            params![actor.financial_proof_id, actor.user_id, actor.session_id],
            |r| r.get(0),
        )
        .map_err(db_error("recheck required financial confirmation"))?;
    if !confirmed {
        return Err(denied(
            "FINANCIAL_STEPUP_INVALID",
            "wallet credentials changed; retry with a new Passkey confirmation",
        ));
    }
    validate_proof_tx(conn, actor, time)
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
        .route(
            "/v1/market-wallet/security/passkeys/registration",
            post(register_start),
        )
        .route(
            "/v1/market-wallet/security/passkeys/registration/finish",
            post(register_finish),
        )
        .route(
            "/v1/market-wallet/security/authenticate",
            post(authenticate_finish),
        )
        .route(
            "/v1/market-wallet/security/passkeys/:id/revoke",
            post(revoke),
        )
}
fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "private, no-store")]
}
async fn status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let actor = crate::api::resolve_router_session(&state, &headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("login required".into()))?;
    let conn = state.store.conn.lock().await;
    let mut s=conn.prepare("SELECT id,usable_after,created_at,revoked_at FROM market_passkeys WHERE user_id=?1 ORDER BY created_at DESC").map_err(db_error("prepare wallet security"))?;
    let keys=s.query_map(params![actor.user_id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"usableAfter":r.get::<_,String>(1)?,"createdAt":r.get::<_,String>(2)?,"revokedAt":r.get::<_,Option<String>>(3)?}))).map_err(db_error("read wallet security"))?.collect::<crate::db::Result<Vec<_>>>().map_err(db_error("read wallet security"))?;
    Ok((
        no_store(),
        Json(
            json!({"sessionReference":actor.session_id,"canManageRiskPolicy":crate::api::is_finance_owner(&state.config,&actor.email),"passkeys":keys,"coolingHours":COOLING_HOURS,"largeTransferMinor":50000,"firstRecipientRequiresPasskey":true}),
        ),
    ))
}
async fn register_start(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let actor = crate::market_finance_auth::write_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Passkey registration"))?;
    let time = crate::market_wallet::now();
    crate::market_finance_auth::require_live_session_tx(&tx, &actor, &time)?;
    let all = passkeys_tx(&tx, &actor.user_id, &time, false)?;
    if all.len() >= 8 {
        return Err(AppError::Conflict(
            "at most eight active wallet Passkeys are allowed".into(),
        ));
    }
    if !all.is_empty() {
        // Another session may have completed its first enrollment after the
        // middleware preflight. Never add a second key with first-key authority.
        require_confirmation_tx(&tx, &actor, &time)?;
    } else {
        // A refreshed password session must never masquerade as fresh email proof.
        let fresh:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM user_sessions WHERE id=?1 AND email_authenticated_at IS NOT NULL AND julianday(email_authenticated_at)>=julianday(?2)-15.0/1440)",params![actor.session_id,time],|r|r.get(0)).map_err(db_error("check fresh email login"))?;
        let ever: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM market_passkeys WHERE user_id=?1)",
                params![actor.user_id],
                |r| r.get(0),
            )
            .map_err(db_error("check wallet recovery"))?;
        let recovery:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM market_passkey_recovery_permits WHERE session_id=?1 AND user_id=?2 AND used_at IS NULL AND julianday(expires_at)>julianday(?3))",params![actor.session_id,actor.user_id,time],|r|r.get(0)).map_err(db_error("check local wallet recovery permit"))?;
        if !fresh || (ever && !recovery) {
            return Err(denied(
                "FINANCIAL_ENROLLMENT_AUTH_REQUIRED",
                "First enrollment requires a fresh email-code login; recovery requires the deployment administrator",
            ));
        }
    }
    let digest = Sha256::digest(actor.user_id.as_bytes());
    let user_handle = Uuid::from_slice(&digest[..16])
        .map_err(|_| AppError::Internal("invalid Passkey user handle".into()))?;
    let (options, registration) = webauthn(&state)?
        .start_passkey_registration(
            user_handle,
            &actor.email,
            &actor.email,
            Some(
                all.into_iter()
                    .map(|(_, _, p)| p.cred_id().clone())
                    .collect(),
            ),
        )
        .map_err(|e| AppError::Internal(format!("start Passkey registration: {e}")))?;
    tx.execute("UPDATE market_security_challenges SET expires_at=?2 WHERE user_id=?1 AND kind='register' AND completed_at IS NULL",params![actor.user_id,time]).map_err(db_error("supersede registration ceremonies"))?;
    let id = challenge_tx(&tx, &actor, "register", "", &registration, &time)?;
    tx.commit()
        .map_err(db_error("commit Passkey registration"))?;
    Ok((
        no_store(),
        Json(json!({"challengeId":id,"options":options})),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Finish<T> {
    challenge_id: String,
    credential: T,
}
fn load_challenge_tx(
    conn: &Connection,
    actor: &AuthSession,
    id: &str,
    kind: &str,
    time: &str,
) -> Result<String, AppError> {
    conn.query_row("SELECT state_json FROM market_security_challenges WHERE id=?1 AND user_id=?2 AND session_id=?3 AND kind=?4 AND completed_at IS NULL AND julianday(expires_at)>julianday(?5)",params![id,actor.user_id,actor.session_id,kind,time],|r|r.get(0)).optional().map_err(db_error("load security challenge"))?.ok_or_else(||denied("FINANCIAL_CHALLENGE_EXPIRED","security challenge expired or already used"))
}
async fn register_finish(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<Finish<RegisterPublicKeyCredential>>,
) -> Result<impl IntoResponse, AppError> {
    let actor = crate::market_finance_auth::write_session(&state, &headers).await?;
    let wa = webauthn(&state)?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin Passkey registration finish"))?;
    let time = crate::market_wallet::now();
    crate::market_finance_auth::require_live_session_tx(&tx, &actor, &time)?;
    let saved: PasskeyRegistration = serde_json::from_str(&load_challenge_tx(
        &tx,
        &actor,
        &input.challenge_id,
        "register",
        &time,
    )?)
    .map_err(|e| AppError::Internal(e.to_string()))?;
    let recovery_after:Option<String>=tx.query_row("SELECT MAX(created_at) FROM market_security_events WHERE user_id=?1 AND kind='local_passkey_recovery'",params![actor.user_id],|r|r.get(0)).map_err(db_error("check registration recovery epoch"))?;
    let challenge_created: String = tx
        .query_row(
            "SELECT created_at FROM market_security_challenges WHERE id=?1",
            params![input.challenge_id],
            |r| r.get(0),
        )
        .map_err(db_error("read registration epoch"))?;
    if recovery_after
        .as_deref()
        .is_some_and(|t| t > challenge_created.as_str())
    {
        return Err(denied(
            "FINANCIAL_CHALLENGE_EXPIRED",
            "registration predates wallet recovery",
        ));
    }
    let active: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_passkeys WHERE user_id=?1 AND revoked_at IS NULL)",
            params![actor.user_id],
            |r| r.get(0),
        )
        .map_err(db_error("recheck registration authority"))?;
    let ever: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM market_passkeys WHERE user_id=?1)",
            params![actor.user_id],
            |r| r.get(0),
        )
        .map_err(db_error("recheck registration history"))?;
    if !active && ever && tx.execute("UPDATE market_passkey_recovery_permits SET used_at=?3 WHERE session_id=?1 AND user_id=?2 AND used_at IS NULL AND julianday(expires_at)>julianday(?3)",params![actor.session_id,actor.user_id,time]).map_err(db_error("consume local recovery permit"))?!=1 {
        return Err(denied("FINANCIAL_ENROLLMENT_AUTH_REQUIRED","wallet recovery permission expired or was used"));
    }
    let key = wa
        .finish_passkey_registration(&input.credential, &saved)
        .map_err(|_| denied("FINANCIAL_PASSKEY_INVALID", "Passkey registration failed"))?;
    let id = Uuid::new_v4().to_string();
    let usable = (Utc::now() + Duration::hours(COOLING_HOURS)).to_rfc3339();
    let count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM market_passkeys WHERE user_id=?1 AND revoked_at IS NULL",
            params![actor.user_id],
            |r| r.get(0),
        )
        .map_err(db_error("count wallet Passkeys"))?;
    if count >= 8 {
        return Err(AppError::Conflict("wallet Passkey limit reached".into()));
    }
    tx.execute("INSERT INTO market_passkeys(id,user_id,credential_id,credential_json,usable_after,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![id,actor.user_id,URL_SAFE_NO_PAD.encode(key.cred_id().as_ref()),serde_json::to_string(&key).map_err(|e|AppError::Internal(e.to_string()))?,usable,time]).map_err(db_error("save wallet Passkey"))?;
    tx.execute(
        "UPDATE market_security_challenges SET completed_at=?2,credential_id=?3 WHERE id=?1",
        params![input.challenge_id, time, id],
    )
    .map_err(db_error("finish registration challenge"))?;
    tx.execute("UPDATE market_security_challenges SET expires_at=?3 WHERE user_id=?1 AND id!=?2 AND kind='register' AND completed_at IS NULL",params![actor.user_id,input.challenge_id,time]).map_err(db_error("close other registration ceremonies"))?;
    event_tx(
        &tx,
        &actor.user_id,
        &actor.session_id,
        "passkey_registered",
        &id,
        json!({"usableAfter":usable,"proofId":actor.financial_proof_id}),
        &time,
    )?;
    crate::market_notifications::enqueue_user_notification_tx(
        &tx,
        "buyer",
        &actor.user_id,
        "wallet_passkey_registered",
        "wallet_security",
        &id,
        &format!("passkey_registered:{id}"),
        json!({"usableAfter":usable}),
        &time,
    )?;
    tx.commit().map_err(db_error("commit wallet Passkey"))?;
    Ok((no_store(), Json(json!({"id":id,"usableAfter":usable}))))
}
fn require_counter_advance(stored: u32, observed: u32) -> Result<(), AppError> {
    if (stored > 0 || observed > 0) && observed <= stored {
        return Err(denied(
            "FINANCIAL_PASSKEY_COUNTER_INVALID",
            "Passkey counter did not advance; use another Passkey or contact the deployment administrator",
        ));
    }
    Ok(())
}
async fn authenticate_finish(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<Finish<PublicKeyCredential>>,
) -> Result<impl IntoResponse, AppError> {
    let actor = crate::market_finance_auth::write_session(&state, &headers).await?;
    let wa = webauthn(&state)?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin financial Passkey verification"))?;
    let time = crate::market_wallet::now();
    crate::market_finance_auth::require_live_session_tx(&tx, &actor, &time)?;
    let saved: PasskeyAuthentication = serde_json::from_str(&load_challenge_tx(
        &tx,
        &actor,
        &input.challenge_id,
        "authenticate",
        &time,
    )?)
    .map_err(|e| AppError::Internal(e.to_string()))?;
    let result = wa
        .finish_passkey_authentication(&input.credential, &saved)
        .map_err(|_| denied("FINANCIAL_PASSKEY_INVALID", "Passkey verification failed"))?;
    let mut used = None;
    for (id, version, mut key) in passkeys_tx(&tx, &actor.user_id, &time, true)? {
        if key.cred_id() == result.cred_id() {
            // The ceremony snapshot may predate another concurrent assertion.
            // Compare against the latest persisted counter inside this transaction.
            let stored: Credential = key.clone().into();
            require_counter_advance(stored.counter, result.counter())?;
            if !result.user_verified() {
                return Err(denied(
                    "FINANCIAL_PASSKEY_INVALID",
                    "user verification is required",
                ));
            }
            key.update_credential(&result).ok_or_else(|| {
                denied("FINANCIAL_PASSKEY_INVALID", "Passkey credential mismatch")
            })?;
            let changed=tx.execute("UPDATE market_passkeys SET credential_json=?3,version=version+1 WHERE id=?1 AND version=?2 AND revoked_at IS NULL",params![id,version,serde_json::to_string(&key).map_err(|e|AppError::Internal(e.to_string()))?]).map_err(db_error("advance Passkey counter"))?;
            if changed != 1 {
                return Err(denied(
                    "FINANCIAL_PASSKEY_INVALID",
                    "Passkey changed; retry",
                ));
            }
            used = Some(id);
            break;
        }
    }
    let used = used.ok_or_else(|| {
        denied(
            "FINANCIAL_PASSKEY_INVALID",
            "Passkey is revoked or cooling down",
        )
    })?;
    tx.execute("UPDATE market_security_challenges SET completed_at=?2,credential_id=?3 WHERE id=?1 AND completed_at IS NULL",params![input.challenge_id,time,used]).map_err(db_error("complete financial confirmation"))?;
    event_tx(
        &tx,
        &actor.user_id,
        &actor.session_id,
        "operation_confirmed",
        &input.challenge_id,
        json!({"passkeyId":used,"proofId":actor.financial_proof_id}),
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit financial confirmation"))?;
    Ok((no_store(), Json(json!({"stepupId":input.challenge_id}))))
}
async fn revoke(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let actor = crate::market_finance_auth::write_session(&state, &headers).await?;
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin revoke wallet Passkey"))?;
    let time = crate::market_wallet::now();
    crate::market_finance_auth::require_live_session_tx(&tx, &actor, &time)?;
    if tx.execute("UPDATE market_passkeys SET revoked_at=COALESCE(revoked_at,?3),version=version+1 WHERE id=?1 AND user_id=?2",params![id,actor.user_id,time]).map_err(db_error("revoke wallet Passkey"))?!=1{return Err(AppError::NotFound("Passkey not found".into()))}
    tx.execute("UPDATE market_security_challenges SET expires_at=?2 WHERE user_id=?1 AND kind='register' AND completed_at IS NULL",params![actor.user_id,time]).map_err(db_error("invalidate registrations after credential revocation"))?;
    event_tx(
        &tx,
        &actor.user_id,
        &actor.session_id,
        "passkey_revoked",
        &id,
        json!({"proofId":actor.financial_proof_id}),
        &time,
    )?;
    tx.commit()
        .map_err(db_error("commit revoke wallet Passkey"))?;
    Ok((no_store(), Json(json!({"revoked":true}))))
}
/// The existing owner-UID-only Unix administration channel is the sole recovery
/// authority. Approve one specifically identified pending key, never a wildcard.
pub(crate) fn local_approve_tx(
    conn: &Connection,
    email: &str,
    key: &str,
    actor: &str,
    operation: &str,
    time: &str,
) -> Result<(), AppError> {
    Uuid::parse_str(operation)
        .map_err(|_| AppError::BadRequest("operation UUID required".into()))?;
    let prior: Option<(String, String)> = conn
        .query_row(
            "SELECT actor,object_id FROM market_security_events WHERE id=?1",
            params![operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error("read local security recovery"))?;
    if let Some((who, object)) = prior {
        return if who == actor && object == key {
            Ok(())
        } else {
            Err(AppError::Conflict("recovery operation conflict".into()))
        };
    }
    let user:String=conn.query_row("SELECT p.user_id FROM market_passkeys p JOIN users u ON u.id=p.user_id WHERE p.id=?1 AND u.email_normalized=?2 AND u.status='active' AND p.revoked_at IS NULL",params![key,email.trim().to_ascii_lowercase()],|r|r.get(0)).optional().map_err(db_error("read local Passkey approval"))?.ok_or_else(||AppError::NotFound("pending Passkey not found for this user".into()))?;
    conn.execute(
        "UPDATE market_passkeys SET usable_after=?2,version=version+1 WHERE id=?1",
        params![key, time],
    )
    .map_err(db_error("approve wallet Passkey"))?;
    conn.execute("INSERT INTO market_security_events(id,user_id,actor,kind,object_id,details_json,created_at) VALUES(?1,?2,?3,'local_passkey_approved',?4,'{}',?5)",params![operation,user,actor,key,time]).map_err(db_error("audit local Passkey approval"))?;
    Ok(())
}

pub(crate) fn local_recover_tx(
    conn: &Connection,
    email: &str,
    session: &str,
    actor: &str,
    operation: &str,
    time: &str,
) -> Result<(), AppError> {
    Uuid::parse_str(operation)
        .map_err(|_| AppError::BadRequest("operation UUID required".into()))?;
    let prior: Option<(String, String)> = conn
        .query_row(
            "SELECT actor,object_id FROM market_security_events WHERE id=?1",
            params![operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_error("read local recovery operation"))?;
    if let Some((who, object)) = prior {
        return if who == actor && object == session {
            Ok(())
        } else {
            Err(AppError::Conflict("recovery operation conflict".into()))
        };
    }
    let user:String=conn.query_row("SELECT s.user_id FROM user_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=?1 AND u.email_normalized=?2 AND u.status='active' AND s.revoked_at IS NULL AND s.auth_version=u.auth_version AND julianday(s.access_expires_at)>julianday(?3) AND julianday(s.email_authenticated_at)>julianday(?3)-15.0/1440",params![session,email.trim().to_ascii_lowercase(),time],|r|r.get(0)).optional().map_err(db_error("read independently approved recovery session"))?.ok_or_else(||denied("FINANCIAL_ENROLLMENT_AUTH_REQUIRED","a fresh email-code login session for this user is required"))?;
    conn.execute("UPDATE market_passkeys SET revoked_at=COALESCE(revoked_at,?2),version=version+1 WHERE user_id=?1 AND revoked_at IS NULL",params![user,time]).map_err(db_error("revoke wallet recovery credentials"))?;
    conn.execute("UPDATE market_security_challenges SET expires_at=?2 WHERE user_id=?1 AND completed_at IS NULL",params![user,time]).map_err(db_error("invalidate recovery ceremonies"))?;
    conn.execute("UPDATE user_sessions SET revoked_at=?3,rotated_at=NULL WHERE user_id=?1 AND id!=?2 AND revoked_at IS NULL",params![user,session,time]).map_err(db_error("revoke other recovery sessions"))?;
    conn.execute("INSERT INTO market_passkey_recovery_permits(session_id,user_id,operation_id,actor,expires_at,created_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(session_id) DO UPDATE SET operation_id=excluded.operation_id,actor=excluded.actor,expires_at=excluded.expires_at,used_at=NULL,created_at=excluded.created_at",params![session,user,operation,actor,(Utc::now()+Duration::minutes(15)).to_rfc3339(),time]).map_err(db_error("create local wallet recovery permit"))?;
    conn.execute("INSERT INTO market_security_events(id,user_id,actor,kind,object_id,details_json,created_at) VALUES(?1,?2,?3,'local_passkey_recovery',?4,'{}',?5)",params![operation,user,actor,session,time]).map_err(db_error("audit local wallet recovery"))?;
    crate::market_notifications::enqueue_user_notification_tx(
        conn,
        "buyer",
        &user,
        "wallet_passkey_recovery",
        "wallet_security",
        operation,
        &format!("wallet_recovery:{operation}"),
        json!({"sessionReference":session}),
        time,
    )?;
    Ok(())
}

pub(crate) fn prune_tx(conn: &Connection, time: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM market_financial_request_proofs WHERE id IN (SELECT p.id FROM market_financial_request_proofs p WHERE julianday(p.expires_at)<julianday(?1)-30 AND NOT EXISTS(SELECT 1 FROM market_financial_authorizations a WHERE a.request_proof_id=p.id) AND NOT EXISTS(SELECT 1 FROM market_transfer_return_consents c WHERE c.request_proof_id=p.id) AND NOT EXISTS(SELECT 1 FROM market_security_challenges c WHERE c.used_proof_id=p.id) AND NOT EXISTS(SELECT 1 FROM market_security_events e WHERE json_extract(e.details_json,'$.proofId')=p.id) AND NOT EXISTS(SELECT 1 FROM market_financial_risk_events e WHERE json_extract(e.details_json,'$.proofId')=p.id) AND NOT EXISTS(SELECT 1 FROM market_finance_reconciliation_cases c WHERE c.code='financial_request_proof_invalid' AND c.object_id=p.id) LIMIT 1000)",params![time]).map_err(db_error("prune unused financial request proofs"))?;
    conn.execute("DELETE FROM market_security_challenges WHERE id IN (SELECT c.id FROM market_security_challenges c WHERE julianday(c.expires_at)<julianday(?1)-30 AND c.completed_at IS NULL AND NOT EXISTS(SELECT 1 FROM market_financial_request_proofs p WHERE p.stepup_id=c.id) LIMIT 1000)",params![time]).map_err(db_error("prune expired security challenges"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    #[test]
    fn encoded_order_ids_cannot_skip_large_release_confirmation() {
        let conn = crate::test_support::database().unwrap();
        let actor = crate::market_finance_auth::test_session(&conn, "encoded-path");
        conn.execute_batch("CREATE TEMP TABLE market_c2c_orders(id TEXT PRIMARY KEY, amount_units INTEGER, status TEXT);
            INSERT INTO market_c2c_orders VALUES('c2c-large',4320000000,'paid'),('c2c-small',8640000,'paid');").unwrap();
        for id in ["c2c-large", "%632c-large", "c2c-%6carge"] {
            assert!(
                high_risk_tx(
                    &conn,
                    &actor,
                    &format!("/v1/market-wallet/c2c/orders/{id}/release"),
                    b"{}"
                )
                .unwrap()
            );
        }
        assert!(
            !high_risk_tx(
                &conn,
                &actor,
                "/v1/market-wallet/c2c/orders/%632c-small/release",
                b"{}"
            )
            .unwrap()
        );
        assert!(
            high_risk_tx(
                &conn,
                &actor,
                "/v1/admin/market-payment-channels/security/settings",
                b"{}",
            )
            .unwrap(),
            "a resource id cannot select the wallet-security ceremony exemption"
        );
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
    fn a_revoked_passkey_or_reused_grant_cannot_authorize_money() {
        let conn = crate::test_support::database().unwrap();
        let mut a = crate::market_finance_auth::test_session(&conn, "proof-test");
        a.auth_source_id = "device".into();
        a.financial_proof_id = Some("p1".into());
        conn.execute("INSERT INTO auth_devices(id,public_key,kind,platform,app_version,status,created_at,last_seen_at) VALUES('device','key','browser','test','test','active','2020-01-01','2020-01-01')",[]).unwrap();
        conn.execute("INSERT INTO market_passkeys(id,user_id,credential_id,credential_json,usable_after,created_at) VALUES('key',?1,'cred','{}','2020-01-01','2020-01-01')",params![a.user_id]).unwrap();
        conn.execute("INSERT INTO market_security_challenges(id,user_id,session_id,kind,binding,state_json,expires_at,completed_at,credential_id,created_at) VALUES('step',?1,?2,'authenticate','binding','{}','2100-01-01','2020-01-01','key','2020-01-01')",params![a.user_id,a.session_id]).unwrap();
        for id in ["p1", "p2"] {
            conn.execute("INSERT INTO market_financial_request_proofs(id,user_id,session_id,device_id,nonce,binding,canonical,signature,public_key,stepup_id,created_at,expires_at) VALUES(?1,?2,?3,'device',?1,'binding','','','','step','2020-01-01','2100-01-01')",params![id,a.user_id,a.session_id]).unwrap();
        }
        conn.execute("INSERT INTO market_financial_request_proofs(id,user_id,session_id,device_id,nonce,binding,canonical,signature,public_key,created_at,expires_at) VALUES('no-stepup',?1,?2,'device','no-stepup','binding','','','','2020-01-01','2100-01-01')",params![a.user_id,a.session_id]).unwrap();
        a.financial_proof_id = Some("no-stepup".into());
        assert!(
            require_confirmation_tx(&conn, &a, "2026-11-17T00:00:00Z").is_err(),
            "an enrollment preflight before the first key cannot add a second key afterwards"
        );
        a.financial_proof_id = Some("p1".into());
        require_confirmation_tx(&conn, &a, "2026-11-17T00:00:00Z").unwrap();
        validate_proof_tx(&conn, &a, "2026-11-17T00:00:00Z").unwrap();
        validate_proof_tx(&conn, &a, "2026-11-17T00:00:00Z").unwrap();
        a.financial_proof_id = Some("p2".into());
        assert!(validate_proof_tx(&conn, &a, "2026-11-17T00:00:00Z").is_err());
        a.financial_proof_id = Some("p1".into());
        conn.execute(
            "UPDATE market_passkeys SET revoked_at='2026-11-17' WHERE id='key'",
            [],
        )
        .unwrap();
        assert!(validate_proof_tx(&conn, &a, "2026-11-17T00:00:00Z").is_err());
    }
    #[test]
    fn passkey_counter_checks_current_state_and_allows_counterless_synced_keys() {
        assert!(require_counter_advance(0, 0).is_ok());
        assert!(require_counter_advance(0, 1).is_ok());
        assert!(require_counter_advance(7, 8).is_ok());
        for (stored, seen) in [(7, 7), (7, 6), (7, 0)] {
            assert!(require_counter_advance(stored, seen).is_err());
        }
    }
    #[test]
    fn failed_money_transaction_does_not_consume_grant_and_pruning_keeps_used_evidence() {
        let c = crate::test_support::database().unwrap();
        let mut a = crate::market_finance_auth::test_session(&c, "grant-rollback");
        a.auth_source_id = "rollback-device".into();
        a.financial_proof_id = Some("rollback-proof".into());
        c.execute("INSERT INTO auth_devices(id,public_key,kind,platform,app_version,status,created_at,last_seen_at) VALUES('rollback-device','key','browser','test','test','active','2020-01-01','2020-01-01')",[]).unwrap();
        c.execute("INSERT INTO market_passkeys(id,user_id,credential_id,credential_json,usable_after,created_at) VALUES('rollback-key',?1,'rollback-cred','{}','2020-01-01','2020-01-01')",params![a.user_id]).unwrap();
        c.execute("INSERT INTO market_security_challenges(id,user_id,session_id,kind,binding,state_json,expires_at,completed_at,credential_id,created_at) VALUES('rollback-step',?1,?2,'authenticate','binding','{}','2030-01-01','2020-01-01','rollback-key','2020-01-01')",params![a.user_id,a.session_id]).unwrap();
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
                "SELECT used_proof_id FROM market_security_challenges WHERE id='rollback-step'",
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
    fn local_recovery_requires_fresh_email_proof_revokes_other_sessions_and_replays_once() {
        let c = crate::test_support::database().unwrap();
        let a = crate::market_finance_auth::test_session(&c, "local-recovery");
        let time = "2026-11-17T12:00:00Z";
        c.execute("INSERT INTO market_passkeys(id,user_id,credential_id,credential_json,usable_after,created_at) VALUES('recovery-key',?1,'recovery-credential','{}','2020-01-01','2020-01-01')",params![a.user_id]).unwrap();
        let op = Uuid::new_v4().to_string();
        assert!(local_recover_tx(&c, &a.email, &a.session_id, "unix:0", &op, time).is_err());
        c.execute(
            "UPDATE user_sessions SET email_authenticated_at=?2 WHERE id=?1",
            params![a.session_id, time],
        )
        .unwrap();
        c.execute("INSERT INTO user_sessions(id,user_id,auth_source_kind,auth_source_id,access_token_hash,refresh_token_hash,access_expires_at,refresh_expires_at,created_at,last_used_at,auth_version) SELECT 'other-recovery-session',user_id,auth_source_kind,auth_source_id,'other-recovery-access','other-recovery-refresh',access_expires_at,refresh_expires_at,created_at,last_used_at,auth_version FROM user_sessions WHERE id=?1",params![a.session_id]).unwrap();
        c.execute("INSERT INTO market_security_challenges(id,user_id,session_id,kind,binding,state_json,expires_at,created_at) VALUES('old-recovery-challenge',?1,?2,'register','','{}','2100-01-01','2026-11-17T11:59:00Z')",params![a.user_id,a.session_id]).unwrap();
        {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            local_recover_tx(&tx, &a.email, &a.session_id, "unix:0", &op, time).unwrap();
            tx.commit().unwrap();
        }
        local_recover_tx(&c, &a.email, &a.session_id, "unix:0", &op, time).unwrap();
        assert!(
            !high_risk_tx(
                &c,
                &a,
                "/v1/market-wallet/security/passkeys/registration",
                b"",
            )
            .unwrap(),
            "locally approved recovery must be able to enroll without a revoked Passkey"
        );
        assert!(load_challenge_tx(&c, &a, "old-recovery-challenge", "register", time).is_err());
        assert!(crate::market_finance_auth::require_live_session_tx(&c, &a, time).is_ok());
        let (keys,sessions,events):(i64,i64,i64)=c.query_row("SELECT (SELECT COUNT(*) FROM market_passkeys WHERE revoked_at IS NULL),(SELECT COUNT(*) FROM user_sessions WHERE revoked_at IS NULL),(SELECT COUNT(*) FROM market_security_events WHERE kind='local_passkey_recovery')",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!((keys, sessions, events), (0, 1, 1));
        assert!(
            local_approve_tx(
                &c,
                &a.email,
                "recovery-key",
                "unix:0",
                &Uuid::new_v4().to_string(),
                time
            )
            .is_err(),
            "recovery-revoked keys cannot be reapproved"
        );
    }
}
