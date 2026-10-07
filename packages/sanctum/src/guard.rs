//! The `sanctum` guard: session authentication for your SPA, API tokens for
//! everyone else.

use std::any::Any;
use std::sync::Arc;

use illuminate_auth::{AuthState, AuthUser, Guard, UserProvider};
use illuminate_config::Repository;
use illuminate_container::try_app;
use illuminate_database::eloquent::Model;
use illuminate_http::{Request, async_trait, current_request};
use illuminate_support::{Carbon, Result, Value, ValueExt};

use crate::access_token::{AccessToken, TokenBindings, TokenOwner, TransientToken};
use crate::config;
use crate::events::{TokenAuthenticated, dispatch};
use crate::facade::state;
use crate::personal_access_token::{PersonalAccessToken, ResolvedTokenable};
use crate::support::{class_basename, ctype_digit, php_empty};

/// The guard behind `auth:sanctum`.
///
/// For each request it first asks the stateful guards (`sanctum.guard`,
/// usually `web`) for a session-authenticated user — your SPA — who gets a
/// [`TransientToken`] that can do everything. Otherwise it reads the API
/// token from the `Authorization: Bearer` header, checks that it hasn't
/// expired, records when it was used, and authenticates its owner with the
/// token attached (see [`HasApiTokens::current_access_token`]).
///
/// The [`SanctumServiceProvider`](crate::SanctumServiceProvider) registers
/// the `sanctum` driver and guard, so `auth:sanctum` works out of the box.
///
/// [`HasApiTokens::current_access_token`]: crate::HasApiTokens::current_access_token
#[derive(Clone, Debug)]
pub struct SanctumGuard {
    name: String,
    provider: Option<String>,
}

impl SanctumGuard {
    /// Create the guard from its configuration (`auth.guards.<name>`): the
    /// optional `provider` restricts tokens to that provider's model.
    pub fn new(name: impl Into<String>, config: &Value) -> Self {
        let provider = match config.get("provider") {
            None | Some(Value::Null) => None,
            Some(provider) => Some(provider.to_string_lossy()).filter(|name| !name.is_empty()),
        };
        Self {
            name: name.into(),
            provider,
        }
    }

    /// The user provider the guard's tokens must belong to, if any.
    pub fn provider_name(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    /// Authenticate the request: Sanctum's `Guard::__invoke`.
    pub async fn authenticate_request(&self, request: &Request) -> Result<Option<AuthUser>> {
        let manager = illuminate_auth::manager();
        for guard in config::guards() {
            if let Some(user) = manager.guard(Some(&guard))?.try_user().await? {
                TokenBindings::for_request(request).set(
                    TokenOwner::of_user(&user),
                    AccessToken::Transient(TransientToken),
                );
                return Ok(Some(user));
            }
        }

        let Some(token) = Self::token_from_request(request) else {
            return Ok(None);
        };
        let Some(mut access_token) = PersonalAccessToken::find_token(&token).await? else {
            return Ok(None);
        };
        let tokenable = access_token.resolve_tokenable().await?;
        if !self.is_valid_access_token(&access_token, tokenable.as_ref()) {
            return Ok(None);
        }
        let Some(tokenable) = tokenable else {
            return Ok(None);
        };

        dispatch(TokenAuthenticated::new(access_token.clone())).await?;

        access_token.last_used_at = Some(Carbon::now());
        access_token.save().await?;

        TokenBindings::for_request(request).set(
            TokenOwner::of_user(&tokenable.user),
            AccessToken::from(access_token),
        );
        Ok(Some(tokenable.user))
    }

    /// The access token sent with the request: the
    /// [`Sanctum::get_access_token_from_request_using`](crate::Sanctum::get_access_token_from_request_using)
    /// callback's, or the bearer token (when it is well-formed).
    pub fn token_from_request(request: &Request) -> Option<String> {
        if let Some(retriever) = state().token_retriever() {
            return retriever(request).filter(|token| !php_empty(token));
        }
        request
            .bearer_token()
            .filter(|token| Self::is_valid_bearer_token(token))
    }

    /// Determine if a bearer token is well-formed: `{id}|{token}` tokens
    /// need a numeric ID (integer keys) and a non-empty token.
    pub fn is_valid_bearer_token(token: &str) -> bool {
        if let Some((id, token)) = token.split_once('|')
            && PersonalAccessToken::has_integer_keys()
        {
            return ctype_digit(id) && !php_empty(token);
        }
        !php_empty(token)
    }

    /// Determine if the token may authenticate: it is within the
    /// `sanctum.expiration` window, its `expires_at` hasn't passed, and it
    /// belongs to the guard's provider — or whatever the
    /// [`Sanctum::authenticate_access_tokens_using`](crate::Sanctum::authenticate_access_tokens_using)
    /// callback decides.
    fn is_valid_access_token(
        &self,
        token: &PersonalAccessToken,
        tokenable: Option<&ResolvedTokenable>,
    ) -> bool {
        let within_expiration = match config::expiration() {
            Some(minutes) => {
                let cutoff = Carbon::now().sub_seconds((minutes * 60.0).round() as i64);
                token
                    .created_at
                    .is_some_and(|created_at| created_at.gt(&cutoff))
            }
            None => true,
        };
        let is_valid =
            within_expiration && !token.is_expired() && self.has_valid_provider(tokenable);

        match state().token_authenticator() {
            Some(callback) => callback(token, is_valid),
            None => is_valid,
        }
    }

    /// Determine if the token's owner is the guard provider's model.
    fn has_valid_provider(&self, tokenable: Option<&ResolvedTokenable>) -> bool {
        let Some(provider) = &self.provider else {
            return true;
        };
        let Some(tokenable) = tokenable else {
            return false;
        };
        let model = try_app::<Repository>()
            .map(|config| config.get(&format!("auth.providers.{provider}.model")))
            .filter(|model| !model.is_null())
            .map(|model| model.to_string_lossy());
        model.is_some_and(|model| class_basename(&model) == class_basename(&tokenable.class_name))
    }

    /// Record the user the guard resolved for the request, keeping the
    /// `_auth_id` request attribute in sync.
    fn remember(&self, state: &AuthState, request: Option<&Request>, user: Option<AuthUser>) {
        state.update(&self.name, |guard| {
            guard.resolved = true;
            if user.is_some() {
                guard.logged_out = false;
            }
            guard.user = user;
        });
        if let Some(request) = request {
            let default = state
                .default_guard()
                .unwrap_or_else(|| illuminate_auth::manager().get_default_driver());
            let id = state
                .user(&default)
                .map(|user| user.id())
                .unwrap_or(Value::Null);
            request.set_attribute("_auth_id", id);
        }
    }
}

#[async_trait]
impl Guard for SanctumGuard {
    fn name(&self) -> &str {
        &self.name
    }

    async fn try_user(&self) -> Result<Option<AuthUser>> {
        let request = current_request();
        let state = match &request {
            Some(request) => AuthState::for_request(request),
            None => AuthState::current(),
        };
        let known = state.guard(&self.name);
        if known.logged_out {
            return Ok(None);
        }
        if known.user.is_some() {
            return Ok(known.user);
        }
        if let Some(user) = illuminate_auth::manager().resolved_user(&state, Some(&self.name)) {
            self.remember(&state, request.as_ref(), Some(user.clone()));
            return Ok(Some(user));
        }
        if known.resolved {
            return Ok(None);
        }
        let user = match &request {
            Some(request) => self.authenticate_request(request).await?,
            None => None,
        };
        self.remember(&state, request.as_ref(), user.clone());
        Ok(user)
    }

    /// Like Laravel's request guards, the current request is validated:
    /// the credentials aren't used.
    async fn validate(&self, _credentials: &Value) -> Result<bool> {
        match current_request() {
            Some(request) => Ok(self.authenticate_request(&request).await?.is_some()),
            None => Ok(false),
        }
    }

    fn set_user(&self, user: AuthUser) {
        let request = current_request();
        let state = match &request {
            Some(request) => AuthState::for_request(request),
            None => AuthState::current(),
        };
        self.remember(&state, request.as_ref(), Some(user));
    }

    fn forget_user(&self) {
        let request = current_request();
        let state = match &request {
            Some(request) => AuthState::for_request(request),
            None => AuthState::current(),
        };
        state.update(&self.name, |guard| {
            guard.user = None;
            guard.resolved = false;
        });
        if let Some(request) = &request {
            request.set_attribute("_auth_id", Value::Null);
        }
    }

    fn provider(&self) -> Option<Arc<dyn UserProvider>> {
        illuminate_auth::manager()
            .create_user_provider(self.provider.as_deref())
            .ok()
            .flatten()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[test]
    fn the_guard_reads_its_provider_from_configuration() {
        let guard = SanctumGuard::new("sanctum", &json!({"driver": "sanctum", "provider": null}));
        assert_eq!(guard.name(), "sanctum");
        assert_eq!(guard.provider_name(), None);

        let guard = SanctumGuard::new("api", &json!({"driver": "sanctum", "provider": "users"}));
        assert_eq!(guard.provider_name(), Some("users"));
    }

    #[test]
    fn bearer_tokens_must_be_well_formed() {
        assert!(SanctumGuard::is_valid_bearer_token("1|abc"));
        assert!(SanctumGuard::is_valid_bearer_token("abc"));
        assert!(!SanctumGuard::is_valid_bearer_token("abc|def"));
        assert!(!SanctumGuard::is_valid_bearer_token("1|"));
        assert!(!SanctumGuard::is_valid_bearer_token("|abc"));
        assert!(!SanctumGuard::is_valid_bearer_token("1|0"));
        assert!(!SanctumGuard::is_valid_bearer_token(""));
        assert!(!SanctumGuard::is_valid_bearer_token("0"));
    }
}
