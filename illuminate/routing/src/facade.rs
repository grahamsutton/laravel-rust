//! The `Route` facade.

use std::sync::Arc;

use illuminate_http::{Middleware, Request};
use illuminate_support::{Error, Value};

use crate::handler::Handler;
use crate::middleware::IntoMiddleware;
use crate::registrar::RouteRegistrar;
use crate::resource::{
    PendingResourceRegistration, PendingSingletonResourceRegistration, ResourceController,
};
use crate::route::{CurrentRoute, RouteDefinition, RouteListing};
use crate::router::{GroupAttributes, Router, ViewRenderer, router};

/// The `Route` facade: register routes and inspect the router.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_routing::{Path, Route};
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Route::get("/greeting", || async { "Hello World" });
///
/// Route::get("/user/{id}", |Path(id): Path<u64>| async move { format!("User {id}") })
///     .where_number("id")
///     .name("users.show");
///
/// Route::middleware("auth").prefix("admin").name("admin.").group(|| {
///     Route::get("/users", || async { "Users" }).name("users");
/// });
///
/// assert!(Route::has("users.show"));
/// assert!(Route::has("admin.users"));
/// assert_eq!(Route::get_routes().len(), 3);
/// ```
pub struct Route;

impl Route {
    /// The router behind the facade.
    pub fn router() -> Arc<Router> {
        router()
    }

    // ------------------------------------------------------------------
    // Registering routes
    // ------------------------------------------------------------------

    /// Register a `GET` (and `HEAD`) route.
    pub fn get<H: Handler<T>, T: 'static>(uri: &str, handler: H) -> RouteDefinition {
        router().get(uri, handler)
    }

    /// Register a `POST` route.
    pub fn post<H: Handler<T>, T: 'static>(uri: &str, handler: H) -> RouteDefinition {
        router().post(uri, handler)
    }

    /// Register a `PUT` route.
    pub fn put<H: Handler<T>, T: 'static>(uri: &str, handler: H) -> RouteDefinition {
        router().put(uri, handler)
    }

    /// Register a `PATCH` route.
    pub fn patch<H: Handler<T>, T: 'static>(uri: &str, handler: H) -> RouteDefinition {
        router().patch(uri, handler)
    }

    /// Register a `DELETE` route.
    pub fn delete<H: Handler<T>, T: 'static>(uri: &str, handler: H) -> RouteDefinition {
        router().delete(uri, handler)
    }

    /// Register an `OPTIONS` route.
    pub fn options<H: Handler<T>, T: 'static>(uri: &str, handler: H) -> RouteDefinition {
        router().options(uri, handler)
    }

    /// Register a `QUERY` route.
    pub fn query<H: Handler<T>, T: 'static>(uri: &str, handler: H) -> RouteDefinition {
        router().query(uri, handler)
    }

    /// Register a route responding to every verb.
    pub fn any<H: Handler<T>, T: 'static>(uri: &str, handler: H) -> RouteDefinition {
        router().any(uri, handler)
    }

    /// Register a route responding to the given verbs.
    pub fn match_<H: Handler<T>, T: 'static>(
        methods: &[&str],
        uri: &str,
        handler: H,
    ) -> RouteDefinition {
        router().match_(methods, uri, handler)
    }

    /// Register a route redirecting to another URI (`302`).
    pub fn redirect(uri: &str, destination: &str) -> RouteDefinition {
        router().redirect(uri, destination)
    }

    /// Register a redirect route with a specific status code.
    pub fn redirect_with_status(uri: &str, destination: &str, status: u16) -> RouteDefinition {
        router().redirect_with_status(uri, destination, status)
    }

    /// Register a route permanently redirecting to another URI (`301`).
    pub fn permanent_redirect(uri: &str, destination: &str) -> RouteDefinition {
        router().permanent_redirect(uri, destination)
    }

    /// Register a route that renders a view.
    pub fn view(uri: &str, view: &str, data: Value) -> RouteDefinition {
        router().view(uri, view, data)
    }

    /// Register a view route with a specific status code.
    pub fn view_with_status(uri: &str, view: &str, data: Value, status: u16) -> RouteDefinition {
        router().view_with_status(uri, view, data, status)
    }

    /// Register the fallback route, run when no other route matches.
    pub fn fallback<H: Handler<T>, T: 'static>(handler: H) -> RouteDefinition {
        router().fallback(handler)
    }

    /// Register a resource controller.
    pub fn resource<C: ResourceController>(
        name: &str,
        controller: C,
    ) -> PendingResourceRegistration {
        router().resource(name, controller)
    }

    /// Register an API resource controller (no `create` or `edit`).
    pub fn api_resource<C: ResourceController>(
        name: &str,
        controller: C,
    ) -> PendingResourceRegistration {
        router().api_resource(name, controller)
    }

    /// Register many resource controllers.
    pub fn resources(resources: Vec<(&str, Arc<dyn ResourceController>)>) {
        router().resources(resources)
    }

    /// Register many API resource controllers.
    pub fn api_resources(resources: Vec<(&str, Arc<dyn ResourceController>)>) {
        router().api_resources(resources)
    }

    /// Register a singleton resource controller.
    pub fn singleton<C: ResourceController>(
        name: &str,
        controller: C,
    ) -> PendingSingletonResourceRegistration {
        router().singleton(name, controller)
    }

    /// Register an API singleton resource controller.
    pub fn api_singleton<C: ResourceController>(
        name: &str,
        controller: C,
    ) -> PendingSingletonResourceRegistration {
        router().api_singleton(name, controller)
    }

    // ------------------------------------------------------------------
    // Groups
    // ------------------------------------------------------------------

    /// Create a route group with the given attributes.
    pub fn group(attributes: GroupAttributes, routes: impl FnOnce()) {
        router().group(attributes, routes)
    }

    /// Prefix a group of routes.
    pub fn prefix(prefix: &str) -> RouteRegistrar {
        router().prefix(prefix)
    }

    /// Attach middleware to a group of routes.
    pub fn middleware(middleware: impl IntoMiddleware) -> RouteRegistrar {
        router().middleware(middleware)
    }

    /// Remove middleware from a group of routes.
    pub fn without_middleware(middleware: impl IntoMiddleware) -> RouteRegistrar {
        router().without_middleware(middleware)
    }

    /// Prefix the names of a group of routes.
    pub fn name(name: &str) -> RouteRegistrar {
        router().name(name)
    }

    /// Alias of [`Route::name`].
    pub fn as_(name: &str) -> RouteRegistrar {
        router().name(name)
    }

    /// Restrict a group of routes to a domain.
    pub fn domain(domain: &str) -> RouteRegistrar {
        router().domain(domain)
    }

    /// Constrain a parameter for a group of routes.
    pub fn where_(parameter: &str, expression: &str) -> RouteRegistrar {
        router().where_(parameter, expression)
    }

    /// Scope nested bindings for a group of routes.
    pub fn scope_bindings() -> RouteRegistrar {
        RouteRegistrar::new((*router()).clone()).scope_bindings()
    }

    /// Authorize a group of routes with the `can` middleware.
    pub fn can(ability: &str, models: &str) -> RouteRegistrar {
        RouteRegistrar::new((*router()).clone()).can(ability, models)
    }

    // ------------------------------------------------------------------
    // Patterns & middleware
    // ------------------------------------------------------------------

    /// Constrain every parameter with this name to a regular expression.
    pub fn pattern(key: &str, pattern: &str) {
        router().pattern(key, pattern)
    }

    /// Register many global patterns.
    pub fn patterns<'a>(patterns: impl IntoIterator<Item = (&'a str, &'a str)>) {
        router().patterns(patterns)
    }

    /// Register a middleware alias.
    pub fn alias_middleware(
        name: &str,
        factory: impl Fn(&[String]) -> Arc<dyn Middleware> + Send + Sync + 'static,
    ) {
        router().alias_middleware(name, factory)
    }

    /// Register a middleware group.
    pub fn middleware_group(name: &str, middleware: impl IntoMiddleware) {
        router().middleware_group(name, middleware)
    }

    /// Add middleware to the end of a group.
    pub fn push_middleware_to_group(group: &str, middleware: impl IntoMiddleware) {
        router().push_middleware_to_group(group, middleware)
    }

    /// Add middleware to the beginning of a group.
    pub fn prepend_middleware_to_group(group: &str, middleware: impl IntoMiddleware) {
        router().prepend_middleware_to_group(group, middleware)
    }

    /// Remove middleware from a group.
    pub fn remove_middleware_from_group(group: &str, middleware: impl IntoMiddleware) {
        router().remove_middleware_from_group(group, middleware)
    }

    /// Set the view renderer used by `Route::view`.
    pub fn set_view_renderer(renderer: ViewRenderer) {
        router().set_view_renderer(renderer)
    }

    /// Teach the router which errors mean "model not found".
    pub fn set_missing_model_detector(detector: impl Fn(&Error) -> bool + Send + Sync + 'static) {
        router().set_missing_model_detector(detector)
    }

    /// Register a callback to run whenever a route is matched.
    pub fn matched(callback: impl Fn(&CurrentRoute, &Request) + Send + Sync + 'static) {
        router().matched(callback)
    }

    /// Localize the `create` and `edit` resource URI verbs.
    pub fn resource_verbs(create: &str, edit: &str) {
        router().resource_verbs(create, edit)
    }

    /// Set global resource parameter names.
    pub fn resource_parameters<'a>(parameters: impl IntoIterator<Item = (&'a str, &'a str)>) {
        router().resource_parameters(parameters)
    }

    // ------------------------------------------------------------------
    // Inspection
    // ------------------------------------------------------------------

    /// Determine if a route with the given name exists.
    pub fn has(name: &str) -> bool {
        router().has(name)
    }

    /// Determine if routes exist for all of the given names.
    pub fn has_all(names: &[&str]) -> bool {
        router().has_all(names)
    }

    /// Find a route by name.
    pub fn get_by_name(name: &str) -> Option<RouteDefinition> {
        router().get_by_name(name)
    }

    /// Every registered route.
    pub fn get_routes() -> Vec<RouteDefinition> {
        router().get_routes()
    }

    /// The routes as displayed by `route:list`.
    pub fn routes() -> Vec<RouteListing> {
        router().route_list()
    }

    /// The route matched for the current request.
    pub fn current() -> Option<Arc<CurrentRoute>> {
        illuminate_http::current_request().and_then(|request| request.extension::<CurrentRoute>())
    }

    /// The name of the route matched for the current request.
    pub fn current_route_name() -> Option<String> {
        Self::current().and_then(|route| route.name())
    }

    /// The action of the route matched for the current request.
    pub fn current_route_action() -> Option<String> {
        Self::current().map(|route| route.action_name())
    }

    /// Determine if the current route's name matches the pattern (`"admin.*"`).
    pub fn is(pattern: &str) -> bool {
        Self::current_route_named(&[pattern])
    }

    /// Determine if the current route's name matches any of the patterns.
    pub fn current_route_named(patterns: &[&str]) -> bool {
        Self::current().is_some_and(|route| route.named(patterns))
    }

    /// Get a parameter of the current route.
    pub fn input(name: &str) -> Option<String> {
        Self::current().and_then(|route| route.parameter(name).map(str::to_string))
    }
}
