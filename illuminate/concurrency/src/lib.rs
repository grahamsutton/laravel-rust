//! # Illuminate Concurrency
//!
//! Sometimes you need to run several slow tasks which don't depend on one
//! another. Running them concurrently can make a big difference, and the
//! [`Concurrency`] facade makes it a one-liner:
//!
//! ```
//! use illuminate_concurrency::Concurrency;
//!
//! # async fn count(table: &str) -> illuminate_support::Result<u64> { Ok(table.len() as u64) }
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let (user_count, order_count) = Concurrency::run((
//!     count("users"),
//!     count("orders"),
//! ))
//! .await?;
//! # assert_eq!((user_count, order_count), (5, 6));
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```
//!
//! ## How it works
//!
//! Laravel's closures can't run at the same time inside one PHP process, so
//! its default `process` driver serializes each closure and runs it in a
//! child PHP process. Rust doesn't need any of that: tasks are futures, and
//! the default `tokio` driver spawns each one onto the Tokio runtime (in
//! parallel, on a multi-threaded runtime). The `sync` driver runs them one
//! after another, which is handy in tests. The `process` and `fork` drivers
//! aren't provided, since a Rust closure can't be serialized and shipped to
//! another process.
//!
//! The default driver is the `concurrency.default` configuration value
//! (`"tokio"` when it isn't set); [`Concurrency::driver`] picks another.
//!
//! ## Deferring tasks
//!
//! [`Concurrency::defer`] runs tasks after the response has been sent,
//! through [`DeferredCallbacks`]: the HTTP kernel scopes a collection to
//! each request and invokes it once the response is on its way.

pub mod deferred;
pub mod driver;
pub mod facade;
pub mod manager;
pub mod provider;
pub mod tasks;

pub use deferred::DeferredCallbacks;
pub use driver::{Driver, SyncDriver, TaskTimedOutException, TokioDriver};
pub use facade::Concurrency;
pub use indexmap::IndexMap;
pub use manager::{ConcurrencyManager, DEFAULT_DRIVER};
pub use provider::ConcurrencyServiceProvider;
pub use tasks::{Assembler, Job, JobOutput, Task, Tasks, task};
