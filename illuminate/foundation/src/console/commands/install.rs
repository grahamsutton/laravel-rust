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
            register_routes(&app.base_path("routes/mod.rs"))?;
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

/// Declare and export the channels module in `routes/mod.rs`.
fn register_routes(path: &str) -> Result<()> {
    let contents = std::fs::read_to_string(path).unwrap_or_default();
    if contents.contains("mod channels;") {
        return Ok(());
    }
    let mut lines: Vec<&str> = contents.lines().collect();
    let module = lines.iter().position(|line| line.starts_with("pub mod ")).unwrap_or(0);
    lines.insert(module, "pub mod channels;");
    let export = lines.iter().position(|line| line.starts_with("pub use ")).unwrap_or(lines.len());
    lines.insert(export, "pub use channels::channels;");
    std::fs::write(path, format!("{}\n", lines.join("\n")))?;
    Ok(())
}

/// Insert `.channels(routes::channels)` after the console routes.
fn add_channels_to_routing(contents: &str) -> Option<String> {
    let anchor = ".commands(routes::console)";
    let index = contents.find(anchor)? + anchor.len();
    let indent = contents[..contents.find(anchor)?]
        .rsplit('\n')
        .next()
        .map(|line| line.chars().take_while(|c| c.is_whitespace()).collect::<String>())
        .unwrap_or_default();
    Some(format!("{}\n{indent}.channels(routes::channels){}", &contents[..index], &contents[index..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_added_to_the_routing() {
        let bootstrap = "        .with_routing(|routing| {\n            routing\n                .web(routes::web)\n                .commands(routes::console)\n                .health(\"/up\");\n        })";
        let updated = add_channels_to_routing(bootstrap).unwrap();
        assert!(updated.contains(".commands(routes::console)\n                .channels(routes::channels)\n                .health(\"/up\");"));
    }
}
