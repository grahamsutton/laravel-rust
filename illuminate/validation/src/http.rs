//! Validating HTTP requests: `request.validate(rules)` and form requests.

use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;

use illuminate_http::{HttpException, Request, UploadedFile};
use illuminate_support::{Result, Value};

use crate::exception::ValidationException;
use crate::rules::Rules;
use crate::validated_input::ValidatedInput;
use crate::validator::{CustomAttributes, CustomMessages, Validator};

/// The data that passed validation for a request, stored as a request
/// extension so `request.validated()` can return it later.
#[derive(Clone, Debug, Default)]
pub struct ValidatedRequestData {
    /// The validated input.
    pub data: Value,
    /// The validated uploaded files, keyed by attribute.
    pub files: IndexMap<String, UploadedFile>,
}

/// Validation helpers for [`Request`] — Laravel's `$request->validate()`.
///
/// ```
/// use illuminate_http::Request;
/// use illuminate_validation::{ValidatesRequests, rules};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let request = Request::create_with("/posts", "POST", json!({"title": "Hello", "draft": true}), Default::default());
///
/// let validated = request.validate(rules! { "title" => "required|max:255" }).await.unwrap();
///
/// assert_eq!(validated, json!({"title": "Hello"}));
/// assert_eq!(request.validated(), json!({"title": "Hello"}));
/// # });
/// ```
#[async_trait]
pub trait ValidatesRequests {
    /// Create a validator for the request's input (and files).
    fn validator(&self, rules: impl Into<Rules>) -> Validator;

    /// Validate the request, returning the validated data. Failures return
    /// a [`ValidationException`] (as an `Error`) for the exception handler.
    async fn validate<R: Into<Rules> + Send>(&self, rules: R) -> Result<Value>;

    /// Validate the request, flashing errors into a named error bag.
    async fn validate_with_bag<R: Into<Rules> + Send>(&self, bag: &str, rules: R) -> Result<Value>;

    /// Validate with custom messages and attribute names.
    async fn validate_with<R, M, A>(&self, rules: R, messages: M, attributes: A) -> Result<Value>
    where
        R: Into<Rules> + Send,
        M: Into<CustomMessages> + Send,
        A: Into<CustomAttributes> + Send;

    /// The data validated by the last successful `validate` call.
    fn validated(&self) -> Value;

    /// The data validated by the last successful `validate` call, as a
    /// [`ValidatedInput`].
    fn safe(&self) -> ValidatedInput;
}

async fn run(request: &Request, mut validator: Validator, bag: Option<&str>) -> Result<Value> {
    match validator.try_validate().await {
        Ok(data) => {
            let files = validator.validated_files();
            request.set_extension(Arc::new(ValidatedRequestData {
                data: data.clone(),
                files,
            }));
            Ok(data)
        }
        Err(error) => match (error.downcast::<ValidationException>(), bag) {
            (Ok(exception), Some(bag)) => Err(exception.error_bag(bag).into()),
            (Ok(exception), None) => Err(exception.into()),
            (Err(error), _) => Err(error),
        },
    }
}

#[async_trait]
impl ValidatesRequests for Request {
    fn validator(&self, rules: impl Into<Rules>) -> Validator {
        Validator::make(self.all(), rules).with_files(self.all_files())
    }

    async fn validate<R: Into<Rules> + Send>(&self, rules: R) -> Result<Value> {
        let validator = ValidatesRequests::validator(self, rules);
        run(self, validator, None).await
    }

    async fn validate_with_bag<R: Into<Rules> + Send>(&self, bag: &str, rules: R) -> Result<Value> {
        let validator = ValidatesRequests::validator(self, rules);
        run(self, validator, Some(bag)).await
    }

    async fn validate_with<R, M, A>(&self, rules: R, messages: M, attributes: A) -> Result<Value>
    where
        R: Into<Rules> + Send,
        M: Into<CustomMessages> + Send,
        A: Into<CustomAttributes> + Send,
    {
        let validator = ValidatesRequests::validator(self, rules)
            .messages(messages)
            .attributes(attributes);
        run(self, validator, None).await
    }

    fn validated(&self) -> Value {
        self.extension::<ValidatedRequestData>()
            .map(|validated| validated.data.clone())
            .unwrap_or_else(|| Value::Object(Default::default()))
    }

    fn safe(&self) -> ValidatedInput {
        match self.extension::<ValidatedRequestData>() {
            Some(validated) => {
                ValidatedInput::with_files(validated.data.clone(), validated.files.clone())
            }
            None => ValidatedInput::default(),
        }
    }
}

/// A form request: a type that encapsulates the authorization and
/// validation rules for a request.
///
/// ```
/// use illuminate_http::Request;
/// use illuminate_validation::{FormRequest, Rules, CustomMessages, rules, validate_form_request};
/// use illuminate_support::json;
///
/// #[derive(Default)]
/// struct StorePostRequest;
///
/// impl FormRequest for StorePostRequest {
///     fn rules(&self, _request: &Request) -> Rules {
///         rules! {
///             "title" => "required|max:255",
///             "body" => "required",
///         }
///     }
///
///     fn messages(&self) -> CustomMessages {
///         [("title.required", "A title is required")].into()
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let request = Request::create_with("/posts", "POST", json!({"title": "Hi", "body": "..."}), Default::default());
/// let validated = validate_form_request::<StorePostRequest>(&request).await.unwrap();
/// assert_eq!(validated.validated(), &json!({"title": "Hi", "body": "..."}));
/// # });
/// ```
#[async_trait]
pub trait FormRequest: Send + Sync + 'static {
    /// The validation rules that apply to the request.
    fn rules(&self, request: &Request) -> Rules;

    /// Determine if the user is authorized to make this request. A `false`
    /// result aborts with a `403 This action is unauthorized.`
    async fn authorize(&self, _request: &Request) -> bool {
        true
    }

    /// Custom error messages.
    fn messages(&self) -> CustomMessages {
        CustomMessages::default()
    }

    /// Custom attribute names.
    fn attributes(&self) -> CustomAttributes {
        CustomAttributes::default()
    }

    /// Prepare the data for validation (merge or normalize input).
    fn prepare_for_validation(&self, _request: &Request) {}

    /// The data to validate (all of the request's input by default).
    fn validation_data(&self, request: &Request) -> Value {
        request.all()
    }

    /// Customize the validator before it runs.
    fn with_validator(&self, validator: Validator, _request: &Request) -> Validator {
        validator
    }

    /// Additional validation, run after the rules (add errors with
    /// `validator.errors_mut().add(...)`).
    fn after(&self, _validator: &mut Validator, _request: &Request) {}

    /// Handle a passed validation attempt.
    fn passed_validation(&self, _request: &Request) {}

    /// Stop validating every attribute after the first failure.
    fn stop_on_first_failure(&self) -> bool {
        false
    }

    /// The error bag errors are flashed into.
    fn error_bag(&self) -> String {
        "default".to_string()
    }

    /// Where to redirect when validation fails (the previous URL by default).
    fn redirect_to(&self, _request: &Request) -> Option<String> {
        None
    }
}

/// A validated form request: the form request itself, the request, and
/// the data that passed validation. Ready to be used as a route handler
/// argument once the router bridges it.
pub struct Validated<T> {
    form: T,
    request: Request,
    data: Value,
    files: IndexMap<String, UploadedFile>,
}

impl<T: FormRequest> Validated<T> {
    /// The validated data.
    pub fn validated(&self) -> &Value {
        &self.data
    }

    /// A single validated value using dot notation (`null` when missing).
    pub fn validated_key(&self, key: &str) -> Value {
        crate::data::get(&self.data, key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// The validated data as a [`ValidatedInput`].
    pub fn safe(&self) -> ValidatedInput {
        ValidatedInput::with_files(self.data.clone(), self.files.clone())
    }

    /// An input value from the request (validated or not).
    pub fn input(&self, key: &str) -> Value {
        self.request.input(key)
    }

    /// A validated uploaded file.
    pub fn file(&self, key: &str) -> Option<&UploadedFile> {
        self.files.get(key)
    }

    /// The underlying request.
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// The form request.
    pub fn form(&self) -> &T {
        &self.form
    }

    /// Take the form request out of the wrapper.
    pub fn into_inner(self) -> T {
        self.form
    }
}

impl<T> std::ops::Deref for Validated<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.form
    }
}

/// Authorize and validate a request with the form request `T`.
///
/// Fails with an `HttpException` (403) when unauthorized, or a
/// [`ValidationException`] (carrying the form request's error bag and
/// redirect) when the data is invalid.
pub async fn validate_form_request<T: FormRequest + Default>(
    request: &Request,
) -> Result<Validated<T>> {
    validate_form_request_with(T::default(), request).await
}

/// Like [`validate_form_request`], with an existing form request instance.
pub async fn validate_form_request_with<T: FormRequest>(
    form: T,
    request: &Request,
) -> Result<Validated<T>> {
    form.prepare_for_validation(request);

    if !form.authorize(request).await {
        return Err(HttpException::with_message(403, "This action is unauthorized.").into());
    }

    let mut validator = Validator::make(form.validation_data(request), form.rules(request))
        .with_files(request.all_files())
        .messages(form.messages())
        .attributes(form.attributes());
    if form.stop_on_first_failure() {
        validator = validator.stop_on_first_failure();
    }
    let mut validator = form.with_validator(validator, request);

    validator.try_passes().await?;
    form.after(&mut validator, request);

    if validator.errors().any() {
        let mut exception = validator.exception().error_bag(form.error_bag());
        if let Some(url) = form.redirect_to(request) {
            exception = exception.redirect_to(url);
        }
        return Err(exception.into());
    }

    let data = validator.validated()?;
    let files = validator.validated_files();
    form.passed_validation(request);
    request.set_extension(Arc::new(ValidatedRequestData {
        data: data.clone(),
        files: files.clone(),
    }));

    Ok(Validated {
        form,
        request: request.clone(),
        data,
        files,
    })
}
