//! The exception thrown when validation fails.

use indexmap::IndexMap;

use illuminate_http::{IntoResponse, Request, Response, render_exception};
use illuminate_support::{MessageBag, Value, json};

/// Input keys that are never flashed back to the session.
const DONT_FLASH: [&str; 3] = ["current_password", "password", "password_confirmation"];

/// Thrown when validation fails. The exception handler renders it as a
/// `422` JSON response (`{"message": ..., "errors": {...}}`) for XHR
/// requests, or as a redirect back with the errors and old input.
///
/// ```
/// use illuminate_validation::ValidationException;
///
/// let exception = ValidationException::with_messages([
///     ("email", "The provided credentials are incorrect."),
///     ("password", "The password is too weak."),
/// ]);
///
/// assert_eq!(exception.status, 422);
/// assert_eq!(exception.message(), "The provided credentials are incorrect. (and 1 more error)");
/// assert_eq!(exception.errors.first("password"), Some("The password is too weak."));
/// ```
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ValidationException {
    /// The validation errors.
    pub errors: MessageBag,
    /// The HTTP status code (422 by default).
    pub status: u16,
    /// The name of the error bag the errors are flashed into.
    pub error_bag: String,
    /// Where to redirect to (instead of the previous URL).
    pub redirect_to: Option<String>,
    message: String,
}

impl ValidationException {
    /// Create an exception for the given errors.
    pub fn new(errors: MessageBag) -> Self {
        let message = Self::summarize(&errors);
        Self {
            errors,
            status: 422,
            error_bag: "default".to_string(),
            redirect_to: None,
            message,
        }
    }

    /// Create an exception from error messages keyed by attribute.
    pub fn with_messages<K, V>(messages: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<Messages>,
    {
        let mut bag = MessageBag::new();
        for (key, value) in messages {
            let key = key.into();
            for message in value.into().0 {
                bag.add(key.clone(), message);
            }
        }
        Self::new(bag)
    }

    /// "The name field is required. (and 2 more errors)"
    fn summarize(errors: &MessageBag) -> String {
        let messages = errors.all();
        let Some(first) = messages.first() else {
            return "The given data was invalid.".to_string();
        };
        match messages.len() - 1 {
            0 => first.to_string(),
            1 => format!("{first} (and 1 more error)"),
            n => format!("{first} (and {n} more errors)"),
        }
    }

    /// The summary message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The errors, keyed by attribute.
    pub fn errors(&self) -> &IndexMap<String, Vec<String>> {
        self.errors.messages()
    }

    /// Set the HTTP status code.
    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// Set the error bag the errors are flashed into.
    pub fn error_bag(mut self, bag: impl Into<String>) -> Self {
        self.error_bag = bag.into();
        self
    }

    /// Set the URL to redirect to.
    pub fn redirect_to(mut self, url: impl Into<String>) -> Self {
        self.redirect_to = Some(url.into());
        self
    }

    /// The JSON payload rendered for XHR requests.
    pub fn to_json(&self) -> Value {
        json!({
            "message": self.message,
            "errors": self.errors.messages(),
        })
    }

    /// Render the exception for the given request — what the foundation's
    /// exception handler does with it: a JSON response for requests that
    /// expect JSON, otherwise a redirect (to `redirect_to`, the `Referer`,
    /// or `/`) flashing the errors and the input (minus passwords).
    pub fn render(&self, request: &Request) -> Response {
        if request.expects_json() {
            return Response::json(&self.to_json()).with_status(self.status);
        }
        let to = self
            .redirect_to
            .clone()
            .or_else(|| request.header("referer"))
            .unwrap_or_else(|| "/".to_string());
        let mut input = request.all();
        if let Value::Object(map) = &mut input {
            for key in DONT_FLASH {
                map.shift_remove(key);
            }
        }
        let bag = match request.input("_error_bag") {
            Value::String(bag) if !bag.is_empty() => bag,
            _ => self.error_bag.clone(),
        };
        Response::redirect(to)
            .with_input(input)
            .with_errors_in(self.errors.clone(), &bag)
    }
}

impl IntoResponse for ValidationException {
    fn into_response(self) -> Response {
        render_exception(self.into())
    }
}

/// One or more messages for an attribute.
#[derive(Clone, Debug, Default)]
pub struct Messages(pub Vec<String>);

impl From<&str> for Messages {
    fn from(message: &str) -> Self {
        Messages(vec![message.to_string()])
    }
}

impl From<String> for Messages {
    fn from(message: String) -> Self {
        Messages(vec![message])
    }
}

impl<S: Into<String>> From<Vec<S>> for Messages {
    fn from(messages: Vec<S>) -> Self {
        Messages(messages.into_iter().map(Into::into).collect())
    }
}

impl<S: Into<String>, const N: usize> From<[S; N]> for Messages {
    fn from(messages: [S; N]) -> Self {
        Messages(messages.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_summarizes_errors() {
        let one = ValidationException::with_messages([("name", "The name field is required.")]);
        assert_eq!(one.message(), "The name field is required.");
        let three = ValidationException::with_messages([
            ("name", vec!["The name field is required.", "Another."]),
            ("email", vec!["Bad email."]),
        ]);
        assert_eq!(
            three.message(),
            "The name field is required. (and 2 more errors)"
        );
        assert_eq!(
            ValidationException::new(MessageBag::new()).message(),
            "The given data was invalid."
        );
    }

    #[test]
    fn it_is_configurable() {
        let exception = ValidationException::with_messages([("email", "Taken.")])
            .error_bag("login")
            .redirect_to("/login")
            .status(400);
        assert_eq!(exception.error_bag, "login");
        assert_eq!(exception.redirect_to.as_deref(), Some("/login"));
        assert_eq!(exception.status, 400);
        assert_eq!(exception.errors()["email"], vec!["Taken."]);
    }

    #[test]
    fn it_renders_json_for_xhr_requests() {
        let mut headers = illuminate_http::HeaderMap::new();
        headers.insert("accept", "application/json".parse().unwrap());
        let request = Request::create_with("/users", "POST", json!({}), headers);
        let response = ValidationException::with_messages([("email", "Taken.")]).render(&request);
        assert_eq!(response.status_code(), 422);
        assert_eq!(
            response.json_body(),
            json!({"message": "Taken.", "errors": {"email": ["Taken."]}})
        );
    }

    #[test]
    fn it_redirects_back_with_errors_and_input() {
        let mut headers = illuminate_http::HeaderMap::new();
        headers.insert("referer", "http://localhost/register".parse().unwrap());
        let request = Request::create_with(
            "/register",
            "POST",
            json!({"name": "Taylor", "password": "secret"}),
            headers,
        );
        let response = ValidationException::with_messages([("email", "Taken.")]).render(&request);
        assert!(response.is_redirect());
        assert_eq!(
            response.target_url().as_deref(),
            Some("http://localhost/register")
        );
        assert_eq!(response.flashed_input(), Some(&json!({"name": "Taylor"})));
        let (bag, errors) = response.flashed_errors().unwrap();
        assert_eq!(bag, "default");
        assert_eq!(errors.first("email"), Some("Taken."));
    }
}
