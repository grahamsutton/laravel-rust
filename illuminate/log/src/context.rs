//! Context: information you capture once and share throughout a request —
//! added to every log entry written while handling it, and carried along to
//! the queued jobs it dispatches.
//!
//! ```
//! use illuminate_log::Context;
//! use illuminate_support::json;
//!
//! # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(Context::run(async {
//! Context::add("url", "https://laravel.com/docs");
//! Context::add("trace_id", "8f5b0a");
//! Context::add_hidden("api_key", "secret");
//!
//! assert_eq!(Context::get("trace_id"), Some(json!("8f5b0a")));
//! assert_eq!(Context::all().len(), 2);
//! assert_eq!(Context::get_hidden("api_key"), Some(json!("secret")));
//! # }));
//! ```
//!
//! Inside an HTTP request the context belongs to that request, so
//! concurrent requests never see each other's data. Anywhere else, it
//! belongs to the current [`Context::run`] scope, or to the application.

use std::future::Future;
use std::sync::{Arc, RwLock};

use illuminate_container::Container;
use illuminate_support::{Map, Value, json};

tokio::task_local! {
    static SCOPED: Arc<ContextRepository>;
}

/// The data held by the context: visible data (added to logs) and hidden
/// data (never logged).
#[derive(Debug, Default)]
pub struct ContextRepository {
    data: RwLock<Map<String, Value>>,
    hidden: RwLock<Map<String, Value>>,
}

impl ContextRepository {
    /// Create an empty context.
    pub fn new() -> Self {
        Self::default()
    }

    fn data(&self, hidden: bool) -> &RwLock<Map<String, Value>> {
        if hidden { &self.hidden } else { &self.data }
    }
}

/// The `Context` facade.
pub struct Context;

impl Context {
    /// The context for the current request, scope, or application.
    pub fn repository() -> Arc<ContextRepository> {
        if let Some(request) = illuminate_http::current_request() {
            return match request.extension::<ContextRepository>() {
                Some(context) => context,
                None => {
                    let context = Arc::new(ContextRepository::new());
                    request.set_extension(context.clone());
                    context
                }
            };
        }
        if let Ok(context) = SCOPED.try_with(Arc::clone) {
            return context;
        }
        let container = Container::get_instance();
        container.singleton_if::<ContextRepository>(|_| Arc::new(ContextRepository::new()));
        container.make::<ContextRepository>()
    }

    /// The current context, without creating one.
    pub(crate) fn existing() -> Option<Arc<ContextRepository>> {
        if let Some(request) = illuminate_http::current_request() {
            return request.extension::<ContextRepository>();
        }
        if let Ok(context) = SCOPED.try_with(Arc::clone) {
            return Some(context);
        }
        illuminate_container::try_app::<ContextRepository>()
    }

    /// The visible context, for log records' `extra` data.
    pub(crate) fn for_logs() -> Map<String, Value> {
        Self::existing()
            .map(|context| context.data.read().unwrap().clone())
            .unwrap_or_default()
    }

    /// Run the future with a context of its own (each queued job gets one).
    pub async fn run<F: Future>(future: F) -> F::Output {
        SCOPED.scope(Arc::new(ContextRepository::new()), future).await
    }

    fn write(hidden: bool, callback: impl FnOnce(&mut Map<String, Value>)) {
        let repository = Self::repository();
        callback(&mut repository.data(hidden).write().unwrap());
    }

    fn read<T>(hidden: bool, callback: impl FnOnce(&Map<String, Value>) -> T) -> T {
        let repository = Self::repository();
        callback(&repository.data(hidden).read().unwrap())
    }

    // ------------------------------------------------------------------
    // Adding and reading
    // ------------------------------------------------------------------

    /// Add a value to the context.
    pub fn add(key: &str, value: impl Into<Value>) {
        let value = value.into();
        Self::write(false, |data| {
            data.insert(key.to_string(), value);
        });
    }

    /// Add several values to the context: `Context::add_many(json!({...}))`.
    pub fn add_many(values: Value) {
        if let Value::Object(values) = values {
            Self::write(false, |data| data.extend(values));
        }
    }

    /// Add a value only if the key isn't already in the context.
    pub fn add_if(key: &str, value: impl Into<Value>) {
        let value = value.into();
        Self::write(false, |data| {
            data.entry(key.to_string()).or_insert(value);
        });
    }

    /// Add a hidden value: available to your code and queued jobs, but never
    /// written to the logs.
    pub fn add_hidden(key: &str, value: impl Into<Value>) {
        let value = value.into();
        Self::write(true, |data| {
            data.insert(key.to_string(), value);
        });
    }

    /// Add a hidden value only if the key isn't already hidden in the context.
    pub fn add_hidden_if(key: &str, value: impl Into<Value>) {
        let value = value.into();
        Self::write(true, |data| {
            data.entry(key.to_string()).or_insert(value);
        });
    }

    /// Get a value from the context.
    pub fn get(key: &str) -> Option<Value> {
        Self::read(false, |data| data.get(key).cloned())
    }

    /// Get a hidden value from the context.
    pub fn get_hidden(key: &str) -> Option<Value> {
        Self::read(true, |data| data.get(key).cloned())
    }

    /// Get a value, adding it with the callback's result when it's missing.
    pub fn remember(key: &str, callback: impl FnOnce() -> Value) -> Value {
        if let Some(value) = Self::get(key) {
            return value;
        }
        let value = callback();
        Self::add(key, value.clone());
        value
    }

    /// Determine if the context has the key (even with a `null` value).
    pub fn has(key: &str) -> bool {
        Self::read(false, |data| data.contains_key(key))
    }

    /// Determine if the context is missing the key.
    pub fn missing(key: &str) -> bool {
        !Self::has(key)
    }

    /// Determine if the hidden context has the key.
    pub fn has_hidden(key: &str) -> bool {
        Self::read(true, |data| data.contains_key(key))
    }

    /// Every value in the context.
    pub fn all() -> Map<String, Value> {
        Self::read(false, Map::clone)
    }

    /// Every hidden value in the context.
    pub fn all_hidden() -> Map<String, Value> {
        Self::read(true, Map::clone)
    }

    /// Only the given keys.
    pub fn only(keys: &[&str]) -> Map<String, Value> {
        Self::read(false, |data| {
            keys.iter()
                .filter_map(|key| data.get(*key).map(|value| (key.to_string(), value.clone())))
                .collect()
        })
    }

    /// Everything except the given keys.
    pub fn except(keys: &[&str]) -> Map<String, Value> {
        Self::read(false, |data| {
            data.iter()
                .filter(|(key, _)| !keys.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
    }

    /// Remove a value from the context and return it.
    pub fn pull(key: &str) -> Option<Value> {
        let mut pulled = None;
        Self::write(false, |data| pulled = data.shift_remove(key));
        pulled
    }

    /// Remove a hidden value from the context and return it.
    pub fn pull_hidden(key: &str) -> Option<Value> {
        let mut pulled = None;
        Self::write(true, |data| pulled = data.shift_remove(key));
        pulled
    }

    /// Remove the given keys from the context.
    pub fn forget(keys: &[&str]) {
        Self::write(false, |data| {
            for key in keys {
                data.shift_remove(*key);
            }
        });
    }

    /// Remove the given keys from the hidden context.
    pub fn forget_hidden(keys: &[&str]) {
        Self::write(true, |data| {
            for key in keys {
                data.shift_remove(*key);
            }
        });
    }

    /// Increment a counter in the context (starting from zero).
    pub fn increment(key: &str, by: i64) -> i64 {
        let mut total = 0;
        Self::write(false, |data| {
            total = data.get(key).and_then(Value::as_i64).unwrap_or(0) + by;
            data.insert(key.to_string(), Value::from(total));
        });
        total
    }

    /// Decrement a counter in the context.
    pub fn decrement(key: &str, by: i64) -> i64 {
        Self::increment(key, -by)
    }

    // ------------------------------------------------------------------
    // Stacks
    // ------------------------------------------------------------------

    /// Push values onto a stack in the context: `Context::push("breadcrumbs", ["first"])`.
    pub fn push<I, V>(key: &str, values: I)
    where
        I: IntoIterator<Item = V>,
        V: Into<Value>,
    {
        Self::push_to(false, key, values.into_iter().map(Into::into).collect());
    }

    /// Push values onto a hidden stack.
    pub fn push_hidden<I, V>(key: &str, values: I)
    where
        I: IntoIterator<Item = V>,
        V: Into<Value>,
    {
        Self::push_to(true, key, values.into_iter().map(Into::into).collect());
    }

    fn push_to(hidden: bool, key: &str, values: Vec<Value>) {
        Self::write(hidden, |data| {
            let entry = data.entry(key.to_string()).or_insert_with(|| Value::Array(Vec::new()));
            if !entry.is_array() {
                *entry = Value::Array(vec![entry.take()]);
            }
            if let Value::Array(stack) = entry {
                stack.extend(values);
            }
        });
    }

    /// Pop the latest value off a stack in the context.
    pub fn pop(key: &str) -> Option<Value> {
        let mut popped = None;
        Self::write(false, |data| {
            if let Some(Value::Array(stack)) = data.get_mut(key) {
                popped = stack.pop();
            }
        });
        popped
    }

    /// Determine if a stack in the context contains the value.
    pub fn stack_contains(key: &str, value: impl Into<Value>) -> bool {
        let value = value.into();
        Self::read(false, |data| {
            matches!(data.get(key), Some(Value::Array(stack)) if stack.contains(&value))
        })
    }

    /// Determine if a hidden stack contains the value.
    pub fn hidden_stack_contains(key: &str, value: impl Into<Value>) -> bool {
        let value = value.into();
        Self::read(true, |data| {
            matches!(data.get(key), Some(Value::Array(stack)) if stack.contains(&value))
        })
    }

    // ------------------------------------------------------------------
    // Scopes, flushing, and queued jobs
    // ------------------------------------------------------------------

    /// Run the future with extra context, restoring the previous context
    /// afterwards.
    pub async fn scope<F: Future>(future: F, data: Value, hidden: Value) -> F::Output {
        let repository = Self::repository();
        let previous = (
            repository.data.read().unwrap().clone(),
            repository.hidden.read().unwrap().clone(),
        );
        if let Value::Object(data) = data {
            repository.data.write().unwrap().extend(data);
        }
        if let Value::Object(hidden) = hidden {
            repository.hidden.write().unwrap().extend(hidden);
        }
        let output = future.await;
        *repository.data.write().unwrap() = previous.0;
        *repository.hidden.write().unwrap() = previous.1;
        output
    }

    /// Determine if the context is empty.
    pub fn is_empty() -> bool {
        Self::all().is_empty() && Self::all_hidden().is_empty()
    }

    /// Forget everything in the context.
    pub fn flush() {
        let repository = Self::repository();
        repository.data.write().unwrap().clear();
        repository.hidden.write().unwrap().clear();
    }

    /// The context, ready to travel with a queued job (`None` when empty).
    pub fn dehydrate() -> Option<Value> {
        if Self::is_empty() {
            return None;
        }
        Some(json!({"data": Self::all(), "hidden": Self::all_hidden()}))
    }

    /// Restore a context captured with [`Context::dehydrate`].
    pub fn hydrate(context: &Value) {
        if let Some(Value::Object(data)) = context.get("data") {
            Self::write(false, |current| current.extend(data.clone()));
        }
        if let Some(Value::Object(hidden)) = context.get("hidden") {
            Self::write(true, |current| current.extend(hidden.clone()));
        }
    }
}
