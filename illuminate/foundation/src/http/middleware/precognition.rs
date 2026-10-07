//! Laravel Precognition: the `precognitive` route middleware.

use illuminate_http::{Middleware, Next, Precognition, Request, Response, async_trait, header};
use illuminate_support::Result;

/// Handle precognitive requests — the `precognitive` middleware.
///
/// A request carrying the `Precognition: true` header is marked as
/// [precognitive](Request::is_precognitive): the route's middleware run and
/// its handler's arguments are resolved — form requests are validated,
/// models are bound — but the handler itself never runs. A successful
/// prediction is answered with `204 No Content` and
/// `Precognition-Success: true`; anything else (a `422` with validation
/// errors, a `403`, ...) is returned as usual. Every precognitive response
/// carries a `Precognition: true` header, and every response passing
/// through the middleware gets `Vary: Precognition`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_foundation::http::middleware::HandlePrecognitiveRequests;
/// use illuminate_http::Request;
/// use illuminate_routing::{Route, RouteMiddleware};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let _guard = Container::set_local_instance(Arc::new(Container::new()));
///
/// // In an application, use the alias: `.middleware("precognitive")`.
/// Route::post("/users", || async { "Created!" })
///     .middleware(RouteMiddleware::of(HandlePrecognitiveRequests));
///
/// let request = Request::create("/users", "POST");
/// request.set_header("Precognition", "true");
/// let response = Route::router().dispatch(request).await;
///
/// assert_eq!(response.status_code(), 204);
/// assert_eq!(response.header("Precognition").as_deref(), Some("true"));
/// assert_eq!(response.header("Precognition-Success").as_deref(), Some("true"));
/// assert_eq!(response.header("Vary").as_deref(), Some("Precognition"));
///
/// let response = Route::router().dispatch(Request::create("/users", "POST")).await;
/// assert_eq!(response.content_string(), "Created!");
/// assert_eq!(response.header("Vary").as_deref(), Some("Precognition"));
/// # });
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct HandlePrecognitiveRequests;

impl HandlePrecognitiveRequests {
    /// Create a new middleware instance.
    pub fn new() -> Self {
        Self
    }

    /// Prepare to handle a precognitive request: mark it as precognitive,
    /// so the router resolves the handler's arguments without running it.
    fn prepare_for_precognition(&self, request: &Request) {
        request.set_attribute("precognitive", true);
    }
}

#[async_trait]
impl Middleware for HandlePrecognitiveRequests {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        if !request.is_attempting_precognition() {
            return Ok(append_vary_header(next.run(request).await));
        }

        self.prepare_for_precognition(&request);

        let mut response = next.run(request).await;
        response.set_header(Precognition::HEADER, "true");

        Ok(append_vary_header(response))
    }
}

/// Append `Precognition` to the response's `Vary` header.
fn append_vary_header(mut response: Response) -> Response {
    let mut vary: Vec<String> = response
        .headers()
        .get_all(header::VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    let listed = vary
        .iter()
        .flat_map(|value| value.split(','))
        .any(|name| name.trim().eq_ignore_ascii_case(Precognition::HEADER));
    if !listed {
        vary.push(Precognition::HEADER.to_string());
    }
    response.set_header(header::VARY.as_str(), &vary.join(", "));
    response
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use illuminate_http::{Destination, run_middleware};

    use super::*;

    fn destination(response: fn() -> Response) -> Destination {
        Arc::new(move |request: Request| {
            Box::pin(async move {
                response().with_header("x-precognitive", &request.is_precognitive().to_string())
            })
        })
    }

    async fn run(request: Request, response: fn() -> Response) -> Response {
        run_middleware(
            request,
            vec![Arc::new(HandlePrecognitiveRequests)],
            destination(response),
        )
        .await
    }

    fn precognitive_request() -> Request {
        let request = Request::create("/users", "POST");
        request.set_header("Precognition", "true");
        request
    }

    #[tokio::test]
    async fn precognitive_requests_are_marked_and_answered_with_the_header() {
        let request = precognitive_request();
        let response = run(request.clone(), || Response::new("ok")).await;

        assert!(request.is_precognitive());
        assert_eq!(response.header("x-precognitive").as_deref(), Some("true"));
        assert_eq!(response.header("precognition").as_deref(), Some("true"));
        assert_eq!(response.header("vary").as_deref(), Some("Precognition"));
    }

    #[tokio::test]
    async fn other_requests_only_get_the_vary_header() {
        let request = Request::create("/users", "POST");
        request.set_header("Precognition", "false");
        let response = run(request.clone(), || Response::new("ok")).await;

        assert!(!request.is_precognitive());
        assert_eq!(response.header("x-precognitive").as_deref(), Some("false"));
        assert!(response.header("precognition").is_none());
        assert_eq!(response.header("vary").as_deref(), Some("Precognition"));
    }

    #[tokio::test]
    async fn existing_vary_headers_are_kept() {
        let response = run(precognitive_request(), || {
            let mut response = Response::new("ok").with_header("Vary", "Accept-Encoding");
            response.append_header("Vary", "Origin");
            response
        })
        .await;
        assert_eq!(
            response.header("vary").as_deref(),
            Some("Accept-Encoding, Origin, Precognition")
        );

        let response = run(precognitive_request(), || {
            Response::new("ok").with_header("Vary", "precognition")
        })
        .await;
        assert_eq!(response.header("vary").as_deref(), Some("precognition"));
    }

    #[tokio::test]
    async fn failed_responses_are_marked_as_precognitive_too() {
        let response = run(precognitive_request(), || Response::make("Invalid", 422)).await;
        assert_eq!(response.status_code(), 422);
        assert_eq!(response.header("precognition").as_deref(), Some("true"));
        assert!(response.header("precognition-success").is_none());
    }
}
