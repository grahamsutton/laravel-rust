//! `reload` — reload the application's running services after a deploy.

use std::sync::{Arc, RwLock};

use illuminate_console::{Command, Console, async_trait};
use illuminate_container::Container;
use illuminate_support::Result;

/// The commands `reload` runs in addition to the framework's, registered by
/// packages — Laravel's `ServiceProvider::reloads()`.
///
/// ```ignore
/// ReloadCommands::reloads("horizon:terminate", "horizon");
/// ```
#[derive(Debug, Default)]
pub struct ReloadCommands {
    commands: RwLock<Vec<(String, String)>>,
}

impl ReloadCommands {
    fn resolve() -> Arc<ReloadCommands> {
        let container = Container::get_instance();
        container.singleton_if::<ReloadCommands>(|_| Arc::new(ReloadCommands::default()));
        container.make::<ReloadCommands>()
    }

    /// Run the given command when the application's services are
    /// reloaded, under the given key (shown by `reload`, and skipped with
    /// `--except=key`).
    pub fn reloads(command: impl Into<String>, key: impl Into<String>) {
        let key = key.into();
        let command = command.into();
        let registry = Self::resolve();
        let mut commands = registry.commands.write().unwrap();
        match commands.iter_mut().find(|(existing, _)| *existing == key) {
            Some(entry) => entry.1 = command,
            None => commands.push((key, command)),
        }
    }

    /// Every reload task: `queue:restart`, `schedule:interrupt`, and the
    /// registered commands, by key.
    pub fn tasks() -> Vec<(String, String)> {
        let mut tasks = vec![
            ("queue".to_string(), "queue:restart".to_string()),
            ("schedule".to_string(), "schedule:interrupt".to_string()),
        ];
        for (key, command) in Self::resolve().commands.read().unwrap().iter() {
            match tasks.iter_mut().find(|(existing, _)| existing == key) {
                Some(task) => task.1 = command.clone(),
                None => tasks.push((key.clone(), command.clone())),
            }
        }
        tasks
    }
}

/// `reload` — Reload running services: restart the queue workers
/// (`queue:restart`), interrupt the scheduler (`schedule:interrupt`), and
/// run every command packages registered with [`ReloadCommands::reloads`].
pub struct ReloadCommand;

#[async_trait]
impl Command for ReloadCommand {
    fn signature(&self) -> &str {
        "reload {--e|except= : The commands to skip}"
    }

    fn description(&self) -> &str {
        "Reload running services"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        cmd.components().info("Reloading services.");

        let exceptions: Vec<String> = cmd
            .option("except")
            .unwrap_or_default()
            .split(',')
            .map(|except| except.trim().to_string())
            .filter(|except| !except.is_empty())
            .collect();

        for (description, command) in ReloadCommands::tasks() {
            if exceptions.contains(&description) || exceptions.contains(&command) {
                continue;
            }
            cmd.components()
                .task(&description, || async { Ok(cmd.call_silently(&command, ()).await? == 0) })
                .await?;
        }

        cmd.new_line(1);
        Ok(())
    }
}
