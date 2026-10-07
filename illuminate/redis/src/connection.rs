//! A Redis connection.

use std::sync::{Arc, Mutex, RwLock};

use indexmap::IndexMap;
use redis::aio::{ConnectionManager, ConnectionManagerConfig, MultiplexedConnection};
use redis::{
    AsyncConnectionConfig, ErrorKind, FromRedisValue, RedisError, RedisResult, ToRedisArgs,
};

use illuminate_support::{Result, Value};

use crate::args::{CommandArgs, concat};
use crate::config::ConnectionConfig;
use crate::keys::prefix_keys;
use crate::limiters::{ConcurrencyLimiterBuilder, DurationLimiterBuilder};
use crate::pipeline::{Pipeline, build_command};
use crate::pubsub::{Subscription, listen};
use crate::value::{from_redis, is_ok, pairs};

/// Commands that may safely be retried once after the connection was lost
/// (the reads Laravel's `PhpRedisConnection` retries).
#[rustfmt::skip]
const RETRYABLE_COMMANDS: &[&str] = &[
    "bitcount", "bitpos", "dbsize", "dump", "exists", "geodist", "geohash", "geopos", "geosearch",
    "get", "getbit", "getrange", "hexists", "hget", "hgetall", "hkeys", "hlen", "hmget", "hmset",
    "hstrlen", "hvals", "keys", "lindex", "llen", "lpos", "lrange", "mget", "mset", "ping",
    "pttl", "randomkey", "scard", "sdiff", "sinter", "sismember", "smembers", "smismember",
    "srandmember", "strlen", "sunion", "time", "ttl", "type", "xinfo", "xlen", "xpending",
    "xrange", "xrevrange", "zcard", "zcount", "zlexcount", "zmscore", "zrange", "zrank",
    "zrevrank", "zscore",
];

/// Commands that block the connection while they wait: they run on
/// dedicated connections, so they never hold up anyone else's commands.
#[rustfmt::skip]
const BLOCKING_COMMANDS: &[&str] = &[
    "blpop", "brpop", "brpoplpush", "blmove", "blmpop", "bzpopmin", "bzpopmax", "bzmpop",
];

/// The most idle blocking connections kept around for reuse.
const MAX_IDLE_BLOCKING_CONNECTIONS: usize = 8;

struct Inner {
    name: String,
    config: ConnectionConfig,
    client: redis::Client,
    manager: RwLock<Option<ConnectionManager>>,
    connecting: tokio::sync::Mutex<()>,
    blocking: Mutex<Vec<MultiplexedConnection>>,
}

/// A connection to a Redis server.
///
/// Connections are cheap to clone: clones share the same auto-reconnecting
/// multiplexed connection, which is established on the first command. Every
/// key is prefixed with the connection's `prefix`, just like phpredis'
/// `OPT_PREFIX`.
///
/// ```no_run
/// use illuminate_redis::Redis;
///
/// # async fn example() -> illuminate_support::Result<()> {
/// let redis = Redis::connection("cache")?;
///
/// redis.set("name", "Taylor").await?;
/// assert_eq!(redis.get("name").await?.as_deref(), Some("Taylor"));
///
/// let values = redis.lrange("names", 5, 10).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("name", &self.inner.name)
            .field("host", &self.inner.config.host)
            .field("port", &self.inner.config.port)
            .field("database", &self.inner.config.database)
            .field("prefix", &self.inner.config.prefix)
            .finish()
    }
}

impl Connection {
    /// Create a connection. Nothing is sent to the server until the first
    /// command.
    ///
    /// ```
    /// use illuminate_redis::{Connection, ConnectionConfig};
    ///
    /// let connection = Connection::new("default", ConnectionConfig::default()).unwrap();
    /// assert_eq!(connection.name(), "default");
    /// ```
    pub fn new(name: impl Into<String>, config: ConnectionConfig) -> Result<Self> {
        let client = redis::Client::open(config.connection_info()?)?;
        Ok(Self {
            inner: Arc::new(Inner {
                name: name.into(),
                config,
                client,
                manager: RwLock::new(None),
                connecting: tokio::sync::Mutex::new(()),
                blocking: Mutex::new(Vec::new()),
            }),
        })
    }

    /// The name of the connection.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The prefix applied to every key.
    pub fn prefix(&self) -> &str {
        &self.inner.config.prefix
    }

    /// The connection's configuration.
    pub fn config(&self) -> &ConnectionConfig {
        &self.inner.config
    }

    /// Determine if both handles share the same underlying connection.
    pub fn same_as(&self, other: &Connection) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// The underlying redis-rs connection, connecting if necessary.
    pub async fn client(&self) -> Result<ConnectionManager> {
        Ok(self.manager().await?)
    }

    /// Close the connection; the next command reconnects.
    pub fn disconnect(&self) {
        self.inner.manager.write().unwrap().take();
        self.inner.blocking.lock().unwrap().clear();
    }

    async fn manager(&self) -> RedisResult<ConnectionManager> {
        if let Some(manager) = self.inner.manager.read().unwrap().clone() {
            return Ok(manager);
        }
        let _connecting = self.inner.connecting.lock().await;
        if let Some(manager) = self.inner.manager.read().unwrap().clone() {
            return Ok(manager);
        }

        let config = &self.inner.config;
        let options = ConnectionManagerConfig::new()
            .set_connection_timeout(config.timeout)
            .set_response_timeout(config.read_timeout)
            .set_number_of_retries(config.max_retries)
            .set_min_delay(config.backoff_base)
            .set_max_delay(config.backoff_cap);
        let manager =
            ConnectionManager::new_with_config(self.inner.client.clone(), options).await?;
        *self.inner.manager.write().unwrap() = Some(manager.clone());
        Ok(manager)
    }

    // ------------------------------------------------------------------
    // Running commands
    // ------------------------------------------------------------------

    /// Run any Redis command, returning its reply as a [`Value`]
    /// (Laravel's `Redis::command`).
    ///
    /// ```no_run
    /// # async fn example(redis: illuminate_redis::Connection) -> illuminate_support::Result<()> {
    /// let values = redis.command("lrange", ("names", 5, 10)).await?;
    /// let pong = redis.command("ping", ()).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn command(&self, method: &str, args: impl CommandArgs) -> Result<Value> {
        Ok(from_redis(self.send(method, args.into_args()).await?))
    }

    /// Run any Redis command, converting its reply with redis-rs'
    /// [`FromRedisValue`] — handy for binary data and custom types.
    ///
    /// ```no_run
    /// # async fn example(redis: illuminate_redis::Connection) -> illuminate_support::Result<()> {
    /// let avatar: Option<Vec<u8>> = redis.query("get", "avatar:1").await?;
    /// let scores: Vec<(String, f64)> = redis.query("zrange", ("board", 0, -1, "WITHSCORES")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query<T: FromRedisValue>(
        &self,
        method: &str,
        args: impl CommandArgs,
    ) -> Result<T> {
        let reply = self.send(method, args.into_args()).await?;
        Ok(redis::from_redis_value(reply)?)
    }

    /// Run a raw command, exactly as given: the first argument is the
    /// command's name, and no keys are prefixed.
    pub async fn execute_raw(&self, args: impl CommandArgs) -> Result<Value> {
        let mut args = args.into_args();
        if args.is_empty() {
            return Ok(Value::Null);
        }
        let method = String::from_utf8_lossy(&args.remove(0)).into_owned();
        let command = build_command(&method, args);
        Ok(from_redis(self.dispatch(&method, &command, false).await?))
    }

    /// Prefix the command's keys and send it.
    async fn send(&self, method: &str, mut args: Vec<Vec<u8>>) -> RedisResult<redis::Value> {
        prefix_keys(method, &mut args, self.prefix());
        let retryable = is_retryable(method, &args);
        let command = build_command(method, args);
        self.dispatch(method, &command, retryable).await
    }

    /// Send a command, retrying it on a lost connection when that is safe.
    async fn dispatch(
        &self,
        method: &str,
        command: &redis::Cmd,
        retryable: bool,
    ) -> RedisResult<redis::Value> {
        if BLOCKING_COMMANDS.contains(&method.to_ascii_lowercase().as_str()) {
            return self.dispatch_blocking(command).await;
        }

        let mut retries = usize::from(retryable).max(self.inner.config.command_retries);
        loop {
            let mut manager = self.manager().await?;
            match command.query_async::<redis::Value>(&mut manager).await {
                Err(error) if retries > 0 && caused_by_lost_connection(&error) => retries -= 1,
                result => return result,
            }
        }
    }

    /// Run a blocking command on a dedicated connection.
    async fn dispatch_blocking(&self, command: &redis::Cmd) -> RedisResult<redis::Value> {
        let idle = self.inner.blocking.lock().unwrap().pop();
        let mut connection = match idle {
            Some(connection) => connection,
            None => {
                let options = AsyncConnectionConfig::new()
                    .set_connection_timeout(self.inner.config.timeout)
                    .set_response_timeout(None);
                self.inner
                    .client
                    .get_multiplexed_async_connection_with_config(&options)
                    .await?
            }
        };
        let result = command.query_async::<redis::Value>(&mut connection).await;
        if result.is_ok() {
            let mut idle = self.inner.blocking.lock().unwrap();
            if idle.len() < MAX_IDLE_BLOCKING_CONNECTIONS {
                idle.push(connection);
            }
        }
        result
    }

    /// Strip the connection's prefix from a key returned by the server.
    fn unprefixed(&self, key: String) -> String {
        match key.strip_prefix(self.prefix()) {
            Some(key) if !self.prefix().is_empty() => key.to_string(),
            _ => key,
        }
    }

    // ------------------------------------------------------------------
    // Strings & keys
    // ------------------------------------------------------------------

    /// Get the value of a key.
    pub async fn get(&self, key: impl AsRef<str>) -> Result<Option<String>> {
        self.query("get", (key.as_ref(),)).await
    }

    /// Set the value of a key.
    pub async fn set(&self, key: impl AsRef<str>, value: impl ToRedisArgs) -> Result<bool> {
        let reply: redis::Value = self.query("set", (key.as_ref(), value)).await?;
        Ok(is_ok(&reply))
    }

    /// Set the value of a key, expiring after the given number of seconds.
    pub async fn set_ex(
        &self,
        key: impl AsRef<str>,
        value: impl ToRedisArgs,
        seconds: u64,
    ) -> Result<bool> {
        let reply: redis::Value = self.query("setex", (key.as_ref(), seconds, value)).await?;
        Ok(is_ok(&reply))
    }

    /// Set the value of a key, only if it doesn't exist yet.
    pub async fn setnx(&self, key: impl AsRef<str>, value: impl ToRedisArgs) -> Result<bool> {
        let reply: redis::Value = self.query("setnx", (key.as_ref(), value)).await?;
        Ok(is_ok(&reply))
    }

    /// Delete one or more keys, returning how many were deleted.
    pub async fn del(&self, keys: impl CommandArgs) -> Result<i64> {
        self.query("del", keys).await
    }

    /// Count how many of the given keys exist.
    pub async fn exists(&self, keys: impl CommandArgs) -> Result<i64> {
        self.query("exists", keys).await
    }

    /// Increment a key by one.
    pub async fn incr(&self, key: impl AsRef<str>) -> Result<i64> {
        self.query("incr", (key.as_ref(),)).await
    }

    /// Increment a key by the given amount.
    pub async fn incrby(&self, key: impl AsRef<str>, amount: i64) -> Result<i64> {
        self.query("incrby", (key.as_ref(), amount)).await
    }

    /// Decrement a key by one.
    pub async fn decr(&self, key: impl AsRef<str>) -> Result<i64> {
        self.query("decr", (key.as_ref(),)).await
    }

    /// Decrement a key by the given amount.
    pub async fn decrby(&self, key: impl AsRef<str>, amount: i64) -> Result<i64> {
        self.query("decrby", (key.as_ref(), amount)).await
    }

    /// Expire a key after the given number of seconds.
    pub async fn expire(&self, key: impl AsRef<str>, seconds: i64) -> Result<bool> {
        let reply: redis::Value = self.query("expire", (key.as_ref(), seconds)).await?;
        Ok(is_ok(&reply))
    }

    /// The remaining time to live of a key, in seconds (`-1` when it never
    /// expires, `-2` when it doesn't exist).
    pub async fn ttl(&self, key: impl AsRef<str>) -> Result<i64> {
        self.query("ttl", (key.as_ref(),)).await
    }

    /// Find the keys matching a pattern. The connection's prefix is applied
    /// to the pattern and removed from the keys that are returned.
    pub async fn keys(&self, pattern: impl AsRef<str>) -> Result<Vec<String>> {
        let keys: Vec<String> = self.query("keys", (pattern.as_ref(),)).await?;
        Ok(keys.into_iter().map(|key| self.unprefixed(key)).collect())
    }

    /// Get the values of several keys (`None` for missing keys).
    pub async fn mget(&self, keys: impl CommandArgs) -> Result<Vec<Option<String>>> {
        let keys = keys.into_args();
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        self.query("mget", keys).await
    }

    /// Set several keys at once.
    pub async fn mset<K, V>(&self, values: impl IntoIterator<Item = (K, V)>) -> Result<bool>
    where
        K: AsRef<str>,
        V: ToRedisArgs,
    {
        let args = concat(
            values
                .into_iter()
                .map(|(key, value)| (key.as_ref(), value).into_args()),
        );
        if args.is_empty() {
            return Ok(true);
        }
        let reply: redis::Value = self.query("mset", args).await?;
        Ok(is_ok(&reply))
    }

    // ------------------------------------------------------------------
    // Hashes
    // ------------------------------------------------------------------

    /// Get a field of a hash.
    pub async fn hget(
        &self,
        key: impl AsRef<str>,
        field: impl AsRef<str>,
    ) -> Result<Option<String>> {
        self.query("hget", (key.as_ref(), field.as_ref())).await
    }

    /// Set a field of a hash, returning `1` when the field is new.
    pub async fn hset(
        &self,
        key: impl AsRef<str>,
        field: impl AsRef<str>,
        value: impl ToRedisArgs,
    ) -> Result<i64> {
        self.query("hset", (key.as_ref(), field.as_ref(), value))
            .await
    }

    /// Get every field of a hash.
    pub async fn hgetall(&self, key: impl AsRef<str>) -> Result<IndexMap<String, String>> {
        let reply: redis::Value = self.query("hgetall", (key.as_ref(),)).await?;
        pairs(reply)
            .into_iter()
            .map(|(field, value)| {
                Ok((
                    redis::from_redis_value(field)?,
                    redis::from_redis_value(value)?,
                ))
            })
            .collect()
    }

    /// Delete one or more fields of a hash.
    pub async fn hdel(&self, key: impl AsRef<str>, fields: impl CommandArgs) -> Result<i64> {
        self.query(
            "hdel",
            concat([(key.as_ref(),).into_args(), fields.into_args()]),
        )
        .await
    }

    /// Determine if a hash has the given field.
    pub async fn hexists(&self, key: impl AsRef<str>, field: impl AsRef<str>) -> Result<bool> {
        self.query("hexists", (key.as_ref(), field.as_ref())).await
    }

    /// Increment a field of a hash by the given amount.
    pub async fn hincrby(
        &self,
        key: impl AsRef<str>,
        field: impl AsRef<str>,
        amount: i64,
    ) -> Result<i64> {
        self.query("hincrby", (key.as_ref(), field.as_ref(), amount))
            .await
    }

    /// The field names of a hash.
    pub async fn hkeys(&self, key: impl AsRef<str>) -> Result<Vec<String>> {
        self.query("hkeys", (key.as_ref(),)).await
    }

    /// The number of fields in a hash.
    pub async fn hlen(&self, key: impl AsRef<str>) -> Result<i64> {
        self.query("hlen", (key.as_ref(),)).await
    }

    // ------------------------------------------------------------------
    // Lists
    // ------------------------------------------------------------------

    /// Prepend one or more values to a list, returning its new length.
    pub async fn lpush(&self, key: impl AsRef<str>, values: impl CommandArgs) -> Result<i64> {
        self.query(
            "lpush",
            concat([(key.as_ref(),).into_args(), values.into_args()]),
        )
        .await
    }

    /// Append one or more values to a list, returning its new length.
    pub async fn rpush(&self, key: impl AsRef<str>, values: impl CommandArgs) -> Result<i64> {
        self.query(
            "rpush",
            concat([(key.as_ref(),).into_args(), values.into_args()]),
        )
        .await
    }

    /// Remove and return the first value of a list.
    pub async fn lpop(&self, key: impl AsRef<str>) -> Result<Option<String>> {
        self.query("lpop", (key.as_ref(),)).await
    }

    /// Remove and return the last value of a list.
    pub async fn rpop(&self, key: impl AsRef<str>) -> Result<Option<String>> {
        self.query("rpop", (key.as_ref(),)).await
    }

    /// The values of a list between two indexes (`-1` is the last value).
    pub async fn lrange(&self, key: impl AsRef<str>, start: i64, stop: i64) -> Result<Vec<String>> {
        self.query("lrange", (key.as_ref(), start, stop)).await
    }

    /// The length of a list.
    pub async fn llen(&self, key: impl AsRef<str>) -> Result<i64> {
        self.query("llen", (key.as_ref(),)).await
    }

    /// The value of a list at the given index.
    pub async fn lindex(&self, key: impl AsRef<str>, index: i64) -> Result<Option<String>> {
        self.query("lindex", (key.as_ref(), index)).await
    }

    /// Remove `count` occurrences of the value from a list (`0` removes them all).
    pub async fn lrem(
        &self,
        key: impl AsRef<str>,
        count: i64,
        value: impl ToRedisArgs,
    ) -> Result<i64> {
        self.query("lrem", (key.as_ref(), count, value)).await
    }

    /// Remove and return the first value of the first non-empty list,
    /// waiting up to `timeout` seconds (`0` waits forever). Returns the
    /// list's key and the value.
    pub async fn blpop(
        &self,
        keys: impl CommandArgs,
        timeout: f64,
    ) -> Result<Option<(String, String)>> {
        self.blocking_pop("blpop", keys, timeout).await
    }

    /// Remove and return the last value of the first non-empty list,
    /// waiting up to `timeout` seconds (`0` waits forever).
    pub async fn brpop(
        &self,
        keys: impl CommandArgs,
        timeout: f64,
    ) -> Result<Option<(String, String)>> {
        self.blocking_pop("brpop", keys, timeout).await
    }

    async fn blocking_pop(
        &self,
        method: &str,
        keys: impl CommandArgs,
        timeout: f64,
    ) -> Result<Option<(String, String)>> {
        let args = concat([keys.into_args(), (timeout.max(0.0),).into_args()]);
        let popped: Option<(String, String)> = self.query(method, args).await?;
        Ok(popped.map(|(key, value)| (self.unprefixed(key), value)))
    }

    // ------------------------------------------------------------------
    // Sets
    // ------------------------------------------------------------------

    /// Add one or more members to a set, returning how many were new.
    pub async fn sadd(&self, key: impl AsRef<str>, members: impl CommandArgs) -> Result<i64> {
        self.query(
            "sadd",
            concat([(key.as_ref(),).into_args(), members.into_args()]),
        )
        .await
    }

    /// Remove one or more members from a set.
    pub async fn srem(&self, key: impl AsRef<str>, members: impl CommandArgs) -> Result<i64> {
        self.query(
            "srem",
            concat([(key.as_ref(),).into_args(), members.into_args()]),
        )
        .await
    }

    /// The members of a set.
    pub async fn smembers(&self, key: impl AsRef<str>) -> Result<Vec<String>> {
        self.query("smembers", (key.as_ref(),)).await
    }

    /// Determine if the value is a member of a set.
    pub async fn sismember(&self, key: impl AsRef<str>, member: impl ToRedisArgs) -> Result<bool> {
        self.query("sismember", (key.as_ref(), member)).await
    }

    /// The number of members in a set.
    pub async fn scard(&self, key: impl AsRef<str>) -> Result<i64> {
        self.query("scard", (key.as_ref(),)).await
    }

    // ------------------------------------------------------------------
    // Sorted sets
    // ------------------------------------------------------------------

    /// Add a member to a sorted set (or update its score), returning `1`
    /// when the member is new.
    pub async fn zadd(
        &self,
        key: impl AsRef<str>,
        score: f64,
        member: impl ToRedisArgs,
    ) -> Result<i64> {
        self.query("zadd", (key.as_ref(), score, member)).await
    }

    /// Increment the score of a member of a sorted set, returning the new score.
    pub async fn zincrby(
        &self,
        key: impl AsRef<str>,
        amount: f64,
        member: impl ToRedisArgs,
    ) -> Result<f64> {
        self.query("zincrby", (key.as_ref(), amount, member)).await
    }

    /// The members of a sorted set between two ranks, lowest score first.
    pub async fn zrange(&self, key: impl AsRef<str>, start: i64, stop: i64) -> Result<Vec<String>> {
        self.query("zrange", (key.as_ref(), start, stop)).await
    }

    /// The members of a sorted set between two ranks, with their scores.
    pub async fn zrange_with_scores(
        &self,
        key: impl AsRef<str>,
        start: i64,
        stop: i64,
    ) -> Result<Vec<(String, f64)>> {
        let reply: redis::Value = self
            .query("zrange", (key.as_ref(), start, stop, "WITHSCORES"))
            .await?;
        let entries = match reply {
            // RESP3 replies with [member, score] pairs...
            redis::Value::Array(items)
                if items
                    .iter()
                    .all(|item| matches!(item, redis::Value::Array(_))) =>
            {
                items.into_iter().flat_map(pairs).collect()
            }
            reply => pairs(reply),
        };
        entries
            .into_iter()
            .map(|(member, score)| {
                Ok((
                    redis::from_redis_value(member)?,
                    redis::from_redis_value(score)?,
                ))
            })
            .collect()
    }

    /// The members of a sorted set with a score between `min` and `max`
    /// (`"-inf"`, `"+inf"` and exclusive `"(5"` bounds work too).
    pub async fn zrangebyscore(
        &self,
        key: impl AsRef<str>,
        min: impl ToRedisArgs,
        max: impl ToRedisArgs,
    ) -> Result<Vec<String>> {
        self.query("zrangebyscore", (key.as_ref(), min, max)).await
    }

    /// Remove one or more members from a sorted set.
    pub async fn zrem(&self, key: impl AsRef<str>, members: impl CommandArgs) -> Result<i64> {
        self.query(
            "zrem",
            concat([(key.as_ref(),).into_args(), members.into_args()]),
        )
        .await
    }

    /// The number of members in a sorted set.
    pub async fn zcard(&self, key: impl AsRef<str>) -> Result<i64> {
        self.query("zcard", (key.as_ref(),)).await
    }

    /// The score of a member of a sorted set.
    pub async fn zscore(
        &self,
        key: impl AsRef<str>,
        member: impl ToRedisArgs,
    ) -> Result<Option<f64>> {
        self.query("zscore", (key.as_ref(), member)).await
    }

    // ------------------------------------------------------------------
    // Server
    // ------------------------------------------------------------------

    /// Publish a message to a channel, returning how many subscribers
    /// received it.
    pub async fn publish(
        &self,
        channel: impl AsRef<str>,
        message: impl ToRedisArgs,
    ) -> Result<i64> {
        self.query("publish", (channel.as_ref(), message)).await
    }

    /// Delete every key of the connection's database.
    pub async fn flushdb(&self) -> Result<bool> {
        let reply: redis::Value = self.query("flushdb", ()).await?;
        Ok(is_ok(&reply))
    }

    // ------------------------------------------------------------------
    // Scripts, pipelines & transactions
    // ------------------------------------------------------------------

    /// Evaluate a Lua script atomically on the server. `keys` are prefixed
    /// and available to the script as `KEYS`, `args` as `ARGV`.
    ///
    /// Scripts are cached by the server and called by their SHA1 after the
    /// first call.
    ///
    /// ```no_run
    /// use illuminate_redis::Redis;
    ///
    /// # async fn example() -> illuminate_support::Result<()> {
    /// let value = Redis::eval(r#"
    ///     local counter = redis.call("incr", KEYS[1])
    ///
    ///     if counter > 5 then
    ///         redis.call("incr", KEYS[2])
    ///     end
    ///
    ///     return counter
    /// "#, ("first-counter", "second-counter"), ()).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn eval(
        &self,
        script: &str,
        keys: impl CommandArgs,
        args: impl CommandArgs,
    ) -> Result<Value> {
        let keys = keys.into_args();
        let arguments = concat([(keys.len(),).into_args(), keys, args.into_args()]);
        let hash = redis::Script::new(script).get_hash().to_string();

        let reply = match self
            .send("evalsha", concat([(hash,).into_args(), arguments.clone()]))
            .await
        {
            Err(error) if error.kind() == ErrorKind::Server(redis::ServerErrorKind::NoScript) => {
                self.send("eval", concat([(script,).into_args(), arguments]))
                    .await?
            }
            reply => reply?,
        };
        Ok(from_redis(reply))
    }

    /// Send several commands in a single round trip, returning their
    /// replies in order.
    ///
    /// ```no_run
    /// use illuminate_redis::Redis;
    ///
    /// # async fn example() -> illuminate_support::Result<()> {
    /// Redis::pipeline(|pipe| {
    ///     for i in 0..1000 {
    ///         pipe.set(format!("key:{i}"), i);
    ///     }
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn pipeline(&self, callback: impl FnOnce(&mut Pipeline)) -> Result<Vec<Value>> {
        self.run_pipeline(callback, false).await
    }

    /// Run several commands in a single, atomic `MULTI` / `EXEC`
    /// transaction, returning their replies in order.
    ///
    /// The commands are only queued inside the closure, so their values
    /// can't be read there — reach for [`eval`](Connection::eval) when the
    /// transaction needs to inspect values.
    ///
    /// ```no_run
    /// use illuminate_redis::Redis;
    ///
    /// # async fn example() -> illuminate_support::Result<()> {
    /// Redis::transaction(|redis| {
    ///     redis.incr("user_visits");
    ///     redis.incr("total_visits");
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn transaction(&self, callback: impl FnOnce(&mut Pipeline)) -> Result<Vec<Value>> {
        self.run_pipeline(callback, true).await
    }

    async fn run_pipeline(
        &self,
        callback: impl FnOnce(&mut Pipeline),
        atomic: bool,
    ) -> Result<Vec<Value>> {
        let mut pipeline = Pipeline::new(self.prefix());
        callback(&mut pipeline);
        if pipeline.is_empty() {
            return Ok(Vec::new());
        }
        let pipe = pipeline.into_pipe(atomic);
        let mut manager = self.manager().await?;
        let replies: Vec<redis::Value> = pipe.query_async(&mut manager).await?;
        Ok(replies.into_iter().map(from_redis).collect())
    }

    // ------------------------------------------------------------------
    // Pub / Sub
    // ------------------------------------------------------------------

    /// Listen for messages published to the given channels, on a dedicated
    /// connection. The callback receives each message and its channel.
    ///
    /// ```no_run
    /// use illuminate_redis::Redis;
    ///
    /// # async fn example() -> illuminate_support::Result<()> {
    /// let subscription = Redis::subscribe(["test-channel"], |message, channel| {
    ///     println!("{channel}: {message}");
    /// }).await?;
    ///
    /// Redis::publish("test-channel", r#"{"name":"Adam Wathan"}"#).await?;
    ///
    /// subscription.unsubscribe().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn subscribe<F>(
        &self,
        channels: impl CommandArgs,
        callback: F,
    ) -> Result<Subscription>
    where
        F: FnMut(String, String) + Send + 'static,
    {
        listen(
            &self.inner.client,
            self.prefix(),
            names(channels),
            false,
            callback,
        )
        .await
    }

    /// Listen for messages published to every channel matching the given
    /// patterns (`*`, `users.*`, ...). The callback receives each message and
    /// the channel it was published to.
    pub async fn psubscribe<F>(
        &self,
        patterns: impl CommandArgs,
        callback: F,
    ) -> Result<Subscription>
    where
        F: FnMut(String, String) + Send + 'static,
    {
        listen(
            &self.inner.client,
            self.prefix(),
            names(patterns),
            true,
            callback,
        )
        .await
    }

    // ------------------------------------------------------------------
    // Limiters
    // ------------------------------------------------------------------

    /// Throttle a callback to a number of executions per window of time.
    pub fn throttle(&self, name: impl Into<String>) -> DurationLimiterBuilder {
        DurationLimiterBuilder::new(Some(self.clone()), name)
    }

    /// Funnel a callback to a number of simultaneous executions.
    pub fn funnel(&self, name: impl Into<String>) -> ConcurrencyLimiterBuilder {
        ConcurrencyLimiterBuilder::new(Some(self.clone()), name)
    }
}

/// Channel names from command arguments.
fn names(channels: impl CommandArgs) -> Vec<String> {
    channels
        .into_args()
        .into_iter()
        .map(|channel| String::from_utf8_lossy(&channel).into_owned())
        .collect()
}

/// Determine if a command may safely be retried after a lost connection.
fn is_retryable(method: &str, args: &[Vec<u8>]) -> bool {
    let method = method.to_ascii_lowercase();
    if method == "set" {
        // A plain SET is idempotent; one with options (NX, GET, ...) isn't.
        return args.len() <= 2;
    }
    RETRYABLE_COMMANDS.contains(&method.as_str())
}

/// Determine if the error means the connection to the server was lost.
fn caused_by_lost_connection(error: &RedisError) -> bool {
    error.is_connection_dropped() || error.is_unrecoverable_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_retryable() {
        assert!(is_retryable("GET", &[]));
        assert!(is_retryable("set", &[b"k".to_vec(), b"v".to_vec()]));
        assert!(!is_retryable(
            "set",
            &[b"k".to_vec(), b"v".to_vec(), b"NX".to_vec()]
        ));
        assert!(!is_retryable("incr", &[]));
    }

    #[test]
    fn connections_describe_themselves() {
        let config = ConnectionConfig {
            prefix: "app:".into(),
            ..ConnectionConfig::default()
        };
        let connection = Connection::new("cache", config).unwrap();
        assert_eq!(connection.name(), "cache");
        assert_eq!(connection.prefix(), "app:");
        assert_eq!(connection.config().port, 6379);
        assert!(connection.same_as(&connection.clone()));
        assert!(format!("{connection:?}").contains("cache"));
        assert_eq!(connection.unprefixed("app:name".into()), "name");
        assert_eq!(connection.unprefixed("other".into()), "other");
        connection.disconnect();
    }
}
