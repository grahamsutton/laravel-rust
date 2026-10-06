//! # Illuminate Translation
//!
//! Laravel's localization features provide a convenient way to retrieve
//! strings in various languages, allowing you to easily support multiple
//! languages within your application.
//!
//! Language lines live in JSON files within your application's `lang`
//! directory (configured with `app.lang_path`):
//!
//! ```text
//! lang/
//!     en/messages.json     {"welcome": "Welcome to our application!"}
//!     es/messages.json
//!     es.json              {"I love programming.": "Me encanta programar."}
//! ```
//!
//! Short keys use "dot" notation (`messages.welcome`), while JSON files use
//! the default translation as the key. The `__` helper handles both, and
//! JSON lines are checked first:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_config::Repository;
//! use illuminate_container::{Container, ServiceProvider};
//! use illuminate_translation::{__, __with, trans_choice, Lang, TranslationServiceProvider};
//! use illuminate_support::json;
//!
//! let dir = tempfile::tempdir().unwrap();
//! std::fs::create_dir(dir.path().join("es")).unwrap();
//! std::fs::write(dir.path().join("es/messages.json"), r#"{"welcome": "Bienvenido, :name"}"#).unwrap();
//! std::fs::write(dir.path().join("es.json"), r#"{"I love programming.": "Me encanta programar."}"#).unwrap();
//!
//! let app = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(app.clone());
//! app.instance(Repository::new(json!({
//!     "app": {"locale": "es", "fallback_locale": "en", "lang_path": dir.path()},
//! })));
//! TranslationServiceProvider.register(&app);
//!
//! assert_eq!(__("I love programming."), "Me encanta programar.");
//! assert_eq!(__with("messages.welcome", json!({"name": "dayle"})), "Bienvenido, dayle");
//! assert_eq!(__("auth.failed"), "These credentials do not match our records.");
//! assert_eq!(trans_choice("{0} There are none|[1,19] There are some|[20,*] There are many", 20), "There are many");
//! ```
//!
//! ## Framework lines
//!
//! The framework's English `auth`, `pagination`, `passwords` and
//! `validation` lines are built in; override any of them with your own
//! `lang/en/{group}.json` file.
//!
//! ## Per-request locales
//!
//! The translator is shared by every request, so the HTTP kernel runs each
//! request inside [`locale_scope`]: calling [`Lang::set_locale`] there only
//! changes the locale for that request.

mod facade;
mod loader;
mod provider;
mod selector;
mod translator;

pub use facade::{__, __with, Lang, trans, trans_choice, trans_choice_with, trans_with};
pub use loader::{ArrayLoader, FileLoader, Loader, framework_lines};
pub use provider::TranslationServiceProvider;
pub use selector::{ChoiceCount, MessageSelector};
pub use translator::{Translator, locale_scope, make_replacements, with_locale, with_locale_async};
