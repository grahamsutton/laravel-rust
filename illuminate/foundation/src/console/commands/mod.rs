//! The framework's Artisan commands.

mod app;
mod basic;
mod database;
mod environment;
mod events;
mod housekeeping;
mod inspection;
pub(crate) mod install;
mod models;
mod optimize;
mod queue;
mod routes;
mod serve;

pub use app::{DownCommand, KeyGenerateCommand, StorageLinkCommand, UpCommand};
pub use basic::{AboutCommand, CacheClearCommand, ConfigShowCommand, EnvironmentCommand, InspireCommand};
pub use database::{
    MigrateCommand, MigrateFreshCommand, MigrateInstallCommand, MigrateRefreshCommand, MigrateResetCommand,
    MigrateRollbackCommand, MigrateStatusCommand, SeedCommand, WipeCommand,
};
pub use environment::{DecryptCommand as EnvDecryptCommand, EncryptCommand as EnvEncryptCommand};
pub use events::{CacheForgetCommand, ChannelListCommand, EventListCommand};
pub use housekeeping::{ClearResetsCommand, PackageDiscoverCommand, StorageUnlinkCommand};
pub use optimize::{OptimizeClearCommand, OptimizeCommand, ViewCacheCommand, ViewClearCommand};
pub use models::{PruneCommand as ModelPruneCommand, ShowModelCommand};
pub use install::{InstallApiCommand, InstallBroadcastingCommand};
pub use inspection::{ShowCommand as DbShowCommand, TableCommand as DbTableCommand};
pub use queue::{
    ListenCommand as QueueListenCommand, PruneBatchesCommand as QueuePruneBatchesCommand,
    RetryBatchCommand as QueueRetryBatchCommand,
    ClearCommand as QueueClearCommand, FlushFailedCommand as QueueFlushCommand,
    ForgetFailedCommand as QueueForgetCommand, ListFailedCommand as QueueFailedCommand,
    MonitorCommand as QueueMonitorCommand, PauseCommand as QueuePauseCommand,
    PruneFailedCommand as QueuePruneFailedCommand, RestartCommand as QueueRestartCommand,
    ResumeCommand as QueueResumeCommand, RetryCommand as QueueRetryCommand, WorkCommand as QueueWorkCommand,
};
pub use routes::RouteListCommand;
pub use serve::ServeCommand;
