//! The `Concurrency` facade.

use std::sync::Arc;

use illuminate_container::{Container, try_app};
use illuminate_process::IntoTimeout;
use illuminate_support::Result;

use crate::driver::Driver;
use crate::manager::ConcurrencyManager;
use crate::tasks::Tasks;

/// The `Concurrency` facade: run independent, slow tasks at the same time.
///
/// Tasks are futures that return `illuminate_support::Result`. Hand `run`
/// a tuple of them and you get a tuple of results back:
///
/// ```
/// use illuminate_concurrency::Concurrency;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let (user_count, order_count) = Concurrency::run((
///     async { Ok(2) },
///     async { Ok(4) },
/// ))
/// .await?;
///
/// assert_eq!((user_count, order_count), (2, 4));
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
///
/// If any task fails, `run` fails with the first error (in task order) once
/// every task has finished.
pub struct Concurrency;

impl Concurrency {
    /// Get the concurrency manager from the container, registering one if
    /// the application hasn't yet.
    pub fn manager() -> Arc<ConcurrencyManager> {
        if let Some(manager) = try_app::<ConcurrencyManager>() {
            return manager;
        }
        let container = Container::get_instance();
        container.singleton_if::<ConcurrencyManager>(|_| Arc::new(ConcurrencyManager::new()));
        container.make::<ConcurrencyManager>()
    }

    /// Get a driver by name (`None` for the default driver).
    ///
    /// ```
    /// use illuminate_concurrency::Concurrency;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let results = Concurrency::driver("sync")?
    ///     .run(vec![async { Ok(1 + 1) }])
    ///     .await?;
    ///
    /// assert_eq!(results, [2]);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn driver<'a>(name: impl Into<Option<&'a str>>) -> Result<Arc<dyn Driver>> {
        Self::manager().driver(name)
    }

    /// Run the given tasks concurrently and return their results, in the
    /// same shape: a tuple, array, `Vec`, or keyed `IndexMap`.
    ///
    /// ```
    /// use illuminate_concurrency::{Concurrency, IndexMap, task};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let results = Concurrency::run(IndexMap::from([
    ///     ("users", task(async { Ok(10) })),
    ///     ("orders", task(async { Ok(25) })),
    /// ]))
    /// .await?;
    ///
    /// assert_eq!(results["users"], 10);
    /// assert_eq!(results["orders"], 25);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn run<T: Tasks>(tasks: T) -> Result<T::Output> {
        Self::driver(None)?.run(tasks).await
    }

    /// Run the given tasks concurrently, failing with a
    /// [`TaskTimedOutException`](crate::TaskTimedOutException) if they don't
    /// all finish within the timeout (seconds, a `Duration`, or a
    /// `CarbonInterval`).
    ///
    /// ```
    /// use std::time::Duration;
    /// use illuminate_concurrency::{Concurrency, TaskTimedOutException};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let error = Concurrency::run_with_timeout(
    ///     (async {
    ///         tokio::time::sleep(Duration::from_secs(5)).await;
    ///         Ok(())
    ///     },),
    ///     Duration::from_millis(10),
    /// )
    /// .await
    /// .unwrap_err();
    ///
    /// assert!(error.is::<TaskTimedOutException>());
    /// # });
    /// ```
    pub async fn run_with_timeout<T: Tasks>(
        tasks: T,
        timeout: impl IntoTimeout,
    ) -> Result<T::Output> {
        Self::driver(None)?.run_with_timeout(tasks, timeout).await
    }

    /// Run the given tasks concurrently once the response has been sent,
    /// discarding their results.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_concurrency::{Concurrency, DeferredCallbacks};
    /// use illuminate_container::Container;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// # let _guard = Container::set_local_instance(Arc::new(Container::new()));
    /// Concurrency::defer((
    ///     async { /* Metrics::report("users") */ Ok(()) },
    ///     async { /* Metrics::report("orders") */ Ok(()) },
    /// ))?;
    ///
    /// // Later, once the response is on its way...
    /// let errors = DeferredCallbacks::current().invoke().await;
    /// assert!(errors.is_empty());
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn defer<T: Tasks>(tasks: T) -> Result<()> {
        Self::driver(None)?.defer(tasks);
        Ok(())
    }

    /// Register a custom driver.
    pub fn extend<F>(name: impl Into<String>, creator: F)
    where
        F: Fn() -> Arc<dyn Driver> + Send + Sync + 'static,
    {
        Self::manager().extend(name, creator);
    }
}
