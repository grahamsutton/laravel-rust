//! Session helpers that work on the current request.

use std::sync::Arc;

use illuminate_http::{Request, Response, current_request};
use illuminate_support::{HtmlString, Value, e, json};

use crate::request::RequestSessionExt;
use crate::store::Store;

/// Get the current request's session, if it has one.
pub fn try_session() -> Option<Arc<Store>> {
    current_request().and_then(|request| request.try_session())
}

/// Get the current request's session.
///
/// # Panics
///
/// Panics with "Session store not set on request." outside of a request
/// handled by the `StartSession` middleware; see [`try_session`].
pub fn session() -> Arc<Store> {
    try_session().expect("Session store not set on request.")
}

/// Get a value from the current session (`null` without a session).
pub fn session_get(key: &str) -> Value {
    try_session().map_or(Value::Null, |session| session.get(key))
}

/// Get a value from the current session, or a default.
pub fn session_get_or(key: &str, default: impl Into<Value>) -> Value {
    match try_session() {
        Some(session) => session.get_or(key, default),
        None => default.into(),
    }
}

/// Store a value in the current session.
pub fn session_put(key: &str, value: impl Into<Value>) {
    session().put(key, value);
}

/// Store many values in the current session.
pub fn session_put_many<K: AsRef<str>, V: Into<Value>>(values: impl IntoIterator<Item = (K, V)>) {
    session().put_many(values);
}

/// Retrieve an old input item for the current request, or a default.
///
/// ```
/// use illuminate_session::old;
/// use illuminate_support::json;
///
/// // Outside of a request (or without old input), the default is returned.
/// assert_eq!(old("username", ""), json!(""));
/// ```
pub fn old(key: &str, default: impl Into<Value>) -> Value {
    match current_request() {
        Some(request) => request.old_or(key, default),
        None => default.into(),
    }
}

/// Get the CSRF token for the current session (empty without a session).
pub fn csrf_token() -> String {
    try_session()
        .and_then(|session| session.token())
        .unwrap_or_default()
}

/// Generate a CSRF token form field.
///
/// ```
/// use illuminate_session::csrf_field;
///
/// assert_eq!(
///     csrf_field().to_html(),
///     r#"<input type="hidden" name="_token" value="" autocomplete="off">"#,
/// );
/// ```
pub fn csrf_field() -> HtmlString {
    HtmlString::new(format!(
        r#"<input type="hidden" name="_token" value="{}" autocomplete="off">"#,
        e(csrf_token())
    ))
}

/// Generate a form field to spoof the HTTP verb used by forms.
///
/// ```
/// use illuminate_session::method_field;
///
/// assert_eq!(method_field("PUT").to_html(), r#"<input type="hidden" name="_method" value="PUT">"#);
/// ```
pub fn method_field(method: &str) -> HtmlString {
    HtmlString::new(format!(
        r#"<input type="hidden" name="_method" value="{}">"#,
        e(method)
    ))
}

/// Get the "intended" URL from the session.
pub fn intended_url() -> Option<String> {
    try_session().and_then(|session| session.get("url.intended").as_str().map(str::to_string))
}

/// Set the "intended" URL in the session.
pub fn set_intended_url(url: impl Into<String>) {
    session().put("url.intended", url.into());
}

/// Redirect to the URL the user was trying to reach before being sent
/// somewhere else (like the login page), or to the default.
pub fn redirect_intended(default: &str) -> Response {
    let intended = try_session()
        .map(|session| session.pull("url.intended"))
        .and_then(|url| url.as_str().map(str::to_string));
    Response::redirect(to_url(intended.as_deref().unwrap_or(default)))
}

/// Redirect to the given path (typically the login page), remembering the
/// current URL as the "intended" one.
pub fn redirect_guest(path: &str) -> Response {
    if let Some(request) = current_request() {
        let intended = if request.is_method("GET") && !request.expects_json() {
            Some(request.full_url())
        } else {
            previous_url(&request)
        };
        if let (Some(intended), Some(session)) = (intended, request.try_session()) {
            session.put("url.intended", intended);
        }
    }
    Response::redirect(to_url(path))
}

/// The previous URL: the referer, or the one remembered in the session.
fn previous_url(request: &Request) -> Option<String> {
    request.header("referer").or_else(|| {
        request
            .try_session()
            .and_then(|session| session.previous_url())
    })
}

/// Turn a path into a full URL for the current request.
fn to_url(path: &str) -> String {
    let absolute = ["http://", "https://", "//", "mailto:", "tel:", "#"]
        .iter()
        .any(|scheme| path.starts_with(scheme));
    match current_request() {
        Some(request) if !absolute => {
            let path = path.trim_start_matches('/');
            if path.is_empty() {
                request.root()
            } else {
                format!("{}/{path}", request.root())
            }
        }
        _ => path.to_string(),
    }
}

/// The validation errors flashed to the session, shaped for views:
/// `{bag: {field: [messages]}}` (an empty object when there are none).
///
/// The view layer shares this as `$errors` on every request.
pub fn view_errors(request: &Request) -> Value {
    match request.try_session().map(|session| session.get("errors")) {
        Some(errors @ Value::Object(_)) => errors,
        _ => json!({}),
    }
}
