//! The [`Console`]: everything a command needs while it runs. It is the
//! Rust spelling of `$this` inside a Laravel command.

use std::sync::{Arc, RwLock};

use illuminate_support::{Map, Result, Value};

use crate::application::Application;
use crate::command::{CommandExit, ManuallyFailedException};
use crate::components::{Components, choice_question};
use crate::formatter::OutputFormatter;
use crate::input::{ArtisanArgs, Input, InputValue};
use crate::output::{Output, PromptStyle, Question, QuestionKind, Verbosity};
use crate::progress::ProgressBar;
use crate::table::Table;

struct Inner {
    name: String,
    input: RwLock<Input>,
    output: Output,
    application: Arc<Application>,
}

/// A running command's input, output and helpers.
///
/// `Console` is a cheap, cloneable handle, so you may freely pass it to
/// other functions and tasks.
#[derive(Clone)]
pub struct Console {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Console").field("name", &self.inner.name).finish()
    }
}

impl Console {
    pub(crate) fn new(name: String, input: Input, output: Output, application: Arc<Application>) -> Self {
        Self {
            inner: Arc::new(Inner {
                name,
                input: RwLock::new(input),
                output,
                application,
            }),
        }
    }

    /// The name of the running command.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The application running the command.
    pub fn application(&self) -> Arc<Application> {
        self.inner.application.clone()
    }

    /// The output the command writes to.
    pub fn output(&self) -> &Output {
        &self.inner.output
    }

    /// Laravel's console components (`$this->components`).
    pub fn components(&self) -> Components {
        Components::new(&self.inner.output)
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    /// The raw value of an argument.
    pub fn argument_value(&self, key: &str) -> Option<InputValue> {
        self.inner.input.read().unwrap().argument(key)
    }

    /// Get the value of a command argument. Array arguments are comma
    /// separated; use [`Console::argument_list`] for those instead.
    pub fn argument(&self, key: &str) -> Option<String> {
        self.argument_value(key).and_then(|value| value.as_string())
    }

    /// Get the values of an array argument (`{user*}`).
    pub fn argument_list(&self, key: &str) -> Vec<String> {
        self.argument_value(key).map(|value| value.as_list()).unwrap_or_default()
    }

    /// Get all of the arguments passed to the command.
    pub fn arguments(&self) -> Value {
        let input = self.inner.input.read().unwrap();
        Value::Object(
            input
                .arguments()
                .into_iter()
                .map(|(name, value)| (name, value.to_value()))
                .collect::<Map<String, Value>>(),
        )
    }

    /// Determine if the command defines the given argument.
    pub fn has_argument(&self, key: &str) -> bool {
        self.inner.input.read().unwrap().definition().argument(key).is_some()
    }

    /// The raw value of an option.
    pub fn option_value(&self, key: &str) -> Option<InputValue> {
        self.inner.input.read().unwrap().option(key)
    }

    /// Get the value of a command option. Switches are `Some("1")` when
    /// given and `None` otherwise.
    pub fn option(&self, key: &str) -> Option<String> {
        self.option_value(key).and_then(|value| value.as_string())
    }

    /// Determine if a switch (or valued option) is "on".
    pub fn option_bool(&self, key: &str) -> bool {
        self.option_value(key).is_some_and(|value| value.as_bool())
    }

    /// Get the values of an array option (`{--id=*}`).
    pub fn option_list(&self, key: &str) -> Vec<String> {
        self.option_value(key).map(|value| value.as_list()).unwrap_or_default()
    }

    /// Get all of the options passed to the command.
    pub fn options(&self) -> Value {
        let input = self.inner.input.read().unwrap();
        Value::Object(
            input
                .options()
                .into_iter()
                .map(|(name, value)| (name, value.to_value()))
                .collect::<Map<String, Value>>(),
        )
    }

    /// Determine if the command defines the given option.
    pub fn has_option(&self, key: &str) -> bool {
        self.inner.input.read().unwrap().definition().option(key).is_some()
    }

    /// Set an argument's value.
    pub fn set_argument(&self, key: &str, value: impl Into<InputValue>) -> Result<()> {
        Ok(self.inner.input.write().unwrap().set_argument(key, value)?)
    }

    /// Set an option's value.
    pub fn set_option(&self, key: &str, value: impl Into<InputValue>) -> Result<()> {
        Ok(self.inner.input.write().unwrap().set_option(key, value)?)
    }

    // ------------------------------------------------------------------
    // Output
    // ------------------------------------------------------------------

    /// Write a string as standard output.
    pub fn line(&self, message: impl AsRef<str>) {
        self.line_with(message, None, Verbosity::Normal);
    }

    /// Write a string with the given style, at the given verbosity.
    pub fn line_with(&self, message: impl AsRef<str>, style: Option<&str>, verbosity: Verbosity) {
        let message = message.as_ref();
        let styled = match style {
            Some(style) => format!("<{style}>{message}</{style}>"),
            None => message.to_string(),
        };
        self.inner.output.write_with(&styled, true, verbosity);
    }

    /// Write a string as information output (green).
    pub fn info(&self, message: impl AsRef<str>) {
        self.line_with(message, Some("info"), Verbosity::Normal);
    }

    /// Write a string as comment output (yellow).
    pub fn comment(&self, message: impl AsRef<str>) {
        self.line_with(message, Some("comment"), Verbosity::Normal);
    }

    /// Write a string as question output (black on cyan).
    pub fn question(&self, message: impl AsRef<str>) {
        self.line_with(message, Some("question"), Verbosity::Normal);
    }

    /// Write a string as error output (white on red).
    pub fn error(&self, message: impl AsRef<str>) {
        self.line_with(message, Some("error"), Verbosity::Normal);
    }

    /// Write a string as warning output (yellow).
    pub fn warn(&self, message: impl AsRef<str>) {
        self.line_with(message, Some("warning"), Verbosity::Normal);
    }

    /// Write a string in an alert box.
    pub fn alert(&self, message: impl AsRef<str>) {
        let message = message.as_ref();
        let length = OutputFormatter::strip(message).chars().count() + 12;

        self.comment("*".repeat(length));
        self.comment(format!("*     {message}     *"));
        self.comment("*".repeat(length));
        self.comment("");
    }

    /// Write blank lines.
    pub fn new_line(&self, count: usize) -> &Self {
        self.inner.output.new_line(count);
        self
    }

    /// Format input to a textual table.
    pub fn table<H, HS, R, C, S>(&self, headers: H, rows: R)
    where
        H: IntoIterator<Item = HS>,
        HS: ToString,
        R: IntoIterator<Item = C>,
        C: IntoIterator<Item = S>,
        S: ToString,
    {
        Table::new().headers(headers).rows(rows).render(&self.inner.output);
    }

    /// Format input to a textual table using one of the named styles
    /// (`default`, `compact`, `borderless`, `box`, ...).
    pub fn table_with_style<H, HS, R, C, S>(&self, headers: H, rows: R, style: &str)
    where
        H: IntoIterator<Item = HS>,
        HS: ToString,
        R: IntoIterator<Item = C>,
        C: IntoIterator<Item = S>,
        S: ToString,
    {
        Table::new()
            .headers(headers)
            .rows(rows)
            .style(style)
            .render(&self.inner.output);
    }

    /// Create a progress bar with the given number of steps.
    pub fn create_progress_bar(&self, max: usize) -> ProgressBar {
        ProgressBar::new(&self.inner.output, max)
    }

    /// Run the callback for each item while displaying a progress bar,
    /// returning the items.
    pub fn with_progress_bar<I, T, F>(&self, items: I, mut callback: F) -> Vec<T>
    where
        I: IntoIterator<Item = T>,
        F: FnMut(&T, &ProgressBar),
    {
        let items: Vec<T> = items.into_iter().collect();
        let bar = self.create_progress_bar(items.len());

        bar.start();
        for item in &items {
            callback(item, &bar);
            bar.advance();
        }
        bar.finish();

        items
    }

    /// The output's verbosity.
    pub fn verbosity(&self) -> Verbosity {
        self.inner.output.verbosity()
    }

    /// Determine if the output is quiet.
    pub fn is_quiet(&self) -> bool {
        self.inner.output.is_quiet()
    }

    /// Determine if the output is verbose (`-v`).
    pub fn is_verbose(&self) -> bool {
        self.inner.output.is_verbose()
    }

    /// Determine if the output is very verbose (`-vv`).
    pub fn is_very_verbose(&self) -> bool {
        self.inner.output.is_very_verbose()
    }

    /// Determine if the output is in debug mode (`-vvv`).
    pub fn is_debug(&self) -> bool {
        self.inner.output.is_debug()
    }

    /// Determine if the command may ask questions.
    pub fn is_interactive(&self) -> bool {
        self.inner.output.is_interactive()
    }

    // ------------------------------------------------------------------
    // Interaction
    // ------------------------------------------------------------------

    /// Prompt the user for input. Returns an empty string when nothing is given.
    pub fn ask(&self, question: impl AsRef<str>) -> String {
        self.inner
            .output
            .ask_text(&Question::new(question.as_ref()), PromptStyle::Symfony)
            .unwrap_or_default()
    }

    /// Prompt the user for input, falling back to the given default.
    pub fn ask_with_default(&self, question: impl AsRef<str>, default: impl Into<String>) -> String {
        let question = Question::new(question.as_ref()).default(Some(default.into()));
        self.inner
            .output
            .ask_text(&question, PromptStyle::Symfony)
            .unwrap_or_default()
    }

    /// Prompt the user for input, hiding what they type.
    pub fn secret(&self, question: impl AsRef<str>) -> String {
        let question = Question::new(question.as_ref()).kind(QuestionKind::Secret);
        self.inner
            .output
            .ask_text(&question, PromptStyle::Symfony)
            .unwrap_or_default()
    }

    /// Confirm a question with the user.
    pub fn confirm(&self, question: impl AsRef<str>, default: bool) -> bool {
        self.inner
            .output
            .ask_confirmation(question.as_ref(), default, PromptStyle::Symfony)
    }

    /// Give the user a single choice from a list of options. The default
    /// is the index of the default option.
    pub fn choice<I, S>(&self, question: impl AsRef<str>, choices: I, default: Option<usize>) -> String
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let question = choice_question(question.as_ref(), choices, default.map(|d| d.to_string()), false);
        self.inner
            .output
            .ask_choice(&question, false, PromptStyle::Symfony)
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// Give the user several choices from a list of options.
    pub fn choice_multiple<I, S>(&self, question: impl AsRef<str>, choices: I, default: &[usize]) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let default = (!default.is_empty()).then(|| default.iter().map(usize::to_string).collect::<Vec<_>>().join(","));
        let question = choice_question(question.as_ref(), choices, default, true);
        self.inner.output.ask_choice(&question, false, PromptStyle::Symfony)
    }

    /// Prompt the user for input with auto-completion suggestions.
    pub fn anticipate<I, S>(&self, question: impl AsRef<str>, suggestions: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.ask_with_completion(question, suggestions, None)
    }

    /// Prompt the user for input with auto-completion suggestions and a default.
    pub fn ask_with_completion<I, S>(&self, question: impl AsRef<str>, suggestions: I, default: Option<&str>) -> String
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut question = Question::new(question.as_ref()).default(default.map(String::from));
        question.suggestions = suggestions.into_iter().map(Into::into).collect();
        self.inner
            .output
            .ask_text(&question, PromptStyle::Symfony)
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------
    // Calling other commands
    // ------------------------------------------------------------------

    /// Call another console command, sharing this command's output.
    pub async fn call(&self, command: &str, args: impl Into<ArtisanArgs>) -> Result<i32> {
        self.inner
            .application
            .call_from_console(self, command, args.into(), self.inner.output.clone())
            .await
    }

    /// Call another console command without output.
    pub async fn call_silently(&self, command: &str, args: impl Into<ArtisanArgs>) -> Result<i32> {
        self.inner
            .application
            .call_from_console(self, command, args.into(), self.inner.output.silenced())
            .await
    }

    /// Alias of [`Console::call_silently`].
    pub async fn call_silent(&self, command: &str, args: impl Into<ArtisanArgs>) -> Result<i32> {
        self.call_silently(command, args).await
    }

    // ------------------------------------------------------------------
    // Flow
    // ------------------------------------------------------------------

    /// Fail the command manually: the message is displayed as an error
    /// and the command exits with `1`.
    ///
    /// ```ignore
    /// return cmd.fail("Something went wrong.");
    /// ```
    pub fn fail<T>(&self, message: impl Into<String>) -> Result<T> {
        Err(ManuallyFailedException::new(message).into())
    }

    /// End the command with the given exit code.
    ///
    /// ```ignore
    /// return cmd.exit(3);
    /// ```
    pub fn exit<T>(&self, code: i32) -> Result<T> {
        Err(CommandExit { code }.into())
    }

    /// The options forwarded to commands called from this one.
    pub(crate) fn context(&self) -> Vec<(String, InputValue)> {
        let input = self.inner.input.read().unwrap();
        ["ansi", "no-interaction", "quiet", "verbose"]
            .into_iter()
            .filter_map(|name| {
                input
                    .option(name)
                    .filter(|value| value.as_bool())
                    .map(|value| (format!("--{name}"), value))
            })
            .collect()
    }
}
