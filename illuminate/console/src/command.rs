//! Console commands.
//!
//! Commands define their name, arguments and options with a single
//! signature, and do their work in `handle`:
//!
//! ```
//! use illuminate_console::{async_trait, Command, Console};
//! use illuminate_support::Result;
//!
//! struct SendEmails;
//!
//! #[async_trait]
//! impl Command for SendEmails {
//!     fn signature(&self) -> &str {
//!         "mail:send {user} {--queue : Whether the job should be queued}"
//!     }
//!
//!     fn description(&self) -> &str {
//!         "Send a marketing email to a user"
//!     }
//!
//!     async fn handle(&self, cmd: Console) -> Result<()> {
//!         let user = cmd.argument("user").unwrap_or_default();
//!
//!         cmd.info(format!("Sending email to: {user}!"));
//!
//!         Ok(())
//!     }
//! }
//! ```

use std::future::Future;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use futures::future::BoxFuture;
use illuminate_support::Result;

use crate::application::Application;
use crate::console::Console;
use crate::input::ArtisanArgs;
use crate::scheduling::Event;

/// The exit code of a successful command.
pub const SUCCESS: i32 = 0;

/// The exit code of a failed command.
pub const FAILURE: i32 = 1;

/// The exit code of a command invoked with invalid input.
pub const INVALID: i32 = 2;

/// An Artisan console command.
#[async_trait]
pub trait Command: Send + Sync + 'static {
    /// The name and signature of the console command, e.g.
    /// `mail:send {user} {--queue}`.
    fn signature(&self) -> &str;

    /// The console command description.
    fn description(&self) -> &str {
        ""
    }

    /// The command's help text, shown by `artisan help <command>`.
    fn help(&self) -> &str {
        ""
    }

    /// Alternative names for the command.
    fn aliases(&self) -> Vec<&str> {
        Vec::new()
    }

    /// Hide the command from the `list` command.
    fn hidden(&self) -> bool {
        false
    }

    /// Additional usage examples, shown by `artisan help <command>`.
    fn usages(&self) -> Vec<&str> {
        Vec::new()
    }

    /// Prompt for missing required arguments instead of failing
    /// (Laravel's `PromptsForMissingInput`).
    fn prompts_for_missing_input(&self) -> bool {
        false
    }

    /// The questions asked for missing arguments, keyed by argument name.
    fn prompt_for_missing_arguments_using(&self) -> Vec<(&str, &str)> {
        Vec::new()
    }

    /// Execute the console command.
    ///
    /// Returning `Ok(())` exits with `0`. Return `cmd.fail("...")` to fail
    /// with exit code `1`, or `cmd.exit(code)` to exit with a specific code.
    /// Any other error is reported and exits with `1`.
    async fn handle(&self, cmd: Console) -> Result<()>;
}

/// Thrown by `cmd.fail(...)` to fail a command with a message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ManuallyFailedException {
    /// The failure message.
    pub message: String,
}

impl ManuallyFailedException {
    /// Create a new exception with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Returned by `cmd.exit(code)` to end a command with a specific exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("The command exited with status code [{code}].")]
pub struct CommandExit {
    /// The exit code.
    pub code: i32,
}

/// Thrown when a command cannot be found.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct CommandNotFoundException {
    /// The full message, including any suggestions.
    pub message: String,
    /// The commands (or namespaces) the user may have meant.
    pub alternatives: Vec<String>,
}

impl CommandNotFoundException {
    /// Create a new exception with the given message and alternatives.
    pub fn new(message: impl Into<String>, alternatives: Vec<String>) -> Self {
        Self {
            message: message.into(),
            alternatives,
        }
    }
}

/// Thrown when a prompt's answer fails validation and the prompt can't
/// ask again (non-interactive input, or a test).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PromptValidationException {
    /// The validation message.
    pub message: String,
    pub(crate) rendered: bool,
}

impl PromptValidationException {
    /// Create a new exception with the given validation message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            rendered: false,
        }
    }
}

type ClosureHandler = Arc<dyn Fn(Console) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// A command defined by a closure (`Artisan::command(...)`).
///
/// ```
/// use illuminate_console::Application;
///
/// let artisan = Application::new();
///
/// artisan.command("mail:send {user}", |cmd| async move {
///     cmd.info(format!("Sending email to: {}!", cmd.argument("user").unwrap()));
///     Ok(())
/// })
/// .purpose("Send a marketing email to a user");
///
/// assert!(artisan.has("mail:send"));
/// ```
#[derive(Clone)]
pub struct ClosureCommand {
    signature: String,
    description: String,
    help: String,
    aliases: Vec<String>,
    hidden: bool,
    handler: ClosureHandler,
    application: Weak<Application>,
}

impl std::fmt::Debug for ClosureCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosureCommand")
            .field("signature", &self.signature)
            .field("description", &self.description)
            .finish()
    }
}

impl ClosureCommand {
    /// Create a closure command (not yet registered with an application).
    pub fn new<F, Fut>(signature: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Console) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self {
            signature: signature.into(),
            description: String::new(),
            help: String::new(),
            aliases: Vec::new(),
            hidden: false,
            handler: Arc::new(move |console| Box::pin(handler(console))),
            application: Weak::new(),
        }
    }

    pub(crate) fn registered_with(mut self, application: &Arc<Application>) -> Self {
        self.application = Arc::downgrade(application);
        self
    }

    fn refresh(&self) {
        if let Some(application) = self.application.upgrade() {
            application.add(self.clone());
        }
    }

    /// The command's name.
    pub fn name(&self) -> String {
        self.signature
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// Set the description of the command.
    pub fn purpose(self, description: impl Into<String>) -> Self {
        self.describe(description)
    }

    /// Set the description of the command.
    pub fn describe(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self.refresh();
        self
    }

    /// Set the help text of the command.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self.refresh();
        self
    }

    /// Add an alias for the command.
    pub fn alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self.refresh();
        self
    }

    /// Hide the command from the `list` command.
    pub fn hide(mut self) -> Self {
        self.hidden = true;
        self.refresh();
        self
    }

    /// Schedule the command, returning the scheduled event so you may
    /// chain its frequency: `.schedule().daily()`.
    pub fn schedule(self) -> Event {
        crate::facades::Schedule::command(&self.name())
    }

    /// Schedule the command with the given arguments.
    pub fn schedule_with(self, args: impl Into<ArtisanArgs>) -> Event {
        crate::facades::Schedule::command_with(&self.name(), args)
    }
}

#[async_trait]
impl Command for ClosureCommand {
    fn signature(&self) -> &str {
        &self.signature
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn help(&self) -> &str {
        &self.help
    }

    fn aliases(&self) -> Vec<&str> {
        self.aliases.iter().map(String::as_str).collect()
    }

    fn hidden(&self) -> bool {
        self.hidden
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        (self.handler)(cmd).await
    }
}
