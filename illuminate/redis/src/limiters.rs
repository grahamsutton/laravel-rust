//! Rate limiting with Redis: `Redis::throttle` and `Redis::funnel`.
//!
//! A [`DurationLimiter`] allows a number of executions per window of time;
//! a [`ConcurrencyLimiter`] allows a number of executions at the same time.
//! Both are atomic Lua scripts, so they work across every process and server
//! sharing the Redis database — perfect for queued jobs talking to a rate
//! limited API:
//!
//! ```no_run
//! use illuminate_redis::Redis;
//!
//! # async fn example() -> illuminate_support::Result<()> {
//! Redis::throttle("key").block(0).allow(10).every(60).then(
//!     || async {
//!         // Lock obtained...
//!         Ok(())
//!     },
//!     || async {
//!         // Could not obtain lock...
//!         Ok(())
//!     },
//! ).await?;
//!
//! Redis::funnel("key").limit(1).then(
//!     || async {
//!         // Lock obtained...
//!         Ok(())
//!     },
//!     || async {
//!         // Could not obtain lock...
//!         Ok(())
//!     },
//! ).await?;
//! # Ok(())
//! # }
//! ```

use std::future::Future;
use std::time::{Duration, Instant};

use illuminate_support::{Carbon, Result, Str, Value, ValueExt};

use crate::connection::Connection;
use crate::facade::Redis;

/// Thrown when a limiter couldn't be acquired before its timeout.
#[derive(Debug, Clone, PartialEq, Eq, Default, thiserror::Error)]
#[error("The Redis limiter could not be acquired before the timeout.")]
pub struct LimiterTimeoutException;

/// The connection a builder runs on: a given one, or the default connection
/// (resolved when the limiter runs).
#[derive(Debug, Clone)]
struct Target(Option<Connection>);

impl Target {
    fn resolve(self) -> Result<Connection> {
        match self.0 {
            Some(connection) => Ok(connection),
            None => Redis::connection(None),
        }
    }
}

/// Wait between attempts until the deadline, failing with a
/// [`LimiterTimeoutException`] once it has passed.
async fn wait_or_timeout(started: Instant, timeout: u64, sleep: u64) -> Result<()> {
    if started.elapsed() >= Duration::from_secs(timeout) {
        return Err(LimiterTimeoutException.into());
    }
    tokio::time::sleep(Duration::from_millis(sleep)).await;
    Ok(())
}

/// Run a limiter's callbacks: the callback when the limiter was acquired,
/// the failure callback when it timed out.
async fn run_or_fail<T, G, GFut>(acquired: Result<T>, failure: G) -> Result<T>
where
    G: FnOnce() -> GFut,
    GFut: Future<Output = Result<T>>,
{
    match acquired {
        Err(error) if error.is::<LimiterTimeoutException>() => failure().await,
        other => other,
    }
}

// ----------------------------------------------------------------------
// Duration limiter
// ----------------------------------------------------------------------

/// Allows a maximum number of executions over a window of time (Laravel's
/// `DurationLimiter`).
#[derive(Debug, Clone)]
pub struct DurationLimiter {
    redis: Connection,
    name: String,
    max_locks: u64,
    decay: u64,
    decays_at: i64,
    remaining: i64,
}

impl DurationLimiter {
    /// Create a limiter allowing `max_locks` executions every `decay` seconds.
    pub fn new(redis: Connection, name: impl Into<String>, max_locks: u64, decay: u64) -> Self {
        Self {
            redis,
            name: name.into(),
            max_locks,
            decay,
            decays_at: 0,
            remaining: 0,
        }
    }

    /// Wait up to `timeout` seconds for a slot, checking every `sleep`
    /// milliseconds. Fails with a [`LimiterTimeoutException`].
    pub async fn block(&mut self, timeout: u64, sleep: u64) -> Result<()> {
        let started = Instant::now();
        while !self.acquire().await? {
            wait_or_timeout(started, timeout, sleep).await?;
        }
        Ok(())
    }

    /// Attempt to acquire a slot in the current window.
    pub async fn acquire(&mut self) -> Result<bool> {
        let results = self.run(ACQUIRE_DURATION_SCRIPT).await?;
        self.decays_at = int(&results, 1);
        self.remaining = int(&results, 2).max(0);
        Ok(results.first().is_some_and(ValueExt::truthy))
    }

    /// Determine if every slot of the current window has been taken.
    pub async fn too_many_attempts(&mut self) -> Result<bool> {
        let results = self.run(TOO_MANY_ATTEMPTS_SCRIPT).await?;
        self.decays_at = int(&results, 0);
        self.remaining = int(&results, 1);
        Ok(self.remaining <= 0)
    }

    /// Clear the limiter.
    pub async fn clear(&self) -> Result<()> {
        self.redis.del(self.name.as_str()).await?;
        Ok(())
    }

    /// The UNIX timestamp the current window ends at.
    pub fn decays_at(&self) -> i64 {
        self.decays_at
    }

    /// The number of slots left in the current window.
    pub fn remaining(&self) -> i64 {
        self.remaining
    }

    async fn run(&self, script: &str) -> Result<Vec<Value>> {
        let now = Carbon::now();
        let microtime = format!("{}.{:06}", now.timestamp(), now.micro());
        let value = self
            .redis
            .eval(
                script,
                (self.name.as_str(),),
                (microtime, now.timestamp(), self.decay, self.max_locks),
            )
            .await?;
        Ok(match value {
            Value::Array(items) => items,
            _ => Vec::new(),
        })
    }
}

fn int(values: &[Value], index: usize) -> i64 {
    values
        .get(index)
        .and_then(ValueExt::to_i64_lossy)
        .unwrap_or(0)
}

/// Builds a [`DurationLimiter`]: `Redis::throttle("key").allow(10).every(60)`.
#[derive(Debug, Clone)]
pub struct DurationLimiterBuilder {
    connection: Target,
    name: String,
    max_locks: u64,
    decay: u64,
    timeout: u64,
    sleep: u64,
}

impl DurationLimiterBuilder {
    pub(crate) fn new(connection: Option<Connection>, name: impl Into<String>) -> Self {
        Self {
            connection: Target(connection),
            name: name.into(),
            max_locks: 1,
            decay: 60,
            timeout: 3,
            sleep: 750,
        }
    }

    /// Set the maximum number of executions per window.
    pub fn allow(mut self, max_locks: u64) -> Self {
        self.max_locks = max_locks;
        self
    }

    /// Set the length of the window, in seconds.
    pub fn every(mut self, decay: u64) -> Self {
        self.decay = decay;
        self
    }

    /// Set how many seconds to wait for a slot (3 by default; `0` doesn't wait).
    pub fn block(mut self, timeout: u64) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set how many milliseconds to sleep between attempts (750 by default).
    pub fn sleep(mut self, milliseconds: u64) -> Self {
        self.sleep = milliseconds;
        self
    }

    /// Run the callback if a slot is obtained, otherwise run the failure
    /// callback.
    pub async fn then<T, F, Fut, G, GFut>(self, callback: F, failure: G) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
        G: FnOnce() -> GFut,
        GFut: Future<Output = Result<T>>,
    {
        let acquired = self.acquire().await;
        match acquired {
            Ok(()) => callback().await,
            Err(error) => run_or_fail(Err(error), failure).await,
        }
    }

    /// Run the callback if a slot is obtained, otherwise fail with a
    /// [`LimiterTimeoutException`].
    pub async fn then_or_fail<T, F, Fut>(self, callback: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.acquire().await?;
        callback().await
    }

    async fn acquire(self) -> Result<()> {
        let connection = self.connection.resolve()?;
        DurationLimiter::new(connection, self.name, self.max_locks, self.decay)
            .block(self.timeout, self.sleep)
            .await
    }
}

/// KEYS[1] - The limiter name
/// ARGV[1] - Current time in microseconds
/// ARGV[2] - Current time in seconds
/// ARGV[3] - Duration of the bucket
/// ARGV[4] - Allowed number of tasks
const ACQUIRE_DURATION_SCRIPT: &str = r#"
local function reset()
    redis.call('HMSET', KEYS[1], 'start', ARGV[2], 'end', ARGV[2] + ARGV[3], 'count', 1)
    return redis.call('EXPIRE', KEYS[1], ARGV[3] * 2)
end

if redis.call('EXISTS', KEYS[1]) == 0 then
    return {reset(), ARGV[2] + ARGV[3], ARGV[4] - 1}
end

if ARGV[1] >= redis.call('HGET', KEYS[1], 'start') and ARGV[1] <= redis.call('HGET', KEYS[1], 'end') then
    return {
        tonumber(redis.call('HINCRBY', KEYS[1], 'count', 1)) <= tonumber(ARGV[4]),
        redis.call('HGET', KEYS[1], 'end'),
        ARGV[4] - redis.call('HGET', KEYS[1], 'count')
    }
end

return {reset(), ARGV[2] + ARGV[3], ARGV[4] - 1}
"#;

/// KEYS[1] - The limiter name
/// ARGV[1] - Current time in microseconds
/// ARGV[2] - Current time in seconds
/// ARGV[3] - Duration of the bucket
/// ARGV[4] - Allowed number of tasks
const TOO_MANY_ATTEMPTS_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then
    return {ARGV[2] + ARGV[3], tonumber(ARGV[4])}
end

if ARGV[1] >= redis.call('HGET', KEYS[1], 'start') and ARGV[1] <= redis.call('HGET', KEYS[1], 'end') then
    return {
        redis.call('HGET', KEYS[1], 'end'),
        ARGV[4] - redis.call('HGET', KEYS[1], 'count')
    }
end

return {ARGV[2] + ARGV[3], tonumber(ARGV[4])}
"#;

// ----------------------------------------------------------------------
// Concurrency limiter
// ----------------------------------------------------------------------

/// Allows a maximum number of simultaneous executions (Laravel's
/// `ConcurrencyLimiter`). Each execution holds one of the slots
/// `{name}1` ... `{name}{max}` until it finishes — or until the slot
/// expires, should the process die.
#[derive(Debug, Clone)]
pub struct ConcurrencyLimiter {
    redis: Connection,
    name: String,
    max_locks: u64,
    release_after: u64,
}

impl ConcurrencyLimiter {
    /// Create a limiter allowing `max_locks` simultaneous executions, each
    /// holding its slot for at most `release_after` seconds.
    pub fn new(
        redis: Connection,
        name: impl Into<String>,
        max_locks: u64,
        release_after: u64,
    ) -> Self {
        Self {
            redis,
            name: name.into(),
            max_locks,
            release_after,
        }
    }

    /// Wait up to `timeout` seconds for a slot (checking every `sleep`
    /// milliseconds), run the callback while holding it, then release it.
    pub async fn block<T, F, Fut>(&self, timeout: u64, sleep: u64, callback: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let id = Str::random(20);
        let started = Instant::now();
        let slot = loop {
            if let Some(slot) = self.acquire(&id).await? {
                break slot;
            }
            wait_or_timeout(started, timeout, sleep).await?;
        };

        let result = callback().await;
        self.release(&slot, &id).await?;
        result
    }

    /// Attempt to take a free slot for the given lock id, returning the
    /// slot's name.
    pub async fn acquire(&self, id: &str) -> Result<Option<String>> {
        let slots: Vec<String> = (1..=self.max_locks)
            .map(|index| format!("{}{index}", self.name))
            .collect();
        let slot = self
            .redis
            .eval(
                ACQUIRE_CONCURRENCY_SCRIPT,
                slots,
                (self.name.as_str(), self.release_after, id),
            )
            .await?;
        Ok(match slot {
            Value::String(slot) => Some(slot),
            _ => None,
        })
    }

    /// Release the slot, if it is still held by the given lock id.
    pub async fn release(&self, slot: &str, id: &str) -> Result<()> {
        self.redis
            .eval(RELEASE_CONCURRENCY_SCRIPT, (slot,), (id,))
            .await?;
        Ok(())
    }
}

/// Builds a [`ConcurrencyLimiter`]: `Redis::funnel("key").limit(2)`.
#[derive(Debug, Clone)]
pub struct ConcurrencyLimiterBuilder {
    connection: Target,
    name: String,
    max_locks: u64,
    release_after: u64,
    timeout: u64,
    sleep: u64,
}

impl ConcurrencyLimiterBuilder {
    pub(crate) fn new(connection: Option<Connection>, name: impl Into<String>) -> Self {
        Self {
            connection: Target(connection),
            name: name.into(),
            max_locks: 1,
            release_after: 60,
            timeout: 3,
            sleep: 250,
        }
    }

    /// Set the maximum number of simultaneous executions (1 by default).
    pub fn limit(mut self, max_locks: u64) -> Self {
        self.max_locks = max_locks;
        self
    }

    /// Set how many seconds a slot is held before it is released
    /// automatically (60 by default).
    pub fn release_after(mut self, seconds: u64) -> Self {
        self.release_after = seconds;
        self
    }

    /// Set how many seconds to wait for a slot (3 by default; `0` doesn't wait).
    pub fn block(mut self, timeout: u64) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set how many milliseconds to sleep between attempts (250 by default).
    pub fn sleep(mut self, milliseconds: u64) -> Self {
        self.sleep = milliseconds;
        self
    }

    /// Run the callback if a slot is obtained (releasing it afterwards),
    /// otherwise run the failure callback.
    pub async fn then<T, F, Fut, G, GFut>(self, callback: F, failure: G) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
        G: FnOnce() -> GFut,
        GFut: Future<Output = Result<T>>,
    {
        let mut ran = false;
        let result = self
            .then_or_fail(|| {
                ran = true;
                callback()
            })
            .await;
        if ran {
            return result;
        }
        run_or_fail(result, failure).await
    }

    /// Run the callback if a slot is obtained (releasing it afterwards),
    /// otherwise fail with a [`LimiterTimeoutException`].
    pub async fn then_or_fail<T, F, Fut>(self, callback: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let connection = self.connection.resolve()?;
        ConcurrencyLimiter::new(connection, self.name, self.max_locks, self.release_after)
            .block(self.timeout, self.sleep, callback)
            .await
    }
}

/// KEYS    - The keys that represent available slots
/// ARGV[1] - The limiter name
/// ARGV[2] - The number of seconds the slot should be reserved
/// ARGV[3] - The unique identifier for this lock
const ACQUIRE_CONCURRENCY_SCRIPT: &str = r#"
for index, value in pairs(redis.call('mget', unpack(KEYS))) do
    if not value then
        redis.call('set', KEYS[index], ARGV[3], "EX", ARGV[2])
        return ARGV[1]..index
    end
end
"#;

/// KEYS[1] - The name of the lock
/// ARGV[1] - The unique identifier for this lock
const RELEASE_CONCURRENCY_SCRIPT: &str = r#"
if redis.call('get', KEYS[1]) == ARGV[1]
then
    return redis.call('del', KEYS[1])
else
    return 0
end
"#;
