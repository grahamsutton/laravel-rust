//! Fluent assertions about responses, mirroring Laravel's `TestResponse`.

use std::sync::Arc;

use illuminate_cookie::CookieValuePrefix;
use illuminate_http::{Cookie, Request, Response};
use illuminate_session::{RequestSessionExt, Store};
use illuminate_support::{Error, Value, ValueExt};

use super::json as json_helpers;

/// A response returned by a test request, with assertion helpers.
///
/// Every assertion panics with a descriptive message on failure and returns
/// `&Self`, so assertions can be chained:
///
/// ```ignore
/// app.get("/").await.assert_ok().assert_see("Laravel");
/// ```
pub struct TestResponse {
    response: Response,
    request: Request,
}

impl TestResponse {
    pub fn new(response: Response, request: Request) -> Self {
        Self { response, request }
    }

    /// The underlying response.
    pub fn response(&self) -> &Response {
        &self.response
    }

    /// The request that produced this response.
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// The status code.
    pub fn status(&self) -> u16 {
        self.response.status_code()
    }

    /// The body as a string.
    pub fn content(&self) -> String {
        self.response.content_string()
    }

    /// The body decoded as JSON.
    pub fn json(&self) -> Value {
        self.response.json_body()
    }

    /// A value from the JSON body using "dot" notation.
    pub fn json_path(&self, path: &str) -> Value {
        json_helpers::path(&self.json(), path)
    }

    /// A response header.
    pub fn header(&self, name: &str) -> Option<String> {
        self.response.header(name)
    }

    /// The exception that produced the response, if any.
    pub fn exception(&self) -> Option<&Arc<Error>> {
        self.response.exception()
    }

    /// The session used while handling the request.
    pub fn session(&self) -> Option<Arc<Store>> {
        self.request.try_session()
    }

    /// Print the response for debugging, then return it.
    pub fn dump(&self) -> &Self {
        eprintln!(
            "Status: {}\nHeaders: {:?}\n\n{}",
            self.status(),
            self.response.headers(),
            self.content()
        );
        if let Some(exception) = self.exception() {
            eprintln!("\nException: {exception:?}");
        }
        self
    }

    fn fail(&self, message: String) -> ! {
        let mut message = message;
        if let Some(exception) = self.exception() {
            message.push_str(&format!("\n\nThe following exception occurred during the request:\n\n{exception:?}"));
        }
        panic!("{message}");
    }

    // ------------------------------------------------------------------
    // Status
    // ------------------------------------------------------------------

    /// Assert the response has the given status code.
    pub fn assert_status(&self, status: u16) -> &Self {
        if self.status() != status {
            self.fail(format!(
                "Expected response status code [{status}] but received {}.",
                self.status()
            ));
        }
        self
    }

    pub fn assert_ok(&self) -> &Self {
        self.assert_status(200)
    }

    pub fn assert_created(&self) -> &Self {
        self.assert_status(201)
    }

    pub fn assert_accepted(&self) -> &Self {
        self.assert_status(202)
    }

    pub fn assert_no_content(&self) -> &Self {
        self.assert_status(204)
    }

    pub fn assert_moved_permanently(&self) -> &Self {
        self.assert_status(301)
    }

    pub fn assert_found(&self) -> &Self {
        self.assert_status(302)
    }

    pub fn assert_bad_request(&self) -> &Self {
        self.assert_status(400)
    }

    pub fn assert_unauthorized(&self) -> &Self {
        self.assert_status(401)
    }

    pub fn assert_payment_required(&self) -> &Self {
        self.assert_status(402)
    }

    pub fn assert_forbidden(&self) -> &Self {
        self.assert_status(403)
    }

    pub fn assert_not_found(&self) -> &Self {
        self.assert_status(404)
    }

    pub fn assert_method_not_allowed(&self) -> &Self {
        self.assert_status(405)
    }

    pub fn assert_conflict(&self) -> &Self {
        self.assert_status(409)
    }

    pub fn assert_gone(&self) -> &Self {
        self.assert_status(410)
    }

    pub fn assert_unprocessable(&self) -> &Self {
        self.assert_status(422)
    }

    pub fn assert_too_many_requests(&self) -> &Self {
        self.assert_status(429)
    }

    pub fn assert_internal_server_error(&self) -> &Self {
        self.assert_status(500)
    }

    pub fn assert_service_unavailable(&self) -> &Self {
        self.assert_status(503)
    }

    /// Assert the response has a 2xx status code.
    pub fn assert_successful(&self) -> &Self {
        if !self.response.is_successful() {
            self.fail(format!(
                "Expected response status code [>=200, <300] but received {}.",
                self.status()
            ));
        }
        self
    }

    /// Assert the response has a 4xx status code.
    pub fn assert_client_error(&self) -> &Self {
        if !self.response.is_client_error() {
            self.fail(format!("Expected a client error status code but received {}.", self.status()));
        }
        self
    }

    /// Assert the response has a 5xx status code.
    pub fn assert_server_error(&self) -> &Self {
        if !self.response.is_server_error() {
            self.fail(format!("Expected a server error status code but received {}.", self.status()));
        }
        self
    }

    // ------------------------------------------------------------------
    // Redirects & headers
    // ------------------------------------------------------------------

    /// Assert the response is a redirect (optionally to the given URI).
    pub fn assert_redirect<'a>(&self, uri: impl Into<Option<&'a str>>) -> &Self {
        if !self.response.is_redirect() {
            self.fail(format!(
                "Expected response status code [201, 301, 302, 303, 307, 308] but received {}.",
                self.status()
            ));
        }
        if let Some(uri) = uri.into() {
            self.assert_location(uri);
        }
        self
    }

    /// Assert the response redirects to the given URI.
    pub fn assert_redirect_to(&self, uri: &str) -> &Self {
        self.assert_redirect(uri)
    }

    /// Assert the response redirects to the given named route.
    pub fn assert_redirect_to_route<'a>(&self, name: &str, parameters: impl illuminate_routing::IntoRouteParameters<'a>) -> &Self {
        let url = illuminate_routing::url_generator()
            .route(name, parameters)
            .unwrap_or_else(|error| self.fail(error.to_string()));
        self.assert_redirect(url.as_str())
    }

    /// Assert the `Location` header matches the URI (absolute or relative).
    pub fn assert_location(&self, uri: &str) -> &Self {
        let location = self.header("location").unwrap_or_default();
        if !urls_match(&location, uri, &self.request) {
            self.fail(format!("Failed asserting that [{location}] matches the expected location [{uri}]."));
        }
        self
    }

    /// Assert the response has the given header (and optionally, value).
    pub fn assert_header(&self, name: &str, value: Option<&str>) -> &Self {
        match self.header(name) {
            None => self.fail(format!("Header [{name}] not present on response.")),
            Some(actual) => {
                if let Some(expected) = value {
                    if actual != expected {
                        self.fail(format!(
                            "Header [{name}] was found, but value [{actual}] does not match [{expected}]."
                        ));
                    }
                }
            }
        }
        self
    }

    /// Assert the response does not have the given header.
    pub fn assert_header_missing(&self, name: &str) -> &Self {
        if self.header(name).is_some() {
            self.fail(format!("Unexpected header [{name}] is present on response."));
        }
        self
    }

    /// Assert the response is a download (optionally of the given file name).
    pub fn assert_download(&self, filename: Option<&str>) -> &Self {
        let disposition = self.header("content-disposition").unwrap_or_default();
        if !disposition.starts_with("attachment") {
            self.fail("Response does not offer a file download.".into());
        }
        if let Some(filename) = filename {
            if !disposition.contains(filename) {
                self.fail(format!("Expected file [{filename}] is not present in Content-Disposition header."));
            }
        }
        self
    }

    // ------------------------------------------------------------------
    // Content
    // ------------------------------------------------------------------

    /// Assert the (escaped) text appears in the response body.
    pub fn assert_see(&self, text: &str) -> &Self {
        let escaped = illuminate_support::e(text);
        let content = self.content();
        if !content.contains(&escaped) && !content.contains(text) {
            self.fail(format!("Failed asserting that the response contains [{text}].\n\n{content}"));
        }
        self
    }

    /// Assert the given strings appear in order.
    pub fn assert_see_in_order(&self, texts: &[&str]) -> &Self {
        let content = self.content();
        let mut position = 0;
        for text in texts {
            let escaped = illuminate_support::e(text);
            match content[position..].find(escaped.as_str()) {
                Some(found) => position += found + escaped.len(),
                None => self.fail(format!("Failed asserting that [{}] appear in order in the response.", texts.join(", "))),
            }
        }
        self
    }

    /// Assert the text appears in the response with tags stripped.
    pub fn assert_see_text(&self, text: &str) -> &Self {
        let stripped = strip_tags(&self.content());
        if !stripped.contains(text) {
            self.fail(format!("Failed asserting that the response text contains [{text}]."));
        }
        self
    }

    /// Assert the text does not appear in the response body.
    pub fn assert_dont_see(&self, text: &str) -> &Self {
        let escaped = illuminate_support::e(text);
        let content = self.content();
        if content.contains(&escaped) || content.contains(text) {
            self.fail(format!("Failed asserting that the response does not contain [{text}]."));
        }
        self
    }

    /// Assert the text does not appear in the response with tags stripped.
    pub fn assert_dont_see_text(&self, text: &str) -> &Self {
        if strip_tags(&self.content()).contains(text) {
            self.fail(format!("Failed asserting that the response text does not contain [{text}]."));
        }
        self
    }

    /// Assert the response body is exactly the given content.
    pub fn assert_content(&self, content: &str) -> &Self {
        if self.content() != content {
            self.fail(format!("Failed asserting that the response content [{}] equals [{content}].", self.content()));
        }
        self
    }

    // ------------------------------------------------------------------
    // JSON
    // ------------------------------------------------------------------

    fn decoded_json(&self) -> Value {
        match serde_json::from_slice::<Value>(self.response.content()) {
            Ok(value) => value,
            Err(_) => self.fail(format!("Invalid JSON was returned from the route.\n\n{}", self.content())),
        }
    }

    /// Assert the response JSON contains the given data (as a subset).
    pub fn assert_json(&self, data: Value) -> &Self {
        let actual = self.decoded_json();
        if !json_helpers::is_subset(&data, &actual) {
            self.fail(format!(
                "Unable to find JSON:\n\n{}\n\nwithin response JSON:\n\n{}",
                pretty(&data),
                pretty(&actual)
            ));
        }
        self
    }

    /// Assert the response JSON exactly matches the given data.
    pub fn assert_exact_json(&self, data: Value) -> &Self {
        let actual = self.decoded_json();
        if actual != data {
            self.fail(format!("Failed asserting that two JSON values are identical.\n\nExpected:\n{}\n\nActual:\n{}", pretty(&data), pretty(&actual)));
        }
        self
    }

    /// Assert the response JSON contains the given fragment anywhere.
    pub fn assert_json_fragment(&self, fragment: Value) -> &Self {
        let actual = self.decoded_json();
        if !json_helpers::contains_fragment(&fragment, &actual) {
            self.fail(format!("Unable to find JSON fragment:\n\n{}\n\nwithin\n\n{}", pretty(&fragment), pretty(&actual)));
        }
        self
    }

    /// Assert the response JSON does not contain the given fragment.
    pub fn assert_json_missing(&self, fragment: Value) -> &Self {
        if json_helpers::contains_fragment(&fragment, &self.decoded_json()) {
            self.fail(format!("Found unexpected JSON fragment:\n\n{}", pretty(&fragment)));
        }
        self
    }

    /// Assert the JSON value at the given path.
    pub fn assert_json_path(&self, path: &str, expected: impl Into<Value>) -> &Self {
        let expected = expected.into();
        let actual = json_helpers::path(&self.decoded_json(), path);
        if !json_helpers::is_subset(&expected, &actual) || (expected.is_array() && expected.count() != actual.count()) {
            self.fail(format!(
                "Failed asserting that [{}] matches the expected value [{}] at path [{path}].",
                actual, expected
            ));
        }
        self
    }

    /// Assert the JSON does not have the given path.
    pub fn assert_json_missing_path(&self, path: &str) -> &Self {
        if self.decoded_json().dot(path).is_some() {
            self.fail(format!("Failed asserting that the JSON path [{path}] is missing."));
        }
        self
    }

    /// Assert the JSON has the given structure.
    pub fn assert_json_structure(&self, structure: Value) -> &Self {
        if let Err(message) = json_helpers::matches_structure(&structure, &self.decoded_json()) {
            self.fail(format!("The JSON does not match the expected structure: {message}"));
        }
        self
    }

    /// Assert the JSON (or the value at `key`) has `count` items.
    pub fn assert_json_count(&self, count: usize, key: Option<&str>) -> &Self {
        let actual = json_helpers::count(&self.decoded_json(), key);
        if actual != count {
            self.fail(format!(
                "Failed to assert that the response count matched the expected {count} (found {actual}){}.",
                key.map(|k| format!(" at [{k}]")).unwrap_or_default()
            ));
        }
        self
    }

    /// Assert the JSON is a list.
    pub fn assert_json_is_array(&self) -> &Self {
        if !self.decoded_json().is_array() {
            self.fail("Failed asserting that the response JSON is an array.".into());
        }
        self
    }

    /// Assert the JSON is an object.
    pub fn assert_json_is_object(&self) -> &Self {
        if !self.decoded_json().is_object() {
            self.fail("Failed asserting that the response JSON is an object.".into());
        }
        self
    }

    /// Assert the JSON response has validation errors for the given keys.
    pub fn assert_json_validation_errors(&self, keys: &[&str]) -> &Self {
        let json = self.decoded_json();
        let errors = json.get("errors").cloned().unwrap_or(Value::Null);
        for key in keys {
            if errors.get(*key).is_none() {
                self.fail(format!(
                    "Failed to find a validation error in the response for key: '{key}'\n\nResponse has the following JSON validation errors:\n\n{}",
                    pretty(&errors)
                ));
            }
        }
        self
    }

    /// Assert the JSON response has no validation errors for the given keys
    /// (or none at all when `keys` is empty).
    pub fn assert_json_missing_validation_errors(&self, keys: &[&str]) -> &Self {
        let json = serde_json::from_slice::<Value>(self.response.content()).unwrap_or(Value::Null);
        let errors = json.get("errors").cloned().unwrap_or(Value::Null);
        if keys.is_empty() {
            if errors.is_filled() {
                self.fail(format!("Response has unexpected validation errors:\n\n{}", pretty(&errors)));
            }
        } else {
            for key in keys {
                if errors.get(*key).is_some() {
                    self.fail(format!("Found unexpected validation error for key: '{key}'"));
                }
            }
        }
        self
    }

    // ------------------------------------------------------------------
    // Session
    // ------------------------------------------------------------------

    fn session_or_fail(&self) -> Arc<Store> {
        self.session()
            .unwrap_or_else(|| self.fail("The request did not use a session.".into()))
    }

    /// Assert the session has the given key (and optionally, value).
    pub fn assert_session_has(&self, key: &str, value: Option<Value>) -> &Self {
        let session = self.session_or_fail();
        if !session.has(key) {
            self.fail(format!("Session is missing expected key [{key}]."));
        }
        if let Some(expected) = value {
            let actual = session.get(key);
            if actual != expected {
                self.fail(format!("Session key [{key}] has value [{actual}], expected [{expected}]."));
            }
        }
        self
    }

    /// Assert the session has every given key/value pair.
    pub fn assert_session_has_all(&self, values: Value) -> &Self {
        if let Value::Object(map) = values {
            for (key, value) in map {
                self.assert_session_has(&key, Some(value));
            }
        }
        self
    }

    /// Assert the session does not have the given key.
    pub fn assert_session_missing(&self, key: &str) -> &Self {
        if self.session().is_some_and(|s| s.has(key)) {
            self.fail(format!("Session has unexpected key [{key}]."));
        }
        self
    }

    fn session_errors(&self, bag: &str) -> Value {
        self.session()
            .map(|s| s.get("errors"))
            .and_then(|errors| errors.get(bag).cloned())
            .unwrap_or(Value::Null)
    }

    /// Assert the session has validation errors for the given keys.
    pub fn assert_session_has_errors(&self, keys: &[&str]) -> &Self {
        self.assert_session_has_errors_in("default", keys)
    }

    /// Assert the session has errors for the given keys in a named bag.
    pub fn assert_session_has_errors_in(&self, bag: &str, keys: &[&str]) -> &Self {
        let errors = self.session_errors(bag);
        if keys.is_empty() && errors.is_blank() {
            self.fail("Session is missing expected key [errors].".into());
        }
        for key in keys {
            if errors.get(*key).is_none() {
                self.fail(format!(
                    "Session missing error: {key}\n\nThe session has the following errors:\n\n{}",
                    pretty(&errors)
                ));
            }
        }
        self
    }

    /// Assert the session has no validation errors.
    pub fn assert_session_has_no_errors(&self) -> &Self {
        let errors = self
            .session()
            .map(|s| s.get("errors"))
            .unwrap_or(Value::Null);
        let has_errors = match &errors {
            Value::Object(bags) => bags.values().any(|bag| bag.is_filled()),
            _ => false,
        };
        if has_errors {
            self.fail(format!("Session has unexpected errors:\n\n{}", pretty(&errors)));
        }
        self
    }

    /// Assert the session does not have errors for the given keys.
    pub fn assert_session_doesnt_have_errors(&self, keys: &[&str]) -> &Self {
        let errors = self.session_errors("default");
        for key in keys {
            if errors.get(*key).is_some() {
                self.fail(format!("Session has unexpected error: {key}"));
            }
        }
        self
    }

    /// Assert the session has old input for the given key.
    pub fn assert_session_has_input(&self, key: &str, value: Option<Value>) -> &Self {
        let session = self.session_or_fail();
        if !session.has_old_input_for(key) {
            self.fail(format!("Session is missing expected old input [{key}]."));
        }
        if let Some(expected) = value {
            let actual = session.get_old_input(key);
            if actual != expected {
                self.fail(format!("Old input [{key}] has value [{actual}], expected [{expected}]."));
            }
        }
        self
    }

    /// Assert the response is valid (no validation errors), JSON or session.
    pub fn assert_valid(&self, keys: &[&str]) -> &Self {
        if self.is_json_response() {
            self.assert_json_missing_validation_errors(keys)
        } else if keys.is_empty() {
            self.assert_session_has_no_errors()
        } else {
            self.assert_session_doesnt_have_errors(keys)
        }
    }

    /// Assert the response has validation errors for the keys, JSON or session.
    pub fn assert_invalid(&self, keys: &[&str]) -> &Self {
        if self.is_json_response() {
            self.assert_json_validation_errors(keys)
        } else {
            self.assert_session_has_errors(keys)
        }
    }

    fn is_json_response(&self) -> bool {
        self.header("content-type")
            .is_some_and(|ct| ct.contains("json"))
    }

    // ------------------------------------------------------------------
    // Cookies
    // ------------------------------------------------------------------

    fn find_cookie(&self, name: &str) -> Option<Cookie> {
        self.response.get_cookie(name).cloned()
    }

    /// Assert the response has the given (encrypted) cookie, optionally
    /// with the given decrypted value.
    pub fn assert_cookie(&self, name: &str, value: Option<&str>) -> &Self {
        let Some(cookie) = self.find_cookie(name) else {
            self.fail(format!("Cookie [{name}] not present on response."));
        };
        if let Some(expected) = value {
            let actual = decrypt_cookie(name, &cookie.value).unwrap_or(cookie.value.clone());
            if actual != expected {
                self.fail(format!("Cookie [{name}] was found, but value [{actual}] does not match [{expected}]."));
            }
        }
        self
    }

    /// Assert the response has the given unencrypted cookie.
    pub fn assert_plain_cookie(&self, name: &str, value: Option<&str>) -> &Self {
        let Some(cookie) = self.find_cookie(name) else {
            self.fail(format!("Cookie [{name}] not present on response."));
        };
        if let Some(expected) = value {
            if cookie.value != expected {
                self.fail(format!("Cookie [{name}] was found, but value [{}] does not match [{expected}].", cookie.value));
            }
        }
        self
    }

    /// Assert the given cookie has been expired.
    pub fn assert_cookie_expired(&self, name: &str) -> &Self {
        match self.find_cookie(name) {
            Some(cookie) if cookie.is_cleared() => self,
            Some(_) => self.fail(format!("Cookie [{name}] is not expired.")),
            None => self.fail(format!("Cookie [{name}] not present on response.")),
        }
    }

    /// Assert the response does not set the given cookie.
    pub fn assert_cookie_missing(&self, name: &str) -> &Self {
        if self.find_cookie(name).is_some() {
            self.fail(format!("Cookie [{name}] is present on response."));
        }
        self
    }
}

/// Decrypt a cookie value written by `EncryptCookies`.
pub fn decrypt_cookie(name: &str, value: &str) -> Option<String> {
    let encrypter = illuminate_encryption::encrypter().ok()?;
    let decrypted = encrypter.decrypt_string(value).ok()?;
    CookieValuePrefix::validate(name, &decrypted, &[encrypter.get_key()])
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn urls_match(location: &str, expected: &str, request: &Request) -> bool {
    if location == expected {
        return true;
    }
    let root = request.root();
    let normalize = |url: &str| -> String {
        let url = url.strip_prefix(&root).unwrap_or(url);
        let url = url
            .strip_prefix("http://localhost")
            .unwrap_or(url);
        let trimmed = url.trim_end_matches('/');
        if trimmed.is_empty() { "/".to_string() } else { trimmed.to_string() }
    };
    normalize(location) == normalize(expected)
}
