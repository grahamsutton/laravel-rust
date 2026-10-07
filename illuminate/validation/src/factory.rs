//! The validation factory (Laravel's `Illuminate\Validation\Factory`): it
//! creates validators and holds custom rule extensions, replacers, the
//! presence verifier and the message resolver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use indexmap::IndexMap;

use illuminate_container::{Container, ServiceProvider, try_app};
use illuminate_support::{Str, Value};

use illuminate_database::DatabaseManager;

use crate::database_presence::DatabasePresenceVerifier;
use crate::messages::MessageResolver;
use crate::presence::PresenceVerifier;
use crate::rule::ValidationContext;

/// A custom string rule registered with `Validator::extend`: receives the
/// attribute, its value, the rule parameters, and the validation context.
pub type ExtensionFn =
    Arc<dyn Fn(&str, &Value, &[String], &ValidationContext<'_>) -> bool + Send + Sync>;

/// A custom placeholder replacer registered with `Validator::replacer`:
/// receives the message, attribute, rule and parameters.
pub type ReplacerFn = Arc<dyn Fn(&str, &str, &str, &[String]) -> String + Send + Sync>;

/// The extensions a validator was created with.
#[derive(Clone, Default)]
pub(crate) struct Extensions {
    /// Custom rules by snake-cased name.
    pub(crate) rules: IndexMap<String, ExtensionFn>,
    /// Studly names of implicit extensions.
    pub(crate) implicit: Vec<String>,
    /// Studly names of dependent extensions.
    pub(crate) dependent: Vec<String>,
    /// Replacers by snake-cased rule name.
    pub(crate) replacers: IndexMap<String, ReplacerFn>,
    /// Fallback messages by snake-cased rule name.
    pub(crate) fallback_messages: IndexMap<String, Value>,
}

/// Creates validators and holds the shared validation configuration.
///
/// The [`crate::Validator`] type's associated functions (`Validator::make`,
/// `Validator::extend`, ...) resolve the factory from the container, so you
/// rarely need to touch it directly.
pub struct Factory {
    extensions: RwLock<Extensions>,
    verifier: RwLock<Option<Arc<dyn PresenceVerifier>>>,
    resolver: RwLock<Option<Arc<dyn MessageResolver>>>,
    exclude_unvalidated_array_keys: AtomicBool,
}

impl Default for Factory {
    fn default() -> Self {
        Self::new()
    }
}

impl Factory {
    /// Create a new factory.
    pub fn new() -> Self {
        Self {
            extensions: RwLock::new(Extensions::default()),
            verifier: RwLock::new(None),
            resolver: RwLock::new(None),
            exclude_unvalidated_array_keys: AtomicBool::new(true),
        }
    }

    /// The factory bound in the current container, registering one if needed.
    pub fn current() -> Arc<Factory> {
        let container = Container::get_instance();
        match container.try_make::<Factory>() {
            Ok(factory) => factory,
            Err(_) => container.instance(Factory::new()),
        }
    }

    pub(crate) fn extensions(&self) -> Extensions {
        self.extensions.read().unwrap().clone()
    }

    /// Register a custom rule.
    pub fn extend(
        &self,
        rule: &str,
        extension: impl Fn(&str, &Value, &[String], &ValidationContext<'_>) -> bool
        + Send
        + Sync
        + 'static,
    ) {
        self.extensions
            .write()
            .unwrap()
            .rules
            .insert(Str::snake(rule), Arc::new(extension));
    }

    /// Register a custom implicit rule (it runs even when the attribute is
    /// missing or empty).
    pub fn extend_implicit(
        &self,
        rule: &str,
        extension: impl Fn(&str, &Value, &[String], &ValidationContext<'_>) -> bool
        + Send
        + Sync
        + 'static,
    ) {
        self.extend(rule, extension);
        self.extensions
            .write()
            .unwrap()
            .implicit
            .push(Str::studly(rule));
    }

    /// Register a custom dependent rule (its parameters name other fields,
    /// and `*` in them is replaced with the current array index).
    pub fn extend_dependent(
        &self,
        rule: &str,
        extension: impl Fn(&str, &Value, &[String], &ValidationContext<'_>) -> bool
        + Send
        + Sync
        + 'static,
    ) {
        self.extend(rule, extension);
        self.extensions
            .write()
            .unwrap()
            .dependent
            .push(Str::studly(rule));
    }

    /// Set the fallback message of a custom rule.
    pub fn fallback_message(&self, rule: &str, message: &str) {
        self.extensions
            .write()
            .unwrap()
            .fallback_messages
            .insert(Str::snake(rule), Value::String(message.to_string()));
    }

    /// Register a custom placeholder replacer for a rule.
    pub fn replacer(
        &self,
        rule: &str,
        replacer: impl Fn(&str, &str, &str, &[String]) -> String + Send + Sync + 'static,
    ) {
        self.extensions
            .write()
            .unwrap()
            .replacers
            .insert(Str::snake(rule), Arc::new(replacer));
    }

    /// Set the presence verifier used by `unique` and `exists`.
    pub fn set_presence_verifier(&self, verifier: Arc<dyn PresenceVerifier>) {
        *self.verifier.write().unwrap() = Some(verifier);
    }

    /// The presence verifier: the factory's own, `dyn PresenceVerifier`
    /// from the container, or — when the container has a
    /// [`DatabaseManager`] — a [`DatabasePresenceVerifier`].
    pub fn presence_verifier(&self) -> Option<Arc<dyn PresenceVerifier>> {
        self.verifier
            .read()
            .unwrap()
            .clone()
            .or_else(try_app::<dyn PresenceVerifier>)
            .or_else(|| {
                try_app::<DatabaseManager>().map(|db| {
                    Arc::new(DatabasePresenceVerifier::new(db)) as Arc<dyn PresenceVerifier>
                })
            })
    }

    /// Set the message resolver (the translator hook).
    pub fn resolve_messages_using(&self, resolver: Arc<dyn MessageResolver>) {
        *self.resolver.write().unwrap() = Some(resolver);
    }

    /// The message resolver: the factory's own, or `dyn MessageResolver`
    /// from the container.
    pub fn message_resolver(&self) -> Option<Arc<dyn MessageResolver>> {
        self.resolver
            .read()
            .unwrap()
            .clone()
            .or_else(try_app::<dyn MessageResolver>)
    }

    /// Include array keys that weren't validated in `validated()` output.
    pub fn include_unvalidated_array_keys(&self) {
        self.exclude_unvalidated_array_keys
            .store(false, Ordering::SeqCst);
    }

    /// Exclude array keys that weren't validated from `validated()` output
    /// (the default).
    pub fn exclude_unvalidated_array_keys(&self) {
        self.exclude_unvalidated_array_keys
            .store(true, Ordering::SeqCst);
    }

    pub(crate) fn excludes_unvalidated_array_keys(&self) -> bool {
        self.exclude_unvalidated_array_keys.load(Ordering::SeqCst)
    }
}

/// Returned by `Validator::extend` so a default message can be attached.
pub struct PendingExtension {
    pub(crate) factory: Arc<Factory>,
    pub(crate) rule: String,
}

impl PendingExtension {
    /// Set the rule's default error message.
    pub fn message(self, message: &str) -> Self {
        self.factory.fallback_message(&self.rule, message);
        self
    }
}

/// Registers the validation [`Factory`] with the container, and — when the
/// application has a database ([`DatabaseManager`]) — binds the
/// [`DatabasePresenceVerifier`] as the default `dyn PresenceVerifier` used
/// by the `unique` and `exists` rules.
///
/// A `dyn PresenceVerifier` you bind yourself is never replaced. The message
/// resolver (`dyn MessageResolver`) is provided by the translation component
/// and picked up from the container.
pub struct ValidationServiceProvider;

impl ValidationServiceProvider {
    /// Bind the database presence verifier, if there is a database and no
    /// verifier has been bound yet.
    fn register_presence_verifier(app: &Container) {
        if app.bound::<DatabaseManager>() && !app.bound::<dyn PresenceVerifier>() {
            app.singleton::<dyn PresenceVerifier>(|container| {
                Arc::new(DatabasePresenceVerifier::new(
                    container.make::<DatabaseManager>(),
                ))
            });
        }
    }
}

impl ServiceProvider for ValidationServiceProvider {
    fn register(&self, app: &Container) {
        app.singleton::<Factory>(|_| Arc::new(Factory::new()));
        Self::register_presence_verifier(app);
    }

    /// The database may be registered after validation, so look again
    /// once every provider has been registered.
    fn boot(&self, app: &Container) {
        Self::register_presence_verifier(app);
    }
}
