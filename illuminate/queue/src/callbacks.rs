//! Callbacks that travel with queued work: chain `catch` callbacks, batch
//! callbacks, and queued closures.
//!
//! Laravel serializes closures. Rust can't, so a closure callback is kept
//! in the memory of the process that registered it and the payload only
//! carries its id. That works for the `sync`, `deferred`, `background` and
//! `array` connections, and for workers running in the same process. When a
//! callback must survive a trip to another process, register a *job*
//! callback instead (`then_dispatch`, `catch_dispatch`, ...): the job is
//! dispatched when the callback fires.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use serde::{Deserialize, Serialize};

use illuminate_http::BoxFuture;
use illuminate_support::{Error, Result, Str};

use crate::bus::batch::Batch;
use crate::envelope::SerializedJob;

/// A reference to a callback, as stored in payloads and batch options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CallbackRef {
    /// A closure registered in the dispatching process.
    Closure {
        /// The closure's id.
        id: String,
    },
    /// A job to dispatch when the callback fires.
    Job {
        /// The serialized job.
        job: SerializedJob,
        /// Only dispatch the job if the batch wasn't cancelled.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unless_cancelled: bool,
    },
}

impl CallbackRef {
    /// The closure id, when this is a closure callback.
    pub fn closure_id(&self) -> Option<&str> {
        match self {
            CallbackRef::Closure { id } => Some(id),
            CallbackRef::Job { .. } => None,
        }
    }
}

/// A batch callback: `then`, `catch`, `finally`, `progress`, ...
pub(crate) type BatchCallback =
    Arc<dyn Fn(Batch, Option<Arc<Error>>) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// A chain `catch` callback.
pub(crate) type ChainCatchCallback =
    Arc<dyn Fn(Arc<Error>) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// A queued closure.
pub(crate) type QueuedClosure = Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;

static CLOSURES: LazyLock<RwLock<HashMap<String, Box<dyn Any + Send + Sync>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Keep a closure in this process, returning its id.
pub(crate) fn store<T: Any + Send + Sync>(callback: T) -> String {
    let id = Str::uuid().to_string();
    CLOSURES
        .write()
        .unwrap()
        .insert(id.clone(), Box::new(callback));
    id
}

/// Get a stored closure of the given type.
pub(crate) fn get<T: Any + Clone>(id: &str) -> Option<T> {
    CLOSURES
        .read()
        .unwrap()
        .get(id)
        .and_then(|callback| callback.downcast_ref::<T>())
        .cloned()
}

/// Determine if a closure with the given id is stored in this process.
pub(crate) fn exists(id: &str) -> bool {
    CLOSURES.read().unwrap().contains_key(id)
}

/// Forget a stored closure.
pub(crate) fn forget(id: &str) {
    CLOSURES.write().unwrap().remove(id);
}

/// Forget every closure referenced by the given callbacks.
pub(crate) fn forget_all<'a>(callbacks: impl IntoIterator<Item = &'a CallbackRef>) {
    let ids: Vec<&str> = callbacks
        .into_iter()
        .filter_map(CallbackRef::closure_id)
        .collect();
    if ids.is_empty() {
        return;
    }
    let mut closures = CLOSURES.write().unwrap();
    for id in ids {
        closures.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closures_are_stored_by_id() {
        let id = store::<Arc<dyn Fn() -> u8 + Send + Sync>>(Arc::new(|| 7));
        assert!(exists(&id));
        let callback = get::<Arc<dyn Fn() -> u8 + Send + Sync>>(&id).unwrap();
        assert_eq!(callback(), 7);
        assert!(get::<Arc<dyn Fn() -> u16 + Send + Sync>>(&id).is_none());
        forget_all([&CallbackRef::Closure { id: id.clone() }]);
        assert!(!exists(&id));
        forget(&id);
    }

    #[test]
    fn references_serialize_with_a_type_tag() {
        let reference = CallbackRef::Closure { id: "abc".into() };
        assert_eq!(
            serde_json::to_value(&reference).unwrap(),
            serde_json::json!({"type": "closure", "id": "abc"})
        );
    }
}
