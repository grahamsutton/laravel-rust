//! The Laravel application.

use std::collections::HashSet;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_support::{Env, Str, Value};

use crate::bootstrap::{self, ConfigFile};

/// The version of the framework.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

type Callback = Box<dyn FnOnce(&Application) + Send>;

/// The Laravel application: a service container that knows where it lives,
/// what environment it runs in, and how to boot itself.
///
/// The application dereferences to its [`Container`], so you can bind and
/// resolve services directly on it.
pub struct Application {
    container: Arc<Container>,
    base_path: PathBuf,
    paths: RwLock<Paths>,
    environment_file: RwLock<String>,
    running_in_console: AtomicBool,
    providers: Mutex<Vec<Arc<dyn ServiceProvider>>>,
    registered_providers: Mutex<HashSet<&'static str>>,
    has_been_bootstrapped: AtomicBool,
    booted: AtomicBool,
    booting_callbacks: Mutex<Vec<Callback>>,
    booted_callbacks: Mutex<Vec<Callback>>,
    terminating_callbacks: Mutex<Vec<Callback>>,
    config_files: Mutex<Vec<ConfigFile>>,
}

/// The application's well-known directories.
#[derive(Clone, Debug)]
struct Paths {
    app: PathBuf,
    bootstrap: PathBuf,
    config: PathBuf,
    database: PathBuf,
    lang: PathBuf,
    public: PathBuf,
    resources: PathBuf,
    storage: PathBuf,
}

impl Paths {
    fn new(base: &Path) -> Self {
        Self {
            app: base.join("app"),
            bootstrap: base.join("bootstrap"),
            config: base.join("config"),
            database: base.join("database"),
            lang: base.join("lang"),
            public: base.join("public"),
            resources: base.join("resources"),
            storage: base.join("storage"),
        }
    }
}

impl Application {
    /// Create a new application rooted at the given base path, and make it
    /// the globally available instance.
    pub fn new(base_path: impl Into<PathBuf>) -> Arc<Self> {
        let app = Self::new_detached(base_path);
        Container::set_instance(app.container.clone());
        app
    }

    /// Create a new application without making it the global instance.
    ///
    /// Tests use this together with [`Container::set_local_instance`] so
    /// every test gets its own isolated application.
    pub fn new_detached(base_path: impl Into<PathBuf>) -> Arc<Self> {
        let base_path = base_path.into();
        let container = Arc::new(Container::new());
        let app = Arc::new(Self {
            paths: RwLock::new(Paths::new(&base_path)),
            base_path,
            container: container.clone(),
            environment_file: RwLock::new(".env".to_string()),
            running_in_console: AtomicBool::new(false),
            providers: Mutex::new(Vec::new()),
            registered_providers: Mutex::new(HashSet::new()),
            has_been_bootstrapped: AtomicBool::new(false),
            booted: AtomicBool::new(false),
            booting_callbacks: Mutex::new(Vec::new()),
            booted_callbacks: Mutex::new(Vec::new()),
            terminating_callbacks: Mutex::new(Vec::new()),
            config_files: Mutex::new(Vec::new()),
        });
        container.instance_arc::<Application>(app.clone());
        container.instance_arc::<Container>(container.clone());
        app
    }

    /// Get the current application instance.
    ///
    /// # Panics
    ///
    /// Panics if no application has been created.
    pub fn current() -> Arc<Application> {
        Container::get_instance()
            .try_make::<Application>()
            .expect("A Laravel application has not been created yet.")
    }

    /// Get the current application instance, if one exists.
    pub fn try_current() -> Option<Arc<Application>> {
        Container::get_instance().try_make::<Application>().ok()
    }

    /// The application's service container.
    pub fn container(&self) -> &Arc<Container> {
        &self.container
    }

    /// The version number of the framework.
    pub fn version(&self) -> &'static str {
        VERSION
    }

    // ------------------------------------------------------------------
    // Paths
    // ------------------------------------------------------------------

    fn join(base: &Path, path: &str) -> String {
        let joined = if path.is_empty() {
            base.to_path_buf()
        } else {
            base.join(path.trim_start_matches(['/', '\\']))
        };
        joined.to_string_lossy().into_owned()
    }

    /// The base path of the installation.
    pub fn base_path(&self, path: &str) -> String {
        Self::join(&self.base_path, path)
    }

    /// The path to the application ("app") directory.
    pub fn app_path(&self, path: &str) -> String {
        Self::join(&self.paths.read().unwrap().app, path)
    }

    /// The path to the bootstrap directory.
    pub fn bootstrap_path(&self, path: &str) -> String {
        Self::join(&self.paths.read().unwrap().bootstrap, path)
    }

    /// The path to the configuration directory.
    pub fn config_path(&self, path: &str) -> String {
        Self::join(&self.paths.read().unwrap().config, path)
    }

    /// The path to the database directory.
    pub fn database_path(&self, path: &str) -> String {
        Self::join(&self.paths.read().unwrap().database, path)
    }

    /// The path to the language files.
    pub fn lang_path(&self, path: &str) -> String {
        Self::join(&self.paths.read().unwrap().lang, path)
    }

    /// The path to the public / web directory.
    pub fn public_path(&self, path: &str) -> String {
        Self::join(&self.paths.read().unwrap().public, path)
    }

    /// The path to the resources directory.
    pub fn resource_path(&self, path: &str) -> String {
        Self::join(&self.paths.read().unwrap().resources, path)
    }

    /// The path to the storage directory.
    pub fn storage_path(&self, path: &str) -> String {
        Self::join(&self.paths.read().unwrap().storage, path)
    }

    /// The path to the environment file directory.
    pub fn environment_path(&self) -> String {
        self.base_path("")
    }

    /// The environment file the application is using.
    pub fn environment_file(&self) -> String {
        self.environment_file.read().unwrap().clone()
    }

    /// The fully qualified path to the environment file.
    pub fn environment_file_path(&self) -> String {
        self.base_path(&self.environment_file())
    }

    /// Set the environment file to be loaded during bootstrapping.
    pub fn load_environment_from(&self, file: impl Into<String>) -> &Self {
        *self.environment_file.write().unwrap() = file.into();
        self
    }

    /// Set the application directory.
    pub fn use_app_path(&self, path: impl Into<PathBuf>) -> &Self {
        self.paths.write().unwrap().app = path.into();
        self
    }

    /// Set the database directory.
    pub fn use_database_path(&self, path: impl Into<PathBuf>) -> &Self {
        self.paths.write().unwrap().database = path.into();
        self
    }

    /// Set the language file directory.
    pub fn use_lang_path(&self, path: impl Into<PathBuf>) -> &Self {
        self.paths.write().unwrap().lang = path.into();
        self
    }

    /// Set the public / web directory.
    pub fn use_public_path(&self, path: impl Into<PathBuf>) -> &Self {
        self.paths.write().unwrap().public = path.into();
        self
    }

    /// Set the resources directory.
    pub fn use_resource_path(&self, path: impl Into<PathBuf>) -> &Self {
        self.paths.write().unwrap().resources = path.into();
        self
    }

    /// Set the storage directory.
    pub fn use_storage_path(&self, path: impl Into<PathBuf>) -> &Self {
        self.paths.write().unwrap().storage = path.into();
        self
    }

    /// Set the bootstrap directory.
    pub fn use_bootstrap_path(&self, path: impl Into<PathBuf>) -> &Self {
        self.paths.write().unwrap().bootstrap = path.into();
        self
    }

    // ------------------------------------------------------------------
    // Environment
    // ------------------------------------------------------------------

    /// Get the current application environment (`local`, `production`, ...).
    pub fn environment(&self) -> String {
        self.config()
            .and_then(|config| match config.get("app.env") {
                Value::Null => None,
                value => Some(illuminate_support::ValueExt::to_string_lossy(&value)),
            })
            .or_else(|| Env::raw("APP_ENV"))
            .unwrap_or_else(|| "production".to_string())
    }

    /// Determine if the application environment matches any of the patterns.
    ///
    /// ```
    /// # use illuminate_foundation::Application;
    /// # let app = Application::new_detached("/tmp");
    /// # app.config_repository().set("app.env", "staging");
    /// assert!(app.environment_is(&["local", "staging"]));
    /// assert!(app.environment_is(&["stag*"]));
    /// ```
    pub fn environment_is(&self, patterns: &[&str]) -> bool {
        let environment = self.environment();
        patterns.iter().any(|p| Str::is(p, &environment))
    }

    /// Determine if the application is in the local environment.
    pub fn is_local(&self) -> bool {
        self.environment() == "local"
    }

    /// Determine if the application is in the production environment.
    pub fn is_production(&self) -> bool {
        self.environment() == "production"
    }

    /// Determine if the application is running unit tests.
    pub fn running_unit_tests(&self) -> bool {
        self.environment() == "testing"
    }

    /// Determine if the application is running in the console.
    pub fn running_in_console(&self) -> bool {
        self.running_in_console.load(Ordering::SeqCst)
    }

    /// Mark whether the application is running in the console.
    pub fn set_running_in_console(&self, value: bool) {
        self.running_in_console.store(value, Ordering::SeqCst);
    }

    /// Determine if the application has debug mode enabled.
    pub fn has_debug_mode_enabled(&self) -> bool {
        self.config().is_some_and(|config| config.boolean("app.debug"))
    }

    /// Get the current application locale.
    pub fn get_locale(&self) -> String {
        self.config()
            .map(|c| c.string_or("app.locale", "en"))
            .unwrap_or_else(|| "en".into())
    }

    /// Set the current application locale.
    pub fn set_locale(&self, locale: &str) {
        self.config_repository().set("app.locale", locale);
    }

    /// Determine if the given locale is the current one.
    pub fn is_locale(&self, locale: &str) -> bool {
        self.get_locale() == locale
    }

    // ------------------------------------------------------------------
    // Maintenance mode
    // ------------------------------------------------------------------

    /// The path of the maintenance mode marker file.
    pub fn maintenance_file(&self) -> String {
        self.storage_path("framework/down")
    }

    /// Determine if the application is currently down for maintenance.
    pub fn is_down_for_maintenance(&self) -> bool {
        Path::new(&self.maintenance_file()).exists()
    }

    /// The maintenance mode payload (`secret`, `retry`, `refresh`, ...).
    pub fn maintenance_data(&self) -> Option<Value> {
        let contents = std::fs::read_to_string(self.maintenance_file()).ok()?;
        serde_json::from_str(&contents).ok()
    }

    // ------------------------------------------------------------------
    // Configuration
    // ------------------------------------------------------------------

    fn config(&self) -> Option<Arc<Repository>> {
        self.container.try_make::<Repository>().ok()
    }

    /// The configuration repository (created on first use).
    pub fn config_repository(&self) -> Arc<Repository> {
        self.container
            .singleton_if::<Repository>(|_| Arc::new(Repository::empty()));
        self.container.make::<Repository>()
    }

    /// Register the application's configuration files.
    pub fn add_config_files(&self, files: impl IntoIterator<Item = ConfigFile>) {
        self.config_files.lock().unwrap().extend(files);
    }

    /// The registered configuration files.
    pub fn config_files(&self) -> Vec<ConfigFile> {
        self.config_files.lock().unwrap().clone()
    }

    // ------------------------------------------------------------------
    // Service providers
    // ------------------------------------------------------------------

    /// Register a service provider with the application.
    ///
    /// Registering the same provider type twice is a no-op. If the
    /// application has already booted, the provider is booted immediately.
    pub fn register<P: ServiceProvider>(&self, provider: P) -> bool {
        self.register_arc(Arc::new(provider))
    }

    /// Register an already-shared service provider.
    pub fn register_arc(&self, provider: Arc<dyn ServiceProvider>) -> bool {
        if !self.registered_providers.lock().unwrap().insert(provider.type_name()) {
            return false;
        }
        provider.register(&self.container);
        self.providers.lock().unwrap().push(provider.clone());
        if self.is_booted() {
            provider.boot(&self.container);
        }
        true
    }

    /// Register a boxed service provider (as listed in `bootstrap/providers.rs`).
    pub fn register_boxed(&self, provider: Box<dyn ServiceProvider>) -> bool {
        self.register_arc(Arc::from(provider))
    }

    /// Determine if a provider of the given type has been registered.
    pub fn provider_is_loaded<P: ServiceProvider>(&self) -> bool {
        self.registered_providers
            .lock()
            .unwrap()
            .contains(std::any::type_name::<P>())
    }

    /// The names of the loaded service providers.
    pub fn loaded_providers(&self) -> Vec<String> {
        self.providers
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.name())
            .collect()
    }

    /// Register a callback to run before the application boots.
    pub fn booting(&self, callback: impl FnOnce(&Application) + Send + 'static) {
        self.booting_callbacks.lock().unwrap().push(Box::new(callback));
    }

    /// Register a callback to run after the application boots (immediately
    /// if it already has).
    pub fn booted(&self, callback: impl FnOnce(&Application) + Send + 'static) {
        if self.is_booted() {
            callback(self);
        } else {
            self.booted_callbacks.lock().unwrap().push(Box::new(callback));
        }
    }

    /// Determine if the application has booted.
    pub fn is_booted(&self) -> bool {
        self.booted.load(Ordering::SeqCst)
    }

    /// Boot the application's service providers.
    pub fn boot(&self) {
        if self.is_booted() {
            return;
        }

        let booting: Vec<Callback> = std::mem::take(&mut *self.booting_callbacks.lock().unwrap());
        for callback in booting {
            callback(self);
        }

        // Providers registered while booting are booted as well.
        let mut index = 0;
        loop {
            let provider = self.providers.lock().unwrap().get(index).cloned();
            match provider {
                Some(provider) => provider.boot(&self.container),
                None => break,
            }
            index += 1;
        }

        self.booted.store(true, Ordering::SeqCst);

        let booted: Vec<Callback> = std::mem::take(&mut *self.booted_callbacks.lock().unwrap());
        for callback in booted {
            callback(self);
        }
    }

    /// Determine if the application has been bootstrapped.
    pub fn has_been_bootstrapped(&self) -> bool {
        self.has_been_bootstrapped.load(Ordering::SeqCst)
    }

    /// Run the standard bootstrappers: load the environment and
    /// configuration, register providers, and boot them.
    pub fn bootstrap(&self) {
        if self.has_been_bootstrapped.swap(true, Ordering::SeqCst) {
            return;
        }
        bootstrap::load_environment_variables(self);
        bootstrap::load_configuration(self);
        bootstrap::register_providers(self);
        self.boot();
    }

    /// Register a terminating callback.
    pub fn terminating(&self, callback: impl FnOnce(&Application) + Send + 'static) {
        self.terminating_callbacks.lock().unwrap().push(Box::new(callback));
    }

    /// Terminate the application, running the terminating callbacks.
    pub fn terminate(&self) {
        let callbacks: Vec<Callback> = std::mem::take(&mut *self.terminating_callbacks.lock().unwrap());
        for callback in callbacks {
            callback(self);
        }
    }
}

impl Deref for Application {
    type Target = Container;

    fn deref(&self) -> &Container {
        &self.container
    }
}

impl std::fmt::Debug for Application {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Application")
            .field("base_path", &self.base_path)
            .field("booted", &self.is_booted())
            .finish()
    }
}
