//! Concurrent requests.
//!
//! ```
//! use illuminate_http_client::Http;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! Http::fake();
//!
//! let responses = Http::pool(|pool| vec![
//!     pool.get("http://localhost/first"),
//!     pool.get("http://localhost/second"),
//!     pool.as_("third").with_headers([("X-Example", "example")]).get("http://localhost/third"),
//! ])
//! .await;
//!
//! assert!(responses[0].ok() && responses[1].ok() && responses["third"].ok());
//! # });
//! ```

use std::fmt;
use std::ops::Index;
use std::sync::Arc;

use futures::StreamExt;
use illuminate_support::Result;

use crate::factory::Factory;
use crate::pending_request::{PendingRequest, ResponseFuture};
use crate::response::Response;

/// Builds the requests of a pool (or batch).
///
/// Every method of [`PendingRequest`] that begins a request is available
/// here too, so each pooled request may be configured on its own.
pub struct Pool {
    factory: Option<Arc<Factory>>,
}

impl Pool {
    pub(crate) fn new(factory: Option<Arc<Factory>>) -> Self {
        Self { factory }
    }

    /// Create a new pending request for the pool.
    pub fn new_request(&self) -> PendingRequest {
        match &self.factory {
            Some(factory) => factory.create_pending_request(),
            None => PendingRequest::new(),
        }
    }

    /// Name the next request, so its response may be retrieved by name.
    pub fn as_(&self, key: impl Into<String>) -> PendingRequest {
        self.new_request().pool_key(key.into())
    }
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Pool")
    }
}

/// The key of a pooled response: its position among the unnamed requests,
/// or the name given with [`Pool::as_`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PoolKey {
    /// An unnamed request's position.
    Index(usize),
    /// A named request.
    Name(String),
}

impl fmt::Display for PoolKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PoolKey::Index(index) => write!(f, "{index}"),
            PoolKey::Name(name) => f.write_str(name),
        }
    }
}

/// The results of a pool, in the order the requests were added.
///
/// Each result is the [`Response`], or the error (usually a
/// [`ConnectionException`](crate::ConnectionException)) if the request
/// failed. Indexing returns the response directly, panicking if the request
/// failed; use [`PoolResponses::get`] and [`PoolResponses::named`] to handle
/// failures gracefully.
pub struct PoolResponses {
    entries: Vec<(PoolKey, Result<Response>)>,
}

impl PoolResponses {
    pub(crate) fn new(entries: Vec<(PoolKey, Result<Response>)>) -> Self {
        Self { entries }
    }

    /// Get the result of the unnamed request at the given position.
    pub fn get(&self, index: usize) -> Option<&Result<Response>> {
        self.find(&PoolKey::Index(index))
    }

    /// Get the result of the named request.
    pub fn named(&self, name: &str) -> Option<&Result<Response>> {
        self.find(&PoolKey::Name(name.to_string()))
    }

    fn find(&self, key: &PoolKey) -> Option<&Result<Response>> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, result)| result)
    }

    /// Iterate over the keys and results.
    pub fn iter(&self) -> impl Iterator<Item = (&PoolKey, &Result<Response>)> {
        self.entries.iter().map(|(key, result)| (key, result))
    }

    /// The keys of the results.
    pub fn keys(&self) -> Vec<&PoolKey> {
        self.entries.iter().map(|(key, _)| key).collect()
    }

    /// The number of results.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Determine if the pool was empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the keys and results as a vector.
    pub fn into_vec(self) -> Vec<(PoolKey, Result<Response>)> {
        self.entries
    }

    fn expect(&self, key: PoolKey) -> &Response {
        match self.find(&key) {
            Some(Ok(response)) => response,
            Some(Err(error)) => panic!("The pooled request [{key}] failed: {error}"),
            None => panic!("The pool has no request [{key}]."),
        }
    }
}

impl Index<usize> for PoolResponses {
    type Output = Response;

    fn index(&self, index: usize) -> &Response {
        self.expect(PoolKey::Index(index))
    }
}

impl Index<&str> for PoolResponses {
    type Output = Response;

    fn index(&self, name: &str) -> &Response {
        self.expect(PoolKey::Name(name.to_string()))
    }
}

impl IntoIterator for PoolResponses {
    type Item = (PoolKey, Result<Response>);
    type IntoIter = std::vec::IntoIter<(PoolKey, Result<Response>)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl fmt::Debug for PoolResponses {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.entries.iter().map(|(key, result)| {
                let summary = match result {
                    Ok(response) => format!("{} {}", response.status(), response.reason()),
                    Err(error) => format!("Error: {error}"),
                };
                (key.to_string(), summary)
            }))
            .finish()
    }
}

/// Assign keys to requests: names for named requests, and positions
/// (counting only unnamed requests) for the rest.
pub(crate) fn keys_for(requests: &[ResponseFuture]) -> Vec<PoolKey> {
    let mut next = 0;
    requests
        .iter()
        .map(|request| match request.key() {
            Some(name) => PoolKey::Name(name.to_string()),
            None => {
                next += 1;
                PoolKey::Index(next - 1)
            }
        })
        .collect()
}

/// Send the requests concurrently, keeping their order.
pub(crate) async fn run_pool(requests: Vec<ResponseFuture>, concurrency: usize) -> PoolResponses {
    let keys = keys_for(&requests);
    let limit = if concurrency == 0 {
        requests.len().max(1)
    } else {
        concurrency
    };

    let results: Vec<Result<Response>> = futures::stream::iter(requests)
        .buffered(limit)
        .collect()
        .await;

    PoolResponses::new(keys.into_iter().zip(results).collect())
}
