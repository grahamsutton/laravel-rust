//! A process that has been configured but not yet run.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use illuminate_support::Result;
use illuminate_support::error::InvalidArgumentException;

use crate::command::{Command, IntoTimeout};
use crate::factory::Factory;
use crate::fake::{FakeInvokedProcess, FakeProcess, FakeProcessDescription, OutputHandler};
use crate::invoked::{InvokedProcess, RealProcess};
use crate::output::OutputType;
use crate::result::ProcessResult;

/// The default number of seconds a process may run.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// A process being configured: its working directory, environment,
/// input, timeouts, and output handling.
///
/// You'll usually start one from the [`Process`](crate::Process) facade,
/// and its fields are public so fake assertions can inspect them:
///
/// ```
/// use std::time::Duration;
/// use illuminate_process::Process;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let result = Process::path("/")
///     .timeout(120)
///     .env([("GREETING", "Hello")])
///     .run("echo $GREETING from $(pwd)")
///     .await?;
///
/// assert_eq!(result.output(), "Hello from /\n");
///
/// let mut pending = Process::timeout(30);
/// pending.idle_timeout(10).input("Laravel");
/// assert_eq!(pending.timeout, Some(Duration::from_secs(30)));
/// assert_eq!(pending.run("cat").await?.output(), "Laravel");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone, Debug)]
pub struct PendingProcess {
    /// The command to invoke the process.
    pub command: Option<Command>,
    /// The working directory of the process.
    pub path: Option<PathBuf>,
    /// The maximum time the process may run (`None` runs forever).
    pub timeout: Option<Duration>,
    /// The maximum time the process may go without writing any output.
    pub idle_timeout: Option<Duration>,
    /// The additional environment variables (`None` removes a variable).
    pub environment: BTreeMap<String, Option<String>>,
    /// The standard input that should be piped into the process.
    pub input: Option<Vec<u8>>,
    /// Indicates whether output should be discarded.
    pub quietly: bool,
    /// Indicates if TTY mode is enabled.
    pub tty: bool,
    factory: Option<Factory>,
}

impl Default for PendingProcess {
    fn default() -> Self {
        Self {
            command: None,
            path: None,
            timeout: Some(DEFAULT_TIMEOUT),
            idle_timeout: None,
            environment: BTreeMap::new(),
            input: None,
            quietly: false,
            tty: false,
            factory: None,
        }
    }
}

impl PendingProcess {
    /// Create a pending process that isn't attached to a factory (so it is
    /// never faked).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a pending process attached to the given factory.
    pub(crate) fn for_factory(factory: Factory) -> Self {
        Self {
            factory: Some(factory),
            ..Self::default()
        }
    }

    /// A copy of the process suitable for recording (without its factory).
    pub(crate) fn detached(&self) -> Self {
        Self {
            factory: None,
            ..self.clone()
        }
    }

    /// Specify the command that will invoke the process.
    pub fn command(&mut self, command: impl Into<Command>) -> &mut Self {
        self.command = Some(command.into());
        self
    }

    /// Specify the working directory of the process.
    pub fn path(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.path = Some(path.into());
        self
    }

    /// Specify the maximum time the process may run: seconds, a
    /// `Duration`, or a `CarbonInterval`.
    pub fn timeout(&mut self, timeout: impl IntoTimeout) -> &mut Self {
        self.timeout = Some(timeout.into_timeout());
        self
    }

    /// Specify the maximum time the process may go without writing output.
    pub fn idle_timeout(&mut self, timeout: impl IntoTimeout) -> &mut Self {
        self.idle_timeout = Some(timeout.into_timeout());
        self
    }

    /// Indicate that the process may run forever without timing out.
    pub fn forever(&mut self) -> &mut Self {
        self.timeout = None;
        self
    }

    /// Set the additional environment variables for the process.
    ///
    /// The process also inherits your application's environment. To remove
    /// an inherited variable, give it a value of `false`:
    ///
    /// ```
    /// use illuminate_process::{EnvValue, Process};
    ///
    /// let mut pending = Process::env([("IMPORT_PATH", "/tmp")]);
    /// pending.env([("IMPORT_PATH", EnvValue::from("/tmp")), ("LOAD_PATH", false.into())]);
    ///
    /// assert_eq!(pending.environment["IMPORT_PATH"].as_deref(), Some("/tmp"));
    /// assert_eq!(pending.environment["LOAD_PATH"], None);
    /// ```
    pub fn env<K, V>(&mut self, environment: impl IntoIterator<Item = (K, V)>) -> &mut Self
    where
        K: Into<String>,
        V: Into<EnvValue>,
    {
        self.environment = environment
            .into_iter()
            .map(|(key, value)| (key.into(), value.into().0))
            .collect();
        self
    }

    /// Set the standard input that should be provided to the process.
    pub fn input(&mut self, input: impl Into<Vec<u8>>) -> &mut Self {
        self.input = Some(input.into());
        self
    }

    /// Discard the process' output, conserving memory.
    pub fn quietly(&mut self) -> &mut Self {
        self.quietly = true;
        self
    }

    /// Enable TTY mode: the process uses your program's input and output,
    /// so it can open an editor like Vim or Nano.
    pub fn tty(&mut self) -> &mut Self {
        self.tty = true;
        self
    }

    /// Determine whether TTY mode is supported on this operating system.
    pub fn supports_tty() -> bool {
        cfg!(unix)
    }

    /// Run the process and wait for it to finish.
    pub async fn run(&self, command: impl Into<Command>) -> Result<ProcessResult> {
        self.run_with_output(command, |_, _| {}).await
    }

    /// Run the process, handing each line of output to the callback as it
    /// arrives.
    ///
    /// ```
    /// use illuminate_process::{OutputType, Process};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let mut errors = Vec::new();
    ///
    /// Process::run_with_output("echo ok; echo oops >&2", |kind, line| {
    ///     if kind == OutputType::Err {
    ///         errors.push(line.to_string());
    ///     }
    /// })
    /// .await?;
    ///
    /// assert_eq!(errors, ["oops\n"]);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn run_with_output<F>(
        &self,
        command: impl Into<Command>,
        mut output: F,
    ) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str),
    {
        let mut pending = self.clone();
        pending.command = Some(command.into());
        pending.execute(&mut output).await
    }

    /// Start the process in the background.
    pub fn start(&self, command: impl Into<Command>) -> Result<InvokedProcess> {
        let mut pending = self.clone();
        pending.command = Some(command.into());
        pending.launch(None)
    }

    /// Start the process in the background, handing each line of output to
    /// the callback whenever the process is polled or waited on.
    pub fn start_with_output<F>(
        &self,
        command: impl Into<Command>,
        output: F,
    ) -> Result<InvokedProcess>
    where
        F: FnMut(OutputType, &str) + Send + 'static,
    {
        let mut pending = self.clone();
        pending.command = Some(command.into());
        pending.launch(Some(Box::new(output)))
    }

    fn configured_command(&self) -> Result<Command> {
        self.command.clone().ok_or_else(|| {
            InvalidArgumentException::new("A command must be specified before a process can run.")
                .into()
        })
    }

    /// Run the configured command and wait for it to finish.
    pub(crate) async fn execute<F>(&self, output: &mut F) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str),
    {
        let command = self.configured_command()?;
        let command_line = command.command_line();

        if let Some(factory) = &self.factory {
            if let Some(fake) = factory.fake_for(&command_line) {
                let result = resolve_synchronous_fake(fake(self), &command_line)?;
                emit_lines(&result, output);
                factory.record_if_recording(self, &result);
                return Ok(result);
            }
            factory.ensure_stray_process_allowed(&command_line)?;
        }

        let mut process = RealProcess::spawn(self, &command, None)?;
        process
            .wait_for(
                |kind, line| {
                    output(kind, line);
                    false
                },
                false,
            )
            .await
    }

    /// Start the configured command in the background.
    pub(crate) fn launch(&self, handler: Option<OutputHandler>) -> Result<InvokedProcess> {
        let command = self.configured_command()?;
        let command_line = command.command_line();

        if let Some(factory) = &self.factory {
            if let Some(fake) = factory.fake_for(&command_line) {
                let description = resolve_asynchronous_fake(fake(self))?;
                let process =
                    FakeInvokedProcess::new(command_line, description).with_output_handler(handler);
                factory.record_if_recording(self, &process.predict_process_result());
                return Ok(InvokedProcess::fake(process));
            }
            factory.ensure_stray_process_allowed(&command_line)?;
        }

        Ok(InvokedProcess::real(RealProcess::spawn(
            self, &command, handler,
        )?))
    }
}

/// Turn a fake into the result of a process run with `run`.
fn resolve_synchronous_fake(fake: FakeProcess, command: &str) -> Result<ProcessResult> {
    match fake {
        FakeProcess::Result(result) if result.command().is_empty() => {
            Ok(result.with_command(command))
        }
        FakeProcess::Result(result) => Ok(result),
        FakeProcess::Description(description) => Ok(description.to_process_result(command)),
        FakeProcess::Sequence(sequence) => {
            resolve_synchronous_fake(sequence.next_process()?, command)
        }
        FakeProcess::Error(make) => Err(make()),
    }
}

/// Turn a fake into the description of a process started with `start`.
fn resolve_asynchronous_fake(fake: FakeProcess) -> Result<FakeProcessDescription> {
    match fake {
        FakeProcess::Result(result) => Ok(FakeProcessDescription::new()
            .replace_output(result.output())
            .replace_error_output(result.error_output())
            .runs_for(0)
            .exit_code(result.exit_code().unwrap_or(0))),
        FakeProcess::Description(description) => Ok(description),
        FakeProcess::Sequence(sequence) => resolve_asynchronous_fake(sequence.next_process()?),
        FakeProcess::Error(make) => Err(make()),
    }
}

/// Hand the output of a faked result to an output callback, line by line.
fn emit_lines<F: FnMut(OutputType, &str)>(result: &ProcessResult, output: &mut F) {
    for line in result.output().split_inclusive('\n') {
        output(OutputType::Out, line);
    }
    for line in result.error_output().split_inclusive('\n') {
        output(OutputType::Err, line);
    }
}

/// The value of an environment variable handed to [`PendingProcess::env`]:
/// a string, or `false` to remove an inherited variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvValue(pub Option<String>);

impl From<&str> for EnvValue {
    fn from(value: &str) -> Self {
        Self(Some(value.to_string()))
    }
}

impl From<String> for EnvValue {
    fn from(value: String) -> Self {
        Self(Some(value))
    }
}

impl From<&String> for EnvValue {
    fn from(value: &String) -> Self {
        Self(Some(value.clone()))
    }
}

impl From<&std::path::Path> for EnvValue {
    fn from(value: &std::path::Path) -> Self {
        Self(Some(value.to_string_lossy().into_owned()))
    }
}

impl From<PathBuf> for EnvValue {
    fn from(value: PathBuf) -> Self {
        Self::from(value.as_path())
    }
}

impl From<bool> for EnvValue {
    /// `false` removes the variable; `true` sets it to `"1"`, like PHP.
    fn from(value: bool) -> Self {
        Self(value.then(|| "1".to_string()))
    }
}

impl<T: Into<EnvValue>> From<Option<T>> for EnvValue {
    fn from(value: Option<T>) -> Self {
        value.map(Into::into).unwrap_or(Self(None))
    }
}

macro_rules! integer_env_values {
    ($($type:ty),*) => {
        $(
            impl From<$type> for EnvValue {
                fn from(value: $type) -> Self {
                    Self(Some(value.to_string()))
                }
            }
        )*
    };
}

integer_env_values!(i32, i64, u32, u64, usize);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_processes_have_laravel_defaults() {
        let pending = PendingProcess::new();
        assert_eq!(pending.timeout, Some(Duration::from_secs(60)));
        assert_eq!(pending.idle_timeout, None);
        assert!(pending.environment.is_empty());
        assert!(!pending.quietly);
        assert!(!pending.tty);
        assert!(pending.command.is_none());
        assert!(pending.path.is_none());
    }

    #[test]
    fn pending_processes_are_configured_fluently() {
        let mut pending = PendingProcess::new();
        pending
            .command("ls -la")
            .path("/tmp")
            .timeout(120)
            .idle_timeout(Duration::from_secs(30))
            .env([("A", "1")])
            .input("Hello")
            .quietly()
            .tty();

        assert_eq!(pending.command, Some(Command::from("ls -la")));
        assert_eq!(pending.path, Some(PathBuf::from("/tmp")));
        assert_eq!(pending.timeout, Some(Duration::from_secs(120)));
        assert_eq!(pending.idle_timeout, Some(Duration::from_secs(30)));
        assert_eq!(pending.environment["A"].as_deref(), Some("1"));
        assert_eq!(pending.input.as_deref(), Some(&b"Hello"[..]));
        assert!(pending.quietly);
        assert!(pending.tty);

        pending.forever();
        assert_eq!(pending.timeout, None);
    }

    #[test]
    fn environment_values_convert_like_php() {
        assert_eq!(EnvValue::from("x"), EnvValue(Some("x".into())));
        assert_eq!(EnvValue::from(false), EnvValue(None));
        assert_eq!(EnvValue::from(true), EnvValue(Some("1".into())));
        assert_eq!(EnvValue::from(42), EnvValue(Some("42".into())));
        assert_eq!(EnvValue::from(None::<&str>), EnvValue(None));
        assert_eq!(EnvValue::from(Some("y")), EnvValue(Some("y".into())));
        assert_eq!(
            EnvValue::from(PathBuf::from("/var/www")),
            EnvValue(Some("/var/www".into()))
        );
    }

    #[test]
    fn env_replaces_the_previous_environment() {
        let mut pending = PendingProcess::new();
        pending.env([("A", "1")]).env([("B", "2")]);
        assert!(!pending.environment.contains_key("A"));
        assert_eq!(pending.environment["B"].as_deref(), Some("2"));
    }

    #[tokio::test]
    async fn processes_need_a_command() {
        let pending = PendingProcess::new();
        let error = pending.execute(&mut |_, _| {}).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "A command must be specified before a process can run."
        );
        assert!(pending.launch(None).is_err());
    }

    #[test]
    fn tty_is_supported_on_unix() {
        assert_eq!(PendingProcess::supports_tty(), cfg!(unix));
    }
}
