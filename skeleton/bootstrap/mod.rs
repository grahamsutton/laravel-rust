pub mod app;
pub mod providers;

pub use app::app;
pub use providers::providers;

/// The Artisan commands in `app/console/commands`, discovered at build time.
pub mod commands {
    laravel::discover_commands!();
}
