//! The component attribute bag: `{{ $attributes->merge(['class' => 'alert']) }}`.

use std::sync::Arc;

use illuminate_support::{Result, Str, Value};
use indexmap::IndexMap;

use crate::exception::BadMethodCallException;
use crate::expr::eval::call_callable;
use crate::functions::{arg, bool_arg, int_arg, str_arg};
use crate::php;
use crate::registry::Registry;
use crate::value::{ArrayKey, ViewObject, ViewValue};

/// The attributes passed to a component that aren't part of its data.
///
/// ```
/// use illuminate_view::{ComponentAttributeBag, ViewValue};
///
/// let attributes = ComponentAttributeBag::from_pairs([
///     ("class", ViewValue::from("mt-4")),
///     ("disabled", ViewValue::Bool(true)),
/// ]);
///
/// let merged = attributes.merge([("class", ViewValue::from("btn")), ("type", "button".into())], true);
/// assert_eq!(merged.to_html(), r#"class="btn mt-4" type="button" disabled="disabled""#);
/// ```
#[derive(Clone, Debug, Default)]
pub struct ComponentAttributeBag {
    attributes: IndexMap<String, ViewValue>,
}

/// A default value that is prepended to (rather than replaced by) the
/// injected value: `$attributes->prepends('profile-controller')`.
#[derive(Clone, Debug)]
pub struct AppendableAttributeValue(pub ViewValue);

impl ViewObject for AppendableAttributeValue {
    fn class_name(&self) -> &str {
        "Illuminate\\View\\AppendableAttributeValue"
    }

    fn to_string_value(&self) -> Option<String> {
        Some(self.0.to_string_lossy())
    }

    fn to_json(&self) -> Value {
        self.0.to_json()
    }
}

impl ComponentAttributeBag {
    /// Create an empty attribute bag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a bag from name / value pairs.
    pub fn from_pairs<K: Into<String>>(pairs: impl IntoIterator<Item = (K, ViewValue)>) -> Self {
        let mut bag = Self::new();
        bag.set_attributes(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect());
        bag
    }

    /// Replace the attributes. An `attributes` entry holding another bag is
    /// merged in (this is how `<x-button {{ $attributes }}>` forwards them).
    pub fn set_attributes(&mut self, mut attributes: IndexMap<String, ViewValue>) {
        if let Some(ViewValue::Object(parent)) = attributes.get("attributes") {
            if let Some(parent) = parent.downcast_ref::<ComponentAttributeBag>() {
                let parent = parent.clone();
                attributes.shift_remove("attributes");
                attributes = parent.merge(attributes, false).attributes;
            }
        }
        self.attributes = attributes;
    }

    /// All of the attributes.
    pub fn all(&self) -> &IndexMap<String, ViewValue> {
        &self.attributes
    }

    /// Get an attribute's value.
    pub fn get(&self, key: &str) -> Option<&ViewValue> {
        self.attributes.get(key)
    }

    /// Determine if all of the given attributes are present.
    pub fn has(&self, key: &str) -> bool {
        self.attributes.contains_key(key)
    }

    /// Set an attribute.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<ViewValue>) {
        self.attributes.insert(key.into(), value.into());
    }

    /// Remove an attribute.
    pub fn remove(&mut self, key: &str) -> Option<ViewValue> {
        self.attributes.shift_remove(key)
    }

    /// The number of attributes.
    pub fn len(&self) -> usize {
        self.attributes.len()
    }

    /// Determine if the rendered bag is empty.
    pub fn is_empty(&self) -> bool {
        self.to_html().trim().is_empty()
    }

    /// Only the given attributes.
    pub fn only(&self, keys: &[String]) -> Self {
        Self {
            attributes: self
                .attributes
                .iter()
                .filter(|(k, _)| keys.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// All but the given attributes.
    pub fn except(&self, keys: &[String]) -> Self {
        Self {
            attributes: self
                .attributes
                .iter()
                .filter(|(k, _)| !keys.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// Only the attributes whose names start with one of the given strings.
    pub fn where_starts_with(&self, needles: &[String]) -> Self {
        self.filter_keys(|k| needles.iter().any(|n| !n.is_empty() && k.starts_with(n.as_str())))
    }

    /// Only the attributes whose names don't start with any of the given strings.
    pub fn where_doesnt_start_with(&self, needles: &[String]) -> Self {
        self.filter_keys(|k| !needles.iter().any(|n| !n.is_empty() && k.starts_with(n.as_str())))
    }

    fn filter_keys(&self, keep: impl Fn(&str) -> bool) -> Self {
        Self {
            attributes: self
                .attributes
                .iter()
                .filter(|(k, _)| keep(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// Merge default attributes into the bag. `class` and `style` values are
    /// appended to the defaults; any other attribute overrides its default.
    pub fn merge<K: Into<String>>(&self, defaults: impl IntoIterator<Item = (K, ViewValue)>, escape: bool) -> Self {
        let defaults: IndexMap<String, ViewValue> = defaults
            .into_iter()
            .map(|(k, v)| {
                let v = if should_escape(escape, &v) { ViewValue::from(illuminate_support::e(v.to_string_lossy())) } else { v };
                (k.into(), v)
            })
            .collect();
        let mut merged = IndexMap::with_capacity(defaults.len() + self.attributes.len());
        for (key, value) in &defaults {
            let value = match value.downcast_ref::<AppendableAttributeValue>() {
                Some(appendable) if !self.attributes.contains_key(key) => resolve_appendable(appendable, escape),
                _ => value.clone(),
            };
            merged.insert(key.clone(), value);
        }
        let is_appendable = |key: &str| {
            key == "class"
                || key == "style"
                || defaults.get(key).is_some_and(|d| d.downcast_ref::<AppendableAttributeValue>().is_some())
        };
        // Appendable attributes come first, like Laravel's partition().
        let ordered = self
            .attributes
            .iter()
            .filter(|(k, _)| is_appendable(k))
            .chain(self.attributes.iter().filter(|(k, _)| !is_appendable(k)));
        for (key, value) in ordered {
            let appendable_default = defaults.get(key).and_then(|d| d.downcast_ref::<AppendableAttributeValue>());
            if is_appendable(key) {
                let default_value = match appendable_default {
                    Some(appendable) => resolve_appendable(appendable, escape),
                    None => defaults.get(key).cloned().unwrap_or_else(|| ViewValue::from("")),
                };
                let mut value = value.clone();
                let mut default_value = default_value;
                if key == "style" {
                    value = ViewValue::from(Str::finish(&value.to_string_lossy(), ";"));
                    if let ViewValue::Str(s) = &default_value {
                        if !s.is_empty() {
                            default_value = ViewValue::from(Str::finish(s, ";"));
                        }
                    }
                }
                let mut parts: Vec<String> = Vec::new();
                for part in [default_value, value] {
                    if part.truthy() {
                        let text = part.to_string_lossy();
                        if !parts.contains(&text) {
                            parts.push(text);
                        }
                    }
                }
                merged.insert(key.clone(), ViewValue::from(parts.join(" ")));
            } else {
                merged.insert(key.clone(), value.clone());
            }
        }
        Self { attributes: merged }
    }

    /// Conditionally merge classes: `$attributes->class(['p-4', 'bg-red' => $hasError])`.
    pub fn class(&self, classes: &ViewValue) -> Self {
        self.merge([("class", ViewValue::from(php::css_classes(&wrap(classes))))], true)
    }

    /// Conditionally merge styles.
    pub fn style(&self, styles: &ViewValue) -> Self {
        self.merge([("style", ViewValue::from(php::css_styles(&wrap(styles))))], true)
    }

    /// Render the attributes as HTML: `class="mt-4" disabled="disabled"`.
    pub fn to_html(&self) -> String {
        let mut out = String::new();
        for (key, value) in &self.attributes {
            let value = match value {
                ViewValue::Bool(false) | ViewValue::Null => continue,
                ViewValue::Bool(true) => {
                    if key == "x-data" || key.starts_with("wire:") {
                        String::new()
                    } else {
                        key.clone()
                    }
                }
                other => other.to_string_lossy(),
            };
            out.push(' ');
            out.push_str(key);
            out.push_str("=\"");
            out.push_str(&php::php_trim(&value).replace('"', "\\\""));
            out.push('"');
        }
        out.trim().to_string()
    }

    /// Remove the given props from the bag (as `@props` does).
    pub fn except_props(&self, props: &ViewValue) -> Self {
        self.except(&prop_names(props))
    }

    /// Only the given props.
    pub fn only_props(&self, props: &ViewValue) -> Self {
        self.only(&prop_names(props))
    }

    fn method(&self, name: &str, args: &[ViewValue], registry: &Arc<Registry>) -> Result<ViewValue> {
        let a0 = arg(args, 0);
        let bag = |bag: ComponentAttributeBag| ViewValue::object(bag);
        Ok(match name {
            "all" | "getAttributes" | "toArray" | "jsonSerialize" => match args.first() {
                Some(keys) if !keys.is_null() => self.only(&names(keys)?).to_value(),
                _ => self.to_value(),
            },
            "first" => self
                .attributes
                .values()
                .next()
                .cloned()
                .unwrap_or_else(|| a0.clone()),
            "get" => self.attributes.get(&str_arg(args, 0)?).cloned().unwrap_or_else(|| arg(args, 1).clone()),
            "has" => {
                let keys = if args.len() > 1 { many(args)? } else { names(a0)? };
                ViewValue::Bool(keys.iter().all(|k| self.has(k)))
            }
            "hasAny" => {
                let keys = if args.len() > 1 { many(args)? } else { names(a0)? };
                ViewValue::Bool(keys.iter().any(|k| self.has(k)))
            }
            "missing" => ViewValue::Bool(!self.has(&str_arg(args, 0)?)),
            "only" => bag(self.only(&names(a0)?)),
            "except" => bag(self.except(&names(a0)?)),
            "filter" => {
                let mut kept = IndexMap::new();
                for (key, value) in &self.attributes {
                    if call_callable(a0, &[value.clone(), ViewValue::from(key.as_str())], registry)?.truthy() {
                        kept.insert(key.clone(), value.clone());
                    }
                }
                bag(ComponentAttributeBag { attributes: kept })
            }
            "whereStartsWith" | "thatStartWith" => bag(self.where_starts_with(&names(a0)?)),
            "whereDoesntStartWith" => bag(self.where_doesnt_start_with(&names(a0)?)),
            "onlyProps" => bag(self.only_props(a0)),
            "exceptProps" => bag(self.except_props(a0)),
            "class" => bag(self.class(a0)),
            "style" => bag(self.style(a0)),
            "merge" => bag(self.merge(pairs(a0)?, bool_arg(args, 1, true))),
            "prepends" => ViewValue::object(AppendableAttributeValue(a0.clone())),
            "isEmpty" => ViewValue::Bool(self.is_empty()),
            "isNotEmpty" => ViewValue::Bool(!self.is_empty()),
            "toHtml" | "__toString" => ViewValue::html(self.to_html()),
            "string" => ViewValue::from(
                self.attributes.get(&str_arg(args, 0)?).map(|v| v.to_string_lossy()).unwrap_or_else(|| arg(args, 1).to_string_lossy()),
            ),
            "boolean" => ViewValue::Bool(
                self.attributes
                    .get(&str_arg(args, 0)?)
                    .map(|v| match v {
                        ViewValue::Str(s) => matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
                        other => other.truthy(),
                    })
                    .unwrap_or(false),
            ),
            "integer" => ViewValue::Int(
                self.attributes
                    .get(&str_arg(args, 0)?)
                    .and_then(ViewValue::as_i64)
                    .unwrap_or_else(|| int_arg(args, 1, 0)),
            ),
            "count" => ViewValue::from(self.attributes.len()),
            _ => {
                return Err(BadMethodCallException::new(format!(
                    "Method Illuminate\\View\\ComponentAttributeBag::{name} does not exist."
                ))
                .into());
            }
        })
    }

    /// The attributes as an array value.
    pub fn to_value(&self) -> ViewValue {
        ViewValue::map(self.attributes.iter().map(|(k, v)| (ArrayKey::new(k), v.clone())))
    }
}

fn should_escape(escape: bool, value: &ViewValue) -> bool {
    escape && !matches!(value, ViewValue::Object(_) | ViewValue::Null | ViewValue::Bool(_) | ViewValue::Html(_))
}

fn resolve_appendable(appendable: &AppendableAttributeValue, escape: bool) -> ViewValue {
    if should_escape(escape, &appendable.0) {
        ViewValue::from(illuminate_support::e(appendable.0.to_string_lossy()))
    } else {
        appendable.0.clone()
    }
}

fn wrap(value: &ViewValue) -> ViewValue {
    match value {
        ViewValue::Array(_) => value.clone(),
        ViewValue::Null => ViewValue::empty_array(),
        other => ViewValue::list([other.clone()]),
    }
}

fn names(value: &ViewValue) -> Result<Vec<String>> {
    match value {
        ViewValue::Array(list) => list.values().map(php::to_str).collect(),
        ViewValue::Null => Ok(Vec::new()),
        other => Ok(vec![php::to_str(other)?]),
    }
}

fn many(args: &[ViewValue]) -> Result<Vec<String>> {
    args.iter().map(php::to_str).collect()
}

fn pairs(value: &ViewValue) -> Result<Vec<(String, ViewValue)>> {
    match value {
        ViewValue::Array(list) => Ok(list.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()),
        ViewValue::Null => Ok(Vec::new()),
        other => Err(crate::exception::TypeError::new(format!(
            "ComponentAttributeBag::merge(): Argument #1 must be of type array, {} given",
            other.type_name()
        ))
        .into()),
    }
}

/// The names `@props` / `@aware` declare, plus their kebab-cased forms.
pub(crate) fn prop_names(props: &ViewValue) -> Vec<String> {
    let mut names = Vec::new();
    if let ViewValue::Array(list) = props {
        for (key, value) in list.iter() {
            let name = match key {
                ArrayKey::Int(_) => value.to_string_lossy(),
                ArrayKey::Str(name) => name.to_string(),
            };
            names.push(Str::kebab(&name));
            names.push(name);
        }
    }
    names
}

/// Attribute bags need the registry for `filter()` callbacks, which the
/// object trait doesn't carry; closures are self-contained, so an empty
/// registry suffices.
fn registry() -> Arc<Registry> {
    thread_local! {
        static REGISTRY: Arc<Registry> = Arc::new(Registry::default());
    }
    REGISTRY.with(Arc::clone)
}

impl ViewObject for ComponentAttributeBag {
    fn class_name(&self) -> &str {
        "Illuminate\\View\\ComponentAttributeBag"
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        Some(self.method(method, args, &registry()))
    }

    fn invoke(&self, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        Some(pairs(arg(args, 0)).map(|defaults| ViewValue::html(self.merge(defaults, true).to_html())))
    }

    fn offset_get(&self, key: &ViewValue) -> Option<ViewValue> {
        Some(self.attributes.get(&key.to_string_lossy()).cloned().unwrap_or_default())
    }

    fn to_html(&self) -> Option<String> {
        Some(ComponentAttributeBag::to_html(self))
    }

    fn count(&self) -> Option<usize> {
        Some(self.attributes.len())
    }

    fn iterate(&self) -> Option<Vec<(ViewValue, ViewValue)>> {
        Some(
            self.attributes
                .iter()
                .map(|(k, v)| (ViewValue::from(k.as_str()), v.clone()))
                .collect(),
        )
    }

    fn to_json(&self) -> Value {
        self.to_value().to_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bag(pairs: &[(&str, ViewValue)]) -> ComponentAttributeBag {
        ComponentAttributeBag::from_pairs(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())))
    }

    #[test]
    fn it_renders_attributes() {
        let attributes = bag(&[
            ("class", "mt-4".into()),
            ("disabled", true.into()),
            ("hidden", false.into()),
            ("x-data", true.into()),
            ("title", "Say \"hi\"".into()),
        ]);
        assert_eq!(attributes.to_html(), r#"class="mt-4" disabled="disabled" x-data="" title="Say \"hi\"""#);
    }

    #[test]
    fn merge_appends_classes_and_overrides_others() {
        let attributes = bag(&[("class", "mb-4".into()), ("type", "submit".into())]);
        let merged = attributes.merge(
            [("class", ViewValue::from("alert alert-error")), ("type", "button".into()), ("role", "alert".into())],
            true,
        );
        assert_eq!(merged.to_html(), r#"class="alert alert-error mb-4" type="submit" role="alert""#);
    }

    #[test]
    fn merge_handles_styles_and_prepends() {
        let attributes = bag(&[("style", "color: red".into()), ("data-controller", "extra".into())]);
        let merged = attributes.merge(
            [
                ("style", ViewValue::from("font-weight: bold")),
                ("data-controller", ViewValue::object(AppendableAttributeValue("profile".into()))),
            ],
            true,
        );
        assert_eq!(merged.to_html(), r#"style="font-weight: bold; color: red;" data-controller="profile extra""#);
    }

    #[test]
    fn it_filters_attributes() {
        let attributes = bag(&[("wire:model", "name".into()), ("class", "x".into()), ("wire:key", "1".into())]);
        assert_eq!(attributes.where_starts_with(&["wire:model".into()]).to_html(), r#"wire:model="name""#);
        assert_eq!(attributes.where_doesnt_start_with(&["wire:".into()]).to_html(), r#"class="x""#);
        assert_eq!(attributes.only(&["class".into()]).to_html(), r#"class="x""#);
    }

    #[test]
    fn nested_bags_merge_into_their_parent() {
        let parent = bag(&[("class", "a".into())]);
        let mut attributes = IndexMap::new();
        attributes.insert("attributes".to_string(), ViewValue::object(parent));
        attributes.insert("class".to_string(), "b".into());
        attributes.insert("id".to_string(), "x".into());
        let mut child = ComponentAttributeBag::new();
        child.set_attributes(attributes);
        assert_eq!(child.to_html(), r#"class="b a" id="x""#);
    }
}
