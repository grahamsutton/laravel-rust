//! `view:cache`, `view:clear`, `optimize`, and `optimize:clear`.
//!
//! Configuration and routes are compiled into your application, so there is
//! nothing to cache for them; Blade templates are compiled when first
//! rendered, and `view:cache` compiles every one of them up front, failing
//! loudly on a template with a syntax error.

use std::path::{Path, PathBuf};

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::Result;
use illuminate_view::Factory;

use crate::application::Application;

/// Every Blade template under the given directory.
fn templates(dir: &Path, extensions: &[String], found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.is_dir() {
            templates(&path, extensions, found);
        } else {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if extensions.iter().any(|extension| extension.starts_with("blade") && name.ends_with(&format!(".{extension}"))) {
                found.push(path);
            }
        }
    }
}

/// `view:cache` — Compile all of the application's Blade templates.
pub struct ViewCacheCommand;

#[async_trait]
impl Command for ViewCacheCommand {
    fn signature(&self) -> &str {
        "view:cache"
    }

    fn description(&self) -> &str {
        "Compile all of the application's Blade templates"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let factory = Factory::resolve();
        let extensions = factory.finder().extensions();
        let mut found = Vec::new();
        for path in factory.finder().paths() {
            templates(&path, &extensions, &mut found);
        }
        found.sort();

        let app = Application::current();
        let mut failed = false;
        for path in &found {
            let source = std::fs::read_to_string(path)?;
            if let Err(error) = factory.blade().check_syntax(&source) {
                failed = true;
                let relative = path
                    .strip_prefix(app.base_path(""))
                    .unwrap_or(path)
                    .display()
                    .to_string();
                cmd.components().error(format!("[{relative}] {error}"));
            }
        }

        if failed {
            return cmd.exit(1);
        }
        cmd.components().info("Blade templates cached successfully.");
        Ok(())
    }
}

/// `view:clear` — Clear all compiled view files.
pub struct ViewClearCommand;

#[async_trait]
impl Command for ViewClearCommand {
    fn signature(&self) -> &str {
        "view:clear"
    }

    fn description(&self) -> &str {
        "Clear all compiled view files"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        Factory::resolve().blade().flush_cache();

        let app = Application::current();
        let compiled = app.config_repository().string("view.compiled");
        if !compiled.is_empty()
            && let Ok(entries) = std::fs::read_dir(&compiled)
        {
            for entry in entries.filter_map(|entry| entry.ok()) {
                let path = entry.path();
                if path.is_file() && path.file_name().is_some_and(|name| name != ".gitignore") {
                    std::fs::remove_file(path)?;
                }
            }
        }

        cmd.components().info("Compiled views cleared successfully.");
        Ok(())
    }
}

/// `optimize` — Cache framework bootstrap, configuration, and metadata to
/// increase performance.
pub struct OptimizeCommand;

#[async_trait]
impl Command for OptimizeCommand {
    fn signature(&self) -> &str {
        "optimize"
    }

    fn description(&self) -> &str {
        "Cache framework bootstrap, configuration, and metadata to increase performance"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        cmd.components()
            .info("Caching framework bootstrap, configuration, and metadata.");
        cmd.components()
            .task("views", || async { Ok(cmd.call_silently("view:cache", ()).await? == 0) })
            .await?;
        cmd.new_line(1);
        Ok(())
    }
}

/// `optimize:clear` — Remove the cached bootstrap files.
pub struct OptimizeClearCommand;

#[async_trait]
impl Command for OptimizeClearCommand {
    fn signature(&self) -> &str {
        "optimize:clear"
    }

    fn description(&self) -> &str {
        "Remove the cached bootstrap files"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        cmd.components().info("Clearing cached bootstrap files.");
        for (task, command) in [("cache", "cache:clear"), ("views", "view:clear")] {
            cmd.components()
                .task(task, || async { Ok(cmd.call_silently(command, ()).await? == 0) })
                .await?;
        }
        cmd.new_line(1);
        Ok(())
    }
}
