//! Route model binding support.
//!
//! Laravel resolves a model type-hinted on a route handler from the route
//! parameter with the same name. In Rust, models implement
//! [`FromRequest`](crate::FromRequest) (the Eloquent `#[derive(Model)]`
//! macro generates it), and use [`route_parameter_for`] to find the value
//! and the custom binding field to look the model up by:
//!
//! ```
//! use illuminate_http::Request;
//! use illuminate_routing::{async_trait, route_parameter_for, FromRequest};
//! use illuminate_http::HttpException;
//! use illuminate_support::Result;
//!
//! struct Podcast {
//!     slug: String,
//! }
//!
//! #[async_trait]
//! impl FromRequest for Podcast {
//!     async fn from_request(request: &Request) -> Result<Self> {
//!         let (value, field) = route_parameter_for(request, "podcast")
//!             .ok_or_else(|| HttpException::new(404))?;
//!         // Query the database by `field.unwrap_or("id")` here...
//!         let _ = field;
//!         Ok(Podcast { slug: value })
//!     }
//! }
//! ```

use illuminate_http::Request;

use crate::route::CurrentRoute;

/// Find the route parameter a model should be bound from.
///
/// Returns the parameter's value and, for routes like `/posts/{post:slug}`,
/// the custom field the model should be retrieved by (`Some("slug")`).
///
/// The parameter is found by name, just like Laravel matches a route
/// segment to the handler argument's name: `default_name` is the model's
/// snake-cased type name (`BlogPost` binds `{blog_post}`). As a convenience,
/// a route whose only parameter is `{id}` binds any model. `None` means
/// there is nothing to bind from.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_http::Request;
/// use illuminate_routing::{route_parameter_for, Router};
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let router = Router::new();
/// router.get("/users/{user}/posts/{post:slug}", |request: Request| async move {
///     let (user, field) = route_parameter_for(&request, "user").unwrap();
///     let (post, post_field) = route_parameter_for(&request, "post").unwrap();
///     format!("{user}:{field:?} {post}:{post_field:?}")
/// });
///
/// let response = router.dispatch(Request::create("/users/1/posts/hello", "GET")).await;
/// assert_eq!(response.content_string(), r#"1:None hello:Some("slug")"#);
/// # });
/// ```
pub fn route_parameter_for(
    request: &Request,
    default_name: &str,
) -> Option<(String, Option<String>)> {
    let current = request.extension::<CurrentRoute>();
    let binding_field = |name: &str| {
        current
            .as_ref()
            .and_then(|route| route.binding_field_for(name))
    };

    if let Some(value) = request.route(default_name) {
        return Some((value, binding_field(default_name)));
    }

    let parameters = request.route_parameters();
    match parameters.get("id") {
        Some(value) if parameters.len() == 1 => Some((value.clone(), binding_field("id"))),
        _ => None,
    }
}

/// Determine if soft deleted models may be bound for the current route
/// (see `RouteDefinition::with_trashed`).
pub fn route_allows_trashed_bindings(request: &Request) -> bool {
    request
        .extension::<CurrentRoute>()
        .is_some_and(|route| route.allows_trashed_bindings())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_found_by_name_or_as_the_only_id_parameter() {
        let request = Request::create("/users/7", "GET");
        request.set_route_parameter("id", "7");
        assert_eq!(
            route_parameter_for(&request, "user"),
            Some(("7".into(), None))
        );
        assert_eq!(
            route_parameter_for(&request, "id"),
            Some(("7".into(), None))
        );

        request.set_route_parameter("post", "9");
        assert_eq!(route_parameter_for(&request, "user"), None);

        let request = Request::create("/users/7/teams", "GET");
        request.set_route_parameter("user", "7");
        assert_eq!(route_parameter_for(&request, "team"), None);
        request.set_route_parameter("post", "9");
        assert_eq!(
            route_parameter_for(&request, "post"),
            Some(("9".into(), None))
        );
        assert!(!route_allows_trashed_bindings(&request));
    }
}
