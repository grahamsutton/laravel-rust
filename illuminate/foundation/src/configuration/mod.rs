//! The configuration objects handed to `bootstrap/app.rs`.

mod middleware;

use std::sync::Arc;

pub use crate::exceptions::Exceptions;
pub use middleware::Middleware;

type RouteLoader = Arc<dyn Fn() + Send + Sync>;

/// Configure how the application's routes are loaded.
///
/// ```ignore
/// .with_routing(|routing| {
///     routing
///         .web(routes::web::routes)
///         .commands(routes::console::routes)
///         .health("/up");
/// })
/// ```
#[derive(Clone, Default)]
pub struct Routing {
    pub(crate) web: Vec<RouteLoader>,
    pub(crate) api: Vec<RouteLoader>,
    pub(crate) commands: Vec<RouteLoader>,
    pub(crate) channels: Vec<RouteLoader>,
    pub(crate) then: Vec<RouteLoader>,
    pub(crate) health: Option<String>,
    pub(crate) api_prefix: Option<String>,
}

impl Routing {
    /// Load web routes: they get the "web" middleware group (sessions,
    /// cookies, CSRF protection).
    pub fn web(&mut self, routes: impl Fn() + Send + Sync + 'static) -> &mut Self {
        self.web.push(Arc::new(routes));
        self
    }

    /// Load API routes: they are prefixed with `/api` and get the stateless
    /// "api" middleware group.
    pub fn api(&mut self, routes: impl Fn() + Send + Sync + 'static) -> &mut Self {
        self.api.push(Arc::new(routes));
        self
    }

    /// Use a different prefix for API routes (default `api`).
    pub fn api_prefix(&mut self, prefix: &str) -> &mut Self {
        self.api_prefix = Some(prefix.trim_matches('/').to_string());
        self
    }

    /// Register closure-based Artisan commands and schedules.
    pub fn commands(&mut self, commands: impl Fn() + Send + Sync + 'static) -> &mut Self {
        self.commands.push(Arc::new(commands));
        self
    }

    /// Register broadcast channels.
    pub fn channels(&mut self, channels: impl Fn() + Send + Sync + 'static) -> &mut Self {
        self.channels.push(Arc::new(channels));
        self
    }

    /// Register the health check route (`/up`).
    pub fn health(&mut self, uri: &str) -> &mut Self {
        self.health = Some(uri.to_string());
        self
    }

    /// Register additional routes after the standard ones.
    pub fn then(&mut self, routes: impl Fn() + Send + Sync + 'static) -> &mut Self {
        self.then.push(Arc::new(routes));
        self
    }

    /// The configured API prefix.
    pub fn get_api_prefix(&self) -> String {
        self.api_prefix.clone().unwrap_or_else(|| "api".to_string())
    }
}
