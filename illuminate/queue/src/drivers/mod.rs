//! The queue drivers that ship with the framework.
//!
//! Other drivers implement [`Queue`](crate::contracts::Queue) and register
//! themselves with [`QueueManager::extend`](crate::QueueManager::extend).

mod array;
pub mod beanstalkd;
pub mod database;
mod deferred;
mod failover;
mod null;
pub mod redis;
pub mod sqs;
mod sync;

pub use array::ArrayQueue;
pub use beanstalkd::{Beanstalkd, BeanstalkdQueue};
pub use database::DatabaseQueue;
pub use deferred::{BackgroundQueue, DeferredQueue};
pub use failover::FailoverQueue;
pub use null::NullQueue;
pub use redis::RedisQueue;
pub use sqs::{OverflowStorage, SqsClient, SqsQueue};
pub use sync::SyncQueue;
