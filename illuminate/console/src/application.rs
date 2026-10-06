//! The console application: Artisan itself.
//!
//! The application holds every registered command, parses the command
//! line, runs the right command and renders any errors.
//!
//! ```
//! use illuminate_console::Application;
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let artisan = Application::new();
//!
//! artisan.command("greet {name}", |cmd| async move {
//!     cmd.line(format!("Hello, {}!", cmd.argument("name").unwrap()));
//!     Ok(())
//! });
//!
//! let status = artisan.call("greet Taylor", ()).await.unwrap();
//!
//! assert_eq!(status, 0);
//! assert_eq!(artisan.output(), "Hello, Taylor!\n");
//! # }
//! ```

use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use illuminate_support::{Error, Result};
use indexmap::IndexMap;
use regex::Regex;

use crate::builtin::{HelpCommand, ListCommand};
use crate::command::{
    ClosureCommand, Command, CommandExit, CommandNotFoundException, FAILURE, INVALID,
    ManuallyFailedException, PromptValidationException, SUCCESS,
};
use crate::components::Components;
use crate::console::Console;
use crate::descriptor;
use crate::formatter::OutputFormatter;
use crate::input::{
    ArgValue, ArtisanArgs, Input, InputArgument, InputDefinition, InputOption, InputValue,
    InvalidDefinitionException, InvalidInputException, OptionMode, tokenize,
};
use crate::output::{Output, Verbosity};
use crate::parser::Parser;

tokio::task_local! {
    static CURRENT_OUTPUT: Output;
}

/// The output of the command currently running on this task, or the
/// terminal when no command is running. Used by the prompts.
pub fn current_output() -> Output {
    static STDOUT: OnceLock<Output> = OnceLock::new();

    CURRENT_OUTPUT
        .try_with(Output::clone)
        .unwrap_or_else(|_| STDOUT.get_or_init(Output::stdout).clone())
}

/// Run the future with the given output as the current output.
pub(crate) async fn scope_output<F: Future>(output: Output, future: F) -> F::Output {
    CURRENT_OUTPUT.scope(output, future).await
}

/// A registered command, with its parsed signature.
pub(crate) struct Registered {
    pub(crate) command: Arc<dyn Command>,
    pub(crate) name: String,
    pub(crate) definition: InputDefinition,
}

type Reporter = Arc<dyn Fn(&Error) + Send + Sync>;
type StartingHook = Arc<dyn Fn(&str) + Send + Sync>;
type FinishedHook = Arc<dyn Fn(&str, i32) + Send + Sync>;

/// Where a command's input comes from.
enum Source {
    Tokens(Vec<String>),
    Named(Vec<(String, ArgValue)>),
}

/// The Artisan console application.
pub struct Application {
    this: Weak<Application>,
    name: RwLock<String>,
    version: RwLock<String>,
    binary: RwLock<String>,
    commands: RwLock<IndexMap<String, Arc<Registered>>>,
    aliases: RwLock<IndexMap<String, String>>,
    last_output: Mutex<String>,
    reporter: RwLock<Option<Reporter>>,
    starting: RwLock<Vec<StartingHook>>,
    finished: RwLock<Vec<FinishedHook>>,
}

impl std::fmt::Debug for Application {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Application")
            .field("name", &self.name())
            .field("version", &self.version())
            .field(
                "commands",
                &self.commands.read().unwrap().keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// An alias of [`Application`], for code that also deals with the
/// foundation's `Application`.
pub type ConsoleApplication = Application;

impl Application {
    /// Create a new console application, named "Laravel Framework", with
    /// the built-in `list` and `help` commands.
    pub fn new() -> Arc<Self> {
        let application = Arc::new_cyclic(|this| Self {
            this: this.clone(),
            name: RwLock::new("Laravel Framework".to_string()),
            version: RwLock::new(env!("CARGO_PKG_VERSION").to_string()),
            binary: RwLock::new("artisan".to_string()),
            commands: RwLock::new(IndexMap::new()),
            aliases: RwLock::new(IndexMap::new()),
            last_output: Mutex::new(String::new()),
            reporter: RwLock::new(None),
            starting: RwLock::new(Vec::new()),
            finished: RwLock::new(Vec::new()),
        });

        application.add(HelpCommand);
        application.add(ListCommand);

        application
    }

    fn arc(&self) -> Arc<Application> {
        self.this
            .upgrade()
            .expect("the console application has been dropped")
    }

    // ------------------------------------------------------------------
    // Identity
    // ------------------------------------------------------------------

    /// The application's name ("Laravel Framework").
    pub fn name(&self) -> String {
        self.name.read().unwrap().clone()
    }

    /// Set the application's name.
    pub fn set_name(&self, name: impl Into<String>) {
        *self.name.write().unwrap() = name.into();
    }

    /// The application's version.
    pub fn version(&self) -> String {
        self.version.read().unwrap().clone()
    }

    /// Set the application's version.
    pub fn set_version(&self, version: impl Into<String>) {
        *self.version.write().unwrap() = version.into();
    }

    /// The name of the binary used to run Artisan ("artisan").
    pub fn binary(&self) -> String {
        self.binary.read().unwrap().clone()
    }

    /// Set the name of the binary used to run Artisan.
    pub fn set_binary(&self, binary: impl Into<String>) {
        *self.binary.write().unwrap() = binary.into();
    }

    /// The long version: `Laravel Framework <info>13.0.0</info>`.
    pub fn long_version(&self) -> String {
        let (name, version) = (self.name(), self.version());

        match (name.is_empty(), version.is_empty()) {
            (false, false) => format!("{name} <info>{version}</info>"),
            (false, true) => name,
            (true, _) => "Console Tool".to_string(),
        }
    }

    /// The options every command accepts (`--help`, `--quiet`, `-v`...).
    pub fn default_definition() -> InputDefinition {
        static DEFINITION: OnceLock<InputDefinition> = OnceLock::new();

        DEFINITION
            .get_or_init(|| {
                InputDefinition::new(
                    vec![InputArgument::required("command").describe("The command to execute")],
                    vec![
                        InputOption::new(
                            "help",
                            Some("h"),
                            OptionMode::None,
                            "Display help for the given command. When no command is given display help for the <info>list</info> command",
                        ),
                        InputOption::new("silent", None, OptionMode::None, "Do not output any message"),
                        InputOption::new(
                            "quiet",
                            Some("q"),
                            OptionMode::None,
                            "Only errors are displayed. All other output is suppressed",
                        ),
                        InputOption::new(
                            "verbose",
                            Some("v|vv|vvv"),
                            OptionMode::None,
                            "Increase the verbosity of messages: 1 for normal output, 2 for more verbose output and 3 for debug",
                        ),
                        InputOption::new("version", Some("V"), OptionMode::None, "Display this application version"),
                        InputOption::new("ansi", None, OptionMode::Negatable, "Force (or disable --no-ansi) ANSI output"),
                        InputOption::new(
                            "no-interaction",
                            Some("n"),
                            OptionMode::None,
                            "Do not ask any interactive question",
                        ),
                        InputOption::new(
                            "env",
                            None,
                            OptionMode::Optional,
                            "The environment the command should run under",
                        ),
                    ],
                )
                .expect("the default definition is valid")
            })
            .clone()
    }

    // ------------------------------------------------------------------
    // Registering commands
    // ------------------------------------------------------------------

    /// Register a command.
    ///
    /// # Panics
    ///
    /// Panics when the command's signature is malformed: that is a bug in
    /// the command which should be fixed right away.
    pub fn add<C: Command>(&self, command: C) {
        self.add_arc(Arc::new(command));
    }

    /// Register a shared command.
    ///
    /// # Panics
    ///
    /// Panics when the command's signature is malformed.
    pub fn add_arc(&self, command: Arc<dyn Command>) {
        if let Err(error) = self.try_add(command.clone()) {
            panic!("Invalid signature [{}]: {error}", command.signature());
        }
    }

    /// Register a command, returning an error if its signature is malformed.
    pub fn try_add(&self, command: Arc<dyn Command>) -> Result<(), InvalidDefinitionException> {
        let signature = Parser::parse(command.signature())?;
        let definition = signature.definition();
        let name = signature.name.clone();

        // Make sure the command doesn't redefine the application's options.
        let _ = definition.merged_with(&Self::default_definition(), true);

        let registered = Arc::new(Registered {
            command: command.clone(),
            name: name.clone(),
            definition,
        });

        {
            let mut aliases = self.aliases.write().unwrap();
            aliases.retain(|_, target| *target != name);
            for alias in command.aliases() {
                aliases.insert(alias.to_string(), name.clone());
            }
        }

        self.commands.write().unwrap().insert(name, registered);

        Ok(())
    }

    /// Register a closure based command.
    pub fn command<F, Fut>(&self, signature: impl Into<String>, handler: F) -> ClosureCommand
    where
        F: Fn(Console) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let command = ClosureCommand::new(signature, handler).registered_with(&self.arc());
        self.add(command.clone());
        command
    }

    /// Remove a command.
    pub fn forget(&self, name: &str) {
        self.commands.write().unwrap().shift_remove(name);
        self.aliases
            .write()
            .unwrap()
            .retain(|_, target| target != name);
    }

    /// Determine if a command (or alias) with the given name exists.
    pub fn has(&self, name: &str) -> bool {
        self.registered(name).is_some()
    }

    /// Get a command by its exact name or alias.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Command>> {
        self.registered(name)
            .map(|registered| registered.command.clone())
    }

    /// Every registered command, keyed by name.
    pub fn all(&self) -> IndexMap<String, Arc<dyn Command>> {
        self.commands
            .read()
            .unwrap()
            .iter()
            .map(|(name, registered)| (name.clone(), registered.command.clone()))
            .collect()
    }

    pub(crate) fn registered(&self, name: &str) -> Option<Arc<Registered>> {
        if let Some(registered) = self.commands.read().unwrap().get(name) {
            return Some(registered.clone());
        }

        let target = self.aliases.read().unwrap().get(name).cloned()?;
        self.commands.read().unwrap().get(&target).cloned()
    }

    /// Every command and alias name, with the command it refers to.
    pub(crate) fn registry_entries(
        &self,
        namespace: Option<&str>,
    ) -> Vec<(String, Arc<Registered>)> {
        let mut entries: Vec<(String, Arc<Registered>)> = self
            .commands
            .read()
            .unwrap()
            .iter()
            .map(|(name, registered)| (name.clone(), registered.clone()))
            .collect();

        for (alias, target) in self.aliases.read().unwrap().iter() {
            if entries.iter().any(|(name, _)| name == alias) {
                continue;
            }
            if let Some(registered) = self.commands.read().unwrap().get(target) {
                entries.push((alias.clone(), registered.clone()));
            }
        }

        match namespace {
            Some(namespace) => {
                let limit = namespace.matches(':').count() + 1;
                entries
                    .into_iter()
                    .filter(|(name, _)| Self::extract_namespace(name, Some(limit)) == namespace)
                    .collect()
            }
            None => entries,
        }
    }

    // ------------------------------------------------------------------
    // Finding commands
    // ------------------------------------------------------------------

    /// Extract the namespace of a command name (`make:model` → `make`).
    pub fn extract_namespace(name: &str, limit: Option<usize>) -> String {
        let mut parts: Vec<&str> = name.split(':').collect();
        parts.pop();

        if let Some(limit) = limit {
            parts.truncate(limit);
        }

        parts.join(":")
    }

    /// Every namespace that contains visible commands.
    pub fn namespaces(&self) -> Vec<String> {
        let mut namespaces: Vec<String> = Vec::new();

        for registered in self.commands.read().unwrap().values() {
            if registered.command.hidden() {
                continue;
            }

            let names = std::iter::once(registered.name.clone())
                .chain(registered.command.aliases().into_iter().map(String::from));

            for name in names {
                let parts: Vec<&str> = name.split(':').collect();
                for index in 1..parts.len() {
                    let namespace = parts[..index].join(":");
                    if !namespaces.contains(&namespace) {
                        namespaces.push(namespace);
                    }
                }
            }
        }

        namespaces
    }

    fn abbreviation_expression(name: &str) -> String {
        name.split(':')
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join("[^:]*:")
            + "[^:]*"
    }

    /// Find the full namespace for the given (possibly abbreviated) namespace.
    pub fn find_namespace(&self, namespace: &str) -> Result<String, CommandNotFoundException> {
        let all = self.namespaces();
        let expression = Self::abbreviation_expression(namespace);
        let regex = Regex::new(&format!("^{expression}")).expect("escaped expression");

        let matches: Vec<String> = all
            .iter()
            .filter(|ns| regex.is_match(ns))
            .cloned()
            .collect();

        if matches.is_empty() {
            let mut message =
                format!("There are no commands defined in the \"{namespace}\" namespace.");
            let alternatives = find_alternatives(namespace, &all);
            append_alternatives(&mut message, &alternatives);
            return Err(CommandNotFoundException::new(message, alternatives));
        }

        let exact = matches.iter().any(|ns| ns == namespace);

        if matches.len() > 1 && !exact {
            return Err(CommandNotFoundException::new(
                format!(
                    "The namespace \"{namespace}\" is ambiguous.\nDid you mean one of these?\n{}.",
                    abbreviation_suggestions(&matches)
                ),
                matches,
            ));
        }

        Ok(if exact {
            namespace.to_string()
        } else {
            matches[0].clone()
        })
    }

    /// Find a command by its name, alias or an unambiguous abbreviation.
    pub fn find(&self, name: &str) -> Result<Arc<dyn Command>, CommandNotFoundException> {
        self.find_registered(name)
            .map(|registered| registered.command.clone())
    }

    pub(crate) fn find_registered(
        &self,
        name: &str,
    ) -> Result<Arc<Registered>, CommandNotFoundException> {
        if let Some(registered) = self.registered(name) {
            return Ok(registered);
        }

        let all: Vec<String> = self
            .registry_entries(None)
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        let expression = Self::abbreviation_expression(name);
        let sensitive = Regex::new(&format!("^{expression}")).expect("escaped expression");
        let insensitive = Regex::new(&format!("(?i)^{expression}")).expect("escaped expression");
        let exact = Regex::new(&format!("(?i)^{expression}$")).expect("escaped expression");

        let mut matches: Vec<String> = all
            .iter()
            .filter(|n| sensitive.is_match(n))
            .cloned()
            .collect();
        if matches.is_empty() {
            matches = all
                .iter()
                .filter(|n| insensitive.is_match(n))
                .cloned()
                .collect();
        }

        if matches.is_empty() || !matches.iter().any(|n| exact.is_match(n)) {
            if let Some(position) = name.rfind(':') {
                self.find_namespace(&name[..position])?;
            }

            let mut message = format!("Command \"{name}\" is not defined.");
            let alternatives: Vec<String> = find_alternatives(name, &all)
                .into_iter()
                .filter(|alternative| {
                    self.registered(alternative)
                        .is_some_and(|r| !r.command.hidden())
                })
                .collect();
            append_alternatives(&mut message, &alternatives);

            return Err(CommandNotFoundException::new(message, alternatives));
        }

        if matches.len() > 1 {
            let resolved: Vec<(String, String)> = matches
                .iter()
                .filter_map(|n| self.registered(n).map(|r| (n.clone(), r.name.clone())))
                .collect();
            matches = resolved
                .iter()
                .filter(|(n, command)| n == command || !matches.contains(command))
                .map(|(n, _)| n.clone())
                .collect();
        }

        if matches.len() > 1 {
            let visible: Vec<String> = matches
                .iter()
                .filter(|n| self.registered(n).is_some_and(|r| !r.command.hidden()))
                .cloned()
                .collect();

            if visible.len() > 1 {
                let usable_width = crate::output::terminal_width().saturating_sub(10);
                let max = visible.iter().map(|n| n.chars().count()).max().unwrap_or(0);
                let abbreviations: Vec<String> = visible
                    .iter()
                    .map(|n| {
                        let description = self
                            .registered(n)
                            .map(|r| r.command.description().to_string())
                            .unwrap_or_default();
                        let abbreviation = format!("{n:<max$} {description}");
                        if abbreviation.chars().count() > usable_width {
                            format!(
                                "{}...",
                                abbreviation
                                    .chars()
                                    .take(usable_width.saturating_sub(3))
                                    .collect::<String>()
                            )
                        } else {
                            abbreviation
                        }
                    })
                    .collect();

                return Err(CommandNotFoundException::new(
                    format!(
                        "Command \"{name}\" is ambiguous.\nDid you mean one of these?\n{}.",
                        abbreviation_suggestions(&abbreviations)
                    ),
                    visible,
                ));
            }

            matches = if visible.is_empty() { matches } else { visible };
        }

        let registered = self.registered(&matches[0]).ok_or_else(|| {
            CommandNotFoundException::new(format!("The command \"{name}\" does not exist."), vec![])
        })?;

        if registered.command.hidden() {
            return Err(CommandNotFoundException::new(
                format!("The command \"{name}\" does not exist."),
                vec![],
            ));
        }

        Ok(registered)
    }

    // ------------------------------------------------------------------
    // Running commands
    // ------------------------------------------------------------------

    /// Run the application with the process' arguments (`argv[0]` is the
    /// binary), writing to the terminal. Returns the exit code.
    ///
    /// ```no_run
    /// #[tokio::main]
    /// async fn main() {
    ///     let artisan = illuminate_console::Application::new();
    ///
    ///     std::process::exit(artisan.run(std::env::args()).await);
    /// }
    /// ```
    pub async fn run<I, S>(&self, argv: I) -> i32
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let tokens: Vec<String> = argv.into_iter().map(Into::into).skip(1).collect();
        self.run_with_output(tokens, &Output::stdout()).await
    }

    /// Run the application with the given tokens (without the binary),
    /// writing to the given output. Errors are rendered, and the exit code
    /// is returned.
    pub async fn run_with_output<I, S>(&self, tokens: I, output: &Output) -> i32
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let tokens: Vec<String> = tokens.into_iter().map(Into::into).collect();

        configure_io_from_tokens(&tokens, output);

        if has_parameter_option(&tokens, &["--version", "-V"]) {
            output.writeln(self.long_version());
            return SUCCESS;
        }

        let result = match first_argument(&tokens) {
            Some(name) => self.execute(&name, Source::Tokens(tokens), output).await,
            None if has_parameter_option(&tokens, &["--help", "-h"]) => {
                let tokens = vec!["help".to_string(), "list".to_string()];
                self.execute("help", Source::Tokens(tokens), output).await
            }
            None => {
                let mut tokens = tokens;
                tokens.insert(0, "list".to_string());
                self.execute("list", Source::Tokens(tokens), output).await
            }
        };

        match result {
            Ok(code) => code,
            Err(error) => self.render_exception(&error, output),
        }
    }

    /// Call a command by name (or as a full command line), capturing its
    /// output. Retrieve the output afterwards with [`Application::output`].
    ///
    /// ```
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// use illuminate_support::json;
    ///
    /// let artisan = illuminate_console::Application::new();
    /// artisan.command("mail:send {user} {--queue=}", |cmd| async move {
    ///     cmd.line(format!("{} on {}", cmd.argument("user").unwrap(), cmd.option("queue").unwrap()));
    ///     Ok(())
    /// });
    ///
    /// artisan.call("mail:send", json!({"user": 1, "--queue": "default"})).await.unwrap();
    /// assert_eq!(artisan.output(), "1 on default\n");
    ///
    /// artisan.call("mail:send 2 --queue=high", ()).await.unwrap();
    /// assert_eq!(artisan.output(), "2 on high\n");
    /// # }
    /// ```
    pub async fn call(&self, command: &str, args: impl Into<ArtisanArgs>) -> Result<i32> {
        let output = Output::buffered();
        let result = self.call_with_output(command, args, &output).await;
        *self.last_output.lock().unwrap() = output.contents();
        result
    }

    /// Call a command, writing to the given output.
    pub async fn call_with_output(
        &self,
        command: &str,
        args: impl Into<ArtisanArgs>,
        output: &Output,
    ) -> Result<i32> {
        let (name, source) = Self::parse_command(command, args.into());

        if !self.has(&name) {
            return Err(CommandNotFoundException::new(
                format!("The command \"{name}\" does not exist."),
                vec![],
            )
            .into());
        }

        match &source {
            Source::Tokens(tokens) => configure_io_from_tokens(tokens, output),
            Source::Named(named) => configure_io_from_named(named, output),
        }

        self.execute(&name, source, output).await
    }

    /// Call a command from within another command.
    pub(crate) async fn call_from_console(
        &self,
        parent: &Console,
        command: &str,
        args: ArtisanArgs,
        output: Output,
    ) -> Result<i32> {
        let (name, source) = Self::parse_command(command, args);
        let context = parent.context();

        let source = match source {
            Source::Tokens(mut tokens) => {
                for (key, value) in context {
                    if !has_parameter_option(&tokens, &[key.as_str()]) {
                        tokens.push(match value {
                            InputValue::String(value) => format!("{key}={value}"),
                            _ => key,
                        });
                    }
                }
                Source::Tokens(tokens)
            }
            Source::Named(mut named) => {
                for (key, value) in context.into_iter().rev() {
                    if !named.iter().any(|(k, _)| *k == key) {
                        let value = match value {
                            InputValue::String(value) => ArgValue::String(value),
                            other => ArgValue::Bool(other.as_bool()),
                        };
                        named.insert(0, (key, value));
                    }
                }
                Source::Named(named)
            }
        };

        self.execute(&name, source, &output).await
    }

    fn parse_command(command: &str, args: ArtisanArgs) -> (String, Source) {
        let mut tokens = tokenize(command);
        if tokens.is_empty() {
            tokens.push(command.to_string());
        }
        let name = tokens[0].clone();

        if !args.named.is_empty() && tokens.len() == 1 && args.tokens.is_empty() {
            return (name, Source::Named(args.named));
        }

        tokens.extend(args.tokens.iter().cloned());
        tokens.extend(args.named_tokens());

        (name, Source::Tokens(tokens))
    }

    async fn execute(&self, name: &str, source: Source, output: &Output) -> Result<i32> {
        let registered = self.find_registered(name)?;

        let wants_help = match &source {
            Source::Tokens(tokens) => has_parameter_option(tokens, &["--help", "-h"]),
            Source::Named(named) => named.iter().any(|(key, value)| {
                (key == "--help" || key == "-h") && *value != ArgValue::Bool(false)
            }),
        };

        if wants_help && registered.name != "help" {
            output.write(descriptor::describe_command(self, &registered));
            return Ok(SUCCESS);
        }

        let definition = registered
            .definition
            .merged_with(&Self::default_definition(), true);

        let mut input = match source {
            Source::Tokens(tokens) => Input::from_tokens(tokens, &definition)?,
            Source::Named(mut named) => {
                named.retain(|(key, _)| key != "command");
                named.insert(
                    0,
                    (
                        "command".to_string(),
                        ArgValue::String(registered.name.clone()),
                    ),
                );
                Input::from_named(&named, &definition)?
            }
        };

        if registered.command.prompts_for_missing_input() && output.is_interactive() {
            self.prompt_for_missing_arguments(&registered, &mut input, output)?;
        }

        input.validate()?;

        let hooks = self.starting.read().unwrap().clone();
        for hook in hooks {
            hook(&registered.name);
        }

        let console = Console::new(registered.name.clone(), input, output.clone(), self.arc());
        let result = CURRENT_OUTPUT
            .scope(output.clone(), registered.command.handle(console))
            .await;

        let result = match result {
            Ok(()) => Ok(SUCCESS),
            Err(error) => {
                if let Some(exit) = error.downcast_ref::<CommandExit>() {
                    Ok(exit.code)
                } else if let Some(failed) = error.downcast_ref::<ManuallyFailedException>() {
                    Components::new(output).error(&failed.message);
                    Ok(FAILURE)
                } else if let Some(failed) = error.downcast_ref::<PromptValidationException>() {
                    if !failed.rendered {
                        Components::new(output).error(&failed.message);
                    }
                    Ok(FAILURE)
                } else {
                    Err(error)
                }
            }
        };

        let code = match &result {
            Ok(code) => *code,
            Err(_) => FAILURE,
        };
        let hooks = self.finished.read().unwrap().clone();
        for hook in hooks {
            hook(&registered.name, code);
        }

        result
    }

    fn prompt_for_missing_arguments(
        &self,
        registered: &Registered,
        input: &mut Input,
        output: &Output,
    ) -> Result<()> {
        let questions = registered.command.prompt_for_missing_arguments_using();

        for argument in registered.definition.arguments() {
            let missing = match input.argument(&argument.name) {
                Some(InputValue::Array(values)) => values.is_empty(),
                Some(InputValue::Null) | None => true,
                Some(_) => false,
            };

            if !argument.required || !missing {
                continue;
            }

            let label = questions
                .iter()
                .find(|(name, _)| *name == argument.name)
                .map(|(_, question)| question.to_string())
                .unwrap_or_else(|| {
                    let subject = if argument.description.is_empty() {
                        format!("the {}", argument.name)
                    } else {
                        illuminate_support::Str::lcfirst(&argument.description)
                    };
                    format!("What is {subject}?")
                });

            let name = argument.name.clone();
            let answer = crate::prompts::text(label)
                .validate(move |value| value.is_empty().then(|| format!("The {name} is required.")))
                .prompt_on(output)?;

            let value = if argument.array {
                InputValue::Array(vec![answer])
            } else {
                InputValue::String(answer)
            };
            input.set_argument(&argument.name, value)?;
        }

        Ok(())
    }

    /// The output of the most recent [`Application::call`].
    pub fn output(&self) -> String {
        self.last_output.lock().unwrap().clone()
    }

    // ------------------------------------------------------------------
    // Errors & hooks
    // ------------------------------------------------------------------

    /// Report exceptions thrown by commands using the given callback (the
    /// foundation wires this to the exception handler).
    pub fn report_exceptions_using(&self, reporter: impl Fn(&Error) + Send + Sync + 'static) {
        *self.reporter.write().unwrap() = Some(Arc::new(reporter));
    }

    /// Report an exception.
    pub fn report(&self, error: &Error) {
        let reporter = self.reporter.read().unwrap().clone();
        if let Some(reporter) = reporter {
            reporter(error);
        }
    }

    /// Register a callback to run before each command (Laravel's
    /// `CommandStarting` event).
    pub fn on_command_starting(&self, callback: impl Fn(&str) + Send + Sync + 'static) {
        self.starting.write().unwrap().push(Arc::new(callback));
    }

    /// Register a callback to run after each command with its exit code
    /// (Laravel's `CommandFinished` event).
    pub fn on_command_finished(&self, callback: impl Fn(&str, i32) + Send + Sync + 'static) {
        self.finished.write().unwrap().push(Arc::new(callback));
    }

    /// Render an exception for the console, returning the exit code to use.
    pub fn render_exception(&self, error: &Error, output: &Output) -> i32 {
        if let Some(not_found) = error.downcast_ref::<CommandNotFoundException>() {
            let mut message = not_found
                .message
                .split('.')
                .next()
                .unwrap_or_default()
                .to_string();
            let components = Components::new(output).verbosity(Verbosity::Quiet);

            if not_found.alternatives.is_empty() {
                components.error(message);
            } else {
                message.push_str(". Did you mean one of these?");
                components.error(message);
                components.bullet_list(&not_found.alternatives);
                output.write_with("", true, Verbosity::Quiet);
            }

            return FAILURE;
        }

        if error.downcast_ref::<InvalidInputException>().is_some()
            || error.downcast_ref::<InvalidDefinitionException>().is_some()
        {
            render_throwable(&error.to_string(), None, output);
            return INVALID;
        }

        self.report(error);

        let chain: Vec<String> = error
            .chain()
            .skip(1)
            .map(|cause| cause.to_string())
            .collect();
        let details = output.is_verbose().then_some(chain);
        render_throwable(&error.to_string(), details, output);

        FAILURE
    }
}

/// Render an exception as Symfony does: a red block holding the message.
fn render_throwable(message: &str, causes: Option<Vec<String>>, output: &Output) {
    output.write_with("", true, Verbosity::Quiet);

    let message = message.trim();
    let terminal_width = output.width().saturating_sub(1).max(10);
    let title = if message.is_empty() {
        "  [Error]  ".to_string()
    } else {
        String::new()
    };
    let mut length = title.chars().count();

    let mut lines: Vec<(String, usize)> = Vec::new();
    for line in message.lines() {
        let chars: Vec<char> = line.chars().collect();
        let chunks: Vec<String> = if chars.is_empty() {
            vec![String::new()]
        } else {
            chars
                .chunks(terminal_width.saturating_sub(4).max(1))
                .map(|chunk| chunk.iter().collect())
                .collect()
        };
        for chunk in chunks {
            let width = chunk.chars().count() + 4;
            length = length.max(width);
            lines.push((chunk, width));
        }
    }

    let empty = format!("<error>{}</error>", " ".repeat(length));
    let mut messages = vec![empty.clone()];
    if !title.is_empty() {
        messages.push(format!(
            "<error>{title}{}</error>",
            " ".repeat(length.saturating_sub(title.len()))
        ));
    }
    for (line, width) in lines {
        messages.push(format!(
            "<error>  {}  {}</error>",
            OutputFormatter::escape(&line),
            " ".repeat(length - width)
        ));
    }
    messages.push(empty);
    messages.push(String::new());

    for message in messages {
        output.write_with(&message, true, Verbosity::Quiet);
    }

    if let Some(causes) = causes {
        for cause in causes {
            output.write_with(
                &format!(
                    "<comment>Caused by:</comment> {}",
                    OutputFormatter::escape(&cause)
                ),
                true,
                Verbosity::Quiet,
            );
        }
    }
}

fn append_alternatives(message: &mut String, alternatives: &[String]) {
    if alternatives.is_empty() {
        return;
    }

    message.push_str(if alternatives.len() == 1 {
        "\n\nDid you mean this?\n    "
    } else {
        "\n\nDid you mean one of these?\n    "
    });
    message.push_str(&alternatives.join("\n    "));
}

fn abbreviation_suggestions(abbreviations: &[String]) -> String {
    format!("    {}", abbreviations.join("\n    "))
}

/// PHP's `levenshtein()`.
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut previous: Vec<usize> = (0..=b.len()).collect();

    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        previous = current;
    }

    previous[b.len()]
}

/// Find alternatives for a mistyped command name, like Symfony does.
fn find_alternatives(name: &str, collection: &[String]) -> Vec<String> {
    const THRESHOLD: usize = 1000;
    let mut alternatives: IndexMap<String, usize> = IndexMap::new();

    let collection_parts: Vec<(String, Vec<&str>)> = collection
        .iter()
        .map(|item| (item.clone(), item.split(':').collect()))
        .collect();

    for (index, subname) in name.split(':').enumerate() {
        for (item, parts) in &collection_parts {
            let exists = alternatives.contains_key(item);

            let Some(part) = parts.get(index) else {
                if exists {
                    *alternatives.get_mut(item).unwrap() += THRESHOLD;
                }
                continue;
            };

            let distance = levenshtein(subname, part);
            if distance as f64 <= subname.len() as f64 / 3.0
                || (!subname.is_empty() && part.contains(subname))
            {
                *alternatives.entry(item.clone()).or_insert(0) += distance;
            } else if exists {
                *alternatives.get_mut(item).unwrap() += THRESHOLD;
            }
        }
    }

    for item in collection {
        let distance = levenshtein(name, item);
        if distance as f64 <= name.len() as f64 / 3.0 || item.contains(name) {
            match alternatives.get_mut(item) {
                Some(existing) => *existing = existing.saturating_sub(distance),
                None => {
                    alternatives.insert(item.clone(), distance);
                }
            }
        }
    }

    let mut alternatives: Vec<String> = alternatives
        .into_iter()
        .filter(|(_, distance)| *distance < 2 * THRESHOLD)
        .map(|(name, _)| name)
        .collect();

    alternatives.sort_by_key(|name| name.to_lowercase());
    alternatives.dedup();
    alternatives
}

/// Symfony's `ArgvInput::hasParameterOption()`.
pub(crate) fn has_parameter_option(tokens: &[String], values: &[&str]) -> bool {
    for token in tokens {
        if token == "--" {
            return false;
        }

        for value in values {
            let leading = if value.starts_with("--") {
                format!("{value}=")
            } else {
                value.to_string()
            };
            if token == value || (!leading.is_empty() && token.starts_with(&leading)) {
                return true;
            }
        }
    }

    false
}

fn parameter_option(tokens: &[String], name: &str) -> Option<String> {
    let mut iter = tokens.iter().peekable();
    while let Some(token) = iter.next() {
        if token == "--" {
            return None;
        }
        if token == name {
            return iter
                .peek()
                .filter(|next| !next.starts_with('-'))
                .map(|next| next.to_string());
        }
        if let Some(value) = token.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

fn first_argument(tokens: &[String]) -> Option<String> {
    let mut iter = tokens.iter();
    while let Some(token) = iter.next() {
        if token == "--" {
            return iter.next().cloned();
        }
        if token == "--env" {
            iter.next();
            continue;
        }
        if token.starts_with('-') {
            continue;
        }
        return Some(token.clone());
    }
    None
}

fn configure_io_from_tokens(tokens: &[String], output: &Output) {
    if has_parameter_option(tokens, &["--ansi"]) {
        output.set_decorated(true);
    } else if has_parameter_option(tokens, &["--no-ansi"]) {
        output.set_decorated(false);
    }

    if has_parameter_option(tokens, &["--no-interaction", "-n"]) {
        output.set_interactive(false);
    }

    if has_parameter_option(tokens, &["--silent"]) {
        output.set_verbosity(Verbosity::Silent);
        output.set_interactive(false);
    } else if has_parameter_option(tokens, &["--quiet", "-q"]) {
        output.set_verbosity(Verbosity::Quiet);
        output.set_interactive(false);
    } else {
        let level = parameter_option(tokens, "--verbose");
        if has_parameter_option(tokens, &["-vvv", "--verbose=3"]) || level.as_deref() == Some("3") {
            output.set_verbosity(Verbosity::Debug);
        } else if has_parameter_option(tokens, &["-vv", "--verbose=2"])
            || level.as_deref() == Some("2")
        {
            output.set_verbosity(Verbosity::VeryVerbose);
        } else if has_parameter_option(tokens, &["-v", "--verbose=1", "--verbose"]) {
            output.set_verbosity(Verbosity::Verbose);
        }
    }
}

fn configure_io_from_named(named: &[(String, ArgValue)], output: &Output) {
    let value = |keys: &[&str]| {
        named
            .iter()
            .find(|(key, _)| keys.contains(&key.as_str()))
            .map(|(_, value)| value.clone())
    };
    let truthy = |value: &ArgValue| match value {
        ArgValue::Null => true,
        ArgValue::Bool(value) => *value,
        ArgValue::String(value) => !value.is_empty() && value != "0",
        ArgValue::Array(values) => !values.is_empty(),
    };

    match value(&["--ansi"]) {
        Some(ArgValue::Bool(false)) => output.set_decorated(false),
        Some(ansi) if truthy(&ansi) => output.set_decorated(true),
        _ => {
            if value(&["--no-ansi"]).is_some_and(|v| truthy(&v)) {
                output.set_decorated(false);
            }
        }
    }

    if value(&["--no-interaction", "-n"]).is_some_and(|v| truthy(&v)) {
        output.set_interactive(false);
    }

    if value(&["--silent"]).is_some_and(|v| truthy(&v)) {
        output.set_verbosity(Verbosity::Silent);
        output.set_interactive(false);
    } else if value(&["--quiet", "-q"]).is_some_and(|v| truthy(&v)) {
        output.set_verbosity(Verbosity::Quiet);
        output.set_interactive(false);
    } else if let Some(verbose) = value(&["--verbose", "-v"]) {
        match verbose {
            ArgValue::String(level) if level == "3" => output.set_verbosity(Verbosity::Debug),
            ArgValue::String(level) if level == "2" => output.set_verbosity(Verbosity::VeryVerbose),
            other if truthy(&other) => output.set_verbosity(Verbosity::Verbose),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn levenshtein_matches_php() {
        assert_eq!(levenshtein("mak", "make"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn it_finds_alternatives() {
        let all = tokens(&["make:model", "make:migration", "migrate", "list", "help"]);
        assert_eq!(
            find_alternatives("mak:model", &all),
            vec!["make:migration", "make:model"]
        );
        assert_eq!(find_alternatives("lst", &all), vec!["list"]);
        assert!(find_alternatives("zzzzzz", &all).is_empty());
    }

    #[test]
    fn it_detects_parameter_options() {
        let t = tokens(&["migrate", "-vv", "--env=local", "--", "-q"]);
        assert!(has_parameter_option(&t, &["-v"]));
        assert!(has_parameter_option(&t, &["-vv"]));
        assert!(!has_parameter_option(&t, &["-vvv"]));
        assert!(has_parameter_option(&t, &["--env"]));
        assert!(!has_parameter_option(&t, &["-q"]));
    }

    #[test]
    fn it_finds_the_first_argument() {
        assert_eq!(
            first_argument(&tokens(&["-v", "migrate", "x"])).as_deref(),
            Some("migrate")
        );
        assert_eq!(
            first_argument(&tokens(&["--env", "local", "migrate"])).as_deref(),
            Some("migrate")
        );
        assert_eq!(first_argument(&tokens(&["--ansi"])), None);
    }

    #[test]
    fn it_configures_io() {
        let output = Output::buffered();
        configure_io_from_tokens(&tokens(&["x", "-vvv", "--ansi", "-n"]), &output);
        assert_eq!(output.verbosity(), Verbosity::Debug);
        assert!(output.is_decorated());
        assert!(!output.is_interactive());

        let output = Output::buffered();
        configure_io_from_tokens(&tokens(&["x", "--verbose=2"]), &output);
        assert_eq!(output.verbosity(), Verbosity::VeryVerbose);

        let output = Output::buffered();
        configure_io_from_tokens(&tokens(&["x", "-q"]), &output);
        assert_eq!(output.verbosity(), Verbosity::Quiet);

        let output = Output::buffered();
        configure_io_from_named(&[("--verbose".into(), ArgValue::Bool(true))], &output);
        assert_eq!(output.verbosity(), Verbosity::Verbose);
    }

    #[test]
    fn it_extracts_namespaces() {
        assert_eq!(Application::extract_namespace("make:model", None), "make");
        assert_eq!(Application::extract_namespace("a:b:c", Some(1)), "a");
        assert_eq!(Application::extract_namespace("a:b:c", None), "a:b");
        assert_eq!(Application::extract_namespace("list", None), "");
    }
}
