//! Helpers used by the code `#[derive(Model)]` generates. Not public API.

use illuminate_support::{Map, Result, Str, Value};
use serde::de::DeserializeOwned;

pub use serde;

/// The conventional table name for a model class: the snake-cased plural
/// (`User` → `users`, `BlogPost` → `blog_posts`).
pub fn table_name_for(class: &str) -> String {
    Str::snake(&Str::plural_studly(class))
}

/// Take an attribute out of a row, casting it to the field's type.
///
/// Missing attributes fall back to the field's default, so models may be
/// hydrated from partial selects.
pub fn take_attribute<T: DeserializeOwned + Default>(
    attributes: &mut Map<String, Value>,
    key: &str,
    class: &str,
) -> Result<T> {
    match attributes.remove(key) {
        None => Ok(T::default()),
        Some(value) => cast_attribute(value, key, class),
    }
}

/// Cast a value to an attribute's type, leniently (SQLite booleans arrive as
/// integers, JSON columns as strings, dates as text, ...).
pub fn cast_attribute<T: DeserializeOwned>(value: Value, key: &str, class: &str) -> Result<T> {
    match crate::de::from_value::<T>(value.clone()) {
        Ok(cast) => Ok(cast),
        Err(lenient) => illuminate_support::cast::<T>(value).map_err(|_| {
            anyhow::anyhow!("Unable to cast attribute [{key}] on model [{class}]: {lenient}")
        }),
    }
}

/// Determine whether an attribute is visible when serializing a model.
pub fn is_visible(key: &str, hidden: &[&str], visible: &[&str]) -> bool {
    (visible.is_empty() || visible.contains(&key)) && !hidden.contains(&key)
}
