//! HTTP cookies.

use std::fmt;

use illuminate_support::{Carbon, CarbonInterval};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

/// Characters that must be encoded in a cookie value.
const COOKIE_VALUE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b',')
    .add(b';')
    .add(b'\\')
    .add(b'%');

/// The `SameSite` attribute of a cookie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    Lax,
    Strict,
    None,
}

impl SameSite {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "lax" => Some(Self::Lax),
            "strict" => Some(Self::Strict),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

impl fmt::Display for SameSite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Lax => "Lax",
            Self::Strict => "Strict",
            Self::None => "None",
        })
    }
}

/// An outgoing cookie.
#[derive(Clone, Debug, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Lifetime in minutes; `None` makes a session cookie, `Some(0)` or
    /// negative expires it immediately.
    pub minutes: Option<i64>,
    pub path: String,
    pub domain: Option<String>,
    pub secure: bool,
    pub http_only: bool,
    pub raw: bool,
    pub same_site: Option<SameSite>,
    pub partitioned: bool,
}

impl Cookie {
    /// Create a new session cookie.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            minutes: None,
            path: "/".to_string(),
            domain: None,
            secure: false,
            http_only: true,
            raw: false,
            same_site: Some(SameSite::Lax),
            partitioned: false,
        }
    }

    /// Create a cookie that lasts "forever" (five years).
    pub fn forever(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(name, value).minutes(2_628_000)
    }

    /// Create a cookie that expires the given cookie on the client.
    pub fn forget(name: impl Into<String>) -> Self {
        Self::new(name, "").minutes(-2_628_000)
    }

    pub fn minutes(mut self, minutes: i64) -> Self {
        self.minutes = Some(minutes);
        self
    }

    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    pub fn http_only(mut self, http_only: bool) -> Self {
        self.http_only = http_only;
        self
    }

    pub fn raw(mut self, raw: bool) -> Self {
        self.raw = raw;
        self
    }

    pub fn same_site(mut self, same_site: Option<SameSite>) -> Self {
        self.same_site = same_site;
        self
    }

    pub fn partitioned(mut self, partitioned: bool) -> Self {
        self.partitioned = partitioned;
        self
    }

    /// Determine if the cookie clears itself on the client.
    pub fn is_cleared(&self) -> bool {
        self.minutes.is_some_and(|m| m <= 0)
    }

    /// The expiry moment, if this isn't a session cookie.
    pub fn expires_at(&self) -> Option<Carbon> {
        self.minutes
            .map(|m| Carbon::now().add(CarbonInterval::minutes(m)))
    }

    /// Render the cookie as a `Set-Cookie` header value.
    pub fn to_header_value(&self) -> String {
        let value = if self.raw {
            self.value.clone()
        } else {
            utf8_percent_encode(&self.value, COOKIE_VALUE).to_string()
        };
        let mut header = format!("{}={}", self.name, value);
        if let Some(minutes) = self.minutes {
            let expires = if minutes <= 0 {
                Carbon::from_timestamp(0)
            } else {
                Carbon::now().add_minutes(minutes)
            };
            header.push_str(&format!("; expires={}", expires.to_cookie_string()));
            header.push_str(&format!("; Max-Age={}", (minutes * 60).max(0)));
        }
        if !self.path.is_empty() {
            header.push_str(&format!("; path={}", self.path));
        }
        if let Some(domain) = &self.domain {
            header.push_str(&format!("; domain={domain}"));
        }
        if self.secure {
            header.push_str("; secure");
        }
        if self.http_only {
            header.push_str("; httponly");
        }
        if let Some(same_site) = self.same_site {
            header.push_str(&format!("; samesite={}", same_site.to_string().to_lowercase()));
        }
        if self.partitioned {
            header.push_str("; partitioned");
        }
        header
    }
}

/// Create a new cookie that lives for the given number of minutes.
///
/// ```
/// use illuminate_http::cookie;
///
/// let cookie = cookie("name", "value", 60);
/// assert_eq!(cookie.minutes, Some(60));
/// ```
pub fn cookie(name: impl Into<String>, value: impl Into<String>, minutes: i64) -> Cookie {
    Cookie::new(name, value).minutes(minutes)
}

/// Parse a `Cookie` request header into name/value pairs.
pub fn parse_cookie_header(header: &str) -> Vec<(String, String)> {
    header
        .split(';')
        .filter_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            let value = value.trim().trim_matches('"');
            let decoded = percent_encoding::percent_decode_str(value)
                .decode_utf8_lossy()
                .into_owned();
            Some((name.trim().to_string(), decoded))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_renders_set_cookie_headers() {
        let header = Cookie::new("laravel_session", "abc 123").to_header_value();
        assert_eq!(header, "laravel_session=abc%20123; path=/; httponly; samesite=lax");

        let forgotten = Cookie::forget("name").to_header_value();
        assert!(forgotten.contains("expires=Thu, 01 Jan 1970 00:00:00 GMT"));
        assert!(forgotten.contains("Max-Age=0"));
    }

    #[test]
    fn it_parses_cookie_headers() {
        let cookies = parse_cookie_header("a=1; b=hello%20world; c=\"quoted\"");
        assert_eq!(cookies[1], ("b".to_string(), "hello world".to_string()));
        assert_eq!(cookies[2].1, "quoted");
    }
}
