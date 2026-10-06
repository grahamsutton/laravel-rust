//! Converting the things your routes return into responses.
//!
//! Return a string and you'll get HTML. Return a JSON value (or anything
//! wrapped in [`Json`]) and you'll get JSON. Return a `Result` and errors are
//! handed to the exception handler. It just works.

use bytes::Bytes;
use http::StatusCode;
use serde::Serialize;

use illuminate_support::{Collection, Error, HtmlString, Value};

use crate::exceptions::{HttpException, render_exception};
use crate::response::Response;

/// Anything that can be turned into an HTTP response.
pub trait IntoResponse {
    fn into_response(self) -> Response;
}

/// Serialize the wrapped data as a JSON response.
///
/// ```
/// use illuminate_http::{IntoResponse, Json};
///
/// #[derive(serde::Serialize)]
/// struct User { name: &'static str }
///
/// let response = Json(User { name: "Taylor" }).into_response();
/// assert_eq!(response.content_string(), r#"{"name":"Taylor"}"#);
/// ```
#[derive(Clone, Debug, Default)]
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        Response::json(&self.0)
    }
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        Response::new(self)
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        Response::new(self)
    }
}

impl IntoResponse for Bytes {
    fn into_response(self) -> Response {
        Response::new(self).with_header("content-type", "application/octet-stream")
    }
}

impl IntoResponse for HtmlString {
    fn into_response(self) -> Response {
        Response::new(self.0)
    }
}

impl IntoResponse for illuminate_support::Stringable {
    fn into_response(self) -> Response {
        Response::new(self.into_string())
    }
}

impl IntoResponse for Value {
    fn into_response(self) -> Response {
        Response::json(&self)
    }
}

impl<T: Serialize> IntoResponse for Collection<T> {
    fn into_response(self) -> Response {
        Response::json(&self)
    }
}

impl<T: Serialize> IntoResponse for Vec<T> {
    fn into_response(self) -> Response {
        Response::json(&self)
    }
}

impl IntoResponse for () {
    fn into_response(self) -> Response {
        Response::new(Bytes::new())
    }
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> Response {
        Response::new(Bytes::new()).with_status(self.as_u16())
    }
}

impl<T: IntoResponse> IntoResponse for (StatusCode, T) {
    fn into_response(self) -> Response {
        self.1.into_response().with_status(self.0.as_u16())
    }
}

impl<T: IntoResponse> IntoResponse for (u16, T) {
    fn into_response(self) -> Response {
        self.1.into_response().with_status(self.0)
    }
}

/// `None` becomes a `404 Not Found`.
impl<T: IntoResponse> IntoResponse for Option<T> {
    fn into_response(self) -> Response {
        match self {
            Some(value) => value.into_response(),
            None => render_exception(HttpException::new(404).into()),
        }
    }
}

/// Errors are reported and rendered by the application's exception handler.
impl<T: IntoResponse, E: Into<Error>> IntoResponse for Result<T, E> {
    fn into_response(self) -> Response {
        match self {
            Ok(value) => value.into_response(),
            Err(error) => render_exception(error.into()),
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        render_exception(self)
    }
}

impl IntoResponse for HttpException {
    fn into_response(self) -> Response {
        render_exception(self.into())
    }
}
