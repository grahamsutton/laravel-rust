//! Run a callback that always takes at least a minimum amount of time —
//! useful to blunt timing attacks (Laravel uses it when checking credentials).
//!
//! ```
//! use std::time::Duration;
//! use illuminate_support::{Sleep, Timebox};
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! Sleep::fake();
//!
//! let result = Timebox::new()
//!     .call(|_timebox| async { "checked" }, Duration::from_millis(100))
//!     .await;
//!
//! assert_eq!(result, "checked");
//! Sleep::assert_slept_times(1);
//! # });
//! ```

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::sleep::Sleep;

/// A minimum-duration wrapper around a callback.
#[derive(Clone, Debug, Default)]
pub struct Timebox {
    early_return: Arc<AtomicBool>,
}

impl Timebox {
    /// Create a new timebox.
    pub fn new() -> Self {
        Self::default()
    }

    /// Invoke the callback, then sleep for whatever remains of `duration`
    /// (unless the callback asked to return early).
    pub async fn call<T, F, Fut>(&self, callback: F, duration: Duration) -> T
    where
        F: FnOnce(Timebox) -> Fut,
        Fut: Future<Output = T>,
    {
        let start = Instant::now();
        let result = callback(self.clone()).await;
        let remaining = duration.saturating_sub(start.elapsed());
        if !self.returns_early() && !remaining.is_zero() {
            Sleep::for_duration(remaining).await;
        }
        result
    }

    /// Indicate that the timebox can return early.
    pub fn return_early(&self) -> &Self {
        self.early_return.store(true, Ordering::SeqCst);
        self
    }

    /// Indicate that the timebox cannot return early.
    pub fn dont_return_early(&self) -> &Self {
        self.early_return.store(false, Ordering::SeqCst);
        self
    }

    /// Determine if the timebox will return early.
    pub fn returns_early(&self) -> bool {
        self.early_return.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn it_waits_for_the_remaining_time() {
        let started = Instant::now();
        let value = Timebox::new().call(|_| async { 42 }, Duration::from_millis(30)).await;
        assert_eq!(value, 42);
        assert!(started.elapsed() >= Duration::from_millis(30));
    }

    #[tokio::test]
    async fn it_can_return_early() {
        Sleep::fake();
        let value = Timebox::new()
            .call(
                |timebox| async move {
                    timebox.return_early();
                    "early"
                },
                Duration::from_secs(10),
            )
            .await;
        assert_eq!(value, "early");
        Sleep::assert_never_slept();

        let timebox = Timebox::new();
        timebox.return_early().dont_return_early();
        assert!(!timebox.returns_early());
        let result: Result<(), &str> = timebox.call(|_| async { Err("failed") }, Duration::from_secs(1)).await;
        assert!(result.is_err());
        Sleep::assert_slept_times(1);
        Sleep::stop_faking();
    }
}
