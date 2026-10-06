//! Make HTTP requests to your application in tests.

use std::sync::Arc;

use indexmap::IndexMap;

use illuminate_container::{Container, LocalInstanceGuard};
use illuminate_cookie::CookieValuePrefix;
use illuminate_http::{HeaderMap, HeaderName, HeaderValue, Request};
use illuminate_session::SessionManager;
use illuminate_support::{Map, Result, Str, Value, json};

use crate::application::Application;
use crate::builder::ApplicationBuilder;
use crate::testing::TestResponse;

/// A test client for your application — Laravel's `TestCase` HTTP helpers.
///
/// ```ignore
/// #[tokio::test]
/// async fn the_application_returns_a_successful_response() {
///     let mut app = TestApp::new(bootstrap::app());
///
///     app.get("/").await.assert_ok();
/// }
/// ```
///
/// Cookies set by responses are remembered and sent with subsequent
/// requests, so sessions carry across requests just like a browser.
pub struct TestApp {
    app: Arc<Application>,
    headers: HeaderMap,
    cookies: IndexMap<String, String>,
    unencrypted_cookies: IndexMap<String, String>,
    pending_session: Map<String, Value>,
    follow_redirects: bool,
    _guard: LocalInstanceGuard,
}

impl TestApp {
    /// Create the application from its builder, configured for testing.
    pub fn new(builder: ApplicationBuilder) -> Self {
        Self::from_application(builder.create())
    }

    /// Wrap an application that was already created.
    pub fn from_application(app: Arc<Application>) -> Self {
        let guard = Container::set_local_instance(app.container().clone());

        // The equivalent of Laravel's phpunit.xml environment.
        app.override_config("app.env", "testing");
        app.override_config("session.driver", "array");
        app.override_config("cache.default", "array");
        app.override_config("queue.default", "sync");
        app.override_config("mail.default", "array");
        app.override_config("hashing.bcrypt.rounds", 4);
        app.override_config("logging.default", "null");
        app.set_running_in_console(false);
        app.bootstrap();

        if app.config_repository().get("app.key").is_null()
            || app.config_repository().string("app.key").is_empty()
        {
            app.override_config("app.key", format!("base64:{}", base64_key()));
        }

        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("localhost"));

        Self {
            app,
            headers,
            cookies: IndexMap::new(),
            unencrypted_cookies: IndexMap::new(),
            pending_session: Map::new(),
            follow_redirects: false,
            _guard: guard,
        }
    }

    /// The application under test.
    pub fn app(&self) -> &Arc<Application> {
        &self.app
    }

    /// Run an Artisan command: `app.artisan("migrate").assert_successful().await;`
    pub fn artisan(&self, command: &str) -> illuminate_console::PendingCommand {
        self.app.bootstrap_console();
        illuminate_console::testing::artisan(command)
    }

    // ------------------------------------------------------------------
    // Request configuration
    // ------------------------------------------------------------------

    /// Add a header to every subsequent request.
    pub fn with_header(&mut self, name: &str, value: &str) -> &mut Self {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::try_from(value)) {
            self.headers.insert(name, value);
        }
        self
    }

    /// Add headers to every subsequent request.
    pub fn with_headers<'a>(&mut self, headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> &mut Self {
        for (name, value) in headers {
            self.with_header(name, value);
        }
        self
    }

    /// Send a bearer token with subsequent requests.
    pub fn with_token(&mut self, token: &str) -> &mut Self {
        self.with_header("Authorization", &format!("Bearer {token}"))
    }

    /// Remove the authorization header.
    pub fn without_token(&mut self) -> &mut Self {
        self.headers.remove("authorization");
        self
    }

    /// Remove every custom header.
    pub fn flush_headers(&mut self) -> &mut Self {
        self.headers = HeaderMap::new();
        self.headers.insert("host", HeaderValue::from_static("localhost"));
        self
    }

    /// Send an (encrypted) cookie with subsequent requests.
    pub fn with_cookie(&mut self, name: &str, value: &str) -> &mut Self {
        if let Some(encrypted) = encrypt_cookie(name, value) {
            self.cookies.insert(name.to_string(), encrypted);
        }
        self
    }

    /// Send a plain, unencrypted cookie with subsequent requests.
    pub fn with_unencrypted_cookie(&mut self, name: &str, value: &str) -> &mut Self {
        self.unencrypted_cookies.insert(name.to_string(), value.to_string());
        self
    }

    /// Forget every remembered cookie.
    pub fn flush_cookies(&mut self) -> &mut Self {
        self.cookies.clear();
        self.unencrypted_cookies.clear();
        self
    }

    /// Put data into the session for the next request.
    pub fn with_session(&mut self, data: Value) -> &mut Self {
        if let Value::Object(map) = data {
            self.pending_session.extend(map);
        }
        self
    }

    /// Set the referring URL (and the session's previous URL).
    pub fn from(&mut self, url: &str) -> &mut Self {
        let url = if url.starts_with("http") { url.to_string() } else { format!("http://localhost/{}", url.trim_start_matches('/')) };
        self.pending_session
            .insert("_previous".into(), json!({ "url": url.clone() }));
        self.with_header("referer", &url)
    }

    /// Automatically follow redirects.
    pub fn following_redirects(&mut self) -> &mut Self {
        self.follow_redirects = true;
        self
    }

    /// Stop following redirects.
    pub fn without_following_redirects(&mut self) -> &mut Self {
        self.follow_redirects = false;
        self
    }

    // ------------------------------------------------------------------
    // Requests
    // ------------------------------------------------------------------

    pub async fn get(&mut self, uri: &str) -> TestResponse {
        self.call("GET", uri, Value::Null, false).await
    }

    pub async fn get_json(&mut self, uri: &str) -> TestResponse {
        self.call("GET", uri, Value::Null, true).await
    }

    pub async fn post(&mut self, uri: &str, data: Value) -> TestResponse {
        self.call("POST", uri, data, false).await
    }

    pub async fn post_json(&mut self, uri: &str, data: Value) -> TestResponse {
        self.call("POST", uri, data, true).await
    }

    pub async fn put(&mut self, uri: &str, data: Value) -> TestResponse {
        self.call("PUT", uri, data, false).await
    }

    pub async fn put_json(&mut self, uri: &str, data: Value) -> TestResponse {
        self.call("PUT", uri, data, true).await
    }

    pub async fn patch(&mut self, uri: &str, data: Value) -> TestResponse {
        self.call("PATCH", uri, data, false).await
    }

    pub async fn patch_json(&mut self, uri: &str, data: Value) -> TestResponse {
        self.call("PATCH", uri, data, true).await
    }

    pub async fn delete(&mut self, uri: &str, data: Value) -> TestResponse {
        self.call("DELETE", uri, data, false).await
    }

    pub async fn delete_json(&mut self, uri: &str, data: Value) -> TestResponse {
        self.call("DELETE", uri, data, true).await
    }

    pub async fn options(&mut self, uri: &str) -> TestResponse {
        self.call("OPTIONS", uri, Value::Null, false).await
    }

    pub async fn head(&mut self, uri: &str) -> TestResponse {
        self.call("HEAD", uri, Value::Null, false).await
    }

    /// Make a JSON request with the given method.
    pub async fn json(&mut self, method: &str, uri: &str, data: Value) -> TestResponse {
        self.call(method, uri, data, true).await
    }

    /// Make a request and return the response.
    pub async fn call(&mut self, method: &str, uri: &str, data: Value, json: bool) -> TestResponse {
        let mut response = self.send(method, uri, data, json).await;
        let mut redirects = 0;
        while self.follow_redirects && response.response().is_redirect() && redirects < 10 {
            let location = response.header("location").unwrap_or_else(|| "/".into());
            let path = location
                .strip_prefix("http://localhost")
                .or_else(|| location.strip_prefix("https://localhost"))
                .unwrap_or(&location)
                .to_string();
            response = self.send("GET", &path, Value::Null, json).await;
            redirects += 1;
        }
        response
    }

    async fn send(&mut self, method: &str, uri: &str, data: Value, json: bool) -> TestResponse {
        if !self.pending_session.is_empty() {
            if let Err(error) = self.start_session_with_pending_data().await {
                panic!("Unable to seed the session: {error}");
            }
        }

        let mut headers = self.headers.clone();
        if json {
            headers.insert("content-type", HeaderValue::from_static("application/json"));
            headers.insert("accept", HeaderValue::from_static("application/json"));
        }
        let cookie_header: Vec<String> = self
            .cookies
            .iter()
            .chain(self.unencrypted_cookies.iter())
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        if !cookie_header.is_empty() {
            if let Ok(value) = HeaderValue::try_from(cookie_header.join("; ")) {
                headers.insert("cookie", value);
            }
        }

        let uri = if uri.starts_with('/') || uri.starts_with("http") { uri.to_string() } else { format!("/{uri}") };
        let parameters = if data.is_null() { Value::Object(Map::new()) } else { data };
        let request = Request::create_with(&uri, method, parameters, headers);

        let response = self.app.handle_request(request.clone()).await;

        for cookie in response.cookies() {
            if cookie.is_cleared() {
                self.cookies.shift_remove(&cookie.name);
            } else {
                self.cookies.insert(cookie.name.clone(), cookie.value.clone());
            }
        }

        TestResponse::new(response, request)
    }

    async fn start_session_with_pending_data(&mut self) -> Result<()> {
        let manager = self.app.make::<SessionManager>();
        let store = manager.driver()?;
        let cookie = manager.cookie_name();

        // Reuse the current session when the client already has one.
        if let Some(existing) = self.cookies.get(&cookie).and_then(|value| crate::testing::test_response::decrypt_cookie(&cookie, value)) {
            store.set_id(Some(&existing));
        }
        store.start().await?;
        for (key, value) in std::mem::take(&mut self.pending_session) {
            store.put(&key, value);
        }
        store.save().await?;

        if let Some(encrypted) = encrypt_cookie(&cookie, &store.id()) {
            self.cookies.insert(cookie, encrypted);
        }
        Ok(())
    }
}

/// Encrypt a cookie value the way `EncryptCookies` does.
pub fn encrypt_cookie(name: &str, value: &str) -> Option<String> {
    let encrypter = illuminate_encryption::encrypter().ok()?;
    let prefixed = format!("{}{}", CookieValuePrefix::create(name, encrypter.get_key()), value);
    encrypter.encrypt_string(&prefixed).ok()
}

fn base64_key() -> String {
    use base64::Engine;
    let bytes: Vec<u8> = Str::random(32).into_bytes();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
