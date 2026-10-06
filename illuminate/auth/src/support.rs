//! Small helpers shared by the guards, providers, and password brokers.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use illuminate_http::Request;
use illuminate_support::{Map, Value, ValueExt};

/// Compare two strings in constant time (PHP's `hash_equals`).
pub(crate) fn hash_equals(known: &str, user: &str) -> bool {
    known.len() == user.len() && bool::from(known.as_bytes().ct_eq(user.as_bytes()))
}

/// A hex-encoded HMAC-SHA256 of `value` (PHP's `hash_hmac('sha256', ...)`).
pub(crate) fn hmac_sha256(value: &str, key: &[u8]) -> String {
    // HMAC accepts keys of any length, so this never fails.
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(value.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// A hex-encoded SHA-256 digest (PHP's `hash('sha256', ...)`).
pub(crate) fn sha256(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// A hex-encoded SHA-1 digest (PHP's `sha1()`).
pub(crate) fn sha1(value: &str) -> String {
    use sha1::Sha1;
    hex::encode(Sha1::digest(value.as_bytes()))
}

/// Decode an application key the way Laravel does (`base64:` prefixed keys
/// are decoded, anything else is used as-is).
pub(crate) fn decode_app_key(key: &str) -> Vec<u8> {
    match key.strip_prefix("base64:") {
        Some(encoded) => STANDARD
            .decode(encoded)
            .unwrap_or_else(|_| key.as_bytes().to_vec()),
        None => key.as_bytes().to_vec(),
    }
}

/// The username and password sent with HTTP Basic authentication.
pub(crate) fn basic_credentials(request: &Request) -> Option<(String, String)> {
    let header = request.header("authorization")?;
    let (scheme, encoded) = header.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = STANDARD.decode(encoded.trim()).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (user, password) = decoded.split_once(':').unwrap_or((decoded.as_str(), ""));
    Some((user.to_string(), password.to_string()))
}

/// Loosely compare two values the way a database `where` clause (or PHP's
/// `==`) would: `1`, `1.0` and `"1"` are all equal.
pub(crate) fn loosely_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Bool(x), other) | (other, Value::Bool(x)) => *x == other.truthy(),
        (Value::Number(_), _) | (_, Value::Number(_)) => match (a.to_f64_lossy(), b.to_f64_lossy()) {
            (Some(x), Some(y)) => x == y,
            _ => a.to_string_lossy() == b.to_string_lossy(),
        },
        _ => a.to_string_lossy() == b.to_string_lossy() && a.is_string() == b.is_string(),
    }
}

/// Remove every credential whose key contains "password" — passwords are
/// never used to *find* users, only to validate them.
pub(crate) fn without_passwords(credentials: &Value) -> Map<String, Value> {
    match credentials {
        Value::Object(map) => map
            .iter()
            .filter(|(key, _)| !key.contains("password"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        _ => Map::new(),
    }
}

/// The plain-text password among the credentials, if there is one.
pub(crate) fn plain_password(credentials: &Value) -> Option<String> {
    match credentials.get("password") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.to_string_lossy()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_hashes_like_php() {
        assert_eq!(
            sha1("Illuminate\\Auth\\SessionGuard"),
            "59ba36addc2b2f9401580f014c7f58ea4e30989d"
        );
        assert_eq!(
            sha256("secret"),
            "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"
        );
        assert_eq!(hmac_sha256("value", b"key").len(), 64);
        assert!(hash_equals("abc", "abc"));
        assert!(!hash_equals("abc", "abd"));
        assert!(!hash_equals("abc", "ab"));
    }

    #[test]
    fn values_compare_loosely() {
        assert!(loosely_equal(&json!(1), &json!("1")));
        assert!(loosely_equal(&json!("taylor@laravel.com"), &json!("taylor@laravel.com")));
        assert!(!loosely_equal(&json!("1"), &json!(2)));
        assert!(loosely_equal(&json!(true), &json!(1)));
        assert!(!loosely_equal(&json!(null), &json!("")));
    }

    #[test]
    fn credentials_never_search_by_password() {
        let credentials = json!({"email": "taylor@laravel.com", "password": "secret", "password_confirmation": "secret"});
        let filtered = without_passwords(&credentials);
        assert_eq!(filtered.len(), 1);
        assert_eq!(plain_password(&credentials).as_deref(), Some("secret"));
    }

    #[test]
    fn basic_credentials_are_decoded() {
        let request = Request::create("/", "GET");
        request.set_header("authorization", "Basic dGF5bG9yQGxhcmF2ZWwuY29tOnNlY3JldA==");
        assert_eq!(
            basic_credentials(&request),
            Some(("taylor@laravel.com".to_string(), "secret".to_string()))
        );
        assert_eq!(decode_app_key("base64:AAAA"), vec![0, 0, 0]);
        assert_eq!(decode_app_key("plain"), b"plain".to_vec());
    }
}
