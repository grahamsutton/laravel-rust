//! A tiny Artisan: `cargo run -p illuminate-console --example artisan -- list`

use illuminate_console::prelude::*;
use illuminate_console::prompts;

#[tokio::main]
async fn main() {
    Artisan::command("inspire", |cmd| async move {
        cmd.components()
            .info("Simplicity is the ultimate sophistication. — Leonardo da Vinci");
        Ok(())
    })
    .purpose("Display an inspiring quote");

    Artisan::command(
        "greet {name? : Who to greet} {--yell : Shout the greeting}",
        |cmd| async move {
            let name = match cmd.argument("name") {
                Some(name) => name,
                None => prompts::text("What is your name?")
                    .required(true)
                    .prompt()?,
            };

            let greeting = format!("Hello, {name}!");
            cmd.info(if cmd.option_bool("yell") {
                greeting.to_uppercase()
            } else {
                greeting
            });

            Ok(())
        },
    )
    .purpose("Greet someone");

    Artisan::command("migrate", |cmd| async move {
        for migration in [
            "2014_10_12_000000_create_users_table",
            "2019_08_19_000000_create_failed_jobs_table",
        ] {
            cmd.components()
                .task(migration, || async { Ok(()) })
                .await?;
        }
        cmd.new_line(1);
        Ok(())
    })
    .purpose("Run the database migrations");

    Schedule::command("inspire").hourly();
    Schedule::call(|| async {})
        .daily()
        .description("Prune stale sessions");

    std::process::exit(Artisan::run(std::env::args()).await);
}
