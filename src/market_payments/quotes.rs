//! Quotes do not create a gateway order or consume pending-order capacity.
use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct QuoteRequest {
    pub(super) channel_id: String,
    pub(super) amount_minor: i64,
    #[serde(default)]
    pub(super) token_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TopupQuote {
    pub id: String,
    #[serde(skip)]
    pub channel_id: String,
    #[serde(skip)]
    pub token_id: Option<String>,
    pub channel_revision: i64,
    pub channel_settings_version: i64,
    pub policy_version: i64,
    pub credit_minor: i64,
    pub fee_bps: i64,
    pub fee_minor: i64,
    pub pay_minor: i64,
    pub gateway_cost_minor: i64,
    pub expires_at: String,
}

pub(super) fn read_quote_tx(
    conn: &Connection,
    user: &str,
    id: &str,
) -> Result<TopupQuote, AppError> {
    conn.query_row(
        "SELECT id, channel_id, token_id, channel_revision, channel_settings_version,
                policy_version, credit_minor, fee_bps, fee_minor, pay_minor, gateway_cost_minor, expires_at
         FROM market_topup_quotes WHERE id = ?1 AND user_id = ?2",
        params![id, user], |row| Ok(TopupQuote {
            id: row.get(0)?, channel_id: row.get(1)?, token_id: row.get(2)?,
            channel_revision: row.get(3)?, channel_settings_version: row.get(4)?,
            policy_version: row.get(5)?, credit_minor: row.get(6)?, fee_bps: row.get(7)?,
            fee_minor: row.get(8)?, pay_minor: row.get(9)?, gateway_cost_minor: row.get(10)?,
            expires_at: row.get(11)?,
        }),
    ).optional().map_err(db_error("read top-up quote"))?
        .ok_or_else(stale_quote)
}

pub(super) fn stale_quote() -> AppError {
    AppError::Coded {
        status: StatusCode::CONFLICT,
        code: "MARKET_TOPUP_QUOTE_CHANGED",
        message: "top-up quote expired or changed; review the current total before paying".into(),
        details: serde_json::json!({}),
    }
}

pub(super) fn validate_method(
    channel: &TopupChannelSettings,
    token: Option<&str>,
) -> Result<(), AppError> {
    match (channel.order_mode.as_str(), token) {
        ("direct", Some(token))
            if token_rail(token).is_some()
                && (channel.allowed_methods.is_empty()
                    || channel.allowed_methods.iter().any(|method| method == token)) =>
        {
            Ok(())
        }
        ("direct", _) => Err(AppError::BadRequest(
            "choose an allowed payment method".into(),
        )),
        (_, Some(_)) => Err(AppError::BadRequest(
            "this channel selects the payment method at checkout".into(),
        )),
        _ => Ok(()),
    }
}

pub(super) async fn create_quote(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<QuoteRequest>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let session = require_session(&state, &headers).await?;
    let owner = crate::api::is_finance_owner(&state.config, &session.email);
    let conn = state.store.conn.lock().await;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("begin top-up quote"))?;
    let quote = quote_tx(&tx, &session.user_id, owner, input, Utc::now())?;
    tx.commit().map_err(db_error("commit top-up quote"))?;
    Ok((no_store(), Json(quote)))
}

pub(super) fn quote_tx(
    tx: &Connection,
    user_id: &str,
    owner: bool,
    input: QuoteRequest,
    at: chrono::DateTime<Utc>,
) -> Result<TopupQuote, AppError> {
    let (mode, approved): (String, bool) = tx
        .query_row(
            "SELECT mode, cutover_approved_at IS NOT NULL FROM market_finance_control WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error("read quote finance mode"))?;
    if !user_topup_allowed(&mode, owner, approved) {
        return Err(AppError::ServiceUnavailable(
            "platform wallet top-up is not enabled".into(),
        ));
    }
    require_new_topups_open_tx(tx)?;
    crate::market_reconcile::ensure_finance_scopes_safe_tx(
        tx,
        &[crate::market_reconcile::FinanceScope::User(user_id)],
    )?;
    let channel = topup_channel_tx(tx, &input.channel_id, owner)?;
    validate_method(&channel, input.token_id.as_deref())?;
    let policy = crate::market_wallet_policy::policy_tx(tx)?;
    let limits = TopupAmountLimits::new(
        channel.min_topup_minor,
        channel.max_topup_minor,
        policy.maintenance_fee_bps,
        policy.version,
    )?;
    let (fee, pay) = limits.validate_credit(input.amount_minor)?;
    let time = at.to_rfc3339();
    let recent: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM market_topup_quotes WHERE user_id = ?1 AND created_at >= ?2",
            params![user_id, (at - chrono::Duration::hours(1)).to_rfc3339()],
            |row| row.get(0),
        )
        .map_err(db_error("count top-up quotes"))?;
    if recent >= 240 {
        return Err(AppError::TooManyRequests(
            "top-up quote rate limit reached".into(),
        ));
    }
    let estimate = i64::try_from(
        (i128::from(pay) * i128::from(channel.cost_rate_bps) + 5_000) / 10_000
            + i128::from(channel.fixed_cost_minor),
    )
    .map_err(|_| AppError::BadRequest("gateway cost overflow".into()))?;
    let quote = TopupQuote {
        id: Uuid::new_v4().to_string(),
        channel_id: input.channel_id,
        token_id: input.token_id,
        channel_revision: channel.revision,
        channel_settings_version: channel.settings_version,
        policy_version: policy.version,
        credit_minor: input.amount_minor,
        fee_bps: policy.maintenance_fee_bps,
        fee_minor: fee,
        pay_minor: pay,
        gateway_cost_minor: estimate,
        expires_at: (at + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    tx.execute("INSERT INTO market_topup_quotes (id, user_id, channel_id, token_id, channel_revision,
        channel_settings_version, policy_version, credit_minor, fee_bps, fee_minor, pay_minor, gateway_cost_minor,
        expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![quote.id, user_id, quote.channel_id, quote.token_id, quote.channel_revision,
            quote.channel_settings_version, quote.policy_version, quote.credit_minor, quote.fee_bps,
            quote.fee_minor, quote.pay_minor, quote.gateway_cost_minor, quote.expires_at, time])
        .map_err(db_error("save top-up quote"))?;
    Ok(quote)
}

pub(super) fn validate_current_tx(
    conn: &Connection,
    quote: &TopupQuote,
    channel: &TopupChannelSettings,
    time: &str,
) -> Result<(), AppError> {
    let policy = crate::market_wallet_policy::policy_tx(conn)?;
    if quote.expires_at.as_str() <= time
        || quote.policy_version != policy.version
        || quote.channel_revision != channel.revision
        || quote.channel_settings_version != channel.settings_version
    {
        return Err(stale_quote());
    }
    let limits = TopupAmountLimits::new(
        channel.min_topup_minor,
        channel.max_topup_minor,
        policy.maintenance_fee_bps,
        policy.version,
    )?;
    let (fee, pay) = limits.validate_credit(quote.credit_minor)?;
    if (quote.fee_minor, quote.pay_minor) != (fee, pay) {
        return Err(stale_quote());
    }
    Ok(())
}
