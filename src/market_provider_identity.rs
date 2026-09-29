use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::{Connection, OptionalExtension, params};
use crate::error::AppError;

const PROFILE_PREFIX: &str = "mp_";

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentityConflictRecord {
    profile_ids: Vec<String>,
    alias_kind: String,
    alias_value: String,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentityConflictCandidate {
    profile_id: String,
    canonical_email: String,
}

/// Conflict evidence is returned to write paths so they can roll back their
/// business transaction and then persist the fence in a fresh transaction.
/// Otherwise the error response would silently erase the audit rows that
/// explain why the operation was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarketProviderIdentityConflict {
    candidate: Option<IdentityConflictCandidate>,
    records: Vec<IdentityConflictRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarketProviderIdentityResolution {
    Resolved(String),
    Conflict(MarketProviderIdentityConflict),
}

impl MarketProviderIdentityConflict {
    fn empty() -> Self {
        Self {
            candidate: None,
            records: Vec::new(),
        }
    }

    fn with_record(
        candidate: Option<IdentityConflictCandidate>,
        record: IdentityConflictRecord,
    ) -> Self {
        Self {
            candidate,
            records: vec![record],
        }
    }
}

fn conflict_record(
    profile_ids: impl IntoIterator<Item = String>,
    alias_kind: &str,
    alias_value: &str,
    detail: &str,
) -> IdentityConflictRecord {
    IdentityConflictRecord {
        profile_ids: profile_ids.into_iter().collect(),
        alias_kind: alias_kind.to_string(),
        alias_value: alias_value.to_string(),
        detail: detail.to_string(),
    }
}

fn map_db(context: &'static str) -> impl FnOnce(crate::db::Error) -> AppError {
    move |error| AppError::Internal(format!("{context} failed: {error}"))
}

fn normalize_email(value: &str) -> Option<String> {
    let email = value.trim().to_ascii_lowercase();
    (!email.is_empty() && email.contains('@')).then_some(email)
}

fn new_profile_id() -> String {
    format!("{PROFILE_PREFIX}{}", Uuid::new_v4().simple())
}

fn default_display_name(profile_id: &str) -> String {
    let suffix = profile_id
        .strip_prefix(PROFILE_PREFIX)
        .unwrap_or(profile_id)
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .to_ascii_uppercase();
    format!("Provider {suffix}")
}

fn require_monotonic_timestamp(
    conn: &Connection,
    earlier: &str,
    later: &str,
    context: &'static str,
) -> Result<(), AppError> {
    let ordered = conn
        .query_row(
            "SELECT CASE
                WHEN julianday(?1) IS NULL OR julianday(?2) IS NULL THEN NULL
                WHEN julianday(?2) >= julianday(?1) THEN 1 ELSE 0
             END",
            params![earlier, later],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(map_db(context))?;
    match ordered {
        Some(1) => Ok(()),
        Some(_) => Err(AppError::Conflict(
            "Market Provider Share window timestamp moved backwards".into(),
        )),
        None => Err(AppError::Internal(
            "Market Provider Share window timestamp is invalid".into(),
        )),
    }
}

fn alias_profile_tx(tx: &Connection, kind: &str, value: &str) -> Result<Option<String>, AppError> {
    tx.query_row(
        "SELECT market_provider_id FROM market_provider_aliases
         WHERE alias_kind = ?1 AND alias_value = ?2",
        params![kind, value],
        |row| row.get(0),
    )
    .optional()
    .map_err(map_db("read Market Provider alias"))
}

fn profile_for_email_tx(tx: &Connection, email: &str) -> Result<Option<String>, AppError> {
    tx.query_row(
        "SELECT id FROM market_provider_profiles WHERE canonical_email = ?1",
        params![email],
        |row| row.get(0),
    )
    .optional()
    .map_err(map_db("read Market Provider email identity"))
}

fn profile_for_user_tx(tx: &Connection, user_id: &str) -> Result<Option<String>, AppError> {
    tx.query_row(
        "SELECT id FROM market_provider_profiles WHERE user_id = ?1",
        params![user_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(map_db("read Market Provider user identity"))
}

fn profile_claim_tx(
    tx: &Connection,
    profile_id: &str,
) -> Result<Option<(Option<String>, String, String)>, AppError> {
    tx.query_row(
        "SELECT user_id, canonical_email, status
         FROM market_provider_profiles WHERE id = ?1",
        params![profile_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(map_db("read Market Provider claim"))
}

fn record_conflict_tx(
    tx: &Connection,
    record: &IdentityConflictRecord,
    now: &str,
) -> Result<(), AppError> {
    let mut recorded = false;
    for profile_id in record.profile_ids.iter().collect::<BTreeSet<_>>() {
        let changed = tx
            .execute(
                "UPDATE market_provider_profiles
             SET status = 'identity_conflict', updated_at = ?2 WHERE id = ?1",
                params![profile_id, now],
            )
            .map_err(map_db("fence conflicting Market Provider identity"))?;
        if changed == 0 {
            continue;
        }
        tx.execute(
            "INSERT INTO market_provider_identity_events (
                id, market_provider_id, event_kind, alias_kind,
                alias_value, detail_json, created_at
             ) VALUES (?1, ?2, 'identity_conflict', ?3, ?4, ?5, ?6)",
            params![
                Uuid::new_v4().to_string(),
                profile_id,
                record.alias_kind,
                record.alias_value,
                serde_json::json!({ "reason": record.detail }).to_string(),
                now,
            ],
        )
        .map_err(map_db("audit Market Provider identity conflict"))?;
        recorded = true;
    }
    if !recorded {
        tx.execute(
            "INSERT INTO market_provider_identity_events (
                id, market_provider_id, event_kind, alias_kind,
                alias_value, detail_json, created_at
             ) VALUES (?1, NULL, 'identity_conflict', ?2, ?3, ?4, ?5)",
            params![
                Uuid::new_v4().to_string(),
                record.alias_kind,
                record.alias_value,
                serde_json::json!({
                    "reason": record.detail,
                    "missingProfileIds": record.profile_ids,
                })
                .to_string(),
                now,
            ],
        )
        .map_err(map_db("audit unbound Market Provider identity conflict"))?;
    }
    Ok(())
}

fn bind_profile_sources_tx(
    tx: &Connection,
    profile_id: &str,
    email: &str,
    now: &str,
) -> Result<Vec<IdentityConflictRecord>, AppError> {
    let host_aliases = tx
        .prepare(
            "SELECT provider_id, market_provider_id FROM host_provider_profiles
             WHERE lower(trim(owner_email)) = ?1 ORDER BY provider_id",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![email], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read Host Provider aliases"))?;
    let share_bindings = tx
        .prepare(
            "SELECT DISTINCT market_provider_id FROM share_market_listings
             WHERE lower(trim(owner_email)) = ?1 AND market_provider_id IS NOT NULL",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![email], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read Share Market Provider bindings"))?;
    let mut conflicts = Vec::new();
    for (alias, direct_binding) in &host_aliases {
        let mut conflicting_profiles = BTreeSet::new();
        if let Some(bound) = direct_binding
            && bound != profile_id
        {
            conflicting_profiles.insert(bound.clone());
        }
        if let Some(bound) = alias_profile_tx(tx, "host_provider_id", alias)?
            && bound != profile_id
        {
            conflicting_profiles.insert(bound);
        }
        if !conflicting_profiles.is_empty() {
            let mut profile_ids = vec![profile_id.to_string()];
            profile_ids.extend(conflicting_profiles);
            let record = conflict_record(
                profile_ids,
                "host_provider_id",
                alias,
                "Host Provider source is already bound to another public identity",
            );
            record_conflict_tx(tx, &record, now)?;
            conflicts.push(record);
        }
    }
    for bound in share_bindings {
        if bound == profile_id {
            continue;
        }
        let record = conflict_record(
            [profile_id.to_string(), bound],
            "email",
            email,
            "Share Market source is already bound to another public identity",
        );
        record_conflict_tx(tx, &record, now)?;
        conflicts.push(record);
    }
    if !conflicts.is_empty() {
        return Ok(conflicts);
    }

    tx.execute(
        "UPDATE host_provider_profiles
         SET market_provider_id = ?1
         WHERE lower(trim(owner_email)) = ?2",
        params![profile_id, email],
    )
    .map_err(map_db("bind Host profile to Market Provider"))?;
    tx.execute(
        "UPDATE share_market_listings
         SET market_provider_id = ?1
         WHERE lower(trim(owner_email)) = ?2",
        params![profile_id, email],
    )
    .map_err(map_db("bind Share listing to Market Provider"))?;
    tx.execute(
        "INSERT INTO market_provider_share_windows (
            id, listing_id, market_provider_id, starts_at, ends_at, created_at
         )
         SELECT 'mpw_' || lower(hex(randomblob(12))), listing.id, ?1,
                listing.created_at,
                CASE WHEN listing.status = 'active' AND listing.deleted_at IS NULL
                     THEN NULL
                     ELSE COALESCE(listing.deleted_at, listing.updated_at)
                END,
                ?3
         FROM share_market_listings listing
         WHERE lower(trim(listing.owner_email)) = ?2
           AND listing.market_provider_id = ?1
           AND NOT EXISTS (
               SELECT 1 FROM market_provider_share_windows window
               WHERE window.listing_id = listing.id
           )",
        params![profile_id, email, now],
    )
    .map_err(map_db("seed Market Provider Share windows"))?;
    tx.execute(
        "INSERT INTO market_provider_share_windows (
            id, listing_id, market_provider_id, starts_at, ends_at, created_at
         )
         SELECT 'mpw_' || lower(hex(randomblob(12))), listing.id, ?1,
                ?3, NULL, ?3
         FROM share_market_listings listing
         WHERE lower(trim(listing.owner_email)) = ?2
           AND listing.market_provider_id = ?1
           AND listing.status = 'active' AND listing.deleted_at IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM market_provider_share_windows window
               WHERE window.listing_id = listing.id AND window.ends_at IS NULL
           )",
        params![profile_id, email, now],
    )
    .map_err(map_db("repair open Market Provider Share windows"))?;

    for (alias, _) in host_aliases {
        tx.execute(
            "INSERT OR IGNORE INTO market_provider_aliases (
                market_provider_id, alias_kind, alias_value, created_at
             ) VALUES (?1, 'host_provider_id', ?2, ?3)",
            params![profile_id, alias, now],
        )
        .map_err(map_db("write Host Provider alias"))?;
    }
    Ok(Vec::new())
}

/// Resolve or create the public identity for one supplier. A verified Router
/// user claims an existing email-only profile in place so public ids never
/// change. Ambiguous aliases are fenced and audited instead of guessed.
pub(crate) fn ensure_market_provider_identity_tx(
    tx: &Connection,
    user_id: Option<&str>,
    raw_email: &str,
    now: &str,
) -> Result<MarketProviderIdentityResolution, AppError> {
    let Some(email) = normalize_email(raw_email) else {
        return Ok(MarketProviderIdentityResolution::Conflict(
            MarketProviderIdentityConflict::empty(),
        ));
    };
    // Historical Host and Share rows can exist before their owner has a Router
    // user. Keep those identities email-only until an authoritative user row
    // exists instead of weakening the profile's users foreign key. A later
    // reconciliation claims the same profile in place, preserving its public id.
    let claim_user_id = match user_id {
        Some(user_id) => {
            let authoritative_email = tx
                .query_row(
                    "SELECT email_normalized FROM users WHERE id = ?1",
                    params![user_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_db("read Market Provider claim user"))?;
            match authoritative_email.as_deref().and_then(normalize_email) {
                Some(authoritative_email) if authoritative_email == email => Some(user_id),
                Some(_) => {
                    return Ok(MarketProviderIdentityResolution::Conflict(
                        MarketProviderIdentityConflict::empty(),
                    ));
                }
                None => None,
            }
        }
        None => None,
    };
    let by_user_alias = match claim_user_id {
        Some(user_id) => alias_profile_tx(tx, "user_id", user_id)?,
        None => None,
    };
    let by_user_profile = match claim_user_id {
        Some(user_id) => profile_for_user_tx(tx, user_id)?,
        None => None,
    };
    if let (Some(alias_profile), Some(direct_profile)) = (&by_user_alias, &by_user_profile)
        && alias_profile != direct_profile
    {
        let record = conflict_record(
            [alias_profile.clone(), direct_profile.clone()],
            "user_id",
            claim_user_id.unwrap_or_default(),
            "user alias and claimed profile resolve to different public identities",
        );
        record_conflict_tx(tx, &record, now)?;
        return Ok(MarketProviderIdentityResolution::Conflict(
            MarketProviderIdentityConflict::with_record(None, record),
        ));
    }
    let by_user = by_user_alias.or(by_user_profile);
    let by_email_alias = alias_profile_tx(tx, "email", &email)?;
    let by_email_profile = profile_for_email_tx(tx, &email)?;
    if let (Some(alias_profile), Some(direct_profile)) = (&by_email_alias, &by_email_profile)
        && alias_profile != direct_profile
    {
        let record = conflict_record(
            [alias_profile.clone(), direct_profile.clone()],
            "email",
            &email,
            "email alias and canonical profile resolve to different public identities",
        );
        record_conflict_tx(tx, &record, now)?;
        return Ok(MarketProviderIdentityResolution::Conflict(
            MarketProviderIdentityConflict::with_record(None, record),
        ));
    }
    let by_email = by_email_alias.or(by_email_profile);

    if let (Some(user_profile), Some(email_profile)) = (&by_user, &by_email)
        && user_profile != email_profile
    {
        let record = conflict_record(
            [user_profile.clone(), email_profile.clone()],
            "email",
            &email,
            "verified user and email aliases resolve to different public identities",
        );
        record_conflict_tx(tx, &record, now)?;
        return Ok(MarketProviderIdentityResolution::Conflict(
            MarketProviderIdentityConflict::with_record(None, record),
        ));
    }

    let profile_id = by_user.or(by_email).unwrap_or_else(new_profile_id);
    let conflict_candidate = IdentityConflictCandidate {
        profile_id: profile_id.clone(),
        canonical_email: email.clone(),
    };
    if let Some((existing_user, canonical_email, status)) = profile_claim_tx(tx, &profile_id)? {
        if canonical_email != email {
            let record = conflict_record(
                [profile_id.clone()],
                "email",
                &email,
                "claimed identity canonical email differs from its authoritative user",
            );
            record_conflict_tx(tx, &record, now)?;
            return Ok(MarketProviderIdentityResolution::Conflict(
                MarketProviderIdentityConflict::with_record(
                    Some(conflict_candidate.clone()),
                    record,
                ),
            ));
        }
        if matches!(status.as_str(), "identity_conflict" | "retired") {
            return Ok(MarketProviderIdentityResolution::Conflict(
                MarketProviderIdentityConflict::empty(),
            ));
        }
        if let (Some(existing_user), Some(user_id)) = (existing_user.as_deref(), claim_user_id)
            && existing_user != user_id
        {
            let record = conflict_record(
                [profile_id.clone()],
                "user_id",
                user_id,
                "public identity is already claimed by another user",
            );
            record_conflict_tx(tx, &record, now)?;
            return Ok(MarketProviderIdentityResolution::Conflict(
                MarketProviderIdentityConflict::with_record(
                    Some(conflict_candidate.clone()),
                    record,
                ),
            ));
        }
        tx.execute(
            "UPDATE market_provider_profiles
             SET user_id = COALESCE(user_id, ?2),
                 claim_state = CASE WHEN ?2 IS NULL THEN claim_state ELSE 'claimed' END,
                 claimed_at = CASE WHEN ?2 IS NULL THEN claimed_at
                                   ELSE COALESCE(claimed_at, ?4) END,
                 display_name = CASE WHEN display_name = 'Provider' THEN ?3 ELSE display_name END,
                 updated_at = ?4
            WHERE id = ?1",
            params![
                profile_id,
                claim_user_id,
                default_display_name(&profile_id),
                now
            ],
        )
        .map_err(map_db("claim Market Provider identity"))?;
    } else {
        tx.execute(
            "INSERT INTO market_provider_profiles (
                id, user_id, canonical_email, display_name, claim_state,
                status, created_at, updated_at, claimed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6, ?7)",
            params![
                profile_id,
                claim_user_id,
                email,
                default_display_name(&profile_id),
                if claim_user_id.is_some() {
                    "claimed"
                } else {
                    "unclaimed"
                },
                now,
                claim_user_id.map(|_| now),
            ],
        )
        .map_err(map_db("create Market Provider identity"))?;
    }

    for (kind, value) in [
        ("email", email.as_str()),
        ("user_id", claim_user_id.unwrap_or("")),
    ] {
        if value.is_empty() {
            continue;
        }
        if let Some(bound) = alias_profile_tx(tx, kind, value)?
            && bound != profile_id
        {
            let record = conflict_record(
                [profile_id.clone(), bound],
                kind,
                value,
                "alias is already bound to another public identity",
            );
            record_conflict_tx(tx, &record, now)?;
            return Ok(MarketProviderIdentityResolution::Conflict(
                MarketProviderIdentityConflict::with_record(
                    Some(conflict_candidate.clone()),
                    record,
                ),
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO market_provider_aliases (
                market_provider_id, alias_kind, alias_value, created_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![profile_id, kind, value, now],
        )
        .map_err(map_db("write Market Provider alias"))?;
    }
    let conflicts = bind_profile_sources_tx(tx, &profile_id, &email, now)?;
    if !conflicts.is_empty() {
        return Ok(MarketProviderIdentityResolution::Conflict(
            MarketProviderIdentityConflict {
                candidate: Some(conflict_candidate),
                records: conflicts,
            },
        ));
    }
    Ok(MarketProviderIdentityResolution::Resolved(profile_id))
}

/// Persist a conflict after the surrounding business transaction has been
/// rolled back. A profile created only inside that failed transaction is
/// restored as an unclaimed, fenced identity so its opaque id and audit trail
/// remain stable for operator repair.
pub(crate) fn persist_market_provider_identity_conflict(
    conn: &Connection,
    conflict: &MarketProviderIdentityConflict,
    now: &str,
) -> Result<(), AppError> {
    if conflict.records.is_empty() {
        return Ok(());
    }
    let tx = conn
        .transaction_with_behavior(crate::db::TransactionBehavior::Immediate)
        .map_err(map_db("begin Market Provider conflict audit"))?;
    if let Some(candidate) = &conflict.candidate {
        let exists = profile_claim_tx(&tx, &candidate.profile_id)?.is_some();
        if !exists && profile_for_email_tx(&tx, &candidate.canonical_email)?.is_none() {
            tx.execute(
                "INSERT INTO market_provider_profiles (
                    id, user_id, canonical_email, display_name, claim_state,
                    status, created_at, updated_at, claimed_at
                 ) VALUES (?1, NULL, ?2, ?3, 'unclaimed',
                           'identity_conflict', ?4, ?4, NULL)",
                params![
                    candidate.profile_id,
                    candidate.canonical_email,
                    default_display_name(&candidate.profile_id),
                    now,
                ],
            )
            .map_err(map_db("restore conflicted Market Provider candidate"))?;
            tx.execute(
                "INSERT OR IGNORE INTO market_provider_aliases (
                    market_provider_id, alias_kind, alias_value, created_at
                 ) VALUES (?1, 'email', ?2, ?3)",
                params![candidate.profile_id, candidate.canonical_email, now],
            )
            .map_err(map_db("restore conflicted Market Provider email alias"))?;
        }
    }
    for record in &conflict.records {
        record_conflict_tx(&tx, record, now)?;
    }
    tx.commit()
        .map_err(map_db("commit Market Provider conflict audit"))
}

/// Close the current half-open Share publication/ownership window. Missing
/// windows are tolerated for legacy or conflict-fenced listings, but an
/// already-closed window is never rewritten.
pub(crate) fn close_share_window_tx(
    conn: &Connection,
    listing_id: &str,
    now: &str,
) -> Result<bool, AppError> {
    let starts_at = conn
        .query_row(
            "SELECT starts_at FROM market_provider_share_windows
             WHERE listing_id = ?1 AND ends_at IS NULL",
            params![listing_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read open Market Provider Share window start"))?;
    let Some(starts_at) = starts_at else {
        return Ok(false);
    };
    require_monotonic_timestamp(
        conn,
        &starts_at,
        now,
        "validate Market Provider Share window close time",
    )?;
    conn.execute(
        "UPDATE market_provider_share_windows
         SET ends_at = ?2
         WHERE listing_id = ?1 AND ends_at IS NULL",
        params![listing_id, now],
    )
    .map(|changed| changed > 0)
    .map_err(map_db("close Market Provider Share window"))
}

/// Open a Share publication/ownership window for the listing's current public
/// identity. This is idempotent only for the same identity; a different open
/// identity is an invariant violation and fails closed.
pub(crate) fn open_share_window_tx(
    conn: &Connection,
    listing_id: &str,
    now: &str,
) -> Result<bool, AppError> {
    let listing = conn
        .query_row(
            "SELECT listing.market_provider_id, listing.status, listing.deleted_at,
                    profile.status
             FROM share_market_listings listing
             LEFT JOIN market_provider_profiles profile
               ON profile.id = listing.market_provider_id
             WHERE listing.id = ?1",
            params![listing_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(map_db("read Market Provider Share window identity"))?;
    let Some((Some(market_provider_id), listing_status, deleted_at, profile_status)) = listing
    else {
        return Ok(false);
    };
    if listing_status != "active" || deleted_at.is_some() {
        return Err(AppError::Conflict(
            "only an active Share listing can open a Market Provider window".into(),
        ));
    }
    if !matches!(profile_status.as_deref(), Some("active") | Some("paused")) {
        return Err(AppError::Conflict(
            "Share listing Market Provider identity is not publishable".into(),
        ));
    }
    let open_provider = conn
        .query_row(
            "SELECT market_provider_id FROM market_provider_share_windows
             WHERE listing_id = ?1 AND ends_at IS NULL",
            params![listing_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read open Market Provider Share window"))?;
    if let Some(open_provider) = open_provider {
        if open_provider == market_provider_id {
            return Ok(false);
        }
        return Err(AppError::Conflict(
            "Share listing has a conflicting open Market Provider window".into(),
        ));
    }
    let invalid_history: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM market_provider_share_windows
                WHERE listing_id = ?1
                  AND (julianday(starts_at) IS NULL
                       OR (ends_at IS NOT NULL AND julianday(ends_at) IS NULL))
             )",
            params![listing_id],
            |row| row.get(0),
        )
        .map_err(map_db("validate Market Provider Share window history"))?;
    if invalid_history {
        return Err(AppError::Internal(
            "Market Provider Share window history contains an invalid timestamp".into(),
        ));
    }
    let latest_end = conn
        .query_row(
            "SELECT ends_at FROM market_provider_share_windows
             WHERE listing_id = ?1 AND ends_at IS NOT NULL
             ORDER BY julianday(ends_at) DESC LIMIT 1",
            params![listing_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db("read latest Market Provider Share window boundary"))?;
    if let Some(latest_end) = latest_end {
        require_monotonic_timestamp(
            conn,
            &latest_end,
            now,
            "validate Market Provider Share window open time",
        )?;
    } else {
        require_monotonic_timestamp(
            conn,
            now,
            now,
            "validate Market Provider Share window open timestamp",
        )?;
    }
    conn.execute(
        "INSERT INTO market_provider_share_windows (
            id, listing_id, market_provider_id, starts_at, ends_at, created_at
         ) VALUES (?1, ?2, ?3, ?4, NULL, ?4)",
        params![
            format!("mpw_{}", Uuid::new_v4().simple()),
            listing_id,
            market_provider_id,
            now,
        ],
    )
    .map_err(map_db("open Market Provider Share window"))?;
    Ok(true)
}

/// Startup and periodic reconciliation. It is the only broad repair path;
/// public GET handlers remain read-only.
pub(crate) fn reconcile_conn(conn: &Connection, now: DateTime<Utc>) -> Result<usize, AppError> {
    let now = now.to_rfc3339();
    let tx = conn
        .transaction_with_behavior(crate::db::TransactionBehavior::Immediate)
        .map_err(map_db("begin Market Provider identity reconciliation"))?;
    let claimed = tx
        .prepare(
            "SELECT u.id, u.email_normalized
             FROM users u
             WHERE EXISTS (SELECT 1 FROM host_provider_profiles host
                           WHERE lower(trim(host.owner_email)) = u.email_normalized)
                OR EXISTS (SELECT 1 FROM share_market_listings listing
                           WHERE lower(trim(listing.owner_email)) = u.email_normalized)
                OR EXISTS (SELECT 1 FROM market_counterparties counterparty
                           WHERE counterparty.supplier_user_id = u.id)
                OR EXISTS (SELECT 1 FROM market_credit_accounts credit
                           WHERE credit.supplier_user_id = u.id)
                OR EXISTS (SELECT 1 FROM market_prepaid_accounts prepaid
                           WHERE prepaid.supplier_user_id = u.id)
                OR EXISTS (SELECT 1 FROM market_service_contracts contract
                           WHERE contract.supplier_user_id = u.id)
                OR EXISTS (SELECT 1 FROM market_recurring_contracts recurring
                           WHERE recurring.supplier_user_id = u.id)
             ORDER BY u.created_at, u.id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read claimed Market Provider candidates"))?;
    let unclaimed = tx
        .prepare(
            "SELECT email FROM (
                SELECT lower(trim(owner_email)) AS email FROM host_provider_profiles
                UNION
                SELECT lower(trim(owner_email)) AS email FROM share_market_listings
                UNION
                SELECT lower(trim(supplier_email)) AS email FROM market_counterparties
                UNION
                SELECT lower(trim(supplier_email)) AS email FROM market_credit_accounts
                UNION
                SELECT lower(trim(supplier_email)) AS email FROM market_prepaid_accounts
                UNION
                SELECT lower(trim(supplier_email)) AS email FROM market_service_contracts
                UNION
                SELECT lower(trim(supplier_email)) AS email FROM market_recurring_contracts
             ) WHERE email != '' ORDER BY email",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(map_db("read unclaimed Market Provider candidates"))?;

    let mut reconciled = 0usize;
    let mut claimed_emails = BTreeSet::new();
    for (user_id, email) in claimed {
        claimed_emails.insert(email.to_ascii_lowercase());
        if matches!(
            ensure_market_provider_identity_tx(&tx, Some(&user_id), &email, &now)?,
            MarketProviderIdentityResolution::Resolved(_)
        ) {
            reconciled += 1;
        }
    }
    for email in unclaimed {
        if claimed_emails.contains(&email) {
            continue;
        }
        if matches!(
            ensure_market_provider_identity_tx(&tx, None, &email, &now)?,
            MarketProviderIdentityResolution::Resolved(_)
        ) {
            reconciled += 1;
        }
    }
    tx.commit()
        .map_err(map_db("commit Market Provider identity reconciliation"))?;
    Ok(reconciled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().expect("open test database");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("enable foreign keys");
        crate::schema::apply(&conn).expect("install schema");
        conn
    }

    #[test]
    fn verified_user_claims_email_profile_without_changing_public_id() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO host_provider_profiles
                (provider_id, owner_email, created_at, updated_at)
             VALUES ('email:seller@example.com', 'seller@example.com', '2026-01-01T00:00:00Z',
                     '2026-01-01T00:00:00Z');",
        )
        .expect("seed email-only Host Provider");
        reconcile_conn(&conn, Utc::now()).expect("create unclaimed identity");
        let before: String = conn
            .query_row(
                "SELECT id FROM market_provider_profiles
                 WHERE canonical_email = 'seller@example.com'",
                [],
                |row| row.get(0),
            )
            .expect("read unclaimed identity");

        conn.execute(
            "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
             VALUES ('seller-user', 'seller@example.com', 'active', ?1, ?1)",
            params![Utc::now().to_rfc3339()],
        )
        .expect("seed verified user");
        reconcile_conn(&conn, Utc::now()).expect("claim identity");

        let (after, user_id, claim_state): (String, Option<String>, String) = conn
            .query_row(
                "SELECT id, user_id, claim_state FROM market_provider_profiles
                 WHERE canonical_email = 'seller@example.com'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read claimed identity");
        assert_eq!(after, before);
        assert_eq!(user_id.as_deref(), Some("seller-user"));
        assert_eq!(claim_state, "claimed");
    }

    #[test]
    fn missing_user_stays_unclaimed_until_authoritative_registration() {
        let conn = database();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO host_provider_profiles
                (provider_id, owner_email, created_at, updated_at)
             VALUES ('future-provider', 'future@example.com', ?1, ?1)",
            params![now],
        )
        .expect("seed pre-registration Provider source");
        let tx = conn.transaction().expect("begin identity transaction");
        let profile_id = match ensure_market_provider_identity_tx(
            &tx,
            Some("future-user"),
            "future@example.com",
            &now,
        )
        .expect("create email-only identity")
        {
            MarketProviderIdentityResolution::Resolved(profile_id) => profile_id,
            MarketProviderIdentityResolution::Conflict(_) => panic!("expected public identity"),
        };
        tx.commit().expect("commit email-only identity");

        let (user_id, claim_state): (Option<String>, String) = conn
            .query_row(
                "SELECT user_id, claim_state FROM market_provider_profiles WHERE id = ?1",
                params![profile_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read email-only identity");
        assert_eq!(user_id, None);
        assert_eq!(claim_state, "unclaimed");

        conn.execute(
            "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
             VALUES ('future-user', 'future@example.com', 'active', ?1, ?1)",
            params![now],
        )
        .expect("register authoritative user");
        reconcile_conn(&conn, Utc::now()).expect("claim registered identity");

        let (claimed_id, claimed_user_id, claimed_state): (String, Option<String>, String) = conn
            .query_row(
                "SELECT id, user_id, claim_state FROM market_provider_profiles
                 WHERE canonical_email = 'future@example.com'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read claimed identity");
        assert_eq!(claimed_id, profile_id);
        assert_eq!(claimed_user_id.as_deref(), Some("future-user"));
        assert_eq!(claimed_state, "claimed");
    }

    #[test]
    fn alias_and_profile_drift_is_fenced_instead_of_preferred() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO market_provider_profiles (
                id, canonical_email, display_name, claim_state,
                status, created_at, updated_at
             ) VALUES
                ('mp_email', 'seller@example.com', 'Email identity', 'unclaimed',
                 'active', 'now', 'now'),
                ('mp_alias', 'other@example.com', 'Alias identity', 'unclaimed',
                 'active', 'now', 'now');
             INSERT INTO market_provider_aliases (
                market_provider_id, alias_kind, alias_value, created_at
             ) VALUES ('mp_alias', 'email', 'seller@example.com', 'now');",
        )
        .expect("seed drifted email identity");
        let tx = conn.transaction().expect("begin drift reconciliation");
        assert!(matches!(
            ensure_market_provider_identity_tx(
                &tx,
                None,
                "seller@example.com",
                "2026-09-28T00:00:00Z",
            )
            .expect("fence drifted identity"),
            MarketProviderIdentityResolution::Conflict(_)
        ));
        tx.commit().expect("commit identity fence");

        let conflicted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_provider_profiles
                 WHERE id IN ('mp_email', 'mp_alias') AND status = 'identity_conflict'",
                [],
                |row| row.get(0),
            )
            .expect("count fenced identities");
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_provider_identity_events
                 WHERE event_kind = 'identity_conflict'",
                [],
                |row| row.get(0),
            )
            .expect("count identity conflict events");
        assert_eq!(conflicted, 2);
        assert_eq!(events, 2);
    }

    #[test]
    fn fenced_identity_cannot_bind_new_market_sources() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO market_provider_profiles (
                id, canonical_email, display_name, claim_state,
                status, created_at, updated_at
             ) VALUES (
                'mp_fenced', 'fenced@example.com', 'Fenced', 'unclaimed',
                'identity_conflict', 'now', 'now'
             );
             INSERT INTO market_provider_aliases (
                market_provider_id, alias_kind, alias_value, created_at
             ) VALUES ('mp_fenced', 'email', 'fenced@example.com', 'now');
             INSERT INTO host_provider_profiles (
                provider_id, owner_email, created_at, updated_at
             ) VALUES ('fenced-host', 'fenced@example.com', 'now', 'now');",
        )
        .expect("seed fenced identity");
        let tx = conn.transaction().expect("begin fenced reconciliation");
        assert!(matches!(
            ensure_market_provider_identity_tx(
                &tx,
                None,
                "fenced@example.com",
                "2026-09-28T00:00:00Z",
            )
            .expect("reject fenced identity"),
            MarketProviderIdentityResolution::Conflict(_)
        ));
        tx.commit().expect("commit fenced reconciliation");

        let binding: Option<String> = conn
            .query_row(
                "SELECT market_provider_id FROM host_provider_profiles
                 WHERE provider_id = 'fenced-host'",
                [],
                |row| row.get(0),
            )
            .expect("read fenced Host binding");
        assert_eq!(binding, None);
    }

    #[test]
    fn source_conflict_is_fenced_without_rebinding_the_host() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO market_provider_profiles (
                id, canonical_email, display_name, claim_state,
                status, created_at, updated_at
             ) VALUES
                ('mp_target', 'target@example.com', 'Target', 'unclaimed',
                 'active', 'now', 'now'),
                ('mp_existing', 'existing@example.com', 'Existing', 'unclaimed',
                 'active', 'now', 'now');
             INSERT INTO market_provider_aliases (
                market_provider_id, alias_kind, alias_value, created_at
             ) VALUES
                ('mp_target', 'email', 'target@example.com', 'now'),
                ('mp_existing', 'email', 'existing@example.com', 'now'),
                ('mp_existing', 'host_provider_id', 'stable-host', 'now');
             INSERT INTO host_provider_profiles (
                provider_id, owner_email, created_at, updated_at, market_provider_id
             ) VALUES (
                'stable-host', 'target@example.com', 'now', 'now', 'mp_existing'
             );",
        )
        .expect("seed conflicting Host Provider source");

        let tx = conn
            .transaction()
            .expect("begin source conflict transaction");
        assert!(matches!(
            ensure_market_provider_identity_tx(
                &tx,
                None,
                "target@example.com",
                "2026-09-28T00:00:00Z",
            )
            .expect("fence conflicting Host source"),
            MarketProviderIdentityResolution::Conflict(_)
        ));
        tx.commit().expect("commit source conflict fence");

        let binding: String = conn
            .query_row(
                "SELECT market_provider_id FROM host_provider_profiles
                 WHERE provider_id = 'stable-host'",
                [],
                |row| row.get(0),
            )
            .expect("read preserved Host binding");
        let conflicted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_provider_profiles
                 WHERE id IN ('mp_target', 'mp_existing')
                   AND status = 'identity_conflict'",
                [],
                |row| row.get(0),
            )
            .expect("count fenced source identities");
        assert_eq!(binding, "mp_existing");
        assert_eq!(conflicted, 2);
    }

    #[test]
    fn conflict_audit_survives_business_rollback_and_restores_new_candidate() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO market_provider_profiles (
                id, canonical_email, display_name, claim_state,
                status, created_at, updated_at
             ) VALUES (
                'mp_existing', 'existing@example.com', 'Existing', 'unclaimed',
                'active', 'now', 'now'
             );
             INSERT INTO market_provider_aliases (
                market_provider_id, alias_kind, alias_value, created_at
             ) VALUES (
                'mp_existing', 'host_provider_id', 'proposed-host', 'now'
             );",
        )
        .expect("seed stale Host Provider alias");

        let tx = conn
            .transaction()
            .expect("begin failed business transaction");
        tx.execute(
            "INSERT INTO host_provider_profiles (
                provider_id, owner_email, created_at, updated_at
             ) VALUES ('proposed-host', 'candidate@example.com', 'now', 'now')",
            [],
        )
        .expect("stage proposed Host Provider source");
        let conflict = match ensure_market_provider_identity_tx(
            &tx,
            None,
            "candidate@example.com",
            "2026-09-28T00:00:00Z",
        )
        .expect("detect proposed source conflict")
        {
            MarketProviderIdentityResolution::Conflict(conflict) => conflict,
            MarketProviderIdentityResolution::Resolved(_) => panic!("expected identity conflict"),
        };
        drop(tx);

        persist_market_provider_identity_conflict(&conn, &conflict, "2026-09-28T00:00:00Z")
            .expect("persist conflict outside failed business transaction");

        let staged_hosts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM host_provider_profiles
                 WHERE provider_id = 'proposed-host'",
                [],
                |row| row.get(0),
            )
            .expect("count rolled-back Host sources");
        let profiles: Vec<(String, String)> = conn
            .prepare(
                "SELECT canonical_email, status FROM market_provider_profiles
                 ORDER BY canonical_email",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .expect("read durable conflict profiles");
        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM market_provider_identity_events
                 WHERE event_kind = 'identity_conflict'",
                [],
                |row| row.get(0),
            )
            .expect("count durable conflict events");
        assert_eq!(staged_hosts, 0);
        assert_eq!(
            profiles,
            vec![
                ("candidate@example.com".into(), "identity_conflict".into()),
                ("existing@example.com".into(), "identity_conflict".into()),
            ]
        );
        assert_eq!(events, 2);
    }

    #[test]
    fn share_window_boundaries_reject_clock_regression() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO market_provider_profiles (
                id, canonical_email, display_name, claim_state,
                status, created_at, updated_at
             ) VALUES (
                'mp_window', 'window@example.com', 'Window', 'unclaimed',
                'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'
             );
             INSERT INTO share_market_listings (
                id, share_id, installation_id, owner_user_id, owner_email,
                status, created_at, updated_at, market_provider_id
             ) VALUES (
                'listing-window', 'share-window', 'installation-window', 'owner',
                'window@example.com', 'active', '2026-02-02T00:00:00Z',
                '2026-02-02T00:00:00Z', 'mp_window'
             );
             INSERT INTO market_provider_share_windows (
                id, listing_id, market_provider_id, starts_at, ends_at, created_at
             ) VALUES (
                'window-initial', 'listing-window', 'mp_window',
                '2026-02-02T00:00:00Z', NULL, '2026-02-02T00:00:00Z'
             );",
        )
        .expect("seed open Provider window");

        assert!(matches!(
            close_share_window_tx(&conn, "listing-window", "2026-02-01T00:00:00Z"),
            Err(AppError::Conflict(_))
        ));
        assert!(
            close_share_window_tx(&conn, "listing-window", "2026-02-03T00:00:00Z")
                .expect("close Provider window")
        );
        assert!(matches!(
            open_share_window_tx(&conn, "listing-window", "2026-02-02T12:00:00Z"),
            Err(AppError::Conflict(_))
        ));
        assert!(
            open_share_window_tx(&conn, "listing-window", "2026-02-04T00:00:00Z")
                .expect("reopen Provider window")
        );
    }

    #[test]
    fn reconciliation_is_idempotent_and_binds_both_markets() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
             VALUES ('provider-user', 'provider@example.com', 'active', 'now', 'now');
             INSERT INTO host_provider_profiles
                (provider_id, owner_email, created_at, updated_at)
             VALUES ('legacy-provider', 'provider@example.com',
                     '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z');
             INSERT INTO share_market_listings
                (id, share_id, installation_id, owner_user_id, owner_email,
                 status, created_at, updated_at)
             VALUES ('listing', 'share', 'installation', 'provider-user',
                     'provider@example.com', 'active',
                     '2026-01-03T00:00:00Z', '2026-01-04T00:00:00Z');",
        )
        .expect("seed both markets");
        reconcile_conn(&conn, Utc::now()).expect("first reconciliation");
        reconcile_conn(&conn, Utc::now()).expect("second reconciliation");

        let profile_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM market_provider_profiles", [], |row| {
                row.get(0)
            })
            .expect("count profiles");
        let distinct_bindings: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT market_provider_id) FROM (
                    SELECT market_provider_id FROM host_provider_profiles
                    UNION ALL
                    SELECT market_provider_id FROM share_market_listings
                 )",
                [],
                |row| row.get(0),
            )
            .expect("count bindings");
        assert_eq!(profile_count, 1);
        assert_eq!(distinct_bindings, 1);
        let (listing_updated_at, window_start, window_end): (String, String, Option<String>) = conn
            .query_row(
                "SELECT listing.updated_at, window.starts_at, window.ends_at
                 FROM share_market_listings listing
                 JOIN market_provider_share_windows window
                   ON window.listing_id = listing.id
                 WHERE listing.id = 'listing'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read reconciled Share Provider window");
        assert_eq!(listing_updated_at, "2026-01-04T00:00:00Z");
        assert_eq!(window_start, "2026-01-03T00:00:00Z");
        assert_eq!(window_end, None);
    }

    #[test]
    fn reconciliation_keeps_contract_and_funding_only_suppliers() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
             VALUES
                ('contract-supplier', 'contract@example.com', 'active', 'now', 'now'),
                ('funding-supplier', 'funding@example.com', 'active', 'now', 'now');
             INSERT INTO market_service_contracts (
                id, account_id, product_kind, product_ref, service_ref, service_label,
                buyer_user_id, buyer_email, supplier_user_id, supplier_email, currency,
                daily_rate_minor, offer_revision, status, trial_seconds_remaining,
                health_state, desired_control_state, applied_control_state,
                last_evaluated_at, activated_at, created_at, updated_at
             ) VALUES (
                'contract-only', 'credit-account', 'share', 'share-only', 'service-only',
                'Contract only', 'buyer', 'buyer@example.com', 'contract-supplier',
                'contract@example.com', 'USD', 100, 1, 'active', 0, 'healthy',
                'active', 'active', 'now', 'now', 'now', 'now'
             );
             INSERT INTO market_prepaid_accounts (
                id, buyer_user_id, buyer_email, supplier_user_id, supplier_email,
                currency, status, posted_balance_units, held_balance_units,
                version, created_at, updated_at
             ) VALUES (
                'funding-only', 'buyer', 'buyer@example.com', 'funding-supplier',
                'funding@example.com', 'USD', 'open', 1000, 0, 1, 'now', 'now'
             );",
        )
        .expect("seed non-supply Provider relationships");

        reconcile_conn(&conn, Utc::now()).expect("reconcile non-supply Providers");
        let before = conn
            .prepare(
                "SELECT canonical_email, id FROM market_provider_profiles
                 ORDER BY canonical_email",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .expect("read non-supply Provider identities");
        assert_eq!(before.len(), 2);
        assert!(before.iter().all(|(_, id)| id.starts_with(PROFILE_PREFIX)));

        reconcile_conn(&conn, Utc::now()).expect("repeat non-supply reconciliation");
        let after = conn
            .prepare(
                "SELECT canonical_email, id FROM market_provider_profiles
                 ORDER BY canonical_email",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()
            })
            .expect("read stable non-supply identities");
        assert_eq!(after, before);
    }
}
