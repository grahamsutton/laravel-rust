//! Global helper functions.

use std::future::Future;
use std::time::Duration;

use serde_json::Value;

use crate::carbon::Carbon;
use crate::str::Str;
use crate::stringable::Stringable;
use crate::value::ValueExt;

pub use crate::collection::collect;
pub use crate::env::env;
pub use crate::error::{throw_if, throw_unless};
pub use crate::html_string::e;

/// Create a new [`Carbon`] instance for the current time.
pub fn now() -> Carbon {
    Carbon::now()
}

/// Create a new [`Carbon`] instance for the current date.
pub fn today() -> Carbon {
    Carbon::today()
}

/// Get a new fluent [`Stringable`] for the given string.
pub fn str(value: impl Into<String>) -> Stringable {
    Stringable::new(value)
}

/// Determine if the given value is "blank".
pub fn blank(value: impl Into<Value>) -> bool {
    value.into().is_blank()
}

/// Determine if a value is "filled".
pub fn filled(value: impl Into<Value>) -> bool {
    !blank(value)
}

/// Call the given closure with the given value, then return the value.
pub fn tap<T>(mut value: T, callback: impl FnOnce(&mut T)) -> T {
    callback(&mut value);
    value
}

/// Pass the given value through the callback and return the result.
pub fn with<T, R>(value: T, callback: impl FnOnce(T) -> R) -> R {
    callback(value)
}

/// Get the class "basename" of the given type.
///
/// ```
/// use illuminate_support::class_basename;
///
/// struct PodcastPublished;
/// assert_eq!(class_basename::<PodcastPublished>(), "PodcastPublished");
/// ```
pub fn class_basename<T: ?Sized>() -> String {
    Str::class_basename(std::any::type_name::<T>())
}

/// Get an item from a value using "dot" notation, with wildcard support.
///
/// ```
/// use illuminate_support::{data_get, json};
///
/// let data = json!({"products": [{"name": "Desk"}, {"name": "Chair"}]});
/// assert_eq!(data_get(&data, "products.*.name"), json!(["Desk", "Chair"]));
/// assert_eq!(data_get(&data, "products.0.name"), json!("Desk"));
/// ```
pub fn data_get(target: &Value, key: &str) -> Value {
    if !key.contains('*') {
        return target.dot_or_null(key);
    }
    let mut segments = key.splitn(2, '.');
    let head = segments.next().unwrap_or_default();
    let rest = segments.next();

    if head == "*" {
        let children: Vec<&Value> = match target {
            Value::Array(items) => items.iter().collect(),
            Value::Object(map) => map.values().collect(),
            _ => return Value::Null,
        };
        let results: Vec<Value> = children
            .into_iter()
            .map(|child| match rest {
                Some(rest) => data_get(child, rest),
                None => child.clone(),
            })
            .collect();
        // Collapse nested wildcards one level, like Laravel.
        if rest.is_some_and(|r| r.contains('*')) {
            return Value::Array(
                results
                    .into_iter()
                    .flat_map(|v| match v {
                        Value::Array(items) => items,
                        other => vec![other],
                    })
                    .collect(),
            );
        }
        return Value::Array(results);
    }

    let (before, after) = key.split_once(".*").unwrap_or((key, ""));
    let next = target.dot_or_null(before);
    if after.is_empty() {
        return data_get(&next, "*");
    }
    data_get(&next, &format!("*{after}"))
}

/// Set an item on a value using "dot" notation.
pub fn data_set(target: &mut Value, key: &str, value: impl Into<Value>) {
    crate::arr::Arr::set(target, key, value);
}

/// Retry an asynchronous operation a given number of times.
///
/// ```
/// # tokio_test();
/// # fn tokio_test() {
/// use illuminate_support::retry;
///
/// let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
/// let mut attempts = 0;
/// let result = runtime.block_on(retry(3, 0, |attempt| {
///     attempts = attempt;
///     async move { if attempt < 3 { Err("not yet") } else { Ok("done") } }
/// }));
/// assert_eq!(result, Ok("done"));
/// assert_eq!(attempts, 3);
/// # }
/// ```
pub async fn retry<T, E, F, Fut>(times: usize, sleep_ms: u64, mut callback: F) -> Result<T, E>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match callback(attempt).await {
            Ok(value) => return Ok(value),
            Err(error) if attempt >= times => return Err(error),
            Err(_) => {
                if sleep_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                }
            }
        }
    }
}

/// Return the first element of a slice.
pub fn head<T>(items: &[T]) -> Option<&T> {
    items.first()
}

/// Return the last element of a slice.
pub fn last<T>(items: &[T]) -> Option<&T> {
    items.last()
}
