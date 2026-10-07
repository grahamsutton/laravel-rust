//! The `Redis` facade.

use std::sync::Arc;

use indexmap::IndexMap;
use redis::{FromRedisValue, ToRedisArgs};

use illuminate_container::{Container, try_app};
use illuminate_support::{Result, Value};

use crate::args::CommandArgs;
use crate::connection::Connection;
use crate::limiters::{ConcurrencyLimiterBuilder, DurationLimiterBuilder};
use crate::manager::RedisManager;
use crate::pipeline::Pipeline;
use crate::provider::make_manager;
use crate::pubsub::Subscription;

/// Resolve the Redis manager, registering it on first use.
fn manager() -> Result<Arc<RedisManager>> {
    if let Some(manager) = try_app::<RedisManager>() {
        return Ok(manager);
    }
    let container = Container::get_instance();
    container.singleton_if::<RedisManager>(make_manager);
    Ok(container.try_make::<RedisManager>()?)
}

/// The default connection.
fn connection() -> Result<Connection> {
    manager()?.connection(None)
}

/// The `Redis` facade.
///
/// Commands go to the `default` connection; use [`Redis::connection`] to
/// pick another one. Every Redis command is available through
/// [`Redis::command`], and the common ones have typed methods of their own.
///
/// ```no_run
/// use illuminate_redis::Redis;
///
/// # async fn example() -> illuminate_support::Result<()> {
/// Redis::set("name", "Taylor").await?;
///
/// let name = Redis::get("name").await?;
/// let values = Redis::lrange("names", 5, 10).await?;
/// let values = Redis::command("lrange", ("names", 5, 10)).await?;
///
/// let redis = Redis::connection("cache")?;
/// # Ok(())
/// # }
/// ```
pub struct Redis;

impl Redis {
    /// Get the Redis manager.
    pub fn manager() -> Result<Arc<RedisManager>> {
        manager()
    }

    /// Get a Redis connection by name (`None` for the default connection).
    pub fn connection<'a>(name: impl Into<Option<&'a str>>) -> Result<Connection> {
        manager()?.connection(name)
    }

    /// Run any Redis command on the default connection.
    pub async fn command(method: &str, args: impl CommandArgs) -> Result<Value> {
        connection()?.command(method, args).await
    }

    /// Run any Redis command, converting its reply. See [`Connection::query`].
    pub async fn query<T: FromRedisValue>(method: &str, args: impl CommandArgs) -> Result<T> {
        connection()?.query(method, args).await
    }

    /// Run a raw, unprefixed command. See [`Connection::execute_raw`].
    pub async fn execute_raw(args: impl CommandArgs) -> Result<Value> {
        connection()?.execute_raw(args).await
    }

    /// Get the value of a key.
    pub async fn get(key: impl AsRef<str>) -> Result<Option<String>> {
        connection()?.get(key).await
    }

    /// Set the value of a key.
    pub async fn set(key: impl AsRef<str>, value: impl ToRedisArgs) -> Result<bool> {
        connection()?.set(key, value).await
    }

    /// Set the value of a key, expiring after the given number of seconds.
    pub async fn set_ex(
        key: impl AsRef<str>,
        value: impl ToRedisArgs,
        seconds: u64,
    ) -> Result<bool> {
        connection()?.set_ex(key, value, seconds).await
    }

    /// Set the value of a key, only if it doesn't exist yet.
    pub async fn setnx(key: impl AsRef<str>, value: impl ToRedisArgs) -> Result<bool> {
        connection()?.setnx(key, value).await
    }

    /// Delete one or more keys.
    pub async fn del(keys: impl CommandArgs) -> Result<i64> {
        connection()?.del(keys).await
    }

    /// Count how many of the given keys exist.
    pub async fn exists(keys: impl CommandArgs) -> Result<i64> {
        connection()?.exists(keys).await
    }

    /// Increment a key by one.
    pub async fn incr(key: impl AsRef<str>) -> Result<i64> {
        connection()?.incr(key).await
    }

    /// Increment a key by the given amount.
    pub async fn incrby(key: impl AsRef<str>, amount: i64) -> Result<i64> {
        connection()?.incrby(key, amount).await
    }

    /// Decrement a key by one.
    pub async fn decr(key: impl AsRef<str>) -> Result<i64> {
        connection()?.decr(key).await
    }

    /// Decrement a key by the given amount.
    pub async fn decrby(key: impl AsRef<str>, amount: i64) -> Result<i64> {
        connection()?.decrby(key, amount).await
    }

    /// Expire a key after the given number of seconds.
    pub async fn expire(key: impl AsRef<str>, seconds: i64) -> Result<bool> {
        connection()?.expire(key, seconds).await
    }

    /// The remaining time to live of a key, in seconds.
    pub async fn ttl(key: impl AsRef<str>) -> Result<i64> {
        connection()?.ttl(key).await
    }

    /// Find the keys matching a pattern.
    pub async fn keys(pattern: impl AsRef<str>) -> Result<Vec<String>> {
        connection()?.keys(pattern).await
    }

    /// Get the values of several keys.
    pub async fn mget(keys: impl CommandArgs) -> Result<Vec<Option<String>>> {
        connection()?.mget(keys).await
    }

    /// Set several keys at once.
    pub async fn mset<K, V>(values: impl IntoIterator<Item = (K, V)>) -> Result<bool>
    where
        K: AsRef<str>,
        V: ToRedisArgs,
    {
        connection()?.mset(values).await
    }

    /// Get a field of a hash.
    pub async fn hget(key: impl AsRef<str>, field: impl AsRef<str>) -> Result<Option<String>> {
        connection()?.hget(key, field).await
    }

    /// Set a field of a hash.
    pub async fn hset(
        key: impl AsRef<str>,
        field: impl AsRef<str>,
        value: impl ToRedisArgs,
    ) -> Result<i64> {
        connection()?.hset(key, field, value).await
    }

    /// Get every field of a hash.
    pub async fn hgetall(key: impl AsRef<str>) -> Result<IndexMap<String, String>> {
        connection()?.hgetall(key).await
    }

    /// Delete one or more fields of a hash.
    pub async fn hdel(key: impl AsRef<str>, fields: impl CommandArgs) -> Result<i64> {
        connection()?.hdel(key, fields).await
    }

    /// Prepend one or more values to a list.
    pub async fn lpush(key: impl AsRef<str>, values: impl CommandArgs) -> Result<i64> {
        connection()?.lpush(key, values).await
    }

    /// Append one or more values to a list.
    pub async fn rpush(key: impl AsRef<str>, values: impl CommandArgs) -> Result<i64> {
        connection()?.rpush(key, values).await
    }

    /// Remove and return the first value of a list.
    pub async fn lpop(key: impl AsRef<str>) -> Result<Option<String>> {
        connection()?.lpop(key).await
    }

    /// Remove and return the last value of a list.
    pub async fn rpop(key: impl AsRef<str>) -> Result<Option<String>> {
        connection()?.rpop(key).await
    }

    /// The values of a list between two indexes.
    pub async fn lrange(key: impl AsRef<str>, start: i64, stop: i64) -> Result<Vec<String>> {
        connection()?.lrange(key, start, stop).await
    }

    /// The length of a list.
    pub async fn llen(key: impl AsRef<str>) -> Result<i64> {
        connection()?.llen(key).await
    }

    /// Add one or more members to a set.
    pub async fn sadd(key: impl AsRef<str>, members: impl CommandArgs) -> Result<i64> {
        connection()?.sadd(key, members).await
    }

    /// Remove one or more members from a set.
    pub async fn srem(key: impl AsRef<str>, members: impl CommandArgs) -> Result<i64> {
        connection()?.srem(key, members).await
    }

    /// The members of a set.
    pub async fn smembers(key: impl AsRef<str>) -> Result<Vec<String>> {
        connection()?.smembers(key).await
    }

    /// Determine if the value is a member of a set.
    pub async fn sismember(key: impl AsRef<str>, member: impl ToRedisArgs) -> Result<bool> {
        connection()?.sismember(key, member).await
    }

    /// Add a member to a sorted set (or update its score).
    pub async fn zadd(key: impl AsRef<str>, score: f64, member: impl ToRedisArgs) -> Result<i64> {
        connection()?.zadd(key, score, member).await
    }

    /// The members of a sorted set between two ranks.
    pub async fn zrange(key: impl AsRef<str>, start: i64, stop: i64) -> Result<Vec<String>> {
        connection()?.zrange(key, start, stop).await
    }

    /// The members of a sorted set with a score between `min` and `max`.
    pub async fn zrangebyscore(
        key: impl AsRef<str>,
        min: impl ToRedisArgs,
        max: impl ToRedisArgs,
    ) -> Result<Vec<String>> {
        connection()?.zrangebyscore(key, min, max).await
    }

    /// Remove one or more members from a sorted set.
    pub async fn zrem(key: impl AsRef<str>, members: impl CommandArgs) -> Result<i64> {
        connection()?.zrem(key, members).await
    }

    /// The number of members in a sorted set.
    pub async fn zcard(key: impl AsRef<str>) -> Result<i64> {
        connection()?.zcard(key).await
    }

    /// Publish a message to a channel.
    pub async fn publish(channel: impl AsRef<str>, message: impl ToRedisArgs) -> Result<i64> {
        connection()?.publish(channel, message).await
    }

    /// Delete every key of the default connection's database.
    pub async fn flushdb() -> Result<bool> {
        connection()?.flushdb().await
    }

    /// Evaluate a Lua script. See [`Connection::eval`].
    pub async fn eval(
        script: &str,
        keys: impl CommandArgs,
        args: impl CommandArgs,
    ) -> Result<Value> {
        connection()?.eval(script, keys, args).await
    }

    /// Send several commands in a single round trip. See [`Connection::pipeline`].
    pub async fn pipeline(callback: impl FnOnce(&mut Pipeline)) -> Result<Vec<Value>> {
        connection()?.pipeline(callback).await
    }

    /// Run several commands in a `MULTI` / `EXEC` transaction. See
    /// [`Connection::transaction`].
    pub async fn transaction(callback: impl FnOnce(&mut Pipeline)) -> Result<Vec<Value>> {
        connection()?.transaction(callback).await
    }

    /// Listen for messages on the given channels. See [`Connection::subscribe`].
    pub async fn subscribe<F>(channels: impl CommandArgs, callback: F) -> Result<Subscription>
    where
        F: FnMut(String, String) + Send + 'static,
    {
        connection()?.subscribe(channels, callback).await
    }

    /// Listen for messages on every channel matching the given patterns. See
    /// [`Connection::psubscribe`].
    pub async fn psubscribe<F>(patterns: impl CommandArgs, callback: F) -> Result<Subscription>
    where
        F: FnMut(String, String) + Send + 'static,
    {
        connection()?.psubscribe(patterns, callback).await
    }

    /// Throttle a callback to a number of executions per window of time, on
    /// the default connection.
    ///
    /// ```no_run
    /// use illuminate_redis::Redis;
    ///
    /// # async fn example() -> illuminate_support::Result<()> {
    /// Redis::throttle("key").allow(10).every(60).then(
    ///     || async { /* Lock obtained... */ Ok(()) },
    ///     || async { /* Could not obtain lock... */ Ok(()) },
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn throttle(name: impl Into<String>) -> DurationLimiterBuilder {
        DurationLimiterBuilder::new(None, name)
    }

    /// Funnel a callback to a number of simultaneous executions, on the
    /// default connection.
    ///
    /// ```no_run
    /// use illuminate_redis::Redis;
    ///
    /// # async fn example() -> illuminate_support::Result<()> {
    /// Redis::funnel("key").limit(1).then(
    ///     || async { /* Lock obtained... */ Ok(()) },
    ///     || async { /* Could not obtain lock... */ Ok(()) },
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn funnel(name: impl Into<String>) -> ConcurrencyLimiterBuilder {
        ConcurrencyLimiterBuilder::new(None, name)
    }
}
