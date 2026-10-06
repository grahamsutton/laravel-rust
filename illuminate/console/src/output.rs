//! Console output (and input): where commands write their messages and
//! read their answers.
//!
//! An [`Output`] is a cheap, cloneable handle. It writes to the terminal,
//! to an in-memory buffer (perfect for `Artisan::call` and tests), or to
//! nowhere at all, and it knows how to ask the user questions.
//!
//! ```
//! use illuminate_console::Output;
//!
//! let output = Output::buffered();
//!
//! output.writeln("<info>Hello</info>, world!");
//!
//! assert_eq!(output.fetch(), "Hello, world!\n");
//! ```

use std::collections::VecDeque;
use std::io::{BufRead, IsTerminal, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::formatter::OutputFormatter;

/// How chatty the output should be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum Verbosity {
    /// Nothing at all is written (`--silent`).
    Silent = 8,
    /// Only errors are written (`-q`, `--quiet`).
    Quiet = 16,
    /// The default verbosity.
    Normal = 32,
    /// Increased verbosity (`-v`).
    Verbose = 64,
    /// Informative non-essential messages (`-vv`).
    VeryVerbose = 128,
    /// Debug messages (`-vvv`).
    Debug = 256,
}

impl Verbosity {
    fn from_u16(value: u16) -> Self {
        match value {
            8 => Verbosity::Silent,
            16 => Verbosity::Quiet,
            64 => Verbosity::Verbose,
            128 => Verbosity::VeryVerbose,
            256 => Verbosity::Debug,
            _ => Verbosity::Normal,
        }
    }
}

impl FromStr for Verbosity {
    type Err = String;

    /// Parse Laravel's verbosity names: `v`, `vv`, `vvv`, `quiet`, `normal`.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "v" | "verbose" => Ok(Verbosity::Verbose),
            "vv" | "very_verbose" => Ok(Verbosity::VeryVerbose),
            "vvv" | "debug" => Ok(Verbosity::Debug),
            "quiet" => Ok(Verbosity::Quiet),
            "silent" => Ok(Verbosity::Silent),
            "normal" | "" => Ok(Verbosity::Normal),
            other => Err(format!("Unknown verbosity [{other}].")),
        }
    }
}

// ----------------------------------------------------------------------
// Questions
// ----------------------------------------------------------------------

/// The kind of question being asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionKind {
    /// A free-form text answer.
    Text,
    /// A hidden answer, such as a password.
    Secret,
    /// A yes / no confirmation.
    Confirm,
    /// A choice between several `(key, label)` options.
    Choice {
        /// The available options as `(key, label)` pairs.
        choices: Vec<(String, String)>,
        /// Whether several options may be selected (comma separated).
        multiple: bool,
    },
}

/// A question asked through the console.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    /// The question text, e.g. "What is your name?".
    pub text: String,
    /// What kind of answer is expected.
    pub kind: QuestionKind,
    /// The default answer, as displayed to the user.
    pub default: Option<String>,
    /// Whether the answer may span several lines.
    pub multiline: bool,
    /// Suggested answers (auto-completion values).
    pub suggestions: Vec<String>,
}

impl Question {
    /// Create a simple text question.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: QuestionKind::Text,
            default: None,
            multiline: false,
            suggestions: Vec::new(),
        }
    }

    /// Set the question kind.
    pub fn kind(mut self, kind: QuestionKind) -> Self {
        self.kind = kind;
        self
    }

    /// Set the default answer.
    pub fn default(mut self, default: Option<String>) -> Self {
        self.default = default;
        self
    }

    /// The choice labels, if this is a choice question.
    pub fn choice_labels(&self) -> Vec<String> {
        match &self.kind {
            QuestionKind::Choice { choices, .. } => choices.iter().map(|(_, label)| label.clone()).collect(),
            _ => Vec::new(),
        }
    }
}

/// The visual style used to render a question prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptStyle {
    /// Symfony's style, used by `$this->ask()`: ` What is your name?:` / ` > `.
    Symfony,
    /// Laravel's console components style: `  What is your name?` / `❯ `.
    Components,
}

/// Something that can answer questions: the keyboard, a list of
/// pre-recorded answers, or a test double.
pub trait InputReader: Send + Sync + 'static {
    /// Read the answer to the given question. `None` means there is no more
    /// input available (end of file), in which case defaults are used.
    fn read(&self, question: &Question) -> Option<String>;

    /// Whether the answers are scripted (tests, pre-recorded input) rather
    /// than typed by a human. Prompts don't retry invalid scripted answers.
    fn is_scripted(&self) -> bool {
        false
    }

    /// Whether the question prompt should be written to the output.
    fn renders_prompts(&self) -> bool {
        true
    }
}

/// Reads answers from the process' standard input.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdinReader;

impl InputReader for StdinReader {
    fn read(&self, question: &Question) -> Option<String> {
        let stdin = std::io::stdin();

        if question.multiline {
            let mut answer = String::new();
            for line in stdin.lock().lines() {
                let line = line.ok()?;
                if !answer.is_empty() {
                    answer.push('\n');
                }
                answer.push_str(&line);
            }
            return Some(answer);
        }

        let hide = question.kind == QuestionKind::Secret && stdin.is_terminal();
        if hide {
            set_terminal_echo(false);
        }

        let mut line = String::new();
        let read = stdin.lock().read_line(&mut line);

        if hide {
            set_terminal_echo(true);
            println!();
        }

        match read {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
        }
    }
}

fn set_terminal_echo(enabled: bool) {
    let _ = std::process::Command::new("stty")
        .arg(if enabled { "echo" } else { "-echo" })
        .stdin(std::process::Stdio::inherit())
        .status();
}

/// Answers questions from a pre-recorded list of lines, in order.
#[derive(Debug, Default)]
pub struct LinesReader {
    lines: Mutex<VecDeque<String>>,
}

impl LinesReader {
    /// Create a reader that answers with the given lines.
    pub fn new<I, S>(lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            lines: Mutex::new(lines.into_iter().map(Into::into).collect()),
        }
    }
}

impl InputReader for LinesReader {
    fn read(&self, _question: &Question) -> Option<String> {
        self.lines.lock().unwrap().pop_front()
    }

    fn is_scripted(&self) -> bool {
        true
    }
}

// ----------------------------------------------------------------------
// The output
// ----------------------------------------------------------------------

enum Sink {
    Stdout,
    Buffer(Mutex<Buffer>),
    Null,
}

#[derive(Default)]
struct Buffer {
    text: String,
    chunks: Vec<String>,
}

struct Inner {
    sink: Sink,
    decorated: AtomicBool,
    verbosity: AtomicU16,
    interactive: AtomicBool,
    width: Option<usize>,
    newlines: AtomicUsize,
    reader: RwLock<Arc<dyn InputReader>>,
}

/// Where console messages are written, and where answers are read from.
#[derive(Clone)]
pub struct Output {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sink = match self.inner.sink {
            Sink::Stdout => "stdout",
            Sink::Buffer(_) => "buffer",
            Sink::Null => "null",
        };
        f.debug_struct("Output")
            .field("sink", &sink)
            .field("decorated", &self.is_decorated())
            .field("verbosity", &self.verbosity())
            .field("interactive", &self.is_interactive())
            .finish()
    }
}

impl Output {
    fn build(sink: Sink, decorated: bool, interactive: bool, width: Option<usize>, reader: Arc<dyn InputReader>) -> Self {
        Self {
            inner: Arc::new(Inner {
                sink,
                decorated: AtomicBool::new(decorated),
                verbosity: AtomicU16::new(Verbosity::Normal as u16),
                interactive: AtomicBool::new(interactive),
                width,
                newlines: AtomicUsize::new(1),
                reader: RwLock::new(reader),
            }),
        }
    }

    /// Output to the terminal. ANSI decoration and interactivity are
    /// detected automatically.
    pub fn stdout() -> Self {
        Self::build(
            Sink::Stdout,
            stdout_supports_color(),
            std::io::stdin().is_terminal(),
            None,
            Arc::new(StdinReader),
        )
    }

    /// Output into an in-memory buffer (undecorated, 80 columns wide,
    /// non-interactive). Read it back with [`Output::fetch`].
    pub fn buffered() -> Self {
        Self::buffered_with_width(80)
    }

    /// Output into an in-memory buffer with the given terminal width.
    pub fn buffered_with_width(width: usize) -> Self {
        Self::build(
            Sink::Buffer(Mutex::new(Buffer::default())),
            false,
            false,
            Some(width),
            Arc::new(LinesReader::default()),
        )
    }

    /// Output that discards everything written to it.
    pub fn null() -> Self {
        Self::build(Sink::Null, false, false, Some(80), Arc::new(LinesReader::default()))
    }

    /// A silent copy of this output that still answers questions the same way.
    pub fn silenced(&self) -> Self {
        let output = Self::build(
            Sink::Null,
            false,
            self.is_interactive(),
            Some(self.width()),
            self.reader(),
        );
        output.set_verbosity(Verbosity::Silent);
        output
    }

    /// Feed the given lines as answers to any questions (making the output
    /// interactive).
    pub fn with_input<I, S>(self, lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.set_reader(Arc::new(LinesReader::new(lines)));
        self.set_interactive(true);
        self
    }

    /// Answer questions using the given reader (making the output interactive).
    pub fn with_reader(self, reader: Arc<dyn InputReader>) -> Self {
        self.set_reader(reader);
        self.set_interactive(true);
        self
    }

    /// Use the given verbosity.
    pub fn with_verbosity(self, verbosity: Verbosity) -> Self {
        self.set_verbosity(verbosity);
        self
    }

    /// Force (or disable) ANSI decoration.
    pub fn with_decoration(self, decorated: bool) -> Self {
        self.set_decorated(decorated);
        self
    }

    // ------------------------------------------------------------------
    // Settings
    // ------------------------------------------------------------------

    /// Determine if the output is decorated with ANSI escape sequences.
    pub fn is_decorated(&self) -> bool {
        self.inner.decorated.load(Ordering::Relaxed)
    }

    /// Enable or disable ANSI decoration.
    pub fn set_decorated(&self, decorated: bool) {
        self.inner.decorated.store(decorated, Ordering::Relaxed);
    }

    /// The current verbosity.
    pub fn verbosity(&self) -> Verbosity {
        Verbosity::from_u16(self.inner.verbosity.load(Ordering::Relaxed))
    }

    /// Set the verbosity.
    pub fn set_verbosity(&self, verbosity: Verbosity) {
        self.inner.verbosity.store(verbosity as u16, Ordering::Relaxed);
    }

    /// Determine if the output is quiet (or silent).
    pub fn is_quiet(&self) -> bool {
        self.verbosity() <= Verbosity::Quiet
    }

    /// Determine if the output is silent.
    pub fn is_silent(&self) -> bool {
        self.verbosity() <= Verbosity::Silent
    }

    /// Determine if the verbosity is at least verbose (`-v`).
    pub fn is_verbose(&self) -> bool {
        self.verbosity() >= Verbosity::Verbose
    }

    /// Determine if the verbosity is at least very verbose (`-vv`).
    pub fn is_very_verbose(&self) -> bool {
        self.verbosity() >= Verbosity::VeryVerbose
    }

    /// Determine if the verbosity is debug (`-vvv`).
    pub fn is_debug(&self) -> bool {
        self.verbosity() >= Verbosity::Debug
    }

    /// Determine if questions may be asked.
    pub fn is_interactive(&self) -> bool {
        self.inner.interactive.load(Ordering::Relaxed)
    }

    /// Allow (or disallow) questions. Non-interactive outputs answer every
    /// question with its default.
    pub fn set_interactive(&self, interactive: bool) {
        self.inner.interactive.store(interactive, Ordering::Relaxed);
    }

    /// The reader used to answer questions.
    pub fn reader(&self) -> Arc<dyn InputReader> {
        self.inner.reader.read().unwrap().clone()
    }

    /// Replace the reader used to answer questions.
    pub fn set_reader(&self, reader: Arc<dyn InputReader>) {
        *self.inner.reader.write().unwrap() = reader;
    }

    /// The width of the terminal, in columns.
    pub fn width(&self) -> usize {
        self.inner.width.unwrap_or_else(terminal_width)
    }

    /// The number of trailing new lines most recently written.
    pub fn new_lines_written(&self) -> usize {
        self.inner.newlines.load(Ordering::Relaxed)
    }

    // ------------------------------------------------------------------
    // Writing
    // ------------------------------------------------------------------

    /// Write a message without a trailing new line.
    pub fn write(&self, message: impl AsRef<str>) {
        self.write_with(message.as_ref(), false, Verbosity::Normal);
    }

    /// Write a message followed by a new line.
    pub fn writeln(&self, message: impl AsRef<str>) {
        self.write_with(message.as_ref(), true, Verbosity::Normal);
    }

    /// Write a message if the output's verbosity allows it.
    pub fn write_with(&self, message: &str, newline: bool, verbosity: Verbosity) {
        if verbosity > self.verbosity() {
            return;
        }

        let formatted = OutputFormatter::format(message, self.is_decorated());

        let trailing = trailing_newlines(&formatted);
        self.inner
            .newlines
            .store(trailing + usize::from(newline), Ordering::Relaxed);

        self.do_write(formatted, newline);
    }

    /// Write blank lines.
    pub fn new_line(&self, count: usize) {
        self.inner.newlines.fetch_add(count, Ordering::Relaxed);

        if Verbosity::Normal > self.verbosity() {
            return;
        }

        self.do_write("\n".repeat(count), false);
    }

    fn do_write(&self, mut text: String, newline: bool) {
        match &self.inner.sink {
            Sink::Null => {}
            Sink::Stdout => {
                if newline {
                    text.push('\n');
                }
                let mut stdout = std::io::stdout().lock();
                let _ = stdout.write_all(text.as_bytes());
                let _ = stdout.flush();
            }
            Sink::Buffer(buffer) => {
                let mut buffer = buffer.lock().unwrap();
                buffer.text.push_str(&text);
                if newline {
                    buffer.text.push('\n');
                }
                buffer.chunks.push(text);
            }
        }
    }

    /// Take everything written to a buffered output so far, emptying it.
    pub fn fetch(&self) -> String {
        match &self.inner.sink {
            Sink::Buffer(buffer) => {
                let mut buffer = buffer.lock().unwrap();
                buffer.chunks.clear();
                std::mem::take(&mut buffer.text)
            }
            _ => String::new(),
        }
    }

    /// Everything written to a buffered output so far.
    pub fn contents(&self) -> String {
        match &self.inner.sink {
            Sink::Buffer(buffer) => buffer.lock().unwrap().text.clone(),
            _ => String::new(),
        }
    }

    /// Every individual message written to a buffered output so far.
    pub fn messages(&self) -> Vec<String> {
        match &self.inner.sink {
            Sink::Buffer(buffer) => buffer.lock().unwrap().chunks.clone(),
            _ => Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // Asking questions
    // ------------------------------------------------------------------

    /// Make sure a blank line precedes the next block of output.
    pub(crate) fn auto_prepend_block(&self) {
        let written = self.new_lines_written();
        if written < 2 {
            self.new_line(2 - written);
        }
    }

    /// Ask a question, returning the raw answer (or `None` when the
    /// output is not interactive or no input remains).
    pub fn ask_question(&self, question: &Question, style: PromptStyle) -> Option<String> {
        if !self.is_interactive() {
            return None;
        }

        let reader = self.reader();
        let renders = reader.renders_prompts();

        if renders {
            self.auto_prepend_block();
            self.write_prompt(question, style);
        }

        let answer = reader.read(question);

        if renders {
            self.new_line(1);
            self.inner.newlines.fetch_add(1, Ordering::Relaxed);
        }

        if answer.is_none() {
            // The input is exhausted, so every further question uses its default.
            self.set_interactive(false);
        }

        answer
    }

    fn write_prompt(&self, question: &Question, style: PromptStyle) {
        let default_label = question.default.as_ref().map(|default| match &question.kind {
            QuestionKind::Confirm => default.clone(),
            QuestionKind::Choice { choices, .. } => default
                .split(',')
                .map(|value| {
                    let value = value.trim();
                    choices
                        .iter()
                        .find(|(key, _)| key == value)
                        .map(|(_, label)| label.clone())
                        .unwrap_or_else(|| value.to_string())
                })
                .collect::<Vec<_>>()
                .join(", "),
            _ => default.clone(),
        });

        let escaped = OutputFormatter::escape(&question.text);
        let multiline = if question.multiline { " (press <comment>Ctrl+D</comment> to continue)" } else { "" };

        match style {
            PromptStyle::Symfony => {
                let text = match (&question.kind, default_label) {
                    (QuestionKind::Confirm, Some(default)) => {
                        format!(" <info>{escaped}{multiline} (yes/no)</info> [<comment>{default}</comment>]:")
                    }
                    (_, None) => format!(" <info>{escaped}{multiline}</info>:"),
                    (_, Some(default)) => format!(
                        " <info>{escaped}{multiline}</info> [<comment>{}</comment>]:",
                        OutputFormatter::escape(&default)
                    ),
                };
                self.writeln(text);

                if let QuestionKind::Choice { choices, .. } = &question.kind {
                    let width = choices.iter().map(|(key, _)| key.chars().count()).max().unwrap_or(0);
                    for (key, label) in choices {
                        self.writeln(format!(
                            "  [<comment>{key:<width$}</comment>] {}",
                            OutputFormatter::escape(label)
                        ));
                    }
                }

                self.write(" > ");
            }
            PromptStyle::Components => {
                let mut label = escaped;
                if !label.ends_with(['?', ':', '!', '.']) {
                    label.push(':');
                }
                let label = format!("  <options=bold>{label}</>{multiline}");

                let text = match (&question.kind, default_label) {
                    (QuestionKind::Confirm, Some(default)) => {
                        format!("<info>{label} (yes/no)</info> [<comment>{default}</comment>]")
                    }
                    (_, None) => format!("<info>{label}</info>"),
                    (_, Some(default)) => format!(
                        "<info>{label}</info> [<comment>{}</comment>]",
                        OutputFormatter::escape(&default)
                    ),
                };
                self.writeln(text);

                if let QuestionKind::Choice { choices, .. } = &question.kind {
                    for (key, label) in choices {
                        crate::components::render_two_column_detail(self, label, key, Verbosity::Normal);
                    }
                }

                self.write("<options=bold>❯ </>");
            }
        }
    }

    pub(crate) fn write_question_error(&self, message: &str) {
        self.auto_prepend_block();
        self.writeln(format!("<error> [ERROR] {} </error>", OutputFormatter::escape(message)));
        self.new_line(1);
    }

    /// Ask a free-form question, returning the default when no answer is given.
    pub fn ask_text(&self, question: &Question, style: PromptStyle) -> Option<String> {
        match self.ask_question(question, style) {
            Some(answer) => {
                let answer = if question.multiline { answer } else { answer.trim().to_string() };
                if answer.is_empty() { question.default.clone().or(Some(answer)) } else { Some(answer) }
            }
            None => question.default.clone(),
        }
    }

    /// Ask a yes / no question.
    pub fn ask_confirmation(&self, text: &str, default: bool, style: PromptStyle) -> bool {
        let question = Question::new(text)
            .kind(QuestionKind::Confirm)
            .default(Some(if default { "yes" } else { "no" }.to_string()));

        match self.ask_question(&question, style) {
            None => default,
            Some(answer) => {
                let answer = answer.trim();
                let is_yes = answer.to_ascii_lowercase().starts_with('y');
                if default { answer.is_empty() || is_yes } else { !answer.is_empty() && is_yes }
            }
        }
    }

    /// Ask a choice question, returning the selected labels (or keys, when
    /// `return_keys` is set). Invalid answers are rejected and asked again.
    pub fn ask_choice(&self, question: &Question, return_keys: bool, style: PromptStyle) -> Vec<String> {
        let (choices, multiple) = match &question.kind {
            QuestionKind::Choice { choices, multiple } => (choices.clone(), *multiple),
            _ => return self.ask_text(question, style).into_iter().collect(),
        };

        let resolve_default = || match &question.default {
            Some(default) => validate_choice(default, &choices, multiple, return_keys)
                .unwrap_or_else(|_| vec![default.clone()]),
            None => Vec::new(),
        };

        loop {
            let answer = match self.ask_question(question, style) {
                None => return resolve_default(),
                Some(answer) if answer.trim().is_empty() => match &question.default {
                    Some(_) => return resolve_default(),
                    None => answer,
                },
                Some(answer) => answer,
            };

            match validate_choice(&answer, &choices, multiple, return_keys) {
                Ok(selected) => return selected,
                Err(message) => self.write_question_error(&message),
            }
        }
    }
}

/// Validate a choice answer the way Symfony's `ChoiceQuestion` does: the
/// answer may be an option's key (index) or its label.
pub(crate) fn validate_choice(
    answer: &str,
    choices: &[(String, String)],
    multiple: bool,
    return_keys: bool,
) -> Result<Vec<String>, String> {
    let answer = answer.trim();

    let selected: Vec<&str> = if multiple {
        let valid = !answer.is_empty() && answer.split(',').all(|part| !part.is_empty());
        if !valid {
            return Err(format!("Value \"{answer}\" is invalid"));
        }
        answer.split(',').map(str::trim).collect()
    } else {
        vec![answer]
    };

    let mut results = Vec::new();

    for value in selected {
        let by_label: Vec<&(String, String)> = choices.iter().filter(|(_, label)| label == value).collect();

        if by_label.len() > 1 {
            let labels: Vec<&str> = choices.iter().map(|(_, label)| label.as_str()).collect();
            return Err(format!(
                "The provided answer is ambiguous. Value should be one of \"{}\".",
                labels.join("\" or \"")
            ));
        }

        let found = by_label
            .first()
            .copied()
            .or_else(|| choices.iter().find(|(key, _)| key == value));

        match found {
            Some((key, label)) => results.push(if return_keys { key.clone() } else { label.clone() }),
            None => return Err(format!("Value \"{value}\" is invalid")),
        }
    }

    Ok(results)
}

fn trailing_newlines(text: &str) -> usize {
    text.len() - text.trim_end_matches('\n').len()
}

fn stdout_supports_color() -> bool {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());

    if set("NO_COLOR") {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return false;
    }
    if set("FORCE_COLOR") {
        return true;
    }

    std::io::stdout().is_terminal()
}

/// The width of the terminal: `COLUMNS`, then `stty size`, then 80.
pub fn terminal_width() -> usize {
    static WIDTH: OnceLock<usize> = OnceLock::new();

    *WIDTH.get_or_init(|| {
        if let Some(columns) = std::env::var("COLUMNS").ok().and_then(|c| c.trim().parse::<usize>().ok()) {
            if columns > 0 {
                return columns;
            }
        }

        if std::io::stdout().is_terminal() {
            let size = std::process::Command::new("stty")
                .arg("size")
                .stdin(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::null())
                .output();

            if let Ok(size) = size {
                let text = String::from_utf8_lossy(&size.stdout);
                if let Some(columns) = text.split_whitespace().nth(1).and_then(|c| c.parse::<usize>().ok()) {
                    if columns > 0 {
                        return columns;
                    }
                }
            }
        }

        80
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_buffers_output() {
        let output = Output::buffered();
        output.write("a");
        output.writeln("b");
        output.new_line(2);
        assert_eq!(output.contents(), "ab\n\n\n");
        assert_eq!(output.messages(), vec!["a", "b", "\n\n"]);
        assert_eq!(output.fetch(), "ab\n\n\n");
        assert_eq!(output.fetch(), "");
    }

    #[test]
    fn it_respects_verbosity() {
        let output = Output::buffered().with_verbosity(Verbosity::Quiet);
        output.writeln("hidden");
        output.write_with("shown", true, Verbosity::Quiet);
        assert_eq!(output.fetch(), "shown\n");

        let output = Output::buffered().with_verbosity(Verbosity::Verbose);
        output.write_with("verbose", true, Verbosity::Verbose);
        output.write_with("debug", true, Verbosity::Debug);
        assert_eq!(output.fetch(), "verbose\n");
        assert!(output.is_verbose());
        assert!(!output.is_debug());
    }

    #[test]
    fn it_tracks_trailing_new_lines() {
        let output = Output::buffered();
        assert_eq!(output.new_lines_written(), 1);
        output.write("x");
        assert_eq!(output.new_lines_written(), 0);
        output.writeln("x\n");
        assert_eq!(output.new_lines_written(), 2);
        output.new_line(1);
        assert_eq!(output.new_lines_written(), 3);
    }

    #[test]
    fn it_answers_from_fed_lines() {
        let output = Output::buffered().with_input(["Taylor", "", "yes"]);
        let question = Question::new("What is your name?");
        assert_eq!(output.ask_text(&question, PromptStyle::Symfony).as_deref(), Some("Taylor"));

        let question = Question::new("Framework?").default(Some("Laravel".into()));
        assert_eq!(output.ask_text(&question, PromptStyle::Symfony).as_deref(), Some("Laravel"));

        assert!(output.ask_confirmation("Continue?", false, PromptStyle::Symfony));

        // Input is exhausted, so defaults are used from now on...
        assert!(!output.ask_confirmation("Again?", false, PromptStyle::Symfony));
        assert!(!output.is_interactive());

        let text = output.contents();
        assert!(text.contains(" What is your name?:\n > \n"));
        assert!(text.contains(" Framework? [Laravel]:"));
        assert!(text.contains(" Continue? (yes/no) [no]:"));
    }

    #[test]
    fn non_interactive_outputs_use_defaults() {
        let output = Output::buffered();
        let question = Question::new("Name?").default(Some("Abigail".into()));
        assert_eq!(output.ask_text(&question, PromptStyle::Symfony).as_deref(), Some("Abigail"));
        assert!(output.ask_confirmation("Sure?", true, PromptStyle::Symfony));
        assert_eq!(output.contents(), "");
    }

    #[test]
    fn it_validates_choices() {
        let choices = vec![
            ("0".to_string(), "PHP".to_string()),
            ("1".to_string(), "Ruby".to_string()),
        ];
        assert_eq!(validate_choice("PHP", &choices, false, false), Ok(vec!["PHP".to_string()]));
        assert_eq!(validate_choice("1", &choices, false, false), Ok(vec!["Ruby".to_string()]));
        assert_eq!(validate_choice("1", &choices, false, true), Ok(vec!["1".to_string()]));
        assert_eq!(
            validate_choice("0, Ruby", &choices, true, false),
            Ok(vec!["PHP".to_string(), "Ruby".to_string()])
        );
        assert_eq!(validate_choice("Go", &choices, false, false), Err("Value \"Go\" is invalid".to_string()));
    }

    #[test]
    fn it_reasks_invalid_choices() {
        let output = Output::buffered().with_input(["Go", "Ruby"]);
        let question = Question::new("Language?").kind(QuestionKind::Choice {
            choices: vec![("0".into(), "PHP".into()), ("1".into(), "Ruby".into())],
            multiple: false,
        });
        assert_eq!(output.ask_choice(&question, false, PromptStyle::Symfony), vec!["Ruby"]);
        let text = output.contents();
        assert!(text.contains("  [0] PHP\n  [1] Ruby\n"));
        assert!(text.contains("[ERROR] Value \"Go\" is invalid"));
    }

    #[test]
    fn verbosity_parses_laravel_names() {
        assert_eq!("v".parse::<Verbosity>(), Ok(Verbosity::Verbose));
        assert_eq!("vvv".parse::<Verbosity>(), Ok(Verbosity::Debug));
        assert_eq!("quiet".parse::<Verbosity>(), Ok(Verbosity::Quiet));
        assert!(Verbosity::Quiet < Verbosity::Normal);
    }
}
