//! The router: route registration, middleware registries, and dispatching.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use indexmap::IndexMap;
use regex::Regex;

use illuminate_container::{Container, try_app};
use illuminate_http::{
    Destination, HttpException, Middleware, Precognition, Request, Response, render_exception,
    with_request,
};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Error, Map, Result, Str, Value};

use crate::exceptions::{
    MiddlewareNotFoundException, RecursiveMiddlewareGroupException, UrlGenerationException,
    method_not_allowed, not_found,
};
use crate::handler::Handler;
use crate::middleware::{
    IntoMiddleware, MiddlewareFactory, RouteMiddleware, RouteMiddlewareStack, ValidateSignature,
    sort_middleware, unique_middleware,
};
use crate::registrar::RouteRegistrar;
use crate::resource::{
    PendingResourceRegistration, PendingSingletonResourceRegistration, ResourceController,
};
use crate::route::{CurrentRoute, RouteAction, RouteDefinition, RouteHandler, RouteListing};

/// Renders a view into a response: `(view name, data) -> response`.
///
/// The router doesn't know about Blade; the application wires the view
/// factory in with [`Router::set_view_renderer`] so `Route::view` works.
pub type ViewRenderer = Arc<dyn Fn(&str, Value) -> Result<Response> + Send + Sync>;

/// Decides whether an extraction error means "the bound model was not
/// found" (so the route's `missing` handler should run).
pub type MissingModelDetector = Arc<dyn Fn(&Error) -> bool + Send + Sync>;

/// A callback run whenever a route is matched.
pub type MatchedCallback = Arc<dyn Fn(&CurrentRoute, &Request) + Send + Sync>;

/// Every HTTP verb the router knows.
pub const VERBS: [&str; 8] = [
    "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "QUERY",
];

/// The attributes shared by a group of routes.
#[derive(Clone, Default)]
pub struct GroupAttributes {
    /// A URI prefix (`"admin"`).
    pub prefix: Option<String>,
    /// A route name prefix (`"admin."`).
    pub name: Option<String>,
    /// The domain the routes respond to (`"{account}.example.com"`).
    pub domain: Option<String>,
    /// Middleware for every route in the group.
    pub middleware: Vec<RouteMiddleware>,
    /// Middleware removed from every route in the group.
    pub excluded_middleware: Vec<RouteMiddleware>,
    /// Parameter constraints.
    pub wheres: IndexMap<String, String>,
    /// The handler used when a bound model is missing.
    pub missing: Option<RouteHandler>,
    /// Whether nested bindings are scoped.
    pub scope_bindings: Option<bool>,
    /// Metadata for every route in the group.
    pub metadata: Map<String, Value>,
}

impl std::fmt::Debug for GroupAttributes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupAttributes")
            .field("prefix", &self.prefix)
            .field("name", &self.name)
            .field("domain", &self.domain)
            .field("middleware", &self.middleware)
            .field("excluded_middleware", &self.excluded_middleware)
            .field("wheres", &self.wheres)
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

impl GroupAttributes {
    /// Merge a nested group's attributes with its parent's: middleware and
    /// constraints are merged, while names and prefixes are appended.
    pub fn merge(
        new: &GroupAttributes,
        old: &GroupAttributes,
        prepend_existing_prefix: bool,
    ) -> Self {
        let old_prefix = old.prefix.clone().unwrap_or_default();
        let prefix = match &new.prefix {
            Some(prefix) if prepend_existing_prefix => Some(format!(
                "{}/{}",
                old_prefix.trim_matches('/'),
                prefix.trim_matches('/')
            )),
            Some(prefix) => Some(format!(
                "{}/{}",
                prefix.trim_matches('/'),
                old_prefix.trim_matches('/')
            )),
            None => old.prefix.clone(),
        };

        let name = match (&old.name, &new.name) {
            (Some(old), new) => Some(format!("{old}{}", new.as_deref().unwrap_or_default())),
            (None, new) => new.clone(),
        };

        let mut wheres = old.wheres.clone();
        wheres.extend(new.wheres.clone());

        let mut middleware = old.middleware.clone();
        middleware.extend(new.middleware.clone());

        let mut excluded_middleware = old.excluded_middleware.clone();
        excluded_middleware.extend(new.excluded_middleware.clone());

        let mut metadata = old.metadata.clone();
        crate::route::merge_metadata(&mut metadata, new.metadata.clone());

        Self {
            prefix,
            name,
            domain: new.domain.clone().or_else(|| old.domain.clone()),
            middleware,
            excluded_middleware,
            wheres,
            missing: new.missing.clone().or_else(|| old.missing.clone()),
            scope_bindings: new.scope_bindings.or(old.scope_bindings),
            metadata,
        }
    }
}

struct RouterInner {
    routes: RwLock<Vec<RouteDefinition>>,
    changes: Arc<AtomicU64>,
    names: RwLock<Option<(u64, HashMap<String, RouteDefinition>)>>,
    group_stack: Mutex<Vec<GroupAttributes>>,
    patterns: RwLock<IndexMap<String, String>>,
    aliases: RwLock<IndexMap<String, MiddlewareFactory>>,
    groups: RwLock<IndexMap<String, Vec<RouteMiddleware>>>,
    priority: RwLock<Vec<String>>,
    view_renderer: Arc<RwLock<Option<ViewRenderer>>>,
    missing_detector: RwLock<Option<MissingModelDetector>>,
    matched: RwLock<Vec<MatchedCallback>>,
    resource_verbs: RwLock<(String, String)>,
    resource_parameters: RwLock<IndexMap<String, String>>,
    singular_resource_parameters: AtomicBool,
}

/// Laravel's router.
///
/// `Router` is a cheap, cloneable handle with interior mutability; the
/// container holds it as `Arc<Router>` and the `Route` facade resolves it
/// from there. Routes are matched in the order they were registered.
///
/// ```
/// use illuminate_http::Request;
/// use illuminate_routing::{Path, Router};
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let router = Router::new();
///
/// router.get("/users/{id}", |Path(id): Path<u64>| async move { format!("User {id}") })
///     .where_number("id")
///     .name("users.show");
///
/// let response = router.dispatch(Request::create("/users/42", "GET")).await;
/// assert_eq!(response.content_string(), "User 42");
///
/// let response = router.dispatch(Request::create("/users/taylor", "GET")).await;
/// assert_eq!(response.status_code(), 404);
/// # });
/// ```
#[derive(Clone)]
pub struct Router {
    inner: Arc<RouterInner>,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("routes", &self.inner.routes.read().unwrap().len())
            .finish_non_exhaustive()
    }
}

struct GroupGuard<'a>(&'a Mutex<Vec<GroupAttributes>>);

impl Drop for GroupGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut stack) = self.0.lock() {
            stack.pop();
        }
    }
}

impl Router {
    /// Create a new router. The `signed` middleware alias is registered out
    /// of the box.
    pub fn new() -> Self {
        let router = Self {
            inner: Arc::new(RouterInner {
                routes: RwLock::new(Vec::new()),
                changes: Arc::new(AtomicU64::new(0)),
                names: RwLock::new(None),
                group_stack: Mutex::new(Vec::new()),
                patterns: RwLock::new(IndexMap::new()),
                aliases: RwLock::new(IndexMap::new()),
                groups: RwLock::new(IndexMap::new()),
                priority: RwLock::new(Vec::new()),
                view_renderer: Arc::new(RwLock::new(None)),
                missing_detector: RwLock::new(None),
                matched: RwLock::new(Vec::new()),
                resource_verbs: RwLock::new(("create".to_string(), "edit".to_string())),
                resource_parameters: RwLock::new(IndexMap::new()),
                singular_resource_parameters: AtomicBool::new(true),
            }),
        };
        router.alias_middleware_factory("signed", ValidateSignature::factory());
        router
    }

    // ------------------------------------------------------------------
    // Registering routes
    // ------------------------------------------------------------------

    /// Register a `GET` (and `HEAD`) route.
    pub fn get<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.add_route(&["GET", "HEAD"], uri, handler.into_action())
    }

    /// Register a `POST` route.
    pub fn post<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.add_route(&["POST"], uri, handler.into_action())
    }

    /// Register a `PUT` route.
    pub fn put<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.add_route(&["PUT"], uri, handler.into_action())
    }

    /// Register a `PATCH` route.
    pub fn patch<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.add_route(&["PATCH"], uri, handler.into_action())
    }

    /// Register a `DELETE` route.
    pub fn delete<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.add_route(&["DELETE"], uri, handler.into_action())
    }

    /// Register an `OPTIONS` route.
    pub fn options<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.add_route(&["OPTIONS"], uri, handler.into_action())
    }

    /// Register a `QUERY` route.
    pub fn query<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.add_route(&["QUERY"], uri, handler.into_action())
    }

    /// Register a route responding to every verb.
    pub fn any<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.add_route(&VERBS, uri, handler.into_action())
    }

    /// Register a route responding to the given verbs.
    pub fn match_<H: Handler<T>, T: 'static>(
        &self,
        methods: &[&str],
        uri: &str,
        handler: H,
    ) -> RouteDefinition {
        self.add_route(methods, uri, handler.into_action())
    }

    /// Register a route with an already type-erased action.
    pub fn add_route(&self, methods: &[&str], uri: &str, action: RouteAction) -> RouteDefinition {
        let route = self.create_route(methods, uri, action);
        self.inner.routes.write().unwrap().push(route.clone());
        self.inner.changes.fetch_add(1, Ordering::SeqCst);
        route
    }

    fn create_route(&self, methods: &[&str], uri: &str, action: RouteAction) -> RouteDefinition {
        let methods: Vec<String> = methods.iter().map(|m| m.to_ascii_uppercase()).collect();
        let methods: Vec<&str> = methods.iter().map(String::as_str).collect();
        let uri = self.prefix_uri(uri);
        let mut route =
            RouteDefinition::with_changes(&methods, &uri, action, self.inner.changes.clone());

        let group = self.inner.group_stack.lock().unwrap().last().cloned();
        if let Some(group) = group {
            route.update(|state| {
                if let Some(prefix) = &group.name {
                    state.name = Some(format!("{prefix}{}", state.name.take().unwrap_or_default()));
                }
                if let Some(prefix) = group.prefix.as_deref().map(|p| p.trim_matches('/'))
                    && !prefix.is_empty()
                {
                    state.prefix = Some(prefix.to_string());
                }
                let mut middleware = group.middleware.clone();
                middleware.append(&mut state.middleware);
                state.middleware = middleware;
                let mut excluded = group.excluded_middleware.clone();
                excluded.append(&mut state.excluded_middleware);
                state.excluded_middleware = excluded;
                let mut wheres = group.wheres.clone();
                wheres.extend(std::mem::take(&mut state.wheres));
                state.wheres = wheres;
                if state.missing.is_none() {
                    state.missing = group.missing.clone();
                }
                if state.scope_bindings.is_none() {
                    state.scope_bindings = group.scope_bindings;
                }
                if !group.metadata.is_empty() {
                    let mut metadata = group.metadata.clone();
                    crate::route::merge_metadata(&mut metadata, std::mem::take(&mut state.metadata));
                    state.metadata = metadata;
                }
            });
            if let Some(domain) = &group.domain
                && route.get_domain().is_none()
            {
                route = route.domain(domain);
            }
        }

        let patterns = self.inner.patterns.read().unwrap().clone();
        if !patterns.is_empty() {
            route.update(|state| {
                let mut wheres = patterns;
                wheres.extend(std::mem::take(&mut state.wheres));
                state.wheres = wheres;
            });
        }

        route
    }

    /// Prefix the URI with the current group's prefix.
    fn prefix_uri(&self, uri: &str) -> String {
        let prefix = self.get_last_group_prefix();
        let joined = format!("{}/{}", prefix.trim_matches('/'), uri.trim_matches('/'));
        let joined = joined.trim_matches('/');
        if joined.is_empty() {
            "/".to_string()
        } else {
            joined.to_string()
        }
    }

    /// Register the fallback route, run when no other route matches.
    pub fn fallback<H: Handler<T>, T: 'static>(&self, handler: H) -> RouteDefinition {
        self.add_route(
            &["GET", "HEAD"],
            "{fallbackPlaceholder}",
            handler.into_action(),
        )
        .where_("fallbackPlaceholder", ".*")
        .fallback()
    }

    /// Register a route that redirects to another URI (`302`). Parameters
    /// in the destination are filled from the route's parameters.
    pub fn redirect(&self, uri: &str, destination: &str) -> RouteDefinition {
        self.redirect_with_status(uri, destination, 302)
    }

    /// Register a redirect route with a specific status code.
    pub fn redirect_with_status(
        &self,
        uri: &str,
        destination: &str,
        status: u16,
    ) -> RouteDefinition {
        let destination = destination.to_string();
        let handler: RouteHandler = Arc::new(move |request: Request| {
            let destination = destination.clone();
            Box::pin(async move {
                if request.is_precognitive() {
                    return Ok(Precognition::success_response());
                }
                let url = fill_destination(&destination, &request)?;
                Ok(Response::redirect_with_status(url, status))
            })
        });
        self.add_route(&VERBS, uri, RouteAction::new("RedirectController", handler))
    }

    /// Register a route that permanently redirects (`301`).
    pub fn permanent_redirect(&self, uri: &str, destination: &str) -> RouteDefinition {
        self.redirect_with_status(uri, destination, 301)
    }

    /// Register a route that simply renders a view. The route's parameters
    /// are merged into the view data.
    pub fn view(&self, uri: &str, view: &str, data: Value) -> RouteDefinition {
        self.view_with_status(uri, view, data, 200)
    }

    /// Register a view route with a specific status code.
    pub fn view_with_status(
        &self,
        uri: &str,
        view: &str,
        data: Value,
        status: u16,
    ) -> RouteDefinition {
        let renderer = self.inner.view_renderer.clone();
        let view = view.to_string();
        let handler: RouteHandler = Arc::new(move |request: Request| {
            let renderer = renderer.read().unwrap().clone();
            let view = view.clone();
            let mut data = match &data {
                Value::Object(map) => map.clone(),
                _ => illuminate_support::Map::new(),
            };
            Box::pin(async move {
                if request.is_precognitive() {
                    return Ok(Precognition::success_response());
                }
                let renderer = renderer.ok_or_else(|| {
                    RuntimeException::new(
                        "No view renderer has been registered with the router. Did you register the ViewServiceProvider?",
                    )
                })?;
                for (key, value) in request.route_parameters() {
                    data.insert(key, Value::String(value));
                }
                Ok(match renderer(&view, Value::Object(data)) {
                    Ok(response) if status != 200 => response.with_status(status),
                    Ok(response) => response,
                    Err(error) => render_exception(error),
                })
            })
        });
        self.add_route(
            &["GET", "HEAD"],
            uri,
            RouteAction::new("ViewController", handler),
        )
    }

    /// Register a resource controller (`index`, `create`, `store`, `show`,
    /// `edit`, `update` and `destroy` routes). The routes are registered
    /// when the returned registration is dropped, after any options have
    /// been chained onto it.
    pub fn resource<C: ResourceController>(
        &self,
        name: &str,
        controller: C,
    ) -> PendingResourceRegistration {
        PendingResourceRegistration::new(self.clone(), name, Arc::new(controller), false)
    }

    /// Register an API resource controller (no `create` or `edit` routes).
    pub fn api_resource<C: ResourceController>(
        &self,
        name: &str,
        controller: C,
    ) -> PendingResourceRegistration {
        PendingResourceRegistration::new(self.clone(), name, Arc::new(controller), true)
    }

    /// Register many resource controllers at once.
    pub fn resources(&self, resources: Vec<(&str, Arc<dyn ResourceController>)>) {
        for (name, controller) in resources {
            PendingResourceRegistration::new(self.clone(), name, controller, false);
        }
    }

    /// Register many API resource controllers at once.
    pub fn api_resources(&self, resources: Vec<(&str, Arc<dyn ResourceController>)>) {
        for (name, controller) in resources {
            PendingResourceRegistration::new(self.clone(), name, controller, true);
        }
    }

    /// Register many resource controllers that allow soft deleted models
    /// to be bound (`with_trashed`) at once.
    pub fn soft_deletable_resources(&self, resources: Vec<(&str, Arc<dyn ResourceController>)>) {
        for (name, controller) in resources {
            PendingResourceRegistration::new(self.clone(), name, controller, false).with_trashed(&[]);
        }
    }

    /// Register many singleton resource controllers at once.
    pub fn singletons(&self, singletons: Vec<(&str, Arc<dyn ResourceController>)>) {
        for (name, controller) in singletons {
            PendingSingletonResourceRegistration::new(self.clone(), name, controller, false);
        }
    }

    /// Register many API singleton resource controllers at once.
    pub fn api_singletons(&self, singletons: Vec<(&str, Arc<dyn ResourceController>)>) {
        for (name, controller) in singletons {
            PendingSingletonResourceRegistration::new(self.clone(), name, controller, true);
        }
    }

    /// Register a singleton resource controller (`show`, `edit` and `update`).
    pub fn singleton<C: ResourceController>(
        &self,
        name: &str,
        controller: C,
    ) -> PendingSingletonResourceRegistration {
        PendingSingletonResourceRegistration::new(self.clone(), name, Arc::new(controller), false)
    }

    /// Register an API singleton resource controller (`show` and `update`).
    pub fn api_singleton<C: ResourceController>(
        &self,
        name: &str,
        controller: C,
    ) -> PendingSingletonResourceRegistration {
        PendingSingletonResourceRegistration::new(self.clone(), name, Arc::new(controller), true)
    }

    // ------------------------------------------------------------------
    // Groups
    // ------------------------------------------------------------------

    /// Create a route group with shared attributes.
    pub fn group(&self, attributes: GroupAttributes, routes: impl FnOnce()) {
        let merged = {
            let stack = self.inner.group_stack.lock().unwrap();
            match stack.last() {
                Some(last) => GroupAttributes::merge(&attributes, last, true),
                None => attributes,
            }
        };
        self.inner.group_stack.lock().unwrap().push(merged);
        let _guard = GroupGuard(&self.inner.group_stack);
        routes();
    }

    /// Determine if the router is currently inside a group.
    pub fn has_group_stack(&self) -> bool {
        !self.inner.group_stack.lock().unwrap().is_empty()
    }

    /// The current group stack.
    pub fn get_group_stack(&self) -> Vec<GroupAttributes> {
        self.inner.group_stack.lock().unwrap().clone()
    }

    /// The prefix of the innermost group.
    pub fn get_last_group_prefix(&self) -> String {
        self.inner
            .group_stack
            .lock()
            .unwrap()
            .last()
            .and_then(|group| group.prefix.clone())
            .unwrap_or_default()
    }

    /// Start a group (or route) with metadata.
    pub fn metadata(&self, metadata: Value) -> RouteRegistrar {
        RouteRegistrar::new(self.clone()).metadata(metadata)
    }

    /// Start a group (or route) with a URI prefix.
    pub fn prefix(&self, prefix: &str) -> RouteRegistrar {
        RouteRegistrar::new(self.clone()).prefix(prefix)
    }

    /// Start a group (or route) with middleware.
    pub fn middleware(&self, middleware: impl IntoMiddleware) -> RouteRegistrar {
        RouteRegistrar::new(self.clone()).middleware(middleware)
    }

    /// Start a group (or route) without the given middleware.
    pub fn without_middleware(&self, middleware: impl IntoMiddleware) -> RouteRegistrar {
        RouteRegistrar::new(self.clone()).without_middleware(middleware)
    }

    /// Start a group (or route) with a route name prefix.
    pub fn name(&self, name: &str) -> RouteRegistrar {
        RouteRegistrar::new(self.clone()).name(name)
    }

    /// Start a group (or route) restricted to a domain.
    pub fn domain(&self, domain: &str) -> RouteRegistrar {
        RouteRegistrar::new(self.clone()).domain(domain)
    }

    /// Start a group (or route) with a parameter constraint.
    pub fn where_(&self, parameter: &str, expression: &str) -> RouteRegistrar {
        RouteRegistrar::new(self.clone()).where_(parameter, expression)
    }

    // ------------------------------------------------------------------
    // Global patterns
    // ------------------------------------------------------------------

    /// Constrain every parameter with the given name, for routes registered
    /// from now on.
    pub fn pattern(&self, key: &str, pattern: &str) {
        self.inner
            .patterns
            .write()
            .unwrap()
            .insert(key.to_string(), pattern.to_string());
    }

    /// Register many global patterns.
    pub fn patterns<'a>(&self, patterns: impl IntoIterator<Item = (&'a str, &'a str)>) {
        for (key, pattern) in patterns {
            self.pattern(key, pattern);
        }
    }

    /// The global patterns.
    pub fn get_patterns(&self) -> IndexMap<String, String> {
        self.inner.patterns.read().unwrap().clone()
    }

    // ------------------------------------------------------------------
    // Middleware
    // ------------------------------------------------------------------

    /// Register a middleware alias with a factory receiving the alias's
    /// parameters (`"throttle:60,1"` → `["60", "1"]`).
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_http::{middleware_fn, Middleware};
    /// use illuminate_routing::Router;
    ///
    /// let router = Router::new();
    /// router.alias_middleware("role", |parameters| {
    ///     let role = parameters.first().cloned().unwrap_or_default();
    ///     middleware_fn(move |request, next| {
    ///         let role = role.clone();
    ///         async move {
    ///             request.set_attribute("role", role);
    ///             Ok(next.run(request).await)
    ///         }
    ///     })
    /// });
    /// assert!(router.has_middleware_alias("role"));
    /// ```
    pub fn alias_middleware(
        &self,
        name: &str,
        factory: impl Fn(&[String]) -> Arc<dyn Middleware> + Send + Sync + 'static,
    ) {
        self.alias_middleware_factory(name, Arc::new(factory));
    }

    /// Register a middleware alias with a shared factory.
    pub fn alias_middleware_factory(&self, name: &str, factory: MiddlewareFactory) {
        self.inner
            .aliases
            .write()
            .unwrap()
            .insert(name.to_string(), factory);
    }

    /// Register a middleware alias for a ready-made instance (any
    /// parameters are ignored).
    pub fn alias_middleware_instance(&self, name: &str, middleware: Arc<dyn Middleware>) {
        self.alias_middleware(name, move |_| middleware.clone());
    }

    /// Determine if a middleware alias exists.
    pub fn has_middleware_alias(&self, name: &str) -> bool {
        self.inner.aliases.read().unwrap().contains_key(name)
    }

    /// The registered middleware alias names.
    pub fn get_middleware_aliases(&self) -> Vec<String> {
        self.inner.aliases.read().unwrap().keys().cloned().collect()
    }

    /// Define (or replace) a middleware group.
    pub fn middleware_group(&self, name: &str, middleware: impl IntoMiddleware) {
        self.inner
            .groups
            .write()
            .unwrap()
            .insert(name.to_string(), middleware.into_middleware());
    }

    /// Add middleware to the end of a group (creating it if needed),
    /// skipping middleware already in the group.
    pub fn push_middleware_to_group(&self, group: &str, middleware: impl IntoMiddleware) {
        let mut groups = self.inner.groups.write().unwrap();
        let list = groups.entry(group.to_string()).or_default();
        for entry in middleware.into_middleware() {
            if !list.iter().any(|existing| existing.same_as(&entry)) {
                list.push(entry);
            }
        }
    }

    /// Add middleware to the beginning of an existing group, skipping
    /// middleware already in the group.
    pub fn prepend_middleware_to_group(&self, group: &str, middleware: impl IntoMiddleware) {
        let mut groups = self.inner.groups.write().unwrap();
        if let Some(list) = groups.get_mut(group) {
            for entry in middleware.into_middleware().into_iter().rev() {
                if !list.iter().any(|existing| existing.same_as(&entry)) {
                    list.insert(0, entry);
                }
            }
        }
    }

    /// Remove middleware from a group.
    pub fn remove_middleware_from_group(&self, group: &str, middleware: impl IntoMiddleware) {
        let mut groups = self.inner.groups.write().unwrap();
        if let Some(list) = groups.get_mut(group) {
            let remove = middleware.into_middleware();
            list.retain(|entry| !remove.iter().any(|r| r.same_as(entry)));
        }
    }

    /// Determine if a middleware group exists.
    pub fn has_middleware_group(&self, name: &str) -> bool {
        self.inner.groups.read().unwrap().contains_key(name)
    }

    /// The middleware groups, by name.
    pub fn get_middleware_groups(&self) -> IndexMap<String, Vec<String>> {
        self.inner
            .groups
            .read()
            .unwrap()
            .iter()
            .map(|(name, list)| {
                (
                    name.clone(),
                    list.iter().map(|m| m.name().to_string()).collect(),
                )
            })
            .collect()
    }

    /// Remove every middleware group.
    pub fn flush_middleware_groups(&self) {
        self.inner.groups.write().unwrap().clear();
    }

    /// Set the middleware priority list: middleware named here always run
    /// in this order relative to each other, wherever they were assigned.
    pub fn set_middleware_priority<S: Into<String>>(&self, priority: impl IntoIterator<Item = S>) {
        *self.inner.priority.write().unwrap() = priority.into_iter().map(Into::into).collect();
    }

    /// The middleware priority list.
    pub fn get_middleware_priority(&self) -> Vec<String> {
        self.inner.priority.read().unwrap().clone()
    }

    /// Expand groups, remove excluded middleware, sort by priority and
    /// remove duplicates.
    pub fn resolve_middleware(
        &self,
        middleware: &[RouteMiddleware],
        excluded: &[RouteMiddleware],
    ) -> Result<Vec<RouteMiddleware>> {
        let groups = self.inner.groups.read().unwrap().clone();

        let mut expanded = Vec::new();
        for entry in middleware {
            expand(entry, &groups, &mut expanded, &mut Vec::new())?;
        }

        let mut exclusions = Vec::new();
        for entry in excluded {
            expand(entry, &groups, &mut exclusions, &mut Vec::new())?;
        }

        let filtered: Vec<RouteMiddleware> = expanded
            .into_iter()
            .filter(|candidate| {
                !exclusions
                    .iter()
                    .any(|excluded| excluded.excludes(candidate))
            })
            .collect();

        let priority = self.inner.priority.read().unwrap().clone();
        Ok(sort_middleware(&priority, filtered))
    }

    /// The fully resolved middleware names for a route (groups expanded).
    pub fn gather_route_middleware_names(&self, route: &RouteDefinition) -> Result<Vec<String>> {
        Ok(self
            .resolve_middleware(&route.get_middleware(), &route.excluded_middleware())?
            .iter()
            .map(|m| m.name().to_string())
            .collect())
    }

    /// Resolve a route's middleware into instances, ready to run.
    pub fn gather_route_middleware(
        &self,
        route: &RouteDefinition,
    ) -> Result<Vec<Arc<dyn Middleware>>> {
        let resolved =
            self.resolve_middleware(&route.get_middleware(), &route.excluded_middleware())?;
        let resolved = unique_middleware(resolved);
        resolved
            .iter()
            .map(|entry| self.resolve_middleware_instance(entry))
            .collect()
    }

    /// Build the middleware instance for a name (via its alias) or return
    /// the instance itself.
    pub fn resolve_middleware_instance(
        &self,
        middleware: &RouteMiddleware,
    ) -> Result<Arc<dyn Middleware>> {
        match middleware {
            RouteMiddleware::Instance { middleware, .. } => Ok(middleware.clone()),
            RouteMiddleware::Name(name) => {
                let base = middleware.base_name();
                let factory = self.inner.aliases.read().unwrap().get(base).cloned();
                match factory {
                    Some(factory) => Ok(factory(&middleware.parameters())),
                    None => Err(MiddlewareNotFoundException { name: name.clone() }.into()),
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Hooks
    // ------------------------------------------------------------------

    /// Set the view renderer used by `Route::view` routes.
    pub fn set_view_renderer(&self, renderer: ViewRenderer) {
        *self.inner.view_renderer.write().unwrap() = Some(renderer);
    }

    /// Determine if a view renderer has been registered.
    pub fn has_view_renderer(&self) -> bool {
        self.inner.view_renderer.read().unwrap().is_some()
    }

    /// Teach the router which extraction errors mean "model not found",
    /// so routes' `missing` handlers run for them (a `404` `HttpException`
    /// always counts).
    pub fn set_missing_model_detector(
        &self,
        detector: impl Fn(&Error) -> bool + Send + Sync + 'static,
    ) {
        *self.inner.missing_detector.write().unwrap() = Some(Arc::new(detector));
    }

    /// Determine if the error means a bound model could not be found.
    pub fn is_missing_model_error(&self, error: &Error) -> bool {
        if let Some(detector) = self.inner.missing_detector.read().unwrap().clone()
            && detector(error)
        {
            return true;
        }
        error
            .downcast_ref::<HttpException>()
            .is_some_and(|e| e.status == 404)
    }

    /// Register a callback to run whenever a route is matched.
    pub fn matched(&self, callback: impl Fn(&CurrentRoute, &Request) + Send + Sync + 'static) {
        self.inner.matched.write().unwrap().push(Arc::new(callback));
    }

    // ------------------------------------------------------------------
    // Resources
    // ------------------------------------------------------------------

    /// Localize the `create` and `edit` URI verbs of resource routes.
    pub fn resource_verbs(&self, create: &str, edit: &str) {
        *self.inner.resource_verbs.write().unwrap() = (create.to_string(), edit.to_string());
    }

    /// The `create` and `edit` URI verbs.
    pub fn get_resource_verbs(&self) -> (String, String) {
        self.inner.resource_verbs.read().unwrap().clone()
    }

    /// Set global resource parameter names (`"users" => "admin_user"`).
    pub fn resource_parameters<'a>(
        &self,
        parameters: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) {
        *self.inner.resource_parameters.write().unwrap() = parameters
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
    }

    pub(crate) fn get_resource_parameters(&self) -> IndexMap<String, String> {
        self.inner.resource_parameters.read().unwrap().clone()
    }

    /// Set whether resource parameters are singularized (`photos` → `{photo}`).
    pub fn singular_resource_parameters(&self, singular: bool) {
        self.inner
            .singular_resource_parameters
            .store(singular, Ordering::SeqCst);
    }

    pub(crate) fn uses_singular_resource_parameters(&self) -> bool {
        self.inner
            .singular_resource_parameters
            .load(Ordering::SeqCst)
    }

    // ------------------------------------------------------------------
    // Inspecting routes
    // ------------------------------------------------------------------

    /// Every registered route, in registration order.
    pub fn get_routes(&self) -> Vec<RouteDefinition> {
        self.inner.routes.read().unwrap().clone()
    }

    /// The number of registered routes.
    pub fn count(&self) -> usize {
        self.inner.routes.read().unwrap().len()
    }

    /// The routes as displayed by `route:list`.
    pub fn route_list(&self) -> Vec<RouteListing> {
        self.get_routes()
            .into_iter()
            .map(|route| {
                let excluded = route.excluded_middleware();
                RouteListing {
                    domain: route.get_domain(),
                    methods: route.methods(),
                    uri: route.uri(),
                    name: route.get_name(),
                    action: route.action_name(),
                    middleware: route
                        .get_middleware()
                        .iter()
                        .filter(|m| !excluded.iter().any(|e| e.excludes(m)))
                        .map(|m| m.name().to_string())
                        .collect(),
                }
            })
            .collect()
    }

    /// Find a route by name.
    pub fn get_by_name(&self, name: &str) -> Option<RouteDefinition> {
        let version = self.inner.changes.load(Ordering::SeqCst);
        if let Some((cached, names)) = self.inner.names.read().unwrap().as_ref()
            && *cached == version
        {
            return names.get(name).cloned();
        }
        let mut names = HashMap::new();
        for route in self.inner.routes.read().unwrap().iter() {
            if let Some(route_name) = route.get_name() {
                names.entry(route_name).or_insert_with(|| route.clone());
            }
        }
        let found = names.get(name).cloned();
        *self.inner.names.write().unwrap() = Some((version, names));
        found
    }

    /// Find the first route using the given action (`"UserController@show"`).
    pub fn get_by_action(&self, action: &str) -> Option<RouteDefinition> {
        self.get_routes()
            .into_iter()
            .find(|route| route.action_name() == action)
    }

    /// Determine if a route with the given name exists.
    pub fn has(&self, name: &str) -> bool {
        self.get_by_name(name).is_some()
    }

    /// Determine if routes exist for all of the given names.
    pub fn has_all(&self, names: &[&str]) -> bool {
        names.iter().all(|name| self.has(name))
    }

    // ------------------------------------------------------------------
    // Dispatching
    // ------------------------------------------------------------------

    /// Dispatch the request to the matching route and return its response.
    ///
    /// The request becomes the "current request" while it is handled. A
    /// `404` is returned when nothing matches, a `405` (with an `Allow`
    /// header) when only other verbs match, and `OPTIONS` requests are
    /// answered automatically. Errors are rendered by the exception handler.
    pub async fn dispatch(&self, request: Request) -> Response {
        let router = self.clone();
        with_request(request.clone(), async move {
            router.dispatch_to_route(request).await
        })
        .await
    }

    async fn dispatch_to_route(&self, request: Request) -> Response {
        match self.find_route(&request) {
            Ok((route, parameters)) => self.run_route(request, route, parameters).await,
            Err(error) => render_exception(error),
        }
    }

    /// The router as a pipeline destination (for the HTTP kernel).
    pub fn as_destination(&self) -> Destination {
        let router = self.clone();
        Arc::new(move |request: Request| {
            let router = router.clone();
            Box::pin(async move { router.dispatch(request).await })
        })
    }

    /// Find the route matching the request, with its parameters.
    pub fn find_route(
        &self,
        request: &Request,
    ) -> Result<(RouteDefinition, IndexMap<String, String>)> {
        let method = request.method().as_str().to_ascii_uppercase();
        let path = crate::compiled::normalize_path(&request.decoded_path());
        let host = request.host();
        let secure = request.secure();

        let routes = self.inner.routes.read().unwrap();
        if let Some(found) = match_against(&routes, &method, &path, &host, secure, true)? {
            return Ok(found);
        }

        let mut others = Vec::new();
        for verb in VERBS.iter().filter(|verb| **verb != method) {
            let candidates: Vec<RouteDefinition> = routes
                .iter()
                .filter(|route| route.has_method(verb))
                .cloned()
                .collect();
            if match_against(&candidates, verb, &path, &host, secure, false)?.is_some() {
                others.push(verb.to_string());
            }
        }
        drop(routes);

        if others.is_empty() {
            return Err(not_found(&request.path()));
        }

        if method == "OPTIONS" {
            let allow = others.join(",");
            let handler: RouteHandler = Arc::new(move |_request: Request| {
                let allow = allow.clone();
                Box::pin(async move { Ok(Response::new("").with_header("Allow", &allow)) })
            });
            let route = RouteDefinition::new(
                &["OPTIONS"],
                &request.path(),
                RouteAction::new("Closure", handler),
            );
            return Ok((route, IndexMap::new()));
        }

        Err(method_not_allowed(&method, &request.path(), &others))
    }

    /// Run the given (matched) route for the request: bind its parameters,
    /// send the request through its middleware, then call its handler.
    pub async fn run_route(
        &self,
        request: Request,
        route: RouteDefinition,
        parameters: IndexMap<String, String>,
    ) -> Response {
        request.set_route(route.get_name(), parameters.clone());
        let current = Arc::new(CurrentRoute::new(route.clone(), parameters));
        request.set_extension(current.clone());

        let callbacks = self.inner.matched.read().unwrap().clone();
        for callback in callbacks {
            callback(&current, &request);
        }

        let middleware = match self.gather_route_middleware(&route) {
            Ok(middleware) => middleware,
            Err(error) => return render_exception(error),
        };
        request.set_extension(Arc::new(RouteMiddlewareStack(middleware.clone())));

        let handler = route.handler();
        let missing = route.missing_handler();
        let router = self.clone();
        let destination: Destination = Arc::new(move |request: Request| {
            let handler = handler.clone();
            let missing = missing.clone();
            let router = router.clone();
            Box::pin(async move {
                match handler(request.clone()).await {
                    Ok(response) => response,
                    Err(error) => match missing {
                        Some(missing) if router.is_missing_model_error(&error) => {
                            missing(request).await.unwrap_or_else(render_exception)
                        }
                        _ => render_exception(error),
                    },
                }
            })
        });

        illuminate_http::build_pipeline(middleware, destination)(request).await
    }

    /// Call `terminate` on the route middleware that handled the request.
    pub async fn terminate(&self, request: &Request, response: &Response) {
        if let Some(stack) = request.extension::<RouteMiddlewareStack>() {
            for middleware in stack.0.iter() {
                middleware.terminate(request, response).await;
            }
        }
    }

    /// The route matched for the current request.
    pub fn current(&self) -> Option<Arc<CurrentRoute>> {
        illuminate_http::current_request().and_then(|request| request.extension::<CurrentRoute>())
    }

    /// The name of the route matched for the current request.
    pub fn current_route_name(&self) -> Option<String> {
        self.current().and_then(|route| route.name())
    }

    /// The action of the route matched for the current request.
    pub fn current_route_action(&self) -> Option<String> {
        self.current().map(|route| route.action_name())
    }

    /// Determine if the current route's action matches any of the patterns
    /// (`"UserController@*"`).
    pub fn uses(&self, patterns: &[&str]) -> bool {
        self.current_route_action()
            .is_some_and(|action| patterns.iter().any(|pattern| Str::is(pattern, &action)))
    }

    /// Determine if the current route's action is exactly the given one.
    pub fn current_route_uses(&self, action: &str) -> bool {
        self.current_route_action().as_deref() == Some(action)
    }

    /// Determine if the current route's name matches any of the patterns.
    pub fn current_route_named(&self, patterns: &[&str]) -> bool {
        self.current().is_some_and(|route| route.named(patterns))
    }
}

fn match_against(
    routes: &[RouteDefinition],
    method: &str,
    path: &str,
    host: &str,
    secure: bool,
    including_method: bool,
) -> Result<Option<(RouteDefinition, IndexMap<String, String>)>> {
    let mut fallback = None;
    for route in routes {
        if let Some(parameters) =
            route.matches_parts(method, path, host, secure, including_method)?
        {
            if route.is_fallback() {
                if fallback.is_none() {
                    fallback = Some((route.clone(), parameters));
                }
                continue;
            }
            return Ok(Some((route.clone(), parameters)));
        }
    }
    Ok(fallback)
}

fn expand(
    entry: &RouteMiddleware,
    groups: &IndexMap<String, Vec<RouteMiddleware>>,
    out: &mut Vec<RouteMiddleware>,
    stack: &mut Vec<String>,
) -> Result<()> {
    match entry {
        RouteMiddleware::Name(name) if groups.contains_key(name) => {
            if stack.contains(name) {
                return Err(RecursiveMiddlewareGroupException {
                    group: name.clone(),
                }
                .into());
            }
            stack.push(name.clone());
            for member in &groups[name] {
                expand(member, groups, out, stack)?;
            }
            stack.pop();
        }
        other => out.push(other.clone()),
    }
    Ok(())
}

static DESTINATION_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{(\w+)(\??)\}").expect("valid regex"));

/// Fill the `{parameters}` of a redirect destination from the request's
/// route parameters.
fn fill_destination(destination: &str, request: &Request) -> Result<String> {
    let parameters = request.route_parameters();
    let mut missing = Vec::new();
    let filled =
        DESTINATION_PLACEHOLDER.replace_all(destination, |captures: &regex::Captures<'_>| {
            match parameters.get(&captures[1]) {
                Some(value) => crate::url::raw_url_encode(value),
                None if &captures[2] == "?" => String::new(),
                None => {
                    missing.push(captures[1].to_string());
                    captures[0].to_string()
                }
            }
        });
    if !missing.is_empty() {
        return Err(
            UrlGenerationException::for_missing_parameters(None, destination, &missing).into(),
        );
    }
    // Optional parameters left out leave empty segments behind; collapse
    // them (without touching the `scheme://` of absolute destinations).
    let (scheme, rest) = match filled.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest.to_string()),
        None => (String::new(), filled.into_owned()),
    };
    let mut rest = REPEATED_SLASHES.replace_all(&rest, "/").into_owned();
    if rest.len() > 1 && rest.ends_with('/') && destination.starts_with('/') {
        rest = rest.trim_end_matches('/').to_string();
    }
    Ok(format!("{scheme}{rest}"))
}

static REPEATED_SLASHES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("/{2,}").expect("valid regex"));

/// Get the router from the container, registering one if the application
/// hasn't bound it yet.
pub fn router() -> Arc<Router> {
    if let Some(router) = try_app::<Router>() {
        return router;
    }
    let container = Container::get_instance();
    container.singleton_if::<Router>(|_| Arc::new(Router::new()));
    container.make::<Router>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_attributes_merge_like_laravel() {
        let outer = GroupAttributes {
            prefix: Some("admin".into()),
            name: Some("admin.".into()),
            middleware: vec![RouteMiddleware::named("auth")],
            wheres: [("id".to_string(), "[0-9]+".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let inner = GroupAttributes {
            prefix: Some("users".into()),
            name: Some("users.".into()),
            middleware: vec![RouteMiddleware::named("verified")],
            ..Default::default()
        };
        let merged = GroupAttributes::merge(&inner, &outer, true);
        assert_eq!(merged.prefix.as_deref(), Some("admin/users"));
        assert_eq!(merged.name.as_deref(), Some("admin.users."));
        assert_eq!(merged.middleware.len(), 2);
        assert_eq!(merged.wheres["id"], "[0-9]+");
    }

    #[test]
    fn redirect_destinations_are_filled_from_parameters() {
        let request = Request::create("/here/5", "GET");
        request.set_route_parameter("id", "5");
        assert_eq!(
            fill_destination("/there/{id}", &request).unwrap(),
            "/there/5"
        );
        assert_eq!(
            fill_destination("/there/{id}/{tab?}", &request).unwrap(),
            "/there/5"
        );
        assert_eq!(
            fill_destination("https://laravel.com", &request).unwrap(),
            "https://laravel.com"
        );
        assert_eq!(
            fill_destination("https://laravel.com/{tab?}/{id}", &request).unwrap(),
            "https://laravel.com/5"
        );
        assert!(fill_destination("/there/{user}", &request).is_err());
    }
}
