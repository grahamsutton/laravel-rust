//! Conditional attributes: values that may be left out of a resource's
//! JSON, and attributes merged into it.
//!
//! The conditional helpers on [`JsonResource`](crate::JsonResource)
//! (`when`, `when_loaded`, `merge_when`, ...) return these values. They
//! serialize to private markers inside the `Value` your `to_array` builds,
//! and the resource strips (or merges) them before the JSON leaves your
//! application — exactly like Laravel's `MissingValue` and `MergeValue`.

use serde::{Serialize, Serializer};

use illuminate_database::eloquent::Model as Eloquent;
use illuminate_support::{Collection, Map, Value, to_value};

/// The marker a missing value serializes to.
pub(crate) const MISSING: &str = "\u{0}illuminate:missing\u{0}";

/// The key of the marker object a merged value serializes to.
pub(crate) const MERGE: &str = "\u{0}illuminate:merge\u{0}";

/// A value that is left out of the resource's JSON entirely.
///
/// You rarely need it directly: the conditional helpers return
/// [`PotentiallyMissing`] values. But it reads nicely when an attribute
/// should simply never appear:
///
/// ```
/// use illuminate_http::Request;
/// use illuminate_http_resources::{JsonResource, MissingValue};
/// use illuminate_support::{Value, json};
///
/// struct Secret(Value);
///
/// impl JsonResource for Secret {
///     type Model = Value;
///
///     fn from_model(value: Value) -> Self {
///         Self(value)
///     }
///
///     fn model(&self) -> &Value {
///         &self.0
///     }
///
///     fn to_array(&self, _request: &Request) -> Value {
///         json!({"id": self.0["id"], "secret": MissingValue})
///     }
/// }
///
/// let resolved = Secret::make(json!({"id": 1})).resolve(&Request::default());
/// assert_eq!(resolved, json!({"id": 1}));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MissingValue;

impl MissingValue {
    /// A missing value is always missing.
    pub fn is_missing(&self) -> bool {
        true
    }
}

impl Serialize for MissingValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(MISSING)
    }
}

/// A value that may be missing from the resource's JSON.
///
/// This is what `when`, `when_loaded`, `when_counted` and friends return:
/// either the value, or a missing value whose key is removed from the
/// response. It works like an `Option`, so it is easy to transform:
///
/// ```
/// use illuminate_http_resources::PotentiallyMissing;
///
/// let count = PotentiallyMissing::Present(3).map(|count| count * 2);
/// assert_eq!(count, PotentiallyMissing::Present(6));
///
/// let missing: PotentiallyMissing<i32> = PotentiallyMissing::Missing;
/// assert_eq!(missing.unwrap_or(0), 0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PotentiallyMissing<T> {
    /// The value is present.
    Present(T),
    /// The value is missing: its key is removed from the JSON.
    Missing,
}

impl<T> PotentiallyMissing<T> {
    /// Whether the value is missing.
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    /// Whether the value is present.
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    /// Transform the value, if it is present (Laravel's callback argument).
    pub fn map<U>(self, callback: impl FnOnce(T) -> U) -> PotentiallyMissing<U> {
        match self {
            Self::Present(value) => PotentiallyMissing::Present(callback(value)),
            Self::Missing => PotentiallyMissing::Missing,
        }
    }

    /// Borrow the value.
    pub fn as_ref(&self) -> PotentiallyMissing<&T> {
        match self {
            Self::Present(value) => PotentiallyMissing::Present(value),
            Self::Missing => PotentiallyMissing::Missing,
        }
    }

    /// The value, or the given default when it is missing (Laravel's
    /// `$default` argument).
    pub fn unwrap_or(self, default: T) -> T {
        match self {
            Self::Present(value) => value,
            Self::Missing => default,
        }
    }

    /// The value, or the result of the callback when it is missing.
    pub fn unwrap_or_else(self, default: impl FnOnce() -> T) -> T {
        match self {
            Self::Present(value) => value,
            Self::Missing => default(),
        }
    }

    /// The value as an `Option`.
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Present(value) => Some(value),
            Self::Missing => None,
        }
    }
}

impl<T> From<Option<T>> for PotentiallyMissing<T> {
    /// `None` is missing.
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Present(value),
            None => Self::Missing,
        }
    }
}

impl<T> From<MissingValue> for PotentiallyMissing<T> {
    fn from(_: MissingValue) -> Self {
        Self::Missing
    }
}

impl<T: Serialize> Serialize for PotentiallyMissing<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Present(value) => value.serialize(serializer),
            Self::Missing => serializer.serialize_str(MISSING),
        }
    }
}

/// Attributes merged into the surrounding array of a resource.
///
/// Returned by `merge`, `merge_when` and `attributes`. Since a `json!`
/// object needs a key for every entry, give the merge any label you like:
/// the merged attributes take its place.
///
/// ```
/// use illuminate_http_resources::{MergeValue, filter};
/// use illuminate_support::json;
///
/// let data = json!({
///     "id": 1,
///     "admin": MergeValue::new(json!({"first-secret": "value", "second-secret": "value"})),
///     "created_at": null,
/// });
///
/// assert_eq!(
///     filter(data),
///     json!({"id": 1, "first-secret": "value", "second-secret": "value", "created_at": null})
/// );
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct MergeValue {
    /// The data to merge.
    pub data: Value,
}

impl MergeValue {
    /// Merge the given data (an object, a list, or anything serializable).
    pub fn new(data: impl Serialize) -> Self {
        Self {
            data: to_value(&data),
        }
    }
}

impl Serialize for MergeValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut marker = Map::new();
        marker.insert(MERGE.to_string(), self.data.clone());
        Value::Object(marker).serialize(serializer)
    }
}

/// Values `when_null` and `when_not_null` check: an `Option` (by value or
/// by reference) or a JSON value.
pub trait Nullable {
    /// The value when it isn't null.
    type Item;

    /// The value, or `None` when it is null.
    fn into_option(self) -> Option<Self::Item>;
}

impl<T> Nullable for Option<T> {
    type Item = T;

    fn into_option(self) -> Option<T> {
        self
    }
}

impl<'a, T> Nullable for &'a Option<T> {
    type Item = &'a T;

    fn into_option(self) -> Option<&'a T> {
        self.as_ref()
    }
}

impl Nullable for Value {
    type Item = Value;

    fn into_option(self) -> Option<Value> {
        (!self.is_null()).then_some(self)
    }
}

impl<'a> Nullable for &'a Value {
    type Item = &'a Value;

    fn into_option(self) -> Option<&'a Value> {
        (!self.is_null()).then_some(self)
    }
}

/// Relationships `when_loaded` can check.
///
/// Pass the relationship's *field* to get the loaded models themselves, or
/// its *name* to get their serialized form:
///
/// ```ignore
/// "posts": PostResource::collection(self.when_loaded(&self.0.posts)),
/// "author": self.when_loaded(&self.0.author).map(UserResource::make),
/// "comments": self.when_loaded("comments"),
/// ```
///
/// Models carry no hidden state, so a relationship counts as loaded when
/// its field holds a value: a `None` belongs-to field is "not loaded", even
/// when it was loaded and found nothing.
pub trait Loadable<'a, M> {
    /// What a loaded relationship yields.
    type Output;

    /// The loaded relationship, if it has been loaded on the model.
    fn loaded(self, model: &'a M) -> Option<Self::Output>;
}

impl<'a, M: Eloquent> Loadable<'a, M> for &str {
    type Output = Value;

    fn loaded(self, model: &'a M) -> Option<Value> {
        if !model.loaded_relations().contains(&self) {
            return None;
        }
        Some(model.to_array().get(self).cloned().unwrap_or(Value::Null))
    }
}

impl<'a, M, T: 'a> Loadable<'a, M> for &'a Option<T> {
    type Output = &'a T;

    fn loaded(self, _model: &'a M) -> Option<&'a T> {
        self.as_ref()
    }
}

impl<'a, M, T: 'a> Loadable<'a, M> for &'a Vec<T> {
    type Output = &'a Vec<T>;

    fn loaded(self, _model: &'a M) -> Option<&'a Vec<T>> {
        Some(self)
    }
}

impl<'a, M, T: 'a> Loadable<'a, M> for &'a Collection<T> {
    type Output = &'a Collection<T>;

    fn loaded(self, _model: &'a M) -> Option<&'a Collection<T>> {
        Some(self)
    }
}

/// Whether the value is the missing marker.
pub(crate) fn is_missing(value: &Value) -> bool {
    matches!(value, Value::String(marker) if marker == MISSING)
}

/// Take the data out of a merge marker, or hand the value back.
fn into_merge(value: Value) -> Result<Value, Value> {
    match value {
        Value::Object(mut map) if map.len() == 1 && map.contains_key(MERGE) => {
            Ok(map.remove(MERGE).unwrap_or(Value::Null))
        }
        other => Err(other),
    }
}

/// Filter a resource's array: remove missing values and merge merged
/// values into place, all the way down.
///
/// This is what resources do to the value your `to_array` returns, so you
/// will rarely call it yourself. Within objects, the first occurrence of a
/// key wins, as with PHP's `+` operator; within lists, merged lists are
/// spliced in place.
///
/// ```
/// use illuminate_http_resources::{MergeValue, MissingValue, filter};
/// use illuminate_support::json;
///
/// let data = json!({
///     "id": 1,
///     "secret": MissingValue,
///     "tags": ["a", MissingValue, MergeValue::new(["b", "c"]), "d"],
/// });
///
/// assert_eq!(filter(data), json!({"id": 1, "tags": ["a", "b", "c", "d"]}));
/// ```
pub fn filter(value: Value) -> Value {
    match into_merge(value) {
        Ok(data) => filter(data),
        Err(Value::Object(map)) => Value::Object(filter_object(map)),
        Err(Value::Array(items)) => Value::Array(filter_array(items)),
        Err(other) => other,
    }
}

fn filter_object(map: Map<String, Value>) -> Map<String, Value> {
    let mut filtered = Map::new();
    let put = |filtered: &mut Map<String, Value>, key: String, value: Value| {
        if !filtered.contains_key(&key) {
            filtered.insert(key, value);
        }
    };
    for (key, value) in map {
        if is_missing(&value) {
            continue;
        }
        match into_merge(value) {
            Ok(data) => match filter(data) {
                Value::Object(merged) => {
                    for (key, value) in merged {
                        put(&mut filtered, key, value);
                    }
                }
                Value::Array(items) => {
                    for (index, value) in items.into_iter().enumerate() {
                        put(&mut filtered, index.to_string(), value);
                    }
                }
                _ => {}
            },
            Err(value) => put(&mut filtered, key, filter(value)),
        }
    }
    filtered
}

fn filter_array(items: Vec<Value>) -> Vec<Value> {
    let mut filtered = Vec::with_capacity(items.len());
    for item in items {
        if is_missing(&item) {
            continue;
        }
        match into_merge(item) {
            Ok(data) => match filter(data) {
                Value::Array(items) => filtered.extend(items),
                Value::Object(map) => filtered.extend(map.into_iter().map(|(_, value)| value)),
                Value::Null => {}
                scalar => filtered.push(scalar),
            },
            Err(item) => filtered.push(filter(item)),
        }
    }
    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn missing_values_serialize_to_the_marker() {
        assert!(is_missing(&to_value(&MissingValue)));
        assert!(is_missing(&to_value(&PotentiallyMissing::<i32>::Missing)));
        assert_eq!(
            to_value(&PotentiallyMissing::Present("taylor")),
            json!("taylor")
        );
        assert!(MissingValue.is_missing());
    }

    #[test]
    fn potentially_missing_values_work_like_options() {
        let present = PotentiallyMissing::Present(2);
        let missing = PotentiallyMissing::<i32>::Missing;

        assert!(present.is_present() && !present.is_missing());
        assert!(missing.is_missing() && !missing.is_present());
        assert_eq!(present.map(|v| v + 1), PotentiallyMissing::Present(3));
        assert_eq!(missing.map(|v| v + 1), PotentiallyMissing::Missing);
        assert_eq!(present.as_ref(), PotentiallyMissing::Present(&2));
        assert_eq!(missing.unwrap_or(9), 9);
        assert_eq!(missing.unwrap_or_else(|| 7), 7);
        assert_eq!(present.unwrap_or_else(|| 7), 2);
        assert_eq!(present.into_option(), Some(2));
        assert_eq!(missing.into_option(), None);
        assert_eq!(
            PotentiallyMissing::from(Some(1)),
            PotentiallyMissing::Present(1)
        );
        assert_eq!(
            PotentiallyMissing::<u8>::from(None),
            PotentiallyMissing::Missing
        );
        assert_eq!(
            PotentiallyMissing::<u8>::from(MissingValue),
            PotentiallyMissing::Missing
        );
    }

    #[test]
    fn merge_values_serialize_to_a_marker_object() {
        let value = to_value(&MergeValue::new(json!({"a": 1})));
        assert_eq!(into_merge(value), Ok(json!({"a": 1})));
        assert_eq!(into_merge(json!({"a": 1})), Err(json!({"a": 1})));
    }

    #[test]
    fn nullable_values_convert_into_options() {
        assert_eq!(Some(1).into_option(), Some(1));
        assert_eq!((&Some(1)).into_option(), Some(&1));
        assert_eq!(json!(null).into_option(), None);
        assert_eq!(json!(1).into_option(), Some(json!(1)));
        assert_eq!((&json!(null)).into_option(), None);
        assert_eq!((&json!("a")).into_option(), Some(&json!("a")));
    }

    #[test]
    fn loadable_fields_report_whether_they_are_loaded() {
        let loaded = Some(vec![1, 2]);
        let not_loaded: Option<Vec<i32>> = None;
        let always = vec![3];
        let collection = Collection::from(vec![4]);
        assert_eq!(Loadable::<()>::loaded(&loaded, &()), Some(&vec![1, 2]));
        assert_eq!(Loadable::<()>::loaded(&not_loaded, &()), None);
        assert_eq!(Loadable::<()>::loaded(&always, &()), Some(&vec![3]));
        assert_eq!(
            Loadable::<()>::loaded(&collection, &()).map(|c| c.len()),
            Some(1)
        );
    }

    #[test]
    fn it_removes_missing_values_from_objects_and_lists() {
        let data = json!({
            "id": 1,
            "secret": MissingValue,
            "nested": {"keep": true, "drop": MissingValue},
            "list": [1, MissingValue, 2, {"deep": MissingValue}],
        });
        assert_eq!(
            filter(data),
            json!({"id": 1, "nested": {"keep": true}, "list": [1, 2, {}]})
        );
    }

    #[test]
    fn it_merges_objects_in_place() {
        let data = json!({
            "id": 1,
            "first": MergeValue::new(json!({"a": 1, "b": MissingValue})),
            "name": "Taylor",
            "second": MergeValue::new(json!({"c": 3})),
        });
        let filtered = filter(data);
        assert_eq!(filtered, json!({"id": 1, "a": 1, "name": "Taylor", "c": 3}));
        let keys: Vec<&String> = filtered.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "a", "name", "c"]);
    }

    #[test]
    fn the_first_occurrence_of_a_key_wins_like_php_plus() {
        let data = json!({
            "id": 1,
            "merged": MergeValue::new(json!({"id": 2, "name": "merged"})),
            "name": "later",
        });
        assert_eq!(filter(data), json!({"id": 1, "name": "merged"}));
    }

    #[test]
    fn it_merges_lists_into_lists() {
        let data = json!([
            1,
            MergeValue::new([2, 3]),
            4,
            MergeValue::new(json!({"x": 5}))
        ]);
        assert_eq!(filter(data), json!([1, 2, 3, 4, 5]));
    }

    #[test]
    fn merged_lists_inside_objects_are_keyed_by_index() {
        let data = json!({"a": 1, "m": MergeValue::new(["x", "y"])});
        assert_eq!(filter(data), json!({"a": 1, "0": "x", "1": "y"}));
    }

    #[test]
    fn missing_merges_and_nested_merges_are_handled() {
        let data = json!({
            "id": 1,
            "conditional": PotentiallyMissing::<MergeValue>::Missing,
            "outer": MergeValue::new(json!({
                "a": 1,
                "inner": MergeValue::new(json!({"b": 2})),
            })),
        });
        assert_eq!(filter(data), json!({"id": 1, "a": 1, "b": 2}));
    }

    #[test]
    fn top_level_markers_are_resolved() {
        assert_eq!(
            filter(to_value(&MergeValue::new(json!({"a": 1})))),
            json!({"a": 1})
        );
        assert_eq!(filter(json!(5)), json!(5));
        assert_eq!(
            filter(json!([MergeValue::new(json!(null)), 1, MergeValue::new(7)])),
            json!([1, 7])
        );
    }
}
