use laravel::prelude::*;

pub struct CreateJobsTable;

#[async_trait]
impl Migration for CreateJobsTable {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("jobs", |table| {
            table.id();
            table.string("queue").index();
            table.long_text("payload");
            table.unsigned_small_integer("attempts");
            table.unsigned_integer("reserved_at").nullable();
            table.unsigned_integer("available_at");
            table.unsigned_integer("created_at");
        })
        .await?;

        Schema::create("job_batches", |table| {
            table.string("id").primary();
            table.string("name");
            table.integer("total_jobs");
            table.integer("pending_jobs");
            table.integer("failed_jobs");
            table.long_text("failed_job_ids");
            table.medium_text("options").nullable();
            table.integer("cancelled_at").nullable();
            table.integer("created_at");
            table.integer("finished_at").nullable();
        })
        .await?;

        Schema::create("failed_jobs", |table| {
            table.id();
            table.string("uuid").unique();
            table.string("connection");
            table.string("queue");
            table.long_text("payload");
            table.long_text("exception");
            table.timestamp("failed_at").use_current();

            table.index(["connection", "queue", "failed_at"]);
        })
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("jobs").await?;
        Schema::drop_if_exists("job_batches").await?;
        Schema::drop_if_exists("failed_jobs").await
    }
}
