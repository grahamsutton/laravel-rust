//! The process factory: creates pending processes, and records and fakes
//! them in tests.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use illuminate_support::error::RuntimeException;
use illuminate_support::{Result, Str};

use crate::command::{Command, IntoTimeout};
use crate::fake::{
    FakeOutput, FakeProcess, FakeProcessDescription, FakeProcessSequence, fake_result,
};
use crate::invoked::InvokedProcess;
use crate::output::OutputType;
use crate::pending::{EnvValue, PendingProcess};
use crate::pipe::Pipe;
use crate::pool::{Pool, ProcessPoolResults};
use crate::result::ProcessResult;

/// A fake handler: decides what a faked process "does".
pub(crate) type FakeHandler = Arc<dyn Fn(&PendingProcess) -> FakeProcess + Send + Sync>;

/// The process factory, which the [`Process`](crate::Process) facade
/// resolves from the container.
///
/// A `Factory` is a cheap handle: clones share the same fakes and recorded
/// processes.
///
/// ```
/// use illuminate_process::Factory;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let factory = Factory::new();
/// factory.fake_commands([("git *", "Already up to date.")]);
///
/// let result = factory.path("/").run("git pull").await?;
///
/// assert_eq!(result.output(), "Already up to date.\n");
/// factory.assert_ran("git pull");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone, Default)]
pub struct Factory {
    state: Arc<State>,
}

#[derive(Default)]
struct State {
    recording: AtomicBool,
    prevent_stray_processes: AtomicBool,
    fake_handlers: RwLock<Vec<(String, FakeHandler)>>,
    recorded: Mutex<Vec<(PendingProcess, ProcessResult)>>,
}

impl fmt::Debug for Factory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Factory")
            .field("recording", &self.is_recording())
            .field(
                "preventing_stray_processes",
                &self.preventing_stray_processes(),
            )
            .field("fakes", &self.state.fake_handlers.read().unwrap().len())
            .field("recorded", &self.state.recorded.lock().unwrap().len())
            .finish()
    }
}

impl Factory {
    /// Create a new process factory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new pending process associated with this factory.
    pub fn new_pending_process(&self) -> PendingProcess {
        PendingProcess::for_factory(self.clone())
    }

    // ------------------------------------------------------------------
    // Starting processes
    // ------------------------------------------------------------------

    /// Begin a process that will run the given command.
    pub fn command(&self, command: impl Into<Command>) -> PendingProcess {
        let mut pending = self.new_pending_process();
        pending.command(command);
        pending
    }

    /// Begin a process that runs in the given working directory.
    pub fn path(&self, path: impl Into<std::path::PathBuf>) -> PendingProcess {
        let mut pending = self.new_pending_process();
        pending.path(path);
        pending
    }

    /// Begin a process with the given timeout.
    pub fn timeout(&self, timeout: impl IntoTimeout) -> PendingProcess {
        let mut pending = self.new_pending_process();
        pending.timeout(timeout);
        pending
    }

    /// Begin a process with the given idle timeout.
    pub fn idle_timeout(&self, timeout: impl IntoTimeout) -> PendingProcess {
        let mut pending = self.new_pending_process();
        pending.idle_timeout(timeout);
        pending
    }

    /// Begin a process that may run forever.
    pub fn forever(&self) -> PendingProcess {
        let mut pending = self.new_pending_process();
        pending.forever();
        pending
    }

    /// Begin a process with the given environment variables.
    pub fn env<K, V>(&self, environment: impl IntoIterator<Item = (K, V)>) -> PendingProcess
    where
        K: Into<String>,
        V: Into<EnvValue>,
    {
        let mut pending = self.new_pending_process();
        pending.env(environment);
        pending
    }

    /// Begin a process with the given standard input.
    pub fn input(&self, input: impl Into<Vec<u8>>) -> PendingProcess {
        let mut pending = self.new_pending_process();
        pending.input(input);
        pending
    }

    /// Begin a process whose output is discarded.
    pub fn quietly(&self) -> PendingProcess {
        let mut pending = self.new_pending_process();
        pending.quietly();
        pending
    }

    /// Begin a process in TTY mode.
    pub fn tty(&self) -> PendingProcess {
        let mut pending = self.new_pending_process();
        pending.tty();
        pending
    }

    /// Run a process and wait for it to finish.
    pub async fn run(&self, command: impl Into<Command>) -> Result<ProcessResult> {
        self.new_pending_process().run(command).await
    }

    /// Run a process, handing each line of output to the callback.
    pub async fn run_with_output<F>(
        &self,
        command: impl Into<Command>,
        output: F,
    ) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str),
    {
        self.new_pending_process()
            .run_with_output(command, output)
            .await
    }

    /// Start a process in the background.
    pub fn start(&self, command: impl Into<Command>) -> Result<InvokedProcess> {
        self.new_pending_process().start(command)
    }

    /// Start a process in the background with an output callback.
    pub fn start_with_output<F>(
        &self,
        command: impl Into<Command>,
        output: F,
    ) -> Result<InvokedProcess>
    where
        F: FnMut(OutputType, &str) + Send + 'static,
    {
        self.new_pending_process()
            .start_with_output(command, output)
    }

    /// Define a pool of processes.
    pub fn pool(&self, callback: impl FnOnce(&mut Pool)) -> Pool {
        let mut pool = Pool::new(self.clone());
        callback(&mut pool);
        pool
    }

    /// Run a pool of processes and wait for them all to finish.
    pub async fn concurrently(
        &self,
        callback: impl FnOnce(&mut Pool),
    ) -> Result<ProcessPoolResults> {
        self.pool(callback).start()?.wait().await
    }

    /// Run a pool of processes with an output callback, and wait for them
    /// all to finish.
    pub async fn concurrently_with_output<F>(
        &self,
        callback: impl FnOnce(&mut Pool),
        output: F,
    ) -> Result<ProcessPoolResults>
    where
        F: Fn(OutputType, &str, &str) + Send + Sync + 'static,
    {
        self.pool(callback).start_with_output(output)?.wait().await
    }

    /// Define and run a series of piped processes.
    pub async fn pipe(&self, callback: impl FnOnce(&mut Pipe)) -> Result<ProcessResult> {
        let mut pipe = Pipe::new(self.clone());
        callback(&mut pipe);
        pipe.run().await
    }

    /// Define and run a series of piped processes with an output callback.
    pub async fn pipe_with_output<F>(
        &self,
        callback: impl FnOnce(&mut Pipe),
        output: F,
    ) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str, &str),
    {
        let mut pipe = Pipe::new(self.clone());
        callback(&mut pipe);
        pipe.run_with_output(output).await
    }

    /// Pipe the given commands into each other.
    pub async fn pipe_commands<C>(
        &self,
        commands: impl IntoIterator<Item = C>,
    ) -> Result<ProcessResult>
    where
        C: Into<Command>,
    {
        let mut pipe = Pipe::new(self.clone());
        for command in commands {
            pipe.command(command);
        }
        pipe.run().await
    }

    // ------------------------------------------------------------------
    // Fakes
    // ------------------------------------------------------------------

    /// Create a fake process result.
    pub fn result(
        &self,
        output: impl Into<FakeOutput>,
        error_output: impl Into<FakeOutput>,
        exit_code: i32,
    ) -> ProcessResult {
        fake_result(output, error_output, exit_code)
    }

    /// Begin describing a fake process lifecycle.
    pub fn describe(&self) -> FakeProcessDescription {
        FakeProcessDescription::new()
    }

    /// Begin describing a fake process sequence.
    pub fn sequence(&self) -> FakeProcessSequence {
        FakeProcessSequence::new()
    }

    /// Fake every process: each one succeeds without any output.
    pub fn fake(&self) -> &Self {
        self.fake_using(|_| ProcessResult::new("", Some(0), "", ""))
    }

    /// Fake every process with the given callback, which receives the
    /// pending process and returns what it should do.
    pub fn fake_using<F, R>(&self, callback: F) -> &Self
    where
        F: Fn(&PendingProcess) -> R + Send + Sync + 'static,
        R: Into<FakeProcess>,
    {
        self.state.recording.store(true, Ordering::SeqCst);
        let handler: FakeHandler = Arc::new(move |pending| callback(pending).into());
        *self.state.fake_handlers.write().unwrap() = vec![("*".to_string(), handler)];
        self
    }

    /// Fake the commands matching the given patterns (`*` is a wildcard).
    /// Commands that don't match any pattern really run.
    pub fn fake_commands<K, V>(&self, fakes: impl IntoIterator<Item = (K, V)>) -> &Self
    where
        K: Into<String>,
        V: Into<FakeProcess>,
    {
        self.state.recording.store(true, Ordering::SeqCst);
        for (pattern, fake) in fakes {
            let fake = fake.into();
            self.register(pattern.into(), Arc::new(move |_| fake.clone()));
        }
        self
    }

    /// Fake the commands matching the given pattern with a callback.
    pub fn fake_command_using<F, R>(&self, pattern: impl Into<String>, callback: F) -> &Self
    where
        F: Fn(&PendingProcess) -> R + Send + Sync + 'static,
        R: Into<FakeProcess>,
    {
        self.state.recording.store(true, Ordering::SeqCst);
        self.register(
            pattern.into(),
            Arc::new(move |pending| callback(pending).into()),
        );
        self
    }

    fn register(&self, pattern: String, handler: FakeHandler) {
        let mut handlers = self.state.fake_handlers.write().unwrap();
        match handlers
            .iter_mut()
            .find(|(existing, _)| *existing == pattern)
        {
            Some(existing) => existing.1 = handler,
            None => handlers.push((pattern, handler)),
        }
    }

    /// Get the fake handler for the given command line, if it is faked.
    pub(crate) fn fake_for(&self, command: &str) -> Option<FakeHandler> {
        self.state
            .fake_handlers
            .read()
            .unwrap()
            .iter()
            .find(|(pattern, _)| pattern == "*" || Str::is(pattern, command))
            .map(|(_, handler)| handler.clone())
    }

    /// Determine if the factory is faking and recording processes.
    pub fn is_recording(&self) -> bool {
        self.state.recording.load(Ordering::SeqCst)
    }

    /// Record the given process if processes are being recorded.
    pub fn record_if_recording(&self, process: &PendingProcess, result: &ProcessResult) -> &Self {
        if self.is_recording() {
            self.record(process, result);
        }
        self
    }

    /// Record the given process.
    pub fn record(&self, process: &PendingProcess, result: &ProcessResult) -> &Self {
        self.state
            .recorded
            .lock()
            .unwrap()
            .push((process.detached(), result.clone()));
        self
    }

    /// Every recorded process, with its result.
    pub fn recorded(&self) -> Vec<(PendingProcess, ProcessResult)> {
        self.state.recorded.lock().unwrap().clone()
    }

    /// Throw an exception for any process that isn't faked, instead of
    /// running it.
    pub fn prevent_stray_processes(&self) -> &Self {
        self.state
            .prevent_stray_processes
            .store(true, Ordering::SeqCst);
        self
    }

    /// Allow processes that aren't faked to run again.
    pub fn allow_stray_processes(&self) -> &Self {
        self.state
            .prevent_stray_processes
            .store(false, Ordering::SeqCst);
        self
    }

    /// Determine if stray processes are being prevented.
    pub fn preventing_stray_processes(&self) -> bool {
        self.state.prevent_stray_processes.load(Ordering::SeqCst)
    }

    /// Fail if the given (unfaked) command may not run.
    pub(crate) fn ensure_stray_process_allowed(&self, command: &str) -> Result<()> {
        if self.is_recording() && self.preventing_stray_processes() {
            return Err(RuntimeException::new(format!(
                "Attempted process [{command}] without a matching fake."
            ))
            .into());
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Assertions
    // ------------------------------------------------------------------

    fn count_matching(&self, callback: impl Fn(&PendingProcess, &ProcessResult) -> bool) -> usize {
        self.state
            .recorded
            .lock()
            .unwrap()
            .iter()
            .filter(|(process, result)| callback(process, result))
            .count()
    }

    /// Assert that a process running the given command was invoked.
    #[track_caller]
    pub fn assert_ran(&self, command: impl Into<Command>) -> &Self {
        let command = command.into();
        self.assert_ran_with(move |process, _| process.command.as_ref() == Some(&command))
    }

    /// Assert that a process passing the given truth test was invoked.
    #[track_caller]
    pub fn assert_ran_with(
        &self,
        callback: impl Fn(&PendingProcess, &ProcessResult) -> bool,
    ) -> &Self {
        assert!(
            self.count_matching(callback) > 0,
            "An expected process was not invoked."
        );
        self
    }

    /// Assert that a process running the given command was invoked a
    /// given number of times.
    #[track_caller]
    pub fn assert_ran_times(&self, command: impl Into<Command>, times: usize) -> &Self {
        let command = command.into();
        self.assert_ran_times_with(
            move |process, _| process.command.as_ref() == Some(&command),
            times,
        )
    }

    /// Assert that a process passing the given truth test was invoked a
    /// given number of times.
    #[track_caller]
    pub fn assert_ran_times_with(
        &self,
        callback: impl Fn(&PendingProcess, &ProcessResult) -> bool,
        times: usize,
    ) -> &Self {
        let count = self.count_matching(callback);
        assert_eq!(
            count, times,
            "An expected process ran {count} times instead of {times} times."
        );
        self
    }

    /// Assert that the given commands were run, in this order (and that
    /// nothing else ran).
    #[track_caller]
    pub fn assert_ran_in_order<C: Into<Command>>(
        &self,
        commands: impl IntoIterator<Item = C>,
    ) -> &Self {
        let commands: Vec<Command> = commands.into_iter().map(Into::into).collect();
        let recorded = self.recorded();

        assert_eq!(
            recorded.len(),
            commands.len(),
            "Expected {} processes to run, but {} ran.",
            commands.len(),
            recorded.len()
        );

        for (index, (command, (process, _))) in commands.iter().zip(&recorded).enumerate() {
            assert!(
                process.command.as_ref() == Some(command),
                "An expected process (#{}) was not invoked.",
                index + 1
            );
        }

        self
    }

    /// Assert that no process running the given command was invoked.
    #[track_caller]
    pub fn assert_not_ran(&self, command: impl Into<Command>) -> &Self {
        let command = command.into();
        self.assert_not_ran_with(move |process, _| process.command.as_ref() == Some(&command))
    }

    /// Assert that no process passing the given truth test was invoked.
    #[track_caller]
    pub fn assert_not_ran_with(
        &self,
        callback: impl Fn(&PendingProcess, &ProcessResult) -> bool,
    ) -> &Self {
        assert!(
            self.count_matching(callback) == 0,
            "An unexpected process was invoked."
        );
        self
    }

    /// Assert that no process running the given command was invoked.
    #[track_caller]
    pub fn assert_didnt_run(&self, command: impl Into<Command>) -> &Self {
        self.assert_not_ran(command)
    }

    /// Alias of [`assert_didnt_run`](Self::assert_didnt_run).
    #[track_caller]
    pub fn assert_did_not_run(&self, command: impl Into<Command>) -> &Self {
        self.assert_not_ran(command)
    }

    /// Assert that no processes were invoked.
    #[track_caller]
    pub fn assert_nothing_ran(&self) -> &Self {
        assert!(
            self.state.recorded.lock().unwrap().is_empty(),
            "An unexpected process was invoked."
        );
        self
    }
}
