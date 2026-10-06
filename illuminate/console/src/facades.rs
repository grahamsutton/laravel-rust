//! The `Artisan` and `Schedule` facades.

use std::future::Future;
use std::sync::Arc;

use illuminate_container::Container;
use illuminate_support::Result;
use indexmap::IndexMap;

use crate::application::Application;
use crate::command::{ClosureCommand, Command};
use crate::console::Console;
use crate::input::ArtisanArgs;
use crate::output::Output;
use crate::scheduling::{self, Event, IntoExitCode};

/// Build the default console application, with the framework's commands.
pub(crate) fn default_application() -> Arc<Application> {
    let application = Application::new();
    scheduling::register_commands(&application);
    application
}

/// Build the default schedule, reading its timezone from the configuration.
pub(crate) fn default_schedule() -> scheduling::Schedule {
    let timezone =
        illuminate_container::try_app::<illuminate_config::Repository>().and_then(|config| {
            let timezone = config.get("app.schedule_timezone");
            let timezone = if timezone.is_null() {
                config.get("app.timezone")
            } else {
                timezone
            };
            timezone
                .as_str()
                .filter(|tz| !tz.is_empty())
                .map(String::from)
        });

    scheduling::Schedule::with_timezone(timezone)
}

/// The Artisan facade: register, call and run console commands.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_console::Artisan;
/// use illuminate_container::Container;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Artisan::command("mail:send {user}", |cmd| async move {
///     cmd.info(format!("Sending email to: {}!", cmd.argument("user").unwrap()));
///     Ok(())
/// })
/// .purpose("Send a marketing email to a user");
///
/// let status = Artisan::call("mail:send 1", ()).await.unwrap();
///
/// assert_eq!(status, 0);
/// assert_eq!(Artisan::output(), "Sending email to: 1!\n");
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Artisan;

impl Artisan {
    /// The console application bound in the container (registering the
    /// default one if needed).
    pub fn application() -> Arc<Application> {
        let container = Container::get_instance();

        if let Ok(application) = container.try_make::<Application>() {
            return application;
        }

        container.singleton_if::<Application>(|_| default_application());
        container.make::<Application>()
    }

    /// Register a closure based command.
    pub fn command<F, Fut>(signature: impl Into<String>, handler: F) -> ClosureCommand
    where
        F: Fn(Console) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self::application().command(signature, handler)
    }

    /// Register a command.
    pub fn register<C: Command>(command: C) {
        Self::application().add(command);
    }

    /// Register a shared command.
    pub fn register_arc(command: Arc<dyn Command>) {
        Self::application().add_arc(command);
    }

    /// Call a command by name (or full command line), capturing its output.
    pub async fn call(command: &str, args: impl Into<ArtisanArgs>) -> Result<i32> {
        Self::application().call(command, args).await
    }

    /// Call a command, writing to the given output.
    pub async fn call_with_output(
        command: &str,
        args: impl Into<ArtisanArgs>,
        output: &Output,
    ) -> Result<i32> {
        Self::application()
            .call_with_output(command, args, output)
            .await
    }

    /// The output of the most recent call.
    pub fn output() -> String {
        Self::application().output()
    }

    /// Every registered command.
    pub fn all() -> IndexMap<String, Arc<dyn Command>> {
        Self::application().all()
    }

    /// Determine if a command exists.
    pub fn has(name: &str) -> bool {
        Self::application().has(name)
    }

    /// Run Artisan with the process' arguments, returning the exit code.
    pub async fn run<I, S>(argv: I) -> i32
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::application().run(argv).await
    }

    /// Define scheduled tasks (Laravel's `withSchedule`).
    pub fn schedule(callback: impl FnOnce(&scheduling::Schedule)) {
        callback(&Schedule::instance());
    }
}

/// The Schedule facade: define your application's scheduled tasks.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_console::Schedule;
/// use illuminate_container::Container;
///
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container);
///
/// Schedule::command("inspire").hourly();
/// Schedule::call(|| async { /* ... */ }).daily();
///
/// assert_eq!(Schedule::events().len(), 2);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Schedule;

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

    /// The schedule bound in the container (registering one if needed).
    pub fn instance() -> scheduling::Schedule {
        let container = Container::get_instance();

        if let Ok(schedule) = container.try_make::<scheduling::Schedule>() {
            return (*schedule).clone();
        }

        container.singleton_if::<scheduling::Schedule>(|_| Arc::new(default_schedule()));
        (*container.make::<scheduling::Schedule>()).clone()
    }

    /// Schedule an Artisan command: `Schedule::command("emails:send --force")`.
    pub fn command(command: &str) -> Event {
        Self::instance().command(command)
    }

    /// Schedule an Artisan command with the given arguments.
    pub fn command_with(command: &str, args: impl Into<ArtisanArgs>) -> Event {
        Self::instance().command_with(command, args)
    }

    /// Schedule a closure.
    #[track_caller]
    pub fn call<F, Fut, R>(callback: F) -> Event
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoExitCode + 'static,
    {
        Self::instance().call(callback)
    }

    /// Schedule a shell command.
    pub fn exec(command: &str) -> Event {
        Self::instance().exec(command)
    }

    /// Every scheduled event.
    pub fn events() -> Vec<Event> {
        Self::instance().events()
    }
}
