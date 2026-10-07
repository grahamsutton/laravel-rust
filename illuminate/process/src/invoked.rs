//! Processes that have been started in the background.

use std::collections::VecDeque;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use illuminate_support::Result;
use illuminate_support::error::RuntimeException;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::task::JoinHandle;

use crate::command::{Command, IntoTimeout};
use crate::exceptions::ProcessTimedOutException;
use crate::fake::{FakeInvokedProcess, OutputHandler};
use crate::output::{LineBuffer, OutputType};
use crate::pending::PendingProcess;
use crate::result::ProcessResult;
use crate::signal;

/// How long to keep collecting output after a process exits, waiting for
/// its pipes to close. (A background child of the process may keep them
/// open long after the process itself is gone.)
const OUTPUT_GRACE: Duration = Duration::from_millis(100);

/// The most time spent collecting output after a process exits.
const OUTPUT_GRACE_LIMIT: Duration = Duration::from_secs(1);

/// A process running in the background, started with
/// [`Process::start`](crate::Process::start).
///
/// ```
/// use illuminate_process::Process;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let mut process = Process::timeout(120).start("echo importing; sleep 0.1; echo done")?;
///
/// while process.running() {
///     // Do other things while the process runs...
///     tokio::time::sleep(std::time::Duration::from_millis(10)).await;
/// }
///
/// let result = process.wait().await?;
/// assert_eq!(result.output(), "importing\ndone\n");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
///
/// Output is collected in the background by Tokio tasks, so when you poll a
/// process in a loop, `.await` something inside the loop to let them run.
/// Dropping an `InvokedProcess` stops the process, like Symfony does.
pub struct InvokedProcess {
    inner: Inner,
}

enum Inner {
    Real(Box<RealProcess>),
    Fake(Box<FakeInvokedProcess>),
}

impl std::fmt::Debug for InvokedProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvokedProcess")
            .field("command", &self.command())
            .field("fake", &matches!(self.inner, Inner::Fake(_)))
            .finish()
    }
}

impl InvokedProcess {
    pub(crate) fn real(process: RealProcess) -> Self {
        Self {
            inner: Inner::Real(Box::new(process)),
        }
    }

    pub(crate) fn fake(process: FakeInvokedProcess) -> Self {
        Self {
            inner: Inner::Fake(Box::new(process)),
        }
    }

    /// Get the process ID, if the process is still running.
    pub fn id(&mut self) -> Option<u32> {
        match &mut self.inner {
            Inner::Real(process) => process.id(),
            Inner::Fake(process) => process.id(),
        }
    }

    /// Get the command line for the process.
    pub fn command(&self) -> &str {
        match &self.inner {
            Inner::Real(process) => &process.command,
            Inner::Fake(process) => process.command(),
        }
    }

    /// Send a signal to the process.
    ///
    /// ```
    /// use illuminate_process::{Process, signal::SIGTERM};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let mut process = Process::start("sleep 10")?;
    /// process.signal(SIGTERM)?;
    ///
    /// assert_eq!(process.wait().await?.exit_code(), Some(128 + SIGTERM));
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn signal(&mut self, signal: i32) -> Result<&mut Self> {
        match &mut self.inner {
            Inner::Real(process) => process.signal(signal)?,
            Inner::Fake(process) => process.signal(signal),
        }
        Ok(self)
    }

    /// Determine if the process has been sent the given signal.
    pub fn has_received_signal(&self, signal: i32) -> bool {
        match &self.inner {
            Inner::Real(process) => process.signals.contains(&signal),
            Inner::Fake(process) => process.has_received_signal(signal),
        }
    }

    /// Stop the process if it is still running, returning its exit code.
    ///
    /// The process is sent `SIGTERM`, then killed if it hasn't exited
    /// within ten seconds.
    pub async fn stop(&mut self) -> Option<i32> {
        self.stop_with(10, None).await
    }

    /// Stop the process with the given signal (default `SIGTERM`), killing
    /// it if it hasn't exited before the timeout.
    pub async fn stop_with(
        &mut self,
        timeout: impl IntoTimeout,
        signal: Option<i32>,
    ) -> Option<i32> {
        match &mut self.inner {
            Inner::Real(process) => process.stop(timeout.into_timeout(), signal).await,
            Inner::Fake(process) => process.stop(),
        }
    }

    /// Determine if the process is still running.
    pub fn running(&mut self) -> bool {
        match &mut self.inner {
            Inner::Real(process) => process.running(),
            Inner::Fake(process) => process.running(),
        }
    }

    /// Get the standard output the process has written so far.
    pub fn output(&mut self) -> String {
        match &mut self.inner {
            Inner::Real(process) => process.output(OutputType::Out),
            Inner::Fake(process) => process.output(),
        }
    }

    /// Get the error output the process has written so far.
    pub fn error_output(&mut self) -> String {
        match &mut self.inner {
            Inner::Real(process) => process.output(OutputType::Err),
            Inner::Fake(process) => process.error_output(),
        }
    }

    /// Get the standard output written since it was last retrieved.
    pub fn latest_output(&mut self) -> String {
        match &mut self.inner {
            Inner::Real(process) => process.latest_output(OutputType::Out),
            Inner::Fake(process) => process.latest_output(),
        }
    }

    /// Get the error output written since it was last retrieved.
    pub fn latest_error_output(&mut self) -> String {
        match &mut self.inner {
            Inner::Real(process) => process.latest_output(OutputType::Err),
            Inner::Fake(process) => process.latest_error_output(),
        }
    }

    /// Ensure that the process has not timed out, stopping it if it has.
    ///
    /// ```
    /// use std::time::Duration;
    /// use illuminate_process::Process;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let mut process = Process::timeout(0.1).start("sleep 5")?;
    ///
    /// let mut timed_out = false;
    /// while process.running() {
    ///     if let Err(exception) = process.ensure_not_timed_out() {
    ///         assert!(exception.to_string().contains("exceeded the timeout of 0.1 seconds"));
    ///         timed_out = true;
    ///         break;
    ///     }
    ///     tokio::time::sleep(Duration::from_millis(20)).await;
    /// }
    ///
    /// assert!(timed_out);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn ensure_not_timed_out(&mut self) -> Result<(), ProcessTimedOutException> {
        match &mut self.inner {
            Inner::Real(process) => process.ensure_not_timed_out(),
            Inner::Fake(_) => Ok(()),
        }
    }

    /// Wait for the process to finish.
    ///
    /// Fails with a [`ProcessTimedOutException`] if the process runs past
    /// its timeout.
    pub async fn wait(&mut self) -> Result<ProcessResult> {
        match &mut self.inner {
            Inner::Real(process) => {
                let mut handler = process.handler.take();
                let result = process
                    .wait_for(
                        |kind, line| {
                            if let Some(handler) = handler.as_mut() {
                                handler(kind, line);
                            }
                            false
                        },
                        false,
                    )
                    .await;
                process.handler = handler;
                result
            }
            Inner::Fake(process) => Ok(process.wait(None)),
        }
    }

    /// Wait for the process to finish, handing each line of output to the
    /// given callback as it arrives.
    ///
    /// ```
    /// use illuminate_process::Process;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let mut lines = Vec::new();
    ///
    /// Process::start("printf 'one\\ntwo\\n'")?
    ///     .wait_with_output(|kind, line| lines.push(format!("{kind}: {line}")))
    ///     .await?;
    ///
    /// assert_eq!(lines, ["out: one\n", "out: two\n"]);
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn wait_with_output<F>(&mut self, mut output: F) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str),
    {
        match &mut self.inner {
            Inner::Real(process) => {
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
            Inner::Fake(process) => Ok(process.wait(Some(&mut output))),
        }
    }

    /// Wait until the given callback returns `true` for a line of output.
    ///
    /// The process keeps running in the background; the returned result is
    /// a snapshot of it at that moment.
    ///
    /// ```
    /// use illuminate_process::Process;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let mut process = Process::start("echo Booting...; echo Ready...; sleep 10")?;
    ///
    /// let snapshot = process.wait_until(|_, output| output == "Ready...\n").await?;
    ///
    /// assert!(snapshot.see_in_output("Ready"));
    /// assert!(process.running());
    /// process.stop().await;
    /// # Ok::<(), illuminate_support::Error>(())
    /// # }).unwrap();
    /// ```
    pub async fn wait_until<F>(&mut self, mut until: F) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str) -> bool,
    {
        match &mut self.inner {
            Inner::Real(process) => process.wait_for(until, true).await,
            Inner::Fake(process) => Ok(process.wait_until(&mut until)),
        }
    }
}

/// A real operating system process, with its output collected by
/// background tasks.
pub(crate) struct RealProcess {
    command: String,
    child: Child,
    status: Option<ExitStatus>,
    exited_at: Option<Instant>,
    receiver: mpsc::UnboundedReceiver<(OutputType, Vec<u8>)>,
    closed: bool,
    tasks: Vec<JoinHandle<()>>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_offset: usize,
    stderr_offset: usize,
    line_buffers: [LineBuffer; 2],
    lines: VecDeque<(OutputType, String)>,
    handler: Option<OutputHandler>,
    started_at: Instant,
    last_output_at: Arc<Mutex<Instant>>,
    timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    signals: Vec<i32>,
}

/// Something that happened while waiting on a process.
enum Event {
    Chunk(Option<(OutputType, Vec<u8>)>),
    Exited(std::io::Result<ExitStatus>),
    Deadline,
    GraceElapsed,
}

impl RealProcess {
    /// Start the process described by the pending process.
    pub(crate) fn spawn(
        pending: &PendingProcess,
        command: &Command,
        handler: Option<OutputHandler>,
    ) -> Result<Self> {
        let command_line = command.command_line();
        let mut os = command.to_os_command()?;

        if let Some(path) = &pending.path {
            if !path.is_dir() {
                return Err(RuntimeException::new(format!(
                    "The provided cwd \"{}\" does not exist.",
                    path.display()
                ))
                .into());
            }
            os.current_dir(path);
        }

        for (key, value) in &pending.environment {
            match value {
                Some(value) => os.env(key, value),
                None => os.env_remove(key),
            };
        }

        if pending.tty {
            os.stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        } else {
            os.stdin(if pending.input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
            let output = || {
                if pending.quietly {
                    Stdio::null()
                } else {
                    Stdio::piped()
                }
            };
            os.stdout(output()).stderr(output());
        }

        os.kill_on_drop(true);

        let mut child = os.spawn().map_err(|error| {
            RuntimeException::new(format!(
                "Unable to launch process [{command_line}]: {error}"
            ))
        })?;

        let started_at = Instant::now();
        let last_output_at = Arc::new(Mutex::new(started_at));
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut tasks = Vec::new();

        if let Some(stdout) = child.stdout.take() {
            tasks.push(tokio::spawn(read_stream(
                OutputType::Out,
                stdout,
                sender.clone(),
                last_output_at.clone(),
            )));
        }
        if let Some(stderr) = child.stderr.take() {
            tasks.push(tokio::spawn(read_stream(
                OutputType::Err,
                stderr,
                sender.clone(),
                last_output_at.clone(),
            )));
        }
        drop(sender);

        if let (Some(mut stdin), Some(input)) = (child.stdin.take(), pending.input.clone()) {
            tasks.push(tokio::spawn(async move {
                let _ = stdin.write_all(&input).await;
                let _ = stdin.shutdown().await;
            }));
        }

        Ok(Self {
            command: command_line,
            child,
            status: None,
            exited_at: None,
            receiver,
            closed: false,
            tasks,
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_offset: 0,
            stderr_offset: 0,
            line_buffers: [LineBuffer::default(), LineBuffer::default()],
            lines: VecDeque::new(),
            handler,
            started_at,
            last_output_at,
            timeout: pending.timeout,
            idle_timeout: pending.idle_timeout,
            signals: Vec::new(),
        })
    }

    fn id(&mut self) -> Option<u32> {
        self.drain_with_stored_handler();
        self.child.id()
    }

    /// Determine if the process is still alive (it may still be
    /// finishing its output after it exits).
    fn alive(&mut self) -> bool {
        if self.status.is_none() {
            match self.child.try_wait() {
                Ok(Some(status)) => self.exited(status),
                Ok(None) => return true,
                Err(_) => return false,
            }
        }
        false
    }

    fn running(&mut self) -> bool {
        self.drain_with_stored_handler();
        if self.alive() {
            return true;
        }
        self.drain_with_stored_handler();
        !self.closed && self.exited_at.is_some_and(|at| at.elapsed() < OUTPUT_GRACE)
    }

    fn signal(&mut self, signal: i32) -> Result<()> {
        self.drain_with_stored_handler();
        let pid = match self.child.id() {
            Some(pid) if self.alive() => pid,
            _ => {
                return Err(
                    RuntimeException::new("Cannot send signal on a non running process.").into(),
                );
            }
        };
        signal::send(pid, signal)?;
        self.signals.push(signal);
        Ok(())
    }

    async fn stop(&mut self, timeout: Duration, signal: Option<i32>) -> Option<i32> {
        if self.alive() {
            let signal = signal.unwrap_or(signal::SIGTERM);
            let signalled = self
                .child
                .id()
                .is_some_and(|pid| signal::send(pid, signal).is_ok());
            if signalled {
                self.signals.push(signal);
            } else {
                let _ = self.child.start_kill();
            }

            match tokio::time::timeout(timeout, self.child.wait()).await {
                Ok(Ok(status)) => self.exited(status),
                _ => self.kill().await,
            }
        }

        self.finish_streams().await;
        self.drain_with_stored_handler();
        self.exit_code()
    }

    /// Kill the process and wait for it to exit.
    async fn kill(&mut self) {
        let _ = self.child.start_kill();
        if let Ok(status) = self.child.wait().await {
            self.exited(status);
        }
    }

    fn exited(&mut self, status: ExitStatus) {
        self.status = Some(status);
        self.exited_at.get_or_insert_with(Instant::now);
    }

    fn exit_code(&self) -> Option<i32> {
        self.status.and_then(exit_code)
    }

    fn output(&mut self, kind: OutputType) -> String {
        self.drain_with_stored_handler();
        let buffer = match kind {
            OutputType::Out => &self.stdout,
            OutputType::Err => &self.stderr,
        };
        String::from_utf8_lossy(buffer).into_owned()
    }

    fn latest_output(&mut self, kind: OutputType) -> String {
        self.drain_with_stored_handler();
        let (buffer, offset) = match kind {
            OutputType::Out => (&self.stdout, &mut self.stdout_offset),
            OutputType::Err => (&self.stderr, &mut self.stderr_offset),
        };
        let latest = String::from_utf8_lossy(&buffer[*offset..]).into_owned();
        *offset = buffer.len();
        latest
    }

    fn ensure_not_timed_out(&mut self) -> Result<(), ProcessTimedOutException> {
        self.drain_with_stored_handler();
        self.alive();
        match self.exceeded_timeout() {
            Some(exceeded) => {
                let _ = self.child.start_kill();
                Err(exceeded.into_exception(self.snapshot()))
            }
            None => Ok(()),
        }
    }

    /// The result of the process as it stands right now.
    fn snapshot(&self) -> ProcessResult {
        ProcessResult::new(
            self.command.clone(),
            self.exit_code(),
            String::from_utf8_lossy(&self.stdout).into_owned(),
            String::from_utf8_lossy(&self.stderr).into_owned(),
        )
    }

    /// Wait for the process to finish, handing each line of output to
    /// `emit`. When `until` is set, stop waiting as soon as `emit` returns
    /// `true`.
    pub(crate) async fn wait_for<F>(&mut self, mut emit: F, until: bool) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str) -> bool,
    {
        loop {
            if self.drain(&mut emit) && until {
                return Ok(self.snapshot());
            }

            if self.status.is_some() && self.closed {
                return Ok(self.snapshot());
            }

            let event = self.next_event().await;

            match event {
                Event::Chunk(Some((kind, bytes))) => self.ingest(kind, bytes),
                Event::Chunk(None) => self.close_streams(),
                Event::Exited(status) => self.exited(status?),
                Event::Deadline => {
                    if let Some(exceeded) = self.exceeded_timeout() {
                        self.kill().await;
                        self.finish_streams().await;
                        self.drain(&mut emit);
                        return Err(exceeded.into_exception(self.snapshot()).into());
                    }
                }
                Event::GraceElapsed => self.abandon_streams(),
            }
        }
    }

    /// Wait for the next thing to happen: output, the process exiting, a
    /// timeout, or the end of the grace period for collecting output.
    async fn next_event(&mut self) -> Event {
        let reading = !self.closed;
        let waiting = self.status.is_none();
        let deadline = if waiting { self.deadline() } else { None };
        let grace = self.grace();

        let far_away = Instant::now() + Duration::from_secs(60 * 60 * 24 * 365);
        let deadline_at = tokio::time::Instant::from_std(deadline.unwrap_or(far_away));

        tokio::select! {
            chunk = self.receiver.recv(), if reading => Event::Chunk(chunk),
            status = self.child.wait(), if waiting => Event::Exited(status),
            _ = tokio::time::sleep_until(deadline_at), if deadline.is_some() => Event::Deadline,
            _ = tokio::time::sleep(grace.unwrap_or_default()), if grace.is_some() => Event::GraceElapsed,
        }
    }

    /// When the process must be stopped for running too long.
    fn deadline(&self) -> Option<Instant> {
        let general = self.timeout.map(|timeout| self.started_at + timeout);
        let idle = self
            .idle_timeout
            .map(|timeout| *self.last_output_at.lock().unwrap() + timeout);
        general.into_iter().chain(idle).min()
    }

    /// Determine which timeout (if any) the process has exceeded.
    fn exceeded_timeout(&self) -> Option<Exceeded> {
        if self.status.is_some() {
            return None;
        }
        if let Some(timeout) = self.timeout
            && self.started_at.elapsed() >= timeout
        {
            return Some(Exceeded::General(timeout));
        }
        if let Some(timeout) = self.idle_timeout
            && self.last_output_at.lock().unwrap().elapsed() >= timeout
        {
            return Some(Exceeded::Idle(timeout));
        }
        None
    }

    /// How much longer to wait for output after the process has exited.
    fn grace(&self) -> Option<Duration> {
        if self.closed {
            return None;
        }
        let exited_at = self.exited_at?;
        Some(OUTPUT_GRACE.min(OUTPUT_GRACE_LIMIT.saturating_sub(exited_at.elapsed())))
    }

    /// Collect the rest of the output of a process that has exited.
    async fn finish_streams(&mut self) {
        while let Some(grace) = self.grace() {
            tokio::select! {
                chunk = self.receiver.recv() => match chunk {
                    Some((kind, bytes)) => self.ingest(kind, bytes),
                    None => self.close_streams(),
                },
                _ = tokio::time::sleep(grace) => self.abandon_streams(),
            }
        }
    }

    /// Stop waiting for output that may never come (a background child of
    /// the process is holding its pipes open).
    fn abandon_streams(&mut self) {
        while let Ok((kind, bytes)) = self.receiver.try_recv() {
            self.ingest(kind, bytes);
        }
        for task in &self.tasks {
            task.abort();
        }
        self.close_streams();
    }

    /// Add a chunk of output to the buffers.
    fn ingest(&mut self, kind: OutputType, bytes: Vec<u8>) {
        let index = match kind {
            OutputType::Out => {
                self.stdout.extend_from_slice(&bytes);
                0
            }
            OutputType::Err => {
                self.stderr.extend_from_slice(&bytes);
                1
            }
        };
        for line in self.line_buffers[index].push(&bytes) {
            self.lines.push_back((kind, line));
        }
    }

    /// Both output streams have finished: flush any partial lines.
    fn close_streams(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        for (index, kind) in [(0, OutputType::Out), (1, OutputType::Err)] {
            if let Some(line) = self.line_buffers[index].flush() {
                self.lines.push_back((kind, line));
            }
        }
    }

    /// Take in everything the readers have collected so far, handing each
    /// complete line to `emit`. Returns `true` if `emit` asked to stop.
    fn drain<F>(&mut self, emit: &mut F) -> bool
    where
        F: FnMut(OutputType, &str) -> bool,
    {
        loop {
            while let Some((kind, line)) = self.lines.pop_front() {
                if emit(kind, &line) {
                    return true;
                }
            }

            if self.closed {
                return false;
            }

            match self.receiver.try_recv() {
                Ok((kind, bytes)) => self.ingest(kind, bytes),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => self.close_streams(),
            }
        }
    }

    fn drain_with_stored_handler(&mut self) {
        let mut handler = self.handler.take();
        self.drain(&mut |kind, line| {
            if let Some(handler) = handler.as_mut() {
                handler(kind, line);
            }
            false
        });
        self.handler = handler;
    }
}

impl Drop for RealProcess {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// The timeout a process has exceeded.
enum Exceeded {
    General(Duration),
    Idle(Duration),
}

impl Exceeded {
    fn into_exception(self, result: ProcessResult) -> ProcessTimedOutException {
        match self {
            Exceeded::General(timeout) => ProcessTimedOutException::new(result, timeout),
            Exceeded::Idle(timeout) => ProcessTimedOutException::idle(result, timeout),
        }
    }
}

/// The exit code for an exit status; processes killed by a signal report
/// `128 + signal`, like the shell.
fn exit_code(status: ExitStatus) -> Option<i32> {
    if let Some(code) = status.code() {
        return Some(code);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return Some(128 + signal);
        }
    }

    None
}

/// Read a stream until it closes, sending each chunk down the channel.
async fn read_stream(
    kind: OutputType,
    mut stream: impl AsyncRead + Unpin,
    sender: mpsc::UnboundedSender<(OutputType, Vec<u8>)>,
    last_output_at: Arc<Mutex<Instant>>,
) {
    let mut buffer = vec![0u8; 8192];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                *last_output_at.lock().unwrap() = Instant::now();
                if sender.send((kind, buffer[..read].to_vec())).is_err() {
                    break;
                }
            }
        }
    }
}
