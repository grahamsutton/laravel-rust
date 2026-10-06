use std::sync::Arc;

use illuminate_http::{HttpException, Middleware, Next, Request, Response, async_trait};
use illuminate_support::Result;

use super::{MiddlewareFactory, route_url, within_request};
use crate::exceptions::redirect_guest;
use crate::facade::manager;

/// The `verified` middleware: only users who verified their e-mail address
/// may pass.
///
/// Users implementing [`MustVerifyEmail`](crate::MustVerifyEmail) who
/// haven't verified their address (and guests) get a `403` for JSON
/// requests, and are redirected to the `verification.notice` route
/// (`/email/verify`) otherwise. `verified:route.name` picks another route.
#[derive(Clone, Debug, Default)]
pub struct EnsureEmailIsVerified {
    redirect_to_route: Option<String>,
}

impl EnsureEmailIsVerified {
    /// Redirect unverified users to the `verification.notice` route.
    pub fn new() -> Self {
        Self::default()
    }

    /// Redirect unverified users to the given route.
    pub fn redirect_to(route: &str) -> Self {
        Self {
            redirect_to_route: Some(route.to_string()),
        }
    }

    /// Build the middleware from route parameters (`verified:route.name`).
    pub fn from_parameters(parameters: &[String]) -> Self {
        Self {
            redirect_to_route: parameters
                .first()
                .map(|route| route.trim().to_string())
                .filter(|route| !route.is_empty()),
        }
    }

    /// The factory registered for the `verified` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|parameters| Arc::new(Self::from_parameters(parameters)) as Arc<dyn Middleware>)
    }
}

#[async_trait]
impl Middleware for EnsureEmailIsVerified {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            let user = manager().resolve_user(None).await;
            let verified = user.as_ref().is_some_and(|user| user.has_verified_email());
            if verified {
                return Ok(next.run(request.clone()).await);
            }
            if request.expects_json() {
                return Err(HttpException::with_message(
                    403,
                    "Your email address is not verified.",
                )
                .into());
            }
            let to = route_url(
                self.redirect_to_route.as_deref(),
                "verification.notice",
                "/email/verify",
            );
            Ok(redirect_guest(&request, &to))
        })
        .await
    }
}
