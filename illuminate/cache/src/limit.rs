//! Rate limit definitions: `Limit::per_minute(60)->by($request->ip())`.

use std::fmt;
use std::sync::Arc;

use illuminate_http::{HeaderMap, Request, Response};

/// Decides, from the response, whether a request counts toward the limit.
pub type AfterCallback = Arc<dyn Fn(&Response) -> bool + Send + Sync>;

/// Builds the response returned when the limit is exceeded, given the
/// request and the rate limit headers.
pub type ResponseCallback = Arc<dyn Fn(&Request, &HeaderMap) -> Response + Send + Sync>;

/// A rate limit: a number of attempts within a window of time.
///
/// ```
/// use illuminate_cache::Limit;
///
/// let limit = Limit::per_minute(60).by("127.0.0.1");
///
/// assert_eq!(limit.max_attempts, 60);
/// assert_eq!(limit.decay_seconds, 60);
/// assert_eq!(limit.key, "127.0.0.1");
///
/// assert_eq!(Limit::per_minutes(5, 10).decay_seconds, 300);
/// assert!(Limit::none().is_unlimited());
/// ```
#[derive(Clone)]
pub struct Limit {
    /// The rate limit signature key.
    pub key: String,
    /// The maximum number of attempts allowed within the given window.
    pub max_attempts: i64,
    /// The number of seconds until the rate limit is reset.
    pub decay_seconds: u64,
    /// Decides whether a response counts toward the limit.
    pub after_callback: Option<AfterCallback>,
    /// Builds the response returned when the limit is exceeded.
    pub response_callback: Option<ResponseCallback>,
    unlimited: bool,
}

impl fmt::Debug for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Limit")
            .field("key", &self.key)
            .field("max_attempts", &self.max_attempts)
            .field("decay_seconds", &self.decay_seconds)
            .field("unlimited", &self.unlimited)
            .finish()
    }
}

impl Limit {
    /// Create a new limit.
    pub fn new(key: impl Into<String>, max_attempts: i64, decay_seconds: u64) -> Self {
        Self {
            key: key.into(),
            max_attempts,
            decay_seconds,
            after_callback: None,
            response_callback: None,
            unlimited: false,
        }
    }

    /// A limit of attempts per second.
    pub fn per_second(max_attempts: i64) -> Self {
        Self::new("", max_attempts, 1)
    }

    /// A limit of attempts per the given number of seconds.
    pub fn per_seconds(decay_seconds: u64, max_attempts: i64) -> Self {
        Self::new("", max_attempts, decay_seconds)
    }

    /// A limit of attempts per minute.
    pub fn per_minute(max_attempts: i64) -> Self {
        Self::new("", max_attempts, 60)
    }

    /// A limit of attempts per the given number of minutes.
    pub fn per_minutes(decay_minutes: u64, max_attempts: i64) -> Self {
        Self::new("", max_attempts, 60 * decay_minutes)
    }

    /// A limit of attempts per hour.
    pub fn per_hour(max_attempts: i64) -> Self {
        Self::new("", max_attempts, 60 * 60)
    }

    /// A limit of attempts per the given number of hours.
    pub fn per_hours(decay_hours: u64, max_attempts: i64) -> Self {
        Self::new("", max_attempts, 60 * 60 * decay_hours)
    }

    /// A limit of attempts per day.
    pub fn per_day(max_attempts: i64) -> Self {
        Self::new("", max_attempts, 60 * 60 * 24)
    }

    /// A limit of attempts per the given number of days.
    pub fn per_days(decay_days: u64, max_attempts: i64) -> Self {
        Self::new("", max_attempts, 60 * 60 * 24 * decay_days)
    }

    /// No limit at all (Laravel's `Unlimited`).
    pub fn none() -> Self {
        let mut limit = Self::new("", i64::MAX, 60);
        limit.unlimited = true;
        limit
    }

    /// Determine if this is the "unlimited" limit.
    pub fn is_unlimited(&self) -> bool {
        self.unlimited
    }

    /// Segment the limit by the given key (an IP address, a user id, ...).
    pub fn by(mut self, key: impl fmt::Display) -> Self {
        self.key = key.to_string();
        self
    }

    /// Only count requests whose response passes the given check.
    pub fn after(mut self, callback: impl Fn(&Response) -> bool + Send + Sync + 'static) -> Self {
        self.after_callback = Some(Arc::new(callback));
        self
    }

    /// Use a custom response when the limit is exceeded.
    ///
    /// ```
    /// use illuminate_cache::Limit;
    /// use illuminate_http::Response;
    ///
    /// let limit = Limit::per_minute(1000).response(|_request, headers| {
    ///     let mut response = Response::make("Custom response...", 429);
    ///     response.headers_mut().extend(headers.clone());
    ///     response
    /// });
    /// assert!(limit.response_callback.is_some());
    /// ```
    pub fn response(
        mut self,
        callback: impl Fn(&Request, &HeaderMap) -> Response + Send + Sync + 'static,
    ) -> Self {
        self.response_callback = Some(Arc::new(callback));
        self
    }

    /// A unique key for the limit, used when several limits share a key.
    pub fn fallback_key(&self) -> String {
        let prefix = if self.key.is_empty() {
            String::new()
        } else {
            format!("{}:", self.key)
        };
        format!(
            "{prefix}attempts:{}:decay:{}",
            self.max_attempts, self.decay_seconds
        )
    }
}

/// What a named rate limiter returns: one or more limits, or a response to
/// send instead.
pub enum LimiterResponse {
    /// The limits to apply.
    Limits(Vec<Limit>),
    /// A response to return immediately.
    Response(Box<Response>),
}

impl From<Limit> for LimiterResponse {
    fn from(limit: Limit) -> Self {
        LimiterResponse::Limits(vec![limit])
    }
}

impl From<Vec<Limit>> for LimiterResponse {
    fn from(limits: Vec<Limit>) -> Self {
        LimiterResponse::Limits(limits)
    }
}

impl<const N: usize> From<[Limit; N]> for LimiterResponse {
    fn from(limits: [Limit; N]) -> Self {
        LimiterResponse::Limits(limits.into())
    }
}

impl From<Response> for LimiterResponse {
    fn from(response: Response) -> Self {
        LimiterResponse::Response(Box::new(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_built_fluently() {
        assert_eq!(Limit::per_second(5).decay_seconds, 1);
        assert_eq!(Limit::per_seconds(10, 5).decay_seconds, 10);
        assert_eq!(Limit::per_hour(5).decay_seconds, 3600);
        assert_eq!(Limit::per_hours(2, 5).decay_seconds, 7200);
        assert_eq!(Limit::per_day(5).decay_seconds, 86400);
        assert_eq!(Limit::per_days(2, 5).decay_seconds, 172_800);
        assert_eq!(Limit::per_minute(3).by(42).key, "42");
        assert_eq!(Limit::none().max_attempts, i64::MAX);
        assert!(!Limit::per_minute(1).is_unlimited());
    }

    #[test]
    fn fallback_keys_include_the_window() {
        assert_eq!(Limit::per_minute(3).fallback_key(), "attempts:3:decay:60");
        assert_eq!(
            Limit::per_day(10).by("user:1").fallback_key(),
            "user:1:attempts:10:decay:86400"
        );
    }

    #[test]
    fn limiter_responses_convert() {
        assert!(
            matches!(LimiterResponse::from(Limit::per_minute(1)), LimiterResponse::Limits(l) if l.len() == 1)
        );
        assert!(matches!(
            LimiterResponse::from([Limit::per_minute(1), Limit::per_day(5)]),
            LimiterResponse::Limits(l) if l.len() == 2
        ));
        assert!(matches!(
            LimiterResponse::from(Response::new("no")),
            LimiterResponse::Response(_)
        ));
        let limit = Limit::per_minute(1).after(|response| response.status_code() == 404);
        assert!((limit.after_callback.unwrap())(&Response::make("", 404)));
    }
}
