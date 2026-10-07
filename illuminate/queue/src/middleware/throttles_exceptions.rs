//! The `ThrottlesExceptions` job middleware.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use illuminate_cache::facades::RateLimiter;
use illuminate_support::{Error, Result};

use super::{JobMiddleware, Next, md5};
use crate::context::{InteractsWithQueue, current_job};
use crate::exceptions::error_is;
use crate::job::ShouldQueue;

type ErrorPredicate = Arc<dyn Fn(&Error) -> bool + Send + Sync>;
type BackoffCallback = Arc<dyn Fn(&Error) -> u64 + Send + Sync>;

#[derive(Clone)]
enum Backoff {
    Minutes(u64),
    Using(BackoffCallback),
}

/// Throttle the exceptions a job throws: once it has thrown `max_attempts`
/// exceptions, further attempts are delayed until `decay_seconds` pass.
/// Perfect for jobs talking to unstable third-party services.
///
/// ```
/// use illuminate_queue::middleware::ThrottlesExceptions;
///
/// // After 10 exceptions, wait 5 minutes before trying again; retry
/// // other failures after 5 minutes too.
/// let middleware = ThrottlesExceptions::new(10, 5 * 60).backoff(5).by("weather-api");
/// # let _ = middleware;
/// ```
#[derive(Clone)]
pub struct ThrottlesExceptions {
    key: Option<String>,
    by_job: bool,
    max_attempts: i64,
    decay_seconds: u64,
    backoff: Backoff,
    report_callback: Option<ErrorPredicate>,
    when_callback: Option<ErrorPredicate>,
    delete_when: Vec<ErrorPredicate>,
    fail_when: Vec<ErrorPredicate>,
    prefix: String,
}

impl ThrottlesExceptions {
    /// Allow `max_attempts` exceptions, then wait `decay_seconds`.
    pub fn new(max_attempts: i64, decay_seconds: u64) -> Self {
        Self {
            key: None,
            by_job: false,
            max_attempts,
            decay_seconds,
            backoff: Backoff::Minutes(0),
            report_callback: None,
            when_callback: None,
            delete_when: Vec::new(),
            fail_when: Vec::new(),
            prefix: "laravel_throttles_exceptions:".to_string(),
        }
    }

    /// Only throttle exceptions passing the given test; others are thrown.
    pub fn when(mut self, callback: impl Fn(&Error) -> bool + Send + Sync + 'static) -> Self {
        self.when_callback = Some(Arc::new(callback));
        self
    }

    /// Delete the job when an exception passing the given test is thrown.
    pub fn delete_when(
        mut self,
        callback: impl Fn(&Error) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.delete_when.push(Arc::new(callback));
        self
    }

    /// Delete the job when an exception of type `E` is thrown.
    pub fn delete_when_error<E>(self) -> Self
    where
        E: fmt::Display + fmt::Debug + Send + Sync + 'static,
    {
        self.delete_when(error_is::<E>)
    }

    /// Fail the job when an exception passing the given test is thrown.
    pub fn fail_when(mut self, callback: impl Fn(&Error) -> bool + Send + Sync + 'static) -> Self {
        self.fail_when.push(Arc::new(callback));
        self
    }

    /// Fail the job when an exception of type `E` is thrown.
    pub fn fail_when_error<E>(self) -> Self
    where
        E: fmt::Display + fmt::Debug + Send + Sync + 'static,
    {
        self.fail_when(error_is::<E>)
    }

    /// Set the prefix of the rate limiter key.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// The number of *minutes* to delay the job after an exception that
    /// didn't hit the threshold yet.
    pub fn backoff(mut self, minutes: u64) -> Self {
        self.backoff = Backoff::Minutes(minutes);
        self
    }

    /// Compute the backoff (in minutes) from the exception.
    pub fn backoff_using(
        mut self,
        callback: impl Fn(&Error) -> u64 + Send + Sync + 'static,
    ) -> Self {
        self.backoff = Backoff::Using(Arc::new(callback));
        self
    }

    /// Share the throttle "bucket" with every job using the same key.
    pub fn by(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Throttle each queued job instance separately.
    pub fn by_job(mut self) -> Self {
        self.by_job = true;
        self
    }

    /// Report throttled exceptions to the exception handler.
    pub fn report(self) -> Self {
        self.report_when(|_| true)
    }

    /// Report throttled exceptions passing the given test.
    pub fn report_when(
        mut self,
        callback: impl Fn(&Error) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.report_callback = Some(Arc::new(callback));
        self
    }

    fn get_key(&self, job: &dyn ShouldQueue) -> String {
        if let Some(key) = &self.key {
            return format!("{}{}", self.prefix, key);
        }
        if self.by_job
            && let Some(uuid) = current_job().and_then(|job| job.uuid().map(String::from))
        {
            return format!("{}{}", self.prefix, uuid);
        }
        format!("{}{}", self.prefix, md5(job.command_name()))
    }

    fn backoff_seconds(&self, error: &Error) -> u64 {
        let minutes = match &self.backoff {
            Backoff::Minutes(minutes) => *minutes,
            Backoff::Using(callback) => callback(error),
        };
        minutes * 60
    }
}

impl fmt::Debug for ThrottlesExceptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThrottlesExceptions")
            .field("key", &self.key)
            .field("max_attempts", &self.max_attempts)
            .field("decay_seconds", &self.decay_seconds)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl JobMiddleware for ThrottlesExceptions {
    async fn handle(&self, job: &dyn ShouldQueue, next: Next<'_>) -> Result<()> {
        let limiter = RateLimiter::instance()?;
        let key = self.get_key(job);

        if limiter.too_many_attempts(&key, self.max_attempts).await? {
            let delay = limiter.available_in(&key).await? + 3;
            return job.release(delay as u64).await;
        }

        match next.run(job).await {
            Ok(()) => {
                limiter.clear(&key).await?;
                Ok(())
            }
            Err(error) => {
                if let Some(when) = &self.when_callback
                    && !when(&error)
                {
                    return Err(error);
                }

                if self
                    .report_callback
                    .as_ref()
                    .is_some_and(|report| report(&error))
                {
                    crate::report(&error);
                }

                if self.delete_when.iter().any(|callback| callback(&error)) {
                    return job.delete().await;
                }

                if self.fail_when.iter().any(|callback| callback(&error)) {
                    return job.fail(error).await;
                }

                limiter.hit(&key, self.decay_seconds).await?;
                job.release(self.backoff_seconds(&error)).await
            }
        }
    }
}
