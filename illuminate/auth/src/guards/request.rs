//! Closure-based request guards (`Auth::via_request`).

use std::any::Any;
use std::future::Future;
use std::sync::Arc;

use illuminate_http::{BoxFuture, Request, async_trait};
use illuminate_support::{Result, Value};

use super::{Guard, Known, Shared};
use crate::providers::UserProvider;
use crate::user::{AuthUser, Authenticatable};

/// The callback behind a [`RequestGuard`]: resolves the user from the
/// incoming request.
pub type RequestGuardCallback =
    Arc<dyn Fn(Request) -> BoxFuture<'static, Result<Option<AuthUser>>> + Send + Sync>;

/// What a request guard callback may return: an optional user (of any
/// authenticatable type, or already erased), optionally wrapped in a `Result`.
pub trait IntoUserResult: Send {
    /// Convert into the guard's result.
    fn into_user_result(self) -> Result<Option<AuthUser>>;
}

impl<U: Authenticatable> IntoUserResult for Option<U> {
    fn into_user_result(self) -> Result<Option<AuthUser>> {
        Ok(self.map(AuthUser::new))
    }
}

impl IntoUserResult for Option<AuthUser> {
    fn into_user_result(self) -> Result<Option<AuthUser>> {
        Ok(self)
    }
}

impl<U: Authenticatable> IntoUserResult for Result<Option<U>> {
    fn into_user_result(self) -> Result<Option<AuthUser>> {
        self.map(|user| user.map(AuthUser::new))
    }
}

impl IntoUserResult for Result<Option<AuthUser>> {
    fn into_user_result(self) -> Result<Option<AuthUser>> {
        self
    }
}

/// Wrap a request guard closure into a [`RequestGuardCallback`].
pub(crate) fn request_callback<F, Fut, R>(callback: F) -> RequestGuardCallback
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoUserResult,
{
    Arc::new(move |request| {
        let future = callback(request);
        Box::pin(async move { future.await.into_user_result() })
    })
}

/// A guard that authenticates requests with a closure — the simplest way
/// to build a custom, request-based authentication system.
///
/// ```ignore
/// Auth::via_request("custom-token", |request: Request| async move {
///     User::where_("token", request.string("token")).first().await
/// });
/// ```
pub struct RequestGuard {
    name: String,
    callback: RequestGuardCallback,
    provider: Option<Arc<dyn UserProvider>>,
    shared: Arc<Shared>,
}

impl RequestGuard {
    pub(crate) fn with_shared(
        name: String,
        callback: RequestGuardCallback,
        provider: Option<Arc<dyn UserProvider>>,
        shared: Arc<Shared>,
    ) -> Self {
        Self {
            name,
            callback,
            provider,
            shared,
        }
    }

    /// Determine if the given request would be authenticated by this guard.
    pub async fn validate_request(&self, request: Request) -> Result<bool> {
        Ok((self.callback)(request).await?.is_some())
    }
}

#[async_trait]
impl Guard for RequestGuard {
    fn name(&self) -> &str {
        &self.name
    }

    async fn try_user(&self) -> Result<Option<AuthUser>> {
        let context = self.shared.context();
        match self.shared.known_user(&context, &self.name) {
            Known::User(user) => return Ok(Some(user)),
            Known::Guest => return Ok(None),
            Known::Unknown => {}
        }
        let user = match &context.request {
            Some(request) => (self.callback)(request.clone()).await?,
            None => None,
        };
        self.shared
            .remember_user(&context, &self.name, user.clone());
        Ok(user)
    }

    /// Request guards validate the current request: the credentials are
    /// not used.
    async fn validate(&self, _credentials: &Value) -> Result<bool> {
        match self.shared.context().request {
            Some(request) => self.validate_request(request).await,
            None => Ok(false),
        }
    }

    fn set_user(&self, user: AuthUser) {
        self.shared
            .remember_user(&self.shared.context(), &self.name, Some(user));
    }

    fn forget_user(&self) {
        self.shared.forget_user(&self.shared.context(), &self.name);
    }

    fn provider(&self) -> Option<Arc<dyn UserProvider>> {
        self.provider.clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
