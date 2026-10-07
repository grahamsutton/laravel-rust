//! Sending a pending request: retries, redirects, the middleware stack,
//! fakes, recording, events and finally the transport.
//!
//! Each attempt flows through these layers, outermost first:
//!
//! ```text
//! retries → redirects (+ cookies) → middleware → "before sending" callbacks
//!         → RequestSending event → fakes / stray request guard → transport
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http::header::{
    AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE, USER_AGENT,
};
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use illuminate_events::Dispatcher;
use illuminate_support::error::InvalidArgumentException;
use illuminate_support::{Error, Map, Result, Sleep, Str, Value, ValueExt};
use url::Url;

use crate::cookies::{Cookie, CookieJar};
use crate::encoding::{
    Part, boundary, build_form, build_query, encode_multipart, normalize_pairs, parse_form,
    parts_from_data,
};
use crate::events::{ConnectionFailed, RequestSending, ResponseReceived};
use crate::exceptions::{ConnectionException, RequestException, StrayRequestException};
use crate::factory::Factory;
use crate::fake::Outcome;
use crate::middleware::{Handler, build_stack};
use crate::pending_request::{BodyFormat, CallOptions, PendingRequest, RetryDelay};
use crate::request::Request;
use crate::response::Response;
use crate::transport::{Transport, TransportOptions, Verify};
use crate::uri_template;

/// The default `User-Agent` header.
const DEFAULT_USER_AGENT: &str = concat!("illuminate-http-client/", env!("CARGO_PKG_VERSION"));

/// The number of redirects followed by default.
const DEFAULT_MAX_REDIRECTS: usize = 5;

impl PendingRequest {
    /// Send the request, retrying as configured.
    pub(crate) async fn execute(
        mut self,
        method: Method,
        url: String,
        call: CallOptions,
    ) -> Result<Response> {
        let max_attempts = self.tries.attempts();
        let mut attempt = 1;

        loop {
            let error = match self.attempt(&method, &url, &call).await {
                Ok(response) if !response.failed() => return Ok(response),
                Ok(response) => {
                    let exception =
                        RequestException::with_truncation(response.clone(), self.truncation);
                    let error = Error::from(exception.clone());
                    let should_retry = self.should_retry(&error);
                    let can_retry = attempt < max_attempts && should_retry;

                    if self.should_throw(&response) {
                        if let Some(callback) = &self.throw_callback {
                            callback(&response, &exception);
                        }
                        if !can_retry {
                            return Err(error);
                        }
                    } else if !can_retry {
                        return if max_attempts > 1 && self.retry_throw {
                            Err(error)
                        } else {
                            Ok(response)
                        };
                    }

                    error
                }
                Err(error) => {
                    if !error.is::<ConnectionException>()
                        || attempt >= max_attempts
                        || !self.should_retry(&error)
                    {
                        return Err(error);
                    }
                    error
                }
            };

            self.sleep_before_retry(attempt, &error).await;
            attempt += 1;
        }
    }

    fn should_retry(&mut self, error: &Error) -> bool {
        match self.retry_when.clone() {
            Some(when) => when(error, self),
            None => true,
        }
    }

    fn should_throw(&self, response: &Response) -> bool {
        self.throw_callback.is_some()
            && self
                .throw_if_callback
                .as_ref()
                .is_none_or(|condition| condition(response))
    }

    async fn sleep_before_retry(&self, attempt: u32, error: &Error) {
        let milliseconds = match (&self.tries, &self.retry_delay) {
            (crate::pending_request::Tries::Backoff(backoff), _)
                if backoff.len() >= attempt as usize =>
            {
                backoff[attempt as usize - 1]
            }
            (_, RetryDelay::Milliseconds(milliseconds)) => *milliseconds,
            (_, RetryDelay::Using(callback)) => callback(attempt, error),
        };

        if milliseconds > 0 {
            Sleep::for_(milliseconds as f64).milliseconds().await;
        }
    }

    /// Make a single attempt: follow redirects through the full stack.
    async fn attempt(&self, method: &Method, url: &str, call: &CallOptions) -> Result<Response> {
        let request = self.build_request(method, url, call)?;
        let exchange = Arc::new(Exchange::new(self));
        let mut jar = self.cookie_jar();

        match self
            .follow_redirects(request.clone(), &exchange, &mut jar)
            .await
        {
            Ok(mut response) => {
                response.set_cookies(jar);
                response.set_truncation(self.truncation);

                let request = exchange.last_request().unwrap_or(request);
                if let Some(dispatcher) = &exchange.dispatcher {
                    dispatcher
                        .dispatch(ResponseReceived {
                            request: request.clone(),
                            response: response.clone(),
                        })
                        .await?;
                }

                for callback in &self.after_response {
                    response = callback(response, &request);
                }

                Ok(response)
            }
            Err(error) => {
                if let Some(exception) = error.downcast_ref::<ConnectionException>() {
                    let request = exchange.last_request().unwrap_or(request);
                    if let Some(factory) = &self.factory {
                        factory.record_request_response_pair(request.clone(), None);
                    }
                    if let Some(dispatcher) = &exchange.dispatcher {
                        dispatcher
                            .dispatch(ConnectionFailed {
                                request,
                                exception: exception.clone(),
                            })
                            .await?;
                    }
                }
                Err(error)
            }
        }
    }

    async fn follow_redirects(
        &self,
        mut request: Request,
        exchange: &Arc<Exchange>,
        jar: &mut CookieJar,
    ) -> Result<Response> {
        let max_redirects = self.max_redirects_option();
        let stack = build_stack(&self.middleware, exchange.clone().handler());
        let mut redirects = 0;

        loop {
            let current = Url::parse(request.url()).ok();

            let mut hop = request.clone();
            if let Some(cookie) = current.as_ref().and_then(|url| jar.header_for(url))
                && let Ok(value) = HeaderValue::from_str(&cookie)
            {
                hop.headers_mut().insert(COOKIE, value);
            }

            let response = stack(hop).await?;

            if let Some(url) = &current {
                let cookies = response
                    .headers()
                    .get_all(SET_COOKIE)
                    .iter()
                    .filter_map(|value| value.to_str().ok());
                jar.extract(cookies, url);
            }

            let Some(max) = max_redirects else {
                return Ok(response);
            };

            let next = match (&current, redirect_location(&response)) {
                (Some(current), Some(location)) => current
                    .join(location)
                    .ok()
                    .filter(|next| matches!(next.scheme(), "http" | "https")),
                _ => None,
            };

            let (Some(current), Some(next)) = (current, next) else {
                return Ok(response);
            };

            if redirects >= max {
                return Err(ConnectionException::new(format!(
                    "Will not follow more than {max} redirects"
                ))
                .into());
            }

            redirects += 1;
            request = redirect_request(request, &response, &current, next);
        }
    }

    // ------------------------------------------------------------------
    // Building the request
    // ------------------------------------------------------------------

    pub(crate) fn build_request(
        &self,
        method: &Method,
        url: &str,
        call: &CallOptions,
    ) -> Result<Request> {
        let url = self.resolve_url(url);

        let mut query = self
            .options
            .get("query")
            .cloned()
            .map(normalize_pairs)
            .unwrap_or(Value::Null);
        if let Some(extra) = call.query.clone() {
            match (&mut query, extra) {
                (Value::Object(query), Value::Object(extra)) => query.extend(extra),
                (query, extra) => *query = extra,
            }
        }
        let url = append_query(&url, &build_query(&query));

        let body = self.prepare_body(call)?;

        let data = match body.data {
            Some(data) => data,
            None => match url.split_once('?') {
                Some((_, query)) => parse_form(query.split('#').next().unwrap_or_default()),
                None => Value::Object(Map::new()),
            },
        };

        let mut headers = HeaderMap::new();
        if !self.has_header("User-Agent") {
            headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT));
        }

        if let Some(Value::Object(configured)) = self.options.get("headers") {
            for (name, value) in configured {
                let header = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                    InvalidArgumentException::new(format!("Invalid HTTP header name [{name}]."))
                })?;
                let values = match value {
                    Value::Array(values) if !values.is_empty() => values.clone(),
                    Value::Array(_) => vec![Value::Null],
                    other => vec![other.clone()],
                };
                for value in values {
                    let value = HeaderValue::from_bytes(value.to_string_lossy().as_bytes())
                        .map_err(|_| {
                            InvalidArgumentException::new(format!(
                                "Invalid value for the HTTP header [{name}]."
                            ))
                        })?;
                    headers.append(header.clone(), value);
                }
            }
        }

        if let Some(content_type) = body.content_type {
            let replace = match headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
            {
                None => true,
                Some(existing) => {
                    content_type.starts_with("multipart/")
                        && existing.starts_with("multipart/")
                        && !existing.contains("boundary=")
                }
            };
            if replace {
                headers.insert(CONTENT_TYPE, HeaderValue::from_str(&content_type)?);
            }
        }

        if let Some((username, password, scheme)) = self.auth()
            && scheme == "basic"
        {
            let credentials =
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Basic {credentials}"))?,
            );
        }

        Ok(
            Request::from_parts(method.clone(), url, headers, body.bytes)
                .with_data(data)
                .with_parts(body.parts)
                .with_attributes(self.attributes.clone()),
        )
    }

    fn resolve_url(&self, url: &str) -> String {
        let url = if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            let joined = format!(
                "{}/{}",
                self.base_url.trim_end_matches('/'),
                url.trim_start_matches('/')
            );
            joined.trim_start_matches('/').to_string()
        };

        if self.url_parameters.is_empty() || !url.contains('{') {
            return url;
        }

        uri_template::expand(&url, &self.url_parameters)
    }

    fn prepare_body(&self, call: &CallOptions) -> Result<PreparedBody> {
        let data = call.data.clone().filter(|data| !data.is_null());

        Ok(match self.body_format {
            BodyFormat::Body => PreparedBody {
                bytes: self.pending_body.clone().unwrap_or_default(),
                content_type: None,
                data: Some(Value::Object(Map::new())),
                parts: Vec::new(),
            },
            BodyFormat::Json => match data {
                Some(data) => PreparedBody {
                    bytes: Bytes::from(serde_json::to_vec(&data)?),
                    content_type: Some("application/json".into()),
                    data: Some(data),
                    parts: Vec::new(),
                },
                None => PreparedBody::empty(),
            },
            BodyFormat::Form => match data.map(normalize_pairs) {
                Some(data) => PreparedBody {
                    bytes: Bytes::from(build_form(&data)),
                    content_type: Some("application/x-www-form-urlencoded".into()),
                    data: Some(match &data {
                        Value::String(raw) => parse_form(raw),
                        _ => data,
                    }),
                    parts: Vec::new(),
                },
                None => PreparedBody::empty(),
            },
            BodyFormat::Multipart => {
                let mut parts = data.as_ref().map(parts_from_data).unwrap_or_default();
                parts.extend(self.pending_files.iter().cloned());

                let boundary = boundary();
                PreparedBody {
                    bytes: encode_multipart(&parts, &boundary),
                    content_type: Some(format!("multipart/form-data; boundary={boundary}")),
                    data: Some(Value::Array(parts.iter().map(Part::to_value).collect())),
                    parts,
                }
            }
        })
    }

    fn has_header(&self, name: &str) -> bool {
        matches!(self.options.get("headers"), Some(Value::Object(headers)) if headers.keys().any(|key| key.eq_ignore_ascii_case(name)))
    }

    /// The configured credentials: `(username, password, "basic" | "digest")`.
    fn auth(&self) -> Option<(String, String, String)> {
        let Some(Value::Array(auth)) = self.options.get("auth") else {
            return None;
        };
        let username = auth.first()?.to_string_lossy();
        let password = auth
            .get(1)
            .map(ValueExt::to_string_lossy)
            .unwrap_or_default();
        let scheme = auth
            .get(2)
            .map(|scheme| scheme.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "basic".to_string());
        Some((username, password, scheme))
    }

    fn max_redirects_option(&self) -> Option<usize> {
        match self.options.get("allow_redirects") {
            Some(Value::Bool(false)) => None,
            Some(Value::Object(options)) => Some(
                options
                    .get("max")
                    .and_then(ValueExt::to_i64_lossy)
                    .map(|max| max.max(0) as usize)
                    .unwrap_or(DEFAULT_MAX_REDIRECTS),
            ),
            _ => Some(DEFAULT_MAX_REDIRECTS),
        }
    }

    fn cookie_jar(&self) -> CookieJar {
        let mut jar = CookieJar::new();
        if let Some(Value::Array(cookies)) = self.options.get("cookies") {
            for cookie in cookies {
                let field = |key: &str| {
                    cookie
                        .get(key)
                        .map(ValueExt::to_string_lossy)
                        .unwrap_or_default()
                };
                jar.set(Cookie::new(field("name"), field("value"), field("domain")));
            }
        }
        jar
    }

    pub(crate) fn transport_options(&self) -> TransportOptions {
        let seconds = |key: &str| {
            self.options
                .get(key)
                .and_then(ValueExt::to_f64_lossy)
                .filter(|seconds| *seconds > 0.0)
                .map(Duration::from_secs_f64)
        };

        let verify = match self.options.get("verify") {
            Some(Value::Bool(false)) => Verify::Disabled,
            Some(Value::String(path)) if !path.is_empty() => Verify::Bundle(PathBuf::from(path)),
            _ => Verify::Default,
        };

        let digest = self
            .auth()
            .filter(|(_, _, scheme)| scheme == "digest")
            .map(|(username, password, _)| (username, password));

        TransportOptions {
            timeout: seconds("timeout"),
            connect_timeout: seconds("connect_timeout"),
            verify,
            digest,
        }
    }
}

struct PreparedBody {
    bytes: Bytes,
    content_type: Option<String>,
    data: Option<Value>,
    parts: Vec<Part>,
}

impl PreparedBody {
    fn empty() -> Self {
        Self {
            bytes: Bytes::new(),
            content_type: None,
            data: None,
            parts: Vec::new(),
        }
    }
}

/// Append a query string to the URL (before any fragment).
fn append_query(url: &str, query: &str) -> String {
    if query.is_empty() {
        return url.to_string();
    }

    let (base, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (url, None),
    };

    let separator = if !base.contains('?') {
        "?"
    } else if base.ends_with('?') || base.ends_with('&') {
        ""
    } else {
        "&"
    };

    match fragment {
        Some(fragment) => format!("{base}{separator}{query}#{fragment}"),
        None => format!("{base}{separator}{query}"),
    }
}

fn redirect_location(response: &Response) -> Option<&str> {
    if !matches!(response.status(), 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    response.headers().get(LOCATION)?.to_str().ok()
}

/// Build the request for the next hop of a redirect, like browsers do:
/// `303`s (and `301` / `302` for unsafe methods) become body-less `GET`s,
/// and credentials are never sent to another origin.
fn redirect_request(previous: Request, response: &Response, from: &Url, to: Url) -> Request {
    let method = previous.method().to_string();
    let switch_to_get = response.status() == 303
        || (response.status() <= 302 && !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS"));

    let mut next = previous.with_url(to.to_string());

    if switch_to_get {
        let method = if matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS") {
            method.as_str()
        } else {
            "GET"
        };
        next = next
            .with_method(method)
            .with_body(Bytes::new())
            .with_data(Value::Object(Map::new()))
            .with_parts(Vec::new())
            .without_header(CONTENT_TYPE.as_str())
            .without_header(CONTENT_LENGTH.as_str());
    }

    if from.origin() != to.origin() {
        next = next
            .without_header(AUTHORIZATION.as_str())
            .without_header(COOKIE.as_str());
    }

    next
}

/// The innermost layers of the stack, shared by every hop of an attempt.
struct Exchange {
    factory: Option<Arc<Factory>>,
    transport: Arc<Transport>,
    options: TransportOptions,
    before_sending: Vec<crate::pending_request::BeforeSending>,
    sink: Option<PathBuf>,
    dispatcher: Option<Arc<Dispatcher>>,
    last_request: Mutex<Option<Request>>,
}

impl Exchange {
    fn new(pending: &PendingRequest) -> Self {
        let transport = match &pending.factory {
            Some(factory) => factory.transport(),
            None => Arc::new(Transport::new()),
        };

        let dispatcher = pending
            .factory
            .as_ref()
            .and_then(|factory| factory.get_dispatcher());

        Self {
            factory: pending.factory.clone(),
            transport,
            options: pending.transport_options(),
            before_sending: pending.before_sending.clone(),
            sink: match pending.options.get("sink") {
                Some(Value::String(path)) if !path.is_empty() => Some(PathBuf::from(path)),
                _ => None,
            },
            dispatcher,
            last_request: Mutex::new(None),
        }
    }

    fn handler(self: Arc<Self>) -> Handler {
        Arc::new(move |request| {
            let exchange = self.clone();
            Box::pin(async move { exchange.handle(request).await })
        })
    }

    fn last_request(&self) -> Option<Request> {
        self.last_request.lock().unwrap().clone()
    }

    async fn handle(&self, mut request: Request) -> Result<Response> {
        for callback in &self.before_sending {
            callback(&mut request);
        }

        *self.last_request.lock().unwrap() = Some(request.clone());

        if let Some(dispatcher) = &self.dispatcher {
            dispatcher
                .dispatch(RequestSending {
                    request: request.clone(),
                })
                .await?;
        }

        let stubbed = match &self.factory {
            Some(factory) => factory.stub_for(&request)?,
            None => None,
        };

        let mut response = match stubbed {
            Some(Outcome::Response(response)) => response,
            Some(Outcome::FailedConnection(message)) => {
                return Err(ConnectionException::new(message).into());
            }
            None => {
                if let Some(factory) = &self.factory
                    && !factory.is_allowed_request_url(request.url())
                {
                    return Err(StrayRequestException::new(request.url()).into());
                }
                self.transport.send(&request, &self.options).await?
            }
        };

        response.set_effective_uri(request.url());

        if let Some(sink) = &self.sink {
            tokio::fs::write(sink, response.bytes()).await?;
        }

        if let Some(factory) = &self.factory {
            factory.record_request_response_pair(request, Some(response.clone()));
        }

        Ok(response)
    }
}

/// Determine if the URL matches a fake's URL pattern.
pub(crate) fn url_matches(pattern: &str, url: &str) -> bool {
    Str::is(&Str::start(pattern, "*"), url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn build(pending: PendingRequest, method: Method, url: &str, call: CallOptions) -> Request {
        pending.build_request(&method, url, &call).unwrap()
    }

    #[test]
    fn urls_are_resolved_against_the_base_url() {
        let pending = PendingRequest::new().base_url("https://api.example.com/v1/");
        assert_eq!(
            pending.resolve_url("/users"),
            "https://api.example.com/v1/users"
        );
        assert_eq!(
            pending.resolve_url("users"),
            "https://api.example.com/v1/users"
        );
        assert_eq!(
            pending.resolve_url("http://other.test/x"),
            "http://other.test/x"
        );
        assert_eq!(PendingRequest::new().resolve_url("/users"), "users");
    }

    #[test]
    fn query_strings_are_appended() {
        assert_eq!(
            append_query("http://a.test/x", "a=1"),
            "http://a.test/x?a=1"
        );
        assert_eq!(
            append_query("http://a.test/x?b=2", "a=1"),
            "http://a.test/x?b=2&a=1"
        );
        assert_eq!(
            append_query("http://a.test/x?", "a=1"),
            "http://a.test/x?a=1"
        );
        assert_eq!(
            append_query("http://a.test/x#top", "a=1"),
            "http://a.test/x?a=1#top"
        );
        assert_eq!(append_query("http://a.test/x", ""), "http://a.test/x");
    }

    #[test]
    fn json_bodies_are_built_by_default() {
        let request = build(
            PendingRequest::new(),
            Method::POST,
            "http://a.test/users",
            CallOptions {
                query: None,
                data: Some(json!({"name": "Taylor"})),
            },
        );

        assert_eq!(request.body(), r#"{"name":"Taylor"}"#);
        assert!(request.has_header_value("Content-Type", "application/json"));
        assert!(request.has_header_value("User-Agent", DEFAULT_USER_AGENT));
        assert_eq!(request["name"], "Taylor");

        let empty = build(
            PendingRequest::new(),
            Method::GET,
            "http://a.test",
            CallOptions::default(),
        );
        assert!(empty.body().is_empty());
        assert!(!empty.has_header("Content-Type"));
    }

    #[test]
    fn get_requests_expose_their_query_as_data() {
        let request = build(
            PendingRequest::new().with_query_parameters(json!({"page": 1})),
            Method::GET,
            "http://a.test/users?sort=name",
            CallOptions {
                query: Some(json!({"name": "Taylor"})),
                data: None,
            },
        );

        assert_eq!(
            request.url(),
            "http://a.test/users?sort=name&page=1&name=Taylor"
        );
        assert_eq!(
            request.data(),
            &json!({"sort": "name", "page": "1", "name": "Taylor"})
        );
    }

    #[test]
    fn form_and_multipart_bodies_are_encoded() {
        let form = build(
            PendingRequest::new().as_form(),
            Method::POST,
            "http://a.test",
            CallOptions {
                query: None,
                data: Some(json!({"name": "Sara Smith"})),
            },
        );
        assert_eq!(form.body(), "name=Sara+Smith");
        assert!(form.is_form());
        assert_eq!(form["name"], "Sara Smith");

        let multipart = build(
            PendingRequest::new()
                .as_json()
                .attach("photo", "PNG", "photo.png"),
            Method::POST,
            "http://a.test",
            CallOptions {
                query: None,
                data: Some(json!({"name": "Taylor"})),
            },
        );
        let content_type = multipart.header("Content-Type").remove(0);
        assert!(
            content_type.starts_with("multipart/form-data; boundary="),
            "{content_type}"
        );
        assert!(multipart.has_file_with("photo", Some("PNG"), Some("photo.png")));
        assert_eq!(multipart.data()[0]["name"], "name");
        assert!(multipart.body().contains("filename=\"photo.png\""));
    }

    #[test]
    fn headers_and_auth_are_applied() {
        let request = build(
            PendingRequest::new()
                .with_headers([("X-Many", "a")])
                .with_header("x-many", "b")
                .replace_headers([("X-Replaced", "1")])
                .replace_headers([("x-replaced", "2")])
                .with_user_agent("Laravel")
                .with_basic_auth("taylor", "secret"),
            Method::GET,
            "http://a.test",
            CallOptions::default(),
        );

        assert_eq!(request.header("X-Many"), ["a", "b"]);
        assert_eq!(request.header("X-Replaced"), ["2"]);
        assert_eq!(request.header("User-Agent"), ["Laravel"]);
        assert_eq!(
            request.header("Authorization"),
            ["Basic dGF5bG9yOnNlY3JldA=="]
        );

        let invalid = PendingRequest::new().with_header("Bad Header", "x");
        assert!(
            invalid
                .build_request(&Method::GET, "http://a.test", &CallOptions::default())
                .is_err()
        );
    }

    #[test]
    fn transport_options_are_read_from_the_options() {
        let options = PendingRequest::new()
            .timeout(2.5)
            .connect_timeout(0)
            .without_verifying()
            .with_digest_auth("taylor", "secret")
            .transport_options();

        assert_eq!(options.timeout, Some(Duration::from_millis(2500)));
        assert_eq!(options.connect_timeout, None);
        assert_eq!(options.verify, Verify::Disabled);
        assert_eq!(options.digest, Some(("taylor".into(), "secret".into())));

        let options = PendingRequest::new()
            .with_options(json!({"verify": "/ca.pem"}))
            .transport_options();
        assert_eq!(options.verify, Verify::Bundle("/ca.pem".into()));
        assert_eq!(options.timeout, Some(Duration::from_secs(30)));
    }

    #[test]
    fn redirects_are_configurable() {
        assert_eq!(PendingRequest::new().max_redirects_option(), Some(5));
        assert_eq!(
            PendingRequest::new()
                .max_redirects(2)
                .max_redirects_option(),
            Some(2)
        );
        assert_eq!(
            PendingRequest::new()
                .without_redirecting()
                .max_redirects_option(),
            None
        );
        assert_eq!(
            PendingRequest::new()
                .with_options(json!({"allow_redirects": false}))
                .max_redirects_option(),
            None
        );
    }

    #[test]
    fn redirected_requests_follow_browser_semantics() {
        let from = Url::parse("https://a.test/login").unwrap();
        let post = Request::new("POST", from.as_str())
            .with_header("Content-Type", "application/json")
            .with_header("Authorization", "Bearer x")
            .with_body("{}");

        let found = Response::new(302, HeaderMap::new(), "");
        let next = redirect_request(
            post.clone(),
            &found,
            &from,
            Url::parse("https://a.test/home").unwrap(),
        );
        assert_eq!(next.method(), "GET");
        assert!(next.body().is_empty());
        assert!(!next.has_header("Content-Type"));
        assert!(next.has_header("Authorization"));

        let temporary = Response::new(307, HeaderMap::new(), "");
        let next = redirect_request(
            post,
            &temporary,
            &from,
            Url::parse("https://b.test/home").unwrap(),
        );
        assert_eq!(next.method(), "POST");
        assert_eq!(next.body(), "{}");
        assert!(!next.has_header("Authorization"));
    }

    #[test]
    fn url_patterns_match_like_laravel() {
        assert!(url_matches("github.com/*", "https://github.com/laravel"));
        assert!(url_matches("*", "https://laravel.com"));
        assert!(url_matches("https://laravel.com", "https://laravel.com"));
        assert!(!url_matches(
            "github.com/*",
            "https://laravel.com/github.com"
        ));
    }
}
