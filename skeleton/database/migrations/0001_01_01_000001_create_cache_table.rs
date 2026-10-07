use laravel::prelude::*;

pub struct CreateCacheTable;

#[async_trait]
impl Migration for CreateCacheTable {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("cache", |table| {
            table.string("key").primary();
            table.medium_text("value");
            table.big_integer("expiration").index();
        })
        .await?;

        Schema::create("cache_locks", |table| {
            table.string("key").primary();
            table.string("owner");
            table.big_integer("expiration").index();
        })
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("cache").await?;
        Schema::drop_if_exists("cache_locks").await
    }
}
