//! The view factory: finding, creating and rendering views, shared data and
//! view composers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use illuminate_config::Repository;
use illuminate_container::{Container, try_app};
use illuminate_http::Request;
use illuminate_support::{Map, Result, Str, Value};

use crate::compiler::BladeCompiler;
use crate::finder::{FileViewFinder, normalize_name};
use crate::value::{ViewData, ViewValue, view_data_from_serialize};
use crate::view::View;

/// A view composer or creator.
pub type ViewCallback = Arc<dyn Fn(&mut View) + Send + Sync>;

/// Resolves data to share with every view from the current request (this is
/// how the session shares `$errors` and old input).
pub type ShareResolver = Arc<dyn Fn(&Request) -> Map<String, Value> + Send + Sync>;

struct Inner {
    finder: FileViewFinder,
    blade: BladeCompiler,
    shared: RwLock<ViewData>,
    composers: RwLock<Vec<(String, ViewCallback)>>,
    creators: RwLock<Vec<(String, ViewCallback)>>,
    resolvers: RwLock<Vec<ShareResolver>>,
}

/// The view factory (`View` facade).
///
/// `Factory` is a cheap, cloneable handle: clones share paths, shared data,
/// composers and the Blade compiler.
///
/// ```
/// use illuminate_view::Factory;
/// use illuminate_support::json;
///
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::write(dir.path().join("greeting.blade.html"), "Hello, {{ $name }}!").unwrap();
///
/// let factory = Factory::new([dir.path()]);
/// let html = factory.make("greeting", json!({"name": "James"})).render().unwrap();
///
/// assert_eq!(html, "Hello, James!");
/// ```
#[derive(Clone)]
pub struct Factory {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Factory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Factory")
            .field("paths", &self.inner.finder.paths())
            .finish_non_exhaustive()
    }
}

/// Anything that can be used as view data: a JSON object, any serializable
/// struct or map, or [`ViewData`].
pub trait IntoViewData {
    /// Convert into view data.
    fn into_view_data(self) -> ViewData;
}

impl IntoViewData for ViewData {
    fn into_view_data(self) -> ViewData {
        self
    }
}

impl IntoViewData for &ViewData {
    fn into_view_data(self) -> ViewData {
        self.clone()
    }
}

impl<T: serde::Serialize> IntoViewData for T {
    fn into_view_data(self) -> ViewData {
        view_data_from_serialize(&self)
    }
}

/// One or more view name patterns (`"profile"`, `["profile", "dashboard"]`, `"admin.*"`).
pub trait ViewPatterns {
    /// The patterns.
    fn patterns(self) -> Vec<String>;
}

impl ViewPatterns for &str {
    fn patterns(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl ViewPatterns for String {
    fn patterns(self) -> Vec<String> {
        vec![self]
    }
}

impl ViewPatterns for &[&str] {
    fn patterns(self) -> Vec<String> {
        self.iter().map(|s| s.to_string()).collect()
    }
}

impl<const N: usize> ViewPatterns for [&str; N] {
    fn patterns(self) -> Vec<String> {
        self.iter().map(|s| s.to_string()).collect()
    }
}

impl ViewPatterns for Vec<&str> {
    fn patterns(self) -> Vec<String> {
        self.into_iter().map(str::to_string).collect()
    }
}

impl ViewPatterns for Vec<String> {
    fn patterns(self) -> Vec<String> {
        self
    }
}

impl Factory {
    /// Create a factory that finds views in the given directories.
    pub fn new(paths: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        Self::with_compiler(paths, BladeCompiler::new())
    }

    /// Create a factory using an existing Blade compiler.
    pub fn with_compiler(
        paths: impl IntoIterator<Item = impl Into<PathBuf>>,
        blade: BladeCompiler,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                finder: FileViewFinder::new(paths),
                blade,
                shared: RwLock::new(ViewData::new()),
                composers: RwLock::new(Vec::new()),
                creators: RwLock::new(Vec::new()),
                resolvers: RwLock::new(Vec::new()),
            }),
        }
    }

    /// Create a factory from the `view.paths` configuration.
    pub fn from_config(config: &Repository) -> Self {
        let paths: Vec<String> = config.strings("view.paths");
        Self::new(paths)
    }

    /// Resolve the factory from the container, registering one (configured
    /// from `view.paths`) if the application hasn't yet.
    pub fn resolve() -> Self {
        if let Some(factory) = try_app::<Factory>() {
            return (*factory).clone();
        }
        let container = Container::get_instance();
        container.singleton_if::<Factory>(|app| {
            let factory = match app.try_make::<Repository>() {
                Ok(config) => Factory::from_config(&config),
                Err(_) => Factory::new(Vec::<PathBuf>::new()),
            };
            Arc::new(factory)
        });
        (*container.make::<Factory>()).clone()
    }

    /// The Blade compiler.
    pub fn blade(&self) -> &BladeCompiler {
        &self.inner.blade
    }

    /// The view finder.
    pub fn finder(&self) -> &FileViewFinder {
        &self.inner.finder
    }

    // ------------------------------------------------------------------
    // Creating views
    // ------------------------------------------------------------------

    /// Get a view instance. The view is found when it is rendered.
    pub fn make(&self, name: &str, data: impl IntoViewData) -> View {
        let mut view = View::named(self, &normalize_name(name), data.into_view_data());
        self.call_creators(&mut view);
        view
    }

    /// Get a view for a file on disk.
    pub fn file(&self, path: impl Into<PathBuf>, data: impl IntoViewData) -> View {
        let path = path.into();
        let mut view = View::file(self, path, data.into_view_data());
        self.call_creators(&mut view);
        view
    }

    /// Get the first view that exists.
    pub fn first(&self, names: &[&str], data: impl IntoViewData) -> Result<View> {
        match names.iter().find(|name| self.exists(name)) {
            Some(name) => Ok(self.make(name, data)),
            None => Err(crate::exception::InvalidArgumentException::new(
                "None of the views in the given array exist.",
            )
            .into()),
        }
    }

    /// Get a view for an inline Blade template.
    pub fn inline(&self, template: &str, data: impl IntoViewData) -> View {
        View::inline(self, template, data.into_view_data())
    }

    /// Render an inline Blade template (`Blade::render`).
    pub fn render_inline(&self, template: &str, data: impl IntoViewData) -> Result<String> {
        self.inline(template, data).render()
    }

    /// Render a view if the condition is true (an empty string otherwise).
    pub fn render_when(
        &self,
        condition: bool,
        name: &str,
        data: impl IntoViewData,
    ) -> Result<String> {
        if condition {
            self.make(name, data).render()
        } else {
            Ok(String::new())
        }
    }

    /// Render a view unless the condition is true.
    pub fn render_unless(
        &self,
        condition: bool,
        name: &str,
        data: impl IntoViewData,
    ) -> Result<String> {
        self.render_when(!condition, name, data)
    }

    /// Determine if a view exists.
    pub fn exists(&self, name: &str) -> bool {
        self.inner.finder.exists(&normalize_name(name))
    }

    pub(crate) fn find(&self, name: &str) -> Result<PathBuf> {
        self.inner.finder.find(name)
    }

    pub(crate) fn find_in_directory(&self, directory: &Path, name: &str) -> Option<PathBuf> {
        self.inner.finder.find_in_directory(directory, name)
    }

    // ------------------------------------------------------------------
    // Shared data
    // ------------------------------------------------------------------

    /// Share a piece of data with every view.
    pub fn share(&self, key: &str, value: impl Into<ViewValue>) {
        self.inner
            .shared
            .write()
            .unwrap()
            .insert(key.to_string(), value.into());
    }

    /// Share several pieces of data with every view.
    pub fn share_many(&self, data: impl IntoViewData) {
        let data = data.into_view_data();
        self.inner.shared.write().unwrap().extend(data);
    }

    /// Get a piece of shared data.
    pub fn shared(&self, key: &str) -> Option<ViewValue> {
        self.inner.shared.read().unwrap().get(key).cloned()
    }

    /// All of the shared data.
    pub fn get_shared(&self) -> ViewData {
        self.inner.shared.read().unwrap().clone()
    }

    pub(crate) fn shared_data(&self) -> ViewData {
        self.inner.shared.read().unwrap().clone()
    }

    /// Share data resolved from the current request with every view
    /// rendered while that request is handled.
    ///
    /// ```
    /// use illuminate_view::Factory;
    /// use illuminate_support::{json, Map};
    ///
    /// let factory = Factory::new(Vec::<String>::new());
    /// factory.share_resolver(|request| {
    ///     let mut data = Map::new();
    ///     data.insert("path".into(), json!(request.path()));
    ///     data
    /// });
    /// ```
    pub fn share_resolver(
        &self,
        resolver: impl Fn(&Request) -> Map<String, Value> + Send + Sync + 'static,
    ) {
        self.inner
            .resolvers
            .write()
            .unwrap()
            .push(Arc::new(resolver));
    }

    /// Data from the share resolvers for the current request.
    pub(crate) fn request_shared_data(&self) -> ViewData {
        let resolvers = self.inner.resolvers.read().unwrap().clone();
        if resolvers.is_empty() {
            return ViewData::new();
        }
        let Some(request) = illuminate_http::current_request() else {
            return ViewData::new();
        };
        let mut data = ViewData::new();
        for resolver in resolvers {
            for (key, value) in resolver(&request) {
                data.insert(key, ViewValue::from(value));
            }
        }
        data
    }

    // ------------------------------------------------------------------
    // Composers & creators
    // ------------------------------------------------------------------

    /// Register a view composer, called just before a matching view renders.
    /// Patterns may use `*` wildcards.
    pub fn composer(
        &self,
        views: impl ViewPatterns,
        callback: impl Fn(&mut View) + Send + Sync + 'static,
    ) {
        let callback: ViewCallback = Arc::new(callback);
        let mut composers = self.inner.composers.write().unwrap();
        for pattern in views.patterns() {
            composers.push((normalize_name(&pattern), callback.clone()));
        }
    }

    /// Register a view creator, called as soon as a matching view is created.
    pub fn creator(
        &self,
        views: impl ViewPatterns,
        callback: impl Fn(&mut View) + Send + Sync + 'static,
    ) {
        let callback: ViewCallback = Arc::new(callback);
        let mut creators = self.inner.creators.write().unwrap();
        for pattern in views.patterns() {
            creators.push((normalize_name(&pattern), callback.clone()));
        }
    }

    pub(crate) fn call_composers(&self, view: &mut View) {
        let composers = self.inner.composers.read().unwrap().clone();
        for (pattern, callback) in composers {
            if Str::is(&pattern, view.name()) {
                callback(view);
            }
        }
    }

    pub(crate) fn call_creators(&self, view: &mut View) {
        let creators = self.inner.creators.read().unwrap().clone();
        for (pattern, callback) in creators {
            if Str::is(&pattern, view.name()) {
                callback(view);
            }
        }
    }

    // ------------------------------------------------------------------
    // Locations
    // ------------------------------------------------------------------

    /// Add a location to the array of view locations.
    pub fn add_location(&self, location: impl Into<PathBuf>) {
        self.inner.finder.add_location(location);
    }

    /// Prepend a location to the array of view locations.
    pub fn prepend_location(&self, location: impl Into<PathBuf>) {
        self.inner.finder.prepend_location(location);
    }

    /// Add a new namespace: `add_namespace("mail", path)` makes `mail::layout` available.
    pub fn add_namespace(&self, namespace: &str, path: impl Into<PathBuf>) -> &Self {
        self.inner.finder.add_namespace(namespace, [path.into()]);
        self
    }

    /// Prepend a path to a namespace.
    pub fn prepend_namespace(&self, namespace: &str, path: impl Into<PathBuf>) -> &Self {
        self.inner
            .finder
            .prepend_namespace(namespace, [path.into()]);
        self
    }

    /// Replace a namespace's paths.
    pub fn replace_namespace(&self, namespace: &str, path: impl Into<PathBuf>) -> &Self {
        self.inner
            .finder
            .replace_namespace(namespace, [path.into()]);
        self
    }

    /// Register a view file extension to search for.
    pub fn add_extension(&self, extension: &str) {
        self.inner.finder.add_extension(extension);
    }

    /// Flush the cache of found views.
    pub fn flush_finder_cache(&self) {
        self.inner.finder.flush();
    }
}
