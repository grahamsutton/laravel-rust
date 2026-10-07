//! Scheduled events: what to run, and when.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use futures::future::BoxFuture;
use illuminate_support::{Carbon, Error, Result};
use sha1::{Digest, Sha1};

use super::cron::CronExpression;
use super::mutex::{EventMutex, InMemoryEventMutex};
use super::schedule::Schedule;
use crate::application::Application;
use crate::output::Output;

pub(crate) type CallbackFn = Arc<dyn Fn() -> BoxFuture<'static, Result<i32>> + Send + Sync>;
type HookFn = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;
/// An "after" hook: receives the task's output.
type AfterFn = Arc<dyn Fn(Arc<str>) -> BoxFuture<'static, ()> + Send + Sync>;
type FilterFn = Arc<dyn Fn() -> bool + Send + Sync>;

/// Values a scheduled closure may return: `()`, a `bool` (`false` fails
/// the task), an exit code, or a `Result`.
pub trait IntoExitCode {
    /// Convert the value into an exit code (or an error).
    fn into_exit_code(self) -> Result<i32>;
}

impl IntoExitCode for () {
    fn into_exit_code(self) -> Result<i32> {
        Ok(0)
    }
}

impl IntoExitCode for bool {
    fn into_exit_code(self) -> Result<i32> {
        Ok(if self { 0 } else { 1 })
    }
}

impl IntoExitCode for i32 {
    fn into_exit_code(self) -> Result<i32> {
        Ok(self)
    }
}

impl<E: Into<Error>> IntoExitCode for std::result::Result<(), E> {
    fn into_exit_code(self) -> Result<i32> {
        self.map(|_| 0).map_err(Into::into)
    }
}

#[derive(Clone)]
pub(crate) enum Task {
    Command(String),
    Exec(String),
    Callback {
        callback: CallbackFn,
        location: Option<String>,
    },
}

/// Report an error through the console application's exception reporter.
fn report(error: &Error) {
    crate::facades::Artisan::application().report(error);
}

/// Sends a scheduled task's output by email (`email_output_to`). The
/// foundation binds one that uses the application's mailer.
#[async_trait::async_trait]
pub trait ScheduleOutputMailer: Send + Sync + 'static {
    /// Send the output to the addresses with the given subject.
    async fn send(&self, addresses: &[String], subject: &str, output: &str) -> Result<()>;
}

#[derive(Clone)]
enum Filter {
    When(FilterFn),
    Skip(FilterFn),
    Between {
        start: String,
        end: String,
        inside: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Condition {
    Always,
    Success,
    Failure,
}

struct State {
    task: Task,
    expression: String,
    timezone: Option<String>,
    environments: Vec<String>,
    even_in_maintenance_mode: bool,
    without_overlapping: bool,
    expires_at: u64,
    on_one_server: bool,
    run_in_background: bool,
    filters: Vec<Filter>,
    description: Option<String>,
    before: Vec<HookFn>,
    after: Vec<(Condition, AfterFn)>,
    repeat_seconds: Option<u32>,
    last_checked: Option<Carbon>,
    even_when_paused: bool,
    last_output: Arc<str>,
    exit_code: Option<i32>,
    skipped_because_overlapping: bool,
    output_path: Option<(PathBuf, bool)>,
    mutex: Arc<dyn EventMutex>,
    mutex_name: Option<String>,
}

/// A scheduled event. `Event` is a cheap, cloneable handle: every method
/// updates the event registered with the schedule.
///
/// ```
/// use illuminate_console::scheduling::Schedule;
///
/// let schedule = Schedule::new();
///
/// let event = schedule.command("emails:send --force").weekly_on(1, "8:00");
///
/// assert_eq!(event.expression(), "0 8 * * 1");
/// ```
#[derive(Clone)]
pub struct Event {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Event")
            .field("expression", &self.expression())
            .field("summary", &self.summary_for_display())
            .finish()
    }
}

fn parse_time(time: &str) -> (String, String) {
    let segments: Vec<&str> = time.split(':').collect();
    let hours = segments[0].trim().parse::<u32>().unwrap_or(0).to_string();
    let minutes = if segments.len() >= 2 {
        segments[1].trim().parse::<u32>().unwrap_or(0).to_string()
    } else {
        "0".to_string()
    };
    (minutes, hours)
}

fn join<I, T>(values: I) -> String
where
    I: IntoIterator<Item = T>,
    T: ToString,
{
    values
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn sha1(value: &str) -> String {
    hex::encode(Sha1::digest(value.as_bytes()))
}

impl Event {
    pub(crate) fn new(task: Task, timezone: Option<String>, mutex: Arc<dyn EventMutex>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                task,
                expression: "* * * * *".to_string(),
                timezone,
                environments: Vec::new(),
                even_in_maintenance_mode: false,
                without_overlapping: false,
                expires_at: 1440,
                on_one_server: false,
                run_in_background: false,
                filters: Vec::new(),
                description: None,
                before: Vec::new(),
                after: Vec::new(),
                repeat_seconds: None,
                last_checked: None,
                even_when_paused: false,
                last_output: Arc::from(""),
                exit_code: None,
                skipped_because_overlapping: false,
                output_path: None,
                mutex,
                mutex_name: None,
            })),
        }
    }

    /// Create a standalone event running an Artisan command (not registered
    /// with any schedule).
    pub fn command(command: impl Into<String>) -> Self {
        Self::new(
            Task::Command(command.into()),
            None,
            Arc::new(InMemoryEventMutex::new()),
        )
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn update(self, callback: impl FnOnce(&mut State)) -> Self {
        callback(&mut self.state());
        self
    }

    // ------------------------------------------------------------------
    // Frequencies
    // ------------------------------------------------------------------

    /// The cron expression representing the event's frequency.
    pub fn cron(self, expression: impl Into<String>) -> Self {
        let expression = expression.into();
        self.update(|state| state.expression = expression)
    }

    fn splice_into_position(self, position: usize, value: impl ToString) -> Self {
        let value = value.to_string();
        self.update(|state| {
            let mut segments: Vec<String> = state
                .expression
                .split_whitespace()
                .map(String::from)
                .collect();
            while segments.len() < 5 {
                segments.push("*".to_string());
            }
            segments[position - 1] = value;
            state.expression = segments.join(" ");
        })
    }

    fn hour_based_schedule(self, minutes: impl ToString, hours: impl ToString) -> Self {
        self.splice_into_position(1, minutes)
            .splice_into_position(2, hours)
    }

    fn repeat_every(self, seconds: u32) -> Self {
        assert!(
            seconds > 0 && 60 % seconds == 0,
            "The seconds [{seconds}] are not evenly divisible by 60."
        );
        self.update(|state| state.repeat_seconds = Some(seconds))
            .every_minute()
    }

    /// Run the event every second (`schedule:run` keeps running it until
    /// the end of the minute).
    pub fn every_second(self) -> Self {
        self.repeat_every(1)
    }

    /// Run the event every two seconds.
    pub fn every_two_seconds(self) -> Self {
        self.repeat_every(2)
    }

    /// Run the event every five seconds.
    pub fn every_five_seconds(self) -> Self {
        self.repeat_every(5)
    }

    /// Run the event every ten seconds.
    pub fn every_ten_seconds(self) -> Self {
        self.repeat_every(10)
    }

    /// Run the event every fifteen seconds.
    pub fn every_fifteen_seconds(self) -> Self {
        self.repeat_every(15)
    }

    /// Run the event every twenty seconds.
    pub fn every_twenty_seconds(self) -> Self {
        self.repeat_every(20)
    }

    /// Run the event every thirty seconds.
    pub fn every_thirty_seconds(self) -> Self {
        self.repeat_every(30)
    }

    /// Schedule the event to run every minute.
    pub fn every_minute(self) -> Self {
        self.splice_into_position(1, "*")
    }

    /// Schedule the event to run every two minutes.
    pub fn every_two_minutes(self) -> Self {
        self.splice_into_position(1, "*/2")
    }

    /// Schedule the event to run every three minutes.
    pub fn every_three_minutes(self) -> Self {
        self.splice_into_position(1, "*/3")
    }

    /// Schedule the event to run every four minutes.
    pub fn every_four_minutes(self) -> Self {
        self.splice_into_position(1, "*/4")
    }

    /// Schedule the event to run every five minutes.
    pub fn every_five_minutes(self) -> Self {
        self.splice_into_position(1, "*/5")
    }

    /// Schedule the event to run every ten minutes.
    pub fn every_ten_minutes(self) -> Self {
        self.splice_into_position(1, "*/10")
    }

    /// Schedule the event to run every fifteen minutes.
    pub fn every_fifteen_minutes(self) -> Self {
        self.splice_into_position(1, "*/15")
    }

    /// Schedule the event to run every thirty minutes.
    pub fn every_thirty_minutes(self) -> Self {
        self.splice_into_position(1, "*/30")
    }

    /// Schedule the event to run hourly.
    pub fn hourly(self) -> Self {
        self.splice_into_position(1, 0)
    }

    /// Schedule the event to run hourly at a given offset in the hour.
    pub fn hourly_at(self, offset: u32) -> Self {
        self.hour_based_schedule(offset, "*")
    }

    /// Schedule the event to run hourly at the given offsets in the hour.
    pub fn hourly_at_offsets<I: IntoIterator<Item = u32>>(self, offsets: I) -> Self {
        self.hour_based_schedule(join(offsets), "*")
    }

    /// Schedule the event to run every odd hour.
    pub fn every_odd_hour(self) -> Self {
        self.every_odd_hour_at(0)
    }

    /// Schedule the event to run every odd hour at the given minute.
    pub fn every_odd_hour_at(self, offset: u32) -> Self {
        self.hour_based_schedule(offset, "1-23/2")
    }

    /// Schedule the event to run every two hours.
    pub fn every_two_hours(self) -> Self {
        self.every_two_hours_at(0)
    }

    /// Schedule the event to run every two hours at the given minute.
    pub fn every_two_hours_at(self, offset: u32) -> Self {
        self.hour_based_schedule(offset, "*/2")
    }

    /// Schedule the event to run every three hours.
    pub fn every_three_hours(self) -> Self {
        self.every_three_hours_at(0)
    }

    /// Schedule the event to run every three hours at the given minute.
    pub fn every_three_hours_at(self, offset: u32) -> Self {
        self.hour_based_schedule(offset, "*/3")
    }

    /// Schedule the event to run every four hours.
    pub fn every_four_hours(self) -> Self {
        self.every_four_hours_at(0)
    }

    /// Schedule the event to run every four hours at the given minute.
    pub fn every_four_hours_at(self, offset: u32) -> Self {
        self.hour_based_schedule(offset, "*/4")
    }

    /// Schedule the event to run every six hours.
    pub fn every_six_hours(self) -> Self {
        self.every_six_hours_at(0)
    }

    /// Schedule the event to run every six hours at the given minute.
    pub fn every_six_hours_at(self, offset: u32) -> Self {
        self.hour_based_schedule(offset, "*/6")
    }

    /// Schedule the event to run daily (at midnight).
    pub fn daily(self) -> Self {
        self.hour_based_schedule(0, 0)
    }

    /// Schedule the event to run daily at a given time (`"13:00"`).
    pub fn at(self, time: &str) -> Self {
        self.daily_at(time)
    }

    /// Schedule the event to run daily at a given time (`"13:00"`).
    pub fn daily_at(self, time: &str) -> Self {
        let (minutes, hours) = parse_time(time);
        self.hour_based_schedule(minutes, hours)
    }

    /// Schedule the event to run twice daily, at the given hours.
    pub fn twice_daily(self, first: u32, second: u32) -> Self {
        self.twice_daily_at(first, second, 0)
    }

    /// Schedule the event to run twice daily at the given hours and minute.
    pub fn twice_daily_at(self, first: u32, second: u32, offset: u32) -> Self {
        self.hour_based_schedule(offset, format!("{first},{second}"))
    }

    /// Schedule the event to run only on weekdays.
    pub fn weekdays(self) -> Self {
        self.splice_into_position(5, "1-5")
    }

    /// Schedule the event to run only on weekends.
    pub fn weekends(self) -> Self {
        self.splice_into_position(5, "6,0")
    }

    /// Schedule the event to run only on Mondays.
    pub fn mondays(self) -> Self {
        self.days([Schedule::MONDAY])
    }

    /// Schedule the event to run only on Tuesdays.
    pub fn tuesdays(self) -> Self {
        self.days([Schedule::TUESDAY])
    }

    /// Schedule the event to run only on Wednesdays.
    pub fn wednesdays(self) -> Self {
        self.days([Schedule::WEDNESDAY])
    }

    /// Schedule the event to run only on Thursdays.
    pub fn thursdays(self) -> Self {
        self.days([Schedule::THURSDAY])
    }

    /// Schedule the event to run only on Fridays.
    pub fn fridays(self) -> Self {
        self.days([Schedule::FRIDAY])
    }

    /// Schedule the event to run only on Saturdays.
    pub fn saturdays(self) -> Self {
        self.days([Schedule::SATURDAY])
    }

    /// Schedule the event to run only on Sundays.
    pub fn sundays(self) -> Self {
        self.days([Schedule::SUNDAY])
    }

    /// Schedule the event to run weekly (Sunday at midnight).
    pub fn weekly(self) -> Self {
        self.splice_into_position(1, 0)
            .splice_into_position(2, 0)
            .splice_into_position(5, 0)
    }

    /// Schedule the event to run weekly on a given day and time.
    pub fn weekly_on(self, day: u32, time: &str) -> Self {
        self.daily_at(time).days([day])
    }

    /// Schedule the event to run monthly (the first day, at midnight).
    pub fn monthly(self) -> Self {
        self.splice_into_position(1, 0)
            .splice_into_position(2, 0)
            .splice_into_position(3, 1)
    }

    /// Schedule the event to run monthly on a given day and time.
    pub fn monthly_on(self, day: u32, time: &str) -> Self {
        self.daily_at(time).splice_into_position(3, day)
    }

    /// Schedule the event to run twice monthly at a given time.
    pub fn twice_monthly(self, first: u32, second: u32, time: &str) -> Self {
        self.daily_at(time)
            .splice_into_position(3, format!("{first},{second}"))
    }

    /// Schedule the event to run on the last day of the month.
    pub fn last_day_of_month(self, time: &str) -> Self {
        let day = Carbon::now().end_of_month().day();
        self.daily_at(time).splice_into_position(3, day)
    }

    /// Schedule the event to run on specific days of the month.
    pub fn days_of_month<I: IntoIterator<Item = u32>>(self, days: I) -> Self {
        self.daily_at("0:0").splice_into_position(3, join(days))
    }

    /// Schedule the event to run quarterly.
    pub fn quarterly(self) -> Self {
        self.splice_into_position(1, 0)
            .splice_into_position(2, 0)
            .splice_into_position(3, 1)
            .splice_into_position(4, "1-12/3")
    }

    /// Schedule the event to run quarterly on a given day and time.
    pub fn quarterly_on(self, day: u32, time: &str) -> Self {
        self.daily_at(time)
            .splice_into_position(3, day)
            .splice_into_position(4, "1-12/3")
    }

    /// Schedule the event to run yearly.
    pub fn yearly(self) -> Self {
        self.splice_into_position(1, 0)
            .splice_into_position(2, 0)
            .splice_into_position(3, 1)
            .splice_into_position(4, 1)
    }

    /// Schedule the event to run yearly on a given month, day, and time.
    pub fn yearly_on(self, month: u32, day: u32, time: &str) -> Self {
        self.daily_at(time)
            .splice_into_position(3, day)
            .splice_into_position(4, month)
    }

    /// Set the days of the week the command should run on.
    pub fn days<I: IntoIterator<Item = u32>>(self, days: I) -> Self {
        self.splice_into_position(5, join(days))
    }

    /// Set the timezone the date should be evaluated on.
    pub fn timezone(self, timezone: impl Into<String>) -> Self {
        let timezone = timezone.into();
        self.update(|state| state.timezone = Some(timezone))
    }

    /// Schedule the event to run between start and end time (`"8:00"`, `"17:00"`).
    pub fn between(self, start: &str, end: &str) -> Self {
        let (start, end) = (start.to_string(), end.to_string());
        self.update(|state| {
            state.filters.push(Filter::Between {
                start,
                end,
                inside: true,
            })
        })
    }

    /// Schedule the event to not run between start and end time.
    pub fn unless_between(self, start: &str, end: &str) -> Self {
        let (start, end) = (start.to_string(), end.to_string());
        self.update(|state| {
            state.filters.push(Filter::Between {
                start,
                end,
                inside: false,
            })
        })
    }

    // ------------------------------------------------------------------
    // Constraints & attributes
    // ------------------------------------------------------------------

    /// Register a callback to further filter the schedule.
    pub fn when(self, callback: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.update(|state| state.filters.push(Filter::When(Arc::new(callback))))
    }

    /// Register a callback to skip the event when it returns `true`.
    pub fn skip(self, callback: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.update(|state| state.filters.push(Filter::Skip(Arc::new(callback))))
    }

    /// Limit the environments the command should run in.
    pub fn environments<I, S>(self, environments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let environments: Vec<String> = environments.into_iter().map(Into::into).collect();
        self.update(|state| state.environments = environments)
    }

    /// State that the command should run even in maintenance mode.
    pub fn even_in_maintenance_mode(self) -> Self {
        self.update(|state| state.even_in_maintenance_mode = true)
    }

    /// Run the event even while the schedule is paused
    /// (`schedule:pause`).
    pub fn even_when_paused(self) -> Self {
        self.update(|state| state.even_when_paused = true)
    }

    /// Do not allow the event to overlap each other (the lock expires
    /// after 24 hours).
    pub fn without_overlapping(self) -> Self {
        self.without_overlapping_for(1440)
    }

    /// Do not allow the event to overlap each other, with a lock expiring
    /// after the given number of minutes.
    pub fn without_overlapping_for(self, expires_at: u64) -> Self {
        self.update(|state| {
            state.without_overlapping = true;
            state.expires_at = expires_at;
        })
    }

    /// Allow the event to only run on one server for each cron expression.
    pub fn on_one_server(self) -> Self {
        self.update(|state| state.on_one_server = true)
    }

    /// State that the command should run in the background.
    pub fn run_in_background(self) -> Self {
        self.update(|state| state.run_in_background = true)
    }

    /// Set the human-friendly description of the event.
    pub fn name(self, description: impl Into<String>) -> Self {
        self.description(description)
    }

    /// Set the human-friendly description of the event.
    pub fn description(self, description: impl Into<String>) -> Self {
        let description = description.into();
        self.update(|state| state.description = Some(description))
    }

    /// Send the output of the command to a given location.
    pub fn send_output_to(self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        self.update(|state| state.output_path = Some((path, false)))
    }

    /// Append the output of the command to a given location.
    pub fn append_output_to(self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        self.update(|state| state.output_path = Some((path, true)))
    }

    /// Use the given mutex to prevent overlaps.
    pub fn prevent_overlaps_using(self, mutex: Arc<dyn EventMutex>) -> Self {
        self.update(|state| state.mutex = mutex)
    }

    /// Use a custom mutex name.
    pub fn create_mutex_name_using(self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.update(|state| state.mutex_name = Some(name))
    }

    // ------------------------------------------------------------------
    // Hooks
    // ------------------------------------------------------------------

    fn hook<F, Fut>(callback: F) -> HookFn
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Arc::new(move || Box::pin(callback()))
    }

    /// Register a callback to be called before the operation.
    pub fn before<F, Fut>(self, callback: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let hook = Self::hook(callback);
        self.update(|state| state.before.push(hook))
    }

    fn after_hook<F, Fut>(callback: F) -> AfterFn
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Arc::new(move |_| Box::pin(callback()))
    }

    fn output_hook<F, Fut>(callback: F) -> AfterFn
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Arc::new(move |output| Box::pin(callback(output.to_string())))
    }

    fn push_after(self, condition: Condition, hook: AfterFn) -> Self {
        self.update(|state| state.after.push((condition, hook)))
    }

    /// Register a callback to be called after the operation.
    pub fn after<F, Fut>(self, callback: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.then(callback)
    }

    /// Register a callback to be called after the operation.
    pub fn then<F, Fut>(self, callback: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.push_after(Condition::Always, Self::after_hook(callback))
    }

    /// Register a callback that receives the task's output after it runs.
    ///
    /// ```
    /// use illuminate_console::scheduling::Schedule;
    ///
    /// Schedule::new()
    ///     .command("emails:send")
    ///     .daily()
    ///     .then_with_output(|output| async move {
    ///         println!("{output}");
    ///     });
    /// ```
    pub fn then_with_output<F, Fut>(self, callback: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.push_after(Condition::Always, Self::output_hook(callback))
    }

    /// Register a callback to be called if the operation succeeds.
    pub fn on_success<F, Fut>(self, callback: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.push_after(Condition::Success, Self::after_hook(callback))
    }

    /// Register a callback that receives the task's output if it succeeds.
    pub fn on_success_with_output<F, Fut>(self, callback: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.push_after(Condition::Success, Self::output_hook(callback))
    }

    /// Register a callback to be called if the operation fails.
    pub fn on_failure<F, Fut>(self, callback: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.push_after(Condition::Failure, Self::after_hook(callback))
    }

    /// Register a callback that receives the task's output if it fails.
    pub fn on_failure_with_output<F, Fut>(self, callback: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.push_after(Condition::Failure, Self::output_hook(callback))
    }

    // ------------------------------------------------------------------
    // Pinging URLs
    // ------------------------------------------------------------------

    fn ping(url: String) -> impl Fn() -> BoxFuture<'static, ()> + Send + Sync + 'static {
        move || {
            let url = url.clone();
            Box::pin(async move {
                let response = illuminate_http_client::Http::new_request()
                    .connect_timeout(10)
                    .timeout(30)
                    .get(&url)
                    .await;
                if let Err(error) = response {
                    report(&error);
                }
            })
        }
    }

    /// Ping the URL before the task runs (a GET request), for monitoring
    /// services like Envoyer or Healthchecks.
    pub fn ping_before(self, url: impl Into<String>) -> Self {
        let ping = Self::ping(url.into());
        self.update(|state| state.before.push(Arc::new(ping)))
    }

    /// Ping the URL before the task runs, if the condition is true.
    pub fn ping_before_if(self, condition: bool, url: impl Into<String>) -> Self {
        if condition { self.ping_before(url) } else { self }
    }

    /// Ping the URL after the task runs.
    pub fn then_ping(self, url: impl Into<String>) -> Self {
        let ping = Self::ping(url.into());
        self.push_after(Condition::Always, Arc::new(move |_| ping()))
    }

    /// Ping the URL after the task runs, if the condition is true.
    pub fn then_ping_if(self, condition: bool, url: impl Into<String>) -> Self {
        if condition { self.then_ping(url) } else { self }
    }

    /// Ping the URL if the task succeeds.
    pub fn ping_on_success(self, url: impl Into<String>) -> Self {
        let ping = Self::ping(url.into());
        self.push_after(Condition::Success, Arc::new(move |_| ping()))
    }

    /// Ping the URL if the task succeeds and the condition is true.
    pub fn ping_on_success_if(self, condition: bool, url: impl Into<String>) -> Self {
        if condition { self.ping_on_success(url) } else { self }
    }

    /// Ping the URL if the task fails.
    pub fn ping_on_failure(self, url: impl Into<String>) -> Self {
        let ping = Self::ping(url.into());
        self.push_after(Condition::Failure, Arc::new(move |_| ping()))
    }

    /// Ping the URL if the task fails and the condition is true.
    pub fn ping_on_failure_if(self, condition: bool, url: impl Into<String>) -> Self {
        if condition { self.ping_on_failure(url) } else { self }
    }

    // ------------------------------------------------------------------
    // Emailing output
    // ------------------------------------------------------------------

    fn email_output(&self, addresses: Vec<String>, only_if_output_exists: bool) -> AfterFn {
        let event = self.clone();
        Arc::new(move |output: Arc<str>| {
            let addresses = addresses.clone();
            let event = event.clone();
            Box::pin(async move {
                if only_if_output_exists && output.trim().is_empty() {
                    return;
                }
                let Ok(mailer) = illuminate_container::Container::get_instance().try_make::<dyn ScheduleOutputMailer>()
                else {
                    report(&illuminate_support::error::error!(
                        "Unable to email the output of [{}]: no mailer is configured.",
                        event.summary_for_display()
                    ));
                    return;
                };
                if let Err(error) = mailer.send(&addresses, &event.email_subject(), &output).await {
                    report(&error);
                }
            })
        })
    }

    fn email_subject(&self) -> String {
        match self.get_description() {
            Some(description) => description,
            None => format!(
                "Scheduled Job Output For [{}]",
                self.get_command().unwrap_or_default()
            ),
        }
    }

    /// Email the task's output to the given addresses, when it wrote any.
    pub fn email_output_to<S: Into<String>>(self, addresses: impl IntoIterator<Item = S>) -> Self {
        let hook = self.email_output(addresses.into_iter().map(Into::into).collect(), true);
        self.push_after(Condition::Always, hook)
    }

    /// Email the task's output to the given addresses, when it wrote any.
    pub fn email_written_output_to<S: Into<String>>(self, addresses: impl IntoIterator<Item = S>) -> Self {
        self.email_output_to(addresses)
    }

    /// Email the task's output to the given addresses if it fails.
    pub fn email_output_on_failure<S: Into<String>>(self, addresses: impl IntoIterator<Item = S>) -> Self {
        let hook = self.email_output(addresses.into_iter().map(Into::into).collect(), false);
        self.push_after(Condition::Failure, hook)
    }

    // ------------------------------------------------------------------
    // Inspecting the event
    // ------------------------------------------------------------------

    /// The cron expression for the event.
    pub fn expression(&self) -> String {
        self.state().expression.clone()
    }

    /// The timezone the event is evaluated in.
    pub fn get_timezone(&self) -> Option<String> {
        self.state().timezone.clone()
    }

    /// The event's description, if one was given.
    pub fn get_description(&self) -> Option<String> {
        self.state().description.clone()
    }

    /// The environments the event runs in (empty means every environment).
    pub fn get_environments(&self) -> Vec<String> {
        self.state().environments.clone()
    }

    /// The command line for command and exec events.
    pub fn get_command(&self) -> Option<String> {
        match &self.state().task {
            Task::Command(command) => Some(command.clone()),
            Task::Exec(command) => Some(command.clone()),
            Task::Callback { .. } => None,
        }
    }

    /// Determine if the event runs a closure.
    pub fn is_callback(&self) -> bool {
        matches!(self.state().task, Task::Callback { .. })
    }

    /// Determine if the event runs an Artisan command.
    pub fn is_command(&self) -> bool {
        matches!(self.state().task, Task::Command(_))
    }

    /// Where the closure of a callback event was defined.
    pub fn location(&self) -> Option<String> {
        match &self.state().task {
            Task::Callback { location, .. } => location.clone(),
            _ => None,
        }
    }

    /// The command line as it is displayed (`artisan inspire`).
    pub fn command_for_display(&self) -> String {
        match &self.state().task {
            Task::Command(command) => format!("artisan {command}"),
            Task::Exec(command) => command.clone(),
            Task::Callback { .. } => "Callback".to_string(),
        }
    }

    /// The summary of the event for display.
    pub fn summary_for_display(&self) -> String {
        if let Some(description) = self.get_description() {
            return description;
        }
        self.command_for_display()
    }

    /// Determine if the event runs in the background.
    pub fn runs_in_background(&self) -> bool {
        self.state().run_in_background
    }

    /// Determine if the event only runs on one server.
    pub fn runs_on_one_server(&self) -> bool {
        self.state().on_one_server
    }

    /// Determine if the event prevents overlapping.
    pub fn prevents_overlapping(&self) -> bool {
        self.state().without_overlapping
    }

    /// Determine if the event runs in maintenance mode.
    pub fn runs_in_maintenance_mode(&self) -> bool {
        self.state().even_in_maintenance_mode
    }

    /// Whether the event runs while the schedule is paused.
    pub fn runs_when_paused(&self) -> bool {
        self.state().even_when_paused
    }

    /// Whether the event repeats within the minute (`every_second`, ...).
    pub fn is_repeatable(&self) -> bool {
        self.state().repeat_seconds.is_some()
    }

    /// The number of seconds between repetitions of a sub-minute event.
    pub fn repeat_seconds(&self) -> Option<u32> {
        self.state().repeat_seconds
    }

    /// Whether a sub-minute event is due to repeat: its interval has
    /// passed since it was last checked.
    pub fn should_repeat_now(&self) -> bool {
        let state = self.state();
        match (state.repeat_seconds, &state.last_checked) {
            (Some(seconds), Some(last_checked)) => {
                Carbon::now().timestamp_millis() - last_checked.timestamp_millis() >= i64::from(seconds) * 1000
            }
            _ => false,
        }
    }

    /// The number of minutes the overlapping lock is held for.
    pub fn expires_at(&self) -> u64 {
        self.state().expires_at
    }

    /// The exit code of the last run.
    pub fn exit_code(&self) -> Option<i32> {
        self.state().exit_code
    }

    /// Determine if the last run was skipped because it would have overlapped.
    pub fn skipped_because_overlapping(&self) -> bool {
        self.state().skipped_because_overlapping
    }

    /// The mutex used to prevent overlaps.
    pub fn mutex(&self) -> Arc<dyn EventMutex> {
        self.state().mutex.clone()
    }

    /// The mutex name for the scheduled command.
    pub fn mutex_name(&self) -> String {
        let state = self.state();

        if let Some(name) = &state.mutex_name {
            return name.clone();
        }

        match &state.task {
            Task::Command(command) => {
                format!(
                    "framework/schedule-{}",
                    sha1(&format!("{}artisan {command}", state.expression))
                )
            }
            Task::Exec(command) => format!(
                "framework/schedule-{}",
                sha1(&format!("{}{command}", state.expression))
            ),
            Task::Callback { location, .. } => {
                let name = state
                    .description
                    .clone()
                    .or_else(|| location.clone())
                    .unwrap_or_default();
                format!("framework/schedule-{}", sha1(&name))
            }
        }
    }

    /// Determine if the event's mutex is currently held.
    pub async fn has_mutex(&self) -> bool {
        self.mutex().exists(self).await
    }

    /// Determine if the event runs in the given environment.
    pub fn runs_in_environment(&self, environment: &str) -> bool {
        let state = self.state();
        state.environments.is_empty() || state.environments.iter().any(|env| env == environment)
    }

    /// Determine if the event's cron expression passes at the given time.
    pub fn expression_passes(&self, now: &Carbon) -> bool {
        let (expression, timezone) = {
            let state = self.state();
            (state.expression.clone(), state.timezone.clone())
        };

        CronExpression::parse(&expression)
            .map(|cron| cron.is_due_in(now, timezone.as_deref()))
            .unwrap_or(false)
    }

    /// Determine if the event is due to run, using the application's
    /// environment (`app.env`).
    pub fn is_due(&self, now: &Carbon) -> bool {
        self.is_due_in(now, &current_environment(), false)
    }

    /// Determine if the event is due in the given environment.
    pub fn is_due_in(&self, now: &Carbon, environment: &str, down_for_maintenance: bool) -> bool {
        if down_for_maintenance && !self.runs_in_maintenance_mode() {
            return false;
        }

        self.expression_passes(now) && self.runs_in_environment(environment)
    }

    /// The next date the event is due to run.
    pub fn next_run_date(&self, now: &Carbon) -> Result<Carbon> {
        let (expression, timezone) = {
            let state = self.state();
            (state.expression.clone(), state.timezone.clone())
        };

        Ok(CronExpression::parse(&expression)?.run_date(
            now,
            0,
            false,
            timezone.as_deref(),
            true,
        )?)
    }

    /// Determine if the filters pass for the event.
    pub async fn filters_pass(&self, now: &Carbon) -> bool {
        let (filters, timezone, without_overlapping) = {
            let mut state = self.state();
            state.last_checked = Some(Carbon::now());
            (
                state.filters.clone(),
                state.timezone.clone(),
                state.without_overlapping,
            )
        };

        for filter in filters {
            let passes = match filter {
                Filter::When(callback) => callback(),
                Filter::Skip(callback) => !callback(),
                Filter::Between { start, end, inside } => {
                    in_time_interval(now, &start, &end, timezone.as_deref()) == inside
                }
            };

            if !passes {
                return false;
            }
        }

        if without_overlapping && self.mutex().exists(self).await {
            return false;
        }

        true
    }

    // ------------------------------------------------------------------
    // Running the event
    // ------------------------------------------------------------------

    /// Run the event.
    pub async fn run(&self, application: &Arc<Application>) -> Result<()> {
        self.run_with(application, false).await
    }

    pub(crate) async fn run_with(
        &self,
        application: &Arc<Application>,
        force_foreground: bool,
    ) -> Result<()> {
        self.state().skipped_because_overlapping = false;

        let (without_overlapping, background) = {
            let state = self.state();
            (
                state.without_overlapping,
                state.run_in_background && !force_foreground,
            )
        };

        if without_overlapping && !self.mutex().create(self).await {
            self.state().skipped_because_overlapping = true;
            return Ok(());
        }

        if background {
            let event = self.clone();
            let application = application.clone();
            tokio::spawn(async move {
                let (code, error) = event.start(&application).await;
                event.finish(code).await;
                if let Some(error) = error {
                    application.report(&error);
                }
                super::events::dispatch(super::events::ScheduledBackgroundTaskFinished { task: event.clone() }).await;
            });
            return Ok(());
        }

        let (code, error) = self.start(application).await;
        self.finish(code).await;

        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn start(&self, application: &Arc<Application>) -> (i32, Option<Error>) {
        let before = self.state().before.clone();
        for hook in before {
            hook().await;
        }

        let task = self.state().task.clone();
        let mut captured = String::new();

        let result = match task {
            Task::Command(command) => {
                let output = Output::buffered();
                let result = application.call_with_output(&command, (), &output).await;
                captured = output.contents();
                result
            }
            Task::Exec(command) => run_shell(&command).await.map(|(code, output)| {
                captured = output;
                code
            }),
            Task::Callback { callback, .. } => callback().await,
        };

        self.write_output(&captured);
        self.state().last_output = Arc::from(captured.as_str());

        match result {
            Ok(code) => (code, None),
            Err(error) => (1, Some(error)),
        }
    }

    fn write_output(&self, captured: &str) {
        let Some((path, append)) = self.state().output_path.clone() else {
            return;
        };

        let result = if append {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut file| file.write_all(captured.as_bytes()))
        } else {
            std::fs::write(&path, captured)
        };

        // Writing the output is best effort, as in Laravel.
        let _ = result;
    }

    async fn finish(&self, code: i32) {
        let (after, without_overlapping, output) = {
            let mut state = self.state();
            state.exit_code = Some(code);
            (state.after.clone(), state.without_overlapping, state.last_output.clone())
        };

        for (condition, hook) in after {
            let run = match condition {
                Condition::Always => true,
                Condition::Success => code == 0,
                Condition::Failure => code != 0,
            };
            if run {
                hook(output.clone()).await;
            }
        }

        if without_overlapping {
            self.mutex().forget(self).await;
        }
    }
}

async fn run_shell(command: &str) -> Result<(i32, String)> {
    let mut process = if cfg!(windows) {
        let mut process = tokio::process::Command::new("cmd");
        process.arg("/C").arg(command);
        process
    } else {
        let mut process = tokio::process::Command::new("sh");
        process.arg("-c").arg(command);
        process
    };

    let output = process.output().await?;
    let mut captured = String::from_utf8_lossy(&output.stdout).into_owned();
    captured.push_str(&String::from_utf8_lossy(&output.stderr));

    Ok((output.status.code().unwrap_or(1), captured))
}

/// The application's environment (`app.env`), defaulting to "production".
pub(crate) fn current_environment() -> String {
    illuminate_container::try_app::<illuminate_config::Repository>()
        .map(|config| config.string_or("app.env", "production"))
        .unwrap_or_else(|| "production".to_string())
}

fn in_time_interval(now: &Carbon, start: &str, end: &str, timezone: Option<&str>) -> bool {
    let now = match timezone {
        Some(timezone) => now.tz(timezone).unwrap_or(*now),
        None => *now,
    };

    let at = |time: &str| {
        let (minutes, hours) = parse_time(time);
        now.start_of_day()
            .set_time(hours.parse().unwrap_or(0), minutes.parse().unwrap_or(0), 0)
    };

    let (mut start, mut end) = (at(start), at(end));

    if end.lt(&start) {
        if start.gt(&now) {
            start = start.sub_day();
        } else {
            end = end.add_day();
        }
    }

    now.between(&start, &end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> Event {
        Event::command("inspire")
    }

    #[test]
    fn frequencies_match_laravel() {
        assert_eq!(event().expression(), "* * * * *");
        assert_eq!(event().every_minute().expression(), "* * * * *");
        assert_eq!(event().every_two_minutes().expression(), "*/2 * * * *");
        assert_eq!(event().every_three_minutes().expression(), "*/3 * * * *");
        assert_eq!(event().every_four_minutes().expression(), "*/4 * * * *");
        assert_eq!(event().every_five_minutes().expression(), "*/5 * * * *");
        assert_eq!(event().every_ten_minutes().expression(), "*/10 * * * *");
        assert_eq!(event().every_fifteen_minutes().expression(), "*/15 * * * *");
        assert_eq!(event().every_thirty_minutes().expression(), "*/30 * * * *");
        assert_eq!(event().hourly().expression(), "0 * * * *");
        assert_eq!(event().hourly_at(17).expression(), "17 * * * *");
        assert_eq!(
            event().hourly_at_offsets([0, 30]).expression(),
            "0,30 * * * *"
        );
        assert_eq!(event().every_odd_hour().expression(), "0 1-23/2 * * *");
        assert_eq!(event().every_odd_hour_at(5).expression(), "5 1-23/2 * * *");
        assert_eq!(event().every_two_hours().expression(), "0 */2 * * *");
        assert_eq!(event().every_three_hours().expression(), "0 */3 * * *");
        assert_eq!(event().every_four_hours().expression(), "0 */4 * * *");
        assert_eq!(event().every_six_hours_at(15).expression(), "15 */6 * * *");
        assert_eq!(event().daily().expression(), "0 0 * * *");
        assert_eq!(event().daily_at("13:00").expression(), "0 13 * * *");
        assert_eq!(event().daily_at("8:05").expression(), "5 8 * * *");
        assert_eq!(event().daily_at("10").expression(), "0 10 * * *");
        assert_eq!(event().at("13:30").expression(), "30 13 * * *");
        assert_eq!(event().twice_daily(1, 13).expression(), "0 1,13 * * *");
        assert_eq!(
            event().twice_daily_at(1, 13, 15).expression(),
            "15 1,13 * * *"
        );
        assert_eq!(event().weekly().expression(), "0 0 * * 0");
        assert_eq!(event().weekly_on(1, "8:00").expression(), "0 8 * * 1");
        assert_eq!(event().monthly().expression(), "0 0 1 * *");
        assert_eq!(event().monthly_on(4, "15:00").expression(), "0 15 4 * *");
        assert_eq!(
            event().twice_monthly(1, 16, "13:00").expression(),
            "0 13 1,16 * *"
        );
        assert_eq!(
            event().days_of_month([1, 10, 20]).expression(),
            "0 0 1,10,20 * *"
        );
        assert_eq!(event().quarterly().expression(), "0 0 1 1-12/3 *");
        assert_eq!(
            event().quarterly_on(4, "14:00").expression(),
            "0 14 4 1-12/3 *"
        );
        assert_eq!(event().yearly().expression(), "0 0 1 1 *");
        assert_eq!(event().yearly_on(6, 1, "17:00").expression(), "0 17 1 6 *");
        assert_eq!(event().cron("1 2 3 4 5").expression(), "1 2 3 4 5");
        assert_eq!(
            event().last_day_of_month("15:00").expression(),
            format!("0 15 {} * *", Carbon::now().end_of_month().day())
        );
    }

    #[test]
    fn day_constraints_match_laravel() {
        assert_eq!(event().weekdays().expression(), "* * * * 1-5");
        assert_eq!(event().weekends().expression(), "* * * * 6,0");
        assert_eq!(event().mondays().expression(), "* * * * 1");
        assert_eq!(event().tuesdays().expression(), "* * * * 2");
        assert_eq!(event().wednesdays().expression(), "* * * * 3");
        assert_eq!(event().thursdays().expression(), "* * * * 4");
        assert_eq!(event().fridays().expression(), "* * * * 5");
        assert_eq!(event().saturdays().expression(), "* * * * 6");
        assert_eq!(event().sundays().expression(), "* * * * 0");
        assert_eq!(event().days([0, 3]).expression(), "* * * * 0,3");
        assert_eq!(
            event().weekly().mondays().at("13:00").expression(),
            "0 13 * * 1"
        );
        assert_eq!(event().weekdays().hourly().expression(), "0 * * * 1-5");
        assert_eq!(event().daily().weekdays().expression(), "0 0 * * 1-5");
    }

    #[test]
    fn it_checks_if_due() {
        let date = Carbon::parse("2024-03-11 13:00:00").unwrap();
        assert!(
            event()
                .daily_at("13:00")
                .is_due_in(&date, "production", false)
        );
        assert!(
            !event()
                .daily_at("14:00")
                .is_due_in(&date, "production", false)
        );
        assert!(event().mondays().is_due_in(&date, "production", false));
        assert!(!event().tuesdays().is_due_in(&date, "production", false));

        let restricted = event().environments(["staging"]);
        assert!(!restricted.is_due_in(&date, "production", false));
        assert!(restricted.is_due_in(&date, "staging", false));

        assert!(!event().is_due_in(&date, "production", true));
        assert!(
            event()
                .even_in_maintenance_mode()
                .is_due_in(&date, "production", true)
        );
    }

    #[test]
    fn it_evaluates_timezones() {
        let date = Carbon::parse("2024-01-15T14:00:00Z").unwrap();
        let event = event().daily_at("9:00").timezone("America/New_York");
        assert!(event.is_due_in(&date, "production", false));
        assert_eq!(
            event.next_run_date(&date).unwrap().to_date_time_string(),
            "2024-01-16 09:00:00"
        );
    }

    #[tokio::test]
    async fn it_filters_with_callbacks_and_times() {
        let now = Carbon::parse("2024-03-11 13:00:00").unwrap();
        assert!(event().when(|| true).filters_pass(&now).await);
        assert!(!event().when(|| false).filters_pass(&now).await);
        assert!(!event().skip(|| true).filters_pass(&now).await);
        assert!(event().between("8:00", "17:00").filters_pass(&now).await);
        assert!(!event().between("14:00", "17:00").filters_pass(&now).await);
        assert!(
            event()
                .unless_between("14:00", "17:00")
                .filters_pass(&now)
                .await
        );
        assert!(
            !event()
                .unless_between("23:00", "14:00")
                .filters_pass(&now)
                .await
        );
        assert!(event().between("22:00", "14:00").filters_pass(&now).await);
    }

    #[test]
    fn it_builds_summaries_and_mutex_names() {
        let event = event();
        assert_eq!(event.summary_for_display(), "artisan inspire");
        assert_eq!(event.get_command().as_deref(), Some("inspire"));
        assert!(event.mutex_name().starts_with("framework/schedule-"));
        assert_eq!(event.mutex_name().len(), "framework/schedule-".len() + 40);

        let named = event.clone().name("Inspire everyone");
        assert_eq!(named.summary_for_display(), "Inspire everyone");

        let custom = Event::command("x").create_mutex_name_using("my-lock");
        assert_eq!(custom.mutex_name(), "my-lock");
    }
}
