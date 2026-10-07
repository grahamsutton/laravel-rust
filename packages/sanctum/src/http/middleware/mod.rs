//! Sanctum's middleware.
//!
//! | Alias       | Middleware                            |
//! | ----------- | ------------------------------------- |
//! | `abilities` | [`CheckAbilities`]                    |
//! | `ability`   | [`CheckForAnyAbility`]                |
//!
//! [`EnsureFrontendRequestsAreStateful`] belongs at the start of the `api`
//! middleware group (Laravel's `$middleware->statefulApi()`), and
//! [`AuthenticateSession`] runs inside it for SPA requests.

mod authenticate_session;
mod check_abilities;
mod ensure_frontend_requests_are_stateful;

use illuminate_auth::AuthenticationException;
use illuminate_http::Request;
use illuminate_routing::MiddlewareFactory;
use illuminate_support::Result;

pub use authenticate_session::AuthenticateSession;
pub use check_abilities::{CheckAbilities, CheckForAnyAbility};
pub use ensure_frontend_requests_are_stateful::EnsureFrontendRequestsAreStateful;

use crate::access_token::AccessToken;
use crate::facade::Sanctum;

/// Every Sanctum middleware alias with its factory, ready for the router
/// (Laravel's `$middleware->alias([...])` in `bootstrap/app.php`).
///
/// ```
/// use laravel_sanctum::http::middleware::middleware_aliases;
///
/// let aliases: Vec<&str> = middleware_aliases().into_iter().map(|(alias, _)| alias).collect();
/// assert_eq!(aliases, ["abilities", "ability"]);
/// ```
pub fn middleware_aliases() -> Vec<(&'static str, MiddlewareFactory)> {
    vec![
        (CheckAbilities::ALIAS, CheckAbilities::factory()),
        (CheckForAnyAbility::ALIAS, CheckForAnyAbility::factory()),
    ]
}

/// The token the request's user authenticated with, or an
/// [`AuthenticationException`] when there's no user or token.
pub(crate) async fn authenticated_token(request: &Request) -> Result<AccessToken> {
    let user = crate::http::request_user(request).await?;
    user.and_then(|user| Sanctum::access_token_for(&user))
        .ok_or_else(|| AuthenticationException::new(Vec::new(), None).into())
}

/// The non-empty, trimmed parameters of a middleware (`abilities:a,b`).
pub(crate) fn filled(parameters: &[String]) -> Vec<String> {
    parameters
        .iter()
        .map(|parameter| parameter.trim().to_string())
        .filter(|parameter| !parameter.is_empty())
        .collect()
}
