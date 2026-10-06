use illuminate_http::{Middleware, Next, Request, Response, async_trait, current_request, with_request};
use illuminate_support::Result;

use crate::jar::CookieQueue;

/// Attaches the cookies queued during the request (via `Cookie::queue`) to
/// the outgoing response.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_cookie::{AddQueuedCookiesToResponse, facades::Cookie};
/// use illuminate_http::{Request, Response, run_middleware};
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// # let container = Arc::new(illuminate_container::Container::new());
/// # let _guard = illuminate_container::Container::set_local_instance(container);
/// let response = run_middleware(
///     Request::create("/", "GET"),
///     vec![Arc::new(AddQueuedCookiesToResponse)],
///     Arc::new(|_request| Box::pin(async {
///         Cookie::queue_make("name", "value", 60);
///         Response::new("Hello World")
///     })),
/// ).await;
///
/// assert_eq!(response.get_cookie("name").unwrap().value, "value");
/// # });
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct AddQueuedCookiesToResponse;

impl AddQueuedCookiesToResponse {
    /// Create the middleware.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Middleware for AddQueuedCookiesToResponse {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let queue = CookieQueue::for_request(&request);

        let mut response = if current_request().is_some() {
            next.run(request).await
        } else {
            with_request(request.clone(), next.run(request)).await
        };

        for cookie in queue.all() {
            response.add_cookie(cookie);
        }

        Ok(response)
    }
}
