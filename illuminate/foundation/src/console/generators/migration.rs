//! `make:migration` — migrations are named by when they were created, so
//! they get a generator of their own.

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Carbon, Result, Str};

use super::{populate, relative, stubs};
use crate::application::Application;

/// `make:migration` — Create a new migration file.
pub struct MakeMigrationCommand;

#[async_trait]
impl Command for MakeMigrationCommand {
    fn signature(&self) -> &str {
        "make:migration {name : The name of the migration}
            {--create= : The table to be created}
            {--table= : The table to migrate}"
    }

    fn description(&self) -> &str {
        "Create a new migration file"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let name = Str::snake(cmd.argument("name").unwrap_or_default().trim());
        if name.is_empty() {
            return cmd.fail("The name of the migration is required.");
        }

        let mut table = cmd.option("table");
        let mut create = false;
        if table.is_none()
            && let Some(created) = cmd.option("create") {
                table = Some(created);
                create = true;
            }
        if table.is_none()
            && let Some((guessed, creating)) = TableGuesser::guess(&name) {
                table = Some(guessed);
                create = creating;
            }

        let stub = match (&table, create) {
            (None, _) => stubs::MIGRATION,
            (Some(_), true) => stubs::MIGRATION_CREATE,
            (Some(_), false) => stubs::MIGRATION_UPDATE,
        };
        let contents = populate(
            stub,
            &[("class", &Str::studly(&name)), ("table", table.as_deref().unwrap_or_default())],
        );

        let app = Application::current();
        let dir = std::path::PathBuf::from(app.database_path("migrations"));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}_{name}.rs", Carbon::now().format("Y_m_d_His")));
        std::fs::write(&path, contents)?;

        cmd.components()
            .info(format!("Migration [{}] created successfully.", relative(&app, &path)));
        Ok(())
    }
}

/// Guesses the table a migration touches from its name.
pub struct TableGuesser;

impl TableGuesser {
    /// `create_flights_table` => `("flights", true)`,
    /// `add_votes_to_users_table` => `("users", false)`.
    pub fn guess(migration: &str) -> Option<(String, bool)> {
        if let Some(rest) = migration.strip_prefix("create_") {
            let table = rest.strip_suffix("_table").unwrap_or(rest);
            if is_word(table) {
                return Some((table.to_string(), true));
            }
        }

        let position = ["_to_", "_from_", "_in_"]
            .iter()
            .filter_map(|marker| migration.rfind(marker).filter(|&i| i > 0).map(|i| i + marker.len()))
            .max()?;
        let rest = &migration[position..];
        let table = rest.strip_suffix("_table").unwrap_or(rest);
        is_word(table).then(|| (table.to_string(), false))
    }
}

fn is_word(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::TableGuesser;

    #[test]
    fn it_guesses_tables() {
        assert_eq!(TableGuesser::guess("create_users_table"), Some(("users".into(), true)));
        assert_eq!(TableGuesser::guess("create_users"), Some(("users".into(), true)));
        assert_eq!(TableGuesser::guess("add_votes_to_users_table"), Some(("users".into(), false)));
        assert_eq!(TableGuesser::guess("remove_votes_from_users"), Some(("users".into(), false)));
        assert_eq!(TableGuesser::guess("change_status_in_flights_table"), Some(("flights".into(), false)));
        assert_eq!(TableGuesser::guess("do_something"), None);
    }
}
