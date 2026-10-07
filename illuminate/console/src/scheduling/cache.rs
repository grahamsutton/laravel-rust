//! Where the scheduler keeps its signals: whether the schedule is paused
//! (`schedule:pause`) and when it was last interrupted
//! (`schedule:interrupt`).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use illuminate_support::Value;

/// The store holding the scheduler's signals. In an application it's the
/// cache, so every `schedule:run` process sees them; the in-memory store
/// works within a single process.
#[async_trait]
pub trait ScheduleCache: Send + Sync + 'static {
    /// Get the value stored under the key.
    async fn get(&self, key: &str) -> Option<Value>;

    /// Store the value under the key until it's forgotten.
    async fn forever(&self, key: &str, value: Value);

    /// Remove the value stored under the key.
    async fn forget(&self, key: &str);
}

/// A [`ScheduleCache`] that lives in this process.
#[derive(Debug, Default)]
pub struct InMemoryScheduleCache {
    values: Mutex<HashMap<String, Value>>,
}

impl InMemoryScheduleCache {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ScheduleCache for InMemoryScheduleCache {
    async fn get(&self, key: &str) -> Option<Value> {
        self.values.lock().unwrap().get(key).cloned()
    }

    async fn forever(&self, key: &str, value: Value) {
        self.values.lock().unwrap().insert(key.to_string(), value);
    }

    async fn forget(&self, key: &str) {
        self.values.lock().unwrap().remove(key);
    }
}
