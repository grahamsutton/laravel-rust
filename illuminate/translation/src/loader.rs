//! Translation loaders read language lines from JSON files (or memory).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use indexmap::IndexMap;

use illuminate_support::error::RuntimeException;
use illuminate_support::{Map, Result, Value};

/// Loads the language lines for a locale, group, and namespace.
///
/// A `group` and `namespace` of `"*"` request the JSON translation strings
/// for the locale (`lang/{locale}.json`).
pub trait Loader: Send + Sync {
    /// Load the messages for the given locale (an object of lines).
    fn load(&self, locale: &str, group: &str, namespace: Option<&str>) -> Result<Value>;

    /// Add a new namespace to the loader.
    fn add_namespace(&self, namespace: &str, hint: PathBuf);

    /// Add a new JSON path to the loader.
    fn add_json_path(&self, path: PathBuf);

    /// Add a new path to the loader.
    fn add_path(&self, _path: PathBuf) {}

    /// Get every registered namespace and its path.
    fn namespaces(&self) -> IndexMap<String, PathBuf>;
}

/// The framework's English language files, by group: `auth`,
/// `pagination`, `passwords` and `validation`. `cargo artisan lang:publish`
/// copies them to the application's `lang/en` directory for customization.
///
/// ```
/// let (group, json) = illuminate_translation::FRAMEWORK_FILES[0];
/// assert_eq!(group, "auth");
/// assert!(json.contains("These credentials do not match our records."));
/// ```
pub const FRAMEWORK_FILES: [(&str, &str); 4] = [
    ("auth", include_str!("../lang/en/auth.json")),
    ("pagination", include_str!("../lang/en/pagination.json")),
    ("passwords", include_str!("../lang/en/passwords.json")),
    ("validation", include_str!("../lang/en/validation.json")),
];

/// The English lines shipped with the framework (`auth`, `pagination`,
/// `passwords` and `validation`). Applications override them with their own
/// `lang/en/{group}.json` files.
static FRAMEWORK_LINES: LazyLock<HashMap<&'static str, Value>> = LazyLock::new(|| {
    FRAMEWORK_FILES
        .into_iter()
        .map(|(group, json)| {
            (
                group,
                serde_json::from_str(json).expect("the framework's language files are valid JSON"),
            )
        })
        .collect()
});

/// The framework's default lines for a locale and group, if it ships any.
pub fn framework_lines(locale: &str, group: &str) -> Option<Value> {
    (locale == "en")
        .then(|| FRAMEWORK_LINES.get(group).cloned())
        .flatten()
}

/// Loads language lines from JSON files on disk.
///
/// ```text
/// lang/
///     en.json             ← "I love programming.": "..."
///     en/messages.json    ← {"welcome": "Welcome to our application!"}
///     es/messages.json
///     vendor/courier/en/messages.json  ← overrides for the "courier" namespace
/// ```
#[derive(Debug)]
pub struct FileLoader {
    paths: RwLock<Vec<PathBuf>>,
    json_paths: RwLock<Vec<PathBuf>>,
    hints: RwLock<IndexMap<String, PathBuf>>,
    framework_lines: bool,
}

impl FileLoader {
    /// Create a loader for the application's `lang` directory, layered on
    /// top of the framework's own English lines.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_paths(vec![path.into()])
    }

    /// Create a loader reading several directories (later ones win).
    pub fn with_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            paths: RwLock::new(paths),
            json_paths: RwLock::new(Vec::new()),
            hints: RwLock::new(IndexMap::new()),
            framework_lines: true,
        }
    }

    /// Don't include the framework's default English lines.
    pub fn without_framework_lines(mut self) -> Self {
        self.framework_lines = false;
        self
    }

    /// The registered paths to translation files.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.paths.read().unwrap().clone()
    }

    /// The registered paths to JSON translation files.
    pub fn json_paths(&self) -> Vec<PathBuf> {
        self.json_paths.read().unwrap().clone()
    }

    fn load_paths(
        &self,
        paths: &[PathBuf],
        locale: &str,
        group: &str,
        framework: bool,
    ) -> Result<Value> {
        let mut output = match framework.then(|| framework_lines(locale, group)).flatten() {
            Some(lines) => lines,
            None => Value::Object(Map::new()),
        };
        for path in paths {
            let full = path.join(locale).join(format!("{group}.json"));
            if let Some(lines) = read_json(&full)? {
                replace_recursive(&mut output, lines);
            }
        }
        Ok(output)
    }

    fn load_namespaced(&self, locale: &str, group: &str, namespace: &str) -> Result<Value> {
        let Some(hint) = self.hints.read().unwrap().get(namespace).cloned() else {
            return Ok(Value::Object(Map::new()));
        };
        let mut lines = self.load_paths(&[hint], locale, group, false)?;
        for path in self.paths() {
            let full = path
                .join("vendor")
                .join(namespace)
                .join(locale)
                .join(format!("{group}.json"));
            if let Some(overrides) = read_json(&full)? {
                replace_recursive(&mut lines, overrides);
            }
        }
        Ok(lines)
    }

    fn load_json_paths(&self, locale: &str) -> Result<Value> {
        let mut output = Map::new();
        let paths = self.json_paths().into_iter().chain(self.paths());
        for path in paths {
            let full = path.join(format!("{locale}.json"));
            if let Some(decoded) = read_json(&full)? {
                match decoded {
                    Value::Object(lines) => output.extend(lines),
                    _ => {
                        return Err(RuntimeException::new(format!(
                            "Translation file [{}] contains an invalid JSON structure.",
                            full.display()
                        ))
                        .into());
                    }
                }
            }
        }
        Ok(Value::Object(output))
    }
}

impl Loader for FileLoader {
    fn load(&self, locale: &str, group: &str, namespace: Option<&str>) -> Result<Value> {
        if is_unsafe_path_segment(locale, false)
            || (group != "*" && is_unsafe_path_segment(group, true))
        {
            return Ok(Value::Object(Map::new()));
        }
        match namespace {
            Some("*") if group == "*" => self.load_json_paths(locale),
            None | Some("*") => self.load_paths(&self.paths(), locale, group, self.framework_lines),
            Some(namespace) => self.load_namespaced(locale, group, namespace),
        }
    }

    fn add_namespace(&self, namespace: &str, hint: PathBuf) {
        self.hints
            .write()
            .unwrap()
            .insert(namespace.to_string(), hint);
    }

    fn add_json_path(&self, path: PathBuf) {
        self.json_paths.write().unwrap().push(path);
    }

    fn add_path(&self, path: PathBuf) {
        self.paths.write().unwrap().push(path);
    }

    fn namespaces(&self) -> IndexMap<String, PathBuf> {
        self.hints.read().unwrap().clone()
    }
}

/// Read and decode a JSON language file, if it exists.
fn read_json(path: &Path) -> Result<Option<Value>> {
    if !path.is_file() {
        return Ok(None);
    }
    let contents = fs::read_to_string(path)?;
    match serde_json::from_str::<Value>(&contents) {
        Ok(Value::Null) | Err(_) => Err(RuntimeException::new(format!(
            "Translation file [{}] contains an invalid JSON structure.",
            path.display()
        ))
        .into()),
        Ok(value) => Ok(Some(value)),
    }
}

/// PHP's `array_replace_recursive`.
pub(crate) fn replace_recursive(base: &mut Value, replacement: Value) {
    match (base, replacement) {
        (Value::Object(base), Value::Object(replacement)) => {
            for (key, value) in replacement {
                match base.get_mut(&key) {
                    Some(existing) if existing.is_object() || existing.is_array() => {
                        replace_recursive(existing, value)
                    }
                    _ => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (Value::Array(base), Value::Array(replacement)) => {
            for (index, value) in replacement.into_iter().enumerate() {
                match base.get_mut(index) {
                    Some(existing) if existing.is_object() || existing.is_array() => {
                        replace_recursive(existing, value)
                    }
                    Some(existing) => *existing = value,
                    None => base.push(value),
                }
            }
        }
        (base, replacement) => *base = replacement,
    }
}

/// Determine if the value is unsafe to use as part of a file path.
fn is_unsafe_path_segment(value: &str, allow_slashes: bool) -> bool {
    value.is_empty()
        || value.contains("..")
        || value.contains('\\')
        || value.contains('\0')
        || (!allow_slashes && value.contains('/'))
}

/// Keeps language lines in memory, which is handy for tests and packages
/// that ship their lines in code.
///
/// ```
/// use illuminate_translation::{ArrayLoader, Translator};
/// use illuminate_support::json;
///
/// let loader = ArrayLoader::new();
/// loader.add_messages("en", "messages", json!({"welcome": "Welcome, :name!"}), None);
///
/// let translator = Translator::with_loader(loader, "en");
/// assert_eq!(translator.get_with("messages.welcome", &json!({"name": "Taylor"})), "Welcome, Taylor!");
/// ```
#[derive(Debug, Default)]
pub struct ArrayLoader {
    messages: RwLock<HashMap<(String, String, String), Value>>,
}

impl ArrayLoader {
    /// Create an empty loader.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add messages to the loader. Use the group `"*"` (and namespace
    /// `"*"`) for JSON translation strings.
    pub fn add_messages(
        &self,
        locale: &str,
        group: &str,
        messages: Value,
        namespace: Option<&str>,
    ) -> &Self {
        let namespace = namespace.filter(|n| !n.is_empty()).unwrap_or("*");
        self.messages.write().unwrap().insert(
            (namespace.to_string(), locale.to_string(), group.to_string()),
            messages,
        );
        self
    }
}

impl Loader for ArrayLoader {
    fn load(&self, locale: &str, group: &str, namespace: Option<&str>) -> Result<Value> {
        let namespace = namespace.filter(|n| !n.is_empty()).unwrap_or("*");
        Ok(self
            .messages
            .read()
            .unwrap()
            .get(&(namespace.to_string(), locale.to_string(), group.to_string()))
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new())))
    }

    fn add_namespace(&self, _namespace: &str, _hint: PathBuf) {}

    fn add_json_path(&self, _path: PathBuf) {}

    fn namespaces(&self) -> IndexMap<String, PathBuf> {
        IndexMap::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn framework_lines_are_embedded() {
        let validation = framework_lines("en", "validation").unwrap();
        assert_eq!(validation["required"], "The :attribute field is required.");
        assert_eq!(
            validation["between"]["numeric"],
            "The :attribute field must be between :min and :max."
        );
        assert_eq!(
            framework_lines("en", "auth").unwrap()["failed"],
            "These credentials do not match our records."
        );
        assert_eq!(
            framework_lines("en", "pagination").unwrap()["next"],
            "Next &raquo;"
        );
        assert_eq!(
            framework_lines("en", "passwords").unwrap()["user"],
            "We can't find a user with that email address."
        );
        assert!(framework_lines("es", "auth").is_none());
        assert!(framework_lines("en", "missing").is_none());
    }

    #[test]
    fn replace_recursive_behaves_like_php() {
        let mut base = json!({"a": {"b": 1, "c": 2}, "list": [1, 2, 3], "d": "x"});
        replace_recursive(
            &mut base,
            json!({"a": {"c": 3, "e": 4}, "list": [9], "d": {"nested": true}}),
        );
        assert_eq!(
            base,
            json!({"a": {"b": 1, "c": 3, "e": 4}, "list": [9, 2, 3], "d": {"nested": true}})
        );
    }

    #[test]
    fn unsafe_segments_are_detected() {
        assert!(is_unsafe_path_segment("", false));
        assert!(is_unsafe_path_segment("../etc", false));
        assert!(is_unsafe_path_segment("a/b", false));
        assert!(!is_unsafe_path_segment("a/b", true));
        assert!(is_unsafe_path_segment("a\\b", true));
        assert!(!is_unsafe_path_segment("en_US", false));
    }
}
