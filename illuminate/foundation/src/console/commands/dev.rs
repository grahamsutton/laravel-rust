//! `dev` and `dev:list`: run every process you need while developing your
//! application — the server, a queue listener, and Vite — side by side,
//! with prefixed and colored output.
//!
//! The framework registers its defaults; add your own (Laravel's
//! `DevCommands`) from a service provider's `boot` method:
//!
//! ```ignore
//! use laravel::foundation::console::commands::DevCommands;
//!
//! DevCommands::artisan("schedule:work").purple();
//! DevCommands::register_as("stripe listen --forward-to localhost:8000/stripe/webhook", "stripe");
//! DevCommands::except(["queue"]);
//! ```

use std::future::Future;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use illuminate_console::{Command, Console, Output, OutputFormatter, async_trait};
use illuminate_container::Container;
use illuminate_process::Process;
use illuminate_support::{Carbon, Result, Value, json};
use tokio::sync::{mpsc, watch};

use crate::application::Application;

/// Who registered a dev command, which decides which one wins when two
/// share a name — Laravel's `DevCommand::PRIORITY_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DevCommandPriority {
    /// One of the framework's defaults.
    Default = 0,
    /// Registered by a package.
    Vendor = 1,
    /// Registered by the application.
    Userland = 2,
}

/// The colors dev commands are shown in (Laravel's `DevCommandColor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevCommandColor {
    Blue,
    Purple,
    Pink,
    Orange,
    Green,
    Yellow,
}

impl DevCommandColor {
    /// Every color, in the order they are handed out.
    pub const ALL: [DevCommandColor; 6] = [
        DevCommandColor::Blue,
        DevCommandColor::Purple,
        DevCommandColor::Pink,
        DevCommandColor::Orange,
        DevCommandColor::Green,
        DevCommandColor::Yellow,
    ];

    /// The color's hex value.
    pub fn value(&self) -> &'static str {
        match self {
            DevCommandColor::Blue => "#93c5fd",
            DevCommandColor::Purple => "#c4b5fd",
            DevCommandColor::Pink => "#fb7185",
            DevCommandColor::Orange => "#fdba74",
            DevCommandColor::Green => "#86efac",
            DevCommandColor::Yellow => "#fcd34d",
        }
    }
}

/// How a dev command is started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevProcessCommand {
    /// An Artisan command (`"queue:listen --tries=1"`), run through the
    /// application's own binary.
    Artisan(String),
    /// A shell command line.
    Shell(String),
}

impl DevProcessCommand {
    /// The command as it is displayed (`artisan serve`, `npm run dev`).
    pub fn display(&self) -> String {
        match self {
            DevProcessCommand::Artisan(command) => format!("artisan {command}"),
            DevProcessCommand::Shell(command) => command.clone(),
        }
    }

    /// The process to run: Artisan commands run through `artisan` (the
    /// `ARTISAN_BINARY` environment variable, or the running executable).
    pub fn to_process_command(&self, artisan: &Path) -> illuminate_process::Command {
        match self {
            DevProcessCommand::Artisan(command) => {
                let mut argv = vec![artisan.to_string_lossy().into_owned()];
                argv.extend(split_arguments(command));
                illuminate_process::Command::Argv(argv)
            }
            DevProcessCommand::Shell(command) => illuminate_process::Command::Shell(command.clone()),
        }
    }
}

/// A process `artisan dev` runs — Laravel's `DevCommand`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevProcess {
    /// The process's name, shown in its output's prefix.
    pub name: String,
    /// What is run.
    pub command: DevProcessCommand,
    /// The prefix's color (assigned automatically when `None`).
    pub color: Option<String>,
    /// Where it was registered (`file:line`).
    pub source: String,
    /// Who registered it.
    pub priority: DevCommandPriority,
}

impl DevProcess {
    /// The name for a command: its first word (`npm run dev` => `npm`).
    pub fn name_from_command(command: &str) -> String {
        command.split_whitespace().next().unwrap_or(command).to_string()
    }

    /// The process as `dev:list --json` shows it.
    pub fn to_json(&self) -> Value {
        json!({
            "command": self.command.display(),
            "name": self.name,
            "color": self.color,
            "source": self.source,
            "priority": self.priority as u8,
        })
    }
}

/// The application's dev commands and how `artisan dev` runs them — the
/// state behind the [`DevCommands`] facade, kept in the container.
#[derive(Debug, Default)]
pub struct DevCommandRegistry {
    commands: RwLock<Vec<DevProcess>>,
    only: RwLock<Vec<String>>,
    except: RwLock<Vec<String>>,
    order: RwLock<Vec<String>>,
    with_timestamps: AtomicBool,
    without_auto_restart: AtomicBool,
    without_vendor_commands: AtomicBool,
    without_default_commands: AtomicBool,
}

impl DevCommandRegistry {
    /// Register a process, unless one with the same name and a higher
    /// priority is already registered.
    pub fn register(&self, process: DevProcess) {
        let mut commands = self.commands.write().unwrap();
        match commands.iter_mut().find(|existing| existing.name == process.name) {
            Some(existing) if process.priority >= existing.priority => *existing = process,
            Some(_) => {}
            None => commands.push(process),
        }
    }

    fn update(&self, name: &str, callback: impl FnOnce(&mut DevProcess)) {
        if let Some(process) = self.commands.write().unwrap().iter_mut().find(|process| process.name == name) {
            callback(process);
        }
    }

    /// The processes `dev` runs, with the framework's defaults for the
    /// given application, filtered, ordered, and colored.
    pub fn commands_for(&self, app: &Application) -> Vec<DevProcess> {
        let mut all: Vec<DevProcess> = Vec::new();
        if !self.without_default_commands.load(Ordering::SeqCst) {
            all.extend(default_commands(app));
        }
        for process in self.commands.read().unwrap().iter() {
            match all.iter_mut().find(|existing| existing.name == process.name) {
                Some(existing) if process.priority >= existing.priority => *existing = process.clone(),
                Some(_) => {}
                None => all.push(process.clone()),
            }
        }

        let only = self.only.read().unwrap().clone();
        let except = self.except.read().unwrap().clone();
        let without_vendor = self.without_vendor_commands.load(Ordering::SeqCst);
        all.retain(|process| {
            !(without_vendor && process.priority == DevCommandPriority::Vendor)
                && (only.is_empty() || only.contains(&process.name))
                && !except.contains(&process.name)
        });

        let order = self.order.read().unwrap().clone();
        if !order.is_empty() {
            all.sort_by_key(|process| order.iter().position(|name| *name == process.name).unwrap_or(usize::MAX));
        }

        fill_in_empty_colors(&mut all);
        all
    }

    /// Whether `dev` prefixes output with the time.
    pub fn should_include_timestamps(&self) -> bool {
        self.with_timestamps.load(Ordering::SeqCst)
    }

    /// Whether `dev` restarts crashed processes.
    pub fn should_auto_restart(&self) -> bool {
        !self.without_auto_restart.load(Ordering::SeqCst)
    }
}

/// Give every process without a color one, using each color once before
/// reusing any.
fn fill_in_empty_colors(processes: &mut [DevProcess]) {
    let mut count = 0;
    for index in 0..processes.len() {
        if processes[index].color.is_some() {
            continue;
        }
        let used: Vec<String> = processes.iter().filter_map(|process| process.color.clone()).collect();
        let color = DevCommandColor::ALL
            .iter()
            .map(DevCommandColor::value)
            .find(|color| !used.iter().any(|used| used == color))
            .unwrap_or_else(|| {
                let color = DevCommandColor::ALL[count % DevCommandColor::ALL.len()].value();
                count += 1;
                color
            });
        processes[index].color = Some(color.to_string());
    }
}

/// The framework's dev commands: the server, a queue listener, and the
/// `dev` script of your `package.json` (Vite).
#[track_caller]
fn default_commands(app: &Application) -> Vec<DevProcess> {
    let source = source(Location::caller());
    let mut defaults = vec![
        DevProcess {
            name: "server".into(),
            command: DevProcessCommand::Artisan("serve".into()),
            color: None,
            source: source.clone(),
            priority: DevCommandPriority::Default,
        },
        DevProcess {
            name: "queue".into(),
            command: DevProcessCommand::Artisan("queue:listen --tries=1 --timeout=0".into()),
            color: None,
            source: source.clone(),
            priority: DevCommandPriority::Default,
        },
    ];
    if has_node_script(&app.base_path("package.json"), "dev") {
        defaults.push(DevProcess {
            name: "vite".into(),
            command: DevProcessCommand::Shell(NodePackageManager::detect(Path::new(&app.base_path(""))).run_command("dev")),
            color: None,
            source,
            priority: DevCommandPriority::Default,
        });
    }
    defaults
}

/// Whether the `package.json` defines the given script.
fn has_node_script(package: &str, script: &str) -> bool {
    std::fs::read_to_string(package)
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
        .is_some_and(|package| package["scripts"][script].is_string())
}

/// The Node package manager a project uses, detected from its lock file
/// (Laravel's `NodePackageManager`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodePackageManager {
    Bun,
    Pnpm,
    Yarn,
    Npm,
}

impl NodePackageManager {
    /// Detect the package manager from the lock files in the directory.
    pub fn detect(base: &Path) -> Self {
        if base.join("bun.lock").exists() || base.join("bun.lockb").exists() {
            NodePackageManager::Bun
        } else if base.join("pnpm-lock.yaml").exists() {
            NodePackageManager::Pnpm
        } else if base.join("yarn.lock").exists() {
            NodePackageManager::Yarn
        } else {
            NodePackageManager::Npm
        }
    }

    /// The command running a `package.json` script (`npm run dev`).
    pub fn run_command(&self, script: &str) -> String {
        match self {
            NodePackageManager::Bun => format!("bun run {script}"),
            NodePackageManager::Pnpm => format!("pnpm run {script}"),
            NodePackageManager::Yarn => format!("yarn run {script}"),
            NodePackageManager::Npm => format!("npm run {script}"),
        }
    }

    /// The command running a package's binary (`npx stripe`).
    pub fn exec_command(&self, command: &str) -> String {
        match self {
            NodePackageManager::Bun => format!("bunx {command}"),
            NodePackageManager::Pnpm => format!("pnpm dlx {command}"),
            NodePackageManager::Yarn => format!("yarn dlx {command}"),
            NodePackageManager::Npm => format!("npx {command}"),
        }
    }
}

fn source(location: &Location<'_>) -> String {
    format!("{}:{}", location.file(), location.line())
}

/// Commands registered from a crate downloaded by Cargo belong to a
/// package; everything else belongs to the application.
fn priority_for(location: &Location<'_>) -> DevCommandPriority {
    let file = location.file().replace('\\', "/");
    if file.contains("/.cargo/registry/") || file.contains("/.cargo/git/") || file.contains("/vendor/") {
        DevCommandPriority::Vendor
    } else {
        DevCommandPriority::Userland
    }
}

fn registry() -> Arc<DevCommandRegistry> {
    let container = Container::get_instance();
    container.singleton_if::<DevCommandRegistry>(|_| Arc::new(DevCommandRegistry::default()));
    container.make::<DevCommandRegistry>()
}

fn base_path() -> PathBuf {
    Application::try_current()
        .map(|app| PathBuf::from(app.base_path("")))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

/// The processes `cargo artisan dev` runs — Laravel's `DevCommands`.
pub struct DevCommands;

impl DevCommands {
    /// The registry behind the facade.
    pub fn registry() -> Arc<DevCommandRegistry> {
        registry()
    }

    #[track_caller]
    fn add(command: DevProcessCommand, name: String) -> PendingDevCommand {
        let location = Location::caller();
        let registry = registry();
        registry.register(DevProcess {
            name: name.clone(),
            command,
            color: None,
            source: source(location),
            priority: priority_for(location),
        });
        PendingDevCommand { registry, name }
    }

    /// Register a shell command, named after its first word.
    #[track_caller]
    pub fn register(command: impl Into<String>) -> PendingDevCommand {
        let command = command.into();
        let name = DevProcess::name_from_command(&command);
        Self::add(DevProcessCommand::Shell(command), name)
    }

    /// Register a shell command with the given name.
    #[track_caller]
    pub fn register_as(command: impl Into<String>, name: impl Into<String>) -> PendingDevCommand {
        Self::add(DevProcessCommand::Shell(command.into()), name.into())
    }

    /// Register an Artisan command (`"horizon"`), named after the command.
    #[track_caller]
    pub fn artisan(command: impl Into<String>) -> PendingDevCommand {
        let command = command.into();
        let name = DevProcess::name_from_command(&command);
        Self::add(DevProcessCommand::Artisan(command), name)
    }

    /// Register an Artisan command with the given name.
    #[track_caller]
    pub fn artisan_as(command: impl Into<String>, name: impl Into<String>) -> PendingDevCommand {
        Self::add(DevProcessCommand::Artisan(command.into()), name.into())
    }

    /// Register a `package.json` script, run with the project's package
    /// manager (`npm run dev`).
    #[track_caller]
    pub fn node(script: impl Into<String>) -> PendingDevCommand {
        let script = script.into();
        let name = DevProcess::name_from_command(&script);
        Self::node_as(script, name)
    }

    /// Register a `package.json` script with the given name.
    #[track_caller]
    pub fn node_as(script: impl Into<String>, name: impl Into<String>) -> PendingDevCommand {
        let command = NodePackageManager::detect(&base_path()).run_command(&script.into());
        Self::add(DevProcessCommand::Shell(command), name.into())
    }

    /// Register a package's binary, run with the project's package manager
    /// (`npx stripe listen`).
    #[track_caller]
    pub fn node_exec(command: impl Into<String>) -> PendingDevCommand {
        let command = command.into();
        let name = DevProcess::name_from_command(&command);
        Self::node_exec_as(command, name)
    }

    /// Register a package's binary with the given name.
    #[track_caller]
    pub fn node_exec_as(command: impl Into<String>, name: impl Into<String>) -> PendingDevCommand {
        let command = NodePackageManager::detect(&base_path()).exec_command(&command.into());
        Self::add(DevProcessCommand::Shell(command), name.into())
    }

    /// The processes `dev` runs for the current application.
    pub fn commands() -> Vec<DevProcess> {
        let registry = registry();
        match Application::try_current() {
            Some(app) => registry.commands_for(&app),
            None => registry.commands.read().unwrap().clone(),
        }
    }

    /// Only run the processes with the given names.
    pub fn only<I, S>(names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        *registry().only.write().unwrap() = names.into_iter().map(Into::into).collect();
    }

    /// Don't run the processes with the given names.
    pub fn except<I, S>(names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        *registry().except.write().unwrap() = names.into_iter().map(Into::into).collect();
    }

    /// Run the named processes in this order (the rest come after them).
    pub fn order<I, S>(names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        *registry().order.write().unwrap() = names.into_iter().map(Into::into).collect();
    }

    /// Prefix every line of output with the time.
    pub fn with_timestamps() {
        registry().with_timestamps.store(true, Ordering::SeqCst);
    }

    /// Whether output is prefixed with the time.
    pub fn should_include_timestamps() -> bool {
        registry().should_include_timestamps()
    }

    /// Don't restart processes that crash.
    pub fn disable_auto_restart() {
        registry().without_auto_restart.store(true, Ordering::SeqCst);
    }

    /// Whether crashed processes are restarted.
    pub fn should_auto_restart() -> bool {
        registry().should_auto_restart()
    }

    /// Don't run the processes packages register.
    pub fn without_vendor_commands() {
        registry().without_vendor_commands.store(true, Ordering::SeqCst);
    }

    /// Don't run the framework's default processes.
    pub fn without_default_commands() {
        registry().without_default_commands.store(true, Ordering::SeqCst);
    }
}

/// A registered dev command, ready to be customized.
#[derive(Debug)]
pub struct PendingDevCommand {
    registry: Arc<DevCommandRegistry>,
    name: String,
}

impl PendingDevCommand {
    /// The command's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Show the command's output prefix in the given color (`"#93c5fd"`,
    /// `"blue"`).
    pub fn color(self, color: impl Into<String>) -> Self {
        let color = color.into();
        self.registry.update(&self.name, |process| process.color = Some(color));
        self
    }

    /// Show the command in blue.
    pub fn blue(self) -> Self {
        self.color(DevCommandColor::Blue.value())
    }

    /// Show the command in purple.
    pub fn purple(self) -> Self {
        self.color(DevCommandColor::Purple.value())
    }

    /// Show the command in pink.
    pub fn pink(self) -> Self {
        self.color(DevCommandColor::Pink.value())
    }

    /// Show the command in orange.
    pub fn orange(self) -> Self {
        self.color(DevCommandColor::Orange.value())
    }

    /// Show the command in green.
    pub fn green(self) -> Self {
        self.color(DevCommandColor::Green.value())
    }

    /// Show the command in yellow.
    pub fn yellow(self) -> Self {
        self.color(DevCommandColor::Yellow.value())
    }
}

/// Split an Artisan command line into arguments, honoring quotes.
fn split_arguments(command: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for character in command.chars() {
        match (quote, character) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(character);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !current.is_empty() {
                    arguments.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => current.push(c),
        }
    }
    if started || !current.is_empty() {
        arguments.push(current);
    }
    arguments
}

// ----------------------------------------------------------------------
// Running the processes
// ----------------------------------------------------------------------

/// Something that happened to one of the running processes.
enum DevEvent {
    Line(usize, String),
    Exited(usize, Option<i32>),
    Restarting(usize),
    Failed(usize, String),
}

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Succeeded,
    Failed,
    Stopped,
}

/// Runs dev processes side by side, prefixing each line of their output
/// with the process's colored name — like the `concurrently` tool Laravel's
/// `composer dev` script uses.
///
/// Crashed processes are restarted (up to five times, a second apart)
/// unless restarting is disabled, in which case one failing process stops
/// all of them. Every process is stopped when the `shutdown` future
/// completes (`Ctrl-C`).
#[derive(Clone, Debug)]
pub struct DevProcessRunner {
    processes: Vec<DevProcess>,
    artisan: PathBuf,
    base_path: Option<PathBuf>,
    restart: bool,
    restart_tries: usize,
    restart_after: Duration,
    timestamps: bool,
}

impl DevProcessRunner {
    /// Create a runner for the given processes.
    pub fn new(processes: Vec<DevProcess>) -> Self {
        let artisan = std::env::var("ARTISAN_BINARY")
            .map(PathBuf::from)
            .or_else(|_| std::env::current_exe())
            .unwrap_or_else(|_| PathBuf::from("artisan"));
        Self {
            processes,
            artisan,
            base_path: None,
            restart: true,
            restart_tries: 5,
            restart_after: Duration::from_secs(1),
            timestamps: false,
        }
    }

    /// Run Artisan commands through the given binary.
    pub fn artisan_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.artisan = binary.into();
        self
    }

    /// Run the processes in the given directory.
    pub fn working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.base_path = Some(path.into());
        self
    }

    /// Restart crashed processes (the default).
    pub fn restart(mut self, restart: bool) -> Self {
        self.restart = restart;
        self
    }

    /// How many times a process is restarted, and how long to wait first.
    pub fn restart_tries(mut self, tries: usize, after: Duration) -> Self {
        self.restart_tries = tries;
        self.restart_after = after;
        self
    }

    /// Prefix every line with the time.
    pub fn timestamps(mut self, timestamps: bool) -> Self {
        self.timestamps = timestamps;
        self
    }

    /// The prefix of a process's lines.
    fn prefix(&self, index: usize) -> String {
        let process = &self.processes[index];
        let color = process.color.as_deref().unwrap_or("default");
        let label = if self.timestamps {
            format!("{} [{}]", Carbon::now().format("H:i:s"), process.name)
        } else {
            format!("[{}]", process.name)
        };
        format!("<fg={color}>{}</>", OutputFormatter::escape(&label))
    }

    /// Run every process until they have all finished, one fails (when not
    /// restarting), or `shutdown` completes. Returns the exit code: `1`
    /// when a process failed for good, `0` otherwise.
    pub async fn run(self, output: &Output, shutdown: impl Future<Output = ()>) -> i32 {
        if self.processes.is_empty() {
            return 0;
        }
        let (events, mut receiver) = mpsc::unbounded_channel::<DevEvent>();
        let (stop, stopped) = watch::channel(false);
        // Processes write into the space left beside the `[name]` prefixes,
        // so they size their lines (`serve`'s dots, ...) to fit.
        let prefix_width = self
            .processes
            .iter()
            .map(|process| process.name.chars().count() + 3 + if self.timestamps { 9 } else { 0 })
            .max()
            .unwrap_or(0);
        let columns = output.width().saturating_sub(prefix_width).max(40);
        let mut tasks = tokio::task::JoinSet::new();
        for (index, process) in self.processes.iter().enumerate() {
            tasks.spawn(supervise(
                index,
                process.command.to_process_command(&self.artisan),
                self.base_path.clone(),
                output.is_decorated(),
                columns,
                if self.restart { self.restart_tries } else { 0 },
                self.restart_after,
                events.clone(),
                stopped.clone(),
            ));
        }
        drop(events);

        let mut shutdown = std::pin::pin!(shutdown);
        let mut interrupted = false;
        let mut failed = false;
        loop {
            tokio::select! {
                event = receiver.recv() => {
                    let Some(event) = event else { break };
                    match event {
                        DevEvent::Line(index, line) => {
                            output.writeln(format!("{} {}", self.prefix(index), OutputFormatter::escape(&line)));
                        }
                        DevEvent::Exited(index, code) => {
                            let code = code.map(|code| code.to_string()).unwrap_or_else(|| "SIGTERM".into());
                            output.writeln(format!(
                                "{} {} exited with code {code}",
                                self.prefix(index),
                                OutputFormatter::escape(&self.processes[index].command.display()),
                            ));
                        }
                        DevEvent::Restarting(index) => {
                            output.writeln(format!(
                                "{} {} restarted",
                                self.prefix(index),
                                OutputFormatter::escape(&self.processes[index].command.display()),
                            ));
                        }
                        DevEvent::Failed(index, error) => {
                            output.writeln(format!("{} {}", self.prefix(index), OutputFormatter::escape(&error)));
                        }
                    }
                }
                Some(result) = tasks.join_next() => {
                    if let Ok(Outcome::Failed) = result {
                        failed = true;
                        if !self.restart && !*stop.borrow() {
                            output.writeln("<fg=gray>--></> Sending SIGTERM to other processes..");
                            let _ = stop.send(true);
                        }
                    }
                }
                _ = &mut shutdown, if !interrupted => {
                    interrupted = true;
                    let _ = stop.send(true);
                }
            }
        }
        while let Some(result) = tasks.join_next().await {
            failed |= matches!(result, Ok(Outcome::Failed));
        }

        if failed && !interrupted { 1 } else { 0 }
    }
}

/// Run a process, restarting it when it crashes, until it finishes or is
/// told to stop.
#[allow(clippy::too_many_arguments)]
async fn supervise(
    index: usize,
    command: illuminate_process::Command,
    path: Option<PathBuf>,
    decorated: bool,
    columns: usize,
    restart_tries: usize,
    restart_after: Duration,
    events: mpsc::UnboundedSender<DevEvent>,
    mut stopped: watch::Receiver<bool>,
) -> Outcome {
    let mut tries = 0;
    loop {
        if *stopped.borrow() {
            return Outcome::Stopped;
        }
        let mut pending = Process::forever();
        if let Some(path) = &path {
            pending.path(path);
        }
        let mut environment = vec![("COLUMNS", columns.to_string())];
        if decorated {
            environment.push(("FORCE_COLOR", "1".to_string()));
        }
        pending.env(environment);
        let mut process = match pending.start(command.clone()) {
            Ok(process) => process,
            Err(error) => {
                let _ = events.send(DevEvent::Failed(index, error.to_string()));
                return Outcome::Failed;
            }
        };

        let lines = events.clone();
        let result = tokio::select! {
            result = process.wait_with_output(|_, line| {
                let line = line.trim_end_matches(['\n', '\r']);
                let _ = lines.send(DevEvent::Line(index, line.to_string()));
            }) => Some(result),
            _ = stopped.wait_for(|stopped| *stopped) => None,
        };

        let Some(result) = result else {
            process.stop_with(5, None).await;
            return Outcome::Stopped;
        };
        let code = match result {
            Ok(result) => result.exit_code(),
            Err(error) => {
                let _ = events.send(DevEvent::Failed(index, error.to_string()));
                None
            }
        };
        if *stopped.borrow() {
            return Outcome::Stopped;
        }
        let _ = events.send(DevEvent::Exited(index, code));
        if code == Some(0) {
            return Outcome::Succeeded;
        }
        if tries >= restart_tries {
            return Outcome::Failed;
        }
        tries += 1;
        tokio::select! {
            _ = tokio::time::sleep(restart_after) => {}
            _ = stopped.wait_for(|stopped| *stopped) => return Outcome::Stopped,
        }
        let _ = events.send(DevEvent::Restarting(index));
    }
}

// ----------------------------------------------------------------------
// The commands
// ----------------------------------------------------------------------

/// `dev` — Run the dev processes.
pub struct DevCommand;

#[async_trait]
impl Command for DevCommand {
    fn signature(&self) -> &str {
        "dev
            {--i|inline : Print output inline (the only mode this port supports)}
            {--timestamps : Display timestamps on each output line}
            {--no-restart : Disable auto-restart on crash}"
    }

    fn description(&self) -> &str {
        "Run the dev processes"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let registry = DevCommands::registry();
        let processes = registry.commands_for(&app);
        if processes.is_empty() {
            cmd.components().warn("Your application doesn't have any dev processes.");
            return Ok(());
        }

        let longest = processes.iter().map(|process| process.name.chars().count()).max().unwrap_or(0);
        for process in &processes {
            cmd.line(format!(
                "<fg={}>[{}]</>{}{}",
                process.color.as_deref().unwrap_or("default"),
                process.name,
                " ".repeat(longest - process.name.chars().count() + 1),
                OutputFormatter::escape(&process.command.display()),
            ));
        }
        cmd.line("");

        let runner = DevProcessRunner::new(processes)
            .working_directory(app.base_path(""))
            .restart(registry.should_auto_restart() && !cmd.option_bool("no-restart"))
            .timestamps(cmd.option_bool("timestamps") || registry.should_include_timestamps());
        let code = runner
            .run(cmd.output(), async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await;

        if code == 0 { Ok(()) } else { cmd.exit(code) }
    }
}

/// `dev:list` — List the registered dev processes.
pub struct DevListCommand;

impl DevListCommand {
    fn is_filtering(cmd: &Console) -> bool {
        cmd.option("filter").is_some_and(|filter| !filter.is_empty())
            || cmd.option_bool("except-vendor")
            || cmd.option_bool("only-vendor")
    }

    fn filter(cmd: &Console, mut processes: Vec<DevProcess>) -> Vec<DevProcess> {
        if let Some(filter) = cmd.option("filter").filter(|filter| !filter.is_empty()) {
            processes.retain(|process| process.name.contains(&filter) || process.command.display().contains(&filter));
        }
        if cmd.option_bool("except-vendor") {
            processes.retain(|process| process.priority != DevCommandPriority::Vendor);
        }
        if cmd.option_bool("only-vendor") {
            processes.retain(|process| process.priority == DevCommandPriority::Vendor);
        }
        processes
    }
}

#[async_trait]
impl Command for DevListCommand {
    fn signature(&self) -> &str {
        "dev:list
            {--json : Output the dev process list as JSON}
            {--filter= : Filter the dev processes by name or command}
            {--except-vendor : Do not display dev processes registered by vendor packages}
            {--only-vendor : Only display dev processes registered by vendor packages}"
    }

    fn description(&self) -> &str {
        "List the registered dev processes"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let processes = Self::filter(&cmd, DevCommands::registry().commands_for(&app));

        if cmd.option_bool("json") || cmd.option_bool("no-interaction") {
            let json: Vec<Value> = processes.iter().map(DevProcess::to_json).collect();
            cmd.output().writeln(OutputFormatter::escape(&Value::Array(json).to_string()));
            return if processes.is_empty() && Self::is_filtering(&cmd) { cmd.exit(1) } else { Ok(()) };
        }

        cmd.new_line(1);
        if processes.is_empty() {
            if Self::is_filtering(&cmd) {
                cmd.components()
                    .error("Your application doesn't have any dev processes matching the given criteria.");
                return cmd.exit(1);
            }
            cmd.components().warn("Your application doesn't have any dev processes.");
            return Ok(());
        }

        let longest = processes.iter().map(|process| process.name.chars().count()).max().unwrap_or(0);
        let columns = cmd.output().width();
        for process in &processes {
            let label = format!("{:<longest$}", process.name);
            let command = process.command.display();
            let mut source = format!(" {}", process.source);
            let text_width = label.chars().count() + command.chars().count() + source.chars().count();
            let buffer = 6;
            let dots = ".".repeat(columns.saturating_sub(text_width + buffer));
            if text_width + buffer > columns {
                let available = columns
                    .saturating_sub(label.chars().count() + command.chars().count() + dots.len() + buffer)
                    .saturating_sub(1);
                source = source.chars().take(available).collect::<String>() + "…";
            }
            cmd.line(format!(
                "  <fg={}>{label}</> {} <fg=#6C7280>{dots}{}</>",
                process.color.as_deref().unwrap_or("default"),
                OutputFormatter::escape(&command),
                OutputFormatter::escape(&source),
            ));
        }

        let count = format!(
            "Showing [{}] dev {} ",
            processes.len(),
            if processes.len() == 1 { "command" } else { "commands" }
        );
        cmd.new_line(1);
        cmd.line(format!(
            "{}<fg=blue;options=bold>{}</>",
            " ".repeat(columns.saturating_sub(count.chars().count() + 1)),
            OutputFormatter::escape(&count),
        ));
        cmd.new_line(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_split_like_a_shell_would() {
        assert_eq!(split_arguments("queue:listen --tries=1  --timeout=0"), ["queue:listen", "--tries=1", "--timeout=0"]);
        assert_eq!(split_arguments("greet 'Taylor Otwell' \"\""), ["greet", "Taylor Otwell", ""]);
    }

    #[test]
    fn node_package_managers_are_detected_from_lock_files() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(NodePackageManager::detect(dir.path()), NodePackageManager::Npm);
        assert_eq!(NodePackageManager::Npm.run_command("dev"), "npm run dev");
        std::fs::write(dir.path().join("yarn.lock"), "").unwrap();
        assert_eq!(NodePackageManager::detect(dir.path()).run_command("dev"), "yarn run dev");
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(NodePackageManager::detect(dir.path()).exec_command("vite"), "pnpm dlx vite");
        std::fs::write(dir.path().join("bun.lock"), "").unwrap();
        assert_eq!(NodePackageManager::detect(dir.path()), NodePackageManager::Bun);
    }

    #[test]
    fn colors_are_assigned_before_any_is_reused() {
        let process = |name: &str, color: Option<&str>| DevProcess {
            name: name.into(),
            command: DevProcessCommand::Shell(name.into()),
            color: color.map(str::to_string),
            source: String::new(),
            priority: DevCommandPriority::Userland,
        };
        let mut processes: Vec<DevProcess> = (0..7).map(|index| process(&index.to_string(), None)).collect();
        processes[1].color = Some(DevCommandColor::Blue.value().into());
        fill_in_empty_colors(&mut processes);
        let colors: Vec<&str> = processes.iter().map(|process| process.color.as_deref().unwrap()).collect();
        assert_eq!(colors[0], DevCommandColor::Purple.value());
        assert_eq!(colors[1], DevCommandColor::Blue.value());
        assert_eq!(colors[2], DevCommandColor::Pink.value());
        assert_eq!(colors[5], DevCommandColor::Yellow.value());
        assert_eq!(colors[6], DevCommandColor::Blue.value());
    }

    #[test]
    fn the_registry_keeps_the_highest_priority_command() {
        let registry = DevCommandRegistry::default();
        let process = |command: &str, priority| DevProcess {
            name: "queue".into(),
            command: DevProcessCommand::Artisan(command.into()),
            color: None,
            source: String::new(),
            priority,
        };
        registry.register(process("queue:work", DevCommandPriority::Userland));
        registry.register(process("queue:listen", DevCommandPriority::Vendor));
        assert_eq!(registry.commands.read().unwrap()[0].command, DevProcessCommand::Artisan("queue:work".into()));
        assert_eq!(DevProcess::name_from_command("npm run dev"), "npm");
    }
}
