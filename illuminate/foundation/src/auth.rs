//! Authenticating against your database: the `eloquent` and `database`
//! user providers.
//!
//! `config/auth.rs` names the user model by class name:
//!
//! ```ignore
//! "providers": {
//!     "users": { "driver": "eloquent", "model": "User" },
//! }
//! ```
//!
//! and the model opts in with `#[derive(Authenticatable)]`, which registers
//! it here.

use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use illuminate_auth::{AuthUser, Authenticatable, GenericUser, UserProvider, rehashed_password};
use illuminate_database::eloquent::Model;
use illuminate_database::{Connection, DatabaseManager};
use illuminate_support::{Carbon, Error, Result, Str, Value, ValueExt, json};

/// An authenticatable Eloquent model, registered by `#[derive(Authenticatable)]`.
pub struct AuthModel {
    class_name: fn() -> &'static str,
    provider: fn() -> Arc<dyn UserProvider>,
}

inventory::collect!(AuthModel);

impl AuthModel {
    /// The registration for the given model.
    pub const fn of<M: Model + Authenticatable>() -> Self {
        Self {
            class_name: <M as Model>::class_name,
            provider: eloquent_provider::<M>,
        }
    }

    /// Find the registered model for a configured name (`User`,
    /// `App\Models\User`, or `crate::app::models::User`).
    pub fn find(name: &str) -> Option<&'static AuthModel> {
        let wanted = Str::class_basename(&name.replace("::", "\\"));
        inventory::iter::<AuthModel>
            .into_iter()
            .find(|model| (model.class_name)() == wanted)
    }

    /// Create the user provider for this model.
    pub fn provider(&self) -> Arc<dyn UserProvider> {
        (self.provider)()
    }
}

fn eloquent_provider<M: Model + Authenticatable>() -> Arc<dyn UserProvider> {
    Arc::new(EloquentUserProvider::<M>::new())
}

/// Credentials without passwords (never query by password).
fn query_credentials(credentials: &Value) -> Vec<(String, Value)> {
    match credentials {
        Value::Object(map) => map
            .iter()
            .filter(|(key, _)| !key.contains("password"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Retrieves users through an Eloquent model.
pub struct EloquentUserProvider<M> {
    model: PhantomData<fn() -> M>,
}

impl<M: Model + Authenticatable> EloquentUserProvider<M> {
    /// Create a provider for the model.
    pub fn new() -> Self {
        Self { model: PhantomData }
    }

    /// Write columns without touching the model's timestamps.
    async fn write(&self, user: &M, values: Value) -> Result<()> {
        M::get_connection()
            .table(M::table())
            .where_(M::primary_key(), Model::get_key(user))
            .update(values)
            .await?;
        Ok(())
    }
}

impl<M: Model + Authenticatable> Default for EloquentUserProvider<M> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<M: Model + Authenticatable> UserProvider for EloquentUserProvider<M> {
    async fn retrieve_by_id(&self, identifier: &Value) -> Result<Option<AuthUser>> {
        Ok(M::query()
            .where_key(identifier.clone())
            .first()
            .await?
            .map(|user| AuthUser::from(&user)))
    }

    async fn update_remember_token(&self, user: &AuthUser, token: &str) -> Result<()> {
        let Some(user) = user.downcast::<M>() else {
            return Ok(());
        };
        let column = user.remember_token_name();
        self.write(&user, json!({ column: token })).await
    }

    async fn retrieve_by_credentials(&self, credentials: &Value) -> Result<Option<AuthUser>> {
        let credentials = query_credentials(credentials);
        if credentials.is_empty() {
            return Ok(None);
        }
        let mut query = M::query();
        for (key, value) in credentials {
            query = match value {
                Value::Array(values) => query.where_in(key.as_str(), values),
                value => query.where_(key.as_str(), value),
            };
        }
        Ok(query.first().await?.map(|user| AuthUser::from(&user)))
    }

    async fn rehash_password_if_required(
        &self,
        user: &AuthUser,
        credentials: &Value,
        force: bool,
    ) -> Result<Option<AuthUser>> {
        let Some(hash) = rehashed_password(user, credentials, force)? else {
            return Ok(None);
        };
        let Some(mut model) = user.downcast::<M>() else {
            return Ok(None);
        };
        model.set_auth_password(&hash);
        let column = model.auth_password_name();
        self.write(&model, json!({ column: hash })).await?;
        Ok(Some(AuthUser::from(&model)))
    }
}

/// Retrieves users straight from a table, as [`GenericUser`]s.
pub struct DatabaseUserProvider {
    connection: Connection,
    table: String,
}

impl DatabaseUserProvider {
    /// Create a provider for the given connection and table.
    pub fn new(connection: Connection, table: impl Into<String>) -> Self {
        Self {
            connection,
            table: table.into(),
        }
    }

    fn user(row: Option<Value>) -> Option<AuthUser> {
        row.map(|attributes| AuthUser::from(&GenericUser::new(attributes)))
    }
}

#[async_trait]
impl UserProvider for DatabaseUserProvider {
    async fn retrieve_by_id(&self, identifier: &Value) -> Result<Option<AuthUser>> {
        let row = self.connection.table(&self.table).where_("id", identifier.clone()).first().await?;
        Ok(Self::user(row))
    }

    async fn update_remember_token(&self, user: &AuthUser, token: &str) -> Result<()> {
        self.connection
            .table(&self.table)
            .where_(user.auth_identifier_name(), user.auth_identifier())
            .update(json!({ user.remember_token_name(): token }))
            .await?;
        Ok(())
    }

    async fn retrieve_by_credentials(&self, credentials: &Value) -> Result<Option<AuthUser>> {
        let credentials = query_credentials(credentials);
        if credentials.is_empty() {
            return Ok(None);
        }
        let mut query = self.connection.table(&self.table);
        for (key, value) in credentials {
            query = match value {
                Value::Array(values) => query.where_in(key.as_str(), values),
                value => query.where_(key.as_str(), value),
            };
        }
        Ok(Self::user(query.first().await?))
    }

    async fn rehash_password_if_required(
        &self,
        user: &AuthUser,
        credentials: &Value,
        force: bool,
    ) -> Result<Option<AuthUser>> {
        let Some(hash) = rehashed_password(user, credentials, force)? else {
            return Ok(None);
        };
        self.connection
            .table(&self.table)
            .where_(user.auth_identifier_name(), user.auth_identifier())
            .update(json!({ user.auth_password_name(): hash.clone() }))
            .await?;
        Ok(Some(user.with_auth_password(&hash)))
    }
}

/// A provider that explains what's missing on every call.
struct MissingModel(String);

#[async_trait]
impl UserProvider for MissingModel {
    async fn retrieve_by_id(&self, _identifier: &Value) -> Result<Option<AuthUser>> {
        Err(self.error())
    }

    async fn update_remember_token(&self, _user: &AuthUser, _token: &str) -> Result<()> {
        Err(self.error())
    }

    async fn retrieve_by_credentials(&self, _credentials: &Value) -> Result<Option<AuthUser>> {
        Err(self.error())
    }
}

impl MissingModel {
    fn error(&self) -> Error {
        illuminate_support::error::error!(
            "The authentication model [{}] is not registered. Add #[derive(Authenticatable)] to it.",
            self.0
        )
    }
}

/// Register the `eloquent` and `database` user provider drivers.
pub fn register_providers() {
    illuminate_auth::Auth::provider("eloquent", |_, config| {
        let model = config.get("model").map(ValueExt::to_string_lossy).unwrap_or_else(|| "User".into());
        match AuthModel::find(&model) {
            Some(registered) => registered.provider(),
            None => Arc::new(MissingModel(model)),
        }
    });
    illuminate_auth::Auth::provider("database", |_, config| {
        let manager = DatabaseManager::resolve();
        let connection = match config.get("connection").and_then(Value::as_str) {
            Some(name) => manager.connection(name),
            None => manager.default_connection(),
        };
        let table = config.get("table").map(ValueExt::to_string_lossy).unwrap_or_else(|| "users".into());
        Arc::new(DatabaseUserProvider::new(connection, table))
    });
}

// ---------------------------------------------------------------------------
// Password reset tokens
// ---------------------------------------------------------------------------

/// A new, random token: an HMAC of random bytes with the application key,
/// like Laravel's `createNewToken`.
fn new_reset_token(hash_key: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(hash_key).expect("HMAC accepts keys of any length");
    mac.update(Str::random(40).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Stores password reset tokens in the `password_reset_tokens` table
/// (Laravel's `DatabaseTokenRepository`). Tokens are stored hashed.
pub struct DatabaseTokenRepository {
    connection: Connection,
    table: String,
    hash_key: Vec<u8>,
    expires: i64,
    throttle: i64,
}

impl DatabaseTokenRepository {
    /// Create a repository; `expires` and `throttle` are in seconds.
    pub fn new(connection: Connection, table: impl Into<String>, hash_key: Vec<u8>, expires: i64, throttle: i64) -> Self {
        Self {
            connection,
            table: table.into(),
            hash_key,
            expires,
            throttle,
        }
    }

    async fn record(&self, user: &AuthUser) -> Result<Option<(String, Carbon)>> {
        let row = self
            .connection
            .table(&self.table)
            .where_("email", user.email_for_password_reset())
            .first()
            .await?;
        Ok(row.and_then(|row| {
            let token = row.get("token")?.as_str()?.to_string();
            let created_at = Carbon::parse(&row.get("created_at")?.to_string_lossy()).ok()?;
            Some((token, created_at))
        }))
    }
}

#[async_trait]
impl illuminate_auth::TokenRepository for DatabaseTokenRepository {
    async fn create(&self, user: &AuthUser) -> Result<String> {
        let email = user.email_for_password_reset();
        self.delete(user).await?;
        let token = new_reset_token(&self.hash_key);
        self.connection
            .table(&self.table)
            .insert(json!({
                "email": email,
                "token": illuminate_hashing::Hash::make(&token)?,
                "created_at": Carbon::now().format("Y-m-d H:i:s"),
            }))
            .await?;
        Ok(token)
    }

    async fn exists(&self, user: &AuthUser, token: &str) -> Result<bool> {
        Ok(match self.record(user).await? {
            Some((hashed, created_at)) => {
                created_at.add_seconds(self.expires) > Carbon::now() && illuminate_hashing::Hash::check(token, &hashed)
            }
            None => false,
        })
    }

    async fn recently_created_token(&self, user: &AuthUser) -> Result<bool> {
        if self.throttle <= 0 {
            return Ok(false);
        }
        Ok(self
            .record(user)
            .await?
            .is_some_and(|(_, created_at)| created_at.add_seconds(self.throttle) > Carbon::now()))
    }

    async fn delete(&self, user: &AuthUser) -> Result<()> {
        self.connection
            .table(&self.table)
            .where_("email", user.email_for_password_reset())
            .delete()
            .await?;
        Ok(())
    }

    async fn delete_expired(&self) -> Result<()> {
        let expired_at = Carbon::now().sub_seconds(self.expires).format("Y-m-d H:i:s");
        self.connection
            .table(&self.table)
            .where_op("created_at", "<", expired_at)
            .delete()
            .await?;
        Ok(())
    }
}

/// Stores password reset tokens in a cache store (Laravel's
/// `CacheTokenRepository`, the `cache` driver).
pub struct CacheTokenRepository {
    store: Option<String>,
    hash_key: Vec<u8>,
    expires: i64,
    throttle: i64,
}

impl CacheTokenRepository {
    /// Create a repository on the given store (`None` for the default).
    pub fn new(store: Option<String>, hash_key: Vec<u8>, expires: i64, throttle: i64) -> Self {
        Self {
            store,
            hash_key,
            expires,
            throttle,
        }
    }

    fn cache(&self) -> Result<illuminate_cache::Repository> {
        match &self.store {
            Some(store) => illuminate_cache::Cache::store(store),
            None => illuminate_cache::Cache::default_store(),
        }
    }

    fn key(user: &AuthUser) -> String {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(user.email_for_password_reset().as_bytes()))
    }

    async fn record(&self, user: &AuthUser) -> Result<Option<(String, Carbon)>> {
        let value: Option<Value> = self.cache()?.get(&Self::key(user)).await?;
        Ok(value.and_then(|value| {
            let token = value.get(0)?.as_str()?.to_string();
            let created_at = Carbon::parse(value.get(1)?.as_str()?).ok()?;
            Some((token, created_at))
        }))
    }
}

#[async_trait]
impl illuminate_auth::TokenRepository for CacheTokenRepository {
    async fn create(&self, user: &AuthUser) -> Result<String> {
        let token = new_reset_token(&self.hash_key);
        let record = json!([illuminate_hashing::Hash::make(&token)?, Carbon::now().format("Y-m-d H:i:s")]);
        self.cache()?.put(&Self::key(user), record, self.expires.max(0) as u64).await?;
        Ok(token)
    }

    async fn exists(&self, user: &AuthUser, token: &str) -> Result<bool> {
        Ok(match self.record(user).await? {
            Some((hashed, created_at)) => {
                created_at.add_seconds(self.expires) > Carbon::now() && illuminate_hashing::Hash::check(token, &hashed)
            }
            None => false,
        })
    }

    async fn recently_created_token(&self, user: &AuthUser) -> Result<bool> {
        if self.throttle <= 0 {
            return Ok(false);
        }
        Ok(self
            .record(user)
            .await?
            .is_some_and(|(_, created_at)| created_at.add_seconds(self.throttle) > Carbon::now()))
    }

    async fn delete(&self, user: &AuthUser) -> Result<()> {
        self.cache()?.forget(&Self::key(user)).await?;
        Ok(())
    }

    async fn delete_expired(&self) -> Result<()> {
        // Cache entries expire on their own.
        Ok(())
    }
}

/// Register the `database` and `cache` password reset token repositories.
pub fn register_token_repositories() {
    let seconds = |config: &Value, key: &str, default: i64, scale: i64| {
        config.get(key).and_then(ValueExt::to_i64_lossy).unwrap_or(default) * scale
    };
    illuminate_auth::Password::token_repository("database", move |_, config, key| {
        let manager = DatabaseManager::resolve();
        let connection = match config.get("connection").and_then(Value::as_str) {
            Some(name) => manager.connection(name),
            None => manager.default_connection(),
        };
        let table = config
            .get("table")
            .map(ValueExt::to_string_lossy)
            .unwrap_or_else(|| "password_reset_tokens".into());
        Arc::new(DatabaseTokenRepository::new(
            connection,
            table,
            key.to_vec(),
            seconds(config, "expire", 60, 60),
            seconds(config, "throttle", 60, 1),
        ))
    });
    illuminate_auth::Password::token_repository("cache", move |_, config, key| {
        let store = config.get("store").and_then(Value::as_str).map(str::to_string);
        Arc::new(CacheTokenRepository::new(
            store,
            key.to_vec(),
            seconds(config, "expire", 60, 60),
            seconds(config, "throttle", 60, 1),
        ))
    });
}
