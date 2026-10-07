//! Shared helpers for the queue's integration tests.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use illuminate_cache::CacheServiceProvider;
use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider, app};
use illuminate_queue::events::QueueEvent;
use illuminate_queue::{BusServiceProvider, Queue, QueueServiceProvider};
use illuminate_support::{Value, json};

/// A log of what happened, shared by jobs and assertions.
#[derive(Default)]
pub struct Recorder {
    entries: Mutex<Vec<String>>,
}

impl Recorder {
    pub fn push(&self, entry: impl Into<String>) {
        self.entries.lock().unwrap().push(entry.into());
    }

    pub fn entries(&self) -> Vec<String> {
        self.entries.lock().unwrap().clone()
    }

    pub fn count(&self, entry: &str) -> usize {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|logged| *logged == entry)
            .count()
    }
}

/// Record an entry in the current test's recorder.
pub fn record(entry: impl Into<String>) {
    app::<Recorder>().push(entry);
}

/// The entries recorded so far.
pub fn recorded() -> Vec<String> {
    app::<Recorder>().entries()
}

/// A test application.
pub struct TestApp {
    pub container: Arc<Container>,
    _guard: LocalInstanceGuard,
}

impl TestApp {
    pub fn config(&self) -> Arc<Repository> {
        self.container.make::<Repository>()
    }
}

/// The default test configuration: an in-memory `array` default queue,
/// plus `sync`, `deferred`, `background` and `null` connections, and an
/// array cache store.
pub fn config() -> Value {
    json!({
        "queue": {
            "default": "array",
            "connections": {
                "array": {"driver": "array", "queue": "default"},
                "secondary": {"driver": "array", "queue": "default"},
                "sync": {"driver": "sync"},
                "deferred": {"driver": "deferred"},
                "background": {"driver": "background"},
                "null": {"driver": "null"},
            },
            "failed": {"driver": "database-uuids"},
        },
        "cache": {
            "default": "array",
            "stores": {"array": {"driver": "array", "serialize": false}},
        },
    })
}

/// Boot a test application with the given configuration.
pub fn app_with(config: Value) -> TestApp {
    let container = Arc::new(Container::new());
    let guard = Container::set_local_instance(container.clone());
    container.instance(Repository::new(config));
    container.instance(Recorder::default());
    CacheServiceProvider.register(&container);
    QueueServiceProvider.register(&container);
    BusServiceProvider.register(&container);
    TestApp {
        container,
        _guard: guard,
    }
}

/// Boot a test application with the default configuration.
pub fn app_default() -> TestApp {
    app_with(config())
}

/// Record every queue event's name.
pub fn record_events() -> Arc<Mutex<Vec<String>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = events.clone();
    Queue::listen_all(move |event: &QueueEvent| log.lock().unwrap().push(event.name().to_string()));
    events
}
