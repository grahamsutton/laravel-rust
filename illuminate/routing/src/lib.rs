//! # Illuminate Routing
//!
//! Laravel's router, for Rust. Define routes with the `Route` facade,
//! handle them with plain async functions whose arguments are extracted
//! from the request, protect them with middleware, and generate URLs to
//! them by name.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_container::Container;
//! use illuminate_http::Request;
//! use illuminate_routing::{route, Path, Route};
//!
//! struct UserController;
//!
//! impl UserController {
//!     async fn show(Path(id): Path<u64>) -> String {
//!         format!("User {id}")
//!     }
//! }
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! # runtime.block_on(async {
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container);
//!
//! Route::get("/", || async { "Welcome!" });
//! Route::get("/users/{id}", UserController::show).where_number("id").name("users.show");
//!
//! let response = Route::router().dispatch(Request::create("/users/1", "GET")).await;
//! assert_eq!(response.content_string(), "User 1");
//!
//! assert_eq!(route("users.show", 1).unwrap(), "http://localhost/users/1");
//! # });
//! ```
//!
//! ## Integration
//!
//! - The HTTP kernel sends requests through its global middleware and then
//!   to [`Router::dispatch`] (or the [`Router::as_destination`] closure),
//!   and calls [`Router::terminate`] once the response has been sent.
//! - The view component installs a renderer with
//!   [`Router::set_view_renderer`] so `Route::view` works.
//! - Models implement [`FromRequest`] using [`route_parameter_for`] for
//!   route model binding, and [`UrlRoutable`] for URL generation.

pub mod binding;
pub mod compiled;
pub mod de;
pub mod exceptions;
pub mod extract;
pub mod facade;
pub mod handler;
pub mod helpers;
pub mod middleware;
pub mod params;
pub mod provider;
pub mod redirect;
pub mod registrar;
pub mod resource;
pub mod route;
pub mod router;
pub mod url;

pub use binding::{route_allows_trashed_bindings, route_parameter_for};
pub use exceptions::{
    InvalidRouteException, InvalidSignatureException, MiddlewareNotFoundException,
    RecursiveMiddlewareGroupException, RouteNotFoundException, UrlGenerationException,
};
pub use extract::{FromRequest, Inject, Input, Path, Query};
pub use facade::Route;
pub use handler::Handler;
pub use helpers::{asset, route, secure_asset, secure_url, url};
pub use middleware::{IntoMiddleware, MiddlewareFactory, RouteMiddleware, RouteMiddlewareStack, ValidateSignature};
pub use params::{IntoRouteParameter, IntoRouteParameters, RouteParameter, RouteParameters, UrlRoutable};
pub use provider::RoutingServiceProvider;
pub use redirect::{Redirect, back, redirect, redirect_to_route, to_route};
pub use registrar::RouteRegistrar;
pub use resource::{PendingResourceRegistration, PendingSingletonResourceRegistration, ResourceController};
pub use route::{CurrentRoute, RouteAction, RouteDefinition, RouteHandler, RouteListing};
pub use router::{GroupAttributes, MatchedCallback, MissingModelDetector, Router, VERBS, ViewRenderer, router};
pub use url::{Expiration, KeyResolver, URL, UrlDefaults, UrlGenerator, url_generator};

/// `Json<T>` is both a response and an extractor.
pub use illuminate_http::Json;

/// Re-exported so implementors of [`FromRequest`] and [`ResourceController`]
/// don't need their own dependency.
pub use async_trait::async_trait;

#[cfg(test)]
mod tests;
