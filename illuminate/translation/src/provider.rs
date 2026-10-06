//! The translation service provider.

use std::sync::Arc;

use illuminate_config::{Config, Repository};
use illuminate_container::{Container, ServiceProvider};

use crate::loader::{FileLoader, Loader};
use crate::translator::{Translator, lang_path};

/// Registers the translation [`Loader`] (`dyn Loader`, a [`FileLoader`]
/// for `app.lang_path` layered on the framework's English lines) and the
/// [`Translator`], configured with `app.locale` and `app.fallback_locale`.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_config::Repository;
/// use illuminate_container::{Container, ServiceProvider};
/// use illuminate_translation::{Lang, TranslationServiceProvider, Translator};
/// use illuminate_support::json;
///
/// let app = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(app.clone());
/// app.instance(Repository::new(json!({
///     "app": {"locale": "es", "fallback_locale": "en", "lang_path": "/path/to/lang"},
/// })));
///
/// TranslationServiceProvider.register(&app);
///
/// assert_eq!(app.make::<Translator>().get_locale(), "es");
/// assert_eq!(Lang::get_fallback().as_deref(), Some("en"));
/// assert_eq!(Lang::get("auth.failed"), "These credentials do not match our records.");
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct TranslationServiceProvider;

impl ServiceProvider for TranslationServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<dyn Loader>(|app| {
            let loader: Arc<dyn Loader> =
                Arc::new(FileLoader::new(lang_path(&config_repository(app))));
            loader
        });
        app.singleton::<Translator>(|app| Arc::new(build_translator(app)));
    }
}

fn config_repository(app: &Container) -> Arc<Repository> {
    app.try_make::<Repository>()
        .unwrap_or_else(|_| Config::repository())
}

/// Build the translator from the container's loader and configuration.
pub(crate) fn build_translator(app: &Container) -> Translator {
    let config = config_repository(app);
    let translator = match app.try_make::<dyn Loader>() {
        Ok(loader) => Translator::with_loader_arc(loader, config.string_or("app.locale", "en")),
        Err(_) => Translator::new(lang_path(&config), config.string_or("app.locale", "en")),
    };
    translator.set_fallback(&config.string_or("app.fallback_locale", "en"));
    translator
}
