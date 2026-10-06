//! Parsing request input — query strings, form bodies, and JSON — into
//! dynamic values, understanding PHP's bracket syntax (`user[name]=Taylor`,
//! `tags[]=php&tags[]=rust`).

use illuminate_support::{Map, Value};

/// Parse a URL-encoded string (query string or form body) into a value.
///
/// ```
/// use illuminate_http::input::parse_query;
/// use illuminate_support::json;
///
/// let parsed = parse_query("name=Taylor&tags[]=php&tags[]=rust&address[city]=Little+Rock");
/// assert_eq!(parsed, json!({
///     "name": "Taylor",
///     "tags": ["php", "rust"],
///     "address": {"city": "Little Rock"},
/// }));
/// ```
pub fn parse_query(query: &str) -> Value {
    let mut root = Value::Object(Map::new());
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        insert_bracketed(&mut root, &key, Value::String(value.into_owned()));
    }
    normalize_lists(root)
}

/// Insert a value into the tree using a PHP-style bracketed key.
pub fn insert_bracketed(root: &mut Value, key: &str, value: Value) {
    let segments = parse_key(key);
    if segments.is_empty() {
        return;
    }
    let mut current = root;
    for (i, segment) in segments.iter().enumerate() {
        let last = i == segments.len() - 1;
        if !current.is_object() && !current.is_array() {
            *current = Value::Object(Map::new());
        }
        match segment {
            None => {
                // "[]" — append to a list.
                if current.as_object().is_some_and(|m| m.is_empty()) {
                    *current = Value::Array(Vec::new());
                }
                if let Value::Object(map) = current {
                    let next = map.len().to_string();
                    if last {
                        map.insert(next, value);
                        return;
                    }
                    current = map.entry(next).or_insert(Value::Object(Map::new()));
                    continue;
                }
                let Value::Array(items) = current else { unreachable!() };
                if last {
                    items.push(value);
                    return;
                }
                items.push(Value::Object(Map::new()));
                current = items.last_mut().unwrap();
            }
            Some(name) => {
                if let Value::Array(items) = current {
                    let map: Map<String, Value> = std::mem::take(items)
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| (i.to_string(), v))
                        .collect();
                    *current = Value::Object(map);
                }
                let Value::Object(map) = current else { unreachable!() };
                if last {
                    map.insert(name.clone(), value);
                    return;
                }
                current = map
                    .entry(name.clone())
                    .or_insert(Value::Object(Map::new()));
            }
        }
    }
}

/// Split `user[address][city]` into `["user", "address", "city"]`; `None`
/// segments represent `[]`.
fn parse_key(key: &str) -> Vec<Option<String>> {
    let Some(open) = key.find('[') else {
        return vec![Some(key.to_string())];
    };
    if !key.ends_with(']') || open == 0 {
        return vec![Some(key.to_string())];
    }
    let mut segments = vec![Some(key[..open].to_string())];
    let mut rest = &key[open..];
    while let Some(stripped) = rest.strip_prefix('[') {
        match stripped.find(']') {
            Some(close) => {
                let name = &stripped[..close];
                segments.push(if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                });
                rest = &stripped[close + 1..];
            }
            None => break,
        }
    }
    segments
}

/// Objects whose keys are exactly "0".."n-1" become lists.
pub fn normalize_lists(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let is_list = !map.is_empty()
                && map
                    .keys()
                    .enumerate()
                    .all(|(i, k)| k == &i.to_string());
            if is_list {
                Value::Array(map.into_iter().map(|(_, v)| normalize_lists(v)).collect())
            } else {
                Value::Object(
                    map.into_iter()
                        .map(|(k, v)| (k, normalize_lists(v)))
                        .collect(),
                )
            }
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_lists).collect()),
        other => other,
    }
}

/// Merge `other` into `base`, with `other` winning on conflicts.
pub fn merge_values(base: &mut Value, other: Value) {
    match (base, other) {
        (Value::Object(left), Value::Object(right)) => {
            for (k, v) in right {
                left.insert(k, v);
            }
        }
        (slot, other) => {
            if !other.is_null() {
                *slot = other;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_parses_nested_lists() {
        assert_eq!(
            parse_query("items[0][name]=a&items[1][name]=b"),
            json!({"items": [{"name": "a"}, {"name": "b"}]})
        );
        assert_eq!(
            parse_query("matrix[][]=1"),
            json!({"matrix": [[ "1" ]]})
        );
    }

    #[test]
    fn it_decodes_values() {
        assert_eq!(
            parse_query("q=hello%20world&plus=a+b&empty="),
            json!({"q": "hello world", "plus": "a b", "empty": ""})
        );
    }
}
