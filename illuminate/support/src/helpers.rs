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
    if !key.contains('*') && !key.contains('{') {
        return target.dot_or_null(key);
    }
    let segments: Vec<&str> = key.split('.').collect();
    data_get_segments(target, &segments).unwrap_or(Value::Null)
}

/// Get an item from a value using "dot" notation, or the given default.
///
/// ```
/// use illuminate_support::{data_get_or, json};
///
/// let data = json!({"products": {"desk": {"price": 100}}});
/// assert_eq!(data_get_or(&data, "products.desk.discount", 0), json!(0));
/// ```
pub fn data_get_or(target: &Value, key: &str, default: impl Into<Value>) -> Value {
    if !key.contains('*') && !key.contains('{') {
        return target.dot(key).cloned().unwrap_or_else(|| default.into());
    }
    let segments: Vec<&str> = key.split('.').collect();
    data_get_segments(target, &segments).unwrap_or_else(|| default.into())
}

/// Walk the segments of a key, supporting `*`, `{first}` and `{last}`.
fn data_get_segments(target: &Value, segments: &[&str]) -> Option<Value> {
    let Some((segment, rest)) = segments.split_first() else {
        return Some(target.clone());
    };
    match *segment {
        "*" => {
            let children: Vec<&Value> = match target {
                Value::Array(items) => items.iter().collect(),
                Value::Object(map) => map.values().collect(),
                _ => return None,
            };
            let results: Vec<Value> = children
                .into_iter()
                .map(|child| data_get_segments(child, rest).unwrap_or(Value::Null))
                .collect();
            if rest.contains(&"*") {
                Some(Value::Array(
                    results
                        .into_iter()
                        .flat_map(|v| match v {
                            Value::Array(items) => items,
                            _ => Vec::new(),
                        })
                        .collect(),
                ))
            } else {
                Some(Value::Array(results))
            }
        }
        "{first}" | "{last}" => {
            let child = match target {
                Value::Array(items) if *segment == "{first}" => items.first(),
                Value::Array(items) => items.last(),
                Value::Object(map) if *segment == "{first}" => map.values().next(),
                Value::Object(map) => map.values().next_back(),
                _ => None,
            }?;
            data_get_segments(child, rest)
        }
        other => {
            let other = match other {
                "\\*" => "*",
                "\\{first}" => "{first}",
                "\\{last}" => "{last}",
                other => other,
            };
            let child = match target {
                Value::Object(map) => map.get(other),
                Value::Array(items) => other.parse::<usize>().ok().and_then(|i| items.get(i)),
                _ => None,
            }?;
            data_get_segments(child, rest)
        }
    }
}

/// Set an item on a value using "dot" notation (`*` wildcards supported).
///
/// ```
/// use illuminate_support::{data_set, json};
///
/// let mut data = json!({"products": [{"name": "Desk 1", "price": 100}, {"name": "Desk 2", "price": 150}]});
/// data_set(&mut data, "products.*.price", 200);
/// assert_eq!(data, json!({"products": [{"name": "Desk 1", "price": 200}, {"name": "Desk 2", "price": 200}]}));
/// ```
pub fn data_set(target: &mut Value, key: &str, value: impl Into<Value>) {
    if key.contains('*') {
        let segments: Vec<&str> = key.split('.').collect();
        set_segments(target, &segments, &value.into(), true);
    } else {
        crate::arr::Arr::set(target, key, value);
    }
}

/// Set an item using "dot" notation only if it is missing (`*` wildcards supported).
///
/// ```
/// use illuminate_support::{data_fill, json};
///
/// let mut data = json!({"products": {"desk": {"price": 100}}});
/// data_fill(&mut data, "products.desk.price", 200);
/// data_fill(&mut data, "products.desk.discount", 10);
/// assert_eq!(data, json!({"products": {"desk": {"price": 100, "discount": 10}}}));
///
/// let mut data = json!({"products": [{"name": "Desk 1", "price": 100}, {"name": "Desk 2"}]});
/// data_fill(&mut data, "products.*.price", 200);
/// assert_eq!(data, json!({"products": [{"name": "Desk 1", "price": 100}, {"name": "Desk 2", "price": 200}]}));
/// ```
pub fn data_fill(target: &mut Value, key: &str, value: impl Into<Value>) {
    let segments: Vec<&str> = key.split('.').collect();
    set_segments(target, &segments, &value.into(), false);
}

/// Laravel's `data_set` algorithm.
fn set_segments(target: &mut Value, segments: &[&str], value: &Value, overwrite: bool) {
    let Some((segment, rest)) = segments.split_first() else {
        return;
    };
    if *segment == "*" {
        if !matches!(target, Value::Array(_) | Value::Object(_)) {
            *target = Value::Array(Vec::new());
        }
        let children: Vec<&mut Value> = match target {
            Value::Array(items) => items.iter_mut().collect(),
            Value::Object(map) => map.values_mut().collect(),
            _ => Vec::new(),
        };
        for child in children {
            if !rest.is_empty() {
                set_segments(child, rest, value, overwrite);
            } else if overwrite {
                *child = value.clone();
            }
        }
        return;
    }
    if !matches!(target, Value::Array(_) | Value::Object(_)) {
        *target = Value::Object(crate::value::Map::new());
    }
    if let Value::Array(items) = target {
        match segment.parse::<usize>() {
            Ok(index) if index <= items.len() => {
                if index == items.len() {
                    items.push(Value::Null);
                }
                let child = &mut items[index];
                if !rest.is_empty() {
                    set_segments(child, rest, value, overwrite);
                } else if overwrite || child.is_null() {
                    *child = value.clone();
                }
                return;
            }
            _ => {
                let map: crate::value::Map<String, Value> = std::mem::take(items)
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v))
                    .collect();
                *target = Value::Object(map);
            }
        }
    }
    if let Value::Object(map) = target {
        if !rest.is_empty() {
            let child = map.entry(segment.to_string()).or_insert(Value::Null);
            set_segments(child, rest, value, overwrite);
        } else if overwrite || !map.contains_key(*segment) {
            map.insert(segment.to_string(), value.clone());
        }
    }
}

/// Remove an item using "dot" notation (`*` wildcards supported).
///
/// ```
/// use illuminate_support::{data_forget, json};
///
/// let mut data = json!({"products": [{"name": "Desk 1", "price": 100}, {"name": "Desk 2", "price": 150}]});
/// data_forget(&mut data, "products.*.price");
/// assert_eq!(data, json!({"products": [{"name": "Desk 1"}, {"name": "Desk 2"}]}));
/// ```
pub fn data_forget(target: &mut Value, key: &str) {
    let segments: Vec<&str> = key.split('.').collect();
    forget_segments(target, &segments);
}

fn forget_segments(target: &mut Value, segments: &[&str]) {
    let Some((segment, rest)) = segments.split_first() else {
        return;
    };
    if *segment == "*" {
        if rest.is_empty() {
            return;
        }
        match target {
            Value::Array(items) => items.iter_mut().for_each(|child| forget_segments(child, rest)),
            Value::Object(map) => map.values_mut().for_each(|child| forget_segments(child, rest)),
            _ => {}
        }
        return;
    }
    let child = match target {
        Value::Object(map) => map.get_mut(*segment),
        Value::Array(items) => segment.parse::<usize>().ok().and_then(|i| items.get_mut(i)),
        _ => None,
    };
    match child {
        Some(child) if !rest.is_empty() => forget_segments(child, rest),
        _ if rest.is_empty() => match target {
            Value::Object(map) => {
                map.shift_remove(*segment);
            }
            Value::Array(items) => {
                if let Some(index) = segment.parse::<usize>().ok().filter(|i| *i < items.len()) {
                    items.remove(index);
                }
            }
            _ => {}
        },
        _ => {}
    }
}

/// Determine whether a value is "blank" (see [`Blank`]).
pub fn is_blank<T: Blank + ?Sized>(value: &T) -> bool {
    value.blank()
}

/// Determine whether a value is "filled" (see [`Blank`]).
pub fn is_filled<T: Blank + ?Sized>(value: &T) -> bool {
    value.filled()
}

/// Laravel's notion of blankness: `None`, empty or whitespace-only strings
/// and empty collections are blank; numbers and booleans never are.
///
/// ```
/// use illuminate_support::{Blank, collect, json};
///
/// assert!("   ".blank());
/// assert!(None::<i32>.blank());
/// assert!(collect(Vec::<i32>::new()).blank());
/// assert!(json!(null).blank());
/// assert!(!0.blank());
/// assert!(!false.blank());
/// assert!(Some("Taylor").filled());
/// ```
pub trait Blank {
    /// Determine if the value is blank.
    fn blank(&self) -> bool;

    /// Determine if the value is filled (not blank).
    fn filled(&self) -> bool {
        !self.blank()
    }
}

impl<T: Blank + ?Sized> Blank for &T {
    fn blank(&self) -> bool {
        (**self).blank()
    }
}

impl<T: Blank> Blank for Option<T> {
    fn blank(&self) -> bool {
        self.as_ref().is_none_or(Blank::blank)
    }
}

impl Blank for str {
    fn blank(&self) -> bool {
        self.trim().is_empty()
    }
}

impl Blank for String {
    fn blank(&self) -> bool {
        self.as_str().blank()
    }
}

impl Blank for Value {
    fn blank(&self) -> bool {
        self.is_blank()
    }
}

impl Blank for Stringable {
    fn blank(&self) -> bool {
        self.value().blank()
    }
}

impl Blank for crate::html_string::HtmlString {
    fn blank(&self) -> bool {
        self.to_html().blank()
    }
}

impl Blank for crate::message_bag::MessageBag {
    fn blank(&self) -> bool {
        self.is_empty()
    }
}

impl<T> Blank for [T] {
    fn blank(&self) -> bool {
        self.is_empty()
    }
}

impl<T> Blank for Vec<T> {
    fn blank(&self) -> bool {
        self.is_empty()
    }
}

impl<T> Blank for crate::collection::Collection<T> {
    fn blank(&self) -> bool {
        self.is_empty()
    }
}

impl<K, V, S> Blank for indexmap::IndexMap<K, V, S> {
    fn blank(&self) -> bool {
        self.is_empty()
    }
}

impl<K, V, S> Blank for std::collections::HashMap<K, V, S> {
    fn blank(&self) -> bool {
        self.is_empty()
    }
}

macro_rules! never_blank {
    ($($ty:ty),*) => {
        $(impl Blank for $ty {
            fn blank(&self) -> bool {
                false
            }
        })*
    };
}

never_blank!(bool, char, i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64, Carbon);

/// Transform the value with the callback if it is filled.
///
/// ```
/// use illuminate_support::{transform, transform_or};
///
/// assert_eq!(transform(5, |n| n * 2), Some(10));
/// assert_eq!(transform(None::<i32>, |n| n.unwrap() * 2), None);
/// assert_eq!(transform_or("", |s| s.len(), 0), 0);
/// ```
pub fn transform<T: Blank, R>(value: T, callback: impl FnOnce(T) -> R) -> Option<R> {
    if value.filled() { Some(callback(value)) } else { None }
}

/// Transform the value with the callback if it is filled, or return the default.
pub fn transform_or<T: Blank, R>(value: T, callback: impl FnOnce(T) -> R, default: R) -> R {
    transform(value, callback).unwrap_or(default)
}

/// Return the value if the condition is true.
///
/// ```
/// use illuminate_support::{when, when_fn};
///
/// assert_eq!(when(true, "Hello World"), Some("Hello World"));
/// assert_eq!(when(false, "Hello World"), None);
/// assert_eq!(when_fn(true, || 1 + 1), Some(2));
/// ```
pub fn when<T>(condition: bool, value: T) -> Option<T> {
    condition.then_some(value)
}

/// Return the callback's value if the condition is true.
pub fn when_fn<T>(condition: bool, callback: impl FnOnce() -> T) -> Option<T> {
    condition.then(callback)
}

/// Replace a given pattern with each value in the array, in order.
///
/// ```
/// use illuminate_support::preg_replace_array;
///
/// let replaced = preg_replace_array("/:[a-z_]+/", &["8:30", "9:00"], "The event will take place between :start and :end");
/// assert_eq!(replaced, "The event will take place between 8:30 and 9:00");
/// ```
pub fn preg_replace_array(pattern: &str, replacements: &[&str], subject: &str) -> String {
    let mut replacements = replacements.iter();
    crate::preg::replace_callback(
        pattern,
        |_| replacements.next().map(|r| r.to_string()).unwrap_or_default(),
        subject,
        None,
    )
    .unwrap_or_else(|_| subject.to_string())
}

/// Determine whether the current environment is Windows based.
pub fn windows_os() -> bool {
    cfg!(windows)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_gets_data_with_wildcards_and_positions() {
        let data = json!({"product-one": {"name": "Desk 1", "price": 100}, "product-two": {"name": "Desk 2", "price": 150}});
        assert_eq!(data_get(&data, "*.name"), json!(["Desk 1", "Desk 2"]));
        let flight = json!({"segments": [
            {"from": "LHR", "departure": "9:00", "to": "IST", "arrival": "15:00"},
            {"from": "IST", "departure": "16:00", "to": "PKX", "arrival": "20:00"},
        ]});
        assert_eq!(data_get(&flight, "segments.{first}.arrival"), json!("15:00"));
        assert_eq!(data_get(&flight, "segments.{last}.arrival"), json!("20:00"));
        assert_eq!(data_get(&flight, "segments.*.from"), json!(["LHR", "IST"]));
        assert_eq!(data_get_or(&flight, "segments.{first}.gate", "A1"), json!("A1"));
        let nested = json!({"users": [{"posts": [{"id": 1}, {"id": 2}]}, {"posts": [{"id": 3}]}]});
        assert_eq!(data_get(&nested, "users.*.posts.*.id"), json!([1, 2, 3]));
    }

    #[test]
    fn it_sets_fills_and_forgets_data() {
        let mut data = json!({"products": {"desk": {"price": 100}}});
        data_set(&mut data, "products.desk.price", 200);
        assert_eq!(data, json!({"products": {"desk": {"price": 200}}}));
        data_fill(&mut data, "products.desk.price", 300);
        assert_eq!(data["products"]["desk"]["price"], json!(200));
        data_forget(&mut data, "products.desk.price");
        assert_eq!(data, json!({"products": {"desk": {}}}));

        let mut list = json!({"items": [{"a": 1}, {"a": 2}]});
        data_fill(&mut list, "items.*.b", 0);
        assert_eq!(list, json!({"items": [{"a": 1, "b": 0}, {"a": 2, "b": 0}]}));
        data_set(&mut list, "items.*", "x");
        assert_eq!(list, json!({"items": ["x", "x"]}));
        let mut empty = json!(null);
        data_set(&mut empty, "a.*.b", 1);
        assert_eq!(empty, json!({"a": []}));
    }

    #[test]
    fn it_checks_blankness() {
        assert!(is_blank(""));
        assert!(is_blank("   "));
        assert!(is_blank(&None::<String>));
        assert!(is_blank(&Vec::<i32>::new()));
        assert!(is_filled(&0));
        assert!(is_filled(&true));
        assert!(is_filled(&false));
        assert!(is_filled(&Some(json!("x"))));
        assert!(blank(json!(null)));
        assert!(filled(0));
        assert_eq!(transform(json!(null), |v| v), None);
        assert_eq!(transform_or(json!(5), |v| v.as_i64().unwrap() * 2, 0), 10);
        assert!(!windows_os() || cfg!(windows));
        assert_eq!(preg_replace_array("/\\?/", &["a"], "? ?"), "a ");
    }
}
