use std::sync::Arc;

use illuminate_auth::{AuthenticationException, Guard, RequestAuthExt, SessionGuard};
use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_session::RequestSessionExt;
use illuminate_support::{Result, ValueExt};

use crate::config;
use crate::facade::Sanctum;
use crate::http::{request_user, within_request};
use crate::support::hash_equals;

/// Logs your SPA's users out when their password changed on another device.
///
/// For each session guard in `sanctum.guard`, the hash of the user's
/// password is kept in the session (`password_hash_{guard}`); when it no
/// longer matches, the user is logged out of those guards, the session is
/// flushed, and an `AuthenticationException` is thrown. It runs for
/// stateful requests (see
/// [`EnsureFrontendRequestsAreStateful`](super::EnsureFrontendRequestsAreStateful)).
#[derive(Clone, Copy, Debug, Default)]
pub struct AuthenticateSession;

impl AuthenticateSession {
    /// Create the middleware.
    pub fn new() -> Self {
        Self
    }

    /// The hash of a password, as stored in the session by the guard.
    fn password_hash(guard: &dyn Guard, password: &str) -> String {
        match guard.downcast_ref::<SessionGuard>() {
            Some(guard) => guard.hash_password_for_cookie(password),
            None => password.to_string(),
        }
    }

    /// The session guards Sanctum authenticates with, by name.
    fn session_guards() -> Result<Vec<(String, Arc<dyn Guard>)>> {
        let manager = illuminate_auth::manager();
        let mut guards = Vec::new();
        for name in config::guards() {
            let guard = manager.guard(Some(&name))?;
            if guard.downcast_ref::<SessionGuard>().is_some() {
                guards.push((name, guard));
            }
        }
        Ok(guards)
    }
}

#[async_trait]
impl Middleware for AuthenticateSession {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        within_request(&request, async {
            let Some(session) = request.try_session() else {
                return Ok(next.run(request.clone()).await);
            };
            let Some(user) = request_user(&request).await? else {
                return Ok(next.run(request.clone()).await);
            };

            let guards = Self::session_guards()?;
            let password = user.auth_password();
            let should_logout: Vec<&(String, Arc<dyn Guard>)> = guards
                .iter()
                .filter(|(name, guard)| {
                    let key = format!("password_hash_{name}");
                    session.has(key.as_str())
                        && !hash_equals(
                            &session.get(&key).to_string_lossy(),
                            &Self::password_hash(guard.as_ref(), &password),
                        )
                })
                .collect();

            if !should_logout.is_empty() {
                for (_, guard) in &should_logout {
                    guard.logout_current_device().await?;
                }
                session.flush();
                let mut names: Vec<String> =
                    should_logout.iter().map(|(name, _)| name.clone()).collect();
                names.push(Sanctum::GUARD.to_string());
                return Err(AuthenticationException::new(names, None).into());
            }

            let response = next.run(request.clone()).await;

            if let Some((name, guard)) = guards.iter().find(|(_, guard)| guard.has_user()) {
                let user = request.auth_user().unwrap_or(user);
                session.put(
                    &format!("password_hash_{name}"),
                    Self::password_hash(guard.as_ref(), &user.auth_password()),
                );
            }
            Ok(response)
        })
        .await
    }
}
