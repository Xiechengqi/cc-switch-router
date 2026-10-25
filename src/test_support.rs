//! Isolated business-test databases, copied from a once-migrated template.
//!
//! Migration/startup tests must continue using `schema::apply` directly. The
//! template is built by that same production path, not by a second schema or a
//! checked-in database. No template survives the test process or a source change.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::db::{Connection, params};
use crate::error::AppError;

pub(crate) struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Result<Self, AppError> {
        let path =
            std::env::temp_dir().join(format!("cc-switch-router-test-db-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).map_err(io_error)?;
        Ok(Self(path))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        // Only the unique directory created by `new`, never a caller's path.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn io_error(error: std::io::Error) -> AppError {
    AppError::Internal(format!("test database snapshot I/O failed: {error}"))
}

fn database_error(error: crate::db::Error) -> AppError {
    AppError::Internal(format!("test database snapshot failed: {error}"))
}

fn template() -> Result<&'static [u8], AppError> {
    static TEMPLATE: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            (|| -> Result<Vec<u8>, AppError> {
                let directory = TestDirectory::new()?;
                let snapshot = directory.0.join("template.sqlite");
                let conn = Connection::open_in_memory().map_err(database_error)?;
                conn.pragma_update(None, "foreign_keys", "ON")
                    .map_err(database_error)?;
                crate::schema::apply(&conn)?;
                conn.execute(
                    "VACUUM main INTO ?1",
                    params![snapshot.to_string_lossy().to_string()],
                )
                .map_err(database_error)?;
                fs::read(snapshot).map_err(io_error)
            })()
            .map_err(|error| error.to_string())
        })
        .as_deref()
        .map_err(|error| AppError::Internal(error.to_string()))
}

/// A disposable replacement for business-test in-memory databases. SQLite's
/// physical snapshot preserves seed rows, migration history, indexes, views and
/// triggers without replaying migrations hundreds of times. Every call owns a
/// separate file; MEMORY journaling/OFF syncing match the old non-durable
/// in-memory fixtures. Connection-local foreign-key settings remain caller-owned.
pub(crate) fn database() -> Result<Connection, AppError> {
    let directory = TestDirectory::new()?;
    let conn = database_at(directory.0.join("business.sqlite"))?.with_test_directory(directory);
    conn.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF;")
        .map_err(database_error)?;
    Ok(conn)
}

/// Seed a new file for persistence/multi-connection tests. Never overwrite an
/// existing database, change its durability PRAGMAs, or own the caller's directory.
pub(crate) fn database_at(path: impl AsRef<Path>) -> Result<Connection, AppError> {
    let bytes = template()?;
    let path = path.as_ref();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    drop(file);
    Connection::open(path).map_err(database_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn template_matches_real_schema_and_keeps_seed_data_and_financial_guards() {
        let real = Connection::open_in_memory().unwrap();
        real.pragma_update(None, "foreign_keys", "ON").unwrap();
        crate::schema::apply(&real).unwrap();
        let copy = database().unwrap();
        for sql in [
            "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'",
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index'",
            "SELECT COUNT(*) FROM sqlite_master WHERE type='view'",
            "SELECT MAX(version) FROM schema_migrations",
            "SELECT COUNT(*) FROM schema_migrations",
            "SELECT COUNT(*) FROM market_c2c_policy",
            "SELECT COUNT(*) FROM market_wallet_policy",
            "SELECT COUNT(*) FROM market_commission_policies",
        ] {
            assert_eq!(scalar(&real, sql), scalar(&copy, sql), "{sql}");
        }
        let schema = |conn: &Connection| {
            conn.prepare("SELECT type,name,tbl_name,COALESCE(sql,'') FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name")
                .unwrap().query_map([], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?)))
                .unwrap().collect::<crate::db::Result<Vec<_>>>().unwrap()
        };
        assert_eq!(schema(&real), schema(&copy));
        copy.pragma_update(None, "foreign_keys", "ON").unwrap();
        assert!(copy.execute("INSERT INTO market_c2c_events(id,order_id,actor_id,action,created_at) VALUES('bad','missing','actor','test','t')", []).is_err());
        crate::market_wallet::credit_verified_topup_tx(
            &copy, "buyer", "topup", 100, "receipt", "t",
        )
        .unwrap();
        assert!(
            copy.execute("UPDATE market_journal_postings SET signed_units=0", [])
                .is_err()
        );
        assert_eq!(
            scalar(&copy, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
            0
        );
    }

    #[test]
    fn copies_are_independent_and_statements_keep_their_directory_alive() {
        let first = database().unwrap();
        let second = database().unwrap();
        first
            .execute("UPDATE market_c2c_policy SET enabled=1", [])
            .unwrap();
        assert_eq!(scalar(&second, "SELECT enabled FROM market_c2c_policy"), 0);
        let path: String = first
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name='main'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut statement = first
            .prepare("SELECT enabled FROM market_c2c_policy")
            .unwrap();
        drop(first);
        assert!(Path::new(&path).exists());
        assert_eq!(
            statement
                .query_map([], |row| row.get::<_, i64>(0))
                .unwrap()
                .next()
                .unwrap()
                .unwrap(),
            1
        );
        drop(statement);
        assert!(!Path::new(&path).parent().unwrap().exists());
    }

    #[test]
    fn persistent_copies_survive_reopen_and_never_overwrite_existing_data() {
        let directory = TestDirectory::new().unwrap();
        let path = directory.0.join("persistent.sqlite");
        let conn = database_at(&path).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        conn.execute("UPDATE market_c2c_policy SET enabled=1", [])
            .unwrap();
        drop(conn);
        assert!(database_at(&path).is_err());
        let reopened = Connection::open(&path).unwrap();
        crate::schema::apply(&reopened).unwrap();
        assert_eq!(
            scalar(&reopened, "SELECT enabled FROM market_c2c_policy"),
            1
        );
        drop(reopened);
    }

    #[test]
    fn transaction_statements_keep_their_directory_until_the_last_clone_drops() {
        let conn = database().unwrap();
        let path: String = conn
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name='main'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let transaction = conn.transaction().unwrap();
        let mut statement = transaction
            .prepare("SELECT enabled FROM market_c2c_policy")
            .unwrap();
        transaction.commit().unwrap();
        drop(conn);
        assert!(Path::new(&path).exists());
        assert_eq!(
            statement
                .query_map([], |row| row.get::<_, i64>(0))
                .unwrap()
                .next()
                .unwrap()
                .unwrap(),
            0
        );
        drop(statement);
        assert!(!Path::new(&path).parent().unwrap().exists());
    }

    #[test]
    fn concurrent_copies_do_not_share_business_state() {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    let conn = database().unwrap();
                    conn.execute("UPDATE market_c2c_policy SET version=123", [])
                        .unwrap();
                    assert_eq!(scalar(&conn, "SELECT version FROM market_c2c_policy"), 123);
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            scalar(
                &database().unwrap(),
                "SELECT version FROM market_c2c_policy"
            ),
            1
        );
    }
}
