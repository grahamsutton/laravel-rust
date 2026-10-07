//! # Eloquent
//!
//! Eloquent is the framework's object-relational mapper. Each database table
//! has a corresponding "model" — a plain struct deriving [`Model`] — used to
//! interact with that table:
//!
//! ```ignore
//! use illuminate_database::eloquent::*;
//!
//! #[derive(Debug, Clone, Default, Model)]
//! #[fillable(name, email)]
//! #[hidden(password)]
//! pub struct User {
//!     pub id: u64,
//!     pub name: String,
//!     pub email: String,
//!     #[hashed]
//!     pub password: String,
//!     pub created_at: Option<Carbon>,
//!     pub updated_at: Option<Carbon>,
//!
//!     #[relation]
//!     pub posts: Option<Vec<Post>>,
//!
//!     pub original: Original,
//! }
//!
//! impl User {
//!     pub fn posts(&self) -> HasMany<Self, Post> {
//!         self.has_many()
//!     }
//! }
//!
//! let user = User::create(json!({"name": "Taylor", "email": "taylor@laravel.com"})).await?;
//! let users = User::where_("active", true).with("posts").latest().get().await?;
//! ```
//!
//! ## Plain structs
//!
//! Models are plain structs, which shapes a few behaviours:
//!
//! - Eloquent tracks "dirty" attributes only for models with an
//!   [`Original`] field: `is_dirty`, `get_dirty`, `was_changed`,
//!   `get_original`, ... report what changed since the model was retrieved,
//!   and `save()` writes only the dirty columns, skipping the `UPDATE` (and
//!   the `updating` / `updated` events) when nothing changed. Without the
//!   field, saving an existing model writes every persisted column except
//!   the primary key.
//! - A model with an `Original` field "exists" once it has been retrieved
//!   or saved. Without one, a model exists when its key is set (non-null,
//!   non-zero, non-empty), and models with non-incrementing keys (UUIDs,
//!   ULIDs, natural keys) check the database before deciding between an
//!   insert and an update.
//! - On insert, `None` attributes are left out so the database applies its
//!   column defaults; other fields are written with their Rust value (use an
//!   `Option` field to rely on a column default).
//! - Relationships are loaded into `#[relation]` fields named after the
//!   relationship method, and `#[computed]` fields receive extra selected
//!   columns such as `posts_count` or a many-to-many `pivot`.
//!
//! ## Attributes
//!
//! `create`, `fill`, `update`, `first_or_create`, ... accept anything that
//! converts into [`Attributes`]: a `json!` object, a map, pairs, or the
//! [`attrs!`](crate::attrs) macro.
//!
//! ## Events
//!
//! Models fire events through their lifecycle (`creating`, `updated`,
//! `deleted`, ...). Listen with closures (`User::created(|user| ...)`),
//! [`Observer`]s (`User::observe(...)` or `#[observed_by(...)]`), or — once
//! the framework has set an [`EventDispatcher`] — named listeners on the
//! `Event` facade (`eloquent.created: User`).
//!
//! ## Scopes, pruning, and the registry
//!
//! Global scopes are closures (`add_global_scope`) or [`Scope`] objects
//! (`add_global_scope_object`, `#[scoped_by(...)]`). Models implementing
//! [`Prunable`] or [`MassPrunable`] are pruned by `model:prune`, and every
//! model is listed in the [`registry`] that powers `model:show`.

#[macro_use]
mod forward;

mod builder;
mod collection;
mod errors;
mod events;
pub mod factories;
pub mod faker;
mod model;
mod original;
mod prunable;
pub mod registry;
pub mod relations;
mod scope;
mod state;

#[doc(hidden)]
pub mod __private;

use std::future::Future;
use std::pin::Pin;

pub use builder::{Builder, TrashedMode};
pub use collection::EloquentCollection;
pub use errors::{MassAssignmentException, ModelNotFoundException, RelationNotFoundException};
pub use events::{
    EventDispatcher, EventOutcome, ModelEvent, Observer, has_event_dispatcher,
    set_event_dispatcher, unset_event_dispatcher,
};
pub use factories::{Factory, FactoryBuilder, HasFactory, Sequence};
pub use faker::Faker;
pub use model::Model;
pub use original::{IntoAttributeNames, Original};
pub use prunable::{MassPrunable, Prunable, PruneKind};
pub use relations::{
    BelongsTo, BelongsToMany, Constraint, DynRelation, EagerSpec, FromEager, HasMany,
    HasManyThrough, HasOne, HasOneOrMany, HasOneOrManyThrough, HasOneThrough, MorphMany, MorphOne,
    MorphTo, Relation, RelationKind, RelationValue, SyncChanges,
};
pub use scope::{Scope, scope_name};
pub use state::{
    hash_using, is_hashed, is_unguarded, morph_map, reguard, report_exceptions_using, unguard,
    unguarded, without_events,
};

/// `#[derive(Model)]`: turn a struct into an Eloquent model.
pub use illuminate_macros::Model;

/// The base query builder, for constraint closures.
pub use crate::query::Builder as QueryBuilder;

pub use crate::expression::{Expression, raw};
pub use async_trait::async_trait;
pub use illuminate_support::{Carbon, Collection, Conditionable, Map, Tappable, Value, json};

/// A boxed, `Send` future (the shape of type-erased async model methods).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The type of a model's primary key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyType {
    /// An integer key (usually auto-incrementing).
    Int,
    /// A string key (UUIDs, ULIDs, slugs, ...).
    String,
}

/// Unique identifiers generated for new models' keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UniqueIds {
    /// The key is supplied by the database or the application.
    None,
    /// An ordered (version 7) UUID, lowercase.
    Uuid,
    /// A ULID, lowercase.
    Ulid,
}

/// Attributes handed to `create`, `fill`, `update`, `first_or_create`, ...:
/// a JSON object, a map, or a list of `(column, value)` pairs.
///
/// ```
/// use illuminate_database::eloquent::Attributes;
/// use illuminate_support::json;
///
/// let from_json = Attributes::from(json!({"name": "Taylor"}));
/// let from_pairs = Attributes::from([("name", "Taylor")]);
/// assert_eq!(from_json, from_pairs);
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attributes(pub Map<String, Value>);

impl Attributes {
    /// An empty set of attributes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set an attribute.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> &mut Self {
        let value = Carbon::with_storage_format(|| value.into());
        self.0.insert(key.into(), value);
        self
    }

    /// Get an attribute.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// Merge other attributes over these ones.
    pub fn merge(mut self, other: impl Into<Attributes>) -> Self {
        for (key, value) in other.into().0 {
            self.0.insert(key, value);
        }
        self
    }

    /// The underlying map.
    pub fn into_map(self) -> Map<String, Value> {
        self.0
    }

    /// Whether there are no attributes.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Value> for Attributes {
    fn from(value: Value) -> Self {
        match value {
            Value::Object(map) => Self(map),
            _ => Self::default(),
        }
    }
}

impl From<Map<String, Value>> for Attributes {
    fn from(map: Map<String, Value>) -> Self {
        Self(map)
    }
}

impl From<&Attributes> for Attributes {
    fn from(attributes: &Attributes) -> Self {
        attributes.clone()
    }
}

impl<K: Into<String>, V: Into<Value>, const N: usize> From<[(K, V); N]> for Attributes {
    fn from(pairs: [(K, V); N]) -> Self {
        let mut attributes = Self::new();
        for (key, value) in pairs {
            attributes.insert(key, value);
        }
        attributes
    }
}

impl<K: Into<String>, V: Into<Value>> From<Vec<(K, V)>> for Attributes {
    fn from(pairs: Vec<(K, V)>) -> Self {
        let mut attributes = Self::new();
        for (key, value) in pairs {
            attributes.insert(key, value);
        }
        attributes
    }
}

impl From<Attributes> for Value {
    fn from(attributes: Attributes) -> Self {
        Value::Object(attributes.0)
    }
}

/// One or more primary keys (`find_many`, `destroy`, `attach`, ...).
pub trait IntoIds {
    /// Convert into a list of keys.
    fn into_ids(self) -> Vec<Value>;
}

macro_rules! scalar_ids {
    ($($ty:ty),*) => {$(
        impl IntoIds for $ty {
            fn into_ids(self) -> Vec<Value> {
                vec![Value::from(self)]
            }
        }
    )*};
}

scalar_ids!(i32, i64, u32, u64, usize, &str, String);

impl IntoIds for Value {
    fn into_ids(self) -> Vec<Value> {
        match self {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            other => vec![other],
        }
    }
}

impl<T: Into<Value>> IntoIds for Vec<T> {
    fn into_ids(self) -> Vec<Value> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<Value>, const N: usize> IntoIds for [T; N] {
    fn into_ids(self) -> Vec<Value> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<Value> + Clone> IntoIds for &[T] {
    fn into_ids(self) -> Vec<Value> {
        self.iter().cloned().map(Into::into).collect()
    }
}

impl<T: Into<Value>> IntoIds for Collection<T> {
    fn into_ids(self) -> Vec<Value> {
        self.into_iter().map(Into::into).collect()
    }
}

/// One or more relationship names (`with`, `load`, `with_count`, ...).
pub trait IntoRelations {
    /// Convert into relation names.
    fn into_relations(self) -> Vec<String>;
}

impl IntoRelations for &str {
    fn into_relations(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl IntoRelations for String {
    fn into_relations(self) -> Vec<String> {
        vec![self]
    }
}

impl<T: Into<String>> IntoRelations for Vec<T> {
    fn into_relations(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<String>, const N: usize> IntoRelations for [T; N] {
    fn into_relations(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<String> + Clone> IntoRelations for &[T] {
    fn into_relations(self) -> Vec<String> {
        self.iter().cloned().map(Into::into).collect()
    }
}

/// A stable string form of a key, used to match related models.
pub(crate) fn key_string(value: &Value) -> Option<String> {
    use illuminate_support::ValueExt;
    match value {
        Value::Null => None,
        Value::Number(n) => Some(
            n.as_i64()
                .map(|i| i.to_string())
                .or_else(|| {
                    n.as_f64()
                        .filter(|f| f.fract() == 0.0)
                        .map(|f| (f as i64).to_string())
                })
                .unwrap_or_else(|| n.to_string()),
        ),
        other => Some(other.to_string_lossy()),
    }
}
