//! The validator.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use illuminate_http::UploadedFile;
use illuminate_support::error::RuntimeException;
use illuminate_support::{Error, Map, MessageBag, Result, Value};

use crate::data::{self, Node};
use crate::exception::ValidationException;
use crate::factory::{Extensions, Factory, PendingExtension};
use crate::messages::MessageResolver;
use crate::parser::{self, Compiled};
use crate::php;
use crate::presence::PresenceVerifier;
use crate::rule::{ValidationContext, ValidationRule};
use crate::rules::{RuleSet, Rules};
use crate::validated_input::ValidatedInput;

/// Rules that run even when the attribute is missing or empty.
pub(crate) const IMPLICIT_RULES: &[&str] = &[
    "Accepted",
    "AcceptedIf",
    "Declined",
    "DeclinedIf",
    "Filled",
    "Missing",
    "MissingIf",
    "MissingUnless",
    "MissingWith",
    "MissingWithAll",
    "Present",
    "PresentIf",
    "PresentUnless",
    "PresentWith",
    "PresentWithAll",
    "Required",
    "RequiredIf",
    "RequiredIfAccepted",
    "RequiredIfDeclined",
    "RequiredUnless",
    "RequiredWith",
    "RequiredWithAll",
    "RequiredWithout",
    "RequiredWithoutAll",
];

/// Rules whose parameters name other fields.
pub(crate) const DEPENDENT_RULES: &[&str] = &[
    "After",
    "AfterOrEqual",
    "Before",
    "BeforeOrEqual",
    "Confirmed",
    "Different",
    "ExcludeIf",
    "ExcludeUnless",
    "ExcludeWith",
    "ExcludeWithout",
    "Gt",
    "Gte",
    "Lt",
    "Lte",
    "AcceptedIf",
    "DeclinedIf",
    "RequiredIf",
    "RequiredIfAccepted",
    "RequiredIfDeclined",
    "RequiredUnless",
    "RequiredWith",
    "RequiredWithAll",
    "RequiredWithout",
    "RequiredWithoutAll",
    "PresentIf",
    "PresentUnless",
    "PresentWith",
    "PresentWithAll",
    "Prohibited",
    "ProhibitedIf",
    "ProhibitedIfAccepted",
    "ProhibitedIfDeclined",
    "ProhibitedUnless",
    "Prohibits",
    "MissingIf",
    "MissingUnless",
    "MissingWith",
    "MissingWithAll",
    "Same",
    "Unique",
];

/// Rules that remove the attribute from the validated data.
pub(crate) const EXCLUDE_RULES: &[&str] = &[
    "Exclude",
    "ExcludeIf",
    "ExcludeUnless",
    "ExcludeWith",
    "ExcludeWithout",
];

/// Rules whose messages depend on the attribute's type.
pub(crate) const SIZE_RULES: &[&str] = &["Size", "Between", "Min", "Max", "Gt", "Lt", "Gte", "Lte"];

/// Rules that make an attribute "numeric" for size checks.
pub(crate) const NUMERIC_RULES: &[&str] = &["Numeric", "Integer", "Decimal"];

/// Rules that apply to uploaded files.
pub(crate) const FILE_RULES: &[&str] = &[
    "Between",
    "Dimensions",
    "Encoding",
    "Extensions",
    "File",
    "Image",
    "Max",
    "Mimes",
    "Mimetypes",
    "Min",
    "Size",
];

static NULL: Value = Value::Null;

/// A callback run after validation (`$validator->after(...)`).
pub type AfterHook = Arc<dyn Fn(&mut Validator) + Send + Sync>;

type SometimesCallback = Arc<dyn Fn(&Value, &Value) -> bool + Send + Sync>;

type AttributeFormatter = Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Clone)]
struct Sometimes {
    attributes: Vec<String>,
    rules: RuleSet,
    callback: SometimesCallback,
}

/// Custom error messages, keyed by `attribute.rule`, `rule`, or attribute
/// (wildcards like `items.*.name.required` work too).
#[derive(Clone, Debug, Default)]
pub struct CustomMessages(pub IndexMap<String, Value>);

impl<K: Into<String>, V: Into<Value>> FromIterator<(K, V)> for CustomMessages {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        CustomMessages(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl<K: Into<String>, V: Into<Value>, const N: usize> From<[(K, V); N]> for CustomMessages {
    fn from(pairs: [(K, V); N]) -> Self {
        pairs.into_iter().collect()
    }
}

impl<K: Into<String>, V: Into<Value>> From<Vec<(K, V)>> for CustomMessages {
    fn from(pairs: Vec<(K, V)>) -> Self {
        pairs.into_iter().collect()
    }
}

impl From<IndexMap<String, Value>> for CustomMessages {
    fn from(map: IndexMap<String, Value>) -> Self {
        CustomMessages(map)
    }
}

/// Custom attribute names (`"email" => "email address"`).
#[derive(Clone, Debug, Default)]
pub struct CustomAttributes(pub IndexMap<String, String>);

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for CustomAttributes {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        CustomAttributes(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl<K: Into<String>, V: Into<String>, const N: usize> From<[(K, V); N]> for CustomAttributes {
    fn from(pairs: [(K, V); N]) -> Self {
        pairs.into_iter().collect()
    }
}

impl<K: Into<String>, V: Into<String>> From<Vec<(K, V)>> for CustomAttributes {
    fn from(pairs: Vec<(K, V)>) -> Self {
        pairs.into_iter().collect()
    }
}

impl From<IndexMap<String, String>> for CustomAttributes {
    fn from(map: IndexMap<String, String>) -> Self {
        CustomAttributes(map)
    }
}

/// One or more attribute names (for `sometimes`).
pub trait IntoAttributeList {
    /// Convert into a list of attribute names.
    fn into_attribute_list(self) -> Vec<String>;
}

impl IntoAttributeList for &str {
    fn into_attribute_list(self) -> Vec<String> {
        vec![self.to_string()]
    }
}

impl IntoAttributeList for String {
    fn into_attribute_list(self) -> Vec<String> {
        vec![self]
    }
}

impl<S: Into<String>, const N: usize> IntoAttributeList for [S; N] {
    fn into_attribute_list(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}

impl<S: Into<String>> IntoAttributeList for Vec<S> {
    fn into_attribute_list(self) -> Vec<String> {
        self.into_iter().map(Into::into).collect()
    }
}

enum Outcome {
    Passed,
    Failed {
        rule: String,
        params: Vec<String>,
        extra_numeric: bool,
    },
    Uploaded,
    Custom {
        name: String,
        messages: Vec<String>,
    },
}

/// Validates data against a set of rules.
///
/// ```
/// use illuminate_validation::{Validator, rules};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let mut validator = Validator::make(
///     json!({"name": "", "email": "taylor@laravel.com", "role": "admin"}),
///     rules! {
///         "name" => "required|string|max:255",
///         "email" => "required|email",
///     },
/// );
///
/// assert!(validator.fails().await);
/// assert_eq!(validator.errors().first("name"), Some("The name field is required."));
///
/// let mut validator = Validator::make(
///     json!({"name": "Taylor", "email": "taylor@laravel.com", "role": "admin"}),
///     rules! { "name" => "required", "email" => "required|email" },
/// );
///
/// // Only the validated keys are returned.
/// assert_eq!(
///     validator.validate().await.unwrap(),
///     json!({"name": "Taylor", "email": "taylor@laravel.com"})
/// );
/// # });
/// ```
pub struct Validator {
    pub(crate) input: Value,
    pub(crate) files: Arc<Vec<UploadedFile>>,
    pub(crate) initial_rules: Rules,
    pub(crate) custom_messages: IndexMap<String, Value>,
    pub(crate) custom_attributes: IndexMap<String, String>,
    pub(crate) custom_values: IndexMap<String, IndexMap<String, String>>,
    after: Vec<AfterHook>,
    sometimes: Vec<Sometimes>,
    stop_on_first_failure: bool,
    exclude_unvalidated_array_keys: bool,
    pub(crate) implicit_attributes_formatter: Option<AttributeFormatter>,
    pub(crate) extensions: Extensions,
    pub(crate) verifier: Option<Arc<dyn PresenceVerifier>>,
    pub(crate) resolver: Option<Arc<dyn MessageResolver>>,

    pub(crate) data: Value,
    pub(crate) rules: IndexMap<String, Vec<Compiled>>,
    pub(crate) implicit_attributes: IndexMap<String, Vec<String>>,
    messages: MessageBag,
    failed_rules: IndexMap<String, IndexMap<String, Vec<String>>>,
    excluded: HashSet<String>,
    pub(crate) distinct_cache: Mutex<HashMap<String, Vec<(String, Value)>>>,
    ran: bool,
}

impl Validator {
    // ------------------------------------------------------------------
    // Creating validators (the `Validator` facade)
    // ------------------------------------------------------------------

    /// Create a new validator for the data and rules, using the factory in
    /// the current container.
    pub fn make(data: impl Into<Value>, rules: impl Into<Rules>) -> Validator {
        Self::with_factory(&Factory::current(), data.into(), rules.into())
    }

    /// Create a new validator using the given factory.
    pub fn with_factory(factory: &Factory, data: Value, rules: Rules) -> Validator {
        let data = match data {
            Value::Object(_) | Value::Array(_) => data,
            _ => Value::Object(Map::new()),
        };
        Validator {
            data: data.clone(),
            input: data,
            files: Arc::new(Vec::new()),
            initial_rules: rules,
            custom_messages: IndexMap::new(),
            custom_attributes: IndexMap::new(),
            custom_values: IndexMap::new(),
            after: Vec::new(),
            sometimes: Vec::new(),
            stop_on_first_failure: false,
            exclude_unvalidated_array_keys: factory.excludes_unvalidated_array_keys(),
            implicit_attributes_formatter: None,
            extensions: factory.extensions(),
            verifier: factory.presence_verifier(),
            resolver: factory.message_resolver(),
            rules: IndexMap::new(),
            implicit_attributes: IndexMap::new(),
            messages: MessageBag::new(),
            failed_rules: IndexMap::new(),
            excluded: HashSet::new(),
            distinct_cache: Mutex::new(HashMap::new()),
            ran: false,
        }
    }

    /// A validator for a rule object's own checks, sharing this
    /// validator's configuration, messages, attribute names and files.
    pub fn nested(&self, data: Value, rules: Rules) -> Validator {
        Validator {
            data: data.clone(),
            input: data,
            files: self.files.clone(),
            initial_rules: rules,
            custom_messages: self.custom_messages.clone(),
            custom_attributes: self.custom_attributes.clone(),
            custom_values: self.custom_values.clone(),
            after: Vec::new(),
            sometimes: Vec::new(),
            stop_on_first_failure: false,
            exclude_unvalidated_array_keys: self.exclude_unvalidated_array_keys,
            implicit_attributes_formatter: self.implicit_attributes_formatter.clone(),
            extensions: self.extensions.clone(),
            verifier: self.verifier.clone(),
            resolver: self.resolver.clone(),
            rules: IndexMap::new(),
            implicit_attributes: IndexMap::new(),
            messages: MessageBag::new(),
            failed_rules: IndexMap::new(),
            excluded: HashSet::new(),
            distinct_cache: Mutex::new(HashMap::new()),
            ran: false,
        }
    }

    /// Register a custom rule for every validator.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_container::Container;
    /// use illuminate_validation::Validator;
    /// use illuminate_support::json;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let _guard = Container::set_local_instance(Arc::new(Container::new()));
    ///
    /// Validator::extend("foo", |_attribute, value, _parameters, _validator| value == "foo")
    ///     .message("The :attribute must be foo.");
    ///
    /// let mut validator = Validator::make(json!({"name": "bar"}), [("name", "foo")]);
    /// assert!(validator.fails().await);
    /// assert_eq!(validator.errors().first("name"), Some("The name must be foo."));
    /// # });
    /// ```
    pub fn extend(
        rule: &str,
        extension: impl Fn(&str, &Value, &[String], &ValidationContext<'_>) -> bool
        + Send
        + Sync
        + 'static,
    ) -> PendingExtension {
        let factory = Factory::current();
        factory.extend(rule, extension);
        PendingExtension {
            factory,
            rule: rule.to_string(),
        }
    }

    /// Register a custom implicit rule (runs even for missing or empty values).
    pub fn extend_implicit(
        rule: &str,
        extension: impl Fn(&str, &Value, &[String], &ValidationContext<'_>) -> bool
        + Send
        + Sync
        + 'static,
    ) -> PendingExtension {
        let factory = Factory::current();
        factory.extend_implicit(rule, extension);
        PendingExtension {
            factory,
            rule: rule.to_string(),
        }
    }

    /// Register a custom dependent rule (its parameters name other fields).
    pub fn extend_dependent(
        rule: &str,
        extension: impl Fn(&str, &Value, &[String], &ValidationContext<'_>) -> bool
        + Send
        + Sync
        + 'static,
    ) -> PendingExtension {
        let factory = Factory::current();
        factory.extend_dependent(rule, extension);
        PendingExtension {
            factory,
            rule: rule.to_string(),
        }
    }

    /// Register a custom placeholder replacer for a rule: it receives the
    /// message, attribute, rule and parameters.
    pub fn replacer(
        rule: &str,
        replacer: impl Fn(&str, &str, &str, &[String]) -> String + Send + Sync + 'static,
    ) {
        Factory::current().replacer(rule, replacer);
    }

    /// Resolve validation messages using the given resolver (the
    /// translation component's hook).
    pub fn resolve_messages_using(resolver: impl MessageResolver + 'static) {
        Factory::current().resolve_messages_using(Arc::new(resolver));
    }

    /// Set the presence verifier used by `unique` and `exists` for every
    /// validator.
    pub fn set_presence_verifier(verifier: impl PresenceVerifier + 'static) {
        Factory::current().set_presence_verifier(Arc::new(verifier));
    }

    // ------------------------------------------------------------------
    // Configuration (builder)
    // ------------------------------------------------------------------

    /// Set custom error messages.
    ///
    /// Keys may be `attribute.rule` (`email.required`), a rule (`required`),
    /// or use wildcards (`items.*.name.required`).
    pub fn messages(mut self, messages: impl Into<CustomMessages>) -> Self {
        self.custom_messages.extend(messages.into().0);
        self
    }

    /// Set custom attribute names (`"email" => "email address"`).
    pub fn attributes(mut self, attributes: impl Into<CustomAttributes>) -> Self {
        self.custom_attributes.extend(attributes.into().0);
        self
    }

    /// Set custom displayable values: `attribute => {value => name}`.
    pub fn values<A, V, K, N>(mut self, values: impl IntoIterator<Item = (A, V)>) -> Self
    where
        A: Into<String>,
        V: IntoIterator<Item = (K, N)>,
        K: Into<String>,
        N: Into<String>,
    {
        for (attribute, names) in values {
            let names = names
                .into_iter()
                .map(|(k, n)| (k.into(), n.into()))
                .collect();
            self.custom_values.insert(attribute.into(), names);
        }
        self
    }

    /// Stop validating all attributes after the first failure.
    pub fn stop_on_first_failure(mut self) -> Self {
        self.stop_on_first_failure = true;
        self
    }

    /// Add a callback to run after validation — perfect for adding errors
    /// that depend on several fields.
    ///
    /// ```
    /// use illuminate_validation::Validator;
    /// use illuminate_support::json;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let mut validator = Validator::make(json!({"field": "x"}), [("field", "required")])
    ///     .after(|validator| {
    ///         validator.errors_mut().add("field", "Something is wrong with this field!");
    ///     });
    ///
    /// assert!(validator.fails().await);
    /// # });
    /// ```
    pub fn after(mut self, callback: impl Fn(&mut Validator) + Send + Sync + 'static) -> Self {
        self.after.push(Arc::new(callback));
        self
    }

    /// Add an "after" callback without consuming the validator.
    pub fn add_after(&mut self, callback: AfterHook) -> &mut Self {
        self.after.push(callback);
        self
    }

    /// Add rules to attributes only when the callback returns `true`. The
    /// callback receives all of the data, and the item an array attribute
    /// (`channels.*.address`) belongs to.
    ///
    /// ```
    /// use illuminate_validation::Validator;
    /// use illuminate_support::json;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let mut validator = Validator::make(json!({"games": 120}), [("games", "required|integer")])
    ///     .sometimes("reason", "required|max:500", |input, _| input["games"].as_i64() >= Some(100));
    ///
    /// assert!(validator.fails().await);
    /// assert_eq!(validator.errors().first("reason"), Some("The reason field is required."));
    /// # });
    /// ```
    pub fn sometimes(
        mut self,
        attributes: impl IntoAttributeList,
        rules: impl Into<RuleSet>,
        callback: impl Fn(&Value, &Value) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.sometimes.push(Sometimes {
            attributes: attributes.into_attribute_list(),
            rules: rules.into(),
            callback: Arc::new(callback),
        });
        self
    }

    /// Add more rules (appended to any existing rules for the attributes).
    pub fn add_rules(mut self, rules: impl Into<Rules>) -> Self {
        self.initial_rules = std::mem::take(&mut self.initial_rules).merge(rules);
        self
    }

    /// Include uploaded files in the data under validation, keyed by input
    /// name (`avatar`, `photos[]`, `user[avatar]`), exactly as returned by
    /// `Request::all_files()`.
    pub fn with_files(mut self, files: IndexMap<String, Vec<UploadedFile>>) -> Self {
        for (name, list) in files {
            if list.is_empty() {
                continue;
            }
            let (path, is_list) = data::bracket_to_dot(&name);
            let store = Arc::make_mut(&mut self.files);
            let mut placeholders = Vec::with_capacity(list.len());
            for file in list {
                placeholders.push(data::file_placeholder(store.len(), &file));
                store.push(file);
            }
            let value = if placeholders.len() == 1 && !is_list {
                placeholders.remove(0)
            } else {
                Value::Array(placeholders)
            };
            data::set(&mut self.input, &path, value);
        }
        self.data = self.input.clone();
        self
    }

    /// Include a single uploaded file in the data under validation.
    pub fn with_file(self, name: &str, file: UploadedFile) -> Self {
        let mut files = IndexMap::new();
        files.insert(name.to_string(), vec![file]);
        self.with_files(files)
    }

    /// Use the given presence verifier for `unique` and `exists`.
    pub fn presence_verifier(mut self, verifier: Arc<dyn PresenceVerifier>) -> Self {
        self.verifier = Some(verifier);
        self
    }

    /// Use the given message resolver for this validator.
    pub fn message_resolver(mut self, resolver: Arc<dyn MessageResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Exclude (the default) or include array keys that weren't validated
    /// when building the validated data.
    pub fn exclude_unvalidated_array_keys(mut self, exclude: bool) -> Self {
        self.exclude_unvalidated_array_keys = exclude;
        self
    }

    /// Format the names of array attributes (`items.0.name`) in messages.
    pub fn implicit_attributes_formatter(
        mut self,
        formatter: impl Fn(&str) -> String + Send + Sync + 'static,
    ) -> Self {
        self.implicit_attributes_formatter = Some(Arc::new(formatter));
        self
    }

    // ------------------------------------------------------------------
    // Running
    // ------------------------------------------------------------------

    /// Determine if the data passes the validation rules.
    ///
    /// # Panics
    ///
    /// Panics when a rule can't run (a missing presence verifier, a rule
    /// that doesn't exist, too few parameters...). Use
    /// [`Validator::try_passes`] to receive those errors instead.
    pub async fn passes(&mut self) -> bool {
        match self.try_passes().await {
            Ok(passes) => passes,
            Err(error) => panic!("{error}"),
        }
    }

    /// Determine if the data fails the validation rules.
    ///
    /// # Panics
    ///
    /// See [`Validator::passes`].
    pub async fn fails(&mut self) -> bool {
        !self.passes().await
    }

    /// Run the validation rules, returning an error when a rule can't run.
    pub async fn try_passes(&mut self) -> Result<bool> {
        self.prepare();

        let snapshot: Vec<(String, Vec<Compiled>)> = self
            .rules
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (attribute, rules) in &snapshot {
            if self.should_be_excluded(attribute) {
                self.remove_attribute(attribute);
                continue;
            }
            if self.stop_on_first_failure && self.messages.any() {
                break;
            }
            for rule in rules {
                self.validate_attribute(attribute, rule).await?;
                if self.should_be_excluded(attribute) || self.should_stop_validating(attribute) {
                    break;
                }
            }
        }

        for (attribute, _) in &snapshot {
            if self.should_be_excluded(attribute) {
                self.remove_attribute(attribute);
            }
        }

        let hooks = std::mem::take(&mut self.after);
        for hook in &hooks {
            hook(self);
        }
        let added = std::mem::replace(&mut self.after, hooks);
        self.after.extend(added);

        self.ran = true;
        Ok(self.messages.is_empty())
    }

    /// Run the validator, returning the validated data or a
    /// [`ValidationException`].
    ///
    /// # Panics
    ///
    /// See [`Validator::passes`]; use [`Validator::try_validate`] to get
    /// configuration errors as an [`Error`].
    pub async fn validate(&mut self) -> std::result::Result<Value, ValidationException> {
        if self.passes().await {
            Ok(self.validated_data().0)
        } else {
            Err(self.exception())
        }
    }

    /// Like [`Validator::validate`], storing errors in a named error bag.
    pub async fn validate_with_bag(
        &mut self,
        bag: &str,
    ) -> std::result::Result<Value, ValidationException> {
        self.validate().await.map_err(|e| e.error_bag(bag))
    }

    /// Run the validator, returning the validated data, or an error that is
    /// either a [`ValidationException`] or a configuration problem.
    pub async fn try_validate(&mut self) -> Result<Value> {
        if self.try_passes().await? {
            Ok(self.validated_data().0)
        } else {
            Err(self.exception().into())
        }
    }

    /// The exception describing the current errors.
    pub fn exception(&self) -> ValidationException {
        ValidationException::new(self.messages.clone())
    }

    // ------------------------------------------------------------------
    // Results
    // ------------------------------------------------------------------

    /// The error messages.
    pub fn errors(&self) -> &MessageBag {
        &self.messages
    }

    /// The error messages, mutably (for "after" callbacks).
    pub fn errors_mut(&mut self) -> &mut MessageBag {
        &mut self.messages
    }

    /// The failed rules, keyed by attribute then rule name, with their
    /// parameters.
    pub fn failed(&self) -> &IndexMap<String, IndexMap<String, Vec<String>>> {
        &self.failed_rules
    }

    /// Determine if validation has run.
    pub fn has_run(&self) -> bool {
        self.ran
    }

    /// The validated data — only the attributes with rules, nested and
    /// wildcard aware, without excluded attributes. Uploaded files are
    /// available through [`Validator::validated_files`].
    ///
    /// # Panics
    ///
    /// Panics when the validator hasn't run yet; call `passes().await`,
    /// `fails().await` or `validate().await` first.
    #[allow(clippy::result_large_err)]
    pub fn validated(&self) -> std::result::Result<Value, ValidationException> {
        self.ensure_ran();
        if self.messages.any() {
            return Err(self.exception());
        }
        Ok(self.validated_data().0)
    }

    /// The uploaded files that were validated, keyed by attribute.
    pub fn validated_files(&self) -> IndexMap<String, UploadedFile> {
        self.ensure_ran();
        self.validated_data().1
    }

    /// The validated data as a [`ValidatedInput`].
    ///
    /// # Panics
    ///
    /// See [`Validator::validated`].
    #[allow(clippy::result_large_err)]
    pub fn safe(&self) -> std::result::Result<ValidatedInput, ValidationException> {
        self.ensure_ran();
        if self.messages.any() {
            return Err(self.exception());
        }
        let (data, files) = self.validated_data();
        Ok(ValidatedInput::with_files(data, files))
    }

    /// The top-level attributes that passed validation.
    pub fn valid(&self) -> Value {
        let invalid: HashSet<String> = self.invalid_roots();
        match &self.data {
            Value::Object(map) => Value::Object(
                map.iter()
                    .filter(|(k, _)| !invalid.contains(*k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// The top-level attributes that failed validation.
    pub fn invalid(&self) -> Value {
        let invalid: HashSet<String> = self.invalid_roots();
        match &self.data {
            Value::Object(map) => Value::Object(
                map.iter()
                    .filter(|(k, _)| invalid.contains(*k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            _ => Value::Object(Map::new()),
        }
    }

    fn invalid_roots(&self) -> HashSet<String> {
        self.messages
            .keys()
            .into_iter()
            .map(|k| k.split('.').next().unwrap_or(k).to_string())
            .collect()
    }

    fn ensure_ran(&self) {
        assert!(
            self.ran,
            "The validator hasn't run yet. Call `passes().await`, `fails().await` or `validate().await` first."
        );
    }

    fn validated_data(&self) -> (Value, IndexMap<String, UploadedFile>) {
        let mut parent_keys: HashSet<String> = HashSet::new();
        if self.exclude_unvalidated_array_keys {
            for key in self.rules.keys() {
                let segments = data::segments_raw(key);
                for end in 1..segments.len() {
                    parent_keys.insert(segments[..end].join("."));
                }
            }
        }

        let mut node = Node::new();
        for (key, rules) in &self.rules {
            let value = data::get(&self.data, key);
            if self.exclude_unvalidated_array_keys
                && rules
                    .iter()
                    .any(|r| matches!(r.raw(), Some("array") | Some("list")))
                && value.is_some_and(|v| !v.is_null())
                && parent_keys.contains(key)
            {
                continue;
            }
            if let Some(value) = value {
                node.set(&data::segments(key), value.clone());
            }
        }

        let value = node.into_value();
        let mut found = Vec::new();
        data::collect_files(&value, "", &mut found);
        let files = found
            .into_iter()
            .filter_map(|(path, index)| self.files.get(index).map(|f| (path, f.clone())))
            .collect();
        let value = data::strip_files(value).unwrap_or_else(|| Value::Object(Map::new()));
        (value, files)
    }

    // ------------------------------------------------------------------
    // Data and rules
    // ------------------------------------------------------------------

    /// The data under validation (after exclusions, once validation has run).
    pub fn data(&self) -> &Value {
        &self.data
    }

    /// The value of an attribute (escaped dots allowed).
    pub(crate) fn value(&self, attribute: &str) -> Option<&Value> {
        data::get(&self.data, attribute)
    }

    /// Determine if the attribute is present in the data.
    pub(crate) fn has(&self, attribute: &str) -> bool {
        data::has(&self.data, attribute)
    }

    /// Resolve the uploaded file a value refers to.
    pub fn file_for(&self, value: &Value) -> Option<&UploadedFile> {
        data::file_index(value).and_then(|index| self.files.get(index))
    }

    /// Determine if the attribute has any of the given (studly-cased) rules.
    pub fn has_rule(&self, attribute: &str, rules: &[&str]) -> bool {
        self.rule_params(attribute, rules).is_some()
    }

    pub(crate) fn rule_params(&self, attribute: &str, rules: &[&str]) -> Option<Vec<String>> {
        self.rules.get(attribute)?.iter().find_map(|rule| {
            let name = rule.name()?;
            rules.contains(&name).then(|| rule.params())
        })
    }

    /// The expanded rules, keyed by concrete attribute (after validation ran).
    pub fn rule_names(&self) -> IndexMap<String, Vec<String>> {
        self.rules
            .iter()
            .map(|(k, rules)| {
                let names = rules
                    .iter()
                    .map(|r| match r {
                        Compiled::Custom(custom) => custom.name(),
                        other => other.name().unwrap_or_default().to_string(),
                    })
                    .collect();
                (k.clone(), names)
            })
            .collect()
    }

    fn prepare(&mut self) {
        self.data = self.input.clone();

        // A single uploaded file addressed with a wildcard (`photos.*`) is a list of one.
        for key in self
            .initial_rules
            .keys()
            .chain(self.sometimes.iter().flat_map(|s| s.attributes.iter()))
        {
            if !key.contains('*') {
                continue;
            }
            if let Some(lead) = data::leading_explicit_path(key)
                && let Some(value) = data::get(&self.data, &lead)
                && data::is_file(value)
            {
                let list = Value::Array(vec![value.clone()]);
                data::set(&mut self.data, &lead, list);
            }
        }

        let exploded = parser::explode(&self.initial_rules, &self.data);
        self.rules = exploded.rules;
        self.implicit_attributes = exploded.implicit_attributes;

        for sometimes in self.sometimes.clone() {
            for attribute in &sometimes.attributes {
                let exploded = parser::explode_one(attribute, &sometimes.rules, &self.data);
                for (pattern, keys) in exploded.implicit_attributes {
                    self.implicit_attributes.entry(pattern).or_insert(keys);
                }
                for (key, compiled) in exploded.rules {
                    let item = self.data_for_sometimes(&key, !attribute.ends_with(".*"));
                    if (sometimes.callback)(&self.data, &item) {
                        self.rules.entry(key).or_default().extend(compiled);
                    }
                }
            }
        }

        self.messages = MessageBag::new();
        self.failed_rules = IndexMap::new();
        self.excluded = HashSet::new();
        self.distinct_cache.lock().unwrap().clear();
    }

    fn data_for_sometimes(&self, attribute: &str, remove_last_segment: bool) -> Value {
        let mut segments = data::segments_raw(attribute);
        if remove_last_segment && segments.len() > 1 {
            segments.pop();
        }
        data::get(&self.data, &segments.join("."))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn should_be_excluded(&self, attribute: &str) -> bool {
        let segments = data::segments_raw(attribute);
        (1..=segments.len()).any(|end| self.excluded.contains(&segments[..end].join(".")))
    }

    fn remove_attribute(&mut self, attribute: &str) {
        data::forget(&mut self.data, attribute);
        self.rules.shift_remove(attribute);
    }

    fn should_stop_validating(&self, attribute: &str) -> bool {
        let cleaned = data::unescape(attribute);
        if self.has_rule(attribute, &["Bail"]) {
            return self.messages.has(&cleaned);
        }
        let failed = self.failed_rules.get(&cleaned);
        if failed.is_some_and(|f| f.contains_key("uploaded")) {
            return true;
        }
        let implicit = self.implicit_rule_names();
        self.has_rule(attribute, &implicit)
            && failed.is_some_and(|f| f.keys().any(|rule| implicit.contains(&rule.as_str())))
    }

    pub(crate) fn implicit_rule_names(&self) -> Vec<&str> {
        IMPLICIT_RULES
            .iter()
            .copied()
            .chain(self.extensions.implicit.iter().map(String::as_str))
            .collect()
    }

    fn is_implicit(&self, rule: &str) -> bool {
        IMPLICIT_RULES.contains(&rule) || self.extensions.implicit.iter().any(|r| r == rule)
    }

    fn depends_on_other_fields(&self, rule: &str) -> bool {
        DEPENDENT_RULES.contains(&rule) || self.extensions.dependent.iter().any(|r| r == rule)
    }

    /// The concrete keys matched by the wildcards of the attribute's rule
    /// (`items.3.name` from `items.*.name` gives `["3"]`).
    fn explicit_keys(&self, attribute: &str) -> Vec<String> {
        let primary = self.primary_attribute(attribute);
        if !primary.contains('*') {
            return Vec::new();
        }
        let pattern = data::segments_raw(&primary);
        let actual = data::segments_raw(attribute);
        pattern
            .iter()
            .zip(&actual)
            .filter(|(p, _)| *p == "*")
            .map(|(_, a)| a.clone())
            .collect()
    }

    fn is_validatable(
        &self,
        rule: &str,
        implicit: bool,
        attribute: &str,
        value: Option<&Value>,
    ) -> bool {
        if EXCLUDE_RULES.contains(&rule) {
            return true;
        }
        let present_or_implicit = match value {
            Some(Value::String(s)) if php::trim(s).is_empty() => implicit,
            _ => self.has(attribute) || implicit,
        };
        let passes_optional = !self.has_rule(attribute, &["Sometimes"]) || self.has(attribute);
        let not_null_if_nullable = implicit
            || !self.has_rule(attribute, &["Nullable"])
            || !matches!(value, Some(Value::Null));
        let not_failed_presence =
            !matches!(rule, "Unique" | "Exists") || !self.messages.has(&data::unescape(attribute));
        present_or_implicit && passes_optional && not_null_if_nullable && not_failed_presence
    }

    async fn validate_attribute(&mut self, attribute: &str, rule: &Compiled) -> Result<()> {
        let outcome = self.evaluate(attribute, rule).await?;
        match outcome {
            Outcome::Passed => {}
            Outcome::Uploaded => self.record_failure(attribute, "uploaded", Vec::new(), false),
            Outcome::Failed {
                rule,
                params,
                extra_numeric,
            } => self.record_failure(attribute, &rule, params, extra_numeric),
            Outcome::Custom { name, messages } => {
                self.record_custom_failure(attribute, &name, messages)
            }
        }
        Ok(())
    }

    async fn evaluate(&self, attribute: &str, rule: &Compiled) -> Result<Outcome> {
        let value = self.value(attribute);

        if let Some(file) = value.and_then(|v| self.file_for(v)) {
            let mut file_or_implicit: Vec<&str> = FILE_RULES.to_vec();
            file_or_implicit.extend(self.implicit_rule_names());
            if !file.is_valid() && self.has_rule(attribute, &file_or_implicit) {
                return Ok(Outcome::Uploaded);
            }
        }

        match rule {
            Compiled::Custom(custom) => {
                self.evaluate_custom(attribute, custom.as_ref(), value)
                    .await
            }
            _ => {
                let name = rule.name().unwrap_or_default().to_string();
                let mut params = rule.params();
                if self.depends_on_other_fields(&name) {
                    let keys = self.explicit_keys(attribute);
                    if !keys.is_empty() {
                        params = params
                            .into_iter()
                            .map(|p| replace_asterisks(&p, &keys))
                            .collect();
                    }
                }
                let implicit = self.is_implicit(&name);
                if !self.is_validatable(&name, implicit, attribute, value) {
                    return Ok(Outcome::Passed);
                }
                let extra_numeric = matches!(name.as_str(), "Gt" | "Gte" | "Lt" | "Lte")
                    && value.is_some_and(php::is_numeric);
                let passed = self
                    .validate_rule(
                        &name,
                        attribute,
                        value.unwrap_or(&NULL),
                        &params,
                        rule,
                        extra_numeric,
                    )
                    .await?;
                Ok(if passed {
                    Outcome::Passed
                } else {
                    Outcome::Failed {
                        rule: name,
                        params,
                        extra_numeric,
                    }
                })
            }
        }
    }

    async fn evaluate_custom(
        &self,
        attribute: &str,
        custom: &dyn ValidationRule,
        value: Option<&Value>,
    ) -> Result<Outcome> {
        let implicit = custom.implicit();
        let present_or_implicit = match value {
            Some(Value::String(s)) if php::trim(s).is_empty() => implicit,
            _ => self.has(attribute) || implicit,
        };
        let passes_optional = !self.has_rule(attribute, &["Sometimes"]) || self.has(attribute);
        let not_null_if_nullable = implicit
            || !self.has_rule(attribute, &["Nullable"])
            || !matches!(value, Some(Value::Null));
        if !(present_or_implicit && passes_optional && not_null_if_nullable) {
            return Ok(Outcome::Passed);
        }

        let context = ValidationContext::new(self, attribute);
        let display = data::unescape(attribute);
        let mut messages: Vec<String> = Vec::new();
        {
            let mut fail = |message: &str| messages.push(message.to_string());
            custom
                .validate_with(&display, value.unwrap_or(&NULL), &context, &mut fail)
                .await;
        }
        if let Some(error) = context.take_error() {
            return Err(error);
        }
        Ok(if messages.is_empty() {
            Outcome::Passed
        } else {
            Outcome::Custom {
                name: custom.name(),
                messages,
            }
        })
    }

    fn record_failure(
        &mut self,
        attribute: &str,
        rule: &str,
        params: Vec<String>,
        extra_numeric: bool,
    ) {
        if EXCLUDE_RULES.contains(&rule) {
            self.excluded.insert(attribute.to_string());
            return;
        }
        let key = data::unescape(attribute);
        let message = self.get_message(attribute, rule, extra_numeric);
        let message = self.make_replacements(&message, attribute, rule, &params, extra_numeric);
        self.messages.add(key.clone(), message);
        self.failed_rules
            .entry(key)
            .or_default()
            .insert(rule.to_string(), params);
    }

    fn record_custom_failure(&mut self, attribute: &str, name: &str, messages: Vec<String>) {
        let key = data::unescape(attribute);
        self.failed_rules
            .entry(key.clone())
            .or_default()
            .insert(name.to_string(), Vec::new());
        let attribute_type = self.attribute_type(attribute, false);
        let messages =
            match self.get_from_local_array(&key, name, &self.custom_messages, attribute_type) {
                Some(Value::String(message)) => vec![message],
                _ if messages.iter().all(|m| m.is_empty()) => vec![name.to_string()],
                _ => messages,
            };
        for message in messages {
            let message = self.replace_common(&message, attribute);
            self.messages.add(key.clone(), message);
        }
    }

    /// Add a failure for the attribute and rule, with the rule's message
    /// (Laravel's `addFailure`). Rules may be studly (`"Required"`) or a
    /// message key (`"password.mixed"`).
    pub fn add_failure(&mut self, attribute: &str, rule: &str, parameters: &[String]) {
        self.record_failure(attribute, rule, parameters.to_vec(), false);
    }

    /// A configuration error raised by a rule.
    pub(crate) fn runtime_error(message: impl Into<String>) -> Error {
        RuntimeException::new(message).into()
    }
}

/// Replace each `*` in a parameter with the next concrete key.
fn replace_asterisks(parameter: &str, keys: &[String]) -> String {
    let mut keys = keys.iter();
    let mut out = String::new();
    for c in parameter.chars() {
        if c == '*' {
            match keys.next() {
                Some(key) => out.push_str(key),
                None => out.push('*'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_container::Container;
    use illuminate_support::json;

    fn assert_send<T: Send>(_: &T) {}
    fn assert_sync<T: Sync>(_: &T) {}

    #[test]
    fn validators_and_their_futures_are_send() {
        let _guard = Container::set_local_instance(Arc::new(Container::new()));
        let mut validator = Validator::make(json!({}), Rules::new());
        assert_sync(&validator);
        assert_send(&validator);
        let future = validator.passes();
        assert_send(&future);
    }

    #[test]
    fn asterisks_are_replaced_in_order() {
        let keys = vec!["1".to_string(), "2".to_string()];
        assert_eq!(replace_asterisks("a.*.b.*", &keys), "a.1.b.2");
        assert_eq!(replace_asterisks("a.*.b.*.c.*", &keys), "a.1.b.2.c.*");
    }
}
