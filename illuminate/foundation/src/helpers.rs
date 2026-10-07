//! Application helpers: paths, environment, and friends.

use std::sync::Arc;

use crate::application::Application;

/// Get the current application.
pub fn application() -> Arc<Application> {
    Application::current()
}

fn with_app(callback: impl FnOnce(&Application) -> String, fallback: &str) -> String {
    match Application::try_current() {
        Some(app) => callback(&app),
        None => fallback.to_string(),
    }
}

/// Get the path to the base of the install.
pub fn base_path(path: &str) -> String {
    with_app(|app| app.base_path(path), path)
}

/// Get the path to the application folder.
pub fn app_path(path: &str) -> String {
    with_app(|app| app.app_path(path), path)
}

/// Get the path to the bootstrap folder.
pub fn bootstrap_path(path: &str) -> String {
    with_app(|app| app.bootstrap_path(path), path)
}

/// Get the configuration path.
pub fn config_path(path: &str) -> String {
    with_app(|app| app.config_path(path), path)
}

/// Get the database path.
pub fn database_path(path: &str) -> String {
    with_app(|app| app.database_path(path), path)
}

/// Get the path to the language folder.
pub fn lang_path(path: &str) -> String {
    with_app(|app| app.lang_path(path), path)
}

/// Get the path to the public folder.
pub fn public_path(path: &str) -> String {
    with_app(|app| app.public_path(path), path)
}

/// Get the path to the resources folder.
pub fn resource_path(path: &str) -> String {
    with_app(|app| app.resource_path(path), path)
}

/// Get the path to the storage folder.
pub fn storage_path(path: &str) -> String {
    with_app(|app| app.storage_path(path), path)
}

/// Get the current application environment.
pub fn app_environment() -> String {
    with_app(|app| app.environment(), "production")
}

/// Report an error to the exception handler without rendering it — the
/// error is logged (or sent wherever your `report` callbacks send it), and
/// your code carries on.
///
/// ```ignore
/// if let Err(error) = sync_podcasts().await {
///     report(&error);
/// }
/// ```
pub fn report(error: &illuminate_support::Error) {
    match illuminate_container::try_app::<dyn illuminate_http::ExceptionHandler>() {
        Some(handler) => {
            if handler.should_report(error) {
                handler.report(error);
            }
        }
        None => eprintln!("{error:?}"),
    }
}

/// Run the future, reporting its error and returning the fallback instead
/// when it fails.
///
/// ```ignore
/// let total = rescue(|| calculate_total(), 0).await;
/// ```
pub async fn rescue<F, Fut, T>(callback: F, fallback: T) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = illuminate_support::Result<T>>,
{
    match callback().await {
        Ok(value) => value,
        Err(error) => {
            report(&error);
            fallback
        }
    }
}

/// Defer work until after the response has been sent to the user (or the
/// command has finished).
///
/// ```ignore
/// Route::post("/orders", |request: Request| async move {
///     let order = Order::create(request.all()).await?;
///
///     defer(async move { Metrics::report_order(&order).await });
///
///     Ok(Json(order))
/// });
/// ```
pub fn defer<F>(future: F)
where
    F: std::future::Future<Output = illuminate_support::Result<()>> + Send + 'static,
{
    illuminate_concurrency::DeferredCallbacks::current().defer(future);
}

/// Run the work deferred while handling a request or command: jobs
/// dispatched after the response, and `defer`red callbacks.
pub(crate) async fn run_deferred(
    jobs: Option<Arc<illuminate_queue::DeferredCallbacks>>,
    callbacks: Arc<illuminate_concurrency::DeferredCallbacks>,
) {
    if let Some(jobs) = jobs {
        jobs.invoke().await;
    }
    for error in callbacks.invoke().await {
        report(&error);
    }
}
