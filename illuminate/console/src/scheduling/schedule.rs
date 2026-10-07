//! The schedule: every scheduled event of the application.

use std::collections::HashMap;
use std::future::Future;
use std::panic::Location;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use illuminate_container::Container;
use illuminate_support::{Carbon, Value, ValueExt};

use super::cache::{InMemoryScheduleCache, ScheduleCache};
use super::event::{CallbackFn, Event, IntoExitCode, Task, current_environment};
use super::mutex::{EventMutex, InMemoryEventMutex, InMemorySchedulingMutex, SchedulingMutex};
use crate::input::ArtisanArgs;

type MaintenanceResolver = Arc<dyn Fn() -> bool + Send + Sync>;

struct Inner {
    events: RwLock<Vec<Event>>,
    timezone: RwLock<Option<String>>,
    event_mutex: RwLock<Arc<dyn EventMutex>>,
    scheduling_mutex: RwLock<Arc<dyn SchedulingMutex>>,
    mutex_cache: Mutex<HashMap<String, bool>>,
    maintenance: RwLock<Option<MaintenanceResolver>>,
    cache: RwLock<Option<Arc<dyn ScheduleCache>>>,
    fallback_cache: Arc<InMemoryScheduleCache>,
    pausable: AtomicBool,
    interruptible: AtomicBool,
}

/// The cache key of the pause signal.
const PAUSED: &str = "illuminate:schedule:paused";

/// The cache key of the interrupt signal.
const INTERRUPT: &str = "illuminate:schedule:interrupt";

/// The application's schedule (`Illuminate\Console\Scheduling\Schedule`).
///
/// A cheap, cloneable handle. Most applications use the
/// [`Schedule`](crate::Schedule) facade instead.
///
/// ```
/// use illuminate_console::scheduling::Schedule;
///
/// let schedule = Schedule::new();
///
/// schedule.command("emails:send Taylor --force").daily();
/// schedule.call(|| async { /* ... */ }).twice_daily(1, 13);
/// schedule.exec("node /home/forge/script.js").daily_at("13:00");
///
/// assert_eq!(schedule.events().len(), 3);
/// assert_eq!(schedule.events()[1].expression(), "0 1,13 * * *");
/// ```
#[derive(Clone)]
pub struct Schedule {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Schedule")
            .field("events", &self.events())
            .finish()
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl Schedule {
    /// Sunday, for use with `days(...)`.
    pub const SUNDAY: u32 = 0;
    /// Monday, for use with `days(...)`.
    pub const MONDAY: u32 = 1;
    /// Tuesday, for use with `days(...)`.
    pub const TUESDAY: u32 = 2;
    /// Wednesday, for use with `days(...)`.
    pub const WEDNESDAY: u32 = 3;
    /// Thursday, for use with `days(...)`.
    pub const THURSDAY: u32 = 4;
    /// Friday, for use with `days(...)`.
    pub const FRIDAY: u32 = 5;
    /// Saturday, for use with `days(...)`.
    pub const SATURDAY: u32 = 6;

    /// Create a new schedule. Mutexes bound in the current container
    /// (`dyn EventMutex`, `dyn SchedulingMutex`) are used, falling back to
    /// in-memory ones.
    pub fn new() -> Self {
        Self::with_timezone(None)
    }

    /// Create a new schedule whose events are evaluated in the given timezone.
    pub fn with_timezone(timezone: Option<String>) -> Self {
        let container = Container::get_instance();

        let event_mutex: Arc<dyn EventMutex> = container
            .try_make::<dyn EventMutex>()
            .unwrap_or_else(|_| Arc::new(InMemoryEventMutex::new()));

        let scheduling_mutex: Arc<dyn SchedulingMutex> = container
            .try_make::<dyn SchedulingMutex>()
            .unwrap_or_else(|_| Arc::new(InMemorySchedulingMutex::new()));

        Self {
            inner: Arc::new(Inner {
                events: RwLock::new(Vec::new()),
                timezone: RwLock::new(timezone),
                event_mutex: RwLock::new(event_mutex),
                scheduling_mutex: RwLock::new(scheduling_mutex),
                mutex_cache: Mutex::new(HashMap::new()),
                maintenance: RwLock::new(None),
                cache: RwLock::new(None),
                fallback_cache: Arc::new(InMemoryScheduleCache::new()),
                pausable: AtomicBool::new(true),
                interruptible: AtomicBool::new(true),
            }),
        }
    }

    /// The default timezone of new events.
    pub fn timezone(&self) -> Option<String> {
        self.inner.timezone.read().unwrap().clone()
    }

    /// Set the default timezone of new events.
    pub fn set_timezone(&self, timezone: Option<String>) {
        *self.inner.timezone.write().unwrap() = timezone;
    }

    /// Use the given mutex to prevent events from overlapping.
    pub fn use_event_mutex(&self, mutex: Arc<dyn EventMutex>) {
        *self.inner.event_mutex.write().unwrap() = mutex;
    }

    /// Use the given mutex to run events on a single server.
    pub fn use_scheduling_mutex(&self, mutex: Arc<dyn SchedulingMutex>) {
        *self.inner.scheduling_mutex.write().unwrap() = mutex;
    }

    /// Determine if the application is down for maintenance using the
    /// given callback (wired up by the foundation).
    pub fn determine_maintenance_mode_using(
        &self,
        resolver: impl Fn() -> bool + Send + Sync + 'static,
    ) {
        *self.inner.maintenance.write().unwrap() = Some(Arc::new(resolver));
    }

    fn is_down_for_maintenance(&self) -> bool {
        self.inner
            .maintenance
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|resolver| resolver())
    }

    fn push(&self, task: Task) -> Event {
        let event = Event::new(
            task,
            self.timezone(),
            self.inner.event_mutex.read().unwrap().clone(),
        );
        self.inner.events.write().unwrap().push(event.clone());
        event
    }

    /// Add a new Artisan command event to the schedule:
    /// `schedule.command("emails:send --force")`.
    pub fn command(&self, command: &str) -> Event {
        self.push(Task::Command(command.trim().to_string()))
    }

    /// Add a new Artisan command event with the given arguments.
    pub fn command_with(&self, command: &str, args: impl Into<ArtisanArgs>) -> Event {
        let args = args.into();
        let command = if args.is_empty() {
            command.trim().to_string()
        } else {
            format!("{} {}", command.trim(), args.to_command_line())
        };
        self.push(Task::Command(command))
    }

    /// Add a new callback event to the schedule.
    #[track_caller]
    pub fn call<F, Fut, R>(&self, callback: F) -> Event
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoExitCode + 'static,
    {
        let location = Location::caller();
        let callback: CallbackFn = Arc::new(move || {
            let future = callback();
            Box::pin(async move { future.await.into_exit_code() })
        });

        self.push(Task::Callback {
            callback,
            location: Some(format!("{}:{}", location.file(), location.line())),
        })
    }

    /// Add a new shell command event to the schedule.
    pub fn exec(&self, command: &str) -> Event {
        self.push(Task::Exec(command.to_string()))
    }

    /// Add a new shell command event with the given arguments.
    pub fn exec_with(&self, command: &str, args: impl Into<ArtisanArgs>) -> Event {
        let args = args.into();
        self.push(Task::Exec(
            format!("{command} {}", args.to_command_line())
                .trim()
                .to_string(),
        ))
    }

    /// Every scheduled event.
    pub fn events(&self) -> Vec<Event> {
        self.inner.events.read().unwrap().clone()
    }

    /// The events that run in any of the given environments.
    pub fn events_for_environments(&self, environments: &[String]) -> Vec<Event> {
        self.events()
            .into_iter()
            .filter(|event| {
                environments
                    .iter()
                    .any(|env| event.runs_in_environment(env))
            })
            .collect()
    }

    /// The events that are due at the given time.
    pub fn due_events(&self, now: &Carbon) -> Vec<Event> {
        let environment = current_environment();
        let down = self.is_down_for_maintenance();

        self.events()
            .into_iter()
            .filter(|event| event.is_due_in(now, &environment, down))
            .collect()
    }

    /// Determine if the server is allowed to run this event.
    pub async fn server_should_run(&self, event: &Event, time: &Carbon) -> bool {
        let name = event.mutex_name();

        if let Some(result) = self.inner.mutex_cache.lock().unwrap().get(&name) {
            return *result;
        }

        let mutex = self.inner.scheduling_mutex.read().unwrap().clone();
        let result = mutex.create(event, time).await;
        self.inner.mutex_cache.lock().unwrap().insert(name, result);
        result
    }

    // ------------------------------------------------------------------
    // Pausing and interrupting
    // ------------------------------------------------------------------

    /// Keep the schedule's pause and interrupt signals in the given store.
    /// By default, the `dyn ScheduleCache` bound in the container is used
    /// (the cache, in an application), falling back to an in-memory one.
    pub fn use_schedule_cache(&self, cache: Arc<dyn ScheduleCache>) {
        *self.inner.cache.write().unwrap() = Some(cache);
    }

    fn cache(&self) -> Arc<dyn ScheduleCache> {
        if let Some(cache) = self.inner.cache.read().unwrap().clone() {
            return cache;
        }
        match Container::get_instance().try_make::<dyn ScheduleCache>() {
            Ok(cache) => cache,
            Err(_) => self.inner.fallback_cache.clone(),
        }
    }

    /// Don't poll the cache for pause and interrupt signals.
    pub fn without_interruption_polling(&self) {
        self.inner.pausable.store(false, Ordering::SeqCst);
        self.inner.interruptible.store(false, Ordering::SeqCst);
    }

    /// Whether the schedule can be paused.
    pub fn is_pausable(&self) -> bool {
        self.inner.pausable.load(Ordering::SeqCst)
    }

    /// Whether the schedule is paused (`schedule:pause`).
    pub async fn is_paused(&self) -> bool {
        self.is_pausable() && self.cache().get(PAUSED).await.is_some_and(|value| value.truthy())
    }

    /// Pause the schedule: due tasks are skipped until it's resumed, unless
    /// they run [`even_when_paused`](Event::even_when_paused).
    pub async fn pause(&self) {
        self.cache().forever(PAUSED, Value::Bool(true)).await;
    }

    /// Resume the schedule.
    pub async fn resume(&self) {
        self.cache().forget(PAUSED).await;
    }

    /// Signal running `schedule:run` processes to stop repeating sub-minute
    /// tasks (`schedule:interrupt`).
    pub async fn interrupt(&self) {
        self.cache()
            .forever(INTERRUPT, Value::from(Carbon::now().timestamp_millis()))
            .await;
    }

    /// Whether the schedule has been interrupted since the given time.
    pub async fn has_been_interrupted_since(&self, time: &Carbon) -> bool {
        if !self.inner.interruptible.load(Ordering::SeqCst) {
            return false;
        }
        self.cache()
            .get(INTERRUPT)
            .await
            .and_then(|value| value.to_i64_lossy())
            .is_some_and(|interrupted_at| interrupted_at >= time.timestamp_millis())
    }

    /// Determine if the application is down for maintenance.
    pub fn down_for_maintenance(&self) -> bool {
        self.is_down_for_maintenance()
    }

    /// Remove every scheduled event.
    pub fn flush(&self) {
        self.inner.events.write().unwrap().clear();
        self.inner.mutex_cache.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_registers_events() {
        let schedule = Schedule::new();
        schedule.command("inspire").hourly();
        schedule
            .command_with("emails:send", ["Taylor", "--force"])
            .daily();
        schedule.exec("echo hi").weekly();
        let closure = schedule.call(|| async {}).name("Closure");

        let events = schedule.events();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].expression(), "0 * * * *");
        assert_eq!(
            events[1].get_command().as_deref(),
            Some("emails:send Taylor --force")
        );
        assert_eq!(events[2].summary_for_display(), "echo hi");
        assert!(events[3].is_callback());
        assert_eq!(closure.summary_for_display(), "Closure");
        assert!(events[3].location().unwrap().contains("schedule.rs:"));
    }

    #[test]
    fn it_finds_due_events() {
        let schedule = Schedule::new();
        schedule.command("a").daily_at("13:00");
        schedule.command("b").daily_at("14:00");
        schedule
            .command("c")
            .every_minute()
            .environments(["staging"]);

        let now = Carbon::parse("2024-03-11 13:00:00").unwrap();
        let due = schedule.due_events(&now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].get_command().as_deref(), Some("a"));

        schedule.determine_maintenance_mode_using(|| true);
        assert!(schedule.due_events(&now).is_empty());
    }

    #[test]
    fn it_applies_the_default_timezone() {
        let schedule = Schedule::with_timezone(Some("America/Chicago".to_string()));
        assert_eq!(
            schedule.command("a").get_timezone().as_deref(),
            Some("America/Chicago")
        );
    }

    #[tokio::test]
    async fn it_runs_on_one_server() {
        let schedule = Schedule::new();
        let event = schedule.command("a").on_one_server();
        let now = Carbon::parse("2024-03-11 13:00:00").unwrap();
        assert!(schedule.server_should_run(&event, &now).await);
        // The result is cached for the rest of the run...
        assert!(schedule.server_should_run(&event, &now).await);

        let other = Schedule::new();
        other.use_scheduling_mutex(schedule.inner.scheduling_mutex.read().unwrap().clone());
        assert!(!other.server_should_run(&event, &now).await);
    }
}
