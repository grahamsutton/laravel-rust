//! The console side of the foundation: the Artisan kernel and the
//! framework's commands.

pub mod commands;
pub mod generators;

use std::sync::{Arc, Mutex};

use illuminate_console::{Artisan, Command, scheduling::Schedule};
use illuminate_http::ExceptionHandler;

use crate::application::{Application, VERSION};
use crate::configuration::Routing;
use crate::exceptions::Handler;

type ScheduleCallback = Arc<dyn Fn(&Schedule) + Send + Sync>;

/// Commands and schedules registered through the application builder.
#[derive(Default)]
pub struct ConsoleConfiguration {
    pub(crate) commands: Mutex<Vec<Arc<dyn Command>>>,
    pub(crate) schedules: Mutex<Vec<ScheduleCallback>>,
}

impl ConsoleConfiguration {
    /// Add commands to register with Artisan.
    pub fn add_commands(&self, commands: impl IntoIterator<Item = Arc<dyn Command>>) {
        self.commands.lock().unwrap().extend(commands);
    }

    /// Add a schedule definition callback.
    pub fn add_schedule(&self, callback: ScheduleCallback) {
        self.schedules.lock().unwrap().push(callback);
    }
}

/// Register the framework's own Artisan commands.
pub fn register_framework_commands() {
    use commands::*;

    Artisan::register(AboutCommand);
    Artisan::register(CacheClearCommand);
    Artisan::register(ConfigShowCommand);
    Artisan::register(DownCommand);
    Artisan::register(EnvironmentCommand);
    Artisan::register(InspireCommand);
    Artisan::register(KeyGenerateCommand);
    Artisan::register(RouteListCommand);
    Artisan::register(ServeCommand);
    Artisan::register(StorageLinkCommand);
    Artisan::register(UpCommand);

    Artisan::register(MigrateCommand::default());
    Artisan::register(MigrateFreshCommand::default());
    Artisan::register(MigrateInstallCommand);
    Artisan::register(MigrateRefreshCommand::default());
    Artisan::register(MigrateResetCommand::default());
    Artisan::register(MigrateRollbackCommand::default());
    Artisan::register(MigrateStatusCommand);
    Artisan::register(SeedCommand);
    Artisan::register(WipeCommand);

    Artisan::register(QueueClearCommand);
    Artisan::register(QueueFailedCommand);
    Artisan::register(QueueFlushCommand);
    Artisan::register(QueueForgetCommand);
    Artisan::register(QueueMonitorCommand);
    Artisan::register(QueuePauseCommand);
    Artisan::register(QueuePruneFailedCommand);
    Artisan::register(QueueRestartCommand);
    Artisan::register(QueueResumeCommand);
    Artisan::register(QueueRetryCommand);
    Artisan::register(QueueWorkCommand);

    Artisan::register(generators::migration::MakeMigrationCommand);
    for generator in generators::commands::all() {
        Artisan::register(generator);
    }

    for command in crate::providers::extra_framework_commands() {
        Artisan::register_arc(command);
    }
}

impl Application {
    /// Prepare Artisan: register commands, load `routes/console.rs`, and
    /// define the schedule. Safe to call more than once.
    pub fn bootstrap_console(self: &Arc<Self>) {
        self.bootstrap();
        if self.bound::<ConsoleBootstrapped>() {
            return;
        }
        self.instance(ConsoleBootstrapped);

        let artisan = Artisan::application();
        artisan.set_name("Laravel Framework");
        artisan.set_version(VERSION);
        artisan.set_binary("artisan");
        if let Ok(handler) = self.try_make::<Handler>() {
            artisan.report_exceptions_using(move |error| {
                if handler.should_report(error) {
                    handler.report(error);
                }
            });
        }

        register_framework_commands();

        if let Ok(configuration) = self.try_make::<ConsoleConfiguration>() {
            for command in configuration.commands.lock().unwrap().iter() {
                Artisan::register_arc(command.clone());
            }
            let schedule = illuminate_console::Schedule::instance();
            for callback in configuration.schedules.lock().unwrap().iter() {
                callback(&schedule);
            }
        }

        if let Ok(routing) = self.try_make::<Routing>() {
            for commands in &routing.commands {
                commands();
            }
        }
    }

    /// Handle the Artisan command line (`cargo artisan ...`), returning the
    /// exit code.
    pub async fn handle_command(self: &Arc<Self>) -> i32 {
        self.handle_command_with(std::env::args()).await
    }

    /// Run Artisan with the given arguments (the first is the binary name).
    pub async fn handle_command_with<I, S>(self: &Arc<Self>, argv: I) -> i32
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.set_running_in_console(true);
        self.bootstrap_console();
        let argv: Vec<String> = argv.into_iter().map(Into::into).collect();
        let code = Artisan::run(argv).await;
        illuminate_queue::DeferredCallbacks::current().invoke().await;
        self.terminate();
        code
    }
}

/// Marks that the console has been bootstrapped.
struct ConsoleBootstrapped;
