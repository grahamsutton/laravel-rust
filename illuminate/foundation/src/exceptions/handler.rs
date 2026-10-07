//! The application's exception handler.

use std::any::TypeId;
use std::sync::{Arc, RwLock};

use illuminate_http::exceptions::HttpResponseException;
use illuminate_http::{ExceptionHandler, HttpException, Request, Response};
use illuminate_support::{Arr, Error, MessageBag, Value, json};

use super::pages::{render_debug_page, render_minimal_page};

type RenderCallback = Arc<dyn Fn(&Error, &Request) -> Option<Response> + Send + Sync>;
type ReportCallback = Arc<dyn Fn(&Error) -> bool + Send + Sync>;
type Preparer = Arc<dyn Fn(&Error) -> Option<Prepared> + Send + Sync>;
type DontReport = Arc<dyn Fn(&Error) -> bool + Send + Sync>;
type JsonWhen = Arc<dyn Fn(&Request, &Error) -> bool + Send + Sync>;
type Respond = Arc<dyn Fn(Response, &Error, &Request) -> Response + Send + Sync>;
type Reporter = Arc<dyn Fn(&Error, Value) + Send + Sync>;
type ViewRenderer = Arc<dyn Fn(&str, Value) -> Option<String> + Send + Sync>;
type Context = Arc<dyn Fn(&Error) -> Value + Send + Sync>;

/// What an exception means for the HTTP response, once "prepared".
#[allow(clippy::large_enum_variant)]
pub enum Prepared {
    /// A ready-made response.
    Response(Response),
    /// An HTTP error with a status code.
    Http {
        status: u16,
        message: String,
        headers: Vec<(String, String)>,
    },
    /// A validation failure.
    Validation {
        message: String,
        errors: MessageBag,
        status: u16,
        error_bag: String,
        redirect_to: Option<String>,
    },
    /// The user must log in.
    Unauthenticated {
        message: String,
        redirect_to: Option<String>,
    },
}

/// Configure how the application reports and renders exceptions.
///
/// This is what `with_exceptions(|exceptions| ...)` hands you in
/// `bootstrap/app.rs`.
#[derive(Clone, Default)]
pub struct Exceptions {
    render_callbacks: Vec<RenderCallback>,
    report_callbacks: Vec<ReportCallback>,
    preparers: Vec<Preparer>,
    dont_report: Vec<DontReport>,
    dont_flash: Vec<String>,
    json_when: Option<JsonWhen>,
    respond: Option<Respond>,
    context: Vec<Context>,
    levels: Vec<(TypeIdMatcher, String)>,
}

#[derive(Clone)]
struct TypeIdMatcher {
    _id: TypeId,
    matches: Arc<dyn Fn(&Error) -> bool + Send + Sync>,
}

impl Exceptions {
    pub fn new() -> Self {
        Self {
            dont_flash: vec![
                "current_password".into(),
                "password".into(),
                "password_confirmation".into(),
            ],
            ..Default::default()
        }
    }

    /// Register a renderable callback for a given exception type.
    ///
    /// ```ignore
    /// exceptions.render(|e: &InvalidOrderException, request| {
    ///     Some(response().make("Invalid order", 422))
    /// });
    /// ```
    pub fn render<E, F>(&mut self, callback: F) -> &mut Self
    where
        E: std::error::Error + Send + Sync + 'static,
        F: Fn(&E, &Request) -> Option<Response> + Send + Sync + 'static,
    {
        self.render_callbacks.push(Arc::new(move |error, request| {
            error.downcast_ref::<E>().and_then(|e| callback(e, request))
        }));
        self
    }

    /// Register a reportable callback for a given exception type. Return
    /// `false` to stop the default logging.
    pub fn report<E, F>(&mut self, callback: F) -> &mut Self
    where
        E: std::error::Error + Send + Sync + 'static,
        F: Fn(&E) -> bool + Send + Sync + 'static,
    {
        self.report_callbacks.push(Arc::new(move |error| {
            error.downcast_ref::<E>().map(&callback).unwrap_or(true)
        }));
        self
    }

    /// Never report exceptions of the given type.
    pub fn dont_report<E: std::error::Error + Send + Sync + 'static>(&mut self) -> &mut Self {
        self.dont_report.push(Arc::new(|error| error.is::<E>()));
        self
    }

    /// Never report exceptions matching the given predicate.
    pub fn dont_report_when(&mut self, callback: impl Fn(&Error) -> bool + Send + Sync + 'static) -> &mut Self {
        self.dont_report.push(Arc::new(callback));
        self
    }

    /// Never flash the given input keys when redirecting back after a
    /// validation failure.
    pub fn dont_flash(&mut self, keys: &[&str]) -> &mut Self {
        self.dont_flash.extend(keys.iter().map(|k| k.to_string()));
        self
    }

    /// Decide when exceptions should be rendered as JSON.
    pub fn should_render_json_when(
        &mut self,
        callback: impl Fn(&Request, &Error) -> bool + Send + Sync + 'static,
    ) -> &mut Self {
        self.json_when = Some(Arc::new(callback));
        self
    }

    /// Customize every rendered exception response.
    pub fn respond(
        &mut self,
        callback: impl Fn(Response, &Error, &Request) -> Response + Send + Sync + 'static,
    ) -> &mut Self {
        self.respond = Some(Arc::new(callback));
        self
    }

    /// Add global context to every reported exception.
    pub fn context(&mut self, callback: impl Fn(&Error) -> Value + Send + Sync + 'static) -> &mut Self {
        self.context.push(Arc::new(callback));
        self
    }

    /// Set the log level used when reporting exceptions of the given type.
    pub fn level<E: std::error::Error + Send + Sync + 'static>(&mut self, level: &str) -> &mut Self {
        self.levels.push((
            TypeIdMatcher {
                _id: TypeId::of::<E>(),
                matches: Arc::new(|error| error.is::<E>()),
            },
            level.to_string(),
        ));
        self
    }

    /// Teach the handler how to interpret an exception type (used by the
    /// framework to map validation, authentication and database exceptions).
    pub fn prepare_using(&mut self, preparer: impl Fn(&Error) -> Option<Prepared> + Send + Sync + 'static) -> &mut Self {
        self.preparers.push(Arc::new(preparer));
        self
    }
}

/// The default exception handler.
pub struct Handler {
    config: RwLock<Exceptions>,
    debug: Arc<dyn Fn() -> bool + Send + Sync>,
    environment: Arc<dyn Fn() -> String + Send + Sync>,
    reporter: RwLock<Option<Reporter>>,
    views: RwLock<Option<ViewRenderer>>,
}

impl Handler {
    /// Create a handler using the given configuration.
    pub fn new(
        config: Exceptions,
        debug: impl Fn() -> bool + Send + Sync + 'static,
        environment: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            config: RwLock::new(config),
            debug: Arc::new(debug),
            environment: Arc::new(environment),
            reporter: RwLock::new(None),
            views: RwLock::new(None),
        }
    }

    /// Replace how exceptions are logged (`(error, context)`).
    pub fn report_using(&self, reporter: impl Fn(&Error, Value) + Send + Sync + 'static) {
        *self.reporter.write().unwrap() = Some(Arc::new(reporter));
    }

    /// Render custom error views (`errors::404`) through the view layer.
    pub fn render_views_using(&self, renderer: impl Fn(&str, Value) -> Option<String> + Send + Sync + 'static) {
        *self.views.write().unwrap() = Some(Arc::new(renderer));
    }

    /// Mutate the handler's configuration.
    pub fn configure(&self, callback: impl FnOnce(&mut Exceptions)) {
        callback(&mut self.config.write().unwrap());
    }

    fn is_debug(&self) -> bool {
        (self.debug)()
    }

    /// Determine if the exception is one the framework never reports.
    fn is_internal_dont_report(&self, error: &Error) -> bool {
        if error.is::<HttpException>() || error.is::<HttpResponseException>() {
            return true;
        }
        matches!(self.prepare(error), Some(Prepared::Validation { .. } | Prepared::Unauthenticated { .. } | Prepared::Http { .. } | Prepared::Response(_)))
    }

    /// Interpret an exception for rendering.
    pub fn prepare(&self, error: &Error) -> Option<Prepared> {
        if let Some(http) = error.downcast_ref::<HttpException>() {
            return Some(Prepared::Http {
                status: http.status,
                message: http.message(),
                headers: http.headers.clone(),
            });
        }
        if let Some(exception) = error.downcast_ref::<HttpResponseException>() {
            return exception.take_response().map(Prepared::Response);
        }
        let preparers = self.config.read().unwrap().preparers.clone();
        preparers.iter().find_map(|prepare| prepare(error))
    }

    fn should_return_json(&self, request: &Request, error: &Error) -> bool {
        match self.config.read().unwrap().json_when.clone() {
            Some(callback) => callback(request, error),
            None => request.expects_json(),
        }
    }

    fn finalize(&self, response: Response, error: &Error, request: &Request) -> Response {
        match self.config.read().unwrap().respond.clone() {
            Some(respond) => respond(response, error, request),
            None => response,
        }
    }

    /// Render an HTTP error as HTML (custom error view or the minimal page).
    pub fn render_http_html(&self, status: u16, message: &str, error: &Error) -> String {
        if let Some(views) = self.views.read().unwrap().clone() {
            let data = json!({
                "exception": {"message": error.to_string(), "status": status},
                "code": status,
                "message": message,
            });
            if let Some(html) = views(&format!("errors::{status}"), data.clone()) {
                return html;
            }
            let family = format!("errors::{}xx", status / 100);
            if let Some(html) = views(&family, data) {
                return html;
            }
        }
        render_minimal_page(status, message)
    }

    fn render_prepared(&self, request: &Request, error: &Error, prepared: Prepared) -> Response {
        match prepared {
            Prepared::Response(response) => response,
            Prepared::Http { status, message, headers } => {
                let mut response = if self.should_return_json(request, error) {
                    let body = if self.is_debug() && status >= 500 {
                        self.debug_json(error)
                    } else {
                        json!({ "message": message })
                    };
                    Response::json(&body)
                } else {
                    Response::new(self.render_http_html(status, &message, error))
                };
                response = response.with_status(status);
                for (name, value) in headers {
                    response.set_header(&name, &value);
                }
                response
            }
            Prepared::Validation {
                message,
                errors,
                status,
                error_bag,
                redirect_to,
            } => {
                if self.should_return_json(request, error) {
                    return Response::json(&json!({
                        "message": message,
                        "errors": errors,
                    }))
                    .with_status(status);
                }
                let dont_flash = self.config.read().unwrap().dont_flash.clone();
                let keys: Vec<&str> = dont_flash.iter().map(String::as_str).collect();
                let input = Arr::except(&request.post_all(), &keys);
                let bag = match request.input("_error_bag") {
                    Value::String(bag) if !bag.is_empty() => bag,
                    _ => error_bag,
                };
                let target = redirect_to.unwrap_or_else(|| previous_url(request));
                Response::redirect(target)
                    .with_input(input)
                    .with_errors_in(errors, &bag)
            }
            Prepared::Unauthenticated { message, redirect_to } => {
                if self.should_return_json(request, error) {
                    return Response::json(&json!({ "message": message })).with_status(401);
                }
                match redirect_to {
                    // Remember where the user was headed, for `redirect()->intended()`.
                    Some(to) => illuminate_session::redirect_guest(&to),
                    None => Response::no_content().with_status(401),
                }
            }
        }
    }

    fn debug_json(&self, error: &Error) -> Value {
        json!({
            "message": error.to_string(),
            "exception": format!("{error:?}").lines().next().unwrap_or_default(),
            "causes": error.chain().skip(1).map(|c| c.to_string()).collect::<Vec<_>>(),
            "trace": format!("{}", error.backtrace()).lines().map(str::to_string).collect::<Vec<_>>(),
        })
    }

    fn render_server_error(&self, request: &Request, error: &Error) -> Response {
        if self.should_return_json(request, error) {
            let body = if self.is_debug() {
                self.debug_json(error)
            } else {
                json!({ "message": "Server Error" })
            };
            return Response::json(&body).with_status(500);
        }
        if self.is_debug() {
            return Response::new(render_debug_page(error, request, &(self.environment)())).with_status(500);
        }
        Response::new(self.render_http_html(500, "Server Error", error)).with_status(500)
    }

    /// Render an exception for the console.
    pub fn render_for_console(&self, error: &Error) -> String {
        let mut out = format!("\n   \x1b[41;1m ERROR \x1b[0m {error}\n");
        for cause in error.chain().skip(1) {
            out.push_str(&format!("\n   Caused by: {cause}"));
        }
        if self.is_debug() {
            let backtrace = error.backtrace().to_string();
            if !backtrace.is_empty() && !backtrace.contains("disabled") {
                out.push_str(&format!("\n\n{backtrace}"));
            }
        }
        out.push('\n');
        out
    }

    fn log_level(&self, error: &Error) -> String {
        self.config
            .read()
            .unwrap()
            .levels
            .iter()
            .find(|(matcher, _)| (matcher.matches)(error))
            .map(|(_, level)| level.clone())
            .unwrap_or_else(|| "error".to_string())
    }
}

/// Where "back" goes: the session's previous URL, the referer, or "/".
fn previous_url(request: &Request) -> String {
    match request.attribute("_previous_url") {
        Value::String(url) if !url.is_empty() => url,
        _ => request.header("referer").unwrap_or_else(|| "/".to_string()),
    }
}

impl ExceptionHandler for Handler {
    fn report(&self, error: &Error) {
        let callbacks = self.config.read().unwrap().report_callbacks.clone();
        let mut keep_logging = true;
        for callback in callbacks {
            if !callback(error) {
                keep_logging = false;
            }
        }
        if !keep_logging {
            return;
        }

        let mut context = json!({ "exception": format!("{error:?}") });
        for extra in self.config.read().unwrap().context.clone() {
            if let (Value::Object(base), Value::Object(more)) = (&mut context, extra(error)) {
                base.extend(more);
            }
        }
        if let Value::Object(map) = &mut context {
            map.insert("level".into(), Value::String(self.log_level(error)));
        }

        match self.reporter.read().unwrap().clone() {
            Some(reporter) => reporter(error, context),
            None => eprintln!("[{}] {error}", illuminate_support::Carbon::now()),
        }
    }

    fn should_report(&self, error: &Error) -> bool {
        if self.is_internal_dont_report(error) {
            return false;
        }
        !self
            .config
            .read()
            .unwrap()
            .dont_report
            .iter()
            .any(|matches| matches(error))
    }

    fn render(&self, request: &Request, error: Error) -> Response {
        let callbacks = self.config.read().unwrap().render_callbacks.clone();
        for callback in callbacks {
            if let Some(response) = callback(&error, request) {
                return self.finalize(response, &error, request);
            }
        }

        let response = match self.prepare(&error) {
            Some(prepared) => self.render_prepared(request, &error, prepared),
            None => self.render_server_error(request, &error),
        };
        let response = self.finalize(response, &error, request);
        response.with_exception(Arc::new(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("Invalid order.")]
    struct InvalidOrderException;

    fn handler(debug: bool) -> Handler {
        Handler::new(Exceptions::new(), move || debug, || "testing".into())
    }

    #[test]
    fn it_renders_http_exceptions() {
        let response = handler(false).render(&Request::create("/", "GET"), HttpException::new(404).into());
        assert_eq!(response.status_code(), 404);
        assert!(response.content_string().contains("Not Found"));
    }

    #[test]
    fn it_renders_json_when_expected() {
        let mut headers = illuminate_http::HeaderMap::new();
        headers.insert("accept", illuminate_http::HeaderValue::from_static("application/json"));
        let request = Request::create_with("/", "GET", json!({}), headers);
        let response = handler(false).render(&request, illuminate_support::error::error!("boom"));
        assert_eq!(response.status_code(), 500);
        assert_eq!(response.json_body(), json!({"message": "Server Error"}));
    }

    #[test]
    fn it_shows_the_debug_page_in_debug_mode() {
        let response = handler(true).render(&Request::create("/", "GET"), illuminate_support::error::error!("Something broke"));
        assert!(response.content_string().contains("Something broke"));
    }

    #[test]
    fn custom_renderers_win() {
        let mut exceptions = Exceptions::new();
        exceptions.render(|_: &InvalidOrderException, _| Some(Response::make("Custom", 422)));
        let handler = Handler::new(exceptions, || false, || "testing".into());
        let response = handler.render(&Request::create("/", "GET"), InvalidOrderException.into());
        assert_eq!(response.status_code(), 422);
        assert_eq!(response.content_string(), "Custom");
    }

    #[test]
    fn http_exceptions_are_not_reported() {
        let handler = handler(false);
        assert!(!handler.should_report(&HttpException::new(404).into()));
        assert!(handler.should_report(&InvalidOrderException.into()));
    }
}
