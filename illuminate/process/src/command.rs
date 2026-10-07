//! The command a process runs, and how long it may run for.

use std::fmt;
use std::time::Duration;

use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{CarbonInterval, Result};

/// The command a process runs.
///
/// A string is a shell command line: it runs through `sh -c`, so pipes,
/// redirects, `&&` and environment expansion all work just like they do in
/// your terminal. An array of arguments runs the program directly, with no
/// shell in between, so nothing needs escaping:
///
/// ```
/// use illuminate_process::Command;
///
/// let shell = Command::from("ls -la | grep laravel");
/// let argv = Command::from(["php", "artisan", "migrate"]);
///
/// assert_eq!(shell.to_string(), "ls -la | grep laravel");
/// assert_eq!(argv.to_string(), "php artisan migrate");
///
/// // Arguments are quoted in the command line only when they need it...
/// assert_eq!(Command::from(["echo", "Hello World"]).to_string(), "echo 'Hello World'");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// A shell command line, run with `sh -c`.
    Shell(String),
    /// A program and its arguments, run directly.
    Argv(Vec<String>),
}

impl Command {
    /// The command line, as it would be typed into a shell.
    ///
    /// This is the string fakes are matched against and the one reported by
    /// [`ProcessResult::command`](crate::ProcessResult::command).
    pub fn command_line(&self) -> String {
        match self {
            Command::Shell(line) => line.clone(),
            Command::Argv(args) => args
                .iter()
                .map(|arg| escape_argument(arg))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// Determine if the command runs through the shell.
    pub fn is_shell(&self) -> bool {
        matches!(self, Command::Shell(_))
    }

    /// Build the operating system command.
    pub(crate) fn to_os_command(&self) -> Result<tokio::process::Command> {
        match self {
            Command::Shell(line) => Ok(shell_command(line)),
            Command::Argv(args) => {
                let (program, arguments) = args.split_first().ok_or_else(|| {
                    InvalidArgumentException::new("The process command must not be empty.")
                })?;
                let mut command = tokio::process::Command::new(program);
                command.args(arguments);
                Ok(command)
            }
        }
    }
}

#[cfg(unix)]
fn shell_command(line: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("sh");
    command.arg("-c").arg(line);
    command
}

#[cfg(not(unix))]
fn shell_command(line: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("cmd");
    command.arg("/C").arg(line);
    command
}

/// Quote an argument for display in a command line, only when it needs it.
pub(crate) fn escape_argument(argument: &str) -> String {
    if argument.is_empty() {
        return "''".to_string();
    }

    let safe = argument
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"-_./=:,+@%^".contains(&byte));

    if safe {
        argument.to_string()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.command_line())
    }
}

impl From<&str> for Command {
    fn from(line: &str) -> Self {
        Command::Shell(line.to_string())
    }
}

impl From<String> for Command {
    fn from(line: String) -> Self {
        Command::Shell(line)
    }
}

impl From<&String> for Command {
    fn from(line: &String) -> Self {
        Command::Shell(line.clone())
    }
}

impl From<Vec<String>> for Command {
    fn from(args: Vec<String>) -> Self {
        Command::Argv(args)
    }
}

impl From<Vec<&str>> for Command {
    fn from(args: Vec<&str>) -> Self {
        Command::Argv(args.into_iter().map(String::from).collect())
    }
}

impl From<&[&str]> for Command {
    fn from(args: &[&str]) -> Self {
        Command::Argv(args.iter().map(|arg| arg.to_string()).collect())
    }
}

impl<const N: usize> From<[&str; N]> for Command {
    fn from(args: [&str; N]) -> Self {
        Command::Argv(args.iter().map(|arg| arg.to_string()).collect())
    }
}

impl<const N: usize> From<[String; N]> for Command {
    fn from(args: [String; N]) -> Self {
        Command::Argv(args.into())
    }
}

impl From<&Command> for Command {
    fn from(command: &Command) -> Self {
        command.clone()
    }
}

/// Anything that can describe a timeout: a number of seconds, a
/// [`Duration`], or a [`CarbonInterval`].
///
/// ```
/// use std::time::Duration;
/// use illuminate_process::IntoTimeout;
/// use illuminate_support::CarbonInterval;
///
/// assert_eq!(120.into_timeout(), Duration::from_secs(120));
/// assert_eq!(1.5.into_timeout(), Duration::from_millis(1500));
/// assert_eq!(CarbonInterval::minutes(2).into_timeout(), Duration::from_secs(120));
/// ```
pub trait IntoTimeout {
    /// Convert the value into a duration.
    fn into_timeout(self) -> Duration;
}

impl IntoTimeout for Duration {
    fn into_timeout(self) -> Duration {
        self
    }
}

impl IntoTimeout for CarbonInterval {
    fn into_timeout(self) -> Duration {
        let months =
            Duration::from_secs(u64::try_from(self.months.max(0)).unwrap_or(0) * 30 * 86_400);
        months + self.duration.to_std().unwrap_or_default()
    }
}

impl IntoTimeout for f64 {
    fn into_timeout(self) -> Duration {
        Duration::try_from_secs_f64(self.max(0.0)).unwrap_or(Duration::MAX)
    }
}

macro_rules! integer_timeouts {
    ($($type:ty),*) => {
        $(
            impl IntoTimeout for $type {
                fn into_timeout(self) -> Duration {
                    Duration::from_secs(u64::try_from(self).unwrap_or(0))
                }
            }
        )*
    };
}

integer_timeouts!(i32, i64, u32, u64, usize);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_shell_commands() {
        let command = Command::from("ls -la");
        assert!(command.is_shell());
        assert_eq!(command.command_line(), "ls -la");
        assert_eq!(
            Command::from(String::from("pwd")),
            Command::Shell("pwd".into())
        );
    }

    #[test]
    fn arrays_are_argument_lists() {
        let command = Command::from(["php", "artisan", "migrate"]);
        assert!(!command.is_shell());
        assert_eq!(command.command_line(), "php artisan migrate");
        assert_eq!(Command::from(vec!["ls"]), Command::Argv(vec!["ls".into()]));
        assert_eq!(
            Command::from(vec![String::from("a"), String::from("b")]).command_line(),
            "a b"
        );
    }

    #[test]
    fn arguments_are_quoted_only_when_needed() {
        assert_eq!(escape_argument("--force"), "--force");
        assert_eq!(escape_argument("path/to/file.txt"), "path/to/file.txt");
        assert_eq!(escape_argument("Hello World"), "'Hello World'");
        assert_eq!(escape_argument("it's"), "'it'\\''s'");
        assert_eq!(escape_argument(""), "''");
        assert_eq!(
            Command::from(["cat composer.json"]).command_line(),
            "'cat composer.json'"
        );
    }

    #[test]
    fn empty_argument_lists_cannot_be_run() {
        let error = Command::Argv(Vec::new()).to_os_command().unwrap_err();
        assert_eq!(error.to_string(), "The process command must not be empty.");
    }

    #[test]
    fn timeouts_accept_seconds_durations_and_intervals() {
        assert_eq!(60.into_timeout(), Duration::from_secs(60));
        assert_eq!(60u64.into_timeout(), Duration::from_secs(60));
        assert_eq!((-5).into_timeout(), Duration::ZERO);
        assert_eq!(0.25.into_timeout(), Duration::from_millis(250));
        assert_eq!(
            Duration::from_millis(10).into_timeout(),
            Duration::from_millis(10)
        );
        assert_eq!(
            CarbonInterval::milliseconds(1_000).into_timeout(),
            Duration::from_secs(1)
        );
    }
}
