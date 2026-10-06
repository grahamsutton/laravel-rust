//! Laravel Prompts: beautiful, user-friendly forms for your commands.
//!
//! These are line-based versions of Laravel Prompts, which behave like
//! Prompts' fallbacks: they work in any terminal, with piped input, and
//! with `PendingCommand`'s expected questions in tests.
//!
//! ```no_run
//! use illuminate_console::prompts::{confirm, select, text};
//!
//! # fn example() -> illuminate_support::Result<()> {
//! let name = text("What is your name?")
//!     .placeholder("E.g. Taylor Otwell")
//!     .required(true)
//!     .validate(|value| (value.len() < 3).then(|| "The name must be at least 3 characters.".into()))
//!     .prompt()?;
//!
//! let role = select("What role should the user have?", ["Member", "Contributor", "Owner"])
//!     .default("Owner")
//!     .prompt()?;
//!
//! let confirmed = confirm("Do you accept the terms?").prompt()?;
//! # Ok(())
//! # }
//! ```

use std::future::Future;
use std::sync::{Arc, Mutex};

use illuminate_support::Result;

use crate::application::current_output;
use crate::command::PromptValidationException;
use crate::components::Components;
use crate::output::{Output, PromptStyle, Question, QuestionKind, validate_choice};
use crate::progress::ProgressBar;
use crate::table::Table;

type Validator = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type ListValidator = Arc<dyn Fn(&[String]) -> Option<String> + Send + Sync>;

/// Render prompts (and notes) written while the future runs to the given output.
pub async fn with_output<F: Future>(output: Output, future: F) -> F::Output {
    crate::application::scope_output(output, future).await
}

fn invalid(output: &Output, message: String) -> Result<()> {
    let scripted = output.reader().is_scripted();
    let interactive = output.is_interactive();

    if interactive {
        Components::new(output).error(&message);
    }

    if scripted || !interactive {
        return Err(PromptValidationException {
            message,
            rendered: interactive,
        }
        .into());
    }

    Ok(())
}

fn required_message(required: &Option<String>) -> String {
    required.clone().unwrap_or_else(|| "Required.".to_string())
}

// ----------------------------------------------------------------------
// Text, textarea, password & suggest
// ----------------------------------------------------------------------

/// A text prompt (also used by `textarea`, `password` and `suggest`).
#[derive(Clone)]
pub struct TextPrompt {
    label: String,
    placeholder: String,
    default: String,
    required: Option<Option<String>>,
    validate: Option<Validator>,
    hint: String,
    multiline: bool,
    secret: bool,
    suggestions: Vec<String>,
}

impl std::fmt::Debug for TextPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextPrompt")
            .field("label", &self.label)
            .finish()
    }
}

/// Prompt the user for text.
pub fn text(label: impl Into<String>) -> TextPrompt {
    TextPrompt {
        label: label.into(),
        placeholder: String::new(),
        default: String::new(),
        required: None,
        validate: None,
        hint: String::new(),
        multiline: false,
        secret: false,
        suggestions: Vec::new(),
    }
}

/// Prompt the user for multiple lines of text.
pub fn textarea(label: impl Into<String>) -> TextPrompt {
    TextPrompt {
        multiline: true,
        ..text(label)
    }
}

/// Prompt the user for a password (the input is hidden).
pub fn password(label: impl Into<String>) -> TextPrompt {
    TextPrompt {
        secret: true,
        ..text(label)
    }
}

/// Prompt the user for text, suggesting possible answers.
pub fn suggest<I, S>(label: impl Into<String>, options: I) -> TextPrompt
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    TextPrompt {
        suggestions: options.into_iter().map(Into::into).collect(),
        ..text(label)
    }
}

impl TextPrompt {
    /// Placeholder text shown when the input is empty.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// The default value.
    pub fn default(mut self, default: impl Into<String>) -> Self {
        self.default = default.into();
        self
    }

    /// Require a value ("Required.").
    pub fn required(mut self, required: bool) -> Self {
        self.required = required.then_some(None);
        self
    }

    /// Require a value, with a custom message.
    pub fn required_with(mut self, message: impl Into<String>) -> Self {
        self.required = Some(Some(message.into()));
        self
    }

    /// Validate the value: return `Some(message)` when it is invalid.
    pub fn validate(
        mut self,
        validator: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.validate = Some(Arc::new(validator));
        self
    }

    /// An informational hint.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    fn check(&self, value: &str) -> Option<String> {
        if let Some(required) = &self.required
            && value.is_empty()
        {
            return Some(required_message(required));
        }

        self.validate.as_ref().and_then(|validate| validate(value))
    }

    /// Prompt the user.
    pub fn prompt(self) -> Result<String> {
        self.prompt_on(&current_output())
    }

    /// Prompt the user through the given output.
    pub fn prompt_on(self, output: &Output) -> Result<String> {
        loop {
            if !output.is_interactive() {
                if let Some(error) = self.check(&self.default) {
                    invalid(output, error)?;
                }
                return Ok(self.default.clone());
            }

            let mut question = Question::new(&self.label)
                .kind(if self.secret {
                    QuestionKind::Secret
                } else {
                    QuestionKind::Text
                })
                .default((!self.default.is_empty()).then(|| self.default.clone()));
            question.multiline = self.multiline;
            question.suggestions = self.suggestions.clone();

            let value = output
                .ask_text(&question, PromptStyle::Components)
                .unwrap_or_default();

            match self.check(&value) {
                Some(error) => invalid(output, error)?,
                None => return Ok(value),
            }
        }
    }
}

// ----------------------------------------------------------------------
// Confirm
// ----------------------------------------------------------------------

/// A yes / no prompt.
#[derive(Clone, Debug)]
pub struct ConfirmPrompt {
    label: String,
    default: bool,
    yes: String,
    no: String,
    required: Option<Option<String>>,
    hint: String,
}

/// Ask the user for confirmation (defaults to "yes").
pub fn confirm(label: impl Into<String>) -> ConfirmPrompt {
    ConfirmPrompt {
        label: label.into(),
        default: true,
        yes: "Yes".to_string(),
        no: "No".to_string(),
        required: None,
        hint: String::new(),
    }
}

impl ConfirmPrompt {
    /// The default answer.
    pub fn default(mut self, default: bool) -> Self {
        self.default = default;
        self
    }

    /// The label of the "yes" answer.
    pub fn yes(mut self, label: impl Into<String>) -> Self {
        self.yes = label.into();
        self
    }

    /// The label of the "no" answer.
    pub fn no(mut self, label: impl Into<String>) -> Self {
        self.no = label.into();
        self
    }

    /// Require the user to answer "yes".
    pub fn required(mut self, required: bool) -> Self {
        self.required = required.then_some(None);
        self
    }

    /// Require the user to answer "yes", with a custom message.
    pub fn required_with(mut self, message: impl Into<String>) -> Self {
        self.required = Some(Some(message.into()));
        self
    }

    /// An informational hint.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// Prompt the user.
    pub fn prompt(self) -> Result<bool> {
        self.prompt_on(&current_output())
    }

    /// Prompt the user through the given output.
    pub fn prompt_on(self, output: &Output) -> Result<bool> {
        loop {
            let value = if output.is_interactive() {
                output.ask_confirmation(&self.label, self.default, PromptStyle::Components)
            } else {
                self.default
            };

            match (&self.required, value) {
                (Some(required), false) => invalid(output, required_message(required))?,
                _ => return Ok(value),
            }
        }
    }
}

// ----------------------------------------------------------------------
// Select & multiselect
// ----------------------------------------------------------------------

/// The options of a select prompt: a list of labels, or `(key, label)`
/// pairs (in which case the selected key is returned).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectOptions {
    options: Vec<(String, String)>,
    keyed: bool,
}

impl SelectOptions {
    fn list<I: IntoIterator<Item = String>>(labels: I) -> Self {
        Self {
            options: labels
                .into_iter()
                .enumerate()
                .map(|(index, label)| (index.to_string(), label))
                .collect(),
            keyed: false,
        }
    }

    fn keyed<I: IntoIterator<Item = (String, String)>>(pairs: I) -> Self {
        Self {
            options: pairs.into_iter().collect(),
            keyed: true,
        }
    }

    fn key_for(&self, value: &str) -> Option<String> {
        self.options
            .iter()
            .find(|(key, label)| (self.keyed && key == value) || label == value)
            .map(|(key, _)| key.clone())
    }
}

impl From<Vec<&str>> for SelectOptions {
    fn from(labels: Vec<&str>) -> Self {
        Self::list(labels.into_iter().map(String::from))
    }
}

impl From<Vec<String>> for SelectOptions {
    fn from(labels: Vec<String>) -> Self {
        Self::list(labels)
    }
}

impl From<&[&str]> for SelectOptions {
    fn from(labels: &[&str]) -> Self {
        Self::list(labels.iter().map(|label| label.to_string()))
    }
}

impl<const N: usize> From<[&str; N]> for SelectOptions {
    fn from(labels: [&str; N]) -> Self {
        Self::list(labels.into_iter().map(String::from))
    }
}

impl<const N: usize> From<[String; N]> for SelectOptions {
    fn from(labels: [String; N]) -> Self {
        Self::list(labels)
    }
}

impl From<Vec<(&str, &str)>> for SelectOptions {
    fn from(pairs: Vec<(&str, &str)>) -> Self {
        Self::keyed(
            pairs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string())),
        )
    }
}

impl From<Vec<(String, String)>> for SelectOptions {
    fn from(pairs: Vec<(String, String)>) -> Self {
        Self::keyed(pairs)
    }
}

impl<const N: usize> From<[(&str, &str); N]> for SelectOptions {
    fn from(pairs: [(&str, &str); N]) -> Self {
        Self::keyed(
            pairs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string())),
        )
    }
}

/// A single choice prompt.
#[derive(Clone)]
pub struct SelectPrompt {
    label: String,
    options: SelectOptions,
    default: Option<String>,
    hint: String,
    validate: Option<Validator>,
}

impl std::fmt::Debug for SelectPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectPrompt")
            .field("label", &self.label)
            .finish()
    }
}

/// Ask the user to select one of the given options.
pub fn select(label: impl Into<String>, options: impl Into<SelectOptions>) -> SelectPrompt {
    SelectPrompt {
        label: label.into(),
        options: options.into(),
        default: None,
        hint: String::new(),
        validate: None,
    }
}

impl SelectPrompt {
    /// The default option (its label, or its key for keyed options).
    pub fn default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }

    /// An informational hint.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// The number of options displayed before scrolling (ignored by the
    /// line-based prompts).
    pub fn scroll(self, _lines: usize) -> Self {
        self
    }

    /// Validate the selection: return `Some(message)` when it is invalid.
    pub fn validate(
        mut self,
        validator: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.validate = Some(Arc::new(validator));
        self
    }

    /// Prompt the user, returning the selected label (or key, for keyed options).
    pub fn prompt(self) -> Result<String> {
        self.prompt_on(&current_output())
    }

    /// Prompt the user through the given output.
    pub fn prompt_on(self, output: &Output) -> Result<String> {
        let default_key = self
            .default
            .as_deref()
            .and_then(|default| self.options.key_for(default));
        let first = self.options.options.first().map(|(key, _)| key.clone());

        loop {
            let value = if output.is_interactive() {
                let question = Question::new(&self.label)
                    .kind(QuestionKind::Choice {
                        choices: self.options.options.clone(),
                        multiple: false,
                    })
                    .default(default_key.clone());
                output
                    .ask_choice(&question, self.options.keyed, PromptStyle::Components)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            } else {
                let key = default_key
                    .clone()
                    .or_else(|| first.clone())
                    .unwrap_or_default();
                validate_choice(&key, &self.options.options, false, self.options.keyed)
                    .ok()
                    .and_then(|values| values.into_iter().next())
                    .unwrap_or_default()
            };

            match self.validate.as_ref().and_then(|validate| validate(&value)) {
                Some(error) => invalid(output, error)?,
                None => return Ok(value),
            }
        }
    }
}

/// A multiple choice prompt.
#[derive(Clone)]
pub struct MultiSelectPrompt {
    label: String,
    options: SelectOptions,
    default: Vec<String>,
    required: Option<Option<String>>,
    hint: String,
    validate: Option<ListValidator>,
}

impl std::fmt::Debug for MultiSelectPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiSelectPrompt")
            .field("label", &self.label)
            .finish()
    }
}

/// Ask the user to select any of the given options.
pub fn multiselect(
    label: impl Into<String>,
    options: impl Into<SelectOptions>,
) -> MultiSelectPrompt {
    MultiSelectPrompt {
        label: label.into(),
        options: options.into(),
        default: Vec::new(),
        required: None,
        hint: String::new(),
        validate: None,
    }
}

impl MultiSelectPrompt {
    /// The options selected by default.
    pub fn default<I, S>(mut self, default: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.default = default.into_iter().map(Into::into).collect();
        self
    }

    /// Require at least one selection.
    pub fn required(mut self, required: bool) -> Self {
        self.required = required.then_some(None);
        self
    }

    /// Require at least one selection, with a custom message.
    pub fn required_with(mut self, message: impl Into<String>) -> Self {
        self.required = Some(Some(message.into()));
        self
    }

    /// An informational hint.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// The number of options displayed before scrolling (ignored).
    pub fn scroll(self, _lines: usize) -> Self {
        self
    }

    /// Validate the selection: return `Some(message)` when it is invalid.
    pub fn validate(
        mut self,
        validator: impl Fn(&[String]) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.validate = Some(Arc::new(validator));
        self
    }

    /// Prompt the user, returning the selected labels (or keys).
    pub fn prompt(self) -> Result<Vec<String>> {
        self.prompt_on(&current_output())
    }

    /// Prompt the user through the given output.
    pub fn prompt_on(self, output: &Output) -> Result<Vec<String>> {
        let default_keys: Vec<String> = self
            .default
            .iter()
            .filter_map(|default| self.options.key_for(default))
            .collect();
        let defaults = || {
            validate_choice(
                &default_keys.join(","),
                &self.options.options,
                true,
                self.options.keyed,
            )
            .unwrap_or_default()
        };

        loop {
            let values = if !output.is_interactive() {
                defaults()
            } else {
                let question = Question::new(&self.label)
                    .kind(QuestionKind::Choice {
                        choices: self.options.options.clone(),
                        multiple: true,
                    })
                    .default((!default_keys.is_empty()).then(|| default_keys.join(",")));

                match output.ask_question(&question, PromptStyle::Components) {
                    None => defaults(),
                    Some(answer) if answer.trim().is_empty() => defaults(),
                    Some(answer) => match validate_choice(
                        &answer,
                        &self.options.options,
                        true,
                        self.options.keyed,
                    ) {
                        Ok(values) => values,
                        Err(error) => {
                            invalid(output, error)?;
                            continue;
                        }
                    },
                }
            };

            if let Some(required) = &self.required
                && values.is_empty()
            {
                invalid(output, required_message(required))?;
                continue;
            }

            match self
                .validate
                .as_ref()
                .and_then(|validate| validate(&values))
            {
                Some(error) => invalid(output, error)?,
                None => return Ok(values),
            }
        }
    }
}

/// A search prompt: ask for a query, then select from the matching options.
pub struct SearchPrompt<F> {
    label: String,
    options: F,
    placeholder: String,
    hint: String,
}

/// Let the user search for an option.
pub fn search<F, O>(label: impl Into<String>, options: F) -> SearchPrompt<F>
where
    F: Fn(&str) -> O,
    O: Into<SelectOptions>,
{
    SearchPrompt {
        label: label.into(),
        options,
        placeholder: String::new(),
        hint: String::new(),
    }
}

impl<F, O> SearchPrompt<F>
where
    F: Fn(&str) -> O,
    O: Into<SelectOptions>,
{
    /// Placeholder text shown when the input is empty.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// An informational hint.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// Prompt the user.
    pub fn prompt(self) -> Result<String> {
        self.prompt_on(&current_output())
    }

    /// Prompt the user through the given output.
    pub fn prompt_on(self, output: &Output) -> Result<String> {
        let query = Components::new(output).ask(&self.label);
        let options: SelectOptions = (self.options)(&query).into();
        select(self.label, options).prompt_on(output)
    }
}

/// Wait for the user to press ENTER.
pub fn pause(message: impl Into<String>) -> bool {
    let output = current_output();
    if output.is_interactive() {
        Components::new(&output).ask(message.into());
    }
    true
}

// ----------------------------------------------------------------------
// Spin & progress
// ----------------------------------------------------------------------

/// Display a message while the future runs, returning its output.
///
/// ```no_run
/// # async fn example() {
/// use illuminate_console::prompts::spin;
///
/// let response = spin(async { "fetched" }, "Fetching response...").await;
/// # }
/// ```
pub async fn spin<F: Future>(future: F, message: impl AsRef<str>) -> F::Output {
    let output = current_output();
    output.writeln(format!(" <fg=cyan>⠂</> {}", message.as_ref()));
    future.await
}

/// A progress bar with a label.
#[derive(Clone)]
pub struct Progress {
    label: Arc<Mutex<String>>,
    hint: Arc<Mutex<String>>,
    bar: ProgressBar,
    output: Output,
}

impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Progress")
            .field("label", &self.label.lock().unwrap())
            .finish()
    }
}

impl Progress {
    /// Create a progress bar with the given number of steps.
    pub fn new(label: impl Into<String>, steps: usize) -> Self {
        let output = current_output();
        Self {
            label: Arc::new(Mutex::new(label.into())),
            hint: Arc::new(Mutex::new(String::new())),
            bar: ProgressBar::new(&output, steps),
            output,
        }
    }

    /// Change the label.
    pub fn label(&self, label: impl Into<String>) -> &Self {
        *self.label.lock().unwrap() = label.into();
        self
    }

    /// Change the hint.
    pub fn hint(&self, hint: impl Into<String>) -> &Self {
        *self.hint.lock().unwrap() = hint.into();
        self
    }

    /// Start the progress bar.
    pub fn start(&self) {
        self.output.auto_prepend_block();
        self.output
            .writeln(format!(" {}", self.label.lock().unwrap()));
        self.bar.start();
    }

    /// Advance the progress bar.
    pub fn advance(&self) {
        self.bar.advance();
    }

    /// Finish the progress bar.
    pub fn finish(&self) {
        self.bar.finish();
        self.output.new_line(1);
        let hint = self.hint.lock().unwrap().clone();
        if !hint.is_empty() {
            self.output.writeln(format!(" <fg=gray>{hint}</>"));
        }
    }
}

/// Run the callback for each step while displaying a progress bar,
/// returning the callback's results (like a `map`).
pub fn progress<I, T, R, F>(label: impl Into<String>, steps: I, mut callback: F) -> Vec<R>
where
    I: IntoIterator<Item = T>,
    F: FnMut(T, &Progress) -> R,
{
    let steps: Vec<T> = steps.into_iter().collect();
    let progress = Progress::new(label, steps.len());

    progress.start();
    let results = steps
        .into_iter()
        .map(|step| {
            let result = callback(step, &progress);
            progress.advance();
            result
        })
        .collect();
    progress.finish();

    results
}

// ----------------------------------------------------------------------
// Informational messages
// ----------------------------------------------------------------------

fn render_note(message: &str, style: Option<&str>, padded: bool) {
    let output = current_output();
    output.auto_prepend_block();

    for line in message.lines() {
        let line = if padded {
            format!(" {line} ")
        } else {
            line.to_string()
        };
        match style {
            Some(style) => output.writeln(format!(" <{style}>{line}</>")),
            None => output.writeln(format!(" {line}")),
        }
    }

    output.new_line(1);
}

/// Display a note.
pub fn note(message: impl AsRef<str>) {
    render_note(message.as_ref(), None, false);
}

/// Display an informational message (green).
pub fn info(message: impl AsRef<str>) {
    render_note(message.as_ref(), Some("fg=green"), false);
}

/// Display a warning (yellow).
pub fn warning(message: impl AsRef<str>) {
    render_note(message.as_ref(), Some("fg=yellow"), false);
}

/// Display an error (red).
pub fn error(message: impl AsRef<str>) {
    render_note(message.as_ref(), Some("fg=red"), false);
}

/// Display an alert (white on red).
pub fn alert(message: impl AsRef<str>) {
    render_note(message.as_ref(), Some("fg=white;bg=red"), true);
}

/// Display an introduction (black on cyan).
pub fn intro(message: impl AsRef<str>) {
    render_note(message.as_ref(), Some("fg=black;bg=cyan"), true);
}

/// Display a closing message (black on cyan).
pub fn outro(message: impl AsRef<str>) {
    render_note(message.as_ref(), Some("fg=black;bg=cyan"), true);
}

/// Display a table.
pub fn table<H, HS, R, C, S>(headers: H, rows: R)
where
    H: IntoIterator<Item = HS>,
    HS: ToString,
    R: IntoIterator<Item = C>,
    C: IntoIterator<Item = S>,
    S: ToString,
{
    let output = current_output();
    output.auto_prepend_block();

    for line in Table::new()
        .headers(headers)
        .rows(rows)
        .style("box")
        .lines()
    {
        output.writeln(format!(" {line}"));
    }

    output.new_line(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripted(lines: &[&str]) -> Output {
        Output::buffered().with_input(lines.iter().copied())
    }

    #[test]
    fn text_prompts_return_answers() {
        let output = scripted(&["Taylor"]);
        assert_eq!(text("Name?").prompt_on(&output).unwrap(), "Taylor");

        let output = scripted(&[""]);
        assert_eq!(
            text("Name?").default("Abigail").prompt_on(&output).unwrap(),
            "Abigail"
        );
    }

    #[test]
    fn required_text_prompts_fail_on_scripted_input() {
        let output = scripted(&[""]);
        let error = text("Name?").required(true).prompt_on(&output).unwrap_err();
        assert_eq!(error.to_string(), "Required.");
        assert!(output.contents().contains("ERROR  Required."));

        let output = scripted(&["Al"]);
        let error = text("Name?")
            .validate(|value| (value.len() < 3).then(|| "Too short.".to_string()))
            .prompt_on(&output)
            .unwrap_err();
        assert_eq!(error.to_string(), "Too short.");
    }

    #[test]
    fn non_interactive_prompts_use_defaults() {
        let output = Output::buffered();
        assert_eq!(
            text("Name?").default("Taylor").prompt_on(&output).unwrap(),
            "Taylor"
        );
        assert!(confirm("Sure?").prompt_on(&output).unwrap());
        assert_eq!(
            select("Role", ["Member", "Owner"])
                .prompt_on(&output)
                .unwrap(),
            "Member"
        );
        assert_eq!(
            select("Role", ["Member", "Owner"])
                .default("Owner")
                .prompt_on(&output)
                .unwrap(),
            "Owner"
        );
        assert!(text("Name?").required(true).prompt_on(&output).is_err());
        assert_eq!(output.contents(), "");
    }

    #[test]
    fn confirm_prompts_can_require_yes() {
        let output = scripted(&["no"]);
        let error = confirm("Accept?")
            .required_with("You must accept.")
            .prompt_on(&output)
            .unwrap_err();
        assert_eq!(error.to_string(), "You must accept.");

        let output = scripted(&["y"]);
        assert!(
            confirm("Accept?")
                .default(false)
                .prompt_on(&output)
                .unwrap()
        );
    }

    #[test]
    fn select_prompts_return_labels_or_keys() {
        let output = scripted(&["Owner"]);
        assert_eq!(
            select("Role", ["Member", "Owner"])
                .prompt_on(&output)
                .unwrap(),
            "Owner"
        );

        let output = scripted(&["1"]);
        assert_eq!(
            select("Role", ["Member", "Owner"])
                .prompt_on(&output)
                .unwrap(),
            "Owner"
        );

        let output = scripted(&["Contributor"]);
        let role = select(
            "Role",
            [("member", "Member"), ("contributor", "Contributor")],
        )
        .prompt_on(&output)
        .unwrap();
        assert_eq!(role, "contributor");

        let output = scripted(&[""]);
        let role = select("Role", [("member", "Member"), ("owner", "Owner")])
            .default("owner")
            .prompt_on(&output)
            .unwrap();
        assert_eq!(role, "owner");
    }

    #[test]
    fn multiselect_prompts_return_several_values() {
        let output = scripted(&["Read, 2"]);
        let permissions = multiselect("Permissions", ["Read", "Create", "Delete"])
            .prompt_on(&output)
            .unwrap();
        assert_eq!(permissions, vec!["Read", "Delete"]);

        let output = scripted(&[""]);
        let permissions = multiselect("Permissions", ["Read", "Create"])
            .prompt_on(&output)
            .unwrap();
        assert!(permissions.is_empty());

        let output = scripted(&[""]);
        let error = multiselect("Permissions", ["Read"])
            .required(true)
            .prompt_on(&output)
            .unwrap_err();
        assert_eq!(error.to_string(), "Required.");

        let output = Output::buffered();
        let permissions = multiselect("Permissions", ["Read", "Create"])
            .default(["Create"])
            .prompt_on(&output)
            .unwrap();
        assert_eq!(permissions, vec!["Create"]);
    }

    #[test]
    fn search_prompts_filter_options() {
        let output = scripted(&["Tay", "Taylor Otwell"]);
        let name = search("Search for a user", |query: &str| {
            ["Taylor Otwell", "Abigail Otwell", "Taylor Swift"]
                .into_iter()
                .filter(|name| name.starts_with(query))
                .collect::<Vec<_>>()
        })
        .prompt_on(&output)
        .unwrap();
        assert_eq!(name, "Taylor Otwell");
    }

    #[tokio::test]
    async fn notes_render_to_the_current_output() {
        let output = Output::buffered();
        with_output(output.clone(), async {
            info("Package installed.");
            intro("Welcome");
            table(["Name"], [["Taylor"]]);
            let value = spin(async { 42 }, "Thinking...").await;
            assert_eq!(value, 42);
            let doubled = progress("Doubling", [1, 2], |n, _| n * 2);
            assert_eq!(doubled, vec![2, 4]);
        })
        .await;

        let text = output.contents();
        assert!(text.contains(" Package installed.\n"));
        assert!(text.contains("  Welcome \n"));
        assert!(text.contains(" │ Name   │\n"));
        assert!(text.contains(" ⠂ Thinking...\n"));
        assert!(text.contains(" Doubling\n"));
        assert!(text.contains(" 2/2 [============================] 100%"));
    }
}
