use crate::db::{Connection, OptionalExtension, params};
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::namespace::{PublicHostKind, normalize_client_subdomain, parse_share_label};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicHostLifecycle {
    Active,
    Disabled,
    Tombstoned,
}

impl PublicHostLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Tombstoned => "tombstoned",
        }
    }

    fn parse(value: &str) -> Result<Self, PublicHostCatalogError> {
        match value {
            "active" => Ok(Self::Active),
            "disabled" => Ok(Self::Disabled),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(PublicHostCatalogError::Corrupt(format!(
                "unknown public host lifecycle {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicHostRecord {
    pub label: String,
    pub route_id: String,
    pub kind: PublicHostKind,
    pub subject_id: String,
    pub installation_id: Option<String>,
    pub target_lane_id: String,
    pub lifecycle: PublicHostLifecycle,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPublicHost<'a> {
    pub label: &'a str,
    pub route_id: &'a str,
    pub kind: PublicHostKind,
    pub subject_id: &'a str,
    pub installation_id: Option<&'a str>,
    pub target_lane_id: &'a str,
}

#[derive(Debug, Error)]
pub enum PublicHostCatalogError {
    #[error("invalid public host: {0}")]
    Invalid(&'static str),
    #[error("public host conflict: {0}")]
    Conflict(String),
    #[error("public host catalog is corrupt: {0}")]
    Corrupt(String),
    #[error("public host catalog database error: {0}")]
    Database(#[from] crate::db::Error),
}

pub fn claim(
    conn: &Connection,
    input: NewPublicHost<'_>,
) -> Result<PublicHostRecord, PublicHostCatalogError> {
    validate_claim(&input)?;
    let label = input.label.trim().to_ascii_lowercase();
    if let Some(existing) = get_by_label(conn, &label)? {
        if same_claim(&existing, &input) && existing.lifecycle != PublicHostLifecycle::Tombstoned {
            return Ok(existing);
        }
        return Err(PublicHostCatalogError::Conflict(format!(
            "label {label} is already assigned to {} {} with lifecycle {}",
            kind_str(existing.kind),
            existing.subject_id,
            existing.lifecycle.as_str(),
        )));
    }
    if let Some(existing) = get_live_by_subject(conn, input.kind, input.subject_id)? {
        return Err(PublicHostCatalogError::Conflict(format!(
            "{} {} already owns label {}",
            kind_str(input.kind),
            input.subject_id,
            existing.label
        )));
    }
    let now = Utc::now();
    conn.execute(
        "INSERT INTO public_hosts (
            label, route_id, kind, subject_id, installation_id, target_lane_id,
            lifecycle, revision, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 1, ?7, ?7)",
        params![
            label,
            input.route_id,
            kind_str(input.kind),
            input.subject_id,
            input.installation_id,
            input.target_lane_id,
            now.to_rfc3339(),
        ],
    )?;
    get_by_label(conn, &label)?.ok_or_else(|| {
        PublicHostCatalogError::Corrupt("inserted public host cannot be read back".into())
    })
}

/// Bind a tombstoned Client label to a new installation.
///
/// Share labels stay permanently retired. Client Market release also tombstones
/// the Client label; the original owner may reclaim that exact label after the
/// old installation has been purged. Takeover-retired names stay blocked because
/// their subject installation is still live.
pub(crate) fn reclaim_tombstoned_client_label(
    conn: &Connection,
    input: NewPublicHost<'_>,
) -> Result<PublicHostRecord, PublicHostCatalogError> {
    validate_claim(&input)?;
    if input.kind != PublicHostKind::Client {
        return Err(PublicHostCatalogError::Invalid(
            "only Client labels can be reclaimed from tombstone",
        ));
    }
    let label = input.label.trim().to_ascii_lowercase();
    let existing = get_by_label(conn, &label)?.ok_or_else(|| {
        PublicHostCatalogError::Conflict(format!(
            "label {label} has no Client tombstone to reclaim"
        ))
    })?;
    if existing.kind != PublicHostKind::Client
        || existing.lifecycle != PublicHostLifecycle::Tombstoned
    {
        return Err(PublicHostCatalogError::Conflict(format!(
            "label {label} is already assigned to {} {} with lifecycle {}",
            kind_str(existing.kind),
            existing.subject_id,
            existing.lifecycle.as_str(),
        )));
    }
    if let Some(live) = get_live_by_subject(conn, input.kind, input.subject_id)? {
        return Err(PublicHostCatalogError::Conflict(format!(
            "{} {} already owns label {}",
            kind_str(input.kind),
            input.subject_id,
            live.label
        )));
    }

    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE public_hosts
         SET route_id = ?2, kind = ?3, subject_id = ?4, installation_id = ?5,
             target_lane_id = ?6, lifecycle = 'active', revision = revision + 1,
             updated_at = ?7
         WHERE label = ?1 AND kind = 'client' AND lifecycle = 'tombstoned'",
        params![
            label,
            input.route_id,
            kind_str(input.kind),
            input.subject_id,
            input.installation_id,
            input.target_lane_id,
            now,
        ],
    )?;
    if changed != 1 {
        return Err(PublicHostCatalogError::Conflict(format!(
            "label {label} could not be reclaimed from tombstone"
        )));
    }
    get_by_label(conn, &label)?.ok_or_else(|| {
        PublicHostCatalogError::Corrupt("reclaimed Client host cannot be read back".into())
    })
}

pub(crate) fn reconcile_share_claim_in_transaction(
    conn: &Connection,
    input: NewPublicHost<'_>,
) -> Result<PublicHostRecord, PublicHostCatalogError> {
    validate_claim(&input)?;
    if input.kind != PublicHostKind::Share {
        return Err(PublicHostCatalogError::Invalid(
            "share claim reconciliation only accepts Share hosts",
        ));
    }

    let label = input.label.trim().to_ascii_lowercase();
    if let Some(existing) = get_by_label(conn, &label)? {
        if !same_claim(&existing, &input) {
            return Err(PublicHostCatalogError::Conflict(format!(
                "label {label} is already assigned to {} {} with lifecycle {}",
                kind_str(existing.kind),
                existing.subject_id,
                existing.lifecycle.as_str(),
            )));
        }
        if existing.lifecycle != PublicHostLifecycle::Tombstoned {
            return Ok(existing);
        }
        if let Some(live) = get_live_by_subject(conn, input.kind, input.subject_id)? {
            return Err(PublicHostCatalogError::Conflict(format!(
                "{} {} already owns label {} with lifecycle {}",
                kind_str(input.kind),
                input.subject_id,
                live.label,
                live.lifecycle.as_str(),
            )));
        }
        let latest =
            get_latest_by_subject(conn, input.kind, input.subject_id)?.ok_or_else(|| {
                PublicHostCatalogError::Corrupt(
                    "tombstoned Share host is missing from its subject history".into(),
                )
            })?;
        if latest.label != label {
            return Err(PublicHostCatalogError::Conflict(format!(
                "{} {} has a newer reserved label {} with lifecycle {}",
                kind_str(input.kind),
                input.subject_id,
                latest.label,
                latest.lifecycle.as_str(),
            )));
        }

        let changed = conn.execute(
            "UPDATE public_hosts
             SET lifecycle = 'active', revision = revision + 1, updated_at = ?2
             WHERE label = ?1 AND lifecycle = 'tombstoned'",
            params![label, Utc::now().to_rfc3339()],
        )?;
        if changed != 1 {
            return Err(PublicHostCatalogError::Corrupt(
                "tombstoned Share host could not be restored".into(),
            ));
        }
        return get_by_label(conn, &label)?.ok_or_else(|| {
            PublicHostCatalogError::Corrupt("restored Share host cannot be read back".into())
        });
    }

    if let Some(existing) = get_live_by_subject(conn, input.kind, input.subject_id)? {
        if !same_claim(&existing, &input) {
            return Err(PublicHostCatalogError::Conflict(format!(
                "{} {} already owns label {} with lifecycle {} and a different identity",
                kind_str(input.kind),
                input.subject_id,
                existing.label,
                existing.lifecycle.as_str(),
            )));
        }
        return replace_claim_in_transaction(conn, &existing.label, input);
    }

    if let Some(existing) = get_latest_by_subject(conn, input.kind, input.subject_id)?
        && !same_claim(&existing, &input)
    {
        return Err(PublicHostCatalogError::Conflict(format!(
            "{} {} was previously assigned to label {} with lifecycle {} and a different identity",
            kind_str(input.kind),
            input.subject_id,
            existing.label,
            existing.lifecycle.as_str(),
        )));
    }

    claim(conn, input)
}

#[cfg(test)]
pub fn replace_claim(
    conn: &mut Connection,
    old_label: &str,
    input: NewPublicHost<'_>,
) -> Result<PublicHostRecord, PublicHostCatalogError> {
    let transaction = conn.transaction()?;
    let record = replace_claim_in_transaction(&transaction, old_label, input)?;
    transaction.commit()?;
    Ok(record)
}

pub fn replace_claim_in_transaction(
    conn: &Connection,
    old_label: &str,
    input: NewPublicHost<'_>,
) -> Result<PublicHostRecord, PublicHostCatalogError> {
    validate_claim(&input)?;
    let old_label = old_label.trim().to_ascii_lowercase();
    let new_label = input.label.trim().to_ascii_lowercase();
    if old_label == new_label {
        return claim(conn, input);
    }
    let existing = get_by_label(conn, &old_label)?
        .ok_or_else(|| PublicHostCatalogError::Conflict("old host claim was not found".into()))?;
    if existing.lifecycle == PublicHostLifecycle::Tombstoned
        || existing.kind != input.kind
        || existing.subject_id != input.subject_id
    {
        return Err(PublicHostCatalogError::Conflict(
            "old host claim does not belong to the requested subject".into(),
        ));
    }
    if let Some(existing) = get_by_label(conn, &new_label)? {
        return Err(PublicHostCatalogError::Conflict(format!(
            "label {new_label} is already reserved with lifecycle {}",
            existing.lifecycle.as_str(),
        )));
    }
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE public_hosts
         SET lifecycle = 'tombstoned', revision = revision + 1, updated_at = ?2
         WHERE label = ?1 AND lifecycle != 'tombstoned'",
        params![old_label, now],
    )?;
    claim(conn, input)
}

pub(crate) fn takeover_rebind(
    conn: &Connection,
    input: NewPublicHost<'_>,
    previous_installation_id: &str,
) -> Result<PublicHostRecord, PublicHostCatalogError> {
    validate_claim(&input)?;
    let label = input.label.trim().to_ascii_lowercase();
    if let Some(subject_host) = get_live_by_subject(conn, input.kind, input.subject_id)?
        && subject_host.label != label
    {
        return Err(PublicHostCatalogError::Conflict(format!(
            "{} {} still owns label {}",
            kind_str(input.kind),
            input.subject_id,
            subject_host.label
        )));
    }

    let Some(existing) = get_by_label(conn, &label)? else {
        return claim(conn, input);
    };
    if same_claim(&existing, &input) {
        set_lifecycle(conn, &label, PublicHostLifecycle::Active)?;
        return get_by_label(conn, &label)?.ok_or_else(|| {
            PublicHostCatalogError::Corrupt("rebound public host cannot be read back".into())
        });
    }
    if existing.installation_id.as_deref() != Some(previous_installation_id) {
        return Err(PublicHostCatalogError::Conflict(format!(
            "label {label} is not owned by the takeover source installation"
        )));
    }

    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE public_hosts
         SET route_id = ?2, kind = ?3, subject_id = ?4, installation_id = ?5,
             target_lane_id = ?6, lifecycle = 'active', revision = revision + 1,
             updated_at = ?7
         WHERE label = ?1",
        params![
            label,
            input.route_id,
            kind_str(input.kind),
            input.subject_id,
            input.installation_id,
            input.target_lane_id,
            now,
        ],
    )?;
    get_by_label(conn, &label)?.ok_or_else(|| {
        PublicHostCatalogError::Corrupt("rebound public host cannot be read back".into())
    })
}

pub fn set_lifecycle(
    conn: &Connection,
    label: &str,
    lifecycle: PublicHostLifecycle,
) -> Result<bool, PublicHostCatalogError> {
    let label = label.trim().to_ascii_lowercase();
    if lifecycle == PublicHostLifecycle::Active {
        let existing = get_by_label(conn, &label)?
            .ok_or_else(|| PublicHostCatalogError::Conflict("host claim was not found".into()))?;
        if existing.lifecycle == PublicHostLifecycle::Tombstoned {
            return Err(PublicHostCatalogError::Conflict(
                "tombstoned host labels cannot be reactivated".into(),
            ));
        }
    }
    let changed = conn.execute(
        "UPDATE public_hosts
         SET lifecycle = ?2, revision = revision + 1, updated_at = ?3
         WHERE label = ?1 AND lifecycle != ?2",
        params![label, lifecycle.as_str(), Utc::now().to_rfc3339()],
    )?;
    Ok(changed == 1)
}

pub fn tombstone_subject(
    conn: &Connection,
    kind: PublicHostKind,
    subject_id: &str,
) -> Result<bool, PublicHostCatalogError> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE public_hosts
         SET lifecycle = 'tombstoned', revision = revision + 1, updated_at = ?3
         WHERE kind = ?1 AND subject_id = ?2 AND lifecycle != 'tombstoned'",
        params![kind_str(kind), subject_id, now],
    )?;
    Ok(changed > 0)
}

pub fn get_by_label(
    conn: &Connection,
    label: &str,
) -> Result<Option<PublicHostRecord>, PublicHostCatalogError> {
    conn.query_row(
        "SELECT label, route_id, kind, subject_id, installation_id, target_lane_id,
                lifecycle, revision, created_at, updated_at
         FROM public_hosts WHERE label = ?1",
        params![label.trim().to_ascii_lowercase()],
        map_record,
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
pub fn list_non_tombstoned(
    conn: &Connection,
) -> Result<Vec<PublicHostRecord>, PublicHostCatalogError> {
    let mut statement = conn.prepare(
        "SELECT label, route_id, kind, subject_id, installation_id, target_lane_id,
                lifecycle, revision, created_at, updated_at
         FROM public_hosts WHERE lifecycle != 'tombstoned' ORDER BY label",
    )?;
    let rows = statement.query_map([], map_record)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn get_live_by_subject(
    conn: &Connection,
    kind: PublicHostKind,
    subject_id: &str,
) -> Result<Option<PublicHostRecord>, PublicHostCatalogError> {
    conn.query_row(
        "SELECT label, route_id, kind, subject_id, installation_id, target_lane_id,
                lifecycle, revision, created_at, updated_at
         FROM public_hosts
         WHERE kind = ?1 AND subject_id = ?2 AND lifecycle != 'tombstoned'",
        params![kind_str(kind), subject_id],
        map_record,
    )
    .optional()
    .map_err(Into::into)
}

fn get_latest_by_subject(
    conn: &Connection,
    kind: PublicHostKind,
    subject_id: &str,
) -> Result<Option<PublicHostRecord>, PublicHostCatalogError> {
    conn.query_row(
        "SELECT label, route_id, kind, subject_id, installation_id, target_lane_id,
                lifecycle, revision, created_at, updated_at
         FROM public_hosts
         WHERE kind = ?1 AND subject_id = ?2
         ORDER BY created_at DESC, rowid DESC
         LIMIT 1",
        params![kind_str(kind), subject_id],
        map_record,
    )
    .optional()
    .map_err(Into::into)
}

fn validate_claim(input: &NewPublicHost<'_>) -> Result<(), PublicHostCatalogError> {
    if input.route_id.trim().is_empty()
        || input.subject_id.trim().is_empty()
        || input.target_lane_id.trim().is_empty()
        || input
            .installation_id
            .is_some_and(|value| value.trim().is_empty())
    {
        return Err(PublicHostCatalogError::Invalid(
            "route, subject, installation, and lane identifiers cannot be empty",
        ));
    }
    let label = input.label.trim().to_ascii_lowercase();
    match input.kind {
        PublicHostKind::Client => {
            normalize_client_subdomain(&label).map_err(PublicHostCatalogError::Invalid)?;
        }
        PublicHostKind::Share => {
            parse_share_label(&label).map_err(PublicHostCatalogError::Invalid)?;
        }
    }
    Ok(())
}

fn same_claim(existing: &PublicHostRecord, input: &NewPublicHost<'_>) -> bool {
    existing.route_id == input.route_id
        && existing.kind == input.kind
        && existing.subject_id == input.subject_id
        && existing.installation_id.as_deref() == input.installation_id
        && existing.target_lane_id == input.target_lane_id
}

fn map_record(row: &crate::db::Row<'_>) -> crate::db::Result<PublicHostRecord> {
    let kind = match row.get::<_, String>(2)?.as_str() {
        "client" => PublicHostKind::Client,
        "share" => PublicHostKind::Share,
        value => {
            return Err(crate::db::Error::FromSqlConversionFailure(
                2,
                crate::db::types::Type::Text,
                Box::new(PublicHostCatalogError::Corrupt(format!(
                    "unknown public host kind {value}"
                ))),
            ));
        }
    };
    let lifecycle_raw = row.get::<_, String>(6)?;
    let lifecycle = PublicHostLifecycle::parse(&lifecycle_raw).map_err(|error| {
        crate::db::Error::FromSqlConversionFailure(6, crate::db::types::Type::Text, Box::new(error))
    })?;
    Ok(PublicHostRecord {
        label: row.get(0)?,
        route_id: row.get(1)?,
        kind,
        subject_id: row.get(3)?,
        installation_id: row.get(4)?,
        target_lane_id: row.get(5)?,
        lifecycle,
        revision: row.get(7)?,
        created_at: parse_time(row.get::<_, String>(8)?, 8)?,
        updated_at: parse_time(row.get::<_, String>(9)?, 9)?,
    })
}

fn parse_time(value: String, column: usize) -> crate::db::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            crate::db::Error::FromSqlConversionFailure(
                column,
                crate::db::types::Type::Text,
                Box::new(error),
            )
        })
}

fn kind_str(kind: PublicHostKind) -> &'static str {
    match kind {
        PublicHostKind::Client => "client",
        PublicHostKind::Share => "share",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::build_share_label;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::apply(&conn).unwrap();
        conn
    }

    fn client_claim<'a>(label: &'a str) -> NewPublicHost<'a> {
        NewPublicHost {
            label,
            route_id: "client:installation-1",
            kind: PublicHostKind::Client,
            subject_id: "installation-1",
            installation_id: Some("installation-1"),
            target_lane_id: "installation-1:client-web",
        }
    }

    fn share_claim<'a>(label: &'a str) -> NewPublicHost<'a> {
        NewPublicHost {
            label,
            route_id: "share:share-1",
            kind: PublicHostKind::Share,
            subject_id: "share-1",
            installation_id: Some("installation-1"),
            target_lane_id: "installation-1",
        }
    }

    #[test]
    fn exact_label_claim_is_idempotent_but_conflicts_are_rejected() {
        let conn = database();
        let label = "alpha-main".to_string();
        let first = claim(&conn, client_claim(&label)).unwrap();
        let second = claim(&conn, client_claim(&label)).unwrap();
        assert_eq!(first, second);

        let conflict = NewPublicHost {
            subject_id: "installation-2",
            route_id: "client:installation-2",
            installation_id: Some("installation-2"),
            target_lane_id: "installation-2:client-web",
            ..client_claim(&label)
        };
        assert!(matches!(
            claim(&conn, conflict),
            Err(PublicHostCatalogError::Conflict(_))
        ));
    }

    #[test]
    fn share_claim_targets_the_clients_namespace_lane() {
        let conn = database();
        let client = "alpha-main".to_string();
        let label = build_share_label("codexx", &client).unwrap();
        let share = claim(
            &conn,
            NewPublicHost {
                label: &label,
                route_id: "share:share-1",
                kind: PublicHostKind::Share,
                subject_id: "share-1",
                installation_id: Some("installation-1"),
                target_lane_id: "installation-1:namespace-data",
            },
        )
        .unwrap();
        assert_eq!(share.kind, PublicHostKind::Share);
        assert_eq!(share.target_lane_id, "installation-1:namespace-data");
    }

    #[test]
    fn exact_tombstoned_share_claim_can_be_restored() {
        let conn = database();
        let label = build_share_label("codexx", "alpha-main").unwrap();
        let original = claim(&conn, share_claim(&label)).unwrap();
        assert!(tombstone_subject(&conn, PublicHostKind::Share, "share-1").unwrap());

        let restored = reconcile_share_claim_in_transaction(&conn, share_claim(&label)).unwrap();

        assert_eq!(restored.lifecycle, PublicHostLifecycle::Active);
        assert_eq!(restored.revision, original.revision + 2);
    }

    #[test]
    fn renamed_share_tombstone_cannot_be_restored() {
        let conn = database();
        let old_label = build_share_label("codexx", "alpha-main").unwrap();
        let new_label = build_share_label("claudex", "alpha-main").unwrap();
        claim(&conn, share_claim(&old_label)).unwrap();
        reconcile_share_claim_in_transaction(&conn, share_claim(&new_label)).unwrap();

        let error =
            reconcile_share_claim_in_transaction(&conn, share_claim(&old_label)).unwrap_err();

        assert!(matches!(error, PublicHostCatalogError::Conflict(_)));
        assert_eq!(
            get_by_label(&conn, &old_label).unwrap().unwrap().lifecycle,
            PublicHostLifecycle::Tombstoned
        );
        assert_eq!(
            get_by_label(&conn, &new_label).unwrap().unwrap().lifecycle,
            PublicHostLifecycle::Active
        );
    }

    #[test]
    fn tombstoned_share_claim_rejects_a_different_installation() {
        let conn = database();
        let label = build_share_label("codexx", "alpha-main").unwrap();
        claim(&conn, share_claim(&label)).unwrap();
        assert!(tombstone_subject(&conn, PublicHostKind::Share, "share-1").unwrap());
        let different_installation = NewPublicHost {
            installation_id: Some("installation-2"),
            target_lane_id: "installation-2",
            ..share_claim(&label)
        };

        let error =
            reconcile_share_claim_in_transaction(&conn, different_installation).unwrap_err();

        assert!(matches!(error, PublicHostCatalogError::Conflict(_)));
        assert_eq!(
            get_by_label(&conn, &label).unwrap().unwrap().lifecycle,
            PublicHostLifecycle::Tombstoned
        );
    }

    #[test]
    fn older_renamed_share_tombstone_stays_retired_after_current_label_is_tombstoned() {
        let conn = database();
        let old_label = build_share_label("codexx", "alpha-main").unwrap();
        let new_label = build_share_label("claudex", "alpha-main").unwrap();
        claim(&conn, share_claim(&old_label)).unwrap();
        reconcile_share_claim_in_transaction(&conn, share_claim(&new_label)).unwrap();
        assert!(tombstone_subject(&conn, PublicHostKind::Share, "share-1").unwrap());

        let error =
            reconcile_share_claim_in_transaction(&conn, share_claim(&old_label)).unwrap_err();
        let restored =
            reconcile_share_claim_in_transaction(&conn, share_claim(&new_label)).unwrap();

        assert!(matches!(error, PublicHostCatalogError::Conflict(_)));
        assert_eq!(restored.lifecycle, PublicHostLifecycle::Active);
        assert_eq!(
            get_by_label(&conn, &old_label).unwrap().unwrap().lifecycle,
            PublicHostLifecycle::Tombstoned
        );
    }

    #[test]
    fn tombstoned_share_subject_rejects_new_label_from_a_different_installation() {
        let conn = database();
        let old_label = build_share_label("codexx", "alpha-main").unwrap();
        let new_label = build_share_label("claudex", "alpha-main").unwrap();
        claim(&conn, share_claim(&old_label)).unwrap();
        assert!(tombstone_subject(&conn, PublicHostKind::Share, "share-1").unwrap());
        let different_installation = NewPublicHost {
            label: &new_label,
            installation_id: Some("installation-2"),
            target_lane_id: "installation-2",
            ..share_claim(&old_label)
        };

        let error =
            reconcile_share_claim_in_transaction(&conn, different_installation).unwrap_err();

        assert!(matches!(error, PublicHostCatalogError::Conflict(_)));
        assert!(get_by_label(&conn, &new_label).unwrap().is_none());
    }

    #[test]
    fn rename_tombstones_the_old_label_and_never_reuses_it() {
        let mut conn = database();
        let old = "alpha-main".to_string();
        let new = "bravo-main".to_string();
        claim(&conn, client_claim(&old)).unwrap();
        replace_claim(&mut conn, &old, client_claim(&new)).unwrap();
        assert_eq!(
            get_by_label(&conn, &old).unwrap().unwrap().lifecycle,
            PublicHostLifecycle::Tombstoned
        );
        assert_eq!(
            get_by_label(&conn, &new).unwrap().unwrap().lifecycle,
            PublicHostLifecycle::Active
        );
        assert!(claim(&conn, client_claim(&old)).is_err());
    }

    #[test]
    fn tombstoned_client_label_can_be_reclaimed_by_a_new_installation() {
        let conn = database();
        let label = "alpha-main".to_string();
        claim(&conn, client_claim(&label)).unwrap();
        assert!(tombstone_subject(&conn, PublicHostKind::Client, "installation-1").unwrap());
        assert_eq!(
            get_by_label(&conn, &label).unwrap().unwrap().lifecycle,
            PublicHostLifecycle::Tombstoned
        );

        let reclaimed = reclaim_tombstoned_client_label(
            &conn,
            NewPublicHost {
                label: &label,
                route_id: "client:installation-2",
                kind: PublicHostKind::Client,
                subject_id: "installation-2",
                installation_id: Some("installation-2"),
                target_lane_id: "installation-2",
            },
        )
        .unwrap();
        assert_eq!(reclaimed.subject_id, "installation-2");
        assert_eq!(reclaimed.lifecycle, PublicHostLifecycle::Active);
        assert_eq!(
            get_by_label(&conn, &label)
                .unwrap()
                .unwrap()
                .installation_id
                .as_deref(),
            Some("installation-2")
        );
    }

    #[test]
    fn live_client_label_cannot_be_reclaimed() {
        let conn = database();
        let label = "alpha-main".to_string();
        claim(&conn, client_claim(&label)).unwrap();
        let error = reclaim_tombstoned_client_label(
            &conn,
            NewPublicHost {
                label: &label,
                route_id: "client:installation-2",
                kind: PublicHostKind::Client,
                subject_id: "installation-2",
                installation_id: Some("installation-2"),
                target_lane_id: "installation-2",
            },
        )
        .unwrap_err();
        assert!(matches!(error, PublicHostCatalogError::Conflict(_)));
    }

    #[test]
    fn disabled_hosts_remain_known_and_can_be_reenabled() {
        let conn = database();
        let label = "alpha-main".to_string();
        claim(&conn, client_claim(&label)).unwrap();
        assert!(set_lifecycle(&conn, &label, PublicHostLifecycle::Disabled).unwrap());
        assert_eq!(list_non_tombstoned(&conn).unwrap().len(), 1);
        assert!(set_lifecycle(&conn, &label, PublicHostLifecycle::Active).unwrap());
    }
}
