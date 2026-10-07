use laravel::prelude::*;

#[derive(Default)]
pub struct DatabaseSeeder;

#[async_trait]
impl Seeder for DatabaseSeeder {
    /// Seed the application's database.
    async fn run(&self) -> Result<()> {
        Ok(())
    }
}
