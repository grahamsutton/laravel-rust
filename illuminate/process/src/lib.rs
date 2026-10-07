//! # Illuminate Process
//!
//! An expressive, minimal API for invoking external processes, focused on
//! the most common use cases and a wonderful developer experience.
//!
//! ```
//! use illuminate_process::Process;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let result = Process::run("echo 'Hello, Artisan'").await?;
//!
//! assert!(result.successful());
//! assert_eq!(result.output(), "Hello, Artisan\n");
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```
//!
//! A quick tour:
//!
//! - **Running processes**: [`Process::run`] waits for a process to finish
//!   and hands back a [`ProcessResult`]. String commands run through
//!   `sh -c`; arrays of arguments (`["php", "artisan", "migrate"]`) run the
//!   program directly. Configure a process with `path`, `env`, `input`,
//!   `timeout`, `idle_timeout`, `forever`, `quietly` and `tty`, and stream
//!   its output line by line with [`Process::run_with_output`].
//! - **Failures**: [`ProcessResult::throw`] turns a failed result into a
//!   [`ProcessFailedException`]; runaway processes fail with a
//!   [`ProcessTimedOutException`] (processes time out after 60 seconds by
//!   default).
//! - **Asynchronous processes**: [`Process::start`] returns an
//!   [`InvokedProcess`] you can poll (`running`, `latest_output`), signal,
//!   stop, and `wait` on.
//! - **Pools and pipes**: [`Process::pool`] and [`Process::concurrently`]
//!   run processes side by side; [`Process::pipe`] feeds the output of each
//!   process into the next.
//! - **Testing**: [`Process::fake`], [`Process::fake_commands`],
//!   [`Process::sequence`], [`Process::describe`],
//!   [`Process::prevent_stray_processes`] and the `assert_*` family. Fakes
//!   live on the [`Factory`] in the current container, so each test gets
//!   its own.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_container::Container;
//! use illuminate_process::Process;
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! # let _guard = Container::set_local_instance(Arc::new(Container::new()));
//! Process::fake_commands([("bash import.sh", Process::result("Imported 10 users.", "", 0))]);
//! Process::prevent_stray_processes();
//!
//! let result = Process::run("bash import.sh").await?;
//!
//! assert_eq!(result.output(), "Imported 10 users.\n");
//! Process::assert_ran("bash import.sh");
//! Process::assert_ran_times("bash import.sh", 1);
//! Process::assert_didnt_run("rm -rf /");
//! # Ok::<(), illuminate_support::Error>(())
//! # }).unwrap();
//! ```

pub mod command;
pub mod exceptions;
pub mod facade;
pub mod factory;
pub mod fake;
pub mod invoked;
pub mod output;
pub mod pending;
pub mod pipe;
pub mod pool;
pub mod provider;
pub mod result;
pub mod signal;

pub use command::{Command, IntoTimeout};
pub use exceptions::{OutOfBoundsException, ProcessFailedException, ProcessTimedOutException};
pub use facade::Process;
pub use factory::Factory;
pub use fake::{FakeOutput, FakeProcess, FakeProcessDescription, FakeProcessSequence};
pub use invoked::InvokedProcess;
pub use output::OutputType;
pub use pending::{DEFAULT_TIMEOUT, EnvValue, PendingProcess};
pub use pipe::Pipe;
pub use pool::{InvokedProcessPool, Pool, ProcessPoolResults};
pub use provider::ProcessServiceProvider;
pub use result::ProcessResult;
