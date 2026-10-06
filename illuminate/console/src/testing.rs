//! Testing console commands: Laravel's `$this->artisan(...)`.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_console::{testing::artisan, Artisan};
//! use illuminate_container::Container;
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container);
//!
//! Artisan::command("question", |cmd| async move {
//!     let name = cmd.ask("What is your name?");
//!     let language = cmd.choice("Which language do you prefer?", ["PHP", "Ruby", "Python"], None);
//!     cmd.line(format!("Your name is {name} and you prefer {language}."));
//!     Ok(())
//! });
//!
//! artisan("question")
//!     .expects_question("What is your name?", "Taylor Otwell")
//!     .expects_question("Which language do you prefer?", "PHP")
//!     .expects_output("Your name is Taylor Otwell and you prefer PHP.")
//!     .doesnt_expect_output("Your name is Taylor Otwell and you prefer Ruby.")
//!     .assert_exit_code(0)
//!     .await;
//! # }
//! ```

use std::collections::{HashMap, VecDeque};
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::application::Application;
use crate::facades::Artisan;
use crate::input::ArtisanArgs;
use crate::output::{InputReader, Output, Question};
use crate::table::Table;

/// Start testing an Artisan command, using the application bound in the
/// current container.
pub fn artisan(command: impl Into<String>) -> PendingCommand {
    PendingCommand::new(Artisan::application(), command)
}

struct ExpectedQuestion {
    question: String,
    answer: String,
}

#[derive(Default)]
struct Interaction {
    expected: VecDeque<ExpectedQuestion>,
    failures: Vec<String>,
    actual_choices: HashMap<String, Vec<String>>,
}

/// Answers questions with the answers expected by a [`PendingCommand`].
#[derive(Default)]
struct ExpectedAnswers {
    interaction: Mutex<Interaction>,
}

impl InputReader for ExpectedAnswers {
    fn read(&self, question: &Question) -> Option<String> {
        let mut interaction = self.interaction.lock().unwrap();

        let matches = interaction
            .expected
            .front()
            .is_some_and(|expected| expected.question == question.text);

        if !matches {
            interaction.failures.push(format!(
                "Unexpected question \"{}\" was asked.",
                question.text
            ));
            return None;
        }

        let choices = match question.choice_labels() {
            labels if labels.is_empty() => question.suggestions.clone(),
            labels => labels,
        };
        interaction
            .actual_choices
            .insert(question.text.clone(), choices);

        interaction
            .expected
            .pop_front()
            .map(|expected| expected.answer)
    }

    fn is_scripted(&self) -> bool {
        true
    }

    fn renders_prompts(&self) -> bool {
        false
    }
}

/// The result of a tested command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    /// The command's exit code.
    pub exit_code: i32,
    /// Everything the command wrote.
    pub output: String,
}

/// A command under test, with its expectations. Await it (or call
/// [`PendingCommand::run`]) to run the command and verify the expectations.
#[must_use = "a pending command does nothing until it is awaited"]
pub struct PendingCommand {
    application: Arc<Application>,
    command: String,
    args: ArtisanArgs,
    questions: Vec<(String, String)>,
    choices: Vec<(String, Vec<String>, bool)>,
    expected_output: Vec<String>,
    unexpected_output: Vec<String>,
    expected_substrings: Vec<String>,
    unexpected_substrings: Vec<String>,
    expects_output: Option<bool>,
    expected_exit_code: Option<i32>,
    unexpected_exit_code: Option<i32>,
    width: usize,
}

impl PendingCommand {
    /// Create a pending command for the given application.
    pub fn new(application: Arc<Application>, command: impl Into<String>) -> Self {
        Self {
            application,
            command: command.into(),
            args: ArtisanArgs::new(),
            questions: Vec::new(),
            choices: Vec::new(),
            expected_output: Vec::new(),
            unexpected_output: Vec::new(),
            expected_substrings: Vec::new(),
            unexpected_substrings: Vec::new(),
            expects_output: None,
            expected_exit_code: None,
            unexpected_exit_code: None,
            width: 80,
        }
    }

    /// Pass parameters to the command (Laravel's `$this->artisan('cmd', [...])`).
    pub fn with_args(mut self, args: impl Into<ArtisanArgs>) -> Self {
        self.args = args.into();
        self
    }

    /// Render the output as if the terminal had the given width.
    pub fn with_terminal_width(mut self, width: usize) -> Self {
        self.width = width;
        self
    }

    /// Specify a question that should be asked when the command runs, and
    /// the answer to give.
    pub fn expects_question(
        mut self,
        question: impl Into<String>,
        answer: impl Into<String>,
    ) -> Self {
        self.questions.push((question.into(), answer.into()));
        self
    }

    /// Specify a confirmation question that should be asked, answered with
    /// `"yes"` or `"no"`.
    pub fn expects_confirmation(self, question: impl Into<String>, answer: &str) -> Self {
        let answer = if answer.eq_ignore_ascii_case("yes") {
            "yes"
        } else {
            "no"
        };
        self.expects_question(question, answer)
    }

    /// Specify a choice question that should be asked, its expected options
    /// (in any order), and the answer to give.
    pub fn expects_choice<I, S>(
        mut self,
        question: impl Into<String>,
        answer: impl Into<String>,
        answers: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let question = question.into();
        self.choices.push((
            question.clone(),
            answers.into_iter().map(Into::into).collect(),
            false,
        ));
        self.expects_question(question, answer)
    }

    /// Like [`PendingCommand::expects_choice`], but the options must be in order.
    pub fn expects_choice_strict<I, S>(
        mut self,
        question: impl Into<String>,
        answer: impl Into<String>,
        answers: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let question = question.into();
        self.choices.push((
            question.clone(),
            answers.into_iter().map(Into::into).collect(),
            true,
        ));
        self.expects_question(question, answer)
    }

    /// Specify a search that should be performed: the query typed, the
    /// options found, and the option selected.
    pub fn expects_search<I, S>(
        self,
        question: impl Into<String>,
        answer: impl Into<String>,
        search: impl Into<String>,
        answers: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let question = question.into();
        self.expects_question(question.clone(), search)
            .expects_choice(question, answer, answers)
    }

    /// Specify output that should be printed when the command runs.
    pub fn expects_output(mut self, output: impl Into<String>) -> Self {
        self.expected_output.push(output.into());
        self
    }

    /// Expect the command to print something.
    pub fn expects_any_output(mut self) -> Self {
        self.expects_output = Some(true);
        self
    }

    /// Specify output that should never be printed.
    pub fn doesnt_expect_output(mut self, output: impl Into<String>) -> Self {
        self.unexpected_output.push(output.into());
        self
    }

    /// Expect the command to print nothing at all.
    pub fn doesnt_expect_any_output(mut self) -> Self {
        self.expects_output = Some(false);
        self
    }

    /// Specify that the output should contain the given string.
    pub fn expects_output_to_contain(mut self, text: impl Into<String>) -> Self {
        self.expected_substrings.push(text.into());
        self
    }

    /// Specify that the output should not contain the given string.
    pub fn doesnt_expect_output_to_contain(mut self, text: impl Into<String>) -> Self {
        self.unexpected_substrings.push(text.into());
        self
    }

    /// Specify a table that should be printed.
    pub fn expects_table<H, HS, R, C, S>(self, headers: H, rows: R) -> Self
    where
        H: IntoIterator<Item = HS>,
        HS: ToString,
        R: IntoIterator<Item = C>,
        C: IntoIterator<Item = S>,
        S: ToString,
    {
        self.expects_table_with_style(headers, rows, "default")
    }

    /// Specify a table, rendered with the given style, that should be printed.
    pub fn expects_table_with_style<H, HS, R, C, S>(
        mut self,
        headers: H,
        rows: R,
        style: &str,
    ) -> Self
    where
        H: IntoIterator<Item = HS>,
        HS: ToString,
        R: IntoIterator<Item = C>,
        C: IntoIterator<Item = S>,
        S: ToString,
    {
        let table = Table::new()
            .headers(headers)
            .rows(rows)
            .style(style)
            .render_to_string();
        for line in table.lines().filter(|line| !line.is_empty()) {
            self.expected_output.push(line.to_string());
        }
        self
    }

    /// Assert that the command exits with the given code.
    pub fn assert_exit_code(mut self, code: i32) -> Self {
        self.expected_exit_code = Some(code);
        self
    }

    /// Assert that the command does not exit with the given code.
    pub fn assert_not_exit_code(mut self, code: i32) -> Self {
        self.unexpected_exit_code = Some(code);
        self
    }

    /// Assert that the command exits successfully.
    pub fn assert_successful(self) -> Self {
        self.assert_exit_code(crate::SUCCESS)
    }

    /// Alias of [`PendingCommand::assert_successful`].
    pub fn assert_ok(self) -> Self {
        self.assert_successful()
    }

    /// Assert that the command does not exit successfully.
    pub fn assert_failed(self) -> Self {
        self.assert_not_exit_code(crate::SUCCESS)
    }

    /// Run the command and verify every expectation.
    ///
    /// # Panics
    ///
    /// Panics (failing the test) when an expectation isn't met.
    pub async fn run(self) -> CommandResult {
        let reader = Arc::new(ExpectedAnswers::default());
        {
            let mut interaction = reader.interaction.lock().unwrap();
            for (question, answer) in &self.questions {
                interaction.expected.push_back(ExpectedQuestion {
                    question: question.clone(),
                    answer: answer.clone(),
                });
            }
        }

        let output = Output::buffered_with_width(self.width).with_reader(reader.clone());

        let result = self
            .application
            .call_with_output(&self.command, self.args.clone(), &output)
            .await;

        let mut error = None;
        let exit_code = match result {
            Ok(code) => code,
            Err(failure) => {
                error = Some(format!("{failure:#}"));
                self.application.render_exception(&failure, &output)
            }
        };

        let text = output.contents();
        let interaction = std::mem::take(&mut *reader.interaction.lock().unwrap());
        let messages = output.messages();

        let fail = |message: String| fail(message, &text, error.as_deref());

        if let Some(failure) = interaction.failures.first() {
            fail(failure.clone());
        }

        if let Some(expected) = self.expected_exit_code {
            if expected != exit_code {
                fail(format!(
                    "Expected status code {expected} but received {exit_code}."
                ));
            }
        } else if let Some(unexpected) = self.unexpected_exit_code
            && unexpected == exit_code
        {
            fail(format!("Unexpected status code {unexpected} was received."));
        }

        if let Some(question) = interaction.expected.front() {
            fail(format!("Question \"{}\" was not asked.", question.question));
        }

        for (question, expected, strict) in &self.choices {
            let mut actual = interaction
                .actual_choices
                .get(question)
                .cloned()
                .unwrap_or_default();
            let mut expected = expected.clone();
            if !strict {
                actual.sort();
                expected.sort();
            }
            if actual != expected {
                fail(format!("Question \"{question}\" has different options."));
            }
        }

        let printed = |expected: &str| {
            messages.iter().any(|message| message == expected)
                || text
                    .lines()
                    .any(|line| line.trim_end() == expected.trim_end())
        };

        for expected in &self.expected_output {
            if !printed(expected) {
                fail(format!("Output \"{expected}\" was not printed."));
            }
        }

        for expected in &self.expected_substrings {
            if !text.contains(expected.as_str()) {
                fail(format!("Output does not contain \"{expected}\"."));
            }
        }

        for unexpected in &self.unexpected_output {
            if printed(unexpected) {
                fail(format!("Output \"{unexpected}\" was printed."));
            }
        }

        for unexpected in &self.unexpected_substrings {
            if text.contains(unexpected.as_str()) {
                fail(format!("Output \"{unexpected}\" was printed."));
            }
        }

        match self.expects_output {
            Some(false) if !text.is_empty() => {
                fail("Expected no output, but output was printed.".to_string())
            }
            Some(true) if text.is_empty() => {
                fail("Expected output, but nothing was printed.".to_string())
            }
            _ => {}
        }

        CommandResult {
            exit_code,
            output: text,
        }
    }
}

fn fail(message: String, output: &str, error: Option<&str>) -> ! {
    let mut message = format!("{message}\n\nThe command's output was:\n{output}");
    if let Some(error) = error {
        message.push_str(&format!("\nThe command failed with: {error}"));
    }
    panic!("{message}");
}

impl IntoFuture for PendingCommand {
    type Output = CommandResult;
    type IntoFuture = BoxFuture<'static, CommandResult>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}
