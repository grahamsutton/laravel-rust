//! The bootstrappers that prepare an application to handle requests and
//! commands: environment, configuration, providers.

use illuminate_support::{Arr, Carbon, Env, Value};

use crate::application::Application;

/// A configuration "file": a name (`app`, `database`, ...) and the function
/// producing its values.
///
/// Configuration files are plain Rust functions so they can call `env()`
/// and the path helpers, exactly like Laravel's PHP config files.
#[derive(Clone, Copy)]
pub struct ConfigFile {
    pub name: &'static str,
    pub loader: fn() -> Value,
}

impl ConfigFile {
    pub const fn new(name: &'static str, loader: fn() -> Value) -> Self {
        Self { name, loader }
    }
}

impl std::fmt::Debug for ConfigFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigFile").field("name", &self.name).finish()
    }
}

/// Load the `.env` file (and `.env.{APP_ENV}` when present).
pub fn load_environment_variables(app: &Application) {
    let base = app.environment_file_path();
    // An environment specific file (e.g. `.env.testing`) takes precedence.
    if let Some(environment) = Env::raw("APP_ENV") {
        let specific = format!("{base}.{environment}");
        if std::path::Path::new(&specific).exists() {
            Env::load(&specific);
        }
    }
    Env::load(&base);
}

/// Options whose nested values are merged with the framework defaults
/// (rather than replaced), exactly like Laravel's `LoadConfiguration`.
fn mergeable_options(name: &str) -> &'static [&'static str] {
    match name {
        "auth" => &["guards", "providers", "passwords"],
        "broadcasting" => &["connections"],
        "cache" => &["stores"],
        "database" => &["connections"],
        "filesystems" => &["disks"],
        "logging" => &["channels"],
        "mail" => &["mailers"],
        "queue" => &["connections"],
        _ => &[],
    }
}

/// Merge an application configuration file over the framework's defaults.
pub fn merge_config(name: &str, base: Value, config: Value) -> Value {
    let (Value::Object(base), Value::Object(config)) = (base.clone(), config.clone()) else {
        return if config.is_null() { base } else { config };
    };
    let mut merged = base.clone();
    for (key, value) in config.iter() {
        merged.insert(key.clone(), value.clone());
    }
    for option in mergeable_options(name) {
        if let (Some(Value::Object(base_option)), Some(Value::Object(config_option))) =
            (base.get(*option), config.get(*option))
        {
            let mut combined = base_option.clone();
            for (key, value) in config_option {
                combined.insert(key.clone(), value.clone());
            }
            merged.insert(option.to_string(), Value::Object(combined));
        }
    }
    Value::Object(merged)
}

/// Load the framework defaults and the application's configuration files.
pub fn load_configuration(app: &Application) {
    let repository = app.config_repository();
    let application_files = app.config_files();

    // Framework defaults first, then the application's files merged on top.
    for default in crate::defaults::all() {
        let base = (default.loader)();
        let merged = match application_files.iter().find(|f| f.name == default.name) {
            Some(file) => merge_config(default.name, base, (file.loader)()),
            None => base,
        };
        repository.set(default.name, merged);
    }
    for file in &application_files {
        if !crate::defaults::all().iter().any(|d| d.name == file.name) {
            repository.set(file.name, (file.loader)());
        }
    }

    for (key, value) in app.config_overrides() {
        repository.set(&key, value);
    }

    let timezone = repository.string_or("app.timezone", "UTC");
    let _ = Carbon::set_default_timezone(&timezone);
}

/// Register the framework's and the application's service providers.
pub fn register_providers(app: &Application) {
    for provider in crate::providers::take_pending(app) {
        app.register_boxed(provider);
    }
}

/// Merge `values` into the configuration at `key`, keeping existing values.
pub fn merge_config_from(app: &Application, key: &str, values: Value) {
    let repository = app.config_repository();
    let mut current = repository.get(key);
    if current.is_null() {
        repository.set(key, values);
        return;
    }
    let mut defaults = values;
    Arr::merge_recursive(&mut defaults, std::mem::take(&mut current));
    repository.set(key, defaults);
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn it_merges_config_like_laravel() {
        let base = json!({"default": "sqlite", "connections": {"sqlite": {"driver": "sqlite"}, "mysql": {"driver": "mysql"}}});
        let app = json!({"default": "mysql", "connections": {"mysql": {"driver": "mysql", "host": "db"}}});
        let merged = merge_config("database", base, app);
        assert_eq!(merged["default"], json!("mysql"));
        assert_eq!(merged["connections"]["sqlite"]["driver"], json!("sqlite"));
        assert_eq!(merged["connections"]["mysql"]["host"], json!("db"));
    }
}
