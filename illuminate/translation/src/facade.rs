//! The `Lang` facade and the `__()`, `trans()` and `trans_choice()` helpers.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use illuminate_container::{Container, try_app};
use illuminate_support::{Result, Value};

use crate::provider::build_translator;
use crate::selector::ChoiceCount;
use crate::translator::Translator;

/// The `Lang` facade.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_container::Container;
/// use illuminate_translation::Lang;
/// use illuminate_support::json;
///
/// # let app = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(app);
/// Lang::add_lines(&json!({"messages.apples": "{0} There are none|{1} There is one|[2,*] There are :count"}), "en");
///
/// assert_eq!(Lang::choice("messages.apples", 0), "There are none");
/// assert_eq!(Lang::choice("messages.apples", 5), "There are 5");
/// assert_eq!(Lang::get("passwords.reset"), "Your password has been reset.");
/// assert!(Lang::has("auth.failed"));
/// ```
pub struct Lang;

impl Lang {
    /// Get the translator from the container, registering one (configured
    /// from `app.lang_path`, `app.locale` and `app.fallback_locale`) if the
    /// application hasn't yet.
    pub fn translator() -> Arc<Translator> {
        if let Some(translator) = try_app::<Translator>() {
            return translator;
        }
        let container = Container::get_instance();
        container.singleton_if::<Translator>(|app| Arc::new(build_translator(app)));
        container.make::<Translator>()
    }

    /// Get the translation for the given key.
    pub fn get(key: &str) -> String {
        Self::translator().get(key)
    }

    /// Get the translation for the given key, replacing `:placeholders`.
    pub fn get_with(key: &str, replace: &Value) -> String {
        Self::translator().get_with(key, replace)
    }

    /// Get the translation for the given key in a specific locale.
    pub fn get_in(key: &str, replace: &Value, locale: &str) -> String {
        Self::translator().get_in(key, replace, locale)
    }

    /// Get the translation (a string, or a group of lines) for the given key.
    pub fn get_value(key: &str, replace: &Value, locale: Option<&str>, fallback: bool) -> Value {
        Self::translator().get_value(key, replace, locale, fallback)
    }

    /// Determine if a translation exists.
    pub fn has(key: &str) -> bool {
        Self::translator().has(key)
    }

    /// Determine if a translation exists for the given locale (no fallback).
    pub fn has_for_locale(key: &str, locale: &str) -> bool {
        Self::translator().has_for_locale(key, locale)
    }

    /// Get a translation according to a count.
    pub fn choice(key: &str, number: impl Into<ChoiceCount>) -> String {
        Self::translator().choice(key, number)
    }

    /// Get a translation according to a count, replacing `:placeholders`.
    pub fn choice_with(key: &str, number: impl Into<ChoiceCount>, replace: &Value) -> String {
        Self::translator().choice_with(key, number, replace)
    }

    /// Get a translation according to a count in a specific locale.
    pub fn choice_in(
        key: &str,
        number: impl Into<ChoiceCount>,
        replace: &Value,
        locale: &str,
    ) -> String {
        Self::translator().choice_in(key, number, replace, locale)
    }

    /// Add translation lines (`"group.item"` keys) to the given locale.
    pub fn add_lines(lines: &Value, locale: &str) {
        Self::translator().add_lines(lines, locale);
    }

    /// Add translation lines to the given locale and namespace.
    pub fn add_namespaced_lines(lines: &Value, locale: &str, namespace: &str) {
        Self::translator().add_namespaced_lines(lines, locale, namespace);
    }

    /// Add a new namespace (`courier::messages.welcome`).
    pub fn add_namespace(namespace: &str, hint: impl Into<PathBuf>) {
        Self::translator().add_namespace(namespace, hint);
    }

    /// Add a new path to the loader.
    pub fn add_path(path: impl Into<PathBuf>) {
        Self::translator().add_path(path);
    }

    /// Add a new JSON path to the loader.
    pub fn add_json_path(path: impl Into<PathBuf>) {
        Self::translator().add_json_path(path);
    }

    /// Register a callback that handles missing translation keys.
    pub fn handle_missing_keys_using(
        callback: impl Fn(&str, &Value, &str, bool) -> Option<String> + Send + Sync + 'static,
    ) {
        Self::translator().handle_missing_keys_using(callback);
    }

    /// Specify a callback that determines the locales to check.
    pub fn determine_locales_using(
        callback: impl Fn(Vec<String>) -> Vec<String> + Send + Sync + 'static,
    ) {
        Self::translator().determine_locales_using(callback);
    }

    /// Get the current locale.
    pub fn locale() -> String {
        Self::translator().get_locale()
    }

    /// Get the current locale.
    pub fn get_locale() -> String {
        Self::translator().get_locale()
    }

    /// Set the current locale (for the current request, inside a
    /// [`locale_scope`](crate::locale_scope)).
    pub fn set_locale(locale: &str) -> Result<()> {
        Self::translator().set_locale(locale)
    }

    /// Determine if the given locale is the current one.
    pub fn is_locale(locale: &str) -> bool {
        Self::translator().is_locale(locale)
    }

    /// Get the fallback locale.
    pub fn get_fallback() -> Option<String> {
        Self::translator().get_fallback()
    }

    /// Set the fallback locale.
    pub fn set_fallback(locale: &str) {
        Self::translator().set_fallback(locale);
    }

    /// Run a callback with the given locale.
    pub fn with_locale<R>(locale: &str, callback: impl FnOnce() -> R) -> R {
        crate::translator::with_locale(locale, callback)
    }

    /// Run a future with the given locale.
    pub async fn with_locale_async<F: Future>(locale: &str, future: F) -> F::Output {
        crate::translator::with_locale_async(locale, future).await
    }
}

/// Translate the given message.
///
/// ```
/// # use std::sync::Arc;
/// # use illuminate_container::Container;
/// use illuminate_translation::{__, __with};
/// use illuminate_support::json;
///
/// # let app = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(app);
/// assert_eq!(__("auth.password"), "The provided password is incorrect.");
/// assert_eq!(__("I love programming."), "I love programming.");
/// assert_eq!(__with("auth.throttle", json!({"seconds": 60})), "Too many login attempts. Please try again in 60 seconds.");
/// ```
pub fn __(key: &str) -> String {
    Lang::get(key)
}

/// Translate the given message, replacing `:placeholders`.
pub fn __with(key: &str, replace: Value) -> String {
    Lang::get_with(key, &replace)
}

/// Translate the given message.
pub fn trans(key: &str) -> String {
    Lang::get(key)
}

/// Translate the given message, replacing `:placeholders`.
pub fn trans_with(key: &str, replace: Value) -> String {
    Lang::get_with(key, &replace)
}

/// Translate the given message based on a count.
///
/// ```
/// # use std::sync::Arc;
/// # use illuminate_container::Container;
/// use illuminate_translation::{trans_choice, trans_choice_with, Lang};
/// use illuminate_support::json;
///
/// # let app = Arc::new(Container::new());
/// # let _guard = Container::set_local_instance(app);
/// Lang::add_lines(&json!({"time.minutes_ago": "{1} :value minute ago|[2,*] :value minutes ago"}), "en");
///
/// assert_eq!(trans_choice("There is one apple|There are many apples", 10), "There are many apples");
/// assert_eq!(trans_choice_with("time.minutes_ago", 5, json!({"value": 5})), "5 minutes ago");
/// ```
pub fn trans_choice(key: &str, number: impl Into<ChoiceCount>) -> String {
    Lang::choice(key, number)
}

/// Translate the given message based on a count, replacing `:placeholders`.
pub fn trans_choice_with(key: &str, number: impl Into<ChoiceCount>, replace: Value) -> String {
    Lang::choice_with(key, number, &replace)
}
