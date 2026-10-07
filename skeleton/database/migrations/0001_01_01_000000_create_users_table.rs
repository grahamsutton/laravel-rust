use laravel::prelude::*;

pub struct CreateUsersTable;

#[async_trait]
impl Migration for CreateUsersTable {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("users", |table| {
            table.id();
            table.string("name");
            table.string("email").unique();
            table.timestamp("email_verified_at").nullable();
            table.string("password");
            table.remember_token();
            table.timestamps();
        })
        .await?;

        Schema::create("password_reset_tokens", |table| {
            table.string("email").primary();
            table.string("token");
            table.timestamp("created_at").nullable();
        })
        .await?;

        Schema::create("sessions", |table| {
            table.string("id").primary();
            table.foreign_id("user_id").nullable().index();
            table.string_len("ip_address", 45).nullable();
            table.text("user_agent").nullable();
            table.long_text("payload");
            table.integer("last_activity").index();
        })
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("users").await?;
        Schema::drop_if_exists("password_reset_tokens").await?;
        Schema::drop_if_exists("sessions").await
    }
}
