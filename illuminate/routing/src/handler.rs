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

use std::future::Future;
use std::sync::Arc;

use illuminate_http::{IntoResponse, Request};

use crate::extract::FromRequest;
use crate::route::{RouteAction, RouteHandler};

/// A function that can handle requests routed to it.
///
/// The `Args` type parameter is a marker describing the handler's arguments;
/// you never name it yourself.
pub trait Handler<Args>: Send + Sync + Sized + 'static {
    /// Type-erase the handler into a [`RouteAction`].
    fn into_action(self) -> RouteAction;
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
        && controller.chars().next().is_some_and(char::is_uppercase) {
            return format!("{controller}@{method}");
        }
    type_name.to_string()
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
                let handler = Arc::new(self);
                let erased: RouteHandler = Arc::new(move |request: Request| {
                    let handler = handler.clone();
                    Box::pin(async move {
                        $(
                            let $arg = <$arg as FromRequest>::from_request(&request).await?;
                        )*
                        Ok(handler($($arg),*).await.into_response())
                    })
                });
                RouteAction::new(name, erased)
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
        assert_eq!(UserController::show.into_action().name, "UserController@show");
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
