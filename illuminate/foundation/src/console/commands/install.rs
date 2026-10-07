//! `install:broadcasting` — set an application up for broadcasting.

use std::path::Path;

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::Result;

use crate::application::Application;

const CONFIG: &str = r#"use laravel::prelude::*;

pub fn config() -> Value {
    json!({
        /*
        |--------------------------------------------------------------------------
        | Default Broadcaster
        |--------------------------------------------------------------------------
        |
        | This option controls the default broadcaster that will be used by the
        | framework when an event needs to be broadcast. You may set this to
        | any of the connections defined in the "connections" array below.
        |
        | Supported: "reverb", "pusher", "ably", "log", "null"
        |
        */

        "default": env("BROADCAST_CONNECTION", "null"),

        /*
        |--------------------------------------------------------------------------
        | Broadcast Connections
        |--------------------------------------------------------------------------
        |
        | Here you may define all of the broadcast connections that will be used
        | to broadcast events to other systems or over WebSockets. Samples of
        | each available type of connection are provided inside this array.
        |
        */

        "connections": {
            "reverb": {
                "driver": "reverb",
                "key": env("REVERB_APP_KEY", Value::Null),
                "secret": env("REVERB_APP_SECRET", Value::Null),
                "app_id": env("REVERB_APP_ID", Value::Null),
                "options": {
                    "host": env("REVERB_HOST", Value::Null),
                    "port": env("REVERB_PORT", 443),
                    "scheme": env("REVERB_SCHEME", "https"),
                    "useTLS": env("REVERB_SCHEME", "https").to_string_lossy() == "https",
                },
                "client_options": {},
            },

            "pusher": {
                "driver": "pusher",
                "key": env("PUSHER_APP_KEY", Value::Null),
                "secret": env("PUSHER_APP_SECRET", Value::Null),
                "app_id": env("PUSHER_APP_ID", Value::Null),
                "options": {
                    "cluster": env("PUSHER_APP_CLUSTER", Value::Null),
                    "host": env("PUSHER_HOST", format!("api-{}.pusher.com", env("PUSHER_APP_CLUSTER", "mt1").to_string_lossy())),
                    "port": env("PUSHER_PORT", 443),
                    "scheme": env("PUSHER_SCHEME", "https"),
                    "encrypted": true,
                    "useTLS": env("PUSHER_SCHEME", "https").to_string_lossy() == "https",
                },
                "client_options": {},
            },

            "ably": {
                "driver": "ably",
                "key": env("ABLY_KEY", Value::Null),
            },

            "log": {
                "driver": "log",
            },

            "null": {
                "driver": "null",
            },
        },
    })
}
"#;

const CHANNELS: &str = r#"use laravel::prelude::*;
{{ import }}
/// The application's broadcast channels.
pub fn channels() {
    Broadcast::channel("App.Models.User.{id}", |user: {{ user }}, id: u64| async move {
        {{ check }}
    });
}
"#;

/// `install:broadcasting` — Create a broadcasting channel routes file.
pub struct InstallBroadcastingCommand;

#[async_trait]
impl Command for InstallBroadcastingCommand {
    fn signature(&self) -> &str {
        "install:broadcasting
            {--force : Overwrite any existing broadcasting routes file}"
    }

    fn description(&self) -> &str {
        "Create a broadcasting channel routes file"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let force = cmd.option_bool("force");

        // The configuration file.
        let config = app.config_path("broadcasting.rs");
        if force || !Path::new(&config).exists() {
            std::fs::write(&config, CONFIG)?;
            register_config(&app.config_path("mod.rs"))?;
            cmd.components().info("Published 'broadcasting' configuration file.");
        }

        // The channels file.
        let channels = app.base_path("routes/channels.rs");
        if force || !Path::new(&channels).exists() {
            let has_user = Path::new(&app.base_path("app/models/user.rs")).exists();
            let contents = if has_user {
                crate::console::generators::populate(
                    CHANNELS,
                    &[
                        ("import", "\nuse crate::app::models::User;\n"),
                        ("user", "User"),
                        ("check", "user.id == id"),
                    ],
                )
            } else {
                crate::console::generators::populate(
                    CHANNELS,
                    &[("import", ""), ("user", "AuthUser"), ("check", "user.id().to_string() == id.to_string()")],
                )
            };
            std::fs::write(&channels, contents)?;
            register_route_module(&app.base_path("routes/mod.rs"), "channels")?;
            cmd.components().info("Published 'channels' route file.");
        }

        // Load the channels from `bootstrap/app.rs`.
        let bootstrap = app.base_path("bootstrap/app.rs");
        if let Ok(contents) = std::fs::read_to_string(&bootstrap)
            && !contents.contains("routes::channels")
        {
            match add_channels_to_routing(&contents) {
                Some(updated) => std::fs::write(&bootstrap, updated)?,
                None => cmd.components().warn(
                    "Unable to automatically add channel route definitions to your application's bootstrap file. Please add `.channels(routes::channels)` to `with_routing`.",
                ),
            }
        }

        cmd.components().info(
            "Broadcasting is set up. Set BROADCAST_CONNECTION (and your Pusher, Reverb, or Ably credentials) in your .env file.",
        );
        Ok(())
    }
}

const API_ROUTES: &str = r#"use laravel::prelude::*;
{{ import }}
/// The application's API routes.
pub fn api() {
    Route::get("/user", |request: Request| async move {
        Json(request.user::<{{ user }}>())
    })
    .middleware("auth:sanctum");
}
"#;

const TOKENS_MIGRATION: &str = r#"use laravel::prelude::*;

pub struct CreatePersonalAccessTokensTable;

#[async_trait]
impl Migration for CreatePersonalAccessTokensTable {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("personal_access_tokens", |table| {
            table.id();
            table.morphs("tokenable");
            table.text("name");
            table.string_len("token", 64).unique();
            table.text("abilities").nullable();
            table.timestamp("last_used_at").nullable();
            table.timestamp("expires_at").nullable().index();
            table.timestamps();
        })
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("personal_access_tokens").await
    }
}
"#;

/// `install:api` — Create an API routes file and install Laravel Sanctum.
pub struct InstallApiCommand;

#[async_trait]
impl Command for InstallApiCommand {
    fn signature(&self) -> &str {
        "install:api
            {--force : Overwrite any existing API routes file}"
    }

    fn description(&self) -> &str {
        "Create an API routes file and install Laravel Sanctum"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let force = cmd.option_bool("force");

        // Sanctum is the `laravel` crate's `sanctum` feature.
        let manifest = app.base_path("Cargo.toml");
        if let Ok(contents) = std::fs::read_to_string(&manifest) {
            match enable_laravel_feature(&contents, "sanctum") {
                Some(updated) => {
                    if updated != contents {
                        std::fs::write(&manifest, updated)?;
                    }
                }
                None => cmd.components().warn(
                    "Unable to enable Laravel Sanctum. Please add features = [\"sanctum\"] to the laravel dependency in Cargo.toml.",
                ),
            }
        }

        // The API routes.
        let routes = app.base_path("routes/api.rs");
        if force || !Path::new(&routes).exists() {
            let has_user = Path::new(&app.base_path("app/models/user.rs")).exists();
            let (import, user) = if has_user {
                ("\nuse crate::app::models::User;\n", "User")
            } else {
                ("", "AuthUser")
            };
            std::fs::create_dir_all(app.base_path("routes"))?;
            std::fs::write(&routes, crate::console::generators::populate(API_ROUTES, &[("import", import), ("user", user)]))?;
            register_route_module(&app.base_path("routes/mod.rs"), "api")?;
            cmd.components().info("Published API routes file.");
        }

        let bootstrap = app.base_path("bootstrap/app.rs");
        if let Ok(contents) = std::fs::read_to_string(&bootstrap)
            && !contents.contains("routes::api")
        {
            match add_to_routing(&contents, ".web(routes::web)", ".api(routes::api)") {
                Some(updated) => std::fs::write(&bootstrap, updated)?,
                None => cmd.components().warn(
                    "Unable to automatically add API route definitions to your application's bootstrap file. Please add `.api(routes::api)` to `with_routing`.",
                ),
            }
        }

        // The personal access tokens migration.
        let migrations = std::path::PathBuf::from(app.database_path("migrations"));
        let exists = std::fs::read_dir(&migrations).is_ok_and(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.file_name().to_string_lossy().ends_with("_create_personal_access_tokens_table.rs"))
        });
        if !exists {
            std::fs::create_dir_all(&migrations)?;
            let name = format!("{}_create_personal_access_tokens_table.rs", illuminate_support::Carbon::now().format("Y_m_d_His"));
            std::fs::write(migrations.join(name), TOKENS_MIGRATION)?;
            cmd.components().info("Published the personal access tokens migration.");
        }

        cmd.components().info(
            "API scaffolding installed. Run `cargo artisan migrate` to create the personal_access_tokens table, and use laravel::sanctum::HasApiTokens to issue tokens.",
        );
        Ok(())
    }
}

/// Add `broadcasting` to the `config_files!` list in `config/mod.rs`.
fn register_config(path: &str) -> Result<()> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Ok(());
    };
    let Some(start) = contents.find("config_files![") else {
        return Ok(());
    };
    let open = start + "config_files![".len();
    let Some(close) = contents[open..].find(']').map(|offset| open + offset) else {
        return Ok(());
    };
    let mut names: Vec<String> = contents[open..close]
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    if names.iter().any(|name| name == "broadcasting") {
        return Ok(());
    }
    names.push("broadcasting".to_string());
    names.sort();
    let updated = format!("{}{}{}", &contents[..open], names.join(", "), &contents[close..]);
    std::fs::write(path, updated)?;
    Ok(())
}

/// Declare and export a routes module in `routes/mod.rs`.
fn register_route_module(path: &str, module: &str) -> Result<()> {
    let contents = std::fs::read_to_string(path).unwrap_or_default();
    if contents.contains(&format!("mod {module};")) {
        return Ok(());
    }
    let declaration = format!("pub mod {module};");
    let export = format!("pub use {module}::{module};");
    let mut lines: Vec<&str> = contents.lines().collect();
    let at = lines.iter().position(|line| line.starts_with("pub mod ")).unwrap_or(0);
    lines.insert(at, &declaration);
    let at = lines.iter().position(|line| line.starts_with("pub use ")).unwrap_or(lines.len());
    lines.insert(at, &export);
    std::fs::write(path, format!("{}\n", lines.join("\n")))?;
    Ok(())
}

/// Insert a routing call (`.channels(routes::channels)`) after another one
/// in `bootstrap/app.rs`.
fn add_to_routing(contents: &str, after: &str, call: &str) -> Option<String> {
    let start = contents.find(after)?;
    let index = start + after.len();
    let indent = contents[..start]
        .rsplit('\n')
        .next()
        .map(|line| line.chars().take_while(|c| c.is_whitespace()).collect::<String>())
        .unwrap_or_default();
    Some(format!("{}\n{indent}{call}{}", &contents[..index], &contents[index..]))
}

fn add_channels_to_routing(contents: &str) -> Option<String> {
    add_to_routing(contents, ".commands(routes::console)", ".channels(routes::channels)")
}

/// Turn on a feature of the `laravel` dependency in `Cargo.toml`.
fn enable_laravel_feature(manifest: &str, feature: &str) -> Option<String> {
    let mut changed = false;
    let lines: Vec<String> = manifest
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if changed || !(trimmed.starts_with("laravel ") || trimmed.starts_with("laravel=") || trimmed.starts_with("laravel.")) {
                return line.to_string();
            }
            let quoted = format!("\"{feature}\"");
            if trimmed.contains(&quoted) {
                changed = true;
                return line.to_string();
            }
            changed = true;
            if trimmed == "laravel.workspace = true" {
                return format!("laravel = {{ workspace = true, features = [{quoted}] }}");
            }
            let value = trimmed.split_once('=').map(|(_, value)| value.trim()).unwrap_or_default();
            if value.starts_with('"') {
                return format!("laravel = {{ version = {value}, features = [{quoted}] }}");
            }
            if let Some(start) = value.find("features = [") {
                let at = start + "features = [".len();
                let separator = if value[at..].starts_with(']') { "" } else { ", " };
                return format!("laravel = {}{quoted}{separator}{}", &value[..at], &value[at..]);
            }
            if let Some(body) = value.strip_suffix('}') {
                return format!("laravel = {}, features = [{quoted}] }}", body.trim_end());
            }
            changed = false;
            line.to_string()
        })
        .collect();
    changed.then(|| format!("{}\n", lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sanctum_feature_is_enabled_in_any_manifest_style() {
        let enable = |line: &str| enable_laravel_feature(&format!("[dependencies]\n{line}\n"), "sanctum").unwrap();
        assert!(enable("laravel.workspace = true").contains(r#"laravel = { workspace = true, features = ["sanctum"] }"#));
        assert!(enable(r#"laravel = "0.1""#).contains(r#"laravel = { version = "0.1", features = ["sanctum"] }"#));
        assert!(enable(r#"laravel = { version = "0.1" }"#).contains(r#"laravel = { version = "0.1", features = ["sanctum"] }"#));
        assert!(enable(r#"laravel = { version = "0.1", features = ["x"] }"#).contains(r#"features = ["sanctum", "x"]"#));
        assert!(enable(r#"laravel = { version = "0.1", features = ["sanctum"] }"#).contains(r#"features = ["sanctum"] }"#));
        assert!(enable_laravel_feature("[dependencies]\nlaravel-build.workspace = true\n", "sanctum").is_none());
    }

    #[test]
    fn channels_are_added_to_the_routing() {
        let bootstrap = "        .with_routing(|routing| {\n            routing\n                .web(routes::web)\n                .commands(routes::console)\n                .health(\"/up\");\n        })";
        let updated = add_channels_to_routing(bootstrap).unwrap();
        assert!(updated.contains(".commands(routes::console)\n                .channels(routes::channels)\n                .health(\"/up\");"));
    }
}
