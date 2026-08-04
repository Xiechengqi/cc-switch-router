use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::db::{Connection, TransactionBehavior, params};
use crate::error::AppError;

const BASELINE_VERSION: i64 = 1;
const BASELINE_SQL: &str = include_str!("../schema/0001_baseline.sql");

pub fn apply(conn: &Connection) -> Result<(), AppError> {
    let checksum = baseline_checksum();
    let has_migration_table = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| AppError::Internal(format!("inspect database schema failed: {error}")))?
        != 0;

    if !has_migration_table {
        reject_nonempty_unmanaged_database(conn)?;
        install_baseline(conn, &checksum)?;
        return Ok(());
    }

    let mut statement = conn
        .prepare("SELECT version, checksum FROM schema_migrations ORDER BY version")
        .map_err(|error| {
            AppError::Internal(format!("prepare schema migration check failed: {error}"))
        })?;
    let migrations = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| AppError::Internal(format!("query schema migrations failed: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            AppError::Internal(format!("read schema migration row failed: {error}"))
        })?;

    match migrations.as_slice() {
        [(BASELINE_VERSION, recorded_checksum)] if recorded_checksum == &checksum => Ok(()),
        [(BASELINE_VERSION, recorded_checksum)] => Err(AppError::Internal(format!(
            "database baseline checksum mismatch: expected {checksum}, found {recorded_checksum}"
        ))),
        [] => Err(AppError::Internal(
            "schema_migrations is empty; partial or legacy databases are unsupported".into(),
        )),
        _ => Err(AppError::Internal(format!(
            "unsupported database migration history: expected only version {BASELINE_VERSION}"
        ))),
    }
}

fn reject_nonempty_unmanaged_database(conn: &Connection) -> Result<(), AppError> {
    let table_count = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| AppError::Internal(format!("inspect existing tables failed: {error}")))?;
    if table_count == 0 {
        return Ok(());
    }
    Err(AppError::Internal(
        "database is not empty and has no migration metadata; legacy database migration is unsupported"
            .into(),
    ))
}

fn install_baseline(conn: &Connection, checksum: &str) -> Result<(), AppError> {
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| AppError::Internal(format!("begin baseline migration failed: {error}")))?;
    transaction
        .execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                checksum TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );",
        )
        .map_err(|error| {
            AppError::Internal(format!("create schema migration table failed: {error}"))
        })?;
    transaction
        .execute_batch(BASELINE_SQL)
        .map_err(|error| AppError::Internal(format!("apply database baseline failed: {error}")))?;
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, checksum, applied_at) VALUES (?1, ?2, ?3)",
            params![BASELINE_VERSION, checksum, Utc::now().to_rfc3339()],
        )
        .map_err(|error| AppError::Internal(format!("record database baseline failed: {error}")))?;
    transaction
        .commit()
        .map_err(|error| AppError::Internal(format!("commit database baseline failed: {error}")))
}

fn baseline_checksum() -> String {
    hex::encode(Sha256::digest(BASELINE_SQL.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_connection() -> Connection {
        Connection::open_in_memory().expect("open test database")
    }

    #[test]
    fn installs_and_reopens_fresh_baseline() {
        let conn = memory_connection();
        apply(&conn).expect("install baseline");
        apply(&conn).expect("reopen matching baseline");

        let table_count = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count baseline tables");
        assert_eq!(table_count, 97);
        let checksum = conn
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("read baseline checksum");
        assert_eq!(checksum, baseline_checksum());
    }

    #[test]
    fn rejects_unmanaged_nonempty_database() {
        let conn = memory_connection();
        conn.execute_batch("CREATE TABLE legacy_data (id INTEGER PRIMARY KEY);")
            .expect("create legacy fixture");
        let error = apply(&conn).expect_err("legacy database must be rejected");
        assert!(
            error
                .to_string()
                .contains("legacy database migration is unsupported")
        );
    }

    #[test]
    fn rejects_modified_baseline_checksum() {
        let conn = memory_connection();
        apply(&conn).expect("install baseline");
        conn.execute(
            "UPDATE schema_migrations SET checksum = 'modified' WHERE version = 1",
            [],
        )
        .expect("modify checksum fixture");
        let error = apply(&conn).expect_err("checksum mismatch must be rejected");
        assert!(error.to_string().contains("checksum mismatch"));
    }
}
