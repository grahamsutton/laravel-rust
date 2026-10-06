//! Extracting typed values from the incoming request.
//!
//! Laravel inspects a route handler's type hints and injects the request,
//! route parameters, and services from the container. In Rust, each handler
//! argument implements [`FromRequest`]:
//!
//! ```
//! use illuminate_http::Request;
//! use illuminate_routing::{Inject, Input, Path, Query};
//!
//! #[derive(serde::Deserialize)]
//! struct Pagination {
//!     page: Option<u32>,
//! }
//!
//! #[derive(serde::Deserialize)]
//! struct NewPost {
//!     title: String,
//! }
//!
//! struct Mailer;
//!
//! async fn index(Query(pagination): Query<Pagination>) -> String {
//!     format!("Page {}", pagination.page.unwrap_or(1))
//! }
//!
//! async fn show(Path((user, post)): Path<(u64, String)>, request: Request) -> String {
//!     format!("{user}/{post} from {}", request.ip().unwrap_or_default())
//! }
//!
//! async fn store(Input(post): Input<NewPost>, mailer: Inject<Mailer>) -> String {
//!     post.title
//! }
//! ```

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use serde::de::DeserializeOwned;

use illuminate_container::Container;
use illuminate_http::{HttpException, Json, Request, async_trait};
use illuminate_support::error::RuntimeException;
use illuminate_support::{Error, Result};

use crate::de::{DeError, from_parameters, from_value};
use crate::route::CurrentRoute;

/// Types that can be created from the incoming request.
///
/// Implement it for your own types to use them as handler arguments. The
/// Eloquent `#[derive(Model)]` macro implements it for models, resolving
/// them from route parameters (see [`crate::route_parameter_for`]).
///
/// ```
/// use illuminate_http::Request;
/// use illuminate_routing::{async_trait, FromRequest};
/// use illuminate_support::Result;
///
/// struct Locale(String);
///
/// #[async_trait]
/// impl FromRequest for Locale {
///     async fn from_request(request: &Request) -> Result<Self> {
///         Ok(Locale(request.header_or("accept-language", "en")))
///     }
/// }
/// ```
#[async_trait]
pub trait FromRequest: Sized + Send {
    /// Create the value from the request. Errors are rendered by the
    /// exception handler (an `HttpException` keeps its status code).
    async fn from_request(request: &Request) -> Result<Self>;
}

#[async_trait]
impl FromRequest for Request {
    async fn from_request(request: &Request) -> Result<Self> {
        Ok(request.clone())
    }
}

/// Optional extraction: `None` when extraction fails.
#[async_trait]
impl<T: FromRequest> FromRequest for Option<T> {
    async fn from_request(request: &Request) -> Result<Self> {
        Ok(T::from_request(request).await.ok())
    }
}

/// Fallible extraction: hands you the error instead of rendering it.
#[async_trait]
impl<T: FromRequest> FromRequest for Result<T> {
    async fn from_request(request: &Request) -> Result<Self> {
        Ok(T::from_request(request).await)
    }
}

#[async_trait]
impl FromRequest for CurrentRoute {
    async fn from_request(request: &Request) -> Result<Self> {
        request
            .extension::<CurrentRoute>()
            .map(|route| (*route).clone())
            .ok_or_else(|| RuntimeException::new("The request has not been routed yet.").into())
    }
}

macro_rules! wrapper {
    ($name:ident) => {
        impl<T> Deref for $name<T> {
            type Target = T;

            fn deref(&self) -> &T {
                &self.0
            }
        }

        impl<T> DerefMut for $name<T> {
            fn deref_mut(&mut self) -> &mut T {
                &mut self.0
            }
        }

        impl<T> $name<T> {
            /// Unwrap the inner value.
            pub fn into_inner(self) -> T {
                self.0
            }
        }
    };
}

/// Route parameters, deserialized into a single value, a tuple (in the
/// order they appear in the URI), or a struct (by name).
///
/// Values are converted leniently (`"42"` → `42`). A parameter that can't
/// be converted (`/users/abc` into `Path<u64>`) results in a `404`, just
/// like a route constraint that doesn't match.
///
/// ```
/// use illuminate_routing::Path;
///
/// #[derive(serde::Deserialize)]
/// struct PostPath {
///     user: u64,
///     slug: String,
/// }
///
/// async fn show(Path(id): Path<u64>) -> String {
///     format!("User {id}")
/// }
///
/// async fn post(Path(params): Path<PostPath>) -> String {
///     format!("{} by {}", params.slug, params.user)
/// }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Path<T>(pub T);

wrapper!(Path);

#[async_trait]
impl<T: DeserializeOwned + Send> FromRequest for Path<T> {
    async fn from_request(request: &Request) -> Result<Self> {
        let parameters = request.route_parameters().into_iter().collect();
        match from_parameters::<T>(parameters) {
            Ok(value) => Ok(Path(value)),
            Err(error) if error.is_shape_error() => Err(RuntimeException::new(format!(
                "Unable to extract the route parameters into [{}]: {error}",
                std::any::type_name::<T>()
            ))
            .into()),
            Err(error) => Err(HttpException::with_message(
                404,
                format!("Invalid route parameter: {error}"),
            )
            .into()),
        }
    }
}

/// The query string, deserialized leniently into `T`.
///
/// A query string that can't be deserialized results in a `400 Bad Request`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Query<T>(pub T);

wrapper!(Query);

#[async_trait]
impl<T: DeserializeOwned + Send> FromRequest for Query<T> {
    async fn from_request(request: &Request) -> Result<Self> {
        from_value::<T>(request.query_all())
            .map(Query)
            .map_err(|error| invalid(400, "Failed to deserialize the query string", error))
    }
}

/// All of the request's input (query string and body, like
/// `$request->all()`), deserialized leniently into `T`.
///
/// Input that can't be deserialized results in a `422 Unprocessable Content`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Input<T>(pub T);

wrapper!(Input);

#[async_trait]
impl<T: DeserializeOwned + Send> FromRequest for Input<T> {
    async fn from_request(request: &Request) -> Result<Self> {
        from_value::<T>(request.all())
            .map(Input)
            .map_err(|error| invalid(422, "The given data was invalid", error))
    }
}

/// A JSON request body, deserialized into `T`.
///
/// Requests without a JSON `Content-Type` are rejected with a `415`; bodies
/// that can't be deserialized with a `422`. Changes made to the input by
/// middleware (such as `TrimStrings`) are respected.
#[async_trait]
impl<T: DeserializeOwned + Send> FromRequest for Json<T> {
    async fn from_request(request: &Request) -> Result<Self> {
        if !request.is_json() {
            return Err(HttpException::with_message(
                415,
                "Expected request with `Content-Type: application/json`.",
            )
            .into());
        }
        from_value::<T>(request.post_all())
            .map(Json)
            .map_err(|error| invalid(422, "The given data was invalid", error))
    }
}

fn invalid(status: u16, prefix: &str, error: DeError) -> Error {
    HttpException::with_message(status, format!("{prefix}: {error}.")).into()
}

/// A service resolved from the container — Laravel's dependency injection
/// for route handlers.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_routing::Inject;
///
/// trait Weather: Send + Sync {
///     fn forecast(&self) -> String;
/// }
///
/// async fn forecast(weather: Inject<dyn Weather>) -> String {
///     weather.forecast()
/// }
/// ```
pub struct Inject<T: ?Sized>(pub Arc<T>);

impl<T: ?Sized> Inject<T> {
    /// Take the shared service handle.
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T: ?Sized> Deref for Inject<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: ?Sized> Clone for Inject<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[async_trait]
impl<T: ?Sized + Send + Sync + 'static> FromRequest for Inject<T> {
    async fn from_request(_request: &Request) -> Result<Self> {
        Container::get_instance()
            .try_make::<T>()
            .map(Inject)
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_http::{HeaderMap, HeaderValue};
    use illuminate_support::json;
    use serde::Deserialize;

    #[derive(Deserialize, Debug, PartialEq)]
    struct Search {
        q: String,
        page: Option<u32>,
    }

    fn status(error: &Error) -> Option<u16> {
        error.downcast_ref::<HttpException>().map(|e| e.status)
    }

    #[tokio::test]
    async fn requests_extract_themselves() {
        let request = Request::create("/?a=1", "GET");
        let extracted = Request::from_request(&request).await.unwrap();
        extracted.merge(json!({"b": 2}));
        assert_eq!(request.input("b"), json!(2));
    }

    #[tokio::test]
    async fn path_parameters_are_extracted() {
        let request = Request::create("/users/7/posts/hello", "GET");
        request.set_route_parameter("user", "7");
        request.set_route_parameter("post", "hello");
        let Path((user, post)) = Path::<(u32, String)>::from_request(&request).await.unwrap();
        assert_eq!((user, post.as_str()), (7, "hello"));

        let error = Path::<u32>::from_request(&request).await.err().unwrap();
        assert_eq!(status(&error), None);
        assert!(error.to_string().contains("Expected 1 route parameter"));

        let request = Request::create("/users/abc", "GET");
        request.set_route_parameter("user", "abc");
        let error = Path::<u32>::from_request(&request).await.err().unwrap();
        assert_eq!(status(&error), Some(404));
    }

    #[tokio::test]
    async fn query_strings_are_extracted_leniently() {
        let request = Request::create("/search?q=rust&page=3", "GET");
        let Query(search) = Query::<Search>::from_request(&request).await.unwrap();
        assert_eq!(
            search,
            Search {
                q: "rust".into(),
                page: Some(3)
            }
        );

        let request = Request::create("/search?page=3", "GET");
        let error = Query::<Search>::from_request(&request).await.err().unwrap();
        assert_eq!(status(&error), Some(400));
        assert!(error.to_string().contains("missing field `q`"));
    }

    #[tokio::test]
    async fn input_merges_query_and_body() {
        let request = Request::create_with(
            "/search?page=2",
            "POST",
            json!({"q": "laravel"}),
            HeaderMap::new(),
        );
        let Input(search) = Input::<Search>::from_request(&request).await.unwrap();
        assert_eq!(
            search,
            Search {
                q: "laravel".into(),
                page: Some(2)
            }
        );
        let request = Request::create("/search", "POST");
        let error = Input::<Search>::from_request(&request).await.err().unwrap();
        assert_eq!(status(&error), Some(422));
    }

    #[tokio::test]
    async fn json_bodies_require_a_json_content_type() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let request =
            Request::create_with("/search", "POST", json!({"q": "php", "page": 1}), headers);
        let Json(search) = Json::<Search>::from_request(&request).await.unwrap();
        assert_eq!(
            search,
            Search {
                q: "php".into(),
                page: Some(1)
            }
        );

        let request =
            Request::create_with("/search", "POST", json!({"q": "php"}), HeaderMap::new());
        let error = Json::<Search>::from_request(&request).await.err().unwrap();
        assert_eq!(status(&error), Some(415));
    }

    #[tokio::test]
    async fn services_are_injected_from_the_container() {
        struct Greeter {
            greeting: &'static str,
        }
        let container = Arc::new(Container::new());
        let _guard = Container::set_local_instance(container.clone());
        container.instance(Greeter { greeting: "hi" });

        let greeter = Inject::<Greeter>::from_request(&Request::default())
            .await
            .unwrap();
        assert_eq!(greeter.greeting, "hi");

        struct Missing;
        assert!(
            Inject::<Missing>::from_request(&Request::default())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn options_and_results_never_fail() {
        let request = Request::create("/", "GET");
        let missing = Option::<Path<u32>>::from_request(&request).await.unwrap();
        assert!(missing.is_none());
        let result = Result::<Path<u32>>::from_request(&request).await.unwrap();
        assert!(result.is_err());
    }
}
