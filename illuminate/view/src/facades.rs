//! The `View` and `Blade` facades.

use std::path::PathBuf;

use illuminate_http::Request;
use illuminate_support::{Map, Result, Value};

use crate::compiler::BladeCompiler;
use crate::component::{Component, ComponentArgs};
use crate::factory::{Factory, IntoViewData, ViewPatterns};
use crate::value::ViewValue;

/// The `View` facade: resolves the view [`Factory`] from the container.
///
/// ```ignore
/// use illuminate_view::facades::View;
///
/// View::share("appName", "Laravel");
/// View::composer("profile", |view| { view.set("count", 42); });
///
/// let html = View::make("greeting", json!({"name": "James"})).render()?;
/// ```
pub struct View;

impl View {
    /// The underlying factory.
    pub fn factory() -> Factory {
        Factory::resolve()
    }

    /// Get a view instance.
    pub fn make(name: &str, data: impl IntoViewData) -> crate::View {
        Factory::resolve().make(name, data)
    }

    /// Get a view for a file on disk.
    pub fn file(path: impl Into<PathBuf>, data: impl IntoViewData) -> crate::View {
        Factory::resolve().file(path, data)
    }

    /// Get the first view that exists.
    pub fn first(names: &[&str], data: impl IntoViewData) -> Result<crate::View> {
        Factory::resolve().first(names, data)
    }

    /// Determine if a view exists.
    pub fn exists(name: &str) -> bool {
        Factory::resolve().exists(name)
    }

    /// Render an inline Blade template.
    pub fn render_inline(template: &str, data: impl IntoViewData) -> Result<String> {
        Factory::resolve().render_inline(template, data)
    }

    /// Share data with every view.
    pub fn share(key: &str, value: impl Into<ViewValue>) {
        Factory::resolve().share(key, value)
    }

    /// Share several pieces of data with every view.
    pub fn share_many(data: impl IntoViewData) {
        Factory::resolve().share_many(data)
    }

    /// Get a piece of shared data.
    pub fn shared(key: &str) -> Option<ViewValue> {
        Factory::resolve().shared(key)
    }

    /// Share data resolved from the current request with every view.
    pub fn share_resolver(
        resolver: impl Fn(&Request) -> Map<String, Value> + Send + Sync + 'static,
    ) {
        Factory::resolve().share_resolver(resolver)
    }

    /// Register a view composer.
    pub fn composer(
        views: impl ViewPatterns,
        callback: impl Fn(&mut crate::View) + Send + Sync + 'static,
    ) {
        Factory::resolve().composer(views, callback)
    }

    /// Register a view creator.
    pub fn creator(
        views: impl ViewPatterns,
        callback: impl Fn(&mut crate::View) + Send + Sync + 'static,
    ) {
        Factory::resolve().creator(views, callback)
    }

    /// Add a view namespace (`mail::layout`).
    pub fn add_namespace(namespace: &str, path: impl Into<PathBuf>) {
        Factory::resolve().add_namespace(namespace, path);
    }

    /// Replace a view namespace's paths.
    pub fn replace_namespace(namespace: &str, path: impl Into<PathBuf>) {
        Factory::resolve().replace_namespace(namespace, path);
    }

    /// Add a location to the view paths.
    pub fn add_location(path: impl Into<PathBuf>) {
        Factory::resolve().add_location(path)
    }

    /// Prepend a location to the view paths.
    pub fn prepend_location(path: impl Into<PathBuf>) {
        Factory::resolve().prepend_location(path)
    }
}

/// The `Blade` facade: resolves the [`BladeCompiler`] of the view factory.
///
/// ```ignore
/// use illuminate_view::facades::Blade;
///
/// Blade::directive("datetime", |args| Ok(format!("<time>{}</time>", args[0])));
/// Blade::if_("disk", |args| config("filesystems.default") == args[0].to_json());
/// Blade::function("route", |args| Ok(url_for(args)));
///
/// let html = Blade::render("Hello, {{ $name }}", json!({"name": "Julian Bashir"}))?;
/// ```
pub struct Blade;

impl Blade {
    /// The Blade compiler.
    pub fn compiler() -> BladeCompiler {
        Factory::resolve().blade().clone()
    }

    /// Render a Blade template string.
    pub fn render(template: &str, data: impl IntoViewData) -> Result<String> {
        Factory::resolve().render_inline(template, data)
    }

    /// Register a custom directive.
    pub fn directive(
        name: &str,
        handler: impl Fn(&[ViewValue]) -> Result<String> + Send + Sync + 'static,
    ) {
        Self::compiler().directive(name, handler)
    }

    /// Register a custom conditional (`Blade::if` in Laravel).
    pub fn if_(name: &str, condition: impl Fn(&[ViewValue]) -> bool + Send + Sync + 'static) {
        Self::compiler().if_(name, condition)
    }

    /// Check a custom conditional.
    pub fn check(name: &str, args: &[ViewValue]) -> bool {
        Self::compiler().check(name, args)
    }

    /// Register a function callable from templates.
    pub fn function(
        name: &str,
        function: impl Fn(&[ViewValue]) -> Result<ViewValue> + Send + Sync + 'static,
    ) {
        Self::compiler().function(name, function)
    }

    /// Register an already shared function.
    pub fn function_arc(name: &str, function: crate::value::ViewFunction) {
        Self::compiler().function_arc(name, function)
    }

    /// Register a class-based component.
    pub fn component<C, F>(alias: &str, factory: F)
    where
        C: Component,
        F: Fn(&mut ComponentArgs) -> Result<C> + Send + Sync + 'static,
    {
        Self::compiler().component(alias, factory)
    }

    /// Register a directory of anonymous components.
    pub fn anonymous_component_path(path: impl Into<PathBuf>, prefix: Option<&str>) {
        Self::compiler().anonymous_component_path(path, prefix)
    }

    /// Register a view directory of anonymous components under a prefix.
    pub fn anonymous_component_namespace(directory: &str, prefix: &str) {
        Self::compiler().anonymous_component_namespace(directory, prefix)
    }

    /// Register a custom echo handler for objects of type `T`.
    pub fn stringable<T: crate::value::ViewObject>(
        handler: impl Fn(&T) -> String + Send + Sync + 'static,
    ) {
        Self::compiler().stringable(handler)
    }

    /// Stop double-encoding HTML entities.
    pub fn without_double_encoding() {
        Self::compiler().without_double_encoding()
    }

    /// Double-encode HTML entities (the default).
    pub fn with_double_encoding() {
        Self::compiler().with_double_encoding()
    }
}
