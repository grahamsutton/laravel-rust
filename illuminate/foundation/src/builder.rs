//! `Application::configure(...)` — the fluent builder behind `bootstrap/app.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use illuminate_container::ServiceProvider;
use illuminate_http::{ExceptionHandler, Response};
use illuminate_routing::Router;

use crate::application::Application;
use crate::bootstrap::ConfigFile;
use crate::configuration::{Exceptions, Middleware, Routing};
use crate::exceptions::Handler;
use crate::providers;

/// A callback run once the application is created.
type CreatedCallback = Box<dyn FnOnce(&Arc<Application>) + Send>;

/// Configure and create a Laravel application.
///
/// ```ignore
/// Application::configure(base_path!())
///     .with_routing(|routing| {
///         routing.web(routes::web).commands(routes::console).health("/up");
///     })
///     .with_middleware(|middleware| {
///         //
///     })
///     .with_exceptions(|exceptions| {
///         //
///     })
///     .create()
/// ```
pub struct ApplicationBuilder {
    app: Arc<Application>,
    routing: Routing,
    middleware: Middleware,
    exceptions: Exceptions,
    providers: Vec<Box<dyn ServiceProvider>>,
    dont_discover: Vec<String>,
    config_files: Vec<ConfigFile>,
    callbacks: Vec<CreatedCallback>,
}

impl Application {
    /// Begin configuring a new application rooted at `base_path`.
    pub fn configure(base_path: impl Into<PathBuf>) -> ApplicationBuilder {
        ApplicationBuilder::new(Application::new(base_path))
    }

    /// Begin configuring an application that isn't the global instance (for tests).
    pub fn configure_detached(base_path: impl Into<PathBuf>) -> ApplicationBuilder {
        ApplicationBuilder::new(Application::new_detached(base_path))
    }
}

impl ApplicationBuilder {
    /// Wrap an existing application.
    pub fn new(app: Arc<Application>) -> Self {
        Self {
            app,
            routing: Routing::default(),
            middleware: Middleware::new(),
            exceptions: Exceptions::new(),
            providers: Vec::new(),
            dont_discover: Vec::new(),
            config_files: Vec::new(),
            callbacks: Vec::new(),
        }
    }

    /// The application being configured.
    pub fn application(&self) -> &Arc<Application> {
        &self.app
    }

    /// Register the application's configuration files.
    pub fn with_config(mut self, files: impl IntoIterator<Item = ConfigFile>) -> Self {
        self.config_files.extend(files);
        self
    }

    /// Register the routing files and the health check route.
    pub fn with_routing(mut self, callback: impl FnOnce(&mut Routing)) -> Self {
        callback(&mut self.routing);
        self
    }

    /// Configure the application's middleware.
    pub fn with_middleware(mut self, callback: impl FnOnce(&mut Middleware)) -> Self {
        callback(&mut self.middleware);
        self
    }

    /// Configure how exceptions are reported and rendered.
    pub fn with_exceptions(mut self, callback: impl FnOnce(&mut Exceptions)) -> Self {
        callback(&mut self.exceptions);
        self
    }

    /// Register additional service providers.
    pub fn with_providers(mut self, providers: Vec<Box<dyn ServiceProvider>>) -> Self {
        self.providers.extend(providers);
        self
    }

    /// Register a single service provider.
    pub fn with_provider(mut self, provider: impl ServiceProvider) -> Self {
        self.providers.push(Box::new(provider));
        self
    }

    /// Register additional Artisan commands.
    pub fn with_commands(self, commands: Vec<Box<dyn illuminate_console::Command>>) -> Self {
        let commands: Vec<Arc<dyn illuminate_console::Command>> =
            commands.into_iter().map(Arc::from).collect();
        self.tap(move |app| {
            app.singleton_if::<crate::console::ConsoleConfiguration>(|_| Arc::new(Default::default()));
            app.make::<crate::console::ConsoleConfiguration>().add_commands(commands);
        })
    }

    /// Define the application's command schedule.
    pub fn with_schedule(
        self,
        callback: impl Fn(&illuminate_console::scheduling::Schedule) + Send + Sync + 'static,
    ) -> Self {
        let callback = Arc::new(callback);
        self.tap(move |app| {
            app.singleton_if::<crate::console::ConsoleConfiguration>(|_| Arc::new(Default::default()));
            app.make::<crate::console::ConsoleConfiguration>().add_schedule(callback);
        })
    }

    /// Register the application's migrations (`database/migrations`).
    ///
    /// ```ignore
    /// .with_migrations(database::migrations::all())
    /// ```
    pub fn with_migrations(
        self,
        migrations: impl IntoIterator<Item = (String, Box<dyn illuminate_database::Migration>)>,
    ) -> Self {
        let migrations: Vec<_> = migrations.into_iter().collect();
        self.tap(move |app| {
            app.singleton_if::<illuminate_database::MigrationRegistry>(|_| Arc::new(Default::default()));
            app.make::<illuminate_database::MigrationRegistry>().extend(migrations);
        })
    }

    /// Register the application's seeders (`database/seeders`).
    ///
    /// ```ignore
    /// .with_seeders(database::seeders::register)
    /// ```
    pub fn with_seeders(self, register: impl FnOnce(&illuminate_database::SeederRegistry) + Send + 'static) -> Self {
        self.tap(move |app| {
            app.singleton_if::<illuminate_database::SeederRegistry>(|_| Arc::new(Default::default()));
            register(&app.make::<illuminate_database::SeederRegistry>());
        })
    }

    /// Register the application's Blade class components (`app/view/components`).
    ///
    /// ```ignore
    /// .with_components(components::register)
    /// ```
    pub fn with_components(self, register: impl FnOnce(&illuminate_view::BladeCompiler) + Send + 'static) -> Self {
        self.tap(move |app| {
            app.booted(move |_| register(illuminate_view::Factory::resolve().blade()));
        })
    }

    /// Register the application's policies (`app/policies`).
    ///
    /// ```ignore
    /// .with_policies(policies::register)
    /// ```
    pub fn with_policies(self, register: impl FnOnce() + Send + 'static) -> Self {
        self.tap(move |app| app.booted(move |_| register()))
    }

    /// Register the broadcasting routes and the application's channels
    /// (`routes/channels.rs`).
    ///
    /// ```ignore
    /// .with_broadcasting(routes::channels)
    /// ```
    pub fn with_broadcasting(mut self, channels: impl Fn() + Send + Sync + 'static) -> Self {
        self.routing.channels(channels);
        self
    }

    /// Don't register the service providers of the given packages
    /// automatically (`"*"` for every package) — Laravel's `dont-discover`.
    pub fn dont_discover(mut self, packages: &[&str]) -> Self {
        self.dont_discover.extend(packages.iter().map(|package| package.to_string()));
        self
    }

    /// Run a callback against the application once it is created (used by
    /// framework extensions such as console commands and migrations).
    pub fn tap(mut self, callback: impl FnOnce(&Arc<Application>) + Send + 'static) -> Self {
        self.callbacks.push(Box::new(callback));
        self
    }

    /// Create the application.
    pub fn create(self) -> Arc<Application> {
        let app = self.app;

        app.add_config_files(self.config_files);
        app.instance(self.middleware);
        app.instance(self.routing.clone());

        // The exception handler.
        let handler = Arc::new(Handler::new(
            self.exceptions,
            || Application::try_current().is_some_and(|app| app.has_debug_mode_enabled()),
            || {
                Application::try_current()
                    .map(|app| app.environment())
                    .unwrap_or_else(|| "production".into())
            },
        ));
        app.instance_arc::<Handler>(handler.clone());
        app.instance_arc::<dyn ExceptionHandler>(handler);

        // Framework providers first, then discovered packages, then the
        // application's own.
        for provider in providers::default_providers() {
            providers::add_pending(&app, provider);
        }
        for package in illuminate_container::discovered_providers() {
            if !self.dont_discover.iter().any(|name| name == package.package || name == "*") {
                providers::add_pending(&app, (package.provider)());
            }
        }
        for provider in self.providers {
            providers::add_pending(&app, provider);
        }

        // Load the routes once everything has booted.
        let routing = self.routing;
        app.booted(move |app| load_routes(app, &routing));

        for callback in self.callbacks {
            callback(&app);
        }

        app
    }
}

/// Register the application's routes.
fn load_routes(app: &Application, routing: &Routing) {
    let Ok(router) = app.try_make::<Router>() else {
        return;
    };

    if let Some(health) = &routing.health {
        router.get(health, || async { Response::new(HEALTH_PAGE) });
    }

    for routes in &routing.web {
        let routes = routes.clone();
        router.middleware("web").group(move || routes());
    }

    let prefix = routing.get_api_prefix();
    for routes in &routing.api {
        let routes = routes.clone();
        router.prefix(&prefix).middleware("api").group(move || routes());
    }

    // Broadcast channels: the `/broadcasting/auth` routes, then the
    // channels' authorization callbacks.
    if !routing.channels.is_empty() {
        illuminate_broadcasting::Broadcast::routes(None);
        for channels in &routing.channels {
            channels();
        }
    }

    for routes in &routing.then {
        routes();
    }
}

const HEALTH_PAGE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Application up</title>
    <style>
        body { margin: 0; font-family: ui-sans-serif, system-ui, sans-serif; display: flex; min-height: 100vh; align-items: center; justify-content: center; background: #f9fafb; color: #111827; }
        .card { text-align: center; }
        .dot { display: inline-block; width: .75rem; height: .75rem; border-radius: 9999px; background: #22c55e; margin-right: .5rem; }
        @media (prefers-color-scheme: dark) { body { background: #030712; color: #f3f4f6; } }
    </style>
</head>
<body>
    <div class="card"><span class="dot"></span>Application up</div>
</body>
</html>
"#;
