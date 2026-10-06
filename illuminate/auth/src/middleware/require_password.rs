use std::sync::Arc;

use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_session::RequestSessionExt;
use illuminate_support::{Carbon, Result, ValueExt, json};

use super::{MiddlewareFactory, route_url, within_request};
use crate::exceptions::redirect_guest;
use crate::facade::manager;

/// The default password confirmation timeout: three hours.
const DEFAULT_TIMEOUT: i64 = 10_800;

/// The `password.confirm` middleware: ask users to confirm their password
/// before sensitive actions.
///
/// The time of the last confirmation is read from the session
/// (`auth.password_confirmed_at`, set by `session.password_confirmed()`).
/// When it's older than `auth.password_timeout` seconds, JSON requests get
/// a `423` and everyone else is redirected to the `password.confirm` route
/// (`/confirm-password`). `password.confirm:route.name,300` customizes the
/// route and the timeout.
#[derive(Clone, Debug, Default)]
pub struct RequirePassword {
    redirect_to_route: Option<String>,
    timeout: Option<i64>,
}

impl RequirePassword {
    /// Require a recently confirmed password, with the configured timeout.
    pub fn new() -> Self {
        Self::default()
    }

    /// Customize the redirect route and the timeout (in seconds).
    pub fn using(redirect_to_route: Option<&str>, timeout: Option<i64>) -> Self {
        Self {
            redirect_to_route: redirect_to_route.map(str::to_string),
            timeout,
        }
    }

    /// Build the middleware from route parameters (`password.confirm:route,seconds`).
    pub fn from_parameters(parameters: &[String]) -> Self {
        let parameter = |index: usize| {
            parameters
                .get(index)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        Self {
            redirect_to_route: parameter(0),
            timeout: parameter(1).and_then(|seconds| seconds.parse().ok()),
        }
    }

    /// The factory registered for the `password.confirm` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|parameters| Arc::new(Self::from_parameters(parameters)) as Arc<dyn Middleware>)
    }

    /// The number of seconds a confirmation lasts.
    pub fn timeout(&self) -> i64 {
        self.timeout.unwrap_or_else(|| {
            manager()
                .config()
                .get("auth.password_timeout")
                .to_i64_lossy()
                .filter(|seconds| *seconds > 0)
                .unwrap_or(DEFAULT_TIMEOUT)
        })
    }

    /// Determine if the user must confirm their password.
    pub fn should_confirm_password(&self, request: &Request) -> bool {
        let now = Carbon::now().timestamp();
        let confirmed_at = request
            .try_session()
            .and_then(|session| session.get("auth.password_confirmed_at").to_i64_lossy())
            .unwrap_or(0);
        now - confirmed_at > self.timeout()
    }
}

#[async_trait]
impl Middleware for RequirePassword {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            if !self.should_confirm_password(&request) {
                return Ok(next.run(request.clone()).await);
            }
            if request.expects_json() {
                return Ok(Response::json(
                    &json!({ "message": "Password confirmation required." }),
                )
                .with_status(423));
            }
            let to = route_url(
                self.redirect_to_route.as_deref(),
                "password.confirm",
                "/confirm-password",
            );
            Ok(redirect_guest(&request, &to))
        })
        .await
    }
}
