//! Resource controllers: CRUD routes with a single line of code.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_container::Container;
//! use illuminate_http::{Request, Response};
//! use illuminate_routing::{async_trait, ResourceController, Route};
//! use illuminate_support::Result;
//!
//! struct PhotoController;
//!
//! #[async_trait]
//! impl ResourceController for PhotoController {
//!     async fn index(&self, _request: Request) -> Result<Response> {
//!         Ok(Response::new("All photos"))
//!     }
//!
//!     async fn show(&self, request: Request) -> Result<Response> {
//!         Ok(Response::new(format!("Photo {}", request.route_or("photo", ""))))
//!     }
//! }
//!
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container);
//!
//! Route::resource("photos", PhotoController);
//!
//! assert!(Route::has("photos.index"));
//! assert_eq!(Route::get_by_name("photos.edit").unwrap().uri(), "photos/{photo}/edit");
//! assert_eq!(Route::get_by_name("photos.show").unwrap().action_name(), "PhotoController@show");
//! ```

use std::sync::Arc;

use indexmap::IndexMap;

use illuminate_http::{HttpException, Request, Response, async_trait};
use illuminate_support::{Result, Str};

use crate::handler::Handler;
use crate::middleware::{IntoMiddleware, RouteMiddleware, unique_middleware};
use crate::route::{RouteAction, RouteDefinition, RouteHandler, patterns};
use crate::router::{GroupAttributes, Router};

/// A controller handling the typical "CRUD" actions for a resource.
///
/// Every action defaults to a `404`, so implement only the ones you need.
/// Errors returned by an action are rendered by the exception handler — or,
/// for "model not found" errors, by the resource's `missing` handler.
#[async_trait]
pub trait ResourceController: Send + Sync + 'static {
    /// Display a listing of the resource.
    async fn index(&self, _request: Request) -> Result<Response> {
        Err(HttpException::new(404).into())
    }

    /// Show the form for creating a new resource.
    async fn create(&self, _request: Request) -> Result<Response> {
        Err(HttpException::new(404).into())
    }

    /// Store a newly created resource.
    async fn store(&self, _request: Request) -> Result<Response> {
        Err(HttpException::new(404).into())
    }

    /// Display the specified resource.
    async fn show(&self, _request: Request) -> Result<Response> {
        Err(HttpException::new(404).into())
    }

    /// Show the form for editing the specified resource.
    async fn edit(&self, _request: Request) -> Result<Response> {
        Err(HttpException::new(404).into())
    }

    /// Update the specified resource.
    async fn update(&self, _request: Request) -> Result<Response> {
        Err(HttpException::new(404).into())
    }

    /// Remove the specified resource.
    async fn destroy(&self, _request: Request) -> Result<Response> {
        Err(HttpException::new(404).into())
    }

    /// The controller's display name for `route:list` (its type name).
    fn controller_name(&self) -> String {
        illuminate_support::class_basename::<Self>()
    }
}

const RESOURCE_DEFAULTS: [&str; 7] = ["index", "create", "store", "show", "edit", "update", "destroy"];
const API_RESOURCE_METHODS: [&str; 5] = ["index", "show", "store", "update", "destroy"];
const SINGLETON_DEFAULTS: [&str; 3] = ["show", "edit", "update"];
const API_SINGLETON_METHODS: [&str; 4] = ["store", "show", "update", "destroy"];

#[derive(Clone, Default)]
struct ResourceOptions {
    only: Option<Vec<String>>,
    except: Vec<String>,
    names: IndexMap<String, String>,
    parameters: IndexMap<String, String>,
    middleware: Vec<RouteMiddleware>,
    middleware_for: IndexMap<String, Vec<RouteMiddleware>>,
    excluded: Vec<RouteMiddleware>,
    excluded_for: IndexMap<String, Vec<RouteMiddleware>>,
    wheres: IndexMap<String, String>,
    shallow: bool,
    missing: Option<RouteHandler>,
    binding_fields: Option<IndexMap<String, String>>,
    trashed: Option<Vec<String>>,
    creatable: bool,
    destroyable: bool,
}

struct Pending {
    router: Router,
    name: String,
    controller: Arc<dyn ResourceController>,
    options: ResourceOptions,
    group: Option<GroupAttributes>,
    singleton: bool,
    registered: bool,
}

impl Pending {
    fn new(router: Router, name: &str, controller: Arc<dyn ResourceController>, singleton: bool, api: bool) -> Self {
        let mut options = ResourceOptions::default();
        if api {
            let only: &[&str] = if singleton { &API_SINGLETON_METHODS } else { &API_RESOURCE_METHODS };
            options.only = Some(only.iter().map(|m| m.to_string()).collect());
        }
        Self {
            router,
            name: name.to_string(),
            controller,
            options,
            group: None,
            singleton,
            registered: false,
        }
    }

    fn register(&mut self) -> Vec<RouteDefinition> {
        if self.registered {
            return Vec::new();
        }
        self.registered = true;

        let mut routes = Vec::new();
        let router = self.router.clone();
        match self.group.clone() {
            Some(group) => router.group(group, || routes = self.register_routes(&self.name)),
            None => routes = self.register_routes(&self.name),
        }
        routes
    }

    fn register_routes(&self, name: &str) -> Vec<RouteDefinition> {
        // "admin/photos" registers the "photos" resource inside an "admin" prefix.
        if let Some((prefix, last)) = name.rsplit_once('/') {
            let mut routes = Vec::new();
            let attributes = GroupAttributes {
                prefix: Some(prefix.to_string()),
                ..GroupAttributes::default()
            };
            self.router.group(attributes, || routes = self.register_routes(last));
            return routes;
        }

        let defaults: Vec<&str> = if self.singleton {
            let mut defaults = SINGLETON_DEFAULTS.to_vec();
            if self.options.creatable {
                defaults.extend(["create", "store", "destroy"]);
            } else if self.options.destroyable {
                defaults.push("destroy");
            }
            defaults
        } else {
            RESOURCE_DEFAULTS.to_vec()
        };

        let methods: Vec<&str> = defaults
            .into_iter()
            .filter(|m| self.options.only.as_ref().is_none_or(|only| only.iter().any(|o| o == m)))
            .filter(|m| !self.options.except.iter().any(|e| e == m))
            .collect();

        let base = self.wildcard(name.rsplit('.').next().unwrap_or(name));
        let (create_verb, edit_verb) = self.router.get_resource_verbs();

        let mut routes = Vec::new();
        for method in methods.iter().copied() {
            let shallow_name = if self.options.shallow && matches!(method, "show" | "edit" | "update" | "destroy") {
                name.rsplit('.').next().unwrap_or(name).to_string()
            } else {
                name.to_string()
            };
            let uri = self.resource_uri(&shallow_name);

            let (verbs, uri): (&[&str], String) = if self.singleton {
                match method {
                    "create" => (&["GET", "HEAD"], format!("{uri}/{create_verb}")),
                    "store" => (&["POST"], uri),
                    "show" => (&["GET", "HEAD"], uri),
                    "edit" => (&["GET", "HEAD"], format!("{uri}/{edit_verb}")),
                    "update" => (&["PUT", "PATCH"], uri),
                    _ => (&["DELETE"], uri),
                }
            } else {
                match method {
                    "index" => (&["GET", "HEAD"], uri),
                    "create" => (&["GET", "HEAD"], format!("{uri}/{create_verb}")),
                    "store" => (&["POST"], uri),
                    "show" => (&["GET", "HEAD"], format!("{uri}/{{{base}}}")),
                    "edit" => (&["GET", "HEAD"], format!("{uri}/{{{base}}}/{edit_verb}")),
                    "update" => (&["PUT", "PATCH"], format!("{uri}/{{{base}}}")),
                    _ => (&["DELETE"], format!("{uri}/{{{base}}}")),
                }
            };

            let route = self.router.add_route(verbs, &uri, self.action(method));
            let route_name = self
                .options
                .names
                .get(method)
                .cloned()
                .unwrap_or_else(|| format!("{shallow_name}.{method}").trim_matches('.').to_string());
            let route = route.name(&route_name);

            let mut middleware = self.options.middleware.clone();
            if let Some(extra) = self.options.middleware_for.get(method) {
                middleware.extend(extra.iter().cloned());
            }
            let mut excluded = self.options.excluded.clone();
            if let Some(extra) = self.options.excluded_for.get(method) {
                excluded.extend(extra.iter().cloned());
            }
            let route = route
                .middleware(unique_middleware(middleware))
                .without_middleware(unique_middleware(excluded));

            let wheres = self.options.wheres.clone();
            let missing = match method {
                "index" | "create" | "store" if !self.singleton => None,
                "create" | "store" | "show" if self.singleton => None,
                _ => self.options.missing.clone(),
            };
            let binding_fields = self.options.binding_fields.clone();
            let parameters = route.parameter_names();
            let with_trashed = self.options.trashed.as_ref().is_some_and(|trashed| {
                if trashed.is_empty() {
                    matches!(method, "show" | "edit" | "update")
                } else {
                    trashed.iter().any(|t| t == method)
                }
            });
            route.update(|state| {
                state.wheres.extend(wheres);
                if missing.is_some() {
                    state.missing = missing;
                }
                if let Some(fields) = binding_fields {
                    state.binding_fields = fields
                        .into_iter()
                        .filter(|(parameter, _)| parameters.contains(parameter))
                        .collect();
                }
                if with_trashed {
                    state.with_trashed = true;
                }
            });

            routes.push(route);
        }
        routes
    }

    /// The URI for a (possibly nested) resource, without its own wildcard.
    fn resource_uri(&self, resource: &str) -> String {
        if !resource.contains('.') {
            return resource.to_string();
        }
        let segments: Vec<&str> = resource.split('.').collect();
        let uri = segments
            .iter()
            .map(|segment| format!("{segment}/{{{}}}", self.wildcard(segment)))
            .collect::<Vec<_>>()
            .join("/");
        let last = segments.last().copied().unwrap_or_default();
        uri.replace(&format!("/{{{}}}", self.wildcard(last)), "")
    }

    /// The parameter name for a resource segment (`photos` → `photo`).
    fn wildcard(&self, value: &str) -> String {
        let value = if let Some(parameter) = self.options.parameters.get(value) {
            parameter.clone()
        } else if let Some(parameter) = self.router.get_resource_parameters().get(value) {
            parameter.clone()
        } else if self.router.uses_singular_resource_parameters() {
            Str::singular(value)
        } else {
            value.to_string()
        };
        value.replace('-', "_")
    }

    fn action(&self, method: &'static str) -> RouteAction {
        let controller = self.controller.clone();
        let name = format!("{}@{method}", self.controller.controller_name());
        let handler: RouteHandler = Arc::new(move |request: Request| {
            let controller = controller.clone();
            Box::pin(async move {
                match method {
                    "index" => controller.index(request).await,
                    "create" => controller.create(request).await,
                    "store" => controller.store(request).await,
                    "show" => controller.show(request).await,
                    "edit" => controller.edit(request).await,
                    "update" => controller.update(request).await,
                    _ => controller.destroy(request).await,
                }
            })
        });
        RouteAction::new(name, handler)
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

macro_rules! pending_methods {
    () => {
        /// Register routes for these actions only.
        pub fn only(mut self, methods: &[&str]) -> Self {
            self.pending.options.only = Some(strings(methods));
            self
        }

        /// Register routes for every action except these.
        pub fn except(mut self, methods: &[&str]) -> Self {
            self.pending.options.except = strings(methods);
            self
        }

        /// Override route names per action (`[("create", "photos.build")]`).
        pub fn names<'a>(mut self, names: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
            for (method, name) in names {
                self.pending.options.names.insert(method.to_string(), name.to_string());
            }
            self
        }

        /// Override the route name of a single action.
        pub fn name(mut self, method: &str, name: &str) -> Self {
            self.pending.options.names.insert(method.to_string(), name.to_string());
            self
        }

        /// Override route parameter names (`[("users", "admin_user")]`).
        pub fn parameters<'a>(mut self, parameters: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
            for (resource, parameter) in parameters {
                self.pending
                    .options
                    .parameters
                    .insert(resource.to_string(), parameter.to_string());
            }
            self
        }

        /// Override a single route parameter name.
        pub fn parameter(mut self, resource: &str, parameter: &str) -> Self {
            self.pending
                .options
                .parameters
                .insert(resource.to_string(), parameter.to_string());
            self
        }

        /// Attach middleware to every route of the resource.
        pub fn middleware(mut self, middleware: impl IntoMiddleware) -> Self {
            self.pending.options.middleware = middleware.into_middleware();
            self
        }

        /// Attach middleware to specific actions.
        pub fn middleware_for(mut self, methods: &[&str], middleware: impl IntoMiddleware) -> Self {
            let middleware = middleware.into_middleware();
            for method in methods {
                self.pending
                    .options
                    .middleware_for
                    .insert(method.to_string(), middleware.clone());
            }
            self
        }

        /// Remove middleware from every route of the resource.
        pub fn without_middleware(mut self, middleware: impl IntoMiddleware) -> Self {
            self.pending.options.excluded.extend(middleware.into_middleware());
            self
        }

        /// Remove middleware from specific actions.
        pub fn without_middleware_for(mut self, methods: &[&str], middleware: impl IntoMiddleware) -> Self {
            let middleware = middleware.into_middleware();
            for method in methods {
                self.pending
                    .options
                    .excluded_for
                    .insert(method.to_string(), middleware.clone());
            }
            self
        }

        /// Constrain a route parameter.
        pub fn where_(mut self, parameter: &str, expression: &str) -> Self {
            self.pending
                .options
                .wheres
                .insert(parameter.to_string(), expression.to_string());
            self
        }

        /// Constrain a route parameter to digits.
        pub fn where_number(self, parameter: &str) -> Self {
            self.where_(parameter, patterns::NUMBER)
        }

        /// Constrain a route parameter to a UUID.
        pub fn where_uuid(self, parameter: &str) -> Self {
            self.where_(parameter, patterns::UUID)
        }

        /// Constrain a route parameter to a ULID.
        pub fn where_ulid(self, parameter: &str) -> Self {
            self.where_(parameter, patterns::ULID)
        }

        /// Handle models that can't be found with the given handler.
        pub fn missing<H: Handler<T>, T: 'static>(mut self, handler: H) -> Self {
            self.pending.options.missing = Some(handler.into_action().handler);
            self
        }

        /// Use custom binding fields for nested resources (`[("comment", "slug")]`).
        pub fn scoped<'a>(mut self, fields: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
            self.pending.options.binding_fields = Some(
                fields
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            );
            self
        }

        /// Allow soft deleted models for the given actions (by default
        /// `show`, `edit` and `update`).
        pub fn with_trashed(mut self, methods: &[&str]) -> Self {
            self.pending.options.trashed = Some(strings(methods));
            self
        }

        /// Register the routes now, returning them.
        pub fn register(mut self) -> Vec<RouteDefinition> {
            self.pending.register()
        }

        pub(crate) fn within(mut self, group: GroupAttributes) -> Self {
            self.pending.group = Some(group);
            self
        }
    };
}

/// A resource registration, completed when dropped (or when
/// [`PendingResourceRegistration::register`] is called).
pub struct PendingResourceRegistration {
    pending: Pending,
}

impl PendingResourceRegistration {
    pub(crate) fn new(router: Router, name: &str, controller: Arc<dyn ResourceController>, api: bool) -> Self {
        Self {
            pending: Pending::new(router, name, controller, false, api),
        }
    }

    /// Use "shallow nesting": member routes (`show`, `edit`, `update`,
    /// `destroy`) drop the parent segments (`/comments/{comment}`).
    pub fn shallow(mut self) -> Self {
        self.pending.options.shallow = true;
        self
    }

    pending_methods!();
}

impl Drop for PendingResourceRegistration {
    fn drop(&mut self) {
        self.pending.register();
    }
}

/// A singleton resource registration, completed when dropped (or when
/// [`PendingSingletonResourceRegistration::register`] is called).
pub struct PendingSingletonResourceRegistration {
    pending: Pending,
}

impl PendingSingletonResourceRegistration {
    pub(crate) fn new(router: Router, name: &str, controller: Arc<dyn ResourceController>, api: bool) -> Self {
        Self {
            pending: Pending::new(router, name, controller, true, api),
        }
    }

    /// Also register `create`, `store` and `destroy` routes.
    pub fn creatable(mut self) -> Self {
        self.pending.options.creatable = true;
        self
    }

    /// Also register the `destroy` route.
    pub fn destroyable(mut self) -> Self {
        self.pending.options.destroyable = true;
        self
    }

    pending_methods!();
}

impl Drop for PendingSingletonResourceRegistration {
    fn drop(&mut self) {
        self.pending.register();
    }
}
