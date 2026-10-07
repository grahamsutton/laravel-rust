//! The scheduler's Artisan commands: `schedule:run`, `schedule:list`,
//! `schedule:work`, `schedule:test`, `schedule:interrupt`,
//! `schedule:pause`, `schedule:resume`, and `schedule:clear-cache`.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_trait::async_trait;
use illuminate_support::{Carbon, Result, Sleep, error::error, json};
use regex::Regex;

use super::event::Event;
use super::events::{
    self as schedule_events, SchedulePaused, ScheduleResumed, ScheduledTaskFailed,
    ScheduledTaskFinished, ScheduledTaskSkipped, ScheduledTaskStarting,
};
use crate::application::Application;
use crate::command::Command;
use crate::console::Console;
use crate::facades::Schedule;
use crate::formatter::OutputFormatter;
use crate::input::ArtisanArgs;
use crate::output::{Output, Verbosity};

/// Register the scheduler's commands with the application.
pub fn register_commands(application: &Application) {
    application.add(ScheduleRunCommand);
    application.add(ScheduleListCommand);
    application.add(ScheduleWorkCommand);
    application.add(ScheduleTestCommand);
    application.add(ScheduleInterruptCommand);
    application.add(SchedulePauseCommand);
    application.add(ScheduleResumeCommand);
    application.add(ScheduleClearCacheCommand);
}

async fn run_event(cmd: &Console, event: &Event, foreground: bool) -> Result<()> {
    let application = cmd.application();
    let background = event.runs_in_background() && !foreground;
    let command = event.command_for_display();
    let summary = if event.is_callback() {
        event.summary_for_display()
    } else {
        command.clone()
    };

    let description = format!(
        "<fg=gray>{}</> Running [{}]{}",
        Carbon::now().format("Y-m-d H:i:s"),
        OutputFormatter::escape(&summary),
        if background { " in background" } else { "" },
    );

    let task_event = event.clone();
    let task_application = application.clone();

    cmd.components()
        .task(description, || async move {
            schedule_events::dispatch(ScheduledTaskStarting { task: task_event.clone() }).await;
            let start = std::time::Instant::now();

            let failure = match task_event.run_with(&task_application, foreground).await {
                Ok(()) => {
                    schedule_events::dispatch(ScheduledTaskFinished {
                        task: task_event.clone(),
                        runtime: (start.elapsed().as_secs_f64() * 100.0).round() / 100.0,
                    })
                    .await;
                    let code = task_event.exit_code();
                    if !background && code.is_some_and(|code| code != 0) {
                        Some(error!(
                            "Scheduled command [{}] failed with exit code [{}].",
                            task_event
                                .get_command()
                                .unwrap_or_else(|| task_event.summary_for_display()),
                            code.unwrap_or_default()
                        ))
                    } else {
                        None
                    }
                }
                Err(error) => Some(error),
            };

            if let Some(error) = failure {
                let error = Arc::new(error);
                schedule_events::dispatch(ScheduledTaskFailed {
                    task: task_event.clone(),
                    exception: error.clone(),
                })
                .await;
                task_application.report(&error);
            }

            Ok(background || task_event.exit_code().is_none_or(|code| code == 0))
        })
        .await?;

    if !event.is_callback() {
        cmd.components().bullet_list([event.summary_for_display()]);
    }

    Ok(())
}

/// `schedule:run`: run the scheduled commands that are due.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScheduleRunCommand;

#[async_trait]
impl Command for ScheduleRunCommand {
    fn signature(&self) -> &str {
        "schedule:run {--whisper : Do not output message indicating that no jobs were ready to run}"
    }

    fn description(&self) -> &str {
        "Run the scheduled commands"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let schedule = Schedule::instance();
        let started_at = Carbon::now();
        let mut events_ran = false;

        let events = schedule.due_events(&started_at);
        let paused = schedule.is_paused().await;

        for event in &events {
            if paused && !event.runs_when_paused() {
                schedule_events::dispatch(ScheduledTaskSkipped { task: event.clone() }).await;
                continue;
            }

            if !event.filters_pass(&started_at).await {
                schedule_events::dispatch(ScheduledTaskSkipped { task: event.clone() }).await;
                continue;
            }

            if !events_ran {
                cmd.new_line(1);
            }

            run_due_event(&cmd, &schedule, event, &started_at).await?;
            events_ran = true;
        }

        let repeatable: Vec<Event> = events.into_iter().filter(Event::is_repeatable).collect();
        if !repeatable.is_empty() {
            events_ran |= repeat_events(&cmd, &schedule, &repeatable, &started_at, events_ran).await?;
        }

        if !events_ran {
            if !cmd.option_bool("whisper") {
                cmd.components()
                    .info("No scheduled commands are ready to run.");
            }
        } else {
            cmd.new_line(1);
        }

        Ok(())
    }
}

/// Run a due event, on this server only if it must run on one server.
async fn run_due_event(
    cmd: &Console,
    schedule: &super::schedule::Schedule,
    event: &Event,
    started_at: &Carbon,
) -> Result<()> {
    if event.runs_on_one_server() {
        if schedule.server_should_run(event, started_at).await {
            run_event(cmd, event, false).await?;
        } else {
            cmd.components().info(format!(
                "Skipping [{}] because the command already ran on another server.",
                event.summary_for_display()
            ));
        }
        return Ok(());
    }
    run_event(cmd, event, false).await
}

/// Keep running sub-minute events until the end of the minute (or until
/// `schedule:interrupt`), returning whether any ran.
async fn repeat_events(
    cmd: &Console,
    schedule: &super::schedule::Schedule,
    events: &[Event],
    started_at: &Carbon,
    mut events_ran: bool,
) -> Result<bool> {
    let mut entered_maintenance_mode = false;
    let end_of_minute = (*started_at).end_of_minute();
    let mut ran = false;

    while Carbon::now() <= end_of_minute {
        let paused = schedule.is_paused().await;

        for event in events {
            if schedule.has_been_interrupted_since(started_at).await {
                return Ok(ran);
            }

            if !event.should_repeat_now() {
                continue;
            }

            if Carbon::now() > end_of_minute {
                return Ok(ran);
            }

            entered_maintenance_mode = entered_maintenance_mode || schedule.down_for_maintenance();
            if entered_maintenance_mode && !event.runs_in_maintenance_mode() {
                continue;
            }

            if paused && !event.runs_when_paused() {
                schedule_events::dispatch(ScheduledTaskSkipped { task: event.clone() }).await;
                continue;
            }

            if !event.filters_pass(&Carbon::now()).await {
                schedule_events::dispatch(ScheduledTaskSkipped { task: event.clone() }).await;
                continue;
            }

            if !events_ran {
                cmd.new_line(1);
                events_ran = true;
            }

            run_due_event(cmd, schedule, event, started_at).await?;
            ran = true;
        }

        Sleep::usleep(100_000).await;
    }

    Ok(ran)
}

/// `schedule:list`: list all scheduled tasks.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScheduleListCommand;

static ARTISAN_ARGUMENTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(artisan [\w\-:]+) (.+)").unwrap());

fn display_timezone(cmd: &Console) -> String {
    cmd.option("timezone").unwrap_or_else(|| {
        illuminate_container::try_app::<illuminate_config::Repository>()
            .map(|config| config.string_or("app.timezone", "UTC"))
            .unwrap_or_else(|| "UTC".to_string())
    })
}

fn display_command(event: &Event) -> String {
    if event.is_callback() {
        match event.get_description() {
            Some(description) => description,
            None => format!("Closure at: {}", event.location().unwrap_or_default()),
        }
    } else {
        event.command_for_display()
    }
}

fn next_due_date(event: &Event, timezone: &str) -> Option<Carbon> {
    let next = event.next_run_date(&Carbon::now()).ok()?;
    Some(next.tz(timezone).unwrap_or(next))
}

#[async_trait]
impl Command for ScheduleListCommand {
    fn signature(&self) -> &str {
        "schedule:list
            {--timezone= : The timezone that times should be displayed in}
            {--environment=* : Display the tasks scheduled to run on this environment}
            {--next : Sort the listed tasks by their next due date}
            {--json : Output the scheduled tasks as JSON}"
    }

    fn description(&self) -> &str {
        "List all scheduled tasks"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let schedule = Schedule::instance();
        let environments = cmd.option_list("environment");

        let mut events = if environments.is_empty() {
            schedule.events()
        } else {
            schedule.events_for_environments(&environments)
        };

        if events.is_empty() {
            if cmd.option_bool("json") {
                cmd.output().writeln("[]");
            } else {
                cmd.components()
                    .info("No scheduled tasks have been defined.");
            }
            return Ok(());
        }

        let timezone = display_timezone(&cmd);

        if cmd.option_bool("next") {
            events
                .sort_by_key(|event| next_due_date(event, &timezone).map(|date| date.timestamp()));
        }

        if cmd.option_bool("json") {
            let mut rows = Vec::new();
            for event in &events {
                let next = next_due_date(event, &timezone);
                rows.push(json!({
                    "expression": event.expression(),
                    "command": display_command(event),
                    "description": event.get_description(),
                    "next_due_date": next.map(|date| date.format("Y-m-d H:i:s P")),
                    "next_due_date_human": next.map(|date| date.diff_for_humans()),
                    "timezone": timezone,
                    "has_mutex": event.has_mutex().await,
                    "repeat_seconds": event.repeat_seconds(),
                    "environments": event.get_environments(),
                    "on_one_server": event.runs_on_one_server(),
                }));
            }
            cmd.output().writeln(serde_json::to_string(&rows)?);
            return Ok(());
        }

        let terminal_width = cmd.output().width();

        let expressions: Vec<Vec<String>> = events
            .iter()
            .map(|event| {
                event
                    .expression()
                    .split_whitespace()
                    .map(String::from)
                    .collect()
            })
            .collect();
        let spacing: Vec<usize> = (0..5)
            .map(|index| {
                expressions
                    .iter()
                    .filter_map(|fields| fields.get(index))
                    .map(|field| field.chars().count())
                    .max()
                    .unwrap_or(1)
            })
            .collect();

        let mut lines = vec![String::new()];

        for (event, fields) in events.iter().zip(&expressions) {
            let repeat = event
                .repeat_seconds()
                .map(|seconds| format!("{seconds}s "))
                .unwrap_or_default();
            let expression = spacing
                .iter()
                .enumerate()
                .map(|(index, width)| {
                    format!(
                        "{:<width$}",
                        fields.get(index).map(String::as_str).unwrap_or("*")
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            let expression = format!("{repeat}{expression}");

            let command = display_command(event);
            let command = if command.chars().count() > 1 {
                format!("{command} ")
            } else {
                String::new()
            };

            let label = "Next Due:";
            let next = next_due_date(event, &timezone);
            let next = match next {
                Some(date) if cmd.is_verbose() => date.format("Y-m-d H:i:s P"),
                Some(date) => date.diff_for_humans(),
                None => "Never".to_string(),
            };

            let has_mutex = if event.has_mutex().await {
                "Has Mutex › "
            } else {
                ""
            };

            let used = format!("{expression}{command}{label}{next}{has_mutex}")
                .chars()
                .count();
            let dots = ".".repeat(terminal_width.saturating_sub(used + 8));

            let command = OutputFormatter::escape(&command);
            let command = ARTISAN_ARGUMENTS
                .replace(&command, "$1 <fg=yellow;options=bold>$2</>")
                .into_owned();

            lines.push(format!(
                "  <fg=yellow>{expression}</> <fg=#6C7280></> {command}<fg=#6C7280>{dots} {has_mutex}{label} {next}</>"
            ));

            if cmd.is_verbose()
                && let Some(description) = event.get_description().filter(|d| d.chars().count() > 1)
            {
                lines.push(format!(
                    "  <fg=#6C7280>{}⇁ {}</>",
                    " ".repeat(expression.chars().count() + 2),
                    OutputFormatter::escape(&description)
                ));
            }
        }

        lines.push(String::new());

        for line in lines {
            cmd.line(line);
        }

        Ok(())
    }
}

/// `schedule:work`: run `schedule:run` every minute.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScheduleWorkCommand;

#[async_trait]
impl Command for ScheduleWorkCommand {
    fn signature(&self) -> &str {
        "schedule:work
            {--run-output-file= : The file to direct <info>schedule:run</info> output to}
            {--whisper : Do not output message indicating that no jobs were ready to run}"
    }

    fn description(&self) -> &str {
        "Start the schedule worker"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let local = crate::scheduling::event::current_environment() == "local";
        cmd.components()
            .verbosity(if local {
                Verbosity::Normal
            } else {
                Verbosity::Verbose
            })
            .info("Running scheduled tasks.");

        let application = cmd.application();
        let whisper = cmd.option_bool("whisper");
        let output_file = cmd.option("run-output-file");

        let mut executions: Vec<tokio::task::JoinHandle<String>> = Vec::new();
        let mut last_execution_started_at = Carbon::now().sub_minutes(10).start_of_minute();
        let mut should_quit = false;

        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);

        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                _ = &mut ctrl_c, if !should_quit => should_quit = true,
            }

            let now = Carbon::now();
            if !should_quit
                && now.second() == 0
                && !now.start_of_minute().eq(&last_execution_started_at)
            {
                let application = application.clone();
                let args: ArtisanArgs = if whisper {
                    ["--whisper"].into()
                } else {
                    ().into()
                };

                executions.push(tokio::spawn(async move {
                    let output = Output::buffered();
                    if let Err(error) = application
                        .call_with_output("schedule:run", args, &output)
                        .await
                    {
                        application.render_exception(&error, &output);
                    }
                    output.contents()
                }));

                last_execution_started_at = now.start_of_minute();
            }

            let mut running = Vec::new();
            for execution in executions.drain(..) {
                if !execution.is_finished() {
                    running.push(execution);
                    continue;
                }

                let text = execution.await.unwrap_or_default();
                let text = text.trim_start_matches('\n');

                match &output_file {
                    Some(path) => {
                        use std::io::Write;
                        let _ = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                            .and_then(|mut file| file.write_all(text.as_bytes()));
                    }
                    None => cmd.output().write(OutputFormatter::escape(text)),
                }
            }
            executions = running;

            if should_quit && executions.is_empty() {
                return Ok(());
            }
        }
    }
}

/// `schedule:test`: run a scheduled command now.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScheduleTestCommand;

#[async_trait]
impl Command for ScheduleTestCommand {
    fn signature(&self) -> &str {
        "schedule:test {--name= : The name of the scheduled command to run}"
    }

    fn description(&self) -> &str {
        "Run a scheduled command"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let events = Schedule::instance().events();

        let names: Vec<String> = events
            .iter()
            .map(|event| match event.get_command() {
                Some(_) => event.command_for_display(),
                None => event.summary_for_display(),
            })
            .collect();

        if names.is_empty() {
            cmd.components()
                .info("No scheduled commands have been defined.");
            return Ok(());
        }

        let index = match cmd.option("name").filter(|name| !name.is_empty()) {
            Some(name) => {
                let matches: Vec<usize> = names
                    .iter()
                    .enumerate()
                    .filter(|(_, command)| command.trim_start_matches("artisan ").trim() == name)
                    .map(|(index, _)| index)
                    .collect();

                if matches.len() != 1 {
                    cmd.components()
                        .info("No matching scheduled command found.");
                    return Ok(());
                }

                matches[0]
            }
            None => {
                let unique =
                    names.iter().collect::<std::collections::HashSet<_>>().len() == names.len();
                let options: Vec<String> = if unique {
                    names.clone()
                } else {
                    names
                        .iter()
                        .enumerate()
                        .map(|(index, name)| format!("{name} [{index}]"))
                        .collect()
                };

                let selected =
                    crate::prompts::select("Which command would you like to run?", options.clone())
                        .prompt()?;
                options
                    .iter()
                    .position(|option| *option == selected)
                    .unwrap_or(0)
            }
        };

        let event = events[index].clone();
        let summary = if event.is_callback() {
            event.summary_for_display()
        } else {
            event.command_for_display()
        };
        let description = format!(
            "Running [{}]{}",
            OutputFormatter::escape(&summary),
            if event.runs_in_background() {
                " normally in background"
            } else {
                ""
            }
        );

        let application: Arc<Application> = cmd.application();
        let task_event = event.clone();
        cmd.components()
            .task(description, || async move {
                task_event.run_with(&application, true).await
            })
            .await?;

        if !event.is_callback() {
            cmd.components().bullet_list([event.summary_for_display()]);
        }

        cmd.new_line(1);

        Ok(())
    }
}

/// `schedule:interrupt`: stop running `schedule:run` processes from
/// repeating sub-minute tasks.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScheduleInterruptCommand;

#[async_trait]
impl Command for ScheduleInterruptCommand {
    fn signature(&self) -> &str {
        "schedule:interrupt"
    }

    fn description(&self) -> &str {
        "Interrupt the current schedule run"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        Schedule::instance().interrupt().await;
        cmd.components().info("Broadcasting schedule interrupt signal.");
        Ok(())
    }
}

/// `schedule:pause`: skip scheduled tasks until the schedule is resumed.
#[derive(Clone, Copy, Debug, Default)]
pub struct SchedulePauseCommand;

#[async_trait]
impl Command for SchedulePauseCommand {
    fn signature(&self) -> &str {
        "schedule:pause"
    }

    fn description(&self) -> &str {
        "Pause the scheduler"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let schedule = Schedule::instance();
        if !schedule.is_pausable() {
            cmd.components().error("Schedule pausing is currently disabled.");
            return cmd.exit(1);
        }
        schedule.pause().await;
        schedule_events::dispatch(SchedulePaused).await;
        cmd.components().info("Scheduled task processing has been paused.");
        Ok(())
    }
}

/// `schedule:resume`: resume a paused schedule.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScheduleResumeCommand;

#[async_trait]
impl Command for ScheduleResumeCommand {
    fn signature(&self) -> &str {
        "schedule:resume"
    }

    fn description(&self) -> &str {
        "Resume the schedule"
    }

    fn aliases(&self) -> Vec<&str> {
        vec!["schedule:continue"]
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        Schedule::instance().resume().await;
        schedule_events::dispatch(ScheduleResumed).await;
        cmd.components().info("Scheduled task processing has resumed.");
        Ok(())
    }
}

/// `schedule:clear-cache`: delete the scheduler's overlap mutexes.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScheduleClearCacheCommand;

#[async_trait]
impl Command for ScheduleClearCacheCommand {
    fn signature(&self) -> &str {
        "schedule:clear-cache"
    }

    fn description(&self) -> &str {
        "Delete the cached mutex files created by scheduler"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let mut cleared = false;
        for event in Schedule::instance().events() {
            if event.mutex().exists(&event).await {
                cmd.components()
                    .info(format!("Deleting mutex for [{}]", event.command_for_display()));
                event.mutex().forget(&event).await;
                cleared = true;
            }
        }
        if !cleared {
            cmd.components().info("No mutex files were found.");
        }
        Ok(())
    }
}
