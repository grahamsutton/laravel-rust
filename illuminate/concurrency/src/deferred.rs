//! Work deferred until after the response has been sent.

use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use futures::future::BoxFuture;
use illuminate_container::{Container, try_app};
use illuminate_support::{Error, Result};

tokio::task_local! {
    static CURRENT: Arc<DeferredCallbacks>;
}

/// Callbacks to run once the current request (or command) is finished:
/// Laravel's `DeferredCallbackCollection`, which `Concurrency::defer` adds
/// to.
///
/// Inside a [`scope`](Self::scope) the scoped collection is current, which
/// keeps the work deferred by concurrent requests apart; otherwise it is
/// the collection bound in the container. The framework runs them with
/// [`invoke`](Self::invoke):
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use illuminate_concurrency::DeferredCallbacks;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let deferred = Arc::new(DeferredCallbacks::new());
/// let reported = Arc::new(AtomicBool::new(false));
///
/// let flag = reported.clone();
/// let response = deferred.clone().scope(async move {
///     DeferredCallbacks::current().defer(async move {
///         flag.store(true, Ordering::SeqCst);
///         Ok(())
///     });
///     "Hello World"
/// })
/// .await;
///
/// assert_eq!(response, "Hello World");
/// assert!(!reported.load(Ordering::SeqCst));
///
/// // ...the response is sent...
/// let errors = deferred.invoke().await;
/// assert!(errors.is_empty());
/// assert!(reported.load(Ordering::SeqCst));
/// # });
/// ```
#[derive(Default)]
pub struct DeferredCallbacks {
    callbacks: Mutex<Vec<BoxFuture<'static, Result<()>>>>,
}

impl DeferredCallbacks {
    /// Create an empty collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// The collection for the current context: the scoped collection, or
    /// the one bound in the container.
    pub fn current() -> Arc<DeferredCallbacks> {
        if let Ok(callbacks) = CURRENT.try_with(Arc::clone) {
            return callbacks;
        }
        if let Some(callbacks) = try_app::<DeferredCallbacks>() {
            return callbacks;
        }
        let container = Container::get_instance();
        container.singleton_if::<DeferredCallbacks>(|_| Arc::new(DeferredCallbacks::new()));
        container.make::<DeferredCallbacks>()
    }

    /// Run the future with this collection as the current one.
    pub fn scope<F: Future>(self: Arc<Self>, future: F) -> impl Future<Output = F::Output> {
        CURRENT.scope(self, future)
    }

    /// Defer the given future until the collection is invoked.
    pub fn defer<F>(&self, callback: F)
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        self.callbacks.lock().unwrap().push(callback.boxed());
    }

    /// The number of deferred callbacks.
    pub fn len(&self) -> usize {
        self.callbacks.lock().unwrap().len()
    }

    /// Determine if there are no deferred callbacks.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forget every deferred callback without running it.
    pub fn flush(&self) {
        self.callbacks.lock().unwrap().clear();
    }

    /// Run every deferred callback (including any deferred while they run),
    /// in the order they were deferred.
    ///
    /// A failing callback doesn't stop the others: its error is collected
    /// and returned so the caller can report it (Laravel `rescue`s them).
    pub async fn invoke(&self) -> Vec<Error> {
        let mut errors = Vec::new();
        loop {
            let callbacks = std::mem::take(&mut *self.callbacks.lock().unwrap());
            if callbacks.is_empty() {
                return errors;
            }
            for callback in callbacks {
                if let Err(error) = callback.await {
                    errors.push(error);
                }
            }
        }
    }
}

impl fmt::Debug for DeferredCallbacks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeferredCallbacks")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::error::RuntimeException;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn callbacks_run_in_order_and_collect_errors() {
        let deferred = Arc::new(DeferredCallbacks::new());
        let order = Arc::new(Mutex::new(Vec::new()));

        for index in 0..3 {
            let order = order.clone();
            deferred.defer(async move {
                order.lock().unwrap().push(index);
                if index == 1 {
                    return Err(RuntimeException::new("Metrics are down.").into());
                }
                Ok(())
            });
        }

        assert_eq!(deferred.len(), 3);
        let errors = deferred.invoke().await;
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2]);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].to_string(), "Metrics are down.");
        assert!(deferred.is_empty());
    }

    #[tokio::test]
    async fn callbacks_deferred_while_invoking_also_run() {
        let deferred = Arc::new(DeferredCallbacks::new());
        let count = Arc::new(AtomicUsize::new(0));

        let inner = deferred.clone();
        let counter = count.clone();
        deferred.defer(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let counter = counter.clone();
            inner.defer(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            Ok(())
        });

        deferred.invoke().await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn the_container_collection_is_current_outside_a_scope() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());

        DeferredCallbacks::current().defer(async { Ok(()) });
        assert_eq!(container.make::<DeferredCallbacks>().len(), 1);

        let scoped = Arc::new(DeferredCallbacks::new());
        scoped
            .clone()
            .scope(async { DeferredCallbacks::current().defer(async { Ok(()) }) })
            .await;
        assert_eq!(scoped.len(), 1);
        assert_eq!(container.make::<DeferredCallbacks>().len(), 1);
    }

    #[test]
    fn callbacks_can_be_flushed() {
        let deferred = DeferredCallbacks::new();
        deferred.defer(async { Ok(()) });
        deferred.flush();
        assert!(deferred.is_empty());
        assert_eq!(format!("{deferred:?}"), "DeferredCallbacks { len: 0 }");
    }
}
