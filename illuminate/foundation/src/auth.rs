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
use illuminate_support::{Error, Result, Str, Value, ValueExt, json};

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
