//! The result of a finished process.

use crate::exceptions::ProcessFailedException;

/// The result of a process: its exit code and everything it wrote.
///
/// ```
/// use illuminate_process::Process;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let result = Process::run("echo Laravel").await?;
///
/// assert!(result.successful());
/// assert!(!result.failed());
/// assert_eq!(result.exit_code(), Some(0));
/// assert_eq!(result.output(), "Laravel\n");
/// assert_eq!(result.error_output(), "");
/// assert!(result.see_in_output("Laravel"));
/// assert_eq!(result.command(), "echo Laravel");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessResult {
    command: String,
    exit_code: Option<i32>,
    output: String,
    error_output: String,
}

impl ProcessResult {
    /// Create a process result.
    ///
    /// Most of the time you'll get results from running processes; in tests,
    /// [`Process::result`](crate::Process::result) builds fake ones.
    pub fn new(
        command: impl Into<String>,
        exit_code: Option<i32>,
        output: impl Into<String>,
        error_output: impl Into<String>,
    ) -> Self {
        Self {
            command: command.into(),
            exit_code,
            output: output.into(),
            error_output: error_output.into(),
        }
    }

    /// Create a copy of the result with the given command.
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = command.into();
        self
    }

    /// Get the original command executed by the process.
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Determine if the process was successful.
    pub fn successful(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Determine if the process failed.
    pub fn failed(&self) -> bool {
        !self.successful()
    }

    /// Get the exit code of the process (`None` while it is still running).
    ///
    /// A process killed by a signal reports `128 + signal`, like the shell.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Get the standard output of the process.
    pub fn output(&self) -> &str {
        &self.output
    }

    /// Get the error output of the process.
    pub fn error_output(&self) -> &str {
        &self.error_output
    }

    /// Determine if the output contains the given string.
    pub fn see_in_output(&self, output: &str) -> bool {
        self.output.contains(output)
    }

    /// Determine if the error output contains the given string.
    pub fn see_in_error_output(&self, output: &str) -> bool {
        self.error_output.contains(output)
    }

    /// Alias of [`see_in_output`](Self::see_in_output).
    pub fn seen_in_output(&self, output: &str) -> bool {
        self.see_in_output(output)
    }

    /// Alias of [`see_in_error_output`](Self::see_in_error_output).
    pub fn seen_in_error_output(&self, output: &str) -> bool {
        self.see_in_error_output(output)
    }

    /// Return an error if the process failed, or the result if it didn't.
    ///
    /// ```
    /// use illuminate_process::{Process, ProcessFailedException};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let output = Process::run("echo ok").await?.throw()?.output().to_string();
    /// assert_eq!(output, "ok\n");
    ///
    /// let error = Process::run("exit 3").await?.throw().unwrap_err();
    /// assert_eq!(error.code(), 3);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn throw(self) -> Result<Self, ProcessFailedException> {
        self.throw_with(|_, _| {})
    }

    /// Return an error if the process failed, calling the callback with the
    /// result and the exception first.
    pub fn throw_with(
        self,
        callback: impl FnOnce(&ProcessResult, &ProcessFailedException),
    ) -> Result<Self, ProcessFailedException> {
        if self.successful() {
            return Ok(self);
        }
        let exception = ProcessFailedException::new(self.clone());
        callback(&self, &exception);
        Err(exception)
    }

    /// Return an error if the process failed and the given condition is true.
    pub fn throw_if(self, condition: bool) -> Result<Self, ProcessFailedException> {
        if condition { self.throw() } else { Ok(self) }
    }

    /// Return an error if the process failed, unless the given condition is true.
    pub fn throw_unless(self, condition: bool) -> Result<Self, ProcessFailedException> {
        self.throw_if(!condition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_results() {
        let result = ProcessResult::new("ls", Some(0), "ProcessTest.php\n", "");
        assert!(result.successful());
        assert!(!result.failed());
        assert!(result.see_in_output("ProcessTest"));
        assert!(result.seen_in_output("ProcessTest"));
        assert!(!result.see_in_error_output("ProcessTest"));
        assert!(!result.seen_in_error_output("ProcessTest"));
        assert_eq!(result.clone().throw().unwrap(), result);
        assert_eq!(result.clone().throw_if(true).unwrap(), result);
    }

    #[test]
    fn failed_results_throw() {
        let result = ProcessResult::new("exit 1", Some(1), "", "oops\n");
        assert!(result.failed());
        assert!(result.see_in_error_output("oops"));

        let exception = result.clone().throw().unwrap_err();
        assert_eq!(exception.result, result);
        assert!(result.clone().throw_if(true).is_err());
        assert!(result.clone().throw_if(false).is_ok());
        assert!(result.clone().throw_unless(true).is_ok());
        assert!(result.clone().throw_unless(false).is_err());
    }

    #[test]
    fn throw_callbacks_receive_the_result_and_exception() {
        let mut seen = None;
        let result = ProcessResult::new("exit 4", Some(4), "", "");
        let error = result
            .throw_with(|result, exception| seen = Some((result.exit_code(), exception.code())))
            .unwrap_err();
        assert_eq!(seen, Some((Some(4), 4)));
        assert_eq!(error.code(), 4);

        let mut called = false;
        let ok = ProcessResult::new("true", Some(0), "", "").throw_with(|_, _| called = true);
        assert!(ok.is_ok());
        assert!(!called);
    }

    #[test]
    fn running_processes_have_no_exit_code() {
        let result = ProcessResult::new("sleep 10", None, "", "");
        assert!(result.failed());
        assert_eq!(result.exit_code(), None);
    }
}
