use std::sync::Arc;

use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_support::{Result, json};

use super::{MiddlewareFactory, within_request};
use crate::facade::manager;

/// The `auth.basic` middleware: HTTP Basic authentication.
///
/// The browser's username is matched against the `email` column by
/// default (`auth.basic:web,username` changes the guard and the column).
/// Invalid credentials get a `401` with a `WWW-Authenticate: Basic` header,
/// which makes the browser prompt for credentials.
///
/// ```
/// use illuminate_auth::middleware::AuthenticateWithBasicAuth;
///
/// let middleware = AuthenticateWithBasicAuth::from_parameters(&["web".into(), "username".into()]);
/// assert_eq!(middleware.guard(), Some("web"));
/// assert_eq!(middleware.field(), "username");
/// ```
#[derive(Clone, Debug, Default)]
pub struct AuthenticateWithBasicAuth {
    guard: Option<String>,
    field: Option<String>,
}

impl AuthenticateWithBasicAuth {
    /// Authenticate with the default guard, by e-mail address.
    pub fn new() -> Self {
        Self::default()
    }

    /// Authenticate with the given guard and username column.
    pub fn using(guard: Option<&str>, field: Option<&str>) -> Self {
        Self {
            guard: guard.map(str::to_string),
            field: field.map(str::to_string),
        }
    }

    /// Build the middleware from route parameters (`auth.basic:guard,field`).
    pub fn from_parameters(parameters: &[String]) -> Self {
        let parameter = |index: usize| {
            parameters
                .get(index)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        Self {
            guard: parameter(0),
            field: parameter(1),
        }
    }

    /// The factory registered for the `auth.basic` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|parameters| Arc::new(Self::from_parameters(parameters)) as Arc<dyn Middleware>)
    }

    /// The guard to authenticate with (`None` for the default).
    pub fn guard(&self) -> Option<&str> {
        self.guard.as_deref()
    }

    /// The column the username is matched against.
    pub fn field(&self) -> &str {
        self.field.as_deref().unwrap_or("email")
    }
}

#[async_trait]
impl Middleware for AuthenticateWithBasicAuth {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            manager()
                .guard(self.guard())?
                .basic(self.field(), &json!({}))
                .await?;
            Ok(next.run(request.clone()).await)
        })
        .await
    }
}
