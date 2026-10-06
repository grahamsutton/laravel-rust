//! The authenticated user on the request.

use illuminate_http::Request;
use illuminate_support::Value;

use crate::facade::manager;
use crate::state::AuthState;
use crate::user::{AuthUser, FromAuthUser};

/// Adds the authenticated user to [`Request`].
///
/// These methods are synchronous: they return the user the request has
/// *already* authenticated — through the `auth` middleware, a login, or
/// any earlier `Auth::user().await`. Use `Auth::user().await` to resolve a
/// user that hasn't been looked up yet.
///
/// ```
/// use std::sync::Arc;
/// use illuminate_auth::{Auth, GenericUser, RequestAuthExt};
/// use illuminate_config::Repository;
/// use illuminate_container::Container;
/// use illuminate_http::{Request, with_request};
/// use illuminate_support::json;
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let container = Arc::new(Container::new());
/// let _guard = Container::set_local_instance(container.clone());
/// container.instance(Repository::new(json!({"auth": {
///     "guards": {"web": {"driver": "session", "provider": "users"}},
///     "providers": {"users": {"driver": "array", "users": [{"id": 1, "name": "Taylor"}]}},
/// }})));
///
/// let request = Request::create("/profile", "GET");
///
/// with_request(request.clone(), async {
///     Auth::login_using_id(1, false).await.unwrap();
/// }).await;
///
/// let user: GenericUser = request.user().unwrap();
/// assert_eq!(user.get("name"), json!("Taylor"));
/// assert_eq!(request.user_id(), Some(json!(1)));
/// assert_eq!(request.attribute("_auth_id"), json!(1));
/// # });
/// ```
pub trait RequestAuthExt {
    /// The authenticated user of the default guard, as a concrete type.
    fn user<U: FromAuthUser>(&self) -> Option<U>;

    /// The authenticated user of the given guard, as a concrete type.
    fn user_for<U: FromAuthUser>(&self, guard: &str) -> Option<U>;

    /// The authenticated user of the default guard, of whatever type.
    fn auth_user(&self) -> Option<AuthUser>;

    /// The ID of the authenticated user.
    fn user_id(&self) -> Option<Value>;

    /// The request's authentication state.
    fn auth_state(&self) -> std::sync::Arc<AuthState>;
}

impl RequestAuthExt for Request {
    fn user<U: FromAuthUser>(&self) -> Option<U> {
        self.auth_user().and_then(|user| U::from_auth_user(&user))
    }

    fn user_for<U: FromAuthUser>(&self, guard: &str) -> Option<U> {
        manager()
            .resolved_user(&self.auth_state(), Some(guard))
            .and_then(|user| U::from_auth_user(&user))
    }

    fn auth_user(&self) -> Option<AuthUser> {
        manager().resolved_user(&self.auth_state(), None)
    }

    fn user_id(&self) -> Option<Value> {
        self.auth_user().map(|user| user.id())
    }

    fn auth_state(&self) -> std::sync::Arc<AuthState> {
        AuthState::for_request(self)
    }
}
