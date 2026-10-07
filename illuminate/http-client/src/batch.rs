//! Request batches: concurrent requests with completion callbacks.
//!
//! ```
//! use illuminate_http_client::Http;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! Http::fake_urls([("*/missing", 404)]);
//! Http::fake();
//!
//! let responses = Http::batch(|batch| vec![
//!     batch.get("http://localhost/first"),
//!     batch.as_("second").get("http://localhost/second"),
//!     batch.get("http://localhost/missing"),
//! ])
//! .before(|batch| assert_eq!(batch.total_requests, 3))
//! .progress(|_batch, key, response| println!("[{key}] completed with {}", response.status()))
//! .catch(|_batch, key, _result| println!("[{key}] failed"))
//! .then(|_batch, _results| println!("Every request succeeded!"))
//! .finally(|batch, _results| assert!(batch.has_failures()))
//! .concurrency(2)
//! .send()
//! .await;
//!
//! assert!(responses["second"].ok());
//! assert!(responses[1].not_found());
//! # });
//! ```

use std::fmt;
use std::sync::Arc;

use futures::StreamExt;
use illuminate_support::{Carbon, Result};

use crate::exceptions::{ConnectionException, RequestException};
use crate::pending_request::ResponseFuture;
use crate::pool::{PoolKey, PoolResponses, keys_for};
use crate::response::Response;

type BatchCallback = Arc<dyn Fn(&Batch) + Send + Sync>;
type ProgressCallback = Arc<dyn Fn(&Batch, &PoolKey, &Response) + Send + Sync>;
type CatchCallback = Arc<dyn Fn(&Batch, &PoolKey, &Result<Response>) + Send + Sync>;
type ResultsCallback = Arc<dyn Fn(&Batch, &PoolResponses) + Send + Sync>;

/// A batch of requests sent concurrently, with callbacks to inspect its
/// progress.
pub struct Batch {
    requests: Vec<ResponseFuture>,
    /// The number of requests in the batch.
    pub total_requests: usize,
    /// The number of requests that haven't completed yet.
    pub pending_requests: usize,
    /// The number of requests that have failed.
    pub failed_requests: usize,
    /// When the batch was created.
    pub created_at: Carbon,
    /// When the batch finished, if it has.
    pub finished_at: Option<Carbon>,
    before: Option<BatchCallback>,
    progress: Option<ProgressCallback>,
    catch: Option<CatchCallback>,
    then: Option<ResultsCallback>,
    finally: Option<ResultsCallback>,
    concurrency: usize,
}

impl Batch {
    pub(crate) fn new(requests: Vec<ResponseFuture>) -> Self {
        Self {
            total_requests: requests.len(),
            pending_requests: requests.len(),
            failed_requests: 0,
            requests,
            created_at: Carbon::now(),
            finished_at: None,
            before: None,
            progress: None,
            catch: None,
            then: None,
            finally: None,
            concurrency: 0,
        }
    }

    /// Register a callback to run before the first request is sent.
    pub fn before(mut self, callback: impl Fn(&Batch) + Send + Sync + 'static) -> Self {
        self.before = Some(Arc::new(callback));
        self
    }

    /// Register a callback to run after each successful request.
    pub fn progress(
        mut self,
        callback: impl Fn(&Batch, &PoolKey, &Response) + Send + Sync + 'static,
    ) -> Self {
        self.progress = Some(Arc::new(callback));
        self
    }

    /// Register a callback to run after each failed request (an error
    /// response, or an error).
    pub fn catch(
        mut self,
        callback: impl Fn(&Batch, &PoolKey, &Result<Response>) + Send + Sync + 'static,
    ) -> Self {
        self.catch = Some(Arc::new(callback));
        self
    }

    /// Register a callback to run once every request has succeeded.
    pub fn then(
        mut self,
        callback: impl Fn(&Batch, &PoolResponses) + Send + Sync + 'static,
    ) -> Self {
        self.then = Some(Arc::new(callback));
        self
    }

    /// Register a callback to run once the batch has finished.
    pub fn finally(
        mut self,
        callback: impl Fn(&Batch, &PoolResponses) + Send + Sync + 'static,
    ) -> Self {
        self.finally = Some(Arc::new(callback));
        self
    }

    /// Limit the number of requests in flight at once.
    pub fn concurrency(mut self, limit: usize) -> Self {
        self.concurrency = limit;
        self
    }

    /// The number of requests processed so far.
    pub fn processed_requests(&self) -> usize {
        self.total_requests - self.pending_requests
    }

    /// Determine if the batch has finished.
    pub fn finished(&self) -> bool {
        self.finished_at.is_some()
    }

    /// Determine if any request in the batch failed.
    pub fn has_failures(&self) -> bool {
        self.failed_requests > 0
    }

    /// Send the batch, returning the results in the order the requests were
    /// added.
    pub async fn send(mut self) -> PoolResponses {
        if let Some(before) = self.before.clone() {
            before(&self);
        }

        let requests = std::mem::take(&mut self.requests);
        let keys = keys_for(&requests);
        let limit = if self.concurrency == 0 {
            requests.len().max(1)
        } else {
            self.concurrency
        };

        let mut completed = futures::stream::iter(requests.into_iter().enumerate())
            .map(|(position, request)| async move { (position, request.await) })
            .buffer_unordered(limit);

        let mut results: Vec<(usize, Result<Response>)> = Vec::with_capacity(keys.len());

        while let Some((position, result)) = completed.next().await {
            self.pending_requests -= 1;
            let key = &keys[position];

            match &result {
                Ok(response) if response.successful() => {
                    if let Some(progress) = self.progress.clone() {
                        progress(&self, key, response);
                    }
                }
                Ok(response) if !response.failed() => {}
                Err(error)
                    if !error.is::<RequestException>() && !error.is::<ConnectionException>() => {}
                _ => {
                    self.failed_requests += 1;
                    if let Some(catch) = self.catch.clone() {
                        catch(&self, key, &result);
                    }
                }
            }

            results.push((position, result));
        }

        results.sort_by_key(|(position, _)| *position);
        let responses = PoolResponses::new(
            keys.into_iter()
                .zip(results.into_iter().map(|(_, result)| result))
                .collect(),
        );

        if !self.has_failures()
            && let Some(then) = self.then.clone()
        {
            then(&self, &responses);
        }

        if let Some(finally) = self.finally.clone() {
            finally(&self, &responses);
        }

        self.finished_at = Some(Carbon::now());
        responses
    }
}

impl fmt::Debug for Batch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Batch")
            .field("total_requests", &self.total_requests)
            .field("pending_requests", &self.pending_requests)
            .field("failed_requests", &self.failed_requests)
            .finish_non_exhaustive()
    }
}
