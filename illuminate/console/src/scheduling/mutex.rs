//! Mutexes preventing scheduled events from overlapping, or from running
//! on more than one server.
//!
//! The in-memory mutexes work within a single process (perfect for
//! `schedule:work`). Bind a cache-backed implementation of [`EventMutex`]
//! and [`SchedulingMutex`] in the container to share locks between
//! processes and servers.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use illuminate_support::Carbon;

use super::event::Event;

/// Prevents an event from overlapping with itself (`without_overlapping`).
#[async_trait]
pub trait EventMutex: Send + Sync + 'static {
    /// Attempt to obtain an event mutex for the given event.
    async fn create(&self, event: &Event) -> bool;

    /// Determine if an event mutex exists for the given event.
    async fn exists(&self, event: &Event) -> bool;

    /// Clear the event mutex for the given event.
    async fn forget(&self, event: &Event);
}

/// Ensures an event runs on a single server (`on_one_server`).
#[async_trait]
pub trait SchedulingMutex: Send + Sync + 'static {
    /// Attempt to obtain a scheduling mutex for the given event and time.
    async fn create(&self, event: &Event, time: &Carbon) -> bool;

    /// Determine if a scheduling mutex exists for the given event and time.
    async fn exists(&self, event: &Event, time: &Carbon) -> bool;
}

#[derive(Debug, Default)]
struct Locks {
    locks: Mutex<HashMap<String, Instant>>,
}

impl Locks {
    fn acquire(&self, key: String, ttl: Duration) -> bool {
        let mut locks = self.locks.lock().unwrap();
        let now = Instant::now();

        if locks.get(&key).is_some_and(|expires| *expires > now) {
            return false;
        }

        locks.insert(key, now + ttl);
        true
    }

    fn exists(&self, key: &str) -> bool {
        self.locks
            .lock()
            .unwrap()
            .get(key)
            .is_some_and(|expires| *expires > Instant::now())
    }

    fn release(&self, key: &str) {
        self.locks.lock().unwrap().remove(key);
    }
}

/// An in-process [`EventMutex`].
#[derive(Debug, Default)]
pub struct InMemoryEventMutex {
    locks: Locks,
}

impl InMemoryEventMutex {
    /// Create a new in-memory event mutex.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EventMutex for InMemoryEventMutex {
    async fn create(&self, event: &Event) -> bool {
        let ttl = Duration::from_secs(event.expires_at() * 60);
        self.locks.acquire(event.mutex_name(), ttl)
    }

    async fn exists(&self, event: &Event) -> bool {
        self.locks.exists(&event.mutex_name())
    }

    async fn forget(&self, event: &Event) {
        self.locks.release(&event.mutex_name());
    }
}

/// An in-process [`SchedulingMutex`].
#[derive(Debug, Default)]
pub struct InMemorySchedulingMutex {
    locks: Locks,
}

impl InMemorySchedulingMutex {
    /// Create a new in-memory scheduling mutex.
    pub fn new() -> Self {
        Self::default()
    }

    fn key(event: &Event, time: &Carbon) -> String {
        format!("{}{}", event.mutex_name(), time.format("Hi"))
    }
}

#[async_trait]
impl SchedulingMutex for InMemorySchedulingMutex {
    async fn create(&self, event: &Event, time: &Carbon) -> bool {
        self.locks
            .acquire(Self::key(event, time), Duration::from_secs(3600))
    }

    async fn exists(&self, event: &Event, time: &Carbon) -> bool {
        self.locks.exists(&Self::key(event, time))
    }
}
