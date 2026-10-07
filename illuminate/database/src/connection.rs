//! A database connection: running raw queries, transactions, the query log
//! and query listeners.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use illuminate_support::{Error, Result, Value, ValueExt};
use sqlx::{Database, MySql, Postgres, Sqlite, TransactionManager};

use crate::driver::{self, Affected, Driver, Pool, RawConnection};
use crate::error::{MultipleColumnsSelectedException, QueryException, caused_by_concurrency_error};
use crate::expression::{Expression, IntoBindings};
use crate::query::Builder;
use crate::query::grammar::QueryGrammar;
use crate::schema::{SchemaBuilder, SchemaGrammar};

/// A query that has been executed, as recorded in the query log.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryLog {
    /// The SQL of the query (with bindings substituted while pretending).
    pub query: String,
    /// The query's bindings.
    pub bindings: Vec<Value>,
    /// How long the query took, in milliseconds (`None` while pretending).
    pub time: Option<f64>,
}

/// The event dispatched every time a query is executed.
#[derive(Clone, Debug)]
pub struct QueryExecuted {
    /// The SQL query that was executed.
    pub sql: String,
    /// The bindings for the query.
    pub bindings: Vec<Value>,
    /// The number of milliseconds it took to execute the query.
    pub time_ms: f64,
    /// The name of the connection the query ran on.
    pub connection_name: String,
    driver: Driver,
}

impl QueryExecuted {
    /// Get the SQL with the bindings substituted in, for debugging.
    pub fn to_raw_sql(&self) -> String {
        QueryGrammar::new(self.driver, "")
            .substitute_bindings_into_raw_sql(&self.sql, &self.bindings)
    }
}

/// A query listener callback.
pub type QueryListener = Arc<dyn Fn(&QueryExecuted) + Send + Sync>;

/// The listeners shared by every connection of a manager.
pub(crate) type Listeners = Arc<RwLock<Vec<QueryListener>>>;

/// A callback run when a transaction commits or rolls back.
pub type TransactionCallback =
    Box<dyn FnOnce() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

tokio::task_local! {
    /// The transactions opened by `transaction(...)` in the current task,
    /// keyed by connection id.
    static TRANSACTIONS: HashMap<u64, Arc<TransactionHandle>>;
}

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

/// An open transaction: a dedicated connection plus the nesting level.
pub(crate) struct TransactionHandle {
    connection: tokio::sync::Mutex<RawConnection>,
    level: AtomicUsize,
    after_commit: Mutex<Vec<(usize, TransactionCallback)>>,
    after_rollback: Mutex<Vec<(usize, TransactionCallback)>>,
}

impl TransactionHandle {
    fn level(&self) -> usize {
        self.level.load(Ordering::SeqCst)
    }
}

impl Drop for TransactionHandle {
    fn drop(&mut self) {
        // A transaction abandoned mid-flight (a panic, or a cancelled future)
        // is rolled back before the connection returns to the pool.
        let remaining = self.level.load(Ordering::SeqCst);
        if remaining == 0 {
            return;
        }
        if let Ok(mut connection) = self.connection.try_lock() {
            for _ in 0..remaining {
                match &mut *connection {
                    RawConnection::Sqlite(c) => {
                        <Sqlite as Database>::TransactionManager::start_rollback(&mut **c)
                    }
                    RawConnection::MySql(c) => {
                        <MySql as Database>::TransactionManager::start_rollback(&mut **c)
                    }
                    RawConnection::Postgres(c) => {
                        <Postgres as Database>::TransactionManager::start_rollback(&mut **c)
                    }
                }
            }
        }
    }
}

impl RawConnection {
    async fn begin(&mut self, statement: Option<String>) -> Result<(), sqlx::Error> {
        let statement = statement.map(Cow::Owned);
        match self {
            RawConnection::Sqlite(c) => {
                <Sqlite as Database>::TransactionManager::begin(&mut **c, statement).await
            }
            RawConnection::MySql(c) => {
                <MySql as Database>::TransactionManager::begin(&mut **c, statement).await
            }
            RawConnection::Postgres(c) => {
                <Postgres as Database>::TransactionManager::begin(&mut **c, statement).await
            }
        }
    }

    async fn commit(&mut self) -> Result<(), sqlx::Error> {
        match self {
            RawConnection::Sqlite(c) => {
                <Sqlite as Database>::TransactionManager::commit(&mut **c).await
            }
            RawConnection::MySql(c) => {
                <MySql as Database>::TransactionManager::commit(&mut **c).await
            }
            RawConnection::Postgres(c) => {
                <Postgres as Database>::TransactionManager::commit(&mut **c).await
            }
        }
    }

    async fn rollback(&mut self) -> Result<(), sqlx::Error> {
        match self {
            RawConnection::Sqlite(c) => {
                <Sqlite as Database>::TransactionManager::rollback(&mut **c).await
            }
            RawConnection::MySql(c) => {
                <MySql as Database>::TransactionManager::rollback(&mut **c).await
            }
            RawConnection::Postgres(c) => {
                <Postgres as Database>::TransactionManager::rollback(&mut **c).await
            }
        }
    }
}

struct Inner {
    id: u64,
    name: String,
    driver: Driver,
    config: Value,
    config_error: Option<String>,
    prefix: RwLock<String>,
    pool: Mutex<Option<Pool>>,
    manual_transaction: Mutex<Option<Arc<TransactionHandle>>>,
    logging: AtomicBool,
    query_log: Mutex<Vec<QueryLog>>,
    pretending: AtomicBool,
    listeners: Listeners,
    total_query_duration: Mutex<f64>,
    records_modified: AtomicBool,
}

/// A database connection.
///
/// `Connection` is a cheap, cloneable handle: every clone shares the same
/// pool, query log and transaction state. Connections are usually obtained
/// from the `DB` facade:
///
/// ```no_run
/// # async fn example() -> illuminate_support::Result<()> {
/// use illuminate_database::DB;
///
/// let users = DB::connection("sqlite").select("select * from users where active = ?", (1,)).await?;
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("name", &self.inner.name)
            .field("driver", &self.inner.driver)
            .finish()
    }
}

impl Connection {
    /// Create a connection from its configuration (the array under
    /// `database.connections.<name>`). The pool connects lazily, the first
    /// time a query runs.
    pub fn new(name: impl Into<String>, config: Value) -> Self {
        Self::with_listeners(name.into(), config, Arc::new(RwLock::new(Vec::new())))
    }

    pub(crate) fn with_listeners(name: String, config: Value, listeners: Listeners) -> Self {
        let driver_name = config
            .get("driver")
            .map(|d| d.to_string_lossy())
            .unwrap_or_default();
        let (driver, config_error) = match Driver::from_name(&driver_name) {
            Some(driver) => (driver, None),
            None if driver_name.is_empty() && config.is_null() => (
                Driver::Sqlite,
                Some(format!("Database connection [{name}] not configured.")),
            ),
            None if driver_name.is_empty() => (
                Driver::Sqlite,
                Some("A driver must be specified.".to_string()),
            ),
            None => (
                Driver::Sqlite,
                Some(format!("Unsupported driver [{driver_name}].")),
            ),
        };
        let prefix = config
            .get("prefix")
            .map(|p| p.to_string_lossy())
            .unwrap_or_default();

        Self {
            inner: Arc::new(Inner {
                id: NEXT_CONNECTION_ID.fetch_add(1, Ordering::SeqCst),
                name,
                driver,
                config,
                config_error,
                prefix: RwLock::new(prefix),
                pool: Mutex::new(None),
                manual_transaction: Mutex::new(None),
                logging: AtomicBool::new(false),
                query_log: Mutex::new(Vec::new()),
                pretending: AtomicBool::new(false),
                listeners,
                total_query_duration: Mutex::new(0.0),
                records_modified: AtomicBool::new(false),
            }),
        }
    }

    /// A connection whose configuration is missing; every query fails with
    /// Laravel's "not configured" exception.
    pub(crate) fn unconfigured(name: &str, listeners: Listeners) -> Self {
        Self::with_listeners(name.to_string(), Value::Null, listeners)
    }

    // ------------------------------------------------------------------
    // Information
    // ------------------------------------------------------------------

    /// Get the database connection name.
    pub fn get_name(&self) -> &str {
        &self.inner.name
    }

    /// Get the driver in use.
    pub fn driver(&self) -> Driver {
        self.inner.driver
    }

    /// Get the PDO-style driver name (`sqlite`, `mysql`, `mariadb`, `pgsql`).
    pub fn get_driver_name(&self) -> &'static str {
        self.inner.driver.name()
    }

    /// Get the connection's configuration array.
    pub fn get_config_array(&self) -> &Value {
        &self.inner.config
    }

    /// Get an option from the connection's configuration.
    pub fn get_config(&self, key: &str) -> Value {
        self.inner.config.dot_or_null(key)
    }

    /// Get the name of the connected database.
    pub fn get_database_name(&self) -> String {
        match self.inner.driver {
            Driver::Sqlite => driver::sqlite_database(&self.inner.config),
            _ => self.get_config("database").to_string_lossy(),
        }
    }

    /// Get the table prefix for the connection.
    pub fn get_table_prefix(&self) -> String {
        self.inner.prefix.read().unwrap().clone()
    }

    /// Set the table prefix in use by the connection.
    pub fn set_table_prefix(&self, prefix: impl Into<String>) -> &Self {
        *self.inner.prefix.write().unwrap() = prefix.into();
        self
    }

    /// Get the query grammar used by the connection.
    pub fn query_grammar(&self) -> QueryGrammar {
        QueryGrammar::new(self.inner.driver, self.get_table_prefix())
            .with_upsert_alias(self.get_config("use_upsert_alias").truthy())
    }

    /// Get the schema grammar used by the connection.
    pub fn schema_grammar(&self) -> SchemaGrammar {
        SchemaGrammar::new(
            self.inner.driver,
            self.get_table_prefix(),
            self.inner.config.clone(),
        )
    }

    /// Get a schema builder instance for the connection.
    pub fn get_schema_builder(&self) -> SchemaBuilder {
        SchemaBuilder::new(self.clone())
    }

    /// Alias of [`Connection::get_schema_builder`].
    pub fn schema(&self) -> SchemaBuilder {
        self.get_schema_builder()
    }

    /// Determine if two handles refer to the same connection.
    pub fn same_as(&self, other: &Connection) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    // ------------------------------------------------------------------
    // Query builders
    // ------------------------------------------------------------------

    /// Begin a fluent query against a database table.
    pub fn table(&self, table: impl Into<crate::Ident>) -> Builder {
        Builder::new(self.clone()).from(table)
    }

    /// Get a new query builder instance.
    pub fn query(&self) -> Builder {
        Builder::new(self.clone())
    }

    /// Get a new raw query expression.
    pub fn raw(&self, value: impl Into<String>) -> Expression {
        Expression::new(value)
    }

    /// Escape a value for safe SQL embedding (used by `to_raw_sql`).
    pub fn escape(&self, value: &Value) -> Result<String> {
        self.query_grammar()
            .escape(value)
            .map_err(|e| anyhow::anyhow!(e))
    }

    // ------------------------------------------------------------------
    // Running queries
    // ------------------------------------------------------------------

    /// Run a select statement against the database.
    pub async fn select(&self, query: &str, bindings: impl IntoBindings) -> Result<Vec<Value>> {
        let bindings = bindings.into_bindings();
        if self.pretending() {
            self.log_query(query, bindings, None);
            return Ok(Vec::new());
        }
        let start = Instant::now();
        match self.fetch_rows(query, &bindings).await {
            Ok(rows) => {
                self.log_query(query, bindings, Some(elapsed_ms(start)));
                Ok(rows)
            }
            Err(error) => Err(self.query_exception(query, bindings, error)),
        }
    }

    /// Run a select statement against the write connection (an alias of
    /// [`Connection::select`]; there are no read replicas yet).
    pub async fn select_from_write_connection(
        &self,
        query: &str,
        bindings: impl IntoBindings,
    ) -> Result<Vec<Value>> {
        self.select(query, bindings).await
    }

    /// Run a select statement and stream its rows one at a time (Laravel's
    /// `cursor`).
    ///
    /// The rows are read from a dedicated pooled connection as you consume
    /// them, so only one row is held in memory at a time. Inside a
    /// transaction, or when the pool holds a single connection (an
    /// in-memory SQLite database), that connection must stay available to
    /// other queries, so the rows are fetched up front instead.
    pub fn cursor(
        &self,
        query: &str,
        bindings: impl IntoBindings,
    ) -> BoxStream<'static, Result<Value>> {
        let bindings = bindings.into_bindings();
        let query = query.to_string();
        let connection = self.clone();
        let streamable = !self.pretending()
            && self.current_transaction().is_none()
            && self.pool().is_ok_and(|pool| pool.max_connections() > 1);
        if !streamable {
            return futures::stream::once(async move { connection.select(&query, bindings).await })
                .flat_map(|result| match result {
                    Ok(rows) => futures::stream::iter(rows.into_iter().map(Ok)).boxed(),
                    Err(error) => futures::stream::iter(vec![Err(error)]).boxed(),
                })
                .boxed();
        }

        let (mut sender, receiver) = futures::channel::mpsc::channel::<Result<Value>>(64);
        let producer = async move {
            let start = Instant::now();
            let pool = match connection.pool() {
                Ok(pool) => pool,
                Err(error) => {
                    let _ = sender
                        .send(Err(connection.query_exception(&query, bindings, error)))
                        .await;
                    return;
                }
            };
            let sql = match connection.inner.driver {
                Driver::Sqlite => query.clone(),
                _ => match connection.interpolate(&query, &bindings) {
                    Ok(sql) => sql,
                    Err(error) => {
                        let _ = sender
                            .send(Err(connection.query_exception(&query, bindings, error)))
                            .await;
                        return;
                    }
                },
            };
            let mut failure = None;
            {
                let mut rows = pool.stream(&sql, &bindings);
                while let Some(row) = rows.next().await {
                    match row {
                        Ok(row) => {
                            if sender.send(Ok(row)).await.is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
            }
            match failure {
                Some(error) => {
                    let _ = sender
                        .send(Err(connection.query_exception(
                            &query,
                            bindings,
                            error.into(),
                        )))
                        .await;
                }
                None => connection.log_query(&query, bindings, Some(elapsed_ms(start))),
            }
        };
        CursorStream {
            producer: Some(Box::pin(producer)),
            rows: receiver,
        }
        .boxed()
    }

    /// Run a select statement and return a single result.
    pub async fn select_one(
        &self,
        query: &str,
        bindings: impl IntoBindings,
    ) -> Result<Option<Value>> {
        Ok(self.select(query, bindings).await?.into_iter().next())
    }

    /// Run a select statement and return the first column of the first row.
    pub async fn scalar(&self, query: &str, bindings: impl IntoBindings) -> Result<Value> {
        match self.select_one(query, bindings).await? {
            Some(Value::Object(map)) => {
                if map.len() > 1 {
                    return Err(MultipleColumnsSelectedException.into());
                }
                Ok(map
                    .into_iter()
                    .next()
                    .map(|(_, v)| v)
                    .unwrap_or(Value::Null))
            }
            _ => Ok(Value::Null),
        }
    }

    /// Run an insert statement against the database.
    pub async fn insert(&self, query: &str, bindings: impl IntoBindings) -> Result<bool> {
        self.statement(query, bindings).await
    }

    /// Run an update statement, returning the number of affected rows.
    pub async fn update(&self, query: &str, bindings: impl IntoBindings) -> Result<u64> {
        self.affecting_statement(query, bindings).await
    }

    /// Run a delete statement, returning the number of affected rows.
    pub async fn delete(&self, query: &str, bindings: impl IntoBindings) -> Result<u64> {
        self.affecting_statement(query, bindings).await
    }

    /// Execute an SQL statement and return the boolean result.
    pub async fn statement(&self, query: &str, bindings: impl IntoBindings) -> Result<bool> {
        let bindings = bindings.into_bindings();
        if self.pretending() {
            self.log_query(query, bindings, None);
            return Ok(true);
        }
        let start = Instant::now();
        match self.execute(query, &bindings).await {
            Ok(_) => {
                self.records_have_been_modified(true);
                self.log_query(query, bindings, Some(elapsed_ms(start)));
                Ok(true)
            }
            Err(error) => Err(self.query_exception(query, bindings, error)),
        }
    }

    /// Run an SQL statement and get the number of rows affected.
    pub async fn affecting_statement(
        &self,
        query: &str,
        bindings: impl IntoBindings,
    ) -> Result<u64> {
        let bindings = bindings.into_bindings();
        if self.pretending() {
            self.log_query(query, bindings, None);
            return Ok(0);
        }
        let start = Instant::now();
        match self.execute(query, &bindings).await {
            Ok(affected) => {
                self.records_have_been_modified(affected.rows > 0);
                self.log_query(query, bindings, Some(elapsed_ms(start)));
                Ok(affected.rows)
            }
            Err(error) => Err(self.query_exception(query, bindings, error)),
        }
    }

    /// Run an insert statement and return the auto-incrementing id it created.
    pub(crate) async fn insert_get_id(
        &self,
        query: &str,
        bindings: Vec<Value>,
        sequence: &str,
    ) -> Result<i64> {
        if self.pretending() {
            self.log_query(query, bindings, None);
            return Ok(0);
        }
        let start = Instant::now();
        if self.inner.driver == Driver::Postgres {
            // The grammar appended `returning "id"`.
            return match self.fetch_rows(query, &bindings).await {
                Ok(rows) => {
                    self.records_have_been_modified(true);
                    self.log_query(query, bindings, Some(elapsed_ms(start)));
                    let id = rows
                        .first()
                        .and_then(|row| row.get(sequence))
                        .and_then(|id| id.to_i64_lossy())
                        .unwrap_or_default();
                    Ok(id)
                }
                Err(error) => Err(self.query_exception(query, bindings, error)),
            };
        }
        match self.execute(query, &bindings).await {
            Ok(affected) => {
                self.records_have_been_modified(true);
                self.log_query(query, bindings, Some(elapsed_ms(start)));
                Ok(affected.last_insert_id.unwrap_or_default())
            }
            Err(error) => Err(self.query_exception(query, bindings, error)),
        }
    }

    /// Run a raw, unprepared query against the database.
    pub async fn unprepared(&self, query: &str) -> Result<bool> {
        if self.pretending() {
            self.log_query(query, Vec::new(), None);
            return Ok(true);
        }
        let start = Instant::now();
        match self.execute_unprepared(query).await {
            Ok(affected) => {
                self.records_have_been_modified(affected.rows > 0);
                self.log_query(query, Vec::new(), Some(elapsed_ms(start)));
                Ok(true)
            }
            Err(error) => Err(self.query_exception(query, Vec::new(), error)),
        }
    }

    fn query_exception(&self, query: &str, bindings: Vec<Value>, error: BoxError) -> Error {
        QueryException::new(self.inner.name.clone(), query, bindings, error).into()
    }

    // ------------------------------------------------------------------
    // Driver plumbing
    // ------------------------------------------------------------------

    fn pool(&self) -> Result<Pool, BoxError> {
        if let Some(error) = &self.inner.config_error {
            return Err(error.clone().into());
        }
        let mut pool = self.inner.pool.lock().unwrap();
        if let Some(pool) = pool.as_ref() {
            return Ok(pool.clone());
        }
        let created = driver::create_pool(self.inner.driver, &self.inner.config)?;
        *pool = Some(created.clone());
        Ok(created)
    }

    /// Bind values client-side for the drivers that need it.
    fn interpolate(&self, query: &str, bindings: &[Value]) -> Result<String, BoxError> {
        let driver = self.inner.driver;
        Ok(driver::interpolate(
            query,
            bindings,
            |value| driver::quote_literal(driver, value),
            driver == Driver::Postgres,
        )?)
    }

    async fn fetch_rows(&self, query: &str, bindings: &[Value]) -> Result<Vec<Value>, BoxError> {
        let sql = match self.inner.driver {
            Driver::Sqlite => String::new(),
            _ => self.interpolate(query, bindings)?,
        };
        if let Some(transaction) = self.current_transaction() {
            let mut connection = transaction.connection.lock().await;
            return Ok(match &mut *connection {
                RawConnection::Sqlite(c) => driver::sqlite_fetch(&mut **c, query, bindings).await?,
                RawConnection::MySql(c) => driver::mysql_fetch(&mut **c, &sql).await?,
                RawConnection::Postgres(c) => driver::postgres_fetch(&mut **c, &sql).await?,
            });
        }
        Ok(match self.pool()? {
            Pool::Sqlite(pool) => driver::sqlite_fetch(&pool, query, bindings).await?,
            Pool::MySql(pool) => driver::mysql_fetch(&pool, &sql).await?,
            Pool::Postgres(pool) => driver::postgres_fetch(&pool, &sql).await?,
        })
    }

    async fn execute(&self, query: &str, bindings: &[Value]) -> Result<Affected, BoxError> {
        let sql = match self.inner.driver {
            Driver::Sqlite => String::new(),
            _ => self.interpolate(query, bindings)?,
        };
        if let Some(transaction) = self.current_transaction() {
            let mut connection = transaction.connection.lock().await;
            return Ok(match &mut *connection {
                RawConnection::Sqlite(c) => {
                    driver::sqlite_execute(&mut **c, query, bindings).await?
                }
                RawConnection::MySql(c) => driver::mysql_execute(&mut **c, &sql).await?,
                RawConnection::Postgres(c) => driver::postgres_execute(&mut **c, &sql).await?,
            });
        }
        Ok(match self.pool()? {
            Pool::Sqlite(pool) => driver::sqlite_execute(&pool, query, bindings).await?,
            Pool::MySql(pool) => driver::mysql_execute(&pool, &sql).await?,
            Pool::Postgres(pool) => driver::postgres_execute(&pool, &sql).await?,
        })
    }

    async fn execute_unprepared(&self, query: &str) -> Result<Affected, BoxError> {
        if let Some(transaction) = self.current_transaction() {
            let mut connection = transaction.connection.lock().await;
            return Ok(match &mut *connection {
                RawConnection::Sqlite(c) => driver::sqlite_unprepared(&mut **c, query).await?,
                RawConnection::MySql(c) => driver::mysql_execute(&mut **c, query).await?,
                RawConnection::Postgres(c) => driver::postgres_execute(&mut **c, query).await?,
            });
        }
        Ok(match self.pool()? {
            Pool::Sqlite(pool) => driver::sqlite_unprepared(&pool, query).await?,
            Pool::MySql(pool) => driver::mysql_execute(&pool, query).await?,
            Pool::Postgres(pool) => driver::postgres_execute(&pool, query).await?,
        })
    }

    // ------------------------------------------------------------------
    // Connection lifecycle
    // ------------------------------------------------------------------

    /// Disconnect from the underlying database: the pool is closed and a new
    /// one is created the next time a query runs.
    pub async fn disconnect(&self) {
        let pool = self.inner.pool.lock().unwrap().take();
        *self.inner.manual_transaction.lock().unwrap() = None;
        if let Some(pool) = pool {
            pool.close().await;
        }
    }

    /// Reconnect to the database.
    pub async fn reconnect(&self) -> Result<()> {
        self.disconnect().await;
        self.pool()
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    // ------------------------------------------------------------------
    // Transactions
    // ------------------------------------------------------------------

    /// The transaction this task should run its queries in, if any.
    fn current_transaction(&self) -> Option<Arc<TransactionHandle>> {
        TRANSACTIONS
            .try_with(|map| map.get(&self.inner.id).cloned())
            .ok()
            .flatten()
            .or_else(|| self.inner.manual_transaction.lock().unwrap().clone())
    }

    /// Execute a closure within a transaction.
    ///
    /// Every query made while the closure runs — through this handle, the
    /// `DB` facade, a query builder or the schema builder — uses the
    /// transaction's connection. Returning an error rolls the transaction
    /// back; nested transactions use savepoints.
    ///
    /// ```no_run
    /// # async fn example() -> illuminate_support::Result<()> {
    /// use illuminate_database::DB;
    /// use illuminate_support::json;
    ///
    /// DB::transaction(|| async {
    ///     DB::table("users").update(json!({"votes": 1})).await?;
    ///     DB::table("posts").delete().await?;
    ///     Ok(())
    /// })
    /// .await?;
    /// # Ok(()) }
    /// ```
    pub async fn transaction<F, Fut, T>(&self, callback: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        if self.pretending() {
            return callback().await;
        }

        if let Some(handle) = self.current_transaction() {
            self.begin_on(&handle).await?;
            return match callback().await {
                Ok(value) => {
                    self.commit_on(&handle).await?;
                    Ok(value)
                }
                Err(error) => {
                    self.rollback_on(&handle).await?;
                    Err(error)
                }
            };
        }

        let handle = self.start_transaction().await?;
        let mut map = TRANSACTIONS.try_with(|map| map.clone()).unwrap_or_default();
        map.insert(self.inner.id, handle.clone());

        match TRANSACTIONS.scope(map, callback()).await {
            Ok(value) => {
                self.commit_on(&handle).await?;
                Ok(value)
            }
            Err(error) => {
                // The original error is more useful than a rollback failure.
                let _ = self.rollback_on(&handle).await;
                Err(error)
            }
        }
    }

    /// Execute a closure within a transaction, retrying it up to `attempts`
    /// times when it fails because of a deadlock.
    pub async fn transaction_with_attempts<F, Fut, T>(
        &self,
        attempts: usize,
        mut callback: F,
    ) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let attempts = attempts.max(1);
        let mut attempt = 1;
        loop {
            match self.transaction(&mut callback).await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let retry = attempt < attempts
                        && self.transaction_level() == 0
                        && caused_by_concurrency_error(&format!("{error:#}"));
                    if !retry {
                        return Err(error);
                    }
                    attempt += 1;
                }
            }
        }
    }

    /// Start a new database transaction (or a savepoint, when one is open).
    ///
    /// Until it is committed or rolled back, every query on this connection
    /// runs inside the transaction.
    pub async fn begin_transaction(&self) -> Result<()> {
        if self.pretending() {
            return Ok(());
        }
        if let Some(handle) = self.current_transaction() {
            return self.begin_on(&handle).await;
        }
        let handle = self.start_transaction().await?;
        *self.inner.manual_transaction.lock().unwrap() = Some(handle);
        Ok(())
    }

    /// Commit the active database transaction.
    pub async fn commit(&self) -> Result<()> {
        if let Some(handle) = self.current_transaction() {
            self.commit_on(&handle).await?;
            self.release_manual_transaction(&handle);
        }
        Ok(())
    }

    /// Rollback the active database transaction (or its latest savepoint).
    pub async fn rollback(&self) -> Result<()> {
        if let Some(handle) = self.current_transaction() {
            self.rollback_on(&handle).await?;
            self.release_manual_transaction(&handle);
        }
        Ok(())
    }

    /// Alias of [`Connection::rollback`], spelled like Laravel's `rollBack`.
    pub async fn roll_back(&self) -> Result<()> {
        self.rollback().await
    }

    /// Get the number of active transactions.
    pub fn transaction_level(&self) -> usize {
        self.current_transaction().map(|h| h.level()).unwrap_or(0)
    }

    /// Execute the callback after the current transaction commits, or right
    /// away when no transaction is open. Callbacks registered inside a
    /// transaction that rolls back are discarded.
    pub fn after_commit(&self, callback: impl FnOnce() + Send + 'static) {
        let deferred = self.defer_until_commit(Box::new(move || {
            callback();
            Box::pin(async {})
        }));
        if let Err(callback) = deferred {
            // No transaction: the wrapper runs the callback synchronously.
            drop(callback());
        }
    }

    /// Execute the async callback after the current transaction commits, or
    /// right away when no transaction is open.
    ///
    /// ```no_run
    /// # async fn example() -> illuminate_support::Result<()> {
    /// use illuminate_database::DB;
    ///
    /// DB::connection("sqlite").after_commit_async(|| async {
    ///     // Notify the shipping service...
    /// })
    /// .await;
    /// # Ok(()) }
    /// ```
    pub async fn after_commit_async<F, Fut>(&self, callback: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if let Err(callback) = self.defer_until_commit(Box::new(move || Box::pin(callback()))) {
            callback().await;
        }
    }

    /// Hold the callback until the current transaction commits. Hands it
    /// back when no transaction is open, so the caller decides what to do.
    /// Callbacks deferred inside a savepoint that rolls back are discarded.
    pub fn defer_until_commit(
        &self,
        callback: TransactionCallback,
    ) -> std::result::Result<(), TransactionCallback> {
        match self.current_transaction() {
            Some(handle) if handle.level() > 0 => {
                let level = handle.level();
                handle.after_commit.lock().unwrap().push((level, callback));
                Ok(())
            }
            _ => Err(callback),
        }
    }

    /// Hold the callback until the current transaction (or savepoint) rolls
    /// back; it is discarded when the transaction commits. Hands it back
    /// when no transaction is open.
    pub fn defer_until_rollback(
        &self,
        callback: TransactionCallback,
    ) -> std::result::Result<(), TransactionCallback> {
        match self.current_transaction() {
            Some(handle) if handle.level() > 0 => {
                let level = handle.level();
                handle
                    .after_rollback
                    .lock()
                    .unwrap()
                    .push((level, callback));
                Ok(())
            }
            _ => Err(callback),
        }
    }

    async fn start_transaction(&self) -> Result<Arc<TransactionHandle>> {
        let pool = self
            .pool()
            .map_err(|e| self.query_exception("begin transaction", Vec::new(), e))?;
        let mut connection = pool
            .acquire()
            .await
            .map_err(|e| self.query_exception("begin transaction", Vec::new(), e.into()))?;
        let statement = match self.inner.driver {
            Driver::Sqlite => self
                .get_config("transaction_mode")
                .as_str()
                .map(|mode| mode.to_ascii_uppercase())
                .filter(|mode| matches!(mode.as_str(), "DEFERRED" | "IMMEDIATE" | "EXCLUSIVE"))
                .map(|mode| format!("BEGIN {mode} TRANSACTION")),
            _ => None,
        };
        connection
            .begin(statement)
            .await
            .map_err(|e| self.query_exception("begin transaction", Vec::new(), e.into()))?;
        Ok(Arc::new(TransactionHandle {
            connection: tokio::sync::Mutex::new(connection),
            level: AtomicUsize::new(1),
            after_commit: Mutex::new(Vec::new()),
            after_rollback: Mutex::new(Vec::new()),
        }))
    }

    async fn begin_on(&self, handle: &TransactionHandle) -> Result<()> {
        let mut connection = handle.connection.lock().await;
        connection
            .begin(None)
            .await
            .map_err(|e| self.query_exception("savepoint", Vec::new(), e.into()))?;
        handle.level.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn commit_on(&self, handle: &TransactionHandle) -> Result<()> {
        if handle.level() == 0 {
            return Ok(());
        }
        let result = {
            let mut connection = handle.connection.lock().await;
            connection.commit().await
        };
        let level = handle.level.fetch_sub(1, Ordering::SeqCst) - 1;
        result.map_err(|e| self.query_exception("commit", Vec::new(), e.into()))?;

        if level == 0 {
            handle.after_rollback.lock().unwrap().clear();
            let callbacks: Vec<_> = std::mem::take(&mut *handle.after_commit.lock().unwrap());
            for (_, callback) in callbacks {
                callback().await;
            }
        } else {
            // Callbacks of the committed savepoint now belong to its parent.
            for callbacks in [&handle.after_commit, &handle.after_rollback] {
                for (callback_level, _) in callbacks.lock().unwrap().iter_mut() {
                    if *callback_level > level {
                        *callback_level = level;
                    }
                }
            }
        }
        Ok(())
    }

    async fn rollback_on(&self, handle: &TransactionHandle) -> Result<()> {
        if handle.level() == 0 {
            return Ok(());
        }
        let result = {
            let mut connection = handle.connection.lock().await;
            connection.rollback().await
        };
        let level = handle.level.fetch_sub(1, Ordering::SeqCst) - 1;
        handle
            .after_commit
            .lock()
            .unwrap()
            .retain(|(callback_level, _)| *callback_level <= level);
        let rolled_back: Vec<_> = {
            let mut callbacks = handle.after_rollback.lock().unwrap();
            let (rolled_back, kept) = std::mem::take(&mut *callbacks)
                .into_iter()
                .partition(|(callback_level, _)| *callback_level > level);
            *callbacks = kept;
            rolled_back
        };
        result.map_err(|e| self.query_exception("rollback", Vec::new(), e.into()))?;
        for (_, callback) in rolled_back {
            callback().await;
        }
        Ok(())
    }

    fn release_manual_transaction(&self, handle: &Arc<TransactionHandle>) {
        if handle.level() == 0 {
            let mut manual = self.inner.manual_transaction.lock().unwrap();
            if manual.as_ref().is_some_and(|m| Arc::ptr_eq(m, handle)) {
                *manual = None;
            }
        }
    }

    // ------------------------------------------------------------------
    // Pretending
    // ------------------------------------------------------------------

    /// Execute the given callback in "dry run" mode: queries are logged but
    /// never run. Returns the queries that would have been executed.
    pub async fn pretend<F, Fut>(&self, callback: F) -> Result<Vec<QueryLog>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let logging = self.inner.logging.swap(true, Ordering::SeqCst);
        let previous_log = std::mem::take(&mut *self.inner.query_log.lock().unwrap());
        let was_pretending = self.inner.pretending.swap(true, Ordering::SeqCst);

        let result = callback().await;

        self.inner
            .pretending
            .store(was_pretending, Ordering::SeqCst);
        let log = std::mem::replace(&mut *self.inner.query_log.lock().unwrap(), previous_log);
        self.inner.logging.store(logging, Ordering::SeqCst);

        result.map(|_| log)
    }

    /// Determine if the connection is in a "dry run".
    pub fn pretending(&self) -> bool {
        self.inner.pretending.load(Ordering::SeqCst)
    }

    // ------------------------------------------------------------------
    // Query logging & events
    // ------------------------------------------------------------------

    fn log_query(&self, query: &str, bindings: Vec<Value>, time: Option<f64>) {
        *self.inner.total_query_duration.lock().unwrap() += time.unwrap_or(0.0);

        let listeners = self.inner.listeners.read().unwrap().clone();
        if !listeners.is_empty() {
            let event = QueryExecuted {
                sql: query.to_string(),
                bindings: bindings.clone(),
                time_ms: time.unwrap_or(0.0),
                connection_name: self.inner.name.clone(),
                driver: self.inner.driver,
            };
            for listener in listeners {
                listener(&event);
            }
        }

        if self.inner.logging.load(Ordering::SeqCst) {
            let query = if self.pretending() {
                self.query_grammar()
                    .substitute_bindings_into_raw_sql(query, &bindings)
            } else {
                query.to_string()
            };
            self.inner.query_log.lock().unwrap().push(QueryLog {
                query,
                bindings,
                time,
            });
        }
    }

    /// Register a database query listener.
    pub fn listen(&self, callback: impl Fn(&QueryExecuted) + Send + Sync + 'static) {
        self.inner
            .listeners
            .write()
            .unwrap()
            .push(Arc::new(callback));
    }

    /// Enable the query log on the connection.
    pub fn enable_query_log(&self) {
        self.inner.logging.store(true, Ordering::SeqCst);
    }

    /// Disable the query log on the connection.
    pub fn disable_query_log(&self) {
        self.inner.logging.store(false, Ordering::SeqCst);
    }

    /// Determine whether we're logging queries.
    pub fn logging(&self) -> bool {
        self.inner.logging.load(Ordering::SeqCst)
    }

    /// Get the connection query log.
    pub fn get_query_log(&self) -> Vec<QueryLog> {
        self.inner.query_log.lock().unwrap().clone()
    }

    /// Get the connection query log with the bindings substituted in.
    pub fn get_raw_query_log(&self) -> Vec<QueryLog> {
        let grammar = self.query_grammar();
        self.get_query_log()
            .into_iter()
            .map(|mut log| {
                log.query = grammar.substitute_bindings_into_raw_sql(&log.query, &log.bindings);
                log
            })
            .collect()
    }

    /// Clear the query log.
    pub fn flush_query_log(&self) {
        self.inner.query_log.lock().unwrap().clear();
    }

    /// Get the total time, in milliseconds, spent running queries.
    pub fn total_query_duration(&self) -> f64 {
        *self.inner.total_query_duration.lock().unwrap()
    }

    /// Reset the total query duration.
    pub fn reset_total_query_duration(&self) {
        *self.inner.total_query_duration.lock().unwrap() = 0.0;
    }

    /// Determine if the connection has modified any database records.
    pub fn has_modified_records(&self) -> bool {
        self.inner.records_modified.load(Ordering::SeqCst)
    }

    /// Indicate if any records have been modified.
    pub fn records_have_been_modified(&self, value: bool) {
        if value {
            self.inner.records_modified.store(true, Ordering::SeqCst);
        }
    }

    /// Reset the record modification state.
    pub fn forget_record_modification_state(&self) {
        self.inner.records_modified.store(false, Ordering::SeqCst);
    }
}

pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The stream behind [`Connection::cursor`]: polling it drives the query
/// (the producer), which hands each row over through a bounded channel.
struct CursorStream {
    producer: Option<futures::future::BoxFuture<'static, ()>>,
    rows: futures::channel::mpsc::Receiver<Result<Value>>,
}

impl futures::Stream for CursorStream {
    type Item = Result<Value>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if let Some(producer) = self.producer.as_mut()
            && producer.as_mut().poll(cx).is_ready()
        {
            self.producer = None;
        }
        self.rows.poll_next_unpin(cx)
    }
}

fn elapsed_ms(start: Instant) -> f64 {
    (start.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0
}
