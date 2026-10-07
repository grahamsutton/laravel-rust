//! The database cache store and its locks.
//!
//! Items live in the `cache` table and locks in the `cache_locks` table,
//! exactly like Laravel's `DatabaseStore` / `DatabaseLock`:
//!
//! ```php
//! Schema::create('cache', function (Blueprint $table) {
//!     $table->string('key')->primary();
//!     $table->mediumText('value');
//!     $table->bigInteger('expiration')->index();
//! });
//!
//! Schema::create('cache_locks', function (Blueprint $table) {
//!     $table->string('key')->primary();
//!     $table->string('owner');
//!     $table->bigInteger('expiration')->index();
//! });
//! ```
//!
//! Values are stored as JSON text (just like the file store), and
//! expirations as UNIX timestamps.

use std::sync::Arc;

use async_trait::async_trait;

use illuminate_database::{Builder, Connection, QueryException};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Carbon, Error, Result, Value, ValueExt, json};

use crate::lock::{Lock, LockDriver, LockInfo};
use crate::repository::FLEXIBLE_CREATED_KEY_PREFIX;
use crate::store::{LockProvider, Store, int_value, separate_lock_store_required};

/// How long "forever" lasts in the database: ten years, like Laravel.
pub const FOREVER_SECONDS: u64 = 315_360_000;

/// Laravel's default lock lottery: prune expired locks on 2 out of 100 acquisitions.
pub const DEFAULT_LOCK_LOTTERY: [u32; 2] = [2, 100];

/// How long a lock without an explicit duration is held: one day.
pub const DEFAULT_LOCK_TIMEOUT: u64 = 86_400;

/// The current UNIX timestamp (honouring `Carbon::set_test_now`).
fn current_time() -> i64 {
    Carbon::now().timestamp()
}

/// The UNIX timestamp `seconds` from now; `0` means "forever".
fn expiration_for(seconds: u64) -> i64 {
    let seconds = if seconds == 0 {
        FOREVER_SECONDS
    } else {
        seconds
    };
    current_time().saturating_add(i64::try_from(seconds).unwrap_or(i64::MAX))
}

/// The expiration timestamp stored in a row.
fn row_expiration(row: &Value) -> i64 {
    row.get("expiration")
        .and_then(ValueExt::to_i64_lossy)
        .unwrap_or(0)
}

/// A string column of a row.
fn row_string(row: &Value, column: &str) -> Option<String> {
    match row.get(column)? {
        Value::Null => None,
        value => Some(value.to_string_lossy()),
    }
}

/// Determine if the error is a deadlock / lock timeout reported by the database.
fn caused_by_concurrency_error(error: &Error) -> bool {
    error
        .downcast_ref::<QueryException>()
        .is_some_and(QueryException::is_concurrency_error)
}

/// A cache store backed by a database table.
///
/// ```
/// use illuminate_cache::{DatabaseStore, LockProvider, Store};
/// use illuminate_database::Connection;
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let connection = Connection::new("sqlite", json!({"driver": "sqlite", "database": ":memory:"}));
/// let schema = connection.get_schema_builder();
/// schema.create("cache", |table| {
///     table.string("key").primary();
///     table.medium_text("value");
///     table.big_integer("expiration").index();
/// }).await.unwrap();
/// schema.create("cache_locks", |table| {
///     table.string("key").primary();
///     table.string("owner");
///     table.big_integer("expiration").index();
/// }).await.unwrap();
///
/// let store = DatabaseStore::new(connection, "cache").with_prefix("laravel-cache-");
///
/// store.put("framework", json!("Laravel"), 60).await.unwrap();
/// assert_eq!(store.get("framework").await.unwrap(), Some(json!("Laravel")));
/// assert_eq!(store.increment("visits", 1).await.unwrap(), 1);
///
/// let lock = store.lock("reports", 10, None);
/// assert!(lock.get().await.unwrap());
/// assert!(!store.lock("reports", 10, None).get().await.unwrap());
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct DatabaseStore {
    connection: Connection,
    lock_connection: Connection,
    table: String,
    prefix: String,
    lock_table: String,
    lock_lottery: Option<[u32; 2]>,
    default_lock_timeout: u64,
}

impl DatabaseStore {
    /// Create a new database store using the given connection and table.
    ///
    /// Locks are kept in the `cache_locks` table of the same connection.
    pub fn new(connection: Connection, table: impl Into<String>) -> Self {
        Self {
            lock_connection: connection.clone(),
            connection,
            table: table.into(),
            prefix: String::new(),
            lock_table: "cache_locks".to_string(),
            lock_lottery: Some(DEFAULT_LOCK_LOTTERY),
            default_lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    /// Prefix every key with the given string (the `prefix` option).
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Keep locks in the given table (the `lock_table` option).
    pub fn with_lock_table(mut self, table: impl Into<String>) -> Self {
        self.lock_table = table.into();
        self
    }

    /// Manage locks using a different connection (the `lock_connection` option).
    pub fn with_lock_connection(mut self, connection: Connection) -> Self {
        self.lock_connection = connection;
        self
    }

    /// Set the odds of pruning expired locks when a lock is acquired
    /// (`[wins, out_of]`, the `lock_lottery` option). `None` never prunes.
    pub fn with_lock_lottery(mut self, lottery: Option<[u32; 2]>) -> Self {
        self.lock_lottery = lottery;
        self
    }

    /// Set how long locks without an explicit duration are held, in
    /// seconds (the `lock_timeout` option).
    pub fn with_default_lock_timeout(mut self, seconds: u64) -> Self {
        self.default_lock_timeout = seconds;
        self
    }

    /// Use a different connection for the cache table.
    pub fn with_connection(mut self, connection: Connection) -> Self {
        self.connection = connection;
        self
    }

    /// The underlying database connection.
    pub fn get_connection(&self) -> &Connection {
        &self.connection
    }

    /// The connection used to manage locks.
    pub fn get_lock_connection(&self) -> &Connection {
        &self.lock_connection
    }

    /// The name of the cache table.
    pub fn get_table(&self) -> &str {
        &self.table
    }

    /// The name of the cache locks table.
    pub fn get_lock_table(&self) -> &str {
        &self.lock_table
    }

    /// Determine if the lock store is separate from the cache store.
    pub fn has_separate_lock_store(&self) -> bool {
        self.lock_table != self.table
    }

    /// Remove an item from the cache, but only if it has expired.
    pub async fn forget_if_expired(&self, key: &str) -> Result<bool> {
        self.forget_many_if_expired(&[key.to_string()], false).await
    }

    /// A query builder for the cache table.
    fn table(&self) -> Builder {
        self.connection.table(self.table.as_str())
    }

    /// A query builder for the cache locks table.
    fn lock_table(&self) -> Builder {
        self.lock_connection.table(self.lock_table.as_str())
    }

    fn prefixed(&self, key: &str) -> String {
        format!("{}{key}", self.prefix)
    }

    /// The key holding the "created at" timestamp of a flexible item.
    fn flexible_key(&self, key: &str) -> String {
        format!("{}{FLEXIBLE_CREATED_KEY_PREFIX}{key}", self.prefix)
    }

    fn serialize(value: &Value) -> Result<String> {
        Ok(serde_json::to_string(value)?)
    }

    /// Decode a stored value; anything that isn't valid JSON is a miss.
    fn unserialize(value: &Value) -> Option<Value> {
        match value {
            Value::String(json) => serde_json::from_str(json).ok(),
            Value::Null => None,
            other => Some(other.clone()),
        }
    }

    /// Delete the given (unprefixed or already prefixed) keys if they expired.
    async fn forget_many_if_expired(&self, keys: &[String], prefixed: bool) -> Result<bool> {
        let keys: Vec<String> = keys
            .iter()
            .flat_map(|key| {
                if prefixed {
                    let unprefixed = key.strip_prefix(&self.prefix).unwrap_or(key);
                    [key.clone(), self.flexible_key(unprefixed)]
                } else {
                    [self.prefixed(key), self.flexible_key(key)]
                }
            })
            .collect();
        self.table()
            .where_in("key", keys)
            .where_op("expiration", "<=", current_time())
            .delete()
            .await?;
        Ok(true)
    }

    /// Increment (or decrement) an item inside a transaction, locking the
    /// row for update where the database supports it.
    async fn increment_or_decrement(&self, key: &str, amount: i64) -> Result<i64> {
        let prefixed = self.prefixed(key);
        let query = || self.table().where_("key", prefixed.as_str());

        self.connection
            .transaction(|| async {
                // Two attempts: if the item is missing but another process
                // inserts it first, the second pass updates that row instead.
                for _ in 0..2 {
                    let now = current_time();
                    match query().lock_for_update().first().await? {
                        Some(row) => {
                            let fresh = row_expiration(&row) > now;
                            let current = match row.get("value").and_then(Self::unserialize) {
                                Some(value) if fresh => int_value(&value),
                                _ => 0,
                            };
                            let new = current.saturating_add(amount);
                            let mut values = json!({"value": Self::serialize(&json!(new))?});
                            if !fresh {
                                values["expiration"] = json!(expiration_for(0));
                            }
                            query().update(values).await?;
                            return Ok(new);
                        }
                        None => {
                            let inserted = self
                                .table()
                                .insert_or_ignore(json!({
                                    "key": prefixed,
                                    "value": Self::serialize(&json!(amount))?,
                                    "expiration": expiration_for(0),
                                }))
                                .await?;
                            if inserted > 0 {
                                return Ok(amount);
                            }
                        }
                    }
                }
                Err(
                    RuntimeException::new(format!("Unable to increment the cache item [{key}]."))
                        .into(),
                )
            })
            .await
    }

    /// The lock driver for this store's locks.
    pub fn lock_driver(&self) -> DatabaseLock {
        DatabaseLock::new(self.lock_connection.clone(), self.lock_table.clone())
            .with_lottery(self.lock_lottery)
            .with_default_timeout(self.default_lock_timeout)
    }
}

#[async_trait]
impl Store for DatabaseStore {
    async fn get(&self, key: &str) -> Result<Option<Value>> {
        let mut values = self.many(&[key.to_string()]).await?;
        Ok(values.pop().and_then(|(_, value)| value))
    }

    async fn many(&self, keys: &[String]) -> Result<Vec<(String, Option<Value>)>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let prefixed: Vec<String> = keys.iter().map(|key| self.prefixed(key)).collect();
        let rows = self.table().where_in("key", prefixed).get().await?;

        // Expired items are removed from the cache and reported as misses.
        let now = current_time();
        let (fresh, expired): (Vec<Value>, Vec<Value>) =
            rows.into_iter().partition(|row| row_expiration(row) > now);
        if !expired.is_empty() {
            let keys: Vec<String> = expired
                .iter()
                .filter_map(|row| row_string(row, "key"))
                .collect();
            self.forget_many_if_expired(&keys, true).await?;
        }

        Ok(keys
            .iter()
            .map(|key| {
                let prefixed = self.prefixed(key);
                let value = fresh
                    .iter()
                    .find(|row| row_string(row, "key").as_deref() == Some(prefixed.as_str()))
                    .and_then(|row| row.get("value"))
                    .and_then(Self::unserialize);
                (key.clone(), value)
            })
            .collect())
    }

    async fn put(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        self.put_many(vec![(key.to_string(), value)], seconds).await
    }

    async fn put_many(&self, values: Vec<(String, Value)>, seconds: u64) -> Result<bool> {
        if values.is_empty() {
            return Ok(true);
        }
        let expiration = expiration_for(seconds);
        let records = values
            .iter()
            .map(|(key, value)| {
                Ok(json!({
                    "key": self.prefixed(key),
                    "value": Self::serialize(value)?,
                    "expiration": expiration,
                }))
            })
            .collect::<Result<Vec<Value>>>()?;
        Ok(self.table().upsert(records, &["key"], None).await? > 0)
    }

    /// Atomically store the item unless a fresh one exists: expired items
    /// are cleared first, then the row is inserted with `insert or ignore`
    /// so concurrent callers have a single winner.
    async fn add(&self, key: &str, value: Value, seconds: u64) -> Result<bool> {
        if self.get(key).await?.is_some() {
            return Ok(false);
        }
        let inserted = self
            .table()
            .insert_or_ignore(json!({
                "key": self.prefixed(key),
                "value": Self::serialize(&value)?,
                "expiration": expiration_for(seconds),
            }))
            .await?;
        Ok(inserted > 0)
    }

    async fn increment(&self, key: &str, value: i64) -> Result<i64> {
        self.increment_or_decrement(key, value).await
    }

    async fn decrement(&self, key: &str, value: i64) -> Result<i64> {
        self.increment_or_decrement(key, value.saturating_neg())
            .await
    }

    async fn forever(&self, key: &str, value: Value) -> Result<bool> {
        self.put(key, value, FOREVER_SECONDS).await
    }

    async fn touch(&self, key: &str, seconds: u64) -> Result<bool> {
        let now = current_time();
        let updated = self
            .table()
            .where_("key", self.prefixed(key))
            .where_op("expiration", ">", now)
            .update(json!({"expiration": expiration_for(seconds)}))
            .await?;
        Ok(updated > 0)
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        self.table()
            .where_in("key", vec![self.prefixed(key), self.flexible_key(key)])
            .delete()
            .await?;
        Ok(true)
    }

    async fn flush(&self) -> Result<bool> {
        self.table().delete().await?;
        Ok(true)
    }

    fn get_prefix(&self) -> String {
        self.prefix.clone()
    }

    fn lock_provider(&self) -> Option<&dyn LockProvider> {
        Some(self)
    }

    async fn flush_locks(&self) -> Result<bool> {
        if !self.has_separate_lock_store() {
            return Err(separate_lock_store_required());
        }
        self.lock_table().delete().await?;
        Ok(true)
    }
}

impl LockProvider for DatabaseStore {
    fn lock(&self, name: &str, seconds: u64, owner: Option<String>) -> Lock {
        Lock::new(
            Arc::new(self.lock_driver()),
            self.prefixed(name),
            seconds,
            owner,
        )
    }
}

/// Locks stored as rows of the `cache_locks` table.
///
/// A lock is acquired by inserting its row; when the row already exists
/// the lock is taken over only if it is ours or has expired. Expired locks
/// are pruned now and then, according to the lock lottery.
#[derive(Debug, Clone)]
pub struct DatabaseLock {
    connection: Connection,
    table: String,
    lottery: Option<[u32; 2]>,
    default_timeout: u64,
}

impl DatabaseLock {
    /// Create a lock driver storing its locks in the given table.
    pub fn new(connection: Connection, table: impl Into<String>) -> Self {
        Self {
            connection,
            table: table.into(),
            lottery: Some(DEFAULT_LOCK_LOTTERY),
            default_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    /// Set the odds of pruning expired locks on acquisition (`None` never prunes).
    pub fn with_lottery(mut self, lottery: Option<[u32; 2]>) -> Self {
        self.lottery = lottery;
        self
    }

    /// Set how long locks without an explicit duration are held, in seconds.
    pub fn with_default_timeout(mut self, seconds: u64) -> Self {
        self.default_timeout = seconds;
        self
    }

    /// The name of the database connection used to manage the locks.
    pub fn get_connection_name(&self) -> &str {
        self.connection.get_name()
    }

    /// Delete every lock that is past its expiration.
    pub async fn prune_expired_locks(&self) -> Result<()> {
        let pruned = self
            .table()
            .where_op("expiration", "<=", current_time())
            .delete()
            .await;
        match pruned {
            Ok(_) => Ok(()),
            Err(error)
                if caused_by_concurrency_error(&error)
                    && self.connection.transaction_level() == 0 =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn table(&self) -> Builder {
        self.connection.table(self.table.as_str())
    }

    /// The UNIX timestamp at which a lock held for `seconds` expires.
    fn expires_at(&self, seconds: u64) -> i64 {
        let timeout = if seconds > 0 {
            seconds
        } else {
            self.default_timeout
        };
        current_time().saturating_add(i64::try_from(timeout).unwrap_or(i64::MAX))
    }

    fn wins_lottery(&self) -> bool {
        match self.lottery {
            Some([wins, out_of]) if out_of > 0 => rand::random_range(1..=out_of) <= wins,
            _ => false,
        }
    }
}

#[async_trait]
impl LockDriver for DatabaseLock {
    async fn acquire(&self, lock: &LockInfo) -> Result<bool> {
        let expiration = self.expires_at(lock.seconds);
        let inserted = self
            .table()
            .insert(json!({
                "key": lock.name,
                "owner": lock.owner,
                "expiration": expiration,
            }))
            .await;

        let acquired = match inserted {
            Ok(_) => true,
            Err(error) => {
                if error.downcast_ref::<QueryException>().is_none()
                    || (self.connection.transaction_level() > 0
                        && caused_by_concurrency_error(&error))
                {
                    return Err(error);
                }
                // The lock exists: take it over if it is ours or has expired.
                let now = current_time();
                let owner = lock.owner.as_str();
                self.table()
                    .where_("key", lock.name.as_str())
                    .where_group(|query| {
                        query
                            .where_("owner", owner)
                            .or_where_op("expiration", "<=", now)
                    })
                    .update(json!({"owner": owner, "expiration": expiration}))
                    .await?
                    >= 1
            }
        };

        if self.wins_lottery() {
            self.prune_expired_locks().await?;
        }

        Ok(acquired)
    }

    async fn release(&self, lock: &LockInfo) -> Result<bool> {
        let deleted = self
            .table()
            .where_("key", lock.name.as_str())
            .where_("owner", lock.owner.as_str())
            .delete()
            .await;
        match deleted {
            Ok(deleted) => Ok(deleted > 0),
            Err(error)
                if caused_by_concurrency_error(&error)
                    && self.connection.transaction_level() == 0 =>
            {
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    async fn force_release(&self, lock: &LockInfo) -> Result<()> {
        self.table()
            .where_("key", lock.name.as_str())
            .delete()
            .await?;
        Ok(())
    }

    async fn current_owner(&self, lock: &LockInfo) -> Result<Option<String>> {
        Ok(self
            .table()
            .where_("key", lock.name.as_str())
            .where_op("expiration", ">", current_time())
            .first()
            .await?
            .and_then(|row| row_string(&row, "owner")))
    }

    async fn refresh(&self, lock: &LockInfo, seconds: u64) -> Result<bool> {
        let updated = self
            .table()
            .where_("key", lock.name.as_str())
            .where_("owner", lock.owner.as_str())
            .where_op("expiration", ">", current_time())
            .update(json!({"expiration": self.expires_at(seconds)}))
            .await?;
        Ok(updated >= 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::Repository;
    use crate::testing::freeze_time;
    use illuminate_config::Repository as Config;
    use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
    use illuminate_database::{DB, DatabaseServiceProvider, Schema};

    const NOW: i64 = 1_700_000_000;

    /// Boot a container with an in-memory SQLite database holding the
    /// `cache` and `cache_locks` tables.
    async fn app() -> (Arc<Container>, LocalInstanceGuard) {
        let container = Arc::new(Container::new());
        let guard = Container::set_local_instance(container.clone());
        container.instance(Config::new(json!({
            "database": {
                "default": "sqlite",
                "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
            },
        })));
        DatabaseServiceProvider.register(&container);
        Schema::create("cache", |table| {
            table.string("key").primary();
            table.medium_text("value");
            table.big_integer("expiration").index();
        })
        .await
        .unwrap();
        Schema::create("cache_locks", |table| {
            table.string("key").primary();
            table.string("owner");
            table.big_integer("expiration").index();
        })
        .await
        .unwrap();
        (container, guard)
    }

    fn store() -> DatabaseStore {
        DatabaseStore::new(DB::default_connection(), "cache").with_prefix("prefix_")
    }

    async fn row(table: &str, key: &str) -> Option<Value> {
        DB::table(table).where_("key", key).first().await.unwrap()
    }

    async fn keys(table: &str) -> Vec<Value> {
        DB::table(table)
            .order_by("key", "asc")
            .pluck("key")
            .await
            .unwrap()
            .into_iter()
            .collect()
    }

    #[tokio::test]
    async fn items_are_stored_as_prefixed_json_rows() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();

        assert!(
            store
                .put("user", json!({"name": "Taylor"}), 60)
                .await
                .unwrap()
        );
        assert_eq!(
            store.get("user").await.unwrap(),
            Some(json!({"name": "Taylor"}))
        );
        assert_eq!(
            row("cache", "prefix_user").await.unwrap(),
            json!({"key": "prefix_user", "value": r#"{"name":"Taylor"}"#, "expiration": NOW + 60})
        );
        assert_eq!(store.get("missing").await.unwrap(), None);
        assert_eq!(store.get_prefix(), "prefix_");
        assert_eq!(store.get_table(), "cache");
        assert_eq!(store.get_lock_table(), "cache_locks");
        assert_eq!(store.get_connection().get_name(), "sqlite");

        // Putting again overwrites the item (an upsert).
        store.put("user", json!("Abigail"), 120).await.unwrap();
        assert_eq!(store.get("user").await.unwrap(), Some(json!("Abigail")));
        assert_eq!(
            row("cache", "prefix_user").await.unwrap()["expiration"],
            NOW + 120
        );
        assert_eq!(DB::table("cache").count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn every_json_value_round_trips() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();
        for (key, value) in [
            ("null", json!(null)),
            ("bool", json!(false)),
            ("int", json!(42)),
            ("float", json!(1.5)),
            ("string", json!("1")),
            ("array", json!([1, "two", {"three": 3}])),
        ] {
            store.put(key, value.clone(), 60).await.unwrap();
            assert_eq!(store.get(key).await.unwrap(), Some(value), "{key}");
        }

        // Rows that don't hold JSON are misses.
        DB::table("cache")
            .insert(json!({"key": "prefix_php", "value": "a:0:{}", "expiration": i64::MAX}))
            .await
            .unwrap();
        assert_eq!(store.get("php").await.unwrap(), None);
    }

    #[tokio::test]
    async fn items_expire_and_are_cleaned_up() {
        let time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();

        store.put("short", json!("a"), 10).await.unwrap();
        store
            .put(
                &format!("{FLEXIBLE_CREATED_KEY_PREFIX}short"),
                json!(NOW),
                10,
            )
            .await
            .unwrap();
        store.put("long", json!("b"), 100).await.unwrap();

        time.travel_seconds(9);
        assert_eq!(store.get("short").await.unwrap(), Some(json!("a")));
        time.travel_seconds(1);
        assert_eq!(store.get("short").await.unwrap(), None);
        assert_eq!(keys("cache").await, vec![json!("prefix_long")]);

        assert!(store.forget_if_expired("long").await.unwrap());
        assert_eq!(store.get("long").await.unwrap(), Some(json!("b")));
        time.travel_seconds(100);
        store.forget_if_expired("long").await.unwrap();
        assert!(keys("cache").await.is_empty());
    }

    #[tokio::test]
    async fn many_items_are_retrieved_in_order() {
        let time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();

        assert!(store.many(&[]).await.unwrap().is_empty());
        assert!(
            store
                .put_many(vec![("a".into(), json!(1)), ("b".into(), json!(2))], 10)
                .await
                .unwrap()
        );
        assert!(store.put_many(Vec::new(), 10).await.unwrap());
        store.put("c", json!(3), 100).await.unwrap();

        let keys_wanted: Vec<String> = ["c", "missing", "a", "b"].map(String::from).to_vec();
        assert_eq!(
            store.many(&keys_wanted).await.unwrap(),
            vec![
                ("c".to_string(), Some(json!(3))),
                ("missing".to_string(), None),
                ("a".to_string(), Some(json!(1))),
                ("b".to_string(), Some(json!(2))),
            ]
        );

        time.travel_seconds(10);
        assert_eq!(
            store.many(&keys_wanted).await.unwrap(),
            vec![
                ("c".to_string(), Some(json!(3))),
                ("missing".to_string(), None),
                ("a".to_string(), None),
                ("b".to_string(), None),
            ]
        );
        assert_eq!(keys("cache").await, vec![json!("prefix_c")]);
    }

    #[tokio::test]
    async fn add_only_stores_missing_or_expired_items() {
        let time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();

        assert!(store.add("key", json!("first"), 10).await.unwrap());
        assert!(!store.add("key", json!("second"), 10).await.unwrap());
        assert_eq!(store.get("key").await.unwrap(), Some(json!("first")));

        time.travel_seconds(10);
        assert!(store.add("key", json!("third"), 10).await.unwrap());
        assert_eq!(store.get("key").await.unwrap(), Some(json!("third")));
    }

    #[tokio::test]
    async fn concurrent_adds_have_a_single_winner() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let store = store.clone();
                tokio::spawn(async move { store.add("race", json!(i), 60).await.unwrap() })
            })
            .collect();
        let mut winners = 0;
        for handle in handles {
            winners += usize::from(handle.await.unwrap());
        }
        assert_eq!(winners, 1);
        assert_eq!(DB::table("cache").count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn items_can_be_incremented_and_decremented() {
        let time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();

        // Missing items start at zero and are kept "forever".
        assert_eq!(store.increment("count", 1).await.unwrap(), 1);
        assert_eq!(store.increment("count", 5).await.unwrap(), 6);
        assert_eq!(store.decrement("count", 2).await.unwrap(), 4);
        assert_eq!(store.decrement("negative", 3).await.unwrap(), -3);
        let count = row("cache", "prefix_count").await.unwrap();
        assert_eq!(count["value"], "4");
        assert_eq!(count["expiration"], NOW + FOREVER_SECONDS as i64);

        // Incrementing keeps the item's lifetime.
        store.put("limited", json!(1), 30).await.unwrap();
        time.travel_seconds(10);
        assert_eq!(store.increment("limited", 1).await.unwrap(), 2);
        time.travel_seconds(20);
        assert_eq!(store.get("limited").await.unwrap(), None);

        // Expired items start over; non-numeric values count as zero.
        store.put("stale", json!(10), 5).await.unwrap();
        time.travel_seconds(5);
        assert_eq!(store.increment("stale", 1).await.unwrap(), 1);
        assert_eq!(store.get("stale").await.unwrap(), Some(json!(1)));
        store.put("numeric", json!("7"), 60).await.unwrap();
        assert_eq!(store.increment("numeric", 1).await.unwrap(), 8);
        store.put("words", json!("hello"), 60).await.unwrap();
        assert_eq!(store.increment("words", 2).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn concurrent_increments_are_not_lost() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let store = store.clone();
                tokio::spawn(async move { store.increment("hits", 1).await.unwrap() })
            })
            .collect();
        for handle in handles {
            handle.await.unwrap();
        }
        assert_eq!(store.get("hits").await.unwrap(), Some(json!(10)));
    }

    #[tokio::test]
    async fn items_can_be_stored_forever_and_touched() {
        let time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();

        assert!(store.forever("forever", json!("always")).await.unwrap());
        assert_eq!(
            row("cache", "prefix_forever").await.unwrap()["expiration"],
            NOW + 315_360_000
        );
        store.put("zero", json!(1), 0).await.unwrap();
        assert_eq!(
            row("cache", "prefix_zero").await.unwrap()["expiration"],
            NOW + 315_360_000
        );

        store.put("touched", json!(1), 10).await.unwrap();
        assert!(store.touch("touched", 100).await.unwrap());
        assert_eq!(
            row("cache", "prefix_touched").await.unwrap()["expiration"],
            NOW + 100
        );
        time.travel_seconds(50);
        assert_eq!(store.get("touched").await.unwrap(), Some(json!(1)));
        assert!(!store.touch("missing", 100).await.unwrap());

        time.travel_seconds(50);
        assert!(
            !store.touch("touched", 100).await.unwrap(),
            "expired items can't be touched"
        );
    }

    #[tokio::test]
    async fn items_can_be_forgotten_and_flushed() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();
        store.put("a", json!(1), 60).await.unwrap();
        store
            .put(&format!("{FLEXIBLE_CREATED_KEY_PREFIX}a"), json!(1), 60)
            .await
            .unwrap();
        store.put("b", json!(2), 60).await.unwrap();

        assert!(store.forget("a").await.unwrap());
        assert_eq!(keys("cache").await, vec![json!("prefix_b")]);
        assert!(store.forget("a").await.unwrap(), "forgetting is idempotent");

        assert!(store.flush().await.unwrap());
        assert_eq!(DB::table("cache").count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_store_works_behind_a_repository() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let cache = Repository::new(store());
        let value: String = cache
            .remember("name", 60, || async { Ok("Taylor".to_string()) })
            .await
            .unwrap();
        assert_eq!(value, "Taylor");
        assert!(cache.has("name").await.unwrap());
        assert!(!cache.add("name", "Abigail", 60).await.unwrap());
        assert_eq!(cache.increment("visits").await.unwrap(), 1);
        cache.put("gone", 1, 0).await.unwrap();
        assert!(cache.missing("gone").await.unwrap());
        assert_eq!(cache.pull("name").await.unwrap(), Some(json!("Taylor")));
        assert!(cache.missing("name").await.unwrap());
    }

    #[tokio::test]
    async fn locks_are_rows_in_the_lock_table() {
        let time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();

        let lock = store.lock("report", 10, None);
        assert_eq!(lock.name(), "prefix_report");
        assert!(lock.get().await.unwrap());
        assert_eq!(
            row("cache_locks", "prefix_report").await.unwrap(),
            json!({"key": "prefix_report", "owner": lock.owner(), "expiration": NOW + 10})
        );

        // Someone else is turned away...
        let other = store.lock("report", 10, None);
        assert!(!other.get().await.unwrap());
        assert!(
            !other.release().await.unwrap(),
            "only the owner may release"
        );
        assert!(lock.is_locked().await.unwrap());
        assert!(lock.is_owned_by_current_process().await.unwrap());
        assert!(!other.is_owned_by_current_process().await.unwrap());

        // ...but the owner may re-acquire (extending it).
        time.travel_seconds(5);
        assert!(lock.acquire().await.unwrap());
        assert_eq!(
            row("cache_locks", "prefix_report").await.unwrap()["expiration"],
            NOW + 15
        );

        // Restored locks act on behalf of the original owner.
        let restored = store.restore_lock("report", lock.owner());
        assert_eq!(restored.seconds(), 0);
        assert!(restored.is_owned_by_current_process().await.unwrap());
        assert!(restored.release().await.unwrap());
        assert!(!lock.is_locked().await.unwrap());
        assert!(other.get().await.unwrap());
        other.force_release().await.unwrap();
        assert!(row("cache_locks", "prefix_report").await.is_none());
    }

    #[tokio::test]
    async fn expired_locks_can_be_taken_over() {
        let time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store().with_lock_lottery(None);

        let first = store.lock("job", 10, None);
        assert!(first.get().await.unwrap());
        time.travel_seconds(10);
        assert!(!first.is_locked().await.unwrap(), "the lock expired");
        assert!(!first.is_owned_by_current_process().await.unwrap());

        let second = store.lock("job", 10, None);
        assert!(second.get().await.unwrap());
        assert_eq!(
            row("cache_locks", "prefix_job").await.unwrap()["owner"],
            json!(second.owner())
        );
        assert!(!first.release().await.unwrap());
        assert!(second.is_locked().await.unwrap());
    }

    #[tokio::test]
    async fn locks_without_a_duration_use_the_default_timeout() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;

        assert!(store().lock("a", 0, None).get().await.unwrap());
        assert_eq!(
            row("cache_locks", "prefix_a").await.unwrap()["expiration"],
            NOW + 86_400
        );

        let store = store().with_default_lock_timeout(60);
        assert!(store.lock("b", 0, None).get().await.unwrap());
        assert_eq!(
            row("cache_locks", "prefix_b").await.unwrap()["expiration"],
            NOW + 60
        );
    }

    #[tokio::test]
    async fn locks_can_be_refreshed_while_owned() {
        let time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();

        let lock = store.lock("long", 10, None);
        assert!(lock.get().await.unwrap());
        time.travel_seconds(8);
        assert!(lock.refresh(None).await.unwrap());
        assert_eq!(
            row("cache_locks", "prefix_long").await.unwrap()["expiration"],
            NOW + 18
        );
        assert!(lock.refresh(Some(100)).await.unwrap());
        assert_eq!(
            row("cache_locks", "prefix_long").await.unwrap()["expiration"],
            NOW + 108
        );
        assert!(!store.lock("long", 10, None).refresh(None).await.unwrap());

        time.travel_seconds(100);
        assert!(
            !lock.refresh(None).await.unwrap(),
            "expired locks can't be refreshed"
        );
    }

    #[tokio::test]
    async fn the_lottery_prunes_expired_locks() {
        let time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;

        let never = store().with_lock_lottery(None);
        assert!(never.lock("old", 10, None).get().await.unwrap());
        time.travel_seconds(10);
        assert!(never.lock("new", 10, None).get().await.unwrap());
        assert_eq!(keys("cache_locks").await.len(), 2);
        let losing = store().with_lock_lottery(Some([0, 100]));
        assert!(losing.lock("other", 10, None).get().await.unwrap());
        assert_eq!(keys("cache_locks").await.len(), 3);

        let always = store().with_lock_lottery(Some([1, 1]));
        assert!(always.lock("fresh", 10, None).get().await.unwrap());
        assert_eq!(
            keys("cache_locks").await,
            vec![
                json!("prefix_fresh"),
                json!("prefix_new"),
                json!("prefix_other")
            ]
        );

        time.travel_seconds(10);
        always.lock_driver().prune_expired_locks().await.unwrap();
        assert!(keys("cache_locks").await.is_empty());
        assert_eq!(always.lock_driver().get_connection_name(), "sqlite");
    }

    #[tokio::test]
    async fn lock_contention_has_a_single_winner() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let lock = store.lock("contended", 10, None);
                tokio::spawn(async move { lock.get().await.unwrap() })
            })
            .collect();
        let mut winners = 0;
        for handle in handles {
            winners += usize::from(handle.await.unwrap());
        }
        assert_eq!(winners, 1);
    }

    #[tokio::test]
    async fn blocked_locks_wait_for_the_owner() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let cache = Repository::new(store());

        let lock = cache.lock("shared", 10);
        assert!(lock.get().await.unwrap());
        let waiting = cache
            .lock("shared", 10)
            .between_blocked_attempts_sleep_for(10);
        let error = waiting.block(0).await.unwrap_err();
        assert_eq!(error.to_string(), "Unable to acquire lock [prefix_shared].");

        lock.release().await.unwrap();
        assert_eq!(waiting.block_with(1, || async { 42 }).await.unwrap(), 42);
        assert!(!waiting.is_locked().await.unwrap());
    }

    #[tokio::test]
    async fn locks_can_be_flushed_from_a_separate_table() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = store();
        assert!(store.has_separate_lock_store());
        assert!(store.lock("a", 0, None).get().await.unwrap());
        assert!(store.flush_locks().await.unwrap());
        assert!(keys("cache_locks").await.is_empty());

        let shared = store.with_lock_table("cache");
        assert!(!shared.has_separate_lock_store());
        assert_eq!(
            shared.flush_locks().await.unwrap_err().to_string(),
            "Flushing locks is only supported when the lock store is separate from the cache store."
        );
    }

    #[tokio::test]
    async fn database_errors_are_reported() {
        let _time = freeze_time(Carbon::from_timestamp(NOW));
        let _app = app().await;
        let store = DatabaseStore::new(DB::default_connection(), "missing_table")
            .with_lock_table("missing_locks");
        assert!(store.get("a").await.is_err());
        assert!(store.put("a", json!(1), 10).await.is_err());
        assert!(store.increment("a", 1).await.is_err());
        let error = store.lock("a", 10, None).get().await.unwrap_err();
        assert!(error.downcast_ref::<QueryException>().is_some());
        assert!(error.to_string().contains("missing_locks"), "{error}");
    }
}
