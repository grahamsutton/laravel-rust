//! The framework's Artisan commands.

mod app;
mod basic;
mod database;
mod routes;
mod serve;

pub use app::{DownCommand, KeyGenerateCommand, StorageLinkCommand, UpCommand};
pub use basic::{AboutCommand, CacheClearCommand, ConfigShowCommand, EnvironmentCommand, InspireCommand};
pub use database::{
    MigrateCommand, MigrateFreshCommand, MigrateInstallCommand, MigrateRefreshCommand, MigrateResetCommand,
    MigrateRollbackCommand, MigrateStatusCommand, SeedCommand, WipeCommand,
};
pub use routes::RouteListCommand;
pub use serve::ServeCommand;
