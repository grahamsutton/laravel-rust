use std::sync::Arc;

use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_routing::MiddlewareFactory;
use illuminate_support::Result;

use super::{authenticated_token, filled};
use crate::exceptions::MissingAbilityException;
use crate::http::within_request;

/// The `abilities` middleware: the request's token must have *every* listed
/// ability.
///
/// ```ignore
/// Route::get("/orders", || async {
///     // Token has both "check-status" and "place-orders" abilities...
/// })
/// .middleware(("auth:sanctum", "abilities:check-status,place-orders"));
/// ```
///
/// Requests without an authenticated user (or token) fail with an
/// `AuthenticationException`; tokens missing an ability with a
/// [`MissingAbilityException`] (a `403`).
///
/// ```
/// use laravel_sanctum::CheckAbilities;
///
/// let middleware = CheckAbilities::from_parameters(&["check-status".into(), "place-orders".into()]);
/// assert_eq!(middleware.abilities(), ["check-status", "place-orders"]);
/// assert_eq!(CheckAbilities::using(&["check-status", "place-orders"]), "abilities:check-status,place-orders");
/// ```
#[derive(Clone, Debug, Default)]
pub struct CheckAbilities {
    abilities: Vec<String>,
}

impl CheckAbilities {
    /// The middleware's alias.
    pub const ALIAS: &'static str = "abilities";

    /// Require every one of the given abilities.
    pub fn new<I, S>(abilities: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            abilities: abilities.into_iter().map(Into::into).collect(),
        }
    }

    /// Build the middleware from route parameters (`abilities:a,b`).
    pub fn from_parameters(parameters: &[String]) -> Self {
        Self {
            abilities: filled(parameters),
        }
    }

    /// The factory registered for the `abilities` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|parameters| Arc::new(Self::from_parameters(parameters)) as Arc<dyn Middleware>)
    }

    /// The middleware name for the given abilities (`abilities:a,b`).
    pub fn using(abilities: &[&str]) -> String {
        format!("{}:{}", Self::ALIAS, abilities.join(","))
    }

    /// The abilities the token must have.
    pub fn abilities(&self) -> &[String] {
        &self.abilities
    }
}

#[async_trait]
impl Middleware for CheckAbilities {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            let token = authenticated_token(&request).await?;
            if let Some(missing) = self.abilities.iter().find(|ability| token.cant(ability)) {
                return Err(MissingAbilityException::new([missing.clone()]).into_error());
            }
            Ok(next.run(request.clone()).await)
        })
        .await
    }
}

/// The `ability` middleware: the request's token must have *at least one*
/// of the listed abilities.
///
/// ```ignore
/// Route::get("/orders", || async {
///     // Token has the "check-status" or "place-orders" ability...
/// })
/// .middleware(("auth:sanctum", "ability:check-status,place-orders"));
/// ```
///
/// ```
/// use laravel_sanctum::CheckForAnyAbility;
///
/// assert_eq!(CheckForAnyAbility::using(&["check-status", "place-orders"]), "ability:check-status,place-orders");
/// ```
#[derive(Clone, Debug, Default)]
pub struct CheckForAnyAbility {
    abilities: Vec<String>,
}

impl CheckForAnyAbility {
    /// The middleware's alias.
    pub const ALIAS: &'static str = "ability";

    /// Require any one of the given abilities.
    pub fn new<I, S>(abilities: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            abilities: abilities.into_iter().map(Into::into).collect(),
        }
    }

    /// Build the middleware from route parameters (`ability:a,b`).
    pub fn from_parameters(parameters: &[String]) -> Self {
        Self {
            abilities: filled(parameters),
        }
    }

    /// The factory registered for the `ability` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|parameters| Arc::new(Self::from_parameters(parameters)) as Arc<dyn Middleware>)
    }

    /// The middleware name for the given abilities (`ability:a,b`).
    pub fn using(abilities: &[&str]) -> String {
        format!("{}:{}", Self::ALIAS, abilities.join(","))
    }

    /// The abilities, any of which the token must have.
    pub fn abilities(&self) -> &[String] {
        &self.abilities
    }
}

#[async_trait]
impl Middleware for CheckForAnyAbility {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            let token = authenticated_token(&request).await?;
            if self.abilities.iter().any(|ability| token.can(ability)) {
                return Ok(next.run(request.clone()).await);
            }
            Err(MissingAbilityException::new(self.abilities.clone()).into_error())
        })
        .await
    }
}
