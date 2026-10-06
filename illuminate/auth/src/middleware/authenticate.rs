use std::sync::Arc;

use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_support::Result;

use super::{MiddlewareFactory, filled, within_request};
use crate::exceptions::AuthenticationException;
use crate::facade::manager;

/// The `auth` middleware: only authenticated users may pass.
///
/// With no parameters the default guard is checked; `auth:admin,api` checks
/// each guard in turn and uses the first one that authenticates the user for
/// the rest of the request. Guests get an [`AuthenticationException`]:
/// a `401` for JSON requests, a redirect to the login page otherwise.
///
/// ```
/// use illuminate_auth::middleware::Authenticate;
///
/// let middleware = Authenticate::from_parameters(&["admin".into(), "api".into()]);
/// assert_eq!(middleware.guards(), ["admin", "api"]);
/// ```
#[derive(Clone, Debug, Default)]
pub struct Authenticate {
    guards: Vec<String>,
}

impl Authenticate {
    /// Authenticate with the default guard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Authenticate with any of the given guards.
    pub fn using(guards: &[&str]) -> Self {
        Self {
            guards: guards.iter().map(|guard| guard.to_string()).collect(),
        }
    }

    /// Build the middleware from route parameters (`auth:admin` → `["admin"]`).
    pub fn from_parameters(parameters: &[String]) -> Self {
        Self {
            guards: filled(parameters),
        }
    }

    /// The factory registered for the `auth` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|parameters| Arc::new(Self::from_parameters(parameters)) as Arc<dyn Middleware>)
    }

    /// The guards the middleware checks.
    pub fn guards(&self) -> &[String] {
        &self.guards
    }

    /// Specify where unauthenticated users should be redirected (return
    /// `None` to respond with a plain `401`). Defaults to the `login` route,
    /// or `/login`.
    pub fn redirect_using(callback: impl Fn(&Request) -> Option<String> + Send + Sync + 'static) {
        manager().hooks().set_guest_redirect(callback);
    }

    /// Determine if the user is logged in to any of the given guards.
    pub async fn authenticate(&self, request: &Request) -> Result<()> {
        let manager = manager();
        let guards: Vec<Option<&str>> = if self.guards.is_empty() {
            vec![None]
        } else {
            self.guards
                .iter()
                .map(|guard| Some(guard.as_str()))
                .collect()
        };

        for guard in &guards {
            if manager.guard(*guard)?.try_user().await?.is_some() {
                manager.should_use(*guard);
                return Ok(());
            }
        }

        let names = guards
            .iter()
            .map(|guard| {
                guard
                    .map(str::to_string)
                    .unwrap_or_else(|| manager.get_default_driver())
            })
            .collect();
        let redirect_to = if request.expects_json() {
            None
        } else {
            manager.hooks().guest_redirect(request)
        };
        Err(AuthenticationException::new(names, redirect_to).into())
    }
}

#[async_trait]
impl Middleware for Authenticate {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            self.authenticate(&request).await?;
            Ok(next.run(request.clone()).await)
        })
        .await
    }
}
