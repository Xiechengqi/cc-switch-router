use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use crate::namespace::{
    PublicHostKind, normalize_client_subdomain, normalize_market_slug, parse_share_label,
};

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
    Database(#[from] rusqlite::Error),
}

pub fn init_schema(conn: &Connection) -> Result<(), PublicHostCatalogError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS public_hosts (
            label TEXT PRIMARY KEY COLLATE NOCASE,
            route_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN ('client', 'share', 'market')),
            subject_id TEXT NOT NULL,
            installation_id TEXT,
            target_lane_id TEXT NOT NULL,
            lifecycle TEXT NOT NULL CHECK(lifecycle IN ('active', 'disabled', 'tombstoned')),
            revision INTEGER NOT NULL CHECK(revision > 0),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_public_hosts_live_subject
            ON public_hosts(kind, subject_id)
            WHERE lifecycle != 'tombstoned';
        CREATE UNIQUE INDEX IF NOT EXISTS idx_public_hosts_live_route
            ON public_hosts(route_id)
            WHERE lifecycle != 'tombstoned';
        CREATE INDEX IF NOT EXISTS idx_public_hosts_target_lane
            ON public_hosts(target_lane_id, lifecycle);",
    )?;
    Ok(())
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
            "label {label} is already assigned to {} {}",
            kind_str(existing.kind),
            existing.subject_id
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
    if get_by_label(conn, &new_label)?.is_some() {
        return Err(PublicHostCatalogError::Conflict(format!(
            "label {new_label} is already reserved"
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
        PublicHostKind::Market => {
            normalize_market_slug(&label).map_err(PublicHostCatalogError::Invalid)?;
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

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<PublicHostRecord> {
    let kind = match row.get::<_, String>(2)?.as_str() {
        "client" => PublicHostKind::Client,
        "share" => PublicHostKind::Share,
        "market" => PublicHostKind::Market,
        value => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(PublicHostCatalogError::Corrupt(format!(
                    "unknown public host kind {value}"
                ))),
            ));
        }
    };
    let lifecycle_raw = row.get::<_, String>(6)?;
    let lifecycle = PublicHostLifecycle::parse(&lifecycle_raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(error))
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

fn parse_time(value: String, column: usize) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn kind_str(kind: PublicHostKind) -> &'static str {
    match kind {
        PublicHostKind::Client => "client",
        PublicHostKind::Share => "share",
        PublicHostKind::Market => "market",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::build_share_label;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
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
    fn disabled_hosts_remain_known_and_can_be_reenabled() {
        let conn = database();
        let label = "alpha-main".to_string();
        claim(&conn, client_claim(&label)).unwrap();
        assert!(set_lifecycle(&conn, &label, PublicHostLifecycle::Disabled).unwrap());
        assert_eq!(list_non_tombstoned(&conn).unwrap().len(), 1);
        assert!(set_lifecycle(&conn, &label, PublicHostLifecycle::Active).unwrap());
    }
}
