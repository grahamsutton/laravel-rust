//! A small cookie jar shared by the requests of a single transfer.
//!
//! Cookies given to [`PendingRequest::with_cookies`](crate::PendingRequest::with_cookies)
//! are sent to matching hosts, and cookies set by the server (including on
//! redirect responses) are remembered and sent along on the next hop. The jar
//! is available afterwards through [`Response::cookies`](crate::Response::cookies).

use url::Url;

/// A single HTTP cookie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cookie {
    /// The cookie's name.
    pub name: String,
    /// The cookie's value.
    pub value: String,
    /// The domain the cookie belongs to.
    pub domain: String,
    /// The path the cookie is restricted to.
    pub path: String,
    /// The raw `Expires` attribute, if one was given.
    pub expires: Option<String>,
    /// The `Max-Age` attribute, in seconds, if one was given.
    pub max_age: Option<i64>,
    /// Whether the cookie may only be sent over HTTPS.
    pub secure: bool,
    /// Whether the cookie is hidden from client-side scripts.
    pub http_only: bool,
}

impl Cookie {
    /// Create a new cookie for the given domain.
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        domain: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            domain: domain.into().trim_start_matches('.').to_lowercase(),
            path: "/".to_string(),
            expires: None,
            max_age: None,
            secure: false,
            http_only: false,
        }
    }

    /// Parse a `Set-Cookie` header received from the given URL.
    pub fn parse(header: &str, url: &Url) -> Option<Self> {
        let mut attributes = header.split(';');
        let (name, value) = attributes.next()?.split_once('=')?;
        let name = name.trim();

        if name.is_empty() {
            return None;
        }

        let mut cookie = Cookie::new(
            name,
            value.trim().trim_matches('"'),
            url.host_str().unwrap_or(""),
        );
        cookie.path = default_path(url);

        for attribute in attributes {
            let (key, value) = match attribute.split_once('=') {
                Some((key, value)) => (key.trim(), value.trim()),
                None => (attribute.trim(), ""),
            };

            match key.to_ascii_lowercase().as_str() {
                "domain" if !value.is_empty() => {
                    cookie.domain = value.trim_start_matches('.').to_lowercase();
                }
                "path" if value.starts_with('/') => cookie.path = value.to_string(),
                "expires" => cookie.expires = Some(value.to_string()),
                "max-age" => cookie.max_age = value.parse().ok(),
                "secure" => cookie.secure = true,
                "httponly" => cookie.http_only = true,
                _ => {}
            }
        }

        Some(cookie)
    }

    /// Determine if the cookie should be sent to the given URL.
    pub fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or("").to_lowercase();
        let host = host.trim_start_matches('[').trim_end_matches(']');

        let domain_matches = self.domain.is_empty()
            || host == self.domain
            || host.ends_with(&format!(".{}", self.domain));

        let path = url.path();
        let path_matches = path == self.path
            || (path.starts_with(&self.path)
                && (self.path.ends_with('/') || path[self.path.len()..].starts_with('/')));

        domain_matches && path_matches && (!self.secure || url.scheme() == "https")
    }

    /// Determine if the cookie has been expired by the server.
    pub fn is_expired(&self) -> bool {
        self.max_age.is_some_and(|age| age <= 0)
    }
}

/// The default path of a cookie set by the given URL (RFC 6265, 5.1.4).
fn default_path(url: &Url) -> String {
    let path = url.path();
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => path[..index].to_string(),
    }
}

/// A collection of cookies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    /// Create an empty cookie jar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cookie jar from name / value pairs for the given domain.
    ///
    /// ```
    /// use illuminate_http_client::CookieJar;
    ///
    /// let jar = CookieJar::from_array([("session", "abc")], "laravel.com");
    /// assert_eq!(jar.get("session").unwrap().value, "abc");
    /// ```
    pub fn from_array(
        cookies: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
        domain: &str,
    ) -> Self {
        let mut jar = Self::new();
        for (name, value) in cookies {
            jar.set(Cookie::new(name, value, domain));
        }
        jar
    }

    /// Add (or replace) a cookie in the jar.
    pub fn set(&mut self, cookie: Cookie) {
        self.cookies.retain(|existing| {
            !(existing.name == cookie.name
                && existing.domain == cookie.domain
                && existing.path == cookie.path)
        });

        if !cookie.is_expired() {
            self.cookies.push(cookie);
        }
    }

    /// Get a cookie by its name.
    pub fn get(&self, name: &str) -> Option<&Cookie> {
        self.cookies.iter().find(|cookie| cookie.name == name)
    }

    /// Get the value of the named cookie.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.get(name).map(|cookie| cookie.value.as_str())
    }

    /// Iterate over every cookie in the jar.
    pub fn iter(&self) -> std::slice::Iter<'_, Cookie> {
        self.cookies.iter()
    }

    /// The number of cookies in the jar.
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Determine if the jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Get all of the cookies as a vector.
    pub fn to_array(&self) -> Vec<Cookie> {
        self.cookies.clone()
    }

    /// Remember the cookies set by a response received from the given URL.
    pub(crate) fn extract<'a>(&mut self, headers: impl IntoIterator<Item = &'a str>, url: &Url) {
        for header in headers {
            if let Some(cookie) = Cookie::parse(header, url) {
                self.set(cookie);
            }
        }
    }

    /// The `Cookie` header value for a request to the given URL, if any.
    pub(crate) fn header_for(&self, url: &Url) -> Option<String> {
        let pairs: Vec<String> = self
            .cookies
            .iter()
            .filter(|cookie| cookie.matches(url))
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect();

        (!pairs.is_empty()).then(|| pairs.join("; "))
    }
}

impl<'a> IntoIterator for &'a CookieJar {
    type Item = &'a Cookie;
    type IntoIter = std::slice::Iter<'a, Cookie>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).unwrap()
    }

    #[test]
    fn it_parses_set_cookie_headers() {
        let cookie = Cookie::parse(
            "session=abc123; Path=/app; Domain=.laravel.com; Secure; HttpOnly; Max-Age=3600",
            &url("https://laravel.com/login"),
        )
        .unwrap();

        assert_eq!(cookie.name, "session");
        assert_eq!(cookie.value, "abc123");
        assert_eq!(cookie.domain, "laravel.com");
        assert_eq!(cookie.path, "/app");
        assert_eq!(cookie.max_age, Some(3600));
        assert!(cookie.secure && cookie.http_only);
    }

    #[test]
    fn cookies_default_to_the_request_host_and_directory() {
        let cookie = Cookie::parse("a=b", &url("http://example.com/users/1")).unwrap();
        assert_eq!(cookie.domain, "example.com");
        assert_eq!(cookie.path, "/users");
        assert!(Cookie::parse("=b", &url("http://example.com")).is_none());
        assert!(Cookie::parse("garbage", &url("http://example.com")).is_none());
    }

    #[test]
    fn cookies_match_domains_paths_and_schemes() {
        let mut cookie = Cookie::new("a", "b", "laravel.com");
        assert!(cookie.matches(&url("http://laravel.com/")));
        assert!(cookie.matches(&url("http://api.laravel.com/users")));
        assert!(!cookie.matches(&url("http://notlaravel.com/")));

        cookie.path = "/docs".into();
        assert!(cookie.matches(&url("http://laravel.com/docs")));
        assert!(cookie.matches(&url("http://laravel.com/docs/13.x")));
        assert!(!cookie.matches(&url("http://laravel.com/documentation")));

        cookie.secure = true;
        assert!(!cookie.matches(&url("http://laravel.com/docs")));
        assert!(cookie.matches(&url("https://laravel.com/docs")));
    }

    #[test]
    fn the_jar_replaces_and_expires_cookies() {
        let mut jar = CookieJar::from_array([("a", "1"), ("b", "2")], "laravel.com");
        let target = url("https://laravel.com/");
        assert_eq!(jar.header_for(&target).unwrap(), "a=1; b=2");

        jar.extract(["a=3", "b=; Max-Age=0"], &target);
        assert_eq!(jar.len(), 1);
        assert_eq!(jar.value("a"), Some("3"));
        assert!(jar.get("b").is_none());
        assert_eq!(jar.header_for(&url("https://forge.com/")), None);
        assert_eq!((&jar).into_iter().count(), 1);
        assert_eq!(jar.to_array()[0].name, "a");
    }
}
