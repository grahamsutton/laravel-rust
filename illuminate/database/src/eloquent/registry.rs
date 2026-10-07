//! The model registry: every `#[derive(Model)]` model in the application.
//!
//! Rust has no runtime reflection, so `#[derive(Model)]` registers each
//! model (at compile time, through [`inventory`]) with the metadata
//! Laravel's console commands read from PHP classes: its table, keys,
//! mass assignment and serialization settings, fields, relationships,
//! observers, global scopes, and whether it is prunable:
//!
//! ```
//! use illuminate_database::eloquent::{Model, registry};
//!
//! #[derive(Debug, Clone, Default, Model)]
//! #[fillable(name)]
//! pub struct Flight {
//!     pub id: u64,
//!     pub name: String,
//! }
//!
//! let flight = registry::find("Flight").unwrap();
//!
//! assert_eq!(flight.table(), "flights");
//! assert_eq!(flight.fillable(), &["name"]);
//! assert_eq!(flight.columns()[1].rust_type(), "String");
//! assert!(!flight.is_prunable());
//! ```
//!
//! The foundation builds `model:show` and `model:prune` on top of it:
//!
//! ```ignore
//! // php artisan model:show User --database=sqlite
//! let info = registry::find("User").unwrap().inspect(Some("sqlite")).await?;
//! println!("{}", serde_json::to_string(&info)?); // --json
//!
//! // php artisan model:prune --pretend
//! for model in registry::models_to_prune(&models, &except)? {
//!     let count = model.pretend_to_prune().await?;
//!     println!("{count} [{}] records will be pruned.", model.class_name());
//! }
//!
//! // php artisan model:prune --chunk=1000
//! for model in registry::models_to_prune(&models, &except)? {
//!     model.prune_with_progress(1000, |total| println!("{total} records")).await?;
//! }
//! ```
//!
//! Every non-generic model is registered; a model is prunable when it
//! implements [`Prunable`] or [`MassPrunable`] (no attribute needed: the
//! derive detects the implementation).

use std::collections::BTreeMap;

use illuminate_support::Result;
use illuminate_support::error::InvalidArgumentException;
use serde::Serialize;

use super::events::{Listener, ModelEvent};
use super::model::Model;
use super::prunable::{self, MassPrunable, Progress, Prunable, PruneKind};
use super::relations::{DynRelation, RelationKind};
use super::scope::short_type_name;
use super::state::EloquentState;
use super::{BoxFuture, KeyType, UniqueIds};
use crate::DatabaseManager;

/// What a model field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FieldKind {
    /// A persisted column.
    Column,
    /// A `#[computed]` value read from queries (`posts_count`, `pivot`, ...).
    Computed,
    /// A `#[relation]` holding a loaded relationship.
    Relation,
}

/// A model field, as written in the struct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Field {
    name: &'static str,
    rust_type: &'static str,
    kind: FieldKind,
    hashed: bool,
}

impl Field {
    /// Describe a field (used by `#[derive(Model)]`).
    #[doc(hidden)]
    pub const fn new(
        name: &'static str,
        rust_type: &'static str,
        kind: FieldKind,
        hashed: bool,
    ) -> Self {
        Self {
            name,
            rust_type,
            kind,
            hashed,
        }
    }

    /// The field's (attribute's) name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The field's Rust type, as written: `Option<Carbon>`, `Vec<Post>`, ...
    pub fn rust_type(&self) -> &'static str {
        self.rust_type
    }

    /// What the field holds.
    pub fn kind(&self) -> FieldKind {
        self.kind
    }

    /// Whether the attribute is hashed when the model is saved (`#[hashed]`).
    pub fn is_hashed(&self) -> bool {
        self.hashed
    }
}

/// Prunes a registered model's records.
#[derive(Clone, Copy)]
pub struct Pruner {
    kind: PruneKind,
    prune: fn(u64, Option<Progress>) -> BoxFuture<'static, Result<u64>>,
    pretend: fn() -> BoxFuture<'static, Result<u64>>,
}

impl Pruner {
    /// The pruner of a [`Prunable`] model.
    pub fn prunable<M: Prunable>() -> Self {
        Self {
            kind: PruneKind::Prunable,
            prune: |chunk, progress| Box::pin(prunable::prune_each::<M>(chunk, progress)),
            pretend: || Box::pin(prunable::pretend::<M>(<M as Prunable>::prunable())),
        }
    }

    /// The pruner of a [`MassPrunable`] model.
    pub fn mass_prunable<M: MassPrunable>() -> Self {
        Self {
            kind: PruneKind::MassPrunable,
            prune: |chunk, progress| Box::pin(prunable::mass_prune::<M>(chunk, progress)),
            pretend: || Box::pin(prunable::pretend::<M>(<M as MassPrunable>::prunable())),
        }
    }

    /// How the model is pruned.
    pub fn kind(&self) -> PruneKind {
        self.kind
    }
}

impl std::fmt::Debug for Pruner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pruner").field("kind", &self.kind).finish()
    }
}

/// A model registered by `#[derive(Model)]`.
pub struct RegisteredModel {
    module_path: &'static str,
    fields: &'static [Field],
    observers: &'static [&'static str],
    scopes: &'static [&'static str],
    pruner: fn() -> Option<Pruner>,
    class_name: fn() -> &'static str,
    table: fn() -> String,
    connection: fn() -> Option<&'static str>,
    primary_key: fn() -> &'static str,
    key_type: fn() -> KeyType,
    incrementing: fn() -> bool,
    timestamps: fn() -> bool,
    soft_deletes: fn() -> bool,
    unique_ids: fn() -> UniqueIds,
    fillable: fn() -> &'static [&'static str],
    guarded: fn() -> &'static [&'static str],
    hidden: fn() -> &'static [&'static str],
    visible: fn() -> &'static [&'static str],
    appends: fn() -> &'static [&'static str],
    per_page: fn() -> u64,
    route_key_name: fn() -> &'static str,
    morph_class: fn() -> String,
    is_fillable: fn(&str) -> bool,
    tracks_changes: fn() -> bool,
    boot: fn(),
    relation: fn(&str) -> Option<Box<dyn DynRelation>>,
    listeners: fn() -> Vec<ObserverInfo>,
    global_scopes: fn() -> Vec<String>,
}

inventory::collect!(RegisteredModel);

impl RegisteredModel {
    /// The registration for the model `M` (used by `#[derive(Model)]`).
    #[doc(hidden)]
    pub const fn new<M: Model>(
        module_path: &'static str,
        fields: &'static [Field],
        observers: &'static [&'static str],
        scopes: &'static [&'static str],
        pruner: fn() -> Option<Pruner>,
    ) -> Self {
        Self {
            module_path,
            fields,
            observers,
            scopes,
            pruner,
            class_name: <M as Model>::class_name,
            table: <M as Model>::table,
            connection: <M as Model>::connection_name,
            primary_key: <M as Model>::primary_key,
            key_type: <M as Model>::key_type,
            incrementing: <M as Model>::incrementing,
            timestamps: <M as Model>::timestamps,
            soft_deletes: <M as Model>::soft_deletes,
            unique_ids: <M as Model>::unique_ids,
            fillable: <M as Model>::fillable,
            guarded: <M as Model>::guarded,
            hidden: <M as Model>::hidden,
            visible: <M as Model>::visible,
            appends: <M as Model>::appends,
            per_page: <M as Model>::per_page,
            route_key_name: <M as Model>::route_key_name,
            morph_class: <M as Model>::morph_class,
            is_fillable: <M as Model>::is_fillable,
            tracks_changes: tracks_changes::<M>,
            boot: boot::<M>,
            relation: <M as Model>::relation,
            listeners: observer_info::<M>,
            global_scopes: global_scopes::<M>,
        }
    }

    /// The model's class name (`User`).
    pub fn class_name(&self) -> &'static str {
        (self.class_name)()
    }

    /// The module the model is defined in (`app::models::user`).
    pub fn module_path(&self) -> &'static str {
        self.module_path
    }

    /// The model's fully qualified path (`app::models::user::User`).
    pub fn qualified_name(&self) -> String {
        format!("{}::{}", self.module_path, self.class_name())
    }

    /// The model's table (`users`).
    pub fn table(&self) -> String {
        (self.table)()
    }

    /// The model's connection (`None` for the default connection).
    pub fn connection(&self) -> Option<&'static str> {
        (self.connection)()
    }

    /// The primary key column.
    pub fn primary_key(&self) -> &'static str {
        (self.primary_key)()
    }

    /// The type of the primary key.
    pub fn key_type(&self) -> KeyType {
        (self.key_type)()
    }

    /// Whether the primary key is auto-incrementing.
    pub fn incrementing(&self) -> bool {
        (self.incrementing)()
    }

    /// Whether `created_at` / `updated_at` are maintained.
    pub fn timestamps(&self) -> bool {
        (self.timestamps)()
    }

    /// Whether the model is soft deletable.
    pub fn soft_deletes(&self) -> bool {
        (self.soft_deletes)()
    }

    /// The unique ids generated for new models' keys.
    pub fn unique_ids(&self) -> UniqueIds {
        (self.unique_ids)()
    }

    /// The mass assignable attributes.
    pub fn fillable(&self) -> &'static [&'static str] {
        (self.fillable)()
    }

    /// The guarded attributes.
    pub fn guarded(&self) -> &'static [&'static str] {
        (self.guarded)()
    }

    /// The attributes hidden from serialization.
    pub fn hidden(&self) -> &'static [&'static str] {
        (self.hidden)()
    }

    /// The attributes visible in serialization (empty means "all").
    pub fn visible(&self) -> &'static [&'static str] {
        (self.visible)()
    }

    /// The appended accessors.
    pub fn appends(&self) -> &'static [&'static str] {
        (self.appends)()
    }

    /// The number of models per page.
    pub fn per_page(&self) -> u64 {
        (self.per_page)()
    }

    /// The column used for route model binding.
    pub fn route_key_name(&self) -> &'static str {
        (self.route_key_name)()
    }

    /// The class name stored in polymorphic `*_type` columns.
    pub fn morph_class(&self) -> String {
        (self.morph_class)()
    }

    /// Whether the attribute may be mass assigned.
    pub fn is_fillable(&self, key: &str) -> bool {
        (self.is_fillable)(key)
    }

    /// Whether the model tracks its original attributes (it has an
    /// [`Original`](super::Original) field).
    pub fn tracks_changes(&self) -> bool {
        (self.tracks_changes)()
    }

    /// Every field of the model, as written in the struct.
    pub fn fields(&self) -> &'static [Field] {
        self.fields
    }

    /// The persisted columns.
    pub fn columns(&self) -> Vec<&'static Field> {
        self.fields_of(FieldKind::Column)
    }

    /// The `#[relation]` fields.
    pub fn relations(&self) -> Vec<&'static Field> {
        self.fields_of(FieldKind::Relation)
    }

    fn fields_of(&self, kind: FieldKind) -> Vec<&'static Field> {
        self.fields
            .iter()
            .filter(|field| field.kind == kind)
            .collect()
    }

    /// The observers named by `#[observed_by(...)]`.
    pub fn observers(&self) -> &'static [&'static str] {
        self.observers
    }

    /// The global scopes named by `#[scoped_by(...)]`.
    pub fn scopes(&self) -> &'static [&'static str] {
        self.scopes
    }

    /// Boot the model in the current application (registering its
    /// `#[observed_by]` observers and `#[scoped_by]` scopes) if it hasn't
    /// been booted yet.
    pub fn boot(&self) {
        (self.boot)()
    }

    /// The model's pruner, when it implements [`Prunable`] or
    /// [`MassPrunable`].
    pub fn pruner(&self) -> Option<Pruner> {
        (self.pruner)()
    }

    /// Whether the model is prunable.
    pub fn is_prunable(&self) -> bool {
        self.pruner().is_some()
    }

    /// How the model is pruned, when it is prunable.
    pub fn prune_kind(&self) -> Option<PruneKind> {
        self.pruner().map(|pruner| pruner.kind)
    }

    /// Prune the model's prunable records (Laravel's `pruneAll`), `chunk`
    /// at a time. Returns how many were pruned (`0` when the model isn't
    /// prunable).
    pub async fn prune(&self, chunk: u64) -> Result<u64> {
        match self.pruner() {
            Some(pruner) => (pruner.prune)(chunk, None).await,
            None => Ok(0),
        }
    }

    /// Prune the model's prunable records, calling `progress` with the
    /// running total after each chunk (where Laravel fires `ModelsPruned`).
    pub async fn prune_with_progress(
        &self,
        chunk: u64,
        progress: impl Fn(u64) + Send + Sync + 'static,
    ) -> Result<u64> {
        match self.pruner() {
            Some(pruner) => (pruner.prune)(chunk, Some(std::sync::Arc::new(progress))).await,
            None => Ok(0),
        }
    }

    /// How many records a prune would delete (`model:prune --pretend`):
    /// `0` when the model isn't prunable.
    pub async fn pretend_to_prune(&self) -> Result<u64> {
        match self.pruner() {
            Some(pruner) => (pruner.pretend)().await,
            None => Ok(0),
        }
    }

    /// Inspect the model against its database table (Laravel's
    /// `ModelInspector::inspect`, behind `model:show`): its columns,
    /// relationships, observers and scopes. `connection` overrides the
    /// model's connection (`--database`).
    pub async fn inspect(&self, connection: Option<&str>) -> Result<ModelInfo> {
        self.boot();
        let manager = DatabaseManager::resolve();
        let connection = match connection.or(self.connection()) {
            Some(name) => manager.connection(name),
            None => manager.default_connection(),
        };
        let table = self.table();
        let schema = connection.get_schema_builder();
        let columns = schema.get_columns(&table).await?;
        let indexes = schema.get_indexes(&table).await?;

        let hidden =
            |name: &str| !super::__private::is_visible(name, self.hidden(), self.visible());
        let field = |name: &str| self.fields.iter().find(|field| field.name == name);
        let mut attributes: Vec<AttributeInfo> = if columns.is_empty() {
            // The table doesn't exist (yet): describe the model's columns.
            self.columns()
                .into_iter()
                .map(|field| AttributeInfo {
                    name: field.name.to_string(),
                    type_: None,
                    increments: field.name == self.primary_key() && self.incrementing(),
                    nullable: None,
                    default: None,
                    unique: None,
                    fillable: self.is_fillable(field.name),
                    hidden: hidden(field.name),
                    appended: None,
                    cast: Some(field.rust_type.to_string()),
                })
                .collect()
        } else {
            columns
                .iter()
                .map(|column| AttributeInfo {
                    name: column.name.clone(),
                    type_: Some(column.type_.clone()),
                    increments: column.auto_increment,
                    nullable: Some(column.nullable),
                    default: column.default.clone(),
                    unique: Some(indexes.iter().any(|index| {
                        index.unique && index.columns.len() == 1 && index.columns[0] == column.name
                    })),
                    fillable: self.is_fillable(&column.name),
                    hidden: hidden(&column.name),
                    appended: None,
                    cast: field(&column.name)
                        .filter(|field| field.kind == FieldKind::Column)
                        .map(|field| field.rust_type.to_string()),
                })
                .collect()
        };
        for appended in self.appends() {
            if attributes
                .iter()
                .any(|attribute| attribute.name == *appended)
            {
                continue;
            }
            attributes.push(AttributeInfo {
                name: appended.to_string(),
                type_: None,
                increments: false,
                nullable: None,
                default: None,
                unique: None,
                fillable: self.is_fillable(appended),
                hidden: hidden(appended),
                appended: Some(true),
                cast: Some("accessor".to_string()),
            });
        }

        let relations = self
            .relations()
            .into_iter()
            .map(|field| {
                let relation = (self.relation)(field.name);
                RelationInfo {
                    name: field.name.to_string(),
                    type_: relation
                        .as_ref()
                        .map(|relation| relation_type(relation.kind()).to_string())
                        .unwrap_or_default(),
                    related: relation
                        .as_ref()
                        .map(|relation| relation.related_class().to_string())
                        .unwrap_or_default(),
                }
            })
            .collect();

        Ok(ModelInfo {
            class: self.class_name().to_string(),
            database: connection.get_name().to_string(),
            table: format!("{}{table}", connection.get_table_prefix()),
            policy: None,
            attributes,
            relations,
            observers: (self.listeners)(),
            scopes: (self.global_scopes)(),
            prunable: self.prune_kind(),
        })
    }
}

impl std::fmt::Debug for RegisteredModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredModel")
            .field("class", &self.class_name())
            .field("module_path", &self.module_path)
            .field("table", &self.table())
            .field("prunable", &self.prune_kind())
            .finish()
    }
}

/// What `model:show` displays about a model (Laravel's `ModelInfo`).
/// Serializes to the same JSON shape as `model:show --json`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelInfo {
    /// The model's class name.
    pub class: String,
    /// The connection the model was inspected on.
    pub database: String,
    /// The model's table, with the connection's prefix.
    pub table: String,
    /// The model's policy: Eloquent doesn't know about policies, so the
    /// foundation fills this in from the `Gate`.
    pub policy: Option<String>,
    /// The table's columns, then the appended accessors.
    pub attributes: Vec<AttributeInfo>,
    /// The `#[relation]` relationships.
    pub relations: Vec<RelationInfo>,
    /// The model's listeners and observers, by event.
    pub observers: Vec<ObserverInfo>,
    /// The names of the model's global scopes.
    pub scopes: Vec<String>,
    /// How the model is pruned, when it is prunable.
    pub prunable: Option<PruneKind>,
}

/// A model attribute: a column, or an appended accessor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AttributeInfo {
    /// The attribute's name.
    pub name: String,
    /// The column type (`varchar`, `integer`, ...); `None` for accessors.
    #[serde(rename = "type")]
    pub type_: Option<String>,
    /// Whether the column auto-increments.
    pub increments: bool,
    /// Whether the column is nullable.
    pub nullable: Option<bool>,
    /// The column's default value.
    pub default: Option<String>,
    /// Whether the column has a unique index of its own.
    pub unique: Option<bool>,
    /// Whether the attribute is mass assignable.
    pub fillable: bool,
    /// Whether the attribute is hidden from serialization.
    pub hidden: bool,
    /// Whether the attribute is an appended accessor.
    pub appended: Option<bool>,
    /// How the attribute is cast: the field's Rust type (`Option<Carbon>`),
    /// or `accessor` for appended accessors. `None` for columns the model
    /// has no field for.
    pub cast: Option<String>,
}

/// A model relationship.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RelationInfo {
    /// The relationship's name (its method and field).
    pub name: String,
    /// The kind of relationship (`HasMany`, `BelongsTo`, ...).
    #[serde(rename = "type")]
    pub type_: String,
    /// The related model's class name.
    pub related: String,
}

/// The listeners of one model event.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ObserverInfo {
    /// The event (`created`, `updating`, ...), or `*` for observers, which
    /// may handle any event.
    pub event: String,
    /// The listeners: observer type names, or `Closure`.
    pub observer: Vec<String>,
}

fn relation_type(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::HasOne => "HasOne",
        RelationKind::HasMany => "HasMany",
        RelationKind::BelongsTo => "BelongsTo",
        RelationKind::BelongsToMany => "BelongsToMany",
        RelationKind::HasOneThrough => "HasOneThrough",
        RelationKind::HasManyThrough => "HasManyThrough",
    }
}

fn tracks_changes<M: Model>() -> bool {
    M::template().original_state().is_some()
}

fn boot<M: Model>() {
    EloquentState::resolve().boot::<M>();
}

fn global_scopes<M: Model>() -> Vec<String> {
    let state = EloquentState::resolve();
    state.boot::<M>();
    state
        .global_scopes::<M>()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn observer_info<M: Model>() -> Vec<ObserverInfo> {
    let state = EloquentState::resolve();
    state.boot::<M>();
    let mut closures: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut observers = Vec::new();
    for listener in state.listeners::<M>() {
        match listener {
            Listener::Closure(event, _) => {
                let order = ModelEvent::all()
                    .iter()
                    .position(|e| *e == event)
                    .unwrap_or(usize::MAX);
                closures
                    .entry(order)
                    .or_default()
                    .push("Closure".to_string());
            }
            Listener::Observer(name, _) => observers.push(short_type_name(name)),
        }
    }
    let mut info: Vec<ObserverInfo> = closures
        .into_iter()
        .map(|(order, observer)| ObserverInfo {
            event: ModelEvent::all()[order].name().to_string(),
            observer,
        })
        .collect();
    if !observers.is_empty() {
        info.push(ObserverInfo {
            event: "*".to_string(),
            observer: observers,
        });
    }
    info
}

/// Every registered model, sorted by class name.
///
/// ```ignore
/// for model in registry::models() {
///     println!("{} => {}", model.class_name(), model.table());
/// }
/// ```
pub fn models() -> Vec<&'static RegisteredModel> {
    let mut models: Vec<&'static RegisteredModel> =
        inventory::iter::<RegisteredModel>.into_iter().collect();
    models.sort_by(|a, b| {
        a.class_name()
            .cmp(b.class_name())
            .then_with(|| a.module_path.cmp(b.module_path))
    });
    models
}

/// Find a registered model by name: its class name (`User`, matched
/// case-insensitively when there's no exact match), a Laravel-style
/// class (`App\Models\User`), or a path suffix (`models::user::User`).
pub fn find(name: &str) -> Option<&'static RegisteredModel> {
    let name = name
        .trim()
        .trim_start_matches(['\\', ':'])
        .replace('\\', "::");
    let models = models();
    let suffix = format!("::{name}");
    if name.contains("::")
        && let Some(model) = models.iter().find(|model| {
            model.qualified_name() == name || model.qualified_name().ends_with(&suffix)
        })
    {
        return Some(*model);
    }
    let class = name.rsplit("::").next().unwrap_or(&name);
    models
        .iter()
        .find(|model| model.class_name() == class)
        .or_else(|| {
            models
                .iter()
                .find(|model| model.class_name().eq_ignore_ascii_case(class))
        })
        .copied()
}

/// Every prunable model, sorted by class name.
pub fn prunable() -> Vec<&'static RegisteredModel> {
    models()
        .into_iter()
        .filter(|model| model.is_prunable())
        .collect()
}

/// The models `model:prune` should prune (Laravel's `PruneCommand::models`):
/// the named `models` when given (unknown names are skipped), otherwise
/// every prunable model except the `except` ones. Naming both fails with an
/// `InvalidArgumentException`.
///
/// ```ignore
/// // php artisan model:prune --model=Flight --chunk=500
/// for model in registry::models_to_prune(&["Flight"], &[])? {
///     model.prune(500).await?;
/// }
/// ```
pub fn models_to_prune<S: AsRef<str>>(
    models: &[S],
    except: &[S],
) -> Result<Vec<&'static RegisteredModel>> {
    if !models.is_empty() && !except.is_empty() {
        return Err(InvalidArgumentException::new(
            "The --model and --except options cannot be combined.",
        )
        .into());
    }
    if !models.is_empty() {
        let mut found: Vec<&'static RegisteredModel> = Vec::new();
        for name in models {
            if let Some(model) = find(name.as_ref())
                && !found.iter().any(|existing| std::ptr::eq(*existing, model))
            {
                found.push(model);
            }
        }
        return Ok(found);
    }
    let except: Vec<&'static RegisteredModel> = except
        .iter()
        .filter_map(|name| find(name.as_ref()))
        .collect();
    Ok(prunable()
        .into_iter()
        .filter(|model| {
            !except
                .iter()
                .any(|excluded| std::ptr::eq(*excluded, *model))
        })
        .collect())
}
