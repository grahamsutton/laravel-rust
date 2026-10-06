//! The cookie jar and the per-request cookie queue.

use std::sync::{Arc, Mutex, RwLock};

use indexmap::IndexMap;

use illuminate_config::Repository;
use illuminate_http::{Cookie, Request, SameSite, current_request};
use illuminate_support::{Value, ValueExt};

/// The number of minutes in "forever" (400 days — the longest lifetime
/// modern browsers honor).
pub(crate) const FOREVER_MINUTES: i64 = 576_000;

/// The cookies queued for an outgoing response, keyed by name and then path.
///
/// While a request is being handled the queue is stored on the request as
/// an extension, which keeps every request's cookies to itself.
#[derive(Debug, Default)]
pub struct CookieQueue {
    cookies: Mutex<IndexMap<String, IndexMap<String, Cookie>>>,
}

impl CookieQueue {
    /// Create an empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the queue attached to the given request, attaching a new one if
    /// the request doesn't have one yet.
    pub fn for_request(request: &Request) -> Arc<CookieQueue> {
        if let Some(queue) = request.extension::<CookieQueue>() {
            return queue;
        }
        let queue = Arc::new(CookieQueue::new());
        request.set_extension(queue.clone());
        queue
    }

    /// Queue a cookie (replacing any queued cookie with the same name and path).
    pub fn queue(&self, cookie: Cookie) {
        self.cookies
            .lock()
            .unwrap()
            .entry(cookie.name.clone())
            .or_default()
            .insert(cookie.path.clone(), cookie);
    }

    /// Get a queued cookie: the most recently queued one with the given name,
    /// or the one queued for a specific path.
    pub fn queued(&self, name: &str, path: Option<&str>) -> Option<Cookie> {
        let cookies = self.cookies.lock().unwrap();
        let by_path = cookies.get(name)?;
        match path {
            None => by_path.values().last().cloned(),
            Some(path) => by_path.get(path).cloned(),
        }
    }

    /// Remove a cookie from the queue (every path, or a specific one).
    pub fn unqueue(&self, name: &str, path: Option<&str>) {
        let mut cookies = self.cookies.lock().unwrap();
        match path {
            None => {
                cookies.shift_remove(name);
            }
            Some(path) => {
                if let Some(by_path) = cookies.get_mut(name) {
                    by_path.shift_remove(path);
                    if by_path.is_empty() {
                        cookies.shift_remove(name);
                    }
                }
            }
        }
    }

    /// Every queued cookie, in the order they were queued.
    pub fn all(&self) -> Vec<Cookie> {
        self.cookies
            .lock()
            .unwrap()
            .values()
            .flat_map(|by_path| by_path.values().cloned())
            .collect()
    }

    /// Determine if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.lock().unwrap().is_empty()
    }

    /// Remove every queued cookie.
    pub fn flush(&self) {
        self.cookies.lock().unwrap().clear();
    }
}

#[derive(Clone, Debug)]
struct Defaults {
    path: String,
    domain: Option<String>,
    secure: Option<bool>,
    same_site: Option<SameSite>,
}

/// The cookie jar: creates cookies with the application's defaults and
/// queues them for the outgoing response.
///
/// ```
/// use illuminate_cookie::CookieJar;
/// use illuminate_http::SameSite;
///
/// let jar = CookieJar::new();
/// jar.set_default_path_and_domain("/app", Some("example.com"), Some(true), Some(SameSite::Strict));
///
/// let cookie = jar.make("name", "value", 60);
/// assert_eq!(cookie.minutes, Some(60));
/// assert_eq!(cookie.path, "/app");
/// assert_eq!(cookie.domain.as_deref(), Some("example.com"));
/// assert!(cookie.secure);
///
/// // Zero minutes makes a "session" cookie that expires when the browser closes.
/// assert_eq!(jar.make("name", "value", 0).minutes, None);
/// ```
#[derive(Debug)]
pub struct CookieJar {
    defaults: RwLock<Defaults>,
    fallback: Arc<CookieQueue>,
}

impl Default for CookieJar {
    fn default() -> Self {
        Self::new()
    }
}

impl CookieJar {
    /// Create a jar with Laravel's defaults: path `/`, no domain, `Lax` same-site.
    pub fn new() -> Self {
        Self {
            defaults: RwLock::new(Defaults {
                path: "/".to_string(),
                domain: None,
                secure: None,
                same_site: Some(SameSite::Lax),
            }),
            fallback: Arc::new(CookieQueue::new()),
        }
    }

    /// Create a jar using the `session.path`, `session.domain`,
    /// `session.secure` and `session.same_site` configuration values.
    pub fn from_config(config: &Repository) -> Self {
        let jar = Self::new();
        let path = config.string_or("session.path", "/");
        let domain = match config.get("session.domain") {
            Value::Null => None,
            value => Some(value.to_string_lossy()).filter(|d| !d.is_empty()),
        };
        let secure = match config.get("session.secure") {
            Value::Null => None,
            _ => Some(config.boolean("session.secure")),
        };
        let same_site = SameSite::parse(&config.string("session.same_site"));
        jar.set_default_path_and_domain(&path, domain.as_deref(), secure, same_site);
        jar
    }

    /// Set the default path, domain, secure flag, and same-site policy.
    pub fn set_default_path_and_domain(
        &self,
        path: &str,
        domain: Option<&str>,
        secure: Option<bool>,
        same_site: Option<SameSite>,
    ) -> &Self {
        *self.defaults.write().unwrap() = Defaults {
            path: if path.is_empty() { "/".to_string() } else { path.to_string() },
            domain: domain.map(str::to_string),
            secure,
            same_site,
        };
        self
    }

    // ------------------------------------------------------------------
    // Creating cookies
    // ------------------------------------------------------------------

    /// Create a new cookie that lives for the given number of minutes
    /// (`0` creates a session cookie). Customize it further with the
    /// cookie's builder methods (`path`, `http_only`, `raw`, ...).
    pub fn make(&self, name: impl Into<String>, value: impl Into<String>, minutes: i64) -> Cookie {
        let defaults = self.defaults.read().unwrap().clone();
        let secure = defaults
            .secure
            .unwrap_or_else(|| current_request().is_some_and(|request| request.secure()));
        let mut cookie = Cookie::new(name, value)
            .path(defaults.path)
            .secure(secure)
            .same_site(defaults.same_site);
        if let Some(domain) = defaults.domain {
            cookie = cookie.domain(domain);
        }
        if minutes != 0 {
            cookie = cookie.minutes(minutes);
        }
        cookie
    }

    /// Create a cookie that lasts "forever" (400 days).
    pub fn forever(&self, name: impl Into<String>, value: impl Into<String>) -> Cookie {
        self.make(name, value, FOREVER_MINUTES)
    }

    /// Create a cookie that expires the given cookie on the client.
    pub fn forget(&self, name: impl Into<String>) -> Cookie {
        self.make(name, "", -2_628_000)
    }

    /// Create a cookie that expires the given cookie for a specific path and domain.
    pub fn forget_at(&self, name: impl Into<String>, path: Option<&str>, domain: Option<&str>) -> Cookie {
        let mut cookie = self.forget(name);
        if let Some(path) = path {
            cookie = cookie.path(path);
        }
        if let Some(domain) = domain {
            cookie = cookie.domain(domain);
        }
        cookie
    }

    // ------------------------------------------------------------------
    // Queueing
    // ------------------------------------------------------------------

    /// The queue for the current request (or the jar's own queue when no
    /// request is being handled).
    pub fn queue_store(&self) -> Arc<CookieQueue> {
        match current_request() {
            Some(request) => CookieQueue::for_request(&request),
            None => self.fallback.clone(),
        }
    }

    /// Queue a cookie to send with the next response.
    pub fn queue(&self, cookie: Cookie) {
        self.queue_store().queue(cookie);
    }

    /// Create a cookie with the jar's defaults and queue it.
    pub fn queue_make(&self, name: impl Into<String>, value: impl Into<String>, minutes: i64) {
        self.queue(self.make(name, value, minutes));
    }

    /// Queue a cookie on a specific request (rather than the current one).
    pub fn queue_on(&self, request: &Request, cookie: Cookie) {
        CookieQueue::for_request(request).queue(cookie);
    }

    /// Queue a cookie that expires the given cookie on the client.
    pub fn expire(&self, name: impl Into<String>) {
        self.queue(self.forget(name));
    }

    /// Queue a cookie that expires the given cookie for a specific path and domain.
    pub fn expire_at(&self, name: impl Into<String>, path: Option<&str>, domain: Option<&str>) {
        self.queue(self.forget_at(name, path, domain));
    }

    /// Remove a cookie from the queue.
    pub fn unqueue(&self, name: &str) {
        self.queue_store().unqueue(name, None);
    }

    /// Remove a cookie queued for a specific path.
    pub fn unqueue_at(&self, name: &str, path: &str) {
        self.queue_store().unqueue(name, Some(path));
    }

    /// Get a queued cookie by name.
    pub fn queued(&self, name: &str) -> Option<Cookie> {
        self.queue_store().queued(name, None)
    }

    /// Get the cookie queued for a specific name and path.
    pub fn queued_at(&self, name: &str, path: &str) -> Option<Cookie> {
        self.queue_store().queued(name, Some(path))
    }

    /// Determine if a cookie has been queued.
    pub fn has_queued(&self, name: &str) -> bool {
        self.queued(name).is_some()
    }

    /// Determine if a cookie has been queued for a specific path.
    pub fn has_queued_at(&self, name: &str, path: &str) -> bool {
        self.queued_at(name, path).is_some()
    }

    /// Get the cookies queued for the next response.
    pub fn get_queued_cookies(&self) -> Vec<Cookie> {
        self.queue_store().all()
    }

    /// Flush the cookies queued for the next response.
    pub fn flush_queued_cookies(&self) -> &Self {
        self.queue_store().flush();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_http::with_request;
    use illuminate_support::json;

    #[test]
    fn it_makes_cookies_with_defaults() {
        let jar = CookieJar::new();
        let cookie = jar.make("color", "blue", 10);
        assert_eq!(cookie.name, "color");
        assert_eq!(cookie.value, "blue");
        assert_eq!(cookie.minutes, Some(10));
        assert_eq!(cookie.path, "/");
        assert_eq!(cookie.domain, None);
        assert!(!cookie.secure);
        assert!(cookie.http_only);
        assert_eq!(cookie.same_site, Some(SameSite::Lax));

        assert_eq!(jar.forever("color", "blue").minutes, Some(576_000));
        let forgotten = jar.forget("color");
        assert!(forgotten.is_cleared());
        assert_eq!(forgotten.value, "");

        let forgotten = jar.forget_at("color", Some("/admin"), Some("example.com"));
        assert_eq!(forgotten.path, "/admin");
        assert_eq!(forgotten.domain.as_deref(), Some("example.com"));
    }

    #[test]
    fn defaults_come_from_session_configuration() {
        let config = Repository::new(json!({
            "session": {"path": "/app", "domain": "laravel.com", "secure": true, "same_site": "strict"},
        }));
        let cookie = CookieJar::from_config(&config).make("a", "b", 1);
        assert_eq!(cookie.path, "/app");
        assert_eq!(cookie.domain.as_deref(), Some("laravel.com"));
        assert!(cookie.secure);
        assert_eq!(cookie.same_site, Some(SameSite::Strict));

        let config = Repository::new(json!({"session": {"path": "/", "domain": null, "secure": null, "same_site": null}}));
        let cookie = CookieJar::from_config(&config).make("a", "b", 1);
        assert_eq!(cookie.domain, None);
        assert!(!cookie.secure);
        assert_eq!(cookie.same_site, None);
    }

    #[test]
    fn cookies_are_queued_on_the_jar_outside_of_requests() {
        let jar = CookieJar::new();
        jar.queue_make("foo", "bar", 0);
        jar.queue(jar.make("foo", "baz", 0).path("/admin"));
        jar.expire("old");

        assert!(jar.has_queued("foo"));
        assert_eq!(jar.queued("foo").unwrap().value, "baz");
        assert_eq!(jar.queued_at("foo", "/").unwrap().value, "bar");
        assert!(jar.has_queued_at("foo", "/admin"));
        assert!(jar.queued("old").unwrap().is_cleared());
        assert_eq!(jar.get_queued_cookies().len(), 3);

        jar.unqueue_at("foo", "/admin");
        assert_eq!(jar.queued("foo").unwrap().value, "bar");
        jar.unqueue("foo");
        assert!(!jar.has_queued("foo"));
        jar.flush_queued_cookies();
        assert!(jar.get_queued_cookies().is_empty());
    }

    #[tokio::test]
    async fn queued_cookies_belong_to_the_current_request() {
        let jar = Arc::new(CookieJar::new());
        let first = Request::create("/one", "GET");
        let second = Request::create("/two", "GET");

        with_request(first.clone(), async {
            jar.queue_make("first", "1", 5);
        })
        .await;
        with_request(second.clone(), async {
            jar.queue_make("second", "2", 5);
            assert!(!jar.has_queued("first"));
        })
        .await;

        assert!(jar.get_queued_cookies().is_empty());
        let first_queue = first.extension::<CookieQueue>().unwrap();
        assert_eq!(first_queue.all().len(), 1);
        assert_eq!(first_queue.queued("first", None).unwrap().value, "1");
        assert!(second.extension::<CookieQueue>().unwrap().queued("first", None).is_none());

        jar.queue_on(&first, Cookie::new("explicit", "yes"));
        assert_eq!(first_queue.all().len(), 2);
    }

    #[tokio::test]
    async fn secure_follows_the_request_when_not_configured() {
        let jar = CookieJar::new();
        let request = Request::create("https://example.com/", "GET");
        let cookie = with_request(request, async { jar.make("a", "b", 1) }).await;
        assert!(cookie.secure);
    }
}
