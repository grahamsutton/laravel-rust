//! The contract for custom validation rules.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use illuminate_http::UploadedFile;
use illuminate_support::{Error, Value};

use crate::data;
use crate::validator::Validator;

/// The `$fail` callback handed to rules: call it with a message to mark the
/// attribute as invalid. `:attribute` and friends are replaced for you.
///
/// ```
/// # use illuminate_validation::FailCallback;
/// fn check(value: &str, fail: &mut FailCallback<'_>) {
///     if value.to_uppercase() != value {
///         fail("The :attribute must be uppercase.");
///     }
/// }
/// ```
pub type FailCallback<'a> = dyn FnMut(&str) + Send + 'a;

/// A custom validation rule object.
///
/// Implement [`ValidationRule::validate`] and call `fail` with a message
/// when the value is invalid, exactly like Laravel's `ValidationRule`:
///
/// ```
/// use illuminate_validation::{async_trait, FailCallback, ValidationRule, Validator, rules};
/// use illuminate_support::{json, Value};
///
/// struct Uppercase;
///
/// #[async_trait]
/// impl ValidationRule for Uppercase {
///     async fn validate(&self, _attribute: &str, value: &Value, fail: &mut FailCallback<'_>) {
///         if value.as_str().is_some_and(|v| v.to_uppercase() != v) {
///             fail("The :attribute must be uppercase.");
///         }
///     }
/// }
///
/// # tokio_test(async {
/// let mut validator = Validator::make(json!({"name": "taylor"}), rules! {
///     "name" => ["required", "string", Uppercase],
/// });
///
/// assert!(validator.fails().await);
/// assert_eq!(validator.errors().first("name"), Some("The name must be uppercase."));
/// # });
/// # fn tokio_test(f: impl std::future::Future<Output = ()>) {
/// #     tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
/// # }
/// ```
///
/// Rules that need the rest of the data under validation (Laravel's
/// `DataAwareRule` / `ValidatorAwareRule`) implement
/// [`ValidationRule::validate_with`] instead, which also receives a
/// [`ValidationContext`].
#[async_trait]
pub trait ValidationRule: Send + Sync {
    /// Run the validation rule. Implement this (or
    /// [`ValidationRule::validate_with`]); the default accepts everything.
    async fn validate(&self, attribute: &str, value: &Value, fail: &mut FailCallback<'_>) {
        let _ = (attribute, value, fail);
    }

    /// Run the validation rule with access to all of the data under
    /// validation. Defaults to calling [`ValidationRule::validate`].
    async fn validate_with(
        &self,
        attribute: &str,
        value: &Value,
        context: &ValidationContext<'_>,
        fail: &mut FailCallback<'_>,
    ) {
        let _ = context;
        self.validate(attribute, value, fail).await
    }

    /// "Implicit" rules run even when the attribute is missing or empty.
    fn implicit(&self) -> bool {
        false
    }

    /// The rule's name, used as the key in `failed()` and for custom
    /// messages (`"name.uppercase"`). Defaults to the type's name.
    fn name(&self) -> String {
        let full = std::any::type_name::<Self>();
        let without_generics = full.split('<').next().unwrap_or(full);
        without_generics
            .rsplit("::")
            .next()
            .unwrap_or(without_generics)
            .to_string()
    }
}

/// The view a rule gets of the validation in progress.
pub struct ValidationContext<'a> {
    pub(crate) validator: &'a Validator,
    pub(crate) attribute: &'a str,
    pub(crate) error: Mutex<Option<Error>>,
}

impl<'a> ValidationContext<'a> {
    pub(crate) fn new(validator: &'a Validator, attribute: &'a str) -> Self {
        Self {
            validator,
            attribute,
            error: Mutex::new(None),
        }
    }

    /// Abort the whole validation with an error (Laravel: throwing an
    /// exception from inside a rule). `try_passes` returns it.
    pub fn abort(&self, error: impl Into<Error>) {
        *self.error.lock().unwrap() = Some(error.into());
    }

    pub(crate) fn take_error(&self) -> Option<Error> {
        self.error.lock().unwrap().take()
    }

    /// All of the data under validation.
    pub fn data(&self) -> &'a Value {
        self.validator.data()
    }

    /// Get a value from the data under validation using "dot" notation
    /// (`null` when missing).
    pub fn input(&self, key: &str) -> Value {
        data::get(self.validator.data(), key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Determine if the data under validation contains the given key.
    pub fn has(&self, key: &str) -> bool {
        data::has(self.validator.data(), key)
    }

    /// Get an uploaded file from the data under validation.
    pub fn file(&self, key: &str) -> Option<&'a UploadedFile> {
        data::get(self.validator.data(), key).and_then(|value| self.validator.file_for(value))
    }

    /// Resolve the uploaded file a value refers to (files are represented
    /// in the data by small placeholder objects).
    pub fn uploaded_file(&self, value: &Value) -> Option<&'a UploadedFile> {
        self.validator.file_for(value)
    }

    /// The attribute under validation, with escaped dots (`v1\.0`).
    pub fn attribute(&self) -> &str {
        self.attribute
    }

    /// Determine if the given attribute has any of the given rules
    /// (`context.has_rule("password", &["Nullable"])`).
    pub fn has_rule(&self, attribute: &str, rules: &[&str]) -> bool {
        self.validator.has_rule(attribute, rules)
    }

    /// The validator running this rule.
    pub fn validator(&self) -> &'a Validator {
        self.validator
    }
}

type ClosureFn = dyn Fn(&str, &Value, &mut FailCallback<'_>) + Send + Sync;

/// A rule defined by a closure (`Rule::closure(...)`).
#[derive(Clone)]
pub struct ClosureRule {
    callback: Arc<ClosureFn>,
    implicit: bool,
}

impl ClosureRule {
    /// Create a closure rule.
    pub fn new(
        callback: impl Fn(&str, &Value, &mut FailCallback<'_>) + Send + Sync + 'static,
    ) -> Self {
        Self {
            callback: Arc::new(callback),
            implicit: false,
        }
    }

    /// Run this closure even when the attribute is missing or empty.
    pub fn implicit(mut self) -> Self {
        self.implicit = true;
        self
    }
}

#[async_trait]
impl ValidationRule for ClosureRule {
    async fn validate(&self, attribute: &str, value: &Value, fail: &mut FailCallback<'_>) {
        (self.callback)(attribute, value, fail);
    }

    fn implicit(&self) -> bool {
        self.implicit
    }

    fn name(&self) -> String {
        "ClosureValidationRule".to_string()
    }
}

impl std::fmt::Debug for ClosureRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosureRule")
            .field("implicit", &self.implicit)
            .finish()
    }
}

/// Verifies the authenticated user's password for the `current_password`
/// rule. The auth component binds an implementation as
/// `dyn CurrentPasswordVerifier` in the container.
#[async_trait]
pub trait CurrentPasswordVerifier: Send + Sync {
    /// Determine if the given password matches the current user's password
    /// on the given guard (the default guard when `None`). Guests fail.
    async fn check(&self, guard: Option<&str>, password: &str) -> bool;
}

/// Checks passwords against known data leaks for `Password::uncompromised()`.
/// Bind an implementation as `dyn UncompromisedVerifier`; without one,
/// `uncompromised()` is a no-op.
#[async_trait]
pub trait UncompromisedVerifier: Send + Sync {
    /// Determine if the password has appeared in data leaks no more than
    /// `threshold` times.
    async fn verify(&self, password: &str, threshold: usize) -> bool;
}
