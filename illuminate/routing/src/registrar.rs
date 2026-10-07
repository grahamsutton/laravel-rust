//! Fluent route group attributes: `Route::prefix("admin").middleware("auth").group(...)`.

use std::sync::Arc;

use illuminate_support::Value;

use crate::handler::Handler;
use crate::middleware::IntoMiddleware;
use crate::resource::{
    PendingResourceRegistration, PendingSingletonResourceRegistration, ResourceController,
};
use crate::route::{RouteDefinition, patterns};
use crate::router::{GroupAttributes, Router};

/// Collects group attributes before defining a group (or a single route).
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_routing::Route;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Route::prefix("admin")
///     .middleware(["auth", "verified"])
///     .name("admin.")
///     .where_number("id")
///     .group(|| {
///         Route::get("/users/{id}", || async { "User" }).name("users.show");
///     });
///
/// let route = Route::get_by_name("admin.users.show").unwrap();
/// assert_eq!(route.uri(), "admin/users/{id}");
/// assert_eq!(route.middleware_names(), vec!["auth", "verified"]);
/// assert_eq!(route.wheres()["id"], "[0-9]+");
/// ```
#[derive(Clone, Debug)]
pub struct RouteRegistrar {
    router: Router,
    attributes: GroupAttributes,
}

impl RouteRegistrar {
    /// Start collecting attributes for the given router.
    pub fn new(router: Router) -> Self {
        Self {
            router,
            attributes: GroupAttributes::default(),
        }
    }

    /// The attributes collected so far.
    pub fn attributes(&self) -> &GroupAttributes {
        &self.attributes
    }

    /// Prefix the URIs of the group's routes.
    pub fn prefix(mut self, prefix: &str) -> Self {
        self.attributes.prefix = Some(prefix.to_string());
        self
    }

    /// Prefix the names of the group's routes (include the trailing `.`).
    pub fn name(mut self, name: &str) -> Self {
        let existing = self.attributes.name.take().unwrap_or_default();
        self.attributes.name = Some(format!("{existing}{name}"));
        self
    }

    /// Alias of [`RouteRegistrar::name`], matching Laravel's `as` attribute.
    pub fn as_(self, name: &str) -> Self {
        self.name(name)
    }

    /// Restrict the group's routes to a domain (`"{account}.example.com"`).
    pub fn domain(mut self, domain: &str) -> Self {
        self.attributes.domain = Some(domain.to_string());
        self
    }

    /// Attach middleware to the group's routes.
    pub fn middleware(mut self, middleware: impl IntoMiddleware) -> Self {
        self.attributes
            .middleware
            .extend(middleware.into_middleware());
        self
    }

    /// Remove middleware from the group's routes.
    pub fn without_middleware(mut self, middleware: impl IntoMiddleware) -> Self {
        self.attributes
            .excluded_middleware
            .extend(middleware.into_middleware());
        self
    }

    /// Authorize the group's routes with the `can` middleware.
    pub fn can(self, ability: &str, models: &str) -> Self {
        let name = if models.is_empty() {
            format!("can:{ability}")
        } else {
            format!("can:{ability},{models}")
        };
        self.middleware(name)
    }

    /// Constrain a parameter for the group's routes.
    pub fn where_(mut self, parameter: &str, expression: &str) -> Self {
        self.attributes
            .wheres
            .insert(parameter.to_string(), expression.to_string());
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

    /// Handle missing models in the group's routes with the given handler.
    pub fn missing<H: Handler<T>, T: 'static>(mut self, handler: H) -> Self {
        self.attributes.missing = Some(handler.into_missing_handler());
        self
    }

    /// Scope nested bindings in the group's routes.
    pub fn scope_bindings(mut self) -> Self {
        self.attributes.scope_bindings = Some(true);
        self
    }

    /// Never scope nested bindings in the group's routes.
    pub fn without_scoped_bindings(mut self) -> Self {
        self.attributes.scope_bindings = Some(false);
        self
    }

    /// Define the group: every route registered inside the callback
    /// receives the collected attributes.
    pub fn group(self, routes: impl FnOnce()) {
        self.router.group(self.attributes, routes);
    }

    fn register(&self, callback: impl FnOnce(&Router) -> RouteDefinition) -> RouteDefinition {
        let mut route = None;
        self.router.group(self.attributes.clone(), || {
            route = Some(callback(&self.router))
        });
        route.expect("the route was registered inside the group")
    }

    /// Register a `GET` route with the collected attributes.
    pub fn get<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.register(|router| router.get(uri, handler))
    }

    /// Register a `POST` route with the collected attributes.
    pub fn post<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.register(|router| router.post(uri, handler))
    }

    /// Register a `PUT` route with the collected attributes.
    pub fn put<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.register(|router| router.put(uri, handler))
    }

    /// Register a `PATCH` route with the collected attributes.
    pub fn patch<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.register(|router| router.patch(uri, handler))
    }

    /// Register a `DELETE` route with the collected attributes.
    pub fn delete<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.register(|router| router.delete(uri, handler))
    }

    /// Register an `OPTIONS` route with the collected attributes.
    pub fn options<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.register(|router| router.options(uri, handler))
    }

    /// Register a route for every verb with the collected attributes.
    pub fn any<H: Handler<T>, T: 'static>(&self, uri: &str, handler: H) -> RouteDefinition {
        self.register(|router| router.any(uri, handler))
    }

    /// Register a route for the given verbs with the collected attributes.
    pub fn match_<H: Handler<T>, T: 'static>(
        &self,
        methods: &[&str],
        uri: &str,
        handler: H,
    ) -> RouteDefinition {
        self.register(|router| router.match_(methods, uri, handler))
    }

    /// Register a redirect route with the collected attributes.
    pub fn redirect(&self, uri: &str, destination: &str) -> RouteDefinition {
        self.register(|router| router.redirect(uri, destination))
    }

    /// Register a view route with the collected attributes.
    pub fn view(&self, uri: &str, view: &str, data: Value) -> RouteDefinition {
        self.register(|router| router.view(uri, view, data))
    }

    /// Register the fallback route with the collected attributes.
    pub fn fallback<H: Handler<T>, T: 'static>(&self, handler: H) -> RouteDefinition {
        self.register(|router| router.fallback(handler))
    }

    /// Register a resource controller with the collected attributes.
    pub fn resource<C: ResourceController>(
        &self,
        name: &str,
        controller: C,
    ) -> PendingResourceRegistration {
        PendingResourceRegistration::new(self.router.clone(), name, Arc::new(controller), false)
            .within(self.attributes.clone())
    }

    /// Register an API resource controller with the collected attributes.
    pub fn api_resource<C: ResourceController>(
        &self,
        name: &str,
        controller: C,
    ) -> PendingResourceRegistration {
        PendingResourceRegistration::new(self.router.clone(), name, Arc::new(controller), true)
            .within(self.attributes.clone())
    }

    /// Register a singleton resource controller with the collected attributes.
    pub fn singleton<C: ResourceController>(
        &self,
        name: &str,
        controller: C,
    ) -> PendingSingletonResourceRegistration {
        PendingSingletonResourceRegistration::new(
            self.router.clone(),
            name,
            Arc::new(controller),
            false,
        )
        .within(self.attributes.clone())
    }
}
