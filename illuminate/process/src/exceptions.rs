//! The exceptions thrown by the process component.

use std::fmt;
use std::time::Duration;

use crate::ProcessResult;

/// Thrown by [`ProcessResult::throw`] when a process exits unsuccessfully.
///
/// The message mirrors Laravel's, including any output the process wrote:
///
/// ```
/// use illuminate_process::Process;
///
/// let result = Process::result("", "Permission denied", 1).with_command("bash import.sh");
/// let exception = result.throw().unwrap_err();
///
/// assert_eq!(exception.to_string(), "The command \"bash import.sh\" failed.\n\n\
///     Exit Code: 1\n\n\
///     Error Output:\n================\nPermission denied\n");
/// assert_eq!(exception.code(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessFailedException {
    /// The result of the failed process.
    pub result: ProcessResult,
}

impl ProcessFailedException {
    /// Create a new exception for the given result.
    pub fn new(result: ProcessResult) -> Self {
        Self { result }
    }

    /// The exception code: the process exit code (or `1` when unknown).
    pub fn code(&self) -> i32 {
        self.result.exit_code().unwrap_or(1)
    }
}

impl fmt::Display for ProcessFailedException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let exit_code = self
            .result
            .exit_code()
            .map(|code| code.to_string())
            .unwrap_or_default();

        write!(
            f,
            "The command \"{}\" failed.\n\nExit Code: {}",
            self.result.command(),
            exit_code
        )?;

        if !self.result.output().is_empty() {
            write!(f, "\n\nOutput:\n================\n{}", self.result.output())?;
        }

        if !self.result.error_output().is_empty() {
            write!(
                f,
                "\n\nError Output:\n================\n{}",
                self.result.error_output()
            )?;
        }

        Ok(())
    }
}

impl std::error::Error for ProcessFailedException {}

/// Thrown when a process runs longer than its timeout, or goes longer than
/// its idle timeout without writing any output.
///
/// Idle timeouts are reported by the same exception (Laravel's
/// `ProcessIdleTimedOutException` is a subclass of it), so catching this one
/// catches both; [`is_idle_timeout`](Self::is_idle_timeout) tells them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessTimedOutException {
    /// The result of the process at the moment it was stopped.
    pub result: ProcessResult,
    timeout: Duration,
    idle: bool,
}

impl ProcessTimedOutException {
    /// The process exceeded its overall timeout.
    pub fn new(result: ProcessResult, timeout: Duration) -> Self {
        Self {
            result,
            timeout,
            idle: false,
        }
    }

    /// The process went too long without writing any output.
    pub fn idle(result: ProcessResult, timeout: Duration) -> Self {
        Self {
            result,
            timeout,
            idle: true,
        }
    }

    /// How long the process was allowed to run (or idle) before timing out.
    pub fn exceeded_timeout(&self) -> Duration {
        self.timeout
    }

    /// Determine if the process hit its idle timeout.
    pub fn is_idle_timeout(&self) -> bool {
        self.idle
    }

    /// Determine if the process hit its overall timeout.
    pub fn is_general_timeout(&self) -> bool {
        !self.idle
    }
}

impl fmt::Display for ProcessTimedOutException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "The process \"{}\" exceeded the {}timeout of {} seconds.",
            self.result.command(),
            if self.idle { "idle " } else { "" },
            self.timeout.as_secs_f64()
        )
    }
}

impl std::error::Error for ProcessTimedOutException {}

/// Thrown when a fake process sequence runs out of results.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct OutOfBoundsException {
    /// The exception message.
    pub message: String,
}

impl OutOfBoundsException {
    /// Create a new exception with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(exit_code: i32, output: &str, error_output: &str) -> ProcessResult {
        ProcessResult::new("exit 1;", Some(exit_code), output, error_output)
    }

    #[test]
    fn failed_messages_without_output() {
        let exception = ProcessFailedException::new(result(1, "", ""));
        assert_eq!(
            exception.to_string(),
            "The command \"exit 1;\" failed.\n\nExit Code: 1"
        );
        assert_eq!(exception.code(), 1);
    }

    #[test]
    fn failed_messages_include_output_and_error_output() {
        let exception = ProcessFailedException::new(result(2, "out\n", "err\n"));
        assert_eq!(
            exception.to_string(),
            "The command \"exit 1;\" failed.\n\nExit Code: 2\n\nOutput:\n================\nout\n\n\nError Output:\n================\nerr\n"
        );
        assert_eq!(exception.code(), 2);
    }

    #[test]
    fn failed_messages_without_an_exit_code() {
        let exception = ProcessFailedException::new(ProcessResult::new("sleep 1", None, "", ""));
        assert_eq!(
            exception.to_string(),
            "The command \"sleep 1\" failed.\n\nExit Code: "
        );
        assert_eq!(exception.code(), 1);
    }

    #[test]
    fn timed_out_messages() {
        let general = ProcessTimedOutException::new(
            ProcessResult::new("sleep 2; exit 1;", None, "", ""),
            Duration::from_secs(1),
        );
        assert_eq!(
            general.to_string(),
            "The process \"sleep 2; exit 1;\" exceeded the timeout of 1 seconds."
        );
        assert!(general.is_general_timeout());
        assert!(!general.is_idle_timeout());

        let idle = ProcessTimedOutException::idle(
            ProcessResult::new("sleep 5;", None, "", ""),
            Duration::from_millis(1_500),
        );
        assert_eq!(
            idle.to_string(),
            "The process \"sleep 5;\" exceeded the idle timeout of 1.5 seconds."
        );
        assert!(idle.is_idle_timeout());
        assert_eq!(idle.exceeded_timeout(), Duration::from_millis(1_500));
    }
}
