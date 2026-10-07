//! Artisan: the command line interface included with Laravel.
//!
//! ```text
//! cargo artisan list
//! cargo artisan serve
//! cargo artisan migrate
//! ```

#[tokio::main]
async fn main() {
    // Bootstrap Laravel and handle the command...
    let app = example_app::bootstrap::app().create();

    let status = app.handle_command().await;

    std::process::exit(status);
}
