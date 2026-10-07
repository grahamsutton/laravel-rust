//! Events the scheduler dispatches as it runs tasks.

use std::sync::Arc;

use illuminate_support::Error;

use super::event::Event;

/// A scheduled task is about to run.
#[derive(Clone, Debug)]
pub struct ScheduledTaskStarting {
    /// The task.
    pub task: Event,
}

/// A scheduled task finished running.
#[derive(Clone, Debug)]
pub struct ScheduledTaskFinished {
    /// The task.
    pub task: Event,
    /// How long it ran, in seconds.
    pub runtime: f64,
}

/// A scheduled task running in the background finished.
#[derive(Clone, Debug)]
pub struct ScheduledBackgroundTaskFinished {
    /// The task.
    pub task: Event,
}

/// A scheduled task failed.
#[derive(Clone, Debug)]
pub struct ScheduledTaskFailed {
    /// The task.
    pub task: Event,
    /// Why it failed.
    pub exception: Arc<Error>,
}

/// A due task was skipped: its filters didn't pass, or the schedule is
/// paused.
#[derive(Clone, Debug)]
pub struct ScheduledTaskSkipped {
    /// The task.
    pub task: Event,
}

/// The schedule was paused (`schedule:pause`).
#[derive(Clone, Copy, Debug)]
pub struct SchedulePaused;

/// The schedule was resumed (`schedule:resume`).
#[derive(Clone, Copy, Debug)]
pub struct ScheduleResumed;

/// Dispatch a scheduler event, reporting listener errors.
pub(crate) async fn dispatch<E: Send + Sync + 'static>(event: E) {
    if let Err(error) = illuminate_events::Event::dispatch(event).await {
        crate::facades::Artisan::application().report(&error);
    }
}
