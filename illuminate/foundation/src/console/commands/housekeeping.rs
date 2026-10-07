//! `storage:unlink`, `auth:clear-resets`, and `package:discover`.

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Result, Value};

use crate::application::Application;

/// `storage:unlink` — Delete the symbolic links configured for the
/// application.
pub struct StorageUnlinkCommand;

#[async_trait]
impl Command for StorageUnlinkCommand {
    fn signature(&self) -> &str {
        "storage:unlink"
    }

    fn description(&self) -> &str {
        "Delete existing symbolic links configured for the application"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let links: Vec<String> = match app.config_repository().get("filesystems.links") {
            Value::Object(map) if !map.is_empty() => map.keys().cloned().collect(),
            _ => vec![app.public_path("storage")],
        };

        for link in links {
            let path = std::path::Path::new(&link);
            if !path.is_symlink() {
                continue;
            }
            std::fs::remove_file(path)?;
            cmd.components().info(format!("The [{link}] link has been deleted."));
        }
        Ok(())
    }
}

/// `auth:clear-resets` — Flush expired password reset tokens.
pub struct ClearResetsCommand;

#[async_trait]
impl Command for ClearResetsCommand {
    fn signature(&self) -> &str {
        "auth:clear-resets {name? : The name of the password broker}"
    }

    fn description(&self) -> &str {
        "Flush expired password reset tokens"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let name = cmd.argument("name").filter(|name| !name.is_empty());
        illuminate_auth::facades::Password::broker(name.as_deref())?
            .get_repository()
            .delete_expired()
            .await?;
        cmd.components().info("Expired reset tokens cleared successfully.");
        Ok(())
    }
}

/// `package:discover` — List the packages whose service providers are
/// discovered (registered with `discover_provider!`).
pub struct PackageDiscoverCommand;

#[async_trait]
impl Command for PackageDiscoverCommand {
    fn signature(&self) -> &str {
        "package:discover"
    }

    fn description(&self) -> &str {
        "Rebuild the cached package manifest"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        cmd.components().info("Discovering packages");

        let mut packages: Vec<&str> = illuminate_container::discovered_providers()
            .iter()
            .map(|provider| provider.package)
            .collect();
        packages.dedup();
        for package in &packages {
            cmd.components().task(*package, || async { Ok(true) }).await?;
        }
        if !packages.is_empty() {
            cmd.new_line(1);
        }
        Ok(())
    }
}
