//! The `personal_access_tokens` migration.

use illuminate_database::{Migration, Schema, async_trait};
use illuminate_support::Result;

/// The table personal access tokens are stored in.
pub const PERSONAL_ACCESS_TOKENS_TABLE: &str = "personal_access_tokens";

/// Create the `personal_access_tokens` table (Sanctum's
/// `create_personal_access_tokens_table` migration, published by
/// `install:api`).
///
/// ```no_run
/// use illuminate_database::{Migrator, migrations};
/// use laravel_sanctum::CreatePersonalAccessTokensTable;
///
/// # async fn example() -> illuminate_support::Result<()> {
/// let migrator = Migrator::resolve(migrations![
///     CreatePersonalAccessTokensTable::NAME => CreatePersonalAccessTokensTable,
/// ]);
/// migrator.run(Default::default()).await?;
/// # Ok(()) }
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct CreatePersonalAccessTokensTable;

impl CreatePersonalAccessTokensTable {
    /// The migration's name, as Sanctum publishes it.
    pub const NAME: &'static str = "2019_12_14_000001_create_personal_access_tokens_table";
}

#[async_trait]
impl Migration for CreatePersonalAccessTokensTable {
    async fn up(&self) -> Result<()> {
        Schema::create(PERSONAL_ACCESS_TOKENS_TABLE, |table| {
            table.id();
            table.morphs("tokenable");
            table.text("name");
            table.string_len("token", 64).unique();
            table.text("abilities").nullable();
            table.timestamp("last_used_at").nullable();
            table.timestamp("expires_at").nullable().index();
            table.timestamps();
        })
        .await
    }

    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists(PERSONAL_ACCESS_TOKENS_TABLE).await
    }
}
