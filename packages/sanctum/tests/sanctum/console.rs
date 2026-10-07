//! `sanctum:prune-expired`.

use illuminate_console::{Artisan, Command};
use illuminate_database::eloquent::*;
use laravel_sanctum::{HasApiTokens, PersonalAccessToken, PruneExpired};

use crate::support::{app, app_with, taylor, travel_to};

async fn names() -> Vec<String> {
    PersonalAccessToken::order_by("id", "asc")
        .get()
        .await
        .unwrap()
        .into_iter()
        .map(|token| token.name)
        .collect()
}

#[tokio::test]
async fn tokens_expired_for_more_than_the_given_hours_are_pruned() {
    let _app = app().await;
    let now = travel_to("2025-06-01 12:00:00");
    let user = taylor().await;
    user.create_token_with_expiry("long-gone", &["*"], now.sub_hours(30))
        .await
        .unwrap();
    user.create_token_with_expiry("recently-expired", &["*"], now.sub_hours(2))
        .await
        .unwrap();
    user.create_token_with_expiry("still-valid", &["*"], now.add_day())
        .await
        .unwrap();
    user.create_token("forever", &["*"]).await.unwrap();

    Artisan::register(PruneExpired);
    let code = Artisan::call("sanctum:prune-expired", ()).await.unwrap();

    assert_eq!(code, 0);
    assert_eq!(
        names().await,
        ["recently-expired", "still-valid", "forever"]
    );
    let output = Artisan::output();
    assert!(output.contains("Pruning tokens with expired expires_at timestamps"));
    assert!(output.contains("Expiration value not specified in configuration file."));
    assert!(output.contains("Tokens expired for more than [24 hours] pruned successfully."));

    Artisan::call("sanctum:prune-expired --hours=1", ())
        .await
        .unwrap();
    assert_eq!(names().await, ["still-valid", "forever"]);
    assert!(Artisan::output().contains("[1 hours]"));
}

#[tokio::test]
async fn tokens_older_than_the_configured_expiration_are_pruned() {
    let _app = app_with(json!({"sanctum": {"expiration": 60}})).await;
    let user = taylor().await;
    travel_to("2025-06-01 00:00:00");
    user.create_token("ancient", &["*"]).await.unwrap();
    travel_to("2025-06-01 10:00:00");
    user.create_token("old", &["*"]).await.unwrap();
    travel_to("2025-06-01 12:00:00");
    user.create_token("new", &["*"]).await.unwrap();

    // Expiration (60 minutes) plus 2 hours of grace: anything created
    // before 09:00 goes.
    Artisan::register(PruneExpired);
    Artisan::call("sanctum:prune-expired --hours=2", ())
        .await
        .unwrap();

    assert_eq!(names().await, ["old", "new"]);
    let output = Artisan::output();
    assert!(
        output.contains("Pruning tokens with expired expiration value based on configuration file")
    );
    assert!(!output.contains("Expiration value not specified"));
}

#[tokio::test]
async fn the_hours_must_be_a_number() {
    let _app = app().await;
    Artisan::register(PruneExpired);

    let code = Artisan::call("sanctum:prune-expired --hours=soon", ())
        .await
        .unwrap();

    assert_eq!(code, 1);
}

#[test]
fn the_command_describes_itself() {
    assert!(
        PruneExpired
            .signature()
            .starts_with("sanctum:prune-expired {--hours=24")
    );
    assert_eq!(
        PruneExpired.description(),
        "Prune tokens expired for more than specified number of hours"
    );
}
