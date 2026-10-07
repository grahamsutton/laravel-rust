//! Small helpers shared by the providers: PHP's `http_build_query`, lenient
//! value access, and HMAC signatures.

use illuminate_support::{Map, Value, ValueExt};
use sha2::{Digest, Sha256};

/// How the query string of an authorization URL is encoded — PHP's
/// `PHP_QUERY_RFC1738` (the default) and `PHP_QUERY_RFC3986`.
///
/// ```
/// use laravel_socialite::QueryEncoding;
/// use illuminate_support::json;
///
/// let fields = json!({"scope": "users.read tweet.read", "redirect_uri": "https://laravel.test/~taylor"});
/// let fields = fields.as_object().unwrap();
///
/// assert_eq!(
///     QueryEncoding::Rfc1738.build_query(fields),
///     "scope=users.read+tweet.read&redirect_uri=https%3A%2F%2Flaravel.test%2F%7Etaylor",
/// );
/// assert_eq!(
///     QueryEncoding::Rfc3986.build_query(fields),
///     "scope=users.read%20tweet.read&redirect_uri=https%3A%2F%2Flaravel.test%2F~taylor",
/// );
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QueryEncoding {
    /// `application/x-www-form-urlencoded` style: spaces become `+` (PHP's
    /// `urlencode`).
    #[default]
    Rfc1738,
    /// RFC 3986 percent-encoding: spaces become `%20` and `~` is left alone
    /// (PHP's `rawurlencode`).
    Rfc3986,
}

impl QueryEncoding {
    /// Build a query string from the given fields, like PHP's
    /// `http_build_query($fields, '', '&', $encoding)`: `null` values are
    /// skipped, booleans become `1` / `0`, and nested values use brackets.
    pub fn build_query(self, fields: &Map<String, Value>) -> String {
        let mut pairs = Vec::new();
        for (key, value) in fields {
            flatten(key.clone(), value, &mut pairs);
        }
        pairs
            .into_iter()
            .map(|(key, value)| format!("{}={}", self.encode(&key), self.encode(&value)))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Percent-encode a single component.
    pub fn encode(self, value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        for byte in value.bytes() {
            match (self, byte) {
                (_, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.') => {
                    out.push(byte as char)
                }
                (QueryEncoding::Rfc3986, b'~') => out.push('~'),
                (QueryEncoding::Rfc1738, b' ') => out.push('+'),
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
        out
    }
}

fn flatten(key: String, value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Null => {}
        Value::Bool(flag) => out.push((key, if *flag { "1" } else { "0" }.to_string())),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                flatten(format!("{key}[{index}]"), item, out);
            }
        }
        Value::Object(map) => {
            for (name, item) in map {
                flatten(format!("{key}[{name}]"), item, out);
            }
        }
        other => out.push((key, other.to_string_lossy())),
    }
}

/// A value as an optional string: `null` (or a missing value) is `None`,
/// scalars are converted the way PHP casts them.
pub(crate) fn optional_string(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(_) | Value::Object(_)) => None,
        Some(other) => Some(other.to_string_lossy()),
    }
}

/// Laravel's `Arr::get($array, $key)`: the exact key first, then "dot"
/// notation (so `https://slack.com/team_id` and `user.image_512` both work).
pub(crate) fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.dot(key).filter(|value| !value.is_null())
}

/// `Arr::get($array, $key)` as an owned value (`null` when missing).
pub(crate) fn get_value(value: &Value, key: &str) -> Value {
    get(value, key).cloned().unwrap_or(Value::Null)
}

/// `Arr::get($array, $key)` as an optional string.
pub(crate) fn get_string(value: &Value, key: &str) -> Option<String> {
    optional_string(get(value, key))
}

/// A list of strings from a string (split on the separator) or an array.
pub(crate) fn string_list(value: Option<&Value>, separator: &str) -> Vec<String> {
    let items: Vec<String> = match value {
        Some(Value::String(text)) if separator.is_empty() => vec![text.clone()],
        Some(Value::String(text)) => text.split(separator).map(str::to_string).collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter(|item| !item.is_null())
            .map(ValueExt::to_string_lossy)
            .collect(),
        _ => Vec::new(),
    };
    items.into_iter().filter(|item| !item.is_empty()).collect()
}

/// Remove duplicate strings, keeping the first occurrence of each (PHP's
/// `array_values(array_unique(...))`).
pub(crate) fn unique(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for item in items {
        if !unique.contains(&item) {
            unique.push(item);
        }
    }
    unique
}

/// PHP's `hash_hmac('sha256', $message, $key)`: a lowercase hex HMAC-SHA256.
pub(crate) fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;

    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let inner_pad: Vec<u8> = block.iter().map(|byte| byte ^ 0x36).collect();
    let outer_pad: Vec<u8> = block.iter().map(|byte| byte ^ 0x5c).collect();

    let inner = Sha256::new()
        .chain_update(&inner_pad)
        .chain_update(message)
        .finalize();
    let outer = Sha256::new()
        .chain_update(&outer_pad)
        .chain_update(inner)
        .finalize();

    outer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The short name of a type (`GithubProvider`), for messages.
pub(crate) fn short_type_name<T: ?Sized>() -> &'static str {
    let full = std::any::type_name::<T>();
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn queries_are_built_like_php() {
        let fields = map(json!({
            "client_id": "abc",
            "redirect_uri": "http://localhost/callback?x=1",
            "scope": "user:email read:org",
            "state": null,
            "flag": true,
            "off": false,
            "count": 2,
        }));
        assert_eq!(
            QueryEncoding::Rfc1738.build_query(&fields),
            "client_id=abc&redirect_uri=http%3A%2F%2Flocalhost%2Fcallback%3Fx%3D1&scope=user%3Aemail+read%3Aorg&flag=1&off=0&count=2"
        );
        assert_eq!(
            QueryEncoding::Rfc3986.build_query(&map(json!({"scope": "a b~c"}))),
            "scope=a%20b~c"
        );
    }

    #[test]
    fn nested_values_use_brackets() {
        let fields = map(json!({"prompt": ["consent", "select_account"], "a": {"b": 1}}));
        assert_eq!(
            QueryEncoding::Rfc1738.build_query(&fields),
            "prompt%5B0%5D=consent&prompt%5B1%5D=select_account&a%5Bb%5D=1"
        );
    }

    #[test]
    fn values_are_read_like_arr_get() {
        let value = json!({
            "user": {"image_512": "https://a.test/512.png"},
            "https://slack.com/team_id": "T1",
            "empty": null,
            "id": 42,
        });
        assert_eq!(
            get_string(&value, "user.image_512").as_deref(),
            Some("https://a.test/512.png")
        );
        assert_eq!(
            get_string(&value, "https://slack.com/team_id").as_deref(),
            Some("T1")
        );
        assert_eq!(get_string(&value, "empty"), None);
        assert_eq!(get_string(&value, "missing"), None);
        assert_eq!(get_string(&value, "id").as_deref(), Some("42"));
        assert_eq!(get_value(&value, "missing"), Value::Null);
    }

    #[test]
    fn string_lists_split_and_skip_blanks() {
        assert_eq!(string_list(Some(&json!("a,b")), ","), ["a", "b"]);
        assert_eq!(string_list(Some(&json!("")), ","), Vec::<String>::new());
        assert_eq!(string_list(Some(&json!(["x", null, "y"])), " "), ["x", "y"]);
        assert_eq!(string_list(None, ","), Vec::<String>::new());
        assert_eq!(unique(["a".into(), "b".into(), "a".into()]), ["a", "b"]);
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Keys longer than the block size are hashed first (RFC 4231, case 6).
        assert_eq!(
            hmac_sha256_hex(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            ),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn short_type_names_drop_the_path() {
        assert_eq!(short_type_name::<Vec<String>>(), "Vec");
        assert_eq!(short_type_name::<QueryEncoding>(), "QueryEncoding");
    }
}
