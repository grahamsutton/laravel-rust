//! The scheduler's Artisan commands: `schedule:run`, `schedule:list`,
//! `schedule:work` and `schedule:test`.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_trait::async_trait;
use illuminate_support::{Carbon, Result, error::error, json};
use regex::Regex;

use super::event::Event;
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
            match task_event.run_with(&task_application, foreground).await {
                Ok(()) => {
                    let code = task_event.exit_code();
                    if !background && code.is_some_and(|code| code != 0) {
                        task_application.report(&error!(
                            "Scheduled command [{}] failed with exit code [{}].",
                            task_event
                                .get_command()
                                .unwrap_or_else(|| task_event.summary_for_display()),
                            code.unwrap_or_default()
                        ));
                    }
                }
                Err(error) => task_application.report(&error),
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

        for event in schedule.due_events(&started_at) {
            if !event.filters_pass(&started_at).await {
                continue;
            }

            if !events_ran {
                cmd.new_line(1);
            }

            if event.runs_on_one_server() {
                if schedule.server_should_run(&event, &started_at).await {
                    run_event(&cmd, &event, false).await?;
                } else {
                    cmd.components().info(format!(
                        "Skipping [{}] because the command already ran on another server.",
                        event.summary_for_display()
                    ));
                }
            } else {
                run_event(&cmd, &event, false).await?;
            }

            events_ran = true;
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
                    "repeat_seconds": null,
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
