//! The queue drivers that ship with the framework.
//!
//! The `database` driver lives with the database component: it implements
//! [`Queue`](crate::contracts::Queue) over the `jobs` table and registers
//! itself with [`QueueManager::extend`](crate::QueueManager::extend).

mod array;
mod deferred;
mod failover;
mod null;
mod sync;

pub use array::ArrayQueue;
pub use deferred::{BackgroundQueue, DeferredQueue};
pub use failover::FailoverQueue;
pub use null::NullQueue;
pub use sync::SyncQueue;
