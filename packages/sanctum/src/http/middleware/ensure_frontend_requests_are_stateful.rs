use std::fmt;
use std::sync::Arc;

use illuminate_config::Repository;
use illuminate_container::try_app;
use illuminate_cookie::{AddQueuedCookiesToResponse, EncryptCookies};
use illuminate_http::{
    Destination, Middleware, Next, Request, Response, async_trait, middleware_fn, run_middleware,
};
use illuminate_routing::{MiddlewareNotFoundException, RouteMiddleware, Router};
use illuminate_session::{StartSession, ValidateCsrfToken};
use illuminate_support::{Result, Str};

use super::AuthenticateSession;
use crate::config::{self, MiddlewareSetting};
use crate::facade::Sanctum;

/// Gives requests from your first-party SPA a session: when the `Referer`
/// (or `Origin`) is one of the `sanctum.stateful` domains, the request runs
/// through cookie encryption, the session, CSRF protection and session
/// authentication — and the `sanctum` request attribute is set — before
/// reaching your routes. Every other request stays stateless.
///
/// Put it at the start of the `api` middleware group (Laravel's
/// `$middleware->statefulApi()`).
///
/// The middleware it runs follow `sanctum.middleware.*`; replace any of
/// them with your application's own instances:
///
/// ```
/// use std::sync::Arc;
/// use illuminate_cookie::EncryptCookies;
/// use laravel_sanctum::EnsureFrontendRequestsAreStateful;
///
/// let stateful = EnsureFrontendRequestsAreStateful::new()
///     .encrypt_cookies_using(Arc::new(EncryptCookies::new().except(["theme"])));
/// ```
#[derive(Clone, Default)]
pub struct EnsureFrontendRequestsAreStateful {
    encrypt_cookies: Option<Arc<dyn Middleware>>,
    validate_csrf_token: Option<Arc<dyn Middleware>>,
    authenticate_session: Option<Arc<dyn Middleware>>,
}

impl EnsureFrontendRequestsAreStateful {
    /// Create the middleware.
    pub fn new() -> Self {
        Self::default()
    }

    /// Use the given cookie encryption middleware.
    pub fn encrypt_cookies_using(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.encrypt_cookies = Some(middleware);
        self
    }

    /// Use the given CSRF protection middleware.
    pub fn validate_csrf_token_using(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.validate_csrf_token = Some(middleware);
        self
    }

    /// Use the given session authentication middleware.
    pub fn authenticate_session_using(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.authenticate_session = Some(middleware);
        self
    }

    /// Determine if the request comes from your first-party frontend: its
    /// `Referer` (or `Origin`) matches one of the `sanctum.stateful` domains.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_config::Repository;
    /// use illuminate_container::Container;
    /// use illuminate_http::{HeaderMap, HeaderValue, Request};
    /// use illuminate_support::json;
    /// use laravel_sanctum::EnsureFrontendRequestsAreStateful;
    ///
    /// let container = Arc::new(Container::new());
    /// let _guard = Container::set_local_instance(container.clone());
    /// container.instance(Repository::new(json!({"sanctum": {"stateful": ["spa.test"]}})));
    ///
    /// let mut headers = HeaderMap::new();
    /// headers.insert("referer", HeaderValue::from_static("https://spa.test/dashboard"));
    /// let request = Request::create_with("/api/user", "GET", json!({}), headers);
    ///
    /// assert!(EnsureFrontendRequestsAreStateful::from_frontend(&request));
    /// assert!(!EnsureFrontendRequestsAreStateful::from_frontend(&Request::create("/api/user", "GET")));
    /// ```
    pub fn from_frontend(request: &Request) -> bool {
        let domain = request
            .header("referer")
            .filter(|referer| !referer.is_empty())
            .or_else(|| request.header("origin"));
        let Some(domain) = domain else {
            return false;
        };
        let domain = Str::replace_first("https://", "", &domain);
        let domain = Str::replace_first("http://", "", &domain);
        let domain = if domain.ends_with('/') {
            domain
        } else {
            format!("{domain}/")
        };

        config::stateful_domains()
            .into_iter()
            .filter(|uri| !uri.is_empty())
            .any(|uri| {
                let uri = if uri == Sanctum::CURRENT_REQUEST_HOST_PLACEHOLDER {
                    request.http_host()
                } else {
                    uri
                };
                Str::is(&format!("{}/*", uri.trim()), &domain)
            })
    }

    /// The middleware a stateful request runs through: mark it stateful,
    /// encrypt cookies, queue cookies, start the session, validate the CSRF
    /// token, and authenticate the session.
    pub fn frontend_middleware(&self) -> Result<Vec<Arc<dyn Middleware>>> {
        let mut middleware: Vec<Arc<dyn Middleware>> =
            vec![middleware_fn(|request: Request, next: Next| async move {
                request.set_attribute("sanctum", true);
                Ok(next.run(request).await)
            })];
        if let Some(encrypt) = configured(&self.encrypt_cookies, "encrypt_cookies", || {
            Arc::new(EncryptCookies::new())
        })? {
            middleware.push(encrypt);
        }
        middleware.push(Arc::new(AddQueuedCookiesToResponse::new()));
        middleware.push(Arc::new(StartSession::new()));
        if let Some(csrf) = configured(&self.validate_csrf_token, "validate_csrf_token", || {
            Arc::new(ValidateCsrfToken::new())
        })? {
            middleware.push(csrf);
        }
        if let Some(session) =
            configured(&self.authenticate_session, "authenticate_session", || {
                Arc::new(AuthenticateSession::new())
            })?
        {
            middleware.push(session);
        }
        Ok(middleware)
    }

    /// Session cookies of stateful requests are HTTP-only and `lax`.
    fn configure_secure_cookie_sessions() {
        if let Some(config) = try_app::<Repository>() {
            config.set("session.http_only", true);
            config.set("session.same_site", "lax");
        }
    }
}

/// One of the stateful middleware: the instance given to the builder, or
/// what `sanctum.middleware.<key>` asks for.
fn configured(
    custom: &Option<Arc<dyn Middleware>>,
    key: &str,
    default: impl FnOnce() -> Arc<dyn Middleware>,
) -> Result<Option<Arc<dyn Middleware>>> {
    if let Some(custom) = custom {
        return Ok(Some(custom.clone()));
    }
    match config::middleware(key) {
        MiddlewareSetting::Default => Ok(Some(default())),
        MiddlewareSetting::Disabled => Ok(None),
        MiddlewareSetting::Alias(name) => {
            let router = try_app::<Router>()
                .ok_or_else(|| MiddlewareNotFoundException { name: name.clone() })?;
            router
                .resolve_middleware_instance(&RouteMiddleware::named(name))
                .map(Some)
        }
    }
}

impl fmt::Debug for EnsureFrontendRequestsAreStateful {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnsureFrontendRequestsAreStateful")
            .field("encrypt_cookies", &self.encrypt_cookies.is_some())
            .field("validate_csrf_token", &self.validate_csrf_token.is_some())
            .field("authenticate_session", &self.authenticate_session.is_some())
            .finish()
    }
}

#[async_trait]
impl Middleware for EnsureFrontendRequestsAreStateful {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        Self::configure_secure_cookie_sessions();
        if !Self::from_frontend(&request) {
            return Ok(next.run(request).await);
        }
        let middleware = self.frontend_middleware()?;
        let destination: Destination = Arc::new(move |request: Request| {
            let next = next.clone();
            Box::pin(async move { next.run(request).await })
        });
        Ok(run_middleware(request, middleware, destination).await)
    }
}
