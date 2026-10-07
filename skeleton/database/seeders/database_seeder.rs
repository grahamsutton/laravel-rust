use laravel::prelude::*;

use crate::app::models::User;

#[derive(Default)]
pub struct DatabaseSeeder;

#[async_trait]
impl Seeder for DatabaseSeeder {
    /// Seed the application's database.
    async fn run(&self) -> Result<()> {
        // User::factory().count(10).create().await?;

        User::factory()
            .state(json!({
                "name": "Test User",
                "email": "test@example.com",
            }))
            .create_one()
            .await?;

        Ok(())
    }
}
