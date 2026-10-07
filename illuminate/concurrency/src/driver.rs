//! Concurrency drivers: how a list of tasks actually runs.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::future::BoxFuture;
use illuminate_container::Container;
use illuminate_process::IntoTimeout;
use illuminate_support::Result;
use illuminate_support::error::RuntimeException;
use tokio::task::JoinSet;

use crate::deferred::DeferredCallbacks;
use crate::tasks::{Job, JobOutput, Tasks};

/// A concurrency driver (Laravel's `Illuminate\Contracts\Concurrency\Driver`).
///
/// A driver receives type-erased jobs and returns their outputs in the same
/// order. You call the friendlier, typed methods on `dyn Driver`:
/// [`run`](#method.run), [`run_with_timeout`](#method.run_with_timeout) and
/// [`defer`](#method.defer).
///
/// ```
/// use illuminate_concurrency::{Driver, SyncDriver};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let driver: &dyn Driver = &SyncDriver;
/// let (two, four) = driver.run((async { Ok(1 + 1) }, async { Ok(2 + 2) })).await?;
///
/// assert_eq!((two, four), (2, 4));
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub trait Driver: Send + Sync + 'static {
    /// Run the jobs, returning their outputs in order. The first job to
    /// fail (in order) fails the whole run.
    ///
    /// The returned future must not borrow the driver, so it can be
    /// deferred.
    fn execute(
        &self,
        jobs: Vec<Job>,
        timeout: Option<Duration>,
    ) -> BoxFuture<'static, Result<Vec<JobOutput>>>;
}

impl dyn Driver {
    /// Run the given tasks concurrently and return their results.
    pub async fn run<T: Tasks>(&self, tasks: T) -> Result<T::Output> {
        let (jobs, assemble) = tasks.into_jobs();
        let outputs = self.execute(jobs, None).await?;
        Ok(assemble(outputs))
    }

    /// Run the given tasks concurrently, failing with a
    /// [`TaskTimedOutException`] if they don't all finish in time.
    pub async fn run_with_timeout<T: Tasks>(
        &self,
        tasks: T,
        timeout: impl IntoTimeout,
    ) -> Result<T::Output> {
        let (jobs, assemble) = tasks.into_jobs();
        let outputs = self.execute(jobs, Some(timeout.into_timeout())).await?;
        Ok(assemble(outputs))
    }

    /// Run the given tasks concurrently after the response has been sent
    /// (see [`DeferredCallbacks`]). Their results are discarded.
    pub fn defer<T: Tasks>(&self, tasks: T) {
        let (jobs, _) = tasks.into_jobs();
        let run = self.execute(jobs, None);
        DeferredCallbacks::current().defer(async move { run.await.map(drop) });
    }
}

/// Thrown when concurrent tasks run past their timeout.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("The concurrent tasks exceeded the timeout of {} seconds.", .timeout.as_secs_f64())]
pub struct TaskTimedOutException {
    /// How long the tasks were allowed to run.
    pub timeout: Duration,
}

/// The default driver: every task runs on its own Tokio task, so they run
/// at the same time (and in parallel on a multi-threaded runtime).
///
/// Every task is awaited, even when one fails; then the first failure (in
/// task order) is returned, just like Laravel throws the first failed
/// task's exception. A task that panics fails with a `RuntimeException`.
/// The current container follows each task onto whichever thread runs it.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioDriver;

impl Driver for TokioDriver {
    fn execute(
        &self,
        jobs: Vec<Job>,
        timeout: Option<Duration>,
    ) -> BoxFuture<'static, Result<Vec<JobOutput>>> {
        let container = Container::get_instance();

        Box::pin(async move {
            let count = jobs.len();
            let mut set = JoinSet::new();
            let mut positions = HashMap::with_capacity(count);

            for (position, job) in jobs.into_iter().enumerate() {
                let handle = set.spawn(WithContainer::new(container.clone(), job));
                positions.insert(handle.id(), position);
            }

            let mut outputs: Vec<Option<Result<JobOutput>>> = (0..count).map(|_| None).collect();

            let collect = async {
                while let Some(joined) = set.join_next_with_id().await {
                    match joined {
                        Ok((id, output)) => outputs[positions[&id]] = Some(output),
                        Err(error) => {
                            let position = positions[&error.id()];
                            outputs[position] = Some(Err(panicked(error)));
                        }
                    }
                }
            };

            match timeout {
                Some(timeout) => {
                    if tokio::time::timeout(timeout, collect).await.is_err() {
                        set.abort_all();
                        return Err(TaskTimedOutException { timeout }.into());
                    }
                }
                None => collect.await,
            }

            outputs
                .into_iter()
                .map(|output| {
                    output.unwrap_or_else(|| {
                        Err(RuntimeException::new("A concurrent task was cancelled.").into())
                    })
                })
                .collect()
        })
    }
}

/// Turn a task's join error into an exception.
fn panicked(error: tokio::task::JoinError) -> illuminate_support::Error {
    if error.is_cancelled() {
        return RuntimeException::new("A concurrent task was cancelled.").into();
    }
    let payload = error.into_panic();
    let message = payload
        .downcast_ref::<&str>()
        .map(|message| message.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Box<dyn Any>".to_string());
    RuntimeException::new(format!("A concurrent task panicked: {message}")).into()
}

/// Runs every task in sequence, in the current task. Useful in tests when
/// you want to rule concurrency out.
///
/// Like Laravel's sync driver, the first failure stops the run.
#[derive(Clone, Copy, Debug, Default)]
pub struct SyncDriver;

impl Driver for SyncDriver {
    fn execute(
        &self,
        jobs: Vec<Job>,
        timeout: Option<Duration>,
    ) -> BoxFuture<'static, Result<Vec<JobOutput>>> {
        Box::pin(async move {
            let run = async {
                let mut outputs = Vec::with_capacity(jobs.len());
                for job in jobs {
                    outputs.push(job.await?);
                }
                Ok(outputs)
            };

            match timeout {
                Some(timeout) => tokio::time::timeout(timeout, run)
                    .await
                    .map_err(|_| TaskTimedOutException { timeout })?,
                None => run.await,
            }
        })
    }
}

/// A future that runs with the given container as the current one, even
/// when it is polled on another thread.
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

impl fmt::Debug for dyn Driver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("dyn Driver")
    }
}
