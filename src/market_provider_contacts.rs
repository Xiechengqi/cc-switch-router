use crate::db::{OptionalExtension, params};
use crate::{ServerState, error::AppError, models::AuthSession, store::AppStore};
use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use chrono::Utc;
use serde::{Deserialize, Serialize};
const MAX_PAYMENT_CONTACTS: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderContact {
    /// `wechat` | `telegram` | `custom`
    pub channel: String,
    pub handle: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderContactsView {
    pub provider_id: String,
    pub owner_email: String,
    #[serde(default)]
    pub contacts: Vec<ProviderContact>,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateProviderContactsRequest {
    contacts: Vec<ProviderContact>,
}

pub fn router() -> Router<ServerState> {
    Router::new().route(
        "/v1/account/provider-contacts",
        get(get_provider_contacts).put(update_provider_contacts),
    )
}
async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<AuthSession, AppError> {
    crate::api::resolve_router_session(state, headers)
        .await?
        .ok_or_else(|| AppError::Unauthorized("authenticated owner session required".into()))
}

fn normalize_provider_contact(mut contact: ProviderContact) -> Result<ProviderContact, AppError> {
    contact.channel = contact.channel.trim().to_ascii_lowercase();
    contact.handle = contact.handle.trim().to_string();
    if !matches!(contact.channel.as_str(), "wechat" | "telegram" | "custom") {
        return Err(AppError::BadRequest(
            "contact channel must be wechat, telegram, or custom".into(),
        ));
    }
    if contact.handle.is_empty() || contact.handle.chars().count() > 200 {
        return Err(AppError::BadRequest(
            "contact handle is required and must be at most 200 characters".into(),
        ));
    }
    Ok(contact)
}

fn normalize_provider_contacts(
    contacts: Vec<ProviderContact>,
) -> Result<Vec<ProviderContact>, AppError> {
    if contacts.len() > MAX_PAYMENT_CONTACTS {
        return Err(AppError::BadRequest(
            "at most 10 contact entries are allowed".into(),
        ));
    }
    let mut output = Vec::with_capacity(contacts.len());
    for contact in contacts {
        let contact = normalize_provider_contact(contact)?;
        if output.iter().any(|existing: &ProviderContact| {
            existing.channel == contact.channel && existing.handle == contact.handle
        }) {
            return Err(AppError::BadRequest("duplicate contact entry".into()));
        }
        output.push(contact);
    }
    Ok(output)
}

async fn get_provider_contacts(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let profile = state
        .store
        .client_market_provider_contacts(&session.user_id, &session.email)
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({ "contacts": profile.contacts, "updatedAt": profile.updated_at })),
    ))
}

async fn update_provider_contacts(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateProviderContactsRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let contacts = normalize_provider_contacts(input.contacts)?;
    let profile = state
        .store
        .client_market_update_provider_contacts(&session, &contacts)
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({ "contacts": profile.contacts, "updatedAt": profile.updated_at })),
    ))
}

fn normalize_email(value: &str) -> Result<String, AppError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 320
        || value.chars().any(char::is_control)
        || !value.contains('@')
    {
        return Err(AppError::BadRequest("invalid account email".into()));
    }
    Ok(value)
}

impl AppStore {
    pub async fn client_market_provider_contacts(
        &self,
        user_id: &str,
        owner_email: &str,
    ) -> Result<ProviderContactsView, AppError> {
        let email = normalize_email(owner_email)?;
        let conn = self.conn.lock().await;
        let row: Option<(String, String, String)> = conn
            .query_row(
                "SELECT owner_email, contacts_json, updated_at
                 FROM market_provider_contacts WHERE user_id = ?1",
                params![user_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| {
                AppError::Internal(format!("read provider contacts failed: {error}"))
            })?;
        if let Some((owner_email, contacts_json, updated_at)) = row {
            return Ok(ProviderContactsView {
                provider_id: user_id.to_string(),
                owner_email,
                contacts: serde_json::from_str(&contacts_json).unwrap_or_default(),
                updated_at,
            });
        }
        Ok(ProviderContactsView {
            provider_id: user_id.to_string(),
            owner_email: email,
            contacts: Vec::new(),
            updated_at: Utc::now().to_rfc3339(),
        })
    }

    pub async fn client_market_update_provider_contacts(
        &self,
        session: &AuthSession,
        contacts: &[ProviderContact],
    ) -> Result<ProviderContactsView, AppError> {
        let email = normalize_email(&session.email)?;
        let now = Utc::now().to_rfc3339();
        let contacts_json = serde_json::to_string(contacts).map_err(|error| {
            AppError::Internal(format!("encode Provider contacts failed: {error}"))
        })?;
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO market_provider_contacts (user_id, owner_email, contacts_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET
                owner_email = excluded.owner_email,
                contacts_json = excluded.contacts_json,
                updated_at = excluded.updated_at",
            params![session.user_id, email, contacts_json, now],
        )
        .map_err(|error| AppError::Internal(format!("save Provider contacts failed: {error}")))?;
        drop(conn);
        self.client_market_provider_contacts(&session.user_id, &session.email)
            .await
    }
}
