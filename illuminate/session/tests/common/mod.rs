//! A tiny "browser" that keeps cookies between requests sent through a
//! middleware pipeline, plus application builders.

#![allow(dead_code)]

use std::future::Future;
use std::sync::{Arc, Mutex};

use illuminate_config::Repository;
use illuminate_container::{Container, LocalInstanceGuard, ServiceProvider};
use illuminate_cookie::{AddQueuedCookiesToResponse, CookieServiceProvider, EncryptCookies};
use illuminate_encryption::EncryptionServiceProvider;
use illuminate_http::exceptions::default_render;
use illuminate_http::{
    BoxFuture, Destination, ExceptionHandler, HeaderMap, HeaderValue, Middleware, Request,
    Response, run_middleware, with_request,
};
use illuminate_session::{SessionServiceProvider, StartSession, TokenMismatchException};
use illuminate_support::{Error, Value, json};

pub const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Renders `TokenMismatchException` as `419 Page Expired`, like the foundation's handler.
pub struct TestExceptionHandler;

impl ExceptionHandler for TestExceptionHandler {
    fn report(&self, _error: &Error) {}

    fn render(&self, request: &Request, error: Error) -> Response {
        if error.downcast_ref::<TokenMismatchException>().is_some() {
            return Response::make("Page Expired", 419);
        }
        default_render(request, error)
    }
}

/// Boot an application container with the given session configuration.
pub fn app(session: Value) -> (Arc<Container>, LocalInstanceGuard) {
    app_with(json!({
        "app": {"name": "Laravel", "key": KEY, "cipher": "AES-256-CBC"},
        "session": session,
    }))
}

/// Boot an application container with the given configuration.
pub fn app_with(config: Value) -> (Arc<Container>, LocalInstanceGuard) {
    let (container, guard) = bare_app(config);
    container.instance_arc::<dyn ExceptionHandler>(Arc::new(TestExceptionHandler));
    (container, guard)
}

/// Boot an application container without an exception handler.
pub fn bare_app(config: Value) -> (Arc<Container>, LocalInstanceGuard) {
    let container = Arc::new(Container::new());
    container.instance(Repository::new(config));
    EncryptionServiceProvider.register(&container);
    CookieServiceProvider.register(&container);
    SessionServiceProvider.register(&container);
    let guard = Container::set_local_instance(container.clone());
    (container, guard)
}

/// The array-driver session configuration used by most tests.
pub fn array_session() -> Value {
    json!({
        "driver": "array",
        "lifetime": 120,
        "expire_on_close": false,
        "encrypt": false,
        "cookie": "laravel_session",
        "path": "/",
        "domain": null,
        "secure": null,
        "http_only": true,
        "same_site": "lax",
        "partitioned": false,
        "lottery": [0, 100],
    })
}

/// The web middleware group: encrypt cookies, queue cookies, start the session.
pub fn web(extra: Vec<Arc<dyn Middleware>>) -> Vec<Arc<dyn Middleware>> {
    let mut middleware: Vec<Arc<dyn Middleware>> = vec![
        Arc::new(EncryptCookies::new()),
        Arc::new(AddQueuedCookiesToResponse),
        Arc::new(StartSession::new()),
    ];
    middleware.extend(extra);
    middleware
}

/// Wrap an async closure as a pipeline destination.
pub fn destination<F, Fut>(handler: F) -> Destination
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Response> + Send + 'static,
{
    let handler = Arc::new(handler);
    Arc::new(move |request: Request| {
        let handler = handler.clone();
        Box::pin(async move { handler(request).await }) as BoxFuture<'static, Response>
    })
}

/// A browser: remembers the cookies it receives and sends them back.
pub struct Browser {
    pub cookies: Mutex<Vec<(String, String)>>,
    middleware: Vec<Arc<dyn Middleware>>,
    destination: Destination,
}

impl Browser {
    pub fn new(middleware: Vec<Arc<dyn Middleware>>, destination: Destination) -> Self {
        Self {
            cookies: Mutex::new(Vec::new()),
            middleware,
            destination,
        }
    }

    pub async fn get(&self, uri: &str) -> Response {
        self.send(uri, "GET", json!({}), HeaderMap::new()).await
    }

    pub async fn post(&self, uri: &str, input: Value) -> Response {
        self.send(uri, "POST", input, HeaderMap::new()).await
    }

    pub async fn send(
        &self,
        uri: &str,
        method: &str,
        input: Value,
        mut headers: HeaderMap,
    ) -> Response {
        let cookie_header = self
            .cookies
            .lock()
            .unwrap()
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if !cookie_header.is_empty() {
            headers.insert("cookie", HeaderValue::from_str(&cookie_header).unwrap());
        }
        let request = Request::create_with(uri, method, input, headers);
        let response = with_request(
            request.clone(),
            run_middleware(request, self.middleware.clone(), self.destination.clone()),
        )
        .await;
        self.remember(&response);
        response
    }

    /// Store cookies exactly as a browser would: the encoded value from `Set-Cookie`.
    fn remember(&self, response: &Response) {
        let mut cookies = self.cookies.lock().unwrap();
        for cookie in response.cookies() {
            cookies.retain(|(name, _)| name != &cookie.name);
            if cookie.is_cleared() {
                continue;
            }
            let header = cookie.to_header_value();
            let pair = header.split(';').next().unwrap();
            let value = pair.split_once('=').unwrap().1.to_string();
            cookies.push((cookie.name.clone(), value));
        }
    }

    pub fn cookie(&self, name: &str) -> Option<String> {
        self.cookies
            .lock()
            .unwrap()
            .iter()
            .find(|(cookie, _)| cookie == name)
            .map(|(_, value)| value.clone())
    }

    pub fn forget_cookies(&self) {
        self.cookies.lock().unwrap().clear();
    }
}
