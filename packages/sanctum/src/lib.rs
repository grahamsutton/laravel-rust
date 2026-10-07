//! # Laravel Sanctum
//!
//! A featherweight authentication system for SPAs (single page
//! applications), mobile applications, and simple, token based APIs.
//!
//! Sanctum solves two problems:
//!
//! - **API tokens.** Users issue themselves "personal access tokens" (like
//!   GitHub's), optionally restricted to a set of *abilities*. Tokens are
//!   stored hashed in the `personal_access_tokens` table and sent in the
//!   `Authorization: Bearer` header.
//! - **SPA authentication.** Your own first-party SPA uses Laravel's regular
//!   cookie-based session authentication — with CSRF protection — whenever
//!   its requests come from one of your `sanctum.stateful` domains.
//!
//! Both are handled by the `sanctum` guard:
//!
//! ```ignore
//! Route::get("/user", |request: Request| async move {
//!     request.user::<User>()
//! })
//! .middleware("auth:sanctum");
//! ```
//!
//! ## Issuing tokens
//!
//! Every authenticatable Eloquent model has the [`HasApiTokens`] methods:
//!
//! ```
//! use std::sync::Arc;
//! use illuminate_auth::Authenticatable;
//! use illuminate_config::Repository;
//! use illuminate_container::{Container, ServiceProvider};
//! use illuminate_database::eloquent::*;
//! use illuminate_database::{DatabaseServiceProvider, Migration, Schema};
//! use laravel_sanctum::{CreatePersonalAccessTokensTable, HasApiTokens, PersonalAccessToken};
//!
//! #[derive(Debug, Clone, Default, Model)]
//! #[fillable(name)]
//! pub struct User {
//!     pub id: u64,
//!     pub name: String,
//! }
//!
//! impl Authenticatable for User {
//!     fn auth_identifier(&self) -> Value {
//!         json!(self.id)
//!     }
//!
//!     fn auth_password(&self) -> String {
//!         String::new()
//!     }
//! }
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> illuminate_support::Result<()> {
//! # let container = Arc::new(Container::new());
//! # let _guard = Container::set_local_instance(container.clone());
//! # container.instance(illuminate_config::Repository::new(json!({"database": {
//! #     "default": "sqlite",
//! #     "connections": {"sqlite": {"driver": "sqlite", "database": ":memory:"}},
//! # }})));
//! # DatabaseServiceProvider.register(&container);
//! # Schema::create("users", |table| { table.id(); table.string("name"); }).await?;
//! # CreatePersonalAccessTokensTable.up().await?;
//! let user = User::create(json!({"name": "Taylor"})).await?;
//!
//! let token = user.create_token("deploy-bot", &["server:update"]).await?;
//! assert!(token.plain_text_token.starts_with("1|"));
//!
//! let found = PersonalAccessToken::find_token(&token.plain_text_token).await?.unwrap();
//! assert_eq!(found.name, "deploy-bot");
//! assert!(found.can("server:update"));
//! assert!(found.cant("server:delete"));
//!
//! assert_eq!(user.tokens().count().await?, 1);
//! # Ok(())
//! # }
//! ```
//!
//! ## Wiring it up
//!
//! - Register [`SanctumServiceProvider`]: it defines the `sanctum` guard
//!   (no `config/auth.php` change needed), merges the `sanctum.*` defaults
//!   (see [`config`]), and registers `GET /sanctum/csrf-cookie`.
//! - Alias the [`CheckAbilities`] (`abilities`) and [`CheckForAnyAbility`]
//!   (`ability`) middleware ([`middleware_aliases`]).
//! - Put [`EnsureFrontendRequestsAreStateful`] first in the `api`
//!   middleware group to authenticate your SPA.
//! - Run the [`CreatePersonalAccessTokensTable`] migration, and schedule the
//!   [`PruneExpired`] command (`sanctum:prune-expired`).
//!
//! ## Testing
//!
//! [`Sanctum::acting_as`] authenticates a user with a token holding the
//! given abilities:
//!
//! ```ignore
//! Sanctum::acting_as(User::factory().create().await?, &["view-tasks"], None);
//!
//! test.get("/api/task").await.assert_ok();
//! ```

pub mod config;
pub mod http;

mod access_token;
mod console;
mod events;
mod exceptions;
mod facade;
mod guard;
mod has_api_tokens;
mod migration;
mod personal_access_token;
mod provider;
mod support;

pub use access_token::{AccessToken, HasAbilities, TransientToken};
pub use console::PruneExpired;
pub use events::TokenAuthenticated;
pub use exceptions::MissingAbilityException;
pub use facade::{Sanctum, TokenableResolver};
pub use guard::SanctumGuard;
pub use has_api_tokens::{HasApiTokens, NewAccessToken};
pub use http::middleware::{
    AuthenticateSession, CheckAbilities, CheckForAnyAbility, EnsureFrontendRequestsAreStateful,
    middleware_aliases,
};
pub use http::{CSRF_COOKIE_ROUTE, CsrfCookieController, define_routes};
pub use migration::{CreatePersonalAccessTokensTable, PERSONAL_ACCESS_TOKENS_TABLE};
pub use personal_access_token::PersonalAccessToken;
pub use provider::SanctumServiceProvider;

/// Everything you need to issue and check API tokens, in one import.
pub mod prelude {
    pub use crate::access_token::{AccessToken, HasAbilities};
    pub use crate::facade::Sanctum;
    pub use crate::has_api_tokens::{HasApiTokens, NewAccessToken};
    pub use crate::personal_access_token::PersonalAccessToken;
}
