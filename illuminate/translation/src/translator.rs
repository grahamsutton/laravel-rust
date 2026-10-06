//! The translator.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use indexmap::IndexMap;

use illuminate_config::Repository;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Arr, Map, Result, Str, Value, ValueExt};

use crate::loader::{FileLoader, Loader};
use crate::selector::{ChoiceCount, MessageSelector};

tokio::task_local! {
    /// The locale for the current request (or job), set by `set_locale`
    /// inside a `locale_scope`.
    static SCOPED_LOCALE: RefCell<Option<String>>;
}

thread_local! {
    /// Guards against infinite loops when a missing-key handler translates.
    static HANDLING_MISSING_KEY: Cell<bool> = const { Cell::new(false) };
}

type MissingKeyCallback = dyn Fn(&str, &Value, &str, bool) -> Option<String> + Send + Sync;
type DetermineLocales = dyn Fn(Vec<String>) -> Vec<String> + Send + Sync;

/// Loaded lines, by namespace, group, and locale.
type Loaded = HashMap<String, HashMap<String, HashMap<String, Value>>>;

/// The translator.
///
/// ```
/// use illuminate_translation::Translator;
/// use illuminate_support::json;
///
/// let dir = tempfile::tempdir().unwrap();
/// std::fs::create_dir(dir.path().join("en")).unwrap();
/// std::fs::write(dir.path().join("en/messages.json"), r#"{"welcome": "Welcome, :name!"}"#).unwrap();
/// std::fs::write(dir.path().join("es.json"), r#"{"I love programming.": "Me encanta programar."}"#).unwrap();
///
/// let translator = Translator::new(dir.path(), "en");
///
/// assert_eq!(translator.get_with("messages.welcome", &json!({"name": "dayle"})), "Welcome, dayle!");
/// assert_eq!(translator.get_in("I love programming.", &json!({}), "es"), "Me encanta programar.");
/// assert_eq!(translator.get("auth.failed"), "These credentials do not match our records.");
/// assert_eq!(translator.get("messages.missing"), "messages.missing");
/// ```
pub struct Translator {
    loader: Arc<dyn Loader>,
    locale: RwLock<String>,
    fallback: RwLock<Option<String>>,
    loaded: RwLock<Loaded>,
    selector: MessageSelector,
    determine_locales: RwLock<Option<Arc<DetermineLocales>>>,
    missing_key_callback: RwLock<Option<Arc<MissingKeyCallback>>>,
}

impl Translator {
    /// Create a translator for the given `lang` directory, layered on top
    /// of the framework's own English lines.
    pub fn new(path: impl Into<PathBuf>, locale: impl Into<String>) -> Self {
        Self::with_loader(FileLoader::new(path), locale)
    }

    /// Create a translator using the given loader.
    pub fn with_loader(loader: impl Loader + 'static, locale: impl Into<String>) -> Self {
        Self::with_loader_arc(Arc::new(loader), locale)
    }

    /// Create a translator using a shared loader.
    pub fn with_loader_arc(loader: Arc<dyn Loader>, locale: impl Into<String>) -> Self {
        Self {
            loader,
            locale: RwLock::new(locale.into()),
            fallback: RwLock::new(None),
            loaded: RwLock::new(HashMap::new()),
            selector: MessageSelector,
            determine_locales: RwLock::new(None),
            missing_key_callback: RwLock::new(None),
        }
    }

    /// Create a translator from the application configuration:
    /// `app.lang_path` (default `lang`), `app.locale` (default `en`) and
    /// `app.fallback_locale` (default `en`).
    pub fn from_config(config: &Repository) -> Self {
        let translator = Self::new(lang_path(config), config.string_or("app.locale", "en"));
        translator.set_fallback(&config.string_or("app.fallback_locale", "en"));
        translator
    }

    // ------------------------------------------------------------------
    // Retrieving lines
    // ------------------------------------------------------------------

    /// Get the translation for the given key, or the key itself when the
    /// line doesn't exist.
    pub fn get(&self, key: &str) -> String {
        self.get_with(key, &Value::Null)
    }

    /// Get the translation for the given key, replacing `:placeholders`.
    pub fn get_with(&self, key: &str, replace: &Value) -> String {
        into_string(self.get_value(key, replace, None, true))
    }

    /// Get the translation for the given key in a specific locale.
    pub fn get_in(&self, key: &str, replace: &Value, locale: &str) -> String {
        into_string(self.get_value(key, replace, Some(locale), true))
    }

    /// Get the translation for the given key: a string, or a whole group of
    /// lines when the key names one (`"validation.between"`).
    ///
    /// This is Laravel's `Translator::get($key, $replace, $locale, $fallback)`.
    pub fn get_value(
        &self,
        key: &str,
        replace: &Value,
        locale: Option<&str>,
        fallback: bool,
    ) -> Value {
        self.translate(key, replace, locale, fallback, true)
    }

    fn translate(
        &self,
        key: &str,
        replace: &Value,
        locale: Option<&str>,
        fallback: bool,
        handle_missing: bool,
    ) -> Value {
        let locale = match locale {
            Some(locale) if !locale.is_empty() => locale.to_string(),
            _ => self.get_locale(),
        };

        // JSON translations are a single, flat file per locale, so they're
        // checked first. That way `__` works for both kinds of keys.
        self.load("*", "*", &locale);

        let Some(line) = self.json_line(&locale, key) else {
            let (namespace, group, item) = self.parse_key(key);

            let locales = if fallback {
                self.locale_array(&locale)
            } else {
                vec![locale.clone()]
            };

            for line_locale in locales {
                if let Some(line) =
                    self.get_line(&namespace, &group, &line_locale, item.as_deref(), replace)
                {
                    return line;
                }
            }

            // If the line doesn't exist, we return the key: quick to spot
            // in the UI when language keys are wrong or missing.
            let key = self.handle_missing_key(key, replace, &locale, fallback, handle_missing);
            return Value::String(make_replacements(&key, replace));
        };

        // Like PHP's `$line ?: $key`, an empty line falls back to the key.
        if !line.truthy() {
            return Value::String(make_replacements(key, replace));
        }
        match line {
            Value::String(line) => Value::String(make_replacements(&line, replace)),
            other => other,
        }
    }

    fn json_line(&self, locale: &str, key: &str) -> Option<Value> {
        self.loaded
            .read()
            .unwrap()
            .get("*")
            .and_then(|groups| groups.get("*"))
            .and_then(|locales| locales.get(locale))
            .and_then(|lines| lines.get(key))
            .filter(|line| !line.is_null())
            .cloned()
    }

    /// Retrieve a language line out of the loaded lines.
    fn get_line(
        &self,
        namespace: &str,
        group: &str,
        locale: &str,
        item: Option<&str>,
        replace: &Value,
    ) -> Option<Value> {
        self.load(namespace, group, locale);

        let line = {
            let loaded = self.loaded.read().unwrap();
            let lines = loaded.get(namespace)?.get(group)?.get(locale)?;
            match item {
                None => lines.clone(),
                Some(item) => lines.dot(item)?.clone(),
            }
        };

        match line {
            Value::String(line) => Some(Value::String(make_replacements(&line, replace))),
            Value::Array(ref items) if !items.is_empty() => {
                Some(replace_recursively(line, replace))
            }
            Value::Object(ref items) if !items.is_empty() => {
                Some(replace_recursively(line, replace))
            }
            _ => None,
        }
    }

    /// Determine if a translation exists (checking the fallback locale too).
    pub fn has(&self, key: &str) -> bool {
        self.has_in(key, None, true)
    }

    /// Determine if a translation exists for the given locale, without
    /// checking the fallback.
    pub fn has_for_locale(&self, key: &str, locale: &str) -> bool {
        self.has_in(key, Some(locale), false)
    }

    /// Determine if a translation exists for a locale, optionally checking
    /// the fallback locale.
    pub fn has_in(&self, key: &str, locale: Option<&str>, fallback: bool) -> bool {
        let locale = match locale {
            Some(locale) if !locale.is_empty() => locale.to_string(),
            _ => self.get_locale(),
        };

        // Missing-key handlers shouldn't run during existence checks.
        let line = self.translate(key, &Value::Null, Some(&locale), fallback, false);

        // For JSON translations the loaded file holds the line; otherwise
        // the line exists when it differs from the key we asked for.
        if self.json_line(&locale, key).is_some() {
            return true;
        }

        line != Value::String(key.to_string())
    }

    // ------------------------------------------------------------------
    // Pluralization
    // ------------------------------------------------------------------

    /// Get a translation according to a count.
    pub fn choice(&self, key: &str, number: impl Into<ChoiceCount>) -> String {
        self.choice_value(key, number.into(), &Value::Null, None)
    }

    /// Get a translation according to a count, replacing `:placeholders`
    /// (`:count` is filled in automatically).
    pub fn choice_with(
        &self,
        key: &str,
        number: impl Into<ChoiceCount>,
        replace: &Value,
    ) -> String {
        self.choice_value(key, number.into(), replace, None)
    }

    /// Get a translation according to a count in a specific locale.
    pub fn choice_in(
        &self,
        key: &str,
        number: impl Into<ChoiceCount>,
        replace: &Value,
        locale: &str,
    ) -> String {
        self.choice_value(key, number.into(), replace, Some(locale))
    }

    fn choice_value(
        &self,
        key: &str,
        number: ChoiceCount,
        replace: &Value,
        locale: Option<&str>,
    ) -> String {
        let locale = self.locale_for_choice(key, locale);
        let line = into_string(self.get_value(key, &Value::Null, Some(&locale), true));

        let mut replace = replacement_map(replace);
        if !replace.contains_key("count") {
            replace.insert("count".to_string(), number.to_value());
        }

        make_replacements(
            &self.selector.choose(&line, number.as_f64(), &locale),
            &Value::Object(replace),
        )
    }

    /// The locale to choose a line in: the requested locale when it has the
    /// line, otherwise the fallback.
    fn locale_for_choice(&self, key: &str, locale: Option<&str>) -> String {
        let locale = match locale {
            Some(locale) if !locale.is_empty() => locale.to_string(),
            _ => self.get_locale(),
        };
        if self.has_for_locale(key, &locale) {
            return locale;
        }
        self.get_fallback().unwrap_or(locale)
    }

    // ------------------------------------------------------------------
    // Loading
    // ------------------------------------------------------------------

    /// Load the specified language group.
    pub fn load(&self, namespace: &str, group: &str, locale: &str) {
        if self.is_loaded(namespace, group, locale) {
            return;
        }

        let lines = match self.loader.load(locale, group, Some(namespace)) {
            Ok(lines) => lines,
            Err(error) => {
                eprintln!("{error}");
                Value::Object(Map::new())
            }
        };

        self.loaded
            .write()
            .unwrap()
            .entry(namespace.to_string())
            .or_default()
            .entry(group.to_string())
            .or_default()
            .entry(locale.to_string())
            .or_insert(lines);
    }

    fn is_loaded(&self, namespace: &str, group: &str, locale: &str) -> bool {
        self.loaded
            .read()
            .unwrap()
            .get(namespace)
            .and_then(|groups| groups.get(group))
            .is_some_and(|locales| locales.contains_key(locale))
    }

    /// Add translation lines to the given locale. Keys are
    /// `"group.item"` (`"messages.welcome"`), or `"*.text"` for JSON
    /// translation strings.
    ///
    /// ```
    /// use illuminate_translation::{ArrayLoader, Translator};
    /// use illuminate_support::json;
    ///
    /// let translator = Translator::with_loader(ArrayLoader::new(), "en");
    /// translator.add_lines(&json!({"messages.welcome": "Hello!", "*.Goodbye": "Bye!"}), "en");
    ///
    /// assert_eq!(translator.get("messages.welcome"), "Hello!");
    /// assert_eq!(translator.get("Goodbye"), "Bye!");
    /// ```
    pub fn add_lines(&self, lines: &Value, locale: &str) {
        self.add_namespaced_lines(lines, locale, "*");
    }

    /// Add translation lines to the given locale and namespace.
    pub fn add_namespaced_lines(&self, lines: &Value, locale: &str, namespace: &str) {
        let Value::Object(lines) = lines else {
            return;
        };
        for (key, value) in lines {
            let Some((group, item)) = key.split_once('.') else {
                continue;
            };
            // Load the group first so its file lines aren't hidden.
            self.load(namespace, group, locale);
            let mut loaded = self.loaded.write().unwrap();
            let target = loaded
                .entry(namespace.to_string())
                .or_default()
                .entry(group.to_string())
                .or_default()
                .entry(locale.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if group == "*" {
                // JSON lines are flat: "Hello. How are you?" is one key.
                if let Value::Object(map) = target {
                    map.insert(item.to_string(), value.clone());
                }
            } else {
                Arr::set(target, item, value.clone());
            }
        }
    }

    /// Parse a key into namespace, group, and item:
    /// `"courier::messages.welcome"` → `("courier", "messages", Some("welcome"))`.
    pub fn parse_key(&self, key: &str) -> (String, String, Option<String>) {
        let (namespace, rest) = match key.split_once("::") {
            Some((namespace, rest)) => {
                // Like PHP's `explode('::', $key)` destructuring, anything
                // after a second `::` is ignored.
                let rest = rest.split("::").next().unwrap_or(rest);
                (namespace.to_string(), rest)
            }
            None => ("*".to_string(), key),
        };
        let (group, item) = match rest.split_once('.') {
            Some((group, item)) => (group.to_string(), Some(item.to_string())),
            None => (rest.to_string(), None),
        };
        (namespace, group, item)
    }

    /// The locales to check: the requested locale, then the fallback.
    fn locale_array(&self, locale: &str) -> Vec<String> {
        let mut locales = vec![locale.to_string()];
        if let Some(fallback) = self.get_fallback().filter(|f| !f.is_empty()) {
            locales.push(fallback);
        }
        locales.retain(|l| !l.is_empty());

        let determine = self.determine_locales.read().unwrap().clone();
        if let Some(determine) = determine {
            locales = determine(locales);
        }

        let mut unique = Vec::with_capacity(locales.len());
        for locale in locales {
            if !unique.contains(&locale) {
                unique.push(locale);
            }
        }
        unique
    }

    /// Specify a callback that determines the locales to check (it receives
    /// the requested and fallback locales).
    pub fn determine_locales_using(
        &self,
        callback: impl Fn(Vec<String>) -> Vec<String> + Send + Sync + 'static,
    ) {
        *self.determine_locales.write().unwrap() = Some(Arc::new(callback));
    }

    fn handle_missing_key(
        &self,
        key: &str,
        replace: &Value,
        locale: &str,
        fallback: bool,
        handle: bool,
    ) -> String {
        if !handle || HANDLING_MISSING_KEY.with(Cell::get) {
            return key.to_string();
        }
        let Some(callback) = self.missing_key_callback.read().unwrap().clone() else {
            return key.to_string();
        };

        // Prevent infinite loops when the callback translates too...
        HANDLING_MISSING_KEY.with(|flag| flag.set(true));
        let handled = callback(key, replace, locale, fallback);
        HANDLING_MISSING_KEY.with(|flag| flag.set(false));

        handled.unwrap_or_else(|| key.to_string())
    }

    /// Register a callback that handles missing translation keys. It
    /// receives the key, replacements, locale and fallback flag, and may
    /// return a replacement line.
    ///
    /// ```
    /// use illuminate_translation::{ArrayLoader, Translator};
    ///
    /// let translator = Translator::with_loader(ArrayLoader::new(), "en");
    /// translator.handle_missing_keys_using(|key, _replace, locale, _fallback| {
    ///     Some(format!("[{locale}] {key}"))
    /// });
    ///
    /// assert_eq!(translator.get("messages.missing"), "[en] messages.missing");
    /// assert!(!translator.has("messages.missing"));
    /// ```
    pub fn handle_missing_keys_using(
        &self,
        callback: impl Fn(&str, &Value, &str, bool) -> Option<String> + Send + Sync + 'static,
    ) {
        *self.missing_key_callback.write().unwrap() = Some(Arc::new(callback));
    }

    /// Stop handling missing translation keys.
    pub fn forget_missing_keys_handler(&self) {
        *self.missing_key_callback.write().unwrap() = None;
    }

    // ------------------------------------------------------------------
    // Loader
    // ------------------------------------------------------------------

    /// Add a new namespace (`"courier"` → `courier::messages.welcome`).
    pub fn add_namespace(&self, namespace: &str, hint: impl Into<PathBuf>) {
        self.loader.add_namespace(namespace, hint.into());
    }

    /// Add a new path to the loader.
    pub fn add_path(&self, path: impl Into<PathBuf>) {
        self.loader.add_path(path.into());
    }

    /// Add a new JSON path to the loader.
    pub fn add_json_path(&self, path: impl Into<PathBuf>) {
        self.loader.add_json_path(path.into());
    }

    /// The registered namespaces.
    pub fn namespaces(&self) -> IndexMap<String, PathBuf> {
        self.loader.namespaces()
    }

    /// The language line loader.
    pub fn get_loader(&self) -> Arc<dyn Loader> {
        self.loader.clone()
    }

    /// The message selector.
    pub fn get_selector(&self) -> MessageSelector {
        self.selector
    }

    // ------------------------------------------------------------------
    // Locales
    // ------------------------------------------------------------------

    /// Get the current locale.
    pub fn locale(&self) -> String {
        self.get_locale()
    }

    /// Get the current locale: the locale set for the current request (see
    /// [`locale_scope`]) or the translator's default.
    pub fn get_locale(&self) -> String {
        SCOPED_LOCALE
            .try_with(|scoped| scoped.borrow().clone())
            .ok()
            .flatten()
            .unwrap_or_else(|| self.locale.read().unwrap().clone())
    }

    /// Set the current locale.
    ///
    /// Inside a [`locale_scope`] (each HTTP request runs in one) this only
    /// changes the locale for that scope; elsewhere it changes the default.
    pub fn set_locale(&self, locale: &str) -> Result<()> {
        if Str::contains_any(locale, &["/", "\\", "..", "\0"]) {
            return Err(
                InvalidArgumentException::new("Invalid characters present in locale.").into(),
            );
        }
        let scoped = SCOPED_LOCALE
            .try_with(|scoped| *scoped.borrow_mut() = Some(locale.to_string()))
            .is_ok();
        if !scoped {
            *self.locale.write().unwrap() = locale.to_string();
        }
        Ok(())
    }

    /// Determine if the given locale is the current one.
    pub fn is_locale(&self, locale: &str) -> bool {
        self.get_locale() == locale
    }

    /// Get the fallback locale.
    pub fn get_fallback(&self) -> Option<String> {
        self.fallback.read().unwrap().clone()
    }

    /// Set the fallback locale.
    pub fn set_fallback(&self, fallback: &str) {
        *self.fallback.write().unwrap() = Some(fallback.to_string()).filter(|f| !f.is_empty());
    }
}

impl fmt::Debug for Translator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Translator")
            .field("locale", &self.get_locale())
            .field("fallback", &self.get_fallback())
            .finish_non_exhaustive()
    }
}

/// Run a future with its own locale: [`Translator::set_locale`] calls made
/// inside only affect it. The HTTP kernel runs each request in a scope,
/// mirroring how PHP gives every request a fresh application.
///
/// ```
/// use illuminate_translation::{ArrayLoader, Translator, locale_scope};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let translator = Translator::with_loader(ArrayLoader::new(), "en");
///
/// locale_scope(async {
///     translator.set_locale("es").unwrap();
///     assert_eq!(translator.get_locale(), "es");
/// })
/// .await;
///
/// assert_eq!(translator.get_locale(), "en");
/// # });
/// ```
pub async fn locale_scope<F: Future>(future: F) -> F::Output {
    let current = SCOPED_LOCALE
        .try_with(|scoped| scoped.borrow().clone())
        .ok()
        .flatten();
    SCOPED_LOCALE.scope(RefCell::new(current), future).await
}

/// Run a future with the given locale (for mailables, notifications, ...).
pub async fn with_locale_async<F: Future>(locale: &str, future: F) -> F::Output {
    SCOPED_LOCALE
        .scope(RefCell::new(Some(locale.to_string())), future)
        .await
}

/// Run a callback with the given locale.
///
/// ```
/// use illuminate_translation::{ArrayLoader, Translator, with_locale};
///
/// let translator = Translator::with_loader(ArrayLoader::new(), "en");
///
/// assert_eq!(with_locale("fr", || translator.get_locale()), "fr");
/// assert_eq!(translator.get_locale(), "en");
/// ```
pub fn with_locale<R>(locale: &str, callback: impl FnOnce() -> R) -> R {
    SCOPED_LOCALE.sync_scope(RefCell::new(Some(locale.to_string())), callback)
}

/// The `lang` directory from configuration (`app.lang_path`, default `lang`).
pub(crate) fn lang_path(config: &Repository) -> PathBuf {
    PathBuf::from(config.string_or("app.lang_path", "lang"))
}

fn into_string(value: Value) -> String {
    match value {
        Value::String(string) => string,
        other => other.to_string_lossy(),
    }
}

fn replacement_map(replace: &Value) -> Map<String, Value> {
    match replace {
        Value::Object(map) => map.clone(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value.clone()))
            .collect(),
        _ => Map::new(),
    }
}

fn replace_recursively(value: Value, replace: &Value) -> Value {
    match value {
        Value::String(line) => Value::String(make_replacements(&line, replace)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|v| replace_recursively(v, replace))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, replace_recursively(v, replace)))
                .collect(),
        ),
        other => other,
    }
}

/// Make the `:placeholder` replacements on a line.
///
/// `:name` is replaced as given, `:Name` with the first letter capitalized,
/// and `:NAME` in uppercase. Longer placeholders are replaced first, and
/// replaced text is never replaced again.
///
/// ```
/// use illuminate_translation::make_replacements;
/// use illuminate_support::json;
///
/// assert_eq!(
///     make_replacements("Welcome, :NAME! (:Name / :name)", &json!({"name": "dayle"})),
///     "Welcome, DAYLE! (Dayle / dayle)",
/// );
/// ```
pub fn make_replacements(line: &str, replace: &Value) -> String {
    let replace = replacement_map(replace);
    if replace.is_empty() {
        return line.to_string();
    }

    let mut should_replace: IndexMap<String, String> = IndexMap::new();
    for (key, value) in &replace {
        let value = match value {
            Value::Null => String::new(),
            other => other.to_string_lossy(),
        };
        should_replace.insert(format!(":{}", Str::ucfirst(key)), Str::ucfirst(&value));
        should_replace.insert(format!(":{}", Str::upper(key)), Str::upper(&value));
        should_replace.insert(format!(":{key}"), value);
    }

    strtr(line, &should_replace)
}

/// PHP's `strtr` with an array: at each position the longest matching key
/// wins, and replaced text is never scanned again.
fn strtr(subject: &str, pairs: &IndexMap<String, String>) -> String {
    let mut keys: Vec<(&String, &String)> =
        pairs.iter().filter(|(from, _)| !from.is_empty()).collect();
    if keys.is_empty() {
        return subject.to_string();
    }
    keys.sort_by_key(|pair| std::cmp::Reverse(pair.0.len()));

    let mut out = String::with_capacity(subject.len());
    let mut rest = subject;
    'outer: while !rest.is_empty() {
        for (from, to) in &keys {
            if rest.starts_with(from.as_str()) {
                out.push_str(to);
                rest = &rest[from.len()..];
                continue 'outer;
            }
        }
        let next = rest.chars().next().map(char::len_utf8).unwrap_or(1);
        out.push_str(&rest[..next]);
        rest = &rest[next..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn replacements_follow_laravels_rules() {
        assert_eq!(
            make_replacements("Hello :name", &json!({"name": "taylor"})),
            "Hello taylor"
        );
        assert_eq!(
            make_replacements("Hello :Name", &json!({"name": "taylor"})),
            "Hello Taylor"
        );
        assert_eq!(
            make_replacements("Hello :NAME", &json!({"name": "taylor"})),
            "Hello TAYLOR"
        );
        assert_eq!(make_replacements("Hello :name", &json!({})), "Hello :name");
        assert_eq!(
            make_replacements("Hello :name", &Value::Null),
            "Hello :name"
        );
        // Longer keys are replaced first...
        assert_eq!(
            make_replacements(":name :names", &json!({"name": "a", "names": "b"})),
            "a b"
        );
        // ...and replaced values are never replaced again.
        assert_eq!(
            make_replacements(":a :b", &json!({"a": ":b", "b": "x"})),
            ":b x"
        );
        // Values of any kind become strings.
        assert_eq!(
            make_replacements(
                ":n :f :t :null",
                &json!({"n": 5, "f": 1.5, "t": true, "null": null})
            ),
            "5 1.5 1 "
        );
        // Unicode is capitalized correctly.
        assert_eq!(
            make_replacements(":Name :NAME", &json!({"name": "élise"})),
            "Élise ÉLISE"
        );
        // Keys given in uppercase keep the value as-is.
        assert_eq!(
            make_replacements(":NAME", &json!({"NAME": "taylor"})),
            "taylor"
        );
        // Lists are keyed by index.
        assert_eq!(
            make_replacements(":0 and :1", &json!(["a", "b"])),
            "a and b"
        );
    }

    #[test]
    fn keys_are_parsed_into_namespace_group_and_item() {
        let translator = Translator::with_loader(crate::ArrayLoader::new(), "en");
        assert_eq!(
            translator.parse_key("messages.welcome"),
            ("*".into(), "messages".into(), Some("welcome".into()))
        );
        assert_eq!(
            translator.parse_key("messages.a.b"),
            ("*".into(), "messages".into(), Some("a.b".into()))
        );
        assert_eq!(
            translator.parse_key("messages"),
            ("*".into(), "messages".into(), None)
        );
        assert_eq!(
            translator.parse_key("courier::messages.welcome"),
            ("courier".into(), "messages".into(), Some("welcome".into()))
        );
        assert_eq!(
            translator.parse_key("courier::messages"),
            ("courier".into(), "messages".into(), None)
        );
    }
}
