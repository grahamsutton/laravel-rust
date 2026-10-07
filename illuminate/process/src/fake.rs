//! Faking processes in tests: fake results, process descriptions, sequences
//! and the fake invoked process that plays a description back.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use illuminate_support::Error;

use crate::exceptions::OutOfBoundsException;
use crate::output::OutputType;
use crate::result::ProcessResult;

/// The output of a fake process: a string, or a list of lines.
///
/// Fake output is normalized the way Laravel normalizes it: every line ends
/// with a newline, and empty output stays empty.
///
/// ```
/// use illuminate_process::FakeOutput;
///
/// assert_eq!(FakeOutput::from("Test output").to_string(), "Test output\n");
/// assert_eq!(FakeOutput::from(vec!["line 1", "", "line 2"]).to_string(), "line 1\n\nline 2\n");
/// assert_eq!(FakeOutput::from("").to_string(), "");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakeOutput {
    lines: Vec<String>,
}

impl FakeOutput {
    /// The individual lines that were given.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The normalized output.
    pub fn normalized(&self) -> String {
        let joined: String = self
            .lines
            .iter()
            .map(|line| format!("{}\n", line.trim_end_matches('\n')))
            .collect();
        normalize(&joined)
    }
}

/// Trim trailing newlines and end the output with exactly one.
fn normalize(output: &str) -> String {
    let trimmed = output.trim_end_matches('\n');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

impl fmt::Display for FakeOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized())
    }
}

impl From<&str> for FakeOutput {
    fn from(output: &str) -> Self {
        Self {
            lines: vec![output.to_string()],
        }
    }
}

impl From<String> for FakeOutput {
    fn from(output: String) -> Self {
        Self {
            lines: vec![output],
        }
    }
}

impl From<&String> for FakeOutput {
    fn from(output: &String) -> Self {
        Self::from(output.clone())
    }
}

impl From<Vec<&str>> for FakeOutput {
    fn from(lines: Vec<&str>) -> Self {
        Self {
            lines: lines.into_iter().map(String::from).collect(),
        }
    }
}

impl From<Vec<String>> for FakeOutput {
    fn from(lines: Vec<String>) -> Self {
        Self { lines }
    }
}

impl From<&[&str]> for FakeOutput {
    fn from(lines: &[&str]) -> Self {
        Self::from(lines.to_vec())
    }
}

impl<const N: usize> From<[&str; N]> for FakeOutput {
    fn from(lines: [&str; N]) -> Self {
        Self::from(lines.to_vec())
    }
}

/// Build a fake process result (see [`Process::result`](crate::Process::result)).
pub(crate) fn fake_result(
    output: impl Into<FakeOutput>,
    error_output: impl Into<FakeOutput>,
    exit_code: i32,
) -> ProcessResult {
    ProcessResult::new(
        "",
        Some(exit_code),
        output.into().normalized(),
        error_output.into().normalized(),
    )
}

/// Describes the lifecycle of a fake process started with `start`: the
/// output it writes (in order), how many times it reports that it is still
/// running, and how it exits.
///
/// ```
/// use illuminate_process::Process;
///
/// let description = Process::describe()
///     .output("First line of standard output")
///     .error_output("First line of error output")
///     .output("Second line of standard output")
///     .exit_code(0)
///     .iterations(3);
///
/// let result = description.to_process_result("bash import.sh");
/// assert_eq!(result.output(), "First line of standard output\nSecond line of standard output\n");
/// assert_eq!(result.error_output(), "First line of error output\n");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeProcessDescription {
    /// The process' ID.
    pub process_id: u32,
    /// All of the process' output, in the order it was described.
    pub output: Vec<(OutputType, String)>,
    /// The process' exit code.
    pub exit_code: i32,
    /// The number of times the process should report that it is running.
    pub run_iterations: usize,
}

impl Default for FakeProcessDescription {
    fn default() -> Self {
        Self {
            process_id: 1000,
            output: Vec::new(),
            exit_code: 0,
            run_iterations: 0,
        }
    }
}

impl FakeProcessDescription {
    /// Begin describing a fake process.
    pub fn new() -> Self {
        Self::default()
    }

    /// Specify the process ID that should be assigned to the process.
    pub fn id(mut self, process_id: u32) -> Self {
        self.process_id = process_id;
        self
    }

    /// Describe a line (or lines) of standard output.
    pub fn output(self, output: impl Into<FakeOutput>) -> Self {
        self.push(OutputType::Out, output.into())
    }

    /// Describe a line (or lines) of error output.
    pub fn error_output(self, output: impl Into<FakeOutput>) -> Self {
        self.push(OutputType::Err, output.into())
    }

    fn push(mut self, kind: OutputType, output: FakeOutput) -> Self {
        for line in output.lines {
            self.output
                .push((kind, format!("{}\n", line.trim_end_matches('\n'))));
        }
        self
    }

    /// Replace the entire standard output with the given string.
    pub fn replace_output(self, output: &str) -> Self {
        self.replace(OutputType::Out, output)
    }

    /// Replace the entire error output with the given string.
    pub fn replace_error_output(self, output: &str) -> Self {
        self.replace(OutputType::Err, output)
    }

    fn replace(mut self, kind: OutputType, output: &str) -> Self {
        self.output.retain(|(existing, _)| *existing != kind);
        if !output.is_empty() {
            self.output
                .push((kind, format!("{}\n", output.trim_end_matches('\n'))));
        }
        self
    }

    /// Specify the process exit code.
    pub fn exit_code(mut self, exit_code: i32) -> Self {
        self.exit_code = exit_code;
        self
    }

    /// Specify how many times `running` should report `true`.
    pub fn iterations(self, iterations: usize) -> Self {
        self.runs_for(iterations)
    }

    /// Specify how many times `running` should report `true`.
    pub fn runs_for(mut self, iterations: usize) -> Self {
        self.run_iterations = iterations;
        self
    }

    /// Convert the description into the result the process will finish with.
    pub fn to_process_result(&self, command: &str) -> ProcessResult {
        ProcessResult::new(
            command,
            Some(self.exit_code),
            self.resolve(OutputType::Out),
            self.resolve(OutputType::Err),
        )
    }

    fn resolve(&self, kind: OutputType) -> String {
        let joined: String = self
            .output
            .iter()
            .filter(|(existing, _)| *existing == kind)
            .map(|(_, buffer)| buffer.as_str())
            .collect();
        if joined.is_empty() {
            joined
        } else {
            format!("{}\n", joined.trim_end_matches('\n'))
        }
    }
}

/// A fake for a process: what a faked command "does" when it runs.
///
/// You rarely build one by hand: anything that converts into a
/// `FakeProcess` may be handed to the fake methods: a [`ProcessResult`], a
/// [`FakeProcessDescription`], a [`FakeProcessSequence`], a string or list
/// of lines (the output), or an `i32` (the exit code).
///
/// ```
/// use illuminate_process::{FakeProcess, Process};
/// use illuminate_support::error::RuntimeException;
///
/// let fakes: Vec<FakeProcess> = vec![
///     Process::result("Test output", "", 0).into(),
///     "Test \"ls\" output".into(),
///     1.into(),
///     FakeProcess::throw(RuntimeException::new("Something went wrong.")),
/// ];
/// # assert_eq!(fakes.len(), 4);
/// ```
#[derive(Clone)]
pub enum FakeProcess {
    /// A finished process result.
    Result(ProcessResult),
    /// A described process lifecycle.
    Description(FakeProcessDescription),
    /// A sequence of results, handed out one invocation at a time.
    Sequence(FakeProcessSequence),
    /// Throw an error instead of running the process.
    Error(Arc<dyn Fn() -> Error + Send + Sync>),
}

impl FakeProcess {
    /// Make the faked process throw the given exception.
    pub fn throw<E>(exception: E) -> Self
    where
        E: std::error::Error + Clone + Send + Sync + 'static,
    {
        FakeProcess::Error(Arc::new(move || Error::new(exception.clone())))
    }
}

impl fmt::Debug for FakeProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FakeProcess::Result(result) => f.debug_tuple("Result").field(result).finish(),
            FakeProcess::Description(description) => {
                f.debug_tuple("Description").field(description).finish()
            }
            FakeProcess::Sequence(sequence) => f.debug_tuple("Sequence").field(sequence).finish(),
            FakeProcess::Error(error) => {
                f.debug_tuple("Error").field(&error().to_string()).finish()
            }
        }
    }
}

impl From<ProcessResult> for FakeProcess {
    fn from(result: ProcessResult) -> Self {
        FakeProcess::Result(result)
    }
}

impl From<FakeProcessDescription> for FakeProcess {
    fn from(description: FakeProcessDescription) -> Self {
        FakeProcess::Description(description)
    }
}

impl From<FakeProcessSequence> for FakeProcess {
    fn from(sequence: FakeProcessSequence) -> Self {
        FakeProcess::Sequence(sequence)
    }
}

impl From<i32> for FakeProcess {
    fn from(exit_code: i32) -> Self {
        FakeProcess::Result(fake_result("", "", exit_code))
    }
}

impl From<FakeOutput> for FakeProcess {
    fn from(output: FakeOutput) -> Self {
        FakeProcess::Result(fake_result(output, "", 0))
    }
}

macro_rules! output_fakes {
    ($($type:ty),*) => {
        $(
            impl From<$type> for FakeProcess {
                fn from(output: $type) -> Self {
                    FakeProcess::from(FakeOutput::from(output))
                }
            }
        )*
    };
}

output_fakes!(&str, String, &String, Vec<&str>, Vec<String>, &[&str]);

impl<const N: usize> From<[&str; N]> for FakeProcess {
    fn from(lines: [&str; N]) -> Self {
        FakeProcess::from(FakeOutput::from(lines))
    }
}

/// A sequence of fake results: each invocation of a faked command takes the
/// next one.
///
/// ```
/// use illuminate_process::Process;
///
/// let sequence = Process::sequence()
///     .push(Process::result("First invocation", "", 0))
///     .push("Second invocation");
///
/// assert!(!sequence.is_empty());
/// ```
#[derive(Clone)]
pub struct FakeProcessSequence {
    state: Arc<Mutex<SequenceState>>,
}

struct SequenceState {
    processes: VecDeque<FakeProcess>,
    fail_when_empty: bool,
    empty_process: Option<FakeProcess>,
}

impl Default for FakeProcessSequence {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(SequenceState {
                processes: VecDeque::new(),
                fail_when_empty: true,
                empty_process: None,
            })),
        }
    }
}

impl FakeProcessSequence {
    /// Create a new, empty sequence.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a result (or description) onto the end of the sequence.
    pub fn push(self, process: impl Into<FakeProcess>) -> Self {
        self.state
            .lock()
            .unwrap()
            .processes
            .push_back(process.into());
        self
    }

    /// Return the given result once the sequence is empty, instead of failing.
    pub fn when_empty(self, process: impl Into<FakeProcess>) -> Self {
        {
            let mut state = self.state.lock().unwrap();
            state.fail_when_empty = false;
            state.empty_process = Some(process.into());
        }
        self
    }

    /// Return an empty, successful result once the sequence is empty.
    pub fn dont_fail_when_empty(self) -> Self {
        self.when_empty(fake_result("", "", 0))
    }

    /// Determine if the sequence has handed out all of its results.
    pub fn is_empty(&self) -> bool {
        self.state.lock().unwrap().processes.is_empty()
    }

    /// The number of results left in the sequence.
    pub fn len(&self) -> usize {
        self.state.lock().unwrap().processes.len()
    }

    /// Take the next result from the sequence.
    pub(crate) fn next_process(&self) -> Result<FakeProcess, OutOfBoundsException> {
        let mut state = self.state.lock().unwrap();
        if let Some(process) = state.processes.pop_front() {
            return Ok(process);
        }
        if state.fail_when_empty {
            return Err(OutOfBoundsException::new(
                "A process was invoked, but the process result sequence is empty.",
            ));
        }
        Ok(state
            .empty_process
            .clone()
            .unwrap_or_else(|| FakeProcess::Result(fake_result("", "", 0))))
    }
}

impl fmt::Debug for FakeProcessSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeProcessSequence")
            .field("remaining", &self.len())
            .finish()
    }
}

/// The output handler of an invoked process.
pub(crate) type OutputHandler = Box<dyn FnMut(OutputType, &str) + Send>;

/// A borrowed output handler.
type LineHandler<'a> = &'a mut dyn FnMut(OutputType, &str);

/// A started process that plays back a [`FakeProcessDescription`].
pub(crate) struct FakeInvokedProcess {
    command: String,
    process: FakeProcessDescription,
    received_signals: Vec<i32>,
    remaining_run_iterations: Option<usize>,
    stopped: bool,
    output_handler: Option<OutputHandler>,
    next_output_index: usize,
    next_error_output_index: usize,
}

impl FakeInvokedProcess {
    pub(crate) fn new(command: impl Into<String>, process: FakeProcessDescription) -> Self {
        Self {
            command: command.into(),
            process,
            received_signals: Vec::new(),
            remaining_run_iterations: None,
            stopped: false,
            output_handler: None,
            next_output_index: 0,
            next_error_output_index: 0,
        }
    }

    pub(crate) fn with_output_handler(mut self, handler: Option<OutputHandler>) -> Self {
        self.output_handler = handler;
        self
    }

    pub(crate) fn id(&mut self) -> Option<u32> {
        self.invoke_stored_handler();
        Some(self.process.process_id)
    }

    pub(crate) fn command(&self) -> &str {
        &self.command
    }

    pub(crate) fn signal(&mut self, signal: i32) {
        self.invoke_stored_handler();
        self.received_signals.push(signal);
    }

    pub(crate) fn has_received_signal(&self, signal: i32) -> bool {
        self.received_signals.contains(&signal)
    }

    /// Determine if the process is still running, using the stored handler.
    pub(crate) fn running(&mut self) -> bool {
        let mut handler = self.output_handler.take();
        let running = match handler.as_mut() {
            Some(handler) => self.running_with(Some(&mut |kind, line| handler(kind, line))),
            None => self.running_with(None),
        };
        self.output_handler = handler;
        running
    }

    fn running_with(&mut self, mut handler: Option<LineHandler<'_>>) -> bool {
        if self.stopped {
            return false;
        }

        self.invoke_next_line(&mut handler);

        let remaining = *self
            .remaining_run_iterations
            .get_or_insert(self.process.run_iterations);

        if remaining == 0 {
            while !self.stopped && self.invoke_next_line(&mut handler) {}
            return false;
        }

        self.remaining_run_iterations = Some(remaining - 1);
        true
    }

    fn invoke_stored_handler(&mut self) -> bool {
        let mut handler = self.output_handler.take();
        let invoked = match handler.as_mut() {
            Some(handler) => {
                self.invoke_next_line(&mut Some(&mut |kind, line| handler(kind, line)))
            }
            None => false,
        };
        self.output_handler = handler;
        invoked
    }

    /// Hand the next line of output to the handler (if there is one).
    fn invoke_next_line(&mut self, handler: &mut Option<LineHandler<'_>>) -> bool {
        let Some(handler) = handler.as_mut() else {
            return false;
        };

        let start = self.next_output_index.min(self.next_error_output_index);

        for index in start..self.process.output.len() {
            let (kind, buffer) = &self.process.output[index];
            match kind {
                OutputType::Out if index >= self.next_output_index => {
                    self.next_output_index = index + 1;
                    handler(OutputType::Out, buffer);
                    return true;
                }
                OutputType::Err if index >= self.next_error_output_index => {
                    self.next_error_output_index = index + 1;
                    handler(OutputType::Err, buffer);
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    pub(crate) fn output(&mut self) -> String {
        self.latest_output();
        self.collect(OutputType::Out, self.next_output_index)
    }

    pub(crate) fn error_output(&mut self) -> String {
        self.latest_error_output();
        self.collect(OutputType::Err, self.next_error_output_index)
    }

    fn collect(&self, kind: OutputType, until: usize) -> String {
        let joined: String = self.process.output[..until]
            .iter()
            .filter(|(existing, _)| *existing == kind)
            .map(|(_, buffer)| buffer.as_str())
            .collect();
        normalize(&joined)
    }

    pub(crate) fn latest_output(&mut self) -> String {
        let (output, next) = self.latest(OutputType::Out, self.next_output_index);
        self.next_output_index = next;
        output
    }

    pub(crate) fn latest_error_output(&mut self) -> String {
        let (output, next) = self.latest(OutputType::Err, self.next_error_output_index);
        self.next_error_output_index = next;
        output
    }

    /// The next line of the given kind at or after `next`, and the index
    /// to continue from.
    fn latest(&self, kind: OutputType, next: usize) -> (String, usize) {
        let remaining = self.process.output.get(next..).unwrap_or_default();
        match remaining.iter().position(|(existing, _)| *existing == kind) {
            Some(offset) => (remaining[offset].1.clone(), next + offset + 1),
            None => (String::new(), next.max(self.process.output.len())),
        }
    }

    /// Wait for the process to finish, handing every remaining line of
    /// output to the given handler (or the stored one).
    pub(crate) fn wait(&mut self, handler: Option<LineHandler<'_>>) -> ProcessResult {
        let result = match handler {
            Some(handler) => {
                while self.invoke_next_line(&mut Some(&mut *handler)) {}
                self.process.to_process_result(&self.command)
            }
            None => {
                let mut stored = self.output_handler.take();
                let result = match stored.as_mut() {
                    Some(stored) => {
                        while self.invoke_next_line(&mut Some(&mut |kind, line| stored(kind, line)))
                        {
                        }
                        self.process.to_process_result(&self.command)
                    }
                    None => self.predict_process_result(),
                };
                self.output_handler = stored;
                result
            }
        };

        self.remaining_run_iterations = Some(0);
        result
    }

    /// Wait until the given callback returns `true` for a line of output.
    pub(crate) fn wait_until(
        &mut self,
        until: &mut dyn FnMut(OutputType, &str) -> bool,
    ) -> ProcessResult {
        let mut should_stop = false;

        loop {
            let running = self.running_with(Some(&mut |kind, line| {
                should_stop = until(kind, line);
            }));
            if !running || should_stop {
                break;
            }
        }

        self.remaining_run_iterations = Some(0);
        self.process.to_process_result(&self.command)
    }

    pub(crate) fn stop(&mut self) -> Option<i32> {
        self.stopped = true;
        self.remaining_run_iterations = Some(0);
        Some(self.process.exit_code)
    }

    pub(crate) fn predict_process_result(&self) -> ProcessResult {
        self.process.to_process_result(&self.command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn describe() -> FakeProcessDescription {
        FakeProcessDescription::new()
    }

    #[test]
    fn fake_output_is_normalized() {
        assert_eq!(
            FakeOutput::from("test output").normalized(),
            "test output\n"
        );
        assert_eq!(
            FakeOutput::from("test output\n\n").normalized(),
            "test output\n"
        );
        assert_eq!(
            FakeOutput::from(vec!["line 1", "line 2"]).normalized(),
            "line 1\nline 2\n"
        );
        assert_eq!(
            FakeOutput::from(vec!["line 1", "", "line 2"]).normalized(),
            "line 1\n\nline 2\n"
        );
        assert_eq!(FakeOutput::from(Vec::<String>::new()).normalized(), "");
        assert_eq!(FakeOutput::from("").normalized(), "");
    }

    #[test]
    fn fake_results_are_normalized() {
        let result = fake_result(["line 1", "line 2"], "error output", 3);
        assert_eq!(result.output(), "line 1\nline 2\n");
        assert_eq!(result.error_output(), "error output\n");
        assert_eq!(result.exit_code(), Some(3));
        assert_eq!(result.command(), "");
    }

    #[test]
    fn descriptions_resolve_into_results() {
        let description = describe()
            .output("line 1")
            .output("")
            .output("line 2")
            .error_output(vec!["err 1", "err 2"])
            .exit_code(2)
            .id(4321);

        let result = description.to_process_result("ls -la");
        assert_eq!(result.command(), "ls -la");
        assert_eq!(result.output(), "line 1\n\nline 2\n");
        assert_eq!(result.error_output(), "err 1\nerr 2\n");
        assert_eq!(result.exit_code(), Some(2));
        assert_eq!(description.process_id, 4321);
    }

    #[test]
    fn descriptions_can_replace_output() {
        let description = describe()
            .output("one")
            .error_output("err")
            .output("two")
            .replace_output("replaced\n")
            .replace_error_output("");

        assert_eq!(
            description.output,
            vec![(OutputType::Out, "replaced\n".to_string())]
        );
    }

    #[test]
    fn sequences_hand_out_results_in_order() {
        let sequence = FakeProcessSequence::new().push("first").push("second");
        assert_eq!(sequence.len(), 2);

        let clone = sequence.clone();
        assert!(
            matches!(clone.next_process(), Ok(FakeProcess::Result(r)) if r.output() == "first\n")
        );
        assert!(
            matches!(sequence.next_process(), Ok(FakeProcess::Result(r)) if r.output() == "second\n")
        );
        assert!(sequence.is_empty());

        let error = sequence.next_process().unwrap_err();
        assert_eq!(
            error.to_string(),
            "A process was invoked, but the process result sequence is empty."
        );
    }

    #[test]
    fn sequences_may_return_a_default_when_empty() {
        let sequence = FakeProcessSequence::new().dont_fail_when_empty();
        assert!(
            matches!(sequence.next_process(), Ok(FakeProcess::Result(r)) if r.output().is_empty())
        );

        let sequence = FakeProcessSequence::new().when_empty("fallback");
        assert!(
            matches!(sequence.next_process(), Ok(FakeProcess::Result(r)) if r.output() == "fallback\n")
        );
    }

    #[test]
    fn fake_processes_convert_from_common_values() {
        assert!(matches!(FakeProcess::from(1), FakeProcess::Result(r) if r.exit_code() == Some(1)));
        assert!(
            matches!(FakeProcess::from("out"), FakeProcess::Result(r) if r.output() == "out\n")
        );
        assert!(
            matches!(FakeProcess::from(vec!["a", "b"]), FakeProcess::Result(r) if r.output() == "a\nb\n")
        );
        assert!(matches!(
            FakeProcess::from(describe()),
            FakeProcess::Description(_)
        ));
        assert!(matches!(
            FakeProcess::from(FakeProcessSequence::new()),
            FakeProcess::Sequence(_)
        ));

        let error = FakeProcess::throw(illuminate_support::error::RuntimeException::new("boom"));
        assert_eq!(format!("{error:?}"), "Error(\"boom\")");
    }

    #[test]
    fn latest_output_walks_through_the_description() {
        let mut process = FakeInvokedProcess::new(
            "echo",
            describe()
                .output("ONE")
                .output("TWO")
                .output("THREE")
                .runs_for(3),
        );

        let mut latest = Vec::new();
        let mut output = Vec::new();
        while process.running() {
            latest.push(process.latest_output());
            output.push(process.output());
        }

        assert_eq!(latest, vec!["ONE\n", "THREE\n", ""]);
        assert_eq!(
            output,
            vec!["ONE\nTWO\n", "ONE\nTWO\nTHREE\n", "ONE\nTWO\nTHREE\n"]
        );
    }

    #[test]
    fn wait_until_stops_once_the_callback_matches() {
        let mut process = FakeInvokedProcess::new(
            "echo",
            describe()
                .output("FIRST")
                .output("SECOND")
                .output("THIRD")
                .output("FOURTH")
                .runs_for(4),
        );

        let mut first = Vec::new();
        let result = process.wait_until(&mut |_, line| {
            first.push(line.to_string());
            line.contains("SECOND")
        });
        assert!(result.successful());
        assert_eq!(first, vec!["FIRST\n", "SECOND\n"]);

        let mut second = Vec::new();
        process.wait_until(&mut |_, line| {
            second.push(line.to_string());
            line.contains("FOURTH")
        });
        assert_eq!(second, vec!["THIRD\n", "FOURTH\n"]);
    }

    #[test]
    fn wait_hands_every_remaining_line_to_the_handler() {
        let mut process = FakeInvokedProcess::new(
            "echo",
            describe()
                .output("FIRST")
                .output("SECOND")
                .output("THIRD")
                .runs_for(3),
        );

        let mut first = Vec::new();
        assert!(
            process
                .wait(Some(&mut |_, line: &str| first.push(line.to_string())))
                .successful()
        );
        assert_eq!(first.len(), 3);

        let mut second = Vec::new();
        process.wait(Some(&mut |_, line: &str| second.push(line.to_string())));
        assert!(second.is_empty());
    }

    #[test]
    fn stopped_processes_stop_running() {
        let mut process =
            FakeInvokedProcess::new("sleep 100", describe().exit_code(143).runs_for(10));
        assert!(process.running());
        assert_eq!(process.stop(), Some(143));
        assert!(!process.running());
    }

    #[test]
    fn stored_handlers_receive_a_line_per_interaction() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let captured = lines.clone();
        let mut process = FakeInvokedProcess::new(
            "sleep 100",
            describe()
                .output("FIRST")
                .output("SECOND")
                .output("THIRD")
                .runs_for(10),
        )
        .with_output_handler(Some(Box::new(move |_, line| {
            captured.lock().unwrap().push(line.to_string())
        })));

        while process.running() {
            process.stop();
        }

        assert_eq!(*lines.lock().unwrap(), vec!["FIRST\n"]);
        assert_eq!(process.id(), Some(1000));
        process.signal(12);
        assert!(process.has_received_signal(12));
        assert!(!process.has_received_signal(9));
        assert_eq!(process.command(), "sleep 100");
    }
}
