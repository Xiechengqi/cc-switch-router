use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::db::{Connection, TransactionBehavior, params};
use crate::error::AppError;

const BASELINE_VERSION: i64 = 1;
const BASELINE_SQL: &str = include_str!("../schema/0001_baseline.sql");
const MIGRATIONS: &[(i64, &str)] = &[
    (
        2,
        include_str!("../schema/0002_security_and_market_indexes.sql"),
    ),
    (3, include_str!("../schema/0003_share_descriptor_sync.sql")),
    (
        4,
        include_str!("../schema/0004_market_billing_integrity.sql"),
    ),
    (
        5,
        include_str!("../schema/0005_market_transaction_integrity.sql"),
    ),
    (
        6,
        include_str!("../schema/0006_share_control_reliability.sql"),
    ),
    (
        7,
        include_str!("../schema/0007_client_market_job_leases.sql"),
    ),
    (
        8,
        include_str!("../schema/0008_client_market_terminal_authorizations.sql"),
    ),
    (
        9,
        include_str!("../schema/0009_share_market_price_changes.sql"),
    ),
    (
        10,
        include_str!("../schema/0010_share_market_reconciliation_cursor.sql"),
    ),
    (
        11,
        include_str!("../schema/0011_installation_upgrade_tasks.sql"),
    ),
    (
        12,
        include_str!("../schema/0012_share_request_log_recovery_cursors.sql"),
    ),
];

pub fn apply(conn: &Connection) -> Result<(), AppError> {
    if !has_migration_table(conn)? {
        reject_nonempty_unmanaged_database(conn)?;
        install_baseline(conn, &migration_checksum(BASELINE_SQL))?;
    }
    let applied_versions = validate_migration_history(conn)?;
    apply_pending_migrations(conn, applied_versions)
}

pub fn check_compatibility(conn: &Connection) -> Result<(), AppError> {
    if !has_migration_table(conn)? {
        return reject_nonempty_unmanaged_database(conn);
    }
    validate_migration_history(conn).map(|_| ())
}

fn has_migration_table(conn: &Connection) -> Result<bool, AppError> {
    let table_count = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| AppError::Internal(format!("inspect database schema failed: {error}")))?;
    Ok(table_count != 0)
}

fn validate_migration_history(conn: &Connection) -> Result<usize, AppError> {
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

    if migrations.is_empty() {
        return Err(AppError::Internal(
            "schema_migrations is empty; partial or legacy databases are unsupported".into(),
        ));
    }
    let known = std::iter::once((BASELINE_VERSION, BASELINE_SQL))
        .chain(MIGRATIONS.iter().copied())
        .collect::<Vec<_>>();
    if migrations.len() > known.len() {
        return Err(AppError::Internal(format!(
            "database schema version {} is newer than this binary supports",
            migrations.last().map(|row| row.0).unwrap_or_default()
        )));
    }
    for ((recorded_version, recorded_checksum), (expected_version, sql)) in
        migrations.iter().zip(known.iter())
    {
        if recorded_version != expected_version {
            return Err(AppError::Internal(format!(
                "unsupported database migration history: expected version {expected_version}, found {recorded_version}"
            )));
        }
        let expected_checksum = migration_checksum(sql);
        if recorded_checksum != &expected_checksum {
            return Err(AppError::Internal(format!(
                "database migration {recorded_version} checksum mismatch: expected {expected_checksum}, found {recorded_checksum}"
            )));
        }
    }
    Ok(migrations.len())
}

fn apply_pending_migrations(conn: &Connection, applied_versions: usize) -> Result<(), AppError> {
    for (version, sql) in MIGRATIONS
        .iter()
        .copied()
        .skip(applied_versions.saturating_sub(1))
    {
        let transaction = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                AppError::Internal(format!(
                    "begin database migration {version} failed: {error}"
                ))
            })?;
        transaction.execute_batch(sql).map_err(|error| {
            AppError::Internal(format!(
                "apply database migration {version} failed: {error}"
            ))
        })?;
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, checksum, applied_at) VALUES (?1, ?2, ?3)",
                params![version, migration_checksum(sql), Utc::now().to_rfc3339()],
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "record database migration {version} failed: {error}"
                ))
            })?;
        transaction.commit().map_err(|error| {
            AppError::Internal(format!(
                "commit database migration {version} failed: {error}"
            ))
        })?;
    }
    Ok(())
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

fn migration_checksum(sql: &str) -> String {
    hex::encode(Sha256::digest(sql.as_bytes()))
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
        assert_eq!(table_count, 111);
        let recovery_cursor_table_count = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'share_request_log_recovery_cursors'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count Share request log recovery cursor tables");
        assert_eq!(recovery_cursor_table_count, 1);
        let legacy_profile_table_count = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'user_profiles'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count legacy profile tables");
        assert_eq!(legacy_profile_table_count, 0);
        let versions = conn
            .prepare("SELECT version, checksum FROM schema_migrations ORDER BY version")
            .expect("prepare migration history")
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .expect("query migration history")
            .collect::<Result<Vec<_>, _>>()
            .expect("read migration history");
        assert_eq!(versions.len(), 12);
        assert_eq!(versions[0], (1, migration_checksum(BASELINE_SQL)));
        assert_eq!(versions[1], (2, migration_checksum(MIGRATIONS[0].1)));
        assert_eq!(versions[2], (3, migration_checksum(MIGRATIONS[1].1)));
        assert_eq!(versions[3], (4, migration_checksum(MIGRATIONS[2].1)));
        assert_eq!(versions[4], (5, migration_checksum(MIGRATIONS[3].1)));
        assert_eq!(versions[5], (6, migration_checksum(MIGRATIONS[4].1)));
        assert_eq!(versions[6], (7, migration_checksum(MIGRATIONS[5].1)));
        assert_eq!(versions[7], (8, migration_checksum(MIGRATIONS[6].1)));
        assert_eq!(versions[8], (9, migration_checksum(MIGRATIONS[7].1)));
        assert_eq!(versions[9], (10, migration_checksum(MIGRATIONS[8].1)));
        assert_eq!(versions[10], (11, migration_checksum(MIGRATIONS[9].1)));
        assert_eq!(versions[11], (12, migration_checksum(MIGRATIONS[10].1)));
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

    #[test]
    fn upgrades_a_valid_baseline_with_pending_migrations() {
        let conn = memory_connection();
        install_baseline(&conn, &migration_checksum(BASELINE_SQL)).expect("install version 1");
        check_compatibility(&conn).expect("version 1 is compatible with this binary");
        apply(&conn).expect("apply pending migration");
        let index_count = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name IN (
                    'idx_market_credit_accounts_buyer_currency_updated',
                    'idx_market_credit_accounts_supplier_currency_updated'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count market credit indexes");
        assert_eq!(index_count, 2);
    }
}
