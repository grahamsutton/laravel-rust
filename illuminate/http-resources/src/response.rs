//! Turning resolved resources into response bodies: Laravel's
//! `ResourceResponse::wrap` and `PaginatedResourceResponse`, including the
//! PHP array semantics they depend on (`array_merge_recursive`).

use indexmap::IndexMap;

use illuminate_support::{Map, Value};

/// A PHP array key: integer-like strings are integers, as in PHP.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Int(i64),
    Str(String),
}

impl Key {
    fn parse(key: String) -> Self {
        match php_int_key(&key) {
            Some(int) => Self::Int(int),
            None => Self::Str(key),
        }
    }

    fn into_string(self) -> String {
        match self {
            Self::Int(int) => int.to_string(),
            Self::Str(key) => key,
        }
    }
}

/// An ordered PHP array.
#[derive(Debug, Default)]
struct PhpArray {
    entries: IndexMap<Key, Value>,
    next: i64,
}

impl PhpArray {
    /// Convert an object or list into a PHP array; scalars are handed back.
    fn from_value(value: Value) -> Result<Self, Value> {
        let mut array = Self::default();
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    array.insert(Key::parse(key), value);
                }
            }
            Value::Array(items) => {
                for value in items {
                    array.push(value);
                }
            }
            other => return Err(other),
        }
        Ok(array)
    }

    /// PHP's `(array)` cast: `null` becomes `[]`, scalars `[scalar]`.
    fn cast(value: Value) -> Self {
        match Self::from_value(value) {
            Ok(array) => array,
            Err(Value::Null) => Self::default(),
            Err(scalar) => {
                let mut array = Self::default();
                array.push(scalar);
                array
            }
        }
    }

    fn insert(&mut self, key: Key, value: Value) {
        if let Key::Int(int) = key {
            self.next = self.next.max(int.saturating_add(1));
        }
        self.entries.insert(key, value);
    }

    fn push(&mut self, value: Value) {
        let index = self.next;
        self.insert(Key::Int(index), value);
    }

    /// Re-key integer keys from zero, keeping string keys (what PHP does to
    /// the first array given to `array_merge_recursive`).
    fn renumbered(self) -> Self {
        let mut array = Self::default();
        for (key, value) in self.entries {
            match key {
                Key::Int(_) => array.push(value),
                key => array.insert(key, value),
            }
        }
        array
    }

    /// Convert back into JSON: a list when the keys are `0..n` in order
    /// (as `json_encode` decides), an object otherwise.
    fn into_value(self) -> Value {
        let is_list = self
            .entries
            .keys()
            .enumerate()
            .all(|(index, key)| *key == Key::Int(index as i64));
        if is_list {
            Value::Array(self.entries.into_values().collect())
        } else {
            Value::Object(
                self.entries
                    .into_iter()
                    .map(|(key, value)| (key.into_string(), value))
                    .collect(),
            )
        }
    }
}

/// Parse a canonical PHP integer key (`"0"`, `"42"`, `"-7"`, but not
/// `"007"` or `"1.5"`).
pub(crate) fn php_int_key(key: &str) -> Option<i64> {
    let digits = key.strip_prefix('-').unwrap_or(key);
    let canonical = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && key != "-0";
    if canonical { key.parse().ok() } else { None }
}

/// PHP's `is_numeric` for strings: optional surrounding whitespace, a sign,
/// digits with an optional fraction, and an optional exponent.
pub(crate) fn is_numeric(value: &str) -> bool {
    let value = value.trim_start_matches([' ', '\t', '\n', '\r', '\u{b}', '\u{c}']);
    let value = value.trim_end_matches([' ', '\t', '\n', '\r', '\u{b}', '\u{c}']);
    let bytes = value.as_bytes();
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let integer = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
    i += integer;
    let mut fraction = 0;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        fraction = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        i += fraction;
    }
    if integer + fraction == 0 {
        return false;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(bytes.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exponent = bytes[j..].iter().take_while(|b| b.is_ascii_digit()).count();
        if exponent == 0 {
            return false;
        }
        i = j + exponent;
    }
    i == bytes.len()
}

/// PHP's `empty()` for the arrays resources pass around.
pub(crate) fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Bool(flag) => !flag,
        Value::String(string) => string.is_empty() || string == "0",
        Value::Number(number) => number.as_f64() == Some(0.0),
    }
}

/// Merge `source` into `destination` the way `php_array_merge_recursive`
/// does: string keys present in both are merged recursively (scalars are
/// collected into a list), integer keys are appended.
fn merge_into(destination: &mut PhpArray, source: PhpArray) {
    for (key, value) in source.entries {
        match key {
            Key::Str(name) => {
                let key = Key::Str(name);
                match destination.entries.get_mut(&key) {
                    Some(existing) => {
                        let mut merged = match std::mem::take(existing) {
                            Value::Null => {
                                let mut array = PhpArray::default();
                                array.push(Value::Null);
                                array
                            }
                            other => PhpArray::cast(other),
                        };
                        match PhpArray::from_value(value) {
                            Ok(array) => merge_into(&mut merged, array),
                            Err(scalar) => merged.push(scalar),
                        }
                        *existing = merged.into_value();
                    }
                    None => destination.insert(key, value),
                }
            }
            Key::Int(_) => destination.push(value),
        }
    }
}

/// PHP's `array_merge_recursive`, over JSON values.
///
/// ```ignore
/// array_merge_recursive([json!({"links": {"self": "a"}}), json!({"links": {"next": "b"}})])
///     == json!({"links": {"self": "a", "next": "b"}})
/// ```
pub(crate) fn array_merge_recursive(arrays: impl IntoIterator<Item = Value>) -> Value {
    let mut arrays = arrays.into_iter();
    let first = arrays.next().unwrap_or(Value::Null);
    let rest: Vec<Value> = arrays.filter(|value| !is_empty(value)).collect();
    let mut destination = match PhpArray::from_value(first) {
        Ok(array) => array.renumbered(),
        Err(scalar) if rest.is_empty() => return scalar,
        Err(scalar) => PhpArray::cast(scalar),
    };
    for source in rest {
        merge_into(&mut destination, PhpArray::cast(source));
    }
    destination.into_value()
}

/// Whether the data (a PHP array) has the given key.
fn has_key(data: &Value, key: &str) -> bool {
    match data {
        Value::Object(map) => map.contains_key(key),
        Value::Array(items) => php_int_key(key)
            .and_then(|index| usize::try_from(index).ok())
            .is_some_and(|index| index < items.len()),
        _ => false,
    }
}

/// Laravel's `ResourceResponse::wrap`: wrap the data in the wrapper (unless
/// it already contains the wrapper key), wrap it in `data` when there is
/// top-level information to add, then merge in the `with` and `additional`
/// data.
pub(crate) fn wrap(
    data: Value,
    with: Value,
    additional: Value,
    wrapper: Option<&str>,
    force_wrapping: bool,
) -> Value {
    // PHP treats an empty wrapper as falsy, but `null !== ''` when forcing.
    let truthy_wrapper = wrapper.filter(|wrapper| !wrapper.is_empty() && *wrapper != "0");

    let default_wrapper_and_unwrapped = if force_wrapping {
        wrapper.is_some()
    } else {
        truthy_wrapper.is_some_and(|wrapper| !has_key(&data, wrapper))
    };

    let data = if default_wrapper_and_unwrapped {
        wrapped(wrapper.unwrap_or_default(), data)
    } else if (!is_empty(&with) || !is_empty(&additional))
        && truthy_wrapper.is_none_or(|wrapper| !has_key(&data, wrapper))
    {
        wrapped(wrapper.unwrap_or("data"), data)
    } else {
        data
    };

    array_merge_recursive([data, with, additional])
}

fn wrapped(key: &str, data: Value) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), data);
    Value::Object(map)
}

/// The default pagination information: the `links` and `meta` keys of a
/// paginated resource response.
pub(crate) fn pagination_information(paginated: &Value) -> Value {
    let get = |key: &str| paginated.get(key).cloned().unwrap_or(Value::Null);
    let mut links = Map::new();
    links.insert("first".into(), get("first_page_url"));
    links.insert("last".into(), get("last_page_url"));
    links.insert("prev".into(), get("prev_page_url"));
    links.insert("next".into(), get("next_page_url"));

    let excluded = [
        "data",
        "first_page_url",
        "last_page_url",
        "prev_page_url",
        "next_page_url",
    ];
    let meta: Map<String, Value> = paginated
        .as_object()
        .map(|map| {
            map.iter()
                .filter(|(key, _)| !excluded.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();

    let mut information = Map::new();
    information.insert("links".into(), Value::Object(links));
    information.insert("meta".into(), Value::Object(meta));
    Value::Object(information)
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_parses_php_integer_keys() {
        assert_eq!(php_int_key("0"), Some(0));
        assert_eq!(php_int_key("42"), Some(42));
        assert_eq!(php_int_key("-7"), Some(-7));
        assert_eq!(php_int_key("007"), None);
        assert_eq!(php_int_key("-0"), None);
        assert_eq!(php_int_key("1.5"), None);
        assert_eq!(php_int_key(""), None);
        assert_eq!(php_int_key("-"), None);
        assert_eq!(php_int_key("data"), None);
        assert_eq!(php_int_key("99999999999999999999"), None);
    }

    #[test]
    fn it_detects_numeric_strings_like_php() {
        for numeric in [
            "1", "-1", "+1.5", ".5", "5.", "1e3", "1E-3", " 12", "12 ", "007",
        ] {
            assert!(is_numeric(numeric), "{numeric} should be numeric");
        }
        for not_numeric in [
            "", "abc", "1a", ".", "1e", "e3", "--1", "inf", "NaN", "0x1A",
        ] {
            assert!(
                !is_numeric(not_numeric),
                "{not_numeric} should not be numeric"
            );
        }
    }

    #[test]
    fn it_knows_what_php_considers_empty() {
        assert!(is_empty(&json!(null)));
        assert!(is_empty(&json!({})));
        assert!(is_empty(&json!([])));
        assert!(is_empty(&json!("")));
        assert!(is_empty(&json!("0")));
        assert!(is_empty(&json!(0)));
        assert!(is_empty(&json!(false)));
        assert!(!is_empty(&json!({"a": 1})));
        assert!(!is_empty(&json!([1])));
        assert!(!is_empty(&json!("a")));
    }

    #[test]
    fn array_merge_recursive_merges_nested_string_keys() {
        let merged = array_merge_recursive([
            json!({"data": [1], "links": {"self": "a"}}),
            json!({"links": {"next": "b"}, "meta": {"total": 1}}),
        ]);
        assert_eq!(
            merged,
            json!({"data": [1], "links": {"self": "a", "next": "b"}, "meta": {"total": 1}})
        );
    }

    #[test]
    fn array_merge_recursive_collects_colliding_scalars() {
        assert_eq!(
            array_merge_recursive([json!({"a": 1}), json!({"a": 2}), json!({"a": 3})]),
            json!({"a": [1, 2, 3]})
        );
        assert_eq!(
            array_merge_recursive([json!({"a": null}), json!({"a": 2})]),
            json!({"a": [null, 2]})
        );
        assert_eq!(
            array_merge_recursive([json!({"a": [1]}), json!({"a": 2})]),
            json!({"a": [1, 2]})
        );
        assert_eq!(
            array_merge_recursive([json!({"a": 1}), json!({"a": {"b": 2}})]),
            json!({"a": {"0": 1, "b": 2}})
        );
    }

    #[test]
    fn array_merge_recursive_appends_and_renumbers_integer_keys() {
        assert_eq!(
            array_merge_recursive([json!([1, 2]), json!([3])]),
            json!([1, 2, 3])
        );
        assert_eq!(
            array_merge_recursive([json!({"5": "a", "9": "b"})]),
            json!(["a", "b"])
        );
        assert_eq!(
            array_merge_recursive([json!({"x": {"5": "a"}}), json!({"x": {"5": "b"}})]),
            json!({"x": {"5": "a", "6": "b"}})
        );
        assert_eq!(
            array_merge_recursive([json!({"name": "a", "3": "b"}), json!(["c"])]),
            json!({"name": "a", "0": "b", "1": "c"})
        );
    }

    #[test]
    fn array_merge_recursive_passes_lone_scalars_through() {
        assert_eq!(array_merge_recursive([json!(5)]), json!(5));
        assert_eq!(array_merge_recursive([json!(5), json!({})]), json!(5));
        assert_eq!(
            array_merge_recursive([json!(5), json!({"a": 1})]),
            json!({"0": 5, "a": 1})
        );
        assert_eq!(
            array_merge_recursive([json!(null), json!(["a"])]),
            json!(["a"])
        );
    }

    #[test]
    fn it_wraps_unwrapped_data() {
        assert_eq!(
            wrap(json!({"id": 1}), json!({}), json!({}), Some("data"), false),
            json!({"data": {"id": 1}})
        );
        assert_eq!(
            wrap(
                json!([{"id": 1}]),
                json!(null),
                json!(null),
                Some("users"),
                false
            ),
            json!({"users": [{"id": 1}]})
        );
    }

    #[test]
    fn it_never_double_wraps() {
        let data = json!({"data": [1, 2], "links": {"self": "link-value"}});
        assert_eq!(
            wrap(data.clone(), json!({}), json!({}), Some("data"), false),
            data
        );
    }

    #[test]
    fn forced_wrapping_wraps_even_wrapped_data() {
        assert_eq!(
            wrap(json!({"data": 1}), json!({}), json!({}), Some("data"), true),
            json!({"data": {"data": 1}})
        );
        assert_eq!(
            wrap(json!({"data": 1}), json!({}), json!({}), None, true),
            json!({"data": 1})
        );
    }

    #[test]
    fn without_a_wrapper_data_is_left_alone() {
        assert_eq!(
            wrap(json!({"id": 1}), json!({}), json!({}), None, false),
            json!({"id": 1})
        );
        assert_eq!(
            wrap(json!([1, 2]), json!(null), json!([]), None, false),
            json!([1, 2])
        );
    }

    #[test]
    fn top_level_information_wraps_unwrapped_data_in_data() {
        assert_eq!(
            wrap(
                json!({"id": 1}),
                json!({"meta": {"a": 1}}),
                json!({}),
                None,
                false
            ),
            json!({"data": {"id": 1}, "meta": {"a": 1}})
        );
        assert_eq!(
            wrap(
                json!({"id": 1}),
                json!({}),
                json!({"version": 2}),
                None,
                false
            ),
            json!({"data": {"id": 1}, "version": 2})
        );
        // `'' ?? 'data'` is `''` in PHP.
        assert_eq!(
            wrap(
                json!({"id": 1}),
                json!({}),
                json!({"version": 2}),
                Some(""),
                false
            ),
            json!({"": {"id": 1}, "version": 2})
        );
    }

    #[test]
    fn with_and_additional_are_merged_recursively_in_order() {
        let body = wrap(
            json!({"id": 1}),
            json!({"meta": {"a": 1}, "version": 1}),
            json!({"meta": {"b": 2}, "version": 2}),
            Some("data"),
            false,
        );
        assert_eq!(
            body,
            json!({"data": {"id": 1}, "meta": {"a": 1, "b": 2}, "version": [1, 2]})
        );
    }

    #[test]
    fn data_with_the_wrapper_key_is_merged_with_top_level_information() {
        let body = wrap(
            json!({"data": [1], "links": {"self": "x"}}),
            json!({"links": {"first": "y"}}),
            json!({}),
            Some("data"),
            false,
        );
        assert_eq!(
            body,
            json!({"data": [1], "links": {"self": "x", "first": "y"}})
        );
    }

    #[test]
    fn it_builds_laravels_pagination_information() {
        let paginated = json!({
            "current_page": 1,
            "first_page_url": "/?page=1",
            "from": 1,
            "last_page": 2,
            "last_page_url": "/?page=2",
            "links": [],
            "next_page_url": "/?page=2",
            "path": "/",
            "per_page": 1,
            "prev_page_url": null,
            "to": 1,
            "total": 2,
        });
        assert_eq!(
            pagination_information(&paginated),
            json!({
                "links": {"first": "/?page=1", "last": "/?page=2", "prev": null, "next": "/?page=2"},
                "meta": {
                    "current_page": 1, "from": 1, "last_page": 2, "links": [],
                    "path": "/", "per_page": 1, "to": 1, "total": 2,
                },
            })
        );
    }
}
