//! # Illuminate Http Resources
//!
//! Eloquent API resources: a transformation layer between your models and
//! the JSON responses your API actually returns.
//!
//! A resource wraps a model and decides, in `to_array`, which attributes
//! make it into the response — including attributes that only appear under
//! certain conditions, and relationships that only appear once they've been
//! loaded:
//!
//! ```
//! use illuminate_http::{IntoResponse, Request};
//! use illuminate_http_resources::prelude::*;
//! use illuminate_support::{Value, json};
//!
//! #[derive(Clone, serde::Serialize)]
//! pub struct Post {
//!     pub id: u64,
//!     pub title: String,
//! }
//!
//! #[derive(Clone, serde::Serialize)]
//! pub struct User {
//!     pub id: u64,
//!     pub name: String,
//!     pub posts: Option<Vec<Post>>,
//! }
//!
//! pub struct PostResource(pub Post);
//!
//! impl JsonResource for PostResource {
//!     type Model = Post;
//!
//!     fn from_model(post: Post) -> Self {
//!         Self(post)
//!     }
//!
//!     fn model(&self) -> &Post {
//!         &self.0
//!     }
//! }
//!
//! pub struct UserResource(pub User);
//!
//! impl JsonResource for UserResource {
//!     type Model = User;
//!
//!     fn from_model(user: User) -> Self {
//!         Self(user)
//!     }
//!
//!     fn model(&self) -> &User {
//!         &self.0
//!     }
//!
//!     fn to_array(&self, request: &Request) -> Value {
//!         json!({
//!             "id": self.0.id,
//!             "name": self.0.name,
//!             "posts": PostResource::collection(self.when_loaded(&self.0.posts)),
//!             "secret": self.when(request.boolean("admin"), || "secret-value"),
//!         })
//!     }
//! }
//!
//! let users = vec![
//!     User { id: 1, name: "Taylor".into(), posts: None },
//!     User { id: 2, name: "Abigail".into(), posts: Some(vec![Post { id: 7, title: "Hello".into() }]) },
//! ];
//!
//! let response = UserResource::collection(users).into_response();
//!
//! assert_eq!(response.json_body(), json!({"data": [
//!     {"id": 1, "name": "Taylor"},
//!     {"id": 2, "name": "Abigail", "posts": [{"id": 7, "title": "Hello"}]},
//! ]}));
//! ```
//!
//! ## The pieces
//!
//! - [`JsonResource`] — implement it for your resources (`UserResource`).
//!   `UserResource::make(user)` and `UserResource::collection(users)`
//!   return a [`Resource`] / [`AnonymousResourceCollection`], which become
//!   JSON responses when returned from a route and resolve in place when
//!   nested in another resource.
//! - [`ResourceCollection`] — implement it for dedicated collections
//!   (`UserCollection`) with their own links and meta data.
//! - Paginators — pass a `LengthAwarePaginator` or `Paginator` to
//!   `collection` (or a collection's `make`) and the response gets Laravel's
//!   `links` and `meta` keys.
//! - [`PotentiallyMissing`], [`MissingValue`] and [`MergeValue`] — what the
//!   conditional helpers return; [`filter`] strips and merges them.
//! - [`Resource::without_wrapping`], [`Resource::wrap`] and
//!   [`JsonResource::wrap`] — control the `data` wrapper. The settings live
//!   in the service container, so tests stay isolated.

mod collection;
mod conditional;
mod resource;
mod response;
mod source;
mod state;

pub use collection::{
    AnonymousCollection, AnonymousResourceCollection, ResourceCollection, Resources,
    ToResourceCollection,
};
pub use conditional::{Loadable, MergeValue, MissingValue, Nullable, PotentiallyMissing, filter};
pub use resource::{JsonResource, Resource, UseResource};
pub use source::{IntoResource, Pagination, Source};
pub use state::{DEFAULT_WRAPPER, HttpResourcesServiceProvider};

/// Everything you need to write resources.
///
/// ```
/// use illuminate_http_resources::prelude::*;
/// ```
pub mod prelude {
    pub use crate::{
        AnonymousResourceCollection, JsonResource, MergeValue, MissingValue, PotentiallyMissing,
        Resource, ResourceCollection, Resources, ToResourceCollection, UseResource,
    };
}

#[cfg(test)]
mod tests;
