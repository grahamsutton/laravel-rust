use std::sync::Arc;

use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_support::Result;

use super::{MiddlewareFactory, filled, within_request};
use crate::exceptions::absolute_url;
use crate::facade::manager;

/// The `guest` middleware: authenticated users are redirected away (from
/// the login and registration pages, typically).
///
/// Users are sent to the `dashboard` or `home` route when the application
/// registered a route resolver (see `Auth::resolve_routes_using`), and to
/// `/` otherwise — or wherever
/// [`redirect_using`](RedirectIfAuthenticated::redirect_using) says.
#[derive(Clone, Debug, Default)]
pub struct RedirectIfAuthenticated {
    guards: Vec<String>,
}

impl RedirectIfAuthenticated {
    /// Check the default guard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check each of the given guards.
    pub fn using(guards: &[&str]) -> Self {
        Self {
            guards: guards.iter().map(|guard| guard.to_string()).collect(),
        }
    }

    /// Build the middleware from route parameters (`guest:admin`).
    pub fn from_parameters(parameters: &[String]) -> Self {
        Self {
            guards: filled(parameters),
        }
    }

    /// The factory registered for the `guest` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|parameters| Arc::new(Self::from_parameters(parameters)) as Arc<dyn Middleware>)
    }

    /// Specify where authenticated users should be redirected.
    pub fn redirect_using(callback: impl Fn(&Request) -> String + Send + Sync + 'static) {
        manager().hooks().set_user_redirect(callback);
    }

    /// The path authenticated users are redirected to.
    pub fn redirect_to(request: &Request) -> String {
        manager().hooks().user_redirect(request)
    }
}

#[async_trait]
impl Middleware for RedirectIfAuthenticated {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            let manager = manager();
            let guards: Vec<Option<&str>> = if self.guards.is_empty() {
                vec![None]
            } else {
                self.guards
                    .iter()
                    .map(|guard| Some(guard.as_str()))
                    .collect()
            };
            for guard in guards {
                if manager.guard(guard)?.try_user().await?.is_some() {
                    let to = Self::redirect_to(&request);
                    return Ok(Response::redirect(absolute_url(&request, &to)));
                }
            }
            Ok(next.run(request.clone()).await)
        })
        .await
    }
}
