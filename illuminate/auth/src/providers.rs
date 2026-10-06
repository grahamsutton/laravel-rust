//! User providers: where guards find their users.

use std::sync::{Arc, RwLock};

use illuminate_hashing::Hash;
use illuminate_http::async_trait;
use illuminate_support::{Result, Value, ValueExt};

use crate::support::{hash_equals, loosely_equal, plain_password, without_passwords};
use crate::user::{AuthUser, Authenticatable, GenericUser};

/// Retrieves users from persistent storage — Laravel's `UserProvider`
/// contract.
///
/// Guards never touch storage themselves: they ask their provider. The
/// framework ships an in-memory [`ArrayUserProvider`]; Eloquent and the
/// query builder plug in their own (registered with `Auth::provider`).
///
/// Only the lookups and remember-token persistence are required.
/// [`retrieve_by_token`](UserProvider::retrieve_by_token) and
/// [`validate_credentials`](UserProvider::validate_credentials) default to
/// Laravel's behavior (compare the remember token in constant time, and
/// check the `password` credential with the `Hash` facade).
///
/// ```
/// use illuminate_auth::{AuthUser, GenericUser, UserProvider};
/// use illuminate_http::async_trait;
/// use illuminate_support::{json, Result, Value};
///
/// struct MongoUserProvider;
///
/// #[async_trait]
/// impl UserProvider for MongoUserProvider {
///     async fn retrieve_by_id(&self, identifier: &Value) -> Result<Option<AuthUser>> {
///         Ok(Some(AuthUser::new(GenericUser::new(json!({"id": identifier})))))
///     }
///
///     async fn update_remember_token(&self, _user: &AuthUser, _token: &str) -> Result<()> {
///         Ok(())
///     }
///
///     async fn retrieve_by_credentials(&self, _credentials: &Value) -> Result<Option<AuthUser>> {
///         Ok(None)
///     }
/// }
/// ```
#[async_trait]
pub trait UserProvider: Send + Sync {
    /// Retrieve a user by their unique identifier.
    async fn retrieve_by_id(&self, identifier: &Value) -> Result<Option<AuthUser>>;

    /// Retrieve a user by their unique identifier and "remember me" token.
    async fn retrieve_by_token(&self, identifier: &Value, token: &str) -> Result<Option<AuthUser>> {
        let Some(user) = self.retrieve_by_id(identifier).await? else {
            return Ok(None);
        };
        Ok(match user.remember_token() {
            Some(remember) if hash_equals(&remember, token) => Some(user),
            _ => None,
        })
    }

    /// Persist the "remember me" token for the given user.
    async fn update_remember_token(&self, user: &AuthUser, token: &str) -> Result<()>;

    /// Retrieve a user by the given credentials. Every credential whose key
    /// contains "password" must be ignored — never validate passwords here.
    async fn retrieve_by_credentials(&self, credentials: &Value) -> Result<Option<AuthUser>>;

    /// Validate a user against the given credentials (the `password` key).
    async fn validate_credentials(&self, user: &AuthUser, credentials: &Value) -> Result<bool> {
        Ok(validate_password(user, credentials))
    }

    /// Rehash the user's password if it's using an outdated work factor (or
    /// always, when `force` is true), persisting the new hash. Returns the
    /// updated user when the password was rehashed.
    async fn rehash_password_if_required(
        &self,
        _user: &AuthUser,
        _credentials: &Value,
        _force: bool,
    ) -> Result<Option<AuthUser>> {
        Ok(None)
    }
}

/// Check the `password` credential against the user's hashed password using
/// the `Hash` facade — the standard implementation of
/// [`UserProvider::validate_credentials`].
pub fn validate_password(user: &AuthUser, credentials: &Value) -> bool {
    let Some(plain) = plain_password(credentials) else {
        return false;
    };
    let hashed = user.auth_password();
    if hashed.is_empty() {
        return false;
    }
    Hash::check(&plain, &hashed)
}

/// Determine if the user's password needs rehashing (or `force` is set),
/// returning the new hash of the `password` credential if so.
pub fn rehashed_password(
    user: &AuthUser,
    credentials: &Value,
    force: bool,
) -> Result<Option<String>> {
    let hashed = user.auth_password();
    if !force && !Hash::needs_rehash(&hashed) {
        return Ok(None);
    }
    let Some(plain) = plain_password(credentials) else {
        return Ok(None);
    };
    Ok(Some(Hash::make(&plain)?))
}

/// Determine if a user's attributes match the given (password-free)
/// credentials: scalar credentials must be loosely equal, and list
/// credentials behave like `where in`.
pub fn matches_credentials(attributes: &Value, credentials: &Value) -> bool {
    let credentials = without_passwords(credentials);
    !credentials.is_empty()
        && credentials.iter().all(|(key, expected)| {
            let actual = attributes.dot(key).unwrap_or(&Value::Null);
            match expected {
                Value::Array(candidates) => candidates.iter().any(|c| loosely_equal(actual, c)),
                other => loosely_equal(actual, other),
            }
        })
}

/// An in-memory user provider — perfect for tests, prototypes, and
/// applications with a fixed set of users.
///
/// It is registered as the `array` provider driver, reading its users from
/// the provider's `users` configuration:
///
/// ```text
/// 'providers' => [
///     'users' => ['driver' => 'array', 'users' => [
///         ['id' => 1, 'email' => 'taylor@laravel.com', 'password' => '$2y$...'],
///     ]],
/// ],
/// ```
///
/// ```
/// use illuminate_auth::{ArrayUserProvider, GenericUser, UserProvider};
/// use illuminate_support::json;
///
/// # let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// # runtime.block_on(async {
/// let provider = ArrayUserProvider::new(vec![
///     GenericUser::new(json!({"id": 1, "email": "taylor@laravel.com"})),
/// ]);
///
/// let user = provider.retrieve_by_credentials(&json!({"email": "taylor@laravel.com", "password": "secret"})).await.unwrap();
/// assert_eq!(user.unwrap().id(), json!(1));
///
/// assert!(provider.retrieve_by_id(&json!("1")).await.unwrap().is_some());
/// # });
/// ```
pub struct ArrayUserProvider<U: Authenticatable = GenericUser> {
    users: RwLock<Vec<U>>,
}

impl<U: Authenticatable> ArrayUserProvider<U> {
    /// Create a provider holding the given users.
    pub fn new(users: Vec<U>) -> Self {
        Self {
            users: RwLock::new(users),
        }
    }

    /// Add a user.
    pub fn push(&self, user: U) {
        self.users.write().unwrap().push(user);
    }

    /// All of the users (with any remember tokens and rehashed passwords).
    pub fn users(&self) -> Vec<U> {
        self.users.read().unwrap().clone()
    }

    /// Find a user by identifier.
    pub fn find(&self, identifier: &Value) -> Option<U> {
        self.users
            .read()
            .unwrap()
            .iter()
            .find(|user| loosely_equal(&user.auth_identifier(), identifier))
            .cloned()
    }

    /// Replace the stored user that has the same identifier as `user`.
    fn update(&self, user: &AuthUser, change: impl FnOnce(&mut U)) -> Option<U> {
        let id = user.auth_identifier();
        let mut users = self.users.write().unwrap();
        let stored = users
            .iter_mut()
            .find(|candidate| loosely_equal(&candidate.auth_identifier(), &id))?;
        change(stored);
        Some(stored.clone())
    }
}

impl ArrayUserProvider<GenericUser> {
    /// Create a provider from a list of attribute objects (the `users`
    /// configuration of the `array` driver).
    pub fn from_value(users: &Value) -> Self {
        let users = match users {
            Value::Array(items) => items.iter().cloned().map(GenericUser::new).collect(),
            Value::Object(map) => map.values().cloned().map(GenericUser::new).collect(),
            _ => Vec::new(),
        };
        Self::new(users)
    }
}

impl<U: Authenticatable> Default for ArrayUserProvider<U> {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[async_trait]
impl<U: Authenticatable> UserProvider for ArrayUserProvider<U> {
    async fn retrieve_by_id(&self, identifier: &Value) -> Result<Option<AuthUser>> {
        Ok(self.find(identifier).map(AuthUser::new))
    }

    async fn update_remember_token(&self, user: &AuthUser, token: &str) -> Result<()> {
        self.update(user, |stored| stored.set_remember_token(token));
        Ok(())
    }

    async fn retrieve_by_credentials(&self, credentials: &Value) -> Result<Option<AuthUser>> {
        Ok(self
            .users
            .read()
            .unwrap()
            .iter()
            .find(|user| matches_credentials(&user.to_value(), credentials))
            .cloned()
            .map(AuthUser::new))
    }

    async fn rehash_password_if_required(
        &self,
        user: &AuthUser,
        credentials: &Value,
        force: bool,
    ) -> Result<Option<AuthUser>> {
        let Some(hashed) = rehashed_password(user, credentials, force)? else {
            return Ok(None);
        };
        Ok(self
            .update(user, |stored| stored.set_auth_password(&hashed))
            .map(AuthUser::new)
            .or_else(|| Some(user.with_auth_password(&hashed))))
    }
}

/// Lets a shared provider be used wherever a provider is expected.
#[async_trait]
impl<P: UserProvider + ?Sized> UserProvider for Arc<P> {
    async fn retrieve_by_id(&self, identifier: &Value) -> Result<Option<AuthUser>> {
        (**self).retrieve_by_id(identifier).await
    }

    async fn retrieve_by_token(&self, identifier: &Value, token: &str) -> Result<Option<AuthUser>> {
        (**self).retrieve_by_token(identifier, token).await
    }

    async fn update_remember_token(&self, user: &AuthUser, token: &str) -> Result<()> {
        (**self).update_remember_token(user, token).await
    }

    async fn retrieve_by_credentials(&self, credentials: &Value) -> Result<Option<AuthUser>> {
        (**self).retrieve_by_credentials(credentials).await
    }

    async fn validate_credentials(&self, user: &AuthUser, credentials: &Value) -> Result<bool> {
        (**self).validate_credentials(user, credentials).await
    }

    async fn rehash_password_if_required(
        &self,
        user: &AuthUser,
        credentials: &Value,
        force: bool,
    ) -> Result<Option<AuthUser>> {
        (**self)
            .rehash_password_if_required(user, credentials, force)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{hashing_container, user};
    use illuminate_container::Container;
    use illuminate_support::json;

    #[tokio::test]
    async fn the_array_provider_finds_users() {
        let container = hashing_container();
        let _guard = Container::set_local_instance(container);
        let provider = ArrayUserProvider::new(vec![
            user(1, "taylor@laravel.com", "secret"),
            user(2, "abigail@laravel.com", "secret"),
        ]);

        let found = provider.retrieve_by_id(&json!(2)).await.unwrap().unwrap();
        assert_eq!(found.to_value()["email"], json!("abigail@laravel.com"));
        assert!(provider.retrieve_by_id(&json!(3)).await.unwrap().is_none());

        let by_credentials = provider
            .retrieve_by_credentials(&json!({"email": "taylor@laravel.com", "password": "wrong"}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_credentials.id(), json!(1));

        let by_list = provider
            .retrieve_by_credentials(
                &json!({"email": ["nobody@laravel.com", "abigail@laravel.com"]}),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_list.id(), json!(2));

        assert!(
            provider
                .retrieve_by_credentials(&json!({"password": "secret"}))
                .await
                .unwrap()
                .is_none()
        );

        assert!(
            provider
                .validate_credentials(&found, &json!({"password": "secret"}))
                .await
                .unwrap()
        );
        assert!(
            !provider
                .validate_credentials(&found, &json!({"password": "nope"}))
                .await
                .unwrap()
        );
        assert!(
            !provider
                .validate_credentials(&found, &json!({}))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn remember_tokens_are_stored_and_checked() {
        let container = hashing_container();
        let _guard = Container::set_local_instance(container);
        let provider = ArrayUserProvider::new(vec![user(1, "taylor@laravel.com", "secret")]);
        let taylor = provider.retrieve_by_id(&json!(1)).await.unwrap().unwrap();

        assert!(
            provider
                .retrieve_by_token(&json!(1), "token")
                .await
                .unwrap()
                .is_none()
        );
        provider
            .update_remember_token(&taylor, "token")
            .await
            .unwrap();
        assert!(
            provider
                .retrieve_by_token(&json!("1"), "token")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            provider
                .retrieve_by_token(&json!(1), "other")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn passwords_are_rehashed_when_forced() {
        let container = hashing_container();
        let _guard = Container::set_local_instance(container);
        let provider = ArrayUserProvider::new(vec![user(1, "taylor@laravel.com", "secret")]);
        let taylor = provider.retrieve_by_id(&json!(1)).await.unwrap().unwrap();

        let unchanged = provider
            .rehash_password_if_required(&taylor, &json!({"password": "secret"}), false)
            .await
            .unwrap();
        assert!(unchanged.is_none());

        let rehashed = provider
            .rehash_password_if_required(&taylor, &json!({"password": "secret"}), true)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(rehashed.auth_password(), taylor.auth_password());
        assert!(Hash::check("secret", &rehashed.auth_password()));
        assert_eq!(
            provider.users()[0].auth_password(),
            rehashed.auth_password()
        );
    }

    #[test]
    fn the_array_provider_reads_configuration() {
        let provider = ArrayUserProvider::from_value(&json!([{"id": 1}, {"id": 2}]));
        assert_eq!(provider.users().len(), 2);
        assert!(provider.find(&json!(2)).is_some());
    }
}
