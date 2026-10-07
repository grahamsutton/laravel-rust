//! The queue drivers that ship with the framework.
//!
//! Other drivers implement [`Queue`](crate::contracts::Queue) and register
//! themselves with [`QueueManager::extend`](crate::QueueManager::extend).

mod array;
pub mod database;
mod deferred;
mod failover;
mod null;
mod sync;

pub use array::ArrayQueue;
pub use database::DatabaseQueue;
pub use deferred::{BackgroundQueue, DeferredQueue};
pub use failover::FailoverQueue;
pub use null::NullQueue;
pub use sync::SyncQueue;
