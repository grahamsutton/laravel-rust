//! Route handlers: async functions and closures whose arguments are
//! extracted from the request.
//!
//! Any async function or closure taking up to eight arguments that
//! implement [`FromRequest`] and returning anything that implements
//! [`IntoResponse`] is a handler:
//!
//! ```
//! use illuminate_http::Request;
//! use illuminate_routing::{Handler, Path};
//!
//! async fn show(Path(id): Path<u64>) -> String {
//!     format!("User {id}")
//! }
//!
//! async fn index(request: Request) -> String {
//!     format!("Page {}", request.query_or("page", 1))
//! }
//!
//! let show = show.into_action();
//! let index = index.into_action();
//! let hello = (|| async { "Hello World" }).into_action();
//!
//! assert_eq!(hello.name, "Closure");
//! ```
//!
//! When the request is [precognitive](Request::is_precognitive) (Laravel
//! Precognition), the arguments are still extracted — so form requests are
//! validated and models are bound — but the handler is not called: the
//! route answers with `204 No Content` and `Precognition-Success: true`.

use std::future::Future;
use std::sync::Arc;

use illuminate_http::{IntoResponse, Precognition, Request};

use crate::extract::FromRequest;
use crate::route::{RouteAction, RouteHandler};

/// A function that can handle requests routed to it.
///
/// The `Args` type parameter is a marker describing the handler's arguments;
/// you never name it yourself.
pub trait Handler<Args>: Send + Sync + Sized + 'static {
    /// Type-erase the handler into a [`RouteAction`].
    ///
    /// For a [precognitive](Request::is_precognitive) request, the action
    /// still extracts every argument — validating form requests and
    /// binding models — but answers with
    /// [`Precognition::success_response`] instead of running the handler.
    fn into_action(self) -> RouteAction;

    /// Type-erase the handler into a callback that always runs, even for
    /// precognitive requests. Routes use it for their `missing` handlers,
    /// whose response replaces the "not found" error.
    #[doc(hidden)]
    fn into_missing_handler(self) -> RouteHandler {
        self.into_action().handler
    }
}

impl Handler<RouteAction> for RouteAction {
    fn into_action(self) -> RouteAction {
        self
    }
}

/// Derive the `route:list` display name for a handler type.
///
/// Associated functions display as `Controller@method`, closures as
/// `Closure`, and free functions by their full path.
///
/// ```
/// use illuminate_routing::handler::action_name_from_type;
///
/// assert_eq!(
///     action_name_from_type("app::http::controllers::user_controller::UserController::show"),
///     "UserController@show"
/// );
/// assert_eq!(action_name_from_type("app::routes::web::{{closure}}"), "Closure");
/// assert_eq!(action_name_from_type("app::routes::home"), "app::routes::home");
/// ```
pub fn action_name_from_type(type_name: &str) -> String {
    if type_name.contains("{{closure}}") || type_name.contains("{closure") {
        return "Closure".to_string();
    }
    let path = type_name.split('<').next().unwrap_or(type_name);
    let segments: Vec<&str> = path.split("::").collect();
    if let [.., controller, method] = segments.as_slice()
        && controller.chars().next().is_some_and(char::is_uppercase)
    {
        return format!("{controller}@{method}");
    }
    type_name.to_string()
}

/// Erase a handler: extract each argument from the request, then call it.
///
/// When `$predict` is true, precognitive requests stop once the arguments
/// were extracted — Laravel's `PrecognitionCallableDispatcher`.
macro_rules! erase_handler {
    ($handler:expr, $predict:expr; $($arg:ident),*) => {{
        let handler = Arc::new($handler);
        let predict: bool = $predict;
        let erased: RouteHandler = Arc::new(move |request: Request| {
            let handler = handler.clone();
            Box::pin(async move {
                $(
                    let $arg = <$arg as FromRequest>::from_request(&request).await?;
                )*
                if predict && request.is_precognitive() {
                    return Ok(Precognition::success_response());
                }
                Ok(handler($($arg),*).await.into_response())
            })
        });
        erased
    }};
}

macro_rules! impl_handler {
    ($($arg:ident),*) => {
        #[allow(non_snake_case, unused_variables, unused_mut)]
        impl<F, Fut, Res, $($arg,)*> Handler<($($arg,)*)> for F
        where
            F: Fn($($arg),*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Res> + Send + 'static,
            Res: IntoResponse + 'static,
            $($arg: FromRequest + 'static,)*
        {
            fn into_action(self) -> RouteAction {
                let name = action_name_from_type(std::any::type_name::<F>());
                RouteAction::new(name, erase_handler!(self, true; $($arg),*))
            }

            fn into_missing_handler(self) -> RouteHandler {
                erase_handler!(self, false; $($arg),*)
            }
        }
    };
}

impl_handler!();
impl_handler!(A1);
impl_handler!(A1, A2);
impl_handler!(A1, A2, A3);
impl_handler!(A1, A2, A3, A4);
impl_handler!(A1, A2, A3, A4, A5);
impl_handler!(A1, A2, A3, A4, A5, A6);
impl_handler!(A1, A2, A3, A4, A5, A6, A7);
impl_handler!(A1, A2, A3, A4, A5, A6, A7, A8);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::Path;
    use illuminate_http::Response;
    use illuminate_support::Result;

    struct UserController;

    impl UserController {
        async fn show(Path(id): Path<u64>) -> String {
            format!("User {id}")
        }
    }

    async fn run<H: Handler<T>, T>(handler: H, request: Request) -> Response {
        let action = handler.into_action();
        (action.handler)(request).await.unwrap()
    }

    #[tokio::test]
    async fn closures_without_arguments_are_handlers() {
        let response = run(|| async { "Hello World" }, Request::default()).await;
        assert_eq!(response.content_string(), "Hello World");
        assert_eq!((|| async { "x" }).into_action().name, "Closure");
    }

    #[tokio::test]
    async fn controller_methods_are_named_after_their_type() {
        assert_eq!(
            UserController::show.into_action().name,
            "UserController@show"
        );
        let request = Request::create("/users/5", "GET");
        request.set_route_parameter("id", "5");
        let response = run(UserController::show, request).await;
        assert_eq!(response.content_string(), "User 5");
    }

    #[tokio::test]
    async fn handlers_may_return_results() {
        let response = run(
            |request: Request| async move {
                if request.has("fail") {
                    return Err(illuminate_http::HttpException::new(418));
                }
                Ok("fine")
            },
            Request::create("/?fail=1", "GET"),
        )
        .await;
        assert_eq!(response.status_code(), 418);
    }

    #[tokio::test]
    async fn extraction_failures_are_returned_as_errors() {
        let action = (|Path(id): Path<u64>| async move { id.to_string() }).into_action();
        let request = Request::create("/users/abc", "GET");
        request.set_route_parameter("id", "abc");
        let result: Result<Response> = (action.handler)(request).await;
        assert!(result.is_err());
    }
}
