//! The token a request was authenticated with: a [`PersonalAccessToken`],
//! or the [`TransientToken`] of a first-party SPA.
//!
//! Laravel attaches the token to the user instance (`withAccessToken`). Rust
//! hands out copies of the user (`request.user::<User>()`), so Sanctum
//! remembers the token *for the user* on the request being handled — every
//! copy of the user sees it.

use std::sync::{Arc, RwLock};

use illuminate_auth::AuthUser;
use illuminate_database::eloquent::Model;
use illuminate_http::{Request, current_request};
use illuminate_support::Result;
use serde::{Serialize, Serializer};

use crate::facade::state;
use crate::personal_access_token::PersonalAccessToken;
use crate::support::key_string;

/// Something that may be granted abilities (Sanctum's `HasAbilities`
/// contract): personal access tokens, transient tokens, and the
/// [`AccessToken`] wrapping either.
pub trait HasAbilities {
    /// Determine if the token has a given ability.
    fn can(&self, ability: &str) -> bool;

    /// Determine if the token is missing a given ability.
    fn cant(&self, ability: &str) -> bool {
        !self.can(ability)
    }
}

/// The token of a request authenticated by your first-party SPA's session:
/// it can do everything.
///
/// ```
/// use laravel_sanctum::TransientToken;
///
/// assert!(TransientToken.can("server:update"));
/// assert!(!TransientToken.cant("server:update"));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TransientToken;

impl TransientToken {
    /// Create a transient token.
    pub fn new() -> Self {
        Self
    }

    /// Determine if the token has a given ability (always).
    pub fn can(&self, _ability: &str) -> bool {
        true
    }

    /// Determine if the token is missing a given ability (never).
    pub fn cant(&self, _ability: &str) -> bool {
        false
    }
}

impl HasAbilities for TransientToken {
    fn can(&self, ability: &str) -> bool {
        TransientToken::can(self, ability)
    }
}

/// The token that authenticated the current user: what
/// `user.current_access_token()` returns.
///
/// ```
/// use laravel_sanctum::{AccessToken, PersonalAccessToken, TransientToken};
///
/// let token = AccessToken::from(PersonalAccessToken {
///     abilities: vec!["server:update".into()],
///     ..Default::default()
/// });
/// assert!(token.can("server:update"));
/// assert!(token.cant("server:delete"));
/// assert!(!token.is_transient());
///
/// let spa = AccessToken::from(TransientToken);
/// assert!(spa.can("server:delete"));
/// assert!(spa.as_personal().is_none());
/// ```
#[derive(Clone, Debug)]
pub enum AccessToken {
    /// An API token from the `personal_access_tokens` table.
    Personal(Box<PersonalAccessToken>),
    /// The token of a session-authenticated (first-party SPA) request.
    Transient(TransientToken),
}

impl AccessToken {
    /// Determine if the token has a given ability.
    pub fn can(&self, ability: &str) -> bool {
        match self {
            Self::Personal(token) => token.can(ability),
            Self::Transient(token) => token.can(ability),
        }
    }

    /// Determine if the token is missing a given ability.
    pub fn cant(&self, ability: &str) -> bool {
        !self.can(ability)
    }

    /// Determine if this is the transient token of a first-party SPA request.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }

    /// The personal access token, unless the request came from the SPA.
    pub fn as_personal(&self) -> Option<&PersonalAccessToken> {
        match self {
            Self::Personal(token) => Some(token),
            Self::Transient(_) => None,
        }
    }

    /// Take the personal access token out of the wrapper.
    pub fn into_personal(self) -> Option<PersonalAccessToken> {
        match self {
            Self::Personal(token) => Some(*token),
            Self::Transient(_) => None,
        }
    }

    /// Revoke the token — `$request->user()->currentAccessToken()->delete()`.
    /// Transient tokens have nothing to delete, so this returns `false` for
    /// them (log the user out of their session instead).
    pub async fn delete(&self) -> Result<bool> {
        match self {
            Self::Personal(token) => token.as_ref().clone().delete().await,
            Self::Transient(_) => Ok(false),
        }
    }
}

impl HasAbilities for AccessToken {
    fn can(&self, ability: &str) -> bool {
        AccessToken::can(self, ability)
    }
}

impl From<PersonalAccessToken> for AccessToken {
    fn from(token: PersonalAccessToken) -> Self {
        Self::Personal(Box::new(token))
    }
}

impl From<TransientToken> for AccessToken {
    fn from(token: TransientToken) -> Self {
        Self::Transient(token)
    }
}

impl Serialize for AccessToken {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Personal(token) => token.serialize(serializer),
            Self::Transient(token) => token.serialize(serializer),
        }
    }
}

// ----------------------------------------------------------------------
// Remembering which token authenticated which user
// ----------------------------------------------------------------------

/// The user an access token was attached to: their type and key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TokenOwner {
    type_name: &'static str,
    key: String,
}

impl TokenOwner {
    /// The owner for a model.
    pub(crate) fn of<M: Model>(model: &M) -> Self {
        Self {
            type_name: std::any::type_name::<M>(),
            key: key_string(&model.get_key()),
        }
    }

    /// The owner for an authenticated user of any type.
    pub(crate) fn of_user(user: &AuthUser) -> Self {
        Self {
            type_name: user.type_name(),
            key: key_string(&user.id()),
        }
    }
}

/// Access tokens keyed by the user they belong to.
#[derive(Default)]
pub(crate) struct TokenBindings {
    tokens: RwLock<Vec<(TokenOwner, AccessToken)>>,
}

impl TokenBindings {
    /// The token attached to the owner.
    pub(crate) fn get(&self, owner: &TokenOwner) -> Option<AccessToken> {
        self.tokens
            .read()
            .unwrap()
            .iter()
            .find(|(bound, _)| bound == owner)
            .map(|(_, token)| token.clone())
    }

    /// Attach a token to the owner (replacing any previous one).
    pub(crate) fn set(&self, owner: TokenOwner, token: AccessToken) {
        let mut tokens = self.tokens.write().unwrap();
        tokens.retain(|(bound, _)| *bound != owner);
        tokens.push((owner, token));
    }

    /// Forget every token.
    pub(crate) fn clear(&self) {
        self.tokens.write().unwrap().clear();
    }

    /// The bindings attached to a request (attaching fresh ones if needed).
    pub(crate) fn for_request(request: &Request) -> Arc<TokenBindings> {
        if let Some(bindings) = request.extension::<TokenBindings>() {
            return bindings;
        }
        let bindings = Arc::new(TokenBindings::default());
        request.set_extension(bindings.clone());
        bindings
    }
}

/// Attach a token to its owner for the current request (or, outside of a
/// request, for the application).
pub(crate) fn attach(owner: TokenOwner, token: AccessToken) {
    match current_request() {
        Some(request) => TokenBindings::for_request(&request).set(owner, token),
        None => state().fallback.set(owner, token),
    }
}

/// The token attached to the owner: on the current request (or the
/// application, outside of one), then by `Sanctum::acting_as`.
pub(crate) fn token_for(owner: &TokenOwner) -> Option<AccessToken> {
    let state = state();
    let attached = match current_request() {
        Some(request) => request
            .extension::<TokenBindings>()
            .and_then(|bindings| bindings.get(owner)),
        None => state.fallback.get(owner),
    };
    attached.or_else(|| state.acting_as.get(owner))
}
