//! Small framework commands: inspire, env, about, config:show, cache:clear.

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Arr, Result, Value, ValueExt};

use crate::application::Application;
use crate::inspiring::Inspiring;

/// `inspire` — Display an inspiring quote.
pub struct InspireCommand;

#[async_trait]
impl Command for InspireCommand {
    fn signature(&self) -> &str {
        "inspire"
    }

    fn description(&self) -> &str {
        "Display an inspiring quote"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        cmd.line(Inspiring::format_for_console(
            &Inspiring::quote(),
            cmd.output().is_decorated(),
        ));
        Ok(())
    }
}

/// `env` — Display the current framework environment.
pub struct EnvironmentCommand;

#[async_trait]
impl Command for EnvironmentCommand {
    fn signature(&self) -> &str {
        "env"
    }

    fn description(&self) -> &str {
        "Display the current framework environment"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let environment = Application::current().environment();
        cmd.components()
            .info(format!("The application environment is [{environment}]."));
        Ok(())
    }
}

/// `about` — Display basic information about your application.
pub struct AboutCommand;

#[async_trait]
impl Command for AboutCommand {
    fn signature(&self) -> &str {
        "about {--only= : The section to display} {--json : Output the information as JSON}"
    }

    fn description(&self) -> &str {
        "Display basic information about your application"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let config = app.config_repository();
        let on_off = |value: bool| if value { "ENABLED" } else { "OFF" }.to_string();

        let sections: Vec<(&str, Vec<(&str, String)>)> = vec![
            (
                "Environment",
                vec![
                    ("Application Name", config.string("app.name")),
                    ("Laravel Version", app.version().to_string()),
                    ("Rust Version", rustc_version()),
                    ("Environment", app.environment()),
                    ("Debug Mode", on_off(app.has_debug_mode_enabled())),
                    ("URL", config.string("app.url").trim_start_matches("http://").trim_start_matches("https://").to_string()),
                    ("Maintenance Mode", on_off(app.is_down_for_maintenance())),
                    ("Timezone", config.string_or("app.timezone", "UTC")),
                    ("Locale", config.string_or("app.locale", "en")),
                ],
            ),
            (
                "Cache",
                vec![
                    // Configuration, listeners, and routes are compiled
                    // into the application.
                    ("Config", "COMPILED".to_string()),
                    ("Events", "COMPILED".to_string()),
                    ("Routes", "COMPILED".to_string()),
                    ("Views", "NOT CACHED".to_string()),
                ],
            ),
            (
                "Drivers",
                vec![
                    ("Broadcasting", config.string_or("broadcasting.default", "log")),
                    ("Cache", config.string("cache.default")),
                    ("Database", config.string("database.default")),
                    ("Logs", log_driver(&config.get("logging.default"), &config.get("logging.channels"))),
                    ("Mail", config.string("mail.default")),
                    ("Queue", config.string("queue.default")),
                    ("Session", config.string("session.driver")),
                    ("Storage", config.string("filesystems.default")),
                ],
            ),
        ];

        let only = cmd.option("only").map(|o| o.to_lowercase());
        let sections: Vec<_> = sections
            .into_iter()
            .filter(|(name, _)| only.as_ref().is_none_or(|only| only.split(',').any(|o| o.trim() == name.to_lowercase())))
            .collect();

        if cmd.option_bool("json") {
            let mut json = illuminate_support::Map::new();
            for (section, rows) in &sections {
                let mut map = illuminate_support::Map::new();
                for (key, value) in rows {
                    map.insert(illuminate_support::Str::snake(&key.replace(' ', "")), Value::String(value.clone()));
                }
                json.insert(section.to_lowercase(), Value::Object(map));
            }
            cmd.line(Value::Object(json).to_string());
            return Ok(());
        }

        cmd.new_line(1);
        for (section, rows) in sections {
            cmd.components()
                .two_column_detail(format!("<fg=green;options=bold>{section}</>"), "");
            for (key, value) in rows {
                let styled = match value.as_str() {
                    "ENABLED" => "<fg=yellow;options=bold>ENABLED</>".to_string(),
                    "OFF" => "OFF".to_string(),
                    "NOT CACHED" => "<fg=yellow;options=bold>NOT CACHED</>".to_string(),
                    "COMPILED" => "<fg=green;options=bold>COMPILED</>".to_string(),
                    _ => value,
                };
                cmd.components().two_column_detail(key, styled);
            }
            cmd.new_line(1);
        }
        Ok(())
    }
}

fn log_driver(default: &Value, channels: &Value) -> String {
    let default = default.to_string_lossy();
    let channel = channels.get(&default).cloned().unwrap_or(Value::Null);
    if channel.get("driver").and_then(Value::as_str) == Some("stack") {
        let stack = Arr::get(&channel, "channels");
        let names: Vec<String> = match stack {
            Value::Array(items) => items.iter().map(|v| v.to_string_lossy()).collect(),
            other => vec![other.to_string_lossy()],
        };
        return format!("<fg=yellow;options=bold>stack</> / {}", names.join(", "));
    }
    default
}

/// The version of the compiler the application was built with.
fn rustc_version() -> String {
    Some(env!("LARAVEL_RUSTC_VERSION"))
        .filter(|v| !v.is_empty())
        .or(option_env!("CARGO_PKG_RUST_VERSION"))
        .unwrap_or("stable")
        .to_string()
}

/// `config:show` — Display all of the values for a given configuration file or key.
pub struct ConfigShowCommand;

#[async_trait]
impl Command for ConfigShowCommand {
    fn signature(&self) -> &str {
        "config:show {config : The configuration file or key to show}"
    }

    fn description(&self) -> &str {
        "Display all of the values for a given configuration file or key"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let key = cmd.argument("config").unwrap_or_default();
        let config = Application::current().config_repository();
        if !config.has(&key) {
            return cmd.fail(format!("Configuration file or key <comment>{key}</comment> does not exist."));
        }
        let value = config.get(&key);
        cmd.new_line(1);
        match &value {
            Value::Object(_) => {
                cmd.components()
                    .two_column_detail(format!("<fg=green;options=bold>{key}</>"), "");
                if let Value::Object(flat) = Arr::dot(&value) {
                    for (k, v) in flat {
                        cmd.components().two_column_detail(k, format_value(&v));
                    }
                }
            }
            other => cmd.components().two_column_detail(format!("<fg=green;options=bold>{key}</>"), format_value(other)),
        }
        cmd.new_line(1);
        Ok(())
    }
}

fn format_value(value: &Value) -> String {
    match value {
        Value::Null => "<fg=red>null</>".to_string(),
        Value::Bool(true) => "<fg=green>true</>".to_string(),
        Value::Bool(false) => "<fg=red>false</>".to_string(),
        Value::Array(items) if items.is_empty() => "[]".to_string(),
        Value::Object(map) if map.is_empty() => "[]".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `cache:clear` — Flush the application cache.
pub struct CacheClearCommand;

#[async_trait]
impl Command for CacheClearCommand {
    fn signature(&self) -> &str {
        "cache:clear {store? : The name of the store you would like to clear}"
    }

    fn description(&self) -> &str {
        "Flush the application cache"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let manager = illuminate_cache::Cache::manager()?;
        let store = cmd.argument("store");
        let repository = manager.driver(store.as_deref())?;
        repository.flush().await?;
        cmd.components().info("Application cache cleared successfully.");
        Ok(())
    }
}
