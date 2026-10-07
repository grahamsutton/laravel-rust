//! The HTTP kernel: the front door for every request.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;

use illuminate_http::{
    BoxFuture, Destination, Middleware as HttpMiddleware, Request, Response, build_pipeline,
    render_exception, with_request,
};
use illuminate_routing::{RouteMiddleware, Router};
use illuminate_support::{Error, Result};

use crate::application::Application;
use crate::configuration::Middleware;
use crate::http::middleware::{
    ConvertEmptyStringsToNull, HandleCors, PreventRequestsDuringMaintenance, ServePublicFiles,
    SetCacheHeaders, TrimStrings, TrustProxies,
};

/// A panic inside a request handler, converted into an error.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct HandlerPanicked {
    pub message: String,
}

/// The HTTP kernel.
pub struct HttpKernel {
    app: Arc<Application>,
    router: Router,
    global: Vec<Arc<dyn HttpMiddleware>>,
    pipeline: Destination,
}

impl HttpKernel {
    /// Build the kernel for the given (bootstrapped) application, syncing
    /// the configured middleware to the router.
    pub fn new(app: Arc<Application>) -> Result<Self> {
        let router = app.make::<Router>().as_ref().clone();
        let config = app
            .try_make::<Middleware>()
            .map(|m| m.as_ref().clone())
            .unwrap_or_default();

        sync_middleware_to_router(&router, &config);

        let mut global_names: Vec<RouteMiddleware> = Vec::new();
        global_names.extend(config.global_prepend.iter().cloned());
        if !config.global_remove.iter().any(|r| r == "*") {
            global_names.extend(default_global_middleware(&app, &config));
        }
        global_names.extend(config.global.iter().cloned());
        global_names.extend(config.global_append.iter().cloned());
        global_names.retain(|m| !config.global_remove.iter().any(|removed| m.name() == removed));

        let global = global_names
            .iter()
            .map(|entry| router.resolve_middleware_instance(entry))
            .collect::<Result<Vec<_>>>()?;

        let pipeline = build_pipeline(global.clone(), router.as_destination());

        Ok(Self {
            app,
            router,
            global,
            pipeline,
        })
    }

    /// The application.
    pub fn application(&self) -> &Arc<Application> {
        &self.app
    }

    /// The router.
    pub fn router(&self) -> &Router {
        &self.router
    }

    /// Handle an incoming request.
    pub async fn handle(&self, request: Request) -> Response {
        // Like PHP's fresh process per request, locale changes made while
        // handling a request don't leak into the next one.
        let response = illuminate_translation::locale_scope(self.send_through_pipeline(request)).await;
        self.app.container().forget_scoped_instances();
        response
    }

    async fn send_through_pipeline(&self, request: Request) -> Response {
        let pipeline = self.pipeline.clone();
        let scoped = request.clone();
        let result = with_request(scoped.clone(), async move {
            AssertUnwindSafe(pipeline(scoped)).catch_unwind().await
        })
        .await;

        match result {
            Ok(response) => response,
            Err(panic) => {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "The request handler panicked.".to_string());
                let error: Error = HandlerPanicked { message }.into();
                with_request(request.clone(), async move { render_exception(error) }).await
            }
        }
    }

    /// Run any "terminable" middleware after the response was sent.
    pub async fn terminate(&self, request: &Request, response: &Response) {
        for middleware in &self.global {
            middleware.terminate(request, response).await;
        }
        self.router.terminate(request, response).await;
    }

    /// A handler suitable for `illuminate_http::server`.
    pub fn into_handler(self: Arc<Self>) -> illuminate_http::server::Handler {
        Arc::new(move |request: Request| {
            let kernel = self.clone();
            Box::pin(async move {
                let deferred = Arc::new(illuminate_concurrency::DeferredCallbacks::new());
                let response = deferred.clone().scope(kernel.handle(request.clone())).await;
                kernel.terminate(&request, &response).await;
                // `dispatch_after_response` jobs and deferred callbacks run once
                // the response is on its way.
                let jobs = request.extension::<illuminate_queue::DeferredCallbacks>();
                if jobs.is_some() || !deferred.is_empty() {
                    let container = kernel.app.container().clone();
                    tokio::spawn(illuminate_container::Container::scope(
                        container,
                        crate::helpers::run_deferred(jobs, deferred),
                    ));
                }
                response
            }) as BoxFuture<'static, Response>
        })
    }
}

/// The framework's default global middleware.
fn default_global_middleware(app: &Application, config: &Middleware) -> Vec<RouteMiddleware> {
    let mut stack = vec![RouteMiddleware::of(ServePublicFiles::new(app.public_path("")))];
    if let Some((hosts, subdomains)) = &config.trusted_hosts {
        let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
        stack.push(RouteMiddleware::of(crate::http::middleware::TrustHosts::at(&hosts, *subdomains)));
    }
    if let Some(proxies) = &config.trusted_proxies {
        let proxies: Vec<&str> = proxies.iter().map(String::as_str).collect();
        stack.push(RouteMiddleware::of(TrustProxies::at(&proxies)));
    }
    stack.push(RouteMiddleware::of(HandleCors::default()));
    let maintenance_except: Vec<&str> = config.maintenance_except.iter().map(String::as_str).collect();
    stack.push(RouteMiddleware::of(
        PreventRequestsDuringMaintenance::default().except(&maintenance_except),
    ));
    let trim_except: Vec<&str> = config.trim_strings_except.iter().map(String::as_str).collect();
    stack.push(RouteMiddleware::of(TrimStrings::default().except(&trim_except)));
    let empty_except: Vec<&str> = config.empty_strings_except.iter().map(String::as_str).collect();
    stack.push(RouteMiddleware::of(ConvertEmptyStringsToNull::default().except(&empty_except)));
    stack
}

/// Register the framework's middleware groups and aliases, then the
/// application's own, on the router.
pub fn sync_middleware_to_router(router: &Router, config: &Middleware) {
    // Aliases.
    router.alias_middleware("cache.headers", |params| {
        Arc::new(SetCacheHeaders::from_parameters(params)) as Arc<dyn HttpMiddleware>
    });
    router.alias_middleware("throttle", illuminate_cache::throttle_middleware);
    for (name, factory) in crate::providers::framework_middleware_aliases(config) {
        router.alias_middleware_factory(&name, factory);
    }
    for (name, factory) in &config.aliases {
        router.alias_middleware_factory(name, factory.clone());
    }

    // Groups.
    let mut web: Vec<RouteMiddleware> = Vec::new();
    let encrypt_except = config.encrypt_cookies_except.clone();
    web.push(RouteMiddleware::of(illuminate_cookie::EncryptCookies::new().except(encrypt_except)));
    web.push(RouteMiddleware::of(illuminate_cookie::AddQueuedCookiesToResponse::new()));
    web.push(RouteMiddleware::of(illuminate_session::StartSession::new()));
    let unit_tests = Application::try_current().is_some_and(|app| app.running_unit_tests());
    web.push(RouteMiddleware::of(
        illuminate_session::ValidateCsrfToken::new()
            .except(config.csrf_except.clone())
            .enabled(!config.disabled_csrf && !unit_tests),
    ));

    let mut api: Vec<RouteMiddleware> = Vec::new();
    if let Some(limiter) = &config.throttle_api {
        api.push(RouteMiddleware::named(format!("throttle:{limiter}")));
    }

    let mut groups = indexmap::IndexMap::new();
    groups.insert("web".to_string(), web);
    groups.insert("api".to_string(), api);
    for (name, middleware) in &config.groups {
        groups.insert(name.clone(), middleware.clone());
    }

    for (name, mut middleware) in groups {
        if let Some(prepend) = config.group_prepend.get(&name) {
            let mut combined = prepend.clone();
            combined.extend(middleware);
            middleware = combined;
        }
        if let Some(append) = config.group_append.get(&name) {
            middleware.extend(append.iter().cloned());
        }
        if let Some(removals) = config.group_remove.get(&name) {
            middleware.retain(|m| !removals.iter().any(|r| m.name() == r || m.base_name() == r));
        }
        router.middleware_group(&name, middleware);
    }
    for (name, middleware) in config.group_append.iter().chain(config.group_prepend.iter()) {
        if !router.has_middleware_group(name) {
            router.middleware_group(name, middleware.clone());
        }
    }

    if let Some(priority) = &config.priority {
        router.set_middleware_priority(priority.clone());
    }
}

impl Application {
    /// The application's HTTP kernel (bootstrapping the application first).
    pub fn http_kernel(self: &Arc<Self>) -> Result<Arc<HttpKernel>> {
        if let Ok(kernel) = self.try_make::<HttpKernel>() {
            return Ok(kernel);
        }
        self.bootstrap();
        let kernel = Arc::new(HttpKernel::new(self.clone())?);
        self.instance_arc::<HttpKernel>(kernel.clone());
        Ok(kernel)
    }

    /// Handle a request through the HTTP kernel.
    pub async fn handle_request(self: &Arc<Self>, request: Request) -> Response {
        match self.http_kernel() {
            Ok(kernel) => {
                let deferred = Arc::new(illuminate_concurrency::DeferredCallbacks::new());
                let response = deferred.clone().scope(kernel.handle(request.clone())).await;
                kernel.terminate(&request, &response).await;
                // Run `dispatch_after_response` jobs and deferred callbacks
                // before handing the response back (the test client relies on it).
                let jobs = request.extension::<illuminate_queue::DeferredCallbacks>();
                crate::helpers::run_deferred(jobs, deferred).await;
                response
            }
            Err(error) => with_request(request, async move { render_exception(error) }).await,
        }
    }

    /// Serve the application over HTTP on the given address until Ctrl+C.
    pub async fn serve(self: &Arc<Self>, addr: std::net::SocketAddr) -> std::io::Result<()> {
        let kernel = self.http_kernel().map_err(std::io::Error::other)?;
        illuminate_http::server::serve(addr, kernel.into_handler()).await
    }
}
