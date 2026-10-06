//! Routes: the URI, verbs, handler and attributes registered with the router.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use indexmap::IndexMap;
use regex::Regex;

use illuminate_http::{BoxFuture, Request, Response};
use illuminate_support::{Result, Str};

use crate::compiled::CompiledRoute;
use crate::exceptions::InvalidRouteException;
use crate::middleware::{IntoMiddleware, RouteMiddleware};

/// A type-erased route handler.
///
/// The future resolves to `Ok(response)` once the handler ran, or to `Err`
/// when one of the handler's arguments could not be extracted from the
/// request (which lets the router invoke the route's `missing` handler for
/// model bindings that could not be found).
pub type RouteHandler = Arc<dyn Fn(Request) -> BoxFuture<'static, Result<Response>> + Send + Sync>;

/// A handler together with its display name (`"UserController@show"`, or
/// `"Closure"`), as shown by `route:list`.
#[derive(Clone)]
pub struct RouteAction {
    pub handler: RouteHandler,
    pub name: String,
}

impl RouteAction {
    /// Create an action from a handler and a display name.
    pub fn new(name: impl Into<String>, handler: RouteHandler) -> Self {
        Self {
            handler,
            name: name.into(),
        }
    }
}

impl std::fmt::Debug for RouteAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteAction").field("name", &self.name).finish()
    }
}

/// The regular expressions behind `where_number`, `where_alpha`, ...
pub mod patterns {
    pub const ALPHA: &str = "[a-zA-Z]+";
    pub const ALPHA_NUMERIC: &str = "[a-zA-Z0-9]+";
    pub const NUMBER: &str = "[0-9]+";
    pub const ULID: &str = "[0-7][0-9a-hjkmnp-tv-zA-HJKMNP-TV-Z]{25}";
    pub const UUID: &str =
        r"[\da-fA-F]{8}-[\da-fA-F]{4}-[\da-fA-F]{4}-[\da-fA-F]{4}-[\da-fA-F]{12}";

    /// A pattern matching exactly one of the given values.
    pub fn one_of<S: AsRef<str>>(values: &[S]) -> String {
        values
            .iter()
            .map(|v| regex::escape(v.as_ref()))
            .collect::<Vec<_>>()
            .join("|")
    }
}

#[derive(Clone, Default)]
pub(crate) struct RouteState {
    pub methods: Vec<String>,
    pub uri: String,
    pub name: Option<String>,
    pub domain: Option<String>,
    pub prefix: Option<String>,
    pub middleware: Vec<RouteMiddleware>,
    pub excluded_middleware: Vec<RouteMiddleware>,
    pub wheres: IndexMap<String, String>,
    pub defaults: IndexMap<String, String>,
    pub binding_fields: IndexMap<String, String>,
    pub fallback: bool,
    pub missing: Option<RouteHandler>,
    pub with_trashed: bool,
    pub scope_bindings: Option<bool>,
}

struct RouteInner {
    state: RwLock<RouteState>,
    action: RouteAction,
    compiled: RwLock<Option<Arc<CompiledRoute>>>,
    changes: Arc<AtomicU64>,
}

/// A route registered with the router.
///
/// `RouteDefinition` is a cheap, shared handle: the router keeps one copy
/// and hands you another, so chained calls like `.name("users.show")` and
/// `.middleware("auth")` update the registered route in place.
///
/// ```
/// use illuminate_routing::RouteDefinition;
/// use illuminate_routing::route::RouteAction;
/// use std::sync::Arc;
///
/// let action = RouteAction::new("Closure", Arc::new(|_request| {
///     Box::pin(async { Ok(illuminate_http::Response::new("Hello")) })
/// }));
///
/// let route = RouteDefinition::new(&["GET"], "/users/{id}", action)
///     .name("users.show")
///     .middleware(["auth", "verified"])
///     .where_number("id");
///
/// assert_eq!(route.uri(), "users/{id}");
/// assert_eq!(route.methods(), vec!["GET", "HEAD"]);
/// assert_eq!(route.get_name().as_deref(), Some("users.show"));
/// assert_eq!(route.wheres()["id"], "[0-9]+");
/// ```
#[derive(Clone)]
pub struct RouteDefinition {
    inner: Arc<RouteInner>,
}

impl std::fmt::Debug for RouteDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state();
        f.debug_struct("RouteDefinition")
            .field("methods", &state.methods)
            .field("uri", &state.uri)
            .field("name", &state.name)
            .field("action", &self.inner.action.name)
            .finish()
    }
}

impl RouteDefinition {
    /// Create a new, standalone route. `GET` routes also respond to `HEAD`.
    pub fn new(methods: &[&str], uri: &str, action: RouteAction) -> Self {
        Self::with_changes(methods, uri, action, Arc::new(AtomicU64::new(0)))
    }

    pub(crate) fn with_changes(
        methods: &[&str],
        uri: &str,
        action: RouteAction,
        changes: Arc<AtomicU64>,
    ) -> Self {
        let mut methods: Vec<String> = methods.iter().map(|m| m.to_ascii_uppercase()).collect();
        if methods.iter().any(|m| m == "GET") && !methods.iter().any(|m| m == "HEAD") {
            methods.push("HEAD".to_string());
        }
        let (uri, binding_fields) = parse_uri(uri);
        Self {
            inner: Arc::new(RouteInner {
                state: RwLock::new(RouteState {
                    methods,
                    uri: normalize_uri(&uri),
                    binding_fields,
                    ..RouteState::default()
                }),
                action,
                compiled: RwLock::new(None),
                changes,
            }),
        }
    }

    pub(crate) fn state(&self) -> std::sync::RwLockReadGuard<'_, RouteState> {
        self.inner.state.read().unwrap()
    }

    pub(crate) fn update(&self, callback: impl FnOnce(&mut RouteState)) {
        callback(&mut self.inner.state.write().unwrap());
        *self.inner.compiled.write().unwrap() = None;
        self.inner.changes.fetch_add(1, Ordering::SeqCst);
    }

    /// Determine if two handles point at the same route.
    pub fn is(&self, other: &RouteDefinition) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    // ------------------------------------------------------------------
    // Fluent configuration
    // ------------------------------------------------------------------

    /// Add or append to the route's name. Inside a group with a name
    /// prefix (`Route::name("admin.")`), the given name is appended to it.
    pub fn name(self, name: &str) -> Self {
        self.update(|state| {
            state.name = Some(match state.name.take() {
                Some(existing) => format!("{existing}{name}"),
                None => name.to_string(),
            });
        });
        self
    }

    /// Replace the route's name entirely.
    pub fn set_name(self, name: Option<&str>) -> Self {
        self.update(|state| state.name = name.map(str::to_string));
        self
    }

    /// Attach middleware to the route.
    pub fn middleware(self, middleware: impl IntoMiddleware) -> Self {
        let middleware = middleware.into_middleware();
        self.update(|state| state.middleware.extend(middleware));
        self
    }

    /// Prevent middleware (usually inherited from a group) from running on
    /// this route.
    pub fn without_middleware(self, middleware: impl IntoMiddleware) -> Self {
        let middleware = middleware.into_middleware();
        self.update(|state| state.excluded_middleware.extend(middleware));
        self
    }

    /// Authorize the route with the `can` middleware: `.can("update", "post")`
    /// is shorthand for `.middleware("can:update,post")`.
    pub fn can(self, ability: &str, models: &str) -> Self {
        let name = if models.is_empty() {
            format!("can:{ability}")
        } else {
            format!("can:{ability},{models}")
        };
        self.middleware(name)
    }

    /// Constrain a parameter with a regular expression.
    pub fn where_(self, parameter: &str, expression: &str) -> Self {
        self.update(|state| {
            state.wheres.insert(parameter.to_string(), expression.to_string());
        });
        self
    }

    /// Constrain many parameters at once.
    pub fn wheres_many<'a>(self, wheres: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let wheres: Vec<(String, String)> = wheres
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        self.update(|state| state.wheres.extend(wheres));
        self
    }

    /// Constrain a parameter to digits.
    pub fn where_number(self, parameter: &str) -> Self {
        self.where_(parameter, patterns::NUMBER)
    }

    /// Constrain a parameter to letters.
    pub fn where_alpha(self, parameter: &str) -> Self {
        self.where_(parameter, patterns::ALPHA)
    }

    /// Constrain a parameter to letters and digits.
    pub fn where_alpha_numeric(self, parameter: &str) -> Self {
        self.where_(parameter, patterns::ALPHA_NUMERIC)
    }

    /// Constrain a parameter to a UUID.
    pub fn where_uuid(self, parameter: &str) -> Self {
        self.where_(parameter, patterns::UUID)
    }

    /// Constrain a parameter to a ULID.
    pub fn where_ulid(self, parameter: &str) -> Self {
        self.where_(parameter, patterns::ULID)
    }

    /// Constrain a parameter to one of the given values.
    pub fn where_in<S: AsRef<str>>(self, parameter: &str, values: &[S]) -> Self {
        let pattern = patterns::one_of(values);
        self.where_(parameter, &pattern)
    }

    /// Set a default value for a parameter (used when it is absent).
    pub fn defaults(self, key: &str, value: &str) -> Self {
        self.update(|state| {
            state.defaults.insert(key.to_string(), value.to_string());
        });
        self
    }

    /// Restrict the route to a domain (`"{account}.example.com"`).
    pub fn domain(self, domain: &str) -> Self {
        let (domain, fields) = parse_uri(domain);
        let domain = domain.replace("http://", "").replace("https://", "");
        self.update(|state| {
            state.domain = Some(domain);
            state.binding_fields.extend(fields);
        });
        self
    }

    /// Prefix the route's URI.
    pub fn prefix(self, prefix: &str) -> Self {
        self.update(|state| {
            let new_prefix = format!(
                "{}/{}",
                prefix.trim_end_matches('/'),
                state.prefix.as_deref().unwrap_or_default().trim_start_matches('/')
            );
            let new_prefix = new_prefix.trim_matches('/');
            if !new_prefix.is_empty() {
                state.prefix = Some(new_prefix.to_string());
            }
            let uri = format!("{}/{}", prefix.trim_end_matches('/'), state.uri.trim_start_matches('/'));
            let (uri, fields) = parse_uri(&uri);
            state.uri = normalize_uri(&uri);
            state.binding_fields.extend(fields);
        });
        self
    }

    /// Mark the route as the fallback route.
    pub fn fallback(self) -> Self {
        self.update(|state| state.fallback = true);
        self
    }

    /// Handle requests whose bound models can't be found with the given
    /// handler instead of a 404.
    pub fn missing<H, T>(self, handler: H) -> Self
    where
        H: crate::handler::Handler<T>,
        T: 'static,
    {
        let action = handler.into_action();
        self.update(|state| state.missing = Some(action.handler));
        self
    }

    /// Allow soft deleted models to be bound to this route.
    pub fn with_trashed(self) -> Self {
        self.update(|state| state.with_trashed = true);
        self
    }

    /// Scope nested ("child") bindings to their parents.
    pub fn scope_bindings(self) -> Self {
        self.update(|state| state.scope_bindings = Some(true));
        self
    }

    /// Never scope nested bindings, even with custom keys.
    pub fn without_scoped_bindings(self) -> Self {
        self.update(|state| state.scope_bindings = Some(false));
        self
    }

    // ------------------------------------------------------------------
    // Inspection
    // ------------------------------------------------------------------

    /// The URI pattern (`users/{id}`), without leading slash (`/` for the root).
    pub fn uri(&self) -> String {
        self.state().uri.clone()
    }

    /// The HTTP verbs the route responds to.
    pub fn methods(&self) -> Vec<String> {
        self.state().methods.clone()
    }

    /// Determine if the route responds to the given verb.
    pub fn has_method(&self, method: &str) -> bool {
        self.state().methods.iter().any(|m| m.eq_ignore_ascii_case(method))
    }

    /// The route's name.
    pub fn get_name(&self) -> Option<String> {
        self.state().name.clone()
    }

    /// Determine if the route's name matches any of the given patterns
    /// (`"admin.*"`).
    pub fn named(&self, patterns: &[&str]) -> bool {
        self.get_name()
            .is_some_and(|name| patterns.iter().any(|pattern| Str::is(pattern, &name)))
    }

    /// The domain the route is restricted to.
    pub fn get_domain(&self) -> Option<String> {
        self.state().domain.clone()
    }

    /// The prefix applied to the route by its groups.
    pub fn get_prefix(&self) -> Option<String> {
        self.state().prefix.clone()
    }

    /// The middleware attached to the route (unresolved names and instances).
    pub fn get_middleware(&self) -> Vec<RouteMiddleware> {
        self.state().middleware.clone()
    }

    /// The names of the middleware attached to the route.
    pub fn middleware_names(&self) -> Vec<String> {
        self.state()
            .middleware
            .iter()
            .map(|m| m.name().to_string())
            .collect()
    }

    /// The middleware excluded from the route.
    pub fn excluded_middleware(&self) -> Vec<RouteMiddleware> {
        self.state().excluded_middleware.clone()
    }

    /// The parameter constraints.
    pub fn wheres(&self) -> IndexMap<String, String> {
        self.state().wheres.clone()
    }

    /// The default parameter values.
    pub fn get_defaults(&self) -> IndexMap<String, String> {
        self.state().defaults.clone()
    }

    /// Determine if this is the fallback route.
    pub fn is_fallback(&self) -> bool {
        self.state().fallback
    }

    /// Determine if soft deleted models may be bound to the route.
    pub fn allows_trashed_bindings(&self) -> bool {
        self.state().with_trashed
    }

    /// Determine if the route enforces scoped bindings.
    pub fn enforces_scoped_bindings(&self) -> bool {
        self.state().scope_bindings == Some(true)
    }

    /// Determine if the route prevents scoped bindings.
    pub fn prevents_scoped_bindings(&self) -> bool {
        self.state().scope_bindings == Some(false)
    }

    /// The action's display name (`"UserController@show"` or `"Closure"`).
    pub fn action_name(&self) -> String {
        self.inner.action.name.clone()
    }

    /// The controller method of the action (`"show"`).
    pub fn action_method(&self) -> String {
        let name = self.action_name();
        name.rsplit('@').next().unwrap_or(&name).to_string()
    }

    /// The route's handler.
    pub fn handler(&self) -> RouteHandler {
        self.inner.action.handler.clone()
    }

    /// The route's `missing` handler, if any.
    pub fn missing_handler(&self) -> Option<RouteHandler> {
        self.state().missing.clone()
    }

    /// The custom binding fields (`{post:slug}` → `post => slug`).
    pub fn binding_fields(&self) -> IndexMap<String, String> {
        self.state().binding_fields.clone()
    }

    /// The binding field for the given parameter.
    pub fn binding_field_for(&self, parameter: &str) -> Option<String> {
        self.state().binding_fields.get(parameter).cloned()
    }

    /// The names of the route's parameters (domain first, then the URI).
    pub fn parameter_names(&self) -> Vec<String> {
        let state = self.state();
        let source = format!("{}{}", state.domain.as_deref().unwrap_or_default(), state.uri);
        parameter_regex()
            .captures_iter(&source)
            .map(|c| c[1].trim_end_matches('?').to_string())
            .collect()
    }

    /// The names of the route's optional parameters (`{name?}`).
    pub fn optional_parameter_names(&self) -> Vec<String> {
        optional_parameters(&self.state().uri)
    }

    /// The compiled regular expressions for the route.
    pub fn compiled(&self) -> Result<Arc<CompiledRoute>, InvalidRouteException> {
        if let Some(compiled) = self.inner.compiled.read().unwrap().as_ref() {
            return Ok(compiled.clone());
        }
        let compiled = {
            let state = self.state();
            Arc::new(CompiledRoute::compile(
                &state.uri,
                state.domain.as_deref(),
                &state.wheres,
                &optional_parameters(&state.uri),
            )?)
        };
        *self.inner.compiled.write().unwrap() = Some(compiled.clone());
        Ok(compiled)
    }

    /// Determine if the route matches the request, optionally ignoring the
    /// HTTP verb, returning the bound parameters when it does.
    pub fn matches(
        &self,
        request: &Request,
        including_method: bool,
    ) -> Result<Option<IndexMap<String, String>>, InvalidRouteException> {
        let path = crate::compiled::normalize_path(&request.decoded_path());
        self.matches_parts(request.method().as_ref(), &path, &request.host(), including_method)
    }

    pub(crate) fn matches_parts(
        &self,
        method: &str,
        path: &str,
        host: &str,
        including_method: bool,
    ) -> Result<Option<IndexMap<String, String>>, InvalidRouteException> {
        if including_method && !self.has_method(method) {
            return Ok(None);
        }
        let compiled = self.compiled()?;
        if !compiled.matches_path(path) || !compiled.matches_host(host) {
            return Ok(None);
        }
        let mut parameters = compiled.bind(path, host).unwrap_or_default();
        for (key, value) in self.state().defaults.iter() {
            parameters.entry(key.clone()).or_insert_with(|| value.clone());
        }
        Ok(Some(parameters))
    }
}

fn parameter_regex() -> Regex {
    Regex::new(r"\{(\w+\??)\}").expect("valid parameter regex")
}

fn optional_parameters(uri: &str) -> Vec<String> {
    Regex::new(r"\{(\w+?)\?\}")
        .expect("valid optional regex")
        .captures_iter(uri)
        .map(|c| c[1].to_string())
        .collect()
}

/// Normalize a route URI: no leading or trailing slashes, `/` for the root.
pub(crate) fn normalize_uri(uri: &str) -> String {
    let trimmed = uri.trim_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Strip custom binding fields from a URI (`{post:slug}` → `{post}`),
/// returning them alongside the cleaned URI.
pub(crate) fn parse_uri(uri: &str) -> (String, IndexMap<String, String>) {
    let regex = Regex::new(r"\{([\w:]+?)(\??)\}").expect("valid binding regex");
    let mut fields = IndexMap::new();
    let cleaned = regex.replace_all(uri, |captures: &regex::Captures<'_>| {
        let inner = &captures[1];
        let optional = &captures[2];
        match inner.split_once(':') {
            Some((name, field)) => {
                fields.insert(name.to_string(), field.to_string());
                format!("{{{name}{optional}}}")
            }
            None => captures[0].to_string(),
        }
    });
    (cleaned.into_owned(), fields)
}

/// The route that matched the current request, along with its parameters.
///
/// The router attaches it to the request as an extension, so it's available
/// to middleware and handlers via `request.extension::<CurrentRoute>()`, the
/// [`CurrentRoute`] extractor, or `Route::current()`.
#[derive(Clone, Debug)]
pub struct CurrentRoute {
    route: RouteDefinition,
    parameters: IndexMap<String, String>,
}

impl CurrentRoute {
    /// Create a new matched route.
    pub fn new(route: RouteDefinition, parameters: IndexMap<String, String>) -> Self {
        Self { route, parameters }
    }

    /// The matched route definition.
    pub fn route(&self) -> &RouteDefinition {
        &self.route
    }

    /// The route's name.
    pub fn name(&self) -> Option<String> {
        self.route.get_name()
    }

    /// Determine if the route's name matches any of the given patterns.
    pub fn named(&self, patterns: &[&str]) -> bool {
        self.route.named(patterns)
    }

    /// The route's URI pattern.
    pub fn uri(&self) -> String {
        self.route.uri()
    }

    /// The route's HTTP verbs.
    pub fn methods(&self) -> Vec<String> {
        self.route.methods()
    }

    /// The action's display name.
    pub fn action_name(&self) -> String {
        self.route.action_name()
    }

    /// Get a bound parameter.
    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters.get(name).map(String::as_str)
    }

    /// All of the bound parameters, in order.
    pub fn parameters(&self) -> &IndexMap<String, String> {
        &self.parameters
    }

    /// Determine if the given parameter was bound.
    pub fn has_parameter(&self, name: &str) -> bool {
        self.parameters.contains_key(name)
    }

    /// The names of the route's parameters.
    pub fn parameter_names(&self) -> Vec<String> {
        self.route.parameter_names()
    }

    /// The binding field for the given parameter (`{post:slug}` → `slug`).
    pub fn binding_field_for(&self, parameter: &str) -> Option<String> {
        self.route.binding_field_for(parameter)
    }

    /// Determine if soft deleted models may be bound.
    pub fn allows_trashed_bindings(&self) -> bool {
        self.route.allows_trashed_bindings()
    }

    /// The parameter preceding the given one (its "parent" for scoped bindings).
    pub fn parent_of_parameter(&self, parameter: &str) -> Option<(&str, &str)> {
        let index = self.parameters.get_index_of(parameter)?;
        let (key, value) = self.parameters.get_index(index.checked_sub(1)?)?;
        Some((key.as_str(), value.as_str()))
    }
}

/// A route's details, as displayed by `route:list`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RouteListing {
    pub domain: Option<String>,
    pub methods: Vec<String>,
    pub uri: String,
    pub name: Option<String>,
    pub action: String,
    pub middleware: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> RouteAction {
        RouteAction::new(
            "Closure",
            Arc::new(|_request| Box::pin(async { Ok(Response::new("ok")) })),
        )
    }

    #[test]
    fn get_routes_also_respond_to_head() {
        let route = RouteDefinition::new(&["get"], "/", action());
        assert_eq!(route.methods(), vec!["GET", "HEAD"]);
        assert_eq!(route.uri(), "/");
        assert!(route.has_method("head"));
        assert!(!RouteDefinition::new(&["POST"], "x", action()).has_method("HEAD"));
    }

    #[test]
    fn names_are_appended() {
        let route = RouteDefinition::new(&["GET"], "users", action()).name("admin.").name("users");
        assert_eq!(route.get_name().as_deref(), Some("admin.users"));
        assert!(route.named(&["admin.*"]));
        assert!(!route.named(&["users.*"]));
    }

    #[test]
    fn binding_fields_are_parsed_from_the_uri() {
        let route = RouteDefinition::new(&["GET"], "users/{user}/posts/{post:slug}", action());
        assert_eq!(route.uri(), "users/{user}/posts/{post}");
        assert_eq!(route.binding_field_for("post").as_deref(), Some("slug"));
        assert_eq!(route.binding_field_for("user"), None);
        assert_eq!(route.parameter_names(), vec!["user", "post"]);
    }

    #[test]
    fn optional_parameters_are_detected() {
        let route = RouteDefinition::new(&["GET"], "users/{name?}", action());
        assert_eq!(route.optional_parameter_names(), vec!["name"]);
        assert_eq!(route.parameter_names(), vec!["name"]);
    }

    #[test]
    fn prefixes_are_prepended() {
        let route = RouteDefinition::new(&["GET"], "users", action()).prefix("admin").prefix("api");
        assert_eq!(route.uri(), "api/admin/users");
        assert_eq!(route.get_prefix().as_deref(), Some("api/admin"));
    }

    #[test]
    fn routes_match_requests() {
        let route = RouteDefinition::new(&["GET"], "users/{id}", action()).where_number("id");
        let request = Request::create("/users/7/", "GET");
        let parameters = route.matches(&request, true).unwrap().unwrap();
        assert_eq!(parameters["id"], "7");
        assert!(route.matches(&Request::create("/users/x", "GET"), true).unwrap().is_none());
        assert!(route.matches(&Request::create("/users/7", "POST"), true).unwrap().is_none());
        assert!(route.matches(&Request::create("/users/7", "POST"), false).unwrap().is_some());
    }

    #[test]
    fn defaults_fill_in_missing_parameters() {
        let route = RouteDefinition::new(&["GET"], "posts/{page?}", action()).defaults("page", "1");
        let parameters = route.matches(&Request::create("/posts", "GET"), true).unwrap().unwrap();
        assert_eq!(parameters["page"], "1");
    }

    #[test]
    fn compiled_routes_are_cached_until_changed() {
        let route = RouteDefinition::new(&["GET"], "users/{id}", action());
        let first = route.compiled().unwrap();
        assert!(Arc::ptr_eq(&first, &route.compiled().unwrap()));
        let route = route.where_number("id");
        assert!(!Arc::ptr_eq(&first, &route.compiled().unwrap()));
    }

    #[test]
    fn where_in_escapes_values() {
        let route = RouteDefinition::new(&["GET"], "category/{category}", action())
            .where_in("category", &["movie", "song", "c++"]);
        assert!(route.matches(&Request::create("/category/song", "GET"), true).unwrap().is_some());
        assert!(route.matches(&Request::create("/category/c++", "GET"), true).unwrap().is_some());
        assert!(route.matches(&Request::create("/category/book", "GET"), true).unwrap().is_none());
    }

    #[test]
    fn current_routes_know_parent_parameters() {
        let route = RouteDefinition::new(&["GET"], "users/{user}/posts/{post}", action());
        let parameters: IndexMap<String, String> =
            [("user".to_string(), "1".to_string()), ("post".to_string(), "2".to_string())]
                .into_iter()
                .collect();
        let current = CurrentRoute::new(route, parameters);
        assert_eq!(current.parent_of_parameter("post"), Some(("user", "1")));
        assert_eq!(current.parent_of_parameter("user"), None);
        assert_eq!(current.parameter("post"), Some("2"));
    }
}
