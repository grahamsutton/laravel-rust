//! Deferring events: `Event::defer(async { ... })` holds back the events
//! dispatched inside the block, and dispatches them once it finishes.

use std::future::Future;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use illuminate_support::Result;

use crate::dispatcher::Dispatcher;

/// A held-back event, ready to be dispatched.
type DeferredEvent = Box<dyn for<'a> FnOnce(&'a Dispatcher) -> BoxFuture<'a, Result<bool>> + Send>;

#[derive(Default)]
struct Deferral {
    /// The events to defer (`None` defers every event).
    only: Option<Vec<String>>,
    events: Vec<DeferredEvent>,
}

tokio::task_local! {
    /// The deferral of the current task (`None` while deferred events are
    /// being dispatched, so they aren't deferred again).
    static DEFERRAL: Option<Arc<Mutex<Deferral>>>;
}

/// Hold the event back if the current task is deferring it, returning it
/// otherwise.
pub(crate) fn hold(name: &str, event: DeferredEvent) -> Option<DeferredEvent> {
    let Some(deferral) = DEFERRAL.try_with(Clone::clone).ok().flatten() else {
        return Some(event);
    };
    let mut deferral = deferral.lock().unwrap();
    let deferred = match &deferral.only {
        None => true,
        Some(only) => only.iter().any(|candidate| matches(candidate, name)),
    };
    if !deferred {
        return Some(event);
    }
    deferral.events.push(event);
    None
}

/// Whether an event name given to `defer_only` names the event: its full
/// name, or (for typed events) the type's name without its path.
fn matches(candidate: &str, name: &str) -> bool {
    candidate == name || name.rsplit("::").next() == Some(candidate)
}

impl Dispatcher {
    /// Run the future, dispatching the events it fires only once it
    /// completes. If it fails, they're never dispatched.
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use illuminate_events::Dispatcher;
    ///
    /// struct OrderShipped;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let events = Dispatcher::new();
    /// let log = Arc::new(Mutex::new(Vec::new()));
    /// let listener_log = log.clone();
    /// events.listen(move |_: &OrderShipped| {
    ///     listener_log.lock().unwrap().push("listener");
    ///     async {}
    /// });
    ///
    /// events
    ///     .defer(async {
    ///         events.dispatch(OrderShipped).await?;
    ///         log.lock().unwrap().push("after dispatching");
    ///         Ok(())
    ///     })
    ///     .await?;
    ///
    /// assert_eq!(*log.lock().unwrap(), ["after dispatching", "listener"]);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn defer<R>(&self, callback: impl Future<Output = Result<R>>) -> Result<R> {
        self.defer_events(None, callback).await
    }

    /// Run the future, deferring only the given events: event names, or
    /// the names of event types (`"OrderShipped"`).
    pub async fn defer_only<S: Into<String>, R>(
        &self,
        events: impl IntoIterator<Item = S>,
        callback: impl Future<Output = Result<R>>,
    ) -> Result<R> {
        let only = events.into_iter().map(Into::into).collect();
        self.defer_events(Some(only), callback).await
    }

    async fn defer_events<R>(&self, only: Option<Vec<String>>, callback: impl Future<Output = Result<R>>) -> Result<R> {
        let deferral = Arc::new(Mutex::new(Deferral { only, events: Vec::new() }));
        let result = DEFERRAL.scope(Some(deferral.clone()), callback).await?;

        let events = std::mem::take(&mut deferral.lock().unwrap().events);
        DEFERRAL
            .scope(None, async {
                for event in events {
                    event(self).await?;
                }
                Ok::<_, illuminate_support::Error>(())
            })
            .await?;

        Ok(result)
    }
}
