//! The `throttle` middleware: Laravel's `ThrottleRequests`.
//!
//! ```ignore
//! Route::get("/api/user", handler).middleware("throttle:60,1");   // 60 requests per minute
//! Route::get("/uploads", handler).middleware("throttle:uploads"); // a named limiter
//! ```
//!
//! Requests are keyed by the authenticated user's id when there is one —
//! read from the request attribute **`_auth_id`**, which the authentication
//! middleware sets — and by the route's domain (request attribute
//! **`_route_domain`**, when the router sets it) plus the client's IP
//! address otherwise.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use md5::{Digest as _, Md5};
use sha1::Sha1;

use illuminate_http::{HeaderMap, HeaderName, HeaderValue, HttpException, HttpResponseException, Middleware, Next, Request, Response};
use illuminate_support::{Carbon, Error, Result, Value, ValueExt};

use crate::limit::{AfterCallback, LimiterResponse, ResponseCallback};
use crate::rate_limiter::RateLimiter;

static SHOULD_HASH_KEYS: AtomicBool = AtomicBool::new(true);

/// Thrown when a `throttle` middleware names a limiter that doesn't exist.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Rate limiter [{limiter}] is not defined.")]
pub struct MissingRateLimiterException {
    pub limiter: String,
}

/// Build Laravel's `ThrottleRequestsException`: a `429 Too Many Requests`
/// [`HttpException`] carrying the rate limit headers.
pub fn throttle_requests_exception(message: impl Into<String>, headers: &HeaderMap) -> HttpException {
    let mut exception = HttpException::with_message(429, message);
    for (name, value) in headers {
        exception = exception.header(header_case(name.as_str()), value.to_str().unwrap_or_default());
    }
    exception
}

/// Restore the conventional casing of the rate limit headers.
fn header_case(name: &str) -> String {
    match name {
        "x-ratelimit-limit" => "X-RateLimit-Limit".into(),
        "x-ratelimit-remaining" => "X-RateLimit-Remaining".into(),
        "x-ratelimit-reset" => "X-RateLimit-Reset".into(),
        "retry-after" => "Retry-After".into(),
        other => other.into(),
    }
}

/// One limit, resolved for the current request.
struct ResolvedLimit {
    key: String,
    max_attempts: i64,
    decay_seconds: u64,
    after_callback: Option<AfterCallback>,
    response_callback: Option<ResponseCallback>,
}

/// The `throttle` middleware.
#[derive(Debug, Clone)]
pub struct ThrottleRequests {
    parameters: Vec<String>,
}

impl ThrottleRequests {
    /// Throttle with the given middleware parameters (`["60", "1"]`,
    /// `["api"]`, `["60", "1", "prefix"]`).
    pub fn new(parameters: &[String]) -> Self {
        Self { parameters: parameters.to_vec() }
    }

    /// Throttle using a named rate limiter.
    pub fn using(name: &str) -> Self {
        Self { parameters: vec![name.to_string()] }
    }

    /// Throttle to `max_attempts` per `decay_minutes`, with an optional key prefix.
    pub fn with(max_attempts: i64, decay_minutes: u64, prefix: &str) -> Self {
        Self { parameters: vec![max_attempts.to_string(), decay_minutes.to_string(), prefix.to_string()] }
    }

    /// Specify whether rate limiter keys should be hashed (the default).
    pub fn should_hash_keys(should_hash_keys: bool) {
        SHOULD_HASH_KEYS.store(should_hash_keys, Ordering::SeqCst);
    }

    fn hashes_keys() -> bool {
        SHOULD_HASH_KEYS.load(Ordering::SeqCst)
    }

    fn limiter() -> Result<Arc<RateLimiter>> {
        crate::provider::rate_limiter()
    }

    async fn handle_request(
        &self,
        limiter: &RateLimiter,
        request: Request,
        next: Next,
        limits: Vec<ResolvedLimit>,
    ) -> Result<Response> {
        for limit in &limits {
            if limiter.too_many_attempts(&limit.key, limit.max_attempts).await? {
                return Err(Self::build_exception(limiter, &request, limit).await?);
            }
        }

        for limit in &limits {
            if limit.after_callback.is_none() {
                limiter.hit(&limit.key, limit.decay_seconds).await?;
            }
        }

        let mut response = next.run(request).await;

        for limit in &limits {
            if let Some(after) = &limit.after_callback
                && after(&response)
            {
                limiter.hit(&limit.key, limit.decay_seconds).await?;
            }
            let remaining = limiter.retries_left(&limit.key, limit.max_attempts).await?;
            add_headers(&mut response, limit.max_attempts, remaining);
        }

        Ok(response)
    }

    fn resolve_request_signature(request: &Request) -> String {
        let user = request.attribute("_auth_id");
        let identifier = if !user.is_null() {
            user.to_string_lossy()
        } else {
            let domain = request.attribute("_route_domain").to_string_lossy();
            format!("{domain}|{}", request.ip().unwrap_or_default())
        };
        if Self::hashes_keys() { hex::encode(Sha1::digest(identifier.as_bytes())) } else { identifier }
    }

    fn resolve_max_attempts(request: &Request, max_attempts: &str) -> Result<i64> {
        let max_attempts = match max_attempts.split_once('|') {
            Some((guest, user)) => {
                if request.attribute("_auth_id").is_null() { guest } else { user }
            }
            None => max_attempts,
        };
        max_attempts
            .trim()
            .parse::<f64>()
            .map(|n| n as i64)
            .map_err(|_| MissingRateLimiterException { limiter: max_attempts.to_string() }.into())
    }

    async fn build_exception(limiter: &RateLimiter, request: &Request, limit: &ResolvedLimit) -> Result<Error> {
        let retry_after = limiter.available_in(&limit.key).await?;
        let headers = get_headers(limit.max_attempts, 0, Some(retry_after));
        Ok(match &limit.response_callback {
            Some(callback) => HttpResponseException::new(callback(request, &headers)).into(),
            None => throttle_requests_exception("Too Many Attempts.", &headers).into(),
        })
    }
}

#[async_trait]
impl Middleware for ThrottleRequests {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let limiter = Self::limiter()?;

        // A single parameter naming a registered limiter uses that limiter.
        if let [name] = self.parameters.as_slice()
            && let Some(named) = limiter.limiter(name)
        {
            let limits = match named(&request) {
                LimiterResponse::Response(response) => return Ok(response),
                LimiterResponse::Limits(limits) => limits,
            };
            if limits.len() == 1 && limits[0].is_unlimited() {
                return Ok(next.run(request).await);
            }
            let limits = limits
                .into_iter()
                .map(|limit| ResolvedLimit {
                    key: if Self::hashes_keys() {
                        hex::encode(Md5::digest(format!("{name}{}", limit.key).as_bytes()))
                    } else {
                        format!("{name}:{}", limit.key)
                    },
                    max_attempts: limit.max_attempts,
                    decay_seconds: limit.decay_seconds,
                    after_callback: limit.after_callback,
                    response_callback: limit.response_callback,
                })
                .collect();
            return self.handle_request(&limiter, request, next, limits).await;
        }

        let parameter = |index: usize, default: &str| -> String {
            self.parameters.get(index).filter(|p| !p.is_empty()).cloned().unwrap_or_else(|| default.to_string())
        };
        let max_attempts = Self::resolve_max_attempts(&request, &parameter(0, "60"))?;
        let decay_minutes: f64 = parameter(1, "1").trim().parse().unwrap_or(1.0);
        let prefix = self.parameters.get(2).cloned().unwrap_or_default();

        let limit = ResolvedLimit {
            key: format!("{prefix}{}", Self::resolve_request_signature(&request)),
            max_attempts,
            decay_seconds: (60.0 * decay_minutes).round() as u64,
            after_callback: None,
            response_callback: None,
        };
        self.handle_request(&limiter, request, next, vec![limit]).await
    }
}

/// Build the rate limit headers.
fn get_headers(max_attempts: i64, remaining_attempts: i64, retry_after: Option<i64>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let mut insert = |name: &'static str, value: i64| {
        if let Ok(value) = HeaderValue::from_str(&value.to_string()) {
            headers.insert(HeaderName::from_static(name), value);
        }
    };
    insert("x-ratelimit-limit", max_attempts);
    insert("x-ratelimit-remaining", remaining_attempts);
    if let Some(retry_after) = retry_after {
        insert("retry-after", retry_after);
        insert("x-ratelimit-reset", Carbon::now().timestamp() + retry_after);
    }
    headers
}

/// Add the limit headers to the response, keeping the most restrictive
/// limit when several apply.
fn add_headers(response: &mut Response, max_attempts: i64, remaining_attempts: i64) {
    let existing = response.header("x-ratelimit-remaining").map(|v| Value::from(v).to_i64_lossy().unwrap_or(0));
    if existing.is_some_and(|existing| existing <= remaining_attempts) {
        return;
    }
    for (name, value) in &get_headers(max_attempts, remaining_attempts, None) {
        response.set_header(name.as_str(), value.to_str().unwrap_or_default());
    }
}

/// A factory for the router's middleware aliases: `throttle:60,1` becomes
/// `throttle_middleware(&["60", "1"])`.
pub fn throttle_middleware(parameters: &[String]) -> Arc<dyn Middleware> {
    Arc::new(ThrottleRequests::new(parameters))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limit::Limit;
    use crate::testing::freeze_time;
    use crate::{CacheServiceProvider, facades};
    use illuminate_config::Repository as Config;
    use illuminate_container::{Container, ServiceProvider};
    use illuminate_http::{Destination, run_middleware};
    use illuminate_support::json;

    fn app() -> (Arc<Container>, illuminate_container::LocalInstanceGuard) {
        let container = Arc::new(Container::new());
        let guard = Container::set_local_instance(container.clone());
        container.instance(Config::new(json!({
            "cache": {"default": "array", "stores": {"array": {"driver": "array"}}},
        })));
        CacheServiceProvider.register(&container);
        (container, guard)
    }

    fn ok() -> Destination {
        Arc::new(|_request| Box::pin(async { Response::new("OK") }))
    }

    fn params(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    async fn send(middleware: &Arc<dyn Middleware>, request: Request) -> Response {
        run_middleware(request, vec![middleware.clone()], ok()).await
    }

    #[tokio::test]
    async fn requests_are_throttled_after_the_limit() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        let throttle = throttle_middleware(&params(&["2", "1"]));

        let response = send(&throttle, Request::create("/", "GET")).await;
        assert_eq!(response.status_code(), 200);
        assert_eq!(response.header("x-ratelimit-limit").unwrap(), "2");
        assert_eq!(response.header("x-ratelimit-remaining").unwrap(), "1");

        let response = send(&throttle, Request::create("/", "GET")).await;
        assert_eq!(response.header("x-ratelimit-remaining").unwrap(), "0");

        time.travel_seconds(15);
        let response = send(&throttle, Request::create("/", "GET")).await;
        assert_eq!(response.status_code(), 429);
        assert!(response.content_string().contains("Too Many Attempts."));
        assert_eq!(response.header("x-ratelimit-limit").unwrap(), "2");
        assert_eq!(response.header("x-ratelimit-remaining").unwrap(), "0");
        assert_eq!(response.header("retry-after").unwrap(), "45");
        assert_eq!(response.header("x-ratelimit-reset").unwrap(), (1_700_000_015 + 45).to_string());

        time.travel_seconds(45);
        assert_eq!(send(&throttle, Request::create("/", "GET")).await.status_code(), 200);
    }

    #[tokio::test]
    async fn the_exception_is_an_http_exception() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        let throttle = ThrottleRequests::with(1, 1, "");
        let next = || Next::new(ok());

        throttle.handle(Request::create("/", "GET"), next()).await.unwrap();
        let error = throttle.handle(Request::create("/", "GET"), next()).await.unwrap_err();
        let exception = error.downcast_ref::<HttpException>().unwrap();
        assert_eq!(exception.status, 429);
        assert_eq!(exception.message(), "Too Many Attempts.");
        let names: Vec<&str> = exception.headers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["X-RateLimit-Limit", "X-RateLimit-Remaining", "Retry-After", "X-RateLimit-Reset"]);
    }

    #[tokio::test]
    async fn users_and_ips_are_limited_separately() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        let throttle = throttle_middleware(&params(&["1", "1"]));

        assert_eq!(send(&throttle, Request::create("/", "GET")).await.status_code(), 200);
        assert_eq!(send(&throttle, Request::create("/", "GET")).await.status_code(), 429);

        let user = Request::create("/", "GET");
        user.set_attribute("_auth_id", 1);
        assert_eq!(send(&throttle, user).await.status_code(), 200);

        let other = Request::create("/", "GET");
        other.set_attribute("_auth_id", 2);
        assert_eq!(send(&throttle, other).await.status_code(), 200);

        let limiter = crate::provider::rate_limiter().unwrap();
        let key = hex::encode(Sha1::digest(b"1"));
        assert_eq!(limiter.attempts(&key).await.unwrap(), 1);
        let key = hex::encode(Sha1::digest(b"|127.0.0.1"));
        assert_eq!(limiter.attempts(&key).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn guests_and_users_can_have_different_limits() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        let throttle = throttle_middleware(&params(&["1|3", "1"]));

        let response = send(&throttle, Request::create("/", "GET")).await;
        assert_eq!(response.header("x-ratelimit-limit").unwrap(), "1");

        let user = Request::create("/", "GET");
        user.set_attribute("_auth_id", 7);
        let response = send(&throttle, user).await;
        assert_eq!(response.header("x-ratelimit-limit").unwrap(), "3");
    }

    #[tokio::test]
    async fn prefixes_separate_limits() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        let a = throttle_middleware(&params(&["1", "1", "a"]));
        let b = throttle_middleware(&params(&["1", "1", "b"]));
        assert_eq!(send(&a, Request::create("/", "GET")).await.status_code(), 200);
        assert_eq!(send(&b, Request::create("/", "GET")).await.status_code(), 200);
        assert_eq!(send(&a, Request::create("/", "GET")).await.status_code(), 429);
    }

    #[tokio::test]
    async fn named_limiters_are_used() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        facades::RateLimiter::for_("api", |request| Limit::per_minute(2).by(request.ip().unwrap_or_default()))
            .unwrap();
        let throttle = throttle_middleware(&params(&["api"]));

        assert_eq!(send(&throttle, Request::create("/", "GET")).await.status_code(), 200);
        let response = send(&throttle, Request::create("/", "GET")).await;
        assert_eq!(response.header("x-ratelimit-remaining").unwrap(), "0");
        assert_eq!(send(&throttle, Request::create("/", "GET")).await.status_code(), 429);

        let key = hex::encode(Md5::digest(b"api127.0.0.1"));
        assert_eq!(facades::RateLimiter::attempts(&key).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn named_limiters_may_be_unlimited_or_respond_directly() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        facades::RateLimiter::for_("vip", |_| Limit::none()).unwrap();
        facades::RateLimiter::for_("closed", |_| Response::make("Closed for maintenance", 503)).unwrap();
        facades::RateLimiter::for_("custom", |_| {
            Limit::per_minute(1).response(|_request, headers| {
                let mut response = Response::make("Slow down!", 429);
                response.headers_mut().extend(headers.clone());
                response
            })
        })
        .unwrap();

        let vip = throttle_middleware(&params(&["vip"]));
        for _ in 0..5 {
            let response = send(&vip, Request::create("/", "GET")).await;
            assert_eq!(response.status_code(), 200);
            assert!(response.header("x-ratelimit-limit").is_none());
        }

        let closed = send(&throttle_middleware(&params(&["closed"])), Request::create("/", "GET")).await;
        assert_eq!((closed.status_code(), closed.content_string()), (503, "Closed for maintenance".to_string()));

        let custom = throttle_middleware(&params(&["custom"]));
        send(&custom, Request::create("/", "GET")).await;
        let response = send(&custom, Request::create("/", "GET")).await;
        assert_eq!(response.status_code(), 429);
        assert_eq!(response.content_string(), "Slow down!");
        assert_eq!(response.header("retry-after").unwrap(), "60");
    }

    #[tokio::test]
    async fn multiple_limits_report_the_most_restrictive() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        facades::RateLimiter::for_("login", |_| [Limit::per_minute(500), Limit::per_minute(3).by("email")]).unwrap();
        let throttle = throttle_middleware(&params(&["login"]));
        let response = send(&throttle, Request::create("/", "GET")).await;
        assert_eq!(response.header("x-ratelimit-limit").unwrap(), "3");
        assert_eq!(response.header("x-ratelimit-remaining").unwrap(), "2");
    }

    #[tokio::test]
    async fn after_callbacks_decide_what_counts() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        facades::RateLimiter::for_("not-found", |_| {
            Limit::per_minute(1).by("enumeration").after(|response| response.status_code() == 404)
        })
        .unwrap();
        let throttle = throttle_middleware(&params(&["not-found"]));

        for _ in 0..3 {
            assert_eq!(send(&throttle, Request::create("/", "GET")).await.status_code(), 200);
        }
        let missing: Destination = Arc::new(|_request| Box::pin(async { Response::make("Missing", 404) }));
        let response = run_middleware(Request::create("/", "GET"), vec![throttle.clone()], missing).await;
        assert_eq!(response.status_code(), 404);
        assert_eq!(send(&throttle, Request::create("/", "GET")).await.status_code(), 429);
    }

    #[tokio::test]
    async fn undefined_limiters_are_reported() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let (_app, _guard) = app();
        let throttle = ThrottleRequests::using("undefined");
        let error = throttle.handle(Request::create("/", "GET"), Next::new(ok())).await.unwrap_err();
        assert_eq!(error.to_string(), "Rate limiter [undefined] is not defined.");
        assert!(error.downcast_ref::<MissingRateLimiterException>().is_some());
    }
}
