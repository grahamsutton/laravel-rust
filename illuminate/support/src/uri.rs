//! Fluent URIs.
//!
//! ```
//! use illuminate_support::{Uri, json};
//!
//! let uri = Uri::of("https://example.com")
//!     .with_scheme("http")
//!     .with_host("test.com")
//!     .with_port(Some(8000))
//!     .with_path("/users")
//!     .with_query(json!({"page": 2}))
//!     .with_fragment("section-1");
//!
//! assert_eq!(uri.to_string(), "http://test.com:8000/users?page=2#section-1");
//! assert_eq!(uri.path(), "users");
//! assert_eq!(uri.query().get("page"), json!("2"));
//! ```

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::arr::Arr;
use crate::collection::Collection;
use crate::stringable::Stringable;
use crate::value::{Map, Value, ValueExt};

/// An immutable, fluent URI.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Uri {
    scheme: Option<String>,
    user: Option<String>,
    password: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
}

impl Uri {
    /// Create a new URI instance from the given string.
    pub fn of(uri: impl AsRef<str>) -> Self {
        parse(uri.as_ref())
    }

    /// Get the URI's scheme.
    pub fn scheme(&self) -> Option<&str> {
        self.scheme.as_deref()
    }

    /// Get the URI's user name.
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Get the URI's user info, including the password (`taylor:secret`).
    pub fn user_info(&self) -> Option<String> {
        self.user.as_ref().map(|user| match &self.password {
            Some(password) => format!("{user}:{password}"),
            None => user.clone(),
        })
    }

    /// Get the URI's password.
    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    /// Get the URI's host.
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// Get the URI's port.
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// Get the URI's authority (`user:password@host:port`).
    pub fn authority(&self) -> Option<String> {
        let host = self.host.as_ref()?;
        let mut authority = String::new();
        if let Some(user_info) = self.user_info() {
            authority.push_str(&user_info);
            authority.push('@');
        }
        authority.push_str(host);
        if let Some(port) = self.port {
            authority.push_str(&format!(":{port}"));
        }
        Some(authority)
    }

    /// Get the URI's path, without leading or trailing slashes (`/` when empty).
    pub fn path(&self) -> String {
        let path = self.path.trim_matches('/');
        if path.is_empty() {
            "/".to_string()
        } else {
            path.to_string()
        }
    }

    /// Get the URI's path segments.
    pub fn path_segments(&self) -> Collection<String> {
        let path = self.path();
        if path == "/" {
            Collection::new()
        } else {
            Collection::make(path.split('/').map(String::from))
        }
    }

    /// Get the URI's query string.
    pub fn query(&self) -> UriQueryString {
        UriQueryString {
            raw: self.query.clone().unwrap_or_default(),
        }
    }

    /// Get the URI's fragment.
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    /// Specify the scheme of the URI.
    pub fn with_scheme(mut self, scheme: impl Into<String>) -> Self {
        let scheme = scheme.into();
        self.scheme = (!scheme.is_empty()).then_some(scheme);
        self
    }

    /// Specify the user and password of the URI.
    pub fn with_user(mut self, user: Option<&str>, password: Option<&str>) -> Self {
        self.user = user.filter(|u| !u.is_empty()).map(String::from);
        self.password = self.user.as_ref().and(password.map(String::from));
        self
    }

    /// Specify the host of the URI.
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        let host = host.into();
        self.host = Some(host);
        self
    }

    /// Specify the port of the URI.
    pub fn with_port(mut self, port: Option<u16>) -> Self {
        self.port = port;
        self
    }

    /// Specify the path of the URI.
    pub fn with_path(mut self, path: impl AsRef<str>) -> Self {
        self.path = crate::str::Str::start(path.as_ref(), "/");
        self
    }

    /// Merge the given query parameters into the URI's query string. Keys
    /// may use "dot" notation (`filter.name`).
    pub fn with_query(self, query: Value) -> Self {
        self.with_query_merging(query, true)
    }

    /// Set the URI's query string, replacing or merging with the existing one.
    pub fn with_query_merging(mut self, query: Value, merge: bool) -> Self {
        let mut new_query = if merge {
            self.query().all()
        } else {
            Value::Object(Map::new())
        };
        if let Value::Object(map) = query {
            for (key, value) in map {
                Arr::set(&mut new_query, &key, value);
            }
        }
        let built = Arr::query(&new_query);
        self.query = (!built.is_empty()).then_some(built);
        self
    }

    /// Merge the given query parameters if their keys are not already present.
    pub fn with_query_if_missing(self, query: Value) -> Self {
        let current = self.query();
        let mut query = query;
        if let Value::Object(map) = &query {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if !current.missing(&key) {
                    Arr::forget(&mut query, &key);
                }
            }
        }
        self.with_query(query)
    }

    /// Push a value onto the end of a query string parameter that is a list.
    pub fn push_onto_query(self, key: &str, value: impl Into<Value>) -> Self {
        let current = crate::helpers::data_get(&self.query().all(), key);
        let values = match Arr::wrap(value.into()) {
            Value::Array(items) => items,
            _ => Vec::new(),
        };
        let merged = match current {
            Value::Array(mut items) => {
                for value in values {
                    if !items.contains(&value) {
                        items.push(value);
                    }
                }
                Value::Array(items)
            }
            Value::Object(mut map) => {
                let mut next = map.len();
                for value in values {
                    while map.contains_key(&next.to_string()) {
                        next += 1;
                    }
                    map.insert(next.to_string(), value);
                }
                Value::Object(map)
            }
            Value::Null => Value::Array(values),
            other => {
                let mut items = vec![other];
                items.extend(values);
                Value::Array(items)
            }
        };
        let mut query = Map::new();
        query.insert(key.to_string(), merged);
        self.with_query(Value::Object(query))
    }

    /// Remove the given query parameters from the URI.
    pub fn without_query(self, keys: &[&str]) -> Self {
        let remaining = Arr::except(&self.query().all(), keys);
        self.replace_query(remaining)
    }

    /// Replace the URI's query string entirely.
    pub fn replace_query(self, query: Value) -> Self {
        self.with_query_merging(query, false)
    }

    /// Specify the fragment of the URI.
    pub fn with_fragment(mut self, fragment: impl Into<String>) -> Self {
        self.fragment = Some(fragment.into());
        self
    }

    /// Remove the fragment from the URI.
    pub fn without_fragment(mut self) -> Self {
        self.fragment = None;
        self
    }

    /// Get the URI as a string with a decoded query string.
    pub fn decode(&self) -> String {
        let mut copy = self.clone();
        if let Some(query) = &self.query {
            copy.query = Some(raw_url_decode(query));
        }
        copy.to_string()
    }

    /// Get the string representation of the URI.
    pub fn value(&self) -> String {
        self.to_string()
    }

    /// Convert the URI into a fluent [`Stringable`].
    pub fn to_stringable(&self) -> Stringable {
        Stringable::new(self.to_string())
    }

    /// Determine if the URI is empty.
    pub fn is_empty(&self) -> bool {
        self.to_string().trim().is_empty()
    }

    /// Determine if the URI is not empty.
    pub fn is_not_empty(&self) -> bool {
        !self.is_empty()
    }
}

impl crate::traits::Conditionable for Uri {}
impl crate::traits::Tappable for Uri {}

impl fmt::Display for Uri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(scheme) = &self.scheme {
            write!(f, "{scheme}:")?;
        }
        if let Some(authority) = self.authority() {
            write!(f, "//{authority}")?;
        }
        f.write_str(&self.path)?;
        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }
        if let Some(fragment) = &self.fragment {
            write!(f, "#{fragment}")?;
        }
        Ok(())
    }
}

impl FromStr for Uri {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Uri::of(s))
    }
}

impl From<&str> for Uri {
    fn from(value: &str) -> Self {
        Uri::of(value)
    }
}

impl From<String> for Uri {
    fn from(value: String) -> Self {
        Uri::of(value)
    }
}

impl From<Uri> for String {
    fn from(uri: Uri) -> Self {
        uri.to_string()
    }
}

impl Serialize for Uri {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Uri {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Uri::of(String::deserialize(deserializer)?))
    }
}

/// A URI's query string, with convenient typed accessors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UriQueryString {
    raw: String,
}

impl UriQueryString {
    /// Parse a raw query string (without the leading `?`).
    pub fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }

    /// Get all of the query parameters as a (nested) value.
    pub fn all(&self) -> Value {
        parse_query(&self.raw)
    }

    /// Get only the given keys from the query parameters.
    pub fn only(&self, keys: &[&str]) -> Value {
        let all = self.all();
        let mut out = Value::Object(Map::new());
        for key in keys {
            Arr::set(&mut out, key, Arr::get(&all, key));
        }
        out
    }

    /// Get a query parameter using "dot" notation (null when missing).
    pub fn get(&self, key: &str) -> Value {
        crate::helpers::data_get(&self.all(), key)
    }

    /// Determine if the query string contains the given key.
    pub fn has(&self, key: &str) -> bool {
        Arr::has(&self.all(), key)
    }

    /// Determine if the query string is missing the given key.
    pub fn missing(&self, key: &str) -> bool {
        !self.has(key)
    }

    /// Get a query parameter as a string.
    pub fn string(&self, key: &str) -> String {
        self.get(key).to_string_lossy()
    }

    /// Get a query parameter as an integer (0 when missing or invalid).
    pub fn integer(&self, key: &str) -> i64 {
        self.get(key).to_i64_lossy().unwrap_or(0)
    }

    /// Get a query parameter as a boolean.
    pub fn boolean(&self, key: &str) -> bool {
        Stringable::new(self.string(key)).to_boolean()
    }

    /// Get the raw (encoded) query string.
    pub fn value(&self) -> &str {
        &self.raw
    }

    /// Get the query string with percent-encoding removed.
    pub fn decode(&self) -> String {
        raw_url_decode(&self.raw)
    }

    /// Get the query parameters as a value (alias of `all`).
    pub fn to_array(&self) -> Value {
        self.all()
    }

    /// Determine if there are no query parameters.
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
}

impl fmt::Display for UriQueryString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

fn parse(input: &str) -> Uri {
    let mut uri = Uri::default();
    let mut rest = input;

    if let Some(index) = rest.find('#') {
        uri.fragment = Some(rest[index + 1..].to_string());
        rest = &rest[..index];
    }
    if let Some(index) = rest.find('?') {
        uri.query = Some(rest[index + 1..].to_string());
        rest = &rest[..index];
    }
    if let Some(index) = rest.find(':') {
        let candidate = &rest[..index];
        let valid = candidate
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
            && !candidate.contains('/');
        if valid {
            uri.scheme = Some(candidate.to_string());
            rest = &rest[index + 1..];
        }
    }
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find('/').unwrap_or(after.len());
        let authority = &after[..end];
        rest = &after[end..];
        let host_port = match authority.rfind('@') {
            Some(at) => {
                let user_info = &authority[..at];
                match user_info.split_once(':') {
                    Some((user, password)) => {
                        uri.user = Some(user.to_string());
                        uri.password = Some(password.to_string());
                    }
                    None => uri.user = Some(user_info.to_string()),
                }
                &authority[at + 1..]
            }
            None => authority,
        };
        let (host, port) = if host_port.starts_with('[') {
            match host_port.find(']') {
                Some(close) => (&host_port[..=close], host_port[close + 1..].strip_prefix(':')),
                None => (host_port, None),
            }
        } else {
            match host_port.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (host_port, None),
            }
        };
        uri.host = Some(host.to_string());
        uri.port = port.and_then(|p| p.parse().ok());
    }
    uri.path = rest.to_string();
    uri
}

/// Parse a query string into a nested value, understanding `a[b]=c` and
/// `a[]=b` syntax like PHP's `parse_str` (but keeping dots in keys).
pub(crate) fn parse_query(raw: &str) -> Value {
    let mut root = Value::Object(Map::new());
    for pair in raw.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (raw_url_decode(k), raw_url_decode(v)),
            None => (raw_url_decode(pair), String::new()),
        };
        let (base, mut brackets) = match key.find('[') {
            Some(index) if index > 0 && key.ends_with(']') => (&key[..index], &key[index..]),
            _ => (key.as_str(), ""),
        };
        let mut path: Vec<Option<String>> = vec![Some(base.to_string())];
        while let Some(rest) = brackets.strip_prefix('[') {
            match rest.find(']') {
                Some(close) => {
                    let segment = &rest[..close];
                    path.push((!segment.is_empty()).then(|| segment.to_string()));
                    brackets = &rest[close + 1..];
                }
                None => break,
            }
        }
        insert_query_value(&mut root, &path, Value::String(value));
    }
    listify(root)
}

fn insert_query_value(target: &mut Value, path: &[Option<String>], value: Value) {
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let Value::Object(map) = target else { return };
    let key = match &path[0] {
        Some(key) => key.clone(),
        None => {
            let next = map
                .keys()
                .filter_map(|k| k.parse::<usize>().ok())
                .max()
                .map(|max| max + 1)
                .unwrap_or(0);
            next.to_string()
        }
    };
    if path.len() == 1 {
        map.insert(key, value);
    } else {
        let child = map.entry(key).or_insert_with(|| Value::Object(Map::new()));
        insert_query_value(child, &path[1..], value);
    }
}

/// Turn objects keyed `0..n` into arrays, recursively.
fn listify(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let is_list = !map.is_empty() && map.keys().enumerate().all(|(i, k)| k == &i.to_string());
            if is_list {
                Value::Array(map.into_iter().map(|(_, v)| listify(v)).collect())
            } else {
                Value::Object(map.into_iter().map(|(k, v)| (k, listify(v))).collect())
            }
        }
        other => other,
    }
}

/// Decode `%XX` sequences, like PHP's `rawurldecode` (`+` is left alone).
pub(crate) fn raw_url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hex = [bytes[i + 1], bytes[i + 2]];
            let hex = std::str::from_utf8(&hex).unwrap_or("00");
            out.push(u8::from_str_radix(hex, 16).unwrap_or(0));
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_inspects_uris() {
        let uri = Uri::of("https://laravel.com/docs/installation");
        assert_eq!(uri.scheme(), Some("https"));
        assert_eq!(uri.user(), None);
        assert_eq!(uri.password(), None);
        assert_eq!(uri.host(), Some("laravel.com"));
        assert_eq!(uri.port(), None);
        assert_eq!(uri.path(), "docs/installation");
        assert_eq!(uri.query().all(), json!({}));
        assert_eq!(uri.query().to_string(), "");
        assert_eq!(uri.query().decode(), "");
        assert_eq!(uri.fragment(), None);
        assert_eq!(uri.to_string(), "https://laravel.com/docs/installation");

        let uri = Uri::of("https://taylor:password@laravel.com/docs/installation?version=1#hello");
        assert_eq!(uri.user(), Some("taylor"));
        assert_eq!(uri.password(), Some("password"));
        assert_eq!(uri.fragment(), Some("hello"));
        assert_eq!(uri.query().all(), json!({"version": "1"}));
        assert_eq!(uri.query().integer("version"), 1);
        assert_eq!(uri.authority().as_deref(), Some("taylor:password@laravel.com"));
        assert_eq!(uri.user_info().as_deref(), Some("taylor:password"));
    }

    #[test]
    fn it_handles_empty_uris_and_fragments() {
        assert!(Uri::of("").is_empty());
        assert!(Uri::of("https://laravel.com").is_not_empty());
        let uri = Uri::of("https://laravel.com/docs/installation#introduction");
        let without = uri.clone().without_fragment();
        assert_eq!(without.fragment(), None);
        assert_eq!(without.value(), "https://laravel.com/docs/installation");
        assert_eq!(uri.fragment(), Some("introduction"));
    }

    #[test]
    fn it_parses_complicated_query_strings() {
        let uri = Uri::of(
            "https://example.com/users?key_1=value&key_2[sub_field]=value&key_3[]=value&key_4[9]=value&key_5[][][foo][9]=bar&key.6=value&flag_value",
        );
        assert_eq!(
            uri.query().all(),
            json!({
                "key_1": "value",
                "key_2": {"sub_field": "value"},
                "key_3": ["value"],
                "key_4": {"9": "value"},
                "key_5": [[{"foo": {"9": "bar"}}]],
                "key.6": "value",
                "flag_value": "",
            })
        );
        assert_eq!(
            uri.query().decode(),
            "key_1=value&key_2[sub_field]=value&key_3[]=value&key_4[9]=value&key_5[][][foo][9]=bar&key.6=value&flag_value"
        );
    }

    #[test]
    fn it_builds_uris() {
        let uri = Uri::of("")
            .with_host("laravel.com")
            .with_scheme("https")
            .with_user(Some("taylor"), Some("password"))
            .with_path("/docs/installation")
            .with_port(Some(80))
            .with_query(json!({"version": 1}))
            .with_fragment("hello");
        assert_eq!(uri.to_string(), "https://taylor:password@laravel.com:80/docs/installation?version=1#hello");
        assert_eq!(uri.to_stringable(), uri.value().as_str());
    }

    #[test]
    fn it_manipulates_query_strings() {
        let uri = Uri::of("https://laravel.com")
            .with_query(json!({
                "name": "Taylor",
                "age": 38,
                "role": {"title": "Developer", "focus": "PHP"},
                "tags": ["person", "employee"],
                "flag": "",
            }))
            .without_query(&["name"]);
        assert_eq!(
            uri.query().decode(),
            "age=38&role[title]=Developer&role[focus]=PHP&tags[0]=person&tags[1]=employee&flag="
        );
        assert_eq!(uri.clone().replace_query(json!({"name": "Taylor"})).query().decode(), "name=Taylor");

        let uri = Uri::of("https://laravel.com?tags[]=foo");
        assert_eq!(uri.clone().push_onto_query("tags", "bar").query().all(), json!({"tags": ["foo", "bar"]}));
        assert_eq!(
            uri.clone().push_onto_query("tags", json!(["bar", "baz"])).query().all(),
            json!({"tags": ["foo", "bar", "baz"]})
        );
        assert_eq!(
            uri.push_onto_query("names", "Taylor").query().all(),
            json!({"tags": ["foo"], "names": ["Taylor"]})
        );
        let uri = Uri::of("https://laravel.com?tag=foo");
        assert_eq!(uri.push_onto_query("tag", "bar").query().all(), json!({"tag": ["foo", "bar"]}));
    }

    #[test]
    fn it_merges_dotted_and_literal_keys() {
        let uri = Uri::of("https://dot.test/?foo.bar=baz");
        assert_eq!(uri.clone().with_query(json!({"foo.bar": "zab"})).query().decode(), "foo.bar=baz&foo[bar]=zab");
        assert_eq!(uri.replace_query(json!({"foo.bar": "zab"})).query().decode(), "foo[bar]=zab");
        let uri = Uri::of("https://laravel.com/?role=user&tenant=10");
        assert_eq!(
            uri.with_query(json!({"*": "admin"})).query().all(),
            json!({"role": "user", "tenant": "10", "*": "admin"})
        );
    }

    #[test]
    fn it_adds_missing_query_parameters() {
        let uri = Uri::of("https://laravel.com?existing=value")
            .with_query_if_missing(json!({"new": "parameter", "existing": "new_value"}));
        assert_eq!(uri.query().decode(), "existing=value&new=parameter");

        let uri = Uri::of("https://laravel.com?name=Taylor&tags[0]=person")
            .with_query_if_missing(json!({"name": "Changed", "age": 38, "tags": ["should", "not", "change"]}));
        assert_eq!(uri.query().decode(), "name=Taylor&tags[0]=person&age=38");

        let uri = Uri::of("https://laravel.com?user[name]=Taylor").with_query_if_missing(json!({
            "user": {"name": "Should Not Change", "age": 38},
            "settings": {"theme": "dark"},
        }));
        assert_eq!(uri.query().all(), json!({"user": {"name": "Taylor"}, "settings": {"theme": "dark"}}));
    }

    #[test]
    fn it_decodes_and_segments() {
        let uri = Uri::of("https://laravel.com/docs/11.x/installation").with_query(json!({"tags": ["first", "second"]}));
        assert_eq!(uri.decode(), "https://laravel.com/docs/11.x/installation?tags[0]=first&tags[1]=second");
        let uri = Uri::of("https://laravel.com/docs/11.x/routing?q=laravel%20docs#route-model-binding");
        assert_eq!(uri.decode(), "https://laravel.com/docs/11.x/routing?q=laravel docs#route-model-binding");
        assert_eq!(Uri::of("https://laravel.com").with_query(json!({})).to_string(), "https://laravel.com");
        assert!(Uri::of("https://laravel.com").path_segments().is_empty());
        assert_eq!(Uri::of("https://laravel.com/one/two").path_segments().all(), &["one", "two"]);
        assert_eq!(Uri::of("http://[::1]:8080/x").host(), Some("[::1]"));
        assert_eq!(Uri::of("http://[::1]:8080/x").port(), Some(8080));
        assert_eq!(serde_json::to_string(&Uri::of("https://a.b/c")).unwrap(), "\"https://a.b/c\"");
    }
}
