//! Publishing files into the application: `config:publish`,
//! `vendor:publish`, `lang:publish`, and `stub:publish`.

use std::path::{Path, PathBuf};

use illuminate_console::{Command, Console, OutputFormatter, async_trait, prompts};
use illuminate_container::{PublishRegistry, PublishRoot, PublishSource, Publishable};
use illuminate_events::Event;
use illuminate_support::{Carbon, Result};

use super::install::register_config;
use crate::application::Application;
use crate::console::generators::stubs;

/// The framework's configuration files, as `config:publish` writes them to
/// `config/{name}.rs`. Every option matches the framework's defaults.
pub const CONFIG_FILES: &[(&str, &str)] = &[
    ("app", include_str!("../stubs/config/app.stub")),
    ("auth", include_str!("../stubs/config/auth.stub")),
    ("broadcasting", include_str!("../stubs/config/broadcasting.stub")),
    ("cache", include_str!("../stubs/config/cache.stub")),
    ("concurrency", include_str!("../stubs/config/concurrency.stub")),
    ("cors", include_str!("../stubs/config/cors.stub")),
    ("database", include_str!("../stubs/config/database.stub")),
    ("filesystems", include_str!("../stubs/config/filesystems.stub")),
    ("hashing", include_str!("../stubs/config/hashing.stub")),
    ("images", include_str!("../stubs/config/images.stub")),
    ("logging", include_str!("../stubs/config/logging.stub")),
    ("mail", include_str!("../stubs/config/mail.stub")),
    ("queue", include_str!("../stubs/config/queue.stub")),
    ("services", include_str!("../stubs/config/services.stub")),
    ("session", include_str!("../stubs/config/session.stub")),
    ("view", include_str!("../stubs/config/view.stub")),
];

/// The framework's copy of a configuration file (`"cors"`).
pub fn config_stub(name: &str) -> Option<&'static str> {
    CONFIG_FILES.iter().find(|(file, _)| *file == name).map(|(_, contents)| *contents)
}

/// Whether a file should be written, given `--existing` and `--force`
/// (Laravel's rule for every publishing command).
fn should_write(cmd: &Console, exists: bool) -> bool {
    if cmd.option_bool("existing") {
        exists
    } else {
        !exists || cmd.option_bool("force")
    }
}

/// A path relative to the application's base path, for display.
fn display(app: &Application, path: &Path) -> String {
    crate::console::generators::relative(app, path)
}

// ----------------------------------------------------------------------
// config:publish
// ----------------------------------------------------------------------

/// `config:publish` — Publish configuration files to your application.
///
/// Configuration files are optional: every option has a default in the
/// framework. Publishing one writes `config/{name}.rs` with Laravel's
/// documented options and adds it to `config_files!` in `config/mod.rs`.
pub struct ConfigPublishCommand;

impl ConfigPublishCommand {
    fn publish(cmd: &Console, app: &Application, name: &str, contents: &str) -> Result<()> {
        let destination = PathBuf::from(app.config_path(&format!("{name}.rs")));
        if destination.exists() && !cmd.option_bool("force") {
            cmd.components()
                .error(format!("The '{name}' configuration file already exists."));
            return Ok(());
        }
        std::fs::create_dir_all(app.config_path(""))?;
        std::fs::write(&destination, contents)?;
        register_config(&app.config_path("mod.rs"), name)?;
        cmd.components().info(format!("Published '{name}' configuration file."));
        Ok(())
    }
}

#[async_trait]
impl Command for ConfigPublishCommand {
    fn signature(&self) -> &str {
        "config:publish
            {name? : The name of the configuration file to publish}
            {--all : Publish all configuration files}
            {--force : Overwrite any existing configuration files}"
    }

    fn description(&self) -> &str {
        "Publish configuration files to your application"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let name = cmd.argument("name").filter(|name| !name.is_empty());

        if name.is_none() && cmd.option_bool("all") {
            for (name, contents) in CONFIG_FILES {
                Self::publish(&cmd, &app, name, contents)?;
            }
            return Ok(());
        }

        let name = match name {
            Some(name) => name,
            None => prompts::select(
                "Which configuration file would you like to publish?",
                CONFIG_FILES.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            )
            .scroll(15)
            .prompt_on(cmd.output())?,
        };
        let name = name.trim_end_matches(".rs");

        match config_stub(name) {
            Some(contents) => Self::publish(&cmd, &app, name, contents),
            None => {
                cmd.components().error("Unrecognized configuration file.");
                cmd.exit(1)
            }
        }
    }
}

// ----------------------------------------------------------------------
// vendor:publish
// ----------------------------------------------------------------------

/// The files of a tag were published — Laravel's `VendorTagPublished`.
#[derive(Clone, Debug, PartialEq)]
pub struct VendorTagPublished {
    /// The tag (`None` when publishing everything, or by provider).
    pub tag: Option<String>,
    /// What was published.
    pub paths: Vec<Publishable>,
}

/// `vendor:publish` — Publish any publishable assets from vendor packages.
///
/// Packages register what they publish from their service providers with
/// [`ServiceProvider::publishes`](illuminate_container::ServiceProvider::publishes):
///
/// ```text
/// cargo artisan vendor:publish --tag=sanctum-config
/// cargo artisan vendor:publish --provider=SanctumServiceProvider
/// ```
pub struct VendorPublishCommand;

/// The state of one `vendor:publish` run.
struct Publisher<'a> {
    cmd: &'a Console,
    app: std::sync::Arc<Application>,
    provider: Option<String>,
    published_at: Carbon,
}

impl Publisher<'_> {
    /// Ask which provider or tag to publish.
    fn prompt(&mut self, registry: &PublishRegistry) -> Result<Vec<String>> {
        let mut choices = vec!["All providers and tags".to_string()];
        choices.extend(
            registry
                .publishable_providers()
                .into_iter()
                .map(|provider| format!("<fg=gray>Provider:</> {provider}")),
        );
        choices.extend(registry.publishable_groups().into_iter().map(|tag| format!("<fg=gray>Tag:</> {tag}")));

        let choice = prompts::select("Which provider or tag's files would you like to publish?", choices.clone())
            .scroll(15)
            .prompt_on(self.cmd.output())?;
        if choice == choices[0] {
            return Ok(Vec::new());
        }
        let stripped = OutputFormatter::strip(&choice);
        match stripped.split_once(": ") {
            Some(("Provider", provider)) => {
                self.provider = Some(provider.to_string());
                Ok(Vec::new())
            }
            Some(("Tag", tag)) => Ok(vec![tag.to_string()]),
            _ => Ok(Vec::new()),
        }
    }

    /// Publish everything registered for a tag (or for the provider, or
    /// everything at all, when there is no tag).
    async fn publish_tag(&mut self, registry: &PublishRegistry, tag: Option<&str>) -> Result<()> {
        let paths = registry.paths_to_publish(self.provider.as_deref(), tag);

        if paths.is_empty() {
            self.cmd.components().info(format!(
                "No publishable resources for tag [{}].",
                tag.unwrap_or_default()
            ));
            return Ok(());
        }

        self.cmd.components().info(format!(
            "Publishing {}assets",
            tag.map(|tag| format!("[{tag}] ")).unwrap_or_default()
        ));
        for publishable in &paths {
            self.publish_item(publishable).await?;
        }

        Event::dispatch(VendorTagPublished {
            tag: tag.map(str::to_string),
            paths,
        })
        .await?;
        self.cmd.new_line(1);
        Ok(())
    }

    /// Where a publishable goes in this application.
    fn destination(&self, publishable: &Publishable) -> PathBuf {
        let path = publishable.path.trim_start_matches('/');
        PathBuf::from(match publishable.root {
            PublishRoot::Base => self.app.base_path(path),
            PublishRoot::Config => self.app.config_path(path),
            PublishRoot::Database => self.app.database_path(path),
            PublishRoot::Lang => self.app.lang_path(path),
            PublishRoot::Public => self.app.public_path(path),
            PublishRoot::Resources => self.app.resource_path(path),
            PublishRoot::Storage => self.app.storage_path(path),
        })
    }

    async fn publish_item(&mut self, publishable: &Publishable) -> Result<()> {
        let to = self.destination(publishable);
        match &publishable.source {
            PublishSource::Contents(contents) => {
                let from = Path::new(&publishable.path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| publishable.path.clone());
                self.publish_file(publishable, &from, contents.as_bytes(), to).await
            }
            PublishSource::Path(from) if from.is_file() => {
                let contents = std::fs::read(from)?;
                self.publish_file(publishable, &from.to_string_lossy(), &contents, to).await
            }
            PublishSource::Path(from) if from.is_dir() => self.publish_directory(publishable, from, &to).await,
            PublishSource::Path(from) => {
                self.cmd
                    .components()
                    .error(format!("Can't locate path: <{}>", from.display()));
                Ok(())
            }
        }
    }

    async fn publish_file(&mut self, publishable: &Publishable, from: &str, contents: &[u8], to: PathBuf) -> Result<()> {
        let to = if publishable.migration {
            self.existing_migration(&to).unwrap_or(to)
        } else {
            to
        };
        let exists = to.exists();

        if !should_write(self.cmd, exists) {
            let message = if self.cmd.option_bool("existing") {
                format!("File [{}] does not exist", display(&self.app, &to))
            } else {
                format!("File [{}] already exists", display(&self.app, &to))
            };
            self.cmd
                .components()
                .two_column_detail(message, "<fg=yellow;options=bold>SKIPPED</>");
            return Ok(());
        }

        let to = if publishable.migration && !exists {
            self.ensure_migration_name_is_up_to_date(to)
        } else {
            to
        };
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&to, contents)?;
        if publishable.is_config_file()
            && let Some(name) = to.file_stem()
        {
            register_config(&self.app.config_path("mod.rs"), &name.to_string_lossy())?;
        }

        self.status(from, &to, "file").await
    }

    async fn publish_directory(&mut self, publishable: &Publishable, from: &Path, to: &Path) -> Result<()> {
        let mut files = Vec::new();
        collect_files(from, &mut files)?;
        files.sort();
        for file in files {
            let relative = file.strip_prefix(from).unwrap_or(&file);
            let mut target = to.join(relative);
            let exists = target.exists();
            if !should_write(self.cmd, exists) {
                continue;
            }
            if publishable.migration && !exists {
                target = self.ensure_migration_name_is_up_to_date(target);
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&file, &target)?;
        }
        self.status(&from.to_string_lossy(), to, "directory").await
    }

    /// A migration that was published before, under an earlier date: the
    /// same name after its timestamp, in the same directory.
    fn existing_migration(&self, to: &Path) -> Option<PathBuf> {
        let name = to.file_name()?.to_string_lossy().into_owned();
        let suffix = strip_migration_date(&name)?;
        std::fs::read_dir(to.parent()?)
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| strip_migration_date(&name.to_string_lossy()).map(|rest| rest == suffix))
                    .unwrap_or(false)
            })
    }

    /// Give a newly published migration the current date, so it runs after
    /// the application's own (when `database.migrations.update_date_on_publish`
    /// is on, as it is by default).
    fn ensure_migration_name_is_up_to_date(&mut self, to: PathBuf) -> PathBuf {
        let update = self
            .app
            .config_repository()
            .get("database.migrations.update_date_on_publish");
        if update == illuminate_support::Value::Bool(false) {
            return to;
        }
        let Some(name) = to.file_name().map(|name| name.to_string_lossy().into_owned()) else {
            return to;
        };
        let Some(rest) = strip_migration_date(&name) else {
            return to;
        };
        self.published_at = self.published_at.add_second();
        to.with_file_name(format!("{}_{rest}", self.published_at.format("Y_m_d_His")))
    }

    async fn status(&self, from: &str, to: &Path, kind: &str) -> Result<()> {
        let to = display(&self.app, to);
        let from = from
            .strip_prefix(&format!("{}/", self.app.base_path("")))
            .unwrap_or(from)
            .to_string();
        self.cmd
            .components()
            .task(format!("Copying {kind} [{from}] to [{to}]"), || async { Ok(true) })
            .await?;
        Ok(())
    }
}

/// `2019_12_14_000001_create_personal_access_tokens_table.rs` =>
/// `create_personal_access_tokens_table.rs`.
fn strip_migration_date(name: &str) -> Option<&str> {
    let bytes = name.as_bytes();
    let pattern = "dddd_dd_dd_dddddd_";
    if bytes.len() <= pattern.len() {
        return None;
    }
    let matches = pattern
        .bytes()
        .zip(bytes)
        .all(|(expected, actual)| if expected == b'd' { actual.is_ascii_digit() } else { expected == *actual });
    matches.then(|| &name[pattern.len()..])
}

fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

#[async_trait]
impl Command for VendorPublishCommand {
    fn signature(&self) -> &str {
        "vendor:publish
            {--existing : Publish and overwrite only the files that have already been published}
            {--force : Overwrite any existing files}
            {--all : Publish assets for all service providers without prompt}
            {--provider= : The service provider that has assets you want to publish}
            {--tag=* : One or many tags that have assets you want to publish}"
    }

    fn description(&self) -> &str {
        "Publish any publishable assets from vendor packages"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let registry = PublishRegistry::resolve(app.container());
        let mut publisher = Publisher {
            cmd: &cmd,
            app: app.clone(),
            provider: None,
            published_at: Carbon::now(),
        };

        let mut tags = Vec::new();
        if !cmd.option_bool("all") {
            publisher.provider = cmd.option("provider").filter(|provider| !provider.is_empty());
            tags = cmd
                .option_list("tag")
                .into_iter()
                .filter(|tag| !tag.is_empty())
                .collect();
            if publisher.provider.is_none() && tags.is_empty() {
                tags = publisher.prompt(&registry)?;
            }
        }

        if tags.is_empty() {
            publisher.publish_tag(&registry, None).await?;
        }
        for tag in &tags {
            publisher.publish_tag(&registry, Some(tag)).await?;
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------
// lang:publish
// ----------------------------------------------------------------------

/// `lang:publish` — Publish all language files that are available for
/// customization: the framework's `auth`, `pagination`, `passwords` and
/// `validation` lines, as `lang/en/{group}.json`.
pub struct LangPublishCommand;

#[async_trait]
impl Command for LangPublishCommand {
    fn signature(&self) -> &str {
        "lang:publish
            {--existing : Publish and overwrite only the files that have already been published}
            {--force : Overwrite any existing files}"
    }

    fn description(&self) -> &str {
        "Publish all language files that are available for customization"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let lang_path = PathBuf::from(app.lang_path("en"));
        std::fs::create_dir_all(&lang_path)?;

        for (group, contents) in illuminate_translation::FRAMEWORK_FILES {
            let to = lang_path.join(format!("{group}.json"));
            if should_write(&cmd, to.exists()) {
                std::fs::write(&to, contents)?;
            }
        }

        cmd.components().info("Language files published successfully.");
        Ok(())
    }
}

// ----------------------------------------------------------------------
// stub:publish
// ----------------------------------------------------------------------

/// `stub:publish` — Publish all stubs that are available for customization.
///
/// The `make:*` generators use your application's `stubs/{name}.stub`
/// whenever it exists.
pub struct StubPublishCommand;

#[async_trait]
impl Command for StubPublishCommand {
    fn signature(&self) -> &str {
        "stub:publish
            {--existing : Publish and overwrite only the files that have already been published}
            {--force : Overwrite any existing files}"
    }

    fn description(&self) -> &str {
        "Publish all stubs that are available for customization"
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let stubs_path = PathBuf::from(app.base_path("stubs"));
        std::fs::create_dir_all(&stubs_path)?;

        for (name, contents) in stubs::ALL {
            let to = stubs_path.join(name);
            if should_write(&cmd, to.exists()) {
                std::fs::write(&to, contents)?;
            }
        }

        cmd.components().info("Stubs published successfully.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_dates_are_recognized() {
        assert_eq!(
            strip_migration_date("2019_12_14_000001_create_personal_access_tokens_table.rs"),
            Some("create_personal_access_tokens_table.rs")
        );
        assert_eq!(strip_migration_date("create_users_table.rs"), None);
        assert_eq!(strip_migration_date("2019_12_14_00001_x.rs"), None);
    }

    #[test]
    fn every_framework_configuration_file_can_be_published() {
        assert_eq!(CONFIG_FILES.len(), 16);
        for (name, contents) in CONFIG_FILES {
            assert!(contents.starts_with("use laravel::prelude::*;"), "{name}");
            assert!(contents.contains("pub fn config() -> Value {"), "{name}");
        }
        assert!(config_stub("cors").unwrap().contains("\"supports_credentials\": false"));
        assert!(config_stub("missing").is_none());
    }
}
