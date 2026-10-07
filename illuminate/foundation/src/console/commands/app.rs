//! `key:generate`, `down`, `up`, and `storage:link`.

use base64::Engine;
use rand::RngCore;

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Result, Str, Value, ValueExt, json};

use crate::application::Application;

/// `key:generate` — Set the application key.
pub struct KeyGenerateCommand;

impl KeyGenerateCommand {
    /// Generate a random key for the configured cipher.
    pub fn generate_random_key() -> String {
        let cipher = Application::try_current()
            .map(|app| app.config_repository().string_or("app.cipher", "AES-256-CBC"))
            .unwrap_or_else(|| "AES-256-CBC".into());
        let length = if cipher.to_uppercase().starts_with("AES-128") { 16 } else { 32 };
        let mut bytes = vec![0u8; length];
        rand::rng().fill_bytes(&mut bytes);
        format!("base64:{}", base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}

#[async_trait]
impl Command for KeyGenerateCommand {
    fn signature(&self) -> &str {
        "key:generate
            {--show : Display the key instead of modifying files}
            {--force : Force the operation to run when in production}"
    }

    fn description(&self) -> &str {
        "Set the application key"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let key = Self::generate_random_key();

        if cmd.option_bool("show") {
            cmd.line(format!("<comment>{key}</comment>"));
            return Ok(());
        }

        let app = Application::current();
        let current = app.config_repository().string("app.key");
        if !current.is_empty() && app.is_production() && !cmd.option_bool("force")
            && !cmd.confirm("Are you sure you want to run this command?", false) {
                cmd.components().warn("Command cancelled.");
                return Ok(());
            }

        let path = app.environment_file_path();
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(_) => {
                cmd.components().error("Unable to set application key. No APP_KEY variable was found in the .env file.");
                return cmd.exit(1);
            }
        };

        let mut found = false;
        let updated: Vec<String> = contents
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("APP_KEY=") {
                    found = true;
                    format!("APP_KEY={key}")
                } else {
                    line.to_string()
                }
            })
            .collect();
        if !found {
            cmd.components().error("Unable to set application key. No APP_KEY variable was found in the .env file.");
            return cmd.exit(1);
        }
        let mut updated = updated.join("\n");
        if contents.ends_with('\n') {
            updated.push('\n');
        }
        std::fs::write(&path, updated)?;
        app.config_repository().set("app.key", key);

        cmd.components().info("Application key set successfully.");
        Ok(())
    }
}

/// `down` — Put the application into maintenance / demo mode.
pub struct DownCommand;

#[async_trait]
impl Command for DownCommand {
    fn signature(&self) -> &str {
        "down
            {--redirect= : The path that users should be redirected to}
            {--render= : The view that should be prerendered for display during maintenance mode}
            {--retry= : The number of seconds after which the request may be retried}
            {--refresh= : The number of seconds after which the browser may refresh}
            {--secret= : The secret phrase that may be used to bypass maintenance mode}
            {--with-secret : Generate a random secret phrase that may be used to bypass maintenance mode}
            {--status=503 : The status code that should be used when returning the maintenance mode response}"
    }

    fn description(&self) -> &str {
        "Put the application into maintenance / demo mode"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        if app.is_down_for_maintenance() {
            cmd.components().info("Application is already down.");
            return Ok(());
        }

        let secret = if cmd.option_bool("with-secret") {
            Some(Str::random(32))
        } else {
            cmd.option("secret")
        };

        let numeric = |name: &str| -> Value {
            cmd.option(name)
                .and_then(|v| v.parse::<i64>().ok())
                .map(Value::from)
                .unwrap_or(Value::Null)
        };

        let payload = json!({
            "except": [],
            "redirect": cmd.option("redirect"),
            "retry": numeric("retry"),
            "refresh": numeric("refresh"),
            "secret": secret,
            "status": cmd.option("status").and_then(|s| s.parse::<u16>().ok()).unwrap_or(503),
            "template": cmd.option("render").map(|view| render_template(&view)).unwrap_or(Value::Null),
        });

        let file = app.maintenance_file();
        if let Some(parent) = std::path::Path::new(&file).parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file, serde_json::to_string_pretty(&payload)?)?;

        cmd.components().info("Application is now in maintenance mode.");
        if let Some(secret) = payload.get("secret").and_then(Value::as_str) {
            let url = app.config_repository().string("app.url");
            cmd.components().info(format!(
                "You may bypass maintenance mode via [{}/{secret}].",
                url.trim_end_matches('/')
            ));
        }
        Ok(())
    }
}

/// Prerender a maintenance view (a plain HTML file path, or a view hook).
fn render_template(view: &str) -> Value {
    if let Some(renderer) = crate::integration::view_renderer()
        && let Some(html) = renderer(view, json!({})) {
            return Value::String(html);
        }
    std::fs::read_to_string(view)
        .map(Value::String)
        .unwrap_or(Value::Null)
}

/// `up` — Bring the application out of maintenance mode.
pub struct UpCommand;

#[async_trait]
impl Command for UpCommand {
    fn signature(&self) -> &str {
        "up"
    }

    fn description(&self) -> &str {
        "Bring the application out of maintenance mode"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        if !app.is_down_for_maintenance() {
            cmd.components().info("Application is already up.");
            return Ok(());
        }
        std::fs::remove_file(app.maintenance_file())?;
        cmd.components().info("Application is now live.");
        Ok(())
    }
}

/// `storage:link` — Create the symbolic links configured for the application.
pub struct StorageLinkCommand;

#[async_trait]
impl Command for StorageLinkCommand {
    fn signature(&self) -> &str {
        "storage:link
            {--relative : Create the symbolic link using relative paths}
            {--force : Recreate existing symbolic links}"
    }

    fn description(&self) -> &str {
        "Create the symbolic links configured for the application"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let links = match app.config_repository().get("filesystems.links") {
            Value::Object(map) if !map.is_empty() => map,
            _ => {
                let mut map = illuminate_support::Map::new();
                map.insert(app.public_path("storage"), Value::String(app.storage_path("app/public")));
                map
            }
        };

        for (link, target) in links {
            let target = target.to_string_lossy();
            let link_path = std::path::Path::new(&link);
            let relative = |path: &str| path.strip_prefix(&app.base_path("")).map(|p| p.trim_start_matches('/').to_string()).unwrap_or_else(|| path.to_string());

            if link_path.exists() || link_path.is_symlink() {
                if cmd.option_bool("force") {
                    let _ = std::fs::remove_file(link_path);
                } else {
                    cmd.components().error(format!("The [{}] link already exists.", relative(&link)));
                    continue;
                }
            }

            std::fs::create_dir_all(&target)?;
            if let Some(parent) = link_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, link_path)?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(&target, link_path)?;

            cmd.components().info(format!(
                "The [{}] link has been connected to [{}].",
                relative(&link),
                relative(&target)
            ));
        }
        Ok(())
    }
}
