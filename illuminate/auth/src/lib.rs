//! # Illuminate Auth
//!
//! Authentication and authorization, the Laravel way.
//!
//! **Authentication** is built from *guards* — which decide how users are
//! authenticated for each request (the session, an API token, a closure) —
//! and *user providers*, which retrieve users from storage. Both are
//! configured in `config/auth.php`, and used through the [`Auth`] facade:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_auth::{Auth, AuthServiceProvider, GenericUser};
//! use illuminate_config::Repository;
//! use illuminate_container::{Container, ServiceProvider};
//! use illuminate_hashing::Hash;
//! use illuminate_support::json;
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! # runtime.block_on(async {
//! let container = Arc::new(Container::new());
//! let _guard = Container::set_local_instance(container.clone());
//! # container.instance(Repository::new(json!({"hashing": {"bcrypt": {"rounds": 4}}})));
//! let password = Hash::make("secret").unwrap();
//!
//! container.instance(Repository::new(json!({
//!     "hashing": {"bcrypt": {"rounds": 4}},
//!     "auth": {
//!         "defaults": {"guard": "web"},
//!         "guards": {"web": {"driver": "session", "provider": "users"}},
//!         "providers": {"users": {"driver": "array", "users": [
//!             {"id": 1, "email": "taylor@laravel.com", "password": password},
//!         ]}},
//!     },
//! })));
//! AuthServiceProvider.register(&container);
//!
//! let credentials = json!({"email": "taylor@laravel.com", "password": "secret"});
//!
//! if Auth::attempt(&credentials, false).await.unwrap() {
//!     let user: GenericUser = Auth::user().await.unwrap();
//!     assert_eq!(user.get("email"), json!("taylor@laravel.com"));
//! }
//! # assert!(Auth::check().await);
//! # });
//! ```
//!
//! **Authorization** uses *gates* (closures) and *policies* (classes of
//! rules for a model), registered on the [`Gate`] facade:
//!
//! ```ignore
//! Gate::define("update-post", |user: &User, post: &Post| user.id == post.user_id);
//! Gate::policy::<Post, _>(PostPolicy);
//!
//! Gate::authorize("update", &post).await?;
//! if user.can("delete", &post) { /* ... */ }
//! ```
//!
//! ## Per-request state
//!
//! Guards are shared by every request the server handles concurrently, so
//! the user a guard resolves is stored on the *request* (see [`AuthState`]),
//! never in the guard. The [`RequestAuthExt`] trait reads it back:
//! `request.user::<User>()`, `request.user_id()`. Once a user is resolved,
//! the request attribute `_auth_id` holds their identifier.
//!
//! ## Integrating a user store
//!
//! Implement [`Authenticatable`] for your user type and [`UserProvider`]
//! for your storage, then register the provider driver:
//!
//! ```ignore
//! Auth::provider("eloquent", |_app, config| Arc::new(EloquentUserProvider::<User>::new(config)));
//! ```

pub mod access;
pub mod blade;
mod exceptions;
mod facade;
pub mod guards;
mod manager;
pub mod middleware;
pub mod passwords;
mod provider;
mod providers;
mod request;
mod state;
mod support;
mod user;

pub use access::{
    AccessGate, AuthResponse, Authorizable, AuthorizationException, Gate, GateArgument,
    GateArguments, IntoAbilities, IntoGateArguments, IntoGateResult, IntoMessage, Policy, UserGate,
    authorize, gate,
};
pub use exceptions::AuthenticationException;
pub use facade::{Auth, auth, manager};
pub use guards::{
    AttemptCallback, Guard, IntoUserResult, Recaller, RequestGuard, RequestGuardCallback,
    SessionGuard, TokenGuard,
};
pub use manager::{AuthHooks, AuthManager, GuardFactory, ProviderFactory, UserResolver};
pub use middleware::{
    Authenticate, AuthenticateSession, AuthenticateWithBasicAuth, Authorize, EnsureEmailIsVerified,
    MiddlewareFactory, RedirectIfAuthenticated, RequirePassword, middleware_aliases,
};
pub use passwords::{
    ArrayTokenRepository, Password, PasswordBroker, PasswordBrokerManager, PasswordStatus,
    TokenRepository, TokenRepositoryFactory, password_brokers,
};
pub use provider::AuthServiceProvider;
pub use providers::{
    ArrayUserProvider, UserProvider, matches_credentials, rehashed_password, validate_password,
};
pub use request::RequestAuthExt;
pub use state::{AuthState, GuardState};
pub use user::{
    AuthUser, AuthUserRef, Authenticatable, DynAuthenticatable, FromAuthUser, GenericUser,
    MustVerifyEmail,
};

/// The facades provided by this component.
pub mod facades {
    pub use crate::access::Gate;
    pub use crate::facade::Auth;
    pub use crate::passwords::Password;
}

/// Everything you need to authenticate and authorize, in one import.
pub mod prelude {
    pub use crate::access::{AuthResponse, Authorizable, Gate, GateArgument, Policy, authorize};
    pub use crate::facade::{Auth, auth};
    pub use crate::passwords::Password;
    pub use crate::request::RequestAuthExt;
    pub use crate::user::{AuthUser, Authenticatable, MustVerifyEmail};
}

#[cfg(test)]
mod tests;
