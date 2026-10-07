//! The `Process` facade.

use std::sync::Arc;

use illuminate_container::{Container, try_app};
use illuminate_support::Result;

use crate::command::{Command, IntoTimeout};
use crate::factory::Factory;
use crate::fake::{
    FakeOutput, FakeProcess, FakeProcessDescription, FakeProcessSequence, fake_result,
};
use crate::invoked::InvokedProcess;
use crate::output::OutputType;
use crate::pending::{EnvValue, PendingProcess};
use crate::pipe::Pipe;
use crate::pool::{Pool, ProcessPoolResults};
use crate::result::ProcessResult;

/// The `Process` facade: invoke external processes, and fake them in tests.
///
/// ```
/// use illuminate_process::Process;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let result = Process::run("ls -la").await?;
///
/// if result.successful() {
///     println!("{}", result.output());
/// }
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
///
/// The facade resolves the [`Factory`] from the current container, so fakes
/// installed by one test never leak into another:
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_process::Process;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// # let _guard = Container::set_local_instance(Arc::new(Container::new()));
/// Process::fake();
///
/// Process::run("bash import.sh").await?;
///
/// Process::assert_ran("bash import.sh");
/// Process::assert_ran_with(|process, _| process.timeout == Some(std::time::Duration::from_secs(60)));
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
pub struct Process;

impl Process {
    /// Get the process factory from the container, registering one if the
    /// application hasn't yet.
    pub fn factory() -> Factory {
        if let Some(factory) = try_app::<Factory>() {
            return (*factory).clone();
        }
        let container = Container::get_instance();
        container.singleton_if::<Factory>(|_| Arc::new(Factory::new()));
        (*container.make::<Factory>()).clone()
    }

    // ------------------------------------------------------------------
    // Invoking processes
    // ------------------------------------------------------------------

    /// Run a process and wait for it to finish.
    pub async fn run(command: impl Into<Command>) -> Result<ProcessResult> {
        Self::factory().run(command).await
    }

    /// Run a process, handing each line of output to the callback as it
    /// arrives, along with its type (`out` or `err`).
    ///
    /// ```
    /// use illuminate_process::Process;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// Process::run_with_output("echo Hello; echo World", |kind, output| {
    ///     print!("[{kind}] {output}");
    /// })
    /// .await?;
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn run_with_output<F>(command: impl Into<Command>, output: F) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str),
    {
        Self::factory().run_with_output(command, output).await
    }

    /// Start a process in the background.
    pub fn start(command: impl Into<Command>) -> Result<InvokedProcess> {
        Self::factory().start(command)
    }

    /// Start a process in the background with an output callback.
    pub fn start_with_output<F>(command: impl Into<Command>, output: F) -> Result<InvokedProcess>
    where
        F: FnMut(OutputType, &str) + Send + 'static,
    {
        Self::factory().start_with_output(command, output)
    }

    /// Begin a process that will run the given command.
    pub fn command(command: impl Into<Command>) -> PendingProcess {
        Self::factory().command(command)
    }

    /// Begin a process that runs in the given working directory.
    pub fn path(path: impl Into<std::path::PathBuf>) -> PendingProcess {
        Self::factory().path(path)
    }

    /// Begin a process with the given timeout (seconds, a `Duration`, or a
    /// `CarbonInterval`). Processes time out after 60 seconds by default.
    pub fn timeout(timeout: impl IntoTimeout) -> PendingProcess {
        Self::factory().timeout(timeout)
    }

    /// Begin a process that may go no longer than the given time without
    /// writing output.
    pub fn idle_timeout(timeout: impl IntoTimeout) -> PendingProcess {
        Self::factory().idle_timeout(timeout)
    }

    /// Begin a process that may run forever.
    pub fn forever() -> PendingProcess {
        Self::factory().forever()
    }

    /// Begin a process with the given environment variables.
    pub fn env<K, V>(environment: impl IntoIterator<Item = (K, V)>) -> PendingProcess
    where
        K: Into<String>,
        V: Into<EnvValue>,
    {
        Self::factory().env(environment)
    }

    /// Begin a process with the given standard input.
    ///
    /// ```
    /// use illuminate_process::Process;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let result = Process::input("Hello World").run("cat").await?;
    ///
    /// assert_eq!(result.output(), "Hello World");
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn input(input: impl Into<Vec<u8>>) -> PendingProcess {
        Self::factory().input(input)
    }

    /// Begin a process whose output is discarded.
    pub fn quietly() -> PendingProcess {
        Self::factory().quietly()
    }

    /// Begin a process in TTY mode.
    pub fn tty() -> PendingProcess {
        Self::factory().tty()
    }

    /// Define a pool of processes to run concurrently.
    pub fn pool(callback: impl FnOnce(&mut Pool)) -> Pool {
        Self::factory().pool(callback)
    }

    /// Run a pool of processes and wait for them all to finish.
    ///
    /// ```
    /// use illuminate_process::Process;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let results = Process::concurrently(|pool| {
    ///     pool.as_("first").path("/").command("pwd");
    ///     pool.as_("second").command("echo two");
    /// })
    /// .await?;
    ///
    /// assert!(results.successful());
    /// assert_eq!(results["first"].output(), "/\n");
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn concurrently(callback: impl FnOnce(&mut Pool)) -> Result<ProcessPoolResults> {
        Self::factory().concurrently(callback).await
    }

    /// Run a pool of processes with an output callback (which also receives
    /// each process' key), and wait for them all to finish.
    pub async fn concurrently_with_output<F>(
        callback: impl FnOnce(&mut Pool),
        output: F,
    ) -> Result<ProcessPoolResults>
    where
        F: Fn(OutputType, &str, &str) + Send + Sync + 'static,
    {
        Self::factory()
            .concurrently_with_output(callback, output)
            .await
    }

    /// Define and run a series of processes, piping the output of each one
    /// into the next.
    pub async fn pipe(callback: impl FnOnce(&mut Pipe)) -> Result<ProcessResult> {
        Self::factory().pipe(callback).await
    }

    /// Define and run a pipe with an output callback (which also receives
    /// each process' key).
    ///
    /// ```
    /// use illuminate_process::Process;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let mut seen = Vec::new();
    ///
    /// Process::pipe_with_output(
    ///     |pipe| {
    ///         pipe.as_("first").command("echo laravel");
    ///         pipe.as_("second").command("tr a-z A-Z");
    ///     },
    ///     |_, output, key| seen.push(format!("{key}: {output}")),
    /// )
    /// .await?;
    ///
    /// assert_eq!(seen, ["first: laravel\n", "second: LARAVEL\n"]);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn pipe_with_output<F>(
        callback: impl FnOnce(&mut Pipe),
        output: F,
    ) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str, &str),
    {
        Self::factory().pipe_with_output(callback, output).await
    }

    /// Pipe the given commands into each other.
    ///
    /// ```
    /// use illuminate_process::Process;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let result = Process::pipe_commands(["echo 'Hello, Laravel'", "grep -o Laravel"]).await?;
    ///
    /// assert_eq!(result.output(), "Laravel\n");
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn pipe_commands<C>(commands: impl IntoIterator<Item = C>) -> Result<ProcessResult>
    where
        C: Into<Command>,
    {
        Self::factory().pipe_commands(commands).await
    }

    // ------------------------------------------------------------------
    // Testing
    // ------------------------------------------------------------------

    /// Create a fake process result: its output (a string or list of
    /// lines), error output, and exit code.
    ///
    /// ```
    /// use illuminate_process::Process;
    ///
    /// let result = Process::result(["line 1", "line 2"], "", 1);
    ///
    /// assert_eq!(result.output(), "line 1\nline 2\n");
    /// assert!(result.failed());
    /// ```
    pub fn result(
        output: impl Into<FakeOutput>,
        error_output: impl Into<FakeOutput>,
        exit_code: i32,
    ) -> ProcessResult {
        fake_result(output, error_output, exit_code)
    }

    /// Begin describing the lifecycle of a fake process started with `start`.
    pub fn describe() -> FakeProcessDescription {
        FakeProcessDescription::new()
    }

    /// Begin describing a sequence of fake results for repeated invocations.
    pub fn sequence() -> FakeProcessSequence {
        FakeProcessSequence::new()
    }

    /// Fake every process: each one succeeds without any output.
    pub fn fake() -> Factory {
        let factory = Self::factory();
        factory.fake();
        factory
    }

    /// Fake every process with the given callback.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_container::Container;
    /// use illuminate_process::{FakeProcess, Process};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// # let _guard = Container::set_local_instance(Arc::new(Container::new()));
    /// Process::fake_using(|process| -> FakeProcess {
    ///     if process.path.is_some() { "in a directory".into() } else { 1.into() }
    /// });
    ///
    /// assert_eq!(Process::path("/tmp").run("ls").await?.output(), "in a directory\n");
    /// assert_eq!(Process::run("ls").await?.exit_code(), Some(1));
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn fake_using<F, R>(callback: F) -> Factory
    where
        F: Fn(&PendingProcess) -> R + Send + Sync + 'static,
        R: Into<FakeProcess>,
    {
        let factory = Self::factory();
        factory.fake_using(callback);
        factory
    }

    /// Fake the commands matching the given patterns; `*` is a wildcard.
    /// Any command that isn't faked will really run.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_container::Container;
    /// use illuminate_process::{FakeProcess, Process};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// # let _guard = Container::set_local_instance(Arc::new(Container::new()));
    /// Process::fake_commands([
    ///     ("cat *", FakeProcess::from(Process::result("Test \"cat\" output", "", 0))),
    ///     ("ls *", Process::sequence().push("First invocation").push("Second invocation").into()),
    ///     ("git *", "Already up to date.".into()),
    /// ]);
    ///
    /// assert_eq!(Process::run("cat composer.json").await?.output(), "Test \"cat\" output\n");
    /// assert_eq!(Process::run("ls -la").await?.output(), "First invocation\n");
    /// assert_eq!(Process::run("ls -la").await?.output(), "Second invocation\n");
    /// assert_eq!(Process::run("git pull").await?.output(), "Already up to date.\n");
    ///
    /// // Commands without a fake really run...
    /// assert_eq!(Process::run("echo real").await?.output(), "real\n");
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn fake_commands<K, V>(fakes: impl IntoIterator<Item = (K, V)>) -> Factory
    where
        K: Into<String>,
        V: Into<FakeProcess>,
    {
        let factory = Self::factory();
        factory.fake_commands(fakes);
        factory
    }

    /// Fake the commands matching the given pattern with a callback.
    pub fn fake_command_using<F, R>(pattern: impl Into<String>, callback: F) -> Factory
    where
        F: Fn(&PendingProcess) -> R + Send + Sync + 'static,
        R: Into<FakeProcess>,
    {
        let factory = Self::factory();
        factory.fake_command_using(pattern, callback);
        factory
    }

    /// Throw an exception for any process that isn't faked.
    pub fn prevent_stray_processes() -> Factory {
        let factory = Self::factory();
        factory.prevent_stray_processes();
        factory
    }

    /// Allow processes that aren't faked to run again.
    pub fn allow_stray_processes() -> Factory {
        let factory = Self::factory();
        factory.allow_stray_processes();
        factory
    }

    /// Determine if stray processes are being prevented.
    pub fn preventing_stray_processes() -> bool {
        Self::factory().preventing_stray_processes()
    }

    /// Determine if processes are being faked and recorded.
    pub fn is_recording() -> bool {
        Self::factory().is_recording()
    }

    /// Every recorded process, with its result.
    pub fn recorded() -> Vec<(PendingProcess, ProcessResult)> {
        Self::factory().recorded()
    }

    /// Assert that a process running the given command was invoked.
    #[track_caller]
    pub fn assert_ran(command: impl Into<Command>) {
        Self::factory().assert_ran(command);
    }

    /// Assert that a process passing the given truth test was invoked.
    #[track_caller]
    pub fn assert_ran_with(callback: impl Fn(&PendingProcess, &ProcessResult) -> bool) {
        Self::factory().assert_ran_with(callback);
    }

    /// Assert that a process running the given command was invoked a given
    /// number of times.
    #[track_caller]
    pub fn assert_ran_times(command: impl Into<Command>, times: usize) {
        Self::factory().assert_ran_times(command, times);
    }

    /// Assert that a process passing the given truth test was invoked a
    /// given number of times.
    #[track_caller]
    pub fn assert_ran_times_with(
        callback: impl Fn(&PendingProcess, &ProcessResult) -> bool,
        times: usize,
    ) {
        Self::factory().assert_ran_times_with(callback, times);
    }

    /// Assert that the given commands ran, in order.
    #[track_caller]
    pub fn assert_ran_in_order<C: Into<Command>>(commands: impl IntoIterator<Item = C>) {
        Self::factory().assert_ran_in_order(commands);
    }

    /// Assert that no process running the given command was invoked.
    #[track_caller]
    pub fn assert_not_ran(command: impl Into<Command>) {
        Self::factory().assert_not_ran(command);
    }

    /// Assert that no process passing the given truth test was invoked.
    #[track_caller]
    pub fn assert_not_ran_with(callback: impl Fn(&PendingProcess, &ProcessResult) -> bool) {
        Self::factory().assert_not_ran_with(callback);
    }

    /// Assert that no process running the given command was invoked.
    #[track_caller]
    pub fn assert_didnt_run(command: impl Into<Command>) {
        Self::factory().assert_didnt_run(command);
    }

    /// Alias of [`assert_didnt_run`](Self::assert_didnt_run).
    #[track_caller]
    pub fn assert_did_not_run(command: impl Into<Command>) {
        Self::factory().assert_did_not_run(command);
    }

    /// Assert that no processes were invoked.
    #[track_caller]
    pub fn assert_nothing_ran() {
        Self::factory().assert_nothing_ran();
    }
}
