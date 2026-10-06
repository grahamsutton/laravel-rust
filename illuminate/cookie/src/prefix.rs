//! The cookie value prefix that binds an encrypted cookie to its name.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;

/// Laravel 9+ prefixes every encrypted cookie value with
/// `hash_hmac('sha1', $cookieName.'v2', $key) . '|'`, so an encrypted value
/// can't be copied from one cookie into another.
///
/// ```
/// use illuminate_cookie::CookieValuePrefix;
///
/// let key = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// let prefix = CookieValuePrefix::create("theme", key);
/// assert_eq!(prefix, "56fe61d00b49b0873109b316cf20235a348fc81d|");
///
/// let value = format!("{prefix}dark");
/// assert_eq!(CookieValuePrefix::validate("theme", &value, &[key]).as_deref(), Some("dark"));
/// assert_eq!(CookieValuePrefix::validate("other", &value, &[key]), None);
/// ```
pub struct CookieValuePrefix;

impl CookieValuePrefix {
    /// Create a new cookie value prefix for the given cookie name.
    pub fn create(cookie_name: &str, key: &[u8]) -> String {
        let mut mac =
            <Hmac<Sha1> as Mac>::new_from_slice(key).expect("HMAC accepts keys of any length");
        mac.update(cookie_name.as_bytes());
        mac.update(b"v2");
        format!("{}|", hex::encode(mac.finalize().into_bytes()))
    }

    /// Remove the cookie value prefix.
    pub fn remove(cookie_value: &str) -> String {
        cookie_value.get(41..).unwrap_or_default().to_string()
    }

    /// Validate that a cookie value carries a valid prefix for one of the
    /// given keys, returning the value with the prefix removed.
    pub fn validate<K: AsRef<[u8]>>(
        cookie_name: &str,
        cookie_value: &str,
        keys: &[K],
    ) -> Option<String> {
        keys.iter().find_map(|key| {
            let prefix = Self::create(cookie_name, key.as_ref());
            let candidate = cookie_value.as_bytes().get(..prefix.len())?;
            bool::from(candidate.ct_eq(prefix.as_bytes())).then(|| Self::remove(cookie_value))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_match_laravel() {
        // hash_hmac('sha1', 'theme'.'v2', str_repeat('a', 32)) computed by PHP.
        assert_eq!(
            CookieValuePrefix::create("theme", b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "56fe61d00b49b0873109b316cf20235a348fc81d|"
        );
        assert_eq!(CookieValuePrefix::create("theme", b"k").len(), 41);
    }

    #[test]
    fn prefixes_can_be_validated_with_previous_keys() {
        let old: &[u8] = b"old-key";
        let new: &[u8] = b"new-key";
        let value = format!("{}blue", CookieValuePrefix::create("color", old));
        assert_eq!(
            CookieValuePrefix::validate("color", &value, &[new, old]).as_deref(),
            Some("blue")
        );
        assert_eq!(CookieValuePrefix::validate("color", &value, &[new]), None);
        assert_eq!(CookieValuePrefix::validate("color", "short", &[new]), None);
        assert_eq!(CookieValuePrefix::remove("short"), "");
    }
}
