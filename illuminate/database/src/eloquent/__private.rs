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

/// The model's hidden and visible attributes: the class defaults, or the
/// instance's own lists after `make_hidden` / `make_visible`.
pub fn visibility<M: super::Model>(model: &M) -> (Vec<String>, Vec<String>) {
    (model.get_hidden(), model.get_visible())
}

/// Determine whether an attribute is visible, given the hidden and visible lists.
pub fn is_visible_in(key: &str, hidden: &[String], visible: &[String]) -> bool {
    (visible.is_empty() || visible.iter().any(|v| v == key)) && !hidden.iter().any(|h| h == key)
}

/// Detects, at the derive's expansion site, whether a model implements
/// [`Prunable`](super::Prunable) or [`MassPrunable`](super::MassPrunable)
/// (autoref specialization: `(&&&PruneProbe::<M>::new()).pruner()`).
pub struct PruneProbe<M>(std::marker::PhantomData<fn() -> M>);

impl<M> PruneProbe<M> {
    /// A probe for the model `M`.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

/// Picked when the model implements `Prunable`.
pub trait ViaPrunable {
    /// The model's pruner.
    fn pruner(&self) -> Option<super::registry::Pruner>;
}

impl<M: super::Prunable> ViaPrunable for &&PruneProbe<M> {
    fn pruner(&self) -> Option<super::registry::Pruner> {
        Some(super::registry::Pruner::prunable::<M>())
    }
}

/// Picked when the model implements `MassPrunable`.
pub trait ViaMassPrunable {
    /// The model's pruner.
    fn pruner(&self) -> Option<super::registry::Pruner>;
}

impl<M: super::MassPrunable> ViaMassPrunable for &PruneProbe<M> {
    fn pruner(&self) -> Option<super::registry::Pruner> {
        Some(super::registry::Pruner::mass_prunable::<M>())
    }
}

/// Picked when the model isn't prunable.
pub trait ViaNotPrunable {
    /// No pruner.
    fn pruner(&self) -> Option<super::registry::Pruner>;
}

impl<M> ViaNotPrunable for PruneProbe<M> {
    fn pruner(&self) -> Option<super::registry::Pruner> {
        None
    }
}
