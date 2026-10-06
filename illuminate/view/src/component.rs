//! Components: class-based components, slots, and the arguments a
//! component tag is given.

use std::sync::Arc;

use illuminate_support::{Result, Str, Value};
use indexmap::IndexMap;

use crate::attributes::ComponentAttributeBag;
use crate::functions::arg;
use crate::value::{ViewData, ViewObject, ViewValue};

/// What a component renders: a view, or an inline Blade template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentView {
    /// The name of a view (`"components.alert"`).
    View(String),
    /// An inline Blade template.
    Inline(String),
}

impl ComponentView {
    /// Render the named view.
    pub fn view(name: impl Into<String>) -> Self {
        ComponentView::View(name.into())
    }

    /// Render an inline Blade template.
    pub fn inline(template: impl Into<String>) -> Self {
        ComponentView::Inline(template.into())
    }
}

/// A class-based Blade component.
///
/// Components are registered with `Blade::component()` along with a
/// factory that builds them from the tag's attributes. Attributes the
/// factory [`take`](ComponentArgs::take)s become the component's data (like
/// constructor arguments in Laravel); everything else lands in the
/// `$attributes` bag.
///
/// ```
/// use illuminate_view::{Component, ComponentArgs, ComponentView, Factory, ViewData, ViewValue};
///
/// struct Alert { kind: String }
///
/// impl Component for Alert {
///     fn render(&self) -> ComponentView {
///         ComponentView::inline(r#"<div {{ $attributes->merge(['class' => 'alert alert-'.$type]) }}>{{ $slot }}</div>"#)
///     }
///
///     fn data(&self) -> ViewData {
///         illuminate_view::data([("type", self.kind.as_str())])
///     }
/// }
///
/// let factory = Factory::new(Vec::<String>::new());
/// factory.blade().component("alert", |args: &mut ComponentArgs| {
///     Ok(Alert { kind: args.take_string("type").unwrap_or_else(|| "info".into()) })
/// });
///
/// let html = factory
///     .render_inline(r#"<x-alert type="error" class="mb-4">Whoops!</x-alert>"#, ViewData::new())
///     .unwrap();
///
/// assert_eq!(html, r#"<div class="alert alert-error mb-4">Whoops!</div>"#);
/// ```
pub trait Component: Send + Sync + 'static {
    /// The view (or inline template) that represents the component.
    fn render(&self) -> ComponentView;

    /// The data exposed to the component's template (Laravel exposes a
    /// component's public properties and methods). Closures become callable
    /// variables: `{{ $isSelected($value) ? 'selected' : '' }}`.
    fn data(&self) -> ViewData {
        ViewData::new()
    }

    /// Whether the component should be rendered at all.
    fn should_render(&self) -> bool {
        true
    }
}

/// The arguments given to a component tag.
///
/// Values are the evaluated attribute values (bound `:attr="..."` values
/// keep their type; plain attributes are strings; boolean attributes are
/// `true`). Names may be looked up in either kebab or camel case.
#[derive(Clone, Debug, Default)]
pub struct ComponentArgs {
    pub(crate) values: IndexMap<String, (ViewValue, bool)>,
    pub(crate) name: String,
}

impl ComponentArgs {
    /// The component's name (what follows `x-` in the tag).
    pub fn component_name(&self) -> &str {
        &self.name
    }

    fn find(&self, name: &str) -> Option<String> {
        if self.values.contains_key(name) {
            return Some(name.to_string());
        }
        let kebab = Str::kebab(name);
        if self.values.contains_key(&kebab) {
            return Some(kebab);
        }
        self.values.keys().find(|k| Str::camel(k) == name).cloned()
    }

    /// Take an argument, removing it from the attribute bag.
    pub fn take(&mut self, name: &str) -> Option<ViewValue> {
        let key = self.find(name)?;
        self.values.shift_remove(&key).map(|(value, _)| value)
    }

    /// Take an argument as a string.
    pub fn take_string(&mut self, name: &str) -> Option<String> {
        self.take(name).map(|v| v.to_string_lossy())
    }

    /// Take an argument as a boolean.
    pub fn take_bool(&mut self, name: &str) -> Option<bool> {
        self.take(name).map(|v| match &v {
            ViewValue::Str(s) => !matches!(
                s.to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "off" | "no"
            ),
            other => other.truthy(),
        })
    }

    /// Take an argument as an integer.
    pub fn take_i64(&mut self, name: &str) -> Option<i64> {
        self.take(name).and_then(|v| v.as_i64())
    }

    /// Peek at an argument without taking it.
    pub fn get(&self, name: &str) -> Option<&ViewValue> {
        self.find(name)
            .and_then(|key| self.values.get(&key))
            .map(|(value, _)| value)
    }

    /// Determine if an argument was given.
    pub fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The remaining attributes, as they'll appear in `$attributes`.
    pub(crate) fn into_attribute_bag(self) -> ComponentAttributeBag {
        let mut bag = ComponentAttributeBag::new();
        bag.set_attributes(
            self.values
                .into_iter()
                .map(|(key, (value, bound))| (key, if bound { sanitize(value) } else { value }))
                .collect(),
        );
        bag
    }
}

/// Laravel escapes bound string values placed in the attribute bag.
pub(crate) fn sanitize(value: ViewValue) -> ViewValue {
    match &value {
        ViewValue::Str(s) => ViewValue::from(illuminate_support::e(&**s)),
        ViewValue::Object(object)
            if object.downcast_ref::<ComponentAttributeBag>().is_none()
                && object.to_html().is_none()
                && object.to_string_value().is_some() =>
        {
            ViewValue::from(illuminate_support::e(
                object.to_string_value().unwrap_or_default(),
            ))
        }
        _ => value,
    }
}

/// The content passed to a component slot.
#[derive(Clone, Debug, Default)]
pub struct ComponentSlot {
    contents: String,
    attributes: ComponentAttributeBag,
}

impl ComponentSlot {
    /// Create a slot with the given HTML contents.
    pub fn new(contents: impl Into<String>, attributes: ComponentAttributeBag) -> Self {
        Self {
            contents: contents.into(),
            attributes,
        }
    }

    /// The slot's HTML.
    pub fn to_html(&self) -> &str {
        &self.contents
    }

    /// The slot's attributes.
    pub fn attributes(&self) -> &ComponentAttributeBag {
        &self.attributes
    }

    /// Determine if the slot is empty.
    pub fn is_empty(&self) -> bool {
        self.contents.is_empty()
    }

    /// Determine if the slot has content other than HTML comments and whitespace.
    pub fn has_actual_content(&self) -> bool {
        let mut rest = self.contents.as_str();
        let mut stripped = String::new();
        while let Some(start) = rest.find("<!--") {
            stripped.push_str(&rest[..start]);
            match rest[start..].find("-->") {
                Some(end) => rest = &rest[start + end + 3..],
                None => {
                    rest = "";
                    break;
                }
            }
        }
        stripped.push_str(rest);
        !stripped.trim().is_empty()
    }
}

impl ViewObject for ComponentSlot {
    fn class_name(&self) -> &str {
        "Illuminate\\View\\ComponentSlot"
    }

    fn get(&self, property: &str) -> Option<ViewValue> {
        (property == "attributes").then(|| ViewValue::object(self.attributes.clone()))
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        Some(Ok(match method {
            "toHtml" | "__toString" => ViewValue::html(self.contents.clone()),
            "isEmpty" => ViewValue::Bool(self.is_empty()),
            "isNotEmpty" => ViewValue::Bool(!self.is_empty()),
            "hasActualContent" => match arg(args, 0) {
                ViewValue::Null => ViewValue::Bool(self.has_actual_content()),
                callback => {
                    return Some(
                        crate::expr::eval::call_callable(
                            callback,
                            &[ViewValue::from(self.contents.as_str())],
                            &Arc::new(crate::registry::Registry::default()),
                        )
                        .map(|v| ViewValue::Bool(!v.to_string_lossy().is_empty())),
                    );
                }
            },
            _ => return None,
        }))
    }

    fn to_html(&self) -> Option<String> {
        Some(self.contents.clone())
    }

    fn to_json(&self) -> Value {
        Value::String(self.contents.clone())
    }
}

/// The `$component` variable available inside slots: exposes the
/// component's data as properties and its closures as methods.
pub(crate) struct ComponentObject {
    pub(crate) data: Arc<ViewData>,
}

impl ViewObject for ComponentObject {
    fn class_name(&self) -> &str {
        "Illuminate\\View\\Component"
    }

    fn get(&self, property: &str) -> Option<ViewValue> {
        self.data.get(property).cloned()
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        match self.data.get(method)? {
            ViewValue::Closure(closure) => Some(closure.call(args)),
            other => Some(Ok(other.clone())),
        }
    }

    fn to_json(&self) -> Value {
        Value::Object(
            self.data
                .iter()
                .map(|(k, v)| (k.clone(), v.to_json()))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_are_found_by_kebab_or_camel_case() {
        let mut args = ComponentArgs::default();
        args.values
            .insert("alert-type".into(), (ViewValue::from("danger"), false));
        args.values
            .insert("class".into(), (ViewValue::from("mt-4"), false));
        assert!(args.has("alertType"));
        assert_eq!(args.take_string("alertType").as_deref(), Some("danger"));
        assert!(!args.has("alert-type"));
        assert_eq!(args.into_attribute_bag().to_html(), r#"class="mt-4""#);
    }

    #[test]
    fn slots_detect_actual_content() {
        let slot = ComponentSlot::new("<!-- nothing -->  ", ComponentAttributeBag::new());
        assert!(!slot.is_empty());
        assert!(!slot.has_actual_content());
        assert!(ComponentSlot::new("<p>hi</p>", ComponentAttributeBag::new()).has_actual_content());
    }
}
