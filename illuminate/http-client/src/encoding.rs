//! Encoding request data: query strings, form bodies and multipart bodies.

use bytes::{BufMut, Bytes, BytesMut};
use illuminate_support::{Arr, Map, UriQueryString, Value, ValueExt};
use rand::Rng;

/// Convert loose input into an object: arrays of `[key, value]` pairs (what
/// serializing `[("name", "Taylor")]` produces) become objects; anything
/// else is returned untouched.
pub(crate) fn normalize_pairs(value: Value) -> Value {
    match value {
        Value::Array(items)
            if !items.is_empty()
                && items
                    .iter()
                    .all(|item| matches!(item, Value::Array(pair) if pair.len() == 2 && pair[0].is_string())) =>
        {
            let mut map = Map::new();
            for item in items {
                if let Value::Array(mut pair) = item {
                    let value = pair.pop().unwrap_or(Value::Null);
                    let key = pair.pop().and_then(|k| k.as_str().map(str::to_string)).unwrap_or_default();
                    map.insert(key, value);
                }
            }
            Value::Object(map)
        }
        other => other,
    }
}

/// Build a query string (RFC 3986 encoding), like PHP's
/// `http_build_query($query, '', '&', PHP_QUERY_RFC3986)`.
pub(crate) fn build_query(value: &Value) -> String {
    match value {
        Value::String(raw) => raw.trim_start_matches('?').to_string(),
        Value::Null => String::new(),
        other => Arr::query(other),
    }
}

/// Build a form body (`application/x-www-form-urlencoded`), like PHP's
/// `http_build_query($data)`: spaces become `+` and nested data uses
/// bracket notation.
pub(crate) fn build_form(value: &Value) -> String {
    if let Value::String(raw) = value {
        return raw.clone();
    }

    let mut pairs = Vec::new();
    flatten(None, value, &mut pairs);

    pairs
        .into_iter()
        .map(|(key, value)| format!("{}={}", form_encode(&key), form_encode(&value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Flatten nested data into `key[sub]` / value pairs.
pub(crate) fn flatten(prefix: Option<&str>, value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Null => {}
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let key = match prefix {
                    Some(prefix) => format!("{prefix}[{index}]"),
                    None => index.to_string(),
                };
                flatten(Some(&key), item, out);
            }
        }
        Value::Object(map) => {
            for (name, item) in map {
                let key = match prefix {
                    Some(prefix) => format!("{prefix}[{name}]"),
                    None => name.clone(),
                };
                flatten(Some(&key), item, out);
            }
        }
        Value::Bool(flag) => {
            if let Some(key) = prefix {
                out.push((key.to_string(), if *flag { "1" } else { "0" }.to_string()));
            }
        }
        scalar => {
            if let Some(key) = prefix {
                out.push((key.to_string(), scalar.to_string_lossy()));
            }
        }
    }
}

fn form_encode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Parse a form body (or query string) into nested data.
pub(crate) fn parse_form(body: &str) -> Value {
    UriQueryString::new(body.replace('+', "%20")).all()
}

/// A single part of a multipart request.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// The form field name.
    pub name: String,
    /// The part's contents.
    pub contents: Bytes,
    /// The filename, when the part is a file.
    pub filename: Option<String>,
    /// Additional headers for the part.
    pub headers: Vec<(String, String)>,
}

impl Part {
    /// Create a new multipart part.
    pub fn new(name: impl Into<String>, contents: impl Into<Bytes>) -> Self {
        Self {
            name: name.into(),
            contents: contents.into(),
            filename: None,
            headers: Vec::new(),
        }
    }

    /// Set the part's filename.
    pub fn filename(mut self, filename: impl Into<String>) -> Self {
        let filename = filename.into();
        self.filename = (!filename.is_empty()).then_some(filename);
        self
    }

    /// Add a header to the part.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The part as data, for request inspection.
    pub(crate) fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".into(), Value::String(self.name.clone()));
        map.insert(
            "contents".into(),
            Value::String(String::from_utf8_lossy(&self.contents).into_owned()),
        );
        if let Some(filename) = &self.filename {
            map.insert("filename".into(), Value::String(filename.clone()));
        }
        if !self.headers.is_empty() {
            let headers = self
                .headers
                .iter()
                .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                .collect();
            map.insert("headers".into(), Value::Object(headers));
        }
        Value::Object(map)
    }

    fn content_type(&self) -> Option<String> {
        self.headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone())
            .or_else(|| {
                self.filename
                    .as_ref()
                    .and_then(|filename| mime_guess::from_path(filename).first())
                    .map(|mime| mime.essence_str().to_string())
            })
    }
}

/// Convert request data into multipart parts.
///
/// Objects become one part per (flattened) field; lists of
/// `{"name": ..., "contents": ...}` objects are used as-is.
pub(crate) fn parts_from_data(data: &Value) -> Vec<Part> {
    match data {
        Value::Array(items) if items.iter().all(is_part_definition) => {
            items.iter().filter_map(part_from_definition).collect()
        }
        Value::Object(map) => {
            let mut parts = Vec::new();
            for (name, value) in map {
                if is_part_definition(value) {
                    parts.extend(part_from_definition(value));
                    continue;
                }
                let mut pairs = Vec::new();
                match value {
                    Value::Null => pairs.push((name.clone(), String::new())),
                    _ => flatten(Some(name), value, &mut pairs),
                }
                parts.extend(
                    pairs
                        .into_iter()
                        .map(|(name, value)| Part::new(name, value)),
                );
            }
            parts
        }
        _ => Vec::new(),
    }
}

fn is_part_definition(value: &Value) -> bool {
    value.get("name").is_some_and(Value::is_string) && value.get("contents").is_some()
}

fn part_from_definition(value: &Value) -> Option<Part> {
    let name = value.get("name")?.as_str()?;
    let contents = match value.get("contents")? {
        Value::String(contents) => contents.clone(),
        other => other.to_string_lossy(),
    };
    let mut part = Part::new(name.to_string(), contents);
    if let Some(filename) = value.get("filename").and_then(Value::as_str) {
        part = part.filename(filename);
    }
    if let Some(Value::Object(headers)) = value.get("headers") {
        for (header, value) in headers {
            part = part.header(header.clone(), value.to_string_lossy());
        }
    }
    Some(part)
}

/// Generate a random multipart boundary.
pub(crate) fn boundary() -> String {
    let bytes: [u8; 20] = rand::rng().random();
    hex::encode(bytes)
}

/// Encode the parts as a `multipart/form-data` body.
pub(crate) fn encode_multipart(parts: &[Part], boundary: &str) -> Bytes {
    let mut body = BytesMut::new();

    for part in parts {
        body.put_slice(format!("--{boundary}\r\n").as_bytes());

        let mut disposition = format!("form-data; name=\"{}\"", escape_quoted(&part.name));
        if let Some(filename) = &part.filename {
            disposition.push_str(&format!("; filename=\"{}\"", escape_quoted(filename)));
        }

        let has_disposition = part
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-disposition"));

        if !has_disposition {
            body.put_slice(format!("Content-Disposition: {disposition}\r\n").as_bytes());
        }

        for (name, value) in &part.headers {
            if !name.eq_ignore_ascii_case("content-type") {
                body.put_slice(format!("{name}: {value}\r\n").as_bytes());
            }
        }

        if let Some(content_type) = part.content_type() {
            body.put_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
        }

        body.put_slice(b"\r\n");
        body.put_slice(&part.contents);
        body.put_slice(b"\r\n");
    }

    body.put_slice(format!("--{boundary}--\r\n").as_bytes());
    body.freeze()
}

fn escape_quoted(value: &str) -> String {
    value
        .replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Recursively replace values in `target` with those in `source`
/// (PHP's `array_replace_recursive`, for objects).
pub(crate) fn replace_recursive(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                match target.get_mut(&key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        replace_recursive(existing, value)
                    }
                    _ => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (target, source) => *target = source,
    }
}

/// Merge `source` into `target`, combining values that share a key (PHP's
/// `array_merge_recursive`, for objects).
pub(crate) fn merge_recursive(target: &mut Map<String, Value>, source: Map<String, Value>) {
    for (key, value) in source {
        match target.get_mut(&key) {
            None => {
                target.insert(key, value);
            }
            Some(Value::Object(existing)) if value.is_object() => {
                if let Value::Object(value) = value {
                    merge_recursive(existing, value);
                }
            }
            Some(existing) => {
                let mut values = match existing.take() {
                    Value::Array(items) => items,
                    other => vec![other],
                };
                match value {
                    Value::Array(items) => values.extend(items),
                    other => values.push(other),
                }
                *existing = Value::Array(values);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_normalizes_pairs_into_objects() {
        let pairs = serde_json::to_value([("name", "Taylor"), ("page", "1")]).unwrap();
        assert_eq!(
            normalize_pairs(pairs),
            json!({"name": "Taylor", "page": "1"})
        );
        assert_eq!(normalize_pairs(json!([1, 2])), json!([1, 2]));
        assert_eq!(normalize_pairs(json!([])), json!([]));
    }

    #[test]
    fn it_builds_query_strings() {
        assert_eq!(
            build_query(
                &json!({"name": "Taylor Otwell", "page": 1, "tags": ["a", "b"], "on": true})
            ),
            "name=Taylor%20Otwell&page=1&tags%5B0%5D=a&tags%5B1%5D=b&on=1"
        );
        assert_eq!(build_query(&json!("?a=b")), "a=b");
        assert_eq!(build_query(&Value::Null), "");
    }

    #[test]
    fn it_builds_and_parses_form_bodies() {
        let data = json!({"name": "Sara", "role": "Privacy Consultant", "nested": {"a": 1}, "off": false, "skip": null});
        let body = build_form(&data);
        assert_eq!(
            body,
            "name=Sara&role=Privacy+Consultant&nested%5Ba%5D=1&off=0"
        );
        assert_eq!(
            parse_form(&body),
            json!({"name": "Sara", "role": "Privacy Consultant", "nested": {"a": "1"}, "off": "0"})
        );
        assert_eq!(parse_form("plus=a%2Bb"), json!({"plus": "a+b"}));
        assert_eq!(build_form(&json!("raw=1")), "raw=1");
    }

    #[test]
    fn it_builds_multipart_parts() {
        let parts = parts_from_data(&json!({
            "name": "Taylor",
            "tags": ["a", "b"],
            "avatar": {"name": "avatar", "contents": "PNG", "filename": "me.png"},
        }));
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[1].name, "tags[0]");
        assert_eq!(parts[3].filename.as_deref(), Some("me.png"));

        let listed =
            parts_from_data(&json!([{"name": "doc", "contents": "x", "headers": {"X-Part": "1"}}]));
        assert_eq!(
            listed[0].headers,
            vec![("X-Part".to_string(), "1".to_string())]
        );
        assert!(parts_from_data(&json!("nope")).is_empty());
        assert_eq!(parts[3].to_value()["filename"], "me.png");
    }

    #[test]
    fn it_encodes_multipart_bodies() {
        let parts = vec![
            Part::new("name", "Taylor"),
            Part::new("photo", "PNG!")
                .filename("photo \"me\".png")
                .header("X-Extra", "1"),
            Part::new("raw", "x").filename(""),
        ];
        let body = encode_multipart(&parts, "BOUNDARY");
        let text = String::from_utf8(body.to_vec()).unwrap();

        assert_eq!(
            text,
            "--BOUNDARY\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nTaylor\r\n\
             --BOUNDARY\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"photo %22me%22.png\"\r\nX-Extra: 1\r\nContent-Type: image/png\r\n\r\nPNG!\r\n\
             --BOUNDARY\r\nContent-Disposition: form-data; name=\"raw\"\r\n\r\nx\r\n\
             --BOUNDARY--\r\n"
        );
        assert_eq!(boundary().len(), 40);
    }

    #[test]
    fn it_merges_and_replaces_recursively() {
        let mut target = json!({"headers": {"A": "1"}, "timeout": 30});
        replace_recursive(&mut target, json!({"headers": {"B": "2"}, "timeout": 5}));
        assert_eq!(
            target,
            json!({"headers": {"A": "1", "B": "2"}, "timeout": 5})
        );

        let mut map = json!({"A": "1", "B": ["x"], "C": {"d": 1}})
            .as_object()
            .unwrap()
            .clone();
        merge_recursive(
            &mut map,
            json!({"A": "2", "B": "y", "C": {"e": 2}, "D": 4})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(
            Value::Object(map),
            json!({"A": ["1", "2"], "B": ["x", "y"], "C": {"d": 1, "e": 2}, "D": 4})
        );
    }
}
