//! Interacting with time: `travel_to`, `travel(5).minutes()`, and
//! `freeze_time`.
//!
//! Time is frozen for the test's thread (tokio tests run on one), so tests
//! running in parallel never see each other's clocks. The clock is restored
//! when the [`TestApp`] is dropped.

use illuminate_support::Carbon;

use super::TestApp;

impl TestApp {
    /// Travel to the given moment and stay there.
    pub fn travel_to(&mut self, date: Carbon) -> &mut Self {
        Carbon::set_thread_test_now(Some(date));
        self.time_traveled = true;
        self
    }

    /// Travel forward (or, with a negative amount, back) in time:
    /// `app.travel(5).minutes()`.
    pub fn travel(&mut self, amount: i64) -> Travel<'_> {
        Travel { app: self, amount }
    }

    /// Freeze time at the current moment, returning it.
    pub fn freeze_time(&mut self) -> Carbon {
        let now = Carbon::now();
        self.travel_to(now);
        now
    }

    /// Freeze time at the start of the current second, returning it.
    pub fn freeze_second(&mut self) -> Carbon {
        let now = Carbon::now().start_of_second();
        self.travel_to(now);
        now
    }

    /// Return to the present.
    pub fn travel_back(&mut self) -> &mut Self {
        Carbon::set_thread_test_now(None);
        self.time_traveled = false;
        self
    }
}

/// A pending trip through time: `app.travel(5).minutes()`.
pub struct Travel<'a> {
    app: &'a mut TestApp,
    amount: i64,
}

impl Travel<'_> {
    fn to(self, date: Carbon) {
        self.app.travel_to(date);
    }

    /// Travel the given number of milliseconds.
    pub fn milliseconds(self) {
        let date = Carbon::now().add_milliseconds(self.amount);
        self.to(date);
    }

    /// Travel the given number of seconds.
    pub fn seconds(self) {
        let date = Carbon::now().add_seconds(self.amount);
        self.to(date);
    }

    /// Travel the given number of minutes.
    pub fn minutes(self) {
        let date = Carbon::now().add_minutes(self.amount);
        self.to(date);
    }

    /// Travel the given number of hours.
    pub fn hours(self) {
        let date = Carbon::now().add_hours(self.amount);
        self.to(date);
    }

    /// Travel the given number of days.
    pub fn days(self) {
        let date = Carbon::now().add_days(self.amount);
        self.to(date);
    }

    /// Travel the given number of weeks.
    pub fn weeks(self) {
        let date = Carbon::now().add_weeks(self.amount);
        self.to(date);
    }

    /// Travel the given number of months.
    pub fn months(self) {
        let date = Carbon::now().add_months(self.amount);
        self.to(date);
    }

    /// Travel the given number of years.
    pub fn years(self) {
        let date = Carbon::now().add_years(self.amount);
        self.to(date);
    }
}
