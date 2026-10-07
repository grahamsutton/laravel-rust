//! # Illuminate Redis
//!
//! [Redis](https://redis.io) is an open source, advanced key-value store —
//! a data structure server whose keys can hold strings, hashes, lists, sets
//! and sorted sets. The [`Redis`] facade gives you all of it:
//!
//! ```no_run
//! use illuminate_redis::Redis;
//!
//! # async fn example() -> illuminate_support::Result<()> {
//! Redis::set("name", "Taylor").await?;
//!
//! let user = Redis::get("user:profile:1").await?;
//! let values = Redis::lrange("names", 5, 10).await?;
//!
//! // Any command at all...
//! let values = Redis::command("lrange", ("names", 5, 10)).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Configuration
//!
//! Connections live in the `redis` section of `config/database`:
//!
//! ```json
//! "redis": {
//!     "client": "phpredis",
//!     "options": {
//!         "cluster": "redis",
//!         "prefix": "laravel-database-"
//!     },
//!     "default": {
//!         "url": null,
//!         "host": "127.0.0.1",
//!         "username": null,
//!         "password": null,
//!         "port": "6379",
//!         "database": "0"
//!     },
//!     "cache": {
//!         "url": null,
//!         "host": "127.0.0.1",
//!         "port": "6379",
//!         "database": "1"
//!     }
//! }
//! ```
//!
//! A connection may be described by a single `url` instead
//! (`redis://user:password@127.0.0.1:6380/1`), and every key is prefixed
//! with `options.prefix` — the slug of your application's name followed by
//! `-database-`, unless you say otherwise. Grab any connection with
//! [`Redis::connection`].
//!
//! ## Transactions, scripts and pipelines
//!
//! [`Redis::transaction`] wraps commands in `MULTI` / `EXEC`, [`Redis::eval`]
//! runs Lua scripts atomically, and [`Redis::pipeline`] sends many commands
//! in a single round trip:
//!
//! ```no_run
//! use illuminate_redis::Redis;
//!
//! # async fn example() -> illuminate_support::Result<()> {
//! Redis::transaction(|redis| {
//!     redis.incr("user_visits");
//!     redis.incr("total_visits");
//! }).await?;
//!
//! let counter = Redis::eval(r#"
//!     local counter = redis.call("incr", KEYS[1])
//!
//!     if counter > 5 then
//!         redis.call("incr", KEYS[2])
//!     end
//!
//!     return counter
//! "#, ("first-counter", "second-counter"), ()).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Pub / Sub
//!
//! ```no_run
//! use illuminate_redis::Redis;
//!
//! # async fn example() -> illuminate_support::Result<()> {
//! let subscription = Redis::psubscribe(["users.*"], |message, channel| {
//!     println!("{channel}: {message}");
//! }).await?;
//!
//! Redis::publish("users.1", r#"{"name":"Adam Wathan"}"#).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Rate limiting
//!
//! [`Redis::throttle`] and [`Redis::funnel`] limit how often — and how many
//! at once — a callback runs, across every server sharing the database. See
//! the [`limiters`] module.

pub mod args;
pub mod config;
pub mod connection;
pub mod facade;
mod keys;
pub mod limiters;
pub mod manager;
pub mod pipeline;
pub mod provider;
pub mod pubsub;
pub mod testing;
pub mod value;

pub use args::CommandArgs;
pub use config::ConnectionConfig;
pub use connection::Connection;
pub use facade::Redis;
pub use limiters::{
    ConcurrencyLimiter, ConcurrencyLimiterBuilder, DurationLimiter, DurationLimiterBuilder,
    LimiterTimeoutException,
};
pub use manager::RedisManager;
pub use pipeline::Pipeline;
pub use provider::RedisServiceProvider;
pub use pubsub::Subscription;

/// The redis-rs client this component is built on, for the rare occasion
/// you need to reach underneath it.
pub use redis as client;
pub use redis::{FromRedisValue, ToRedisArgs};

/// Everything you need to work with Redis, in one import.
pub mod prelude {
    pub use crate::{CommandArgs, Connection, Redis};
}
