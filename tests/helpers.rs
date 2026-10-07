//! Deferred work, concurrency, processes, and error helpers, through the
//! `laravel` crate.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use laravel::prelude::*;
use laravel::testing::TestApp;

static DEFERRED: AtomicUsize = AtomicUsize::new(0);

fn app() -> TestApp {
    let dir = tempfile::tempdir().unwrap().keep();
    TestApp::new(Application::configure_detached(&dir).with_routing(|routing| {
        routing.web(|| {
            Route::get("/orders", || async {
                defer(async {
                    DEFERRED.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                });
                assert_eq!(DEFERRED.load(Ordering::SeqCst), 0, "deferred work waits for the response");
                "Order placed"
            });

            Route::get("/dashboard", || async {
                let (users, orders) = Concurrency::run((
                    async { Ok(3) },
                    async { Ok("12 orders".to_string()) },
                ))
                .await?;
                Ok::<_, Error>(format!("{users} users, {orders}"))
            });

            Route::get("/repositories", || async {
                let response = Http::with_token("secret", "Bearer")
                    .get("https://api.github.com/user/repos")
                    .await?;
                Ok::<_, Error>(response.json_path("0.name").to_string_lossy())
            });

            Route::get("/deploy", || async {
                let result = Process::run("bash deploy.sh").await?;
                Ok::<_, Error>(result.output().to_string())
            });
        });
    }))
}

#[tokio::test]
async fn deferred_work_runs_after_the_response() {
    let mut app = app();

    app.get("/orders").await.assert_ok().assert_see("Order placed");

    assert_eq!(DEFERRED.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn tasks_run_concurrently() {
    let mut app = app();

    app.get("/dashboard").await.assert_ok().assert_see("3 users, 12 orders");
}

#[tokio::test]
async fn processes_can_be_faked() {
    let mut app = app();
    Process::fake_commands([("bash deploy.sh", Process::result("Deployed!", "", 0))]);

    app.get("/deploy").await.assert_ok().assert_see("Deployed!");

    Process::assert_ran("bash deploy.sh");
}

#[tokio::test]
async fn failures_can_be_rescued() {
    let _app = app();
    let attempts = Arc::new(AtomicUsize::new(0));

    let counter = attempts.clone();
    let total = rescue(
        || async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Err::<u64, _>(laravel::support::error::error!("The payment gateway is down."))
        },
        0,
    )
    .await;

    assert_eq!(total, 0);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn http_requests_can_be_faked() {
    let mut app = app();
    Http::fake_urls([("api.github.com/*", Http::response(json!([{"name": "laravel"}]), 200, &[]))]);

    app.get("/repositories").await.assert_ok().assert_see("laravel");

    Http::assert_sent(|request| {
        request.url() == "https://api.github.com/user/repos" && request.has_header_value("Authorization", "Bearer secret")
    });
}

#[tokio::test]
async fn critical_log_entries_are_sent_to_slack() {
    let app = app();
    app.app().override_config("logging.channels.slack.url", "https://hooks.slack.com/services/T000/B000/XXXX");
    Http::fake();

    let slack = Log::channel("slack");
    slack.info("Somebody signed in.");
    slack.critical("The database is down.");

    // Entries are posted in the background.
    for _ in 0..50 {
        if !Http::recorded().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    Http::assert_sent_count(1);
    Http::assert_sent(|request| {
        let body = request.data();
        request.url() == "https://hooks.slack.com/services/T000/B000/XXXX"
            && body["username"] == "Laravel Log"
            && body["icon_emoji"] == ":boom:"
            && body["attachments"][0]["text"] == "The database is down."
            && body["attachments"][0]["color"] == "danger"
            && body["attachments"][0]["fields"][0]["value"] == "CRITICAL"
    });
}
