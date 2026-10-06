//! Laravel's modern console components: the beautiful, consistent output
//! you see in `migrate`, `about`, `route:list` and friends.
//!
//! ```
//! use illuminate_console::{Components, Output};
//!
//! let output = Output::buffered();
//! let components = Components::new(&output);
//!
//! components.info("The application is in the [production] environment");
//!
//! assert_eq!(output.fetch(), "\n   INFO  The application is in the [production] environment.\n\n");
//! ```

use std::future::Future;
use std::sync::LazyLock;
use std::time::Instant;

use illuminate_support::Result;
use regex::Regex;

use crate::formatter::OutputFormatter;
use crate::output::{Output, PromptStyle, Question, QuestionKind, Verbosity};

/// The outcome of a [`Components::task`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskResult {
    /// The task succeeded: `DONE`.
    Success,
    /// The task failed: `FAIL`.
    Failure,
    /// The task was skipped: `SKIPPED`.
    Skipped,
}

impl From<()> for TaskResult {
    fn from(_: ()) -> Self {
        TaskResult::Success
    }
}

impl From<bool> for TaskResult {
    fn from(success: bool) -> Self {
        if success {
            TaskResult::Success
        } else {
            TaskResult::Failure
        }
    }
}

/// The console components factory (`$this->components` in Laravel).
#[derive(Clone, Debug)]
pub struct Components {
    output: Output,
    verbosity: Verbosity,
}

static DYNAMIC_CONTENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]").unwrap());

fn highlight_dynamic_content(value: &str) -> String {
    DYNAMIC_CONTENT
        .replace_all(value, "<options=bold>[$1]</>")
        .into_owned()
}

fn ensure_punctuation(value: String) -> String {
    if value.ends_with(['.', '?', '!', ':']) {
        value
    } else {
        format!("{value}.")
    }
}

fn ensure_no_punctuation(mut value: String) -> String {
    if value.ends_with(['.', '?', '!', ':']) {
        value.pop();
    }
    value
}

/// Render a two column detail line (`  First ........ Second`).
pub(crate) fn render_two_column_detail(
    output: &Output,
    first: &str,
    second: &str,
    verbosity: Verbosity,
) {
    let first = ensure_no_punctuation(highlight_dynamic_content(first));
    let second = highlight_dynamic_content(second);

    let width = output.width().min(150);
    let first_width = OutputFormatter::width(&first);
    let second_width = OutputFormatter::width(&second);

    let dots = if second.is_empty() {
        width.saturating_sub(5 + first_width)
    } else {
        width.saturating_sub(6 + first_width + second_width)
    };

    let mut line = format!("  {first} <fg=gray>{}</>", ".".repeat(dots));
    if !second.is_empty() {
        line.push(' ');
        line.push_str(&second);
    }

    output.write_with(&line, true, verbosity);
}

fn run_time_for_humans(start: Instant) -> String {
    let milliseconds = start.elapsed().as_secs_f64() * 1000.0;

    if milliseconds <= 1000.0 {
        return format!("{milliseconds:.2}ms");
    }

    let total = (milliseconds / 1000.0).floor() as u64;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);

    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 || parts.is_empty() {
        parts.push(format!("{seconds}s"));
    }
    parts.join(" ")
}

impl Components {
    /// Create a components factory writing to the given output.
    pub fn new(output: &Output) -> Self {
        Self {
            output: output.clone(),
            verbosity: Verbosity::Normal,
        }
    }

    /// Render subsequent components only at the given verbosity.
    pub fn verbosity(mut self, verbosity: Verbosity) -> Self {
        self.verbosity = verbosity;
        self
    }

    /// The underlying output.
    pub fn output(&self) -> &Output {
        &self.output
    }

    // ------------------------------------------------------------------
    // Lines
    // ------------------------------------------------------------------

    /// Render an info line: `  INFO  Message.`
    pub fn info(&self, message: impl AsRef<str>) {
        self.line("info", message);
    }

    /// Render a success line: `  SUCCESS  Message.`
    pub fn success(&self, message: impl AsRef<str>) {
        self.line("success", message);
    }

    /// Render a warning line: `  WARN  Message.`
    pub fn warn(&self, message: impl AsRef<str>) {
        self.line("warn", message);
    }

    /// Render an error line: `  ERROR  Message.`
    pub fn error(&self, message: impl AsRef<str>) {
        self.line("error", message);
    }

    /// Render a line with the given style (`info`, `success`, `warn` or `error`).
    pub fn line(&self, style: &str, message: impl AsRef<str>) {
        let (background, foreground, title) = match style {
            "success" => ("green", "white", "SUCCESS"),
            "warn" | "warning" => ("yellow", "black", "WARN"),
            "error" => ("red", "white", "ERROR"),
            _ => ("blue", "white", "INFO"),
        };

        if self.verbosity > self.output.verbosity() {
            return;
        }

        let content = ensure_punctuation(highlight_dynamic_content(message.as_ref()));
        let margin_top = 2usize.saturating_sub(self.output.new_lines_written());

        let rendered = format!(
            "{}  <bg={background};fg={foreground}> {title} </> {content}\n",
            "\n".repeat(margin_top)
        );

        self.output.write_with(&rendered, true, self.verbosity);
    }

    /// Render an alert: a full width, yellow, upper-cased banner.
    pub fn alert(&self, message: impl AsRef<str>) {
        if self.verbosity > self.output.verbosity() {
            return;
        }

        let content =
            ensure_punctuation(highlight_dynamic_content(message.as_ref())).to_uppercase();
        let content = content.replace("<OPTIONS=BOLD>", "<options=bold>");
        let width = self
            .output
            .width()
            .saturating_sub(4)
            .max(OutputFormatter::width(&content));
        let text_width = OutputFormatter::width(&content);
        let left = (width - text_width) / 2;
        let right = width - text_width - left;

        let decorated = self.output.is_decorated();
        let block = |text: String| {
            if decorated {
                format!("  <bg=yellow;fg=black>{text}</>")
            } else {
                format!("  {text}").trim_end().to_string()
            }
        };

        let lines = [
            String::new(),
            block(" ".repeat(width)),
            block(format!(
                "{}{content}{}",
                " ".repeat(left),
                " ".repeat(right)
            )),
            block(" ".repeat(width)),
        ];

        self.output
            .write_with(&lines.join("\n"), true, self.verbosity);
    }

    /// Render a bulleted list: `  ⇂ Item`.
    pub fn bullet_list<I, S>(&self, elements: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for element in elements {
            let element = ensure_no_punctuation(highlight_dynamic_content(element.as_ref()));
            self.output
                .write_with(&format!("  <fg=gray>⇂ {element}</>"), true, self.verbosity);
        }
    }

    /// Render a two column detail line: `  First .............. Second`.
    pub fn two_column_detail(&self, first: impl AsRef<str>, second: impl AsRef<str>) {
        render_two_column_detail(
            &self.output,
            first.as_ref(),
            second.as_ref(),
            self.verbosity,
        );
    }

    /// Run a task, rendering its description, how long it took, and
    /// whether it was `DONE` or a `FAIL`:
    ///
    /// ```text
    ///   2014_10_12_000000_create_users_table ........... 12.34ms DONE
    /// ```
    pub async fn task<F, Fut, T>(&self, description: impl AsRef<str>, task: F) -> Result<TaskResult>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
        T: Into<TaskResult>,
    {
        let description = ensure_no_punctuation(highlight_dynamic_content(description.as_ref()));
        let description_width = OutputFormatter::width(&description);

        self.output
            .write_with(&format!("  {description} "), false, self.verbosity);

        let start = Instant::now();
        let result = task().await;

        let run_time = format!(" {}", run_time_for_humans(start));
        let width = self.output.width().min(150);
        let dots = width.saturating_sub(description_width + run_time.chars().count() + 10);

        self.output.write_with(
            &format!("<fg=gray>{}</>", ".".repeat(dots)),
            false,
            self.verbosity,
        );
        self.output
            .write_with(&format!("<fg=gray>{run_time}</>"), false, self.verbosity);

        let outcome = match &result {
            Ok(_) => None,
            Err(_) => Some(TaskResult::Failure),
        };

        let (outcome, result) = match result {
            Ok(value) => {
                let outcome: TaskResult = value.into();
                (outcome, Ok(outcome))
            }
            Err(error) => (outcome.unwrap_or(TaskResult::Failure), Err(error)),
        };

        let label = match outcome {
            TaskResult::Failure => " <fg=red;options=bold>FAIL</>",
            TaskResult::Skipped => " <fg=yellow;options=bold>SKIPPED</>",
            TaskResult::Success => " <fg=green;options=bold>DONE</>",
        };
        self.output.write_with(label, true, self.verbosity);

        result
    }

    // ------------------------------------------------------------------
    // Questions
    // ------------------------------------------------------------------

    /// Ask a question.
    pub fn ask(&self, question: impl AsRef<str>) -> String {
        self.output
            .ask_text(&Question::new(question.as_ref()), PromptStyle::Components)
            .unwrap_or_default()
    }

    /// Ask a question, falling back to the given default.
    pub fn ask_with_default(
        &self,
        question: impl AsRef<str>,
        default: impl Into<String>,
    ) -> String {
        let question = Question::new(question.as_ref()).default(Some(default.into()));
        self.output
            .ask_text(&question, PromptStyle::Components)
            .unwrap_or_default()
    }

    /// Ask a question with suggested answers.
    pub fn ask_with_completion<I, S>(
        &self,
        question: impl AsRef<str>,
        choices: I,
        default: Option<&str>,
    ) -> String
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut question = Question::new(question.as_ref()).default(default.map(String::from));
        question.suggestions = choices.into_iter().map(Into::into).collect();
        self.output
            .ask_text(&question, PromptStyle::Components)
            .unwrap_or_default()
    }

    /// Ask a question, hiding the answer as it is typed.
    pub fn secret(&self, question: impl AsRef<str>) -> String {
        let question = Question::new(question.as_ref()).kind(QuestionKind::Secret);
        self.output
            .ask_text(&question, PromptStyle::Components)
            .unwrap_or_default()
    }

    /// Ask for confirmation.
    pub fn confirm(&self, question: impl AsRef<str>, default: bool) -> bool {
        self.output
            .ask_confirmation(question.as_ref(), default, PromptStyle::Components)
    }

    /// Give the user a single choice from a list of options.
    pub fn choice<I, S>(
        &self,
        question: impl AsRef<str>,
        choices: I,
        default: Option<usize>,
    ) -> String
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let question = choice_question(
            question.as_ref(),
            choices,
            default.map(|d| d.to_string()),
            false,
        );
        self.output
            .ask_choice(&question, false, PromptStyle::Components)
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// Give the user several choices from a list of options.
    pub fn choice_multiple<I, S>(
        &self,
        question: impl AsRef<str>,
        choices: I,
        default: &[usize],
    ) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let default = (!default.is_empty()).then(|| {
            default
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(",")
        });
        let question = choice_question(question.as_ref(), choices, default, true);
        self.output
            .ask_choice(&question, false, PromptStyle::Components)
    }
}

pub(crate) fn choice_question<I, S>(
    text: &str,
    choices: I,
    default: Option<String>,
    multiple: bool,
) -> Question
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let choices = choices
        .into_iter()
        .enumerate()
        .map(|(index, label)| (index.to_string(), label.into()))
        .collect();

    Question::new(text)
        .kind(QuestionKind::Choice { choices, multiple })
        .default(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn components() -> (Output, Components) {
        let output = Output::buffered();
        let components = Components::new(&output);
        (output, components)
    }

    #[test]
    fn it_renders_lines() {
        let (output, components) = components();
        components.info("Hello");
        assert_eq!(output.fetch(), "\n   INFO  Hello.\n\n");

        components.warn("Careful!");
        components.error("Oops");
        components.success("Yes");
        assert_eq!(
            output.fetch(),
            "   WARN  Careful!\n\n   ERROR  Oops.\n\n   SUCCESS  Yes.\n\n"
        );
    }

    #[test]
    fn lines_add_margin_after_text() {
        let (output, components) = components();
        output.writeln("Text");
        components.info("Message");
        assert_eq!(output.fetch(), "Text\n\n   INFO  Message.\n\n");
    }

    #[test]
    fn it_highlights_dynamic_content_when_decorated() {
        let output = Output::buffered().with_decoration(true);
        Components::new(&output).info("In [production]");
        let text = output.fetch();
        assert!(text.contains("\x1b[1m[production]\x1b[22m."));
        assert!(text.contains("\x1b[37;44m INFO \x1b[39;49m"));
    }

    #[test]
    fn it_renders_two_column_details() {
        let (output, components) = components();
        components.two_column_detail("Application Name", "Laravel");
        let line = output.fetch();
        assert_eq!(line.trim_end().chars().count(), 78);
        assert!(line.starts_with("  Application Name ...."));
        assert!(line.ends_with(".... Laravel\n"));

        components.two_column_detail("Environment", "");
        let line = output.fetch();
        assert_eq!(line.trim_end().chars().count(), 78);
        assert!(line.ends_with("...\n"));
    }

    #[test]
    fn it_renders_bullet_lists() {
        let (output, components) = components();
        components.bullet_list(["make:model", "make:migration."]);
        assert_eq!(output.fetch(), "  ⇂ make:model\n  ⇂ make:migration\n");
    }

    #[test]
    fn it_renders_alerts() {
        let (output, components) = components();
        components.alert("Application in production");
        let text = output.fetch();
        assert!(text.contains("APPLICATION IN PRODUCTION."));
        assert!(text.starts_with('\n'));
    }

    #[tokio::test]
    async fn it_renders_tasks() {
        let (output, components) = components();
        let result = components
            .task("Migrating", || async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(result, TaskResult::Success);
        let line = output.fetch();
        assert!(line.starts_with("  Migrating ...."));
        assert!(line.contains("ms DONE\n"));
        assert_eq!(line.trim_end().chars().count(), 78);

        let result = components
            .task("Skipping", || async { Ok(false) })
            .await
            .unwrap();
        assert_eq!(result, TaskResult::Failure);
        assert!(output.fetch().ends_with(" FAIL\n"));

        let error = components
            .task("Exploding", || async {
                Err::<(), _>(illuminate_support::error::error!("Boom"))
            })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Boom");
        assert!(output.fetch().ends_with(" FAIL\n"));
    }

    #[test]
    fn it_asks_questions_in_laravel_style() {
        let output = Output::buffered().with_input(["Taylor", "1", "y"]);
        let components = Components::new(&output);
        assert_eq!(components.ask("What is your name?"), "Taylor");
        assert_eq!(
            components.choice("Language", ["PHP", "Rust"], Some(0)),
            "Rust"
        );
        assert!(components.confirm("Continue", false));

        let text = output.contents();
        assert!(text.contains("  What is your name?\n❯ \n"));
        assert!(text.contains("  Language: [PHP]\n"));
        assert!(text.contains("  PHP ...."));
        assert!(text.contains("  Continue: (yes/no) [no]\n"));
    }

    #[test]
    fn it_respects_verbosity() {
        let (output, components) = components();
        components
            .clone()
            .verbosity(Verbosity::Verbose)
            .info("Hidden");
        components
            .clone()
            .verbosity(Verbosity::Verbose)
            .bullet_list(["a"]);
        assert_eq!(output.fetch(), "");
    }
}
