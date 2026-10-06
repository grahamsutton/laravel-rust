use std::sync::Arc;

use subtle::ConstantTimeEq;

use illuminate_config::Repository;
use illuminate_container::try_app;
use illuminate_cookie::CookieValuePrefix;
use illuminate_encryption::{Encrypter, encrypter};
use illuminate_http::{Cookie, HttpException, Middleware, Next, Request, Response, async_trait};
use illuminate_support::{Result, Value};

use crate::exceptions::TokenMismatchException;
use crate::manager::SessionConfig;
use crate::request::RequestSessionExt;

/// Protects the application from cross-site request forgeries.
///
/// Requests that only read (`GET`, `HEAD`, `OPTIONS`) pass straight through.
/// Everything else must come from the same origin (per the browser's
/// `Sec-Fetch-Site` header) or carry the session's CSRF token — as the
/// `_token` input, the `X-CSRF-TOKEN` header, or the encrypted
/// `X-XSRF-TOKEN` header that JavaScript libraries copy from the
/// `XSRF-TOKEN` cookie. A mismatch fails with a [`TokenMismatchException`],
/// which renders as `419 Page Expired`.
///
/// Like Laravel, verification is skipped while running unit tests: when the
/// `app.running_unit_tests` configuration value is true (the testing harness
/// sets it). Call [`ValidateCsrfToken::verify_during_unit_tests`] to keep it on.
///
/// ```
/// use illuminate_session::ValidateCsrfToken;
///
/// let middleware = ValidateCsrfToken::new().except(["stripe/*", "http://example.com/foo/bar"]);
/// assert_eq!(middleware.get_excluded_paths().len(), 2);
/// assert_eq!(ValidateCsrfToken::ALIAS, "csrf");
/// ```
#[derive(Clone, Debug)]
pub struct ValidateCsrfToken {
    except: Vec<String>,
    add_http_cookie: bool,
    allow_same_site: bool,
    origin_only: bool,
    enabled: bool,
    skip_during_unit_tests: bool,
    encrypter: Option<Arc<Encrypter>>,
}

/// Laravel's older name for [`ValidateCsrfToken`].
pub type VerifyCsrfToken = ValidateCsrfToken;

/// Laravel 13's name for [`ValidateCsrfToken`].
pub type PreventRequestForgery = ValidateCsrfToken;

impl Default for ValidateCsrfToken {
    fn default() -> Self {
        Self {
            except: Vec::new(),
            add_http_cookie: true,
            allow_same_site: false,
            origin_only: false,
            enabled: true,
            skip_during_unit_tests: true,
            encrypter: None,
        }
    }
}

impl ValidateCsrfToken {
    /// The middleware's alias.
    pub const ALIAS: &'static str = "csrf";

    /// Create the middleware, using the application's encrypter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create the middleware with a specific encrypter (used for `X-XSRF-TOKEN`).
    pub fn with_encrypter(encrypter: Arc<Encrypter>) -> Self {
        Self {
            encrypter: Some(encrypter),
            ..Self::default()
        }
    }

    /// The URIs that should be excluded from verification (`stripe/*`, full URLs, ...).
    pub fn except<I, S>(mut self, uris: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.except.extend(uris.into_iter().map(Into::into));
        self
    }

    /// Allow requests from the same site (subdomains) in addition to the same origin.
    pub fn allow_same_site(mut self, allow: bool) -> Self {
        self.allow_same_site = allow;
        self
    }

    /// Rely solely on origin verification, rejecting requests that fail it
    /// with a `403` instead of falling back to the token.
    pub fn use_origin_only(mut self, origin_only: bool) -> Self {
        self.origin_only = origin_only;
        self
    }

    /// Choose whether the `XSRF-TOKEN` cookie is added to responses.
    pub fn add_http_cookie(mut self, add: bool) -> Self {
        self.add_http_cookie = add;
        self
    }

    /// Turn verification on or off entirely.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Keep verifying tokens even while running unit tests.
    pub fn verify_during_unit_tests(mut self) -> Self {
        self.skip_during_unit_tests = false;
        self
    }

    /// The URIs that are excluded from verification.
    pub fn get_excluded_paths(&self) -> &[String] {
        &self.except
    }

    /// Determine if the `XSRF-TOKEN` cookie should be added to the response.
    pub fn should_add_xsrf_token_cookie(&self) -> bool {
        !self.origin_only && self.add_http_cookie
    }

    /// Determine if the HTTP request uses a "read" verb.
    fn is_reading(&self, request: &Request) -> bool {
        ["HEAD", "GET", "OPTIONS"].contains(&request.method().as_str())
    }

    /// Determine if the application is running unit tests.
    fn running_unit_tests(&self) -> bool {
        self.skip_during_unit_tests
            && try_app::<Repository>()
                .is_some_and(|config| config.boolean("app.running_unit_tests"))
    }

    /// Determine if the request has a URI that should be excluded.
    fn in_except_array(&self, request: &Request) -> bool {
        self.except.iter().any(|except| {
            let except = if except == "/" {
                except.as_str()
            } else {
                except.trim_matches('/')
            };
            request.full_url_is(except) || request.is(except)
        })
    }

    /// Determine if the request has a valid origin based on the `Sec-Fetch-Site` header.
    fn has_valid_origin(&self, request: &Request) -> Result<bool> {
        match request.header("sec-fetch-site").as_deref() {
            Some("same-origin") => return Ok(true),
            Some("same-site") if self.allow_same_site => return Ok(true),
            _ => {}
        }
        if self.origin_only {
            return Err(HttpException::with_message(403, "Origin mismatch.").into());
        }
        Ok(false)
    }

    /// Determine if the session and input CSRF tokens match.
    fn tokens_match(&self, request: &Request) -> bool {
        let Some(session_token) = request.try_session().and_then(|session| session.token()) else {
            return false;
        };
        let Some(token) = self.get_token_from_request(request) else {
            return false;
        };
        !session_token.is_empty()
            && session_token.len() == token.len()
            && bool::from(session_token.as_bytes().ct_eq(token.as_bytes()))
    }

    /// Get the CSRF token from the request.
    fn get_token_from_request(&self, request: &Request) -> Option<String> {
        let input = match request.input("_token") {
            Value::String(token) if !token.is_empty() => Some(token),
            _ => None,
        };
        let token = input.or_else(|| {
            request
                .header("x-csrf-token")
                .filter(|token| !token.is_empty())
        });
        if token.is_some() {
            return token;
        }

        let header = request
            .header("x-xsrf-token")
            .filter(|header| !header.is_empty())?;
        let encrypter = match &self.encrypter {
            Some(encrypter) => encrypter.clone(),
            None => encrypter().ok()?,
        };
        Some(
            encrypter
                .decrypt_string(&header)
                .map(|value| CookieValuePrefix::remove(&value))
                .unwrap_or_default(),
        )
    }

    /// Add the `XSRF-TOKEN` cookie (readable by JavaScript) to the response.
    fn add_cookie_to_response(&self, request: &Request, response: &mut Response) {
        let Some(token) = request.try_session().and_then(|session| session.token()) else {
            return;
        };
        let config = try_app::<Repository>()
            .map(|config| SessionConfig::from_repository(&config))
            .unwrap_or_default();
        let mut cookie = Cookie::new("XSRF-TOKEN", token)
            .minutes(config.lifetime)
            .path(config.path.clone())
            .secure(config.secure.unwrap_or_else(|| request.secure()))
            .http_only(false)
            .same_site(config.same_site)
            .partitioned(config.partitioned);
        if let Some(domain) = config.domain {
            cookie = cookie.domain(domain);
        }
        response.add_cookie(cookie);
    }
}

#[async_trait]
impl Middleware for ValidateCsrfToken {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let passes = self.is_reading(&request)
            || !self.enabled
            || self.running_unit_tests()
            || self.in_except_array(&request)
            || self.has_valid_origin(&request)?
            || self.tokens_match(&request);

        if !passes {
            return Err(TokenMismatchException::default().into());
        }

        let mut response = next.run(request.clone()).await;
        if self.should_add_xsrf_token_cookie() {
            self.add_cookie_to_response(&request, &mut response);
        }
        Ok(response)
    }
}
