use std::sync::Arc;

use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_session::RequestSessionExt;
use illuminate_support::{Result, ValueExt};

use super::{MiddlewareFactory, within_request};
use crate::exceptions::AuthenticationException;
use crate::facade::manager;
use crate::guards::{Guard, SessionGuard};
use crate::support::hash_equals;

/// The `auth.session` middleware: log users out when their password
/// changed on another device (see `Auth::logout_other_devices`).
///
/// The (signed) password hash is kept in the session; when it no longer
/// matches the user's current password, the session is flushed and an
/// `AuthenticationException` is thrown.
#[derive(Clone, Copy, Debug, Default)]
pub struct AuthenticateSession;

impl AuthenticateSession {
    /// Create the middleware.
    pub fn new() -> Self {
        Self
    }

    /// The factory registered for the `auth.session` alias.
    pub fn factory() -> MiddlewareFactory {
        Arc::new(|_| Arc::new(Self) as Arc<dyn Middleware>)
    }

    fn hash(guard: &dyn Guard, password_hash: &str) -> String {
        match guard.downcast_ref::<SessionGuard>() {
            Some(guard) => guard.hash_password_for_cookie(password_hash),
            None => password_hash.to_string(),
        }
    }

    fn validate_password_hash(guard: &dyn Guard, password_hash: &str, stored: &str) -> bool {
        hash_equals(&Self::hash(guard, password_hash), stored) || hash_equals(password_hash, stored)
    }

    fn store_password_hash(request: &Request, guard: &dyn Guard, key: &str) {
        let (Some(session), Some(user)) = (request.try_session(), guard_user(request, guard))
        else {
            return;
        };
        session.put(key, Self::hash(guard, &user.auth_password()));
    }

    async fn logout(request: &Request, guard: &dyn Guard) -> Result<Response> {
        guard.logout_current_device().await?;
        if let Some(session) = request.try_session() {
            session.flush();
        }
        let manager = manager();
        Err(AuthenticationException::new(
            vec![manager.get_default_driver()],
            manager.hooks().guest_redirect(request),
        )
        .into())
    }
}

/// The user the guard already resolved for the request.
fn guard_user(request: &Request, guard: &dyn Guard) -> Option<crate::AuthUser> {
    crate::request::RequestAuthExt::auth_state(request).user(guard.name())
}

#[async_trait]
impl Middleware for AuthenticateSession {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            let Some(session) = request.try_session() else {
                return Ok(next.run(request.clone()).await);
            };
            let manager = manager();
            let guard = manager.guard(None)?;
            let user = match guard.try_user().await? {
                Some(user) if !user.auth_password().is_empty() => user,
                _ => return Ok(next.run(request.clone()).await),
            };
            let password = user.auth_password();
            let key = format!("password_hash_{}", manager.get_default_driver());

            if guard.via_remember() {
                let from_cookie = guard
                    .downcast_ref::<SessionGuard>()
                    .and_then(|session_guard| request.cookie(&session_guard.get_recaller_name()))
                    .and_then(|cookie| cookie.split('|').nth(2).map(str::to_string));
                let valid = from_cookie.is_some_and(|hash| {
                    Self::validate_password_hash(guard.as_ref(), &password, &hash)
                });
                if !valid {
                    return Self::logout(&request, guard.as_ref()).await;
                }
            }

            if !session.has(key.as_str()) {
                Self::store_password_hash(&request, guard.as_ref(), &key);
            }

            let stored = session.get(&key).to_string_lossy();
            if !Self::validate_password_hash(guard.as_ref(), &password, &stored) {
                return Self::logout(&request, guard.as_ref()).await;
            }

            let response = next.run(request.clone()).await;
            if guard_user(&request, guard.as_ref()).is_some() {
                Self::store_password_hash(&request, guard.as_ref(), &key);
            }
            Ok(response)
        })
        .await
    }
}
