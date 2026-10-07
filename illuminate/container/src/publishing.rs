//! Publishing a package's files into the application — Laravel's
//! `$this->publishes([...], 'courier-config')`.
//!
//! A package's service provider registers what it can publish, usually its
//! configuration file, migrations, views and translations, under one or more
//! *tags*. `cargo artisan vendor:publish --tag=courier-config` then copies the
//! files into the application, where they are the application's to change.
//!
//! Rust packages ship compiled, so their files are usually embedded with
//! `include_str!`; a file or directory on disk works too:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_container::{Container, PublishRegistry, Publishable, ServiceProvider};
//!
//! struct CourierServiceProvider;
//!
//! impl ServiceProvider for CourierServiceProvider {
//!     fn boot(&self, app: &Container) {
//!         self.publishes(app, [Publishable::config("courier.rs", "pub fn config() {}")], "courier-config");
//!
//!         self.publishes_migrations(
//!             app,
//!             [Publishable::migration("2024_01_01_000000_create_couriers_table.rs", "// ...")],
//!             "courier-migrations",
//!         );
//!     }
//! }
//!
//! let app = Container::new();
//! CourierServiceProvider.boot(&app);
//!
//! let registry = PublishRegistry::resolve(&app);
//! assert_eq!(registry.publishable_groups(), ["courier-config", "courier-migrations"]);
//! assert_eq!(registry.paths_to_publish(None, Some("courier-config"))[0].destination(), "config/courier.rs");
//! ```

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::{Container, ServiceProvider};

/// The application directory a published file is written to. The
/// `vendor:publish` command resolves it with the application's paths, so a
/// package never needs to know where the application lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PublishRoot {
    /// The application's base path.
    Base,
    /// `config/`.
    Config,
    /// `database/`.
    Database,
    /// `lang/`.
    Lang,
    /// `public/`.
    Public,
    /// `resources/`.
    Resources,
    /// `storage/`.
    Storage,
}

impl PublishRoot {
    /// The directory's conventional location, relative to the base path.
    pub fn directory(&self) -> &'static str {
        match self {
            PublishRoot::Base => "",
            PublishRoot::Config => "config",
            PublishRoot::Database => "database",
            PublishRoot::Lang => "lang",
            PublishRoot::Public => "public",
            PublishRoot::Resources => "resources",
            PublishRoot::Storage => "storage",
        }
    }
}

/// What a [`Publishable`] copies into the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishSource {
    /// Contents compiled into the package (typically with `include_str!`).
    Contents(Cow<'static, str>),
    /// A file, or a directory that is copied recursively.
    Path(PathBuf),
}

/// A file (or directory) a package can publish into the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publishable {
    /// What is copied.
    pub source: PublishSource,
    /// The application directory it is copied into.
    pub root: PublishRoot,
    /// Where it goes, relative to `root`.
    pub path: String,
    /// Whether it is a migration whose timestamp is updated when it is
    /// published (see [`ServiceProvider::publishes_migrations`]).
    pub migration: bool,
}

impl Publishable {
    /// Publish the given contents to `path` within `root`.
    pub fn contents(root: PublishRoot, path: impl Into<String>, contents: impl Into<Cow<'static, str>>) -> Self {
        Self {
            source: PublishSource::Contents(contents.into()),
            root,
            path: path.into(),
            migration: false,
        }
    }

    /// Publish a file or directory from disk to `path` within `root`.
    pub fn path(from: impl Into<PathBuf>, root: PublishRoot, path: impl Into<String>) -> Self {
        Self {
            source: PublishSource::Path(from.into()),
            root,
            path: path.into(),
            migration: false,
        }
    }

    /// A configuration file: `Publishable::config("courier.rs", CONFIG)`
    /// publishes `config/courier.rs`, which `vendor:publish` also adds to
    /// the application's `config_files!` list.
    pub fn config(name: impl Into<String>, contents: impl Into<Cow<'static, str>>) -> Self {
        Self::contents(PublishRoot::Config, name, contents)
    }

    /// A migration, published into `database/migrations`.
    pub fn migration(file: impl Into<String>, contents: impl Into<Cow<'static, str>>) -> Self {
        Self::contents(PublishRoot::Database, format!("migrations/{}", file.into()), contents).as_migration()
    }

    /// A view, published into `resources/views` (`"vendor/courier/mail.blade.html"`).
    pub fn view(path: impl Into<String>, contents: impl Into<Cow<'static, str>>) -> Self {
        Self::contents(PublishRoot::Resources, format!("views/{}", path.into()), contents)
    }

    /// A language file, published into `lang` (`"vendor/courier/en/messages.json"`).
    pub fn lang(path: impl Into<String>, contents: impl Into<Cow<'static, str>>) -> Self {
        Self::contents(PublishRoot::Lang, path, contents)
    }

    /// A public asset, published into `public` (`"vendor/courier/app.css"`).
    pub fn asset(path: impl Into<String>, contents: impl Into<Cow<'static, str>>) -> Self {
        Self::contents(PublishRoot::Public, path, contents)
    }

    /// Mark the file as a migration, so its timestamp is updated when it is
    /// published.
    pub fn as_migration(mut self) -> Self {
        self.migration = true;
        self
    }

    /// The destination relative to the application's base path, using the
    /// conventional directory names (`config/courier.rs`).
    pub fn destination(&self) -> String {
        match self.root.directory() {
            "" => self.path.clone(),
            directory => format!("{directory}/{}", self.path.trim_start_matches('/')),
        }
    }

    /// Whether both publish to the same place.
    pub fn same_destination(&self, other: &Publishable) -> bool {
        self.root == other.root && self.path.trim_start_matches('/') == other.path.trim_start_matches('/')
    }

    /// Whether this publishes a configuration file directly within `config`.
    pub fn is_config_file(&self) -> bool {
        self.root == PublishRoot::Config && !self.path.contains('/') && self.path.ends_with(".rs")
    }
}

/// The tags (groups) a publish call files its paths under: a single tag
/// (`"courier-config"`), several (`["courier", "courier-config"]`), or none
/// (`()`).
pub trait PublishGroups {
    /// The tag names.
    fn into_groups(self) -> Vec<String>;
}

impl PublishGroups for () {
    fn into_groups(self) -> Vec<String> {
        Vec::new()
    }
}

impl PublishGroups for &str {
    fn into_groups(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl PublishGroups for String {
    fn into_groups(self) -> Vec<String> {
        vec![self]
    }
}

impl PublishGroups for Option<&str> {
    fn into_groups(self) -> Vec<String> {
        self.map(str::to_string).into_iter().collect()
    }
}

impl<const N: usize> PublishGroups for [&str; N] {
    fn into_groups(self) -> Vec<String> {
        self.iter().map(|group| group.to_string()).collect()
    }
}

impl PublishGroups for &[&str] {
    fn into_groups(self) -> Vec<String> {
        self.iter().map(|group| group.to_string()).collect()
    }
}

impl PublishGroups for Vec<&str> {
    fn into_groups(self) -> Vec<String> {
        self.into_iter().map(str::to_string).collect()
    }
}

impl PublishGroups for Vec<String> {
    fn into_groups(self) -> Vec<String> {
        self
    }
}

#[derive(Debug, Default)]
struct Entries {
    providers: Vec<(String, Vec<Publishable>)>,
    groups: Vec<(String, Vec<Publishable>)>,
}

fn merge(entries: &mut Vec<(String, Vec<Publishable>)>, key: &str, paths: &[Publishable]) {
    let position = match entries.iter().position(|(existing, _)| existing == key) {
        Some(position) => position,
        None => {
            entries.push((key.to_string(), Vec::new()));
            entries.len() - 1
        }
    };
    let list = &mut entries[position].1;
    for path in paths {
        match list.iter_mut().find(|existing| existing.same_destination(path)) {
            Some(existing) => *existing = path.clone(),
            None => list.push(path.clone()),
        }
    }
}

/// Whether a provider's registered name matches the one asked for: its
/// full type name (`laravel_sanctum::provider::SanctumServiceProvider`),
/// its crate and name (`laravel_sanctum::SanctumServiceProvider`), or just
/// its name (`SanctumServiceProvider`). Laravel's backslashes work too.
fn provider_matches(registered: &str, asked: &str) -> bool {
    let asked = asked.trim().replace('\\', "::");
    let asked = asked.trim_start_matches("::");
    if registered == asked {
        return true;
    }
    let short = registered.rsplit("::").next().unwrap_or(registered);
    let krate = registered.split("::").next().unwrap_or(registered);
    asked == short || asked == format!("{krate}::{short}")
}

/// Everything the application's packages can publish, by provider and by
/// tag — Laravel's `ServiceProvider::$publishes` and `$publishGroups`.
///
/// The registry lives in the container, so each application (and each test)
/// has its own.
#[derive(Debug, Default)]
pub struct PublishRegistry {
    entries: RwLock<Entries>,
}

impl PublishRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The container's registry, created on first use.
    pub fn resolve(app: &Container) -> Arc<PublishRegistry> {
        app.singleton_if::<PublishRegistry>(|_| Arc::new(PublishRegistry::new()));
        app.make::<PublishRegistry>()
    }

    /// The current container's registry, created on first use.
    pub fn current() -> Arc<PublishRegistry> {
        Self::resolve(&Container::get_instance())
    }

    /// Register paths a provider publishes, under the given tags.
    pub fn publishes(&self, provider: &str, paths: impl IntoIterator<Item = Publishable>, groups: impl PublishGroups) {
        let paths: Vec<Publishable> = paths.into_iter().collect();
        let mut entries = self.entries.write().unwrap();
        merge(&mut entries.providers, provider, &paths);
        for group in groups.into_groups() {
            merge(&mut entries.groups, &group, &paths);
        }
    }

    /// The paths to publish for a provider and / or tag — every path when
    /// neither is given, nothing when the provider or tag is unknown.
    pub fn paths_to_publish(&self, provider: Option<&str>, group: Option<&str>) -> Vec<Publishable> {
        let entries = self.entries.read().unwrap();
        let for_provider = |provider: &str| -> Vec<Publishable> {
            entries
                .providers
                .iter()
                .filter(|(registered, _)| provider_matches(registered, provider))
                .flat_map(|(_, paths)| paths.iter().cloned())
                .collect()
        };
        let for_group = |group: &str| -> Vec<Publishable> {
            entries
                .groups
                .iter()
                .find(|(name, _)| name == group)
                .map(|(_, paths)| paths.clone())
                .unwrap_or_default()
        };

        match (provider.filter(|p| !p.is_empty()), group.filter(|g| !g.is_empty())) {
            (Some(provider), Some(group)) => {
                let in_group = for_group(group);
                for_provider(provider)
                    .into_iter()
                    .filter(|path| in_group.iter().any(|candidate| candidate == path))
                    .collect()
            }
            (None, Some(group)) => for_group(group),
            (Some(provider), None) => for_provider(provider),
            (None, None) => {
                let mut all: Vec<Publishable> = Vec::new();
                for (_, paths) in &entries.providers {
                    for path in paths {
                        match all.iter_mut().find(|existing| existing.same_destination(path)) {
                            Some(existing) => *existing = path.clone(),
                            None => all.push(path.clone()),
                        }
                    }
                }
                all
            }
        }
    }

    /// The providers that have something to publish, sorted.
    pub fn publishable_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> =
            self.entries.read().unwrap().providers.iter().map(|(name, _)| name.clone()).collect();
        providers.sort();
        providers
    }

    /// The tags that have something to publish, sorted.
    pub fn publishable_groups(&self) -> Vec<String> {
        let mut groups: Vec<String> =
            self.entries.read().unwrap().groups.iter().map(|(name, _)| name.clone()).collect();
        groups.sort();
        groups
    }

    /// Determine if a provider (by any of its names) publishes anything.
    pub fn has_provider(&self, provider: &str) -> bool {
        self.entries
            .read()
            .unwrap()
            .providers
            .iter()
            .any(|(registered, _)| provider_matches(registered, provider))
    }

    /// Forget everything that was registered.
    pub fn flush(&self) {
        *self.entries.write().unwrap() = Entries::default();
    }
}

/// Register a provider's publishable paths (the body of
/// [`ServiceProvider::publishes`]).
pub(crate) fn publishes<P: ServiceProvider + ?Sized>(
    provider: &P,
    app: &Container,
    paths: impl IntoIterator<Item = Publishable>,
    groups: impl PublishGroups,
) {
    PublishRegistry::resolve(app).publishes(provider.type_name(), paths, groups);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CourierServiceProvider;

    impl ServiceProvider for CourierServiceProvider {
        fn boot(&self, app: &Container) {
            self.publishes(app, [Publishable::config("courier.rs", "config")], "courier-config");
            self.publishes(app, [Publishable::view("vendor/courier/mail.blade.html", "view")], ["courier", "courier-views"]);
            self.publishes_migrations(app, [Publishable::migration("2024_01_01_000000_create_couriers_table.rs", "up")], "courier-migrations");
        }
    }

    struct PodcastServiceProvider;

    impl ServiceProvider for PodcastServiceProvider {
        fn boot(&self, app: &Container) {
            self.publishes(app, [Publishable::config("podcast.rs", "podcast")], ());
        }
    }

    fn registry() -> Arc<PublishRegistry> {
        let app = Container::new();
        CourierServiceProvider.boot(&app);
        PodcastServiceProvider.boot(&app);
        PublishRegistry::resolve(&app)
    }

    #[test]
    fn paths_are_registered_by_provider_and_tag() {
        let registry = registry();
        assert_eq!(registry.publishable_groups(), ["courier", "courier-config", "courier-migrations", "courier-views"]);
        assert_eq!(registry.publishable_providers().len(), 2);
        assert!(registry.has_provider("CourierServiceProvider"));
        assert!(registry.has_provider("illuminate_container::CourierServiceProvider"));
        assert!(!registry.has_provider("MissingServiceProvider"));

        let config = registry.paths_to_publish(None, Some("courier-config"));
        assert_eq!(config.len(), 1);
        assert_eq!(config[0].destination(), "config/courier.rs");
        assert!(config[0].is_config_file());

        let migrations = registry.paths_to_publish(None, Some("courier-migrations"));
        assert!(migrations[0].migration);
        assert_eq!(migrations[0].destination(), "database/migrations/2024_01_01_000000_create_couriers_table.rs");
    }

    #[test]
    fn paths_can_be_filtered_by_provider_and_tag() {
        let registry = registry();
        assert_eq!(registry.paths_to_publish(Some("CourierServiceProvider"), None).len(), 3);
        assert_eq!(registry.paths_to_publish(Some("CourierServiceProvider"), Some("courier-views")).len(), 1);
        assert!(registry.paths_to_publish(Some("PodcastServiceProvider"), Some("courier-views")).is_empty());
        assert!(registry.paths_to_publish(None, Some("missing")).is_empty());
        assert!(registry.paths_to_publish(Some("Missing"), None).is_empty());
        assert_eq!(registry.paths_to_publish(None, None).len(), 4);
    }

    #[test]
    fn publishing_to_the_same_destination_twice_replaces_it() {
        let registry = PublishRegistry::new();
        registry.publishes("Provider", [Publishable::config("a.rs", "one")], "tag");
        registry.publishes("Provider", [Publishable::config("a.rs", "two"), Publishable::config("b.rs", "two")], "tag");
        let paths = registry.paths_to_publish(None, Some("tag"));
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].source, PublishSource::Contents("two".into()));

        registry.flush();
        assert!(registry.publishable_groups().is_empty());
    }
}
