//! The Blade compiler: parses templates (caching them until the file
//! changes) and holds everything registered with Blade — custom
//! directives, conditionals, functions and components.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use illuminate_support::Result;

use crate::component::{Component, ComponentArgs};
use crate::registry::{AnonymousComponentPath, Registry};
use crate::template::{self, Node, Template};
use crate::value::{ViewFunction, ViewValue};

/// How many inline templates are kept compiled.
const INLINE_CACHE_LIMIT: usize = 1024;

struct CachedTemplate {
    modified: Option<SystemTime>,
    len: u64,
    template: Arc<Template>,
}

struct Inner {
    registry: RwLock<Arc<Registry>>,
    files: RwLock<HashMap<PathBuf, CachedTemplate>>,
    inline: RwLock<HashMap<Arc<str>, Arc<Template>>>,
}

/// The Blade compiler.
///
/// Templates are parsed into a tree once and cached by path; a template is
/// re-parsed only when its file changes. `BladeCompiler` is a cheap,
/// cloneable handle — every clone shares the same registrations and cache.
///
/// ```
/// use illuminate_view::{Factory, ViewData};
///
/// let factory = Factory::new(Vec::<String>::new());
/// let blade = factory.blade();
///
/// blade.directive("datetime", |args| {
///     Ok(format!("<time>{}</time>", args[0]))
/// });
/// blade.if_("admin", |args| args.first().is_some_and(|role| role.to_string() == "admin"));
///
/// let html = factory
///     .render_inline("@datetime('2024-01-01') @admin('admin') Hi admin! @endadmin", ViewData::new())
///     .unwrap();
///
/// assert_eq!(html, "<time>2024-01-01</time>  Hi admin! ");
/// ```
#[derive(Clone)]
pub struct BladeCompiler {
    inner: Arc<Inner>,
}

impl Default for BladeCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for BladeCompiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BladeCompiler").finish_non_exhaustive()
    }
}

impl BladeCompiler {
    /// Create a new compiler.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                registry: RwLock::new(Arc::new(Registry::default())),
                files: RwLock::new(HashMap::new()),
                inline: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// A snapshot of everything registered.
    pub(crate) fn registry(&self) -> Arc<Registry> {
        self.inner.registry.read().unwrap().clone()
    }

    fn update(&self, flush: bool, change: impl FnOnce(&mut Registry)) {
        {
            let mut guard = self.inner.registry.write().unwrap();
            let mut next = (**guard).clone();
            change(&mut next);
            *guard = Arc::new(next);
        }
        if flush {
            self.flush_cache();
        }
    }

    // ------------------------------------------------------------------
    // Extending Blade
    // ------------------------------------------------------------------

    /// Register a custom directive. The handler receives the directive's
    /// evaluated arguments and returns the HTML to output (unescaped).
    ///
    /// Directive names may only contain letters, numbers and underscores.
    pub fn directive(
        &self,
        name: &str,
        handler: impl Fn(&[ViewValue]) -> Result<String> + Send + Sync + 'static,
    ) {
        assert!(
            is_valid_directive_name(name),
            "The directive name [{name}] is not valid. Directive names must only contain alphanumeric characters and underscores."
        );
        let name = name.to_string();
        self.update(true, move |r| {
            r.directives.insert(name, Arc::new(handler));
        });
    }

    /// Register a custom conditional: `@name(...)`, `@elsename(...)`,
    /// `@unlessname(...)` and `@endname`. (`if` is a Rust keyword, hence the
    /// trailing underscore.)
    pub fn if_(
        &self,
        name: &str,
        condition: impl Fn(&[ViewValue]) -> bool + Send + Sync + 'static,
    ) {
        assert!(
            is_valid_directive_name(name),
            "The directive name [{name}] is not valid. Directive names must only contain alphanumeric characters and underscores."
        );
        let name = name.to_string();
        self.update(true, move |r| {
            r.conditions.insert(name, Arc::new(condition));
        });
    }

    /// Check a custom conditional registered with [`BladeCompiler::if_`].
    pub fn check(&self, name: &str, args: &[ViewValue]) -> bool {
        self.registry()
            .conditions
            .get(name)
            .is_some_and(|condition| condition(args))
    }

    /// Register a function callable from templates, or override a built-in
    /// one. Names like `"Route::has"` register static calls.
    pub fn function(
        &self,
        name: &str,
        function: impl Fn(&[ViewValue]) -> Result<ViewValue> + Send + Sync + 'static,
    ) {
        self.function_arc(name, Arc::new(function));
    }

    /// Register an already shared function.
    pub fn function_arc(&self, name: &str, function: ViewFunction) {
        let name = name.to_string();
        self.update(false, move |r| {
            r.functions.insert(name, function);
        });
    }

    /// Determine if a function has been registered.
    pub fn has_function(&self, name: &str) -> bool {
        self.registry().functions.contains_key(name)
    }

    /// Register a class-based component under a tag alias: `<x-{alias}>`.
    ///
    /// The factory builds the component from the tag's arguments, taking
    /// the ones it needs; the rest become the component's `$attributes`.
    pub fn component<C, F>(&self, alias: &str, factory: F)
    where
        C: Component,
        F: Fn(&mut ComponentArgs) -> Result<C> + Send + Sync + 'static,
    {
        let alias = alias.to_string();
        let factory = Arc::new(
            move |args: &mut ComponentArgs| -> Result<Arc<dyn Component>> {
                Ok(Arc::new(factory(args)?))
            },
        );
        self.update(false, move |r| {
            r.components.insert(alias, factory);
        });
    }

    /// Register a directory of anonymous components, optionally under a
    /// prefix (`<x-dashboard::panel />`).
    pub fn anonymous_component_path(&self, path: impl Into<PathBuf>, prefix: Option<&str>) {
        let path = AnonymousComponentPath {
            path: path.into(),
            prefix: prefix.map(str::to_string),
        };
        self.update(false, move |r| r.anonymous_paths.push(path));
    }

    /// Register a view directory (in dot notation) holding anonymous
    /// components under a prefix: `anonymous_component_namespace("flights.components", "flights")`.
    pub fn anonymous_component_namespace(&self, directory: &str, prefix: &str) {
        let directory = directory
            .replace('/', ".")
            .trim_matches(['.', ' '])
            .to_string();
        let prefix = prefix.to_string();
        self.update(false, move |r| {
            r.anonymous_namespaces.insert(prefix, directory);
        });
    }

    /// Register a custom echo handler: how objects of type `T` are turned
    /// into strings when echoed.
    ///
    /// ```
    /// use illuminate_view::{Factory, ViewObject, ViewValue, data};
    ///
    /// struct Money(i64);
    /// impl ViewObject for Money {}
    ///
    /// let factory = Factory::new(Vec::<String>::new());
    /// factory.blade().stringable(|money: &Money| format!("${:.2}", money.0 as f64 / 100.0));
    ///
    /// let html = factory.render_inline("Cost: {{ $money }}", data([("money", ViewValue::object(Money(1999)))])).unwrap();
    /// assert_eq!(html, "Cost: $19.99");
    /// ```
    pub fn stringable<T: crate::value::ViewObject>(
        &self,
        handler: impl Fn(&T) -> String + Send + Sync + 'static,
    ) {
        let handler: crate::registry::EchoHandler =
            Arc::new(move |value: &ViewValue| value.downcast_ref::<T>().map(&handler));
        self.update(false, move |r| r.echo_handlers.push(handler));
    }

    /// Stop double-encoding HTML entities in `{{ }}` echoes.
    pub fn without_double_encoding(&self) {
        self.update(false, |r| r.double_encode = false);
    }

    /// Double-encode HTML entities in `{{ }}` echoes (the default).
    pub fn with_double_encoding(&self) {
        self.update(false, |r| r.double_encode = true);
    }

    /// The names of the registered custom directives.
    pub fn custom_directives(&self) -> Vec<String> {
        self.registry().directives.keys().cloned().collect()
    }

    // ------------------------------------------------------------------
    // Compiling
    // ------------------------------------------------------------------

    /// Check that a template compiles, reporting the first problem.
    pub fn check_syntax(&self, template: &str) -> Result<()> {
        template::parse(template, &self.registry())?;
        Ok(())
    }

    /// Compile a template string (cached by its contents).
    pub(crate) fn compile_string(&self, source: &str) -> Result<Arc<Template>> {
        if let Some(template) = self.inner.inline.read().unwrap().get(source) {
            return Ok(template.clone());
        }
        let template = Arc::new(template::parse(source, &self.registry())?);
        let mut cache = self.inner.inline.write().unwrap();
        if cache.len() >= INLINE_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(source.into(), template.clone());
        Ok(template)
    }

    /// Compile the template at `path`, re-using the cached tree unless the
    /// file has changed. Plain `.html` (and `.css`) files are served as-is.
    pub(crate) fn compile_file(&self, path: &Path) -> Result<Arc<Template>> {
        let metadata = std::fs::metadata(path).map_err(|e| {
            crate::exception::InvalidArgumentException::new(format!(
                "File does not exist at path {}: {e}",
                path.display()
            ))
        })?;
        let modified = metadata.modified().ok();
        let len = metadata.len();
        if let Some(cached) = self.inner.files.read().unwrap().get(path)
            && cached.modified == modified
            && cached.len == len
            && modified.is_some()
        {
            return Ok(cached.template.clone());
        }
        let source = std::fs::read_to_string(path)?;
        let template = if is_blade(path) {
            Arc::new(template::parse(&source, &self.registry())?)
        } else {
            Arc::new(Template {
                nodes: vec![Node::Text(source)],
                extends: Vec::new(),
            })
        };
        self.inner.files.write().unwrap().insert(
            path.to_path_buf(),
            CachedTemplate {
                modified,
                len,
                template: template.clone(),
            },
        );
        Ok(template)
    }

    /// Forget every compiled template (Laravel's `view:clear`).
    pub fn flush_cache(&self) {
        self.inner.files.write().unwrap().clear();
        self.inner.inline.write().unwrap().clear();
    }
}

fn is_valid_directive_name(name: &str) -> bool {
    let mut parts = name.splitn(2, "::");
    let valid =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    parts.next().is_some_and(valid) && parts.next().is_none_or(valid)
}

/// Determine if a file is a Blade template (rather than a plain file).
pub(crate) fn is_blade(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    name.ends_with(".blade.html") || name.ends_with(".blade.php") || name.ends_with(".php")
}
