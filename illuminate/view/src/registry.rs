//! The extensions registered with Blade: functions, directives, custom
//! conditionals and components.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use illuminate_support::Result;

use crate::component::{Component, ComponentArgs};
use crate::value::{ViewFunction, ViewValue};

/// A custom directive: receives the evaluated arguments and returns the HTML
/// to output.
pub type DirectiveHandler = Arc<dyn Fn(&[ViewValue]) -> Result<String> + Send + Sync>;

/// A custom conditional registered with `Blade::if_`.
pub type ConditionHandler = Arc<dyn Fn(&[ViewValue]) -> bool + Send + Sync>;

/// Builds a class-based component from the attributes on its tag.
pub type ComponentFactory =
    Arc<dyn Fn(&mut ComponentArgs) -> Result<Arc<dyn Component>> + Send + Sync>;

/// Converts objects of a particular type to strings when echoed (`Blade::stringable`).
pub(crate) type EchoHandler = Arc<dyn Fn(&ViewValue) -> Option<String> + Send + Sync>;

/// An extra directory holding anonymous components.
#[derive(Clone, Debug)]
pub(crate) struct AnonymousComponentPath {
    pub path: PathBuf,
    pub prefix: Option<String>,
}

/// A snapshot of everything registered with Blade.
#[derive(Clone)]
pub(crate) struct Registry {
    pub functions: HashMap<String, ViewFunction>,
    pub directives: HashMap<String, DirectiveHandler>,
    pub conditions: HashMap<String, ConditionHandler>,
    pub components: HashMap<String, ComponentFactory>,
    pub anonymous_paths: Vec<AnonymousComponentPath>,
    pub anonymous_namespaces: HashMap<String, String>,
    pub echo_handlers: Vec<EchoHandler>,
    pub double_encode: bool,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            functions: HashMap::new(),
            directives: HashMap::new(),
            conditions: HashMap::new(),
            components: HashMap::new(),
            anonymous_paths: Vec::new(),
            anonymous_namespaces: HashMap::new(),
            echo_handlers: Vec::new(),
            double_encode: true,
        }
    }
}

impl Registry {
    /// Call a registered function if it exists.
    pub(crate) fn call(&self, name: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        self.functions.get(name).map(|function| function(args))
    }

    /// Apply the first matching custom echo handler.
    pub(crate) fn apply_echo_handlers(&self, value: &ViewValue) -> Option<String> {
        if self.echo_handlers.is_empty() {
            return None;
        }
        self.echo_handlers.iter().find_map(|handler| handler(value))
    }

    /// Escape a string for HTML using the configured double-encoding mode.
    pub(crate) fn escape(&self, value: &str) -> String {
        crate::php::escape(value, self.double_encode)
    }
}
