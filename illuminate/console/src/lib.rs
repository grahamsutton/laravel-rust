//! # Illuminate Console
//!
//! Artisan is the command line interface included with Laravel. This crate
//! is the engine behind it: commands with expressive signatures, beautiful
//! output components, interactive prompts, testing helpers, and the task
//! scheduler.
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_console::prelude::*;
//! use illuminate_container::Container;
//!
//! struct SendEmails;
//!
//! #[async_trait]
//! impl Command for SendEmails {
//!     fn signature(&self) -> &str {
//!         "mail:send {user : The ID of the user} {--queue : Whether the job should be queued}"
//!     }
//!
//!     fn description(&self) -> &str {
//!         "Send a marketing email to a user"
//!     }
//!
//!     async fn handle(&self, cmd: Console) -> Result<()> {
//!         let user = cmd.argument("user").unwrap_or_default();
//!
//!         if cmd.option_bool("queue") {
//!             cmd.components().info(format!("Queueing email for [{user}]"));
//!         } else {
//!             cmd.info(format!("Sending email to: {user}!"));
//!         }
//!
//!         Ok(())
//!     }
//! }
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container);
//!
//! Artisan::register(SendEmails);
//!
//! assert_eq!(Artisan::call("mail:send 1", ()).await.unwrap(), 0);
//! assert_eq!(Artisan::output(), "Sending email to: 1!\n");
//! # }
//! ```

#![warn(missing_docs)]

pub mod application;
mod builtin;
pub mod command;
pub mod components;
pub mod console;
mod descriptor;
pub mod facades;
pub mod formatter;
pub mod input;
pub mod output;
pub mod parser;
pub mod progress;
pub mod prompts;
pub mod provider;
pub mod scheduling;
pub mod table;
pub mod testing;

pub use application::{Application, ConsoleApplication, current_output};
pub use async_trait::async_trait;
pub use builtin::{HelpCommand, ListCommand};
pub use command::{
    ClosureCommand, Command, CommandExit, CommandNotFoundException, FAILURE, INVALID,
    ManuallyFailedException, PromptValidationException, SUCCESS,
};
pub use components::{Components, TaskResult};
pub use console::Console;
pub use facades::{Artisan, Schedule};
pub use formatter::OutputFormatter;
pub use input::{
    ArgValue, ArtisanArgs, InputArgument, InputDefinition, InputOption, InputValue,
    InvalidDefinitionException, InvalidInputException, OptionMode,
};
pub use output::{
    InputReader, LinesReader, Output, PromptStyle, Question, QuestionKind, StdinReader, Verbosity,
};
pub use parser::{Parser, Signature};
pub use progress::ProgressBar;
pub use provider::ConsoleServiceProvider;
pub use scheduling::{CronExpression, Event};
pub use table::{Table, TableStyle};
pub use testing::{CommandResult, PendingCommand};

/// Everything you need to write commands and schedules.
pub mod prelude {
    pub use crate::{
        Application, Artisan, ArtisanArgs, ClosureCommand, Command, Components, Console, Event,
        Output, Schedule, TaskResult, Verbosity, async_trait,
    };
    pub use illuminate_support::Result;
}
