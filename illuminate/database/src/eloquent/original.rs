//! Dirty tracking: the attributes a model had when it was retrieved or last
//! saved, so Eloquent knows what changed.

use std::fmt;
use std::hash::{Hash, Hasher};

use illuminate_support::{Map, Value};

use super::model::Model;

/// The original attributes of a model: what it looked like when it was
/// retrieved or last saved.
///
/// Models are plain structs, so Eloquent can only remember their original
/// state when you give it somewhere to keep it. Add an `Original` field to
/// a model and Eloquent starts tracking changes: `is_dirty`, `get_dirty`,
/// `was_changed`, `get_original`, ... report what changed, and `save()`
/// only writes the dirty columns (skipping the `UPDATE` entirely when
/// nothing changed).
///
/// ```
/// use illuminate_database::eloquent::*;
/// # use std::sync::Arc;
/// # use illuminate_container::Container;
/// # use illuminate_database::{DatabaseManager, Schema};
///
/// #[derive(Debug, Clone, Default, Model)]
/// #[fillable(title, body)]
/// pub struct Post {
///     pub id: u64,
///     pub title: String,
///     pub body: String,
///     pub original: Original,
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> illuminate_support::Result<()> {
/// # let container = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(container.clone());
/// # container.instance(DatabaseManager::from_config(json!({
/// #     "default": "sqlite",
/// #     "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
/// # })));
/// # Schema::create("posts", |table| {
/// #     table.id();
/// #     table.string("title");
/// #     table.text("body");
/// # }).await?;
/// # Post::create(json!({"title": "Hello", "body": "World"})).await?;
/// let mut post = Post::find_or_fail(1).await?;
/// assert!(post.is_clean());
///
/// post.title = "Laravel, in Rust".into();
/// assert!(post.is_dirty_any("title"));
/// assert!(post.is_clean_all("body"));
/// assert_eq!(post.get_original_attribute("title"), json!("Hello"));
///
/// post.save().await?; // update "posts" set "title" = ? where "id" = ?
/// assert!(post.was_changed_any("title"));
/// assert_eq!(post.get_previous()["title"], json!("Hello"));
/// # Ok(())
/// # }
/// ```
///
/// The field is never a column, never serialized, and never makes two
/// models unequal: it is bookkeeping, not data. (If your model also derives
/// `serde::Deserialize`, mark the field `#[serde(skip)]`.)
#[derive(Clone, Default)]
pub struct Original {
    /// The attributes as of the last sync (`None` until the model is
    /// retrieved or saved).
    pub(crate) attributes: Option<Map<String, Value>>,
    /// The attributes changed by the last save.
    pub(crate) changes: Map<String, Value>,
    /// The values the changed attributes had before the last save.
    pub(crate) previous: Map<String, Value>,
    /// Whether the model exists in the database.
    pub(crate) exists: bool,
    /// Whether the model was inserted during the current request.
    pub(crate) recently_created: bool,
    /// The instance's hidden attributes (`make_hidden`), when changed.
    pub(crate) hidden: Option<Vec<String>>,
    /// The instance's visible attributes (`make_visible`), when changed.
    pub(crate) visible: Option<Vec<String>>,
    /// The columns that weren't retrieved (a partial select).
    pub(crate) missing: Vec<String>,
}

impl Original {
    /// A fresh tracker, for a model that hasn't been saved yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the tracked model exists in the database.
    pub fn exists(&self) -> bool {
        self.exists
    }

    /// Whether the original attributes have been recorded (the model was
    /// retrieved or saved).
    pub fn is_synced(&self) -> bool {
        self.attributes.is_some()
    }
}

impl fmt::Debug for Original {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Original")
            .field("exists", &self.exists)
            .field("synced", &self.is_synced())
            .field("changes", &self.changes.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Tracking state never makes two models unequal.
impl PartialEq for Original {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Original {}

/// Tracking state never changes a model's hash.
impl Hash for Original {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

/// One or more attribute names (`is_dirty_any`, `was_changed_any`,
/// `sync_original_attributes`, ...): a `&str`, a `String`, an array, a
/// slice or a `Vec` of them.
pub trait IntoAttributeNames {
    /// Convert into attribute names.
    fn into_attribute_names(self) -> Vec<String>;
}

impl IntoAttributeNames for &str {
    fn into_attribute_names(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl IntoAttributeNames for String {
    fn into_attribute_names(self) -> Vec<String> {
        vec![self]
    }
}

impl IntoAttributeNames for &String {
    fn into_attribute_names(self) -> Vec<String> {
        vec![self.clone()]
    }
}

impl<T: Into<String>> IntoAttributeNames for Vec<T> {
    fn into_attribute_names(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<String>, const N: usize> IntoAttributeNames for [T; N] {
    fn into_attribute_names(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<T: Into<String> + Clone> IntoAttributeNames for &[T] {
    fn into_attribute_names(self) -> Vec<String> {
        self.iter().cloned().map(Into::into).collect()
    }
}

/// Whether an attribute's current value is equivalent to its original one
/// (Laravel's `originalIsEquivalent`). Both sides come from the model's
/// storage format, so they share a representation; numbers compare by value
/// so `1` and `1.0` are the same.
pub(crate) fn is_equivalent(original: Option<&Value>, current: &Value) -> bool {
    let Some(original) = original else {
        return false;
    };
    if original == current {
        return true;
    }
    match (original, current) {
        (_, Value::Null) | (Value::Null, _) => false,
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => {
                (a - b).abs() < f64::EPSILON * 4.0 * a.abs().max(b.abs()).max(1.0)
            }
            _ => false,
        },
        _ => false,
    }
}

/// Whether a set of changes touches any of the given attributes (every
/// attribute when none are given).
pub(crate) fn has_changes(changes: &Map<String, Value>, attributes: &[String]) -> bool {
    if attributes.is_empty() {
        return !changes.is_empty();
    }
    attributes
        .iter()
        .any(|attribute| changes.contains_key(attribute))
}

/// Mark a freshly hydrated model as existing, recording its attributes as
/// the originals.
pub(crate) fn hydrated<M: Model>(model: &mut M) {
    if model.original_state().is_none() {
        return;
    }
    let attributes = model.to_attributes();
    if let Some(original) = model.original_state_mut() {
        original.attributes = Some(attributes);
        original.exists = true;
        original.recently_created = false;
    }
}

/// Mark a model as inserted.
pub(crate) fn inserted<M: Model>(model: &mut M) {
    if let Some(original) = model.original_state_mut() {
        original.exists = true;
        original.recently_created = true;
    }
}

/// Mark a model as permanently deleted.
pub(crate) fn deleted<M: Model>(model: &mut M) {
    if let Some(original) = model.original_state_mut() {
        original.exists = false;
    }
}

/// The key identifying the model's row: the original key, so a model whose
/// key changed still updates the right row.
pub(crate) fn key_for_save_query<M: Model>(model: &M) -> Value {
    model
        .original_state()
        .and_then(|original| original.attributes.as_ref())
        .and_then(|attributes| attributes.get(M::primary_key()))
        .filter(|key| super::model::key_is_set(key))
        .cloned()
        .unwrap_or_else(|| model.get_key())
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn equivalence_follows_laravel() {
        assert!(is_equivalent(Some(&json!("Taylor")), &json!("Taylor")));
        assert!(!is_equivalent(None, &json!("Taylor")));
        assert!(!is_equivalent(Some(&json!("Taylor")), &json!("Abigail")));
        assert!(!is_equivalent(Some(&json!(1)), &Value::Null));
        assert!(!is_equivalent(Some(&Value::Null), &json!(1)));
        assert!(is_equivalent(Some(&Value::Null), &Value::Null));
        assert!(is_equivalent(Some(&json!(1)), &json!(1.0)));
        assert!(is_equivalent(Some(&json!(0.1 + 0.2)), &json!(0.3)));
        assert!(!is_equivalent(Some(&json!(1)), &json!(2)));
        assert!(!is_equivalent(Some(&json!("1")), &json!(1)));
        assert!(is_equivalent(
            Some(&json!({"a": 1, "b": 2})),
            &json!({"b": 2, "a": 1})
        ));
    }

    #[test]
    fn changes_can_be_filtered_by_attribute() {
        let changes: Map<String, Value> = json!({"title": "New"}).as_object().unwrap().clone();
        assert!(has_changes(&changes, &[]));
        assert!(has_changes(&changes, &["title".into()]));
        assert!(has_changes(&changes, &["body".into(), "title".into()]));
        assert!(!has_changes(&changes, &["body".into()]));
        assert!(!has_changes(&Map::new(), &[]));
    }

    #[test]
    fn attribute_names_come_in_many_shapes() {
        assert_eq!("title".into_attribute_names(), ["title"]);
        assert_eq!(String::from("title").into_attribute_names(), ["title"]);
        assert_eq!(["title", "body"].into_attribute_names(), ["title", "body"]);
        assert_eq!(vec!["title"].into_attribute_names(), ["title"]);
        assert_eq!((&["title"][..]).into_attribute_names(), ["title"]);
    }

    #[test]
    fn trackers_are_invisible_to_equality() {
        let mut synced = Original::new();
        synced.attributes = Some(Map::new());
        synced.exists = true;
        assert_eq!(synced, Original::new());
        assert!(synced.exists() && synced.is_synced());
        assert!(!Original::new().exists());
        assert_eq!(
            format!("{:?}", Original::new()),
            "Original { exists: false, synced: false, changes: [] }"
        );
    }
}
