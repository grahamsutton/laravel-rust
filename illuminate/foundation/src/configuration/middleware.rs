//! Configure the application's middleware, Laravel 11 style.
//!
//! ```ignore
//! Application::configure(base_path)
//!     .with_middleware(|middleware| {
//!         middleware.web_append(EnsureUserIsSubscribed);
//!         middleware.alias("subscribed", |_| Arc::new(EnsureUserIsSubscribed));
//!         middleware.validate_csrf_tokens(&["stripe/*"]);
//!     })
//! ```

use std::sync::Arc;

use indexmap::IndexMap;

use illuminate_http::Middleware as HttpMiddleware;
use illuminate_routing::{IntoMiddleware, MiddlewareFactory, RouteMiddleware};

/// The application's middleware configuration.
#[derive(Clone)]
pub struct Middleware {
    pub(crate) global: Vec<RouteMiddleware>,
    pub(crate) global_prepend: Vec<RouteMiddleware>,
    pub(crate) global_append: Vec<RouteMiddleware>,
    pub(crate) global_remove: Vec<String>,
    pub(crate) groups: IndexMap<String, Vec<RouteMiddleware>>,
    pub(crate) group_prepend: IndexMap<String, Vec<RouteMiddleware>>,
    pub(crate) group_append: IndexMap<String, Vec<RouteMiddleware>>,
    pub(crate) group_remove: IndexMap<String, Vec<String>>,
    pub(crate) aliases: Vec<(String, MiddlewareFactory)>,
    pub(crate) priority: Option<Vec<String>>,
    pub(crate) csrf_except: Vec<String>,
    pub(crate) encrypt_cookies_except: Vec<String>,
    pub(crate) trim_strings_except: Vec<String>,
    pub(crate) empty_strings_except: Vec<String>,
    pub(crate) maintenance_except: Vec<String>,
    pub(crate) trusted_proxies: Option<Vec<String>>,
    pub(crate) trusted_hosts: Option<(Vec<String>, bool)>,
    pub(crate) redirect_guests_to: Option<String>,
    pub(crate) redirect_users_to: Option<String>,
    pub(crate) throttle_api: Option<String>,
    pub(crate) disabled_csrf: bool,
}

impl Default for Middleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware {
    /// The framework's default middleware configuration.
    pub fn new() -> Self {
        Self {
            global: Vec::new(),
            global_prepend: Vec::new(),
            global_append: Vec::new(),
            global_remove: Vec::new(),
            groups: IndexMap::new(),
            group_prepend: IndexMap::new(),
            group_append: IndexMap::new(),
            group_remove: IndexMap::new(),
            aliases: Vec::new(),
            priority: None,
            csrf_except: Vec::new(),
            encrypt_cookies_except: Vec::new(),
            trim_strings_except: Vec::new(),
            empty_strings_except: Vec::new(),
            maintenance_except: Vec::new(),
            trusted_proxies: None,
            trusted_hosts: None,
            redirect_guests_to: None,
            redirect_users_to: None,
            throttle_api: None,
            disabled_csrf: false,
        }
    }

    /// Append middleware to the application's global middleware stack.
    pub fn append(&mut self, middleware: impl IntoMiddleware) -> &mut Self {
        self.global_append.extend(middleware.into_middleware());
        self
    }

    /// Prepend middleware to the application's global middleware stack.
    pub fn prepend(&mut self, middleware: impl IntoMiddleware) -> &mut Self {
        self.global_prepend.extend(middleware.into_middleware());
        self
    }

    /// Remove a framework middleware (by type name) from the global stack.
    pub fn remove<M: HttpMiddleware>(&mut self) -> &mut Self {
        self.global_remove.push(std::any::type_name::<M>().to_string());
        self
    }

    /// Replace the entire global middleware stack.
    pub fn use_(&mut self, middleware: impl IntoMiddleware) -> &mut Self {
        self.global = middleware.into_middleware();
        self.global_remove.push("*".to_string());
        self
    }

    /// Define (or replace) a middleware group.
    pub fn group(&mut self, name: &str, middleware: impl IntoMiddleware) -> &mut Self {
        self.groups.insert(name.to_string(), middleware.into_middleware());
        self
    }

    /// Append middleware to a group.
    pub fn append_to_group(&mut self, group: &str, middleware: impl IntoMiddleware) -> &mut Self {
        self.group_append
            .entry(group.to_string())
            .or_default()
            .extend(middleware.into_middleware());
        self
    }

    /// Prepend middleware to a group.
    pub fn prepend_to_group(&mut self, group: &str, middleware: impl IntoMiddleware) -> &mut Self {
        self.group_prepend
            .entry(group.to_string())
            .or_default()
            .extend(middleware.into_middleware());
        self
    }

    /// Remove middleware (by alias name or type name) from a group.
    pub fn remove_from_group(&mut self, group: &str, name: &str) -> &mut Self {
        self.group_remove
            .entry(group.to_string())
            .or_default()
            .push(name.to_string());
        self
    }

    /// Append middleware to the "web" group.
    pub fn web_append(&mut self, middleware: impl IntoMiddleware) -> &mut Self {
        self.append_to_group("web", middleware)
    }

    /// Prepend middleware to the "web" group.
    pub fn web_prepend(&mut self, middleware: impl IntoMiddleware) -> &mut Self {
        self.prepend_to_group("web", middleware)
    }

    /// Append middleware to the "api" group.
    pub fn api_append(&mut self, middleware: impl IntoMiddleware) -> &mut Self {
        self.append_to_group("api", middleware)
    }

    /// Prepend middleware to the "api" group.
    pub fn api_prepend(&mut self, middleware: impl IntoMiddleware) -> &mut Self {
        self.prepend_to_group("api", middleware)
    }

    /// Register a middleware alias, built from its parameters
    /// (`"role:admin"` → `["admin"]`).
    pub fn alias(
        &mut self,
        name: &str,
        factory: impl Fn(&[String]) -> Arc<dyn HttpMiddleware> + Send + Sync + 'static,
    ) -> &mut Self {
        self.aliases.push((name.to_string(), Arc::new(factory)));
        self
    }

    /// Register a middleware alias for a ready-made instance.
    pub fn alias_instance(&mut self, name: &str, middleware: impl HttpMiddleware) -> &mut Self {
        let middleware: Arc<dyn HttpMiddleware> = Arc::new(middleware);
        self.alias(name, move |_| middleware.clone())
    }

    /// Set the middleware priority (by alias name).
    pub fn priority<S: Into<String>>(&mut self, priority: impl IntoIterator<Item = S>) -> &mut Self {
        self.priority = Some(priority.into_iter().map(Into::into).collect());
        self
    }

    /// URIs that should be excluded from CSRF verification.
    pub fn validate_csrf_tokens(&mut self, except: &[&str]) -> &mut Self {
        self.csrf_except.extend(except.iter().map(|s| s.to_string()));
        self
    }

    /// Disable CSRF verification entirely (not recommended).
    pub fn without_csrf_protection(&mut self) -> &mut Self {
        self.disabled_csrf = true;
        self
    }

    /// Cookies that should not be encrypted.
    pub fn encrypt_cookies(&mut self, except: &[&str]) -> &mut Self {
        self.encrypt_cookies_except.extend(except.iter().map(|s| s.to_string()));
        self
    }

    /// Input keys that should not be trimmed.
    pub fn trim_strings(&mut self, except: &[&str]) -> &mut Self {
        self.trim_strings_except.extend(except.iter().map(|s| s.to_string()));
        self
    }

    /// Input keys that should not be converted from empty strings to null.
    pub fn convert_empty_strings_to_null(&mut self, except: &[&str]) -> &mut Self {
        self.empty_strings_except.extend(except.iter().map(|s| s.to_string()));
        self
    }

    /// URIs reachable while the application is in maintenance mode.
    pub fn prevent_requests_during_maintenance(&mut self, except: &[&str]) -> &mut Self {
        self.maintenance_except.extend(except.iter().map(|s| s.to_string()));
        self
    }

    /// Trust the given proxies (`&["*"]` for all).
    pub fn trust_proxies(&mut self, at: &[&str]) -> &mut Self {
        self.trusted_proxies = Some(at.iter().map(|s| s.to_string()).collect());
        self
    }

    /// Only answer requests for the given host patterns (regular
    /// expressions), plus every subdomain of the application URL when
    /// `subdomains` is true.
    ///
    /// ```ignore
    /// middleware.trust_hosts(&["^laravel\\.test$"], true);
    /// ```
    pub fn trust_hosts(&mut self, at: &[&str], subdomains: bool) -> &mut Self {
        self.trusted_hosts = Some((at.iter().map(|s| s.to_string()).collect(), subdomains));
        self
    }

    /// Where guests are sent when authentication is required.
    pub fn redirect_guests_to(&mut self, path: &str) -> &mut Self {
        self.redirect_guests_to = Some(path.to_string());
        self
    }

    /// Where authenticated users are sent by the "guest" middleware.
    pub fn redirect_users_to(&mut self, path: &str) -> &mut Self {
        self.redirect_users_to = Some(path.to_string());
        self
    }

    /// Configure both redirect targets at once.
    pub fn redirect_to(&mut self, guests: &str, users: &str) -> &mut Self {
        self.redirect_guests_to(guests).redirect_users_to(users)
    }

    /// Throttle API requests using the given named rate limiter.
    pub fn throttle_api(&mut self, limiter: &str) -> &mut Self {
        self.throttle_api = Some(limiter.to_string());
        self
    }
}
