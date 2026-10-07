//! Task scheduling (`Illuminate\Console\Scheduling`).
//!
//! Define your schedule with the [`Schedule`](crate::Schedule) facade, and
//! run `artisan schedule:run` every minute (or `artisan schedule:work`):
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_console::Schedule;
//! use illuminate_container::Container;
//!
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container);
//!
//! Schedule::command("emails:send Taylor --force").daily();
//! Schedule::command("report:generate").timezone("America/New_York").at("2:00");
//! Schedule::call(|| async { /* delete recent users... */ }).daily().without_overlapping();
//!
//! assert_eq!(Schedule::events()[1].expression(), "0 2 * * *");
//! ```

pub mod cache;
pub mod commands;
pub mod cron;
pub mod event;
pub mod events;
pub mod mutex;
pub mod schedule;

pub use cache::{InMemoryScheduleCache, ScheduleCache};
pub use commands::{
    ScheduleClearCacheCommand, ScheduleInterruptCommand, ScheduleListCommand, SchedulePauseCommand,
    ScheduleResumeCommand, ScheduleRunCommand, ScheduleTestCommand, ScheduleWorkCommand,
    register_commands,
};
pub use cron::CronExpression;
pub use event::{Event, IntoExitCode, ScheduleOutputMailer};
pub use mutex::{EventMutex, InMemoryEventMutex, InMemorySchedulingMutex, SchedulingMutex};
pub use schedule::Schedule;
