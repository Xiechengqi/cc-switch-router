use std::error::Error as StdError;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

pub use libsql::TransactionBehavior;
use libsql::params::Params as LibsqlParams;
use thiserror::Error;

pub mod types {
    pub use libsql::Value;

    #[allow(dead_code)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Type {
        Null,
        Integer,
        Real,
        Text,
        Blob,
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Database(libsql::Error),
    #[error("database unavailable: {0}")]
    Unavailable(String),
    #[error("query returned no rows")]
    QueryReturnedNoRows,
    #[error("column {0} has an incompatible value")]
    InvalidColumnType(usize),
    #[error("column {0} conversion failed: {2}")]
    FromSqlConversionFailure(
        usize,
        types::Type,
        #[source] Box<dyn StdError + Send + Sync>,
    ),
    #[error("parameter conversion failed: {0}")]
    ToSqlConversionFailure(String),
    #[error("database runtime failed: {0}")]
    Runtime(String),
}

impl From<libsql::Error> for Error {
    fn from(error: libsql::Error) -> Self {
        Self::Database(error)
    }
}

impl Error {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

struct DatabaseRuntime {
    handle: tokio::runtime::Handle,
}

impl DatabaseRuntime {
    fn start() -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("cc-switch-router-db".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("cc-switch-router-db-worker")
                    .enable_all()
                    .build()
                    .expect("build database runtime");
                sender
                    .send(runtime.handle().clone())
                    .expect("publish database runtime handle");
                runtime.block_on(std::future::pending::<()>());
            })
            .expect("spawn database runtime thread");
        Self {
            handle: receiver.recv().expect("receive database runtime handle"),
        }
    }

    fn run<F, T>(&self, future: F) -> Result<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.handle.spawn(async move {
            let _ = sender.send(future.await);
        });
        receive_without_blocking_tokio(receiver)
            .map_err(|error| Error::Runtime(format!("database task stopped: {error}")))?
    }
}

fn receive_without_blocking_tokio<T>(
    receiver: Receiver<T>,
) -> std::result::Result<T, mpsc::RecvError> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle)
            if matches!(
                handle.runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            ) =>
        {
            tokio::task::block_in_place(|| receiver.recv())
        }
        _ => receiver.recv(),
    }
}

fn database_runtime() -> &'static DatabaseRuntime {
    static RUNTIME: OnceLock<DatabaseRuntime> = OnceLock::new();
    RUNTIME.get_or_init(DatabaseRuntime::start)
}

/// Selects SQLite's serialized threading mode before any direct
/// `libsql-rusqlite` connection is opened in this process.
///
/// libsql performs this configuration during its first local database build.
/// Metrics and alerting use the compatible synchronous API directly, so they
/// must cross the same initialization barrier before opening their databases.
pub fn initialize_sqlite_runtime() -> Result<()> {
    static INITIALIZED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    match INITIALIZED.get_or_init(|| {
        database_runtime()
            .run(async move {
                let database = libsql::Builder::new_local(":memory:").build().await?;
                drop(database);
                Ok(())
            })
            .map_err(|error| error.to_string())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(Error::Runtime(format!(
            "initialize SQLite threading mode failed: {error}"
        ))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMode {
    Local,
    TursoReplica,
}

impl ConnectionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::TursoReplica => "turso",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DatabaseHealthSnapshot {
    pub mode: ConnectionMode,
    pub available: bool,
    pub last_attempt_at_ms: Option<i64>,
    pub last_success_at_ms: Option<i64>,
    pub last_failure_at_ms: Option<i64>,
    pub consecutive_failures: u64,
    pub last_frames_synced: u64,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct DatabaseHealthHandle {
    mode: ConnectionMode,
    tracker: Arc<DatabaseHealthTracker>,
}

impl DatabaseHealthHandle {
    pub fn snapshot(&self) -> DatabaseHealthSnapshot {
        self.tracker.snapshot(self.mode)
    }
}

#[derive(Clone)]
pub struct DatabaseSyncHandle {
    database: Arc<libsql::Database>,
    mode: ConnectionMode,
    health: Arc<DatabaseHealthTracker>,
}

impl DatabaseSyncHandle {
    pub fn sync(&self) -> Result<usize> {
        if self.mode == ConnectionMode::Local {
            self.health.record_success();
            return Ok(0);
        }
        let database = self.database.clone();
        let result = database_runtime()
            .run(async move { Ok(database.sync().await?.frames_synced()) })
            .map_err(|error| classify_error(self.mode, error));
        match &result {
            Ok(frames_synced) => self.health.record_sync_success(*frames_synced),
            Err(error) => self.health.record_failure(error),
        }
        result
    }
}

struct DatabaseHealthTracker {
    available: AtomicBool,
    last_attempt_at_ms: AtomicI64,
    last_success_at_ms: AtomicI64,
    last_failure_at_ms: AtomicI64,
    consecutive_failures: AtomicU64,
    last_frames_synced: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl DatabaseHealthTracker {
    fn available() -> Self {
        let now = unix_timestamp_ms();
        Self {
            available: AtomicBool::new(true),
            last_attempt_at_ms: AtomicI64::new(now),
            last_success_at_ms: AtomicI64::new(now),
            last_failure_at_ms: AtomicI64::new(0),
            consecutive_failures: AtomicU64::new(0),
            last_frames_synced: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }

    fn record_success(&self) {
        let now = unix_timestamp_ms();
        self.available.store(true, Ordering::Release);
        self.last_attempt_at_ms.store(now, Ordering::Release);
        self.last_success_at_ms.store(now, Ordering::Release);
        self.consecutive_failures.store(0, Ordering::Release);
        *self
            .last_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }

    fn record_sync_success(&self, frames_synced: usize) {
        self.record_success();
        self.last_frames_synced.store(
            u64::try_from(frames_synced).unwrap_or(u64::MAX),
            Ordering::Release,
        );
    }

    fn record_failure(&self, error: &Error) {
        let now = unix_timestamp_ms();
        self.available.store(false, Ordering::Release);
        self.last_attempt_at_ms.store(now, Ordering::Release);
        self.last_failure_at_ms.store(now, Ordering::Release);
        self.consecutive_failures.fetch_add(1, Ordering::AcqRel);
        *self
            .last_error
            .lock()
            .unwrap_or_else(|state| state.into_inner()) = Some(error.to_string());
    }

    fn snapshot(&self, mode: ConnectionMode) -> DatabaseHealthSnapshot {
        DatabaseHealthSnapshot {
            mode,
            available: self.available.load(Ordering::Acquire),
            last_attempt_at_ms: nonzero_timestamp(self.last_attempt_at_ms.load(Ordering::Acquire)),
            last_success_at_ms: nonzero_timestamp(self.last_success_at_ms.load(Ordering::Acquire)),
            last_failure_at_ms: nonzero_timestamp(self.last_failure_at_ms.load(Ordering::Acquire)),
            consecutive_failures: self.consecutive_failures.load(Ordering::Acquire),
            last_frames_synced: self.last_frames_synced.load(Ordering::Acquire),
            last_error: self
                .last_error
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        }
    }
}

fn unix_timestamp_ms() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn nonzero_timestamp(value: i64) -> Option<i64> {
    (value > 0).then_some(value)
}

fn classify_error(mode: ConnectionMode, error: Error) -> Error {
    if mode != ConnectionMode::TursoReplica {
        return error;
    }
    match error {
        Error::Database(database_error) if is_remote_unavailable(&database_error) => {
            Error::Unavailable(database_error.to_string())
        }
        other => other,
    }
}

fn is_remote_unavailable(error: &libsql::Error) -> bool {
    matches!(
        error,
        libsql::Error::ConnectionFailed(_)
            | libsql::Error::Hrana(_)
            | libsql::Error::WriteDelegation(_)
            | libsql::Error::Replication(_)
            | libsql::Error::Sync(_)
    )
}

#[derive(Clone)]
pub struct Connection {
    inner: libsql::Connection,
    database: Arc<libsql::Database>,
    mode: ConnectionMode,
    health: Arc<DatabaseHealthTracker>,
}

impl fmt::Debug for Connection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Connection")
            .field("mode", &self.mode)
            .field("health", &self.health.snapshot(self.mode))
            .finish_non_exhaustive()
    }
}

impl Connection {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        database_runtime().run(async move {
            let database = libsql::Builder::new_local(path).build().await?;
            Self::from_database(database, ConnectionMode::Local, 0)
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        database_runtime().run(async move {
            let database = libsql::Builder::new_local(":memory:").build().await?;
            Self::from_database(database, ConnectionMode::Local, 0)
        })
    }

    pub fn open_remote_replica(
        path: impl AsRef<Path>,
        url: String,
        auth_token: String,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        database_runtime()
            .run(async move {
                let database = libsql::Builder::new_remote_replica(path, url, auth_token)
                    .read_your_writes(true)
                    .build()
                    .await?;
                let replicated = database.sync().await?;
                Self::from_database(
                    database,
                    ConnectionMode::TursoReplica,
                    replicated.frames_synced(),
                )
            })
            .map_err(|error| classify_error(ConnectionMode::TursoReplica, error))
    }

    fn from_database(
        database: libsql::Database,
        mode: ConnectionMode,
        frames_synced: usize,
    ) -> Result<Self> {
        let database = Arc::new(database);
        let inner = database.connect()?;
        let connection = Self {
            inner,
            database,
            mode,
            health: Arc::new(DatabaseHealthTracker::available()),
        };
        connection.health.record_sync_success(frames_synced);
        Ok(connection)
    }

    pub fn health_handle(&self) -> DatabaseHealthHandle {
        DatabaseHealthHandle {
            mode: self.mode,
            tracker: self.health.clone(),
        }
    }

    pub fn sync_handle(&self) -> DatabaseSyncHandle {
        DatabaseSyncHandle {
            database: self.database.clone(),
            mode: self.mode,
            health: self.health.clone(),
        }
    }

    pub fn execute<P>(&self, sql: &str, params: P) -> Result<usize>
    where
        P: Params,
    {
        let params = into_libsql_params(params)?;
        let sql = sql.to_string();
        let connection = self.inner.clone();
        let result = database_runtime().run(async move {
            let changed = connection.execute(&sql, params).await?;
            usize::try_from(changed)
                .map_err(|_| Error::Runtime("affected row count overflow".into()))
        });
        self.track_remote_operation(result)
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let sql = sql.to_string();
        let connection = self.inner.clone();
        let result = database_runtime().run(async move {
            connection.execute_batch(&sql).await?;
            Ok(())
        });
        self.track_remote_operation(result)
    }

    pub fn prepare(&self, sql: &str) -> Result<Statement> {
        Ok(Statement {
            connection: self.clone(),
            sql: sql.to_string(),
        })
    }

    pub fn query_row<P, F, T>(&self, sql: &str, params: P, mapper: F) -> Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        let row = self
            .query_one_owned(sql, params)?
            .ok_or(Error::QueryReturnedNoRows)?;
        mapper(&row)
    }

    fn query_one_owned<P>(&self, sql: &str, params: P) -> Result<Option<Row<'static>>>
    where
        P: Params,
    {
        let params = into_libsql_params(params)?;
        let sql = sql.to_string();
        let connection = self.inner.clone();
        let result = database_runtime().run(async move {
            let mut rows = connection.query(&sql, params).await?;
            let column_count = usize::try_from(rows.column_count())
                .map_err(|_| Error::Runtime("negative database column count".into()))?;
            let Some(row) = rows.next().await? else {
                return Ok(None);
            };
            let mut values = Vec::with_capacity(column_count);
            for index in 0..column_count {
                values.push(row.get_value(index as i32)?);
            }
            Ok(Some(Row {
                values,
                marker: PhantomData,
            }))
        });
        self.track_remote_query(result)
    }

    fn query_owned<P>(&self, sql: &str, params: P) -> Result<Vec<Row<'static>>>
    where
        P: Params,
    {
        let params = into_libsql_params(params)?;
        let sql = sql.to_string();
        let connection = self.inner.clone();
        let result = database_runtime().run(async move {
            let mut rows = connection.query(&sql, params).await?;
            let column_count = usize::try_from(rows.column_count())
                .map_err(|_| Error::Runtime("negative database column count".into()))?;
            let mut output = Vec::new();
            while let Some(row) = rows.next().await? {
                let mut values = Vec::with_capacity(column_count);
                for index in 0..column_count {
                    values.push(row.get_value(index as i32)?);
                }
                output.push(Row {
                    values,
                    marker: PhantomData,
                });
            }
            Ok(output)
        });
        self.track_remote_query(result)
    }

    pub fn transaction(&self) -> Result<Transaction<'_>> {
        self.transaction_with_behavior(TransactionBehavior::Deferred)
    }

    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        self.transaction()
    }

    pub fn transaction_with_behavior(
        &self,
        behavior: TransactionBehavior,
    ) -> Result<Transaction<'_>> {
        let connection = self.inner.clone();
        let transaction = database_runtime()
            .run(async move { Ok(connection.transaction_with_behavior(behavior).await?) });
        let transaction = self.track_remote_operation(transaction)?;
        let transaction_connection = Connection {
            inner: transaction.deref().clone(),
            database: self.database.clone(),
            mode: self.mode,
            health: self.health.clone(),
        };
        Ok(Transaction {
            inner: Some(transaction),
            connection: transaction_connection,
            marker: PhantomData,
        })
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.inner.last_insert_rowid()
    }

    pub fn pragma_update<T: ToSql>(
        &self,
        _schema: Option<&str>,
        pragma: &str,
        value: T,
    ) -> Result<()> {
        if !pragma
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(Error::ToSqlConversionFailure("invalid pragma name".into()));
        }
        let value = value.to_sql_value()?;
        let literal = match value {
            types::Value::Null => "NULL".into(),
            types::Value::Integer(value) => value.to_string(),
            types::Value::Real(value) => value.to_string(),
            types::Value::Text(value) => format!("'{}'", value.replace('\'', "''")),
            types::Value::Blob(_) => {
                return Err(Error::ToSqlConversionFailure(
                    "blob pragma values are unsupported".into(),
                ));
            }
        };
        self.execute_batch(&format!("PRAGMA {pragma} = {literal}"))
    }

    fn track_remote_operation<T>(&self, result: Result<T>) -> Result<T> {
        if self.mode == ConnectionMode::Local {
            return result;
        }
        let result = result.map_err(|error| classify_error(self.mode, error));
        match &result {
            Ok(_) => self.health.record_success(),
            Err(error) if error.is_unavailable() => self.health.record_failure(error),
            Err(_) => {}
        }
        result
    }

    fn track_remote_query<T>(&self, result: Result<T>) -> Result<T> {
        if self.mode == ConnectionMode::Local {
            return result;
        }
        let result = result.map_err(|error| classify_error(self.mode, error));
        if let Err(error) = &result
            && error.is_unavailable()
        {
            self.health.record_failure(error);
        }
        result
    }
}

pub struct Transaction<'connection> {
    inner: Option<libsql::Transaction>,
    connection: Connection,
    marker: PhantomData<&'connection Connection>,
}

impl Transaction<'_> {
    pub fn commit(mut self) -> Result<()> {
        let transaction = self
            .inner
            .take()
            .ok_or_else(|| Error::Runtime("transaction already completed".into()))?;
        let mode = self.connection.mode;
        let health = self.connection.health.clone();
        let result = database_runtime().run(async move {
            transaction.commit().await?;
            Ok(())
        });
        let result = result.map_err(|error| classify_error(mode, error));
        if mode == ConnectionMode::TursoReplica {
            match &result {
                Ok(_) => health.record_success(),
                Err(error) if error.is_unavailable() => health.record_failure(error),
                Err(_) => {}
            }
        }
        result
    }
}

impl Deref for Transaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        let Some(transaction) = self.inner.take() else {
            return;
        };
        let _ = database_runtime().run(async move {
            transaction.rollback().await?;
            Ok(())
        });
    }
}

pub struct Statement {
    connection: Connection,
    sql: String,
}

impl Statement {
    pub fn query_map<'statement, P, F, T>(
        &'statement mut self,
        params: P,
        mapper: F,
    ) -> Result<MappedRows<'statement, F>>
    where
        P: Params,
        F: FnMut(&Row<'_>) -> Result<T>,
    {
        Ok(MappedRows {
            rows: self.connection.query_owned(&self.sql, params)?.into_iter(),
            mapper,
            marker: PhantomData,
        })
    }
}

pub struct MappedRows<'statement, F> {
    rows: std::vec::IntoIter<Row<'static>>,
    mapper: F,
    marker: PhantomData<&'statement Statement>,
}

impl<F, T> Iterator for MappedRows<'_, F>
where
    F: FnMut(&Row<'_>) -> Result<T>,
{
    type Item = Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.rows.next().map(|row| (self.mapper)(&row))
    }
}

pub struct Row<'row> {
    values: Vec<types::Value>,
    marker: PhantomData<&'row ()>,
}

impl Row<'_> {
    pub fn get<I, T>(&self, index: I) -> Result<T>
    where
        I: RowIndex,
        T: FromSql,
    {
        let index = index.index()?;
        let value = self
            .values
            .get(index)
            .ok_or(Error::InvalidColumnType(index))?;
        T::from_sql(value, index)
    }
}

pub trait RowIndex {
    fn index(self) -> Result<usize>;
}

impl RowIndex for usize {
    fn index(self) -> Result<usize> {
        Ok(self)
    }
}

impl RowIndex for i32 {
    fn index(self) -> Result<usize> {
        usize::try_from(self).map_err(|_| Error::InvalidColumnType(0))
    }
}

pub trait FromSql: Sized {
    fn from_sql(value: &types::Value, index: usize) -> Result<Self>;
}

impl FromSql for types::Value {
    fn from_sql(value: &types::Value, _index: usize) -> Result<Self> {
        Ok(value.clone())
    }
}

impl FromSql for String {
    fn from_sql(value: &types::Value, index: usize) -> Result<Self> {
        match value {
            types::Value::Text(value) => Ok(value.clone()),
            _ => Err(Error::InvalidColumnType(index)),
        }
    }
}

impl FromSql for Vec<u8> {
    fn from_sql(value: &types::Value, index: usize) -> Result<Self> {
        match value {
            types::Value::Blob(value) => Ok(value.clone()),
            _ => Err(Error::InvalidColumnType(index)),
        }
    }
}

impl FromSql for i64 {
    fn from_sql(value: &types::Value, index: usize) -> Result<Self> {
        match value {
            types::Value::Integer(value) => Ok(*value),
            _ => Err(Error::InvalidColumnType(index)),
        }
    }
}

macro_rules! from_sql_integer {
    ($($type:ty),+ $(,)?) => {
        $(
            impl FromSql for $type {
                fn from_sql(value: &types::Value, index: usize) -> Result<Self> {
                    let value = i64::from_sql(value, index)?;
                    <$type>::try_from(value).map_err(|_| Error::InvalidColumnType(index))
                }
            }
        )+
    };
}

from_sql_integer!(i8, i16, i32, isize, u8, u16, u32, u64, usize);

impl FromSql for f64 {
    fn from_sql(value: &types::Value, index: usize) -> Result<Self> {
        match value {
            types::Value::Real(value) => Ok(*value),
            types::Value::Integer(value) => Ok(*value as f64),
            _ => Err(Error::InvalidColumnType(index)),
        }
    }
}

impl FromSql for bool {
    fn from_sql(value: &types::Value, index: usize) -> Result<Self> {
        Ok(i64::from_sql(value, index)? != 0)
    }
}

impl<T: FromSql> FromSql for Option<T> {
    fn from_sql(value: &types::Value, index: usize) -> Result<Self> {
        if matches!(value, types::Value::Null) {
            Ok(None)
        } else {
            T::from_sql(value, index).map(Some)
        }
    }
}

pub trait ToSql {
    fn to_sql_value(&self) -> Result<types::Value>;
}

pub fn to_value<T: ToSql + ?Sized>(value: &T) -> Result<types::Value> {
    value.to_sql_value()
}

impl<T: ToSql + ?Sized> ToSql for &T {
    fn to_sql_value(&self) -> Result<types::Value> {
        (*self).to_sql_value()
    }
}

impl ToSql for str {
    fn to_sql_value(&self) -> Result<types::Value> {
        Ok(types::Value::Text(self.to_string()))
    }
}

impl ToSql for String {
    fn to_sql_value(&self) -> Result<types::Value> {
        Ok(types::Value::Text(self.clone()))
    }
}

impl ToSql for [u8] {
    fn to_sql_value(&self) -> Result<types::Value> {
        Ok(types::Value::Blob(self.to_vec()))
    }
}

impl ToSql for Vec<u8> {
    fn to_sql_value(&self) -> Result<types::Value> {
        Ok(types::Value::Blob(self.clone()))
    }
}

impl ToSql for bool {
    fn to_sql_value(&self) -> Result<types::Value> {
        Ok(types::Value::Integer(i64::from(*self)))
    }
}

macro_rules! to_sql_signed {
    ($($type:ty),+ $(,)?) => {
        $(
            impl ToSql for $type {
                fn to_sql_value(&self) -> Result<types::Value> {
                    i64::try_from(*self)
                        .map(types::Value::Integer)
                        .map_err(|_| Error::ToSqlConversionFailure("integer is outside SQLite range".into()))
                }
            }
        )+
    };
}

to_sql_signed!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl ToSql for f32 {
    fn to_sql_value(&self) -> Result<types::Value> {
        Ok(types::Value::Real(f64::from(*self)))
    }
}

impl ToSql for f64 {
    fn to_sql_value(&self) -> Result<types::Value> {
        Ok(types::Value::Real(*self))
    }
}

impl ToSql for types::Value {
    fn to_sql_value(&self) -> Result<types::Value> {
        Ok(self.clone())
    }
}

impl<T: ToSql> ToSql for Option<T> {
    fn to_sql_value(&self) -> Result<types::Value> {
        match self {
            Some(value) => value.to_sql_value(),
            None => Ok(types::Value::Null),
        }
    }
}

pub trait Params {
    fn into_values(self) -> Result<Vec<types::Value>>;
}

impl Params for () {
    fn into_values(self) -> Result<Vec<types::Value>> {
        Ok(Vec::new())
    }
}

impl Params for [(); 0] {
    fn into_values(self) -> Result<Vec<types::Value>> {
        Ok(Vec::new())
    }
}

impl<T: ToSql> Params for &[T] {
    fn into_values(self) -> Result<Vec<types::Value>> {
        self.iter().map(ToSql::to_sql_value).collect()
    }
}

impl<T: ToSql> Params for Vec<T> {
    fn into_values(self) -> Result<Vec<types::Value>> {
        self.iter().map(ToSql::to_sql_value).collect()
    }
}

pub struct ParamsList {
    values: Vec<Result<types::Value>>,
}

impl ParamsList {
    #[doc(hidden)]
    pub fn new(values: Vec<Result<types::Value>>) -> Self {
        Self { values }
    }
}

impl Params for ParamsList {
    fn into_values(self) -> Result<Vec<types::Value>> {
        self.values.into_iter().collect()
    }
}

pub fn params_from_iter<I>(values: I) -> ParamsList
where
    I: IntoIterator,
    I::Item: ToSql,
{
    ParamsList::new(
        values
            .into_iter()
            .map(|value| value.to_sql_value())
            .collect(),
    )
}

fn into_libsql_params<P: Params>(params: P) -> Result<LibsqlParams> {
    let values = params.into_values()?;
    if values.is_empty() {
        Ok(LibsqlParams::None)
    } else {
        Ok(LibsqlParams::Positional(values))
    }
}

pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[macro_export]
macro_rules! db_params {
    () => {
        $crate::db::ParamsList::new(Vec::new())
    };
    ($($value:expr),+ $(,)?) => {
        $crate::db::ParamsList::new(vec![$($crate::db::to_value(&$value)),+])
    };
}

pub use crate::db_params as params;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_facade_supports_queries_and_immediate_transactions() {
        let connection = Connection::open_in_memory().expect("open local database");
        connection
            .execute_batch("CREATE TABLE values_table (id INTEGER PRIMARY KEY, value TEXT);")
            .expect("create table");
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("begin immediate transaction");
        transaction
            .execute(
                "INSERT INTO values_table (value) VALUES (?1)",
                params!["hello"],
            )
            .expect("insert value");
        transaction.commit().expect("commit value");

        let value = connection
            .query_row("SELECT value FROM values_table", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("query value");
        assert_eq!(value, "hello");
        let first = connection
            .query_row(
                "SELECT 1 AS value UNION ALL SELECT abs(-9223372036854775808)",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("query_row must stop after the first row");
        assert_eq!(first, 1);
        assert_eq!(
            connection.sync_handle().sync().expect("sync local handle"),
            0
        );
        assert!(connection.health_handle().snapshot().available);
    }

    #[test]
    fn remote_transport_errors_are_classified_as_unavailable() {
        let error = classify_error(
            ConnectionMode::TursoReplica,
            Error::Database(libsql::Error::ConnectionFailed("offline".into())),
        );
        assert!(error.is_unavailable());
        assert!(error.to_string().starts_with("database unavailable:"));

        let local_error = classify_error(
            ConnectionMode::Local,
            Error::Database(libsql::Error::ConnectionFailed("offline".into())),
        );
        assert!(!local_error.is_unavailable());
    }

    #[test]
    fn remote_query_failure_updates_shared_health_without_local_read_recovery() {
        let mut connection = Connection::open_in_memory().expect("open local database");
        connection.mode = ConnectionMode::TursoReplica;
        let health = connection.health_handle();

        let error = connection
            .track_remote_query::<()>(Err(Error::Database(libsql::Error::ConnectionFailed(
                "remote unavailable".into(),
            ))))
            .expect_err("remote query must fail");
        assert!(error.is_unavailable());
        assert!(!health.snapshot().available);

        connection
            .track_remote_query(Ok(()))
            .expect("local replica read can still succeed");
        assert!(!health.snapshot().available);
    }

    #[test]
    fn health_tracker_recovers_after_a_successful_remote_operation() {
        let tracker = DatabaseHealthTracker::available();
        tracker.record_failure(&Error::Unavailable("remote offline".into()));

        let failed = tracker.snapshot(ConnectionMode::TursoReplica);
        assert!(!failed.available);
        assert_eq!(failed.consecutive_failures, 1);
        assert!(failed.last_failure_at_ms.is_some());
        assert!(
            failed
                .last_error
                .as_deref()
                .unwrap()
                .contains("remote offline")
        );

        tracker.record_sync_success(17);
        let recovered = tracker.snapshot(ConnectionMode::TursoReplica);
        assert!(recovered.available);
        assert_eq!(recovered.consecutive_failures, 0);
        assert_eq!(recovered.last_frames_synced, 17);
        assert!(recovered.last_success_at_ms.is_some());
        assert!(recovered.last_error.is_none());

        tracker.record_success();
        assert_eq!(
            tracker
                .snapshot(ConnectionMode::TursoReplica)
                .last_frames_synced,
            17,
            "a successful delegated write must preserve the latest sync metric"
        );
    }
}
