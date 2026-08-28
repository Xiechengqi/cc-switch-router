use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::db::types::Value;
use crate::db::{Connection, TransactionBehavior, params};
use crate::error::AppError;

const BASELINE_VERSION: i64 = 1;
// Frozen once released: every managed database records this file's checksum, so
// editing it makes every existing database fail startup and self-upgrade with a
// version 1 checksum mismatch. Schema changes belong in a new MIGRATIONS entry.
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
    (
        13,
        include_str!("../schema/0013_drop_client_market_recovery.sql"),
    ),
    (14, include_str!("../schema/0014_share_remote_policy.sql")),
    (15, include_str!("../schema/0015_notification_channels.sql")),
    (
        16,
        include_str!("../schema/0016_user_notification_channel_checks.sql"),
    ),
    (
        17,
        include_str!("../schema/0017_telegram_failure_diagnostics.sql"),
    ),
    (
        18,
        include_str!("../schema/0018_single_delivery_channel.sql"),
    ),
    (
        19,
        include_str!("../schema/0019_retire_legacy_token_market.sql"),
    ),
    (
        20,
        include_str!("../schema/0020_share_free_access_policy.sql"),
    ),
    (
        21,
        include_str!("../schema/0021_physically_retire_legacy_token_market.sql"),
    ),
    (
        22,
        include_str!("../schema/0022_client_market_subscription_subdomain.sql"),
    ),
    (
        23,
        include_str!("../schema/0023_share_market_rent_contract.sql"),
    ),
    (
        24,
        include_str!("../schema/0024_share_market_contract_integrity.sql"),
    ),
    (
        25,
        include_str!("../schema/0025_share_market_completion.sql"),
    ),
    (26, include_str!("../schema/0026_share_market_all_apps.sql")),
    (
        27,
        include_str!("../schema/0027_share_model_health_slots.sql"),
    ),
    (
        28,
        include_str!("../schema/0028_share_model_health_evidence.sql"),
    ),
    (
        29,
        include_str!("../schema/0029_installation_online_days.sql"),
    ),
    (30, include_str!("../schema/0030_user_model_routing.sql")),
    (
        31,
        include_str!("../schema/0031_user_model_routing_wildcard.sql"),
    ),
    (
        32,
        include_str!("../schema/0032_grok_media_policy_and_request_kinds.sql"),
    ),
    (
        33,
        include_str!("../schema/0033_request_usage_semantics.sql"),
    ),
];

pub fn apply(conn: &Connection) -> Result<(), AppError> {
    if !has_migration_table(conn)? {
        reject_nonempty_unmanaged_database(conn)?;
        install_baseline(conn, &migration_checksum(BASELINE_SQL))?;
    }
    let applied_versions = validate_migration_history(conn)?;
    apply_pending_migrations(conn, applied_versions)?;
    Ok(())
}

pub fn check_compatibility(conn: &Connection) -> Result<(), AppError> {
    if !has_migration_table(conn)? {
        return reject_nonempty_unmanaged_database(conn);
    }
    let applied_versions = validate_migration_history(conn)?;
    if applied_versions >= LEGACY_TOKEN_MARKET_RETIREMENT_VERSION as usize
        && applied_versions < LEGACY_TOKEN_MARKET_PHYSICAL_RETIREMENT_VERSION as usize
    {
        validate_legacy_token_market_archive(conn)?;
    }
    Ok(())
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
        if version == LEGACY_TOKEN_MARKET_PHYSICAL_RETIREMENT_VERSION {
            validate_legacy_token_market_archive(&transaction)?;
        }
        transaction.execute_batch(sql).map_err(|error| {
            AppError::Internal(format!(
                "apply database migration {version} failed: {error}"
            ))
        })?;
        if version == 19 {
            populate_legacy_token_market_archive_checksums(&transaction)?;
            ensure_legacy_token_market_archive_is_read_only(&transaction)?;
        }
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

const LEGACY_TOKEN_MARKET_RETIREMENT_VERSION: i64 = 19;
const LEGACY_TOKEN_MARKET_PHYSICAL_RETIREMENT_VERSION: i64 = 21;
const LEGACY_TOKEN_MARKET_ARCHIVES: &[(&str, &str)] = &[
    ("router_markets", "legacy_token_market_router_markets"),
    (
        "public_hosts(kind=market)",
        "legacy_token_market_public_hosts",
    ),
    (
        "market_notification_emails",
        "legacy_token_market_notification_emails",
    ),
    ("market_request_logs", "legacy_token_market_request_logs"),
    (
        "market_disabled_shares",
        "legacy_token_market_disabled_shares",
    ),
    (
        "market_share_model_failure_state",
        "legacy_token_market_share_model_failure_state",
    ),
    (
        "market_share_runtime_states",
        "legacy_token_market_share_runtime_states",
    ),
];
const LEGACY_TOKEN_MARKET_READ_ONLY_TABLES: &[(&str, &str, bool)] = &[
    (
        "router_markets",
        "legacy_token_market_router_markets_read_only",
        true,
    ),
    (
        "market_notification_emails",
        "legacy_token_market_notification_emails_read_only",
        true,
    ),
    (
        "market_request_logs",
        "legacy_token_market_request_logs_read_only",
        true,
    ),
    (
        "market_disabled_shares",
        "legacy_token_market_disabled_shares_read_only",
        true,
    ),
    (
        "market_share_model_failure_state",
        "legacy_token_market_model_failure_read_only",
        true,
    ),
    (
        "market_share_runtime_states",
        "legacy_token_market_runtime_states_read_only",
        true,
    ),
    (
        "legacy_token_market_router_markets",
        "legacy_token_market_archive_router_markets_read_only",
        false,
    ),
    (
        "legacy_token_market_public_hosts",
        "legacy_token_market_archive_public_hosts_read_only",
        false,
    ),
    (
        "legacy_token_market_notification_emails",
        "legacy_token_market_archive_notification_emails_read_only",
        false,
    ),
    (
        "legacy_token_market_request_logs",
        "legacy_token_market_archive_request_logs_read_only",
        false,
    ),
    (
        "legacy_token_market_disabled_shares",
        "legacy_token_market_archive_disabled_shares_read_only",
        false,
    ),
    (
        "legacy_token_market_share_model_failure_state",
        "legacy_token_market_archive_model_failure_read_only",
        false,
    ),
    (
        "legacy_token_market_share_runtime_states",
        "legacy_token_market_archive_runtime_states_read_only",
        false,
    ),
    (
        "legacy_token_market_archive_manifest",
        "legacy_token_market_archive_manifest_read_only",
        true,
    ),
];

/// Fill the migration manifest with a deterministic SHA-256 over the archived
/// source columns.  SQLite does not provide a portable SHA-256 function, so we
/// compute it in the Rust migration transaction.  The synthetic `archived_at`
/// column is intentionally excluded: timestamps must not change the checksum.
fn populate_legacy_token_market_archive_checksums(conn: &Connection) -> Result<(), AppError> {
    for (_, archive_table) in LEGACY_TOKEN_MARKET_ARCHIVES {
        let (row_count, checksum) = legacy_token_market_archive_fingerprint(conn, archive_table)?;
        let expected_count = conn
            .query_row(
                "SELECT row_count FROM legacy_token_market_archive_manifest WHERE archive_table = ?1",
                params![archive_table],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "read legacy Token Market archive manifest for {archive_table} failed: {error}"
                ))
            })?;
        if expected_count != row_count {
            return Err(AppError::Internal(format!(
                "legacy Token Market archive row count mismatch for {archive_table}: manifest {expected_count}, observed {row_count}"
            )));
        }
        conn.execute(
            "UPDATE legacy_token_market_archive_manifest
                SET checksum = ?1,
                    notes = CASE
                        WHEN instr(COALESCE(notes, ''), 'checksum=sha256-v1') = 0
                        THEN COALESCE(notes, '') || '; checksum=sha256-v1'
                        ELSE notes
                    END
              WHERE archive_table = ?2",
            params![checksum, archive_table],
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "write legacy Token Market archive checksum for {archive_table} failed: {error}"
            ))
        })?;
    }
    Ok(())
}

fn ensure_legacy_token_market_archive_is_read_only(conn: &Connection) -> Result<(), AppError> {
    for (table, trigger_base, insert_has_suffix) in LEGACY_TOKEN_MARKET_READ_ONLY_TABLES {
        for (operation, suffix) in [
            ("INSERT", "insert"),
            ("UPDATE", "update"),
            ("DELETE", "delete"),
        ] {
            let trigger_name = if operation == "INSERT" && !*insert_has_suffix {
                (*trigger_base).to_string()
            } else {
                format!("{trigger_base}_{suffix}")
            };
            conn.execute_batch(&format!(
                "CREATE TRIGGER IF NOT EXISTS \"{trigger_name}\"
                     BEFORE {operation} ON \"{table}\"
                     BEGIN SELECT RAISE(ABORT, 'legacy Token Market archive is read-only'); END;"
            ))
            .map_err(|error| {
                AppError::Internal(format!(
                    "lock legacy Token Market table {table} failed: {error}"
                ))
            })?;
        }
    }
    Ok(())
}

fn validate_legacy_token_market_archive(conn: &Connection) -> Result<(), AppError> {
    let manifest_rows = conn
        .query_row(
            "SELECT COUNT(*) FROM legacy_token_market_archive_manifest",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "count legacy Token Market archive manifest failed: {error}"
            ))
        })?;
    if manifest_rows != LEGACY_TOKEN_MARKET_ARCHIVES.len() as i64 {
        return Err(AppError::Internal(format!(
            "legacy Token Market archive manifest row count mismatch: expected {}, found {manifest_rows}",
            LEGACY_TOKEN_MARKET_ARCHIVES.len()
        )));
    }

    for (source_table, archive_table) in LEGACY_TOKEN_MARKET_ARCHIVES {
        let manifest = conn
            .query_row(
                "SELECT source_table, row_count, checksum, notes
                   FROM legacy_token_market_archive_manifest
                  WHERE archive_table = ?1",
                params![archive_table],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .map_err(|error| {
                AppError::Internal(format!(
                    "read legacy Token Market archive manifest for {archive_table} failed: {error}"
                ))
            })?;
        let (observed_count, observed_checksum) =
            legacy_token_market_archive_fingerprint(conn, archive_table)?;
        if manifest.0 != *source_table
            || manifest.1 != observed_count
            || manifest.2 != observed_checksum
            || !manifest
                .3
                .as_deref()
                .is_some_and(|notes| notes.contains("checksum=sha256-v1"))
        {
            return Err(AppError::Internal(format!(
                "legacy Token Market archive verification failed for {archive_table}"
            )));
        }
    }
    Ok(())
}

fn legacy_token_market_archive_fingerprint(
    conn: &Connection,
    archive_table: &str,
) -> Result<(i64, String), AppError> {
    let quoted_table = format!("\"{}\"", archive_table.replace('"', "\"\""));
    let column_count = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM pragma_table_info('{}')",
                archive_table.replace('\'', "''")
            ),
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| {
            AppError::Internal(format!(
                "inspect legacy Token Market archive {archive_table} failed: {error}"
            ))
        })?;
    if column_count < 1 {
        return Err(AppError::Internal(format!(
            "legacy Token Market archive {archive_table} has no source columns"
        )));
    }
    let source_column_count = usize::try_from(column_count - 1).map_err(|_| {
        AppError::Internal(format!(
            "invalid legacy Token Market archive column count for {archive_table}"
        ))
    })?;
    let mut statement = conn
        .prepare(&format!("SELECT * FROM {quoted_table}"))
        .map_err(|error| {
            AppError::Internal(format!(
                "prepare legacy Token Market archive {archive_table} failed: {error}"
            ))
        })?;
    let rows = statement
        .query_map([], |row| {
            let mut encoded = Vec::new();
            for index in 0..source_column_count {
                let value: Value = row.get(index)?;
                append_archive_value(&mut encoded, &value);
            }
            Ok(encoded)
        })
        .map_err(|error| {
            AppError::Internal(format!(
                "read legacy Token Market archive {archive_table} failed: {error}"
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            AppError::Internal(format!(
                "decode legacy Token Market archive {archive_table} failed: {error}"
            ))
        })?;
    let row_count = i64::try_from(rows.len()).map_err(|_| {
        AppError::Internal(format!(
            "legacy Token Market archive {archive_table} row count overflow"
        ))
    })?;
    let mut rows = rows;
    rows.sort();
    let mut hasher = Sha256::new();
    hasher.update(b"cc-switch-router:legacy-token-market-archive:v1\0");
    hasher.update(archive_table.as_bytes());
    hasher.update([0]);
    for encoded in rows {
        hasher.update((encoded.len() as u64).to_le_bytes());
        hasher.update(&encoded);
    }
    Ok((row_count, hex::encode(hasher.finalize())))
}

fn append_archive_value(buffer: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => buffer.push(0),
        Value::Integer(number) => {
            buffer.push(1);
            buffer.extend_from_slice(&number.to_le_bytes());
        }
        Value::Real(number) => {
            buffer.push(2);
            buffer.extend_from_slice(&number.to_bits().to_le_bytes());
        }
        Value::Text(text) => {
            buffer.push(3);
            buffer.extend_from_slice(&(text.len() as u64).to_le_bytes());
            buffer.extend_from_slice(text.as_bytes());
        }
        Value::Blob(bytes) => {
            buffer.push(4);
            buffer.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            buffer.extend_from_slice(bytes);
        }
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
        assert_eq!(table_count, 128);
        let removed_client_recovery_table_count = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'client_market_recovery_state'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count removed Client recovery tables");
        assert_eq!(removed_client_recovery_table_count, 0);
        let cleanup_retry_table_count = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'client_market_cleanup_retry_state'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count Client cleanup retry tables");
        assert_eq!(cleanup_retry_table_count, 1);
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
        assert_eq!(versions.len(), MIGRATIONS.len() + 1);
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
        assert_eq!(versions[12], (13, migration_checksum(MIGRATIONS[11].1)));
        assert_eq!(versions[13], (14, migration_checksum(MIGRATIONS[12].1)));
        assert_eq!(versions[14], (15, migration_checksum(MIGRATIONS[13].1)));
        assert_eq!(versions[15], (16, migration_checksum(MIGRATIONS[14].1)));
        assert_eq!(versions[16], (17, migration_checksum(MIGRATIONS[15].1)));
        assert_eq!(versions[17], (18, migration_checksum(MIGRATIONS[16].1)));
        assert_eq!(versions[18], (19, migration_checksum(MIGRATIONS[17].1)));
        assert_eq!(versions[19], (20, migration_checksum(MIGRATIONS[18].1)));
        assert_eq!(versions[20], (21, migration_checksum(MIGRATIONS[19].1)));
        assert_eq!(versions[21], (22, migration_checksum(MIGRATIONS[20].1)));
        assert_eq!(versions[22], (23, migration_checksum(MIGRATIONS[21].1)));
        assert_eq!(versions[23], (24, migration_checksum(MIGRATIONS[22].1)));
        assert_eq!(versions[24], (25, migration_checksum(MIGRATIONS[23].1)));
        assert_eq!(versions[25], (26, migration_checksum(MIGRATIONS[24].1)));
        assert_eq!(versions[26], (27, migration_checksum(MIGRATIONS[25].1)));
        assert_eq!(versions[27], (28, migration_checksum(MIGRATIONS[26].1)));
        assert_eq!(versions[28], (29, migration_checksum(MIGRATIONS[27].1)));
        assert_eq!(versions[29], (30, migration_checksum(MIGRATIONS[28].1)));
        assert_eq!(versions[30], (31, migration_checksum(MIGRATIONS[29].1)));
        assert_eq!(versions[31], (32, migration_checksum(MIGRATIONS[30].1)));
        assert_eq!(versions[32], (33, migration_checksum(MIGRATIONS[31].1)));
    }

    /// The history assertion above is easy to forget when adding a migration
    /// (versions 14 and 15 both landed with it stale). Pin the relationship
    /// itself so the next omission fails loudly instead of silently skipping
    /// the newest entries.
    #[test]
    fn migration_history_covers_every_registered_migration() {
        let conn = memory_connection();
        apply(&conn).expect("install baseline");
        let recorded = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count migration history");
        assert_eq!(recorded as usize, MIGRATIONS.len() + 1);
    }

    /// Databases in the field record this checksum for version 1; a new value
    /// here means every existing deployment would refuse to start or upgrade.
    #[test]
    fn baseline_checksum_stays_frozen() {
        assert_eq!(
            migration_checksum(BASELINE_SQL),
            "53ee1ae8055a7bf720ba8eaced5be6af2957b385017bd2cb8c2fa32e77a0e5a9",
            "schema/0001_baseline.sql is immutable; add a migration instead"
        );
    }

    #[test]
    fn migration_13_retires_recovery_state_on_an_existing_database() {
        let conn = memory_connection();
        install_baseline(&conn, &migration_checksum(BASELINE_SQL)).expect("install version 1");
        conn.execute_batch(
            "INSERT INTO provisioning_jobs (id, type, status, created_at, updated_at)
                 VALUES ('job-create', 'create', 'succeeded', 'now', 'now'),
                        ('job-recover', 'recover', 'failed', 'now', 'now');",
        )
        .expect("seed legacy provisioning jobs");

        apply(&conn).expect("migrate an existing database");

        let surviving_jobs = conn
            .query_row("SELECT COUNT(*) FROM provisioning_jobs", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count surviving jobs");
        assert_eq!(surviving_jobs, 1);
        let legacy_objects = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE name IN (
                    'client_market_recovery_state',
                    'idx_client_market_recovery_due',
                    'client_market_cleanup_recovery_state',
                    'idx_client_market_cleanup_recovery_due',
                    'provisioning_jobs_new'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count retired schema objects");
        assert_eq!(legacy_objects, 0);
        let job_indexes = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'provisioning_jobs'
                   AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count rebuilt job indexes");
        assert_eq!(job_indexes, 5);
        let error = conn
            .execute(
                "INSERT INTO provisioning_jobs
                    (id, type, status, created_at, updated_at)
                 VALUES ('job-recover-2', 'recover', 'pending', 'now', 'now')",
                [],
            )
            .expect_err("migrated databases must reject recovery jobs too");
        assert!(error.to_string().contains("CHECK constraint failed"));
    }

    #[test]
    fn migration_15_installs_channel_neutral_notification_contract() {
        let conn = memory_connection();
        apply(&conn).expect("install final schema");
        for table in [
            "notification_deliveries",
            "notification_delivery_items",
            "notification_delivery_attempts",
            "user_notification_channels",
            "telegram_bot_runtime",
            "telegram_bind_tokens",
            "telegram_inbound_updates",
            "telegram_poll_cursors",
        ] {
            let exists = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get::<_, i64>(0),
                )
                .expect("inspect notification table");
            assert_eq!(exists, 1, "missing {table}");
        }
        for legacy in ["email_delivery_batches", "email_delivery_batch_items"] {
            let exists = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![legacy],
                    |row| row.get::<_, i64>(0),
                )
                .expect("inspect legacy notification table");
            assert_eq!(exists, 0, "legacy table {legacy} must be retired");
        }
    }

    #[test]
    fn migration_16_installs_user_notification_channel_checks() {
        let conn = memory_connection();
        apply(&conn).expect("install final schema");
        let exists = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'user_notification_channel_checks'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("inspect user notification channel checks");
        assert_eq!(exists, 1);
        conn.execute(
            "INSERT INTO user_notification_channel_checks (
                id, channel, config_fingerprint, status, actor_email, tested_at
             ) VALUES ('check-valid', 'telegram', ?1, 'success', 'admin@example.com', 'now')",
            params!["a".repeat(64)],
        )
        .expect("accept a fingerprint-scoped channel check");
        let invalid = conn.execute(
            "INSERT INTO user_notification_channel_checks (
                id, channel, config_fingerprint, status, actor_email, tested_at
             ) VALUES ('check-invalid', 'telegram', 'old-bot', 'success',
                       'admin@example.com', 'now')",
            [],
        );
        assert!(
            invalid.is_err(),
            "channel checks must carry a SHA-256 fingerprint"
        );
    }

    #[test]
    fn migration_17_installs_telegram_failure_diagnostics() {
        let conn = memory_connection();
        apply(&conn).expect("install final schema");
        for (table, columns) in [
            (
                "telegram_bot_runtime",
                [
                    "transport_status",
                    "last_failure_code",
                    "last_failure_hint",
                    "last_failure_details_json",
                    "last_failure_at",
                ]
                .as_slice(),
            ),
            (
                "user_notification_channel_checks",
                ["failure_code", "failure_hint", "failure_details_json"].as_slice(),
            ),
        ] {
            for column in columns {
                let exists = conn
                    .query_row(
                        &format!(
                            "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"
                        ),
                        params![column],
                        |row| row.get::<_, i64>(0),
                    )
                    .expect("inspect Telegram diagnostic column");
                assert_eq!(exists, 1, "missing {table}.{column}");
            }
        }
    }

    /// Delivery is single-select: the schema, not just the write path, refuses
    /// to leave a user with two selected channels.
    #[test]
    fn migration_18_enforces_a_single_selected_channel_per_user() {
        let conn = memory_connection();
        apply(&conn).expect("install final schema");
        conn.execute_batch(
            "INSERT INTO users (id, email_normalized, created_at, last_login_at)
             VALUES ('user-single', 'single@example.com', 'now', 'now');
             INSERT INTO user_notification_channels
                (user_id, channel, enabled, state, target, revision, created_at, updated_at)
             VALUES ('user-single', 'email', 1, 'ready', 'single@example.com', 1, 'now', 'now');",
        )
        .expect("seed the default email selection");

        let conflict = conn
            .execute(
                "INSERT INTO user_notification_channels
                    (user_id, channel, enabled, state, target, provider_identity,
                     revision, created_at, updated_at)
                 VALUES ('user-single', 'telegram', 1, 'ready', '4242', 'bot-1', 1, 'now', 'now')",
                [],
            )
            .expect_err("two selected channels must be impossible");
        assert!(conflict.to_string().contains("UNIQUE constraint failed"));

        // Deselecting first is always allowed, and an unselected row does not
        // participate in the constraint at all.
        conn.execute_batch(
            "UPDATE user_notification_channels SET enabled = 0
              WHERE user_id = 'user-single' AND channel = 'email';
             INSERT INTO user_notification_channels
                (user_id, channel, enabled, state, target, provider_identity,
                 revision, created_at, updated_at)
             VALUES ('user-single', 'telegram', 1, 'ready', '4242', 'bot-1', 1, 'now', 'now');",
        )
        .expect("switching the selection stays possible");
    }

    #[test]
    fn migration_15_normalizes_extensible_user_channels() {
        let conn = memory_connection();
        apply(&conn).expect("install baseline");
        conn.execute_batch(
            "INSERT INTO users (id, email_normalized, created_at, last_login_at)
             VALUES ('user-a', 'a@example.com', 'now', 'now'),
                    ('user-b', 'b@example.com', 'now', 'now');",
        )
        .expect("seed users");

        conn.execute_batch(
            "INSERT INTO user_notification_channels
                (user_id, channel, enabled, state, target, provider_identity,
                 revision, verified_at, created_at, updated_at)
             VALUES ('user-a', 'telegram', 1, 'ready', '4242', 'bot-1', 1, 'now', 'now', 'now');",
        )
        .expect("bind the first account");
        let conflict = conn
            .execute(
                "INSERT INTO user_notification_channels
                    (user_id, channel, enabled, state, target, provider_identity,
                     revision, verified_at, created_at, updated_at)
                 VALUES ('user-b', 'telegram', 1, 'ready', '4242', 'bot-1', 1, 'now', 'now', 'now')",
                [],
            )
            .expect_err("a chat must not back two accounts");
        assert!(conflict.to_string().contains("UNIQUE constraint failed"));

        conn.execute(
            "INSERT INTO user_notification_channels
                (user_id, channel, enabled, state, target, revision, created_at, updated_at)
             VALUES ('user-b', 'matrix', 0, 'unbound', NULL, 1, 'now', 'now')",
            [],
        )
        .expect("schema permits future registered channels without a migration");
    }

    #[test]
    fn provisioning_jobs_only_accept_explicit_create_or_cleanup_work() {
        let conn = memory_connection();
        apply(&conn).expect("install baseline");

        let error = conn
            .execute(
                "INSERT INTO provisioning_jobs
                    (id, type, status, created_at, updated_at)
                 VALUES ('job-recover', 'recover', 'pending', 'now', 'now')",
                [],
            )
            .expect_err("automatic recovery jobs must be rejected");
        assert!(error.to_string().contains("CHECK constraint failed"));
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

    fn install_schema_through(conn: &Connection, version: i64) {
        let latest = MIGRATIONS.last().map(|(version, _)| *version).unwrap_or(1);
        assert!((1..=latest).contains(&version));
        install_baseline(conn, &migration_checksum(BASELINE_SQL)).expect("install baseline");
        for (migration_version, sql) in MIGRATIONS.iter().copied().take((version - 1) as usize) {
            if migration_version == LEGACY_TOKEN_MARKET_PHYSICAL_RETIREMENT_VERSION {
                validate_legacy_token_market_archive(conn)
                    .expect("validate legacy Token Market archive");
            }
            conn.execute_batch(sql)
                .unwrap_or_else(|error| panic!("apply migration {migration_version}: {error}"));
            if migration_version == LEGACY_TOKEN_MARKET_RETIREMENT_VERSION {
                populate_legacy_token_market_archive_checksums(conn)
                    .expect("finalize legacy Token Market archive");
                ensure_legacy_token_market_archive_is_read_only(conn)
                    .expect("lock legacy Token Market archive");
            }
            conn.execute(
                "INSERT INTO schema_migrations (version, checksum, applied_at)
                 VALUES (?1, ?2, 'test')",
                params![migration_version, migration_checksum(sql)],
            )
            .unwrap_or_else(|error| panic!("record migration {migration_version}: {error}"));
        }
    }

    #[test]
    fn migrations_27_through_33_upgrade_a_version_26_database() {
        let conn = memory_connection();
        install_schema_through(&conn, 26);

        let before = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name IN (
                    'share_model_health_slots',
                    'share_model_probe_observations',
                    'share_model_probe_epochs'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("inspect version 26 model-health tables");
        assert_eq!(before, 0);

        apply(&conn).expect("upgrade version 26 through model-health evidence schema");
        let tables = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name IN (
                    'share_model_health_slots',
                    'share_model_probe_observations',
                    'share_model_probe_epochs'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count model-health tables after upgrade");
        assert_eq!(tables, 3);
        let evidence_columns = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('share_model_health_slots')
                 WHERE name IN (
                    'observation_id', 'probe_epoch_id', 'outcome', 'failure_domain',
                    'reason_code', 'evidence_scope', 'evidence_version'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count model-health evidence columns after upgrade");
        assert_eq!(evidence_columns, 7);
        let latest_version = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("read upgraded schema version");
        assert_eq!(latest_version, 33);
        check_compatibility(&conn).expect("upgraded version 33 is compatible");
    }

    #[test]
    fn migration_30_installs_user_model_routing_without_a_share_foreign_key() {
        let conn = memory_connection();
        install_schema_through(&conn, 29);
        conn.execute_batch(
            "INSERT INTO users (id, email_normalized, created_at, last_login_at)
             VALUES ('route-user', 'route@example.com', 'now', 'now');",
        )
        .expect("seed routing user before upgrade");

        apply(&conn).expect("upgrade version 29 through user model routing schema");

        let table_count = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name IN (
                    'user_model_routing_profiles',
                    'user_model_routes',
                    'user_model_route_events'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count user model routing tables");
        assert_eq!(table_count, 3);

        let route_foreign_keys = conn
            .prepare("PRAGMA foreign_key_list('user_model_routes')")
            .expect("prepare route foreign keys")
            .query_map([], |row| row.get::<_, String>(2))
            .expect("query route foreign keys")
            .collect::<Result<Vec<_>, _>>()
            .expect("read route foreign keys");
        assert_eq!(route_foreign_keys, vec!["users".to_string()]);

        conn.execute_batch(
            "INSERT INTO user_model_routing_profiles
                (user_id, revision, created_at, updated_at)
             VALUES ('route-user', 1, 'now', 'now');
             INSERT INTO user_model_routes
                (id, user_id, app_type, requested_model, target_share_id, created_at, updated_at)
             VALUES ('route-stale', 'route-user', 'codex', 'gpt-exact',
                     'removed-share', 'now', 'now');",
        )
        .expect("stale Share target remains representable");
        let duplicate = conn.execute(
            "INSERT INTO user_model_routes
                (id, user_id, app_type, requested_model, target_share_id, created_at, updated_at)
             VALUES ('route-duplicate', 'route-user', 'codex', 'gpt-exact',
                     'another-share', 'now', 'now')",
            [],
        );
        assert!(duplicate.is_err(), "exact route keys must remain unique");

        let latest_version = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("read upgraded schema version");
        assert_eq!(latest_version, 33);
    }

    #[test]
    fn migration_31_preserves_exact_routes_and_admits_only_the_standalone_wildcard() {
        let conn = memory_connection();
        install_schema_through(&conn, 30);
        conn.execute_batch(
            "INSERT INTO users (id, email_normalized, created_at, last_login_at)
             VALUES ('wildcard-user', 'wildcard@example.com', 'now', 'now');
             INSERT INTO user_model_routes
                (id, user_id, app_type, requested_model, target_share_id, created_at, updated_at)
             VALUES ('route-kept', 'wildcard-user', 'codex', 'gpt-5.6-sol',
                     'share-a', 'then', 'then');",
        )
        .expect("seed an exact route before the wildcard upgrade");

        apply(&conn).expect("upgrade version 30 through the wildcard schema");

        // The table is rebuilt to add the CHECK; existing rows must survive intact,
        // identity and timestamps included, or saved routes would silently vanish.
        let kept = conn
            .query_row(
                "SELECT requested_model, target_share_id, created_at
                 FROM user_model_routes WHERE id = 'route-kept'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .expect("exact route survives the rebuild");
        assert_eq!(
            kept,
            (
                "gpt-5.6-sol".to_string(),
                "share-a".to_string(),
                "then".to_string()
            )
        );

        let route_foreign_keys = conn
            .prepare("PRAGMA foreign_key_list('user_model_routes')")
            .expect("prepare route foreign keys")
            .query_map([], |row| row.get::<_, String>(2))
            .expect("query route foreign keys")
            .collect::<Result<Vec<_>, _>>()
            .expect("read route foreign keys");
        assert_eq!(route_foreign_keys, vec!["users".to_string()]);

        conn.execute_batch(
            "INSERT INTO user_model_routes
                (id, user_id, app_type, requested_model, target_share_id, created_at, updated_at)
             VALUES ('route-wildcard', 'wildcard-user', 'codex', '*',
                     'share-c', 'now', 'now');",
        )
        .expect("the standalone wildcard is a valid route key");

        let second_wildcard = conn.execute(
            "INSERT INTO user_model_routes
                (id, user_id, app_type, requested_model, target_share_id, created_at, updated_at)
             VALUES ('route-wildcard-2', 'wildcard-user', 'codex', '*',
                     'share-d', 'now', 'now')",
            [],
        );
        assert!(
            second_wildcard.is_err(),
            "the existing unique key must cap each (user, app) at one wildcard"
        );

        conn.execute_batch(
            "INSERT INTO user_model_routes
                (id, user_id, app_type, requested_model, target_share_id, created_at, updated_at)
             VALUES ('route-wildcard-claude', 'wildcard-user', 'claude', '*',
                     'share-e', 'now', 'now');",
        )
        .expect("wildcards are scoped per app, not per user");

        for pattern in ["gpt-*", "*-turbo", "a*b"] {
            let partial = conn.execute(
                "INSERT INTO user_model_routes
                    (id, user_id, app_type, requested_model, target_share_id, created_at, updated_at)
                 VALUES ('route-pattern', 'wildcard-user', 'gemini', ?1,
                         'share-f', 'now', 'now')",
                params![pattern],
            );
            assert!(
                partial.is_err(),
                "`{pattern}` must be rejected so `*` cannot decay into pattern matching"
            );
        }
    }

    #[test]
    fn migration_32_adds_fail_closed_grok_policy_and_typed_media_log_columns() {
        let conn = memory_connection();
        install_schema_through(&conn, 31);

        apply(&conn).expect("upgrade version 31 through Grok media schema");

        let share_columns = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('shares')
                 WHERE name IN (
                    'grok_image_generation_enabled',
                    'grok_image_edit_enabled',
                    'grok_video_generation_enabled'
                 ) AND [notnull] = 1 AND dflt_value = '0'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count fail-closed Grok Share policy columns");
        assert_eq!(share_columns, 3);

        let request_log_columns = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('share_request_logs')
                 WHERE name IN (
                    'request_kind', 'operation', 'parent_request_id', 'error_message',
                    'media_task_id', 'media_status', 'video_duration_seconds',
                    'video_resolution', 'video_aspect_ratio'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count typed media request log columns");
        assert_eq!(request_log_columns, 9);

        let media_indexes = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name IN (
                    'idx_share_request_logs_share_kind_created',
                    'idx_share_request_logs_parent_request',
                    'idx_share_request_logs_media_task'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count media request log indexes");
        assert_eq!(media_indexes, 3);
    }

    #[test]
    fn migration_33_adds_truthful_request_usage_semantics() {
        let conn = memory_connection();
        install_schema_through(&conn, 32);
        conn.execute_batch(
            "INSERT INTO share_request_logs (
                request_id, installation_id, share_id, share_name, provider_id,
                provider_name, app_type, model, request_model, status_code,
                latency_ms, input_tokens, output_tokens, cache_read_tokens,
                cache_creation_tokens, is_streaming, created_at
             ) VALUES (
                'legacy-request', 'installation', 'share', 'Share', 'provider',
                'Provider', 'codex', 'gpt-test', 'gpt-test', 200,
                1000, 10, 2, 0, 0, 1, 1
             );",
        )
        .expect("seed version 32 request log");

        apply(&conn).expect("upgrade request usage semantics");

        let legacy = conn
            .query_row(
                "SELECT cache_usage_observed, usage_estimated
                   FROM share_request_logs WHERE request_id = 'legacy-request'",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .expect("read migrated request usage semantics");
        assert_eq!(legacy, (1, 0));
        let latest_version = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("read upgraded schema version");
        assert_eq!(latest_version, 33);
    }

    #[test]
    fn migration_24_preserves_dispatched_terms_and_expires_legacy_quotes() {
        let conn = memory_connection();
        install_schema_through(&conn, 23);
        conn.execute_batch(
            "INSERT INTO shares
                (share_id, capacity_pool_id, installation_id, share_name, owner_email,
                 for_sale, app_type, token_limit, parallel_limit, tokens_used,
                 requests_count, share_status, created_at, expires_at, updated_at)
             VALUES ('share-v23', 'pool-v23', 'installation-v23', 'Share v23',
                     'owner@example.com', 'Free', 'codex', -1, -1, 0, 0, 'active',
                     '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z',
                     '2026-01-01T00:00:00Z');
             INSERT INTO share_market_listings
                (id, share_id, installation_id, owner_user_id, owner_email, status,
                 created_at, updated_at)
             VALUES ('listing-v23', 'share-v23', 'installation-v23', 'owner-v23',
                     'owner@example.com', 'active', '2026-01-01T00:00:00Z',
                     '2026-01-01T00:00:00Z');
             INSERT INTO share_market_seats
                (id, listing_id, position, status, token_period_json,
                 service_duration_days, offer_revision, current_subscription_id,
                 created_at, updated_at)
             VALUES
                ('seat-v23-pending', 'listing-v23', 1, 'reserved', '\"day\"', 7, 1,
                 'subscription-v23-pending', '2026-01-01T00:00:00Z',
                 '2026-01-01T00:00:00Z'),
                ('seat-v23-active', 'listing-v23', 2, 'occupied', '\"day\"', 7, 1,
                 'subscription-v23-active', '2026-01-01T00:00:00Z',
                 '2026-01-01T00:00:00Z');
             INSERT INTO share_market_subscriptions
                (id, seat_id, listing_id, share_id, installation_id, entitlement_id,
                 owner_user_id, owner_email, renter_user_id, renter_email, status,
                 token_period_json, service_duration_days, offer_revision,
                 activated_at, expires_at, created_at, updated_at, service_started_at)
             VALUES
                ('subscription-v23-pending', 'seat-v23-pending', 'listing-v23', 'share-v23',
                 'installation-v23', 'entitlement-v23-pending', 'owner-v23',
                 'owner@example.com', 'renter-v23-pending', 'pending@example.com',
                 'grant_pending', '\"day\"', 7, 1, NULL, NULL,
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', NULL),
                ('subscription-v23-active', 'seat-v23-active', 'listing-v23', 'share-v23',
                 'installation-v23', 'entitlement-v23-active', 'owner-v23',
                 'owner@example.com', 'renter-v23-active', 'active@example.com',
                 'active_postpaid', '\"day\"', 7, 1, '2026-01-02T00:00:00Z',
                 '2026-01-09T00:00:00Z', '2026-01-01T00:00:00Z',
                 '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z');
             INSERT INTO share_market_rent_quotes
                (id, seat_id, listing_id, share_id, renter_user_id, renter_email,
                 offer_revision, snapshot_json, trial_seconds_remaining, status,
                 expires_at, created_at, updated_at)
             VALUES ('quote-v23', 'seat-v23-pending', 'listing-v23', 'share-v23',
                     'renter-v23-pending', 'pending@example.com', 1, '{}', 0, 'active',
                     '2026-01-01T00:02:00Z', '2026-01-01T00:00:00Z',
                     '2026-01-01T00:00:00Z');
             INSERT INTO market_service_contracts
                (id, account_id, product_kind, product_ref, service_ref, service_label,
                 buyer_user_id, buyer_email, supplier_user_id, supplier_email, currency,
                 daily_rate_minor, offer_revision, status, trial_seconds_remaining,
                 last_evaluated_at, activated_at, created_at, updated_at)
             VALUES ('contract-v23', 'account-v23', 'share', 'subscription-v23-active',
                     'share-v23', 'Share v23', 'renter-v23-active', 'active@example.com',
                     'owner-v23', 'owner@example.com', 'USD', 1200, 1, 'active', 0,
                     '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z',
                     '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z');",
        )
        .expect("seed version 23 Share Market contracts");

        apply(&conn).expect("upgrade version 23 Share Market contracts");

        let quote: (String, Option<String>) = conn
            .query_row(
                "SELECT status, required_app FROM share_market_rent_quotes WHERE id = 'quote-v23'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read migrated quote");
        assert_eq!(quote, ("expired".into(), None));
        let subscriptions = conn
            .prepare(
                "SELECT id, required_app, json_extract(service_snapshot_json, '$.schemaVersion'),
                        service_started_at
                 FROM share_market_subscriptions ORDER BY id",
            )
            .expect("prepare migrated subscriptions")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .expect("query migrated subscriptions")
            .collect::<Result<Vec<_>, _>>()
            .expect("read migrated subscriptions");
        assert_eq!(
            subscriptions,
            vec![
                (
                    "subscription-v23-active".into(),
                    "codex".into(),
                    0,
                    Some("2026-01-02T00:00:00Z".into()),
                ),
                ("subscription-v23-pending".into(), "codex".into(), 0, None,),
            ]
        );
        let contract_service_started_at: String = conn
            .query_row(
                "SELECT service_started_at FROM market_service_contracts
                 WHERE id = 'contract-v23'",
                [],
                |row| row.get(0),
            )
            .expect("read migrated billing service start");
        assert_eq!(contract_service_started_at, "2026-01-02T00:00:00Z");
    }

    #[test]
    fn migration_26_backfills_app_bundles_and_schedules_active_scope_upgrades() {
        let conn = memory_connection();
        install_schema_through(&conn, 25);
        conn.execute_batch(
            r#"INSERT INTO shares
                (share_id, capacity_pool_id, installation_id, share_name, owner_email,
                 for_sale, app_type, token_limit, parallel_limit, tokens_used,
                 requests_count, share_status, created_at, expires_at, updated_at)
             VALUES ('share-v25', 'pool-v25', 'installation-v25', 'Share v25',
                     'owner@example.com', 'Free', 'codex', -1, 3, 0, 0, 'active',
                     '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z',
                     '2026-01-01T00:00:00Z');
             INSERT INTO share_market_listings
                (id, share_id, installation_id, owner_user_id, owner_email, status,
                 created_at, updated_at)
             VALUES ('listing-v25', 'share-v25', 'installation-v25', 'owner-v25',
                     'owner@example.com', 'active', '2026-01-01T00:00:00Z',
                     '2026-01-01T00:00:00Z');
             INSERT INTO share_market_seats
                (id, listing_id, position, status, token_period_json, offer_revision,
                 current_subscription_id, created_at, updated_at)
             VALUES
                ('seat-v25-active', 'listing-v25', 1, 'occupied', '"day"', 1,
                 'subscription-v25-active', '2026-01-01T00:00:00Z',
                 '2026-01-01T00:00:00Z'),
                ('seat-v25-released', 'listing-v25', 2, 'available', '"day"', 1,
                 NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO share_market_subscriptions
                (id, seat_id, listing_id, share_id, installation_id, entitlement_id,
                 owner_user_id, owner_email, renter_user_id, renter_email, status,
                 token_period_json, offer_revision, activated_at, service_started_at,
                 created_at, updated_at, released_at, required_app,
                 service_snapshot_json, app_scope_enforced_at)
             VALUES
                ('subscription-v25-active', 'seat-v25-active', 'listing-v25',
                 'share-v25', 'installation-v25', 'entitlement-v25-active',
                 'owner-v25', 'owner@example.com', 'renter-v25-active',
                 'active@example.com', 'active_free', '"day"', 1,
                 '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z',
                 '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z', NULL, 'codex',
                 '{"schemaVersion":1,"requiredApp":"codex","supportedApps":["claude","codex"]}',
                 '2026-01-02T00:00:00Z'),
                ('subscription-v25-released', 'seat-v25-released', 'listing-v25',
                 'share-v25', 'installation-v25', 'entitlement-v25-released',
                 'owner-v25', 'owner@example.com', 'renter-v25-released',
                 'released@example.com', 'released', '"day"', 1, NULL, NULL,
                 '2026-01-01T00:00:00Z', '2026-01-03T00:00:00Z',
                 '2026-01-03T00:00:00Z', 'gemini', '{}',
                 '2026-01-02T00:00:00Z');
             INSERT INTO share_market_rent_quotes
                (id, seat_id, listing_id, share_id, renter_user_id, renter_email,
                 offer_revision, snapshot_json, trial_seconds_remaining, status,
                 required_app, expires_at, created_at, updated_at)
             VALUES
                ('quote-v25-active', 'seat-v25-active', 'listing-v25', 'share-v25',
                 'quote-renter-v25-active', 'quote-active@example.com', 1,
                 '{"service":{"schemaVersion":1,"requiredApp":"codex","supportedApps":["claude","codex"]}}',
                 0, 'active', 'codex', '2026-01-01T00:02:00Z',
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
                ('quote-v25-consumed', 'seat-v25-released', 'listing-v25', 'share-v25',
                 'quote-renter-v25-consumed', 'quote-consumed@example.com', 1, '{}',
                 0, 'consumed', 'gemini', '2026-01-01T00:02:00Z',
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');"#,
        )
        .expect("seed version 25 App-scoped Share Market contracts");

        apply(&conn).expect("upgrade version 25 Share Market App contracts");

        let quotes = conn
            .prepare(
                "SELECT id, status, contract_apps_json
                 FROM share_market_rent_quotes ORDER BY id",
            )
            .expect("prepare migrated App quote rows")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .expect("query migrated App quote rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("read migrated App quote rows");
        assert_eq!(
            quotes,
            vec![
                (
                    "quote-v25-active".into(),
                    "expired".into(),
                    r#"["claude","codex"]"#.into(),
                ),
                (
                    "quote-v25-consumed".into(),
                    "consumed".into(),
                    r#"["gemini"]"#.into(),
                ),
            ]
        );
        let subscriptions = conn
            .prepare(
                "SELECT id, contract_apps_json, app_scope_enforced_at
                 FROM share_market_subscriptions ORDER BY id",
            )
            .expect("prepare migrated App subscription rows")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .expect("query migrated App subscription rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("read migrated App subscription rows");
        assert_eq!(
            subscriptions,
            vec![
                (
                    "subscription-v25-active".into(),
                    r#"["claude","codex"]"#.into(),
                    None,
                ),
                (
                    "subscription-v25-released".into(),
                    r#"["gemini"]"#.into(),
                    Some("2026-01-02T00:00:00Z".into()),
                ),
            ]
        );
        let invalid_bundle = conn.execute(
            "UPDATE share_market_subscriptions SET contract_apps_json = '{}' WHERE id = ?1",
            params!["subscription-v25-active"],
        );
        assert!(
            invalid_bundle.is_err(),
            "contract Apps must remain a JSON array"
        );
    }

    #[test]
    fn migration_25_matches_runtime_public_market_projection_and_backfills_release_time() {
        let conn = memory_connection();
        install_schema_through(&conn, 24);
        let payload = serde_json::json!({
            "summary": "Share seat rented",
            "marketKind": "share",
            "billingEventType": "service_started",
            "installationId": "installation-v24",
            "shareId": "share-v24",
            "shareName": "Share v24",
            "appType": "codex",
            "subdomain": "share-v24-route",
            "ownerEmail": "owner@example.com",
            "supplierEmail": "supplier@example.com",
            "listingId": "listing-v24",
            "seatId": "seat-v24",
            "seatPosition": 1,
            "seatStatus": "occupied",
            "subscriptionStatus": "released",
            "parallelLimit": 2,
            "tokenLimit": 1_000_000,
            "tokenPeriod": "day",
            "dailyRateMinor": 1200,
            "currency": "USD",
            "serviceDurationDays": 10,
            "offerRevision": 3,
            "paymentMethods": [{
                "kind": "alipay",
                "account": "private-payment-account"
            }],
            "paymentContacts": [{
                "kind": "email",
                "value": "payments@example.com"
            }],
            "renterUserId": "private-renter-id",
            "renterEmail": "private-renter@example.com",
            "buyerEmail": "private-buyer@example.com",
            "balanceMinor": 1234,
            "invoiceId": "private-invoice",
            "paymentReference": "private-reference",
            "evidenceUrl": "https://private.invalid/evidence",
            "dispute": { "reason": "private-dispute" },
            "actorUserId": "private-actor-id",
            "actorEmail": "private-actor@example.com"
        });
        let payload_json = serde_json::to_string(&payload).expect("encode historical payload");
        let billing_payload = serde_json::json!({
            "summary": "Payment is due",
            "marketKind": "billing",
            "billingEventType": "payment_due",
            "installationId": "installation-v24",
            "supplierEmail": "supplier@example.com",
            "paymentMethods": [{
                "kind": "alipay",
                "account": "private-billing-account"
            }],
            "paymentContacts": [{
                "kind": "email",
                "value": "payments@example.com"
            }],
            "buyerUserId": "private-buyer-id",
            "buyerEmail": "private-buyer@example.com",
            "balanceMinor": 1234,
            "creditLimitMinor": 5000,
            "invoiceId": "private-invoice",
            "paymentReference": "private-reference",
            "evidenceUrl": "https://private.invalid/evidence",
            "dispute": { "reason": "private-dispute" },
            "refundReference": "private-refund-reference"
        });
        let billing_payload_json =
            serde_json::to_string(&billing_payload).expect("encode historical billing payload");
        let client_payload = serde_json::json!({
            "summary": "Client provisioned",
            "marketKind": "client",
            "installationId": "installation-v24",
            "clientLabel": "client-v24-route",
            "providerEmail": "provider@example.com",
            "hostname": "host-v24.example.com",
            "status": "active",
            "dailyRateMinor": 2400,
            "currency": "USD",
            "offerRevision": 4,
            "trialHours": 12,
            "freeDurationDays": 3,
            "activatedAt": "2026-01-01T00:00:00Z",
            "expiresAt": "2026-02-01T00:00:00Z",
            "providerDeniedClientAccess": false,
            "reason": "service ready",
            "failureCode": "none",
            "paymentMethods": [{
                "kind": "crypto",
                "account": "private-client-payment-account",
                "address": "private-client-payment-address"
            }],
            "paymentContacts": [{
                "channel": "telegram",
                "handle": "public-provider-contact"
            }],
            "clientUserId": "private-client-user-id",
            "clientOwnerEmail": "private-client-owner@example.com",
            "providerUserId": "private-provider-user-id",
            "actorUserId": "private-actor-id",
            "actorEmail": "private-actor@example.com"
        });
        let client_payload_json =
            serde_json::to_string(&client_payload).expect("encode historical Client payload");
        conn.execute_batch(
            "INSERT INTO shares
                (share_id, capacity_pool_id, installation_id, share_name, owner_email,
                 for_sale, app_type, token_limit, parallel_limit, tokens_used,
                 requests_count, share_status, created_at, expires_at, updated_at)
             VALUES ('share-v24', 'pool-v24', 'installation-v24', 'Share v24',
                     'owner@example.com', 'Free', 'codex', -1, 3, 0, 0, 'active',
                     '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z',
                     '2026-02-01T00:00:00Z');
             INSERT INTO share_market_listings
                (id, share_id, installation_id, owner_user_id, owner_email, status,
                 created_at, updated_at)
             VALUES ('listing-v24', 'share-v24', 'installation-v24', 'owner-v24',
                     'owner@example.com', 'active', '2026-01-01T00:00:00Z',
                     '2026-02-01T00:00:00Z');
             INSERT INTO share_market_seats
                (id, listing_id, position, status, token_period_json,
                 offer_revision, current_subscription_id, created_at, updated_at)
             VALUES ('seat-v24', 'listing-v24', 1, 'available', '\"day\"', 3,
                     NULL, '2026-01-01T00:00:00Z', '2026-02-01T00:00:00Z');
             INSERT INTO share_market_subscriptions
                (id, seat_id, listing_id, share_id, installation_id, entitlement_id,
                 owner_user_id, owner_email, renter_user_id, renter_email, status,
                 parallel_limit, token_limit, token_period_json, daily_rate_minor,
                 currency, service_duration_days, offer_revision, created_at, updated_at,
                 released_at)
             VALUES ('subscription-v24', 'seat-v24', 'listing-v24', 'share-v24',
                     'installation-v24', 'entitlement-v24', 'owner-v24',
                     'owner@example.com', 'renter-v24', 'renter@example.com', 'released',
                     2, 1000000, '\"day\"', 1200, 'USD', 10, 3,
                     '2026-01-01T00:00:00Z', '2026-02-01T00:00:00Z', NULL);
             INSERT INTO chat_rooms
                (id, installation_id, client_label_snapshot, owner_email_snapshot,
                 created_at, updated_at)
             VALUES ('room-v24', 'installation-v24', 'Client v24', 'owner@example.com',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .expect("seed version 24 Share and chat rows");
        conn.execute(
            "INSERT INTO chat_messages (
                id, room_id, author_user_id, author_email, author_label,
                client_message_id, author_kind, message_kind, event_type,
                event_payload_json, payload_version, source_event_id, body,
                created_at, updated_at
             ) VALUES ('message-v24', 'room-v24', 'system', '', 'System message',
                       'share-market:event-v24', 'system', 'market_event', 'seat_rented',
                       ?1, 1, 'share-market:event-v24', 'Share seat rented',
                       '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            params![payload_json],
        )
        .expect("seed historical materialized market event");
        conn.execute(
            "INSERT INTO chat_messages (
                id, room_id, author_user_id, author_email, author_label,
                client_message_id, author_kind, message_kind, event_type,
                event_payload_json, payload_version, source_event_id, body,
                created_at, updated_at
             ) VALUES ('billing-message-v24', 'room-v24', 'system', '', 'System message',
                       'market_billing:event-v24', 'system', 'market_event', 'payment_due',
                       ?1, 1, 'market_billing:event-v24', 'Payment is due',
                       '2026-01-01T00:00:01Z', '2026-01-01T00:00:01Z')",
            params![billing_payload_json],
        )
        .expect("seed historical materialized billing event");
        conn.execute(
            "INSERT INTO client_chat_system_outbox (
                id, installation_id, source_kind, source_event_id, event_type,
                payload_json, follower_user_ids_json, status, attempts,
                created_at, updated_at
             ) VALUES ('outbox-v24', 'installation-v24', 'share_market', 'event-v24',
                       'seat_rented', ?1, '[]', 'dead_letter', 3,
                       '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            params![payload_json],
        )
        .expect("seed historical dead-letter market event");
        conn.execute(
            "INSERT INTO chat_messages (
                id, room_id, author_user_id, author_email, author_label,
                client_message_id, author_kind, message_kind, event_type,
                event_payload_json, payload_version, source_event_id, body,
                created_at, updated_at
             ) VALUES ('client-message-v24', 'room-v24', 'system', '', 'System message',
                       'client_market:event-v24', 'system', 'market_event', 'client_provisioned',
                       ?1, 1, 'client_market:event-v24', 'Client provisioned',
                       '2026-01-01T00:00:02Z', '2026-01-01T00:00:02Z')",
            params![client_payload_json],
        )
        .expect("seed historical materialized Client Market event");
        conn.execute(
            "INSERT INTO client_chat_system_outbox (
                id, installation_id, source_kind, source_event_id, event_type,
                payload_json, follower_user_ids_json, status, attempts,
                created_at, updated_at, completed_at
             ) VALUES ('client-outbox-v24', 'installation-v24', 'client_market',
                       'client-event-v24', 'client_provisioned', ?1, '[]', 'completed', 1,
                       '2026-01-01T00:00:02Z', '2026-01-01T00:00:02Z',
                       '2026-01-01T00:00:02Z')",
            params![client_payload_json],
        )
        .expect("seed historical completed Client Market event");

        apply(&conn).expect("upgrade version 24 public market events");

        let expected = crate::store::client_chat::public_market_event_payload(&payload);
        let (message_payload, payload_version): (String, i64) = conn
            .query_row(
                "SELECT event_payload_json, payload_version FROM chat_messages
                 WHERE id = 'message-v24'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read migrated market message");
        let outbox_payload: String = conn
            .query_row(
                "SELECT payload_json FROM client_chat_system_outbox WHERE id = 'outbox-v24'",
                [],
                |row| row.get(0),
            )
            .expect("read migrated market outbox");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&message_payload)
                .expect("decode migrated message payload"),
            expected
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&outbox_payload)
                .expect("decode migrated outbox payload"),
            expected
        );
        assert_eq!(payload_version, 2);
        let (billing_message_payload, billing_payload_version): (String, i64) = conn
            .query_row(
                "SELECT event_payload_json, payload_version FROM chat_messages
                 WHERE id = 'billing-message-v24'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read migrated billing message");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&billing_message_payload)
                .expect("decode migrated billing payload"),
            crate::store::client_chat::public_market_event_payload(&billing_payload)
        );
        assert_eq!(billing_payload_version, 2);
        let expected_client =
            crate::store::client_chat::public_market_event_payload(&client_payload);
        let (client_message_payload, client_payload_version): (String, i64) = conn
            .query_row(
                "SELECT event_payload_json, payload_version FROM chat_messages
                 WHERE id = 'client-message-v24'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read migrated Client Market message");
        let client_outbox_payload: String = conn
            .query_row(
                "SELECT payload_json FROM client_chat_system_outbox
                 WHERE id = 'client-outbox-v24'",
                [],
                |row| row.get(0),
            )
            .expect("read migrated Client Market outbox");
        let client_message_payload =
            serde_json::from_str::<serde_json::Value>(&client_message_payload)
                .expect("decode migrated Client Market message");
        assert_eq!(client_message_payload, expected_client);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&client_outbox_payload)
                .expect("decode migrated Client Market outbox"),
            expected_client
        );
        assert_eq!(client_payload_version, 2);
        assert_eq!(
            client_message_payload["supplierEmail"],
            "provider@example.com"
        );
        assert_eq!(client_message_payload["clientLabel"], "client-v24-route");
        assert_eq!(client_message_payload["dailyRateMinor"], 2400);
        assert_eq!(
            client_message_payload["paymentMethodKinds"],
            serde_json::json!(["crypto"])
        );
        assert_eq!(
            client_message_payload["contacts"][0]["handle"],
            "public-provider-contact"
        );
        for private_field in [
            "clientUserId",
            "clientOwnerEmail",
            "providerUserId",
            "actorUserId",
            "actorEmail",
            "paymentMethods",
        ] {
            assert!(client_message_payload.get(private_field).is_none());
        }
        let released_at: String = conn
            .query_row(
                "SELECT released_at FROM share_market_subscriptions
                 WHERE id = 'subscription-v24'",
                [],
                |row| row.get(0),
            )
            .expect("read backfilled terminal timestamp");
        assert_eq!(released_at, "2026-02-01T00:00:00Z");
    }

    #[test]
    fn migration_25_backfills_historical_invoice_credits_into_accrual_net_amounts() {
        let conn = memory_connection();
        install_schema_through(&conn, 24);
        conn.execute_batch(
            "INSERT INTO market_credit_accounts (
                id, buyer_user_id, buyer_email, supplier_user_id, supplier_email,
                currency, status, credit_kind, credit_limit_minor, credit_source,
                credit_revision, created_at, updated_at
             ) VALUES ('credit-backfill-account', 'credit-backfill-buyer', 'buyer@example.com',
                       'credit-backfill-supplier', 'supplier@example.com', 'USD', 'active',
                       'limited', 1000, 'counterparty', 1,
                       '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO market_service_contracts (
                id, account_id, product_kind, product_ref, service_ref, service_label,
                buyer_user_id, buyer_email, supplier_user_id, supplier_email, currency,
                daily_rate_minor, offer_revision, status, trial_seconds_remaining,
                last_evaluated_at, activated_at, created_at, updated_at
             ) VALUES ('credit-backfill-contract', 'credit-backfill-account', 'client_host',
                       'credit-backfill-product', 'credit-backfill-service', 'Backfill service',
                       'credit-backfill-buyer', 'buyer@example.com', 'credit-backfill-supplier',
                       'supplier@example.com', 'USD', 100, 1, 'active', 0,
                       '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                       '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO market_invoices (
                id, account_id, sequence, amount_minor, amount_cny_minor,
                usd_cny_rate_micros, amount_units, currency, payment_methods_json,
                payment_contacts_json, payment_profile_updated_at, status, due_at,
                deadline_at, opened_at
             ) VALUES ('credit-backfill-invoice', 'credit-backfill-account', 1, 1, 7,
                       7000000, 300, 'USD', '[]', '[]', '2026-01-01T00:00:00Z',
                       'paid', '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z',
                       '2026-01-01T00:00:00Z');
             INSERT INTO market_service_intervals (
                id, contract_id, state, observation_reason, started_at, ended_at,
                elapsed_seconds, trial_seconds, billable_seconds, amount_units,
                invoice_id, created_at, updated_at
             ) VALUES
                ('credit-backfill-interval-1', 'credit-backfill-contract', 'healthy', 'test',
                 '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 60, 0, 60, 100,
                 'credit-backfill-invoice', '2026-01-01T00:01:00Z', '2026-01-01T00:01:00Z'),
                ('credit-backfill-interval-2', 'credit-backfill-contract', 'healthy', 'test',
                 '2026-01-01T00:01:00Z', '2026-01-01T00:02:00Z', 60, 0, 60, 200,
                 'credit-backfill-invoice', '2026-01-01T00:02:00Z', '2026-01-01T00:02:00Z');
             INSERT INTO market_accrual_entries (
                id, account_id, contract_id, interval_id, currency, daily_rate_minor,
                billable_seconds, amount_units, status, invoice_id, created_at, updated_at
             ) VALUES
                ('credit-backfill-accrual-1', 'credit-backfill-account',
                 'credit-backfill-contract', 'credit-backfill-interval-1', 'USD', 100, 60,
                 100, 'invoiced', 'credit-backfill-invoice', '2026-01-01T00:01:00Z',
                 '2026-01-01T00:01:00Z'),
                ('credit-backfill-accrual-2', 'credit-backfill-account',
                 'credit-backfill-contract', 'credit-backfill-interval-2', 'USD', 100, 60,
                 200, 'invoiced', 'credit-backfill-invoice', '2026-01-01T00:02:00Z',
                 '2026-01-01T00:02:00Z');
             INSERT INTO market_credit_notes (
                id, account_id, invoice_id, kind, amount_units, amount_minor, currency,
                reason, status, created_by_user_id, created_by_email, created_at
             ) VALUES
                ('credit-backfill-service-note', 'credit-backfill-account',
                 'credit-backfill-invoice', 'service_credit', 150, 1, 'USD', 'service credit',
                 'applied', 'credit-backfill-supplier', 'supplier@example.com',
                 '2026-01-01T00:03:00Z'),
                ('credit-backfill-refund-note', 'credit-backfill-account',
                 'credit-backfill-invoice', 'external_refund', 75, 1, 'USD', 'external refund',
                 'recorded', 'credit-backfill-supplier', 'supplier@example.com',
                 '2026-01-01T00:04:00Z');",
        )
        .expect("seed historical invoice credits");

        apply(&conn).expect("upgrade historical invoice credits");

        let credits = conn
            .prepare(
                "SELECT id, credited_units FROM market_accrual_entries
                 WHERE invoice_id = 'credit-backfill-invoice' ORDER BY created_at, id",
            )
            .expect("prepare migrated accrual credits")
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .expect("query migrated accrual credits")
            .collect::<Result<Vec<_>, _>>()
            .expect("read migrated accrual credits");
        assert_eq!(
            credits,
            vec![
                ("credit-backfill-accrual-1".into(), 100),
                ("credit-backfill-accrual-2".into(), 125),
            ]
        );
    }

    #[test]
    fn migration_20_canonicalizes_free_access_and_enforces_market_exclusion() {
        let conn = memory_connection();
        // Seed at v18 so `apply` runs the v19 archive finalizer before v20.
        // The raw test helper intentionally does not execute Rust post-hooks.
        install_schema_through(&conn, 18);
        conn.execute_batch(
            "INSERT INTO shares
                (share_id, capacity_pool_id, installation_id, share_name, owner_email,
                 for_sale, app_type, token_limit, parallel_limit, tokens_used,
                 requests_count, share_status, created_at, expires_at, updated_at)
             VALUES
                ('share-free', 'pool-free', 'installation-free', 'Free', 'owner@example.com',
                 'Free', 'codex', -1, -1, 0, 0, 'active', 'now', '2099-01-01T00:00:00Z', 'now'),
                ('share-listed', 'pool-listed', 'installation-listed', 'Listed', 'owner@example.com',
                 'Free', 'codex', -1, -1, 0, 0, 'active', 'now', '2099-01-01T00:00:00Z', 'now'),
                ('share-subscribed', 'pool-subscribed', 'installation-subscribed', 'Subscribed', 'owner@example.com',
                 'Free', 'codex', -1, -1, 0, 0, 'active', 'now', '2099-01-01T00:00:00Z', 'now'),
                ('share-yes', 'pool-yes', 'installation-yes', 'Legacy yes', 'owner@example.com',
                 'Yes', 'codex', -1, -1, 0, 0, 'active', 'now', '2099-01-01T00:00:00Z', 'now');
             INSERT INTO share_market_listings
                (id, share_id, installation_id, owner_user_id, owner_email, status,
                 deleted_at, created_at, updated_at)
             VALUES
                ('listing-existing', 'share-listed', 'installation-listed', 'owner-user',
                 'owner@example.com', 'active', NULL, 'now', 'now'),
                ('listing-subscribed', 'share-subscribed', 'installation-subscribed', 'owner-user',
                 'owner@example.com', 'closed', NULL, 'now', 'now');
             INSERT INTO share_market_seats
                (id, listing_id, position, status, token_period_json, offer_revision,
                 created_at, updated_at)
             VALUES ('seat-subscribed', 'listing-subscribed', 1, 'occupied', '{}', 1,
                     'now', 'now');
             INSERT INTO share_market_subscriptions
                (id, seat_id, listing_id, share_id, installation_id, entitlement_id,
                 owner_user_id, owner_email, renter_user_id, renter_email, status,
                 token_period_json, offer_revision, created_at, updated_at)
             VALUES ('subscription-active', 'seat-subscribed', 'listing-subscribed',
                     'share-subscribed', 'installation-subscribed', 'entitlement-active',
                     'owner-user', 'owner@example.com', 'renter-active',
                     'renter@example.com', 'active', '{}', 1, 'now', 'now');",
        )
        .expect("seed access-policy migration");

        apply(&conn).expect("apply access-policy migration");

        let policies = conn
            .prepare(
                "SELECT share_id, free_access, share_access_policy_version, for_sale
                   FROM shares ORDER BY share_id",
            )
            .expect("prepare migrated policies")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .expect("query migrated policies")
            .collect::<Result<Vec<_>, _>>()
            .expect("read migrated policies");
        assert_eq!(
            policies,
            vec![
                ("share-free".into(), 1, 1, "No".into()),
                ("share-listed".into(), 0, 1, "No".into()),
                ("share-subscribed".into(), 0, 1, "No".into()),
                ("share-yes".into(), 0, 1, "No".into()),
            ]
        );

        assert!(
            conn.execute(
                "UPDATE shares SET free_access = 1 WHERE share_id = 'share-listed'",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE shares SET free_access = 1 WHERE share_id = 'share-subscribed'",
                [],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "INSERT INTO share_market_listings
                    (id, share_id, installation_id, owner_user_id, owner_email, status,
                     deleted_at, created_at, updated_at)
                 VALUES ('listing-free', 'share-free', 'installation-free', 'owner-user',
                         'owner@example.com', 'active', NULL, 'now', 'now')",
                [],
            )
            .is_err()
        );

        conn.execute_batch(
            "INSERT INTO share_market_listings
                (id, share_id, installation_id, owner_user_id, owner_email, status,
                 deleted_at, created_at, updated_at)
             VALUES ('listing-closed', 'share-free', 'installation-free', 'owner-user',
                     'owner@example.com', 'closed', NULL, 'now', 'now');
             INSERT INTO share_market_seats
                (id, listing_id, position, status, token_period_json, offer_revision,
                 created_at, updated_at)
             VALUES ('seat-released', 'listing-closed', 1, 'available', '{}', 1, 'now', 'now');
             INSERT INTO share_market_subscriptions
                (id, seat_id, listing_id, share_id, installation_id, entitlement_id,
                 owner_user_id, owner_email, renter_user_id, renter_email, status,
                 token_period_json, offer_revision, created_at, updated_at)
             VALUES ('subscription-released', 'seat-released', 'listing-closed', 'share-free',
                     'installation-free', 'entitlement-released', 'owner-user',
                     'owner@example.com', 'renter-user', 'renter@example.com', 'released',
                     '{}', 1, 'now', 'now');",
        )
        .expect("seed inactive market rows for public free Share");
        assert!(
            conn.execute(
                "UPDATE share_market_listings SET status = 'active' WHERE id = 'listing-closed'",
                [],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE share_market_subscriptions SET status = 'active'
                  WHERE id = 'subscription-released'",
                [],
            )
            .is_err()
        );
    }

    #[test]
    fn migration_19_archive_is_verified_before_21_physical_retirement() {
        let conn = memory_connection();
        install_schema_through(&conn, 18);
        conn.execute_batch(
            "INSERT INTO router_markets
                (id, display_name, email, subdomain, public_base_url,
                 created_at, updated_at, last_seen_at)
             VALUES ('legacy-market', 'Legacy Market', 'legacy@example.com', 'legacy',
                     'https://legacy.example.com', 'now', 'now', 'now');
             INSERT INTO public_hosts
                (label, route_id, kind, subject_id, target_lane_id, lifecycle,
                 revision, created_at, updated_at)
             VALUES ('legacy-market-host', 'market:legacy', 'market', 'legacy-market',
                     'legacy-market', 'active', 1, 'now', 'now');
             INSERT INTO market_notification_emails
                (id, market_email, kind, to_email, locale, payload_json, status, created_at)
             VALUES ('legacy-mail', 'legacy@example.com', 'topup_paid', 'buyer@example.com',
                     'en', '{}', 'sent', 'now');
             INSERT INTO market_request_logs
                (request_id, market_id, market_email, market_subdomain, status,
                 created_at, synced_at)
             VALUES ('req_legacy_archive', 'legacy-market', 'legacy@example.com', 'legacy',
                     'settled', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
             INSERT INTO market_disabled_shares
                (market_email, share_id, disabled_by_email, created_at, updated_at)
             VALUES ('legacy@example.com', 'share-legacy', 'legacy@example.com', 'now', 'now');
             INSERT INTO market_share_model_failure_state
                (market_email, share_id, app_type, requested_model, last_status,
                 last_checked_at, updated_at)
             VALUES ('legacy@example.com', 'share-legacy', 'codex', 'gpt-5', 'failed', 1, 1);
             INSERT INTO market_share_runtime_states
                (market_email, share_id, scope, kind, created_at, updated_at)
             VALUES ('legacy@example.com', 'share-legacy', 'share', 'cooldown', 'now', 'now');",
        )
        .expect("seed legacy Token Market rows");

        apply(&conn).expect("apply archive and physical retirement migrations");

        for table in [
            "router_markets",
            "market_notification_emails",
            "market_request_logs",
            "market_disabled_shares",
            "market_share_model_failure_state",
            "market_share_runtime_states",
            "legacy_token_market_archive_manifest",
            "legacy_token_market_router_markets",
            "legacy_token_market_public_hosts",
            "legacy_token_market_notification_emails",
            "legacy_token_market_request_logs",
            "legacy_token_market_disabled_shares",
            "legacy_token_market_share_model_failure_state",
            "legacy_token_market_share_runtime_states",
        ] {
            let table_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .unwrap_or_else(|error| panic!("inspect retired table {table}: {error}"));
            assert_eq!(table_count, 0, "retired table still exists: {table}");
        }

        let live_market_hosts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM public_hosts WHERE kind = 'market'",
                [],
                |row| row.get(0),
            )
            .expect("count live market hosts");
        assert_eq!(live_market_hosts, 0);
        let live_host_kind_rejected = conn.execute(
            "INSERT INTO public_hosts
                (label, route_id, kind, subject_id, target_lane_id, lifecycle,
                 revision, created_at, updated_at)
             VALUES ('new-market-host', 'market:new', 'market', 'new-market',
                     'new-market', 'active', 1, 'now', 'now')",
            [],
        );
        assert!(live_host_kind_rejected.is_err());

        let legacy_observations: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM capacity_request_observations
                 WHERE request_id = 'req_legacy_archive'
                    OR source_kind = 'legacy_token_market'",
                [],
                |row| row.get(0),
            )
            .expect("count retired observations");
        assert_eq!(legacy_observations, 0);

        conn.execute(
            "INSERT INTO gateway_request_observations (
                request_id, gateway_id, request_agent, requested_model,
                actual_model, actual_model_source, status, created_at, observed_at
             ) VALUES ('req_gateway_identity', 'gateway-identity', 'codex',
                       'gpt-5', 'gpt-5', 'official', 'success', 'now', 'now')",
            [],
        )
        .expect("insert Gateway observation fixture");
        let projected_identity: (Option<String>, Option<String>, String) = conn
            .query_row(
                "SELECT gateway_id, user_email, source_kind
                   FROM capacity_request_observations
                  WHERE request_id = 'req_gateway_identity'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read Gateway compatibility view");
        assert_eq!(
            projected_identity,
            (Some("gateway-identity".into()), None, "gateway".into())
        );

        let before: (i64, i64, String) = conn
            .query_row(
                "SELECT source_rows, retained_rows, retired_at
                   FROM data_retirement_audit
                  WHERE component = 'router-local-capacity-market-v1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read retirement receipt");
        assert_eq!(before.0, 1);
        assert_eq!(before.1, 0);

        apply(&conn).expect("repeat retirement migration");
        let after: (i64, i64, String) = conn
            .query_row(
                "SELECT source_rows, retained_rows, retired_at
                   FROM data_retirement_audit
                  WHERE component = 'router-local-capacity-market-v1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read repeated retirement receipt");
        assert_eq!(before, after);
    }

    #[test]
    fn migration_21_retains_only_identified_share_users() {
        let conn = memory_connection();
        install_schema_through(&conn, 18);
        conn.execute_batch(
            "INSERT INTO shares (
                 share_id, capacity_pool_id, installation_id, share_name, owner_email,
                 app_type, token_limit, parallel_limit, tokens_used, requests_count,
                 share_status, created_at, expires_at, user_grants_json, updated_at
             ) VALUES (
                 'share-known', 'pool-known', 'installation-known', 'Known Share',
                 'owner@example.com', 'codex', -1, 3, 0, 0, 'active', 'now',
                 '9999-12-31T23:59:59Z',
                 '{\"grant@example.com\":{\"email\":\"grant@example.com\",\"role\":\"shareto\",\"active\":false}}',
                 'now'
             );
             INSERT INTO users (id, email_normalized, status, created_at, last_login_at)
             VALUES ('known-user', 'known@example.com', 'active', 'now', 'now');
             INSERT INTO market_request_logs (
                 request_id, market_id, market_email, market_subdomain, user_email,
                 share_id, model, request_agent, status, created_at, synced_at
             ) VALUES
                 ('req-owner', 'legacy-market', 'legacy@example.com', 'legacy',
                  'owner@example.com', 'share-known', 'gpt-5', 'codex', 'settled',
                  '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
                 ('req-grant', 'legacy-market', 'legacy@example.com', 'legacy',
                  'grant@example.com', 'share-known', 'gpt-5', 'codex', 'settled',
                  '2026-01-01T00:00:01Z', '2026-01-01T00:00:01Z'),
                 ('req-known', 'legacy-market', 'legacy@example.com', 'legacy',
                  'known@example.com', 'share-known', 'gpt-5', 'codex', 'settled',
                  '2026-01-01T00:00:02Z', '2026-01-01T00:00:02Z'),
                 ('req-unknown', 'legacy-market', 'legacy@example.com', 'legacy',
                  'unknown@example.com', 'share-known', 'gpt-5', 'codex', 'settled',
                  '2026-01-01T00:00:03Z', '2026-01-01T00:00:03Z');",
        )
        .expect("seed identified and unidentified legacy observations");

        apply(&conn).expect("apply identification-aware retirement migration");

        let retained: Vec<(String, Option<String>)> = conn
            .prepare(
                "SELECT request_id, user_email
                   FROM share_request_logs
                  ORDER BY request_id",
            )
            .expect("prepare retained Share observations")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query retained Share observations")
            .collect::<Result<Vec<_>, _>>()
            .expect("read retained Share observations");
        assert_eq!(
            retained,
            vec![
                (
                    "req-grant".to_string(),
                    Some("grant@example.com".to_string())
                ),
                (
                    "req-known".to_string(),
                    Some("known@example.com".to_string())
                ),
                (
                    "req-owner".to_string(),
                    Some("owner@example.com".to_string())
                ),
            ]
        );

        let receipt: (i64, i64) = conn
            .query_row(
                "SELECT source_rows, retained_rows
                   FROM data_retirement_audit
                  WHERE component = 'router-local-capacity-market-v1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read identification-aware retirement receipt");
        assert_eq!(receipt, (4, 3));
    }

    #[test]
    fn migration_21_rejects_a_tampered_migration_19_archive() {
        let conn = memory_connection();
        install_schema_through(&conn, 18);
        conn.execute_batch(
            "INSERT INTO router_markets
                (id, display_name, email, subdomain, public_base_url,
                 created_at, updated_at, last_seen_at)
             VALUES ('legacy-market', 'Legacy Market', 'legacy@example.com', 'legacy',
                     'https://legacy.example.com', 'now', 'now', 'now');",
        )
        .expect("seed legacy Token Market row");

        let (version, migration_19) = MIGRATIONS
            .iter()
            .copied()
            .find(|(version, _)| *version == LEGACY_TOKEN_MARKET_RETIREMENT_VERSION)
            .expect("migration 19 exists");
        conn.execute_batch(migration_19)
            .expect("apply migration 19 SQL");
        populate_legacy_token_market_archive_checksums(&conn)
            .expect("finalize migration 19 archive checksums");
        ensure_legacy_token_market_archive_is_read_only(&conn).expect("lock migration 19 archive");
        conn.execute(
            "INSERT INTO schema_migrations (version, checksum, applied_at)
             VALUES (?1, ?2, 'test')",
            params![version, migration_checksum(migration_19)],
        )
        .expect("record migration 19");
        check_compatibility(&conn).expect("untampered migration 19 archive is compatible");

        conn.execute_batch(
            "DROP TRIGGER legacy_token_market_archive_router_markets_read_only_update;
             UPDATE legacy_token_market_router_markets SET display_name = 'tampered';",
        )
        .expect("simulate an operator bypassing an archive trigger");
        let error = apply(&conn).expect_err("migration 21 must reject a corrupted archive");
        assert!(error.to_string().contains("archive verification failed"));

        let migration_21_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                params![LEGACY_TOKEN_MARKET_PHYSICAL_RETIREMENT_VERSION],
                |row| row.get(0),
            )
            .expect("inspect migration 21 history");
        assert_eq!(migration_21_rows, 0);
        let archived_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM legacy_token_market_router_markets",
                [],
                |row| row.get(0),
            )
            .expect("archive remains available after rejected retirement");
        assert_eq!(archived_rows, 1);
    }
}
