//! Eloquent relationships.
//!
//! Relationships are defined as methods returning a relation, and loaded
//! into `#[relation]` fields of the same name:
//!
//! ```ignore
//! #[derive(Debug, Clone, Default, Model)]
//! pub struct User {
//!     pub id: u64,
//!     #[relation]
//!     pub posts: Option<Vec<Post>>,
//!     #[relation]
//!     pub profile: Option<Profile>,
//! }
//!
//! impl User {
//!     pub fn posts(&self) -> HasMany<Self, Post> {
//!         self.has_many()
//!     }
//!
//!     pub fn profile(&self) -> HasOne<Self, Profile> {
//!         self.has_one()
//!     }
//! }
//!
//! let posts = user.posts().where_("published", true).get().await?;
//! let users = User::with(["posts", "profile"]).get().await?;
//! ```

mod belongs_to;
mod belongs_to_many;
mod has_one_or_many;
mod through;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use illuminate_support::{Collection, Map, Result, Value};
use serde::Serialize;

use super::builder::Builder;
use super::model::Model;
use super::{BoxFuture, key_string};
use crate::query::Builder as QueryBuilder;

pub use belongs_to::{BelongsTo, MorphTo};
pub use belongs_to_many::BelongsToMany;
pub use has_one_or_many::{HasMany, HasOne, HasOneOrMany, MorphMany, MorphOne};
pub use through::{HasManyThrough, HasOneOrManyThrough, HasOneThrough};

/// A constraint applied to an eager load's query.
pub type Constraint = Arc<dyn Fn(QueryBuilder) -> QueryBuilder + Send + Sync>;

/// How a relationship should be eager loaded: nested relationships to load
/// on the related models, a query constraint and the columns to select.
#[derive(Clone, Default)]
pub struct EagerSpec {
    /// Relationships to load on the related models (`posts.comments`).
    pub nested: Vec<(String, EagerSpec)>,
    /// A constraint on the related query.
    pub constraint: Option<Constraint>,
    /// The columns to select (`posts:id,title`).
    pub columns: Option<Vec<String>>,
}

impl fmt::Debug for EagerSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EagerSpec")
            .field("nested", &self.nested)
            .field("constrained", &self.constraint.is_some())
            .field("columns", &self.columns)
            .finish()
    }
}

impl EagerSpec {
    /// An empty spec: load the relationship as defined.
    pub fn new() -> Self {
        Self::default()
    }

    /// Constrain the related query.
    pub fn constrain(
        mut self,
        constraint: impl Fn(QueryBuilder) -> QueryBuilder + Send + Sync + 'static,
    ) -> Self {
        self.constraint = Some(Arc::new(constraint));
        self
    }

    /// Apply the column selection and constraint to a base query.
    pub fn apply(&self, mut query: QueryBuilder) -> QueryBuilder {
        if let Some(columns) = &self.columns {
            query = query.select(columns.clone());
        }
        if let Some(constraint) = &self.constraint {
            query = constraint(query);
        }
        query
    }

    /// Apply the spec to a related model query: selection, constraint and
    /// nested eager loads.
    pub(crate) fn apply_to<R: Model>(&self, mut builder: Builder<R>) -> Builder<R> {
        if let Some(columns) = &self.columns {
            let qualified: Vec<String> =
                columns.iter().map(|c| builder.qualify_column(c)).collect();
            builder.query = builder.query.select(qualified);
        }
        if let Some(constraint) = &self.constraint {
            builder.query = constraint(builder.query);
        }
        for (name, spec) in &self.nested {
            merge_spec(&mut builder.eager, name, spec.clone());
        }
        builder
    }

    /// Parse relationship paths (`posts.comments`, `posts:id,title`) into
    /// a tree of eager loads.
    pub fn tree(relations: Vec<String>) -> Vec<(String, EagerSpec)> {
        let mut tree = Vec::new();
        for relation in relations {
            add_path(&mut tree, &relation, None);
        }
        tree
    }
}

fn merge_spec(tree: &mut Vec<(String, EagerSpec)>, name: &str, spec: EagerSpec) {
    match tree.iter_mut().find(|(existing, _)| existing == name) {
        Some((_, existing)) => {
            if spec.constraint.is_some() {
                existing.constraint = spec.constraint;
            }
            if spec.columns.is_some() {
                existing.columns = spec.columns;
            }
            for (nested, nested_spec) in spec.nested {
                merge_spec(&mut existing.nested, &nested, nested_spec);
            }
        }
        None => tree.push((name.to_string(), spec)),
    }
}

/// Add a relationship path to an eager load tree. The constraint and column
/// selection apply to the last relationship of the path.
pub(crate) fn add_path(
    tree: &mut Vec<(String, EagerSpec)>,
    path: &str,
    constraint: Option<Constraint>,
) {
    let (path, columns) = match path.split_once(':') {
        Some((path, columns)) => (
            path.trim(),
            Some(
                columns
                    .split(',')
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .collect(),
            ),
        ),
        None => (path.trim(), None),
    };
    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    let mut level = tree;
    for (index, segment) in segments.iter().enumerate() {
        let last = index + 1 == segments.len();
        let position = match level.iter().position(|(name, _)| name == segment) {
            Some(position) => position,
            None => {
                level.push((segment.to_string(), EagerSpec::default()));
                level.len() - 1
            }
        };
        let spec = &mut level[position].1;
        if last {
            if columns.is_some() {
                spec.columns = columns.clone();
            }
            if constraint.is_some() {
                spec.constraint = constraint.clone();
            }
        }
        level = &mut spec.nested;
    }
}

/// Remove a relationship (and its nested loads) from an eager load tree.
pub(crate) fn remove_path(tree: &mut Vec<(String, EagerSpec)>, path: &str) {
    let mut segments: Vec<&str> = path.split('.').collect();
    let Some(last) = segments.pop() else { return };
    let mut level = tree;
    for segment in segments {
        match level.iter_mut().find(|(name, _)| name == segment) {
            Some((_, spec)) => level = &mut spec.nested,
            None => return,
        }
    }
    level.retain(|(name, _)| name != last);
}

/// A relationship between a parent model `P` and related models.
///
/// Implemented by every relation type; `#[derive(Model)]` uses it to eager
/// load `#[relation]` fields.
pub trait Relation<P: Model>: Send + Sized + 'static {
    /// The related model.
    type Related: Model;

    /// What the relationship loads for one parent: `Vec<R>` or `Option<R>`.
    type Loaded: Send + 'static;

    /// Load the relationship for every parent with a single query, returning
    /// the results aligned with the parents.
    fn eager_load<'a>(
        self,
        parents: &'a [P],
        spec: EagerSpec,
    ) -> BoxFuture<'a, Result<Vec<Self::Loaded>>>;

    /// Erase the relationship's types (for `has`, `with_count`, factories).
    fn into_dyn(self) -> Box<dyn DynRelation>;
}

/// The kinds of relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationKind {
    /// `has_one` / `morph_one`.
    HasOne,
    /// `has_many` / `morph_many`.
    HasMany,
    /// `belongs_to` / `morph_to`.
    BelongsTo,
    /// `belongs_to_many`.
    BelongsToMany,
    /// `has_one_through`.
    HasOneThrough,
    /// `has_many_through`.
    HasManyThrough,
}

/// A type-erased relationship, used to build existence and aggregate
/// sub-queries (`has`, `where_has`, `with_count`, ...) and by factories.
pub trait DynRelation: Send + Sync {
    /// The kind of relationship.
    fn kind(&self) -> RelationKind;

    /// The related model's class name.
    fn related_class(&self) -> &'static str;

    /// The related model's table.
    fn related_table(&self) -> String;

    /// The query matching the related rows of a parent row, correlated with
    /// the parent query's table (or alias) `parent`. Returns the query and
    /// the table (or alias) the related model is selected from.
    fn existence_query(&self, parent: &str) -> (QueryBuilder, String);

    /// A relationship defined on the related model.
    fn related_relation(&self, name: &str) -> Option<Box<dyn DynRelation>>;

    /// Attributes a related model needs to belong to the parent
    /// (`{"user_id": 1}` for a has-many).
    fn attributes_for_related(&self, parent: &Map<String, Value>) -> Map<String, Value> {
        let _ = parent;
        Map::new()
    }

    /// Attributes the parent model needs to belong to a related model
    /// (`{"user_id": 1}` for a belongs-to).
    fn attributes_for_parent(&self, related: &Map<String, Value>) -> Map<String, Value> {
        let _ = related;
        Map::new()
    }

    /// Attach related models to a parent (many-to-many relationships).
    fn attach_models<'a>(
        &'a self,
        parent: &'a Map<String, Value>,
        related: &'a [Map<String, Value>],
    ) -> BoxFuture<'a, Result<()>> {
        let _ = (parent, related);
        Box::pin(async { Ok(()) })
    }
}

/// A `#[relation]` field's value: serialized by `to_array` when loaded.
pub trait RelationValue {
    /// Whether the relationship has been loaded.
    fn is_loaded(&self) -> bool;

    /// The serialized related models.
    fn to_value(&self) -> Value;
}

fn models_to_value<R: Model>(models: &[R]) -> Value {
    Value::Array(
        models
            .iter()
            .map(|model| Value::Object(model.to_array()))
            .collect(),
    )
}

impl<R: Model> RelationValue for Option<Vec<R>> {
    fn is_loaded(&self) -> bool {
        self.is_some()
    }

    fn to_value(&self) -> Value {
        self.as_deref().map(models_to_value).unwrap_or(Value::Null)
    }
}

impl<R: Model> RelationValue for Vec<R> {
    fn is_loaded(&self) -> bool {
        true
    }

    fn to_value(&self) -> Value {
        models_to_value(self)
    }
}

impl<R: Model> RelationValue for Option<Collection<R>> {
    fn is_loaded(&self) -> bool {
        self.is_some()
    }

    fn to_value(&self) -> Value {
        self.as_deref().map(models_to_value).unwrap_or(Value::Null)
    }
}

impl<R: Model> RelationValue for Collection<R> {
    fn is_loaded(&self) -> bool {
        true
    }

    fn to_value(&self) -> Value {
        models_to_value(self)
    }
}

/// Also covers `Option<Box<R>>`, since boxed models are models.
impl<R: Model> RelationValue for Option<R> {
    fn is_loaded(&self) -> bool {
        self.is_some()
    }

    fn to_value(&self) -> Value {
        self.as_ref()
            .map(|model| Value::Object(model.to_array()))
            .unwrap_or(Value::Null)
    }
}

/// Convert what a relationship loaded into a `#[relation]` field's type.
pub trait FromEager<L> {
    /// Convert the loaded value.
    fn from_eager(loaded: L) -> Self;
}

impl<R> FromEager<Vec<R>> for Option<Vec<R>> {
    fn from_eager(loaded: Vec<R>) -> Self {
        Some(loaded)
    }
}

impl<R> FromEager<Vec<R>> for Vec<R> {
    fn from_eager(loaded: Vec<R>) -> Self {
        loaded
    }
}

impl<R> FromEager<Vec<R>> for Option<Collection<R>> {
    fn from_eager(loaded: Vec<R>) -> Self {
        Some(loaded.into())
    }
}

impl<R> FromEager<Vec<R>> for Collection<R> {
    fn from_eager(loaded: Vec<R>) -> Self {
        loaded.into()
    }
}

impl<R> FromEager<Option<R>> for Option<R> {
    fn from_eager(loaded: Option<R>) -> Self {
        loaded
    }
}

impl<R> FromEager<Option<R>> for Option<Box<R>> {
    fn from_eager(loaded: Option<R>) -> Self {
        loaded.map(Box::new)
    }
}

/// The changes made by `sync`, `sync_without_detaching` and `toggle`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SyncChanges {
    /// The keys that were attached.
    pub attached: Vec<Value>,
    /// The keys that were detached.
    pub detached: Vec<Value>,
    /// The keys whose pivot records were updated.
    pub updated: Vec<Value>,
}

static ALIASES: AtomicUsize = AtomicUsize::new(0);

/// A unique alias for self-referencing existence queries.
pub(crate) fn next_alias() -> String {
    format!(
        "laravel_reserved_{}",
        ALIASES.fetch_add(1, Ordering::Relaxed)
    )
}

/// The distinct, non-null keys of the models.
pub(crate) fn collect_keys<M: Model>(models: &[M], column: &str) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    let mut keys = Vec::new();
    for model in models {
        let key = model.get_attribute(column);
        if let Some(string) = key_string(&key)
            && seen.insert(string)
        {
            keys.push(key);
        }
    }
    keys
}

/// Group values by key.
pub(crate) fn group_by_key<T>(items: Vec<(Value, T)>) -> HashMap<String, Vec<T>> {
    let mut groups: HashMap<String, Vec<T>> = HashMap::new();
    for (key, item) in items {
        if let Some(key) = key_string(&key) {
            groups.entry(key).or_default().push(item);
        }
    }
    groups
}

/// Pull the columns with the given prefix out of a row.
pub(crate) fn extract_prefixed(row: &mut Map<String, Value>, prefix: &str) -> Map<String, Value> {
    let keys: Vec<String> = row
        .keys()
        .filter(|key| key.starts_with(prefix))
        .cloned()
        .collect();
    let mut extracted = Map::new();
    for key in keys {
        if let Some(value) = row.remove(&key) {
            extracted.insert(key[prefix.len()..].to_string(), value);
        }
    }
    extracted
}

/// The parent's value for a key column (`Null` when missing).
pub(crate) fn attribute(attributes: &Map<String, Value>, key: &str) -> Value {
    attributes.get(key).cloned().unwrap_or(Value::Null)
}
