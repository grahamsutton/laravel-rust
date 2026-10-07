//! Work deferred until after the response has been sent.
//!
//! `dispatch_after_response`, the `deferred` queue connection and
//! `PendingBatch::dispatch_after_response` all add callbacks to the
//! current [`DeferredCallbacks`]. While a request is being handled that is
//! a collection attached to the request; otherwise it is the one bound in
//! the container.
//!
//! The HTTP kernel runs the request's callbacks once the response has been
//! sent (and the console kernel runs the container's when the command
//! finishes) by awaiting [`DeferredCallbacks::invoke`].

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use illuminate_container::{Container, try_app};
use illuminate_http::{BoxFuture, current_request};

type Callback = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

/// A collection of callbacks to run after the response has been sent
/// (Laravel's `DeferredCallbackCollection`).
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use illuminate_queue::DeferredCallbacks;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let deferred = DeferredCallbacks::new();
/// let ran = Arc::new(AtomicBool::new(false));
///
/// let flag = ran.clone();
/// deferred.defer(move || async move { flag.store(true, Ordering::SeqCst) });
/// assert!(!ran.load(Ordering::SeqCst));
///
/// // ...the response is sent...
/// deferred.invoke().await;
/// assert!(ran.load(Ordering::SeqCst));
/// # });
/// ```
#[derive(Default)]
pub struct DeferredCallbacks {
    callbacks: Mutex<Vec<Callback>>,
}

impl DeferredCallbacks {
    /// Create an empty collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// The collection for the current context: the current request's, or
    /// the container's.
    pub fn current() -> Arc<DeferredCallbacks> {
        if let Some(request) = current_request() {
            if let Some(callbacks) = request.extension::<DeferredCallbacks>() {
                return callbacks;
            }
            let callbacks = Arc::new(DeferredCallbacks::new());
            request.set_extension(callbacks.clone());
            return callbacks;
        }

        if let Some(callbacks) = try_app::<DeferredCallbacks>() {
            return callbacks;
        }
        let container = Container::get_instance();
        container.singleton_if::<DeferredCallbacks>(|_| Arc::new(DeferredCallbacks::new()));
        container.make::<DeferredCallbacks>()
    }

    /// Defer the callback until after the response.
    pub fn defer<F, Fut>(&self, callback: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.callbacks
            .lock()
            .unwrap()
            .push(Box::new(move || Box::pin(callback())));
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

    /// Run every deferred callback (including callbacks deferred while
    /// running them), in the order they were deferred.
    pub async fn invoke(&self) {
        loop {
            let callbacks = std::mem::take(&mut *self.callbacks.lock().unwrap());
            if callbacks.is_empty() {
                return;
            }
            for callback in callbacks {
                callback().await;
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

/// A future that runs with the given container as the current one, even
/// when it is polled on another thread (spawned tasks).
pub(crate) struct WithContainer<F> {
    container: Arc<Container>,
    future: Pin<Box<F>>,
}

impl<F> WithContainer<F> {
    pub(crate) fn new(container: Arc<Container>, future: F) -> Self {
        Self {
            container,
            future: Box::pin(future),
        }
    }
}

impl<F: Future> Future for WithContainer<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let _guard = Container::set_local_instance(this.container.clone());
        this.future.as_mut().poll(cx)
    }
}

/// Spawn the future onto the runtime with the current container.
pub(crate) fn spawn_with_container<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(WithContainer::new(Container::get_instance(), future))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn callbacks_run_in_order_including_nested_ones() {
        let deferred = Arc::new(DeferredCallbacks::new());
        let order = Arc::new(Mutex::new(Vec::new()));

        let (first, nested_deferred) = (order.clone(), deferred.clone());
        deferred.defer(move || async move {
            first.lock().unwrap().push(1);
            let nested = first.clone();
            nested_deferred.defer(move || async move { nested.lock().unwrap().push(3) });
        });
        let second = order.clone();
        deferred.defer(move || async move { second.lock().unwrap().push(2) });

        assert_eq!(deferred.len(), 2);
        deferred.invoke().await;
        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3]);
        assert!(deferred.is_empty());
    }

    #[tokio::test]
    async fn requests_get_their_own_collection() {
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container);

        let request = illuminate_http::Request::default();
        let callbacks = illuminate_http::with_request(request.clone(), async {
            let callbacks = DeferredCallbacks::current();
            callbacks.defer(|| async {});
            assert!(Arc::ptr_eq(&callbacks, &DeferredCallbacks::current()));
            callbacks
        })
        .await;

        assert_eq!(request.extension::<DeferredCallbacks>().unwrap().len(), 1);
        assert!(!Arc::ptr_eq(&callbacks, &DeferredCallbacks::current()));
        assert!(DeferredCallbacks::current().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawned_futures_keep_the_container() {
        struct Marker(usize);
        let container = Arc::new(Container::new());
        container.instance(Marker(7));
        let _guard = Container::set_local_instance(container);

        let seen = Arc::new(AtomicUsize::new(0));
        let observed = seen.clone();
        spawn_with_container(async move {
            tokio::task::yield_now().await;
            observed.store(illuminate_container::app::<Marker>().0, Ordering::SeqCst);
        })
        .await
        .unwrap();
        assert_eq!(seen.load(Ordering::SeqCst), 7);
    }
}
