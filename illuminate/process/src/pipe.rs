//! Piping the output of one process into the next.

use illuminate_support::Result;
use illuminate_support::error::InvalidArgumentException;

use crate::command::{Command, IntoTimeout};
use crate::factory::Factory;
use crate::output::OutputType;
use crate::pending::{EnvValue, PendingProcess};
use crate::pool::pending_process_starters;
use crate::result::ProcessResult;

/// A series of processes, defined with
/// [`Process::pipe`](crate::Process::pipe), where the output of each
/// process becomes the input of the next.
///
/// The processes run one after another; if one fails, the pipe stops and
/// its result is returned. Otherwise you get the result of the last one:
///
/// ```
/// use illuminate_process::Process;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let result = Process::pipe(|pipe| {
///     pipe.command("printf 'Laravel\\nSymfony\\nRails\\n'");
///     pipe.command("grep -i laravel");
/// })
/// .await?;
///
/// assert_eq!(result.output(), "Laravel\n");
/// # Ok::<(), illuminate_support::Error>(())
/// # }).unwrap();
/// ```
#[derive(Debug)]
pub struct Pipe {
    factory: Factory,
    processes: Vec<(String, PendingProcess)>,
    next_index: usize,
}

impl Pipe {
    /// Create a new, empty pipe for the given factory.
    pub fn new(factory: Factory) -> Self {
        Self {
            factory,
            processes: Vec::new(),
            next_index: 0,
        }
    }

    pending_process_starters!();

    /// Run the processes in the pipe.
    pub async fn run(self) -> Result<ProcessResult> {
        self.run_with_output(|_, _, _| {}).await
    }

    /// Run the processes in the pipe, handing each line of output to the
    /// callback along with the key of the process that wrote it.
    pub async fn run_with_output<F>(self, mut output: F) -> Result<ProcessResult>
    where
        F: FnMut(OutputType, &str, &str),
    {
        let mut previous: Option<ProcessResult> = None;

        for (key, mut pending) in self.processes {
            if let Some(previous) = &previous {
                if previous.failed() {
                    return Ok(previous.clone());
                }
                if !previous.output().is_empty() {
                    pending.input(previous.output());
                }
            }

            let result = pending
                .execute(&mut |kind: OutputType, line: &str| output(kind, line, &key))
                .await?;

            previous = Some(result);
        }

        previous.ok_or_else(|| {
            InvalidArgumentException::new("A process pipe must contain at least one process.")
                .into()
        })
    }
}
