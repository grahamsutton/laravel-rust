//! Authenticatable users: the contract, the type-erased handle the guards
//! pass around, and the simple `GenericUser`.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use serde::{Serialize, Serializer};

use illuminate_support::{Map, Value, ValueExt, to_value};

/// The contract every user type implements so it can be authenticated.
///
/// Only two methods are required: how to identify the user, and their
/// hashed password. Everything else has Laravel's defaults: the identifier
/// is named `id`, the password `password`, and the "remember me" token
/// `remember_token`.
///
/// ```
/// use illuminate_auth::Authenticatable;
/// use illuminate_support::{json, Value};
/// use serde::Serialize;
///
/// #[derive(Clone, Serialize)]
/// struct User {
///     id: u64,
///     email: String,
///     #[serde(skip)]
///     password: String,
///     remember_token: Option<String>,
/// }
///
/// impl Authenticatable for User {
///     fn auth_identifier(&self) -> Value {
///         json!(self.id)
///     }
///
///     fn auth_password(&self) -> String {
///         self.password.clone()
///     }
///
///     fn remember_token(&self) -> Option<String> {
///         self.remember_token.clone()
///     }
///
///     fn set_remember_token(&mut self, token: &str) {
///         self.remember_token = Some(token.to_string());
///     }
/// }
///
/// let user = User { id: 1, email: "taylor@laravel.com".into(), password: "hash".into(), remember_token: None };
/// assert_eq!(user.auth_identifier_name(), "id");
/// assert_eq!(user.to_value()["email"], json!("taylor@laravel.com"));
/// ```
pub trait Authenticatable: Serialize + Clone + Send + Sync + 'static {
    /// The name of the unique identifier for the user (`id`).
    fn auth_identifier_name(&self) -> &'static str {
        "id"
    }

    /// The unique identifier for the user.
    fn auth_identifier(&self) -> Value;

    /// The name of the password attribute for the user (`password`).
    fn auth_password_name(&self) -> &'static str {
        "password"
    }

    /// The (hashed) password for the user. An empty string means the user
    /// has no password.
    fn auth_password(&self) -> String;

    /// Replace the user's hashed password — used when a password is
    /// automatically rehashed after logging in. Persisting the change is
    /// the user provider's job.
    fn set_auth_password(&mut self, _hashed: &str) {}

    /// The name of the "remember me" token attribute (`remember_token`).
    fn remember_token_name(&self) -> &'static str {
        "remember_token"
    }

    /// The token value for the "remember me" session.
    fn remember_token(&self) -> Option<String> {
        None
    }

    /// Set the token value for the "remember me" session.
    fn set_remember_token(&mut self, _token: &str) {}

    /// The user as a JSON value (what `Auth::user()` serializes to).
    fn to_value(&self) -> Value {
        to_value(self)
    }

    /// The e-mail address where password reset links are sent (Laravel's
    /// `CanResetPassword`): the `email` attribute by default.
    fn email_for_password_reset(&self) -> String {
        self.to_value()
            .get("email")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_default()
    }

    /// Users who must verify their e-mail address return themselves here.
    ///
    /// ```ignore
    /// impl MustVerifyEmail for User {}
    ///
    /// impl Authenticatable for User {
    ///     // ...
    ///     fn must_verify_email(&self) -> Option<&dyn MustVerifyEmail> {
    ///         Some(self)
    ///     }
    /// }
    /// ```
    fn must_verify_email(&self) -> Option<&dyn MustVerifyEmail> {
        None
    }
}

/// Users that must verify their e-mail address before using parts of the
/// application (the `verified` middleware).
///
/// By default a user is verified when their `email_verified_at` attribute
/// is filled. Remember to return the user from
/// [`Authenticatable::must_verify_email`] so the framework can tell.
///
/// ```
/// use illuminate_auth::{Authenticatable, AuthUser, MustVerifyEmail};
/// use illuminate_support::{json, Value};
/// use serde::Serialize;
///
/// #[derive(Clone, Serialize)]
/// struct User {
///     id: u64,
///     email: String,
///     email_verified_at: Option<String>,
/// }
///
/// impl MustVerifyEmail for User {}
///
/// impl Authenticatable for User {
///     fn auth_identifier(&self) -> Value { json!(self.id) }
///     fn auth_password(&self) -> String { String::new() }
///     fn must_verify_email(&self) -> Option<&dyn MustVerifyEmail> { Some(self) }
/// }
///
/// let user = AuthUser::new(User { id: 1, email: "taylor@laravel.com".into(), email_verified_at: None });
/// assert!(user.must_verify_email());
/// assert!(!user.has_verified_email());
/// ```
pub trait MustVerifyEmail: DynAuthenticatable {
    /// Determine if the user has verified their email address.
    fn has_verified_email(&self) -> bool {
        self.dyn_to_value()
            .get("email_verified_at")
            .is_some_and(ValueExt::is_filled)
    }

    /// The e-mail address that should be used for verification.
    fn email_for_verification(&self) -> String {
        self.dyn_to_value()
            .get("email")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_default()
    }
}

/// The object-safe face of [`Authenticatable`], implemented for every
/// authenticatable type. You never implement this yourself; it's what lets
/// guards hold users of any type behind an [`AuthUser`].
pub trait DynAuthenticatable: Any + Send + Sync {
    #[doc(hidden)]
    fn dyn_auth_identifier_name(&self) -> &'static str;
    #[doc(hidden)]
    fn dyn_auth_identifier(&self) -> Value;
    #[doc(hidden)]
    fn dyn_auth_password_name(&self) -> &'static str;
    #[doc(hidden)]
    fn dyn_auth_password(&self) -> String;
    #[doc(hidden)]
    fn dyn_remember_token_name(&self) -> &'static str;
    #[doc(hidden)]
    fn dyn_remember_token(&self) -> Option<String>;
    #[doc(hidden)]
    fn dyn_to_value(&self) -> Value;
    #[doc(hidden)]
    fn dyn_email_for_password_reset(&self) -> String;
    #[doc(hidden)]
    fn dyn_must_verify_email(&self) -> Option<&dyn MustVerifyEmail>;
    #[doc(hidden)]
    fn dyn_type_name(&self) -> &'static str;
    #[doc(hidden)]
    fn dyn_as_any(&self) -> &(dyn Any + Send + Sync);
    #[doc(hidden)]
    fn dyn_with_remember_token(&self, token: &str) -> Arc<dyn DynAuthenticatable>;
    #[doc(hidden)]
    fn dyn_with_auth_password(&self, hashed: &str) -> Arc<dyn DynAuthenticatable>;
}

impl<U: Authenticatable> DynAuthenticatable for U {
    fn dyn_auth_identifier_name(&self) -> &'static str {
        self.auth_identifier_name()
    }

    fn dyn_auth_identifier(&self) -> Value {
        self.auth_identifier()
    }

    fn dyn_auth_password_name(&self) -> &'static str {
        self.auth_password_name()
    }

    fn dyn_auth_password(&self) -> String {
        self.auth_password()
    }

    fn dyn_remember_token_name(&self) -> &'static str {
        self.remember_token_name()
    }

    fn dyn_remember_token(&self) -> Option<String> {
        self.remember_token()
    }

    fn dyn_to_value(&self) -> Value {
        self.to_value()
    }

    fn dyn_email_for_password_reset(&self) -> String {
        self.email_for_password_reset()
    }

    fn dyn_must_verify_email(&self) -> Option<&dyn MustVerifyEmail> {
        self.must_verify_email()
    }

    fn dyn_type_name(&self) -> &'static str {
        std::any::type_name::<U>()
    }

    fn dyn_as_any(&self) -> &(dyn Any + Send + Sync) {
        self
    }

    fn dyn_with_remember_token(&self, token: &str) -> Arc<dyn DynAuthenticatable> {
        let mut user = self.clone();
        user.set_remember_token(token);
        Arc::new(user)
    }

    fn dyn_with_auth_password(&self, hashed: &str) -> Arc<dyn DynAuthenticatable> {
        let mut user = self.clone();
        user.set_auth_password(hashed);
        Arc::new(user)
    }
}

/// An authenticated user of any type: what guards, gates, and the request
/// hold on to. Cloning is cheap, and the concrete user is always one
/// downcast away.
///
/// ```
/// use illuminate_auth::{AuthUser, GenericUser};
/// use illuminate_support::json;
///
/// let user = AuthUser::new(GenericUser::new(json!({"id": 1, "name": "Taylor"})));
///
/// assert_eq!(user.id(), json!(1));
/// assert!(user.is::<GenericUser>());
///
/// let generic: GenericUser = user.downcast().unwrap();
/// assert_eq!(generic.get("name"), json!("Taylor"));
/// ```
#[derive(Clone)]
pub struct AuthUser {
    inner: Arc<dyn DynAuthenticatable>,
}

impl AuthUser {
    /// Wrap an authenticatable user.
    pub fn new<U: Authenticatable>(user: U) -> Self {
        Self {
            inner: Arc::new(user),
        }
    }

    /// Wrap an already shared user.
    pub fn from_arc(user: Arc<dyn DynAuthenticatable>) -> Self {
        Self { inner: user }
    }

    /// The name of the user's unique identifier.
    pub fn auth_identifier_name(&self) -> &'static str {
        self.inner.dyn_auth_identifier_name()
    }

    /// The user's unique identifier.
    pub fn auth_identifier(&self) -> Value {
        self.inner.dyn_auth_identifier()
    }

    /// Alias of [`AuthUser::auth_identifier`].
    pub fn id(&self) -> Value {
        self.auth_identifier()
    }

    /// The name of the password attribute.
    pub fn auth_password_name(&self) -> &'static str {
        self.inner.dyn_auth_password_name()
    }

    /// The user's hashed password (empty when they have none).
    pub fn auth_password(&self) -> String {
        self.inner.dyn_auth_password()
    }

    /// The name of the "remember me" token attribute.
    pub fn remember_token_name(&self) -> &'static str {
        self.inner.dyn_remember_token_name()
    }

    /// The user's "remember me" token.
    pub fn remember_token(&self) -> Option<String> {
        self.inner
            .dyn_remember_token()
            .filter(|token| !token.is_empty())
    }

    /// A copy of the user with the given "remember me" token.
    pub fn with_remember_token(&self, token: &str) -> AuthUser {
        Self::from_arc(self.inner.dyn_with_remember_token(token))
    }

    /// A copy of the user with the given hashed password.
    pub fn with_auth_password(&self, hashed: &str) -> AuthUser {
        Self::from_arc(self.inner.dyn_with_auth_password(hashed))
    }

    /// The user as a JSON value.
    pub fn to_value(&self) -> Value {
        self.inner.dyn_to_value()
    }

    /// The e-mail address password reset links are sent to.
    pub fn email_for_password_reset(&self) -> String {
        self.inner.dyn_email_for_password_reset()
    }

    /// Determine if the user must verify their e-mail address.
    pub fn must_verify_email(&self) -> bool {
        self.inner.dyn_must_verify_email().is_some()
    }

    /// The user as a [`MustVerifyEmail`] implementation, if it is one.
    pub fn as_must_verify_email(&self) -> Option<&dyn MustVerifyEmail> {
        self.inner.dyn_must_verify_email()
    }

    /// Determine if the user has verified their e-mail address. Users that
    /// don't need to verify their address always have.
    pub fn has_verified_email(&self) -> bool {
        self.as_must_verify_email()
            .is_none_or(|user| user.has_verified_email())
    }

    /// The concrete type name of the user (`app::models::User`).
    pub fn type_name(&self) -> &'static str {
        self.inner.dyn_type_name()
    }

    /// Determine if the user is of the given type.
    pub fn is<U: 'static>(&self) -> bool {
        self.inner.dyn_as_any().is::<U>()
    }

    /// Borrow the concrete user.
    pub fn downcast_ref<U: 'static>(&self) -> Option<&U> {
        self.inner.dyn_as_any().downcast_ref::<U>()
    }

    /// A copy of the concrete user.
    pub fn downcast<U: Authenticatable>(&self) -> Option<U> {
        self.downcast_ref::<U>().cloned()
    }

    /// The concrete user as `Any`.
    pub fn as_any(&self) -> &(dyn Any + Send + Sync) {
        self.inner.dyn_as_any()
    }

    /// Determine if two handles hold the same user (same type and identifier).
    pub fn is_same(&self, other: &AuthUser) -> bool {
        self.type_name() == other.type_name()
            && crate::support::loosely_equal(&self.id(), &other.id())
    }
}

impl fmt::Debug for AuthUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthUser")
            .field("type", &self.type_name())
            .field("id", &self.id())
            .finish()
    }
}

impl Serialize for AuthUser {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(serializer)
    }
}

impl<U: Authenticatable> From<&U> for AuthUser {
    fn from(user: &U) -> Self {
        AuthUser::new(user.clone())
    }
}

impl From<&AuthUser> for AuthUser {
    fn from(user: &AuthUser) -> Self {
        user.clone()
    }
}

/// Types an [`AuthUser`] can be turned back into: the erased handle itself,
/// or any concrete [`Authenticatable`] type (by downcasting).
pub trait FromAuthUser: Sized {
    /// Convert the authenticated user, or `None` when it's of another type.
    fn from_auth_user(user: &AuthUser) -> Option<Self>;
}

impl FromAuthUser for AuthUser {
    fn from_auth_user(user: &AuthUser) -> Option<Self> {
        Some(user.clone())
    }
}

impl<U: Authenticatable> FromAuthUser for U {
    fn from_auth_user(user: &AuthUser) -> Option<Self> {
        user.downcast::<U>()
    }
}

/// Types an [`AuthUser`] can be *borrowed* as: the erased handle itself, or
/// any concrete [`Authenticatable`] type. Used by gate and policy callbacks.
pub trait AuthUserRef: 'static {
    /// Borrow the authenticated user as `Self`.
    fn from_auth_user_ref(user: &AuthUser) -> Option<&Self>;
}

impl AuthUserRef for AuthUser {
    fn from_auth_user_ref(user: &AuthUser) -> Option<&Self> {
        Some(user)
    }
}

impl<U: Authenticatable> AuthUserRef for U {
    fn from_auth_user_ref(user: &AuthUser) -> Option<&Self> {
        user.downcast_ref::<U>()
    }
}

/// A simple, attribute-backed user — handy for the array provider, request
/// guards, and tests (Laravel's `GenericUser`).
///
/// ```
/// use illuminate_auth::{Authenticatable, GenericUser};
/// use illuminate_support::json;
///
/// let mut user = GenericUser::new(json!({"id": 1, "email": "taylor@laravel.com", "password": "hash"}));
///
/// assert_eq!(user.auth_identifier(), json!(1));
/// assert_eq!(user.auth_password(), "hash");
///
/// user.set_remember_token("token");
/// assert_eq!(user.remember_token().as_deref(), Some("token"));
/// assert_eq!(user.get("email"), json!("taylor@laravel.com"));
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenericUser {
    attributes: Map<String, Value>,
}

impl GenericUser {
    /// Create a user from its attributes (a JSON object).
    pub fn new(attributes: Value) -> Self {
        Self {
            attributes: match attributes {
                Value::Object(map) => map,
                _ => Map::new(),
            },
        }
    }

    /// Get an attribute (`null` when missing).
    pub fn get(&self, key: &str) -> Value {
        self.attributes.get(key).cloned().unwrap_or(Value::Null)
    }

    /// Set an attribute.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.attributes.insert(key.into(), value.into());
    }

    /// Determine if an attribute is set (and not null).
    pub fn has(&self, key: &str) -> bool {
        self.attributes
            .get(key)
            .is_some_and(|value| !value.is_null())
    }

    /// Remove an attribute.
    pub fn forget(&mut self, key: &str) {
        self.attributes.shift_remove(key);
    }

    /// All of the user's attributes.
    pub fn attributes(&self) -> &Map<String, Value> {
        &self.attributes
    }
}

impl Serialize for GenericUser {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.attributes.serialize(serializer)
    }
}

impl Authenticatable for GenericUser {
    fn auth_identifier(&self) -> Value {
        self.get(self.auth_identifier_name())
    }

    fn auth_password(&self) -> String {
        self.get(self.auth_password_name()).to_string_lossy()
    }

    fn set_auth_password(&mut self, hashed: &str) {
        let name = self.auth_password_name();
        self.set(name, hashed);
    }

    fn remember_token(&self) -> Option<String> {
        match self.get(self.remember_token_name()) {
            Value::Null => None,
            value => Some(value.to_string_lossy()),
        }
    }

    fn set_remember_token(&mut self, token: &str) {
        let name = self.remember_token_name();
        self.set(name, token);
    }

    fn to_value(&self) -> Value {
        Value::Object(self.attributes.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    #[derive(Clone, Serialize)]
    struct Admin {
        id: u64,
        email_verified_at: Option<String>,
    }

    impl MustVerifyEmail for Admin {}

    impl Authenticatable for Admin {
        fn auth_identifier(&self) -> Value {
            json!(self.id)
        }

        fn auth_password(&self) -> String {
            String::new()
        }

        fn must_verify_email(&self) -> Option<&dyn MustVerifyEmail> {
            Some(self)
        }
    }

    #[test]
    fn users_are_erased_and_recovered() {
        let user = AuthUser::new(GenericUser::new(
            json!({"id": 7, "email": "taylor@laravel.com"}),
        ));
        assert_eq!(user.id(), json!(7));
        assert_eq!(user.auth_identifier_name(), "id");
        assert_eq!(user.auth_password_name(), "password");
        assert_eq!(user.remember_token_name(), "remember_token");
        assert_eq!(user.email_for_password_reset(), "taylor@laravel.com");
        assert!(user.downcast_ref::<Admin>().is_none());
        assert!(user.downcast::<GenericUser>().is_some());
        assert!(user.type_name().ends_with("GenericUser"));
        assert_eq!(
            serde_json::to_value(&user).unwrap()["email"],
            json!("taylor@laravel.com")
        );

        let remembered = user.with_remember_token("abc");
        assert_eq!(remembered.remember_token().as_deref(), Some("abc"));
        assert_eq!(user.remember_token(), None);
        assert!(remembered.is_same(&user));

        let rehashed = user.with_auth_password("new-hash");
        assert_eq!(rehashed.auth_password(), "new-hash");
    }

    #[test]
    fn users_may_need_to_verify_their_email() {
        let unverified = AuthUser::new(Admin {
            id: 1,
            email_verified_at: None,
        });
        assert!(unverified.must_verify_email());
        assert!(!unverified.has_verified_email());

        let verified = AuthUser::new(Admin {
            id: 1,
            email_verified_at: Some("2024-01-01".into()),
        });
        assert!(verified.has_verified_email());

        let generic = AuthUser::new(GenericUser::new(json!({"id": 1})));
        assert!(!generic.must_verify_email());
        assert!(generic.has_verified_email());
    }

    #[test]
    fn users_convert_back_into_concrete_types() {
        let user = AuthUser::from(&GenericUser::new(json!({"id": 1})));
        assert!(GenericUser::from_auth_user(&user).is_some());
        assert!(Admin::from_auth_user(&user).is_none());
        assert!(AuthUser::from_auth_user(&user).is_some());
        assert!(<GenericUser as AuthUserRef>::from_auth_user_ref(&user).is_some());
    }
}
