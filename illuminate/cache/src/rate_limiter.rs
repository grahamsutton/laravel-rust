//! The rate limiter.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, RwLock};

use illuminate_http::Request;
use illuminate_support::{Carbon, Result, ValueExt};

use crate::limit::LimiterResponse;
use crate::repository::Repository;

/// A named rate limiter callback.
pub type LimiterCallback = Arc<dyn Fn(&Request) -> LimiterResponse + Send + Sync>;

/// Limits how often an action may be performed within a window of time,
/// keeping its counters in the cache.
///
/// ```
/// use illuminate_cache::{ArrayStore, RateLimiter, Repository};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let limiter = RateLimiter::new(Repository::new(ArrayStore::new()));
///
/// for _ in 0..5 {
///     let sent = limiter.attempt("send-message:1", 5, || async { "Sent!" }, 60).await.unwrap();
///     assert_eq!(sent, Some("Sent!"));
/// }
///
/// assert_eq!(limiter.attempt("send-message:1", 5, || async { "Sent!" }, 60).await.unwrap(), None);
/// assert!(limiter.too_many_attempts("send-message:1", 5).await.unwrap());
/// assert_eq!(limiter.available_in("send-message:1").await.unwrap(), 60);
/// # });
/// ```
pub struct RateLimiter {
    cache: Repository,
    limiters: RwLock<HashMap<String, LimiterCallback>>,
}

impl RateLimiter {
    /// Create a new rate limiter using the given cache.
    pub fn new(cache: Repository) -> Self {
        Self {
            cache,
            limiters: RwLock::new(HashMap::new()),
        }
    }

    /// The cache the limiter keeps its counters in.
    pub fn cache(&self) -> &Repository {
        &self.cache
    }

    /// Register a named limiter configuration.
    ///
    /// ```
    /// use illuminate_cache::{ArrayStore, Limit, RateLimiter, Repository};
    ///
    /// let limiter = RateLimiter::new(Repository::new(ArrayStore::new()));
    ///
    /// limiter.for_("api", |request| {
    ///     Limit::per_minute(60).by(request.ip().unwrap_or_default())
    /// });
    ///
    /// assert!(limiter.limiter("api").is_some());
    /// ```
    pub fn for_<F, R>(&self, name: &str, callback: F) -> &Self
    where
        F: Fn(&Request) -> R + Send + Sync + 'static,
        R: Into<LimiterResponse>,
    {
        let callback: LimiterCallback = Arc::new(move |request| callback(request).into());
        self.limiters
            .write()
            .unwrap()
            .insert(name.to_string(), callback);
        self
    }

    /// Get the given named rate limiter.
    ///
    /// When a limiter returns several limits sharing a key, each one gets a
    /// unique fallback key so they don't count against each other.
    pub fn limiter(&self, name: &str) -> Option<LimiterCallback> {
        let limiter = self.limiters.read().unwrap().get(name).cloned()?;
        Some(Arc::new(move |request: &Request| match limiter(request) {
            LimiterResponse::Limits(mut limits) => {
                let mut seen = HashSet::new();
                let duplicates: HashSet<String> = limits
                    .iter()
                    .filter(|limit| !seen.insert(limit.key.clone()))
                    .map(|l| l.key.clone())
                    .collect();
                for limit in &mut limits {
                    if duplicates.contains(&limit.key) {
                        limit.key = limit.fallback_key();
                    }
                }
                LimiterResponse::Limits(limits)
            }
            response => response,
        }))
    }

    /// Attempt to execute a callback if it's not limited. Returns `None`
    /// when there are no attempts left.
    pub async fn attempt<T, F, Fut>(
        &self,
        key: &str,
        max_attempts: i64,
        callback: F,
        decay_seconds: u64,
    ) -> Result<Option<T>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        if self.too_many_attempts(key, max_attempts).await? {
            return Ok(None);
        }
        let result = callback().await;
        self.hit(key, decay_seconds).await?;
        Ok(Some(result))
    }

    /// Determine if the given key has been "accessed" too many times.
    pub async fn too_many_attempts(&self, key: &str, max_attempts: i64) -> Result<bool> {
        if self.attempts(key).await? >= max_attempts {
            if self
                .cache
                .has(&format!("{}:timer", Self::clean_rate_limiter_key(key)))
                .await?
            {
                return Ok(true);
            }
            self.reset_attempts(key).await?;
        }
        Ok(false)
    }

    /// Increment the counter for a given key for a given decay time.
    pub async fn hit(&self, key: &str, decay_seconds: u64) -> Result<i64> {
        self.increment(key, decay_seconds, 1).await
    }

    /// Increment the counter for a given key by the given amount.
    pub async fn increment(&self, key: &str, decay_seconds: u64, amount: i64) -> Result<i64> {
        let key = Self::clean_rate_limiter_key(key);

        self.cache
            .add(
                &format!("{key}:timer"),
                Self::available_at(decay_seconds),
                decay_seconds,
            )
            .await?;
        let added = self.cache.add(&key, 0, decay_seconds).await?;
        let hits = self.cache.increment_by(&key, amount).await?;

        if !added && hits == amount {
            self.cache.put(&key, amount, decay_seconds).await?;
        }

        Ok(hits)
    }

    /// Decrement the counter for a given key by the given amount.
    pub async fn decrement(&self, key: &str, decay_seconds: u64, amount: i64) -> Result<i64> {
        self.increment(key, decay_seconds, -amount).await
    }

    /// Get the number of attempts for the given key.
    pub async fn attempts(&self, key: &str) -> Result<i64> {
        let key = Self::clean_rate_limiter_key(key);
        Ok(self
            .cache
            .get(&key)
            .await?
            .and_then(|v| v.to_i64_lossy())
            .unwrap_or(0))
    }

    /// Reset the number of attempts for the given key.
    pub async fn reset_attempts(&self, key: &str) -> Result<bool> {
        self.cache.forget(&Self::clean_rate_limiter_key(key)).await
    }

    /// Get the number of retries left for the given key.
    pub async fn remaining(&self, key: &str, max_attempts: i64) -> Result<i64> {
        let key = Self::clean_rate_limiter_key(key);
        let attempts = self.attempts(&key).await?;
        Ok((max_attempts - attempts).max(0))
    }

    /// Alias of [`RateLimiter::remaining`].
    pub async fn retries_left(&self, key: &str, max_attempts: i64) -> Result<i64> {
        self.remaining(key, max_attempts).await
    }

    /// Clear the hits and lockout timer for the given key.
    pub async fn clear(&self, key: &str) -> Result<()> {
        let key = Self::clean_rate_limiter_key(key);
        self.reset_attempts(&key).await?;
        self.cache.forget(&format!("{key}:timer")).await?;
        Ok(())
    }

    /// Get the number of seconds until the key is accessible again.
    pub async fn available_in(&self, key: &str) -> Result<i64> {
        let key = Self::clean_rate_limiter_key(key);
        let timer = self
            .cache
            .get(&format!("{key}:timer"))
            .await?
            .and_then(|v| v.to_i64_lossy())
            .unwrap_or(0);
        Ok((timer - Carbon::now().timestamp()).max(0))
    }

    /// The UNIX timestamp the given number of seconds from now.
    fn available_at(seconds: u64) -> i64 {
        Carbon::now().timestamp() + seconds as i64
    }

    /// Clean the rate limiter key from unicode characters, the way Laravel
    /// does: `htmlentities()` the key and keep the first letter of each
    /// named entity (`é` becomes `e`, `&` becomes `a`).
    ///
    /// ```
    /// use illuminate_cache::RateLimiter;
    ///
    /// assert_eq!(RateLimiter::clean_rate_limiter_key("jérôme@example.com"), "jerome@example.com");
    /// assert_eq!(RateLimiter::clean_rate_limiter_key("a&b"), "aab");
    /// ```
    pub fn clean_rate_limiter_key(key: &str) -> String {
        let mut cleaned = String::with_capacity(key.len());
        for c in key.chars() {
            match html_entity(c) {
                Some(entity) => match entity.chars().next() {
                    Some(first) if entity.chars().all(|c| c.is_ascii_alphabetic()) => {
                        cleaned.push(first)
                    }
                    _ => {
                        cleaned.push('&');
                        cleaned.push_str(entity);
                        cleaned.push(';');
                    }
                },
                None if c == '\'' => cleaned.push_str("&#039;"),
                None => cleaned.push(c),
            }
        }
        cleaned
    }
}

/// The HTML 4 named entity for a character, as `htmlentities()` encodes it.
fn html_entity(c: char) -> Option<&'static str> {
    const LATIN1: [&str; 96] = [
        "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf",
        "laquo", "not", "shy", "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro",
        "para", "middot", "cedil", "sup1", "ordm", "raquo", "frac14", "frac12", "frac34", "iquest",
        "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig", "Ccedil", "Egrave",
        "Eacute", "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve",
        "Oacute", "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc",
        "Uuml", "Yacute", "THORN", "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring",
        "aelig", "ccedil", "egrave", "eacute", "ecirc", "euml", "igrave", "iacute", "icirc",
        "iuml", "eth", "ntilde", "ograve", "oacute", "ocirc", "otilde", "ouml", "divide", "oslash",
        "ugrave", "uacute", "ucirc", "uuml", "yacute", "thorn", "yuml",
    ];
    match c {
        '&' => Some("amp"),
        '<' => Some("lt"),
        '>' => Some("gt"),
        '"' => Some("quot"),
        '\u{a0}'..='\u{ff}' => Some(LATIN1[c as usize - 0xa0]),
        'Œ' => Some("OElig"),
        'œ' => Some("oelig"),
        'Š' => Some("Scaron"),
        'š' => Some("scaron"),
        'Ÿ' => Some("Yuml"),
        'ƒ' => Some("fnof"),
        '–' => Some("ndash"),
        '—' => Some("mdash"),
        '‘' => Some("lsquo"),
        '’' => Some("rsquo"),
        '“' => Some("ldquo"),
        '”' => Some("rdquo"),
        '•' => Some("bull"),
        '…' => Some("hellip"),
        '€' => Some("euro"),
        '™' => Some("trade"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array_store::ArrayStore;
    use crate::limit::Limit;
    use crate::testing::freeze_time;

    fn limiter() -> RateLimiter {
        RateLimiter::new(Repository::new(ArrayStore::new()))
    }

    #[tokio::test]
    async fn hits_count_attempts_within_the_decay_window() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let limiter = limiter();

        assert_eq!(limiter.hit("key", 60).await.unwrap(), 1);
        assert_eq!(limiter.hit("key", 60).await.unwrap(), 2);
        assert_eq!(limiter.increment("key", 60, 5).await.unwrap(), 7);
        assert_eq!(limiter.decrement("key", 60, 2).await.unwrap(), 5);
        assert_eq!(limiter.attempts("key").await.unwrap(), 5);
        assert_eq!(limiter.remaining("key", 10).await.unwrap(), 5);
        assert_eq!(limiter.retries_left("key", 3).await.unwrap(), 0);
        assert!(limiter.too_many_attempts("key", 5).await.unwrap());
        assert!(!limiter.too_many_attempts("key", 6).await.unwrap());

        time.travel_seconds(30);
        assert_eq!(limiter.available_in("key").await.unwrap(), 30);

        // Once the window passes, the counter starts over.
        time.travel_seconds(30);
        assert!(!limiter.too_many_attempts("key", 5).await.unwrap());
        assert_eq!(limiter.attempts("key").await.unwrap(), 0);
        assert_eq!(limiter.hit("key", 60).await.unwrap(), 1);
        assert_eq!(limiter.available_in("key").await.unwrap(), 60);
    }

    #[tokio::test]
    async fn attempts_run_the_callback_until_limited() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let limiter = limiter();
        for attempt in 1..=3 {
            assert_eq!(
                limiter
                    .attempt("send", 3, || async move { attempt }, 60)
                    .await
                    .unwrap(),
                Some(attempt)
            );
        }
        assert_eq!(
            limiter
                .attempt("send", 3, || async { 4 }, 60)
                .await
                .unwrap(),
            None
        );
        assert_eq!(limiter.attempts("send").await.unwrap(), 3);
    }

    #[tokio::test]
    async fn attempts_and_timers_can_be_cleared() {
        let _time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let limiter = limiter();
        limiter.hit("key", 60).await.unwrap();
        assert!(limiter.reset_attempts("key").await.unwrap());
        assert_eq!(limiter.attempts("key").await.unwrap(), 0);
        assert_eq!(limiter.available_in("key").await.unwrap(), 60);

        limiter.hit("key", 60).await.unwrap();
        limiter.clear("key").await.unwrap();
        assert_eq!(limiter.attempts("key").await.unwrap(), 0);
        assert_eq!(limiter.available_in("key").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn an_expired_counter_with_a_live_timer_restarts() {
        let time = freeze_time(Carbon::from_timestamp(1_700_000_000));
        let limiter = limiter();
        limiter.hit("key", 10).await.unwrap();
        // The counter disappears but the timer survives (e.g. it was evicted).
        limiter.cache().forget("key").await.unwrap();
        time.travel_seconds(1);
        assert_eq!(limiter.hit("key", 10).await.unwrap(), 1);
        assert_eq!(limiter.attempts("key").await.unwrap(), 1);
    }

    #[test]
    fn named_limiters_get_unique_keys() {
        let limiter = limiter();
        limiter.for_("uploads", |_| {
            vec![
                Limit::per_minute(10).by("user:1"),
                Limit::per_day(1000).by("user:1"),
            ]
        });
        let callback = limiter.limiter("uploads").unwrap();
        let LimiterResponse::Limits(limits) = callback(&Request::default()) else {
            panic!("expected limits")
        };
        assert_eq!(limits[0].key, "user:1:attempts:10:decay:60");
        assert_eq!(limits[1].key, "user:1:attempts:1000:decay:86400");
        assert!(limiter.limiter("missing").is_none());
    }

    #[test]
    fn keys_are_cleaned_like_htmlentities() {
        assert_eq!(RateLimiter::clean_rate_limiter_key("Ünïcödé"), "Unicode");
        assert_eq!(RateLimiter::clean_rate_limiter_key("<tag>"), "ltagg");
        assert_eq!(RateLimiter::clean_rate_limiter_key("x²"), "x&sup2;");
        assert_eq!(RateLimiter::clean_rate_limiter_key("it's"), "it&#039;s");
        assert_eq!(
            RateLimiter::clean_rate_limiter_key("192.168.0.1|login"),
            "192.168.0.1|login"
        );
    }
}
