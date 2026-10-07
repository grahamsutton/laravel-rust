//! A fluent, fakeable `sleep`.
//!
//! ```
//! use illuminate_support::Sleep;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! Sleep::fake();
//!
//! Sleep::for_(1).second().await;
//! Sleep::for_(1.5).minutes().await;
//! Sleep::for_(1).second().and(10).milliseconds().await;
//!
//! Sleep::assert_slept_times(3);
//! Sleep::assert_sequence(vec![
//!     Sleep::for_(1).second(),
//!     Sleep::for_(90).seconds(),
//!     Sleep::for_(1010).milliseconds(),
//! ]);
//! # });
//! ```
//!
//! A `Sleep` does nothing until it is awaited (or [`Sleep::blocking`] is
//! called). Faking is per-thread, which keeps parallel tests independent;
//! use a current-thread runtime (the `#[tokio::test]` default) in tests.

use std::cell::RefCell;
use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::time::Duration;

use crate::carbon::{Carbon, CarbonInterval};

type FakeCallback = Box<dyn FnMut(Duration)>;

#[derive(Default)]
struct FakeState {
    sequence: Vec<Duration>,
    callbacks: Vec<FakeCallback>,
    sync_with_carbon: bool,
}

thread_local! {
    static FAKE: RefCell<Option<FakeState>> = const { RefCell::new(None) };
}

type WhileCallback = Box<dyn FnMut() -> bool + Send>;

/// A pending pause in execution.
pub struct Sleep {
    duration: Duration,
    pending: Option<f64>,
    should_sleep: bool,
    while_: Option<WhileCallback>,
}

impl Sleep {
    /// Begin a sleep of the given amount; follow it with a unit
    /// (`Sleep::for_(2).seconds()`). Unit-less sleeps are treated as seconds.
    pub fn for_(amount: impl Into<f64>) -> Self {
        Self {
            duration: Duration::ZERO,
            pending: Some(amount.into()),
            should_sleep: true,
            while_: None,
        }
    }

    /// Sleep for the given duration.
    pub fn for_duration(duration: impl Into<CarbonInterval>) -> Self {
        Self {
            duration: duration.into().to_std(),
            pending: None,
            should_sleep: true,
            while_: None,
        }
    }

    /// Sleep until the given moment.
    pub fn until(moment: Carbon) -> Self {
        let millis = moment.timestamp_millis() - Carbon::now().timestamp_millis();
        Self::for_duration(Duration::from_millis(millis.max(0) as u64))
    }

    /// Sleep for the given number of seconds (PHP's `sleep`).
    #[allow(clippy::self_named_constructors)]
    pub fn sleep(seconds: u64) -> Self {
        Self::for_duration(Duration::from_secs(seconds))
    }

    /// Sleep for the given number of microseconds (PHP's `usleep`).
    pub fn usleep(microseconds: u64) -> Self {
        Self::for_duration(Duration::from_micros(microseconds))
    }

    fn add_pending(mut self, unit_micros: f64) -> Self {
        let amount = self.pending.take().unwrap_or(0.0).max(0.0);
        self.duration += Duration::from_micros((amount * unit_micros).round() as u64);
        self
    }

    /// Interpret the pending amount as minutes.
    pub fn minutes(self) -> Self {
        self.add_pending(60_000_000.0)
    }

    /// Alias of `minutes`.
    pub fn minute(self) -> Self {
        self.minutes()
    }

    /// Interpret the pending amount as seconds.
    pub fn seconds(self) -> Self {
        self.add_pending(1_000_000.0)
    }

    /// Alias of `seconds`.
    pub fn second(self) -> Self {
        self.seconds()
    }

    /// Interpret the pending amount as milliseconds.
    pub fn milliseconds(self) -> Self {
        self.add_pending(1_000.0)
    }

    /// Alias of `milliseconds`.
    pub fn millisecond(self) -> Self {
        self.milliseconds()
    }

    /// Interpret the pending amount as microseconds.
    pub fn microseconds(self) -> Self {
        self.add_pending(1.0)
    }

    /// Alias of `microseconds`.
    pub fn microsecond(self) -> Self {
        self.microseconds()
    }

    /// Add another amount to the sleep (`Sleep::for_(1).second().and(10).milliseconds()`).
    pub fn and(mut self, amount: impl Into<f64>) -> Self {
        self.pending = Some(amount.into());
        self
    }

    /// Keep sleeping while the callback returns true.
    pub fn while_(mut self, callback: impl FnMut() -> bool + Send + 'static) -> Self {
        self.while_ = Some(Box::new(callback));
        self
    }

    /// Only sleep when the condition is true.
    pub fn when(mut self, condition: bool) -> Self {
        self.should_sleep = condition;
        self
    }

    /// Only sleep when the condition is false.
    pub fn unless(self, condition: bool) -> Self {
        self.when(!condition)
    }

    /// The total duration of the sleep.
    pub fn duration(&self) -> Duration {
        let pending = self.pending.map(|s| Duration::from_secs_f64(s.max(0.0))).unwrap_or_default();
        self.duration + pending
    }

    /// Sleep, then return the value of the callback.
    pub async fn then<R>(self, callback: impl FnOnce() -> R) -> R {
        self.await;
        callback()
    }

    /// Sleep by blocking the current thread (respects faking).
    pub fn blocking(mut self) {
        if !self.should_sleep {
            return;
        }
        let duration = self.duration();
        if record_fake(duration) {
            return;
        }
        match self.while_.as_mut() {
            Some(condition) => {
                while condition() {
                    std::thread::sleep(duration);
                }
            }
            None => std::thread::sleep(duration),
        }
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Fake sleeping on this thread: sleeps are recorded instead of performed.
    pub fn fake() {
        FAKE.with(|f| *f.borrow_mut() = Some(FakeState::default()));
    }

    /// Fake sleeping and advance a (thread-local) frozen "now" by each sleep.
    pub fn fake_with_carbon() {
        Self::fake();
        Self::sync_with_carbon(true);
    }

    /// Stop faking sleeps on this thread.
    pub fn stop_faking() {
        FAKE.with(|f| *f.borrow_mut() = None);
    }

    /// Determine if sleeping is being faked on this thread.
    pub fn is_fake() -> bool {
        FAKE.with(|f| f.borrow().is_some())
    }

    /// Advance the thread's frozen "now" whenever a fake sleep happens.
    pub fn sync_with_carbon(value: bool) {
        FAKE.with(|f| {
            if let Some(state) = f.borrow_mut().as_mut() {
                state.sync_with_carbon = value;
            }
        });
        if value && Carbon::thread_test_now().is_none() {
            Carbon::set_thread_test_now(Some(Carbon::now()));
        }
    }

    /// Register a callback to run whenever a fake sleep happens.
    pub fn when_faking_sleep(callback: impl FnMut(Duration) + 'static) {
        FAKE.with(|f| {
            if let Some(state) = f.borrow_mut().as_mut() {
                state.callbacks.push(Box::new(callback));
            }
        });
    }

    /// The durations slept while faking, in order.
    pub fn slept() -> Vec<Duration> {
        FAKE.with(|f| f.borrow().as_ref().map(|s| s.sequence.clone()).unwrap_or_default())
    }

    /// Assert that a sleep matching the callback happened `times` times.
    #[track_caller]
    pub fn assert_slept(mut expected: impl FnMut(Duration) -> bool, times: usize) {
        let count = Self::slept().into_iter().filter(|d| expected(*d)).count();
        assert_eq!(count, times, "The expected sleep was found [{count}] times instead of [{times}].");
    }

    /// Assert that sleep happened the given number of times.
    #[track_caller]
    pub fn assert_slept_times(expected: usize) {
        let count = Self::slept().len();
        assert_eq!(count, expected, "Expected [{expected}] sleeps but found [{count}].");
    }

    /// Assert the exact sequence of sleeps.
    #[track_caller]
    pub fn assert_sequence(sequence: Vec<Sleep>) {
        Self::assert_slept_times(sequence.len());
        for (expected, actual) in sequence.iter().zip(Self::slept()) {
            assert_eq!(
                expected.duration(),
                actual,
                "Expected sleep duration of [{:?}] but actually slept for [{actual:?}].",
                expected.duration()
            );
        }
    }

    /// Assert that sleep was never called.
    #[track_caller]
    pub fn assert_never_slept() {
        Self::assert_slept_times(0);
    }

    /// Assert that no sleep actually paused execution (every duration was zero).
    #[track_caller]
    pub fn assert_insomniac() {
        for duration in Self::slept() {
            assert!(duration.is_zero(), "Unexpected sleep duration of [{duration:?}] found.");
        }
    }
}

/// Record a fake sleep if faking is enabled on this thread.
fn record_fake(duration: Duration) -> bool {
    let synced = FAKE.with(|f| {
        let mut state = f.borrow_mut();
        let state = state.as_mut()?;
        state.sequence.push(duration);
        for callback in state.callbacks.iter_mut() {
            callback(duration);
        }
        Some(state.sync_with_carbon)
    });
    match synced {
        Some(true) => {
            let now = Carbon::now().add(CarbonInterval::from(duration));
            Carbon::set_thread_test_now(Some(now));
            true
        }
        Some(false) => true,
        None => false,
    }
}

impl IntoFuture for Sleep {
    type Output = ();
    type IntoFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        let duration = self.duration();
        if !self.should_sleep || record_fake(duration) {
            return Box::pin(std::future::ready(()));
        }
        let mut condition = self.while_;
        Box::pin(async move {
            match condition.as_mut() {
                Some(condition) => {
                    while condition() {
                        tokio::time::sleep(duration).await;
                    }
                }
                None => tokio::time::sleep(duration).await,
            }
        })
    }
}

impl fmt::Debug for Sleep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sleep")
            .field("duration", &self.duration())
            .field("should_sleep", &self.should_sleep)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn it_records_fake_sleeps() {
        Sleep::fake();
        Sleep::assert_never_slept();
        Sleep::for_(1).second().await;
        Sleep::for_(2).seconds().await;
        Sleep::for_(500).milliseconds().await;
        Sleep::for_(5000).microseconds().await;
        Sleep::usleep(10).await;
        Sleep::sleep(3).await;
        Sleep::assert_slept_times(6);
        Sleep::assert_slept(|d| d == Duration::from_secs(2), 1);
        Sleep::assert_sequence(vec![
            Sleep::for_(1).second(),
            Sleep::for_(2).seconds(),
            Sleep::for_(500).milliseconds(),
            Sleep::for_(5).milliseconds(),
            Sleep::usleep(10),
            Sleep::sleep(3),
        ]);
        Sleep::stop_faking();
    }

    #[tokio::test]
    async fn it_supports_conditions_and_values() {
        Sleep::fake();
        Sleep::for_(1).second().when(false).await;
        Sleep::for_(1).second().unless(true).await;
        Sleep::assert_never_slept();
        assert_eq!(Sleep::for_(1).second().then(|| 1 + 1).await, 2);
        Sleep::for_(0).seconds().await;
        Sleep::assert_slept_times(2);
        Sleep::fake();
        Sleep::for_(0).seconds().await;
        Sleep::assert_insomniac();
        Sleep::stop_faking();
    }

    #[tokio::test]
    async fn it_syncs_with_carbon() {
        let start = Carbon::parse("2024-01-01 00:00:00").unwrap();
        Carbon::set_thread_test_now(Some(start));
        Sleep::fake_with_carbon();
        Sleep::for_(90).seconds().await;
        assert_eq!(Carbon::now(), start.add_seconds(90));
        assert_eq!(start.diff_for_humans(), "1 minute ago");
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        Sleep::when_faking_sleep(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        Sleep::for_(1).minute().blocking();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Sleep::stop_faking();
        Carbon::set_thread_test_now(None);
    }

    #[tokio::test]
    async fn it_really_sleeps() {
        let started = std::time::Instant::now();
        Sleep::for_(20).milliseconds().await;
        assert!(started.elapsed() >= Duration::from_millis(20));
        let remaining = Arc::new(AtomicUsize::new(2));
        let counter = remaining.clone();
        Sleep::for_(1)
            .millisecond()
            .while_(move || counter.fetch_sub(1, Ordering::SeqCst) > 0)
            .await;
        assert_eq!(remaining.load(Ordering::SeqCst), usize::MAX);
        assert_eq!(Sleep::for_(1.5).minutes().duration(), Duration::from_secs(90));
        assert_eq!(Sleep::for_(1).second().and(10).milliseconds().duration(), Duration::from_millis(1010));
        let until = Sleep::until(Carbon::now().add_seconds(2)).duration();
        assert!(until > Duration::from_millis(1500) && until <= Duration::from_secs(2));
    }
}
