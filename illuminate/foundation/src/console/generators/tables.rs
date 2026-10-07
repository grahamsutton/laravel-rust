//! `make:cache-table`, `make:session-table`, `make:queue-table`,
//! `make:queue-failed-table`, and `make:queue-batches-table`: migrations
//! for the tables Laravel's database drivers use.

use illuminate_console::{Command, Console, async_trait};
use illuminate_support::{Carbon, Result, Str};

use super::populate;
use crate::application::Application;

/// A command creating the migration for one of the framework's tables.
pub struct MakeTableCommand {
    name: &'static str,
    description: &'static str,
    /// The config key naming the table, and its default.
    table: (&'static str, &'static str),
    stub: &'static str,
}

impl MakeTableCommand {
    /// Every table migration generator.
    pub fn all() -> Vec<MakeTableCommand> {
        vec![
            MakeTableCommand {
                name: "make:cache-table",
                description: "Create a migration for the cache database table",
                table: ("cache.stores.database.table", "cache"),
                stub: CACHE,
            },
            MakeTableCommand {
                name: "make:session-table",
                description: "Create a migration for the session database table",
                table: ("session.table", "sessions"),
                stub: SESSIONS,
            },
            MakeTableCommand {
                name: "make:queue-table",
                description: "Create a migration for the queue jobs database table",
                table: ("queue.connections.database.table", "jobs"),
                stub: JOBS,
            },
            MakeTableCommand {
                name: "make:queue-failed-table",
                description: "Create a migration for the failed queue jobs database table",
                table: ("queue.failed.table", "failed_jobs"),
                stub: FAILED_JOBS,
            },
            MakeTableCommand {
                name: "make:queue-batches-table",
                description: "Create a migration for the batches database table",
                table: ("queue.batching.table", "job_batches"),
                stub: JOB_BATCHES,
            },
        ]
    }
}

#[async_trait]
impl Command for MakeTableCommand {
    fn signature(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        self.description
    }

    async fn handle(&self, cmd: Console) -> Result<()> {
        let app = Application::current();
        let table = app.config_repository().string_or(self.table.0, self.table.1);
        let dir = std::path::PathBuf::from(app.database_path("migrations"));
        let suffix = format!("_create_{table}_table.rs");

        let exists = std::fs::read_dir(&dir).is_ok_and(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
        });
        if exists {
            cmd.components().error("Migration already exists.");
            return cmd.exit(1);
        }

        let contents = populate(
            self.stub,
            &[("class", &Str::studly(&format!("create_{table}_table"))), ("table", &table)],
        );
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{}{suffix}", Carbon::now().format("Y_m_d_His"))), contents)?;

        cmd.components().info("Migration created successfully.");
        Ok(())
    }
}

const CACHE: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Migration for {{ class }} {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("{{ table }}", |table| {
            table.string("key").primary();
            table.medium_text("value");
            table.big_integer("expiration").index();
        })
        .await?;

        Schema::create("{{ table }}_locks", |table| {
            table.string("key").primary();
            table.string("owner");
            table.big_integer("expiration").index();
        })
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("{{ table }}").await?;
        Schema::drop_if_exists("{{ table }}_locks").await
    }
}
"#;

const SESSIONS: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Migration for {{ class }} {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("{{ table }}", |table| {
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
        Schema::drop_if_exists("{{ table }}").await
    }
}
"#;

const JOBS: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Migration for {{ class }} {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("{{ table }}", |table| {
            table.id();
            table.string("queue").index();
            table.long_text("payload");
            table.unsigned_small_integer("attempts");
            table.unsigned_integer("reserved_at").nullable();
            table.unsigned_integer("available_at");
            table.unsigned_integer("created_at");
        })
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("{{ table }}").await
    }
}
"#;

const FAILED_JOBS: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Migration for {{ class }} {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("{{ table }}", |table| {
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
        Schema::drop_if_exists("{{ table }}").await
    }
}
"#;

const JOB_BATCHES: &str = r#"use laravel::prelude::*;

pub struct {{ class }};

#[async_trait]
impl Migration for {{ class }} {
    /// Run the migrations.
    async fn up(&self) -> Result<()> {
        Schema::create("{{ table }}", |table| {
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
        .await
    }

    /// Reverse the migrations.
    async fn down(&self) -> Result<()> {
        Schema::drop_if_exists("{{ table }}").await
    }
}
"#;
