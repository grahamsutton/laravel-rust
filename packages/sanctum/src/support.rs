//! Small helpers shared across Sanctum: hashing, checksums, and the bits of
//! PHP semantics the token format relies on.

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use illuminate_support::{Str, Value, ValueExt};

/// A hex-encoded SHA-256 digest (PHP's `hash('sha256', ...)`): how tokens
/// are stored in the `personal_access_tokens` table.
pub(crate) fn sha256(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// The CRC-32 checksum of a string as eight lowercase hex digits (PHP's
/// `hash('crc32b', ...)`), appended to every token so secret scanners can
/// recognize them.
pub(crate) fn crc32b(value: &str) -> String {
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in value.bytes() {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    format!("{:08x}", !crc)
}

/// Compare two strings in constant time (PHP's `hash_equals`).
pub(crate) fn hash_equals(known: &str, user: &str) -> bool {
    known.len() == user.len() && bool::from(known.as_bytes().ct_eq(user.as_bytes()))
}

/// PHP's `empty()` for a string: `""` and `"0"` are empty.
pub(crate) fn php_empty(value: &str) -> bool {
    value.is_empty() || value == "0"
}

/// PHP's `ctype_digit()`: a non-empty string of ASCII digits.
pub(crate) fn ctype_digit(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// A stable string form of a model key (`1`, `"1"` and `1.0` agree).
pub(crate) fn key_string(key: &Value) -> String {
    match key {
        Value::Null => String::new(),
        Value::Number(number) => number
            .as_i64()
            .map(|integer| integer.to_string())
            .or_else(|| number.as_u64().map(|integer| integer.to_string()))
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|float| float.fract() == 0.0)
                    .map(|float| (float as i64).to_string())
            })
            .unwrap_or_else(|| number.to_string()),
        other => other.to_string_lossy(),
    }
}

/// The "class basename" of a configured model name: `App\Models\User`,
/// `crate::models::User` and `User` are all `User`.
pub(crate) fn class_basename(name: &str) -> String {
    Str::class_basename(&name.replace("::", "\\"))
}

/// The `host[:port]` of a URL (PHP's `parse_url` host and port), or `None`
/// when the URL has no host.
pub(crate) fn host_with_port(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url.strip_prefix("//")?,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!host.is_empty()).then(|| host.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn digests_match_php() {
        assert_eq!(
            sha256("secret"),
            "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"
        );
        assert_eq!(crc32b("123456789"), "cbf43926");
        assert_eq!(crc32b(""), "00000000");
        assert_eq!(
            crc32b("The quick brown fox jumps over the lazy dog"),
            "414fa339"
        );
    }

    #[test]
    fn strings_are_compared_in_constant_time() {
        assert!(hash_equals("abc", "abc"));
        assert!(!hash_equals("abc", "abd"));
        assert!(!hash_equals("abc", "abcd"));
    }

    #[test]
    fn php_string_predicates() {
        assert!(php_empty(""));
        assert!(php_empty("0"));
        assert!(!php_empty("00"));
        assert!(ctype_digit("123"));
        assert!(!ctype_digit(""));
        assert!(!ctype_digit("12a"));
    }

    #[test]
    fn keys_have_a_stable_string_form() {
        assert_eq!(key_string(&json!(1)), "1");
        assert_eq!(key_string(&json!("1")), "1");
        assert_eq!(key_string(&json!(1.0)), "1");
        assert_eq!(key_string(&json!("01HV")), "01HV");
        assert_eq!(key_string(&Value::Null), "");
    }

    #[test]
    fn class_basenames_ignore_namespaces() {
        assert_eq!(class_basename("App\\Models\\User"), "User");
        assert_eq!(class_basename("crate::models::User"), "User");
        assert_eq!(class_basename("User"), "User");
    }

    #[test]
    fn hosts_are_parsed_from_urls() {
        assert_eq!(
            host_with_port("http://localhost").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            host_with_port("https://app.test:8443/path?x=1").as_deref(),
            Some("app.test:8443")
        );
        assert_eq!(
            host_with_port("http://user:pass@example.com/").as_deref(),
            Some("example.com")
        );
        assert_eq!(host_with_port("//cdn.test/a").as_deref(), Some("cdn.test"));
        assert_eq!(host_with_port("not a url"), None);
        assert_eq!(host_with_port(""), None);
    }
}
