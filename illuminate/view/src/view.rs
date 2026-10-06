//! A view instance: a template plus the data it renders with.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use illuminate_http::{IntoResponse, Response, render_exception};
use illuminate_support::{MessageBag, Result, Value};

use crate::factory::{Factory, IntoViewData};
use crate::objects::ViewErrorBag;
use crate::render::{Renderer, ViewContext};
use crate::template::Template;
use crate::value::{ViewData, ViewValue};

/// Where a view's template comes from.
#[derive(Clone)]
enum Source {
    /// A named view, found when it renders.
    Named,
    /// A file on disk.
    File(PathBuf),
    /// An inline template string.
    Inline(Arc<str>),
    /// An already compiled template.
    Compiled(Arc<Template>),
}

/// Information about a rendered view, attached to responses as an extension
/// so tests can assert on it (`assertViewIs`, `assertViewHas`).
#[derive(Clone, Debug)]
pub struct ViewInfo {
    /// The view's name.
    pub name: String,
    /// The view's data (after composers ran) as JSON.
    pub data: Value,
    /// The view's data as view values.
    pub values: ViewData,
}

impl ViewInfo {
    /// Get a piece of the view's data.
    pub fn get(&self, key: &str) -> Option<&ViewValue> {
        self.values.get(key)
    }

    /// Determine if the view has a piece of data.
    pub fn has(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }
}

/// A view, ready to be rendered.
///
/// ```
/// use illuminate_view::{Factory, ViewValue};
///
/// let factory = Factory::new(Vec::<String>::new());
/// let view = factory
///     .inline("{{ $name }} is an {{ $occupation }}", ())
///     .with("name", "Victoria")
///     .with("occupation", "Astronaut");
///
/// assert_eq!(view.render().unwrap(), "Victoria is an Astronaut");
/// ```
#[derive(Clone)]
pub struct View {
    factory: Factory,
    name: String,
    source: Source,
    data: ViewData,
}

impl std::fmt::Debug for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View").field("name", &self.name).field("data", &self.data).finish()
    }
}

impl View {
    pub(crate) fn named(factory: &Factory, name: &str, data: ViewData) -> Self {
        Self { factory: factory.clone(), name: name.to_string(), source: Source::Named, data }
    }

    pub(crate) fn file(factory: &Factory, path: PathBuf, data: ViewData) -> Self {
        Self { factory: factory.clone(), name: path.display().to_string(), source: Source::File(path), data }
    }

    pub(crate) fn inline(factory: &Factory, template: &str, data: ViewData) -> Self {
        Self { factory: factory.clone(), name: "__inline".into(), source: Source::Inline(template.into()), data }
    }

    pub(crate) fn inline_template(factory: &Factory, template: Arc<Template>, data: ViewData) -> Self {
        Self { factory: factory.clone(), name: "__components::inline".into(), source: Source::Compiled(template), data }
    }

    /// The name of the view.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The view's data.
    pub fn data(&self) -> &ViewData {
        &self.data
    }

    /// The view's data as JSON.
    pub fn data_json(&self) -> Value {
        Value::Object(self.data.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
    }

    /// Get a piece of the view's data.
    pub fn get(&self, key: &str) -> Option<&ViewValue> {
        self.data.get(key)
    }

    /// Determine if the view has a piece of data.
    pub fn has(&self, key: &str) -> bool {
        self.data.contains_key(key)
    }

    /// The path of the view's template, when it lives on disk.
    pub fn path(&self) -> Option<PathBuf> {
        match &self.source {
            Source::Named => self.factory.find(&self.name).ok(),
            Source::File(path) => Some(path.clone()),
            _ => None,
        }
    }

    /// The factory the view belongs to.
    pub fn factory(&self) -> &Factory {
        &self.factory
    }

    /// Add a piece of data to the view.
    pub fn with(mut self, key: &str, value: impl Into<ViewValue>) -> Self {
        self.set(key, value);
        self
    }

    /// Add data to the view from a JSON object, a serializable struct or
    /// [`ViewData`].
    pub fn with_data(mut self, data: impl IntoViewData) -> Self {
        self.merge(data);
        self
    }

    /// Add a piece of data to a view you hold by reference (in composers).
    pub fn set(&mut self, key: &str, value: impl Into<ViewValue>) -> &mut Self {
        self.data.insert(key.to_string(), value.into());
        self
    }

    /// Merge data into a view you hold by reference.
    pub fn merge(&mut self, data: impl IntoViewData) -> &mut Self {
        self.data.extend(data.into_view_data());
        self
    }

    /// Remove a piece of data.
    pub fn forget(&mut self, key: &str) -> &mut Self {
        self.data.shift_remove(key);
        self
    }

    /// Add validation errors to the view (`$errors`).
    pub fn with_errors(mut self, errors: impl Into<MessageBag>) -> Self {
        self.data.insert("errors".into(), ViewValue::object(ViewErrorBag::new().put("default", errors.into())));
        self
    }

    pub(crate) fn context(&self) -> ViewContext {
        ViewContext {
            name: self.name.as_str().into(),
            path: self.path().map(|p| Arc::<Path>::from(p.as_path())),
        }
    }

    /// The compiled template for this view.
    pub(crate) fn template(&self, _registry: &crate::registry::Registry) -> Result<Arc<Template>> {
        let blade = self.factory.blade();
        match &self.source {
            Source::Named => {
                let path = self.factory.find(&self.name)?;
                match blade.compile_file(&path) {
                    Ok(template) => Ok(template),
                    Err(error) if !path.exists() => {
                        self.factory.flush_finder_cache();
                        let path = self.factory.find(&self.name).map_err(|_| error)?;
                        blade.compile_file(&path)
                    }
                    Err(error) => Err(error),
                }
            }
            Source::File(path) => blade.compile_file(path),
            Source::Inline(source) => blade.compile_string(source),
            Source::Compiled(template) => Ok(template.clone()),
        }
    }

    /// Render the view.
    pub fn render(&self) -> Result<String> {
        self.render_with_data().map(|(html, _)| html)
    }

    /// Render the view, returning the HTML and the data it was rendered with.
    pub(crate) fn render_with_data(&self) -> Result<(String, ViewData)> {
        let mut renderer = Renderer::new(&self.factory);
        renderer.render_view(self)
    }

    /// Render the view and return only the named `@fragment`.
    pub fn fragment(&self, name: &str) -> Result<String> {
        let mut renderer = Renderer::new(&self.factory);
        renderer.render_view(self)?;
        renderer
            .fragments()
            .get(name)
            .cloned()
            .ok_or_else(|| crate::exception::InvalidArgumentException::new(format!("Fragment [{name}] not found.")).into())
    }

    /// Render the view and return the named fragments, concatenated.
    pub fn fragments(&self, names: &[&str]) -> Result<String> {
        let mut renderer = Renderer::new(&self.factory);
        renderer.render_view(self)?;
        let mut out = String::new();
        for name in names {
            match renderer.fragments().get(*name) {
                Some(content) => out.push_str(content),
                None => {
                    return Err(crate::exception::InvalidArgumentException::new(format!("Fragment [{name}] not found.")).into());
                }
            }
        }
        Ok(out)
    }

    /// Render the named fragment if the condition is true, otherwise the whole view.
    pub fn fragment_if(&self, condition: bool, name: &str) -> Result<String> {
        if condition { self.fragment(name) } else { self.render() }
    }

    /// Render the named fragments if the condition is true, otherwise the whole view.
    pub fn fragments_if(&self, condition: bool, names: &[&str]) -> Result<String> {
        if condition { self.fragments(names) } else { self.render() }
    }
}

/// Views render into HTML responses. Rendering errors are handed to the
/// exception handler, and the response carries a [`ViewInfo`] extension.
impl IntoResponse for View {
    fn into_response(self) -> Response {
        match self.render_with_data() {
            Ok((html, data)) => {
                let mut response = Response::new(html);
                let json = Value::Object(data.iter().map(|(k, v)| (k.clone(), v.to_json())).collect());
                response.set_extension(Arc::new(ViewInfo { name: self.name.clone(), data: json, values: data }));
                response
            }
            Err(error) => render_exception(error),
        }
    }
}
