//! Reading and writing the data under validation with "dot" notation.
//!
//! Like Laravel, a dot always means "nested" and an escaped dot (`v1\.0`)
//! addresses a key that literally contains a period. Uploaded files live in
//! the same tree as small placeholder objects, so presence checks, wildcard
//! expansion and array counting treat them like any other input.

use illuminate_http::UploadedFile;
use illuminate_support::{Map, Value, json};

/// The marker key of an uploaded file placeholder.
pub(crate) const FILE_KEY: &str = "\u{0}uploaded_file";

/// Split an attribute into its segments, honoring `\.` escapes.
pub(crate) fn segments(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'.') => {
                current.push('.');
                chars.next();
            }
            '.' => out.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    out.push(current);
    out
}

/// Escape a literal key so it can be used as a single path segment.
pub(crate) fn escape_segment(key: &str) -> String {
    key.replace('.', "\\.")
}

/// Join already-escaped segments into a path.
pub(crate) fn join(prefix: &str, segment: &str) -> String {
    if prefix.is_empty() {
        segment.to_string()
    } else {
        format!("{prefix}.{segment}")
    }
}

/// The attribute as shown to humans (and used as the error key): escaped
/// dots become plain dots.
pub(crate) fn unescape(path: &str) -> String {
    path.replace("\\.", ".")
}

/// The index of the uploaded file a placeholder refers to.
pub(crate) fn file_index(value: &Value) -> Option<usize> {
    value
        .as_object()
        .and_then(|map| map.get(FILE_KEY))
        .and_then(Value::as_u64)
        .map(|i| i as usize)
}

/// Determine if the value is an uploaded file placeholder.
pub(crate) fn is_file(value: &Value) -> bool {
    file_index(value).is_some()
}

/// Build the placeholder stored in the data for an uploaded file.
pub(crate) fn file_placeholder(index: usize, file: &UploadedFile) -> Value {
    json!({
        FILE_KEY: index,
        "name": file.client_original_name(),
        "mime_type": file.mime_type(),
        "size": file.size(),
    })
}

/// Determine if the value is a PHP array (a list or a map, but not a file).
pub(crate) fn is_array(value: &Value) -> bool {
    match value {
        Value::Array(_) => true,
        Value::Object(_) => !is_file(value),
        _ => false,
    }
}

fn child<'a>(value: &'a Value, segment: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) if !is_file(value) => map.get(segment),
        Value::Array(items) => segment.parse::<usize>().ok().and_then(|i| items.get(i)),
        _ => None,
    }
}

/// Get the value at the given path.
pub(crate) fn get<'a>(data: &'a Value, path: &str) -> Option<&'a Value> {
    get_segments(data, &segments(path))
}

/// Get the value at the given (unescaped) segments.
pub(crate) fn get_segments<'a>(data: &'a Value, segments: &[String]) -> Option<&'a Value> {
    let mut current = data;
    for segment in segments {
        current = child(current, segment)?;
    }
    Some(current)
}

/// Determine if the given path exists (even when its value is `null`).
pub(crate) fn has(data: &Value, path: &str) -> bool {
    get(data, path).is_some()
}

/// Set a value at the given path, creating nested objects as needed.
pub(crate) fn set(data: &mut Value, path: &str, value: Value) {
    let segments = segments(path);
    set_segments(data, &segments, value);
}

pub(crate) fn set_segments(data: &mut Value, segments: &[String], value: Value) {
    let mut current = data;
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        if !(current.is_object() || current.is_array()) || is_file(current) {
            *current = Value::Object(Map::new());
        }
        let array_index = match &*current {
            Value::Array(items) => segment
                .parse::<usize>()
                .ok()
                .filter(|index| *index <= items.len()),
            _ => None,
        };
        if current.is_array() && array_index.is_none() {
            let items = match std::mem::take(current) {
                Value::Array(items) => items,
                _ => Vec::new(),
            };
            *current = Value::Object(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v))
                    .collect(),
            );
        }
        match current {
            Value::Array(items) => {
                let index = array_index.unwrap_or(items.len());
                if index == items.len() {
                    items.push(Value::Null);
                }
                if last {
                    items[index] = value;
                    return;
                }
                current = &mut items[index];
            }
            Value::Object(map) => {
                if last {
                    map.insert(segment.clone(), value);
                    return;
                }
                current = map
                    .entry(segment.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
            }
            _ => unreachable!("containers were normalized above"),
        }
    }
}

/// Remove the value at the given path.
pub(crate) fn forget(data: &mut Value, path: &str) {
    let segments = segments(path);
    let Some((last, parents)) = segments.split_last() else {
        return;
    };
    let mut current = data;
    for segment in parents {
        current = match current {
            Value::Object(map) => match map.get_mut(segment) {
                Some(next) => next,
                None => return,
            },
            Value::Array(items) => {
                match segment.parse::<usize>().ok().and_then(|i| items.get_mut(i)) {
                    Some(next) => next,
                    None => return,
                }
            }
            _ => return,
        };
    }
    match current {
        Value::Object(map) if !map.contains_key(FILE_KEY) => {
            map.shift_remove(last);
        }
        Value::Array(items) => {
            // PHP's unset() leaves a gap; we emulate it by converting the
            // list into a map keyed by the remaining indexes.
            if let Ok(index) = last.parse::<usize>()
                && index < items.len()
            {
                let map: Map<String, Value> = std::mem::take(items)
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| *i != index)
                    .map(|(i, v)| (i.to_string(), v))
                    .collect();
                *current = Value::Object(map);
            }
        }
        _ => {}
    }
}

/// The keys of an array value (list indexes or map keys), escaped for use
/// as path segments.
pub(crate) fn keys(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => (0..items.len()).map(|i| i.to_string()).collect(),
        Value::Object(map) if !is_file(value) => map.keys().map(|k| escape_segment(k)).collect(),
        _ => Vec::new(),
    }
}

/// Determine if an array value has the given (literal) key.
pub(crate) fn array_has_key(value: &Value, key: &str) -> bool {
    child(value, key).is_some()
}

/// Flatten a value with "dot" keys, exactly like Laravel's `Arr::dot`:
/// empty arrays (and files) are kept as leaves.
pub(crate) fn flatten<'a>(value: &'a Value, prefix: &str, out: &mut Vec<(String, &'a Value)>) {
    if is_array(value) && !keys_is_empty(value) {
        for key in keys(value) {
            if let Some(item) = child(value, &key.replace("\\.", ".")) {
                flatten(item, &join(prefix, &key), out);
            }
        }
    } else if !prefix.is_empty() {
        out.push((prefix.to_string(), value));
    }
}

fn keys_is_empty(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        _ => true,
    }
}

/// Determine if a path matches a wildcard pattern segment-by-segment.
/// `*` matches exactly one segment (possibly empty when `allow_empty`).
pub(crate) fn matches_pattern(pattern: &str, path: &str, allow_empty: bool) -> bool {
    let pattern = segments_raw(pattern);
    let path = segments_raw(path);
    pattern.len() == path.len()
        && pattern
            .iter()
            .zip(&path)
            .all(|(p, s)| (p == "*" && (allow_empty || !s.is_empty())) || p == s)
}

/// Split a path into its *escaped* segments (keeping `\.` intact).
pub(crate) fn segments_raw(path: &str) -> Vec<String> {
    segments(path)
        .into_iter()
        .map(|s| escape_segment(&s))
        .collect()
}

/// The explicit path that leads up to the first wildcard (`users.*.email`
/// gives `users`).
pub(crate) fn leading_explicit_path(attribute: &str) -> Option<String> {
    let segments = segments_raw(attribute);
    let explicit: Vec<String> = segments.into_iter().take_while(|s| s != "*").collect();
    if explicit.is_empty() {
        None
    } else {
        Some(explicit.join("."))
    }
}

/// Expand a wildcard attribute (`items.*.name`) into the concrete
/// attributes present in the data (`items.0.name`, `items.1.name`).
pub(crate) fn expand_wildcard(pattern: &str, data: &Value) -> Vec<String> {
    let segments = segments_raw(pattern);
    let mut out = Vec::new();
    walk(Some(data), &segments, String::new(), &mut out);
    out
}

fn walk(node: Option<&Value>, rest: &[String], prefix: String, out: &mut Vec<String>) {
    let Some((segment, tail)) = rest.split_first() else {
        out.push(prefix);
        return;
    };
    if segment == "*" {
        let Some(node) = node else { return };
        if !is_array(node) {
            return;
        }
        for key in keys(node) {
            let child_value = child(node, &key.replace("\\.", "."));
            walk(child_value, tail, join(&prefix, &key), out);
        }
        return;
    }
    if !tail.iter().any(|s| s == "*") {
        // Trailing explicit segments are always produced, even when missing:
        // Laravel fills them with `null` before gathering the keys.
        if node.is_some() || !prefix.is_empty() {
            let mut path = join(&prefix, segment);
            for s in tail {
                path = join(&path, s);
            }
            out.push(path);
        }
        return;
    }
    let next = node.and_then(|n| child(n, &segment.replace("\\.", ".")));
    if next.is_none() {
        return;
    }
    walk(next, tail, join(&prefix, segment), out);
}

/// Convert PHP's bracketed input names (`user[avatar]`, `photos[]`) into
/// dot paths. The boolean is `true` when the name ended with `[]`.
pub(crate) fn bracket_to_dot(name: &str) -> (String, bool) {
    let is_list = name.ends_with("[]");
    let name = name.trim_end_matches("[]");
    let Some(open) = name.find('[') else {
        return (escape_segment(name), is_list);
    };
    let mut parts = vec![escape_segment(&name[..open])];
    let mut rest = &name[open..];
    while let Some(stripped) = rest.strip_prefix('[') {
        match stripped.find(']') {
            Some(close) => {
                parts.push(escape_segment(&stripped[..close]));
                rest = &stripped[close + 1..];
            }
            None => break,
        }
    }
    (parts.join("."), is_list)
}

/// Remove uploaded file placeholders from a value. Returns `None` when the
/// value was a file (or a container holding only files).
pub(crate) fn strip_files(value: Value) -> Option<Value> {
    if is_file(&value) {
        return None;
    }
    match value {
        Value::Array(items) => {
            let had_items = !items.is_empty();
            let kept: Vec<Value> = items.into_iter().filter_map(strip_files).collect();
            if had_items && kept.is_empty() {
                None
            } else {
                Some(Value::Array(kept))
            }
        }
        Value::Object(map) => {
            let had_items = !map.is_empty();
            let kept: Map<String, Value> = map
                .into_iter()
                .filter_map(|(k, v)| strip_files(v).map(|v| (k, v)))
                .collect();
            if had_items && kept.is_empty() {
                None
            } else {
                Some(Value::Object(kept))
            }
        }
        other => Some(other),
    }
}

/// Collect every file placeholder in a value, keyed by its dot path.
pub(crate) fn collect_files(value: &Value, prefix: &str, out: &mut Vec<(String, usize)>) {
    if let Some(index) = file_index(value) {
        out.push((prefix.to_string(), index));
        return;
    }
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                collect_files(item, &join(prefix, &i.to_string()), out);
            }
        }
        Value::Object(map) => {
            for (k, item) in map {
                collect_files(item, &join(prefix, k), out);
            }
        }
        _ => {}
    }
}

/// A tree used to assemble the validated data, mirroring PHP's `Arr::set`
/// on arrays: containers become lists when their keys are `0..n`.
pub(crate) enum Node {
    Leaf(Value),
    Branch(indexmap::IndexMap<String, Node>),
}

impl Node {
    pub(crate) fn new() -> Self {
        Node::Branch(indexmap::IndexMap::new())
    }

    pub(crate) fn set(&mut self, segments: &[String], value: Value) {
        let Some((first, rest)) = segments.split_first() else {
            *self = Node::Leaf(value);
            return;
        };
        if let Node::Leaf(existing) = self {
            let existing = std::mem::take(existing);
            let mut branch = indexmap::IndexMap::new();
            match existing {
                Value::Object(map) if !map.contains_key(FILE_KEY) => {
                    for (k, v) in map {
                        branch.insert(k, Node::Leaf(v));
                    }
                }
                Value::Array(items) => {
                    for (i, v) in items.into_iter().enumerate() {
                        branch.insert(i.to_string(), Node::Leaf(v));
                    }
                }
                _ => {}
            }
            *self = Node::Branch(branch);
        }
        let Node::Branch(children) = self else {
            unreachable!()
        };
        let child = children.entry(first.clone()).or_insert_with(Node::new);
        if rest.is_empty() {
            *child = Node::Leaf(value);
        } else {
            child.set(rest, value);
        }
    }

    pub(crate) fn into_value(self) -> Value {
        match self {
            Node::Leaf(value) => value,
            Node::Branch(children) => {
                let is_list = !children.is_empty()
                    && children
                        .keys()
                        .enumerate()
                        .all(|(i, k)| *k == i.to_string());
                if is_list {
                    Value::Array(children.into_values().map(Node::into_value).collect())
                } else {
                    Value::Object(
                        children
                            .into_iter()
                            .map(|(k, v)| (k, v.into_value()))
                            .collect(),
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_splits_escaped_paths() {
        assert_eq!(segments("a.b\\.c.d"), vec!["a", "b.c", "d"]);
        assert_eq!(unescape("v1\\.0"), "v1.0");
    }

    #[test]
    fn it_reads_nested_values() {
        let data = json!({"user": {"name": "Taylor", "tags": ["a", "b"]}, "v1.0": true});
        assert_eq!(get(&data, "user.name"), Some(&json!("Taylor")));
        assert_eq!(get(&data, "user.tags.1"), Some(&json!("b")));
        assert_eq!(get(&data, "v1\\.0"), Some(&json!(true)));
        assert_eq!(get(&data, "v1.0"), None);
        assert!(has(&data, "user"));
    }

    #[test]
    fn it_expands_wildcards() {
        let data = json!({"items": [{"name": "a"}, {"other": 1}, "scalar"], "empty": []});
        assert_eq!(
            expand_wildcard("items.*.name", &data),
            vec!["items.0.name", "items.1.name", "items.2.name"]
        );
        assert_eq!(
            expand_wildcard("items.*", &data),
            vec!["items.0", "items.1", "items.2"]
        );
        assert!(expand_wildcard("missing.*.name", &data).is_empty());
        assert!(expand_wildcard("empty.*", &data).is_empty());
        let nested = json!({"a": [{"b": [1, 2]}, {"b": [3]}]});
        assert_eq!(
            expand_wildcard("a.*.b.*", &nested),
            vec!["a.0.b.0", "a.0.b.1", "a.1.b.0"]
        );
    }

    #[test]
    fn it_sets_and_forgets_values() {
        let mut data = json!({});
        set(&mut data, "user.name", json!("Taylor"));
        assert_eq!(data, json!({"user": {"name": "Taylor"}}));
        forget(&mut data, "user.name");
        assert_eq!(data, json!({"user": {}}));
    }

    #[test]
    fn it_converts_bracket_names() {
        assert_eq!(
            bracket_to_dot("user[avatar]"),
            ("user.avatar".to_string(), false)
        );
        assert_eq!(bracket_to_dot("photos[]"), ("photos".to_string(), true));
        assert_eq!(bracket_to_dot("avatar"), ("avatar".to_string(), false));
    }

    #[test]
    fn nodes_become_lists_when_sequential() {
        let mut node = Node::new();
        node.set(&["items".into(), "0".into(), "name".into()], json!("a"));
        node.set(&["items".into(), "1".into(), "name".into()], json!("b"));
        assert_eq!(
            node.into_value(),
            json!({"items": [{"name": "a"}, {"name": "b"}]})
        );
    }
}
