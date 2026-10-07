//! Queue commands: `queue:work`, `queue:failed`, `queue:retry`, and friends.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use illuminate_console::{Command, Console, async_trait};
use illuminate_queue::events::{JobFailed, JobProcessed, JobProcessing, JobReleasedAfterException};
use illuminate_queue::{Queue, QueuedJob, Worker, WorkerOptions};
use illuminate_support::{Carbon, Result};

use crate::application::Application;

/// The connection name and queue(s) a command works with.
fn connection_and_queue(cmd: &Console) -> (String, String) {
    let manager = Queue::manager();
    let connection = cmd.argument("connection").unwrap_or_else(|| manager.get_default_driver());
    let queue = cmd.option("queue").filter(|q| !q.is_empty()).unwrap_or_else(|| {
        Application::current()
            .config_repository()
            .string_or(&format!("queue.connections.{connection}.queue"), "default")
    });
    (connection, queue)
}

fn seconds(cmd: &Console, option: &str, default: u64) -> u64 {
    cmd.option(option).and_then(|value| value.parse().ok()).unwrap_or(default)
}

fn run_time(started: Instant) -> String {
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if ms < 1000.0 { format!("{ms:.2}ms") } else { format!("{:.2}s", ms / 1000.0) }
}

/// `queue:work` — Start processing jobs on the queue as a daemon.
pub struct WorkCommand;

#[async_trait]
impl Command for WorkCommand {
    fn signature(&self) -> &str {
        "queue:work
            {connection? : The name of the queue connection to work}
            {--name=default : The name of the worker}
            {--queue= : The names of the queues to work}
            {--once : Only process the next job on the queue}
            {--stop-when-empty : Stop when the queue is empty}
            {--backoff=0 : The number of seconds to wait before retrying a job that encountered an uncaught exception}
            {--max-jobs=0 : The number of jobs to process before stopping}
            {--max-time=0 : The maximum number of seconds the worker should run}
            {--force : Force the worker to run even in maintenance mode}
            {--memory=128 : The memory limit in megabytes}
            {--sleep=3 : Number of seconds to sleep when no job is available}
            {--rest=0 : Number of seconds to rest between jobs}
            {--timeout=60 : The number of seconds a child process can run}
            {--tries=1 : Number of times to attempt a job before logging it failed}"
    }

    fn description(&self) -> &str {
        "Start processing jobs on the queue as a daemon"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let (connection, queue) = connection_and_queue(&cmd);

        let options = WorkerOptions {
            name: cmd.option("name").unwrap_or_else(|| "default".into()),
            backoff: cmd
                .option("backoff")
                .unwrap_or_else(|| "0".into())
                .split(',')
                .filter_map(|value| value.trim().parse().ok())
                .collect(),
            memory: seconds(&cmd, "memory", 128),
            timeout: Duration::from_secs(seconds(&cmd, "timeout", 60)),
            sleep: Duration::from_secs(seconds(&cmd, "sleep", 3)),
            max_tries: seconds(&cmd, "tries", 1) as u32,
            force: cmd.option_bool("force"),
            stop_when_empty: cmd.option_bool("stop-when-empty"),
            max_jobs: seconds(&cmd, "max-jobs", 0),
            max_time: Duration::from_secs(seconds(&cmd, "max-time", 0)),
            rest: Duration::from_secs(seconds(&cmd, "rest", 0)),
            ..WorkerOptions::default()
        };

        listen_for_events(&cmd);

        let app = Application::current();
        let worker = Worker::make()
            .with_name(options.name.clone())
            .down_for_maintenance_using(move || app.is_down_for_maintenance());

        if cmd.option_bool("once") {
            return worker.run_next_job(&connection, &queue, &options).await;
        }

        cmd.components()
            .info(format!("Processing jobs from the [{queue}] {}.", if queue.contains(',') { "queues" } else { "queue" }));

        worker.listen_for_signals(&connection, &queue);
        let reason = worker.daemon(&connection, &queue, &options).await?;
        match reason.exit_code() {
            0 => Ok(()),
            code => cmd.exit(code),
        }
    }
}

/// Print a line per job: `  2024-01-01 12:00:00 App\Jobs\ProcessPodcast ..... 15.21ms DONE`.
fn listen_for_events(cmd: &Console) {
    let output = cmd.output().clone();
    let started: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

    let write_start = {
        let output = output.clone();
        let started = started.clone();
        move |job: &QueuedJob| {
            *started.lock().unwrap() = Some(Instant::now());
            output.write(format!(
                "  <fg=gray>{}</> {}",
                Carbon::now().format("Y-m-d H:i:s"),
                job.resolve_name()
            ));
        }
    };
    let write_end = {
        let output = output.clone();
        let started = started.clone();
        move |job: &QueuedJob, status: &str| {
            let run_time = started.lock().unwrap().take().map(run_time).unwrap_or_default();
            let dots = output.width().min(150).saturating_sub(job.resolve_name().chars().count() + run_time.chars().count() + 31);
            output.writeln(format!(" <fg=gray>{}</> <fg=gray>{run_time}</> {status}", ".".repeat(dots)));
        }
    };

    Queue::listen::<JobProcessing>(move |event| write_start(&event.job));
    let end = Arc::new(write_end);
    {
        let end = end.clone();
        Queue::listen::<JobProcessed>(move |event| end(&event.job, "<fg=green;options=bold>DONE</>"));
    }
    {
        let end = end.clone();
        Queue::listen::<JobReleasedAfterException>(move |event| end(&event.job, "<fg=yellow;options=bold>FAIL</>"));
    }
    Queue::listen::<JobFailed>(move |event| end(&event.job, "<fg=red;options=bold>FAIL</>"));
}

/// `queue:failed` — List all of the failed queue jobs.
pub struct ListFailedCommand;

#[async_trait]
impl Command for ListFailedCommand {
    fn signature(&self) -> &str {
        "queue:failed"
    }

    fn description(&self) -> &str {
        "List all of the failed queue jobs"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let jobs = Queue::failed_jobs().await?;
        if jobs.is_empty() {
            cmd.components().info("No failed jobs found.");
            return Ok(());
        }
        cmd.new_line(1);
        for job in jobs {
            cmd.components().two_column_detail(
                format!("<fg=gray>{}</> {}", job.failed_at.format("Y-m-d H:i:s"), job.id),
                format!("{}@{} {}", job.connection, job.queue, job.display_name()),
            );
        }
        cmd.new_line(1);
        Ok(())
    }
}

/// `queue:retry` — Retry a failed queue job.
pub struct RetryCommand;

#[async_trait]
impl Command for RetryCommand {
    fn signature(&self) -> &str {
        "queue:retry
            {id?* : The ID of the failed job or \"all\" to retry all jobs}
            {--queue= : Retry all of the failed jobs for the specified queue}"
    }

    fn description(&self) -> &str {
        "Retry a failed queue job"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let ids = cmd.argument_list("id");
        let queue = cmd.option("queue");

        let ids: Vec<String> = if ids.iter().any(|id| id == "all") || (ids.is_empty() && queue.is_some()) {
            Queue::failed_jobs()
                .await?
                .into_iter()
                .filter(|job| queue.as_ref().is_none_or(|queue| &job.queue == queue))
                .map(|job| job.id)
                .collect()
        } else {
            ids
        };

        if ids.is_empty() {
            cmd.components().info("No retryable jobs found.");
            return Ok(());
        }

        cmd.components().info("Pushing failed queue jobs back onto the queue.");
        for id in ids {
            let mut found = true;
            cmd.components()
                .task(&id, || async {
                    found = Queue::retry_failed(&id).await?;
                    Ok(found)
                })
                .await?;
            if !found {
                cmd.components().error(format!("Unable to find failed job with ID [{id}]."));
            }
        }
        cmd.new_line(1);
        Ok(())
    }
}

/// `queue:forget` — Delete a failed queue job.
pub struct ForgetFailedCommand;

#[async_trait]
impl Command for ForgetFailedCommand {
    fn signature(&self) -> &str {
        "queue:forget {id : The ID of the failed job}"
    }

    fn description(&self) -> &str {
        "Delete a failed queue job"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let id = cmd.argument("id").unwrap_or_default();
        if Queue::forget_failed(&id).await? {
            cmd.components().info("Failed job deleted successfully.");
            Ok(())
        } else {
            cmd.components().error(format!("No failed job matches the given ID [{id}]."));
            cmd.exit(1)
        }
    }
}

/// `queue:flush` — Flush all of the failed queue jobs.
pub struct FlushFailedCommand;

#[async_trait]
impl Command for FlushFailedCommand {
    fn signature(&self) -> &str {
        "queue:flush {--hours= : The number of hours to retain failed job data}"
    }

    fn description(&self) -> &str {
        "Flush all of the failed queue jobs"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let hours = cmd.option("hours").and_then(|hours| hours.parse().ok());
        Queue::flush_failed(hours).await?;
        match hours {
            Some(hours) => cmd
                .components()
                .info(format!("All jobs that failed more than {hours} hours ago have been deleted successfully.")),
            None => cmd.components().info("All failed jobs deleted successfully."),
        }
        Ok(())
    }
}

/// `queue:prune-failed` — Prune stale entries from the failed jobs table.
pub struct PruneFailedCommand;

#[async_trait]
impl Command for PruneFailedCommand {
    fn signature(&self) -> &str {
        "queue:prune-failed {--hours=24 : The number of hours to retain failed jobs data}"
    }

    fn description(&self) -> &str {
        "Prune stale entries from the failed jobs table"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let hours = seconds(&cmd, "hours", 24) as i64;
        let count = Queue::prune_failed(Carbon::now().sub_hours(hours)).await?;
        cmd.components().info(format!("{count} entries deleted."));
        Ok(())
    }
}

/// `queue:clear` — Delete all of the jobs from the specified queue.
pub struct ClearCommand;

#[async_trait]
impl Command for ClearCommand {
    fn signature(&self) -> &str {
        "queue:clear
            {connection? : The name of the queue connection to clear}
            {--queue= : The name of the queue to clear}
            {--force : Force the operation to run when in production}"
    }

    fn description(&self) -> &str {
        "Delete all of the jobs from the specified queue"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        if Application::current().is_production() && !cmd.option_bool("force") {
            cmd.components().warn("Application In Production.");
            if !cmd.confirm("Are you sure you want to run this command?", false) {
                cmd.components().warn("Command cancelled.");
                return cmd.exit(1);
            }
        }
        let (connection, queue) = connection_and_queue(&cmd);
        let count = Queue::connection(&connection)?.clear(Some(&queue)).await?;
        let noun = if count == 1 { "job" } else { "jobs" };
        cmd.components()
            .info(format!("Cleared {count} {noun} from the [{queue}] queue."));
        Ok(())
    }
}

/// `queue:restart` — Restart queue worker daemons after their current job.
pub struct RestartCommand;

#[async_trait]
impl Command for RestartCommand {
    fn signature(&self) -> &str {
        "queue:restart"
    }

    fn description(&self) -> &str {
        "Restart queue worker daemons after their current job"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        Queue::restart().await?;
        cmd.components().info("Broadcasting queue restart signal.");
        Ok(())
    }
}

/// Parse `connection:queue` (or just `queue` on the default connection).
fn parse_queue(value: &str) -> (String, String) {
    match value.split_once(':') {
        Some((connection, queue)) => (connection.to_string(), queue.to_string()),
        None => (Queue::manager().get_default_driver(), value.to_string()),
    }
}

/// `queue:pause` — Pause job processing for a specific queue.
pub struct PauseCommand;

#[async_trait]
impl Command for PauseCommand {
    fn signature(&self) -> &str {
        "queue:pause {queue : The name of the queue that should be paused}"
    }

    fn description(&self) -> &str {
        "Pause job processing for a specific queue"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let (connection, queue) = parse_queue(&cmd.argument("queue").unwrap_or_default());
        Queue::pause(&connection, &queue).await?;
        cmd.components()
            .info(format!("Job processing on queue [{connection}:{queue}] has been paused."));
        Ok(())
    }
}

/// `queue:resume` — Resume job processing for a paused queue.
pub struct ResumeCommand;

#[async_trait]
impl Command for ResumeCommand {
    fn signature(&self) -> &str {
        "queue:resume {queue : The name of the queue that should resume processing}"
    }

    fn description(&self) -> &str {
        "Resume job processing for a paused queue"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let (connection, queue) = parse_queue(&cmd.argument("queue").unwrap_or_default());
        Queue::resume(&connection, &queue).await?;
        cmd.components()
            .info(format!("Job processing on queue [{connection}:{queue}] has been resumed."));
        Ok(())
    }
}

/// `queue:monitor` — Monitor the size of the specified queues.
pub struct MonitorCommand;

#[async_trait]
impl Command for MonitorCommand {
    fn signature(&self) -> &str {
        "queue:monitor
            {queues : The names of the queues to monitor}
            {--max=1000 : The maximum number of jobs that can be on the queue before an event is dispatched}"
    }

    fn description(&self) -> &str {
        "Monitor the size of the specified queues"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let max = seconds(&cmd, "max", 1000);
        let mut rows = Vec::new();
        for value in cmd.argument("queues").unwrap_or_default().split(',') {
            let (connection, queue) = parse_queue(value.trim());
            let size = Queue::connection(&connection)?.size(Some(&queue)).await?;
            let status = if size >= max { "<fg=yellow;options=bold>ALERT</>" } else { "<fg=green;options=bold>OK</>" };
            rows.push((format!("[{connection}] {queue}"), format!("[{size}] {status}")));
        }
        cmd.new_line(1);
        cmd.components()
            .two_column_detail("<fg=gray>Queue name</>", "<fg=gray>Size / Status</>");
        for (name, status) in rows {
            cmd.components().two_column_detail(name, status);
        }
        cmd.new_line(1);
        Ok(())
    }
}
