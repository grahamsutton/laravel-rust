//! Fluent assertions about responses, mirroring Laravel's `TestResponse`.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use futures::StreamExt;

use illuminate_cookie::CookieValuePrefix;
use illuminate_http::{Body, BodyStream, Cookie, Request, Response};
use illuminate_routing::{IntoRouteParameters, url_generator};
use illuminate_session::{RequestSessionExt, Store};
use illuminate_support::{Arr, Error, Map, Value, ValueExt, data_get};
use illuminate_view::{Factory, ViewInfo};

use super::content;
use super::fluent::AssertableJson;
use super::json as json_helpers;

/// A response returned by a test request, with assertion helpers.
///
/// Every assertion panics with a descriptive message on failure and returns
/// `&Self`, so assertions can be chained:
///
/// ```ignore
/// app.get("/").await.assert_ok().assert_see("Laravel");
/// ```
///
/// Streamed responses are read lazily, when you first ask for their
/// content: `response.assert_streamed_content("Hello").await`.
pub struct TestResponse {
    response: Response,
    request: Request,
    stream: Mutex<Option<BodyStream>>,
    streamed: bool,
    streamed_content: OnceLock<String>,
}

impl TestResponse {
    /// Wrap a response (and the request that produced it). A streamed body
    /// is moved into the test response, to be read by
    /// [`streamed_content`](Self::streamed_content).
    pub fn new(mut response: Response, request: Request) -> Self {
        let stream = match response.body() {
            Body::Stream(_) => match response.take_body() {
                Body::Stream(stream) => Some(stream.into_inner()),
                _ => None,
            },
            _ => None,
        };
        Self {
            streamed: stream.is_some(),
            stream: Mutex::new(stream),
            streamed_content: OnceLock::new(),
            response,
            request,
        }
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

    /// The body as a string (empty for streamed responses — see
    /// [`streamed_content`](Self::streamed_content)).
    pub fn content(&self) -> String {
        self.response.content_string()
    }

    /// The body decoded as JSON.
    pub fn json(&self) -> Value {
        match self.streamed_content.get() {
            Some(content) => serde_json::from_str(content).unwrap_or(Value::Null),
            None => self.response.json_body(),
        }
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

    /// The original data the response was built from (the data of a JSON
    /// response), if any.
    pub fn original(&self) -> Option<&Value> {
        self.response.original()
    }

    /// The view that rendered the response, if any — its name and data
    /// (Laravel's `$response->original` for view responses).
    pub fn view(&self) -> Option<Arc<ViewInfo>> {
        self.response.extension::<ViewInfo>()
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

    /// Assert the response has a `200 OK` status code.
    pub fn assert_ok(&self) -> &Self {
        self.assert_status(200)
    }

    /// Assert the response has a `201 Created` status code.
    pub fn assert_created(&self) -> &Self {
        self.assert_status(201)
    }

    /// Assert the response has a `202 Accepted` status code.
    pub fn assert_accepted(&self) -> &Self {
        self.assert_status(202)
    }

    /// Assert the response has a `204 No Content` status code and an empty body.
    pub fn assert_no_content(&self) -> &Self {
        self.assert_status(204);
        if !self.response.content().is_empty() {
            self.fail("Response content is not empty.".into());
        }
        self
    }

    /// Assert a precognitive request passed validation: a `204 No Content`
    /// response with `Precognition-Success: true`.
    pub fn assert_successful_precognition(&self) -> &Self {
        self.assert_no_content();
        match self.header("Precognition-Success") {
            None => self.fail("Header [Precognition-Success] not present on response.".into()),
            Some(value) if value != "true" => {
                self.fail("The Precognition-Success header was found, but the value is not `true`.".into())
            }
            Some(_) => {}
        }
        self
    }

    /// Assert the response has a `301 Moved Permanently` status code.
    pub fn assert_moved_permanently(&self) -> &Self {
        self.assert_status(301)
    }

    /// Assert the response has a `302 Found` status code.
    pub fn assert_found(&self) -> &Self {
        self.assert_status(302)
    }

    /// Assert the response has a `304 Not Modified` status code.
    pub fn assert_not_modified(&self) -> &Self {
        self.assert_status(304)
    }

    /// Assert the response has a `307 Temporary Redirect` status code.
    pub fn assert_temporary_redirect(&self) -> &Self {
        self.assert_status(307)
    }

    /// Assert the response has a `308 Permanent Redirect` status code.
    pub fn assert_permanent_redirect(&self) -> &Self {
        self.assert_status(308)
    }

    /// Assert the response has a `400 Bad Request` status code.
    pub fn assert_bad_request(&self) -> &Self {
        self.assert_status(400)
    }

    /// Assert the response has a `401 Unauthorized` status code.
    pub fn assert_unauthorized(&self) -> &Self {
        self.assert_status(401)
    }

    /// Assert the response has a `402 Payment Required` status code.
    pub fn assert_payment_required(&self) -> &Self {
        self.assert_status(402)
    }

    /// Assert the response has a `403 Forbidden` status code.
    pub fn assert_forbidden(&self) -> &Self {
        self.assert_status(403)
    }

    /// Assert the response has a `404 Not Found` status code.
    pub fn assert_not_found(&self) -> &Self {
        self.assert_status(404)
    }

    /// Assert the response has a `405 Method Not Allowed` status code.
    pub fn assert_method_not_allowed(&self) -> &Self {
        self.assert_status(405)
    }

    /// Assert the response has a `406 Not Acceptable` status code.
    pub fn assert_not_acceptable(&self) -> &Self {
        self.assert_status(406)
    }

    /// Assert the response has a `408 Request Timeout` status code.
    pub fn assert_request_timeout(&self) -> &Self {
        self.assert_status(408)
    }

    /// Assert the response has a `409 Conflict` status code.
    pub fn assert_conflict(&self) -> &Self {
        self.assert_status(409)
    }

    /// Assert the response has a `410 Gone` status code.
    pub fn assert_gone(&self) -> &Self {
        self.assert_status(410)
    }

    /// Assert the response has a `415 Unsupported Media Type` status code.
    pub fn assert_unsupported_media_type(&self) -> &Self {
        self.assert_status(415)
    }

    /// Assert the response has a `422 Unprocessable Content` status code.
    pub fn assert_unprocessable(&self) -> &Self {
        self.assert_status(422)
    }

    /// Assert the response has a `424 Failed Dependency` status code.
    pub fn assert_failed_dependency(&self) -> &Self {
        self.assert_status(424)
    }

    /// Assert the response has a `429 Too Many Requests` status code.
    pub fn assert_too_many_requests(&self) -> &Self {
        self.assert_status(429)
    }

    /// Assert the response has a `500 Internal Server Error` status code.
    pub fn assert_internal_server_error(&self) -> &Self {
        self.assert_status(500)
    }

    /// Assert the response has a `503 Service Unavailable` status code.
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
            self.fail(format!(
                "Expected response status code [>=400, < 500] but received {}.",
                self.status()
            ));
        }
        self
    }

    /// Assert the response has a 5xx status code.
    pub fn assert_server_error(&self) -> &Self {
        if !self.response.is_server_error() {
            self.fail(format!(
                "Expected response status code [>=500, < 600] but received {}.",
                self.status()
            ));
        }
        self
    }

    // ------------------------------------------------------------------
    // Redirects & headers
    // ------------------------------------------------------------------

    fn assert_is_redirect(&self) {
        if !self.response.is_redirect() {
            self.fail(format!(
                "Expected response status code [201, 301, 302, 303, 307, 308] but received {}.",
                self.status()
            ));
        }
    }

    /// Assert the response is a redirect (optionally to the given URI).
    pub fn assert_redirect<'a>(&self, uri: impl Into<Option<&'a str>>) -> &Self {
        self.assert_is_redirect();
        if let Some(uri) = uri.into() {
            self.assert_location(uri);
        }
        self
    }

    /// Assert the response redirects to the given URI.
    pub fn assert_redirect_to(&self, uri: &str) -> &Self {
        self.assert_redirect(uri)
    }

    /// Assert the response redirects to a URI that contains the given string.
    pub fn assert_redirect_contains(&self, uri: &str) -> &Self {
        self.assert_is_redirect();
        let location = self.header("location").unwrap_or_default();
        if !location.contains(uri) {
            self.fail(format!("Redirect location [{location}] does not contain [{uri}]."));
        }
        self
    }

    /// Assert the response redirects back to the previous location — the
    /// `Referer` header, or the previous URL remembered by the session.
    pub fn assert_redirect_back(&self) -> &Self {
        self.assert_is_redirect();
        let previous =
            illuminate_http::with_request_sync(self.request.clone(), || url_generator().previous());
        self.assert_location(&previous)
    }

    /// Assert the response redirects back with validation errors in the
    /// session for the given keys (any errors at all when `keys` is empty).
    pub fn assert_redirect_back_with_errors(&self, keys: &[&str]) -> &Self {
        self.assert_redirect_back();
        self.assert_session_has_errors(keys)
    }

    /// Assert the response redirects back without validation errors.
    pub fn assert_redirect_back_without_errors(&self) -> &Self {
        self.assert_redirect_back();
        self.assert_session_has_no_errors()
    }

    /// Assert the response redirects to the given named route.
    pub fn assert_redirect_to_route<'a>(&self, name: &str, parameters: impl IntoRouteParameters<'a>) -> &Self {
        let url = url_generator()
            .route(name, parameters)
            .unwrap_or_else(|error| self.fail(error.to_string()));
        self.assert_redirect(url.as_str())
    }

    /// Assert the response redirects to a URL with a valid signature —
    /// optionally, to the given named route:
    ///
    /// ```ignore
    /// response.assert_redirect_to_signed_route(None, ());
    /// response.assert_redirect_to_signed_route("unsubscribe", json!({"user": 1}));
    /// ```
    pub fn assert_redirect_to_signed_route<'a, 'b>(
        &self,
        name: impl Into<Option<&'a str>>,
        parameters: impl IntoRouteParameters<'b>,
    ) -> &Self {
        let generator = url_generator();
        let expected = name.into().map(|name| {
            generator
                .route(name, parameters)
                .unwrap_or_else(|error| self.fail(error.to_string()))
        });

        self.assert_is_redirect();

        let location = self.header("location").unwrap_or_default();
        let request = Request::create(&location, "GET");
        if !generator.has_valid_signature(&request) {
            self.fail("The response is not a redirect to a signed route.".into());
        }

        if let Some(expected) = expected {
            let expected = generator.to(&expected);
            let unsigned = request.full_url_without_query(&["signature", "expires"]);
            let unsigned = unsigned.trim_end_matches('?');
            if unsigned != expected {
                self.fail(format!(
                    "Failed asserting that the signed redirect [{unsigned}] matches the expected route [{expected}]."
                ));
            }
        }
        self
    }

    /// Assert the response redirects to the given controller action
    /// (`"UserController@show"`).
    pub fn assert_redirect_to_action<'a>(&self, action: &str, parameters: impl IntoRouteParameters<'a>) -> &Self {
        let generator = url_generator();
        let route = generator
            .router()
            .get_by_action(action)
            .unwrap_or_else(|| self.fail(format!("Action {action} not defined.")));
        let url = generator
            .to_route(&route, parameters, true)
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
                if let Some(expected) = value
                    && actual != expected {
                        self.fail(format!(
                            "Header [{name}] was found, but value [{actual}] does not match [{expected}]."
                        ));
                    }
            }
        }
        self
    }

    /// Assert the response has the given header and that its value
    /// contains the given string.
    pub fn assert_header_contains(&self, name: &str, value: &str) -> &Self {
        let Some(actual) = self.header(name) else {
            self.fail(format!("Header [{name}] not present on response."));
        };
        if !actual.contains(value) {
            self.fail(format!(
                "Header [{name}] was found, but [{actual}] does not contain [{value}]."
            ));
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
        if let Some(filename) = filename
            && !disposition.contains(filename) {
                self.fail(format!("Expected file [{filename}] is not present in Content-Disposition header."));
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

    /// Assert the given HTML appears, unescaped, in the response body.
    pub fn assert_see_html(&self, html: &str) -> &Self {
        let content = self.content();
        if !content.contains(html) {
            self.fail(format!("Failed asserting that the response contains [{html}].\n\n{content}"));
        }
        self
    }

    /// Assert the given (escaped) strings appear in order.
    pub fn assert_see_in_order(&self, texts: &[&str]) -> &Self {
        if let Err(message) = content::see_in_order(&self.content(), &content::prepare(texts, true)) {
            self.fail(message);
        }
        self
    }

    /// Assert the given HTML strings appear, unescaped, in order.
    pub fn assert_see_html_in_order(&self, html: &[&str]) -> &Self {
        if let Err(message) = content::see_in_order(&self.content(), &content::prepare(html, false)) {
            self.fail(message);
        }
        self
    }

    /// Assert the text appears in the response with tags stripped.
    pub fn assert_see_text(&self, text: &str) -> &Self {
        if let Err(message) = content::see_in_html(&self.content(), &content::prepare(&[text], true), false, false) {
            self.fail(message);
        }
        self
    }

    /// Assert the given strings appear in order in the response text.
    pub fn assert_see_text_in_order(&self, texts: &[&str]) -> &Self {
        if let Err(message) = content::see_in_html(&self.content(), &content::prepare(texts, true), true, false) {
            self.fail(message);
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

    /// Assert the given HTML does not appear in the response body.
    pub fn assert_dont_see_html(&self, html: &str) -> &Self {
        if self.content().contains(html) {
            self.fail(format!("Failed asserting that the response does not contain [{html}]."));
        }
        self
    }

    /// Assert the text does not appear in the response with tags stripped.
    pub fn assert_dont_see_text(&self, text: &str) -> &Self {
        if let Err(message) = content::see_in_html(&self.content(), &content::prepare(&[text], true), false, true) {
            self.fail(message);
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
    // Streamed responses
    // ------------------------------------------------------------------

    /// Assert the response was streamed.
    pub fn assert_streamed(&self) -> &Self {
        if !self.streamed {
            self.fail("Expected the response to be streamed, but it wasn't.".into());
        }
        self
    }

    /// Assert the response was not streamed.
    pub fn assert_not_streamed(&self) -> &Self {
        if self.streamed {
            self.fail("Response was unexpectedly streamed.".into());
        }
        self
    }

    /// Read the streamed response to the end and return its content. The
    /// stream only runs the first time; later calls return the same content.
    pub async fn streamed_content(&self) -> String {
        if let Some(content) = self.streamed_content.get() {
            return content.clone();
        }
        if !self.streamed {
            self.fail("The response is not a streamed response.".into());
        }

        let stream = self.stream.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(mut stream) = stream {
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => bytes.extend_from_slice(&chunk),
                    Err(error) => self.fail(format!("The streamed response failed: {error:?}")),
                }
            }
            let _ = self.streamed_content.set(String::from_utf8_lossy(&bytes).into_owned());
        }
        self.streamed_content.get().cloned().unwrap_or_default()
    }

    /// Assert the streamed response's content is exactly the given string.
    pub async fn assert_streamed_content(&self, value: &str) -> &Self {
        let content = self.streamed_content().await;
        if content != value {
            self.fail(format!(
                "Failed asserting that the streamed content [{content}] is identical to [{value}]."
            ));
        }
        self
    }

    /// Assert the streamed response's content is the given data, encoded as JSON.
    pub async fn assert_streamed_json_content(&self, value: Value) -> &Self {
        self.assert_streamed_content(&json_helpers::encode(&value)).await
    }

    // ------------------------------------------------------------------
    // JSON
    // ------------------------------------------------------------------

    fn decoded_json(&self) -> Value {
        let content = match self.streamed_content.get() {
            Some(content) => content.as_bytes().to_vec(),
            None if self.streamed => self.fail(
                "The streamed response has not been read yet. Await `streamed_content()` before making JSON assertions."
                    .into(),
            ),
            None => self.response.content().to_vec(),
        };
        match serde_json::from_slice::<Value>(&content) {
            Ok(value) => value,
            Err(_) => self.fail(format!(
                "Invalid JSON was returned from the route.\n\n{}",
                String::from_utf8_lossy(&content)
            )),
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

    /// Make fluent assertions against the response JSON — Laravel's
    /// `assertJson(fn (AssertableJson $json) => ...)`.
    ///
    /// Unless you call [`etc`](AssertableJson::etc), every property of a
    /// JSON object must be asserted against:
    ///
    /// ```ignore
    /// response.assert_json_fluent(|json| {
    ///     json.where_("id", 1)
    ///         .where_("name", "Victoria Faith")
    ///         .missing("password")
    ///         .etc()
    /// });
    /// ```
    pub fn assert_json_fluent(&self, callback: impl FnOnce(AssertableJson) -> AssertableJson) -> &Self {
        let assert = callback(AssertableJson::from_array(self.decoded_json()));
        if assert.is_assoc() {
            assert.interacted();
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

    /// Assert the response JSON matches the given data, ignoring the order
    /// of keys and list items.
    pub fn assert_similar_json(&self, data: Value) -> &Self {
        let actual = json_helpers::sorted_encoding(&self.decoded_json());
        let expected = json_helpers::sorted_encoding(&data);
        if actual != expected {
            self.fail(format!(
                "Failed asserting that two JSON values are similar.\n\nExpected:\n{expected}\n\nActual:\n{actual}"
            ));
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

    /// Assert the response JSON contains each of the given fragments.
    pub fn assert_json_fragments(&self, fragments: impl IntoIterator<Item = Value>) -> &Self {
        for fragment in fragments {
            self.assert_json_fragment(fragment);
        }
        self
    }

    /// Assert none of the given `key: value` pairs appear anywhere in the
    /// response JSON.
    pub fn assert_json_missing(&self, data: Value) -> &Self {
        let actual = self.decoded_json();
        if let Some((pair, _)) = json_helpers::find_pairs(&data, &actual)
            .into_iter()
            .find(|(_, found)| *found)
        {
            self.fail(format!(
                "Found unexpected JSON fragment:\n\n[{}]\n\nwithin\n\n[{}].",
                json_helpers::encode(&pair),
                json_helpers::sorted_encoding(&actual)
            ));
        }
        self
    }

    /// Assert the given `key: value` pairs don't *all* appear in the
    /// response JSON.
    pub fn assert_json_missing_exact(&self, data: Value) -> &Self {
        let actual = self.decoded_json();
        let pairs = json_helpers::find_pairs(&data, &actual);
        if pairs.iter().all(|(_, found)| *found) {
            self.fail(format!(
                "Found unexpected JSON fragment:\n\n[{}]\n\nwithin\n\n[{}].",
                json_helpers::encode(&data),
                json_helpers::sorted_encoding(&actual)
            ));
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

    /// Assert the JSON value at the given path passes the truth test.
    pub fn assert_json_path_with(&self, path: &str, callback: impl FnOnce(&Value) -> bool) -> &Self {
        let actual = json_helpers::path(&self.decoded_json(), path);
        if !callback(&actual) {
            self.fail(format!(
                "Failed asserting that the value [{actual}] at path [{path}] fulfills the expectations defined by the closure."
            ));
        }
        self
    }

    /// Assert the JSON values at several paths:
    /// `assert_json_paths(json!({"team.owner.name": "Darian", "team.members.0.name": "Sally"}))`.
    pub fn assert_json_paths(&self, paths: Value) -> &Self {
        for (path, expected) in self.bindings(paths) {
            self.assert_json_path(&path, expected);
        }
        self
    }

    /// Assert the JSON list at the given path contains exactly the expected
    /// values, in any order.
    pub fn assert_json_path_canonicalizing(&self, path: &str, expected: Value) -> &Self {
        let actual = json_helpers::path(&self.decoded_json(), path);
        if !json_helpers::canonically_equal(&expected, &actual) {
            self.fail(format!(
                "Failed asserting that [{actual}] matches the expected values [{expected}] at path [{path}], ignoring order."
            ));
        }
        self
    }

    /// Assert the JSON lists at several paths contain exactly the expected
    /// values, in any order.
    pub fn assert_json_paths_canonicalizing(&self, paths: Value) -> &Self {
        for (path, expected) in self.bindings(paths) {
            self.assert_json_path_canonicalizing(&path, expected);
        }
        self
    }

    /// Assert the JSON does not have the given path (`*` wildcards allowed).
    pub fn assert_json_missing_path(&self, path: &str) -> &Self {
        if json_helpers::has_path(&self.decoded_json(), path) {
            if path.contains('*') {
                self.fail(format!("Found unexpected path [{path}] within the response JSON."));
            }
            self.fail(format!("Failed asserting that the JSON path [{path}] is missing."));
        }
        self
    }

    /// Assert the JSON has none of the given paths.
    pub fn assert_json_missing_paths(&self, paths: &[&str]) -> &Self {
        for path in paths {
            self.assert_json_missing_path(path);
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

    /// Assert the JSON has exactly the given structure: no keys beyond the
    /// ones listed, at any level (except under `*`).
    pub fn assert_exact_json_structure(&self, structure: Value) -> &Self {
        if let Err(message) = json_helpers::matches_exact_structure(&structure, &self.decoded_json()) {
            self.fail(format!("The JSON does not exactly match the expected structure: {message}"));
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

    fn bindings(&self, bindings: Value) -> Map<String, Value> {
        match bindings {
            Value::Object(map) => map,
            other => self.fail(format!("Expected a JSON object of paths, got [{other}].")),
        }
    }

    /// The `errors` of a JSON validation response. Invalid JSON fails the
    /// test, unless `lenient` (then there are simply no errors).
    fn json_validation_errors(&self, lenient: bool) -> Value {
        let json = if lenient {
            serde_json::from_slice::<Value>(self.response.content()).unwrap_or(Value::Null)
        } else {
            self.decoded_json()
        };
        json.get("errors").cloned().unwrap_or(Value::Null)
    }

    fn json_validation_errors_message(errors: &Value) -> String {
        if errors.is_filled() {
            format!("Response has the following JSON validation errors:\n\n{}\n", pretty(errors))
        } else {
            "Response does not have JSON validation errors.".to_string()
        }
    }

    /// Assert the JSON response has validation errors for the given keys.
    pub fn assert_json_validation_errors(&self, keys: &[&str]) -> &Self {
        if keys.is_empty() {
            self.fail("No validation errors were provided.".into());
        }
        for key in keys {
            self.assert_json_validation_error_for(key);
        }
        self
    }

    /// Assert the JSON response has a validation error for the given key.
    pub fn assert_json_validation_error_for(&self, key: &str) -> &Self {
        let errors = self.json_validation_errors(false);
        if errors.get(key).is_none() {
            self.fail(format!(
                "Failed to find a validation error in the response for key: '{key}'\n\n{}",
                Self::json_validation_errors_message(&errors)
            ));
        }
        self
    }

    /// Assert the JSON response has validation errors for the given keys,
    /// and no others.
    pub fn assert_only_json_validation_errors(&self, keys: &[&str]) -> &Self {
        self.assert_json_validation_errors(keys);
        let errors = self.json_validation_errors(false);
        let unexpected: Vec<String> = match &errors {
            Value::Object(map) => map
                .keys()
                .filter(|key| !keys.contains(&key.as_str()))
                .map(|key| format!("'{key}'"))
                .collect(),
            _ => Vec::new(),
        };
        if !unexpected.is_empty() {
            self.fail(format!("Response has unexpected validation errors: {}", unexpected.join(", ")));
        }
        self
    }

    /// Assert the JSON response has no validation errors for the given keys
    /// (or none at all when `keys` is empty).
    pub fn assert_json_missing_validation_errors(&self, keys: &[&str]) -> &Self {
        let errors = self.json_validation_errors(true);
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
    // Views
    // ------------------------------------------------------------------

    fn ensure_view(&self) -> Arc<ViewInfo> {
        self.view()
            .unwrap_or_else(|| self.fail("The response is not a view.".into()))
    }

    /// The data the view was rendered with: shared data, then the view's own.
    fn gathered_view_data(&self) -> Value {
        Value::Object(gather_view_data(&self.ensure_view().data))
    }

    /// Get a piece of the response view's data (`None` for all of it).
    pub fn view_data<'a>(&self, key: impl Into<Option<&'a str>>) -> Value {
        let data = self.gathered_view_data();
        match key.into() {
            Some(key) => Arr::get(&data, key),
            None => data,
        }
    }

    /// Assert the response is the given view.
    pub fn assert_view_is(&self, name: &str) -> &Self {
        let view = self.ensure_view();
        if view.name != name {
            self.fail(format!(
                "Failed asserting that the response view [{}] is [{name}].",
                view.name
            ));
        }
        self
    }

    /// Assert the response view has a piece of data — and, when given, that
    /// it equals the value: `assert_view_has("name", json!("Taylor"))` or
    /// `assert_view_has("name", None)`.
    pub fn assert_view_has(&self, key: &str, value: impl Into<Option<Value>>) -> &Self {
        if let Err(message) = view_has(&self.gathered_view_data(), key, value.into()) {
            self.fail(message);
        }
        self
    }

    /// Assert the response view's data at `key` passes the truth test.
    pub fn assert_view_has_with(&self, key: &str, callback: impl FnOnce(&Value) -> bool) -> &Self {
        if let Err(message) = view_has_with(&self.gathered_view_data(), key, callback) {
            self.fail(message);
        }
        self
    }

    /// Assert the response view has all of the given data: an object of
    /// key/value pairs, or a list of keys.
    pub fn assert_view_has_all(&self, bindings: Value) -> &Self {
        if let Err(message) = view_has_all(&self.gathered_view_data(), bindings) {
            self.fail(message);
        }
        self
    }

    /// Assert the response view is missing a piece of data.
    pub fn assert_view_missing(&self, key: &str) -> &Self {
        if let Err(message) = view_missing(&self.gathered_view_data(), key) {
            self.fail(message);
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

    /// Assert the session does not have old input for the given key.
    pub fn assert_session_missing_input(&self, key: &str) -> &Self {
        if self.session().is_some_and(|session| session.has_old_input_for(key)) {
            self.fail(format!("Session has unexpected key [{key}]."));
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

    /// Assert the response has validation errors for the keys, and no
    /// others — JSON or session.
    pub fn assert_only_invalid(&self, keys: &[&str]) -> &Self {
        if self.is_json_response() {
            return self.assert_only_json_validation_errors(keys);
        }
        self.assert_session_has_errors(keys);
        let unexpected: Vec<String> = match self.session_errors("default") {
            Value::Object(map) => map
                .keys()
                .filter(|key| !keys.contains(&key.as_str()))
                .map(|key| format!("'{key}'"))
                .collect(),
            _ => Vec::new(),
        };
        if !unexpected.is_empty() {
            self.fail(format!("Response has unexpected validation errors: {}", unexpected.join(", ")));
        }
        self
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
        if let Some(expected) = value
            && cookie.value != expected {
                self.fail(format!("Cookie [{name}] was found, but value [{}] does not match [{expected}].", cookie.value));
            }
        self
    }

    /// Assert the given cookie has been expired.
    pub fn assert_cookie_expired(&self, name: &str) -> &Self {
        match self.find_cookie(name) {
            Some(cookie) if cookie.is_cleared() => self,
            Some(cookie) => self.fail(format!(
                "Cookie [{name}] is not expired, it expires at [{}].",
                cookie.expires_at().map(|at| at.to_string()).unwrap_or_default()
            )),
            None => self.fail(format!("Cookie [{name}] not present on response.")),
        }
    }

    /// Assert the response has the given cookie, and that it hasn't expired
    /// (session cookies never expire).
    pub fn assert_cookie_not_expired(&self, name: &str) -> &Self {
        match self.find_cookie(name) {
            Some(cookie) if cookie.is_cleared() => self.fail(format!(
                "Cookie [{name}] is expired, it expired at [{}].",
                cookie.expires_at().map(|at| at.to_string()).unwrap_or_default()
            )),
            Some(_) => self,
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

/// The data shared with every view, then the view's own data (Laravel's
/// `View::gatherData`).
pub(crate) fn gather_view_data(data: &Value) -> Map<String, Value> {
    let mut gathered = Map::new();
    if let Some(factory) = illuminate_container::try_app::<Factory>() {
        for (key, value) in factory.get_shared().iter() {
            gathered.insert(key.clone(), value.to_json());
        }
    }
    if let Value::Object(data) = data {
        for (key, value) in data {
            gathered.insert(key.clone(), value.clone());
        }
    }
    gathered
}

pub(crate) fn view_has(data: &Value, key: &str, value: Option<Value>) -> Result<(), String> {
    match value {
        None if !Arr::has(data, key) => Err(format!("Failed asserting that the data contains the key [{key}].")),
        None => Ok(()),
        Some(expected) => {
            let actual = data_get(data, key);
            if json_helpers::loosely_equal(&expected, &actual) {
                Ok(())
            } else {
                Err(format!(
                    "Failed asserting that [{key}] matches the expected value.\n\nExpected: {expected}\nActual: {actual}"
                ))
            }
        }
    }
}

pub(crate) fn view_has_with(data: &Value, key: &str, callback: impl FnOnce(&Value) -> bool) -> Result<(), String> {
    if callback(&data_get(data, key)) {
        Ok(())
    } else {
        Err(format!(
            "Failed asserting that the value at [{key}] fulfills the expectations defined by the closure."
        ))
    }
}

pub(crate) fn view_has_all(data: &Value, bindings: Value) -> Result<(), String> {
    match bindings {
        Value::Object(map) => {
            for (key, value) in map {
                view_has(data, &key, Some(value))?;
            }
            Ok(())
        }
        Value::Array(keys) => {
            for key in keys {
                view_has(data, &key.to_string_lossy(), None)?;
            }
            Ok(())
        }
        Value::String(key) => view_has(data, &key, None),
        other => Err(format!("Expected an object or a list of view data keys, got [{other}].")),
    }
}

pub(crate) fn view_missing(data: &Value, key: &str) -> Result<(), String> {
    if Arr::has(data, key) {
        Err(format!("Failed asserting that the data does not contain the key [{key}]."))
    } else {
        Ok(())
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
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
